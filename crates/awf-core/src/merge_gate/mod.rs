//! 確定性 merge gate：機械複核合併 agent（dev-merge-steward）的需求對照表，並檢查其他合併條件。
//! 原本是 0.x 的 Python 工具 `merge_gate.py`；判定、exit code、輸出文字照 0.x 的 Python 版，只會一樣或更嚴。
//!
//! agent 可能被 PR 內容說服；這支程式只看結構化輸入，不讀 PR 說明，說服不了。
//! 結束碼：0＝merge；1＝hold；3＝escalate；2＝參數錯誤。輸入缺漏一律判 hold（fail-closed）。
//!
//! 和 0.x 的 Python 版的差別（都不會讓判定變鬆）：
//! - 0.x 的 Python 版遇到形狀不對的設定值會丟沒接住的例外（traceback、exit 1）；這裡印一行訊息、exit 1（`Fail::Crash`）。
//! - JSON 解析見 `pyjson`（NaN、巢狀過深當成讀不到）；整數只收 ASCII 數字。
//! - hunk 標頭只收 ASCII 數字，行數超過 64 位元當成解析不了（hold）。
//!
//! 私有的重複實作：`recipe_tamper_errors`、`load_profile_for`（對應 `save_run::recipe_tamper_errors`、`save_run::load_profile`）。
//! 判定規則相同，但兩份不是逐案例相同，所以沒有併成一份：
//! - 讀 profile 的 JSON：save_run 多擋「落單的代理字元跳脫」與「超出 64 位元的整數」（`save_run::py::loads`），這裡用 `pyjson::loads` 不擋。
//! - 形狀不對的 run.json（缺 `resolved`、`build_kinds` 的項目不是字串…）：save_run 照 Python 的例外類別回錯，這裡是 `Crash`，訊息文字不同
//!   （兩邊都不放行）。
//! - 相對路徑：save_run 以 `Ctx::cwd` 為基準，這裡的路徑在呼叫前已經接上專案目錄。
//! 要併的話以 save_run 那份為準（只會更嚴），並逐項更新 `tests/parity/gates.expected.json` 受影響的案例。

pub mod pyargs;
pub mod pyjson;
pub mod pyutil;
mod selftest;

use crate::hooks::re::Re;
use crate::pyfmt::{self, repr_str, str_of, truthy};
use crate::recipe;
use crate::schema::{self, Schemas};
use fancy_regex::Regex as FRegex;
use pyargs::{opt, req, ArgErr, Kind, Opt, Parsed};
use pyjson::JsonErr;
use pyutil::{head, strip, ReadErr};
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

pub use selftest::selftest;

pub const GATE_VERSION: &str = "1";
pub const TIERS: &[&str] = &["low", "medium", "high"];
const DEFAULT_DAILY_CAP: i128 = 5;
const DEFAULT_SMALL_LINES: i128 = 100; // low 等級的改動行數上限
const DEFAULT_LARGE_LINES: i128 = 400; // 超過就是 high
const DEFAULT_LARGE_FILES: i128 = 20;
const PASS_CHECK: &[&str] = &["success", "pass"];
// skipped／neutral 預設不算通過（required check 被跳過＝沒驗到）；只有列在 auto_merge.allowed_skipped_checks 的 check 名稱才算
const SKIP_CHECK: &[&str] = &["skipped", "skipping", "skip", "neutral"];
const ADAPTERS: &[&str] = &["json", "command", "spec-md", "openspec"];

pub const USAGE: &str = "確定性 merge gate（只有 merge 時 exit 0）

用法：
  awf merge-gate check \\
      --decision <steward 信封或 payload 的 JSON 檔，- 代表 stdin> \\
      --repo acme-fork/app --pr 3 --head-sha <目前 PR head SHA> \\
      --evidence <正規化證據 JSON；也接受 verifier 的 criteria 信封> \\
      --checks <required checks JSON：gh pr checks --json name,state 的輸出，或 {\"名稱\": \"success\"}> \\
      --diff <git diff base...head 的輸出檔> \\
      --tests-before N --tests-after N \\
      [--findings <審查發現 JSON> ...] [--reviewed-sha SHA ...] [--base-sha SHA] [--git-dir <repo>] \\
      [--requirements <路徑>] [--requirements-source json|command|spec-md|openspec] [--requirements-change <變更名>] \\
      [--timestamp <ISO 8601>] [--project-dir <專案根>] [--runs-dir <目錄>] [--no-audit] \\
      [--merged-today N] [--frozen <理由>] [--recipe <run.json>]
  awf merge-gate --selftest

輸出：stdout 一個 JSON（decision、reasons、escalations、followups、risk_tier…）。
結束碼：0＝merge；1＝hold；3＝escalate；2＝參數錯誤。輸入缺漏一律判 hold（fail-closed）。
";

// 動到就交給人（escalate）的路徑：CI、.github、hooks、gate、release、版本檔、依賴與鎖檔、migration、auth、CODEOWNERS
const SENSITIVE: &[(&str, &str)] = &[
    (r"(^|/)\.github/", "GitHub 設定或 CI"),
    (r"(^|/)(\.gitlab-ci\.yml|Jenkinsfile|azure-pipelines\.yml|bitbucket-pipelines\.yml)$|(^|/)\.circleci/|(^|/)\.buildkite/", "CI 設定"),
    (r"(^|/)\.claude/|(^|/)\.claude-plugin/", "agent 設定、守門或 gate"),
    (r"(^|/)(\.husky|\.githooks)/|^(plugin/)?hooks/|(^|/)\.pre-commit-config\.yaml$", "hooks"),
    (r"merge_gate|(^|/)guard\.config\.json", "merge gate"),
    (r"(^|/)release[^/]*(/|$)|(^|/)\.release|(^|/)(\.goreleaser\.ya?ml|\.releaserc[^/]*)$", "release"),
    (r"(^|/)(VERSION|version\.txt|_version\.py|version\.py)$", "版本檔"),
    (r"(^|/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|npm-shrinkwrap\.json|bun\.lockb?|poetry\.lock|uv\.lock|Pipfile\.lock|Cargo\.lock|go\.sum|Gemfile\.lock|composer\.lock|packages\.lock\.json|[^/]*\.lock)$", "依賴鎖檔"),
    (r"(^|/)(package\.json|pyproject\.toml|setup\.py|setup\.cfg|requirements[^/]*\.txt|Pipfile|go\.mod|Cargo\.toml|Gemfile|composer\.json|pom\.xml|build\.gradle(\.kts)?)$", "依賴或版本宣告"),
    (r"(^|/)migrations?/", "資料庫 migration"),
    (r"(^|/)CODEOWNERS$", "CODEOWNERS"),
    (r"(^|/)(auth|oauth|authn|authz)(/|\.[a-z]+$)|(^|/)[^/]*[_-](auth|oauth)\.[a-z]+$", "auth"),
    // 放寬閘門：改這些設定可能放寬測試、lint、型別或覆蓋率門檻，要人會簽
    (r"(^|/)(jest|vitest|playwright|cypress)\.config\.[a-z]+$|(^|/)karma\.conf\.[a-z]+$|(^|/)(pytest\.ini|tox\.ini|\.coveragerc|codecov\.ya?ml|mypy\.ini|\.flake8|\.?ruff\.toml|\.rubocop\.yml)$|(^|/)\.nycrc[^/]*$|(^|/)\.eslintrc[^/]*$|(^|/)eslint\.config\.[a-z]+$|(^|/)tsconfig[^/]*\.json$|(^|/)\.golangci\.ya?ml$|(^|/)phpunit\.xml[^/]*$|(^|/)\.stylelintrc[^/]*$",
     "測試／lint／型別／覆蓋率設定"),
];
const DOC: &str = r"(?i)(^|/)docs?/|\.(md|mdx|rst|txt|adoc)$";
const TEST: &str = r"(?i)(^|/)(tests?|__tests__|spec|e2e)/|(^|/)test_[^/]+$|_test\.[a-z]+$|\.(test|spec)\.[a-z]+$";
// 新增的 skip／only／fixme（含常見框架寫法）
const SKIP: &str = concat!(
    r"\b(it|test|describe|context|suite)\.(skip|only|fixme|todo)\b|\b(xit|xdescribe|fit|fdescribe|xtest)\s*\(",
    r"|@pytest\.mark\.(skip|skipif|xfail)\b|\bpytest\.(skip|xfail)\s*\(|@unittest\.(skip|skipIf|skipUnless|expectedFailure)\b",
    r"|\bt\.Skip(Now|f)?\s*\(|@(Disabled|Ignore)\b|\btest\.fixme\b"
);
// 新增的壓制註解（lint、型別、覆蓋率）：放寬檢查要人會簽
const SUPPRESS: &str = concat!(
    r"eslint-disable|@ts-(ignore|nocheck|expect-error)\b|#\s*type:\s*ignore|#\s*noqa\b|pragma:\s*no\s*cover",
    r"|(istanbul|c8)\s+ignore|//\s*nolint\b|\bNOLINT\b|@SuppressWarnings\b|rubocop:disable"
);
// 內建範例 adapter（spec-md）的預設 ID 樣式：行首（可帶清單符號、表格線、粗體）的 AC-1／AC1／REQ-1 之類
const DEFAULT_ID_PATTERN: &str = r"^\s*(?:[-*+]\s+|\d+[.)]\s+|\|\s*|#+\s+)?(?:\*\*|`)?\[?([A-Z][A-Z0-9]*-?\d+)\]?(?:\*\*|`)?(?=[\s:：|.)、]|$)";

// ------------------------------------------------------------------ 正規式（Python re → Rust 的差異在這裡補）

/// Python 的 `$` 也配「結尾換行之前」；Rust 的不會。固定樣式裡的 `$` 都換成這個。
fn py_dollar(pat: &str) -> String {
    pat.replace('$', r"(?:\n?\z)")
}

/// Python 的 `\b` 以它自己的 `\w`（字母、各種數字、底線）為準，`\s` 多了 U+001C–U+001F；
/// 照這個定義展開，避免 Rust 的字集較寬而漏配（漏配＝少擋）。
fn py_word_space(pat: &str) -> String {
    const W: &str = r"[\p{L}\p{N}_]";
    let boundary = format!("(?:(?<={W})(?!{W})|(?<!{W})(?={W}))");
    pat.replace(r"\b", &boundary).replace(r"\s", r"[\s\x1c-\x1f]")
}

fn sensitive() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| SENSITIVE.iter().map(|(p, what)| (Regex::new(&py_dollar(p)).expect("固定的正規式"), *what)).collect())
}

fn doc_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&py_dollar(DOC)).expect("固定的正規式"))
}

fn test_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&py_dollar(TEST)).expect("固定的正規式"))
}

fn skip_re() -> &'static FRegex {
    static R: OnceLock<FRegex> = OnceLock::new();
    R.get_or_init(|| FRegex::new(&py_word_space(SKIP)).expect("固定的正規式"))
}

fn suppress_re() -> &'static FRegex {
    static R: OnceLock<FRegex> = OnceLock::new();
    R.get_or_init(|| FRegex::new(&py_word_space(SUPPRESS)).expect("固定的正規式"))
}

fn placeholder_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\{\{[A-Z_]+\}\}").expect("固定的正規式"))
}

fn hunk_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^@@ -[0-9]+(?:,([0-9]+))? \+[0-9]+(?:,([0-9]+))? @@").expect("固定的正規式"))
}

/// 配不配得到；正規式引擎自己出錯（回溯上限）時當成配到（擋下來）。
fn hit(re: &FRegex, text: &str) -> bool {
    re.is_match(text).unwrap_or(true)
}

/// 路徑的分類（對跑用）：命中的高風險路徑說明、是不是文件、是不是測試。
pub fn path_kind(path: &str) -> (Option<&'static str>, bool, bool) {
    (sensitive().iter().find(|(re, _)| re.is_match(path)).map(|(_, what)| *what), doc_re().is_match(path), test_re().is_match(path))
}

/// 新增行的分類（對跑用）：是不是 skip／only 之類、是不是壓制註解。
pub fn line_kind(line: &str) -> (bool, bool) {
    (hit(skip_re(), line), hit(suppress_re(), line))
}

// ------------------------------------------------------------------ 執行環境與錯誤

/// 執行環境：測試可以換掉 teams 目錄、環境變數與 stdin。
pub struct Ctx {
    /// schema 所在的 `.claude/teams`。`None`（實際執行）＝跟著專案根：`<專案>/.claude/teams`，所以 schema、guard.config.json、
    /// methodology.json 一定來自同一個專案，和在哪個目錄執行無關。`Some`（測試、selftest）＝固定用這一份；
    /// 這時沒給專案根就用它往上兩層（0.x 的 Python 版「工具檔所在的專案」）。
    pub teams: Option<PathBuf>,
    /// 沒給 `--project-dir` 時的專案根：實際執行時是 [`crate::project::find`] 的結果
    /// （`CLAUDE_PROJECT_DIR`，沒有就從目前目錄往上找最外層的 `.claude/teams/VERSION`）。
    pub project_dir_env: Option<String>,
    /// 另外加給子程序（git、需求清單指令）的環境變數。
    pub env: Vec<(String, String)>,
    /// `--decision -` 之類讀的 stdin；None 時讀真的 stdin。
    pub stdin: Option<String>,
}

