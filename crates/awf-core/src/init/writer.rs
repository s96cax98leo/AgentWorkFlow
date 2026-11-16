//! 把模版檔寫進專案：替換佔位符、去掉 `.tmpl`，以及「不破壞使用者的既有設定」的每一條規則。
//!
//! - 「複製哪些檔」：[`scaffold_plan`]、[`workflow_plan`]（回傳來源與目的地的清單）。1.0 起工具都在 `awf` 執行檔裡，
//!   scaffold 裡萬一還有 `.claude/teams/tools/` 或 `.py`（0.x 的模版）也不複製（[`not_copied`]）。
//! - 「hook 怎麼註冊」：[`hook_command`]（寫進 settings 的指令）與 [`dest_for`] 裡選哪一份 settings 模版；
//!   `--hooks project` 用的執行檔與 merge gate 的執行檔由 [`Writer::put_bytes`] 放。

use super::merge::{heading_at, merge_settings, missing_fields, refresh_notes};
use super::{crash, read_raw, read_text, read_text_sig, Kv, R};
use crate::kb::pyish;
use crate::merge_gate::pyjson;
use crate::merge_gate::pyutil::ReadErr;
use crate::pyfmt;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub const CLAUDE_IMPORT: &str = ".claude/teams/agent-workflow.CLAUDE.md";
/// 模版 CLAUDE.md 第一行的標記，用來認出 init 自己產生的 CLAUDE.md。
pub const CLAUDE_MARK: &str = "<!-- agent-workflow：由 awf init 從模版產生";
/// 0.x 的 init 寫的標記：既有專案的 CLAUDE.md 還是這一行，照樣認（`--force` 更新時會換成新的）。
pub const CLAUDE_MARK_0X: &str = "<!-- agent-workflow：由 init.py 從模版產生";

/// 這份 CLAUDE.md 是不是 init 產生的（新舊兩種標記都認）。
pub fn is_generated_claude_md(body: &str) -> bool {
    body.contains(CLAUDE_MARK) || body.contains(CLAUDE_MARK_0X)
}
/// 使用者的檔案：已存在時不整份覆蓋（`--force` 也一樣），改走合併；只有 `--overwrite-user-files` 才整份覆蓋。
pub const MERGE_JSON: &[&str] = &[".claude/settings.json", ".mcp.json"];
/// 和使用者共用的目錄：同名檔案要看得出是模版產生的，`--force` 才覆蓋（否則當成使用者的檔案保留）。
pub const SHARED_DIRS: &[&str] = &[".claude/workflows/", ".claude/hooks/", ".github/workflows/"];
pub const GUARD_CFG: &str = ".claude/teams/guard.config.json";
/// init 自己算的佔位符（不從 workflow.config 讀），值已是 JSON 片段，原樣代入。
pub const COMPUTED: &[&str] = &["AWF_HOOK_CMD", "AWF_HOOK_SHELL"];

/// 模版檔裡由使用者填寫的段：`--force`／`--upgrade` 時這個標題（含）之後保留使用者的內容，之前換成新版模版。
pub fn splice_heading(rel: &str) -> Option<&'static str> {
    match rel {
        ".claude/teams/rules.md" => Some("## 專案規則"),
        CLAUDE_IMPORT | "CLAUDE.md" => Some("## 專案特定"),
        _ => None,
    }
}

fn ph() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\{\{([A-Z_]+)\}\}").expect("固定的正規式"))
}

/// 看得出是模版產生的檔。
pub fn ours() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"agent-workflow|agent-merge-gate|/\*@schema:").expect("固定的正規式"))
}

/// 覆寫檔的標記（`// awf-override: <版本>`）。
pub fn wf_override() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    let ws = pyish::WS_CLASS;
    R.get_or_init(|| Regex::new(&format!(r"(?m)^// awf-override: *([^{ws}]+)[{ws}]*$")).expect("固定的正規式"))
}

/// `json.dumps(s, ensure_ascii=False)[1:-1]`：字串放進 JSON 檔時的跳脫。
pub fn json_escape(s: &str) -> String {
    let q = pyish::json_str(s);
    q.get(1..q.len().saturating_sub(1)).unwrap_or_default().to_string()
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `p.as_posix()`（相對路徑）。
pub fn posix(p: &Path) -> String {
    p.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
        Component::ParentDir => Some("..".to_string()),
        _ => None,
    }).collect::<Vec<_>>().join("/")
}

