//! 用步驟宣告的自訂段（profile 的 `flow.stages[].run: "steps"`）與政策核准的端到端測試（不花 token）：
//! `awf init` 套出暫存專案，profile 加一個自訂段 `uat`（接在 test 後面、由 bugfix 點名、`gate: "policy"`），
//! 每一段給假的段結果；`uat` 的結果放在假的 workflow 紀錄裡，讓 `awf done` 自己取得（這樣才是標準結果）。
//!
//! - steps＋policy 的自訂段從開工走到結案（政策核准、由工具執行的指令記進 run.json）；
//! - check 指令失敗：政策核准被拒、交人；人核准之後照常往下；
//! - 回傳說 loop 過關，但工具重跑 until 不過：政策核准被拒；
//! - 少了一步的交件、手動給的結果、定義在跑完之後改過：政策核准被拒；
//! - `run: "script"` 搭 `gate: "policy"` 被 profile 的語意規則拒絕；
//! - profile 的事件定義裡的佔位符（從內建事件複製過來的）會被代入；事件可以用 `extends` 與 `steps`。
//!
//! 範本與 schema 取自這個 repo 的 `plugin/`（`AWF_PLUGIN_DIR`）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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

/// 自訂段 uat：一個角度的 fanout、可跳過的反駁、最多兩輪的 loop（結束條件是目標 repo 裡有 ok.flag）、一個 check（有 lint.flag）。
fn uat() -> Value {
    json!({"id": "uat", "after": "test", "run": "steps", "gate": "policy", "approver": "示範客戶 ACME 的 PM", "context": "驗收 {epic}",
        "steps": [
            {"id": "survey", "kind": "fanout", "angles": [{"key": "api", "ask": "逐條確認驗收清單"}]},
            {"id": "refute", "kind": "challenge", "ask": "逐則反駁", "optional": true},
            {"id": "fix", "kind": "loop", "max_rounds": 2, "until": {"command": "test -f ok.flag", "timeout": 30}, "steps": [{"id": "report", "kind": "write", "ask": "把結果寫進驗收報告"}]},
            {"id": "lint", "kind": "check", "command": "test -f lint.flag", "timeout": 30}
        ]})
}

