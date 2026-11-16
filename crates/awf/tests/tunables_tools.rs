//! 工具端「寫死的值改成可設定」的測試（第 12 批）：每一項都驗三件事——有設定時生效、沒設定時和原本一樣、
//! 寫壞的設定退回預設或回錯誤（只會往嚴的方向）。
//!
//! 一律用真的執行檔、乾淨的環境（拿掉 `CLAUDE_*`、`AWF_*`）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const RUN: &str = "_runs/r1";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("awf-tunables-{}-{}-{}", std::process::id(), n, tag));
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
    awf_env(args, cwd, stdin, &[])
}

fn awf_env(args: &[&str], cwd: &Path, stdin: Option<&str>, env: &[(&str, &str)]) -> Out {
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

fn sr(root: &Path, args: &[&str], stdin: Option<&str>) -> Out {
    let mut a = vec!["save-run"];
    a.extend(args);
    awf(&a, root, stdin)
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

fn set_profile(root: &Path, profile: &Value) {
    write_json(&root.join(".claude/teams/methodology.json"), profile);
}

fn run_json(root: &Path) -> Value {
    read_json(&root.join(RUN).join("run.json"))
}

/// 補上 recipe 與 trace（紀錄檔一併寫好）。
fn traced(root: &Path, stage: &str, result: Value) -> Value {
    let data = run_json(root);
    let mut out = result;
    if let Some(rec) = data.get("recipe").filter(|r| r.is_object()) {
        out["recipe"] = json!({"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]});
    }
    let name = "walk.md";
    write(&root.join(RUN).join(stage).join(name), "# 紀錄\n");
    out["trace"] = json!([{"label": "walk", "agent_type": "x", "phase": "p", "record": format!("{RUN}/{stage}/{name}"), "ok": true}]);
    out
}

fn save(root: &Path, stage: &str, result: Value) -> Out {
    let r = traced(root, stage, result);
    sr(root, &["stage", RUN, stage], Some(&r.to_string()))
}

fn approve(root: &Path, stage: &str, by: &str) -> Out {
    sr(root, &["approve", RUN, stage, "--by", by, "--note", "p", "--at", "t"], None)
}

fn design_of(subs: Value) -> Value {
    json!({"design": envelope(json!({"epic": "refund", "subtasks": subs})), "challenge": envelope(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "查過"}]}))})
}

fn task(acc: &[&str]) -> Value {
    json!({"id": "t1", "title": "t1", "kind": "code", "owns": ["src/refund.py"], "depends_on": [], "acceptance": acc})
}

fn build_of() -> Value {
    json!({"x": envelope(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/refund.py"], "tests": []}))})
}

/// init 與 plan 存檔（recipe feature）。
fn plan(root: &Path) {
    must(sr(root, &["init", RUN, "--event", "sdlc", "--target", "repo", "--requirement", "r"], None), "init");
    let docs = json!([{"path": "specs/refund/intent.md", "kind": "intent", "audience": "owner"}]);
    let plan = json!({"stage": "plan", "epic_dir": "repo/.worktrees/refund", "recipe": {"name": "feature", "modifiers": [], "size": ""}, "intent": envelope(json!({"documents": docs}))});
    must(sr(root, &["stage", RUN, "plan"], Some(&plan.to_string())), "stage plan");
}

/// plan 核准 → design 存檔並由人核准。
fn upto_design(root: &Path, design: Value) {
    plan(root);
    must(approve(root, "plan", "human"), "approve plan");
    must(save(root, "design", design), "stage design");
    must(approve(root, "design", "human"), "approve design");
}

// ------------------------------------------------------------------ T2：gates.extra（只能加閘門）

fn gates(root: &Path) -> Vec<bool> {
    let d = run_json(root);
    ["plan", "design", "build", "sync", "test", "deliver", "merge"].iter().map(|st| d["stages"][st]["gate"].as_bool().unwrap_or(false)).collect()
}

/// 沒設定：閘門就是原本的四段。
#[test]
fn t2_default_gates_unchanged() {
    let t = Tmp::new("t2-default");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({})));
    upto_design(&root, design_of(json!([task(&["AC-1 下載"])])));
    must(save(&root, "build", build_of()), "stage build");
    assert_eq!(gates(&root), [true, true, false, false, true, true, false]);
    let o = must(save(&root, "sync", build_of()), "stage sync");
    assert!(!o.out.contains("需要使用者核准"), "sync 沒有閘門：{}", o.all());
}

/// 有設定：build 做完要等核准；沒核准時後面的段不能核准，狀態機回 gate。拿掉既有閘門的寫法被拒絕。
#[test]
fn t2_extra_gate_blocks_the_chain_until_approved() {
    let t = Tmp::new("t2-extra");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({"gates": {"extra": ["build"]}})));
    upto_design(&root, design_of(json!([task(&["AC-1 下載"])])));
    assert_eq!(gates(&root), [true, true, true, false, true, true, false], "init 當下就標上 build 的閘門");
    let o = must(save(&root, "build", build_of()), "stage build");
    assert!(o.out.contains("需要使用者核准"), "build 存檔後要說明等核准：{}", o.all());
    must(save(&root, "sync", build_of()), "stage sync");
    let o = approve(&root, "sync", "human");
    assert!(o.code == 1 && o.all().contains("前面的 build 段還沒核准"), "build 沒核准時後面的段不能核准：{}", o.all());
    // 狀態機：下一步是 build 的閘門（不是往下跑 test）
    let n = awf(&["next", "--run", RUN], &root, None);
    let act: Value = serde_json::from_str(&n.out).unwrap_or_else(|e| panic!("next 的輸出不是 JSON（{e}）：{}", n.all()));
    assert!(act["action"] == "gate" && act["stage"] == "build", "狀態機要停在 build 的閘門：{act}");
    must(approve(&root, "build", "human"), "approve build");
    must(approve(&root, "sync", "human"), "build 核准後 sync 可以核准");
}

