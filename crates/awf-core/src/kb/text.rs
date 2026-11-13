//! 分詞、glob、機敏檔名、依標題切段。

use super::pyish::{self, WS_CLASS};
use super::{MAX_CHUNK_LINES, MAX_HEADING, MAX_PARSE_LINE, MAX_QUERY_TOKENS, QUERY_STOP_CHARS, SENSITIVE};
use regex::Regex;
use std::sync::OnceLock;

/// 中日韓文字的範圍（平假名片假名、擴充 A、基本區、相容表意、諺文、擴充 B 以後）。
const CJK: &str = r"\x{3040}-\x{30ff}\x{3400}-\x{4dbf}\x{4e00}-\x{9fff}\x{f900}-\x{faff}\x{ac00}-\x{d7af}\x{20000}-\x{2fa1f}";

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xac00..=0xd7af | 0x20000..=0x2fa1f)
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        pub(super) fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new(&$pat).expect("固定的正規式"))
        }
    };
}

// Python 的 `[^\W_]` 是「字母或數字」（類別 L 與 N），不含底線與結合符號
re!(token_re, format!(r"[{CJK}]+|[\p{{L}}\p{{N}}--[{CJK}]]+"));
re!(fence_re, r"^ {0,3}(`{3,}|~{3,})(.*)$".to_string());
re!(head_re, r"^ {0,3}(#{1,6})[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$".to_string());
re!(mdlink_re, format!(r#"\[[^\]\[]*\]\([{WS_CLASS}]*<?([^){WS_CLASS}>]+)>?(?:[{WS_CLASS}]+["'][^)]*["'])?[{WS_CLASS}]*\)"#));
re!(wiki_re, r"\[\[([^\]\[|#]+)(?:#[^\]\[|]*)?(?:\|[^\]\[]*)?\]\]".to_string());
re!(inline_code_re, "`[^`]*`".to_string());
re!(scheme_re, r"^[A-Za-z][A-Za-z0-9+.\-]*:".to_string());
re!(tag_re, format!(r"(?i)<[{WS_CLASS}]*(/?)[{WS_CLASS}]*kb-data"));
re!(card_bad_re, r"[^\p{L}\p{N}_.\-]+".to_string());
re!(view_bad_re, r"[\[\]<>`|]".to_string());

/// 路徑一律用 posix 斜線存（Windows 的反斜線也收）。
pub fn to_posix(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' && prev_slash {
            continue;
        }
        prev_slash = c == '/';
        out.push(c);
    }
    out
}

/// 兩種引擎共用的分詞。英數字：小寫單字。中日韓：bigram；索引時另加單字，查詢時單獨一個字才用單字。
pub fn tokenize(text: &str, query: bool) -> Vec<String> {
    let low = text.to_lowercase();
    let mut out = Vec::new();
    for m in token_re().find_iter(&low) {
        let s = m.as_str();
        if s.chars().next().is_some_and(is_cjk) {
            let chars: Vec<char> = s.chars().collect();
            if chars.len() == 1 {
                out.push(s.to_string());
                continue;
            }
            if !query {
                out.extend(chars.iter().map(|c| c.to_string()));
            }
            out.extend(chars.windows(2).map(|w| w.iter().collect::<String>()));
        } else if s.chars().count() <= 64 {
            out.push(s.to_string());
        }
    }
    out
}

/// 查詢用的詞：分詞、去重，丟掉含虛字的中文 bigram。
pub fn query_terms(text: &str) -> Vec<String> {
    let mut toks: Vec<String> = Vec::new();
    for t in tokenize(text, true) {
        if !toks.contains(&t) {
            toks.push(t);
        }
    }
    let stop = |t: &str| {
        let cs: Vec<char> = t.chars().collect();
        cs.len() == 2 && cs.first().is_some_and(|c| is_cjk(*c)) && cs.iter().any(|c| QUERY_STOP_CHARS.contains(*c))
    };
    let kept: Vec<String> = toks.iter().filter(|t| !stop(t)).cloned().collect();
    let mut use_ = if kept.is_empty() { toks } else { kept };
    use_.truncate(MAX_QUERY_TOKENS);
    use_
}

