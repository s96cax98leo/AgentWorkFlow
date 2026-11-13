//! 和 Python 的 `json.loads` 同規則的解析器：錯誤訊息（`Expecting value: line 1 column 1 (char 0)`）逐字相同。
//!
//! 閘門工具把 JSON 的解析錯誤直接放進判定理由，所以不能用 serde_json 的訊息。
//! 和 Python 的差別（都往嚴的方向）：
//! - `NaN`、`Infinity`、溢位成無限大的數字：`loads` 當成錯誤；`loads_lenient` 換成 null（只給「讀不懂就跳過」的地方用，
//!   避免把 Python 讀得懂的稽核紀錄漏掉）。
//! - 超過 64 位元的整數變成浮點數；落單的代理字元換成 U+FFFD。
//! - 巢狀超過 `MAX_DEPTH` 層回 `TooDeep`（Python 是 RecursionError），呼叫端不能把它當成「沒有這筆資料」。

use serde_json::{Map, Number, Value};

pub const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonErr {
    /// 對應 json.JSONDecodeError，內容是 `str(e)`。
    Decode(String),
    NonFinite,
    TooDeep,
}

impl std::fmt::Display for JsonErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonErr::Decode(m) => f.write_str(m),
            JsonErr::NonFinite => f.write_str("JSON 裡有 NaN 或 Infinity，不接受"),
            JsonErr::TooDeep => write!(f, "JSON 巢狀超過 {MAX_DEPTH} 層，不接受"),
        }
    }
}

enum Stop {
    /// scan_once 的 StopIteration(idx)：這個位置沒有值。
    NoValue(usize),
    Err(JsonErr),
}

struct P<'a> {
    s: &'a [char],
    lenient: bool,
}

type R<T> = Result<T, Stop>;

impl P<'_> {
    fn fail<T>(&self, msg: &str, pos: usize) -> R<T> {
        Err(Stop::Err(JsonErr::Decode(self.message(msg, pos))))
    }

    fn message(&self, msg: &str, pos: usize) -> String {
        let pos = pos.min(self.s.len());
        let before = &self.s[..pos];
        let line = before.iter().filter(|&&c| c == '\n').count() + 1;
        let col = match before.iter().rposition(|&c| c == '\n') {
            Some(i) => pos - i,
            None => pos + 1,
        };
        format!("{msg}: line {line} column {col} (char {pos})")
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) { i += 1; }
        i
    }

    fn lit(&self, i: usize, word: &str) -> bool {
        let n = word.chars().count();
        self.s.get(i..i + n).is_some_and(|part| part.iter().copied().eq(word.chars()))
    }

    fn non_finite(&self, end: usize) -> R<(Value, usize)> {
        if self.lenient { Ok((Value::Null, end)) } else { Err(Stop::Err(JsonErr::NonFinite)) }
    }

    fn scan(&self, i: usize, depth: usize) -> R<(Value, usize)> {
        let Some(c) = self.at(i) else { return Err(Stop::NoValue(i)) };
        match c {
            '"' => self.string(i + 1).map(|(s, e)| (Value::String(s), e)),
            '{' | '[' if depth >= MAX_DEPTH => Err(Stop::Err(JsonErr::TooDeep)),
            '{' => self.object(i + 1, depth + 1),
            '[' => self.array(i + 1, depth + 1),
            'n' if self.lit(i, "null") => Ok((Value::Null, i + 4)),
            't' if self.lit(i, "true") => Ok((Value::Bool(true), i + 4)),
            'f' if self.lit(i, "false") => Ok((Value::Bool(false), i + 5)),
            'N' if self.lit(i, "NaN") => self.non_finite(i + 3),
            'I' if self.lit(i, "Infinity") => self.non_finite(i + 8),
            '-' if self.lit(i, "-Infinity") => self.non_finite(i + 9),
            _ => self.number(i),
        }
    }

    fn number(&self, start: usize) -> R<(Value, usize)> {
        let digit = |i: usize| self.at(i).is_some_and(|c| c.is_ascii_digit());
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
            if !digit(i) { return Err(Stop::NoValue(start)); }
        }
        match self.at(i) {
            Some('1'..='9') => while digit(i) { i += 1; },
            Some('0') => i += 1,
            _ => return Err(Stop::NoValue(start)),
        }
        let mut float = false;
        if self.at(i) == Some('.') && digit(i + 1) {
            float = true;
            i += 2;
            while digit(i) { i += 1; }
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut j = i + 1;
            if matches!(self.at(j), Some('+' | '-')) { j += 1; }
            if digit(j) {
                float = true;
                while digit(j) { j += 1; }
                i = j;
            }
        }
        let text: String = self.s[start..i].iter().collect();
        if !float {
            if let Ok(n) = text.parse::<i64>() { return Ok((Value::Number(n.into()), i)); }
            if let Ok(n) = text.parse::<u64>() { return Ok((Value::Number(n.into()), i)); }
        }
        match text.parse::<f64>().ok().and_then(Number::from_f64) {
            Some(n) => Ok((Value::Number(n), i)),
            None => self.non_finite(i),
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let mut v = 0;
        for k in 0..4 {
            v = v * 16 + self.at(at + k)?.to_digit(16)?;
        }
        Some(v)
    }

    /// `begin` 是開頭引號之後的位置；回傳（內容、結尾引號之後的位置）。
    fn string(&self, begin: usize) -> R<(String, usize)> {
        let len = self.s.len();
        let quote = begin - 1;
        let mut out = String::new();
        let mut end = begin;
        loop {
            let mut next = end;
            while next < len {
                let c = self.s[next];
                if c == '"' || c == '\\' { break; }
                if c <= '\u{1f}' { return self.fail("Invalid control character at", next); }
                out.push(c);
                next += 1;
            }
            if next >= len { return self.fail("Unterminated string starting at", quote); }
            let c = self.s[next];
            next += 1;
            if c == '"' { return Ok((out, next)); }
            let Some(esc) = self.at(next) else { return self.fail("Unterminated string starting at", quote) };
            if esc != 'u' {
                end = next + 1;
                out.push(match esc {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{08}',
                    'f' => '\u{0c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return self.fail("Invalid \\escape", end - 2),
                });
                continue;
            }
            next += 1;
            end = next + 4;
            if end >= len { return self.fail("Invalid \\uXXXX escape", next - 1); }
            let Some(mut cp) = self.hex4(next) else { return self.fail("Invalid \\uXXXX escape", end - 5) };
            if (0xd800..=0xdbff).contains(&cp) && end + 6 < len && self.at(end) == Some('\\') && self.at(end + 1) == Some('u') {
                let Some(lo) = self.hex4(end + 2) else { return self.fail("Invalid \\uXXXX escape", end + 1) };
                if (0xdc00..=0xdfff).contains(&lo) {
                    cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                    end += 6;
                }
            }
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
        }
    }

    fn object(&self, mut i: usize, depth: usize) -> R<(Value, usize)> {
        let mut map = Map::new();
        i = self.ws(i);
        if self.at(i) != Some('}') {
            loop {
                if self.at(i) != Some('"') { return self.fail("Expecting property name enclosed in double quotes", i); }
                let (key, next) = self.string(i + 1)?;
                i = self.ws(next);
                if self.at(i) != Some(':') { return self.fail("Expecting ':' delimiter", i); }
                i = self.ws(i + 1);
                let (val, next) = self.scan(i, depth)?;
                map.insert(key, val);
                i = self.ws(next);
                if self.at(i) == Some('}') { break; }
                if self.at(i) != Some(',') { return self.fail("Expecting ',' delimiter", i); }
                i = self.ws(i + 1);
            }
        }
        Ok((Value::Object(map), i + 1))
    }

    fn array(&self, mut i: usize, depth: usize) -> R<(Value, usize)> {
        let mut items = Vec::new();
        i = self.ws(i);
        if self.at(i) != Some(']') {
            loop {
                let (val, next) = self.scan(i, depth)?;
                items.push(val);
                i = self.ws(next);
                if self.at(i) == Some(']') { break; }
                if self.at(i) != Some(',') { return self.fail("Expecting ',' delimiter", i); }
                i = self.ws(i + 1);
            }
        }
        Ok((Value::Array(items), i + 1))
    }
}

