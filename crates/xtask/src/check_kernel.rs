//! 執行核心（`snippets/kernel.js`）的單元檢查：workflow 的情境走不到的行為直接對片段測。原本是 0.x 的 Python 工具 `check_kernel.py`。
//!
//! 用 node 載入 challenge、recipe、kernel 三個片段，給假的 agent()；測試本體在 `js/kernel.cjs`（原樣搬來），檢查：
//!   1. label 重複時第二次起自動加序號（trace、紀錄檔、失敗升級都靠 label 對，不能撞）。
//!   2. label 重複又碰到失敗升級時，標 superseded 的是自己那一筆。
//!   3. 交件的 status 與 summary 記進 trace 的 digest。
//!   4. 派工數到上限就不再派，label 記進 capped，trace 記一筆 capped=true。
//!   5. 限制文件（challenge 片段的 awfConstraints）。
//!   6. 片段裡的派工（challengeList）在啟用核心後也經過它。
//!   7. dev-integrator、dev-merge-steward 與 retry:false 的派工失敗不自動重派。
//!
//! 沒有 node 時判失敗（不略過）。和 0.x 的 Python 版不同的地方（只會更嚴）：node 的輸出不是 JSON 陣列時判失敗
//! （0.x 的 Python 版只要是合法 JSON 就逐項印出）。

use crate::util::{self, Captured};
use awf_core::merge_gate::pyutil;
use awf_core::pyfmt;
use serde_json::Value;
use std::path::Path;

const JS: &str = include_str!("js/kernel.cjs");
const SNIPPETS: [&str; 3] = ["challenge", "recipe", "kernel"];

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path) -> Captured {
    let Some(node) = util::which("node") else {
        return (1, "check_kernel：找不到 node（fail-closed）\n".into(), String::new());
    };
    let dir = util::snippet_dir(root);
    let files: Vec<_> = SNIPPETS.iter().map(|n| dir.join(format!("{n}.js"))).collect();
    let q = match util::node_eval(&node, JS, &files, None) {
        Ok(q) => q,
        Err(e) => return (1, format!("check_kernel：node 執行失敗：{e}\n"), String::new()),
    };
    if q.code != 0 {
        return (1, format!("check_kernel：node 執行失敗：{}\n", pyutil::tail(&q.stderr, 800)), String::new());
    }
    let errs = match serde_json::from_str::<Value>(&q.stdout) {
        Ok(Value::Array(a)) => a,
        _ => return (1, format!("check_kernel：輸出讀不懂：{}\n", pyutil::tail(&q.stdout, 400)), String::new()),
    };
    let mut out = String::new();
    for e in &errs {
        out.push_str(&format!("  [失敗] {}\n", pyfmt::str_of(e)));
    }
    if errs.is_empty() {
        out.push_str("check_kernel：通過（label 去重、升級標對筆、digest、派工數上限、限制文件）\n");
        (0, out, String::new())
    } else {
        out.push_str(&format!("check_kernel：{} 項失敗\n", errs.len()));
        (1, out, String::new())
    }
}
