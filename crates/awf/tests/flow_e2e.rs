//! 自訂段（profile 的 `flow`）的端到端測試（不花 token）：`awf init` 套出暫存專案，profile 加一個自訂段 `uat`
//! （接在 test 後面、由 bugfix 點名），每一段給假的段結果，只用 `awf start`／`next`／`done`／`approve` 走：
//!
//! - 一個自訂段從開工走到結案：順序、腳本、閘門、產出缺件、status、不能政策核准；
//! - 該拒絕或交人的：宣告不合規則（`after: "merge"`、`gate` 不是 human、id 撞內建段、腳本在 `.claude/flows/` 以外）、
//!   自訂段沒核准就想進下一段／推送／過 merge gate、鎖定的 recipe 點名的自訂段被從 profile 拿掉；
//! - 沒有點名就不跑。
//!
//! 範本與 schema 取自這個 repo 的 `plugin/`（`AWF_PLUGIN_DIR`）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// 合格的自訂段腳本：派工走執行核心、每個 return 都帶 trace。
const UAT_JS: &str = "export const meta = { name: 'uat', description: '示範客戶 ACME 的驗收' }
const FINDINGS = /*@schema:findings*/ null /*@end*/
const r = await awfAgent('照驗收清單逐條確認，結果寫進驗收報告', { label: 'uat:check', agentType: NS + 'tpm-reviewer', schema: FINDINGS })
if (!r) return { stage: 'uat', halted: true, halt_reason: '缺件：uat:check', recipe: RECIPE, trace: awfTrace() }
return { stage: 'uat', halted: false, review: r, recipe: RECIPE, trace: awfTrace() }
";

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

fn uat() -> Value {
    json!({"id": "uat", "after": "test", "run": "script", "script": ".claude/flows/uat.js", "gate": "human", "approver": "示範客戶 ACME 的 PM", "artifacts": ["驗收報告.md"]})
}

fn flow_of(stage: Value) -> Value {
    json!({"stages": [stage], "labels": {"deliver": "交付審查", "uat": "客戶驗收"}})
}

impl Proj {
    /// `flow`＝profile 的 `flow`（None＝不寫）；`named`＝bugfix 的 stages 要不要點名 uat（接在 test 後面）。
    fn new(tag: &str, flow: Option<Value>, named: bool) -> Proj {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("awf-flow-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).expect("建暫存專案");
        let tmp = tmp.canonicalize().expect("解開暫存目錄");
        let p = Proj { dir: tmp.join("proj"), tmp };
        let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", root().join("plugin/workflow.config.example").to_str().unwrap()]);
        assert_eq!(r.code, 0, "awf init：\n{}", r.all());
        p.put(".claude/flows/uat.js", UAT_JS);
        p.edit_profile(|m| {
            // 模型分級不是這裡要測的：假的 trace 不帶 model／tier
            m["models"] = json!({"mode": "inherit"});
            if let Some(f) = flow.clone() {
                m["flow"] = f;
            }
            if named {
                let st = m["recipes"]["bugfix"]["stages"].as_array_mut().expect("bugfix 有 stages");
                let at = st.iter().position(|x| x == "test").expect("bugfix 有 test") + 1;
                st.insert(at, json!("uat"));
            }
        });
        p
    }

    fn profile(&self) -> PathBuf {
        self.dir.join(".claude/teams/methodology.json")
    }

    fn edit_profile(&self, f: impl Fn(&mut Value)) {
        let mut m = load(&self.profile());
        f(&mut m);
        put(&self.profile(), &serde_json::to_string_pretty(&m).unwrap());
    }

    fn put(&self, rel: &str, text: &str) {
        put(&self.dir.join(rel), text);
    }

    fn awf(&self, args: &[&str]) -> Out {
        let mut c = Command::new(EXE);
        c.args(args).current_dir(&self.dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
                c.env_remove(k);
            }
        }
        c.env("CLAUDE_PROJECT_DIR", &self.dir).env("AWF_PLUGIN_DIR", root().join("plugin")).env("AWF_TRANSCRIPTS_DIR", self.tmp.join("transcripts"));
        let o = c.output().expect("啟動 awf");
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
    }

    fn run_json(&self, run: &str) -> Value {
        load(&self.dir.join(run).join("run.json"))
    }

