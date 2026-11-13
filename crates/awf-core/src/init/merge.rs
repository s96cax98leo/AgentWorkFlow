//! 和使用者既有設定合併的純函式：深度合併（使用者的值優先）、hooks 合併、缺少欄位的比對與補上。
//!
//! 這裡的函式都不碰檔案。硬性要求：使用者已有的值一律保留。

use super::{crash, R};
use crate::kb::pyish::WS_CLASS;
use crate::pyfmt;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// `deep_merge(base, extra)`：extra 的值蓋過 base，兩邊都是物件時往下合併。
pub fn deep_merge(base: &Map<String, Value>, extra: &Map<String, Value>) -> Map<String, Value> {
    let mut out = base.clone();
    for (k, v) in extra {
        let merged = match (v, out.get(k)) {
            (Value::Object(e), Some(Value::Object(b))) => Value::Object(deep_merge(b, e)),
            _ => v.clone(),
        };
        out.insert(k.clone(), merged);
    }
    out
}

/// 深度合併：使用者已有的值一律保留（純量、清單都不改），只補上模版有、使用者沒有的鍵；
/// `hooks` 依事件合併（見 [`merge_hooks`]）。
pub fn merge_settings(user: &Map<String, Value>, tmpl: &Map<String, Value>) -> R<Map<String, Value>> {
    let mut out = user.clone();
    for (k, v) in tmpl {
        if let (true, Value::Object(th)) = (k == "hooks", v) {
            let empty = Map::new();
            let uh = match user.get(k) {
                Some(Value::Object(o)) => o,
                _ => &empty,
            };
            out.insert(k.clone(), Value::Object(merge_hooks(uh, th)?));
        } else if !out.contains_key(k) {
            out.insert(k.clone(), v.clone());
        } else if let (Value::Object(t), Some(Value::Object(u))) = (v, out.get(k)) {
            let merged = merge_settings(u, t)?;
            out.insert(k.clone(), Value::Object(merged));
        }
    }
    Ok(out)
}

fn awf_mode_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!(r"awf_hook\.py\W*[{WS_CLASS}]+(guard-shell|guard-paths|envelope)\b")).expect("固定的正規式"))
}

/// 這個 hook 是 awf_hook.py 的哪個模式；不是就回 None。
/// hook 的註冊方式改了（例如改成執行檔）時，要一起改這裡認舊註冊的規則。
pub fn awf_mode(h: &Map<String, Value>) -> Option<String> {
    let cmd = h.get("command").map(pyfmt::str_of).unwrap_or_default();
    awf_mode_rx().captures(&cmd).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

/// awf_hook.py 的每個模式在每個事件只留一份：舊的不論掛在哪個 matcher 都先拿掉，再放上模版這版的；
/// 使用者自己的 hooks 原樣保留。
pub fn merge_hooks(user: &Map<String, Value>, tmpl: &Map<String, Value>) -> R<Map<String, Value>> {
    let mut out = user.clone();
    for (event, entries) in tmpl {
        let entries: &[Value] = entries.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut modes: Vec<String> = Vec::new();
        for e in entries {
            for th in pyfmt::items(e.get("hooks")) {
                if let Some(m) = th.as_object().and_then(awf_mode) {
                    modes.push(m);
                }
            }
        }
        let cur = match out.get(event) {
            None => Vec::new(),
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return crash(format!("settings 的 hooks.{event} 不是陣列，沒辦法合併")),
        };
        let mut keep: Vec<Value> = Vec::new();
        for mut ue in cur {
            if let Some(hooks) = ue.as_object().and_then(|o| o.get("hooks")).and_then(Value::as_array) {
                let had = hooks.len();
                let mut left: Vec<Value> = Vec::new();
                for uh in hooks {
                    let Some(o) = uh.as_object() else {
                        return crash(format!("settings 的 hooks.{event} 裡有不是物件的 hook，沒辦法合併"));
                    };
                    if !awf_mode(o).is_some_and(|m| modes.contains(&m)) {
                        left.push(uh.clone());
                    }
                }
                let now_empty = left.is_empty();
                if let Some(o) = ue.as_object_mut() {
                    o.insert("hooks".to_string(), Value::Array(left));
                }
                if had > 0 && now_empty {
                    continue; // 只剩舊的 awf hook 的項目整個拿掉
                }
            }
            keep.push(ue);
        }
        keep.extend(entries.iter().cloned());
        out.insert(event.clone(), Value::Array(keep));
    }
    Ok(out)
}

/// base 有、have 沒有的欄位路徑（a.b.c）與 base 的值；兩邊都是物件時往下比，清單與純量不比內容。
/// `atomic` 時，子欄位全是純量或清單的物件當成一個值：已經有了就不往裡面補。
pub fn missing_fields(base: &Map<String, Value>, have: &Map<String, Value>, prefix: &str, atomic: bool) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for (k, v) in base {
        let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match (v, have.get(k)) {
            (_, None) => out.push((p, v.clone())),
            (Value::Object(b), Some(Value::Object(h))) => {
                if atomic && !b.values().any(Value::is_object) {
                    continue;
                }
                out.extend(missing_fields(b, h, &p, atomic));
            }
            _ => {}
        }
    }
    out
}

