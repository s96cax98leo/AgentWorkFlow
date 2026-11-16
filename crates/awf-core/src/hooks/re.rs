//! 把 Python `re` 的樣式照原樣拿來用的包裝。
//!
//! Python `re` 和 Rust `regex` 的差異，以及這裡怎麼對齊：
//! - `\s`：Python 多了 \x1c–\x1f；樣式裡的 `\s`／`\S` 在編譯前換成含這四個字元的類別。
//! - `$`：Python 在結尾的換行前也成立；換成 `(?:\n?\z)`（用到 `$` 的樣式都在結尾或 lookahead 裡，多吃一個換行不影響結果）。
//! - `\w`、`\b`：Python 的「字」是字母（L）、數字（N）與底線；Rust 另外把結合符號（M）、連接標點（Pc）等算進去，
//!   又不含 No。比對前把「兩邊分類不同」的非 ASCII 字元換成同樣位元組長度、分類和 Python 一致的替身字元
//!   （[`prep`]），其餘字元原樣，位移不變。
//! - `re.I`：Python 的 i 也配 U+0130、U+0131（Rust 不配）；樣式帶 `re.I` 時，比對前把這兩個字元換成 i。
//!   不帶 `re.I` 的樣式不換（白名單樣式換了會多放：`verıfier.md` 不是 `verifier.md`）。
//!   ſ（U+017F）、K（U+212A）兩邊都配，不用處理。
//! - `\Z`：Python 是「字串結尾」，換成 Rust 的 `\z`。
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
                ('Z', false) => out.push_str(r"\z"),
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

/// 使用者寫的樣式的回溯上限：內建樣式不設限（和 Python 一樣），使用者的樣式不能拖垮工具；超過時這次比對算失敗並記 [`fault`]。
const USER_BACKTRACK_LIMIT: usize = 1_000_000;