/// 設定是後來才加的：存檔當下有列的段就標上閘門；之後把設定拿掉，已經標上的不會消失。
#[test]
fn t2_gate_added_later_applies_on_save_and_never_drops() {
    let t = Tmp::new("t2-later");
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(json!({})));
    upto_design(&root, design_of(json!([task(&["AC-1 下載"])])));
    assert_eq!(gates(&root)[2..4], [false, false]);
    set_profile(&root, &builtin_profile(json!({"gates": {"extra": ["build", "sync"]}})));
    must(save(&root, "build", build_of()), "stage build");
    assert!(gates(&root)[2], "存檔時 profile 有列 build：標上閘門");
    set_profile(&root, &builtin_profile(json!({})));
    must(save(&root, "build", build_of()), "重存 build");
    assert!(gates(&root)[2], "拿掉設定不會把已經標上的閘門拿掉");
    assert_eq!(gates(&root)[..2], [true, true]);
}

/// 寫壞的設定：不認得的段、想拿掉既有閘門、型別不對，profile 一律判不合規則（不會默默當成沒加）。
#[test]
fn t2_bad_gates_setting_is_an_error() {
    for (bad, want) in [(json!({"extra": ["plan"]}), "gates.extra 只能列"), (json!({"extra": ["deploy"]}), "gates.extra 只能列"), (json!({"remove": ["design"]}), "gates 不認得的鍵"), (json!({"extra": "build"}), "gates.extra 要是陣列")] {
        let t = Tmp::new("t2-bad");
        let root = t.path().join("proj");
        mk(&root, &builtin_profile(json!({})));
        upto_design(&root, design_of(json!([task(&["AC-1 下載"])])));
        set_profile(&root, &builtin_profile(json!({"gates": bad})));
        let o = save(&root, "build", build_of());
        assert!(o.code == 1 && o.all().contains(want), "{bad}：存檔要報 profile 不合規則（{want}）：{}", o.all());
        assert_eq!(gates(&root)[..2], [true, true], "既有閘門不受影響");
        assert_eq!(run_json(&root)["stages"]["build"]["status"], "invalid");
    }
}

// ------------------------------------------------------------------ T5：限制對照的判定詞

fn stage_list(root: &Path, stage: &str, key: &str) -> Vec<String> {
    run_json(root)["stages"][stage][key].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// design 存檔；`record` 是那一段執行紀錄的內容。
fn save_design_with_record(root: &Path, record: &str) -> Out {
    let r = traced(root, "design", design_of(json!([task(&["AC-1 下載"])])));
    write(&root.join(RUN).join("design/walk.md"), record);
    sr(root, &["stage", RUN, "design"], Some(&r.to_string()))
}

fn constraint_profile(extra: Value) -> Value {
    let mut p = builtin_profile(json!({"constraints": [{"id": "c1", "file": "docs/c.md", "stages": ["design"], "trace": "table"}]}));
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        p[k] = v;
    }
    p
}

fn t5_project(tag: &str, profile: &Value) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, profile);
    write(&root.join("docs/c.md"), "# 限制\n");
    plan(&root);
    must(approve(&root, "plan", "human"), "approve plan");
    (t, root)
}

const EN_OK: &str = "# record\n\n## Constraint check\n\n| id | verdict | note |\n|---|---|---|\n| c1 | compliant | fine |\n";
const EN_DEV: &str = "# record\n\n| id | verdict | note |\n|---|---|---|\n| c1 | deviation | differs |\n";
const ZH_OK: &str = "# 紀錄\n\n## 限制對照\n\n| 限制 | 判定 | 說明 |\n|---|---|---|\n| c1 | 符合 | 沒問題 |\n";
const ZH_DEV: &str = "# 紀錄\n\n| 限制 | 判定 | 說明 |\n|---|---|---|\n| c1 | 偏離 | 不一樣 |\n";

fn en_format() -> Value {
    json!({"constraints_format": {"heading": "Constraint check", "verdicts": {"ok": "compliant", "deviation": "deviation", "na": "n/a"}}})
}

/// 沒設定：只認中文的判定詞，英文寫的對照列算沒有對照（和原本一樣）。
#[test]
fn t5_default_only_knows_builtin_verdicts() {
    let (_t, root) = t5_project("t5-default", &constraint_profile(json!({})));
    must(save_design_with_record(&root, ZH_OK), "stage design");
    assert!(stage_list(&root, "design", "constraint_issues").is_empty());
    must(save_design_with_record(&root, EN_OK), "stage design");
    assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 要逐條對照，這一段的執行紀錄卻沒有它的〈限制對照〉列"]);
    must(save_design_with_record(&root, ZH_DEV), "stage design");
    assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 有偏離（walk.md），要由人決定"]);
}