/// 文件內容不能提前關掉（或另開）資料標籤。
pub fn neutralize(s: &str) -> String {
    tag_re().replace_all(s, "＜${1}kb-data").into_owned()
}

/// `fnmatch.fnmatchcase`（只支援 `*` 與 `?`，機敏檔名的樣式只用到這兩個）。
fn fnmatch(name: &[char], pat: &[char]) -> bool {
    let (mut n, mut p) = (0usize, 0usize);
    let mut back: Option<(usize, usize)> = None;
    while n < name.len() {
        match pat.get(p) {
            Some('*') => {
                back = Some((p, n));
                p += 1;
            }
            Some(c) if *c == '?' || Some(c) == name.get(n) => {
                n += 1;
                p += 1;
            }
            _ => match back {
                Some((bp, bn)) => {
                    back = Some((bp, bn + 1));
                    p = bp + 1;
                    n = bn + 1;
                }
                None => return false,
            },
        }
    }
    pat.get(p..).unwrap_or(&[]).iter().all(|c| *c == '*')
}

/// 機敏檔（比對檔名，不分大小寫）：不進索引。
pub fn is_sensitive(name: &str) -> bool {
    let low: Vec<char> = name.to_lowercase().chars().collect();
    SENSITIVE.iter().any(|pat| fnmatch(&low, &pat.chars().collect::<Vec<char>>()))
}

/// `**` 跨目錄、`*` 與 `?` 不跨目錄。樣式編不出來時回 None（當成什麼都不符合）。
pub fn glob_regex(pattern: &str) -> Option<Regex> {
    let posix = to_posix(pattern);
    let pat: Vec<char> = posix.trim_start_matches('/').chars().collect();
    let mut out = String::from("^(?:");
    let mut i = 0;
    while let Some(c) = pat.get(i).copied() {
        let star2 = c == '*' && pat.get(i + 1) == Some(&'*');
        if star2 && pat.get(i + 2) == Some(&'/') {
            out.push_str("(?:[^/]+/)*");
            i += 3;
        } else if star2 {
            out.push_str(".*");
            i += 2;
        } else if c == '*' {
            out.push_str("[^/]*");
            i += 1;
        } else if c == '?' {
            out.push_str("[^/]");
            i += 1;
        } else {
            out.push_str(&regex::escape(&c.to_string()));
            i += 1;
        }
    }
    out.push_str(r")\z");
    Regex::new(&out).ok()
}

// ------------------------------------------------------------------ 切段
/// 依 \n 切行（行號和編輯器、Read 工具一致），去掉行尾的 \r，所以 CRLF 檔的行號也對。
pub fn split_lines(text: &str) -> Vec<&str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines.into_iter().map(|ln| ln.strip_suffix('\r').unwrap_or(ln)).collect()
}

