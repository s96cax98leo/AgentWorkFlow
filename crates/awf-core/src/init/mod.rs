//! 把模版套進專案（複製 scaffold、替換佔位符、合併既有設定、升級）。由根目錄的 `init.py` 移植。
//!
//! 行為和 Python 版相同（對跑基準在 `tests/parity/init.expected.json`）：輸出、exit code、訊息文字、寫出的檔案內容逐字相同。
//! 訊息開頭的「init.py：」與說明文字裡的 `init.py`、`build.py` 也照舊（之後改成執行檔的那一批再一起換）。
//!
//! 和 Python 版不同的地方：
//! - 模版根目錄：Python 版用 init.py 自己的位置；這裡依序看 `--source`、環境變數 `AWF_TEMPLATE_DIR`、
//!   `CLAUDE_PLUGIN_ROOT`／`AWF_PLUGIN_DIR` 的上一層、執行檔往上找到含 `scaffold/` 與 `plugin/` 的目錄。
//! - 最後的內嵌（build）在程式內呼叫 [`crate::build`]，不另外跑 `build.py`（專案裡有 `tools/build.py` 才做，條件照舊）。
//! - `--hooks project` 寫進 settings 的 Python：`--python` 沒給時在 PATH 找 `python3`／`python`，問它自己的絕對路徑
//!   （Python 版用「跑 init.py 的那個 Python」）。找不到而且要用時就停下。
//! - Python 版直接當掉（traceback）的情況（檔案不是 UTF-8、設定的形狀不對、寫不進檔案），這裡在 stderr 印一行說明，exit code 同樣是 1。
//! - JSON 裡的 `NaN`、`Infinity` 當成不合法（Python 讀得進來）：對使用者的檔案只會變成「沒有合併、原樣保留」。
//! - `~使用者名` 開頭的路徑不展開（只展開 `~`）。
//!
//! 核心函式都收明確的模版根（[`Site`]）與目標路徑，不讀行程的目前目錄與環境變數；那些由 [`Env::from_process`] 收集。

mod args;
mod legacy;
mod merge;
mod profile;
mod writer;

pub use args::Args;
pub use legacy::{pristine_legacy_workflow, tidy_old_workflows, Tidy};
pub use merge::{deep_merge, merge_hooks, merge_settings, missing_fields, set_missing};
pub use profile::{apply_answers, parse_constraints};
pub use writer::{dest_for, hook_command, hook_files_plan, scaffold_plan, workflow_plan, Item, Writer};

use crate::kb::pyish::WS_CLASS;
use crate::merge_gate::pyutil::{self, ReadErr};
use crate::merge_gate::pyjson;
use crate::pyfmt;
use regex::Regex;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Python 版的說明文字（`init.py` 的 docstring），缺 `--target` 時印在 stderr。
const DOC: &str = include_str!("usage.txt");

/// init 寫進專案的版本戳（內容＝plugin.json 的 version）。
pub const VERSION_FILE: &str = ".claude/teams/VERSION";
/// 這次用的佔位符值（`--upgrade` 沒給 `--config` 時沿用）。
pub const RECORD_FILE: &str = ".claude/teams/workflow.config";

/// 停下來：在 stderr 印「init.py：<訊息>」，exit 1（Python 版的 `die`，以及它直接當掉的情況）。
#[derive(Debug)]
pub struct Stop(pub String);

pub(crate) type R<T> = Result<T, Stop>;

pub(crate) fn die<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop(msg.into()))
}

/// Python 版在這裡會直接當掉（traceback）；這裡印一行說明，exit code 一樣是 1。
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

/// 行程層級的輸入：模版在哪、目前目錄、家目錄、預設的 Python。測試直接給值，不碰行程狀態。
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// 模版根目錄（底下有 `scaffold/`、`plugin/`、`workflow.config.example`）；`--source` 優先。
    pub source: Option<PathBuf>,
    /// 相對路徑的基準，也是找 `./workflow.config` 的地方。
    pub cwd: PathBuf,
    /// `~` 展開成什麼。
    pub home: Option<String>,
    /// `--hooks project` 寫進 settings 的 Python（`--python` 沒給時用）；None 表示到 PATH 找。
    pub python: Option<String>,
}

