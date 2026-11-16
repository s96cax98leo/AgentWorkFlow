//! 1.0 獨立審查（移植）發現的閘門端問題：每一則一組測試，輸入取自審查的重現。
//!
//! - H1 專案根：閘門不能因為「在哪個目錄執行」而換一份設定、schema 或 profile。
//! - M3：重存某段的回傳讀不懂時，舊的結果與核准不能原封不動留著。
//! - plan.json 讀不懂：拆工檢查不能整段跳過。
//! - `awf run-brief --log`：不能拿來寫工具自己的狀態檔。
//!
//! 一律用真的執行檔、乾淨的環境（拿掉 `CLAUDE_*`、`AWF_*`），目前目錄由各案例指定。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("awf-review-{}-{}-{}", std::process::id(), n, tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建立暫存目錄");
        Tmp(d.canonicalize().expect("解開暫存目錄"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn scaffold_teams() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin/scaffold/.claude/teams")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("建立目錄");
    for e in std::fs::read_dir(from).expect("讀目錄").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_dir(&src, &dst) } else { std::fs::copy(&src, &dst).map(|_| ()).expect("複製檔案") }
    }
}

fn write(p: &Path, text: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).expect("建立目錄");
    }
    std::fs::write(p, text).expect("寫檔");
}

fn write_json(p: &Path, v: &Value) {
    write(p, &serde_json::to_string(v).expect("JSON"));
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

/// 一個「init 過」的專案的最小骨架：`.claude/teams/` 有 VERSION 與 schemas。
fn mk_project(root: &Path) {
    let teams = root.join(".claude/teams");
    copy_dir(&scaffold_teams().join("schemas"), &teams.join("schemas"));
    write(&teams.join("VERSION"), "1.0.0\n");
}

struct Out {
    code: i32,
    out: String,
    err: String,
}

/// 跑 `awf <args>`：目前目錄是 `cwd`，環境拿掉 `CLAUDE_*`、`AWF_*` 再加上 `env`。
fn awf(args: &[&str], cwd: &Path, stdin: Option<&str>, env: &[(&str, &str)]) -> Out {
    let mut c = Command::new(EXE);
    c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("執行 awf");
    {
        let mut pipe = child.stdin.take().expect("stdin");
        let _ = pipe.write_all(stdin.unwrap_or("").as_bytes());
    }
    let o = child.wait_with_output().expect("等 awf 結束");
    Out { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() }
}

// ------------------------------------------------------------------ merge-gate 的輸入（同審查的 h.py）

fn reqs() -> Value {
    json!([{"id": "REQ-1", "text": "匯出按鈕下載 CSV", "source": "ACME"}, {"id": "REQ-2", "text": "超過一萬筆時提示", "source": "ACME"}])
}

fn row(id: &str, text: &str) -> Value {
    json!({"requirement_id": id, "requirement": text, "evidence": [format!("results.json {id}")], "result": "match"})
}

fn decision(rows: Value) -> Value {
    json!({"status": "done", "summary": "示範", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [],
        "payload": {"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": SHA, "requirements_source": "json",
            "traceability": rows, "out_of_scope": [], "followups": [], "risk_tier": "low", "gate_reasons": []}})
}

fn mk_diff(path: &str, plus: &[&str]) -> String {
    let body: String = plus.iter().map(|l| format!("+{l}\n")).collect();
    format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1,{} @@\n{body}", plus.len())
}

fn gate_args(inp: &Path, decision: &str, evidence: &str, diff: &str) -> Vec<String> {
    let f = |n: &str| s(&inp.join(n));
    ["check", "--decision", &f(decision), "--repo", "acme-fork/app", "--pr", "3", "--head-sha", SHA, "--evidence", &f(evidence), "--checks", &f("c.json"), "--diff", &f(diff),
        "--tests-before", "1", "--tests-after", "1", "--timestamp", "2099-10-24T09:00:00+00:00"].iter().map(|x| x.to_string()).collect()
}

fn gate(args: &[String], extra: &[&str], cwd: &Path, env: &[(&str, &str)]) -> (Out, Value) {
    let mut a: Vec<&str> = vec!["merge-gate"];
    a.extend(args.iter().map(String::as_str));
    a.extend(extra);
    let o = awf(&a, cwd, None, env);
    let v = serde_json::from_str(&o.out).unwrap_or(Value::Null);
    (o, v)
}

/// 重現 A 的現場：真專案 `good`（auto_merge 沒啟用），底下的 `repos/app` 自帶一份寬鬆的 `.claude/teams`。
fn scene_a(t: &Tmp, nested_version: bool) -> (PathBuf, PathBuf, Vec<String>) {
    let good = t.path().join("good");
    let evil = good.join("repos/app");
    mk_project(&good);
    write_json(&good.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs", "auto_merge": {"enabled": false}, "requirements": {"source": "json", "path": "reqs.json"}}));
    write_json(&good.join("reqs.json"), &reqs());
    copy_dir(&scaffold_teams().join("schemas"), &evil.join(".claude/teams/schemas"));
    if nested_version {
        write(&evil.join(".claude/teams/VERSION"), "1.0.0\n");
    }
    write_json(&evil.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "../../_tpm/runs",
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "high"}, "daily_cap": 999, "large_lines": 1000000000},
        "requirements": {"source": "json", "path": "easy.json"}}));
    write_json(&evil.join("easy.json"), &json!([{"id": "X-1", "text": "隨便"}]));
    let inp = t.path().join("in");
    write_json(&inp.join("d.json"), &decision(json!([row("X-1", "隨便")])));
    write_json(&inp.join("e.json"), &json!({"commit": SHA, "results": [{"requirement_id": "X-1", "result": "pass"}]}));
    write_json(&inp.join("c.json"), &json!({"ci": "success"}));
    write(&inp.join("diff.txt"), &mk_diff("src/a.py", &["x = 1"]));
    let args = gate_args(&inp, "d.json", "e.json", "diff.txt");
    (good, evil, args)
}

