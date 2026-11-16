//! init 訪談的偵測：從目標專案看得出來的現況，當每個主題的預設值並說明依據。
//!
//! 原則：
//! - **只讀**：不寫任何檔、不執行專案的任何程式（不跑 npm、make、測試）。有 `.git` 時只呼叫 git 的三個唯讀查詢（最近的 commit 標題、
//!   分支名、遠端的預設分支）；沒有 git 或失敗就略過那幾項。
//! - **不讀機敏檔**：只讀固定名稱的幾個設定檔（`package.json`、`Makefile`、`justfile`、`pyproject.toml`），其餘只看「有沒有這個檔或目錄」。
//!   不讀 `.env`、金鑰、`.git/config`（遠端網址可能帶憑證）。符號連結不跟。
//! - 偵測只是建議：互動問答把它當預設值給人確認；`--defaults` 與 `--answers` 不採用，只列在對照表的「偵測到」欄。
//! - 任何一項出錯都當成沒偵測到，不影響 init。
//!
//! `awf init --detect --target <專案> [--json]` 只印這裡的結果。給 `/agent-workflow:init-workflow` 與互動問答共用。

use serde_json::{json, Map, Value};
use std::path::Path;
use std::process::{Command, Stdio};

pub const SCHEMA_VERSION: u64 = 1;
/// 設定檔最多讀這麼多位元組（再大就不是手寫的設定檔了）。
const MAX_READ: u64 = 512 * 1024;
const MAX_LIST: usize = 50;
/// 找旁邊的 repo 時最多看幾個目錄項、最多列幾個候選。
const MAX_SIBLINGS_SCAN: usize = 500;
const MAX_REPO_CANDIDATES: usize = 10;
/// guard.config.json 範本的預設（要和 `guard.config.json.tmpl` 一致；訪談與偵測拿它當基準）。
pub const GUARD_PROTECTED: &[&str] = &["main", "master"];
pub const GUARD_ACCEPTANCE: &[&str] = &["tests/acceptance", "e2e"];

/// 偵測到的一件事：鍵、值、依據（從哪裡看出來的）。
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub key: String,
    pub value: Value,
    pub basis: String,
}

/// 一個訪談主題。
#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub id: &'static str,
    pub title: &'static str,
    /// 這個主題要跟使用者確認什麼。
    pub ask: &'static str,
    /// 這個主題對應的答案檔鍵。
    pub answer_keys: &'static [&'static str],
    pub detected: Vec<Finding>,
    /// 建議的答案（答案檔的鍵 → 值）：互動問答當預設值。
    pub suggest: Map<String, Value>,
}

/// 訪談的主題（順序就是問的順序）：（id, 標題, 要確認什麼, 答案檔的鍵）。
pub const TOPICS: &[(&str, &str, &str, &[&str])] = &[
    ("methodology", "開發規範與各段做法", "用哪個 profile 當基礎（default、lightweight 或自己的 JSON）；build 段要不要測試先行", &["methodology", "test_first", "overrides"]),
    ("conventions", "慣例", "code style 的檢查指令、commit 格式與檢查 regex、分支前綴、落後主線時怎麼跟上、PR 大小上限、文件格式", &["code_style", "commit", "commit_regex", "branch_prefix", "sync_strategy", "pr_size", "doc_format"]),
    ("requirements", "需求來源", "需求從哪來（spec 檔、需求 JSON、指令、規格層 openspec）、id 怎麼編；merge gate 也用它", &["requirements"]),
    ("constraints", "限制文件", "有沒有要 agent 遵守的文件（架構、設計、審查規範…），各套用到哪些段", &["constraints"]),
    ("models", "模型分級", "models.mode：inherit（全部沿用主對話模型）、auto（依工作輕重分級，審查與合併有下限）、fixed（只用 profile 的預設等級）", &["models"]),
    ("read_limits", "讀取預算", "要不要限制模版 agent 的讀取量（預設不啟用；warn 只提醒、enforce 擋）與門檻", &["read_limits"]),
    ("parallelism", "平行與拆分", "超過幾個子任務拆子計劃、最多幾個子計劃、要不要分片驗證、每波做完要不要停", &["parallelism"]),
    ("scrub", "去客戶化掃描", "要不要掃（預設不啟用）、規則檔放哪、什麼時候掃（when：publish）", &["scrub"]),
    ("knowledge", "知識庫與知識圖", "知識庫來源；要不要建知識圖、流程要不要自動更新與帶位置（knowledge.auto.*）、跨 repo（knowledge.repos）", &["knowledge"]),
    ("links", "連結", "要不要記連結帳本（驗收條件 ↔ spec 小節、檔案、測試、commit）；links.enforce：不齊時要不要擋政策核准", &["links"]),
    ("deliverables", "交付文件", "要不要產交付文件（預設不啟用）；細項用 /agent-workflow:deliverables 設定", &["deliverables"]),
    ("ci", "專案的測試／lint CI", "要不要由 awf ci render 產生 CI 檔（預設不啟用）；跑哪些指令、哪些必過", &["ci"]),
    ("build_loop", "build 修正迴圈", "修正迴圈要不要只跑受影響的測試（預設關；test 段的閘門照舊全跑）", &["affected_tests_only"]),
    ("auto_merge", "自動合併", "要不要啟用（預設關；啟用前要完成信任模型的檢查清單）", &["auto_merge"]),
    ("main_session", "主 session 的習慣", "會不會在主 session 直接 push 主線、合併 PR（會＝放寬主 session 的 git／GitHub 限制）", &["main_session_push_merge"]),
    ("hooks", "hook 註冊方式", "plugin（由 plugin 註冊）或 project（註冊在專案的 settings.json；Windows 建議）", &["hooks"]),
    ("guard", "守門設定", "受保護分支、驗收測試目錄、文件目錄等 guard.config.json 的值（沒回答就用範本的預設）", &["guard"]),
];

