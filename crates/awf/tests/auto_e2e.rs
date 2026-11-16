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
            assert!(!action.to_string().contains("知識圖"), "更新成功時不該多說明：{action}");
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
            assert!(!action.to_string().contains("知識圖"), "{action}");
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

// ------------------------------------------------------------------ E1：中斷回收

/// 假的 agent transcript：派工單（提到 run 目錄）與最後交的信封。
fn transcript(p: &Proj, run: &str, wf: &str, agent: &str, label: &str, envelope: &Value) {
    let dir = p.tmp.join("transcripts/sess-1/subagents/workflows").join(wf);
    let run_abs = p.dir.join(run);
    let lines = [
        json!({"type": "user", "message": {"role": "user", "content": format!("任務：{label}。run_dir＝{}", run_abs.display())}}),
        json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "name": "StructuredOutput", "input": envelope}]}}),
    ];
    put(&dir.join(format!("agent-{agent}.jsonl")), &format!("{}\n{}\n", lines[0], lines[1]));
    put(&dir.join(format!("agent-{agent}.meta.json")), &json!({"agentType": "agent-workflow:tpm-researcher", "description": label}).to_string());
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
    let mut out: Vec<String> = m.map(|m| m.keys().filter(|k| ["good", "nolocate", "norecord", "badschema", "stale", "notdone"].contains(&k.as_str())).cloned().collect()).unwrap_or_default();
    out.sort();
    out
}

/// plan 段被中斷（workflow 沒有回傳）：下一次 next 只收「紀錄檔在、信封過 schema、來源沒變」的調研角度，寫進 reuse.json 並在動作裡列出來。
#[test]
fn interrupted_plan_recovers_only_verifiable_research() {
    let p = Proj::new("recover", Some(json!({"update": false})));
    let run = "_tpm/runs/r1";
    p.put("src/stale.py", "def stale():\n    return 1\n");
    p.start(run);
    let first = p.next(run, &[]);
    assert_eq!(first["action"], "run_workflow");
    assert!(first["highlights"].get("recovered").is_none() && reuse_angles(&p, run).is_empty());
    let rec = |angle: &str| p.put(&format!("{run}/plan/research-{angle}.md"), &format!("# 調研：{angle}\n\n退款邏輯在 src/refund.py:1。\n"));
    // 齊全：定位＋調研＋紀錄檔
    transcript(&p, run, "wf_cut", "l1", "locate:good", &locations("good", "src/refund.py"));
    transcript(&p, run, "wf_cut", "r1", "research:good", &findings("good"));
    rec("good");
    // 不知道讀了哪些來源（沒有定位、工具也沒定位到）
    transcript(&p, run, "wf_cut", "r2", "research:nolocate", &findings("nolocate"));
    rec("nolocate");
    // 沒有紀錄檔
    transcript(&p, run, "wf_cut", "l3", "locate:norecord", &locations("norecord", "src/refund.py"));
    transcript(&p, run, "wf_cut", "r3", "research:norecord", &findings("norecord"));
    // 信封不過 schema（findings 少必要欄位）
    transcript(&p, run, "wf_cut", "l4", "locate:badschema", &locations("badschema", "src/refund.py"));
    transcript(&p, run, "wf_cut", "r4", "research:badschema", &envl(json!({"lens": "badschema", "findings": [{"id": "f1"}]})));
    rec("badschema");
    // 信封不是 done
    let mut nd = findings("notdone");
    nd["status"] = json!("blocked");
    transcript(&p, run, "wf_cut", "l5", "locate:notdone", &locations("notdone", "src/refund.py"));
    transcript(&p, run, "wf_cut", "r5", "research:notdone", &nd);
    rec("notdone");
    // 來源在調研交件之後又改過
    transcript(&p, run, "wf_cut", "l6", "locate:stale", &locations("stale", "src/stale.py"));
    transcript(&p, run, "wf_cut", "r6", "research:stale", &findings("stale"));
    rec("stale");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    std::fs::File::options().write(true).open(p.dir.join("src/stale.py")).and_then(|f| f.set_modified(later)).expect("改來源的修改時間");

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

/// 有回傳的 workflow（跑完了只是還沒 done）不是「被中斷」：不回收。
#[test]
fn finished_workflow_is_not_recovered() {
    let p = Proj::new("finished", Some(json!({"update": false})));
    let run = "_tpm/runs/r1";
    p.start(run);
    let first = p.next(run, &[]);
    transcript(&p, run, "wf_ok", "l1", "locate:good", &locations("good", "src/refund.py"));
    transcript(&p, run, "wf_ok", "r1", "research:good", &findings("good"));
    p.put(&format!("{run}/plan/research-good.md"), "# 調研：good\n\n退款邏輯在 src/refund.py:1。\n");
    let script = p.dir.join(first["script"].as_str().unwrap());
    put(&p.tmp.join("transcripts/sess-1/workflows/wf_ok.json"), &json!({"scriptPath": script.to_str().unwrap(), "status": "completed", "result": p.result("plan")}).to_string());
    let a = p.next(run, &[]);
    assert!(a["highlights"].get("recovered").is_none(), "{a}");
    assert!(reuse_angles(&p, run).is_empty() && p.run_json(run)["awf"].get("recovered").is_none());
}
