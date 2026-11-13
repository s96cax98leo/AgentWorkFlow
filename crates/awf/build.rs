// 把原始碼的雜湊編進執行檔：CI 用它確認 commit 進去的執行檔和當下的原始碼一致。
// 範圍：crates/ 底下的 .rs 與 Cargo.toml、根目錄的 Cargo.toml 與 Cargo.lock。
// 路徑用 / 分隔並排序、內容把 CRLF 換成 LF，三個平台算出來的值相同。
use sha2::{Digest, Sha256};
use std::{fs, path::{Path, PathBuf}};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") { continue; }
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") || p.file_name().is_some_and(|n| n == "Cargo.toml") {
            out.push(p);
        }
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    for name in ["Cargo.toml", "Cargo.lock"] {
        let p = root.join(name);
        if p.is_file() { files.push(p); }
    }
    let mut items: Vec<(String, Vec<u8>)> = files.into_iter().filter_map(|p| {
        let rel = p.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
        let rel = rel.trim_start_matches("../../").to_string();
        let body = fs::read(&p).ok()?;
        let body: Vec<u8> = String::from_utf8_lossy(&body).replace("\r\n", "\n").into_bytes();
        Some((rel, body))
    }).collect();
    items.sort();
    let mut h = Sha256::new();
    for (rel, body) in &items {
        h.update(rel.as_bytes()); h.update([0u8]); h.update(body); h.update([0u8]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=AWF_SOURCE_HASH={}", &hex[..16]);
    println!("cargo:rerun-if-changed=../../crates");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
}
