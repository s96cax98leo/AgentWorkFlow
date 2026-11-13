//! 產生並核對兩份「舊版本長什麼樣」的指紋檔（都從 git 歷史算，內容只和歷史有關）：
//! - `plugin/workflow-templates/legacy.json`：舊版 init 複製進專案的 workflow 副本（這個檔的本體）。
//! - `plugin/legacy-tools.json`：0.x 的 init 複製進專案的工具與 hook（產生程式在 `awf_core::init::legacy_tools`，這裡只呼叫）。
//!
//! 0.5 起 workflow 不再複製進專案（由 `awf next` 依 plugin 的範本逐段產生）。`awf init --upgrade` 要能分辨專案裡的
//! `.claude/workflows/*.js` 是「舊版原樣的副本」（可以移除）還是「使用者改過的」（保留）。做法：把各版的範本依佔位符
//! 與開發規範 profile 的內嵌區切成固定片段，只記每段的長度（字元數）與雜湊；升級時拿專案的檔逐段比對，佔位符的位置
//! 要剛好是專案 workflow.config 記的值。全部對得上才算原樣。
//!
//! 版本與 commit 寫在 [`RELEASES`]。0.5 之後不再複製，所以這份清單不會再變長；這支指令平常只用來確認 commit 進去的
//! `legacy.json` 和 git 歷史算出來的逐字相同。
//!
//! 用法：
//!   cargo xtask gen-legacy            # 只比對（預設）；任何一份不一致 exit 1
//!   cargo xtask gen-legacy --write    # 兩份都重寫（legacy-tools.json 用檔案裡記的 revs）
//!
//! 需要完整的 git 歷史（[`RELEASES`] 的那幾個 commit、legacy-tools.json 的 `revs` 要拿得到）；淺層 clone 拿不到時判失敗，不略過。
//! 預設是比對，`--write` 才寫（`--check` 照認，等同預設）。
//!
//! 歷史裡的路徑（[`OLD_DIR`]、`legacy_tools::SOURCES`）是各版當時的位置，不隨之後的搬家改。

use crate::util::{self, Captured};
use awf_core::init::legacy_tools;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// （版本、git rev）。
pub const RELEASES: [(&str, &str); 3] = [("0.3.0", "v0.3.0"), ("0.4.0", "7ab673c"), ("0.4.1", "d81cb5c")];
const OLD_DIR: &str = "scaffold/.claude/workflows";
const OUT_REL: &str = "plugin/workflow-templates/legacy.json";
/// 檔案開頭的說明。字面要和已發佈的 legacy.json 相同（裡面提到的是 0.x 當初產生它的腳本），改了檔案內容就變了。
const NOTE: &str = "舊版 init 複製進專案的 workflow 副本的指紋（init.py --upgrade 用來分辨原樣的副本與使用者改過的檔）。由 tests/gen_legacy_templates.py 產生，不要手改。";

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let r = util::run(Command::new("git").args(args).current_dir(root), None)?;
    if r.code != 0 {
        return Err(format!("git {} 失敗：{}", args.join(" "), r.stderr.trim()));
    }
    Ok(r.stdout)
}

fn seg(text: &str) -> Value {
    let hex: String = Sha1::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    json!([text.chars().count(), hex.get(..16).unwrap_or_default()])
}

/// 把範本切成（佔位符名單、各段的長度與雜湊）。
pub fn split(text: &str) -> Result<(Vec<Value>, Vec<Value>), String> {
    let hole = regex::Regex::new(r"(?s)\{\{([A-Z_]+)\}\}|(/\*@methodology\*/.*?/\*@end\*/)").map_err(|e| e.to_string())?;
    let (mut keys, mut segs, mut pos) = (Vec::new(), Vec::new(), 0);
    for c in hole.captures_iter(text) {
        let Some(all) = c.get(0) else { continue };
        segs.push(seg(text.get(pos..all.start()).unwrap_or_default()));
        keys.push(json!(c.get(1).map(|m| m.as_str()).unwrap_or("@METH")));
        pos = all.end();
    }
    segs.push(seg(text.get(pos..).unwrap_or_default()));
    Ok((keys, segs))
}