impl Proj {
    /// `stage`＝profile 的 `flow.stages` 唯一的一筆（None＝不寫 flow）；有寫就讓 bugfix 點名 uat（接在 test 後面）。
    fn new(tag: &str, stage: Option<Value>) -> Proj {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("awf-steps-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).expect("建暫存專案");
        let tmp = tmp.canonicalize().expect("解開暫存目錄");
        let p = Proj { dir: tmp.join("proj"), tmp };
        let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", root().join("plugin/workflow.config.example").to_str().unwrap()]);
        assert_eq!(r.code, 0, "awf init：\n{}", r.all());
        p.edit_profile(|m| {
            // 模型分級不是這裡要測的：假的 trace 不帶 model／tier
            m["models"] = json!({"mode": "inherit"});
            if let Some(s) = stage.clone() {
                m["flow"] = json!({"stages": [s]});
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

    fn next(&self, run: &str, extra: &[&str]) -> Value {
        let mut a = vec!["next", "--run", run];
        a.extend_from_slice(extra);
        let r = self.awf(&a);
        assert_eq!(r.code, 0, "awf next {extra:?}：\n{}", r.all());
        r.json()
    }

    fn trace(&self, run: &str, stage: &str, labels: &[(&str, &str)]) -> Vec<Value> {
        labels
            .iter()
            .map(|(label, role)| {
                let rel = format!("{run}/{stage}/{}.md", label.replace(':', "-"));
                self.put(&rel, "# 紀錄\n\n## 產出\n- 做完了\n");
                json!({"label": label, "agent_type": format!("agent-workflow:{role}"), "phase": "Run", "record": self.dir.join(&rel).to_str().unwrap(), "ok": true})
            })
            .collect()
    }

    fn recipe(&self, run: &str) -> Value {
        let rec = self.run_json(run)["recipe"].clone();
        json!({"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]})
    }

    /// 內建段的假結果（同 flow_e2e）。
    fn result(&self, run: &str, stage: &str) -> Value {
        if stage == "plan" {
            self.put("specs/export/intent.md", "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n## 現象\n匯出空白\n## 重現步驟\n1. 點匯出\n## 預期行為\n有資料\n## 根因\n空集合沒處理\n");
            return json!({"stage": "plan", "epic_dir": self.dir.to_str().unwrap(), "recipe": {"name": "bugfix", "modifiers": []},
                "subtasks": [{"id": "t1", "title": "t1", "kind": "code", "owns": ["src/export.py"], "depends_on": [], "acceptance": ["AC-1 下載 CSV"]}],
                "intent": envl(json!({"documents": [{"path": "specs/export/intent.md", "kind": "intent", "audience": "owner"}]})), "trace": []});
        }
        let res = self.run_json(run)["recipe"]["resolved"].clone();
        let mut labels: Vec<(String, &str)> = Vec::new();
        match stage {
            "test" => labels.push(("verify".to_string(), "dev-verifier")),
            "deliver" => {
                for axis in res["deliver_axes"].as_array().cloned().unwrap_or_default() {
                    labels.push((format!("review:{}", axis.as_str().unwrap_or_default()), "tpm-reviewer"));
                }
            }
            _ => labels.push((format!("{stage}:x"), "dev-implementer")),
        }
        let pairs: Vec<(&str, &str)> = labels.iter().map(|(l, r)| (l.as_str(), *r)).collect();
        let mut out = json!({"stage": stage, "halted": false, "recipe": self.recipe(run), "trace": self.trace(run, stage, &pairs)});
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
            "deliver" => out["review"] = envl(json!({"lens": "spec", "findings": []})),
            _ => out["x"] = envl(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/export.py"], "tests": []})),
        }
        out
    }

    /// uat 的假結果，形狀同 flow-steps.js 的回傳：一輪就過（判定回報 pass）、沒有發現所以反駁跳過。
    fn uat_result(&self, run: &str) -> Value {
        let trace = self.trace(run, "uat", &[("survey:api", "tpm-researcher"), ("fix.r1:report", "tpm-writer"), ("fix.r1:until", "dev-verifier")]);
        json!({"stage": "uat", "recipe": self.recipe(run), "halted": false, "missing": [],
            "steps": {
                "survey": {"kind": "fanout", "results": [envl(json!({"lens": "api", "findings": []}))], "findings": 0},
                "refute": {"kind": "challenge", "skipped": true, "targets": 0},
                "fix": {"kind": "loop", "rounds": 1, "passed": true,
                    "until": envl(json!({"commit": SHA, "scope_covered": "until", "scope_skipped": "none", "anti_cheat": [],
                        "criteria": [{"id": "until", "criterion": "結束條件的指令 exit 0", "command": "test -f ok.flag", "result": "pass", "evidence": "exit 0"}]})),
                    "steps": {"report": {"kind": "write", "result": envl(json!({"documents": [{"path": format!("{run}/uat/驗收報告.md"), "kind": "other", "audience": "owner"}]}))}}},
                "lint": {"kind": "check", "deferred": true}
            },
            "checks": [{"id": "fix", "kind": "until"}, {"id": "lint", "kind": "check"}],
            "trace": trace})
    }

    fn done(&self, run: &str, stage: &str) {
        let f = self.tmp.join(format!("{stage}.json"));
        put(&f, &self.result(run, stage).to_string());
        let r = self.awf(&["done", "--run", run, "--stage", stage, "--output", f.to_str().unwrap(), "--no-next"]);
        assert_eq!(r.code, 0, "awf done（{stage}）：\n{}", r.all());
    }

    /// 把 `result` 放進假的 workflow 紀錄（scriptPath 是 awf next 給的腳本），再跑 `awf done`（不給 --output：由它自己從紀錄取得）。
    fn done_from_transcript(&self, run: &str, act: &Value, result: &Value, tag: &str) -> Out {
        let script = self.dir.join(act["script"].as_str().expect("動作有 script"));
        let dir = self.tmp.join("transcripts/sess-1/workflows");
        let _ = std::fs::remove_dir_all(&dir);
        put(&dir.join(format!("wf_{tag}.json")), &json!({"runId": format!("wf_{tag}"), "scriptPath": script.to_str().unwrap(), "status": "completed", "result": result}).to_string());
        self.awf(&["done", "--run", run, "--no-next"])
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

    /// 開工、走到 uat 要跑的那一步，回傳那個動作。
    fn to_uat(&self, run: &str) -> Value {
        self.start(run);
        let (seq, act) = self.walk_to(run, "run_workflow:uat");
        assert_eq!(seq, ["run_workflow:plan", "gate:plan", "run_workflow:build", "run_workflow:sync", "run_workflow:test", "gate:test"]);
        act
    }

    fn flags(&self, names: &[&str]) {
        for n in ["ok.flag", "lint.flag"] {
            let f = self.dir.join(n);
            if names.contains(&n) {
                put(&f, "ok\n");
            } else {
                let _ = std::fs::remove_file(f);
            }
        }
    }
}

/// steps＋policy 的自訂段從開工走到結案。
#[test]
fn steps_policy_stage_from_start_to_close() {
    let p = Proj::new("full", Some(uat()));
    let d = p.awf(&["doctor", "--plugin-dir", root().join("plugin").to_str().unwrap()]).all();
    assert!(d.contains("自訂段：uat（接在 test 後面；用步驟宣告，先試政策核准）"), "awf doctor 要列出自訂段：\n{d}");
    let run = "_tpm/runs/r1";
    let act = p.to_uat(run);

    // 動作：plugin 的 flow-steps 範本，不是自訂腳本（沒有 custom）；帶段名與定義的指紋
    assert_eq!(act["workflow"], "flow-uat");
    assert_eq!(act["script"], format!("{run}/.wf/flow-uat.js"));
    assert!(act.get("custom").is_none() && act.get("override").is_none(), "{act}");
    assert_eq!(act["steps"]["stage"], "uat");
    assert_eq!(act["steps"]["sha1"].as_str().map(str::len), Some(40), "{act}");
    let body = std::fs::read_to_string(p.dir.join(run).join(".wf/flow-uat.js")).unwrap();
    assert!(body.contains("name: 'flow-steps'") && body.contains("function awfStepsRun(") && body.contains("\"stage\":\"uat\""), "腳本是 flow-steps 範本、帶這一段的名字");
    // 步驟（含指令）來自內嵌的 profile，不在 args 裡
    let args = load(&p.dir.join(run).join(".wf/flow-uat.args.json"));
    assert!(args.get("steps").is_none() && !args.to_string().contains("ok.flag"), "args 不帶步驟與指令：{args}");
    assert!(body.contains("test -f ok.flag"), "指令在內嵌的 profile 裡");
    assert!(!body.contains("{{"), "沒有殘留的佔位符");
    assert!(act["notes"].to_string().contains("2 個指令在存檔時由工具執行") && act["notes"].to_string().contains("先試政策核准"), "{act}");

    // 兩個指令都會過；結果由 awf done 從 workflow 紀錄取得
    p.flags(&["ok.flag", "lint.flag"]);
    let r = p.done_from_transcript(run, &act, &p.uat_result(run), "uat1");
    assert_eq!(r.code, 0, "{}", r.all());
    let st = p.run_json(run)["stages"]["uat"].clone();
    assert_eq!(st["status"], "done", "{st}");
    assert!(st.get("step_issues").is_none(), "{st}");
    let checks = st["step_checks"].as_array().expect("記了指令的結果");
    assert_eq!(checks.iter().map(|c| (c["id"].as_str().unwrap(), c["kind"].as_str().unwrap(), c["exit"].as_i64().unwrap())).collect::<Vec<_>>(), [("fix", "until", 0), ("lint", "check", 0)]);
    let saved = load(&p.dir.join(run).join("uat.json"));
    assert_eq!(saved["flow_steps"]["stage"], "uat", "awf done 寫上標準步驟腳本的標記");
    assert!(saved.get("manual_output").is_none() && saved.get("custom_script").is_none());

    // awf next：先試政策核准，過了直接往下一段
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("deliver")), "{a}");
    let ap = p.run_json(run)["stages"]["uat"]["approved"].clone();
    assert_eq!((ap["by"].as_str(), ap["kind"].as_str()), (Some("policy"), Some("policy")), "{ap}");
    assert!(ap["note"].as_str().unwrap_or_default().contains("gate=policy"), "{ap}");

    // 後面照常：deliver、merge，結案（閘門鏈、結案都認這筆政策核准）
    let (seq, _) = p.walk_to(run, "done");
    assert_eq!(seq, ["run_workflow:deliver", "gate:deliver", "manual:merge", "run_workflow:merge"]);
    let r = p.awf(&["save-run", "close", p.dir.join(run).to_str().unwrap(), "--by", "human", "--at", "t"]);
    assert_eq!(r.code, 0, "awf save-run close：\n{}", r.all());

    // profile 之後把 gate 改回 human：這筆政策核准就不算數了（閘門鏈重新擋）
    p.edit_profile(|m| m["flow"]["stages"][0]["gate"] = json!("human"));
    let r = p.approve(run, "deliver", "human");
    assert!(r.code != 0 && r.all().contains("uat"), "gate 改回 human 之後，政策核准的 uat 不採信：{}", r.all());
}

/// check 指令失敗：政策核准被拒、交人；人核准之後照常往下。
#[test]
fn failed_check_command_goes_to_human() {
    let p = Proj::new("check", Some(uat()));
    let run = "_tpm/runs/r1";
    let act = p.to_uat(run);
    p.flags(&["ok.flag"]); // lint.flag 不在：check 會失敗
    let r = p.done_from_transcript(run, &act, &p.uat_result(run), "uat1");
    assert_eq!(r.code, 0, "指令沒過不是存檔失敗：{}", r.all());
    assert!(r.all().contains("步驟 lint 的檢查指令由工具重跑沒過") && r.all().contains("不能政策核准"), "{}", r.all());
    let st = p.run_json(run)["stages"]["uat"].clone();
    assert_eq!(st["status"], "done");
    assert!(st["step_issues"].to_string().contains("lint"), "{st}");
    assert_eq!(st["step_checks"][1]["exit"], 1, "{st}");

    // awf next：試了政策核准、被拒，停在閘門交人
    let g = p.next(run, &[]);
    assert_eq!((g["action"].as_str(), g["stage"].as_str()), (Some("gate"), Some("uat")), "{g}");
    assert!(g["policy_refused"].to_string().contains("步驟 lint 的檢查指令"), "{g}");
    assert!(g["step_issues"].to_string().contains("lint"), "{g}");
    assert!(p.run_json(run)["stages"]["uat"].get("approved").is_none());
    // 直接下政策核准一樣被拒
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("檢查指令由工具重跑沒過"), "{}", r.all());
    // run.json 記的 step_issues 被清掉也沒用：核准時重跑，不採信紀錄
    let rj = p.dir.join(run).join("run.json");
    let mut d = load(&rj);
    d["stages"]["uat"].as_object_mut().unwrap().remove("step_issues");
    d["stages"]["uat"]["step_checks"][1]["exit"] = json!(0);
    put(&rj, &d.to_string());
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("檢查指令由工具重跑沒過"), "{}", r.all());

    // 人核准（只警告、不擋），之後照常往下
    let r = p.approve(run, "uat", "示範客戶 ACME 的 PM");
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(p.run_json(run)["stages"]["uat"]["approved"]["kind"], "human");
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("deliver")), "{a}");
}

/// 回傳說 loop 過關了，但工具重跑 until 的指令不過：政策核准被拒。
#[test]
fn until_reported_pass_but_tool_rerun_fails() {
    let p = Proj::new("until", Some(uat()));
    let run = "_tpm/runs/r1";
    let act = p.to_uat(run);
    p.flags(&["lint.flag"]); // ok.flag 不在：until 的指令實際上不會過
    let r = p.done_from_transcript(run, &act, &p.uat_result(run), "uat1");
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.all().contains("步驟 fix 的loop 的結束條件由工具重跑沒過"), "{}", r.all());
    let g = p.next(run, &[]);
    assert_eq!((g["action"].as_str(), g["stage"].as_str()), (Some("gate"), Some("uat")), "{g}");
    assert!(g["policy_refused"].to_string().contains("步驟 fix"), "{g}");
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("結束條件由工具重跑沒過"), "{}", r.all());
    assert!(p.run_json(run)["stages"]["uat"].get("approved").is_none());
}

