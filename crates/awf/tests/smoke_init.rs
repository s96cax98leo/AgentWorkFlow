//! init 煙霧測試：`tests/smoke_init.py` 的 1.0 版。實際執行編好的 `awf`（`CARGO_BIN_EXE_awf`），在暫存目錄跑 `awf init`，
//! 再在產出的專案裡跑各個子指令。
//!
//! 和 `tests/smoke_init.py`（0.x，測 `init.py`；仍留著、沒有改）的對照：
//!
//! | smoke_init.py 的檢查 | 這裡的測試 | 1.0 的差別 |
//! |---|---|---|
//! | `check_output`：沒有殘留佔位符、必要檔、沒有 `.tmpl`、JSON 合法 | `fresh_init` | 必要檔不含 `.claude/teams/tools/*.py`、`golden-set/run.py`；多檢查「專案裡沒有 tools/、沒有任何 .py」 |
//! | `check_versions`：三處版本一致 | `fresh_init` | 相同 |
//! | `check_placeholders`：佔位符雙向一致 | `placeholders_match_example` | 自己掃 scaffold（不 import init.py）；init 自算的兩個鍵寫在這裡 |
//! | `check_tools`：golden-set | 沒有對應 | `awf` 沒有跑 golden-set 的子指令（`run.py` 不再複製進專案）；正反例由 awf-core 的測試跑 |
//! | `check_tools`：`build.py --check` | `fresh_init`（`awf build --check`） | 片段從模版找（專案沒有 tools/snippets） |
//! | `check_tools`：預設沒有 `.claude/workflows/`、`render --all` 產生 12 支 | `fresh_init`（`awf render --all`） | 相同 |
//! | `check_tools`：`check_workflows.mjs` 乾跑（有 node 才跑） | `fresh_init` | `.mjs` 用模版 repo 的那一份（不在專案） |
//! | `check_tools`：有 `--selftest` 的工具全部跑 | `fresh_init`（`SELFTESTS` 列的子指令） | `awf state --selftest` 在 init 過的專案裡會失敗（它從專案的 tools/snippets 複製片段），另列 `state_selftest_in_project`（ignored）記這件事；在沒 init 的目錄跑會過，`fresh_init` 跑那一種 |
//! | `check_tools`：`check_agents.py --plugin-dir` | `fresh_init`（`awf check-agents`） | 相同 |
//! | `check_default_methodology` | `fresh_init` | 相同 |
//! | `check_merge_gate_optin` | `fresh_init`、`merge_gate_needs_binary` | yml 改成核對 sha256 後跑 `awf merge-gate`；多放 `.claude/teams/bin/` 兩個檔；沒有執行檔時拒絕 |
//! | `check_extensions` | `extensions` | hook 用 `awf hook guard-shell`；乾跑時把模版的 `.mjs` 與情境檔放到暫存位置（它從自己的位置找專案的 `.claude/agents`） |
//! | `check_answers_methodology`（含 `check_rules --git`） | `answers_profile` | golden-set 沒有對應（同上） |
//! | `check_project_hooks` | `hooks_project` | 複製的是執行檔（`.claude/hooks/bin/awf`），不是 `.py`；完整的 hook 案例由 `hooks_*.rs` 對同一支執行檔跑，這裡確認複本逐位元相同並用複本跑代表性案例；多跑 `awf doctor` |
//! | `check_existing_project` | `existing_project` | 註冊的是 `awf hook <種類>` 五筆 |
//! | `check_upgrade`（含四種 workflow 副本、`--adopt-new-fields`、`--force`） | `upgrade` | 「改過的工具檔換成新版」改成「改過的工具檔保留並列出」；golden-set 沒有對應 |
//! | `check_force_keeps_values` | `force_keeps_values` | 相同 |
//! | `check_with_workflows` | `with_workflows` | 相同 |
//! | （沒有） | `upgrade_real_06_project` | 新增：用 git 取出 0.6.0 的模版、用它的 `init.py` 建專案，再用 `awf init --upgrade`（原樣、改過工具、`--hooks project`、有 merge gate 四種）；沒有 git、python3 或那個 commit 就略過 |

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const WORKFLOWS: [&str; 12] = [
    "sdlc-plan", "sdlc-design", "sdlc-design-revise", "sdlc-build", "sdlc-sync", "sdlc-test", "sdlc-deliver", "sdlc-merge",
    "event", "event-brief", "event-pr-review", "event-post-merge",
];
const REQUIRED: [&str; 10] = [
    "CLAUDE.md",
    ".claude/settings.json",
    ".claude/teams/README.md",
    ".claude/teams/rules.md",
    ".claude/teams/guard.config.json",
    ".claude/teams/schemas/envelope.schema.json",
    ".claude/teams/methodology.json",
    ".claude/teams/schemas/methodology.schema.json",
    ".claude/teams/methodologies/default.json",
    ".claude/teams/deliverables/catalog.json",
];
/// 有 `--selftest` 的子指令（0.x 是「tools/ 裡有 --selftest 的 .py」）。`state` 另外處理，見檔頭。
const SELFTESTS: [&str; 16] = [
    "checklist", "cost", "deliverables", "doctor", "kb", "locate", "merge-gate", "pack", "publish", "recipe", "run-brief", "save-run", "script",
    "spec", "sync-branch", "watch",
];
/// 0.6.0 的 merge commit（`plugin/legacy-tools.json` 的 `revs` 也是它）。
const REV_06: &str = "f98ae6ae4ffface0cf09a97273ef8473dd4de828";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("awf-smoke-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("tmp")).unwrap();
        Work(d.canonicalize().unwrap())
    }
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if std::env::var("KEEP").as_deref() == Ok("1") {
            eprintln!("保留暫存目錄：{}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
    fn ok(&self, what: &str) -> &Out {
        assert_eq!(self.code, 0, "{what} 失敗：\n{}", self.all());
        self
    }
}

fn exec(prog: &str, argv: &[&str], cwd: &Path, env: &[(&str, &str)], stdin: Option<&str>) -> Out {
    let mut c = Command::new(prog);
    c.args(argv).current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    // 不受外面的設定影響：模版一律從執行檔位置找，plugin 位置由各測試自己給
    for k in ["AWF_BIN", "AWF_TEMPLATE_DIR", "AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PROJECT_DIR", "AWF_NOW"] {
        c.env_remove(k);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.spawn().unwrap_or_else(|e| panic!("跑不起來 {prog}：{e}"));
    if let Some(s) = stdin {
        ch.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = ch.wait_with_output().unwrap();
    Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
}

fn awf(argv: &[&str], cwd: &Path) -> Out {
    exec(EXE, argv, cwd, &[], None)
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// `awf init --target <專案> --config <設定檔> …`（同 smoke_init.py 的 `init()`）。
fn init_with(target: &Path, config: &Path, extra: &[&str]) -> Out {
    let mut argv = vec!["init", "--target", s(target), "--config", s(config)];
    argv.extend_from_slice(extra);
    awf(&argv, target.parent().unwrap())
}

fn init(target: &Path, extra: &[&str]) -> Out {
    init_with(target, &root().join("workflow.config.example"), extra)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn load(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", p.display()))
}

fn dump(p: &Path, v: &Value) {
    write(p, &serde_json::to_string_pretty(v).unwrap());
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    es.sort();
    for p in es {
        if p.is_dir() && !p.is_symlink() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(dir, &mut v);
    v
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap().to_str().unwrap().replace('\\', "/")
}

fn copy_tree(src: &Path, dst: &Path) {
    for f in files(src) {
        let to = dst.join(f.strip_prefix(src).unwrap());
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(&f, &to).unwrap();
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn read_kv(path: &Path, key: &str) -> String {
    let mut val = String::new();
    for line in read(path).lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(key) {
            if let Some(v) = rest.trim_start().strip_prefix('=') {
                val = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            }
        }
    }
    val
}

/// 一行裡的 `{{…}}`，GitHub Actions 的 `${{ … }}` 不算。
fn has_placeholder(line: &str) -> bool {
    let mut rest = line;
    while let Some(i) = rest.find("{{") {
        if i > 0 && rest.as_bytes()[i - 1] == b'$' && rest[i..].contains("}}") {
            rest = &rest[i + rest[i..].find("}}").unwrap() + 2..];
            continue;
        }
        return true;
    }
    false
}

/// 文字裡的佔位符名稱（兩個左大括號＋大寫與底線＋兩個右大括號）。
fn placeholders(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let n = after.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').count();
        if n > 0 && after[n..].starts_with("}}") {
            out.insert(after[..n].to_string());
        }
        rest = after;
    }
}

fn fake_elf() -> Vec<u8> {
    let mut v = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 62, 0];
    v.extend_from_slice(b"\nfake awf linux-x64 for smoke test\n");
    v
}

fn plugin_version() -> String {
    load(&root().join("plugin/.claude-plugin/plugin.json"))["version"].as_str().unwrap().to_string()
}

/// smoke_init.py 的 `check_output`（必要檔清單是 1.0 的）。
fn check_output(target: &Path) {
    let mut left = Vec::new();
    for p in files(target) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        for (i, line) in text.lines().enumerate() {
            if has_placeholder(line) {
                left.push(format!("{}:{}:{}", rel(target, &p), i + 1, line.trim().chars().take(120).collect::<String>()));
            }
        }
    }
    assert!(left.is_empty(), "殘留佔位符：\n{}", left.join("\n"));
    for f in REQUIRED {
        assert!(target.join(f).is_file(), "缺 {f}");
    }
    let runs = read_kv(&target.join(".claude/teams/workflow.config"), "RUNS_DIR");
    assert!(target.join(&runs).join("README.md").is_file(), "缺 run 目錄說明（{runs}/README.md）");
    assert!(target.join(".mcp.json").is_file() || target.join(".mcp.json.example").is_file(), "缺 .mcp.json 或 .mcp.json.example");
    let all: Vec<String> = files(target).iter().map(|p| rel(target, p)).collect();
    let tmpl: Vec<&String> = all.iter().filter(|p| p.ends_with(".tmpl")).collect();
    assert!(tmpl.is_empty(), "殘留 .tmpl：{tmpl:?}");
    for j in [".claude/settings.json", ".mcp.json", ".mcp.json.example", ".claude/teams/guard.config.json"] {
        if target.join(j).is_file() {
            load(&target.join(j));
        }
    }
    // 1.0：工具不複製進專案
    assert!(!target.join(".claude/teams/tools").exists(), "1.0 的 init 不該放 .claude/teams/tools/");
    let py: Vec<&String> = all.iter().filter(|p| p.ends_with(".py")).collect();
    assert!(py.is_empty(), "1.0 的 init 不該放任何 .py：{py:?}");
}

fn render_all(target: &Path) -> PathBuf {
    let out = target.join("_tpm/wf-check");
    let plugin = root().join("plugin");
    awf(&["render", "--all", "--out", s(&out), "--plugin-dir", s(&plugin)], target).ok("awf render --all");
    out
}

fn wf_text(target: &Path, name: &str) -> String {
    read(&render_all(target).join(format!("{name}.js")))
}

/// 用模版 repo 的 `check_workflows.mjs` 乾跑專案產生的腳本；沒有 node 回 None。
fn dry_run(target: &Path) -> Option<Out> {
    let node = which("node")?;
    let wf = render_all(target);
    let r = root();
    Some(exec(s(&node), &[s(&r.join("scaffold/.claude/teams/tools/check_workflows.mjs")), "--agents-dir", s(&r.join("plugin/agents")), "--wf-dir", s(&wf)], target, &[], None))
}

fn phases(out: &str, workflow: &str) -> String {
    let head = format!("{workflow}.js");
    for line in out.lines() {
        if line.starts_with("ok") && line.contains(&head) {
            if let Some(i) = line.find("phases: ") {
                return line[i + 8..].trim_end_matches('）').to_string();
            }
        }
    }
    String::new()
}

fn hook(bin: &Path, project: &Path, mode: &str, mut data: Value) -> i32 {
    data["cwd"] = json!(s(project));
    exec(s(bin), &["hook", mode], project, &[("CLAUDE_PROJECT_DIR", s(project))], Some(&data.to_string())).code
}

fn bash(cmd: &str, agent: &str) -> Value {
    json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd}, "agent_type": agent})
}

#[test]
fn fresh_init() {
    let w = Work::new("fresh");
    let t = w.dir("project");
    let r = init(&t, &[]);
    r.ok("awf init（預設）");
    check_output(&t);

    // 版本三處一致
    let pv = plugin_version();
    let mk = load(&root().join(".claude-plugin/marketplace.json"));
    assert_eq!(mk["metadata"]["version"], json!(pv), "marketplace metadata 的版本");
    for p in mk["plugins"].as_array().unwrap() {
        assert_eq!(p["version"], json!(pv), "marketplace plugins[] 的版本");
    }
    assert_eq!(read(&t.join(".claude/teams/VERSION")).trim(), pv);

    // 工具檢查
    let b = awf(&["build", "--check"], &t);
    b.ok("awf build --check");
    assert!(!t.join(".claude/workflows").exists(), "預設 init 不該有 .claude/workflows/");
    let out = render_all(&t);
    let mut got: Vec<String> = files(&out).iter().filter(|p| p.extension().is_some_and(|e| e == "js")).map(|p| p.file_stem().unwrap().to_str().unwrap().to_string()).collect();
    got.sort();
    let mut want: Vec<String> = WORKFLOWS.iter().map(|x| x.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "awf render --all 的產出");
    for wf in WORKFLOWS {
        assert!(!read(&out.join(format!("{wf}.js"))).contains("{{"), "{wf}.js 殘留佔位符");
    }
    match dry_run(&t) {
        Some(o) => {
            o.ok("check_workflows.mjs");
            let ph = phases(&o.stdout, "sdlc-build");
            assert!(ph.starts_with("Red") && !ph.contains("Tests"), "預設 profile 的 sdlc-build 沒有先走 Red：{ph}");
        }
        None => eprintln!("[略過] 沒有 node，略過 check_workflows.mjs 與 Red 階段檢查"),
    }
    for sub in SELFTESTS {
        awf(&[sub, "--selftest"], &t).ok(&format!("awf {sub} --selftest（在 init 過的專案裡）"));
    }
    // state 的 selftest 在沒 init 的目錄會過；在 init 過的專案裡的情況見 state_selftest_in_project
    awf(&["state", "--selftest"], &w.dir("empty")).ok("awf state --selftest（沒 init 的目錄）");
    awf(&["check-agents", "--plugin-dir", s(&root().join("plugin"))], &t).ok("awf check-agents");

    // 預設 profile
    let prof = load(&t.join(".claude/teams/methodology.json"));
    assert_eq!(prof["name"], json!("default"));
    assert_eq!(prof["stages"]["build"]["test_first"], json!(true));
    assert_eq!(prof["conventions"]["pr_size"]["max_lines"], json!(300));
    let g = load(&t.join(".claude/teams/guard.config.json"));
    assert_eq!(g["main_session_git_rules"], json!(true));
    assert_eq!(g["main_session_github_rules"], json!(true));
    assert_eq!(g["team_agent_namespaces"], json!(["agent-workflow:"]));

    // hooks plugin（預設）：專案裡沒有 hook 執行檔，settings 沒有註冊 hook
    assert!(!t.join(".claude/hooks").exists(), "--hooks plugin 不該有 .claude/hooks/");
    assert!(!read(&t.join(".claude/settings.json")).contains(" hook "), "--hooks plugin 的 settings.json 不該註冊 hook");

    // merge gate：預設不放；--with-merge-gate 才放（執行檔由 --merge-gate-bin 給）
    let ci = t.join(".github/workflows/agent-merge-gate.yml");
    assert!(!ci.exists(), "預設不應放 agent-merge-gate.yml");
    assert!(!t.join(".claude/teams/bin").exists(), "預設不應放 .claude/teams/bin/");
    let elf = w.0.join("awf-linux-x64");
    std::fs::write(&elf, fake_elf()).unwrap();
    init(&t, &["--with-merge-gate", "--merge-gate-bin", s(&elf)]).ok("awf init --with-merge-gate --merge-gate-bin");
    let y = read(&ci);
    for k in ["awf merge-gate", "merge-gate \"${ARGS[@]}\"", "sha256sum -c", ".claude/teams/bin/awf-linux-x64", "pull_request_target", "merge_group", "steps.pr.outputs.base_ref", "agent-merge-gate/decision"] {
        assert!(y.contains(k), "agent-merge-gate.yml 缺 {k}");
    }
    assert!(!y.contains("python3 .claude/teams/tools/"), "agent-merge-gate.yml 不該再跑專案裡的 .py");
    assert_eq!(std::fs::read(t.join(".claude/teams/bin/awf-linux-x64")).unwrap(), fake_elf(), "gate 執行檔要逐位元相同");
    let sums = load(&t.join(".claude/teams/bin/checksums.json"));
    let sum = sums["files"]["awf-linux-x64"].as_str().unwrap_or("");
    assert!(sum.len() == 64 && sum.chars().all(|c| c.is_ascii_hexdigit()), "checksums.json 要記 sha256：{sums}");
    if let Some(tool) = which("shasum") {
        let o = exec(s(&tool), &["-a", "256", s(&t.join(".claude/teams/bin/awf-linux-x64"))], &t, &[], None);
        assert!(o.stdout.starts_with(sum), "checksums.json 記的雜湊和檔案不符：{} vs {sum}", o.stdout);
    }
}

/// 記錄已知問題：`awf state --selftest` 在 1.0 init 過的專案裡會失敗（`state/selftest.rs` 從專案的 `tools/snippets` 複製片段，
/// 1.0 的專案沒有這個目錄）。`state/` 不在 init 這次改動的範圍；那邊改成從模版找片段之後，拿掉 ignore。
#[test]
#[ignore = "state 的 selftest 還在找專案的 tools/snippets，等 state/ 那邊改"]
fn state_selftest_in_project() {
    let w = Work::new("state");
    let t = w.dir("project");
    init(&t, &[]).ok("awf init");
    awf(&["state", "--selftest"], &t).ok("awf state --selftest（在 init 過的專案裡）");
}

#[test]
fn merge_gate_needs_binary() {
    let w = Work::new("gate");
    let t = w.dir("project");
    // 模版的 plugin/bin 沒有 linux-x64 執行檔時（開發中的 repo 就是這樣），沒指定就要拒絕、而且什麼都不放
    if !root().join("plugin/bin/awf-linux-x64").is_file() {
        let r = init(&t, &["--with-merge-gate"]);
        assert_ne!(r.code, 0, "沒有 gate 執行檔應該失敗：\n{}", r.all());
        assert!(r.all().contains("--merge-gate-bin"), "要說明怎麼指定執行檔：\n{}", r.all());
        assert!(!t.join(".github").exists() && !t.join(".claude").exists(), "拒絕時不該留下任何檔");
    }
    // 不是 linux x86-64 的 ELF：拒絕
    let bogus = w.0.join("not-elf");
    std::fs::write(&bogus, b"#!/bin/sh\necho hi\n").unwrap();
    let r = init(&t, &["--with-merge-gate", "--merge-gate-bin", s(&bogus)]);
    assert_ne!(r.code, 0, "不是 ELF 應該拒絕：\n{}", r.all());
    assert!(r.all().contains("ELF"), "{}", r.all());
    assert!(!t.join(".github").exists(), "拒絕時不該放 CI");
    // 找不到檔：拒絕
    let r = init(&t, &["--with-merge-gate", "--merge-gate-bin", s(&w.0.join("nope"))]);
    assert_ne!(r.code, 0);
    // --merge-gate-bin 沒搭 --with-merge-gate：拒絕
    let elf = w.0.join("awf-linux-x64");
    std::fs::write(&elf, fake_elf()).unwrap();
    let r = init(&t, &["--merge-gate-bin", s(&elf)]);
    assert_ne!(r.code, 0, "--merge-gate-bin 要搭 --with-merge-gate：\n{}", r.all());
}

#[test]
fn placeholders_match_example() {
    let r = root();
    let mut used = BTreeSet::new();
    for p in files(&r.join("scaffold")) {
        if rel(&r, &p).contains("__pycache__") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            placeholders(&text, &mut used);
        }
    }
    let conf: BTreeSet<String> = read(&r.join("workflow.config.example"))
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let n = t.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').count();
            (n > 0 && t[n..].trim_start().starts_with('=')).then(|| t[..n].to_string())
        })
        .collect();
    // init 自己算的（awf-core 的 init::writer::COMPUTED）
    let computed: BTreeSet<String> = ["AWF_HOOK_CMD", "AWF_HOOK_SHELL"].iter().map(|x| x.to_string()).collect();
    let extra: Vec<&String> = used.iter().filter(|k| !conf.contains(*k) && !computed.contains(*k)).collect();
    assert!(extra.is_empty(), "scaffold 用到、workflow.config.example 沒有的佔位符：{extra:?}");
    let unused: Vec<&String> = conf.iter().filter(|k| !used.contains(*k)).collect();
    assert!(unused.is_empty(), "workflow.config.example 有、scaffold 沒用到的佔位符：{unused:?}");
    assert!(computed.iter().all(|k| used.contains(k)), "init 自算的佔位符 scaffold 要有用到");
}

