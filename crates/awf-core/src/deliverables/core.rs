//! 型錄與設定、文件的讀寫、逐節的來源與過期判定、盤點（status）、總目錄、檢查（check）與隱私規則。

use super::py;
use super::*;
use crate::kb::pyish::{self, WS_CLASS};
use crate::pyfmt::{self, repr, str_of, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

pub(super) static NULL: Value = Value::Null;

macro_rules! rx {
    ($name:ident, $pat:expr) => {
        pub(super) fn $name() -> &'static regex::Regex {
            static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            R.get_or_init(|| regex::Regex::new(&$pat).expect("固定的正規式"))
        }
    };
}
pub(super) use rx;

rx!(drive_re, r"^[A-Za-z]:".to_string());
rx!(dot_slash_re, r"^(?:\./)+".to_string());
rx!(id_re, r"^[a-z0-9][a-z0-9-]{0,63}\n?\z".to_string());
rx!(meta_re, format!(r"^([A-Za-z_][\p{{L}}\p{{N}}_\-]*)[{WS_CLASS}]*:[{WS_CLASS}]*(.*)$"));
rx!(mark_re, format!(r"^<!--[{WS_CLASS}]*awf:source[{WS_CLASS}]*(.*?)[{WS_CLASS}]*-->[{WS_CLASS}]*$"));
rx!(hash_drop_re, r"[\p{P}\p{S}\p{Z}\p{C}]".to_string());
rx!(scheme_re, "^(?i:[a-z\u{130}\u{131}][a-z0-9+.\\-\u{130}\u{131}]*:|#|//)".to_string());
rx!(join_re, format!(r"[{WS_CLASS}]*\n[{WS_CLASS}]*"));

// ------------------------------------------------------------------ 小工具
/// `d.get(k)`；不是物件或沒有這個鍵回 null。
pub(super) fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&NULL)
}

/// `d.get(k)` 是字串才回內容，否則空字串。
pub(super) fn sget<'a>(v: &'a Value, k: &str) -> &'a str {
    get(v, k).as_str().unwrap_or("")
}

/// `x or []` 再逐項走：陣列是元素，字串是每個字元，物件是鍵（照 Python 的迭代）。
pub(super) fn iter_any(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        _ => Vec::new(),
    }
}

