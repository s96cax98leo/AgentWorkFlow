//! 開發規範 profile 的繼承（`methodology.json` 的 `extends`）：所有讀 profile 的地方共用的一套解析。
//!
//! `extends` 是字串陣列（也接受單一字串），每一項是
//! - 內建 profile 的名字：模版目錄（[`crate::template::find`]）的 `scaffold/.claude/teams/methodologies/<名>.json` 存在就認；
//! - 或專案內的相對路徑：一定在 `.claude/teams/` 底下、副檔名 `.json`、不能是絕對路徑或含 `..`
//!   （team agent 寫不到 `.claude/teams/`，所以繼承來源不會被 agent 竄改）。
//!
//! 合併順序：`extends` 由左到右依序疊上，最後疊上檔案自己的內容；後面的覆寫前面的。被繼承的檔自己也可以有 `extends`
//! （深度上限 [`MAX_DEPTH`]、偵測循環）。合併規則：物件逐鍵深度合併；純量與陣列整個取代；值是 `null` ＝拿掉繼承來的這個鍵。
//! `extends` 本身與 `name` 不繼承（`name` 只取檔案自己的）。
//!
//! **沒有 `extends` 鍵時原樣回傳**（[`resolve_file`] 不碰檔案系統），所以沒用到繼承的專案每個指令的輸出逐字不變。
//! 解析失敗（來源不存在、路徑不合規則、循環、太深、JSON 讀不懂）一律回錯誤：呼叫端照它「profile 讀不懂」時的處理，
//! 不退回「當作沒有 extends」。

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 繼承鏈的深度上限（檔案自己是第 0 層；被繼承的檔最多到第 4 層）。
pub const MAX_DEPTH: usize = 4;
/// 一份 profile 的 `extends` 最多幾項。
pub const MAX_ITEMS: usize = 8;
/// 繼承來源的大小上限（hook 每次工具呼叫都會讀，不讓一個大檔拖慢）。
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// 相對路徑一定要在這個目錄底下（相對於專案根）。
pub const REL_PREFIX: &str = ".claude/teams/";
/// 鍵名。
pub const KEY: &str = "extends";

/// 這份 profile 有沒有寫 `extends`（有這個鍵就算，值不合法也算：交給解析報錯）。
pub fn has_extends(doc: &Value) -> bool {
    doc.as_object().is_some_and(|o| o.contains_key(KEY))
}

/// 檔案在 `<root>/.claude/teams/` 底下時回專案根 `<root>`。
pub fn root_of(path: &Path) -> Option<PathBuf> {
    let teams = path.parent()?;
    let claude = teams.parent()?;
    (teams.file_name()? == "teams" && claude.file_name()? == ".claude").then(|| claude.parent().map(Path::to_path_buf).unwrap_or_default())
}

/// 不在 `.claude/teams/` 底下的 profile（`--profile` 指定的）用哪個專案根解析相對路徑：
/// `CLAUDE_PROJECT_DIR` → 從目前目錄往上找（[`crate::project::locate`]，不印說明）→ 目前目錄。
fn fallback_root() -> PathBuf {
    let cwd = std::env::current_dir().ok().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|| ".".into());
    let env = std::env::var(crate::project::ENV).ok();
    crate::project::locate(None, env.as_deref(), &cwd).map(|f| PathBuf::from(f.root)).unwrap_or_else(|| PathBuf::from(cwd))
}

/// 已經讀好的 profile（`path` 是它的位置）：沒有 `extends` 原樣回傳；有的話回傳合併後、去掉 `extends` 的結果。
/// 專案根取自路徑（`<root>/.claude/teams/…`），不在那底下時見 [`fallback_root`]。
pub fn resolve_file(path: &Path, doc: Value) -> Result<Value, String> {
    if !has_extends(&doc) {
        return Ok(doc);
    }
    let root = root_of(path).unwrap_or_else(fallback_root);
    Resolver::new(&root, None).resolve(&label_of(&root, path), doc).map(|(v, _)| v)
}

