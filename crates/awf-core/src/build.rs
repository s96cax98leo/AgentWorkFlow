//! 把 schema、methodology 與共用片段內嵌進 workflow 腳本（Workflow 腳本不能 import 檔案）。
//! 原本是 0.x 的 Python 工具 `build.py`；輸出與訊息是對跑基準的一部分（`tests/parity/build.expected.json`）。
//!
//! 標記寫法（腳本內）：
//!   - `/*@schema:<payload 名>*/ null /*@end*/`：換成信封＋payload 的完整 schema（`envelope` 代表 payload 不限形狀）。
//!   - `/*@methodology*/ null /*@end*/`：換成專案 `.claude/teams/methodology.json` 的內容；沒有檔案時保留 null。
//!   - `/*@snippet:<名>*/ … /*@end*/`：中間換成 `snippets/<名>.js` 的內容（標記保留，可重跑）。
//!
//! 片段的規則：只定義函式（不能有頂層 await）、不能含 `/*@` 標記或佔位符（雙大括號）；開頭可寫
//! `// @requires: <名>[, <名>]`，引用它的 workflow 必須也引用這些片段。違反時 exit 1。
//! profile 的 extensions（使用者自己的 agent）必須登記在 guard.config.json 的 extension_agents，否則不內嵌（exit 1）。
//!
//! 和 0.x 的 Python 版不同的地方：0.x 的 Python 版依工具檔的位置決定處理哪個目錄，這裡由呼叫端給 [`Paths`]；
//! 0.x 的 Python 版直接當掉（traceback）的情況（未知的 schema 名、檔案不是 UTF-8 等），這裡在 stderr 印一行說明，exit code 同樣是 1。

use crate::kb::pyish::{self, WS_CLASS};
use crate::merge_gate::{pyjson, pyutil};
use crate::pyfmt;
use crate::schema::Schemas;
use crate::template;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 一次 build 要用到的三個目錄。
#[derive(Debug, Clone)]
pub struct Paths {
    /// 要處理的 workflow 腳本所在的目錄（只看這一層的 `*.js`）。
    pub wf_dir: PathBuf,
    /// `.claude/teams`：底下有 `schemas/`、`methodology.json`、`guard.config.json`。
    pub teams_dir: PathBuf,
    /// 共用片段的目錄（模版：`workflow-templates/snippets`；專案：`.claude/teams/tools/snippets`，0.x 的專案才有）。
    pub snippet_dir: PathBuf,
    /// `snippet_dir` 裡沒有某個片段時，再到這裡找（專案模式：模版／plugin 的片段目錄）。
    pub snippet_fallback: Option<PathBuf>,
}

impl Paths {
    /// 模版自己：`root` 是模版根目錄（plugin 目錄，見 [`crate::template`]）。處理 `workflow-templates/`，
    /// 片段用 `workflow-templates/snippets/`，schema 用 `scaffold/.claude/teams/` 的。
    pub fn template(root: &Path) -> Self {
        Self { wf_dir: template::workflow_templates(root), snippet_dir: template::snippets(root), teams_dir: template::teams(root), snippet_fallback: None }
    }

    /// 已 init 的專案：處理 `.claude/workflows/`（使用者留在專案的覆寫檔，沒有就什麼都不做）。
    /// 片段只找專案自己的 `.claude/teams/tools/snippets/`（0.x 的專案才有）。
    pub fn project(root: &Path) -> Self {
        Self::project_with(root, None)
    }

    /// 已 init 的專案，片段依序找：專案自己的 `.claude/teams/tools/snippets/`（舊專案還有時）→ `template_snippets`（模版／plugin 的）。
    /// 1.0 的 init 不再把片段複製進專案，所以覆寫檔的片段通常來自後者。
    pub fn project_with(root: &Path, template_snippets: Option<PathBuf>) -> Self {
        let teams = root.join(".claude").join("teams");
        Self { wf_dir: root.join(".claude").join("workflows"), snippet_dir: teams.join("tools").join("snippets"), teams_dir: teams, snippet_fallback: template_snippets }
    }

    /// 片段要找的目錄，依先後。
    pub fn snippet_dirs(&self) -> Vec<&Path> {
        std::iter::once(self.snippet_dir.as_path()).chain(self.snippet_fallback.as_deref()).collect()
    }