/// 陣列裡的字串（`[x for x in v or [] if isinstance(x, str)]`）。
pub(super) fn strs(v: &Value) -> Vec<String> {
    iter_any(v).iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

/// `"%s" % v`
pub(super) fn fmt(v: &Value) -> String {
    str_of(v)
}

pub(super) fn posix(s: &str) -> String {
    s.replace('\\', "/")
}

pub(super) fn sha1_text(text: &str) -> String {
    Sha1::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// 檔案內容的 sha1（\r\n 先換成 \n）；讀不到回 None。
pub(super) fn sha1_file(p: &str) -> Option<String> {
    let raw = std::fs::read(p).ok()?;
    let mut data = Vec::with_capacity(raw.len());
    let mut i = 0;
    while let Some(b) = raw.get(i).copied() {
        if b == b'\r' && raw.get(i + 1) == Some(&b'\n') {
            data.push(b'\n');
            i += 2;
        } else {
            data.push(b);
            i += 1;
        }
    }
    Some(Sha1::digest(&data).iter().map(|b| format!("{b:02x}")).collect())
}

pub(super) fn is_file(p: &str) -> bool {
    std::path::Path::new(p).is_file()
}

pub(super) fn is_dir(p: &str) -> bool {
    std::path::Path::new(p).is_dir()
}

/// `p.read_text(encoding="utf-8", errors="replace")`
pub(super) fn read_text(p: &str) -> Option<String> {
    pyish::read_text(p).ok()
}

pub(super) fn load_json(p: &str) -> Option<Value> {
    pyish::json_loads(&read_text(p)?).ok()
}

/// `load_json(p) or {}`，而且只收物件。
pub(super) fn load_obj(p: &str) -> Value {
    match load_json(p) {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

/// 寫檔（建上層目錄、換行一律 \n）。寫不進去時在 stderr 說明。
pub(super) fn write_lf(p: &str, text: &str) {
    if let Some(i) = p.rfind('/') {
        let _ = std::fs::create_dir_all(p.get(..i).unwrap_or("."));
    }
    if let Err(e) = std::fs::write(p, text.as_bytes()) {
        eprintln!("deliverables：寫不進 {p}（{e}）");
    }
}

/// `posix(p.relative_to(root))`
pub(super) fn rel_to(p: &str, root: &str) -> String {
    if root == "/" {
        return p.trim_start_matches('/').to_string();
    }
    match p.strip_prefix(root) {
        Some("") => ".".to_string(),
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
        _ => p.to_string(),
    }
}

/// `Path(p).parent`
pub(super) fn parent(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => p.get(..i).unwrap_or(".").to_string(),
        None => ".".to_string(),
    }
}

/// `Path(p).name`
pub(super) fn name_of(p: &str) -> String {
    let n = pyish::ppath(p);
    if n == "." || n == "/" {
        return String::new();
    }
    n.rsplit('/').next().unwrap_or("").to_string()
}

fn has_dotdot(g: &str) -> bool {
    posix(g).split('/').any(|seg| seg == "..")
}

/// 相對路徑（不含 ..、不是絕對路徑）；不合理回空字串。
pub fn safe_rel(raw: &Value) -> String {
    let text = if truthy(raw) { str_of(raw) } else { String::new() };
    let p = posix(&text);
    let s = pyish::strip(&p).trim_end_matches('/');
    if s.is_empty() || s.starts_with('/') || drive_re().is_match(s) || s.split('/').any(|seg| seg == ".." || seg.is_empty()) {
        return String::new();
    }
    s.to_string()
}

/// `**` 跨目錄、`*` 不跨目錄、`?` 一個字元。和資料包工具的寫法一致。
pub fn glob_re(g: &str) -> Option<Regex> {
    let p = posix(g);
    let g = dot_slash_re().replace(pyish::strip(&p), "").into_owned();
    let cs: Vec<char> = g.chars().collect();
    let mut out = String::from("^(?:");
    let mut i = 0;
    while let Some(c) = cs.get(i).copied() {
        let star2 = c == '*' && cs.get(i + 1) == Some(&'*');
        if star2 && cs.get(i + 2) == Some(&'/') {
            out.push_str("(?:.*/)?");
            i += 3;
            continue;
        }
        if star2 {
            out.push_str(".*");
            i += 2;
            continue;
        }
        match c {
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            _ => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    // Python 的 `$` 也接受結尾的一個換行
    out.push_str(r")\n?\z");
    Regex::new(&out).ok()
}

/// 清單裡對得上任何一個 glob 的檔（照清單的順序）。
pub fn match_files(files: &[String], globs: &[Value]) -> Vec<String> {
    let res: Vec<Regex> = globs.iter().filter_map(Value::as_str).filter(|g| !pyish::is_blank(g)).filter_map(glob_re).collect();
    files.iter().filter(|f| res.iter().any(|r| r.is_match(f))).cloned().collect()
}

fn walk_files(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    let (mut dirs, mut files): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let path = e.path();
        if path.is_dir() {
            // os.walk 不跟著符號連結往下走
            let link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            if !link && !SKIP_DIRS.contains(&name.as_str()) && !crate::layout::is_worktrees_name(&name) {
                dirs.push(name);
            }
        } else {
            files.push(name);
        }
    }
    files.sort();
    for f in files {
        out.push(if rel.is_empty() { f } else { format!("{rel}/{f}") });
        if out.len() >= MAX_FILES {
            return false;
        }
    }
    dirs.sort();
    for d in dirs {
        let sub = if rel.is_empty() { d.clone() } else { format!("{rel}/{d}") };
        if !walk_files(&dir.join(&d), &sub, out) {
            return false;
        }
    }
    true
}

/// git 的文字輸出：解成 UTF-8（壞的位元組換掉），\r\n 與 \r 換成 \n。
pub(super) fn git_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").replace('\r', "\n")
}

/// repo 裡的檔（相對路徑、排序）。有 git 用 ls-files，沒有就走訪（跳過常見的產出目錄）。
pub fn repo_files(repo: &str) -> Vec<String> {
    let git = std::process::Command::new("git").args(["-C", repo, "ls-files", "-co", "--exclude-standard"]).stdin(std::process::Stdio::null()).output();
    if let Ok(o) = git {
        let text = git_text(&o.stdout);
        if o.status.success() && !pyish::is_blank(&text) {
            let mut v: Vec<String> = py::splitlines(&text).into_iter().filter(|x| !pyish::is_blank(x)).map(posix).collect();
            v.sort();
            v.truncate(MAX_FILES);
            return v;
        }
    }
    let mut out = Vec::new();
    if walk_files(std::path::Path::new(repo), "", &mut out) {
        out.sort();
    }
    out
}

// ------------------------------------------------------------------ 型錄與設定
fn embedded_catalog() -> &'static Value {
    static C: OnceLock<Value> = OnceLock::new();
    C.get_or_init(|| pyish::json_loads(include_str!("../../../../plugin/scaffold/.claude/teams/deliverables/catalog.json")).unwrap_or(Value::Null))
}

fn is_catalog(v: &Value) -> bool {
    v.is_object() && get(v, "documents").is_array()
}

/// 型錄：專案的 `.claude/teams/deliverables/catalog.json`，沒有就用執行檔內建的那一份。
pub fn load_catalog(root: &str) -> Value {
    let mut cat = load_catalog_raw(root);
    // 型錄的來源寫的是預設的 spec 目錄（`specs/**/…`）；專案改了 spec 目錄（guard.config.json 的 specs_dir）就跟著換
    let sd = crate::layout::for_project(root).specs_dir;
    if sd != crate::layout::DEFAULT_SPECS_DIR {
        if let Some(docs) = cat.get_mut("documents").and_then(Value::as_array_mut) {
            for d in docs {
                if let Some(srcs) = d.get_mut("sources").and_then(Value::as_array_mut) {
                    for s in srcs {
                        if let Some(rest) = s.as_str().and_then(|t| t.strip_prefix("specs/")) {
                            *s = Value::String(format!("{sd}/{rest}"));
                        }
                    }
                }
            }
        }
    }
    cat
}

fn load_catalog_raw(root: &str) -> Value {
    if let Some(v) = load_json(&pyish::pjoin(root, &format!(".claude/teams/{CATALOG_REL}"))) {
        if is_catalog(&v) {
            return v;
        }
    }
    if is_catalog(embedded_catalog()) {
        return embedded_catalog().clone();
    }
    json!({"groups": [], "documents": [], "presets": []})
}

/// 執行檔內建的型錄（selftest 用）。
pub fn builtin_catalog() -> Value {
    embedded_catalog().clone()
}

pub(super) fn cat_docs(cat: &Value) -> &[Value] {
    pyfmt::items(cat.get("documents"))
}

/// 型錄的 id → 文件。
pub fn catalog_index(cat: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    for d in cat_docs(cat) {
        if let (true, Some(id)) = (d.is_object(), get(d, "id").as_str()) {
            m.insert(id.to_string(), d.clone());
        }
    }
    m
}

fn is_str_list(v: &Value) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(|x| x.as_str().is_some_and(|s| !pyish::is_blank(s))))
}

