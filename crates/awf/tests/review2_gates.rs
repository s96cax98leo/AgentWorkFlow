//! 1.0 第二次獨立審查（新功能）發現的閘門端問題：每一則一組測試，輸入取自審查的重現。
//!
//! - 第 2 則：規格檢查的基準沒釘住（核准之後把沒過的情境從變更的差異檔刪掉，檢查就過）。
//! - 第 3 則：`links.enforce` 只檢「有宣告」，不檢真假。
//! - 第一次審查沒做完的：有 recipe 的 run 找不到 profile 時，存檔當下就要拒絕。
//!
//! 一律用真的執行檔、乾淨的環境（拿掉 `CLAUDE_*`、`AWF_*`）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RUN: &str = "_runs/r1";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("awf-review2-{}-{}-{}", std::process::id(), n, tag));
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

struct Out {
    code: i32,
    out: String,
    err: String,
}

impl Out {
    fn all(&self) -> String {
        format!("{}{}", self.out, self.err)
    }
}

fn awf(args: &[&str], cwd: &Path, stdin: Option<&str>) -> Out {
    let mut c = Command::new(EXE);
    c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    let mut child = c.spawn().expect("執行 awf");
    {
        let mut pipe = child.stdin.take().expect("stdin");
        let _ = pipe.write_all(stdin.unwrap_or("").as_bytes());
    }
    let o = child.wait_with_output().expect("等 awf 結束");
    Out { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() }
}

fn sr(root: &Path, args: &[&str], stdin: Option<&str>) -> Out {
    let mut a = vec!["save-run"];
    a.extend(args);
    awf(&a, root, stdin)
}

fn must(o: Out, what: &str) {
    assert_eq!(o.code, 0, "{what} 應該成功：{}", o.all());
}

fn envelope(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

fn builtin_profile(extra: Value) -> Value {
    let d = read_json(&scaffold_teams().join("methodologies/default.json"));
    let mut p = json!({"recipes": d["recipes"], "modifiers": d["modifiers"]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    p
}

const INTENT: &str = "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n";
const ORDERS: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n";
const DELTA: &str = "## ADDED Requirements\n\n### Requirement: 申請退款\n系統 SHALL 讓會員在七天內申請退款。\n\n#### Scenario: 七天內\n- **WHEN** 收貨未滿七天\n- **THEN** 受理退款\n\n#### Scenario: 超過七天\n- **WHEN** 收貨超過七天\n- **THEN** 拒絕並說明\n";
const OVER7_BLOCK: &str = "\n#### Scenario: 超過七天\n- **WHEN** 收貨超過七天\n- **THEN** 拒絕並說明\n";
const MODIFIED: &str = "\n## MODIFIED Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單並寄確認信。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立並寄確認信\n";
const PROPOSAL: &str = "# 退款\n\n## Why\n示範客戶 ACME 的會員要能退款。\n\n## What Changes\n- 訂單加退款申請\n\n## Non-Goals\n- 不做部分退款\n";
const IN7: &str = "orders/申請退款#七天內";
const OVER7: &str = "orders/申請退款#超過七天";

/// 一個 init 過的專案：schemas、VERSION、profile；`openspec` 為真時放一份規格與進行中的變更 `refund`。
fn mk(root: &Path, profile: &Value, openspec: bool) {
    let teams = root.join(".claude/teams");
    copy_dir(&scaffold_teams().join("schemas"), &teams.join("schemas"));
    write(&teams.join("VERSION"), "1.0.0\n");
    write_json(&teams.join("methodology.json"), profile);
    write(&root.join("repo/.worktrees/refund/specs/refund/intent.md"), INTENT);
    std::fs::create_dir_all(root.join("_runs")).expect("建立 _runs");
    if openspec {
        write(&root.join("openspec/specs/orders/spec.md"), ORDERS);
        write(&root.join("openspec/changes/refund/proposal.md"), PROPOSAL);
        write(&root.join("openspec/changes/refund/tasks.md"), "## 1. 實作\n- [ ] 1.1 退款申請\n");
        write(&root.join("openspec/changes/refund/specs/orders/spec.md"), DELTA);
    }
}

fn delta_file(root: &Path) -> PathBuf {
    root.join("openspec/changes/refund/specs/orders/spec.md")
}

fn run_json(root: &Path) -> Value {
    read_json(&root.join(RUN).join("run.json"))
}

fn approved(root: &Path, stage: &str) -> bool {
    run_json(root)["stages"][stage].get("approved").is_some()
}

/// 補上 recipe 與 trace（紀錄檔一併寫好）。
fn traced(root: &Path, stage: &str, result: Value, entries: &[(&str, &str)]) -> Value {
    let data = run_json(root);
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

fn save(root: &Path, stage: &str, result: Value, entries: &[(&str, &str)]) -> Out {
    let r = traced(root, stage, result, entries);
    sr(root, &["stage", RUN, stage], Some(&r.to_string()))
}

fn approve(root: &Path, stage: &str, by: &str) -> Out {
    sr(root, &["approve", RUN, stage, "--by", by, "--note", "p", "--at", "t"], None)
}

fn task(acc: &[&str], refs: &[&str], owns: &[&str]) -> Value {
    json!({"id": "t1", "title": "t1", "kind": "code", "owns": owns, "depends_on": [], "acceptance": acc, "spec_refs": refs})
}

fn design_of(subs: Value) -> Value {
    json!({"design": envelope(json!({"epic": "refund", "subtasks": subs})), "challenge": envelope(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "查過"}]}))})
}

