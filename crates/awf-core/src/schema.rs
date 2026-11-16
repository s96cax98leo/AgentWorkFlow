//! schema 工具：載入、組合（信封＋payload）、驗證，以及 schema 表達不了的語意規則。
//!
//! 原本是 0.x 的 Python 工具 `schemalib.py`。錯誤訊息的文字是對跑基準的一部分
//! （`tests/parity/schema.expected.json`），改訊息要重錄基準並說明。
//!
//! 只支援 schemas/ 用到的 JSON Schema 關鍵字（`SUPPORTED_KEYWORDS`）。不認得的關鍵字等於沒檢查，
//! 所以 `unsupported_keywords` 要回空清單才算合格。
//!
//! 和 0.x 的 Python 版的已知差異：使用者寫的正規式（schema 的 pattern、`check_regex`、`id_pattern`、`privacy_allow`）
//! 改由 Rust 的引擎（fancy-regex）解讀，寫壞時的錯誤說明文字不同（「不是合法的正規式：」之後那一段）。

use crate::pyfmt::{self, items, repr, str_of, str_opt, truthy, truthy_opt};
use fancy_regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const SUPPORTED_KEYWORDS: &[&str] = &[
    "type", "properties", "required", "enum", "items", "minItems", "maxItems", "minLength", "maxLength", "minimum", "maximum",
    "pattern", "additionalProperties",
    // 註記類，不影響驗證
    "$schema", "$id", "title", "description", "examples", "default", "format", "$comment",
];

/// 一個事件類型的角度數與前後步驟數上限的預設值（profile 的 `limits.event` 可調；和 event.js 的預設一致）。
pub const EVENT_MAX_ANGLES: i64 = 8;
pub const EVENT_MAX_STEPS: i64 = 3;

/// 不能拿來當自訂事件名的字：不是 event.js 的事件、SDLC 的段、工具自己的目錄名。
pub const EVENT_RESERVED: &[&str] = &[
    "sdlc", "brief", "pr-review", "post-merge", "merge", "debate", "run", "plan", "design", "build", "sync", "test", "deliver", "archive", "logs",
];

/// 一個專案的 schema 目錄與設定（`.claude/teams/`）。
pub struct Schemas {
    pub teams_dir: PathBuf,
    /// 測試專用：覆蓋 guard.config.json 的同名鍵（golden-set 用它給 task_id_prefix）。
    pub config_override: Option<Map<String, Value>>,
}

fn is_search(re: &str, text: &str) -> bool {
    Regex::new(re).ok().and_then(|r| r.is_match(text).ok()).unwrap_or(false)
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

fn obj<'a>(v: Option<&'a Value>) -> Option<&'a Map<String, Value>> {
    v.and_then(Value::as_object)
}

fn is_int(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_i64() || n.is_u64())
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

impl Schemas {
    pub fn new(teams_dir: impl Into<PathBuf>) -> Self {
        Self { teams_dir: teams_dir.into(), config_override: None }
    }

    fn schema_dir(&self) -> PathBuf {
        self.teams_dir.join("schemas")
    }

