//! 守門設定（`.claude/teams/guard.config.json`）的讀取與角色對應。原本是 0.x 的 Python 工具 `awf_config.py`。
//!
//! 沒有設定檔時用安全預設（只套用角色規則），不擋主 session 的一般操作，所以還沒 init 的專案不會被誤擋。
//! 「模版角色」＝ agent_type 的命名空間在 team_agent_namespaces（預設只有 agent-workflow:），且角色名以
//! team_agent_prefixes 開頭；或角色名就是 [`RESERVED_ROLES`] 之一；或登記在 extension_agents。
//!
//! 和 0.x 的 Python 版的差別：Python 的設定是不限型別的 dict，欄位型別寫錯時要等用到那一行才出錯。
//! 這裡在載入時就把欄位轉成固定型別，型別不對記在 [`Config::type_fault`]，守門一開始就當成意外錯誤擋下（只會更嚴）。
//! 載入階段就會讓 0.x 的 Python 版丟例外的寫法（runs_dir 不是字串、writer_dirs 元素不是字串）照舊回 `Err`，
//! 由入口改用內建最小規則。

use crate::hooks::path;
use crate::hooks::py;
use crate::hooks::re::Re;
use crate::pyfmt;
use serde_json::{json, Map, Value};

/// 對應 Python 例外的類別名與訊息（訊息文字只用類別名的地方居多）。
#[derive(Debug, Clone)]
pub struct PyErr {
    pub class: String,
    pub msg: String,
}

impl PyErr {
    pub fn new(class: &str, msg: &str) -> Self {
        PyErr { class: class.to_string(), msg: msg.to_string() }
    }
}

pub const DEFAULT_NS: &str = "agent-workflow:";

/// plugin 發佈的模版角色名。沒有命名空間但角色名和這些完全相同的 agent，照樣套模版角色規則（fail-closed）。
pub const RESERVED_ROLES: &[&str] = &[
    "dev-spec-author", "dev-acceptance-tester", "dev-implementer", "dev-integrator", "dev-verifier",
    "dev-design-reviewer", "dev-security-reviewer", "dev-pr-reviewer", "dev-merge-steward", "dev-release-watcher",
    "tpm-planner", "tpm-researcher", "tpm-reviewer", "tpm-skeptic", "tpm-upstream-analyst", "tpm-writer",
];

/// 擴充 agent 能套用的模版角色：只有唯讀或只寫報告的角色。
pub const EXTENSION_ROLES: &[&str] = &[
    "tpm-researcher", "tpm-reviewer", "tpm-skeptic",
    "dev-verifier", "dev-pr-reviewer", "dev-security-reviewer", "dev-design-reviewer",
];
pub const EXTENSION_FALLBACK: &str = "tpm-reviewer";

/// 只寫執行紀錄的角色。刻意不開放成設定鍵：從清單拿掉角色只會放寬。
pub const RECORD_ONLY_ROLES: &[&str] = &["tpm-reviewer", "tpm-skeptic", "tpm-planner", "dev-release-watcher"];

/// 已合併的舊角色名（相容一版）：舊名照新角色的規則套用。
pub const ROLE_ALIASES: &[(&str, &str)] = &[("dev-pr-reviewer", "tpm-reviewer"), ("tpm-upstream-analyst", "tpm-researcher")];

pub fn alias(name: &str) -> &str {
    ROLE_ALIASES.iter().find(|(old, _)| *old == name).map(|(_, new)| *new).unwrap_or(name)
}

const LIST_KEYS: &[&str] = &["upstream_repos", "forbidden_paths", "allowed_branch_prefixes", "writer_dirs", "team_agent_prefixes",
    "protected_branches", "acceptance_test_dirs", "linked_repos"];
const BOOL_KEYS: &[&str] = &["main_session_git_rules", "main_session_github_rules"];
const KEEP_EMPTY: &[&str] = &["upstream_repos", "forbidden_paths", "fork_repo", "repos_dir"];

fn defaults() -> Map<String, Value> {
    let v = json!({
        "runs_dir": "_tpm/runs",
        "briefs_dir": "_tpm/briefs",
        "repos_dir": "",
        "linked_repos": [],
        "upstream_repos": [],
        "fork_repo": "",
        "forbidden_paths": [],
        "allowed_branch_prefixes": ["feature/", "bugfix/", "hotfix/", "release/", "merge/"],
        "writer_dirs": [],
        "team_agent_prefixes": ["tpm-", "dev-"],
        "team_agent_namespaces": [DEFAULT_NS],
        "role_map": {
            "dev-spec-author": "spec-author",
            "dev-acceptance-tester": "acceptance-tester",
            "dev-implementer": "implementer",
            "dev-verifier": "verifier",
            "dev-design-reviewer": "design-reviewer",
            "dev-security-reviewer": "security-reviewer",
            "dev-pr-reviewer": "pr-reviewer",
            "tpm-reviewer": "pr-reviewer",
            "tpm-skeptic": "pr-reviewer",
            "tpm-researcher": "pr-reviewer",
            "tpm-upstream-analyst": "pr-reviewer",
            "dev-merge-steward": "merge-steward",
            "dev-release-watcher": "release-watcher"
        },
        "protected_branches": ["main", "master"],
        "main_session_git_rules": true,
        "main_session_github_rules": true,
        "auto_merge": {},
        "extension_agents": {},
        "acceptance_test_dirs": ["tests/acceptance", "e2e"]
    });
    match v {
        Value::Object(o) => o,
        _ => Map::new(),
    }
}

