//! 守門的共用部分：設定與工作區（[`Guard`]）、路徑判斷、跑 git、轉呼叫目標 repo 自己的 guard。原本是 0.x 的 Python 工具 `guard.py`。
//!
//! 規則本身在 `guard_shell.rs`（Bash／PowerShell）與 `guard_paths.rs`（Edit／Write／Read／Grep）。

use super::path;
use super::py;
use super::re::Re;
use crate::config::{self, AutoMerge, Config, PyErr};
use crate::pyfmt;
use serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// 提早結束：`Exit`＝已經印好訊息、用這個 exit code 結束；`Err`＝意外錯誤（對應 Python 的例外）。
#[derive(Debug)]
pub enum Stop {
    Exit(i32),
    Err(PyErr),
}

impl From<PyErr> for Stop {
    fn from(e: PyErr) -> Self {
        Stop::Err(e)
    }
}

pub type R<T> = Result<T, Stop>;

pub fn say(msg: &str) {
    let _ = writeln!(std::io::stderr(), "{msg}");
}

pub fn err<T>(class: &str, msg: &str) -> R<T> {
    Err(Stop::Err(PyErr::new(class, msg)))
}

pub const AUDIT_FILES: &[&str] = &["merge-audit.jsonl", "publish-audit.jsonl"];
const BUILTIN_STAGES: &[&str] = &[
    "plan", "design", "design-revise", "build", "sync", "test", "deliver", "merge",
    "brief", "meeting", "demo-check", "decision", "post-merge", "upstream", "pr-review", "reqs-change", "run",
];
pub const PACK_STATE: &[&str] = &["reuse", "pack-args"];
pub const SHARED_CONTEXT_NAMES: &[&str] = &["context.md", "pack.md", "decisions.jsonl", "決策記憶.md", "結案摘要.md", "links.jsonl"];

pub static GIT_GLOBAL_OPTS: Re = Re::new(
    r#"\bgit\s+(?:(?:-C|-c)\s+(?:"[^"]*"|'[^']*'|\S+)\s+|--(?:git-dir|work-tree|namespace)=\S+\s+|--no-pager\s+|--bare\s+)+"#, "");
pub static SLUG: Re = Re::new(r"^[\w.-]+/[\w.-]+$", "");

/// 一次 hook 呼叫的守門狀態（對應 guard.py 的模組層變數）。
pub struct Guard {
    pub cfg: Config,
    /// 工作區（`CLAUDE_PROJECT_DIR` 或目前目錄），已正規化。
    pub ws: String,
    /// runs_dir 加結尾斜線；沒有 runs_dir 是空字串。
    pub runs: String,
    pub repos: String,
    pub am: AutoMerge,
    pub run_stages: Vec<String>,
    pub stage_json_fragment: Re,
    ws_real: OnceLock<Result<String, PyErr>>,
}

/// `data.get(key) or ""` 當字串用：假值是空字串，字串原樣，其他型別在 Python 會丟例外。
pub fn str_or_empty(data: &Value, key: &str) -> R<String> {
    match data.get(key) {
        None => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) if !pyfmt::truthy(v) => Ok(String::new()),
        Some(_) => err("AttributeError", &format!("{key} 不是字串")),
    }
}

/// `data.get("tool_input") or {}`。
pub fn tool_input(data: &Value) -> R<Map<String, Value>> {
    match data.get("tool_input") {
        Some(Value::Object(o)) => Ok(o.clone()),
        Some(v) if pyfmt::truthy(v) => err("AttributeError", "tool_input 不是物件"),
        _ => Ok(Map::new()),
    }
}

/// guard.py 的 `norm()`：反斜線換成 /，Git Bash 的 /c/… 換成 c:/…。
pub fn norm(p: &str) -> String {
    let p = p.replace('\\', "/");
    let core = p.strip_suffix('\n').unwrap_or(&p);
    for cand in [p.as_str(), core] {
        let b = cand.as_bytes();
        if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') && !cand[2..].contains('\n') {
            return format!("{}:{}", &cand[1..2], if b.len() == 2 { "/" } else { &cand[2..] });
        }
    }
    p
}

