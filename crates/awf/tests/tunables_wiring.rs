//! 「可設定的值兩邊都要接上」的測試（第 12 批第二輪）：範本認得的設定，閘門端也要照同一份設定判。
//!
//! - 限制對照的用語（`constraints_format`）：派工單（challenge 片段）要 agent 寫的節名與判定詞，`awf save-run stage` 認得。
//! - 驗收條件編號的樣式（`stages.design.criteria_id_pattern`）：連結檢查和拆工檢查用同一個來源。
//! - 換角色（`roles`）與合併角色（`guard.config.json` 的 `auto_merge.merger_role`）：模型下限照被頂替的角色算，
//!   不採信 trace 自己記的 `role_as`。
//!
//! 一律用真的執行檔、乾淨的環境（拿掉 `CLAUDE_*`、`AWF_*`）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const RUN: &str = "_runs/r1";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GHOST: &str = "由 save_run 代寫";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("awf-wiring-{}-{}-{}", std::process::id(), n, tag));
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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scaffold_teams() -> PathBuf {
    repo_root().join("plugin/scaffold/.claude/teams")
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

fn sr(root: &Path, args: &[&str], stdin: Option<&str>) -> Out {
    let mut c = Command::new(EXE);
    c.arg("save-run").args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
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

fn must(o: Out, what: &str) -> Out {
    assert_eq!(o.code, 0, "{what} 應該成功：{}", o.all());
    o
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

/// 一個 init 過的專案：schemas、VERSION、profile。
fn mk(root: &Path, profile: &Value) {
    let teams = root.join(".claude/teams");
    copy_dir(&scaffold_teams().join("schemas"), &teams.join("schemas"));
    write(&teams.join("VERSION"), "1.0.0\n");
    write_json(&teams.join("methodology.json"), profile);
    write(&root.join("repo/.worktrees/refund/specs/refund/intent.md"), INTENT);
    std::fs::create_dir_all(root.join("_runs")).expect("建立 _runs");
}

fn run_json(root: &Path) -> Value {
    read_json(&root.join(RUN).join("run.json"))
}

fn stage_list(root: &Path, stage: &str, key: &str) -> Vec<String> {
    run_json(root)["stages"][stage][key].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// 補上 recipe 與 trace（紀錄檔一併寫好）。`entries`：（label, agent_type）。
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

fn plan_result() -> Value {
    let docs = json!([{"path": "specs/refund/intent.md", "kind": "intent", "audience": "owner"}]);
    json!({"stage": "plan", "epic_dir": "repo/.worktrees/refund", "recipe": {"name": "feature", "modifiers": [], "size": ""}, "intent": envelope(json!({"documents": docs}))})
}

/// init、plan 存檔並由人核准。
fn planned(root: &Path) {
    must(sr(root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    must(sr(root, &["stage", RUN, "plan"], Some(&plan_result().to_string())), "stage plan");
    must(approve(root, "plan", "human"), "approve plan");
}

fn task(acc: &[&str]) -> Value {
    json!({"id": "t1", "title": "t1", "kind": "code", "owns": ["src/export.py"], "depends_on": [], "acceptance": acc, "spec_refs": ["spec.md#匯出"]})
}

fn design_of(subs: Value) -> Value {
    json!({"design": envelope(json!({"epic": "refund", "subtasks": subs})), "challenge": envelope(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "查過"}]}))})
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|d| [d.join(name), d.join(format!("{name}.exe"))]).find(|p| p.is_file())
}

// ------------------------------------------------------------------ 限制對照的用語：派工單寫的詞，閘門認得

/// 用 node 載入 challenge 片段，回這份 profile 在 design 段的限制規則（共同前綴裡的那幾行）。
fn constraint_rule(node: &Path, profile: &Value) -> String {
    let snippet = repo_root().join("plugin/workflow-templates/snippets/challenge.js");
    let code = "const fs = require('fs'); const src = fs.readFileSync(process.argv[1], 'utf8'); const p = JSON.parse(process.argv[2]);\n\
        const k = new Function(src + '\\nreturn { awfConstraints, awfConstraintRule, awfTune }')();\n\
        process.stdout.write(k.awfConstraintRule(k.awfConstraints(p, 'design'), k.awfTune(p).constraints_format))";
    let o = Command::new(node).arg("-e").arg(code).arg(&snippet).arg(profile.to_string()).output().expect("執行 node");
    assert!(o.status.success(), "node 失敗：{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// 從派工單的規則取出節名與三個判定詞（「在執行紀錄檔加〈節名〉一節…格式固定：| <id> | 甲／乙／丙 | …」）。
fn words_in_rule(rule: &str) -> (String, [String; 3]) {
    let heading = rule.split('〈').nth(1).and_then(|t| t.split('〉').next()).unwrap_or_else(|| panic!("規則裡找不到節名：{rule}")).to_string();
    let cell = rule.split("| <id> | ").nth(1).and_then(|t| t.split(" | ").next()).unwrap_or_else(|| panic!("規則裡找不到判定詞：{rule}"));
    let w: Vec<String> = cell.split('／').map(str::to_string).collect();
    assert_eq!(w.len(), 3, "判定詞要有三個：{cell}");
    (heading, [w[0].clone(), w[1].clone(), w[2].clone()])
}

fn constraint_profile(extra: Value) -> Value {
    let mut p = builtin_profile(json!({"constraints": [{"id": "c1", "file": "docs/c.md", "stages": ["design"], "trace": "table"}]}));
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    p
}

/// design 存檔；`record` 是那一段執行紀錄的內容。
fn save_design_with_record(root: &Path, record: &str) -> Out {
    let r = traced(root, "design", design_of(json!([task(&["AC-1 下載"])])), &[]);
    write(&root.join(RUN).join("design/walk.md"), record);
    sr(root, &["stage", RUN, "design"], Some(&r.to_string()))
}

/// agent 照派工單寫的對照表：節名、每個限制一列。
fn record_following(heading: &str, verdict: Option<&str>) -> String {
    let row = verdict.map(|v| format!("| c1 | {v} | docs/c.md:1 |\n")).unwrap_or_default();
    format!("# record\n\n## {heading}\n\n| id | verdict | basis |\n|---|---|---|\n{row}")
}

/// 端到端：profile 設了英文的節名與判定詞 → 派工單照設定寫 → agent 照派工單寫紀錄 → 存檔工具判對
/// （符合、不適用沒問題；偏離有問題；缺列有問題）。沒設定時同一套流程用內建的字，也對得上。
#[test]
fn constraint_words_in_the_dispatch_are_the_ones_the_gate_reads() {
    let Some(node) = which("node") else {
        eprintln!("[略過] 沒有 node，略過限制對照用語的端到端測試");
        return;
    };
    let en = json!({"constraints_format": {"heading": "Constraint trace", "verdicts": {"ok": "compliant", "deviation": "deviates", "na": "n/a"}}});
    for (tag, extra, want) in [("en", en, ("Constraint trace", ["compliant", "deviates", "n/a"])), ("zh", json!({}), ("限制對照", ["符合", "偏離", "不適用"]))] {
        let profile = constraint_profile(extra);
        let rule = constraint_rule(&node, &profile);
        let (heading, [ok, dev, na]) = words_in_rule(&rule);
        assert_eq!((heading.as_str(), [ok.as_str(), dev.as_str(), na.as_str()]), want, "派工單的用語要照設定：{rule}");

        let t = Tmp::new(&format!("cf-{tag}"));
        let root = t.path().join("proj");
        mk(&root, &profile);
        write(&root.join("docs/c.md"), "# 限制\n");
        planned(&root);
        must(save_design_with_record(&root, &record_following(&heading, Some(&ok))), "stage design");
        assert!(stage_list(&root, "design", "constraint_issues").is_empty(), "{tag}：寫「{ok}」不該有問題：{:?}", stage_list(&root, "design", "constraint_issues"));
        must(approve(&root, "design", "policy"), "符合時可以政策核准");
        must(save_design_with_record(&root, &record_following(&heading, Some(&na))), "stage design");
        assert!(stage_list(&root, "design", "constraint_issues").is_empty(), "{tag}：寫「{na}」不該有問題");
        must(save_design_with_record(&root, &record_following(&heading, Some(&dev))), "stage design");
        assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 有偏離（walk.md），要由人決定"], "{tag}：寫「{dev}」要算偏離");
        let o = approve(&root, "design", "policy");
        assert!(o.code == 1 && o.all().contains("限制 [c1] 有偏離"), "{tag}：偏離不能政策核准：{}", o.all());
        must(save_design_with_record(&root, &record_following(&heading, None)), "stage design");
        assert_eq!(stage_list(&root, "design", "constraint_issues"), [format!("限制 [c1] 要逐條對照，這一段的執行紀錄卻沒有它的〈{heading}〉列")], "{tag}：缺列要有問題");
    }
}

/// 設定寫壞時兩邊一起退回內建的字：派工單不會要 agent 寫工具不認得的詞。
#[test]
fn bad_constraint_format_falls_back_on_both_sides() {
    let Some(node) = which("node") else {
        eprintln!("[略過] 沒有 node，略過限制對照用語的端到端測試");
        return;
    };
    for bad in [json!({"heading": "Trace", "verdicts": {"ok": "偏離"}}), json!({"verdicts": {"ok": "x", "deviation": "x"}}), json!({"verdicts": {"ok": "a|b"}}), json!({"heading": " pad"}), json!({"extra": 1})] {
        let rule = constraint_rule(&node, &constraint_profile(json!({"constraints_format": bad.clone()})));
        let (heading, words) = words_in_rule(&rule);
        assert_eq!((heading.as_str(), words.each_ref().map(String::as_str)), ("限制對照", ["符合", "偏離", "不適用"]), "{bad}：派工單要退回內建的字");
    }
}

// ------------------------------------------------------------------ 驗收條件編號的樣式：連結檢查和拆工檢查同一個來源

fn links_profile(pattern: Option<&str>) -> Value {
    let d = read_json(&scaffold_teams().join("methodologies/default.json"));
    let mut design = d["stages"]["design"].clone();
    if let Some(p) = pattern {
        design["criteria_id_pattern"] = json!(p);
    }
    builtin_profile(json!({"links": {"enforce": true}, "stages": {"design": design}}))
}

fn test_of(rows: &[(&str, &str)]) -> Value {
    let crit: Vec<Value> = rows.iter().map(|(id, cmd)| json!({"id": id, "criterion": "c", "command": cmd, "result": "pass", "evidence": "e"})).collect();
    json!({"verifier": envelope(json!({"commit": SHA, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": crit}))})
}

/// 走到 test 存檔（links.enforce 開著）。子任務的驗收條件是 `acc`，test 段逐條結果的 id 是 `row`。
fn links_run(tag: &str, pattern: Option<&str>, acc: &str, row: &str) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, &links_profile(pattern));
    planned(&root);
    must(save(&root, "design", design_of(json!([task(&[acc])])), &[]), "stage design");
    must(approve(&root, "design", "human"), "approve design");
    let built = json!({"x": envelope(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/export.py"], "tests": []}))});
    must(save(&root, "build", built.clone(), &[]), "stage build");
    must(save(&root, "sync", built, &[]), "stage sync");
    let _ = save(&root, "test", test_of(&[(row, "pytest tests/test_export.py::test_csv")]), &[("verify", "agent-workflow:dev-verifier")]);
    (t, root)
}

/// 有設定：照樣式認編號，連結接得起來；分片前綴（`api:ISSUE-12`）也認。政策核准重算、照樣通過。
#[test]
fn links_use_the_configured_criteria_id_pattern() {
    for row in ["ISSUE-12", "api:ISSUE-12"] {
        let (_t, root) = links_run("links-set", Some(r"\bISSUE-\d+\b"), "ISSUE-12 匯出 CSV", row);
        assert!(stage_list(&root, "design", "link_gaps").is_empty(), "design：{:?}", stage_list(&root, "design", "link_gaps"));
        assert!(stage_list(&root, "test", "link_gaps").is_empty(), "test（{row}）：{:?}", stage_list(&root, "test", "link_gaps"));
        must(approve(&root, "test", "policy"), "連結齊全可以政策核准");
        let ledger = std::fs::read_to_string(root.join("_runs/links.jsonl")).unwrap_or_default();
        assert!(ledger.contains("ISSUE-12"), "帳本要記自訂編號的連結：{ledger}");
    }
    // 測試結果裡沒有這條編號：照樣列缺口、擋政策核准
    let (_t, root) = links_run("links-miss", Some(r"\bISSUE-\d+\b"), "ISSUE-12 匯出 CSV", "ISSUE-99");
    assert_eq!(stage_list(&root, "test", "link_gaps"), ["ISSUE-12：缺 測試、commit"]);
    let o = approve(&root, "test", "policy");
    assert!(o.code == 1 && o.all().contains("連結檢查：ISSUE-12：缺 測試、commit"), "{}", o.all());
}

/// 沒設定：只認內建的「AC-…」，自訂寫法的編號算沒有可辨識的 id（和原本一樣）。
/// 有設定時內建寫法不再算數，訊息說明現在認的是哪一種編號。
#[test]
fn links_without_the_pattern_only_know_builtin_ids() {
    let (_t, root) = links_run("links-default", None, "ISSUE-12 匯出 CSV", "ISSUE-12");
    assert_eq!(stage_list(&root, "design", "link_gaps"), ["子任務 t1 的驗收條件沒有可辨識的 id（「AC-…」編號或規格 id），連不到 spec、測試與 commit"]);
    let (_t, root) = links_run("links-default-ac", None, "AC-1 匯出 CSV", "AC-1");
    assert!(stage_list(&root, "test", "link_gaps").is_empty(), "{:?}", stage_list(&root, "test", "link_gaps"));
    let (_t, root) = links_run("links-set-ac", Some(r"\bISSUE-\d+\b"), "AC-1 匯出 CSV", "AC-1");
    assert_eq!(stage_list(&root, "design", "link_gaps"), ["子任務 t1 的驗收條件沒有可辨識的 id（符合 stages.design.criteria_id_pattern 的編號或規格 id），連不到 spec、測試與 commit"]);
}

// ------------------------------------------------------------------ 換角色與合併角色：模型下限照被頂替的角色算

fn models() -> Value {
    json!({"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"}, "defaults": {"*": "light"}, "floors": {}})
}

/// plan 存檔，trace 只有反駁 intent 那一步（紀錄檔寫好）。
fn save_plan_with_challenge(root: &Path, agent: &str, tier: &str, model: &str, role_as: Option<&str>) -> Out {
    let mut r = plan_result();
    write(&root.join(RUN).join("plan/challenge-intent.md"), "# 紀錄\n");
    let mut t = json!({"label": "challenge:intent", "agent_type": agent, "phase": "Challenge", "record": format!("{RUN}/plan/challenge-intent.md"), "ok": true, "tier": tier, "model": model});
    if let Some(x) = role_as {
        t["role_as"] = json!(x);
    }
    r["trace"] = json!([t]);
    sr(root, &["stage", RUN, "plan"], Some(&r.to_string()))
}

/// `roles` 把 plan 的反駁換成自己的 agent：存檔時照 tpm-skeptic 在 plan 段的下限（standard）重算。
/// 低於下限判 invalid、剛好通過；trace 不帶 `role_as` 或謊報成別的角色一樣被擋。
#[test]
fn swapped_agent_below_the_floor_is_invalid_at_save() {
    let t = Tmp::new("roles-floor");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({"models": models(), "roles": {"plan.challenge": "acme-skeptic"}})));
    must(sr(&root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    let low = save_plan_with_challenge(&root, "acme-skeptic", "light", "haiku", Some("tpm-skeptic"));
    assert!(low.code == 1 && low.all().contains("模型分級：challenge:intent（tpm-skeptic）用了 light，低於下限 standard"), "低於下限要判錯：{}", low.all());
    assert_eq!(run_json(&root)["stages"]["plan"]["status"], "invalid");
    for (role_as, what) in [(None, "不帶 role_as"), (Some("tpm-researcher"), "謊報 role_as")] {
        let o = save_plan_with_challenge(&root, "acme-skeptic", "light", "haiku", role_as);
        assert!(o.code == 1 && o.all().contains("（tpm-skeptic）用了 light，低於下限 standard"), "{what} 也要照 tpm-skeptic 的下限：{}", o.all());
    }
    let lied = save_plan_with_challenge(&root, "acme-skeptic", "standard", "sonnet", Some("tpm-researcher"));
    assert!(lied.code == 1 && lied.all().contains("role_as=tpm-researcher"), "謊報的 role_as 本身也要列出來：{}", lied.all());
    let ok = save_plan_with_challenge(&root, "acme-skeptic", "standard", "sonnet", Some("tpm-skeptic"));
    assert!(ok.code == 0 && !ok.all().contains("模型分級"), "剛好在下限要通過：{}", ok.all());
    assert_ne!(run_json(&root)["stages"]["plan"]["status"], "invalid");
}

/// profile 沒有 `roles`：同一筆 trace（不是模版角色、沒有 role_as）照舊當擴充 agent，不檢查。
#[test]
fn extension_agents_are_still_not_checked() {
    let t = Tmp::new("roles-none");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({"models": models()})));
    must(sr(&root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    let o = save_plan_with_challenge(&root, "acme-skeptic", "light", "haiku", None);
    assert!(o.code == 0 && !o.all().contains("模型分級"), "沒有換角色時不該多擋：{}", o.all());
}

fn merge_decision() -> Value {
    let mut e = envelope(json!({"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": SHA, "traceability": [], "out_of_scope": [], "followups": [], "risk_tier": "low", "gate_reasons": [], "merge_state": "merged"}));
    e["evidence"] = json!([{"claim": "gate 判 merge", "source": "awf merge-gate", "result": "exit 0"}]);
    e
}

/// merge 存檔：兩筆派工（analyze、execute:merge）都是 `agent`，等級 `tier`；不寫紀錄檔。
fn save_merge(root: &Path, agent: &str, tier: &str, model: &str) -> Out {
    let entry = |label: &str, file: &str| json!({"label": label, "agent_type": agent, "phase": "Execute", "record": format!("{RUN}/merge/{file}"), "ok": true, "tier": tier, "model": model});
    let r = json!({"stage": "merge", "analysis": merge_decision(), "execute": merge_decision(), "trace": [entry("analyze", "analyze.md"), entry("execute:merge", "execute-merge.md")]});
    sr(root, &["stage", RUN, "merge"], Some(&r.to_string()))
}

fn merger_project(tag: &str, guard: Option<Value>) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({"models": models()})));
    if let Some(g) = guard {
        write_json(&root.join(".claude/teams/guard.config.json"), &g);
    }
    must(sr(&root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    (t, root)
}

const MERGER: &str = "acme:release-merger";

fn merger_guard() -> Value {
    json!({"auto_merge": {"enabled": true, "merger_role": MERGER}, "extension_agents": {MERGER: "tpm-reviewer"}})
}

/// `guard.config.json` 指定了別的合併角色：存檔時照 dev-merge-steward 的下限（strong）算，trace 有沒有記 `role_as` 都一樣；
/// 它沒有寫紀錄檔時和 dev-merge-steward 一樣依信封代寫，不算缺件。
#[test]
fn configured_merger_is_held_to_the_steward_floor_and_gets_its_record_written() {
    let (_t, root) = merger_project("merger-set", Some(merger_guard()));
    let low = save_merge(&root, MERGER, "standard", "sonnet");
    assert!(low.code == 1 && low.all().contains("（dev-merge-steward）用了 standard，低於下限 strong"), "低於下限要判錯：{}", low.all());
    let ok = save_merge(&root, MERGER, "strong", "opus");
    assert!(ok.code == 0 && !ok.all().contains("模型分級"), "strong 要通過：{}", ok.all());
    for f in ["analyze.md", "execute-merge.md"] {
        let body = std::fs::read_to_string(root.join(RUN).join("merge").join(f)).unwrap_or_default();
        assert!(body.contains(GHOST) && body.contains("gate 判 merge"), "{f} 要依信封代寫：{body}");
    }
    assert!(run_json(&root)["stages"]["merge"].get("missing_records").is_none(), "代寫的紀錄不算缺件");
}

/// 合併角色自己寫了紀錄檔（有 Write 工具的 agent）：照一般角色核對，不拿代寫的蓋掉。
#[test]
fn configured_merger_keeps_its_own_record() {
    let (_t, root) = merger_project("merger-own", Some(merger_guard()));
    let own = root.join(RUN).join("merge/analyze.md");
    write(&own, "# 我自己寫的紀錄\n\n逐條對照見下。\n");
    must(save_merge(&root, MERGER, "strong", "opus"), "stage merge");
    assert_eq!(std::fs::read_to_string(&own).unwrap_or_default(), "# 我自己寫的紀錄\n\n逐條對照見下。\n", "自己寫的紀錄不能被蓋掉");
    let other = std::fs::read_to_string(root.join(RUN).join("merge/execute-merge.md")).unwrap_or_default();
    assert!(other.contains(GHOST), "沒寫的那一筆照樣代寫：{other}");
    assert!(run_json(&root)["stages"]["merge"].get("missing_records").is_none());
}

/// 設定檔沒指定（或指定的就是預設值）：這個 agent 只是擴充 agent，和原本一樣不查下限；
/// 預設的 dev-merge-steward 照舊查、照舊代寫。
#[test]
fn without_the_setting_nothing_changes_for_the_merge_stage() {
    for (tag, guard) in [("merger-none", None), ("merger-default", Some(json!({"auto_merge": {"enabled": true, "merger_role": "agent-workflow:dev-merge-steward"}})))] {
        let (_t, root) = merger_project(tag, guard);
        let o = save_merge(&root, MERGER, "standard", "sonnet");
        assert!(!o.all().contains("模型分級"), "{tag}：沒指定時不該照 steward 的下限查別的 agent：{}", o.all());
        let low = save_merge(&root, "agent-workflow:dev-merge-steward", "standard", "sonnet");
        assert!(low.code == 1 && low.all().contains("（dev-merge-steward）用了 standard，低於下限 strong"), "{tag}：{}", low.all());
        let ok = save_merge(&root, "agent-workflow:dev-merge-steward", "strong", "opus");
        assert!(ok.code == 0, "{tag}：{}", ok.all());
        let body = std::fs::read_to_string(root.join(RUN).join("merge/analyze.md")).unwrap_or_default();
        assert!(body.contains(GHOST), "{tag}：steward 的紀錄照舊代寫");
    }
}