/// 合併預設後的設定。
#[derive(Debug, Clone)]
pub struct Config {
    pub runs_dir: String,
    pub briefs_dir: String,
    pub repos_dir: String,
    pub linked_repos: Vec<String>,
    pub upstream_repos: Vec<String>,
    pub fork_repo: String,
    pub forbidden_paths: Vec<String>,
    pub allowed_branch_prefixes: Vec<String>,
    pub writer_dirs: Vec<String>,
    pub team_agent_prefixes: Vec<String>,
    pub team_agent_namespaces: Vec<String>,
    pub role_map: Map<String, Value>,
    pub protected_branches: Vec<String>,
    pub main_session_git_rules: bool,
    pub main_session_github_rules: bool,
    pub auto_merge: Value,
    pub extension_agents: Value,
    pub acceptance_test_dirs: Vec<String>,
    /// spec 目錄（`specs_dir`，相對於目標 repo；不合規則用預設 `specs`，見 [`crate::layout`]）。
    pub specs_dir: String,
    /// worktree 目錄名（`worktrees_dir`；不合規則用預設 `.worktrees`）。
    pub worktrees_dir: String,
    /// 專案另外加的機敏檔樣式（`secret_files_extra`，正規式；只能加）。
    pub secret_files_extra: Vec<String>,
    /// 專案另外加的機敏值樣式（`secret_patterns_extra`，正規式；只能加）。
    pub secret_patterns_extra: Vec<String>,
    /// 專案有設定檔（讀不懂也算：fail-closed）。
    pub configured: bool,
    /// 設定檔讀不懂的原因；空字串＝沒問題。
    pub broken: String,
    /// 欄位型別不對（0.x 的 Python 版用到時會丟例外）。守門類一開始就擋。
    pub type_fault: Option<String>,
}

/// auto_merge 設定（合併預設值並正規化）。
#[derive(Debug, Clone)]
pub struct AutoMerge {
    pub enabled: bool,
    pub repos: Map<String, Value>,
    pub merger_role: String,
    pub merge_method: String,
    pub freeze_writers: Vec<String>,
    pub upstream_authorized_by: Map<String, Value>,
    pub freeze_file: String,
    /// 退回 PR 時要不要把它轉回 draft（`gh pr ready --undo`）。預設 true；只有明寫 `false` 才不轉（request-changes 照舊）。
    pub return_to_draft: bool,
}

/// 工作區：`CLAUDE_PROJECT_DIR`；沒有就和閘門用同一套找法（[`crate::project::locate`]：從目前目錄往上找最外層含
/// `.claude/teams/VERSION` 的目錄），都找不到才用目前目錄（沒套用的專案照舊）。這樣 hook 讀的守門設定與稽核紀錄，
/// 和 `awf merge-gate`、`awf save-run` 寫的是同一個專案的。有設環境變數時不碰檔案系統（Claude Code 跑 hook 一定會設）。
pub fn workspace() -> Result<String, PyErr> {
    if let Some(v) = std::env::var_os(crate::project::ENV).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty()) {
        return Ok(path::pp(&v));
    }
    let cwd = path::cwd()?;
    Ok(match crate::project::locate(None, None, &cwd) {
        Some(found) => path::pp(&found.root),
        None => cwd,
    })
}

pub fn config_path(ws: &str) -> String {
    path::join(ws, ".claude/teams/guard.config.json")
}

pub fn methodology_path(ws: &str) -> String {
    path::join(ws, ".claude/teams/methodology.json")
}

fn has_placeholder(s: &str) -> bool {
    // \{\{[A-Z_]+\}\}
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j] == b'_') {
                j += 1;
            }
            if j > i + 2 && j + 1 < b.len() && b[j] == b'}' && b[j + 1] == b'}' {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn clean(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(if has_placeholder(s) { String::new() } else { py::strip(s).to_string() }),
        Value::Array(a) => Value::Array(a.iter().map(clean).filter(|x| !matches!(x, Value::String(s) if s.is_empty())).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), clean(x))).collect()),
        other => other.clone(),
    }
}

fn namespaces(v: &Value) -> Vec<String> {
    let items: Vec<String> = match v {
        Value::String(s) => s.split(',').filter(|x| !py::strip(x).is_empty()).map(str::to_string).collect(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for x in items {
        if has_placeholder(&x) {
            continue;
        }
        let mut x = py::strip(&x).to_string();
        if !x.is_empty() && !x.ends_with(':') {
            x.push(':');
        }
        if !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

fn to_bool(v: &Value, default: bool) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) if !py::strip(s).is_empty() => !matches!(py::lower(py::strip(s)).as_str(), "false" | "no" | "0" | "off"),
        _ => default,
    }
}

fn io_class(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "FileNotFoundError",
        K::PermissionDenied => "PermissionError",
        K::InvalidData => "UnicodeDecodeError",
        _ => {
            // EISDIR：Linux、macOS 都是 21
            if e.raw_os_error() == Some(21) { "IsADirectoryError" } else { "OSError" }
        }
    }
}

