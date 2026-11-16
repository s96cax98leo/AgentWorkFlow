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
    pub fn task_id_regex(&self) -> Option<Regex> {
        let prefix = self.config_str("task_id_prefix");
        if prefix.is_empty() { return None; }
        Regex::new(&format!(r"(?<![\w-]){}\d+\b", regex::escape(&prefix))).ok()
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
        if name == "methodology" && data.is_object() { errs.extend(methodology_errors(data)); }
        if name == "deliverables" && data.is_object() { errs.extend(deliverables_errors(Some(data), "交付設定")); }
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
        let tid = self.task_id_regex();
        for (i, w) in items(get(env, "derived_work")).iter().enumerate() {
            if let (Some(re), Some(s)) = (&tid, w.as_str()) {
                if re.is_match(s).unwrap_or(false) {
                    errs.push(format!("derived_work[{i}]：不可自編任務編號（交 {} 編）：{s}", self.task_authority()));
                }
            }
        }
        let empty = Value::Object(Map::new());
        let payload = get(env, "payload").filter(|p| p.is_object()).unwrap_or(&empty);
        if let Some(Value::Array(subtasks)) = get(payload, "subtasks") {
            let subs: Vec<&Value> = subtasks.iter().filter(|s| s.is_object()).collect();
            let id_of = |s: &Value| str_opt(get(s, "id"));
            let ids: HashSet<String> = subs.iter().map(|s| repr_opt(get(s, "id"))).collect();
            let mut owner: HashMap<String, (String, String)> = HashMap::new(); // 檔案 → (id 的 repr, id 的顯示字串)
            for s in &subs {
                let me = (repr_opt(get(s, "id")), id_of(s));
                for f in items(get(s, "owns")) {
                    let key = repr(f);
                    if let Some(prev) = owner.get(&key) {
                        if prev.0 != me.0 {
                            errs.push(format!("子任務 {} 與 {} 同時擁有 {}（檔案所有權不得重疊）", prev.1, me.1, str_of(f)));
                        }
                    }
                    owner.insert(key, me.clone());
                }
                for d in items(get(s, "depends_on")) {
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
            for s in &subs {
                let mine = wave[&repr_opt(get(s, "id"))];
                for d in items(get(s, "depends_on")) {
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
    let rows: Vec<&Value> = items(p.get("traceability")).iter().filter(|r| r.is_object()).collect();
    let followups: Vec<&Value> = items(p.get("followups")).iter().filter(|f| f.is_object()).collect();
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
    let subs: Vec<&Value> = items(p.get("sub_plans")).iter().filter(|s| s.is_object()).collect();
    let split = truthy_opt(p.get("split"));
    if split && subs.len() < 2 { errs.push("split=true 時至少要 2 個子計劃".into()); }
    if !split && !subs.is_empty() { errs.push("split=false 時 sub_plans 要留空".into()); }
    let ids: Vec<String> = subs.iter().map(|s| str_opt(get(s, "id"))).collect();
    // 0.x 的 Python 版走訪的是 set（順序不固定）；這裡排序，輸出固定
    let dup: BTreeSet<&String> = ids.iter().filter(|x| ids.iter().filter(|y| y == x).count() > 1).collect();
    for i in dup { errs.push(format!("子計劃 id {i} 重複")); }
    let mut order: Vec<String> = Vec::new();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for s in &subs {
        let sid = str_opt(get(s, "id"));
        if !deps.contains_key(&sid) { order.push(sid.clone()); }
        deps.insert(sid, items(get(s, "depends_on")).iter().map(str_of).collect());
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
        for b in &subs[i + 1..] {
            for pa in items(get(a, "owns")) {
                for pb in items(get(b, "owns")) {
                    let (sa, sb) = (str_of(pa), str_of(pb));
                    if covers(&sa, &sb) || covers(&sb, &sa) {
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
pub fn methodology_errors(doc: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    let rx = get(doc, "conventions").and_then(|c| get(c, "commit")).and_then(|c| get(c, "check_regex")).and_then(Value::as_str);
    if let Some(rx) = rx.filter(|r| !r.is_empty()) {
        if let Err(e) = Regex::new(rx) {
            errs.push(format!("conventions.commit.check_regex 不是合法的正規式：{e}"));
        }
    }
    // 知識圖另外收的 repo（knowledge.repos）：id 不重複；
    // sources 是相對那個 repo 的 glob（不能是絕對路徑或含 ..），不能指名機敏檔；spec_root 要在專案裡
    if let Some(know) = get(doc, "knowledge").filter(|k| k.is_object()) {
        let repos: Vec<&Value> = items(get(know, "repos")).iter().filter(|r| r.is_object()).collect();
        for n in dup_strings(&repos.iter().map(|r| get(r, "id")).collect::<Vec<_>>()) {
            errs.push(format!("knowledge.repos 的 id「{n}」重複"));
        }
        let sensitive = regex::Regex::new(r"(?i)(?:^|/)(?:\.env(?:\..*)?|.*\.(?:pem|key|p12|pfx)|id_rsa|id_ed25519|credentials.*|secrets?\..*)\n?$").expect("固定的正規式");
        for r in &repos {
            let rid = str_opt(get(r, "id"));
            if get(r, "path").and_then(Value::as_str).is_some_and(|p| p.trim().is_empty() || p.contains('\0')) {
                errs.push(format!("knowledge.repos[{rid}].path 不能是空白"));
            }
            for g in items(get(r, "sources")).iter().filter_map(Value::as_str) {
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
    if let Some(events) = obj(get(doc, "events")) {
        for (name, ev) in events {
            if !ev.is_object() { continue; }
            if !name_re.is_match(name) {
                errs.push(format!("events 的事件名「{name}」要是小寫英數與連字號（32 字以內）"));
            }
            if EVENT_RESERVED.contains(&name.as_str()) {
                errs.push(format!("events 不能用「{name}」當事件名（已經是別的流程或 SDLC 的段）"));
            }
            let steps: Vec<&Value> = ["before", "after"].iter().flat_map(|k| items(get(ev, k)).iter()).filter(|s| s.is_object()).collect();
            let names: Vec<String> = steps.iter().map(|s| str_opt(get(s, "name"))).collect();
            let labels: Vec<String> = steps.iter().map(|s| str_opt(get(s, "label"))).collect();
            let uniq = |v: &[String]| v.iter().collect::<HashSet<_>>().len() == v.len();
            if !uniq(&names) || !uniq(&labels) {
                errs.push(format!("events[{name}] 的 before／after 步驟 name 或 label 重複"));
            }
            let gather = get(ev, "gather").filter(|g| g.is_object());
            let keys: Vec<String> = items(gather.and_then(|g| get(g, "angles"))).iter().filter(|a| a.is_object()).map(|a| str_opt(get(a, "key"))).collect();
            if !uniq(&keys) {
                errs.push(format!("events[{name}].gather.angles 的 key 重複"));
            }
            let glabel = gather.and_then(|g| get(g, "label")).filter(|l| truthy(l)).map(str_of).unwrap_or_else(|| "gather".into());
            let clash = labels.iter().any(|lb| lb == "write" || lb == "challenge" || lb.starts_with(&format!("{glabel}:")));
            if clash || glabel == "write" || glabel == "challenge" {
                errs.push(format!("events[{name}] 的 label 和 challenge、write 或角度的 label 撞名"));
            }
            let before: BTreeSet<String> = items(get(ev, "before")).iter().filter(|s| s.is_object()).map(|s| str_opt(get(s, "name"))).collect();
            let mut write_pool: BTreeSet<String> = names.iter().cloned().collect();
            for k in ["all", "kept", "doubted", "doubted_ids"] { write_pool.insert(k.into()); }
            for (place, pool) in [("gather", &before), ("write", &write_pool)] {
                let part = get(ev, place).filter(|p| p.is_object());
                for d in items(part.and_then(|p| get(p, "data"))) {
                    if !d.is_object() { continue; }
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
    let cons: Vec<&Value> = items(get(doc, "constraints")).iter().filter(|c| c.is_object()).collect();
    for n in dup_strings(&cons.iter().map(|c| get(c, "id")).collect::<Vec<_>>()) {
        errs.push(format!("constraints 的 id「{n}」重複"));
    }
    for c in &cons {
        let cid = str_opt(get(c, "id"));
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
    let ext_names: Vec<Value> = items(get(doc, "extensions")).iter().filter(|e| e.is_object()).map(|e| get(e, "name").cloned().unwrap_or(Value::Null)).collect();
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
    errs.extend(deliverables_errors(get(doc, "deliverables"), "deliverables"));
    // 去客戶化掃描（awf scrub）：路徑要在專案裡、規則要編得過（訊息只說第幾條，不印規則）、when 只能是做得到的時機
    if let Some(scrub) = get(doc, "scrub").filter(|s| !s.is_null()) {
        errs.extend(crate::scrub::config_errors(scrub));
    }
    errs
}

/// 交付設定的語意規則（profile 的 deliverables 與專案的 deliverables.json 共用）。
pub fn deliverables_errors(d: Option<&Value>, place: &str) -> Vec<String> {
    let mut errs = Vec::new();
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
    let docs: Vec<&Value> = items(get(d, "documents")).iter().filter(|x| x.is_object()).collect();
    for n in dup_strings(&docs.iter().map(|x| get(x, "id")).collect::<Vec<_>>()) {
        errs.push(format!("{place}.documents 的 id「{n}」重複"));
    }
    for x in &docs {
        let xid = str_opt(get(x, "id"));
        for g in items(get(x, "sources")) {
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
    for rx in items(get(d, "privacy_allow")) {
        let ok = rx.as_str().is_some_and(|s| Regex::new(s).is_ok());
        if !ok { errs.push(format!("{place}.privacy_allow 有寫壞的正規式")); }
    }
    errs
}

/// 從 agent 回報文字取出信封：優先最後一個 ```json 區塊，否則嘗試整段。
pub fn extract_json(text: &str) -> Option<Value> {
    let re = regex::Regex::new(r"(?s)```json\s*(\{.*?\})\s*```").expect("固定的正規式");
    let blocks: Vec<&str> = re.captures_iter(text).filter_map(|c| c.get(1).map(|m| m.as_str())).collect();
    for raw in blocks.iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(raw) { return Some(v); }
    }
    serde_json::from_str(text).ok()
}