/// `__pycache__` 與 `.DS_Store` 不複製、也不算殘留。
pub fn skip_file(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str() == "__pycache__") || p.file_name().is_some_and(|n| n == ".DS_Store")
}

/// `sorted(p for p in root.rglob("*") if p.is_file())`：依路徑的各段排序。
pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let is_real_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_real_dir {
                walk(&p, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

/// `sorted(dir.glob("*.js"))`（只看這一層）。
pub fn js_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| file_name(p).ends_with(".js"))
        .collect();
    out.sort();
    out
}

/// 一個要放進專案的檔：來源、目的地、要不要替換佔位符。
#[derive(Debug, Clone)]
pub struct Item {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub text_render: bool,
}

/// 決定 scaffold 裡的一個檔放到專案的哪裡（相對路徑 `rel`）；回 None 表示這次不放。
pub fn dest_for(rel: &Path, target: &Path, runs: &str, hooks: &str, with_mcp: bool, with_merge_gate: bool) -> Option<PathBuf> {
    let name = file_name(rel);
    let mut rel = rel.to_path_buf();
    let mut parts = rel.components();
    if parts.next().is_some_and(|c| c.as_os_str() == "_runs") {
        rel = Path::new(runs).join(parts.as_path());
    }
    let posix = posix(&rel);
    if posix == ".github/workflows/agent-merge-gate.yml.tmpl" && !with_merge_gate {
        return None; // 沒有 gate App 時這個 CI 會讓每個 PR 失敗，預設不放
    }
    if posix == ".claude/settings.json.tmpl" && hooks != "plugin" {
        return None;
    }
    if posix == ".claude/settings.project-hooks.json.tmpl" {
        if hooks != "project" {
            return None;
        }
        rel = rel.with_file_name("settings.json.tmpl");
    }
    if posix == ".mcp.json.tmpl" && !with_mcp {
        rel = rel.with_file_name(".mcp.json.example.tmpl");
    }
    if name.ends_with(".tmpl") {
        let cur = file_name(&rel);
        let cut = cur.len().saturating_sub(".tmpl".len());
        rel = rel.with_file_name(cur.get(..cut).unwrap_or_default());
    }
    Some(target.join(rel))
}

/// 不複製進專案的 scaffold 檔（相對 scaffold 的路徑）：`.claude/teams/tools/` 整個目錄與任何 `.py`。
/// 這個 repo 的 scaffold 已經沒有這些檔；規則留著，`--source` 指到還帶著 0.x 工具的模版時一樣不會放進專案。
pub fn not_copied(rel: &Path) -> bool {
    let p = posix(rel);
    p.starts_with(".claude/teams/tools/") || p.ends_with(".py")
}

/// `--hooks project` 時執行檔在專案裡的位置（相對專案根目錄）。
pub fn hook_bin_rel() -> &'static str {
    if cfg!(windows) { ".claude/hooks/bin/awf.exe" } else { ".claude/hooks/bin/awf" }
}

/// merge gate 的 CI 用的執行檔與雜湊檔在專案裡的位置（CI 範本寫的也是這兩個路徑）。
pub const GATE_BIN_REL: &str = ".claude/teams/bin/awf-linux-x64";
pub const GATE_SUMS_REL: &str = ".claude/teams/bin/checksums.json";
/// `checksums.json` 裡 linux-x64 執行檔的鍵（發佈檔名，見 `crates/xtask/src/checksums.rs`）。
pub const GATE_BIN_NAME: &str = "awf-linux-x64";

/// 複製哪些檔（一）：`scaffold/` 底下的設定（[`not_copied`] 的不放），依 [`dest_for`] 決定位置。
pub fn scaffold_plan(scaffold: &Path, target: &Path, runs: &str, hooks: &str, with_mcp: bool, with_merge_gate: bool) -> Vec<Item> {
    let mut out = Vec::new();
    for src in walk_files(scaffold) {
        if skip_file(&src) {
            continue;
        }
        let Ok(rel) = src.strip_prefix(scaffold) else { continue };
        if not_copied(rel) {
            continue;
        }
        if let Some(dst) = dest_for(rel, target, runs, hooks, with_mcp, with_merge_gate) {
            out.push(Item { src, dst, text_render: true });
        }
    }
    out
}

/// 複製哪些檔（二）：`--with-workflows` 的覆寫檔（`plugin/workflow-templates/*.js`）。
pub fn workflow_plan(wf_templates: &Path, target: &Path) -> Vec<Item> {
    js_files(wf_templates).into_iter().map(|src| {
        let dst = target.join(".claude").join("workflows").join(file_name(&src));
        Item { src, dst, text_render: true }
    }).collect()
}