    fn start(&self, run: &str) {
        let r = self.awf(&["start", "--run", run, "--event", "sdlc", "--repo", ".", "--epic", "export", "--requirement", "匯出按鈕下載 CSV"]);
        assert_eq!(r.code, 0, "awf start：\n{}", r.all());
    }

    fn next_raw(&self, run: &str, extra: &[&str]) -> Out {
        let mut a = vec!["next", "--run", run];
        a.extend_from_slice(extra);
        self.awf(&a)
    }

    fn next(&self, run: &str, extra: &[&str]) -> Value {
        let r = self.next_raw(run, extra);
        assert_eq!(r.code, 0, "awf next {extra:?}：\n{}", r.all());
        r.json()
    }

    /// 某一段的假結果。plan 以外的段帶 run.json 鎖定的 recipe 與 trace（紀錄檔真的寫出來）。
    fn result(&self, run: &str, stage: &str) -> Value {
        if stage == "plan" {
            self.put("specs/export/intent.md", "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n## 現象\n匯出空白\n## 重現步驟\n1. 點匯出\n## 預期行為\n有資料\n## 根因\n空集合沒處理\n");
            return json!({"stage": "plan", "epic_dir": self.dir.to_str().unwrap(), "recipe": {"name": "bugfix", "modifiers": []},
                "subtasks": [{"id": "t1", "title": "t1", "kind": "code", "owns": ["src/export.py"], "depends_on": [], "acceptance": ["AC-1 下載 CSV"]}],
                "intent": envl(json!({"documents": [{"path": "specs/export/intent.md", "kind": "intent", "audience": "owner"}]})), "trace": []});
        }
        let rec = self.run_json(run)["recipe"].clone();
        let res = rec["resolved"].clone();
        let mut labels: Vec<(String, &str)> = Vec::new();
        match stage {
            "test" => labels.push(("verify".to_string(), "dev-verifier")),
            "deliver" => {
                for axis in res["deliver_axes"].as_array().cloned().unwrap_or_default() {
                    labels.push((format!("review:{}", axis.as_str().unwrap_or_default()), "tpm-reviewer"));
                }
            }
            "uat" => labels.push(("uat:check".to_string(), "tpm-reviewer")),
            _ => labels.push((format!("{stage}:x"), "dev-implementer")),
        }
        let trace: Vec<Value> = labels
            .iter()
            .map(|(label, role)| {
                let rel = format!("{run}/{stage}/{}.md", label.replace(':', "-"));
                self.put(&rel, "# 紀錄\n\n## 產出\n- 做完了\n");
                json!({"label": label, "agent_type": format!("agent-workflow:{role}"), "phase": "Run", "record": self.dir.join(&rel).to_str().unwrap(), "ok": true})
            })
            .collect();
        let mut out = json!({"stage": stage, "halted": false, "recipe": {"name": rec["name"], "modifiers": rec["modifiers"], "stages": res["stages"]}, "trace": trace});
        match stage {
            "test" => {
                out["verifier"] = envl(json!({"commit": SHA, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
                    "criteria": [{"id": "AC-1", "criterion": "c", "command": "pytest", "result": "pass", "evidence": "e"}]}));
            }
            "merge" => {
                out["x"] = envl(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": [], "tests": []}));
                out["merged"] = json!(true);
                out["merge_state"] = json!("merged");
                out["head_sha"] = json!(SHA);
            }
            "uat" | "deliver" => out["review"] = envl(json!({"lens": "spec", "findings": []})),
            _ => out["x"] = envl(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/export.py"], "tests": []})),
        }
        out
    }

    fn done_raw(&self, run: &str, stage: &str) -> Out {
        let f = self.tmp.join(format!("{stage}.json"));
        put(&f, &self.result(run, stage).to_string());
        self.awf(&["done", "--run", run, "--stage", stage, "--output", f.to_str().unwrap(), "--no-next"])
    }

    fn done(&self, run: &str, stage: &str) {
        let r = self.done_raw(run, stage);
        assert_eq!(r.code, 0, "awf done（{stage}）：\n{}", r.all());
    }

    fn approve(&self, run: &str, stage: &str, by: &str) -> Out {
        self.awf(&["approve", "--run", run, "--stage", stage, "--by", by])
    }

    /// 走到 `stop`（`動作:段`）這個動作出現為止（不處理它）；回傳走過的動作序列與停下來的那個動作。
    fn walk_to(&self, run: &str, stop: &str) -> (Vec<String>, Value) {
        let mut seq = Vec::new();
        let mut last = Value::Null;
        for _ in 0..40 {
            let a = self.next(run, &[]);
            last = a.clone();
            let (action, stage) = (a["action"].as_str().unwrap_or_default().to_string(), a["stage"].as_str().unwrap_or_default().to_string());
            let key = if stage.is_empty() || action == "done" { action.clone() } else { format!("{action}:{stage}") };
            if key == stop {
                return (seq, a);
            }
            match action.as_str() {
                "run_workflow" => {
                    seq.push(key);
                    assert!(self.dir.join(a["script"].as_str().unwrap_or("?")).is_file(), "{stage} 段的腳本沒有產生：{a}");
                    if stage == "uat" {
                        self.put(&format!("{run}/uat/驗收報告.md"), "# 驗收報告\n\n- 通過\n");
                    }
                    self.done(run, &stage);
                }
                "gate" => {
                    seq.push(key);
                    let r = self.approve(run, &stage, "human");
                    assert_eq!(r.code, 0, "awf approve（{stage}）：\n{}", r.all());
                }
                "need_input" => {
                    let mut argv: Vec<String> = Vec::new();
                    for k in a["need"].as_object().expect("need 是物件").keys() {
                        argv.push("--set".to_string());
                        argv.push(format!("{k}=false"));
                    }
                    self.next(run, &argv.iter().map(String::as_str).collect::<Vec<_>>());
                }
                "manual" => {
                    seq.push(key);
                    self.next(run, &["--set", "pr=7"]);
                }
                other => panic!("沒預期的動作 {other}（要走到 {stop}）：{a}\n已走：{seq:?}"),
            }
        }
        panic!("走了 40 步還沒到 {stop}：{seq:?}\n最後的動作：{last}");
    }
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// 一個自訂段從開工走到結案。
#[test]
fn custom_stage_from_start_to_close() {
    let p = Proj::new("full", Some(flow_of(uat())), true);
    let d = p.doctor();
    assert!(d.contains("自訂段：uat（接在 test 後面）"), "awf doctor 要列出自訂段：\n{d}");
    let run = "_tpm/runs/r1";
    p.start(run);

    // 走到自訂段要跑的那一步：前面的段照 recipe 的順序
    let (seq, act) = p.walk_to(run, "run_workflow:uat");
    assert_eq!(seq, ["run_workflow:plan", "gate:plan", "run_workflow:build", "run_workflow:sync", "run_workflow:test", "gate:test"], "自訂段排在 test 之後");
    let d = p.run_json(run);
    assert_eq!(strs(&d["recipe"]["resolved"]["stages"]), ["plan", "build", "sync", "test", "uat", "deliver", "merge"], "recipe 點名的自訂段排在 after 指的段後面");
    let keys: Vec<String> = d["stages"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["plan", "design", "build", "sync", "test", "uat", "deliver", "merge"], "run.json 的段順序");
    assert_eq!(d["stages"]["uat"], json!({"status": "pending", "gate": true, "flow": true}));

    // 動作：用 profile 指定的腳本（不需要 --script），組裝後接上執行核心；派工資訊帶產出清單
    assert_eq!(act["workflow"], "flow-uat");
    assert_eq!(act["script"], format!("{run}/.wf/flow-uat.js"));
    assert_eq!(act["custom"]["source"], ".claude/flows/uat.js");
    assert_eq!(act["custom"]["flow"], true);
    let body = std::fs::read_to_string(p.dir.join(run).join(".wf/flow-uat.js")).unwrap();
    assert!(body.contains("function awfAgent(") && body.contains("stage: \"uat\"") && body.contains("\"artifacts\":[\"驗收報告.md\"]"), "腳本要接上執行核心、帶這一段的參數");
    assert!(act["notes"].to_string().contains("自訂段") && act["notes"].to_string().contains("示範客戶 ACME 的 PM"), "{act}");
    // 自訂段的腳本由 profile 指定，不能用 --script 換；內建段可換腳本的名單也沒有變大
    let r = p.next_raw(run, &["--script", ".claude/flows/uat.js", "--stage", "uat"]);
    assert!(r.code != 0 && r.all().contains("不能用 --script 換"), "{}", r.all());

    // 做完但宣告的產出還沒寫：列成缺件；停在閘門
    let r = p.done_raw(run, "uat");
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.all().contains("自訂段的產出缺檔：uat/驗收報告.md"), "{}", r.all());
    let st = p.run_json(run)["stages"]["uat"].clone();
    assert_eq!((st["status"].as_str(), st["gate"].as_bool(), st["flow"].as_bool()), (Some("done"), Some(true), Some(true)), "{st}");
    assert!(st["missing_records"].to_string().contains("驗收報告.md"), "{st}");
    assert_eq!(st["approver"], "示範客戶 ACME 的 PM");
    // 產出補上、重存：缺件消失
    p.put(&format!("{run}/uat/驗收報告.md"), "# 驗收報告\n\n- 通過\n");
    p.done(run, "uat");
    let st = p.run_json(run)["stages"]["uat"].clone();
    assert!(st.get("missing_records").is_none(), "產出補上之後不該再有缺件：{st}");

    // 閘門：一律由人核准，顯示核准者、產出、給人看的段名
    let g = p.next(run, &[]);
    assert_eq!((g["action"].as_str(), g["stage"].as_str()), (Some("gate"), Some("uat")), "{g}");
    assert_eq!(g["human_only"], true);
    assert_eq!(g["custom"], true);
    assert_eq!(g["approver"], "示範客戶 ACME 的 PM");
    assert_eq!(g["artifacts"], json!(["驗收報告.md"]));
    assert_eq!(g["label"], "客戶驗收");
    // 再問一次還是同一個閘門（沒核准不會往下）
    assert_eq!(p.next(run, &[])["stage"], "uat");
    // 政策核准一律拒絕
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("一律由人核准"), "{}", r.all());
    assert!(p.run_json(run)["stages"]["uat"].get("approved").is_none());

    // 沒核准就想進下一段：deliver 可以存，但不能核准（閘門鏈算進自訂段）
    p.done(run, "deliver");
    let r = p.approve(run, "deliver", "human");
    assert!(r.code != 0 && r.all().contains("前面的 uat 段還沒核准"), "{}", r.all());
    // 推送與 merge gate 也一樣
    let r = p.publish(run);
    assert!(r.code != 0 && r.all().contains("自訂段還沒過") && r.all().contains("uat"), "{}", r.all());
    let gate = p.merge_gate(run);
    assert_eq!(gate["decision"], "hold", "{gate}");
    assert!(gate.to_string().contains("自訂段還沒過") && gate.to_string().contains("uat"), "{gate}");

    // status：段清單有自訂段（只加欄位）
    let s = p.awf(&["save-run", "status", p.dir.join(run).to_str().unwrap(), "--json"]).json();
    let rows = s["stages"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, ["plan", "design", "build", "sync", "test", "uat", "deliver", "merge"]);
    let row = rows.iter().find(|r| r["name"] == "uat").unwrap();
    assert_eq!((row["custom"].as_bool(), row["approver"].as_str(), row["label"].as_str(), row["gate"].as_bool()), (Some(true), Some("示範客戶 ACME 的 PM"), Some("客戶驗收"), Some(true)), "{row}");
    assert_eq!(rows.iter().find(|r| r["name"] == "deliver").unwrap()["label"], "交付審查");
    assert!(rows.iter().find(|r| r["name"] == "plan").unwrap().get("label").is_none() && rows.iter().find(|r| r["name"] == "plan").unwrap().get("custom").is_none());

    // 人核准之後才往下；走到完成、結案
    let r = p.approve(run, "uat", "示範客戶 ACME 的 PM");
    assert_eq!(r.code, 0, "{}", r.all());
    let gate = p.merge_gate(run);
    assert!(!gate.to_string().contains("自訂段還沒過"), "自訂段核准之後 merge gate 不再因為它 hold：{gate}");
    // deliver 還沒核准：推送照舊被原本的閘門擋（自訂段過了不代表後面的段可以省）
    let r = p.publish(run);
    assert!(r.code != 0 && !r.all().contains("自訂段還沒過") && r.all().contains("deliver"), "{}", r.all());
    let (seq, _) = p.walk_to(run, "done");
    assert_eq!(seq, ["gate:deliver", "manual:merge", "run_workflow:merge"], "自訂段核准之後接著走完");
    let r = p.awf(&["save-run", "close", p.dir.join(run).to_str().unwrap(), "--by", "human", "--at", "t"]);
    assert_eq!(r.code, 0, "awf save-run close：\n{}", r.all());
}

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "acme").env("GIT_AUTHOR_EMAIL", "dev@example.invalid").env("GIT_COMMITTER_NAME", "acme").env("GIT_COMMITTER_EMAIL", "dev@example.invalid").output().expect("啟動 git");
    assert!(o.status.success(), "git {args:?}：{}", String::from_utf8_lossy(&o.stderr));
}