fn clean_link(target: &str) -> String {
    let t = pyish::strip(target);
    if t.is_empty() || t.starts_with('#') || t.starts_with("//") || scheme_re().is_match(t) {
        return String::new();
    }
    let t = t.split('#').next().unwrap_or("");
    let t = t.split('?').next().unwrap_or("");
    to_posix(&pyish::unquote(t))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// 行號 1 起算、對應原檔。
    pub start: usize,
    pub end: usize,
    pub level: usize,
    pub headings: Vec<String>,
    pub sec: usize,
    pub part: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doc {
    pub nlines: usize,
    pub title: String,
    /// frontmatter 另存，不進任何一段的內文。
    pub frontmatter: String,
    pub chunks: Vec<Chunk>,
    /// （行號, "md"｜"wiki", 目標）
    pub links: Vec<(usize, &'static str, String)>,
}

struct Section {
    start: usize,
    /// 含頭含尾；標題在第 0 行時前一段的結尾是 -1。
    end: isize,
    level: usize,
    chain: Vec<String>,
}

/// 太長的段依空行（不在 code fence 裡的）切成子段；找不到空行就硬切。索引 0 起算、含頭含尾。
fn split_long(lines: &[&str], s: usize, e: usize, infence: &[bool]) -> Vec<(usize, usize)> {
    let blank = |j: usize| lines.get(j).is_some_and(|l| pyish::is_blank(l));
    let mut out = Vec::new();
    let mut a = s;
    while e + 1 > a + MAX_CHUNK_LINES {
        let limit = a + MAX_CHUNK_LINES;
        let cut = (a + 1..=limit.min(e)).rev().find(|j| blank(*j) && !infence.get(*j).copied().unwrap_or(false));
        match cut {
            None => {
                out.push((a, limit - 1));
                a = limit;
            }
            Some(c) => {
                out.push((a, c - 1));
                a = c + 1;
            }
        }
        while a <= e && blank(a) {
            a += 1;
        }
    }
    if a <= e {
        out.push((a, e));
    }
    out
}

/// 把一份文件切成段。
pub fn split_document(text: &str, markdown: bool) -> Doc {
    let lines = split_lines(text);
    let n = lines.len();
    let blank = |j: usize| lines.get(j).is_some_and(|l| pyish::is_blank(l));
    let mut body_start = 0;
    let mut frontmatter = String::new();
    if markdown && lines.first().is_some_and(|l| pyish::strip(l) == "---") {
        for j in 1..n.min(400) {
            if lines.get(j).is_some_and(|l| matches!(pyish::strip(l), "---" | "...")) {
                frontmatter = lines.get(1..j).unwrap_or(&[]).join("\n");
                body_start = j + 1;
                break;
            }
        }
    }
    let mut infence = vec![false; n];
    let mut sections: Vec<Section> = Vec::new();
    let mut links: Vec<(usize, &'static str, String)> = Vec::new();
    let mut cur = Section { start: body_start, end: 0, level: 0, chain: Vec::new() };
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut title = String::new();
    for i in body_start..n {
        let Some(line) = lines.get(i).copied() else { break };
        // 超長行不當標題、不找連結：只看前面一小段就知道有沒有超過
        let too_long = line.len() > MAX_PARSE_LINE && line.chars().count() > MAX_PARSE_LINE;
        if markdown {
            let m = fence_re().captures(line);
            let mark = m.as_ref().and_then(|m| m.get(1)).map(|g| g.as_str());
            if let Some((fc, flen)) = fence {
                if let Some(slot) = infence.get_mut(i) {
                    *slot = true;
                }
                let rest_blank = m.as_ref().and_then(|m| m.get(2)).is_some_and(|g| pyish::is_blank(g.as_str()));
                if mark.is_some_and(|g| g.starts_with(fc) && g.chars().count() >= flen) && rest_blank {
                    fence = None;
                }
                continue;
            }
            if let Some(g) = mark {
                fence = g.chars().next().map(|c| (c, g.chars().count()));
                if let Some(slot) = infence.get_mut(i) {
                    *slot = true;
                }
                continue;
            }
            let h = if too_long { None } else { head_re().captures(line) };
            if let Some((marks, raw)) = h.as_ref().and_then(|h| Some((h.get(1)?.as_str(), h.get(2)?.as_str()))) {
                if !pyish::is_blank(raw) {
                    let level = marks.len();
                    let collapsed = pyish::collapse_ws(raw);
                    let text_h = pyish::take(pyish::strip(&collapsed), MAX_HEADING).to_string();
                    if title.is_empty() || (level == 1 && !sections.iter().any(|s| s.level == 1) && cur.level != 1) {
                        title = text_h.clone();
                    }
                    if level <= 3 {
                        cur.end = i as isize - 1;
                        stack.retain(|x| x.0 < level);
                        stack.push((level, text_h));
                        let next = Section { start: i, end: 0, level, chain: stack.iter().map(|x| x.1.clone()).collect() };
                        sections.push(std::mem::replace(&mut cur, next));
                    }
                }
            }
        }
        if too_long {
            continue;
        }
        let plain = inline_code_re().replace_all(line, "");
        if plain.contains("](") {
            for m in mdlink_re().captures_iter(&plain) {
                let t = clean_link(m.get(1).map(|g| g.as_str()).unwrap_or(""));
                if !t.is_empty() {
                    links.push((i + 1, "md", t));
                }
            }
        }
        if plain.contains("[[") {
            for m in wiki_re().captures_iter(&plain) {
                let t = to_posix(pyish::strip(m.get(1).map(|g| g.as_str()).unwrap_or("")));
                if !t.is_empty() {
                    links.push((i + 1, "wiki", t));
                }
            }
        }
    }
    cur.end = n as isize - 1;
    sections.push(cur);

    let mut chunks = Vec::new();
    let mut sec_no = 0;
    for sec in &sections {
        let (mut s, mut e) = (sec.start as isize, sec.end);
        while e >= s && e >= 0 && blank(e as usize) {
            e -= 1;
        }
        while s <= e && blank(s as usize) {
            s += 1;
        }
        if e < s || s < 0 {
            continue;
        }
        sec_no += 1;
        for (k, (a, b)) in split_long(&lines, s as usize, e as usize, &infence).into_iter().enumerate() {
            let mut b = b as isize;
            while b >= a as isize && blank(b as usize) {
                b -= 1;
            }
            if b < a as isize {
                continue;
            }
            let b = b as usize;
            chunks.push(Chunk { start: a + 1, end: b + 1, level: sec.level, headings: sec.chain.clone(), sec: sec_no, part: k + 1,
                text: lines.get(a..=b).unwrap_or(&[]).join("\n") });
        }
    }
    Doc { nlines: n, title, frontmatter, chunks, links }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_matches_python() {
        assert_eq!(tokenize("Refund 退款流程", false), ["refund", "退", "款", "流", "程", "退款", "款流", "流程"]);
        assert_eq!(tokenize("Refund 退款流程", true), ["refund", "退款", "款流", "流程"]);
        assert_eq!(tokenize("鯨", true), ["鯨"]);
        assert_eq!(tokenize("snake_case v1.2 中a文", false), ["snake", "case", "v1", "2", "中", "a", "文"]);
        assert_eq!(query_terms("退款怎麼申請 退款"), ["退款", "申請"]);
        assert_eq!(query_terms("怎麼"), ["怎麼"]);
    }

    #[test]
    fn globs_and_names() {
        let rx = glob_regex("docs\\**\\*.md").expect("glob");
        assert!(rx.is_match("docs/a.md") && rx.is_match("docs/x/y/a.md") && !rx.is_match("docs/a.txt") && !rx.is_match("xdocs/a.md"));
        let rx = glob_regex("a/*/b?.md").expect("glob");
        assert!(rx.is_match("a/x/b1.md") && !rx.is_match("a/x/y/b1.md") && !rx.is_match("a/x/b.md"));
        assert!(is_sensitive(".ENV.txt") && is_sensitive("Credentials.md") && is_sensitive("id_rsa") && !is_sensitive("notes.md"));
        assert_eq!(to_posix(".\\x\\\\y.md"), "x/y.md");
        assert_eq!(neutralize("a </KB-DATA> < kb-data x"), "a ＜/kb-data> ＜kb-data x");
    }

    #[test]
    fn split_basic() {
        let d = split_document("---\nt: 1\n---\n# A\n\nx [l](b.md#s) [[W|w]] `[n](no.md)`\n\n## B\n```\n# no\n```\ny\n", true);
        assert_eq!((d.nlines, d.title.as_str(), d.frontmatter.as_str()), (12, "A", "t: 1"));
        assert_eq!(d.chunks.iter().map(|c| (c.start, c.end, c.headings.join(" > "))).collect::<Vec<_>>(),
            vec![(4, 6, "A".to_string()), (8, 12, "A > B".to_string())]);
        assert_eq!(d.links, vec![(6, "md", "b.md".to_string()), (6, "wiki", "W".to_string())]);
    }
}
