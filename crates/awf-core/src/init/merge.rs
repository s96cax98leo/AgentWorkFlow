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

/// 0.x 的註冊：`… awf_hook.py <種類>`（Python 腳本）。
fn old_hook_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!(r"awf_hook\.py\W*[{WS_CLASS}]+(guard-shell|guard-paths|envelope)\b")).expect("固定的正規式"))
}

/// 1.0 的註冊：`<路徑>/awf hook <種類>`（執行檔；路徑可以加引號，Windows 是 `awf.exe`）。
/// 種類＝專案 settings 範本會註冊的那幾種（守門、交件，加上輔助的 `mark-dirty`、`prompt-context`）。
/// `awf` 前面要是開頭、空白、路徑分隔或引號，才不會把使用者自己叫 `my-awf` 之類的指令認成我們的。
fn new_hook_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&format!(r#"(?:^|[{WS_CLASS}/\\"'&])awf(?:\.exe)?["']?[{WS_CLASS}]+hook[{WS_CLASS}]+(guard-shell|guard-paths|envelope|mark-dirty|prompt-context)\b"#)).expect("固定的正規式"))
}

/// 這段文字（一個指令，或整份 hooks 的 JSON）裡有沒有（0.x 的註冊, 1.0 的註冊）。
pub fn hook_forms(text: &str) -> (bool, bool) {
    (old_hook_rx().is_match(text) || text.contains("awf_hook.py"), new_hook_rx().is_match(&text.replace("\\\"", "\"")))
}