/// 交付設定的格式與語意（schema 表達不了的一起檢查）。
pub fn config_errors(cfg: &Value) -> Vec<String> {
    let Some(obj) = cfg.as_object() else { return vec!["交付設定不是物件".to_string()] };
    let mut errs = Vec::new();
    if let Some(d) = obj.get("dir") {
        if safe_rel(d).is_empty() {
            errs.push(format!("dir 要是專案內的相對路徑（不能是絕對路徑或含 ..）：{}", repr(d)));
        }
    }
    if let Some(e) = obj.get("summary_template").and_then(super::template::path_error) {
        errs.push(format!("summary_template {e}"));
    }
    let Some(docs) = get(cfg, "documents").as_array() else {
        errs.push("documents 要是陣列".to_string());
        return errs;
    };
    let mut seen: Vec<&str> = Vec::new();
    for (i, d) in docs.iter().enumerate() {
        let id = match (d.is_object(), get(d, "id").as_str()) {
            (true, Some(id)) if id_re().is_match(id) => id,
            _ => {
                errs.push(format!("documents[{i}] 要有 id（小寫英數與連字號）"));
                continue;
            }
        };
        if seen.contains(&id) {
            errs.push(format!("documents 的 id 重複：{id}"));
        }
        seen.push(id);
        for (key, allowed) in [("source", &SOURCE_TYPES[..]), ("when", &WHENS[..]), ("depth", &DEPTHS[..])] {
            if let Some(v) = d.get(key) {
                if !v.as_str().is_some_and(|s| allowed.contains(&s)) {
                    errs.push(format!("{id} 的 {key} 要是 {}：{}", allowed.join("／"), repr(v)));
                }
            }
        }
        if let Some(e) = d.get("template").and_then(super::template::path_error) {
            errs.push(format!("{id} 的 template {e}"));
        }
        for key in ["sections", "sources", "on", "design_sections"] {
            if d.get(key).is_some_and(|v| !is_str_list(v)) {
                errs.push(format!("{id} 的 {key} 要是字串陣列"));
            }
        }
        let srcs = get(d, "sources");
        if truthy(srcs) {
            for g in iter_any(srcs) {
                if let Some(g) = g.as_str() {
                    if g.starts_with('/') || drive_re().is_match(g) || has_dotdot(g) {
                        errs.push(format!("{id} 的 sources 不能是絕對路徑或含 ..：{g}"));
                    }
                }
            }
        }
    }
    errs
}

fn join_head(errs: &[String]) -> String {
    errs.iter().take(4).cloned().collect::<Vec<_>>().join("；")
}

/// 回傳（設定或 None, 來源說明）。專案的 deliverables.json 優先；沒有時看 profile 的 deliverables（要 enabled: true）。
pub fn load_config(root: &str) -> (Option<Value>, String) {
    let rel = format!(".claude/teams/{CONFIG_NAME}");
    let p = pyish::pjoin(root, &rel);
    if is_file(&p) {
        let mut cfg = match load_json(&p) {
            Some(v @ Value::Object(_)) => v,
            _ => return (None, format!("{rel} 讀不懂")),
        };
        let errs = config_errors(&cfg);
        if errs.is_empty() {
            super::template::fill(root, &mut cfg);
        }
        return if errs.is_empty() { (Some(cfg), rel) } else { (None, format!("交付設定有問題：{}", join_head(&errs))) };
    }
    let pf = pyish::pjoin(root, ".claude/teams/methodology.json");
    // 繼承（extends）解析失敗照「讀不懂」：當作 profile 沒有交付設定
    let prof = load_json(&pf).and_then(|v| crate::profile::resolve_str(&pf, v).ok()).unwrap_or(Value::Null);
    let d = get(&prof, "deliverables");
    if d.is_object() && get(d, "enabled") == &Value::Bool(true) && get(d, "documents").is_array() {
        let errs = config_errors(d);
        let mut d = d.clone();
        if errs.is_empty() {
            super::template::fill(root, &mut d);
        }
        return if errs.is_empty() { (Some(d), "methodology.json 的 deliverables".to_string()) } else { (None, format!("profile 的 deliverables 有問題：{}", join_head(&errs))) };
    }
    (None, "沒有交付設定（用 awf deliverables configure 產生；沒有時流程不產生也不檢查交付文件）".to_string())
}

/// 設定裡的每份文件＝型錄的預設內容，被設定裡寫的欄位蓋掉。型錄沒有的 id 是自訂文件（預設 assisted）。
pub fn resolve_docs(cfg: &Value, cat: &Value) -> Vec<Value> {
    let idx = catalog_index(cat);
    let mut out = Vec::new();
    for d in pyfmt::items(cfg.get("documents")) {
        let (Some(obj), Some(id)) = (d.as_object(), get(d, "id").as_str()) else { continue };
        let mut base: Map<String, Value> = match idx.get(id).and_then(Value::as_object) {
            Some(b) => b.clone(),
            None => json!({"id": id, "title": id, "group": "custom", "source": "assisted", "sections": []}).as_object().cloned().unwrap_or_default(),
        };
        for (k, v) in obj {
            if !(v.is_null() || v.as_str() == Some("")) {
                base.insert(k.clone(), v.clone());
            }
        }
        base.entry("when").or_insert(json!("run"));
        base.entry("depth").or_insert(json!("full"));
        base.entry("sections").or_insert(json!([]));
        base.insert("custom".to_string(), Value::Bool(!idx.contains_key(id)));
        out.push(Value::Object(base));
    }
    out
}

pub fn out_dir(cfg: &Value, root: &str) -> String {
    let rel = safe_rel(get(cfg, "dir"));
    pyish::pjoin(root, if rel.is_empty() { DEFAULT_DIR } else { &rel })
}