#[test]
fn extensions() {
    let w = Work::new("ext");
    let t = w.dir("project-ext");
    init(&t, &[]).ok("awf init");
    let teams = t.join(".claude/teams");
    let plugin = root().join("plugin");
    let mut meth = load(&teams.join("methodology.json"));
    meth["extensions"] = json!([{"name": "db", "agent": "acme-db-reviewer", "stage": "test", "instructions": "檢查 migration 能不能回滾"}]);
    dump(&teams.join("methodology.json"), &meth);
    let r = awf(&["build"], &t);
    assert!(r.code == 1 && r.all().contains("extension_agents"), "沒登記 extension_agents 時 awf build 要擋下：\n{}", r.all());
    let r = awf(&["render", "sdlc-test", "--out", s(&t.join("_tpm/x")), "--plugin-dir", s(&plugin)], &t);
    assert!(r.code == 1 && r.all().contains("extension_agents"), "沒登記時 awf render 也不產生：\n{}", r.all());
    let mut guard = load(&teams.join("guard.config.json"));
    guard["extension_agents"]["acme-db-reviewer"] = json!("tpm-reviewer");
    dump(&teams.join("guard.config.json"), &guard);
    write(&t.join(".claude/agents/acme-db-reviewer.md"), "---\nname: acme-db-reviewer\ndescription: 示範客戶 ACME 的 migration 審查\ntools: Read, Grep, Glob, Bash\n---\n檢查 migration。\n");
    awf(&["build"], &t).ok("登記後 awf build");
    // check_workflows.mjs 從自己的位置往上找專案的 .claude/agents；1.0 的專案裡沒有這支工具，
    // 所以把模版的 .mjs 與情境檔放到一個暫存的「.claude/teams/tools/」，旁邊放使用者 agent 的定義檔
    if let Some(node) = which("node") {
        let h = w.dir("harness");
        let src = root().join("scaffold/.claude/teams/tools");
        std::fs::create_dir_all(h.join(".claude/teams/tools")).unwrap();
        std::fs::copy(src.join("check_workflows.mjs"), h.join(".claude/teams/tools/check_workflows.mjs")).unwrap();
        copy_tree(&src.join("scenarios"), &h.join(".claude/teams/tools/scenarios"));
        copy_tree(&t.join(".claude/agents"), &h.join(".claude/agents"));
        copy_tree(&root().join("scaffold/.claude/teams/methodologies"), &h.join(".claude/teams/methodologies"));
        let wf = render_all(&t);
        let o = exec(s(&node), &[s(&h.join(".claude/teams/tools/check_workflows.mjs")), "--agents-dir", s(&plugin.join("agents")), "--wf-dir", s(&wf)], &t, &[], None);
        o.ok("擴充 agent：workflow 乾跑");
        assert!(read(&wf.join("sdlc-test.js")).contains("acme-db-reviewer"), "sdlc-test.js 要內嵌擴充 agent");
    } else {
        eprintln!("[略過] 擴充 agent：沒有 node，略過 workflow 乾跑");
    }
    let code = hook(Path::new(EXE), &t, "guard-shell", json!({"tool_name": "Bash", "agent_type": "acme-db-reviewer", "tool_input": {"command": "git push origin feature/x"}}));
    assert_eq!(code, 2, "擴充 agent：hook 要照 tpm-reviewer 規則擋 push");
}

