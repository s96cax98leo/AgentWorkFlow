//! Python `json.loads` 失敗時的錯誤訊息（`str(JSONDecodeError)`），例如 `Expecting value: line 1 column 1 (char 0)`。
//!
//! 交件檢查會把這段訊息原樣回給 agent，所以照 CPython（C 版 scanner）的判斷順序與位置重做一次掃描。
//! 只在 serde_json 已經判定解析失敗後才呼叫；掃不出錯（Python 接受而 serde 不接受的寫法）回 None。

const MAX_DEPTH: usize = 500;

struct Scan<'a> {
    s: &'a [char],
}

type E = (&'static str, usize);

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

impl Scan<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(is_ws) {
            i += 1;
        }
        i
    }

    fn lit(&self, i: usize, word: &str) -> bool {
        word.chars().enumerate().all(|(k, c)| self.at(i + k) == Some(c))
    }

    /// 字串內容（i 是開頭引號的下一格）；回傳結尾引號的下一格。
    fn string(&self, i: usize) -> Result<usize, E> {
        let begin = i.saturating_sub(1);
        let len = self.s.len();
        let mut next = i;
        loop {
            let mut c = '\0';
            while next < len {
                c = self.s[next];
                if c == '"' || c == '\\' {
                    break;
                }
                if (c as u32) <= 0x1f {
                    return Err(("Invalid control character at", next));
                }
                next += 1;
            }
            if next >= len || !(c == '"' || c == '\\') {
                return Err(("Unterminated string starting at", begin));
            }
            next += 1;
            if c == '"' {
                return Ok(next);
            }
            if next == len {
                return Err(("Unterminated string starting at", begin));
            }
            let e = self.s[next];
            if e != 'u' {
                let end = next + 1;
                if !matches!(e, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    return Err(("Invalid \\escape", end - 2));
                }
                next = end;
            } else {
                next += 1;
                let end = next + 4;
                if end >= len {
                    return Err(("Invalid \\uXXXX escape", next - 1));
                }
                let mut cp: u32 = 0;
                while next < end {
                    match self.s[next].to_digit(16) {
                        Some(d) if self.s[next].is_ascii() => cp = cp * 16 + d,
                        _ => return Err(("Invalid \\uXXXX escape", end - 5)),
                    }
                    next += 1;
                }
                // 高代理後面接 \uXXXX：再讀四位（寫壞一樣報錯）
                if (0xD800..=0xDBFF).contains(&cp) && end + 6 < len && self.s[next] == '\\' && self.s[next + 1] == 'u' {
                    let end2 = end + 6;
                    let mut k = next + 2;
                    while k < end2 {
                        match self.s[k].to_digit(16) {
                            Some(_) if self.s[k].is_ascii() => {}
                            _ => return Err(("Invalid \\uXXXX escape", end2 - 5)),
                        }
                        k += 1;
                    }
                    let low: String = self.s[next + 2..end2].iter().collect();
                    if u32::from_str_radix(&low, 16).is_ok_and(|v| (0xDC00..=0xDFFF).contains(&v)) {
                        next = end2;
                    }
                }
            }
        }
    }

    fn number(&self, start: usize) -> Result<usize, E> {
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('1'..='9') => {
                i += 1;
                while self.at(i).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
            }
            Some('0') => i += 1,
            _ => return Err(("Expecting value", start)),
        }
        if self.at(i) == Some('.') && self.at(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            i += 2;
            while self.at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i + 1;
            if matches!(self.at(j), Some('-' | '+')) {
                j += 1;
            }
            if self.at(j).is_some_and(|c| c.is_ascii_digit()) {
                while self.at(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
                i = j;
            }
        }
        Ok(i)
    }

    fn value(&self, i: usize, depth: usize) -> Result<usize, Option<E>> {
        if depth > MAX_DEPTH {
            return Err(None);
        }
        let Some(c) = self.at(i) else { return Err(Some(("Expecting value", i))) };
        match c {
            '"' => self.string(i + 1).map_err(Some),
            '{' => self.object(i + 1, depth + 1),
            '[' => self.array(i + 1, depth + 1),
            'n' if self.lit(i, "null") => Ok(i + 4),
            't' if self.lit(i, "true") => Ok(i + 4),
            'f' if self.lit(i, "false") => Ok(i + 5),
            'N' if self.lit(i, "NaN") => Ok(i + 3),
            'I' if self.lit(i, "Infinity") => Ok(i + 8),
            '-' if self.lit(i, "-Infinity") => Ok(i + 9),
            _ => self.number(i).map_err(Some),
        }
    }

    fn object(&self, i: usize, depth: usize) -> Result<usize, Option<E>> {
        let mut i = self.skip_ws(i);
        if self.at(i) == Some('}') {
            return Ok(i + 1);
        }
        loop {
            if self.at(i) != Some('"') {
                return Err(Some(("Expecting property name enclosed in double quotes", i)));
            }
            i = self.skip_ws(self.string(i + 1).map_err(Some)?);
            if self.at(i) != Some(':') {
                return Err(Some(("Expecting ':' delimiter", i)));
            }
            i = self.skip_ws(i + 1);
            i = self.skip_ws(self.value(i, depth)?);
            if self.at(i) == Some('}') {
                return Ok(i + 1);
            }
            if self.at(i) != Some(',') {
                return Err(Some(("Expecting ',' delimiter", i)));
            }
            i = self.skip_ws(i + 1);
        }
    }

    fn array(&self, i: usize, depth: usize) -> Result<usize, Option<E>> {
        let mut i = self.skip_ws(i);
        if self.at(i) == Some(']') {
            return Ok(i + 1);
        }
        loop {
            i = self.skip_ws(self.value(i, depth)?);
            if self.at(i) == Some(']') {
                return Ok(i + 1);
            }
            if self.at(i) != Some(',') {
                return Err(Some(("Expecting ',' delimiter", i)));
            }
            i = self.skip_ws(i + 1);
        }
    }
}

