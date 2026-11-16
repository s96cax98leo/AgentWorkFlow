//! 自動接點的端到端測試（不花 token）：虛構專案（幾個原始檔、一個 OpenSpec 變更、profile 有 `links`），
//! 每一段給假的段結果，只用 `awf start`／`next`／`done`／`approve` 從開工走到結案，確認：
//!
//! - 每段開跑前知識圖有更新（中途加的符號查得到）、資料包有產生、連結帳本有寫；
//! - merge 之後多一個「併回規格」的動作，做完或略過之後不再出現；
//! - 結案後 vault 有重寫；
//! - 把圖刪掉、或 `knowledge.auto` 全關再跑：流程照樣走完，圖與 vault 不會被建出來；
//! - 中斷回收（E1）：條件齊才收，有一項不確定就不收。
//!
//! 範本與 schema 取自這個 repo 的 `plugin/`（`AWF_PLUGIN_DIR`）；專案用 `awf init` 套出來。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const ORDERS: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n";
const DELTA: &str = "## ADDED Requirements\n\n### Requirement: 申請退款\n系統 SHALL 讓會員在七天內申請退款。\n\n#### Scenario: 七天內\n- **WHEN** 收貨未滿七天\n- **THEN** 受理退款\n\n#### Scenario: 超過七天\n- **WHEN** 收貨超過七天\n- **THEN** 拒絕並說明\n";
const PROPOSAL: &str = "# 退款\n\n## Why\n示範客戶 ACME 的會員要能退款。\n\n## What Changes\n- 訂單加退款申請\n\n## Non-Goals\n- 不做部分退款\n";
const IN7: &str = "orders/申請退款#七天內";
const OVER7: &str = "orders/申請退款#超過七天";

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo 根目錄")
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
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("不是 JSON（{e}）：\n{}", self.all()))
    }
}

/// 暫存專案。
struct Proj {
    tmp: PathBuf,
    dir: PathBuf,
}

impl Drop for Proj {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().expect("有上層目錄")).expect("建目錄");
    std::fs::write(p, text).expect("寫檔");
}

fn load(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).unwrap_or_else(|e| panic!("{} 不是 JSON：{e}", p.display()))
}