impl Proj {
    fn doctor(&self) -> String {
        self.awf(&["doctor", "--plugin-dir", root().join("plugin").to_str().unwrap()]).all()
    }

    /// 推送的試跑（`--dry-run`，不連網）：目標 repo 是暫存目錄裡的 git repo，origin 指向設定的 fork。
    fn publish(&self, run: &str) -> Out {
        let repo = self.tmp.join("app");
        if !repo.is_dir() {
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            put(&repo.join("README.md"), "# 示範客戶 ACME\n");
            git(&repo, &["add", "-A"]);
            git(&repo, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]);
            git(&repo, &["remote", "add", "origin", "https://github.com/acme-fork/app.git"]);
            git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
            git(&repo, &["checkout", "-qb", "feature/export"]);
            put(&repo.join("docs/export.md"), "# 匯出 CSV\n");
            git(&repo, &["add", "-A"]);
            git(&repo, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "docs: 匯出說明"]);
            let gp = self.dir.join(".claude/teams/guard.config.json");
            let mut g = load(&gp);
            g["fork_repo"] = json!("acme-fork/app");
            put(&gp, &serde_json::to_string_pretty(&g).unwrap());
            put(&self.tmp.join("body.md"), "示範 PR 說明\n");
        }
        self.awf(&["publish", "push", "--repo-dir", repo.to_str().unwrap(), "--branch", "feature/export", "--run", self.dir.join(run).to_str().unwrap(), "--title", "docs: 匯出", "--body-file", self.tmp.join("body.md").to_str().unwrap(), "--dry-run"])
    }

    /// 只帶 `--recipe` 跑一次 merge gate（其他輸入沒給，本來就不會是 merge；這裡只看自訂段有沒有被算進去）。
    fn merge_gate(&self, run: &str) -> Value {
        self.put("d.diff", "diff --git a/docs/a.md b/docs/a.md\n--- a/docs/a.md\n+++ b/docs/a.md\n@@ -1 +1 @@\n-a\n+b\n");
        let rj = self.dir.join(run).join("run.json");
        let o = self.awf(&["merge-gate", "check", "--repo", "acme-fork/app", "--pr", "3", "--head-sha", SHA, "--diff", "d.diff", "--no-audit", "--recipe", rj.to_str().unwrap()]);
        serde_json::from_str(&o.stdout).unwrap_or_else(|e| panic!("merge-gate 的輸出不是 JSON（{e}）：{}", o.all()))
    }
}

