//! 建圖與增量更新（`awf kg build`）。
//!
//! 流程：把上一版的圖複製成暫存檔 → 只重算變了的檔（先比大小與修改時間，再比 sha256）→ 全域來源與跨檔推導整個重算
//! → 改名蓋掉舊圖。超過時間上限就丟掉暫存檔，上一版原封不動。

use super::code::{self, Lang};
use super::resolve::{self, Aux};
use super::sources;
use super::store::{self, e, NodeRow};
use crate::kb::{self, pyish};
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 原始檔超過這個大小就略過。
pub const MAX_CODE_BYTES: u64 = 1024 * 1024;
/// 預設的時間上限（秒）。
pub const DEFAULT_TIMEOUT: u64 = 60;
/// 一個節點最多存幾個不同的詞。
const MAX_TERMS: usize = 600;

#[derive(Debug, Clone, Default)]
pub struct BuildOpts {
    pub root: Option<String>,
    pub profile: Option<String>,
    /// 圖放在哪個目錄（預設是 profile 的 `knowledge.dir`，再沒有就是 `_tpm/kb`）。
    pub dir: Option<String>,
    /// 規格根目錄（沒給用 profile 的 `knowledge.spec_root`，再沒有就是 `openspec`）。
    pub spec_root: Option<String>,
    pub full: bool,
    /// 時間上限（秒）；None 用 profile 的 `knowledge.timeout`，再沒有就是 [`DEFAULT_TIMEOUT`]。
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Updated,
    Unchanged,
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct BuildReport {
    pub outcome: Outcome,
    pub db: String,
    pub files: usize,
    pub changed: usize,
    pub removed: usize,
    pub nodes: usize,
    pub edges: usize,
    pub unresolved: usize,
    pub skipped: Vec<(String, String)>,
    pub bad_links: usize,
    pub notes: Vec<String>,
}

/// 別的 repo（profile 的 `knowledge.repos`）：只讀、只收 `sources` 列出的 glob、機敏檔不收。
#[derive(Debug, Clone, PartialEq)]
pub struct RepoSrc {
    /// 節點 id 與圖裡的路徑用的前綴（`<id>:`）。
    pub id: String,
    /// 這個 repo 的根目錄（已解開符號連結的絕對路徑）。
    pub abs: String,
    /// 設定裡寫的路徑（輸出時用它拼出讀得到的路徑）。
    pub display: String,
    pub sources: Vec<String>,
}

/// repo id 的寫法：小寫英數與連字號，字母開頭。
pub fn repo_id_ok(id: &str) -> bool {
    let mut cs = id.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase()) && id.len() <= 32 && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 路徑開頭的 `<repo id>:`（別的 repo 的檔）→（id, 其餘）。
pub fn split_repo(path: &str) -> Option<(&str, &str)> {
    let (id, rest) = path.split_once(':')?;
    (repo_id_ok(id) && !id.contains('/')).then_some((id, rest))
}

/// 目錄配置。
pub struct Layout {
    /// 專案根目錄（已解開符號連結的絕對路徑）。
    pub root: String,
    /// 圖所在的目錄（絕對路徑）。
    pub dir: String,
    pub db: String,
    /// runs_dir（相對於根目錄；沒有設定是空字串）。
    pub runs: String,
    /// 規格根目錄（絕對路徑）與顯示用的寫法。
    pub spec_root: String,
    pub spec_display: String,
    pub cfg: kb::Config,
    /// 別的 repo（`knowledge.repos`）；設定不合規則的項目略過並記在 `notes`。
    pub repos: Vec<RepoSrc>,
    /// profile 的 `knowledge.timeout`（秒）。
    pub timeout: Option<u64>,
    pub notes: Vec<String>,
}

/// profile 的 `knowledge` 物件；讀不到或不是物件回空的。
fn knowledge_of(root: &str, profile: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let p = match profile.filter(|p| !p.is_empty()) {
        Some(p) => pyish::ppath(p),
        None => pyish::pjoin(root, ".claude/teams/methodology.json"),
    };
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("knowledge").and_then(|k| k.as_object()).cloned())
        .unwrap_or_default()
}