/// 同 [`resolve_file`]，路徑用字串給。
pub fn resolve_str(path: &str, doc: Value) -> Result<Value, String> {
    resolve_file(Path::new(path), doc)
}

/// 讀檔＋解析＋合併：檔案不存在回 `Ok(None)`；讀不到、不是 JSON、繼承失敗回 `Err(訊息)`。不檢查是不是物件、不驗 schema。
pub fn load(path: &Path) -> Result<Option<Value>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    resolve_file(path, doc).map(Some)
}

/// 繼承的結果：合併後的 profile、依疊上順序的來源（最後一個是檔案自己）、每個鍵路徑最後由哪些來源決定。
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub value: Value,
    pub sources: Vec<String>,
    pub origins: BTreeMap<String, Vec<String>>,
}

/// 讀檔並解析，連同來源清單（doctor、錯誤訊息標來源用）。檔案不存在回 `Ok(None)`。
pub fn inspect(path: &Path) -> Result<Option<Resolved>, String> {
    inspect_with(path, None)
}

/// 同 [`inspect`]，內建 profile 從 `template`（模版根目錄）找；None 用 [`crate::template::find`]。
pub fn inspect_with(path: &Path, template: Option<&Path>) -> Result<Option<Resolved>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let root = root_of(path).unwrap_or_else(fallback_root);
    let label = label_of(&root, path);
    if !has_extends(&doc) {
        return Ok(Some(Resolved { value: doc, sources: vec![label], origins: BTreeMap::new() }));
    }
    let mut r = Resolver::new(&root, template.map(Path::to_path_buf));
    let (value, origins) = r.resolve(&label, doc)?;
    Ok(Some(Resolved { value, sources: r.order, origins }))
}

/// 在驗證錯誤後面標上來源（`（來源：…）`）：訊息裡出現的鍵路徑取最長的一個對到來源；對不到時列整條鏈。
/// 只在 profile 有 `extends` 時呼叫（沒有繼承時訊息逐字不變）。
pub fn blame(errors: &[String], r: &Resolved) -> Vec<String> {
    errors.iter().map(|e| {
        let hit = r.origins.iter().filter(|(p, _)| mentions(e, p)).max_by_key(|(p, _)| p.len());
        match hit {
            Some((_, src)) => format!("{e}（來源：{}）", src.join("、")),
            None => format!("{e}（合併自：{}）", r.sources.join(" → ")),
        }
    }).collect()
}

/// 已經算好的錯誤（`path` 的 profile 合併後驗出來的）標上來源；檔案沒有 `extends` 或重讀失敗時原樣回傳。只在出錯時呼叫（會重讀繼承鏈）。
pub fn blame_path(path: &Path, errors: Vec<String>) -> Vec<String> {
    match inspect(path) {
        Ok(Some(r)) if r.sources.len() > 1 => blame(&errors, &r),
        _ => errors,
    }
}

fn mentions(msg: &str, path: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut from = 0;
    while let Some(i) = msg[from..].find(path) {
        let s = from + i;
        let e = s + path.len();
        let before = msg[..s].chars().next_back();
        let after = msg[e..].chars().next();
        if !before.is_some_and(|c| ident(c) || c == '.' && !msg[..s].ends_with("$.")) && !after.is_some_and(ident) {
            return true;
        }
        from = s + path.len().max(1);
        if from >= msg.len() { break; }
        while !msg.is_char_boundary(from) { from += 1; }
    }
    false
}

fn label_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).ok().map(|p| p.to_string_lossy().replace('\\', "/")).filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

/// 內建 profile 名字的規則：英數開頭，只有英數、`-`、`_`，最長 64。
fn builtin_name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
}