/// H1 重現 A：在自帶設定的子目錄執行、沒給 `--project-dir`，不能照子目錄那份設定判 merge，也不能把 merge 紀錄寫進真專案的稽核檔。
#[test]
fn h1_merge_gate_in_subdir_with_own_config_uses_the_real_project() {
    for nested_version in [false, true] {
        let t = Tmp::new("h1a");
        let (good, evil, args) = scene_a(&t, nested_version);
        let (o, v) = gate(&args, &[], &evil, &[]);
        assert_ne!(v["decision"], "merge", "子目錄自帶設定不能讓 gate 判 merge（nested_version={nested_version}）：{}", o.out);
        assert_ne!(o.code, 0, "exit code 要非零：{}{}", o.out, o.err);
        let audit = std::fs::read_to_string(good.join("_tpm/runs/merge-audit.jsonl")).unwrap_or_default();
        assert!(!audit.contains("\"decision\": \"merge\"") && !audit.contains("\"decision\":\"merge\""), "真專案的稽核檔不能有 merge 紀錄：{audit}");
        if nested_version {
            assert!(o.err.contains("專案根") && o.err.contains(&s(&good)), "路徑上有兩個專案根時要在 stderr 說明用了哪一個：{}", o.err);
        }
        // 判定用的是真專案的設定：auto_merge 沒啟用
        assert_eq!(v["decision"], "hold", "{}{}", o.out, o.err);
        // CLAUDE_PROJECT_DIR 有設時以它為準
        let (o2, v2) = gate(&args, &[], &evil, &[("CLAUDE_PROJECT_DIR", &s(&good))]);
        assert_eq!(v2["decision"], "hold", "{}{}", o2.out, o2.err);
    }
}

