//! 交付文件用到知識圖的部分（D7 的 deliver、D9）。都是「有圖才做、只會更嚴」：
//! 沒有圖、圖開不了時，這裡每個函式都不做事，原本的輸出逐字不變。
//!
//! - [`extend_affected`]：deliver 段的受影響清單，多列「改到的檔在圖裡連到的小節」。
//! - [`Links`]：小節記錄它連到的需求、情境、符號的指紋；之後指紋變了而內文沒動，那一節算過期。
//! - [`project_matrix`]：專案層的追溯矩陣（每個現行需求目前由哪些檔案、測試、commit 支撐）。

use super::core::*;
use super::*;
use crate::kg::{flow, flow::Loc, Graph};
use crate::pyfmt::truthy;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// deliver：改到的檔（`owned`，目標 repo 的相對路徑）在圖裡連到的交付文件小節，原本的清單沒有的就加進去。
/// 只加交付設定裡有的文件與小節、更新時機是每次交付的；外部文件、追溯矩陣、開發紀錄不加。
/// 回傳（要寫進資料包的那一節, 多列了幾個小節）。
pub(super) fn extend_affected(g: &Graph, prefix: Option<&str>, cfg: &Value, root: &str, owned: &[String], aff: &mut Vec<Value>) -> (Vec<String>, usize) {
    let Some(prefix) = prefix else { return (Vec::new(), 0) };
    let paths: Vec<String> = owned.iter().map(|p| posix(p)).filter(|p| !p.is_empty() && !p.starts_with('/') && !p.split('/').any(|c| c == "..")).map(|p| format!("{prefix}{p}")).collect();
    let files = flow::known_files(g, &paths);
    let found = flow::doc_sections(g, &files);
    if found.is_empty() {
        return (Vec::new(), 0);
    }
    let cat = load_catalog(root);
    let docs = resolve_docs(cfg, &cat);
    let mut locs: Vec<Loc> = Vec::new();
    for (path, title, why) in found {
        if locs.len() >= flow::MAX_LINKS {
            break;
        }
        let Some(d) = docs.iter().find(|d| rel_to(&doc_path(cfg, root, d), root) == path) else { continue };
        let id = sget(d, "id");
        let when = get(d, "when");
        if sget(d, "source") == "external" || id == MATRIX_ID || id == RECORD_ID || (truthy(when) && when != "run") || !strs(get(d, "sections")).contains(&title) {
            continue;
        }
        match aff.iter_mut().find(|a| sget(a, "id") == id) {
            Some(a) => {
                if strs(get(a, "sections")).contains(&title) {
                    continue;
                }
                if let Some(secs) = a.get_mut("sections").and_then(Value::as_array_mut) {
                    secs.push(json!(title));
                }
            }
            None => {
                let depth = if truthy(get(d, "depth")) { get(d, "depth").clone() } else { json!("full") };
                aff.push(json!({"id": id, "title": get(d, "title"), "file": path, "depth": depth, "source": get(d, "source"), "why": format!("知識圖：{why}"),
                    "sections": [title], "sources": iter_any(get(d, "sources"))}));
            }
        }
        locs.push(Loc { path, start: 0, end: 0, why: format!("小節「{title}」：{why}") });
    }
    let n = locs.len();
    (flow::section("知識圖：依連結多列的交付文件小節（只憑改動路徑看不出來，這次一併檢查）", &[("小節", &locs)]), n)
}

/// 小節連到的需求、情境、符號（從知識圖）。
pub struct Links(Graph);

impl Links {
    /// 專案有圖而且能開才有。
    pub fn open(root: &str) -> Option<Links> {
        flow::open_root(root).map(Links)
    }

    /// 記錄用：這個小節現在連到的節點與指紋。沒有連結回 None（紀錄裡就不放這個鍵）。
    pub fn record(&self, doc_rel: &str, title: &str) -> Option<Value> {
        let m = flow::section_links(&self.0, doc_rel, title);
        (!m.is_empty()).then(|| json!(m))
    }

    /// 來源紀錄裡記過的連結，現在指紋不同的（或節點不見了的）。沒記過連結回空。
    pub fn changed(&self, rec: &Value) -> Vec<String> {
        let Some(old) = rec.get("links").and_then(Value::as_object) else { return Vec::new() };
        let recorded: BTreeMap<String, String> = old.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect();
        flow::changed_links(&self.0, &recorded)
    }
}

/// 在原本的判定之後多看一項：原本算沒過期（fresh）的小節，記過的連結有變就改成過期；
/// 內文也改過的話算「還沒記錄」（和來源改了的情況同一套規則：sync 會記成最新）。其他狀態原樣回傳（只會更嚴）。
pub(super) fn linked_state(base: (&'static str, String), doc_id: &str, sec: &Section, state: &Value, links: Option<&Links>) -> (&'static str, String) {
    let Some(links) = links.filter(|_| base.0 == "fresh") else { return base };
    let rec = get(get(state, doc_id), &sec.title);
    let changed = links.changed(rec);
    if changed.is_empty() {
        return base;
    }
    if get(rec, "body").as_str() != Some(body_hash(&sec.body).as_str()) {
        return ("unstamped", "內文改過，還沒記錄來源（跑 awf deliverables sync）".to_string());
    }
    let shown: Vec<&str> = changed.iter().map(String::as_str).take(4).collect();
    ("stale", format!("連到的需求或符號改了、內文沒動：{}", shown.join("、")))
}

/// 專案層的追溯矩陣。沒有圖回 Err（說明）。
pub fn project_matrix(root: &str) -> Result<(String, Value), String> {
    let g = flow::open_root(root).ok_or_else(|| "這個專案沒有知識圖（或圖開不了）：專案層的追溯矩陣要從圖算，先跑 awf kg build；單一 run 的矩陣用 awf deliverables matrix --run <run 目錄>".to_string())?;
    let rows = flow::requirement_rows(&g);
    let cell = |v: &[String]| if v.is_empty() { "—".to_string() } else { v.iter().map(|x| x.replace('|', "／")).collect::<Vec<_>>().join("、") };
    let mut lines = vec![
        "# 專案追溯矩陣（現行需求）".to_string(),
        String::new(),
        "從知識圖算出來的：連結帳本（驗收條件連到的檔案、測試、commit）與程式裡的 `@awf` 標記。圖是上次 `awf kg build` 時的樣子。".to_string(),
        String::new(),
        "| 需求 | 規格位置 | 情境數 | 檔案 | 測試 | commit |".to_string(),
        "|---|---|---|---|---|---|".to_string(),
    ];
    let mut bare = 0usize;
    for r in &rows {
        if r.files.is_empty() && r.tests.is_empty() && r.commits.is_empty() {
            bare += 1;
        }
        lines.push(format!("| {}（{}） | {} | {} | {} | {} | {} |", r.id, r.title.replace('|', "／"), r.spec, r.scenarios, cell(&r.files), cell(&r.tests), cell(&r.commits)));
    }
    if rows.is_empty() {
        lines.push("| — | — | — | — | — | — |".to_string());
    }
    lines.push(String::new());
    lines.push(format!("現行需求 {} 個，其中 {bare} 個沒有任何檔案、測試或 commit 連到。", rows.len()));
    let js: Vec<Value> = rows.iter().map(|r| json!({"id": r.id, "title": r.title, "spec": r.spec, "scenarios": r.scenarios, "files": r.files, "tests": r.tests, "commits": r.commits})).collect();
    Ok((format!("{}\n", lines.join("\n")), json!({"requirements": js, "unsupported": bare})))
}
