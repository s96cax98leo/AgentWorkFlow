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

#[allow(dead_code)]
fn unused_helpers(p: &Path) -> String {
    let _ = awf_env(&[], p, None, &[]);
    s(p)
}
