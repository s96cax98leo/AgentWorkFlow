//! 知識層用到的 Python 標準庫行為：空白判斷、路徑運算、`json.loads` 的錯誤訊息、例外的類別名與文字。
//!
//! 這些是 kb 模組私有的最小實作（原本的 kb.py 直接用標準庫）。輸出要和 0.x 的 Python 版逐字相同，
//! 所以連錯誤訊息的寫法都照 CPython 3.12。之後整併到共用模組時，這個檔可以整個搬走。

use crate::pyfmt;
use serde_json::{Map, Number, Value};
use std::collections::HashMap;

// ------------------------------------------------------------------ 例外
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    /// 可以直接印給人看的錯誤（KbError）。
    Kb,
    /// SQLite 的錯誤（sqlite3.Error 的子類別）。
    Sqlite,
    /// 既有索引不能沿用，整份重建（_Rebuild）。
    Rebuild,
    /// 其他例外（OSError、TypeError…）。
    Other,
}

/// 對應 Python 的一個例外：類別名與 `str(e)`。
#[derive(Debug, Clone)]
pub struct PyErr {
    pub kind: ErrKind,
    pub class: String,
    pub msg: String,
}

impl PyErr {
    pub fn kb(msg: impl Into<String>) -> Self {
        PyErr { kind: ErrKind::Kb, class: "KbError".into(), msg: msg.into() }
    }
    pub fn rebuild(msg: &str) -> Self {
        PyErr { kind: ErrKind::Rebuild, class: "_Rebuild".into(), msg: msg.into() }
    }
    pub fn other(class: &str, msg: impl Into<String>) -> Self {
        PyErr { kind: ErrKind::Other, class: class.into(), msg: msg.into() }
    }
    /// OSError：`[Errno N] 說明: '路徑'`，類別名依錯誤種類（FileNotFoundError、PermissionError…）。
    pub fn os(e: &std::io::Error, path: &str) -> Self {
        Self::os2(e, path, None)
    }
    pub fn os2(e: &std::io::Error, path: &str, dst: Option<&str>) -> Self {
        let class = os_class(e);
        let text = e.to_string();
        let text = text.rfind(" (os error ").and_then(|i| text.get(..i)).unwrap_or(&text).to_string();
        let mut msg = match e.raw_os_error() {
            Some(n) => format!("[Errno {n}] {text}: {}", pyfmt::repr_str(path)),
            None => text,
        };
        if let Some(d) = dst {
            msg.push_str(&format!(" -> {}", pyfmt::repr_str(d)));
        }
        PyErr { kind: ErrKind::Other, class: class.into(), msg }
    }
}

/// OSError 的子類別名（`e.__class__.__name__`）。
pub fn os_class(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "FileNotFoundError",
        K::PermissionDenied => "PermissionError",
        K::AlreadyExists => "FileExistsError",
        K::Interrupted => "InterruptedError",
        K::TimedOut => "TimeoutError",
        K::BrokenPipe => "BrokenPipeError",
        _ => match e.raw_os_error() {
            Some(20) if cfg!(unix) => "NotADirectoryError",
            Some(21) if cfg!(unix) => "IsADirectoryError",
            _ => "OSError",
        },
    }
}

impl From<rusqlite::Error> for PyErr {
    fn from(e: rusqlite::Error) -> Self {
        let (class, msg) = match &e {
            rusqlite::Error::SqliteFailure(f, m) => {
                let class = match f.extended_code & 0xff {
                    2 | 12 => "InternalError",
                    7 => "MemoryError",
                    1 | 3..=6 | 8..=10 | 13..=17 => "OperationalError",
                    18 => "DataError",
                    19 | 20 => "IntegrityError",
                    21 | 25 => "InterfaceError",
                    _ => "DatabaseError",
                };
                (class, m.clone().unwrap_or_else(|| f.to_string()))
            }
            other => ("DatabaseError", other.to_string()),
        };
        PyErr { kind: ErrKind::Sqlite, class: class.into(), msg }
    }
}

