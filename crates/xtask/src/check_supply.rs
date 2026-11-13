//! 供應鏈檢查。
//!
//! plugin 會在使用者的所有專案執行，所以要守住下面幾條：
//!   1. action 都釘 40 字元 commit SHA：.github/workflows/*.yml 與 plugin/scaffold/.github/workflows/*.tmpl。
//!   2. 版控（git ls-files）裡沒有 `__pycache__`、`.pyc`、.env、私鑰形狀的檔案或內容。
//!   3. `Cargo.lock` 存在而且在版控裡（建置出來的執行檔用哪些版本是固定的）。
//!   4. Rust 相依都來自 crates.io：`Cargo.lock` 裡沒有 git 來源，也沒有 workspace 以外的 path 相依。
//!   5. Rust 相依的名單和允許清單（`crates/xtask/allowed-crates.txt`，一行一個 crate 名）一致：多了、少了都算失敗，
//!      所以新增（或間接帶進）一個 crate 一定要有人明確把它加進清單。
//!   6. Node 腳本（乾跑用的 `check_workflows.mjs`、`scenarios/*.mjs`，xtask 的 `js/*.cjs`）只 import 內建模組或相對路徑，
//!      而且不用網路模組。
//!
//! 失敗時逐條列出，exit 1。檔案不是 UTF-8、跑不了 git 都列成一條失敗（只會更嚴）。

use crate::termre::{self, Pat};
use crate::util::{self, Captured};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Patterns {
    uses: Pat,
    pinned: Pat,
    key_name: Pat,
    env_name: Pat,
    env_allowed: Pat,
    key_content: Pat,
}

fn patterns() -> Result<Patterns, String> {
    let c = |p: &str| termre::compile(p).map_err(|e| format!("內建的樣式編不過（{e}）"));
    Ok(Patterns {
        uses: c(r#"(?m)^\s*(?:-\s*)?uses:\s*['"]?([^\s#'"]+)"#)?,
        pinned: c(r"@[0-9a-f]{40}$")?,
        key_name: c(r"(?i)(^|/)(id_(rsa|dsa|ecdsa|ed25519)|.*\.(pem|key|p12|pfx|jks|keystore|ppk))$")?,
        env_name: c(r"(^|/)\.env(\.[^/]*)?$")?,
        env_allowed: c(r"\.(example|sample|template|tmpl)$")?,
        // 私鑰內容的樣式用拼接寫，避免這支檔案自己被掃到
        key_content: c(&format!("-----BEGIN [A-Z ]*{}", "PRIVATE KEY-----"))?,
    })
}

/// `<root>/<dir>` 這一層、檔名以 `suffix` 結尾的檔案（相對路徑，依檔名排序）。
fn files_in(root: &Path, dir: &str, suffix: &str) -> Vec<String> {
    let base = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
    let mut names: Vec<String> = std::fs::read_dir(&base).into_iter().flatten().flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(suffix) && n.len() > suffix.len())
        .collect();
    names.sort();
    names.into_iter().map(|n| if dir.is_empty() { n } else { format!("{dir}/{n}") }).collect()
}

fn full(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn check_pinned_actions(root: &Path, pats: &Patterns, errors: &mut Vec<String>) {
    let mut files = files_in(root, ".github/workflows", ".yml");
    files.extend(files_in(root, ".github/workflows", ".yaml"));
    files.extend(files_in(root, "plugin/scaffold/.github/workflows", ".tmpl"));
    for rel in files {
        let text = match util::read_text(&full(root, &rel)) {
            Ok(t) => t,
            Err(_) => {
                errors.push(format!("{rel}：讀不到或不是 UTF-8"));
                continue;
            }
        };
        let found = match pats.uses.find_iter(&text) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("{rel}：{e}"));
                continue;
            }
        };
        for (start, all, reference) in found {
            if reference.starts_with("./") || reference.starts_with("docker://") {
                continue;
            }
            // 樣式出錯時當成沒釘（只會更嚴）
            if !pats.pinned.search(reference).unwrap_or(false) {
                // 第一個群組在整段配到的最後面
                let at = start + all.len().saturating_sub(reference.len());
                let line = text.get(..at).unwrap_or_default().matches('\n').count() + 1;
                errors.push(format!("{rel}:{line}：action 沒有釘 40 字元 commit SHA（{reference}）"));
            }
        }
    }
}

