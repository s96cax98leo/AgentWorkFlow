//! 升級時整理 0.x 複製進專案的工具副本（`.claude/teams/tools/`、`.claude/hooks/` 的 Python 腳本）。
//!
//! 1.0 起工具都在 `awf` 執行檔裡，專案不再放這些檔。`plugin/legacy-tools.json` 記了已發佈各版裡每個檔的指紋
//! （落在專案裡的內容：換行統一成 `\n`；有佔位符的檔切成段，和 `plugin/workflow-templates/legacy.json` 同一個做法）。
//! 升級時：指紋相符的（原樣）移除；路徑是我們放過的、但內容對不上的（改過）保留並列出；
//! 路徑不是我們放過的（使用者自己加的）不動。移除檔案是破壞性動作，所以只認指紋，認不出來就留著。
//!
//! 指紋檔由 [`generate`] 從 git 歷史產生（之後接到 `cargo xtask gen-legacy`）；`tests/init_v1.rs` 會核對檔案內容和重新產生的一致。

use super::legacy::take_chars;
use super::writer::{ours, posix, skip_file, walk_files};
use super::Kv;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// 指紋檔在模版裡的位置（相對模版根目錄）。
pub const FILE: &str = "plugin/legacy-tools.json";
/// 版本號所在的檔（相對模版根目錄）。
const PLUGIN_JSON: &str = "plugin/.claude-plugin/plugin.json";

/// 0.x 的 init 複製了什麼：（模版裡的路徑, 專案裡的路徑, 複製時換不換佔位符）。前兩個是目錄，第三個是單一檔。
pub const SOURCES: &[(&str, &str, bool)] = &[
    ("scaffold/.claude/teams/tools", ".claude/teams/tools", true),
    ("plugin/hooks", ".claude/hooks", false),
    ("scaffold/.claude/teams/golden-set/run.py", ".claude/teams/golden-set/run.py", true),
];
/// `plugin/hooks/` 裡 0.x 的 init 沒有複製的檔。
const NOT_COPIED: &[&str] = &["plugin/hooks/hooks.json"];

fn placeholder() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\{\{([A-Z_]+)\}\}").expect("固定的正規式"))
}

fn sha1_16(data: &[u8]) -> String {
    let hex: String = Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(16).collect()
}

/// 0.x 的 init 用文字模式讀模版檔（`\r\n`、單獨的 `\r` 都變成 `\n`）再原樣寫出；專案的檔之後可能被 git 換成 CRLF。兩邊都統一成 `\n` 再比。
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 模版裡的路徑對到專案裡的哪個路徑；不是 0.x 的 init 會複製的檔回 None。
pub fn project_path(repo_path: &str) -> Option<(String, bool)> {
    if NOT_COPIED.contains(&repo_path) || repo_path.split('/').any(|c| c == "__pycache__") || repo_path.ends_with(".DS_Store") {
        return None;
    }
    SOURCES.iter().find_map(|(src, dst, render)| {
        if repo_path == *src {
            Some((dst.to_string(), *render))
        } else {
            repo_path.strip_prefix(src).and_then(|r| r.strip_prefix('/')).map(|rest| (format!("{dst}/{rest}"), *render))
        }
    })
}

/// 模版裡某一版的內容 → 指紋。沒有佔位符（或複製時不換佔位符）：整份的雜湊（字串）；
/// 有佔位符：`{"keys": [佔位符名…], "segs": [[字元數, 雜湊]…]}`（佔位符之間的每一段）。不是 UTF-8 的檔照位元組算雜湊。
pub fn fingerprint(content: &[u8], render: bool) -> Value {
    let Ok(text) = std::str::from_utf8(content) else { return Value::String(sha1_16(content)) };
    let text = normalize(text);
    if !render || !placeholder().is_match(&text) {
        return Value::String(sha1_16(text.as_bytes()));
    }
    let (mut keys, mut segs, mut last) = (Vec::new(), Vec::new(), 0usize);
    for c in placeholder().captures_iter(&text) {
        let (Some(all), Some(key)) = (c.get(0), c.get(1)) else { continue };
        let piece = text.get(last..all.start()).unwrap_or_default();
        segs.push(json!([piece.chars().count(), sha1_16(piece.as_bytes())]));
        keys.push(Value::String(key.as_str().to_string()));
        last = all.end();
    }
    let piece = text.get(last..).unwrap_or_default();
    segs.push(json!([piece.chars().count(), sha1_16(piece.as_bytes())]));
    json!({"keys": keys, "segs": segs})
}