/// H1：找不到專案根（往上沒有 `.claude/teams/VERSION`、沒給 `--project-dir`、沒有 `CLAUDE_PROJECT_DIR`）時報錯，不退回目前目錄。
#[test]
fn h1_merge_gate_without_a_project_root_refuses() {
    let t = Tmp::new("h1none");
    let (good, evil, args) = scene_a(&t, false);
    std::fs::remove_file(good.join(".claude/teams/VERSION")).expect("刪 VERSION");
    let (o, v) = gate(&args, &["--no-audit"], &evil, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert_ne!(v["decision"], "merge", "{}", o.out);
    assert!(o.err.contains("找不到專案根"), "要說明找不到專案根：{}", o.err);
}

/// H1 重現 B：`--project-dir` 指真專案，目前目錄的 schema 被換成 `{}`：schema 要跟著專案根，不合格的對照表一樣 hold。
#[test]
fn h1_merge_gate_schemas_follow_the_project_dir() {
    let t = Tmp::new("h1b");
    let good = t.path().join("good2");
    mk_project(&good);
    write_json(&good.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app",
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "medium", "acme-up/app": "low"}, "daily_cap": 2},
        "requirements": {"source": "json", "path": "reqs.json"}}));
    write_json(&good.join("reqs.json"), &reqs());
    let lax = t.path().join("lax");
    copy_dir(&scaffold_teams().join("schemas"), &lax.join(".claude/teams/schemas"));
    write(&lax.join(".claude/teams/schemas/payloads/merge-decision.schema.json"), "{}");
    let inp = t.path().join("in");
    // 對照表的格式不合 schema：少 gate_reasons、risk_tier 不在列舉裡、每列少 requirement
    let mut bad = decision(json!([
        {"requirement_id": "REQ-1", "result": "match", "evidence": ["x"]},
        {"requirement_id": "REQ-2", "result": "match", "evidence": ["x"]}]));
    bad["payload"].as_object_mut().expect("payload").remove("gate_reasons");
    bad["payload"]["risk_tier"] = json!("whatever");
    write_json(&inp.join("d2.json"), &bad);
    write_json(&inp.join("e.json"), &json!({"commit": SHA, "results": [
        {"requirement_id": "REQ-1", "result": "pass", "evidence": "1 passed"}, {"requirement_id": "REQ-2", "result": "pass", "evidence": "1 passed"}]}));
    write_json(&inp.join("c.json"), &json!({"ci": "success"}));
    write(&inp.join("diff2.txt"), &mk_diff("tests/test_export.py", &["def test_csv():", "    assert export() == 'csv'"]));
    let args = gate_args(&inp, "d2.json", "e.json", "diff2.txt");
    let pd = s(&good);
    let (o1, v1) = gate(&args, &["--project-dir", &pd, "--no-audit"], &good, &[]);
    assert_eq!(v1["decision"], "hold", "在專案根執行：{}{}", o1.out, o1.err);
    let (o2, v2) = gate(&args, &["--project-dir", &pd, "--no-audit"], &lax, &[]);
    assert_eq!(v2["decision"], "hold", "在 schema 被換掉的目錄執行，判定要一樣：{}{}", o2.out, o2.err);
    assert_eq!(v1["reasons"], v2["reasons"], "理由也要一樣（同一份 schema）");
}

// ------------------------------------------------------------------ save-run 的劇本（同審查的 harness.py）

