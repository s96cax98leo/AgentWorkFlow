//! 規格層給流程用的介面（S4）：需求來源 adapter `openspec`、存檔與閘門的檢查都從這裡拿資料。
//!
//! 這裡只整理資料，不做判定：一個變更有哪些需求與情境（驗收條件）、哪些 id 存在、
//! proposal 的節怎麼對到 intent 的必要節。判定在 `save_run` 與 `merge_gate`。

use super::read::{ChangeState, Requirement, Tree};
use serde_json::Value;
use std::collections::BTreeSet;

/// 一個變更要驗的一條：情境（驗收條件），或沒有情境的需求本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 穩定 id：`<能力>/<需求代稱>#<情境代稱>`，沒有情境的需求是 `<能力>/<需求代稱>`。
    pub id: String,
    /// 所屬需求的 id。
    pub requirement: String,
    /// 「需求標題／情境標題」。
    pub title: String,
    /// 相對於規格根目錄的檔。
    pub file: String,
    pub line: usize,
    pub is_scenario: bool,
}

/// proposal 的節對照表裡的特殊項目：變更的規格差異裡至少有一個情境，就算有這一節。
pub const SCENARIOS_TOKEN: &str = "@scenarios";

/// 內建的對照表：intent 的必要節 → proposal 裡算數的節名（標題含這個字就算；OpenSpec 慣用的英文節名）。
/// 表上沒有的必要節只認同名的標題。profile 的 `requirements.section_map` 可以整項覆寫。
pub const DEFAULT_SECTION_MAP: &[(&str, &[&str])] = &[
    ("目標", &["Why"]),
    ("範圍", &["What Changes"]),
    ("不做的事", &["Non-Goals", "Non Goals", "Out of Scope"]),
    ("驗收條件", &["Acceptance Criteria", "Success Criteria", SCENARIOS_TOKEN]),
];

/// 進行中的變更（已歸檔的不算）。
pub fn active_changes(tree: &Tree) -> Vec<&str> {
    tree.changes.iter().filter(|c| c.state == ChangeState::Active).map(|c| c.name.as_str()).collect()
}

fn push_items(out: &mut Vec<Item>, r: &Requirement) {
    if r.id.is_empty() {
        return;
    }
    if r.scenarios.is_empty() {
        out.push(Item { id: r.id.clone(), requirement: r.id.clone(), title: r.title.clone(), file: r.file.clone(), line: r.span.start, is_scenario: false });
    }
    for s in r.scenarios.iter().filter(|s| !s.id.is_empty()) {
        out.push(Item { id: s.id.clone(), requirement: r.id.clone(), title: format!("{}／{}", r.title, s.title), file: r.file.clone(), line: s.span.start, is_scenario: true });
    }
}

/// 一個進行中的變更要驗的條目：ADDED 與 MODIFIED 的需求底下的情境（沒有情境的需求算它自己一條）。
/// REMOVED、只有 RENAMED 的需求沒有東西要驗。找不到這個變更回 None。id 要先由 `ids::assign` 填好。
pub fn change_items(tree: &Tree, change: &str) -> Option<Vec<Item>> {
    let ch = tree.change(change, ChangeState::Active)?;
    let mut out = Vec::new();
    for d in &ch.deltas {
        for r in d.added.iter().chain(d.modified.iter()) {
            push_items(&mut out, r);
        }
    }
    let mut seen = BTreeSet::new();
    out.retain(|i| seen.insert(i.id.clone()));
    Some(out)
}

/// 一個變更動到的需求 id（ADDED、MODIFIED）。
pub fn change_requirements(tree: &Tree, change: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(ch) = tree.change(change, ChangeState::Active) {
        for d in &ch.deltas {
            for r in d.added.iter().chain(d.modified.iter()).filter(|r| !r.id.is_empty()) {
                if !out.contains(&r.id) {
                    out.push(r.id.clone());
                }
            }
        }
    }
    out
}

/// 找得到的 id：現行規格的需求與情境，加上指定變更的差異（ADDED、MODIFIED）裡的。
pub fn known_ids(tree: &Tree, change: Option<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut add = |r: &Requirement| {
        if !r.id.is_empty() {
            out.insert(r.id.clone());
        }
        for s in r.scenarios.iter().filter(|s| !s.id.is_empty()) {
            out.insert(s.id.clone());
        }
    };
    for cap in &tree.capabilities {
        cap.requirements.iter().for_each(&mut add);
    }
    if let Some(ch) = change.and_then(|n| tree.change(n, ChangeState::Active)) {
        for d in &ch.deltas {
            d.added.iter().chain(d.modified.iter()).for_each(&mut add);
        }
    }
    out
}