impl Ctx {
    /// 實際執行時的環境：專案根照全部子指令共用的解析（[`crate::project`]），schema 跟著專案根。
    /// 找不到專案根時 `project_dir_env` 是 None：沒帶 `--project-dir` 就報錯，不退回目前目錄。
    pub fn real() -> Self {
        let found = crate::project::find(None);
        if let Some(n) = found.as_ref().and_then(|f| f.note.as_ref()) {
            eprintln!("awf merge-gate：{n}");
        }
        Ctx { teams: None, project_dir_env: found.map(|f| f.root), env: Vec::new(), stdin: None }
    }

    /// 這次判定用的 `.claude/teams`（schema 在它底下）。
    fn teams_of(&self, proj: &str) -> PathBuf {
        self.teams.clone().unwrap_or_else(|| PathBuf::from(pyutil::join(proj, ".claude/teams")))
    }
}

/// 出錯的兩種：`Input` 對應 0.x 的 Python 版的 InputError（判 hold 並列進理由）；
/// `Crash` 是 0.x 的 Python 版會丟出沒接住的例外的地方（這裡印訊息、exit 1，不輸出判定）。
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    Input(String),
    Crash(String),
}

type Crash = String;

fn crash(what: &str) -> Crash {
    format!("{what}（0.x 的 Python 版在這裡丟例外結束；一律不放行）")
}

static NULL: Value = Value::Null;

/// `v.get(k)`：v 不是物件或沒有這個鍵都回 null。
fn g<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object().and_then(|o| o.get(k)).unwrap_or(&NULL)
}

/// `str(v or "")`。
fn str_or_empty(v: &Value) -> String {
    if truthy(v) { str_of(v) } else { String::new() }
}

fn repr_strs(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| repr_str(s)).collect::<Vec<_>>().join(", "))
}

fn first<T>(v: &[T], n: usize) -> &[T] {
    &v[..v.len().min(n)]
}

/// 能不能當 dict 的鍵或放進 set。
fn hashable(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

/// `for x in (v or [])`：字串逐字、物件逐鍵；其他真值（數字、true）在 Python 是 TypeError。
fn py_iter(v: &Value) -> Result<Vec<Value>, Crash> {
    if !truthy(v) { return Ok(Vec::new()); }
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(crash(&format!("TypeError：{} 不能逐項讀", pyfmt::repr(other)))),
    }
}

enum IntErr {
    /// TypeError 或 ValueError（daily_cap 那裡有接）。
    Bad,
}

/// `int(v)`。
fn py_int(v: &Value) -> Result<i128, IntErr> {
    match v {
        Value::Bool(b) => Ok(*b as i128),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { return Ok(i as i128); }
            if let Some(u) = n.as_u64() { return Ok(u as i128); }
            let f = n.as_f64().ok_or(IntErr::Bad)?.trunc();
            if f.abs() < 1e38 { Ok(f as i128) } else { Err(IntErr::Bad) }
        }
        Value::String(s) => pyutil::int_of_str(s).ok_or(IntErr::Bad),
        _ => Err(IntErr::Bad),
    }
}

/// `int(v or 預設)`；轉不了在 Python 是沒接住的例外。
fn int_or(v: &Value, default: i128, what: &str) -> Result<i128, Crash> {
    if !truthy(v) { return Ok(default); }
    py_int(v).map_err(|_| crash(&format!("auto_merge.{what} 不是整數（{}）", pyfmt::repr(v))))
}

// ------------------------------------------------------------------ 設定與路徑

/// 專案根：`--project-dir` → 環境給的（見 [`Ctx::project_dir_env`]）→ 固定的 teams 往上兩層（只有測試與 selftest）。
/// 都沒有就是錯誤：不拿目前目錄當專案（目前目錄可以是 PR 的 checkout，自帶一份 `.claude/teams`）。
fn project_dir(ctx: &Ctx, arg: Option<&str>) -> Result<String, Crash> {
    if let Some(a) = arg.filter(|a| !a.is_empty()) { return Ok(pyutil::resolve(a)); }
    if let Some(e) = ctx.project_dir_env.as_deref().filter(|e| !e.is_empty()) { return Ok(pyutil::resolve(e)); }
    match &ctx.teams {
        Some(t) => Ok(pyutil::parent(&pyutil::parent(&pyutil::resolve(&t.to_string_lossy())))),
        None => Err(crate::project::not_found_message()),
    }
}

fn load_config(ctx: &Ctx, proj: &str) -> Value {
    let p = pyutil::join(proj, ".claude/teams/guard.config.json");
    let sch = Schemas::new(ctx.teams_of(proj));
    let path = PathBuf::from(&p);
    Value::Object(sch.config(if path.exists() { Some(path.as_path()) } else { None }))
}

fn auto_merge(cfg: &Value) -> &Value {
    let am = g(cfg, "auto_merge");
    if am.is_object() { am } else { &NULL }
}

/// 和 hook（awf_config）同一個來源：guard.config.json 的 runs_dir，預設 _tpm/runs，相對專案根。
fn runs_dir(proj: &str, cfg: &Value, over: Option<&str>) -> Result<String, Crash> {
    let d = match over.filter(|o| !o.is_empty()) {
        Some(o) => o.to_string(),
        None => match g(cfg, "runs_dir") {
            v if !truthy(v) => "_tpm/runs".to_string(),
            Value::String(s) => s.clone(),
            other => return Err(crash(&format!("AttributeError：runs_dir 不是字串（{}）", pyfmt::repr(other)))),
        },
    };
    let d = strip(&d);
    Ok(if pyutil::is_absolute(d) { pyutil::path_str(d) } else { pyutil::join(proj, d.trim_matches('/')) })
}

/// 凍結旗標：auto_merge.freeze_file，預設 <runs_dir>/auto-merge.freeze.json（和 hook 同一個預設）。
fn freeze_file(proj: &str, cfg: &Value) -> Result<String, Crash> {
    let raw = str_or_empty(g(auto_merge(cfg), "freeze_file"));
    let raw = strip(&raw);
    if raw.is_empty() || placeholder_re().is_match(raw) {
        return Ok(pyutil::join(&runs_dir(proj, cfg, None)?, "auto-merge.freeze.json"));
    }
    Ok(if pyutil::is_absolute(raw) { pyutil::path_str(raw) } else { pyutil::join(proj, raw.trim_matches('/')) })
}

fn list_of(v: &Value) -> Result<Vec<String>, Crash> {
    if let Value::String(s) = v {
        return Ok(s.split(',').map(strip).filter(|x| !x.is_empty()).map(str::to_string).collect());
    }
    Ok(py_iter(v)?.into_iter().filter_map(|x| match x {
        Value::String(s) if !strip(&s).is_empty() => Some(s),
        _ => None,
    }).collect())
}

/// 判定日（YYYY-MM-DD）：max(--timestamp 的日期, 系統的 UTC 日期)。
/// --timestamp 只能把「今天」往後推（更嚴），不能用舊日期規避每日上限或讓過期的上游授權復活；讀不懂時用系統日期。
pub fn judgment_day(timestamp: &str) -> String {
    let today = pyutil::today_utc();
    let given = pyutil::parse_iso_date(head(timestamp, 10)).unwrap_or(today);
    pyutil::iso(given.max(today))
}

/// 上游授權人（和 hook 的 guard.upstream_auth 同語意）：回傳（授權人或 None、沒有時的理由）。
/// 值可以是字串，或 {"by", "link", "scope", "expires": "YYYY-MM-DD"}；過期、日期讀不懂、by 空白都算沒有。
pub fn upstream_auth(am: &Value, repo: &str, today: &str) -> (Option<String>, String) {
    let mut v = g(g(am, "upstream_authorized_by"), repo);
    if v.is_object() {
        let exp = str_or_empty(g(v, "expires"));
        let exp = strip(&exp);
        if !exp.is_empty() {
            let day = pyutil::parse_iso_date(head(today, 10)).unwrap_or_else(pyutil::today_utc);
            match pyutil::parse_iso_date(head(exp, 10)) {
                Some(d) if d < day => return (None, format!("授權過期（expires {}，判定日 {}）", head(exp, 10), pyutil::iso(day))),
                Some(_) => {}
                None => return (None, format!("授權的 expires 讀不懂（{exp}），當成過期")),
            }
        }
        v = g(v, "by");
    }
    let who = str_or_empty(v);
    let who = strip(&who);
    if who.is_empty() { (None, "沒有授權人".into()) } else { (Some(who.to_string()), String::new()) }
}

// ------------------------------------------------------------------ 需求來源 adapter

#[derive(Debug, Clone, PartialEq)]
pub struct Req {
    pub id: String,
    pub text: String,
    pub source: String,
}

fn read_fail(e: ReadErr, input: impl FnOnce(&str) -> String) -> Fail {
    match e {
        ReadErr::Os(m) => Fail::Input(input(&m)),
        ReadErr::Decode(m) => Fail::Crash(crash(&format!("UnicodeDecodeError：{m}"))),
    }
}

fn normalize_requirements(data: &Value, source: &str) -> Result<Vec<Req>, Fail> {
    let items = if data.is_object() { g(data, "requirements") } else { data };
    let Value::Array(items) = items else {
        return Err(Fail::Input(format!("需求清單格式不對（{source}）：要是陣列，或帶 requirements 陣列的物件")));
    };
    let mut out: Vec<Req> = Vec::new();
    for (i, r) in items.iter().enumerate() {
        let id = str_or_empty(g(r, "id"));
        let rid = strip(&id);
        if !r.is_object() || rid.is_empty() {
            return Err(Fail::Input(format!("需求清單第 {i} 項缺 id（{source}）")));
        }
        if out.iter().any(|x| x.id == rid) {
            return Err(Fail::Input(format!("需求清單有重複的 id {rid}（{source}）")));
        }
        let src = g(r, "source");
        out.push(Req { id: rid.to_string(), text: str_or_empty(g(r, "text")), source: if truthy(src) { str_of(src) } else { source.to_string() } });
    }
    Ok(out)
}

fn adapter_json(path: &str) -> Result<Vec<Req>, Fail> {
    let text = pyutil::read_text(path).map_err(|e| read_fail(e, |m| format!("讀不到需求清單 {path}：{m}")))?;
    let data = pyjson::loads(&text).map_err(|e| Fail::Input(format!("讀不到需求清單 {path}：{e}")))?;
    normalize_requirements(&data, &format!("json:{}", pyutil::name(path)))
}

fn adapter_command(ctx: &Ctx, command: &Value, proj: &str, env_extra: &[(&str, String)]) -> Result<Vec<Req>, Fail> {
    if !truthy(command) { return Err(Fail::Input("requirements.source=command 但沒有設定 command".into())); }
    let (mut cmd, shown) = match command {
        Value::String(s) => {
            let mut c = if cfg!(windows) { let mut c = Command::new("cmd"); c.arg("/C"); c } else { let mut c = Command::new("/bin/sh"); c.arg("-c"); c };
            c.arg(s);
            (c, s.clone())
        }
        Value::Array(a) => {
            let parts: Option<Vec<&str>> = a.iter().map(Value::as_str).collect();
            let Some(parts) = parts else { return Err(Fail::Crash(crash("TypeError：requirements.command 的項目要是字串"))) };
            let Some((prog, rest)) = parts.split_first() else { return Err(Fail::Input("requirements.source=command 但沒有設定 command".into())) };
            let mut c = Command::new(prog);
            c.args(rest);
            (c, pyfmt::repr(command))
        }
        other => return Err(Fail::Crash(crash(&format!("TypeError：requirements.command 要是字串或陣列（{}）", pyfmt::repr(other))))),
    };
    cmd.current_dir(proj).env("PYTHONUTF8", "1");
    for (k, v) in &ctx.env { cmd.env(k, v); }
    for (k, v) in env_extra { cmd.env(k, v); }
    let r = match pyutil::run(&mut cmd, None, Some(Duration::from_secs(120))) {
        Ok(r) => r,
        Err(pyutil::RunErr::Os(e)) => {
            let prog = match command { Value::Array(a) => a.first().and_then(Value::as_str).unwrap_or("").to_string(), _ => "/bin/sh".to_string() };
            return Err(Fail::Input(format!("需求清單指令執行失敗：{}", pyutil::os_error_text(&e, &prog))));
        }
        Err(pyutil::RunErr::Timeout) => return Err(Fail::Input(format!("需求清單指令執行失敗：Command '{shown}' timed out after 120 seconds"))),
    };
    if r.code != 0 {
        let err = pyutil::text(&r.stderr);
        return Err(Fail::Input(format!("需求清單指令 exit {}：{}", r.code, pyutil::tail(strip(&err), 300))));
    }
    let data = pyjson::loads(&pyutil::text(&r.stdout)).map_err(|e| Fail::Input(format!("需求清單指令的輸出不是 JSON：{e}")))?;
    normalize_requirements(&data, "command")
}