fn envelope(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

fn builtin_profile() -> Value {
    let d = read_json(&scaffold_teams().join("methodologies/default.json"));
    json!({"recipes": d["recipes"], "modifiers": d["modifiers"]})
}

const INTENT: &str = "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n";
const RUN: &str = "_runs/r1";

fn mk_run_project(root: &Path, profile: &Value) {
    mk_project(root);
    write_json(&root.join(".claude/teams/methodology.json"), profile);
    write(&root.join("repo/specs/e/intent.md"), INTENT);
    std::fs::create_dir_all(root.join("_runs")).expect("建立 _runs");
}

fn sr(root: &Path, args: &[&str], stdin: Option<&str>) -> Out {
    let mut a = vec!["save-run"];
    a.extend(args);
    awf(&a, root, stdin, &[])
}

/// 補上 recipe 與 trace（紀錄檔一併寫好）。
fn traced(root: &Path, stage: &str, result: Value, entries: &[(&str, &str)]) -> Value {
    let data = read_json(&root.join(RUN).join("run.json"));
    let mut out = result;
    if let Some(rec) = data.get("recipe").filter(|r| r.is_object()) {
        out["recipe"] = json!({"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]});
    }
    let entries: Vec<(&str, &str)> = if entries.is_empty() { vec![("walk", "x")] } else { entries.to_vec() };
    let mut trace = Vec::new();
    for (label, agent) in entries {
        let name = format!("{}.md", label.replace(':', "_"));
        write(&root.join(RUN).join(stage).join(&name), "# 紀錄\n");
        trace.push(json!({"label": label, "agent_type": agent, "phase": "p", "record": format!("{RUN}/{stage}/{name}"), "ok": true}));
    }
    out["trace"] = Value::Array(trace);
    out
}

fn design_env() -> Value {
    envelope(json!({"epic": "e", "subtasks": [{"id": "t", "title": "t", "kind": "code", "owns": ["a"], "depends_on": [], "acceptance": ["x"]}]}))
}

fn build_env() -> Value {
    envelope(json!({"task_id": "t", "branch": "b", "commits": [], "files_changed": [], "tests": []}))
}

fn test_env() -> Value {
    envelope(json!({"commit": SHA, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
        "criteria": [{"id": "C1", "criterion": "c", "command": "x", "result": "pass", "evidence": "e"}]}))
}

fn must(o: Out, what: &str) {
    assert_eq!(o.code, 0, "{what} 應該成功：{}{}", o.out, o.err);
}

/// 走到 `upto` 段存檔完（不核准 `upto`）。
fn baseline(root: &Path, upto: &str) {
    must(sr(root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    let plan = json!({"stage": "plan", "epic_dir": "repo", "recipe": {"name": "feature", "modifiers": [], "size": ""},
        "intent": envelope(json!({"documents": [{"path": "specs/e/intent.md", "kind": "intent", "audience": "owner"}]}))});
    must(sr(root, &["stage", RUN, "plan"], Some(&plan.to_string())), "stage plan");
    if upto == "plan" {
        return;
    }
    must(sr(root, &["approve", RUN, "plan", "--by", "human", "--at", "t"], None), "approve plan");
    let seq: [(&str, Value, &[(&str, &str)]); 5] = [
        ("design", json!({"design": design_env()}), &[]),
        ("build", json!({"x": build_env()}), &[]),
        ("sync", json!({"x": build_env()}), &[]),
        ("test", json!({"v": test_env()}), &[("verify", "agent-workflow:dev-verifier")]),
        ("deliver", json!({"x": build_env()}), &[("review:standards", "x"), ("review:spec", "x"), ("review:bugs", "x"), ("review:history", "x")]),
    ];
    for (stage, result, entries) in seq {
        let r = traced(root, stage, result, entries);
        must(sr(root, &["stage", RUN, stage], Some(&r.to_string())), &format!("stage {stage}"));
        if stage == upto {
            return;
        }
        if matches!(stage, "design" | "test" | "deliver") {
            must(sr(root, &["approve", RUN, stage, "--by", "human", "--at", "t"], None), &format!("approve {stage}"));
        }
    }
}

/// 重現 C 的現場：profile 有一條一定不過的限制（`check: "false"`），feature 的 security_review 定成 always；test 段已存檔。
fn scene_c(t: &Tmp) -> PathBuf {
    let root = t.path().join("proj");
    let mut p = builtin_profile();
    p["constraints"] = json!([{"id": "c1", "file": "README.md", "stages": ["test"], "check": "false"}]);
    p["recipes"]["feature"]["test"] = json!({"design_review": "auto", "security_review": "always", "modes": ["verify"]});
    mk_run_project(&root, &p);
    write(&root.join("README.md"), "x");
    baseline(&root, "sync");
    let r = traced(&root, "test", json!({"v": test_env()}), &[("verify", "agent-workflow:dev-verifier"), ("sec", "agent-workflow:dev-security-reviewer")]);
    let _ = sr(&root, &["stage", RUN, "test"], Some(&r.to_string()));
    root
}

fn approved(root: &Path, stage: &str) -> bool {
    read_json(&root.join(RUN).join("run.json"))["stages"][stage].get("approved").is_some()
}

/// H1 重現 C：政策核准的判定不能因為換了目前目錄（或環境變數指到別處）就少算專案的限制文件與 profile。
#[test]
fn h1_save_run_policy_approval_does_not_depend_on_cwd() {
    let t = Tmp::new("h1c");
    let root = scene_c(&t);
    let policy = |run: &str, cwd: &Path, env: &[(&str, &str)]| awf(&["save-run", "approve", run, "test", "--by", "policy", "--note", "p", "--at", "t"], cwd, None, env);
    let at_root = policy(RUN, &root, &[]);
    assert_ne!(at_root.code, 0, "在專案根：限制文件的檢查沒過，不能政策核准：{}{}", at_root.out, at_root.err);
    assert!(at_root.err.contains("限制文件"), "{}", at_root.err);
    let in_sub = policy("../_runs/r1", &root.join("repo"), &[]);
    assert_ne!(in_sub.code, 0, "在子目錄執行也不能通過：{}{}", in_sub.out, in_sub.err);
    assert!(in_sub.err.contains("限制文件"), "子目錄執行時一樣要算到專案的限制文件：{}", in_sub.err);
    assert!(!approved(&root, "test"));
    // CLAUDE_PROJECT_DIR 指到沒有 profile 的地方：run.json 已有 recipe，profile 找不到是錯誤
    let elsewhere = policy(RUN, &root, &[("CLAUDE_PROJECT_DIR", &s(&root.join("repo")))]);
    assert_ne!(elsewhere.code, 0, "profile 找不到時不能核准：{}{}", elsewhere.out, elsewhere.err);
    assert!(!approved(&root, "test"));
    // 人工核准也一樣：有 recipe 的 run 找不到 profile 就不能重算，不採信
    let human = awf(&["save-run", "approve", RUN, "test", "--by", "human", "--at", "t"], &root, None, &[("CLAUDE_PROJECT_DIR", &s(&root.join("repo")))]);
    assert_ne!(human.code, 0, "{}{}", human.out, human.err);
    assert!(human.err.contains("profile"), "{}", human.err);
    assert!(!approved(&root, "test"));
}

/// H1 重現 C（手改）：run.json 的 `recipe.resolved` 被改鬆，在子目錄執行一樣要被重算擋下。
#[test]
fn h1_save_run_tampered_recipe_is_caught_from_a_subdir() {
    let t = Tmp::new("h1c2");
    let root = scene_c(&t);
    let f = root.join(RUN).join("run.json");
    let mut rj = read_json(&f);
    rj["recipe"]["resolved"]["test"]["security_review"] = json!("auto");
    write(&f, &format!("{}\n", serde_json::to_string_pretty(&rj).expect("JSON")));
    let o = awf(&["save-run", "approve", "../_runs/r1", "test", "--by", "human", "--at", "t"], &root.join("repo"), None, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.err.contains("recipe"), "{}", o.err);
    assert!(!approved(&root, "test"));
}

/// H1：save-run 的 stage／approve 找不到專案根時報錯。
#[test]
fn h1_save_run_without_a_project_root_refuses() {
    let t = Tmp::new("h1d");
    let root = scene_c(&t);
    std::fs::remove_file(root.join(".claude/teams/VERSION")).expect("刪 VERSION");
    let o = awf(&["save-run", "approve", RUN, "test", "--by", "human", "--at", "t"], &root, None, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.err.contains("找不到專案根"), "{}", o.err);
    let r = traced(&root, "test", json!({"v": test_env()}), &[("verify", "agent-workflow:dev-verifier")]);
    let o = sr(&root, &["stage", RUN, "test"], Some(&r.to_string()));
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.err.contains("找不到專案根"), "{}", o.err);
}

// ------------------------------------------------------------------ M3：save-run stage 的回傳讀不懂

/// M3 重現：test 段已經通過並核准；重跑後的結果是 fail，但回傳裡有工具讀不懂的值（超大整數、NaN、巢狀太深）。
/// 不能留下舊的 pass 與舊核准：該段要標 invalid、核准清掉，之後的核准（這一段與後面的段）都過不了。
#[test]
fn m3_unreadable_stage_result_invalidates_the_stage() {
    let fail = r#"{"v": {"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [],
        "payload": {"commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
        "criteria": [{"id": "C1", "criterion": "c", "command": "x", "result": "fail", "evidence": "e"}]}}, "extra": @X@}"#;
    let deep = format!("{}1{}", "[".repeat(300), "]".repeat(300));
    for (tag, bad) in [("huge", "100000000000000000000"), ("nan", "NaN"), ("deep", deep.as_str()), ("cut", "{\"a\": ")] {
        let t = Tmp::new(&format!("m3-{tag}"));
        let root = t.path().join("proj");
        mk_run_project(&root, &builtin_profile());
        baseline(&root, "deliver");
        assert!(approved(&root, "test"), "前提：test 已核准");
        let before = std::fs::read_to_string(root.join(RUN).join("test.json")).expect("test.json");
        let o = sr(&root, &["stage", RUN, "test"], Some(&fail.replace("@X@", bad)));
        assert_ne!(o.code, 0, "{tag}：回傳讀不懂要失敗：{}{}", o.out, o.err);
        assert!(o.err.contains("讀不懂"), "{tag}：要說明是回傳讀不懂：{}", o.err);
        let run = read_json(&root.join(RUN).join("run.json"));
        assert_eq!(run["stages"]["test"]["status"], "invalid", "{tag}：該段要標 invalid");
        assert!(!approved(&root, "test"), "{tag}：舊核准要清掉");
        // 舊的結果檔不能原封不動留著當成這一段的結果
        let after = std::fs::read_to_string(root.join(RUN).join("test.json")).unwrap_or_default();
        assert_ne!(after, before, "{tag}：舊的 test.json 不能留著");
        for by in ["policy", "human"] {
            let o = sr(&root, &["approve", RUN, "test", "--by", by, "--note", "p", "--at", "t"], None);
            assert_ne!(o.code, 0, "{tag}：test 不能再核准（{by}）：{}{}", o.out, o.err);
        }
        // 後面的 deliver 也核准不了（閘門鏈）
        let o = sr(&root, &["approve", RUN, "deliver", "--by", "policy", "--note", "p", "--at", "t"], None);
        assert_ne!(o.code, 0, "{tag}：deliver 的政策核准要被擋：{}{}", o.out, o.err);
        assert!(!approved(&root, "deliver"));
    }
}

/// M3：run.json 沒有那一段時，讀不懂的回傳不能讓 run.json 多出一段。
#[test]
fn m3_run_json_is_untouched_when_the_stage_is_unknown() {
    // 段名不是這個 run 有的段、或 run.json 本身讀不到：不能因為回傳讀不懂就憑空多出一段
    let t = Tmp::new("m3-unknown");
    let root = t.path().join("proj");
    mk_run_project(&root, &builtin_profile());
    baseline(&root, "plan");
    let before = std::fs::read_to_string(root.join(RUN).join("run.json")).expect("run.json");
    let o = sr(&root, &["stage", RUN, "no-such-stage"], Some("{\"a\": NaN}"));
    assert_ne!(o.code, 0);
    assert_eq!(std::fs::read_to_string(root.join(RUN).join("run.json")).expect("run.json"), before);
}


/// M3：`awf done --output <檔>` 給的結果檔在、但讀不懂時也一樣（走的是狀態機，不經過 save-run 的命令列）。
#[test]
fn m3_awf_done_with_an_unreadable_output_file_invalidates_the_stage() {
    let t = Tmp::new("m3-done");
    let root = t.path().join("proj");
    mk_run_project(&root, &builtin_profile());
    baseline(&root, "deliver");
    assert!(approved(&root, "test"), "前提：test 已核准");
    write(&root.join("bad.json"), "{\"v\": {\"status\": \"done\"}, \"n\": 100000000000000000000}");
    let o = awf(&["done", "--run", RUN, "--stage", "test", "--output", "bad.json"], &root, None, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    let run = read_json(&root.join(RUN).join("run.json"));
    assert_eq!(run["stages"]["test"]["status"], "invalid", "該段要標 invalid：{}{}", o.out, o.err);
    assert!(!approved(&root, "test"), "舊核准要清掉");
    // 檔不存在（打錯路徑）不算「結果讀不懂」：什麼都不改
    let t2 = Tmp::new("m3-done-typo");
    let root2 = t2.path().join("proj");
    mk_run_project(&root2, &builtin_profile());
    baseline(&root2, "deliver");
    let o = awf(&["done", "--run", RUN, "--stage", "test", "--output", "no-such.json"], &root2, None, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(approved(&root2, "test"), "路徑打錯不該讓核准失效");
}


// ------------------------------------------------------------------ 低：自訂 id_pattern 的「字」、plan.json 讀不懂

/// 自訂的 `requirements.id_pattern`：`\b`、`\w` 對「字」的看法要和 0.x 一樣（結合符號不算字）。
/// 需求文件裡編號後面緊接一個結合符號：0.x 認得出這條需求；之前這裡認不出來、整條需求從清單消失，沒被對照也判 merge。
#[test]
fn low_custom_id_pattern_treats_combining_marks_like_python() {
    let t = Tmp::new("low-idp");
    let root = t.path().join("proj");
    mk_project(&root);
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs",
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "high"}, "daily_cap": 99},
        "requirements": {"source": "spec-md", "path": "reqs.md", "id_pattern": "^- (REQ-\\d+)\\b"}}));
    write(&root.join("reqs.md"), "- REQ-1 匯出按鈕下載 CSV\n- REQ-2\u{301} 超過一萬筆時提示\n");
    let inp = t.path().join("in");
    write_json(&inp.join("d.json"), &decision(json!([row("REQ-1", "匯出按鈕下載 CSV")])));
    write_json(&inp.join("e.json"), &json!({"commit": SHA, "results": [{"requirement_id": "REQ-1", "result": "pass"}]}));
    write_json(&inp.join("c.json"), &json!({"ci": "success"}));
    write(&inp.join("diff.txt"), &mk_diff("src/a.py", &["x = 1"]));
    let (o, v) = gate(&gate_args(&inp, "d.json", "e.json", "diff.txt"), &["--reviewed-sha", SHA], &root, &[]);
    assert_ne!(v["decision"], "merge", "REQ-2 沒被對照，不能判 merge：{}{}", o.out, o.err);
    assert!(o.out.contains("REQ-2"), "要指出 REQ-2 沒被對照：{}", o.out);
    // 對照組：兩條都對照到就可以 merge（確認上面擋下來的原因是 REQ-2，不是別的）
    write_json(&inp.join("d2.json"), &decision(json!([row("REQ-1", "匯出按鈕下載 CSV"), row("REQ-2", "超過一萬筆時提示")])));
    write_json(&inp.join("e2.json"), &json!({"commit": SHA, "results": [{"requirement_id": "REQ-1", "result": "pass"}, {"requirement_id": "REQ-2", "result": "pass"}]}));
    let (o, v) = gate(&gate_args(&inp, "d2.json", "e2.json", "diff.txt"), &["--reviewed-sha", SHA], &root, &[]);
    assert_eq!(v["decision"], "merge", "{}{}", o.out, o.err);
    // 寫壞的樣式照舊是錯誤（不是當成沒有需求）
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs",
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "high"}, "daily_cap": 99},
        "requirements": {"source": "spec-md", "path": "reqs.md", "id_pattern": "^- (REQ-\\d+"}}));
    let (o, v) = gate(&gate_args(&inp, "d2.json", "e2.json", "diff.txt"), &["--reviewed-sha", SHA], &root, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert_ne!(v["decision"], "merge", "{}{}", o.out, o.err);
}

