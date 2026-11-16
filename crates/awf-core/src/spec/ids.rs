//! 穩定 id：需求 `<能力>/<代稱>`、情境 `<能力>/<代稱>#<情境代稱>`。
//!
//! 代稱第一次由標題產生，之後記在 `<root>/ids.json`；改標題不改 id。
//! `ids.json` 只記標題，所以「像不像同一個需求」用標題與情境標題的重疊來判斷；
//! 像的一律列成待確認，不自動猜。已歸檔變更的 RENAMED 是明確的改名，直接沿用舊 id。

use super::read::{clean, norm_title, prob, rel_path, ChangeState, Problem, Requirement, Tree};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const IDS_FILE: &str = "ids.json";
pub const IDS_VERSION: u64 = 1;
const SIMILAR: f64 = 0.5;
const SLUG_MAX: usize = 80;

/// `ids.json` 裡的一個需求。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegEntry {
    pub title: String,
    /// 情境代稱 → 情境標題。
    pub scenarios: BTreeMap<String, String>,
}

/// `ids.json` 的內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub version: u64,
    /// `<能力>/<代稱>` → 需求。
    pub requirements: BTreeMap<String, RegEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry { version: IDS_VERSION, requirements: BTreeMap::new() }
    }
}

/// 產生 id 的選項。
#[derive(Debug, Clone, Default)]
pub struct IdOptions {
    /// 專案自己的編號樣式（`--id-pattern`）；有第 1 組就用第 1 組，否則用整個符合的字串。
    pub pattern: Option<Regex>,
    /// 人確認的改名：（舊 id、新標題）。
    pub adopt: Vec<(String, String)>,
    /// 人確認已經不存在的 id。
    pub retire: Vec<String>,
}

/// 待確認：登記的標題找不到了，而規格裡有沒登記、看起來像它的標題。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub id: String,
    pub title: String,
    /// 可能是它的新標題。
    pub candidates: Vec<String>,
    pub file: String,
    pub line: usize,
}

/// `assign` 的結果。
#[derive(Debug, Clone)]
pub struct IdReport {
    /// 照現在的規格應該寫進 ids.json 的內容（待確認的項目保持原樣）。
    pub registry: Registry,
    /// ids.json 本來有沒有。
    pub existed: bool,
    /// 和現有 ids.json 的差別，一項一行。
    pub changes: Vec<String>,
    pub pending: Vec<Pending>,
    pub problems: Vec<Problem>,
}

/// 由標題產生代稱：NFKC、小寫、字母數字與中日韓文字以外的換成 `-`、去頭尾 `-`。
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for c in s.nfkc().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            if n >= SLUG_MAX {
                break;
            }
            out.push(c);
            n += 1;
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn unique(base: &str, used: &BTreeSet<String>) -> String {
    if !used.contains(base) {
        return base.to_string();
    }
    let mut n = 2usize;
    loop {
        let c = format!("{base}-{n}");
        if !used.contains(&c) {
            return c;
        }
        n += 1;
    }
}

fn bigrams(s: &str) -> HashSet<(char, char)> {
    let v: Vec<char> = s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    v.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect()
}

/// 兩個標題像不像（字元 bigram 的 Dice 係數，0～1）。
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return if a.to_lowercase() == b.to_lowercase() { 1.0 } else { 0.0 };
    }
    2.0 * x.intersection(&y).count() as f64 / (x.len() + y.len()) as f64
}

fn req_similar(entry: &RegEntry, req: &Requirement) -> bool {
    if title_similarity(&entry.title, &req.title) >= SIMILAR {
        return true;
    }
    let old: HashSet<&str> = entry.scenarios.values().map(String::as_str).collect();
    let new: HashSet<&str> = req.scenarios.iter().map(|s| s.title.as_str()).collect();
    let union = old.union(&new).count();
    union > 0 && old.intersection(&new).count() as f64 / union as f64 >= SIMILAR
}

// ───────────────────────── ids.json ─────────────────────────