/// `Path.read_text(encoding="utf-8")`；失敗回 Python 的例外類別名。
pub fn read_text(p: &str) -> Result<String, &'static str> {
    if p.contains('\0') {
        return Err("ValueError");
    }
    let bytes = std::fs::read(p).map_err(|e| io_class(&e))?;
    String::from_utf8(bytes).map_err(|_| "UnicodeDecodeError")
}

fn str_list(v: Option<&Value>, key: &str, fault: &mut Option<String>) -> Vec<String> {
    match v {
        None | Some(Value::Null) => {
            if matches!(v, Some(Value::Null)) && matches!(key, "upstream_repos" | "forbidden_paths" | "allowed_branch_prefixes" | "team_agent_prefixes") {
                fault.get_or_insert(format!("guard.config.json 的 {key} 型別不對"));
            }
            Vec::new()
        }
        Some(Value::String(s)) if s.is_empty() => Vec::new(),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for x in a {
                match x {
                    Value::String(s) => out.push(s.clone()),
                    _ => {
                        fault.get_or_insert(format!("guard.config.json 的 {key} 型別不對"));
                    }
                }
            }
            out
        }
        Some(other) => {
            // `x or []` 的欄位：假值當成空的
            if !(pyfmt::truthy(other)) && matches!(key, "protected_branches" | "acceptance_test_dirs" | "linked_repos") {
                return Vec::new();
            }
            fault.get_or_insert(format!("guard.config.json 的 {key} 型別不對"));
            Vec::new()
        }
    }
}

/// `secret_files_extra`、`secret_patterns_extra` 最多幾條、每條最多幾個字。
pub const SECRET_EXTRA_MAX: usize = 64;
pub const SECRET_EXTRA_PATTERN_MAX: usize = 300;

/// 專案另外加的機敏樣式清單：沒寫或 null 是空的；要是「非空字串的陣列」，條數與長度有上限。
/// 不合規定時回空清單並把原因記進 `bad`（只記第一個）。
fn extra_patterns(v: Option<&Value>, key: &str, bad: &mut String) -> Vec<String> {
    let mut fail = |why: String| {
        if bad.is_empty() {
            *bad = why;
        }
        Vec::new()
    };
    match v {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            if a.len() > SECRET_EXTRA_MAX {
                return fail(format!("{key} 最多 {SECRET_EXTRA_MAX} 條"));
            }
            let mut out = Vec::new();
            for x in a {
                match x.as_str().filter(|s| !py::strip(s).is_empty() && s.chars().count() <= SECRET_EXTRA_PATTERN_MAX) {
                    Some(s) => out.push(s.to_string()),
                    None => return fail(format!("{key} 的每一項要是 1–{SECRET_EXTRA_PATTERN_MAX} 個字的正規式")),
                }
            }
            out
        }
        Some(_) => fail(format!("{key} 要是正規式的陣列")),
    }
}

/// 編譯過的專案機敏樣式（guard.config.json 的 `secret_files_extra`、`secret_patterns_extra`）。
/// publish、hook、kb、kg 共用這一份；內建的清單各自照舊，這裡只會多擋。
#[derive(Default)]
pub struct SecretExtra {
    /// 機敏檔的路徑樣式（Python `re` 的寫法，不分大小寫，search）。
    pub files: Vec<Re>,
    /// 機敏值的樣式（Python `re` 的寫法，分大小寫，search；比對新增的每一行）。
    pub patterns: Vec<Re>,
    /// 用不了的原因（設定檔讀不懂、樣式編不過）。非空時呼叫端要往嚴的方向處理。
    pub errors: Vec<String>,
}

impl SecretExtra {
    /// 從載入好的設定編譯。
    pub fn of(cfg: &Config) -> SecretExtra {
        let mut out = SecretExtra::default();
        if !cfg.broken.is_empty() && (cfg.broken.starts_with("secret_files_extra") || cfg.broken.starts_with("secret_patterns_extra")) {
            out.errors.push(cfg.broken.clone());
        }
        for (i, src) in cfg.secret_files_extra.iter().enumerate() {
            match Re::python(src, "i") {
                Ok(re) => out.files.push(re),
                Err(e) => out.errors.push(format!("secret_files_extra[{i}] {e}")),
            }
        }
        for (i, src) in cfg.secret_patterns_extra.iter().enumerate() {
            match Re::python(src, "") {
                Ok(re) => out.patterns.push(re),
                Err(e) => out.errors.push(format!("secret_patterns_extra[{i}] {e}")),
            }
        }
        out
    }

    /// 讀 `<專案>/.claude/teams/guard.config.json` 並編譯；沒有設定檔是空的。
    /// 整份設定檔讀不懂（不是 JSON、別的欄位型別不對）時也是空的：那是各工具原本就有的處理，這裡只管這兩個欄位自己寫壞的情況。
    pub fn for_project(root: &str) -> SecretExtra {
        if !path::exists(&config_path(root)) {
            return SecretExtra::default();
        }
        load(root).map(|cfg| SecretExtra::of(&cfg)).unwrap_or_default()
    }

