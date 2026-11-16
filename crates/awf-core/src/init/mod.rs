//! 把模版套進專案（複製 scaffold 的設定、替換佔位符、合併既有設定、升級）。
//!
//! 起點是 0.x 的 Python 版 `init.py` 的移植；1.0 起行為不同了（對跑基準 `tests/parity/init.expected.json` 改當這個實作的回歸基準）：
//! - **工具不複製進專案**：`scaffold/.claude/teams/tools/` 與 scaffold 裡的 `.py` 不放（工具都在 `awf` 執行檔裡）。
//!   覆寫檔（`--with-workflows`）要的片段，內嵌時先找專案自己的 `tools/snippets/`（舊專案還有時），再找模版的。
//! - **`--hooks project`**：把這支執行檔複製到 `<專案>/.claude/hooks/bin/`，settings 用它的絕對路徑註冊 `awf hook <種類>`。
//!   不需要 Python；`--python` 收下、提示一行、忽略。
//! - **`--upgrade`／`--force`**：0.x 複製進專案的工具副本，指紋相符的移除、改過的保留並列出（[`legacy_tools`]）；
//!   settings 裡 0.x 註冊的 hook 與權限換成 1.0 的寫法，使用者自己的不動。
//! - **`--with-merge-gate`**：另外把 linux-x64 的執行檔與 `checksums.json` 放進 `.claude/teams/bin/`（CI 從受保護的 base 取出來跑）。
//!
//! 其他細節：
//! - 模版根目錄就是 plugin 目錄（底下有 `scaffold/`、`workflow-templates/`、`workflow.config.example`）：`--source` 優先，
//!   其餘照 [`crate::template::find`]（`AWF_TEMPLATE_DIR` → `AWF_PLUGIN_DIR`／`CLAUDE_PLUGIN_ROOT` → 從執行檔往上找）。
//!   `--source` 與 `AWF_TEMPLATE_DIR` 給 plugin 目錄或 repo 根（底下有 `plugin/`）都可以。
//! - 最後的內嵌（build）在程式內呼叫 [`crate::build`]。
//! - 檔案不是 UTF-8、設定的形狀不對、寫不進檔案：在 stderr 印一行說明，exit code 1。
//! - JSON 裡的 `NaN`、`Infinity` 當成不合法：對使用者的檔案只會變成「沒有合併、原樣保留」。
//! - `~使用者名` 開頭的路徑不展開（只展開 `~`）。
//!
//! 核心函式都收明確的模版根（[`Site`]）與目標路徑，不讀行程的目前目錄與環境變數；那些由 [`Env::from_process`] 收集。

mod args;
pub mod detect;
pub mod interview;
mod legacy;
pub mod legacy_tools;
mod merge;
mod profile;
mod writer;

pub use args::Args;
pub use legacy::{pristine_legacy_workflow, tidy_old_workflows, Tidy};
pub use legacy_tools::{tidy_old_tools, ToolsTidy};
pub use merge::{deep_merge, hook_forms, merge_hooks, merge_settings, migrate_allow, missing_fields, set_missing};
pub use profile::{apply_answers, parse_constraints};
pub use writer::{dest_for, hook_bin_rel, hook_command, is_generated_claude_md, not_copied, scaffold_plan, workflow_plan, Item, Writer, CLAUDE_MARK, CLAUDE_MARK_0X, GATE_BIN_NAME, GATE_BIN_REL, GATE_SUMS_REL};

use crate::kb::pyish::WS_CLASS;
use crate::merge_gate::pyutil::{self, ReadErr};
use crate::merge_gate::pyjson;
use crate::pyfmt;
use crate::template;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// 說明文字，缺 `--target` 時印在 stderr。
const DOC: &str = include_str!("usage.txt");

/// init 寫進專案的版本戳（內容＝plugin.json 的 version）。
pub const VERSION_FILE: &str = ".claude/teams/VERSION";
/// 這次用的佔位符值（`--upgrade` 沒給 `--config` 時沿用）。
pub const RECORD_FILE: &str = ".claude/teams/workflow.config";
/// `NOTIFY_HOW` 留空時寫進 rules.md 的說法。
pub const NOTIFY_NONE: &str = "在對話中提問即可，不另外通知";

/// 停下來：在 stderr 印「awf init：<訊息>」，exit 1。
#[derive(Debug)]
pub struct Stop(pub String);

pub(crate) type R<T> = Result<T, Stop>;

pub(crate) fn die<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop(msg.into()))
}

/// 環境出錯（讀不了、寫不進、形狀不對）：印一行說明，exit code 1。
pub(crate) fn crash<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop(msg.into()))
}

// ------------------------------------------------------------------ 讀寫檔

/// `p.read_text(encoding="utf-8")`（含換行轉換）。
pub(crate) fn read_text(p: &Path) -> Result<String, ReadErr> {
    pyutil::read_text(&p.to_string_lossy())
}

/// `p.read_text(encoding="utf-8-sig")`：開頭的 BOM 去掉。
pub(crate) fn read_text_sig(p: &Path) -> Result<String, ReadErr> {
    let t = read_text(p)?;
    Ok(match t.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => t,
    })
}

/// `open(p, encoding="utf-8", newline="")`：不轉換換行。
pub(crate) fn read_raw(p: &Path) -> Result<String, ReadErr> {
    let name = p.to_string_lossy();
    let bytes = pyutil::read_bytes(&name)?;
    String::from_utf8(bytes).map_err(|e| ReadErr::Decode(format!("'utf-8' codec can't decode bytes in position {}", e.utf8_error().valid_up_to())))
}

/// `p.write_text(text, encoding="utf-8")`：文字模式，Windows 上換行會變成 CRLF。
pub(crate) fn write_text_native(p: &Path, text: &str) -> R<()> {
    let text = if cfg!(windows) { text.replace('\n', "\r\n") } else { text.to_string() };
    writer::write_text(p, &text)
}

// ------------------------------------------------------------------ 佔位符的值

/// `KEY=value` 的有序對照（照檔案裡的順序；重複的鍵留在第一次出現的位置、值取最後一個）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kv(Vec<(String, String)>);

impl Kv {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, k: &str, v: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v.to_string(),
            None => self.0.push((k.to_string(), v.to_string())),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

fn kv_line() -> Regex {
    Regex::new("^[A-Z_]+=").expect("固定的正規式")
}

/// 讀 KEY=value（不執行設定檔內容）；值兩端的雙引號去掉。保留檔案裡的順序。
pub fn parse_kv(text: &str) -> Kv {
    let rx = kv_line();
    let mut out = Kv::default();
    for line in pyutil::splitlines(text) {
        let line = line.trim_end_matches('\r');
        if !rx.is_match(line) {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap_or((line, ""));
        let v = v.strip_prefix('"').unwrap_or(v);
        let v = v.strip_suffix('"').unwrap_or(v);
        out.set(k, v);
    }
    out
}

fn read_kv(p: &Path, shown: &str) -> R<Kv> {
    match read_text_sig(p) {
        Ok(t) => Ok(parse_kv(&t)),
        Err(e) => crash(format!("讀不了設定檔 {shown}：{}", e.msg())),
    }
}

/// workflow.config.example 裡這個鍵上一行的註解（互動詢問時當提示）。
fn hint_for(example_text: &str, key: &str) -> String {
    let mut prev = "";
    for line in pyutil::splitlines(example_text) {
        if line.strip_prefix(key).is_some_and(|r| r.starts_with('=')) {
            return match prev.strip_prefix('#') {
                Some(rest) => rest.trim_start_matches(pyutil::is_py_space).to_string(),
                None => prev.to_string(),
            };
        }
        prev = line;
    }
    String::new()
}

// ------------------------------------------------------------------ 環境

/// 顯示用的路徑文字（`str(Path(...))`）與實際要開的路徑（相對路徑接在目前目錄後面）。
#[derive(Debug, Clone)]
pub struct Shown {
    pub shown: String,
    pub real: PathBuf,
}

/// 行程層級的輸入：模版在哪、目前目錄、家目錄、這支執行檔。測試直接給值，不碰行程狀態。
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// 模版根目錄（plugin 目錄，或底下有 `plugin/` 的 repo 根；見 [`crate::template`]）；`--source` 優先。
    pub source: Option<PathBuf>,
    /// 相對路徑的基準，也是找 `./workflow.config` 的地方。
    pub cwd: PathBuf,
    /// `~` 展開成什麼。
    pub home: Option<String>,
    /// `--hooks project` 要複製進專案的執行檔：環境變數 `AWF_BIN` 指的檔，沒設就是這支執行檔自己（解開符號連結）。
    pub awf_bin: Option<PathBuf>,
}

impl Env {
    /// 從行程的環境收集（模版根目錄見 [`crate::template::find`]）。
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let awf_bin = var("AWF_BIN").map(PathBuf::from).or_else(|| std::env::current_exe().ok()).map(|p| p.canonicalize().unwrap_or(p));
        Self {
            source: crate::template::find(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            awf_bin,
        }
    }
}

/// 模版根目錄與解析路徑要用的東西。核心函式都收這個，不自己找模版。
#[derive(Debug, Clone)]
pub struct Site {
    /// 模版根目錄＝plugin 目錄（已解開符號連結）。
    pub here: PathBuf,
    pub cwd: PathBuf,
    pub home: Option<String>,
}

impl Site {
    pub fn scaffold(&self) -> PathBuf {
        template::scaffold(&self.here)
    }

    pub fn scaffold_teams(&self) -> PathBuf {
        template::teams(&self.here)
    }

