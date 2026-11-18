//! 去客戶化掃描：用一批正規式（一行一條）掃檔案內容與路徑名，擋掉不該出現的字串（客戶名、內部代號…）。
//!
//! 兩個用途共用這裡的掃描邏輯：
//! - `awf scrub`：使用者的專案。**要不要掃、掃什麼、什麼時候掃都由使用者在 profile 的 `scrub` 決定，預設完全不啟用。**
//! - `cargo xtask scrub`：模版 repo 自己（規則檔固定是 `tests/scrub_terms*.txt`，輸出格式是它自己的）。
//!
//! ## profile 的 `scrub`
//!
//! | 鍵 | 意思 |
//! |---|---|
//! | `enabled` | 預設 false。沒開時 `awf scrub` 印「未啟用」、exit 0，不讀任何檔；`awf publish` 完全不掃 |
//! | `terms` | 直接寫在 profile 的規則（陣列，一條一個正規式）。會進版控，真實名稱不要寫這裡 |
//! | `terms_files` | 專案內的規則檔（相對專案根目錄；一行一個正規式，`#` 開頭是註解）。可以是 gitignore 的本機檔，不存在就略過並提示 |
//! | `paths`／`exclude` | 掃哪些路徑、略過哪些（glob，相對被掃的 repo）。沒寫 `paths` 就全部 |
//! | `when` | 自動跑的時機，目前只有 `publish`（`awf publish` 推送前掃要推的檔，命中就不推）。空的＝只在人手動跑時掃 |
//!
//! 正規式的語法同 [`crate::termre`]（0.x 沿用下來的那一套）。
//!
//! ## 不外洩
//!
//! 規則本身可能就是機密名稱，所以：命中只印 `路徑:行號` 與「第幾條規則（來源:行號）」，**不印命中的字串、也不印規則**；
//! 規則編不過只報來源與行號。
//!
//! ## 失敗語意
//!
//! 這是使用者要求的閘：設定讀不懂、規則編不過、列不出檔案，都算失敗（`awf scrub` exit 2；`awf publish` 不推）。
//! profile 整份不是合法 JSON 時看不出有沒有啟用：內容裡出現 `"scrub"` 就當成失敗，沒有就當成沒啟用（和沒有這個功能時一樣）。

use crate::merge_gate::pyutil;
use crate::termre::{self, Pat};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ------------------------------------------------------------------ 共用的掃描邏輯

/// 一條規則：改寫好的正規式與它的來源（不留原文）。
pub struct Rule {
    pub pat: Pat,
    /// 來源：規則檔的路徑，或 `scrub.terms`。
    pub source: String,
    /// 來源裡的第幾行（inline 的規則是第幾條），從 1 起算。
    pub line: usize,
}

/// 讀一份規則文字：一行一條，空行與 `#` 開頭的註解略過。編不過回（行號, 原因）；原因不含規則內容。
pub fn parse_terms(text: &str, source: &str) -> Result<Vec<Rule>, (usize, String)> {
    let text = pyutil::universal_newlines(text);
    let mut out = Vec::new();
    for (i, line) in pyutil::splitlines(&text).into_iter().enumerate() {
        if pyutil::strip(line).is_empty() || line.trim_start_matches(pyutil::is_py_space).starts_with('#') {
            continue;
        }
        match termre::compile(line) {
            Ok(pat) => out.push(Rule { pat, source: source.to_string(), line: i + 1 }),
            Err(why) => return Err((i + 1, why)),
        }
    }
    Ok(out)
}

/// 一個要掃的項目：顯示用的相對路徑、實際路徑、是不是檔案。目錄本身也算一項，用來掃路徑名。
pub struct Entry {
    pub shown: String,
    pub path: PathBuf,
    pub is_file: bool,
}

/// 相對路徑的各段 → 顯示用的寫法（`./a/b`；空的是 `.`）。
pub fn shown_of(rel: &[String]) -> String {
    if rel.is_empty() { ".".to_string() } else { format!("./{}", rel.join("/")) }
}

/// 照 `os.walk` 的順序走一棵目錄（每層先列子目錄、再列檔案，都依名稱排序，然後才往下走）。
/// `skip` 是任何一層都略過的名字（目錄或檔案，例如 `.git`）；`skip_root` 只在最上層略過。指向目錄的符號連結列出來但不往下走。
pub fn walk_dir(dir: &Path, rel: &[String], skip: &[&str], skip_root: &[&str], out: &mut Vec<Entry>) {
    let mut dirs: Vec<(String, bool)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            dirs.push((name, is_link));
        } else {
            files.push(name);
        }
    }
    dirs.retain(|(d, _)| !skip.contains(&d.as_str()) && !(rel.is_empty() && skip_root.contains(&d.as_str())));
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
        if skip.contains(&f.as_str()) {
            continue; // worktree 裡的 .git 是檔案
        }
        out.push(Entry { shown: shown_of(&child(f)), path: dir.join(f), is_file: true });
    }
    for (d, is_link) in &dirs {
        if !is_link {
            walk_dir(&dir.join(d), &child(d), skip, skip_root, out);
        }
    }
}

/// 二進位檔：前 8192 個位元組裡有 NUL。
pub fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|b| *b == 0)
}