    /// 專案設定（沒有檔案或格式錯時回空的）；還沒替換的佔位符當成空值。
    /// `path` 沒給時讀 `<teams>/guard.config.json` 並套用測試用的覆蓋值。
    pub fn config(&self, path: Option<&Path>) -> Map<String, Value> {
        let default = self.teams_dir.join("guard.config.json");
        let text = std::fs::read_to_string(path.unwrap_or(&default)).unwrap_or_default();
        let mut data = match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o)) => o,
            _ => Map::new(),
        };
        if path.is_none() {
            if let Some(ov) = &self.config_override {
                for (k, v) in ov { data.insert(k.clone(), v.clone()); }
            }
        }
        let placeholder = regex::Regex::new(r"\{\{[A-Z_]+\}\}").expect("固定的正規式");
        data.into_iter().map(|(k, v)| {
            let blank = matches!(&v, Value::String(s) if placeholder.is_match(s));
            (k, if blank { Value::String(String::new()) } else { v })
        }).collect()
    }

    fn config_str(&self, key: &str) -> String {
        match self.config(None).get(key) {
            Some(Value::String(s)) => s.trim().to_string(),
            _ => String::new(),
        }
    }

    /// 上層系統發的任務編號長相：task_id_prefix 後接數字；沒設定前綴就不檢查。
    /// 用 [`crate::hooks::re::Re`]：`\w`、`\b` 對「字」的看法和 0.x 的 Python 版一致（結合符號不算字；
    /// 直接用 Rust 的引擎時，編號前後緊接結合符號就認不出來）。
    pub fn task_id_regex(&self) -> Option<crate::hooks::re::Re> {
        let prefix = self.config_str("task_id_prefix");
        if prefix.is_empty() { return None; }
        let re = crate::hooks::re::Re::dynamic(format!(r"(?<![\w-]){}\d+\b", regex::escape(&prefix)), "");
        re.compiles().then_some(re)
    }

    /// `text` 裡長得像上層系統任務編號的區間（位元組位移）；沒設定前綴回空的。
    ///
    /// 前綴是純 ASCII 時就是 [`Self::task_id_regex`]。前綴自己帶非 ASCII 字元時不能用那個樣式：
    /// `hooks::re` 比對前會把「Python 與 Rust 對是不是字看法不同」的字元（例如結合符號）換成替身，
    /// 樣式裡的前綴卻還是原字，永遠對不上——設了這種前綴，「不可自編任務編號」就等於沒檢查。
    /// 這裡改成照同一個樣式的意思逐處核對：前綴原樣出現、前一個字元不是字也不是連字號、後面接一個以上的數字、
    /// 數字後面不是字（「字」「數字」用 `hooks::re` 判斷，和樣式的 `\w`、`\d` 同一套）。
    pub fn task_id_spans(&self, text: &str) -> Vec<(usize, usize)> {
        use crate::hooks::re::Re;
        let prefix = self.config_str("task_id_prefix");
        if prefix.is_empty() {
            return Vec::new();
        }
        if prefix.is_ascii() {
            return self.task_id_regex().map(|re| re.find_all(text)).unwrap_or_default();
        }
        static WORD: Re = Re::new(r"\A\w", "");
        static DIGITS: Re = Re::new(r"\A\d+", "");
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = text[from..].find(prefix.as_str()).map(|i| i + from) {
            // 下一輪從這次開頭的下一個字元找起（沒對上的位置後面可能還有）
            let step = text[at..].chars().next().map_or(1, char::len_utf8);
            from = at + step;
            let before_ok = text[..at].chars().next_back().is_none_or(|c| c != '-' && !WORD.is_match(c.encode_utf8(&mut [0u8; 4])));
            let digits_at = at + prefix.len();
            let Some((_, n)) = DIGITS.find(&text[digits_at..]) else { continue };
            let end = digits_at + n;
            if before_ok && !WORD.is_match(&text[end..]) {
                out.push((at, end));
                from = end;
            }
        }
        out
    }

    pub fn task_authority(&self) -> String {
        let s = self.config_str("task_authority");
        if s.is_empty() { "上層系統".into() } else { s }
    }

    pub fn payload_names(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.schema_dir().join("payloads")).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".schema.json")).map(str::to_string))
            .collect();
        out.sort();
        out
    }

    pub fn load(&self, name: &str) -> Result<Value, String> {
        let mut path = self.schema_dir().join(format!("{name}.schema.json"));
        if !path.exists() {
            path = self.schema_dir().join("payloads").join(format!("{name}.schema.json"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("讀不到 schema {}：{e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("schema {} 不是合法的 JSON：{e}", path.display()))
    }

    /// 信封＋指定 payload 的完整 schema；payload 沒給時 payload 欄位不限形狀。
    pub fn compose(&self, payload: Option<&str>) -> Result<Value, String> {
        let mut env = self.load("envelope")?;
        if let Some(p) = payload.filter(|p| !p.is_empty()) {
            let sub = self.load(p)?;
            if let Some(props) = env.get_mut("properties").and_then(Value::as_object_mut) {
                props.insert("payload".into(), sub);
            }
        }
        Ok(strip_meta(&env, false))
    }

    /// 不是信封 payload 的獨立 schema（例如 methodology），去掉 $id、title、description。
    pub fn doc_schema(&self, name: &str) -> Result<Value, String> {
        Ok(strip_meta(&self.load(name)?, false))
    }

    /// 信封：schema＋語意規則。
    pub fn check(&self, env: &Value, payload: Option<&str>) -> Result<Vec<String>, String> {
        let mut errs = validate(env, &self.compose(payload)?, "$");
        if env.is_object() { errs.extend(self.semantic_errors(env)); }
        Ok(errs)
    }

    /// 獨立文件（methodology、deliverables）：schema＋語意規則。
    pub fn check_doc(&self, data: &Value, name: &str) -> Result<Vec<String>, String> {
        let mut errs = validate(data, &self.doc_schema(name)?, "$");
        // 型別錯誤 JSON Schema 已經報過（上一行），語意規則這裡不重複報
        if name == "methodology" && data.is_object() { errs.extend(methodology_errors_with(data, false)); }
        if name == "deliverables" && data.is_object() { errs.extend(deliverables_errors_with(Some(data), "交付設定", false)); }
        if name == "program" && data.is_object() { errs.extend(crate::program::semantic_errors(data)); }
        Ok(errs)
    }

    /// schema 表達不了的交接規則。
    pub fn semantic_errors(&self, env: &Value) -> Vec<String> {
        let mut errs = Vec::new();
        let blocking: Vec<&Value> = items(get(env, "open_questions")).iter()
            .filter(|q| q.is_object() && truthy_opt(get(q, "blocking"))).collect();
        if get(env, "status").and_then(Value::as_str) == Some("done") && !blocking.is_empty() {
            errs.push("有 blocking 的 open_questions 時 status 不能是 done".to_string());
        }
        for (i, q) in blocking.iter().enumerate() {
            if items(get(q, "options")).len() < 2 {
                errs.push(format!("open_questions[{i}]：blocking 問題要附 2–3 個選項與代價"));
            }
        }
        for (i, w) in items(get(env, "derived_work")).iter().enumerate() {
            if let Some(s) = w.as_str() {
                if !self.task_id_spans(s).is_empty() {
                    errs.push(format!("derived_work[{i}]：不可自編任務編號（交 {} 編）：{s}", self.task_authority()));
                }
            }
        }
        let empty = Value::Object(Map::new());
        let payload = get(env, "payload").filter(|p| p.is_object()).unwrap_or(&empty);
        if get(payload, "subtasks").is_some_and(|v| !v.is_null()) {
            let subs: Vec<&Value> = objects_of(get(payload, "subtasks"), "payload.subtasks", &mut errs);
            let id_of = |s: &Value| str_opt(get(s, "id"));
            // 先看型別：每個子任務的 id、owns、depends_on（型別不對的記錯誤，下面當成沒有那一項繼續檢查）
            let mut owns_of: Vec<Vec<&Value>> = Vec::new();
            let mut deps_of: Vec<Vec<&Value>> = Vec::new();
            for (i, s) in subs.iter().enumerate() {
                key_field(get(s, "id"), &format!("payload.subtasks[{i}].id"), &mut errs);
                let who = if get(s, "id").is_some_and(Value::is_string) { format!("子任務 {} 的", id_of(s)) } else { format!("payload.subtasks[{i}] 的") };
                owns_of.push(strings_of(get(s, "owns"), &format!("{who} owns"), &mut errs));
                deps_of.push(strings_of(get(s, "depends_on"), &format!("{who} depends_on"), &mut errs));
            }
            let ids: HashSet<String> = subs.iter().map(|s| repr_opt(get(s, "id"))).collect();
            let mut owner: HashMap<String, (String, String)> = HashMap::new(); // 檔案 → (id 的 repr, id 的顯示字串)
            for (n, s) in subs.iter().enumerate() {
                let me = (repr_opt(get(s, "id")), id_of(s));
                for f in owns_of[n].iter().copied() {
                    let key = repr(f);
                    if let Some(prev) = owner.get(&key) {
                        if prev.0 != me.0 {
                            errs.push(format!("子任務 {} 與 {} 同時擁有 {}（檔案所有權不得重疊）", prev.1, me.1, str_of(f)));
                        }
                    }
                    owner.insert(key, me.clone());
                }
                for d in deps_of[n].iter().copied() {
                    match d.as_str() {
                        Some(ds) if ds.starts_with('@') => {
                            // 子計劃規劃時依賴別的子計劃，彙整時由 sdlc-design 展開成該子計劃的子任務
                            if !regex::Regex::new(r"^[a-z0-9][a-z0-9-]*$").expect("固定的正規式").is_match(&ds[1..]) {
                                errs.push(format!("子任務 {} 的子計劃依賴格式不合法（要是 @<子計劃 id>）：{ds}", id_of(s)));
                            }
                        }
                        _ => {
                            if !ids.contains(&repr(d)) {
                                errs.push(format!("子任務 {} 依賴不存在的 {}", id_of(s), str_of(d)));
                            }
                        }
                    }
                }
            }
            // 波次：不能依賴比自己晚的波次（沒標的算第 1 波）
            let wave_of = |s: &Value| -> i64 {
                match get(s, "wave") {
                    Some(Value::Number(n)) => n.as_i64().filter(|w| *w > 0).unwrap_or(1),
                    _ => 1,
                }
            };
            let wave: HashMap<String, i64> = subs.iter().map(|s| (repr_opt(get(s, "id")), wave_of(s))).collect();
            for (n, s) in subs.iter().enumerate() {
                let mine = wave[&repr_opt(get(s, "id"))];
                for d in deps_of[n].iter().copied() {
                    if let Some(ds) = d.as_str() {
                        if let Some(w) = wave.get(&repr(d)) {
                            if *w > mine {
                                errs.push(format!("子任務 {}（第 {mine} 波）依賴第 {w} 波的 {ds}：不能依賴比自己晚的波次", id_of(s)));
                            }
                        }
                    }
                }
            }
        }
        errs.extend(merge_decision_errors(payload));
        errs.extend(plan_outline_errors(payload));
        errs
    }
}

fn repr_opt(v: Option<&Value>) -> String {
    v.map(repr).unwrap_or_else(|| "None".into())
}

// 語意規則走訪前的型別檢查。0.x 的 Python 版直接走訪值：字串逐字元、物件走鍵、不能當鍵的值丟例外（exit 1）。
// 這裡不學它「字串逐字元」，而是型別不對就記一條錯誤（指出欄位）並當成空的繼續：不能因為型別不對就什麼都沒檢查到。

/// 清單欄位：沒有或 null 當成沒寫（空的）；陣列回元素；其他型別記一條錯誤，回空的。
fn list_of<'a>(v: Option<&'a Value>, what: &str, errs: &mut Vec<String>) -> &'a [Value] {
    match v {
        None | Some(Value::Null) => &[],
        Some(Value::Array(a)) => a,
        Some(other) => {
            errs.push(format!("{what} 要是陣列（收到 {}）", type_name(other)));
            &[]
        }
    }
}

/// 物件的清單：同 [`list_of`]，另外每個元素都要是物件（不是的記一條錯誤、跳過）。
fn objects_of<'a>(v: Option<&'a Value>, what: &str, errs: &mut Vec<String>) -> Vec<&'a Value> {
    let mut out = Vec::new();
    for (i, x) in list_of(v, what, errs).iter().enumerate() {
        if x.is_object() { out.push(x) } else { errs.push(format!("{what}[{i}] 要是物件（收到 {}）", type_name(x))) }
    }
    out
}

