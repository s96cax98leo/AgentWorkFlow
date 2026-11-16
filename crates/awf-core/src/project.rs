//! 專案根目錄在哪：所有子指令共用的一套解析。
//!
//! 0.x 的工具檔放在專案的 `.claude/teams/tools/` 裡，從自己的位置就知道專案在哪，和「在哪個目錄執行」無關。
//! 1.0 的工具是 plugin 裡的單一執行檔，沒有這個錨點；只看目前目錄的話，在子目錄執行就會換一份設定、schema 與 profile
//! （子目錄可以是 PR 的 checkout，內容不可信）。所以順序固定是（[`locate`]）：
//!
//! 1. 明確指定：`--project-dir`（各子指令自己的旗標，例如 kb、kg、program 的 `--root`）。
//! 2. 環境變數 `CLAUDE_PROJECT_DIR`（Claude Code 對 hook 會設；Bash 工具裡沒有）。
//! 3. 從目前目錄往上找含 `.claude/teams/VERSION`（`awf init` 寫的）的目錄，取**最外層**那一個：
//!    最近的那個可能是子目錄自帶的另一份 `.claude/teams/`。路徑上有不只一個時附一句說明（[`Found::note`]）。
//!
//! 找不到時：閘門類（merge-gate、save-run 的 stage／approve／escalate、publish）用 [`require`]，報錯、不退回目前目錄；
//! 輔助類用 [`root_or_cwd`]，維持原本「用目前目錄」的行為。
//!
//! hook 那一側（`config::workspace`）是 `CLAUDE_PROJECT_DIR`，沒有就用目前目錄；Claude Code 執行 hook 時一定會設這個變數，
//! 所以只要 Claude Code 的專案就是最外層有 VERSION 的那個目錄，兩邊是同一個專案根（合併稽核紀錄的寫入與讀取位置因此一致）。

use crate::kb::pyish;
use std::path::Path;

/// 專案根的記號：`awf init` 寫的版本檔。
pub const MARKER: &str = ".claude/teams/VERSION";
/// 指定專案根的環境變數。
pub const ENV: &str = "CLAUDE_PROJECT_DIR";

/// 專案根是怎麼決定的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// 明確指定（`--project-dir`）。
    Explicit,
    /// 環境變數 `CLAUDE_PROJECT_DIR`。
    Env,
    /// 從目前目錄往上找到的。
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// 專案根目錄（絕對路徑、已解開符號連結、用 `/`）。
    pub root: String,
    pub how: How,
    /// 往上找時路徑上有不只一個專案根：說明用了哪一個（給 stderr）。
    pub note: Option<String>,
}

fn resolve(p: &str, cwd: &str) -> String {
    let abs = if pyish::is_abs(p) { p.to_string() } else { pyish::pjoin(cwd, p) };
    pyish::resolve(&abs).unwrap_or(abs)
}

fn has_marker(dir: &Path) -> bool {
    dir.join(MARKER).is_file()
}

/// `dir` 往上（含自己）每一個含記號的目錄，由近到遠。
pub fn roots_above(dir: &str) -> Vec<String> {
    Path::new(dir).ancestors().filter(|d| has_marker(d)).map(|d| d.to_string_lossy().replace('\\', "/")).collect()
}

/// 解析專案根：`explicit` → `env` → 從 `cwd` 往上找最外層的記號。空字串等於沒給。找不到回 None。
pub fn locate(explicit: Option<&str>, env: Option<&str>, cwd: &str) -> Option<Found> {
    if let Some(e) = explicit.filter(|e| !e.is_empty()) {
        return Some(Found { root: resolve(e, cwd), how: How::Explicit, note: None });
    }
    if let Some(e) = env.filter(|e| !e.is_empty()) {
        return Some(Found { root: resolve(e, cwd), how: How::Env, note: None });
    }
    let start = resolve(".", cwd);
    let roots = roots_above(&start);
    let root = roots.last()?.clone();
    let note = (roots.len() > 1).then(|| {
        let inner: Vec<&str> = roots.iter().take(roots.len() - 1).map(String::as_str).collect();
        format!("目前目錄往上有 {} 個專案根（{MARKER}）：用最外層的 {root}，不用 {}；要指定別的請設 {ENV} 或帶 --project-dir", roots.len(), inner.join("、"))
    });
    Some(Found { root, how: How::Up, note })
}

