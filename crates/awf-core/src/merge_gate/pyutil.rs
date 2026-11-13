//! 閘門工具共用的小工具：照 Python 的語意處理文字、路徑、日期與子程序。
//!
//! merge-gate 與 sync-branch 的訊息文字是對跑基準的一部分，所以這裡每個函式都對應一個 Python 的行為
//! （`str.strip`、`str.splitlines`、`Path.read_text` 的換行轉換、`OSError` 的文字、`date.fromisoformat`…）。
//! 之後會整併成全工具共用的模組；目前只有這兩支用。

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ------------------------------------------------------------------ 文字

/// Python 的 `str.isspace()` 認定的空白（比 Unicode White_Space 多了 U+001C–U+001F）。
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `s.strip()`。
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `s[:n]`（以 code point 計）。
pub fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[-n:]`（以 code point 計）。
pub fn tail(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n { return s; }
    match s.char_indices().nth(total - n) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// `s.splitlines()`：Python 認的行界比 `\n` 多（`\r`、`\v`、`\f`、U+001C–U+001E、U+0085、U+2028、U+2029）。
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk { continue; }
        out.push(&s[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(j, '\n')) = it.peek() {
                it.next();
                next = j + 1;
            }
        }
        start = next;
    }
    if start < s.len() { out.push(&s[start..]); }
    out
}

/// 文字模式讀檔的換行轉換（`\r\n`、`\r` → `\n`）。
pub fn universal_newlines(s: &str) -> String {
    if !s.contains('\r') { return s.to_string(); }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// 子程序輸出（`text=True, errors="replace"`）。
pub fn text(bytes: &[u8]) -> String {
    universal_newlines(&String::from_utf8_lossy(bytes))
}

/// Python 的 `int(s)`（s 是字串）：前後空白、正負號、數字間單一底線。只收 ASCII 數字；
/// Python 另外收其他文字的十進位數字，這裡不收（呼叫端一律往嚴的方向處理）。
pub fn int_of_str(s: &str) -> Option<i128> {
    let t = strip(s);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") { return None; }
    let mut n: i128 = 0;
    for c in digits.chars() {
        if c == '_' { continue; }
        let d = c.to_digit(10)? as i128;
        n = n.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -n } else { n })
}

// ------------------------------------------------------------------ 讀檔與錯誤文字

/// 讀檔失敗的種類：`Os` 對應 Python 的 OSError，`Decode` 對應 UnicodeDecodeError（Python 版多半沒接、直接結束）。
#[derive(Debug, Clone, PartialEq)]
pub enum ReadErr {
    Os(String),
    Decode(String),
}

impl ReadErr {
    pub fn msg(&self) -> &str {
        match self {
            ReadErr::Os(m) | ReadErr::Decode(m) => m,
        }
    }
}

/// `str(OSError)`：`[Errno 2] No such file or directory: '/路徑'`。
pub fn os_error_text(e: &std::io::Error, name: &str) -> String {
    let full = e.to_string();
    match e.raw_os_error() {
        Some(n) => {
            let what = full.strip_suffix(&format!(" (os error {n})")).unwrap_or(&full);
            format!("[Errno {n}] {what}: {}", crate::pyfmt::repr_str(name))
        }
        None => full,
    }
}

fn decode_error_text(bytes: &[u8], e: &std::str::Utf8Error) -> String {
    let pos = e.valid_up_to();
    let b = bytes.get(pos).copied().unwrap_or(0);
    let why = match e.error_len() {
        None => "unexpected end of data",
        Some(_) if (0x80..=0xc1).contains(&b) || b >= 0xf5 => "invalid start byte",
        Some(_) => "invalid continuation byte",
    };
    format!("'utf-8' codec can't decode byte 0x{b:02x} in position {pos}: {why}")
}

/// `Path(p).read_bytes()`。
pub fn read_bytes(path: &str) -> Result<Vec<u8>, ReadErr> {
    std::fs::read(path).map_err(|e| ReadErr::Os(os_error_text(&e, path)))
}

/// `Path(p).read_text(encoding="utf-8")`（含換行轉換）。
pub fn read_text(path: &str) -> Result<String, ReadErr> {
    let bytes = read_bytes(path)?;
    match std::str::from_utf8(&bytes) {
        Ok(s) => Ok(universal_newlines(s)),
        Err(e) => Err(ReadErr::Decode(decode_error_text(&bytes, &e))),
    }
}

/// `Path(p).read_text(encoding="utf-8", errors="replace")`。
pub fn read_text_lossy(path: &str) -> Result<String, ReadErr> {
    Ok(text(&read_bytes(path)?))
}

// ------------------------------------------------------------------ 路徑（照 pathlib 的文字規則）

/// `str(Path(s))`：去掉多餘的分隔符與 `.`，空字串是 `.`；`..` 保留。
pub fn path_str(s: &str) -> String {
    let mut out = PathBuf::new();
    for c in Path::new(s).components() {
        if !matches!(c, Component::CurDir) { out.push(c.as_os_str()); }
    }
    let t = out.to_string_lossy().into_owned();
    if t.is_empty() { ".".into() } else { t }
}

pub fn is_absolute(s: &str) -> bool {
    Path::new(s).is_absolute()
}

/// `str(Path(base) / rel)`：rel 是絕對路徑時就是 rel。
pub fn join(base: &str, rel: &str) -> String {
    if rel.is_empty() { return path_str(base); }
    if is_absolute(rel) { return path_str(rel); }
    path_str(&Path::new(base).join(rel).to_string_lossy())
}

/// `Path(s).name`。
pub fn name(s: &str) -> String {
    match Path::new(&path_str(s)).components().next_back() {
        Some(Component::Normal(n)) => n.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

/// `str(Path(s).parent)`。
pub fn parent(s: &str) -> String {
    let p = path_str(s);
    match Path::new(&p).parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_string_lossy().into_owned(),
        Some(_) => ".".into(),
        None => p,
    }
}

/// `str(Path(s).resolve())`：轉成絕對路徑、解開符號連結；不存在的尾段照字面接上（`..` 往上一層）。
pub fn resolve(s: &str) -> String {
    let abs = if is_absolute(s) {
        PathBuf::from(path_str(s))
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path_str(s))
    };
    let comps: Vec<Component> = abs.components().collect();
    for cut in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..cut].iter().map(|c| c.as_os_str()).collect();
        let Ok(mut real) = std::fs::canonicalize(&prefix) else { continue };
        for c in &comps[cut..] {
            match c {
                Component::ParentDir => { real.pop(); }
                Component::CurDir => {}
                other => real.push(other.as_os_str()),
            }
        }
        return real.to_string_lossy().into_owned();
    }
    abs.to_string_lossy().into_owned()
}

// ------------------------------------------------------------------ 日期（proleptic Gregorian，同 datetime.date）

pub type Ymd = (i32, u32, u32);

pub fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// `date(y, m, d).toordinal()`：0001-01-01 是 1。
pub fn to_ordinal((y, m, d): Ymd) -> i64 {
    let y1 = (y - 1) as i64;
    let before_year = y1 * 365 + y1 / 4 - y1 / 100 + y1 / 400;
    let before_month: i64 = (1..m).map(|k| days_in_month(y, k) as i64).sum();
    before_year + before_month + d as i64
}

/// `date.fromordinal(n)`；超出 0001–9999 年回 None。
pub fn from_ordinal(n: i64) -> Option<Ymd> {
    if n < 1 { return None; }
    let mut n0 = n - 1;
    let n400 = n0 / 146_097;
    n0 %= 146_097;
    let n100 = (n0 / 36_524).min(3);
    n0 -= n100 * 36_524;
    let n4 = n0 / 1_461;
    n0 %= 1_461;
    let n1 = (n0 / 365).min(3);
    n0 -= n1 * 365;
    let y = (n400 * 400 + n100 * 100 + n4 * 4 + n1 + 1) as i32;
    if y > 9999 { return None; }
    let mut m = 1;
    let mut rest = n0 as u32;
    while rest >= days_in_month(y, m) {
        rest -= days_in_month(y, m);
        m += 1;
    }
    Some((y, m, rest + 1))
}

/// `date.isoformat()`。
pub fn iso((y, m, d): Ymd) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// 往後（或往前）幾天。
pub fn add_days(day: Ymd, n: i64) -> Option<Ymd> {
    from_ordinal(to_ordinal(day) + n)
}

fn valid(y: i32, m: u32, d: u32) -> Option<Ymd> {
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) { return None; }
    Some((y, m, d))
}

fn digits(b: &[u8], at: usize, n: usize) -> Option<i32> {
    let part = b.get(at..at + n)?;
    let mut v = 0;
    for c in part {
        if !c.is_ascii_digit() { return None; }
        v = v * 10 + (c - b'0') as i32;
    }
    Some(v)
}

/// `date.fromisoformat(s)`（Python 3.11 以後的規則）：長度 7、8、10（以 UTF-8 位元組計）；
/// 收 `YYYY-MM-DD`、`YYYYMMDD`、週曆 `YYYY-Www[-D]`、`YYYYWww[D]`。和 CPython 一樣，解析完不檢查後面還有沒有東西
/// （`20991024xx` 讀成 2099-10-24）。讀不懂回 None（Python 丟 ValueError）。
pub fn parse_iso_date(s: &str) -> Option<Ymd> {
    let b = s.as_bytes();
    if !matches!(b.len(), 7 | 8 | 10) { return None; }
    let y = digits(b, 0, 4)?;
    let sep = b.get(4) == Some(&b'-');
    let mut p = if sep { 5 } else { 4 };
    if b.get(p) == Some(&b'W') {
        p += 1;
        let week = digits(b, p, 2)?;
        p += 2;
        let day = if p < b.len() {
            if sep {
                if b.get(p) != Some(&b'-') { return None; }
                p += 1;
            }
            digits(b, p, 1)?
        } else {
            1
        };
        if !(1..=9999).contains(&y) { return None; }
        let jan1 = to_ordinal((y, 1, 1));
        let first_weekday = (jan1 + 6) % 7; // 0＝週一
        if week <= 0 || week >= 53 {
            let long_year = first_weekday == 3 || (first_weekday == 2 && is_leap(y));
            if !(week == 53 && long_year) { return None; }
        }
        if !(1..=7).contains(&day) { return None; }
        let mut week1_monday = jan1 - first_weekday;
        if first_weekday > 3 { week1_monday += 7; }
        return from_ordinal(week1_monday + (week as i64 - 1) * 7 + day as i64 - 1);
    }
    let m = digits(b, p, 2)?;
    p += 2;
    if sep {
        if b.get(p) != Some(&b'-') { return None; }
        p += 1;
    }
    let d = digits(b, p, 2)?;
    valid(y, m.try_into().ok()?, d.try_into().ok()?)
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 系統的 UTC 日期。
pub fn today_utc() -> Ymd {
    from_ordinal(now_secs().div_euclid(86_400) + 719_163).unwrap_or((1970, 1, 1))
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`。
pub fn now_iso_seconds() -> String {
    let secs = now_secs();
    let (day, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let date = from_ordinal(day + 719_163).unwrap_or((1970, 1, 1));
    format!("{}T{:02}:{:02}:{:02}+00:00", iso(date), rem / 3600, rem % 3600 / 60, rem % 60)
}

// ------------------------------------------------------------------ 子程序

pub struct Output {
    /// exit code；被訊號結束時是負的訊號編號（同 Python 的 returncode）。
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub enum RunErr {
    /// 跑不起來（找不到程式等），對應 OSError。
    Os(std::io::Error),
    /// 逾時，子程序已經殺掉。
    Timeout,
}

fn exit_code(st: std::process::ExitStatus) -> i32 {
    if let Some(c) = st.code() { return c; }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() { return -sig; }
    }
    -1
}

/// 跑子程序並收 stdout、stderr。`input` 有給就寫進 stdin，沒給 stdin 接空裝置。不會 panic。
pub fn run(cmd: &mut Command, input: Option<Vec<u8>>, timeout: Option<Duration>) -> Result<Output, RunErr> {
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(RunErr::Os)?;
    // 讀寫都另開執行緒，避免管線塞滿互相卡住
    let writer = child.stdin.take().zip(input).map(|(mut pipe, data)| std::thread::spawn(move || { let _ = pipe.write_all(&data); }));
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = so.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let start = Instant::now();
    let status = match timeout {
        None => child.wait().ok(),
        Some(limit) => loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {
                    if start.elapsed() >= limit {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break None,
            }
        },
    };
    if let Some(w) = writer { let _ = w.join(); }
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    match status {
        Some(st) => Ok(Output { code: exit_code(st), stdout, stderr }),
        None => Err(RunErr::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_match_python() {
        assert_eq!(to_ordinal((1, 1, 1)), 1);
        assert_eq!(to_ordinal((1970, 1, 1)), 719_163);
        assert_eq!(to_ordinal((2000, 2, 29)), 730_179);
        assert_eq!(from_ordinal(730_179), Some((2000, 2, 29)));
        assert_eq!(from_ordinal(730_180), Some((2000, 3, 1)));
        assert_eq!(from_ordinal(to_ordinal((1900, 3, 1)) - 1), Some((1900, 2, 28))); // 1900 不是閏年
        assert_eq!(from_ordinal(to_ordinal((2024, 12, 31))), Some((2024, 12, 31)));
        assert_eq!(from_ordinal(to_ordinal((9999, 12, 31))), Some((9999, 12, 31)));
        assert_eq!(from_ordinal(to_ordinal((9999, 12, 31)) + 1), None);
        assert_eq!(add_days((2024, 2, 28), 1), Some((2024, 2, 29)));
        assert_eq!(add_days((2023, 2, 28), 1), Some((2023, 3, 1)));
        assert_eq!(add_days((2024, 1, 1), -1), Some((2023, 12, 31)));
        for n in (1..3_652_059).step_by(997) {
            assert_eq!(to_ordinal(from_ordinal(n).unwrap()), n);
        }
        assert_eq!(parse_iso_date("2099-10-24"), Some((2099, 10, 24)));
        assert_eq!(parse_iso_date("20991024xx"), Some((2099, 10, 24)));
        assert_eq!(parse_iso_date("2099-W43-6"), Some((2099, 10, 24)));
        assert_eq!(parse_iso_date("2099W43"), Some((2099, 10, 19)));
        assert_eq!(parse_iso_date("2098-W53"), None);
        assert_eq!(parse_iso_date("9999-W52-7"), None);
        assert_eq!(parse_iso_date("2099-02-29"), None);
        assert_eq!(parse_iso_date("2096-02-29"), Some((2096, 2, 29)));
        assert_eq!(parse_iso_date("明年"), None);
    }

    #[test]
    fn text_helpers_match_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd\u{2028}e"), vec!["a", "b", "c", "", "d", "e"]);
        assert_eq!(splitlines("a\n"), vec!["a"]);
        assert!(splitlines("").is_empty());
        assert_eq!(tail("示範客戶", 2), "客戶");
        assert_eq!(head("示範客戶", 2), "示範");
        assert_eq!(int_of_str(" +1_0 "), Some(10));
        assert_eq!(int_of_str("-0"), Some(0));
        assert_eq!(int_of_str("1__0"), None);
        assert_eq!(int_of_str("unknown"), None);
        assert_eq!(path_str("a//b/./c/"), "a/b/c");
        assert_eq!(path_str(""), ".");
        assert_eq!(join("/p", "/q/r"), "/q/r");
        assert_eq!(name("/a/b.json"), "b.json");
    }
}