/// 有設定：設定的詞認得，設定的「偏離」一樣擋；內建的中文詞照舊認得、照舊擋。
#[test]
fn t5_configured_verdicts_are_recognized_and_deviation_still_blocks() {
    let (_t, root) = t5_project("t5-set", &constraint_profile(en_format()));
    must(save_design_with_record(&root, EN_OK), "stage design");
    assert!(stage_list(&root, "design", "constraint_issues").is_empty(), "設定的「符合」要認得");
    must(save_design_with_record(&root, EN_DEV), "stage design");
    assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 有偏離（walk.md），要由人決定"]);
    let o = approve(&root, "design", "policy");
    assert!(o.code == 1 && o.all().contains("限制文件：限制 [c1] 有偏離"), "有偏離不能政策核准：{}", o.all());
    must(save_design_with_record(&root, ZH_DEV), "stage design");
    assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 有偏離（walk.md），要由人決定"], "內建的「偏離」照舊擋");
    must(save_design_with_record(&root, ZH_OK), "stage design");
    assert!(stage_list(&root, "design", "constraint_issues").is_empty(), "內建的「符合」照舊認得");
    must(save_design_with_record(&root, "# record\n"), "stage design");
    assert_eq!(stage_list(&root, "design", "constraint_issues"), ["限制 [c1] 要逐條對照，這一段的執行紀錄卻沒有它的〈Constraint check〉列"], "訊息用設定的節名");
}

/// 寫壞的設定（把內建的詞改成別的意思、三個詞相同、含表格線）：profile 判不合規則，不會因此少擋。
#[test]
fn t5_bad_format_is_an_error_and_never_loosens() {
    for bad in [json!({"verdicts": {"ok": "偏離"}}), json!({"verdicts": {"deviation": "符合"}}), json!({"verdicts": {"ok": "x", "deviation": "x"}}), json!({"verdicts": {"ok": "a|b"}}), json!({"verdicts": {"fine": "ok"}})] {
        let (_t, root) = t5_project("t5-bad", &constraint_profile(json!({})));
        set_profile(&root, &constraint_profile(json!({"constraints_format": bad})));
        let o = save_design_with_record(&root, ZH_DEV);
        assert!(o.code == 1 && o.all().contains("constraints_format"), "{bad}：要報 profile 不合規則：{}", o.all());
        assert_eq!(run_json(&root)["stages"]["design"]["status"], "invalid");
    }
}

// ------------------------------------------------------------------ T7：驗收條件編號的樣式

fn t7_project(tag: &str, profile: &Value, spec: &str) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, profile);
    write(&root.join("repo/.worktrees/refund/specs/refund/spec.md"), spec);
    plan(&root);
    must(approve(&root, "plan", "human"), "approve plan");
    (t, root)
}

fn pattern_profile(pat: &str) -> Value {
    let d = read_json(&scaffold_teams().join("methodologies/default.json"));
    let mut design = d["stages"]["design"].clone();
    design["criteria_id_pattern"] = json!(pat);
    builtin_profile(json!({"stages": {"design": design}}))
}

const ISSUE_SPEC: &str = "# spec\n- ISSUE-12：匯出按鈕下載 CSV\n- ISSUE-13：檔名帶日期\n- AC-9 這一條用內建樣式才看得到\n";

/// 沒設定：只認「AC-…」。spec 用別的編號時兩邊對不起來，工具不比（和原本一樣）。
#[test]
fn t7_default_pattern_is_ac() {
    let (_t, root) = t7_project("t7-default", &builtin_profile(json!({})), ISSUE_SPEC);
    must(save(&root, "design", design_of(json!([task(&["ISSUE-12 下載"])]))), "stage design");
    assert!(stage_list(&root, "design", "plan_check").is_empty(), "子任務沒用 AC- 編號：不比");
    must(save(&root, "design", design_of(json!([task(&["AC-1 下載"])]))), "stage design");
    assert_eq!(stage_list(&root, "design", "plan_check"), ["spec 有 1 條驗收條件沒有任何子任務對應：AC-9", "子任務引用了 spec 沒有的驗收條件：AC-1"]);
}

/// 有設定：照樣式認編號，spec 有而沒有子任務對應的、子任務引用而 spec 沒有的都抓得到。
#[test]
fn t7_custom_pattern_finds_orphans_and_ghosts() {
    let (_t, root) = t7_project("t7-set", &pattern_profile(r"\bISSUE-\d+\b"), ISSUE_SPEC);
    must(save(&root, "design", design_of(json!([task(&["ISSUE-12 下載", "ISSUE-99 不存在"])]))), "stage design");
    assert_eq!(stage_list(&root, "design", "plan_check"), ["spec 有 1 條驗收條件沒有任何子任務對應：ISSUE-13", "子任務引用了 spec 沒有的驗收條件：ISSUE-99"]);
    let o = approve(&root, "design", "policy");
    assert!(o.code == 1 && o.all().contains("拆工檢查：spec 有 1 條"), "政策核准重算、照樣擋：{}", o.all());
    must(save(&root, "design", design_of(json!([task(&["ISSUE-12 下載", "ISSUE-13 檔名"])]))), "stage design");
    assert!(stage_list(&root, "design", "plan_check").is_empty());
}

/// 寫壞的樣式（編不過、會符合空字串、不是字串）：回錯誤，不是當成沒設定。
#[test]
fn t7_bad_pattern_is_an_error() {
    for (bad, want) in [(json!("ISSUE-(\\d+"), "樣式編不過"), (json!("\\d*"), "不能符合空字串"), (json!(12), "要是字串"), (json!("[\\S]"), "不支援的寫法")] {
        let (_t, root) = t7_project("t7-bad", &builtin_profile(json!({})), ISSUE_SPEC);
        let d = read_json(&scaffold_teams().join("methodologies/default.json"));
        let mut design = d["stages"]["design"].clone();
        design["criteria_id_pattern"] = bad.clone();
        set_profile(&root, &builtin_profile(json!({"stages": {"design": design}})));
        let o = save(&root, "design", design_of(json!([task(&["ISSUE-12 下載"])])));
        assert!(o.code == 1 && o.all().contains("stages.design.criteria_id_pattern") && o.all().contains(want), "{bad}：要報錯（{want}）：{}", o.all());
        assert_eq!(run_json(&root)["stages"]["design"]["status"], "invalid");
    }
}

