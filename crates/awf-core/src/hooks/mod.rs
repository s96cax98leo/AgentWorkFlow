//! hook 的入口與規則（guard-shell、guard-paths、envelope、session-start）。原本是 0.x 的 Python hook（`guard.py`、`envelope_hook.py` 等）。
//!
//! 用法：`awf hook <guard-shell|guard-paths|envelope|session-start|mark-dirty|prompt-context> < hook-input.json`
//!
//!   guard-shell   PreToolUse（Bash|PowerShell）：擋 agent push、agent 的 GitHub 寫入、寫入上游、動禁止路徑、不合規的分支名；
//!                 之後轉呼叫目標 repo 自己的 guard-git。
//!   guard-paths   PreToolUse（Edit|Write|NotebookEdit|Read|Grep）：擋寫禁止路徑、各角色寫入範圍以外的檔案；Read 另套讀取預算。
//!   envelope      PreToolUse（SubagentHandback、StructuredOutput）、SubagentStop、TaskCompleted：檢查交件信封。
//!   session-start SessionStart：plugin 版本和專案版本不一致時印一行提示；另外注入幾行摘要（進行中的 run、斷掉的連結數、最近的決策）。永遠 exit 0。
//!   mark-dirty    PostToolUse（Edit|Write）：把改到的檔記進知識圖的待更新清單（沒有圖就不記）。永遠 exit 0。
//!   prompt-context UserPromptSubmit：訊息提到需求 id、檔名、符號名時查知識圖，注入幾行位置（profile 開啟才做）。永遠 exit 0。
//!                 後三種是提示與自動接點（[`auto`]）：輸出格式與依據寫在該模組的說明。
//!
//! 結束碼：0 放行；2 擋下（stderr 回給 Claude）。
//!
//! 失敗語意（plugin 以 user scope 安裝時會套在使用者所有專案，所以「hook 壞掉就鎖死所有工具」不能接受）：
//!   1. hook 跑不起來（找不到執行檔、逾時）：hooks.json 設 onFailure=continue，放行。
//!   2. 跑得起來但輸入壞掉（空、不是 JSON、缺 tool_name／tool_input、shell 指令超過 [`MAX_SHELL_COMMAND`]）：
//!      守門類一律擋（exit 2），交件類放行（exit 0）。
//!   3. 設定載入失敗（0.x 的 Python 版是 guard.py／awf_config.py 載入失敗），或程式內部 panic：改用本檔內建的最小規則
//!      （[`fallback_shell`]），只擋 push／send-pack、gh pr merge、--approve、gh api 寫入、curl 等打 merge／reviews；
//!      其他放行，guard-paths 一律放行。stderr 印一行提示。最小規則刻意不依賴其他模組的狀態。
//!   4. 守門規則執行中出現意外錯誤（對應 Python 的例外）：守門類擋（exit 2），交件類放行（exit 0）。
//!   5. 讀取預算是輔助功能：任何失敗都放行，不影響前面幾條既有檢查的結果。
//!
//! release 是 `panic = "abort"`，所以不靠 `catch_unwind`：panic hook 裡直接套第 3 條並結束行程。

pub mod auto;
pub mod awf_cli;
pub mod envelope;
pub mod guard;
pub mod guard_paths;
pub mod guard_shell;
pub mod path;
pub mod ps;
pub mod py;
pub mod pyjson;
pub mod re;
pub mod read_budget;
pub mod session_start;

use crate::config::PyErr;
use guard::{say, Guard, Stop};
use re::Re;
use serde_json::Value;
use std::io::Read;
use std::sync::OnceLock;

const MODES: &[&str] = &["guard-shell", "guard-paths", "envelope"];
/// 守門規則對指令長度大約是平方成長，超長指令會拖到 timeout，而 timeout＝onFailure continue＝放行。
/// 為了不讓「把指令灌長」變成繞過守門的方法，超過上限的 shell 指令直接擋。
pub const MAX_SHELL_COMMAND: usize = 128 * 1024;

fn is_guard(mode: &str) -> bool {
    mode == "guard-shell" || mode == "guard-paths"
}

