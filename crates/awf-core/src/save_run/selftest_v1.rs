//! `awf save-run --selftest` 的 1.0 案例：規格層（S4）、連結帳本（D1）、連結檢查（D6）、對外介面（E3）。
//! 每一項都有該擋、該放、沒啟用時不變。在自己的暫存目錄跑，不碰 repo。

use super::py;
use super::*;
use crate::{merge_gate, recipe, schema, spec};
use serde_json::{json, Value};

const ORDERS: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n";
const DELTA: &str = "## ADDED Requirements\n\n### Requirement: 申請退款\n系統 SHALL 讓會員在七天內申請退款。\n\n#### Scenario: 七天內\n- **WHEN** 收貨未滿七天\n- **THEN** 受理退款\n\n#### Scenario: 超過七天\n- **WHEN** 收貨超過七天\n- **THEN** 拒絕並說明\n";
const BAD_DELTA: &str = "## MODIFIED Requirements\n\n### Requirement: 不存在的需求\n系統 SHALL 做一件事。\n\n#### Scenario: 某情況\n- **WHEN** 甲\n- **THEN** 乙\n";
const PROPOSAL: &str = "# 退款\n\n## Why\n示範客戶 ACME 的會員要能退款。\n\n## What Changes\n- 訂單加退款申請\n\n## Non-Goals\n- 不做部分退款\n";
const INTENT: &str = "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n";
const IN7: &str = "orders/申請退款#七天內";
const OVER7: &str = "orders/申請退款#超過七天";

fn put(path: &str, text: &str) {
    let _ = std::fs::create_dir_all(py::pparent(path));
    let _ = std::fs::write(path, text.as_bytes());
}

fn slurp(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

fn env(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

fn task(id: &str, acc: &[&str], refs: &[&str], owns: &[&str]) -> Value {
    json!({"id": id, "title": id, "kind": "code", "owns": owns, "depends_on": [], "acceptance": acc, "spec_refs": refs})
}

fn design_of(subs: Value) -> Value {
    json!({"design": env(json!({"epic": "refund", "subtasks": subs})), "challenge": env(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "查過"}]}))})
}

fn test_of(rows: &[(&str, &str)]) -> Value {
    let crit: Vec<Value> = rows.iter().map(|(id, cmd)| json!({"id": id, "criterion": "c", "command": cmd, "result": "pass", "evidence": "e"})).collect();
    json!({"verifier": env(json!({"commit": "a".repeat(40), "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": crit}))})
}

struct T {
    ctx: Ctx,
    proj: String,
    n: usize,
}

impl T {
    fn call(&self, a: &[&str], stdin: Option<&Value>) -> (i32, String) {
        let argv: Vec<String> = a.iter().map(|x| (*x).to_string()).collect();
        let text = stdin.map(|v| recipe::dumps(v, None)).unwrap_or_default();
        let (code, out, err) = run_captured(&argv, &text, &self.ctx);
        (code, out + &err)
    }

    fn profile(&self, extra: Value) {
        let mut base = json!({"recipes": recipe::builtin_recipes(), "modifiers": recipe::builtin_modifiers()});
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        put(&self.ctx.profile, &recipe::dumps(&base, None));
    }

    fn epic_dir(&self) -> String {
        format!("{}/repo/.worktrees/refund", self.proj)
    }

    /// 新的 run（沒有 recipe 的完整流程），plan 存好；`with_intent`＝信封列出 intent.md。回傳（目錄、plan 的 exit code、輸出）。
    fn plan(&mut self, with_intent: bool) -> (String, i32, String) {
        self.n += 1;
        let d = format!("{}/_runs/v{}", self.proj, self.n);
        self.call(&["init", &d, "--event", "sdlc", "--target", &format!("{}/repo", self.proj), "--requirement", "r"], None);
        let docs = if with_intent { json!([{"path": "specs/refund/intent.md", "kind": "intent", "audience": "owner"}]) } else { json!([]) };
        let (c, o) = self.call(&["stage", &d, "plan"], Some(&json!({"stage": "plan", "epic_dir": self.epic_dir(), "intent": env(json!({"documents": docs}))})));
        (d, c, o)
    }