fn repos_of(root: &str, know: &serde_json::Map<String, serde_json::Value>, notes: &mut Vec<String>) -> Vec<RepoSrc> {
    let mut out: Vec<RepoSrc> = Vec::new();
    let Some(list) = know.get("repos").and_then(|v| v.as_array()) else { return out };
    for (i, r) in list.iter().enumerate() {
        let text = |k: &str| r.get(k).and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
        let (id, path) = (text("id"), kb::to_posix(text("path")));
        let sources: Vec<String> = r.get("sources").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()).map(|g| kb::to_posix(g.trim())).filter(|g| !g.is_empty()).collect();
        let bad = |why: &str, notes: &mut Vec<String>| notes.push(format!("knowledge.repos[{i}] 略過：{why}"));
        if !repo_id_ok(id) || out.iter().any(|x| x.id == id) {
            bad("id 要是小寫英數與連字號、字母開頭，而且不能重複", notes);
            continue;
        }
        if path.is_empty() || path.contains('\0') || sources.is_empty() {
            bad("要有 path 與至少一個 sources", notes);
            continue;
        }
        if sources.iter().any(|g| g.starts_with('/') || g.split('/').any(|c| c == "..") || g.chars().nth(1) == Some(':')) {
            bad("sources 要是相對於那個 repo 的 glob，不能是絕對路徑或含 ..", notes);
            continue;
        }
        let joined = if pyish::is_abs(&path) { pyish::ppath(&path) } else { pyish::pjoin(root, &path) };
        let abs = match std::fs::canonicalize(&joined) {
            Ok(p) if p.is_dir() => kb::to_posix(&p.to_string_lossy()),
            _ => {
                bad(&format!("找不到目錄 {}", pyish::take(&path, 120)), notes);
                continue;
            }
        };
        if abs == root {
            bad("path 就是專案自己（專案自己的檔本來就會收）", notes);
            continue;
        }
        out.push(RepoSrc { id: id.to_string(), abs, display: path.trim_end_matches('/').to_string(), sources });
    }
    out
}

pub fn layout(root: Option<&str>, profile: Option<&str>, dir: Option<&str>, spec_root: Option<&str>) -> Result<Layout, String> {
    let cfg = kb::load_config(root, profile, None, None, dir).map_err(|err| format!("讀不到專案設定（{}）", err.msg))?;
    if !Path::new(&cfg.root).is_dir() {
        return Err(format!("專案根目錄不存在：{}", cfg.root));
    }
    let runs = kb::guard_dir(&cfg.root, "runs_dir", "_tpm/runs");
    let know = knowledge_of(&cfg.root, profile);
    let mut notes = Vec::new();
    let from_profile = know.get("spec_root").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let spec = spec_root.filter(|s| !s.is_empty()).or(from_profile).unwrap_or("openspec");
    crate::spec::check_root_arg(spec).map_err(|e| if spec_root.is_some_and(|s| !s.is_empty()) { e.replace("--root", "--spec-root") } else { e.replace("--root", "knowledge.spec_root") })?;
    let timeout = know.get("timeout").and_then(|v| v.as_u64()).filter(|t| (1..=3600).contains(t));
    let repos = repos_of(&cfg.root, &know, &mut notes);
    let spec_abs = if pyish::is_abs(spec) { pyish::ppath(spec) } else { pyish::pjoin(&cfg.root, spec) };
    let spec_display = if pyish::is_under(&spec_abs, &cfg.root) { kb::to_posix(&pyish::relpath(&spec_abs, &cfg.root)) } else { kb::to_posix(spec) };
    Ok(Layout { db: pyish::pjoin(&cfg.dir, store::DB_NAME), root: cfg.root.clone(), dir: cfg.dir.clone(), runs, spec_root: spec_abs, spec_display, cfg, repos, timeout, notes })
}

pub(crate) enum Fail {
    Timeout,
    Msg(String),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Msg(s)
    }
}

pub(crate) struct Clock {
    start: Instant,
    limit: Duration,
}

impl Clock {
    pub(crate) fn check(&self) -> Result<(), Fail> {
        if self.start.elapsed() >= self.limit { Err(Fail::Timeout) } else { Ok(()) }
    }
}

/// 識別字拆成單字：`getUserName`、`get_user_name` → 原字加上 `get user name`。
pub fn ident_words(name: &str) -> String {
    let mut out = String::from(name);
    out.push(' ');
    let cs: Vec<char> = name.chars().collect();
    for (i, c) in cs.iter().enumerate() {
        let boundary = i > 0 && c.is_uppercase() && (cs[i - 1].is_lowercase() || cs[i - 1].is_ascii_digit() || cs.get(i + 1).is_some_and(|n| n.is_lowercase()) && cs[i - 1].is_uppercase());
        if boundary || !c.is_alphanumeric() {
            out.push(' ');
        }
        if c.is_alphanumeric() {
            out.push(*c);
        }
    }
    out
}

