//! `awf tokens`：設計系統的 token 檢查。由設計系統 skill（`plugin/skills/design-system-template`）原本的兩支腳本
//! `check-tokens.py`（掃寫死的色碼）與 `verify-tokens.py`（tokens.css 與正本一字不差）移植。
//!
//! 參數、輸出文字、exit code 和原本逐字相同（對跑基準 `tests/parity/tokens.expected.json`），只有三處不同：
//!   1. 用法訊息的程式名改成 `awf tokens check`。
//!   2. 原本腳本放在 skill 裡，用自己的位置找 `references/`；現在是單一執行檔，改成 `--ref <目錄>` 或自動找（[`find_ref`]）。
//!   3. 原本直接當掉（Python 的 Traceback，exit 1）的輸入——設定的行段不是數字、正本或 SHA256SUMS 讀不到、
//!      檔案不是 UTF-8——這裡印一行說明，同樣 exit 1。
//!
//! 兩支都只讀檔、不寫任何東西。

use crate::merge_gate::pyutil::{int_of_str, is_py_space, splitlines, strip, universal_newlines};
use crate::pyfmt::repr_str;
use fancy_regex::Regex as FRegex;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const USAGE: &str = "設計系統的 token 檢查：掃寫死的色碼、確認 tokens.css 與設計正本一字不差。只讀檔，不寫任何東西。

用法：
    awf tokens check [--all] <檔案或目錄>...
    awf tokens verify [tokens.css] [正本檔]
    awf tokens --selftest