/// 專案裡的檔和這個指紋相不相符。`vals` 是專案的佔位符值（有佔位符的檔才用得到）。
pub fn matches(content: &[u8], print: &Value, vals: &Kv) -> bool {
    let Ok(text) = std::str::from_utf8(content) else {
        return print.as_str() == Some(sha1_16(content).as_str());
    };
    let text = normalize(text);
    if let Some(h) = print.as_str() {
        return sha1_16(text.as_bytes()) == h;
    }
    let keys: Vec<&str> = print.get("keys").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let Some(segs) = print.get("segs").and_then(Value::as_array) else { return false };
    if segs.is_empty() || keys.len() + 1 != segs.len() {
        return false;
    }
    let mut pos = 0usize;
    for (i, seg) in segs.iter().enumerate() {
        let n = seg.get(0).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
        let (Some((piece, next)), Some(hash)) = (n.and_then(|n| take_chars(&text, pos, n)), seg.get(1).and_then(Value::as_str)) else { return false };
        if sha1_16(piece.as_bytes()) != hash {
            return false;
        }
        pos = next;
        let Some(key) = keys.get(i) else { continue };
        // 沒有值的佔位符，0.x 的 init 會原樣留著
        let kept = format!("{{{{{key}}}}}");
        let val = vals.get(key).unwrap_or(&kept);
        if !text.get(pos..).is_some_and(|rest| rest.starts_with(val)) {
            return false;
        }
        pos += val.len();
    }
    pos == text.len()
}