/// 字串的清單：同 [`list_of`]，另外每個元素都要是字串（不是的記一條錯誤、跳過）。回傳的仍是原本的值（都是字串）。
fn strings_of<'a>(v: Option<&'a Value>, what: &str, errs: &mut Vec<String>) -> Vec<&'a Value> {
    let mut out = Vec::new();
    for (i, x) in list_of(v, what, errs).iter().enumerate() {
        if x.is_string() { out.push(x) } else { errs.push(format!("{what}[{i}] 要是字串（收到 {}）", type_name(x))) }
    }
    out
}

/// 拿來當鍵比對的欄位（id、requirement_id）：沒有或 null 照舊（由 schema 的 required 管）；有值就要是字串。
/// 數字的 `1` 與 `1.0` 在 0.x 是同一個鍵、陣列與物件在 0.x 丟例外，這裡一律記錯誤。
fn key_field(v: Option<&Value>, what: &str, errs: &mut Vec<String>) {
    if let Some(x) = v.filter(|x| !x.is_null() && !x.is_string()) {
        errs.push(format!("{what} 要是字串（收到 {}）", type_name(x)));
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "布林值",
        Value::Number(_) => "數字",
        Value::String(_) => "字串",
        Value::Array(_) => "陣列",
        Value::Object(_) => "物件",
    }
}

/// 移除 schema 節點上的 $id、title、description，讓內嵌進 workflow 的 schema 精簡。
/// `properties` 底下的鍵是欄位名（例如子任務的 title 欄位），不能刪。
pub fn strip_meta(node: &Value, in_properties: bool) -> Value {
    match node {
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, v) in o {
                if in_properties {
                    out.insert(k.clone(), strip_meta(v, false));
                } else if !matches!(k.as_str(), "$id" | "title" | "description") {
                    out.insert(k.clone(), strip_meta(v, k == "properties"));
                }
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| strip_meta(v, false)).collect()),
        other => other.clone(),
    }
}

/// schema 裡 validate 不認得的關鍵字（不認得＝沒檢查）。properties 底下的鍵是欄位名，不算關鍵字。
pub fn unsupported_keywords(schema: &Value, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Value::Object(o) = schema {
        for (k, v) in o {
            if !SUPPORTED_KEYWORDS.contains(&k.as_str()) {
                out.push(format!("{path}.{k}"));
            }
            if k == "properties" {
                if let Value::Object(props) = v {
                    for (name, sub) in props {
                        out.extend(unsupported_keywords(sub, &format!("{path}.properties.{name}")));
                    }
                }
            } else if (k == "items" || k == "additionalProperties") && v.is_object() {
                out.extend(unsupported_keywords(v, &format!("{path}.{k}")));
            }
        }
    }
    out
}

fn type_ok(data: &Value, kind: &str) -> bool {
    match kind {
        "object" => data.is_object(),
        "array" => data.is_array(),
        "string" => data.is_string(),
        "boolean" => data.is_boolean(),
        "number" => data.is_number(),
        "integer" => is_int(data),
        "null" => data.is_null(),
        _ => false,
    }
}

/// 回傳錯誤清單（空＝通過）。
pub fn validate(data: &Value, schema: &Value, path: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(sc) = schema.as_object() else { return errs };
    if let Some(t) = sc.get("type").filter(|t| truthy(t)) {
        let kinds: Vec<String> = match t {
            Value::Array(a) => a.iter().map(str_of).collect(),
            other => vec![str_of(other)],
        };
        if !kinds.iter().any(|k| type_ok(data, k)) {
            return vec![format!("{path}：型別應為 {}", kinds.join("／"))];
        }
    }
    if let Some(en) = sc.get("enum") {
        if !items(Some(en)).iter().any(|x| pyfmt::eq(x, data)) {
            errs.push(format!("{path}：值 {} 不在 {}", repr(data), repr(en)));
        }
    }
    if let Value::String(s) = data {
        let n = pyfmt::len(s) as f64;
        if let Some(min) = sc.get("minLength").and_then(num) {
            if n < min { errs.push(format!("{path}：不能是空字串")); }
        }
        if let Some(max) = sc.get("maxLength") {
            if num(max).is_some_and(|m| n > m) {
                errs.push(format!("{path}：長度 {} 超過上限 {}（細節寫進檔案，信封只放摘要與路徑）", pyfmt::len(s), str_of(max)));
            }
        }
        if let Some(Value::String(p)) = sc.get("pattern") {
            if !is_search(p, s) {
                errs.push(format!("{path}：{} 不符合格式 {p}", repr(data)));
            }
        }
    }
    if let Some(x) = num(data) {
        if let Some(min) = sc.get("minimum") {
            if num(min).is_some_and(|m| x < m) { errs.push(format!("{path}：小於 {}", str_of(min))); }
        }
        if let Some(max) = sc.get("maximum") {
            if num(max).is_some_and(|m| x > m) { errs.push(format!("{path}：大於 {}", str_of(max))); }
        }
    }
    if let Value::Object(d) = data {
        for key in items(sc.get("required")) {
            let k = str_of(key);
            if !d.contains_key(&k) { errs.push(format!("{path}：缺欄位 {k}")); }
        }
        let empty = Map::new();
        let props = obj(sc.get("properties")).unwrap_or(&empty);
        for (key, sub) in props {
            if let Some(v) = d.get(key) {
                errs.extend(validate(v, sub, &format!("{path}.{key}")));
            }
        }
        let extra = sc.get("additionalProperties");
        for (key, v) in d {
            if props.contains_key(key) { continue; }
            match extra {
                Some(Value::Bool(false)) => errs.push(format!("{path}：不允許的欄位 {key}")),
                Some(rule @ Value::Object(_)) => errs.extend(validate(v, rule, &format!("{path}.{key}"))),
                _ => {}
            }
        }
    }
    if let Value::Array(a) = data {
        if let Some(min) = sc.get("minItems") {
            if num(min).is_some_and(|m| (a.len() as f64) < m) { errs.push(format!("{path}：至少要 {} 項", str_of(min))); }
        }
        if let Some(max) = sc.get("maxItems") {
            if num(max).is_some_and(|m| (a.len() as f64) > m) {
                errs.push(format!("{path}：{} 項，超過上限 {}（其餘寫進檔案）", a.len(), str_of(max)));
            }
        }
        if let Some(sub) = sc.get("items") {
            for (i, item) in a.iter().enumerate() {
                errs.extend(validate(item, sub, &format!("{path}[{i}]")));
            }
        }
    }
    errs
}

