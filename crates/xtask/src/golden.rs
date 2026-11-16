//! golden set：schema 的正例、反例、語意反例。原本是 0.x 的 Python 工具 `run.py`。
//!
//! 範例在 `golden-set/valid/*.json` 與 `golden-set/invalid/*.json`（只看這兩個目錄的這一層；`recipes/`、
//! `token-budget.json` 是別的檢查的資料，和 run.py 一樣不看）。範例檔的鍵：
//!   - `payload`：信封＋該 payload；`schema`：獨立文件（例如 methodology），直接用那份 schema 驗。
//!   - `expect`（選填）：錯誤訊息要含這段文字（確認是被對的規則擋下）。
//!   - `config`（選填）：這一例用的測試專用設定（覆蓋 guard.config.json，例如 task_id_prefix）。
//!   - `semantic`（選填）：語意反例，schema 本身擋不住、由語意規則擋。
//!
//! 另外把 `methodologies/*.json`（內建 profile）與 `methodology.json`（有的話）當正例。
//!
//! 每個範例的判定（全部成立才算通過）：
//!   1. 用到的 schema 只用驗證器認得的關鍵字。
//!   2. 有 `expect` 時，錯誤訊息裡要有它。
//!   3. 正例零錯誤；反例要有錯誤。
//!   4. 不是語意反例的反例，光靠 schema（不算語意規則）就要擋得住。
//!
//! 和 0.x 的 Python 版不同的地方：0.x 的 Python 版另外拿官方的 `jsonschema` 套件對每個範例再驗一次，確認兩個驗證器對 schema 部分的
//! 判斷一致。這裡只有 awf-core 自己的驗證器（不為了檢查多帶一個相依）；第 4 條對應 0.x 的 Python 版「官方 jsonschema 應該擋
//! 但通過」那一項。兩個驗證器的逐例對照留在 `tests/parity/schema.expected.json`（awf-core 的 schema_parity 測試）。
//! 範例檔讀不懂、缺 `data`、既沒有 `payload` 也沒有 `schema`：0.x 的 Python 版當掉，這裡算那一例失敗。
//!
//! 判定邏輯只有一份，在 [`awf_core::golden`]（`awf golden` 用同一份）；這裡只決定目錄（模版的 `.claude/teams/golden-set`）與輸出格式。
//!
//! 用法：`cargo xtask golden [--verbose] [--root <目錄>]`（`--verbose` 每個範例印一行判定）。

use crate::util::{self, Captured};
use std::path::Path;

pub use awf_core::golden::CaseResult;

/// 跑全部範例：回每一例的結果，順序同 run.py（valid、invalid、內建 profile、專案 profile）。
pub fn run_all(root: &Path) -> Vec<CaseResult> {
    awf_core::golden::run_all(&util::teams_dir(root).join("golden-set"))
}

/// 指令本體：`root` 是 repo 根目錄，`args` 是 `--verbose`。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let results = run_all(root);
    let mut out = String::new();
    let (mut ok, mut bad) = (0, 0);
    let count = |kind: &str| results.iter().filter(|r| r.kind == kind).count();
    for r in &results {
        if r.problems.is_empty() {
            ok += 1;
            if verbose {
                out.push_str(&format!("ok   {}/{}（錯誤 {}，其中 schema {}）\n", r.kind, r.name, r.errors, r.schema_errors));
            }
        } else {
            bad += 1;
            out.push_str(&format!("FAIL {}/{}：{}\n", r.kind, r.name, r.problems.join("；")));
        }
    }
    if results.is_empty() {
        out.push_str("golden set：沒有任何範例（golden-set/valid、invalid 是空的？）\n");
        return (1, out, String::new());
    }
    out.push_str(&format!("golden set：通過 {ok}，失敗 {bad}（正例 {}、反例 {}）\n", count("valid"), count("invalid")));
    (if bad == 0 { 0 } else { 1 }, out, String::new())
}
