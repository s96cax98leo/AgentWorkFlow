//! 專案的主線分支與目錄布局（`guard.config.json` 的 `base_branch`、`base_remote`、`specs_dir`、`worktrees_dir`、`spec_templates_dir`）。
//!
//! 值放在守門設定而不是 profile：hook、工具（sync-branch、publish、recipe check-diff、deliverables）與產生 workflow 腳本
//! 的狀態機都已經讀這一份，team agent 改不到它，而且它本來就記這個專案的 repo 長相（fork、上游、受保護分支、run 目錄）。
//! init 從佔位符 `BASE_BRANCH`、`BASE_REMOTE`、`SPECS_DIR`、`WORKTREES_DIR`、`SPEC_TEMPLATES_DIR` 寫進去；範本的同名佔位符在產生腳本時也從這裡取值（[`placeholders`]），
//! 所以範本、工具讀到的一定是同一個值。
//!
//! 規則只有一條：沒寫、型別不對、不合規則，一律用預設值（＝原本寫死的 `main`、`origin`、`specs`、`.worktrees`、
//! `<spec 目錄>/_templates`）。目錄都是「相對於目標 repo」的路徑：spec 目錄與範本目錄可以有好幾層（`docs/specs`），
//! worktree 目錄只能是一層的目錄名（工具用它當「走訪時略過的目錄名」）。命令列旗標（`--base`、`--remote`）
//! 仍然優先。受保護分支（`protected_branches`）是另一件事：這裡的值不會讓任何分支變成可以直接 push。

use serde_json::Value;

pub const DEFAULT_BASE_BRANCH: &str = "main";
pub const DEFAULT_BASE_REMOTE: &str = "origin";
pub const DEFAULT_SPECS_DIR: &str = "specs";
pub const DEFAULT_WORKTREES_DIR: &str = ".worktrees";

/// 合併預設後的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// 主線分支名（不含 remote）。
    pub base_branch: String,
    /// 主線所在的 remote。
    pub base_remote: String,
    /// spec 目錄（相對於目標 repo；`<specs_dir>/<epic>/` 放 intent、spec、plan…）。
    pub specs_dir: String,
    /// worktree 目錄（目標 repo 底下的一層目錄名；`<worktrees_dir>/<epic>` 是 epic worktree）。
    pub worktrees_dir: String,
    /// 文件範本目錄（相對於目標 repo；沒寫＝`<specs_dir>/_templates`）。
    pub spec_templates_dir: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            base_branch: DEFAULT_BASE_BRANCH.to_string(),
            base_remote: DEFAULT_BASE_REMOTE.to_string(),
            specs_dir: DEFAULT_SPECS_DIR.to_string(),
            worktrees_dir: DEFAULT_WORKTREES_DIR.to_string(),
            spec_templates_dir: format!("{DEFAULT_SPECS_DIR}/_templates"),
        }
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

/// 目錄（相對於目標 repo）：1–4 層，每層英數開頭或 `.`／`_` 開頭、只有英數與 `._-`，全長 100 字以內；
/// 不是絕對路徑、沒有 `.`／`..` 這種層、不在 `.claude`、`.github`、`.git` 底下（不分大小寫；`.git` 在哪一層都不行）。
/// 寫的時候前後的 `/` 會先去掉（[`dir_norm`]）。
pub fn dir_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    !s.is_empty() && s.len() <= 100 && parts.len() <= 4
        && parts.iter().all(|p| {
            let b = p.as_bytes();
            !b.is_empty() && b.len() <= 64 && *p != "." && !p.contains("..")
                && (b[0].is_ascii_alphanumeric() || matches!(b[0], b'.' | b'_'))
                && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                && !p.ends_with('.') && !p.eq_ignore_ascii_case(".git")
        })
        && !matches!(parts[0].to_ascii_lowercase().as_str(), ".claude" | ".github" | ".git")
}

/// worktree 目錄：合 [`dir_ok`] 而且只有一層。
pub fn worktrees_ok(s: &str) -> bool {
    dir_ok(s) && !s.contains('/')
}

/// 目錄值的正規化：去前後空白與頭尾的 `/`（`specs/` 和 `specs` 是同一個值）。開頭有 `/` 的是絕對路徑，不動它、讓檢查擋掉。
pub fn dir_norm(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('/') { t.to_string() } else { t.trim_end_matches('/').to_string() }
}

fn pick_dir(guard: &Value, key: &str, ok: fn(&str) -> bool, default: &str) -> String {
    match guard.get(key).and_then(Value::as_str).map(dir_norm) {
        Some(s) if ok(&s) => s,
        _ => default.to_string(),
    }
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
        let specs_dir = pick_dir(guard, "specs_dir", dir_ok, DEFAULT_SPECS_DIR);
        Layout {
            base_branch: pick(guard, "base_branch", branch_ok, DEFAULT_BASE_BRANCH),
            base_remote: pick(guard, "base_remote", remote_ok, DEFAULT_BASE_REMOTE),
            spec_templates_dir: pick_dir(guard, "spec_templates_dir", dir_ok, &format!("{specs_dir}/_templates")),
            specs_dir,
            worktrees_dir: pick_dir(guard, "worktrees_dir", worktrees_ok, DEFAULT_WORKTREES_DIR),
        }
    }

    /// 是不是全部用預設的目錄（呼叫端用來走「和原本逐字相同」的那條路）。
    pub fn default_dirs(&self) -> bool {
        self.specs_dir == DEFAULT_SPECS_DIR && self.worktrees_dir == DEFAULT_WORKTREES_DIR
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
pub const RECORD_KEYS: &[(&str, &str)] = &[("base_branch", "BASE_BRANCH"), ("base_remote", "BASE_REMOTE"), ("specs_dir", "SPECS_DIR"),
    ("worktrees_dir", "WORKTREES_DIR"), ("spec_templates_dir", "SPEC_TEMPLATES_DIR")];

/// 這個行程所在專案的布局（只算一次）：沒有專案根目錄可以傳的地方用（走訪時要略過的目錄名、文件角度的路徑判斷）。
/// 工具都是在專案根目錄執行的；找不到專案就是全部預設。
pub fn cached() -> &'static Layout {
    static L: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();
    L.get_or_init(current)
}

