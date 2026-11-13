//! 去客戶化掃描：用 `tests/scrub_terms.txt`（公開的通用規則）與 `tests/scrub_terms.local.txt`（本機的真實名稱，不進版控；
//! 存在才讀）掃描檔案內容與路徑名，兩份都是一行一個 ERE 正規式。由 `tests/scrub_check.py` 移植。
//!
//! 用法：
//!   cargo xtask scrub                        # 掃整個 repo
//!   cargo xtask scrub README.md docs tests   # 只掃指定的檔案或目錄（相對 repo 根目錄）
//! 任何命中就以 1 退出，並列出「檔案:行號:內容」。排除 .git、__pycache__、node_modules、根目錄的本機目錄
//! （_tpm、graft、.claude、.idea、Rust 的建置產物 target）、兩份掃描清單本身與二進位檔。
//!
//! 和 Python 版不同的地方（都只會更嚴，不會更鬆）：
//!   - 清單裡 Rust 編不過的規則：明確報錯、exit 1，不略過。錯誤訊息只寫檔名與行號，不印規則內容。
//!   - 正規式的差異由 [`crate::pyre`] 逐條改寫；路徑名結尾有換行時多比一次。
//!   - 指定的路徑是 repo 以外的絕對路徑：Python 版當掉（traceback），這裡印一行說明，exit code 同樣是 1。

use crate::pyre::{self, Pat};
use awf_core::merge_gate::pyutil;
use std::path::{Path, PathBuf};

const TERMS_NAME: &str = "scrub_terms.txt";
const LOCAL_TERMS_NAME: &str = "scrub_terms.local.txt";
const SKIP_DIRS: &[&str] = &[".git", "__pycache__", "node_modules"];
/// repo 根目錄下只存在本機、不進版控的目錄（run 紀錄、graft 快取、本機 settings 與 agent worktree）
const LOCAL_ROOT_DIRS: &[&str] = &["_tpm", "graft", ".claude", ".idea", "target"];

/// 讀兩份清單；回 `Err((exit code, stderr 的文字))`。
fn load_patterns(root: &Path) -> Result<Vec<Pat>, (i32, String)> {
    let terms = root.join("tests").join(TERMS_NAME);
    if !terms.is_file() {
        return Err((2, format!("找不到掃描清單：{}\n", terms.display())));
    }
    let mut pats = Vec::new();
    for name in [TERMS_NAME, LOCAL_TERMS_NAME] {
        let f = root.join("tests").join(name);
        if !f.is_file() {
            continue;
        }
        let text = std::fs::read(&f).ok().and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| (1, format!("掃描清單 tests/{name} 讀不到或不是 UTF-8\n")))?;
        let text = pyutil::universal_newlines(&text);
        for (i, line) in pyutil::splitlines(&text).into_iter().enumerate() {
            if pyutil::strip(line).is_empty() || line.trim_start_matches(pyutil::is_py_space).starts_with('#') {
                continue;
            }
            match pyre::compile(line) {
                Ok(p) => pats.push(p),
                Err(why) => {
                    return Err((1, format!("掃描清單 tests/{name} 第 {} 行的規則編不過（{why}）；規則不能略過，請改寫這一行\n", i + 1)));
                }
            }
        }
    }
    Ok(pats)
}

/// 一個要掃的項目：顯示用的相對路徑、實際路徑、是不是檔案。目錄本身也算一項，用來掃路徑名。
struct Entry {
    shown: String,
    path: PathBuf,
    is_file: bool,
}

fn shown_of(rel: &[String]) -> String {
    if rel.is_empty() { ".".to_string() } else { format!("./{}", rel.join("/")) }
}

/// 照 `os.walk` 的順序（每層先列子目錄、再列檔案，都依名稱排序，然後才往下走）。
fn walk_dir(dir: &Path, rel: &[String], out: &mut Vec<Entry>) {
    let mut dirs: Vec<(String, bool)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // 和 os.walk 一樣：指向目錄的符號連結列在目錄那一組，但不往下走
        let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            dirs.push((name, is_link));
        } else {
            files.push(name);
        }
    }
    dirs.retain(|(d, _)| !SKIP_DIRS.contains(&d.as_str()) && !(rel.is_empty() && LOCAL_ROOT_DIRS.contains(&d.as_str())));
    dirs.sort();
    files.sort();
    let child = |name: &str| {
        let mut r = rel.to_vec();
        r.push(name.to_string());
        r
    };
    for (d, _) in &dirs {
        out.push(Entry { shown: shown_of(&child(d)), path: dir.join(d), is_file: false });
    }
    for f in &files {
        if SKIP_DIRS.contains(&f.as_str()) {
            continue; // worktree 裡的 .git 是檔案
        }
        out.push(Entry { shown: shown_of(&child(f)), path: dir.join(f), is_file: true });
    }
    for (d, is_link) in &dirs {
        if !is_link {
            walk_dir(&dir.join(d), &child(d), out);
        }
    }
}

