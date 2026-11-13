//! 從 Python 原始碼找出 import 的模組名（供應鏈檢查用），取代 Python 版的 `ast.parse` ＋ `ast.walk`。
//!
//! 不是完整的 Python 解析器：先切成語彙（名字、字串、數字、符號；跳過註解與續行），再照關鍵字找：
//!   - `import a.b [as x], c`：每個模組一筆。
//!   - `from a.b import x [as y], z`：`a.b` 一筆，另外每個名字一筆 `a.b.x`（`from http import client` 要看得到
//!     `http.client`）。相對 import（開頭有點）不列。
//!   - `__import__("x")`、`<任何東西>.import_module("x")`：第一個參數是字串常數時列一筆。
//!
//! `import` 是 Python 的保留字，字串與註解以外出現一定是 import 陳述，所以照關鍵字找不會漏。
//! 和 `ast` 不同的地方（都只會更嚴）：
//!   - 抓得到的語法錯誤只有沒收尾的字串與不成對的括號；其他語法錯誤由 `tests/check_compile.py` 把關。
//!   - f-string 大括號裡的運算式照一般程式碼再切一次（裡面的動態載入也列）；切不動時回錯誤。
//!   - 結果照原始碼的順序，不是 `ast.walk` 的廣度優先順序。

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    /// 一般字串（含 r、u 前綴）的值。
    Str(String),
    /// bytes、f-string 等不是字串常數的字面值。
    OtherStr,
    Num,
    Op(char),
    /// 一行陳述的結尾（括號外的換行）。
    Newline,
}

