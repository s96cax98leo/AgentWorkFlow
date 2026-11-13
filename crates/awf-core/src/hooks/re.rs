//! 把 Python `re` 的樣式照原樣拿來用的包裝。
//!
//! Python `re` 和 Rust `regex` 的差異，以及這裡怎麼對齊：
//! - `\s`：Python 多了 \x1c–\x1f；樣式裡的 `\s`／`\S` 在編譯前換成含這四個字元的類別。
//! - `$`：Python 在結尾的換行前也成立；換成 `(?:\n?\z)`（用到 `$` 的樣式都在結尾或 lookahead 裡，多吃一個換行不影響結果）。
//! - `\w`、`\b`：Python 的「字」是字母（L）、數字（N）與底線；Rust 另外把結合符號（M）、連接標點（Pc）等算進去，
//!   又不含 No。比對前把「兩邊分類不同」的非 ASCII 字元換成同樣位元組長度、分類和 Python 一致的替身字元
//!   （[`prep`]），其餘字元原樣，位移不變。
//! - `re.I`：Python 的 i 也配 U+0130、U+0131（Rust 不配）；比對前把這兩個字元換成 i。換了以後不帶 `re.I` 的樣式
//!   也會把它們當成 i，只會多擋。ſ（U+017F）、K（U+212A）兩邊都配，不用處理。
//! - lookahead／lookbehind：Rust `regex` 沒有，含這些的樣式用 `fancy_regex`（回溯上限調到最大，和 Python 一樣不設限）。
//!
//! 樣式編譯失敗或執行失敗時記在 [`fault`]，由最外層當成意外錯誤處理（守門類擋下），不會默默放行。

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static FAULT: AtomicBool = AtomicBool::new(false);

/// 記一筆「判定不可信」（樣式以外的地方也用，例如巢狀深度超過上限）。
pub fn mark_fault() {
    FAULT.store(true, Ordering::Relaxed);
}

/// 有樣式編譯或執行失敗（判定不可信）。
pub fn fault() -> bool {
    FAULT.load(Ordering::Relaxed)
}

enum Eng {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

pub struct Re {
    src: Cow<'static, str>,
    flags: &'static str,
    cell: OnceLock<Option<Eng>>,
}

/// 把 Python 樣式換成 Rust 的寫法；回傳（樣式, 是否需要 fancy_regex）。
fn translate(src: &str, flags: &str) -> Option<(String, bool)> {
    let mut out = String::with_capacity(src.len() + 16);
    if flags.contains('i') {
        out.push_str("(?i)");
    }
    if flags.contains('s') {
        out.push_str("(?s)");
    }
    let fancy = src.contains("(?=") || src.contains("(?!") || src.contains("(?<=") || src.contains("(?<!");
    let mut in_class = false;
    let mut class_start = false;
    let mut it = src.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            let n = it.next()?;
            match (n, in_class) {
                ('s', false) => out.push_str(r"[\s\x1c-\x1f]"),
                ('s', true) => out.push_str(r"\s\x1c-\x1f"),
                ('S', false) => out.push_str(r"[^\s\x1c-\x1f]"),
                ('S', true) => return None,
                _ => {
                    out.push('\\');
                    out.push(n);
                }
            }
            class_start = false;
            continue;
        }
        if in_class {
            if c == ']' && !class_start {
                in_class = false;
            }
            if c == '[' {
                out.push('\\'); // Python 的類別裡 [ 是字面值；Rust 會當成巢狀類別
            }
            if !(class_start && c == '^') {
                class_start = false;
            }
            out.push(c);
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                class_start = true;
                out.push(c);
            }
            '$' => out.push_str(r"(?:\n?\z)"),
            _ => out.push(c),
        }
    }
    Some((out, fancy))
}

/// 一段區間（原字串的位元組位移）。
pub type Span = (usize, usize);

impl Re {
    pub const fn new(src: &'static str, flags: &'static str) -> Self {
        Re { src: Cow::Borrowed(src), flags, cell: OnceLock::new() }
    }

