//! 交付文件：型錄、交付設定、逐節的來源標記與過期判定、需求追溯矩陣。
//! 由 `scaffold/.claude/teams/tools/deliverables.py` 移植，輸出、結束碼、訊息文字、寫出的檔案內容和 Python 版逐字相同
//! （對跑基準在 `tests/parity/deliverables.expected.json`；訊息裡的 `deliverables.py` 字樣照舊）。
//!
//! 設計：
//! - 交付什麼由專案決定：`.claude/teams/deliverables.json`（configure 在人確認後寫）；沒有時看 profile 的
//!   `deliverables`（要 `enabled: true`）。兩者都沒有就什麼都不做，流程也不會多派 agent。
//! - 型錄（`.claude/teams/deliverables/catalog.json`，沒有時用執行檔內建的）只是可選的清單與預設內容。
//! - 每個小節標題下一行是來源標記 `<!-- awf:source <glob>; <glob> -->`。來源檔的雜湊記在
//!   `.claude/teams/deliverables.state.json`（team agent 寫不到），來源改了而內文沒動，那一節就是過期。
//! - 這是輔助工具：閘門（save_run、publish、merge_gate）不讀它的輸出。
//!
//! 專案根目錄：原本的工具用「工具往上三層」；單一執行檔沒有這個位置，改成 `CLAUDE_PROJECT_DIR`，沒有就用目前的工作目錄。
//!
//! 隱私規則（`privacy_rules`、`privacy_text`、`run_name_rule`）只能加、不能拿掉：交付文件是給外部的人看的。

mod core;
mod flow;
mod html5;
mod py;
mod selftest;

pub use self::core::{
    body_hash, broken_links, builtin_catalog, catalog_index, check_all, config_errors, doc_path, doc_sections, doc_status, glob_re, index_text, is_placeholder,
    load_catalog, load_config, load_state, match_files, out_dir, parse_doc, privacy_hits, privacy_text, repo_files, resolve_docs, run_name_rule, safe_rel,
    save_state, section_sources, section_state, skeleton, source_root, status_all, ExtraRule, Parsed, Section,
};
pub use self::flow::{
    affected, changed_paths, configure, ensure_doc, entry_key, export, init_docs, matrix_block, matrix_rows, match_title, pack_args, record_block, repo_of, run,
    run_facts, set_pandoc_override, stamp, strip_paths, suggest, sync, upsert_entry, write_matrix, Facts,
};

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
    python3 deliverables.py configure [--preset <名>] [--suggest] [--import <清單檔>] [--add a,b] [--remove a,b]
                                      [--custom-unmatched] [--dir <目錄>] [--write --by <誰>] [--json]
    python3 deliverables.py init                     # 依交付設定建目錄與文件骨架（已有的檔不動）
    python3 deliverables.py status [--json]          # 每份文件：齊全、過期、缺原料、等人補
    python3 deliverables.py check                    # 檔頭、過期小節、禁用字、隱私字串、失效連結、總目錄；有問題 exit 1
    python3 deliverables.py index                    # 重寫總目錄（README.md）
    python3 deliverables.py affected --run <run_dir> [--paths-file <檔>] [--milestone] [--close] [--json]
    python3 deliverables.py matrix --run <run_dir> [--stdout]     # 需求追溯矩陣
    python3 deliverables.py sync --run <run_dir> [--reviewed <文件id>#<小節>]…   # 併交付摘要、更新來源紀錄
    python3 deliverables.py export [--out <目錄>]    # 有 pandoc 就轉 docx
    python3 deliverables.py --selftest

設計：
- 交付什麼由專案決定：`.claude/teams/deliverables.json`（configure 在人確認後寫）；沒有時看 profile 的
  `deliverables`（要 `enabled: true`）。兩者都沒有就什麼都不做，流程也不會多派 agent。
- 型錄（`.claude/teams/deliverables/catalog.json`）只是可選的清單與預設內容；設定裡可以改寫任何欄位，也可以自訂文件。
- 每個小節標題下一行是來源標記 `<!-- awf:source <glob>; <glob> -->`。來源檔的雜湊記在
  `.claude/teams/deliverables.state.json`（team agent 寫不到），來源改了而內文沒動，那一節就是過期。
- 這是輔助工具：閘門（save_run、publish、merge_gate）不讀它的輸出。只用標準庫，跨平台。
";

/// 沒指定時的專案根目錄：`CLAUDE_PROJECT_DIR`，沒有就用目前的工作目錄。
pub fn default_root() -> String {
    let raw = std::env::var("CLAUDE_PROJECT_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| ".".to_string());
    crate::kb::pyish::resolve(&raw).unwrap_or(raw)
}

/// `awf deliverables …`
pub fn main(args: &[String]) -> i32 {
    let mut out = String::new();
    let code = run(&default_root(), args, &mut out);
    print!("{out}");
    code
}