fn envl(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

impl Proj {
    /// `auto`＝profile 的 `knowledge.auto`（None＝不寫這個鍵）。
    fn new(tag: &str, auto: Option<Value>) -> Proj {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("awf-e2e-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).expect("建暫存專案");
        let tmp = tmp.canonicalize().expect("解開暫存目錄");
        let p = Proj { dir: tmp.join("proj"), tmp };
        let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", root().join("plugin/workflow.config.example").to_str().unwrap()]);
        assert_eq!(r.code, 0, "awf init：\n{}", r.all());
        p.put("openspec/specs/orders/spec.md", ORDERS);
        p.put("openspec/changes/refund/proposal.md", PROPOSAL);
        p.put("openspec/changes/refund/tasks.md", "## 1. 實作\n- [ ] 1.1 退款申請\n");
        p.put("openspec/changes/refund/specs/orders/spec.md", DELTA);
        p.put("src/refund.py", "def refund(order):\n    return True\n");
        p.put("tests/test_refund.py", "def test_in7():\n    assert True\n");
        let mp = p.dir.join(".claude/teams/methodology.json");
        let mut m = load(&mp);
        m["requirements"] = json!({"source": "openspec", "path": "openspec"});
        m["links"] = json!({});
        if let Some(a) = auto {
            if !m["knowledge"].is_object() {
                m["knowledge"] = json!({});
            }
            m["knowledge"]["auto"] = a;
        }
        put(&mp, &m.to_string());
        p
    }

    fn put(&self, rel: &str, text: &str) {
        put(&self.dir.join(rel), text);
    }

    fn awf_env(&self, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let mut c = Command::new(EXE);
        c.args(args).current_dir(&self.dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
                c.env_remove(k);
            }
        }
        c.env("CLAUDE_PROJECT_DIR", &self.dir).env("AWF_PLUGIN_DIR", root().join("plugin")).env("AWF_TRANSCRIPTS_DIR", self.tmp.join("transcripts"));
        for (k, v) in extra {
            c.env(k, v);
        }
        let o = c.output().expect("啟動 awf");
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
    }

    fn awf(&self, args: &[&str]) -> Out {
        self.awf_env(args, &[])
    }

    fn graph(&self) -> PathBuf {
        self.dir.join("_tpm/kb/graph.db")
    }

    fn vault(&self) -> PathBuf {
        self.dir.join("_tpm/kb/vault")
    }

    fn ledger(&self) -> PathBuf {
        self.dir.join("_tpm/runs/links.jsonl")
    }

    fn run_json(&self, run: &str) -> Value {
        load(&self.dir.join(run).join("run.json"))
    }

    /// 圖裡查不查得到這個符號（`awf kg locate`）。
    fn knows(&self, symbol: &str) -> bool {
        let r = self.awf(&["kg", "locate", symbol]);
        r.code == 0 && r.stdout.contains("src/extra.py")
    }

    /// 某一段的假結果（沒有 recipe 的完整流程；形狀同 `awf save-run --selftest` 的 1.0 案例）。
    fn result(&self, stage: &str) -> Value {
        let task = json!({"id": "t1", "title": "t1", "kind": "code", "owns": ["src/refund.py"], "depends_on": [], "acceptance": [format!("{IN7} 受理"), format!("{OVER7} 拒絕")], "spec_refs": ["orders/申請退款"]});
        let build = |files: Value| json!({"x": envl(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": files, "tests": []}))});
        match stage {
            "plan" => json!({"stage": "plan", "epic_dir": self.dir.to_str().unwrap(), "intent": envl(json!({"documents": []}))}),
            "design" => json!({"design": envl(json!({"epic": "refund", "subtasks": [task]})), "challenge": envl(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "查過"}]}))}),
            "build" | "sync" => build(json!(["src/refund.py"])),
            "test" => {
                let crit: Vec<Value> = [IN7, OVER7].iter().map(|id| json!({"id": id, "criterion": "c", "command": "pytest tests/test_refund.py", "result": "pass", "evidence": "e"})).collect();
                json!({"verifier": envl(json!({"commit": "a".repeat(40), "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": crit}))})
            }
            "deliver" => build(json!([])),
            "merge" => {
                let mut m = build(json!([]));
                m["merged"] = json!(true);
                m["merge_state"] = json!("merged");
                m["head_sha"] = json!("b".repeat(40));
                m
            }
            other => panic!("沒有 {other} 段的假結果"),
        }
    }

    fn start(&self, run: &str) {
        let r = self.awf(&["start", "--run", run, "--event", "sdlc", "--repo", ".", "--epic", "refund", "--requirement", "會員要能退款"]);
        assert_eq!(r.code, 0, "awf start：\n{}", r.all());
    }

    fn next(&self, run: &str, extra: &[&str]) -> Value {
        let mut a = vec!["next", "--run", run];
        a.extend_from_slice(extra);
        let r = self.awf(&a);
        assert_eq!(r.code, 0, "awf next {extra:?}：\n{}", r.all());
        r.json()
    }

    fn done(&self, run: &str, stage: &str) {
        let f = self.tmp.join(format!("{stage}.json"));
        put(&f, &self.result(stage).to_string());
        let r = self.awf(&["done", "--run", run, "--output", f.to_str().unwrap(), "--no-next"]);
        assert_eq!(r.code, 0, "awf done（{stage}）：\n{}", r.all());
    }

    /// 只用 next／done／approve 走到「每一段都完成」。`before_stage(段, 動作)` 在每個 run_workflow 動作回來之後、存結果之前呼叫；
    /// `on_apply(動作)` 處理「併回規格」的動作。回傳動作序列（`動作:段`；併回規格記成 `manual:spec-apply`）。
    fn walk(&self, run: &str, before_stage: &mut dyn FnMut(&str, &Value), on_apply: &mut dyn FnMut(&Value)) -> Vec<String> {
        let mut seq = Vec::new();
        for _ in 0..40 {
            let a = self.next(run, &[]);
            let (action, stage) = (a["action"].as_str().unwrap_or_default().to_string(), a["stage"].as_str().unwrap_or_default().to_string());
            match action.as_str() {
                "run_workflow" => {
                    seq.push(format!("run_workflow:{stage}"));
                    assert!(self.dir.join(a["script"].as_str().unwrap_or("?")).is_file(), "{stage} 段的腳本沒有產生：{a}");
                    assert!(self.dir.join(run).join(&stage).join("pack.md").is_file(), "{stage} 段的資料包沒有產生");
                    before_stage(&stage, &a);
                    self.done(run, &stage);
                }
                "gate" => {
                    seq.push(format!("gate:{stage}"));
                    let r = self.awf(&["approve", "--run", run, "--stage", &stage, "--by", "human"]);
                    assert_eq!(r.code, 0, "awf approve（{stage}）：\n{}", r.all());
                }
                "need_input" => {
                    let mut argv: Vec<String> = Vec::new();
                    for k in a["need"].as_object().expect("need 是物件").keys() {
                        argv.push("--set".to_string());
                        argv.push(format!("{k}=false"));
                    }
                    self.next(run, &argv.iter().map(String::as_str).collect::<Vec<_>>());
                    // 補完參數的那一次 next 已經回了 run_workflow：下一圈再拿一次同一個動作
                }
                "manual" if a.get("apply").is_some() => {
                    seq.push("manual:spec-apply".to_string());
                    on_apply(&a);
                }
                "manual" => {
                    seq.push(format!("manual:{stage}"));
                    self.next(run, &["--set", "pr=7"]);
                }
                "done" => {
                    seq.push("done".to_string());
                    return seq;
                }
                other => panic!("沒預期的動作 {other}：{a}"),
            }
        }
        panic!("走了 40 步還沒走完：{seq:?}");
    }

    fn close(&self, run: &str) -> Out {
        let r = self.awf(&["save-run", "close", self.dir.join(run).to_str().unwrap(), "--by", "human", "--at", "t"]);
        assert_eq!(r.code, 0, "awf save-run close：\n{}", r.all());
        r
    }
}

