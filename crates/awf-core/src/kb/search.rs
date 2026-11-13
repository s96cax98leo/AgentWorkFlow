//! 查詢（FTS5 與純 BM25 兩條路、同一條重排規則）、輸出格式、lint。

use super::index::{fts5_available, load_config, meta, open_ro};
use super::pyish::{self, ErrKind, PyErr};
use super::text::{self, neutralize, query_terms, to_posix};
use super::{B, CLOSE_TAG, COVERAGE_W, DB_NAME, ENTRY_STEMS, K1, MAX_PARSE_LINE, OPEN_TAG, SCHEMA_VERSION, SNIPPET_CHARS, W_ANC, W_BODY, W_HEAD, W_PATH};
use rusqlite::types::Value as Sql;
use rusqlite::{params_from_iter, Connection};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 一筆查詢結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub start: i64,
    pub end: i64,
    pub headings: String,
    pub snippet: String,
    pub score: f64,
    pub stale: bool,
    pub missing: bool,
}

/// 查詢的完整結果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchInfo {
    pub results: Vec<Hit>,
    /// 沒列出的筆數。
    pub truncated: usize,
    /// 有沒有截短節錄。
    pub shortened: bool,
    pub engine: String,
    pub note: String,
}

/// `query()` 的參數。
#[derive(Debug, Clone)]
pub struct QueryOpts {
    pub root: Option<String>,
    pub top: i64,
    pub budget: i64,
    pub engine: String,
    pub kb_dir: Option<String>,
    pub profile: Option<String>,
}

impl Default for QueryOpts {
    fn default() -> Self {
        QueryOpts { root: None, top: 8, budget: 4000, engine: "auto".into(), kb_dir: None, profile: None }
    }
}

fn marks(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// 純 BM25：公式和 FTS5 內建的 bm25() 一樣（含欄位權重）。回傳 [(chunk_id, 分數)]，分數越大越相關。
fn score_bm25(con: &Connection, terms: &[String], limit: usize) -> Result<Vec<(i64, f64)>, PyErr> {
    let (n_rows, total): (i64, i64) = con.query_row("SELECT count(*), coalesce(sum(ntok), 0) FROM chunks", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if n_rows == 0 {
        return Ok(Vec::new());
    }
    let mut avgdl = total as f64 / n_rows as f64;
    if avgdl == 0.0 {
        avgdl = 1.0;
    }
    let sql = format!(
        "SELECT p.term, p.chunk_id, p.th, p.ta, p.tp, p.tb, c.ntok FROM postings p JOIN chunks c ON c.id=p.chunk_id WHERE p.term IN ({})",
        marks(terms.len()));
    let mut st = con.prepare(&sql)?;
    let mut by_term: HashMap<String, Vec<(i64, f64, i64)>> = HashMap::new();
    let rows = st.query_map(params_from_iter(terms.iter()), |r| {
        let (th, ta, tp, tb): (i64, i64, i64, i64) = (r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
        let freq = W_HEAD * th as f64 + W_ANC * ta as f64 + W_PATH * tp as f64 + W_BODY * tb as f64;
        Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, freq, r.get::<_, Option<i64>>(6)?.unwrap_or(0))))
    })?;
    for row in rows {
        let (term, hit) = row?;
        by_term.entry(term).or_default().push(hit);
    }
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for term in terms {
        let Some(rows) = by_term.get(term) else { continue };
        let df = rows.len() as f64;
        let mut idf = ((n_rows as f64 - df + 0.5) / (df + 0.5)).ln();
        if idf <= 0.0 {
            idf = 1e-6;
        }
        for (cid, freq, ntok) in rows {
            let add = idf * (freq * (K1 + 1.0)) / (freq + K1 * (1.0 - B + B * *ntok as f64 / avgdl));
            *scores.entry(*cid).or_insert(0.0) += add;
        }
    }
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    out.truncate(limit);
    Ok(out)
}

