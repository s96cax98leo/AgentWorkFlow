//! 確定性推送工具。原本是 0.x 的 Python 工具 `publish.py`：每一個拒絕條件、exit code、訊息文字、
//! 稽核紀錄的內容都和 0.x 的 Python 版相同（對跑基準在 `tests/parity/publish.expected.json`）。
//!
//! 無人值守時由主 session 呼叫這支推分支、開 PR（不交給 agent）。它自己把關：
//!
//! 1. 目標只能是 guard.config.json 的 fork_repo；remote 的 push URL 必須指向 fork，指向上游或禁止的 repo 一律拒絕。
//! 2. 分支名要符合 allowed_branch_prefixes，不能是 protected_branches 或 PR 的 base。不 force push。
//! 3. 推送前掃 diff 新增的行（常見金鑰樣式、私鑰區塊、機敏檔名、`--secrets-file` 列的值）。命中只報檔名與樣式名稱，不印值。
//!    專案的 profile 啟用了去客戶化掃描、而且 `scrub.when` 含 `publish` 時，接著掃這次要推的檔（`scrub_gate`）；命中或掃不了都不推。預設不啟用。
//!    diff 用 `merge_gate::parse_diff` 解析；diff 解析不了一律拒絕。
//! 4. 依政策判定能不能推（和 save_run 共用同一套判定）：run 已取消、閘門鏈不完整、recipe 不能推送、政策核准的段重算後有違規，都拒絕。
//! 5. diff 形狀和 recipe 要相符（`recipe::diff_shortfalls`）。
//! 6. `git push <remote> refs/heads/<分支>:refs/heads/<分支>`，再 `gh pr create --repo <fork>`。
//!    有 `--supersedes N` 時 PR 說明末尾加一行「取代 #N」；舊 PR 由人關閉。
//! 7. 每次執行（含拒絕、dry-run）附加一行到 `<runs_dir>/publish-audit.jsonl`。
//!
//! 結束碼：0＝已推送（或 dry-run 檢查全過）；1＝拒絕或推送／開 PR 失敗；2＝參數錯誤。
//!
//! 和 0.x 的 Python 版的差別（都不放寬判定；對跑測試的 `STRICTER`、`KNOWN` 逐項列名）：
//! - 讀進來的 JSON（run.json、各段的結果檔、profile）有 NaN、Infinity、落單的代理字元跳脫、超出 64 位元範圍的整數、
//!   巢狀超過 200 層時一律當成讀不懂（拒絕）。
//! - 0.x 的 Python 版丟出沒接住的例外的地方（traceback、exit 1、不寫稽核紀錄），這裡印「awf publish：類別：訊息」、exit 1，同樣不推送。
//! - 專案根目錄：沒給 `--project-dir` 時用 `CLAUDE_PROJECT_DIR`，沒有就從目前目錄往上找最外層的 `.claude/teams/VERSION`
//!   （[`crate::project`]；0.x 的 Python 版是工具檔所在的專案）。找不到就拒絕，不退回目前目錄。
//! - `--supersedes` 只收 ASCII 數字、而且要在 64 位元範圍內。
//! - 用法訊息的指令名是 `awf publish`。

use crate::hooks::py::lower;
use crate::hooks::re::{group, Re};
use crate::kb::pyish;
use crate::merge_gate::pyargs::{self, opt as o, req, ArgErr, Kind, Opt};
use crate::merge_gate::pyutil::{self, int_of_str, splitlines, strip, tail};
use crate::merge_gate::parse_diff;
use crate::pyfmt::{self, repr_str};
use crate::save_run::py::{self, contains, get, get_opt, idx, truthy};
use crate::save_run::{self, Ctx, PyErr};
use crate::{recipe, schema, scrub};
use serde_json::{json, Map, Value};
use std::process::Command;

pub const USAGE: &str = "確定性推送工具：推功能分支到 fork 並開 PR

用法（在已 init 的專案根目錄）：
  awf publish push --repo-dir <目標 repo 目錄> --branch feature/x \\
      --run <run_dir> --title \"<PR 標題>\" --body-file <PR 說明檔> \\
      [--base main] [--remote origin] [--stage deliver] [--draft] [--no-pr] [--dry-run] \\
      [--secrets-file <機敏值清單，一行一個>] [--supersedes <舊 PR 編號>] [--project-dir <專案根>] [--timestamp <ISO 8601>]
  awf publish --selftest     # 用假的 git push 與 gh，不連網、不真的推送

結束碼：0＝已推送（或 dry-run 檢查全過）；1＝拒絕或推送／開 PR 失敗；2＝參數錯誤。
";

pub const DEFAULT_PREFIXES: [&str; 5] = ["feature/", "bugfix/", "hotfix/", "release/", "merge/"];
pub const DEFAULT_PROTECTED: [&str; 2] = ["main", "master"];
/// 推送是 deliver 之後的動作：閘門鏈固定要求到 deliver。
pub const MIN_STAGE: &str = "deliver";

static PLACEHOLDER: Re = Re::new(r"\{\{[A-Z_]+\}\}", "");
static SLUG_IN_URL: Re = Re::new(r"[:/]([\w.-]+/[\w.-]+?)(?:\.git)?/?$", "");
static FORBIDDEN_SLUG: Re = Re::new(r"^[\w.-]+/[\w.-]+$", "");