pub fn parse_registry(text: &str) -> Result<Registry, String> {
    let v: Value = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text)).map_err(|e| format!("不是合法的 JSON：{e}"))?;
    let top = v.as_object().ok_or("最外層要是物件")?;
    match top.get("version").and_then(Value::as_u64) {
        Some(IDS_VERSION) => {}
        _ => return Err(format!("version 要是 {IDS_VERSION}")),
    }
    let reqs = top.get("requirements").and_then(Value::as_object).ok_or("缺 requirements（物件）")?;
    let mut requirements = BTreeMap::new();
    for (id, e) in reqs {
        let ok = id.split_once('/').is_some_and(|(c, s)| !c.is_empty() && !s.is_empty() && !s.contains(['/', '#']));
        if !ok {
            return Err(format!("id「{}」不是 <能力>/<代稱> 的形式", clean(id)));
        }
        let title = e.get("title").and_then(Value::as_str).ok_or_else(|| format!("「{}」缺 title（字串）", clean(id)))?;
        let mut scenarios = BTreeMap::new();
        match e.get("scenarios") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, t) in m {
                    let t = t.as_str().ok_or_else(|| format!("「{}」的情境「{}」要是字串", clean(id), clean(k)))?;
                    if k.is_empty() || k.contains(['/', '#']) {
                        return Err(format!("「{}」的情境代稱「{}」不合法", clean(id), clean(k)));
                    }
                    scenarios.insert(k.clone(), norm_title(t));
                }
            }
            Some(_) => return Err(format!("「{}」的 scenarios 要是物件", clean(id))),
        }
        requirements.insert(id.clone(), RegEntry { title: norm_title(title), scenarios });
    }
    Ok(Registry { version: IDS_VERSION, requirements })
}

/// 讀 `<root>/ids.json`；沒有這個檔回 `None`。
pub fn load_registry(root: &Path) -> Result<Option<Registry>, String> {
    let p = root.join(IDS_FILE);
    if fs::symlink_metadata(&p).is_err() {
        return Ok(None);
    }
    let canon_root = fs::canonicalize(root).map_err(|e| format!("讀不到規格目錄：{e}"))?;
    let inside = fs::canonicalize(&p).map(|c| c.starts_with(&canon_root)).unwrap_or(false);
    if !inside {
        return Err(format!("{IDS_FILE} 是指到規格目錄外的符號連結，不讀"));
    }
    let text = super::read::read_text(&p).map_err(|e| format!("{IDS_FILE}：{e}"))?;
    parse_registry(&text).map(Some).map_err(|e| format!("{IDS_FILE}：{e}"))
}

/// `ids.json` 的文字：鍵排序、縮排 2、UTF-8 不跳脫、結尾換行。同樣的內容永遠產生同樣的位元組。
pub fn registry_text(reg: &Registry) -> String {
    let mut reqs = Map::new();
    for (id, e) in &reg.requirements {
        let mut scn = Map::new();
        for (k, t) in &e.scenarios {
            scn.insert(k.clone(), Value::String(t.clone()));
        }
        let mut m = Map::new();
        m.insert("scenarios".into(), Value::Object(scn));
        m.insert("title".into(), Value::String(e.title.clone()));
        reqs.insert(id.clone(), Value::Object(m));
    }
    let mut top = Map::new();
    top.insert("requirements".into(), Value::Object(reqs));
    top.insert("version".into(), Value::from(reg.version));
    let mut s = serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

/// 原子寫入 `<root>/ids.json`（先寫暫存檔再 rename）。
pub fn write_registry(root: &Path, reg: &Registry) -> Result<(), String> {
    let target = root.join(IDS_FILE);
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{IDS_FILE} 是符號連結，不寫"));
    }
    let tmp = rel_path(root, &format!("{IDS_FILE}.awf-new"));
    fs::write(&tmp, registry_text(reg).as_bytes()).map_err(|e| format!("寫不了 {IDS_FILE} 的暫存檔：{e}"))?;
    fs::rename(&tmp, &target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("換不了 {IDS_FILE}：{e}")
    })
}

// ───────────────────────── 配 id ─────────────────────────