fn base_seq() -> Vec<String> {
    ["run_workflow:plan", "gate:plan", "run_workflow:design", "gate:design", "run_workflow:build", "run_workflow:sync", "run_workflow:test", "gate:test", "run_workflow:deliver", "gate:deliver", "manual:merge", "run_workflow:merge"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn with_apply() -> Vec<String> {
    let mut s = base_seq();
    s.push("manual:spec-apply".to_string());
    s.push("done".to_string());
    s
}

/// 圖已經在（人建過一次）、`knowledge.auto` 沒寫：每段開跑前更新圖、帳本有寫、merge 之後問一次併回規格、結案後重寫 vault。
/// 然後把圖刪掉再跑一個 run：照樣走完，圖與 vault 不會被建出來。
#[test]
fn full_flow_updates_graph_ledger_spec_and_vault() {
    let p = Proj::new("full", None);
    let r = p.awf(&["kg", "build"]);
    assert_eq!(r.code, 0, "awf kg build：\n{}", r.all());
    assert!(p.graph().is_file() && !p.knows("brand_new_helper"));
    let run = "_tpm/runs/r1";
    p.start(run);
    let mut seen: Vec<(String, bool)> = Vec::new();
    let mut applies: Vec<Value> = Vec::new();
    let seq = p.walk(
        run,
        &mut |stage, action| {
            assert!(!action.to_string().contains("資料包用上一版的圖"), "更新成功時不該有更新失敗的警告：{action}");
            // 這一段開跑前的圖，查不查得到上一段結束後才加的符號
            seen.push((stage.to_string(), p.knows("brand_new_helper")));
            if stage == "plan" {
                p.put("src/extra.py", "def brand_new_helper(order):\n    return order\n");
                p.awf(&["hook", "mark-dirty"]);
            }
        },
        &mut |a| {
            applies.push(a.clone());
            // 還沒做也還沒略過：再問一次 next，同一個動作還在
            let again = p.next(run, &[]);
            assert_eq!(again["apply"], a["apply"], "還沒處理的併回規格要繼續提醒");
            let skipped = p.next(run, &["--skip", "spec-apply"]);
            assert_eq!(skipped["action"], "done", "略過之後直接到完成：{skipped}");
        },
    );
    assert_eq!(seq, with_apply());
    let flags: Vec<bool> = seen.iter().map(|(_, k)| *k).collect();
    assert_eq!(flags, [false, true, true, true, true, true, true], "plan 段之後加的符號，design 段開跑前就要在圖裡：{seen:?}");
    // 併回規格的動作：沿用 manual 的形狀，附試跑摘要與兩個指令
    let a = &applies[0];
    assert_eq!(applies.len(), 1);
    assert_eq!(a["stage"], "merge");
    assert_eq!(a["apply"], "awf spec apply --change refund --root openspec --write");
    assert_eq!(a["skip"], format!("awf next --run {run} --skip spec-apply"));
    assert!(a["preview"].as_str().is_some_and(|t| t.contains("申請退款")), "試跑摘要要有要併的需求：{a}");
    assert!(p.dir.join("openspec/changes/refund/proposal.md").is_file() && !std::fs::read_to_string(p.dir.join("openspec/specs/orders/spec.md")).unwrap().contains("申請退款"), "試跑不能寫任何檔");
    // 略過記在 run.json，之後不再出現
    assert_eq!(p.run_json(run)["awf"]["spec_apply"]["state"], "skipped");
    assert_eq!(p.next(run, &[])["action"], "done");
    // 連結帳本
    let ledger = std::fs::read_to_string(p.ledger()).expect("連結帳本有寫");
    assert!(ledger.contains("\"req_run\"") && ledger.contains("\"ac_spec\"") && ledger.contains("\"ac_test\""), "帳本要有各段的連結：\n{ledger}");
    // 結案：vault 重寫
    assert!(!p.vault().exists());
    let r = p.close(run);
    assert!(r.all().contains("vault") && p.vault().is_dir(), "結案後要重寫 vault：\n{}", r.all());

    // 把圖刪掉再跑一個 run：照樣走完，不會自動建第一份圖，也沒有多的說明
    std::fs::remove_dir_all(p.dir.join("_tpm/kb")).expect("刪掉圖");
    let run2 = "_tpm/runs/r2";
    p.start(run2);
    let seq2 = p.walk(
        run2,
        &mut |_, action| {
            assert!(!action.to_string().contains("知識圖"), "沒有圖時不該提知識圖：{action}");
            assert!(!p.graph().exists(), "沒有圖時不能自動建第一份");
        },
        &mut |_| {
            p.next(run2, &["--skip", "spec-apply"]);
        },
    );
    assert_eq!(seq2, with_apply(), "併回規格看的是規格層，和有沒有圖無關");
    let r = p.close(run2);
    assert!(!r.all().contains("vault") && !p.vault().exists() && !p.graph().exists(), "沒有圖時結案不碰 vault：\n{}", r.all());
}

/// `knowledge.auto.update` 明確開啟：沒有圖也建；merge 之後真的併回規格（變更歸檔）就算做完，不用另外告知。
#[test]
fn explicit_update_builds_graph_and_apply_is_detected() {
    let p = Proj::new("apply", Some(json!({"update": true})));
    let run = "_tpm/runs/r1";
    p.start(run);
    assert!(!p.graph().exists());
    let mut first = true;
    let seq = p.walk(
        run,
        &mut |stage, _| {
            if first {
                assert!(p.graph().is_file(), "{stage}：update 明確開啟時，第一段開跑前就建圖");
                first = false;
            }
        },
        &mut |a| {
            let cmd: Vec<&str> = a["apply"].as_str().unwrap().split(' ').skip(1).collect();
            let r = p.awf(&cmd);
            assert_eq!(r.code, 0, "{}：\n{}", a["apply"], r.all());
        },
    );
    assert_eq!(seq, with_apply());
    assert!(std::fs::read_to_string(p.dir.join("openspec/specs/orders/spec.md")).unwrap().contains("申請退款"), "apply 之後現行規格有新需求");
    let d = p.run_json(run);
    assert_eq!(d["awf"]["spec_apply"]["state"], "done", "{}", d["awf"]);
    assert_eq!(d["awf"]["spec_apply"]["change"], "refund");
    assert_eq!(p.next(run, &[])["action"], "done", "做完之後不再出現");
    p.close(run);
    assert!(p.vault().is_dir());
}

/// `knowledge.auto` 全關（圖還在）：圖不更新、沒有併回規格的動作、結案不重寫 vault；流程和沒有這些功能時一樣。
#[test]
fn all_off_adds_nothing() {
    let p = Proj::new("off", Some(json!({"update": false, "session": false, "prompt": false, "spec_apply": false, "export": false})));
    assert_eq!(p.awf(&["kg", "build"]).code, 0);
    let before = std::fs::read(p.graph()).expect("圖建好了");
    let run = "_tpm/runs/r1";
    p.start(run);
    let seq = p.walk(
        run,
        &mut |stage, action| {
            assert!(!action.to_string().contains("資料包用上一版的圖"), "{action}");
            if stage == "plan" {
                p.put("src/extra.py", "def brand_new_helper(order):\n    return order\n");
            }
        },
        &mut |a| panic!("spec_apply 關掉了不該有這個動作：{a}"),
    );
    let mut want = base_seq();
    want.push("done".to_string());
    assert_eq!(seq, want);
    assert!(p.run_json(run)["awf"].get("spec_apply").is_none());
    assert_eq!(std::fs::read(p.graph()).unwrap(), before, "update 關掉時圖不能動");
    let r = p.close(run);
    assert!(!r.all().contains("vault") && !p.vault().exists(), "export 關掉時結案不碰 vault：\n{}", r.all());
}

// ------------------------------------------------------------------ 更新失敗／逾時只警告；取消之後也重寫 vault

/// 動作裡「知識圖沒更新成」的警告（`notes` 裡的那一行）。
fn graph_warnings(action: &Value) -> Vec<String> {
    action["notes"].as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|n| n.contains("資料包用上一版的圖")).map(str::to_string).collect()).unwrap_or_default()
}