// ------------------------------------------------------------------ T10、T11：merge-gate 的高風險路徑與路徑分類

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn one_file_diff(path: &str) -> String {
    format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n")
}

/// 跑一次 merge-gate（其他輸入都沒給，所以一定是 hold 或 escalate；這裡只看風險等級與交給人的理由）。
fn gate(tag: &str, auto_merge: Value, profile: Option<Value>, path: &str) -> Value {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, &profile.unwrap_or_else(|| builtin_profile(json!({}))));
    let mut am = json!({"enabled": true, "repos": {"acme-fork/app": "medium"}});
    for (k, v) in auto_merge.as_object().cloned().unwrap_or_default() {
        am[k] = v;
    }
    write_json(&root.join(".claude/teams/guard.config.json"), &json!({"auto_merge": am}));
    write(&root.join("d.diff"), &one_file_diff(path));
    let o = awf(&["merge-gate", "check", "--repo", "acme-fork/app", "--pr", "3", "--head-sha", SHA, "--diff", "d.diff", "--project-dir", &s(&root), "--no-audit"], &root, None);
    serde_json::from_str(&o.out).unwrap_or_else(|e| panic!("merge-gate 的輸出不是 JSON（{e}）：{}", o.all()))
}

fn escalations(v: &Value) -> String {
    v["escalations"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// 沒設定：一般的程式檔是 medium，沒有高風險路徑的理由。
#[test]
fn t10_default_sensitive_paths_unchanged() {
    let v = gate("t10-default", json!({}), None, "src/billing/x.py");
    assert!(v["risk_tier"] == "medium" && !escalations(&v).contains("高風險"), "{v}");
    let v = gate("t10-builtin", json!({}), None, ".github/workflows/ci.yml");
    assert!(v["risk_tier"] == "high" && escalations(&v).contains("改到高風險路徑：.github/workflows/ci.yml（GitHub 設定或 CI）"), "{v}");
}

/// 有設定：命中的路徑算高風險、交給人；內建清單照舊。
#[test]
fn t10_extra_sensitive_paths_escalate() {
    let am = json!({"sensitive_paths_extra": ["(^|/)billing/"]});
    let v = gate("t10-set", am.clone(), None, "src/billing/x.py");
    assert!(v["risk_tier"] == "high" && escalations(&v).contains("改到高風險路徑：src/billing/x.py（專案設定的高風險路徑）"), "{v}");
    let v = gate("t10-set-other", am.clone(), None, "src/orders/x.py");
    assert!(v["risk_tier"] == "medium", "沒命中的路徑不受影響：{v}");
    let v = gate("t10-set-builtin", am, None, ".github/workflows/ci.yml");
    assert!(escalations(&v).contains("（GitHub 設定或 CI）"), "內建清單拿不掉：{v}");
}

/// 寫壞的設定（樣式編不過、不是陣列、元素不是字串）：這次判定一律 high、交給人。
#[test]
fn t10_bad_setting_escalates() {
    for bad in [json!(["billing/("]), json!("billing/"), json!([12]), json!([""])] {
        let v = gate("t10-bad", json!({"sensitive_paths_extra": bad.clone()}), None, "docs/a.md");
        assert!(v["risk_tier"] == "high" && escalations(&v).contains("高風險路徑的設定讀不懂（auto_merge.sensitive_paths_extra"), "{bad}：{v}");
        assert_ne!(v["decision"], "merge");
    }
}

/// T11 沒設定：只改文件是 low。
#[test]
fn t11_default_docs_only_is_low() {
    let v = gate("t11-default", json!({}), None, "docs/api/a.md");
    assert!(v["risk_tier"] == "low", "{v}");
}

/// T11 有設定：歸成 code 的文件不再是 low；歸成 migration、manifest、agent_config 的路徑交給人。
#[test]
fn t11_extra_classes_only_tighten() {
    let prof = |extra: Value| Some(builtin_profile(json!({"diff_classes_extra": extra})));
    let v = gate("t11-code", json!({}), prof(json!({"code": ["^docs/api/"]})), "docs/api/a.md");
    assert!(v["risk_tier"] == "medium", "歸成 code 的文件不是 low：{v}");
    let v = gate("t11-code-miss", json!({}), prof(json!({"code": ["^docs/api/"]})), "docs/guide/a.md");
    assert!(v["risk_tier"] == "low", "沒命中的文件照舊：{v}");
    for (cls, what) in [("migration", "資料庫 migration"), ("manifest", "依賴或版本宣告"), ("agent_config", "agent 設定、守門或 gate")] {
        let v = gate("t11-cls", json!({}), prof(json!({cls: ["(^|/)schema/.*\\.ya?ml$"]})), "db/schema/Orders.YML");
        assert!(v["risk_tier"] == "high" && escalations(&v).contains(&format!("db/schema/Orders.YML（{what}；專案設定的分類）")), "{cls}：{v}");
    }
}

/// T11 寫壞的設定（樣式編不過、想把路徑歸成 doc）：交給人，不當成沒設定。
#[test]
fn t11_bad_classes_escalate() {
    for bad in [json!({"migration": ["schema/("]}), json!({"doc": ["^src/generated/"]}), json!({"code": "src/"}), json!(["src/"])] {
        let v = gate("t11-bad", json!({}), Some(builtin_profile(json!({"diff_classes_extra": bad.clone()}))), "docs/a.md");
        assert!(v["risk_tier"] == "high" && escalations(&v).contains("profile 的路徑分類設定讀不懂（diff_classes_extra"), "{bad}：{v}");
    }
}

// ------------------------------------------------------------------ T15：專案另外加的機敏樣式（hook、kb、kg）

const DEV: &str = "agent-workflow:dev-implementer";

/// 一個有守門設定的專案；`extra` 併進 guard.config.json。
fn guarded(tag: &str, extra: Value) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("ws");
    mk(&root, &builtin_profile(json!({})));
    let mut cfg = json!({"runs_dir": "_runs", "fork_repo": "acme-fork/app", "writer_dirs": ["_runs", "docs"]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        cfg[k] = v;
    }
    write_json(&root.join(".claude/teams/guard.config.json"), &cfg);
    (t, root)
}

/// 跑一次 hook；回（exit code, stderr）。
fn hook(root: &Path, mode: &str, tool: &str, agent: &str, input: Value) -> (i32, String) {
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": s(root), "session_id": "abcdefgh12", "tool_input": input});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    let o = awf_env(&["hook", mode], root, Some(&d.to_string()), &[("CLAUDE_PROJECT_DIR", &s(root))]);
    assert!(!o.err.contains("panicked at") && !o.err.contains("意外錯誤"), "hook 不能出意外錯誤：{}", o.err);
    (o.code, o.err)
}

fn read_code(root: &Path, agent: &str, file: &str) -> i32 {
    hook(root, "guard-paths", "Read", agent, json!({"file_path": file})).0
}

fn shell_code(root: &Path, tool: &str, agent: &str, cmd: &str) -> i32 {
    hook(root, "guard-shell", tool, agent, json!({"command": cmd})).0
}

fn secret_files() -> Value {
    json!({"secret_files_extra": ["(^|/)vault/.*\\.ya?ml$"]})
}

/// 沒設定：只擋內建的機敏檔。
#[test]
fn t15_default_secret_files_unchanged() {
    let (_t, root) = guarded("t15-default", json!({}));
    assert_eq!(read_code(&root, "", "conf/vault/prod.yaml"), 0);
    assert_eq!(read_code(&root, "", "conf/.env"), 2);
    assert_eq!(read_code(&root, "", "conf/.env.example"), 0);
    for (tool, cmd) in [("Bash", "cat conf/vault/prod.yaml"), ("PowerShell", "Get-Content conf/vault/prod.yaml")] {
        assert_eq!(shell_code(&root, tool, "", cmd), 0, "{tool}：{cmd}");
    }
}

/// 有設定：命中的檔不能讀（Read、Grep 的 glob、Bash、PowerShell），主 session 與 agent 都擋；沒命中的照舊；內建的照舊。
#[test]
fn t15_extra_secret_files_block_reads() {
    let (_t, root) = guarded("t15-set", secret_files());
    for agent in ["", DEV] {
        assert_eq!(read_code(&root, agent, "conf/vault/prod.yaml"), 2, "Read 要擋（{agent}）");
        assert_eq!(read_code(&root, agent, "conf/Vault/PROD.YML"), 2, "不分大小寫（{agent}）");
        assert_eq!(read_code(&root, agent, "conf/app.yaml"), 0, "沒命中的照舊（{agent}）");
        assert_eq!(read_code(&root, agent, "conf/.env"), 2, "內建的照舊擋（{agent}）");
        for (tool, block, allow) in [("Bash", "cat conf/vault/prod.yaml", "cat conf/app.yaml"), ("PowerShell", "Get-Content conf/vault/prod.yaml", "Get-Content conf/app.yaml")] {
            assert_eq!(shell_code(&root, tool, agent, block), 2, "{tool}：{block}（{agent}）");
            assert_eq!(shell_code(&root, tool, agent, allow), 0, "{tool}：{allow}（{agent}）");
        }
    }
    let (code, _) = hook(&root, "guard-paths", "Grep", "", json!({"pattern": "password", "glob": "conf/vault/prod.yaml"}));
    assert_eq!(code, 2, "Grep 的 glob 指到機敏檔也擋");
}

/// 寫壞的設定（樣式編不過、不是陣列、元素不是字串）：照「設定檔讀不懂」處理——模版角色的寫入全擋；
/// 主 session 不被鎖死（要能修設定檔）；內建的機敏檔照舊擋；hook 不 panic。
#[test]
fn t15_bad_secret_setting_goes_strict_without_locking_the_user_out() {
    let write_in = |root: &Path, agent: &str| hook(root, "guard-paths", "Write", agent, json!({"file_path": s(&root.join("docs/a.md")), "content": "x"}));
    let (_t, good) = guarded("t15-good", secret_files());
    assert_eq!(write_in(&good, "agent-workflow:tpm-writer").0, 0, "設定正常時 tpm-writer 可以寫 docs/");
    for bad in [json!({"secret_files_extra": ["vault/("]}), json!({"secret_files_extra": "vault/"}), json!({"secret_files_extra": [12]}), json!({"secret_patterns_extra": ["ACME-(\\d+"]})] {
        let (_t, root) = guarded("t15-bad", bad.clone());
        let (code, err) = write_in(&root, "agent-workflow:tpm-writer");
        assert!(code == 2 && err.contains("guard.config.json 讀不懂") && err.contains("secret_"), "{bad}：模版角色的寫入要擋並說明：{code} {err}");
        assert_eq!(write_in(&root, "").0, 0, "{bad}：主 session 不被鎖死");
        assert_eq!(read_code(&root, "", "conf/.env"), 2, "{bad}：內建的機敏檔照舊擋");
        assert_eq!(read_code(&root, "", "conf/app.yaml"), 0, "{bad}：一般的檔照舊可以讀");
    }
}

fn kb_project(tag: &str, extra: Value) -> (Tmp, PathBuf) {
    let (t, root) = guarded(tag, extra);
    write(&root.join("docs/a.md"), "# 正常\n\n內文 okmarker\n");
    write(&root.join("docs/internal/b.md"), "# 內部\n\n內文 innermarker\n");
    (t, root)
}

/// kb：沒設定時 docs/internal 照收；有設定時不收（列在略過的清單）；寫壞了不建索引。
#[test]
fn t15_kb_skips_extra_secret_files() {
    let (_t, root) = kb_project("t15-kb-default", json!({}));
    must(awf(&["kb", "build"], &root, None), "kb build");
    assert!(awf(&["kb", "query", "innermarker"], &root, None).out.contains("docs/internal/b.md"), "沒設定時照收");

    let (_t, root) = kb_project("t15-kb-set", json!({"secret_files_extra": ["(^|/)internal/"]}));
    let o = must(awf(&["kb", "build"], &root, None), "kb build");
    assert!(o.all().contains("docs/internal/b.md") && o.all().contains("機敏檔"), "要列出略過的機敏檔：{}", o.all());
    assert!(!awf(&["kb", "query", "innermarker"], &root, None).out.contains("b.md"), "命中的檔不進索引");
    assert!(awf(&["kb", "query", "okmarker"], &root, None).out.contains("docs/a.md"), "其他檔照收");

    let (_t, root) = kb_project("t15-kb-bad", json!({"secret_files_extra": ["internal/("]}));
    let o = awf(&["kb", "build"], &root, None);
    // kb 是輔助工具：失敗只警告、不擋流程（exit 0），但索引不能建出來
    assert!(o.all().contains("機敏檔的設定用不了"), "寫壞了不建索引：{}", o.all());
    let q = awf(&["kb", "query", "innermarker"], &root, None);
    assert!(!q.out.contains("b.md") && !q.out.contains("docs/a.md"), "寫壞了不該有索引內容：{}", q.all());
}

/// kg：和 kb 同一份樣式。
#[test]
fn t15_kg_skips_extra_secret_files() {
    let (_t, root) = kb_project("t15-kg-default", json!({}));
    must(awf(&["kg", "build"], &root, None), "kg build");
    assert!(awf(&["kg", "locate", "innermarker"], &root, None).out.contains("docs/internal/b.md"), "沒設定時照收");
    let (_t, root) = kb_project("t15-kg-set", json!({"secret_files_extra": ["(^|/)internal/"]}));
    let o = must(awf(&["kg", "build"], &root, None), "kg build");
    let hit = awf(&["kg", "locate", "innermarker"], &root, None);
    assert!(!hit.out.contains("docs/internal/b.md"), "命中的檔不進圖：{}\n{}", o.all(), hit.all());
    let (_t, root) = kb_project("t15-kg-bad", json!({"secret_files_extra": ["internal/("]}));
    let o = awf(&["kg", "build"], &root, None);
    assert!(o.code != 0 && o.all().contains("機敏檔的設定用不了"), "寫壞了不建圖：{}", o.all());
}

// ------------------------------------------------------------------ T17：資料包的大小（knowledge.pack）

/// 有文件、有一份 300 行需求檔的專案。
fn pack_project(tag: &str, extra: Value) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("ws");
    mk(&root, &builtin_profile(extra));
    for i in 1..=4 {
        write(&root.join(format!("docs/export-{i}.md")), &format!("# 匯出報表 {i}\n\n匯出 CSV 報表的說明第 {i} 份，exportmarker。\n"));
    }
    let req: String = (1..=300).map(|i| format!("第 {i} 行：匯出 CSV 報表 exportmarker\n")).collect();
    write(&root.join("docs/req.md"), &req);
    write_json(&root.join(RUN).join("run.json"), &json!({"requirement": "docs/req.md"}));
    (t, root)
}

fn pack_md(root: &Path) -> String {
    must(awf(&["pack", "plan", "--run", RUN], root, None), "pack plan");
    std::fs::read_to_string(root.join(RUN).join("plan/pack.md")).expect("pack.md")
}

fn req_lines(pack: &str) -> usize {
    pack.lines().filter(|l| l.starts_with("第 ") && l.contains(" 行：")).count()
}

fn kb_rows(pack: &str) -> usize {
    pack.lines().filter(|l| l.starts_with("- docs/") && l.contains(" ｜ ")).count()
}

#[test]
fn t17_pack_sizes_follow_the_profile() {
    let (_t, root) = pack_project("t17-default", json!({}));
    let base = pack_md(&root);
    assert_eq!(req_lines(&base), 200, "預設貼 200 行");
    assert!(base.contains("共 300 行"), "{base}");
    assert!(kb_rows(&base) >= 2, "預設的知識庫命中不只一筆：{base}");

    let (_t, root) = pack_project("t17-set", json!({"knowledge": {"pack": {"requirement_lines": 50, "top": 1}}}));
    let set = pack_md(&root);
    assert_eq!(req_lines(&set), 50, "requirement_lines=50");
    assert!(set.contains("共 300 行"), "省略的說明照舊");
    assert_eq!(kb_rows(&set), 1, "top=1 只取一筆：{set}");
    assert_eq!(pack_md(&root), set, "同樣的輸入重跑逐字相同（可重現）");

    // 不合範圍、型別不對：那一項用預設，資料包和沒設定時逐字相同
    for bad in [json!({"requirement_lines": 5, "top": 0}), json!({"requirement_lines": "50", "top": 1.5, "budget": true}), json!([1])] {
        let (_t, root) = pack_project("t17-bad", json!({"knowledge": {"pack": bad.clone()}}));
        assert_eq!(pack_md(&root), base, "{bad}");
    }
}

// ------------------------------------------------------------------ T19：副檔名與工具定位的門檻

fn kb_ext_project(tag: &str, know: Value) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("ws");
    mk(&root, &builtin_profile(json!({"knowledge": know})));
    write(&root.join("docs/a.md"), "# 正常\n\n內文 okmarker\n");
    write(&root.join("docs/b.rst"), "標題\n====\n\n內文 rstmarker\n");
    (t, root)
}

