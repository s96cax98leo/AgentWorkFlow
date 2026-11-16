//! 模版在哪：全部子指令共用的一套解析。
//!
//! 「模版根目錄」就是 plugin 目錄，底下有：
//!
//! | 路徑 | 內容 |
//! |---|---|
//! | `scaffold/` | `awf init` 複製進專案的設定（`.claude/teams/` 的 rules、schemas、methodologies、golden-set…） |
//! | `workflow-templates/` | workflow 範本，以及 `snippets/`（共用片段）、`scenarios/`、`check_workflows.mjs`（乾跑） |
//! | `workflow.config.example` | 佔位符與預設值 |
//! | `.claude-plugin/plugin.json` | 版本 |
//! | `bin/` | 啟動器與各平台的執行檔 |
//!
//! 從 marketplace 安裝的 plugin 只有這個目錄，所以所有找模版的地方都以它為準；這個 repo 的 plugin 在 `plugin/`。
//!
//! 解析順序（[`find`]）：
//! 1. 環境變數 `AWF_TEMPLATE_DIR`：給 plugin 目錄或 repo 根（底下有 `plugin/`）都可以（[`normalize`]）。
//! 2. `AWF_PLUGIN_DIR`、`CLAUDE_PLUGIN_ROOT`：本身就是模版根目錄。
//! 3. 從執行檔往上找：執行檔在 `<plugin>/bin/` 底下時用那個 plugin；開發時（`target/debug/awf`）往上找到 repo 根再接 `plugin/`。
//!
//! 明確指定的路徑（`awf init --source`）一樣交給 [`normalize`]。

use std::path::{Path, PathBuf};

/// 指定模版位置的環境變數。
pub const ENV_TEMPLATE: &str = "AWF_TEMPLATE_DIR";
/// plugin 目錄的環境變數（依序）。
pub const ENV_PLUGIN: [&str; 2] = ["AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT"];
/// workflow 範本的目錄名。
pub const WORKFLOW_TEMPLATES: &str = "workflow-templates";

/// `d` 是不是模版根目錄（plugin 目錄）：底下有 `scaffold/`，加上 `workflow-templates/` 或 `.claude-plugin/plugin.json` 其中一個
/// （缺一個的殘缺安裝也認得出來，之後由用到的地方各自報缺什麼）。
pub fn is_root(d: &Path) -> bool {
    d.join("scaffold").is_dir() && (d.join(WORKFLOW_TEMPLATES).is_dir() || plugin_json(d).is_file())
}

/// 把人給的路徑變成模版根目錄：給的是 plugin 目錄就用它；給的是 repo 根（底下的 `plugin/` 是模版根目錄）就接上 `plugin/`。
/// 兩種都不是時原樣回傳（呼叫端檢查 `scaffold/` 存不存在並報錯）。
pub fn normalize(p: &Path) -> PathBuf {
    if !is_root(p) && is_root(&p.join("plugin")) {
        return p.join("plugin");
    }
    p.to_path_buf()
}

/// 從 `start` 往上找模版根目錄：某一層自己是，或它底下的 `plugin/` 是。
pub fn find_up(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|d| {
        if is_root(d) {
            Some(d.to_path_buf())
        } else {
            Some(d.join("plugin")).filter(|p| is_root(p))
        }
    })
}

/// 同 [`find`]，環境變數與執行檔位置由呼叫端給（測試用）。
pub fn find_with(var: &dyn Fn(&str) -> Option<String>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = var(ENV_TEMPLATE) {
        return Some(normalize(Path::new(&p)));
    }
    if let Some(p) = ENV_PLUGIN.iter().filter_map(|k| var(k)).map(PathBuf::from).find(|d| is_root(d)) {
        return Some(p);
    }
    exe.and_then(Path::parent).and_then(find_up)
}

/// 模版根目錄（見模組說明的解析順序）；找不到回 None。
pub fn find() -> Option<PathBuf> {
    let var = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let exe = std::env::current_exe().ok().map(|p| p.canonicalize().unwrap_or(p));
    find_with(&var, exe.as_deref())
}

/// 同 [`find_installed`]，環境變數與執行檔位置由呼叫端給（測試用）。
pub fn find_installed_with(var: &dyn Fn(&str) -> Option<String>, exe: Option<&Path>) -> Option<PathBuf> {
    let from_exe = exe.and_then(Path::parent).and_then(Path::parent).map(Path::to_path_buf);
    ENV_PLUGIN.iter().filter_map(|k| var(k)).map(PathBuf::from).chain(from_exe).find(|d| plugin_json(d).is_file())
}