pub(crate) fn add_terms(map: &mut BTreeMap<String, i64>, text: &str, w: i64) -> usize {
    let toks = kb::tokenize(text, false);
    let n = toks.len();
    for t in toks {
        *map.entry(t).or_insert(0) += w;
    }
    n
}

pub(crate) fn cap_terms(map: BTreeMap<String, i64>) -> BTreeMap<String, i64> {
    if map.len() <= MAX_TERMS {
        return map;
    }
    let mut v: Vec<(String, i64)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(MAX_TERMS);
    v.into_iter().collect()
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() { 0 } else { text.matches('\n').count() + usize::from(!text.ends_with('\n')) }
}

fn file_node(con: &Connection, rel: &str, sub: &str, nlines: usize) -> Result<String, String> {
    let id = format!("file:{rel}");
    let mut terms = BTreeMap::new();
    let ntok = add_terms(&mut terms, &ident_words(pyish::splitext_stem(rel)), 2);
    store::put_node(con, &NodeRow { id: &id, kind: "file", sub, name: rel, path: rel, start: 1, end: nlines.max(1), ntok, owner: rel })?;
    store::put_terms(con, &id, &terms, rel)?;
    Ok(id)
}

/// 同一個檔裡重複的 id 加 `~2`、`~3`…
fn unique_id(seen: &mut HashMap<String, usize>, id: String) -> String {
    let n = seen.entry(id.clone()).or_insert(0);
    *n += 1;
    if *n == 1 { id } else { format!("{id}~{n}") }
}

fn index_code(con: &Connection, rel: &str, lang: Lang, text: &str) -> Result<usize, String> {
    let nlines = count_lines(text);
    let file_id = file_node(con, rel, lang.name(), nlines)?;
    let syms = code::parse(lang, text).unwrap_or_default();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut first: HashMap<String, String> = HashMap::new();
    let stem = ident_words(pyish::splitext_stem(pyish::basename(rel)));
    let src_lines: Vec<&str> = text.split('\n').collect();
    for d in &syms.defs {
        let q = d.qualified();
        let id = unique_id(&mut seen, format!("sym:{rel}#{q}"));
        let mut terms = BTreeMap::new();
        let mut ntok = add_terms(&mut terms, &ident_words(&d.name), 3);
        ntok += add_terms(&mut terms, &ident_words(&d.parent), 1);
        ntok += add_terms(&mut terms, &stem, 1);
        store::put_node(con, &NodeRow { id: &id, kind: "sym", sub: d.kind, name: &q, path: rel, start: d.start, end: d.end, ntok, owner: rel })?;
        store::put_terms(con, &id, &terms, rel)?;
        store::put_fp(con, &id, &src_lines, d.start, d.end, rel)?;
        let parent = first.get(&d.parent).cloned().unwrap_or_else(|| file_id.clone());
        store::put_edge(con, &parent, &id, "contains", "strong", "code", 1.0, rel)?;
        first.entry(q).or_insert(id);
    }
    let mut st = con.prepare_cached("INSERT INTO f_imports VALUES(?,?,?,?,?)").map_err(e)?;
    for (k, i) in syms.imports.iter().enumerate() {
        st.execute(params![rel, k as i64, i.module, resolve::pack_names(&i.names), i.line as i64]).map_err(e)?;
    }
    let mut st = con.prepare_cached("INSERT INTO f_calls VALUES(?,?,?,?,?,?)").map_err(e)?;
    let mut dedupe: HashSet<(&str, &str, &str)> = HashSet::new();
    for (k, c) in syms.calls.iter().enumerate() {
        if dedupe.insert((&c.name, &c.qual, &c.scope)) {
            st.execute(params![rel, k as i64, c.name, c.qual, c.scope, c.line as i64]).map_err(e)?;
        }
    }
    let mut st = con.prepare_cached("INSERT INTO f_inherits VALUES(?,?,?,?,?,?)").map_err(e)?;
    for (k, i) in syms.inherits.iter().enumerate() {
        st.execute(params![rel, k as i64, i.scope, i.target, i.kind, i.line as i64]).map_err(e)?;
    }
    let mut st = con.prepare_cached("INSERT INTO f_refs VALUES(?,?,?,?,?,?)").map_err(e)?;
    for (k, m) in syms.marks.iter().enumerate() {
        let scope = first.get(&m.scope).cloned().unwrap_or_else(|| file_id.clone());
        st.execute(params![rel, k as i64, "mark", m.target, scope, m.line as i64]).map_err(e)?;
    }
    Ok(nlines)
}