    /// 路徑命中專案加的機敏檔樣式（路徑用斜線）。
    pub fn file_hit(&self, p: &str) -> bool {
        !self.files.is_empty() && {
            let n = p.replace('\\', "/");
            self.files.iter().any(|re| re.is_match(&n))
        }
    }

    /// 這一行命中的機敏值樣式（第幾條，從 1 算）。
    pub fn line_hits(&self, line: &str) -> Vec<usize> {
        self.patterns.iter().enumerate().filter(|(_, re)| re.is_match(line)).map(|(i, _)| i + 1).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.patterns.is_empty() && self.errors.is_empty()
    }
}

/// 回傳合併預設後的設定。設定檔存在但讀不到或不是 JSON 物件時 fail-closed：視為已設定，其餘欄位用預設值，`broken` 記下原因
/// （守門看到 `broken` 會把模版角色的寫入全部擋下，主 session 與使用者自己的 agent 照預設值的規則）。
/// `Err`＝0.x 的 Python 版在載入階段就會丟例外的寫法。
pub fn load(ws: &str) -> Result<Config, PyErr> {
    let mut cfg = defaults();
    let dflt = defaults();
    let mut configured = false;
    let mut broken = String::new();
    let p = config_path(ws);
    if path::exists(&p) {
        let data: Option<Value> = match read_text(&p) {
            Err(class) => {
                broken = class.to_string();
                None
            }
            // 和 0.x 的 json.loads 同一套寬鬆規則：NaN、Infinity、超大的數字、落單的 surrogate 都讀得懂，
            // 不能因為某個用不到的欄位裡有這種值就把整份設定當成壞掉（那樣 forbidden_paths、extension_agents 會一起失效）
            Ok(text) => match crate::merge_gate::pyjson::loads_lenient(&text) {
                Ok(v) => Some(v),
                Err(crate::merge_gate::pyjson::JsonErr::TooDeep) => {
                    broken = "RecursionError".to_string();
                    None
                }
                Err(_) => {
                    broken = "JSONDecodeError".to_string();
                    None
                }
            },
        };
        configured = true;
        match data {
            Some(Value::Object(data)) => {
                for (k, v) in &data {
                    if k.starts_with('_') {
                        continue;
                    }
                    if k == "team_agent_namespaces" {
                        let ns = namespaces(v);
                        if !ns.is_empty() {
                            cfg.insert(k.clone(), Value::Array(ns.into_iter().map(Value::String).collect()));
                        }
                        continue;
                    }
                    let mut v = clean(v);
                    if BOOL_KEYS.contains(&k.as_str()) {
                        cfg.insert(k.clone(), Value::Bool(to_bool(&v, true)));
                        continue;
                    }
                    if LIST_KEYS.contains(&k.as_str()) {
                        if let Value::String(s) = &v {
                            v = Value::Array(s.split(',').map(py::strip).filter(|x| !x.is_empty()).map(|x| Value::String(x.to_string())).collect());
                        }
                    }
                    let empty = match &v {
                        Value::String(s) => s.is_empty(),
                        Value::Array(a) => a.is_empty(),
                        Value::Null => true,
                        _ => false,
                    };
                    if empty && dflt.contains_key(k) && !KEEP_EMPTY.contains(&k.as_str()) {
                        continue; // 空值不蓋掉有意義的預設
                    }
                    cfg.insert(k.clone(), v);
                }
            }
            _ => {
                if broken.is_empty() {
                    broken = "不是 JSON 物件".to_string();
                }
            }
        }
    }
    let mut dirs: Vec<String> = Vec::new();
    for k in ["runs_dir", "briefs_dir", "repos_dir"] {
        let s = match cfg.get(k) {
            Some(Value::String(s)) => s.trim_matches('/').to_string(),
            Some(v) if pyfmt::truthy(v) => return Err(PyErr::new("AttributeError", &format!("{k} 不是字串"))),
            _ => String::new(),
        };
        dirs.push(s);
    }
    let (runs_dir, briefs_dir, repos_dir) = (dirs[0].clone(), dirs[1].clone(), dirs[2].clone());
    let writer_dirs: Vec<String> = match cfg.get("writer_dirs") {
        Some(v) if pyfmt::truthy(v) => match v {
            Value::Array(a) => {
                let mut out = Vec::new();
                for x in a {
                    match x {
                        Value::String(s) => out.push(format!("{}/", s.trim_matches('/'))),
                        _ => return Err(PyErr::new("AttributeError", "writer_dirs 的元素不是字串")),
                    }
                }
                out
            }
            Value::Object(o) => o.keys().map(|s| format!("{}/", s.trim_matches('/'))).collect(),
            _ => return Err(PyErr::new("TypeError", "writer_dirs 不是清單")),
        },
        _ => [&runs_dir, &briefs_dir, &"docs".to_string()].iter().filter(|d| !d.is_empty()).map(|d| format!("{}/", d.trim_matches('/'))).collect(),
    };
    let mut fault: Option<String> = None;
    let linked_repos: Vec<String> = match cfg.get("linked_repos") {
        Some(Value::Array(a)) => a.iter().filter(|x| pyfmt::truthy(x)).map(pyfmt::str_of).collect(),
        Some(v) if pyfmt::truthy(v) => {
            fault.get_or_insert("guard.config.json 的 linked_repos 型別不對".to_string());
            Vec::new()
        }
        _ => Vec::new(),
    };
    let fork_repo = match cfg.get("fork_repo") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if pyfmt::truthy(v) => {
            fault.get_or_insert("guard.config.json 的 fork_repo 型別不對".to_string());
            String::new()
        }
        _ => String::new(),
    };
    let role_map = match cfg.get("role_map") {
        Some(Value::Object(o)) => o.clone(),
        _ => {
            fault.get_or_insert("guard.config.json 的 role_map 型別不對".to_string());
            Map::new()
        }
    };
    // 專案另外加的機敏樣式：寫壞了（型別不對、太多、太長）照「設定檔讀不懂」處理——模版角色的寫入全擋、主 session 照預設規則
    let mut extra_bad = String::new();
    let secret_files_extra = extra_patterns(cfg.get("secret_files_extra"), "secret_files_extra", &mut extra_bad);
    let secret_patterns_extra = extra_patterns(cfg.get("secret_patterns_extra"), "secret_patterns_extra", &mut extra_bad);
    if broken.is_empty() {
        broken = extra_bad;
    }
    let lay = crate::layout::Layout::of(&Value::Object(cfg.clone()));
    let out = Config {
        specs_dir: lay.specs_dir,
        worktrees_dir: lay.worktrees_dir,
        runs_dir,
        briefs_dir,
        repos_dir,
        linked_repos,
        upstream_repos: str_list(cfg.get("upstream_repos"), "upstream_repos", &mut fault),
        fork_repo,
        forbidden_paths: str_list(cfg.get("forbidden_paths"), "forbidden_paths", &mut fault),
        allowed_branch_prefixes: str_list(cfg.get("allowed_branch_prefixes"), "allowed_branch_prefixes", &mut fault),
        writer_dirs,
        team_agent_prefixes: str_list(cfg.get("team_agent_prefixes"), "team_agent_prefixes", &mut fault),
        team_agent_namespaces: str_list(cfg.get("team_agent_namespaces"), "team_agent_namespaces", &mut fault),
        role_map,
        protected_branches: str_list(cfg.get("protected_branches"), "protected_branches", &mut fault),
        main_session_git_rules: cfg.get("main_session_git_rules").and_then(Value::as_bool).unwrap_or(true),
        main_session_github_rules: cfg.get("main_session_github_rules").and_then(Value::as_bool).unwrap_or(true),
        auto_merge: cfg.get("auto_merge").cloned().unwrap_or(Value::Null),
        extension_agents: cfg.get("extension_agents").cloned().unwrap_or(Value::Null),
        acceptance_test_dirs: str_list(cfg.get("acceptance_test_dirs"), "acceptance_test_dirs", &mut fault),
        secret_files_extra,
        secret_patterns_extra,
        configured,
        broken,
        type_fault: fault,
    };
    Ok(out)
}