/// 沒有點名就不跑：profile 宣告了自訂段，但 recipe 沒有點名。
#[test]
fn declared_but_not_named_does_not_run() {
    let p = Proj::new("unnamed", Some(flow_of(uat())), false);
    let d = p.doctor();
    assert!(d.contains("自訂段 uat 宣告了") && d.contains("不會跑"), "awf doctor 要提示沒有被點名的自訂段：\n{d}");
    let run = "_tpm/runs/r1";
    p.start(run);
    let (seq, _) = p.walk_to(run, "done");
    assert!(!seq.iter().any(|s| s.contains("uat")), "{seq:?}");
    let d = p.run_json(run);
    assert!(d["stages"].get("uat").is_none(), "沒點名的自訂段不該出現在 run.json");
    // 沒點名的自訂段不能自己存進來當成一段
    let r = p.done_raw(run, "uat");
    assert!(r.code != 0 || p.run_json(run)["stages"]["uat"]["status"] != "done", "{}", r.all());
}

/// 宣告不合規則：plan 存檔時就擋下來（recipe 點名的段不認得、profile 不合規則），不會默默略過那一段。
#[test]
fn invalid_declarations_are_refused() {
    let mut bad_script = uat();
    bad_script["script"] = json!("scripts/uat.js");
    let mut after_merge = uat();
    after_merge["after"] = json!("merge");
    let mut gate_policy = uat();
    gate_policy["gate"] = json!("policy");
    let mut id_builtin = uat();
    id_builtin["id"] = json!("design");
    let cases: Vec<(&str, Value, &str)> = vec![
        ("after-merge", json!({"stages": [after_merge]}), "after 不能是 merge"),
        ("gate-policy", json!({"stages": [gate_policy]}), "gate 這一版只收"),
        ("id-builtin", json!({"stages": [id_builtin]}), "不能和內建的段同名"),
        ("script-outside", json!({"stages": [bad_script]}), ".claude/flows/"),
    ];
    for (tag, flow, want) in cases {
        let named = tag != "id-builtin";
        let p = Proj::new(tag, Some(flow), named);
        p.put("scripts/uat.js", UAT_JS);
        // doctor 列得出來
        let d = p.awf(&["doctor", "--plugin-dir", root().join("plugin").to_str().unwrap()]);
        assert!(d.all().contains(want), "[{tag}] awf doctor 要列出「{want}」：\n{}", d.all());
        let run = "_tpm/runs/r1";
        p.start(run);
        let a = p.next(run, &[]);
        assert_eq!(a["stage"], "plan", "[{tag}] {a}");
        let r = p.done_raw(run, "plan");
        let st = p.run_json(run)["stages"]["plan"].clone();
        let all = format!("{}{}", r.all(), st);
        assert!(st["status"] != "done", "[{tag}] 宣告不合規則時 plan 不能是 done：{st}\n{}", r.all());
        assert!(all.contains(want), "[{tag}] 要說明「{want}」：\n{all}");
        let r = p.approve(run, "plan", "human");
        assert!(r.code != 0, "[{tag}] plan 不能核准：{}", r.all());
        // 不會跑到自訂段
        let a = p.next(run, &[]);
        assert!(a["stage"] != "uat" && a["action"] != "gate", "[{tag}] {a}");
    }
}