#[test]
fn t19_kb_extensions_follow_the_profile() {
    let (_t, root) = kb_ext_project("t19-ext-default", json!({"sources": ["docs/**/*"]}));
    must(awf(&["kb", "build"], &root, None), "kb build");
    assert!(!awf(&["kb", "query", "rstmarker"], &root, None).out.contains("b.rst"), "預設不收 .rst");
    assert!(awf(&["kb", "query", "okmarker"], &root, None).out.contains("docs/a.md"));

    let (_t, root) = kb_ext_project("t19-ext-set", json!({"sources": ["docs/**/*"], "extensions": [".md", ".rst"]}));
    must(awf(&["kb", "build"], &root, None), "kb build");
    assert!(awf(&["kb", "query", "rstmarker"], &root, None).out.contains("docs/b.rst"), "列了 .rst 就收");
    assert!(awf(&["kb", "query", "okmarker"], &root, None).out.contains("docs/a.md"));

    for bad in [json!(["rst"]), json!([".RST"]), json!(".rst"), json!([]), json!([".md", 3])] {
        let (_t, root) = kb_ext_project("t19-ext-bad", json!({"sources": ["docs/**/*"], "extensions": bad.clone()}));
        let o = must(awf(&["kb", "build"], &root, None), "kb build");
        assert!(o.all().contains("knowledge.extensions 不合規定"), "{bad}：要警告：{}", o.all());
        assert!(!awf(&["kb", "query", "rstmarker"], &root, None).out.contains("b.rst"), "{bad}：退回預設、不收 .rst");
        assert!(awf(&["kb", "query", "okmarker"], &root, None).out.contains("docs/a.md"), "{bad}：預設的照收");
    }
}