/// 範例 adapter：逐行套 id_pattern，第一個群組當 ID、該行其餘文字當需求原文；同一 ID 只取第一次出現。
/// 只是示範怎麼把 markdown 轉成正規化清單，不代表需求必須這樣寫。
fn adapter_spec_md(path: &str, id_pattern: &Value) -> Result<Vec<Req>, Fail> {
    let text = pyutil::read_text(path).map_err(|e| read_fail(e, |m| format!("讀不到需求文件 {path}：{m}")))?;
    // 使用者自訂的樣式走 hooks::re：`\w`、`\b`、`\s`、`$` 對照 0.x 的 Python 版（直接用 Rust 的引擎時，
    // 編號旁邊緊接結合符號的那一行會認不出來，整條需求就從清單消失）。內建樣式的差異已經在 py_word_space 補過，照舊。
    enum Pat {
        Builtin(FRegex),
        Custom(crate::hooks::re::Re),
    }
    let pat = match id_pattern {
        v if !truthy(v) => Pat::Builtin(
            FRegex::new(&py_word_space(&DEFAULT_ID_PATTERN.replace(r"[\s:", "[@S@:")).replace("@S@", r"\s\x1c-\x1f"))
                .map_err(|e| Fail::Crash(crash(&format!("re.error：requirements.id_pattern 寫壞了（{e}）"))))?,
        ),
        Value::String(s) => {
            let re = crate::hooks::re::Re::dynamic(s.clone(), "");
            if !re.compiles() {
                return Err(Fail::Crash(crash(&format!("re.error：requirements.id_pattern 寫壞了（{s}）"))));
            }
            Pat::Custom(re)
        }
        other => return Err(Fail::Crash(crash(&format!("TypeError：requirements.id_pattern 要是字串（{}）", pyfmt::repr(other))))),
    };
    let mut out: Vec<Req> = Vec::new();
    for (n, line) in pyutil::splitlines(&text).into_iter().enumerate() {
        // （整個符合的區間, 第一個群組的區間）
        let spans: (Option<(usize, usize)>, Option<(usize, usize)>) = match &pat {
            Pat::Builtin(re) => match re.captures(line) {
                Ok(Some(c)) => (c.get(0).map(|m| (m.start(), m.end())), c.get(1).map(|m| (m.start(), m.end()))),
                Ok(None) => continue,
                Err(e) => return Err(Fail::Crash(crash(&format!("requirements.id_pattern 配對失敗（{e}）")))),
            },
            Pat::Custom(re) => match re.captures(line) {
                Some(c) => (c.first().copied().flatten(), c.get(1).copied().flatten()),
                None => continue,
            },
        };
        let (Some(whole), Some(id)) = spans else {
            return Err(Fail::Crash(crash("IndexError：requirements.id_pattern 沒有第一個群組")));
        };
        let id = &line[id.0..id.1];
        if out.iter().any(|x| x.id == id) { continue; }
        let rest = line[whole.1..].trim_matches(|c| " \t:：|*`-、.)".contains(c));
        out.push(Req { id: id.to_string(), text: rest.to_string(), source: format!("{}:{}", pyutil::name(path), n + 1) });
    }
    if out.is_empty() {
        return Err(Fail::Input(format!("{path} 裡找不到符合 id_pattern 的需求（spec-md 是範例 adapter，需求寫法不同請改用 json 或 command）")));
    }
    Ok(out)
}

/// 需求來源 `openspec`：這次變更（進行中）要驗的每一條情境是一筆需求，id 用規格層的穩定 id。
/// 變更名稱：`--requirements-change` → 設定的 `requirements.change` → 只有一個進行中的變更時用它；判不出來就是輸入缺漏（hold）。
/// 規格檢查（`awf spec validate --change`）有問題一樣當成輸入缺漏：差異指到不存在的需求時，對照沒有意義。
/// 回傳（需求清單、來源標籤）。`save_run` 的規格檢查讀同一份設定（[`crate::spec::source_cfg`]）。
pub fn adapter_openspec(rc: &Value, proj: &str, change_override: Option<&str>) -> Result<(Vec<Req>, String), Fail> {
    use crate::spec;
    let (conf, opened, change) = openspec_pick(rc, proj, change_override)?;
    let Some(items) = spec::change_items(&opened.tree, &change) else {
        return Err(Fail::Input(format!("規格目錄 {} 沒有進行中的變更「{change}」", conf.display)));
    };
    let problems = spec::validate(&opened.tree, &opened.ids, Some(&change));
    if let Some(first) = problems.first() {
        return Err(Fail::Input(format!("變更「{change}」的規格檢查有 {} 項問題（awf spec validate --change {change}）：{}", problems.len(), spec::format_problem(&conf.display, first))));
    }
    if items.is_empty() {
        return Err(Fail::Input(format!("變更「{change}」的規格差異沒有任何要驗的需求或情境（ADDED／MODIFIED 是空的）")));
    }
    let reqs = items.into_iter().map(|i| Req { source: format!("{}/{}:{}", conf.display, i.file, i.line), id: i.id, text: i.title }).collect();
    Ok((reqs, format!("openspec:{}#{change}", conf.display)))
}

/// 讀規格目錄並決定這次對照哪一個變更（規則見 [`adapter_openspec`]）。回傳（設定、讀好的規格、變更名稱）。
fn openspec_pick(rc: &Value, proj: &str, change_override: Option<&str>) -> Result<(crate::spec::SourceCfg, crate::spec::Opened, String), Fail> {
    use crate::spec;
    let conf = spec::source_cfg(rc, proj).map_err(|e| Fail::Input(format!("需求來源 openspec 的設定不對：{e}")))?;
    let opened = spec::open_source(&conf).map_err(|e| Fail::Input(format!("讀不到規格目錄 {}：{e}", conf.display)))?;
    let active = spec::active_changes(&opened.tree);
    let named = change_override.map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).or_else(|| Some(conf.change.clone()).filter(|c| !c.is_empty()));
    let change = match named {
        Some(c) => {
            spec::check_change_name(&c).map_err(Fail::Input)?;
            c
        }
        None => match active.as_slice() {
            [one] => (*one).to_string(),
            [] => return Err(Fail::Input(format!("規格目錄 {} 沒有進行中的變更，不知道這次要對照哪些需求（用 --requirements-change 指定）", conf.display))),
            many => return Err(Fail::Input(format!("規格目錄 {} 有 {} 個進行中的變更（{}），用 --requirements-change 或 requirements.change 指定這次是哪一個", conf.display, many.len(), many.join("、")))),
        },
    };
    Ok((conf, opened, change))
}

/// 需求來源 `openspec` 實際用的設定：`--requirements` 有給時蓋掉 `path`。
fn openspec_conf(rc: &Value, path_override: Option<&str>) -> Value {
    let mut conf = if rc.is_object() { rc.clone() } else { json!({}) };
    if let (Some(p), Some(o)) = (path_override, conf.as_object_mut()) {
        o.insert("path".into(), json!(p));
    }
    conf
}

/// 需求來源是 `openspec`、又帶了 `--recipe <run.json>` 時：規格現在的樣子要和 run.json 記下的基準一樣
/// （design 核准時記的；沒有 design 段的 recipe 用 plan 的）。需求清單取自專案工作目錄的差異檔，team agent 改得到：
/// 核准之後把沒過的條目刪掉，清單就變短、對照也跟著過。條目變少、差異檔變了、沒有基準都回問題（hold）。
/// 沒有 run.json（CI）時沒有東西可以比：那邊的需求清單取自受保護的 base 分支，見 docs/自動合併信任模型.md。
fn openspec_baseline_errors(rc: &Value, proj: &str, path_override: Option<&str>, change_override: Option<&str>, run: &Value) -> Result<Vec<String>, Fail> {
    let (conf, opened, change) = openspec_pick(&openspec_conf(rc, path_override), proj, change_override)?;
    Ok(crate::save_run::baseline_drift("merge", run, &conf, &opened, &change))
}

/// 需求設定「有設」＝path 或 command 不是空的（還沒替換的佔位符當成空）。
pub(crate) fn configured(rc: &Value) -> bool {
    if !rc.is_object() { return false; }
    ["path", "command"].iter().any(|k| {
        let v = g(rc, k);
        matches!(v, Value::String(_) | Value::Array(_)) && truthy(v) && !placeholder_re().is_match(&str_of(v))
    })
}

/// 開發規範 profile（.claude/teams/methodology.json）的 requirements；沒有檔案或格式錯回空的。
fn methodology_requirements(proj: &str) -> Result<Value, Fail> {
    let path = pyutil::join(proj, ".claude/teams/methodology.json");
    let text = match pyutil::read_text(&path) {
        Ok(t) => t,
        Err(ReadErr::Os(_)) => return Ok(Value::Null),
        Err(e) => return Err(read_fail(e, |m| m.to_string())),
    };
    let data = match pyjson::loads(&text) {
        Ok(d) => d,
        Err(JsonErr::Decode(_)) => return Ok(Value::Null),
        Err(e) => return Err(Fail::Input(format!("讀不到 {path}：{e}"))),
    };
    // 繼承（extends）解析失敗：hold（不當作沒有 extends，也不默默改用守門設定的需求來源）
    let data = crate::profile::resolve_str(&path, data).map_err(|e| Fail::Input(format!("讀不到 {path}：{e}")))?;
    let rc = g(&data, "requirements");
    Ok(if rc.is_object() { rc.clone() } else { Value::Null })
}

/// 回傳（需求設定、出處）：guard.config.json 有設 path／command 時以它為準，否則用 methodology profile 的。
fn requirements_config(cfg: &Value, proj: &str) -> Result<(Value, &'static str), Fail> {
    let rc = g(cfg, "requirements");
    let rc = if rc.is_object() { rc.clone() } else { Value::Null };
    if configured(&rc) { return Ok((rc, "guard.config.json")); }
    let mc = methodology_requirements(proj)?;
    if truthy(&mc) { return Ok((mc, "methodology.json")); }
    Ok((rc, "guard.config.json"))
}

fn load_requirements(ctx: &Ctx, cfg: &Value, proj: &str, path_override: Option<&str>, source_override: Option<&str>, change_override: Option<&str>, env_extra: &[(&str, String)]) -> Result<(Vec<Req>, String), Fail> {
    let (rc, origin) = requirements_config(cfg, proj)?;
    let path_override = path_override.filter(|p| !p.is_empty());
    let source: Value = match source_override.filter(|s| !s.is_empty()) {
        Some(s) => json!(s),
        None if truthy(g(&rc, "source")) => g(&rc, "source").clone(),
        None => json!(if path_override.unwrap_or("").ends_with(".json") { "json" } else { "spec-md" }),
    };
    let source = match &source {
        Value::String(s) if ADAPTERS.contains(&s.as_str()) => s.as_str(),
        v if !hashable(v) => return Err(Fail::Crash(crash("TypeError：requirements.source 不能是陣列或物件"))),
        other => return Err(Fail::Input(format!("不認得的需求來源 {}（可用：{}）", str_of(other), ADAPTERS.join("、")))),
    };
    let raw = match path_override {
        Some(p) => p.to_string(),
        None => match g(&rc, "path") {
            v if !truthy(v) => String::new(),
            Value::String(s) => s.clone(),
            other => return Err(Fail::Crash(crash(&format!("TypeError：requirements.path 要是字串（{}）", pyfmt::repr(other))))),
        },
    };
    let path = if raw.is_empty() { None } else { Some(pyutil::join(proj, &raw)) };
    if source == "openspec" {
        let conf = openspec_conf(&rc, path_override);
        let (reqs, label) = adapter_openspec(&conf, proj, change_override)?;
        return Ok((reqs, if origin == "methodology.json" && path_override.is_none() { format!("{label}（methodology.json）") } else { label }));
    }
    let reqs = match (source, &path) {
        ("command", _) => adapter_command(ctx, g(&rc, "command"), proj, env_extra)?,
        (_, None) => return Err(Fail::Input("沒有需求來源：請在 guard.config.json 或 .claude/teams/methodology.json 設 requirements.path，或帶 --requirements".into())),
        ("json", Some(p)) => adapter_json(p)?,
        (_, Some(p)) => adapter_spec_md(p, g(&rc, "id_pattern"))?,
    };
    let label = if raw.is_empty() { source.to_string() } else { format!("{source}:{raw}") };
    Ok((reqs, if origin == "methodology.json" && path_override.is_none() { format!("{label}（methodology.json）") } else { label }))
}

// ------------------------------------------------------------------ 其他輸入的正規化

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub requirement_id: String,
    pub result: String,
    pub evidence: String,
}