fn exists(root: &Path, rel: &str) -> bool {
    root.join(rel).symlink_metadata().is_ok()
}

fn is_dir(root: &Path, rel: &str) -> bool {
    root.join(rel).symlink_metadata().is_ok_and(|m| m.is_dir())
}

/// 讀一個固定名稱的小設定檔（一般檔、不是符號連結、不超過上限、UTF-8）；其他情況回 None。
fn read_small(root: &Path, rel: &str) -> Option<String> {
    let p = root.join(rel);
    let meta = p.symlink_metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_READ {
        return None;
    }
    String::from_utf8(std::fs::read(&p).ok()?).ok()
}

/// 目錄底下的檔名（不遞迴、排序、最多 `MAX_LIST` 個）。
fn names_in(root: &Path, rel: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join(rel)).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
    out.sort();
    out.truncate(MAX_LIST);
    out
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    if !exists(root, ".git") {
        return None;
    }
    let out = Command::new("git")
        .arg("-C").arg(root)
        // 偵測的對象是還沒看過的 repo：它自己的設定不能讓 git 去跑別的程式（fsmonitor、pager、簽章檢查）
        .args(["-c", "core.fsmonitor=false", "-c", "core.pager=cat", "-c", "log.showSignature=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// 偵測到的測試與 lint 指令：（種類 test／lint／format, 指令, 依據）。只看設定檔的內容，不執行。
pub fn commands(root: &Path) -> Vec<(&'static str, String, String)> {
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    if let Some(doc) = read_small(root, "package.json").and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        let pm = if exists(root, "pnpm-lock.yaml") {
            "pnpm"
        } else if exists(root, "yarn.lock") {
            "yarn"
        } else if exists(root, "bun.lockb") || exists(root, "bun.lock") {
            "bun"
        } else {
            "npm"
        };
        let scripts = doc.get("scripts").and_then(Value::as_object);
        let has = |name: &str| scripts.and_then(|s| s.get(name)).and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty() && !v.contains("no test specified"));
        if has("test") {
            out.push(("test", if pm == "npm" { "npm test".to_string() } else { format!("{pm} run test") }, "package.json 的 scripts.test".to_string()));
        }
        for (kind, name) in [("lint", "lint"), ("lint", "typecheck"), ("format", "format:check"), ("format", "fmt:check")] {
            if has(name) {
                out.push((kind, format!("{pm} run {name}"), format!("package.json 的 scripts.{name}")));
            }
        }
    }
    if exists(root, "Cargo.toml") {
        out.push(("test", "cargo test".to_string(), "Cargo.toml".to_string()));
        out.push(("lint", "cargo clippy --all-targets -- -D warnings".to_string(), "Cargo.toml（Rust 慣用的 lint）".to_string()));
        out.push(("format", "cargo fmt --check".to_string(), "Cargo.toml（Rust 慣用的格式檢查）".to_string()));
    }
    if exists(root, "go.mod") {
        out.push(("test", "go test ./...".to_string(), "go.mod".to_string()));
        if exists(root, ".golangci.yml") || exists(root, ".golangci.yaml") {
            out.push(("lint", "golangci-lint run".to_string(), ".golangci.yml".to_string()));
        } else {
            out.push(("lint", "go vet ./...".to_string(), "go.mod（Go 內建的檢查）".to_string()));
        }
    }
    let pyproject = read_small(root, "pyproject.toml").unwrap_or_default();
    if pyproject.contains("[tool.pytest") || exists(root, "pytest.ini") {
        out.push(("test", "pytest".to_string(), if exists(root, "pytest.ini") { "pytest.ini".to_string() } else { "pyproject.toml 的 [tool.pytest…]".to_string() }));
    }
    if pyproject.contains("[tool.ruff") || exists(root, "ruff.toml") || exists(root, ".ruff.toml") {
        out.push(("lint", "ruff check .".to_string(), "ruff 的設定".to_string()));
        out.push(("format", "ruff format --check .".to_string(), "ruff 的設定".to_string()));
    } else if pyproject.contains("[tool.black") {
        out.push(("format", "black --check .".to_string(), "pyproject.toml 的 [tool.black]".to_string()));
    }
    for (file, runner) in [("Makefile", "make"), ("justfile", "just")] {
        let Some(text) = read_small(root, file) else { continue };
        for (kind, target) in [("test", "test"), ("lint", "lint"), ("lint", "check"), ("lint", "verify"), ("format", "fmt-check")] {
            let hit = text.lines().any(|l| l.strip_prefix(target).is_some_and(|r| r.starts_with(':') && !r.starts_with(":=")));
            if hit && !out.iter().any(|(k, _, _)| *k == kind) {
                out.push((kind, format!("{runner} {target}"), format!("{file} 的 {target}")));
            }
        }
    }
    out
}