/// 路徑名命中哪一條規則（第一條；索引從 0 起算）。Err 是正規式執行失敗。
pub fn match_name(rules: &[Rule], name: &str) -> Result<Option<usize>, String> {
    for (i, r) in rules.iter().enumerate() {
        if r.pat.search_name(name)? {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// 內容逐行比：回（行號（從 1）, 第一條命中的規則的索引, 那一行）。
pub fn match_lines<'t>(rules: &[Rule], text: &'t str) -> Result<Vec<(usize, usize, &'t str)>, String> {
    let mut out = Vec::new();
    for (n, line) in pyutil::splitlines(text).into_iter().enumerate() {
        for (i, r) in rules.iter().enumerate() {
            if r.pat.search(line)? {
                out.push((n + 1, i, line));
                break;
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ 專案的設定（profile 的 scrub）

/// `when` 可以寫的值。
pub const WHEN: &[&str] = &["publish"];
/// inline 規則的來源名稱。
pub const INLINE: &str = "scrub.terms";
/// 沒有 git 時略過的目錄名。
const SKIP_DIRS: &[&str] = &[".git", "__pycache__", "node_modules"];
/// profile 相對專案根目錄的位置。
const PROFILE_REL: &str = ".claude/teams/methodology.json";

/// 專案的掃描設定（已啟用才會有）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub terms: Vec<String>,
    pub terms_files: Vec<String>,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub when: Vec<String>,
}

impl Config {
    /// `when` 有沒有這個時機。
    pub fn runs_on(&self, when: &str) -> bool {
        self.when.iter().any(|w| w == when)
    }
}

fn bad_path(p: &str) -> bool {
    let t = p.replace('\\', "/");
    let drive = t.as_bytes().get(1) == Some(&b':') && t.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    t.trim().is_empty() || t.starts_with('/') || t.starts_with('~') || drive || t.split('/').any(|c| c == "..") || t.contains('\0')
}

/// glob → 正規式：`*` 不跨目錄、`**` 跨目錄、`?` 一個字元；其他照字面。
fn glob_re(glob: &str) -> Option<Regex> {
    let g = glob.replace('\\', "/");
    let g = g.trim_start_matches("./").trim_end_matches('/');
    let cs: Vec<char> = g.chars().collect();
    let mut out = String::from("^(?:");
    let mut i = 0;
    while let Some(c) = cs.get(i).copied() {
        let star2 = c == '*' && cs.get(i + 1) == Some(&'*');
        if star2 && cs.get(i + 2) == Some(&'/') {
            out.push_str("(?:.*/)?");
            i += 3;
        } else if star2 {
            out.push_str(".*");
            i += 2;
        } else {
            match c {
                '*' => out.push_str("[^/]*"),
                '?' => out.push_str("[^/]"),
                _ => out.push_str(&regex::escape(&c.to_string())),
            }
            i += 1;
        }
    }
    out.push_str(r")\z");
    Regex::new(&out).ok()
}

fn strings<'a>(v: Option<&'a Value>, key: &str, errs: &mut Vec<String>) -> Vec<&'a str> {
    match v {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            if a.iter().any(|x| !x.is_string()) {
                errs.push(format!("scrub.{key} 的項目要是字串"));
            }
            a.iter().filter_map(Value::as_str).collect()
        }
        Some(_) => {
            errs.push(format!("scrub.{key} 要是陣列"));
            Vec::new()
        }
    }
}

/// `scrub` 這一節的規則（schema 表達不了的部分，加上型別；[`crate::schema::methodology_errors`] 與 [`parse_config`] 共用）。
/// 訊息不含規則內容：編不過的規則只說是第幾條。
pub fn config_errors(scrub: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(o) = scrub.as_object() else {
        return vec!["scrub 要是物件".to_string()];
    };
    if o.get("enabled").is_some_and(|v| !v.is_boolean()) {
        errs.push("scrub.enabled 要是 true 或 false".to_string());
    }
    for (i, t) in strings(o.get("terms"), "terms", &mut errs).iter().enumerate() {
        if pyutil::strip(t).is_empty() {
            errs.push(format!("scrub.terms 第 {} 條是空的（空的規則會命中每一行）", i + 1));
        } else if t.contains('\n') || t.contains('\r') {
            errs.push(format!("scrub.terms 第 {} 條不能有換行（一條一個正規式）", i + 1));
        } else if let Err(why) = termre::compile(t) {
            errs.push(format!("scrub.terms 第 {} 條規則編不過（{why}）", i + 1));
        }
    }
    for (i, f) in strings(o.get("terms_files"), "terms_files", &mut errs).iter().enumerate() {
        if bad_path(f) {
            errs.push(format!("scrub.terms_files 第 {} 項要是專案內的相對路徑，不能是絕對路徑或含 ..", i + 1));
        }
    }
    for key in ["paths", "exclude"] {
        for (i, g) in strings(o.get(key), key, &mut errs).iter().enumerate() {
            if bad_path(g) {
                errs.push(format!("scrub.{key} 第 {} 項要是相對路徑的 glob，不能是絕對路徑或含 ..", i + 1));
            } else if glob_re(g).is_none() {
                errs.push(format!("scrub.{key} 第 {} 項的 glob 讀不懂", i + 1));
            }
        }
    }
    for w in strings(o.get("when"), "when", &mut errs) {
        if !WHEN.contains(&w) {
            errs.push(format!("scrub.when 只能是 {}（收到 {w}）", WHEN.join("、")));
        }
    }
    errs
}

/// 從 profile 取掃描設定。`Ok(None)`＝沒啟用（沒有 `scrub`、是 null、`enabled` 不是 true）；設定有錯回 Err。
pub fn parse_config(profile: &Value) -> Result<Option<Config>, String> {
    let scrub = match profile.get("scrub") {
        None | Some(Value::Null) => return Ok(None),
        Some(s) => s,
    };
    let errs = config_errors(scrub);
    if !errs.is_empty() {
        return Err(format!("profile 的 scrub 設定有錯：{}", errs.join("；")));
    }
    if scrub.get("enabled") != Some(&Value::Bool(true)) {
        return Ok(None);
    }
    let list = |k: &str| -> Vec<String> {
        scrub.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    Ok(Some(Config { terms: list("terms"), terms_files: list("terms_files"), paths: list("paths"), exclude: list("exclude"), when: list("when") }))
}

/// 讀專案的 profile 檔取掃描設定。檔案不存在＝沒啟用。
pub fn load_config(profile_path: &Path) -> Result<Option<Config>, String> {
    if !profile_path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read(profile_path).map_err(|e| format!("讀不到 profile {}：{e}", profile_path.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match serde_json::from_str::<Value>(text) {
        // 繼承（extends）解析失敗：看不出有沒有啟用掃描，一律當失敗（不能默默不掃）
        Ok(v) => parse_config(&crate::profile::resolve_file(profile_path, v)
            .map_err(|e| format!("profile {} 的繼承（extends）讀不懂（{e}），看不出去客戶化掃描有沒有啟用", profile_path.display()))?),
        // 整份讀不懂：看不出有沒有啟用。提到 scrub 就當失敗（不能默默不掃），沒提到就和沒有這個功能時一樣
        Err(e) if text.contains("\"scrub\"") => Err(format!("profile {} 不是合法 JSON（{e}），看不出去客戶化掃描有沒有啟用", profile_path.display())),
        Err(_) => Ok(None),
    }
}

/// 載入後的規則。
pub struct Rules {
    pub rules: Vec<Rule>,
    /// 要讓人知道的事（規則檔不存在、沒有任何規則）。
    pub notes: Vec<String>,
    /// 讀到的規則檔（相對專案根目錄）：掃描時不掃它們的內容（每條規則都會命中自己）。
    pub files: Vec<String>,
}

fn norm_rel(p: &str) -> String {
    let t = p.replace('\\', "/");
    t.split('/').filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join("/")
}

/// 讀設定裡的全部規則：先 inline 的 `terms`，再依序每個規則檔。`root` 是專案根目錄。
pub fn load_rules(cfg: &Config, root: &Path) -> Result<Rules, String> {
    let mut out = Rules { rules: Vec::new(), notes: Vec::new(), files: Vec::new() };
    for (i, t) in cfg.terms.iter().enumerate() {
        match termre::compile(t) {
            Ok(pat) => out.rules.push(Rule { pat, source: INLINE.to_string(), line: i + 1 }),
            Err(why) => return Err(format!("{INLINE} 第 {} 條規則編不過（{why}）", i + 1)),
        }
    }
    for f in &cfg.terms_files {
        let rel = norm_rel(f);
        if bad_path(f) {
            return Err(format!("規則檔要是專案內的相對路徑：{rel}"));
        }
        let path = root.join(&rel);
        if !path.is_file() {
            out.notes.push(format!("規則檔 {rel} 不存在，略過（這份清單的規則這次沒有掃）"));
            continue;
        }
        let text = std::fs::read(&path).ok().and_then(|b| String::from_utf8(b).ok()).ok_or_else(|| format!("規則檔 {rel} 讀不到或不是 UTF-8"))?;
        let rules = parse_terms(&text, &rel).map_err(|(n, why)| format!("規則檔 {rel} 第 {n} 行的規則編不過（{why}）；規則不能略過，請改寫這一行"))?;
        out.rules.extend(rules);
        out.files.push(rel);
    }
    if out.rules.is_empty() {
        out.notes.push("已啟用，但沒有任何規則（scrub.terms 是空的、terms_files 的檔都不存在或是空的），沒有東西可以掃".to_string());
    }
    Ok(out)
}

/// 掃描範圍（`paths`／`exclude`）。
pub struct Scope {
    paths: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl Scope {
    pub fn new(cfg: &Config) -> Result<Scope, String> {
        let build = |globs: &[String], key: &str| -> Result<Vec<Regex>, String> {
            globs.iter().enumerate().map(|(i, g)| glob_re(g).ok_or_else(|| format!("scrub.{key} 第 {} 項的 glob 讀不懂", i + 1))).collect()
        };
        Ok(Scope { paths: build(&cfg.paths, "paths")?, exclude: build(&cfg.exclude, "exclude")? })
    }

    /// glob 對到這個路徑本身或它的某一層上層目錄（寫 `docs` 就涵蓋 `docs/` 底下全部）。
    fn hit(globs: &[Regex], rel: &str) -> bool {
        let mut ends: Vec<usize> = rel.match_indices('/').map(|(i, _)| i).collect();
        ends.push(rel.len());
        ends.iter().any(|&e| globs.iter().any(|g| g.is_match(&rel[..e])))
    }

    /// 這個路徑（相對 repo 根目錄，`/` 分隔）在不在掃描範圍裡。
    pub fn covers(&self, rel: &str) -> bool {
        (self.paths.is_empty() || Scope::hit(&self.paths, rel)) && !Scope::hit(&self.exclude, rel)
    }
}

/// 一處命中。不含命中的字串。
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// 相對路徑；路徑名命中、而且命中的是某一層目錄時，結尾有 `/`。
    pub path: String,
    /// 行號（從 1）；0＝路徑名命中。
    pub line: usize,
    /// 規則的索引（從 0）。
    pub rule: usize,
}

/// 掃描結果。
#[derive(Debug, Default)]
pub struct Report {
    pub hits: Vec<Hit>,
    /// 實際掃了內容或路徑名的檔案數。
    pub files: usize,
    pub notes: Vec<String>,
}

/// 「規則 #N（來源:行號）」。
pub fn rule_label(rules: &[Rule], idx: usize) -> String {
    match rules.get(idx) {
        Some(r) if r.source == INLINE => format!("規則 #{}（{INLINE} 第 {} 條）", idx + 1, r.line),
        Some(r) => format!("規則 #{}（{}:{}）", idx + 1, r.source, r.line),
        None => format!("規則 #{}", idx + 1),
    }
}

/// 一處命中的文字（`路徑:行號  規則 #N（來源:行號）`；路徑名命中沒有行號）。
pub fn hit_label(rules: &[Rule], h: &Hit) -> String {
    if h.line == 0 {
        format!("{}  {}", h.path, rule_label(rules, h.rule))
    } else {
        format!("{}:{}  {}", h.path, h.line, rule_label(rules, h.rule))
    }
}

/// 掃一批檔案（相對路徑，`/` 分隔）：路徑名（含每一層上層目錄，同一個目錄只報一次）與內容。
/// `read` 回檔案內容；`Ok(None)`＝這個檔不掃內容（不存在、符號連結、規則檔自己）。二進位檔略過內容。
pub fn scan_files(rules: &[Rule], scope: &Scope, names: &[String], read: &mut dyn FnMut(&str) -> Result<Option<Vec<u8>>, String>) -> Result<Report, String> {
    let mut rep = Report::default();
    if rules.is_empty() {
        return Ok(rep);
    }
    let mut seen_prefix: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for name in names {
        let rel = norm_rel(name);
        if rel.is_empty() || !scope.covers(&rel) {
            continue;
        }
        rep.files += 1;
        // 路徑名：由上往下找第一層命中的（目錄命中就不再往下報）
        let mut ends: Vec<usize> = rel.match_indices('/').map(|(i, _)| i).collect();
        ends.push(rel.len());
        for &e in &ends {
            let prefix = &rel[..e];
            let hit = match seen_prefix.get(prefix) {
                Some(h) => *h,
                None => {
                    let h = match_name(rules, prefix)?;
                    seen_prefix.insert(prefix.to_string(), h);
                    h
                }
            };
            if let Some(rule) = hit {
                let shown = if e == rel.len() { rel.clone() } else { format!("{prefix}/") };
                if reported.insert(shown.clone()) {
                    rep.hits.push(Hit { path: shown, line: 0, rule });
                }
                break;
            }
        }
        let Some(data) = read(&rel)? else { continue };
        if is_binary(&data) {
            continue;
        }
        let text = String::from_utf8_lossy(&data);
        for (line, rule, _) in match_lines(rules, &text)? {
            rep.hits.push(Hit { path: rel.clone(), line, rule });
        }
    }
    Ok(rep)
}

fn git_out(root: &Path, args: &[&str]) -> Option<(bool, Vec<u8>, String)> {
    let out = Command::new("git").arg("-C").arg(root).args(args).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0").output().ok()?;
    Some((out.status.success(), out.stdout, String::from_utf8_lossy(&out.stderr).trim().to_string()))
}

/// 列出 `root` 底下要掃的檔（相對 `root`）：在 git repo 裡用 git 列（有追蹤的＋沒追蹤但沒被 .gitignore 略過的）；
/// 不是 git repo（或沒有 git）就自己走目錄，略過 `.git`、`node_modules`、`__pycache__`，這時看不到 .gitignore（回傳的第二個值是說明）。
/// `targets` 是相對 `root` 的檔案或目錄，空的＝全部。
pub fn list_files(root: &Path, targets: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let inside = git_out(root, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|(ok, out, _)| ok && String::from_utf8_lossy(&out).trim() == "true");
    if inside {
        let mut args: Vec<String> = ["ls-files", "-z", "--cached", "--others", "--exclude-standard"].iter().map(|s| s.to_string()).collect();
        if !targets.is_empty() {
            args.push("--".into());
            args.extend(targets.iter().map(|t| format!(":(literal){t}")));
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (ok, out, err) = git_out(root, &argv).ok_or_else(|| "跑不了 git ls-files".to_string())?;
        if !ok {
            return Err(format!("git ls-files 失敗：{err}"));
        }
        let mut names: Vec<String> = out.split(|b| *b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).into_owned()).collect();
        names.sort();
        names.dedup();
        return Ok((names, None));
    }
    let mut entries = Vec::new();
    walk_dir(root, &[], SKIP_DIRS, &[], &mut entries);
    let wanted: Vec<String> = targets.iter().map(|t| norm_rel(t)).collect();
    let mut names: Vec<String> = entries.iter().filter(|e| e.is_file).map(|e| e.shown.trim_start_matches("./").to_string())
        .filter(|n| wanted.is_empty() || wanted.iter().any(|t| t.is_empty() || n == t || n.starts_with(&format!("{t}/"))))
        .collect();
    names.sort();
    Ok((names, Some("這裡不是 git repo（或沒有 git），看不到 .gitignore：除了 .git、node_modules、__pycache__ 以外的檔都掃".to_string())))
}

/// 掃內容時要略過的檔：規則檔自己；profile 有 inline 規則時也略過 profile（每條規則都會命中自己）。路徑名照掃。
pub fn self_files(cfg: &Config, rules: &Rules) -> BTreeSet<String> {
    let mut skip: BTreeSet<String> = rules.files.iter().cloned().collect();
    if !cfg.terms.is_empty() {
        skip.insert(PROFILE_REL.to_string());
    }
    skip
}

/// 掃工作目錄：`root` 是專案根目錄，`targets` 是相對 `root` 的檔案或目錄（空的＝全部）。
pub fn scan_tree(root: &Path, cfg: &Config, rules: &Rules, targets: &[String]) -> Result<Report, String> {
    let scope = Scope::new(cfg)?;
    let (names, note) = list_files(root, targets)?;
    let skip = self_files(cfg, rules);
    let mut read = |rel: &str| -> Result<Option<Vec<u8>>, String> {
        if skip.contains(rel) {
            return Ok(None);
        }
        let p = root.join(rel);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => std::fs::read(&p).map(Some).map_err(|e| format!("讀不到 {rel}：{e}")),
            _ => Ok(None), // 不存在（git 裡刪了還沒 commit）、符號連結、目錄
        }
    };
    let mut rep = scan_files(&rules.rules, &scope, &names, &mut read)?;
    rep.notes.extend(rules.notes.iter().cloned());
    rep.notes.extend(note);
    Ok(rep)
}

// ------------------------------------------------------------------ CLI

pub const USAGE: &str = "去客戶化掃描：用專案自己訂的規則（正規式）掃檔案內容與路徑名，擋掉不該出現的字串。
預設不啟用；要不要掃、規則、範圍、時機都在 .claude/teams/methodology.json 的 scrub 設定（見 docs/自訂與擴充.md）。只讀檔，不寫任何東西。

用法：
    awf scrub [路徑…] [--json]
    awf scrub --selftest

  路徑…     只掃這些檔案或目錄（相對目前目錄；要在專案裡）。不給就掃整個專案。scrub.paths／exclude 照樣套用。
  --json    印 JSON（enabled、ok、hits、files、rules、notes）
在 git repo 裡只掃 git 看得到的檔（有追蹤的，以及沒追蹤但沒被 .gitignore 略過的）；二進位檔不掃內容。
命中只印「路徑:行號」與「規則 #N（來源:行號）」，不印命中的字串，也不印規則。
沒啟用：印「未啟用」、exit 0。通過 exit 0；有命中 exit 1；設定或規則有錯、掃不了 exit 2。
";

/// （exit code，stdout，stderr）。
pub type Captured = (i32, String, String);

fn hit_json(rules: &[Rule], h: &Hit) -> Value {
    let r = rules.get(h.rule);
    json!({
        "path": h.path,
        "line": if h.line == 0 { Value::Null } else { json!(h.line) },
        "kind": if h.line == 0 { "path" } else { "content" },
        "rule": h.rule + 1,
        "source": r.map(|r| r.source.clone()),
        "source_line": r.map(|r| r.line),
    })
}

fn dump(v: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into()))
}

/// 指令本體：`root` 是專案根目錄（絕對路徑），`cwd` 是相對路徑的基準。
pub fn run_captured(argv: &[String], root: &Path, cwd: &Path) -> Captured {
    let (mut as_json, mut paths) = (false, Vec::new());
    let mut only_paths = false;
    for a in argv {
        match a.as_str() {
            _ if only_paths => paths.push(a.clone()),
            "-h" | "--help" => return (0, USAGE.to_string(), String::new()),
            "--json" => as_json = true,
            "--" => only_paths = true,
            x if x.starts_with('-') => return (2, String::new(), format!("awf scrub：不認得的參數「{x}」\n\n{USAGE}")),
            _ => paths.push(a.clone()),
        }
    }
    let fail = |msg: String| -> Captured {
        if as_json {
            (2, dump(&json!({"enabled": true, "ok": false, "error": msg})), String::new())
        } else {
            (2, String::new(), format!("awf scrub：{msg}\n"))
        }
    };
    let cfg = match load_config(&root.join(PROFILE_REL)) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return if as_json {
                (0, dump(&json!({"enabled": false})), String::new())
            } else {
                (0, "去客戶化掃描：未啟用（要用的話在 .claude/teams/methodology.json 設 scrub.enabled 與規則，見 docs/自訂與擴充.md）\n".to_string(), String::new())
            };
        }
        Err(e) => return fail(e),
    };
    // 路徑參數：相對目前目錄，換成相對專案根目錄
    let mut targets = Vec::new();
    for p in &paths {
        let full = cwd.join(p);
        if !full.exists() {
            return fail(format!("找不到路徑：{p}"));
        }
        let real = full.canonicalize().unwrap_or(full);
        let base = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        match real.strip_prefix(&base) {
            Ok(rel) => targets.push(rel.to_string_lossy().replace('\\', "/")),
            Err(_) => return fail(format!("路徑不在專案裡：{p}")),
        }
    }
    if targets.iter().any(String::is_empty) {
        targets.clear(); // 給的是專案根目錄＝全部
    }
    let rules = match load_rules(&cfg, root) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let rep = match scan_tree(root, &cfg, &rules, &targets) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let ok = rep.hits.is_empty();
    if as_json {
        let hits: Vec<Value> = rep.hits.iter().map(|h| hit_json(&rules.rules, h)).collect();
        let doc = json!({"enabled": true, "ok": ok, "hits": hits, "files": rep.files, "rules": rules.rules.len(), "notes": rep.notes});
        return (if ok { 0 } else { 1 }, dump(&doc), String::new());
    }
    let mut out = String::new();
    let mut err: String = rep.notes.iter().map(|n| format!("注意：{n}\n")).collect();
    let (content, names): (Vec<&Hit>, Vec<&Hit>) = rep.hits.iter().partition(|h| h.line > 0);
    if !content.is_empty() {
        out.push_str("== 內容命中 ==\n");
        out.extend(content.iter().map(|h| format!("{}\n", hit_label(&rules.rules, h))));
    }
    if !names.is_empty() {
        out.push_str("== 路徑名命中 ==\n");
        out.extend(names.iter().map(|h| format!("{}\n", hit_label(&rules.rules, h))));
    }
    if !ok {
        err.push_str(&format!("去客戶化掃描：不合格，共 {} 處命中（不印命中的字串與規則；照「規則 #N（來源:行號）」回去查）\n", rep.hits.len()));
        return (1, out, err);
    }
    out.push_str(&format!("去客戶化掃描：通過（{} 個檔、{} 條規則）\n", rep.files, rules.rules.len()));
    (0, out, err)
}

