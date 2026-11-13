//! AgentWorkFlow 的核心函式庫：設定、schema 驗證、recipe、守門規則、狀態機、知識圖。
//!
//! 原則：閘門與守門的每一個判定、exit code、訊息文字，都和 0.x 的 Python 版錄下來的回歸基準逐字相同
//! （`tests/parity/`）。行為只能在確實該變的時候改，而且只能變嚴。

pub mod build;
pub mod check_agents;
pub mod check_envelope;
pub mod check_rules;
pub mod checklist;
pub mod config;
pub mod cost;
pub mod deliverables;
pub mod doctor;
pub mod hooks;
pub mod init;
pub mod kb;
pub mod locate;
pub mod merge_gate;
pub mod pack;
pub mod publish;
pub mod pyfmt;
pub mod recipe;
pub mod run_brief;
pub mod save_run;
pub mod schema;
pub mod script;
pub mod spec;
pub mod state;
pub mod sync_branch;
pub mod template;

/// 版本（workspace 的版本）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod watch;