/// 設定裡的角色名補上命名空間：沒有命名空間（不含 :）時補 team_agent_namespaces 的第一個非空值。
pub fn full_role(role: &str, cfg: &Config) -> String {
    let role = py::strip(role);
    if role.is_empty() || role.contains(':') {
        return role.to_string();
    }
    let ns = if cfg.team_agent_namespaces.is_empty() {
        DEFAULT_NS.to_string()
    } else {
        cfg.team_agent_namespaces.iter().find(|n| !n.is_empty()).cloned().unwrap_or_default()
    };
    format!("{ns}{role}")
}

/// auto_merge 設定合併預設值並正規化。
pub fn auto_merge(cfg: &Config) -> AutoMerge {
    let empty = Map::new();
    let raw = cfg.auto_merge.as_object().unwrap_or(&empty);
    let get = |k: &str| -> Option<&Value> {
        if k.starts_with('_') {
            return None;
        }
        raw.get(k).filter(|v| !matches!(v, Value::Null) && !matches!(v, Value::String(s) if s.is_empty()))
    };
    let repos: Map<String, Value> = match get("repos") {
        Some(Value::String(s)) => s.split(',').map(py::strip).filter(|x| !x.is_empty()).map(|x| (x.to_string(), json!("low"))).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).filter(|x| !x.is_empty()).map(|x| (x.to_string(), json!("low"))).collect(),
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    let enabled = matches!(get("enabled"), Some(Value::Bool(true)));
    let merger_default = format!("{DEFAULT_NS}dev-merge-steward");
    let merger_raw = match get("merger_role") {
        None => merger_default,
        Some(v) if pyfmt::truthy(v) => pyfmt::str_of(v),
        Some(_) => String::new(),
    };
    let merger_role = full_role(&merger_raw, cfg);
    let fw: Vec<String> = match get("freeze_writers") {
        None => vec![format!("{DEFAULT_NS}tpm-writer")],
        Some(Value::String(s)) => s.split(',').map(|x| py::strip(x).to_string()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(_) => Vec::new(),
    };
    let freeze_writers = fw.iter().filter(|x| !py::strip(x).is_empty()).map(|x| full_role(x, cfg)).collect();
    let merge_method = match get("merge_method") {
        Some(v) if pyfmt::truthy(v) => py::lower(py::strip(&pyfmt::str_of(v))),
        _ => "squash".to_string(),
    };
    let upstream_authorized_by = match get("upstream_authorized_by") {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    let ff = match get("freeze_file") {
        Some(v) if pyfmt::truthy(v) => py::strip(&pyfmt::str_of(v).replace('\\', "/")).to_string(),
        _ => String::new(),
    };
    let freeze_file = if ff.is_empty() {
        let runs = if cfg.runs_dir.is_empty() { "_tpm/runs" } else { &cfg.runs_dir };
        format!("{}/auto-merge.freeze.json", runs.trim_matches('/'))
    } else {
        ff
    };
    let return_to_draft = !matches!(get("return_to_draft"), Some(Value::Bool(false)));
    AutoMerge { enabled, repos, merger_role, merge_method, freeze_writers, upstream_authorized_by, freeze_file, return_to_draft }
}

/// `guard.config.json` 明寫的合併角色，而且角色名不是內建的 `dev-merge-steward`：回正規化後的完整 agent_type。
/// 沒有設定檔、沒寫、設定檔讀不懂、值不是一行字、角色名仍是 `dev-merge-steward`（照角色名認就夠）都回 `None`。
///
/// 存檔工具與狀態機用它認「這筆派工是合併角色」（代寫紀錄、模型下限、自訂腳本不能派）：只認設定檔，
/// 不認 trace 自己的說法。回 `None` 時各處的行為和沒有這個設定時相同。hook 的放行判斷不看這裡（它比對完整 agent_type）。
pub fn custom_merger(ws: &str) -> Option<String> {
    let cfg = load(ws).ok()?;
    if !cfg.broken.is_empty() {
        return None;
    }
    let raw = cfg.auto_merge.as_object()?.get("merger_role")?.as_str()?;
    if py::strip(raw).is_empty() {
        return None;
    }
    let full = auto_merge(&cfg).merger_role;
    let one_line = !full.is_empty() && !full.chars().any(|c| c.is_control() || c == '`');
    (one_line && role_name(&full) != "dev-merge-steward").then_some(full)
}

/// plugin 的 agent 會帶命名空間（例如 agent-workflow:dev-implementer），比對前去掉。
pub fn role_name(agent: &str) -> &str {
    agent.rsplit(':').next().unwrap_or("")
}

/// agent_type 的命名空間（含冒號）；沒有命名空間回空字串。
pub fn namespace(agent: &str) -> String {
    match agent.rfind(':') {
        Some(i) => format!("{}:", &agent[..i]),
        None => String::new(),
    }
}

/// 內建的資料助手（1.2.2）：plugin 的 `awf-lookup`（查資料、整理後回報）與 `awf-refresh`（把衍生資料更新到最新）。都不交信封。
/// 守門把它們當成 [`LOOKUP_RULES_AS`] 那個唯讀角色：shell 只能讀、`awf` 只能跑唯讀子指令。這個對應寫死，設定檔改不掉（只會更嚴）。
/// `awf-refresh` 另外可以跑逐字相符的幾個更新指令（`hooks/guard_shell.rs` 的 `REFRESH_EXACT`）。
pub const LOOKUP_AGENT: &str = "awf-lookup";
pub const REFRESH_AGENT: &str = "awf-refresh";
const LOOKUP_RULES_AS: &str = "tpm-reviewer";
/// 是不是內建的更新員（命名空間的規則同 [`is_lookup_agent`]）。
pub fn is_refresh_agent(agent: &str, cfg: &Config) -> bool {
    role_name(agent) == REFRESH_AGENT && is_lookup_agent(agent, cfg)
}

/// 是不是內建的資料助手（查詢員或更新員）：帶模版的命名空間，或沒有命名空間的同名 agent（和 [`RESERVED_ROLES`] 的同名規則一致）。
pub fn is_lookup_agent(agent: &str, cfg: &Config) -> bool {
    if role_name(agent) != LOOKUP_AGENT && role_name(agent) != REFRESH_AGENT {
        return false;
    }
    let ns = namespace(agent);
    let default_ns = [DEFAULT_NS.to_string()];
    let team_ns: &[String] = if cfg.team_agent_namespaces.is_empty() { &default_ns } else { &cfg.team_agent_namespaces };
    ns.is_empty() || team_ns.contains(&ns)
}

/// 登記在 extension_agents 的擴充 agent 回傳要套用的模版角色名；沒登記回空字串。比對完整 agent_type。
/// 內建的查詢員（[`is_lookup_agent`]）一律套唯讀角色，不看設定檔。
pub fn extension_role(agent: &str, cfg: &Config) -> &'static str {
    if is_lookup_agent(agent, cfg) {
        return LOOKUP_RULES_AS;
    }
    let Some(ext) = cfg.extension_agents.as_object() else { return "" };
    if agent.is_empty() {
        return "";
    }
    let Some(v) = ext.get(agent) else { return "" };
    let mapped = if pyfmt::truthy(v) { pyfmt::str_of(v) } else { String::new() };
    let role = role_name(&mapped);
    EXTENSION_ROLES.iter().find(|r| **r == role).copied().unwrap_or(EXTENSION_FALLBACK)
}

/// agent 是不是模版角色：傳**完整的 agent_type**（含命名空間）。
pub fn is_team_agent(agent: &str, cfg: &Config) -> bool {
    if agent.is_empty() {
        return false;
    }
    if RESERVED_ROLES.contains(&role_name(agent)) || !extension_role(agent, cfg).is_empty() {
        return true;
    }
    let default_ns = [DEFAULT_NS.to_string()];
    let ns: &[String] = if cfg.team_agent_namespaces.is_empty() { &default_ns } else { &cfg.team_agent_namespaces };
    ns.contains(&namespace(agent)) && cfg.team_agent_prefixes.iter().any(|p| role_name(agent).starts_with(p.as_str()))
}

/// 模版角色回傳去掉命名空間的角色名，擴充 agent 回傳它套用的角色，都不是回空字串。舊名回傳併入的新角色名。
pub fn team_role(agent: &str, cfg: &Config) -> String {
    if !RESERVED_ROLES.contains(&role_name(agent)) {
        let ext = extension_role(agent, cfg);
        if !ext.is_empty() {
            return alias(ext).to_string();
        }
    }
    if !is_team_agent(agent, cfg) {
        return String::new();
    }
    alias(role_name(agent)).to_string()
}

/// 守門每次都要從 profile（methodology.json）拿的東西：一次讀檔、一次解析。
#[derive(Debug, Clone, Default)]
pub struct ProfileBits {
    /// `events` 裡自訂的事件類型名（段名）。
    pub event_stages: Vec<String>,
    /// `knowledge.repos[].path` 照寫的原文（還沒解析成絕對路徑；不是非空字串的項目不收）。
    pub knowledge_repos: Vec<String>,
    /// `flow.stages[].id`：自訂段的段名（run 目錄的 `<段>.json` 是閘門資料）。只看形狀、不看其他規則是否合格（寧可多擋）。
    pub flow_stages: Vec<String>,
}

/// 讀不到、格式不對一律當作沒有這些設定（回空的）。
pub fn profile_bits(ws: &str) -> ProfileBits {
    let mut out = ProfileBits::default();
    let path = methodology_path(ws);
    let Ok(text) = read_text(&path) else { return out };
    let Ok(doc) = serde_json::from_str::<Value>(&text) else { return out };
    // 繼承（extends）：沒有時不碰檔案系統；解析失敗照「讀不懂當作沒設定」
    let Ok(Value::Object(doc)) = crate::profile::resolve_str(&path, doc) else { return out };
    if let Some(Value::Object(ev)) = doc.get("events") {
        out.event_stages = ev.keys().filter(|k| {
            let b = k.as_bytes();
            !b.is_empty() && b.len() <= 40 && b[0].is_ascii_lowercase()
                && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        }).cloned().collect();
    }
    if let Some(Value::Array(list)) = doc.get("flow").and_then(|f| f.get("stages")) {
        out.flow_stages = list.iter().take(MAX_FLOW_STAGES).filter_map(|x| x.get("id").and_then(Value::as_str)).filter(|id| crate::stages::flow_id_ok(id)).map(str::to_string).collect();
    }
    if let Some(Value::Array(repos)) = doc.get("knowledge").and_then(|k| k.get("repos")) {
        for r in repos.iter().take(MAX_KNOWLEDGE_REPOS) {
            if let Some(p) = r.get("path").and_then(Value::as_str).map(py::strip).filter(|p| !p.is_empty() && !p.contains('\0')) {
                out.knowledge_repos.push(p.to_string());
            }
        }
    }
    out
}

/// 守門只看 `flow.stages` 的前幾筆（規則上限是 8；多看一些是為了「宣告超過上限」時照樣擋，清單再長也不讓 hook 變慢）。
pub const MAX_FLOW_STAGES: usize = 64;

/// 守門只看 `knowledge.repos` 的前幾筆（每一筆都要解析路徑；清單再長也不讓 hook 變慢）。
pub const MAX_KNOWLEDGE_REPOS: usize = 32;

/// profile 的 events 裡自訂的事件類型名（段名）。讀不到、格式不對一律回空清單。
pub fn custom_event_stages(ws: &str) -> Vec<String> {
    profile_bits(ws).event_stages
}

/// 讀取預算設定（methodology.json 的 limits）。
#[derive(Debug, Clone)]
pub struct Limits {
    pub enforce: bool,
    pub whole_file_lines: u64,
    pub default_budget: u64,
    pub roles: Vec<(String, u64)>,
}

fn limit_int(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) if n.is_u64() => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// None＝整個功能關閉：沒有 methodology.json、讀不懂、沒有 limits 鍵、沒寫 mode、mode 是 off 或不認得的值。
pub fn limits(ws: &str) -> Option<Limits> {
    let path = methodology_path(ws);
    let text = read_text(&path).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let data: Value = serde_json::from_str(text).ok()?;
    // 繼承（extends）解析失敗＝讀不懂＝關閉（讀取預算的任何例外都放行，I9）
    let data = crate::profile::resolve_str(&path, data).ok()?;
    let lim = data.as_object()?.get("limits")?.as_object()?;
    let mode = lim.get("mode")?.as_str()?;
    if mode != "enforce" && mode != "warn" {
        return None;
    }
    let empty = Map::new();
    let rb = lim.get("read_budget").and_then(Value::as_object).unwrap_or(&empty);
    let raw_roles = rb.get("roles").and_then(Value::as_object).unwrap_or(&empty);
    let mut roles: Vec<(String, u64)> = Vec::new();
    for (k, v) in raw_roles {
        if k.is_empty() || k.starts_with('_') {
            continue;
        }
        let name = alias(role_name(py::strip(k))).to_string();
        let n = limit_int(Some(v));
        match roles.iter_mut().find(|(r, _)| *r == name) {
            Some(slot) => slot.1 = n,
            None => roles.push((name, n)),
        }
    }
    Some(Limits {
        enforce: mode == "enforce",
        whole_file_lines: limit_int(lim.get("whole_file_lines")),
        default_budget: limit_int(rb.get("default")),
        roles,
    })
}

/// 輪數預算的模式（methodology.json 的 `limits.turns.mode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnMode {
    Off,
    Warn,
    Enforce,
}