pub fn doc_path(cfg: &Value, root: &str, doc: &Value) -> String {
    pyish::pjoin(&out_dir(cfg, root), &format!("{}.md", sget(doc, "id")))
}

/// 來源 glob 的基準：指定的目標 repo → 設定的 repo → 專案根目錄。
pub fn source_root(cfg: &Value, root: &str, repo: Option<&str>) -> String {
    if let Some(r) = repo {
        return r.to_string();
    }
    let rel = safe_rel(get(cfg, "repo"));
    if rel.is_empty() { root.to_string() } else { pyish::pjoin(root, &rel) }
}

// ------------------------------------------------------------------ 文件的讀寫
fn or_str(doc: &Value, key: &str, default: &str) -> String {
    let v = get(doc, key);
    if truthy(v) { str_of(v) } else { default.to_string() }
}

/// 文件骨架：檔頭、每個小節的標題、來源標記與「（待補）」。
pub fn skeleton(doc: &Value) -> String {
    let id = sget(doc, "id");
    let title = or_str(doc, "title", id);
    let mut head: Vec<String> = vec!["---".into(), format!("id: {id}"), format!("title: {title}"), format!("group: {}", or_str(doc, "group", "custom")),
        format!("source: {}", or_str(doc, "source", "assisted")), format!("depth: {}", or_str(doc, "depth", "full")), "---".into(), String::new(),
        format!("# {title}"), String::new()];
    if sget(doc, "source") == "external" {
        head.push(format!("這份文件由外部提供。請把檔案放在 `external/{id}/`，檢查時只看有沒有附。"));
        head.push(String::new());
        return head.join("\n");
    }
    let srcs = strs(get(doc, "sources")).join("; ");
    for s in strs(get(doc, "sections")) {
        head.extend([format!("## {s}"), format!("<!-- awf:source {srcs} -->"), String::new(), PLACEHOLDER.to_string(), String::new()]);
    }
    head.join("\n")
}

/// 文件骨架：這份文件設了範本而且讀得到就照範本（[`super::template::skeleton`]），否則是 [`skeleton`]。
pub fn skeleton_for(root: &str, doc: &Value) -> String {
    match doc.get("template").and_then(|t| super::template::read(root, t)) {
        Some(tpl) => super::template::skeleton(doc, &tpl),
        None => skeleton(doc),
    }
}

/// 文件的一個小節（二級標題）。
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    /// 實際比對的來源（parse_doc：來源標記；doc_sections：文件層級的 sources ∪ 來源標記）。
    pub sources: Vec<String>,
    /// 小節自己的來源標記。
    pub marker: Vec<String>,
    pub body: String,
    /// 標題在第幾行（從 1 起算）。
    pub line: usize,
}

/// parse_doc 的結果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub meta: Map<String, Value>,
    pub sections: Vec<Section>,
}

/// 小節＝二級標題；來源標記是標題後第一個非空行。
pub fn parse_doc(text: &str) -> Parsed {
    let norm = text.replace("\r\n", "\n");
    let lines: Vec<&str> = norm.split('\n').collect();
    let mut meta = Map::new();
    let mut i = 0;
    if lines.first().is_some_and(|l| pyish::strip(l) == "---") {
        i = 1;
        while let Some(ln) = lines.get(i).filter(|l| pyish::strip(l) != "---") {
            if let Some(m) = meta_re().captures(ln) {
                let (k, v) = (m.get(1).map(|x| x.as_str()).unwrap_or(""), m.get(2).map(|x| x.as_str()).unwrap_or(""));
                meta.insert(k.to_string(), Value::String(pyish::strip(v).to_string()));
            }
            i += 1;
        }
        i += 1;
    }
    struct Cur {
        title: String,
        sources: Option<Vec<String>>,
        body: Vec<String>,
        line: usize,
    }
    let mut sections: Vec<Cur> = Vec::new();
    let mut fence = false;
    for (n, ln) in lines.iter().enumerate().skip(i) {
        if ln.trim_start_matches(pyish::is_space).starts_with("```") {
            fence = !fence;
        }
        if !fence && ln.starts_with("## ") && !ln.starts_with("###") {
            sections.push(Cur { title: pyish::strip(ln.get(3..).unwrap_or("")).to_string(), sources: None, body: Vec::new(), line: n + 1 });
            continue;
        }
        let Some(cur) = sections.last_mut() else { continue };
        if cur.sources.is_none() && cur.body.iter().all(|b| pyish::is_blank(b)) {
            if let Some(m) = mark_re().captures(pyish::strip(ln)) {
                let inner = m.get(1).map(|x| x.as_str()).unwrap_or("");
                cur.sources = Some(inner.split(';').map(pyish::strip).filter(|x| !x.is_empty()).map(str::to_string).collect());
                continue;
            }
        }
        cur.body.push(ln.to_string());
    }
    let sections = sections.into_iter().map(|c| {
        let sources = c.sources.unwrap_or_default();
        Section { title: c.title, marker: sources.clone(), sources, body: pyish::strip(&c.body.join("\n")).to_string(), line: c.line }
    }).collect();
    Parsed { meta, sections }
}

/// parse_doc，再把每個小節的來源設成「文件層級的 sources ∪ 小節的來源標記」：標記只能加、不能減，
/// 把標記刪掉或改成對不到檔的 glob，都躲不過過期判定。
pub fn doc_sections(doc: &Value, text: &str) -> Parsed {
    let mut parsed = parse_doc(text);
    let base = strs(get(doc, "sources"));
    for s in parsed.sections.iter_mut() {
        let mut all = base.clone();
        all.extend(s.marker.iter().filter(|g| !base.contains(g)).cloned());
        s.sources = all;
    }
    parsed
}