fn split_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once('#') {
        Some((r, s)) => (r, Some(s)),
        None => (id, None),
    }
}

fn cap_of(id: &str) -> &str {
    id.split_once('/').map_or("", |(c, _)| c)
}

fn slug_of(id: &str) -> &str {
    id.split_once('/').map_or(id, |(_, s)| s)
}

/// 給一個需求的情境配 id。`entry` 是登記的內容（會更新成照現況該有的樣子）；`register` 為否時只配暫時的 id。
fn assign_scenarios(req: &mut Requirement, entry: &mut RegEntry, register: bool, pending: &mut Vec<Pending>) {
    let by_title: HashMap<String, String> = entry.scenarios.iter().map(|(k, t)| (t.clone(), k.clone())).collect();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let mut unmatched = Vec::new();
    for (i, s) in req.scenarios.iter_mut().enumerate() {
        match by_title.get(&s.title) {
            Some(k) if !matched.contains(k) => {
                s.id = format!("{}#{k}", req.id);
                matched.insert(k.clone());
            }
            _ => unmatched.push(i),
        }
    }
    let missing: Vec<String> = entry.scenarios.keys().filter(|k| !matched.contains(*k)).cloned().collect();
    let mut held: HashSet<usize> = HashSet::new();
    for k in missing {
        let old = entry.scenarios.get(&k).cloned().unwrap_or_default();
        let cands: Vec<usize> = unmatched.iter().copied().filter(|&i| req.scenarios.get(i).is_some_and(|s| title_similarity(&old, &s.title) >= SIMILAR)).collect();
        if cands.is_empty() {
            entry.scenarios.remove(&k);
            continue;
        }
        let titles = cands.iter().filter_map(|&i| req.scenarios.get(i).map(|s| s.title.clone())).collect();
        let line = cands.first().and_then(|&i| req.scenarios.get(i)).map_or(req.span.start, |s| s.span.start);
        pending.push(Pending { id: format!("{}#{k}", req.id), title: old, candidates: titles, file: req.file.clone(), line });
        held.extend(cands);
    }
    let mut used: BTreeSet<String> = entry.scenarios.keys().cloned().collect();
    for i in unmatched {
        let Some(s) = req.scenarios.get_mut(i) else { continue };
        let mut base = slugify(&s.title);
        if base.is_empty() {
            base = format!("scn-{}", i + 1);
        }
        let slug = unique(&base, &used);
        used.insert(slug.clone());
        s.id = format!("{}#{slug}", req.id);
        if register && !held.contains(&i) {
            entry.scenarios.insert(slug, s.title.clone());
        }
    }
}

fn pattern_slug(pattern: Option<&Regex>, title: &str) -> Option<String> {
    let caps = pattern?.captures(title)?;
    let m = caps.get(1).or_else(|| caps.get(0))?;
    let s = slugify(m.as_str());
    (!s.is_empty()).then_some(s)
}

fn describe(old: &Registry, new: &Registry) -> Vec<String> {
    let mut out = Vec::new();
    let keys: BTreeSet<&String> = old.requirements.keys().chain(new.requirements.keys()).collect();
    for id in keys {
        match (old.requirements.get(id), new.requirements.get(id)) {
            (None, Some(n)) => {
                out.push(format!("+ {id}　「{}」", clean(&n.title)));
                for (k, t) in &n.scenarios {
                    out.push(format!("+ {id}#{k}　「{}」", clean(t)));
                }
            }
            (Some(o), None) => out.push(format!("- {id}　「{}」", clean(&o.title))),
            (Some(o), Some(n)) => {
                if o.title != n.title {
                    out.push(format!("~ {id}　標題「{}」→「{}」", clean(&o.title), clean(&n.title)));
                }
                let ks: BTreeSet<&String> = o.scenarios.keys().chain(n.scenarios.keys()).collect();
                for k in ks {
                    match (o.scenarios.get(k), n.scenarios.get(k)) {
                        (None, Some(t)) => out.push(format!("+ {id}#{k}　「{}」", clean(t))),
                        (Some(t), None) => out.push(format!("- {id}#{k}　「{}」", clean(t))),
                        (Some(a), Some(b)) if a != b => out.push(format!("~ {id}#{k}　標題「{}」→「{}」", clean(a), clean(b))),
                        _ => {}
                    }
                }
            }
            (None, None) => {}
        }
    }
    out
}

