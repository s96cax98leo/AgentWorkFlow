//! 交付文件：型錄、交付設定、逐節的來源標記與過期判定、需求追溯矩陣。
//! 原本是 0.x 的 Python 工具 `deliverables.py`，輸出、結束碼、訊息文字、寫出的檔案內容和 0.x 的 Python 版逐字相同
//! （對跑基準在 `tests/parity/deliverables.expected.json`；訊息裡的指令已改成 `awf deliverables`，基準檔的對應字串一起換過）。
//!
//! 設計：
//! - 交付什麼由專案決定：`.claude/teams/deliverables.json`（configure 在人確認後寫）；沒有時看 profile 的
//!   `deliverables`（要 `enabled: true`）。兩者都沒有就什麼都不做，流程也不會多派 agent。
//! - 型錄（`.claude/teams/deliverables/catalog.json`，沒有時用執行檔內建的）只是可選的清單與預設內容。
//! - 每個小節標題下一行是來源標記 `<!-- awf:source <glob>; <glob> -->`。來源檔的雜湊記在
//!   `.claude/teams/deliverables.state.json`（team agent 寫不到），來源改了而內文沒動，那一節就是過期。
//! - 這是輔助工具：閘門（save_run、publish、merge_gate）不讀它的輸出。
//!
//! 專案根目錄：原本的工具用「工具往上三層」；單一執行檔沒有這個位置，改用全部子指令共用的解析（`crate::project`）。
//!
//! 隱私規則（`privacy_rules`、`privacy_text`、`run_name_rule`）只能加、不能拿掉：交付文件是給外部的人看的。

mod core;
mod flow;
mod html5;
mod kgflow;
mod py;
mod selftest;
mod selftest_v1;

pub use self::core::{
    body_hash, broken_links, builtin_catalog, catalog_index, check_all, config_errors, doc_path, doc_sections, doc_status, glob_re, index_text, is_placeholder,
    doc_status_with, load_catalog, load_config, load_state, match_files, out_dir, parse_doc, privacy_hits, privacy_text, repo_files, resolve_docs, run_name_rule, safe_rel,
    save_state, section_sources, section_state, skeleton, source_root, status_all, ExtraRule, Parsed, Section,
};
pub use self::flow::{
    affected, changed_paths, configure, ensure_doc, entry_key, export, init_docs, matrix_block, matrix_rows, match_title, pack_args, pack_args_with, record_block, repo_of, run,
    run_facts, set_pandoc_override, stamp, strip_paths, suggest, sync, upsert_entry, write_matrix, Facts,
};

pub use self::kgflow::{project_matrix, Links};
pub use self::selftest::selftest;

pub const CONFIG_NAME: &str = "deliverables.json";
pub const STATE_NAME: &str = "deliverables.state.json";
pub const CATALOG_REL: &str = "deliverables/catalog.json";
pub const DEFAULT_DIR: &str = "docs/deliverables";
pub const INDEX_NAME: &str = "README.md";
pub const RECORD_ID: &str = "dev-record";
pub const MATRIX_ID: &str = "rtm";
pub const SUMMARY_NAME: &str = "交付摘要.md";
pub const PLACEHOLDER: &str = "（待補）";
pub const SOURCE_TYPES: [&str; 3] = ["auto", "assisted", "external"];
pub const WHENS: [&str; 3] = ["run", "milestone", "close"];
pub const DEPTHS: [&str; 2] = ["full", "summary"];
pub const STATUS_TEXT: [(&str, &str); 4] = [("ok", "齊全"), ("stale", "過期"), ("missing", "缺原料"), ("waiting", "等人補")];
const SKIP_DIRS: [&str; 11] = [".git", "node_modules", ".worktrees", ".venv", "venv", "__pycache__", "dist", "build", ".idea", ".codegraph", "graft"];
const MAX_FILES: usize = 20000;
const MAX_SECTION_FILES: usize = 300;
/// 內部 agent 的角色名（隱私規則用）。
const AGENT_ROLES: [&str; 16] = ["planner", "researcher", "reviewer", "skeptic", "writer", "spec-author", "acceptance-tester", "implementer", "integrator", "verifier",
    "design-reviewer", "security-reviewer", "merge-steward", "release-watcher", "upstream-analyst", "pr-reviewer"];

