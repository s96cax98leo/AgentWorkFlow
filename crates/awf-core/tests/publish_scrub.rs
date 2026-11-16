//! `awf publish` 的去客戶化掃描接點（profile 的 `scrub.when` 含 `publish`）。
//!
//! 沒啟用時 publish 的行為由 `publish_parity` 顧（逐字相同）；這裡只測啟用之後：命中不推、掃不了不推、乾淨的照常往下走。
//! 每一例都在推送之前就停下（命中或掃不了在掃描那一步；乾淨的那幾例沒給 `--run`，停在閘門檢查），不會真的 push；
//! remote 也只是一個不存在的網址。沒有 git 時略過。

use awf_core::publish::{self as pb, Args, Env};
use awf_core::save_run::Ctx;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// 規則用的字串都是虛構的
const SECRET: &str = "ACME-Orbital";
const CODE_NAME: &str = "zephyr-internal";

struct Fx {
    root: PathBuf,
    proj: PathBuf,
    repo: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(repo: &Path, args: &[&str]) -> bool {
    Command::new("git").arg("-C").arg(repo).args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]).args(args)
        .output().map(|o| o.status.success()).unwrap_or(false)
}

fn put(p: &Path, body: &[u8]) {
    std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

/// 專案（guard.config.json）與底下的 repo：main 加三個功能分支（乾淨的、內容命中的、路徑名命中的）。沒有 git 回 None。
fn fixture(tag: &str) -> Option<Fx> {
    let root = std::env::temp_dir().join(format!("awf-publish-scrub-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let root = root.canonicalize().expect("canonicalize");
    let (proj, repo) = (root.join("proj"), root.join("proj/repos/app"));
    let cfg = json!({"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "upstream_repos": "acme-up/app",
        "allowed_branch_prefixes": "feature/", "protected_branches": ["main"]});
    put(&proj.join(".claude/teams/guard.config.json"), cfg.to_string().as_bytes());
    std::fs::create_dir_all(&repo).expect("mkdir");
    let fx = Fx { root, proj, repo };
    if !git(&fx.repo, &["init", "-q", "-b", "main"]) {
        eprintln!("[略過] 沒有 git");
        return None;
    }
    let commit = |rel: &str, body: &[u8]| {
        put(&fx.repo.join(rel), body);
        assert!(git(&fx.repo, &["add", "-A"]) && git(&fx.repo, &["commit", "-qm", "c"]), "commit {rel}");
    };
    commit("README.md", "# 示範客戶 ACME\n".as_bytes());
    assert!(git(&fx.repo, &["remote", "add", "origin", "https://github.com/acme-fork/app.git"]));
    assert!(git(&fx.repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]));
    for (branch, rel, body) in [
        ("feature/clean", "docs/export.md".to_string(), "# 匯出 CSV\n".to_string()),
        ("feature/content", "docs/notes.md".to_string(), format!("第一行\n這是 {SECRET} 的案子\n")),
        ("feature/path", format!("src/{CODE_NAME}/a.txt"), "ok\n".to_string()),
    ] {
        assert!(git(&fx.repo, &["checkout", "-q", "main"]) && git(&fx.repo, &["checkout", "-qb", branch]));
        commit(&rel, body.as_bytes());
    }
    // 工作目錄停在 main：掃的要是「要推的分支」的內容，不是工作目錄
    assert!(git(&fx.repo, &["checkout", "-q", "main"]));
    Some(fx)
}

impl Fx {
    fn profile(&self, scrub: Value) {
        put(&self.proj.join(".claude/teams/methodology.json"), json!({"name": "t", "scrub": scrub}).to_string().as_bytes());
    }

    fn push(&self, branch: &str) -> (i32, Value) {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let ctx = Ctx::new(&s(&self.proj), &s(&self.proj));
        let env = Env { ctx, project: s(&self.proj), git: vec!["git".into()], gh: vec!["gh-不會被叫到".into()], env: Vec::new() };
        let a = Args {
            repo_dir: s(&self.repo),
            branch: branch.into(),
            base: "main".into(),
            remote: "origin".into(),
            stage: "deliver".into(),
            no_pr: true,
            project_dir: Some(s(&self.proj)),
            timestamp: Some("2026-10-24T09:00:00+00:00".into()),
            ..Args::default()
        };
        pb::publish(&env, &a).expect("publish 不該丟例外")
    }
}

fn reasons(rec: &Value) -> String {
    rec["reasons"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

#[test]
fn publish_scrub_gate() {
    let Some(fx) = fixture("gate") else { return };
    let on = |extra: Value| {
        let mut s = json!({"enabled": true, "terms": [SECRET], "terms_files": [".claude/teams/scrub.local.txt"], "when": ["publish"]});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            s[k] = v;
        }
        s
    };
    put(&fx.proj.join(".claude/teams/scrub.local.txt"), format!("# 本機清單\n{CODE_NAME}\n").as_bytes());

    // 內容命中：不推，理由只有路徑、行號、規則編號
    fx.profile(on(json!({})));
    let (code, rec) = fx.push("feature/content");
    let why = reasons(&rec);
    assert!(code == 1 && rec["decision"] == "refused", "{rec}");
    assert!(why.contains("去客戶化掃描命中（1 處）") && why.contains("docs/notes.md:2  規則 #1（scrub.terms 第 1 條）"), "{why}");
    assert!(!rec.to_string().contains(SECRET) && !rec.to_string().contains(CODE_NAME), "稽核紀錄不能有命中的字串或規則：{rec}");

    // 路徑名命中（規則來自本機規則檔）
    let (code, rec) = fx.push("feature/path");
    let why = reasons(&rec);
    assert!(code == 1 && why.contains("去客戶化掃描命中") && why.contains("規則 #2（.claude/teams/scrub.local.txt:2）"), "{why}");

    // 乾淨的分支：掃描通過，接著停在閘門檢查（沒給 --run）
    let (code, rec) = fx.push("feature/clean");
    let why = reasons(&rec);
    assert!(code == 1 && why.contains("去客戶化掃描通過（1 個檔、2 條規則）") && why.contains("沒有 --run"), "{why}");

    // exclude 把命中的路徑排除 → 掃描通過
    fx.profile(on(json!({"exclude": ["docs/**"]})));
    let why = reasons(&fx.push("feature/content").1);
    assert!(why.contains("去客戶化掃描通過（0 個檔") && why.contains("沒有 --run"), "{why}");

    // when 沒有 publish、或沒啟用：完全不掃（有命中的分支也照常往下走）
    for scrub in [json!({"enabled": true, "terms": [SECRET]}), json!({"enabled": false, "terms": [SECRET], "when": ["publish"]})] {
        fx.profile(scrub);
        let why = reasons(&fx.push("feature/content").1);
        assert!(!why.contains("去客戶化") && why.contains("沒有 --run"), "{why}");
    }

    // 掃描本身出錯：一律不推
    put(&fx.proj.join(".claude/teams/bad.txt"), format!("ok\n(?a){SECRET}\n").as_bytes());
    fx.profile(json!({"enabled": true, "terms_files": [".claude/teams/bad.txt"], "when": ["publish"]}));
    let (code, rec) = fx.push("feature/clean");
    let why = reasons(&rec);
    assert!(code == 1 && why.contains("去客戶化掃描跑不了") && why.contains("bad.txt 第 2 行") && !why.contains(SECRET), "{why}");
    fx.profile(json!({"enabled": true, "terms": [SECRET], "when": ["publish", "deliver"]}));
    let why = reasons(&fx.push("feature/clean").1);
    assert!(why.contains("去客戶化掃描跑不了") && why.contains("scrub.when"), "{why}");
    put(&fx.proj.join(".claude/teams/methodology.json"), b"{\"scrub\": {\"enabled\": true,}");
    let why = reasons(&fx.push("feature/clean").1);
    assert!(why.contains("去客戶化掃描跑不了") && why.contains("不是合法 JSON"), "{why}");

    // 規則檔不存在：略過（這次沒有規則可掃），照常往下走
    fx.profile(json!({"enabled": true, "terms_files": ["nope.txt"], "when": ["publish"]}));
    let why = reasons(&fx.push("feature/content").1);
    assert!(!why.contains("命中") && !why.contains("跑不了") && why.contains("nope.txt 不存在") && why.contains("沒有 --run"), "{why}");
}