/// 走訪 repo 時要略過的 worktree 目錄名：原本寫死的 `.worktrees` 一律略過，另外設定的名字也略過。
pub fn is_worktrees_name(name: &str) -> bool {
    name == DEFAULT_WORKTREES_DIR || name == cached().worktrees_dir
}

/// 範本裡由這裡供值的佔位符（鍵, 值）。`workflow.config` 的同名值只是 init 的輸入，產生腳本時不看它。
pub fn placeholders(l: &Layout) -> Vec<(&'static str, String)> {
    vec![("BASE_BRANCH", l.base_branch.clone()), ("BASE_REMOTE", l.base_remote.clone()), ("SPECS_DIR", l.specs_dir.clone()),
        ("WORKTREES_DIR", l.worktrees_dir.clone()), ("SPEC_TEMPLATES_DIR", l.spec_templates_dir.clone())]
}

/// 目錄規則的說明（錯誤訊息用）。
pub const DIR_RULE: &str = "相對於目標 repo 的路徑，最多 4 層，只收英數與 . _ -，不能是絕對路徑、含 ..，也不能在 .claude、.github、.git 底下";

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
    let mut dir = |key: &'static str, ok: fn(&str) -> bool, rule: &str, default: &str| {
        match guard.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.trim().is_empty() || ok(&dir_norm(s)) => {}
            Some(v) => out.push((key, format!("{key} 不合規則（{rule}；收到 {}）：工具、範本與 hook 改用預設值 {default}", crate::pyfmt::repr(v)))),
        }
    };
    dir("specs_dir", dir_ok, DIR_RULE, DEFAULT_SPECS_DIR);
    dir("worktrees_dir", worktrees_ok, "目標 repo 底下的一層目錄名，只收英數與 . _ -，不能是 .claude、.github、.git", DEFAULT_WORKTREES_DIR);
    dir("spec_templates_dir", dir_ok, DIR_RULE, "<specs_dir>/_templates");
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
    fn dirs_default_when_missing_or_invalid() {
        let d = Layout::default();
        assert_eq!((d.specs_dir.as_str(), d.worktrees_dir.as_str(), d.spec_templates_dir.as_str()), ("specs", ".worktrees", "specs/_templates"));
        assert!(d.default_dirs());
        let l = Layout::of(&json!({"specs_dir": "docs/specs/", "worktrees_dir": ".wt"}));
        assert_eq!((l.specs_dir.as_str(), l.worktrees_dir.as_str(), l.spec_templates_dir.as_str()), ("docs/specs", ".wt", "docs/specs/_templates"));
        assert!(!l.default_dirs());
        assert_eq!(Layout::of(&json!({"specs_dir": "docs/specs", "spec_templates_dir": "docs/tpl"})).spec_templates_dir, "docs/tpl");
        // 範本目錄不合規則：跟著 spec 目錄
        assert_eq!(Layout::of(&json!({"specs_dir": "docs/specs", "spec_templates_dir": "../tpl"})).spec_templates_dir, "docs/specs/_templates");
        for bad in ["", "/abs", "/", "..", "a/../b", "./a", "a/./b", ".claude", ".Claude/specs", ".github/specs", ".git", ".GIT/x", "a/.git/b", "a//b", "a b", "a;b", "a$b",
            "a`b", "{{SPECS_DIR}}", "-x", "a/-x", "a\\b", "C:/x", "~/specs", "a/b/c/d/e", "a.", "specs/*", "a\nb"] {
            let l = Layout::of(&json!({"specs_dir": bad, "worktrees_dir": bad, "spec_templates_dir": bad}));
            assert_eq!(l, d, "{bad:?}");
            assert!(bad.trim().is_empty() || problems(&json!({"specs_dir": bad})).len() == 1, "{bad:?}");
        }
        for bad in [json!(3), json!(["specs"]), json!({"a": 1}), json!(true)] {
            assert_eq!(Layout::of(&json!({"specs_dir": bad, "worktrees_dir": bad, "spec_templates_dir": bad})), d);
        }
        // worktree 目錄只能一層
        assert_eq!(Layout::of(&json!({"worktrees_dir": "tmp/wt"})).worktrees_dir, ".worktrees");
        assert!(dir_ok("docs/specs") && dir_ok("_specs") && dir_ok(".specs") && dir_ok("a/b/c/d") && worktrees_ok(".wt") && !worktrees_ok("a/b"));
        assert_eq!(problems(&json!({"specs_dir": "/abs", "worktrees_dir": "a/b", "spec_templates_dir": ".git/x"})).iter().map(|x| x.0).collect::<Vec<_>>(),
            vec!["specs_dir", "worktrees_dir", "spec_templates_dir"]);
    }

    #[test]
    fn problems_list_bad_values() {
        assert!(problems(&json!({"base_branch": "develop", "base_remote": ""})).is_empty());
        let p = problems(&json!({"base_branch": "a b", "base_remote": 3}));
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), vec!["base_branch", "base_remote"]);
    }
}
