//! 專案的主線分支與目錄布局（`guard.config.json` 的 `base_branch`、`base_remote`）。
//!
//! 值放在守門設定而不是 profile：hook、工具（sync-branch、publish、recipe check-diff、deliverables）與產生 workflow 腳本
//! 的狀態機都已經讀這一份，team agent 改不到它，而且它本來就記這個專案的 repo 長相（fork、上游、受保護分支、run 目錄）。
//! init 從佔位符 `BASE_BRANCH`、`BASE_REMOTE` 寫進去；範本的同名佔位符在產生腳本時也從這裡取值（[`placeholders`]），
//! 所以範本、工具讀到的一定是同一個值。
//!
//! 規則只有一條：沒寫、型別不對、不合規則，一律用預設值（＝原本寫死的 `main`、`origin`）。命令列旗標（`--base`、`--remote`）
//! 仍然優先。受保護分支（`protected_branches`）是另一件事：這裡的值不會讓任何分支變成可以直接 push。

use serde_json::Value;

pub const DEFAULT_BASE_BRANCH: &str = "main";
pub const DEFAULT_BASE_REMOTE: &str = "origin";

/// 合併預設後的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// 主線分支名（不含 remote）。
    pub base_branch: String,
    /// 主線所在的 remote。
    pub base_remote: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { base_branch: DEFAULT_BASE_BRANCH.to_string(), base_remote: DEFAULT_BASE_REMOTE.to_string() }
    }
}

/// 分支名：英數與 `._/-`，1–100 字；不以 `-`、`/`、`.` 開頭，不以 `/`、`.`、`.lock` 結尾，沒有 `..` 與 `//`。
/// （比 git 的規則窄：值會寫進派工單、交給 git 當參數，也會內嵌進 workflow 腳本的字串。）
pub fn branch_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 100
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'))
        && !matches!(b[0], b'-' | b'/' | b'.')
        && !s.ends_with('/') && !s.ends_with('.') && !s.ends_with(".lock")
        && !s.contains("..") && !s.contains("//")
}

/// remote 名：英數開頭，英數與 `._-`，1–64 字。
pub fn remote_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && !s.contains("..") && !s.ends_with('.') && !s.ends_with(".lock")
}

fn pick(guard: &Value, key: &str, ok: fn(&str) -> bool, default: &str) -> String {
    match guard.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if ok(s) => s.to_string(),
        _ => default.to_string(),
    }
}

impl Layout {
    /// 從讀好的 guard.config.json（JSON 物件）取值；不是物件就是全部預設。
    pub fn of(guard: &Value) -> Layout {
        Layout {
            base_branch: pick(guard, "base_branch", branch_ok, DEFAULT_BASE_BRANCH),
            base_remote: pick(guard, "base_remote", remote_ok, DEFAULT_BASE_REMOTE),
        }
    }

    /// 沒帶 `base` 時的比對基準：`<remote>/<branch>`（預設 `origin/main`）。
    pub fn base_ref(&self) -> String {
        format!("{}/{}", self.base_remote, self.base_branch)
    }
}

/// 讀一份 guard.config.json；讀不到、不是 JSON 物件回 `Null`（呼叫端得到全部預設）。
pub fn read_guard(path: &str) -> Value {
    match crate::config::read_text(path).ok().and_then(|t| crate::merge_gate::pyjson::loads_lenient(&t).ok()) {
        Some(v) if v.is_object() => v,
        _ => Value::Null,
    }
}

/// 專案（`<root>/.claude/teams/guard.config.json`）的布局。
pub fn for_project(root: &str) -> Layout {
    Layout::of(&read_guard(&crate::config::config_path(root)))
}

/// 從目前目錄找到的專案的布局（`CLAUDE_PROJECT_DIR`，否則往上找 `.claude/teams/VERSION`）；找不到專案就是全部預設。
pub fn current() -> Layout {
    match crate::project::find(None) {
        Some(f) => for_project(&f.root),
        None => Layout::default(),
    }
}

/// 守門設定的鍵 ↔ `workflow.config` 的佔位符（init 的紀錄檔要跟著 `--reconfigure` 改的那幾個）。
pub const RECORD_KEYS: &[(&str, &str)] = &[("base_branch", "BASE_BRANCH"), ("base_remote", "BASE_REMOTE")];

/// 範本裡由這裡供值的佔位符（鍵, 值）。`workflow.config` 的同名值只是 init 的輸入，產生腳本時不看它。
pub fn placeholders(l: &Layout) -> Vec<(&'static str, String)> {
    vec![("BASE_BRANCH", l.base_branch.clone()), ("BASE_REMOTE", l.base_remote.clone())]
}

/// 守門設定裡這幾個鍵寫了但不合規則的（`awf init` 的答案檔與 `awf doctor` 用）：（鍵, 說明）。
pub fn problems(guard: &Value) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let mut chk = |key: &'static str, ok: fn(&str) -> bool, rule: &str, default: &str| {
        match guard.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.trim().is_empty() || ok(s.trim()) => {}
            Some(v) => out.push((key, format!("{key} 不合規則（{rule}；收到 {}）：工具與範本改用預設值 {default}", crate::pyfmt::repr(v)))),
        }
    };
    chk("base_branch", branch_ok, "分支名只收英數與 . _ / -，不含 remote", DEFAULT_BASE_BRANCH);
    chk("base_remote", remote_ok, "remote 名只收英數與 . _ -", DEFAULT_BASE_REMOTE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_when_missing_or_invalid() {
        assert_eq!(Layout::of(&Value::Null), Layout::default());
        assert_eq!(Layout::of(&json!({})).base_ref(), "origin/main");
        assert_eq!(Layout::of(&json!({"base_branch": "develop", "base_remote": "upstream"})).base_ref(), "upstream/develop");
        assert_eq!(Layout::of(&json!({"base_branch": " release/1.x "})).base_ref(), "origin/release/1.x");
        for bad in [json!(""), json!("{{BASE_BRANCH}}"), json!("-x"), json!("a..b"), json!("a b"), json!("a;rm"), json!("x'"), json!("a`b"), json!("${x}"), json!("a/"),
            json!("x.lock"), json!(3), json!(["main"]), json!("origin/main\n"), json!("a\\b"), json!("/abs"), json!(".hidden")] {
            let l = Layout::of(&json!({"base_branch": bad, "base_remote": bad}));
            if bad == json!("origin/main\n") {
                // 前後空白會去掉：這個值合規則
                assert_eq!(l.base_branch, "origin/main");
                continue;
            }
            assert_eq!(l, Layout::default(), "{bad}");
        }
        assert!(!remote_ok("a/b") && remote_ok("up-stream.1"));
    }

    #[test]
    fn problems_list_bad_values() {
        assert!(problems(&json!({"base_branch": "develop", "base_remote": ""})).is_empty());
        let p = problems(&json!({"base_branch": "a b", "base_remote": 3}));
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), vec!["base_branch", "base_remote"]);
    }
}
