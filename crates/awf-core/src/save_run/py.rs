//! Python 語意的小工具（save_run 私有）。
//!
//! save_run.py 對形狀不對的輸入（欄位是別的型別、缺鍵）多半直接丟例外結束（exit 1、不記核准）。
//! 這裡照同樣的規則回 [`PyErr`]（類別名照 Python），由最外層印訊息、exit 1：0.x 的 Python 版會丟例外的地方不能變成放行。

use crate::kb::pyish;
use crate::kb::PyErr;
use crate::merge_gate::pyjson;
use crate::{pyfmt, recipe};
use serde_json::{Map, Value};

pub type R<T> = Result<T, PyErr>;

pub static NULL: Value = Value::Null;

/// `type(v).__name__`
pub fn tname(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

pub fn type_err(msg: impl Into<String>) -> PyErr {
    PyErr::other("TypeError", msg)
}

pub fn attr_err(v: &Value, name: &str) -> PyErr {
    PyErr::other("AttributeError", format!("'{}' object has no attribute '{name}'", tname(v)))
}

pub fn key_err(k: &str) -> PyErr {
    PyErr::other("KeyError", pyfmt::repr_str(k))
}

pub fn from_recipe(e: recipe::PyErr) -> PyErr {
    PyErr::other(e.kind, e.msg)
}

pub fn truthy(v: &Value) -> bool {
    pyfmt::truthy(v)
}

/// `str(v)`
pub fn s(v: &Value) -> String {
    pyfmt::str_of(v)
}

/// `str(v or default)`
pub fn s_or(v: &Value, default: &str) -> String {
    if truthy(v) { s(v) } else { default.to_string() }
}

/// `v.get(k)`：不是物件丟 AttributeError；拿不到是 None。
pub fn get<'a>(v: &'a Value, k: &str) -> R<&'a Value> {
    match v {
        Value::Object(o) => Ok(o.get(k).unwrap_or(&NULL)),
        other => Err(attr_err(other, "get")),
    }
}

/// `v.get(k, 預設)`：分得出「沒有這個鍵」與「值是 null」。
pub fn get_opt<'a>(v: &'a Value, k: &str) -> R<Option<&'a Value>> {
    match v {
        Value::Object(o) => Ok(o.get(k)),
        other => Err(attr_err(other, "get")),
    }
}

/// `v[k]`（k 是字串）。
pub fn idx<'a>(v: &'a Value, k: &str) -> R<&'a Value> {
    match v {
        Value::Object(o) => o.get(k).ok_or_else(|| key_err(k)),
        Value::Array(_) => Err(type_err("list indices must be integers or slices, not str")),
        Value::String(_) => Err(type_err("string indices must be integers, not 'str'")),
        other => Err(type_err(format!("'{}' object is not subscriptable", tname(other)))),
    }
}

/// `k in v`（k 是字串）：物件看鍵、清單看元素、字串看子字串；其他丟 TypeError。
pub fn contains(v: &Value, k: &str) -> R<bool> {
    match v {
        Value::Object(o) => Ok(o.contains_key(k)),
        Value::Array(a) => Ok(a.iter().any(|x| x.as_str() == Some(k))),
        Value::String(t) => Ok(t.contains(k)),
        other => Err(type_err(format!("argument of type '{}' is not iterable", tname(other)))),
    }
}

/// `needle in hay`（needle 是任意值）：清單逐項比、字串看子字串（needle 要是字串）、物件看鍵；其他丟 TypeError。
pub fn value_in(needle: &Value, hay: &Value) -> R<bool> {
    match hay {
        Value::Array(a) => Ok(a.iter().any(|x| pyfmt::eq(x, needle))),
        Value::String(t) => match needle {
            Value::String(n) => Ok(t.contains(n.as_str())),
            other => Err(type_err(format!("'in <string>' requires string as left operand, not {}", tname(other)))),
        },
        Value::Object(o) => {
            hashable(needle)?;
            Ok(needle.as_str().is_some_and(|k| o.contains_key(k)))
        }
        other => Err(type_err(format!("argument of type '{}' is not iterable", tname(other)))),
    }
}

/// `for x in v`：清單是元素、字串是每個字、物件是鍵；其他丟 TypeError。
pub fn iter(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(t) => Ok(t.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(type_err(format!("'{}' object is not iterable", tname(other)))),
    }
}

/// `for x in (v or [])`
pub fn iter_or(v: &Value) -> R<Vec<Value>> {
    if truthy(v) { iter(v) } else { Ok(Vec::new()) }
}

/// `len(v)`
pub fn len(v: &Value) -> R<usize> {
    match v {
        Value::Array(a) => Ok(a.len()),
        Value::String(t) => Ok(t.chars().count()),
        Value::Object(o) => Ok(o.len()),
        other => Err(type_err(format!("object of type '{}' has no len()", tname(other)))),
    }
}

/// `len(v or [])`
pub fn len_or(v: &Value) -> R<usize> {
    if truthy(v) { len(v) } else { Ok(0) }
}

