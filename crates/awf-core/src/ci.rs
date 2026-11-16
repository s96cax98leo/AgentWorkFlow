//! 專案的測試／lint CI：依 profile 的 `ci` 產生 GitHub Actions 的 workflow 檔。**預設不啟用。**
//!
//! 模版只管工作流程：跑哪些檢查、指令是什麼、哪些必過，都由使用者在 `.claude/teams/methodology.json` 的 `ci` 決定
//! （`awf init` 的訪談會從 package.json、Makefile、Cargo.toml 等偵測既有的測試／lint 指令當預設）。
//!
//! ## profile 的 `ci`
//!
//! | 鍵 | 意思 |
//! |---|---|
//! | `enabled` | 預設 false。沒開時 `awf ci render` 印「未啟用」、exit 0，不寫任何檔；doctor 不檢查 |
//! | `provider` | 目前只支援 `github` |
//! | `name` | workflow 名稱，也是檔名（`.github/workflows/<name>.yml`）。預設 `project-ci`；不能用模版自己的 `agent-merge-gate` |
//! | `jobs` | `[{id, name, run, required}]`：`run` 是使用者定的指令；`required: true` 的 job 名稱會成為 merge gate 的 required check |
//! | `triggers` | `pull_request`、`push`、`workflow_dispatch`；預設 `["pull_request"]` |
//! | `runs_on` | runner 標籤（字串或字串陣列）；預設 `ubuntu-latest` |
//!
//! ## 產生的檔
//!
//! - 檔頭兩行：「由 awf ci render 產生」的標記，與內容的 sha256（`# awf-ci-hash: sha256:<hex>`，算的是這一行之後的全部內容）。
//! - 第三方 action 只有 checkout，釘在 commit SHA；`persist-credentials: false`。
//! - 最小權限：`permissions: contents: read`。不放任何 secrets；`run` 裡不能寫 GitHub 的運算式（語意規則擋）。
//! - 只用 `pull_request`（不是 `pull_request_target`），fork 來的 PR 拿不到 secrets。
//!
//! ## 寫入
//!
//! `awf ci render` **預設只印不寫**；`--write` 才寫，而且只有主 session 或人能跑（team agent 由 hook 擋）。
//! 既有的同名檔不是這支工具產生的（沒有標記）就拒絕覆寫。寫完把 `required: true` 的 job 名稱併進
//! `guard.config.json` 的 `auto_merge.required_checks`（只加不刪；`awf merge-gate` 會要求這些 check 都出現在 checks 裡）。
//!
//! ## 漂移
//!
//! `awf ci check` 與 `awf doctor`：檔不存在、標記的雜湊和內容不符（被手改）、內容和現在的 profile 產生的不同，都提示並附修法。

use crate::merge_gate::pyjson;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const PROFILE_REL: &str = ".claude/teams/methodology.json";
pub const GUARD_REL: &str = ".claude/teams/guard.config.json";
pub const DEFAULT_NAME: &str = "project-ci";
pub const DEFAULT_RUNS_ON: &str = "ubuntu-latest";
pub const DEFAULT_TRIGGERS: &[&str] = &["pull_request"];
/// 模版自己的 workflow 檔名：使用者的 CI 不能和它同名。
pub const RESERVED_NAMES: &[&str] = &["agent-merge-gate"];
pub const PROVIDERS: &[&str] = &["github"];
pub const TRIGGERS: &[&str] = &["pull_request", "push", "workflow_dispatch"];
/// checkout 釘在 commit SHA（和 merge gate 的 CI 範本用同一個版本）。
pub const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1";
pub const MARK: &str = "# 由 awf ci render 產生";
pub const HASH_PREFIX: &str = "# awf-ci-hash: sha256:";
/// guard.config.json 裡 merge gate 讀的必過 check 清單。
pub const REQUIRED_KEY: &str = "required_checks";

/// （exit code，stdout，stderr）。
pub type Captured = (i32, String, String);

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub run: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub name: String,
    pub jobs: Vec<Job>,
    pub triggers: Vec<String>,
    pub runs_on: Vec<String>,
}

fn name_re() -> Regex {
    Regex::new("^[a-z0-9][a-z0-9-]{0,63}$").expect("固定的正規式")
}

fn id_re() -> Regex {
    Regex::new("^[a-z][a-z0-9_-]{0,39}$").expect("固定的正規式")
}

fn label_re() -> Regex {
    Regex::new("^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").expect("固定的正規式")
}

/// GitHub Actions 的運算式起頭（`$` 接兩個左大括號）。指令裡出現就拒絕：運算式會在 shell 執行前展開，是注入與洩漏 secrets 的入口。
fn expr_open() -> String {
    format!("${}{}", '{', '{')
}

