//! 只給這個 repo 用的維護工具（`cargo xtask <指令>`）的本體；`main.rs` 只分派指令。

pub mod awfbin;
pub mod bench;
pub mod bench_fixture;
pub mod check_all;
pub mod check_budget;
pub mod check_docs;
pub mod check_kernel;
pub mod check_patterns;
pub mod check_recipes;
pub mod check_supply;
pub mod checksums;
pub mod dist;
pub mod gen_legacy;
pub mod golden;
pub mod launchers;
pub mod pyimports;
pub mod pyre;
pub mod scrub;
pub mod source_hash;
pub mod tempproj;
pub mod util;

use std::path::PathBuf;

/// repo 根目錄：xtask 只在這個 repo 裡編譯與執行，所以用編譯時的位置推算。
pub fn repo_root() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    p.canonicalize().unwrap_or(p)
}