/// 相對路徑的規則（見模組說明）；不合規則回原因。
pub fn rel_problem(p: &str) -> Option<&'static str> {
    if p.contains('\0') {
        return Some("含 NUL 字元");
    }
    let n = p.replace('\\', "/");
    let b = n.as_bytes();
    if n.starts_with('/') || (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()) {
        return Some("不能是絕對路徑");
    }
    if n.split('/').any(|c| c == "..") {
        return Some("不能含 ..");
    }
    if !n.starts_with(REL_PREFIX) || n.len() <= REL_PREFIX.len() || n.split('/').any(|c| c.is_empty() || c == ".") {
        return Some("要是 .claude/teams/ 底下的檔（相對於專案根，例如 .claude/teams/acme-base.json）");
    }
    if !n.ends_with(".json") {
        return Some("副檔名要是 .json");
    }
    None
}

struct Resolver {
    root: PathBuf,
    template: Option<Option<PathBuf>>,
    /// 疊上順序（去重前的實際順序；同一個來源被兩條路繼承時列兩次）。
    order: Vec<String>,
}

impl Resolver {
    fn new(root: &Path, template: Option<PathBuf>) -> Resolver {
        Resolver { root: root.to_path_buf(), template: template.map(Some), order: Vec::new() }
    }

    fn builtin_dir(&mut self) -> Option<PathBuf> {
        if self.template.is_none() {
            self.template = Some(crate::template::find());
        }
        self.template.clone().flatten().map(|t| crate::template::teams(&t).join("methodologies"))
    }

    fn resolve(&mut self, label: &str, doc: Value) -> Result<(Value, BTreeMap<String, Vec<String>>), String> {
        let mut stack = vec![label.to_string()];
        let (v, origins) = self.expand(label, doc, &mut stack)?;
        Ok((Value::Object(v), origins))
    }

    /// 一項 extends → （識別、顯示名、檔案）。
    fn locate(&mut self, from: &str, item: &str) -> Result<(String, PathBuf), String> {
        let t = item.trim();
        if t.contains('/') || t.contains('\\') || t.ends_with(".json") || t.contains(':') {
            if let Some(why) = rel_problem(t) {
                return Err(format!("{from} 的 extends「{t}」{why}"));
            }
            let rel = t.replace('\\', "/");
            return Ok((rel.clone(), self.root.join(&rel)));
        }
        if !builtin_name_ok(t) {
            return Err(format!("{from} 的 extends「{t}」不是內建 profile 的名字，也不是 .claude/teams/ 底下的 .json 路徑"));
        }
        let Some(dir) = self.builtin_dir() else {
            return Err(format!("{from} 的 extends「{t}」：找不到模版目錄，無法讀內建 profile（設 AWF_TEMPLATE_DIR 或重新安裝 plugin）"));
        };
        let file = dir.join(format!("{t}.json"));
        if !file.is_file() {
            return Err(format!("{from} 的 extends「{t}」：沒有這個內建 profile（模版的 methodologies/ 底下沒有 {t}.json）"));
        }
        Ok((format!("內建 {t}"), file))
    }