/// 給整棵樹配 id（現行規格的需求與情境、進行中變更的差異），並算出 ids.json 該有的內容。
/// 不寫任何檔。`--adopt`／`--retire` 指到不存在的東西時回 `Err`。
pub fn assign(tree: &mut Tree, reg: Option<&Registry>, opts: &IdOptions) -> Result<IdReport, String> {
    let orig = reg.cloned().unwrap_or_default();
    let mut work = orig.clone();
    let mut problems = Vec::new();
    let mut pending = Vec::new();

    for id in &opts.retire {
        let (rid, scn) = split_id(id);
        let gone = match scn {
            Some(s) => work.requirements.get_mut(rid).and_then(|e| e.scenarios.remove(s)).is_some(),
            None => work.requirements.remove(rid).is_some(),
        };
        if !gone {
            return Err(format!("--retire：ids.json 沒有「{}」", clean(id)));
        }
    }
    for (id, title) in &opts.adopt {
        let title = norm_title(title);
        if title.is_empty() {
            return Err(format!("--adopt：「{}」的新標題是空的", clean(id)));
        }
        let (rid, scn) = split_id(id);
        let entry = work.requirements.get_mut(rid).ok_or_else(|| format!("--adopt：ids.json 沒有「{}」", clean(rid)))?;
        match scn {
            Some(s) => *entry.scenarios.get_mut(s).ok_or_else(|| format!("--adopt：ids.json 沒有「{}」", clean(id)))? = title,
            None => entry.title = title,
        }
    }

    // 已歸檔變更的 RENAMED：明確的改名，舊 id 跟著新標題走。
    for ch in tree.changes.iter().filter(|c| c.state == ChangeState::Archived) {
        for d in &ch.deltas {
            let present: HashSet<&str> = tree.capabilities.iter().filter(|c| c.name == d.capability).flat_map(|c| c.requirements.iter().map(|r| r.title.as_str())).collect();
            for r in &d.renamed {
                if present.contains(r.from.as_str()) {
                    continue;
                }
                let in_cap = |id: &str| cap_of(id) == d.capability;
                if work.requirements.iter().any(|(id, e)| in_cap(id) && e.title == r.to) {
                    continue;
                }
                if let Some((_, e)) = work.requirements.iter_mut().find(|(id, e)| cap_of(id) == d.capability && e.title == r.from) {
                    e.title = r.to.clone();
                }
            }
        }
    }

    for cap in &mut tree.capabilities {
        let prefix = format!("{}/", cap.name);
        let keys: Vec<String> = work.requirements.keys().filter(|k| cap_of(k) == cap.name).cloned().collect();
        let mut by_title: HashMap<String, String> = HashMap::new();
        for k in &keys {
            let Some(e) = work.requirements.get(k) else { continue };
            if let Some(other) = by_title.get(&e.title) {
                problems.push(prob("ids", "ids-dup", IDS_FILE, 1, format!("「{other}」與「{k}」登記了同一個標題「{}」", clean(&e.title))));
            } else {
                by_title.insert(e.title.clone(), k.clone());
            }
        }
        let mut matched: BTreeSet<String> = BTreeSet::new();
        let mut unmatched = Vec::new();
        for (i, r) in cap.requirements.iter_mut().enumerate() {
            match by_title.get(&r.title) {
                Some(k) if !matched.contains(k) => {
                    r.id = k.clone();
                    matched.insert(k.clone());
                    if let Some(e) = work.requirements.get_mut(k) {
                        assign_scenarios(r, e, true, &mut pending);
                    }
                }
                _ => unmatched.push(i),
            }
        }
        let mut held: HashSet<usize> = HashSet::new();
        for k in keys.iter().filter(|k| !matched.contains(*k)) {
            let Some(e) = work.requirements.get(k) else { continue };
            let cands: Vec<usize> = unmatched.iter().copied().filter(|&i| cap.requirements.get(i).is_some_and(|r| req_similar(e, r))).collect();
            if cands.is_empty() {
                work.requirements.remove(k);
                continue;
            }
            let titles = cands.iter().filter_map(|&i| cap.requirements.get(i).map(|r| r.title.clone())).collect();
            let line = cands.first().and_then(|&i| cap.requirements.get(i)).map_or(1, |r| r.span.start);
            pending.push(Pending { id: k.clone(), title: e.title.clone(), candidates: titles, file: cap.file.clone(), line });
            held.extend(cands);
        }
        let mut used: BTreeSet<String> = work.requirements.keys().filter(|k| cap_of(k) == cap.name).map(|k| slug_of(k).to_string()).collect();
        for i in unmatched {
            let Some(r) = cap.requirements.get_mut(i) else { continue };
            let from_pattern = pattern_slug(opts.pattern.as_ref(), &r.title);
            let mut base = from_pattern.clone().unwrap_or_else(|| slugify(&r.title));
            if base.is_empty() {
                base = format!("req-{}", i + 1);
            }
            let slug = unique(&base, &used);
            if from_pattern.is_some() && slug != base {
                problems.push(prob("ids", "ids-dup", &r.file, r.span.start, format!("編號「{base}」在能力 {} 裡重複", cap.name)));
            }
            used.insert(slug.clone());
            r.id = format!("{prefix}{slug}");
            let mut entry = RegEntry { title: r.title.clone(), scenarios: BTreeMap::new() };
            let register = !held.contains(&i);
            assign_scenarios(r, &mut entry, register, &mut pending);
            if register {
                work.requirements.insert(r.id.clone(), entry);
            }
        }
    }
    // 能力整個不見了的登記：沒有東西可以對，直接算移除。
    let caps: HashSet<&str> = tree.capabilities.iter().map(|c| c.name.as_str()).collect();
    work.requirements.retain(|k, _| caps.contains(cap_of(k)));

    // --adopt 指定的標題要真的對到
    for (id, title) in &opts.adopt {
        let title = norm_title(title);
        if !tree.find(id).is_some_and(|f| f.title == title) {
            return Err(format!("--adopt：規格裡找不到可以接「{}」的標題「{}」", clean(id), clean(&title)));
        }
    }

    assign_deltas(tree);
    pending.sort_by(|a, b| a.id.cmp(&b.id));
    let changes = describe(&orig, &work);
    Ok(IdReport { registry: work, existed: reg.is_some(), changes, pending, problems })
}