fn check_tracked_files(root: &Path, pats: &Patterns, errors: &mut Vec<String>) {
    let r = match util::run(Command::new("git").args(["ls-files", "-z"]).current_dir(root), None) {
        Ok(r) => r,
        Err(e) => {
            errors.push(format!("git ls-files 失敗：{e}"));
            return;
        }
    };
    if r.code != 0 {
        // gate 只會更嚴：拿不到版控清單就算失敗，不推定沒問題
        errors.push(format!("git ls-files 失敗：{}", r.stderr.trim()));
        return;
    }
    // 樣式出錯時當成命中（只會更嚴）
    let hit = |p: &Pat, text: &str| p.search_name(text).unwrap_or(true);
    for name in r.stdout.split('\0') {
        if name.is_empty() {
            continue;
        }
        if name.split('/').any(|p| p == "__pycache__") || name.ends_with(".pyc") || name.ends_with(".pyo") {
            errors.push(format!("{name}：版控裡不該有 Python 快取檔"));
        } else if hit(&pats.env_name, name) && !pats.env_allowed.search_name(name).unwrap_or(false) {
            errors.push(format!("{name}：版控裡不該有 .env 檔"));
        } else if hit(&pats.key_name, name) {
            errors.push(format!("{name}：版控裡不該有私鑰形狀的檔案"));
        } else {
            let p = full(root, name);
            let small = std::fs::metadata(&p).map(|m| m.is_file() && m.len() <= 2_000_000).unwrap_or(false);
            if !small {
                continue;
            }
            if let Ok(data) = std::fs::read(&p) {
                if pats.key_content.search(&String::from_utf8_lossy(&data)).unwrap_or(true) {
                    errors.push(format!("{name}：內容含私鑰"));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Rust 相依
/// crates.io 的來源字串（`Cargo.lock` 的 `source`）。
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// 允許清單的位置（相對 repo 根目錄）。
pub const ALLOWED_CRATES: &str = "crates/xtask/allowed-crates.txt";

/// `Cargo.lock` 的一個 `[[package]]`。
struct LockPkg {
    name: String,
    source: Option<String>,
}

/// `key = "value"` 這種行的值（Cargo.lock 與 Cargo.toml 裡用得到的寫法）。
fn toml_str(line: &str, key: &str) -> Option<String> {
    let rest = line.trim().strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
    let rest = rest.strip_prefix('"')?;
    rest.find('"').map(|end| rest.get(..end).unwrap_or_default().to_string())
}

fn lock_packages(text: &str) -> Result<Vec<LockPkg>, String> {
    let mut out: Vec<LockPkg> = Vec::new();
    let mut in_pkg = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[[package]]";
            if in_pkg {
                out.push(LockPkg { name: String::new(), source: None });
            }
            continue;
        }
        let Some(cur) = out.last_mut().filter(|_| in_pkg) else { continue };
        if let Some(v) = toml_str(line, "name") {
            cur.name = v;
        } else if let Some(v) = toml_str(line, "source") {
            cur.source = Some(v);
        }
    }
    if out.is_empty() || out.iter().any(|p| p.name.is_empty()) {
        return Err("Cargo.lock 讀不懂（沒有 [[package]]，或有 package 沒有 name）".into());
    }
    Ok(out)
}

/// workspace 成員的 crate 名（根 `Cargo.toml` 的 `members`，各成員 `Cargo.toml` 的 `[package]` `name`）。
fn workspace_members(root: &Path) -> Result<BTreeSet<String>, String> {
    let text = util::read_text(&root.join("Cargo.toml")).map_err(|_| "讀不到 Cargo.toml".to_string())?;
    let re = regex::Regex::new(r"(?s)\bmembers\s*=\s*\[(.*?)\]").map_err(|e| e.to_string())?;
    let list = re.captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str()).ok_or("Cargo.toml 沒有 workspace 的 members")?;
    let mut out = BTreeSet::new();
    for dir in list.split(',').map(|x| x.trim().trim_matches('"')).filter(|x| !x.is_empty()) {
        if dir.contains('*') {
            return Err(format!("workspace 的 members 用了萬用字元（{dir}），無法核對成員名單"));
        }
        let manifest = util::read_text(&full(root, dir).join("Cargo.toml")).map_err(|_| format!("讀不到 {dir}/Cargo.toml"))?;
        let mut in_package = false;
        let mut name = None;
        for line in manifest.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_package = t == "[package]";
            } else if in_package && name.is_none() {
                name = toml_str(line, "name");
            }
        }
        out.insert(name.ok_or_else(|| format!("{dir}/Cargo.toml 沒有 [package] 的 name"))?);
    }
    Ok(out)
}