fn cwd() -> String {
    std::env::current_dir().ok().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|| ".".to_string())
}

/// 用行程的環境解析（見 [`locate`]）。
pub fn find(explicit: Option<&str>) -> Option<Found> {
    locate(explicit, std::env::var(ENV).ok().as_deref(), &cwd())
}

fn say(f: &Found) {
    if let Some(n) = &f.note {
        eprintln!("awf：{n}");
    }
}

/// 找不到專案根時給人看的說明。
pub fn not_found_message() -> String {
    format!("找不到專案根：目前目錄往上沒有 {MARKER}（awf init 寫的），也沒有設 {ENV}。請在已套用的專案裡執行，或帶 --project-dir")
}

/// 閘門類用：找不到就回錯誤（不退回目前目錄）。路徑上有不只一個專案根時在 stderr 印一行說明。
pub fn require(explicit: Option<&str>) -> Result<String, String> {
    match find(explicit) {
        Some(f) => {
            say(&f);
            Ok(f.root)
        }
        None => Err(not_found_message()),
    }
}

/// 輔助類用：找不到就用目前目錄（原本的行為）。路徑上有不只一個專案根時在 stderr 印一行說明。
pub fn root_or_cwd(explicit: Option<&str>) -> String {
    match find(explicit) {
        Some(f) => {
            say(&f);
            f.root
        }
        None => {
            let c = cwd();
            pyish::resolve(&c).unwrap_or(c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("awf-project-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建立暫存目錄");
        d.canonicalize().expect("解開暫存目錄").to_string_lossy().replace('\\', "/")
    }

    fn mark(dir: &str) {
        let p = Path::new(dir).join(MARKER);
        std::fs::create_dir_all(p.parent().expect("上層")).expect("建立目錄");
        std::fs::write(p, "1.0.0\n").expect("寫 VERSION");
    }

    #[test]
    fn explicit_then_env_then_outermost_marker() {
        let t = tmp("order");
        let (outer, inner) = (format!("{t}/outer"), format!("{t}/outer/repos/app"));
        let deep = format!("{inner}/src");
        std::fs::create_dir_all(&deep).expect("建立目錄");
        // 沒有記號：找不到
        assert_eq!(locate(None, None, &deep), None);
        assert_eq!(locate(Some(""), Some(""), &deep), None);
        mark(&outer);
        let f = locate(None, None, &deep).expect("往上找得到");
        assert_eq!((f.root.as_str(), f.how, f.note.is_none()), (outer.as_str(), How::Up, true));
        // 子目錄自帶一份：仍然用最外層，並說明
        mark(&inner);
        let f = locate(None, None, &deep).expect("往上找得到");
        assert_eq!((f.root.as_str(), f.how), (outer.as_str(), How::Up));
        let note = f.note.expect("要有說明");
        assert!(note.contains(&outer) && note.contains(&inner), "{note}");
        // 環境變數蓋過往上找；明確指定蓋過環境變數；相對路徑以 cwd 為基準
        let f = locate(None, Some(&inner), &deep).expect("env");
        assert_eq!((f.root.as_str(), f.how), (inner.as_str(), How::Env));
        let f = locate(Some(".."), Some(&outer), &deep).expect("explicit");
        assert_eq!((f.root.as_str(), f.how), (inner.as_str(), How::Explicit));
        // 只有 .claude/teams 目錄、沒有 VERSION 檔的不算
        let fake = format!("{t}/fake/sub");
        std::fs::create_dir_all(format!("{t}/fake/.claude/teams/VERSION")).expect("建立目錄");
        std::fs::create_dir_all(&fake).expect("建立目錄");
        assert_eq!(locate(None, None, &fake), None);
        let _ = std::fs::remove_dir_all(&t);
    }
}
