//! `awf save-run --selftest`：在暫存目錄跑，不碰 repo。項目名稱與數量和 save_run.py 的 selftest 一樣。

use super::Ctx;

/// 跑自我測試，結果寫進 `out`；全部通過回 0，否則 1。
pub fn selftest(_ctx: &Ctx, out: &mut String) -> i32 {
    out.push_str("selftest 尚未移植\n");
    1
}