/// recipe 點名了自訂段、profile 卻沒有 `flow`：一樣在 plan 段擋下來（不會把那一段默默丟掉、照常往下走）。
#[test]
fn named_without_flow_is_refused() {
    let p = Proj::new("noflow", None, true);
    let want = "不是認得的段";
    assert!(p.doctor().contains(want), "awf doctor 要列出「{want}」：\n{}", p.doctor());
    let run = "_tpm/runs/r1";
    p.start(run);
    assert_eq!(p.next(run, &[])["stage"], "plan");
    let r = p.done_raw(run, "plan");
    let st = p.run_json(run)["stages"]["plan"].clone();
    assert!(st["status"] != "done" && format!("{}{}", r.all(), st).contains(want), "{st}\n{}", r.all());
    assert!(p.approve(run, "plan", "human").code != 0);
}

/// 鎖定的 recipe 點名的自訂段被從 profile 拿掉（或腳本不見、檢查不過）：`awf next` 停下交人，不默默略過。
#[test]
fn locked_recipe_with_removed_stage_halts() {
    let p = Proj::new("removed", Some(flow_of(uat())), true);
    let run = "_tpm/runs/r1";
    p.start(run);
    let (_, _) = p.walk_to(run, "run_workflow:build");

    // 腳本不見
    let (_, _) = p.walk_to(run, "run_workflow:uat");
    std::fs::remove_file(p.dir.join(run).join(".wf/flow-uat.js")).unwrap();
    std::fs::rename(p.dir.join(".claude/flows/uat.js"), p.dir.join(".claude/flows/uat.js.bak")).unwrap();
    let r = p.next_raw(run, &[]);
    assert!(r.code != 0 && r.all().contains("不存在"), "腳本不存在要停下：{}", r.all());
    assert!(p.doctor().contains("自訂段 uat 的腳本 .claude/flows/uat.js 不存在"), "awf doctor 要列出不存在的腳本：\n{}", p.doctor());
    // 腳本檢查不過（直接呼叫 agent()）
    p.put(".claude/flows/uat.js", &UAT_JS.replace("await awfAgent(", "await agent("));
    let r = p.next_raw(run, &[]);
    assert!(r.code != 0 && r.all().contains("agent()"), "腳本檢查不過要停下：{}", r.all());
    assert!(!p.dir.join(run).join(".wf/flow-uat.js").is_file(), "檢查不過不能產生腳本");
    assert!(p.doctor().contains("自訂段 uat 的腳本 .claude/flows/uat.js 不能用"), "awf doctor 要列出沒通過檢查的腳本：\n{}", p.doctor());
    // 符號連結連到 .claude/flows/ 以外的檔：不採用
    #[cfg(unix)]
    {
        std::fs::remove_file(p.dir.join(".claude/flows/uat.js")).unwrap();
        p.put("scripts/elsewhere.js", UAT_JS);
        std::os::unix::fs::symlink(p.dir.join("scripts/elsewhere.js"), p.dir.join(".claude/flows/uat.js")).unwrap();
        let r = p.next_raw(run, &[]);
        assert!(r.code != 0 && r.all().contains("實際位置不在"), "連到別處的腳本不採用：{}", r.all());
        std::fs::remove_file(p.dir.join(".claude/flows/uat.js")).unwrap();
    }
    p.put(".claude/flows/uat.js", UAT_JS);

    // profile 拿掉 flow（recipe 也不再點名）：鎖定的 recipe 還點名 uat
    p.edit_profile(|m| {
        m.as_object_mut().unwrap().remove("flow");
        let st = m["recipes"]["bugfix"]["stages"].as_array_mut().unwrap();
        st.retain(|x| x != "uat");
    });
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["status"].as_str()), (Some("halted"), Some("flow")), "{a}");
    assert!(a["reason"].to_string().contains("uat"), "{a}");
    // 存檔、核准、升級、推送、merge gate 都不採信
    let r = p.done_raw(run, "deliver");
    assert!(p.run_json(run)["stages"]["deliver"]["status"] != "done", "自訂段不在 profile 時後面的段不能存成 done：{}", r.all());
    let r = p.awf(&["save-run", "escalate", p.dir.join(run).to_str().unwrap(), "--recipe", "feature", "--by", "human", "--at", "t", "--reason", "加段"]);
    assert!(r.code != 0 && r.all().contains("uat"), "升級不能把自訂段默默略過：{}", r.all());
    let gate = p.merge_gate(run);
    assert_eq!(gate["decision"], "hold", "{gate}");
    assert!(gate.to_string().contains("uat"), "{gate}");

    // 只把宣告補回來、順序改掉（after 換成 build）：一樣停下
    p.edit_profile(|m| {
        let mut u = uat();
        u["after"] = json!("build");
        m["flow"] = json!({"stages": [u]});
        let st = m["recipes"]["bugfix"]["stages"].as_array_mut().unwrap();
        let at = st.iter().position(|x| x == "build").unwrap() + 1;
        st.insert(at, json!("uat"));
    });
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["status"].as_str()), (Some("halted"), Some("flow")), "順序和核准時不同也要停下：{a}");
}

