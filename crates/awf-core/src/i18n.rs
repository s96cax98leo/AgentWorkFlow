//! 派工單的字串表（profile 的 `prompt_lang`、`prompt_overrides`）。
//!
//! 派工單裡由片段產生的固定文字（共同前綴、限制文件那幾行、執行紀錄檔那一行等）寫在
//! `workflow-templates/snippets/challenge.js` 的 `awfText()`，內容和 `workflow-templates/i18n/zh-TW.json` 逐字相同
//! （`cargo xtask check-i18n` 核對；鍵集合另和這裡的 [`KEYS`] 一致）。
//!
//! 內嵌方式：workflow 腳本不能讀檔，所以 `prompt_lang` 不是 zh-TW 時，產生腳本的工具（`awf next`／`awf render` 的
//! [`crate::state::render`]、自訂腳本、`awf build` 的專案覆寫檔）把 `i18n/<語言>.json` 的內容放進內嵌 profile 的保留鍵
//! [`RESERVED_KEY`]（[`augment`]）。`prompt_overrides` 不經工具，片段直接讀 profile。
//! 沒設 `prompt_lang`（或是 zh-TW）而且沒有 `prompt_overrides` 時，內嵌的 profile 和原本逐字相同、派工單也逐字相同。
//!
//! 只影響派工單的文字：閘門與守門不讀字串表。閘門比對的字（〈限制對照〉的節名與判定詞等）不在字串表裡，
//! 換語言時用 profile 的 `constraints_format` 改（兩邊共用同一份設定）。
//!
//! 語言檔找不到或讀不懂時退回內建的字（＝zh-TW），`awf doctor` 提示（[`lang_problem`]）。

use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 內建的語言（片段裡的字）。
pub const DEFAULT_LANG: &str = "zh-TW";
/// 內嵌 profile 裡放解析後語言檔的保留鍵（工具寫，使用者不能寫：[`profile_errors`]）。
pub const RESERVED_KEY: &str = "_prompt_strings";
/// `prompt_overrides` 每個值的字數上限（以 code point 計，同 JSON Schema 的 maxLength）。
pub const OVERRIDE_MAX: usize = 2000;
/// 語言檔每個值的字數上限（片段同一個數）。
pub const LANG_MAX: usize = 4000;

/// 字串表的鍵（和 `awfText()`、`i18n/*.json` 的鍵集合一致；`cargo xtask check-i18n` 核對）。
pub const KEYS: [&str; 46] = [
    "prefix.heading", "prefix.rules", "prefix.external", "prefix.context", "prefix.save_context", "prefix.read_hook", "prefix.kb",
    "prefix.code_tools", "prefix.record", "prefix.envelope", "prefix.kg",
    "record.tail",
    "constraint.rule", "constraint.kb", "constraint.table_rule", "constraint.line", "constraint.note", "constraint.paths", "constraint.table",
    "meth.profile", "meth.doc_format_default", "meth.none",
    "commit.format", "commit.example", "commit.repo",
    "ext.heading", "ext.body",
    "subtask.rules", "subtask.no_test_first",
    "sd.head", "sd.steps", "sd.scopes", "sd.topics", "sd.document", "sd.document_default",
    "design.artifacts", "design.req_style", "design.req_style_default", "design.guidance",
    "delta.impact",
    "integrate.in_place", "integrate.stop", "integrate.transit",
    "sep.list", "sep.semicolon", "sep.slash",
];

/// 不能被換掉的變數：rules.md 必讀與輸出語言（I3）、派工單最後一行的執行紀錄檔（I6）。
/// 這兩個鍵的字（語言檔或 prompt_overrides）缺了這些變數就不採用（片段的 awfStrings 同一條規則）；`record.tail` 另外要以 `{record}` 結尾。
pub const MUST_KEEP: [(&str, &[&str]); 2] = [("prefix.rules", &["rules", "lang"]), ("record.tail", &["record"])];

/// 這個鍵的字可不可以採用（[`MUST_KEEP`] 的規則）。
pub fn keeps_required(key: &str, s: &str) -> bool {
    let need = MUST_KEEP.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(&[]);
    let have = vars(s);
    need.iter().all(|n| have.contains(*n)) && (key != "record.tail" || s.ends_with("{record}"))
}

/// 語言代號的寫法（也擋掉路徑字元）：`en`、`zh-TW`、`pt-BR` 這類。
pub fn lang_name_ok(s: &str) -> bool {
    let mut parts = s.split('-');
    let head = parts.next().unwrap_or_default();
    let head_ok = (2..=3).contains(&head.len()) && head.bytes().all(|b| b.is_ascii_alphabetic());
    let rest: Vec<&str> = parts.collect();
    head_ok && rest.len() <= 2 && rest.iter().all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// 字串裡的 `{名稱}`（片段的 awfFill 會代入的變數；名稱是英數與底線）。
pub fn vars(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        rest = &rest[i + 1..];
        let Some(j) = rest.find('}') else { break };
        let name = &rest[..j];
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            out.insert(name.to_string());
            rest = &rest[j + 1..];
        }
    }
    out
}