/// 讀 stdin 並檢查基本形狀；`Err`＝輸入有問題的說明。
fn read_input(mode: &str) -> Result<Value, String> {
    let mut bytes = Vec::new();
    let _ = std::io::stdin().lock().read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let raw = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if py::strip(raw).is_empty() {
        return Err("輸入是空的".into());
    }
    let data: Value = serde_json::from_str(raw).map_err(|_| "不是合法 JSON：JSONDecodeError".to_string())?;
    if !data.is_object() {
        return Err("不是 JSON 物件".into());
    }
    if is_guard(mode) {
        let ok = data.get("tool_name").is_some_and(Value::is_string) && data.get("tool_input").is_some_and(Value::is_object);
        if !ok {
            return Err("缺 tool_name 或 tool_input".into());
        }
        if mode == "guard-shell" {
            if let Some(Value::String(cmd)) = data.get("tool_input").and_then(|t| t.get("command")) {
                let n = cmd.chars().count();
                if n > MAX_SHELL_COMMAND {
                    return Err(format!("指令長度 {n} 字元，超過守門可在時限內檢查的上限 {MAX_SHELL_COMMAND}；請拆成較短的指令或改寫成腳本檔"));
                }
            }
        }
    }
    Ok(data)
}

// ───────── 內建最小規則：設定載入失敗或內部 panic 時用，刻意不依賴設定與其他規則 ─────────

static FB_SEG: Re = Re::new(r"[;&|\n]+", "");
static FB_GIT: Re = Re::new(r"(?:^|[\s/\\(])git(?:\.exe)?(?=\s|$)", "i");
static FB_PUSH: Re = Re::new(r"(?:^|\s)(?:push|send-pack)(?=\s|$)|\bgit-send-pack\b", "i");
static FB_MERGE: Re = Re::new(r"\bgh(?:\.exe)?\s+pr\s+merge\b", "i");
static FB_APPROVE: Re = Re::new(r"(?:^|\s)--approve\b|\bgh(?:\.exe)?\s+pr\s+review\b.*\s-[A-Za-z]*a[A-Za-z]*(?=\s|$)|\baddPullRequestReview\b", "i");
static FB_GH_API: Re = Re::new(r"\bgh(?:\.exe)?\s+api\b", "i");
static FB_API_WRITE: Re = Re::new(r#"(?:-X\s*|--method[\s=]+)["']?(?:POST|PUT|PATCH|DELETE)\b|\s(?:-f|-F|--field|--raw-field|--input)[\s=]"#, "i");
static FB_WEB: Re = Re::new(r"(?:^|[\s/\\(])(?:curl|wget|invoke-restmethod|irm|invoke-webrequest|iwr)(?:\.exe)?(?=\s|$)", "i");
static FB_PR_WRITE: Re = Re::new(r"/merges?(?![\w-])|/reviews\b|mergePullRequest|enablePullRequestAutoMerge|mergeBranch", "i");

/// 這個模組所有固定的樣式（測試用）。
pub fn fallback_patterns() -> Vec<&'static Re> {
    vec![&FB_SEG, &FB_GIT, &FB_PUSH, &FB_MERGE, &FB_APPROVE, &FB_GH_API, &FB_API_WRITE, &FB_WEB, &FB_PR_WRITE]
}

/// 回傳擋下的理由；None＝放行。
pub fn fallback_shell(data: &Value) -> Option<&'static str> {
    let cmd = match data.get("tool_input").and_then(|t| t.get("command")) {
        Some(Value::String(s)) => s.clone(),
        Some(v) if crate::pyfmt::truthy(v) => crate::pyfmt::str_of(v),
        _ => String::new(),
    };
    for seg in FB_SEG.split(&cmd) {
        if (FB_GIT.is_match(seg) && FB_PUSH.is_match(seg)) || seg.contains("git-send-pack") {
            return Some("push／send-pack");
        }
    }
    if FB_MERGE.is_match(&cmd) {
        return Some("gh pr merge");
    }
    if FB_APPROVE.is_match(&cmd) {
        return Some("核准 PR（--approve）");
    }
    if FB_GH_API.is_match(&cmd) && FB_API_WRITE.is_match(&cmd) {
        return Some("gh api 寫入");
    }
    if FB_WEB.is_match(&cmd) && FB_PR_WRITE.is_match(&cmd) {
        return Some("用 HTTP 客戶端合併或審查 PR");
    }
    // 樣式本身出問題時沒有辦法判斷：寧可擋下
    if re::fault() {
        return Some("無法判斷的指令");
    }
    None
}