fn languages(root: &Path) -> Vec<(&'static str, &'static str)> {
    let marks: &[(&str, &str)] = &[
        ("tsconfig.json", "TypeScript"), ("package.json", "JavaScript"), ("Cargo.toml", "Rust"), ("pyproject.toml", "Python"), ("setup.py", "Python"),
        ("requirements.txt", "Python"), ("go.mod", "Go"), ("pom.xml", "Java"), ("build.gradle", "Java／Kotlin"), ("build.gradle.kts", "Kotlin"), ("Gemfile", "Ruby"),
        ("composer.json", "PHP"), ("Package.swift", "Swift"), ("mix.exs", "Elixir"),
    ];
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    for (file, lang) in marks {
        if exists(root, file) && !out.iter().any(|(l, _)| l == lang) {
            out.push((lang, file));
        }
    }
    out
}

const CONVENTIONAL: &[&str] = &["feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert", "release", "merge"];

/// commit 標題是不是「type: 摘要」或「type(scope): 摘要」。
fn conventional_type(subject: &str) -> Option<&str> {
    let (head, rest) = subject.split_once(':')?;
    if !rest.starts_with(' ') {
        return None;
    }
    let head = head.trim_end_matches('!');
    let ty = head.split('(').next().unwrap_or(head);
    (CONVENTIONAL.contains(&ty) && (head == ty || head.ends_with(')'))).then_some(ty)
}

/// 專案旁邊（同一層目錄）的 git repo，寫成 `../<名>`；知識圖「別的 repo」的候選。只看有沒有 `.git`，不讀內容。
fn sibling_repos(root: &Path) -> Vec<String> {
    let Some(parent) = root.parent() else { return Vec::new() };
    let me = root.file_name();
    let mut out: Vec<String> = std::fs::read_dir(parent).into_iter().flatten().flatten().take(MAX_SIBLINGS_SCAN)
        .filter(|e| Some(e.file_name().as_os_str()) != me && e.path().join(".git").symlink_metadata().is_ok())
        .filter_map(|e| e.file_name().to_str().map(|n| format!("../{n}")))
        .collect();
    out.sort();
    out.truncate(MAX_REPO_CANDIDATES);
    out
}