/// 回傳（證據清單、驗證時的 commit、anti_cheat）。接受：正規化清單；{"commit","anti_cheat","results"}；
/// 或 criteria payload／信封（例如 dev-verifier 的產出，id 視為 requirement_id）。
fn normalize_evidence(data: &Value) -> Result<(Vec<Evidence>, Option<Value>, Value), Fail> {
    let data = if g(data, "payload").is_object() { g(data, "payload") } else { data };
    let (mut commit, mut anti, mut items) = (&NULL, Value::Array(Vec::new()), data);
    if let Value::Object(o) = data {
        commit = g(data, "commit");
        if truthy(g(data, "anti_cheat")) { anti = g(data, "anti_cheat").clone(); }
        items = if o.contains_key("results") { g(data, "results") } else { g(data, "criteria") };
    }
    let Value::Array(items) = items else {
        return Err(Fail::Input("證據格式不對：要是 [{requirement_id, result, evidence}] 或帶 results／criteria 的物件".into()));
    };
    let mut out = Vec::new();
    for (i, e) in items.iter().enumerate() {
        let Value::Object(o) = e else { return Err(Fail::Input(format!("證據第 {i} 項不是物件"))) };
        let rid = if o.contains_key("requirement_id") { g(e, "requirement_id") } else { g(e, "id") };
        if !truthy(rid) { return Err(Fail::Input(format!("證據第 {i} 項缺 requirement_id"))); }
        out.push(Evidence { requirement_id: str_of(rid), result: str_or_empty(g(e, "result")), evidence: str_or_empty(g(e, "evidence")) });
    }
    let commit = if truthy(commit) && commit.as_str() != Some("n/a") { Some(commit.clone()) } else { None };
    Ok((out, commit, anti))
}

fn normalize_checks(data: &Value) -> Result<Vec<(String, String)>, Fail> {
    if let Value::Object(o) = data {
        return Ok(o.iter().map(|(k, v)| (k.clone(), str_of(v))).collect());
    }
    let Value::Array(items) = data else { return Err(Fail::Input("required checks 格式不對".into())) };
    let mut out = Vec::new();
    for c in items {
        if !c.is_object() { return Err(Fail::Input("required checks 每項要是物件".into())); }
        let pick = |keys: &[&str], default: &str| keys.iter().map(|k| g(c, k)).find(|v| truthy(v)).map(str_of).unwrap_or_else(|| default.to_string());
        out.push((pick(&["name", "context"], "?"), pick(&["conclusion", "state", "bucket", "status"], "")));
    }
    Ok(out)
}

fn normalize_findings(data: &Value) -> Vec<Value> {
    let data = if g(data, "payload").is_object() { g(data, "payload") } else { data };
    let empty = Value::Array(Vec::new());
    let data = match data {
        Value::Object(o) => o.get("findings").unwrap_or(&empty),
        other => other,
    };
    match data {
        Value::Array(a) => a.iter().filter(|f| f.is_object()).cloned().collect(),
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------ diff 解析

/// `s[a:b]`（超出範圍就截到邊界）。
fn sl(s: &[char], a: usize, b: usize) -> &[char] {
    let b = b.min(s.len());
    if a >= b { &[] } else { &s[a..b] }
}

fn text_of(s: &[char]) -> String {
    s.iter().collect()
}

fn find_all(s: &[char], pat: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + pat.len() <= s.len() {
        if &s[i..i + pat.len()] == pat {
            out.push(i);
            i += pat.len();
        } else {
            i += 1;
        }
    }
    out
}

/// git 的 C 風格引號路徑（core.quotePath）：`s[i]` 是開頭的引號；回傳（還原跳脫後的字串、結尾引號之後的位置）。格式不對回 None。
fn c_unquote(s: &[char], i: usize) -> Option<(String, usize)> {
    let mut out: Vec<u8> = Vec::new();
    let mut j = i + 1;
    while j < s.len() {
        let c = s[j];
        if c == '"' { return Some((String::from_utf8_lossy(&out).into_owned(), j + 1)); }
        if c == '\\' {
            if let [a @ '0'..='3', b @ '0'..='7', d @ '0'..='7'] = *sl(s, j + 1, j + 4) {
                let oct = |x: char| x as u32 - '0' as u32;
                out.push((oct(a) * 64 + oct(b) * 8 + oct(d)) as u8);
                j += 4;
                continue;
            }
            let byte = match s.get(j + 1) {
                Some('a') => 7,
                Some('b') => 8,
                Some('t') => 9,
                Some('n') => 10,
                Some('v') => 11,
                Some('f') => 12,
                Some('r') => 13,
                Some('"') => 34,
                Some('\\') => 92,
                _ => return None,
            };
            out.push(byte);
            j += 2;
            continue;
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        j += 1;
    }
    None
}

/// ---／+++／rename／copy 行的路徑：引號路徑還原跳脫；沒引號的取到第一個 tab 為止（含 tab 的檔名 git 一定加引號）。
fn path_field(s: &[char]) -> Option<String> {
    if s.first() == Some(&'"') {
        let (p, end) = c_unquote(s, 0)?;
        return if end == s.len() || s.get(end) == Some(&'\t') { Some(p) } else { None };
    }
    let cut = s.iter().position(|&c| c == '\t').unwrap_or(s.len());
    if cut == 0 { None } else { Some(text_of(&s[..cut])) }
}

fn strip_prefix(p: Option<String>, prefix: &str) -> Option<String> {
    let p = p?;
    if p == "/dev/null" { return Some(p); }
    match p.strip_prefix(prefix) {
        Some(rest) if !rest.is_empty() => Some(rest.to_string()),
        _ => None,
    }
}

/// 「diff --git 」之後的部分 →（舊路徑、新路徑）（去掉 a/、b/）。有歧義或格式不對回 None，呼叫端 hold。
fn git_header(rest: &[char]) -> Option<(String, String)> {
    let (a, b): (Option<String>, Option<String>);
    if rest.first() == Some(&'"') {
        let (qa, end) = c_unquote(rest, 0)?;
        if rest.get(end) != Some(&' ') { return None; }
        let tail = sl(rest, end + 1, rest.len());
        if tail.first() == Some(&'"') {
            let (qb, end2) = c_unquote(tail, 0)?;
            if end2 != tail.len() { return None; }
            b = Some(qb);
        } else {
            b = Some(text_of(tail));
        }
        a = Some(qa);
    } else if rest.last() == Some(&'"') {
        let mut found = None;
        for at in find_all(rest, &[' ', '"']) {
            if let Some((q, end)) = c_unquote(rest, at + 1) {
                if end == rest.len() {
                    found = Some((text_of(&rest[..at]), q));
                    break;
                }
            }
        }
        let (fa, fb) = found?;
        (a, b) = (Some(fa), Some(fb));
    } else {
        let n = rest.len();
        let half = n / 2;
        if n % 2 == 1 && rest.get(half) == Some(&' ') && sl(rest, 2, half) == sl(rest, half + 3, n) {
            // 沒改名：兩邊相同，檔名有空白也切得開
            (a, b) = (Some(text_of(&rest[..half])), Some(text_of(sl(rest, half + 1, n))));
        } else {
            let cuts = find_all(rest, &[' ', 'b', '/']);
            let [cut] = cuts[..] else { return None }; // 改名且檔名含「 b/」：切法不只一種
            (a, b) = (Some(text_of(&rest[..cut])), Some(text_of(sl(rest, cut + 1, n))));
        }
    }
    let (a, b) = (strip_prefix(a, "a/")?, strip_prefix(b, "b/")?);
    if a.is_empty() || b.is_empty() || a == "/dev/null" || b == "/dev/null" { return None; }
    Some((a, b))
}

/// 一個 `diff --git` 區塊（publish 掃機敏檔名也用它）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    pub old: Option<String>,
    pub new: Option<String>,
    pub deleted: bool,
    pub added: bool,
    pub binary: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    /// 路徑 → [新增行數, 刪除行數]，照出現順序。
    pub files: Vec<(String, [u64; 2])>,
    index: HashMap<String, usize>,
    pub added_lines: Vec<(String, String)>,
    pub entries: Vec<Entry>,
    pub errors: Vec<String>,
}

impl Diff {
    fn touch(&mut self, p: Option<&str>) {
        let Some(p) = p.filter(|p| !p.is_empty() && *p != "/dev/null") else { return };
        if !self.index.contains_key(p) {
            self.index.insert(p.to_string(), self.files.len());
            self.files.push((p.to_string(), [0, 0]));
        }
    }

    /// `files[cur][which] += 1`；cur 不在 files 裡時 Python 是 KeyError。
    fn bump(&mut self, cur: Option<&str>, which: usize) -> Result<(), Crash> {
        let slot = cur.and_then(|c| self.index.get(c)).and_then(|&i| self.files.get_mut(i));
        match slot {
            Some((_, counts)) => {
                counts[which] += 1;
                Ok(())
            }
            None => Err(crash(&format!("KeyError：diff 的行屬於沒有記錄的檔案（{}）", cur.map(repr_str).unwrap_or_else(|| "None".into())))),
        }
    }

    /// 只有 files 的 Diff（對跑 risk_tier 用）。
    pub fn from_files(files: Vec<(String, [u64; 2])>) -> Diff {
        let index = files.iter().enumerate().map(|(i, (p, _))| (p.clone(), i)).collect();
        Diff { files, index, ..Diff::default() }
    }

    pub fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|(p, _)| p.as_str()).collect()
    }

    /// 和 0.x 的 Python 版 parse_diff 回傳的 dict 同形狀（對跑用）。
    pub fn to_value(&self) -> Value {
        let files: Map<String, Value> = self.files.iter().map(|(p, c)| (p.clone(), json!([c[0], c[1]]))).collect();
        let entries: Vec<Value> = self.entries.iter().map(|e| json!({"old": e.old, "new": e.new, "deleted": e.deleted, "added": e.added, "binary": e.binary})).collect();
        json!({"files": files, "added_lines": self.added_lines.iter().map(|(p, l)| json!([p, l])).collect::<Vec<_>>(), "entries": entries, "errors": self.errors})
    }
}