/// hook 怎麼註冊：回傳（AWF_HOOK_CMD, AWF_HOOK_SHELL），都是已經 JSON 跳脫好的片段。範本在後面接「 guard-shell」等種類。
///
/// `bin` 是執行檔的絕對路徑（專案裡的那份複本）。
/// - macOS／Linux：shell form 交給 sh -c，路徑加引號。
/// - Windows：明確指定 `"shell": "powershell"`，用呼叫運算子 `&` 執行。
pub fn hook_command(bin: &str) -> (String, String) {
    let (cmd, shell) = if cfg!(windows) {
        (format!("& \"{bin}\" hook"), ", \"shell\": \"powershell\"")
    } else {
        (format!("\"{bin}\" hook"), "")
    };
    (json_escape(&cmd), shell.to_string())
}

/// 原樣寫出（不轉換換行），上層目錄沒有就建立。
pub fn write_text(dst: &Path, text: &str) -> R<()> {
    if let Some(d) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(d) {
            return crash(format!("建不了目錄 {}：{e}", d.display()));
        }
    }
    match std::fs::write(dst, text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) => crash(format!("寫不進 {}：{e}", dst.display())),
    }
}

fn os_or_crash<T>(r: Result<T, ReadErr>, what: &Path) -> R<T> {
    match r {
        Ok(t) => Ok(t),
        Err(e) => crash(format!("讀不了 {}：{}", what.display(), e.msg())),
    }
}

pub struct Writer<'a> {
    pub target: &'a Path,
    pub vals: &'a Kv,
    pub force: bool,
    pub overwrite_user: bool,
    /// 這版模版會產生的所有檔案（用來找舊版殘留）。
    pub seen: HashSet<PathBuf>,
    /// 模版檔裡的使用者設定：更新模版部分、保留使用者部分。
    pub preserved: Vec<(PathBuf, String)>,
    pub written: Vec<PathBuf>,
    /// 模版檔已存在（加 `--force` 更新）。
    pub skipped: Vec<PathBuf>,
    /// 使用者的檔案，保留不動。
    pub user_kept: Vec<PathBuf>,
    /// 使用者的檔案，合併或追加了模版的內容。
    pub merged: Vec<PathBuf>,
    pub leftovers: Vec<String>,
    /// 過程中要印的訊息（呼叫端依序印到 stdout）。
    pub notes: Vec<String>,
}

impl<'a> Writer<'a> {
    pub fn new(target: &'a Path, vals: &'a Kv, force: bool, overwrite_user: bool) -> Self {
        Self {
            target, vals, force, overwrite_user,
            seen: HashSet::new(), preserved: Vec::new(), written: Vec::new(), skipped: Vec::new(),
            user_kept: Vec::new(), merged: Vec::new(), leftovers: Vec::new(), notes: Vec::new(),
        }
    }

    /// 相對專案根目錄的路徑（用 `/`）。
    pub fn rel(&self, p: &Path) -> String {
        posix(p.strip_prefix(self.target).unwrap_or(p))
    }

    /// 替換佔位符；JSON 檔裡的值照 JSON 字串跳脫。沒有值的佔位符留著。
    pub fn render(&self, text: &str, as_json: bool) -> String {
        ph().replace_all(text, |c: &regex::Captures| {
            let all = c.get(0).map(|m| m.as_str()).unwrap_or_default();
            let k = c.get(1).map(|m| m.as_str()).unwrap_or_default();
            match self.vals.get(k) {
                None => all.to_string(),
                Some(v) if COMPUTED.contains(&k) => v.to_string(),
                Some(v) if as_json => json_escape(v),
                Some(v) => v.to_string(),
            }
        }).into_owned()
    }

    fn render_file(&mut self, src: &Path, dst: &Path) -> Result<String, ReadErr> {
        let text = read_text(src)?;
        let name = file_name(dst);
        let is_json = (name.len() > ".json".len() && name.ends_with(".json")) || name.ends_with(".json.example");
        let text = self.render(&text, is_json);
        let rel = self.rel(dst);
        for m in ph().find_iter(&text) {
            self.leftovers.push(format!("{rel}：{}", m.as_str()));
        }
        Ok(text)
    }

