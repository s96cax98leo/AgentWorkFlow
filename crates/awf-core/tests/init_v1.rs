//! `awf init` 1.0 的行為（回歸基準 `init_parity.rs` 之外，逐點寫明該成功與該拒絕的情況）：
//!
//! - A 工具不複製進專案；覆寫檔的片段先找專案自己的、再找模版的；`--python` 忽略。
//! - B `--hooks project`：複製執行檔、settings 寫它的絕對路徑；0.x 的註冊與權限換成新的，使用者自己的不動。
//! - C `--upgrade`：0.x 的工具副本指紋相符的移除、改過的保留；用 git 歷史裡真正的 0.6 內容測。
//! - D `--with-merge-gate`：放 linux-x64 執行檔與 checksums.json；缺了或不對就拒絕。
//!
//! C 要從 git 歷史取 0.6 的內容（`plugin/legacy-tools.json` 的 `revs`）；歷史不在（例如淺層 clone）時那幾個測試印一行說明後略過。
//! 重新產生指紋檔：`AWF_GEN_LEGACY=1 cargo test -p awf-core --test init_v1 legacy_tools_json`。
#![cfg(unix)]

use awf_core::init::{self, legacy_tools};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo 根目錄")
}

fn tmp(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let d = std::env::temp_dir().join(format!("init-v1-{tag}-{}-{nanos}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&d).expect("建暫存目錄");
    d.canonicalize().expect("暫存目錄")
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("建目錄");
        for e in std::fs::read_dir(src).expect("讀目錄").flatten() {
            let n = e.file_name();
            if n == "__pycache__" || n == ".DS_Store" {
                continue;
            }
            copy_tree(&src.join(&n), &dst.join(&n));
        }
    } else if src.is_file() {
        std::fs::create_dir_all(dst.parent().expect("上層目錄")).expect("建目錄");
        std::fs::copy(src, dst).unwrap_or_else(|e| panic!("複製不了 {}：{e}", src.display()));
    }
}

/// 模版的複本：只有 plugin 目錄的內容（和從 marketplace 安裝的一樣，旁邊沒有 repo 的其他東西）。
/// 沒有 `bin/awf-linux-x64`：和還沒發佈的 repo 一樣。
fn template_copy(dst: &Path) {
    for rel in ["scaffold", ".claude-plugin", "workflow-templates", "legacy-tools.json", "workflow.config.example"] {
        copy_tree(&repo().join("plugin").join(rel), &dst.join(rel));
    }
}

/// 所有測試共用的唯讀模版。
fn template() -> &'static Path {
    static T: OnceLock<PathBuf> = OnceLock::new();
    T.get_or_init(|| {
        let d = tmp("tpl");
        template_copy(&d);
        d
    })
}

fn fake_elf() -> Vec<u8> {
    let mut v = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 62, 0];
    v.extend_from_slice(b"\nfake awf linux-x64\n");
    v
}

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// 假的 awf 執行檔（`--hooks project` 要複製的那一支）。
fn fake_awf(dir: &Path) -> PathBuf {
    let p = dir.join("fake-awf");
    std::fs::write(&p, "#!/bin/sh\n# 假的 awf\n").expect("寫假執行檔");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("改權限");
    p
}

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn run_from(source: &Path, cwd: &Path, awf_bin: Option<PathBuf>, args: &[&str]) -> Out {
    let env = init::Env { source: Some(source.to_path_buf()), cwd: cwd.to_path_buf(), home: None, awf_bin };
    let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (mut out, mut err, mut input) = (Vec::new(), Vec::new(), std::io::Cursor::new(Vec::new()));
    let code = init::run(&argv, &env, &mut out, &mut err, &mut input, false);
    Out { code, out: String::from_utf8_lossy(&out).into_owned(), err: String::from_utf8_lossy(&err).into_owned() }
}

fn run(cwd: &Path, awf_bin: Option<PathBuf>, args: &[&str]) -> Out {
    run_from(template(), cwd, awf_bin, args)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))
}

fn json_of(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{} 不是 JSON：{e}", p.display()))
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("建目錄");
    std::fs::write(p, text).unwrap_or_else(|e| panic!("寫不進 {}：{e}", p.display()));
}