fn locate_project(tag: &str, locate: Option<Value>) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("ws");
    let extra = match locate {
        Some(l) => json!({"knowledge": {"locate": l}}),
        None => json!({}),
    };
    mk(&root, &builtin_profile(extra));
    for (dir, n) in [("src", 4), ("vendor/lib", 2)] {
        for i in 1..=n {
            write(&root.join(format!("app/{dir}/export_{i}.py")), &format!("def export_csv_report_{i}(rows):\n    # exportcsv reportrows 匯出報表\n    return rows\n"));
        }
    }
    (t, root)
}

fn locate_json(root: &Path) -> (String, Vec<String>) {
    let o = must(awf(&["locate", "--repo", "app", "--requirement", "export_csv_report exportcsv reportrows 匯出報表", "--angle", "code-contracts", "--json"], root, None), "locate");
    let v: Value = serde_json::from_str(&o.out).expect("JSON");
    let paths = v["code-contracts"]["locations"].as_array().cloned().unwrap_or_default().iter().map(|l| l["path"].as_str().unwrap_or_default().to_string()).collect();
    (o.out, paths)
}

#[test]
fn t19_locate_thresholds_follow_the_profile() {
    let (_t, root) = locate_project("t19-loc-default", None);
    let (base, paths) = locate_json(&root);
    assert!(paths.len() >= 4 && paths.iter().any(|p| p.starts_with("vendor/")), "預設會看 vendor/、段數不只 2：{base}");

    let (_t, root) = locate_project("t19-loc-set", Some(json!({"prune": ["vendor"], "max_segments": 2})));
    let (out, paths) = locate_json(&root);
    assert!(!paths.is_empty() && paths.len() <= 2, "max_segments=2：{out}");
    assert!(paths.iter().all(|p| !p.starts_with("vendor/")), "prune 加了 vendor：{out}");

    let (_t, root) = locate_project("t19-loc-prune", Some(json!({"prune": ["vendor"]})));
    let (out, paths) = locate_json(&root);
    assert!(paths.iter().all(|p| !p.starts_with("vendor/")) && paths.iter().any(|p| p.starts_with("src/")), "只加 prune：{out}");

    // 不合規定：那一項用預設，輸出和沒設定時逐字相同（prune 有一項不對就整份不用；內建的略過清單拿不掉）
    for bad in [json!({"max_segments": 0, "min_coverage": 7, "segment_lines": 1}), json!({"prune": ["a/b", "vendor"]}), json!({"prune": "vendor", "max_files": 1}), json!("x")] {
        let (_t, root) = locate_project("t19-loc-bad", Some(bad.clone()));
        assert_eq!(locate_json(&root).0, base, "{bad}");
    }
}