/// 已經套用過的專案：需求來源（methodology.json 或 guard.config.json 的 `requirements.path`）指到工作區外時，
/// 回它所在的 repo（往上找最近一個有 `.git` 的目錄；找不到就用那個路徑所在的目錄）。寫法照設定檔裡的寫法（相對或絕對）。
fn requirements_outside(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for file in [".claude/teams/methodology.json", ".claude/teams/guard.config.json"] {
        let Some(doc) = read_small(root, file).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let Some(p) = doc.pointer("/requirements/path").and_then(Value::as_str) else { continue };
        let norm = p.replace('\\', "/");
        let abs = Path::new(p).is_absolute() || norm.starts_with('/');
        let mut parts: Vec<&str> = norm.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
        if !(abs || parts.contains(&"..")) {
            continue;
        }
        let join = |parts: &[&str]| if abs { format!("/{}", parts.join("/")) } else { parts.join("/") };
        let is_repo = |parts: &[&str]| root.join(join(parts)).join(".git").symlink_metadata().is_ok();
        let written = parts.clone();
        let mut hit: Option<String> = None;
        for _ in 0..6 {
            if parts.is_empty() || parts.last() == Some(&"..") {
                break;
            }
            if is_repo(&parts) {
                hit = Some(join(&parts));
                break;
            }
            parts.pop();
        }
        let dir = hit.or_else(|| {
            // 沒有找到 repo：檔案就用它所在的目錄
            let mut w = written.clone();
            if root.join(join(&w)).is_file() {
                w.pop();
            }
            (!w.is_empty() && w.last() != Some(&"..")).then(|| join(&w))
        });
        if let Some(d) = dir.filter(|d| !out.contains(d)) {
            out.push(d);
        }
    }
    out
}

/// guard.config.json 的 `linked_repos`（工作區以外、dev-* 可以改的我方 repo）。
fn linked_repos(root: &Path) -> Vec<String> {
    read_small(root, ".claude/teams/guard.config.json").and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|d| d.get("linked_repos").and_then(Value::as_array).cloned()).unwrap_or_default()
        .iter().filter_map(Value::as_str).map(str::to_string).collect()
}

fn finding(key: &str, value: Value, basis: impl Into<String>) -> Finding {
    Finding { key: key.to_string(), value, basis: basis.into() }
}

fn topic(id: &str) -> Topic {
    let (id, title, ask, answer_keys) = TOPICS.iter().find(|t| t.0 == id).copied().unwrap_or(("", "", "", &[]));
    Topic { id, title, ask, answer_keys, detected: Vec::new(), suggest: Map::new() }
}