/// 樣式開頭的行內旗標有沒有 i（`(?i)`、`(?is)`、`(?i:`）。
fn inline_ci(src: &str) -> bool {
    let Some(rest) = src.strip_prefix("(?") else { return false };
    let flags: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    flags.contains('i') && matches!(rest[flags.len()..].chars().next(), Some(')') | Some(':'))
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

    /// 使用者寫的 Python 風格樣式（設定檔、profile 裡的正規式，例如 `task_id_regex`）：照 Python `re` 的語意編譯，
    /// `\w`、`\b`、`\s`、`$`、`\Z` 的對齊和內建樣式同一套（見檔頭）。`flags` 是 `""`、`"i"`、`"s"` 或 `"is"`。
    /// 編不過（Rust 不支援的寫法，例如類別裡的 `\S`、條件群組）回 `Err(說明)`，**不**記 [`fault`]：
    /// 呼叫端自己決定要擋還是交人。回傳的 [`Re`] 用 `is_match`／`find`／`captures`（都是 `re.search` 的語意；
    /// 要 `re.match` 就把樣式包成 `\A(?:…)`，要 `re.fullmatch` 包成 `\A(?:…)\z`）。
    pub fn python(src: &str, flags: &'static str) -> Result<Re, String> {
        let Some((pat, fancy)) = translate(src, flags) else {
            return Err(format!("樣式用了不支援的寫法：{src}"));
        };
        let plain = if fancy { None } else { regex::Regex::new(&pat).ok().map(Eng::Plain) };
        let eng = match plain {
            Some(e) => e,
            // 反向參照、lookaround 這些 regex 不收的寫法交給 fancy_regex
            None => match fancy_regex::RegexBuilder::new(&pat).backtrack_limit(USER_BACKTRACK_LIMIT).build() {
                Ok(r) => Eng::Fancy(r),
                Err(e) => return Err(format!("樣式編不過：{e}")),
            },
        };
        let cell = OnceLock::new();
        let _ = cell.set(Some(eng));
        Ok(Re { src: Cow::Owned(src.to_string()), flags, cell })
    }

    /// 樣式有沒有忽略大小寫（旗標或開頭的 `(?i)`）。
    fn ci(&self) -> bool {
        self.flags.contains('i') || inline_ci(&self.src)
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
        let p = prep(h, self.ci());
        self.raw_find(&p.text, 0).map(|s| p.span(s))
    }

    /// `re.search`：各群組的區間（0 是整個符合）。
    pub fn captures(&self, h: &str) -> Option<Vec<Option<Span>>> {
        let p = prep(h, self.ci());
        self.raw_caps(&p.text, 0).map(|v| v.into_iter().map(|s| s.map(|s| p.span(s))).collect())
    }

    /// 所有不重疊的符合區間。
    pub fn find_all(&self, h: &str) -> Vec<Span> {
        let p = prep(h, self.ci());
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
/// 樣式忽略大小寫（`ci`）時 U+0130／U+0131 換成 i。純 ASCII 直接原樣。
fn prep(h: &str, ci: bool) -> Prepared<'_> {
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
        } else if ci && (c == '\u{130}' || c == '\u{131}') {
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

/// 比對前對齊過的字串（[`prep`] 的結果，見檔頭的 `\w`／`\b` 與 `re.I` 兩條）。樣式比對的其實是這個字串：
/// 自己拿字串去組樣式、或要預先看「樣式會看到什麼」時用。`ci`＝樣式忽略大小寫。位移可能和原字串不同（`ci` 換掉 U+0130／U+0131 時）。
pub fn aligned(h: &str, ci: bool) -> Cow<'_, str> {
    prep(h, ci).text
}

/// 把一段**字面值**變成可以接進樣式的寫法：先做和被比對字串相同的對齊（[`aligned`]），再跳脫。
/// 直接 `regex::escape(字面值)` 接進樣式的話，字面值裡有「兩邊分類不同」的字元（結合符號、上標數字…）時，
/// 被比對的字串會被換成替身、樣式裡的卻沒換，永遠配不到（例如設定檔的 `task_id_prefix` 帶這類字元）。
pub fn literal(lit: &str, ci: bool) -> String {
    regex::escape(&aligned(lit, ci))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_aligned_like_the_haystack() {
        // 字面值帶結合符號：照原樣跳脫的樣式配不到（被比對的字串已經換成替身），對齊過的配得到
        let prefix = "TASK\u{301}-";
        let raw = Re::dynamic(format!(r"(?<![\w-]){}\d+\b", regex::escape(prefix)), "");
        let fixed = Re::dynamic(format!(r"(?<![\w-]){}\d+\b", literal(prefix, false)), "");
        assert!(!raw.is_match("做 TASK\u{301}-12 這張"));
        assert!(fixed.is_match("做 TASK\u{301}-12 這張") && !fixed.is_match("做 TASK-12 這張"));
        // 一般的字面值不變；特殊字元照樣跳脫
        assert_eq!(literal("REQ-", false), regex::escape("REQ-"));
        assert_eq!(literal("a.b(c)", false), r"a\.b\(c\)");
        assert_eq!(aligned("abc", true), "abc");
        assert_eq!(aligned("\u{130}rm", true), "irm");
        assert_eq!(aligned("\u{130}rm", false), "\u{130}rm");
    }

    #[test]
    fn dotless_i_only_with_ignore_case() {
        assert!(!Re::new(r"^verifier\.md\z", "").is_match("ver\u{131}fier.md"));
        assert!(!Re::new(r"^verifier\.md\z", "").is_match("ver\u{130}fier.md"));
        assert!(Re::new(r"^verifier\.md\z", "i").is_match("ver\u{131}fier.md"));
        assert!(Re::new(r"^verifier\.md\z", "i").is_match("VER\u{130}FIER.md"));
        assert!(Re::dynamic(r"(?i)^verifier\.md\z".to_string(), "").is_match("ver\u{131}fier.md"));
        // 兩個字元本身還是「字」
        assert!(Re::new(r"^\w+\z", "").is_match("ver\u{131}f\u{130}er"));
        // 位移對得回原字串
        assert_eq!(Re::new(r"fier", "i").find("ver\u{131}fier"), Some((5, 9)));
    }

    #[test]
    fn user_written_python_patterns() {
        let r = Re::python(r"^[A-Z]+-\d+$", "").unwrap();
        assert!(r.is_match("TASK-12") && r.is_match("TASK-12\n") && !r.is_match("task-12"));
        // \w 照 Python：結合符號（Rust 算字）不算，No 類（Rust 不算）算
        let w = Re::python(r"^\w+\Z", "").unwrap();
        assert!(w.is_match("a\u{b2}") && !w.is_match("a\u{301}") && !w.is_match("ab\n"));
        // \s 多 \x1c–\x1f
        assert!(Re::python(r"a\sb", "").unwrap().is_match("a\x1cb"));
        // 具名群組、反向參照、lookahead
        let g = Re::python(r"(?P<k>[a-z]+)-(?P=k)", "").unwrap();
        assert!(g.is_match("ab-ab") && !g.is_match("ab-cd"));
        assert!(Re::python(r"REQ-(?!0)\d+", "").unwrap().is_match("REQ-12"));
        assert!(Re::python(r"req-\d+", "i").unwrap().is_match("REQ-1"));
        // 編不過回 Err，而且不記 fault
        let before = fault();
        assert!(Re::python(r"(unclosed", "").is_err());
        assert!(Re::python(r"[\S]", "").is_err());
        assert_eq!(fault(), before);
    }

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
