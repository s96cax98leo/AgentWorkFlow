//! `awf golden`：跑專案的 golden-set（交件信封與各種 payload 的正例、反例、語意反例）。
//! 取代原本專案裡的 `golden-set/run.py`（要另外裝 jsonschema 套件）；改了 schema、補了範例之後跑一次。
//!
//! 範例在 `<golden-set>/valid/*.json` 與 `<golden-set>/invalid/*.json`（只看這兩個目錄的這一層；`recipes/`、
//! `token-budget.json` 是別的檢查的資料，不看）。schema 取自 golden-set 的上一層（`.claude/teams/schemas/`）。範例檔的鍵：
//!   - `payload`：信封＋該 payload（`null`＝只驗信封）；`schema`：獨立文件（例如 methodology），直接用那份 schema 驗。
//!   - `data`：要驗的內容。
//!   - `expect`（選填）：錯誤訊息要含這段文字（確認是被對的規則擋下）。
//!   - `config`（選填）：這一例用的測試專用設定（覆蓋 guard.config.json，例如 task_id_prefix）。
//!   - `semantic`（選填）：語意反例，schema 本身擋不住、由語意規則擋。
//!
//! 另外把上一層的 `methodologies/*.json`（內建 profile）與 `methodology.json`（專案自己的，有的話）當正例。
//!
//! 每個範例的判定（全部成立才算通過）：
//!   1. 用到的 schema 只用驗證器認得的關鍵字。
//!   2. 有 `expect` 時，錯誤訊息裡要有它。
//!   3. 正例零錯誤；反例要有錯誤。
//!   4. 不是語意反例的反例，光靠 schema（不算語意規則）就要擋得住；語意反例（標了 `semantic`）由含語意規則的完整檢查擋下。
//!
//! 和 `run.py` 不同的地方：`run.py` 另外拿官方的 jsonschema 套件對每個範例再驗一次；這裡只有 awf 自己的驗證器，
//! 第 4 條對應 `run.py`「官方 jsonschema 應該擋但通過」那一項。範例檔讀不懂、缺 `data`、既沒有 `payload` 也沒有 `schema`：
//! `run.py` 當掉，這裡算那一例失敗。
//!
//! `cargo xtask golden`（模版 repo 自己用的那一支）呼叫這裡的 [`run_all`]，判定只有這一份；兩支只差在目錄怎麼決定與總結那一行的寫法。

use crate::pyfmt::{repr_str, truthy};
use crate::schema::{self, Schemas};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "跑專案的 golden-set：交件信封與各種 payload 的正例（valid/）、反例（invalid/）、語意反例（反例裡標 semantic 的）。
改了 schema 或補了範例之後跑一次。只讀檔，不寫任何東西。

用法：
    awf golden [--dir <golden-set 目錄>] [--verbose]

  --dir <目錄>   golden-set 目錄；預設是專案的 .claude/teams/golden-set。schema 取自它上一層的 schemas/。
  --verbose      每個範例印一行判定（預設只印失敗的）
全部通過 exit 0；有失敗、沒有任何範例、目錄不存在 exit 1；參數錯誤 exit 2。
";

/// （exit code，stdout，stderr）。
pub type Captured = (i32, String, String);

/// 一個範例的結果。
pub struct CaseResult {
    /// `valid` 或 `invalid`。
    pub kind: &'static str,
    pub name: String,
    /// 語意反例（範例標了 `semantic`）。
    pub semantic: bool,
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

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到檔案：{e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "不是 UTF-8 文字".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("不是合法 JSON：{e}"))
}

/// Python 的 list repr（`['a', 'b']`）。
fn list_repr(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| repr_str(s)).collect::<Vec<_>>().join(", "))
}