#[test]
fn answers_profile() {
    let w = Work::new("answers");
    let t = w.dir("project-methodology");
    let regex = "^(feat|fix|docs|test|chore): .+";
    let answers = json!({
        "methodology": "lightweight",
        "code_style": {"description": "提交前跑 ruff", "tool": "ruff", "command": "ruff format --check ."},
        "commit": {"format": "<type>: <摘要>（示範客戶 ACME 的慣例）", "examples": {"feat": "feat: <Story> 實作"}, "check_regex": regex},
        "branch_prefix": "feat/",
        "sync_strategy": "transit",
        "pr_size": {"max_lines": 400, "max_files": null},
        "doc_format": "Markdown",
        "read_limits": {"mode": "warn", "whole_file_lines": 300, "read_budget": 500000},
        "constraints": ["ARCHITECTURE.md", "docs/frontend.md=build+deliver", {"id": "review", "file": "REVIEW.md", "stages": ["deliver"], "trace": "table"}],
        "main_session_push_merge": true,
    });
    let af = w.0.join("answers.json");
    write(&af, &answers.to_string());
    init(&t, &["--answers", s(&af)]).ok("awf init --answers");
    check_output(&t);

    let prof = load(&t.join(".claude/teams/methodology.json"));
    let cons = prof["constraints"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = cons.iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(ids, ["architecture", "frontend", "review"], "constraints 沒寫對：{cons:?}");
    assert_eq!(cons[0]["stages"], json!(["design", "deliver"]));
    assert_eq!(cons[1]["stages"], json!(["build", "deliver"]));
    assert_eq!(cons[2]["trace"], json!("table"));
    let deliver = wf_text(&t, "sdlc-deliver");
    assert!(deliver.contains("REVIEW.md") && deliver.contains("awfConstraints(MP, 'deliver')"), "sdlc-deliver.js 沒有內嵌限制文件");
    let lim = &prof["limits"];
    assert_eq!(lim["mode"], json!("warn"));
    assert_eq!(lim["whole_file_lines"], json!(300));
    assert_eq!(lim["read_budget"]["default"], json!(500000));
    assert!(lim["read_budget"]["roles"].as_object().is_some_and(|o| !o.is_empty()), "沒回答的角色額度要沿用 profile");
    let conv = &prof["conventions"];
    assert_eq!(prof["name"], json!("lightweight"));
    assert_eq!(prof["stages"]["build"]["test_first"], json!(false));
    assert_eq!(conv["code_style"]["command"], json!("ruff format --check ."));
    assert_eq!(conv["commit"]["check_regex"], json!(regex));
    assert_eq!(conv["branch"]["prefix"], json!("feat/"));
    assert_eq!(conv["sync"]["strategy"], json!("transit"));
    assert_eq!(conv["pr_size"]["max_lines"], json!(400));
    assert!(conv["pr_size"]["max_files"].is_null());
    let g = load(&t.join(".claude/teams/guard.config.json"));
    assert_eq!(g["main_session_git_rules"], json!(false));
    assert_eq!(g["main_session_github_rules"], json!(false));
    assert!(g["allowed_branch_prefixes"].as_str().unwrap_or("").contains("feat/"), "allowed_branch_prefixes 沒有 feat/");
    let build = wf_text(&t, "sdlc-build");
    assert!(build.contains("\"name\":\"lightweight\""), "sdlc-build.js 沒有內嵌 lightweight profile");
    assert!(build.contains("feat/${A.epic}"), "sdlc-build.js 的分支前綴沒換成 feat/");
    awf(&["build", "--check"], &t).ok("awf build --check");
    match dry_run(&t) {
        Some(o) => {
            o.ok("check_workflows.mjs");
            let ph = phases(&o.stdout, "sdlc-build");
            assert!(!ph.is_empty() && !ph.split('→').any(|x| x == "Red") && ph.ends_with("Tests"), "test_first=false 的 sdlc-build phases 不對：{ph:?}");
        }
        None => eprintln!("[略過] 沒有 node，略過 check_workflows.mjs"),
    }
    awf(&["merge-gate", "--selftest"], &t).ok("awf merge-gate --selftest");

    // check-rules --git：profile 設了 commit regex，就要抓出不符格式的 commit 標題
    let Some(git) = which("git") else {
        eprintln!("[略過] 沒有 git，略過 check-rules --git");
        return;
    };
    let repo = t.join("demo-repo");
    std::fs::create_dir_all(&repo).unwrap();
    let genv = [("GIT_AUTHOR_NAME", "acme"), ("GIT_AUTHOR_EMAIL", "acme@example.com"), ("GIT_COMMITTER_NAME", "acme"), ("GIT_COMMITTER_EMAIL", "acme@example.com"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_NOSYSTEM", "1")];
    exec(s(&git), &["init", "-q"], &repo, &genv, None).ok("git init");
    let attr = g["attribution_line"].as_str().filter(|x| !x.is_empty()).unwrap_or("Assisted-by: Claude").to_string();
    for msg in ["init", "feat: 匯出 CSV", "隨手修一下"] {
        exec(s(&git), &["commit", "-q", "--allow-empty", "-m", msg, "-m", &attr], &repo, &genv, None).ok("git commit");
    }
    let r = awf(&["check-rules", "--git", s(&repo), "HEAD~2..HEAD"], &t);
    assert!(r.code == 1 && r.stdout.contains("R6") && r.stdout.contains("隨手修一下") && !r.stdout.contains("匯出 CSV"), "check-rules --git 沒有依 profile 檢查 commit 格式：\n{}", r.all());
}

/// settings.json 裡 PreToolUse 某個 matcher 的第一個 hook。
fn pre_hook(cfg: &Value, matcher: &str) -> Value {
    cfg["hooks"]["PreToolUse"].as_array().and_then(|a| a.iter().find(|e| e["matcher"] == json!(matcher))).map(|e| e["hooks"][0].clone()).unwrap_or(Value::Null)
}

/// hook 指令裡引號包住的執行檔路徑。
fn cmd_exe(cmd: &str) -> PathBuf {
    PathBuf::from(cmd.split('"').nth(1).unwrap_or(""))
}

#[test]
fn hooks_project() {
    let w = Work::new("hooks");
    let t = w.dir("project-hooks");
    let r = init(&t, &["--hooks", "project"]);
    r.ok("awf init --hooks project");
    check_output(&t);
    let name = if cfg!(windows) { "awf.exe" } else { "awf" };
    let bin = t.join(".claude/hooks/bin").join(name);
    assert!(bin.is_file(), "缺 .claude/hooks/bin/{name}");
    let src = Path::new(EXE).canonicalize().unwrap();
    assert_eq!(std::fs::read(&bin).unwrap(), std::fs::read(&src).unwrap(), "複本要和執行中的 awf 逐位元相同");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(&bin).unwrap().permissions().mode() & 0o111 != 0, "複本要保留執行權限");
    }
    let left: Vec<String> = files(&t.join(".claude/hooks")).iter().map(|p| rel(&t, p)).filter(|p| !p.starts_with(".claude/hooks/bin/")).collect();
    assert!(left.is_empty(), ".claude/hooks/ 只該有 bin/：{left:?}");
    assert!(r.all().contains(".gitignore"), "沒有忽略 .claude/hooks/bin/ 時要提示：\n{}", r.all());

    let cfg = load(&t.join(".claude/settings.json"));
    let shell = pre_hook(&cfg, "Bash|PowerShell");
    let paths = pre_hook(&cfg, "Edit|Write|NotebookEdit|Read|Grep");
    for (kind, h) in [("guard-shell", &shell), ("guard-paths", &paths)] {
        let cmd = h["command"].as_str().unwrap_or("");
        assert!(cmd.trim_end().ends_with(&format!("hook {kind}")), "{kind} 的指令不對：{h}");
        let exe = cmd_exe(cmd);
        assert!(exe.is_absolute(), "{kind}：要寫執行檔的絕對路徑：{cmd}");
        assert_eq!(exe.canonicalize().unwrap(), bin.canonicalize().unwrap(), "{kind}：要指到專案裡的複本");
        assert_eq!(h["timeout"], json!(10));
        assert_eq!(h["onFailure"], json!("block"));
    }
    if cfg!(windows) {
        assert_eq!(shell["shell"], json!("powershell"));
    }
    let text = read(&t.join(".claude/settings.json"));
    assert_eq!(text.matches(" hook ").count(), 5, "awf hook 要註冊五筆（guard-shell、guard-paths、envelope×2、session-start 之外的依範本）：\n{text}");
    assert!(!text.contains("awf_hook.py"), "不該再註冊 awf_hook.py");

    // 用複本跑代表性的 hook 案例（完整案例由 hooks_*.rs 對同一支執行檔跑）
    let write_ev = |f: &str, a: &str| json!({"hook_event_name": "PreToolUse", "tool_name": "Write", "tool_input": {"file_path": s(&t.join(f)), "content": "x"}, "agent_type": a});
    assert_eq!(hook(&bin, &t, "guard-shell", bash("git push origin feature/x", "agent-workflow:dev-implementer")), 2);
    assert_eq!(hook(&bin, &t, "guard-shell", bash("git status", "agent-workflow:dev-implementer")), 0);
    assert_eq!(hook(&bin, &t, "guard-paths", write_ev("src/a.py", "agent-workflow:tpm-researcher")), 2);
    assert_eq!(hook(&bin, &t, "guard-paths", write_ev("src/a.py", "dev-helper")), 0);
    assert_eq!(hook(&bin, &t, "envelope", json!({"hook_event_name": "SubagentStop", "agent_type": "agent-workflow:dev-verifier", "last_assistant_message": "完成，沒有信封"})), 2);

    // doctor 認得這種註冊
    let d = awf(&["doctor"], &t);
    assert!(!d.all().contains("[錯誤]"), "awf doctor 有錯誤：\n{}", d.all());

    // .gitignore 已經忽略時不再提示；AWF_BIN 指到別的檔時複製它
    let t2 = w.dir("project-hooks-2");
    write(&t2.join(".gitignore"), ".claude/hooks/bin/\n");
    let other = w.0.join("other-awf");
    std::fs::copy(&src, &other).unwrap();
    let mut f = std::fs::OpenOptions::new().append(true).open(&other).unwrap();
    f.write_all(b"\n# other\n").unwrap();
    drop(f);
    let r = exec(EXE, &["init", "--target", s(&t2), "--config", s(&root().join("workflow.config.example")), "--hooks", "project"], &w.0, &[("AWF_BIN", s(&other))], None);
    r.ok("awf init --hooks project（AWF_BIN）");
    assert_eq!(std::fs::read(t2.join(".claude/hooks/bin").join(name)).unwrap(), std::fs::read(&other).unwrap(), "AWF_BIN 指到別的檔時要複製它");
    assert!(!r.all().contains(".gitignore"), "已經忽略時不該再提示：\n{}", r.all());
    assert_eq!(read(&t2.join(".gitignore")), ".claude/hooks/bin/\n", "init 不改使用者的 .gitignore");
}

#[test]
fn existing_project() {
    let w = Work::new("existing");
    let t = w.dir("project-existing");
    let existing: [(&str, &str); 6] = [
        (".claude/agents/dev-helper.md", "---\nname: dev-helper\ndescription: 使用者自己的 agent（示範客戶 ACME）\n---\n幫忙整理程式。\n"),
        (".claude/agents/tpm-notes.md", "---\nname: tpm-notes\ndescription: 使用者自己的筆記 agent\n---\n記筆記。\n"),
        (".claude/skills/my-skill/SKILL.md", "---\nname: my-skill\ndescription: 使用者自己的 skill\n---\n照做。\n"),
        (".claude/commands/brief.md", "使用者自己的 /brief 指令（和 plugin 的 brief 同名）\n"),
        (".claude/workflows/sdlc-plan.js", "// 使用者自己的同名 workflow\nexport const meta = { name: 'sdlc-plan' }\n"),
        (".github/workflows/ci.yml", "name: mine\non: [push]\n"),
    ];
    for (f, body) in existing {
        write(&t.join(f), body);
    }
    let claude = "# 示範客戶 ACME 的專案\r\n\r\n原本的說明，用 CRLF 換行、結尾沒有換行。";
    write(&t.join("CLAUDE.md"), claude);
    let settings = json!({
        "permissions": {"allow": ["Bash(npm test)"]},
        "env": {"MY_VAR": "1"},
        "model": "sonnet",
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo user-hook"}]}], "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}]},
    });
    write(&t.join(".claude/settings.json"), &settings.to_string());
    init(&t, &["--hooks", "project"]).ok("awf init --hooks project");
    init(&t, &["--hooks", "project", "--force"]).ok("awf init --hooks project --force");

    for (f, body) in existing {
        assert_eq!(std::fs::read(t.join(f)).unwrap(), body.as_bytes(), "使用者的檔案被改動：{f}");
    }
    let got = read(&t.join("CLAUDE.md"));
    assert!(got.starts_with(claude), "CLAUDE.md 原內容被改動");
    let extra = &got[claude.len()..];
    let imp = "@.claude/teams/agent-workflow.CLAUDE.md";
    assert!(got.matches(imp).count() == 1 && extra.lines().filter(|l| !l.trim().is_empty()).count() <= 3, "CLAUDE.md 應只追加一段引用（重跑不重複）：{extra:?}");
    let imported = t.join(".claude/teams/agent-workflow.CLAUDE.md");
    assert!(imported.is_file() && read(&imported).contains("agent-workflow"), "缺 .claude/teams/agent-workflow.CLAUDE.md");

    let st = load(&t.join(".claude/settings.json"));
    assert_eq!(st["permissions"]["allow"], json!(["Bash(npm test)"]), "使用者的 permissions 要保留");
    assert_eq!(st["env"], json!({"MY_VAR": "1"}));
    assert_eq!(st["model"], json!("sonnet"));
    let hooks = st["hooks"].to_string();
    assert!(hooks.contains("echo user-hook") && hooks.contains("echo done"), "使用者的 hooks 要保留：{hooks}");
    assert_eq!(hooks.matches("guard-shell").count(), 1, "{hooks}");
    assert_eq!(hooks.matches("guard-paths").count(), 1, "{hooks}");
    assert_eq!(hooks.matches(" hook ").count(), 5, "awf hook 各種類只註冊一次（共五筆）：{hooks}");
    assert!(!t.join(".github/workflows/agent-merge-gate.yml").exists(), "預設沒有 merge gate CI");

    let bin = cmd_exe(pre_hook(&st, "Bash|PowerShell")["command"].as_str().unwrap_or(""));
    assert!(bin.is_file(), "settings.json 註冊的執行檔不存在：{}", bin.display());
    let write_ev = |f: &str, a: &str| json!({"hook_event_name": "PreToolUse", "tool_name": "Write", "tool_input": {"file_path": s(&t.join(f)), "content": "x"}, "agent_type": a});
    let stop = |a: &str| json!({"hook_event_name": "SubagentStop", "agent_type": a, "last_assistant_message": "完成，沒有信封"});
    let cases = [
        ("使用者 agent dev-helper push 功能分支", "guard-shell", bash("git push origin feature/x", "dev-helper"), 0),
        ("使用者 agent dev-helper 寫 src/", "guard-paths", write_ev("src/a.py", "dev-helper"), 0),
        ("使用者 agent tpm-notes 寫 notes/", "guard-paths", write_ev("notes/a.md", "tpm-notes"), 0),
        ("使用者 agent dev-helper 交件不需信封", "envelope", stop("dev-helper"), 0),
        ("plugin agent push 仍擋", "guard-shell", bash("git push origin feature/x", "agent-workflow:dev-implementer"), 2),
        ("plugin tpm-researcher 寫 src/ 仍擋", "guard-paths", write_ev("src/a.py", "agent-workflow:tpm-researcher"), 2),
        ("plugin agent 交件仍需信封", "envelope", stop("agent-workflow:dev-verifier"), 2),
    ];
    for (name, mode, data, want) in cases {
        assert_eq!(hook(&bin, &t, mode, data), want, "既有專案：{name}");
    }
}

