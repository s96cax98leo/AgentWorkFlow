//! 派工單的字串表（`plugin/workflow-templates/i18n/*.json`，profile 的 `prompt_lang` 選用；說明見 `awf_core::i18n`）的一致性：
//!   1. `zh-TW.json` 和片段 `snippets/challenge.js` 的 `awfText()`（內建的字）逐字相同、鍵的順序也相同（用 node 執行片段取值）。
//!   2. 每一份語言檔的鍵集合等於 `awf_core::i18n::KEYS`（工具端的語意規則照它判 `prompt_overrides` 的鍵）。
//!   3. 每個字串的 `{變數}` 集合和 zh-TW 的同一個鍵相同；rules.md、輸出語言、執行紀錄檔那幾個變數不能少（`i18n::MUST_KEEP`）。
//!   4. 值是 1–`i18n::LANG_MAX` 字的字串；檔案裡沒有連續兩個左大括號（init 會當成佔位符）。
//!   5. 內建的語言（zh-TW、en）都在。
//!
//! 沒有 node 時判失敗（不略過）。

use crate::util::{self, Captured};
use awf_core::i18n;
use awf_core::merge_gate::pyutil;
use serde_json::Value;
use std::path::Path;

const BUILTIN: [&str; 2] = ["zh-TW", "en"];

const JS: &str = "const fs = require('fs'); const src = fs.readFileSync(process.argv[1], 'utf8');\
const t = new Function(src + '\\nreturn awfText()')(); process.stdout.write(JSON.stringify(Object.entries(t)))";

/// 一份語言檔的問題。`zh` 是 zh-TW 的內容（拿來比變數）。
fn file_errors(name: &str, text: &str, zh: Option<&serde_json::Map<String, Value>>) -> Vec<String> {
    let mut errs = Vec::new();
    if text.contains(&"{".repeat(2)) {
        errs.push(format!("{name}：不能有連續兩個左大括號（init 會當成佔位符）"));
    }
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return vec![format!("{name}：不是合法 JSON：{e}")],
    };
    let Some(o) = v.as_object() else { return vec![format!("{name}：不是 JSON 物件")] };
    let keys: Vec<&str> = o.keys().map(String::as_str).collect();
    let missing: Vec<&str> = i18n::KEYS.iter().copied().filter(|k| !keys.contains(k)).collect();
    let extra: Vec<&str> = keys.iter().copied().filter(|k| !i18n::KEYS.contains(k)).collect();
    if !missing.is_empty() {
        errs.push(format!("{name}：少了鍵 {}", missing.join("、")));
    }
    if !extra.is_empty() {
        errs.push(format!("{name}：多了 awf_core::i18n::KEYS 沒有的鍵 {}", extra.join("、")));
    }
    for (k, x) in o {
        let Some(s) = x.as_str().filter(|s| !s.is_empty() && s.chars().count() <= i18n::LANG_MAX) else {
            errs.push(format!("{name}：{k} 要是 1–{} 字的字串", i18n::LANG_MAX));
            continue;
        };
        if !i18n::keeps_required(k, s) {
            errs.push(format!("{name}：{k} 少了不能省的變數（rules.md、輸出語言、執行紀錄檔）"));
        }
        if let Some(z) = zh.and_then(|z| z.get(k)).and_then(Value::as_str) {
            let (a, b) = (i18n::vars(z), i18n::vars(s));
            if a != b {
                errs.push(format!("{name}：{k} 的變數 {:?} 和 zh-TW 的 {:?} 不同", b, a));
            }
        }
    }
    errs
}

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path) -> Captured {
    let plugin = util::plugin_dir(root);
    let dir = awf_core::template::i18n(&plugin);
    let mut errs: Vec<String> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(lang) = name.strip_suffix(".json") {
            if !i18n::lang_name_ok(lang) {
                errs.push(format!("{name}：檔名不是語言代號（profile 的 prompt_lang 選不到它）"));
            }
            match util::read_text(&e.path()) {
                Ok(t) => files.push((name, t)),
                Err(m) => errs.push(m),
            }
        }
    }
    files.sort();
    for b in BUILTIN {
        if !files.iter().any(|(n, _)| n == &format!("{b}.json")) {
            errs.push(format!("缺內建的語言檔 i18n/{b}.json"));
        }
    }
    let zh: Option<serde_json::Map<String, Value>> = files.iter().find(|(n, _)| n == "zh-TW.json")
        .and_then(|(_, t)| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.as_object().cloned());
    for (n, t) in &files {
        errs.extend(file_errors(n, t, zh.as_ref()));
    }
    // 片段的內建字和 zh-TW.json 逐字相同、順序相同
    match util::which("node") {
        None => errs.push("找不到 node，無法核對片段的內建字（fail-closed）".into()),
        Some(node) => {
            let snippet = util::snippet_dir(root).join("challenge.js");
            match util::node_eval(&node, JS, &[snippet], None) {
                Ok(q) if q.code == 0 => match serde_json::from_str::<Vec<(String, String)>>(&q.stdout) {
                    Ok(pairs) => {
                        let want: Vec<(String, String)> = zh.iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect();
                        if pairs != want {
                            let first = pairs.iter().zip(want.iter()).find(|(a, b)| a != b).map(|(a, _)| a.0.clone())
                                .unwrap_or_else(|| format!("鍵數 {} 對 {}", pairs.len(), want.len()));
                            errs.push(format!("片段 challenge.js 的 awfText() 和 i18n/zh-TW.json 不一致（第一個不同：{first}）；改了片段的字就照抄到 zh-TW.json，en.json 一起改"));
                        }
                    }
                    Err(_) => errs.push(format!("awfText() 的輸出讀不懂：{}", pyutil::tail(&q.stdout, 300))),
                },
                Ok(q) => errs.push(format!("node 執行片段失敗：{}", pyutil::tail(&q.stderr, 600))),
                Err(e) => errs.push(format!("node 執行失敗：{e}")),
            }
        }
    }
    let mut out = String::new();
    for e in &errs {
        out.push_str(&format!("  [失敗] {e}\n"));
    }
    if errs.is_empty() {
        out.push_str(&format!("check_i18n：通過（{} 份語言檔、{} 個鍵；zh-TW＝片段的內建字，變數一致）\n", files.len(), i18n::KEYS.len()));
        (0, out, String::new())
    } else {
        out.push_str(&format!("check_i18n：{} 項失敗\n", errs.len()));
        (1, out, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_bad_files() {
        let zh: serde_json::Map<String, Value> = i18n::KEYS.iter().map(|k| (k.to_string(), Value::String(format!("{k} {{x}}")))).collect();
        let ok = serde_json::to_string(&zh).unwrap();
        assert!(file_errors("zh-TW.json", &ok, Some(&zh)).iter().all(|e| e.contains("不能省")), "只有 MUST_KEEP 的鍵會報");
        let mut bad = zh.clone();
        bad.insert("prefix.heading".into(), Value::String("no vars".into()));
        bad.insert("extra.key".into(), Value::String("x".into()));
        bad.shift_remove("sep.list");
        let errs = file_errors("en.json", &serde_json::to_string(&bad).unwrap(), Some(&zh));
        assert!(errs.iter().any(|e| e.contains("少了鍵 sep.list")) && errs.iter().any(|e| e.contains("extra.key")) && errs.iter().any(|e| e.contains("prefix.heading 的變數")), "{errs:?}");
        assert!(file_errors("x.json", "{\"a\": \"{{X}}\"}", None).iter().any(|e| e.contains("左大括號")));
    }
}