/// git diff 輸出 → 改了哪些檔、新增了哪些行。
///
/// - files 同時記改名／複製前後的路徑與刪除的檔案：風險等級要看到檔案「從哪裡移出來」。
/// - 路徑支援 git 的 C 風格引號（非 ASCII、tab 等檔名），還原 octal 跳脫。
/// - 依 hunk 標頭的行數判斷哪些是內容行：新增行內容以「++ 」開頭時整行是「+++ 」，不能當成檔案標頭；
///   只有 hunk 外、緊接在「--- 」之後的「+++ 」才是標頭。
/// - 解析不了的 diff --git 標頭、---／+++ 標頭、hunk 標頭，或行數對不上（diff 被截斷）都記進 errors，呼叫端一律 hold。
pub fn parse_diff(text: &str) -> Result<Diff, Crash> {
    let mut d = Diff::default();
    let mut cur: Option<String> = None;
    let mut ent: Option<usize> = None;
    let mut left: [u64; 2] = [0, 0]; // 目前 hunk 還剩的 舊／新 行數
    let mut minus: Option<Option<String>> = None; // 上一行是「--- 」標頭時，記它的路徑

    // 不用 splitlines：內容裡的 \f、\x1c、U+2028 等不能拆行，否則 hunk 行數會錯
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") { lines.pop(); }
    for (idx, line) in lines.into_iter().enumerate() {
        let n = idx + 1;
        if left[0] > 0 || left[1] > 0 {
            let c = line.chars().next();
            if c == Some('+') && left[1] > 0 {
                left[1] -= 1;
                d.bump(cur.as_deref(), 0)?;
                d.added_lines.push((cur.clone().unwrap_or_default(), line[1..].to_string()));
                continue;
            }
            if c == Some('-') && left[0] > 0 {
                left[0] -= 1;
                d.bump(cur.as_deref(), 1)?;
                continue;
            }
            if matches!(c, Some(' ') | None) && left[0] > 0 && left[1] > 0 {
                left[0] -= 1;
                left[1] -= 1;
                continue;
            }
            if c == Some('\\') { continue; }
            d.errors.push(format!("第 {n} 行：hunk 行數和標頭對不上（diff 可能被截斷或竄改）"));
            left = [0, 0];
        }
        let prev_minus = minus.take();
        let chars: Vec<char>;
        if let Some(rest) = line.strip_prefix("diff --git ") {
            chars = rest.chars().collect();
            match git_header(&chars) {
                None => {
                    d.errors.push(format!("第 {n} 行：解析不了的 diff --git 標頭：{}", repr_str(head(line, 200))));
                    cur = None;
                    d.entries.push(Entry::default());
                }
                Some((a, b)) => {
                    d.entries.push(Entry { old: Some(a.clone()), new: Some(b.clone()), ..Entry::default() });
                    d.touch(Some(&a));
                    d.touch(Some(&b));
                    cur = Some(b);
                }
            }
            ent = Some(d.entries.len() - 1);
        } else if line.starts_with("@@") {
            let m = hunk_re().captures(line);
            let counts = m.as_ref().map(|m| {
                // 「-3」省略行數＝1；樣式保證全是數字，轉不成 u64 只會是太大（Python 的整數沒有上限），當成最大值
                let num = |i: usize| m.get(i).map_or(Some(1), |x| Some(x.as_str().parse::<u64>().unwrap_or(u64::MAX)));
                (num(1), num(2))
            });
            match counts {
                Some((Some(a), Some(b))) if cur.is_some() => left = [a, b],
                Some((Some(_), Some(_))) => d.errors.push(format!("第 {n} 行：hunk 前面沒有可辨認的檔案標頭：{}", repr_str(head(line, 120)))),
                _ => d.errors.push(format!("第 {n} 行：解析不了的 hunk 標頭：{}", repr_str(head(line, 120)))),
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            chars = rest.chars().collect();
            let p = strip_prefix(path_field(&chars), "a/");
            if p.is_none() { d.errors.push(format!("第 {n} 行：解析不了的 --- 標頭：{}", repr_str(head(line, 200)))); }
            minus = Some(p);
        } else if let (Some(rest), Some(old)) = (line.strip_prefix("+++ "), prev_minus.as_ref()) {
            chars = rest.chars().collect();
            let Some(new) = strip_prefix(path_field(&chars), "b/") else {
                d.errors.push(format!("第 {n} 行：解析不了的 +++ 標頭：{}", repr_str(head(line, 200))));
                continue;
            };
            let at = match ent {
                Some(at) => at,
                None => {
                    d.entries.push(Entry { old: old.clone(), new: Some(new.clone()), ..Entry::default() });
                    d.entries.len() - 1
                }
            };
            ent = Some(at);
            if let Some(e) = d.entries.get_mut(at) {
                if new == "/dev/null" { e.deleted = true; } else if old.as_deref() == Some("/dev/null") { e.added = true; }
            }
            d.touch(old.as_deref());
            d.touch(Some(&new));
            cur = if new != "/dev/null" { Some(new) } else { old.clone() };
        } else if ["rename from ", "copy from ", "rename to ", "copy to "].iter().any(|p| line.starts_with(p)) {
            let mut parts = line.splitn(3, ' ');
            let (_, word, path) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            chars = path.chars().collect();
            let p = path_field(&chars);
            let Some(p) = p else {
                d.errors.push(format!("第 {n} 行：解析不了的改名／複製路徑：{}", repr_str(head(line, 200))));
                continue;
            };
            d.touch(Some(&p));
            let old_side = word == "from";
            if let Some(e) = ent.and_then(|at| d.entries.get_mut(at)) {
                if old_side { e.old = Some(p.clone()); } else { e.new = Some(p.clone()); }
            }
            if !old_side { cur = Some(p); }
        } else if line.starts_with("deleted file mode") && ent.is_some() {
            if let Some(e) = ent.and_then(|at| d.entries.get_mut(at)) { e.deleted = true; }
        } else if line.starts_with("new file mode") && ent.is_some() {
            if let Some(e) = ent.and_then(|at| d.entries.get_mut(at)) { e.added = true; }
        } else if (line.starts_with("Binary files ") || line == "GIT binary patch") && ent.is_some() {
            if let Some(e) = ent.and_then(|at| d.entries.get_mut(at)) { e.binary = true; }
        } else if line.starts_with('+') {
            // hunk 外的新增行（格式不對或被竄改）：照樣當新增行掃描，不放過
            if cur.is_none() {
                d.errors.push(format!("第 {n} 行：不屬於任何檔案的新增行"));
            } else {
                d.bump(cur.as_deref(), 0)?;
                d.added_lines.push((cur.clone().unwrap_or_default(), line[1..].to_string()));
            }
        }
    }
    if left[0] > 0 || left[1] > 0 { d.errors.push("diff 在 hunk 中間結束（可能被截斷）".into()); }
    Ok(d)
}

/// `auto_merge.sensitive_paths_extra` 最多幾條、每條最多幾個字。
pub const SENSITIVE_EXTRA_MAX: usize = 64;
pub const SENSITIVE_EXTRA_PATTERN_MAX: usize = 300;

/// guard.config.json 的 `auto_merge.sensitive_paths_extra`：專案另外加的高風險路徑樣式（Python `re` 的寫法，分大小寫，
/// 和內建樣式一樣用 search）。只能加：內建清單拿不掉。寫得不對回 `Err(說明)`，呼叫端一律交給人。
pub fn sensitive_extra(am: &Value) -> Result<Vec<Re>, String> {
    let what = "auto_merge.sensitive_paths_extra";
    let v = g(am, "sensitive_paths_extra");
    if v.is_null() { return Ok(Vec::new()); }
    let Some(items) = v.as_array() else { return Err(format!("{what} 要是正規式的陣列")) };
    if items.len() > SENSITIVE_EXTRA_MAX { return Err(format!("{what} 最多 {SENSITIVE_EXTRA_MAX} 條（收到 {} 條）", items.len())); }
    let mut out = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let Some(src) = it.as_str().filter(|t| !strip(t).is_empty() && t.chars().count() <= SENSITIVE_EXTRA_PATTERN_MAX) else {
            return Err(format!("{what}[{i}] 要是 1–{SENSITIVE_EXTRA_PATTERN_MAX} 個字的正規式"));
        };
        out.push(Re::python(src, "").map_err(|e| format!("{what}[{i}] {e}"))?);
    }
    Ok(out)
}

/// 專案在 profile 的 `diff_classes_extra` 加的類別，對應到 gate 的高風險說明（code 不算高風險，只是不再當成文件或測試）。
fn extra_class_label(cls: &str) -> Option<&'static str> {
    match cls {
        "migration" => Some("資料庫 migration"),
        "manifest" => Some("依賴或版本宣告"),
        "agent_config" => Some("agent 設定、守門或 gate"),
        _ => None,
    }
}

/// 風險等級與交給人的理由。
pub fn risk_tier(diff: &Diff, am: &Value) -> Result<(&'static str, Vec<String>), Crash> {
    risk_tier_with(diff, am, None)
}

/// 同 [`risk_tier`]，另外套用專案加的設定（都只會往嚴的方向）：
/// - `auto_merge.sensitive_paths_extra` 命中的路徑算高風險；寫壞了整個判 high、交給人。
/// - `classes`（profile 的 `diff_classes_extra`）：命中 migration、manifest、agent_config 的路徑算高風險；
///   命中任何一類（含 code）的路徑不再當成文件或測試，所以不會落在 low。
pub fn risk_tier_with(diff: &Diff, am: &Value, classes: Option<&recipe::DiffExtra>) -> Result<(&'static str, Vec<String>), Crash> {
    let lines: i128 = diff.files.iter().map(|(_, c)| (c[0] + c[1]) as i128).sum();
    let count = diff.files.len() as i128;
    let extra = match sensitive_extra(am) {
        Ok(x) => x,
        Err(e) => return Ok(("high", vec![format!("高風險路徑的設定讀不懂（{e}），無法確認這次改動的風險")])),
    };
    let class_hits = |path: &str| -> Vec<&'static str> { classes.map(|c| c.hits(path)).unwrap_or_default() };
    let hits: Vec<String> = diff.files.iter().filter_map(|(path, _)| {
        sensitive().iter().find(|(re, _)| re.is_match(path)).map(|(_, what)| format!("{path}（{what}）"))
            .or_else(|| extra.iter().any(|re| re.is_match(path)).then(|| format!("{path}（專案設定的高風險路徑）")))
            .or_else(|| class_hits(path).into_iter().find_map(extra_class_label).map(|what| format!("{path}（{what}；專案設定的分類）")))
    }).collect();
    let large = int_or(g(am, "large_lines"), DEFAULT_LARGE_LINES, "large_lines")?;
    let small = int_or(g(am, "small_lines"), DEFAULT_SMALL_LINES, "small_lines")?;
    if !hits.is_empty() { return Ok(("high", vec![format!("改到高風險路徑：{}", hits.join("、"))])); }
    if lines > large || count > int_or(g(am, "large_files"), DEFAULT_LARGE_FILES, "large_files")? {
        return Ok(("high", vec![format!("diff 過大：{count} 個檔、{lines} 行")]));
    }
    let all = |re: &Regex| diff.files.iter().all(|(p, _)| re.is_match(p) && class_hits(p).is_empty());
    if !diff.files.is_empty() && lines <= small && (all(doc_re()) || all(test_re())) { return Ok(("low", Vec::new())); }
    Ok(("medium", Vec::new()))
}

// ------------------------------------------------------------------ 判定

/// 已載入的輸入（0.x 的 Python 版的 inp dict）。None＝沒給或讀不到。
#[derive(Debug, Clone, Default)]
pub struct Inp {
    pub repo: String,
    pub pr: i64,
    pub head_sha: String,
    pub reviewed_shas: Vec<String>,
    pub base_sha: Option<String>,
    pub tests_before: Option<i64>,
    pub tests_after: Option<i64>,
    pub findings_given: bool,
    pub merged_today: Option<String>,
    pub frozen_arg: Option<String>,
    pub timestamp: String,
    pub decision: Option<Value>,
    pub requirements: Option<Vec<Req>>,
    pub requirements_source: String,
    pub evidence: Option<Vec<Evidence>>,
    pub evidence_commit: Option<Value>,
    pub anti_cheat: Value,
    pub checks: Option<Vec<(String, String)>>,
    pub findings: Vec<Value>,
    pub diff: Option<Diff>,
    pub recipe: Option<Value>,
    /// 專案 profile 的 `diff_classes_extra`（原值；None＝沒設定）。
    pub diff_extra: Option<Value>,
    pub patch_ids: HashMap<String, Option<String>>,
    pub frozen: Option<String>,
}

struct Verdict {
    hold: Vec<String>,
    esc: Vec<String>,
    carried: Vec<String>,
}

impl Verdict {
    fn need(&mut self, present: bool, what: &str) -> bool {
        if !present { self.hold.push(format!("缺輸入：{what}（fail-closed）")); }
        present
    }
}

/// rebase 後沿用 verdict（auto_merge.carry_verdict_on_rebase，預設關閉）：patch-id 由 gather 用 git 自己算，相同才沿用。
fn sha_mismatch(v: &mut Verdict, old: &Value, when: &str, head_sha: &str, carry: bool, pids: &HashMap<String, Option<String>>) -> Result<(), Crash> {
    let shown = str_of(old);
    if !carry {
        v.hold.push(format!("head SHA 不符：{when} {shown}，目前 {head_sha}"));
        return Ok(());
    }
    if !hashable(old) { return Err(crash("TypeError：commit 不能是陣列或物件")); }
    let id = |k: &str| pids.get(k).and_then(|x| x.as_deref()).filter(|x| !x.is_empty());
    let (a, b) = (old.as_str().and_then(id), id(head_sha));
    match (a, b) {
        (Some(a), Some(b)) if a == b => v.carried.push(format!("沿用 verdict：{when} {} → 目前 {}（patch-id 相同）", head(&shown, 12), head(head_sha, 12))),
        _ => {
            let why = if a.is_some() && b.is_some() { "patch-id 不同" } else { "無法計算 patch-id" };
            v.hold.push(format!("head SHA 不符：{when} {shown}，目前 {head_sha}（{why}，不沿用）"));
        }
    }
    Ok(())
}

fn sorted_keys(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sorted_keys(&o[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted_keys).collect()),
        other => other.clone(),
    }
}