fn check_lock_tracked(root: &Path, errors: &mut Vec<String>) {
    if !root.join("Cargo.lock").is_file() {
        errors.push("Cargo.lock 不存在".into());
        return;
    }
    match util::run(Command::new("git").args(["ls-files", "--error-unmatch", "Cargo.lock"]).current_dir(root), None) {
        Ok(r) if r.code == 0 => {}
        Ok(_) => errors.push("Cargo.lock 不在版控裡（git ls-files 沒有它）".into()),
        Err(e) => errors.push(format!("git ls-files 失敗：{e}")),
    }
}

/// 讀 `Cargo.lock`；讀不到或讀不懂記一條錯誤、回 None（後面兩條都靠它，各自報一次）。
fn read_lock(root: &Path) -> Result<Vec<LockPkg>, String> {
    lock_packages(&util::read_text(&root.join("Cargo.lock")).map_err(|_| "讀不到 Cargo.lock".to_string())?)
}

fn check_registry_only(root: &Path, errors: &mut Vec<String>) {
    let (pkgs, members) = match (read_lock(root), workspace_members(root)) {
        (Ok(p), Ok(m)) => (p, m),
        (p, m) => {
            errors.extend(p.err());
            errors.extend(m.err());
            return;
        }
    };
    for p in &pkgs {
        match &p.source {
            None if members.contains(&p.name) => {}
            None => errors.push(format!("{}：沒有來源（workspace 以外的 path 相依）", p.name)),
            Some(src) if src == CRATES_IO => {}
            Some(src) => errors.push(format!("{}：來源不是 crates.io（{src}）", p.name)),
        }
    }
    for m in &members {
        if !pkgs.iter().any(|p| p.source.is_none() && p.name == *m) {
            errors.push(format!("{m}：workspace 成員不在 Cargo.lock 裡（Cargo.lock 沒有跟上）"));
        }
    }
}

fn check_allowed_crates(root: &Path, errors: &mut Vec<String>) {
    let pkgs = match read_lock(root) {
        Ok(p) => p,
        Err(e) => {
            errors.push(e);
            return;
        }
    };
    let text = match util::read_text(&full(root, ALLOWED_CRATES)) {
        Ok(t) => t,
        Err(_) => {
            errors.push(format!("讀不到允許清單 {ALLOWED_CRATES}"));
            return;
        }
    };
    let allowed: BTreeSet<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let have: BTreeSet<&str> = pkgs.iter().filter(|p| p.source.is_some()).map(|p| p.name.as_str()).collect();
    for name in have.difference(&allowed) {
        errors.push(format!("{name}：Cargo.lock 有、允許清單沒有（確認要用它之後加進 {ALLOWED_CRATES}）"));
    }
    for name in allowed.difference(&have) {
        errors.push(format!("{name}：允許清單有、Cargo.lock 沒有（已經不用就從 {ALLOWED_CRATES} 拿掉）"));
    }
}

// ------------------------------------------------------------------ Node 腳本
/// 可以用的內建模組（不含 `node:` 前綴）。網路相關的（http、https、net、tls、dgram、dns、http2）刻意不列。
const NODE_BUILTINS: &[&str] = &["assert", "child_process", "crypto", "events", "fs", "module", "os", "path", "process", "url", "util", "vm"];