/// 偵測目標專案。`windows`＝這台機器是不是 Windows（影響 hook 註冊方式的建議）。
pub fn detect(root: &Path, windows: bool) -> Vec<Topic> {
    let mut out: Vec<Topic> = TOPICS.iter().map(|t| topic(t.0)).collect();
    let mut at = |id: &str, f: &mut dyn FnMut(&mut Topic)| {
        if let Some(t) = out.iter_mut().find(|t| t.id == id) {
            f(t);
        }
    };
    let cmds = commands(root);
    let first = |kind: &str| cmds.iter().find(|(k, _, _)| *k == kind);

    at("methodology", &mut |t| {
        let langs = languages(root);
        if !langs.is_empty() {
            t.detected.push(finding("languages", json!(langs.iter().map(|(l, _)| *l).collect::<Vec<_>>()), langs.iter().map(|(_, f)| *f).collect::<Vec<_>>().join("、")));
        }
        let tests: Vec<&str> = ["tests", "test", "__tests__", "spec", "e2e"].into_iter().filter(|d| is_dir(root, d)).collect();
        if !tests.is_empty() {
            t.detected.push(finding("test_dirs", json!(tests), "專案根目錄有這些目錄"));
        }
        for (k, c, basis) in cmds.iter().filter(|(k, _, _)| *k == "test") {
            t.detected.push(finding(&format!("{k}_command"), json!(c), basis.clone()));
        }
        let docs: Vec<&str> = ["CLAUDE.md", "CONTRIBUTING.md", "AGENTS.md"].into_iter().filter(|f| exists(root, f)).collect();
        if !docs.is_empty() {
            t.detected.push(finding("guides", json!(docs), "既有的開發說明（訪談時可以先讀它們整理成建議值）"));
        }
    });

    let subjects: Vec<String> = git(root, &["log", "-n", "50", "--no-merges", "--format=%s"]).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
    let branches: Vec<String> = git(root, &["for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes"]).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
    let default_branch = git(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).map(|t| t.trim().trim_start_matches("origin/").to_string()).filter(|s| !s.is_empty());

    at("conventions", &mut |t| {
        let lint_cfg: Vec<&str> = [
            ".eslintrc", ".eslintrc.js", ".eslintrc.cjs", ".eslintrc.json", "eslint.config.js", "eslint.config.mjs", ".prettierrc", ".prettierrc.json", "biome.json",
            "ruff.toml", ".ruff.toml", ".flake8", "rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".golangci.yml", ".golangci.yaml", ".editorconfig", ".rubocop.yml",
        ].into_iter().filter(|f| exists(root, f)).collect();
        if !lint_cfg.is_empty() {
            t.detected.push(finding("lint_configs", json!(lint_cfg), "既有的 lint／格式設定檔"));
        }
        if let Some((_, cmd, basis)) = first("lint").or_else(|| first("format")) {
            t.detected.push(finding("code_style_command", json!(cmd), basis.clone()));
            let tool = cmd.split_whitespace().next().unwrap_or_default();
            t.suggest.insert("code_style".into(), json!({"command": cmd, "tool": tool, "description": format!("提交前跑 {cmd}，不得新增違規。")}));
        }
        if subjects.len() >= 5 {
            let hits: Vec<&str> = subjects.iter().filter_map(|s| conventional_type(s)).collect();
            let ratio = hits.len() * 100 / subjects.len();
            if ratio >= 60 {
                let mut types: Vec<&str> = Vec::new();
                for h in &hits {
                    if !types.contains(h) {
                        types.push(h);
                    }
                }
                types.sort_unstable();
                t.detected.push(finding("commit_format", json!("<type>(<scope>): <摘要>"), format!("最近 {} 個 commit 有 {ratio}% 是這個格式（用到的 type：{}）", subjects.len(), types.join("、"))));
                t.suggest.insert("commit".into(), json!("<type>(<scope>): <摘要>（scope 可省略）"));
                t.suggest.insert("commit_regex".into(), json!(format!("^({})(\\([^)]+\\))?!?: .+", types.join("|"))));
            } else {
                t.detected.push(finding("commit_format", Value::Null, format!("最近 {} 個 commit 只有 {ratio}% 是 <type>: <摘要> 的格式，看不出固定格式", subjects.len())));
            }
        }
        let mut counts: Vec<(String, usize)> = Vec::new();
        for b in &branches {
            let b = b.strip_prefix("origin/").unwrap_or(b);
            if let Some((p, _)) = b.split_once('/') {
                let key = format!("{p}/");
                match counts.iter_mut().find(|(k, _)| *k == key) {
                    Some(c) => c.1 += 1,
                    None => counts.push((key, 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if let Some((p, n)) = counts.first().filter(|(_, n)| *n >= 2) {
            t.detected.push(finding("branch_prefix", json!(p), format!("{n} 個分支用這個前綴（共 {} 個分支）", branches.len())));
            t.suggest.insert("branch_prefix".into(), json!(p));
        }
        if let Some(d) = &default_branch {
            t.detected.push(finding("default_branch", json!(d), "遠端 origin 的預設分支（主線分支在守門主題設定：guard 的 base_branch）"));
        }
        for f in [".github/pull_request_template.md", ".github/PULL_REQUEST_TEMPLATE.md", "REVIEW.md"] {
            if exists(root, f) {
                t.detected.push(finding("review_doc", json!(f), "既有的 PR 範本或審查規範"));
            }
        }
        let count = |ext: &str| names_in(root, "docs").iter().filter(|n| n.ends_with(ext)).count();
        let (md, rst, adoc) = (count(".md"), count(".rst"), count(".adoc"));
        if md + rst + adoc > 0 {
            let (name, n) = [("Markdown", md), ("reStructuredText", rst), ("AsciiDoc", adoc)].into_iter().max_by_key(|x| x.1).unwrap_or(("Markdown", 0));
            t.detected.push(finding("doc_format", json!(name), format!("docs/ 底下 {n} 個這種格式的檔")));
            if name != "Markdown" {
                t.suggest.insert("doc_format".into(), json!(name));
            }
        }
    });

    at("requirements", &mut |t| {
        if is_dir(root, "openspec") && (is_dir(root, "openspec/specs") || is_dir(root, "openspec/changes")) {
            t.detected.push(finding("openspec", json!("openspec"), "專案有 openspec/（規格層）"));
            t.suggest.insert("requirements".into(), json!({"source": "openspec", "path": "openspec"}));
        }
        if is_dir(root, "specs") {
            t.detected.push(finding("specs_dir", json!("specs"), "專案有 specs/"));
        }
        if is_dir(root, ".github/ISSUE_TEMPLATE") {
            t.detected.push(finding("issue_templates", json!(names_in(root, ".github/ISSUE_TEMPLATE")), "有 issue 範本：需求可能來自 issue（requirements.source 用 command）"));
        }
    });

    at("constraints", &mut |t| {
        let docs: Vec<&str> = ["ARCHITECTURE.md", "DESIGN.md", "REVIEW.md", "docs/ARCHITECTURE.md", "docs/DESIGN.md", "docs/architecture.md", "docs/design.md"].into_iter().filter(|f| exists(root, f)).collect();
        if !docs.is_empty() {
            t.detected.push(finding("candidates", json!(docs), "看起來像規範的文件（要不要當限制文件、套用到哪些段由你決定；不自動加）"));
        }
    });

    at("scrub", &mut |t| {
        let files: Vec<&str> = [".claude/teams/scrub_terms.txt", "tests/scrub_terms.txt", "scrub_terms.txt"].into_iter().filter(|f| exists(root, f)).collect();
        if !files.is_empty() {
            t.detected.push(finding("terms_files", json!(files), "既有的規則檔（只看有沒有，不讀內容）"));
            t.suggest.insert("scrub".into(), json!({"enabled": false, "terms_files": files}));
        }
    });

    at("knowledge", &mut |t| {
        let docs: Vec<&str> = ["docs", "specs", "openspec", "adr", "docs/adr"].into_iter().filter(|d| is_dir(root, d)).collect();
        if !docs.is_empty() {
            t.detected.push(finding("doc_dirs", json!(docs), "有文件的目錄（知識庫來源的候選）"));
        }
        let tools: Vec<&str> = [(".codegraph", "codegraph"), (".graft", "graft")].into_iter().filter(|(d, _)| is_dir(root, d)).map(|(_, n)| n).collect();
        if !tools.is_empty() {
            t.detected.push(finding("code_index", json!(tools), "專案已經有程式索引"));
        }
        if exists(root, "_tpm/kb/graph.db") {
            t.detected.push(finding("graph", json!("_tpm/kb/graph.db"), "已經有知識圖（knowledge.auto.update 沒寫時，流程只更新已經存在的圖）"));
        }
        // 別的 repo 的候選：已經登記成 linked_repos（dev-* 可以改）的不列，因為收進知識圖的 repo 對 team agent 唯讀
        let linked = linked_repos(root);
        let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/') || root.join(a) == root.join(b);
        let sib: Vec<String> = sibling_repos(root).into_iter().filter(|s| !linked.iter().any(|l| same(l, s))).collect();
        if !sib.is_empty() {
            t.detected.push(finding("sibling_repos", json!(sib), "專案旁邊（同一層目錄）的 git repo：knowledge.repos 的候選（不自動加；收進來的 repo 對 team agent 唯讀）"));
        }
        let req: Vec<String> = requirements_outside(root).into_iter().filter(|s| !sib.contains(s) && !linked.iter().any(|l| same(l, s))).collect();
        if !req.is_empty() {
            t.detected.push(finding("requirements_outside", json!(req), "需求來源（requirements.path）指到工作區外：knowledge.repos 的候選（不自動加）"));
        }
    });

    at("deliverables", &mut |t| {
        if is_dir(root, "docs/deliverables") {
            t.detected.push(finding("dir", json!("docs/deliverables"), "已經有交付文件目錄"));
        }
        if exists(root, ".claude/teams/deliverables.json") {
            t.detected.push(finding("config", json!(".claude/teams/deliverables.json"), "已經有交付設定"));
        }
    });

    at("ci", &mut |t| {
        let files: Vec<String> = names_in(root, ".github/workflows").into_iter().filter(|n| n.ends_with(".yml") || n.ends_with(".yaml")).map(|n| format!(".github/workflows/{n}")).collect();
        let other: Vec<&str> = [".gitlab-ci.yml", ".circleci/config.yml", "Jenkinsfile", "azure-pipelines.yml", "bitbucket-pipelines.yml"].into_iter().filter(|f| exists(root, f)).collect();
        if !files.is_empty() {
            t.detected.push(finding("github_workflows", json!(files), "既有的 GitHub Actions workflow（不會動它們；awf 的檢查放獨立的檔）"));
        }
        if !other.is_empty() {
            t.detected.push(finding("other_ci", json!(other), "別的 CI 平台的設定檔（awf ci 目前只產生 GitHub Actions）"));
        }
        let mut jobs: Vec<Value> = Vec::new();
        for (kind, cmd, basis) in &cmds {
            t.detected.push(finding(&format!("{kind}_command"), json!(cmd), basis.clone()));
            if !jobs.iter().any(|j| j["id"] == json!(kind)) {
                jobs.push(json!({"id": kind, "name": kind, "run": cmd, "required": *kind == "test"}));
            }
        }
        if !jobs.is_empty() {
            t.suggest.insert("ci".into(), json!({"enabled": false, "provider": "github", "jobs": jobs}));
        }
    });

    at("auto_merge", &mut |t| {
        for f in ["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"] {
            if exists(root, f) {
                t.detected.push(finding("codeowners", json!(f), "有 CODEOWNERS（啟用自動合併前的檢查清單會用到）"));
            }
        }
    });

    at("hooks", &mut |t| {
        if windows {
            t.detected.push(finding("platform", json!("windows"), "這台機器是 Windows：建議 hooks 註冊在專案（project）"));
            t.suggest.insert("hooks".into(), json!("project"));
        }
        if exists(root, ".claude/settings.json") {
            t.detected.push(finding("settings", json!(".claude/settings.json"), "專案已經有 settings.json（init 只做深度合併，你的值優先）"));
        }
        if exists(root, ".claude/teams/VERSION") {
            t.detected.push(finding("installed", json!(read_small(root, ".claude/teams/VERSION").unwrap_or_default().trim()), "這個專案已經套用過（改設定用 --reconfigure，換新版用 --upgrade）"));
        }
    });

    at("guard", &mut |t| {
        let mut sug = Map::new();
        if let Some(d) = default_branch.as_deref().filter(|d| !GUARD_PROTECTED.contains(d)) {
            t.detected.push(finding("default_branch", json!(d), "遠端 origin 的預設分支不在範本預設的受保護分支（main、master）裡：建議加進 protected_branches"));
            let mut list: Vec<&str> = GUARD_PROTECTED.to_vec();
            list.push(d);
            sug.insert("protected_branches".to_string(), json!(list));
        }
        if let Some(d) = default_branch.as_deref().filter(|d| *d != crate::layout::DEFAULT_BASE_BRANCH && crate::layout::branch_ok(d)) {
            t.detected.push(finding("base_branch", json!(d), "遠端 origin 的預設分支不是 main：建議 base_branch 設成它（沒帶 base 時的比對基準與 PR 的目標分支）"));
            sug.insert("base_branch".to_string(), json!(d));
        }
        // spec 目錄：沒有 specs/ 而有常見的別名時，建議照既有的目錄
        if !is_dir(root, "specs") {
            if let Some(d) = ["docs/specs", "doc/specs", "spec"].into_iter().find(|d| is_dir(root, d)) {
                t.detected.push(finding("specs_dir", json!(d), "專案沒有 specs/、有這個目錄：建議 specs_dir 設成它（每個需求的 intent、spec、plan 放在 <specs_dir>/<epic>/）"));
                sug.insert("specs_dir".to_string(), json!(d));
            }
        }
        let dirs: Vec<&str> = ["tests/acceptance", "e2e", "tests/e2e", "test/acceptance", "test/e2e", "acceptance"].into_iter().filter(|d| is_dir(root, d)).collect();
        if !dirs.is_empty() {
            t.detected.push(finding("acceptance_dirs", json!(dirs), "看起來像驗收測試的目錄（acceptance_test_dirs 的候選；dev-implementer 不能改裡面的檔）"));
            let extra: Vec<&str> = dirs.iter().copied().filter(|d| !GUARD_ACCEPTANCE.contains(d)).collect();
            if !extra.is_empty() {
                let mut list: Vec<&str> = GUARD_ACCEPTANCE.to_vec();
                list.extend(extra);
                sug.insert("acceptance_test_dirs".to_string(), json!(list));
            }
        }
        if !sug.is_empty() {
            t.suggest.insert("guard".into(), Value::Object(sug));
        }
    });
    out
}

/// 全部主題的建議答案合成一份（答案檔的形狀）。
pub fn suggested(topics: &[Topic]) -> Map<String, Value> {
    let mut out = Map::new();
    for t in topics {
        for (k, v) in &t.suggest {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// `--detect --json` 的輸出。
pub fn to_json(root: &Path, topics: &[Topic]) -> Value {
    let list: Vec<Value> = topics.iter().map(|t| {
        json!({
            "id": t.id,
            "title": t.title,
            "ask": t.ask,
            "answer_keys": t.answer_keys,
            "detected": t.detected.iter().map(|f| json!({"key": f.key, "value": f.value, "basis": f.basis})).collect::<Vec<_>>(),
            "suggest": t.suggest,
        })
    }).collect();
    json!({"schema_version": SCHEMA_VERSION, "target": root.to_string_lossy(), "topics": list, "suggested_answers": suggested(topics)})
}

fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "（看不出來）".to_string(),
        Value::Array(a) if a.iter().all(Value::is_string) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、"),
        other => other.to_string(),
    }
}

/// `--detect` 的文字輸出。
pub fn text_of(root: &Path, topics: &[Topic]) -> String {
    let mut out = format!("偵測 {}（只讀；偵測到的是建議，訪談時逐項確認）\n", root.display());
    for t in topics {
        out.push_str(&format!("\n== {}（{}） ==\n  要確認：{}\n  答案檔的鍵：{}\n", t.title, t.id, t.ask, t.answer_keys.join("、")));
        if t.detected.is_empty() {
            out.push_str("  偵測：沒有看得出來的現況\n");
        }
        for f in &t.detected {
            out.push_str(&format!("  偵測：{} = {}（依據：{}）\n", f.key, compact(&f.value), f.basis));
        }
        for (k, v) in &t.suggest {
            out.push_str(&format!("  建議的答案：{k} = {v}\n"));
        }
    }
    out
}

/// 某個答案鍵偵測到什麼（對照表的「偵測到」欄）；沒有回空字串。
pub fn detected_for(topics: &[Topic], answer_key: &str) -> String {
    topics.iter().find_map(|t| t.suggest.get(answer_key)).map(|v| match v {
        Value::Object(o) => o.get("command").or_else(|| o.get("source")).or_else(|| o.get("terms_files")).or_else(|| o.get("jobs")).map(|x| match x {
            Value::Array(a) if a.iter().all(Value::is_object) => a.iter().filter_map(|j| j.get("run").and_then(Value::as_str)).collect::<Vec<_>>().join("；"),
            other => compact(other),
        }).unwrap_or_else(|| v.to_string()),
        other => compact(other),
    }).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventional_subjects() {
        assert_eq!(conventional_type("feat: 加匯出"), Some("feat"));
        assert_eq!(conventional_type("fix(api)!: 修正"), Some("fix"));
        assert_eq!(conventional_type("Update readme"), None);
        assert_eq!(conventional_type("feat:沒有空白"), None);
        assert_eq!(conventional_type("wip: x"), None);
    }

    #[test]
    fn topics_have_unique_ids_and_keys() {
        let mut ids: Vec<&str> = TOPICS.iter().map(|t| t.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOPICS.len());
        let mut keys: Vec<&str> = TOPICS.iter().flat_map(|t| t.3.iter().copied()).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "答案檔的鍵不能分屬兩個主題");
    }

    #[test]
    fn empty_dir_detects_nothing() {
        // 包一層：偵測會看「同一層目錄」有沒有別的 repo，系統暫存目錄底下可能正好有
        let top = std::env::temp_dir().join(format!("awf-detect-empty-{}", std::process::id()));
        let d = top.join("proj");
        let _ = std::fs::create_dir_all(&d);
        let topics = detect(&d, false);
        assert!(topics.iter().all(|t| t.detected.is_empty() && t.suggest.is_empty()), "{topics:?}");
        assert!(suggested(&topics).is_empty());
        let _ = std::fs::remove_dir_all(&top);
    }

    #[test]
    fn knowledge_repo_candidates_and_guard_suggestions() {
        let top = std::env::temp_dir().join(format!("awf-detect-repos-{}", std::process::id()));
        let d = top.join("proj");
        for sub in ["proj/.claude/teams", "proj/tests/e2e", "acme-specs/.git", "acme-lib/.git", "plain", "shared/reqs/.git"] {
            let _ = std::fs::create_dir_all(top.join(sub));
        }
        let _ = std::fs::write(top.join("shared/reqs/requirements.json"), "[]");
        let _ = std::fs::write(d.join(".claude/teams/methodology.json"), r#"{"requirements": {"source": "json", "path": "../shared/reqs/requirements.json"}}"#);
        let _ = std::fs::write(d.join(".claude/teams/guard.config.json"), r#"{"linked_repos": ["../acme-lib"]}"#);
        let topics = detect(&d, false);
        let get = |topic: &str, key: &str| topics.iter().filter(|t| t.id == topic).flat_map(|t| t.detected.iter()).find(|f| f.key == key).map(|f| f.value.clone());
        // 同一層的 repo 列成候選；登記成 linked_repos 的不列（收進知識圖就變唯讀）；沒有 .git 的不列
        assert_eq!(get("knowledge", "sibling_repos"), Some(json!(["../acme-specs"])));
        assert_eq!(get("knowledge", "requirements_outside"), Some(json!(["../shared/reqs"])));
        assert_eq!(get("guard", "acceptance_dirs"), Some(json!(["tests/e2e"])));
        assert_eq!(suggested(&topics).get("guard"), Some(&json!({"acceptance_test_dirs": ["tests/acceptance", "e2e", "tests/e2e"]})));
        // 候選不會變成建議的答案（不自動加）
        assert!(!suggested(&topics).contains_key("knowledge"));
        let _ = std::fs::remove_dir_all(&top);
    }
}