/// 擋人用的路徑比對一律不分大小寫。
pub fn fold(s: &str) -> String {
    py::fold(s)
}

/// 指令名：去掉路徑與 .exe 等副檔名，轉小寫。
pub fn cmd_name(tok: &str) -> String {
    let base = py::lower(tok.rsplit(['\\', '/']).next().unwrap_or(""));
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(b) = base.strip_suffix(ext) {
            // `$` 在結尾換行前也成立的情況不存在（副檔名後面沒有換行才會走到這裡）
            return b.to_string();
        }
    }
    // re.sub(r"\.(exe|cmd|bat|com|ps1)$", "", base)：結尾是「副檔名＋換行」時也會去掉副檔名、留下換行
    if let Some(core) = base.strip_suffix('\n') {
        for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
            if let Some(b) = core.strip_suffix(ext) {
                return format!("{b}\n");
            }
        }
    }
    base
}

/// 子程序的文字輸出：UTF-8（壞位元組換成替代字元）、換行統一成 \n。
fn text_mode(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").replace('\r', "\n")
}

/// 跑 git 並回傳（exit code, stdout）；找不到 git（無法執行）回 None。
pub fn run_git(args: &[&str]) -> Option<(i32, String)> {
    let out = Command::new("git").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().ok()?;
    Some((exit_code(&out.status), text_mode(&out.stdout)))
}

fn exit_code(st: &std::process::ExitStatus) -> i32 {
    if let Some(c) = st.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(s) = st.signal() {
            return -s;
        }
    }
    -1
}

pub fn git_toplevel(start: &str) -> Option<String> {
    let mut d = path::pp(start);
    loop {
        if path::exists(&d) {
            break;
        }
        let p = path::parent(&d);
        if p == d {
            break;
        }
        d = p;
    }
    if path::is_file(&d) {
        d = path::parent(&d);
    }
    let (rc, out) = run_git(&["-C", &d, "rev-parse", "--show-toplevel"])?;
    let top = py::strip(&out);
    if rc == 0 && !top.is_empty() { Some(path::pp(top)) } else { None }
}

/// repo 自己的 guard：先找 <guard>.py（跨平台），再找 bash 腳本 <guard>.sh。
fn find_repo_guard(top: &str, guard: &str) -> Option<String> {
    for ext in [".py", ".sh"] {
        let p = path::join(top, &format!(".claude/hooks/{guard}{ext}"));
        if path::is_file(&p) {
            return Some(p);
        }
    }
    None
}

/// 找可以跑 bash 腳本的 bash。Windows 上避開 System32 的 WSL bash.exe，優先用 Git for Windows 的。
fn find_bash() -> Option<String> {
    if !cfg!(windows) {
        return path::which("bash");
    }
    if let Some(env) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty()) {
        if path::is_file(&env) {
            return Some(env);
        }
    }
    if let Some(git) = path::which("git") {
        if let Ok(real) = path::resolve(&git) {
            let root = path::parent(&path::parent(&real));
            for cand in ["bin/bash.exe", "usr/bin/bash.exe"] {
                let c = path::join(&root, cand);
                if path::is_file(&c) {
                    return Some(c);
                }
            }
        }
    }
    path::which("bash").filter(|b| !b.to_lowercase().contains("system32"))
}

/// 跑 repo 的 .py guard 用的 Python：PATH 上的 python3，再來是 python（Windows 另試 py）。
fn find_python() -> Option<String> {
    let names: &[&str] = if cfg!(windows) { &["python", "python3", "py"] } else { &["python3", "python"] };
    names.iter().find_map(|n| path::which(n))
}

pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = it.peek().copied() {
                it.next();
                next = j + 1;
            }
        }
        start = next;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