fn parse(text: &str, lenient: bool) -> Result<Value, JsonErr> {
    let chars: Vec<char> = text.chars().collect();
    let p = P { s: &chars, lenient };
    if chars.first() == Some(&'\u{feff}') {
        return Err(JsonErr::Decode(p.message("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0)));
    }
    let (value, end) = match p.scan(p.ws(0), 0) {
        Ok(x) => x,
        Err(Stop::NoValue(at)) => return Err(JsonErr::Decode(p.message("Expecting value", at))),
        Err(Stop::Err(e)) => return Err(e),
    };
    let end = p.ws(end);
    if end != chars.len() { return Err(JsonErr::Decode(p.message("Extra data", end))); }
    Ok(value)
}

/// `json.loads(text)`；NaN、Infinity 當成錯誤。
pub fn loads(text: &str) -> Result<Value, JsonErr> {
    parse(text, false)
}

/// `json.loads(text)`；NaN、Infinity 換成 null（給「讀不懂就跳過」的地方用）。
pub fn loads_lenient(text: &str) -> Result<Value, JsonErr> {
    parse(text, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn err(t: &str) -> String {
        loads(t).unwrap_err().to_string()
    }

    #[test]
    fn messages_match_python() {
        assert_eq!(err(""), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(err("{"), "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)");
        assert_eq!(err("{\"a\" 1}"), "Expecting ':' delimiter: line 1 column 6 (char 5)");
        assert_eq!(err("[1 2]"), "Expecting ',' delimiter: line 1 column 4 (char 3)");
        assert_eq!(err("[1,\n]"), "Expecting value: line 2 column 1 (char 4)");
        assert_eq!(err("[1] x"), "Extra data: line 1 column 5 (char 4)");
        assert_eq!(err("\"abc"), "Unterminated string starting at: line 1 column 1 (char 0)");
        assert_eq!(err("\"\\x\""), "Invalid \\escape: line 1 column 2 (char 1)");
        assert_eq!(loads("{\"a\": [1, -0, 1.5, 1e2, \"\\u00e9\\ud83d\\ude00\"], \"a\": null}").unwrap(), json!({"a": null}));
        assert_eq!(loads(" [1, 2.0] ").unwrap().to_string(), "[1,2.0]");
        assert_eq!(loads("[NaN]"), Err(JsonErr::NonFinite));
        assert_eq!(loads_lenient("[NaN, -Infinity]").unwrap(), json!([null, null]));
        assert_eq!(loads(&"[".repeat(5000)), Err(JsonErr::TooDeep));
    }
}
