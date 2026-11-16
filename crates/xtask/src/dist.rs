//! `cargo xtask dist`：建置某個平台的發佈用執行檔，複製成 `plugin/bin/` 用的檔名。
//!
//! 預設輸出到 `target/dist/`（不進版控）。發佈時才由人把五個檔放進 `plugin/bin/`，流程見 `docs/建置與發佈.md`。

use crate::util::{self, Captured};
use std::path::{Path, PathBuf};
use std::process::Command;

/// target triple → `plugin/bin/` 裡的檔名。啟動器（`plugin/bin/awf`、`awf.cmd`）挑的檔名要和這張表一致，
/// `cargo xtask check-launchers` 會核對。
pub const TARGETS: &[(&str, &str)] = &[
    ("aarch64-apple-darwin", "awf-darwin-arm64"),
    ("x86_64-apple-darwin", "awf-darwin-x64"),
    ("x86_64-unknown-linux-gnu", "awf-linux-x64"),
    ("aarch64-unknown-linux-gnu", "awf-linux-arm64"),
    ("x86_64-pc-windows-msvc", "awf-windows-x64.exe"),
];

/// triple 對應的檔名。
pub fn file_for(triple: &str) -> Option<&'static str> {
    TARGETS.iter().find(|(t, _)| *t == triple).map(|(_, f)| *f)
}

/// 五個發佈檔名（照表的順序）。
pub fn file_names() -> Vec<&'static str> {
    TARGETS.iter().map(|(_, f)| *f).collect()
}

/// 這台機器（xtask 自己被編譯成的平台）對應的 triple；不在表裡回 None。
pub fn host_triple() -> Option<&'static str> {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    };
    Some(triple)
}

/// 這台機器對應的發佈檔名。
pub fn host_file() -> Option<&'static str> {
    host_triple().and_then(file_for)
}

/// 跑執行檔的 `--version`，回 `(source <雜湊>)` 裡的雜湊。
pub fn version_hash(bin: &Path) -> Result<String, String> {
    let ran = util::run(Command::new(bin).arg("--version"), None)?;
    if ran.code != 0 {
        return Err(format!("{} --version 的 exit code 是 {}", bin.display(), ran.code));
    }
    let line = ran.stdout.trim();
    line.split_once("(source ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .map(str::to_string)
        .ok_or_else(|| format!("{} --version 的輸出看不懂：{line}", bin.display()))
}

fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(d) if !d.is_empty() => {
            let p = PathBuf::from(d);
            if p.is_absolute() {
                p
            } else {
                root.join(p)
            }
        }
        _ => root.join("target"),
    }
}

fn dist(root: &Path, args: &[String]) -> Result<String, String> {
    let (target, rest) = util::take_opt(args, "--target")?;
    let (out, rest) = util::take_opt(&rest, "--out")?;
    if let Some(extra) = rest.first() {
        return Err(format!("不認得的參數「{extra}」（用法：cargo xtask dist [--target <triple>] [--out <目錄>]）"));
    }
    let known = || TARGETS.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("、");
    let triple = match target {
        Some(t) => t,
        None => host_triple().ok_or_else(|| format!("這台機器不在支援的平台裡，請用 --target 指定（{}）", known()))?.to_string(),
    };
    let name = file_for(&triple).ok_or_else(|| format!("不支援的 target「{triple}」（支援：{}）", known()))?;
    let out_dir = match out {
        Some(o) => {
            let p = PathBuf::from(o);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir().map_err(|e| format!("讀不到目前目錄：{e}"))?.join(p)
            }
        }
        None => root.join("target").join("dist"),
    };

    let cargo = std::env::var_os("CARGO").filter(|c| !c.is_empty()).unwrap_or_else(|| "cargo".into());
    let status = Command::new(&cargo)
        .args(["build", "--release", "--locked", "-p", "awf", "--target", &triple])
        .current_dir(root)
        .status()
        .map_err(|e| format!("跑不起來 cargo：{e}"))?;
    if !status.success() {
        return Err(format!("cargo build 失敗（target {triple}）；沒裝這個 target 的話先跑 rustup target add {triple}"));
    }

    let exe = if name.ends_with(".exe") { "awf.exe" } else { "awf" };
    let built = target_dir(root).join(&triple).join("release").join(exe);
    if !built.is_file() {
        return Err(format!("建置完卻找不到產物 {}", built.display()));
    }
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("建不了目錄 {}：{e}", out_dir.display()))?;
    let dest = out_dir.join(name);
    let size = std::fs::copy(&built, &dest).map_err(|e| format!("複製到 {} 失敗：{e}", dest.display()))?;

    let mut msg = format!("已建置 {}（{size} 位元組，target {triple}）\n", dest.display());
    if host_triple() == Some(triple.as_str()) {
        let hash = version_hash(&dest)?;
        let want = crate::source_hash::source_hash(root);
        if hash != want {
            return Err(format!("{} 的原始碼雜湊是 {hash}，現在的原始碼是 {want}（建置期間原始碼變了？）", dest.display()));
        }
        msg.push_str(&format!("--version 的原始碼雜湊 {hash} 與現在的原始碼一致\n"));
    } else {
        msg.push_str("不是本機的平台，沒有試跑\n");
    }
    Ok(msg)
}

/// `cargo xtask dist [--target <triple>] [--out <目錄>]`。cargo 的輸出直接印到終端機。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    match dist(root, args) {
        Ok(msg) => (0, msg, String::new()),
        Err(e) => (1, String::new(), format!("dist 失敗：{e}\n")),
    }
}