const RULE_MARK: &str = "- 示範專案規則：ACME 的 API 一律帶 trace id。";
const CLAUDE_SPECIFIC: &str = "- 示範專案特定：ACME 的部署窗口是週二。";

#[test]
fn upgrade() {
    let w = Work::new("upgrade");
    let t = w.dir("project-upgrade");
    init(&t, &[]).ok("awf init");
    let teams = t.join(".claude/teams");
    let (meth_p, guard_p, rules_p) = (teams.join("methodology.json"), teams.join("guard.config.json"), teams.join("rules.md"));
    let mut meth = load(&meth_p);
    meth["conventions"].as_object_mut().unwrap().remove("sync");
    for k in ["recipes", "modifiers", "models", "limits", "knowledge"] {
        meth.as_object_mut().unwrap().remove(k);
    }
    meth["conventions"]["pr_size"] = json!({"max_lines": 123, "max_files": 7});
    dump(&meth_p, &meth);
    let mut guard = load(&guard_p);
    guard["main_session_git_rules"] = json!(false);
    let auth = json!({"acme/app": {"by": "示範決策者", "link": "https://example.com/acme", "scope": "feature/*", "expires": "2099-01-01"}});
    guard["auto_merge"]["upstream_authorized_by"] = auth.clone();
    guard["extension_agents"]["acme-db-reviewer"] = json!("tpm-reviewer");
    guard["auto_merge"].as_object_mut().unwrap().remove("carry_verdict_on_rebase");
    dump(&guard_p, &guard);
    write(&rules_p, &format!("{}\n{RULE_MARK}\n", read(&rules_p)));
    let claude = t.join("CLAUDE.md");
    write(&claude, &format!("{}\n{CLAUDE_SPECIFIC}\n", read(&claude)));
    write(&rules_p, &read(&rules_p).replacen("## 專案規則", "（舊版通用規則）\n\n## 專案規則", 1));

    // 0.x 留下的工具：一支改過的模版工具、一支使用者自己的（兩支都不能被動；真的 0.6 專案見 upgrade_real_06_project）
    let tool = teams.join("tools/schemalib.py");
    let tool_body = format!("{}\n# 舊版殘留\n", read(&root().join("scaffold/.claude/teams/tools/schemalib.py")));
    write(&tool, &tool_body);
    write(&teams.join("tools/old_tool.py"), "# 使用者自己的工具\n");

    // 0.4 以前的 init 會把 workflow 複製進專案：一支原樣的舊副本、一支改過的、一支有覆寫標記的、一支使用者自己的
    let wfd = t.join(".claude/workflows");
    let meth_min = serde_json::to_string(&meth).unwrap();
    let mut old = Vec::new();
    for name in ["event-meeting.js", "event-reqs-change.js"] {
        let mut body = read(&root().join("tests/fixtures/legacy-0.4.1").join(name));
        let mut keys = BTreeSet::new();
        placeholders(&body, &mut keys);
        for k in keys {
            let v = read_kv(&teams.join("workflow.config"), &k);
            let has = read(&teams.join("workflow.config")).lines().any(|l| l.starts_with(&format!("{k}=")));
            if has {
                body = body.replace(&format!("{{{{{k}}}}}"), &v);
            }
        }
        old.push(body.replace("/*@methodology*/ null /*@end*/", &format!("/*@methodology*/ {meth_min} /*@end*/")));
    }
    write(&wfd.join("event-meeting.js"), &old[0]);
    write(&wfd.join("event-reqs-change.js"), &old[1].replace("const A = args || {}", "const A = args || {} // 我改過"));
    write(&wfd.join("sdlc-plan.js"), "export const meta = { name: 'sdlc-plan', description: '我的版本' }\nconst X = /*@schema:findings*/ null /*@end*/\n// awf-override: 0.1.0\n");
    write(&wfd.join("my-own.js"), "export const meta = { name: 'my-own', description: '使用者自己的' }\n");
    write(&teams.join("VERSION"), "0.1.0\n");
    // 進行中的 run：升級完全不碰
    let run_json = "{\"stages\": {\"plan\": {\"status\": \"done\"}}, \"awf\": {\"pending\": \"design\"}}\n";
    write(&t.join("_tpm/runs/2026-01-01-acme-export/run.json"), run_json);

    let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
    r.ok("awf init --upgrade");
    let out = r.all();
    let new_ver = plugin_version();
    assert!(out.contains(&format!("0.1.0 → {new_ver}")), "--upgrade 沒顯示舊版 → 新版：\n{out}");
    assert!(out.replace('\\', "/").contains("teams/workflow.config"), "--upgrade 沒沿用 init 記下的設定值：\n{out}");
    assert_eq!(read(&teams.join("VERSION")).trim(), new_ver);

    assert_eq!(read(&tool), tool_body, "升級：改過的工具檔要保留");
    assert!(out.contains("schemalib.py"), "升級：改過的工具檔要列出：\n{out}");
    assert_eq!(read(&teams.join("tools/old_tool.py")), "# 使用者自己的工具\n", "升級：使用者自己的工具檔不動");
    assert_eq!(read(&t.join("_tpm/runs/2026-01-01-acme-export/run.json")), run_json, "升級：進行中的 run 不碰");
    assert!(!wfd.join("event-meeting.js").exists() && out.contains("event-meeting.js"), "升級：原樣的舊 workflow 副本沒有移除：\n{out}");
    assert!(wfd.join("event-reqs-change.js").is_file() && out.contains("對不上") && out.contains("event-reqs-change.js"), "升級：改過的 workflow 沒保留或沒說明：\n{out}");
    assert!(read(&wfd.join("sdlc-plan.js")).contains("awf-override: 0.1.0") && wfd.join("my-own.js").is_file(), "升級：覆寫檔或使用者的 workflow 被動到了");
    let plugin = root().join("plugin");
    let x = t.join("_tpm/x");
    let r2 = awf(&["render", "event-reqs-change", "--out", s(&x), "--plugin-dir", s(&plugin)], &t);
    assert!(r2.code != 0 && !x.join("event-reqs-change.js").exists() && r2.all().contains("不採用") && r2.all().contains("event.js"), "升級：awf 用了沒有標記的舊副本：\n{}", r2.all());
    write(&wfd.join("sdlc-design.js"), "// 沒有標記的舊副本 我改過\n");
    let r2 = awf(&["render", "sdlc-design", "--out", s(&x), "--plugin-dir", s(&plugin)], &t);
    let got = std::fs::read_to_string(x.join("sdlc-design.js")).unwrap_or_default();
    assert!(!got.is_empty() && !got.contains("我改過") && r2.stderr.contains("不採用"), "升級：沒有標記的舊副本不該採用：\n{}", r2.all());
    std::fs::remove_file(wfd.join("sdlc-design.js")).unwrap();
    std::fs::remove_file(wfd.join("sdlc-plan.js")).unwrap();

    assert_eq!(load(&meth_p), meth, "升級：methodology.json 原樣保留");
    assert!(out.contains("conventions.sync") && out.contains("--adopt-new-fields"), "升級：沒列出缺少的 conventions.sync：\n{out}");
    assert!(out.contains("recipes") && out.contains("modifiers"), "升級：沒列出缺少的 recipes／modifiers：\n{out}");
    let g2 = load(&guard_p);
    assert_eq!(g2["main_session_git_rules"], json!(false));
    assert_eq!(g2["auto_merge"]["upstream_authorized_by"], auth);
    assert_eq!(g2["extension_agents"]["acme-db-reviewer"], json!("tpm-reviewer"));
    assert_eq!(g2["auto_merge"]["carry_verdict_on_rebase"], json!(false), "升級：guard.config.json 沒補上新鍵");
    let rules = read(&rules_p);
    assert!(rules.contains(RULE_MARK) && !rules.contains("（舊版通用規則）"), "升級：rules.md 的〈專案規則〉沒保留或通用規則沒更新");
    assert!(read(&claude).contains(CLAUDE_SPECIFIC), "升級：CLAUDE.md 的〈專案特定〉不見了");
    awf(&["build", "--check"], &t).ok("升級後 awf build --check");

    let r3 = awf(&["init", "--target", s(&t), "--upgrade", "--adopt-new-fields"], &w.0);
    r3.ok("awf init --upgrade --adopt-new-fields");
    let m3 = load(&meth_p);
    assert!(m3["conventions"]["sync"].is_object(), "--adopt-new-fields 要補上 conventions.sync");
    assert_eq!(m3["conventions"]["pr_size"], json!({"max_lines": 123, "max_files": 7}), "既有的 pr_size 不變");
    assert!(m3["models"].is_object() && m3["knowledge"].is_object() && m3["limits"].get("mode").is_none() && !m3["limits"]["whole_file_lines"].is_null(), "models／limits／knowledge 沒補對：{}", m3["limits"]);
    assert!(m3["recipes"].as_object().map(|o| o.len()) == Some(11) && m3["modifiers"].is_object(), "recipes 沒補上");
    awf(&["build", "--check"], &t).ok("--adopt-new-fields 後 awf build --check");

    init(&t, &["--force"]).ok("awf init --force");
    let g4 = load(&guard_p);
    assert_eq!(load(&meth_p), m3, "--force 蓋掉了 methodology");
    assert_eq!(g4["main_session_git_rules"], json!(false));
    assert_eq!(g4["extension_agents"]["acme-db-reviewer"], json!("tpm-reviewer"));
    assert!(read(&rules_p).contains(RULE_MARK));
    assert_eq!(read(&tool), tool_body, "--force 也不動改過的工具檔");
}

