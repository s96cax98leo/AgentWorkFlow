//! check-launchers、checksums、dist 檔名表的測試。
//!
//! check-launchers：對這個 repo 實跑要通過；在暫存副本裡把啟動器弄壞一處（挑錯檔、行尾、非內建指令、
//! 檔名、執行權限、.gitattributes）要報失敗、exit 1，而且訊息指得出是哪一項。
//! checksums：用假的執行檔（印出版本那一行的小 sh 腳本）走過寫入、核對、過期、損壞、缺檔。
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use xtask::util::TempDir;
use xtask::{checksums, dist, launchers, source_hash};

fn root() -> PathBuf {
    xtask::repo_root()
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// 暫存的 repo 根目錄，裡面只有兩個啟動器與 `.gitattributes`。
fn launcher_copy(tag: &str) -> TempDir {
    let tmp = TempDir::new(&format!("awf-xtask-{tag}-")).expect("暫存目錄");
    let bin = tmp.path().join("plugin/bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    for name in ["awf", "awf.cmd"] {
        std::fs::copy(root().join("plugin/bin").join(name), bin.join(name)).expect("copy");
    }
    std::fs::copy(root().join(".gitattributes"), tmp.path().join(".gitattributes")).expect("copy");
    tmp
}

/// 把檔案裡的 `from` 換成 `to`；`from` 一定要出現（否則測試沒有改到東西）。
fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("讀檔");
    assert!(text.contains(from), "{} 裡找不到要改的地方：{from}", path.display());
    std::fs::write(path, text.replace(from, to)).expect("寫檔");
}

/// 弄壞之後要失敗，而且失敗的那一行含 `needle`。
fn assert_fails(tmp: &TempDir, needle: &str) {
    let (rc, out, err) = launchers::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}{err}");
    assert!(err.contains("啟動器檢查失敗"), "{err}");
    assert!(out.lines().any(|l| l.contains("失敗") && l.contains(needle)), "失敗的項目裡沒有「{needle}」：\n{out}");
}

// ------------------------------------------------------------------ check-launchers

#[test]
fn launchers_pass_on_repo() {
    let (rc, out, err) = launchers::run_captured(&root());
    assert_eq!(rc, 0, "{out}{err}");
    assert!(out.contains("啟動器檢查通過："), "{out}");
    assert!(!out.contains("失敗"), "{out}");
    // 十種 uname 組合都實際跑過，而且 awf.cmd 在這裡只做靜態檢查。
    assert_eq!(out.lines().filter(|l| l.contains("awf（sh）：") && l.contains(" → awf-")).count(), 10, "{out}");
    assert!(out.contains("略過  awf.cmd：實際執行"), "{out}");
}

#[test]
fn launchers_copy_passes_unchanged() {
    // 副本原樣要通過：後面幾個「弄壞」的測試才代表是那一處造成失敗。
    let tmp = launcher_copy("launch-ok");
    let (rc, out, err) = launchers::run_captured(tmp.path());
    assert_eq!(rc, 0, "{out}{err}");
}

#[test]
fn launchers_catch_wrong_pick() {
    let tmp = launcher_copy("launch-pick");
    edit(&tmp.path().join("plugin/bin/awf"), "x86_64|amd64) arch=x64 ;;", "x86_64|amd64) arch=arm64 ;;");
    assert_fails(&tmp, "Darwin x86_64 → awf-darwin-x64");
    assert_fails(&tmp, "Linux x86_64 → awf-linux-x64");
}

#[test]
fn launchers_catch_lost_args_and_exit_code() {
    let tmp = launcher_copy("launch-args");
    // 少了引號：有空白的參數會被拆開、空字串會不見。
    edit(&tmp.path().join("plugin/bin/awf"), "exec \"$bin\" \"$@\"", "exec \"$bin\" $@");
    assert_fails(&tmp, "參數原樣傳到");

    let tmp = launcher_copy("launch-exit");
    edit(&tmp.path().join("plugin/bin/awf"), "exec \"$bin\" \"$@\"", "\"$bin\" \"$@\" || exit 1");
    assert_fails(&tmp, "exit code 42 原樣傳回");
}

#[test]
fn launchers_catch_symlink_and_awf_bin() {
    let tmp = launcher_copy("launch-link");
    edit(&tmp.path().join("plugin/bin/awf"), "while [ -L \"$self\" ]; do", "while false; do");
    assert_fails(&tmp, "經符號連結呼叫（絕對路徑）");

    let tmp = launcher_copy("launch-env");
    edit(&tmp.path().join("plugin/bin/awf"), "if [ -n \"${AWF_BIN:-}\" ]; then", "if false; then");
    assert_fails(&tmp, "AWF_BIN 有設就用它");
}