fn fallback(mode: &str, data: &Value, class: &str) -> i32 {
    say(&format!("agent-workflow hook：守門模組載入失敗（{class}），改用最小規則；請更新或重裝 agent-workflow plugin。"));
    if mode != "guard-shell" {
        return 0;
    }
    if let Some(why) = fallback_shell(data) {
        say(&format!("agent-workflow 守門（最小規則）：擋下 {why}。守門模組恢復前，這類操作請由人在終端機自己執行。"));
        return 2;
    }
    0
}

// ───────── 主流程 ─────────

/// panic hook 要用的狀態：模式與已經讀進來的輸入。
static PANIC_CTX: OnceLock<(String, Option<Value>)> = OnceLock::new();
static PANIC_MODE: OnceLock<String> = OnceLock::new();

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|_| {
        // 不印預設的 panic 訊息；照失敗語意第 3 條結束。這裡再 panic 會直接中止，所以只做最單純的事。
        let mode = PANIC_MODE.get().cloned().unwrap_or_default();
        let code = match PANIC_CTX.get() {
            Some((m, Some(data))) if is_guard(m) => fallback(m, data, "InternalError"),
            _ if mode == "guard-shell" || mode == "guard-paths" => {
                say("agent-workflow 守門：檢查時發生意外錯誤（InternalError），為了安全先擋下。");
                2
            }
            _ => 0,
        };
        std::process::exit(code);
    }));
}

/// 除錯版才有的故障注入（整合測試用）：release 版沒有這段，環境變數不能拿來關掉守門。
#[cfg(debug_assertions)]
fn test_fault() -> String {
    std::env::var("AWF_HOOK_TEST_FAULT").unwrap_or_default()
}

#[cfg(not(debug_assertions))]
fn test_fault() -> String {
    String::new()
}

fn unexpected(mode: &str, e: &PyErr) -> i32 {
    if is_guard(mode) {
        say(&format!("agent-workflow 守門：檢查時發生意外錯誤（{}：{}），為了安全先擋下。", e.class, py::head(&e.msg, 200)));
        return 2;
    }
    say(&format!("agent-workflow 交件檢查：發生意外錯誤（{}），略過檢查。", e.class));
    0
}

fn load_guard() -> Result<Guard, PyErr> {
    let f = test_fault();
    if f == "load" {
        return Err(PyErr::new("RuntimeError", "boom"));
    }
    Guard::load()
}

fn run_guard(mode: &str, g: &Guard, data: &Value) -> guard::R<()> {
    let f = test_fault();
    if f == "panic" {
        panic!("故障注入");
    }
    if f == "check" {
        return guard::err("RuntimeError", "boom");
    }
    if let Some(t) = &g.cfg.type_fault {
        return guard::err("TypeError", t);
    }
    let is_read = matches!(data.get("tool_name").and_then(Value::as_str), Some("Read") | Some("Grep"));
    let budget_msg = if mode == "guard-shell" {
        g.check_shell_rules(data)?;
        read_budget::check_shell(data, &g.cfg, &g.ws)
    } else if is_read {
        g.check_read(data)?; // 機敏檔先擋
        read_budget::check_read(data, &g.cfg, &g.ws) // Read 記帳並擋大檔；Grep 的 content 模式也計入額度
    } else {
        g.check_paths_rules(data)?;
        None
    };
    if let Some(msg) = budget_msg {
        say(&format!("已被 agent-workflow hook 擋下：{msg}"));
        return Err(Stop::Exit(2));
    }
    Ok(())
}