/// `str(e)`（e 是 `json.loads(text)` 丟的 JSONDecodeError）；掃不出錯回 None。
pub fn error_message(text: &str) -> Option<String> {
    let s: Vec<char> = text.chars().collect();
    let sc = Scan { s: &s };
    let start = sc.skip_ws(0);
    let (msg, pos) = match sc.value(start, 0) {
        Ok(end) => {
            let end = sc.skip_ws(end);
            if end == s.len() {
                return None;
            }
            ("Extra data", end)
        }
        Err(Some(e)) => e,
        Err(None) => return None,
    };
    let pos = pos.min(s.len());
    let before = &s[..pos];
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let col = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    Some(format!("{msg}: line {line} column {col} (char {pos})"))
}

#[cfg(test)]
mod tests {
    use super::error_message as m;

    #[test]
    fn messages_match_cpython() {
        assert_eq!(m("").as_deref(), Some("Expecting value: line 1 column 1 (char 0)"));
        assert_eq!(m("{").as_deref(), Some("Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"));
        assert_eq!(m("{\"a\" 1}").as_deref(), Some("Expecting ':' delimiter: line 1 column 6 (char 5)"));
        assert_eq!(m("{\"a\": 1 \"b\"}").as_deref(), Some("Expecting ',' delimiter: line 1 column 9 (char 8)"));
        assert_eq!(m("[1, 2").as_deref(), Some("Expecting ',' delimiter: line 1 column 6 (char 5)"));
        assert_eq!(m("[1,").as_deref(), Some("Expecting value: line 1 column 4 (char 3)"));
        assert_eq!(m("{\"a\": 1}\nx").as_deref(), Some("Extra data: line 2 column 1 (char 9)"));
        assert_eq!(m("\"abc").as_deref(), Some("Unterminated string starting at: line 1 column 1 (char 0)"));
        assert_eq!(m("\"a\\qb\"").as_deref(), Some("Invalid \\escape: line 1 column 3 (char 2)"));
        assert_eq!(m("{\"a\": tru}").as_deref(), Some("Expecting value: line 1 column 7 (char 6)"));
        assert_eq!(m("{\"a\": 1,}").as_deref(), Some("Expecting property name enclosed in double quotes: line 1 column 9 (char 8)"));
        assert_eq!(m("{\"a\": 1}"), None);
    }
}