impl Proj {
    fn abandon(&self, run: &str) -> Out {
        self.awf(&["save-run", "abandon", self.dir.join(run).to_str().unwrap(), "--reason", "需求取消", "--by", "human", "--at", "t"])
    }

    /// 改 profile 的 `knowledge.auto`。
    fn set_auto(&self, auto: Value) {
        let mp = self.dir.join(".claude/teams/methodology.json");
        let mut m = load(&mp);
        if !m["knowledge"].is_object() {
            m["knowledge"] = json!({});
        }
        m["knowledge"]["auto"] = auto;
        put(&mp, &m.to_string());
    }
}

/// 圖更新不了（圖檔的位置被一個目錄佔住，寫不進去）：`awf next` 照樣回這一段的動作，多一行警告；
/// 這一段存得進去，下一段開跑前再試一次、再警告一行。圖檔只是內容壞掉則會重建，不算失敗。
/// 取消時 vault 寫不了只在 stderr 提醒，取消本身照樣成功、結案摘要照寫。
#[test]
fn failed_update_only_warns_and_the_flow_goes_on() {
    let p = Proj::new("broken", Some(json!({"update": true})));
    assert_eq!(p.awf(&["kg", "build"]).code, 0);
    let run = "_tpm/runs/r1";
    p.start(run);

    // 1. 圖檔內容壞掉（不是 SQLite）：更新時整份重建，不警告
    let garbage = b"this is not a sqlite database, just bytes ".repeat(64);
    std::fs::write(p.graph(), &garbage).expect("把圖寫壞");
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("plan")), "{a}");
    assert!(graph_warnings(&a).is_empty(), "讀不懂的圖檔會重建，不算更新失敗：{a}");
    assert_ne!(std::fs::read(p.graph()).expect("圖檔還在"), garbage, "圖重建了");
    assert_eq!(p.awf(&["kg", "locate", "refund"]).code, 0, "重建的圖查得到東西");
    p.done(run, "plan");

    // 2. 圖檔的位置變成目錄：更新失敗 → 閘門不警告；下一段開跑的動作照樣回來，警告正好一行
    std::fs::remove_file(p.graph()).expect("拿掉圖檔");
    std::fs::create_dir_all(p.graph().join("x")).expect("用目錄佔住圖檔的位置");
    let g = p.next(run, &[]);
    assert_eq!(g["action"], "gate", "{g}");
    assert!(graph_warnings(&g).is_empty(), "閘門不是開跑，不更新圖也不警告：{g}");
    assert_eq!(p.awf(&["approve", "--run", run, "--stage", "plan", "--by", "human"]).code, 0);
    let mut d = p.next(run, &[]);
    if d["action"] == "need_input" {
        let keys: Vec<String> = d["need"].as_object().expect("need 是物件").keys().cloned().collect();
        let mut argv: Vec<String> = Vec::new();
        for k in keys {
            argv.push("--set".to_string());
            argv.push(format!("{k}=false"));
        }
        d = p.next(run, &argv.iter().map(String::as_str).collect::<Vec<_>>());
    }
    assert_eq!((d["action"].as_str(), d["stage"].as_str()), (Some("run_workflow"), Some("design")), "更新失敗時動作照樣回來：{d}");
    let w = graph_warnings(&d);
    assert_eq!(w.len(), 1, "警告正好一行：{d}");
    assert!(w[0].starts_with("警告：") && w[0].contains("知識圖") && !w[0].contains('\n'), "{w:?}");
    assert!(p.dir.join(d["script"].as_str().unwrap_or("?")).is_file(), "腳本照樣產生：{d}");
    assert!(p.dir.join(run).join("design/pack.md").is_file(), "資料包照樣產生");
    assert!(p.graph().join("x").is_dir(), "更新失敗不能動原本在那裡的東西");
    p.done(run, "design");
    // 同一段再問一次 next：再試一次、還是只有一行
    assert_eq!(p.next(run, &[])["action"], "gate");

    // 3. 圖修好了，但 vault 的位置被一個檔案佔住：取消照樣成功，只提醒 vault 沒重寫
    std::fs::remove_dir_all(p.graph()).expect("拿掉佔位的目錄");
    assert_eq!(p.awf(&["kg", "build"]).code, 0);
    p.put("_tpm/kb/vault", "不是目錄\n");
    let r = p.abandon(run);
    assert_eq!(r.code, 0, "vault 寫不了不能讓取消失敗：\n{}", r.all());
    assert!(p.run_json(run)["abandoned"].is_object());
    assert!(p.dir.join(run).join("archive/結案摘要.md").is_file(), "結案摘要照寫");
    assert!(r.stderr.contains("vault 沒有重寫") && r.stderr.contains("awf kg export"), "要說明沒重寫與怎麼補：\n{}", r.all());
    assert_eq!(std::fs::read_to_string(p.vault()).unwrap(), "不是目錄\n", "佔位的檔不能被改掉");
}

