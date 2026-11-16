//! 給流程接點用的查詢（`awf pack`、`awf deliverables`）：資料包要帶的行段、交付文件小節連到的東西、專案層的追溯矩陣。
//!
//! 全部唯讀、不建圖（建圖由 `awf kg build` 或流程的自動接點做）。圖不存在或開不了時 [`open_root`] 回 None，
//! 呼叫端就照沒有圖的做法走。結果只有「路徑:起訖行＋一句理由」，條數有上限，同一張圖查兩次結果相同。

use super::build::{layout, split_repo};
use super::query::{impact, label, locate_scored, neutral, open, short, Graph, Item, Node, CLOSE_TAG, OPEN_TAG};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// plan：程式行段、相關決策、會碰到的既有需求與情境各最多幾條。
pub const MAX_CODE: usize = 12;
pub const MAX_DECISIONS: usize = 6;
pub const MAX_REQS: usize = 10;
/// design：連著要改的檔的需求、測試各最多幾條。
pub const MAX_LINKS: usize = 12;
/// build：每個子任務的呼叫者、相依各最多幾條。
pub const MAX_CALLERS: usize = 8;
pub const MAX_DEPS: usize = 8;
/// 一個行段最多幾行（和工具先定位的上限一致）。
pub const MAX_SEGMENT_LINES: usize = 120;
/// 程式角度算「夠」的門檻：用詞直接命中的符號至少幾個、分布在至少幾個檔。
pub const ENOUGH_SYMBOLS: usize = 3;
pub const ENOUGH_FILES: usize = 2;

/// 一個位置：`路徑:起訖行` 與一句理由。`end` 是 0 表示沒有行號（只印路徑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub why: String,
}

/// 開專案的圖。沒有圖、版本不同、沒建完、讀不到設定都回 None（不建圖、不印訊息）。
pub fn open_root(root: &str) -> Option<Graph> {
    // 讀設定時的警告不往外印：呼叫端（資料包、交付文件）自己已經讀過一次設定，這裡再印會重複
    let prev = crate::kb::swap_err_sink(Some(String::new()));
    let g = layout(Some(root), None, None, None).ok().and_then(|lay| open(&lay).ok());
    crate::kb::swap_err_sink(prev);
    g
}

fn loc(g: &Graph, i: &Item) -> Loc {
    Loc { path: g.readable(&i.path), start: i.start, end: i.end, why: format!("{} {}：{}", label(&i.kind), short(&i.id), i.reason) }
}

fn is_code(g: &Graph, path: &str) -> bool {
    g.con.prepare_cached("SELECT src FROM files WHERE path=?").and_then(|mut s| s.query_row([path], |r| r.get::<_, String>(0))).is_ok_and(|s| s == "code")
}

/// 圖裡記的大小與修改時間和現在的檔一樣（圖建好之後這個檔沒動過）。
fn fresh(g: &Graph, path: &str) -> bool {
    let rec: Option<(i64, i64)> = g.con.prepare_cached("SELECT size, mtime_ns FROM files WHERE path=?").and_then(|mut s| s.query_row([path], |r| Ok((r.get(0)?, r.get(1)?)))).ok();
    let Some((size, mtime)) = rec else { return false };
    let Ok(md) = std::fs::metadata(format!("{}/{path}", g.root)) else { return false };
    let now = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(-1);
    md.len() as i64 == size && now == mtime
}

/// 這個符號或它所在的檔，有沒有宣告出來的連結接到需求（連結帳本的 AC → 檔案／測試，或 `@awf` 標記）。
fn linked_to_requirement(g: &Graph, n: &Node) -> bool {
    let file = format!("file:{}", n.path);
    g.in_edges(&file).iter().chain(g.in_edges(&n.id).iter()).any(|e| matches!(e.kind.as_str(), "ac_file" | "ac_test"))
        || g.out_edges(&n.id).iter().any(|e| e.kind == "marks")
        || g.con.prepare_cached("SELECT 1 FROM edges e JOIN nodes s ON s.id = e.src WHERE e.kind='marks' AND s.path=? LIMIT 1").and_then(|mut s| s.exists([&n.path])).unwrap_or(false)
}

/// plan 段的資料。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanFacts {
    /// 程式行段（不含測試檔）。
    pub code: Vec<Loc>,
    pub decisions: Vec<Loc>,
    /// 這個需求會碰到的既有需求與情境。
    pub reqs: Vec<Loc>,
    /// 用詞直接命中、在專案自己的 repo 裡、圖建好後沒動過的符號（給工具先定位用；路徑是圖裡的寫法）。
    pub direct: Vec<Loc>,
    /// `direct` 裡至少有一個接得到需求（連結帳本或 `@awf` 標記）。
    pub linked: bool,
}