/// 其他不能政策核准的情況：少了一步的交件、手動給的結果、定義在跑完之後改過、回傳自己帶標記。
#[test]
fn policy_needs_every_step_and_a_standard_result() {
    let p = Proj::new("strict", Some(uat()));
    let run = "_tpm/runs/r1";
    let act = p.to_uat(run);
    p.flags(&["ok.flag", "lint.flag"]);

    // 少了 write 那一步的交件（回傳自己仍然說 passed）
    let mut r1 = p.uat_result(run);
    r1["trace"].as_array_mut().unwrap().retain(|t| t["label"] != "fix.r1:report");
    let r = p.done_from_transcript(run, &act, &r1, "uat1");
    assert_eq!(r.code, 0, "{}", r.all());
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("fix.r1:report"), "少一步的交件要被擋：{}", r.all());

    // 手動給的結果（--output），而且結果裡自己塞了標記：標記被拿掉、不能政策核准
    let mut forged = p.uat_result(run);
    forged["flow_steps"] = act["steps"].clone();
    let f = p.tmp.join("uat.json");
    put(&f, &forged.to_string());
    let r = p.awf(&["done", "--run", run, "--stage", "uat", "--output", f.to_str().unwrap(), "--no-next"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let saved = load(&p.dir.join(run).join("uat.json"));
    assert!(saved.get("flow_steps").is_none() && saved["manual_output"] == true, "{saved}");
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("手動給的") && r.all().contains("沒有標準步驟腳本的標記"), "{}", r.all());

    // 標準結果，但定義在跑完之後改過（多一個 check）
    let act = p.next(run, &["--stage", "uat", "--workflow", "flow-uat"]);
    assert_eq!((act["action"].as_str(), act["stage"].as_str()), (Some("run_workflow"), Some("uat")), "{act}");
    let r = p.done_from_transcript(run, &act, &p.uat_result(run), "uat2");
    assert_eq!(r.code, 0, "{}", r.all());
    p.edit_profile(|m| m["flow"]["stages"][0]["steps"].as_array_mut().unwrap().push(json!({"id": "extra", "kind": "check", "command": "true"})));
    let r = p.approve(run, "uat", "policy");
    assert!(r.code != 0 && r.all().contains("步驟定義在這一段跑完之後改過"), "{}", r.all());
    assert!(p.run_json(run)["stages"]["uat"].get("approved").is_none());
}