fn is_template_root(d: &Path) -> bool {
    d.join("scaffold").is_dir() && d.join("plugin").is_dir()
}

impl Env {
    /// 從行程的環境收集：`AWF_TEMPLATE_DIR` → `CLAUDE_PLUGIN_ROOT`／`AWF_PLUGIN_DIR` 的上一層 → 執行檔往上找。
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let mut source = var("AWF_TEMPLATE_DIR").map(PathBuf::from);
        if source.is_none() {
            source = ["CLAUDE_PLUGIN_ROOT", "AWF_PLUGIN_DIR"].iter().filter_map(|k| var(k))
                .filter_map(|p| Path::new(&p).parent().map(Path::to_path_buf))
                .find(|d| is_template_root(d));
        }
        if source.is_none() {
            let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
            source = exe.as_deref().and_then(|p| p.ancestors().skip(1).find(|d| is_template_root(d)).map(Path::to_path_buf));
        }
        Self {
            source,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            python: var("AWF_PYTHON"),
        }
    }
}

/// 模版根目錄與解析路徑要用的東西。核心函式都收這個，不自己找模版。
#[derive(Debug, Clone)]
pub struct Site {
    /// 模版根目錄（已解開符號連結；Python 版的 `HERE`）。
    pub here: PathBuf,
    pub cwd: PathBuf,
    pub home: Option<String>,
}

impl Site {
    pub fn scaffold(&self) -> PathBuf {
        self.here.join("scaffold")
    }

    pub fn scaffold_teams(&self) -> PathBuf {
        self.scaffold().join(".claude").join("teams")
    }

    pub fn example(&self) -> PathBuf {
        self.here.join("workflow.config.example")
    }

    pub fn plugin_hooks(&self) -> PathBuf {
        self.here.join("plugin").join("hooks")
    }

    pub fn methodologies(&self) -> PathBuf {
        self.scaffold_teams().join("methodologies")
    }

    pub fn plugin_json(&self) -> PathBuf {
        self.here.join("plugin").join(".claude-plugin").join("plugin.json")
    }

