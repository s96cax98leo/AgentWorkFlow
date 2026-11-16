//! 和 Python 標準庫行為相同的小工具：字串（strip／split／casefold）、shlex、json.dumps、時間。
//!
//! 守門規則的判定建立在這些細節上（例如 `str.split()` 把 \x1c 也當空白、`shlex.split` 的引號規則），
//! 所以不直接用 Rust 的 `trim`／`split_whitespace`，而是照 Python 的定義重寫。

use serde_json::Value;

/// Python 的 `str.isspace()`（單一字元）。比 Unicode White_Space 多了 \x1c–\x1f。
pub fn is_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `s.split()`：以空白切，不留空字串。
pub fn split_ws(s: &str) -> Vec<String> {
    s.split(is_space).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

/// `s.casefold()` 的近似：大寫再小寫做兩輪（ß→ss、ſ→s、ς→σ、ẞ→ss）。
/// 只用在「兩邊都 fold 再比」的地方；和真正的 casefold 相比只會把更多寫法視為相同，也就是只會多擋。
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.to_uppercase().to_lowercase().to_uppercase().to_lowercase()
}

pub fn lower(s: &str) -> String {
    if s.is_ascii() { s.to_ascii_lowercase() } else { s.to_lowercase() }
}

/// 前 n 個字元（`s[:n]`）。
pub fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 最後 n 個字元（`s[-n:]`）。
pub fn tail(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    match s.char_indices().nth(total - n) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// `shlex.split(s)`（posix、whitespace_split、不處理註解）。引號沒關或結尾是跳脫字元回 Err（Python 丟 ValueError）。
pub fn shlex_split(s: &str) -> Result<Vec<String>, ()> {
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Ws,
        Word,
        Quote(char),
        Esc,
    }
    let mut out = Vec::new();
    let mut tok = String::new();
    let mut quoted = false;
    let mut st = St::Ws;
    let mut esc_from = St::Word;
    for c in s.chars() {
        match st {
            St::Ws => {
                if matches!(c, ' ' | '\t' | '\r' | '\n') {
                    continue;
                }
                if c == '\\' {
                    esc_from = St::Word;
                    st = St::Esc;
                } else if c == '\'' || c == '"' {
                    st = St::Quote(c);
                } else {
                    tok.push(c);
                    st = St::Word;
                }
            }
            St::Quote(q) => {
                quoted = true;
                if c == q {
                    st = St::Word;
                } else if c == '\\' && q == '"' {
                    esc_from = st;
                    st = St::Esc;
                } else {
                    tok.push(c);
                }
            }
            St::Esc => {
                if let St::Quote(q) = esc_from {
                    if c != '\\' && c != q {
                        tok.push('\\');
                    }
                }
                tok.push(c);
                st = esc_from;
            }
            St::Word => {
                if matches!(c, ' ' | '\t' | '\r' | '\n') {
                    if !tok.is_empty() || quoted {
                        out.push(std::mem::take(&mut tok));
                    }
                    quoted = false;
                    st = St::Ws;
                } else if c == '\'' || c == '"' {
                    st = St::Quote(c);
                } else if c == '\\' {
                    esc_from = St::Word;
                    st = St::Esc;
                } else {
                    tok.push(c);
                }
            }
        }
    }
    match st {
        St::Quote(_) | St::Esc => Err(()),
        _ => {
            if !tok.is_empty() || quoted {
                out.push(tok);
            }
            Ok(out)
        }
    }
}

/// guard.py 的 `shlex_tokens`：切不開就退回空白切。
pub fn shlex_tokens(s: &str) -> Vec<String> {
    shlex_split(s).unwrap_or_else(|_| split_ws(s))
}