/// repo 裡自己的 Node 腳本（相對路徑）：乾跑工具與情境、xtask 的測試本體。
fn node_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for (dir, exts) in [
        ("plugin/workflow-templates", &[".mjs", ".cjs"][..]),
        ("plugin/workflow-templates/scenarios", &[".mjs", ".cjs"][..]),
        ("crates/xtask/src/js", &[".mjs", ".cjs", ".js"][..]),
    ] {
        for ext in exts {
            files.extend(files_in(root, dir, ext));
        }
    }
    files
}

fn check_node_builtin_only(root: &Path, errors: &mut Vec<String>) {
    let pats = [
        r#"(?m)^[ \t]*(?:import|export)\b[^\n'"]*?\bfrom[ \t]*['"]([^'"\n]+)['"]"#,
        r#"(?m)^[ \t]*\}[ \t]*from[ \t]*['"]([^'"\n]+)['"]"#,
        r#"(?m)^[ \t]*import[ \t]*['"]([^'"\n]+)['"]"#,
        r#"\b(?:require|import)[ \t]*\([ \t]*['"]([^'"\n]+)['"]"#,
    ];
    let mut res = Vec::new();
    for p in pats {
        match regex::Regex::new(p) {
            Ok(r) => res.push(r),
            Err(e) => {
                errors.push(format!("內建的樣式編不過（{e}）"));
                return;
            }
        }
    }
    for rel in node_files(root) {
        let text = match util::read_text(&full(root, &rel)) {
            Ok(t) => t,
            Err(_) => {
                errors.push(format!("{rel}：讀不到或不是 UTF-8"));
                continue;
            }
        };
        let mut found: Vec<(usize, &str)> = Vec::new();
        for re in &res {
            for c in re.captures_iter(&text) {
                if let Some(m) = c.get(1) {
                    let line = text.get(..m.start()).unwrap_or_default().matches('\n').count() + 1;
                    if !found.contains(&(line, m.as_str())) {
                        found.push((line, m.as_str()));
                    }
                }
            }
        }
        found.sort();
        for (line, spec) in found {
            let bare = spec.strip_prefix("node:").unwrap_or(spec);
            let local = spec.starts_with("./") || spec.starts_with("../");
            if !local && !NODE_BUILTINS.contains(&bare) {
                errors.push(format!("{rel}:{line}：只能 import 內建模組（不含網路）或相對路徑（{spec}）"));
            }
        }
    }
}

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path) -> Captured {
    let pats = match patterns() {
        Ok(p) => p,
        Err(e) => return (1, String::new(), format!("{e}\n")),
    };
    let mut out = String::new();
    let mut failed = 0;
    let mut report = |name: &str, errors: Vec<String>| {
        if errors.is_empty() {
            out.push_str(&format!("[通過] {name}\n"));
        } else {
            failed += errors.len();
            out.push_str(&format!("[失敗] {name}\n"));
            for e in &errors {
                out.push_str(&format!("  - {e}\n"));
            }
        }
    };
    let mut errors = Vec::new();
    check_pinned_actions(root, &pats, &mut errors);
    report("action 釘 SHA", std::mem::take(&mut errors));
    check_tracked_files(root, &pats, &mut errors);
    report("版控沒有快取、.env、私鑰", std::mem::take(&mut errors));
    check_lock_tracked(root, &mut errors);
    report("Cargo.lock 在版控裡", std::mem::take(&mut errors));
    check_registry_only(root, &mut errors);
    report("Rust 相依都來自 crates.io", std::mem::take(&mut errors));
    check_allowed_crates(root, &mut errors);
    report("Rust 相依和允許清單一致", std::mem::take(&mut errors));
    check_node_builtin_only(root, &mut errors);
    report("Node 腳本只用內建模組", std::mem::take(&mut errors));
    if failed > 0 {
        out.push_str(&format!("\n供應鏈檢查失敗：{failed} 條\n"));
        return (1, out, String::new());
    }
    out.push_str("\n供應鏈檢查通過\n");
    (0, out, String::new())
}
