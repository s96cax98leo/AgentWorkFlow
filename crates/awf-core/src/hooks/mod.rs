//! hook 的入口與規則（guard-shell、guard-paths、envelope、session-start）。由 `plugin/hooks/*.py` 移植。

/// `awf hook <種類>`：從 stdin 讀 hook 輸入，回傳 exit code。
pub fn main(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("awf hook：還沒移植");
    1
}