fn spec_design() -> Value {
    design_of(json!([task(&[&format!("{IN7} 受理"), &format!("{OVER7} 拒絕")], &["orders/申請退款"], &["src/refund.py"])]))
}

fn build_of(files: &[&str]) -> Value {
    json!({"x": envelope(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": files, "tests": []}))})
}

fn test_of(commit: &str, rows: &[(&str, &str)]) -> Value {
    let crit: Vec<Value> = rows.iter().map(|(id, cmd)| json!({"id": id, "criterion": "c", "command": cmd, "result": "pass", "evidence": "e"})).collect();
    json!({"verifier": envelope(json!({"commit": commit, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": crit}))})
}

const VERIFIER: &[(&str, &str)] = &[("verify", "agent-workflow:dev-verifier")];
const AXES: &[(&str, &str)] = &[("review:standards", "x"), ("review:spec", "x"), ("review:bugs", "x"), ("review:history", "x")];

/// init 與 plan 存檔（recipe feature）；`openspec` 為假時信封列出 intent.md。
fn plan(root: &Path, openspec: bool) {
    must(sr(root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    let docs = if openspec { json!([]) } else { json!([{"path": "specs/refund/intent.md", "kind": "intent", "audience": "owner"}]) };
    let plan = json!({"stage": "plan", "epic_dir": "repo/.worktrees/refund", "recipe": {"name": "feature", "modifiers": [], "size": ""}, "intent": envelope(json!({"documents": docs}))});
    must(sr(root, &["stage", RUN, "plan"], Some(&plan.to_string())), "stage plan");
}

/// plan 核准 → design 存檔並由人核准 → build、sync 存檔。
fn upto_sync(root: &Path, openspec: bool, design: Value, files: &[&str]) {
    plan(root, openspec);
    must(approve(root, "plan", "human"), "approve plan");
    must(save(root, "design", design, &[]), "stage design");
    must(approve(root, "design", "human"), "approve design");
    must(save(root, "build", build_of(files), &[]), "stage build");
    must(save(root, "sync", build_of(files), &[]), "stage sync");
}

fn openspec_profile() -> Value {
    builtin_profile(json!({"requirements": {"source": "openspec", "path": "openspec"}}))
}

fn issues(root: &Path, stage: &str) -> Vec<String> {
    run_json(root)["stages"][stage]["spec_issues"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

// ------------------------------------------------------------------ 第 2 則：規格檢查的基準

/// 重現：design 核准之後，把沒過的情境從變更的差異檔刪掉，test 段只交剩下那一條的結果。
/// 之前：規格檢查照「現在的差異檔」算，政策核准通過。現在：和 design 核准時記下的基準比，條目變少、差異檔變了都算問題。
#[test]
fn r2_removing_a_scenario_after_design_approval_blocks_policy_approval() {
    let t = Tmp::new("r2-rm");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    upto_sync(&root, true, spec_design(), &["src/refund.py"]);
    let base = run_json(&root)["stages"]["design"]["approved"]["spec"].clone();
    assert!(base["items"].get(OVER7).is_some() && base["files"].get("specs/orders/spec.md").is_some(), "design 核准時要記下條目與差異檔的雜湊：{base}");
    assert!(run_json(&root)["stages"]["plan"]["approved"]["spec"]["proposal"].as_str().is_some_and(|x| x.len() == 64), "plan 核准時要記下 proposal 的雜湊");
    // agent 把沒過的情境刪掉
    write(&delta_file(&root), &DELTA.replace(OVER7_BLOCK, ""));
    let o = save(&root, "test", test_of(SHA, &[(IN7, "pytest tests/test_refund.py::test_in7")]), VERIFIER);
    let si = issues(&root, "test");
    assert!(si.iter().any(|m| m.contains("少了") && m.contains(OVER7)), "要指出少了哪一條：{si:?}\n{}", o.all());
    assert!(si.iter().any(|m| m.contains("差異檔") && m.contains("specs/orders/spec.md")), "要指出哪個差異檔變了：{si:?}");
    let o = approve(&root, "test", "policy");
    assert_ne!(o.code, 0, "政策核准要被擋：{}", o.all());
    assert!(o.err.contains("規格檢查") && o.err.contains(OVER7), "{}", o.err);
    assert!(!approved(&root, "test"));
    // 人工核准只警告
    let o = approve(&root, "test", "human");
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.err.contains("規格檢查"), "人工核准要看得到警告：{}", o.err);
}

/// 差異檔內容變了（條目沒少）也算；改回去就過。合法的修訂路徑是重跑 design 段：重存、重新核准之後基準跟著換。
#[test]
fn r2_changed_delta_file_blocks_until_design_is_rerun() {
    let t = Tmp::new("r2-edit");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    upto_sync(&root, true, spec_design(), &["src/refund.py"]);
    let both = [(IN7, "pytest tests/test_refund.py::test_in7"), (OVER7, "pytest tests/test_refund.py::test_over7")];
    must(save(&root, "test", test_of(SHA, &both), VERIFIER), "stage test");
    assert!(issues(&root, "test").is_empty(), "沒動規格時沒有問題：{:?}", issues(&root, "test"));
    // 存檔之後才改：政策核准重算，不採信 run.json 記的結果
    write(&delta_file(&root), &DELTA.replace("拒絕並說明", "拒絕"));
    let o = approve(&root, "test", "policy");
    assert_ne!(o.code, 0, "差異檔改過，政策核准要被擋：{}", o.all());
    assert!(o.err.contains("差異檔"), "{}", o.err);
    write(&delta_file(&root), DELTA);
    must(approve(&root, "test", "policy"), "改回去之後政策核准");
    // 走合法路徑：改規格 → 重存 design（核准清掉）→ 重新核准 → 基準換成新的
    write(&delta_file(&root), &DELTA.replace("拒絕並說明", "拒絕"));
    must(save(&root, "design", spec_design(), &[]), "重存 design");
    assert!(!approved(&root, "design"), "重存 design 會清掉核准（基準跟著清掉）");
    must(approve(&root, "design", "human"), "重新核准 design");
    must(save(&root, "build", build_of(&["src/refund.py"]), &[]), "stage build");
    must(save(&root, "sync", build_of(&["src/refund.py"]), &[]), "stage sync");
    must(save(&root, "test", test_of(SHA, &both), VERIFIER), "stage test");
    must(approve(&root, "test", "policy"), "基準換新之後政策核准");
}

/// `ids.json` 被改、讓基準裡的 id 對不到原本那一條：算問題。
#[test]
fn r2_ids_json_edit_that_moves_a_baseline_id_is_an_issue() {
    let t = Tmp::new("r2-ids");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    write(&delta_file(&root), &format!("{DELTA}{MODIFIED}"));
    must(awf(&["spec", "ids", "--write"], &root, None), "spec ids --write");
    let modified = "orders/建立訂單#庫存足夠";
    let design = design_of(json!([task(&[&format!("{IN7} 受理"), &format!("{OVER7} 拒絕"), &format!("{modified} 寄信")], &["orders/申請退款"], &["src/refund.py"])]));
    upto_sync(&root, true, design, &["src/refund.py"]);
    assert!(run_json(&root)["stages"]["design"]["approved"]["spec"]["items"].get(modified).is_some());
    // agent 改 ids.json：現行需求的 id 換掉，MODIFIED 那一條跟著換 id，逐條結果用新 id 就「對得上」
    let ids = root.join("openspec/ids.json");
    let text = std::fs::read_to_string(&ids).expect("ids.json");
    write(&ids, &text.replace("\"orders/建立訂單\"", "\"orders/下單\""));
    let rows = [(IN7, "pytest tests/a.py"), (OVER7, "pytest tests/b.py"), ("orders/下單#庫存足夠", "pytest tests/c.py")];
    let _ = save(&root, "test", test_of(SHA, &rows), VERIFIER);
    let si = issues(&root, "test");
    assert!(si.iter().any(|m| m.contains(modified)), "基準裡的 id 不見了要列出來：{si:?}");
    let o = approve(&root, "test", "policy");
    assert_ne!(o.code, 0, "{}", o.all());
}

/// plan 核准之後 proposal 被改：design 段的規格檢查列出來，政策核准不過。
#[test]
fn r2_proposal_changed_after_plan_approval_is_an_issue_at_design() {
    let t = Tmp::new("r2-prop");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    plan(&root, true);
    must(approve(&root, "plan", "human"), "approve plan");
    write(&root.join("openspec/changes/refund/proposal.md"), &PROPOSAL.replace("不做部分退款", "部分退款也做"));
    must(save(&root, "design", spec_design(), &[]), "stage design");
    let si = issues(&root, "design");
    assert!(si.iter().any(|m| m.contains("proposal")), "{si:?}");
    let o = approve(&root, "design", "policy");
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(o.err.contains("proposal"), "{}", o.err);
}

/// deliver 段也重算（之前規格檢查只在 design、test）。
#[test]
fn r2_deliver_recomputes_against_the_baseline() {
    let t = Tmp::new("r2-deliver");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    upto_sync(&root, true, spec_design(), &["src/refund.py"]);
    let both = [(IN7, "pytest tests/test_refund.py::test_in7"), (OVER7, "pytest tests/test_refund.py::test_over7")];
    must(save(&root, "test", test_of(SHA, &both), VERIFIER), "stage test");
    must(approve(&root, "test", "human"), "approve test");
    must(save(&root, "deliver", build_of(&[]), AXES), "stage deliver");
    must(approve(&root, "deliver", "policy"), "沒動規格：deliver 政策核准");
    write(&delta_file(&root), &DELTA.replace(OVER7_BLOCK, ""));
    must(save(&root, "deliver", build_of(&[]), AXES), "重存 deliver");
    assert!(issues(&root, "deliver").iter().any(|m| m.contains(OVER7)), "{:?}", issues(&root, "deliver"));
    let o = approve(&root, "deliver", "policy");
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(o.err.contains("規格檢查"), "{}", o.err);
}

/// 沒選 openspec：run.json 不多任何鍵。
#[test]
fn r2_no_baseline_keys_without_openspec() {
    let t = Tmp::new("r2-off");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({})), false);
    let design = design_of(json!([task(&["AC-1 下載"], &[], &["src/a.py"])]));
    upto_sync(&root, false, design, &["src/a.py"]);
    let run = run_json(&root);
    for st in ["plan", "design"] {
        let keys: Vec<&String> = run["stages"][st]["approved"].as_object().expect("approved").keys().collect();
        assert_eq!(keys, ["by", "at", "note", "kind"], "{st} 的核准紀錄不能多鍵");
    }
}

// ------------------------------------------------------------------ 第 2 則在 merge 段

fn decision(rows: Value) -> Value {
    json!({"status": "done", "summary": "示範", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [],
        "payload": {"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": SHA, "requirements_source": "openspec",
            "traceability": rows, "out_of_scope": [], "followups": [], "risk_tier": "low", "gate_reasons": []}})
}

fn row(id: &str) -> Value {
    json!({"requirement_id": id, "requirement": id, "evidence": [format!("results.json {id}")], "result": "match"})
}

fn gate(root: &Path, inp: &Path, ids: &[&str], extra: &[&str]) -> (Out, Value) {
    write_json(&inp.join("d.json"), &decision(Value::Array(ids.iter().map(|i| row(i)).collect())));
    write_json(&inp.join("e.json"), &json!({"commit": SHA, "results": ids.iter().map(|i| json!({"requirement_id": i, "result": "pass", "evidence": "1 passed"})).collect::<Vec<_>>()}));
    write_json(&inp.join("c.json"), &json!({"ci": "success"}));
    write(&inp.join("diff.txt"), "diff --git a/tests/test_refund.py b/tests/test_refund.py\n--- a/tests/test_refund.py\n+++ b/tests/test_refund.py\n@@ -0,0 +1,2 @@\n+def test_in7():\n+    assert refund() == 'ok'\n");
    let f = |n: &str| s(&inp.join(n));
    let (d, e, c, df) = (f("d.json"), f("e.json"), f("c.json"), f("diff.txt"));
    let mut a: Vec<&str> = vec!["merge-gate", "check", "--decision", &d, "--repo", "acme-fork/app", "--pr", "3", "--head-sha", SHA, "--reviewed-sha", SHA, "--evidence", &e, "--checks", &c, "--diff", &df,
        "--tests-before", "1", "--tests-after", "1", "--timestamp", "2099-10-24T09:00:00+00:00", "--no-audit"];
    a.extend(extra);
    let o = awf(&a, root, None);
    let v = serde_json::from_str(&o.out).unwrap_or(Value::Null);
    (o, v)
}

/// merge 段：`awf merge-gate` 的需求清單取自專案工作目錄的差異檔（agent 改得到）。
/// 帶 `--recipe <run.json>` 時和 run.json 記下的條目比：條目少了就 hold。
#[test]
fn r2_merge_gate_holds_when_spec_items_shrink_after_design_approval() {
    let t = Tmp::new("r2-gate");
    let root = t.path().join("proj");
    mk(&root, &openspec_profile(), true);
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"runs_dir": "_runs", "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "high"}, "daily_cap": 9}}));
    upto_sync(&root, true, spec_design(), &["src/refund.py"]);
    let inp = t.path().join("in");
    let run = s(&root.join(RUN).join("run.json"));
    let (o, v) = gate(&root, &inp, &[IN7, OVER7], &["--recipe", &run]);
    assert_eq!(v["decision"], "merge", "前提：規格沒動、兩條都對照 → merge：{}", o.all());
    // agent 刪掉一條，steward 只對照剩下那一條
    write(&delta_file(&root), &DELTA.replace(OVER7_BLOCK, ""));
    let (o, v) = gate(&root, &inp, &[IN7], &["--recipe", &run]);
    assert_eq!(v["decision"], "hold", "條目比 design 核准時少，要 hold：{}", o.all());
    assert!(o.out.contains(OVER7), "理由要指出少了哪一條：{}", o.out);
}