/// 宣告不合規則的：`run: "script"` 搭 `gate: "policy"`、steps 與 script 併用、超過上限、換上沒登記的 agent。
#[test]
fn bad_declarations_are_rejected() {
    let script_policy = json!({"id": "uat", "after": "test", "run": "script", "script": ".claude/flows/uat.js", "gate": "policy"});
    let mut both = uat();
    both["script"] = json!(".claude/flows/uat.js");
    let mut nested = uat();
    nested["steps"][2]["steps"] = json!([{"id": "inner", "kind": "loop", "max_rounds": 1, "until": {"command": "true"}, "steps": [{"id": "w", "kind": "write", "ask": "x"}]}]);
    let mut rounds = uat();
    rounds["steps"][2]["max_rounds"] = json!(6);
    for (tag, stage, want) in [
        ("script-policy", script_policy, "只有 run 是 steps 才能用"),
        ("both", both, "不能寫 script"),
        ("nested", nested, "loop 裡不能再放 loop"),
        ("rounds", rounds, "max_rounds"),
    ] {
        let p = Proj::new(tag, Some(stage));
        p.put(".claude/flows/uat.js", "export const meta = { name: 'uat', description: 'x' }\nreturn { stage: 'uat', trace: awfTrace() }\n");
        let d = p.awf(&["doctor", "--plugin-dir", root().join("plugin").to_str().unwrap()]).all();
        assert!(d.contains(want), "{tag}：awf doctor 要指出「{want}」：\n{d}");
        // 閘門工具讀 profile 時一樣拒絕（只跑語意規則的那條路）：plan 段存檔就因為 profile 不合規則判 invalid
        let run = "_tpm/runs/r1";
        p.start(run);
        let a = p.next(run, &[]);
        assert_eq!(a["stage"], "plan", "{a}");
        let f = p.tmp.join("plan.json");
        put(&f, &p.result(run, "plan").to_string());
        let r = p.awf(&["done", "--run", run, "--stage", "plan", "--output", f.to_str().unwrap(), "--no-next"]);
        assert!(r.code != 0 && r.all().contains(want), "{tag}：存檔時要因為 profile 不合規則被擋：{}", r.all());
        assert_eq!(p.run_json(run)["stages"]["plan"]["status"], "invalid");
    }

    // 換上沒登記的 agent：拒絕產生腳本；登記之後才產生
    let mut ext = uat();
    ext["steps"][0]["role"] = json!("acme:perf-bot");
    let p = Proj::new("ext", Some(ext));
    let run = "_tpm/runs/r1";
    p.start(run);
    // 和 extensions 一樣：沒登記時任何一段的腳本都不產生
    let r = p.awf(&["next", "--run", run]);
    assert!(r.code != 0 && r.all().contains("acme:perf-bot（steps 換上去的）") && r.all().contains("extension_agents"), "{}", r.all());
    let gp = p.dir.join(".claude/teams/guard.config.json");
    let mut g = load(&gp);
    g["extension_agents"]["acme:perf-bot"] = json!("tpm-researcher");
    put(&gp, &serde_json::to_string_pretty(&g).unwrap());
    let (_, a) = p.walk_to(run, "run_workflow:uat");
    assert_eq!(a["workflow"], "flow-uat", "{a}");
}