/// merge-decision payload（dev-merge-steward 的對照表）的語意規則；不是這種 payload 就回空清單。
/// merge-gate 也呼叫這裡，hook、golden-set、gate 三處判斷一致。
pub fn merge_decision_errors(payload: &Value) -> Vec<String> {
    let Some(p) = payload.as_object() else { return vec![] };
    if !(p.contains_key("traceability") && p.contains_key("decision")) { return vec![]; }
    let mut errs = Vec::new();
    // 型別先看過：對照表與 followups 要是物件的陣列，requirement_id 要是字串（型別不對的記錯誤，不能當成沒有那幾列）
    let rows: Vec<&Value> = objects_of(p.get("traceability"), "traceability", &mut errs);
    let followups: Vec<&Value> = objects_of(p.get("followups"), "followups", &mut errs);
    for (i, r) in rows.iter().enumerate() {
        key_field(get(r, "requirement_id"), &format!("traceability[{i}].requirement_id"), &mut errs);
    }
    for (i, f) in followups.iter().enumerate() {
        key_field(get(f, "requirement_id"), &format!("followups[{i}].requirement_id"), &mut errs);
    }
    let out_of_scope = truthy_opt(p.get("out_of_scope"));
    let is_match = |r: &Value| get(r, "result").and_then(Value::as_str) == Some("match");
    let bad: Vec<Value> = rows.iter().filter(|r| !is_match(r)).map(|r| get(r, "requirement_id").cloned().unwrap_or(Value::Null)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    for r in &rows {
        let rid = get(r, "requirement_id");
        if !seen.insert(repr_opt(rid)) {
            errs.push(format!("traceability：需求 {} 重複出現", str_opt(rid)));
        }
        if is_match(r) && !truthy_opt(get(r, "evidence")) {
            errs.push(format!("traceability：需求 {} 判 match 卻沒有證據", str_opt(rid)));
        }
    }
    let decision = p.get("decision").and_then(Value::as_str);
    if decision == Some("merge") {
        if !bad.is_empty() {
            errs.push(format!("decision=merge 時不得有 mismatch／missing：{}", repr(&Value::Array(bad.clone()))));
        }
        if !followups.is_empty() { errs.push("decision=merge 時不得有 followups".into()); }
        if out_of_scope { errs.push("decision=merge 時不得有 out_of_scope（對不到需求的改動）".into()); }
    }
    let covered: HashSet<String> = followups.iter().map(|f| repr_opt(get(f, "requirement_id"))).collect();
    for rid in &bad {
        if !covered.contains(&repr(rid)) {
            errs.push(format!("需求 {} 不一致或缺證據，卻沒有對應的 followup（新需求）", str_of(rid)));
        }
    }
    if out_of_scope && followups.is_empty() { errs.push("有 out_of_scope 改動時要列 followups".into()); }
    if p.get("needs_sync") == Some(&Value::Bool(true)) && decision == Some("merge") {
        errs.push("needs_sync=true（PR 落後或有衝突）時 decision 不能是 merge".into());
    }
    if let Some(ms) = p.get("merge_state").and_then(Value::as_str) {
        if (ms == "merged" || ms == "queued") && decision != Some("merge") {
            errs.push(format!("merge_state={ms} 時 decision 必須是 merge"));
        }
    }
    errs
}

/// 路徑 a 是否涵蓋 b（相同，或 a 是 b 的上層目錄）。
fn covers(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_end_matches('/'), b.trim_end_matches('/'));
    a == b || b.starts_with(&format!("{a}/"))
}

