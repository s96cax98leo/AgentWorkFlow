//! 去客戶化掃描：用 `tests/scrub_terms.txt`（公開的通用規則）與 `tests/scrub_terms.local.txt`（本機的真實名稱，不進版控；
//! 存在才讀）掃描檔案內容與路徑名，兩份都是一行一個 ERE 正規式。原本是 0.x 的 Python 工具 `scrub_check.py`。
//!
//! 用法：
//!   cargo xtask scrub                        # 掃整個 repo
//!   cargo xtask scrub README.md docs tests   # 只掃指定的檔案或目錄（相對 repo 根目錄）
//! 任何命中就以 1 退出，並列出「檔案:行號:內容」。排除 .git、__pycache__、node_modules、根目錄的本機目錄
//! （_tpm、graft、.claude、.idea、Rust 的建置產物 target）、兩份掃描清單本身與二進位檔。
//!
//! 讀規則、走目錄、逐行比對的邏輯在 [`awf_core::scrub`]（專案用的 `awf scrub` 是同一套）；這裡只決定規則檔、略過哪些目錄與輸出格式。
//!
//! 和 0.x 的 Python 版不同的地方（都只會更嚴，不會更鬆）：
//!   - 清單裡 Rust 編不過的規則：明確報錯、exit 1，不略過。錯誤訊息只寫檔名與行號，不印規則內容。
//!   - 正規式的差異由 [`awf_core::termre`] 逐條改寫；路徑名結尾有換行時多比一次。
//!   - 指定的路徑是 repo 以外的絕對路徑：0.x 的 Python 版當掉（traceback），這裡印一行說明，exit code 同樣是 1。

use awf_core::scrub::{is_binary, match_lines, match_name, parse_terms, shown_of, walk_dir, Entry, Rule};
use std::path::{Path, PathBuf};

const TERMS_NAME: &str = "scrub_terms.txt";
const LOCAL_TERMS_NAME: &str = "scrub_terms.local.txt";
const SKIP_DIRS: &[&str] = &[".git", "__pycache__", "node_modules"];
/// repo 根目錄下只存在本機、不進版控的目錄（run 紀錄、graft 快取、本機 settings 與 agent worktree）
const LOCAL_ROOT_DIRS: &[&str] = &["_tpm", "graft", ".claude", ".idea", "target"];

/// 讀兩份清單；回 `Err((exit code, stderr 的文字))`。
fn load_patterns(root: &Path) -> Result<Vec<Rule>, (i32, String)> {
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
        let rules = parse_terms(&text, &format!("tests/{name}"))
            .map_err(|(line, why)| (1, format!("掃描清單 tests/{name} 第 {line} 行的規則編不過（{why}）；規則不能略過，請改寫這一行\n")))?;
        pats.extend(rules);
    }
    Ok(pats)
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
            // 不存在的路徑和 0.x 的 Python 版一樣略過
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
            walk_dir(target, rel, SKIP_DIRS, LOCAL_ROOT_DIRS, &mut entries);
        }
        for e in entries {
            match match_name(&pats, &e.shown) {
                Ok(Some(_)) => path_hits.push(e.shown.clone()),
                Ok(None) => {}
                Err(msg) => return failed(msg),
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
            match match_lines(&pats, &text) {
                Ok(hits) => content_hits.extend(hits.into_iter().map(|(n, _, line)| format!("{}:{n}:{line}", e.shown))),
                Err(msg) => return failed(msg),
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