/// `awf scrub …`
pub fn main(argv: &[String]) -> i32 {
    if argv.first().map(String::as_str) == Some("--selftest") {
        return selftest();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = PathBuf::from(crate::deliverables::default_root());
    let (code, out, err) = run_captured(argv, &root, &cwd);
    print!("{out}");
    eprint!("{err}");
    code
}

// ------------------------------------------------------------------ selftest

struct T {
    base: PathBuf,
    n: usize,
    fails: Vec<String>,
    total: usize,
}

impl T {
    fn proj(&mut self, scrub: Option<Value>) -> PathBuf {
        self.n += 1;
        let d = self.base.join(format!("p{}", self.n));
        let _ = std::fs::create_dir_all(d.join(".claude/teams"));
        if let Some(s) = scrub {
            let prof = json!({"name": "selftest", "scrub": s});
            let _ = std::fs::write(d.join(PROFILE_REL), dump(&prof));
        }
        d
    }

    fn put(&self, d: &Path, rel: &str, body: &[u8]) {
        let p = d.join(rel);
        if let Some(up) = p.parent() {
            let _ = std::fs::create_dir_all(up);
        }
        let _ = std::fs::write(p, body);
    }

    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        self.total += 1;
        if !ok {
            self.fails.push(format!("{name}：{detail}"));
        }
    }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// 把每個行為走一遍（暫存目錄；有 git 才跑 .gitignore 那一例）。回 exit code。
pub fn selftest() -> i32 {
    let base = std::env::temp_dir().join(format!("awf-scrub-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::create_dir_all(&base);
    let base = base.canonicalize().unwrap_or(base);
    let mut t = T { base: base.clone(), n: 0, fails: Vec::new(), total: 0 };
    // 規則用的字串都是虛構的；輸出裡不能出現它們
    let (secret, code_name) = ("ACME-Orbital", "zephyr-internal");
    let run = |d: &Path, a: &[&str]| run_captured(&args(a), d, d);

    // 1. 沒啟用：沒有 profile、沒有 scrub、enabled 是 false——都印未啟用、exit 0，而且不讀規則檔（壞掉的規則檔也不報錯）
    let d = t.proj(None);
    let (c, o, e) = run(&d, &[]);
    t.check("沒有 profile → 未啟用", c == 0 && o.contains("未啟用") && e.is_empty(), &format!("{c} {o} {e}"));
    let d = t.proj(Some(json!({"enabled": false, "terms": [secret], "terms_files": ["bad.txt"]})));
    t.put(&d, "bad.txt", b"(?a)x\n");
    t.put(&d, "a.md", format!("{secret}\n").as_bytes());
    let (c, o, e) = run(&d, &[]);
    t.check("enabled=false → 未啟用，不讀規則檔、不掃", c == 0 && o.contains("未啟用") && e.is_empty(), &format!("{c} {o} {e}"));
    let (c, o, _) = run(&d, &["--json"]);
    t.check("未啟用的 JSON", c == 0 && serde_json::from_str::<Value>(&o).ok() == Some(json!({"enabled": false})), &o);
    let d = t.proj(None);
    t.put(&d, PROFILE_REL, b"{\"name\": \"x\"}\n");
    t.check("profile 沒有 scrub → 未啟用", run(&d, &[]).0 == 0, "");

    // 2. 命中：內容與路徑名；不印命中的字串與規則
    let d = t.proj(Some(json!({"enabled": true, "terms": [secret], "terms_files": ["local/terms.txt", "missing.txt"]})));
    t.put(&d, "local/terms.txt", format!("# 本機清單\n\n{code_name}\n").as_bytes());
    t.put(&d, "docs/a.md", format!("第一行\n這裡提到 {secret} 的案子\n{code_name} 與 {secret}\n").as_bytes());
    t.put(&d, &format!("src/{code_name}/x.txt"), b"ok\n");
    t.put(&d, &format!("src/{code_name}/y.txt"), b"ok\n");
    t.put(&d, "bin/blob.dat", format!("\0\0{secret}").as_bytes());
    t.put(&d, "clean.md", b"nothing here\n");
    let (c, o, e) = run(&d, &[]);
    let all = format!("{o}{e}");
    t.check("有命中 exit 1", c == 1, &all);
    t.check("內容命中列 路徑:行號 與規則來源", o.contains("docs/a.md:2  規則 #1（scrub.terms 第 1 條）") && o.contains("docs/a.md:3  規則 #1"), &o);
    t.check("路徑名命中：目錄只報一次", o.matches(&"== 路徑名命中 ==".to_string()).count() == 1 && o.matches("規則 #2（local/terms.txt:3）").count() == 1, &o);
    t.check("不印命中的字串與規則", !all.replace(&format!("src/{code_name}/"), "").contains(secret) && !all.replace(&format!("src/{code_name}/"), "").contains(code_name), &all);
    t.check("二進位檔不掃內容", !o.contains("blob.dat"), &o);
    t.check("規則檔與 profile 不掃自己的內容", !o.lines().any(|l| l.starts_with("local/terms.txt:") || l.starts_with(".claude/teams/methodology.json:")), &o);
    t.check("不存在的規則檔：略過並提示", e.contains("missing.txt") && e.contains("略過"), &e);
    let (c, o, _) = run(&d, &["--json"]);
    let j: Value = serde_json::from_str(&o).unwrap_or(Value::Null);
    let first = &j["hits"][0];
    t.check(
        "JSON：命中的欄位",
        c == 1 && j["ok"] == json!(false) && j["rules"] == json!(2) && first["path"] == json!("docs/a.md") && first["line"] == json!(2)
            && first["rule"] == json!(1) && first["source"] == json!(INLINE) && !o.contains(secret),
        &o,
    );
    // 指定路徑：只掃那裡
    let (c, o, _) = run(&d, &["clean.md"]);
    t.check("指定乾淨的檔 → 通過", c == 0 && o.contains("通過（1 個檔、2 條規則）"), &o);
    let (c, o, _) = run(&d, &["docs"]);
    t.check("指定目錄 → 只報那裡的命中", c == 1 && o.contains("docs/a.md:2") && !o.contains("路徑名命中"), &o);
    let (c, _, e) = run(&d, &["nope.md"]);
    t.check("指定的路徑不存在 → exit 2", c == 2 && e.contains("找不到路徑"), &e);
    let (c, _, e) = run(&d, &["--bogus"]);
    t.check("不認得的參數 → exit 2", c == 2 && e.contains("不認得的參數"), &e);

    // 3. 範圍：paths／exclude
    let d = t.proj(Some(json!({"enabled": true, "terms": [secret], "paths": ["docs", "src/**/*.md"], "exclude": ["docs/vendor/**"]})));
    t.put(&d, "docs/a.md", format!("{secret}\n").as_bytes());
    t.put(&d, "docs/vendor/b.md", format!("{secret}\n").as_bytes());
    t.put(&d, "src/x/c.md", format!("{secret}\n").as_bytes());
    t.put(&d, "src/x/c.rs", format!("// {secret}\n").as_bytes());
    t.put(&d, "other/d.md", format!("{secret}\n").as_bytes());
    let (c, o, _) = run(&d, &[]);
    t.check("paths／exclude", c == 1 && o.contains("docs/a.md:1") && o.contains("src/x/c.md:1") && !o.contains("vendor") && !o.contains("c.rs") && !o.contains("other/"), &o);

    // 4. 通過
    let d = t.proj(Some(json!({"enabled": true, "terms": [secret]})));
    t.put(&d, "a.md", b"clean\n");
    let (c, o, e) = run(&d, &[]);
    t.check("沒有命中 → exit 0", c == 0 && o.contains("通過") && !e.contains("不合格"), &format!("{o}{e}"));

    // 5. 設定或規則有錯：exit 2，只報來源與行號
    let d = t.proj(Some(json!({"enabled": true, "terms_files": ["terms.txt"]})));
    t.put(&d, "terms.txt", format!("ok-term\n(?a){secret}\n").as_bytes());
    let (c, o, e) = run(&d, &[]);
    t.check("規則檔編不過 → exit 2、報檔名與行號、不印規則", c == 2 && e.contains("terms.txt 第 2 行") && !format!("{o}{e}").contains(secret), &e);
    let d = t.proj(Some(json!({"enabled": true, "terms": ["fine", format!("(?a){secret}")]})));
    let (c, o, e) = run(&d, &[]);
    t.check("inline 規則編不過 → exit 2、報第幾條、不印規則", c == 2 && e.contains("scrub.terms 第 2 條") && !format!("{o}{e}").contains(secret), &e);
    for (name, bad, want) in [
        ("絕對路徑的規則檔", json!({"enabled": true, "terms_files": ["/etc/terms.txt"]}), "terms_files 第 1 項"),
        ("含 .. 的規則檔", json!({"enabled": true, "terms_files": ["../terms.txt"]}), "terms_files 第 1 項"),
        ("含 .. 的 paths", json!({"enabled": true, "terms": ["x"], "paths": ["a/../../b"]}), "scrub.paths 第 1 項"),
        ("when 寫了做不到的時機", json!({"enabled": true, "terms": ["x"], "when": ["deliver"]}), "scrub.when 只能是 publish"),
        ("enabled 不是布林", json!({"enabled": "yes", "terms": ["x"]}), "scrub.enabled"),
        ("空的規則", json!({"enabled": true, "terms": ["  "]}), "第 1 條是空的"),
    ] {
        let d = t.proj(Some(bad));
        let (c, _, e) = run(&d, &[]);
        t.check(name, c == 2 && e.contains(want), &e);
    }
    let d = t.proj(None);
    t.put(&d, PROFILE_REL, b"{\"scrub\": {\"enabled\": true,}\n");
    let (c, _, e) = run(&d, &[]);
    t.check("profile 不是合法 JSON、提到 scrub → exit 2（不能默默不掃）", c == 2 && e.contains("不是合法 JSON"), &e);
    let d = t.proj(None);
    t.put(&d, PROFILE_REL, b"{\"name\": }\n");
    t.check("profile 不是合法 JSON、沒提到 scrub → 和沒有這個功能時一樣", run(&d, &[]).0 == 0, "");

    // 6. 啟用但沒有規則：提示、exit 0
    let d = t.proj(Some(json!({"enabled": true, "terms_files": ["local.txt"]})));
    t.put(&d, "a.md", b"x\n");
    let (c, _, e) = run(&d, &[]);
    t.check("沒有任何規則 → 提示、exit 0", c == 0 && e.contains("沒有任何規則"), &e);

    // 7. 設定的解析
    let cfg = parse_config(&json!({"scrub": {"enabled": true, "terms": ["a"], "when": ["publish"]}})).ok().flatten();
    t.check("when 含 publish", cfg.as_ref().is_some_and(|c| c.runs_on("publish") && !c.runs_on("deliver")), "");
    t.check("沒有 scrub、null、enabled 沒寫 → None", [json!({}), json!({"scrub": null}), json!({"scrub": {"terms": ["a"]}})].iter().all(|p| parse_config(p) == Ok(None)), "");
    t.check("沒啟用但設定有錯 → 照樣報錯", parse_config(&json!({"scrub": {"enabled": false, "when": ["deliver"]}})).is_err(), "");
    let sc = Scope::new(&Config { paths: vec!["docs".into(), "*.md".into()], exclude: vec!["docs/tmp/**".into()], ..Config::default() });
    t.check(
        "範圍的比對",
        sc.as_ref().is_ok_and(|s| s.covers("docs/a/b.md") && s.covers("README.md") && !s.covers("src/a.md") && !s.covers("docs/tmp/x.md") && !s.covers("src/a.rs")),
        "",
    );

    // 8. 尊重 .gitignore（要有 git）
    let d = t.proj(Some(json!({"enabled": true, "terms": [secret]})));
    t.put(&d, ".gitignore", b"ignored/\n*.log\n");
    t.put(&d, "ignored/a.md", format!("{secret}\n").as_bytes());
    t.put(&d, "run.log", format!("{secret}\n").as_bytes());
    t.put(&d, "kept.md", format!("{secret}\n").as_bytes());
    let has_git = git_out(&d, &["init", "-q"]).is_some_and(|(ok, _, _)| ok);
    if has_git {
        let (c, o, e) = run(&d, &[]);
        t.check(".gitignore 略過的檔不掃、沒追蹤的檔照掃", c == 1 && o.contains("kept.md:1") && !o.contains("ignored/") && !o.contains("run.log") && !e.contains("不是 git repo"), &format!("{o}{e}"));
    } else {
        println!("（沒有 git，略過 .gitignore 那一例）");
        let (c, o, e) = run(&d, &[]);
        t.check("沒有 git：全部掃並說明", c == 1 && o.contains("ignored/a.md:1") && e.contains("看不到 .gitignore"), &format!("{o}{e}"));
    }

    let _ = std::fs::remove_dir_all(&base);
    if t.fails.is_empty() {
        println!("awf scrub selftest：{} 項全過", t.total);
        0
    } else {
        for f in &t.fails {
            println!("FAIL {f}");
        }
        println!("awf scrub selftest：{} 項失敗（共 {} 項）", t.fails.len(), t.total);
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        assert_eq!(selftest(), 0);
    }

    #[test]
    fn terms_text() {
        let rules = parse_terms("# 註解\n\nfoo\n  # 縮排的註解\nbar\\d+\n", "t.txt").unwrap_or_default();
        assert_eq!(rules.iter().map(|r| r.line).collect::<Vec<_>>(), vec![3, 5]);
        assert_eq!(match_name(&rules, "x/bar12"), Ok(Some(1)));
        assert_eq!(match_lines(&rules, "a\nfoo bar1\nbar2").map(|v| v.iter().map(|h| (h.0, h.1)).collect::<Vec<_>>()), Ok(vec![(2, 0), (3, 1)]));
        assert_eq!(parse_terms("ok\n(?a)x\n", "t.txt").err().map(|e| e.0), Some(2));
    }

    #[test]
    fn bad_paths() {
        for p in ["/abs", "../x", "a/../b", "C:/x", "c:\\x", "~/x", "", " "] {
            assert!(bad_path(p), "{p}");
        }
        for p in ["a/b.txt", ".claude/teams/scrub.local.txt", "docs/**/*.md", "a..b"] {
            assert!(!bad_path(p), "{p}");
        }
    }
}