    /// 從 `start` 判斷：`start` 自己是已 init 的專案（有 `.claude/teams/`、而且不是模版）就當專案；
    /// 否則 `start` 或它上層是模版（plugin 目錄，或底下有 `plugin/` 的 repo 根；[`template::find_up`]）時處理模版；都不是就當專案。
    /// 當專案時，片段找不到會再找 `template_snippets`。
    pub fn detect_with(start: &Path, template_snippets: Option<PathBuf>) -> Self {
        let mut paths = Self::detect(start);
        if paths.snippet_fallback.is_none() && paths.wf_dir.ends_with(Path::new(".claude").join("workflows")) {
            paths.snippet_fallback = template_snippets;
        }
        paths
    }

    /// 同 [`Paths::detect_with`]，不給模版的片段目錄（專案模式只找專案自己的）。
    pub fn detect(start: &Path) -> Self {
        let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        let is_template = |d: &Path| template::is_root(d) || template::is_root(&d.join("plugin"));
        if !is_template(&start) && start.join(".claude").join("teams").is_dir() {
            return Self::project(&start);
        }
        match template::find_up(&start) {
            Some(root) => Self::template(&root),
            None => Self::project(&start),
        }
    }
}

/// 內嵌失敗的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    /// 片段不合規則（0.x 的 Python 版的 SnippetError）：印在 stdout，前面加 workflow 檔名。
    Snippet(String),
    /// 其他（讀不到檔、未知的 schema）：0.x 的 Python 版在這裡直接當掉，這裡印在 stderr。
    Other(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Snippet(m) | BuildError::Other(m) => f.write_str(m),
        }
    }
}

/// 片段內容的快取（片段名 → 內容），同一次 build 共用。
pub type SnippetCache = HashMap<String, String>;

fn rx(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).expect("固定的正規式"))
}

fn mark() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, r"(?s)/\*@schema:([a-z-]+)\*/.*?/\*@end\*/")
}

fn meth_mark() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, r"(?s)/\*@methodology\*/.*?/\*@end\*/")
}

fn snip_mark() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, r"(?s)/\*@snippet:([a-z0-9-]+)\*/.*?/\*@end\*/")
}

fn requires() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Python 的 `\s` 比 Rust 多 U+001C～U+001F，換成同範圍的字元類別
    R.get_or_init(|| Regex::new(&format!(r"(?m)^//[{WS_CLASS}]*@requires:[{WS_CLASS}]*(.+)$")).expect("固定的正規式"))
}

fn await_word() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Python 的 `\bawait\b`：`\w` 是字母、數字（含各種數字字元）與底線
    rx(&R, r"(?:^|[^\p{L}\p{N}_])await(?:$|[^\p{L}\p{N}_])")
}

/// 文字模式讀檔（`read_text(encoding="utf-8")`）：嚴格 UTF-8，`\r\n`、`\r` 轉成 `\n`。
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} 不是 UTF-8", path.display()))?;
    Ok(pyutil::universal_newlines(&text))
}

/// `json.dumps(v, ensure_ascii=False, separators=(",", ":"))`
pub fn dumps_compact(v: &Value) -> String {
    let mut out = String::new();
    dump(v, &mut out);
    out
}

fn dump(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(_) => out.push_str(&pyfmt::repr(v)),
        Value::String(s) => out.push_str(&pyish::json_str(s)),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&pyish::json_str(k));
                out.push(':');
                dump(x, out);
            }
            out.push('}');
        }
    }
}

/// 讀 `<snippet_dir>/<name>.js`；不存在或內容不合規則就回 [`BuildError::Snippet`]。
pub fn load_snippet(name: &str, snippet_dir: &Path, cache: &mut SnippetCache) -> Result<String, BuildError> {
    load_snippet_from(name, &[snippet_dir], cache)
}