/// 反引號裡像路徑或 id 的字串（要有 `/`，不含空白）。
fn ticks(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(a) = rest.find('`') {
        let after = &rest[a + 1..];
        let Some(b) = after.find('`') else { break };
        let span = &after[..b];
        if span.contains('/') && span.len() <= 200 && span.len() >= 3 && !span.contains(char::is_whitespace) {
            out.push(span);
        }
        rest = &after[b + 1..];
    }
    out
}

fn index_doc(con: &Connection, rel: &str, text: &str, src: &str) -> Result<usize, String> {
    let low = rel.to_lowercase();
    let markdown = kb::MARKDOWN_EXTS.iter().any(|x| low.ends_with(x));
    let doc = kb::split_document(text, markdown);
    let file_id = file_node(con, rel, src, doc.nlines)?;
    struct Sec {
        start: usize,
        end: usize,
        level: usize,
        headings: Vec<String>,
        terms: BTreeMap<String, i64>,
        ntok: usize,
    }
    let mut secs: BTreeMap<usize, Sec> = BTreeMap::new();
    for ch in &doc.chunks {
        let s = secs.entry(ch.sec).or_insert_with(|| {
            let mut terms = BTreeMap::new();
            let n = ch.headings.len();
            let mut ntok = add_terms(&mut terms, ch.headings.last().map(String::as_str).unwrap_or(""), 3);
            ntok += add_terms(&mut terms, &ch.headings.get(..n.saturating_sub(1)).unwrap_or(&[]).join(" "), 1);
            Sec { start: ch.start, end: ch.end, level: ch.level, headings: ch.headings.clone(), terms, ntok }
        });
        s.start = s.start.min(ch.start);
        s.end = s.end.max(ch.end);
        s.ntok += add_terms(&mut s.terms, &ch.text, 1);
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for s in secs.into_values() {
        let id = unique_id(&mut seen, format!("sec:{rel}#{}", s.headings.join("/")));
        let name = s.headings.last().cloned().unwrap_or_default();
        store::put_node(con, &NodeRow { id: &id, kind: "sec", sub: &s.level.to_string(), name: &name, path: rel, start: s.start, end: s.end, ntok: s.ntok, owner: rel })?;
        store::put_terms(con, &id, &cap_terms(s.terms), rel)?;
        store::put_edge(con, &file_id, &id, "contains", "strong", "kb", 1.0, rel)?;
        spans.push((s.start, s.end, id));
    }
    let at = |ln: usize| spans.iter().find(|s| s.0 <= ln && ln <= s.1).map(|s| s.2.clone()).unwrap_or_else(|| file_id.clone());
    let mut refs: Vec<(&str, String, String, usize)> = Vec::new();
    for (ln, kind, target) in &doc.links {
        refs.push((kind, target.clone(), at(*ln), *ln));
    }
    for (k, line) in text.split('\n').enumerate() {
        if line.len() <= kb::MAX_PARSE_LINE {
            for t in ticks(line) {
                refs.push(("tick", t.to_string(), at(k + 1), k + 1));
            }
        }
    }
    let mut dedupe: HashSet<(String, String, String)> = HashSet::new();
    let mut st = con.prepare_cached("INSERT INTO f_refs VALUES(?,?,?,?,?,?)").map_err(e)?;
    for (k, (kind, target, scope, ln)) in refs.iter().enumerate() {
        if dedupe.insert((kind.to_string(), target.clone(), scope.clone())) {
            st.execute(params![rel, k as i64, kind, target, scope, *ln as i64]).map_err(e)?;
        }
    }
    Ok(doc.nlines)
}

struct Src {
    rel: String,
    abs: String,
    src: &'static str,
    lang: Option<Lang>,
}

fn sensitive(rel: &str) -> bool {
    kb::is_sensitive(pyish::basename(rel)) || crate::hooks::guard::is_secret_path(rel)
}

/// 程式檔：照 .gitignore 走訪（不需要是 git repo），略過隱藏目錄、相依套件目錄、圖自己的目錄。
fn scan_code(lay: &Layout, out: &mut Vec<Src>, aux: &mut Aux, skipped: &mut Vec<(String, String)>, clock: &Clock) -> Result<(), Fail> {
    let dir_abs = lay.dir.clone();
    let mut wb = ignore::WalkBuilder::new(&lay.root);
    wb.hidden(true).git_ignore(true).git_exclude(false).git_global(false).require_git(false).follow_links(false).sort_by_file_name(|a, b| a.cmp(b));
    wb.filter_entry(move |ent| {
        let name = ent.file_name().to_string_lossy();
        !(kb::PRUNE_DIRS.contains(&name.as_ref()) || ent.path().to_string_lossy() == dir_abs.as_str())
    });
    for (k, ent) in wb.build().enumerate() {
        if k % 256 == 0 {
            clock.check()?;
        }
        let Ok(ent) = ent else { continue };
        if !ent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(abs) = ent.path().to_str() else { continue };
        let rel = kb::to_posix(&pyish::relpath(abs, &lay.root));
        if rel.starts_with("../") {
            continue;
        }
        match pyish::basename(&rel) {
            "tsconfig.json" | "jsconfig.json" => aux.tsconfigs.push(rel.clone()),
            "go.mod" => aux.gomods.push(rel.clone()),
            "Cargo.toml" => aux.cargos.push(rel.clone()),
            _ => {}
        }
        let Some(lang) = Lang::from_path(&rel) else { continue };
        if sensitive(&rel) {
            skipped.push((rel, "機敏檔".into()));
            continue;
        }
        match ent.metadata() {
            Ok(m) if m.len() > MAX_CODE_BYTES => {
                skipped.push((rel, "超過 1 MB".into()));
                continue;
            }
            Err(_) => {
                skipped.push((rel, "讀不到".into()));
                continue;
            }
            _ => {}
        }
        out.push(Src { rel, abs: abs.to_string(), src: "code", lang: Some(lang) });
    }
    Ok(())
}

/// 別的 repo：只收 `sources` 的 glob 對得上的檔（程式檔與文字檔），照 .gitignore 走訪；機敏檔、連到 repo 外的連結不收。
/// 圖裡的路徑是 `<repo id>:<相對於那個 repo 的路徑>`。
fn scan_repo(repo: &RepoSrc, lay: &Layout, out: &mut Vec<Src>, skipped: &mut Vec<(String, String)>, clock: &Clock) -> Result<(), Fail> {
    let globs: Vec<regex::Regex> = repo.sources.iter().filter_map(|g| kb::glob_regex(g)).collect();
    let dir_abs = lay.dir.clone();
    let mut wb = ignore::WalkBuilder::new(&repo.abs);
    wb.hidden(true).git_ignore(true).git_exclude(false).git_global(false).require_git(false).follow_links(false).sort_by_file_name(|a, b| a.cmp(b));
    wb.filter_entry(move |ent| {
        let name = ent.file_name().to_string_lossy();
        !(kb::PRUNE_DIRS.contains(&name.as_ref()) || ent.path().to_string_lossy() == dir_abs.as_str())
    });
    for (k, ent) in wb.build().enumerate() {
        if k % 256 == 0 {
            clock.check()?;
        }
        let Ok(ent) = ent else { continue };
        if !ent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(abs) = ent.path().to_str() else { continue };
        let inner = kb::to_posix(&pyish::relpath(abs, &repo.abs));
        if inner.starts_with("../") || !globs.iter().any(|g| g.is_match(&inner)) {
            continue;
        }
        let lang = Lang::from_path(&inner);
        let low = inner.to_lowercase();
        if lang.is_none() && !kb::TEXT_EXTS.iter().any(|x| low.ends_with(x)) {
            continue;
        }
        let rel = format!("{}:{inner}", repo.id);
        if sensitive(&inner) {
            skipped.push((rel, "機敏檔".into()));
            continue;
        }
        // 連結的名字正常、指到的卻在那個 repo 外面或是機敏檔：不收
        match std::fs::canonicalize(abs) {
            Ok(real) => {
                let real = kb::to_posix(&real.to_string_lossy());
                if !pyish::is_under(&real, &repo.abs) {
                    skipped.push((rel, "連到 repo 外".into()));
                    continue;
                }
                if sensitive(&real) {
                    skipped.push((rel, "機敏檔".into()));
                    continue;
                }
            }
            Err(_) => {
                skipped.push((rel, "讀不到".into()));
                continue;
            }
        }
        match ent.metadata() {
            Ok(m) if m.len() > MAX_CODE_BYTES => {
                skipped.push((rel, "超過 1 MB".into()));
                continue;
            }
            Err(_) => {
                skipped.push((rel, "讀不到".into()));
                continue;
            }
            _ => {}
        }
        out.push(Src { rel, abs: abs.to_string(), src: if lang.is_some() { "code" } else { "doc" }, lang });
    }
    Ok(())
}

/// 符號連結的名字正常、指到的卻是機敏檔或被排除的位置：解開後用實際的路徑再過一次機敏檔與排除規則。
/// 不是連結（解開後就是自己）回 None。
fn link_refusal(abs: &str, root: &str, excludes: &[regex::Regex]) -> Option<&'static str> {
    let Ok(real) = std::fs::canonicalize(abs) else { return Some("讀不到") };
    let real = kb::to_posix(&real.to_string_lossy());
    let root_real = std::fs::canonicalize(root).map(|r| kb::to_posix(&r.to_string_lossy())).unwrap_or_else(|_| root.to_string());
    if !pyish::is_under(&real, &root_real) {
        return Some("連到專案外");
    }
    let rel = kb::to_posix(&pyish::relpath(&real, &root_real));
    if sensitive(&rel) || rel.split('/').any(|seg| kb::is_sensitive(seg) || matches!(seg.to_lowercase().as_str(), "secrets" | "secret" | ".ssh" | ".aws" | ".gnupg")) {
        return Some("連到機敏檔");
    }
    if excludes.iter().any(|rx| rx.is_match(&rel)) || rel.split('/').any(|seg| kb::PRUNE_DIRS.contains(&seg)) {
        return Some("連到被排除的位置");
    }
    None
}