/// plan-outline payload（要不要拆子計劃）的語意規則；不是這種 payload 就回空清單。
pub fn plan_outline_errors(payload: &Value) -> Vec<String> {
    let Some(p) = payload.as_object() else { return vec![] };
    if !(p.contains_key("split") && p.contains_key("sub_plans")) { return vec![]; }
    let mut errs = Vec::new();
    let subs: Vec<&Value> = objects_of(p.get("sub_plans"), "sub_plans", &mut errs);
    // 型別先看過：id 要是字串，depends_on、owns 要是字串的陣列（型別不對的記錯誤，下面當成沒有那一項繼續檢查）
    let mut dep_strs: Vec<Vec<String>> = Vec::new();
    let mut own_strs: Vec<Vec<String>> = Vec::new();
    for (i, s) in subs.iter().enumerate() {
        key_field(get(s, "id"), &format!("sub_plans[{i}].id"), &mut errs);
        let who = if get(s, "id").is_some_and(Value::is_string) { format!("子計劃 {} 的", str_opt(get(s, "id"))) } else { format!("sub_plans[{i}] 的") };
        dep_strs.push(strings_of(get(s, "depends_on"), &format!("{who} depends_on"), &mut errs).into_iter().map(str_of).collect());
        own_strs.push(strings_of(get(s, "owns"), &format!("{who} owns"), &mut errs).into_iter().map(str_of).collect());
    }
    let split = truthy_opt(p.get("split"));
    if split && subs.len() < 2 { errs.push("split=true 時至少要 2 個子計劃".into()); }
    if !split && !subs.is_empty() { errs.push("split=false 時 sub_plans 要留空".into()); }
    let ids: Vec<String> = subs.iter().map(|s| str_opt(get(s, "id"))).collect();
    // 0.x 的 Python 版走訪的是 set（順序不固定）；這裡排序，輸出固定
    let dup: BTreeSet<&String> = ids.iter().filter(|x| ids.iter().filter(|y| y == x).count() > 1).collect();
    for i in dup { errs.push(format!("子計劃 id {i} 重複")); }
    let mut order: Vec<String> = Vec::new();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for (n, s) in subs.iter().enumerate() {
        let sid = str_opt(get(s, "id"));
        if !deps.contains_key(&sid) { order.push(sid.clone()); }
        deps.insert(sid, dep_strs[n].clone());
    }
    for sid in &order {
        for d in &deps[sid] {
            if d == sid {
                errs.push(format!("子計劃 {sid} 依賴自己"));
            } else if !deps.contains_key(d) {
                errs.push(format!("子計劃 {sid} 依賴不存在的子計劃 {d}"));
            }
        }
    }
    // 相依不得有循環（拓撲排序排不完就是有循環）
    let mut left: HashMap<String, HashSet<String>> = deps.iter()
        .map(|(k, v)| (k.clone(), v.iter().filter(|d| deps.contains_key(*d) && *d != k).cloned().collect())).collect();
    while !left.is_empty() {
        let ready: Vec<String> = left.iter().filter(|(_, v)| v.is_empty()).map(|(k, _)| k.clone()).collect();
        if ready.is_empty() {
            let mut names: Vec<&String> = left.keys().collect();
            names.sort();
            errs.push(format!("子計劃相依有循環：{}", names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、")));
            break;
        }
        for k in &ready { left.remove(k); }
        for v in left.values_mut() { for k in &ready { v.remove(k); } }
    }
    for (i, a) in subs.iter().enumerate() {
        for (j, b) in subs.iter().enumerate().skip(i + 1) {
            for sa in &own_strs[i] {
                for sb in &own_strs[j] {
                    if covers(sa, sb) || covers(sb, sa) {
                        errs.push(format!("子計劃 {} 與 {} 的範圍重疊：{sa}／{sb}", str_opt(get(a, "id")), str_opt(get(b, "id"))));
                    }
                }
            }
        }
    }
    errs
}

fn outside(s: &str) -> bool {
    let n = s.replace('\\', "/");
    n.starts_with('/') || regex::Regex::new(r"^[A-Za-z]:").expect("固定的正規式").is_match(&n) || n.split('/').any(|p| p == "..")
}

fn dup_strings(vals: &[Option<&Value>]) -> BTreeSet<String> {
    let strs: Vec<&str> = vals.iter().filter_map(|v| v.and_then(Value::as_str)).collect();
    strs.iter().filter(|x| strs.iter().filter(|y| y == x).count() > 1).map(|s| s.to_string()).collect()
}

/// 開發規範 profile 的語意規則（schema 表達不了的部分）。
/// 清單與物件欄位的型別也在這裡檢查：閘門讀 profile（`save_run::load_profile`、merge gate）只跑這個函式，沒有 JSON Schema 先擋。
pub fn methodology_errors(doc: &Value) -> Vec<String> {
    methodology_errors_with(doc, true)
}

/// `typed`：要不要回報「型別不對」。[`Schemas::check_doc`] 已經跑過 JSON Schema（同一個欄位的型別錯誤它報過了），傳 false 不重複報；
/// 型別不對的欄位不管回不回報，都當成空的繼續檢查其他規則。
fn methodology_errors_with(doc: &Value, typed: bool) -> Vec<String> {
    let mut errs = Vec::new();
    // JSON Schema 也擋得到的型別錯誤先收在這裡，最後依 typed 決定要不要回報
    let mut terrs: Vec<String> = Vec::new();
    let rx = get(doc, "conventions").and_then(|c| get(c, "commit")).and_then(|c| get(c, "check_regex")).and_then(Value::as_str);
    if let Some(rx) = rx.filter(|r| !r.is_empty()) {
        if let Err(e) = Regex::new(rx) {
            errs.push(format!("conventions.commit.check_regex 不是合法的正規式：{e}"));
        }
    }
    // rebase 前備份 ref 的樣式（conventions.sync.backup_ref）：規則和 awf sync-branch 用的同一個函式
    if let Some(p) = get(doc, "conventions").and_then(|c| get(c, "sync")).and_then(|s| get(s, "backup_ref")).and_then(Value::as_str) {
        if !crate::sync_branch::backup_ref_ok(p) {
            errs.push(format!("conventions.sync.backup_ref 要在 refs/backup/ 底下，<branch> 與 <date> 各出現一次，其餘只能是英數與 . _ - /（收到 {p}）"));
        }
    }
    // 清單不能有重複的項目（schema 的檢查器沒有 uniqueItems）
    // 以下幾個清單欄位：型別不對記錯誤（閘門讀 profile 時只跑這裡，沒有 JSON Schema 先擋；當成沒寫就什麼都沒檢查到）
    let record_sections: Vec<Option<&Value>> =
        list_of(get(doc, "stages").and_then(|s| get(s, "deliver")).and_then(|d| get(d, "record_sections")), "stages.deliver.record_sections", &mut terrs).iter().map(Some).collect();
    for n in dup_strings(&record_sections) {
        errs.push(format!("stages.deliver.record_sections 的「{n}」重複"));
    }
    if let Some(k) = get(doc, "knowledge").filter(|k| !k.is_null() && !k.is_object()) {
        terrs.push(format!("knowledge 要是物件（收到 {}）", type_name(k)));
    }
    let code_tools: Vec<Option<&Value>> = list_of(get(doc, "knowledge").and_then(|k| get(k, "code_tools")), "knowledge.code_tools", &mut terrs).iter().map(Some).collect();
    for n in dup_strings(&code_tools) {
        errs.push(format!("knowledge.code_tools 的「{n}」重複"));
    }
    // 知識圖另外收的 repo（knowledge.repos）：id 不重複；
    // sources 是相對那個 repo 的 glob（不能是絕對路徑或含 ..），不能指名機敏檔；spec_root 要在專案裡
    if let Some(know) = get(doc, "knowledge").filter(|k| k.is_object()) {
        let repos: Vec<&Value> = objects_of(get(know, "repos"), "knowledge.repos", &mut terrs);
        for n in dup_strings(&repos.iter().map(|r| get(r, "id")).collect::<Vec<_>>()) {
            errs.push(format!("knowledge.repos 的 id「{n}」重複"));
        }
        let sensitive = regex::Regex::new(r"(?i)(?:^|/)(?:\.env(?:\..*)?|.*\.(?:pem|key|p12|pfx)|id_rsa|id_ed25519|credentials.*|secrets?\..*)\n?$").expect("固定的正規式");
        for r in &repos {
            let rid = str_opt(get(r, "id"));
            if get(r, "path").and_then(Value::as_str).is_some_and(|p| p.trim().is_empty() || p.contains('\0')) {
                errs.push(format!("knowledge.repos[{rid}].path 不能是空白"));
            }
            for g in strings_of(get(r, "sources"), &format!("knowledge.repos[{rid}].sources"), &mut terrs).into_iter().filter_map(Value::as_str) {
                if outside(g) {
                    errs.push(format!("knowledge.repos[{rid}].sources 要是相對那個 repo 的 glob，不能是絕對路徑或含 ..（收到 {g}）"));
                } else if sensitive.is_match(&g.replace('\\', "/")) {
                    errs.push(format!("knowledge.repos[{rid}].sources 不能指名機敏檔（{g}）"));
                }
            }
        }
        if let Some(sr) = get(know, "spec_root").and_then(Value::as_str) {
            if outside(sr) {
                errs.push(format!("knowledge.spec_root 要是專案內的相對路徑，不能是絕對路徑或含 ..（收到 {sr}）"));
            }
        }
    }
    // 自訂事件類型（event.js）：名字不能和別的流程或 SDLC 的段撞名；data 的 from 要指得到東西；回傳的鍵不能蓋掉固定欄位
    let name_re = regex::Regex::new(r"^[a-z][a-z0-9-]{0,31}\n?$").expect("固定的正規式");
    // 角度與前後步驟的上限：limits.event（沒寫或不合法用預設 8 與 3；和 event.js 的 MAX_ANGLES、MAX_STEPS 同一個算法）。
    // schema 的 maxItems 是可調範圍的上限（16、6），預設的上限由這裡擋
    let ev_limit = |k: &str, lo: i64, hi: i64, d: i64| -> usize {
        let v = get(doc, "limits").and_then(|l| get(l, "event")).and_then(|e| get(e, k));
        let n = v.and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)));
        n.filter(|n| (lo..=hi).contains(n)).unwrap_or(d) as usize
    };
    let (max_angles, max_steps) = (ev_limit("max_angles", 1, 16, EVENT_MAX_ANGLES), ev_limit("max_steps", 0, 6, EVENT_MAX_STEPS));
    if let Some(v) = get(doc, "events").filter(|v| !v.is_null() && !v.is_object()) {
        terrs.push(format!("events 要是物件（事件名 → 定義；收到 {}）", type_name(v)));
    }
    if let Some(events) = obj(get(doc, "events")) {
        for (name, ev) in events {
            if !ev.is_object() {
                terrs.push(format!("events[{name}] 要是物件（收到 {}）", type_name(ev)));
                continue;
            }
            // 型別先看過：角度、前後步驟、data 都要是物件的陣列（型別不對時上限、重複、from 都檢查不到）
            let angles = objects_of(get(ev, "gather").and_then(|g| get(g, "angles")), &format!("events[{name}].gather.angles"), &mut terrs);
            let before_steps = objects_of(get(ev, "before"), &format!("events[{name}].before"), &mut terrs);
            let after_steps = objects_of(get(ev, "after"), &format!("events[{name}].after"), &mut terrs);
            let n_angles = angles.len();
            if n_angles > max_angles {
                errs.push(format!("events[{name}].gather.angles 有 {n_angles} 個，超過上限 {max_angles}（limits.event.max_angles；沒寫用 {EVENT_MAX_ANGLES}，最多 16）"));
            }
            for (k, n_steps) in [("before", before_steps.len()), ("after", after_steps.len())] {
                if n_steps > max_steps {
                    errs.push(format!("events[{name}].{k} 有 {n_steps} 步，超過上限 {max_steps}（limits.event.max_steps；沒寫用 {EVENT_MAX_STEPS}，最多 6）"));
                }
            }
            if !name_re.is_match(name) {
                errs.push(format!("events 的事件名「{name}」要是小寫英數與連字號（32 字以內）"));
            }
            if EVENT_RESERVED.contains(&name.as_str()) {
                errs.push(format!("events 不能用「{name}」當事件名（已經是別的流程或 SDLC 的段）"));
            }
            let steps: Vec<&Value> = before_steps.iter().chain(after_steps.iter()).copied().collect();
            let names: Vec<String> = steps.iter().map(|s| str_opt(get(s, "name"))).collect();
            let labels: Vec<String> = steps.iter().map(|s| str_opt(get(s, "label"))).collect();
            let uniq = |v: &[String]| v.iter().collect::<HashSet<_>>().len() == v.len();
            if !uniq(&names) || !uniq(&labels) {
                errs.push(format!("events[{name}] 的 before／after 步驟 name 或 label 重複"));
            }
            let gather = get(ev, "gather").filter(|g| g.is_object());
            let keys: Vec<String> = angles.iter().map(|a| str_opt(get(a, "key"))).collect();
            if !uniq(&keys) {
                errs.push(format!("events[{name}].gather.angles 的 key 重複"));
            }
            let glabel = gather.and_then(|g| get(g, "label")).filter(|l| truthy(l)).map(str_of).unwrap_or_else(|| "gather".into());
            let clash = labels.iter().any(|lb| lb == "write" || lb == "challenge" || lb.starts_with(&format!("{glabel}:")));
            if clash || glabel == "write" || glabel == "challenge" {
                errs.push(format!("events[{name}] 的 label 和 challenge、write 或角度的 label 撞名"));
            }
            let before: BTreeSet<String> = before_steps.iter().map(|s| str_opt(get(s, "name"))).collect();
            let mut write_pool: BTreeSet<String> = names.iter().cloned().collect();
            for k in ["all", "kept", "doubted", "doubted_ids"] { write_pool.insert(k.into()); }
            for (place, pool) in [("gather", &before), ("write", &write_pool)] {
                let part = get(ev, place).filter(|p| p.is_object());
                for d in objects_of(part.and_then(|p| get(p, "data")), &format!("events[{name}].{place}.data"), &mut terrs) {
                    let from = get(d, "from");
                    let hit = matches!(from, Some(Value::String(s)) if pool.contains(s));
                    if !hit {
                        let avail = if pool.is_empty() { "沒有".to_string() } else { pool.iter().cloned().collect::<Vec<_>>().join("、") };
                        errs.push(format!("events[{name}].{place}.data 的 from「{}」指不到東西（可用：{avail}）", str_opt(from)));
                    }
                }
            }
            let ret = get(ev, "returns").filter(|r| r.is_object());
            let pick = |k: &str, dflt: &str| ret.and_then(|r| get(r, k)).filter(|v| truthy(v)).map(str_of).unwrap_or_else(|| dflt.into());
            let mut used = vec![pick("all", "items"), pick("doubted", "refuted")];
            used.extend(names.iter().cloned());
            let fixed: BTreeSet<&str> = ["trace", "event", "docs", "halted", "stage", "halt_reason", "missing", "note", "capped", "challenge"].into_iter().collect();
            if !uniq(&used) || used.iter().any(|u| fixed.contains(u.as_str())) {
                errs.push(format!("events[{name}] 回傳的鍵名（returns 與步驟的 name）重複，或用到固定欄位（{}）", fixed.iter().copied().collect::<Vec<_>>().join("、")));
            }
        }
    }
    // 限制文件、recipe、modifier 是閘門會讀的設定（save-run 讀 profile 時只跑這裡的語意規則，沒有再跑一次 schema）：
    // 型別不對不能當成沒寫，否則限制文件的檢查就默默不做了
    let cons: Vec<&Value> = objects_of(get(doc, "constraints"), "constraints", &mut errs);
    for n in dup_strings(&cons.iter().map(|c| get(c, "id")).collect::<Vec<_>>()) {
        errs.push(format!("constraints 的 id「{n}」重複"));
    }
    for k in ["recipes", "modifiers"] {
        if let Some(v) = get(doc, k).filter(|v| !v.is_null() && !v.is_object()) {
            errs.push(format!("{k} 要是物件（收到 {}）", type_name(v)));
        }
    }
    for (i, c) in cons.iter().enumerate() {
        let cid = str_opt(get(c, "id"));
        let who = if get(c, "id").is_some_and(Value::is_string) { format!("constraints[{cid}]") } else { format!("constraints[{i}]") };
        strings_of(get(c, "stages"), &format!("{who}.stages"), &mut errs);
        strings_of(get(c, "paths"), &format!("{who}.paths"), &mut errs);
        if let Some(f) = get(c, "file").and_then(Value::as_str).filter(|f| !f.is_empty()) {
            let norm = f.replace('\\', "/");
            if outside(f) {
                errs.push(format!("constraints[{cid}].file 要是相對路徑，不能是絕對路徑或含 ..（收到 {f}）"));
            }
            let base = norm.rsplit('/').next().unwrap_or("").to_lowercase();
            if base.starts_with(".env") || [".pem", ".key", ".p12", ".pfx"].iter().any(|x| base.ends_with(x)) {
                errs.push(format!("constraints[{cid}].file 不能是機敏檔（{f}）"));
            }
        }
        for g in items(get(c, "paths")) {
            if let Some(gs) = g.as_str() {
                let n = gs.replace('\\', "/");
                if n.starts_with('/') || n.split('/').any(|p| p == "..") {
                    errs.push(format!("constraints[{cid}].paths 的 glob 要相對目標 repo，不能是絕對路徑或含 ..（收到 {gs}）"));
                }
            }
        }
    }
    let ext_names: Vec<Value> = objects_of(get(doc, "extensions"), "extensions", &mut terrs).into_iter().map(|e| get(e, "name").cloned().unwrap_or(Value::Null)).collect();
    let mut dup: Vec<String> = ext_names.iter().filter(|x| ext_names.iter().filter(|y| pyfmt::eq(x, y)).count() > 1).map(str_of).collect();
    dup.sort();
    dup.dedup();
    for n in dup { errs.push(format!("extensions：name {n} 重複")); }
    // 模型分級：形狀不對直接報錯；等級對應的模型不能倒掛（strong 對到比 standard 弱的模型，下限就形同虛設）
    if let Some(models) = get(doc, "models").filter(|m| !m.is_null()) {
        match models.as_object() {
            None => errs.push("models 要是物件".into()),
            Some(m) => {
                for k in ["tiers", "defaults", "floors"] {
                    if m.get(k).is_some_and(|v| !v.is_object()) { errs.push(format!("models.{k} 要是物件")); }
                }
                let tiers = obj(m.get("tiers"));
                let order = |s: &str| ["haiku", "sonnet", "opus", "inherit"].iter().position(|x| *x == s);
                let got = |t: &str, builtin: &str| tiers.and_then(|ts| ts.get(t)).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(builtin).to_string();
                let val = |t: &str| match t { "light" => got(t, "haiku"), "standard" => got(t, "sonnet"), _ => got(t, "inherit") };
                for (lo, hi) in [("light", "standard"), ("standard", "strong"), ("light", "strong")] {
                    let (gl, gh) = (val(lo), val(hi));
                    if let (Some(a), Some(b)) = (order(&gl), order(&gh)) {
                        if b < a { errs.push(format!("models.tiers：{hi}（{gh}）不能比 {lo}（{gl}）弱")); }
                    }
                }
            }
        }
    }
    if let Some(recipes) = obj(get(doc, "recipes")) {
        for (rn, r) in recipes {
            if !r.is_object() { continue; }
            let stages: Vec<&str> = match get(r, "stages") { Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(), _ => vec![] };
            // intent 不能省（I1）：每個 recipe 都要有 plan 段
            if !stages.contains(&"plan") {
                errs.push(format!("recipes.{rn}：stages 沒有 plan（每個 recipe 都要產出 intent、由人在 plan 閘門核准）"));
            }
            // 會寫程式的 recipe 一定要經過 build、deliver（審查）、merge（gate）
            let kinds = get(r, "build_kinds").map(truthy).unwrap_or(true);
            let publishable = get(r, "publishable") != Some(&Value::Bool(false));
            if kinds && publishable {
                let lost: Vec<&str> = ["build", "deliver", "merge"].into_iter().filter(|x| !stages.contains(x)).collect();
                if !lost.is_empty() {
                    errs.push(format!("recipes.{rn}：會產生改動的 recipe 一定要有 {} 段", lost.join("、")));
                }
            }
            if stages.contains(&"test") && !stages.contains(&"build") {
                errs.push(format!("recipes.{rn}：有 test 段就要有 build 段"));
            }
        }
    }
    let req = get(doc, "requirements").filter(|r| r.is_object());
    let rget = |k: &str| req.and_then(|r| get(r, k));
    if rget("source").and_then(Value::as_str) == Some("command") {
        let cmd = rget("command").filter(|c| truthy(c)).map(str_of).unwrap_or_default();
        if cmd.trim().is_empty() { errs.push("requirements.source=command 時要設 command".into()); }
    }
    if let Some(pat) = rget("id_pattern").and_then(Value::as_str).filter(|p| !p.is_empty()) {
        match Regex::new(pat) {
            Ok(re) => if re.captures_len() < 2 { errs.push("requirements.id_pattern 要有一個群組（需求 ID）".into()); },
            Err(e) => errs.push(format!("requirements.id_pattern 不是合法的正規式：{e}")),
        }
    }
    if rget("source").and_then(Value::as_str) == Some("openspec") {
        // 規格根目錄可以在另一個 repo（絕對路徑），但不能用 .. 繞出去；變更名稱是 changes/ 底下的一個目錄名
        if let Some(p) = rget("path").and_then(Value::as_str).filter(|p| !p.is_empty()) {
            if let Err(e) = crate::spec::check_root_arg(p) { errs.push(e.replace("--root", "requirements.path")); }
        }
        if let Some(c) = rget("change").and_then(Value::as_str) {
            if let Err(e) = crate::spec::check_change_name(c.trim()) { errs.push(format!("requirements.change：{e}")); }
        }
    }
    if let Some(map) = rget("section_map") {
        match map.as_object() {
            None => errs.push("requirements.section_map 要是物件（必要節 → proposal 的節名清單）".into()),
            Some(o) => for (k, v) in o {
                let names: Option<Vec<&str>> = v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect());
                let ok = !k.trim().is_empty() && names.as_ref().is_some_and(|n| !n.is_empty() && n.len() == v.as_array().map_or(0, Vec::len) && n.iter().all(|x| !x.trim().is_empty()));
                if !ok { errs.push(format!("requirements.section_map.{k} 要是非空的節名清單")); }
                for n in names.iter().flatten().filter(|n| n.starts_with('@') && **n != crate::spec::SCENARIOS_TOKEN) {
                    errs.push(format!("requirements.section_map.{k}：不認得的記號 {n}（只有 {}）", crate::spec::SCENARIOS_TOKEN));
                }
            },
        }
    }
    if let Some(l) = get(doc, "links") {
        // 這裡的 enforce 會擋政策核准：寫錯型別不能默默當成沒開
        match l.as_object() {
            None => errs.push("links 要是物件（例如 {\"enforce\": false}）".into()),
            Some(o) => {
                if o.get("enforce").is_some_and(|e| !e.is_boolean()) { errs.push("links.enforce 要是 true 或 false".into()); }
                for k in o.keys().filter(|k| k.as_str() != "enforce") { errs.push(format!("links 不認得的鍵：{k}")); }
            }
        }
    }
    if let Some(v) = get(doc, "deliverables").filter(|v| !v.is_null() && !v.is_object()) {
        terrs.push(format!("deliverables 要是物件（收到 {}）", type_name(v)));
    }
    errs.extend(deliverables_errors_with(get(doc, "deliverables"), "deliverables", typed));
    if typed {
        errs.extend(terrs);
    }
    // 去客戶化掃描（awf scrub）：路徑要在專案裡、規則要編得過（訊息只說第幾條，不印規則）、when 只能是做得到的時機
    if let Some(scrub) = get(doc, "scrub").filter(|s| !s.is_null()) {
        errs.extend(crate::scrub::config_errors(scrub));
    }
    // 專案的 CI（awf ci）：id 與 name 不重複、run 不能是空的或含 GitHub 的運算式、檔名不能撞模版自己的
    if let Some(ci) = get(doc, "ci").filter(|s| !s.is_null()) {
        errs.extend(crate::ci::config_errors(ci));
    }
    errs
}