/// profile 要的語言（`prompt_lang` 是字串才算；沒寫是 None）。
pub fn requested_lang(meth: &Value) -> Option<&str> {
    meth.get("prompt_lang").and_then(Value::as_str)
}

/// 讀一份語言檔：只收 [`KEYS`] 裡的鍵、1–[`LANG_MAX`] 字的字串（其餘略過，片段對那幾個鍵用內建的字）。
pub fn load_table(path: &Path) -> Result<Map<String, Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let o = v.as_object().ok_or_else(|| format!("{} 不是 JSON 物件", path.display()))?;
    let mut out = Map::new();
    for k in KEYS {
        if let Some(s) = o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty() && s.chars().count() <= LANG_MAX && keeps_required(k, s)) {
            out.insert(k.to_string(), Value::String(s.to_string()));
        }
    }
    if out.is_empty() {
        return Err(format!("{} 沒有任何認得的鍵", path.display()));
    }
    Ok(out)
}

/// 依 profile 的 `prompt_lang` 解析出要內嵌的語言檔內容：
/// `Ok(None)`＝用內建的字（沒寫、是 zh-TW）；`Err`＝寫了別的語言但用不了（原因），也用內建的字。
/// `dirs`：依先後找 `<dir>/<語言>.json`。
pub fn resolve(meth: &Value, dirs: &[PathBuf]) -> Result<Option<(PathBuf, Map<String, Value>)>, String> {
    let Some(lang) = meth.get("prompt_lang").filter(|v| !v.is_null()) else { return Ok(None) };
    let Some(lang) = lang.as_str() else { return Err("prompt_lang 要是字串".into()) };
    if lang == DEFAULT_LANG {
        return Ok(None);
    }
    if !lang_name_ok(lang) {
        return Err(format!("prompt_lang 的「{lang}」不是語言代號（例如 en、zh-TW）"));
    }
    let Some(path) = dirs.iter().map(|d| d.join(format!("{lang}.json"))).find(|p| p.is_file()) else {
        return Err(format!("找不到 prompt_lang「{lang}」的字串表 i18n/{lang}.json"));
    };
    load_table(&path).map(|t| Some((path, t)))
}

/// 產生腳本前把語言檔放進要內嵌的 profile：`prompt_lang` 用得了而且不是 zh-TW 時加 [`RESERVED_KEY`]；
/// 其餘情況回原樣（profile 裡原本就有的保留鍵一律拿掉：只有工具能寫）。沒有 profile 回 None。
pub fn augment(meth: Option<&Value>, dirs: &[PathBuf]) -> Option<Value> {
    let m = meth?;
    let Some(o) = m.as_object() else { return Some(m.clone()) };
    let table = resolve(m, dirs).ok().flatten();
    if table.is_none() && !o.contains_key(RESERVED_KEY) {
        return Some(m.clone());
    }
    let mut o = o.clone();
    o.shift_remove(RESERVED_KEY);
    if let Some((_, t)) = table {
        o.insert(RESERVED_KEY.to_string(), Value::Object(t));
    }
    Some(Value::Object(o))
}

/// 片段目錄（`.../workflow-templates/snippets`）旁邊的字串表目錄。
pub fn beside_snippets(snippet_dir: &Path) -> Option<PathBuf> {
    snippet_dir.parent().map(|p| p.join("i18n"))
}

/// `awf doctor` 用：寫了 `prompt_lang` 卻用不了時的說明（用得了或沒寫回 None）。
pub fn lang_problem(meth: &Value, dirs: &[PathBuf]) -> Option<String> {
    resolve(meth, dirs).err()
}