/// `shlex.quote(s)`。
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'));
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn dumps_str(s: &str, ensure_ascii: bool, out: &mut String) {
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
            c if ensure_ascii && !c.is_ascii() => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dumps_into(v: &Value, ensure_ascii: bool, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&crate::pyfmt::float(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => dumps_str(s, ensure_ascii, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_into(x, ensure_ascii, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dumps_str(k, ensure_ascii, out);
                out.push_str(": ");
                dumps_into(x, ensure_ascii, out);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(v, ensure_ascii=…)`：分隔符是「, 」與「: 」。
pub fn json_dumps(v: &Value, ensure_ascii: bool) -> String {
    let mut out = String::new();
    dumps_into(v, ensure_ascii, &mut out);
    out
}

/// `int(x)`（x 是 JSON 值）；轉不了回 None（Python 丟 TypeError／ValueError）。
pub fn to_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i as i128)
            } else if let Some(u) = n.as_u64() {
                Some(u as i128)
            } else {
                let f = n.as_f64()?;
                if f.is_finite() { Some(f.trunc() as i128) } else { None }
            }
        }
        Value::String(s) => {
            let t = strip(s);
            let (neg, digits) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            // int() 接受數字之間的單一底線（1_000）
            if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
                return None;
            }
            let clean: String = digits.chars().filter(|c| *c != '_').collect();
            if !clean.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            clean.parse::<i128>().ok().map(|x| if neg { -x } else { x })
        }
        _ => None,
    }
}

/// 自 1970-01-01 起的天數 → (年, 月, 日)。
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())`。
pub fn utc_stamp() -> String {
    let t = unix_now() as i64;
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, s / 3600, s % 3600 / 60, s % 60)
}

/// 本機時區的今天（`date.today()`），回傳 (年, 月, 日)。取不到本機時間時回 None。
#[cfg(unix)]
pub fn local_today() -> Option<(i64, u32, u32)> {
    // 不引入額外相依：直接呼叫 C 函式庫的 localtime_r。struct tm 在 Linux（glibc、musl）與 macOS 的前九個欄位都是 int，
    // 後面是 gmtoff 與時區名指標；多留一些空間以免不同平台的尾端欄位不同。
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        _pad: [i64; 8],
    }
    extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let now = unix_now() as i64;
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0, _pad: [0; 8] };
    // SAFETY：兩個指標都指向這個函式內有效、大小足夠的區域；localtime_r 只寫入 out。
    let r = unsafe { localtime_r(&now, &mut tm) };
    if r.is_null() || !(1..=31).contains(&tm.mday) || !(0..=11).contains(&tm.mon) {
        return None;
    }
    Some((tm.year as i64 + 1900, tm.mon as u32 + 1, tm.mday as u32))
}

#[cfg(windows)]
pub fn local_today() -> Option<(i64, u32, u32)> {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut st = SystemTime { year: 0, month: 0, dow: 0, day: 0, hour: 0, minute: 0, second: 0, ms: 0 };
    // SAFETY：指標指向有效的 SYSTEMTIME，GetLocalTime 只寫入它。
    unsafe { GetLocalTime(&mut st) };
    if st.month == 0 || st.day == 0 {
        return None;
    }
    Some((st.year as i64, st.month as u32, st.day as u32))
}

#[cfg(not(any(unix, windows)))]
pub fn local_today() -> Option<(i64, u32, u32)> {
    None
}

/// `date.fromisoformat(s)`：接受 YYYY-MM-DD 與 YYYYMMDD；其他（含 ISO 週曆寫法）回 None。
/// Python 3.11 之後另外接受週曆寫法；這裡不收，授權期限讀不懂＝沒有授權，只會更嚴。
pub fn parse_iso_date(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = b.get(r)?;
        if part.iter().all(u8::is_ascii_digit) { std::str::from_utf8(part).ok()?.parse().ok() } else { None }
    };
    let (y, m, d) = if b.len() == 10 && b[4] == b'-' && b[7] == b'-' {
        (digits(0..4)?, digits(5..7)?, digits(8..10)?)
    } else if b.len() == 8 {
        (digits(0..4)?, digits(4..6)?, digits(6..8)?)
    } else {
        return None;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if leap { 29 } else { 28 },
        _ => return None,
    };
    if y == 0 || d == 0 || d > dim {
        return None;
    }
    Some((y as i64, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shlex_matches_python() {
        assert_eq!(shlex_split("a 'b c' \"d\\\"e\" f\\ g ''").unwrap(), vec!["a", "b c", "d\"e", "f g", ""]);
        assert_eq!(shlex_split("a\"b\\xc\"").unwrap(), vec!["ab\\xc"]);
        assert!(shlex_split("a 'b").is_err());
        assert!(shlex_split("a\\").is_err());
        assert_eq!(shlex_quote("a b"), "'a b'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote("a/b-c.d"), "a/b-c.d");
    }

    #[test]
    fn dumps_matches_python() {
        let v: Value = serde_json::from_str(r#"{"a": [1, 1.5, true, null], "中": "x\ny"}"#).unwrap();
        assert_eq!(json_dumps(&v, true), format!("{{\"a\": [1, 1.5, true, null], \"{}u4e2d\": \"x{}ny\"}}", '\\', '\\'));
        assert_eq!(json_dumps(&v, false), "{\"a\": [1, 1.5, true, null], \"中\": \"x\\ny\"}");
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(parse_iso_date("2026-02-29"), None);
        assert_eq!(parse_iso_date("2024-02-29"), Some((2024, 2, 29)));
        assert!(local_today().is_some());
    }
}