fn runs_on_of(v: Option<&Value>) -> Result<Vec<String>, String> {
    let items: Vec<&Value> = match v {
        None | Some(Value::Null) => return Ok(vec![DEFAULT_RUNS_ON.to_string()]),
        Some(Value::String(_)) => v.into_iter().collect(),
        Some(Value::Array(a)) if !a.is_empty() => a.iter().collect(),
        Some(_) => return Err("ci.runs_on 要是 runner 標籤（字串），或非空的字串陣列".to_string()),
    };
    let re = label_re();
    let mut out = Vec::new();
    for it in items {
        match it.as_str() {
            Some(s) if re.is_match(s) => out.push(s.to_string()),
            _ => return Err("ci.runs_on 的標籤只能是英數字、點、底線、連字號（不能是運算式）".to_string()),
        }
    }
    Ok(out)
}

/// 不能進 yml 的字元：YAML 當成換行的 U+0085、U+2028、U+2029，其他控制字元（`\n`、`\t` 由呼叫端決定收不收），
/// 以及看不見的格式字元（零寬、雙向控制、BOM）。它們能讓一段文字在產生的 yml 裡跳出原本的位置。
fn yaml_break(c: char) -> bool {
    matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}') || crate::hooks::safe::invisible(c)
}

const BREAK_MSG: &str = "不能有 Unicode 的行分隔或看不見的字元（U+0085、U+2028、U+2029、零寬與雙向控制字元；YAML 會把行分隔當成換行）";

/// profile 的 `ci` 的語意規則（schema 表達不了的）。回錯誤清單；空的＝可以用。
pub fn config_errors(ci: &Value) -> Vec<String> {
    let Some(o) = ci.as_object() else { return vec!["ci 要是物件".to_string()] };
    let mut errs = Vec::new();
    if o.get("enabled").is_some_and(|v| !v.is_boolean()) {
        errs.push("ci.enabled 要是 true 或 false".to_string());
    }
    if let Some(p) = o.get("provider") {
        if !p.as_str().is_some_and(|s| PROVIDERS.contains(&s)) {
            errs.push(format!("ci.provider 目前只支援 {}", PROVIDERS.join("、")));
        }
    }
    if let Some(n) = o.get("name") {
        match n.as_str() {
            Some(s) if RESERVED_NAMES.contains(&s) => errs.push(format!("ci.name 不能是 {s}（模版自己的 workflow 檔名）")),
            Some(s) if name_re().is_match(s) => {}
            _ => errs.push("ci.name 只能是小寫英數字與連字號（它也是 .github/workflows/ 底下的檔名）".to_string()),
        }
    }
    if let Err(e) = runs_on_of(o.get("runs_on")) {
        errs.push(e);
    }
    if let Some(t) = o.get("triggers") {
        match t.as_array() {
            Some(a) if !a.is_empty() => {
                let mut seen: Vec<&str> = Vec::new();
                for x in a {
                    match x.as_str() {
                        Some(s) if TRIGGERS.contains(&s) => {
                            if seen.contains(&s) {
                                errs.push(format!("ci.triggers 重複：{s}"));
                            }
                            seen.push(s);
                        }
                        _ => errs.push(format!("ci.triggers 只能是 {}", TRIGGERS.join("、"))),
                    }
                }
            }
            _ => errs.push("ci.triggers 要是非空的陣列".to_string()),
        }
    }
    let jobs = match o.get("jobs") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.iter().collect(),
        Some(_) => {
            errs.push("ci.jobs 要是陣列".to_string());
            Vec::new()
        }
    };
    let (mut ids, mut names): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let expr = expr_open();
    for (i, j) in jobs.iter().enumerate() {
        let n = i + 1;
        let Some(j) = j.as_object() else {
            errs.push(format!("ci.jobs 第 {n} 項要是物件"));
            continue;
        };
        let id = j.get("id").and_then(Value::as_str).unwrap_or_default();
        if !id_re().is_match(id) {
            errs.push(format!("ci.jobs 第 {n} 項的 id 要是小寫英文字母開頭的英數字、底線、連字號"));
        } else if ids.iter().any(|x| x == id) {
            errs.push(format!("ci.jobs 的 id 重複：{id}"));
        }
        ids.push(id.to_string());
        let name = j.get("name").and_then(Value::as_str).unwrap_or(id).trim().to_string();
        if name.chars().any(yaml_break) {
            errs.push(format!("ci.jobs 第 {n} 項的 name {BREAK_MSG}"));
        } else if name.is_empty() || name.chars().any(char::is_control) {
            errs.push(format!("ci.jobs 第 {n} 項的 name 不能是空的或含換行"));
        } else if name.contains(&expr) {
            errs.push(format!("ci.jobs 第 {n} 項的 name 不能寫 GitHub Actions 的運算式"));
        } else if names.contains(&name) {
            errs.push(format!("ci.jobs 的 name 重複：{name}（required check 靠名稱對，重複會分不出來）"));
        }
        names.push(name);
        match j.get("run").and_then(Value::as_str) {
            Some(r) if !r.trim().is_empty() => {
                if r.contains(&expr) {
                    errs.push(format!("ci.jobs 第 {n} 項的 run 不能寫 GitHub Actions 的運算式（含 secrets）；需要的值請在指令裡用環境變數"));
                }
                if r.chars().any(yaml_break) {
                    errs.push(format!("ci.jobs 第 {n} 項的 run {BREAK_MSG}"));
                } else if r.chars().any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t') {
                    errs.push(format!("ci.jobs 第 {n} 項的 run 有控制字元"));
                }
            }
            _ => errs.push(format!("ci.jobs 第 {n} 項的 run 不能是空的（要跑什麼指令由你決定）")),
        }
        if j.get("required").is_some_and(|v| !v.is_boolean()) {
            errs.push(format!("ci.jobs 第 {n} 項的 required 要是 true 或 false"));
        }
    }
    if o.get("enabled") == Some(&Value::Bool(true)) && jobs.is_empty() {
        errs.push("ci.enabled 是 true 時 ci.jobs 至少要有一項".to_string());
    }
    errs
}