/// 升級（escalate）加進來的自訂段：照順序處理，後面已完成的段改成 stale。
#[test]
fn escalate_adds_custom_stage_in_order() {
    let p = Proj::new("escalate", Some(flow_of(uat())), false);
    p.edit_profile(|m| {
        m["modifiers"]["client"] = json!({"description": "示範客戶 ACME 要驗收", "add_stages": ["uat"]});
    });
    let run = "_tpm/runs/r1";
    p.start(run);
    let (seq, _) = p.walk_to(run, "manual:merge");
    assert!(!seq.iter().any(|s| s.contains("uat")));
    let r = p.awf(&["save-run", "escalate", p.dir.join(run).to_str().unwrap(), "--recipe", "bugfix", "--modifiers", "client", "--by", "human", "--at", "t", "--reason", "客戶要驗收"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!(strs(&d["recipe"]["resolved"]["stages"]), ["plan", "build", "sync", "test", "uat", "deliver", "merge"]);
    assert_eq!(d["stages"]["uat"], json!({"status": "pending", "gate": true, "flow": true}));
    assert_eq!(d["stages"]["deliver"]["status"], "stale", "加進來的段之後已完成的段要重跑：{}", d["stages"]["deliver"]);
    assert_eq!(d["stages"]["test"]["status"], "done", "前面的段不動");
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("uat")), "{a}");
}
