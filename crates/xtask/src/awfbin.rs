//! 建置並呼叫這個 repo 的 `awf` 執行檔（check-all 的 selftest、bench 的備資料包用）。
//! 大部分檢查直接在行程內呼叫 awf-core；只有「要測的就是執行檔本身」或「工具以行程的目前目錄為基準」時才起子程序。

use crate::util;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `awf` 執行檔的位置：和 xtask 同一個輸出目錄（`target/debug` 或 `target/release`）。
pub fn exe() -> PathBuf {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    dir.join(if cfg!(windows) { "awf.exe" } else { "awf" })
}

/// `cargo build -p awf`（已經是最新時很快）。回執行檔的位置；建不起來回說明。
pub fn build(root: &Path) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let release = std::env::current_exe().ok().is_some_and(|p| p.parent().and_then(|d| d.file_name()).is_some_and(|n| n == "release"));
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "-q", "-p", "awf"]).current_dir(root);
    if release {
        cmd.arg("--release");
    }
    let r = util::run(&mut cmd, None)?;
    let exe = exe();
    if r.code != 0 || !exe.is_file() {
        return Err(format!("{}{}（cargo build -p awf exit {}；預期的執行檔：{}）", r.stdout, r.stderr, r.code, exe.display()));
    }
    Ok(exe)
}

/// 在 `cwd` 下跑 `awf` 的指令。會影響結果的環境變數先拿掉：專案與 plugin 的位置一律由目前目錄與參數決定。
pub fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new(exe());
    cmd.current_dir(cwd);
    for k in ["CLAUDE_PROJECT_DIR", "AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT", "AWF_TEMPLATE_DIR", "AWF_NOW", "AWF_TRANSCRIPTS_DIR", "AWF_BIN"] {
        cmd.env_remove(k);
    }
    cmd
}