/// 常見金鑰樣式（只回報名稱，不印值）。樣式照 0.x 的 Python 版原樣，由 `hooks::re` 對齊 `\s`、`\w`、`\b`、`$` 的差異。
static SECRET_PATTERNS: [(&str, Re); 12] = [
    ("AWS access key", Re::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "")),
    ("GitHub token", Re::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{60,}\b", "")),
    ("GitLab token", Re::new(r"\bglpat-[A-Za-z0-9_-]{20,}\b", "")),
    ("Slack token", Re::new(r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b", "")),
    ("Slack webhook", Re::new(r"https://hooks\.slack\.com/services/[A-Za-z0-9/]{20,}", "")),
    ("Google API key", Re::new(r"\bAIza[0-9A-Za-z_-]{35}\b", "")),
    ("Stripe live key", Re::new(r"\b[sr]k_live_[0-9A-Za-z]{20,}\b", "")),
    ("OpenAI／Anthropic 類 API key", Re::new(r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{32,}\b", "")),
    ("私鑰區塊", Re::new(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----", "")),
    ("JWT", Re::new(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b", "")),
    ("連線字串內的密碼", Re::new(r"\b[a-z][a-z0-9+.-]*://[^\s:/@]+:[^\s@/]{6,}@[\w.-]+", "")),
    (
        "疑似寫死的密碼或金鑰",
        Re::new(r#"\b(?:password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|private[_-]?key)\b["']?\s*[:=]\s*["'][^"'\s]{12,}["']"#, "i"),
    ),
];
static SECRET_FILES: Re =
    Re::new(r"(?:^|/)(?:\.env(?:\.[\w-]+)?|id_(?:rsa|dsa|ecdsa|ed25519)|[^/]+\.(?:pem|p12|pfx|key|keystore|jks)|\.npmrc|\.pypirc|credentials(?:\.json)?)$", "");
static SECRET_FILE_OK: Re = Re::new(r"\.env\.(?:example|sample|template)$", "");

/// 停下來的兩種：`Refused` 對應 0.x 的 Python 版的 Refused（記進稽核紀錄、exit 1）；
/// `Crash` 是 0.x 的 Python 版丟出沒接住的例外的地方（這裡印訊息、exit 1，不寫稽核紀錄、不推送）。
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    Refused(String),
    Crash(String),
}

impl From<PyErr> for Stop {
    fn from(e: PyErr) -> Self {
        Stop::Crash(format!("{}：{}", e.class, e.msg))
    }
}

type R<T> = Result<T, Stop>;

fn refused<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop::Refused(msg.into()))
}

/// 執行環境：測試用它換掉專案根目錄、git／gh 指令與子程序的環境變數。
#[derive(Debug, Clone)]
pub struct Env {
    /// save_run 的環境（schema 所在的 teams 目錄、重算 recipe 用的專案 profile、相對路徑的基準）。
    pub ctx: Ctx,
    /// 沒給 `--project-dir` 時的專案根目錄（[`crate::project`] 的解析結果）。
    pub project: String,
    /// git 指令（selftest 換成會記錄 push 的替身）。
    pub git: Vec<String>,
    /// gh 指令。
    pub gh: Vec<String>,
    /// 另外加給子程序（git、gh）的環境變數。
    pub env: Vec<(String, String)>,
}

impl Env {
    /// 命令列用（selftest、說明這類不判定的路徑）：專案根目錄照 [`crate::project`] 的解析，找不到就用目前目錄。
    pub fn real() -> Env {
        let ctx = Ctx::from_env();
        Env { project: ctx.pack_root.clone(), ctx, git: vec!["git".into()], gh: vec!["gh".into()], env: Vec::new() }
    }

    /// 命令列用（推送）：專案根目錄是 `explicit`（`--project-dir`）→ `CLAUDE_PROJECT_DIR` → 從目前目錄往上找最外層的
    /// `.claude/teams/VERSION`；找不到回錯誤，不退回目前目錄。設定、schema、重算 recipe 用的 profile 都跟著同一個專案根。
    pub fn for_gate(explicit: Option<&str>) -> Result<Env, String> {
        let ctx = Ctx::for_gate(explicit)?;
        Ok(Env { project: ctx.pack_root.clone(), ctx, git: vec!["git".into()], gh: vec!["gh".into()], env: Vec::new() })
    }
}

/// `push` 的參數（已經照 argparse 的規則解析過）。
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub repo_dir: String,
    pub branch: String,
    pub run: Option<String>,
    pub stage: String,
    pub base: String,
    pub remote: String,
    pub title: Option<String>,
    pub body_file: Option<String>,
    pub draft: bool,
    pub no_pr: bool,
    pub dry_run: bool,
    pub secrets_file: Option<String>,
    pub supersedes: Option<i64>,
    pub project_dir: Option<String>,
    pub timestamp: Option<String>,
    /// 人當下確認過「這個 run 有子任務被略過」（`--confirm-skipped-subtasks`）；沒有略過時用不到。
    pub confirm_skipped: bool,
}

/// 真值判斷後的字串（`if a.x:`）：沒給或空字串都是 None。
fn some(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

// ------------------------------------------------------------------ 設定

/// 整理過的 guard.config.json。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub fork_repo: String,
    pub upstream_repos: Vec<String>,
    pub forbidden: Vec<String>,
    pub prefixes: Vec<String>,
    pub protected: Vec<String>,
    pub runs_dir: String,
}

impl Config {
    /// 和 0.x 的 Python 版 load_config 回傳的 dict 同形狀（對跑用）。
    pub fn to_value(&self) -> Value {
        json!({"fork_repo": self.fork_repo, "upstream_repos": self.upstream_repos, "forbidden": self.forbidden, "prefixes": self.prefixes, "protected": self.protected, "runs_dir": self.runs_dir})
    }
}

/// `_list(v, default)`：字串用逗號切；只留非空、沒有佔位符的字串；空的用預設。
/// 值是數字之類不能逐項走的東西時 Python 丟 TypeError。
pub fn list_of(v: Option<&Value>, default: &[&str]) -> Result<Vec<String>, PyErr> {
    let items: Vec<Value> = match v {
        Some(Value::String(s)) => s.split(',').map(|x| Value::String(strip(x).to_string())).collect(),
        Some(other) => py::iter_or(other)?,
        None => Vec::new(),
    };
    let out: Vec<String> = items.iter().filter_map(Value::as_str).filter(|x| !strip(x).is_empty() && !PLACEHOLDER.is_match(x)).map(|x| strip(x).to_string()).collect();
    Ok(if out.is_empty() { default.iter().map(|x| x.to_string()).collect() } else { out })
}

/// `str(v or 預設)`
fn str_or(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(x) if truthy(x) => pyfmt::str_of(x),
        _ => default.to_string(),
    }
}

/// 讀 `<專案>/.claude/teams/guard.config.json`（沒有檔案當成空的）。
pub fn load_config(proj: &str) -> Result<Config, PyErr> {
    let p = pyish::pjoin(proj, ".claude/teams/guard.config.json");
    let cfg = if std::path::Path::new(&p).exists() { schema::Schemas::new(pyish::pjoin(proj, ".claude/teams")).config(Some(std::path::Path::new(&p))) } else { Map::new() };
    let runs = str_or(cfg.get("runs_dir"), "_tpm/runs");
    let runs = strip(&runs).trim_matches('/');
    Ok(Config {
        fork_repo: strip(&str_or(cfg.get("fork_repo"), "")).to_string(),
        upstream_repos: list_of(cfg.get("upstream_repos"), &[])?,
        forbidden: list_of(cfg.get("forbidden_paths"), &[])?.into_iter().filter(|x| FORBIDDEN_SLUG.is_match(x)).collect(),
        prefixes: list_of(cfg.get("allowed_branch_prefixes"), &DEFAULT_PREFIXES)?,
        protected: list_of(cfg.get("protected_branches"), &DEFAULT_PROTECTED)?,
        runs_dir: if runs.is_empty() { "_tpm/runs".to_string() } else { runs.to_string() },
    })
}

// ------------------------------------------------------------------ git／gh

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 跑一個子程序（`text=True, errors="replace"`，stdin 接空裝置）。跑不起來時 Python 丟 OSError（沒接）。
fn run_cmd(env: &Env, argv: &[String], cwd: Option<&str>) -> R<Out> {
    let prog = argv.first().map(String::as_str).unwrap_or_default();
    let mut c = Command::new(prog);
    c.args(argv.get(1..).unwrap_or_default());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    for (k, v) in &env.env {
        c.env(k, v);
    }
    c.env("PYTHONUTF8", "1").env("GIT_TERMINAL_PROMPT", "0");
    match pyutil::run(&mut c, None, None) {
        Ok(r) => Ok(Out { code: r.code, stdout: pyutil::text(&r.stdout), stderr: pyutil::text(&r.stderr) }),
        Err(pyutil::RunErr::Os(e)) => Err(PyErr::os(&e, prog).into()),
        Err(pyutil::RunErr::Timeout) => Err(Stop::Crash(format!("TimeoutExpired：{prog} 逾時"))),
    }
}

fn git(env: &Env, repo: &str, args: &[&str]) -> R<Out> {
    let mut argv = env.git.clone();
    argv.push("-C".into());
    argv.push(repo.to_string());
    argv.extend(args.iter().map(|x| x.to_string()));
    run_cmd(env, &argv, None)
}

/// URL 裡的 `owner/name`（小寫）；認不出來回空字串。
pub fn slug_of(url: &str) -> String {
    let t = strip(url);
    SLUG_IN_URL.captures(t).and_then(|c| group(t, &c, 1).map(lower)).unwrap_or_default()
}

// ------------------------------------------------------------------ 檢查

/// 推送目標與分支名。
pub fn check_target(cfg: &Config, branch: &str, base: &str) -> Result<(), String> {
    let fork = &cfg.fork_repo;
    if fork.is_empty() {
        return Err("guard.config.json 沒有 fork_repo，不知道能推到哪裡".into());
    }
    if cfg.upstream_repos.iter().chain(&cfg.forbidden).any(|s| lower(s) == lower(fork)) {
        return Err(format!("fork_repo {fork} 同時列在 upstream_repos 或 forbidden_paths"));
    }
    if branch.is_empty() || ["-", "+", "refs/"].iter().any(|p| branch.starts_with(p)) || branch.contains(':') || branch.contains("..") {
        return Err(format!("分支名不合法：{}（只寫分支名，例如 feature/x）", repr_str(branch)));
    }
    if cfg.protected.iter().any(|p| p == branch) || branch == base {
        return Err(format!("{branch} 是受保護分支或 PR 的 base，不直接推送"));
    }
    if !cfg.prefixes.iter().any(|p| branch.starts_with(p.as_str())) {
        return Err(format!("分支名 {branch} 不在允許的前綴（{}）", cfg.prefixes.join("、")));
    }
    Ok(())
}

/// remote 的每個 push URL 都要指向 fork。
fn check_remote(env: &Env, repo: &str, remote: &str, cfg: &Config) -> R<()> {
    let r = git(env, repo, &["remote", "get-url", "--all", "--push", remote])?;
    let urls = if r.code == 0 { crate::hooks::py::split_ws(&r.stdout) } else { Vec::new() };
    check_urls(&urls, repo, remote, cfg).map_err(Stop::Refused)
}

/// `check_remote` 的判定部分（urls 是 `git remote get-url --all --push` 的結果）。
pub fn check_urls(urls: &[String], repo: &str, remote: &str, cfg: &Config) -> Result<(), String> {
    if urls.is_empty() {
        return Err(format!("{repo} 沒有 remote {remote}"));
    }
    let fork = lower(&cfg.fork_repo);
    for url in urls {
        let s = slug_of(url);
        for up in cfg.upstream_repos.iter().chain(&cfg.forbidden) {
            if lower(up) == s || lower(url).contains(&lower(up)) {
                return Err(format!("remote {remote} 指向上游或禁止的 repo {up}"));
            }
        }
        if s != fork {
            return Err(format!("remote {remote} 的 push URL 指向 {}，不是 fork_repo {}", if s.is_empty() { url } else { &s }, cfg.fork_repo));
        }
    }
    Ok(())
}

/// 機敏檔名（`.env.example` 這類範本不算）。
pub fn secret_file(path: &str) -> bool {
    !path.is_empty() && path != "/dev/null" && SECRET_FILES.is_match(path) && !SECRET_FILE_OK.is_match(path)
}

/// 回傳命中的「檔名：樣式名稱」；不回傳值本身。看新增的行與留下來的檔名（新增、修改、改名或複製後的路徑）。
///
/// diff 用 `merge_gate::parse_diff` 解析（和 gate 同一套）；機敏檔名從每個 diff --git 區塊比對，
/// 所以沒有 ---／+++ 行的二進位檔與空檔也會命中。`names` 是 `git diff --name-only --diff-filter=ACMRT` 的結果，再比對一次。
/// diff 解析不了就當成命中（無法確認沒有機敏值，fail-closed）。Err 是 0.x 的 Python 版在 parse_diff 丟例外的地方。
pub fn scan_secrets(diff: &str, literals: &[String], names: &[String]) -> Result<Vec<String>, String> {
    scan_secrets_with(diff, literals, names, &crate::config::SecretExtra::default())
}

/// 同 [`scan_secrets`]，另外比對專案加的機敏樣式（guard.config.json 的 `secret_files_extra`、`secret_patterns_extra`；只會多命中）。
/// 設定用不了（`extra.errors` 非空）也當成命中：無法確認沒有機敏值。
pub fn scan_secrets_with(diff: &str, literals: &[String], names: &[String], extra: &crate::config::SecretExtra) -> Result<Vec<String>, String> {
    let parsed = parse_diff(diff)?;
    let mut hits: Vec<String> = parsed.errors.iter().map(|e| format!("diff：{e}，無法確認沒有機敏值")).collect();
    let kept = parsed.entries.iter().filter(|e| !e.deleted).filter_map(|e| e.new.as_deref()).filter(|p| !p.is_empty());
    hits.extend(extra.errors.iter().map(|e| format!("guard.config.json：{e}，無法確認沒有機敏值")));
    hits.extend(kept.chain(names.iter().map(String::as_str)).filter(|p| secret_file(p) || (*p != "/dev/null" && extra.file_hit(p))).map(|p| format!("{p}：機敏檔名")));
    for (cur, body) in &parsed.added_lines {
        for n in extra.line_hits(body) {
            hits.push(format!("{cur}：專案設定的機敏樣式（secret_patterns_extra 第 {n} 條）"));
        }
        for (name, rx) in &SECRET_PATTERNS {
            if rx.is_match(body) {
                hits.push(format!("{cur}：{name}"));
            }
        }
        if literals.iter().any(|v| !v.is_empty() && body.contains(v.as_str())) {
            hits.push(format!("{cur}：--secrets-file 列的機敏值"));
        }
    }
    hits.sort();
    hits.dedup();
    Ok(hits)
}

/// `json.loads(path.read_text())`，照 0.x 的 Python 版的 `except (OSError, json.JSONDecodeError)`：
/// 讀不到、不是看得懂的 JSON 回 Err(訊息)；不是 UTF-8、巢狀太深是沒接住的例外。
fn load_json(ctx: &Ctx, path: &str) -> R<Result<Value, String>> {
    match ctx.load_json(path) {
        Ok(v) => Ok(Ok(v)),
        Err(e) if matches!(e.class.as_str(), "UnicodeDecodeError" | "RecursionError") => Err(e.into()),
        Err(e) => Ok(Err(e.msg)),
    }
}

/// 回傳核准紀錄的摘要；不能推就回 Refused。
///
/// 固定要求整條閘門鏈到 deliver（含）都 done、有閘門的都已核准；`--stage` 只能指定 deliver 或更後面的段（更嚴），
/// 給更前面的段、或不是 SDLC 的段一律拒絕（不能用 `--stage` 降低要求）。`run_dir` 是已經 resolve 過的路徑。
pub fn check_policy(ctx: &Ctx, run_dir: Option<&str>, stage: &str) -> R<String> {
    let sdlc = save_run::SDLC;
    let min = sdlc.iter().position(|x| *x == MIN_STAGE).unwrap_or(0);
    let pos = match sdlc.iter().position(|x| *x == stage) {
        Some(p) if p >= min => p,
        _ => return refused(format!("--stage {} 不行：推送前至少要 {MIN_STAGE} 段核准，--stage 只能是 {}", repr_str(stage), sdlc.get(min..).unwrap_or_default().join("、"))),
    };
    let Some(run_dir) = run_dir else { return refused("沒有 --run：推送前要有 run.json 的閘門紀錄") };
    let f = pyish::pjoin(run_dir, "run.json");
    let data = match load_json(ctx, &f)? {
        Ok(v) => v,
        Err(e) => return refused(format!("讀不到 {f}：{e}")),
    };
    if truthy(get(&data, "abandoned")?) {
        return refused(format!("{}，不推送", save_run::abandoned_error(&data)?));
    }
    let empty = json!({});
    let stages = match get(&data, "stages")? {
        v if v.is_object() => v,
        _ => &empty,
    };
    let tamper = save_run::recipe_tamper_errors(ctx, &data, "")?;
    if !tamper.is_empty() {
        return refused(tamper.iter().take(3).cloned().collect::<Vec<_>>().join("；"));
    }
    let r = recipe::recipe_of_run(&data).map_err(py::from_recipe)?;
    let no_stages = json!([]);
    let want = match get(&r, "stages")? {
        v if truthy(v) => v,
        _ => &no_stages,
    };
    let name = py::s(get(&r, "name")?);
    if !get_opt(&r, "publishable")?.map_or(true, truthy) {
        return refused(format!("recipe {name} 不能推送（publishable=false；spike 拒推，要落地請另開 run）"));
    }
    // 自訂段（profile 的 flow）一定在 merge 之前：recipe 點名的每一個都要做完、由人核准才能推（含排在 deliver 後面的）。
    // 沒有自訂段時是空的。chain_errors 也會查排在 --stage 之前的；這裡另外擋「deliver 之後、merge 之前」的與政策核准的紀錄
    // 政策核准的自訂段：只有現在的 profile 把它宣告成 run: steps＋gate: policy 才算數（沒有自訂段時不讀 profile）
    let customs = crate::stages::run_customs(&data);
    let flow_prof = if customs.is_empty() {
        Value::Null
    } else {
        let (p, perr) = save_run::load_profile(ctx, "");
        if perr.is_empty() { p } else { Value::Null }
    };
    let flow_pending = crate::stages::flow_unapproved_with(&data, Some(crate::stages::FLOW_NO_ANCHOR), &flow_prof);
    if !flow_pending.is_empty() {
        return refused(format!("recipe {name} 的自訂段還沒過：{}", flow_pending.iter().take(5).cloned().collect::<Vec<_>>().join("；")));
    }
    let ends: Vec<&str> = if stage == MIN_STAGE { vec![MIN_STAGE] } else { vec![MIN_STAGE, stage] };
    for s in &ends {
        if !contains(want, s)? {
            return refused(format!("recipe {name} 沒有 {s} 段，不能推送"));
        }
    }
    // recipe 裡有閘門的段（與 deliver、--stage）一段都不能少；chain_errors 只略過沒有 recipe 的舊 run 沒有的 sync
    let mut missing: Vec<&str> = Vec::new();
    for s in sdlc.iter().take(pos + 1) {
        if contains(want, s)? && (save_run::GATED.contains(s) || ends.contains(s)) && !get(stages, s)?.is_object() {
            missing.push(s);
        }
    }
    if !missing.is_empty() {
        return refused(format!("run.json 沒有 {} 這{}段", missing.join("、"), if missing.len() > 1 { "幾" } else { "一" }));
    }
    for s in &ends {
        let st = idx(stages, s)?;
        let status = get(st, "status")?;
        if status.as_str() != Some("done") {
            return refused(format!("{s} 狀態是 {}，不是 done", py::s(status)));
        }
        if (*s == MIN_STAGE || truthy(get(st, "gate")?)) && !truthy(get(st, "approved")?) {
            return refused(format!("{s} 還沒核准（awf save-run approve）"));
        }
    }
    let appr = match idx(idx(stages, MIN_STAGE)?, "approved")? {
        v if v.is_object() => v,
        _ => &empty,
    };
    let chain = save_run::chain_errors_with(&data, stage, &flow_prof)?;
    // 政策核准的自訂段：重跑同一套政策判定（含由工具重跑步驟的指令；run.json 的紀錄不採信）
    for c in &customs {
        let a2 = stages.get(c.as_str()).and_then(|s| s.get("approved"));
        let by_policy = a2.is_some_and(|a| a.get("by").and_then(Value::as_str) == Some("policy") || a.get("kind").and_then(Value::as_str) == Some("policy"));
        if !by_policy {
            continue;
        }
        let result = match load_json(ctx, &pyish::pjoin(run_dir, &format!("{c}.json")))? {
            Ok(v) => v,
            Err(e) => return refused(format!("政策核准要讀 {c}.json，讀不到：{e}")),
        };
        let errs = save_run::policy_violations(ctx, c, &result, Some(run_dir))?;
        if !errs.is_empty() {
            return refused(format!("自訂段 {c} 政策判定不通過：{}", errs.iter().take(5).cloned().collect::<Vec<_>>().join("；")));
        }
    }
    if !chain.is_empty() {
        return refused(format!("recipe 閘門鏈（{name}）不完整：{}", chain.iter().take(5).cloned().collect::<Vec<_>>().join("；")));
    }
    let spec_ctx = {
        let (prof, _) = save_run::load_profile(ctx, "");
        if prof.is_object() { save_run::open_spec(ctx, run_dir, &data, &prof) } else { None }
    };
    // 到 stage 為止凡是政策核准的段，都重跑同一套政策判定（run.json 可能被手改，不信任紀錄本身）
    for s in sdlc.iter().take(pos + 1) {
        if !contains(want, s)? || !get(stages, s)?.is_object() {
            continue;
        }
        let a2 = match get(idx(stages, s)?, "approved")? {
            v if truthy(v) => v,
            _ => &empty,
        };
        if !a2.is_object() {
            return refused(format!("{s} 段的核准紀錄格式不對（{}），不採信", pyfmt::repr(a2)));
        }
        if get(a2, "by")?.as_str() != Some("policy") && get(a2, "kind")?.as_str() != Some("policy") {
            continue;
        }
        let result = match load_json(ctx, &pyish::pjoin(run_dir, &format!("{s}.json")))? {
            Ok(v) => v,
            Err(e) => return refused(format!("政策核准要讀 {s}.json，讀不到：{e}")),
        };
        let errs = save_run::policy_violations(ctx, s, &result, Some(run_dir))?;
        if !errs.is_empty() {
            return refused(format!("{s} 段政策判定不通過：{}", errs.iter().take(5).cloned().collect::<Vec<_>>().join("；")));
        }
        // 需求來源是 openspec 時（沒選就是 None，什麼都不做）：規格檢查也重算，含「和核准時記下的基準比」
        if let Some(sc) = &spec_ctx {
            let si = save_run::spec_issues(s, &result, sc, &data)?;
            if !si.is_empty() {
                return refused(format!("{s} 段規格檢查不通過：{}", si.iter().take(5).cloned().collect::<Vec<_>>().join("；")));
            }
        }
    }
    let tail_text = if stage != MIN_STAGE { format!("；要求到 {stage} 段，已 done") } else { String::new() };
    let field = |k: &str| -> Result<String, PyErr> { Ok(get_opt(appr, k)?.map(py::s).unwrap_or_default()) };
    Ok(format!("recipe {name}；{MIN_STAGE} 由 {} 核准（{} @ {}）{tail_text}", py::s(get(appr, "by")?), field("kind")?, field("at")?))
}

/// diff 形狀和 run.json 的 recipe 要相符（`recipe::diff_shortfalls`）；不符就回 Refused，提示用 escalate 升級。
pub fn check_shape(ctx: &Ctx, run_dir: &str, paths: &[String]) -> R<String> {
    let f = pyish::pjoin(run_dir, "run.json");
    let data = match load_json(ctx, &f)? {
        Ok(v) => v,
        Err(e) => return refused(format!("讀不到 {f}：{e}")),
    };
    let r = recipe::recipe_of_run(&data).map_err(py::from_recipe)?;
    let name = py::s(get(&r, "name")?);
    // 專案加的路徑分類（profile 的 diff_classes_extra，只會多出不符）；寫壞了不能當成沒設定
    let extra = match recipe::diff_extra(&save_run::load_profile(ctx, "").0) {
        Ok(x) => x,
        Err(es) => return refused(format!("profile 的路徑分類設定讀不懂，無法確認 diff 形狀：{}", es.iter().take(3).cloned().collect::<Vec<_>>().join("；"))),
    };
    let short = recipe::diff_shortfalls_with(&r, &json!(paths), Some(&extra)).map_err(py::from_recipe)?;
    if !short.is_empty() {
        return refused(format!(
            "diff 形狀和 recipe {name} 不符：{}（要升級 recipe 用 awf save-run escalate {run_dir} --recipe <名> --by --at --reason，重跑缺的段後再推）",
            short.iter().take(5).cloned().collect::<Vec<_>>().join("；")
        ));
    }
    Ok(format!("diff 形狀和 recipe {name} 相符（{} 個檔）", paths.len()))
}

// ------------------------------------------------------------------ 主流程

pub fn supersedes_line(n: i64) -> String {
    format!("取代 #{n}（原 PR 由人關閉；本工具不關閉）")
}

/// `--supersedes` 的值：正整數。
pub fn positive_int(v: &str) -> Result<i64, String> {
    match int_of_str(v).and_then(|n| i64::try_from(n).ok()) {
        Some(n) if n > 0 => Ok(n),
        _ => Err(format!("要是正整數：{}", repr_str(v))),
    }
}

/// 稽核紀錄的路徑：`<專案>/<runs_dir>/publish-audit.jsonl`。
fn append_audit(proj: &str, cfg: &Config, rec: &Value) -> Result<String, PyErr> {
    let dir = pyish::pjoin(proj, &cfg.runs_dir);
    let path = pyish::pjoin(&dir, "publish-audit.jsonl");
    pyish::mkdir_p(&dir)?;
    let line = format!("{}\n", recipe::dumps(rec, None));
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).create(true).open(&path).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| PyErr::os(&e, &path))?;
    Ok(path)
}

/// 專案根目錄：`--project-dir`，沒有就用環境給的。
fn project_dir(env: &Env, arg: Option<&str>) -> Result<String, PyErr> {
    match arg {
        Some(a) => env.ctx.resolve(a),
        None => Ok(env.project.clone()),
    }
}

fn set(rec: &mut Map<String, Value>, k: &str, v: Value) {
    rec.insert(k.to_string(), v);
}

fn reason(rec: &mut Map<String, Value>, text: String) {
    if let Some(Value::Array(a)) = rec.get_mut("reasons") {
        a.push(Value::String(text));
    }
}

/// 暫存的 PR 說明檔（`tempfile.mkstemp(prefix="publish-body-", suffix=".md")`）。
fn temp_body(text: &str) -> Result<String, PyErr> {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    for n in 0..1000u32 {
        let path = dir.join(format!("publish-body-{}-{nanos}-{n}.md", std::process::id()));
        let name = path.to_string_lossy().into_owned();
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(mut f) => {
                f.write_all(text.as_bytes()).map_err(|e| PyErr::os(&e, &name))?;
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(PyErr::os(&e, &name)),
        }
    }
    Err(PyErr::other("FileExistsError", "[Errno 17] No usable temporary file name found"))
}

/// 推送前的去客戶化掃描：專案 profile 的 `scrub` 已啟用、而且 `when` 含 `publish` 才跑（見 [`crate::scrub`]）。
/// 掃的是「要推的那個分支」裡、這次 diff 涉及的檔（內容取自分支的 commit，不是工作目錄）與它們的路徑名。
/// 沒啟用回 `Ok(None)`，不讀規則、不多跑任何 git 指令。命中、或掃描本身出錯（設定讀不懂、規則編不過、讀不到檔）都拒絕推送：
/// 這是使用者要求的閘，只會更嚴。通過回一句給稽核紀錄的說明。訊息只有路徑、行號與規則編號，沒有命中的字串與規則。
fn scrub_gate(env: &Env, repo: &str, branch: &str, names: &[String]) -> R<Option<String>> {
    let ctx = &env.ctx;
    let broken = |e: String| Stop::Refused(format!("去客戶化掃描跑不了（{e}），沒有推送"));
    let cfg = match scrub::load_config(std::path::Path::new(&ctx.abs(&ctx.profile))) {
        Ok(Some(c)) if c.runs_on("publish") => c,
        Ok(_) => return Ok(None),
        Err(e) => return Err(broken(e)),
    };
    let root = ctx.abs(&ctx.project_root());
    let rules = scrub::load_rules(&cfg, std::path::Path::new(&root)).map_err(broken)?;
    let scope = scrub::Scope::new(&cfg).map_err(broken)?;
    // 推的就是專案自己這個 repo 時，規則檔與 profile 不掃內容（每條規則都會命中自己）
    let real = |p: &str| std::path::Path::new(p).canonicalize().ok();
    let skip = if real(repo).is_some() && real(repo) == real(&root) { scrub::self_files(&cfg, &rules) } else { Default::default() };
    let mut read = |rel: &str| -> Result<Option<Vec<u8>>, String> {
        if skip.contains(rel) {
            return Ok(None);
        }
        let spec = format!("refs/heads/{branch}:{rel}");
        let stop = |s: Stop| match s {
            Stop::Refused(m) | Stop::Crash(m) => m,
        };
        let blob = git(env, repo, &["cat-file", "blob", &spec]).map_err(stop)?;
        if blob.code == 0 {
            return Ok(Some(blob.stdout.into_bytes()));
        }
        // 不是檔案內容的項目（submodule）沒有東西可掃；其他讀不到的情況不能當成沒事
        let kind = git(env, repo, &["cat-file", "-t", &spec]).map_err(stop)?;
        if kind.code == 0 && strip(&kind.stdout) == "commit" {
            return Ok(None);
        }
        Err(format!("讀不到分支上的 {rel}"))
    };
    let rep = scrub::scan_files(&rules.rules, &scope, names, &mut read).map_err(broken)?;
    let notes: String = rules.notes.iter().map(|n| format!("；注意：{n}")).collect();
    if !rep.hits.is_empty() {
        let shown: Vec<String> = rep.hits.iter().take(10).map(|h| scrub::hit_label(&rules.rules, h)).collect();
        return refused(format!("去客戶化掃描命中（{} 處）：{}{notes}", rep.hits.len(), shown.join("、")));
    }
    Ok(Some(format!("去客戶化掃描通過（{} 個檔、{} 條規則）{notes}", rep.files, rules.rules.len())))
}

/// 檢查、推送、開 PR。成功時稽核紀錄的 decision 與 exit code 已經寫進 `rec`；回傳 exit code。
fn steps(env: &Env, a: &Args, cfg: &Config, repo: &str, run_dir: Option<&str>, rec: &mut Map<String, Value>) -> R<i32> {
    let ctx = &env.ctx;
    check_target(cfg, &a.branch, &a.base).map_err(Stop::Refused)?;
    check_remote(env, repo, &a.remote, cfg)?;
    let head = git(env, repo, &["rev-parse", "--verify", &format!("refs/heads/{}^{{commit}}", a.branch)])?;
    if head.code != 0 {
        return refused(format!("本機沒有分支 {}", a.branch));
    }
    set(rec, "head_sha", json!(strip(&head.stdout)));
    let base_ref = format!("refs/remotes/{}/{}", a.remote, a.base);
    if git(env, repo, &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")])?.code != 0 {
        return refused(format!("找不到 {}/{}（先 git fetch {} {}），無法掃 diff", a.remote, a.base, a.remote, a.base));
    }
    let rng = format!("{base_ref}...refs/heads/{}", a.branch);
    // 明寫 a/、b/ 前綴：使用者設了 diff.noprefix／diff.mnemonicPrefix 時 parse_diff 認不得標頭，會一律拒絕
    let d = git(env, repo, &["diff", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", &rng])?;
    if d.code != 0 {
        return refused(format!("git diff 失敗：{}", tail(strip(&d.stderr), 200)));
    }
    if strip(&d.stdout).is_empty() {
        return refused(format!("{} 相對 {}/{} 沒有改動", a.branch, a.remote, a.base));
    }
    let nm = git(env, repo, &["diff", "--name-only", "-z", "--no-ext-diff", "--diff-filter=ACMRT", &rng])?;
    if nm.code != 0 {
        return refused(format!("git diff --name-only 失敗：{}", tail(strip(&nm.stderr), 200)));
    }
    let names: Vec<String> = nm.stdout.split('\0').filter(|n| !n.is_empty()).map(str::to_string).collect();
    let mut literals: Vec<String> = Vec::new();
    if let Some(sf) = some(&a.secrets_file) {
        let shown = pyish::ppath(sf);
        match ctx.read_text(&shown) {
            Ok(text) => literals = splitlines(&text).into_iter().map(strip).filter(|l| !l.is_empty()).map(str::to_string).collect(),
            Err(e) if e.class == "UnicodeDecodeError" => return Err(e.into()),
            Err(e) => return refused(format!("讀不到 --secrets-file：{}", e.msg)),
        }
    }
    // 專案另外加的機敏樣式（guard.config.json；沒設定時是空的，結果和以前逐字相同）
    let extra = crate::config::SecretExtra::for_project(&ctx.abs(&ctx.project_root()));
    let hits = scan_secrets_with(&d.stdout, &literals, &names, &extra).map_err(Stop::Crash)?;
    if !hits.is_empty() {
        return refused(format!("diff 疑似含機敏值（{} 處）：{}", hits.len(), hits.iter().take(10).cloned().collect::<Vec<_>>().join("、")));
    }
    // 使用者設定的去客戶化掃描（profile 的 scrub.when 含 publish 才跑；沒啟用時這裡什麼都不做，輸出和以前逐字相同）
    if let Some(note) = scrub_gate(env, repo, &a.branch, &names)? {
        reason(rec, note);
    }
    let policy = check_policy(ctx, run_dir, &a.stage)?;
    reason(rec, policy);
    // 還沒有人確認的暫定做法一律擋；有子任務被略過時要人當下確認（--confirm-skipped-subtasks）。沒有這些紀錄時不多做事
    if let Some(Ok(data)) = run_dir.map(|r| load_json(ctx, &pyish::pjoin(r, "run.json"))).transpose()? {
        let holds = save_run::human::run_holds(&data, a.confirm_skipped);
        if !holds.is_empty() {
            return refused(holds.join("；"));
        }
        let skipped = save_run::human::skipped_subtasks(&data);
        if !skipped.is_empty() {
            reason(rec, format!("有子任務被略過（{}），推送前已由人確認", skipped.join("、")));
        }
    }
    let mut shape_paths: Vec<String> = parse_diff(&d.stdout).map_err(Stop::Crash)?.paths().into_iter().map(str::to_string).chain(names.iter().cloned()).collect();
    shape_paths.sort();
    shape_paths.dedup();
    // check_policy 已經擋掉沒有 --run 的情況
    let shape = check_shape(ctx, run_dir.unwrap_or_default(), &shape_paths)?;
    reason(rec, shape);
    let body = some(&a.body_file).filter(|b| ctx.is_file(b));
    let pr_ready = some(&a.title).is_some() && body.is_some();
    if !a.no_pr && !pr_ready {
        return refused("開 PR 要有 --title 與存在的 --body-file（或加 --no-pr 只推分支）");
    }
    if a.dry_run {
        set(rec, "decision", json!("dry-run"));
        reason(rec, "檢查全部通過（dry-run，沒有推送）".into());
        return Ok(0);
    }
    let refspec = format!("refs/heads/{0}:refs/heads/{0}", a.branch);
    let p = git(env, repo, &["push", &a.remote, &refspec])?;
    if p.code != 0 {
        return refused(format!("git push 失敗：{}", tail(strip(&p.stderr), 300)));
    }
    set(rec, "decision", json!("pushed"));
    if !a.no_pr {
        let mut body_file = ctx.resolve(body.unwrap_or_default())?;
        let mut tmp_body: Option<String> = None;
        if let Some(n) = a.supersedes.filter(|n| *n != 0) {
            let text = ctx.read_text(&body_file)?;
            let text = format!("{}\n\n{}\n", text.trim_end_matches('\n'), supersedes_line(n));
            body_file = temp_body(&text)?;
            tmp_body = Some(body_file.clone());
        }
        let mut argv = env.gh.clone();
        for x in ["pr", "create", "--repo", &cfg.fork_repo, "--base", &a.base, "--head", &a.branch, "--title", a.title.as_deref().unwrap_or_default(), "--body-file", &body_file] {
            argv.push(x.to_string());
        }
        if a.draft {
            argv.push("--draft".into());
        }
        let g = run_cmd(env, &argv, Some(repo));
        if let Some(t) = &tmp_body {
            let _ = std::fs::remove_file(t);
        }
        let g = g?;
        if g.code != 0 {
            return refused(format!("已推送，但 gh pr create 失敗：{}", tail(strip(&g.stderr), 300)));
        }
        set(rec, "pr_url", json!(splitlines(strip(&g.stdout)).last().copied().unwrap_or_default()));
        set(rec, "decision", json!("published"));
    }
    Ok(0)
}

/// 一次推送：回傳（exit code、稽核紀錄）。Err 是 0.x 的 Python 版丟出沒接住的例外的地方（不寫稽核紀錄）。
pub fn publish(env: &Env, a: &Args) -> Result<(i32, Value), String> {
    let crash = |s: Stop| match s {
        Stop::Crash(m) | Stop::Refused(m) => m,
    };
    let proj = project_dir(env, some(&a.project_dir)).map_err(|e| crash(e.into()))?;
    let cfg = load_config(&proj).map_err(|e| crash(e.into()))?;
    // 沒帶 --base：PR 的目標分支用守門設定的 base_branch（預設 main）。推送的 remote 是 fork，和 base_remote 無關
    let with_base;
    let a = if a.base.is_empty() {
        // 這個 run 的 recipe 有專屬的主線（profile 的 conventions.branch.bases）時用它；run.json 讀不到就照守門設定（閘門之後一樣會拒絕）
        let own = some(&a.run).and_then(|r| env.ctx.resolve(r).ok()).and_then(|r| std::fs::read(pyish::pjoin(&r, "run.json")).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|run| crate::layout::run_base_for_project(&proj, &run));
        with_base = Args { base: own.unwrap_or_else(|| crate::layout::for_project(&proj).base_branch), ..a.clone() };
        &with_base
    } else {
        a
    };
    let repo = env.ctx.resolve(&a.repo_dir).map_err(|e| crash(e.into()))?;
    let run_dir = match some(&a.run) {
        Some(r) => Some(env.ctx.resolve(r).map_err(|e| crash(e.into()))?),
        None => None,
    };
    let mut rec = Map::new();
    set(&mut rec, "timestamp", json!(some(&a.timestamp).map(str::to_string).unwrap_or_else(pyutil::now_iso_seconds)));
    set(&mut rec, "repo", json!(cfg.fork_repo));
    set(&mut rec, "branch", json!(a.branch));
    set(&mut rec, "base", json!(a.base));
    set(&mut rec, "head_sha", json!(""));
    set(&mut rec, "run", json!(run_dir.clone().unwrap_or_default()));
    set(&mut rec, "stage", json!(a.stage));
    set(&mut rec, "decision", json!("refused"));
    set(&mut rec, "reasons", json!([]));
    set(&mut rec, "pr_url", json!(""));
    set(&mut rec, "dry_run", json!(a.dry_run));
    set(&mut rec, "supersedes", a.supersedes.map_or(Value::Null, |n| json!(n)));
    let code = match steps(env, a, &cfg, &repo, run_dir.as_deref(), &mut rec) {
        Ok(code) => code,
        Err(Stop::Refused(m)) => {
            reason(&mut rec, m);
            1
        }
        Err(Stop::Crash(m)) => return Err(m),
    };
    let audit = append_audit(&proj, &cfg, &Value::Object(rec.clone())).map_err(|e| crash(e.into()))?;
    set(&mut rec, "audit", json!(audit));
    Ok((code, Value::Object(rec)))
}

const OPTS: &[Opt] = &[
    req("--repo-dir", Kind::Str),
    req("--branch", Kind::Str),
    o("--run", Kind::Str),
    o("--stage", Kind::Str),
    o("--base", Kind::Str),
    o("--remote", Kind::Str),
    o("--title", Kind::Str),
    o("--body-file", Kind::Str),
    o("--draft", Kind::Flag),
    o("--no-pr", Kind::Flag),
    o("--dry-run", Kind::Flag),
    o("--secrets-file", Kind::Str),
    o("--supersedes", Kind::Str),
    o("--project-dir", Kind::Str),
    o("--timestamp", Kind::Str),
    o("--confirm-skipped-subtasks", Kind::Flag),
];

/// 照 argparse 解析 `push` 的參數。Err(None) 是 `-h`；Err(Some(訊息)) 是參數錯誤（exit 2）。
pub fn parse_args(argv: &[String]) -> Result<Args, Option<String>> {
    let parsed = pyargs::split_cmd(argv, &["push"]).and_then(|(_, rest, extras)| pyargs::parse(OPTS, rest, extras));
    let p = match parsed {
        Ok(p) => p,
        Err(ArgErr::Help) => return Err(None),
        Err(e) => {
            let prog = if matches!(e, ArgErr::Sub(_)) { "awf publish push" } else { "awf publish" };
            return Err(Some(format!("{prog}: error: {}", e.message())));
        }
    };
    let text = |k: &str| p.get(k).map(str::to_string);
    let supersedes = match p.get("--supersedes") {
        Some(v) => Some(positive_int(v).map_err(|m| Some(format!("awf publish push: error: argument --supersedes: {m}")))?),
        None => None,
    };
    Ok(Args {
        repo_dir: text("--repo-dir").unwrap_or_default(),
        branch: text("--branch").unwrap_or_default(),
        run: text("--run"),
        stage: text("--stage").unwrap_or_else(|| MIN_STAGE.to_string()),
        // 沒給＝空字串：`publish` 讀到專案之後換成守門設定的 base_branch（沒設定就是 main）
        base: text("--base").unwrap_or_default(),
        remote: text("--remote").unwrap_or_else(|| "origin".to_string()),
        title: text("--title"),
        body_file: text("--body-file"),
        draft: p.flag("--draft"),
        no_pr: p.flag("--no-pr"),
        dry_run: p.flag("--dry-run"),
        secrets_file: text("--secrets-file"),
        supersedes,
        project_dir: text("--project-dir"),
        timestamp: text("--timestamp"),
        confirm_skipped: p.flag("--confirm-skipped-subtasks"),
    })
}

/// CLI 的本體：stdout、stderr 的文字寫進 `out`、`errs`，回傳 exit code。
pub fn run(env: &Env, argv: &[String], out: &mut String, errs: &mut String) -> i32 {
    if argv.iter().any(|a| a == "--selftest") {
        return selftest(env, out);
    }
    let a = match parse_args(argv) {
        Ok(a) => a,
        Err(None) => {
            out.push_str(USAGE);
            return 0;
        }
        Err(Some(m)) => {
            errs.push_str(&format!("用法：awf publish push --repo-dir <目錄> --branch <分支> [選項…]（awf publish --help 看全部）\n{m}\n"));
            return 2;
        }
    };
    match publish(env, &a) {
        Ok((code, rec)) => {
            out.push_str(&recipe::dumps(&rec, Some(2)));
            out.push('\n');
            code
        }
        Err(m) => {
            // 0.x 的 Python 版在這裡是沒接住的例外（traceback、exit 1）
            errs.push_str(&format!("awf publish：{m}\n"));
            1
        }
    }
}

/// `awf publish …`
pub fn main(args: &[String]) -> i32 {
    // selftest 用的替身（對應 0.x 的 Python 版 selftest 臨時寫的 fake_git.py、fake_gh.py）
    match args.first().map(String::as_str) {
        Some("--selftest-fake-git") => return fake_git(args.get(1..).unwrap_or_default()),
        Some("--selftest-fake-gh") => return fake_gh(args.get(1..).unwrap_or_default()),
        _ => {}
    }
    // 真的要推送時一定要找得到專案根（--project-dir 也算）；selftest、說明、用法錯誤不需要
    let env = match parse_args(args) {
        Ok(a) if !args.iter().any(|x| x == "--selftest") => match Env::for_gate(some(&a.project_dir)) {
            Ok(e) => e,
            Err(m) => {
                eprintln!("awf publish：{m}，沒有推送");
                return 1;
            }
        },
        _ => Env::real(),
    };
    let (mut out, mut errs) = (String::new(), String::new());
    let code = run(&env, args, &mut out, &mut errs);
    print!("{out}");
    eprint!("{errs}");
    code
}

// ------------------------------------------------------------------ selftest

fn append_line(path: &str, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// 假的 git：push 只記下參數（`<base>.log`），其他照常交給真的 git。
fn fake_git(args: &[String]) -> i32 {
    let (Some(base), rest) = (args.first(), args.get(1..).unwrap_or_default()) else { return 2 };
    if rest.iter().any(|a| a == "push") {
        append_line(&format!("{base}.log"), &rest.join(" "));
        return 0;
    }
    Command::new("git").args(rest).status().ok().and_then(|s| s.code()).unwrap_or(1)
}

/// 假的 gh：記下參數（`<base>.log`）與 PR 說明（`<base>.body`），印一個 PR 網址。
fn fake_gh(args: &[String]) -> i32 {
    let (Some(base), rest) = (args.first(), args.get(1..).unwrap_or_default()) else { return 2 };
    append_line(&format!("{base}.log"), &rest.join(" "));
    if let Some(body) = rest.iter().position(|a| a == "--body-file").and_then(|i| rest.get(i + 1)) {
        let text = std::fs::read(body).unwrap_or_default();
        let _ = std::fs::write(format!("{base}.body"), text);
    }
    println!("https://github.com/acme-fork/app/pull/7");
    0
}

fn slurp(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

fn put(path: &str, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{path}：{e}"))?;
    }
    std::fs::write(path, data).map_err(|e| format!("{path}：{e}"))
}

struct Tester<'a> {
    /// 被測的環境：假的 git 與 gh、專案的 profile 指到暫存專案。
    env: Env,
    /// 建 run 用的 save_run 環境（0.x 的 Python 版是另外跑 save_run.py：它的專案是工具所在的專案）。
    tool: Ctx,
    proj: String,
    repo: String,
    epic: String,
    body: String,
    out: &'a mut String,
    ok: bool,
    n_cases: usize,
    verbose: bool,
}

/// `make_run` 的選項。
#[derive(Default)]
struct RunOpts<'b> {
    /// None＝deliver 不核准；"policy-forged"＝手改成政策核准。
    deliver_by: Option<&'b str>,
    deliver_payload: Option<Value>,
    deliver_extra: Option<Value>,
    recipe: Option<&'b str>,
}

impl Tester<'_> {
    fn g(&self, args: &[&str]) -> Result<String, String> {
        let mut c = Command::new("git");
        c.args(["-C", &self.repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]).args(args);
        match pyutil::run(&mut c, None, None) {
            Ok(r) if r.code == 0 => Ok(strip(&pyutil::text(&r.stdout)).to_string()),
            Ok(r) => Err(format!("git {args:?}：{}", pyutil::text(&r.stderr))),
            Err(_) => Err(format!("執行不了 git {args:?}")),
        }
    }

    fn commit_file(&self, rel: &str, data: &[u8], msg: &str) -> Result<(), String> {
        put(&format!("{}/{rel}", self.repo), data)?;
        self.g(&["add", "-A"])?;
        self.g(&["commit", "-qm", msg]).map(|_| ())
    }

    fn sub(&self, argv: &[&str], stdin: &str) {
        let argv: Vec<String> = argv.iter().map(|x| x.to_string()).collect();
        let _ = save_run::run_captured(&argv, stdin, &self.tool);
    }

    fn run_json(&self, rd: &str) -> Value {
        serde_json::from_str(&slurp(&format!("{rd}/run.json"))).unwrap_or(Value::Null)
    }

    fn make_run(&self, name: &str, opts: RunOpts) -> Result<String, String> {
        let rd = format!("{}/_tpm/runs/{name}", self.proj);
        let profile = format!("{}/.claude/teams/methodology.json", self.proj);
        let base = json!({"summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": []});
        let envelope = |payload: Value| {
            let mut e = base.as_object().cloned().unwrap_or_default();
            e.insert("status".into(), json!("done"));
            e.insert("payload".into(), payload);
            Value::Object(e)
        };
        self.sub(&["init", &rd, "--event", "sdlc"], "");
        for stage in save_run::SDLC {
            if stage != "plan" && self.run_json(&rd)["stages"][stage]["status"] == "skipped" {
                continue;
            }
            let mut payload = match (&opts.deliver_payload, stage) {
                (Some(p), "deliver") => p.clone(),
                _ => json!({}),
            };
            if stage == "test" {
                payload = json!({"commit": "a".repeat(40), "scope_covered": "全部", "scope_skipped": "none", "anti_cheat": [],
                    "criteria": [{"id": "AC-1", "criterion": "c", "command": "true", "result": "pass", "evidence": "e"}]});
            }
            let mut res = json!({"x": [envelope(payload)]});
            if stage == "plan" {
                res = json!({"epic_dir": self.epic, "intent": envelope(json!({"documents": [{"path": "specs/e/intent.md", "kind": "intent", "audience": "owner"}]}))});
                if let Some(r) = opts.recipe {
                    res["recipe"] = json!({"name": r, "modifiers": [], "size": "small"});
                }
            }
            if let (true, Some(extra)) = (stage == "deliver", opts.deliver_extra.as_ref().and_then(Value::as_object)) {
                for (k, v) in extra {
                    res[k.as_str()] = v.clone();
                }
            }
            if opts.recipe.is_some() && stage != "plan" {
                // 新版 workflow 的回傳：帶 run.json 鎖定的 recipe 與 trace（每個該派的角色一筆，紀錄檔已寫）
                let rec = self.run_json(&rd)["recipe"].clone();
                let rv = &rec["resolved"];
                res["recipe"] = json!({"name": rec["name"], "modifiers": rec["modifiers"], "stages": rv["stages"]});
                let mut roles: Vec<(String, String)> = vec![("agent".into(), "agent-workflow:dev-implementer".into())];
                if stage == "test" {
                    roles = vec![("verify#1".into(), "agent-workflow:dev-verifier".into())];
                    for (k, role) in save_run::REVIEW_ROLES {
                        if rv["test"][k] == "always" {
                            roles.push((k.to_string(), format!("agent-workflow:{role}")));
                        }
                    }
                }
                if stage == "deliver" {
                    roles = rv["deliver_axes"].as_array().map(|a| a.iter().map(|x| (format!("review:{}", x.as_str().unwrap_or_default()), "agent-workflow:tpm-reviewer".to_string())).collect()).unwrap_or_default();
                }
                let mut trace = Vec::new();
                for (label, at) in roles {
                    let f = format!("{rd}/{stage}/{}.md", label.replace([':', '#'], "-"));
                    put(&f, "# 紀錄\n".as_bytes())?;
                    trace.push(json!({"label": label, "agent_type": at, "phase": "p", "record": f, "ok": true}));
                }
                res["trace"] = Value::Array(trace);
            }
            self.sub(&["stage", &rd, stage, "--profile", &profile], &recipe::dumps(&res, None));
            if stage == "deliver" {
                match opts.deliver_by {
                    None => break,
                    Some("policy-forged") => {
                        let mut data = self.run_json(&rd);
                        data["stages"]["deliver"]["approved"] = json!({"by": "policy", "at": "t", "note": "p", "kind": "policy"});
                        put(&format!("{rd}/run.json"), recipe::dumps(&data, None).as_bytes())?;
                        break;
                    }
                    Some(_) => {}
                }
            }
            let by = if stage == "deliver" { opts.deliver_by.unwrap_or("使用者") } else { "使用者" };
            self.sub(&["approve", &rd, stage, "--by", by, "--at", "t", "--note", "p", "--profile", &profile], "");
        }
        Ok(rd)
    }

    fn args(&self, run: Option<&str>) -> Args {
        Args {
            repo_dir: self.repo.clone(),
            branch: "feature/export".into(),
            run: run.map(str::to_string),
            title: Some("docs: 匯出說明".into()),
            body_file: Some(self.body.clone()),
            base: "main".into(),
            remote: "origin".into(),
            stage: "deliver".into(),
            project_dir: Some(self.proj.clone()),
            timestamp: Some("2026-10-24T09:00:00+00:00".into()),
            ..Args::default()
        }
    }

    /// 跑一次 publish：decision 要是 `want`；有給 `why` 時要因為這條理由（不是剛好被別的理由擋下）。
    fn case(&mut self, name: &str, want: &str, why: Option<&str>, a: Args) -> Value {
        self.n_cases += 1;
        let (code, mut rec) = match publish(&self.env, &a) {
            Ok(x) => x,
            Err(m) => (1, json!({"decision": "例外", "reasons": [m]})),
        };
        let decision = rec["decision"].as_str().unwrap_or_default().to_string();
        let mut good = decision == want && (code == 0) == matches!(want, "published" | "dry-run" | "pushed");
        if let Some(w) = why {
            if !rec["reasons"].as_array().is_some_and(|a| a.iter().any(|r| r.as_str().is_some_and(|t| t.contains(w)))) {
                good = false;
                if let Some(Value::Array(a)) = rec.get_mut("reasons") {
                    a.push(json!(format!("理由裡沒有「{w}」")));
                }
            }
        }
        let detail = if !good || self.verbose { format!(" {}", pyfmt::repr(&rec["reasons"])) } else { String::new() };
        self.out.push_str(&format!("  [{}] {name}：{decision}（預期 {want}）{detail}\n", if good { "通過" } else { "失敗" }));
        self.ok = self.ok && good;
        rec
    }

    fn expect(&mut self, name: &str, cond: bool) {
        self.out.push_str(&format!("  [{}] {name}\n", if cond { "通過" } else { "失敗" }));
        self.ok = self.ok && cond;
    }
}

/// 用假的 git push 與 gh 把每個拒絕條件走一遍（不連網、不真的推送）。回傳 exit code。
/// `env.ctx` 的 teams 目錄要有 schema（在 `scaffold/` 或已 init 的專案根目錄跑）。
pub fn selftest(env: &Env, out: &mut String) -> i32 {
    let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "awf".into());
    let fake = |flag: &str| vec![me.clone(), "publish".to_string(), flag.to_string()];
    selftest_with(env, &fake("--selftest-fake-git"), &fake("--selftest-fake-gh"), out)
}

/// selftest 的本體。`fake_git`、`fake_gh` 是替身指令的開頭，後面會接「紀錄檔的路徑前綴」與原本的參數：
/// 假的 git 遇到 push 只把參數記到 `<前綴>.log`，其他交給真的 git；假的 gh 把參數記到 `<前綴>.log`、PR 說明抄到 `<前綴>.body`、印一個 PR 網址。
pub fn selftest_with(env: &Env, fake_git: &[String], fake_gh: &[String], out: &mut String) -> i32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("awf-publish-selftest-{}-{nanos}", std::process::id()));
    let root_text = root.to_string_lossy().into_owned();
    let result = std::fs::create_dir_all(&root).map_err(|e| e.to_string()).and_then(|_| selftest_body(env, fake_git, fake_gh, &root_text, out));
    let _ = std::fs::remove_dir_all(&root);
    let ok = match result {
        Ok(ok) => ok,
        Err(e) => {
            out.push_str(&format!("  [失敗] selftest 中斷：{e}\n"));
            false
        }
    };
    out.push_str(&format!("publish selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    if ok { 0 } else { 1 }
}

fn selftest_body(env: &Env, fake_git: &[String], fake_gh: &[String], root: &str, out: &mut String) -> Result<bool, String> {
    let proj = format!("{root}/proj");
    let cfg = json!({"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "upstream_repos": "acme-up/app",
        "allowed_branch_prefixes": "feature/,bugfix/", "protected_branches": ["main", "master"]});
    let cfg_path = format!("{proj}/.claude/teams/guard.config.json");
    put(&cfg_path, recipe::dumps(&cfg, None).as_bytes())?;
    // 專案 profile 用內建 recipes（stage plan 用 --profile 指定這份；check_policy 重算 recipe 時也讀這份）
    let profile = format!("{proj}/.claude/teams/methodology.json");
    put(&profile, recipe::dumps(&json!({"recipes": recipe::builtin_recipes(), "modifiers": recipe::builtin_modifiers()}), None).as_bytes())?;
    let epic = format!("{proj}/epic");
    put(
        &format!("{epic}/specs/e/intent.md"),
        "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n## 驗收條件\n- AC-1\n## 調研問題\nx\n## 判準\nx\n## 現象\nx\n## 重現步驟\nx\n## 預期行為\nx\n## 根因\nx\n".as_bytes(),
    )?;
    let (fg, fh) = (format!("{root}/fake_git"), format!("{root}/fake_gh"));
    let with_base = |prefix: &[String], base: &str| prefix.iter().cloned().chain([base.to_string()]).collect::<Vec<String>>();
    let mut pub_ctx = env.ctx.clone();
    pub_ctx.profile = profile;
    let pub_env = Env {
        ctx: pub_ctx,
        project: proj.clone(),
        git: with_base(fake_git, &fg),
        gh: with_base(fake_gh, &fh),
        env: env.env.clone(),
    };
    let repo = format!("{proj}/repos/app");
    std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
    let body = format!("{root}/body.md");
    let mut t = Tester { env: pub_env, tool: env.ctx.clone(), proj: proj.clone(), repo, epic, body: body.clone(), out, ok: true, n_cases: 0, verbose: std::env::var_os("AWF_VERBOSE").is_some_and(|v| !v.is_empty()) };

    t.g(&["init", "-q", "-b", "main"])?;
    t.commit_file("README.md", "# 示範客戶 ACME\n".as_bytes(), "init")?;
    t.g(&["remote", "add", "origin", "https://github.com/acme-fork/app.git"])?;
    t.g(&["remote", "add", "upstream", "https://github.com/acme-up/app.git"])?;
    t.g(&["update-ref", "refs/remotes/origin/main", "HEAD"])?;
    t.g(&["update-ref", "refs/remotes/upstream/main", "HEAD"])?;
    t.g(&["checkout", "-qb", "feature/export"])?;
    t.commit_file("docs/export.md", "# 匯出 CSV\n".as_bytes(), "docs: 匯出說明")?;
    t.g(&["checkout", "-qb", "feature/leak"])?;
    let key = format!("AKIA{}", "Q".repeat(16)); // 執行時組出來，不在原始碼留完整樣式
    t.commit_file("config.py", format!("AWS_KEY = '{key}'\n").as_bytes(), "feat: 設定")?;
    // A-4：沒有 ---／+++ 行的機敏檔（二進位憑證、空的 .env），以及內容以「++ 」開頭的金鑰行（整行是 +++，舊版當成標頭）
    let plus3 = format!("++ {key}\n");
    let extra: [(&str, &str, &[u8]); 3] = [("feature/p12", "certs/client.p12", b"\x00\x01\x02\xff binary"), ("feature/emptyenv", ".env", b""), ("feature/plus3", "notes.md", plus3.as_bytes())];
    for (br, fname, data) in extra {
        t.g(&["checkout", "-q", "main"])?;
        t.g(&["checkout", "-qb", br])?;
        t.commit_file(fname, data, &format!("feat: {fname}"))?;
    }
    t.g(&["checkout", "-q", "main"])?;
    t.g(&["checkout", "-qb", "feature/code"])?;
    t.commit_file("src/export.py", b"def export():\n    return 'csv'\n", "feat: 匯出")?;
    t.g(&["checkout", "-q", "main"])?;
    t.g(&["branch", "wip/x", "feature/export"])?;
    put(&body, "示範 PR 說明\n".as_bytes())?;

    let human = || RunOpts { deliver_by: Some("使用者"), ..RunOpts::default() };
    let good_run = t.make_run("r-good", human())?;
    let unapproved = t.make_run("r-pending", RunOpts::default())?;
    let finding = json!({"lens": "bugs", "findings": [{"id": "B1", "severity": "important", "claim": "c", "location": "a:1", "recommendation": "r", "confidence": 0.9}]});
    let forged = t.make_run("r-forged", RunOpts { deliver_by: Some("policy-forged"), deliver_payload: Some(finding), ..RunOpts::default() })?;
    let refuted = json!({"verdicts": [{"target_id": "PR 說明", "refuted": true, "reason": "漏列驗收條件"}]});
    let skeptic = t.make_run("r-skeptic", RunOpts { deliver_by: Some("policy-forged"), deliver_payload: Some(refuted), ..RunOpts::default() })?;
    let lost = t.make_run("r-lost", RunOpts { deliver_by: Some("policy-forged"), deliver_extra: Some(json!({"docs": null})), ..RunOpts::default() })?;
    let docs_run = t.make_run("r-docs", RunOpts { recipe: Some("docs"), ..human() })?;
    let bug_run = t.make_run("r-bugfix", RunOpts { recipe: Some("bugfix"), ..human() })?;
    let spike_run = t.make_run("r-spike", RunOpts { recipe: Some("spike"), ..RunOpts::default() })?;
    let dropped = t.make_run("r-abandon", human())?;
    t.sub(&["abandon", &dropped, "--reason", "需求取消", "--by", "使用者", "--at", "t"], "");

    t.out.push_str("publish selftest（假的 git push 與 gh，不連網）\n");
    let (git_log, gh_log, gh_body) = (format!("{fg}.log"), format!("{fh}.log"), format!("{fh}.body"));
    let _ = std::fs::remove_file(&git_log);
    let _ = std::fs::remove_file(&gh_log);
    let base = t.args(Some(&good_run));
    let with_run = |r: &str| Args { run: Some(r.to_string()), ..base.clone() };
    let on = |branch: &str| Args { branch: branch.to_string(), ..base.clone() };
    let with_stage = |stage: &str, r: &str| Args { stage: stage.to_string(), ..with_run(r) };

    t.case("dry-run：檢查全過但不推送", "dry-run", None, Args { dry_run: true, ..base.clone() });
    let pushed = slurp(&git_log);
    t.expect("dry-run 沒有呼叫 git push", pushed.is_empty());
    let rec = t.case("功能分支、已核准、無機敏值 → 推送並開 PR", "published", None, base.clone());
    let (pushed, ghlog) = (slurp(&git_log), slurp(&gh_log));
    let good = pushed.contains("push origin refs/heads/feature/export:refs/heads/feature/export")
        && !pushed.contains("--force")
        && ghlog.contains("pr create --repo acme-fork/app")
        && rec["pr_url"].as_str().is_some_and(|u| u.ends_with("/pull/7"));
    t.expect("push 只推該分支、不 force；gh pr create 明確寫 --repo fork", good);
    let _ = std::fs::remove_file(&gh_log);
    let rec = t.case("--supersedes 12 → 推送並開 PR", "published", None, Args { supersedes: Some(12), ..base.clone() });
    let (ghlog, sent) = (slurp(&gh_log), slurp(&gh_body));
    let good = sent.contains("取代 #12") && sent.starts_with("示範 PR 說明") && !ghlog.contains("pr close") && !ghlog.contains(&body) && rec["supersedes"] == 12 && !slurp(&body).contains("取代");
    t.expect("PR 說明含「取代 #12」、原說明檔不動、沒有執行 gh pr close", good);
    t.case("推到受保護分支 main → 拒絕", "refused", None, on("main"));
    t.case("分支前綴不合規（wip/）→ 拒絕", "refused", None, on("wip/x"));
    t.case("remote 指向上游 → 拒絕", "refused", None, Args { remote: "upstream".into(), ..base.clone() });
    let leak = t.case("diff 含 AWS 金鑰樣式 → 拒絕", "refused", None, on("feature/leak"));
    let good = !recipe::dumps(&leak, None).contains(&key) && leak["reasons"].as_array().is_some_and(|a| a.iter().any(|r| r.as_str().is_some_and(|x| x.contains("AWS"))));
    t.expect("機敏值命中只報樣式名稱，不印出值", good);
    let lit = format!("{root}/secrets.txt");
    put(&lit, "匯出 CSV\n".as_bytes())?;
    t.case("diff 含 --secrets-file 列的值 → 拒絕", "refused", None, Args { secrets_file: Some(lit), ..base.clone() });
    t.case("新增二進位憑證檔（.p12，diff 只有 Binary files 行）→ 拒絕", "refused", Some("certs/client.p12：機敏檔名"), on("feature/p12"));
    t.case("新增空的 .env（diff 沒有 ---／+++ 行）→ 拒絕", "refused", Some(".env：機敏檔名"), on("feature/emptyenv"));
    let rec = t.case("新增行內容以「++ 」開頭的金鑰（整行是 +++）→ 拒絕", "refused", Some("notes.md：AWS access key"), on("feature/plus3"));
    t.expect("「++ 」開頭的金鑰命中也不印出值", !recipe::dumps(&rec, None).contains(&key));
    // A-7：--stage 只能要求 deliver 或更後面的段；給更前面的段不能降低要求
    t.case("--stage test（比 deliver 前面，deliver 還沒核准）→ 拒絕", "refused", Some("--stage 'test' 不行"), with_stage("test", &unapproved));
    t.case("--stage plan（比 deliver 前面）→ 拒絕", "refused", Some("--stage 'plan' 不行"), with_stage("plan", &good_run));
    t.case("--stage run（不是 SDLC 的段）→ 拒絕", "refused", Some("--stage 'run' 不行"), with_stage("run", &good_run));
    let mpend = format!("{proj}/_tpm/runs/r-merge-pending");
    let mut data = t.run_json(&good_run);
    data["stages"]["merge"] = json!({"status": "pending", "gate": false});
    put(&format!("{mpend}/run.json"), recipe::dumps(&data, None).as_bytes())?;
    t.case("--stage merge（更嚴：merge 段還沒 done）→ 拒絕", "refused", Some("merge 狀態是 pending"), with_stage("merge", &mpend));
    t.case("--stage merge（deliver 已核准、merge 已 done）→ dry-run 通過", "dry-run", Some("要求到 merge 段"), Args { dry_run: true, ..with_stage("merge", &good_run) });
    let nodeliver = format!("{proj}/_tpm/runs/r-nodeliver");
    let mut data = t.run_json(&good_run);
    if let Some(st) = data["stages"].as_object_mut() {
        st.shift_remove("deliver");
        st.shift_remove("design");
    }
    put(&format!("{nodeliver}/run.json"), recipe::dumps(&data, None).as_bytes())?;
    t.case("run.json 少了有閘門的 deliver、design 段（手改）→ 拒絕", "refused", Some("run.json 沒有 design、deliver"), with_run(&nodeliver));
    t.case("deliver 段還沒核准 → 拒絕", "refused", None, with_run(&unapproved));
    t.case("政策核准但實際結果有 important 發現 → 拒絕", "refused", None, with_run(&forged));
    t.case("政策核准但 skeptic 推翻了產出 → 拒絕", "refused", None, with_run(&skeptic));
    t.case("政策核准但 agent 缺件（docs: null）→ 拒絕", "refused", None, with_run(&lost));
    t.case("run 已 abandon（需求取消）→ 拒絕", "refused", None, with_run(&dropped));
    let code_on = |r: &str| Args { branch: "feature/code".into(), ..with_run(r) };
    t.case("recipe 閘門鏈：bugfix 沒有 design 段（skipped）不要求核准 → dry-run 通過", "dry-run", Some("recipe bugfix"), Args { dry_run: true, ..code_on(&bug_run) });
    let bug_pending = format!("{proj}/_tpm/runs/r-bugfix-test-pending");
    let mut data = t.run_json(&bug_run);
    if let Some(st) = data["stages"]["test"].as_object_mut() {
        st.shift_remove("approved");
    }
    put(&format!("{bug_pending}/run.json"), recipe::dumps(&data, None).as_bytes())?;
    t.case("recipe 閘門鏈：bugfix 的 test 段沒核准 → 拒絕", "refused", Some("前面的 test 段還沒核准"), code_on(&bug_pending));
    let bug_gone = format!("{proj}/_tpm/runs/r-bugfix-no-build");
    data["stages"]["test"]["approved"] = json!({"by": "使用者", "at": "t", "note": "", "kind": "human"});
    if let Some(st) = data["stages"].as_object_mut() {
        st.shift_remove("build");
    }
    put(&format!("{bug_gone}/run.json"), recipe::dumps(&data, None).as_bytes())?;
    t.case("recipe 閘門鏈：recipe 有的 build 段被拿掉 → 拒絕", "refused", Some("缺段"), code_on(&bug_gone));
    t.case("spike 拒推（recipe publishable=false）", "refused", Some("spike 拒推"), with_run(&spike_run));
    t.case("diff 形狀：docs 改程式拒推（提示 escalate）", "refused", Some("awf save-run escalate"), code_on(&docs_run));
    t.case("diff 形狀：相符放行（docs recipe 只改文件）→ 推送並開 PR", "published", Some("diff 形狀和 recipe docs 相符"), with_run(&docs_run));
    t.case("沒有 --run → 拒絕", "refused", None, Args { run: None, ..base.clone() });
    let mut cfg_up = cfg.clone();
    cfg_up["fork_repo"] = json!("acme-up/app");
    put(&cfg_path, recipe::dumps(&cfg_up, None).as_bytes())?;
    t.case("fork_repo 設成上游 → 拒絕", "refused", None, base.clone());
    put(&cfg_path, recipe::dumps(&cfg, None).as_bytes())?;
    let audit = slurp(&format!("{proj}/_tpm/runs/publish-audit.jsonl"));
    let recs: Vec<Value> = splitlines(&audit).into_iter().filter(|l| !strip(l).is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let need = ["timestamp", "repo", "branch", "base", "head_sha", "run", "stage", "decision", "reasons", "pr_url"];
    let good = recs.len() == t.n_cases && recs.iter().all(|r| need.iter().all(|k| r.get(k).is_some())) && recs.get(1).is_some_and(|r| r["decision"] == "published");
    t.expect(&format!("publish-audit.jsonl 每次執行都有一行、欄位齊全（{} 行）", recs.len()), good);
    Ok(t.ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        for (name, rx) in &SECRET_PATTERNS {
            assert!(rx.compiles(), "{name}");
        }
        for rx in [&PLACEHOLDER, &SLUG_IN_URL, &FORBIDDEN_SLUG, &SECRET_FILES, &SECRET_FILE_OK] {
            assert!(rx.compiles(), "{}", rx.source());
        }
    }

    #[test]
    fn slug_and_secret_file() {
        assert_eq!(slug_of("https://github.com/Acme-Fork/App.git\n"), "acme-fork/app");
        assert_eq!(slug_of("git@github.com:acme-fork/app"), "acme-fork/app");
        assert_eq!(slug_of("nothing"), "");
        assert!(secret_file("a/.env") && secret_file("certs/x.pem\n") && secret_file("id_rsa"));
        assert!(!secret_file("a/.env.example") && !secret_file("/dev/null") && !secret_file("src/env.py"));
    }

    /// 閘門不讀 watcher 的兩個檔（save_run selftest 的 B4.2 對 publish 的那一半）。
    #[test]
    fn does_not_read_watcher_files() {
        let src = include_str!("publish.rs");
        let (a, b) = (["progress", ".jsonl"].concat(), [".watch", "-state"].concat());
        assert!(!src.contains(&a) && !src.contains(&b));
    }
}