/// 「裝好的 plugin」在哪（SessionStart 的版本提示用）：只認有版本檔（`.claude-plugin/plugin.json`）的目錄，
/// 依序看 [`ENV_PLUGIN`] 的環境變數、執行檔所在目錄的上一層（`<plugin>/bin/awf`）。
/// 比 [`find`] 窄：不看 `AWF_TEMPLATE_DIR`、不從執行檔一路往上找，所以開發時的 `target/debug/awf` 不會被當成裝好的 plugin
/// （提示會進使用者的對話，寧可不提示）。
pub fn find_installed() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()).filter(|s| !s.is_empty());
    find_installed_with(&var, std::env::current_exe().ok().as_deref())
}

/// [`find`] 的文字版（路徑分隔一律 `/`），給用字串路徑的模組。
pub fn find_str() -> Option<String> {
    find().and_then(|p| p.to_str().map(|s| s.replace('\\', "/")))
}

/// `awf init` 複製進專案的設定。
pub fn scaffold(root: &Path) -> PathBuf {
    root.join("scaffold")
}

/// 模版的 `.claude/teams`（schemas、methodologies、golden-set、deliverables 型錄）。
pub fn teams(root: &Path) -> PathBuf {
    scaffold(root).join(".claude").join("teams")
}

/// workflow 範本的目錄。
pub fn workflow_templates(root: &Path) -> PathBuf {
    root.join(WORKFLOW_TEMPLATES)
}

/// 共用片段（`/*@snippet:<名>*/` 內嵌的來源）。
pub fn snippets(root: &Path) -> PathBuf {
    workflow_templates(root).join("snippets")
}

/// 乾跑的情境檔。
pub fn scenarios(root: &Path) -> PathBuf {
    workflow_templates(root).join("scenarios")
}

/// 乾跑 workflow 的 Node 腳本。
pub fn check_workflows(root: &Path) -> PathBuf {
    workflow_templates(root).join("check_workflows.mjs")
}

/// 佔位符與預設值。
pub fn config_example(root: &Path) -> PathBuf {
    root.join("workflow.config.example")
}

/// plugin 的版本檔。
pub fn plugin_json(root: &Path) -> PathBuf {
    root.join(".claude-plugin").join("plugin.json")
}

/// 啟動器、發佈用的執行檔與 `checksums.json`。
pub fn bin(root: &Path) -> PathBuf {
    root.join("bin")
}

/// 文字路徑版的 [`teams`]（`/` 分隔）。
pub fn teams_str(root: &str) -> String {
    format!("{}/scaffold/.claude/teams", root.trim_end_matches('/'))
}