// ------------------------------------------------------------------ 產生指紋檔

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git").current_dir(repo).args(args).stdin(Stdio::null()).output().map_err(|e| format!("跑不了 git：{e}"))?;
    if !out.status.success() {
        return Err(format!("git {} 失敗：{}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    git(repo, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// 一次取很多個 blob 的內容（`git cat-file --batch`）。
fn cat_blobs(repo: &Path, shas: &[String]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut child = Command::new("git").current_dir(repo).args(["cat-file", "--batch"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("跑不了 git cat-file：{e}"))?;
    let mut stdin = child.stdin.take().ok_or("git cat-file 沒有 stdin")?;
    let feed: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(feed.as_bytes());
    });
    let out = child.wait_with_output().map_err(|e| format!("git cat-file 失敗：{e}"))?;
    let _ = writer.join();
    let data = out.stdout;
    let (mut pos, mut got) = (0usize, BTreeMap::new());
    for sha in shas {
        let rest = data.get(pos..).unwrap_or_default();
        let nl = rest.iter().position(|b| *b == b'\n').ok_or_else(|| format!("git cat-file 的輸出不完整（{sha}）"))?;
        let head = String::from_utf8_lossy(rest.get(..nl).unwrap_or_default()).into_owned();
        let size: usize = head.rsplit(' ').next().and_then(|n| n.parse().ok()).filter(|_| head.contains(" blob ")).ok_or_else(|| format!("git cat-file 讀不到 {sha}：{head}"))?;
        let body = rest.get(nl + 1..nl + 1 + size).ok_or_else(|| format!("git cat-file 的輸出不完整（{sha}）"))?;
        got.insert(sha.clone(), body.to_vec());
        pos += nl + 1 + size + 1;
    }
    Ok(got)
}

fn print_key(v: &Value) -> (u8, String) {
    match v.as_str() {
        Some(s) => (0, s.to_string()),
        None => (1, v.to_string()),
    }
}

/// 從 git 歷史產生指紋檔的內容：`revs`（tag、分支或 commit）往回走到的每一個 commit 裡，0.x 的 init 會複製的檔的每一個版本。
/// 不只取版本 tag：使用者從 marketplace 裝到的是預設分支當時的內容，兩次發佈之間的 commit 也可能被 init 過。
/// 結果只和 `revs` 指到的 commit 有關（可重現）：`revs` 存成完整的 commit id，檔案與指紋都排序。
pub fn generate(repo: &Path, revs: &[String]) -> Result<Value, String> {
    if revs.is_empty() {
        return Err("沒有給 rev".to_string());
    }
    let mut heads: Vec<String> = Vec::new();
    for r in revs {
        heads.push(git_text(repo, &["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")]).map_err(|e| format!("找不到 rev {r}（{e}）"))?.trim().to_string());
    }
    heads.sort();
    heads.dedup();
    let srcs: Vec<&str> = SOURCES.iter().map(|(s, _, _)| *s).collect();
    let mut args: Vec<&str> = vec!["rev-list"];
    args.extend(heads.iter().map(String::as_str));
    args.push("--");
    args.extend(srcs.iter().copied());
    args.push(PLUGIN_JSON);
    let commits: Vec<String> = git_text(repo, &args)?.lines().map(str::to_string).collect();
    // 專案裡的路徑 → {(blob, 換不換佔位符)}
    let mut seen: BTreeMap<String, BTreeSet<(String, bool)>> = BTreeMap::new();
    let mut version_blobs: BTreeSet<String> = BTreeSet::new();
    for c in &commits {
        let mut a: Vec<&str> = vec!["ls-tree", "-r", "-z", c.as_str(), "--"];
        a.extend(srcs.iter().copied());
        a.push(PLUGIN_JSON);
        for rec in git_text(repo, &a)?.split('\0').filter(|r| !r.is_empty()) {
            let Some((meta, path)) = rec.split_once('\t') else { continue };
            let mut f = meta.split(' ');
            let (Some(_mode), Some("blob"), Some(sha)) = (f.next(), f.next(), f.next()) else { continue };
            if path == PLUGIN_JSON {
                version_blobs.insert(sha.to_string());
            } else if let Some((dst, render)) = project_path(path) {
                seen.entry(dst).or_default().insert((sha.to_string(), render));
            }
        }
    }
    let mut want: BTreeSet<String> = version_blobs.clone();
    want.extend(seen.values().flatten().map(|(s, _)| s.clone()));
    let blobs = cat_blobs(repo, &want.into_iter().collect::<Vec<_>>())?;
    let mut versions: BTreeSet<Vec<u64>> = BTreeSet::new();
    for sha in &version_blobs {
        let v = blobs.get(sha).and_then(|b| serde_json::from_slice::<Value>(b).ok()).and_then(|d| d.get("version").and_then(Value::as_str).map(str::to_string));
        if let Some(parts) = v.and_then(|v| v.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<u64>>>()) {
            versions.insert(parts);
        }
    }
    let mut files = Map::new();
    let mut count = 0usize;
    for (dst, set) in &seen {
        let mut prints: Vec<Value> = Vec::new();
        for (sha, render) in set {
            let p = fingerprint(blobs.get(sha).map(Vec::as_slice).unwrap_or_default(), *render);
            if !prints.contains(&p) {
                prints.push(p);
            }
        }
        prints.sort_by_key(print_key);
        count += prints.len();
        files.insert(dst.clone(), Value::Array(prints));
    }
    let mut doc = Map::new();
    doc.insert(
        "_說明".to_string(),
        json!("0.x 的 init 複製進專案的工具（.claude/teams/tools/、.claude/hooks/、golden-set/run.py）各版內容的指紋。awf init --upgrade 用它認出原樣的舊副本並移除（1.0 的工具都在 awf 執行檔裡）；改過的保留。由 awf-core 的 init::legacy_tools::generate 從 git 歷史產生，不要手改。"),
    );
    doc.insert("revs".to_string(), json!(heads));
    doc.insert("versions".to_string(), json!(versions.iter().map(|v| v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")).collect::<Vec<_>>()));
    doc.insert("count".to_string(), json!({"files": files.len(), "fingerprints": count}));
    doc.insert("files".to_string(), Value::Object(files));
    Ok(Value::Object(doc))
}

/// 指紋檔的文字（鍵照 [`generate`] 的順序，一個檔一行，結尾換行）。
pub fn render(doc: &Value) -> String {
    let mut out = String::from("{\n");
    let obj = doc.as_object().cloned().unwrap_or_default();
    let n = obj.len();
    for (i, (k, v)) in obj.iter().enumerate() {
        let comma = if i + 1 < n { "," } else { "" };
        match (k.as_str(), v.as_object()) {
            ("files", Some(files)) => {
                out.push_str(" \"files\": {\n");
                let m = files.len();
                for (j, (path, prints)) in files.iter().enumerate() {
                    out.push_str(&format!("  {}: {}{}\n", Value::String(path.clone()), prints, if j + 1 < m { "," } else { "" }));
                }
                out.push_str(&format!(" }}{comma}\n"));
            }
            _ => out.push_str(&format!(" {}: {v}{comma}\n", Value::String(k.clone()))),
        }
    }
    out.push_str("}\n");
    out
}

// ------------------------------------------------------------------ 升級時整理

/// 整理的結果（路徑都相對專案根目錄，用 `/`）。
#[derive(Debug, Default, PartialEq)]
pub struct ToolsTidy {
    /// 原樣的舊副本：已移除。
    pub removed: Vec<String>,
    /// 我們放過的路徑、內容和任何一版都對不上（改過）：保留。
    pub modified: Vec<String>,
    /// `.claude/teams/tools/` 底下不是我們放過的檔（使用者自己加的）：不動。
    pub unknown: Vec<String>,
    /// 移除不了的檔與原因。
    pub failed: Vec<String>,
    /// 專案裡有舊工具，但讀不到指紋檔：什麼都沒移除。
    pub no_index: bool,
    /// `.claude/hooks/` 還有 settings 在用，這次沒有整理。
    pub hooks_skipped: bool,
}

impl ToolsTidy {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.modified.is_empty() && self.unknown.is_empty() && self.failed.is_empty()
    }
}

/// 清 `dir/__pycache__`：原始檔（同目錄的 `<名>.py`）已經不在的 `.pyc`／`.pyo` 移除，清空了就連目錄一起移除。
/// 還留著的 `.py`（改過的、使用者自己的）的快取不動；不是快取的檔也不動。
fn clean_cache(dir: &Path) {
    let cache = dir.join("__pycache__");
    let Ok(rd) = std::fs::read_dir(&cache) else { return };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !(e.path().is_file() && (n.ends_with(".pyc") || n.ends_with(".pyo"))) {
            continue;
        }
        let stem = n.split('.').next().unwrap_or("");
        if !dir.join(format!("{stem}.py")).is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let _ = std::fs::remove_dir(&cache);
}

/// 由下往上清：`__pycache__` 裡原始檔已經不在的快取（見 `clean_cache`）、以及空目錄。`root` 自己空了也移除。
fn prune(root: &Path) {
    fn walk(dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let subs: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
        for d in &subs {
            if d.file_name().is_some_and(|n| n == "__pycache__") {
                clean_cache(dir);
            } else {
                walk(d);
            }
        }
        // 只剩 .DS_Store 的目錄也算空的
        let left: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        if !left.is_empty() && left.iter().all(|p| p.file_name().is_some_and(|n| n == ".DS_Store")) {
            for p in &left {
                let _ = std::fs::remove_file(p);
            }
        }
        let _ = std::fs::remove_dir(dir); // 不是空的會失敗，正好
    }
    if root.is_dir() {
        walk(root);
    }
}

/// 讀指紋檔的 `files`；讀不到或形狀不對回 None。
pub fn load_index(template_root: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(template_root.join(FILE)).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.get("files")?.as_object().cloned()
}

/// 升級時整理專案裡 0.x 的工具副本。`skip_hooks`：`.claude/hooks/` 這次不碰（settings 還在用裡面的腳本）。
/// `.claude/hooks/bin/`（1.0 放執行檔的地方）與 `.claude/hooks/` 裡不是我們放過的檔一律不碰。
pub fn tidy_old_tools(target: &Path, index: Option<&Map<String, Value>>, vals: &Kv, skip_hooks: bool) -> ToolsTidy {
    let mut out = ToolsTidy::default();
    let tools = target.join(".claude").join("teams").join("tools");
    let hooks = target.join(".claude").join("hooks");
    let golden = target.join(".claude").join("teams").join("golden-set").join("run.py");
    let mut cands: Vec<PathBuf> = Vec::new();
    cands.extend(walk_files(&tools));
    if !skip_hooks {
        cands.extend(walk_files(&hooks).into_iter().filter(|p| !p.starts_with(hooks.join("bin"))));
    }
    if golden.is_file() {
        cands.push(golden);
    }
    cands.retain(|p| !skip_file(p));
    out.hooks_skipped = skip_hooks && hooks.is_dir() && walk_files(&hooks).iter().any(|p| !p.starts_with(hooks.join("bin")) && !skip_file(p));
    let Some(index) = index else {
        out.no_index = cands.iter().any(|p| p.starts_with(&tools));
        return out;
    };
    for p in cands {
        let rel = posix(p.strip_prefix(target).unwrap_or(&p));
        let Some(prints) = index.get(&rel).and_then(Value::as_array) else {
            if p.starts_with(&tools) {
                out.unknown.push(rel);
            }
            continue;
        };
        let Ok(content) = std::fs::read(&p) else {
            out.modified.push(rel);
            continue;
        };
        if prints.iter().any(|fp| matches(&content, fp, vals)) {
            match std::fs::remove_file(&p) {
                Ok(()) => out.removed.push(rel),
                Err(e) => out.failed.push(format!("{rel}（{e}）")),
            }
        } else if p.starts_with(&hooks) && !std::str::from_utf8(&content).is_ok_and(|t| ours().is_match(t)) {
            // .claude/hooks/ 是和使用者共用的目錄：同名、但看不出是模版的檔，當成使用者自己的，不列
            continue;
        } else {
            out.modified.push(rel);
        }
    }
    if !out.removed.is_empty() {
        prune(&tools);
        if !skip_hooks {
            for sub in ["tests"] {
                prune(&hooks.join(sub));
            }
            // .claude/hooks/ 自己：只清我們留下的 __pycache__，空了才移除（裡面可能有使用者的 hook、或 1.0 的 bin/）
            clean_cache(&hooks);
            let _ = std::fs::remove_dir(&hooks);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths() {
        assert_eq!(project_path("scaffold/.claude/teams/tools/awf.py"), Some((".claude/teams/tools/awf.py".to_string(), true)));
        assert_eq!(project_path("scaffold/.claude/teams/tools/snippets/kernel.js"), Some((".claude/teams/tools/snippets/kernel.js".to_string(), true)));
        assert_eq!(project_path("plugin/hooks/tests/run.py"), Some((".claude/hooks/tests/run.py".to_string(), false)));
        assert_eq!(project_path("scaffold/.claude/teams/golden-set/run.py"), Some((".claude/teams/golden-set/run.py".to_string(), true)));
        assert_eq!(project_path("plugin/hooks/hooks.json"), None);
        assert_eq!(project_path("plugin/hooks/__pycache__/guard.cpython-312.pyc"), None);
        assert_eq!(project_path("scaffold/.claude/teams/toolsx/a.py"), None);
        assert_eq!(project_path("scaffold/.claude/teams/rules.md"), None);
    }

    #[test]
    fn whole_file_fingerprint_ignores_line_endings_only() {
        let fp = fingerprint("a = 1\nb = 2\n".as_bytes(), true);
        assert!(fp.is_string());
        let vals = Kv::default();
        assert!(matches(b"a = 1\nb = 2\n", &fp, &vals));
        assert!(matches(b"a = 1\r\nb = 2\r\n", &fp, &vals));
        assert!(!matches(b"a = 1\nb = 2\n\n", &fp, &vals));
        assert!(!matches(b"a = 1\nb = 3\n", &fp, &vals));
        assert!(!matches(b"a = 1 \nb = 2\n", &fp, &vals));
        // 不是 UTF-8：照位元組比
        let bin = fingerprint(&[0xff, 0x00, 0x0d, 0x0a], true);
        assert!(matches(&[0xff, 0x00, 0x0d, 0x0a], &bin, &vals));
        assert!(!matches(&[0xff, 0x00, 0x0a], &bin, &vals));
    }

    #[test]
    fn placeholder_fingerprint_needs_the_projects_values() {
        let open = "{".repeat(2);
        let close = "}".repeat(2);
        let src = format!("RUNS = \"{open}RUNS_DIR{close}\"\nNS = '{open}AGENT_NS{close}'\n# 結尾\n");
        let fp = fingerprint(src.as_bytes(), true);
        assert_eq!(fp["keys"], json!(["RUNS_DIR", "AGENT_NS"]));
        let mut vals = Kv::default();
        vals.set("RUNS_DIR", "_acme/runs");
        vals.set("AGENT_NS", "示範:");
        let landed = "RUNS = \"_acme/runs\"\nNS = '示範:'\n# 結尾\n";
        assert!(matches(landed.as_bytes(), &fp, &vals));
        assert!(matches(landed.replace('\n', "\r\n").as_bytes(), &fp, &vals));
        assert!(!matches(landed.replace("結尾", "改過").as_bytes(), &fp, &vals));
        assert!(!matches(landed.replace("_acme/runs", "_tpm/runs").as_bytes(), &fp, &vals));
        assert!(!matches(format!("{landed} ").as_bytes(), &fp, &vals));
        // 複製時不換佔位符的檔（hook）：整份算
        assert!(fingerprint(src.as_bytes(), false).is_string());
        // 專案沒有這個值：0.x 的 init 會把佔位符原樣留著
        let mut partial = Kv::default();
        partial.set("RUNS_DIR", "r");
        assert!(matches(format!("RUNS = \"r\"\nNS = '{open}AGENT_NS{close}'\n# 結尾\n").as_bytes(), &fp, &partial));
    }

    #[test]
    fn broken_fingerprints_never_match() {
        let vals = Kv::default();
        for fp in [json!({}), json!({"keys": ["A"], "segs": []}), json!({"keys": [], "segs": [[0, "x"], [0, "y"]]}), json!(null), json!(3)] {
            assert!(!matches(b"", &fp, &vals), "{fp}");
        }
    }

    #[test]
    fn render_is_stable_json() {
        let doc = json!({"_說明": "x", "revs": ["a"], "files": {"p/a.py": ["h1", {"keys": ["K"], "segs": [[1, "h"], [0, "g"]]}], "p/b.py": ["h2"]}});
        let text = render(&doc);
        assert_eq!(serde_json::from_str::<Value>(&text).ok(), Some(doc));
        assert!(text.ends_with("}\n") && text.contains("\n  \"p/b.py\": [\"h2\"]\n"), "{text}");
    }
}