impl PlanFacts {
    pub fn is_empty(&self) -> bool {
        self.code.is_empty() && self.decisions.is_empty() && self.reqs.is_empty()
    }

    /// 程式角度算不算「夠」（可以不派定位 agent）。保守：四個條件都要成立，另外呼叫端還要核對工具自己的定位有重疊。
    /// 1. 用詞直接命中的符號（不是沿連結擴展出來的）至少 [`ENOUGH_SYMBOLS`] 個；
    /// 2. 分布在至少 [`ENOUGH_FILES`] 個檔；
    /// 3. 這些檔在圖建好之後沒有動過（`direct` 只收這種）；
    /// 4. 其中至少一個接得到需求（帳本或標記），也就是圖裡有證據說這些程式和需求有關。
    pub fn code_enough(&self) -> bool {
        let files: BTreeSet<&str> = self.direct.iter().map(|l| l.path.as_str()).collect();
        self.direct.len() >= ENOUGH_SYMBOLS && files.len() >= ENOUGH_FILES && self.linked
    }
}

/// plan：依需求的文字找程式行段、相關決策、會碰到的既有需求與情境。`names` 是這個 run 可能對應的變更名稱（變更動到的需求排最前面）。
pub fn plan_facts(g: &Graph, text: &str, names: &[String]) -> PlanFacts {
    let mut out = PlanFacts::default();
    let mut seen: HashSet<String> = HashSet::new();
    for name in names.iter().filter(|n| !n.trim().is_empty()) {
        let chg = format!("chg:{}", name.trim());
        for ed in g.out_edges(&chg).into_iter().filter(|e| e.kind == "changes") {
            let Some(n) = g.node(&ed.dst) else { continue };
            if out.reqs.len() < MAX_REQS && seen.insert(n.id.clone()) {
                out.reqs.push(Loc { path: g.readable(&n.path), start: n.start, end: n.end, why: format!("{} {}：進行中的變更 {} 動到它", label(&n.kind), short(&n.id), neutral(name.trim())) });
            }
        }
    }
    let (ans, seeds) = locate_scored(g, text, 60);
    for i in &ans.items {
        match i.kind.as_str() {
            "sym" | "file" if is_code(g, &i.path) => {
                if g.is_test(&i.path) {
                    continue;
                }
                if out.code.len() < MAX_CODE {
                    out.code.push(loc(g, i));
                }
                if i.kind == "sym" && seeds.contains(&i.id) && split_repo(&i.path).is_none() && fresh(g, &i.path) {
                    if let Some(n) = g.node(&i.id) {
                        out.linked = out.linked || linked_to_requirement(g, &n);
                    }
                    out.direct.push(Loc { path: i.path.clone(), start: i.start, end: i.end.min(i.start + MAX_SEGMENT_LINES - 1), why: format!("知識圖：{}", i.reason) });
                }
            }
            "dec" if out.decisions.len() < MAX_DECISIONS => out.decisions.push(loc(g, i)),
            "req" | "scn" | "cap" if out.reqs.len() < MAX_REQS && seen.insert(i.id.clone()) => out.reqs.push(loc(g, i)),
            _ => {}
        }
    }
    out.direct.truncate(MAX_CODE);
    out
}

/// 圖裡有的檔（照給的順序、不重複）。`paths` 可以是輸出裡看到的寫法。
pub fn known_files(g: &Graph, paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        if let Some(n) = g.find(p).into_iter().find(|n| n.kind == "file") {
            if !out.contains(&n.path) {
                out.push(n.path);
            }
        }
    }
    out
}

/// 圖裡路徑以 `prefix` 開頭的程式檔（目錄或 glob 的固定前綴；最多 `cap` 個）。
pub fn files_under(g: &Graph, prefix: &str, cap: usize) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let all = g.strings("SELECT path FROM files WHERE src='code' AND substr(path, 1, length(?1)) = ?1 ORDER BY path", prefix);
    all.into_iter().take(cap).collect()
}

fn group(g: &Graph, items: &[Item], name: &str, cap: usize) -> Vec<Loc> {
    items.iter().filter(|i| i.group == name).take(cap).map(|i| Loc { path: i.path.clone(), start: i.start, end: i.end, why: format!("{} {}：{}", label(&i.kind), short(&i.id), i.reason) }).map(|mut l| {
        l.path = g.readable(&l.path);
        l
    }).collect()
}