/// 判定（不寫檔）。hold 理由優先於 escalate。`audit_lines` 是 merge-audit.jsonl 已有的紀錄。
pub fn evaluate(sch: &Schemas, inp: &Inp, cfg: &Value, audit_lines: &[Value]) -> Result<Value, Crash> {
    let mut v = Verdict { hold: Vec::new(), esc: Vec::new(), carried: Vec::new() };
    let mut followups: Vec<Value> = Vec::new();
    let repo = inp.repo.as_str();
    let pr = Value::from(inp.pr);
    let head_sha = strip(&inp.head_sha);
    let am = auto_merge(cfg);

    if head_sha.is_empty() { v.hold.push("缺輸入：目前 head SHA".into()); }
    let carry = truthy(g(am, "carry_verdict_on_rebase")) && inp.base_sha.as_deref().is_some_and(|b| !b.is_empty());
    let pids = &inp.patch_ids;

    // 1. steward 的信封／payload：schema＋語意規則
    let empty = Value::Object(Map::new());
    let mut payload: &Value = &empty;
    if let (true, Some(d)) = (v.need(inp.decision.is_some(), "steward 的 merge-decision"), inp.decision.as_ref()) {
        let errs = if g(d, "payload").is_object() {
            payload = g(d, "payload");
            sch.check(d, Some("merge-decision")).map_err(|e| crash(&e))?
        } else {
            if d.is_object() { payload = d; }
            let full = sch.compose(Some("merge-decision")).map_err(|e| crash(&e))?;
            let mut errs = schema::validate(d, g(g(&full, "properties"), "payload"), "$");
            errs.extend(schema::merge_decision_errors(d));
            errs
        };
        if !errs.is_empty() { v.hold.push(format!("merge-decision 不合格：{}", repr_strs(first(&errs, 5)))); }
        if !pyfmt::eq(g(payload, "repo"), &json!(repo)) || !pyfmt::eq(g(payload, "pr"), &pr) {
            v.hold.push(format!("merge-decision 的 repo／pr（{}#{}）與這次（{repo}#{}）不同", str_of(g(payload, "repo")), str_of(g(payload, "pr")), inp.pr));
        }
        if !head_sha.is_empty() && g(payload, "head_sha").as_str() != Some(head_sha) {
            sha_mismatch(&mut v, &json!(str_of(g(payload, "head_sha"))), "對照時", head_sha, carry, pids)?;
        }
        match g(payload, "decision").as_str() {
            Some("hold") => v.hold.push("steward 判 hold（gate 只會更嚴，不會放寬）".into()),
            Some("escalate") => v.esc.push("steward 判 escalate（gate 只會更嚴，不會放寬）".into()),
            _ => {}
        }
        followups.extend(py_iter(g(payload, "followups"))?.into_iter().filter(Value::is_object));
    }

    // 2. 需求對照（traceability）機械複核
    let ev_ok = v.need(inp.evidence.is_some(), "驗證證據");
    if let (true, Some(reqs)) = (v.need(inp.requirements.is_some(), "需求清單"), inp.requirements.as_ref()) {
        let mut ev_by: HashMap<&str, Vec<&Evidence>> = HashMap::new();
        for e in inp.evidence.iter().flatten() { ev_by.entry(e.requirement_id.as_str()).or_default().push(e); }
        let mut rows: Vec<(Value, Value)> = Vec::new();
        for r in py_iter(g(payload, "traceability"))?.into_iter().filter(Value::is_object) {
            let key = g(&r, "requirement_id").clone();
            if !hashable(&key) { return Err(crash("TypeError：traceability 的 requirement_id 不能是陣列或物件")); }
            match rows.iter_mut().find(|(k, _)| pyfmt::eq(k, &key)) {
                Some(slot) => slot.1 = r,
                None => rows.push((key, r)),
            }
        }
        let mut have_fu: Vec<Value> = Vec::new();
        for f in &followups {
            let key = g(f, "requirement_id");
            if !hashable(key) { return Err(crash("TypeError：followups 的 requirement_id 不能是陣列或物件")); }
            have_fu.push(key.clone());
        }
        let mut fu = |followups: &mut Vec<Value>, rid: &str, expected: &str, actual: String, adj: &str| {
            if !have_fu.iter().any(|k| k.as_str() == Some(rid)) {
                followups.push(json!({"requirement_id": rid, "expected": expected, "actual": actual, "adjustment": adj}));
                have_fu.push(json!(rid));
            }
        };
        for r in reqs {
            let rid = r.id.as_str();
            let Some((_, row)) = rows.iter().find(|(k, _)| k.as_str() == Some(rid)) else {
                v.hold.push(format!("traceability：需求 {rid} 沒有出現在對照表（missing）"));
                fu(&mut followups, rid, &r.text, "對照表漏列這條需求".into(), "補上這條需求的對照與驗證證據");
                continue;
            };
            let result = g(row, "result");
            if result.as_str() != Some("match") {
                v.hold.push(format!("traceability：需求 {rid} 判 {}", str_of(result)));
                fu(&mut followups, rid, &r.text, format!("對照結果 {}", str_of(result)), "依需求調整實作或補證據");
                continue;
            }
            if !ev_ok { continue; }
            let evs = ev_by.get(rid).map(Vec::as_slice).unwrap_or(&[]);
            let bad: Vec<String> = evs.iter().filter(|e| e.result != "pass").map(|e| e.result.clone()).collect();
            if evs.is_empty() {
                v.hold.push(format!("traceability：需求 {rid} 宣稱 match，但驗證證據裡沒有這條（missing）"));
                fu(&mut followups, rid, &r.text, "沒有驗證證據".into(), "補驗證並附證據");
            } else if !bad.is_empty() {
                let bad = repr_strs(&bad);
                v.hold.push(format!("traceability：需求 {rid} 宣稱 match，但驗證結果是 {bad}（mismatch）"));
                let shown = evs.first().map(|e| head(&e.evidence, 200)).unwrap_or("");
                fu(&mut followups, rid, &r.text, format!("驗證結果 {bad}：{shown}"), "修正實作直到驗證 pass");
            }
        }
        let extra: Vec<Value> = rows.iter().filter(|(k, _)| !k.as_str().is_some_and(|k| reqs.iter().any(|r| r.id == k))).map(|(k, _)| k.clone()).collect();
        if !extra.is_empty() { v.hold.push(format!("traceability：對照表有需求清單裡沒有的 ID {}", pyfmt::repr(&Value::Array(extra)))); }
        let oos = g(payload, "out_of_scope");
        if truthy(oos) {
            let len = match oos {
                Value::String(s) => s.chars().count(),
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                other => return Err(crash(&format!("TypeError：out_of_scope 不是清單（{}）", pyfmt::repr(other)))),
            };
            v.hold.push(format!("traceability：有 {len} 項對不到需求的改動（out_of_scope）"));
            for o in py_iter(oos)?.iter().filter(|o| o.is_object()) {
                followups.push(json!({"requirement_id": "out_of_scope", "expected": "改動都要對到需求",
                    "actual": format!("{}：{}", str_of(g(o, "location")), str_of(g(o, "change"))), "adjustment": "移出這個 PR，或先補需求再改"}));
            }
        }
        if reqs.is_empty() { v.hold.push("traceability：需求清單是空的".into()); }
    }

    // 3. 驗證證據：全部 pass、anti_cheat 為空、驗證時的 commit 等於 head
    if let Some(evidence) = &inp.evidence {
        let mut notpass: Vec<String> = evidence.iter().filter(|e| e.result != "pass").map(|e| e.requirement_id.clone()).collect();
        notpass.sort();
        notpass.dedup();
        if !notpass.is_empty() { v.hold.push(format!("驗證未全部 pass：{}", repr_strs(&notpass))); }
        if evidence.is_empty() { v.hold.push("驗證證據是空的".into()); }
        if truthy(&inp.anti_cheat) {
            let shown = match &inp.anti_cheat {
                Value::Array(a) => pyfmt::repr(&Value::Array(first(a, 5).to_vec())),
                Value::String(s) => head(s, 5).to_string(),
                other => return Err(crash(&format!("TypeError：anti_cheat 不是清單（{}）", pyfmt::repr(other)))),
            };
            v.hold.push(format!("反作弊檢查有發現：{shown}"));
        }
        match &inp.evidence_commit {
            None => v.hold.push("驗證證據沒有 commit（不知道驗的是哪個 commit，無法綁定 head SHA；fail-closed）".into()),
            Some(c) if !head_sha.is_empty() && c.as_str() != Some(head_sha) => sha_mismatch(&mut v, c, "驗證時", head_sha, carry, pids)?,
            Some(_) => {}
        }
    }
    if inp.findings_given && inp.reviewed_shas.is_empty() {
        v.hold.push("有審查發現（--findings）卻沒有 reviewed-sha（不知道審的是哪個 commit；fail-closed）".into());
    }
    for s in &inp.reviewed_shas {
        if !head_sha.is_empty() && s != head_sha { sha_mismatch(&mut v, &json!(s), "審查時", head_sha, carry, pids)?; }
    }

    // 4. required checks
    if let (true, Some(checks)) = (v.need(inp.checks.is_some(), "required checks 狀態"), inp.checks.as_ref()) {
        if checks.is_empty() { v.hold.push("沒有任何 required check".into()); }
        let allowed = list_of(g(am, "allowed_skipped_checks"))?;
        // 專案自己列的必過 check（awf ci render --write 會把 required 的 job 名稱加進來）：沒出現在 checks 裡＝沒驗到，hold
        let missing: Vec<String> = list_of(g(am, "required_checks"))?.into_iter().filter(|n| !checks.iter().any(|(k, _)| k == n)).collect();
        if !missing.is_empty() { v.hold.push(format!("auto_merge.required_checks 列的 check 沒有出現在 checks 裡（沒驗到）：{}", repr_strs(&missing))); }
        let (mut failed, mut skipped) = (Vec::new(), Vec::new());
        for (n, s) in checks {
            let st = strip(s).to_lowercase();
            if PASS_CHECK.contains(&st.as_str()) { continue; }
            if SKIP_CHECK.contains(&st.as_str()) {
                if !allowed.contains(n) { skipped.push(format!("{n}={s}")); }
                continue;
            }
            failed.push(format!("{n}={s}"));
        }
        if !failed.is_empty() { v.hold.push(format!("required checks 未通過：{}", repr_strs(&failed))); }
        if !skipped.is_empty() {
            v.hold.push(format!("required checks 被跳過或 neutral（不算通過；要放行請列進 auto_merge.allowed_skipped_checks）：{}", repr_strs(&skipped)));
        }
    }

    // 5. 留存的 important／blocker 發現（不確定就停：skeptic 推翻也不算數，要真的修掉、重審後不再出現）
    let sev: Vec<String> = inp.findings.iter().filter(|f| matches!(g(f, "severity").as_str(), Some("important" | "blocker")))
        .map(|f| format!("{}({})", str_of(g(f, "id")), str_of(g(f, "severity")))).collect();
    if !sev.is_empty() { v.hold.push(format!("有留存的 important／blocker 發現：{}", repr_strs(&sev))); }

    // 6. 測試數不減少
    if v.need(inp.tests_before.is_some(), "合併前測試數") && v.need(inp.tests_after.is_some(), "合併後測試數") {
        if let (Some(before), Some(after)) = (inp.tests_before, inp.tests_after) {
            if after < before { v.hold.push(format!("測試數減少：{before} → {after}")); }
        }
    }

    // 7. diff：新增 skip／only／fixme、風險等級
    let mut tier = "high";
    if let (true, Some(diff)) = (v.need(inp.diff.is_some(), "diff"), inp.diff.as_ref()) {
        if !diff.errors.is_empty() {
            v.hold.push(format!("diff 解析不了，無法確認改了哪些檔與新增了什麼（fail-closed）：{}", repr_strs(first(&diff.errors, 3))));
        }
        let show = |re: &FRegex| -> Vec<String> {
            diff.added_lines.iter().filter(|(_, l)| hit(re, l)).map(|(p, l)| format!("{p}: {}", head(strip(l), 80))).collect()
        };
        let skips = show(skip_re());
        if !skips.is_empty() { v.hold.push(format!("新增 skip／only／fixme：{}", repr_strs(first(&skips, 5)))); }
        let sup = show(suppress_re());
        if !sup.is_empty() { v.esc.push(format!("新增壓制註解（放寬 lint／型別／覆蓋率檢查，要人會簽）：{}", repr_strs(first(&sup, 5)))); }
        if diff.files.is_empty() { v.hold.push("diff 是空的".into()); }
        // 專案加的路徑分類（profile 的 diff_classes_extra）：寫壞了不能當成沒設定，交給人
        let classes = match recipe::diff_extra_of(inp.diff_extra.as_ref()) {
            Ok(c) => Some(c),
            Err(es) => {
                v.esc.push(format!("profile 的路徑分類設定讀不懂（{}），無法確認這次改動的分類", first(&es, 3).join("；")));
                None
            }
        };
        let (t, why) = risk_tier_with(diff, am, classes.as_ref())?;
        tier = if classes.is_none() { "high" } else { t };
        v.esc.extend(why);
        if let Some(r) = &inp.recipe {
            let paths = Value::Array(diff.files.iter().map(|(p, _)| json!(p)).collect());
            let short = recipe::diff_shortfalls_with(r, &paths, classes.as_ref()).map_err(|e| crash(&e.to_string()))?;
            let name = r.as_object().and_then(|o| o.get("name")).map(str_of).unwrap_or_else(|| "?".into());
            v.hold.extend(short.into_iter().map(|x| format!("recipe 不足（{name}）：{x}")));
        }
    }

    // 8. 白名單、風險上限、上游授權、每日上限
    let repos = g(am, "repos");
    if !truthy(g(am, "enabled")) { v.esc.push("auto_merge 未啟用（guard.config.json 的 auto_merge.enabled）".into()); }
    match repos.as_object().and_then(|o| o.get(repo)) {
        None => v.esc.push(format!("repo {repo} 不在自動合併白名單（auto_merge.repos）")),
        Some(cap) => {
            let cap_tier = cap.as_str().filter(|c| TIERS.contains(c)).unwrap_or("low");
            let rank = |t: &str| TIERS.iter().position(|x| *x == t).unwrap_or(0);
            if rank(tier) > rank(cap_tier) { v.esc.push(format!("風險等級 {tier} 超過 {repo} 的上限 {cap_tier}")); }
        }
    }
    if tier == "high" && !v.esc.iter().any(|e| e.contains("高風險") || e.contains("過大")) {
        v.esc.push("風險等級 high 一律交給人".into());
    }
    let day = judgment_day(&inp.timestamp);
    if list_of(g(cfg, "upstream_repos"))?.iter().any(|r| r == repo) {
        let (who, why) = upstream_auth(am, repo, &day);
        if who.is_none() { v.esc.push(format!("{repo} 是上游 repo，upstream_authorized_by 沒有有效的授權人：{why}")); }
    }
    if let Some(f) = inp.frozen.as_deref().filter(|f| !f.is_empty()) { v.hold.push(format!("自動合併已凍結（{f}；解凍由人處理）")); }
    if let Some(f) = inp.frozen_arg.as_deref().filter(|f| !f.is_empty()) { v.hold.push(format!("自動合併已凍結（--frozen：{f}；解凍由人處理）")); }
    let cap_raw = g(am, "daily_cap");
    let cap = if cap_raw.is_null() || cap_raw.as_str() == Some("") {
        DEFAULT_DAILY_CAP
    } else {
        match py_int(cap_raw) {
            Ok(n) => n,
            Err(IntErr::Bad) => {
                v.hold.push(format!("auto_merge.daily_cap 不是整數（{}），fail-closed", pyfmt::repr(cap_raw)));
                0
            }
        }
    };
    let mut merged_today = audit_lines.iter().filter(|a| {
        let ts = a.as_object().and_then(|o| o.get("timestamp")).map(str_of).unwrap_or_default();
        g(a, "decision").as_str() == Some("merge") && head(&ts, 10) == day
    }).count() as i128;
    if let Some(ext) = &inp.merged_today {
        match pyutil::int_of_str(ext).filter(|n| *n >= 0) {
            Some(n) => merged_today = merged_today.max(n),
            None => v.hold.push(format!("當日已合併數讀不懂（--merged-today {}），fail-closed", repr_str(ext))),
        }
    }
    if merged_today >= cap { v.hold.push(format!("超過每日合併上限：今天（{day}）已合併 {merged_today} 次，上限 {cap}")); }

    let decision = if !v.hold.is_empty() { "hold" } else if !v.esc.is_empty() { "escalate" } else { "merge" };
    let reasons: Vec<String> = if decision != "merge" {
        v.hold.iter().chain(&v.esc).cloned().collect()
    } else {
        std::iter::once("全部檢查通過".to_string()).chain(v.carried.iter().cloned()).collect()
    };
    // 稽核用：這次 gate 有沒有依 recipe 反查 diff、用的是哪個 recipe（resolved 的雜湊，事後可比對 run.json）
    let recipe_info = match &inp.recipe {
        Some(r) if r.is_object() => {
            let text = recipe::dumps(&sorted_keys(r), None);
            let hash: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            json!({"name": g(r, "name"), "sha256": hash})
        }
        _ => Value::Null,
    };
    Ok(json!({
        "decision": decision, "repo": repo, "pr": pr, "head_sha": head_sha, "reasons": reasons,
        "hold_reasons": v.hold, "escalations": v.esc, "followups": followups, "carried": v.carried,
        "risk_tier": tier, "requirements_source": inp.requirements_source,
        "timestamp": inp.timestamp, "gate_version": GATE_VERSION,
        "recipe": recipe_info,
    }))
}