/// 更新逾時（`knowledge.auto.timeout` 設最小的 1 秒、專案大到 1 秒掃不完）：動作照樣回來、警告一行、上一版的圖原封不動。
/// 專案大小看這台機器調：先量一次完整建圖，不到 3 秒就加倍，確定 1 秒一定不夠才測。
#[test]
fn update_timeout_only_warns_and_keeps_the_previous_graph() {
    let p = Proj::new("timeout", None);
    let write_all = |n: usize, tail: &str| {
        for i in 0..n {
            let mut text = String::new();
            for j in 0..150 {
                text.push_str(&format!("def fn_{i}_{j}(order):\n    return helper_{j}(order)\n\n"));
            }
            text.push_str(tail);
            p.put(&format!("src/bulk/m{i}.py"), &text);
        }
    };
    let mut n = 200;
    let mut slow_enough = false;
    for _ in 0..5 {
        write_all(n, "");
        let t = std::time::Instant::now();
        let r = p.awf(&["kg", "build", "--timeout", "600"]);
        assert_eq!(r.code, 0, "awf kg build：\n{}", r.all());
        if t.elapsed() >= std::time::Duration::from_secs(3) {
            slow_enough = true;
            break;
        }
        n *= 2;
    }
    if !slow_enough {
        eprintln!("略過：這台機器建 {n} 個檔的圖還不到 3 秒，做不出穩定的逾時");
        return;
    }
    let before = std::fs::read(p.graph()).expect("上一版的圖");
    // 每個檔都改過：這次更新要全部重算，1 秒做不完
    write_all(n, "def brand_new_tail(order):\n    return order\n");
    p.set_auto(json!({"timeout": 1}));
    let run = "_tpm/runs/r1";
    p.start(run);
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("plan")), "逾時時動作照樣回來：{a}");
    let w = graph_warnings(&a);
    assert_eq!(w.len(), 1, "警告正好一行：{a}");
    assert!(w[0].starts_with("警告：") && w[0].contains("逾時") && !w[0].contains('\n'), "{w:?}");
    assert_eq!(std::fs::read(p.graph()).expect("圖檔還在"), before, "逾時放棄這次，上一版的圖原封不動");
    let left: Vec<String> = std::fs::read_dir(p.dir.join("_tpm/kb")).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| f.contains(".tmp-")).collect()).unwrap_or_default();
    assert!(left.is_empty(), "沒有留下暫存檔：{left:?}");
    assert!(p.dir.join(run).join("plan/pack.md").is_file(), "資料包照樣產生（用上一版的圖）");
    p.done(run, "plan");
    assert_eq!(p.next(run, &[])["action"], "gate", "這一段照常存、照常到閘門");
}

