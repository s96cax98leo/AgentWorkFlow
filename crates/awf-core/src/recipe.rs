//! 依需求類型分流（recipe、modifier）與模型分級的解析。原本是 0.x 的 Python 工具 `recipe.py`。
//!
//! recipe 定義在開發規範 profile（methodology.json）的 recipes／modifiers；模版內建 11 個 recipe 與 5 個 modifier
//! （`builtin_recipes`、`builtin_modifiers`）。profile 沒有 recipes（舊版）或指定了不存在的 recipe 時，
//! 一律當成 feature（`fallback_feature`），只會更嚴，不會變鬆。
//!
//! 移植方式：資料一律是 `serde_json::Value`，Python 的 `None` 對應 `Value::Null`（所有參數都可以給 Null）。
//! 0.x 的 Python 版碰到形狀不對的輸入會丟例外（工具當掉、exit 1）；這裡回 `Err(PyErr)`，`kind` 是 Python 的例外類別名。
//! 呼叫端碰到 Err 一律要當成失敗（fail-closed），不能當成「沒有問題」。
//! 對跑基準在 `tests/parity/recipe.expected.json`（`tests/parity/record_recipe.py` 錄）。
//!
//! 和 0.x 的 Python 版的已知差異：
//! - `classify` 回排序過的清單（Python 回 set，走訪順序不固定）。
//! - 沒有 `__file__`：`load_profile`、`golden_cases`、`selftest` 的 teams 目錄由呼叫端給；`main` 用目前目錄的 `.claude/teams`。
//! - 讀 JSON 用 serde_json：不收 NaN／Infinity，超過 64 位元的整數會變成浮點數。
//! - Python 沒接住的例外（traceback）在 CLI 這裡是一行錯誤訊息，exit code 一樣是 1。