/// 進行中變更的差異：MODIFIED／REMOVED／RENAMED 對到現行需求的 id，ADDED 給預計的 id。
fn assign_deltas(tree: &mut Tree) {
    let Tree { capabilities, changes, .. } = tree;
    for ch in changes.iter_mut().filter(|c| c.state == ChangeState::Active) {
        for d in &mut ch.deltas {
            let cap = capabilities.iter().find(|c| c.name == d.capability);
            let current: HashMap<&str, &Requirement> = cap.map(|c| c.requirements.iter().map(|r| (r.title.as_str(), r)).collect()).unwrap_or_default();
            let mut used: BTreeSet<String> = cap.map(|c| c.requirements.iter().map(|r| slug_of(&r.id).to_string()).collect()).unwrap_or_default();
            for r in &mut d.renamed {
                r.id = current.get(r.from.as_str()).map(|x| x.id.clone()).unwrap_or_default();
            }
            let renamed: HashMap<&str, &str> = d.renamed.iter().map(|r| (r.to.as_str(), r.from.as_str())).collect();
            for r in d.modified.iter_mut().chain(d.removed.iter_mut()) {
                let target = current.get(r.title.as_str()).or_else(|| renamed.get(r.title.as_str()).and_then(|f| current.get(f)));
                let Some(t) = target else { continue };
                r.id = t.id.clone();
                let known: HashMap<&str, &str> = t.scenarios.iter().map(|s| (s.title.as_str(), s.id.as_str())).collect();
                let mut used_s: BTreeSet<String> = t.scenarios.iter().filter_map(|s| s.id.split_once('#').map(|(_, k)| k.to_string())).collect();
                for (i, s) in r.scenarios.iter_mut().enumerate() {
                    if let Some(id) = known.get(s.title.as_str()) {
                        s.id = (*id).to_string();
                        continue;
                    }
                    let mut base = slugify(&s.title);
                    if base.is_empty() {
                        base = format!("scn-{}", i + 1);
                    }
                    let slug = unique(&base, &used_s);
                    used_s.insert(slug.clone());
                    s.id = format!("{}#{slug}", r.id);
                }
            }
            for (i, r) in d.added.iter_mut().enumerate() {
                let mut base = slugify(&r.title);
                if base.is_empty() {
                    base = format!("req-{}", i + 1);
                }
                let slug = unique(&base, &used);
                used.insert(slug.clone());
                r.id = format!("{}/{slug}", d.capability);
                let mut used_s = BTreeSet::new();
                for (j, s) in r.scenarios.iter_mut().enumerate() {
                    let mut b = slugify(&s.title);
                    if b.is_empty() {
                        b = format!("scn-{}", j + 1);
                    }
                    let slug = unique(&b, &used_s);
                    used_s.insert(slug.clone());
                    s.id = format!("{}#{slug}", r.id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::read::{parse_delta_text, parse_spec_text, Change, TaskStats};
    use std::path::PathBuf;

    fn tree(spec: &str) -> Tree {
        let (c, problems) = parse_spec_text(spec, "orders", "specs/orders/spec.md");
        Tree { root: PathBuf::new(), display: "openspec".into(), capabilities: vec![c], changes: Vec::new(), problems, skipped: Vec::new() }
    }

    fn ids(t: &Tree) -> Vec<String> {
        t.capabilities.iter().flat_map(|c| c.requirements.iter().flat_map(|r| std::iter::once(r.id.clone()).chain(r.scenarios.iter().map(|s| s.id.clone())))).collect()
    }

    const SPEC: &str = "### Requirement: 建立訂單\n#### Scenario: 庫存足夠\n#### Scenario: 庫存不足\n### Requirement: Cancel an Order!\n#### Scenario: Before shipping\n";

    #[test]
    fn slugs() {
        assert_eq!(slugify("Cancel an Order!"), "cancel-an-order");
        assert_eq!(slugify("  建立　訂單（含贈品） "), "建立-訂單-含贈品");
        assert_eq!(slugify("ＡＢＣ　１２"), "abc-12");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(&"長".repeat(200)).chars().count(), SLUG_MAX);
    }

    #[test]
    fn first_time_ids_come_from_titles() {
        let mut t = tree(SPEC);
        let rep = assign(&mut t, None, &IdOptions::default()).unwrap();
        assert_eq!(ids(&t), ["orders/建立訂單", "orders/建立訂單#庫存足夠", "orders/建立訂單#庫存不足", "orders/cancel-an-order", "orders/cancel-an-order#before-shipping"]);
        assert!(rep.pending.is_empty() && !rep.existed);
        assert_eq!(rep.changes.len(), 5);
    }

    #[test]
    fn empty_and_colliding_titles() {
        let mut t = tree("### Requirement: A b\n#### Scenario: ?\n#### Scenario: !\n### Requirement: a-B\n#### Scenario: x\n### Requirement: ???\n#### Scenario: x\n");
        assign(&mut t, None, &IdOptions::default()).unwrap();
        assert_eq!(ids(&t), ["orders/a-b", "orders/a-b#scn-1", "orders/a-b#scn-2", "orders/a-b-2", "orders/a-b-2#x", "orders/req-3", "orders/req-3#x"]);
    }

    #[test]
    fn registry_text_is_stable_and_round_trips() {
        let mut t = tree(SPEC);
        let rep = assign(&mut t, None, &IdOptions::default()).unwrap();
        let text = registry_text(&rep.registry);
        assert!(text.ends_with("}\n") && text.contains("\"建立訂單\"") && text.starts_with("{\n  \"requirements\": {\n"));
        let back = parse_registry(&text).unwrap();
        assert_eq!(back, rep.registry);
        assert_eq!(registry_text(&back), text);
        let mut t2 = tree(SPEC);
        let rep2 = assign(&mut t2, Some(&back), &IdOptions::default()).unwrap();
        assert!(rep2.changes.is_empty() && rep2.pending.is_empty());
        assert_eq!(ids(&t), ids(&t2));
    }

    #[test]
    fn bad_registry_is_rejected() {
        for bad in ["", "[]", "{\"version\":2,\"requirements\":{}}", "{\"version\":1}", "{\"version\":1,\"requirements\":{\"x\":{\"title\":\"a\"}}}", "{\"version\":1,\"requirements\":{\"a/b\":{}}}", "{\"version\":1,\"requirements\":{\"a/b#c\":{\"title\":\"t\"}}}"] {
            assert!(parse_registry(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn retitle_is_pending_until_adopted() {
        let mut t = tree(SPEC);
        let reg = assign(&mut t, None, &IdOptions::default()).unwrap().registry;
        let edited = SPEC.replace("Requirement: 建立訂單", "Requirement: 建立新訂單");
        let mut t = tree(&edited);
        let rep = assign(&mut t, Some(&reg), &IdOptions::default()).unwrap();
        assert_eq!(rep.pending.len(), 1);
        assert_eq!(rep.pending[0].id, "orders/建立訂單");
        assert_eq!(rep.pending[0].candidates, ["建立新訂單"]);
        // 待確認的不動：舊的登記留著、新標題不登記
        assert_eq!(rep.registry, reg);
        assert_eq!(t.capabilities[0].requirements[0].id, "orders/建立新訂單");

        let mut t = tree(&edited);
        let opts = IdOptions { adopt: vec![("orders/建立訂單".into(), "建立新訂單".into())], ..IdOptions::default() };
        let rep = assign(&mut t, Some(&reg), &opts).unwrap();
        assert!(rep.pending.is_empty());
        assert_eq!(t.capabilities[0].requirements[0].id, "orders/建立訂單");
        assert_eq!(t.capabilities[0].requirements[0].scenarios[0].id, "orders/建立訂單#庫存足夠");
        assert_eq!(rep.registry.requirements["orders/建立訂單"].title, "建立新訂單");
        assert_eq!(rep.changes, ["~ orders/建立訂單　標題「建立訂單」→「建立新訂單」"]);

        let mut t = tree(&edited);
        let opts = IdOptions { retire: vec!["orders/建立訂單".into()], ..IdOptions::default() };
        let rep = assign(&mut t, Some(&reg), &opts).unwrap();
        assert!(rep.pending.is_empty());
        assert!(rep.registry.requirements.contains_key("orders/建立新訂單") && !rep.registry.requirements.contains_key("orders/建立訂單"));
    }

    #[test]
    fn unrelated_replacement_is_not_pending() {
        let mut t = tree(SPEC);
        let reg = assign(&mut t, None, &IdOptions::default()).unwrap().registry;
        let mut t = tree("### Requirement: 建立訂單\n#### Scenario: 庫存足夠\n#### Scenario: 庫存不足\n### Requirement: 發票\n#### Scenario: 開立\n");
        let rep = assign(&mut t, Some(&reg), &IdOptions::default()).unwrap();
        assert!(rep.pending.is_empty(), "{:?}", rep.pending);
        assert!(rep.changes.iter().any(|c| c.starts_with("- orders/cancel-an-order")));
        assert!(rep.changes.iter().any(|c| c.starts_with("+ orders/發票")));
    }

    #[test]
    fn adopt_and_retire_errors() {
        let mut t = tree(SPEC);
        let reg = assign(&mut t, None, &IdOptions::default()).unwrap().registry;
        for opts in [
            IdOptions { adopt: vec![("orders/沒有".into(), "x".into())], ..IdOptions::default() },
            IdOptions { adopt: vec![("orders/建立訂單".into(), "規格裡沒有的標題".into())], ..IdOptions::default() },
            IdOptions { retire: vec!["orders/沒有".into()], ..IdOptions::default() },
        ] {
            assert!(assign(&mut tree(SPEC), Some(&reg), &opts).is_err());
        }
    }

    #[test]
    fn id_pattern_uses_project_numbers() {
        let mut t = tree("### Requirement: REQ-12: 建立訂單\n#### Scenario: 一\n### Requirement: [us-3] 取消\n#### Scenario: 一\n### Requirement: 沒編號\n#### Scenario: 一\n");
        let opts = IdOptions { pattern: Regex::new(r"^\[?((?i:req|us)-\d+)").ok(), ..IdOptions::default() };
        let rep = assign(&mut t, None, &opts).unwrap();
        let got: Vec<&str> = t.capabilities[0].requirements.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(got, ["orders/req-12", "orders/us-3", "orders/沒編號"]);
        assert!(rep.problems.is_empty());
        let mut t = tree("### Requirement: REQ-1 甲\n#### Scenario: 一\n### Requirement: REQ-1 乙\n#### Scenario: 一\n");
        let rep = assign(&mut t, None, &opts).unwrap();
        assert!(rep.problems.iter().any(|p| p.code == "ids-dup"));
    }

    #[test]
    fn archived_rename_keeps_old_id() {
        let mut t = tree(SPEC);
        let reg = assign(&mut t, None, &IdOptions::default()).unwrap().registry;
        let mut t = tree(&SPEC.replace("Requirement: 建立訂單", "Requirement: 下單"));
        let (d, _) = parse_delta_text("## RENAMED Requirements\n- FROM: `### Requirement: 建立訂單`\n- TO: `### Requirement: 下單`\n", "rename-order", "orders", "f");
        t.changes.push(Change { name: "rename-order".into(), dir: "changes/archive/2026-01-02-rename-order".into(), state: ChangeState::Archived, archived_date: Some("2026-01-02".into()), deltas: vec![d], tasks: TaskStats::default(), has_proposal: true, has_tasks: true, has_design: false });
        let rep = assign(&mut t, Some(&reg), &IdOptions::default()).unwrap();
        assert!(rep.pending.is_empty());
        assert_eq!(t.capabilities[0].requirements[0].id, "orders/建立訂單");
        assert_eq!(rep.changes, ["~ orders/建立訂單　標題「建立訂單」→「下單」"]);
    }

    #[test]
    fn active_delta_gets_ids() {
        let mut t = tree(SPEC);
        let (d, _) = parse_delta_text("## ADDED Requirements\n### Requirement: 退款\n#### Scenario: 七天內\n## MODIFIED Requirements\n### Requirement: 下單\n#### Scenario: 庫存足夠\n#### Scenario: 新情境\n## RENAMED Requirements\n- FROM: `### Requirement: 建立訂單`\n- TO: `### Requirement: 下單`\n", "c", "orders", "f");
        t.changes.push(Change { name: "c".into(), dir: "changes/c".into(), state: ChangeState::Active, archived_date: None, deltas: vec![d], tasks: TaskStats::default(), has_proposal: true, has_tasks: true, has_design: false });
        assign(&mut t, None, &IdOptions::default()).unwrap();
        let d = &t.changes[0].deltas[0];
        assert_eq!(d.added[0].id, "orders/退款");
        assert_eq!(d.added[0].scenarios[0].id, "orders/退款#七天內");
        assert_eq!(d.renamed[0].id, "orders/建立訂單");
        assert_eq!(d.modified[0].id, "orders/建立訂單");
        assert_eq!(d.modified[0].scenarios[0].id, "orders/建立訂單#庫存足夠");
        assert_eq!(d.modified[0].scenarios[1].id, "orders/建立訂單#新情境");
    }

    #[test]
    fn similarity() {
        assert!(title_similarity("建立訂單", "建立新訂單") >= SIMILAR);
        assert!(title_similarity("建立訂單", "發票") < SIMILAR);
        assert_eq!(title_similarity("a", "A"), 1.0);
    }
}