/// 取消（`awf save-run abandon`）之後和結案一樣重寫 vault：有圖才做、`notes/` 不動；
/// `knowledge.auto.export` 關掉或沒有圖時不碰。
#[test]
fn abandon_rewrites_the_vault() {
    let p = Proj::new("abandon", None);
    assert_eq!(p.awf(&["kg", "build"]).code, 0);
    p.put("_tpm/kb/vault/notes/mine.md", "我自己的筆記\n");
    let run = "_tpm/runs/r1";
    p.start(run);
    assert_eq!(p.next(run, &[])["action"], "run_workflow");
    p.done(run, "plan");
    let pages = |p: &Proj| -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![p.vault()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path.strip_prefix(p.vault()).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out.sort();
        out
    };
    assert_eq!(pages(&p), ["notes/mine.md"], "取消之前 vault 只有使用者的筆記");
    let r = p.abandon(run);
    assert_eq!(r.code, 0, "awf save-run abandon：\n{}", r.all());
    assert!(r.stdout.contains("已取消") && r.stdout.contains("vault 已重寫"), "取消之後要重寫 vault：\n{}", r.all());
    let after = pages(&p);
    assert!(after.len() > 1 && after.iter().any(|f| f.ends_with(".md") && !f.starts_with("notes/")), "vault 有寫出頁面：{after:?}");
    assert_eq!(std::fs::read_to_string(p.vault().join("notes/mine.md")).unwrap(), "我自己的筆記\n", "notes/ 不動");
    assert!(p.dir.join(run).join("archive/結案摘要.md").is_file());
    // 已經取消過：保留原紀錄，不再重寫
    let again = p.abandon(run);
    assert!(again.code == 0 && again.stdout.contains("已經取消過") && !again.all().contains("vault"), "{}", again.all());

    // export 關掉：取消不碰 vault
    let q = Proj::new("abandon-off", Some(json!({"export": false})));
    assert_eq!(q.awf(&["kg", "build"]).code, 0);
    q.start(run);
    let r = q.abandon(run);
    assert!(r.code == 0 && !r.all().contains("vault") && !q.vault().exists(), "export 關掉時取消不碰 vault：\n{}", r.all());

    // 沒有圖：取消不碰 vault，也不會建圖
    let z = Proj::new("abandon-nograph", None);
    z.start(run);
    let r = z.abandon(run);
    assert!(r.code == 0 && !r.all().contains("vault") && !z.vault().exists() && !z.graph().exists(), "沒有圖時取消不碰 vault：\n{}", r.all());
}

// ------------------------------------------------------------------ E1：中斷回收

/// 假的 agent transcript：派工單（提到 run 目錄）、（有給的話）寫紀錄檔的 Write、最後交的信封。
fn transcript_in(p: &Proj, sess: &str, run: &str, wf: &str, agent: &str, label: &str, envelope: &Value, wrote: Option<&str>) -> PathBuf {
    let dir = p.tmp.join("transcripts").join(sess).join("subagents/workflows").join(wf);
    let run_abs = p.dir.join(run);
    let mut lines = vec![json!({"type": "user", "message": {"role": "user", "content": format!("任務：{label}。run_dir＝{}", run_abs.display())}})];
    if let Some(text) = wrote {
        let angle = label.split_once(':').map(|x| x.1).unwrap_or(label);
        let file = run_abs.join(format!("plan/research-{angle}.md"));
        lines.push(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "Write", "input": {"file_path": file.to_str().unwrap(), "content": text}}]}}));
    }
    lines.push(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "StructuredOutput", "input": envelope}]}}));
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let file = dir.join(format!("agent-{agent}.jsonl"));
    put(&file, &body);
    put(&dir.join(format!("agent-{agent}.meta.json")), &json!({"agentType": "agent-workflow:tpm-researcher", "description": label}).to_string());
    file
}

fn transcript(p: &Proj, run: &str, wf: &str, agent: &str, label: &str, envelope: &Value) {
    transcript_in(p, "sess-1", run, wf, agent, label, envelope, None);
}

/// 調研紀錄的內文（紀錄檔與 transcript 裡的 Write 用同一份）。
fn record_text(angle: &str) -> String {
    format!("# 調研：{angle}\n\n退款邏輯在 src/refund.py:1。\n")
}

/// 調研 agent 的 transcript，帶「寫了紀錄檔」的 Write。
fn research(p: &Proj, sess: &str, run: &str, wf: &str, agent: &str, angle: &str, envelope: &Value) -> PathBuf {
    transcript_in(p, sess, run, wf, agent, &format!("research:{angle}"), envelope, Some(&record_text(angle)))
}

/// 這個 session 存在（`awf next` 會把它記進 pending）。要在 next 之前呼叫。
fn session(p: &Proj, sess: &str) {
    put(&p.tmp.join("transcripts").join(format!("{sess}.jsonl")), "{}\n");
}

/// 還在跑（沒有回傳）的 workflow 紀錄。
fn running_wf(p: &Proj, sess: &str, wf: &str, script: &Path) {
    put(&p.tmp.join("transcripts").join(sess).join(format!("workflows/{wf}.json")), &json!({"runId": wf, "scriptPath": script.to_str().unwrap(), "status": "running"}).to_string());
}

fn findings(angle: &str) -> Value {
    let mut e = envl(json!({"lens": angle, "findings": [{"id": "f1", "severity": "info", "claim": "退款邏輯在 refund()", "location": "src/refund.py:1", "recommendation": "沿用", "confidence": 0.9}]}));
    e["evidence"] = json!([{"claim": "c", "source": "src/refund.py:1", "result": "ok"}]);
    e
}