fn dispatch(mode: &str) -> i32 {
    let data = match read_input(mode) {
        Ok(d) => d,
        Err(why) => {
            if is_guard(mode) {
                say(&format!("agent-workflow 守門：hook 輸入有問題（{why}），為了安全先擋下。"));
                return 2;
            }
            say(&format!("agent-workflow 交件檢查：hook 輸入有問題（{why}），略過檢查。"));
            return 0;
        }
    };
    let _ = PANIC_CTX.set((mode.to_string(), Some(data.clone())));

    if mode == "envelope" {
        let loaded = crate::config::workspace().and_then(|ws| crate::config::load(&ws).map(|cfg| (ws, cfg)));
        let (ws, cfg) = match loaded {
            Ok(x) => x,
            Err(e) => {
                // 交件檢查壞掉不能讓 agent 交不了件
                say(&format!("agent-workflow 交件檢查：模組載入失敗（{}），略過檢查。", e.class));
                return 0;
            }
        };
        if let Some(t) = &cfg.type_fault {
            return unexpected(mode, &PyErr::new("TypeError", t));
        }
        return match envelope::run(&data, &cfg, &ws) {
            Ok(()) => 0,
            Err(Stop::Exit(c)) => c,
            Err(Stop::Err(e)) => unexpected(mode, &e),
        };
    }

    let g = match load_guard() {
        Ok(g) => g,
        Err(e) => return fallback(mode, &data, &e.class),
    };
    let code = match run_guard(mode, &g, &data) {
        Ok(()) => 0,
        Err(Stop::Exit(c)) => c,
        Err(Stop::Err(e)) => return unexpected(mode, &e),
    };
    if code == 0 && re::fault() {
        return unexpected(mode, &PyErr::new("RuntimeError", "樣式比對失敗，無法確認這個動作是否允許"));
    }
    code
}

/// `awf hook <種類>`：從 stdin 讀 hook 輸入，回傳 exit code。
pub fn main(args: &[String]) -> i32 {
    let mode = args.first().map(String::as_str).unwrap_or("");
    let _ = PANIC_MODE.set(mode.to_string());
    install_panic_hook();
    if args.len() == 1 {
        // 提示與自動接點：永遠 exit 0（出錯、逾時都只是不提示）
        match mode {
            "session-start" => return session_start::run(),
            "mark-dirty" => return auto::mark_dirty(),
            "prompt-context" => return auto::prompt_context(),
            _ => {}
        }
    }
    if args.len() != 1 || !MODES.contains(&mode) {
        say(&format!("用法：awf hook <{}> < hook-input.json", MODES.join("|")));
        return 1;
    }
    start_deadline(mode);
    dispatch(mode)
}

/// 守門類 hook 的內部期限（毫秒）。plugin 的 hooks.json 給 10 秒、逾時算「跑不起來」而放行；
/// 一條切得出上千段的長指令可以把檢查拖過 10 秒，所以在那之前自己擋下。0.x 的 Python 版沒有這一層（只會更嚴）。
const GUARD_DEADLINE_MS: u64 = 8000;

/// 交件類（envelope）與提示類不設期限：它們出問題時本來就放行。
fn start_deadline(mode: &str) {
    if !matches!(mode, "guard-shell" | "guard-paths") {
        return;
    }
    let ms = std::env::var("AWF_HOOK_DEADLINE_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0).unwrap_or(GUARD_DEADLINE_MS);
    let _ = std::thread::Builder::new().name("awf-hook-deadline".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        eprintln!("AWF 守門：這次檢查超過 {} 秒還沒做完，為了安全先擋下。請把指令拆短（一次少做幾件事）再執行。", ms.div_ceil(1000));
        std::process::exit(2);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sh(cmd: &str) -> Value {
        json!({"tool_name": "Bash", "tool_input": {"command": cmd}})
    }

    #[test]
    fn patterns_compile() {
        for r in fallback_patterns().into_iter().chain(guard_paths::all_patterns()) {
            assert!(r.compiles(), "樣式編不過：{}", r.source());
        }
    }

    #[test]
    fn minimal_rules() {
        for c in ["ls -la", "git status", "git commit -m x", "git log --oneline", "gh pr view 3", "gh api repos/acme-fork/app/pulls",
                  "curl https://example.com/merge-sort.html"] {
            assert_eq!(fallback_shell(&sh(c)), None, "{c}");
        }
        for c in ["git push origin HEAD:feature/x", "cd repos/app && git -C . push --force", "git send-pack origin main",
                  "gh pr merge 3 --squash", "gh pr review 3 --approve", "gh pr review 3 -a",
                  "gh api -X PUT repos/acme-fork/app/pulls/3/merge", "gh api graphql -f query='mutation{mergePullRequest}'",
                  "curl -X PUT https://api.github.com/repos/acme-fork/app/pulls/3/merge",
                  "curl -d '{\"event\":\"APPROVE\"}' https://api.github.com/repos/a/b/pulls/3/reviews"] {
            assert!(fallback_shell(&sh(c)).is_some(), "{c}");
        }
    }
}