/// 這個 hook 是我們的哪個種類（0.x 的 `awf_hook.py <種類>` 與 1.0 的 `awf hook <種類>` 都認）；不是就回 None。
pub fn awf_mode(h: &Map<String, Value>) -> Option<String> {
    let cmd = h.get("command").map(pyfmt::str_of).unwrap_or_default();
    old_hook_rx().captures(&cmd).or_else(|| new_hook_rx().captures(&cmd)).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

/// 我們的 hook 每個種類在每個事件只留一份：舊的（含 0.x 的 `awf_hook.py`）不論掛在哪個 matcher 都先拿掉，
/// 再放上模版這版的；使用者自己的 hooks 原樣保留。
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

/// 0.x 的 settings 範本放的權限 → 1.0 的寫法（None＝1.0 沒有對應的東西，拿掉）。
/// 只列已發佈各版範本裡真的出現過的項目；使用者自己加的（例如別的工具的 `python3 …`）不在這裡，不會被動到。
pub const OLD_ALLOW: &[(&str, Option<&str>)] = &[
    ("Bash(python3 .claude/teams/tools/build.py:*)", Some("Bash(awf build:*)")),
    ("Bash(python3 .claude/teams/tools/check_envelope.py:*)", Some("Bash(awf check-envelope:*)")),
    ("Bash(python3 .claude/teams/tools/check_rules.py:*)", Some("Bash(awf check-rules:*)")),
    ("Bash(python3 .claude/teams/tools/check_agents.py:*)", Some("Bash(awf check-agents:*)")),
    ("Bash(python3 .claude/teams/tools/save_run.py:*)", Some("Bash(awf save-run:*)")),
    ("Bash(node .claude/teams/tools/check_workflows.mjs:*)", None),
];

/// 把 `permissions.allow` 裡 0.x 範本放的項目換成 1.0 的寫法（就地換，順序不變；新的已經有了就只拿掉舊的）。
/// `keep` 回 true 的舊項目不動（例如它指的檔還在專案裡）。回傳（舊的, 換成什麼）。
pub fn migrate_allow(doc: &mut Map<String, Value>, keep: &dyn Fn(&str) -> bool) -> Vec<(String, Option<String>)> {
    let mut changed = Vec::new();
    let Some(allow) = doc.get_mut("permissions").and_then(Value::as_object_mut).and_then(|p| p.get_mut("allow")).and_then(Value::as_array_mut) else { return changed };
    let mut out: Vec<Value> = Vec::with_capacity(allow.len());
    let have = |list: &[Value], s: &str| list.iter().any(|v| v.as_str() == Some(s));
    for v in allow.iter() {
        let hit = v.as_str().and_then(|s| OLD_ALLOW.iter().find(|(old, _)| *old == s)).filter(|(old, _)| !keep(old));
        match hit {
            None => out.push(v.clone()),
            Some((old, new)) => {
                if let Some(n) = new.filter(|n| !have(&out, n) && !have(allow, n)) {
                    out.push(Value::String(n.to_string()));
                }
                changed.push((old.to_string(), new.map(str::to_string)));
            }
        }
    }
    if !changed.is_empty() {
        *allow = out;
    }
    changed
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
    fn old_and_new_registrations_are_both_ours() {
        let mode = |cmd: &str| awf_mode(&m(json!({"command": cmd})));
        // 0.x
        assert_eq!(mode("\"/usr/bin/python3\" \"${CLAUDE_PROJECT_DIR}/.claude/hooks/awf_hook.py\" guard-shell").as_deref(), Some("guard-shell"));
        assert_eq!(mode("& \"C:\\Py\\python.exe\" \"${CLAUDE_PROJECT_DIR}/.claude/hooks/awf_hook.py\" envelope").as_deref(), Some("envelope"));
        // 1.0
        assert_eq!(mode("\"/p/.claude/hooks/bin/awf\" hook guard-paths").as_deref(), Some("guard-paths"));
        assert_eq!(mode("& \"C:\\p\\.claude\\hooks\\bin\\awf.exe\" hook guard-shell").as_deref(), Some("guard-shell"));
        assert_eq!(mode("awf hook envelope").as_deref(), Some("envelope"));
        assert_eq!(mode("\"/p/.claude/hooks/bin/awf\" hook mark-dirty").as_deref(), Some("mark-dirty"));
        assert_eq!(mode("& \"C:\\p\\.claude\\hooks\\bin\\awf.exe\" hook prompt-context").as_deref(), Some("prompt-context"));
        // 使用者自己的
        for cmd in ["echo hi", "my-awf hook envelope", "awfx hook envelope", "awf hooks envelope", "awf hook session-start", "awf kb query hook guard-shell", "python3 my_hook.py guard-shell"] {
            assert_eq!(mode(cmd), None, "{cmd}");
        }
        assert_eq!(hook_forms("{\"command\":\"\\\"/p/.claude/hooks/bin/awf\\\" hook guard-shell\"}"), (false, true));
        assert_eq!(hook_forms("{\"command\":\"py x/awf_hook.py guard-shell\"}"), (true, false));
        assert_eq!(hook_forms("{\"command\":\"echo hi\"}"), (false, false));
    }

    #[test]
    fn upgrade_replaces_old_registration_only() {
        let tmpl = m(json!({"PreToolUse": [{"matcher": "Bash|PowerShell", "hooks": [{"command": "\"/p/.claude/hooks/bin/awf\" hook guard-shell"}]}],
            "SubagentStop": [{"hooks": [{"command": "\"/p/.claude/hooks/bin/awf\" hook envelope"}]}]}));
        let user = m(json!({"PreToolUse": [
            {"matcher": "Bash|PowerShell", "hooks": [{"command": "\"py\" \"x/.claude/hooks/awf_hook.py\" guard-shell", "timeout": 10}]},
            {"matcher": "Bash", "hooks": [{"command": "echo mine"}, {"command": "\"/old/.claude/hooks/bin/awf\" hook guard-shell"}]}
        ], "SubagentStop": [{"hooks": [{"command": "py awf_hook.py envelope"}, {"command": "my-awf hook envelope"}]}], "Stop": [{"hooks": [{"command": "echo done"}]}]}));
        let got = merge_hooks(&user, &tmpl).ok().unwrap_or_default();
        assert_eq!(Value::Object(got.clone()), json!({"PreToolUse": [
            {"matcher": "Bash", "hooks": [{"command": "echo mine"}]},
            {"matcher": "Bash|PowerShell", "hooks": [{"command": "\"/p/.claude/hooks/bin/awf\" hook guard-shell"}]}
        ], "SubagentStop": [{"hooks": [{"command": "my-awf hook envelope"}]}, {"hooks": [{"command": "\"/p/.claude/hooks/bin/awf\" hook envelope"}]}],
            "Stop": [{"hooks": [{"command": "echo done"}]}]}));
        // 再跑一次不變
        assert_eq!(merge_hooks(&got, &tmpl).ok(), Some(got));
    }

    #[test]
    fn upgrade_adds_missing_auxiliary_hooks_once() {
        let h = |kind: &str| json!({"type": "command", "command": format!("\"/p/.claude/hooks/bin/awf\" hook {kind}")});
        let tmpl = m(json!({"PreToolUse": [{"matcher": "Bash|PowerShell", "hooks": [h("guard-shell")]}],
            "PostToolUse": [{"matcher": "Edit|Write", "hooks": [h("mark-dirty")]}], "UserPromptSubmit": [{"hooks": [h("prompt-context")]}]}));
        // 第 10 批以前用 --hooks project 套用的專案：沒有後兩種；使用者自己另有一個 PostToolUse
        let user = m(json!({"PreToolUse": [{"matcher": "Bash|PowerShell", "hooks": [h("guard-shell")]}],
            "PostToolUse": [{"matcher": "Write", "hooks": [{"command": "echo mine"}]}]}));
        let got = merge_hooks(&user, &tmpl).ok().unwrap_or_default();
        assert_eq!(Value::Object(got.clone()), json!({"PreToolUse": [{"matcher": "Bash|PowerShell", "hooks": [h("guard-shell")]}],
            "PostToolUse": [{"matcher": "Write", "hooks": [{"command": "echo mine"}]}, {"matcher": "Edit|Write", "hooks": [h("mark-dirty")]}],
            "UserPromptSubmit": [{"hooks": [h("prompt-context")]}]}));
        // 再跑一次不變（不重複註冊）；手動加過（別的路徑、別的 matcher）的也只留一份
        assert_eq!(merge_hooks(&got, &tmpl).ok(), Some(got));
        let manual = m(json!({"PostToolUse": [{"matcher": "Edit|Write|NotebookEdit", "hooks": [{"command": "awf hook mark-dirty"}]}]}));
        let got = merge_hooks(&manual, &tmpl).ok().unwrap_or_default();
        assert_eq!(got["PostToolUse"], json!([{"matcher": "Edit|Write", "hooks": [h("mark-dirty")]}]));
    }

    #[test]
    fn allow_migration_touches_only_our_entries() {
        let mut doc = m(json!({"permissions": {"allow": [
            "Bash(npm test)",
            "Bash(python3 .claude/teams/tools/build.py:*)",
            "Bash(python3 .claude/teams/tools/kb.py:*)",
            "Bash(python3 .claude/teams/tools/save_run.py:*)",
            "Bash(awf save-run:*)",
            "Bash(node .claude/teams/tools/check_workflows.mjs:*)",
            "Bash(python3 .claude/teams/tools/check_rules.py --git:*)",
            "Bash(git status:*)"
        ], "deny": ["Bash(python3 .claude/teams/tools/build.py:*)"]}}));
        let changed = migrate_allow(&mut doc, &|_| false);
        assert_eq!(changed.len(), 3);
        assert_eq!(Value::Object(doc.clone()), json!({"permissions": {"allow": [
            "Bash(npm test)",
            "Bash(awf build:*)",
            "Bash(python3 .claude/teams/tools/kb.py:*)",
            "Bash(awf save-run:*)",
            "Bash(python3 .claude/teams/tools/check_rules.py --git:*)",
            "Bash(git status:*)"
        ], "deny": ["Bash(python3 .claude/teams/tools/build.py:*)"]}}));
        assert!(migrate_allow(&mut doc, &|_| false).is_empty());
        // 指的檔還在：不動
        let mut kept = m(json!({"permissions": {"allow": ["Bash(node .claude/teams/tools/check_workflows.mjs:*)"]}}));
        assert!(migrate_allow(&mut kept, &|old| old.contains("check_workflows")).is_empty());
        // 形狀不對：不動、不當掉
        let mut odd = m(json!({"permissions": {"allow": "all"}}));
        assert!(migrate_allow(&mut odd, &|_| false).is_empty());
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