/// 原本的模組說明（沒給參數或 `--help` 時原樣印出）。
const DOC: &str = "交付文件：型錄、交付設定、逐節的來源標記與過期判定、需求追溯矩陣。

用法（在已 init 的專案根目錄）：
    awf deliverables configure [--preset <名>] [--suggest] [--import <清單檔>] [--add a,b] [--remove a,b]
                                      [--custom-unmatched] [--dir <目錄>] [--write --by <誰>] [--json]
    awf deliverables init                     # 依交付設定建目錄與文件骨架（已有的檔不動）
    awf deliverables status [--json]          # 每份文件：齊全、過期、缺原料、等人補
    awf deliverables check                    # 檔頭、過期小節、禁用字、隱私字串、失效連結、總目錄；有問題 exit 1
    awf deliverables index                    # 重寫總目錄（README.md）
    awf deliverables affected --run <run_dir> [--paths-file <檔>] [--milestone] [--close] [--json]
    awf deliverables matrix --run <run_dir> [--stdout]     # 需求追溯矩陣
    awf deliverables sync --run <run_dir> [--reviewed <文件id>#<小節>]…   # 併交付摘要、更新來源紀錄
    awf deliverables export [--out <目錄>]    # 有 pandoc 就轉 docx
    awf deliverables --selftest

設計：
- 交付什麼由專案決定：`.claude/teams/deliverables.json`（configure 在人確認後寫）；沒有時看 profile 的
  `deliverables`（要 `enabled: true`）。兩者都沒有就什麼都不做，流程也不會多派 agent。
- 型錄（`.claude/teams/deliverables/catalog.json`）只是可選的清單與預設內容；設定裡可以改寫任何欄位，也可以自訂文件。
- 每個小節標題下一行是來源標記 `<!-- awf:source <glob>; <glob> -->`。來源檔的雜湊記在
  `.claude/teams/deliverables.state.json`（team agent 寫不到），來源改了而內文沒動，那一節就是過期。
- 這是輔助工具：閘門（save_run、publish、merge_gate）不讀它的輸出。只用標準庫，跨平台。
";

/// 1.0 加的說明：接在 [`DOC`] 後面印（`DOC` 本身是對跑基準的一部分，逐字不動）。
const DOC_V1: &str = "
1.0（要有知識圖，awf kg build 建的；沒有圖時以下都不做，其他指令的行為不變）：
    awf deliverables matrix --project [--json]             # 專案層的追溯矩陣：每個現行需求目前由哪些檔案、測試、commit 支撐；沒有圖回非零
- sync 另外記下小節連到的需求、情境、符號與程式檔的指紋；之後它們變了而內文沒動，那一節也算過期（只會讓更多小節過期）。
";

/// 沒指定時的專案根目錄：全部子指令共用的解析（[`crate::project`]：`CLAUDE_PROJECT_DIR` → 從目前目錄往上找最外層的
/// `.claude/teams/VERSION`）；找不到就用目前的工作目錄（輔助工具的行為；閘門改用 `project::require`）。
pub fn default_root() -> String {
    crate::project::root_or_cwd(None)
}

/// `awf deliverables …`
pub fn main(args: &[String]) -> i32 {
    let mut out = String::new();
    let mut code = run(&default_root(), args, &mut out);
    if args.first().map(String::as_str) == Some("--selftest") {
        // 1.0 的案例接在原本的後面（原本那一段的輸出是對跑基準的一部分，不動）
        code = code.max(selftest_v1::selftest(&mut out));
    }
    if matches!(args.first().map(String::as_str), None | Some("-h") | Some("--help")) {
        out.push_str(DOC_V1);
    }
    print!("{out}");
    code
}