/// plan.json 讀不懂（NaN、巢狀太深）時，epic 不能當成沒有而跳過拆工檢查的 spec 對照：記成一條問題，政策核准因此不過。
#[test]
fn low_unreadable_plan_json_blocks_policy_approval_of_design() {
    let deep = format!("{{\"epic_dir\": \"specs/e\", \"x\": {}1{}}}", "[".repeat(300), "]".repeat(300));
    for (tag, text) in [("nan", "{\"epic_dir\": \"specs/e\", \"x\": NaN}".to_string()), ("deep", deep), ("cut", "{\"epic_dir\": ".to_string())] {
        let t = Tmp::new(&format!("low-plan-{tag}"));
        let root = t.path().join("proj");
        mk_run_project(&root, &builtin_profile());
        baseline(&root, "plan");
        must(sr(&root, &["approve", RUN, "plan", "--by", "human", "--at", "t"], None), "approve plan");
        // 對照組：plan.json 正常時 design 可以政策核准
        let r = traced(&root, "design", json!({"design": design_env()}), &[]);
        must(sr(&root, &["stage", RUN, "design"], Some(&r.to_string())), "stage design");
        let ok = sr(&root, &["approve", RUN, "design", "--by", "policy", "--note", "p", "--at", "t"], None);
        assert_eq!(ok.code, 0, "{tag}：對照組應該可以政策核准：{}{}", ok.out, ok.err);
        // plan.json 壞掉之後重存 design：要記成問題，政策核准不過
        write(&root.join(RUN).join("plan.json"), &text);
        let o = sr(&root, &["stage", RUN, "design"], Some(&r.to_string()));
        assert!(o.err.contains("plan.json"), "{tag}：存檔時要指出 plan.json 讀不懂：{}{}", o.out, o.err);
        let o = sr(&root, &["approve", RUN, "design", "--by", "policy", "--note", "p", "--at", "t"], None);
        assert_ne!(o.code, 0, "{tag}：plan.json 讀不懂時不能政策核准：{}{}", o.out, o.err);
        assert!(o.err.contains("plan.json"), "{tag}：{}", o.err);
        assert!(!approved(&root, "design"));
    }
}