    pub fn example(&self) -> PathBuf {
        template::config_example(&self.here)
    }

    /// 發佈用的執行檔與 `checksums.json` 所在的目錄。
    pub fn plugin_bin(&self) -> PathBuf {
        template::bin(&self.here)
    }

    /// 共用片段在模版裡的位置。
    pub fn snippets(&self) -> PathBuf {
        template::snippets(&self.here)
    }

    pub fn methodologies(&self) -> PathBuf {
        self.scaffold_teams().join("methodologies")
    }

    pub fn plugin_json(&self) -> PathBuf {
        template::plugin_json(&self.here)
    }

    pub fn wf_templates(&self) -> PathBuf {
        template::workflow_templates(&self.here)
    }

    /// 模版所在的 repo 根目錄（有 marketplace 清單時）：從 clone 下來的 repo 跑才有；只裝 plugin 時沒有。
    pub fn repo(&self) -> Option<PathBuf> {
        self.here.parent().filter(|d| d.join(".claude-plugin").join("marketplace.json").is_file()).map(Path::to_path_buf)
    }

    /// `Path(s).expanduser()`：顯示用的文字照 pathlib 的規則整理，相對路徑接在 cwd 後面。
    pub fn user_path(&self, s: &str) -> Shown {
        let expanded = match (&self.home, s) {
            (Some(h), "~") => h.clone(),
            (Some(h), _) if s.starts_with("~/") => format!("{}/{}", h.trim_end_matches('/'), s.get(2..).unwrap_or_default()),
            _ => s.to_string(),
        };
        let shown = pyutil::path_str(&expanded);
        let real = if Path::new(&shown).is_absolute() { PathBuf::from(&shown) } else { self.cwd.join(&shown) };
        Shown { shown, real }
    }
}

// ------------------------------------------------------------------ 輸出與問答

/// stdout、stderr 與問答用的輸入。
pub struct Console<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    input: &'a mut dyn BufRead,
    /// stdin 是不是終端機（`sys.stdin.isatty()`）；不是就不詢問。
    pub tty: bool,
    pending: VecDeque<String>,
}

impl<'a> Console<'a> {
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write, input: &'a mut dyn BufRead, tty: bool) -> Self {
        Self { out, err, input, tty, pending: VecDeque::new() }
    }

    /// `print(s)`
    pub fn say(&mut self, s: &str) {
        let _ = writeln!(self.out, "{s}");
    }

    /// `print(s, file=sys.stderr)`
    pub fn say_err(&mut self, s: &str) {
        let _ = writeln!(self.err, "{s}");
    }

    fn write_raw(&mut self, s: &str) {
        let _ = self.out.write_all(s.as_bytes());
    }

    /// 讀一行（去掉結尾的換行）；沒有東西可讀回 None。`\r\n` 與單獨的 `\r` 都算換行。
    fn read_line(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            let mut buf = Vec::new();
            match self.input.read_until(b'\n', &mut buf) {
                Ok(n) if n > 0 => {}
                _ => return None,
            }
            let text = pyutil::universal_newlines(&String::from_utf8_lossy(&buf));
            self.pending.extend(text.split_inclusive('\n').map(str::to_string));
        }
        let line = self.pending.pop_front()?;
        Some(line.strip_suffix('\n').map(str::to_string).unwrap_or(line))
    }

    /// `input(prompt)`；EOF 回 None。
    pub fn input(&mut self, prompt: &str) -> Option<String> {
        self.write_raw(prompt);
        let _ = self.out.flush();
        self.read_line()
    }

    /// 互動詢問一題：沒回答（Enter 或 EOF）回 None；輸入 `-` 回空字串。
    pub fn ask_opt(&mut self, prompt: &str, default: &str) -> Option<String> {
        let line = self.input(&format!("{prompt} [{default}]: ")).unwrap_or_default();
        let ans = pyutil::strip(&line);
        if ans == "-" {
            Some(String::new())
        } else if ans.is_empty() {
            None
        } else {
            Some(ans.to_string())
        }
    }

    /// 互動詢問一題：Enter＝預設值；輸入 `-` ＝留空。
    pub fn ask(&mut self, prompt: &str, default: &str) -> String {
        self.ask_opt(prompt, default).unwrap_or_else(|| default.to_string())
    }
}

/// 決定每個佔位符的值：設定檔 > 互動詢問 > workflow.config.example 的預設值。
/// `guard_later`＝後面的訪談會問守門設定：互動模式不在這裡重複問那幾個佔位符（`interview::GUARD_PLACEHOLDERS`）。
fn collect_values(con: &mut Console, site: &Site, config: Option<&Shown>, defaults: bool, guard_later: bool) -> R<Kv> {
    let example = site.example();
    let example_text = match read_text_sig(&example) {
        Ok(t) => t,
        Err(e) => return crash(format!("讀不了 {}：{}", example.display(), e.msg())),
    };
    let ex = parse_kv(&example_text);
    let cf = match config {
        Some(c) => read_kv(&c.real, &c.shown)?,
        None => Kv::default(),
    };
    let interactive = config.is_none() && !defaults && con.tty;
    if interactive {
        con.say("沒有 workflow.config，逐一詢問（Enter＝用預設值；輸入 - ＝留空）：");
    }
    let mut vals = Kv::default();
    for (k, default) in ex.iter() {
        let mut val = default.to_string();
        if config.is_some() {
            val = cf.get(k).unwrap_or(default).to_string();
        } else if interactive && k != "METHODOLOGY" && !(guard_later && interview::GUARD_PLACEHOLDERS.contains(&k)) {
            // 開發規範 profile 在後面的開發規範問答一起問；守門的兩個清單在訪談的守門主題問
            con.say(&format!("\n{}", hint_for(&example_text, k)));
            let ans = con.input(&format!("{k} [{default}]: ")).unwrap_or_default();
            if ans == "-" {
                val = String::new();
            } else if !ans.is_empty() {
                val = ans;
            }
        }
        vals.set(k, &val);
    }
    // 通知方式留空：規則 11 的句子不能缺一塊，改成「不另外通知」的寫法
    if vals.get("NOTIFY_HOW").is_some_and(|s| pyutil::strip(s).is_empty()) {
        vals.set("NOTIFY_HOW", NOTIFY_NONE);
    }
    // 文件目錄會變成 tpm-writer 的寫入範圍：只收工作區裡的相對路徑
    if let Some(bad) = vals.get("DOCS_DIRS").unwrap_or_default().split(',').map(pyutil::strip).filter(|d| !d.is_empty()).find(|d| !interview::writer_dir_ok(d)) {
        return die(format!("DOCS_DIRS 要是工作區裡的相對路徑（逗號分隔），不能是絕對路徑、含 ..，也不能在 .claude、.github、.git 底下（收到 {}）", pyfmt::repr_str(bad)));
    }
    // 主線分支與 remote：會寫進派工單、交給 git，也會內嵌進 workflow 腳本，只收單純的名字（留空＝預設）
    for (k, ok, rule) in [("BASE_BRANCH", crate::layout::branch_ok as fn(&str) -> bool, "分支名只收英數與 . _ / -，不含 remote"), ("BASE_REMOTE", crate::layout::remote_ok, "remote 名只收英數與 . _ -")] {
        let v = pyutil::strip(vals.get(k).unwrap_or_default()).to_string();
        if !v.is_empty() && !ok(&v) {
            return die(format!("{k} 不合規則（{rule}；收到 {}）", pyfmt::repr_str(&v)));
        }
    }
    Ok(vals)
}

fn plugin_version(site: &Site) -> R<String> {
    let path = site.plugin_json();
    let doc = match read_text(&path) {
        Ok(t) => pyjson::loads(&t).ok(),
        Err(ReadErr::Os(_)) => None,
        Err(ReadErr::Decode(m)) => return crash(format!("{} 不是 UTF-8：{m}", path.display())),
    };
    match doc.as_ref().and_then(|d| d.get("version")).and_then(Value::as_str) {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => die(format!("讀不到 {} 的 version", path.display())),
    }
}

// ------------------------------------------------------------------ 執行檔（hooks、merge gate）