/// `sep.join(items)`：元素要全是字串。
pub fn join(sep: &str, items: &[Value]) -> R<String> {
    let mut parts: Vec<&str> = Vec::with_capacity(items.len());
    for (i, x) in items.iter().enumerate() {
        match x {
            Value::String(t) => parts.push(t),
            other => return Err(type_err(format!("sequence item {i}: expected str instance, {} found", tname(other)))),
        }
    }
    Ok(parts.join(sep))
}

/// `v[:n]`：清單取前 n 個、字串取前 n 個字（之後再被當成可迭代的東西用）。
pub fn slice_head(v: &Value, n: usize) -> R<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.iter().take(n).cloned().collect()),
        Value::String(t) => Ok(t.chars().take(n).map(|c| Value::String(c.to_string())).collect()),
        Value::Object(_) => Err(PyErr::other("KeyError", format!("slice(None, {n}, None)"))),
        other => Err(type_err(format!("'{}' object is not subscriptable", tname(other)))),
    }
}

/// 放進 set、當 dict 的鍵、`x in {…}`：清單與物件不行（unhashable）。
pub fn hashable(v: &Value) -> R<()> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(type_err(format!("unhashable type: '{}'", tname(v)))),
        _ => Ok(()),
    }
}

/// `set(v or [])`（去重、保留第一次出現的順序）。
pub fn set_or(v: &Value) -> R<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    for x in iter_or(v)? {
        hashable(&x)?;
        if !out.iter().any(|y| pyfmt::eq(y, &x)) {
            out.push(x);
        }
    }
    Ok(out)
}

/// `set(a or []) - set(b or [])` 是不是非空。
pub fn set_gains(a: &Value, b: &Value) -> R<bool> {
    let (a, b) = (set_or(a)?, set_or(b)?);
    Ok(a.iter().any(|x| !b.iter().any(|y| pyfmt::eq(x, y))))
}

/// `sorted(items)`：全是字串照字碼排；全是數字（含布林）照數值排；不超過一個不用比；其他組合當成 TypeError（只會更嚴）。
pub fn sorted(items: Vec<Value>) -> R<Vec<Value>> {
    if items.len() <= 1 {
        return Ok(items);
    }
    if items.iter().all(Value::is_string) {
        let mut v = items;
        v.sort_by(|a, b| a.as_str().unwrap_or_default().cmp(b.as_str().unwrap_or_default()));
        return Ok(v);
    }
    let num = |x: &Value| match x {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    if items.iter().all(|x| num(x).is_some_and(|f| f.is_finite())) {
        let mut v = items;
        v.sort_by(|a, b| num(a).partial_cmp(&num(b)).unwrap_or(std::cmp::Ordering::Equal));
        return Ok(v);
    }
    let (a, b) = (items.first().map_or("?", tname), items.iter().find(|x| !x.is_string()).map_or("?", tname));
    Err(type_err(format!("'<' not supported between instances of '{b}' and '{a}'")))
}

/// `" ".join(s.split())`
pub fn squash(t: &str) -> String {
    crate::hooks::py::split_ws(t).join(" ")
}

/// `s[:n]`
pub fn take(t: &str, n: usize) -> &str {
    pyish::take(t, n)
}

/// `s.strip()`
pub fn strip(t: &str) -> &str {
    pyish::strip(t)
}

/// 一個可以原地改的物件；不是物件時照 Python 對它做 `x[k] = v` 會丟的例外。
pub fn obj_mut(v: &mut Value) -> R<&mut Map<String, Value>> {
    match v {
        Value::Object(o) => Ok(o),
        other => Err(type_err(format!("'{}' object does not support item assignment", tname(other)))),
    }
}

// ------------------------------------------------------------------ JSON（閘門用：讀不懂一律拒絕）

/// JSON 原文裡有沒有落單的代理字元跳脫（高位那一半後面沒接低位那一半之類）。
fn lone_surrogate_escape(text: &str) -> bool {
    let b = text.as_bytes();
    let unit = |i: usize| -> Option<u32> {
        if b.get(i) != Some(&b'\\') || b.get(i + 1) != Some(&b'u') {
            return None;
        }
        u32::from_str_radix(text.get(i + 2..i + 6)?, 16).ok()
    };
    let mut i = 0;
    while i < b.len() {
        if b.get(i) != Some(&b'\\') {
            i += 1;
            continue;
        }
        match unit(i) {
            Some(hi) if (0xd800..0xdc00).contains(&hi) => {
                if !unit(i + 6).is_some_and(|lo| (0xdc00..0xe000).contains(&lo)) {
                    return true;
                }
                i += 12;
            }
            Some(lo) if (0xdc00..0xe000).contains(&lo) => return true,
            Some(_) => i += 6,
            None => i += 2,
        }
    }
    false
}

/// JSON 原文（字串以外的地方）有沒有超出 64 位元範圍的整數：解析器會把它變成浮點數，寫回去就不是原來的數。
fn huge_integer(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b.get(i).copied().unwrap_or(0) {
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b.get(i).copied().unwrap_or(0) {
                        b'\\' => i += 2,
                        b'"' => break,
                        _ => i += 1,
                    }
                }
                i += 1;
            }
            c if c == b'-' || c.is_ascii_digit() => {
                let start = i;
                i += 1;
                while b.get(i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
                    i += 1;
                }
                let tok = text.get(start..i).unwrap_or_default();
                let digits = tok.strip_prefix('-').unwrap_or(tok);
                if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) && tok.parse::<i64>().is_err() && tok.parse::<u64>().is_err() {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

/// `json.loads(text)`，但閘門只收看得懂的：NaN、Infinity、落單的代理字元跳脫、超出 64 位元範圍的整數、巢狀太深都當成讀不懂。
/// 錯誤的類別：語法錯是 JSONDecodeError，其餘是 ValueError（Python 的 `except ValueError` 兩種都接得到），太深是 RecursionError。
pub fn loads(text: &str) -> R<Value> {
    match pyjson::loads(text) {
        Ok(v) => {
            if lone_surrogate_escape(text) {
                return Err(PyErr::other("ValueError", "JSON 裡有落單的代理字元跳脫，不接受"));
            }
            if huge_integer(text) {
                return Err(PyErr::other("ValueError", "JSON 裡有超出 64 位元範圍的整數，不接受"));
            }
            Ok(v)
        }
        Err(pyjson::JsonErr::Decode(m)) => Err(PyErr::other("JSONDecodeError", m)),
        Err(e @ pyjson::JsonErr::NonFinite) => Err(PyErr::other("ValueError", e.to_string())),
        Err(e @ pyjson::JsonErr::TooDeep) => Err(PyErr::other("RecursionError", e.to_string())),
    }
}

/// 這個錯誤 Python 的 `except (OSError, ValueError)` 接不接得到（RecursionError 之類接不到）。
pub fn os_or_value(e: &PyErr) -> bool {
    !matches!(e.class.as_str(), "RecursionError" | "TypeError" | "AttributeError" | "KeyError" | "RuntimeError")
}

/// `json.dumps(v, ensure_ascii=False, indent=indent)`
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    recipe::dumps(v, indent)
}

/// `json.dumps(v, ensure_ascii=False, sort_keys=True)`
pub fn dumps_sorted(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let mut m = Map::new();
                for k in keys {
                    m.insert(k.clone(), sort(o.get(k).unwrap_or(&NULL)));
                }
                Value::Object(m)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    recipe::dumps(&sort(v), None)
}

// ------------------------------------------------------------------ 路徑（照 pathlib 的文字規則，一律用 /）

/// `str(Path(p))`
pub fn pp(p: &str) -> String {
    pyish::ppath(p)
}

/// `str(Path(a) / b)`
pub fn pj(a: &str, b: &str) -> String {
    pyish::pjoin(a, b)
}

/// `Path(p).name`
pub fn pname(p: &str) -> String {
    let t = pp(p);
    if t == "." || t == "/" { String::new() } else { pyish::basename(&t).to_string() }
}

/// `str(Path(p).parent)`
pub fn pparent(p: &str) -> String {
    let t = pp(p);
    match t.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => t.get(..i).unwrap_or(".").to_string(),
    }
}