fn locations(angle: &str, path: &str) -> Value {
    let mut e = envl(json!({"angle": angle, "locations": [{"path": path, "start": 1, "end": 2, "why": "退款的進入點"}], "queries": ["Grep refund"], "gaps": []}));
    e["evidence"] = json!([{"claim": "c", "source": format!("{path}:1"), "result": "ok"}]);
    e
}

fn reuse_angles(p: &Proj, run: &str) -> Vec<String> {
    let f = p.dir.join(run).join("plan/reuse.json");
    if !f.is_file() {
        return Vec::new();
    }
    let v = load(&f);
    let m = v.get("angles").and_then(Value::as_object).or_else(|| v.as_object());
    let names = ["good", "nolocate", "norecord", "badschema", "stale", "notdone", "tampered", "nowrite", "edited", "dup", "future"];
    let mut out: Vec<String> = m.map(|m| m.keys().filter(|k| names.contains(&k.as_str())).cloned().collect()).unwrap_or_default();
    out.sort();
    out
}

fn set_future(f: &Path) {
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(f).and_then(|f| f.set_modified(later)).expect("改修改時間");
}

/// plan 段被中斷（workflow 沒有回傳）：下一次 next 只收「紀錄檔在、信封過 schema、來源沒變」的調研角度，寫進 reuse.json 並在動作裡列出來。
#[test]
fn interrupted_plan_recovers_only_verifiable_research() {
    let p = Proj::new("recover", Some(json!({"update": false})));
    let run = "_tpm/runs/r1";
    p.put("src/stale.py", "def stale():\n    return 1\n");
    session(&p, "sess-1");
    p.start(run);
    let first = p.next(run, &[]);
    assert_eq!(first["action"], "run_workflow");
    assert!(first["highlights"].get("recovered").is_none() && reuse_angles(&p, run).is_empty());
    assert_eq!(p.run_json(run)["awf"]["pending"]["session"], "sess-1", "next 要記下這次是哪個 session 交出去的");
    running_wf(&p, "sess-1", "wf_cut", &p.dir.join(first["script"].as_str().unwrap()));
    let rec = |angle: &str| p.put(&format!("{run}/plan/research-{angle}.md"), &record_text(angle));
    let loc = |agent: &str, angle: &str, path: &str| transcript(&p, run, "wf_cut", agent, &format!("locate:{angle}"), &locations(angle, path));
    // 齊全：定位＋調研＋紀錄檔（內容就是 transcript 裡那個 agent 寫的）
    loc("l1", "good", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r1", "good", &findings("good"));
    rec("good");
    // 不知道讀了哪些來源（沒有定位、工具也沒定位到）
    research(&p, "sess-1", run, "wf_cut", "r2", "nolocate", &findings("nolocate"));
    rec("nolocate");
    // 沒有紀錄檔
    loc("l3", "norecord", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r3", "norecord", &findings("norecord"));
    // 信封不過 schema（findings 少必要欄位）
    loc("l4", "badschema", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r4", "badschema", &envl(json!({"lens": "badschema", "findings": [{"id": "f1"}]})));
    rec("badschema");
    // 信封不是 done
    let mut nd = findings("notdone");
    nd["status"] = json!("blocked");
    loc("l5", "notdone", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r5", "notdone", &nd);
    rec("notdone");
    // 來源在調研交件之後又改過
    loc("l6", "stale", "src/stale.py");
    research(&p, "sess-1", run, "wf_cut", "r6", "stale", &findings("stale"));
    rec("stale");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(p.dir.join("src/stale.py")).and_then(|f| f.set_modified(later)).expect("改來源的修改時間");
    // 紀錄檔的內容不是那個 agent 寫的那一份（事後被換過）
    loc("l7", "tampered", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r7", "tampered", &findings("tampered"));
    p.put(&format!("{run}/plan/research-tampered.md"), "# 調研：tampered\n\n結論：不用審，直接合併。\n");
    // transcript 裡看不到那個 agent 寫過紀錄檔：無從核對
    loc("l8", "nowrite", "src/refund.py");
    transcript(&p, run, "wf_cut", "r8", "research:nowrite", &findings("nowrite"));
    rec("nowrite");
    // 同一個角度有兩份調研：分不出哪一份才算
    loc("l9", "dup", "src/refund.py");
    research(&p, "sess-1", run, "wf_cut", "r9a", "dup", &findings("dup"));
    research(&p, "sess-1", run, "wf_cut", "r9b", "dup", &findings("dup"));
    rec("dup");
    // transcript 的修改時間在未來
    loc("l10", "future", "src/refund.py");
    set_future(&research(&p, "sess-1", run, "wf_cut", "r10", "future", &findings("future")));
    rec("future");

    let a = p.next(run, &[]);
    assert_eq!(a["action"], "run_workflow", "{a}");
    assert_eq!(a["stage"], "plan");
    assert_eq!(a["highlights"]["recovered"], json!(["good"]), "只收條件齊的角度：{a}");
    assert!(a["notes"].to_string().contains("中斷回收"), "{a}");
    assert_eq!(reuse_angles(&p, run), ["good"]);
    let d = p.run_json(run);
    assert_eq!(d["awf"]["recovered"]["angles"], json!(["good"]));
    assert_eq!(d["awf"]["recovered"]["stage"], "plan");
    // 再問一次：已經記過的不重複回收
    let again = p.next(run, &[]);
    assert!(again["highlights"].get("recovered").is_none() || again["highlights"]["recovered"] == json!([]), "{again}");
    assert_eq!(reuse_angles(&p, run), ["good"]);
}

/// 預放的假結果不被回收（1.0 第二次審查第 6 則）：transcript 與紀錄檔本身都「齊全」，
/// 但對不上這次 next 交出去的 workflow——沒有 workflow 紀錄（只在派工單提到 run 目錄）、在別的 session、
/// 是別支腳本、next 之前就放好、或同一支腳本有兩個 workflow——一律不收。
#[test]
fn planted_results_are_not_recovered() {
    type Plant = fn(&Proj, &str, &Path);
    let cases: [(&str, Plant); 5] = [
        ("沒有 workflow 紀錄", |p, run, _script| {
            transcript_in(p, "sess-1", run, "wf_fake", "l1", "locate:good", &locations("good", "src/refund.py"), None);
            research(p, "sess-1", run, "wf_fake", "r1", "good", &findings("good"));
        }),
        ("別的 session", |p, run, script| {
            running_wf(p, "sess-9", "wf_fake", script);
            transcript_in(p, "sess-9", run, "wf_fake", "l1", "locate:good", &locations("good", "src/refund.py"), None);
            research(p, "sess-9", run, "wf_fake", "r1", "good", &findings("good"));
        }),
        ("別支腳本", |p, run, script| {
            running_wf(p, "sess-1", "wf_fake", &script.with_file_name("other.js"));
            transcript_in(p, "sess-1", run, "wf_fake", "l1", "locate:good", &locations("good", "src/refund.py"), None);
            research(p, "sess-1", run, "wf_fake", "r1", "good", &findings("good"));
        }),
        ("同一支腳本有兩個 workflow", |p, run, script| {
            for wf in ["wf_a", "wf_b"] {
                running_wf(p, "sess-1", wf, script);
                transcript_in(p, "sess-1", run, wf, "l1", "locate:good", &locations("good", "src/refund.py"), None);
            }
            research(p, "sess-1", run, "wf_a", "r1", "good", &findings("good"));
        }),
        ("workflow 紀錄的修改時間在未來", |p, run, script| {
            running_wf(p, "sess-1", "wf_fake", script);
            set_future(&p.tmp.join("transcripts/sess-1/workflows/wf_fake.json"));
            transcript_in(p, "sess-1", run, "wf_fake", "l1", "locate:good", &locations("good", "src/refund.py"), None);
            research(p, "sess-1", run, "wf_fake", "r1", "good", &findings("good"));
        }),
    ];
    for (name, plant) in cases {
        let p = Proj::new("planted", Some(json!({"update": false})));
        let run = "_tpm/runs/r1";
        session(&p, "sess-1");
        p.start(run);
        let first = p.next(run, &[]);
        assert_eq!(first["action"], "run_workflow", "{name}");
        plant(&p, run, &p.dir.join(first["script"].as_str().unwrap()));
        p.put(&format!("{run}/plan/research-good.md"), &record_text("good"));
        let a = p.next(run, &[]);
        assert!(a["highlights"].get("recovered").is_none(), "{name}：不能回收：{a}");
        assert!(reuse_angles(&p, run).is_empty() && p.run_json(run)["awf"].get("recovered").is_none(), "{name}");
    }
    // 不知道是哪個 session 交出去的（next 當時認不出 session）：不收
    let p = Proj::new("planted-nosess", Some(json!({"update": false})));
    let run = "_tpm/runs/r1";
    p.start(run);
    let first = p.next(run, &[]);
    assert!(p.run_json(run)["awf"]["pending"].get("session").is_none(), "認不出 session 時不寫這個鍵");
    running_wf(&p, "sess-1", "wf_fake", &p.dir.join(first["script"].as_str().unwrap()));
    transcript_in(&p, "sess-1", run, "wf_fake", "l1", "locate:good", &locations("good", "src/refund.py"), None);
    research(&p, "sess-1", run, "wf_fake", "r1", "good", &findings("good"));
    p.put(&format!("{run}/plan/research-good.md"), &record_text("good"));
    let a = p.next(run, &[]);
    assert!(a["highlights"].get("recovered").is_none() && reuse_angles(&p, run).is_empty(), "{a}");
}

/// 有回傳的 workflow（跑完了只是還沒 done）不是「被中斷」：不回收。
#[test]
fn finished_workflow_is_not_recovered() {
    let p = Proj::new("finished", Some(json!({"update": false})));
    let run = "_tpm/runs/r1";
    session(&p, "sess-1");
    p.start(run);
    let first = p.next(run, &[]);
    transcript(&p, run, "wf_ok", "l1", "locate:good", &locations("good", "src/refund.py"));
    research(&p, "sess-1", run, "wf_ok", "r1", "good", &findings("good"));
    p.put(&format!("{run}/plan/research-good.md"), &record_text("good"));
    let script = p.dir.join(first["script"].as_str().unwrap());
    put(&p.tmp.join("transcripts/sess-1/workflows/wf_ok.json"), &json!({"scriptPath": script.to_str().unwrap(), "status": "completed", "result": p.result("plan")}).to_string());
    let a = p.next(run, &[]);
    assert!(a["highlights"].get("recovered").is_none(), "{a}");
    assert!(reuse_angles(&p, run).is_empty() && p.run_json(run)["awf"].get("recovered").is_none());
}