/// 解析 profile 的 `ci`。沒有這個鍵回 `Ok(None)`；有錯回第一批錯誤。
pub fn parse_config(profile: &Value) -> Result<Option<Config>, String> {
    let Some(ci) = profile.get("ci").filter(|v| !v.is_null()) else { return Ok(None) };
    let errs = config_errors(ci);
    if !errs.is_empty() {
        return Err(errs.join("；"));
    }
    let s = |v: Option<&Value>, d: &str| v.and_then(Value::as_str).unwrap_or(d).to_string();
    let jobs = ci.get("jobs").and_then(Value::as_array).map(|a| {
        a.iter().map(|j| {
            let id = s(j.get("id"), "");
            Job {
                name: s(j.get("name"), &id).trim().to_string(),
                run: s(j.get("run"), "").replace("\r\n", "\n").replace('\r', "\n"),
                required: j.get("required") == Some(&Value::Bool(true)),
                id,
            }
        }).collect()
    }).unwrap_or_default();
    let triggers = match ci.get("triggers").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => DEFAULT_TRIGGERS.iter().map(|x| x.to_string()).collect(),
    };
    Ok(Some(Config {
        enabled: ci.get("enabled") == Some(&Value::Bool(true)),
        name: s(ci.get("name"), DEFAULT_NAME),
        jobs,
        triggers,
        runs_on: runs_on_of(ci.get("runs_on"))?,
    }))
}

/// 產生的檔放哪（相對專案根目錄）。
pub fn file_rel(cfg: &Config) -> String {
    format!(".github/workflows/{}.yml", cfg.name)
}