/// `_norm(p)`：`str(p).replace("\\", "/")`
pub fn norm(p: &str) -> String {
    p.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_json() {
        let class = |t: &str| loads(t).map_err(|e| e.class).err().unwrap_or_default();
        assert!(loads(r#"{"a": 1, "b": [1.5, -2, "😀"]}"#).is_ok());
        assert_eq!(class("[NaN]"), "ValueError");
        assert_eq!(class(r#"["\ud800"]"#), "ValueError");
        assert_eq!(class(r#"["\udc00\ud800"]"#), "ValueError");
        assert!(loads(r#"["\\ud800"]"#).is_ok());
        assert_eq!(class("[123456789012345678901234567890]"), "ValueError");
        assert_eq!(class("[-9223372036854775809]"), "ValueError");
        assert_eq!(class("[18446744073709551616]"), "ValueError");
        assert!(loads("[18446744073709551615, -9223372036854775808, \"123456789012345678901234567890\", 1e30, -0, 0]").is_ok());
        assert_eq!(class("{"), "JSONDecodeError");
        assert_eq!(class(&"[".repeat(5000)), "RecursionError");
        assert_eq!(dumps(&loads("{\"b\": 1.0, \"a\": [], \"c\": {}}").unwrap_or_default(), Some(2)), "{\n  \"b\": 1.0,\n  \"a\": [],\n  \"c\": {}\n}");
        assert_eq!(dumps_sorted(&loads("{\"b\": 1, \"a\": \"字\"}").unwrap_or_default()), "{\"a\": \"字\", \"b\": 1}");
    }

    #[test]
    fn paths() {
        assert_eq!(pname("a/b/"), "b");
        assert_eq!(pname("."), "");
        assert_eq!(pparent("/a"), "/");
        assert_eq!(pparent("a"), ".");
        assert_eq!(pj("a", "/b"), "/b");
    }
}