/// id 在哪裡：（相對於規格根目錄的檔、起行、迄行、標題）。變更的差異優先（那是這次要做的版本），再找現行規格。
pub fn locate(tree: &Tree, change: Option<&str>, id: &str) -> Option<(String, usize, usize, String)> {
    if let Some(ch) = change.and_then(|n| tree.change(n, ChangeState::Active)) {
        for d in &ch.deltas {
            for r in d.added.iter().chain(d.modified.iter()) {
                if r.id == id {
                    return Some((r.file.clone(), r.span.start, r.span.end, r.title.clone()));
                }
                if let Some(s) = r.scenarios.iter().find(|s| s.id == id) {
                    return Some((r.file.clone(), s.span.start, s.span.end, s.title.clone()));
                }
            }
        }
    }
    tree.find(id).map(|f| (f.file, f.span.start, f.span.end, f.title))
}

/// 一個必要節在 proposal 裡算數的節名：節名自己，加上對照表的（`overrides` 有這一項就用它的，否則用內建的）。
pub fn section_aliases(overrides: Option<&Value>, section: &str) -> Vec<String> {
    let mut out = vec![section.to_string()];
    let own = overrides.and_then(Value::as_object).and_then(|o| o.get(section)).and_then(Value::as_array);
    let extra: Vec<String> = match own {
        Some(list) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => DEFAULT_SECTION_MAP.iter().find(|(k, _)| *k == section).map(|(_, v)| v.iter().map(|x| (*x).to_string()).collect()).unwrap_or_default(),
    };
    for a in extra {
        if !a.trim().is_empty() && !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// proposal 缺哪些必要節。每個必要節用 [`section_aliases`] 的節名去找：其中一個有標題而且底下有內容就算有；
/// 對照到 [`SCENARIOS_TOKEN`] 時，變更的差異裡有情境（`scenarios > 0`）也算有。對不到就是缺。
pub fn proposal_missing(text: &str, need: &[String], overrides: Option<&Value>, scenarios: usize) -> Vec<String> {
    let mut miss = Vec::new();
    for sec in need {
        let mut found = false;
        let mut empty_heading = false;
        for name in section_aliases(overrides, sec) {
            if name == SCENARIOS_TOKEN {
                found = found || scenarios > 0;
                continue;
            }
            match crate::recipe::missing_sections(text, &Value::Array(vec![Value::String(name)])) {
                Ok(m) if m.is_empty() => found = true,
                Ok(m) => empty_heading = empty_heading || m.iter().any(|x| x.as_str().is_some_and(|t| t.contains("只有標題沒有內容"))),
                Err(_) => {}
            }
            if found {
                break;
            }
        }
        if !found {
            miss.push(if empty_heading { format!("{sec}（只有標題沒有內容）") } else { sec.clone() });
        }
    }
    miss
}

/// 需求來源 `openspec` 的設定（guard.config.json 或 profile 的 `requirements`）。
#[derive(Debug, Clone)]
pub struct SourceCfg {
    /// 規格根目錄（絕對路徑，或接在專案根目錄後面）。
    pub root: std::path::PathBuf,
    /// 顯示用的寫法（設定裡寫的）。
    pub display: String,
    /// 專案自己的需求編號樣式（`requirements.id_pattern`）。
    pub pattern: Option<regex::Regex>,
    /// 設定裡指定的變更名稱（`requirements.change`）；沒指定是空字串。
    pub change: String,
    /// proposal 的節對照表（`requirements.section_map`）；沒設是 None。
    pub section_map: Option<Value>,
}

/// 讀 `requirements` 設定：`path` 是規格根目錄（預設 `openspec`；相對於專案根目錄，也可以是另一個 repo 的絕對路徑）。
pub fn source_cfg(rc: &Value, proj: &str) -> Result<SourceCfg, String> {
    let text = |k: &str| -> Result<String, String> {
        match rc.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.trim().to_string()),
            Some(_) => Err(format!("requirements.{k} 要是字串")),
        }
    };
    let mut display = text("path")?.replace('\\', "/");
    if display.is_empty() || display.contains("{{") {
        display = "openspec".to_string();
    }
    super::check_root_arg(&display).map_err(|e| e.replace("--root", "requirements.path"))?;
    let root = if std::path::Path::new(&display).is_absolute() { std::path::PathBuf::from(&display) } else { std::path::Path::new(proj).join(&display) };
    let pattern = match text("id_pattern")? {
        p if p.is_empty() => None,
        p => Some(regex::Regex::new(&p).map_err(|e| format!("requirements.id_pattern 不是合法的正規式：{e}"))?),
    };
    let change = text("change")?;
    if !change.is_empty() {
        super::check_change_name(&change)?;
    }
    let section_map = rc.get("section_map").filter(|v| v.is_object()).cloned();
    Ok(SourceCfg { root, display, pattern, change, section_map })
}

/// 照設定讀規格目錄並配 id。
pub fn open_source(cfg: &SourceCfg) -> Result<super::Opened, String> {
    super::open(&cfg.root, &cfg.display, &super::IdOptions { pattern: cfg.pattern.clone(), adopt: Vec::new(), retire: Vec::new() })
}