fn run_case(teams: &Path, kind: &'static str, name: String, case: &Value) -> CaseResult {
    let semantic = case.get("semantic").is_some_and(truthy);
    let mut res = CaseResult { kind, name, semantic, errors: 0, schema_errors: 0, problems: Vec::new() };
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
    if kind == "invalid" && !semantic && schema_only.is_empty() {
        res.problems.push("不是語意反例，schema 應該擋但通過（語意反例要標 semantic）".into());
    }
    res
}

/// 跑全部範例：`here` 是 golden-set 目錄。順序同 run.py（valid、invalid、內建 profile、專案 profile）。
pub fn run_all(here: &Path) -> Vec<CaseResult> {
    let teams = here.parent().map(Path::to_path_buf).unwrap_or_else(|| here.join(".."));
    let mut out = Vec::new();
    let mut one = |kind: &'static str, path: &Path, wrap: bool| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let case = read_json(path).map(|v| if wrap { serde_json::json!({"schema": "methodology", "data": v}) } else { v });
        out.push(match case {
            Ok(c) => run_case(&teams, kind, name, &c),
            Err(e) => CaseResult { kind, name, semantic: false, errors: 0, schema_errors: 0, problems: vec![e] },
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

/// 指令本體：`default_dir` 是沒給 `--dir` 時的 golden-set 目錄；相對的 `--dir` 以 `cwd` 為準。
pub fn run_captured(argv: &[String], default_dir: &Path, cwd: &Path) -> Captured {
    let (mut dir, mut verbose) = (None, false);
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return (0, USAGE.to_string(), String::new()),
            "--verbose" | "-v" => verbose = true,
            "--dir" => match it.next() {
                Some(d) => dir = Some(cwd.join(d)),
                None => return (2, String::new(), "awf golden：--dir 後面要接 golden-set 目錄\n".into()),
            },
            other => match other.strip_prefix("--dir=") {
                Some(d) => dir = Some(cwd.join(d)),
                None => return (2, String::new(), format!("awf golden：不認得的參數「{other}」\n\n{USAGE}")),
            },
        }
    }
    let here = dir.unwrap_or_else(|| default_dir.to_path_buf());
    if !here.is_dir() {
        return (1, String::new(), format!(
            "awf golden：找不到 golden-set 目錄 {}（專案還沒套用模版，或不在專案根目錄？用 --dir <目錄> 指定）\n",
            here.display()
        ));
    }
    let results = run_all(&here);
    let mut out = String::new();
    for r in &results {
        if r.problems.is_empty() {
            if verbose {
                out.push_str(&format!("ok   {}/{}（錯誤 {}，其中 schema {}）\n", r.kind, r.name, r.errors, r.schema_errors));
            }
        } else {
            out.push_str(&format!("FAIL {}/{}：{}\n", r.kind, r.name, r.problems.join("；")));
        }
    }
    if results.is_empty() {
        out.push_str(&format!("golden set：沒有任何範例（{} 的 valid/、invalid/ 是空的？）\n", here.display()));
        return (1, out, String::new());
    }
    // 每一類：通過數／總數
    let tally = |pick: &dyn Fn(&CaseResult) -> bool| {
        let all: Vec<&CaseResult> = results.iter().filter(|r| pick(r)).collect();
        format!("{}／{}", all.iter().filter(|r| r.problems.is_empty()).count(), all.len())
    };
    let ok = results.iter().filter(|r| r.problems.is_empty()).count();
    let bad = results.len() - ok;
    out.push_str(&format!(
        "golden set：通過 {ok}，失敗 {bad}（正例 {}、反例 {}，其中語意反例 {}）\n",
        tally(&|r| r.kind == "valid"), tally(&|r| r.kind == "invalid"), tally(&|r| r.kind == "invalid" && r.semantic)
    ));
    (if bad == 0 { 0 } else { 1 }, out, String::new())
}

/// `awf golden …`
pub fn main(argv: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let default_dir = crate::check_envelope::default_teams().join("golden-set");
    let (code, out, err) = run_captured(argv, &default_dir, &cwd);
    print!("{out}");
    eprint!("{err}");
    code
}
