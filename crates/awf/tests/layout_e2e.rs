//! 主線分支與目錄布局「有設定」的端到端測試：實際執行編好的 `awf`，在暫存目錄 init 一個專案，
//! 產生全部 workflow 腳本、乾跑（有 node 才跑），再看派工單、工具與 hook 用的是不是設定的值。
//! 「沒設定時逐字不變」由既有的回歸基準（`tests/parity/`）、`golden-set/token-budget.json` 與 hook 案例負責，這裡只補「有設定」。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

const EXE: &str = env!("CARGO_BIN_EXE_awf");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

struct Work(PathBuf);

impl Work {
    fn new(tag: &str) -> Work {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("awf-layout-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Work(d.canonicalize().unwrap())
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        if std::env::var("KEEP").as_deref() != Ok("1") {
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
}

fn exec(prog: &str, argv: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Out {
    let mut c = Command::new(prog);
    c.args(argv).current_dir(cwd).stdin(Stdio::null()).env_remove("CLAUDE_PROJECT_DIR").env_remove("AWF_PLUGIN_DIR").env_remove("CLAUDE_PLUGIN_ROOT");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap_or_else(|e| panic!("跑不起來 {prog}：{e}"));
    Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
}

fn awf(argv: &[&str], cwd: &Path) -> Out {
    exec(EXE, argv, cwd, &[])
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn load(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{} 不是 JSON：{e}", p.display()))
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// `awf init --target <專案> --config <內容>`。
fn init(w: &Work, name: &str, config: &str) -> PathBuf {
    let c = w.0.join(format!("{name}.workflow.config"));
    write(&c, config);
    let t = w.0.join(name);
    let r = awf(&["init", "--target", s(&t), "--config", s(&c)], &w.0);
    assert_eq!(r.code, 0, "awf init 失敗：\n{}", r.all());
    t
}

fn render_all(t: &Path) -> PathBuf {
    let out = t.join("_tpm/wf");
    let r = awf(&["render", "--all", "--out", s(&out), "--plugin-dir", s(&root().join("plugin"))], t);
    assert_eq!(r.code, 0, "awf render --all 失敗：\n{}", r.all());
    out
}

fn script(t: &Path, name: &str) -> String {
    read(&render_all(t).join(format!("{name}.js")))
}

/// 乾跑全部腳本並回傳情境報告（沒有 node 回 None）。
fn dry_run(t: &Path) -> Option<Value> {
    let node = which("node")?;
    let wf = render_all(t);
    let report = t.join("_tpm/report.json");
    let r = exec(
        s(&node),
        &[s(&root().join("plugin/workflow-templates/check_workflows.mjs")), "--project", s(t), "--agents-dir", s(&root().join("plugin/agents")), "--wf-dir", s(&wf)],
        t,
        &[("AWF_SCENARIO_REPORT", s(&report))],
    );
    assert_eq!(r.code, 0, "乾跑失敗：\n{}", r.all());
    Some(load(&report))
}

/// 報告裡某支腳本、名稱以 `prefix` 開頭的情境的每張派工單。
fn prompts(report: &Value, wf: &str, prefix: &str) -> Vec<(String, String)> {
    let sc = report[wf]["scenarios"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.starts_with(prefix))));
    let sc = sc.unwrap_or_else(|| panic!("{wf} 沒有「{prefix}…」情境"));
    sc["prompts"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default()
}

fn git(repo: &Path, argv: &[&str]) {
    let o = Command::new("git").args(argv).current_dir(repo).stdin(Stdio::null())
        .envs([("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@example.invalid"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@example.invalid")])
        .output().unwrap_or_else(|e| panic!("git 跑不起來：{e}"));
    assert!(o.status.success(), "git {argv:?}：{}", String::from_utf8_lossy(&o.stderr));
}

/// W14：主線設成 develop、remote 設成 upstream。範本、工具都用新值；沒設定的專案照舊是 origin/main。
#[test]
fn base_branch_configured() {
    let w = Work::new("base");
    let t = init(&w, "proj", "PROJECT_NAME=acme-portal\nBASE_BRANCH=develop\nBASE_REMOTE=upstream\n");
    let guard = t.join(".claude/teams/guard.config.json");
    let g = load(&guard);
    assert_eq!((g["base_branch"].as_str(), g["base_remote"].as_str()), (Some("develop"), Some("upstream")));
    let rules = read(&t.join(".claude/teams/rules.md"));
    assert!(rules.contains("主線分支 `develop`") && rules.contains("--base upstream/develop"), "rules.md 的範例要帶設定的主線分支");

    // 1. 產生的腳本：預設 base 是 upstream/develop，沒有殘留的 origin/main
    let wf = render_all(&t);
    for name in ["sdlc-plan", "sdlc-build", "sdlc-sync", "sdlc-test", "sdlc-deliver", "sdlc-merge"] {
        let text = read(&wf.join(format!("{name}.js")));
        assert!(text.contains("'upstream/develop'") && !text.contains("origin/main"), "{name}.js 的預設 base 沒有跟著設定");
    }
    let pm = read(&wf.join("event-post-merge.js"));
    assert!(pm.contains("--branch develop；") && pm.contains("讀 develop 的 CI") && !pm.contains("--branch main"), "event-post-merge.js 要看設定的主線分支");

    // 2. 乾跑通過，沒帶 base 的派工單用新值
    match dry_run(&t) {
        Some(rep) => {
            let sync = prompts(&rep, "sdlc-sync.js", "主線分支：");
            assert!(sync.iter().any(|(l, p)| l == "plan" && p.contains("--base upstream/develop") && p.contains("基準 upstream/develop）")), "sdlc-sync 的派工單：{sync:?}");
            let plan = prompts(&rep, "sdlc-plan.js", "主線分支：");
            assert!(!plan.is_empty() && plan.iter().all(|(_, p)| p.contains("，從 upstream/develop 開）")), "sdlc-plan 的派工單沒有用設定的主線分支");
        }
        None => eprintln!("[略過] 沒有 node，略過乾跑"),
    }

    // 3. 範本的值取自 guard.config.json（工具讀的同一份）：手改之後下一次產生就跟上；不合規則退回預設
    let mut hand = g.clone();
    hand["base_branch"] = Value::String("release/2.x".into());
    write(&guard, &serde_json::to_string_pretty(&hand).unwrap());
    assert!(script(&t, "sdlc-sync").contains("A.base || 'upstream/release/2.x'"));
    hand["base_branch"] = Value::String("bad name; rm -rf".into());
    hand["base_remote"] = Value::String("x'y".into());
    write(&guard, &serde_json::to_string_pretty(&hand).unwrap());
    assert!(script(&t, "sdlc-sync").contains("A.base || 'origin/main'"), "不合規則的值要退回預設");
    let d = awf(&["doctor", "--plugin-dir", s(&root().join("plugin"))], &t);
    assert!(d.all().contains("base_branch 不合規則") && d.all().contains("base_remote 不合規則"), "awf doctor 要列出不合規則的值：\n{}", d.all());
    write(&guard, &serde_json::to_string_pretty(&g).unwrap());
    let d = awf(&["doctor", "--plugin-dir", s(&root().join("plugin"))], &t);
    assert!(d.all().contains("主線分支 develop（base_branch）不在受保護分支裡"), "主線不在 protected_branches 時 awf doctor 要提示：\n{}", d.all());

    // 4. 工具：沒帶 --base 時用設定的值；帶了以命令列為準
    if which("git").is_some() {
        let repo = t.join("repos/app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        write(&repo.join("a.txt"), "a\n");
        git(&repo, &["add", "-A"]);
        git(&repo, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "chore: 起點"]);
        git(&repo, &["branch", "feature/a"]);
        let plan = |extra: &[&str], cwd: &Path| {
            let mut argv = vec!["sync-branch", "plan", "--repo", s(&repo), "--branch", "feature/a", "--no-fetch"];
            argv.extend_from_slice(extra);
            awf(&argv, cwd).stdout
        };
        assert!(plan(&[], &t).contains("找不到 base upstream/develop"), "sync-branch 沒帶 --base：{}", plan(&[], &t));
        assert!(plan(&["--config", s(&guard)], &w.0).contains("找不到 base upstream/develop"));
        assert!(plan(&["--base", "origin/trunk"], &t).contains("找不到 base origin/trunk"), "--base 優先");
        assert!(plan(&[], &w.0).contains("找不到 base origin/main"), "不在專案裡、repo 也沒有設定：照舊是 origin/main");
        let run = t.join("_tpm/runs/2026-11-16-acme-export");
        write(&run.join("run.json"), &serde_json::json!({"target_repo": s(&repo), "recipe": {"name": "feature"}}).to_string());
        let cd = awf(&["recipe", "check-diff", "--run", s(&run)], &t);
        assert!(cd.code != 0 && cd.all().contains("upstream/develop"), "recipe check-diff 沒帶 --base：\n{}", cd.all());
        let pb = awf(&["publish", "push", "--repo-dir", s(&repo), "--branch", "feature/a", "--run", s(&run), "--dry-run", "--project-dir", s(&t)], &t);
        assert!(pb.all().contains("\"base\": \"develop\""), "awf publish 沒帶 --base：PR 的目標分支是設定的主線分支\n{}", pb.all());
        let pb = awf(&["publish", "push", "--repo-dir", s(&repo), "--branch", "feature/a", "--run", s(&run), "--dry-run", "--project-dir", s(&t), "--base", "trunk"], &t);
        assert!(pb.all().contains("\"base\": \"trunk\""), "--base 優先：\n{}", pb.all());
    }

    // 5. 沒設定：和以前一樣
    let t2 = init(&w, "dflt", "PROJECT_NAME=acme-portal\n");
    let g2 = load(&t2.join(".claude/teams/guard.config.json"));
    assert_eq!((g2["base_branch"].as_str(), g2["base_remote"].as_str()), (Some("main"), Some("origin")));
    let sync = script(&t2, "sdlc-sync");
    assert!(sync.contains("const BASE = A.base || 'origin/main'\n"));
    // 舊專案（守門設定沒有這兩個鍵、紀錄檔也沒有）：產生的腳本逐字相同
    let mut old = g2.clone();
    old.as_object_mut().unwrap().remove("base_branch");
    old.as_object_mut().unwrap().remove("base_remote");
    write(&t2.join(".claude/teams/guard.config.json"), &serde_json::to_string_pretty(&old).unwrap());
    let rec = t2.join(".claude/teams/workflow.config");
    let kept: String = read(&rec).lines().filter(|l| !l.starts_with("BASE_")).map(|l| format!("{l}\n")).collect();
    write(&rec, &kept);
    assert_eq!(script(&t2, "sdlc-sync"), sync, "沒有新鍵的舊專案要產生一樣的腳本");

    // 6. 不合規則的佔位符值：init 拒絕
    let c = w.0.join("bad.workflow.config");
    for bad in ["BASE_BRANCH=origin main\n", "BASE_BRANCH=-x\n", "BASE_REMOTE=a/b\n", "BASE_BRANCH=x';process.exit()//\n"] {
        write(&c, bad);
        let r = awf(&["init", "--target", s(&w.0.join("bad")), "--config", s(&c)], &w.0);
        assert!(r.code != 0 && r.all().contains("不合規則"), "{bad}：\n{}", r.all());
        assert!(!w.0.join("bad/.claude/teams/guard.config.json").exists(), "{bad}");
    }
}

/// W14：答案檔與 `--reconfigure` 改主線分支；`--detect` 從遠端的預設分支看。
#[test]
fn base_branch_answers_and_detect() {
    let w = Work::new("ans");
    let t = init(&w, "proj", "PROJECT_NAME=acme-portal\n");
    let guard = t.join(".claude/teams/guard.config.json");
    let ans = w.0.join("answers.json");
    write(&ans, &serde_json::json!({"guard": {"base_branch": "develop", "protected_branches": ["main", "develop"]}}).to_string());
    let r = awf(&["init", "--target", s(&t), "--reconfigure", "--answers", s(&ans)], &w.0);
    assert_eq!(r.code, 0, "--reconfigure 失敗：\n{}", r.all());
    assert_eq!(load(&guard)["base_branch"], "develop");
    assert!(read(&t.join(".claude/teams/workflow.config")).contains("BASE_BRANCH=develop\n"), "紀錄檔要跟上：\n{}", r.all());
    assert!(script(&t, "sdlc-deliver").contains("const BASE = A.base || 'origin/develop'\n"));
    write(&ans, &serde_json::json!({"guard": {"base_branch": "not ok"}}).to_string());
    let r = awf(&["init", "--target", s(&t), "--reconfigure", "--answers", s(&ans)], &w.0);
    assert!(r.code != 0 && r.all().contains("base_branch 不合規則"), "不合規則的答案要拒絕：\n{}", r.all());
    assert_eq!(load(&guard)["base_branch"], "develop");

    if which("git").is_some() {
        let p = w.0.join("existing");
        std::fs::create_dir_all(&p).unwrap();
        git(&p, &["init", "-q", "-b", "develop"]);
        write(&p.join("a.txt"), "a\n");
        git(&p, &["add", "-A"]);
        git(&p, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "chore: 起點"]);
        git(&p, &["update-ref", "refs/remotes/origin/develop", "HEAD"]);
        git(&p, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/develop"]);
        let d = awf(&["init", "--detect", "--target", s(&p), "--json"], &w.0);
        assert_eq!(d.code, 0, "{}", d.all());
        let doc: Value = serde_json::from_str(&d.stdout).unwrap_or_else(|e| panic!("--detect --json 不是 JSON：{e}\n{}", d.stdout));
        assert_eq!(doc["suggested_answers"]["guard"]["base_branch"], "develop", "{}", d.stdout);
    }
}
