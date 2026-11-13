//! AgentWorkFlow 的核心函式庫：設定、schema 驗證、recipe、守門規則、狀態機、知識圖。
//!
//! 移植原則：閘門與守門的每一個判定、exit code、訊息文字，都要和原本的 Python 版逐字相同
//! （對跑基準在 `tests/parity/`）。行為只能在確實該變的時候改，而且只能變嚴。

pub mod checklist;
pub mod config;
pub mod hooks;
pub mod kb;
pub mod pyfmt;
pub mod recipe;
pub mod schema;

/// 版本（workspace 的版本）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
