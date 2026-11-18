//! 1.2 補的幾條「合法的路」的端到端測試（不花 token；假的段結果，只用 `awf` 的指令）：
//!
//! - A1：段停在 needs_decision，人把每一題 blocking 都答了就往下走；少答、對不上、`--by policy` 都被拒。
//! - A3：直接存檔的結果和 workflow 實際回傳的不一致 → 標成非標準、政策核准被拒。
//! - A4：build 續跑時略過子任務；被依賴的不能單獨略過；略過之後不能政策核准。
//! - A5：halted 的 `retry` 只給指令；沒先跑 `awf next --retry` 就重跑舊腳本時，`awf done` 只在分得清楚時採用。
//! - B：本機合併（`conventions.merge.mode: local`）從 deliver 核准走到結案；審過的 commit 不在主線時不算完成。
//! - C：`decisions.auto_accept` 開啟後照建議先做、下一個閘門列出；plan 與 high risk 不適用；關閉時輸出逐字不變。
//!
//! 範本與 schema 取自這個 repo 的 `plugin/`（`AWF_PLUGIN_DIR`）；專案用 `awf init` 套出來。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
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

/// 一題 blocking 的問題。`risk` 是 None 時不帶這個欄位。
fn question(text: &str, rec: &str, risk: Option<&str>) -> Value {
    let mut q = json!({"question": text, "blocking": true, "why_blocking": "w", "options": [{"choice": "A", "cost": "x"}, {"choice": "B", "cost": "y"}], "recommendation": rec, "default_applied": "none"});
    if let Some(r) = risk {
        q["risk"] = json!(r);
    }
    q
}

/// 把結果裡 `key` 那一份信封改成在等人決定。
fn ask(mut result: Value, key: &str, questions: Vec<Value>) -> Value {
    result[key]["status"] = json!("needs_decision");
    result[key]["open_questions"] = Value::Array(questions);
    result
}