    /// 展開一份 profile：回（合併後的內容, 每個鍵路徑的來源）。
    fn expand(&mut self, label: &str, doc: Value, stack: &mut Vec<String>) -> Result<(Map<String, Value>, Origins), String> {
        let Value::Object(mut own) = doc else {
            return Err(format!("{label} 不是 JSON 物件"));
        };
        let items: Vec<String> = match own.shift_remove(KEY) {
            // 沒有 extends 的檔（例如內建 profile）照原樣用：裡面的 null 是它自己的值，不是「拿掉」
            None => {
                let mut origins = Origins::new();
                merge(&mut Map::new(), own.clone(), label, &mut origins, None, "");
                self.order.push(label.to_string());
                return Ok((own, origins));
            }
            Some(Value::String(s)) => vec![s],
            Some(Value::Array(a)) => {
                let mut out = Vec::new();
                for (i, x) in a.iter().enumerate() {
                    match x.as_str() {
                        Some(s) => out.push(s.to_string()),
                        None => return Err(format!("{label} 的 extends[{i}] 要是字串")),
                    }
                }
                out
            }
            Some(_) => return Err(format!("{label} 的 extends 要是字串或字串陣列")),
        };
        if items.len() > MAX_ITEMS {
            return Err(format!("{label} 的 extends 最多 {MAX_ITEMS} 項（收到 {}）", items.len()));
        }
        let mut acc = Map::new();
        let mut origins = Origins::new();
        for (n, item) in items.iter().enumerate() {
            let (id, file) = self.locate(label, item)?;
            if stack.contains(&id) {
                let mut cyc: Vec<String> = stack.clone();
                cyc.push(id.clone());
                return Err(format!("extends 循環：{}", cyc.join(" → ")));
            }
            if stack.len() > MAX_DEPTH {
                return Err(format!("extends 太深（最多 {MAX_DEPTH} 層）：{} → {id}", stack.join(" → ")));
            }
            if !file.is_file() {
                return Err(format!("{label} 的 extends「{}」：檔案不存在（{id}，相對於專案根）", item.trim()));
            }
            let meta = std::fs::metadata(&file).map_err(|e| format!("{id}：讀不到（{e}）"))?;
            if meta.len() > MAX_BYTES {
                return Err(format!("{id}：檔案太大（上限 {} MB）", MAX_BYTES / 1024 / 1024));
            }
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{id}：讀不到（{e}）"))?;
            let parent: Value = serde_json::from_str(&text).map_err(|e| format!("{id} 不是合法 JSON：{e}"))?;
            stack.push(id.clone());
            let (mut sub, mut sub_origins) = self.expand(&id, parent, stack)?;
            stack.pop();
            sub.shift_remove("name");
            sub_origins.remove("name");
            if n == 0 {
                // 第一層照它展開後的樣子當底（保留它自己的 null 值）
                acc = sub;
                origins = sub_origins;
            } else {
                merge(&mut acc, sub, &id, &mut origins, Some(&sub_origins), "");
            }
        }
        self.order.push(label.to_string());
        merge(&mut acc, own, label, &mut origins, None, "");
        Ok((acc, origins))
    }
}

type Origins = BTreeMap<String, Vec<String>>;

fn drop_prefix(origins: &mut Origins, p: &str) {
    origins.remove(p);
    let pre = format!("{p}.");
    origins.retain(|k, _| !k.starts_with(&pre));
}