use crate::hooks::re::Re;
use crate::pyfmt::{self, str_of, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// SDLC 的段與順序（[`crate::stages`] 是唯一來源）。
pub const SDLC: &[&str] = &crate::stages::IDS;
pub const PLAN_MODES: &[&str] = &["feature", "diagnose", "impact", "invariants", "changelog", "baseline", "research", "revert"];
pub const ALL_ANGLES: &[&str] = &["requirement", "specs-adr", "code-contracts", "upstream"];
pub const ALL_AXES: &[&str] = &["standards", "spec", "bugs", "history"];
pub const CODE_KINDS: &[&str] = &["code", "test", "doc"];
/// intent 一定要有的節（I1）；profile 的 stages.plan.required_sections 可以覆寫（不能是空的）
pub const BASE_SECTIONS: &[&str] = &["目標", "範圍", "不做的事", "驗收條件"];
pub const TIERS: &[&str] = &["light", "standard", "strong"];
/// strong 預設 inherit：沿用使用者主對話選的模型，模版不替使用者指定最貴的模型
pub const BUILTIN_TIER_MODELS: &[(&str, &str)] = &[("light", "haiku"), ("standard", "sonnet"), ("strong", "inherit")];
/// 閘門角色的最低等級：profile 的 floors 只能再往上加，不能蓋掉這份（鍵是「角色」或「角色@段」）
pub const BUILTIN_MODEL_FLOORS: &[(&str, &str)] = &[
    ("dev-merge-steward", "strong"),
    ("dev-security-reviewer", "strong"),
    ("tpm-skeptic@merge", "strong"),
    ("dev-spec-author@plan", "strong"),
    ("tpm-skeptic@plan", "standard"), // plan 閘門前唯一的反駁，不能排到 light
];
pub const GATE_MODES: &[&str] = &["plan-only", "strict"];

const USAGE: &str = r#"recipe：依需求類型決定這次 SDLC 要走哪些段、派哪些角色（確定性解析，和 snippets/recipe.js 同一套規則）。

recipe 定義在開發規範 profile（methodology.json）的 recipes／modifiers；模版內建 11 個 recipe 與 5 個 modifier
（BUILTIN_RECIPES、BUILTIN_MODIFIERS，兩份內建 profile 照抄）。profile 沒有 recipes（舊版）或指定了不存在的 recipe 時，
一律當成 feature（FALLBACK_FEATURE＝改版前的完整流程），只會更嚴，不會變鬆。

規則（JS 版 snippets/recipe.js 必須一致；golden-set/recipes/*.json 兩邊都跑）：
  - 每個 recipe 都有 plan 段（intent 不能省）；stages 依 SDLC 順序排好。
  - size=small 時套 recipe 的 small 覆寫（只能改 research_angles 與 split），之後才疊 modifiers。
  - modifiers 只會加嚴：段、lens、審查軸、測試模式取聯集；design_review／security_review 取較嚴者（never < auto < always）。
  - 不認得的 modifier 記在 errors（呼叫端要停下交人），不默默略過。
  - stricter_or_equal(new, old)：escalate 只准加嚴（段、lens、審查軸、測試模式是超集，審查等級不降）。
  - diff_shortfalls(resolved, paths)：實際的 diff 形狀需要的流程 recipe 沒有時，列出理由（publish 拒推、merge_gate hold）。

用法：
    awf recipe resolve [--profile <methodology.json>] --recipe <名> [--modifiers a,b] [--size small]
    awf recipe check-diff --run <run_dir> [--base <ref>] [--repo <dir>]   # 依 run.json 的 recipe 檢查 diff 形狀
    awf recipe --selftest
只用標準庫。
"#;

// ------------------------------------------------------------------ Python 語意的小工具

/// 0.x 的 Python 版會丟的例外。`kind` 是例外類別名（TypeError、AttributeError、KeyError…）。
#[derive(Debug, Clone, PartialEq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

impl fmt::Display for PyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.msg)
    }
}

impl std::error::Error for PyErr {}

type R<T> = Result<T, PyErr>;

static NULL: Value = Value::Null;

fn empty_obj() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| Value::Object(Map::new()))
}

fn s(x: &str) -> Value {
    Value::String(x.to_string())
}

fn strs(xs: &[&str]) -> Value {
    Value::Array(xs.iter().map(|x| s(x)).collect())
}

fn tname(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => if n.is_f64() { "float" } else { "int" },
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn perr(kind: &'static str, msg: String) -> PyErr {
    PyErr { kind, msg }
}

fn no_attr(v: &Value, attr: &str) -> PyErr {
    perr("AttributeError", format!("'{}' object has no attribute '{}'", tname(v), attr))
}

/// `v.get(k)`：只有 dict 有 get。
fn dget<'a>(v: &'a Value, k: &str) -> R<&'a Value> {
    match v {
        Value::Object(o) => Ok(o.get(k).unwrap_or(&NULL)),
        _ => Err(no_attr(v, "get")),
    }
}

/// `v.get(k, 預設)`。
fn dget_d<'a>(v: &'a Value, k: &str, d: &'a Value) -> R<&'a Value> {
    match v {
        Value::Object(o) => Ok(o.get(k).unwrap_or(d)),
        _ => Err(no_attr(v, "get")),
    }
}

fn hashable(v: &Value) -> R<()> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(perr("TypeError", format!("unhashable type: '{}'", tname(v)))),
        _ => Ok(()),
    }
}

/// `table.get(key)`，key 是任意值：不是字串的鍵一定查不到，list／dict 當鍵是 TypeError。
fn dget_v<'a>(table: &'a Value, key: &Value) -> R<&'a Value> {
    let o = match table {
        Value::Object(o) => o,
        _ => return Err(no_attr(table, "get")),
    };
    hashable(key)?;
    Ok(match key {
        Value::String(k) => o.get(k).unwrap_or(&NULL),
        _ => &NULL,
    })
}

/// `x or {}`。
fn or_obj(v: &Value) -> &Value {
    if truthy(v) { v } else { empty_obj() }
}

/// `(profile or {}).get(k)`。
fn pget<'a>(profile: &'a Value, k: &str) -> R<&'a Value> {
    dget(or_obj(profile), k)
}

/// 走訪：list 是元素、str 是字元、dict 是鍵，其他 TypeError。
fn iter(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(t) => Ok(t.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| s(k)).collect()),
        _ => Err(perr("TypeError", format!("'{}' object is not iterable", tname(v)))),
    }
}

/// `list(v or [])`。
fn iter_or(v: &Value) -> R<Vec<Value>> {
    if truthy(v) { iter(v) } else { Ok(Vec::new()) }
}

fn has(list: &[Value], x: &Value) -> bool {
    list.iter().any(|y| pyfmt::eq(y, x))
}

fn has_str(list: &[Value], x: &str) -> bool {
    list.iter().any(|y| y.as_str() == Some(x))
}

/// `item in container`。
fn contains(container: &Value, item: &Value) -> R<bool> {
    match container {
        Value::Object(o) => {
            hashable(item)?;
            Ok(matches!(item, Value::String(k) if o.contains_key(k)))
        }
        Value::Array(a) => Ok(has(a, item)),
        Value::String(t) => match item {
            Value::String(i) => Ok(t.contains(i.as_str())),
            _ => Err(perr("TypeError", format!("'in <string>' requires string as left operand, not {}", tname(item)))),
        },
        _ => Err(perr("TypeError", format!("argument of type '{}' is not iterable", tname(container)))),
    }
}

fn as_index(key: &Value) -> Option<i64> {
    match key {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) if !n.is_f64() => n.as_i64(),
        _ => None,
    }
}

/// `container[key]`。
fn index(container: &Value, key: &Value) -> R<Value> {
    fn at<T>(len: usize, i: i64, what: &str, pick: impl Fn(usize) -> T) -> R<T> {
        let j = if i < 0 { i + len as i64 } else { i };
        if j < 0 || j as usize >= len { return Err(perr("IndexError", format!("{what} index out of range"))); }
        Ok(pick(j as usize))
    }
    match container {
        Value::Object(o) => {
            hashable(key)?;
            match key {
                Value::String(k) if o.contains_key(k) => Ok(o[k].clone()),
                _ => Err(perr("KeyError", pyfmt::repr(key))),
            }
        }
        Value::Array(a) => match as_index(key) {
            Some(i) => at(a.len(), i, "list", |j| a[j].clone()),
            None => Err(perr("TypeError", format!("list indices must be integers or slices, not {}", tname(key)))),
        },
        Value::String(t) => match as_index(key) {
            Some(i) => {
                let cs: Vec<char> = t.chars().collect();
                at(cs.len(), i, "string", |j| Value::String(cs[j].to_string()))
            }
            None => Err(perr("TypeError", format!("string indices must be integers, not '{}'", tname(key)))),
        },
        _ => Err(perr("TypeError", format!("'{}' object is not subscriptable", tname(container)))),
    }
}

/// `'、'.join(items)`：有不是字串的元素是 TypeError。
fn join_strs(items: &[Value]) -> R<String> {
    let mut out = Vec::with_capacity(items.len());
    for (i, x) in items.iter().enumerate() {
        match x {
            Value::String(t) => out.push(t.as_str()),
            _ => return Err(perr("TypeError", format!("sequence item {i}: expected str instance, {} found", tname(x)))),
        }
    }
    Ok(out.join("、"))
}

/// Python 的 `str.isspace()`（單一字元）。`re` 的 `\s` 與 `str.strip()` 都用它。
fn py_space(c: char) -> bool {
    matches!(c as u32, 0x09..=0x0d | 0x1c..=0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000)
}

fn py_strip(t: &str) -> &str {
    t.trim_matches(py_space)
}

/// Python 的 `str.splitlines()`。
fn py_splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if !matches!(c as u32, 0x0a..=0x0d | 0x1c..=0x1e | 0x85 | 0x2028 | 0x2029) { continue; }
        out.push(&text[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(j, '\n')) = it.peek() {
                it.next();
                end = j + 1;
            }
        }
        start = end;
    }
    if start < text.len() { out.push(&text[start..]); }
    out
}

// ------------------------------------------------------------------ 內建資料

/// 各 plan_mode 另外要有的節；profile 的 plan_modes.<mode>.sections 可以覆寫
pub fn mode_sections() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| json!({
        "feature": [],
        "diagnose": ["現象", "重現步驟", "預期行為", "根因"],
        "impact": ["既有驗收條件異動"],
        "invariants": ["不可改變的行為"],
        "changelog": ["上游變更與破壞性變更"],
        "baseline": ["量測基線"],
        "research": ["調研問題", "判準"],
        "revert": ["回退原因", "回退範圍", "恢復條件"],
    }))
}

/// 改版前的完整流程：沒有 recipes 的舊 profile、不認得的 recipe 都用它。
pub fn fallback_feature() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| json!({
        "description": "新功能：完整 7 段（改版前的預設流程）",
        "stages": SDLC,
        "plan_mode": "feature",
        "research_angles": ALL_ANGLES,
        "design_lenses": ["coverage", "plan", "system-design"],
        "test": {"design_review": "auto", "security_review": "auto", "modes": ["verify"]},
        "deliver_axes": ALL_AXES,
        "build_kinds": CODE_KINDS,
        "publishable": true,
        "followup": false,
        "split": true,
    }))
}

const NO_DESIGN: &[&str] = &["plan", "build", "sync", "test", "deliver", "merge"];
const SHORT: &[&str] = &["plan", "build", "test", "deliver", "merge"];

struct Spec<'a> {
    desc: &'a str,
    stages: &'a [&'a str],
    mode: &'a str,
    angles: &'a [&'a str],
    lenses: &'a [&'a str],
    test: Value,
    axes: &'a [&'a str],
    kinds: &'a [&'a str],
    publishable: bool,
    followup: bool,
    split: bool,
    gate: Option<&'a str>,
}

fn mk(x: Spec) -> Value {
    let mut d = json!({
        "description": x.desc, "stages": x.stages, "plan_mode": x.mode, "research_angles": x.angles, "design_lenses": x.lenses,
        "test": x.test, "deliver_axes": x.axes, "build_kinds": x.kinds, "publishable": x.publishable, "followup": x.followup, "split": x.split,
    });
    if let Some(g) = x.gate {
        d["gate_mode"] = s(g); // 閘門模式（見 gate_mode()）；沒寫＝strict
    }
    d
}

fn t(dr: &str, sr: &str, modes: &[&str]) -> Value {
    json!({"design_review": dr, "security_review": sr, "modes": modes})
}

/// 內建的 11 個 recipe（兩份內建 profile 照抄）。
pub fn builtin_recipes() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| {
        let base = |desc, stages, mode, angles, lenses, test, axes, kinds| Spec {
            desc, stages, mode, angles, lenses, test, axes, kinds, publishable: true, followup: false, split: false, gate: None,
        };
        let vr: &[&str] = &["verify", "regression"];
        let mut feature = fallback_feature().clone();
        feature["small"] = json!({"research_angles": ["requirement", "code-contracts"], "split": false});
        let mut m = Map::new();
        m.insert("feature".into(), feature);
        m.insert("bugfix".into(), mk(base("回報的 bug、錯誤行為：先診斷根因（重現→根因），修正並補回歸測試；沒有 design 段",
            NO_DESIGN, "diagnose", &["requirement", "code-contracts"], &[], t("auto", "auto", vr), &["spec", "bugs"], CODE_KINDS)));
        m.insert("change".into(), mk(Spec { split: true, ..base("需求變更（CR）：先盤點既有功能與驗收條件要改、留、刪哪些，design 只寫差異，test 加跑受影響舊驗收條件的回歸",
            SDLC, "impact", &["requirement", "specs-adr", "code-contracts"], &["coverage", "plan", "delta"], t("auto", "auto", vr), ALL_AXES, CODE_KINDS) }));
        m.insert("hotfix".into(), mk(Spec { followup: true, ..base("線上事故：最短路徑修正，事後一定要開後續 run 補完整流程",
            SHORT, "diagnose", &["code-contracts"], &[], t("auto", "auto", vr), &["bugs"], CODE_KINDS) }));
        m.insert("refactor".into(), mk(base("不改行為的重構：先列出不能改變的行為、先寫 characterization test，驗行為等價",
            NO_DESIGN, "invariants", &["code-contracts", "specs-adr"], &[], t("auto", "auto", &["verify", "equivalence"]), &["standards", "bugs", "history"], CODE_KINDS)));
        m.insert("deps".into(), mk(base("依賴升級：讀 changelog 找破壞性變更，強制安全審查",
            NO_DESIGN, "changelog", &["requirement", "code-contracts"], &[], t("auto", "always", vr), &["standards", "bugs"], CODE_KINDS)));
        m.insert("docs".into(), mk(Spec { gate: Some("plan-only"), ..base("只改文件、設定說明、文案：build 只能有 doc 子任務",
            &["plan", "build", "deliver", "merge"], "research", &["requirement"], &[], t("never", "never", &[]), &["standards", "spec"], &["doc"]) }));
        m.insert("test-only".into(), mk(Spec { gate: Some("plan-only"), ..base("補測試、修 flaky：不能改產品程式",
            SHORT, "research", &["code-contracts"], &[], t("never", "auto", &["verify"]), &["standards", "bugs"], &["test"]) }));
        m.insert("perf".into(), mk(Spec { split: true, ..base("效能改善：先量基線，改完前後對照",
            SDLC, "baseline", &["requirement", "code-contracts"], &["coverage", "plan", "system-design"], t("auto", "auto", &["verify", "benchmark"]), ALL_AXES, CODE_KINDS) }));
        m.insert("spike".into(), mk(Spec { publishable: false, ..base("調研、可行性評估：只有 plan 段，產出 intent 與調研結論，不能推送",
            &["plan"], "research", &["requirement", "specs-adr", "code-contracts"], &[], t("never", "never", &[]), &[], &[]) }));
        m.insert("revert".into(), mk(base("合併後回退：build 只跑 git revert，事後補原因與恢復條件",
            SHORT, "revert", &["code-contracts"], &[], t("auto", "auto", vr), &["bugs"], CODE_KINDS)));
        Value::Object(m)
    })
}

/// 內建的 5 個 modifier。
pub fn builtin_modifiers() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| json!({
        "ui": {"description": "有畫面改動：一定做設計審查", "test": {"design_review": "always"}},
        "security": {"description": "有安全面（auth、權限、機敏資料、外部輸入）：一定做安全審查",
                     "test": {"security_review": "always"}, "deliver_axes": ["bugs"], "model_floors": {"tpm-skeptic": "strong"}},
        "migration": {"description": "資料庫 migration：強制 design 段與回滾計畫、一定做安全審查",
                      "add_stages": ["design"], "design_lenses": ["coverage", "plan", "rollback"],
                      "test": {"security_review": "always", "modes": ["regression"]}},
        "public_api": {"description": "公開 API 或對外契約：強制 design 段檢查相容性，deliver 加歷史軸",
                       "add_stages": ["design"], "design_lenses": ["coverage", "plan", "compat"], "deliver_axes": ["spec", "history"]},
        "large": {"description": "範圍大：允許拆子計劃、調研四個角度全開",
                  "research_angles": ALL_ANGLES, "split": true, "model_floors": {"dev-spec-author": "strong"}},
    }))
}

fn pairs_obj(pairs: &[(&str, &str)]) -> Value {
    Value::Object(pairs.iter().map(|(k, v)| (k.to_string(), s(v))).collect())
}

fn builtin_floors() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| pairs_obj(BUILTIN_MODEL_FLOORS))
}

fn builtin_tier_models() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| pairs_obj(BUILTIN_TIER_MODELS))
}

/// `{"recipes": 內建, "modifiers": 內建}`：測試與 golden 案例說的「builtin」profile。
pub fn builtin_profile() -> Value {
    json!({"recipes": builtin_recipes(), "modifiers": builtin_modifiers()})
}

// ------------------------------------------------------------------ 模型分級（I7：AI 建議，工具守下限，只會往上）
// 和 snippets/recipe.js 的 resolveModel 完全一致（golden-set/recipes/*.json 的 model_cases 兩邊對跑）。

fn rank_str(x: &str) -> i32 {
    TIERS.iter().position(|y| *y == x).map(|i| i as i32).unwrap_or(-1)
}

fn tier_rank(v: &Value) -> i32 {
    match v {
        Value::String(x) => rank_str(x),
        _ => -1,
    }
}

fn tier_max(tiers: &[&Value]) -> String {
    let mut best = "";
    for v in tiers {
        if tier_rank(v) > rank_str(best) { best = v.as_str().unwrap_or(""); }
    }
    best.to_string()
}

fn tier_lookup(table: &Value, role: &Value, stage: &Value) -> R<String> {
    let table = if table.is_object() { table } else { empty_obj() };
    let a = dget_v(table, role)?;
    let b = dget(table, &format!("{}@{}", str_of(role), str_of(stage)))?;
    Ok(tier_max(&[a, b]))
}

/// 高一級的等級；已經最高（或不認得）回空字串。
pub fn model_up(tier: &Value) -> String {
    let i = tier_rank(tier);
    if 0 <= i && (i as usize) < TIERS.len() - 1 { TIERS[i as usize + 1].to_string() } else { String::new() }
}

/// 等級對應的模型別名；profile 寫 inherit 表示沿用主對話模型（回空字串）。
pub fn tier_model(profile: &Value, tier: &Value) -> R<String> {
    let models = pget(profile, "models")?;
    let tiers = match models {
        Value::Object(o) if o.get("tiers").is_some_and(Value::is_object) => &o["tiers"],
        _ => empty_obj(),
    };
    let own = dget_v(tiers, tier)?;
    let m = match own {
        Value::String(x) if !x.is_empty() => x.clone(),
        _ => dget_v(builtin_tier_models(), tier)?.as_str().unwrap_or("").to_string(),
    };
    Ok(if m == "inherit" { String::new() } else { m })
}

/// 回 `{mode, tier, model, floor, reason}`。
/// profile 沒有 models 或 mode=inherit：不指定模型（tier、model 都是空字串），全部沿用主對話模型。
/// mode=fixed：用 defaults，不採 AI 建議。mode=auto：AI 建議（suggested）優先，沒有或不認得就用 defaults。
/// 最後一律取 max(上面的結果, 內建 floor, profile floor, modifiers 的 model_floors)：只會往上。
pub fn resolve_model(profile: &Value, role: &Value, stage: &Value, suggested: &Value, modifiers: &Value) -> R<Value> {
    let models = pget(profile, "models")?;
    let auto = s("auto");
    let mode_of = |m: &'_ Value| -> Value { let v = m.get("mode").unwrap_or(&NULL); if truthy(v) { v.clone() } else { auto.clone() } };
    if !models.is_object() || pyfmt::eq(&mode_of(models), &s("inherit")) {
        return Ok(json!({"mode": "inherit", "tier": "", "model": "", "floor": "", "reason": "沒有設定 models 或 mode=inherit：沿用主對話模型"}));
    }
    let mode = mode_of(models);
    let defaults = match dget(models, "defaults")? {
        d @ Value::Object(_) => d,
        _ => empty_obj(),
    };
    let mut default = tier_max(&[dget(defaults, &format!("{}@{}", str_of(role), str_of(stage)))?]);
    if default.is_empty() { default = tier_max(&[dget_v(defaults, role)?]); }
    if default.is_empty() { default = tier_max(&[dget(defaults, "*")?]); }
    if default.is_empty() { default = "standard".to_string(); }
    let mut floor = {
        let a = s(&tier_lookup(builtin_floors(), role, stage)?);
        let b = s(&tier_lookup(dget(models, "floors")?, role, stage)?);
        tier_max(&[&a, &b])
    };
    let mods_def = or_obj(pget(profile, "modifiers")?);
    for m in iter_or(modifiers)? {
        let Value::String(name) = &m else { continue };
        // 內建 modifier 的下限一律生效（profile 把 model_floors 拿掉也一樣）；profile 自己的只能再往上加
        for src in [builtin_modifiers(), mods_def] {
            if let Some(md @ Value::Object(_)) = src.as_object().and_then(|o| o.get(name)) {
                let add = s(&tier_lookup(dget(md, "model_floors")?, role, stage)?);
                floor = tier_max(&[&s(&floor), &add]);
            }
        }
    }
    let is_auto = pyfmt::eq(&mode, &auto);
    let (base, mut why) = if is_auto && tier_rank(suggested) >= 0 {
        (str_of(suggested), format!("AI 建議 {}", str_of(suggested)))
    } else if is_auto && truthy(suggested) {
        (default.clone(), format!("AI 建議的等級「{}」不認得，用預設 {default}", str_of(suggested)))
    } else {
        (default.clone(), format!("預設 {default}{}", if pyfmt::eq(&mode, &s("fixed")) { "（mode=fixed，不採 AI 建議）" } else { "" }))
    };
    let tier = tier_max(&[&s(&base), &s(&floor)]);
    if tier != base { why.push_str(&format!("，低於下限 {floor}，往上調成 {tier}")); }
    let model = tier_model(profile, &s(&tier))?;
    Ok(json!({"mode": mode, "tier": tier, "model": model, "floor": floor, "reason": why}))
}

// ------------------------------------------------------------------ recipe 解析

/// 依 `full`（profile 的全部段：內建＋`flow` 宣告的自訂段，見 [`crate::stages::flow_order`]）的順序排；不在 `full` 裡的不留。
fn order(full: &[String], stages: &[Value]) -> R<Vec<Value>> {
    for x in stages { hashable(x)?; }
    Ok(full.iter().filter(|x| has_str(stages, x)).map(|x| s(x)).collect())
}

/// `stages` 裡不在 `full` 的字串（不是內建的段、profile 的 flow 也沒有宣告），照原順序。
fn unknown_stages(full: &[String], stages: &[Value]) -> Vec<String> {
    stages.iter().filter_map(Value::as_str).filter(|x| !full.iter().any(|f| f == x)).map(str::to_string).collect()
}

fn union_vec(mut out: Vec<Value>, b: &Value) -> R<Vec<Value>> {
    for x in iter_or(b)? {
        if !has(&out, &x) { out.push(x); }
    }
    Ok(out)
}

fn union(a: &Value, b: &Value) -> R<Vec<Value>> {
    union_vec(iter_or(a)?, b)
}

fn level(v: &Value) -> R<i32> {
    hashable(v)?;
    Ok(match v.as_str() {
        Some("never") => 0,
        Some("always") => 2,
        _ => 1,
    })
}

fn stricter<'a>(a: &'a Value, b: &'a Value) -> R<&'a Value> {
    Ok(if level(a)? >= level(b)? { a } else { b })
}

/// 讀 profile（methodology.json）；檔案不存在回 None。
pub fn load_profile(path: &Path) -> R<Option<Value>> {
    if !path.is_file() { return Ok(None); }
    let text = std::fs::read_to_string(path).map_err(|e| perr("OSError", format!("{}：{e}", path.display())))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| perr("JSONDecodeError", format!("{}：{e}", path.display())))?;
    // 繼承（extends）：合併後的結果；解析失敗和 JSON 讀不懂一樣回錯誤
    crate::profile::resolve_file(path, doc).map(Some).map_err(|e| perr("ValueError", format!("{}：{e}", path.display())))
}

/// 回傳解析後的 recipe：`{...recipe 欄位, name, requested, resolved_from, modifiers, size, errors}`。
pub fn resolve(profile: &Value, name: &Value, modifiers: &Value, size: &Value) -> R<Value> {
    let recipes = or_obj(pget(profile, "recipes")?);
    let mods_def = or_obj(pget(profile, "modifiers")?);
    let mut errors: Vec<Value> = Vec::new();
    let feature = s("feature");
    let requested = if truthy(name) { name.clone() } else { feature.clone() };
    let (base, used, src) = if contains(recipes, &requested)? {
        (index(recipes, &requested)?, requested.clone(), "profile".to_string())
    } else if contains(recipes, &feature)? {
        let src = if truthy(name) && !pyfmt::eq(name, &feature) { format!("profile（{} 不存在，當成 feature）", str_of(&requested)) } else { "profile".to_string() };
        (index(recipes, &feature)?, feature.clone(), src)
    } else {
        let src = if !truthy(recipes) { "fallback（profile 沒有 recipes）".to_string() } else { format!("fallback（{} 不存在）", str_of(&requested)) };
        (fallback_feature().clone(), feature.clone(), src)
    };
    let bo = base.as_object().ok_or_else(|| no_attr(&base, "items"))?;
    let mut r: Map<String, Value> = bo.iter().filter(|(k, _)| k.as_str() != "small").map(|(k, v)| (k.clone(), v.clone())).collect();
    let auto = s("auto");
    let t0 = r.entry("test").or_insert_with(|| json!({})).clone();
    let mut t_dr = dget_d(&t0, "design_review", &auto)?.clone();
    let mut t_sr = dget_d(&t0, "security_review", &auto)?.clone();
    let mut t_modes = iter_or(dget(&t0, "modes")?)?;
    for k in ["research_angles", "design_lenses", "deliver_axes"] {
        r.entry(k).or_insert_with(|| json!([]));
    }
    r.entry("build_kinds").or_insert_with(|| strs(CODE_KINDS));
    r.entry("publishable").or_insert(Value::Bool(true));
    r.entry("followup").or_insert(Value::Bool(false));
    r.entry("split").or_insert(Value::Bool(false));
    let stages0 = r.get("stages").cloned().unwrap_or_else(|| json!([]));
    if !contains(&stages0, &s("plan"))? {
        errors.push(s(&format!("recipe {} 沒有 plan 段（intent 不能省），改加上 plan", str_of(&used))));
    }
    let mut with_plan = vec![s("plan")];
    with_plan.extend(iter(&stages0)?);
    // profile 有 flow 時，段名要認得：內建七段，或 flow 宣告過的自訂段。不認得的不能默默略過（呼叫端看到 errors 會停下交人）。
    // 沒有 flow 的 profile 照舊（不是內建的段名直接略過；那種 profile 由 schema 的語意規則擋），輸出和原本逐字相同
    let full = crate::stages::flow_order(profile);
    let has_flow = profile.get("flow").is_some_and(Value::is_object);
    for x in unknown_stages(&full, &with_plan).into_iter().filter(|_| has_flow) {
        errors.push(s(&format!("recipe {} 的段 {x} 不認得（不是內建的段，profile 的 flow 也沒有宣告）", str_of(&used))));
    }
    let mut stages = order(&full, &with_plan)?;
    if pyfmt::eq(size, &s("small")) {
        if let Some(Value::Object(sm)) = bo.get("small") {
            if let Some(v) = sm.get("research_angles") { r.insert("research_angles".into(), Value::Array(iter(v)?)); }
            if let Some(v) = sm.get("split") { r.insert("split".into(), Value::Bool(truthy(v))); }
        }
    }
    let mut applied: Vec<Value> = Vec::new();
    for m in iter_or(modifiers)? {
        let own = dget_v(mods_def, &m)?;
        let md = if truthy(own) { own } else if !truthy(mods_def) { dget_v(builtin_modifiers(), &m)? } else { &NULL };
        if md.is_null() {
            errors.push(s(&format!("不認得的 modifier：{}", str_of(&m))));
            continue;
        }
        applied.push(m.clone());
        for x in unknown_stages(&full, &iter_or(dget(md, "add_stages")?)?).into_iter().filter(|_| has_flow) {
            errors.push(s(&format!("modifier {} 的 add_stages 有不認得的段：{x}", str_of(&m))));
        }
        stages = order(&full, &union_vec(stages, dget(md, "add_stages")?)?)?;
        for k in ["research_angles", "design_lenses", "deliver_axes"] {
            let merged = union(&r[k], dget(md, k)?)?;
            r.insert(k.into(), Value::Array(merged));
        }
        let mt = or_obj(dget(md, "test")?);
        for (k, cur) in [("design_review", &mut t_dr), ("security_review", &mut t_sr)] {
            if contains(mt, &s(k))? {
                let new = index(mt, &s(k))?;
                *cur = stricter(&new, cur)?.clone();
            }
        }
        t_modes = union_vec(t_modes, dget(mt, "modes")?)?;
        if truthy(dget(md, "split")?) { r.insert("split".into(), Value::Bool(true)); }
    }
    // 有 design 段就一定要有 lens（不然 design 段沒有人挑錯）
    if has_str(&stages, "design") && !truthy(&r["design_lenses"]) {
        r.insert("design_lenses".into(), strs(&["coverage", "plan"]));
    }
    if has_str(&stages, "test") && !has(&t_modes, &s("verify")) {
        t_modes.insert(0, s("verify"));
    }
    r.insert("stages".into(), Value::Array(stages));
    r.insert("test".into(), json!({"design_review": t_dr, "security_review": t_sr, "modes": t_modes}));
    r.insert("name".into(), used);
    r.insert("requested".into(), requested);
    r.insert("resolved_from".into(), s(&src));
    r.insert("modifiers".into(), Value::Array(applied));
    r.insert("size".into(), if truthy(size) { size.clone() } else { s("") });
    r.insert("errors".into(), Value::Array(errors));
    Ok(Value::Object(r))
}

/// `[x for x in old.get(key) or [] if x not in (new 那邊的清單 or [])]`；new 那邊每個 x 才求值一次（old 是空的就不會碰 new）。
/// `or_empty` 是 false 時沒有 `or []`：new 那邊是 None 就是 TypeError。
fn lost_items(old_list: &Value, or_empty: bool, new_list: impl Fn() -> R<Value>) -> R<Vec<Value>> {
    let mut lost = Vec::new();
    for x in iter_or(old_list)? {
        let n = new_list()?;
        if (or_empty && !truthy(&n)) || !contains(&n, &x)? { lost.push(x); }
    }
    Ok(lost)
}

/// new 不比 old 鬆就回空清單；否則列出變鬆的地方（escalate 據此拒絕）。
pub fn stricter_or_equal(new: &Value, old: &Value) -> R<Vec<String>> {
    let mut errs = Vec::new();
    for (key, what) in [("stages", "段"), ("design_lenses", "design lens"), ("deliver_axes", "deliver 審查軸")] {
        let lost = lost_items(dget(old, key)?, true, || dget(new, key).cloned())?;
        if !lost.is_empty() { errs.push(format!("少了{what}：{}", join_strs(&lost)?)); }
    }
    // Python 這一行是 `x not in (new 的 modes) or []`：運算子優先序讓 `or []` 沒有作用，new 沒有 modes 時丟 TypeError（照舊，只會更嚴）
    let lost = lost_items(dget(or_obj(dget(old, "test")?), "modes")?, false, || dget(or_obj(dget(new, "test")?), "modes").cloned())?;
    if !lost.is_empty() { errs.push(format!("少了測試模式：{}", join_strs(&lost)?)); }
    let auto = s("auto");
    for k in ["design_review", "security_review"] {
        let a = dget_d(or_obj(dget(new, "test")?), k, &auto)?;
        let b = dget_d(or_obj(dget(old, "test")?), k, &auto)?;
        if level(a)? < level(b)? { errs.push(format!("{k} 從 {} 降成 {}", str_of(b), str_of(a))); }
    }
    Ok(errs)
}

/// escalate 用：以 new 為準，再把 old 有、new 沒有的段、lens、審查軸、測試模式補回去，審查等級取較嚴者。
/// 結果一定 stricter_or_equal 於 old 與 new。build_kinds、plan_mode 照 new（升級本來就是換一種做法）。
pub fn merge_stricter(new: &Value, old: &Value) -> R<Value> {
    merge_stricter_in(&Value::Null, new, old)
}

/// 同 [`merge_stricter`]，段的順序照 `profile`（內建段＋`flow` 宣告的自訂段）。
/// 兩邊有、profile 卻沒宣告的自訂段不會被丟掉（丟掉等於少一段）：排在最後一個內建段之前，由 `stages::flow_tamper` 另外擋。
pub fn merge_stricter_in(profile: &Value, new: &Value, old: &Value) -> R<Value> {
    let mut m = match new {
        Value::Object(o) => o.clone(),
        _ => return Err(no_attr(new, "get")),
    };
    let both = union(dget(new, "stages")?, dget(old, "stages")?)?;
    let full = crate::stages::flow_order(profile);
    let mut merged = order(&full, &both)?;
    // 只有給了 profile 才保留（沒給 profile 的 merge_stricter 照舊：不是內建的段名直接略過，輸出和原本逐字相同）
    let stray: Vec<Value> = unknown_stages(&full, &both).iter().filter(|x| crate::stages::flow_id_ok(x)).map(|x| s(x)).collect();
    if profile.is_object() && !stray.is_empty() {
        let at = merged.iter().rposition(|x| x.as_str() == Some(crate::stages::FLOW_NO_ANCHOR)).unwrap_or(merged.len());
        merged.splice(at..at, stray);
    }
    m.insert("stages".into(), Value::Array(merged));
    for k in ["design_lenses", "deliver_axes", "research_angles"] {
        m.insert(k.into(), Value::Array(union(dget(new, k)?, dget(old, k)?)?));
    }
    let auto = s("auto");
    let mut tv = m.entry("test").or_insert_with(|| json!({})).clone();
    let ot = or_obj(dget(old, "test")?);
    for k in ["design_review", "security_review"] {
        let v = stricter(dget_d(&tv, k, &auto)?, dget_d(ot, k, &auto)?)?.clone();
        tv[k] = v;
    }
    let modes = union(dget(&tv, "modes")?, dget(ot, "modes")?)?;
    tv["modes"] = Value::Array(modes);
    m.insert("test".into(), tv);
    // 後續 run 的義務只能保留、不能因 escalate 消失（X3 I1）：升級後是否補完整流程，由 close 檢查整條流程是否完成
    m.insert("followup".into(), Value::Bool(truthy(dget(new, "followup")?) || truthy(dget(old, "followup")?)));
    m.insert("modifiers".into(), Value::Array(union(dget(new, "modifiers")?, dget(old, "modifiers")?)?));
    Ok(Value::Object(m))
}

/// escalate 拒絕的情況：新 recipe 本身少了舊 recipe 的段（明顯是降級）。其他差異由 merge_stricter 補回。
pub fn escalate_errors(new: &Value, old: &Value) -> R<Vec<String>> {
    let lost = lost_items(dget(old, "stages")?, true, || dget(new, "stages").cloned())?;
    if lost.is_empty() { return Ok(Vec::new()); }
    let name = str_of(dget(new, "name")?);
    Ok(vec![format!("新 recipe {name} 少了段：{}（降級一律拒絕；要降級只能 abandon 後重開 run）", join_strs(&lost)?)])
}

// ------------------------------------------------------------------ diff 形狀
// 分類一律往嚴的方向判（X3 I6）：認不出來的都算 code。
// - doc：只認文件副檔名；.txt 只有在 docs/ 底下才算（CMakeLists.txt、requirements*.txt 不是文件）。
// - test：根目錄的測試目錄（tests/、test/、__tests__/、e2e/、spec/）底下的檔，或任何位置的測試檔名；
//   巢狀的 src/spec/、app/test/ 底下不是測試檔名的檔算 code。
// - migration、manifest（依賴宣告與鎖檔）、agent_config（agent 與 CI 設定）可和其他類並存，各自要求更嚴的流程。

struct Shapes {
    doc_ext: Regex,
    doc_txt: Regex,
    test_root: Regex,
    test_name: Regex,
    migration: Regex,
    manifest: Regex,
    agent_config: Regex,
}

/// Python 的 `$` 也會對到結尾換行之前；`re.I` 用 Rust 的 `(?i)`（`classify` 另外補兩個土耳其文的 i）。
fn rx(p: &str) -> Regex {
    Regex::new(&format!("(?i){}", p.replace('$', r"(?:\n?\z)"))).expect("內建正規式")
}

fn shapes() -> &'static Shapes {
    static V: OnceLock<Shapes> = OnceLock::new();
    V.get_or_init(|| Shapes {
        doc_ext: rx(r"\.(md|mdx|rst|adoc)$"),
        doc_txt: rx(r"^docs?/.*\.txt$"),
        test_root: rx(r"^(tests?|__tests__|e2e|spec)/"),
        test_name: rx(r"(^|/)test_[^/]+\.[a-z]+$|_test\.[a-z]+$|_spec\.[a-z]+$|\.(test|spec)\.[a-z]+$|(^|/)conftest\.py$"),
        migration: rx(r"(^|/)migrations?/|(^|/)db/migrate/|(^|/)alembic/|(^|/)(flyway|liquibase)/|\.sql$"),
        manifest: rx(concat!(
            r"(^|/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|npm-shrinkwrap\.json|bun\.lockb?|poetry\.lock|uv\.lock|Pipfile\.lock|Cargo\.lock|go\.sum",
            r"|Gemfile\.lock|composer\.lock|packages\.lock\.json|package\.json|pyproject\.toml|setup\.py|setup\.cfg|requirements[^/]*\.txt|constraints[^/]*\.txt",
            r"|Pipfile|go\.mod|Cargo\.toml|Gemfile|composer\.json|pom\.xml|build\.gradle(\.kts)?|settings\.gradle(\.kts)?|gradle\.lockfile|libs\.versions\.toml",
            r"|[^/]+\.csproj|[^/]+\.fsproj|Directory\.Packages\.props|packages\.config|mix\.exs|mix\.lock|pubspec\.(yaml|lock)|Podfile(\.lock)?",
            r"|Package\.(swift|resolved)|deno\.(json|jsonc|lock)|vcpkg\.json|conanfile\.(txt|py)|flake\.lock)$",
            r"|(^|/)requirements/[^/]+\.(txt|in)$",
        )),
        agent_config: rx(r"^\.claude/|^\.github/|(^|/)(CLAUDE|AGENTS)\.md$|(^|/)\.mcp\.json$"),
    })
}

/// profile 的 `diff_classes_extra` 可以加樣式的類別。只能加嚴：把更多路徑算成這幾類；
/// doc、test 不在裡面（把路徑歸成文件或測試等於放寬）。
pub const DIFF_EXTRA_CLASSES: [&str; 4] = ["migration", "manifest", "agent_config", "code"];
/// 每一類最多幾條樣式、每條最多幾個字。
pub const DIFF_EXTRA_MAX: usize = 32;
pub const DIFF_EXTRA_PATTERN_MAX: usize = 300;

/// 編譯過的 `diff_classes_extra`（專案自己加的路徑分類樣式；不分大小寫，和內建樣式一樣）。
#[derive(Default)]
pub struct DiffExtra {
    by: Vec<(&'static str, Vec<Re>)>,
}

impl DiffExtra {
    /// 這個路徑另外命中的類別（照 [`DIFF_EXTRA_CLASSES`] 的順序）。
    pub fn hits(&self, path: &str) -> Vec<&'static str> {
        let p = norm_path(path);
        self.by.iter().filter(|(_, res)| res.iter().any(|re| re.is_match(&p))).map(|(k, _)| *k).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by.iter().all(|(_, v)| v.is_empty())
    }
}

/// 讀 `diff_classes_extra`（`v` 是那個鍵的值；None 或 null＝沒設定）。寫得不對回 `Err(每一處的說明)`：
/// 呼叫端（publish、merge-gate、check-diff）要往嚴的方向處理，不能當成沒設定。
pub fn diff_extra_of(v: Option<&Value>) -> Result<DiffExtra, Vec<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(DiffExtra::default()) };
    let what = "diff_classes_extra";
    let Some(o) = v.as_object() else { return Err(vec![format!("{what} 要是物件")]) };
    let mut errs = Vec::new();
    for k in o.keys().filter(|k| !DIFF_EXTRA_CLASSES.contains(&k.as_str())) {
        errs.push(format!("{what} 不認得的類別：{k}（只能加 {}；把路徑歸成 doc 或 test 等於放寬，不開放）", DIFF_EXTRA_CLASSES.join("、")));
    }
    let mut by = Vec::new();
    for cls in DIFF_EXTRA_CLASSES {
        let Some(list) = o.get(cls).filter(|l| !l.is_null()) else { continue };
        let Some(items) = list.as_array() else {
            errs.push(format!("{what}.{cls} 要是正規式的陣列"));
            continue;
        };
        if items.len() > DIFF_EXTRA_MAX {
            errs.push(format!("{what}.{cls} 最多 {DIFF_EXTRA_MAX} 條（收到 {} 條）", items.len()));
            continue;
        }
        let mut res = Vec::new();
        for (i, it) in items.iter().enumerate() {
            match it.as_str().filter(|t| !t.trim().is_empty() && t.chars().count() <= DIFF_EXTRA_PATTERN_MAX) {
                None => errs.push(format!("{what}.{cls}[{i}] 要是 1–{DIFF_EXTRA_PATTERN_MAX} 個字的正規式")),
                Some(src) => match Re::python(src, "i") {
                    Ok(re) => res.push(re),
                    Err(e) => errs.push(format!("{what}.{cls}[{i}] {e}")),
                },
            }
        }
        by.push((cls, res));
    }
    if errs.is_empty() { Ok(DiffExtra { by }) } else { Err(errs) }
}

/// profile 的 `diff_classes_extra`（profile 是 null 或沒寫＝沒設定）。
pub fn diff_extra(profile: &Value) -> Result<DiffExtra, Vec<String>> {
    diff_extra_of(profile.get("diff_classes_extra"))
}

/// 分類用的路徑寫法：反斜線換成斜線、去掉開頭的 `./`。
fn norm_path(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while p.starts_with("./") { p.drain(..2); }
    p
}

/// 一個路徑屬於哪幾類（code、test、doc、migration、manifest、agent_config）。回排序過的清單。
pub fn classify(path: &str) -> Vec<&'static str> {
    classify_with(path, None)
}

/// 同 [`classify`]，另外套用專案加的樣式（只會多歸類，不會少）：
/// 命中 migration、manifest、agent_config 的路徑和內建樣式命中時一樣處理（不再算文件或測試目錄裡的一般檔）；
/// 命中 code 的路徑在原本的分類之外再算一次 code（原本是文件或測試的仍然保留那一類，兩邊的要求都要滿足）。
pub fn classify_with(path: &str, extra: Option<&DiffExtra>) -> Vec<&'static str> {
    let added = extra.map(|e| e.hits(path)).unwrap_or_default();
    let mut p = path.replace('\\', "/");
    while p.starts_with("./") { p.drain(..2); }
    // Python 的 re.I 把 İ（U+0130）與 ı（U+0131）當成 i；Rust 的大小寫折疊不會
    let p = p.replace(['\u{130}', '\u{131}'], "i");
    let sh = shapes();
    let mut cats = Vec::new();
    if sh.migration.is_match(&p) || added.contains(&"migration") { cats.push("migration"); }
    if sh.manifest.is_match(&p) || added.contains(&"manifest") { cats.push("manifest"); }
    if sh.agent_config.is_match(&p) || added.contains(&"agent_config") { cats.push("agent_config"); }
    if sh.test_name.is_match(&p) || (sh.test_root.is_match(&p) && cats.is_empty()) {
        cats.push("test");
    } else if (sh.doc_ext.is_match(&p) || sh.doc_txt.is_match(&p)) && cats.is_empty() {
        cats.push("doc");
    } else {
        cats.push("code");
    }
    if added.contains(&"code") && !cats.contains(&"code") { cats.push("code"); }
    cats.sort_unstable();
    cats
}

#[cfg(test)]
mod diff_extra_tests {
    use super::*;

    fn paths(p: &[&str]) -> Value {
        Value::Array(p.iter().map(|x| s(x)).collect())
    }

    #[test]
    fn no_setting_is_identical_to_builtin() {
        let none = diff_extra(&Value::Null).expect("沒設定");
        assert!(none.is_empty());
        for p in ["docs/a.md", "src/a.py", "tests/test_a.py", "db/migrations/1.sql", "package.json", ".github/x.yml", "README.md", "e2e/helpers.ts"] {
            assert_eq!(classify_with(p, Some(&none)), classify(p), "{p}");
        }
    }

    #[test]
    fn extra_classes_only_add() {
        let x = diff_extra(&json!({"diff_classes_extra": {"migration": ["(^|/)schema/"], "manifest": ["(^|/)deps\\.txt$"], "agent_config": ["^tools/bots/"], "code": ["^docs/api/", "^tests/fixtures/"]}})).expect("合法的設定");
        assert_eq!(classify_with("db/Schema/orders.yml", Some(&x)), ["code", "migration"]);
        assert_eq!(classify_with("docs/schema/orders.md", Some(&x)), ["code", "migration"], "命中 migration 的文件不再算文件");
        assert_eq!(classify_with("deps.txt", Some(&x)), ["code", "manifest"]);
        assert_eq!(classify_with("tools/bots/a.md", Some(&x)), ["agent_config", "code"]);
        assert_eq!(classify_with("docs/api/a.md", Some(&x)), ["code", "doc"], "歸成 code 的文件兩類都算");
        assert_eq!(classify_with("tests/fixtures/test_a.py", Some(&x)), ["code", "test"]);
        assert_eq!(classify_with("docs/guide/a.md", Some(&x)), ["doc"], "沒命中的照舊");
        // 每個路徑的分類都只多不少
        for p in ["docs/a.md", "src/a.py", "tests/test_a.py", "db/migrations/1.sql", "package.json", "docs/api/a.md", "db/schema/x.yml", "deps.txt"] {
            let (base, with) = (classify(p), classify_with(p, Some(&x)));
            assert!(base.iter().all(|c| with.contains(c) || (*c == "doc" || *c == "test") && with.contains(&"code")), "{p}：{base:?} → {with:?}");
        }
    }

    #[test]
    fn shortfalls_only_grow() {
        let prof = json!({"recipes": builtin_recipes(), "modifiers": builtin_modifiers()});
        let docs = resolve(&prof, &s("docs"), &json!([]), &Value::Null).expect("docs recipe");
        let x = diff_extra(&json!({"diff_classes_extra": {"code": ["^docs/api/"], "migration": ["(^|/)schema/"]}})).expect("合法的設定");
        assert!(diff_shortfalls(&docs, &paths(&["docs/api/a.md"])).expect("內建").is_empty());
        let got = diff_shortfalls_with(&docs, &paths(&["docs/api/a.md"]), Some(&x)).expect("加嚴");
        assert!(got.iter().any(|e| e.contains("不允許改產品程式")), "{got:?}");
        let bug = resolve(&prof, &s("bugfix"), &json!([]), &Value::Null).expect("bugfix recipe");
        assert!(diff_shortfalls(&bug, &paths(&["db/schema/a.yml"])).expect("內建").is_empty());
        let got = diff_shortfalls_with(&bug, &paths(&["db/schema/a.yml"]), Some(&x)).expect("加嚴");
        assert!(got.iter().any(|e| e.contains("改到 migration")), "{got:?}");
    }

    #[test]
    fn bad_settings_are_errors() {
        for bad in [json!({"doc": ["^src/"]}), json!({"test": ["^src/"]}), json!({"code": ["src/("]}), json!({"code": "src/"}), json!({"code": [1]}), json!({"code": [""]}), json!("x"), json!({"code": vec!["a"; DIFF_EXTRA_MAX + 1]})] {
            assert!(diff_extra(&json!({"diff_classes_extra": bad.clone()})).is_err(), "{bad} 應該被拒絕");
        }
        assert!(diff_extra(&json!({"diff_classes_extra": null})).is_ok());
        assert!(diff_extra(&json!({"diff_classes_extra": {}})).is_ok());
    }
}

/// `sorted(set(...))` 的 repr：字串照字碼排，數字照大小排，混型別（Python 比不了）是 TypeError。
fn sorted_set_repr(kinds: &[Value]) -> R<String> {
    let mut v: Vec<&Value> = kinds.iter().collect();
    if v.len() > 1 {
        if v.iter().all(|x| x.is_string()) {
            v.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        } else if v.iter().all(|x| matches!(x, Value::Bool(_) | Value::Number(_))) {
            let n = |x: &Value| match x {
                Value::Bool(b) => *b as i64 as f64,
                other => other.as_f64().unwrap_or(0.0),
            };
            v.sort_by(|a, b| n(a).partial_cmp(&n(b)).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            return Err(perr("TypeError", "'<' not supported between instances of different types".to_string()));
        }
    }
    Ok(format!("[{}]", v.iter().map(|x| pyfmt::repr(x)).collect::<Vec<_>>().join(", ")))
}

/// 實際 diff 需要、但 recipe 沒有的流程；空清單＝相符。
pub fn diff_shortfalls(r: &Value, paths: &Value) -> R<Vec<String>> {
    diff_shortfalls_with(r, paths, None)
}

/// 同 [`diff_shortfalls`]，路徑分類另外套用專案加的樣式（[`classify_with`]；只會多出不符，不會少）。
pub fn diff_shortfalls_with(r: &Value, paths: &Value, extra: Option<&DiffExtra>) -> R<Vec<String>> {
    let mut out = Vec::new();
    let name = || -> R<String> { Ok(str_of(dget(r, "name")?)) };
    if !truthy(dget_d(r, "publishable", &Value::Bool(true))?) {
        out.push(format!("recipe {} 不能推送（publishable=false）", name()?));
    }
    let mut kinds: Vec<Value> = Vec::new();
    for k in iter_or(dget(r, "build_kinds")?)? {
        hashable(&k)?;
        if !has(&kinds, &k) { kinds.push(k); }
    }
    let mut by: Vec<(&'static str, Vec<String>)> = Vec::new();
    for p in iter(paths)? {
        let Value::String(p) = p else { return Err(no_attr(&p, "replace")) };
        for c in classify_with(&p, extra) {
            match by.iter_mut().find(|(k, _)| *k == c) {
                Some((_, list)) => list.push(p.clone()),
                None => by.push((c, vec![p.clone()])),
            }
        }
    }
    let got = |cat: &str| -> Option<&Vec<String>> { by.iter().find(|(k, _)| *k == cat).map(|(_, v)| v) };
    let head = |list: &Vec<String>, n: usize| list.iter().take(n).map(String::as_str).collect::<Vec<_>>().join("、");
    for (cat, what) in [("code", "產品程式"), ("test", "測試"), ("doc", "文件")] {
        if let Some(list) = got(cat) {
            if !has(&kinds, &s(cat)) {
                out.push(format!("recipe {} 不允許改{what}（build_kinds={}），卻改了：{}", name()?, sorted_set_repr(&kinds)?, head(list, 5)));
            }
        }
    }
    if let Some(list) = got("migration") {
        let no_design = !contains(dget_d(r, "stages", &json!([]))?, &s("design"))?;
        let lenses = dget(r, "design_lenses")?;
        if no_design || !truthy(lenses) || !contains(lenses, &s("rollback"))? {
            out.push(format!("改到 migration（{}），需要 design 段與回滾計畫（加 modifier migration）", head(list, 3)));
        }
    }
    let always = s("always");
    if let Some(list) = got("manifest") {
        if !pyfmt::eq(dget(or_obj(dget(r, "test")?), "security_review")?, &always) {
            out.push(format!("改到依賴宣告或鎖檔（{}），需要安全審查（用 deps recipe 或加 modifier security）", head(list, 3)));
        }
    }
    if let Some(list) = got("agent_config") {
        if !pyfmt::eq(dget(or_obj(dget(r, "test")?), "security_review")?, &always) {
            out.push(format!("改到 agent 或 CI 設定（{}），需要安全審查（加 modifier security）", head(list, 3)));
        }
    }
    Ok(out)
}

/// 這個 run 的閘門模式：`{mode, reason}`。
/// strict＝每個閘門都等人核准（預設）；plan-only＝plan 由人核准，其餘閘門先試政策核准（結果乾淨才過），不過就交人。
/// 只會往嚴的方向判：profile 沒寫、寫錯、認不得的 recipe、帶了任何 modifier（ui、security、migration…）一律 strict。
/// plan 段不受它影響，一律由人核准。
pub fn gate_mode(profile: &Value, run_recipe: &Value) -> R<Value> {
    let rr = if run_recipe.is_object() { run_recipe } else { empty_obj() };
    let raw = dget(rr, "name")?;
    let name = if truthy(raw) { str_of(raw) } else { String::new() };
    let recipes = pget(profile, "recipes")?;
    let spec = if recipes.is_object() { dget(recipes, &name)? } else { &NULL };
    if name.is_empty() || !spec.is_object() {
        return Ok(json!({"mode": "strict", "reason": "profile 沒有這個 recipe 的定義"}));
    }
    let mods: Vec<String> = iter_or(dget(rr, "modifiers")?)?.iter().filter(|m| truthy(m)).map(str_of).collect();
    if !mods.is_empty() {
        return Ok(json!({"mode": "strict", "reason": format!("帶了 modifier（{}），每段都由人核准", mods.join("、"))}));
    }
    let mode = dget(spec, "gate_mode")?;
    if mode.as_str() == Some("plan-only") {
        return Ok(json!({"mode": "plan-only", "reason": format!("profile 的 recipes.{name}.gate_mode")}));
    }
    let reason = if mode.is_null() || mode.as_str() == Some("strict") { "profile 沒設 gate_mode（預設 strict）".to_string() } else { format!("gate_mode 的值不認得：{}", str_of(mode)) };
    Ok(json!({"mode": "strict", "reason": reason}))
}

/// run.json 的 recipe；沒有（舊 run）就當成 feature 的完整流程。
pub fn recipe_of_run(run: &Value) -> R<Value> {
    if let Value::Object(rec) = dget(run, "recipe")? {
        if let Some(res @ Value::Object(_)) = rec.get("resolved") { return Ok(res.clone()); }
    }
    resolve(&NULL, &s("feature"), &NULL, &NULL)
}

/// intent 要有的節：基本四節（profile 可覆寫）加上 plan_mode 專屬的節。
pub fn required_sections(profile: &Value, mode: &Value) -> R<Vec<Value>> {
    let plan = or_obj(dget(or_obj(pget(profile, "stages")?), "plan")?);
    let own = dget(plan, "required_sections")?;
    let builtin = strs(BASE_SECTIONS);
    let base = if truthy(own) { own } else { &builtin };
    let modes = or_obj(pget(profile, "plan_modes")?);
    let extra = if contains(modes, mode)? {
        dget(or_obj(dget_v(modes, mode)?), "sections")?
    } else {
        match dget_v(mode_sections(), mode)? {
            Value::Null => &NULL,
            v => v,
        }
    };
    union(base, extra)
}

fn hashes(line: &str) -> usize {
    line.chars().take_while(|c| *c == '#').count()
}

/// `re.match(r"^#{1,6}\s", ln)`。
fn is_heading(line: &str) -> bool {
    let n = hashes(line);
    (1..=6).contains(&n) && line.chars().nth(n).is_some_and(py_space)
}

/// markdown 裡沒有標題或標題下沒有內容的節。標題要含節名（例如「## 4. 驗收條件」）。
pub fn missing_sections(text: &str, sections: &Value) -> R<Vec<Value>> {
    let lines = py_splitlines(text);
    let heads: Vec<usize> = (0..lines.len()).filter(|i| is_heading(lines[*i])).collect();
    let mut miss = Vec::new();
    for sec in iter(sections)? {
        let mut hit = None;
        for &i in &heads {
            let Value::String(name) = &sec else {
                return Err(perr("TypeError", format!("'in <string>' requires string as left operand, not {}", tname(&sec))));
            };
            if lines[i].contains(name.as_str()) {
                hit = Some(i);
                break;
            }
        }
        let Some(i) = hit else {
            miss.push(sec);
            continue;
        };
        let nxt = heads.iter().copied().find(|&j| j > i && hashes(lines[j]) <= hashes(lines[i])).unwrap_or(lines.len());
        let has_body = lines[i + 1..nxt].iter().any(|ln| !py_strip(ln).is_empty() && !is_heading(ln));
        if !has_body { miss.push(s(&format!("{}（只有標題沒有內容）", str_of(&sec)))); }
    }
    Ok(miss)
}

// ------------------------------------------------------------------ JSON 輸出（照 Python 的 json.dumps）

fn dump(v: &Value, indent: Option<usize>, depth: usize, out: &mut String) {
    let nl = |out: &mut String, d: usize| {
        if let Some(n) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(n * d));
        }
    };
    let sep = if indent.is_some() { "," } else { ", " };
    match v {
        Value::Number(n) if n.is_f64() => out.push_str(&pyfmt::float(n.as_f64().unwrap_or(0.0))),
        Value::Array(a) if !a.is_empty() => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 { out.push_str(sep); }
                nl(out, depth + 1);
                dump(x, indent, depth + 1, out);
            }
            nl(out, depth);
            out.push(']');
        }
        Value::Object(o) if !o.is_empty() => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 { out.push_str(sep); }
                nl(out, depth + 1);
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                dump(x, indent, depth + 1, out);
            }
            nl(out, depth);
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `json.dumps(v, ensure_ascii=False, indent=indent)`。
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    dump(v, indent, 0, &mut out);
    out
}

// ------------------------------------------------------------------ CLI

/// `git` 的執行結果（check-diff 用；測試可以換成假的）。
pub struct GitOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn opt<'a>(argv: &'a [String], name: &str, default: &'a str) -> &'a str {
    match argv.iter().position(|a| a == name) {
        Some(i) if i + 1 < argv.len() => &argv[i + 1],
        _ => default,
    }
}

/// CLI 的本體：`teams` 是 `.claude/teams`，`git` 收 git 的參數（不含 `git` 本身）。stdout、stderr 的文字寫進 `out`、`errs`。
pub fn run(teams: &Path, argv: &[String], git: &dyn Fn(&[String]) -> Result<GitOut, String>, out: &mut String, errs: &mut String) -> i32 {
    match argv.first().map(String::as_str) {
        Some("--selftest") => selftest(teams, out),
        Some("resolve") => {
            let p = opt(argv, "--profile", "");
            let path = if p.is_empty() { teams.join("methodology.json") } else { PathBuf::from(p) };
            let mods: Vec<Value> = opt(argv, "--modifiers", "").split(',').filter(|m| !m.is_empty()).map(s).collect();
            let arg = |name: &str| { let v = opt(argv, name, ""); if v.is_empty() { Value::Null } else { s(v) } };
            let r = load_profile(&path).and_then(|prof| resolve(&prof.unwrap_or(Value::Null), &arg("--recipe"), &Value::Array(mods), &arg("--size")));
            match r {
                Ok(r) => {
                    out.push_str(&dumps(&r, Some(2)));
                    out.push('\n');
                    if truthy(&r["errors"]) { 1 } else { 0 }
                }
                Err(e) => {
                    errs.push_str(&format!("{e}\n"));
                    1
                }
            }
        }
        Some("check-diff") => match check_diff(teams, argv, git, out, errs) {
            Ok(code) => code,
            Err(e) => {
                errs.push_str(&format!("{e}\n"));
                1
            }
        },
        _ => {
            out.push_str(USAGE);
            out.push('\n');
            2
        }
    }
}

fn check_diff(teams: &Path, argv: &[String], git: &dyn Fn(&[String]) -> Result<GitOut, String>, out: &mut String, errs: &mut String) -> R<i32> {
    let run_path = PathBuf::from(opt(argv, "--run", "")).join("run.json");
    let text = std::fs::read_to_string(&run_path).map_err(|e| perr("OSError", format!("{}：{e}", run_path.display())))?;
    let run: Value = serde_json::from_str(&text).map_err(|e| perr("JSONDecodeError", format!("{}：{e}", run_path.display())))?;
    let repo = match opt(argv, "--repo", "") {
        "" => match dget(&run, "target_repo")? {
            v if !truthy(v) => ".".to_string(),
            Value::String(v) => v.clone(),
            v => return Err(perr("TypeError", format!("expected str, bytes or os.PathLike object, not {}", tname(v)))),
        },
        v => v.to_string(),
    };
    // 沒帶 --base：守門設定（teams 目錄的 guard.config.json）的 base_remote／base_branch，沒設定就是 origin/main
    let lay = crate::layout::Layout::of(&crate::layout::read_guard(&teams.join("guard.config.json").to_string_lossy()));
    // 這個 run 的 recipe 有專屬的主線（profile 的 conventions.branch.bases）時用它
    let base_default = match crate::layout::run_base(&crate::layout::read_profile(&teams.join("methodology.json").to_string_lossy()), &run) {
        Some(b) => format!("{}/{b}", lay.base_remote),
        None => lay.base_ref(),
    };
    let base = opt(argv, "--base", &base_default);
    // 值會直接交給 git：- 開頭的會被當成 git 的選項
    if base.starts_with('-') || repo.starts_with('-') {
        errs.push_str("--base 與 --repo 不能以 - 開頭（fail-closed）\n");
        return Ok(1);
    }
    // --name-status：改名與刪除的舊路徑也算進來（X3 M2：src/app.py 搬到 docs/app.py 不能被當成只改文件）
    let args: Vec<String> = ["-C", &repo, "diff", "--name-status", "-M", &format!("{base}...HEAD")].iter().map(|x| x.to_string()).collect();
    let p = git(&args).map_err(|e| perr("OSError", e))?;
    if p.code != 0 {
        errs.push_str(&format!("讀不到 diff：{}（fail-closed）\n", py_strip(&p.stderr)));
        return Ok(1);
    }
    let mut paths = Vec::new();
    for ln in py_splitlines(&p.stdout) {
        if py_strip(ln).is_empty() { continue; }
        paths.extend(ln.split('\t').skip(1).filter(|x| !py_strip(x).is_empty()).map(s));
    }
    // 專案加的路徑分類（profile 的 diff_classes_extra）：寫壞了就不比，直接回錯誤（fail-closed）
    let prof = load_profile(&teams.join("methodology.json")).ok().flatten().unwrap_or(Value::Null);
    let extra = match diff_extra(&prof) {
        Ok(x) => x,
        Err(es) => {
            errs.push_str(&format!("profile 的 {}（fail-closed）\n", es.join("；")));
            return Ok(1);
        }
    };
    let found = diff_shortfalls_with(&recipe_of_run(&run)?, &Value::Array(paths), Some(&extra))?;
    for e in &found {
        errs.push_str(&format!("recipe 不足：{e}（要升級就用 awf save-run escalate）\n"));
    }
    out.push_str(&if found.is_empty() { "diff 形狀和 recipe 相符\n".to_string() } else { format!("共 {} 項不符\n", found.len()) });
    Ok(if found.is_empty() { 0 } else { 1 })
}

fn real_git(args: &[String]) -> Result<GitOut, String> {
    // Python 的 text=True 會把 \r\n、\r 換成 \n
    let text = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n").replace('\r', "\n");
    let o = std::process::Command::new("git").args(args).output().map_err(|e| format!("執行不了 git：{e}"))?;
    Ok(GitOut { code: o.status.code().unwrap_or(-1), stdout: text(&o.stdout), stderr: text(&o.stderr) })
}

/// `awf recipe <resolve|check-diff|--selftest>`：teams 目錄是目前目錄的 `.claude/teams`。回傳 exit code。
pub fn main(args: &[String]) -> i32 {
    let (mut out, mut errs) = (String::new(), String::new());
    let teams = Path::new(&crate::project::root_or_cwd(None)).join(".claude/teams");
    let code = run(&teams, args, &real_git, &mut out, &mut errs);
    print!("{out}");
    eprint!("{errs}");
    code
}

// ------------------------------------------------------------------ golden 案例與 selftest

/// `<teams>/golden-set/recipes/*.json`，依檔名排序。
pub fn golden_cases(teams: &Path) -> R<Vec<(PathBuf, Value)>> {
    let dir = teams.join("golden-set").join("recipes");
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".json")))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    let mut out = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).map_err(|e| perr("OSError", format!("{}：{e}", p.display())))?;
        let case = serde_json::from_str(&text).map_err(|e| perr("JSONDecodeError", format!("{}：{e}", p.display())))?;
        out.push((p, case));
    }
    Ok(out)
}

/// golden case 用哪份 profile：`"builtin"`（預設）、`"none"`，或直接給一份。
pub fn case_profile(case: &Value) -> R<Value> {
    let builtin = s("builtin");
    let prof = dget_d(case, "profile", &builtin)?;
    Ok(match prof.as_str() {
        Some("builtin") => builtin_profile(),
        Some("none") => Value::Null,
        _ => prof.clone(),
    })
}

/// golden case 的實際輸出（JS 版用同一個格式比對）。
pub fn run_case(case: &Value) -> R<Value> {
    let prof = case_profile(case)?;
    let r = resolve(&prof, dget(case, "recipe")?, dget(case, "modifiers")?, dget(case, "size")?)?;
    let mut out = Map::new();
    for k in ["name", "stages", "plan_mode", "research_angles", "design_lenses", "test", "deliver_axes", "build_kinds", "publishable", "followup", "split", "modifiers", "errors"] {
        out.insert(k.into(), index(&r, &s(k))?);
    }
    let strings = |v: Vec<String>| Value::Array(v.iter().map(|x| s(x)).collect());
    if let Some(oc) = case.get("old") {
        let old = resolve(&prof, dget(oc, "recipe")?, dget(oc, "modifiers")?, dget(oc, "size")?)?;
        out.insert("escalate_errors".into(), strings(escalate_errors(&r, &old)?));
        let merged = merge_stricter_in(&prof, &r, &old)?;
        out.insert("merged_stages".into(), merged["stages"].clone());
        let ok = stricter_or_equal(&merged, &old)?.is_empty() && stricter_or_equal(&merged, &r)?.is_empty();
        out.insert("merged_ok".into(), Value::Bool(ok));
    }
    if let Some(paths) = case.get("paths") {
        out.insert("shortfalls".into(), strings(diff_shortfalls(&r, paths)?));
    }
    if let Some(mc) = case.get("model_cases") {
        let mut models = Vec::new();
        for m in iter(mc)? {
            let mut got = resolve_model(&prof, dget(&m, "role")?, dget(&m, "stage")?, dget(&m, "suggested")?, &r["modifiers"])?;
            if let Some(o) = got.as_object_mut() { o.shift_remove("reason"); }
            models.push(got);
        }
        out.insert("models".into(), Value::Array(models));
    }
    Ok(Value::Object(out))
}

struct Checks<'a> {
    out: &'a mut String,
    ok: bool,
}

impl Checks<'_> {
    fn check(&mut self, name: &str, cond: R<bool>, detail: impl FnOnce() -> String) {
        let (pass, detail) = match cond {
            Ok(true) => (true, String::new()),
            Ok(false) => (false, detail()),
            Err(e) => (false, e.to_string()),
        };
        self.out.push_str(&format!("  [{}] {name}{}\n", if pass { "通過" } else { "失敗" }, if pass { String::new() } else { format!("：{detail}") }));
        self.ok = self.ok && pass;
    }
}

/// `--selftest`：輸出寫進 `out`，回傳 exit code。`teams` 底下要有 `golden-set/recipes/`。
pub fn selftest(teams: &Path, out: &mut String) -> i32 {
    out.push_str("recipe selftest\n");
    let mut c = Checks { out, ok: true };
    let no = String::new;
    let bi = builtin_profile();
    let n = &NULL;
    let rz = |name: &str| resolve(&bi, &s(name), n, n);
    let rm_ = |name: &str, mods: &[&str]| resolve(&bi, &s(name), &strs(mods), n);
    let sdlc = strs(SDLC);
    let recipes = builtin_recipes().as_object().expect("內建 recipes");
    let modifiers = builtin_modifiers().as_object().expect("內建 modifiers");
    let empty = |v: R<Vec<String>>| v.map(|x| x.is_empty());
    let paths = |xs: &[&str]| strs(xs);

    c.check("內建 11 個 recipe、5 個 modifier", Ok(recipes.len() == 11 && modifiers.len() == 5),
        || pyfmt::repr(&Value::Array(recipes.keys().map(|k| s(k)).collect())));
    c.check("每個 recipe 都有 plan 段", Ok(recipes.values().all(|r| has_str(pyfmt::items(r.get("stages")), "plan"))), no);
    c.check("會寫程式的 recipe 都有 build、deliver、merge",
        Ok(recipes.values().filter(|r| truthy(&r["build_kinds"])).all(|r| ["build", "deliver", "merge"].iter().all(|x| has_str(pyfmt::items(r.get("stages")), x)))), no);
    c.check("沒有 recipes 的舊 profile 當成 feature 完整流程", (|| Ok(
        resolve(&json!({"name": "x"}), &s("bugfix"), n, n)?["stages"] == sdlc && resolve(n, n, n, n)?["name"] == "feature"))(), no);
    c.check("未知 recipe 當成 feature", (|| Ok(rz("nope")?["name"] == "feature" && rz("nope")?["stages"] == sdlc))(), no);
    let bug = rz("bugfix").unwrap_or(Value::Null);
    c.check("bugfix 沒有 design、plan_mode=diagnose", Ok(!has_str(pyfmt::items(bug.get("stages")), "design") && bug["plan_mode"] == "diagnose"), no);
    let loose = (|| -> R<Vec<Value>> {
        let mut loose = Vec::new();
        for m in modifiers.keys() {
            for name in recipes.keys() {
                if !stricter_or_equal(&rm_(name, &[m])?, &rz(name)?)?.is_empty() { loose.push(s(&format!("{m}@{name}"))); }
            }
        }
        Ok(loose)
    })();
    c.check("modifier 疊上去只會加嚴（全部組合）", loose.clone().map(|l| l.is_empty()), || pyfmt::repr(&Value::Array(loose.clone().unwrap_or_default())));
    let mig = rm_("bugfix", &["migration"]).unwrap_or(Value::Null);
    c.check("migration 加回 design 段與 rollback lens", Ok(has_str(pyfmt::items(mig.get("stages")), "design") && has_str(pyfmt::items(mig.get("design_lenses")), "rollback")), no);
    c.check("不認得的 modifier 記在 errors", (|| Ok(truthy(&rm_("bugfix", &["zzz"])?["errors"])))(), no);
    let feat = rz("feature").unwrap_or(Value::Null);
    c.check("escalate：bugfix → feature 接受，合併後不比兩者鬆、保留 regression", (|| {
        let merged = merge_stricter(&feat, &bug)?;
        Ok(escalate_errors(&feat, &bug)?.is_empty() && stricter_or_equal(&merged, &bug)?.is_empty() && stricter_or_equal(&merged, &feat)?.is_empty()
            && has_str(pyfmt::items(merged["test"].get("modes")), "regression"))
    })(), no);
    c.check("escalate：feature → bugfix 少了 design 段，拒絕", escalate_errors(&bug, &feat).map(|e| !e.is_empty()), no);
    c.check("escalate：hotfix → bugfix 仍保留後續 run 的義務（不能靠升級清掉）", (|| Ok(merge_stricter(&bug, &rz("hotfix")?)?["followup"] == true))(), no);
    c.check("size=small 只影響 feature 的調研角度", (|| Ok(resolve(&bi, &s("feature"), n, &s("small"))?["research_angles"] == strs(&["requirement", "code-contracts"])))(), no);
    let docs = rz("docs").unwrap_or(Value::Null);
    c.check("docs 改程式 → 不符", (|| Ok(!diff_shortfalls(&docs, &paths(&["src/a.py"]))?.is_empty() && diff_shortfalls(&docs, &paths(&["docs/a.md", "README.md"]))?.is_empty()))(), no);
    let gm = |prof: &Value, name: &str, mods: &[&str]| -> R<bool> { Ok(gate_mode(prof, &json!({"name": name, "modifiers": mods}))?["mode"] == "strict") };
    c.check("gate_mode：內建 profile 的 docs、test-only 是 plan-only，其餘 strict", (|| {
        let mut ok = !gm(&bi, "docs", &[])? && !gm(&bi, "test-only", &[])?;
        for name in recipes.keys().filter(|k| !["docs", "test-only"].contains(&k.as_str())) { ok = ok && gm(&bi, name, &[])?; }
        Ok(ok)
    })(), no);
    c.check("gate_mode：帶任何 modifier 一律 strict；沒有 profile、認不得的 recipe、值寫錯都是 strict", (|| Ok(
        gm(&bi, "docs", &["security"])? && gm(n, "docs", &[])? && gm(&bi, "nope", &[])?
            && gm(&json!({"recipes": {"docs": {"gate_mode": "none"}}}), "docs", &[])? && gm(&json!({"recipes": {"docs": {}}}), "docs", &[])?))(), no);
    let tonly = rz("test-only").unwrap_or(Value::Null);
    c.check("test-only 改產品程式 → 不符", (|| Ok(!diff_shortfalls(&tonly, &paths(&["src/a.py"]))?.is_empty() && diff_shortfalls(&tonly, &paths(&["tests/test_a.py"]))?.is_empty()))(), no);
    c.check("bugfix 碰 migration → 不符；加 migration modifier 相符", (|| Ok(
        !diff_shortfalls(&bug, &paths(&["db/migrations/001.sql"]))?.is_empty() && diff_shortfalls(&mig, &paths(&["db/migrations/001.sql"]))?.is_empty()))(), no);
    c.check("bugfix 改依賴宣告 → 不符；deps 相符", (|| Ok(
        !diff_shortfalls(&bug, &paths(&["package.json"]))?.is_empty() && diff_shortfalls(&rz("deps")?, &paths(&["package.json", "package-lock.json"]))?.is_empty()))(), no);
    c.check("spike 不能推送", (|| Ok(!diff_shortfalls(&rz("spike")?, &json!([]))?.is_empty()))(), no);
    let base_sections = strs(BASE_SECTIONS);
    c.check("intent 節：feature 四節、bugfix 另加根因", (|| Ok(
        Value::Array(required_sections(n, &s("feature"))?) == base_sections && has_str(&required_sections(n, &s("diagnose"))?, "根因")))(), no);
    let md = "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC1\n";
    let miss = missing_sections(md, &base_sections);
    c.check("intent 節：空節判缺", miss.clone().map(|m| Value::Array(m) == strs(&["不做的事（只有標題沒有內容）"])), || pyfmt::repr(&Value::Array(miss.clone().unwrap_or_default())));

    // 模型分級（I7）
    let with = |base: &Value, key: &str, v: Value| { let mut x = base.clone(); x[key] = v; x };
    let mut mp = bi.clone();
    mp["models"] = json!({"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"},
        "defaults": {"*": "standard", "tpm-researcher": "light"}, "floors": {"tpm-researcher@plan": "standard"}});
    let models_with = |key: &str, v: Value| with(&mp, "models", with(&mp["models"], key, v));
    let rm = |prof: &Value, role: &str, stage: &str, sug: Option<&str>, mods: &[&str]| resolve_model(prof, &s(role), &s(stage), &sug.map(s).unwrap_or(Value::Null), &strs(mods));
    c.check("模型：沒有 models 或 mode=inherit 全部沿用主對話模型", (|| Ok(
        rm(&bi, "tpm-researcher", "plan", None, &[])?["model"] == "" && rm(&bi, "x", "plan", None, &[])?["mode"] == "inherit"
            && rm(&models_with("mode", s("inherit")), "dev-merge-steward", "merge", None, &[])?["tier"] == ""))(), no);
    c.check("模型：AI 建議低於下限會被往上調", (|| Ok(
        rm(&mp, "tpm-researcher", "plan", Some("light"), &[])?["tier"] == "standard" && rm(&mp, "tpm-researcher", "design", Some("light"), &[])?["tier"] == "light"))(), no);
    c.check("模型：AI 可以建議更高", (|| Ok(rm(&mp, "tpm-researcher", "plan", Some("strong"), &[])?["model"] == "opus"))(), no);
    c.check("模型：閘門角色的內建下限 profile 蓋不掉", (|| Ok(
        rm(&models_with("floors", json!({"dev-merge-steward": "light"})), "dev-merge-steward", "merge", Some("light"), &[])?["tier"] == "strong"
            && rm(&mp, "tpm-skeptic", "merge", Some("light"), &[])?["tier"] == "strong" && rm(&mp, "dev-spec-author", "plan", Some("light"), &[])?["tier"] == "strong"))(), no);
    c.check("模型：mode=fixed 不採 AI 建議", (|| Ok(rm(&models_with("mode", s("fixed")), "tpm-writer", "deliver", Some("light"), &[])?["tier"] == "standard"))(), no);
    c.check("模型：不認得的等級當成沒建議", (|| Ok(rm(&mp, "tpm-writer", "deliver", Some("ultra"), &[])?["tier"] == "standard"))(), no);
    c.check("模型：modifier 的 model_floors 只加嚴", (|| Ok(
        rm(&mp, "tpm-skeptic", "plan", Some("light"), &["security"])?["tier"] == "strong" && rm(&mp, "tpm-skeptic", "design", Some("light"), &[])?["tier"] == "light"))(), no);
    c.check("模型：plan 的 skeptic 內建下限 standard", (|| Ok(rm(&mp, "tpm-skeptic", "plan", Some("light"), &[])?["tier"] == "standard"))(), no);
    let nomf = with(&mp, "modifiers", json!({"security": {"description": "舊 profile：沒有 model_floors"}}));
    c.check("模型：profile 的 modifier 拿掉 model_floors，內建的下限照樣生效", (|| Ok(rm(&nomf, "tpm-skeptic", "design", Some("light"), &["security"])?["tier"] == "strong"))(), no);
    c.check("模型：models 形狀不對不丟例外", (|| Ok(
        rm(&json!({"models": {"tiers": "x", "defaults": 3, "floors": []}}), "tpm-writer", "plan", None, &[])?["model"] == "sonnet"
            && rm(&json!({"models": {"mode": ""}}), "tpm-writer", "plan", None, &[])?["mode"] == "auto"))(), no);
    let lower = (|| -> R<Vec<Value>> {
        let mut lower = Vec::new();
        for role in ["tpm-researcher", "dev-merge-steward", "tpm-skeptic", "dev-spec-author", "zzz"] {
            for st in SDLC {
                for sg in [None, Some("light"), Some("standard"), Some("strong")] {
                    let got = rm(&mp, role, st, sg, &[])?;
                    if tier_rank(&got["tier"]) < tier_rank(&got["floor"]) { lower.push(json!([role, st, sg])); }
                }
            }
        }
        Ok(lower)
    })();
    c.check("模型：任何組合都不低於下限", lower.clone().map(|l| l.is_empty()), || pyfmt::repr(&Value::Array(lower.clone().unwrap_or_default().into_iter().take(3).collect())));
    c.check("模型：等級寫 inherit 表示沿用主對話模型", (|| Ok(
        tier_model(&json!({"models": {"tiers": {"strong": "inherit"}}}), &s("strong"))?.is_empty() && model_up(&s("light")) == "standard" && model_up(&s("strong")).is_empty()))(), no);

    let cases = golden_cases(teams);
    let count = cases.as_ref().map(|x| x.len()).unwrap_or(0);
    c.check("golden-set/recipes 有案例", cases.as_ref().map(|x| x.len() >= 8).map_err(Clone::clone), || count.to_string());
    let mut bad = 0;
    for (p, case) in cases.unwrap_or_default() {
        let fname = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let got = match run_case(&case) {
            Ok(g) => g,
            Err(e) => {
                bad += 1;
                c.check(&format!("golden {fname}"), Err(e), no);
                continue;
            }
        };
        let mut diff = Map::new();
        if let Some(want) = case.get("expect").and_then(Value::as_object) {
            for (k, v) in want {
                let g = got.get(k).unwrap_or(&NULL);
                if !pyfmt::eq(g, v) { diff.insert(k.clone(), json!([g, v])); }
            }
        }
        if !diff.is_empty() {
            bad += 1;
            c.check(&format!("golden {fname}"), Ok(false), || dumps(&Value::Object(diff), None).chars().take(400).collect());
        }
    }
    c.check(&format!("golden 案例全部相符（{count} 個）"), Ok(bad == 0), no);

    // X3 I6：這些路徑以前會被當成文件或測試而放行
    let sneak_docs = ["lib/docs/render.py", "src/doc/api.ts", "docs/conf.py", "CMakeLists.txt", ".claude/teams/rules.md", ".claude/agents/a.md", "CLAUDE.md"];
    let sneak_test = ["src/spec/handler.rb", "pkg/e2e/main.go", "app/test/server.go"];
    let sneak_bug = ["db/migrate/1.rb", "alembic/versions/a.py", "x.sql", "gradle/libs.versions.toml", "requirements/base.txt", "a.csproj",
        "Directory.Packages.props", "mix.lock", "pubspec.lock", "constraints.txt", ".github/workflows/ci.yml"];
    let leaked = (|| -> R<Vec<Value>> {
        let mut leaked = Vec::new();
        for (r, list) in [(&docs, &sneak_docs[..]), (&tonly, &sneak_test[..]), (&bug, &sneak_bug[..])] {
            for p in list {
                if diff_shortfalls(r, &paths(&[p]))?.is_empty() { leaked.push(s(p)); }
            }
        }
        Ok(leaked)
    })();
    c.check("diff 形狀：程式藏在 docs/、spec/ 目錄、.txt、agent／CI 設定、各語言的 migration 與依賴宣告都會被抓到",
        leaked.clone().map(|l| l.is_empty()), || pyfmt::repr(&Value::Array(leaked.clone().unwrap_or_default())));
    c.check("diff 形狀：正常的文件、測試照樣放行", (|| Ok(
        empty(diff_shortfalls(&docs, &paths(&["docs/a.md", "README.md", "docs/robots.txt"])))?
            && empty(diff_shortfalls(&tonly, &paths(&["tests/helpers.py", "src/a.test.ts", "e2e/tests/a.spec.ts"])))?))(), no);
    let ok = c.ok;
    out.push_str(&format!("recipe selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    if ok { 0 } else { 1 }
}