impl Proj {
    fn new(tag: &str) -> Proj {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("awf-human-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).expect("建暫存專案");
        let tmp = tmp.canonicalize().expect("解開暫存目錄");
        let p = Proj { dir: tmp.join("proj"), tmp };
        let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", root().join("plugin/workflow.config.example").to_str().unwrap()]);
        assert_eq!(r.code, 0, "awf init：\n{}", r.all());
        // 模型分級不是這裡要測的：假的 trace 不帶 model／tier
        p.edit_profile(|m| m["models"] = json!({"mode": "inherit"}));
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

    fn awf_in(&self, args: &[&str], stdin: Option<&str>) -> Out {
        let mut c = Command::new(EXE);
        c.args(args).current_dir(&self.dir).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            if k.starts_with("CLAUDE_") || k.starts_with("AWF_") || k.starts_with("GIT_") {
                c.env_remove(k);
            }
        }
        c.env("CLAUDE_PROJECT_DIR", &self.dir).env("AWF_PLUGIN_DIR", root().join("plugin")).env("AWF_TRANSCRIPTS_DIR", self.tmp.join("transcripts"));
        let mut child = c.spawn().expect("啟動 awf");
        if let Some(text) = stdin {
            child.stdin.take().expect("stdin").write_all(text.as_bytes()).expect("寫 stdin");
        }
        let o = child.wait_with_output().expect("等 awf");
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
    }

    fn awf(&self, args: &[&str]) -> Out {
        self.awf_in(args, None)
    }

    fn git(&self, args: &[&str]) -> String {
        let o = Command::new("git").arg("-C").arg(&self.dir).args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]).args(args).output().expect("啟動 git");
        assert!(o.status.success(), "git {args:?}：{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
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

    /// 某一段的假結果（`sha`＝test 段驗證當下的 commit）。
    fn result(&self, run: &str, stage: &str, sha: &str) -> Value {
        if stage == "plan" {
            self.put("specs/export/intent.md", "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n## 現象\n匯出空白\n## 重現步驟\n1. 點匯出\n## 預期行為\n有資料\n## 根因\n空集合沒處理\n");
            let task = |id: &str, deps: Value| json!({"id": id, "title": id, "kind": "code", "owns": [format!("src/{id}.py")], "depends_on": deps, "acceptance": ["AC-1 下載 CSV"]});
            return json!({"stage": "plan", "epic_dir": self.dir.to_str().unwrap(), "recipe": {"name": "bugfix", "modifiers": []},
                "subtasks": [task("t1", json!([])), task("t2", json!(["t1"])), task("t3", json!([]))],
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
                out["verifier"] = envl(json!({"commit": sha, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
                    "criteria": [{"id": "AC-1", "criterion": "c", "command": "pytest", "result": "pass", "evidence": "e"}]}));
            }
            "deliver" => out["review"] = envl(json!({"lens": "spec", "findings": []})),
            _ => out["x"] = envl(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/export.py"], "tests": []})),
        }
        out
    }

    /// 手動給結果存一段（`awf done --output`）。
    fn done_with(&self, run: &str, stage: &str, result: &Value) -> Out {
        let f = self.tmp.join(format!("{stage}.json"));
        put(&f, &result.to_string());
        self.awf(&["done", "--run", run, "--stage", stage, "--output", f.to_str().unwrap(), "--no-next"])
    }

    fn done(&self, run: &str, stage: &str, sha: &str) {
        let r = self.done_with(run, stage, &self.result(run, stage, sha));
        assert_eq!(r.code, 0, "awf done（{stage}）：\n{}", r.all());
    }

    fn approve(&self, run: &str, stage: &str, extra: &[&str]) -> Out {
        let mut a = vec!["approve", "--run", run, stage];
        a.extend_from_slice(extra);
        self.awf(&a)
    }

    /// 假的 workflow 紀錄：用 `script` 跑出 `result`。
    fn record(&self, name: &str, script: &str, result: &Value) {
        put(&self.tmp.join(format!("transcripts/sess-1/workflows/{name}.json")), &json!({"runId": name, "scriptPath": self.dir.join(script).to_str().unwrap(), "status": "completed", "result": result}).to_string());
    }

    /// plan 用手動結果存好並由人核准。
    fn plan_approved(&self, run: &str) {
        self.start(run);
        self.next(run, &[]);
        self.done(run, "plan", "");
        let r = self.approve(run, "plan", &["--by", "human"]);
        assert_eq!(r.code, 0, "{}", r.all());
    }

    /// 走到 `stop`（`動作:段`）出現為止，回傳那個動作。
    fn walk_to(&self, run: &str, stop: &str, sha: &str) -> Value {
        for _ in 0..30 {
            let a = self.next(run, &[]);
            let (action, stage) = (a["action"].as_str().unwrap_or_default().to_string(), a["stage"].as_str().unwrap_or_default().to_string());
            if format!("{action}:{stage}") == stop {
                return a;
            }
            match action.as_str() {
                "run_workflow" => self.done(run, &stage, sha),
                "gate" => {
                    let r = self.approve(run, &stage, &["--by", "human"]);
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
                other => panic!("沒預期的動作 {other}（要走到 {stop}）：{a}"),
            }
        }
        panic!("走了 30 步還沒到 {stop}");
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// A1：人把每一題 blocking 都答了就往下走。
#[test]
fn a1_answers_move_a_needs_decision_stage_forward() {
    let p = Proj::new("a1");
    let run = "_tpm/runs/r1";
    p.start(run);
    p.next(run, &[]);
    let asked = ask(p.result(run, "plan", ""), "intent", vec![question("匯出要不要含已刪除的訂單？", "A", None), question("檔名用什麼格式？", "B", None)]);
    let r = p.done_with(run, "plan", &asked);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(p.run_json(run)["stages"]["plan"]["status"], "needs_decision");

    // halted：列出每一題的序號與答覆的指令；retry 只是一行指令，不是可以直接拿去跑的動作
    let h = p.next(run, &[]);
    assert_eq!((h["action"].as_str(), h["status"].as_str()), (Some("halted"), Some("needs_decision")), "{h}");
    assert_eq!(h["blocking"].as_array().map(Vec::len), Some(2), "{h}");
    assert_eq!(h["blocking"][1]["id"], "#2");
    assert!(h["answer"].as_str().is_some_and(|a| a.contains("awf approve") && a.contains("#1=") && a.contains("#2=")), "{h}");
    assert!(h["retry"].as_str().is_some_and(|x| x.ends_with("--retry")), "retry 要是指令字串：{h}");

    // 沒帶答覆：照舊拒絕；少答一題、序號對不上、原文對不上、政策核准：都拒絕，什麼都不記
    let r = p.approve(run, "plan", &["--by", "human"]);
    assert!(r.code != 0 && r.all().contains("只有 done 的段能核准"), "{}", r.all());
    let r = p.approve(run, "plan", &["--by", "human", "--answer", "#1=不含"]);
    assert!(r.code != 0 && r.all().contains("還缺") && r.all().contains("#2"), "{}", r.all());
    let r = p.approve(run, "plan", &["--by", "human", "--answer", "#1=不含", "--answer", "#2=日期", "--answer", "#3=多的"]);
    assert!(r.code != 0 && r.all().contains("對不到問題"), "{}", r.all());
    let r = p.approve(run, "plan", &["--by", "human", "--answer", "#1=不含", "--answer", "#2=日期", "--answer", "沒問過的題目=x"]);
    assert!(r.code != 0 && r.all().contains("對不到這一段的任何問題"), "{}", r.all());
    let r = p.approve(run, "plan", &["--by", "policy", "--note", "x", "--answer", "#1=不含", "--answer", "#2=日期"]);
    assert!(r.code != 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!(d["stages"]["plan"]["status"], "needs_decision");
    assert!(d["stages"]["plan"].get("approved").is_none() && d["stages"]["plan"].get("decided").is_none(), "{d}");
    assert!(!p.dir.join("_tpm/runs/decisions.jsonl").exists(), "被拒的答覆不能進決策記憶");

    // 每一題都答（序號與原文都可以）：視為人工核准；原始信封不改
    let before = std::fs::read_to_string(p.dir.join(run).join("plan.json")).unwrap();
    let r = p.approve(run, "plan", &["--by", "human", "--answer", "#1=不含已刪除的", "--answer", "檔名用什麼格式？=export-日期.csv"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.all().contains("視為人工核准"), "{}", r.all());
    assert_eq!(std::fs::read_to_string(p.dir.join(run).join("plan.json")).unwrap(), before, "結果檔不能被改寫");
    let d = p.run_json(run);
    let st = &d["stages"]["plan"];
    assert_eq!((st["status"].as_str(), st["approved"]["kind"].as_str(), st["decided"]["by"].as_str()), (Some("done"), Some("human"), Some("human")), "{st}");
    assert_eq!(st["decided"]["answers"].as_array().map(Vec::len), Some(2));
    assert_eq!(st["decided"]["answers"][0]["answer"], "不含已刪除的");
    assert_eq!(d["recipe"]["locked"], true, "plan 核准照樣鎖定 recipe");
    let mem = std::fs::read_to_string(p.dir.join("_tpm/runs/decisions.jsonl")).unwrap();
    assert_eq!(mem.lines().count(), 2, "{mem}");
    assert!(mem.contains("匯出要不要含已刪除的訂單？") && mem.contains("\"by\": \"human\""), "{mem}");

    // 下一段：提示有哪幾題是靠答覆往下走的，資料包帶上答覆
    let a = p.next(run, &[]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("build")), "{a}");
    assert!(a["notes"].to_string().contains("靠答覆往下走"), "{a}");
    let pack = std::fs::read_to_string(p.dir.join(run).join("build/pack.md")).unwrap();
    assert!(pack.contains("不含已刪除的") && pack.contains("export-日期.csv"), "資料包要帶答覆：\n{pack}");
    // 這種段之後不能政策核准
    let r = p.awf(&["save-run", "approve", run, "plan", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("靠答覆往下走"), "{}", r.all());

    // 不是只差問題沒答（有 blocker 發現）：這條路不適用
    let run2 = "_tpm/runs/r2";
    p.plan_approved(run2);
    p.walk_to(run2, "run_workflow:deliver", SHA);
    let mut bad = ask(p.result(run2, "deliver", SHA), "review", vec![question("要不要一起改？", "A", Some("low"))]);
    bad["review"]["payload"]["findings"] = json!([{"id": "f1", "severity": "blocker", "claim": "c", "evidence": [{"source": "a:1", "result": "r"}], "confidence": 0.9, "suggestion": "s"}]);
    p.done_with(run2, "deliver", &bad);
    assert_eq!(p.run_json(run2)["stages"]["deliver"]["status"], "needs_decision", "{}", p.run_json(run2)["stages"]["deliver"]);
    let h = p.next(run2, &[]);
    assert!(h.get("answer").is_none(), "有 blocker 發現時不給答覆的指令：{h}");
    let r = p.approve(run2, "deliver", &["--by", "human", "--answer", "#1=改"]);
    assert!(r.code != 0 && r.all().contains("不是只差問題沒答"), "{}", r.all());
}

/// A3：直接存檔的結果和 workflow 實際回傳的不一致 → 標成非標準。
#[test]
fn a3_edited_result_is_marked_nonstandard() {
    let p = Proj::new("a3");
    let run = "_tpm/runs/r1";
    p.start(run);
    let a = p.next(run, &[]);
    let asked = ask(p.result(run, "plan", ""), "intent", vec![question("要不要含已刪除的？", "A", None)]);
    p.record("wf_1", a["script"].as_str().unwrap(), &asked);
    let r = p.awf(&["done", "--run", run, "--no-next"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!(d["stages"]["plan"]["status"], "needs_decision");
    assert_eq!(d["stages"]["plan"]["wf_run"]["id"], "wf_1");
    let file = p.dir.join(run).join("plan.json");
    assert!(load(&file).get("manual_output").is_none());

    // 原樣重存：不標記
    let same = std::fs::read_to_string(&file).unwrap();
    let r = p.awf_in(&["save-run", "stage", run, "plan"], Some(&same));
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(!r.all().contains("不一致") && load(&file).get("edited_output").is_none(), "{}", r.all());

    // 把 status 改成 done、blocking 改成 false 再存：照樣存、狀態是 done，但標成非標準
    let mut edited = load(&file);
    edited["intent"]["status"] = json!("done");
    edited["intent"]["open_questions"][0]["blocking"] = json!(false);
    let r = p.awf_in(&["save-run", "stage", run, "plan"], Some(&edited.to_string()));
    assert_eq!(r.code, 0, "原本能存的結果還是要能存：{}", r.all());
    assert!(r.all().contains("和 workflow 實際回傳的不一致") && r.all().contains("intent."), "{}", r.all());
    assert_eq!(p.run_json(run)["stages"]["plan"]["status"], "done");
    assert!(load(&file)["edited_output"].as_array().is_some_and(|l| !l.is_empty()), "{}", load(&file));
    let r = p.awf(&["save-run", "approve", run, "plan", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("和 workflow 實際回傳的不一致"), "{}", r.all());

    // 標過非標準之後，把標記拿掉再存也拿不掉
    let mut stripped = load(&file);
    stripped.as_object_mut().unwrap().remove("edited_output");
    let back: Value = serde_json::from_str(&same).unwrap();
    let _ = stripped;
    let r = p.awf_in(&["save-run", "stage", run, "plan"], Some(&back.to_string()));
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(load(&file)["edited_output"].is_array(), "上一份標過非標準，直接重存不會變回標準：{}", r.all());
}

/// A4：build 續跑時略過子任務。
#[test]
fn a4_skip_subtask_on_build_retry() {
    let p = Proj::new("a4");
    let run = "_tpm/runs/r1";
    p.plan_approved(run);
    // 還沒停下：不能略過
    let r = p.next_raw(run, &["--retry", "--skip-subtask", "t3", "--reason", "卡住", "--by", "human"]);
    assert!(r.code != 0 && r.all().contains("只能在 build 段停下"), "{}", r.all());
    p.next(run, &[]);
    let mut halted = p.result(run, "build", "");
    halted["halted"] = json!(true);
    halted["halt_reason"] = json!("t2 卡住");
    let r = p.done_with(run, "build", &halted);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(p.run_json(run)["stages"]["build"]["status"], "blocked");

    // 被依賴的不能單獨略過；不存在的 id、沒有理由、沒有 --by、--by policy 都拒絕
    for (extra, why) in [
        (vec!["--retry", "--skip-subtask", "t1", "--reason", "卡住", "--by", "human"], "t2 依賴被略過的 t1"),
        (vec!["--retry", "--skip-subtask", "t9", "--reason", "卡住", "--by", "human"], "沒有 t9 這個子任務"),
        (vec!["--retry", "--skip-subtask", "t3", "--by", "human"], "--reason"),
        (vec!["--retry", "--skip-subtask", "t3", "--reason", "卡住"], "--by"),
        (vec!["--retry", "--skip-subtask", "t3", "--reason", "卡住", "--by", "policy"], "由人決定"),
        (vec!["--skip-subtask", "t3", "--reason", "卡住", "--by", "human"], "--retry"),
        (vec!["--retry", "--skip-subtask", "t1", "--skip-subtask", "t2", "--skip-subtask", "t3", "--reason", "卡住", "--by", "human"], "沒有剩下要做的子任務"),
    ] {
        let r = p.next_raw(run, &extra);
        assert!(r.code != 0 && r.all().contains(why), "{extra:?} 應該被拒（{why}）：{}", r.all());
        assert!(p.run_json(run)["awf"].get("skipped_subtasks").is_none(), "被拒時不能留下紀錄");
    }
    // 一起略過就可以：記在 run.json，腳本的子任務只剩 t3
    let a = p.next(run, &["--retry", "--skip-subtask", "t1", "--skip-subtask", "t2", "--reason", "第三方 API 還沒開通", "--by", "human"]);
    assert_eq!((a["action"].as_str(), a["stage"].as_str()), (Some("run_workflow"), Some("build")), "{a}");
    assert!(a["notes"].to_string().contains("略過子任務"), "{a}");
    let d = p.run_json(run);
    let sk = d["awf"]["skipped_subtasks"].as_array().unwrap();
    assert_eq!(sk.len(), 2);
    assert_eq!((sk[0]["id"].as_str(), sk[0]["by"].as_str(), sk[0]["reason"].as_str()), (Some("t1"), Some("human"), Some("第三方 API 還沒開通")));
    let args = load(&p.dir.join(run).join(".wf/sdlc-build.args.json"));
    let ids: Vec<&str> = args["subtasks"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ids, ["t3"]);

    // build 做完：結果帶上略過的子任務；這個 run 的 build 與後面的段都不能政策核准；閘門上列出
    p.done(run, "build", "");
    assert_eq!(load(&p.dir.join(run).join("build.json"))["skipped_subtasks"], json!(["t1", "t2"]));
    let r = p.awf(&["save-run", "approve", run, "build", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("有子任務被略過"), "{}", r.all());
    let g = p.walk_to(run, "gate:test", SHA);
    assert_eq!(g["skipped_subtasks"].as_array().map(Vec::len), Some(2), "閘門要列出略過的子任務：{g}");
    let r = p.awf(&["save-run", "approve", run, "test", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("有子任務被略過"), "{}", r.all());
    let pack = std::fs::read_to_string(p.dir.join(run).join("test/pack.md")).unwrap();
    assert!(pack.contains("t1") && pack.contains("第三方 API 還沒開通"), "後面各段的資料包要列出略過的子任務：\n{pack}");
}

/// A5：重現「沒先跑 awf next --retry 就重跑舊腳本」，並確認只在分得清楚時採用。
#[test]
fn a5_done_after_rerunning_the_old_script() {
    let p = Proj::new("a5");
    let run = "_tpm/runs/r1";
    p.start(run);
    let a = p.next(run, &[]);
    let script = a["script"].as_str().unwrap().to_string();
    let mut halted = p.result(run, "plan", "");
    halted["halted"] = json!(true);
    halted["halt_reason"] = json!("調研缺件");
    p.record("wf_1", &script, &halted);
    let r = p.awf(&["done", "--run", run, "--no-next"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!(d["stages"]["plan"]["status"], "blocked");
    assert!(d["awf"].get("pending").is_none(), "done 之後沒有 pending（根因：之後直接重跑舊腳本，done 就沒有腳本路徑可以對）");

    // halted 的 retry 只給指令（以前是一個帶 script、then 的動作，看起來可以直接執行）
    let h = p.next(run, &[]);
    assert_eq!(h["retry"], format!("awf next --run {run} --retry"), "{h}");
    assert!(h.to_string().matches(".wf/sdlc-plan.js").count() == 0, "halted 動作不該再帶腳本路徑：{h}");

    // 主 session 沒跑 next --retry、直接用舊腳本重跑了一次：沒有 pending，但分得清楚（上次存檔之後恰好一筆）→ 採用，而且是標準結果
    std::thread::sleep(std::time::Duration::from_millis(1100));
    p.record("wf_2", &script, &p.result(run, "plan", ""));
    let r = p.awf(&["done", "--run", run, "--stage", "plan", "--no-next"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!((d["stages"]["plan"]["status"].as_str(), d["stages"]["plan"]["wf_run"]["id"].as_str()), (Some("done"), Some("wf_2")), "{d}");
    assert!(load(&p.dir.join(run).join("plan.json")).get("manual_output").is_none());
    assert!(load(&p.dir.join(run).join("plan.json")).get("edited_script").is_none(), "舊腳本沒被改過（和上一次 next 產生的相同）：退路取得的仍是標準結果");

    // 兩筆以上：不猜，交人
    let run2 = "_tpm/runs/r2";
    p.start(run2);
    let a = p.next(run2, &[]);
    let script = a["script"].as_str().unwrap().to_string();
    let mut halted = p.result(run2, "plan", "");
    halted["halted"] = json!(true);
    p.record("wf_3", &script, &halted);
    assert_eq!(p.awf(&["done", "--run", run2, "--no-next"]).code, 0);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    p.record("wf_4", &script, &p.result(run2, "plan", ""));
    p.record("wf_5", &script, &p.result(run2, "plan", ""));
    let r = p.awf(&["done", "--run", run2, "--stage", "plan", "--no-next"]);
    assert!(r.code != 0 && r.all().contains("分不出哪一筆"), "{}", r.all());
    assert_eq!(p.run_json(run2)["stages"]["plan"]["status"], "blocked");
    // 已經 done 的段沒有 pending 時不走退路
    let r = p.awf(&["done", "--run", run, "--stage", "plan", "--no-next"]);
    assert!(r.code != 0 && r.all().contains("找不到這一段 workflow 的回傳"), "{}", r.all());
}

/// A6：`awf next` 產生的腳本被手改之後才執行 → 非標準結果：政策核准被拒、人可以核准；重新產生腳本照跑才回到標準結果。
#[test]
fn a6_edited_script_is_marked_nonstandard() {
    let p = Proj::new("a6");
    let run = "_tpm/runs/r1";
    p.plan_approved(run);
    let a = p.walk_to(run, "run_workflow:test", SHA);
    let script = a["script"].as_str().unwrap().to_string();
    let sha = p.run_json(run)["awf"]["pending"]["script_sha1"].as_str().expect("next 要記下腳本的雜湊").to_string();
    assert_eq!(sha.len(), 40);
    // 主 session 手改腳本再執行
    let file = p.dir.join(&script);
    let made = std::fs::read_to_string(&file).unwrap();
    put(&file, &format!("{made}\n// 手改：少派一個 agent\n"));
    p.record("wf_t1", &script, &p.result(run, "test", SHA));
    let r = p.awf(&["done", "--run", run, "--no-next"]);
    assert_eq!(r.code, 0, "改過的腳本跑出來的結果照樣能存：{}", r.all());
    assert!(r.all().contains("被改過") && r.all().contains("awf next --retry"), "{}", r.all());
    let saved = load(&p.dir.join(run).join("test.json"));
    assert!(saved["edited_script"]["why"].as_str().is_some_and(|w| w.contains("腳本檔的內容")), "{saved}");
    assert!(p.run_json(run)["awf"]["nonstandard"]["test"]["edited"].is_object());
    // 閘門：說明與重點都列出；政策核准被拒、人可以核准
    let g = p.next(run, &[]);
    assert_eq!((g["action"].as_str(), g["stage"].as_str()), (Some("gate"), Some("test")), "{g}");
    assert!(g["highlights"]["edited_script"].is_object() && g["say"].as_str().is_some_and(|x| x.contains("被改過")), "{g}");
    let r = p.awf(&["save-run", "approve", run, "test", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("被改過"), "{}", r.all());
    assert!(p.run_json(run)["stages"]["test"].get("approved").is_none());
    // 把標記從結果檔拿掉再直接存：拿不掉
    let mut stripped = saved.clone();
    stripped.as_object_mut().unwrap().remove("edited_script");
    let r = p.awf_in(&["save-run", "stage", run, "test"], Some(&stripped.to_string()));
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(load(&p.dir.join(run).join("test.json"))["edited_script"].is_object(), "直接重存不會把非標準變回標準：{}", r.all());
    // 重新產生腳本、照跑：回到標準結果
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let a = p.next(run, &["--stage", "test", "--workflow", "sdlc-test", "--retry"]);
    assert_eq!(a["action"], "run_workflow", "{a}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), made, "next 重新產生的腳本和原本的相同");
    p.record("wf_t2", &script, &p.result(run, "test", SHA));
    let r = p.awf(&["done", "--run", run, "--no-next"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(!r.all().contains("被改過"), "{}", r.all());
    assert!(load(&p.dir.join(run).join("test.json")).get("edited_script").is_none());
    assert!(p.run_json(run)["awf"]["nonstandard"].get("test").is_none());
    let r = p.approve(run, "test", &["--by", "human"]);
    assert_eq!(r.code, 0, "{}", r.all());

    // 同樣的情況、人直接核准非標準的結果：可以
    let run2 = "_tpm/runs/r2";
    p.plan_approved(run2);
    let a = p.walk_to(run2, "run_workflow:test", SHA);
    let script = a["script"].as_str().unwrap().to_string();
    put(&p.dir.join(&script), "// 整支換掉\n");
    p.record("wf_t3", &script, &p.result(run2, "test", SHA));
    assert_eq!(p.awf(&["done", "--run", run2, "--no-next"]).code, 0);
    assert!(load(&p.dir.join(run2).join("test.json"))["edited_script"].is_object());
    let r = p.awf(&["save-run", "approve", run2, "test", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("被改過"), "{}", r.all());
    let r = p.approve(run2, "test", &["--by", "human"]);
    assert_eq!(r.code, 0, "人可以核准非標準的結果：{}", r.all());
}

/// B：本機合併從 deliver 核准走到結案。
#[test]
fn b_local_merge_to_close() {
    let p = Proj::new("b");
    p.edit_profile(|m| m["conventions"]["merge"] = json!({"mode": "local"}));
    p.put(".gitignore", "_tpm/\n");
    p.put("src/export.py", "def export():\n    return []\n");
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["add", "-A"]);
    p.git(&["commit", "-q", "-m", "init"]);
    p.git(&["switch", "-q", "-c", "feature/export"]);
    p.put("src/export.py", "def export():\n    return [1]\n");
    p.put("tests/test_export.py", "def test_export():\n    assert True\n");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-q", "-m", "fix"]);
    let sha = p.git(&["rev-parse", "HEAD"]);
    p.git(&["switch", "-q", "main"]);

    let run = "_tpm/runs/r1";
    p.plan_approved(run);
    // 走到 merge：不派合併 agent、不要 PR 號，回 manual 請人自己合併
    let m = p.walk_to(run, "manual:merge", &sha);
    assert!(m["what"].as_str().is_some_and(|w| w.contains("本機合併")), "{m}");
    assert!(m["then"].as_str().is_some_and(|t| t.contains("--confirm-merge")), "{m}");
    assert!(m.get("publish").is_none(), "本機合併不走推送：{m}");
    assert!(!p.dir.join(run).join(".wf/sdlc-merge.js").exists(), "不產生合併 agent 的腳本");

    // 還沒合併就確認：不算完成，說明缺什麼
    let r = p.next_raw(run, &["--confirm-merge"]);
    assert!(r.code != 0 && r.all().contains("--by"), "{}", r.all());
    let m = p.next(run, &["--confirm-merge", "--by", "human"]);
    assert_eq!(m["action"], "manual");
    assert!(m["not_done"].to_string().contains("還不在主線"), "{m}");
    assert_eq!(p.run_json(run)["stages"]["merge"]["status"], "pending");
    // 快轉合併沒有合併 commit：不算
    p.git(&["merge", "-q", "--ff-only", "feature/export"]);
    let m = p.next(run, &["--confirm-merge", "--by", "human"]);
    assert!(m["not_done"].to_string().contains("找不到") && m["not_done"].to_string().contains("--no-ff"), "{m}");
    p.git(&["reset", "-q", "--hard", "HEAD~1"]);
    // 凍結旗標有設：不算
    p.git(&["merge", "-q", "--no-ff", "-m", "merge export", "feature/export"]);
    p.put("_tpm/runs/auto-merge.freeze.json", "{\"frozen\": true, \"reason\": \"main 紅了\"}");
    let m = p.next(run, &["--confirm-merge", "--by", "human"]);
    assert!(m["not_done"].to_string().contains("凍結"), "{m}");
    std::fs::remove_file(p.dir.join("_tpm/runs/auto-merge.freeze.json")).unwrap();
    // 還有沒人確認的暫定做法：不算
    let mut d = p.run_json(run);
    d["awf"]["unconfirmed"] = json!([{"stage": "test", "question": "q", "applied": "A"}]);
    put(&p.dir.join(run).join("run.json"), &d.to_string());
    let m = p.next(run, &["--confirm-merge", "--by", "human"]);
    assert!(m["not_done"].to_string().contains("暫定做法"), "{m}");
    let r = p.awf(&["approve", "--run", run, "--confirm-unconfirmed", "--by", "human"]);
    assert_eq!(r.code, 0, "{}", r.all());

    // 人合併好、確認：寫 merge 段的結果與稽核紀錄，接著可以結案
    let done = p.next(run, &["--confirm-merge", "--by", "human"]);
    assert_eq!(done["action"], "done", "{done}");
    let res = load(&p.dir.join(run).join("merge.json"));
    assert_eq!((res["mode"].as_str(), res["merged"].as_bool(), res["reviewed_sha"].as_str()), (Some("local"), Some(true), Some(sha.as_str())), "{res}");
    assert_eq!(res["merge_commit"].as_str().unwrap(), p.git(&["rev-parse", "main"]));
    let audit = std::fs::read_to_string(p.dir.join("_tpm/runs/merge-audit.jsonl")).unwrap();
    assert!(audit.lines().count() == 1 && audit.contains("\"decision\": \"local-merge\"") && audit.contains(&sha), "{audit}");
    assert_eq!(p.run_json(run)["stages"]["merge"]["status"], "done");
    let r = p.awf(&["save-run", "close", run, "--by", "human", "--at", "2026-11-18T00:00:00Z"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(p.dir.join(run).join("archive/結案摘要.md").is_file());

    // 審查之後分支又有改動才合併：合進來的不是審過的那個 commit，不算完成
    let run2 = "_tpm/runs/r2";
    p.git(&["switch", "-q", "feature/export"]);
    p.put("src/export.py", "def export():\n    return [1, 2]\n");
    p.put("tests/test_export.py", "def test_export():\n    assert 1\n");
    p.git(&["commit", "-q", "-am", "fix2"]);
    let reviewed = p.git(&["rev-parse", "HEAD"]);
    p.put("src/export.py", "def export():\n    return [1, 2, 3]\n");
    p.git(&["commit", "-q", "-am", "審查之後才加的"]);
    p.git(&["switch", "-q", "main"]);
    p.git(&["merge", "-q", "--no-ff", "-m", "merge export 2", "feature/export"]);
    p.plan_approved(run2);
    p.walk_to(run2, "manual:merge", &reviewed);
    let m = p.next(run2, &["--confirm-merge", "--by", "human"]);
    assert!(m["not_done"].to_string().contains("不是審過的那個 commit"), "{m}");
    let r = p.awf(&["save-run", "close", run2, "--by", "human", "--at", "2026-11-18T00:00:00Z"]);
    assert!(r.code != 0 && r.all().contains("merge 段還沒 done"), "{}", r.all());

    // auto_merge 與 local 同時設：報錯
    let g = p.dir.join(".claude/teams/guard.config.json");
    let mut cfg = load(&g);
    cfg["auto_merge"]["enabled"] = json!(true);
    put(&g, &cfg.to_string());
    let r = p.next_raw(run2, &[]);
    assert!(r.code != 0 && r.all().contains("不能同時"), "{}", r.all());
}

/// C：照建議先做（預設關閉）。
#[test]
fn c_auto_accept_recommended() {
    let p = Proj::new("c");
    let run = "_tpm/runs/r1";
    p.plan_approved(run);
    p.walk_to(run, "run_workflow:test", SHA);
    let low = ask(p.result(run, "test", SHA), "verifier", vec![question("快取要不要一起清？", "A：一起清", Some("low"))]);
    assert_eq!(p.done_with(run, "test", &low).code, 0);

    // 關閉（沒寫與寫 off）：照舊停下，輸出逐字相同
    let off1 = p.next_raw(run, &[]);
    p.edit_profile(|m| m["decisions"] = json!({"auto_accept": "off"}));
    let off2 = p.next_raw(run, &[]);
    assert_eq!(off1.stdout, off2.stdout);
    assert_eq!(off1.json()["action"], "halted");
    assert!(p.run_json(run)["awf"].get("unconfirmed").is_none());

    // 開啟：照建議記答覆、往下走到閘門；一次列出；不寫進跨 run 的決策記憶
    p.edit_profile(|m| m["decisions"] = json!({"auto_accept": "recommended"}));
    let g = p.next(run, &[]);
    assert_eq!((g["action"].as_str(), g["stage"].as_str()), (Some("gate"), Some("test")), "{g}");
    assert_eq!(g["unconfirmed"][0]["auto"], true, "{g}");
    assert_eq!(g["unconfirmed"][0]["applied"], "A：一起清");
    let d = p.run_json(run);
    let st = &d["stages"]["test"];
    assert_eq!((st["status"].as_str(), st["decided"]["by"].as_str(), st["decided"]["auto"].as_bool()), (Some("done"), Some("auto-recommended"), Some(true)), "{st}");
    assert!(st.get("approved").is_none(), "照建議先做不算核准");
    assert!(!p.dir.join("_tpm/runs/decisions.jsonl").exists(), "照建議先做的答覆不進決策記憶");
    // 不能政策核准；人核准也要明確確認
    let r = p.awf(&["save-run", "approve", run, "test", "--by", "policy", "--at", "2026-11-18T00:00:00Z", "--note", "x"]);
    assert!(r.code != 0 && r.all().contains("沒有人確認過"), "{}", r.all());
    let r = p.approve(run, "test", &["--by", "human"]);
    assert!(r.code != 0 && r.all().contains("--confirm-unconfirmed"), "{}", r.all());

    // 推翻：回到 needs_decision，不會再自動照建議做
    let r = p.awf(&["approve", "--run", run, "--reject-unconfirmed", "test", "--by", "human"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert_eq!(d["stages"]["test"]["status"], "needs_decision");
    assert!(d["awf"].get("unconfirmed").is_none() && d["stages"]["test"].get("decided").is_none(), "{d}");
    assert_eq!(p.next(run, &[])["action"], "halted");
    // 重存（重跑）之後又可以；這次人確認
    assert_eq!(p.done_with(run, "test", &low).code, 0);
    assert_eq!(p.next(run, &[])["action"], "gate");
    let r = p.approve(run, "test", &["--by", "human", "--confirm-unconfirmed"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let d = p.run_json(run);
    assert!(d["awf"].get("unconfirmed").is_none());
    assert_eq!(d["stages"]["test"]["decided"]["confirmed"]["by"], "human");
    assert!(std::fs::read_to_string(p.dir.join("_tpm/runs/decisions.jsonl")).unwrap().contains("快取要不要一起清？"), "人確認之後才進決策記憶");

    // 不適用：沒標 risk（當 high）、標 high、沒有建議、題數超過上限、plan 段、排除的 modifier
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("沒標 risk", vec![question("q1", "A", None)]),
        ("high", vec![question("q1", "A", Some("high"))]),
        ("沒有建議", vec![question("q1", "none", Some("low"))]),
        ("超過上限", (0..6).map(|i| question(&format!("q{i}"), "A", Some("low"))).collect()),
    ];
    for (i, (name, qs)) in cases.into_iter().enumerate() {
        let r = format!("_tpm/runs/c-{i}");
        p.plan_approved(&r);
        p.walk_to(&r, "run_workflow:test", SHA);
        assert_eq!(p.done_with(&r, "test", &ask(p.result(&r, "test", SHA), "verifier", qs)).code, 0);
        assert_eq!(p.next(&r, &[])["action"], "halted", "{name} 不適用照建議先做");
        assert!(p.run_json(&r)["awf"].get("unconfirmed").is_none());
    }
    let r = "_tpm/runs/c-plan";
    p.start(r);
    p.next(r, &[]);
    assert_eq!(p.done_with(r, "plan", &ask(p.result(r, "plan", ""), "intent", vec![question("q", "A", Some("low"))])).code, 0);
    assert_eq!(p.next(r, &[])["action"], "halted", "plan 段一律交人");
    let r = "_tpm/runs/c-mod";
    p.start(r);
    p.next(r, &[]);
    let mut plan = p.result(r, "plan", "");
    plan["recipe"]["modifiers"] = json!(["public_api"]);
    assert_eq!(p.done_with(r, "plan", &plan).code, 0, "plan（public_api）");
    assert_eq!(p.approve(r, "plan", &["--by", "human"]).code, 0);
    p.walk_to(r, "run_workflow:test", SHA);
    assert_eq!(p.done_with(r, "test", &ask(p.result(r, "test", SHA), "verifier", vec![question("q", "A", Some("low"))])).code, 0);
    assert_eq!(p.next(r, &[])["action"], "halted", "帶 public_api 的 recipe 一律交人");
}