    pub fn dynamic(src: String, flags: &'static str) -> Self {
        Re { src: Cow::Owned(src), flags, cell: OnceLock::new() }
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    fn eng(&self) -> Option<&Eng> {
        let e = self.cell.get_or_init(|| {
            let (pat, fancy) = translate(&self.src, self.flags)?;
            if fancy {
                fancy_regex::RegexBuilder::new(&pat).backtrack_limit(usize::MAX).build().ok().map(Eng::Fancy)
            } else {
                regex::Regex::new(&pat).ok().map(Eng::Plain)
            }
        });
        if e.is_none() {
            FAULT.store(true, Ordering::Relaxed);
        }
        e.as_ref()
    }

    /// 樣式編得過（測試用）。
    pub fn compiles(&self) -> bool {
        self.cell.get_or_init(|| {
            let (pat, fancy) = translate(&self.src, self.flags)?;
            if fancy {
                fancy_regex::RegexBuilder::new(&pat).backtrack_limit(usize::MAX).build().ok().map(Eng::Fancy)
            } else {
                regex::Regex::new(&pat).ok().map(Eng::Plain)
            }
        }).is_some()
    }

    fn raw_caps(&self, t: &str, from: usize) -> Option<Vec<Option<Span>>> {
        match self.eng()? {
            Eng::Plain(r) => {
                let mut locs = r.capture_locations();
                r.captures_read_at(&mut locs, t, from)?;
                Some((0..locs.len()).map(|i| locs.get(i)).collect())
            }
            Eng::Fancy(r) => match r.captures_from_pos(t, from) {
                Ok(Some(c)) => Some((0..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect()),
                Ok(None) => None,
                Err(_) => {
                    FAULT.store(true, Ordering::Relaxed);
                    None
                }
            },
        }
    }

    fn raw_find(&self, t: &str, from: usize) -> Option<Span> {
        match self.eng()? {
            Eng::Plain(r) => r.find_at(t, from).map(|m| (m.start(), m.end())),
            Eng::Fancy(r) => match r.find_from_pos(t, from) {
                Ok(m) => m.map(|m| (m.start(), m.end())),
                Err(_) => {
                    FAULT.store(true, Ordering::Relaxed);
                    None
                }
            },
        }
    }

    /// `re.search(...) is not None`。
    pub fn is_match(&self, h: &str) -> bool {
        self.find(h).is_some()
    }

    /// `re.search`：第一個符合的區間。
    pub fn find(&self, h: &str) -> Option<Span> {
        let p = prep(h);
        self.raw_find(&p.text, 0).map(|s| p.span(s))
    }

    /// `re.search`：各群組的區間（0 是整個符合）。
    pub fn captures(&self, h: &str) -> Option<Vec<Option<Span>>> {
        let p = prep(h);
        self.raw_caps(&p.text, 0).map(|v| v.into_iter().map(|s| s.map(|s| p.span(s))).collect())
    }

    /// 所有不重疊的符合區間。
    pub fn find_all(&self, h: &str) -> Vec<Span> {
        let p = prep(h);
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= p.text.len() {
            let Some((s, e)) = self.raw_find(&p.text, pos) else { break };
            out.push(p.span((s, e)));
            if e > s {
                pos = e;
            } else {
                // 空符合：往前一個字元
                pos = match p.text[e..].chars().next() {
                    Some(c) => e + c.len_utf8(),
                    None => break,
                };
            }
        }
        out
    }

    /// `re.sub(樣式, 固定字串, h)`。
    pub fn replace_all(&self, h: &str, rep: &str) -> String {
        let spans = self.find_all(h);
        if spans.is_empty() {
            return h.to_string();
        }
        let mut out = String::with_capacity(h.len());
        let mut last = 0;
        for (s, e) in spans {
            out.push_str(h.get(last..s).unwrap_or(""));
            out.push_str(rep);
            last = e;
        }
        out.push_str(h.get(last..).unwrap_or(""));
        out
    }

    /// `re.split(樣式, h)`（樣式沒有群組）。
    pub fn split<'a>(&self, h: &'a str) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut last = 0;
        for (s, e) in self.find_all(h) {
            if e == s {
                continue;
            }
            out.push(h.get(last..s).unwrap_or(""));
            last = e;
        }
        out.push(h.get(last..).unwrap_or(""));
        out
    }
}

/// 群組文字；群組沒參與回 None。
pub fn group<'a>(h: &'a str, caps: &[Option<Span>], i: usize) -> Option<&'a str> {
    caps.get(i).copied().flatten().and_then(|(s, e)| h.get(s..e))
}

struct Prepared<'a> {
    text: Cow<'a, str>,
    /// 換過字元長度時：text 的位元組位移 → 原字串的位元組位移（長度是 text.len()+1）。
    map: Option<Vec<usize>>,
}

