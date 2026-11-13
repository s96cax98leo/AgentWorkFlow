//! 驗證一份交件信封：schema＋交接語意規則。原本是 0.x 的 Python 工具 `check_envelope.py`。
//!
//! 訊息文字是對跑基準的一部分（`tests/parity/small.expected.json`）；裡面提到的舊工具路徑等引用全部改寫時一起換。

use crate::pyfmt::str_opt;
use crate::schema::{extract_json, Schemas};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "驗證一份交件信封：schema＋交接語意規則。

用法：
    awf check-envelope <file.json> [--payload findings]
    echo '<回報文字，最後一個 ```json 區塊是信封>' | awf check-envelope - [--payload ...]
不帶 --payload 時，若信封裡有 \"payload_type\" 以外的提示也不猜，只驗信封本體。
通過 exit 0；不通過 exit 1，原因印到 stderr。
";

/// 專案的 `.claude/teams`：`CLAUDE_PROJECT_DIR`，沒有就用目前目錄。
pub fn default_teams() -> PathBuf {
    let root = std::env::var("CLAUDE_PROJECT_DIR").ok().filter(|s| !s.is_empty()).map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."));
    root.join(".claude").join("teams")
}

/// 回（exit code，stdout，stderr）。`stdin` 只在來源是 `-` 時才會被呼叫。
pub fn run_captured(argv: &[String], teams: &Path, stdin: &mut dyn FnMut() -> String) -> (i32, String, String) {
    let Some(src) = argv.first() else {
        return (2, String::new(), format!("{USAGE}\n"));
    };
    let payload = match argv.iter().position(|a| a == "--payload") {
        Some(i) => match argv.get(i + 1) {
            Some(p) => Some(p.as_str()),
            None => return (1, String::new(), "awf check-envelope：--payload 後面要接 payload 名稱\n".into()),
        },
        None => None,
    };
    let text = if src == "-" {
        stdin()
    } else {
        match std::fs::read(src) {
            Ok(b) => match String::from_utf8(b) {
                Ok(s) => s,
                Err(_) => return (1, String::new(), format!("awf check-envelope：{src} 不是 UTF-8 文字\n")),
            },
            Err(e) => return (1, String::new(), format!("awf check-envelope：讀不到 {src}：{e}\n")),
        }
    };
    // Python 的 json.loads("null") 回 None，和「找不到」同一條路
    let Some(env) = extract_json(&text).filter(|v| !v.is_null()) else {
        return (1, String::new(), "找不到交件信封：回報最後要有一個 ```json 區塊（格式見 .claude/teams/schemas/envelope.schema.json）\n".into());
    };
    let errs = match Schemas::new(teams).check(&env, payload) {
        Ok(e) => e,
        Err(e) => return (1, String::new(), format!("awf check-envelope：{e}\n")),
    };
    if !errs.is_empty() {
        let mut err = String::from("交件信封不合格（awf check-envelope）：\n");
        for e in &errs {
            err.push_str(&format!("- {e}\n"));
        }
        return (1, String::new(), err);
    }
    (0, format!("信封合格：status={}，confidence={}\n", str_opt(env.get("status")), str_opt(env.get("confidence"))), String::new())
}

/// `awf check-envelope …`
pub fn main(argv: &[String]) -> i32 {
    let mut stdin = || {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        s
    };
    let (code, out, err) = run_captured(argv, &default_teams(), &mut stdin);
    print!("{out}");
    eprint!("{err}");
    code
}