struct Lexed {
    tok: Tok,
    line: usize,
}

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_name_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// 解開一般字串裡的跳脫。認不得的跳脫照 Python 的做法保留反斜線。
fn unescape(raw: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let hex = |from: usize, n: usize| -> Option<char> {
        let digits: String = raw.get(from..from + n)?.iter().collect();
        u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32)
    };
    while let Some(&c) = raw.get(i) {
        i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&e) = raw.get(i) else {
            out.push('\\');
            break;
        };
        i += 1;
        match e {
            '\n' => {}
            '\\' | '\'' | '"' => out.push(e),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0'..='7' => {
                let mut v = e.to_digit(8).unwrap_or(0);
                let mut n = 1;
                while n < 3 {
                    match raw.get(i).and_then(|d| d.to_digit(8)) {
                        Some(d) => {
                            v = v * 8 + d;
                            i += 1;
                            n += 1;
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            'x' | 'u' | 'U' => {
                let n = match e {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                match hex(i, n) {
                    Some(ch) => {
                        out.push(ch);
                        i += n;
                    }
                    None => {
                        out.push('\\');
                        out.push(e);
                    }
                }
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// f-string 裡每個 `{…}` 的內容（運算式，連同轉換與格式說明）。`{{`、`}}` 是一般字元。
fn fstring_fields(raw: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = raw.get(i) {
        if c == '{' && raw.get(i + 1) == Some(&'{') {
            i += 2;
        } else if c == '{' {
            let start = i + 1;
            let mut depth = 1;
            i += 1;
            while let Some(&d) = raw.get(i) {
                match d {
                    '{' | '[' | '(' => depth += 1,
                    '}' | ']' | ')' => depth -= 1,
                    '"' | '\'' => {
                        // 運算式裡的字串：跳到同一種引號收尾
                        i += 1;
                        while raw.get(i).is_some_and(|x| *x != d) {
                            i += 1;
                        }
                    }
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                i += 1;
            }
            out.push(raw.get(start..i.min(raw.len())).unwrap_or_default().iter().collect());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn lex(src: &str) -> Result<Vec<Lexed>, String> {
    let s: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut stack: Vec<(char, usize)> = Vec::new();
    while let Some(&c) = s.get(i) {
        if c == '\n' {
            if stack.is_empty() {
                out.push(Lexed { tok: Tok::Newline, line });
            }
            line += 1;
            i += 1;
        } else if c == '\\' && s.get(i + 1) == Some(&'\n') {
            line += 1;
            i += 2;
        } else if c.is_whitespace() || c == '\\' {
            i += 1;
        } else if c == '#' {
            while s.get(i).is_some_and(|x| *x != '\n') {
                i += 1;
            }
        } else if is_name_start(c) || c == '"' || c == '\'' {
            let start = i;
            while s.get(i).is_some_and(|x| is_name_char(*x)) {
                i += 1;
            }
            let word: String = s.get(start..i).unwrap_or_default().iter().collect();
            let quote = match s.get(i) {
                Some(&q) if q == '"' || q == '\'' => q,
                _ => {
                    out.push(Lexed { tok: Tok::Name(word), line });
                    continue;
                }
            };
            let prefix = word.to_ascii_lowercase();
            if !matches!(prefix.as_str(), "" | "r" | "u" | "b" | "f" | "t" | "br" | "rb" | "fr" | "rf" | "tr" | "rt") {
                // 名字後面直接接引號：不是合法的字串前綴，名字照收，字串下一輪再讀
                out.push(Lexed { tok: Tok::Name(word), line });
                continue;
            }
            let at = line;
            let triple = s.get(i + 1) == Some(&quote) && s.get(i + 2) == Some(&quote);
            i += if triple { 3 } else { 1 };
            let body = i;
            let end;
            loop {
                match s.get(i) {
                    None => return Err(format!("第 {at} 行的字串沒有收尾")),
                    Some('\\') => {
                        if s.get(i + 1) == Some(&'\n') {
                            line += 1;
                        }
                        i += 2;
                    }
                    Some('\n') if !triple => return Err(format!("第 {at} 行的字串沒有收尾")),
                    Some('\n') => {
                        line += 1;
                        i += 1;
                    }
                    Some(&q) if q == quote => {
                        if !triple {
                            end = i;
                            i += 1;
                            break;
                        }
                        if s.get(i + 1) == Some(&quote) && s.get(i + 2) == Some(&quote) {
                            end = i;
                            i += 3;
                            break;
                        }
                        i += 1;
                    }
                    Some(_) => i += 1,
                }
            }
            let raw = s.get(body..end.min(s.len())).unwrap_or_default();
            let tok = if prefix.contains('f') || prefix.contains('t') {
                // f-string 本身不是字串常數；大括號裡的運算式照一般程式碼再切一次，接在後面（裡面可能有動態載入）
                out.push(Lexed { tok: Tok::OtherStr, line: at });
                for field in fstring_fields(raw) {
                    let inner = lex(&field).map_err(|e| format!("第 {at} 行的 f-string 看不懂（{e}）"))?;
                    out.extend(inner.into_iter().filter(|t| t.tok != Tok::Newline).map(|t| Lexed { tok: t.tok, line: at }));
                }
                Tok::OtherStr
            } else if prefix.contains('b') {
                Tok::OtherStr
            } else if prefix.contains('r') {
                Tok::Str(raw.iter().collect())
            } else {
                Tok::Str(unescape(raw))
            };
            out.push(Lexed { tok, line: at });
        } else if c.is_ascii_digit() || (c == '.' && s.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let hexish = c == '0' && matches!(s.get(i + 1), Some('x' | 'X'));
            i += 1;
            while let Some(&d) = s.get(i) {
                let exp_sign = !hexish && (d == '+' || d == '-') && matches!(s.get(i - 1), Some('e' | 'E'));
                if is_name_char(d) || d == '.' || exp_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Lexed { tok: Tok::Num, line });
        } else {
            match c {
                '(' | '[' | '{' => stack.push((c, line)),
                ')' | ']' | '}' => {
                    let open = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    match stack.pop() {
                        Some((o, _)) if o == open => {}
                        _ => return Err(format!("第 {line} 行的 {c} 沒有對應的開頭")),
                    }
                }
                _ => {}
            }
            out.push(Lexed { tok: Tok::Op(c), line });
            i += 1;
        }
    }
    if let Some((c, at)) = stack.pop() {
        return Err(format!("第 {at} 行的 {c} 沒有收尾"));
    }
    Ok(out)
}

fn name_at(toks: &[Lexed], i: usize) -> Option<&str> {
    match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Name(n)) => Some(n.as_str()),
        _ => None,
    }
}

fn op_at(toks: &[Lexed], i: usize, c: char) -> bool {
    matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Op(x)) if *x == c)
}

/// 從 `i` 讀一個 `a.b.c`；回（名字、下一個位置）。
fn dotted(toks: &[Lexed], mut i: usize) -> Option<(String, usize)> {
    let mut name = name_at(toks, i)?.to_string();
    i += 1;
    while op_at(toks, i, '.') {
        let Some(part) = name_at(toks, i + 1) else { break };
        name.push('.');
        name.push_str(part);
        i += 2;
    }
    Some((name, i))
}

/// `from … import` 的 `from` 在哪裡：往回只能是名字與點；回（from 的位置、模組名、開頭的點數）。
fn from_head(toks: &[Lexed], import_at: usize) -> Option<(usize, String, usize)> {
    let mut j = import_at;
    while j > 0 {
        j -= 1;
        match &toks.get(j)?.tok {
            Tok::Name(n) if n == "from" => {
                let parts = toks.get(j + 1..import_at)?;
                if parts.is_empty() {
                    return None;
                }
                let level = parts.iter().take_while(|t| t.tok == Tok::Op('.')).count();
                let module: String = parts.iter().skip(level).map(|t| match &t.tok {
                    Tok::Name(n) => n.as_str(),
                    _ => ".",
                }).collect();
                return Some((j, module, level));
            }
            Tok::Name(_) | Tok::Op('.') => {}
            _ => return None,
        }
    }
    None
}

/// 呼叫的第一個參數是不是字串常數（可以多包幾層括號、相鄰字串相接）；是就回它的值。
fn first_string_arg(toks: &[Lexed], mut i: usize) -> Option<String> {
    let mut depth = 0;
    while op_at(toks, i, '(') {
        depth += 1;
        i += 1;
    }
    let mut value = String::new();
    let mut any = false;
    while let Some(Tok::Str(s)) = toks.get(i).map(|t| &t.tok) {
        value.push_str(s);
        any = true;
        i += 1;
    }
    if !any {
        return None;
    }
    while depth > 0 && op_at(toks, i, ')') {
        depth -= 1;
        i += 1;
    }
    (depth == 0 && (op_at(toks, i, ',') || op_at(toks, i, ')'))).then_some(value)
}

/// 回（行號、完整模組名），照原始碼的順序；解析不了回說明。
pub fn imports(src: &str) -> Result<Vec<(usize, String)>, String> {
    if src.contains('\0') {
        return Err("原始碼裡有 NUL 字元".into());
    }
    let toks = lex(src)?;
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let Tok::Name(word) = &t.tok else { continue };
        if word == "import" {
            if let Some((from_at, module, level)) = from_head(&toks, i) {
                if level > 0 {
                    continue;
                }
                let line = toks.get(from_at).map(|t| t.line).unwrap_or(t.line);
                out.push((line, module.clone()));
                let mut j = i + 1;
                if op_at(&toks, j, '(') {
                    j += 1;
                }
                loop {
                    if op_at(&toks, j, '*') {
                        out.push((line, format!("{module}.*")));
                        j += 1;
                    } else if let Some(n) = name_at(&toks, j) {
                        out.push((line, format!("{module}.{n}")));
                        j += 1;
                        if name_at(&toks, j) == Some("as") {
                            j += 2;
                        }
                    } else {
                        break;
                    }
                    if !op_at(&toks, j, ',') {
                        break;
                    }
                    j += 1;
                }
            } else {
                let mut j = i + 1;
                while let Some((name, next)) = dotted(&toks, j) {
                    out.push((t.line, name));
                    j = next;
                    if name_at(&toks, j) == Some("as") {
                        j += 2;
                    }
                    if !op_at(&toks, j, ',') {
                        break;
                    }
                    j += 1;
                }
            }
        } else if (word == "__import__" || word == "import_module") && op_at(&toks, i + 1, '(') {
            if let Some(value) = first_string_arg(&toks, i + 2) {
                // 行號取整個呼叫式的開頭（`importlib.import_module(` 的 importlib）
                let mut start = i;
                while start >= 2 && op_at(&toks, start - 1, '.') && name_at(&toks, start - 2).is_some() {
                    start -= 2;
                }
                out.push((toks.get(start).map(|t| t.line).unwrap_or(t.line), value));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::imports;

    fn got(src: &str) -> Vec<(usize, String)> {
        imports(src).expect("解析得了")
    }

    fn pairs(xs: &[(usize, &str)]) -> Vec<(usize, String)> {
        xs.iter().map(|(l, m)| (*l, m.to_string())).collect()
    }

    #[test]
    fn plain_and_from_imports() {
        assert_eq!(got("import os, sys as s\nimport a.b.c\n"), pairs(&[(1, "os"), (1, "sys"), (2, "a.b.c")]));
        assert_eq!(got("from http import client, server as s\n"), pairs(&[(1, "http"), (1, "http.client"), (1, "http.server")]));
        assert_eq!(got("from a.b import (\n  x,\n  y as z,\n)\n"), pairs(&[(1, "a.b"), (1, "a.b.x"), (1, "a.b.y")]));
        assert_eq!(got("from os import *\n"), pairs(&[(1, "os"), (1, "os.*")]));
        assert_eq!(got("try: import x\nexcept ImportError: pass\nif 1: import y; import z\n"), pairs(&[(1, "x"), (3, "y"), (3, "z")]));
    }

    #[test]
    fn relative_imports_are_skipped() {
        assert!(got("from . import x\nfrom .mod import y\nfrom ..a.b import z\nfrom ... import w\n").is_empty());
    }

    #[test]
    fn strings_and_comments_do_not_count() {
        let src = "# import socket\nx = 'import socket'\ny = \"\"\"\nimport socket\nfrom a import b\n\"\"\"\nz = r'\\\\'\nimport json\n";
        assert_eq!(got(src), pairs(&[(8, "json")]));
        assert_eq!(got("def f():\n    yield from g()\n    raise E from e\nimport re\n"), pairs(&[(4, "re")]));
    }

    #[test]
    fn dynamic_imports() {
        assert_eq!(got("m = __import__('socket')\n"), pairs(&[(1, "socket")]));
        assert_eq!(got("import importlib\nm = importlib.import_module(\n  \"http\" \".client\", None)\n"), pairs(&[(1, "importlib"), (2, "http.client")]));
        assert_eq!(got("m = __import__((\"req\\x75ests\"))\n"), pairs(&[(1, "requests")]));
        // 不是字串常數的參數 ast 也不列
        assert!(got("m = __import__(name)\nn = __import__('a' + b)\no = __import__(b'x')\ndef import_module(name): pass\n").is_empty());
    }

    #[test]
    fn fstring_fields_are_scanned() {
        assert_eq!(got("x = 1\ny = f\"a {__import__('socket').x!r:>{w}} {{literal}}\"\n"), pairs(&[(2, "socket")]));
        // 正規式裡的大括號與一般欄位不算
        assert!(got("import_re = rf\"\\bimport\\s+{m}\\b|__import__\\s*\\(\\s*[\\\"']{m}|x{{2}}\"\n").is_empty());
    }

    #[test]
    fn broken_source_is_an_error() {
        assert!(imports("x = 'abc\nimport os\n").is_err());
        assert!(imports("x = \"\"\"abc\nimport os\n").is_err());
        assert!(imports("x = (1, 2\n").is_err());
        assert!(imports("x = 1)\n").is_err());
        assert!(imports("x = f\"{'abc}\"\n").is_err());
        assert!(imports("a\0b").is_err());
    }
}