fn score_fts5(con: &Connection, terms: &[String], limit: usize) -> Result<Vec<(i64, f64)>, PyErr> {
    let query = terms.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect::<Vec<_>>().join(" OR ");
    let mut st = con.prepare("SELECT rowid, bm25(fts, ?, ?, ?, ?) AS s FROM fts WHERE fts MATCH ? ORDER BY s, rowid LIMIT ?")?;
    let rows = st.query_map(rusqlite::params![W_HEAD, W_ANC, W_PATH, W_BODY, query, i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
        Ok((r.get::<_, i64>(0)?, -r.get::<_, f64>(1)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn snippet(text: &str, has_heading: bool) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if has_heading && lines.len() > 1 && lines.first().is_some_and(|l| l.chars().count() <= MAX_PARSE_LINE && text::head_re().is_match(l)) {
        lines.remove(0);
    }
    let joined = pyish::collapse_ws(&lines.join(" "));
    let s = pyish::strip(&joined);
    if s.chars().count() <= SNIPPET_CHARS { s.to_string() } else { format!("{}…", pyish::take(s, SNIPPET_CHARS - 1)) }
}

fn entry_head(i: usize, r: &Hit) -> String {
    let flag = if r.missing { " ｜ missing" } else if r.stale { " ｜ stale" } else { "" };
    // 檔名裡的換行不能讓輸出另起一行
    let path: String = r.path.chars().map(|c| if (c as u32) < 32 || c as u32 == 127 { ' ' } else { c }).collect();
    format!("{i}. {path}:{}-{} ｜ {}{flag}", r.start, r.end, if r.headings.is_empty() { "（無標題）" } else { &r.headings })
}

/// 輸出總字數不超過 budget：放不下就截短最後一筆的節錄或少列幾筆。回傳（留下的, 有沒有截短節錄）。
fn fit_budget(rows: &[Hit], budget: i64) -> (Vec<Hit>, bool) {
    let len = |s: &str| s.chars().count() as i64;
    let mut used = len(OPEN_TAG) + len(CLOSE_TAG) + 2;
    let reserve = 24; // 留給「已截斷，另有 N 筆」那一行
    let mut out = Vec::new();
    for (k, r) in rows.iter().enumerate() {
        let i = k + 1;
        let head = len(&entry_head(i, r)) + 1;
        let full = head + 3 + len(&r.snippet) + 1;
        let room = budget - used - if i < rows.len() { reserve } else { 0 };
        if full <= room {
            out.push(r.clone());
            used += full;
            continue;
        }
        let avail = budget - used - reserve - head - 4;
        if avail >= 20 {
            let mut cut = r.clone();
            cut.snippet = format!("{}…", pyish::take(&r.snippet, (avail - 1) as usize));
            out.push(cut);
            return (out, true);
        }
        break;
    }
    (out, false)
}

/// 查詢的完整結果。唯讀，不重建索引。
pub fn search(text_q: &str, o: &QueryOpts) -> Result<SearchInfo, PyErr> {
    let cfg = load_config(o.root.as_deref(), o.profile.as_deref(), None, None, o.kb_dir.as_deref())?;
    let root = cfg.root.as_str();
    let db = pyish::pjoin(&cfg.dir, DB_NAME);
    let mut info = SearchInfo::default();
    if !Path::new(&db).is_file() {
        let within = pyish::is_under(&pyish::resolve(&db)?, root);
        let shown = if within { to_posix(&pyish::relpath(&db, root)) } else { DB_NAME.to_string() };
        info.note = format!("索引不存在（{shown}），請主 session 先跑 `kb.py build`");
        return Ok(info);
    }
    let terms = query_terms(text_q);
    if terms.is_empty() {
        info.note = "問題裡沒有可以查的字".into();
        return Ok(info);
    }
    let top = usize::try_from(o.top.max(1)).unwrap_or(1);
    type Row = (String, i64, i64, String, i64, String, i64, i64);
    let (final_, rows): (Vec<(f64, i64)>, HashMap<i64, Row>) = {
        let con = open_ro(&db)?;
        let m = meta(&con)?;
        let get = |k: &str| m.get(k).cloned().flatten();
        if get("schema").as_deref() != Some(SCHEMA_VERSION) {
            info.note = "索引版本不同，請主 session 重跑 `kb.py build`".into();
            return Ok(info);
        }
        let has_fts = get("fts5").as_deref() == Some("1") && fts5_available();
        let mut use_fts = matches!(o.engine.as_str(), "auto" | "fts5") && has_fts;
        if o.engine == "fts5" && !has_fts {
            info.note = "這份索引或這台 Python 沒有 FTS5，改用 bm25".into();
        }
        let limit = top.saturating_mul(25).max(200);
        let mut scored: Vec<(i64, f64)> = Vec::new();
        if use_fts {
            match score_fts5(&con, &terms, limit) {
                Ok(s) => scored = s,
                Err(e) if e.kind == ErrKind::Sqlite => {
                    info.note = format!("FTS5 查詢失敗（{}），改用 bm25", e.class);
                    use_fts = false;
                }
                Err(e) => return Err(e),
            }
        }
        if !use_fts {
            scored = score_bm25(&con, &terms, limit)?;
        }
        info.engine = if use_fts { "fts5" } else { "bm25" }.into();
        if scored.is_empty() {
            return Ok(info);
        }
        let ids: Vec<i64> = scored.iter().map(|x| x.0).collect();
        let mut args: Vec<Sql> = terms.iter().cloned().map(Sql::Text).collect();
        args.extend(ids.iter().map(|i| Sql::Integer(*i)));
        let sql = format!("SELECT chunk_id, count(*) FROM postings WHERE term IN ({}) AND chunk_id IN ({}) GROUP BY chunk_id", marks(terms.len()), marks(ids.len()));
        let mut st = con.prepare(&sql)?;
        let hit: HashMap<i64, i64> = st.query_map(params_from_iter(args.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        // 命中的查詢詞越齊全越前面（兩種引擎用同一條規則重排）
        let nterms = terms.len() as f64;
        let final_: Vec<(f64, i64)> = scored
            .iter()
            .map(|(cid, s)| (s * (1.0 - COVERAGE_W + COVERAGE_W * hit.get(cid).copied().unwrap_or(0) as f64 / nterms), *cid))
            .collect();
        let sql = format!(
            "SELECT c.id, c.path, c.start, c.end, c.headings, c.level, c.text, f.mtime_ns, f.size FROM chunks c JOIN files f ON f.path=c.path WHERE c.id IN ({})",
            marks(ids.len()));
        let mut st = con.prepare(&sql)?;
        let rows: HashMap<i64, Row> = st
            .query_map(params_from_iter(ids.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default(), r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    r.get::<_, Option<String>>(6)?.unwrap_or_default(), r.get::<_, Option<i64>>(7)?.unwrap_or(0), r.get::<_, Option<i64>>(8)?.unwrap_or(0))))
            })?
            .collect::<Result<_, _>>()?;
        (final_, rows)
    };
    let mut ranked: Vec<(f64, &Row)> = final_.iter().filter_map(|(s, cid)| rows.get(cid).map(|r| (pyish::round_nd(*s, 6), r))).collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1 .0.cmp(&b.1 .0)).then_with(|| a.1 .1.cmp(&b.1 .1)));
    let mut results = Vec::new();
    let mut stat_cache: HashMap<&str, &'static str> = HashMap::new();
    for (score, (path, start, end, headings, level, body, mtime_ns, size)) in ranked.iter().take(top) {
        let state = *stat_cache.entry(path.as_str()).or_insert_with(|| match pyish::stat(&pyish::pjoin(root, path)) {
            Ok((m, s)) if m == *mtime_ns && s == *size => "",
            Ok(_) => "stale",
            Err(_) => "missing",
        });
        results.push(Hit { path: neutralize(path), start: *start, end: *end, headings: neutralize(headings),
            snippet: neutralize(&snippet(body, *level != 0)), score: pyish::round_nd(*score, 4), stale: !state.is_empty(), missing: state == "missing" });
    }
    let (kept, shortened) = fit_budget(&results, o.budget.max(0));
    info.truncated = if kept.len() < results.len() { ranked.len() - kept.len() } else { 0 };
    info.results = kept;
    info.shortened = shortened;
    Ok(info)
}

/// 給其他工具用。不回錯誤：索引不存在、資料庫壞掉都回空清單，原因放在 `note`。
pub fn query(text_q: &str, o: &QueryOpts) -> SearchInfo {
    search(text_q, o).unwrap_or_else(|e| SearchInfo { note: format!("查詢失敗（{}：{}）", e.class, pyish::take(&e.msg, 120)), ..Default::default() })
}

pub fn format_results(info: &SearchInfo) -> String {
    let mut out = vec![OPEN_TAG.to_string()];
    for (k, r) in info.results.iter().enumerate() {
        out.push(entry_head(k + 1, r));
        out.push(format!("   {}", r.snippet));
    }
    if info.results.is_empty() && info.truncated == 0 {
        out.push("（沒有符合的段落）".into());
    }
    if info.truncated > 0 {
        out.push(format!("（已截斷，另有 {} 筆）", info.truncated));
    } else if info.shortened {
        out.push("（已截斷：節錄已截短）".into());
    }
    out.push(CLOSE_TAG.to_string());
    out.join("\n")
}

/// `json.dumps(results, ensure_ascii=False, indent=1)`
pub fn results_json(results: &[Hit]) -> String {
    if results.is_empty() {
        return "[]".into();
    }
    let items: Vec<String> = results
        .iter()
        .map(|r| {
            let b = |v: bool| if v { "true" } else { "false" };
            format!(" {{\n  \"path\": {},\n  \"start\": {},\n  \"end\": {},\n  \"headings\": {},\n  \"snippet\": {},\n  \"score\": {},\n  \"stale\": {},\n  \"missing\": {}\n }}",
                pyish::json_str(&r.path), r.start, r.end, pyish::json_str(&r.headings), pyish::json_str(&r.snippet), crate::pyfmt::float(r.score), b(r.stale), b(r.missing))
        })
        .collect();
    format!("[\n{}\n]", items.join(",\n"))
}

// ------------------------------------------------------------------ lint
#[derive(Debug, Clone, PartialEq)]
pub struct Broken {
    pub path: String,
    pub line: i64,
    pub target: String,
    /// "md" 或 "wiki"
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LintResult {
    pub broken: Vec<Broken>,
    pub orphans: Vec<String>,
    pub note: String,
}

fn file_stem_fold(p: &str) -> String {
    pyish::casefold(pyish::splitext_stem(pyish::basename(p)))
}

/// 壞連結與孤兒頁。唯讀。
pub fn lint(root: Option<&str>, kb_dir: Option<&str>, profile: Option<&str>) -> Result<LintResult, PyErr> {
    let cfg = load_config(root, profile, None, None, kb_dir)?;
    let root = cfg.root.as_str();
    let db = pyish::pjoin(&cfg.dir, DB_NAME);
    let mut res = LintResult::default();
    if !Path::new(&db).is_file() {
        res.note = "索引不存在，請主 session 先跑 `kb.py build`".into();
        return Ok(res);
    }
    let (paths, links): (Vec<String>, Vec<(String, i64, String, String)>) = {
        let con = open_ro(&db)?;
        let mut st = con.prepare("SELECT path FROM files ORDER BY path")?;
        let paths = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut st = con.prepare("SELECT path, line, kind, target FROM links ORDER BY path, line")?;
        let links = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
        (paths, links)
    };
    let known: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut by_stem: HashMap<String, Vec<&str>> = HashMap::new();
    for p in &paths {
        by_stem.entry(file_stem_fold(p)).or_default().push(p);
    }
    let mut inbound: HashSet<String> = HashSet::new();
    let norm = |base_dir: &str, target: &str| -> String {
        let joined = if target.starts_with('/') {
            target.trim_start_matches('/').to_string()
        } else if base_dir.is_empty() {
            target.to_string()
        } else {
            format!("{base_dir}/{target}")
        };
        to_posix(&pyish::normpath(&joined))
    };
    let exists = |p: &str| !p.contains('\0') && std::fs::metadata(p).is_ok();

    for (path, line, kind, target) in &links {
        let base_dir = path.rsplit_once('/').map(|x| x.0).unwrap_or("");
        let mut hits: Vec<String> = Vec::new();
        if kind == "md" {
            let cand = norm(base_dir, target);
            if known.contains(cand.as_str()) {
                hits.push(cand);
            } else if (!cand.starts_with("..") && exists(&pyish::pjoin(root, &cand)))
                || (cand.starts_with("..") && exists(&pyish::pjoin(&pyish::pjoin(root, base_dir), target)))
            {
                hits.push(String::new());
            }
        } else {
            let t = if target.to_lowercase().ends_with(".md") { pyish::take(target, target.chars().count().saturating_sub(3)) } else { target.as_str() };
            for cand in [format!("{t}.md"), norm(base_dir, &format!("{t}.md"))] {
                if known.contains(cand.as_str()) {
                    hits.push(cand);
                }
            }
            if hits.is_empty() {
                let tail = format!("/{}.md", pyish::casefold(t));
                hits = paths.iter().filter(|p| pyish::casefold(p).ends_with(&tail)).cloned().collect();
                if hits.is_empty() && !t.contains('/') {
                    hits = by_stem.get(&pyish::casefold(t)).map(|v| v.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
                }
            }
            if hits.is_empty() && [target.clone(), norm(base_dir, target)].iter().any(|c| exists(&pyish::pjoin(root, c))) {
                hits.push(String::new());
            }
        }
        if hits.is_empty() {
            res.broken.push(Broken { path: path.clone(), line: *line, target: target.clone(), kind: kind.clone() });
        }
        inbound.extend(hits.into_iter().filter(|h| !h.is_empty() && h != path));
    }
    for p in &paths {
        if !inbound.contains(p) && !ENTRY_STEMS.contains(&file_stem_fold(p).as_str()) {
            res.orphans.push(p.clone());
        }
    }
    Ok(res)
}