    /// 使用者可能已經有的檔案。已處理回 true；回 false 時交給一般流程（目標不存在或 `--overwrite-user-files`）。
    fn put_user(&mut self, src: &Path, dst: &Path) -> R<bool> {
        if !dst.exists() || self.overwrite_user {
            return Ok(false);
        }
        let rel = self.rel(dst);
        if rel == "CLAUDE.md" {
            self.put_claude_md(src, dst)?;
            return Ok(true);
        }
        if self.force && rel == GUARD_CFG {
            self.put_guard(src, dst)?;
            return Ok(true);
        }
        if let (true, Some(heading)) = (self.force, splice_heading(&rel)) {
            self.put_splice(src, dst, heading)?;
            return Ok(true);
        }
        if MERGE_JSON.contains(&rel.as_str()) {
            let text = os_or_crash(self.render_file(src, dst), src)?;
            let user = match read_text_sig(dst) {
                Ok(t) => pyjson::loads(&t).ok(),
                Err(ReadErr::Os(_)) => None,
                Err(e @ ReadErr::Decode(_)) => return os_or_crash(Err(e), dst),
            };
            let (Some(user), Ok(tmpl)) = (user, pyjson::loads(&text)) else {
                self.user_kept.push(dst.to_path_buf());
                self.notes.push(format!("注意：{rel} 不是合法 JSON，沒有合併；請照 {} 手動補上模版的設定", file_name(src)));
                return Ok(true);
            };
            let Value::Object(user) = user else {
                self.user_kept.push(dst.to_path_buf());
                return Ok(true);
            };
            let Value::Object(tmpl) = tmpl else {
                return crash(format!("模版 {} 不是 JSON 物件", file_name(src)));
            };
            let merged = Value::Object(merge_settings(&user, &tmpl)?);
            if !pyfmt::eq(&merged, &Value::Object(user)) {
                write_text(dst, &format!("{}\n", crate::recipe::dumps(&merged, Some(2))))?;
                self.merged.push(dst.to_path_buf());
            } else {
                self.user_kept.push(dst.to_path_buf());
            }
            return Ok(true);
        }
        if SHARED_DIRS.iter().any(|d| rel.starts_with(d)) {
            let is_ours = read_text(dst).map(|t| ours().is_match(&t)).unwrap_or(false);
            if !is_ours {
                self.user_kept.push(dst.to_path_buf());
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 既有 CLAUDE.md：init 自己產生的照一般規則（`--force` 才更新）；使用者的不覆蓋，
    /// 模版內容寫成 `.claude/teams/agent-workflow.CLAUDE.md`，再在結尾追加一段 @ 引用（已引用過就不再追加）。
    fn put_claude_md(&mut self, src: &Path, dst: &Path) -> R<()> {
        let body = os_or_crash(read_raw(dst), dst)?;
        if is_generated_claude_md(&body) {
            // 先前 init 在空專案產生的 CLAUDE.md
            if self.force {
                self.put_splice(src, dst, "## 專案特定")?;
            } else {
                self.skipped.push(dst.to_path_buf());
            }
            return Ok(());
        }
        let import = self.target.join(CLAUDE_IMPORT);
        self.put(src, &import, true)?;
        if body.contains(&format!("@{CLAUDE_IMPORT}")) {
            self.user_kept.push(dst.to_path_buf());
            return Ok(());
        }
        let nl = if body.contains("\r\n") { "\r\n" } else { "\n" };
        let lead = if body.is_empty() || body.ends_with('\n') || body.ends_with('\r') { "" } else { nl };
        let block = format!("{lead}{nl}<!-- agent-workflow：模版的工作流程說明（awf init 追加；移除模版時刪掉這兩行） -->{nl}@{CLAUDE_IMPORT}{nl}");
        let appended = std::fs::OpenOptions::new().append(true).open(dst).and_then(|mut f| std::io::Write::write_all(&mut f, block.as_bytes()));
        if let Err(e) = appended {
            return crash(format!("寫不進 {}：{e}", dst.display()));
        }
        self.merged.push(dst.to_path_buf());
        Ok(())
    }

    /// guard.config.json：使用者的值一律保留，只補上新版模版新增的鍵，說明文字換成新版。
    fn put_guard(&mut self, src: &Path, dst: &Path) -> R<()> {
        let text = os_or_crash(self.render_file(src, dst), src)?;
        let user = match read_text_sig(dst) {
            Ok(t) => pyjson::loads(&t).ok(),
            Err(ReadErr::Os(_)) => None,
            Err(e @ ReadErr::Decode(_)) => return os_or_crash(Err(e), dst),
        };
        let both = match (user, pyjson::loads(&text)) {
            (Some(Value::Object(u)), Ok(t)) => Some((u, t)),
            _ => None,
        };
        let Some((user, tmpl)) = both else {
            self.user_kept.push(dst.to_path_buf());
            self.notes.push(format!("注意：{} 不是合法 JSON 物件，沒有更新；請照模版 {} 手動補上新欄位", self.rel(dst), file_name(src)));
            return Ok(());
        };
        let Value::Object(tmpl) = tmpl else {
            return crash(format!("模版 {} 不是 JSON 物件", file_name(src)));
        };
        let added: Vec<String> = missing_fields(&tmpl, &user, "", false).into_iter().map(|(p, _)| p)
            .filter(|p| !p.rsplit('.').next().unwrap_or_default().starts_with("_說明"))
            .collect();
        let mut merged = merge_settings(&user, &tmpl)?;
        refresh_notes(&mut merged, &tmpl);
        let merged = Value::Object(merged);
        if !pyfmt::eq(&merged, &Value::Object(user)) {
            write_text(dst, &format!("{}\n", crate::recipe::dumps(&merged, Some(2))))?;
        }
        let how = if added.is_empty() { "沒有新欄位".to_string() } else { format!("補上新欄位：{}", added.join("、")) };
        self.preserved.push((dst.to_path_buf(), format!("你的值全部保留；{how}")));
        Ok(())
    }

    /// 標題之前換成新版模版，標題（含）之後保留使用者的內容；任一邊找不到標題就整份保留。
    fn put_splice(&mut self, src: &Path, dst: &Path, heading: &str) -> R<()> {
        let new = os_or_crash(self.render_file(src, dst), src)?;
        let old = os_or_crash(read_raw(dst), dst)?;
        let (Some(i_new), Some(i_old)) = (heading_at(&new, heading), heading_at(&old, heading)) else {
            self.user_kept.push(dst.to_path_buf());
            self.notes.push(format!("注意：{} 找不到「{heading}」標題，整份保留沒有更新；要更新請手動對照模版 {}", self.rel(dst), file_name(src)));
            return Ok(());
        };
        let out = format!("{}{}", new.get(..i_new).unwrap_or_default(), old.get(i_old..).unwrap_or_default());
        if out != old {
            write_text(dst, &out)?;
        }
        let title = heading.trim_start_matches(['#', ' ']);
        self.preserved.push((dst.to_path_buf(), format!("「{title}」段之後保留，前面更新成新版")));
        Ok(())
    }

    /// 放一個不是從 scaffold 文字檔來的模版檔（執行檔、雜湊檔）：已存在而且沒加 `--force` 就略過，其餘照寫。
    /// `exec` 時設成可執行。先寫到旁邊的暫存檔再改名（目的地可能正在被 hook 執行）。
    pub fn put_bytes(&mut self, dst: &Path, bytes: &[u8], exec: bool) -> R<()> {
        self.seen.insert(dst.to_path_buf());
        if dst.exists() && !(self.force || self.overwrite_user) {
            self.skipped.push(dst.to_path_buf());
            return Ok(());
        }
        if std::fs::read(dst).is_ok_and(|old| old == bytes) && is_exec(dst) == exec {
            self.written.push(dst.to_path_buf()); // 內容相同：不重寫（可能就是正在執行的這一支）
            return Ok(());
        }
        let Some(dir) = dst.parent() else { return crash(format!("寫不進 {}", dst.display())) };
        if let Err(e) = std::fs::create_dir_all(dir) {
            return crash(format!("建不了目錄 {}：{e}", dir.display()));
        }
        let tmp = dir.join(format!(".{}.awf-new", file_name(dst)));
        let done = std::fs::write(&tmp, bytes).and_then(|()| set_exec(&tmp, exec)).and_then(|()| std::fs::rename(&tmp, dst));
        if let Err(e) = done {
            let _ = std::fs::remove_file(&tmp);
            return crash(format!("寫不進 {}：{e}", dst.display()));
        }
        self.written.push(dst.to_path_buf());
        Ok(())
    }

    /// 放一個模版檔。使用者的檔案不覆寫（規則見 `put_user`）。
    pub fn put(&mut self, src: &Path, dst: &Path, text_render: bool) -> R<()> {
        self.seen.insert(dst.to_path_buf());
        if self.put_user(src, dst)? {
            return Ok(());
        }
        if dst.exists() && !(self.force || self.overwrite_user) {
            self.skipped.push(dst.to_path_buf());
            return Ok(());
        }
        if let Some(d) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(d) {
                return crash(format!("建不了目錄 {}：{e}", d.display()));
            }
        }
        let text = if text_render { self.render_file(src, dst) } else { read_text(src) };
        let text = match text {
            Ok(t) => t,
            Err(ReadErr::Decode(_)) => {
                // 不是文字檔：原樣複製（含權限）
                if let Err(e) = std::fs::copy(src, dst) {
                    return crash(format!("複製不了 {} → {}：{e}", src.display(), dst.display()));
                }
                self.written.push(dst.to_path_buf());
                return Ok(());
            }
            Err(e) => return os_or_crash(Err(e), src),
        };
        write_text(dst, &text)?;
        // shutil.copymode：權限照來源檔
        if let Ok(meta) = std::fs::metadata(src) {
            if let Err(e) = std::fs::set_permissions(dst, meta.permissions()) {
                return crash(format!("改不了 {} 的權限：{e}", dst.display()));
            }
        }
        self.written.push(dst.to_path_buf());
        Ok(())
    }
}

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_exec(_p: &Path) -> bool {
    true
}

#[cfg(unix)]
fn set_exec(p: &Path, exec: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 }))
}