impl Guard {
    /// 載入設定（對應 `import guard` 的模組層）。`Err`＝載入失敗，入口改用內建最小規則。
    pub fn load() -> Result<Guard, PyErr> {
        let ws = config::workspace()?;
        let cfg = config::load(&ws)?;
        let runs = if cfg.runs_dir.is_empty() { String::new() } else { format!("{}/", cfg.runs_dir) };
        let repos = if cfg.repos_dir.is_empty() { String::new() } else { format!("{}/", cfg.repos_dir) };
        let am = config::auto_merge(&cfg);
        let mut run_stages: Vec<String> = BUILTIN_STAGES.iter().map(|s| s.to_string()).collect();
        for s in config::custom_event_stages(&ws) {
            if !run_stages.contains(&s) {
                run_stages.push(s);
            }
        }
        run_stages.sort();
        let mut names: Vec<&str> = run_stages.iter().map(String::as_str).chain(PACK_STATE.iter().copied()).collect();
        names.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        let alt = names.iter().map(|s| regex::escape(s)).collect::<Vec<_>>().join("|");
        let stage_json_fragment = Re::dynamic(format!(r"(?:^|[^\w-])(?:{alt})\W{{0,16}}\.json\b"), "i");
        Ok(Guard { cfg, ws, runs, repos, am, run_stages, stage_json_fragment, ws_real: OnceLock::new() })
    }

    /// 印出擋下的訊息，回傳要結束的狀態（呼叫端 `return Err(g.block(…))`）。
    pub fn block(&self, msg: &str) -> Stop {
        let mut msg = msg.to_string();
        if !self.cfg.broken.is_empty() {
            msg.push_str(&format!("；guard.config.json 讀不懂（{}），已改用最嚴格的預設，請由人修正設定檔", self.cfg.broken));
        }
        say(&format!("已被 agent-workflow hook 擋下：{msg}（規則：.claude/teams/rules.md；設定：.claude/teams/guard.config.json）"));
        Stop::Exit(2)
    }

    /// `WORKSPACE.resolve()`。
    pub fn ws_real(&self) -> R<String> {
        self.ws_real.get_or_init(|| path::resolve(&self.ws)).clone().map_err(Stop::Err)
    }

    /// 相對於工作區的路徑；不在工作區裡回 None。
    pub fn rel_to_workspace(&self, p: &str) -> R<Option<String>> {
        let t = match path::resolve(&norm(p)) {
            Ok(t) => t,
            Err(e) if e.class == "ValueError" => return Ok(None),
            Err(e) => return Err(Stop::Err(e)),
        };
        Ok(path::relative_to(&t, &self.ws_real()?))
    }