/// 指令列的一個路徑 →（實際路徑、相對 repo 根目錄的各段）。和 pathlib 一樣丟掉 `.` 與空段、保留 `..`；
/// repo 以外的絕對路徑回 None。
fn resolve_target(root: &Path, arg: &str) -> Option<(PathBuf, Vec<String>)> {
    let parts = |s: &str| -> Vec<String> { s.split('/').filter(|p| !p.is_empty() && *p != ".").map(str::to_string).collect() };
    if arg.starts_with('/') {
        let root_parts = parts(&root.to_string_lossy());
        let mine = parts(arg);
        let rest = mine.strip_prefix(root_parts.as_slice())?;
        return Some((PathBuf::from(arg), rest.to_vec()));
    }
    let rel = parts(arg);
    let mut full = root.to_path_buf();
    for p in &rel {
        full.push(p);
    }
    Some((full, rel))
}

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|b| *b == 0)
}

/// 指令本體：`root` 是 repo 根目錄，`argv` 是要掃的路徑（空的就掃整個 repo）。回（exit code、stdout、stderr）。
pub fn run_captured(root: &Path, argv: &[String]) -> (i32, String, String) {
    let pats = match load_patterns(root) {
        Ok(p) => p,
        Err((code, msg)) => return (code, String::new(), msg),
    };
    let mut targets: Vec<(PathBuf, Vec<String>)> = Vec::new();
    if argv.is_empty() {
        targets.push((root.to_path_buf(), Vec::new()));
    }
    for a in argv {
        match resolve_target(root, a) {
            Some(t) => targets.push(t),
            // 不存在的路徑和 Python 版一樣略過
            None if !Path::new(a).exists() => {}
            None => return (1, String::new(), format!("路徑不在 repo 裡：{a}\n")),
        }
    }
    let mut content_hits: Vec<String> = Vec::new();
    let mut path_hits: Vec<String> = Vec::new();
    let failed = |msg: String| (1, String::new(), format!("{msg}\n"));
    for (target, rel) in &targets {
        if !target.exists() {
            continue;
        }
        let mut entries = Vec::new();
        if target.is_file() {
            entries.push(Entry { shown: shown_of(rel), path: target.clone(), is_file: true });
        } else {
            entries.push(Entry { shown: shown_of(rel), path: target.clone(), is_file: false });
            walk_dir(target, rel, &mut entries);
        }
        for e in entries {
            for pat in &pats {
                match pat.search_name(&e.shown) {
                    Ok(true) => {
                        path_hits.push(e.shown.clone());
                        break;
                    }
                    Ok(false) => {}
                    Err(msg) => return failed(msg),
                }
            }
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !e.is_file || name == TERMS_NAME || name == LOCAL_TERMS_NAME {
                continue;
            }
            let Ok(data) = std::fs::read(&e.path) else { continue };
            if is_binary(&data) {
                continue;
            }
            let text = String::from_utf8_lossy(&data);
            for (i, line) in pyutil::splitlines(&text).into_iter().enumerate() {
                for pat in &pats {
                    match pat.search(line) {
                        Ok(true) => {
                            content_hits.push(format!("{}:{}:{line}", e.shown, i + 1));
                            break;
                        }
                        Ok(false) => {}
                        Err(msg) => return failed(msg),
                    }
                }
            }
        }
    }
    let mut out = String::new();
    if !content_hits.is_empty() {
        out.push_str(&format!("== 內容命中 ==\n{}\n", content_hits.join("\n")));
    }
    if !path_hits.is_empty() {
        out.push_str(&format!("== 路徑名命中 ==\n{}\n", path_hits.join("\n")));
    }
    let hits = content_hits.len() + path_hits.len();
    if hits > 0 {
        return (1, out, format!("去客戶化掃描：不合格，共 {hits} 處命中\n"));
    }
    let local = if root.join("tests").join(LOCAL_TERMS_NAME).is_file() { "含本機清單" } else { "沒有本機清單，只用公開規則" };
    let scope = if argv.is_empty() { ".".to_string() } else { argv.join(" ") };
    out.push_str(&format!("去客戶化掃描：通過（掃描範圍：{scope}；{local}）\n"));
    (0, out, String::new())
}
