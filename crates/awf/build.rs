// 把原始碼的雜湊編進執行檔：CI 用它確認 commit 進去的執行檔和當下的原始碼一致。
// 算法在 crates/xtask/src/source_hash.rs，和 `cargo xtask checksums` 共用同一個檔
// （用 #[path] 直接編進來，build.rs 不相依 xtask 或 awf-core）。
use std::path::Path;

#[allow(dead_code)]
#[path = "../xtask/src/source_hash.rs"]
mod source_hash;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    println!("cargo:rustc-env=AWF_SOURCE_HASH={}", source_hash::source_hash(&root));
    println!("cargo:rerun-if-changed=../../crates");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
}