/// design：這些檔現在被哪些需求、測試連著。回傳（需求與情境, 測試）。
pub fn file_links(g: &Graph, files: &[String]) -> (Vec<Loc>, Vec<Loc>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let ans = impact(g, files);
    (group(g, &ans.items, "需求", MAX_LINKS), group(g, &ans.items, "受影響的測試", MAX_LINKS))
}

/// build：這些檔的呼叫者（引用它的程式）、會受影響的測試、相依（它用到的別的檔；同一個行段只列一次）。
pub fn callers_and_deps(g: &Graph, files: &[String]) -> (Vec<Loc>, Vec<Loc>, Vec<Loc>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let ans = impact(g, files);
    let callers = group(g, &ans.items, "引用它的程式", MAX_CALLERS);
    let tests = group(g, &ans.items, "受影響的測試", MAX_CALLERS);
    let own: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut deps: Vec<Loc> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in files {
        let mut members = vec![format!("file:{f}")];
        members.extend(g.strings("SELECT id FROM nodes WHERE owner=? AND kind='sym' ORDER BY id", f));
        for m in &members {
            for ed in g.out_edges(m) {
                if !matches!(ed.kind.as_str(), "imports" | "calls" | "extends" | "implements") {
                    continue;
                }
                let Some(n) = g.node(&ed.dst) else { continue };
                if own.contains(n.path.as_str()) || n.path.is_empty() || !seen.insert(n.id.clone()) {
                    continue;
                }
                let path = g.readable(&n.path);
                if deps.len() < MAX_DEPS && !deps.iter().any(|d| d.path == path && d.start == n.start && d.end == n.end) {
                    deps.push(Loc { path, start: n.start, end: n.end, why: format!("{} {}：{} 用到它（{}）", label(&n.kind), short(&n.id), short(m), ed.kind) });
                }
            }
        }
    }
    (callers, tests, deps)
}

/// deliver：改這些檔會碰到的文件小節（做過相關需求的 run 改過的交付文件小節、提到這些檔或需求的文件小節）。
/// 回傳（相對於專案根目錄的檔, 小節標題, 理由）；呼叫端只留交付設定裡的文件。
pub fn doc_sections(g: &Graph, files: &[String]) -> Vec<(String, String, String)> {
    if files.is_empty() {
        return Vec::new();
    }
    let ans = impact(g, files);
    let mut out: Vec<(String, String, String)> = Vec::new();
    for i in ans.items.iter().filter(|i| i.group == "交付文件小節" || i.group == "文件小節") {
        let Some(n) = g.node(&i.id) else { continue };
        let title = match n.kind.as_str() {
            "sec" => n.name.clone(),
            "dlv" => n.name.split_once('#').map(|x| x.1.to_string()).unwrap_or_default(),
            _ => continue,
        };
        if n.path.is_empty() || title.is_empty() || out.iter().any(|x| x.0 == n.path && x.1 == title) {
            continue;
        }
        out.push((n.path.clone(), title, i.reason.clone()));
    }
    out
}

/// 資料包裡的一節：標題、資料標籤、各組的行段。各組都是空的就回空（不加這一節）。
pub fn section(title: &str, groups: &[(&str, &[Loc])]) -> Vec<String> {
    if groups.iter().all(|g| g.1.is_empty()) {
        return Vec::new();
    }
    let mut out = vec![format!("## {title}"), String::new(), OPEN_TAG.to_string()];
    for (name, locs) in groups.iter().filter(|g| !g.1.is_empty()) {
        out.push(format!("〔{name}〕"));
        for l in locs.iter() {
            let at = if l.end == 0 { l.path.clone() } else { format!("{}:{}-{}", l.path, l.start, l.end) };
            out.push(neutral(&format!("- {at} ｜ {}", l.why)));
        }
    }
    out.push(CLOSE_TAG.to_string());
    out
}

// ---------------------------------------------------------------- 交付文件：小節連到的需求與符號
fn fp_of(g: &Graph, id: &str) -> Option<String> {
    if let Some(path) = id.strip_prefix("file:") {
        return g.con.prepare_cached("SELECT sha256 FROM files WHERE path=?").and_then(|mut s| s.query_row([path], |r| r.get::<_, String>(0))).ok().map(|h| h.chars().take(16).collect());
    }
    g.con.prepare_cached("SELECT hash FROM fps WHERE node=?").and_then(|mut s| s.query_row([id], |r| r.get::<_, String>(0))).ok()
}