// ------------------------------------------------------------------ T24：長輸出濃縮（limits.run_brief）

fn brief_project(tag: &str, extra: Value) -> (Tmp, PathBuf) {
    let t = Tmp::new(tag);
    let root = t.path().join("ws");
    mk(&root, &builtin_profile(extra));
    let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    write(&root.join("out.txt"), &text);
    (t, root)
}

/// 濃縮的文字（拿掉會變的 log 路徑那一行）。
fn brief_text(root: &Path, extra_args: &[&str]) -> String {
    let log = s(&root.join("_tpm/runs/r1/logs/b.log"));
    let mut a = vec!["run-brief", "--log", &log];
    a.extend(extra_args);
    #[cfg(unix)]
    a.extend(["--", "cat", "out.txt"]);
    #[cfg(not(unix))]
    a.extend(["--", "cmd", "/c", "type", "out.txt"]);
    must(awf(&a, root, None), "run-brief").out
}

fn shown_lines(text: &str) -> usize {
    text.lines().filter(|l| l.trim_start().starts_with("line ")).count()
}

#[test]
fn t24_run_brief_sizes_follow_the_profile() {
    let (_t, root) = brief_project("t24-default", json!({}));
    let base = brief_text(&root, &[]);
    assert_eq!(shown_lines(&base), 40, "認不得框架時預設留 40 行：{base}");

    let (_t, root) = brief_project("t24-set", json!({"limits": {"run_brief": {"unknown_tail": 7}}}));
    let set = brief_text(&root, &[]);
    assert_eq!(shown_lines(&set), 7, "unknown_tail=7：{set}");
    assert_eq!(shown_lines(&brief_text(&root, &["--tail", "3"])), 3, "--tail 有給以它為準");

    for bad in [json!({"unknown_tail": 0}), json!({"unknown_tail": "7", "tail": 9999, "max_fails": -1}), json!(7)] {
        let (_t, root) = brief_project("t24-bad", json!({"limits": {"run_brief": bad.clone()}}));
        assert_eq!(shown_lines(&brief_text(&root, &[])), 40, "{bad}：退回預設");
    }
}