/// `--hooks project` 要複製進專案的執行檔內容。找不到就停下（settings 要寫它的絕對路徑，不能寫一個不存在的檔）。
fn hook_bin_bytes(env: &Env) -> R<Vec<u8>> {
    let Some(src) = env.awf_bin.as_deref().filter(|p| p.is_file()) else {
        return die(match &env.awf_bin {
            Some(p) => format!("--hooks project 要把 awf 執行檔複製進專案，但找不到 {}；用環境變數 AWF_BIN 指定執行檔", p.display()),
            None => "--hooks project 要把 awf 執行檔複製進專案，但找不到這支執行檔的位置；用環境變數 AWF_BIN 指定".to_string(),
        });
    };
    match std::fs::read(src) {
        Ok(b) if !b.is_empty() => Ok(b),
        Ok(_) => die(format!("--hooks project：{} 是空檔，不是執行檔", src.display())),
        Err(e) => crash(format!("讀不了 {}：{e}", src.display())),
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// 是不是 linux x86-64 的 ELF 執行檔（只看檔頭：魔術數字、64 位元、little-endian、機器類型 62）。
pub fn is_elf_x86_64(data: &[u8]) -> bool {
    data.len() >= 20 && data.starts_with(b"\x7fELF") && data.get(4) == Some(&2) && data.get(5) == Some(&1) && data.get(18) == Some(&62) && data.get(19) == Some(&0)
}

/// merge gate 的 CI 要放進專案的兩個檔：linux-x64 的執行檔與記它 sha256 的 `checksums.json`。
#[derive(Debug)]
pub struct GateFiles {
    pub bin: Vec<u8>,
    pub checksums: String,
}

/// 找出並核對 merge gate 要用的執行檔（在寫任何檔之前做：缺了就整個停下，不留半套）。
/// 預設用模版的 `plugin/bin/awf-linux-x64` 與同目錄的 `checksums.json`；`--merge-gate-bin`（與 `--merge-gate-checksums`）可以指定別的。
fn gate_files(site: &Site, a: &Args) -> R<GateFiles> {
    let given = a.merge_gate_bin.as_deref().filter(|s| !s.is_empty());
    let given_sums = a.merge_gate_checksums.as_deref().filter(|s| !s.is_empty());
    let (bin_path, bin_shown) = match given {
        Some(g) => (site.user_path(g).real, g.to_string()),
        None => {
            let p = site.plugin_bin().join(GATE_BIN_NAME);
            (p.clone(), p.display().to_string())
        }
    };
    if !bin_path.is_file() {
        return die(match given {
            Some(g) => format!("--merge-gate-bin 指定的檔案不存在：{g}"),
            None => format!(
                "--with-merge-gate 要把 linux-x64 的執行檔放進專案（CI 從受保護的 base 取出它來跑 awf merge-gate），但模版裡沒有 {bin_shown}。\
發佈版的 plugin 才有這個檔；用自己建置的模版時，加 --merge-gate-bin <linux-x64 的 awf 執行檔> 指定"
            ),
        });
    }
    let bin = match std::fs::read(&bin_path) {
        Ok(b) => b,
        Err(e) => return crash(format!("讀不了 {bin_shown}：{e}")),
    };
    if !is_elf_x86_64(&bin) {
        return die(format!("{bin_shown} 不是 linux x86-64 的執行檔（ELF 檔頭不對）；merge gate 的 CI 在 ubuntu 的 x64 runner 上跑，別的平台的執行檔不能用"));
    }
    let sum = sha256_hex(&bin);
    let sums_path = match (given, given_sums) {
        (_, Some(s)) => Some((site.user_path(s).real, s.to_string())),
        (None, None) => {
            let p = site.plugin_bin().join("checksums.json");
            Some((p.clone(), p.display().to_string()))
        }
        (Some(_), None) => None,
    };
    let checksums = match sums_path {
        None => format!("{{\n  \"files\": {{\n    \"{GATE_BIN_NAME}\": \"{sum}\"\n  }},\n  \"source\": \"unknown\"\n}}\n"),
        Some((p, shown)) => {
            let text = match read_text_sig(&p) {
                Ok(t) => t,
                Err(ReadErr::Os(_)) if given_sums.is_none() => return die(format!("模版裡沒有 {shown}（記 {GATE_BIN_NAME} 的 sha256）；用 --merge-gate-bin 與 --merge-gate-checksums 指定")),
                Err(ReadErr::Os(_)) => return die(format!("--merge-gate-checksums 指定的檔案不存在：{shown}")),
                Err(e) => return crash(format!("讀不了 {shown}：{}", e.msg())),
            };
            let want = pyjson::loads(&text).ok().and_then(|d| d.get("files").and_then(|f| f.get(GATE_BIN_NAME)).and_then(Value::as_str).map(str::to_string));
            match want {
                Some(w) if w == sum => text,
                Some(w) => return die(format!("{shown} 記的 {GATE_BIN_NAME} sha256（{w}）和執行檔 {bin_shown} 的（{sum}）不符，不放進專案")),
                None => return die(format!("{shown} 讀不到 files.{GATE_BIN_NAME}（格式：{{\"files\": {{\"{GATE_BIN_NAME}\": \"<sha256>\"}}, \"source\": …}}）")),
            }
        }
    };
    Ok(GateFiles { bin, checksums })
}

/// 專案的 `.gitignore` 有沒有忽略這個相對路徑所在的目錄（只看最常見的寫法；看不出來就當沒有，多提示一次無妨）。
fn gitignore_covers(target: &Path, rel_dir: &str) -> bool {
    let Ok(text) = read_text_sig(&target.join(".gitignore")) else { return false };
    let mut hit = false;
    for line in pyutil::splitlines(&text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (neg, pat) = match line.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let pat = pat.trim_start_matches("**/").trim_start_matches('/');
        let pat = pat.trim_end_matches("/**").trim_end_matches("/*").trim_end_matches('/');
        if !pat.is_empty() && (rel_dir == pat || rel_dir.starts_with(&format!("{pat}/"))) {
            hit = !neg;
        }
    }
    hit
}

// ------------------------------------------------------------------ 內嵌（build）

/// init 最後的內嵌步驟：處理專案的覆寫檔（`.claude/workflows/*.js`；沒有就只做檢查），回（exit code, stdout, stderr）。
/// 片段先找專案自己的 `.claude/teams/tools/snippets/`（舊專案還有時），再找模版的 `template_snippets`。
pub fn run_build(target: &Path, template_snippets: &Path) -> (i32, String, String) {
    crate::build::run_captured(&[], &crate::build::Paths::project_with(target, Some(template_snippets.to_path_buf())))
}

// ------------------------------------------------------------------ 主流程

/// settings.json 的 hooks 裡有沒有我們的註冊：（0.x 的 `awf_hook.py`, 1.0 的 `awf hook <種類>`）。
fn awf_hooks_in(settings: &Path) -> (bool, bool) {
    let Ok(text) = read_text_sig(settings) else { return (false, false) };
    let Ok(Value::Object(doc)) = pyjson::loads(&text) else { return (false, false) };
    let hooks = doc.get("hooks").cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    merge::hook_forms(&crate::recipe::dumps(&hooks, None))
}

/// 升級時把 settings.json 的 `permissions.allow` 裡 0.x 範本放的項目換成 1.0 的寫法（見 [`merge::migrate_allow`]）。
/// 檔案不是合法 JSON 物件就不動。回傳換了哪些。
fn migrate_settings_allow(target: &Path) -> R<Vec<(String, Option<String>)>> {
    let path = target.join(".claude").join("settings.json");
    let Ok(text) = read_text_sig(&path) else { return Ok(Vec::new()) };
    let Ok(Value::Object(mut doc)) = pyjson::loads(&text) else { return Ok(Vec::new()) };
    // 指的檔還在專案裡（例如改過而保留的 check_workflows.mjs）：那一項不動
    let keep = |old: &str| old.split_whitespace().any(|w| w.trim_end_matches(":*)").starts_with(".claude/teams/tools/") && w.ends_with(".mjs:*)") && target.join(w.trim_end_matches(":*)")).is_file());
    let changed = merge::migrate_allow(&mut doc, &keep);
    if !changed.is_empty() {
        writer::write_text(&path, &format!("{}\n", crate::recipe::dumps(&Value::Object(doc), Some(2))))?;
    }
    Ok(changed)
}

fn main_session_rx() -> Regex {
    Regex::new(&format!(r#"("main_session_(?:git|github)_rules"[{WS_CLASS}]*:[{WS_CLASS}]*)true"#)).expect("固定的正規式")
}

fn list(con: &mut Console, target: &Path, paths: &[PathBuf]) {
    for p in paths {
        con.say(&format!("  - {}", writer::posix(p.strip_prefix(target).unwrap_or(p))));
    }
}

fn go(argv: &[String], env: &Env, con: &mut Console) -> R<i32> {
    let a = match args::parse(argv) {
        Ok(a) => a,
        Err(args::ArgStop::Help) => {
            con.say(args::USAGE);
            con.say("");
            con.say(DOC.trim_end_matches('\n'));
            return Ok(0);
        }
        Err(args::ArgStop::Error(m)) => {
            con.say_err(args::USAGE);
            con.say_err(&format!("awf init: error: {m}"));
            return Ok(2);
        }
    };
    let positional = a.positional.as_deref().filter(|s| !s.is_empty());
    let target_opt = a.target.as_deref().filter(|s| !s.is_empty());
    if let (Some(_), Some(p)) = (target_opt, positional) {
        return die(format!("多給了一個目錄：{p}"));
    }
    let Some(target_arg) = target_opt.or(positional) else {
        con.say_err(DOC);
        return die("缺 --target");
    };
    if a.hooks != "plugin" && a.hooks != "project" {
        return die(format!("--hooks 只能是 plugin 或 project（收到 {}）", a.hooks));
    }
    let mut site = Site { here: PathBuf::new(), cwd: env.cwd.clone(), home: env.home.clone() };
    let source = match a.source.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(site.user_path(s).real),
        None => env.source.clone().map(|p| if p.is_absolute() { p } else { env.cwd.join(p) }),
    };
    let Some(source) = source else {
        return die("找不到模版（plugin 目錄，底下要有 scaffold/ 與 workflow-templates/）；用 --source <plugin 目錄> 或環境變數 AWF_TEMPLATE_DIR 指定");
    };
    site.here = template::normalize(Path::new(&pyutil::resolve(&source.to_string_lossy())));
    if !site.scaffold().is_dir() {
        return die(format!("找不到 {}", site.scaffold().display()));
    }
    if !site.example().is_file() {
        return die(format!("找不到 {}", site.example().display()));
    }

    if a.python.as_deref().is_some_and(|s| !s.is_empty()) {
        con.say("注意：--python 是 0.x 的選項；1.0 不再需要 Python，已忽略。");
    }

    let target_user = site.user_path(target_arg);
    if a.json && !a.detect {
        return die("--json 要搭配 --detect");
    }
    if a.detect {
        // 只印偵測結果：不建目錄、不寫任何檔
        let root = PathBuf::from(pyutil::resolve(&target_user.real.to_string_lossy()));
        let topics = detect::detect(&root, cfg!(windows));
        if a.json {
            con.say(&crate::recipe::dumps(&detect::to_json(&root, &topics), Some(2)));
        } else {
            con.write_raw(&detect::text_of(&root, &topics));
        }
        return Ok(0);
    }
    if a.reconfigure {
        if a.upgrade || a.force || a.overwrite_user_files || a.with_merge_gate || a.with_workflows || a.with_mcp {
            return die("--reconfigure 只改設定（methodology.json、guard.config.json），不能和 --upgrade、--force、--overwrite-user-files、--with-* 一起用");
        }
        if !target_user.real.join(".claude").join("teams").is_dir() {
            return die(format!("{} 還沒 init 過（沒有 .claude/teams/），不能 --reconfigure；先跑一次 init", target_user.shown));
        }
        let target = PathBuf::from(pyutil::resolve(&target_user.real.to_string_lossy()));
        return reconfigure(con, &site, &target, &a);
    }
    // 既有專案已經有模版產生的 merge gate CI：升級時一起換成執行檔的版本（舊的那份跑的是會被移除的 merge_gate.py）
    let gate_yml = target_user.real.join(".github").join("workflows").join("agent-merge-gate.yml");
    let gate_implied = !a.with_merge_gate && (a.force || a.upgrade) && read_text(&gate_yml).is_ok_and(|t| writer::ours().is_match(&t));
    let with_merge_gate = a.with_merge_gate || gate_implied;
    if !with_merge_gate && (a.merge_gate_bin.is_some() || a.merge_gate_checksums.is_some()) {
        return die("--merge-gate-bin／--merge-gate-checksums 要搭配 --with-merge-gate");
    }
    if a.merge_gate_checksums.is_some() && a.merge_gate_bin.is_none() {
        return die("--merge-gate-checksums 要搭配 --merge-gate-bin（沒指定執行檔時用模版自己的 checksums.json）");
    }
    // 要放進專案的執行檔先找齊、核對好，缺了就在動任何檔之前停下
    let gate = if with_merge_gate {
        match gate_files(&site, &a) {
            Ok(g) => Some(g),
            Err(Stop(m)) if gate_implied => {
                return die(format!("這個專案已經有 merge gate 的 CI（.github/workflows/agent-merge-gate.yml），升級要一起換成 1.0 的版本（等同加了 --with-merge-gate）。{m}"));
            }
            Err(e) => return Err(e),
        }
    } else {
        None
    };
    // 指令列明確要 --hooks project：執行檔先找好（缺了就在動任何檔之前停下）。答案檔或問答才決定的，等答案出來再找
    let hook_bin_early = if a.hooks == "project" { Some(hook_bin_bytes(env)?) } else { None };

    if let Err(e) = std::fs::create_dir_all(&target_user.real) {
        return crash(format!("建不了目錄 {}：{e}", target_user.shown));
    }
    let target = PathBuf::from(pyutil::resolve(&target_user.real.to_string_lossy()));
    let target = target.as_path();
    let force = a.force || a.upgrade;
    let new_ver = plugin_version(&site)?;
    let vfile = target.join(VERSION_FILE);
    let old_ver = if vfile.is_file() {
        match read_text(&vfile) {
            Ok(t) => pyutil::strip(&t).to_string(),
            Err(e) => return crash(format!("讀不了 {}：{}", vfile.display(), e.msg())),
        }
    } else {
        String::new()
    };
    if a.upgrade {
        if !target.join(".claude").join("teams").is_dir() {
            return die(format!("{} 還沒 init 過（沒有 .claude/teams/），不能 --upgrade；先不加 --upgrade 跑一次 init", target.display()));
        }
        con.say(&format!("== 升級 agent-workflow：{} → {new_ver} ==", if old_ver.is_empty() { "未記錄（0.2.0 以前的版本）" } else { &old_ver }));
    }

    let record_path = target.join(RECORD_FILE);
    let record = Shown { shown: record_path.to_string_lossy().into_owned(), real: record_path.clone() };
    let config_arg = a.config.as_deref().filter(|s| !s.is_empty());
    let mut config: Option<Shown> = None;
    if let Some(c) = config_arg {
        let p = site.user_path(c);
        if !p.real.is_file() {
            return die(format!("找不到設定檔 {c}"));
        }
        config = Some(p);
    } else if force && record.real.is_file() {
        // --force 和 --upgrade 一樣沿用 init 記下的值，不用範例預設值重算佔位符
        config = Some(record.clone());
    } else {
        for c in [site.cwd.join("workflow.config"), site.here.join("workflow.config")] {
            if c.is_file() {
                config = Some(Shown { shown: c.to_string_lossy().into_owned(), real: c });
                break;
            }
        }
    }
    if force {
        let shown = match &config {
            Some(c) => c.shown.clone(),
            None => format!("找不到 init 時的設定（{RECORD_FILE}），用 workflow.config.example 的預設值；不對時加 --config 重跑"),
        };
        con.say(&format!("佔位符的值：{shown}"));
    }

    let mut answers: Option<Shown> = None;
    if let Some(ans) = a.answers.as_deref().filter(|s| !s.is_empty()) {
        let p = site.user_path(ans);
        if !p.real.is_file() {
            return die(format!("找不到答案檔 {ans}"));
        }
        answers = Some(p);
    }

    let defaults = a.defaults || a.upgrade; // 升級不詢問
    let mut vals = collect_values(con, &site, config.as_ref(), defaults, answers.is_none())?;
    let interactive = config.is_none() && !defaults && con.tty;
    // 這次的答案會不會寫成 methodology.json（已經有的專案保留它自己的，--overwrite-user-files 才換）
    let meth = target.join(".claude").join("teams").join("methodology.json");
    let applies = !meth.exists() || a.overwrite_user_files;
    let asking = interactive && answers.is_none();
    // 訪談：先偵測專案的現況，問答時當預設值；--defaults、--answers 不採用，只列在對照表
    let topics = if applies || asking { detect::detect(target, cfg!(windows)) } else { Vec::new() };
    let mut seed = Map::new();
    if asking {
        let sug = detect::suggested(&topics);
        for k in ["code_style", "commit", "commit_regex", "doc_format"] {
            if let Some(v) = sug.get(k) {
                seed.insert(k.to_string(), v.clone());
            }
        }
        for t in topics.iter().filter(|t| t.id == "conventions") {
            for f in &t.detected {
                con.say(&format!("  偵測到 {}：{}（{}）", f.key, interview::brief(&f.value), f.basis));
            }
        }
    }
    let (name, mut ans) = profile::collect_answers(con, &site, &vals, answers.as_ref(), interactive, defaults, &seed)?;
    let main_free = profile::ask_main_session(con, answers.as_ref(), interactive)?;
    if asking {
        if main_free {
            ans.insert("main_session_push_merge".to_string(), Value::Bool(true));
        }
        if name != "default" {
            ans.insert("methodology".to_string(), Value::String(name.clone()));
        }
        let base = profile::load_profile(&site, &name)?;
        let Some(base) = base.as_object() else { return crash("開發規範 profile 要是 JSON 物件") };
        interview::ask_more(con, &mut ans, base, &topics, &vals, a.hooks_given, None)?;
    }
    let hooks = match interview::hooks_answer(&ans)? {
        Some(h) if !a.hooks_given => h,
        // 升級而且沒有指定（指令列、答案檔、問答都沒有）：專案的 settings.json 已經有 1.0 寫法的註冊，就照 project 模式升級
        //（補上這一版多的註冊、更新執行檔複本），不然它會停在舊的那幾筆。只有 0.x 寫法的照舊：等使用者明確加 --hooks project。
        None if a.upgrade && !a.hooks_given && awf_hooks_in(&target.join(".claude").join("settings.json")).1 => "project".to_string(),
        _ => a.hooks.clone(),
    };
    let methodology = profile::finish_methodology(&site, &mut vals, &name, &ans)?;
    let overlay = interview::guard_overlay(&ans, false)?;
    if applies {
        for line in interview::table(&topics, &ans, &methodology, &overlay, main_free, &hooks) {
            con.say(&line);
        }
        if asking {
            let ok = con.ask("照這張表寫入（y／n）", "y");
            if !pyutil::strip(&ok).to_lowercase().starts_with('y') {
                con.say("沒有寫入任何設定。");
                return Ok(1);
            }
        }
    }
    let hook_bin = match hook_bin_early {
        Some(b) => Some(b),
        None if hooks == "project" => Some(hook_bin_bytes(env)?),
        None => None,
    };
    let hook_bin_path = target.join(hook_bin_rel());
    let (hook_cmd, hook_shell) = writer::hook_command(&hook_bin_path.to_string_lossy());
    vals.set("AWF_HOOK_CMD", &hook_cmd);
    vals.set("AWF_HOOK_SHELL", &hook_shell);
    let runs = vals.get("RUNS_DIR").filter(|s| !s.is_empty()).unwrap_or("_tpm/runs").trim_matches('/').to_string();
    let briefs = vals.get("BRIEFS_DIR").unwrap_or_default().trim_matches('/').to_string();

    let mut w = Writer::new(target, &vals, force, a.overwrite_user_files);
    for it in writer::scaffold_plan(&site.scaffold(), target, &runs, &hooks, a.with_mcp, with_merge_gate) {
        w.put(&it.src, &it.dst, it.text_render)?;
    }

    let mut wf_written: Vec<PathBuf> = Vec::new();
    if a.with_workflows {
        for it in writer::workflow_plan(&site.wf_templates(), target) {
            if it.dst.is_file() && !a.overwrite_user_files {
                let body = match pyutil::read_text_lossy(&it.dst.to_string_lossy()) {
                    Ok(t) => t,
                    Err(e) => return crash(format!("讀不了 {}：{}", it.dst.display(), e.msg())),
                };
                if writer::wf_override().is_match(&body) {
                    w.user_kept.push(it.dst.clone()); // 已經是覆寫檔（使用者可能改過）：不覆蓋
                    continue;
                }
            }
            let before = w.written.len();
            w.put(&it.src, &it.dst, it.text_render)?;
            if w.written.len() > before {
                wf_written.push(it.dst.clone());
            }
        }
    }

    if let Some(bytes) = &hook_bin {
        w.put_bytes(&hook_bin_path, bytes, true)?;
    }
    if let Some(g) = &gate {
        w.put_bytes(&target.join(GATE_BIN_REL), &g.bin, true)?;
        w.put_bytes(&target.join(GATE_SUMS_REL), g.checksums.as_bytes(), false)?;
    }
    if gate_implied {
        w.notes.push("專案已經有 merge gate 的 CI：這次一起更新（等同加了 --with-merge-gate）。".to_string());
    }
    for n in std::mem::take(&mut w.notes) {
        con.say(&n);
    }

    if !briefs.is_empty() {
        if let Err(e) = std::fs::create_dir_all(target.join(&briefs)) {
            return crash(format!("建不了目錄 {}：{e}", target.join(&briefs).display()));
        }
    }

    // 版本戳與這次的佔位符值：模版檔都是這版時才寫 VERSION（有略過的舊模版檔時不謊報版本）
    let templates_current = force || a.overwrite_user_files || w.skipped.is_empty();
    if templates_current {
        writer::write_text(&vfile, &format!("{new_ver}\n"))?;
    } else if old_ver.is_empty() {
        con.say(&format!("注意：有已存在的模版檔沒有更新，沒寫 {VERSION_FILE}；用 --upgrade 更新到 {new_ver}"));
    }
    // 紀錄檔只在「明確給了 --config」「值就是從紀錄檔來的」或「還沒有紀錄檔」時寫；
    // 值來自其他地方（./workflow.config、範例預設值）時不覆寫既有的紀錄，避免把使用者 init 時的值換掉
    let record_ok = config_arg.is_some() || config.as_ref().is_some_and(|c| c.shown == record.shown) || !record.real.is_file();
    if (templates_current || !record.real.is_file()) && record_ok {
        let mut text = String::from("# awf init 這次用的佔位符值（awf init --upgrade／--force 沒給 --config 時沿用）。不要放金鑰或密碼。\n");
        for (k, v) in vals.iter().filter(|(k, _)| !writer::COMPUTED.contains(k)) {
            text.push_str(&format!("{k}={v}\n"));
        }
        writer::write_text(&record.real, &text)?;
    }

    // 主 session 的習慣：會直接 push main、合併 PR 時放寬主 session 的 git／GitHub 限制（只改這次寫出的 guard.config.json）
    let gcfg = target.join(".claude").join("teams").join("guard.config.json");
    if main_free {
        if w.written.contains(&gcfg) {
            let t = match read_text(&gcfg) {
                Ok(t) => t,
                Err(e) => return crash(format!("讀不了 {}：{}", gcfg.display(), e.msg())),
            };
            writer::write_text(&gcfg, &main_session_rx().replace_all(&t, "${1}false"))?;
            con.say("主 session 的 git／GitHub 限制已放寬（main_session_git_rules、main_session_github_rules = false）。注意：之後啟用 auto_merge 時，主 session 仍不能合併、核准 PR（信任模型要求）。");
        } else {
            con.say(&format!("注意：{} 已存在、沒有改；要放寬主 session 的限制，自己把 main_session_git_rules、main_session_github_rules 改成 false。", w.rel(&gcfg)));
        }
    }

    // 答案檔的 guard／auto_merge：合併進這次寫出的 guard.config.json（答案優先）
    if !overlay.is_empty() {
        if w.written.contains(&gcfg) {
            let doc = match read_text_sig(&gcfg) {
                Ok(t) => pyjson::loads(&t).ok(),
                Err(e) => return crash(format!("讀不了 {}：{}", gcfg.display(), e.msg())),
            };
            let Some(Value::Object(doc)) = doc else { return crash(format!("{} 不是合法的 JSON 物件", gcfg.display())) };
            writer::write_text(&gcfg, &format!("{}\n", crate::recipe::dumps(&Value::Object(merge::deep_merge(&doc, &overlay)), Some(2))))?;
            if overlay.get("auto_merge").and_then(|x| x.get("enabled")) == Some(&Value::Bool(true)) {
                con.say("自動合併已啟用（auto_merge.enabled = true）。白名單（auto_merge.repos）沒填之前不會合併任何 repo；伺服器端的 ruleset、gate App、CODEOWNERS 要照 docs/自動合併信任模型.md 的啟用前檢查清單設好。");
            }
        } else {
            con.say(&format!("注意：{} 已存在、沒有改；答案檔的 guard／auto_merge 沒有套用（用 awf init --reconfigure 改）。", w.rel(&gcfg)));
        }
    }

    // 開發規範 profile：寫成專案的 .claude/teams/methodology.json（已存在時保留專案自己的；--overwrite-user-files 才整份換掉）
    let mut exit_code = 0;
    if meth.exists() && !a.overwrite_user_files {
        w.user_kept.push(meth.clone());
        if force || a.adopt_new_fields {
            exit_code = profile::profile_upgrade(con, &site, &meth, a.adopt_new_fields)?;
        }
    } else {
        write_text_native(&meth, &format!("{}\n", crate::recipe::dumps(&Value::Object(methodology.clone()), Some(2))))?;
        w.written.push(meth.clone());
        // 這次採用的答案存起來，之後 awf init --reconfigure 拿它當預設
        save_answers(con, target, &ans)?;
    }
    if !w.user_kept.contains(&meth) {
        con.say(&format!("開發規範 profile：{}（{}）", pyfmt::str_opt(methodology.get("name")), w.rel(&meth)));
        con.say(&format!("這次的答案存在 {}；之後要改設定：awf init --target <專案> --reconfigure（只改有變的設定）。", interview::ANSWERS_FILE));
    }

    // 0.x 複製進專案的工具副本：升級時指紋相符的移除，改過的保留（在算舊版殘留之前做，移除的就不會再被列一次）
    let st_path = target.join(".claude").join("settings.json");
    let (old_hooks, new_hooks) = if st_path.is_file() { awf_hooks_in(&st_path) } else { (false, false) };
    let tools_dir = target.join(".claude").join("teams").join("tools");
    let mut tidy = ToolsTidy::default();
    let mut allow_changed: Vec<(String, Option<String>)> = Vec::new();
    if force {
        let index = legacy_tools::load_index(&site.here);
        // settings 還註冊著 0.x 的 awf_hook.py（這次沒有換成執行檔）：.claude/hooks/ 的腳本還在用，不能移除
        tidy = tidy_old_tools(target, index.as_ref(), &vals, old_hooks);
        allow_changed = migrate_settings_allow(target)?;
    }
    let old_tools_left: Vec<PathBuf> = if tools_dir.is_dir() { writer::walk_files(&tools_dir).into_iter().filter(|p| !writer::skip_file(p)).collect() } else { Vec::new() };

    // 舊版殘留：專案裡模版目錄下、這版模版已不再提供的檔案（不刪，列出來讓人決定）。tools/ 由上面那一段處理
    let mut stale: Vec<PathBuf> = Vec::new();
    for sub in ["schemas", "golden-set", "methodologies"] {
        let d = target.join(".claude").join("teams").join(sub);
        if d.is_dir() {
            stale.extend(writer::walk_files(&d).into_iter().filter(|p| !writer::skip_file(p) && !w.seen.contains(p)));
        }
    }

    con.say(&format!("{} {} 個模版檔到 {}", if force { "更新" } else { "寫入" }, w.written.len(), target.display()));
    if !w.preserved.is_empty() {
        con.say(&format!("保留使用者設定（模版部分更新）{} 個：", w.preserved.len()));
        for (p, how) in &w.preserved {
            con.say(&format!("  - {}：{how}", w.rel(p)));
        }
    }
    if !w.merged.is_empty() {
        con.say(&format!("合併進 {} 個既有的使用者檔案（原內容保留）：", w.merged.len()));
        for p in &w.merged {
            let how = if p.file_name().is_some_and(|n| n == "CLAUDE.md") { "結尾追加一段 @ 引用" } else { "深度合併，使用者的值優先" };
            con.say(&format!("  - {}（{how}）", w.rel(p)));
        }
    }
    if !w.user_kept.is_empty() {
        con.say(&format!("保留 {} 個使用者的檔案，沒有改動（--force／--upgrade 也不覆蓋；要整份換成模版版本加 --overwrite-user-files）：", w.user_kept.len()));
        list(con, target, &w.user_kept);
    }
    if !w.skipped.is_empty() {
        con.say(&format!("略過 {} 個已存在的模版檔（用 --upgrade 更新）：", w.skipped.len()));
        list(con, target, &w.skipped);
    }
    if st_path.is_file() {
        if hooks == "plugin" && (old_hooks || new_hooks) {
            con.say("注意：.claude/settings.json 裡有 awf 的 hooks（先前用 --hooks project init 過），但這次是 --hooks plugin。plugin 也會註冊同一組 hooks，請擇一：刪掉 settings.json 裡 awf 的 hooks，或改回 --hooks project。");
        }
        if hooks == "plugin" && old_hooks {
            con.say("  其中有 0.x 的寫法（python … awf_hook.py）：1.0 的 hook 是執行檔。要留在專案就加 --hooks project 重跑升級（會換成 awf hook <種類>）；在那之前 .claude/hooks/ 的舊腳本先留著沒有移除。");
        }
        if hooks == "project" && !new_hooks {
            con.say("注意：.claude/settings.json 沒有 awf hook 的 hooks（可能不是合法 JSON），請照 .claude/settings.README.md 手動註冊。");
        }
        if hooks == "project" && old_hooks {
            con.say("注意：.claude/settings.json 還有 0.x 的 hook 註冊（python … awf_hook.py）沒有換掉，請手動刪掉那幾筆；.claude/hooks/ 的舊腳本先留著沒有移除。");
        }
    }
    if !tidy.removed.is_empty() {
        con.say(&format!("移除 {} 個 0.x 複製進專案、沒改過的工具檔（1.0 的工具都在 awf 執行檔裡，不用放在專案）。", tidy.removed.len()));
    }
    if !tidy.modified.is_empty() {
        con.say(&format!("保留 {} 個改過的舊工具檔（和任何一版原樣都對不上；1.0 不再使用它們，確認不需要後手動刪除）：", tidy.modified.len()));
        for p in &tidy.modified {
            con.say(&format!("  - {p}"));
        }
    }
    if !tidy.unknown.is_empty() {
        con.say(&format!(".claude/teams/tools/ 底下不是模版放的檔 {} 個，沒有動：", tidy.unknown.len()));
        for p in &tidy.unknown {
            con.say(&format!("  - {p}"));
        }
    }
    if !tidy.failed.is_empty() {
        con.say(&format!("警告：有 {} 個舊工具檔移除不了：{}", tidy.failed.len(), tidy.failed.join("、")));
    }
    if tidy.no_index {
        con.say(&format!("注意：讀不到 {}（各版工具的指紋），.claude/teams/tools/ 的舊工具沒有整理；1.0 不再使用它們。", site.here.join(legacy_tools::FILE).display()));
    }
    if !force && !old_tools_left.is_empty() {
        con.say(&format!("注意：.claude/teams/tools/ 還有 {} 個 0.x 複製進來的工具檔。1.0 的工具都在 awf 執行檔裡，不再使用它們；跑 awf init --upgrade 會移除沒改過的、保留改過的。", old_tools_left.len()));
    }
    if !allow_changed.is_empty() {
        con.say(&format!(".claude/settings.json 的權限：{} 筆 0.x 的寫法換成 1.0 的（你自己加的權限沒有動）：", allow_changed.len()));
        for (old, new) in &allow_changed {
            con.say(&format!("  - {old} → {}", new.as_deref().unwrap_or("（拿掉：這個檔不再放在專案）")));
        }
    }
    if !stale.is_empty() {
        con.say(&format!("這版模版已不再提供、專案裡還留著的檔案 {} 個（沒有刪；確認沒在用後手動刪除）：", stale.len()));
        list(con, target, &stale);
    }
    if !w.leftovers.is_empty() {
        con.say("警告：還有沒替換的佔位符（workflow.config 缺這些值）：");
        let uniq: std::collections::BTreeSet<&String> = w.leftovers.iter().collect();
        for x in uniq {
            con.say(&format!("  - {x}"));
        }
    }

    // 舊版複製進專案的 workflow 副本：要在內嵌之前整理（內嵌會把新版的片段放進去，之後就認不出是不是原樣）
    if force && !a.with_workflows {
        let t = tidy_old_workflows(target, &site.wf_templates(), &vals);
        if !t.removed.is_empty() {
            con.say(&format!("移除 {} 支舊版複製進來、沒改過的 workflow 副本（0.5 起由 awf 依 plugin 的範本產生，不用放在專案）：{}", t.removed.len(), t.removed.join("、")));
        }
        if !t.overrides.is_empty() {
            con.say(&format!("保留 {} 支 workflow 覆寫檔（有 awf-override 標記，awf 會用它們）：{}", t.overrides.len(), t.overrides.join("、")));
        }
        if !t.kept.is_empty() {
            con.say(&format!("保留 {} 支和任何一版範本都對不上的 workflow（可能改過）：{}。awf 不會採用它們；要繼續用就在檔案結尾加一行「// awf-override: <版本>」，不用就刪掉。", t.kept.len(), t.kept.join("、")));
        }
    }

    // ------------------------------------------------------------ 內嵌 schema 與自我檢查
    {
        let (code, out, err) = run_build(target, &site.snippets());
        con.write_raw(&out);
        let _ = con.err.write_all(err.as_bytes());
        if code != 0 {
            con.say("警告：內嵌（awf build）失敗，修正後在專案根目錄手動跑 awf build");
            if a.upgrade && exit_code == 0 {
                exit_code = 1;
            }
        }
    }

    // 覆寫檔的標記要在內嵌之後加（內嵌會重寫內嵌的部分）；awf 只採用有這個標記的專案 workflow
    for dst in &wf_written {
        let body = match read_text(dst) {
            Ok(t) => t,
            Err(e) => return crash(format!("讀不了 {}：{}", dst.display(), e.msg())),
        };
        if !writer::wf_override().is_match(&body) {
            let nl = if body.ends_with('\n') { "" } else { "\n" };
            writer::write_text(dst, &format!("{body}{nl}// awf-override: {new_ver}\n"))?;
        }
    }
    if !wf_written.is_empty() {
        con.say(&format!("workflow 覆寫檔 {} 支放在 .claude/workflows/（結尾有 awf-override 標記；awf 會用它們取代 plugin 的範本，plugin 更新後要自己對照更新。不需要改 workflow 的話可以整個刪掉）。", wf_written.len()));
    }

    if a.upgrade {
        con.say(&format!("\n升級完成：{} → {new_ver}。用 git diff 檢查變更；驗證指令見下面第 5 步。", if old_ver.is_empty() { "未記錄" } else { &old_ver }));
        con.say("記得也更新 plugin（agents、skills、hooks、awf 執行檔）：claude plugin update agent-workflow@agentworkflow，再 /reload-plugins。");
    }

    if hook_bin.is_some() && !gitignore_covers(target, ".claude/hooks/bin") {
        con.say(&format!(
            "\n提示：{} 是這台機器這個平台的執行檔，不建議進版控（.gitignore 沒有忽略 .claude/hooks/bin/；要不要加由你決定，init 不改 .gitignore）。\n  settings.json 寫的是它的絕對路徑：換機器、換平台或搬專案位置後，各自跑一次 awf init --target <專案> --upgrade --hooks project。",
            hook_bin_rel()
        ));
    }

    next_steps(con, &site, target, &hooks);
    Ok(exit_code)
}

/// 把採用的答案寫到 `.claude/teams/init-answers.json`（像機敏值的字串不存）。
fn save_answers(con: &mut Console, target: &Path, ans: &Map<String, Value>) -> R<()> {
    let (text, redacted) = interview::answers_text(ans);
    writer::write_text(&target.join(interview::ANSWERS_FILE), &text)?;
    if !redacted.is_empty() {
        con.say(&format!("注意：答案裡這些值看起來像機敏值，沒有存進 {}：{}", interview::ANSWERS_FILE, redacted.join("、")));
    }
    Ok(())
}

fn load_object(path: &Path, what: &str) -> R<Map<String, Value>> {
    let text = match read_text_sig(path) {
        Ok(t) => t,
        Err(e) => return die(format!("讀不了 {what}（{}）：{}", path.display(), e.msg())),
    };
    match pyjson::loads(&text) {
        Ok(Value::Object(o)) => Ok(o),
        _ => die(format!("{what}（{}）不是合法的 JSON 物件", path.display())),
    }
}

/// `--reconfigure`：拿存下來的答案當預設重新設定（互動再問一輪，或用 `--answers` 給要改的鍵），
/// 只把「答案有變的設定」寫回 methodology.json 與 guard.config.json。其他鍵（包含使用者手改過的）不動。
fn reconfigure(con: &mut Console, site: &Site, target: &Path, a: &Args) -> R<i32> {
    let teams = target.join(".claude").join("teams");
    let (meth_path, guard_path, record_path) = (teams.join("methodology.json"), teams.join("guard.config.json"), target.join(RECORD_FILE));
    let answers_path = target.join(interview::ANSWERS_FILE);
    let cur = load_object(&meth_path, "methodology.json")?;
    let saved = match interview::load_answers(&answers_path)? {
        Some(s) => s,
        None => {
            con.say(&format!("注意：沒有 {}（這個專案 init 的時候還沒有答案檔）：以「沒有回答」當基準，只會寫入這次回答到的設定。", interview::ANSWERS_FILE));
            Map::new()
        }
    };
    let vals = if record_path.is_file() { read_kv(&record_path, RECORD_FILE)? } else { Kv::default() };
    let topics = detect::detect(target, cfg!(windows));
    // `--reconfigure <主題>`：只重問一個主題
    let only = a.reconfigure_topic.as_deref();
    let only_topic = match only {
        None => None,
        Some(id) => {
            let singles = interview::single_topics();
            let Some(t) = topics.iter().find(|t| t.id == id) else {
                return die(format!("沒有 {} 這個主題。可以單獨重問的主題：{}（全部主題見 awf init --detect）", pyfmt::repr_str(id), singles.join("、")));
            };
            if !singles.contains(&id) {
                return die(format!("主題 {id}（{}）的題目互相有關，不能單獨重問：不帶主題跑 awf init --reconfigure 問一整輪，或用 --answers 只寫要改的鍵。可以單獨重問的主題：{}", t.title, singles.join("、")));
            }
            Some(t)
        }
    };

    let mut new_ans = saved.clone();
    let asked = a.answers.as_deref().filter(|s| !s.is_empty()).is_none() && !a.defaults && con.tty;
    if let Some(file) = a.answers.as_deref().filter(|s| !s.is_empty()) {
        let p = site.user_path(file);
        if !p.real.is_file() {
            return die(format!("找不到答案檔 {file}"));
        }
        // 答案檔只要寫要改的鍵；值寫 null＝拿掉那個答案（回到 profile 的預設）
        let given = load_object(&p.real, "答案檔")?;
        if let Some(t) = only_topic {
            let other: Vec<&str> = given.keys().map(String::as_str).filter(|k| !t.answer_keys.contains(k)).collect();
            if !other.is_empty() {
                return die(format!("--reconfigure {} 只改這個主題（答案檔的鍵：{}），答案檔卻還有：{}。拿掉它們，或不帶主題重跑", t.id, t.answer_keys.join("、"), other.join("、")));
            }
        }
        for (k, v) in given {
            if v.is_null() {
                new_ans.shift_remove(&k);
            } else {
                new_ans.insert(k, v);
            }
        }
    } else if let (true, Some(t)) = (asked, only_topic) {
        con.say(&format!("重新設定「{}」（{}）：其他主題不問、不動。每題的預設值是上次的答案（沒有的用偵測到的現況或 profile），Enter＝不改。", t.title, t.id));
        if t.id == "main_session" {
            let was = saved.get("main_session_push_merge").is_some_and(interview::yes);
            let m = con.ask("你既有的 skill 或習慣會不會在主 session 直接 push main、合併 PR（y／n）", if was { "y" } else { "n" });
            let free = pyutil::strip(&m).to_lowercase().starts_with('y') || pyutil::strip(&m) == "會";
            if free || saved.contains_key("main_session_push_merge") {
                new_ans.insert("main_session_push_merge".to_string(), Value::Bool(free));
            }
        } else {
            let name = saved.get("methodology").and_then(Value::as_str).or(vals.get("METHODOLOGY")).filter(|s| !s.is_empty()).unwrap_or("default").to_string();
            let base = profile::load_profile(site, &name)?;
            let Some(base) = base.as_object() else { return crash("開發規範 profile 要是 JSON 物件") };
            interview::ask_more(con, &mut new_ans, base, &topics, &vals, false, Some(t.id))?;
        }
    } else if asked {
        con.say("重新設定：每題的預設值是上次的答案（沒有的用偵測到的現況），Enter＝不改。");
        let name0 = saved.get("methodology").and_then(Value::as_str).or(vals.get("METHODOLOGY")).filter(|s| !s.is_empty()).unwrap_or("default").to_string();
        let mut seed = saved.clone();
        let sug = detect::suggested(&topics);
        for k in ["code_style", "commit", "commit_regex", "doc_format"] {
            if let (false, Some(v)) = (seed.contains_key(k), sug.get(k)) {
                seed.insert(k.to_string(), v.clone());
            }
        }
        seed.shift_remove("methodology");
        let (name, ans) = profile::ask_methodology(con, site, &name0, vals.get("BRANCH_PREFIX").unwrap_or_default(), &seed)?;
        new_ans = ans;
        new_ans.insert("methodology".to_string(), Value::String(name.clone()));
        let was = saved.get("main_session_push_merge").is_some_and(interview::yes);
        let m = con.ask("你既有的 skill 或習慣會不會在主 session 直接 push main、合併 PR（y／n）", if was { "y" } else { "n" });
        let free = pyutil::strip(&m).to_lowercase().starts_with('y') || pyutil::strip(&m) == "會";
        if free || saved.contains_key("main_session_push_merge") {
            new_ans.insert("main_session_push_merge".to_string(), Value::Bool(free));
        }
        let base = profile::load_profile(site, &name)?;
        let Some(base) = base.as_object() else { return crash("開發規範 profile 要是 JSON 物件") };
        interview::ask_more(con, &mut new_ans, base, &topics, &vals, false, None)?;
    }

    let name_of = |ans: &Map<String, Value>| ans.get("methodology").and_then(Value::as_str).or(vals.get("METHODOLOGY")).filter(|s| !s.is_empty()).unwrap_or("default").to_string();
    let (mut vals_old, mut vals_new) = (vals.clone(), vals.clone());
    let p_old = profile::finish_methodology(site, &mut vals_old, &name_of(&saved), &saved)?;
    let p_new = profile::finish_methodology(site, &mut vals_new, &name_of(&new_ans), &new_ans)?;
    let mut changes: Vec<interview::Change> = Vec::new();
    interview::diff_leaves(&p_old, &p_new, &[], &mut changes);
    let (g_old, g_new) = (interview::guard_overlay(&saved, true)?, interview::guard_overlay(&new_ans, true)?);
    let mut gchanges: Vec<interview::Change> = Vec::new();
    interview::diff_leaves(&g_old, &g_new, &[], &mut gchanges);
    // 答案拿掉的守門設定不知道原本的預設是什麼：不動 guard.config.json，列出來交人
    let dropped: Vec<String> = gchanges.iter().filter(|c| c.1.is_none()).map(|c| c.0.join(".")).collect();
    gchanges.retain(|c| c.1.is_some());

    let mut next = cur.clone();
    let mut lines: Vec<String> = Vec::new();
    for c in &changes {
        let path = c.0.join(".");
        let before = interview::get_path(&cur, &c.0).cloned();
        if before.as_ref() == c.1.as_ref() {
            continue; // 已經是這個值
        }
        let expected = interview::get_path(&p_old, &c.0);
        let hand = before.as_ref() != expected;
        if !interview::apply_change(&mut next, c) {
            lines.push(format!("  - methodology.json：{path} 沒有改（上層的鍵不是物件）"));
            continue;
        }
        lines.push(format!(
            "  - methodology.json：{path}：{} → {}{}",
            before.as_ref().map(|v| merge::short(v, 60)).unwrap_or_else(|| "（沒有）".to_string()),
            c.1.as_ref().map(|v| merge::short(v, 60)).unwrap_or_else(|| "（拿掉）".to_string()),
            if hand { "（這個鍵你手改過，這次的回答取代它）" } else { "" }
        ));
    }
    let guard_cur = if guard_path.is_file() { Some(load_object(&guard_path, "guard.config.json")?) } else { None };
    let mut guard_next = guard_cur.clone().unwrap_or_default();
    let mut guard_changed = false;
    for c in &gchanges {
        let before = interview::get_path(&guard_next, &c.0).cloned();
        if before.as_ref() == c.1.as_ref() {
            continue;
        }
        if guard_cur.is_none() || !interview::apply_change(&mut guard_next, c) {
            lines.push(format!("  - guard.config.json：{} 沒有改（檔案不存在或上層的鍵不是物件）", c.0.join(".")));
            continue;
        }
        guard_changed = true;
        lines.push(format!(
            "  - guard.config.json：{}：{} → {}",
            c.0.join("."),
            before.as_ref().map(|v| merge::short(v, 60)).unwrap_or_else(|| "（沒有）".to_string()),
            c.1.as_ref().map(|v| merge::short(v, 60)).unwrap_or_default()
        ));
    }
    // 分支前綴改了：紀錄檔與 guard.config 的允許前綴一起跟上（只加不刪）
    let (bp_old, bp_new) = (vals_old.get("BRANCH_PREFIX").unwrap_or_default().to_string(), vals_new.get("BRANCH_PREFIX").unwrap_or_default().to_string());
    let prefix_changed = bp_old != bp_new && !bp_new.is_empty();
    if prefix_changed && guard_cur.is_some() {
        let list = guard_next.entry("allowed_branch_prefixes").or_insert_with(|| Value::Array(Vec::new()));
        if let Some(l) = list.as_array_mut() {
            if !l.iter().any(|x| x.as_str() == Some(bp_new.as_str())) {
                l.insert(0, Value::String(bp_new.clone()));
                guard_changed = true;
                lines.push(format!("  - guard.config.json：allowed_branch_prefixes 加上 {bp_new}"));
            }
        }
    }
    // 主線分支改了：紀錄檔的同名佔位符一起跟上（之後 --force 重寫 guard.config.json 才不會改回去）
    let mut record_notes: Vec<String> = Vec::new();
    for (gk, pk) in crate::layout::RECORD_KEYS {
        if let Some(v) = guard_next.get(*gk).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            if guard_changed && vals_new.get(pk).is_some_and(|old| old != v) && gchanges.iter().any(|c| c.0.first().map(String::as_str) == Some(*gk)) {
                vals_new.set(pk, v);
                record_notes.push(format!("{pk}={v}"));
            }
        }
    }
    let meth_changed = next != cur;
    if meth_changed {
        let errs = profile::check_methodology(site, &next)?;
        if !errs.is_empty() {
            return die(format!("照這些回答改完的 methodology.json 不符合 schema，沒有寫入：{}", errs.iter().take(5).map(String::as_str).collect::<Vec<_>>().join("；")));
        }
    }
    let hooks_new = interview::hooks_answer(&new_ans)?;
    let main_free = new_ans.get("main_session_push_merge").is_some_and(interview::yes);
    for line in interview::table(&topics, &new_ans, &next, &g_new, main_free, hooks_new.as_deref().unwrap_or("plugin")) {
        con.say(&line);
    }
    if lines.is_empty() {
        con.say("和現在的設定比，沒有要改的。");
    } else {
        con.say(&format!("會改 {} 項（其他鍵不動）：", lines.len()));
        for l in &lines {
            con.say(l);
        }
        if asked {
            let ok = con.ask("照這張表寫入（y／n）", "y");
            if !pyutil::strip(&ok).to_lowercase().starts_with('y') {
                con.say("沒有寫入任何設定。");
                return Ok(1);
            }
        }
    }
    if meth_changed {
        write_text_native(&meth_path, &format!("{}\n", crate::recipe::dumps(&Value::Object(next), Some(2))))?;
    }
    if guard_changed {
        writer::write_text(&guard_path, &format!("{}\n", crate::recipe::dumps(&Value::Object(guard_next), Some(2))))?;
    }
    if (prefix_changed || !record_notes.is_empty()) && record_path.is_file() {
        let mut text = String::from("# awf init 這次用的佔位符值（awf init --upgrade／--force 沒給 --config 時沿用）。不要放金鑰或密碼。\n");
        for (k, v) in vals_new.iter().filter(|(k, _)| !writer::COMPUTED.contains(k)) {
            text.push_str(&format!("{k}={v}\n"));
        }
        writer::write_text(&record_path, &text)?;
        if prefix_changed {
            con.say(&format!("分支前綴改成 {bp_new}：{RECORD_FILE} 已更新；rules.md 等模版檔裡的前綴要跑 awf init --target <專案> --upgrade 才會換。"));
        }
        if !record_notes.is_empty() {
            con.say(&format!("{RECORD_FILE} 已更新：{}（workflow 腳本下一次 awf next 產生時就用新值）。", record_notes.join("、")));
        }
    }
    if !dropped.is_empty() {
        con.say(&format!("注意：這次的答案拿掉了這些守門設定，guard.config.json 沒有改回預設（不知道你原本的值），要改請自己改：{}", dropped.join("、")));
    }
    if hooks_new != interview::hooks_answer(&saved)? {
        con.say(&format!("hook 註冊方式改成 {}：跑 awf init --target <專案> --upgrade --hooks {} 才會重新註冊。", hooks_new.as_deref().unwrap_or("plugin"), hooks_new.as_deref().unwrap_or("plugin")));
    }
    save_answers(con, target, &new_ans)?;
    con.say(&format!("答案已存到 {}。", interview::ANSWERS_FILE));
    if meth_changed {
        // 有覆寫檔的專案要重新內嵌 profile；沒有覆寫檔時這一步只做檢查
        let (code, out, err) = run_build(target, &site.snippets());
        con.write_raw(&out);
        let _ = con.err.write_all(err.as_bytes());
        if code != 0 {
            con.say("警告：內嵌（awf build）失敗，修正後在專案根目錄手動跑 awf build");
            return Ok(1);
        }
    }
    Ok(0)
}

/// 結尾的「下一步」說明。
fn next_steps(con: &mut Console, site: &Site, target: &Path, hooks: &str) {
    let here = &site.here;
    let teams = target.join(".claude").join("teams");
    let plugin = here;
    // 從 clone 下來的 repo 跑：文件就在旁邊的 docs/；只裝 plugin 時沒有文件，改指 repo 裡的位置
    let repo = site.repo();
    let doc = |name: &str| match &repo {
        Some(r) => r.join("docs").join(name).display().to_string(),
        None => format!("AgentWorkFlow repo 的 docs/{name}"),
    };
    let trust = doc("自動合併信任模型.md");
    con.say("\n下一步：");
    if hooks == "plugin" {
        let install = match &repo {
            Some(r) => format!("       /plugin marketplace add {}\n       /plugin install agent-workflow@agentworkflow", r.display()),
            None => format!("       這支 awf 來自 {}；還沒在 Claude Code 啟用時：/plugin install agent-workflow@agentworkflow", here.display()),
        };
        con.say(&format!(
            "  1. 在 Claude Code 安裝 plugin（hooks、agents、skills、commands、awf 執行檔都由 plugin 提供）：\n{install}\n  2. 確認 hooks 只註冊一次：{} 不應有 \"hooks\"；在 Claude Code 跑 /hooks，awf hook guard-shell 只出現一次（來源是 plugin）。",
            target.join(".claude").join("settings.json").display()
        ));
    } else {
        con.say(&format!(
            "  1. awf 執行檔已複製到 {}，hooks 註冊在 .claude/settings.json（用它的絕對路徑跑 awf hook <種類>；跑不起來時擋下）。\n     若也要用 plugin 的 agents／skills／commands：安裝 plugin 後，到 /plugin 停用它的 hooks，或改回 --hooks plugin 重新 init（加 --force）。\n  2. 確認 hooks 只註冊一次：在 Claude Code 跑 /hooks，awf hook guard-shell 只出現一次（來源是專案 settings）。",
            target.join(hook_bin_rel()).display()
        ));
    }
    let claude_doc = if target.join(writer::CLAUDE_IMPORT).exists() { writer::CLAUDE_IMPORT } else { "CLAUDE.md" };
    con.say(&format!(
        "  3. 填寫 {} 的「## 專案規則」段與 {claude_doc} 的「專案特定」段。\n  4. 檢查 {}（上游、fork、禁止路徑、分支前綴）與\n     {}（開發規範與慣例；改了下一段 workflow 就生效，不用重新 init）。\n  5. 驗證（在 {} 執行；awf 由 plugin 的 bin/ 提供，裝好 plugin 後在 Claude Code 裡就能直接叫）：\n       awf doctor\n       awf render --all --out _tpm/wf-check --plugin-dir {}\n       awf check-agents --plugin-dir {}",
        teams.join("rules.md").display(),
        teams.join("guard.config.json").display(),
        teams.join("methodology.json").display(),
        target.display(),
        plugin.display(),
        plugin.display()
    ));
    let gate = target.join(".github").join("workflows").join("agent-merge-gate.yml");
    if gate.exists() {
        con.say(&format!(
            "  6. merge gate CI 已放在 {}，它用的執行檔與 sha256 在 {}（三個檔要一起進受保護的 base 分支）。\n     在 GitHub 設好 gate App、ruleset 綁定來源之前，它會讓 PR 的 check 失敗；啟用前檢查清單見 {}。",
            gate.display(),
            target.join(".claude").join("teams").join("bin").display(),
            trust
        ));
    } else {
        con.say(&format!(
            "  6. merge gate CI（auto_merge 用）預設沒有放。要啟用自動合併時，先完成 {}\n     的啟用前檢查清單，再加 --with-merge-gate 重跑 init。",
            trust
        ));
    }
    con.say(&format!("  套用到既有專案時保留了什麼、怎麼移除模版：見 {}〈套用到既有專案〉。", doc("自訂與擴充.md")));
}

/// 指令本體：`args` 是 `awf init` 後面的參數；輸出寫到 `out`／`err`，問答從 `input` 讀（`tty` 為真才詢問）。回 exit code。
pub fn run(args: &[String], env: &Env, out: &mut dyn Write, err: &mut dyn Write, input: &mut dyn BufRead, tty: bool) -> i32 {
    let mut con = Console::new(out, err, input, tty);
    let code = match go(args, env, &mut con) {
        Ok(code) => code,
        Err(Stop(msg)) => {
            con.say_err(&format!("awf init：{msg}"));
            1
        }
    };
    let _ = con.out.flush();
    let _ = con.err.flush();
    code
}

/// `awf init …`
pub fn main(args: &[String]) -> i32 {
    use std::io::IsTerminal;
    let stdin = std::io::stdin();
    let tty = stdin.is_terminal();
    let (mut out, mut err, mut input) = (std::io::stdout(), std::io::stderr(), stdin.lock());
    run(args, &Env::from_process(), &mut out, &mut err, &mut input, tty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_keeps_order_and_strips_quotes() {
        let kv = parse_kv("# 註解\nA=1\nB=\"x y\"\nlower=no\nC=\"\nA=2\r\nD=a=b\n");
        let got: Vec<(&str, &str)> = kv.iter().collect();
        assert_eq!(got, vec![("A", "2"), ("B", "x y"), ("C", ""), ("D", "a=b")]);
    }

    #[test]
    fn hint_is_previous_comment() {
        let ex = "# 第一行\n\n#   專案名。預設：x\nPROJECT_NAME=x\nNAKED=1\n";
        assert_eq!(hint_for(ex, "PROJECT_NAME"), "專案名。預設：x");
        assert_eq!(hint_for(ex, "NAKED"), "PROJECT_NAME=x");
        assert_eq!(hint_for(ex, "NOPE"), "");
    }

    #[test]
    fn user_path_follows_pathlib() {
        let site = Site { here: PathBuf::from("/src"), cwd: PathBuf::from("/work"), home: Some("/h/demo".into()) };
        assert_eq!(site.user_path("./a//b/").shown, "a/b");
        assert_eq!(site.user_path("a").real, PathBuf::from("/work/a"));
        assert_eq!(site.user_path("~/x").shown, "/h/demo/x");
        assert_eq!(site.user_path("/abs/y").real, PathBuf::from("/abs/y"));
    }
}
