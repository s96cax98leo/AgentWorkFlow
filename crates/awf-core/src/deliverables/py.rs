//! 交付文件工具用到的 Python 標準庫行為：`re` 的語意、`html.unescape`、`str.splitlines`、`json.dumps`。
//!
//! 這些是 deliverables 模組私有的實作。隱私規則靠它們和 0.x 的 Python 版判定一致，改動前先跑對跑基準。

use super::html5::{HTML5, INVALID_CHARREFS, INVALID_CODEPOINTS};
use crate::kb::pyish::WS_CLASS;
use serde_json::{Map, Value};

/// Python `re` 的 `\w`（字串樣式）：字母、數字（類別 L 與 N）與底線。
const W: &str = r"\p{L}\p{N}_";

/// 把 Python `re` 的樣式改寫成 `fancy_regex` 語意相同的樣式。
///
/// - `\w`、`\s`、`\d`、`\b` 照 Python 的 Unicode 定義（Rust 的 `\w` 多了結合符號與連接標點、`\s` 少了 U+001C～U+001F）。
/// - `\Z` 是字串結尾；`\<`、`\>` 在 Python 是字面的角括號（Rust 當成單字邊界）。
/// - 字元類別裡的 `[`、`&`、`~` 在 Python 是字面字元（Rust 當成巢狀類別或集合運算）。
/// - `ci`＝整個樣式不分大小寫（呼叫端自己加 `(?i)`）：Python 的 `re.I` 把 İ（U+0130）與 ı（U+0131）當成 i，Rust 不會，這裡補上。
pub(super) fn translate(pat: &str, ci: bool) -> String {
    let cs: Vec<char> = pat.chars().collect();
    let at = |i: usize| cs.get(i).copied();
    let mut out = String::with_capacity(pat.len() * 2);
    let mut i = 0;
    let mut in_class = false;
    let mut class_start = 0;
    let mut class_has_i = false;
    while let Some(c) = at(i) {
        if c == '\\' {
            let Some(n) = at(i + 1) else {
                out.push('\\');
                break;
            };
            i += 2;
            match n {
                'w' if in_class => out.push_str(W),
                'w' => out.push_str(&format!("[{W}]")),
                'W' => out.push_str(&format!("[^{W}]")),
                's' if in_class => out.push_str(WS_CLASS),
                's' => out.push_str(&format!("[{WS_CLASS}]")),
                'S' => out.push_str(&format!("[^{WS_CLASS}]")),
                'd' => out.push_str(r"\p{Nd}"),
                'D' => out.push_str(r"\P{Nd}"),
                'b' if in_class => out.push_str(r"\x08"),
                'b' => out.push_str(&format!("(?:(?<=[{W}])(?![{W}])|(?<![{W}])(?=[{W}]))")),
                'B' if !in_class => out.push_str(&format!("(?:(?<=[{W}])(?=[{W}])|(?<![{W}])(?![{W}]))")),
                'Z' if !in_class => out.push_str(r"\z"),
                '<' | '>' => out.push(n),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }
        if in_class {
            match c {
                ']' if i > class_start => {
                    if ci && class_has_i {
                        // 類別結尾的 `-` 是字面的連字號：補的字元要放在它前面，不然會變成一段範圍
                        let tail_hyphen = out.ends_with('-') && !out.ends_with("\\-");
                        if tail_hyphen {
                            out.pop();
                        }
                        out.push_str("\u{130}\u{131}");
                        if tail_hyphen {
                            out.push_str("\\-");
                        }
                    }
                    out.push(']');
                    in_class = false;
                }
                '[' => out.push_str(r"\["),
                '&' => out.push_str(r"\&"),
                '~' => out.push_str(r"\~"),
                _ => {
                    if ci {
                        if c == 'i' || c == 'I' {
                            class_has_i = true;
                        }
                        if at(i + 1) == Some('-') {
                            if let Some(hi) = at(i + 2).filter(|h| *h != ']' && *h != '\\') {
                                if (c <= 'i' && 'i' <= hi) || (c <= 'I' && 'I' <= hi) {
                                    class_has_i = true;
                                }
                            }
                        }
                    }
                    out.push(c);
                }
            }
            i += 1;
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                class_has_i = false;
                out.push('[');
                i += 1;
                if at(i) == Some('^') {
                    out.push('^');
                    i += 1;
                }
                class_start = i;
                if at(i) == Some(']') {
                    out.push_str(r"\]");
                    i += 1;
                }
                continue;
            }
            '(' if at(i + 1) == Some('?') => {
                // 旗標或群組的種類：照抄，不改裡面的字母
                out.push_str("(?");
                i += 2;
                while let Some(f) = at(i).filter(|f| f.is_ascii_alphabetic() || *f == '-') {
                    out.push(f);
                    i += 1;
                }
                continue;
            }
            'i' | 'I' if ci => out.push_str("[i\u{130}\u{131}]"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// 編譯改寫後的樣式；編不出來回 None。不設回溯上限（Python 的 `re` 沒有上限）。
pub(super) fn compile_raw(pat: &str) -> Option<fancy_regex::Regex> {
    fancy_regex::RegexBuilder::new(pat).backtrack_limit(usize::MAX).build().ok()
}

/// `re.compile(pat)`（使用者寫的 Python 樣式）；編不出來回 None。
pub(super) fn compile_py(pat: &str) -> Option<fancy_regex::Regex> {
    compile_raw(&translate(pat, false))
}

/// `rx.search(text)`；執行出錯當成有命中（只會更嚴）。
pub(super) fn search(rx: &fancy_regex::Regex, text: &str) -> bool {
    rx.is_match(text).unwrap_or(true)
}

/// `rx.finditer(text)` 的每個 `m.group(0)`；執行出錯時回 Err（呼叫端當成有命中）。
pub(super) fn find_all<'t>(rx: &fancy_regex::Regex, text: &'t str) -> Result<Vec<&'t str>, ()> {
    let mut out = Vec::new();
    for m in rx.find_iter(text) {
        match m {
            Ok(m) => out.push(m.as_str()),
            Err(_) => return Err(()),
        }
    }
    Ok(out)
}

/// `str.splitlines()`：\n、\r、\r\n、\v、\f、\x1c～\x1e、\x85、U+2028、U+2029 都算換行。
pub(super) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(s.get(start..i).unwrap_or(""));
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = it.peek().copied() {
                    it.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(s.get(start..).unwrap_or(""));
    }
    out
}

fn html5_get(name: &str) -> Option<&'static str> {
    HTML5.binary_search_by(|(k, _)| k.as_bytes().cmp(name.as_bytes())).ok().and_then(|i| HTML5.get(i)).map(|x| x.1)
}