// ------------------------------------------------------------------ T28：閘門重點列多少（limits.display）

/// design 存檔後狀態機回的閘門重點裡，agents 有幾筆。
fn gate_agents(tag: &str, extra: Value) -> (usize, Value) {
    let t = Tmp::new(tag);
    let root = t.path().join("proj");
    mk(&root, &builtin_profile(extra));
    plan(&root);
    must(approve(&root, "plan", "human"), "approve plan");
    must(save(&root, "design", design_of(json!([task(&["AC-1 下載"])]))), "stage design");
    let n = awf(&["next", "--run", RUN], &root, None);
    let act: Value = serde_json::from_str(&n.out).unwrap_or_else(|e| panic!("next 的輸出不是 JSON（{e}）：{}", n.all()));
    assert!(act["action"] == "gate" && act["stage"] == "design", "{act}");
    (act["highlights"]["agents"].as_array().map(Vec::len).unwrap_or(0), act)
}

#[test]
fn t28_gate_highlights_follow_the_profile() {
    let (n, base) = gate_agents("t28-default", json!({}));
    assert_eq!(n, 2, "design 段有兩份交件：{base}");
    assert_eq!(gate_agents("t28-set", json!({"limits": {"display": {"agents": 1}}})).0, 1, "agents=1 只列一筆");
    for bad in [json!({"agents": 0}), json!({"agents": "1"}), json!({"agents": 9999}), json!([1])] {
        let (n, act) = gate_agents("t28-bad", json!({"limits": {"display": bad.clone()}}));
        assert_eq!(n, 2, "{bad}：退回預設");
        assert_eq!(act["highlights"], base["highlights"], "{bad}：重點和沒設定時相同");
    }
}

// ------------------------------------------------------------------ T23：watcher 的節奏（progress.watch）

/// 單次模式的輸出不受節奏設定影響；命令列旗標寫壞照舊回錯誤（設定不會蓋掉這個檢查）。
#[test]
fn t23_watch_once_is_unchanged_by_the_setting() {
    let once = |tag: &str, extra: Value| {
        let t = Tmp::new(tag);
        let root = t.path().join("proj");
        mk(&root, &builtin_profile(extra));
        plan(&root);
        let o = must(awf(&["watch", "--run", RUN, "--once", "--json"], &root, None), "watch --once");
        let bad = awf(&["watch", "--run", RUN, "--once", "--interval", "x"], &root, None);
        assert!(bad.code == 1 && bad.all().contains("要是整數"), "{}", bad.all());
        let mut v: Value = serde_json::from_str(&o.out).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{}", o.all()));
        // 拿掉每次都不同的欄位（時間、暫存目錄）
        for k in ["at", "run", "run_dir", "updated_at"] {
            if let Some(m) = v.as_object_mut() {
                m.remove(k);
            }
        }
        v
    };
    let base = once("t23-default", json!({}));
    assert_eq!(once("t23-set", json!({"progress": {"watch": {"interval": 30, "stall_seconds": 60, "progress_step": 2000}}})), base);
    assert_eq!(once("t23-bad", json!({"progress": {"watch": {"interval": 0, "stall_seconds": "x", "progress_step": 1}}})), base);
}

#[allow(dead_code)]
fn unused_helpers(p: &Path) -> String {
    let _ = awf_env(&[], p, None, &[]);
    s(p)
}
