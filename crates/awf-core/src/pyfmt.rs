//! 和 Python 相同的值格式化與比較。
//!
//! 原本的工具把值直接放進錯誤訊息（`{x!r}`、`{x}`、清單的 repr），訊息文字是對跑基準的一部分，
//! 所以這裡照 Python 的規則輸出：字串優先用單引號、布林是 True／False、null 是 None、浮點數一定帶小數點。

use serde_json::Value;

/// Python 的 `repr(x)`（x 是 `json.loads` 得到的值）。
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Number(n) => number(n),
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", repr_str(k), repr(v))).collect::<Vec<_>>().join(", ")),
    }
}

/// Python 的 `str(x)`：字串是原樣，其他同 repr。
pub fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// `x.get(k)` 拿不到時 Python 給 None；格式化成字串是 "None"。
pub fn str_opt(v: Option<&Value>) -> String {
    v.map(str_of).unwrap_or_else(|| "None".into())
}

fn number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() { return i.to_string(); }
    if let Some(u) = n.as_u64() { return u.to_string(); }
    float(n.as_f64().unwrap_or(0.0))
}

/// Python 的 `repr(float)`。
pub fn float(f: f64) -> String {
    if f.is_nan() { return "nan".into(); }
    if f.is_infinite() { return if f > 0.0 { "inf".into() } else { "-inf".into() }; }
    let a = f.abs();
    if a != 0.0 && !(1e-4..1e16).contains(&a) {
        // 科學記號：Python 寫成 1e+22、1.5e-07
        let s = format!("{f:e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let ev: i32 = e.parse().unwrap_or(0);
        return format!("{m}e{}{:02}", if ev < 0 { '-' } else { '+' }, ev.abs());
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// Python 的 `repr(str)`。
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => { out.push('\\'); out.push(c); }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if !printable(c) => {
                let u = c as u32;
                if u <= 0xff { out.push_str(&format!("\\x{u:02x}")) }
                else if u <= 0xffff { out.push_str(&format!("\\u{u:04x}")) }
                else { out.push_str(&format!("\\U{u:08x}")) }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// 近似 Python 的 `str.isprintable()`：控制字元、格式字元、分隔符、未指派的不算。
/// 這裡涵蓋實際會出現在訊息裡的範圍（C1 控制字元、零寬與方向字元、行段分隔、BOM、代理區與私用區以外的常見字元）。
fn printable(c: char) -> bool {
    let u = c as u32;
    !(matches!(u, 0x80..=0xa0 | 0xad | 0x200b..=0x200f | 0x2028..=0x202f | 0x205f..=0x206f | 0x3000 | 0xfeff | 0xfff9..=0xfffb)
        || matches!(u, 0x1680 | 0x2000..=0x200a)
        || matches!(u, 0xe000..=0xf8ff | 0xf0000..=0x10ffff))
}

/// Python 的真值判斷（`if x`、`x or 預設`）。
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn truthy_opt(v: Option<&Value>) -> bool {
    v.map(truthy).unwrap_or(false)
}

/// Python 的 `==`：數字與布林互通（True == 1 == 1.0），容器逐項比。
pub fn eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, w))),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => {
                // 兩邊都是整數時照整數比，避免大數轉浮點失真
                if let (Value::Number(m), Value::Number(n)) = (a, b) {
                    if let (Some(i), Some(j)) = (m.as_i64(), n.as_i64()) { return i == j; }
                }
                x == y
            }
            _ => false,
        },
    }
}

/// Python 的 `len(str)`：以 code point 計。
pub fn len(s: &str) -> usize {
    s.chars().count()
}

/// 陣列元素；不是陣列（或是 None、空值）時當成空的，對應 `x.get(k) or []`。
pub fn items(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr(&json!("a")), "'a'");
        assert_eq!(repr(&json!("it's")), "\"it's\"");
        assert_eq!(repr(&json!("a'b\"c")), "'a\\'b\"c'");
        assert_eq!(repr(&json!("中文\n")), "'中文\\n'");
        assert_eq!(repr(&json!([1, 1.0, true, null, "x"])), "[1, 1.0, True, None, 'x']");
        assert_eq!(repr(&json!({"k": [1.5, 1e22]})), "{'k': [1.5, 1e+22]}");
        assert_eq!(repr(&json!(0.00001)), "1e-05");
        assert_eq!(repr(&json!("\u{200b}")), "'\\u200b'");
    }

    #[test]
    fn eq_matches_python() {
        assert!(eq(&json!(1), &json!(1.0)));
        assert!(eq(&json!(true), &json!(1)));
        assert!(!eq(&json!("1"), &json!(1)));
        assert!(!eq(&json!(null), &json!(false)));
        assert!(eq(&json!([1, {"a": 2}]), &json!([1.0, {"a": 2}])));
    }
}