/// 輪數預算設定（methodology.json 的 `limits.turns`）。和讀取預算（[`limits`]）各自獨立：`limits.mode` 沒寫時這個照樣有效。
#[derive(Debug, Clone)]
pub struct Turns {
    pub mode: TurnMode,
    pub default: u64,
    pub roles: Vec<(String, u64)>,
}

/// 沒寫 `limits.turns` 時的門檻（工具呼叫次數）。
pub const TURNS_DEFAULT: u64 = 50;
/// 門檻的上下限：超出範圍的值一律退回預設。
pub const TURNS_MIN: u64 = 10;
pub const TURNS_MAX: u64 = 500;

impl Turns {
    /// 預設：只提醒、門檻 [`TURNS_DEFAULT`]。
    pub fn builtin() -> Turns {
        Turns { mode: TurnMode::Warn, default: TURNS_DEFAULT, roles: Vec::new() }
    }

    /// 這個角色的門檻。
    pub fn threshold(&self, role: &str) -> u64 {
        self.roles.iter().find(|(r, _)| r == role).map(|(_, n)| *n).unwrap_or(self.default)
    }
}

fn turn_int(v: Option<&Value>) -> Option<u64> {
    match v {
        Some(Value::Number(n)) => n.as_u64().filter(|n| (TURNS_MIN..=TURNS_MAX).contains(n)),
        _ => None,
    }
}

