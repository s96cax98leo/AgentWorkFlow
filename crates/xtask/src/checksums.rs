//! `cargo xtask checksums`：`plugin/bin/` 裡發佈用執行檔的 sha256 與原始碼雜湊（`checksums.json`）。
//!
//! 格式：`{"files": {"<檔名>": "<sha256>"}, "source": "<原始碼雜湊>"}`，鍵排序、縮排 2、結尾換行。
//! `--check` 抓得到的是「執行檔過期（原始碼改了沒重建）」與「檔案損壞或被換掉卻沒更新 json」；
//! 它證明不了執行檔確實由這份原始碼建出（見 `docs/建置與發佈.md` 的已知限制）。

use crate::dist;
use crate::source_hash::source_hash;
use crate::util::{self, Captured};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE: &str = "checksums.json";

const USAGE: &str = "用法：cargo xtask checksums [--dir <目錄>] [--write|--check] [--require-all]";

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    Ok(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// 目錄裡的 `awf-*` 檔與各自的 sha256。名字不在五個發佈檔名裡的算錯誤（放錯檔或打錯名字）。
fn scan(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut found = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(found) };
    let known = dist::file_names();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("awf-") || !e.path().is_file() {
            continue;
        }
        if !known.contains(&name.as_str()) {
            return Err(format!("{} 裡有不認得的檔 {name}（發佈檔名只有：{}）", dir.display(), known.join("、")));
        }
        found.insert(name, sha256_file(&e.path())?);
    }
    Ok(found)
}

fn missing(found: &BTreeMap<String, String>) -> Vec<&'static str> {
    dist::file_names().into_iter().filter(|n| !found.contains_key(*n)).collect()
}

/// `checksums.json` 的內容（鍵排序、縮排 2、結尾換行）。
pub fn render(source: &str, files: &BTreeMap<String, String>) -> Result<String, String> {
    let mut inner = Map::new();
    for (name, sum) in files {
        inner.insert(name.clone(), Value::String(sum.clone()));
    }
    let mut doc = Map::new();
    doc.insert("files".to_string(), Value::Object(inner));
    doc.insert("source".to_string(), Value::String(source.to_string()));
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| format!("產生 JSON 失敗：{e}"))?;
    Ok(text + "\n")
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    true
}

fn check(root: &Path, dir: &Path, found: &BTreeMap<String, String>, require_all: bool) -> Result<String, Vec<String>> {
    let json_path = dir.join(FILE);
    let lack = missing(found);
    if found.is_empty() && !json_path.exists() {
        if require_all {
            return Err(vec![format!("{} 裡還沒有發佈用的執行檔，缺：{}", dir.display(), lack.join("、"))]);
        }
        return Ok(format!("還沒有發佈用的執行檔（{}），略過核對\n", dir.display()));
    }
    let mut errs = Vec::new();
    if require_all && !lack.is_empty() {
        errs.push(format!("缺執行檔：{}", lack.join("、")));
    }
    let source = source_hash(root);
    match util::read_json(&json_path) {
        Err(e) => errs.push(format!("{e}（有執行檔就要有 {FILE}，用 --write 產生）")),
        Ok(doc) => {
            match doc.get("source").and_then(Value::as_str) {
                Some(s) if s == source => {}
                Some(s) => errs.push(format!("{FILE} 的 source 是 {s}，現在的原始碼是 {source}：執行檔過期了，要重新建置")),
                None => errs.push(format!("{FILE} 沒有 source")),
            }
            let listed: BTreeMap<String, String> = doc
                .get("files")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default();
            for (name, sum) in found {
                match listed.get(name) {
                    Some(want) if want == sum => {}
                    Some(want) => errs.push(format!("{name} 的 sha256 是 {sum}，{FILE} 記的是 {want}")),
                    None => errs.push(format!("{name} 不在 {FILE} 裡")),
                }
            }
            for name in listed.keys().filter(|n| !found.contains_key(*n)) {
                errs.push(format!("{FILE} 記了 {name}，目錄裡卻沒有這個檔"));
            }
        }
    }
    for name in found.keys().filter(|n| !n.ends_with(".exe")) {
        if !executable(&dir.join(name)) {
            errs.push(format!("{name} 沒有執行權限（下載的 artifact 不保留權限：chmod +x 後用 git update-index --chmod=+x）"));
        }
    }
    // 本機平台的那一個可以實際跑：編進去的原始碼雜湊也要對。
    let mut ran = String::new();
    if let Some(host) = dist::host_file().filter(|h| found.contains_key(*h)) {
        match dist::version_hash(&dir.join(host)) {
            Ok(h) if h == source => ran = format!("；{host} 實際執行的原始碼雜湊也一致"),
            Ok(h) => errs.push(format!("{host} --version 的原始碼雜湊是 {h}，現在的原始碼是 {source}")),
            Err(e) => errs.push(e),
        }
    }
    if errs.is_empty() {
        Ok(format!("checksums 核對通過：{} 個執行檔，source {source}{ran}\n", found.len()))
    } else {
        Err(errs)
    }
}

fn resolve_dir(root: &Path, dir: Option<String>) -> PathBuf {
    match dir {
        None => root.join("plugin").join("bin"),
        Some(d) => {
            let p = PathBuf::from(d);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p)
            }
        }
    }
}

fn run(root: &Path, args: &[String]) -> Result<String, Vec<String>> {
    let one = |e: String| vec![e];
    let (dir, rest) = util::take_opt(args, "--dir").map_err(one)?;
    let mut write = false;
    let mut do_check = false;
    let mut require_all = false;
    for a in &rest {
        match a.as_str() {
            "--write" => write = true,
            "--check" => do_check = true,
            "--require-all" => require_all = true,
            other => return Err(vec![format!("不認得的參數「{other}」\n{USAGE}")]),
        }
    }
    if write && do_check {
        return Err(vec![format!("--write 與 --check 只能擇一\n{USAGE}")]);
    }
    let dir = resolve_dir(root, dir);
    let found = scan(&dir).map_err(one)?;
    if do_check {
        return check(root, &dir, &found, require_all);
    }
    let lack = missing(&found);
    if require_all && !lack.is_empty() {
        return Err(vec![format!("缺執行檔：{}", lack.join("、"))]);
    }
    let text = render(&source_hash(root), &found).map_err(one)?;
    if !write {
        return Ok(text);
    }
    if found.is_empty() {
        return Err(vec![format!("{} 裡沒有任何執行檔，不寫 {FILE}", dir.display())]);
    }
    let path = dir.join(FILE);
    std::fs::write(&path, &text).map_err(|e| vec![format!("寫不了 {}：{e}", path.display())])?;
    Ok(format!("已寫入 {}（{} 個執行檔）\n", path.display(), found.len()))
}

/// `cargo xtask checksums [--dir <目錄>] [--write|--check] [--require-all]`。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    match run(root, args) {
        Ok(out) => (0, out, String::new()),
        Err(errs) => (1, String::new(), errs.iter().map(|e| format!("checksums 失敗：{e}\n")).collect()),
    }
}