#[test]
fn launchers_catch_wrong_exit_for_missing() {
    let tmp = launcher_copy("launch-127");
    edit(&tmp.path().join("plugin/bin/awf"), "設成執行檔路徑\" >&2; exit 127; }", "設成執行檔路徑\" >&2; exit 1; }");
    assert_fails(&tmp, "執行檔不存在 → 127");
}

#[test]
fn launchers_catch_sh_static() {
    let tmp = launcher_copy("launch-mode");
    let sh = tmp.path().join("plugin/bin/awf");
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert_fails(&tmp, "awf：檔案有執行權限");

    let tmp = launcher_copy("launch-long");
    let sh = tmp.path().join("plugin/bin/awf");
    let mut text = std::fs::read_to_string(&sh).expect("讀檔");
    text.push_str(&"# 多的一行\n".repeat(launchers::SH_MAX_LINES));
    std::fs::write(&sh, text).expect("寫檔");
    assert_fails(&tmp, "awf：不超過");

    let tmp = launcher_copy("launch-syntax");
    edit(&tmp.path().join("plugin/bin/awf"), "self=$0\n", "self=$0\nif true; then\n");
    assert_fails(&tmp, "sh -n 語法檢查");
}

#[test]
fn launchers_catch_cmd_static() {
    let tmp = launcher_copy("cmd-lf");
    let cmd = tmp.path().join("plugin/bin/awf.cmd");
    edit(&cmd, "\r\n", "\n");
    assert_fails(&tmp, "awf.cmd：行尾都是 CRLF");

    let tmp = launcher_copy("cmd-name");
    edit(&tmp.path().join("plugin/bin/awf.cmd"), "awf-windows-x64.exe", "awf-windows-arm64.exe");
    assert_fails(&tmp, "awf.cmd：提到的檔名對得上檔名表");

    let tmp = launcher_copy("cmd-foreign");
    edit(&tmp.path().join("plugin/bin/awf.cmd"), "setlocal DisableDelayedExpansion\r\n", "setlocal DisableDelayedExpansion\r\npowershell -Command \"exit 0\"\r\n");
    assert_fails(&tmp, "第 7 行用了 powershell");

    let tmp = launcher_copy("cmd-ascii");
    edit(&tmp.path().join("plugin/bin/awf.cmd"), "rem Set AWF_BIN", "rem 設 AWF_BIN");
    assert_fails(&tmp, "awf.cmd：只有 ASCII");

    let tmp = launcher_copy("cmd-args");
    edit(&tmp.path().join("plugin/bin/awf.cmd"), "\"%bin%\" %*", "\"%bin%\"");
    assert_fails(&tmp, "缺 %*");
}

#[test]
fn launchers_catch_gitattributes() {
    let tmp = launcher_copy("attrs");
    edit(&tmp.path().join(".gitattributes"), "plugin/bin/awf.cmd text eol=crlf\n", "");
    assert_fails(&tmp, "缺 plugin/bin/awf.cmd text eol=crlf");

    let tmp = launcher_copy("attrs-bin");
    edit(&tmp.path().join(".gitattributes"), "plugin/bin/awf-linux-arm64 binary\n", "");
    assert_fails(&tmp, "缺 plugin/bin/awf-linux-arm64 binary");
}

// ------------------------------------------------------------------ dist 的檔名表

#[test]
fn dist_table_is_the_five_release_names() {
    assert_eq!(dist::file_for("aarch64-apple-darwin"), Some("awf-darwin-arm64"));
    assert_eq!(dist::file_for("x86_64-apple-darwin"), Some("awf-darwin-x64"));
    assert_eq!(dist::file_for("x86_64-unknown-linux-gnu"), Some("awf-linux-x64"));
    assert_eq!(dist::file_for("aarch64-unknown-linux-gnu"), Some("awf-linux-arm64"));
    assert_eq!(dist::file_for("x86_64-pc-windows-msvc"), Some("awf-windows-x64.exe"));
    assert_eq!(dist::file_for("x86_64-unknown-linux-musl"), None);
    assert_eq!(dist::file_names().len(), 5);
    // 跑測試的機器（CI 的三種 runner、開發機）都在表裡。
    assert!(dist::host_file().is_some());
}

#[test]
fn dist_rejects_unknown_target_and_args() {
    let (rc, _, err) = dist::run_captured(&root(), &strs(&["--target", "x86_64-unknown-linux-musl"]));
    assert_eq!(rc, 1);
    assert!(err.contains("不支援的 target「x86_64-unknown-linux-musl」"), "{err}");
    let (rc, _, err) = dist::run_captured(&root(), &strs(&["--release"]));
    assert_eq!(rc, 1);
    assert!(err.contains("不認得的參數「--release」"), "{err}");
}