// ------------------------------------------------------------------ run-brief --log 只能寫 run 目錄裡的 log

/// 審查的重現：`awf run-brief --log <任意路徑> -- echo <內容>` 可以把內容原樣寫進工具自己的狀態檔。
/// `--log` 的目標只能是 runs_dir 底下某個 run 目錄裡的 .md／.log／.txt，而且不是狀態檔、不在 archive/ 底下。
#[test]
fn run_brief_log_cannot_overwrite_state_files() {
    let t = Tmp::new("rb");
    let root = t.path().join("proj");
    mk_project(&root);
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs"}));
    let run = root.join("_tpm/runs/r1");
    write(&run.join("run.json"), "{\"keep\": 1}\n");
    write(&root.join("_tpm/runs/merge-audit.jsonl"), "keep\n");
    write(&run.join("archive/結案摘要.md"), "keep\n");
    write(&run.join("context.md"), "keep\n");
    let rb = |log: &str, cwd: &Path| awf(&["run-brief", "--log", log, "--", "echo", "{\"decision\": \"merge\"}"], cwd, None, &[]);
    let refused = [
        "_tpm/runs/r1/run.json",
        "_tpm/runs/merge-audit.jsonl",
        "_tpm/runs/r1/links.jsonl",
        "_tpm/runs/decisions.jsonl",
        "_tpm/runs/r1/plan/reuse.json",
        "_tpm/runs/r1/plan/pack-args.json",
        "_tpm/runs/r1/archive/結案摘要.md",
        "_tpm/runs/r1/archive/x.log",
        "_tpm/runs/r1/context.md",
        "_tpm/runs/r1/plan/logs/a.sh",
        "_tpm/runs/r1/plan/logs/a",
        "_tpm/runs/x.log",
        ".claude/teams/guard.config.json",
        ".claude/teams/notes.md",
        "src/a.log",
        "_tpm/runs/r1/../../../.claude/teams/x.md",
        "_tpm/runs/r1/plan/../run.json",
        "_tpm/runs/r1/RUN.JSON",
        "_tpm/runs/r1/Archive/x.md",
    ];
    for log in refused {
        let o = rb(log, &root);
        assert_ne!(o.code, 0, "{log} 應該被拒絕：{}{}", o.out, o.err);
        assert!(o.err.contains("--log"), "{log}：要說明原因：{}", o.err);
        assert!(!o.out.contains("decision"), "{log}：被拒絕時不能執行指令：{}", o.out);
    }
    let outside = t.path().join("outside.log");
    let o = rb(&s(&outside), &root);
    assert_ne!(o.code, 0, "專案外的路徑應該被拒絕：{}{}", o.out, o.err);
    assert!(!outside.exists());
    // 原本的內容都沒被動到，被拒絕的目標也沒有被建立
    assert_eq!(std::fs::read_to_string(run.join("run.json")).expect("run.json"), "{\"keep\": 1}\n");
    assert_eq!(std::fs::read_to_string(root.join("_tpm/runs/merge-audit.jsonl")).expect("audit"), "keep\n");
    assert_eq!(std::fs::read_to_string(run.join("archive/結案摘要.md")).expect("摘要"), "keep\n");
    assert_eq!(std::fs::read_to_string(run.join("context.md")).expect("context"), "keep\n");
    assert_eq!(std::fs::read_to_string(root.join(".claude/teams/guard.config.json")).expect("設定"), json!({"runs_dir": "_tpm/runs"}).to_string());
    for p in ["_tpm/runs/r1/links.jsonl", "_tpm/runs/decisions.jsonl", "src/a.log", ".claude/teams/notes.md", "_tpm/runs/x.log"] {
        assert!(!root.join(p).exists(), "{p} 不該被建立");
    }
    // 符號連結指到 run 目錄外面：照實際位置判
    #[cfg(unix)]
    {
        std::fs::create_dir_all(root.join("elsewhere")).expect("建立目錄");
        std::os::unix::fs::symlink(root.join("elsewhere"), run.join("link")).expect("建立連結");
        let o = rb("_tpm/runs/r1/link/a.log", &root);
        assert_ne!(o.code, 0, "經過符號連結寫到外面應該被拒絕：{}{}", o.out, o.err);
        assert!(!root.join("elsewhere/a.log").exists());
        std::os::unix::fs::symlink(run.join("run.json"), run.join("alias.log")).expect("建立連結");
        let o = rb("_tpm/runs/r1/alias.log", &root);
        assert_ne!(o.code, 0, "目標是符號連結應該被拒絕：{}{}", o.out, o.err);
        assert_eq!(std::fs::read_to_string(run.join("run.json")).expect("run.json"), "{\"keep\": 1}\n");
    }
}