/// 讀 `limits.turns`。沒有 methodology.json、讀不懂、沒寫或寫錯：一律退回預設（warn、50）；個別欄位不合法只退回那一欄。
pub fn turns(ws: &str) -> Turns {
    let mut out = Turns::builtin();
    let path = methodology_path(ws);
    let Ok(text) = read_text(&path) else { return out };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(data) = serde_json::from_str::<Value>(text) else { return out };
    let Ok(data) = crate::profile::resolve_str(&path, data) else { return out };
    let Some(t) = data.get("limits").and_then(|l| l.get("turns")).and_then(Value::as_object) else { return out };
    out.mode = match t.get("mode").and_then(Value::as_str) {
        Some("off") => TurnMode::Off,
        Some("enforce") => TurnMode::Enforce,
        _ => TurnMode::Warn,
    };
    if let Some(n) = turn_int(t.get("default")) {
        out.default = n;
    }
    if let Some(Value::Object(roles)) = t.get("roles") {
        for (k, v) in roles {
            let Some(n) = turn_int(Some(v)) else { continue };
            if k.is_empty() || k.starts_with('_') {
                continue;
            }
            let name = alias(role_name(py::strip(k))).to_string();
            match out.roles.iter_mut().find(|(r, _)| *r == name) {
                Some(slot) => slot.1 = n,
                None => out.roles.push((name, n)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        load("/nonexistent-awf-config-test").unwrap()
    }

    #[test]
    fn roles() {
        let cfg = base();
        assert!(!cfg.configured);
        assert!(is_team_agent("agent-workflow:dev-implementer", &cfg));
        assert!(is_team_agent("dev-implementer", &cfg));
        assert!(!is_team_agent("dev-helper", &cfg));
        assert!(is_team_agent("agent-workflow:dev-helper", &cfg));
        assert!(!is_team_agent("other:tpm-notes", &cfg));
        assert_eq!(team_role("agent-workflow:dev-pr-reviewer", &cfg), "tpm-reviewer");
        assert_eq!(team_role("my-agent", &cfg), "");
        assert_eq!(namespace("a:b:c"), "a:b:");
        assert_eq!(role_name("a:b:c"), "c");
        let am = auto_merge(&cfg);
        assert!(!am.enabled);
        assert_eq!(am.merger_role, "agent-workflow:dev-merge-steward");
        assert_eq!(am.freeze_file, "_tpm/runs/auto-merge.freeze.json");
        assert_eq!(cfg.writer_dirs, vec!["_tpm/runs/", "_tpm/briefs/", "docs/"]);
    }

    #[test]
    fn placeholder() {
        assert!(has_placeholder("x {{FORK_REPO}} y"));
        assert!(!has_placeholder("{{lower}}"));
        assert!(!has_placeholder("{{}}"));
    }
}