/// `over` 疊到 `base` 上（見模組說明的合併規則），同時記下每個鍵路徑由哪些來源決定
/// （`over_origins`：`over` 自己是展開過的 profile 時它內部的來源；None＝整層都來自 `src`）。
fn merge(base: &mut Map<String, Value>, over: Map<String, Value>, src: &str, origins: &mut Origins, over_origins: Option<&Origins>, prefix: &str) {
    for (k, v) in over {
        let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            Value::Null => {
                base.shift_remove(&k);
                drop_prefix(origins, &p);
            }
            Value::Object(o) => {
                let fresh = !matches!(base.get(&k), Some(Value::Object(_)));
                if fresh {
                    drop_prefix(origins, &p);
                    base.insert(k.clone(), Value::Object(Map::new()));
                }
                let from = over_origins.and_then(|m| m.get(&p)).cloned().unwrap_or_else(|| vec![src.to_string()]);
                let e = origins.entry(p.clone()).or_default();
                for f in from {
                    if !e.contains(&f) {
                        e.push(f);
                    }
                }
                if let Some(Value::Object(b)) = base.get_mut(&k) {
                    merge(b, o, src, origins, over_origins, &p);
                }
            }
            other => {
                drop_prefix(origins, &p);
                let from = over_origins.and_then(|m| m.get(&p)).cloned().unwrap_or_else(|| vec![src.to_string()]);
                origins.insert(p, from);
                base.insert(k, other);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Proj(PathBuf);
    impl Proj {
        fn new(tag: &str) -> Proj {
            let d = std::env::temp_dir().join(format!("awf-profile-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join(".claude/teams")).unwrap();
            Proj(d)
        }
        fn put(&self, rel: &str, v: &Value) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v.to_string()).unwrap();
        }
        fn main(&self) -> PathBuf {
            self.0.join(".claude/teams/methodology.json")
        }
        fn resolve(&self, doc: Value) -> Result<Value, String> {
            let tpl = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin");
            let mut r = Resolver::new(&self.0, Some(tpl));
            r.resolve(".claude/teams/methodology.json", doc).map(|(v, _)| v)
        }
    }
    impl Drop for Proj {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn no_extends_is_identity() {
        let p = Proj::new("id");
        let doc = json!({"name": "x", "z": null, "a": {"b": null}});
        assert_eq!(resolve_file(&p.main(), doc.clone()).unwrap(), doc);
    }

    #[test]
    fn single_and_order() {
        let p = Proj::new("one");
        p.put(".claude/teams/a.json", &json!({"name": "A", "k": 1, "o": {"x": 1, "y": 1}, "arr": [1, 2]}));
        p.put(".claude/teams/b.json", &json!({"name": "B", "k": 2, "o": {"y": 2}, "arr": [3]}));
        let v = p.resolve(json!({"extends": [".claude/teams/a.json", ".claude/teams/b.json"], "o": {"z": 3}})).unwrap();
        assert_eq!(v, json!({"k": 2, "o": {"x": 1, "y": 2, "z": 3}, "arr": [3]}));
        assert!(v.get("name").is_none(), "name 不繼承");
        let v = p.resolve(json!({"extends": [".claude/teams/b.json", ".claude/teams/a.json"], "name": "me"})).unwrap();
        assert_eq!(v["k"], json!(1));
        assert_eq!(v["arr"], json!([1, 2]));
        assert_eq!(v["name"], json!("me"));
        // 單一字串
        let v = p.resolve(json!({"extends": ".claude/teams/a.json"})).unwrap();
        assert_eq!(v["k"], json!(1));
        assert!(v.get("extends").is_none());
    }

    #[test]
    fn multi_level_null_and_arrays() {
        let p = Proj::new("multi");
        p.put(".claude/teams/base.json", &json!({"a": {"b": 1, "c": 2}, "list": ["x", "y"], "gone": true}));
        p.put(".claude/teams/mid.json", &json!({"extends": ".claude/teams/base.json", "a": {"c": null, "d": 4}, "gone": null, "list": ["z"]}));
        let v = p.resolve(json!({"extends": [".claude/teams/mid.json"], "a": {"e": {"f": null}}})).unwrap();
        assert_eq!(v, json!({"a": {"b": 1, "d": 4, "e": {}}, "list": ["z"]}));
        // 沒有 extends 的底層照原樣（它自己的 null 是值）；疊上來的 null 才是「拿掉」
        p.put(".claude/teams/nulls.json", &json!({"x": null, "y": {"z": null, "w": 1}}));
        let v = p.resolve(json!({"extends": ".claude/teams/nulls.json"})).unwrap();
        assert_eq!(v, json!({"x": null, "y": {"z": null, "w": 1}}));
        let v = p.resolve(json!({"extends": ".claude/teams/nulls.json", "x": null, "y": {"z": 2}})).unwrap();
        assert_eq!(v, json!({"y": {"z": 2, "w": 1}}));
    }

    #[test]
    fn cycle_and_depth() {
        let p = Proj::new("cyc");
        p.put(".claude/teams/a.json", &json!({"extends": ".claude/teams/b.json"}));
        p.put(".claude/teams/b.json", &json!({"extends": ".claude/teams/a.json"}));
        let e = p.resolve(json!({"extends": ".claude/teams/a.json"})).unwrap_err();
        assert!(e.contains("循環"), "{e}");
        let e = p.resolve(json!({"extends": ".claude/teams/methodology.json"})).unwrap_err();
        assert!(e.contains("循環"), "{e}");
        for i in 1..=5 {
            let next = if i < 5 { json!({"extends": format!(".claude/teams/l{}.json", i + 1)}) } else { json!({"k": 1}) };
            p.put(&format!(".claude/teams/l{i}.json"), &next);
        }
        let e = p.resolve(json!({"extends": ".claude/teams/l1.json"})).unwrap_err();
        assert!(e.contains("太深"), "{e}");
        // 剛好四層可以
        let v = p.resolve(json!({"extends": ".claude/teams/l2.json"})).unwrap();
        assert_eq!(v["k"], json!(1));
    }

    #[test]
    fn bad_paths() {
        let p = Proj::new("bad");
        p.put("x.json", &json!({}));
        p.put(".claude/teams/t.txt", &json!({}));
        for bad in ["/etc/x.json", "C:/x.json", ".claude/teams/../../x.json", "x.json", "docs/a.json", ".claude/teams/t.txt", ".claude/teams/./a.json"] {
            let e = p.resolve(json!({"extends": [bad]})).unwrap_err();
            assert!(e.contains(bad), "{bad}: {e}");
        }
        let e = p.resolve(json!({"extends": [".claude/teams/none.json"]})).unwrap_err();
        assert!(e.contains("不存在"), "{e}");
        let e = p.resolve(json!({"extends": [1]})).unwrap_err();
        assert!(e.contains("extends[0]"), "{e}");
        let e = p.resolve(json!({"extends": {"a": 1}})).unwrap_err();
        assert!(e.contains("字串"), "{e}");
        p.put(".claude/teams/broken.json", &json!(1));
        std::fs::write(p.0.join(".claude/teams/broken.json"), "{壞").unwrap();
        let e = p.resolve(json!({"extends": [".claude/teams/broken.json"]})).unwrap_err();
        assert!(e.contains("不是合法 JSON"), "{e}");
        p.put(".claude/teams/arr.json", &json!([1]));
        let e = p.resolve(json!({"extends": [".claude/teams/arr.json"]})).unwrap_err();
        assert!(e.contains("不是 JSON 物件"), "{e}");
    }

    #[test]
    fn builtin_names() {
        let p = Proj::new("bi");
        let v = p.resolve(json!({"extends": ["default"], "name": "acme", "conventions": {"pr_size": 123}})).unwrap();
        let def: Value = serde_json::from_str(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/scaffold/.claude/teams/methodologies/default.json")).unwrap()).unwrap();
        assert_eq!(v["name"], json!("acme"));
        assert_eq!(v["conventions"]["pr_size"], json!(123));
        assert_eq!(v["stages"], def["stages"]);
        let e = p.resolve(json!({"extends": ["waterfall-none"]})).unwrap_err();
        assert!(e.contains("沒有這個內建 profile"), "{e}");
        let e = p.resolve(json!({"extends": ["bad name"]})).unwrap_err();
        assert!(e.contains("不是內建 profile 的名字"), "{e}");
        let v = p.resolve(json!({"extends": ["default", "lightweight"]})).unwrap();
        assert!(v.get("name").is_none());
    }

    #[test]
    fn blame_points_to_source() {
        let p = Proj::new("blame");
        p.put(".claude/teams/a.json", &json!({"conventions": {"pr_size": "x"}, "k": 1}));
        let mut r = Resolver::new(&p.0, None);
        let (value, origins) = r.resolve(".claude/teams/methodology.json", json!({"extends": ".claude/teams/a.json", "k": 2})).unwrap();
        let res = Resolved { value, sources: r.order.clone(), origins };
        let out = blame(&["$.conventions.pr_size：型別應為 integer".to_string(), "其他".to_string()], &res);
        assert!(out[0].ends_with("（來源：.claude/teams/a.json）"), "{}", out[0]);
        assert!(out[1].contains("合併自：.claude/teams/a.json → .claude/teams/methodology.json"), "{}", out[1]);
        assert!(mentions("k：錯", "k"));
        assert!(!mentions("kk：錯", "k"));
    }

    #[test]
    fn rel_rules() {
        assert!(rel_problem(".claude/teams/a.json").is_none());
        assert!(rel_problem(".claude/teams/sub/a.json").is_none());
        assert!(rel_problem(".claude\\teams\\a.json").is_none());
        assert!(rel_problem(".claude/teams/").is_some());
        assert_eq!(root_of(Path::new("/p/.claude/teams/methodology.json")), Some(PathBuf::from("/p")));
        assert_eq!(root_of(Path::new("/p/x/methodology.json")), None);
    }
}