/// 文字路徑版的 [`snippets`]（`/` 分隔）。
pub fn snippets_str(root: &str) -> String {
    format!("{}/{WORKFLOW_TEMPLATES}/snippets", root.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("awf-template-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("暫存目錄");
        d.canonicalize().expect("暫存目錄")
    }

    fn mk_plugin(d: &Path) {
        std::fs::create_dir_all(d.join("scaffold")).expect("mkdir");
        std::fs::create_dir_all(d.join(WORKFLOW_TEMPLATES)).expect("mkdir");
        std::fs::create_dir_all(d.join("bin")).expect("mkdir");
    }

    #[test]
    fn normalize_accepts_plugin_dir_or_repo_root() {
        let t = tmp("norm");
        let (repo, bare) = (t.join("repo"), t.join("bare"));
        mk_plugin(&repo.join("plugin"));
        std::fs::create_dir_all(&bare).expect("mkdir");
        assert_eq!(normalize(&repo), repo.join("plugin"), "repo 根 → 接上 plugin/");
        assert_eq!(normalize(&repo.join("plugin")), repo.join("plugin"), "plugin 目錄原樣");
        assert_eq!(normalize(&bare), bare, "都不是：原樣（由呼叫端報錯）");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn find_order_and_exe_fallback() {
        let t = tmp("find");
        let (repo, installed, other) = (t.join("repo"), t.join("cache/agent-workflow/1.0.0"), t.join("other"));
        mk_plugin(&repo.join("plugin"));
        mk_plugin(&installed);
        mk_plugin(&other);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let none = |_: &str| None;

        // 執行檔在 <plugin>/bin/ 底下：用那個 plugin（只有 plugin 目錄的安裝）
        assert_eq!(find_with(&none, Some(&installed.join("bin/awf-darwin-arm64"))), Some(installed.clone()));
        // 開發時：target/debug/awf → repo 根 → plugin/
        assert_eq!(find_with(&none, Some(&repo.join("target/debug/awf"))), Some(repo.join("plugin")));
        assert_eq!(find_with(&none, Some(&repo.join("target/debug/deps/x-123"))), Some(repo.join("plugin")));
        // 找不到
        assert_eq!(find_with(&none, Some(&t.join("elsewhere/awf"))), None);
        assert_eq!(find_with(&none, None), None);

        // plugin 的環境變數本身就是模版根目錄，AWF_PLUGIN_DIR 先；不是模版根目錄的略過
        let both = |k: &str| match k {
            "AWF_PLUGIN_DIR" => Some(s(&other)),
            "CLAUDE_PLUGIN_ROOT" => Some(s(&installed)),
            _ => None,
        };
        assert_eq!(find_with(&both, Some(&repo.join("target/debug/awf"))), Some(other.clone()));
        let bad_first = |k: &str| match k {
            "AWF_PLUGIN_DIR" => Some(s(&t.join("nope"))),
            "CLAUDE_PLUGIN_ROOT" => Some(s(&installed)),
            _ => None,
        };
        assert_eq!(find_with(&bad_first, None), Some(installed.clone()));
        // 舊的寫法（上一層才是模版）不再成立：CLAUDE_PLUGIN_ROOT 指到 repo 的 plugin/ 就是它自己
        let root_var = |k: &str| (k == "CLAUDE_PLUGIN_ROOT").then(|| s(&repo.join("plugin")));
        assert_eq!(find_with(&root_var, None), Some(repo.join("plugin")));

        // AWF_TEMPLATE_DIR 最優先，plugin 目錄或 repo 根都接受；指到不存在的地方時原樣回傳
        let other_s = s(&other);
        let tpl = |v: PathBuf| {
            let o = other_s.clone();
            move |k: &str| match k {
                "AWF_TEMPLATE_DIR" => Some(v.to_string_lossy().into_owned()),
                "CLAUDE_PLUGIN_ROOT" => Some(o.clone()),
                _ => None,
            }
        };
        assert_eq!(find_with(&tpl(repo.clone()), None), Some(repo.join("plugin")));
        assert_eq!(find_with(&tpl(installed.clone()), None), Some(installed.clone()));
        assert_eq!(find_with(&tpl(t.join("nope")), None), Some(t.join("nope")));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn installed_needs_manifest() {
        let t = tmp("inst");
        let (a, b, bare) = (t.join("a"), t.join("b"), t.join("bare"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d.join(".claude-plugin")).expect("mkdir");
            std::fs::write(plugin_json(d), "{}").expect("write");
        }
        std::fs::create_dir_all(&bare).expect("mkdir");
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let none = |_: &str| None;
        // 執行檔在 <plugin>/bin/ 底下；再往上的不算（開發時的 target/debug/awf 不是裝好的 plugin）
        assert_eq!(find_installed_with(&none, Some(&a.join("bin/awf"))), Some(a.clone()));
        assert_eq!(find_installed_with(&none, Some(&a.join("target/debug/awf"))), None);
        assert_eq!(find_installed_with(&none, None), None);
        // 環境變數先於執行檔位置，AWF_PLUGIN_DIR 先；沒有版本檔的略過
        let both = |k: &str| match k {
            "AWF_PLUGIN_DIR" => Some(s(&b)),
            "CLAUDE_PLUGIN_ROOT" => Some(s(&a)),
            _ => None,
        };
        assert_eq!(find_installed_with(&both, Some(&a.join("bin/awf"))), Some(b.clone()));
        let bad_first = |k: &str| match k {
            "AWF_PLUGIN_DIR" => Some(s(&bare)),
            "CLAUDE_PLUGIN_ROOT" => Some(s(&a)),
            _ => None,
        };
        assert_eq!(find_installed_with(&bad_first, None), Some(a.clone()));
        let only_bare = |k: &str| (k == "CLAUDE_PLUGIN_ROOT").then(|| s(&bare));
        assert_eq!(find_installed_with(&only_bare, None), None);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn layout_paths() {
        let r = Path::new("/p");
        assert_eq!(teams(r), Path::new("/p/scaffold/.claude/teams"));
        assert_eq!(snippets(r), Path::new("/p/workflow-templates/snippets"));
        assert_eq!(scenarios(r), Path::new("/p/workflow-templates/scenarios"));
        assert_eq!(check_workflows(r), Path::new("/p/workflow-templates/check_workflows.mjs"));
        assert_eq!(config_example(r), Path::new("/p/workflow.config.example"));
        assert_eq!(plugin_json(r), Path::new("/p/.claude-plugin/plugin.json"));
        assert_eq!(teams_str("/p/"), "/p/scaffold/.claude/teams");
        assert_eq!(snippets_str("/p"), "/p/workflow-templates/snippets");
    }
}