// ------------------------------------------------------------------ checksums

/// 假的執行檔：`--version` 印出帶指定雜湊的那一行。`salt` 讓每個檔的內容（與 sha256）不同。
fn fake_bin(dir: &Path, name: &str, hash: &str, salt: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n# {salt}\necho \"awf 0.0.0 (source {hash})\"\n")).expect("寫檔");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn fake_all(dir: &Path, hash: &str) {
    for name in dist::file_names() {
        fake_bin(dir, name, hash, name);
    }
}

fn sums(dir: &Path, flags: &[&str]) -> (i32, String, String) {
    let mut args = strs(&["--dir", &dir.to_string_lossy()]);
    args.extend(strs(flags));
    checksums::run_captured(&root(), &args)
}

#[test]
fn checksums_empty_dir() {
    let tmp = TempDir::new("awf-xtask-sums-empty-").expect("暫存目錄");
    let (rc, out, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert!(out.contains("還沒有發佈用的執行檔"), "{out}");

    let (rc, _, err) = sums(tmp.path(), &["--check", "--require-all"]);
    assert_eq!(rc, 1);
    for name in dist::file_names() {
        assert!(err.contains(name), "缺檔清單要列出 {name}：{err}");
    }

    let (rc, _, err) = sums(tmp.path(), &["--write"]);
    assert_eq!(rc, 1);
    assert!(err.contains("沒有任何執行檔"), "{err}");
    assert!(!tmp.path().join(checksums::FILE).exists());
}

#[test]
fn checksums_repo_dir_passes_during_development() {
    // plugin/bin 在開發期間沒有執行檔（發佈前才 commit）；有的話就要和 checksums.json 一致。
    let (rc, out, err) = checksums::run_captured(&root(), &strs(&["--check"]));
    assert_eq!(rc, 0, "{out}{err}");
}

#[test]
fn checksums_write_then_check() {
    let tmp = TempDir::new("awf-xtask-sums-ok-").expect("暫存目錄");
    let hash = source_hash::source_hash(&root());
    assert_eq!(hash.len(), source_hash::SOURCE_HASH_LEN);
    fake_all(tmp.path(), &hash);

    let (rc, out, err) = sums(tmp.path(), &["--write", "--require-all"]);
    assert_eq!(rc, 0, "{out}{err}");
    let text = std::fs::read_to_string(tmp.path().join(checksums::FILE)).expect("checksums.json");
    // 鍵排序、縮排 2、結尾換行；重寫一次逐字相同。
    assert!(text.starts_with("{\n  \"files\": {\n    \"awf-darwin-arm64\": \""), "{text}");
    assert!(text.ends_with(&format!("  \"source\": \"{hash}\"\n}}\n")), "{text}");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let files = doc["files"].as_object().expect("files");
    let keys: Vec<&String> = files.keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(files.len(), 5);
    assert!(files.values().all(|v| v.as_str().is_some_and(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))));
    let (rc, _, _) = sums(tmp.path(), &["--write"]);
    assert_eq!(rc, 0);
    assert_eq!(std::fs::read_to_string(tmp.path().join(checksums::FILE)).expect("checksums.json"), text);

    let (rc, out, err) = sums(tmp.path(), &["--check", "--require-all"]);
    assert_eq!(rc, 0, "{out}{err}");
    assert!(out.contains("checksums 核對通過：5 個執行檔") && out.contains("實際執行的原始碼雜湊也一致"), "{out}");

    // 不加參數只印內容、不寫檔。
    let (rc, out, _) = sums(tmp.path(), &[]);
    assert_eq!(rc, 0);
    assert_eq!(out, text);
}