// ------------------------------------------------------------------ 稽核紀錄

/// merge-audit.jsonl 已有的紀錄；讀不懂的行跳過，檔案讀不到當成沒有。
pub fn read_audit(path: &str) -> Result<Vec<Value>, Crash> {
    let text = match pyutil::read_text(path) {
        Ok(t) => t,
        Err(ReadErr::Os(_)) => return Ok(Vec::new()),
        Err(ReadErr::Decode(m)) => return Err(crash(&format!("UnicodeDecodeError：{path}：{m}"))),
    };
    let mut out = Vec::new();
    for line in pyutil::splitlines(&text) {
        match pyjson::loads_lenient(line) {
            Ok(rec) if rec.is_object() => out.push(rec),
            Ok(_) | Err(JsonErr::Decode(_)) => {}
            // 不能把「這裡解析不了、Python 讀得懂」的紀錄當成沒有（會少算當日合併數）
            Err(e) => return Err(crash(&format!("{path}：{e}"))),
        }
    }
    Ok(out)
}

fn append_audit(path: &str, result: &Value) -> Result<(), Crash> {
    use std::io::Write;
    let rec: Map<String, Value> = ["timestamp", "repo", "pr", "head_sha", "decision", "reasons", "risk_tier", "followups", "carried", "requirements_source", "gate_version"]
        .iter().filter_map(|k| result.get(k).map(|v| (k.to_string(), v.clone()))).collect();
    let line = format!("{}\n", recipe::dumps(&Value::Object(rec), None));
    let fail = |e: std::io::Error| crash(&format!("寫不進稽核紀錄 {path}：{e}"));
    std::fs::create_dir_all(pyutil::parent(path)).map_err(fail)?;
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path).map_err(fail)?;
    f.write_all(line.as_bytes()).map_err(fail)
}

// ------------------------------------------------------------------ recipe 的鎖定與竄改檢查（save-run 的私有複本）

/// save_run.load_profile_for：`--profile` 有給就用它，否則只認工具所在專案的 `<teams>/methodology.json`；找不到用 None（內建節）。
/// 讀不到、不是 JSON 物件、不符合 profile 的語意規則，回傳錯誤（fail-closed）。
fn load_profile_for(teams: &str, given: Option<&str>) -> (Value, String) {
    let path = match given {
        Some(p) => pyutil::path_str(p),
        None => pyutil::join(teams, "methodology.json"),
    };
    if !PathBuf::from(&path).is_file() {
        return (Value::Null, given.map(|p| format!("--profile {p} 不存在")).unwrap_or_default());
    }
    let text = match pyutil::read_text(&path) {
        Ok(t) => t,
        Err(e) => return (Value::Null, format!("讀不到開發規範 profile {path}：{}", e.msg())),
    };
    let prof = match pyjson::loads(&text) {
        Ok(p) => p,
        Err(e) => return (Value::Null, format!("讀不到開發規範 profile {path}：{e}")),
    };
    // 繼承（extends）：解析失敗＝profile 讀不懂（fail-closed）
    let prof = match crate::profile::resolve_str(&path, prof) {
        Ok(p) => p,
        Err(e) => return (Value::Null, format!("讀不到開發規範 profile {path}：{e}")),
    };
    if !prof.is_object() { return (Value::Null, format!("開發規範 profile {path} 不是 JSON 物件")); }
    let errs = schema::methodology_errors(&prof);
    let errs = if errs.is_empty() { errs } else { crate::profile::blame_path(std::path::Path::new(&path), errs) };
    if !errs.is_empty() { return (Value::Null, format!("開發規範 profile {path} 不合規則：{}", first(&errs, 3).join("；"))); }
    (prof, String::new())
}

/// save_run.recipe_tamper_errors：run.json 的 recipe.resolved 不能比「用專案 profile 依 name、modifiers、size 重算」的結果鬆。
fn recipe_tamper_errors(teams: &str, data: &Value, profile: Option<&str>) -> Result<Vec<String>, Crash> {
    let rec = g(data, "recipe");
    let res = g(rec, "resolved");
    if !rec.is_object() || !res.is_object() { return Ok(Vec::new()); }
    let (prof, perr) = load_profile_for(teams, profile);
    if !perr.is_empty() { return Ok(vec![perr]); }
    let py = |e: recipe::PyErr| crash(&e.to_string());
    let or = |v: &Value, default: Value| if truthy(v) { v.clone() } else { default };
    let base = recipe::resolve(&prof, g(rec, "name"), &or(g(rec, "modifiers"), json!([])), &or(g(rec, "size"), Value::Null)).map_err(py)?;
    let name = str_of(g(rec, "name"));
    let mut errs: Vec<String> = py_iter(g(&base, "errors"))?.iter().map(|e| format!("run.json 的 recipe 和重算結果不一致：{}", str_of(e))).collect();
    if !pyfmt::eq(g(&base, "name"), g(rec, "name")) {
        errs.push(format!("run.json 的 recipe {name} 用目前的 profile 解析成 {}（profile 改過？），不採信", str_of(g(&base, "name"))));
    }
    let mut loose = recipe::stricter_or_equal(res, &base).map_err(py)?;
    let base_kinds = py_iter(g(&base, "build_kinds"))?;
    let extra: Vec<Value> = py_iter(g(res, "build_kinds"))?.into_iter().filter(|k| !base_kinds.iter().any(|b| pyfmt::eq(b, k))).collect();
    if !extra.is_empty() {
        // escalate 時 build_kinds 照新 recipe，所以合法的 resolved 不會多出重算結果沒有的種類
        let names: Option<Vec<&str>> = extra.iter().map(Value::as_str).collect();
        let names = names.ok_or_else(|| crash("TypeError：build_kinds 的項目要是字串"))?;
        loose.push(format!("build_kinds 多了 {}", names.join("、")));
    }
    let flag = |v: &Value, k: &str| v.as_object().and_then(|o| o.get(k)).map(truthy).unwrap_or(true);
    if flag(res, "publishable") && !flag(&base, "publishable") { loose.push("publishable 被改成 true".into()); }
    if truthy(g(&base, "followup")) && !truthy(g(res, "followup")) { loose.push("followup 被拿掉".into()); }
    errs.extend(loose.into_iter().map(|e| format!("run.json 的 recipe.resolved 比 {name} 的定義鬆（手改過？）：{e}")));
    // 自訂段（profile 的 flow）：點名的段要還在 profile 裡、順序要照 profile（resolved 沒有自訂段時不做事）
    errs.extend(crate::stages::flow_tamper(&prof, g(res, "stages")));
    Ok(errs)
}

// ------------------------------------------------------------------ CLI

const OPTS: &[Opt] = &[
    opt("--decision", Kind::Str),
    req("--repo", Kind::Str),
    req("--pr", Kind::Int),
    req("--head-sha", Kind::Str),
    opt("--reviewed-sha", Kind::Append),
    opt("--base-sha", Kind::Str),
    opt("--git-dir", Kind::Str),
    opt("--evidence", Kind::Str),
    opt("--requirements", Kind::Str),
    opt("--requirements-source", Kind::Choice(&["command", "json", "openspec", "spec-md"])),
    opt("--requirements-change", Kind::Str),
    opt("--checks", Kind::Str),
    opt("--findings", Kind::Append),
    opt("--diff", Kind::Str),
    opt("--tests-before", Kind::Int),
    opt("--tests-after", Kind::Int),
    opt("--timestamp", Kind::Str),
    opt("--project-dir", Kind::Str),
    opt("--runs-dir", Kind::Str),
    opt("--no-audit", Kind::Flag),
    opt("--merged-today", Kind::Str),
    opt("--recipe", Kind::Str),
    opt("--frozen", Kind::Str),
];

struct Reader<'a> {
    ctx: &'a Ctx,
    stdin_used: Cell<bool>,
}

impl Reader<'_> {
    fn stdin(&self) -> String {
        if self.stdin_used.replace(true) { return String::new(); }
        match &self.ctx.stdin {
            Some(s) => s.clone(),
            None => {
                use std::io::Read;
                let mut bytes = Vec::new();
                let _ = std::io::stdin().read_to_end(&mut bytes);
                pyutil::text(&bytes)
            }
        }
    }

    /// 讀一個 JSON 輸入：`-` 是 stdin；不是 `{`／`[` 開頭的當成 agent 回報文字，從裡面取信封（取不到是 null）。
    fn json(&self, arg: &str) -> Result<Value, Fail> {
        let text = if arg == "-" { self.stdin() } else { pyutil::read_text(arg).map_err(|e| read_fail(e, |m| format!("讀不到 {arg}：{m}")))? };
        let t = strip(&text);
        if t.starts_with('{') || t.starts_with('[') {
            pyjson::loads(&text).map_err(|e| Fail::Input(format!("讀不到 {arg}：{e}")))
        } else {
            Ok(schema::extract_json(&text).unwrap_or(Value::Null))
        }
    }
}

/// `git diff $(merge-base base sha) sha | git patch-id --stable`；任何失敗回 None（呼叫端 fail-closed）。
fn patch_id(ctx: &Ctx, git_dir: &str, base: &str, sha: &str) -> Option<String> {
    let git = |args: &[&str], data: Option<Vec<u8>>| -> Option<Vec<u8>> {
        let mut c = Command::new("git");
        c.arg("-C").arg(git_dir).args(args);
        for (k, v) in &ctx.env { c.env(k, v); }
        // 0.x 的 Python 版沒給 stdin 時是繼承；這裡沒有要給它讀的東西，一律接空裝置
        let r = pyutil::run(&mut c, data, None).ok()?;
        if r.code == 0 { Some(r.stdout) } else { None }
    };
    let mb = String::from_utf8(git(&["merge-base", base, sha], None)?).ok()?;
    let diff = git(&["diff", strip(&mb), sha], None)?;
    if diff.iter().all(u8::is_ascii_whitespace) { return None; }
    let out = String::from_utf8(git(&["patch-id", "--stable"], Some(diff))?).ok()?;
    out.split(pyutil::is_py_space).find(|x| !x.is_empty()).map(str::to_string)
}