/// 交付設定的語意規則（profile 的 deliverables 與專案的 deliverables.json 共用）。
pub fn deliverables_errors(d: Option<&Value>, place: &str) -> Vec<String> {
    deliverables_errors_with(d, place, true)
}

/// `typed` 同 [`methodology_errors_with`]。
fn deliverables_errors_with(d: Option<&Value>, place: &str, typed: bool) -> Vec<String> {
    let mut errs = Vec::new();
    let mut terrs: Vec<String> = Vec::new();
    let Some(d) = d.filter(|d| d.is_object()) else { return errs };
    for key in ["dir", "repo", "banned_terms_file"] {
        if let Some(v) = get(d, key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            if outside(v) {
                errs.push(format!("{place}.{key} 要是專案內的相對路徑，不能是絕對路徑或含 ..（收到 {v}）"));
            }
        }
    }
    if let Some(bt) = get(d, "banned_terms_file").and_then(Value::as_str) {
        let sensitive = regex::Regex::new(r"(?i)(?:^|/)(?:\.env(?:\..*)?|.*\.(?:pem|key|p12|pfx)|id_rsa|id_ed25519|credentials.*|secrets?\..*)\n?$").expect("固定的正規式");
        if sensitive.is_match(&bt.replace('\\', "/")) {
            errs.push(format!("{place}.banned_terms_file 不能是機敏檔（{bt}）"));
        }
    }
    // 型別不對記錯誤：這幾個清單管的是路徑範圍與隱私放行，當成沒寫就檢查不到
    let docs: Vec<&Value> = objects_of(get(d, "documents"), &format!("{place}.documents"), &mut terrs);
    for n in dup_strings(&docs.iter().map(|x| get(x, "id")).collect::<Vec<_>>()) {
        errs.push(format!("{place}.documents 的 id「{n}」重複"));
    }
    for x in &docs {
        let xid = str_opt(get(x, "id"));
        for g in strings_of(get(x, "sources"), &format!("{place}.documents[{xid}].sources"), &mut terrs) {
            if let Some(gs) = g.as_str() {
                if outside(gs) {
                    errs.push(format!("{place}.documents[{xid}].sources 的 glob 要相對目標 repo，不能是絕對路徑或含 ..（收到 {gs}）"));
                }
            }
        }
        if let Some(so) = obj(get(x, "section_on")) {
            for (k, v) in so {
                let ok = matches!(v, Value::Array(a) if a.iter().all(|t| matches!(t, Value::String(s) if !s.is_empty())));
                if !ok { errs.push(format!("{place}.documents[{xid}].section_on[{k}] 要是字串陣列")); }
            }
        }
    }
    for rx in list_of(get(d, "privacy_allow"), &format!("{place}.privacy_allow"), &mut terrs) {
        let ok = rx.as_str().is_some_and(|s| Regex::new(s).is_ok());
        if !ok { errs.push(format!("{place}.privacy_allow 有寫壞的正規式")); }
    }
    if typed {
        errs.extend(terrs);
    }
    errs
}