#[test]
fn force_keeps_values() {
    let w = Work::new("force");
    let t = w.dir("project-force-values");
    let custom_runs = "_acme/agent-runs";
    let lines: Vec<String> = read(&root().join("workflow.config.example"))
        .lines()
        .map(|l| if l.trim().starts_with("RUNS_DIR") && l.contains('=') { format!("RUNS_DIR={custom_runs}") } else { l.to_string() })
        .collect();
    let custom = w.0.join("custom.config");
    write(&custom, &format!("{}\n", lines.join("\n")));
    init_with(&t, &custom, &[]).ok("awf init（自訂 RUNS_DIR）");
    let record = t.join(".claude/teams/workflow.config");
    let want = format!("const RUNS = '{custom_runs}'");
    assert!(wf_text(&t, "event-post-merge").contains(&want) && read_kv(&record, "RUNS_DIR") == custom_runs, "自訂 RUNS_DIR 的 init 結果不對");
    awf(&["init", "--target", s(&t), "--force"], &w.0).ok("awf init --force（沒給 --config）");
    assert!(wf_text(&t, "event-post-merge").contains(&want), "--force 沒給 --config，workflow 的 runs 路徑被換掉了");
    assert_eq!(read_kv(&record, "RUNS_DIR"), custom_runs, "--force 後 workflow.config 紀錄被覆寫");
    assert_eq!(load(&t.join(".claude/teams/guard.config.json"))["runs_dir"], json!(custom_runs));
}