fn files_under(root: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push(p.strip_prefix(base).expect("相對路徑").to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn is_exec(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// 每個檔的內容（用來確認「完全沒被動到」）。
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    files_under(root).into_iter().map(|rel| {
        let data = std::fs::read(root.join(&rel)).unwrap_or_default();
        (rel, data)
    }).collect()
}

// ------------------------------------------------------------------ A：工具不複製進專案

#[test]
fn a_fresh_init_copies_settings_but_no_tools() {
    let d = tmp("a1");
    let r = run(&d, None, &["--target", "p", "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let p = d.join("p");
    let files = files_under(&p);
    assert!(!p.join(".claude/teams/tools").exists(), "不應該有 .claude/teams/tools/");
    assert!(!p.join(".claude/hooks").exists(), "--hooks plugin 不應該有 .claude/hooks/");
    let py: Vec<&String> = files.iter().filter(|f| f.ends_with(".py") || f.ends_with(".mjs") || f.ends_with(".pyc")).collect();
    assert!(py.is_empty(), "專案裡不應該有 Python 或 node 的工具檔：{py:?}");
    for must in [
        "CLAUDE.md", ".claude/settings.json", ".claude/settings.README.md", ".mcp.json.example",
        ".claude/teams/VERSION", ".claude/teams/workflow.config", ".claude/teams/guard.config.json", ".claude/teams/methodology.json",
        ".claude/teams/rules.md", ".claude/teams/README.md", ".claude/teams/schemas/envelope.schema.json", ".claude/teams/schemas/methodology.schema.json",
        ".claude/teams/methodologies/default.json", ".claude/teams/deliverables/catalog.json", ".claude/teams/golden-set/token-budget.json",
        "_tpm/runs/README.md",
    ] {
        assert!(p.join(must).is_file(), "缺 {must}");
    }
    assert!(files.iter().any(|f| f.starts_with(".claude/teams/golden-set/valid/")), "golden-set 的案例要在");
    let open = "{".repeat(2);
    for f in &files {
        if let Ok(t) = std::fs::read_to_string(p.join(f)) {
            assert!(!t.contains(&open), "{f} 還有沒替換的佔位符");
        }
    }
    // 訊息講的是 awf 子指令，不是 Python 工具
    for old in ["init.py", "awf.py", "build.py", "python3", "awf_hook.py", "check_workflows.mjs"] {
        assert!(!r.out.contains(old), "訊息裡還有 {old}：\n{}", r.out);
    }
    for new in ["awf doctor", "awf render --all", "awf check-agents", "awf hook guard-shell"] {
        assert!(r.out.contains(new), "訊息裡沒有 {new}：\n{}", r.out);
    }
    // settings：只有權限，寫的是 awf 子指令
    let st = json_of(&p.join(".claude/settings.json"));
    assert!(st.get("hooks").is_none(), "--hooks plugin 的 settings 不應該註冊 hook");
    let allow = st["permissions"]["allow"].to_string();
    assert!(allow.contains("Bash(awf build:*)") && allow.contains("Bash(awf save-run:*)"), "{allow}");
    assert!(!allow.contains(".claude/teams/tools"), "權限不該再指到專案裡的工具：{allow}");
    assert!(read(&p.join(".claude/teams/workflow.config")).starts_with("# awf init 這次用的佔位符值"));
    let _ = std::fs::remove_dir_all(&d);
}

/// CLAUDE.md 的標記：新產生的寫「由 awf init 從模版產生」；0.x 寫的「由 init.py 從模版產生」照樣認成 init 自己產生的。
#[test]
fn a_claude_md_mark_new_and_0x_are_both_recognized() {
    use awf_core::init::{is_generated_claude_md, CLAUDE_MARK, CLAUDE_MARK_0X};
    let d = tmp("a-mark");
    let r = run(&d, None, &["--target", "p", "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let md = d.join("p/CLAUDE.md");
    let fresh = read(&md);
    assert!(fresh.starts_with("<!-- agent-workflow：由 awf init 從模版產生（"), "{}", fresh.lines().next().unwrap_or_default());
    assert!(!fresh.contains("init.py"), "新產生的 CLAUDE.md 不該再提 init.py");
    assert!(is_generated_claude_md(&fresh) && is_generated_claude_md(&format!("{CLAUDE_MARK_0X}（…） -->\n# x\n")));
    assert!(!is_generated_claude_md("# 使用者自己的 CLAUDE.md\n") && CLAUDE_MARK != CLAUDE_MARK_0X);

    // 兩種標記的既有檔都走「init 自己產生的」那條路：不加 --force 原樣略過、不另寫引用檔；--force 更新標記之前的內容並保留〈專案特定〉
    for (name, mark) in [("old", CLAUDE_MARK_0X), ("new", CLAUDE_MARK)] {
        let p = d.join(name);
        let body = format!("{mark}（舊的說明） -->\n# 舊標題\n\n舊內容\n\n## 專案特定\n\n我自己加的規則\n");
        write(&p.join("CLAUDE.md"), &body);
        let r = run(&d, None, &["--target", name, "--defaults"]);
        assert_eq!(r.code, 0, "{name}：{}", r.err);
        assert_eq!(read(&p.join("CLAUDE.md")), body, "{name}：沒有 --force 不該動");
        assert!(!p.join(".claude/teams/agent-workflow.CLAUDE.md").exists(), "{name}：被當成使用者的檔了");
        let r = run(&d, None, &["--target", name, "--defaults", "--force"]);
        assert_eq!(r.code, 0, "{name}：{}", r.err);
        let now = read(&p.join("CLAUDE.md"));
        assert!(now.starts_with(CLAUDE_MARK) && !now.contains("舊內容") && now.contains("我自己加的規則"), "{name}：{now}");
        assert!(!p.join(".claude/teams/agent-workflow.CLAUDE.md").exists(), "{name}：被當成使用者的檔了");
    }
    // 沒有標記的是使用者的檔：不覆蓋，改寫引用檔
    let u = d.join("user");
    write(&u.join("CLAUDE.md"), "# 我的專案\n");
    let r = run(&d, None, &["--target", "user", "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(read(&u.join("CLAUDE.md")).starts_with("# 我的專案\n") && u.join(".claude/teams/agent-workflow.CLAUDE.md").is_file());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_python_option_is_ignored_with_a_note() {
    let d = tmp("a2");
    // 給了不存在的路徑也不報錯（0.x 會拒絕）
    let r = run(&d, Some(fake_awf(&d)), &["--target", "p", "--defaults", "--hooks", "project", "--python", "/nope/python3"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("--python 是 0.x 的選項；1.0 不再需要 Python，已忽略"), "{}", r.out);
    assert!(!read(&d.join("p/.claude/settings.json")).contains("/nope/python3"));
    // 沒給就沒有這一行
    let r = run(&d, None, &["--target", "q", "--defaults"]);
    assert!(r.code == 0 && !r.out.contains("--python"), "{}", r.out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_snippets_come_from_the_project_first_then_the_template() {
    use awf_core::build::{self, Paths};
    let d = tmp("a3");
    let r = run(&d, None, &["--target", "p", "--defaults", "--with-workflows"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    let p = d.join("p");
    let tpl_snips = awf_core::template::snippets(template());
    assert!(tpl_snips.join("kernel.js").is_file(), "模版的片段目錄不對：{}", tpl_snips.display());
    assert_eq!(tpl_snips, template().join("workflow-templates/snippets"));
    // 1.0 的專案沒有自己的片段：覆寫檔內嵌的是模版的
    assert!(!p.join(".claude/teams/tools").exists());
    let wf = p.join(".claude/workflows/sdlc-plan.js");
    let ver = json_of(&repo().join("plugin/.claude-plugin/plugin.json"))["version"].as_str().expect("version").to_string();
    assert!(read(&wf).ends_with(&format!("// awf-override: {ver}\n")), "覆寫檔結尾要有標記");
    assert_eq!(files_under(&p.join(".claude/workflows")).len(), 12);
    let with_tpl = Paths::project_with(&p, Some(tpl_snips.clone()));
    let (code, out, err) = build::run_captured(&["--check".to_string()], &with_tpl);
    assert_eq!(code, 0, "有模版的片段：--check 要過\n{out}{err}");
    // 只找專案自己的（沒有）：說片段檔不存在
    let (code, out, _) = build::run_captured(&["--check".to_string()], &Paths::project(&p));
    assert!(code == 1 && out.contains("片段檔不存在") && out.contains(".claude/teams/tools/snippets"), "{out}");
    // 舊專案還留著自己的片段：以專案的為準（只有那一個片段；其他的照樣用模版的）
    let kernel = read(&tpl_snips.join("kernel.js"));
    write(&p.join(".claude/teams/tools/snippets/kernel.js"), &format!("{kernel}\n// 專案自己改過的片段\n"));
    let (code, out, _) = build::run_captured(&["--check".to_string()], &with_tpl);
    assert!(code == 1 && out.contains("不一致"), "專案的片段不同：--check 要指出來\n{out}");
    let (code, out, err) = build::run_captured(&[], &with_tpl);
    assert_eq!(code, 0, "{out}{err}");
    let text = read(&wf);
    assert!(text.contains("// 專案自己改過的片段"), "要內嵌專案的 kernel 片段");
    assert!(text.contains("/*@snippet:recipe*/"), "其他片段照樣內嵌");
    // 兩個目錄的先後：第一個有的為準，沒有才往後找
    let mut cache = build::SnippetCache::new();
    let own = p.join(".claude/teams/tools/snippets");
    let got = build::load_snippet_from("kernel", &[&own, &tpl_snips], &mut cache).expect("kernel");
    assert!(got.contains("專案自己改過的片段"));
    let got = build::load_snippet_from("recipe", &[&own, &tpl_snips], &mut cache).expect("recipe");
    assert_eq!(got, read(&tpl_snips.join("recipe.js")).replace("\r\n", "\n").trim_matches('\n'));
    let miss = build::load_snippet_from("nope", &[&own, &tpl_snips], &mut build::SnippetCache::new());
    assert!(matches!(miss, Err(build::BuildError::Snippet(m)) if m.contains(&own.join("nope.js").display().to_string())));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_rerun_without_upgrade_only_adds() {
    let d = tmp("a4");
    let p = d.join("p");
    assert_eq!(run(&d, None, &["--target", "p", "--defaults"]).code, 0);
    // 使用者改了模版檔、加了自己的檔；專案裡還有 0.x 的工具
    write(&p.join(".claude/teams/rules.md"), "我改過的 rules\n");
    write(&p.join(".claude/teams/tools/awf.py"), "# 0.x 的工具\n");
    write(&p.join(".claude/teams/tools/mine.py"), "# 我自己的\n");
    std::fs::remove_file(p.join(".claude/teams/schemas/envelope.schema.json")).expect("刪");
    let before = snapshot(&p);
    let r = run(&d, None, &["--target", "p", "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let after = snapshot(&p);
    let changed: Vec<&String> = after.iter().filter(|(rel, data)| before.iter().find(|(r0, _)| r0 == rel).is_some_and(|(_, d0)| d0 != data)).map(|(r, _)| r).collect();
    assert!(changed.is_empty(), "沒帶 --upgrade 不該改既有的檔：{changed:?}");
    let added: Vec<&String> = after.iter().filter(|(rel, _)| !before.iter().any(|(r0, _)| r0 == rel)).map(|(r, _)| r).collect();
    assert_eq!(added, vec![".claude/teams/schemas/envelope.schema.json"], "只補缺的檔");
    assert!(r.out.contains(".claude/teams/tools/ 還有 2 個 0.x 複製進來的工具檔") && r.out.contains("awf init --upgrade"), "{}", r.out);
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ B：hook 註冊

fn hook_commands(settings: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for entries in settings["hooks"].as_object().into_iter().flat_map(|o| o.values()) {
        for e in entries.as_array().into_iter().flatten() {
            for h in e["hooks"].as_array().into_iter().flatten() {
                out.push(h["command"].as_str().unwrap_or_default().to_string());
            }
        }
    }
    out
}

#[test]
fn b_hooks_project_copies_the_binary_and_registers_its_absolute_path() {
    let d = tmp("b1");
    let awf = fake_awf(&d);
    let r = run(&d, Some(awf.clone()), &["--target", "p", "--defaults", "--hooks", "project"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let p = d.join("p");
    let bin = p.join(".claude/hooks/bin/awf");
    assert_eq!(std::fs::read(&bin).ok(), std::fs::read(&awf).ok(), "複本的內容要和來源相同");
    assert!(is_exec(&bin), "複本要能執行");
    assert_eq!(files_under(&p.join(".claude/hooks")), vec!["bin/awf"], "不再複製 plugin/hooks 的 .py");
    let st = json_of(&p.join(".claude/settings.json"));
    let cmds = hook_commands(&st);
    let want = |kind: &str| format!("\"{}\" hook {kind}", bin.display());
    assert_eq!(cmds, vec![want("guard-shell"), want("guard-paths"), want("envelope"), want("envelope"), want("envelope"), want("mark-dirty"), want("prompt-context")]);
    // 輔助 hook（待更新清單、提問時帶位置）：失敗放行，不像守門那樣 block
    assert_eq!(st["hooks"]["PostToolUse"][0]["matcher"], "Edit|Write");
    assert_eq!(st["hooks"]["PostToolUse"][0]["hooks"][0]["onFailure"], "continue");
    assert_eq!(st["hooks"]["UserPromptSubmit"][0]["hooks"][0]["onFailure"], "continue");
    let pre = st["hooks"]["PreToolUse"].as_array().expect("PreToolUse");
    assert_eq!(pre[0]["matcher"], "Bash|PowerShell");
    assert_eq!(pre[0]["hooks"][0]["onFailure"], "block");
    assert_eq!(pre[1]["hooks"][0]["onFailure"], "block");
    assert_eq!(pre[0]["hooks"][0]["timeout"], 10);
    // doctor 認新寫法用的正規式（doctor.rs 的 project_hook_re）要認得
    let doctor_rx = regex::Regex::new(r#"awf_hook\.py|awf(?:\.exe)?[\\"']*\s+hook\s+(?:guard-shell|guard-paths|envelope|session-start)\b"#).expect("正規式");
    assert!(doctor_rx.is_match(&serde_json::to_string(&st["hooks"]).expect("JSON")));
    assert_eq!(init::hook_forms(&serde_json::to_string(&st["hooks"]).expect("JSON")), (false, true));
    // 沒有 .gitignore：提示（不改 .gitignore）
    assert!(r.out.contains("不建議進版控") && r.out.contains("awf init --target <專案> --upgrade --hooks project"), "{}", r.out);
    assert!(!p.join(".gitignore").exists(), "init 不建立 .gitignore");
    // 重跑（--force）：不重複註冊，執行檔換成新的
    std::fs::write(&awf, "#!/bin/sh\n# 新版的假 awf\n").expect("寫");
    let r = run(&d, Some(awf.clone()), &["--target", "p", "--force", "--hooks", "project"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(hook_commands(&json_of(&p.join(".claude/settings.json"))), cmds);
    assert_eq!(read(&bin), "#!/bin/sh\n# 新版的假 awf\n");
    assert!(is_exec(&bin));
    // 沒帶 --force 的重跑：已存在的執行檔不換
    std::fs::write(&awf, "#!/bin/sh\n# 更新的\n").expect("寫");
    let r = run(&d, Some(awf), &["--target", "p", "--defaults", "--hooks", "project"]);
    assert!(r.code == 0 && r.out.contains(".claude/hooks/bin/awf"), "{}", r.out);
    assert_eq!(read(&bin), "#!/bin/sh\n# 新版的假 awf\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn b_gitignore_hint_only_when_not_ignored() {
    for (ignore, hint) in [
        (".claude/hooks/bin/\n", false),
        ("node_modules\n/.claude/hooks/bin\n", false),
        (".claude/hooks/\n", false),
        ("**/.claude/hooks/bin/**\n", false),
        ("# .claude/hooks/bin/\nnode_modules/\n", true),
        (".claude/hooks/bin/\n!.claude/hooks/bin/\n", true),
        (".claude/hooks/binx/\n", true),
    ] {
        let d = tmp("b2");
        write(&d.join("p/.gitignore"), ignore);
        let r = run(&d, Some(fake_awf(&d)), &["--target", "p", "--defaults", "--hooks", "project"]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(r.out.contains("不建議進版控"), hint, ".gitignore={ignore:?}\n{}", r.out);
        assert_eq!(read(&d.join("p/.gitignore")), ignore, "init 不改 .gitignore");
        let _ = std::fs::remove_dir_all(&d);
    }
    // --hooks plugin 沒有執行檔，也就沒有提示
    let d = tmp("b2p");
    let r = run(&d, None, &["--target", "p", "--defaults"]);
    assert!(r.code == 0 && !r.out.contains("不建議進版控"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn b_hooks_project_refuses_without_a_binary() {
    let d = tmp("b3");
    for bin in [None, Some(d.join("nope/awf")), Some(d.clone())] {
        let r = run(&d, bin, &["--target", "p", "--defaults", "--hooks", "project"]);
        assert_eq!(r.code, 1);
        assert!(r.err.contains("--hooks project 要把 awf 執行檔複製進專案") && r.err.contains("AWF_BIN"), "{}", r.err);
        assert!(!d.join("p").exists(), "找不到執行檔時不該動專案");
    }
    write(&d.join("empty"), "");
    let r = run(&d, Some(d.join("empty")), &["--target", "p", "--defaults", "--hooks", "project"]);
    assert!(r.code == 1 && r.err.contains("是空檔"), "{}", r.err);
    // --hooks plugin 用不到執行檔
    assert_eq!(run(&d, None, &["--target", "p", "--defaults"]).code, 0);
    let _ = std::fs::remove_dir_all(&d);
}

const OLD_PY: &str = "\"/usr/bin/python3\" \"${CLAUDE_PROJECT_DIR}/.claude/hooks/awf_hook.py\"";

/// 0.6 的 `--hooks project` 寫出的 settings.json，加上使用者自己的 hook 與權限。
fn old_project_settings() -> Value {
    json!({
        "$schema": "https://json.schemastore.org/claude-code-settings.json",
        "model": "sonnet",
        "permissions": {
            "allow": [
                "Bash(python3 .claude/teams/tools/build.py:*)",
                "Bash(python3 .claude/teams/tools/check_envelope.py:*)",
                "Bash(npm test:*)",
                "Bash(python3 .claude/teams/tools/check_rules.py:*)",
                "Bash(python3 .claude/teams/tools/check_agents.py:*)",
                "Bash(python3 .claude/teams/tools/save_run.py:*)",
                "Bash(node .claude/teams/tools/check_workflows.mjs:*)",
                "Bash(python3 .claude/teams/tools/kb.py:*)",
                "Bash(python3 scripts/mine.py:*)",
                "Bash(git status:*)"
            ],
            "deny": ["Read(./.env)", "Read(./secrets/**)"]
        },
        "hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo 使用者的 hook"}]},
                {"matcher": "Bash|PowerShell", "hooks": [{"type": "command", "command": format!("{OLD_PY} guard-shell"), "timeout": 10, "onFailure": "block"}]},
                {"matcher": "Edit|Write|NotebookEdit|Read|Grep", "hooks": [
                    {"type": "command", "command": format!("{OLD_PY} guard-paths"), "timeout": 10, "onFailure": "block"},
                    {"type": "command", "command": "my-awf hook guard-paths"}
                ]},
                {"matcher": "SubagentHandback|StructuredOutput", "hooks": [{"type": "command", "command": format!("{OLD_PY} envelope"), "onFailure": "continue"}]}
            ],
            "SubagentStop": [{"hooks": [{"type": "command", "command": format!("{OLD_PY} envelope")}]}],
            "TaskCompleted": [{"hooks": [{"type": "command", "command": format!("{OLD_PY} envelope")}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}]
        }
    })
}

#[test]
fn b_upgrade_replaces_old_registration_and_allow_but_not_the_users() {
    let d = tmp("b4");
    let p = d.join("p");
    assert_eq!(run(&d, None, &["--target", "p", "--defaults"]).code, 0);
    write(&p.join(".claude/settings.json"), &serde_json::to_string_pretty(&old_project_settings()).expect("JSON"));
    let awf = fake_awf(&d);
    let r = run(&d, Some(awf.clone()), &["--target", "p", "--upgrade", "--hooks", "project"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    let st = json_of(&p.join(".claude/settings.json"));
    let cmds = hook_commands(&st);
    let bin = p.join(".claude/hooks/bin/awf");
    let ours: Vec<&String> = cmds.iter().filter(|c| c.contains(&bin.display().to_string())).collect();
    assert_eq!(ours.len(), 7, "新的註冊七筆：{cmds:?}");
    assert!(!cmds.iter().any(|c| c.contains("awf_hook.py")), "舊的註冊要換掉，不留兩份：{cmds:?}");
    for mine in ["echo 使用者的 hook", "my-awf hook guard-paths", "echo done"] {
        assert_eq!(cmds.iter().filter(|c| c.as_str() == mine).count(), 1, "使用者的 hook「{mine}」要原樣留著：{cmds:?}");
    }
    assert_eq!(st["hooks"]["PreToolUse"][0], json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "echo 使用者的 hook"}]}));
    assert_eq!(st["hooks"]["Stop"], json!([{"hooks": [{"type": "command", "command": "echo done"}]}]));
    assert_eq!(st["model"], "sonnet");
    // 權限：我們放的五筆換掉、check_workflows.mjs 那筆拿掉；使用者自己的（含他自己加的 kb.py）原樣、順序不變
    assert_eq!(st["permissions"]["allow"], json!([
        "Bash(awf build:*)",
        "Bash(awf check-envelope:*)",
        "Bash(npm test:*)",
        "Bash(awf check-rules:*)",
        "Bash(awf check-agents:*)",
        "Bash(awf save-run:*)",
        "Bash(python3 .claude/teams/tools/kb.py:*)",
        "Bash(python3 scripts/mine.py:*)",
        "Bash(git status:*)"
    ]));
    assert_eq!(st["permissions"]["deny"], json!(["Read(./.env)", "Read(./secrets/**)"]), "使用者的 deny 不動（模版的 deny 不硬塞進去）");
    assert!(r.out.contains("6 筆 0.x 的寫法換成 1.0 的") && r.out.contains("Bash(python3 .claude/teams/tools/save_run.py:*) → Bash(awf save-run:*)"), "{}", r.out);
    // 再升級一次：不變
    let before = read(&p.join(".claude/settings.json"));
    let r = run(&d, Some(awf), &["--target", "p", "--upgrade", "--hooks", "project"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(read(&p.join(".claude/settings.json")), before);
    assert!(!r.out.contains("0.x 的寫法換成"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn b_plain_rerun_does_not_touch_allow_or_tools() {
    let d = tmp("b5");
    let p = d.join("p");
    assert_eq!(run(&d, None, &["--target", "p", "--defaults"]).code, 0);
    let mut old = old_project_settings();
    old.as_object_mut().expect("物件").remove("hooks");
    write(&p.join(".claude/settings.json"), &serde_json::to_string_pretty(&old).expect("JSON"));
    let before = read(&p.join(".claude/settings.json"));
    let r = run(&d, None, &["--target", "p", "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(read(&p.join(".claude/settings.json")), before, "沒帶 --upgrade 不換權限");
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ C：升級 0.6 的專案

fn git_ok(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").current_dir(repo()).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// 指紋檔記的 rev（最後一個 0.x 版本）；git 歷史裡沒有就回 None（測試略過）。
fn legacy_rev() -> Option<String> {
    let doc = json_of(&repo().join(legacy_tools::REPO_FILE));
    let rev = doc["revs"].as_array()?.first()?.as_str()?.to_string();
    git_ok(&["cat-file", "-e", &format!("{rev}^{{commit}}")])?;
    Some(rev)
}

fn skip(what: &str) {
    eprintln!("略過「{what}」：git 歷史裡沒有 plugin/legacy-tools.json 記的 commit（淺層 clone？）");
}

/// 把 `rev` 那一版模版裡的一個檔放進專案（0.x 的 init 對 scaffold 的文字檔會換佔位符）。
fn lay(rev: &str, repo_path: &str, dst: &Path, runs: &str) {
    let data = git_ok(&["show", &format!("{rev}:{repo_path}")]).unwrap_or_else(|| panic!("git show {rev}:{repo_path} 失敗"));
    let open = "{".repeat(2);
    let close = "}".repeat(2);
    let data = match String::from_utf8(data) {
        Ok(t) if repo_path.starts_with("scaffold/") => t.replace(&format!("{open}RUNS_DIR{close}"), runs).into_bytes(),
        Ok(t) => t.into_bytes(),
        Err(e) => e.into_bytes(),
    };
    std::fs::create_dir_all(dst.parent().expect("上層目錄")).expect("建目錄");
    std::fs::write(dst, data).expect("寫檔");
}

/// 還原出 0.6 的專案：先用這版 init 出設定，再把 0.6 的工具（與 `--hooks project` 的腳本、settings）照 0.6 的 init 放的位置放進去。
/// 回傳放進去的檔（相對專案）。
fn make_06_project(rev: &str, p: &Path, hooks_project: bool) -> Vec<String> {
    let d = p.parent().expect("上層目錄");
    let name = p.file_name().expect("目錄名").to_string_lossy().into_owned();
    assert_eq!(run(d, None, &["--target", &name, "--defaults"]).code, 0);
    let listing = String::from_utf8(git_ok(&["ls-tree", "-r", "--name-only", rev, "--", "scaffold/.claude/teams/tools", "plugin/hooks", "scaffold/.claude/teams/golden-set/run.py"]).expect("ls-tree")).expect("UTF-8");
    let mut laid = Vec::new();
    for path in listing.lines() {
        let Some((rel, _)) = legacy_tools::project_path(path) else { continue };
        if rel.starts_with(".claude/hooks/") && !hooks_project {
            continue;
        }
        lay(rev, path, &p.join(&rel), "_tpm/runs");
        laid.push(rel);
    }
    write(&p.join(".claude/teams/VERSION"), "0.6.0\n");
    let mut st = old_project_settings();
    if !hooks_project {
        st.as_object_mut().expect("物件").remove("hooks");
    }
    write(&p.join(".claude/settings.json"), &serde_json::to_string_pretty(&st).expect("JSON"));
    // 進行中的 run
    write(&p.join("_tpm/runs/2026-11-01-demo/run.json"), "{\"stages\": {\"plan\": {\"status\": \"done\"}}, \"awf\": {\"pending\": \"design\"}}\n");
    write(&p.join("_tpm/runs/2026-11-01-demo/.wf/sdlc-design.js"), "// 產生的腳本\n");
    write(&p.join("_tpm/runs/2026-11-01-demo/plan/pack.md"), "# 資料包\n");
    write(&p.join("_tpm/runs/decisions.jsonl"), "{\"q\": \"示範客戶 ACME 的決定\"}\n");
    laid.sort();
    laid
}

#[test]
fn c_upgrade_removes_a_pristine_06_copy() {
    let Some(rev) = legacy_rev() else { return skip("原樣的 0.6 專案") };
    let d = tmp("c1");
    let p = d.join("p");
    let laid = make_06_project(&rev, &p, false);
    assert!(laid.len() > 40 && laid.contains(&".claude/teams/tools/save_run.py".to_string()) && laid.contains(&".claude/teams/tools/snippets/kernel.js".to_string()), "0.6 的工具檔：{}", laid.len());
    // Python 跑過會留下快取
    write(&p.join(".claude/teams/tools/__pycache__/awf.cpython-312.pyc"), "pyc");
    let runs_before = snapshot(&p.join("_tpm/runs/2026-11-01-demo"));
    let decisions = read(&p.join("_tpm/runs/decisions.jsonl"));
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    assert!(r.out.contains("== 升級 agent-workflow：0.6.0 → "), "{}", r.out);
    assert!(r.out.contains(&format!("移除 {} 個 0.x 複製進專案、沒改過的工具檔", laid.len())), "{}", r.out);
    assert!(!p.join(".claude/teams/tools").exists(), "原樣的工具全部移除後，tools/ 目錄也要移除：{:?}", files_under(&p.join(".claude/teams/tools")));
    assert!(!p.join(".claude/teams/golden-set/run.py").exists());
    assert!(!r.out.contains("改過的舊工具檔") && !r.out.contains("不是模版放的檔"), "{}", r.out);
    // 進行中的 run 完全不碰
    assert_eq!(snapshot(&p.join("_tpm/runs/2026-11-01-demo")), runs_before);
    assert_eq!(read(&p.join("_tpm/runs/decisions.jsonl")), decisions);
    // 權限換成新的
    assert!(read(&p.join(".claude/settings.json")).contains("Bash(awf build:*)"));
    let ver = json_of(&repo().join("plugin/.claude-plugin/plugin.json"))["version"].as_str().expect("version").to_string();
    assert_eq!(read(&p.join(".claude/teams/VERSION")), format!("{ver}\n"));
    // 再升級一次：沒有東西可移除
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert!(r.code == 0 && !r.out.contains("0.x 複製進專案"), "{}", r.out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c_upgrade_keeps_modified_tools_and_the_users_own_files() {
    let Some(rev) = legacy_rev() else { return skip("改過工具的 0.6 專案") };
    let d = tmp("c2");
    let p = d.join("p");
    let laid = make_06_project(&rev, &p, false);
    let tools = p.join(".claude/teams/tools");
    // 改過的：一支工具、一個片段、一個情境檔；使用者自己加的兩個檔；一個換成 CRLF 的（內容沒改，算原樣）
    let edited = [".claude/teams/tools/save_run.py", ".claude/teams/tools/snippets/kernel.js", ".claude/teams/tools/scenarios/sdlc-build.mjs"];
    for rel in edited {
        let t = read(&p.join(rel));
        write(&p.join(rel), &format!("{t}\n// 我改過\n"));
    }
    write(&tools.join("my_tool.py"), "# 示範客戶 ACME 自己的工具\n");
    write(&tools.join("scenarios/mine.mjs"), "// 自己的情境\n");
    let crlf = read(&tools.join("checklist.py")).replace('\n', "\r\n");
    write(&tools.join("checklist.py"), &crlf);
    let keep: Vec<(String, String)> = edited.iter().map(|r| r.to_string()).chain([".claude/teams/tools/my_tool.py".to_string(), ".claude/teams/tools/scenarios/mine.mjs".to_string()])
        .map(|rel| (rel.clone(), read(&p.join(&rel)))).collect();
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    assert!(r.out.contains(&format!("移除 {} 個 0.x 複製進專案、沒改過的工具檔", laid.len() - 3)), "{}", r.out);
    assert!(r.out.contains("保留 3 個改過的舊工具檔") && r.out.contains("1.0 不再使用它們"), "{}", r.out);
    for rel in edited {
        assert!(r.out.contains(&format!("  - {rel}")), "改過的 {rel} 要逐一列出：\n{}", r.out);
    }
    assert!(r.out.contains(".claude/teams/tools/ 底下不是模版放的檔 2 個，沒有動") && r.out.contains("  - .claude/teams/tools/my_tool.py"), "{}", r.out);
    for (rel, body) in &keep {
        assert_eq!(&read(&p.join(rel)), body, "{rel} 不該被動到");
    }
    let mut left = files_under(&tools);
    left.sort();
    assert_eq!(left, vec!["my_tool.py", "save_run.py", "scenarios/mine.mjs", "scenarios/sdlc-build.mjs", "snippets/kernel.js"], "只剩改過的與使用者自己的");
    // 改過而保留的片段：專案的優先（覆寫檔內嵌時會用到它）
    assert!(read(&tools.join("snippets/kernel.js")).contains("我改過"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c_upgrade_of_a_hooks_project_install() {
    let Some(rev) = legacy_rev() else { return skip("--hooks project 的 0.6 專案") };
    let d = tmp("c3");
    let p = d.join("p");
    let laid = make_06_project(&rev, &p, true);
    let hooks = p.join(".claude/hooks");
    assert!(hooks.join("awf_hook.py").is_file() && hooks.join("guard.py").is_file() && hooks.join("tests/run.py").is_file());
    write(&hooks.join("my-hook.sh"), "#!/bin/sh\necho 使用者自己的 hook 腳本\n");
    write(&hooks.join("__pycache__/guard.cpython-312.pyc"), "pyc");
    let n_hooks = laid.iter().filter(|r| r.starts_with(".claude/hooks/")).count();
    assert!(n_hooks >= 7, "{n_hooks}");

    // 一、沒帶 --hooks project 就升級：settings 還註冊著 awf_hook.py，腳本還在用，不能移除（否則 onFailure: block 會把使用者鎖死）
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    assert!(r.out.contains("加 --hooks project 重跑升級") && r.out.contains("舊腳本先留著沒有移除"), "{}", r.out);
    assert!(hooks.join("awf_hook.py").is_file() && hooks.join("guard.py").is_file(), "還在用的 hook 腳本不能移除");
    assert!(read(&p.join(".claude/settings.json")).contains("awf_hook.py"));
    assert!(!p.join(".claude/teams/tools").exists(), "工具照樣整理");
    assert!(r.out.contains(&format!("移除 {} 個 0.x 複製進專案、沒改過的工具檔", laid.len() - n_hooks)), "{}", r.out);

    // 二、--upgrade --hooks project：註冊換成執行檔，舊腳本移除，使用者的腳本不動
    let awf = fake_awf(&d);
    let r = run(&d, Some(awf), &["--target", "p", "--upgrade", "--hooks", "project"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    let st = json_of(&p.join(".claude/settings.json"));
    let cmds = hook_commands(&st);
    assert!(!cmds.iter().any(|c| c.contains("awf_hook.py") || c.contains("python")), "{cmds:?}");
    assert_eq!(cmds.iter().filter(|c| c.contains("/.claude/hooks/bin/awf\" hook ")).count(), 7, "{cmds:?}");
    assert!(cmds.contains(&"echo 使用者的 hook".to_string()) && cmds.contains(&"my-awf hook guard-paths".to_string()) && cmds.contains(&"echo done".to_string()));
    assert_eq!(files_under(&hooks), vec!["bin/awf", "my-hook.sh"], "舊腳本與它的快取、空的 tests/ 都清掉，使用者的腳本與新的執行檔留著");
    assert!(r.out.contains(&format!("移除 {n_hooks} 個 0.x 複製進專案、沒改過的工具檔")), "{}", r.out);
    assert!(!r.out.contains("舊腳本先留著"), "{}", r.out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c_upgrade_of_a_project_with_merge_gate() {
    let Some(rev) = legacy_rev() else { return skip("有 merge gate 的 0.6 專案") };
    let d = tmp("c4");
    let p = d.join("p");
    make_06_project(&rev, &p, false);
    let yml = p.join(".github/workflows/agent-merge-gate.yml");
    lay(&rev, "scaffold/.github/workflows/agent-merge-gate.yml.tmpl", &yml, "_tpm/runs");
    assert!(read(&yml).contains("python3 .claude/teams/tools/merge_gate.py"));
    let before = snapshot(&p);

    // 一、模版沒有 linux-x64 執行檔：整個升級停下，專案完全沒動（不留「CI 還在叫已經被移除的 merge_gate.py」的半套）
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(r.err.contains("已經有 merge gate 的 CI") && r.err.contains("--merge-gate-bin"), "{}", r.err);
    assert_eq!(snapshot(&p), before, "停下時專案不該有任何改動");

    // 二、指定執行檔：CI 換成執行檔的版本，執行檔與 checksums 放進 .claude/teams/bin/，舊工具移除
    let elf = d.join("awf-linux-x64");
    std::fs::write(&elf, fake_elf()).expect("寫");
    let r = run(&d, None, &["--target", "p", "--upgrade", "--merge-gate-bin", "awf-linux-x64"]);
    assert_eq!(r.code, 0, "{}\n{}", r.out, r.err);
    assert!(r.out.contains("專案已經有 merge gate 的 CI：這次一起更新"), "{}", r.out);
    let text = read(&yml);
    assert!(!text.contains("python3 .claude/teams/tools/merge_gate.py") && text.contains("merge-gate \"${ARGS[@]}\""), "CI 要換成執行檔的版本");
    assert_eq!(std::fs::read(p.join(".claude/teams/bin/awf-linux-x64")).ok(), Some(fake_elf()));
    assert!(is_exec(&p.join(".claude/teams/bin/awf-linux-x64")));
    assert_eq!(json_of(&p.join(".claude/teams/bin/checksums.json"))["files"]["awf-linux-x64"], sha256(&fake_elf()));
    assert!(!p.join(".claude/teams/tools").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c_user_owned_gate_yml_is_not_taken_over() {
    let d = tmp("c5");
    let p = d.join("p");
    assert_eq!(run(&d, None, &["--target", "p", "--defaults"]).code, 0);
    // 同名、但看不出是模版產生的 CI：不當成「已經有 merge gate」，升級照常、檔案不動
    write(&p.join(".github/workflows/agent-merge-gate.yml"), "name: mine\non: [push]\n");
    let r = run(&d, None, &["--target", "p", "--upgrade"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(read(&p.join(".github/workflows/agent-merge-gate.yml")), "name: mine\non: [push]\n");
    assert!(!p.join(".claude/teams/bin").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c_fingerprints_cover_every_06_file() {
    let Some(rev) = legacy_rev() else { return skip("指紋對得上 0.6 的內容") };
    let index = legacy_tools::load_index(&repo().join("plugin")).expect("plugin/legacy-tools.json");
    let listing = String::from_utf8(git_ok(&["ls-tree", "-r", "--name-only", &rev, "--", "scaffold/.claude/teams/tools", "plugin/hooks", "scaffold/.claude/teams/golden-set/run.py"]).expect("ls-tree")).expect("UTF-8");
    let mut vals = init::Kv::default();
    vals.set("RUNS_DIR", "_acme/agent-runs");
    let (mut n, open, close) = (0, "{".repeat(2), "}".repeat(2));
    for path in listing.lines() {
        let Some((rel, render)) = legacy_tools::project_path(path) else {
            assert_eq!(path, "plugin/hooks/hooks.json", "只有 hooks.json 不是 init 複製的");
            continue;
        };
        let raw = git_ok(&["show", &format!("{rev}:{path}")]).expect("git show");
        // 落在專案裡的內容：scaffold 的檔換過佔位符
        let landed = match (render, String::from_utf8(raw.clone())) {
            (true, Ok(t)) => t.replace(&format!("{open}RUNS_DIR{close}"), "_acme/agent-runs").into_bytes(),
            _ => raw,
        };
        let prints = index.get(&rel).and_then(Value::as_array).unwrap_or_else(|| panic!("指紋檔沒有 {rel}"));
        assert!(prints.iter().any(|fp| legacy_tools::matches(&landed, fp, &vals)), "{rel} 的 0.6 內容對不上任何指紋");
        let mut edited = landed.clone();
        edited.extend_from_slice(b"\n# edited\n");
        assert!(!prints.iter().any(|fp| legacy_tools::matches(&edited, fp, &vals)), "{rel} 改過之後不該對得上");
        n += 1;
    }
    assert!(n >= 50, "0.6 的工具檔數：{n}");
}

#[test]
fn legacy_tools_json_matches_git_history() {
    let path = repo().join(legacy_tools::REPO_FILE);
    if std::env::var("AWF_GEN_LEGACY").is_ok_and(|v| !v.is_empty() && v != "0") {
        // 重新產生：AWF_GEN_LEGACY=1 用檔案裡記的 revs；其他值當成 rev（逗號分隔），例如 AWF_GEN_LEGACY=main
        let v = std::env::var("AWF_GEN_LEGACY").unwrap_or_default();
        let revs: Vec<String> = if v == "1" {
            json_of(&path)["revs"].as_array().expect("revs").iter().filter_map(|r| r.as_str().map(str::to_string)).collect()
        } else {
            v.split(',').map(str::to_string).collect()
        };
        let doc = legacy_tools::generate(&repo(), &revs).expect("產生指紋檔");
        std::fs::write(&path, legacy_tools::render(&doc)).expect("寫指紋檔");
        eprintln!("已重新產生 {}：{}", path.display(), doc["count"]);
        return;
    }
    let Some(_) = legacy_rev() else { return skip("指紋檔和 git 歷史一致") };
    let have = json_of(&path);
    let revs: Vec<String> = have["revs"].as_array().expect("revs").iter().filter_map(|r| r.as_str().map(str::to_string)).collect();
    let doc = legacy_tools::generate(&repo(), &revs).expect("產生指紋檔");
    assert_eq!(read(&path), legacy_tools::render(&doc), "plugin/legacy-tools.json 和 git 歷史產生的不同；用 AWF_GEN_LEGACY=1 重新產生");
    // 產生器的拒絕情況
    assert!(legacy_tools::generate(&repo(), &[]).is_err());
    assert!(legacy_tools::generate(&repo(), &["no-such-rev-acme".to_string()]).is_err_and(|e| e.contains("找不到 rev")));
    assert!(legacy_tools::generate(&std::env::temp_dir(), &revs).is_err(), "不是 repo 的目錄要回錯誤");
}

#[test]
fn c_broken_or_missing_index_removes_nothing() {
    let d = tmp("c6");
    let tpl = d.join("tpl");
    template_copy(&tpl);
    let p = d.join("p");
    assert_eq!(run_from(&tpl, &d, None, &["--target", "p", "--defaults"]).code, 0);
    write(&p.join(".claude/teams/tools/awf.py"), "# 0.x 的工具\n");
    for broken in [Some("{ 壞掉"), Some("{\"files\": []}"), None] {
        match broken {
            Some(t) => write(&tpl.join("legacy-tools.json"), t),
            None => std::fs::remove_file(tpl.join("legacy-tools.json")).expect("刪"),
        }
        let r = run_from(&tpl, &d, None, &["--target", "p", "--upgrade"]);
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(p.join(".claude/teams/tools/awf.py").is_file(), "認不出來就留著");
        assert!(r.out.contains("讀不到") && r.out.contains("legacy-tools.json") && r.out.contains("沒有整理"), "{}", r.out);
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ D：merge gate 的 CI

#[test]
fn d_merge_gate_needs_a_linux_x64_binary() {
    let d = tmp("d1");
    // 模版沒有 bin/awf-linux-x64（還沒發佈）：明確失敗，專案不動
    let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("--with-merge-gate 要把 linux-x64 的執行檔放進專案") && r.err.contains("bin/awf-linux-x64") && r.err.contains("--merge-gate-bin"), "{}", r.err);
    assert!(!d.join("p").exists(), "失敗時不該建立專案");
    // 指定的檔不存在、不是 ELF、是別的平台或架構的執行檔：拒絕
    write(&d.join("script"), "#!/bin/sh\necho hi\n");
    std::fs::write(d.join("macho"), [0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1, 3, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]).expect("寫");
    let mut arm = fake_elf();
    arm[18] = 183; // aarch64
    std::fs::write(d.join("arm64"), &arm).expect("寫");
    let mut elf32 = fake_elf();
    elf32[4] = 1;
    std::fs::write(d.join("elf32"), &elf32).expect("寫");
    std::fs::write(d.join("short"), b"\x7fELF").expect("寫");
    for (bin, why) in [("nope", "不存在"), ("script", "不是 linux x86-64 的執行檔"), ("macho", "不是 linux x86-64"), ("arm64", "不是 linux x86-64"), ("elf32", "不是 linux x86-64"), ("short", "不是 linux x86-64")] {
        let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate", "--merge-gate-bin", bin]);
        assert!(r.code == 1 && r.err.contains(why), "{bin}：{}", r.err);
        assert!(!d.join("p").exists());
    }
    assert!(init::is_elf_x86_64(&fake_elf()) && !init::is_elf_x86_64(&arm) && !init::is_elf_x86_64(b""));
    // 選項的搭配
    std::fs::write(d.join("good"), fake_elf()).expect("寫");
    let r = run(&d, None, &["--target", "p", "--defaults", "--merge-gate-bin", "good"]);
    assert!(r.code == 1 && r.err.contains("要搭配 --with-merge-gate"), "{}", r.err);
    write(&d.join("sums.json"), &format!("{{\"files\": {{\"awf-linux-x64\": \"{}\"}}, \"source\": \"示範\"}}\n", sha256(&fake_elf())));
    let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate", "--merge-gate-checksums", "sums.json"]);
    assert!(r.code == 1 && r.err.contains("要搭配 --merge-gate-bin"), "{}", r.err);
    // checksums 對不上、沒有這個檔的紀錄、不存在：拒絕
    write(&d.join("bad.json"), "{\"files\": {\"awf-linux-x64\": \"00ff\"}}\n");
    write(&d.join("none.json"), "{\"files\": {\"awf-darwin-arm64\": \"00ff\"}}\n");
    for (sums, why) in [("bad.json", "不符"), ("none.json", "讀不到 files.awf-linux-x64"), ("nope.json", "不存在")] {
        let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate", "--merge-gate-bin", "good", "--merge-gate-checksums", sums]);
        assert!(r.code == 1 && r.err.contains(why), "{sums}：{}", r.err);
        assert!(!d.join("p").exists());
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn d_merge_gate_places_binary_checksums_and_ci() {
    let d = tmp("d2");
    std::fs::write(d.join("good"), fake_elf()).expect("寫");
    let sum = sha256(&fake_elf());
    // 一、只給執行檔：checksums.json 依它產生
    let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate", "--merge-gate-bin", "good"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let p = d.join("p");
    assert_eq!(std::fs::read(p.join(".claude/teams/bin/awf-linux-x64")).ok(), Some(fake_elf()));
    assert!(is_exec(&p.join(".claude/teams/bin/awf-linux-x64")));
    assert!(!is_exec(&p.join(".claude/teams/bin/checksums.json")));
    assert_eq!(json_of(&p.join(".claude/teams/bin/checksums.json")), json!({"files": {"awf-linux-x64": sum}, "source": "unknown"}));
    assert!(r.out.contains("merge gate CI 已放在") && r.out.contains(".claude/teams/bin"), "{}", r.out);
    // CI：從受保護的 base 取出、核對 sha256、再執行 awf merge-gate；不跑 Python 工具
    let yml = read(&p.join(".github/workflows/agent-merge-gate.yml"));
    for need in [
        "pull_request_target", "merge_group", "ref: ${{ steps.pr.outputs.base_ref }}", "persist-credentials: false",
        ".claude/teams/bin/awf-linux-x64", ".claude/teams/bin/checksums.json", "sha256sum -c", "merge-gate \"${ARGS[@]}\"",
        "agent-merge-gate/decision", "permissions:\n  contents: read",
    ] {
        assert!(yml.contains(need), "CI 範本缺「{need}」");
    }
    assert!(!yml.contains("python3 .claude/teams/tools") && !yml.contains("tools/merge_gate.py \""), "CI 不該再跑專案裡的 Python 工具");
    // action 都釘 40 字元的 commit SHA
    let uses: Vec<&str> = yml.lines().filter(|l| l.trim_start().starts_with("uses:")).collect();
    assert!(!uses.is_empty());
    let pinned = regex::Regex::new(r"^\s*uses: [\w./-]+@[0-9a-f]{40} # v[\d.]+$").expect("正規式");
    for u in uses {
        assert!(pinned.is_match(u), "action 沒有釘 SHA：{u}");
    }
    // 核對那一步在執行 gate 之前
    let (check, exec) = (yml.find("sha256sum -c").expect("核對"), yml.find("merge-gate \"${ARGS[@]}\"").expect("執行"));
    assert!(check < exec, "要先核對 sha256 再執行");

    // 二、給了對得上的 checksums：原樣放進去
    let sums = format!("{{\n  \"files\": {{\n    \"awf-darwin-arm64\": \"aa\",\n    \"awf-linux-x64\": \"{sum}\"\n  }},\n  \"source\": \"示範\"\n}}\n");
    write(&d.join("sums.json"), &sums);
    let r = run(&d, None, &["--target", "q", "--defaults", "--with-merge-gate", "--merge-gate-bin", "good", "--merge-gate-checksums", "sums.json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(read(&d.join("q/.claude/teams/bin/checksums.json")), sums);

    // 三、模版自己有執行檔與 checksums（發佈版的樣子）：不用指定
    let tpl = d.join("tpl");
    template_copy(&tpl);
    std::fs::create_dir_all(tpl.join("bin")).expect("建目錄");
    std::fs::write(tpl.join("bin/awf-linux-x64"), fake_elf()).expect("寫");
    write(&tpl.join("bin/checksums.json"), &sums);
    let r = run_from(&tpl, &d, None, &["--target", "r", "--defaults", "--with-merge-gate"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(read(&d.join("r/.claude/teams/bin/checksums.json")), sums);
    // 模版的 checksums 對不上或不見了：拒絕
    write(&tpl.join("bin/checksums.json"), &sums.replace(&sum, "00"));
    let r = run_from(&tpl, &d, None, &["--target", "s", "--defaults", "--with-merge-gate"]);
    assert!(r.code == 1 && r.err.contains("不符") && !d.join("s").exists(), "{}", r.err);
    std::fs::remove_file(tpl.join("bin/checksums.json")).expect("刪");
    let r = run_from(&tpl, &d, None, &["--target", "s", "--defaults", "--with-merge-gate"]);
    assert!(r.code == 1 && r.err.contains("checksums.json") && !d.join("s").exists(), "{}", r.err);

    // 四、預設不放；沒帶 --force 的重跑不換已存在的執行檔
    assert_eq!(run(&d, None, &["--target", "t", "--defaults"]).code, 0);
    assert!(!d.join("t/.github").exists() && !d.join("t/.claude/teams/bin").exists());
    let mut newer = fake_elf();
    newer.extend_from_slice(b"newer\n");
    std::fs::write(d.join("newer"), &newer).expect("寫");
    let r = run(&d, None, &["--target", "p", "--defaults", "--with-merge-gate", "--merge-gate-bin", "newer"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(std::fs::read(p.join(".claude/teams/bin/awf-linux-x64")).ok(), Some(fake_elf()), "沒帶 --force 不覆寫");
    let r = run(&d, None, &["--target", "p", "--upgrade", "--with-merge-gate", "--merge-gate-bin", "newer"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(std::fs::read(p.join(".claude/teams/bin/awf-linux-x64")).ok(), Some(newer.clone()));
    assert_eq!(json_of(&p.join(".claude/teams/bin/checksums.json"))["files"]["awf-linux-x64"], sha256(&newer), "執行檔與 checksums 一起換");
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ 訪談補題與 --reconfigure <主題>

/// 互動模式（tty）跑 init，`fed` 是使用者打的字。
fn run_tty(cwd: &Path, args: &[&str], fed: &str) -> Out {
    let env = init::Env { source: Some(template().to_path_buf()), cwd: cwd.to_path_buf(), home: None, awf_bin: None };
    let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (mut out, mut err, mut input) = (Vec::new(), Vec::new(), std::io::Cursor::new(fed.as_bytes().to_vec()));
    let code = init::run(&argv, &env, &mut out, &mut err, &mut input, true);
    Out { code, out: String::from_utf8_lossy(&out).into_owned(), err: String::from_utf8_lossy(&err).into_owned() }
}

#[test]
fn e_reconfigure_one_topic_only_asks_and_changes_that_topic() {
    let d = tmp("topic");
    let p = d.join("proj");
    std::fs::create_dir_all(d.join("acme-specs/.git")).expect("旁邊的 repo");
    let ps = p.to_string_lossy().into_owned();
    let r = run(&d, None, &["--target", &ps, "--defaults"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let (meth, guard) = (p.join(".claude/teams/methodology.json"), p.join(".claude/teams/guard.config.json"));
    let before = json_of(&meth);
    let guard_before = json_of(&guard);

    // 1. 全部 Enter：什麼都不改
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "knowledge"], "");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert!(r.out.contains("沒有要改的"), "{}", r.out);
    assert!(r.out.contains("候選") && r.out.contains("../acme-specs"), "旁邊的 repo 列成候選：{}", r.out);
    assert!(!r.out.contains("模型分級") || !r.out.contains("models.mode（"), "只問這個主題：{}", r.out);
    assert!(!r.out.contains("要不要啟用自動合併"), "別的主題不問：{}", r.out);
    assert_eq!(json_of(&meth), before);

    // 2. 知識圖：建圖、提問帶位置、收旁邊的 repo
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "knowledge"], "y\ny\n../acme-specs\ndocs/**/*.md, specs/**/*.md\ny\n");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let now = json_of(&meth);
    assert_eq!(now["knowledge"]["auto"]["update"], true);
    assert_eq!(now["knowledge"]["auto"]["prompt"], true);
    assert_eq!(now["knowledge"]["repos"], serde_json::json!([{"id": "acme-specs", "path": "../acme-specs", "sources": ["docs/**/*.md", "specs/**/*.md"]}]));
    let strip = |v: &Value| {
        let mut o = v.as_object().cloned().unwrap_or_default();
        o.remove("knowledge");
        o
    };
    assert_eq!(strip(&now), strip(&before), "其他鍵不動");
    assert_eq!(json_of(&guard), guard_before);
    // 再問一次全部 Enter：預設值是上次的答案，不會改回去
    let r = run_tty(&d, &["--target", &ps, "--reconfigure=knowledge"], "");
    assert!(r.code == 0 && r.out.contains("沒有要改的"), "{}{}", r.out, r.err);
    assert_eq!(json_of(&meth), now);

    // 3. 平行與拆分：只改回答和預設不同的那一題
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "parallelism"], "6\n\ny\n");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let par = json_of(&meth);
    assert_eq!(par["parallelism"]["split_threshold"], 6);
    assert_eq!(par["parallelism"]["max_sub_plans"], before["parallelism"]["max_sub_plans"]);
    let bad = run_tty(&d, &["--target", &ps, "--reconfigure", "parallelism"], "1\n");
    assert_ne!(bad.code, 0, "小於 2 要拒絕");
    assert_eq!(json_of(&meth), par);

    // 4. 守門設定：受保護分支多一個；不能清空
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "guard"], "main, master, release\n\ny\n");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let g = json_of(&guard);
    assert_eq!(g["protected_branches"], serde_json::json!(["main", "master", "release"]));
    assert_eq!(g["acceptance_test_dirs"], guard_before["acceptance_test_dirs"]);
    assert_eq!(json_of(&meth), par, "守門主題不動 methodology.json");
    let bad = run_tty(&d, &["--target", &ps, "--reconfigure", "guard"], "-\n");
    assert_ne!(bad.code, 0, "受保護分支不能清空");
    assert_eq!(json_of(&guard), g);

    // 5. 交付文件：內建 profile 有文件清單、預設不啟用 → 回答 y 才啟用
    assert_ne!(before["deliverables"]["enabled"], true);
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "deliverables"], "y\ny\n");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(json_of(&meth)["deliverables"]["enabled"], true);

    // 6. 不能單獨問的主題、不認得的主題、答案檔帶了別的主題的鍵：都拒絕、不寫
    let snap = snapshot(&p);
    let r = run_tty(&d, &["--target", &ps, "--reconfigure", "conventions"], "");
    assert!(r.code != 0 && r.err.contains("不能單獨重問"), "{}{}", r.out, r.err);
    let r = run_tty(&d, &["--target", &ps, "--reconfigure=nope"], "");
    assert!(r.code != 0 && r.err.contains("沒有 'nope' 這個主題"), "{}{}", r.out, r.err);
    let af = d.join("a.json");
    write(&af, r#"{"knowledge": {"auto": {"prompt": false}}, "auto_merge": true}"#);
    let afs = af.to_string_lossy().into_owned();
    let r = run(&d, None, &["--target", &ps, "--reconfigure", "knowledge", "--answers", &afs]);
    assert!(r.code != 0 && r.err.contains("auto_merge"), "{}{}", r.out, r.err);
    assert_eq!(snapshot(&p), snap, "被拒絕時什麼都沒寫");
    // 只有那個主題的鍵：照做
    write(&af, r#"{"knowledge": {"auto": {"update": true, "prompt": false}, "repos": []}}"#);
    let r = run(&d, None, &["--target", &ps, "--reconfigure", "knowledge", "--answers", &afs]);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    let last = json_of(&meth);
    assert_eq!(last["knowledge"]["auto"]["prompt"], false);
    assert_eq!(last["knowledge"]["repos"], serde_json::json!([]));
    assert_eq!(json_of(&guard), g);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn e_interactive_init_asks_the_new_topics_last() {
    let d = tmp("ask");
    let p = d.join("proj");
    let ps = p.to_string_lossy().into_owned();
    // 全部 Enter：新的題目有問到，而且什麼都沒有多寫
    let r = run_tty(&d, &["--target", &ps], "");
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    for q in ["要不要建知識圖", "提問時要不要帶位置", "有沒有別的 repo 要收進知識圖", "要不要產交付文件", "預估子任務超過幾個就拆成子計劃", "受保護分支", "驗收測試目錄"] {
        assert!(r.out.contains(q), "沒有問「{q}」：{}", r.out);
    }
    let hooks_at = r.out.find("hook 註冊方式（plugin＝").expect("既有的最後一題");
    assert!(r.out.find("要不要建知識圖").is_some_and(|i| i > hooks_at), "新的題目接在既有題目後面");
    let d2 = tmp("ask-defaults");
    let p2 = d2.join("proj");
    let r2 = run(&d2, None, &["--target", &p2.to_string_lossy(), "--defaults"]);
    assert_eq!(r2.code, 0, "{}", r2.err);
    for f in [".claude/teams/methodology.json", ".claude/teams/guard.config.json"] {
        assert_eq!(json_of(&p.join(f)), json_of(&p2.join(f)), "全部 Enter 和 --defaults 的 {f} 一樣");
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}