fn gather(ctx: &Ctx, a: &Parsed, cfg: &Value, proj: &str) -> Result<(Inp, Vec<String>), Crash> {
    let reader = Reader { ctx, stdin_used: Cell::new(false) };
    let mut inp = Inp {
        repo: a.get("--repo").unwrap_or("").to_string(),
        pr: a.int("--pr").unwrap_or(0),
        head_sha: a.get("--head-sha").unwrap_or("").to_string(),
        reviewed_shas: a.all("--reviewed-sha"),
        base_sha: a.get("--base-sha").map(str::to_string),
        tests_before: a.int("--tests-before"),
        tests_after: a.int("--tests-after"),
        findings_given: !a.all("--findings").is_empty(),
        merged_today: a.get("--merged-today").map(str::to_string),
        frozen_arg: a.get("--frozen").map(strip).filter(|f| !f.is_empty()).map(str::to_string),
        timestamp: a.truthy("--timestamp").map(str::to_string).unwrap_or_else(pyutil::now_iso_seconds),
        anti_cheat: Value::Array(Vec::new()),
        ..Inp::default()
    };
    let mut errors: Vec<String> = Vec::new();
    let mut attempt = |r: Result<(), Fail>| -> Result<(), Crash> {
        match r {
            Ok(()) => Ok(()),
            Err(Fail::Input(m)) => {
                errors.push(m);
                Ok(())
            }
            Err(Fail::Crash(m)) => Err(m),
        }
    };

    attempt((|| {
        if let Some(p) = a.truthy("--decision") { inp.decision = Some(reader.json(p)?).filter(|d| !d.is_null()); }
        Ok(())
    })())?;
    attempt((|| {
        let env = [("AWF_REPO", inp.repo.clone()), ("AWF_PR", inp.pr.to_string()), ("AWF_HEAD_SHA", inp.head_sha.clone())];
        let (reqs, source) = load_requirements(ctx, cfg, proj, a.get("--requirements"), a.get("--requirements-source"), a.get("--requirements-change"), &env)?;
        inp.requirements = Some(reqs);
        inp.requirements_source = source;
        Ok(())
    })())?;
    attempt((|| {
        if let Some(p) = a.truthy("--evidence") {
            let (ev, commit, anti) = normalize_evidence(&reader.json(p)?)?;
            (inp.evidence, inp.evidence_commit, inp.anti_cheat) = (Some(ev), commit, anti);
        }
        Ok(())
    })())?;
    attempt((|| {
        if let Some(p) = a.truthy("--checks") { inp.checks = Some(normalize_checks(&reader.json(p)?)?); }
        Ok(())
    })())?;
    attempt((|| {
        let mut all = Vec::new();
        for p in a.all("--findings") { all.extend(normalize_findings(&reader.json(&p)?)); }
        inp.findings = all;
        Ok(())
    })())?;
    attempt((|| {
        if let Some(p) = a.truthy("--diff") {
            let text = pyutil::read_text_lossy(p).map_err(|e| read_fail(e, |m| format!("讀不到 diff {p}：{m}")))?;
            inp.diff = Some(parse_diff(&text).map_err(Fail::Crash)?);
        }
        Ok(())
    })())?;
    // 專案加的路徑分類：只取 profile 的 diff_classes_extra 這一個鍵（profile 不存在、讀不懂或沒寫這個鍵＝沒設定，照舊）
    inp.diff_extra = pyutil::read_text(&pyutil::join(proj, ".claude/teams/methodology.json")).ok()
        .and_then(|t| pyjson::loads(&t).ok())
        .and_then(|p| crate::profile::resolve_str(&pyutil::join(proj, ".claude/teams/methodology.json"), p).ok())
        .and_then(|p| p.get("diff_classes_extra").filter(|v| !v.is_null()).cloned());
    attempt((|| {
        let Some(p) = a.truthy("--recipe") else { return Ok(()) };
        // 只接受 run.json：不接受呼叫端自備的 resolved（可以拿 feature 的定義蓋過 docs）；
        // resolved 要和用專案 profile 重算的結果一樣嚴，plan 要已核准（recipe 已鎖定）
        let data = reader.json(p)?;
        if !(g(&data, "stages").is_object() && g(&data, "recipe").is_object()) {
            return Err(Fail::Input(format!("--recipe {p} 不是 run.json（要有 stages 與 recipe.resolved；不接受單獨的 recipe JSON）")));
        }
        let r = g(g(&data, "recipe"), "resolved");
        if !(r.is_object() && g(r, "stages").is_array() && g(r, "build_kinds").is_array()) {
            return Err(Fail::Input(format!("--recipe {p} 的 recipe.resolved 格式不對")));
        }
        if !truthy(g(g(&data, "recipe"), "locked")) {
            return Err(Fail::Input(format!("--recipe {p} 的 recipe 還沒鎖定（plan 沒核准）")));
        }
        let meth = pyutil::join(proj, ".claude/teams/methodology.json");
        let teams = pyutil::resolve(&ctx.teams_of(proj).to_string_lossy());
        // run.json 已經有 recipe（plan 段用專案的 profile 解析、鎖定的）：這時專案的 profile 不見了，
        // 重算只會退回內建節，profile 定得更嚴的部分就漏掉了，所以不採信（0.x 這裡退回工具旁的 profile 或內建節）
        if !PathBuf::from(&meth).is_file() { return Err(Fail::Input(crate::save_run::profile_missing(&meth))); }
        let tamper = recipe_tamper_errors(&teams, &data, Some(meth.as_str())).map_err(Fail::Crash)?;
        if !tamper.is_empty() { return Err(Fail::Input(first(&tamper, 3).join("；"))); }
        // 自訂段（profile 的 flow）一定在 merge 之前：recipe 點名的每一個都要做完、由人核准（沒有自訂段時是空的）
        // 政策核准的紀錄只有現在的 profile 把那一段宣告成 run: steps＋gate: policy 才算數（profile 不合規則就都不算）
        let flow_prof = {
            let p = crate::layout::read_profile(&meth);
            if crate::stages::run_customs(&data).is_empty() || !crate::schema::methodology_errors(&p).is_empty() { Value::Null } else { p }
        };
        let flow_pending = crate::stages::flow_unapproved_with(&data, None, &flow_prof);
        if !flow_pending.is_empty() { return Err(Fail::Input(format!("--recipe {p} 的自訂段還沒過：{}", first(&flow_pending, 5).join("；")))); }
        // 還沒有人確認的暫定做法、被略過的子任務：都不是 agent 能自己合併的情況，交人（沒有這些紀錄時不多做事）
        let unconfirmed = crate::save_run::human::unconfirmed_of(&data);
        if !unconfirmed.is_empty() { return Err(Fail::Input(format!("--recipe {p} 還有 {} 項暫定做法沒有人確認過：{}", unconfirmed.len(), first(&unconfirmed, 3).join("；")))); }
        let skipped = crate::save_run::human::skipped_subtasks(&data);
        if !skipped.is_empty() { return Err(Fail::Input(format!("--recipe {p} 有子任務被略過（{}）：要不要合併由人決定，agent 不合併", skipped.join("、")))); }
        inp.recipe = Some(r.clone());
        if inp.requirements_source.starts_with("openspec:") {
            let (rc, _) = requirements_config(cfg, proj)?;
            let drift = openspec_baseline_errors(&rc, proj, a.get("--requirements").filter(|p| !p.is_empty()), a.get("--requirements-change"), &data)?;
            if !drift.is_empty() { return Err(Fail::Input(format!("規格和 run.json 核准時記下的基準不同：{}", first(&drift, 3).join("；")))); }
        }
        Ok(())
    })())?;

    if truthy(g(auto_merge(cfg), "carry_verdict_on_rebase")) {
        if let Some(base) = a.truthy("--base-sha") {
            let p = inp.decision.as_ref().map(|d| if g(d, "payload").is_object() { g(d, "payload") } else { d });
            let mut shas: Vec<String> = vec![inp.head_sha.clone()];
            shas.extend(inp.reviewed_shas.iter().cloned());
            shas.extend(inp.evidence_commit.as_ref().and_then(Value::as_str).map(str::to_string));
            shas.extend(p.and_then(|p| g(p, "head_sha").as_str()).map(str::to_string));
            let git_dir = a.truthy("--git-dir").map(pyutil::resolve).unwrap_or_else(|| proj.to_string());
            for s in shas.into_iter().filter(|s| !s.is_empty()) {
                if !inp.patch_ids.contains_key(&s) {
                    let id = patch_id(ctx, &git_dir, base, &s);
                    inp.patch_ids.insert(s, id);
                }
            }
        }
    }
    Ok((inp, errors))
}

fn exit_code(decision: &str) -> i32 {
    match decision {
        "merge" => 0,
        "escalate" => 3,
        _ => 1,
    }
}

fn cmd_check(ctx: &Ctx, a: &Parsed, out: &mut String) -> Result<i32, Crash> {
    let proj = project_dir(ctx, a.get("--project-dir"))?;
    let cfg = load_config(ctx, &proj);
    let audit = pyutil::join(&runs_dir(&proj, &cfg, a.get("--runs-dir"))?, "merge-audit.jsonl");
    let (mut inp, errors) = gather(ctx, a, &cfg, &proj)?;
    let ff = freeze_file(&proj, &cfg)?;
    if PathBuf::from(&ff).exists() {
        // 讀不懂也算凍結（fail-closed）
        let fz = pyutil::read_text(&ff).ok().and_then(|t| pyjson::loads(&t).ok()).unwrap_or_else(|| json!({}));
        if !(fz.is_object() && g(&fz, "frozen") == &Value::Bool(false)) {
            let reason = fz.as_object().and_then(|o| o.get("reason")).map(str_of).unwrap_or_default();
            inp.frozen = Some(format!("{ff}：{reason}"));
        }
    }
    let sch = Schemas::new(ctx.teams_of(&proj));
    let mut result = evaluate(&sch, &inp, &cfg, &read_audit(&audit)?)?;
    if !errors.is_empty() {
        // 輸入讀不到：一律 hold
        let mut reasons: Vec<Value> = errors.iter().map(|e| json!(format!("輸入錯誤：{e}"))).collect();
        reasons.extend(pyfmt::items(result.get("reasons")).iter().filter(|r| r.as_str() != Some("全部檢查通過")).cloned());
        result["decision"] = json!("hold");
        result["reasons"] = Value::Array(reasons);
    }
    if !a.flag("--no-audit") {
        append_audit(&audit, &result)?;
        result["audit"] = json!(audit);
    }
    out.push_str(&recipe::dumps(&result, Some(2)));
    out.push('\n');
    Ok(exit_code(result["decision"].as_str().unwrap_or("hold")))
}

/// CLI 的本體：stdout、stderr 的文字寫進 `out`、`errs`，回傳 exit code。
pub fn run(ctx: &Ctx, argv: &[String], out: &mut String, errs: &mut String) -> i32 {
    if argv.iter().any(|a| a == "--selftest") {
        let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "awf".into());
        // selftest 在暫存目錄跑，只借用 schema：固定的 teams → 專案的 → 目前目錄的（原本的行為）
        let teams = ctx.teams.clone().unwrap_or_else(|| ctx.teams_of(ctx.project_dir_env.as_deref().unwrap_or(".")));
        let fixed = Ctx { teams: Some(teams), project_dir_env: None, env: ctx.env.clone(), stdin: ctx.stdin.clone() };
        return selftest(&fixed, &json!([me, "merge-gate", "--selftest-requirements"]), out);
    }
    let parsed = pyargs::split_cmd(argv, &["check"]).and_then(|(_, rest, extras)| pyargs::parse(OPTS, rest, extras));
    let a = match parsed {
        Ok(a) => a,
        Err(ArgErr::Help) => {
            out.push_str(USAGE);
            return 0;
        }
        Err(e) => {
            let prog = if matches!(e, ArgErr::Sub(_)) { "awf merge-gate check" } else { "awf merge-gate" };
            errs.push_str(&format!("用法：awf merge-gate check --repo <owner/name> --pr <編號> --head-sha <SHA> [選項…]（awf merge-gate --help 看全部）\n{prog}: error: {}\n", e.message()));
            return 2;
        }
    };
    match cmd_check(ctx, &a, out) {
        Ok(code) => code,
        Err(m) => {
            errs.push_str(&format!("awf merge-gate：{m}\n"));
            1
        }
    }
}

/// `awf merge-gate …`
pub fn main(args: &[String]) -> i32 {
    // selftest 的「command 需求來源」案例用：印一份需求清單（對應 0.x 的 Python 版 selftest 臨時寫的 gen_reqs.py）
    if args.first().map(String::as_str) == Some("--selftest-requirements") {
        let repo = std::env::var("AWF_REPO").unwrap_or_default();
        println!("{}", json!({"requirements": [{"id": "ISSUE-7", "text": "登入頁顯示錯誤訊息", "source": format!("issue #7 of {repo}")}]}));
        return 0;
    }
    let (mut out, mut errs) = (String::new(), String::new());
    let code = run(&Ctx::real(), args, &mut out, &mut errs);
    print!("{out}");
    eprint!("{errs}");
    code
}