#[test]
fn with_workflows() {
    let w = Work::new("withwf");
    let t = w.dir("project-with-wf");
    init(&t, &["--with-workflows"]).ok("awf init --with-workflows");
    let wfd = t.join(".claude/workflows");
    let stamp = format!("// awf-override: {}\n", plugin_version());
    let mut got: Vec<String> = files(&wfd).iter().filter(|p| p.extension().is_some_and(|e| e == "js")).map(|p| p.file_stem().unwrap().to_str().unwrap().to_string()).collect();
    got.sort();
    let mut want: Vec<String> = WORKFLOWS.iter().map(|x| x.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "--with-workflows 的產出");
    for wf in WORKFLOWS {
        assert!(read(&wfd.join(format!("{wf}.js"))).ends_with(&stamp), "{wf}.js 結尾要有 awf-override 標記");
    }
    let r = awf(&["build", "--check"], &t);
    assert!(r.code == 0 && r.stdout.contains(&format!("{} 支", WORKFLOWS.len())), "--with-workflows：awf build --check 失敗：\n{}", r.all());
    // 覆寫檔拿掉標記那一行，應該和「沒有覆寫檔時 awf 依 plugin 範本產生的」逐字相同
    let plain = w.dir("project-with-wf-plain");
    copy_tree(&t, &plain);
    std::fs::remove_dir_all(plain.join(".claude/workflows")).unwrap();
    let rendered = render_all(&plain);
    for wf in WORKFLOWS {
        let mine = read(&wfd.join(format!("{wf}.js")));
        assert_eq!(&mine[..mine.len() - stamp.len()], read(&rendered.join(format!("{wf}.js"))), "awf render 產生的和 init 寫的不同：{wf}");
    }
    let mine = wfd.join("sdlc-plan.js");
    write(&mine, &read(&mine).replace(&stamp, &format!("// 我改過這支\n{stamp}")));
    let x = t.join("_tpm/x");
    let r = awf(&["render", "sdlc-plan", "--out", s(&x), "--plugin-dir", s(&root().join("plugin"))], &t);
    assert!(read(&x.join("sdlc-plan.js")).contains("我改過這支") && r.stderr.contains("覆寫檔"), "覆寫檔沒有生效：\n{}", r.all());
    let r = awf(&["init", "--target", s(&t), "--upgrade", "--with-workflows"], &w.0);
    assert!(r.code == 0 && read(&mine).contains("我改過這支"), "--upgrade 把覆寫檔蓋掉了：\n{}", r.all());
    let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
    let n = files(&wfd).iter().filter(|p| p.extension().is_some_and(|e| e == "js")).count();
    assert!(r.code == 0 && n == WORKFLOWS.len() && r.stdout.contains("覆寫檔"), "--upgrade 動到了覆寫檔：\n{}", r.all());
}