/// 必過的 check 名稱（`required: true` 的 job 的 name）。
pub fn required_names(cfg: &Config) -> Vec<String> {
    cfg.jobs.iter().filter(|j| j.required).map(|j| j.name.clone()).collect()
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// YAML 的雙引號字串（JSON 的字串寫法在 YAML 裡同義）。
fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn body_of(cfg: &Config) -> String {
    let mut b = String::new();
    b.push_str(&format!("name: {}\n", quoted(&cfg.name)));
    b.push_str("on:\n");
    for t in TRIGGERS.iter().filter(|t| cfg.triggers.iter().any(|x| x == *t)) {
        b.push_str(&format!("  {t}:\n"));
    }
    b.push_str("permissions:\n  contents: read\n");
    b.push_str("jobs:\n");
    let runs_on = match cfg.runs_on.as_slice() {
        [one] => one.clone(),
        many => format!("[{}]", many.join(", ")),
    };
    for j in &cfg.jobs {
        b.push_str(&format!("  {}:\n    name: {}\n    runs-on: {runs_on}\n    steps:\n", j.id, quoted(&j.name)));
        b.push_str(&format!("      - uses: {CHECKOUT}\n        with:\n          persist-credentials: false\n"));
        b.push_str(&format!("      - name: {}\n        run: |\n", quoted(&j.name)));
        for line in j.run.trim().split('\n') {
            let line = line.trim_end();
            if line.is_empty() {
                b.push('\n');
            } else {
                b.push_str(&format!("          {line}\n"));
            }
        }
    }
    b
}

/// 整份 workflow 檔的內容（檔頭的標記與雜湊＋本體）。同一份設定每次產生的內容相同。
pub fn render(cfg: &Config) -> String {
    let body = body_of(cfg);
    format!("{MARK}（來源：{PROFILE_REL} 的 ci）。不要手改：改 profile 後重跑 awf ci render --write。\n{HASH_PREFIX}{}\n{body}", sha256_hex(&body))
}

/// 檔案和設定的關係。
#[derive(Debug, Clone, PartialEq)]
pub enum Drift {
    /// 和現在的設定產生的內容相同。
    Same,
    /// 還沒產生。
    Missing,
    /// 同名檔不是這支工具產生的（沒有標記）。
    NotOurs,
    /// 標記的雜湊和內容不符：產生之後被手改過。
    Edited,
    /// 內容完整，但和現在的 profile 產生的不同（profile 改了還沒重新產生）。
    Stale,
    /// 讀不了。
    Unreadable(String),
}

/// 比對既有的檔與設定。
pub fn drift_of(existing: Option<&str>, cfg: &Config) -> Drift {
    let Some(text) = existing else { return Drift::Missing };
    let text = text.replace("\r\n", "\n");
    let mut lines = text.splitn(3, '\n');
    let (l1, l2, body) = (lines.next().unwrap_or_default(), lines.next().unwrap_or_default(), lines.next().unwrap_or_default());
    if !l1.starts_with(MARK) {
        return Drift::NotOurs;
    }
    match l2.strip_prefix(HASH_PREFIX) {
        Some(h) if h.trim() == sha256_hex(body) => {}
        _ => return Drift::Edited,
    }
    if text == render(cfg) { Drift::Same } else { Drift::Stale }
}

fn read_opt(p: &Path) -> Result<Option<String>, String> {
    match std::fs::read(p) {
        Ok(b) => String::from_utf8(b).map(Some).map_err(|_| "不是 UTF-8".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 專案裡那份檔和設定的關係。
pub fn drift_in(root: &Path, cfg: &Config) -> Drift {
    match read_opt(&root.join(file_rel(cfg))) {
        Ok(t) => drift_of(t.as_deref(), cfg),
        Err(e) => Drift::Unreadable(e),
    }
}

/// 給人看的一句話與修法；一致時回 None。
pub fn drift_message(d: &Drift, rel: &str) -> Option<(String, String)> {
    let rerun = "在專案根目錄由主 session 或人執行：awf ci render --write".to_string();
    match d {
        Drift::Same => None,
        Drift::Missing => Some((format!("ci 已啟用，但還沒產生 {rel}"), rerun)),
        Drift::NotOurs => Some((format!("{rel} 不是 awf ci render 產生的（沒有標記），不會覆寫"), "把 profile 的 ci.name 改成別的名字，或自己處理那個檔後再跑 awf ci render --write".to_string())),
        Drift::Edited => Some((format!("{rel} 產生之後被手改過（檔頭的雜湊和內容不符）"), format!("要改檢查內容請改 {PROFILE_REL} 的 ci，再{rerun}（手改的部分會被換掉）"))),
        Drift::Stale => Some((format!("{rel} 和現在 profile 的 ci 產生的內容不同（profile 改了還沒重新產生）"), rerun)),
        Drift::Unreadable(e) => Some((format!("{rel} 讀不了：{e}"), "確認檔案權限與編碼".to_string())),
    }
}

/// 讀 profile。回 `Ok(None)`＝沒有 profile 或沒有 `ci`。
pub fn load_config(profile_path: &Path) -> Result<Option<Config>, String> {
    let Some(text) = read_opt(profile_path).map_err(|e| format!("讀不了 {}：{e}", profile_path.display()))? else { return Ok(None) };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let doc = pyjson::loads(text).map_err(|e| format!("{} 不是合法 JSON：{e}", profile_path.display()))?;
    parse_config(&doc)
}

/// 把必過的 check 名稱併進 guard.config.json 的 `auto_merge.required_checks`（只加不刪）。回這次新增的名稱。
pub fn merge_required(guard_path: &Path, names: &[String]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let Some(text) = read_opt(guard_path).map_err(|e| format!("讀不了 {}：{e}", guard_path.display()))? else {
        return Err(format!("找不到 {}（專案還沒 init？）", guard_path.display()));
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(Value::Object(mut doc)) = pyjson::loads(text) else {
        return Err(format!("{} 不是合法的 JSON 物件", guard_path.display()));
    };
    let am = doc.entry("auto_merge").or_insert_with(|| json!({}));
    let Some(am) = am.as_object_mut() else { return Err("guard.config.json 的 auto_merge 不是物件".to_string()) };
    let list = am.entry(REQUIRED_KEY).or_insert_with(|| json!([]));
    let Some(list) = list.as_array_mut() else { return Err(format!("guard.config.json 的 auto_merge.{REQUIRED_KEY} 不是陣列")) };
    let mut added = Vec::new();
    for n in names {
        if !list.iter().any(|x| x.as_str() == Some(n)) {
            list.push(json!(n));
            added.push(n.clone());
        }
    }
    if !added.is_empty() {
        let out = format!("{}\n", crate::recipe::dumps(&Value::Object(doc), Some(2)));
        std::fs::write(guard_path, out).map_err(|e| format!("寫不進 {}：{e}", guard_path.display()))?;
    }
    Ok(added)
}

pub const USAGE: &str = "專案的測試／lint CI：依 .claude/teams/methodology.json 的 ci 產生 GitHub Actions 的 workflow 檔。
預設不啟用（ci.enabled 是 true 才有作用）；跑哪些指令、哪些必過都由專案在 profile 決定（見 docs/自訂與擴充.md〈專案的 CI〉）。

用法：
    awf ci render [--write] [--root <專案根目錄>] [--profile <檔>]
    awf ci check [--root <專案根目錄>] [--profile <檔>]
    awf ci --selftest

  render    印出會產生的 workflow 檔（stdout）；預設只印不寫
  --write   寫到 .github/workflows/<ci.name>.yml，並把 required: true 的 job 名稱併進 guard.config.json 的
            auto_merge.required_checks（只加不刪）。只由主 session 或人執行；同名檔不是這支工具產生的就拒絕覆寫
  check     比對既有的檔與 profile：一致或沒啟用 exit 0；還沒產生、被手改、和 profile 不同 exit 1
產生的檔：action 釘在 commit SHA、權限只有 contents: read、不放 secrets；檔頭有產生標記與內容雜湊。
設定有錯 exit 2。
";

struct Cli {
    verb: String,
    write: bool,
    root: Option<String>,
    profile: Option<String>,
}

fn parse_cli(argv: &[String]) -> Result<Cli, String> {
    let mut c = Cli { verb: String::new(), write: false, root: None, profile: None };
    let mut i = 0;
    while let Some(a) = argv.get(i) {
        i += 1;
        match a.as_str() {
            "--write" => c.write = true,
            "--root" | "--profile" => {
                let Some(v) = argv.get(i) else { return Err(format!("{a} 後面要接值")) };
                i += 1;
                if a == "--root" { c.root = Some(v.clone()) } else { c.profile = Some(v.clone()) }
            }
            s if s.starts_with('-') => return Err(format!("不認得的選項：{s}")),
            s if c.verb.is_empty() => c.verb = s.to_string(),
            s => return Err(format!("多了一個參數：{s}")),
        }
    }
    Ok(c)
}

/// 指令本體：`root` 是預設的專案根目錄。
pub fn run_captured(argv: &[String], root: &Path) -> Captured {
    if argv.is_empty() || argv.iter().any(|a| a == "-h" || a == "--help") {
        return (0, USAGE.to_string(), String::new());
    }
    let cli = match parse_cli(argv) {
        Ok(c) => c,
        Err(e) => return (2, String::new(), format!("awf ci：{e}\n\n{USAGE}")),
    };
    let root: PathBuf = cli.root.as_deref().map(PathBuf::from).unwrap_or_else(|| root.to_path_buf());
    let profile = cli.profile.as_deref().map(PathBuf::from).unwrap_or_else(|| root.join(PROFILE_REL));
    if cli.verb != "render" && cli.verb != "check" {
        return (2, String::new(), format!("awf ci：不認得的子指令「{}」\n\n{USAGE}", cli.verb));
    }
    if cli.write && cli.verb != "render" {
        return (2, String::new(), "awf ci：--write 只能搭配 render\n".to_string());
    }
    let cfg = match load_config(&profile) {
        Ok(Some(c)) if c.enabled => c,
        Ok(_) => return (0, format!("未啟用：{} 的 ci.enabled 不是 true（沒有產生或檢查任何檔）\n", profile.display()), String::new()),
        Err(e) => return (2, String::new(), format!("awf ci：設定有錯：{e}\n")),
    };
    let rel = file_rel(&cfg);
    let drift = drift_in(&root, &cfg);
    if cli.verb == "check" {
        return match drift_message(&drift, &rel) {
            None => (0, format!("一致：{rel} 和 profile 的 ci 相同\n"), String::new()),
            Some((msg, fix)) => (1, format!("{msg}\n修法：{fix}\n"), String::new()),
        };
    }
    let text = render(&cfg);
    let required = required_names(&cfg);
    if !cli.write {
        let mut note = format!("（只印不寫）會寫到 {rel}；加 --write 才寫。");
        if !required.is_empty() {
            note.push_str(&format!("必過的 check（--write 時併進 guard.config.json 的 auto_merge.{REQUIRED_KEY}）：{}", required.join("、")));
        }
        return (0, text, format!("{note}\n"));
    }
    if matches!(drift, Drift::NotOurs) {
        let (msg, fix) = drift_message(&drift, &rel).unwrap_or_default();
        return (1, String::new(), format!("awf ci：{msg}\n修法：{fix}\n"));
    }
    if let Drift::Unreadable(e) = &drift {
        return (1, String::new(), format!("awf ci：{rel} 讀不了：{e}\n"));
    }
    let path = root.join(&rel);
    if let Some(up) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(up) {
            return (1, String::new(), format!("awf ci：建不了目錄 {}：{e}\n", up.display()));
        }
    }
    if drift != Drift::Same {
        if let Err(e) = std::fs::write(&path, &text) {
            return (1, String::new(), format!("awf ci：寫不進 {}：{e}\n", path.display()));
        }
    }
    let mut out = match drift {
        Drift::Same => format!("沒有變動：{rel}\n"),
        Drift::Missing => format!("已產生：{rel}\n"),
        _ => format!("已更新：{rel}\n"),
    };
    let mut err = String::new();
    match merge_required(&root.join(GUARD_REL), &required) {
        Ok(added) if !added.is_empty() => out.push_str(&format!("guard.config.json 的 auto_merge.{REQUIRED_KEY} 新增：{}（只加不刪；不再必過的名稱要自己拿掉）\n", added.join("、"))),
        Ok(_) => {}
        Err(e) => err.push_str(&format!("警告：必過的 check 沒有併進 guard.config.json（{e}）；請自己把 {} 加進 auto_merge.{REQUIRED_KEY}\n", required.join("、"))),
    }
    out.push_str("這個檔要 commit 並進受保護的 base 分支才會生效；要讓它成為必過的 check，還要在 GitHub 的 ruleset 把這些 job 名稱設成 required。\n");
    (0, out, err)
}

/// `awf ci …`
pub fn main(argv: &[String]) -> i32 {
    if argv.first().map(String::as_str) == Some("--selftest") {
        return selftest();
    }
    let root = PathBuf::from(crate::deliverables::default_root());
    let (code, out, err) = run_captured(argv, &root);
    print!("{out}");
    eprint!("{err}");
    code
}

// ------------------------------------------------------------------ selftest

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// 把每個行為走一遍（暫存目錄）。回 exit code。
pub fn selftest() -> i32 {
    let base = std::env::temp_dir().join(format!("awf-ci-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (mut fails, mut total): (Vec<String>, usize) = (Vec::new(), 0);
    let mut n = 0usize;
    let mut proj = |ci: Option<Value>, guard: Option<&str>| -> PathBuf {
        n += 1;
        let d = base.join(format!("p{n}"));
        let _ = std::fs::create_dir_all(d.join(".claude/teams"));
        if let Some(c) = ci {
            let _ = std::fs::write(d.join(PROFILE_REL), format!("{}\n", json!({"name": "selftest", "ci": c})));
        }
        if let Some(g) = guard {
            let _ = std::fs::write(d.join(GUARD_REL), g);
        }
        d
    };
    let mut check = |name: &str, ok: bool, detail: String| {
        total += 1;
        if !ok {
            fails.push(format!("{name}：{detail}"));
        }
    };
    let two = || json!({"enabled": true, "provider": "github", "jobs": [
        {"id": "test", "name": "測試", "run": "npm ci\nnpm test", "required": true},
        {"id": "lint", "run": "npm run lint"}
    ]});
    let run = |a: &[&str], d: &Path| run_captured(&args(a), d);

    // 1. 沒有 profile、沒有 ci、enabled=false：未啟用、exit 0、不寫檔
    for (label, ci) in [("沒有 profile", None), ("enabled=false", Some(json!({"enabled": false, "jobs": [{"id": "t", "run": "make test"}]}))), ("沒寫 enabled", Some(json!({"jobs": []})))] {
        let d = proj(ci, None);
        let (code, out, _) = run(&["render", "--write"], &d);
        check(&format!("未啟用（{label}）"), code == 0 && out.starts_with("未啟用") && !d.join(".github").exists(), format!("{code} {out}"));
    }

    // 2. 預設只印不寫
    let d = proj(Some(two()), Some("{\n  \"auto_merge\": {\"enabled\": false}\n}\n"));
    let (code, out, err) = run(&["render"], &d);
    check("render 只印不寫", code == 0 && out.starts_with(MARK) && !d.join(".github").exists() && err.contains("只印不寫"), format!("{code} {err}"));
    let hash_ok = out.split('\n').nth(1).and_then(|l| l.strip_prefix(HASH_PREFIX)).is_some_and(|h| h == sha256_hex(out.splitn(3, '\n').nth(2).unwrap_or_default()));
    check("檔頭的雜湊＝本體的 sha256", hash_ok, out.clone());
    check("action 釘 SHA、最小權限、不存憑證", out.contains(CHECKOUT) && out.contains("permissions:\n  contents: read\n") && out.contains("persist-credentials: false") && !out.contains("secrets") && !out.contains("pull_request_target"), out.clone());
    check("uses 都釘在 40 位的 SHA", out.lines().filter(|l| l.trim_start().starts_with("- uses:")).all(|l| Regex::new("@[0-9a-f]{40} ").is_ok_and(|r| r.is_match(l))), out.clone());
    check("多行指令", out.contains("        run: |\n          npm ci\n          npm test\n"), out.clone());
    check("沒寫 name 用 id、預設觸發與 runner", out.contains("  lint:\n    name: \"lint\"\n    runs-on: ubuntu-latest\n") && out.contains("on:\n  pull_request:\npermissions:"), out.clone());
    check("可重現", run(&["render"], &d).1 == out, String::new());

    // 3. --write：寫檔、required 併進 guard.config（只加不刪）、重跑沒有變動
    let (code, wout, werr) = run(&["render", "--write"], &d);
    let written = std::fs::read_to_string(d.join(".github/workflows/project-ci.yml")).unwrap_or_default();
    check("--write 寫檔", code == 0 && written == out && wout.contains("已產生") && werr.is_empty(), format!("{code} {wout} {werr}"));
    let guard = std::fs::read_to_string(d.join(GUARD_REL)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_default();
    check("required 併進 guard.config", guard["auto_merge"][REQUIRED_KEY] == json!(["測試"]) && guard["auto_merge"]["enabled"] == json!(false), guard.to_string());
    let (code, wout, _) = run(&["render", "--write"], &d);
    check("重跑沒有變動", code == 0 && wout.contains("沒有變動") && !wout.contains("新增"), wout);
    check("check 一致", run(&["check"], &d).0 == 0, String::new());

    // 4. 只加不刪：既有的名稱留著
    let d2 = proj(Some(two()), Some("{\"auto_merge\": {\"required_checks\": [\"build\"]}}"));
    let _ = run(&["render", "--write"], &d2);
    let guard = std::fs::read_to_string(d2.join(GUARD_REL)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_default();
    check("只加不刪", guard["auto_merge"][REQUIRED_KEY] == json!(["build", "測試"]), guard.to_string());

    // 5. 漂移：手改、profile 改了、不是我們的檔
    let yml = d.join(".github/workflows/project-ci.yml");
    let _ = std::fs::write(&yml, format!("{written}# 手改\n"));
    let (code, cout, _) = run(&["check"], &d);
    check("手改＝漂移", code == 1 && cout.contains("手改") && cout.contains("awf ci render --write"), cout);
    let _ = run(&["render", "--write"], &d);
    check("--write 修好手改", std::fs::read_to_string(&yml).unwrap_or_default() == written, String::new());
    let mut changed = two();
    changed["jobs"][1]["run"] = json!("npm run lint -- --max-warnings 0");
    let _ = std::fs::write(d.join(PROFILE_REL), json!({"name": "selftest", "ci": changed}).to_string());
    let (code, cout, _) = run(&["check"], &d);
    check("profile 改了＝漂移", code == 1 && cout.contains("不同"), cout);
    let d3 = proj(Some(two()), None);
    let _ = std::fs::create_dir_all(d3.join(".github/workflows"));
    let _ = std::fs::write(d3.join(".github/workflows/project-ci.yml"), "name: mine\n");
    let (code, _, werr) = run(&["render", "--write"], &d3);
    check("不覆寫別人的檔", code == 1 && werr.contains("不會覆寫") && std::fs::read_to_string(d3.join(".github/workflows/project-ci.yml")).unwrap_or_default() == "name: mine\n", werr);
    let d4 = proj(Some(two()), None);
    let (code, cout, _) = run(&["check"], &d4);
    check("還沒產生", code == 1 && cout.contains("還沒產生"), cout);
    let (code, wout, werr) = run(&["render", "--write"], &d4);
    check("沒有 guard.config：檔照寫、只警告", code == 0 && wout.contains("已產生") && werr.contains("警告"), format!("{wout}{werr}"));

    // 6. 語意規則
    let expr = expr_open();
    let bad: Vec<(&str, Value, &str)> = vec![
        ("id 重複", json!({"jobs": [{"id": "a", "run": "x"}, {"id": "a", "name": "b", "run": "y"}]}), "id 重複"),
        ("run 空的", json!({"jobs": [{"id": "a", "run": "  "}]}), "run 不能是空的"),
        ("沒有 run", json!({"jobs": [{"id": "a"}]}), "run 不能是空的"),
        ("run 有運算式", json!({"jobs": [{"id": "a", "run": format!("echo {expr} secrets.TOKEN }}}}")}]}), "運算式"),
        ("name 重複", json!({"jobs": [{"id": "a", "name": "n", "run": "x"}, {"id": "b", "name": "n", "run": "y"}]}), "name 重複"),
        ("provider", json!({"provider": "gitlab"}), "只支援"),
        ("保留的檔名", json!({"name": "agent-merge-gate"}), "模版自己的"),
        ("檔名跳出目錄", json!({"name": "../x"}), "ci.name"),
        ("觸發", json!({"triggers": ["pull_request_target"]}), "ci.triggers"),
        ("runner 是運算式", json!({"runs_on": format!("{expr} x }}}}")}), "ci.runs_on"),
        ("啟用但沒有 job", json!({"enabled": true}), "至少要有一項"),
        ("run 有 U+2028（YAML 當換行，能在 run 區塊外接出新的鍵）", json!({"jobs": [{"id": "a", "run": "npm test\u{2028}permissions: write-all\u{2028}on: pull_request_target"}]}), "行分隔"),
        ("run 有 U+2029", json!({"jobs": [{"id": "a", "run": "npm test\u{2029}    env:"}]}), "行分隔"),
        ("run 有 U+0085", json!({"jobs": [{"id": "a", "run": "npm test\u{85}x: y"}]}), "行分隔"),
        ("run 有看不見的字元", json!({"jobs": [{"id": "a", "run": "npm\u{202e} test"}]}), "行分隔"),
        ("name 有 U+2028", json!({"jobs": [{"id": "a", "name": "測試\u{2028}permissions: write-all", "run": "x"}]}), "行分隔"),
        ("name 有 U+0085", json!({"jobs": [{"id": "a", "name": "測試\u{85}x", "run": "x"}]}), "name"),
        ("required 型別", json!({"jobs": [{"id": "a", "run": "x", "required": "yes"}]}), "required 要是"),
    ];
    for (label, ci, want) in bad {
        let errs = config_errors(&ci);
        check(&format!("語意：{label}"), errs.iter().any(|e| e.contains(want)), errs.join("；"));
    }
    check("語意：正常的沒有錯", config_errors(&two()).is_empty(), config_errors(&two()).join("；"));
    let d5 = proj(Some(json!({"enabled": true, "jobs": [{"id": "a", "run": ""}]})), None);
    let (code, _, cerr) = run(&["render"], &d5);
    check("設定有錯 exit 2", code == 2 && cerr.contains("設定有錯"), cerr);
    check("不認得的選項 exit 2", run(&["render", "--wri"], &d).0 == 2 && run(&["check", "--write"], &d).0 == 2, String::new());

    // 7. 觸發與 runner
    let d6 = proj(Some(json!({"enabled": true, "name": "acme-checks", "triggers": ["push", "pull_request"], "runs_on": ["self-hosted", "linux"], "jobs": [{"id": "t", "run": "make test"}]})), None);
    let (_, out6, err6) = run(&["render"], &d6);
    check("觸發順序固定、runner 陣列、檔名", out6.contains("on:\n  pull_request:\n  push:\n") && out6.contains("runs-on: [self-hosted, linux]") && err6.contains(".github/workflows/acme-checks.yml"), format!("{out6}{err6}"));

    let _ = std::fs::remove_dir_all(&base);
    if fails.is_empty() {
        println!("ci selftest：{total} 項全過");
        0
    } else {
        for f in &fails {
            eprintln!("  失敗 {f}");
        }
        eprintln!("ci selftest：{} / {total} 項失敗", fails.len());
        1
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        assert_eq!(super::selftest(), 0);
    }
}