#[cfg(not(unix))]
fn set_exec(_p: &Path, _exec: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(rel: &str, hooks: &str, mcp: bool, gate: bool) -> Option<String> {
        dest_for(Path::new(rel), Path::new("/t"), "_x/runs", hooks, mcp, gate).map(|p| posix(p.strip_prefix("/t").unwrap_or(&p)))
    }

    #[test]
    fn dest_rules() {
        assert_eq!(dest("_runs/README.md", "plugin", false, false).as_deref(), Some("_x/runs/README.md"));
        assert_eq!(dest("CLAUDE.md.tmpl", "plugin", false, false).as_deref(), Some("CLAUDE.md"));
        assert_eq!(dest(".mcp.json.tmpl", "plugin", false, false).as_deref(), Some(".mcp.json.example"));
        assert_eq!(dest(".mcp.json.tmpl", "plugin", true, false).as_deref(), Some(".mcp.json"));
        assert_eq!(dest(".claude/settings.json.tmpl", "project", false, false), None);
        assert_eq!(dest(".claude/settings.project-hooks.json.tmpl", "project", false, false).as_deref(), Some(".claude/settings.json"));
        assert_eq!(dest(".claude/settings.project-hooks.json.tmpl", "plugin", false, false), None);
        assert_eq!(dest(".github/workflows/agent-merge-gate.yml.tmpl", "plugin", false, false), None);
        assert_eq!(dest(".github/workflows/agent-merge-gate.yml.tmpl", "plugin", false, true).as_deref(), Some(".github/workflows/agent-merge-gate.yml"));
    }

    #[test]
    fn tools_and_python_are_not_copied() {
        for rel in [".claude/teams/tools/awf.py", ".claude/teams/tools/snippets/kernel.js", ".claude/teams/tools/check_workflows.mjs", ".claude/teams/golden-set/run.py"] {
            assert!(not_copied(Path::new(rel)), "{rel}");
        }
        for rel in [".claude/teams/rules.md", ".claude/teams/schemas/envelope.schema.json", ".claude/teams/golden-set/valid/a.json", ".claude/teams/toolsx/a.md", "_runs/README.md"] {
            assert!(!not_copied(Path::new(rel)), "{rel}");
        }
    }

    #[test]
    fn hook_command_quotes_the_binary() {
        let (cmd, shell) = hook_command("/p/示範 專案/.claude/hooks/bin/awf");
        if cfg!(windows) {
            assert_eq!((cmd.as_str(), shell.as_str()), ("& \\\"/p/示範 專案/.claude/hooks/bin/awf\\\" hook", ", \"shell\": \"powershell\""));
        } else {
            assert_eq!((cmd.as_str(), shell.as_str()), ("\\\"/p/示範 專案/.claude/hooks/bin/awf\\\" hook", ""));
        }
    }

    #[test]
    fn override_mark() {
        assert!(wf_override().is_match("x\n// awf-override: 0.5.0\n"));
        assert!(wf_override().is_match("// awf-override:1 \n\n"));
        assert!(!wf_override().is_match("x // awf-override: 1\n"));
        assert!(!wf_override().is_match("// awf-override: \n"));
    }
}