// ------------------------------------------------------------------ 字串
/// Python 的 `str.isspace()`（單一字元）。比 Unicode 的 White_Space 多了 U+001C～U+001F。
pub fn is_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// 放進正規式字元類別裡的空白（對應 Python `re` 的 `\s`）。
pub const WS_CLASS: &str = r"\t-\r\x1c-\x1f \x{85}\x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}";

/// `s.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `not s.strip()`
pub fn is_blank(s: &str) -> bool {
    s.chars().all(is_space)
}

/// `re.sub(r"\s+", " ", s)`
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if is_space(c) {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// `s[:n]`（以 code point 計）。
pub fn take(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

/// 近似 Python 的 `str.casefold()`。
///
/// 標準庫沒有 case folding 的表，這裡用「轉大寫再轉小寫」做兩輪（ß→ss、ς→σ、ẞ→ss 都對），
/// 另外處理兩個例外：無點的 ı 不變；切羅基字母折成大寫。和 Unicode 的 CaseFolding 表在常見文字上一致。
pub fn casefold(s: &str) -> String {
    fn step(c: char, out: &mut String) {
        let u = c as u32;
        if c == 'ı' {
            out.push(c);
        } else if (0x13a0..=0x13fd).contains(&u) || (0xab70..=0xabbf).contains(&u) {
            out.extend(c.to_uppercase());
        } else {
            for up in c.to_uppercase() {
                out.extend(up.to_lowercase());
            }
        }
    }
    let mut once = String::with_capacity(s.len());
    s.chars().for_each(|c| step(c, &mut once));
    let mut twice = String::with_capacity(once.len());
    once.chars().for_each(|c| step(c, &mut twice));
    twice
}

/// `urllib.parse.unquote(s)`：%XX 解成位元組，再照 UTF-8 解碼（壞的位元組換成 U+FFFD）。
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }
    let mut out = String::with_capacity(s.len());
    let mut run: Vec<u8> = Vec::new();
    let flush = |run: &mut Vec<u8>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(run.len());
        let mut i = 0;
        while i < run.len() {
            let b = run.get(i).copied().unwrap_or(0);
            if b == b'%' {
                if let (Some(h), Some(l)) = (run.get(i + 1).copied().and_then(hex), run.get(i + 2).copied().and_then(hex)) {
                    bytes.push(h * 16 + l);
                    i += 3;
                    continue;
                }
            }
            bytes.push(b);
            i += 1;
        }
        out.push_str(&String::from_utf8_lossy(&bytes));
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii() {
            run.push(c as u8);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `round(x, nd)`：依十進位的正確值四捨六入五成雙。
pub fn round_nd(x: f64, nd: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.nd$}").parse().unwrap_or(x)
}

/// `time.strftime("%Y-%m-%d %H:%M", time.gmtime(秒))`
pub fn utc_minute(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // 公曆換算（Howard Hinnant 的 civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

// ------------------------------------------------------------------ 路徑（一律用 / 的字串）
pub fn is_abs(p: &str) -> bool {
    if p.starts_with('/') {
        return true;
    }
    if cfg!(windows) {
        let b = p.as_bytes();
        return p.starts_with('\\') || (b.len() >= 3 && b.get(1) == Some(&b':') && matches!(b.get(2), Some(b'/') | Some(b'\\')));
    }
    false
}

/// `str(pathlib.Path(p))`：去掉多餘的斜線與 `.`，不動 `..`。
pub fn ppath(p: &str) -> String {
    let lead = if p.starts_with('/') { "/" } else { "" };
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    if parts.is_empty() {
        return if lead.is_empty() { ".".into() } else { "/".into() };
    }
    format!("{lead}{}", parts.join("/"))
}

/// `str(Path(a) / b)`：b 是絕對路徑時取代 a。
pub fn pjoin(a: &str, b: &str) -> String {
    if is_abs(b) {
        return ppath(b);
    }
    ppath(&format!("{a}/{b}"))
}

/// `os.path.normpath(p)`（posix）
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let initial = if p.starts_with("//") && !p.starts_with("///") { 2 } else if p.starts_with('/') { 1 } else { 0 };
    let mut comps: Vec<&str> = Vec::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(c);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let out = format!("{}{}", "/".repeat(initial), comps.join("/"));
    if out.is_empty() { ".".into() } else { out }
}

fn cwd() -> String {
    std::env::current_dir().ok().and_then(|p| p.to_str().map(|s| s.replace('\\', "/"))).unwrap_or_else(|| "/".into())
}

/// `os.path.abspath(p)`
pub fn abspath(p: &str) -> String {
    if is_abs(p) { normpath(p) } else { normpath(&format!("{}/{p}", cwd())) }
}

/// `os.path.relpath(path, start)`
pub fn relpath(path: &str, start: &str) -> String {
    let (p, s) = (abspath(path), abspath(start));
    let pl: Vec<&str> = p.split('/').filter(|x| !x.is_empty()).collect();
    let sl: Vec<&str> = s.split('/').filter(|x| !x.is_empty()).collect();
    let i = pl.iter().zip(sl.iter()).take_while(|(a, b)| a == b).count();
    let mut rel: Vec<&str> = vec![".."; sl.len() - i];
    rel.extend(pl.iter().skip(i));
    if rel.is_empty() { ".".into() } else { rel.join("/") }
}

/// `os.path.commonpath([p, root]) == root`（兩個都是正規化過的絕對路徑）。
pub fn is_under(p: &str, root: &str) -> bool {
    p == root || root == "/" || p.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// `os.path.splitext(p)[0]`：最後一段開頭的點不算副檔名。
pub fn splitext_stem(p: &str) -> &str {
    let base_at = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = p.get(base_at..).unwrap_or("");
    match base.rfind('.') {
        Some(dot) if !base.get(..dot).unwrap_or("").chars().all(|c| c == '.') => p.get(..base_at + dot).unwrap_or(p),
        _ => p,
    }
}

/// `Path(p).name`
pub fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn split_head(path: &str) -> (String, String) {
    // posixpath.split
    let i = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (head, tail) = (path.get(..i).unwrap_or(""), path.get(i..).unwrap_or(""));
    let trimmed = head.trim_end_matches('/');
    (if trimmed.is_empty() { head.to_string() } else { trimmed.to_string() }, tail.to_string())
}

fn join2(a: &str, b: &str) -> String {
    // posixpath.join
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

#[cfg(unix)]
fn join_real(mut path: String, rest: &str, seen: &mut HashMap<String, Option<String>>, depth: usize) -> (String, bool) {
    // posixpath._joinrealpath（strict=False）
    let mut rest = rest.to_string();
    if rest.starts_with('/') {
        rest = rest.get(1..).unwrap_or("").to_string();
        path = "/".into();
    }
    while !rest.is_empty() {
        let (name, tail) = match rest.split_once('/') {
            Some((n, t)) => (n.to_string(), t.to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = tail;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if path.is_empty() {
                path = "..".into();
            } else {
                let (head, tail) = split_head(&path);
                path = if tail == ".." { join2(&join2(&head, ".."), "..") } else { head };
            }
            continue;
        }
        let newpath = join2(&path, &name);
        let is_link = std::fs::symlink_metadata(&newpath).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(known) = seen.get(&newpath) {
            match known {
                Some(p) => {
                    path = p.clone();
                    continue;
                }
                None => return (join2(&newpath, &rest), false), // 連結繞圈
            }
        }
        if depth > 200 {
            return (join2(&newpath, &rest), false);
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(&newpath).ok().and_then(|t| t.to_str().map(str::to_string)) {
            Some(t) => t,
            None => return (join2(&newpath, &rest), false),
        };
        let (p, ok) = join_real(path, &target, seen, depth + 1);
        if !ok {
            return (join2(&p, &rest), false);
        }
        path = p.clone();
        seen.insert(newpath, Some(p));
    }
    (path, true)
}

/// `str(Path(p).resolve())`：解開符號連結，不存在的部分照字面接上。
///
/// 連結繞圈時 pathlib 丟 RuntimeError（不是 OSError），這裡照樣回錯誤。
pub fn resolve(p: &str) -> Result<String, PyErr> {
    #[cfg(unix)]
    {
        let start = if is_abs(p) { p.to_string() } else { format!("{}/{p}", cwd()) };
        let (path, ok) = join_real(String::new(), &start, &mut HashMap::new(), 0);
        let out = abspath(&path);
        if !ok {
            return Err(PyErr::other("RuntimeError", format!("Symlink loop from {}", pyfmt::repr_str(&out))));
        }
        Ok(out)
    }
    #[cfg(not(unix))]
    {
        let _: Option<HashMap<String, Option<String>>> = None;
        let _ = (split_head, join2);
        match std::fs::canonicalize(p) {
            Ok(c) => {
                let s = c.to_string_lossy().replace('\\', "/");
                Ok(s.strip_prefix("//?/").map(str::to_string).unwrap_or(s))
            }
            Err(_) => Ok(abspath(&p.replace('\\', "/"))),
        }
    }
}

/// `Path(p).mkdir(parents=True, exist_ok=True)`；錯誤訊息帶實際失敗的那一層。
pub fn mkdir_p(p: &str) -> Result<(), PyErr> {
    match std::fs::create_dir(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (parent, _) = split_head(p);
            if parent.is_empty() || parent == p {
                return Err(PyErr::os(&e, p));
            }
            mkdir_p(&parent)?;
            match std::fs::create_dir(p) {
                Ok(()) => Ok(()),
                Err(_) if std::path::Path::new(p).is_dir() => Ok(()),
                Err(e) => Err(PyErr::os(&e, p)),
            }
        }
        Err(_) if std::path::Path::new(p).is_dir() => Ok(()),
        Err(e) => Err(PyErr::os(&e, p)),
    }
}

/// 檔案的（mtime 奈秒, 大小），跟著符號連結走；對應 `p.stat()` 的 st_mtime_ns、st_size。
pub fn stat(p: &str) -> std::io::Result<(i64, i64)> {
    let m = std::fs::metadata(p)?;
    let t = m.modified()?;
    let ns = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    };
    Ok((ns, i64::try_from(m.len()).unwrap_or(i64::MAX)))
}

/// `p.read_text(encoding="utf-8", errors="replace")`：壞的位元組換成 U+FFFD，換行一律轉成 \n。
pub fn read_text(p: &str) -> std::io::Result<String> {
    let raw = std::fs::read(p)?;
    let s = String::from_utf8_lossy(&raw);
    Ok(if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.into_owned() })
}

// ------------------------------------------------------------------ json.loads
/// `json.loads(text)`。失敗時回 `str(JSONDecodeError)`（例如 `Expecting value: line 1 column 1 (char 0)`）；
/// 巢狀太深回 RecursionError。NaN、Infinity 這裡表示不了，當成 null（知識層只看物件與字串）。
pub fn json_loads(text: &str) -> Result<Value, PyErr> {
    let s: Vec<char> = text.chars().collect();
    let p = Json { s: &s };
    if s.first() == Some(&'\u{feff}') {
        return Err(p.err("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let (v, end) = p.scan(p.ws(0), 0)?;
    let end = p.ws(end);
    if end != s.len() {
        return Err(p.err("Extra data", end));
    }
    Ok(v)
}

struct Json<'a> {
    s: &'a [char],
}

impl Json<'_> {
    fn err(&self, msg: &str, pos: usize) -> PyErr {
        let before = self.s.get(..pos).unwrap_or(self.s);
        let line = before.iter().filter(|c| **c == '\n').count() + 1;
        let col = match before.iter().rposition(|c| *c == '\n') {
            Some(i) => pos - i,
            None => pos + 1,
        };
        PyErr::other("JSONDecodeError", format!("{msg}: line {line} column {col} (char {pos})"))
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn lit(&self, i: usize, word: &str) -> bool {
        word.chars().enumerate().all(|(k, c)| self.at(i + k) == Some(c))
    }

    fn scan(&self, i: usize, depth: usize) -> Result<(Value, usize), PyErr> {
        if depth > 900 {
            return Err(PyErr::other("RecursionError", "maximum recursion depth exceeded while decoding a JSON object from a unicode string"));
        }
        match self.at(i) {
            Some('"') => self.string(i + 1).map(|(s, e)| (Value::String(s), e)),
            Some('{') => self.object(i + 1, depth),
            Some('[') => self.array(i + 1, depth),
            Some('n') if self.lit(i, "null") => Ok((Value::Null, i + 4)),
            Some('t') if self.lit(i, "true") => Ok((Value::Bool(true), i + 4)),
            Some('f') if self.lit(i, "false") => Ok((Value::Bool(false), i + 5)),
            Some('N') if self.lit(i, "NaN") => Ok((Value::Null, i + 3)),
            Some('I') if self.lit(i, "Infinity") => Ok((Value::Null, i + 8)),
            Some('-') if self.lit(i, "-Infinity") => Ok((Value::Null, i + 9)),
            _ => self.number(i),
        }
    }

    fn number(&self, start: usize) -> Result<(Value, usize), PyErr> {
        let fail = || self.err("Expecting value", start);
        let digit = |i: usize| self.at(i).is_some_and(|c| c.is_ascii_digit());
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('0') => i += 1,
            Some('1'..='9') => {
                while digit(i) {
                    i += 1;
                }
            }
            _ => return Err(fail()),
        }
        let mut is_float = false;
        if self.at(i) == Some('.') && digit(i + 1) {
            is_float = true;
            i += 2;
            while digit(i) {
                i += 1;
            }
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i + 1;
            if matches!(self.at(j), Some('-' | '+')) {
                j += 1;
            }
            if digit(j) {
                is_float = true;
                while digit(j) {
                    j += 1;
                }
                i = j;
            }
        }
        let text: String = self.s.get(start..i).unwrap_or(&[]).iter().collect();
        let v = if is_float {
            text.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
        } else {
            match text.parse::<i64>() {
                Ok(n) => Value::Number(n.into()),
                Err(_) => text.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number).unwrap_or(Value::Null),
            }
        };
        Ok((v, i))
    }

    fn string(&self, begin: usize) -> Result<(String, usize), PyErr> {
        let unterminated = || self.err("Unterminated string starting at", begin.saturating_sub(1));
        let mut out = String::new();
        let mut i = begin;
        loop {
            let c = self.at(i).ok_or_else(unterminated)?;
            match c {
                '"' => return Ok((out, i + 1)),
                '\\' => {
                    let esc = self.at(i + 1).ok_or_else(unterminated)?;
                    if esc != 'u' {
                        out.push(match esc {
                            '"' => '"',
                            '\\' => '\\',
                            '/' => '/',
                            'b' => '\u{8}',
                            'f' => '\u{c}',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            _ => return Err(self.err("Invalid \\escape", i)),
                        });
                        i += 2;
                        continue;
                    }
                    let hex4 = |at: usize| -> Option<u32> {
                        let mut v = 0u32;
                        for k in 0..4 {
                            v = v * 16 + self.at(at + k)?.to_digit(16)?;
                        }
                        Some(v)
                    };
                    // CPython：\u 後面要有四個十六進位字元，而且後面還要有東西
                    if i + 6 >= self.s.len() {
                        return Err(self.err("Invalid \\uXXXX escape", i + 1));
                    }
                    let mut cp = hex4(i + 2).ok_or_else(|| self.err("Invalid \\uXXXX escape", i + 1))?;
                    i += 6;
                    if (0xd800..=0xdbff).contains(&cp) && self.at(i) == Some('\\') && self.at(i + 1) == Some('u') && i + 6 < self.s.len() {
                        if let Some(lo) = hex4(i + 2).filter(|v| (0xdc00..=0xdfff).contains(v)) {
                            cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                            i += 6;
                        }
                    }
                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                }
                c if (c as u32) <= 0x1f => return Err(self.err("Invalid control character at", i)),
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }

    fn object(&self, start: usize, depth: usize) -> Result<(Value, usize), PyErr> {
        let mut map = Map::new();
        let mut i = self.ws(start);
        if self.at(i) == Some('}') {
            return Ok((Value::Object(map), i + 1));
        }
        loop {
            if self.at(i) != Some('"') {
                return Err(self.err("Expecting property name enclosed in double quotes", i));
            }
            let (key, e) = self.string(i + 1)?;
            i = self.ws(e);
            if self.at(i) != Some(':') {
                return Err(self.err("Expecting ':' delimiter", i));
            }
            i = self.ws(i + 1);
            let (v, e) = self.scan(i, depth + 1)?;
            map.insert(key, v);
            i = self.ws(e);
            match self.at(i) {
                Some('}') => return Ok((Value::Object(map), i + 1)),
                Some(',') => i = self.ws(i + 1),
                _ => return Err(self.err("Expecting ',' delimiter", i)),
            }
        }
    }

    fn array(&self, start: usize, depth: usize) -> Result<(Value, usize), PyErr> {
        let mut items = Vec::new();
        let mut i = self.ws(start);
        if self.at(i) == Some(']') {
            return Ok((Value::Array(items), i + 1));
        }
        loop {
            let (v, e) = self.scan(i, depth + 1)?;
            items.push(v);
            i = self.ws(e);
            match self.at(i) {
                Some(']') => return Ok((Value::Array(items), i + 1)),
                Some(',') => i = self.ws(i + 1),
                _ => return Err(self.err("Expecting ',' delimiter", i)),
            }
        }
    }
}

/// `json.dumps(s, ensure_ascii=False)`（字串）。
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_python() {
        assert_eq!(normpath("a/../../b/./c//d/"), "../b/c/d");
        assert_eq!(normpath("docs/.."), ".");
        assert_eq!(relpath("/a/b/c", "/a/x"), "../b/c");
        assert_eq!(relpath("/a", "/a"), ".");
        assert_eq!(splitext_stem("docs/a.b.md"), "docs/a.b");
        assert_eq!(splitext_stem("docs/.env"), "docs/.env");
        assert_eq!(splitext_stem("x.d/readme"), "x.d/readme");
        assert_eq!(ppath("/a//b/./c/"), "/a/b/c");
        assert_eq!(pjoin("/r", "../x"), "/r/../x");
        assert!(is_under("/a/b", "/a") && !is_under("/ab", "/a") && is_under("/a", "/a"));
    }

    #[test]
    fn strings_match_python() {
        assert_eq!(casefold("Straße ΑΣ ẞ"), "strasse ασ ss");
        assert_eq!(unquote("a%20b%E4%B8%AD%zz%"), "a b中%zz%");
        assert_eq!(collapse_ws("a \t\u{3000}b\x1cc"), "a b c");
        assert_eq!(utc_minute(0), "1970-01-01 00:00");
        assert_eq!(utc_minute(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(round_nd(0.0078125, 6), 0.007812);
    }

    #[test]
    fn json_errors_match_python() {
        let e = |t: &str| json_loads(t).err().map(|e| e.msg).unwrap_or_default();
        assert_eq!(e("{壞掉"), "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)");
        assert_eq!(e(""), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(e("{\"a\": 1,}"), "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)");
        assert_eq!(e("[1,\n 2 3]"), "Expecting ',' delimiter: line 2 column 4 (char 7)");
        assert_eq!(e("{\"a\" 1}"), "Expecting ':' delimiter: line 1 column 6 (char 5)");
        assert_eq!(e("1 2"), "Extra data: line 1 column 3 (char 2)");
        assert_eq!(e("\"abc"), "Unterminated string starting at: line 1 column 1 (char 0)");
        assert_eq!(e("\"a\\qb\""), "Invalid \\escape: line 1 column 3 (char 2)");
        assert_eq!(json_loads("{\"a\": [1, 2.5, \"x\\u4e2d\", null, true]}").ok(), serde_json::from_str("{\"a\": [1, 2.5, \"x中\", null, true]}").ok());
    }
}