/// 內文的雜湊：空白、標點、全形半形的差異都不算改動（只動這些不能讓過期的小節變成最新）。
pub fn body_hash(body: &str) -> String {
    let mut kept = String::with_capacity(body.len());
    let mut buf = [0u8; 4];
    for ch in body.nfkc() {
        if !(pyish::is_space(ch) || hash_drop_re().is_match(ch.encode_utf8(&mut buf))) {
            kept.push(ch);
        }
    }
    sha1_text(&kept)
}

pub fn is_placeholder(body: &str) -> bool {
    let b = pyish::strip(body);
    b.is_empty() || b == PLACEHOLDER
}

/// 這一節的來源檔 → 雜湊。來源寫的是 glob，對不到檔就是空的。
pub fn section_sources(sources: &[String], files: &[String], src_root: &str) -> Map<String, Value> {
    let safe: Vec<Value> = sources.iter().filter(|g| !(g.starts_with('/') || drive_re().is_match(g) || has_dotdot(g))).map(|g| Value::String(g.clone())).collect();
    let mut out = Map::new();
    for f in match_files(files, &safe).into_iter().take(MAX_SECTION_FILES) {
        if let Some(h) = sha1_file(&pyish::pjoin(src_root, &f)) {
            out.insert(f, Value::String(h));
        }
    }
    out
}

pub fn load_state(root: &str) -> Value {
    load_obj(&pyish::pjoin(root, &format!(".claude/teams/{STATE_NAME}")))
}

pub fn save_state(root: &str, state: &Value) {
    write_lf(&pyish::pjoin(root, &format!(".claude/teams/{STATE_NAME}")), &format!("{}\n", py::dumps_sorted(state, Some(1))));
}

/// （狀態, 說明）。狀態：placeholder、fresh、stale、unstamped。
pub fn section_state(doc_id: &str, sec: &Section, state: &Value, files: &[String], src_root: &str) -> (&'static str, String) {
    if is_placeholder(&sec.body) {
        return ("placeholder", "還沒寫".to_string());
    }
    if sec.sources.is_empty() {
        // 這份文件沒有設定來源（例如由工具寫的開發紀錄）：沒有東西可以過期
        return ("fresh", String::new());
    }
    let rec = match get(state, doc_id) {
        d @ Value::Object(_) => get(d, &sec.title),
        _ => &NULL,
    };
    let Some(rec_src) = rec.as_object().and_then(|r| r.get("sources")) else {
        return ("unstamped", "有內容但沒有來源紀錄（跑 awf deliverables sync 記錄）".to_string());
    };
    let cur = section_sources(&sec.sources, files, src_root);
    let empty = Map::new();
    let old = rec_src.as_object();
    if old.is_some_and(|o| o.len() == cur.len() && cur.iter().all(|(k, v)| o.get(k) == Some(v))) {
        return ("fresh", String::new());
    }
    let old = old.unwrap_or(&empty);
    let keys: BTreeSet<&String> = cur.keys().chain(old.keys()).collect();
    let changed: Vec<&str> = keys.into_iter().filter(|k| cur.get(*k) != old.get(*k)).map(|k| k.as_str()).take(4).collect();
    if get(rec, "body").as_str() == Some(body_hash(&sec.body).as_str()) {
        return ("stale", format!("來源改了、內文沒動：{}", changed.join("、")));
    }
    ("unstamped", "內文改過，還沒記錄來源（跑 awf deliverables sync）".to_string())
}

// ------------------------------------------------------------------ status
/// 一份文件的狀態（ok、stale、missing、waiting）與各小節的狀態。
pub fn doc_status(doc: &Value, cfg: &Value, root: &str, state: &Value, files: &[String], src_root: &str) -> Value {
    doc_status_with(doc, cfg, root, state, files, src_root, None)
}

/// [`doc_status`]，有知識圖時多看「小節連到的需求或符號變了」（只會讓更多小節過期）。
pub fn doc_status_with(doc: &Value, cfg: &Value, root: &str, state: &Value, files: &[String], src_root: &str, links: Option<&super::kgflow::Links>) -> Value {
    let id = sget(doc, "id");
    let p = doc_path(cfg, root, doc);
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("title".into(), get(doc, "title").clone());
    out.insert("source".into(), get(doc, "source").clone());
    out.insert("when".into(), get(doc, "when").clone());
    out.insert("file".into(), json!(rel_to(&p, root)));
    out.insert("sections".into(), json!([]));
    let done = |mut out: Map<String, Value>, secs: Vec<Value>, status: &str, detail: String| -> Value {
        out.insert("sections".into(), Value::Array(secs));
        out.insert("status".into(), json!(status));
        out.insert("detail".into(), json!(detail));
        Value::Object(out)
    };
    let sources = get(doc, "sources");
    let src_head = |default: Option<&str>| -> String {
        let mut v = strs(sources);
        if v.is_empty() {
            v.extend(default.map(str::to_string));
        }
        v.into_iter().take(3).collect::<Vec<_>>().join("、")
    };
    if sget(doc, "source") == "external" {
        let ext = pyish::pjoin(&out_dir(cfg, root), &format!("external/{id}"));
        let got = std::fs::read_dir(&ext).map(|rd| rd.flatten().filter(|e| e.path().is_file() && !e.file_name().to_string_lossy().starts_with('.')).count()).unwrap_or(0);
        return if got > 0 { done(out, vec![], "ok", format!("已附 {got} 個檔")) } else { done(out, vec![], "waiting", format!("等外部提供；放在 {}/", rel_to(&ext, root))) };
    }
    let have_material = !match_files(files, &iter_any(sources)).is_empty();
    let Some(text) = (if is_file(&p) { read_text(&p) } else { None }) else {
        let detail = if have_material || !truthy(sources) { "文件還沒建立（跑 awf deliverables init）".to_string() } else { format!("文件還沒建立，也找不到原料（{}）", src_head(None)) };
        return done(out, vec![], "missing", detail);
    };
    let parsed = doc_sections(doc, &text);
    let absent: Vec<String> = strs(get(doc, "sections")).into_iter().filter(|s| !parsed.sections.iter().any(|x| &x.title == s)).collect();
    let mut secs = Vec::new();
    let mut kinds: Vec<(&str, Vec<String>)> = Vec::new();
    for s in &parsed.sections {
        let (st, why) = super::kgflow::linked_state(section_state(id, s, state, files, src_root), id, s, state, links);
        secs.push(json!({"title": s.title, "state": st, "detail": why}));
        match kinds.iter_mut().find(|k| k.0 == st) {
            Some(k) => k.1.push(s.title.clone()),
            None => kinds.push((st, vec![s.title.clone()])),
        }
    }
    let kind = |name: &str| kinds.iter().find(|k| k.0 == name).map(|k| k.1.clone()).unwrap_or_default();
    if !kind("stale").is_empty() {
        return done(out, secs, "stale", format!("過期的小節：{}", kind("stale").join("、")));
    }
    if !kind("unstamped").is_empty() {
        return done(out, secs, "stale", format!("還沒記錄來源的小節：{}", kind("unstamped").join("、")));
    }
    let mut todo = kind("placeholder");
    todo.extend(absent);
    if !todo.is_empty() {
        if sget(doc, "source") == "assisted" {
            return done(out, secs, "waiting", format!("等人補的小節：{}", todo.join("、")));
        }
        if !have_material {
            return done(out, secs, "missing", format!("找不到原料（{}）；小節：{}", src_head(Some("沒有設定 sources")), todo.join("、")));
        }
        return done(out, secs, "stale", format!("原料已有、還沒產生的小節：{}", todo.join("、")));
    }
    done(out, secs, "ok", String::new())
}

