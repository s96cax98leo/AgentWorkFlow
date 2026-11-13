//! golden set：schema 的正例、反例、語意反例。原本是 0.x 的 Python 工具 `run.py`。
//!
//! 範例在 `golden-set/valid/*.json` 與 `golden-set/invalid/*.json`（只看這兩個目錄的這一層；`recipes/`、
//! `token-budget.json` 是別的檢查的資料，和 run.py 一樣不看）。範例檔的鍵：
//!   - `payload`：信封＋該 payload；`schema`：獨立文件（例如 methodology），直接用那份 schema 驗。
//!   - `expect`（選填）：錯誤訊息要含這段文字（確認是被對的規則擋下）。
//!   - `config`（選填）：這一例用的測試專用設定（覆蓋 guard.config.json，例如 task_id_prefix）。
//!   - `semantic`（選填）：語意反例，schema 本身擋不住、由語意規則擋。
//!
//! 另外把 `methodologies/*.json`（內建 profile）與 `methodology.json`（有的話）當正例。
//!
//! 每個範例的判定（全部成立才算通過）：
//!   1. 用到的 schema 只用驗證器認得的關鍵字。
//!   2. 有 `expect` 時，錯誤訊息裡要有它。
//!   3. 正例零錯誤；反例要有錯誤。
//!   4. 不是語意反例的反例，光靠 schema（不算語意規則）就要擋得住。
//!
//! 和 0.x 的 Python 版不同的地方：0.x 的 Python 版另外拿官方的 `jsonschema` 套件對每個範例再驗一次，確認兩個驗證器對 schema 部分的
//! 判斷一致。這裡只有 awf-core 自己的驗證器（不為了檢查多帶一個相依）；第 4 條對應 0.x 的 Python 版「官方 jsonschema 應該擋
//! 但通過」那一項。兩個驗證器的逐例對照留在 `tests/parity/schema.expected.json`（awf-core 的 schema_parity 測試）。
//! 範例檔讀不懂、缺 `data`、既沒有 `payload` 也沒有 `schema`：0.x 的 Python 版當掉，這裡算那一例失敗。
//!
//! 用法：`cargo xtask golden [--verbose] [--root <目錄>]`（`--verbose` 每個範例印一行判定）。

use crate::util::{self, Captured};
use awf_core::schema::{self, Schemas};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 一個範例的結果。
pub struct CaseResult {
    /// `valid` 或 `invalid`。
    pub kind: &'static str,
    pub name: String,
    /// 完整檢查（schema＋語意規則）的錯誤數。
    pub errors: usize,
    /// 只算 schema 的錯誤數。
    pub schema_errors: usize,
    /// 沒通過的理由（空的＝通過）。
    pub problems: Vec<String>,
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Python 的 list repr（`['a', 'b']`）。
fn list_repr(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| awf_core::pyfmt::repr_str(s)).collect::<Vec<_>>().join(", "))
}

fn run_case(teams: &Path, kind: &'static str, name: String, case: &Value) -> CaseResult {
    let mut res = CaseResult { kind, name, errors: 0, schema_errors: 0, problems: Vec::new() };
    let mut schemas = Schemas::new(teams);
    schemas.config_override = case.get("config").and_then(Value::as_object).cloned();
    let Some(data) = case.get("data") else {
        res.problems.push("範例沒有 data".into());
        return res;
    };
    let doc = case.get("schema").and_then(Value::as_str);
    // `"payload": null` 是「只驗信封、payload 不限形狀」；連鍵都沒有才算範例寫錯
    let (schema, mine) = match (doc, case.get("payload")) {
        (Some(d), _) => (schemas.doc_schema(d), schemas.check_doc(data, d)),
        (None, Some(p)) if p.is_null() || p.is_string() => (schemas.compose(p.as_str()), schemas.check(data, p.as_str())),
        (None, Some(_)) => {
            res.problems.push("範例的 payload 要是字串或 null".into());
            return res;
        }
        (None, None) => {
            res.problems.push("範例既沒有 payload 也沒有 schema".into());
            return res;
        }
    };
    let (schema, mine) = match (schema, mine) {
        (Ok(s), Ok(m)) => (s, m),
        (s, m) => {
            res.problems.extend(s.err());
            res.problems.extend(m.err());
            res.problems.dedup();
            return res;
        }
    };
    let schema_only = schema::validate(data, &schema, "$");
    res.errors = mine.len();
    res.schema_errors = schema_only.len();
    for u in schema::unsupported_keywords(&schema, "$") {
        res.problems.push(format!("schema 用了驗證器不認得的關鍵字：{u}"));
    }
    if let Some(expect) = case.get("expect").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        if !mine.iter().any(|e| e.contains(expect)) {
            res.problems.push(format!("錯誤訊息沒有「{expect}」：{}", list_repr(&mine)));
        }
    }
    let want_ok = kind == "valid";
    if mine.is_empty() != want_ok {
        res.problems.push(format!("驗證器判 {}：{}", if mine.is_empty() { "通過" } else { "不通過" }, list_repr(&mine)));
    }
    let semantic = case.get("semantic").is_some_and(awf_core::pyfmt::truthy);
    if kind == "invalid" && !semantic && schema_only.is_empty() {
        res.problems.push("不是語意反例，schema 應該擋但通過（語意反例要標 semantic）".into());
    }
    res
}

/// 跑全部範例：回每一例的結果，順序同 run.py（valid、invalid、內建 profile、專案 profile）。
pub fn run_all(root: &Path) -> Vec<CaseResult> {
    let teams = util::teams_dir(root);
    let here = teams.join("golden-set");
    let mut out = Vec::new();
    let mut one = |kind: &'static str, path: &Path, wrap: bool| {
        let name = file_name(path);
        let case = util::read_json(path).map(|v| if wrap { serde_json::json!({"schema": "methodology", "data": v}) } else { v });
        out.push(match case {
            Ok(c) => run_case(&teams, kind, name, &c),
            Err(e) => CaseResult { kind, name, errors: 0, schema_errors: 0, problems: vec![e] },
        });
    };
    for kind in ["valid", "invalid"] {
        for f in json_files(&here.join(kind)) {
            one(kind, &f, false);
        }
    }
    for f in json_files(&teams.join("methodologies")) {
        one("valid", &f, true);
    }
    let own = teams.join("methodology.json");
    if own.is_file() {
        one("valid", &own, true);
    }
    out
}

/// 指令本體：`root` 是 repo 根目錄，`args` 是 `--verbose`。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let results = run_all(root);
    let mut out = String::new();
    let (mut ok, mut bad) = (0, 0);
    let count = |kind: &str| results.iter().filter(|r| r.kind == kind).count();
    for r in &results {
        if r.problems.is_empty() {
            ok += 1;
            if verbose {
                out.push_str(&format!("ok   {}/{}（錯誤 {}，其中 schema {}）\n", r.kind, r.name, r.errors, r.schema_errors));
            }
        } else {
            bad += 1;
            out.push_str(&format!("FAIL {}/{}：{}\n", r.kind, r.name, r.problems.join("；")));
        }
    }
    if results.is_empty() {
        out.push_str("golden set：沒有任何範例（golden-set/valid、invalid 是空的？）\n");
        return (1, out, String::new());
    }
    out.push_str(&format!("golden set：通過 {ok}，失敗 {bad}（正例 {}、反例 {}）\n", count("valid"), count("invalid")));
    (if bad == 0 { 0 } else { 1 }, out, String::new())
}