fn charref(num: u64) -> String {
    if let Ok(i) = INVALID_CHARREFS.binary_search_by(|(k, _)| u64::from(*k).cmp(&num)) {
        return INVALID_CHARREFS.get(i).map(|x| x.1.to_string()).unwrap_or_default();
    }
    if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
        return "\u{FFFD}".to_string();
    }
    if INVALID_CODEPOINTS.binary_search_by(|k| u64::from(*k).cmp(&num)).is_ok() {
        return String::new();
    }
    u32::try_from(num).ok().and_then(char::from_u32).map(String::from).unwrap_or_else(|| "\u{FFFD}".to_string())
}

/// `html.unescape(s)`（CPython 3.12）。
pub(super) fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let cs: Vec<char> = s.chars().collect();
    let at = |i: usize| cs.get(i).copied();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(c) = at(i) {
        if c != '&' {
            out.push(c);
            i += 1;
            continue;
        }
        let j = i + 1;
        if at(j) == Some('#') {
            let mut k = j + 1;
            let mut num: u64 = 0;
            while let Some(d) = at(k).and_then(|d| if d.is_ascii_digit() { d.to_digit(10) } else { None }) {
                num = num.saturating_mul(10).saturating_add(u64::from(d));
                k += 1;
            }
            if k == j + 1 && matches!(at(j + 1), Some('x' | 'X')) {
                k = j + 2;
                while let Some(d) = at(k).and_then(|d| if d.is_ascii_hexdigit() { d.to_digit(16) } else { None }) {
                    num = num.saturating_mul(16).saturating_add(u64::from(d));
                    k += 1;
                }
                if k == j + 2 {
                    k = j + 1;
                }
            }
            if k == j + 1 {
                out.push('&');
                i += 1;
                continue;
            }
            if at(k) == Some(';') {
                k += 1;
            }
            out.push_str(&charref(num));
            i = k;
            continue;
        }
        let mut k = j;
        while k - j < 32 && at(k).is_some_and(|x| !matches!(x, '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';')) {
            k += 1;
        }
        if k == j {
            out.push('&');
            i += 1;
            continue;
        }
        if at(k) == Some(';') {
            k += 1;
        }
        let name: Vec<char> = cs.get(j..k).map(<[char]>::to_vec).unwrap_or_default();
        let full: String = name.iter().collect();
        if let Some(v) = html5_get(&full) {
            out.push_str(v);
        } else {
            // 找最長的、對得上的開頭（照標準的規定）
            let mut done = false;
            for x in (2..name.len()).rev() {
                let head: String = name.iter().take(x).collect();
                if let Some(v) = html5_get(&head) {
                    out.push_str(v);
                    out.extend(name.iter().skip(x));
                    done = true;
                    break;
                }
            }
            if !done {
                out.push('&');
                out.push_str(&full);
            }
        }
        i = k;
    }
    out
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut m = Map::new();
            for k in keys {
                m.insert(k.clone(), sort_keys(o.get(k).unwrap_or(&Value::Null)));
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// `json.dumps(v, ensure_ascii=False, indent=indent)`
pub(super) fn dumps(v: &Value, indent: Option<usize>) -> String {
    crate::recipe::dumps(v, indent)
}

/// `json.dumps(v, ensure_ascii=False, indent=indent, sort_keys=True)`
pub(super) fn dumps_sorted(v: &Value, indent: Option<usize>) -> String {
    crate::recipe::dumps(&sort_keys(v), indent)
}

/// `str(OSError)`：`[Errno N] 說明: '路徑'`。
pub(super) fn os_err(e: &std::io::Error, path: &str) -> String {
    crate::kb::pyish::PyErr::os(e, path).msg
}