#[test]
fn checksums_catch_tampered_missing_and_extra() {
    let tmp = TempDir::new("awf-xtask-sums-bad-").expect("暫存目錄");
    let hash = source_hash::source_hash(&root());
    fake_all(tmp.path(), &hash);
    assert_eq!(sums(tmp.path(), &["--write"]).0, 0);

    // 檔案內容變了。
    fake_bin(tmp.path(), "awf-linux-x64", &hash, "被換掉");
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains("awf-linux-x64 的 sha256 是"), "{err}");
    fake_bin(tmp.path(), "awf-linux-x64", &hash, "awf-linux-x64");
    assert_eq!(sums(tmp.path(), &["--check"]).0, 0);

    // 少一個檔：json 還記著它；--require-all 另外列缺檔。
    std::fs::remove_file(tmp.path().join("awf-windows-x64.exe")).expect("rm");
    let (rc, _, err) = sums(tmp.path(), &["--check", "--require-all"]);
    assert_eq!(rc, 1);
    assert!(err.contains("缺執行檔：awf-windows-x64.exe") && err.contains("記了 awf-windows-x64.exe"), "{err}");
    // 只有部分檔、json 也照實寫時，不加 --require-all 會過，加了不過。
    assert_eq!(sums(tmp.path(), &["--write"]).0, 0);
    assert_eq!(sums(tmp.path(), &["--check"]).0, 0);
    let (rc, _, err) = sums(tmp.path(), &["--write", "--require-all"]);
    assert_eq!(rc, 1);
    assert!(err.contains("缺執行檔：awf-windows-x64.exe"), "{err}");

    // 有執行檔卻沒有 json。
    std::fs::remove_file(tmp.path().join(checksums::FILE)).expect("rm");
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains("--write"), "{err}");

    // 檔名不在表裡。
    fake_bin(tmp.path(), "awf-linux-riscv64", &hash, "x");
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains("不認得的檔 awf-linux-riscv64"), "{err}");
}

#[test]
fn checksums_catch_stale_source_and_lost_mode() {
    // json 的 source 過期（原始碼改了、執行檔沒重建）。
    let tmp = TempDir::new("awf-xtask-sums-stale-").expect("暫存目錄");
    let hash = source_hash::source_hash(&root());
    fake_all(tmp.path(), &hash);
    assert_eq!(sums(tmp.path(), &["--write"]).0, 0);
    let json = tmp.path().join(checksums::FILE);
    edit(&json, &hash, "0000000000000000");
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains("source 是 0000000000000000") && err.contains("執行檔過期了"), "{err}");

    // json 對、但本機平台的執行檔編進去的是別的雜湊。
    let tmp = TempDir::new("awf-xtask-sums-embedded-").expect("暫存目錄");
    fake_all(tmp.path(), "ffffffffffffffff");
    assert_eq!(sums(tmp.path(), &["--write"]).0, 0);
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains("--version 的原始碼雜湊是 ffffffffffffffff"), "{err}");

    // 執行權限不見了（下載的 artifact 不保留權限）。
    let tmp = TempDir::new("awf-xtask-sums-mode-").expect("暫存目錄");
    fake_all(tmp.path(), &hash);
    assert_eq!(sums(tmp.path(), &["--write"]).0, 0);
    let other = dist::file_names().into_iter().find(|n| !n.ends_with(".exe") && Some(*n) != dist::host_file()).expect("別的平台");
    std::fs::set_permissions(tmp.path().join(other), std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let (rc, _, err) = sums(tmp.path(), &["--check"]);
    assert_eq!(rc, 1);
    assert!(err.contains(&format!("{other} 沒有執行權限")), "{err}");
}

#[test]
fn checksums_reject_bad_flags() {
    let (rc, _, err) = checksums::run_captured(&root(), &strs(&["--write", "--check"]));
    assert_eq!(rc, 1);
    assert!(err.contains("只能擇一"), "{err}");
    let (rc, _, err) = checksums::run_captured(&root(), &strs(&["--verify"]));
    assert_eq!(rc, 1);
    assert!(err.contains("不認得的參數「--verify」"), "{err}");
}

// ------------------------------------------------------------------ 原始碼雜湊

#[test]
fn source_hash_ignores_line_endings_and_sees_changes() {
    let mk = |tag: &str, body: &str| {
        let tmp = TempDir::new(&format!("awf-xtask-hash-{tag}-")).expect("暫存目錄");
        std::fs::create_dir_all(tmp.path().join("crates/a/src")).expect("mkdir");
        std::fs::create_dir_all(tmp.path().join("crates/a/target")).expect("mkdir");
        std::fs::write(tmp.path().join("crates/a/src/lib.rs"), body).expect("寫檔");
        std::fs::write(tmp.path().join("crates/a/target/skip.rs"), tag).expect("寫檔");
        std::fs::write(tmp.path().join("crates/a/notes.md"), tag).expect("寫檔");
        std::fs::write(tmp.path().join("Cargo.lock"), "lock\n").expect("寫檔");
        let h = source_hash::source_hash(tmp.path());
        (tmp, h)
    };
    let (_a, lf) = mk("lf", "fn a() {}\n");
    let (_b, crlf) = mk("crlf", "fn a() {}\r\n");
    let (_c, changed) = mk("changed", "fn b() {}\n");
    assert_eq!(lf, crlf, "CRLF 與 LF 要算出同一個值；target/ 與非原始碼的檔不算");
    assert_ne!(lf, changed);
}