    /// plan 核准之後存 design。
    fn design(&mut self, with_intent: bool, subs: Value) -> (String, i32, String) {
        let (d, _, _) = self.plan(with_intent);
        self.call(&["approve", &d, "plan", "--by", "human", "--at", "t"], None);
        let (c, o) = self.call(&["stage", &d, "design"], Some(&design_of(subs)));
        (d, c, o)
    }

    /// design 核准、build 與 sync 存好之後存 test。
    fn test(&mut self, with_intent: bool, subs: Value, rows: &[(&str, &str)]) -> (String, i32, String) {
        let (d, _, _) = self.design(with_intent, subs);
        self.call(&["approve", &d, "design", "--by", "human", "--at", "t"], None);
        let build = json!({"x": env(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/refund.py", "src/export.py"], "tests": []}))});
        self.call(&["stage", &d, "build"], Some(&build));
        self.call(&["stage", &d, "sync"], Some(&build));
        let (c, o) = self.call(&["stage", &d, "test"], Some(&test_of(rows)));
        (d, c, o)
    }

    fn st(&self, d: &str, stage: &str) -> Value {
        load(&self.ctx, d).map(|v| v["stages"][stage].clone()).unwrap_or(Value::Null)
    }

    fn ledger(&self) -> String {
        format!("{}/_runs/{LEDGER}", self.proj)
    }

    fn active(&self) -> Vec<Value> {
        active_links(&slurp(&self.ledger()))
    }
}

fn kinds(rows: &[Value], run: &str) -> Vec<String> {
    let mut k: Vec<String> = rows.iter().filter(|r| r["run"] == *py::pname(run)).filter_map(|r| r["kind"].as_str().map(str::to_string)).collect();
    k.sort();
    k.dedup();
    k
}

fn has_link(rows: &[Value], kind: &str, from: &str, to: &str) -> bool {
    rows.iter().any(|r| r["kind"] == kind && r["from"]["id"] == from && r["to"]["id"] == to)
}

/// 跑 1.0 的案例，把結果行寫進 `out`；全部通過回 true。
pub fn run(ctx: &Ctx, out: &mut String) -> bool {
    let mut ok_all = true;
    macro_rules! check {
        ($name:expr, $cond:expr, $detail:expr) => {{
            let cond: bool = $cond;
            let tail = if cond {
                String::new()
            } else {
                let detail: String = $detail;
                let chars: Vec<char> = detail.chars().collect();
                format!("：{}", chars.iter().skip(chars.len().saturating_sub(500)).collect::<String>())
            };
            out.push_str(&format!("  [{}] {}{tail}\n", if cond { "通過" } else { "失敗" }, $name));
            ok_all = ok_all && cond;
        }};
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("awf-save-run-v1.{}{nanos}", std::process::id())).to_string_lossy().replace('\\', "/");
    let tmp = crate::kb::pyish::resolve(&tmp).unwrap_or(tmp);
    let proj = format!("{tmp}/proj");
    let inner = Ctx { teams: ctx.abs(&ctx.teams), profile: format!("{proj}/.claude/teams/methodology.json"), pack_root: proj.clone(), cwd: proj.clone(), today: ctx.today.clone() };
    let mut t = T { ctx: inner, proj: proj.clone(), n: 0 };
    let schemas = schema::Schemas::new(t.ctx.abs(&t.ctx.teams));
    let human = ["--by", "human", "--at", "t"];
    let policy = ["--by", "policy", "--at", "t", "--note", "p"];
    macro_rules! approve {
        ($d:expr, $st:expr, $who:expr) => {{
            let mut a: Vec<&str> = vec!["approve", $d, $st];
            a.extend($who);
            t.call(&a, None)
        }};
    }
    put(&format!("{}/specs/refund/intent.md", t.epic_dir()), INTENT);
    put(&format!("{proj}/openspec/specs/orders/spec.md"), ORDERS);
    put(&format!("{proj}/openspec/changes/refund/proposal.md"), PROPOSAL);
    put(&format!("{proj}/openspec/changes/refund/tasks.md"), "## 1. 實作\n- [ ] 1.1 退款申請\n");
    put(&format!("{proj}/openspec/changes/refund/specs/orders/spec.md"), DELTA);
    let openspec = json!({"requirements": {"source": "openspec", "path": "openspec"}});
    let good = || json!([task("t1", &[&format!("{IN7} 受理"), &format!("{OVER7} 拒絕"), "lint 通過"], &["orders/申請退款"], &["src/refund.py"])]);

    // ── S4 規格層接進存檔與閘門（需求來源是 openspec 才啟用）
    t.profile(json!({}));
    let (d, c, o) = t.plan(false);
    let (d2, c2, o2) = t.design(true, good());
    check!(
        "S4 沒選 openspec：plan 照舊要信封列出 intent 文件；design 不做規格檢查、run.json 沒有 spec_issues、沒有連結帳本",
        c != 0 && o.contains("沒有 kind=intent 的文件") && t.st(&d, "plan").get("spec_issues").is_none() && c2 == 0 && !o2.contains("規格檢查") && t.st(&d2, "design").get("spec_issues").is_none()
            && !std::path::Path::new(&t.ledger()).exists(),
        o + &o2
    );
    t.profile(openspec.clone());
    let (_, c, o) = t.plan(false);
    check!("S4 intent：proposal 就是 intent（Why／What Changes／Non-Goals 對到必要節，驗收條件由差異的情境滿足）→ plan 通過", c == 0 && o.contains("狀態 done"), o);
    put(&format!("{proj}/openspec/changes/refund/proposal.md"), &PROPOSAL.replace("## Non-Goals\n- 不做部分退款\n", ""));
    let (_, c, o) = t.plan(false);
    check!("S4 intent：proposal 少了「不做的事」對應的節 → plan 判 invalid，列出缺哪一節", c != 0 && o.contains("缺必要的節") && o.contains("不做的事") && o.contains("狀態 invalid"), o);
    put(&format!("{proj}/openspec/changes/refund/proposal.md"), &PROPOSAL.replace("## Non-Goals\n", "## Non-Goals\n\n## 其他\n"));
    let (_, c, o) = t.plan(false);
    check!("S4 intent：對到的節只有標題沒有內容 → 一樣算缺", c != 0 && o.contains("不做的事（只有標題沒有內容）"), o);
    put(&format!("{proj}/openspec/changes/refund/proposal.md"), &PROPOSAL.replace("## Non-Goals", "## 先不碰"));
    t.profile(json!({"requirements": {"source": "openspec", "path": "openspec", "section_map": {"不做的事": ["先不碰"]}}}));
    let (_, c, o) = t.plan(false);
    t.profile(json!({"requirements": {"source": "openspec", "path": "openspec", "section_map": {"驗收條件": ["Acceptance"]}}}));
    let (_, c2, o2) = t.plan(false);
    check!(
        "S4 intent：對照表可由 profile 覆寫（requirements.section_map）；覆寫後對不到就是缺",
        c == 0 && c2 != 0 && o2.contains("驗收條件"),
        o + &o2
    );
    put(&format!("{proj}/openspec/changes/refund/proposal.md"), PROPOSAL);
    t.profile(json!({"requirements": {"source": "openspec", "path": "nowhere"}}));
    let (_, c, o) = t.plan(false);
    std::fs::rename(format!("{proj}/openspec/changes/refund"), format!("{proj}/openspec/changes/other")).ok();
    t.profile(openspec.clone());
    let (_, c2, o2) = t.plan(false);
    std::fs::rename(format!("{proj}/openspec/changes/other"), format!("{proj}/openspec/changes/refund")).ok();
    check!(
        "S4 intent：規格目錄讀不到、找不到這個 run 對應的變更 → invalid（不當成沒有規格）",
        c != 0 && o.contains("讀不到規格目錄") && c2 != 0 && o2.contains("找不到這個 run 對應的進行中變更"),
        o + &o2
    );
    let (d, c, o) = t.design(false, good());
    let (c2, o2) = approve!(&d, "design", policy);
    check!("S4 design：spec_refs 與 acceptance 填的 id 都在規格（現行＋變更的差異）裡、spec validate 沒問題 → 沒有 spec_issues，可以政策核准", c == 0 && t.st(&d, "design").get("spec_issues").is_none() && c2 == 0, o + &o2);
    let (d, c, o) = t.design(false, json!([task("t1", &[&format!("{IN7} 受理"), "orders/沒這條#x 過"], &["orders/查無此節", "spec.md#退款"], &["src/refund.py"])]));
    let (c2, o2) = approve!(&d, "design", policy);
    let (c3, o3) = approve!(&d, "design", human);
    let si: Vec<String> = t.st(&d, "design")["spec_issues"].as_array().map(|a| a.iter().map(py::s).collect()).unwrap_or_default();
    check!(
        "S4 design：id 在規格裡找不到、變更的情境沒有子任務對應 → 記 spec_issues、政策核准被擋；人工核准照記並警告",
        c == 0 && si.iter().any(|m| m.contains("spec_refs 有 2 項")) && si.iter().any(|m| m.contains("acceptance 有 1 項")) && si.iter().any(|m| m.contains(OVER7)) && c2 != 0 && o2.contains("規格檢查：")
            && c3 == 0 && o3.contains("規格檢查有 3 項問題"),
        o + &o2 + &o3
    );
    put(&format!("{proj}/openspec/changes/refund/specs/orders/spec.md"), &format!("{DELTA}\n{BAD_DELTA}"));
    let (d, _, o) = t.design(false, good());
    let (c2, o2) = approve!(&d, "design", policy);
    put(&format!("{proj}/openspec/changes/refund/specs/orders/spec.md"), DELTA);
    let (c3, o3) = approve!(&d, "design", policy);
    check!(
        "S4 design：存檔時跑 spec validate（差異指到不存在的需求）→ 政策核准被擋；規格修好後政策核准重算就過（不採信 run.json）",
        o.contains("spec validate：") && o.contains("不存在的需求") && c2 != 0 && c3 == 0,
        o + &o2 + &o3
    );
    let shard = format!("orders:{OVER7}");
    let (d, c, o) = t.test(false, good(), &[(IN7, "pytest tests/test_refund.py::test_in7"), (&shard, "pytest tests/test_refund.py::test_over7")]);
    let (c2, o2) = approve!(&d, "test", policy);
    check!("S4 test：逐條結果用同一組 id（分片前綴去掉再對）→ 沒有 spec_issues，可以政策核准", c == 0 && t.st(&d, "test").get("spec_issues").is_none() && c2 == 0, o + &o2);
    let (d, _, o) = t.test(false, good(), &[(IN7, "pytest tests/test_refund.py"), ("C9", "pytest tests/test_x.py")]);
    let (c2, o2) = approve!(&d, "test", policy);
    check!(
        "S4 test：有對不到規格的 id、有情境沒驗 → 都列出來，政策核准被擋",
        o.contains("逐條結果有 1 個 id") && o.contains("C9") && o.contains("沒有逐條結果") && o.contains(OVER7) && c2 != 0 && o2.contains("規格檢查："),
        o + &o2
    );
    // 需求設定的讀法和 merge gate 一樣：guard.config.json 有設 path／command 時以它為準
    let teams2 = format!("{tmp}/teams2");
    put(&format!("{teams2}/guard.config.json"), &json!({"requirements": {"source": "json", "path": "reqs.json"}}).to_string());
    let c2 = Ctx { teams: teams2.clone(), ..t.ctx.clone() };
    let off = spec_setting(&c2, &openspec).is_none();
    put(&format!("{teams2}/guard.config.json"), &json!({"requirements": {"source": "json", "path": "", "command": ""}}).to_string());
    let on = spec_setting(&c2, &openspec).is_some();
    put(&format!("{teams2}/guard.config.json"), &json!({"requirements": {"source": "openspec", "path": "openspec"}}).to_string());
    let on2 = spec_setting(&c2, &json!({})).is_some();
    check!("S4 需求設定：guard.config.json 有設別的來源時以它為準（不啟用）；沒設才用 profile；兩邊都能選 openspec", off && on && on2, format!("{off} {on} {on2}"));
    // merge gate 的 adapter 共用同一份規格與 id
    let rc = json!({"source": "openspec", "path": "openspec"});
    let got = merge_gate::adapter_openspec(&rc, &proj, None);
    let ids: Vec<String> = got.as_ref().map(|(r, _)| r.iter().map(|x| x.id.clone()).collect()).unwrap_or_default();
    put(&format!("{proj}/openspec/changes/second/proposal.md"), PROPOSAL);
    put(&format!("{proj}/openspec/changes/second/tasks.md"), "- [ ] 1.1 x\n");
    let two = merge_gate::adapter_openspec(&rc, &proj, None);
    let named = merge_gate::adapter_openspec(&rc, &proj, Some("refund"));
    let empty = merge_gate::adapter_openspec(&rc, &proj, Some("second"));
    let _ = std::fs::remove_dir_all(format!("{proj}/openspec/changes/second"));
    let fail_text = |r: &Result<(Vec<merge_gate::Req>, String), merge_gate::Fail>| match r {
        Err(merge_gate::Fail::Input(m)) => m.clone(),
        _ => String::new(),
    };
    check!(
        "S4 merge gate：adapter openspec 回這次變更的情境（穩定 id）；變更判不出來、差異沒有東西要驗 → 輸入缺漏（hold）",
        ids == [IN7.to_string(), OVER7.to_string()] && got.as_ref().is_ok_and(|(_, l)| l == "openspec:openspec#refund") && fail_text(&two).contains("2 個進行中的變更") && named.is_ok()
            && fail_text(&empty).contains("沒有任何要驗的"),
        format!("{ids:?} {} {}", fail_text(&two), fail_text(&empty))
    );
    let (item_ok, known) = match spec::open_source(&spec::source_cfg(&rc, &proj).unwrap_or_else(|_| unreachable_cfg())) {
        Ok(o) => (spec::change_items(&o.tree, "refund").is_some_and(|i| i.len() == 2), spec::known_ids(&o.tree, Some("refund"))),
        Err(_) => (false, Default::default()),
    };
    check!("S4 規格層介面：change_items、known_ids（現行＋差異）", item_ok && known.contains("orders/建立訂單#庫存足夠") && known.contains(IN7) && known.contains("orders/申請退款"), format!("{known:?}"));

    // ── D1 連結帳本（profile 有 links 才寫）
    let ac_task = |refs: &[&str]| json!([task("t1", &["AC-1 匯出 CSV"], refs, &["src/export.py"])]);
    t.profile(json!({"links": {}}));
    let (d, c, o) = t.test(true, ac_task(&["spec.md#匯出"]), &[("AC-1", "pytest tests/test_export.py::test_csv")]);
    let rows = t.active();
    let bad_rows: Vec<String> = slurp(&t.ledger()).lines().flat_map(|l| schemas.check_doc(&py::loads(l).unwrap_or(Value::Null), "link").unwrap_or_else(|e| vec![e])).collect();
    let sha = "a".repeat(40);
    check!(
        "D1 帳本：plan／design／build／test 存檔時把宣告的連結附加到 <runs_dir>/links.jsonl，每行符合 link.schema.json、強度 strong",
        c == 0 && kinds(&rows, &d) == ["ac_commit", "ac_file", "ac_spec", "ac_test", "req_run"] && bad_rows.is_empty() && rows.iter().all(|r| r["strength"] == "strong" && r["op"] == "add")
            && has_link(&rows, "req_run", "refund", &py::pname(&d)) && has_link(&rows, "ac_spec", "AC-1", "spec.md#匯出") && has_link(&rows, "ac_file", "AC-1", "src/export.py")
            && has_link(&rows, "ac_test", "AC-1", "tests/test_export.py") && has_link(&rows, "ac_commit", "AC-1", &sha),
        o + &format!("{:?} {bad_rows:?}", kinds(&rows, &d))
    );
    let before = slurp(&t.ledger());
    t.call(&["stage", &d, "test"], Some(&test_of(&[("AC-1", "pytest tests/test_export.py::test_csv")])));
    let same = slurp(&t.ledger()) == before;
    t.call(&["stage", &d, "test"], Some(&test_of(&[("AC-1", "pytest tests/test_export2.py")])));
    let after = slurp(&t.ledger());
    let rows = t.active();
    let mine: Vec<&Value> = rows.iter().filter(|r| r["run"] == *py::pname(&d) && r["stage"] == "test").collect();
    check!(
        "D1 帳本：重存同一段，內容一樣不寫；內容變了先寫一筆 supersede 再寫新的（只追加），有效的連結沒有重複也沒有舊的",
        same && after.starts_with(&before) && after.contains("\"op\": \"supersede\"") && mine.len() == 2 && has_link(&rows, "ac_test", "AC-1", "tests/test_export2.py")
            && !has_link(&rows, "ac_test", "AC-1", "tests/test_export.py"),
        format!("{same} {}", mine.len())
    );
    let (c, o) = t.call(&["decide", &d, "--question", "匯出要不要含表頭", "--answer", "要", "--by", "human", "--at", "t"], None);
    let (c2, o2) = t.call(&["approve", &d, "test", "--by", "human", "--at", "t", "--answer", "逾時怎麼算=照 30 秒"], None);
    let dec: Vec<Value> = t.active().into_iter().filter(|r| r["kind"] == "decision_req").collect();
    check!(
        "D1 帳本：decide 與 approve --answer 記「決策 → 需求」",
        c == 0 && c2 == 0 && dec.len() == 2 && dec.iter().all(|r| r["to"]["id"] == "refund" && r["from"]["type"] == "decision") && dec[0]["stage"] == "decide" && dec[1]["stage"] == "approve:test",
        o + &o2 + &format!("{dec:?}")
    );
    let deliver = json!({"x": env(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": [], "tests": []})),
        "deliverables": {"docs": [], "updated": [{"id": "sad", "file": "docs/SAD.md", "sections": ["匯出"]}], "summary": null}});
    t.call(&["stage", &d, "deliver"], Some(&deliver));
    let merge = json!({"merged": true, "head_sha": "b".repeat(40), "x": env(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": [], "tests": []}))});
    t.call(&["approve", &d, "deliver", "--by", "human", "--at", "t"], None);
    t.call(&["stage", &d, "merge"], Some(&merge));
    let rows = t.active();
    check!(
        "D1 帳本：deliver 記「改動 → 交付文件小節」、merge 記「AC → commit」",
        has_link(&rows, "change_deliverable", &py::pname(&d), "sad#匯出") && has_link(&rows, "ac_commit", "AC-1", &"b".repeat(40)),
        format!("{:?}", kinds(&rows, &d))
    );
    let _ = std::fs::remove_file(t.ledger());
    let _ = std::fs::create_dir_all(t.ledger());
    let (d, c, o) = t.design(true, ac_task(&["spec.md#匯出"]));
    let _ = std::fs::remove_dir_all(t.ledger());
    check!("D1 帳本：寫不了（輔助功能）→ 只警告，這一段照常存檔", c == 0 && o.contains("連結帳本") && o.contains("警告") && t.st(&d, "design")["status"] == "done", o);
    t.profile(openspec_with(&openspec, json!({"links": {}})));
    let (d, _, _) = t.design(false, good());
    let rows = t.active();
    check!(
        "D1 帳本：需求來源是 openspec 時，兩端用規格層的穩定 id（變更與需求 → run、情境 → spec 小節）",
        has_link(&rows, "req_run", "refund", &py::pname(&d)) && has_link(&rows, "req_run", "orders/申請退款", &py::pname(&d)) && has_link(&rows, "ac_spec", IN7, IN7)
            && has_link(&rows, "ac_spec", IN7, "orders/申請退款") && has_link(&rows, "ac_file", OVER7, "src/refund.py"),
        format!("{rows:?}")
    );
    let _ = std::fs::remove_file(t.ledger());

    // ── D6 閘門端的連結檢查（不讀帳本，從各段結果重算）
    t.profile(json!({"links": {"enforce": false}}));
    let (d, c, o) = t.test(true, ac_task(&[]), &[("C1", "pytest tests/test_export.py")]);
    let (c2, o2) = approve!(&d, "test", policy);
    let gaps: Vec<String> = t.st(&d, "test")["link_gaps"].as_array().map(|a| a.iter().map(py::s).collect()).unwrap_or_default();
    let (_, st_text) = t.call(&["status", &d], None);
    check!(
        "D6 連結檢查：links.enforce 沒開 → 缺的環節記進 run.json 的 link_gaps、列警告、status 看得到，政策核准照過",
        c == 0 && gaps == ["AC-1：缺 spec 小節、測試、commit"] && o.contains("連結檢查：AC-1：缺 spec 小節、測試、commit（links.enforce 沒開，只提醒）") && c2 == 0 && st_text.contains("連結不齊 1 項"),
        o + &o2 + &st_text
    );
    t.profile(json!({"links": {"enforce": true}}));
    let (d, _, o) = t.test(true, ac_task(&[]), &[("C1", "pytest tests/test_export.py")]);
    let (c2, o2) = approve!(&d, "test", policy);
    // 帳本與 run.json 的紀錄都不是判定依據：補假連結、清掉 link_gaps，政策核准仍然重算而擋
    put(&t.ledger(), &format!("{}\n", json!({"schema_version": 1, "op": "add", "kind": "ac_test", "from": {"type": "ac", "id": "AC-1"}, "to": {"type": "test", "id": "tests/fake.py"}, "run": py::pname(&d), "stage": "test", "source": "x", "strength": "strong"})));
    let mut data = load(&t.ctx, &d).unwrap_or(Value::Null);
    if let Some(m) = data["stages"]["test"].as_object_mut() {
        m.shift_remove("link_gaps");
    }
    let _ = dump(&t.ctx, &d, &data);
    let (c3, o3) = approve!(&d, "test", policy);
    let (c4, o4) = approve!(&d, "test", human);
    check!(
        "D6 連結檢查：links.enforce 開啟 → 不齊就不能政策核准（改帳本、清掉 run.json 的紀錄也一樣，重算為準）；人工核准照記",
        o.contains("不能政策核准") && c2 != 0 && o2.contains("連結檢查：AC-1") && c3 != 0 && o3.contains("連結檢查：AC-1") && c4 == 0,
        o + &o2 + &o3 + &o4
    );
    let _ = std::fs::remove_file(t.ledger());
    let (d, c, o) = t.test(true, ac_task(&["spec.md#匯出"]), &[("AC-1", "pytest tests/test_export.py")]);
    let (c2, o2) = approve!(&d, "test", policy);
    check!("D6 連結檢查：每條 AC 都有 spec 小節、檔案、測試、commit → 沒有 link_gaps，enforce 開著也能政策核准", c == 0 && t.st(&d, "test").get("link_gaps").is_none() && !o.contains("連結檢查") && c2 == 0, o + &o2);
    let (d, _, o) = t.design(true, json!([task("t1", &["匯出要能下載"], &[], &[])]));
    check!(
        "D6 連結檢查：design 段只看 spec 小節與檔案；驗收條件沒有可辨識的 id 也列出來",
        t.st(&d, "design")["link_gaps"].as_array().is_some_and(|a| a.len() == 1 && py::s(&a[0]).contains("沒有可辨識的 id")),
        o
    );
    let _ = std::fs::remove_file(t.ledger());
    t.profile(json!({}));
    let (d, c, o) = t.test(true, ac_task(&[]), &[("C1", "pytest tests/test_export.py")]);
    let (c2, o2) = approve!(&d, "test", policy);
    check!(
        "D6 連結檢查：profile 沒有 links → 不檢查、不寫帳本、輸出與 run.json 不變",
        c == 0 && !o.contains("連結") && t.st(&d, "test").get("link_gaps").is_none() && !std::path::Path::new(&t.ledger()).exists() && c2 == 0,
        o + &o2
    );
    t.profile(json!({"links": {"enforce": "yes"}}));
    let (_, o) = t.call(&["approve", &d, "test", "--by", "policy", "--at", "t2", "--note", "p"], None);
    check!("D6 連結檢查：links.enforce 寫錯型別 → profile 不合規則，政策核准被擋（不默默當成沒開）", o.contains("links.enforce 要是 true 或 false"), o);

    // ── E3 對外介面：schema_version、recipe 跳過的段
    t.profile(json!({}));
    t.n += 1;
    let d = format!("{proj}/_runs/v{}", t.n);
    t.call(&["init", &d, "--event", "sdlc", "--target", &format!("{proj}/repo"), "--requirement", "r"], None);
    let docs_plan = json!({"stage": "plan", "epic_dir": t.epic_dir(), "recipe": {"name": "docs", "modifiers": []},
        "intent": env(json!({"documents": [{"path": "specs/refund/intent.md", "kind": "intent", "audience": "owner"}]}))});
    let (_, o) = t.call(&["stage", &d, "plan"], Some(&docs_plan));
    let (c2, snap) = t.call(&["status", &d, "--json"], None);
    let j = py::loads(&snap).unwrap_or(Value::Null);
    let errs = schemas.check_doc(&j, "run-status").unwrap_or_else(|e| vec![e]);
    let skipped: Vec<String> = j["stages"].as_array().into_iter().flatten().filter(|s| s["skipped"] == true).map(|s| py::s(&s["name"])).collect();
    let want: Vec<String> = SDLC.iter().filter(|s| load(&t.ctx, &d).is_ok_and(|v| v["stages"][**s]["status"] == "skipped")).map(|s| (*s).to_string()).collect();
    check!(
        "E3 對外介面：status --json 有 schema_version、符合 run-status.schema.json；recipe 跳過的段明確標 skipped（其餘是 false）",
        c2 == 0 && j["schema_version"] == STATUS_SCHEMA_VERSION && j["v"] == 1 && errs.is_empty() && !skipped.is_empty() && skipped == want
            && j["stages"].as_array().is_some_and(|a| a.iter().all(|s| s["skipped"].is_boolean() && s["spec_issues"] == 0 && s["link_gaps"] == 0)),
        snap + &format!("{errs:?}{skipped:?}{want:?}") + &o
    );
    let _ = std::fs::remove_dir_all(&tmp);
    ok_all
}

fn openspec_with(base: &Value, extra: Value) -> Value {
    let mut m = base.as_object().cloned().unwrap_or_default();
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        m.insert(k, v);
    }
    Value::Object(m)
}

/// 設定讀不進來時的替代值（selftest 的固定設定不會走到；不用 panic）。
fn unreachable_cfg() -> spec::SourceCfg {
    spec::SourceCfg { root: std::path::PathBuf::from("/nonexistent"), display: "openspec".to_string(), pattern: None, change: String::new(), section_map: None }
}