impl Prepared<'_> {
    fn span(&self, (s, e): Span) -> Span {
        match &self.map {
            Some(m) => (m.get(s).copied().unwrap_or(0), m.get(e).copied().unwrap_or(0)),
            None => (s, e),
        }
    }
}

fn class_res() -> Option<&'static (regex::Regex, regex::Regex)> {
    static C: OnceLock<Option<(regex::Regex, regex::Regex)>> = OnceLock::new();
    let c = C.get_or_init(|| {
        // Rust 算字、Python 不算；Python 算字、Rust 不算
        let a = regex::Regex::new(r"^[\w--[\p{L}\p{N}_]]$").ok()?;
        let b = regex::Regex::new(r"^[[\p{L}\p{N}]--\w]$").ok()?;
        Some((a, b))
    });
    if c.is_none() {
        FAULT.store(true, Ordering::Relaxed);
    }
    c.as_ref()
}

/// 比對前的字串：非 ASCII 字元裡，Python 與 Rust 對「是不是字」看法不同的換成替身（同位元組長度），
/// U+0130／U+0131 換成 i。純 ASCII 直接原樣。
fn prep(h: &str) -> Prepared<'_> {
    if h.is_ascii() {
        return Prepared { text: Cow::Borrowed(h), map: None };
    }
    let Some((rust_only, py_only)) = class_res() else {
        return Prepared { text: Cow::Borrowed(h), map: None };
    };
    let mut text = String::with_capacity(h.len());
    let mut map: Vec<usize> = Vec::new();
    let mut changed_len = false;
    let mut changed = false;
    let mut buf = [0u8; 4];
    for (i, c) in h.char_indices() {
        let rep = if c.is_ascii() {
            c
        } else if c == '\u{130}' || c == '\u{131}' {
            changed_len = true;
            'i'
        } else {
            let s: &str = c.encode_utf8(&mut buf);
            if rust_only.is_match(s) {
                // Python 不算字：換成同長度的標點／符號
                match c.len_utf8() {
                    2 => '\u{a1}',
                    3 => '\u{3001}',
                    _ => '\u{1f000}',
                }
            } else if py_only.is_match(s) {
                // Python 算字：換成同長度、沒有大小寫的字母
                match c.len_utf8() {
                    2 => '\u{1bb}',
                    3 => '\u{4e00}',
                    _ => '\u{20000}',
                }
            } else {
                c
            }
        };
        if rep != c {
            changed = true;
        }
        for _ in 0..rep.len_utf8() {
            map.push(i);
        }
        text.push(rep);
    }
    map.push(h.len());
    if !changed {
        return Prepared { text: Cow::Borrowed(h), map: None };
    }
    Prepared { text: Cow::Owned(text), map: if changed_len { Some(map) } else { None } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_and_space() {
        let r = Re::new(r"^a\s+b$", "");
        assert!(r.is_match("a \x1c b"));
        assert!(r.is_match("a b\n"));
        assert!(!r.is_match("a b\n\n"));
        let r = Re::new(r"x(?=\s|$)", "");
        assert!(r.is_match("x"));
        assert!(r.is_match("ax\tb"));
        assert!(!r.is_match("xy"));
    }

    #[test]
    fn word_boundary_like_python() {
        let r = Re::new(r"\bmerge\b", "i");
        // 結合符號在 Python 不算字：merge 後面接 U+0301 仍有邊界
        assert!(r.is_match("gh pr merge\u{301}"));
        // 上標數字（No）在 Python 算字：沒有邊界
        assert!(!r.is_match("merge\u{b2}"));
        assert!(r.is_match("合併 MERGE 完成"));
        assert!(!r.is_match("emerged"));
        let r = Re::new(r"\birm\b", "i");
        assert!(r.is_match("\u{130}rm x"));
        assert_eq!(r.find("a \u{131}rm x"), Some((2, 6)));
    }

    #[test]
    fn replace_and_split() {
        let r = Re::new(r"\d?>>?\s*/dev/null", "i");
        assert_eq!(r.replace_all("a 2>/dev/null b >> /dev/null", " "), "a   b  ");
        let r = Re::new(r"[;&|\n]+", "");
        assert_eq!(r.split("a;b&&c"), vec!["a", "b", "c"]);
        let r = Re::new(r"[a[b]+", "");
        assert_eq!(r.find("x[ab]"), Some((1, 4)));
    }
}