/// 每份文件的狀態；沒有交付設定回（None, 說明）。
pub fn status_all(root: &str, repo: Option<&str>) -> (Option<Vec<Value>>, String) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (None, note) };
    let cat = load_catalog(root);
    let src_root = source_root(&cfg, root, repo);
    let files = repo_files(&src_root);
    let state = load_state(root);
    let links = super::kgflow::Links::open(root);
    (Some(resolve_docs(&cfg, &cat).iter().map(|d| doc_status_with(d, &cfg, root, &state, &files, &src_root, links.as_ref())).collect()), note)
}

// ------------------------------------------------------------------ index
fn group_of(d: &Value) -> String {
    let g = get(d, "group");
    if truthy(g) { str_of(g) } else { "custom".to_string() }
}

/// 總目錄（README.md）的內容。
pub fn index_text(cfg: &Value, cat: &Value) -> String {
    let cat_groups: Vec<&Value> = pyfmt::items(cat.get("groups")).iter().filter(|g| g.is_object()).collect();
    let title_of = |gid: &str| -> String {
        cat_groups.iter().rev().find(|g| sget(g, "id") == gid).map(|g| fmt(get(g, "title")))
            .unwrap_or_else(|| if gid == "custom" { "自訂".to_string() } else { gid.to_string() })
    };
    let label = |v: &Value| match v.as_str() {
        Some("auto") => "自動產生".to_string(),
        Some("assisted") => "工具給原料、人補內容".to_string(),
        Some("external") => "外部提供".to_string(),
        _ => fmt(v),
    };
    let when = |v: &Value| match v.as_str() {
        Some("run") => "每次交付更新".to_string(),
        Some("milestone") => "里程碑".to_string(),
        Some("close") => "結案".to_string(),
        _ => fmt(v),
    };
    let mut lines: Vec<String> = vec!["# 交付文件總目錄".into(), String::new(), "這份目錄由 `awf deliverables index` 產生，請不要手改。".into(), String::new()];
    let docs = resolve_docs(cfg, cat);
    let mut order: Vec<String> = cat_groups.iter().map(|g| sget(g, "id").to_string()).collect();
    order.push("custom".to_string());
    for gid in &order {
        let mine: Vec<&Value> = docs.iter().filter(|d| { let g = group_of(d); &g == gid || (gid == "custom" && !order.contains(&g)) }).collect();
        if mine.is_empty() {
            continue;
        }
        lines.extend([format!("## {}", title_of(gid)), String::new(), "| 文件 | 檔案 | 來源 | 更新時機 |".into(), "|---|---|---|---|".into()]);
        for d in mine {
            let id = sget(d, "id");
            let link = if sget(d, "source") == "external" { format!("external/{id}/") } else { format!("[{id}.md]({id}.md)") };
            lines.push(format!("| {} | {link} | {} | {} |", fmt(get(d, "title")), label(get(d, "source")), when(get(d, "when"))));
        }
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n").trim_end_matches(pyish::is_space))
}

// ------------------------------------------------------------------ 隱私規則
type Fx = fancy_regex::Regex;

/// 隱私字串：交付文件是給外部的人看的。命中任何一條，check 失敗、sync 不寫。規則只能加、不能拿掉。
pub(super) fn privacy_rules() -> &'static [(&'static str, Fx)] {
    static R: OnceLock<Vec<(&'static str, Fx)>> = OnceLock::new();
    R.get_or_init(|| {
        let roles: Vec<String> = AGENT_ROLES.iter().map(|r| r.replace('-', "[-_]")).collect();
        let pats: Vec<(&'static str, String)> = vec![
            ("電子郵件", py::translate(r"[A-Za-z0-9._%+-]+\s*@\s*[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+", false)),
            ("本機路徑", format!("(?i){}", py::translate(concat!(r"(?:/", r"Users/|/", r"home/|/root/|/var/folders/|/private/(?:tmp|var)/|/tmp/|~/)[^\s]*|[A-Za-z]:\\+", r"Users\\+[^\\\s]+|\\\\[\w.-]+\\[\w$.-]+"), true))),
            ("憑證或金鑰", format!("{}|(?i:{})",
                py::translate(concat!(r"-----BEGIN [A-Z ]*PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}|\bsk-[A-Za-z0-9_-]{20,}|\bAKIA[0-9A-Z]{16}\b",
                    r"|\bxox[abprs]-[A-Za-z0-9-]{10,}|\bAIza[0-9A-Za-z_-]{30,}|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                    r"|hooks\.slack\.com/services/\S+"), false),
                py::translate(r"\bbearer\s+[A-Za-z0-9._~+/-]{16,}", true))),
            ("密碼或 token 的值", format!("(?i){}", py::translate(r#"(?:\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?key|token|client[_-]?secret)\b|密碼|密鑰|金鑰)["']?\s*[:=]\s*["']?[^\s"'<>（）]{6,}"#, true))),
            ("內部 agent 名稱", format!("(?i){}", py::translate(&format!(r"\b(?:tpm|dev)\s*[-_\\]{{1,2}}\s*(?:{})\b", roles.join("|")), true))),
            ("內部網路位址或主機", format!("(?i){}", py::translate(concat!(r"\b(?:10\.\d{1,3}|192\.168|172\.(?:1[6-9]|2\d|3[01])|127\.\d{1,3})\.\d{1,3}\.\d{1,3}\b|\blocalhost\b",
                r"|\b[\w-]+(?:\.[\w-]+)*\.(?:internal|intranet|corp|lan|local|localdomain)\b|\bf[cd][0-9a-f]{2}:[0-9a-f:]{4,}"), true))),
        ];
        pats.into_iter().map(|(name, p)| (name, py::compile_raw(&p).expect("固定的隱私規則"))).collect()
    })
}

/// 比對前的正規化：全形轉半形（NFKC）、去掉零寬字元、HTML 實體還原、反斜線跳脫的連字號還原。
pub fn privacy_text(text: &str) -> String {
    let t: String = py::html_unescape(text).nfkc().filter(|c| !matches!(*c as u32, 0x200b..=0x200f | 0x2060..=0x2064 | 0xfeff | 0xad)).collect();
    t.replace("\\-", "-")
}

/// 額外的隱私規則（種類, 一組字面值）：文字裡出現任何一個字面值就算命中。
pub struct ExtraRule {
    pub name: String,
    rx: Option<Fx>,
    /// 正規式編不出來時改用字面比對（只會一樣或更嚴）。
    lits: Vec<String>,
}

impl ExtraRule {
    /// 依序比對這些字面值（和 `"|".join(re.escape(n) …)` 編成的正規式相同）。
    pub fn literals(name: &str, lits: &[String]) -> Self {
        let pats: Vec<String> = lits.iter().map(|n| regex::escape(n)).collect();
        ExtraRule { name: name.to_string(), rx: if pats.is_empty() { None } else { py::compile_raw(&pats.join("|")) }, lits: lits.to_vec() }
    }
}

fn hit_any(rx: &Fx, text: &str, allowed: &[Fx]) -> bool {
    match py::find_all(rx, text) {
        Ok(ms) => ms.into_iter().any(|m| !allowed.iter().any(|a| py::search(a, m))),
        Err(()) => true,
    }
}

/// 命中的隱私字串種類與行號（不印內容本身，避免把機敏值再印一次）。比對前先正規化（privacy_text），
/// 另把整份文字去掉換行再掃一次，抓拆成兩行的信箱與 token。
pub fn privacy_hits(text: &str, allow: &[Value], extra: &[ExtraRule]) -> Vec<String> {
    let allowed: Vec<Fx> = allow.iter().filter_map(Value::as_str).filter_map(py::compile_py).collect();
    let mut rules: Vec<(&str, &Fx)> = privacy_rules().iter().map(|(n, r)| (*n, r)).collect();
    rules.extend(extra.iter().filter_map(|x| x.rx.as_ref().map(|r| (x.name.as_str(), r))));
    let fallback: Vec<&ExtraRule> = extra.iter().filter(|x| x.rx.is_none() && !x.lits.is_empty()).collect();
    let mut out = Vec::new();
    let norm_text = privacy_text(&text.replace("\r\n", "\n"));
    for (n, ln) in norm_text.split('\n').enumerate() {
        for (name, rx) in &rules {
            if hit_any(rx, ln, &allowed) {
                out.push(format!("第 {} 行：{name}", n + 1));
            }
        }
        for x in &fallback {
            if x.lits.iter().any(|l| !l.is_empty() && ln.contains(l.as_str()) && !allowed.iter().any(|a| py::search(a, l))) {
                out.push(format!("第 {} 行：{}", n + 1, x.name));
            }
        }
    }
    if out.is_empty() {
        let joined = join_re().replace_all(&norm_text, "");
        for (name, rx) in rules.iter().take(3) {
            if hit_any(rx, &joined, &allowed) {
                out.push(format!("跨行：{name}"));
            }
        }
    }
    out
}

/// runs_dir 底下的 run 目錄名稱與 runs_dir 本身：都是內部資訊，不能出現在交付文件。
pub fn run_name_rule(root: &str) -> ExtraRule {
    let guard = load_json(&pyish::pjoin(root, ".claude/teams/guard.config.json")).unwrap_or(Value::Null);
    let mut rd = safe_rel(get(&guard, "runs_dir"));
    if rd.is_empty() {
        rd = "_tpm/runs".to_string();
    }
    let mut names: Vec<String> = std::fs::read_dir(pyish::pjoin(root, &rd))
        .map(|d| d.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| pyfmt::len(n) >= 6).collect())
        .unwrap_or_default();
    names.sort();
    let skip = names.len().saturating_sub(500);
    let mut lits = vec![rd];
    lits.extend(names.into_iter().skip(skip));
    ExtraRule::literals("內部 run 目錄", &lits)
}

// ------------------------------------------------------------------ check
/// 一行一個正規式（# 開頭是註解）。讀不到或寫壞的列在說明裡。
fn load_patterns(root: &str, rel: &Value) -> (Vec<Fx>, Vec<String>) {
    let rel = safe_rel(rel);
    if rel.is_empty() {
        return (vec![], vec![]);
    }
    let Some(raw) = read_text(&pyish::pjoin(root, &rel)) else { return (vec![], vec![format!("禁用字清單讀不到：{rel}")]) };
    let (mut pats, mut notes) = (Vec::new(), Vec::new());
    for ln in py::splitlines(&raw) {
        let ln = pyish::strip(ln);
        if ln.is_empty() || ln.starts_with('#') {
            continue;
        }
        match py::compile_py(ln) {
            Some(r) => pats.push(r),
            None => notes.push("禁用字清單有寫壞的正規式（略過，不印內容）".to_string()),
        }
    }
    (pats, notes)
}

fn link_re() -> &'static Fx {
    static R: OnceLock<Fx> = OnceLock::new();
    R.get_or_init(|| py::compile_raw(&py::translate(r#"(?<!\!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)"#, false)).expect("固定的正規式"))
}

/// 對不到檔的相對連結（程式碼區塊裡的不看）。
pub fn broken_links(text: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence = false;
    for (n, ln) in text.replace("\r\n", "\n").split('\n').enumerate() {
        if ln.trim_start_matches(pyish::is_space).starts_with("```") {
            fence = !fence;
        }
        if fence {
            continue;
        }
        for m in link_re().captures_iter(ln).flatten() {
            let t = m.get(1).map(|x| x.as_str()).unwrap_or("");
            if scheme_re().is_match(t) {
                continue;
            }
            let t = t.split('#').next().unwrap_or("");
            if !t.is_empty() && !std::path::Path::new(&pyish::pjoin(base, t)).exists() {
                out.push(format!("第 {} 行：連結對不到檔（{t}）", n + 1));
            }
        }
    }
    out
}

pub(super) fn allow_list(cfg: &Value) -> Vec<Value> {
    pyfmt::items(cfg.get("privacy_allow")).to_vec()
}

/// 回傳（問題清單, 說明）。只檢查交付設定裡列的文件。沒有交付設定回 (None, 說明)。
pub fn check_all(root: &str, repo: Option<&str>) -> (Option<Vec<String>>, String) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (None, note) };
    let cat = load_catalog(root);
    let docs = resolve_docs(&cfg, &cat);
    let src_root = source_root(&cfg, root, repo);
    let files = repo_files(&src_root);
    let state = load_state(root);
    let (banned, notes) = load_patterns(root, get(&cfg, "banned_terms_file"));
    let allow = allow_list(&cfg);
    let mut errs: Vec<String> = notes;
    let runrule = [run_name_rule(root)];
    let d_out = out_dir(&cfg, root);
    let links = super::kgflow::Links::open(root);
    for doc in &docs {
        let id = sget(doc, "id");
        if sget(doc, "source") == "external" {
            let st = doc_status(doc, &cfg, root, &state, &files, &src_root);
            if sget(&st, "status") != "ok" {
                errs.push(format!("{id}：{}", sget(&st, "detail")));
            }
            continue;
        }
        let p = doc_path(&cfg, root, doc);
        let rel = rel_to(&p, root);
        let Some(text) = (if is_file(&p) { read_text(&p) } else { None }) else {
            errs.push(format!("{rel}：文件不存在（跑 awf deliverables init）"));
            continue;
        };
        let parsed = doc_sections(doc, &text);
        for key in ["id", "title", "group", "source"] {
            if !parsed.meta.get(key).is_some_and(truthy) {
                errs.push(format!("{rel}：檔頭缺 {key}"));
            }
        }
        if let Some(mid) = parsed.meta.get("id").and_then(Value::as_str).filter(|m| !m.is_empty() && *m != id) {
            errs.push(format!("{rel}：檔頭的 id（{mid}）和檔名不一致"));
        }
        for s in &parsed.sections {
            let (st, why) = super::kgflow::linked_state(section_state(id, s, &state, &files, &src_root), id, s, &state, links.as_ref());
            if st == "stale" || st == "unstamped" {
                errs.push(format!("{rel}〈{}〉：{why}", s.title));
            }
        }
        for (n, ln) in text.split('\n').enumerate() {
            if banned.iter().any(|b| py::search(b, ln)) {
                errs.push(format!("{rel} 第 {} 行：出現禁用字", n + 1));
            }
        }
        errs.extend(privacy_hits(&text, &allow, &runrule).into_iter().map(|h| format!("{rel} {h}")));
        errs.extend(broken_links(&text, &parent(&p)).into_iter().map(|h| format!("{rel} {h}")));
    }
    let idx = pyish::pjoin(&d_out, INDEX_NAME);
    match if is_file(&idx) { read_text(&idx) } else { None } {
        None => errs.push(format!("{}：總目錄不存在（跑 awf deliverables index）", rel_to(&idx, root))),
        Some(t) if t.replace("\r\n", "\n") != index_text(&cfg, &cat) => errs.push(format!("{}：總目錄和交付設定不一致（跑 awf deliverables index）", rel_to(&idx, root))),
        _ => {}
    }
    (Some(errs), note)
}