    /// 路徑在 linked_repos 底下時，回它相對於那個 repo 的路徑。
    pub fn linked_rel(&self, p: &str) -> R<Option<String>> {
        let target = match path::resolve(&norm(p)) {
            Ok(t) => t,
            Err(e) if e.class == "ValueError" => return Err(Stop::Err(e)),
            Err(_) => return Ok(None),
        };
        let Some(home) = path::home().and_then(|h| path::resolve(&h).ok()) else { return Ok(None) };
        for e in &self.cfg.linked_repos {
            let e = py::strip(e);
            if e.is_empty() {
                continue;
            }
            let root = path::expanduser(e);
            let root = if path::is_abs(&root) { root } else { path::join(&self.ws, &root) };
            let root = match path::resolve(&root) {
                Ok(r) => r,
                Err(err) if err.class == "ValueError" => return Err(Stop::Err(err)),
                Err(_) => continue,
            };
            if path::parts(&root).len() < 3 || root == home {
                continue;
            }
            if let Some(r) = path::relative_to(&target, &root) {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// 這個 session 自己的 scratchpad（系統暫存目錄底下的 …/<session_id>/scratchpad/…）。
    pub fn in_scratchpad(&self, p: &str, data: &Value) -> R<bool> {
        let sid = match data.get("session_id") {
            Some(v) if pyfmt::truthy(v) => pyfmt::str_of(v),
            _ => String::new(),
        };
        let b = sid.as_bytes();
        let ok = b.len() >= 8 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-');
        if !ok {
            return Ok(false);
        }
        let target = match path::resolve(&norm(p)) {
            Ok(t) => t,
            Err(e) if e.class == "ValueError" => return Err(Stop::Err(e)),
            Err(_) => return Ok(false),
        };
        let mut roots: Vec<String> = Vec::new();
        for r in [path::gettempdir(), "/tmp".to_string(), "/private/tmp".to_string()] {
            if !r.is_empty() && path::is_dir(&r) {
                match path::resolve(&r) {
                    Ok(x) => roots.push(x),
                    Err(_) => return Ok(false),
                }
            }
        }
        let tparts = path::parts(&target);
        let under = roots.iter().any(|r| {
            let rp = path::parts(r);
            rp.len() <= tparts.len() && rp.iter().zip(&tparts).all(|(a, b)| a == b)
        });
        if !under {
            return Ok(false);
        }
        Ok(tparts.windows(2).enumerate().any(|(k, w)| k + 2 < tparts.len() && w[0] == sid && w[1] == "scratchpad"))
    }

    /// forbidden_paths 的每一項：路徑清單，以及長得像 owner/repo 且不是現有路徑的（加上 upstream_repos）。
    pub fn forbidden_entries(&self) -> (Vec<String>, Vec<String>) {
        let mut paths = Vec::new();
        let mut slugs = Vec::new();
        for e in &self.cfg.forbidden_paths {
            let e = e.trim_end_matches('/');
            if e.is_empty() {
                continue;
            }
            let p = if path::os_isabs(e) { path::pp(e) } else { path::join(&self.ws, e) };
            if SLUG.is_match(e) && !path::exists(&p) {
                slugs.push(e.to_string());
            }
            paths.push(e.to_string());
        }
        slugs.extend(self.cfg.upstream_repos.iter().cloned());
        (paths, slugs)
    }

    pub fn under_forbidden(&self, p: &str) -> R<Option<String>> {
        let rel = self.rel_to_workspace(p)?.map(|r| fold(&r));
        let absp = if path::is_abs(p) { Some(fold(&path::resolve(&norm(p))?)) } else { None };
        for e in self.forbidden_entries().0 {
            if path::os_isabs(&e) {
                let target = fold(&path::resolve(&e)?);
                if let Some(a) = &absp {
                    if *a == target || a.starts_with(&format!("{target}/")) {
                        return Ok(Some(e));
                    }
                }
            } else if let Some(r) = &rel {
                let fe = fold(&e);
                if *r == fe || r.starts_with(&format!("{fe}/")) {
                    return Ok(Some(e));
                }
            }
        }
        Ok(None)
    }

    pub fn freeze_path(&self) -> String {
        let f = path::pp(&norm(&self.am.freeze_file));
        if path::is_abs(&f) { f } else { path::join(&self.ws, &f) }
    }

    /// 回傳碰到的受保護檔：guard.config.json、稽核紀錄、凍結檔。
    pub fn protected_target(&self, text: &str, is_path: bool) -> R<Option<String>> {
        let t = norm(text);
        let freeze = &self.am.freeze_file;
        let freeze_name = freeze.rsplit('/').next().unwrap_or("");
        if is_path {
            let rel = fold(&self.rel_to_workspace(&t)?.unwrap_or_else(|| t.clone()));
            let base = rel.rsplit('/').next().unwrap_or("");
            if rel.ends_with(".claude/teams/guard.config.json") {
                return Ok(Some("guard.config.json".into()));
            }
            for a in AUDIT_FILES {
                if base == *a {
                    return Ok(Some(a.to_string()));
                }
            }
            if rel == fold(&norm(freeze)) || base == fold(freeze_name) {
                return Ok(Some(freeze.clone()));
            }
            return Ok(None);
        }
        let ft = fold(&t);
        for n in ["guard.config.json", AUDIT_FILES[0], AUDIT_FILES[1], freeze_name] {
            if !n.is_empty() && ft.contains(&fold(n)) {
                return Ok(Some(if n == freeze_name { freeze.clone() } else { n.to_string() }));
            }
        }
        Ok(None)
    }

    /// 完整 agent_type 等於 auto_merge.merger_role，且是模版角色。
    pub fn is_steward(&self, data: &Value) -> R<bool> {
        let a = str_or_empty(data, "agent_type")?;
        Ok(!self.am.merger_role.is_empty() && a == self.am.merger_role && config::is_team_agent(&a, &self.cfg))
    }

    /// 這次是用 Write 寫入 {"frozen": true, …}（只凍結）。
    pub fn write_is_freeze(&self, data: &Value) -> R<bool> {
        if data.get("tool_name").and_then(Value::as_str) != Some("Write") {
            return Ok(false);
        }
        let ti = tool_input(data)?;
        let content = match ti.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(v) if pyfmt::truthy(v) => return err("TypeError", "content 不是字串"),
            _ => String::new(),
        };
        let Ok(body) = serde_json::from_str::<Value>(&content) else { return Ok(false) };
        Ok(matches!(body.get("frozen"), Some(Value::Bool(true))) && body.is_object())
    }

    pub fn writes_freeze(&self, data: &Value) -> R<bool> {
        let a = str_or_empty(data, "agent_type")?;
        if !self.am.freeze_writers.contains(&a) || !config::is_team_agent(&a, &self.cfg) {
            return Ok(false);
        }
        self.write_is_freeze(data)
    }

    /// 轉呼叫目標 repo 自己的 guard；它擋（exit 2）我們就擋。工作區本身的 hooks 不轉呼叫。
    pub fn forward(&self, top: Option<String>, guard: &str, data: &Value) -> R<()> {
        let Some(top) = top else { return Ok(()) };
        if path::resolve(&top)? == self.ws_real()? {
            return Ok(());
        }
        let mut script = find_repo_guard(&top, guard);
        if script.is_none() {
            // worktree 從沒有 guard 的分支開出時，改用主 repo 的 guard
            let main = match run_git(&["-C", &top, "rev-parse", "--path-format=absolute", "--git-common-dir"]) {
                Some((0, out)) => Some(path::parent(py::strip(&out))),
                _ => None,
            };
            script = main.and_then(|m| find_repo_guard(&m, guard));
        }
        let Some(script) = script else { return Ok(()) };
        let mut fwd = data.clone();
        let has_ti = matches!(fwd.get("tool_input"), Some(Value::Object(_)));
        if !has_ti {
            if let Some(v) = fwd.get("tool_input") {
                if pyfmt::truthy(v) {
                    return err("TypeError", "tool_input 不是物件");
                }
            }
        }
        if let Some(Value::Object(ti)) = fwd.get_mut("tool_input") {
            if let Some(c) = ti.get("command").cloned() {
                match c {
                    Value::String(s) => {
                        ti.insert("command".into(), Value::String(GIT_GLOBAL_OPTS.replace_all(&s, "git ")));
                    }
                    _ => return err("TypeError", "command 不是字串"),
                }
            }
        }
        let agent = config::team_role(&str_or_empty(data, "agent_type")?, &self.cfg);
        if let Some(obj) = fwd.as_object_mut() {
            if let Some(mapped) = self.cfg.role_map.get(&agent) {
                obj.insert("agent_type".into(), mapped.clone());
            } else if !agent.is_empty() {
                obj.shift_remove("agent_type"); // 沒有對應的角色，以主 session 身分套用 repo 規則
            }
        }
        let top_name = path::name(&top);
        let script_name = path::name(&script);
        let mut cmd = if script.ends_with(".py") {
            let Some(pyexe) = find_python() else {
                return Err(self.block(&format!("{top_name} 有自己的 {script_name}，但找不到 Python（python3 或 python）來執行它；\
請安裝 Python，或把 repo 的 guard 改寫成 {guard}.sh")));
            };
            let mut c = Command::new(pyexe);
            c.arg(&script);
            c
        } else {
            let Some(bash) = find_bash() else {
                return Err(self.block(&format!("{top_name} 有自己的 {script_name}，但找不到 bash 來執行它（Windows 請安裝 Git for Windows，\
或設 CLAUDE_CODE_GIT_BASH_PATH；也可以把 repo 的 guard 改寫成 {guard}.py）")));
            };
            let mut c = Command::new(bash);
            c.arg(&script);
            c
        };
        cmd.env("CLAUDE_PROJECT_DIR", path::display(&top)).env("PYTHONUTF8", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return err("OSError", &e.to_string()),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(py::json_dumps(&fwd, true).as_bytes());
        }
        let out = match child.wait_with_output() {
            Ok(o) => o,
            Err(e) => return err("OSError", &e.to_string()),
        };
        let rc = exit_code(&out.status);
        let stderr = text_mode(&out.stderr);
        if rc == 2 {
            say(&format!("{}（由 agent-workflow hook 轉呼叫 {top_name} 的 {script_name}）", py::strip(&stderr)));
            return Err(Stop::Exit(2));
        }
        if rc != 0 {
            // repo 的 guard 自己出錯：無法確認是否允許，fail-closed
            let e = py::tail(py::strip(&stderr), 300);
            let detail = if e.is_empty() { String::new() } else { format!("：{e}") };
            return Err(self.block(&format!("轉呼叫 {top_name} 的 {script_name} 出錯（exit {rc}），無法確認這個動作是否允許，先擋下{detail}；請修正該 repo 的 guard 腳本")));
        }
        Ok(())
    }
}

/// 機敏檔（和 publish.py 的 SECRET_FILES 同一份清單）。
static SECRET_FILES: Re = Re::new(r"(?:^|/)(?:\.env(?:\.[\w-]+)?|id_(?:rsa|dsa|ecdsa|ed25519)|[^/]+\.(?:pem|p12|pfx|key|keystore|jks)|\.npmrc|\.pypirc|credentials(?:\.json)?)$", "i");
pub static SECRET_FILE_OK: Re = Re::new(r"\.env\.(?:example|sample|template)$", "");

pub fn is_secret_path(p: &str) -> bool {
    let n = norm(p.trim_matches(|c| c == '"' || c == '\''));
    let n = n.trim_end_matches('/');
    SECRET_FILES.is_match(n) && !SECRET_FILE_OK.is_match(n)
}

/// 知識圖的資料庫檔名（`awf kg`）與它旁邊由工具與人維護的 vault。
pub const KG_DB_NAMES: &[&str] = &["graph.db", "kb.sqlite"];

/// 知識圖的檔：`graph.db`（含暫存檔與 journal），或 vault 底下的任何檔（`notes/` 是人寫的，也算）。
/// vault 的認法：路徑裡有一層叫 `vault`，而且它的上一層放著知識層或知識圖的資料庫（所以別的專案自己的 vault 目錄不受影響）。
pub fn kg_path(p: &str) -> bool {
    let n = norm(p.trim_matches(|c| c == '"' || c == '\''));
    let parts: Vec<&str> = n.split('/').collect();
    let base = fold(parts.last().copied().unwrap_or(""));
    if base == "graph.db" || base.starts_with("graph.db.") || base.starts_with("graph.db-") {
        return true;
    }
    parts.iter().enumerate().skip(1).any(|(i, seg)| {
        fold(seg) == "vault" && {
            let parent = parts[..i].join("/");
            KG_DB_NAMES.iter().any(|db| std::path::Path::new(&format!("{parent}/{db}")).is_file())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_like_python() {
        assert_eq!(norm("/c/work/x"), "c:/work/x");
        assert_eq!(norm("/c"), "c:/");
        assert_eq!(norm("a\\b"), "a/b");
        assert_eq!(norm("/cc/x"), "/cc/x");
        assert_eq!(norm("/c/x\n"), "c:/x");
    }

    #[test]
    fn names_and_lines() {
        assert_eq!(cmd_name("C:\\Program Files\\Git\\cmd\\GIT.EXE"), "git");
        assert_eq!(cmd_name("/usr/bin/gh"), "gh");
        assert_eq!(splitlines("a\r\nb\n\nc"), vec!["a", "b", "", "c"]);
        assert!(is_secret_path("x/.env"));
        assert!(is_secret_path("'ID_RSA'"));
        assert!(!is_secret_path(".env.example"));
        assert!(is_secret_path("a/b.PEM/"));
    }
}