共同選項：
  --ref <目錄>   設計系統 skill 的 references/ 目錄（放 tokens.css、allowlist.txt、source.conf、SHA256SUMS、正本）。
                 不給時依序找：目前目錄的 references/；專案 .claude/skills/*/references/（剛好只有一個有 source.conf 時）。

check：掃寫死的色碼，不在白名單裡的 hex／rgb()／rgba()／hsl()／oklch() 等一律列出。
  目錄：遞迴掃 css/scss/ts/tsx/js/jsx/mjs/cjs/vue/svelte/html/svg，略過 node_modules、.next、dist、build、
        coverage 與測試檔（*.test.*、*.spec.*、__tests__/）。
  正本本身（SHA-256 與 references/SHA256SUMS 相同）預設只掃 source.conf 的 ALLOW_RANGES；加 --all 才掃整份。
  白名單＝references/tokens.css 的所有色值
         ＋ references/allowlist.txt（一行一個色值）
         ＋ 正本 ALLOW_RANGES 行段出現的色值（references/source.conf 有設 SOURCE 與 ALLOW_RANGES 時）。
    hex 正規化：小寫、3／4 位展開成 6／8 位（#fff ＝ #ffffff）；%23 開頭（SVG data URL）視同 #。
    rgb()／rgba() 正規化：去空白、數字轉成同一寫法、沒有 alpha 視為 1（rgb(0 0 0 / .5) ＝ rgba(0,0,0,0.5)）。
    含 var( 或 calc( 的函式不算色碼，略過。
  不偵測：顏色關鍵字（white、black、red…）、color-mix() 本身（裡面的色碼照樣會被掃到）。
  結束碼：0＝沒有違規；1＝有違規；2＝用法錯誤。

verify：逐一確認 references/tokens.css 的每個「變數: 值」都能在正本找到原字串（一字不差）。
  預設讀 references/tokens.css，正本檔名取自 references/source.conf 的 SOURCE。
  全部找得到、正本 SHA-256 與 SHA256SUMS 相符、（有設 TOKEN_RANGE 時）第 1 部分與正本該段整段相同，才結束碼 0。
  沒有 HTML 正本（SOURCE 留空）時，只檢查 tokens.css 沒有重複宣告，並提示改用人工比對。
";

/// （exit code，stdout，stderr）。
pub type Captured = (i32, String, String);

// ------------------------------------------------------------------ Python 語意的小工具

/// `str(Path(s))`：連續的斜線收成一個、拿掉 `.` 與結尾的斜線；空字串是 `.`。
fn py_path(s: &str) -> String {
    let root = if s.starts_with("//") && !s.starts_with("///") {
        "//"
    } else if s.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() && root.is_empty() {
        return ".".to_string();
    }
    format!("{root}{}", parts.join("/"))
}

/// `Path(s).name`。
fn py_name(s: &str) -> String {
    py_path(s).rsplit('/').next().unwrap_or_default().to_string()
}

/// `path.read_text(encoding="utf-8")`；失敗回說明（Python 版在這裡當掉）。
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} 不是 UTF-8 文字", path.display()))?;
    Ok(universal_newlines(&text))
}

/// `int(s)`；失敗回說明。
fn py_int(s: &str, what: &str) -> Result<i64, String> {
    match int_of_str(s) {
        Some(n) => Ok(n.clamp(i64::MIN as i128 / 4, i64::MAX as i128 / 4) as i64),
        None => Err(format!("source.conf 的 {what} 有不是整數的值：{}", repr_str(s))),
    }
}

/// `float(s)`：正負號、小數、指數、inf／nan、數字間的單一底線。只收 ASCII 數字。
fn py_float(s: &str) -> Option<f64> {
    let t = strip(s);
    if !t.contains('_') {
        return t.parse().ok();
    }
    let chars: Vec<char> = t.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            let digit = |j: Option<usize>| j.and_then(|j| chars.get(j)).is_some_and(char::is_ascii_digit);
            if !digit(i.checked_sub(1)) || !digit(Some(i + 1)) {
                return None;
            }
        }
    }
    chars.iter().filter(|c| **c != '_').collect::<String>().parse().ok()
}

/// 拿掉小數結尾的 0（與落單的小數點）。
fn trim_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

/// `f"{x:g}"`：六位有效數字、去掉結尾的 0；指數小於 -4 或不小於 6 時用科學記號。
fn py_g(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let sci = format!("{f:.5e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        return format!("{}e{}{:02}", trim_zeros(mant), if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let fixed = format!("{f:.*}", (5 - exp) as usize);
    trim_zeros(&fixed).to_string()
}

/// Python 的切片 `items[start:stop]`（負數從尾端算、超出就夾住）。
fn py_slice<'a, T>(items: &'a [T], start: i64, stop: i64) -> &'a [T] {
    let len = items.len() as i64;
    let fix = |i: i64| if i < 0 { (i + len).max(0) } else { i.min(len) } as usize;
    let (a, b) = (fix(start), fix(stop));
    if a < b { &items[a..b] } else { &[] }
}

// ------------------------------------------------------------------ 設定

/// `references/source.conf`：`鍵=值`，`#` 開頭是註解。沒有這個檔就是空的。
fn read_conf(refdir: &Path) -> Result<HashMap<String, String>, String> {
    let mut c = HashMap::new();
    let p = refdir.join("source.conf");
    if p.exists() {
        for line in splitlines(&read_text(&p)?) {
            let line = strip(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                c.insert(strip(k).to_string(), strip(v).to_string());
            }
        }
    }
    Ok(c)
}

/// `ALLOW_RANGES` 的行段（1 起算、含頭尾）轉成 0 起算的半開區間；空的區間不留。
fn ranges(spec: &str) -> Result<Vec<(i64, i64)>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(strip).filter(|s| !s.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, ""));
        let lo = py_int(a, "ALLOW_RANGES")? - 1;
        let hi = py_int(if b.is_empty() { a } else { b }, "ALLOW_RANGES")?;
        if lo < hi {
            out.push((lo, hi));
        }
    }
    Ok(out)
}

/// 照 Python 的索引規則走過行段裡「存在的行」：超出行數的略過，負數從尾端算；負到超出範圍時 Python 版當掉。
fn each_line<'a>(lines: &[&'a str], rng: &[(i64, i64)], mut f: impl FnMut(usize, &'a str)) -> Result<(), String> {
    let len = lines.len() as i64;
    for &(lo, hi) in rng {
        if lo < -len {
            return Err(format!("行段的起點超出範圍（第 {} 行）", lo + 1));
        }
        for i in lo..hi.min(len) {
            let idx = if i < 0 { i + len } else { i } as usize;
            f(idx, lines[idx]);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ 色碼

fn hex_re() -> &'static FRegex {
    static R: OnceLock<FRegex> = OnceLock::new();
    R.get_or_init(|| FRegex::new(r"(?<![\w&])(?:#|%23)([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![0-9a-zA-Z_-])").expect("HEX"))
}

fn func_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(([^()]*)\)").expect("FUNC"))
}

fn norm_hex(h: &str) -> String {
    let h = h.to_lowercase();
    if matches!(h.chars().count(), 3 | 4) {
        return format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>());
    }
    format!("#{h}")
}

fn num(s: &str) -> Option<String> {
    let s = strip(s);
    match s.strip_suffix('%') {
        Some(p) => Some(format!("{}%", py_g(py_float(p)?))),
        None => Some(py_g(py_float(s)?)),
    }
}

fn norm_func(name: &str, inner: &str) -> Option<String> {
    let name = name.to_lowercase();
    if inner.contains("var(") || inner.contains("calc(") {
        return None;
    }
    let mut vals = strip(inner).split(|c: char| is_py_space(c) || c == ',' || c == '/').filter(|p| !p.is_empty()).map(num).collect::<Option<Vec<String>>>()?;
    if name == "rgb" || name == "rgba" {
        if vals.len() == 3 {
            vals.push("1".to_string());
        }
        return Some(format!("rgba({})", vals.join(",")));
    }
    Some(format!("{name}({})", vals.join(",")))
}

/// 一行裡的色碼：（原字串，正規化後的值）。先 hex、再函式寫法，各自照出現順序。
fn colors(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in hex_re().captures_iter(line).flatten() {
        if let (Some(all), Some(h)) = (m.get(0), m.get(1)) {
            out.push((all.as_str().to_string(), norm_hex(h.as_str())));
        }
    }
    for m in func_re().captures_iter(line) {
        if let Some(n) = norm_func(&m[1], &m[2]) {
            out.push((m[0].to_string(), n));
        }
    }
    out
}

/// 找出 `/* … */` 註解的位元組範圍（沒關的註解不算）。
fn comment_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("/*") {
        let start = at + i;
        let Some(j) = text[start + 2..].find("*/") else { break };
        let end = start + 2 + j + 2;
        out.push((start, end));
        at = end;
    }
    out
}

/// 去掉註解；`keep_len`＝換成同樣字數的空白（保留位置）。
fn strip_comments(text: &str, keep_len: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (a, b) in comment_spans(text) {
        out.push_str(&text[at..a]);
        if keep_len {
            out.extend(std::iter::repeat_n(' ', text[a..b].chars().count()));
        }
        at = b;
    }
    out.push_str(&text[at..]);
    out
}

// ------------------------------------------------------------------ check

const EXT: &[&str] = &["css", "scss", "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "html", "htm", "svg"];
const SKIP_DIRS: &[&str] = &["node_modules", ".next", "dist", "build", "coverage", "__tests__", ".turbo", "out"];

/// `Path(name).suffix in EXT`（檔名開頭的點不算副檔名）。
fn wanted_ext(name: &str) -> bool {
    match name.rfind('.') {
        Some(i) if i > 0 => EXT.contains(&&name[i + 1..]),
        _ => false,
    }
}

fn is_test_file(name: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\.(test|spec)\.[a-z]+$").expect("TEST")).is_match(name)
}

/// `os.walk`：先這一層的檔（照名字排），再往下走各子目錄（照名字排）；符號連結的目錄不往下走。讀不到的目錄略過。
fn walk(shown: &str, real: &Path, out: &mut Vec<(String, PathBuf)>) {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(real).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()) { dirs.push(name) } else { files.push(name) }
    }
    dirs.retain(|d| !SKIP_DIRS.contains(&d.as_str()));
    dirs.sort();
    files.sort();
    for f in files {
        if wanted_ext(&f) && !is_test_file(&f) {
            out.push((py_path(&format!("{shown}/{f}")), real.join(&f)));
        }
    }
    for d in dirs {
        let sub = real.join(&d);
        if !std::fs::symlink_metadata(&sub).is_ok_and(|m| m.file_type().is_symlink()) {
            walk(&format!("{shown}/{d}"), &sub, out);
        }
    }
}

/// `awf tokens check`：`argv` 是 `[--all] <檔案或目錄>...`；相對路徑以 `cwd` 為準。
pub fn run_check(argv: &[String], refdir: &Path, cwd: &Path) -> Captured {
    let paths: Vec<&String> = argv.iter().filter(|a| *a != "--all").collect();
    if paths.is_empty() {
        return (2, String::new(), "用法：awf tokens check [--all] <檔案或目錄>...\n".into());
    }
    let scan_all = argv.iter().any(|a| a == "--all");
    let (mut out, mut err) = (String::new(), String::new());
    match check_body(&paths, scan_all, refdir, cwd, &mut out, &mut err) {
        Ok(code) => (code, out, err),
        Err(e) => {
            err.push_str(&format!("awf tokens check：{e}\n"));
            (1, out, err)
        }
    }
}

fn check_body(paths: &[&String], scan_all: bool, refdir: &Path, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    let conf = read_conf(refdir)?;
    let mut allow: HashSet<String> = HashSet::new();
    let tokens = refdir.join("tokens.css");
    if tokens.exists() {
        let body = strip_comments(&read_text(&tokens)?, false); // 註解裡的範例不算
        for line in splitlines(&body) {
            allow.extend(colors(line).into_iter().map(|(_, n)| n));
        }
    }
    let al = refdir.join("allowlist.txt");
    if al.exists() {
        for line in splitlines(&read_text(&al)?) {
            // 「# 」開頭是註解；#abc 這種色值保留
            let chars: Vec<(usize, char)> = line.char_indices().collect();
            let cut = chars.iter().enumerate().find(|(k, (_, c))| {
                *c == '#' && (*k == 0 || is_py_space(chars[k - 1].1)) && chars.get(k + 1).is_some_and(|(_, n)| is_py_space(*n))
            });
            let line = strip(cut.map_or(line, |(_, (i, _))| &line[..*i]));
            if let Some(first) = line.split(is_py_space).find(|s| !s.is_empty()) {
                allow.extend(colors(first).into_iter().map(|(_, n)| n));
            }
        }
    }
    let (mut ds_sha, mut ds_rng): (Option<String>, Vec<(i64, i64)>) = (None, Vec::new());
    let src = conf.get("SOURCE").map(String::as_str).unwrap_or_default();
    if !src.is_empty() {
        let ds_text = read_text(&refdir.join(src))?;
        let ds_lines = splitlines(&ds_text);
        ds_rng = ranges(conf.get("ALLOW_RANGES").map(String::as_str).unwrap_or_default())?;
        each_line(&ds_lines, &ds_rng, |_, line| allow.extend(colors(line).into_iter().map(|(_, n)| n)))?;
        let sums = refdir.join("SHA256SUMS");
        if sums.exists() {
            let text = read_text(&sums)?;
            let first = text.split(is_py_space).find(|s| !s.is_empty()).ok_or_else(|| format!("{} 是空的", sums.display()))?;
            ds_sha = Some(first.to_string());
        }
    }
    if allow.is_empty() {
        err.push_str("警告：白名單是空的（tokens.css 還是骨架？），所有色碼都會列為違規\n");
    }

    let mut viol: Vec<(String, usize, String, String)> = Vec::new();
    let mut scanned = 0;
    for p in paths {
        let shown = py_path(p);
        let real = cwd.join(p.as_str());
        if !real.exists() {
            err.push_str(&format!("找不到：{shown}\n"));
            return Ok(2);
        }
        let mut found = Vec::new();
        if real.is_file() { found.push((shown, real)) } else { walk(&shown, &real, &mut found) }
        for (fp, real) in found {
            let data = std::fs::read(&real).map_err(|e| format!("讀不到 {fp}：{e}"))?;
            let Ok(text) = std::str::from_utf8(&data) else { continue };
            let lines = splitlines(text);
            let digest = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect::<String>();
            let source_only = !scan_all && !ds_rng.is_empty() && ds_sha.as_deref() == Some(digest.as_str());
            let whole = [(0, lines.len() as i64)];
            if source_only {
                out.push_str(&format!("[正本] {fp}：只掃 ALLOW_RANGES；--all 掃整份\n"));
            }
            scanned += 1;
            each_line(&lines, if source_only { &ds_rng } else { &whole }, |i, line| {
                for (raw, n) in colors(line) {
                    if !allow.contains(&n) {
                        viol.push((fp.clone(), i + 1, raw, n));
                    }
                }
            })?;
        }
    }
    for (fp, ln, raw, n) in &viol {
        let same = *n == raw.to_lowercase();
        out.push_str(&format!("{fp}:{ln}: {raw}{}\n", if same { String::new() } else { format!("  (= {n})") }));
    }
    let uniq = viol.iter().map(|v| v.3.as_str()).collect::<HashSet<_>>().len();
    out.push_str(&format!("白名單 {} 個色值；掃描 {scanned} 個檔；違規 {} 處（{uniq} 種色值）\n", allow.len(), viol.len()));
    Ok(if viol.is_empty() { 0 } else { 1 })
}

// ------------------------------------------------------------------ verify

/// `awf tokens verify`：`argv` 是 `[tokens.css] [正本檔]`；tokens.css 的相對路徑以 `cwd` 為準，正本檔的相對路徑以 `refdir` 為準。
pub fn run_verify(argv: &[String], refdir: &Path, cwd: &Path) -> Captured {
    let mut out = String::new();
    match verify_body(argv, refdir, cwd, &mut out) {
        Ok(code) => (code, out, String::new()),
        Err(e) => (1, out, format!("awf tokens verify：{e}\n")),
    }
}

fn verify_body(argv: &[String], refdir: &Path, cwd: &Path, out: &mut String) -> Result<i32, String> {
    let conf = read_conf(refdir)?;
    let tokens_path = argv.first().map_or_else(|| refdir.join("tokens.css"), |a| cwd.join(a));
    let src = argv.get(1).map(String::as_str).unwrap_or_else(|| conf.get("SOURCE").map(String::as_str).unwrap_or_default());
    let css = read_text(&tokens_path)?;
    let body = strip_comments(&css, true); // 去註解、保留位置
    static DECL: OnceLock<FRegex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    let decl_re = DECL.get_or_init(|| FRegex::new(r"(--[a-zA-Z0-9-]+)\s*:\s*[^;{}]+(?=;)").expect("DECL"));
    let name_re = NAME.get_or_init(|| Regex::new(r"(--[a-zA-Z0-9-]+)\s*:").expect("NAME"));
    let decls: Vec<&str> = decl_re.find_iter(&body).flatten().map(|m| m.as_str()).collect();
    let names = |text: &str| -> Vec<String> { name_re.captures_iter(text).map(|c| c[1].to_string()).collect() };
    // 「第 2 部分」在原文的位置（以字元計），換算成去註解後的位元組位置
    let cut = css.find("第 2 部分").map(|b| css[..b].chars().count()).map(|n| body.char_indices().nth(n).map_or(body.len(), |(i, _)| i));
    let names1 = names(cut.map_or(body.as_str(), |c| &body[..c]));
    let part2 = cut.map(|c| names(&body[c..])).unwrap_or_default();

    if decls.is_empty() {
        out.push_str("tokens.css 還沒有任何宣告（仍是骨架）：先照 SKILL.md 從正本擷取\n");
        return Ok(1);
    }
    let uniq1: BTreeSet<&str> = names1.iter().map(String::as_str).collect();
    if src.is_empty() {
        let dup: Vec<String> = uniq1.iter().filter(|n| names1.iter().filter(|x| x == *n).count() > 1).map(|n| repr_str(n)).collect();
        out.push_str(&format!("沒有設定正本（source.conf 的 SOURCE 是空的）：宣告 {} 個；同名重複 {} 個 [{}]\n", decls.len(), dup.len(), dup.join(", ")));
        out.push_str("請人工對照設計正本；能取得 HTML／CSS 正本時，填 source.conf 再跑一次\n");
        return Ok(0);
    }

    let ds_path = if src.starts_with('/') || Path::new(src).is_absolute() { PathBuf::from(src) } else { refdir.join(src) };
    let mut errs: Vec<String> = Vec::new();
    let sums_text = read_text(&refdir.join("SHA256SUMS"))?;
    let sum = sums_text.split(is_py_space).find(|s| !s.is_empty());
    let data = std::fs::read(&ds_path).map_err(|e| format!("讀不到 {}：{e}", ds_path.display()))?;
    let digest = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect::<String>();
    if sum != Some(digest.as_str()) {
        errs.push(format!("正本 SHA-256 不符：SHA256SUMS={}，實際={digest}", sum.unwrap_or("(空)")));
    }
    let ds = universal_newlines(&String::from_utf8(data).map_err(|_| format!("{} 不是 UTF-8 文字", ds_path.display()))?);
    let missing: Vec<&str> = decls.iter().copied().filter(|raw| !ds.contains(raw)).collect();
    errs.extend(missing.iter().map(|raw| format!("正本找不到：{raw}")));

    let rng = conf.get("TOKEN_RANGE").map(String::as_str).unwrap_or_default();
    let mut same = None;
    if !rng.is_empty() {
        let (a, b) = rng.split_once('-').unwrap_or((rng, ""));
        let lo = py_int(a, "TOKEN_RANGE")? - 1;
        let hi = py_int(if b.is_empty() { a } else { b }, "TOKEN_RANGE")?;
        let block = py_slice(&splitlines(&ds), lo, hi).join("\n");
        let ok = css.contains(&block);
        same = Some(ok);
        if !ok {
            errs.push(format!("tokens.css 第 1 部分與正本 DS:{rng} 不是整段相同"));
        }
    }

    out.push_str(&format!("正本：{}  SHA-256 {digest}\n", py_name(&ds_path.to_string_lossy())));
    out.push_str(&format!("宣告總數 {}；找得到 {}；找不到 {}\n", decls.len(), decls.len() - missing.len(), missing.len()));
    out.push_str(&format!("第 1 部分：{} 個宣告＝{} 個變數＋同名覆寫 {} 個\n", names1.len(), uniq1.len(), names1.len() - uniq1.len()));
    out.push_str(&format!("第 2 部分（別名，正本 :root 外）：{} 個{}\n", part2.len(), if part2.is_empty() { String::new() } else { format!("：{}", part2.join(", ")) }));
    if let Some(ok) = same {
        out.push_str(&format!("DS:{rng} 整段比對：{}\n", if ok { "相同" } else { "不同" }));
    }
    if !errs.is_empty() {
        out.push_str(&errs.join("\n"));
        out.push('\n');
        return Ok(1);
    }
    out.push_str("結果：全部找得到\n");
    Ok(0)
}

// ------------------------------------------------------------------ 入口

/// 沒給 `--ref` 時找 `references/`：目前目錄的 `references/`（在 skill 目錄裡跑）；
/// 不然看專案的 `.claude/skills/*/references/`，剛好只有一個有 `source.conf` 才用（多個就要人指定，不猜）。
pub fn find_ref(cwd: &Path, project: &Path) -> Result<PathBuf, String> {
    let here = cwd.join("references");
    if here.join("source.conf").is_file() || here.join("tokens.css").is_file() {
        return Ok(here);
    }
    let skills = project.join(".claude").join("skills");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&skills).into_iter().flatten().flatten()
        .map(|e| e.path().join("references"))
        .filter(|r| r.join("source.conf").is_file())
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "找不到設計系統 skill 的 references/（目前目錄沒有 references/，{} 底下也沒有帶 references/source.conf 的 skill）；用 --ref <目錄> 指定",
            skills.display()
        )),
        _ => Err(format!(
            "有多個設計系統 skill（{}）；用 --ref <目錄> 指定要用哪一個的 references/",
            found.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
        )),
    }
}

/// 從參數裡拿掉 `--ref <目錄>`／`--ref=<目錄>`。
fn take_ref(argv: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let (mut value, mut rest) = (None, Vec::new());
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == "--ref" {
            value = Some(it.next().ok_or("--ref 後面要接 references/ 目錄")?.clone());
        } else if let Some(v) = a.strip_prefix("--ref=") {
            value = Some(v.to_string());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((value, rest))
}

/// `awf tokens …`（不含 `--selftest`）：`cwd` 是目前目錄，`project` 是專案根目錄。
pub fn run_captured(argv: &[String], cwd: &Path, project: &Path) -> Captured {
    let (given, rest) = match take_ref(argv) {
        Ok(x) => x,
        Err(e) => return (2, String::new(), format!("awf tokens：{e}\n")),
    };
    let sub = rest.first().map(String::as_str).unwrap_or_default();
    match sub {
        "-h" | "--help" => return (0, USAGE.to_string(), String::new()),
        "check" | "verify" => {}
        "" => return (2, String::new(), USAGE.to_string()),
        other => return (2, String::new(), format!("awf tokens：不認得的子指令「{other}」\n\n{USAGE}")),
    }
    let args = &rest[1..];
    if sub == "check" && !args.iter().any(|a| a != "--all") {
        return run_check(args, Path::new(""), cwd); // 用法錯誤，不必先找 references/
    }
    let refdir = match given {
        Some(r) => cwd.join(r),
        None => match find_ref(cwd, project) {
            Ok(r) => r,
            Err(e) => return (2, String::new(), format!("awf tokens {sub}：{e}\n")),
        },
    };
    if sub == "check" { run_check(args, &refdir, cwd) } else { run_verify(args, &refdir, cwd) }
}

/// `awf tokens …`
pub fn main(argv: &[String]) -> i32 {
    if argv.first().is_some_and(|a| a == "--selftest") {
        return selftest();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project = PathBuf::from(crate::project::root_or_cwd(None));
    let (code, out, err) = run_captured(argv, &cwd, &project);
    print!("{out}");
    eprint!("{err}");
    code
}

// ------------------------------------------------------------------ selftest

/// `awf tokens --selftest`。前兩項是原本 repo 檢查裡「check-tokens 掃描器」那一步的兩項（名稱與順序相同），後面是補的。
pub fn selftest() -> i32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("awf-tokens-{}-{nanos}", std::process::id()));
    let put = |rel: &str, text: &str| {
        let p = tmp.join(rel);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, text);
    };
    let ds = "<style>\n:root {\n  --c-primary: #0A5FFF;\n  --c-shadow: rgba(0, 0, 0, .5);\n}\n</style>\n";
    let sha = Sha256::digest(ds.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
    put("empty/tokens.css", ":root {\n  /* --c: #123456; */\n}\n");
    put("ref/tokens.css", ":root {\n  --c-primary: #0A5FFF;\n  --c-shadow: rgba(0, 0, 0, .5);\n}\n");
    put("ref/ds.html", ds);
    put("ref/SHA256SUMS", &format!("{sha}  ds.html\n"));
    put("ref/source.conf", "SOURCE=ds.html\nALLOW_RANGES=\nTOKEN_RANGE=3-4\n");
    put("bad/tokens.css", ":root {\n  --c-primary: #0A5FFE;\n}\n");
    put("bad/ds.html", ds);
    put("bad/SHA256SUMS", &format!("{sha}  ds.html\n"));
    put("bad/source.conf", "SOURCE=ds.html\n");
    put("proj/hard.css", "a { color: #123456; }\n");
    put("proj/ok.css", "a { color: var(--fg); }\n");
    put("proj/same.css", "a { color: #0a5fff; box-shadow: 0 0 0 rgb(0 0 0 / 0.5); }\n");
    put("proj/src/a.css", "a { color: #0a5fff; }\n");
    put("proj/src/a.test.ts", "const c = '#123456';\n");
    put("proj/src/node_modules/m/i.js", "const c = '#123456';\n");
    let s = |x: &str| x.to_string();
    let proj = tmp.join("proj");
    let check = |refdir: &str, args: &[&str]| run_check(&args.iter().map(|a| s(a)).collect::<Vec<_>>(), &tmp.join(refdir), &proj);
    let cases: Vec<(&str, i32, Captured)> = vec![
        ("寫死色碼要被抓", 1, check("empty", &["hard.css"])),
        ("var() 要放行", 0, check("empty", &["ok.css"])),
        ("白名單裡的色值要放行（寫法不同也算同一個）", 0, check("ref", &["same.css"])),
        ("不在白名單的色值要被抓", 1, check("ref", &["hard.css"])),
        ("掃目錄時略過測試檔與 node_modules", 0, check("ref", &["src"])),
        ("沒給路徑是用法錯誤", 2, check("ref", &["--all"])),
        ("找不到路徑是用法錯誤", 2, check("ref", &["nope"])),
        ("verify：tokens.css 與正本一字不差", 0, run_verify(&[], &tmp.join("ref"), &proj)),
        ("verify：正本找不到的宣告要失敗", 1, run_verify(&[], &tmp.join("bad"), &proj)),
        ("verify：還是骨架要失敗", 1, run_verify(&[], &tmp.join("empty"), &proj)),
    ];
    println!("tokens selftest");
    let mut ok = true;
    for (what, want, (got, out, err)) in cases {
        let pass = got == want;
        println!("  [{}] {what}（want={want} got={got}）", if pass { "通過" } else { "失敗" });
        if !pass {
            ok = false;
            print!("{out}{err}");
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    println!("tokens selftest：{}", if ok { "全部通過" } else { "有失敗" });
    if ok { 0 } else { 1 }
}