/// profile 的語意規則（`schema::methodology_errors` 呼叫）：`prompt_overrides` 只能蓋字串表裡有的鍵、值是 1–2000 字的字串；
/// 保留鍵不能自己寫。`prompt_lang` 的語言檔存不存在不在這裡判（退回預設、由 doctor 提示）。
/// `typed`：要不要回報 JSON Schema 也擋得到的型別與長度錯誤（同 `schema::methodology_errors_with`）；不認得的鍵與保留鍵一律回報。
pub fn profile_errors(doc: &Value, typed: bool) -> Vec<String> {
    let mut errs = Vec::new();
    if doc.get(RESERVED_KEY).is_some() {
        errs.push(format!("{RESERVED_KEY} 是工具內嵌語言檔用的保留鍵，不能寫在 methodology.json（要換派工單的字用 prompt_lang 或 prompt_overrides）"));
    }
    if let Some(l) = doc.get("prompt_lang").filter(|v| typed && !v.is_null()) {
        match l.as_str() {
            Some(s) if lang_name_ok(s) => {}
            Some(s) => errs.push(format!("prompt_lang 的「{s}」不是語言代號（例如 en、zh-TW）")),
            None => errs.push("prompt_lang 要是字串（例如 \"en\"）".into()),
        }
    }
    match doc.get("prompt_overrides") {
        None | Some(Value::Null) => {}
        Some(Value::Object(o)) => {
            for (k, v) in o {
                if !KEYS.contains(&k.as_str()) {
                    errs.push(format!("prompt_overrides 不認得的鍵：{k}（只能蓋字串表裡有的鍵，見 workflow-templates/i18n/zh-TW.json）"));
                    continue;
                }
                if v.as_str().is_some_and(|s| !keeps_required(k, s)) {
                    errs.push(format!(
                        "prompt_overrides.{k} 不能拿掉 {}（rules.md 必讀、輸出語言、派工單最後一行的執行紀錄檔不能省）",
                        MUST_KEEP.iter().find(|(x, _)| x == k).map(|(_, v)| v.iter().map(|n| format!("{{{n}}}")).collect::<Vec<_>>().join("、")).unwrap_or_default()
                            + if k == "record.tail" { "，而且要以 {record} 結尾" } else { "" }
                    ));
                }
                match v.as_str() {
                    _ if !typed => {}
                    Some(s) if !s.is_empty() && s.chars().count() <= OVERRIDE_MAX => {}
                    Some(_) => errs.push(format!("prompt_overrides.{k} 要是 1–{OVERRIDE_MAX} 字的字串")),
                    None => errs.push(format!("prompt_overrides.{k} 要是字串")),
                }
            }
        }
        Some(_) if typed => errs.push("prompt_overrides 要是物件（{鍵: 字串}）".into()),
        Some(_) => {}
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("awf-i18n-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lang_names() {
        for ok in ["en", "zh-TW", "pt-BR", "ja", "sr-Latn-RS"] {
            assert!(lang_name_ok(ok), "{ok}");
        }
        for bad in ["", "e", "english", "../en", "en/x", "en-", "zh_TW", "en-TOOLONGPART"] {
            assert!(!lang_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn vars_found() {
        let v = vars("- 必讀 {rules}（含）；{lang}。{ 不算 }{a_b}{}");
        assert_eq!(v.into_iter().collect::<Vec<_>>(), vec!["a_b", "lang", "rules"]);
    }

    #[test]
    fn augment_default_is_unchanged() {
        let d = tmp_dir("default");
        let m = json!({"name": "x", "prompt_overrides": {"prefix.external": "ACME"}});
        assert_eq!(augment(Some(&m), &[d.clone()]), Some(m.clone()));
        let zh = json!({"name": "x", "prompt_lang": "zh-TW"});
        assert_eq!(augment(Some(&zh), &[d.clone()]), Some(zh.clone()));
        assert_eq!(augment(None, &[d]), None);
    }

    #[test]
    fn augment_embeds_known_keys_only() {
        let d = tmp_dir("en");
        std::fs::write(d.join("en.json"), r#"{"prefix.heading": "Common", "nope": "x", "prefix.kb": ""}"#).unwrap();
        let m = json!({"name": "x", "prompt_lang": "en", "_prompt_strings": {"prefix.heading": "forged"}});
        let out = augment(Some(&m), &[d.clone()]).unwrap();
        assert_eq!(out[RESERVED_KEY], json!({"prefix.heading": "Common"}));
        // 找不到語言檔：拿掉偽造的保留鍵、不內嵌
        let ja = json!({"name": "x", "prompt_lang": "ja", "_prompt_strings": {"prefix.heading": "forged"}});
        assert_eq!(augment(Some(&ja), &[d.clone()]), Some(json!({"name": "x", "prompt_lang": "ja"})));
        assert!(lang_problem(&ja, &[d.clone()]).is_some_and(|m| m.contains("ja")));
        assert!(lang_problem(&json!({"prompt_lang": "en"}), &[d]).is_none());
    }

    fn profile_errors_t(v: &Value) -> Vec<String> {
        profile_errors(v, true)
    }

    #[test]
    fn override_rules() {
        assert_eq!(profile_errors(&json!({"prompt_overrides": {"prefix.nope": "x", "prefix.kb": ""}}), false).len(), 1);
        assert!(profile_errors_t(&json!({"prompt_lang": "en", "prompt_overrides": {"prefix.external": "ok"}})).is_empty());
        let e = profile_errors_t(&json!({"prompt_overrides": {"prefix.nope": "x", "prefix.kb": "", "prefix.kg": 3}}));
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e[0].contains("prefix.nope"));
        let long = "字".repeat(OVERRIDE_MAX + 1);
        assert_eq!(profile_errors_t(&json!({"prompt_overrides": {"prefix.kb": long}})).len(), 1);
        assert_eq!(profile_errors_t(&json!({"prompt_lang": "../x"})).len(), 1);
        assert_eq!(profile_errors_t(&json!({"_prompt_strings": {}})).len(), 1);
        // rules.md 與執行紀錄檔不能被蓋掉
        assert_eq!(profile_errors_t(&json!({"prompt_overrides": {"prefix.rules": "只讀 {rules}"}})).len(), 1);
        assert_eq!(profile_errors_t(&json!({"prompt_overrides": {"record.tail": "紀錄檔：{record}（必寫）"}})).len(), 1);
        assert!(profile_errors_t(&json!({"prompt_overrides": {"record.tail": "Record: {record}", "prefix.rules": "Read {rules}; language {lang}."}})).is_empty());
    }
}
