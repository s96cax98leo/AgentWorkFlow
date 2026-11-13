//! 原始碼雜湊：`crates/awf/build.rs`（編進執行檔，`awf --version` 印出來）與 `cargo xtask checksums` 共用同一份算法。
//! build.rs 用 `#[path]` 直接把這個檔當模組編進去，所以這裡只能用標準庫與 sha2，不能引用 xtask 或 awf-core 的東西。
//!
//! 範圍：`crates/` 底下的 `.rs` 與 `Cargo.toml`（略過名叫 `target` 的目錄）、根目錄的 `Cargo.toml` 與 `Cargo.lock`。
//! 路徑用 `/` 分隔並排序、內容把 CRLF 換成 LF，三個平台算出來的值相同。

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// 雜湊取 sha256 十六進位的前幾碼。
pub const SOURCE_HASH_LEN: usize = 16;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") || p.file_name().is_some_and(|n| n == "Cargo.toml") {
            out.push(p);
        }
    }
}

/// `root` 是 repo 根目錄。回 sha256 的前 16 碼。
pub fn source_hash(root: &Path) -> String {
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    for name in ["Cargo.toml", "Cargo.lock"] {
        let p = root.join(name);
        if p.is_file() {
            files.push(p);
        }
    }
    let mut items: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
            let body = fs::read(&p).ok()?;
            let body: Vec<u8> = String::from_utf8_lossy(&body).replace("\r\n", "\n").into_bytes();
            Some((rel, body))
        })
        .collect();
    items.sort();
    let mut h = Sha256::new();
    for (rel, body) in &items {
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update(body);
        h.update([0u8]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(SOURCE_HASH_LEN).collect()
}