/// 從 git 歷史算出 legacy.json 的全文（鍵照字母順序、沒有多餘空白、結尾一個換行）。
pub fn build(root: &Path) -> Result<String, String> {
    let mut files: BTreeMap<String, Vec<(Vec<Value>, Vec<Value>, Vec<Value>)>> = BTreeMap::new();
    for (ver, rev) in RELEASES {
        let listing = git(root, &["ls-tree", "--name-only", rev, &format!("{OLD_DIR}/")])?;
        for name in listing.split_whitespace().filter(|n| n.ends_with(".js")) {
            let text = git(root, &["show", &format!("{rev}:{name}")])?;
            let (keys, segs) = split(&text)?;
            let base = name.rsplit('/').next().unwrap_or(name).to_string();
            let vs = files.entry(base).or_default();
            match vs.iter_mut().find(|(k, s, _)| *k == keys && *s == segs) {
                Some((_, _, versions)) => versions.push(json!(ver)),
                None => vs.push((keys, segs, vec![json!(ver)])),
            }
        }
    }
    if files.is_empty() {
        return Err(format!("git 歷史裡找不到任何舊版 workflow（{OLD_DIR}/*.js）"));
    }
    let mut out = Map::new();
    for (name, vs) in files {
        let entries: Vec<Value> = vs.into_iter().map(|(keys, segs, versions)| json!({"keys": keys, "segs": segs, "versions": versions})).collect();
        out.insert(name, Value::Array(entries));
    }
    let doc = json!({"_說明": NOTE, "files": out});
    Ok(format!("{}\n", serde_json::to_string(&doc).map_err(|e| e.to_string())?))
}

/// `plugin/legacy-tools.json`：用檔案裡記的 `revs` 從 git 歷史重新產生，回（一不一致、說明）。`write` 時寫回。
fn tools(root: &Path, write: bool) -> Result<(bool, String), String> {
    let path = root.join(legacy_tools::REPO_FILE);
    let have = util::read_text(&path)?;
    let revs: Vec<String> = serde_json::from_str::<Value>(&have).ok()
        .and_then(|d| d.get("revs").and_then(Value::as_array).map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_string)).collect()))
        .ok_or_else(|| format!("{} 沒有 revs", legacy_tools::REPO_FILE))?;
    let doc = legacy_tools::generate(root, &revs)?;
    let text = legacy_tools::render(&doc);
    if write {
        std::fs::write(&path, &text).map_err(|e| format!("寫不了 {}：{e}", legacy_tools::REPO_FILE))?;
        return Ok((true, format!("已寫 {}：{}\n", legacy_tools::REPO_FILE, doc.get("count").cloned().unwrap_or(Value::Null))));
    }
    let same = have == text;
    let msg = if same { "一致".to_string() } else { "和 git 歷史算出來的不一致（確認後用 cargo xtask gen-legacy --write 重寫）".to_string() };
    Ok((same, format!("legacy-tools.json：{msg}\n")))
}

/// 指令本體：`root` 是 repo 根目錄，`args` 是 `--write`（`--check` 等同預設的只比對）。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    let (code, out, err) = templates(root, args);
    if code != 0 {
        return (code, out, err);
    }
    match tools(root, args.iter().any(|a| a == "--write")) {
        Ok((same, msg)) => (if same { 0 } else { 1 }, format!("{msg}{out}"), err),
        Err(e) => (1, out, format!("gen-legacy：{e}\n")),
    }
}

/// `plugin/workflow-templates/legacy.json` 的比對或重寫。
fn templates(root: &Path, args: &[String]) -> Captured {
    let write = args.iter().any(|a| a == "--write");
    let text = match build(root) {
        Ok(t) => t,
        Err(e) => return (1, String::new(), format!("gen-legacy：{e}\n")),
    };
    let out_path = root.join("plugin").join("workflow-templates").join("legacy.json");
    if !write {
        let same = std::fs::read(&out_path).map(|b| b == text.as_bytes()).unwrap_or(false);
        let msg = if same { "一致".to_string() } else { "和 git 歷史算出來的不一致（確認後用 cargo xtask gen-legacy --write 重寫）".to_string() };
        return (if same { 0 } else { 1 }, format!("legacy.json：{msg}\n"), String::new());
    }
    if let Err(e) = std::fs::write(&out_path, &text) {
        return (1, String::new(), format!("gen-legacy：寫不了 {OUT_REL}：{e}\n"));
    }
    let doc: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let files = doc.get("files").and_then(Value::as_object);
    let n = files.map(Map::len).unwrap_or(0);
    let versions: usize = files.map(|f| f.values().map(|v| v.as_array().map(Vec::len).unwrap_or(0)).sum()).unwrap_or(0);
    (0, format!("已寫 {OUT_REL}：{n} 支 workflow、{versions} 個版本的指紋，{} KB\n", text.chars().count() / 1024), String::new())
}