/// 一個交付文件小節（二級標題）連到的需求、情境、檔案與符號，以及它們現在的指紋。
/// 連結來源：小節內文提到的 id 與路徑（含底下的子小節）、改過這個小節的 run 所做的需求。最多 40 個。
pub fn section_links(g: &Graph, doc: &str, title: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let secs: Vec<String> = (|| -> rusqlite::Result<Vec<String>> {
        let mut st = g.con.prepare_cached("SELECT id FROM nodes WHERE kind='sec' AND path=?1 AND name=?2 AND sub='2' ORDER BY start LIMIT 1")?;
        let rows = st.query_map([doc, title], |r| r.get::<_, String>(0))?;
        rows.collect()
    })()
    .unwrap_or_default();
    let Some(top) = secs.first() else { return out };
    let mut members = vec![top.clone()];
    members.extend(g.strings("SELECT id FROM nodes WHERE kind='sec' AND substr(id, 1, length(?1) + 1) = ?1 || '/' ORDER BY id", top));
    let mut targets: BTreeSet<String> = BTreeSet::new();
    for m in &members {
        for ed in g.out_edges(m) {
            // 記需求、情境、符號與程式檔；文件檔不記（文件之間互相提到很常見，一改就連動太吵）
            if matches!(ed.kind.as_str(), "mentions" | "links") && ed.dst.strip_prefix("file:").is_none_or(|p| is_code(g, p)) {
                targets.insert(ed.dst);
            }
        }
        for ed in g.in_edges(m).into_iter().filter(|e| e.kind == "change_deliverable") {
            for r in g.in_edges(&ed.src).into_iter().filter(|e| e.kind == "req_run") {
                targets.insert(r.src);
            }
        }
    }
    for t in targets.into_iter().take(40) {
        if let Some(fp) = fp_of(g, &t) {
            out.insert(t, fp);
        }
    }
    out
}

/// 記錄過的連結裡，現在指紋不一樣（或節點不見了）的那些。只看記錄過的 id：新多出來的連結不算變動。
pub fn changed_links(g: &Graph, recorded: &BTreeMap<String, String>) -> Vec<String> {
    recorded.iter().filter(|(id, old)| fp_of(g, id).as_deref() != Some(old.as_str())).map(|(id, _)| id.clone()).collect()
}

// ---------------------------------------------------------------- 專案層的追溯矩陣
/// 一個現行需求目前由哪些檔案、測試、commit 支撐。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqRow {
    pub id: String,
    pub title: String,
    /// 規格檔的位置（`路徑:起訖行`）。
    pub spec: String,
    pub scenarios: usize,
    pub files: Vec<String>,
    pub tests: Vec<String>,
    pub commits: Vec<String>,
}

/// 每個現行需求一列（照 id 排序）。連結取自連結帳本（AC → 檔案／測試／commit）與 `@awf` 標記，含需求底下的情境。
pub fn requirement_rows(g: &Graph) -> Vec<ReqRow> {
    let mut out = Vec::new();
    for id in g.strings("SELECT id FROM nodes WHERE kind='req' AND sub=? ORDER BY id", "current") {
        let Some(n) = g.node(&id) else { continue };
        let mut ids = vec![id.clone()];
        ids.extend(g.out_edges(&id).into_iter().filter(|e| e.kind == "contains" && e.provenance == "spec").map(|e| e.dst));
        let (mut files, mut tests, mut commits) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for x in &ids {
            for ed in g.out_edges(x) {
                let dst = g.node(&ed.dst);
                let shown = dst.as_ref().filter(|d| !d.path.is_empty()).map(|d| g.readable(&d.path)).unwrap_or_else(|| short(&ed.dst).to_string());
                match ed.kind.as_str() {
                    "ac_file" if dst.as_ref().is_some_and(|d| g.is_test(&d.path)) => {
                        tests.insert(shown);
                    }
                    "ac_file" => {
                        files.insert(if dst.is_some() { shown } else { format!("{shown}（不在圖裡）") });
                    }
                    "ac_test" => {
                        tests.insert(shown);
                    }
                    "ac_commit" => {
                        commits.insert(short(&ed.dst).chars().take(12).collect::<String>());
                    }
                    _ => {}
                }
            }
            for ed in g.in_edges(x).into_iter().filter(|e| e.kind == "marks") {
                let Some(src) = g.node(&ed.src) else { continue };
                if g.is_test(&src.path) {
                    tests.insert(g.readable(&src.path));
                } else {
                    files.insert(g.readable(&src.path));
                }
            }
        }
        out.push(ReqRow {
            id: short(&id).to_string(),
            title: n.name.clone(),
            spec: format!("{}:{}-{}", g.readable(&n.path), n.start, n.end),
            scenarios: ids.len() - 1,
            files: files.into_iter().collect(),
            tests: tests.into_iter().collect(),
            commits: commits.into_iter().collect(),
        });
    }
    out
}