/// 既有用法不能被擋：派工單指定的 `--log <run 目錄>/<段>/logs/<label>.log`（相對、絕對、從子目錄執行），以及不帶 `--log`。
#[test]
fn run_brief_log_inside_a_run_directory_still_works() {
    let t = Tmp::new("rb-ok");
    let root = t.path().join("proj");
    mk_project(&root);
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs"}));
    std::fs::create_dir_all(root.join("_tpm/runs/r1")).expect("建立 run 目錄");
    std::fs::create_dir_all(root.join("repo")).expect("建立子目錄");
    let rb = |log: &str, cwd: &Path| awf(&["run-brief", "--log", log, "--", "echo", "hello"], cwd, None, &[]);
    let abs = s(&root.join("_tpm/runs/r1/build/logs/c.log"));
    for (log, cwd, file) in [
        ("_tpm/runs/r1/plan/logs/a.log", root.clone(), "_tpm/runs/r1/plan/logs/a.log"),
        ("../_tpm/runs/r1/test/logs/b.log", root.join("repo"), "_tpm/runs/r1/test/logs/b.log"),
        (abs.as_str(), root.join("repo"), "_tpm/runs/r1/build/logs/c.log"),
        ("_tpm/runs/r1/test/logs/verify.txt", root.clone(), "_tpm/runs/r1/test/logs/verify.txt"),
        ("_tpm/runs/r1/test/輸出.md", root.clone(), "_tpm/runs/r1/test/輸出.md"),
    ] {
        let o = rb(log, &cwd);
        assert_eq!(o.code, 0, "{log} 應該可以寫：{}{}", o.out, o.err);
        assert_eq!(std::fs::read_to_string(root.join(file)).expect("log 檔"), "hello\n", "{log}");
    }
    // 沒有設定檔時用預設的 runs_dir（_tpm/runs）
    std::fs::remove_file(root.join(".claude/teams/guard.config.json")).expect("刪設定");
    let o = rb("_tpm/runs/r1/plan/logs/d.log", &root);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    // 不帶 --log：寫到 AWF_LOG_DIR，不受限制
    let logs = t.path().join("logs");
    let o = awf(&["run-brief", "--name", "demo", "--", "echo", "hello"], &root, None, &[("AWF_LOG_DIR", &s(&logs))]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(logs.join("demo-1.log").is_file());
}