/// 只在 path 不存在時寫入（中間的物件沒有就建立）；既有值永遠不改。
pub fn set_missing(doc: &mut Map<String, Value>, path: &str, value: &Value) {
    let keys: Vec<&str> = path.split('.').collect();
    let Some((last, mids)) = keys.split_last() else { return };
    let mut cur = doc;
    for k in mids {
        if !cur.get(*k).is_some_and(Value::is_object) {
            if cur.contains_key(*k) {
                return; // 既有值不是物件：不改
            }
            cur.insert((*k).to_string(), Value::Object(Map::new()));
        }
        let Some(next) = cur.get_mut(*k).and_then(Value::as_object_mut) else { return };
        cur = next;
    }
    if !cur.contains_key(*last) {
        cur.insert((*last).to_string(), value.clone());
    }
}

/// 設定檔裡的說明文字（`_說明` 開頭的鍵）換成新版模版的內容；其他值不動。
pub fn refresh_notes(doc: &mut Map<String, Value>, tmpl: &Map<String, Value>) {
    for (k, v) in tmpl {
        if k.starts_with("_說明") && v.is_string() {
            doc.insert(k.clone(), v.clone());
        } else if let (Value::Object(t), Some(Value::Object(d))) = (v, doc.get_mut(k)) {
            refresh_notes(d, t);
        }
    }
}

/// 標題那一行（結尾可以有空白）在文字裡的位元組位置；找不到回 None。
pub fn heading_at(text: &str, heading: &str) -> Option<usize> {
    let rx = Regex::new(&format!(r"(?m)^{}[ \t]*\r?$", regex::escape(heading))).ok()?;
    rx.find(text).map(|m| m.start())
}

/// `json.dumps(v, ensure_ascii=False)`，超過 n 個字就截斷加「…」。
pub fn short(v: &Value, n: usize) -> String {
    let t = crate::recipe::dumps(v, None);
    if t.chars().count() <= n {
        t
    } else {
        format!("{}…", t.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn user_values_win() {
        let user = m(json!({"a": 1, "b": {"x": [1]}, "c": "keep"}));
        let tmpl = m(json!({"a": 2, "b": {"x": [2], "y": 3}, "d": 4, "c": {"z": 1}}));
        let got = merge_settings(&user, &tmpl).ok().unwrap_or_default();
        assert_eq!(Value::Object(got), json!({"a": 1, "b": {"x": [1], "y": 3}, "c": "keep", "d": 4}));
    }

    #[test]
    fn hooks_registered_once() {
        let tmpl = m(json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{"command": "\"py\" \"x/awf_hook.py\" guard-shell"}]}]}));
        let user = m(json!({"PreToolUse": [
            {"matcher": "Old", "hooks": [{"command": "python3 awf_hook.py guard-shell"}]},
            {"matcher": "Mine", "hooks": [{"command": "echo hi"}, {"command": "awf_hook.py guard-shell"}]}
        ], "Stop": [1]}));
        let got = merge_hooks(&user, &tmpl).ok().unwrap_or_default();
        assert_eq!(Value::Object(got), json!({"PreToolUse": [
            {"matcher": "Mine", "hooks": [{"command": "echo hi"}]},
            {"matcher": "Bash", "hooks": [{"command": "\"py\" \"x/awf_hook.py\" guard-shell"}]}
        ], "Stop": [1]}));
    }

    #[test]
    fn missing_and_set() {
        let base = m(json!({"a": {"p": 1, "q": 2}, "s": {"deep": {"k": 1}, "n": 1}, "z": 9}));
        let have = m(json!({"a": {"p": 5}, "s": {"deep": {}}}));
        let miss = missing_fields(&base, &have, "", true);
        let paths: Vec<&str> = miss.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["s.n", "z"]);
        let mut doc = have.clone();
        for (p, v) in &miss {
            set_missing(&mut doc, p, v);
        }
        assert_eq!(Value::Object(doc), json!({"a": {"p": 5}, "s": {"deep": {}, "n": 1}, "z": 9}));
    }
}