    pub fn wf_templates(&self) -> PathBuf {
        self.here.join("plugin").join("workflow-templates")
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
fn collect_values(con: &mut Console, site: &Site, config: Option<&Shown>, defaults: bool) -> R<Kv> {
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
        } else if interactive && k != "METHODOLOGY" {
            // 開發規範 profile 在後面的開發規範問答一起問
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

// ------------------------------------------------------------------ hooks 用的 Python

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// 到 PATH 找 Python，並問它自己的絕對路徑（在 venv 裡時回它底下的基礎 Python：venv 可能被刪）。
pub fn find_python() -> Option<String> {
    let names: &[&str] = if cfg!(windows) { &["python.exe", "python3.exe"] } else { &["python3", "python"] };
    let exe = find_on_path(names)?;
    let probe = "import sys\nfrom pathlib import Path\ne = sys.executable\nb = getattr(sys, '_base_executable', None)\n\
if sys.prefix != sys.base_prefix and b and Path(b).is_file():\n    e = b\nprint(Path(e).resolve())\n";
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["-c", probe]);
    let asked = pyutil::run(&mut cmd, None, Some(std::time::Duration::from_secs(20))).ok()
        .filter(|o| o.code == 0)
        .map(|o| pyutil::strip(&pyutil::text(&o.stdout)).to_string())
        .filter(|p| Path::new(p).is_file());
    asked.or_else(|| exe.canonicalize().ok().map(|p| p.to_string_lossy().into_owned()))
}

/// 寫進 settings.json 的 Python：`--python` > [`Env::python`] > PATH 上的 Python。
/// 找不到時，`--hooks project` 停下（要寫絕對路徑）；其他情況用不到，回 `python3`。
fn python_for_hooks(site: &Site, env: &Env, over: Option<&str>, hooks: &str) -> R<String> {
    if let Some(o) = over.filter(|s| !s.is_empty()) {
        let p = site.user_path(o);
        if !p.real.is_file() {
            return die(format!("--python 指定的檔案不存在：{o}"));
        }
        return Ok(pyutil::resolve(&p.real.to_string_lossy()));
    }
    if let Some(p) = &env.python {
        return Ok(p.clone());
    }
    match find_python() {
        Some(p) => Ok(p),
        None if hooks == "project" => die("找不到 Python（--hooks project 要把 Python 的絕對路徑寫進 settings.json）；用 --python <路徑> 指定"),
        None => Ok("python3".to_string()),
    }
}

/// Windows 上 `python3` 指令跑不跑得起來（plugin 的 hooks 用它啟動）。
fn python3_works() -> bool {
    let Some(exe) = find_on_path(&["python3.exe", "python3.cmd", "python3.bat", "python3"]) else { return false };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["-c", "print('ok')"]);
    match pyutil::run(&mut cmd, None, Some(std::time::Duration::from_secs(20))) {
        Ok(o) => o.code == 0 && pyutil::strip(&pyutil::text(&o.stdout)) == "ok",
        Err(_) => false,
    }
}

// ------------------------------------------------------------------ 內嵌（build）

/// init 最後的內嵌步驟：回（exit code, stdout, stderr）；專案裡沒有 `tools/build.py` 時不做（回 None，條件同 Python 版）。
/// 之後不複製工具進專案時，這個條件要拿掉。
pub fn run_build(target: &Path) -> Option<(i32, String, String)> {
    if !target.join(".claude").join("teams").join("tools").join("build.py").is_file() {
        return None;
    }
    Some(crate::build::run_captured(&[], &crate::build::Paths::project(target)))
}

// ------------------------------------------------------------------ 主流程

fn has_awf_hooks(settings: &Path) -> bool {
    let Ok(text) = read_text_sig(settings) else { return false };
    let Ok(Value::Object(doc)) = pyjson::loads(&text) else { return false };
    let hooks = doc.get("hooks").cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    crate::recipe::dumps(&hooks, None).contains("awf_hook.py")
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
            con.say_err(&format!("init.py: error: {m}"));
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
        return die("找不到模版（要有 scaffold/ 與 plugin/）；用 --source <模版根目錄> 或環境變數 AWF_TEMPLATE_DIR 指定");
    };
    site.here = PathBuf::from(pyutil::resolve(&source.to_string_lossy()));
    if !site.scaffold().is_dir() {
        return die(format!("找不到 {}", site.scaffold().display()));
    }
    if !site.example().is_file() {
        return die(format!("找不到 {}", site.example().display()));
    }

    let target_user = site.user_path(target_arg);
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
    let mut vals = collect_values(con, &site, config.as_ref(), defaults)?;
    let interactive = config.is_none() && !defaults && con.tty;
    let methodology = profile::build_methodology(con, &site, &mut vals, answers.as_ref(), interactive, defaults)?;
    let main_free = profile::ask_main_session(con, answers.as_ref(), interactive)?;
    let python = python_for_hooks(&site, env, a.python.as_deref(), &a.hooks)?;
    let (hook_cmd, hook_shell) = writer::hook_command(&python);
    vals.set("AWF_HOOK_CMD", &hook_cmd);
    vals.set("AWF_HOOK_SHELL", &hook_shell);
    let runs = vals.get("RUNS_DIR").filter(|s| !s.is_empty()).unwrap_or("_tpm/runs").trim_matches('/').to_string();
    let briefs = vals.get("BRIEFS_DIR").unwrap_or_default().trim_matches('/').to_string();