fn walk_md(dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ents: Vec<_> = rd.flatten().collect();
    ents.sort_by_key(|x| x.file_name());
    for ent in ents {
        let Ok(ft) = ent.file_type() else { continue };
        let p = ent.path();
        if ft.is_dir() {
            walk_md(&p, out, depth + 1);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "md") {
            if let Some(s) = p.to_str() {
                out.push(s.to_string());
            }
        }
    }
}

fn collect(lay: &Layout, aux: &mut Aux, skipped: &mut Vec<(String, String)>, clock: &Clock) -> Result<Vec<Src>, Fail> {
    let mut out = Vec::new();
    scan_code(lay, &mut out, aux, skipped, clock)?;
    let (docs, skip) = kb::scan(&lay.cfg, kb::MAX_BYTES).map_err(|err| format!("掃文件失敗（{}）", err.msg))?;
    skipped.extend(skip);
    let excludes: Vec<regex::Regex> = lay.cfg.exclude.iter().filter_map(|p| kb::glob_regex(p)).collect();
    for (rel, abs) in docs {
        if let Some(why) = link_refusal(&abs, &lay.root, &excludes) {
            skipped.push((rel, why.into()));
            continue;
        }
        out.push(Src { rel, abs, src: "doc", lang: None });
    }
    let mut notes = Vec::new();
    walk_md(Path::new(&pyish::pjoin(&lay.dir, "vault/notes")), &mut notes, 0);
    for abs in notes {
        let rel = if pyish::is_under(&abs, &lay.root) { kb::to_posix(&pyish::relpath(&abs, &lay.root)) } else { format!("@kg/{}", kb::to_posix(&pyish::relpath(&abs, &lay.dir))) };
        if sensitive(&rel) {
            skipped.push((rel, "機敏檔".into()));
            continue;
        }
        out.push(Src { rel, abs, src: "note", lang: None });
    }
    for repo in &lay.repos {
        scan_repo(repo, lay, &mut out, skipped, clock)?;
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out.dedup_by(|a, b| a.rel == b.rel);
    skipped.sort();
    Ok(out)
}

struct Counts {
    files: usize,
    changed: usize,
    removed: usize,
    touched: bool,
}

fn update_files(con: &Connection, srcs: &[Src], skipped: &mut Vec<(String, String)>, clock: &Clock) -> Result<Counts, Fail> {
    let mut have: HashMap<String, (String, i64, i64, String)> = HashMap::new();
    {
        let mut st = con.prepare("SELECT path, src, size, mtime_ns, sha256 FROM files").map_err(e)?;
        let mut rows = st.query([]).map_err(e)?;
        while let Some(r) = rows.next().map_err(e)? {
            have.insert(r.get(0).map_err(e)?, (r.get(1).map_err(e)?, r.get(2).map_err(e)?, r.get(3).map_err(e)?, r.get(4).map_err(e)?));
        }
    }
    let mut c = Counts { files: 0, changed: 0, removed: 0, touched: false };
    let mut live: HashSet<&str> = HashSet::new();
    for s in srcs {
        clock.check()?;
        let Ok(md) = std::fs::metadata(&s.abs) else {
            skipped.push((s.rel.clone(), "讀不到".into()));
            continue;
        };
        let size = md.len() as i64;
        let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0);
        let old = have.get(&s.rel);
        if old.is_some_and(|o| o.0 == s.src && o.1 == size && o.2 == mtime) {
            live.insert(&s.rel);
            c.files += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(&s.abs) else {
            skipped.push((s.rel.clone(), "讀不到".into()));
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            skipped.push((s.rel.clone(), "不是 UTF-8 文字".into()));
            continue;
        };
        if text.contains('\0') {
            skipped.push((s.rel.clone(), "二進位檔".into()));
            continue;
        }
        live.insert(&s.rel);
        c.files += 1;
        c.touched = true;
        let sha = store::sha256_hex(text.as_bytes());
        if old.is_some_and(|o| o.0 == s.src && o.3 == sha) {
            con.execute("UPDATE files SET size=?, mtime_ns=? WHERE path=?", params![size, mtime, s.rel]).map_err(e)?;
            continue;
        }
        store::delete_owner(con, &s.rel)?;
        let nlines = match s.lang {
            Some(lang) => index_code(con, &s.rel, lang, &text)?,
            None => index_doc(con, &s.rel, &text, s.src)?,
        };
        let lang = s.lang.map(Lang::name).unwrap_or("");
        let is_test = s.lang.is_some() && code::is_test_path(&s.rel);
        con.execute("INSERT INTO files VALUES(?,?,?,?,?,?,?,?)", params![s.rel, s.src, lang, size, mtime, sha, nlines as i64, is_test as i64]).map_err(e)?;
        c.changed += 1;
    }
    let mut gone: Vec<&String> = have.keys().filter(|p| !live.contains(p.as_str())).collect();
    gone.sort();
    for p in gone {
        store::delete_owner(con, p)?;
        c.removed += 1;
        c.touched = true;
    }
    Ok(c)
}

fn rm(p: &str) {
    let _ = std::fs::remove_file(p);
    let _ = std::fs::remove_file(format!("{p}-journal"));
}

fn run(con: &Connection, lay: &Layout, full: bool, clock: &Clock, rep: &mut BuildReport) -> Result<bool, Fail> {
    let before = store::meta(con)?;
    store::set_meta(con, "index_state", store::STATE_BUILDING)?;
    con.execute_batch("BEGIN").map_err(e)?;
    let mut aux = Aux::default();
    let srcs = collect(lay, &mut aux, &mut rep.skipped, clock)?;
    let c = update_files(con, &srcs, &mut rep.skipped, clock)?;
    clock.check()?;
    sources::git_facts(con, &lay.root, full, &before, &mut rep.notes)?;
    clock.check()?;
    store::delete_global(con)?;
    sources::specs(con, lay, &mut rep.notes)?;
    sources::decisions(con, lay)?;
    clock.check()?;
    let world = resolve::World::load(con, &lay.root, &aux)?;
    resolve::link_code(con, &world)?;
    clock.check()?;
    sources::refs(con, &world)?;
    sources::git_edges(con, &world)?;
    rep.bad_links = sources::ledger(con, lay, &world, &mut rep.notes)?;
    clock.check()?;
    con.execute("DELETE FROM meta WHERE key LIKE 'repo:%'", []).map_err(e)?;
    for r in &lay.repos {
        store::set_meta(con, &format!("repo:{}", r.id), &r.display)?;
    }
    let hash = store::graph_hash(con)?;
    for (k, v) in [
        ("schema_version", store::SCHEMA_VERSION),
        ("content_version", store::CONTENT_VERSION),
        ("graph_hash", hash.as_str()),
        ("files_total", &c.files.to_string()),
        ("links_bad_lines", &rep.bad_links.to_string()),
        ("index_state", store::STATE_READY),
    ] {
        store::set_meta(con, k, v)?;
    }
    con.execute_batch("COMMIT").map_err(e)?;
    rep.files = c.files;
    rep.changed = c.changed;
    rep.removed = c.removed;
    rep.nodes = store::count(con, "nodes");
    rep.edges = store::count(con, "edges");
    rep.unresolved = store::count(con, "unresolved");
    let same_graph = store::usable(&before).is_ok() && before.get("graph_hash") == Some(&hash) && c.changed == 0 && c.removed == 0;
    let same_head = before.get("git_head") == store::meta(con)?.get("git_head");
    rep.outcome = if same_graph { Outcome::Unchanged } else { Outcome::Updated };
    // 內容沒變但檔案的修改時間變了：仍然寫回，下次才不用再算一次雜湊
    Ok(!(same_graph && same_head && !c.touched))
}

/// 建圖或增量更新。Err 是失敗的原因；逾時不是 Err，看 `outcome`。
pub fn build(o: &BuildOpts) -> Result<BuildReport, String> {
    let start = Instant::now();
    let lay = layout(o.root.as_deref(), o.profile.as_deref(), o.dir.as_deref(), o.spec_root.as_deref())?;
    let clock = Clock { start, limit: Duration::from_secs(o.timeout.or(lay.timeout).unwrap_or(DEFAULT_TIMEOUT)) };
    // 專案另外加的機敏檔樣式（guard.config.json 的 secret_files_extra；和 publish、hook、kb 同一份）。
    // `sensitive` 走 hook 的 is_secret_path，所以放進同一個執行緒區域變數；設定用不了就不建圖（分不出哪些檔是機敏的）
    let extra = crate::config::SecretExtra::for_project(&lay.root);
    if let Some(e) = extra.errors.first() {
        return Err(format!("機敏檔的設定用不了（{e}）：不建圖，請先修正 .claude/teams/guard.config.json"));
    }
    crate::hooks::guard::set_secret_files_extra(extra.files);
    pyish::mkdir_p(&lay.dir).map_err(|err| format!("建不出目錄 {}（{}）", lay.dir, err.msg))?;
    let tmp = format!("{}.tmp-{}", lay.db, std::process::id());
    rm(&tmp);
    let prev_ok = !o.full && Path::new(&lay.db).is_file() && store::open_ro(&lay.db).and_then(|c| store::meta(&c)).is_ok_and(|m| store::usable(&m).is_ok());
    if prev_ok {
        std::fs::copy(&lay.db, &tmp).map_err(|err| format!("複製上一版的圖失敗（{err}）"))?;
    }
    let mut rep = BuildReport { outcome: Outcome::Updated, db: lay.db.clone(), files: 0, changed: 0, removed: 0, nodes: 0, edges: 0, unresolved: 0, skipped: Vec::new(), bad_links: 0, notes: lay.notes.clone() };
    let result = store::open_rw(&tmp, !prev_ok).map_err(Fail::Msg).and_then(|con| {
        let r = run(&con, &lay, o.full || !prev_ok, &clock, &mut rep);
        drop(con);
        r
    });
    match result {
        Ok(true) => {
            if let Ok(f) = std::fs::File::open(&tmp) {
                let _ = f.sync_all();
            }
            std::fs::rename(&tmp, &lay.db).map_err(|err| {
                rm(&tmp);
                format!("寫不進 {}（{err}）", lay.db)
            })?;
            rm(&tmp);
        }
        Ok(false) => rm(&tmp),
        Err(Fail::Timeout) => {
            rm(&tmp);
            rep.outcome = Outcome::TimedOut;
        }
        Err(Fail::Msg(m)) => {
            rm(&tmp);
            return Err(m);
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let toks = |s: &str| kb::tokenize(&ident_words(s), false);
        assert_eq!(toks("getUserName"), vec!["getusername", "get", "user", "name"]);
        assert_eq!(toks("export_invoice"), vec!["export", "invoice", "export", "invoice"]);
        assert_eq!(toks("HTTPServer"), vec!["httpserver", "http", "server"]);
        assert_eq!(ticks("看 `src/a.ts` 與 `billing/export-invoice#csv`，不是 `x`、`a b/c`"), vec!["src/a.ts", "billing/export-invoice#csv"]);
    }
}