/// profile 的事件定義：從內建事件複製過來的佔位符要被代入；`extends` 與 `steps` 產生得出腳本。
#[test]
fn profile_events_placeholders_extends_and_steps() {
    let p = Proj::new("events", None);
    p.edit_profile(|m| {
        m["events"] = json!({
            // 從 event.js 的內建定義複製過來的文字帶佔位符
            "handover": {"title": "交接", "args": ["source"], "context": "來源：{source}\nrun 目錄：{run_dir}",
                "gather": {"angles": [{"key": "todo", "ask": "要交給 {{TASK_AUTHORITY}} 編號的新工作；規格在 {{SPECS_DIR}}/。"}]},
                "challenge": {"ask": "逐則反駁"}, "write": {"ask": "寫交接摘要"}},
            "meeting": {"extends": "meeting", "write": {"owner_check": true}},
            "audit": {"title": "稽核", "args": ["target"], "context": "對象：{target}",
                "steps": [{"id": "scan", "kind": "fanout", "angles": [{"key": "docs", "ask": "看文件"}]}, {"id": "refute", "kind": "challenge", "ask": "反駁"}, {"id": "report", "kind": "write", "ask": "寫報告"}]}
        });
    });
    let out = p.tmp.join("wf");
    let r = p.awf(&["render", "--all", "--out", out.to_str().unwrap(), "--plugin-dir", root().join("plugin").to_str().unwrap()]);
    assert_eq!(r.code, 0, "awf render：\n{}", r.all());
    let body = std::fs::read_to_string(out.join("event.js")).unwrap();
    assert!(!body.contains("{{TASK_AUTHORITY}}") && !body.contains("{{SPECS_DIR}}"), "profile 的事件定義裡的佔位符要被代入");
    assert!(body.contains("編號的新工作；規格在 specs/。"), "代入的是設定的值（SPECS_DIR 預設 specs）");
    assert!(body.contains("\"extends\":\"meeting\""), "extends 的定義原樣內嵌，由範本合併");

    // extends 的事件：必要參數取自底（內建的 meeting 要 source）
    let run = "_tpm/runs/e1";
    let r = p.awf(&["start", "--run", run, "--event", "meeting"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let a = p.next(run, &[]);
    assert_eq!(a["action"], "need_input", "{a}");
    assert!(a["need"].as_object().is_some_and(|n| n.contains_key("source")), "{a}");

    // 用 steps 的事件：產生得出腳本
    let run = "_tpm/runs/e2";
    let r = p.awf(&["start", "--run", run, "--event", "audit", "--set", "target=報表頁"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["workflow"].as_str()), (Some("run_workflow"), Some("event")), "{a}");

    // 不認得的佔位符：拒絕產生（不留著沒換掉的字）
    p.edit_profile(|m| m["events"]["handover"]["write"]["ask"] = json!("寫到 {{NO_SUCH_KEY}}"));
    let r = p.awf(&["render", "--all", "--out", out.to_str().unwrap(), "--plugin-dir", root().join("plugin").to_str().unwrap()]);
    assert!(r.code != 0 && r.all().contains("NO_SUCH_KEY"), "{}", r.all());
}