/// 同 [`load_snippet`]，依序在 `dirs` 找（第一個有這個片段的目錄為準）；都沒有時訊息列第一個目錄的路徑。
pub fn load_snippet_from(name: &str, dirs: &[&Path], cache: &mut SnippetCache) -> Result<String, BuildError> {
    if let Some(t) = cache.get(name) {
        return Ok(t.clone());
    }
    let file = format!("{name}.js");
    let first = dirs.first().map(|d| d.join(&file)).unwrap_or_else(|| PathBuf::from(&file));
    let Some(p) = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file()) else {
        return Err(BuildError::Snippet(format!("片段檔不存在：{}", first.display())));
    };
    let raw = read_text(&p).map_err(BuildError::Other)?;
    let text = raw.replace("\r\n", "\n");
    let text = text.trim_matches('\n');
    if text.contains("/*@") {
        return Err(BuildError::Snippet(format!("片段 {name} 不能含 /*@ 標記（會和內嵌標記混淆）")));
    }
    if text.contains(&"{".repeat(2)) {
        return Err(BuildError::Snippet(format!(
            "片段 {name} 不能含佔位符（雙大括號）：片段不經 init 換值，workflow 用到的值一律用參數傳進去"
        )));
    }
    let top_await = text.split('\n').find(|ln| {
        let first = ln.chars().next();
        !matches!(first, None | Some(' ' | '\t' | '/' | '*' | '}' | ')' | ']')) && await_word().is_match(ln)
    });
    if let Some(ln) = top_await {
        return Err(BuildError::Snippet(format!(
            "片段 {name} 有頂層 await（片段只定義函式，由 workflow 呼叫）：{}",
            pyish::strip(ln)
        )));
    }
    cache.insert(name.to_string(), text.to_string());
    Ok(text.to_string())
}