/// 最後一個 ```json 區塊的內容（由短到長的幾種切法）；沒有 json 區塊回 None。
/// 區塊從最後一個「```json」開始（「```jsonl」「```json5」這類不算），不看內容長什麼樣子：寫壞的（少了右括號、沒收尾、
/// 不是物件）也算，才不會被當成沒交而退回前一個。收尾是後面的「```」；JSON 的字串裡可能也有「```」，
/// 所以每一個「```」（以及文字結尾）都是一種可能的收尾，由呼叫端依序試。
fn last_json_block(text: &str) -> Option<Vec<&str>> {
    const OPEN: &str = "```json";
    let mut end = text.len();
    let start = loop {
        let i = text[..end].rfind(OPEN)?;
        let after = i + OPEN.len();
        if text[after..].chars().next().is_none_or(|c| c.is_whitespace() || c == '{' || c == '[') {
            break after;
        }
        end = i;
    };
    let rest = &text[start..];
    let mut cuts: Vec<&str> = rest.match_indices("```").map(|(i, _)| rest[..i].trim()).collect();
    cuts.push(rest.trim());
    Some(cuts)
}

/// 文字裡有沒有 ```json 區塊（不管解析得了與否）。[`extract_json`] 回 None 時用來分辨「沒有交」與「交了但讀不懂」。
pub fn has_json_block(text: &str) -> bool {
    last_json_block(text).is_some()
}

/// 從 agent 回報文字取出信封：有 ```json 區塊時只看**最後一個**，它解析不了就是沒有信封（回 None）；
/// 不往前找（前面的區塊可能是被後面更正掉的舊版本，退回去等於採用了 agent 自己已經改掉的結論）。
/// 沒有 ```json 區塊時試整段。
pub fn extract_json(text: &str) -> Option<Value> {
    match last_json_block(text) {
        Some(cuts) => cuts.iter().find_map(|raw| serde_json::from_str::<Value>(raw).ok()),
        None => serde_json::from_str(text).ok(),
    }
}
