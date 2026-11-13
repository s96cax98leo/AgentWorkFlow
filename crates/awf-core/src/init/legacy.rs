//! 升級時整理舊版複製進專案的 workflow 副本（`.claude/workflows/*.js`）。
//!
//! 0.5 起 workflow 不複製進專案。`plugin/workflow-templates/legacy.json` 記了各版範本切開後每一段的長度與雜湊；
//! 原樣的舊副本（只差佔位符的值與內嵌的 profile）移除，改過的保留並說明，有覆寫標記的照用，使用者自己的不碰。

use super::writer::{js_files, ours, wf_override};
use super::{read_text, Kv};
use crate::merge_gate::pyjson;
use regex::Regex;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

fn wf_meth() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)\A/\*@methodology\*/.*?/\*@end\*/").expect("固定的正規式"))
}

fn sha1_16(s: &str) -> String {
    let hex: String = Sha1::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(16).collect()
}

/// 從位元組位置 `pos` 起取 `n` 個字元；不夠長回 None。回傳（那一段, 下一個位置）。
pub(super) fn take_chars(text: &str, pos: usize, n: usize) -> Option<(&str, usize)> {
    let rest = text.get(pos..)?;
    if n == 0 {
        return Some(("", pos));
    }
    let (last, c) = rest.char_indices().nth(n - 1)?;
    let end = last + c.len_utf8();
    Some((rest.get(..end)?, pos + end))
}

/// 專案裡的 workflow 是不是某一版 init 原樣複製的副本（只差佔位符的值與內嵌的 profile）。是的話回傳版本，不是回空字串。
pub fn pristine_legacy_workflow(text: &str, name: &str, vals: &Kv, legacy: &Value) -> String {
    let text = text.replace("\r\n", "\n");
    let entries = legacy.get("files").and_then(|f| f.get(name)).and_then(Value::as_array);
    for entry in entries.into_iter().flatten() {
        let keys: Vec<&str> = entry.get("keys").and_then(Value::as_array).map(|a| a.iter().map(|k| k.as_str().unwrap_or_default()).collect()).unwrap_or_default();
        let segs: &[Value] = entry.get("segs").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let (mut pos, mut ok) = (0usize, true);
        for (i, seg) in segs.iter().enumerate() {
            let ln = seg.get(0).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok());
            let hash = seg.get(1).and_then(Value::as_str);
            let got = ln.and_then(|n| take_chars(&text, pos, n));
            let (Some((piece, next)), Some(hash)) = (got, hash) else {
                ok = false;
                break;
            };
            if sha1_16(piece) != hash {
                ok = false;
                break;
            }
            pos = next;
            let Some(key) = keys.get(i) else { continue };
            let rest = text.get(pos..).unwrap_or_default();
            if *key == "@METH" {
                match wf_meth().find(rest) {
                    Some(m) => pos += m.end(),
                    None => {
                        ok = false;
                        break;
                    }
                }
            } else {
                match vals.get(key) {
                    Some(v) if rest.starts_with(v) => pos += v.len(),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        if ok && pos == text.len() {
            let versions: Vec<&str> = entry.get("versions").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            return if versions.is_empty() { "?".to_string() } else { versions.join("、") };
        }
    }
    String::new()
}

/// 整理的結果：（移除的原樣舊副本, 保留的覆寫檔, 保留但不會被採用的檔）。
#[derive(Debug, Default, PartialEq)]
pub struct Tidy {
    pub removed: Vec<String>,
    pub overrides: Vec<String>,
    pub kept: Vec<String>,
}

/// 升級時整理專案的 `.claude/workflows/`。使用者自己的 workflow 不碰。
pub fn tidy_old_workflows(target: &Path, wf_templates: &Path, vals: &Kv) -> Tidy {
    let mut out = Tidy::default();
    let d = target.join(".claude").join("workflows");
    let legacy = read_text(&wf_templates.join("legacy.json")).ok().and_then(|t| pyjson::loads(&t).ok()).unwrap_or(Value::Null);
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut known: BTreeSet<String> = js_files(wf_templates).iter().map(|p| name_of(p)).collect();
    if let Some(files) = legacy.get("files").and_then(Value::as_object) {
        known.extend(files.keys().cloned());
    }
    if !d.is_dir() {
        return out;
    }
    for f in js_files(&d) {
        let name = name_of(&f);
        if !known.contains(&name) {
            continue;
        }
        let Ok(text) = read_text(&f) else { continue };
        if wf_override().is_match(&text) {
            out.overrides.push(name);
        } else if !ours().is_match(&text) {
            continue; // 同名但不是模版的檔：使用者自己的
        } else if !pristine_legacy_workflow(&text, &name, vals, &legacy).is_empty() {
            if std::fs::remove_file(&f).is_ok() {
                out.removed.push(name);
            }
        } else {
            out.kept.push(name);
        }
    }
    let empty = std::fs::read_dir(&d).map(|mut rd| rd.next().is_none()).unwrap_or(false);
    if empty {
        let _ = std::fs::remove_dir(&d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pristine_needs_exact_segments() {
        let (a, b, c) = ("const NS = '", "'\nconst M = ", "\n// 結尾\n");
        let legacy = json!({"files": {"x.js": [{"versions": ["0.3.0", "0.4.0"], "keys": ["AGENT_NS", "@METH"],
            "segs": [[a.chars().count(), sha1_16(a)], [b.chars().count(), sha1_16(b)], [c.chars().count(), sha1_16(c)]]}]}});
        let mut vals = Kv::default();
        vals.set("AGENT_NS", "示範:");
        let good = format!("{a}示範:{b}/*@methodology*/ {{\"name\":\"default\"}} /*@end*/{c}");
        assert_eq!(pristine_legacy_workflow(&good, "x.js", &vals, &legacy), "0.3.0、0.4.0");
        assert_eq!(pristine_legacy_workflow(&good.replace('\n', "\r\n"), "x.js", &vals, &legacy), "0.3.0、0.4.0");
        assert_eq!(pristine_legacy_workflow(&format!("{good} "), "x.js", &vals, &legacy), "");
        assert_eq!(pristine_legacy_workflow(&good.replace("示範:", "別的:"), "x.js", &vals, &legacy), "");
        assert_eq!(pristine_legacy_workflow(&good.replace("結尾", "改過"), "x.js", &vals, &legacy), "");
        assert_eq!(pristine_legacy_workflow(&good, "y.js", &vals, &legacy), "");
    }
}