/// 片段開頭 `// @requires: <名>[, <名>]` 列的片段名。
pub fn snippet_requires(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in requires().captures_iter(text) {
        for n in c.get(1).map(|m| m.as_str()).unwrap_or_default().split(',') {
            let n = pyish::strip(n);
            if !n.is_empty() {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// 專案的開發規範 profile；沒有檔案回 `Ok(None)`。不是合法 JSON 或不合 schema 回 `Err(訊息)`（不內嵌壞掉的 profile）。
pub fn load_methodology(teams_dir: &Path) -> Result<Option<Value>, String> {
    let path = teams_dir.join("methodology.json");
    if !path.is_file() {
        return Ok(None);
    }
    let text = read_text(&path)?;
    let data = pyjson::loads(&text).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let errs = Schemas::new(teams_dir).check_doc(&data, "methodology")?;
    if !errs.is_empty() {
        let head: Vec<&str> = errs.iter().take(5).map(String::as_str).collect();
        return Err(format!("{} 不符合 schemas/methodology.schema.json：{}", path.display(), head.join("；")));
    }
    Ok(Some(data))
}

/// profile 的 extensions 裡、沒有登記在 guard.config.json `extension_agents` 的 agent（`名稱（agent）`）。
/// 沒有守門設定檔時不檢查。沒登記的擴充 agent 不套任何角色規則，所以一律擋下、不內嵌。
pub fn unregistered_extensions(methodology: Option<&Value>, teams_dir: &Path) -> Result<Vec<String>, String> {
    let exts: Vec<&Value> = methodology.and_then(|m| m.get("extensions")).and_then(Value::as_array)
        .map(|a| a.iter().filter(|e| e.is_object()).collect()).unwrap_or_default();
    // roles（把某一步換成自己的 agent）的值一樣要登記：（步驟, agent）
    let swapped: Vec<(&String, &str)> = methodology.and_then(|m| m.get("roles")).and_then(Value::as_object)
        .map(|r| r.iter().filter_map(|(k, v)| v.as_str().filter(|a| !a.is_empty()).map(|a| (k, a))).collect()).unwrap_or_default();
    let guard = teams_dir.join("guard.config.json");
    if (exts.is_empty() && swapped.is_empty()) || !guard.is_file() {
        return Ok(Vec::new());
    }
    let text = read_text(&guard)?;
    // 不是合法 JSON、最外層不是物件、沒有這個鍵或值是空的：都當成沒有登記任何 agent
    let registered = pyjson::loads_lenient(&text).ok().and_then(|v| v.as_object().and_then(|o| o.get("extension_agents")).cloned())
        .filter(pyfmt::truthy);
    let mut out = Vec::new();
    for e in exts {
        let agent = e.get("agent");
        let known = match (&registered, agent) {
            (None, _) => false,
            (Some(Value::Object(o)), Some(Value::String(a))) => o.contains_key(a),
            (Some(Value::Object(_)), _) => false,
            (Some(Value::Array(a)), Some(x)) => a.iter().any(|y| pyfmt::eq(x, y)),
            (Some(Value::Array(a)), None) => a.iter().any(Value::is_null),
            (Some(Value::String(s)), Some(Value::String(a))) => s.contains(a.as_str()),
            // 0.x 的 Python 版在這裡當掉（對數字或字串做不合法的 in）；這裡當成沒登記，只會更嚴
            _ => false,
        };
        if !known {
            out.push(format!("{}（{}）", pyfmt::str_opt(e.get("name")), pyfmt::str_opt(agent)));
        }
    }
    for (step, agent) in swapped {
        if !matches!(&registered, Some(Value::Object(o)) if o.contains_key(agent)) {
            out.push(format!("roles.{step}（{agent}）"));
        }
    }
    Ok(out)
}

/// 只做片段內嵌：檢查 `@requires`，再把每個片段標記之間換成片段內容。
pub fn render_snippets(text: &str, snippet_dir: &Path, cache: &mut SnippetCache) -> Result<String, BuildError> {
    render_snippets_from(text, &[snippet_dir], cache)
}

/// 同 [`render_snippets`]，片段依序在 `dirs` 找。
pub fn render_snippets_from(text: &str, dirs: &[&Path], cache: &mut SnippetCache) -> Result<String, BuildError> {
    let used: Vec<&str> = snip_mark().captures_iter(text).filter_map(|c| c.get(1)).map(|m| m.as_str()).collect();
    for name in &used {
        for req in snippet_requires(&load_snippet_from(name, dirs, cache)?) {
            if !used.iter().any(|u| *u == req) {
                return Err(BuildError::Snippet(format!("片段 {name} 需要片段 {req}，workflow 沒有引用 /*@snippet:{req}*/")));
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for c in snip_mark().captures_iter(text) {
        let (Some(all), Some(name)) = (c.get(0), c.get(1)) else { continue };
        out.push_str(text.get(last..all.start()).unwrap_or_default());
        let body = load_snippet_from(name.as_str(), dirs, cache)?;
        out.push_str(&format!("/*@snippet:{}*/\n{body}\n/*@end*/", name.as_str()));
        last = all.end();
    }
    out.push_str(text.get(last..).unwrap_or_default());
    Ok(out)
}

/// 只做 schema 內嵌：每個 `/*@schema:<名>*/ … /*@end*/` 換成信封＋payload 的完整 schema。
pub fn embed_schemas(text: &str, teams_dir: &Path) -> Result<String, BuildError> {
    let schemas = Schemas::new(teams_dir);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for c in mark().captures_iter(text) {
        let (Some(all), Some(name)) = (c.get(0), c.get(1)) else { continue };
        let name = name.as_str();
        let schema = schemas.compose(if name == "envelope" { None } else { Some(name) }).map_err(BuildError::Other)?;
        out.push_str(text.get(last..all.start()).unwrap_or_default());
        out.push_str(&format!("/*@schema:{name}*/ {} /*@end*/", dumps_compact(&schema)));
        last = all.end();
    }
    out.push_str(text.get(last..).unwrap_or_default());
    Ok(out)
}

/// 只做 methodology 內嵌：每個 `/*@methodology*/ … /*@end*/` 換成 profile 的 JSON（沒有 profile 時是 null）。
/// 狀態機的 `render` 用這個（片段與 schema 已經內嵌在範本裡）。
pub fn embed_methodology(text: &str, methodology: Option<&Value>) -> String {
    let meth = methodology.map(dumps_compact).unwrap_or_else(|| "null".to_string());
    let rep = format!("/*@methodology*/ {meth} /*@end*/");
    meth_mark().replace_all(text, regex::NoExpand(&rep)).into_owned()
}

/// 完整內嵌：片段 → schema → methodology。
pub fn render(text: &str, methodology: Option<&Value>, teams_dir: &Path, snippet_dir: &Path, cache: &mut SnippetCache) -> Result<String, BuildError> {
    render_from(text, methodology, teams_dir, &[snippet_dir], cache)
}

/// 同 [`render`]，片段依序在 `snippet_dirs` 找。
pub fn render_from(text: &str, methodology: Option<&Value>, teams_dir: &Path, snippet_dirs: &[&Path], cache: &mut SnippetCache) -> Result<String, BuildError> {
    let text = render_snippets_from(text, snippet_dirs, cache)?;
    let text = embed_schemas(&text, teams_dir)?;
    Ok(embed_methodology(&text, methodology))
}

/// `wf_dir` 這一層的 `*.js`，依檔名排序。
fn workflow_files(wf_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(wf_dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().map(|n| (n.to_string(), e.path())))
        .filter(|(n, _)| n.ends_with(".js"))
        .collect();
    out.sort();
    out
}

/// 指令本體：回（exit code、stdout、stderr）。`args` 只看有沒有 `--check`（不一致就 exit 1、不寫檔）。
pub fn run_captured(args: &[String], paths: &Paths) -> (i32, String, String) {
    let check = args.iter().any(|a| a == "--check");
    let mut out = String::new();
    let fail = |out: String, msg: String| (1, out, format!("{msg}\n"));
    let files = if paths.wf_dir.is_dir() { workflow_files(&paths.wf_dir) } else { Vec::new() };
    let methodology = match load_methodology(&paths.teams_dir) {
        Ok(m) => m,
        Err(msg) => return fail(out, msg),
    };
    let missing = match unregistered_extensions(methodology.as_ref(), &paths.teams_dir) {
        Ok(m) => m,
        Err(msg) => return fail(out, msg),
    };
    if !missing.is_empty() {
        out.push_str(&format!(
            "methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents：{}（登記要套用哪個模版角色的規則，例如 \"tpm-reviewer\"；可以用 add-agent skill（/agent-workflow:add-agent）設定）\n",
            missing.join("、")
        ));
        return (1, out, String::new());
    }
    let mut cache = SnippetCache::new();
    let mut stale: Vec<&str> = Vec::new();
    for (name, path) in &files {
        let old = match read_text(path) {
            Ok(t) => t,
            Err(msg) => return fail(out, msg),
        };
        let new = match render_from(&old, methodology.as_ref(), &paths.teams_dir, &paths.snippet_dirs(), &mut cache) {
            Ok(t) => t,
            Err(BuildError::Snippet(e)) => {
                out.push_str(&format!("{name}：{e}\n"));
                return (1, out, String::new());
            }
            Err(BuildError::Other(msg)) => return fail(out, format!("{name}：{msg}")),
        };
        if new != old {
            stale.push(name);
            if !check {
                // 原樣寫出（不把 \n 轉成 \r\n）
                if let Err(e) = std::fs::write(path, new.as_bytes()) {
                    return fail(out, format!("寫不進 {}：{e}", path.display()));
                }
            }
        }
    }
    if check && !stale.is_empty() {
        out.push_str(&format!(
            "workflow 內嵌的 schema、開發規範 profile 或片段跟 schemas/、methodology.json、snippets/ 不一致：{}（請跑 awf build）\n",
            stale.join("、")
        ));
        return (1, out, String::new());
    }
    let prof = match methodology.as_ref().filter(|m| pyfmt::truthy(m)) {
        Some(m) => format!("（開發規範 profile：{}）", pyfmt::str_opt(m.get("name"))),
        None => "（沒有 methodology.json，workflow 用中性指引）".to_string(),
    };
    let tail = if check { "，全部一致".to_string() } else { format!("，改了 {} 支", stale.len()) };
    out.push_str(&format!("{}：{} 支 workflow{prof}{tail}\n", if check { "檢查" } else { "已更新" }, files.len()));
    (0, out, String::new())
}

/// `awf build [--check]`：專案根（[`crate::project`] 的解析，找不到就用目前目錄）交給 [`Paths::detect`] 判斷處理哪個目錄。
pub fn main(args: &[String]) -> i32 {
    let start = PathBuf::from(crate::project::root_or_cwd(None));
    // 專案模式：片段先找專案自己的，再找模版／plugin 的（1.0 的專案沒有自己的片段）
    let template_snippets = template::find().map(|r| template::snippets(&r));
    let (code, out, err) = run_captured(args, &Paths::detect_with(&start, template_snippets));
    print!("{out}");
    eprint!("{err}");
    code
}