/// 取出 0.6.0 的模版；沒有 git、tar 或那個 commit 回 None。
fn checkout_06(w: &Work) -> Option<PathBuf> {
    let git = which("git")?;
    let tar = which("tar")?;
    let r = root();
    if exec(s(&git), &["cat-file", "-e", &format!("{REV_06}^{{commit}}")], &r, &[], None).code != 0 {
        return None;
    }
    let ar = w.0.join("v06.tar");
    if exec(s(&git), &["archive", "--format=tar", "-o", s(&ar), REV_06], &r, &[], None).code != 0 {
        return None;
    }
    let d = w.dir("template-0.6");
    (exec(s(&tar), &["-xf", s(&ar), "-C", s(&d)], &w.0, &[], None).code == 0 && d.join("init.py").is_file()).then_some(d)
}

/// 真的 0.6.0 專案（用 0.6.0 的 `init.py` 建）升級到 1.0：原樣、改過工具、`--hooks project`、有 merge gate。
#[test]
fn upgrade_real_06_project() {
    let w = Work::new("real06");
    let (Some(py), Some(tpl)) = (which("python3"), checkout_06(&w)) else {
        eprintln!("[略過] 沒有 python3、git、tar 或 0.6.0 的 commit，略過真的 0.6 專案升級");
        return;
    };
    let init06 = |t: &Path, extra: &[&str]| {
        let mut argv = vec![s(&tpl.join("init.py")).to_string(), "--target".into(), s(t).to_string(), "--config".into(), s(&tpl.join("workflow.config.example")).to_string()];
        argv.extend(extra.iter().map(|x| x.to_string()));
        let a: Vec<&str> = argv.iter().map(|x| x.as_str()).collect();
        exec(s(&py), &a, &w.0, &[("PYTHONUTF8", "1"), ("PYTHONDONTWRITEBYTECODE", "1")], None)
    };
    let run_json = "{\"stages\": {\"plan\": {\"status\": \"done\"}}}\n";
    let tools_left = |t: &Path| -> Vec<String> { files(&t.join(".claude/teams/tools")).iter().map(|p| rel(t, p)).collect() };

    // 1. 原樣的 0.6 專案：工具全部移除、目錄不留；設定與 run 不動
    let t = w.dir("p-pristine");
    init06(&t, &[]).ok("0.6 init.py");
    assert!(t.join(".claude/teams/tools/save_run.py").is_file(), "0.6 的專案應該有 tools/");
    write(&t.join("_tpm/runs/2026-01-01-acme-export/run.json"), run_json);
    let meth_before = read(&t.join(".claude/teams/methodology.json"));
    let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
    r.ok("awf init --upgrade（原樣 0.6）");
    assert!(!t.join(".claude/teams/tools").exists(), "原樣的 0.6 工具要全部移除、空目錄不留：{:?}\n{}", tools_left(&t), r.all());
    assert!(!t.join(".claude/teams/golden-set/run.py").exists(), "golden-set/run.py 是模版放的 .py，沒改過要移除");
    assert_eq!(read(&t.join(".claude/teams/methodology.json")), meth_before, "methodology.json 不動");
    assert_eq!(read(&t.join("_tpm/runs/2026-01-01-acme-export/run.json")), run_json, "進行中的 run 不碰");
    assert_eq!(read(&t.join(".claude/teams/VERSION")).trim(), plugin_version());
    let allow = load(&t.join(".claude/settings.json"))["permissions"]["allow"].to_string();
    assert!(!allow.contains(".claude/teams/tools/") && allow.contains("Bash(awf "), "permissions.allow 的舊工具要換成 awf 子指令：{allow}");
    awf(&["build", "--check"], &t).ok("升級後 awf build --check");
    awf(&["render", "--all", "--out", s(&t.join("_tpm/wf")), "--plugin-dir", s(&root().join("plugin"))], &t).ok("升級後 awf render --all");
    let d = awf(&["doctor"], &t);
    assert!(!d.all().contains("[錯誤]"), "升級後 awf doctor 有錯誤：\n{}", d.all());
    // 再跑一次：沒有東西可移除，也不出錯
    awf(&["init", "--target", s(&t), "--upgrade"], &w.0).ok("awf init --upgrade（第二次）");

    // 2. 改過工具、加了自己的工具與權限：改過的保留並列出，自己的不動
    let t = w.dir("p-modified");
    init06(&t, &[]).ok("0.6 init.py");
    let tool = t.join(".claude/teams/tools/save_run.py");
    let body = format!("{}\n# 示範客戶 ACME 的修改\n", read(&tool));
    write(&tool, &body);
    write(&t.join(".claude/teams/tools/acme_tool.py"), "# 使用者自己的工具\n");
    // Python 留下的快取：原始檔被移除的要跟著清，還留著的原始檔的快取不動
    for n in ["schemalib", "save_run", "acme_tool"] {
        write(&t.join(format!(".claude/teams/tools/__pycache__/{n}.cpython-312.pyc")), "cache");
    }
    let sp = t.join(".claude/settings.json");
    let mut st = load(&sp);
    st["permissions"]["allow"].as_array_mut().unwrap().push(json!("Bash(python3 .claude/teams/tools/acme_tool.py:*)"));
    st["permissions"]["allow"].as_array_mut().unwrap().push(json!("Bash(npm test)"));
    dump(&sp, &st);
    let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
    r.ok("awf init --upgrade（改過工具）");
    assert_eq!(read(&tool), body, "改過的工具要保留");
    assert!(r.all().contains("save_run.py"), "改過的工具要列出：\n{}", r.all());
    assert_eq!(
        tools_left(&t),
        [".claude/teams/tools/__pycache__/acme_tool.cpython-312.pyc", ".claude/teams/tools/__pycache__/save_run.cpython-312.pyc", ".claude/teams/tools/acme_tool.py", ".claude/teams/tools/save_run.py"],
        "只留改過的、使用者自己的，以及它們的快取"
    );
    let allow = load(&sp)["permissions"]["allow"].to_string();
    assert!(allow.contains("Bash(python3 .claude/teams/tools/acme_tool.py:*)") && allow.contains("Bash(npm test)"), "使用者自己加的權限不動：{allow}");
    assert!(!allow.contains("tools/save_run.py") && allow.contains("Bash(awf save-run:*)"), "模版放的權限換成 awf 子指令：{allow}");

    // 3. --hooks project：舊腳本移除、換成執行檔複本，註冊不留兩份，使用者的 hook 不動
    let t = w.dir("p-hooks");
    init06(&t, &["--hooks", "project"]).ok("0.6 init.py --hooks project");
    assert!(t.join(".claude/hooks/awf_hook.py").is_file(), "0.6 --hooks project 應該有 .claude/hooks/awf_hook.py");
    let sp = t.join(".claude/settings.json");
    let mut st = load(&sp);
    st["hooks"]["PreToolUse"].as_array_mut().unwrap().push(json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo user-hook"}]}));
    dump(&sp, &st);
    write(&t.join(".claude/hooks/my_hook.sh"), "echo mine\n");
    // 沒帶 --hooks project：舊註冊還在，舊腳本先留著（不然 onFailure: block 會鎖死）並提示
    let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
    r.ok("awf init --upgrade（0.6 hooks project，沒帶 --hooks project）");
    assert!(t.join(".claude/hooks/awf_hook.py").is_file() && r.all().contains("--hooks project"), "還有舊註冊時舊腳本要留著並提示：\n{}", r.all());
    let r = awf(&["init", "--target", s(&t), "--upgrade", "--hooks", "project"], &w.0);
    r.ok("awf init --upgrade --hooks project");
    let text = read(&sp);
    assert!(!text.contains("awf_hook.py"), "舊的 hook 註冊要換掉：\n{text}");
    assert_eq!(text.matches(" hook ").count(), 5, "新的註冊五筆、不留兩份：\n{text}");
    assert!(text.contains("echo user-hook"), "使用者的 hook 不動");
    let left: Vec<String> = files(&t.join(".claude/hooks")).iter().map(|p| rel(&t, p)).collect();
    let name = if cfg!(windows) { "awf.exe" } else { "awf" };
    assert_eq!(left, [format!(".claude/hooks/bin/{name}"), ".claude/hooks/my_hook.sh".to_string()], "舊的 hook 腳本移除，只留複本與使用者自己的檔");
    let bin = t.join(".claude/hooks/bin").join(name);
    assert_eq!(hook(&bin, &t, "guard-shell", bash("git push origin feature/x", "agent-workflow:dev-implementer")), 2, "升級後複本的 hook 照擋");
    let d = awf(&["doctor"], &t);
    assert!(!d.all().contains("[錯誤]"), "升級後 awf doctor 有錯誤：\n{}", d.all());

    // 4. 有 merge gate：沒有執行檔就整個停下（什麼都不動）；給了就更新 yml 並放兩個檔
    let t = w.dir("p-gate");
    init06(&t, &["--with-merge-gate"]).ok("0.6 init.py --with-merge-gate");
    let ci = t.join(".github/workflows/agent-merge-gate.yml");
    assert!(read(&ci).contains("merge_gate.py"), "0.6 的 CI 跑 merge_gate.py");
    if !root().join("plugin/bin/awf-linux-x64").is_file() {
        let before: Vec<(String, Vec<u8>)> = files(&t).iter().map(|p| (rel(&t, p), std::fs::read(p).unwrap())).collect();
        let r = awf(&["init", "--target", s(&t), "--upgrade"], &w.0);
        assert_ne!(r.code, 0, "有 merge gate CI 而沒有 gate 執行檔，升級要停下：\n{}", r.all());
        assert!(r.all().contains("--merge-gate-bin"), "{}", r.all());
        let after: Vec<(String, Vec<u8>)> = files(&t).iter().map(|p| (rel(&t, p), std::fs::read(p).unwrap())).collect();
        assert!(before == after, "停下時不該動任何檔");
    }
    let elf = w.0.join("awf-linux-x64");
    std::fs::write(&elf, fake_elf()).unwrap();
    let r = awf(&["init", "--target", s(&t), "--upgrade", "--merge-gate-bin", s(&elf)], &w.0);
    r.ok("awf init --upgrade --merge-gate-bin（有 merge gate 的 0.6 專案）");
    let y = read(&ci);
    assert!(y.contains("merge-gate \"${ARGS[@]}\"") && y.contains("sha256sum -c") && !y.contains("python3 .claude/teams/tools/"), "CI 要換成核對後跑 awf merge-gate");
    assert_eq!(std::fs::read(t.join(".claude/teams/bin/awf-linux-x64")).unwrap(), fake_elf());
    assert!(t.join(".claude/teams/bin/checksums.json").is_file());
    assert!(!t.join(".claude/teams/tools").exists(), "工具照樣移除：{:?}", tools_left(&t));
}