    let mut w = Writer::new(target, &vals, force, a.overwrite_user_files);
    for it in writer::scaffold_plan(&site.scaffold(), target, &runs, &a.hooks, a.with_mcp, a.with_merge_gate) {
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

    if a.hooks == "project" {
        for it in writer::hook_files_plan(&site.plugin_hooks(), target) {
            w.put(&it.src, &it.dst, it.text_render)?;
        }
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
        let mut text = String::from("# init.py 這次用的佔位符值（init.py --upgrade／--force 沒給 --config 時沿用）。不要放金鑰或密碼。\n");
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

    // 開發規範 profile：寫成專案的 .claude/teams/methodology.json（已存在時保留專案自己的；--overwrite-user-files 才整份換掉）
    let meth = target.join(".claude").join("teams").join("methodology.json");
    let mut exit_code = 0;
    if meth.exists() && !a.overwrite_user_files {
        w.user_kept.push(meth.clone());
        if force || a.adopt_new_fields {
            exit_code = profile::profile_upgrade(con, &site, &meth, a.adopt_new_fields)?;
        }
    } else {
        write_text_native(&meth, &format!("{}\n", crate::recipe::dumps(&Value::Object(methodology.clone()), Some(2))))?;
        w.written.push(meth.clone());
    }
    if !w.user_kept.contains(&meth) {
        con.say(&format!("開發規範 profile：{}（{}）", pyfmt::str_opt(methodology.get("name")), w.rel(&meth)));
    }

    // 舊版殘留：專案裡模版目錄下、這版模版已不再提供的檔案（不刪，列出來讓人決定）
    let mut stale: Vec<PathBuf> = Vec::new();
    for sub in ["tools", "schemas", "golden-set", "methodologies"] {
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
    let st_path = target.join(".claude").join("settings.json");
    if st_path.is_file() {
        let has_awf = has_awf_hooks(&st_path);
        if a.hooks == "plugin" && has_awf {
            con.say("注意：.claude/settings.json 裡有 awf_hook.py 的 hooks（先前用 --hooks project init 過），但這次是 --hooks plugin。plugin 也會註冊同一組 hooks，請擇一：刪掉 settings.json 裡的 awf_hook.py hooks，或改回 --hooks project。");
        }
        if a.hooks == "project" && !has_awf {
            con.say("注意：.claude/settings.json 沒有 awf_hook.py 的 hooks（可能不是合法 JSON），請照 .claude/settings.README.md 手動註冊。");
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
            con.say(&format!("移除 {} 支舊版複製進來、沒改過的 workflow 副本（0.5 起由 awf.py 依 plugin 的範本產生，不用放在專案）：{}", t.removed.len(), t.removed.join("、")));
        }
        if !t.overrides.is_empty() {
            con.say(&format!("保留 {} 支 workflow 覆寫檔（有 awf-override 標記，awf.py 會用它們）：{}", t.overrides.len(), t.overrides.join("、")));
        }
        if !t.kept.is_empty() {
            con.say(&format!("保留 {} 支和任何一版範本都對不上的 workflow（可能改過）：{}。awf.py 不會採用它們；要繼續用就在檔案結尾加一行「// awf-override: <版本>」，不用就刪掉。", t.kept.len(), t.kept.join("、")));
        }
    }

    // ------------------------------------------------------------ 內嵌 schema 與自我檢查
    if let Some((code, out, err)) = run_build(target) {
        con.write_raw(&out);
        let _ = con.err.write_all(err.as_bytes());
        if code != 0 {
            con.say("警告：build.py 失敗，請手動跑 python3 .claude/teams/tools/build.py");
            if a.upgrade && exit_code == 0 {
                exit_code = 1;
            }
        }
    }

    // 覆寫檔的標記要在內嵌之後加（內嵌會重寫內嵌的部分）；awf.py 只採用有這個標記的專案 workflow
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
        con.say(&format!("workflow 覆寫檔 {} 支放在 .claude/workflows/（結尾有 awf-override 標記；awf.py 會用它們取代 plugin 的範本，plugin 更新後要自己對照更新。不需要改 workflow 的話可以整個刪掉）。", wf_written.len()));
    }

    if a.upgrade {
        con.say(&format!("\n升級完成：{} → {new_ver}。用 git diff 檢查變更；驗證指令見下面第 5 步。", if old_ver.is_empty() { "未記錄" } else { &old_ver }));
        con.say("記得也更新 plugin（agents、skills、hooks）：claude plugin update agent-workflow@agentworkflow，再 /reload-plugins。");
    }

    if a.hooks == "plugin" && cfg!(windows) && !python3_works() {
        con.say("\n警告：這台 Windows 的 python3 指令跑不起來（常見是 Microsoft Store 的空殼或只裝了 py 啟動器）。plugin 的 hooks 用 python3 啟動，跑不起來時守門會擋下所有 shell 與改檔動作。改用 --hooks project 重跑（init.py 會寫入 Python 絕對路徑），或安裝能用 python3 執行的 Python。");
    }

    next_steps(con, &site, target, &a.hooks, &python);
    Ok(exit_code)
}

/// 結尾的「下一步」說明。
fn next_steps(con: &mut Console, site: &Site, target: &Path, hooks: &str, python: &str) {
    let here = &site.here;
    let teams = target.join(".claude").join("teams");
    let plugin = here.join("plugin");
    let trust = here.join("docs").join("自動合併信任模型.md");
    con.say("\n下一步：");
    if hooks == "plugin" {
        con.say(&format!(
            "  1. 在 Claude Code 安裝 plugin（hooks、agents、skills、commands 都由 plugin 提供）：\n       /plugin marketplace add {}\n       /plugin install agent-workflow@agentworkflow\n  2. 確認 hooks 只註冊一次：{} 不應有 \"hooks\"；在 Claude Code 跑 /hooks，awf_hook.py guard-shell 只出現一次（來源是 plugin）。",
            here.display(),
            target.join(".claude").join("settings.json").display()
        ));
    } else {
        con.say(&format!(
            "  1. hooks 已複製到 {}，並註冊在 .claude/settings.json（Python：{python}）。\n     若也要用 plugin 的 agents／skills／commands：安裝 plugin 後，到 /plugin 停用它的 hooks，或改回 --hooks plugin 重新 init（加 --force）。\n  2. 確認 hooks 只註冊一次：在 Claude Code 跑 /hooks，awf_hook.py guard-shell 只出現一次（來源是專案 settings）。",
            target.join(".claude").join("hooks").display()
        ));
    }
    let claude_doc = if target.join(writer::CLAUDE_IMPORT).exists() { writer::CLAUDE_IMPORT } else { "CLAUDE.md" };
    con.say(&format!(
        "  3. 填寫 {} 的「## 專案規則」段與 {claude_doc} 的「專案特定」段。\n  4. 檢查 {}（上游、fork、禁止路徑、分支前綴）與\n     {}（開發規範與慣例；改了下一段 workflow 就生效，不用重跑 build.py）。\n  5. 驗證（在 {} 執行；Windows 把 python3 換成 py -3 或 python）：\n       uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py\n       python3 .claude/teams/tools/awf.py render --all --out _tpm/wf-check --plugin-dir {}\n       node .claude/teams/tools/check_workflows.mjs --agents-dir {} --wf-dir _tpm/wf-check\n       python3 .claude/teams/tools/check_agents.py --plugin-dir {}\n       python3 {} --project {}",
        teams.join("rules.md").display(),
        teams.join("guard.config.json").display(),
        teams.join("methodology.json").display(),
        target.display(),
        plugin.display(),
        plugin.join("agents").display(),
        plugin.display(),
        plugin.join("hooks").join("tests").join("run.py").display(),
        target.display()
    ));
    let gate = target.join(".github").join("workflows").join("agent-merge-gate.yml");
    if gate.exists() {
        con.say(&format!(
            "  6. merge gate CI 已放在 {}。在 GitHub 設好 gate App、ruleset 綁定來源之前，它會讓 PR 的 check 失敗；\n     啟用前檢查清單見 {}。",
            gate.display(),
            trust.display()
        ));
    } else {
        con.say(&format!(
            "  6. merge gate CI（auto_merge 用）預設沒有放。要啟用自動合併時，先完成 {}\n     的啟用前檢查清單，再加 --with-merge-gate 重跑 init。",
            trust.display()
        ));
    }
    con.say(&format!("  套用到既有專案時保留了什麼、怎麼移除模版：見 {}〈套用到既有專案〉。", here.join("docs").join("自訂與擴充.md").display()));
}

/// 指令本體：`args` 是 `awf init` 後面的參數；輸出寫到 `out`／`err`，問答從 `input` 讀（`tty` 為真才詢問）。回 exit code。
pub fn run(args: &[String], env: &Env, out: &mut dyn Write, err: &mut dyn Write, input: &mut dyn BufRead, tty: bool) -> i32 {
    let mut con = Console::new(out, err, input, tty);
    let code = match go(args, env, &mut con) {
        Ok(code) => code,
        Err(Stop(msg)) => {
            con.say_err(&format!("init.py：{msg}"));
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
