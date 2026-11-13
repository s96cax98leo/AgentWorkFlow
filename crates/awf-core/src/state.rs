//! 狀態機：主 session 每段只做 next 與 done。由 `scaffold/.claude/teams/tools/awf.py` 移植。`args[0]` 是子指令名。

/// `awf next／done／approve／render／handoff …`
pub fn main(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("awf next／done／approve／render／handoff：還沒移植");
    1
}
