//! `awf save-run --selftest`：在暫存目錄跑 init／stage／approve／escalate／close／abandon，驗證各閘門規則，不碰 repo。
//! 項目名稱、順序、數量和 save_run.py 的 selftest 一樣。

use super::py::{self, pj, pname, pparent, truthy};
use super::*;
use crate::{pack, recipe, schema};
use serde_json::{json, Value};

const FULL_INTENT: &str = "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n";
const DIAG: &str = "## 現象\n匯出空白\n## 重現步驟\n1. 點匯出\n## 預期行為\n有資料\n";
const SPEC: &str = "agent-workflow:dev-spec-author";
const RES: &str = "agent-workflow:tpm-researcher";
const SKEPTIC: &str = "agent-workflow:tpm-skeptic";

/// 判定程式的原始碼（不含這支 selftest）：B4.2 用來確認它們不讀 watcher 的兩個檔。
/// publish 還沒移植完，等它移植時在自己的 selftest 檢查。
const SOURCES: [&str; 11] = [
    include_str!("speccheck.rs"),
    include_str!("links.rs"),
    include_str!("mod.rs"),
    include_str!("cli.rs"),
    include_str!("policy.rs"),
    include_str!("trace.rs"),
    include_str!("checks.rs"),
    include_str!("decisions.rs"),
    include_str!("archive.rs"),
    include_str!("py.rs"),
    include_str!("../merge_gate/mod.rs"),
];

fn put(path: &str, text: &str) {
    let _ = std::fs::create_dir_all(pparent(path));
    let _ = std::fs::write(path, text.as_bytes());
}

fn slurp(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

fn is_file(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// `{**base, **over}`
fn with(base: &Value, over: Value) -> Value {
    let mut m = base.as_object().cloned().unwrap_or_default();
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        m.insert(k, v);
    }
    Value::Object(m)
}

fn env(payload: Value) -> Value {
    env_with(payload, json!([]))
}

fn env_with(payload: Value, artifacts: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": artifacts, "evidence": [], "confidence": 0.9,
           "open_questions": [], "derived_work": [], "payload": payload})
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(py::s).collect()).unwrap_or_default()
}

fn has(list: &[String], needle: &str) -> bool {
    list.iter().any(|m| m.contains(needle))
}

struct S {
    ctx: Ctx,
    proj: String,
    epic: String,
    n: usize,
    default_intent: String,
    design: Value,
    build: Value,
    test: Value,
}

impl S {
    fn call(&self, a: &[&str], stdin: Option<&Value>) -> (i32, String) {
        let argv: Vec<String> = a.iter().map(|x| x.to_string()).collect();
        let text = stdin.map(|v| recipe::dumps(v, None)).unwrap_or_default();
        let (code, out, err) = run_captured(&argv, &text, &self.ctx);
        (code, out + &err)
    }

    fn write_intent(&self, text: &str, name: &str) -> String {
        put(&pj(&self.epic, &format!("specs/e/{name}")), text);
        format!("specs/e/{name}")
    }

    fn plan_res(&self, rec: Option<Value>, path: Option<&str>) -> Value {
        let mut r = json!({"stage": "plan", "epic_dir": self.epic,
            "intent": env(json!({"documents": [{"path": path.unwrap_or(&self.default_intent), "kind": "intent", "audience": "owner"}]}))});
        if let (Some(rec), Some(m)) = (rec, r.as_object_mut()) {
            m.insert("recipe".into(), rec);
        }
        r
    }

    fn new_run(&mut self, rec: Option<Value>, approve_plan: bool, intent: Option<&str>) -> String {
        self.n += 1;
        let d = format!("{}/_runs/r{}", self.proj, self.n);
        self.call(&["init", &d, "--event", "sdlc", "--target", "x", "--requirement", "r"], None);
        if rec.is_some() || approve_plan {
            self.call(&["stage", &d, "plan"], Some(&self.plan_res(rec, intent)));
            if approve_plan {
                self.call(&["approve", &d, "plan", "--by", "human", "--at", "t"], None);
            }
        }
        d
    }

    fn load(&self, d: &str) -> Value {
        load(&self.ctx, d).unwrap_or(Value::Null)
    }

    fn dump(&self, d: &str, data: &Value) {
        let _ = dump(&self.ctx, d, data);
    }

    fn stages(&self, d: &str) -> Value {
        self.load(d)["stages"].clone()
    }

    fn walk(&self, d: &str, upto: &str, skip_plan: bool) {
        let steps = [
            ("plan", self.plan_res(None, None)),
            ("design", json!({"design": self.design})),
            ("build", json!({"x": self.build})),
            ("sync", json!({"x": self.build})),
            ("test", json!({"v": self.test})),
            ("deliver", json!({"x": self.build})),
        ];
        for (st, r) in steps {
            if st == upto {
                return;
            }
            if (skip_plan && st == "plan") || self.stages(d)[st]["status"] == "skipped" {
                continue;
            }
            let r = if st == "plan" { r } else { self.traced(d, st, r) };
            self.call(&["stage", d, st], Some(&r));
            if GATED.contains(&st) {
                self.call(&["approve", d, st, "--by", "human", "--at", "t"], None);
            }
        }
    }

    /// 有 recipe 的 run：回傳補上 run.json 的 recipe 與一筆 trace（紀錄檔已寫），模擬新版 workflow 的回傳。
    fn traced(&self, d: &str, st: &str, r: Value) -> Value {
        let data = self.load(d);
        if !has_recipe(&data).unwrap_or(false) {
            return r;
        }
        let rec = &data["recipe"];
        let mut out = with(&r, json!({"recipe": {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]}}));
        if out.get("trace").is_none() {
            let f = format!("{d}/{st}/walk.md");
            put(&f, "# 紀錄\n");
            out = with(&out, json!({"trace": [{"label": "walk", "agent_type": "x", "phase": "p", "record": f, "ok": true}]}));
        }
        out
    }

    fn finish(&self, d: &str) {
        self.walk(d, "merge", true);
        let mut data = self.load(d);
        if let Some(m) = data.get_mut("stages").and_then(Value::as_object_mut) {
            m.insert("merge".into(), json!({"status": "done", "gate": false, "envelopes": 1}));
        }
        self.dump(d, &data);
    }

    fn snap(&self, d: &str) -> (Value, Vec<String>) {
        let (c, o) = self.call(&["status", d, "--json"], None);
        let j = if c == 0 { py::loads(&o).unwrap_or_else(|_| json!({})) } else { json!({}) };
        let errs = schema::Schemas::new(self.ctx.abs(&self.ctx.teams)).check_doc(&j, "run-status").unwrap_or_else(|e| vec![e]);
        (j, errs)
    }

    fn design_run(&mut self, des: &Value, ver: &Value, table: Option<&str>) -> (String, i32, String) {
        let d = self.new_run(None, false, None);
        self.walk(&d, "design", false);
        let g = format!("{d}/design");
        let extra = table.map(|t| format!("\n## 限制對照\n\n| 限制 | 判定 | 依據 |\n|---|---|---|\n{t}\n")).unwrap_or_default();
        put(&format!("{g}/outline.md"), &format!("# 紀錄\n{extra}"));
        put(&format!("{g}/challenge.md"), "# 紀錄\n");
        let tr = json!([
            {"label": "outline", "agent_type": SPEC, "phase": "Outline", "record": format!("{g}/outline.md"), "ok": true},
            {"label": "challenge", "agent_type": SKEPTIC, "phase": "Challenge", "record": format!("{g}/challenge.md"), "ok": true}]);
        let (c, o) = self.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": tr})));
        (d, c, o)
    }

    fn k_run(&mut self) -> String {
        self.n += 1;
        let d = format!("{}/_runs/k{}", self.proj, self.n);
        self.call(&["init", &d, "--event", "sdlc", "--target", &format!("{}/repo", self.proj), "--requirement", "r"], None);
        d
    }

    fn k_stage(&self, d: &str, trace: Value, extra: Value) -> (i32, String) {
        let base = self.plan_res(Some(json!({"name": "feature", "modifiers": [], "plan_mode": "feature"})), None);
        self.call(&["stage", d, "plan"], Some(&with(&with(&base, json!({"trace": trace})), extra)))
    }
}

fn k_entry(d: &str, label: &str, agent: &str, tier: &str, model: &str) -> Value {
    let f = format!("{d}/plan/{}.md", label.replace(':', "-"));
    put(&f, "# 紀錄\n");
    json!({"label": label, "agent_type": agent, "phase": "p", "record": f, "ok": true, "model": model, "tier": tier, "reason": "x"})
}

/// 子任務（B5 用）。
fn task(id: &str, deps: &[&str], acc: &[&str], kind: &str, owns: &[&str]) -> Value {
    json!({"id": id, "title": id, "kind": kind, "owns": owns, "depends_on": deps, "acceptance": acc})
}

fn wrap(subs: Value) -> Value {
    json!({"stage": "design", "design": {"status": "done", "payload": {"epic": "demo", "subtasks": subs}}})
}

/// 跑自我測試，結果寫進 `out`；全部通過回 0，否則 1。`ctx` 只用它的 schema 目錄與資料包的專案根目錄。
pub fn selftest(ctx: &Ctx, out: &mut String) -> i32 {
    let mut ok_all = true;
    macro_rules! check {
        ($name:expr, $cond:expr) => {
            check!($name, $cond, String::new())
        };
        ($name:expr, $cond:expr, $detail:expr) => {{
            let cond: bool = $cond;
            let tail = if cond {
                String::new()
            } else {
                let detail: String = $detail;
                let chars: Vec<char> = detail.chars().collect();
                format!("：{}", chars.iter().skip(chars.len().saturating_sub(400)).collect::<String>())
            };
            out.push_str(&format!("  [{}] {}{tail}\n", if cond { "通過" } else { "失敗" }, $name));
            ok_all = ok_all && cond;
        }};
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("awf-save-run.{}{nanos}", std::process::id()));
    let tmp = tmp.to_string_lossy().replace('\\', "/");
    let tmp = crate::kb::pyish::resolve(&tmp).unwrap_or(tmp);
    let proj = format!("{tmp}/proj");
    let _ = std::fs::create_dir_all(format!("{proj}/.claude/teams"));
    // 專案 profile 用內建 recipes；selftest 把「工具所在專案的 profile」指到這份
    let profile_path = format!("{proj}/.claude/teams/methodology.json");
    put(&profile_path, &recipe::dumps(&json!({"recipes": recipe::builtin_recipes(), "modifiers": recipe::builtin_modifiers()}), None));
    let epic = format!("{proj}/repo/.worktrees/e");
    let _ = std::fs::create_dir_all(format!("{epic}/specs/e"));
    let design = env(json!({"epic": "e", "subtasks": [{"id": "t", "title": "t", "kind": "code", "owns": ["a"], "depends_on": [], "acceptance": ["x"]}]}));
    let build = env(json!({"task_id": "t", "branch": "b", "commits": [], "files_changed": [], "tests": []}));
    let test = env(json!({"commit": "a".repeat(40), "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
        "criteria": [{"id": "C1", "criterion": "c", "command": "x", "result": "pass", "evidence": "e"}]}));
    let refuted = env(json!({"verdicts": [{"target_id": "x", "refuted": true, "reason": "缺資料模型"}]}));
    let inner = Ctx { teams: ctx.abs(&ctx.teams), profile: profile_path.clone(), pack_root: ctx.pack_root.clone(), cwd: ctx.cwd.clone(), today: ctx.today.clone() };
    let mut s = S { ctx: inner, proj: proj.clone(), epic: epic.clone(), n: 0, default_intent: String::new(), design: design.clone(), build: build.clone(), test: test.clone() };
    s.default_intent = s.write_intent(FULL_INTENT, "intent.md");
    let human = ["--by", "human", "--at", "t"];
    let policy = ["--by", "policy", "--at", "t", "--note", "p"];
    macro_rules! approve {
        ($d:expr, $st:expr, $who:expr) => {{
            let mut a: Vec<&str> = vec!["approve", $d, $st];
            a.extend($who);
            s.call(&a, None)
        }};
    }
    let cx = s.ctx.clone();

    out.push_str("save_run selftest（暫存目錄，不碰 repo）\n");
    // 重存清核准
    let d = s.new_run(None, false, None);
    s.walk(&d, "test", false);
    s.call(&["stage", &d, "test"], Some(&json!({"v": test})));
    let (c1, _) = approve!(&d, "test", human);
    let (c2, o) = s.call(&["stage", &d, "test"], Some(&json!({"v": test})));
    check!("重存清核准：重存 test 後舊核准失效，並在輸出說明", c1 == 0 && c2 == 0 && !truthy(&s.stages(&d)["test"]["approved"]) && o.contains("已失效"), o);
    // 零信封
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&json!({"intent": null})));
    let (c2, o2) = approve!(&d, "plan", human);
    check!("零信封：記成 invalid 而且不能核准", c != 0 && s.stages(&d)["plan"]["status"] == "invalid" && o.contains(NO_ENVELOPE) && c2 != 0, o + &o2);
    // skeptic 推翻
    let d = s.new_run(None, false, None);
    s.walk(&d, "design", false);
    s.call(&["stage", &d, "design"], Some(&json!({"design": design, "analysis": [refuted]})));
    let (c, o) = approve!(&d, "design", policy);
    let d2 = s.new_run(None, false, None);
    s.walk(&d2, "design", false);
    s.call(&["stage", &d2, "design"], Some(&json!({"design": design, "analysis": [null]})));
    let (c2, o2) = approve!(&d2, "design", policy);
    let d3 = s.new_run(None, false, None);
    s.walk(&d3, "design", false);
    let stands = env(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "成立"}]}));
    s.call(&["stage", &d3, "design"], Some(&json!({"design": design, "analysis": [stands]})));
    let (c3, o3) = approve!(&d3, "design", policy);
    check!("skeptic 推翻：refuted 與 skeptic 缺件（null）都不能政策核准，全部成立才行", c != 0 && o.contains("推翻") && c2 != 0 && o2.contains("缺件") && c3 == 0, o + &o2 + &o3);
    let viol = policy_violations(&cx, "plan", &json!({"intent": design, "challenge": null}), None).unwrap_or_default();
    check!("skeptic 推翻：challenge: null 判為 agent 缺件", viol.iter().any(|v| v.contains("challenge") && v.contains("缺件")), format!("{viol:?}"));
    // plan 禁政策
    let d = s.new_run(None, false, None);
    s.call(&["stage", &d, "plan"], Some(&s.plan_res(None, None)));
    let (c, o) = approve!(&d, "plan", policy);
    let (c2, _) = approve!(&d, "plan", human);
    check!("plan 禁政策：plan 段 --by policy 拒絕、人工核准可以", c != 0 && o.contains("由人核准") && c2 == 0, o);
    // 閘門鏈
    let d = s.new_run(None, false, None);
    s.call(&["stage", &d, "plan"], Some(&s.plan_res(None, None)));
    approve!(&d, "plan", human);
    s.call(&["stage", &d, "design"], Some(&json!({"design": design})));
    for (st, r) in [("build", json!({"x": build})), ("sync", json!({"x": build})), ("test", json!({"v": test}))] {
        s.call(&["stage", &d, st], Some(&r));
    }
    let (c, o) = approve!(&d, "test", human);
    approve!(&d, "design", human);
    let (c2, o2) = approve!(&d, "test", human);
    check!("閘門鏈：design 沒核准時 test 不能核准，補核准後可以", c != 0 && o.contains("design") && c2 == 0, o + &o2);
    // abandon
    let d = s.new_run(None, false, None);
    s.call(&["stage", &d, "plan"], Some(&s.plan_res(None, None)));
    let (c, _) = s.call(&["abandon", &d, "--reason", "需求取消", "--by", "human", "--at", "t"], None);
    let (c2, o2) = approve!(&d, "plan", human);
    let (_, st_out) = s.call(&["status", &d], None);
    let ab = s.load(&d)["abandoned"].clone();
    let (c3, _) = s.call(&["abandon", &d, "--by", "human", "--at", "t"], None);
    check!(
        "abandon：記錄 {by, at, reason}、之後不能核准、status 顯示已取消、缺 --reason 拒絕",
        c == 0 && ab == json!({"by": "human", "at": "t", "reason": "需求取消"}) && c2 != 0 && o2.contains("已取消") && st_out.contains("已取消") && c3 != 0,
        o2 + &st_out
    );

    // ── B13 決策記憶
    let d = s.new_run(Some(json!({"name": "feature"})), false, None);
    let runs_p = pparent(&d);
    for f in [pack::DECISIONS, pack::DECISIONS_VIEW] {
        let _ = std::fs::remove_file(pj(&runs_p, f));
    }
    let (c, o) = s.call(
        &["approve", &d, "plan", "--by", "human", "--at", "2026-11-12T01:00:00Z", "--answer", "匯出檔的編碼用什麼？=UTF-8 不加 BOM", "--answer", "要不要支援 xlsx＝這一版不要", "--answer-scope", "project", "--answer-expires", "2099-01-01"],
        None,
    );
    let decs = Value::Array(pack::load_decisions(&runs_p));
    check!(
        "B13 決策記憶：人核准時的 --answer 記進 decisions.jsonl（問題、答覆、誰、何時、範圍、到期、run、epic、段）",
        c == 0
            && decs.as_array().map_or(0, Vec::len) == 2
            && decs[0]["question"] == "匯出檔的編碼用什麼？"
            && decs[0]["answer"] == "UTF-8 不加 BOM"
            && decs[1]["answer"] == "這一版不要"
            && decs[0]["by"] == "human"
            && decs[0]["scope"] == "project"
            && decs[0]["expires"] == "2099-01-01"
            && decs[0]["run"] == pname(&d).as_str()
            && decs[0]["stage"] == "plan"
            && decs[0]["epic"] == "e"
            && o.contains("已記 2 筆決定"),
        o + &decs.to_string()
    );
    check!("B13 決策記憶：決策記憶.md 跟著更新（給人與知識庫看）", slurp(&pj(&runs_p, pack::DECISIONS_VIEW)).contains("UTF-8 不加 BOM"));
    let (c, o) = s.call(&["decide", &d, "--question", "匯出檔的編碼用什麼？", "--answer", "改成加 BOM", "--by", "human", "--at", "t", "--scope", "project"], None);
    let rel = |run: &str, req: &str, today: &str| pack::relevant_decisions(&runs_p, run, "", req, today, &[]).unwrap_or_default();
    let got = rel("別的 run", "匯出檔的編碼", "2026-11-12");
    let answers: Vec<String> = got.iter().filter(|x| py::s(&x["question"]).contains("編碼")).map(|x| py::s(&x["answer"])).collect();
    check!("B13 決策記憶：decide 可以在核准以外記一筆；同一題後面的答覆取代前面的；之後的 run 查得到", c == 0 && answers == ["改成加 BOM"], o + &Value::Array(got).to_string());
    let xlsx = |today: &str| rel("x", "要不要支援 xlsx 匯出", today).iter().any(|x| py::s(&x["question"]).contains("xlsx"));
    check!("B13 決策記憶：到期日過了就查不到", !xlsx("2099-06-01") && xlsx("2026-11-12"));
    let n0 = pack::load_decisions(&runs_p).len();
    let d2 = s.new_run(Some(json!({"name": "feature"})), false, None);
    let long_q = "q".repeat(301);
    let bad = [
        s.call(&["approve", &d2, "plan", "--by", "human", "--at", "t", "--answer", "沒有答覆"], None),
        s.call(&["approve", &d2, "plan", "--by", "human", "--at", "t", "--answer", "q=a", "--answer-scope", "everywhere"], None),
        s.call(&["approve", &d2, "plan", "--by", "human", "--at", "t", "--answer", "q=a", "--answer-expires", "明年"], None),
        s.call(&["decide", &d2, "--question", "q", "--answer", "a", "--by", "policy", "--at", "t"], None),
        s.call(&["decide", &d2, "--question", &long_q, "--answer", "a", "--by", "human", "--at", "t"], None),
    ];
    check!(
        "B13 決策記憶：格式不對（沒答覆、範圍或到期日寫錯、太長）與政策核准都拒絕，而且不記、不核准",
        bad.iter().all(|(c, _)| *c != 0) && pack::load_decisions(&runs_p).len() == n0 && !truthy(&s.load(&d2)["stages"]["plan"]["approved"]),
        format!("{:?}", bad.iter().map(|(_, o)| o).collect::<Vec<_>>())
    );
    let d3 = s.new_run(Some(json!({"name": "feature"})), true, None);
    s.call(&["stage", &d3, "design"], Some(&json!({"stage": "design", "recipe": {"name": "feature"}, "design": design, "trace": []})));
    let (c, o) = s.call(&["approve", &d3, "design", "--by", "policy", "--at", "t", "--note", "p", "--answer", "q=a"], None);
    check!("B13 決策記憶：政策核准不能帶 --answer", c != 0 && o.contains("只能由人記") && pack::load_decisions(&runs_p).len() == n0, o);

    // ── SP2.1 intent 檢查（I1）
    let feature = || Some(json!({"name": "feature"}));
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(feature(), Some("specs/e/沒有這個檔.md"))));
    let (c2, o2) = approve!(&d, "plan", human);
    let d2 = s.new_run(None, false, None);
    let no_intent = with(&s.plan_res(feature(), None), json!({"intent": env(json!({"documents": [{"path": "x.md", "kind": "spec", "audience": "owner"}]}))}));
    let (c3, o3) = s.call(&["stage", &d2, "plan"], Some(&no_intent));
    check!(
        "intent 節：缺檔（intent 檔不存在、或沒有 kind=intent 的文件）→ invalid、不能核准",
        c != 0 && s.stages(&d)["plan"]["status"] == "invalid" && o.contains("intent 檔不存在") && c2 != 0 && c3 != 0 && o3.contains("kind=intent"),
        o + &o2 + &o3
    );
    let no_ac = s.write_intent("# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n", "no-ac.md");
    let empty = s.write_intent("# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC-1\n", "empty.md");
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(feature(), Some(&no_ac))));
    let d2 = s.new_run(None, false, None);
    let (c2, o2) = s.call(&["stage", &d2, "plan"], Some(&s.plan_res(feature(), Some(&empty))));
    check!(
        "intent 節：缺必要節（少了驗收條件、或節只有標題）→ invalid",
        c != 0 && o.contains("驗收條件") && s.stages(&d)["plan"]["status"] == "invalid" && c2 != 0 && o2.contains("不做的事（只有標題沒有內容）"),
        o + &o2
    );
    let no_root = s.write_intent(&format!("{FULL_INTENT}{DIAG}"), "no-root.md");
    let with_root = s.write_intent(&format!("{FULL_INTENT}{DIAG}## 根因\n空集合沒處理\n"), "root.md");
    let with_root = with_root.as_str();
    let bugfix = || Some(json!({"name": "bugfix"}));
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(bugfix(), Some(&no_root))));
    let d2 = s.new_run(None, false, None);
    let (c2, o2) = s.call(&["stage", &d2, "plan"], Some(&s.plan_res(bugfix(), Some(with_root))));
    check!(
        "intent 節：diagnose 要根因（bugfix 少了根因 → invalid；補上 → done）",
        c != 0 && o.contains("根因") && o.contains("plan_mode=diagnose") && c2 == 0 && s.stages(&d2)["plan"]["status"] == "done",
        o + &o2
    );
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(feature(), None)));
    let (c2, o2) = approve!(&d, "plan", human);
    check!("intent 節：齊全可核准（四節齊全 → done、人工核准可以）", c == 0 && c2 == 0 && truthy(&s.stages(&d)["plan"]["approved"]), o + &o2);
    let prof = format!("{proj}/custom-profile.json");
    put(&prof, &recipe::dumps(&json!({"stages": {"plan": {"required_sections": ["問題"]}}}), None));
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan", "--profile", &prof], Some(&s.plan_res(feature(), None)));
    let (c2, o2) = s.call(&["stage", &d, "plan", "--profile", &format!("{proj}/沒有.json")], Some(&s.plan_res(feature(), None)));
    check!("intent 節：--profile 覆寫必要節；--profile 讀不到 → invalid（fail-closed）", c != 0 && o.contains("問題") && c2 != 0 && o2.contains("不存在"), o + &o2);

    // ── SP2.2 recipe 鎖定
    let d = s.new_run(Some(json!({"name": "bugfix", "modifiers": [], "size": "small", "reasons": [{"claim": "回報錯誤", "evidence": "issue"}]})), false, Some(with_root));
    let rec0 = s.load(&d)["recipe"].clone();
    let sk = s.stages(&d);
    let ctx_text = slurp(&format!("{d}/context.md"));
    check!(
        "recipe 鎖定：stage plan 記錄 recipe（工具重算 resolved），recipe 沒有的段標 skipped，context.md 有 recipe 摘要",
        rec0["name"] == "bugfix"
            && rec0["locked"] == false
            && rec0["resolved"]["plan_mode"] == "diagnose"
            && sk["design"] == json!({"status": "skipped", "gate": false})
            && ctx_text.contains("- recipe：bugfix")
            && ctx_text.contains("目標 repo：x"),
        py::take(&recipe::dumps(&rec0, None), 300).to_string() + &ctx_text
    );
    let (c, _) = approve!(&d, "plan", human);
    let locked = s.load(&d)["recipe"]["locked"].clone();
    let (c2, o2) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(feature(), Some(with_root))));
    let after = s.load(&d);
    check!(
        "recipe 鎖定：核准後鎖定；再 stage plan 換 recipe → invalid、recipe 不變、核准清除",
        c == 0 && locked == true && c2 != 0 && o2.contains("要改走 awf save-run escalate") && after["recipe"]["name"] == "bugfix" && !truthy(&after["stages"]["plan"]["approved"]),
        o2
    );
    let d = s.new_run(bugfix(), false, Some(with_root));
    let (c, o) = approve!(&d, "plan", policy);
    check!("recipe 鎖定：policy 不能核准 plan（recipe 與 intent 由人在 plan 閘門核准）", c != 0 && o.contains("由人核准") && !truthy(&s.load(&d)["recipe"]["locked"]), o);
    let d = s.new_run(Some(json!({"name": "bugfix", "modifiers": ["zzz"]})), false, Some(with_root));
    check!("recipe 鎖定：不認得的 modifier → plan invalid（交人）", s.stages(&d)["plan"]["status"] == "invalid");

    // ── SP2.3 escalate
    let d = s.new_run(bugfix(), true, Some(with_root));
    let (c, o) = s.call(&["escalate", &d, "--recipe", "feature", "--by", "human", "--at", "t2", "--reason", "其實是新功能"], None);
    let rec = s.load(&d)["recipe"].clone();
    check!(
        "escalate：加嚴（bugfix → feature：補上 design 段、保留 regression、history 記一筆、仍鎖定）",
        c == 0
            && strs(&rec["resolved"]["stages"]).iter().any(|x| x == "design")
            && s.stages(&d)["design"]["status"] == "pending"
            && strs(&rec["resolved"]["test"]["modes"]).iter().any(|x| x == "regression")
            && rec["history"].as_array().map_or(0, Vec::len) == 1
            && rec["history"][0]["from"] == "bugfix"
            && rec["history"][0]["reason"] == "其實是新功能"
            && truthy(&rec["locked"]),
        o
    );
    let d = s.new_run(feature(), true, None);
    let (c, o) = s.call(&["escalate", &d, "--recipe", "bugfix", "--by", "human", "--at", "t2", "--reason", "想省事"], None);
    let (c2, o2) = s.call(&["escalate", &d, "--recipe", "feature", "--modifiers", "zzz", "--by", "human", "--at", "t2", "--reason", "x"], None);
    let (c3, o3) = s.call(&["escalate", &d, "--recipe", "feature", "--by", "human", "--at", "t2"], None);
    let rec = s.load(&d)["recipe"].clone();
    check!(
        "escalate：降級拒絕（feature → bugfix 少了 design；不認得的 modifier、缺 --reason 也拒絕；recipe 不變）",
        c != 0 && o.contains("降級") && c2 != 0 && c3 != 0 && rec["name"] == "feature" && !truthy(&rec["history"]),
        o + &o2 + &o3
    );
    let d = s.new_run(bugfix(), true, Some(with_root));
    s.walk(&d, "deliver", true);
    let before = s.stages(&d);
    let (c, o) = s.call(&["escalate", &d, "--recipe", "bugfix", "--modifiers", "migration", "--by", "human", "--at", "t2", "--reason", "碰到 migration"], None);
    let st = s.stages(&d);
    s.call(&["stage", &d, "deliver"], Some(&json!({"x": build})));
    let (c2, o2) = approve!(&d, "deliver", human);
    let (c3, o3) = approve!(&d, "test", human);
    check!(
        "escalate：後面的段改成 stale（加 design 段 → 已 done 的 build、sync、test 變 stale 並清核准，閘門鏈擋 deliver）",
        c == 0
            && before["test"]["status"] == "done"
            && st["design"]["status"] == "pending"
            && ["build", "sync", "test"].iter().all(|x| st[*x]["status"] == "stale")
            && !truthy(&st["test"]["approved"])
            && truthy(&st["plan"]["approved"])
            && c2 != 0
            && o2.contains("design")
            && c3 != 0,
        o + &o2 + &o3
    );

    // ── SP2.4 閘門鏈依 recipe
    let pop_stage = |data: &mut Value, name: &str| {
        if let Some(m) = data.get_mut("stages").and_then(Value::as_object_mut) {
            m.shift_remove(name);
        }
    };
    let d = s.new_run(bugfix(), true, Some(with_root));
    s.walk(&d, "deliver", true);
    let mut data = s.load(&d);
    pop_stage(&mut data, "test");
    s.dump(&d, &data);
    s.call(&["stage", &d, "deliver"], Some(&json!({"x": build})));
    let (c, o) = approve!(&d, "deliver", human);
    let errs = chain_errors(&data, "deliver").unwrap_or_default();
    check!("閘門鏈依 recipe：缺段（recipe 有 test、run.json 被拿掉 → 判錯，不再略過）", c != 0 && o.contains("缺段") && has(&errs, "test"), o);
    let d = s.new_run(None, false, None);
    s.walk(&d, "test", false);
    let mut data = s.load(&d);
    pop_stage(&mut data, "sync");
    s.dump(&d, &data);
    s.call(&["stage", &d, "test"], Some(&json!({"v": test})));
    let (c, o) = approve!(&d, "test", human);
    let mut data = s.load(&d);
    pop_stage(&mut data, "design");
    let legacy_errs = chain_errors(&data, "test").unwrap_or_default();
    let stages_only = chain_errors(&data["stages"], "test").unwrap_or_default();
    check!(
        "閘門鏈依 recipe：舊 run 當成 feature（沒有 recipe：缺 sync 仍容許、缺 design 判錯；只給 stages 也一樣）",
        data.get("recipe").is_none() && c == 0 && has(&legacy_errs, "design") && legacy_errs == stages_only,
        o + &format!("{legacy_errs:?}")
    );
    let d = s.new_run(bugfix(), true, Some(with_root));
    s.walk(&d, "test", true);
    s.call(&["stage", &d, "test"], Some(&s.traced(&d, "test", json!({"v": test}))));
    let (c, o) = approve!(&d, "test", human);
    let (c2, o2) = s.call(&["stage", &d, "design"], Some(&s.traced(&d, "design", json!({"design": design}))));
    check!(
        "閘門鏈依 recipe：bugfix 不需要 design（design 是 skipped 也能核准 test；硬存 design 段判 invalid）",
        c == 0 && s.stages(&d)["design"]["status"] == "skipped" && c2 != 0 && o2.contains("沒有 design 段"),
        o + &o2
    );

    // ── SP2.5 執行紀錄索引（I6）
    let d = s.new_run(None, false, None);
    s.walk(&d, "design", false);
    let rdir = format!("{d}/design");
    put(&format!("{rdir}/outline.md"), "# 紀錄\n");
    put(&format!("{rdir}/challenge.md"), "# 紀錄\n");
    let trace = json!([
        {"label": "outline", "agent_type": SPEC, "phase": "Outline", "record": format!("{rdir}/outline.md"), "ok": true},
        {"label": "challenge", "agent_type": SKEPTIC, "phase": "Challenge", "record": format!("{rdir}/challenge.md"), "ok": true}]);
    let des = env_with(design["payload"].clone(), json!([format!("{rdir}/outline.md"), "specs/e/design.md"]));
    let ver = env(json!({"verdicts": [{"target_id": "x", "refuted": false, "reason": "成立"}]}));
    let (c, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": trace})));
    let idx = slurp(&format!("{rdir}/{INDEX_NAME}"));
    let (c2, o2) = approve!(&d, "design", policy);
    check!(
        "執行紀錄索引：產生（每個 trace 一列：label、角色、phase、紀錄檔、成果檔、狀態；紀錄齊全可政策核准）",
        c == 0
            && idx.contains("| label | 角色 | phase | 紀錄檔 | 成果檔 | 狀態 |")
            && idx.matches("| outline |").count() == 1
            && idx.contains("| challenge |")
            && idx.contains("specs/e/design.md")
            && s.stages(&d)["design"].get("missing_records").is_none()
            && c2 == 0,
        o + &idx + &o2
    );
    // B7 限制文件：文件不存在、缺對照列、有偏離、檢查指令沒過 → 不能政策核准；都符合才行；沒有 constraints 的 profile 行為不變
    let keep_profile = slurp(&profile_path);
    let cbase = py::loads(&keep_profile).unwrap_or(Value::Null);
    let (ok_cmd, bad_cmd) = ("exit 0", "exit 3");
    let with_constraints = |cons: Value| put(&profile_path, &recipe::dumps(&with(&cbase, json!({"constraints": cons})), None));
    put(&format!("{proj}/ARCHITECTURE.md"), "# 架構\n\n分層：api → service → repository。\n");
    let arch = json!({"id": "arch", "file": "ARCHITECTURE.md", "stages": ["design"], "trace": "table"});
    with_constraints(json!([arch]));
    let (d, c, o) = s.design_run(&des, &ver, Some("| arch | 符合 | design.md:12 |"));
    let (c2, o2) = approve!(&d, "design", policy);
    check!("B7 限制文件：文件在、對照列是「符合」→ 可以政策核准", c == 0 && s.stages(&d)["design"].get("constraint_issues").is_none() && c2 == 0, o + &o2);
    let (d, _, o) = s.design_run(&des, &ver, None);
    let (c2, o2) = approve!(&d, "design", policy);
    let (c3, o3) = approve!(&d, "design", human);
    check!(
        "B7 限制文件：沒有對照列 → 記 constraint_issues、政策核准被擋；人工核准照記並警告",
        has(&strs(&s.stages(&d)["design"]["constraint_issues"]), "沒有它的〈限制對照〉列") && c2 != 0 && o2.contains("限制文件") && c3 == 0 && o3.contains("限制文件"),
        o + &o2 + &o3
    );
    let (d, _, o) = s.design_run(&des, &ver, Some("| arch | 偏離 | 這次要讓 api 直接讀 repository，理由見 design.md:40 |"));
    let (c2, o2) = approve!(&d, "design", policy);
    check!("B7 限制文件：對照列是「偏離」→ 政策核准被擋（要由人決定）", c2 != 0 && o2.contains("偏離"), o + &o2);
    with_constraints(json!([with(&arch, json!({"file": "docs/NOPE.md"}))]));
    let (d, _, o) = s.design_run(&des, &ver, Some("| arch | 符合 | x |"));
    let (c2, o2) = approve!(&d, "design", policy);
    check!("B7 限制文件：文件不存在 → 政策核准被擋", c2 != 0 && o2.contains("不存在"), o + &o2);
    with_constraints(json!([{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": bad_cmd}]));
    let (d, _, o) = s.design_run(&des, &ver, None);
    let (c2, o2) = approve!(&d, "design", policy);
    let chk = s.stages(&d)["design"]["constraint_checks"].clone();
    check!("B7 限制文件：檢查指令 exit 非 0 → 記結果、政策核准被擋", c2 != 0 && o2.contains("檢查指令沒過") && truthy(&chk) && chk[0]["exit"] == 3, o + &o2 + &chk.to_string());
    with_constraints(json!([{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": ok_cmd}, {"id": "rev", "file": "docs/NOPE.md", "stages": ["deliver"], "trace": "table"}]));
    let (d, _, o) = s.design_run(&des, &ver, None);
    let (c2, o2) = approve!(&d, "design", policy);
    check!("B7 限制文件：檢查指令通過、trace 沒設、別段的限制不算 → 可以政策核准", c2 == 0 && s.stages(&d)["design"].get("constraint_issues").is_none(), o + &o2);
    let fe = json!({"constraints": [{"id": "fe", "file": "ARCHITECTURE.md", "stages": ["build"], "paths": ["web/**"], "trace": "table"}]});
    let ci = constraint_issues(&cx, &d, "build", &fe, None, true).map(|x| x.0).unwrap_or_else(|e| vec![e.msg]);
    check!("B7 限制文件：build 段有 paths 的限制不在這裡要求對照（只發給對得上的子任務）", ci.is_empty(), format!("{ci:?}"));
    put(&profile_path, &keep_profile);
    let (d, c, o) = s.design_run(&des, &ver, None);
    check!("B7 限制文件：profile 沒有 constraints → 行為不變", c == 0 && s.stages(&d)["design"].get("constraint_issues").is_none(), o);
    // ── B5 plan_check：拆工的確定性檢查
    let t = |id: &str| task(id, &[], &["AC-1 通過"], "code", &["src/a.py"]);
    let td = |id: &str, deps: &[&str]| task(id, deps, &["AC-1 通過"], "code", &["src/a.py"]);
    let pci = |r: Value, spec: Option<&str>| plan_check_issues(&cx, &r, spec).unwrap_or_else(|e| vec![format!("例外：{}", e.msg)]);
    check!(
        "B5 plan_check：乾淨的拆工沒有問題；回傳沒有子任務時不檢查",
        pci(wrap(json!([t("t1"), task("t2", &["t1"], &["AC-1 通過"], "code", &["src/b.py"])])), None).is_empty() && pci(json!({"stage": "design"}), None).is_empty()
    );
    let pc = pci(wrap(json!([td("t1", &["t9"]), t("t1"), td("t2", &["t3"]), td("t3", &["t2"])])), None);
    check!("B5 plan_check：id 重複、相依指向不存在、相依循環都列出", has(&pc, "id 重複：t1") && has(&pc, "不存在的子任務：t9") && has(&pc, "循環：t2、t3"), format!("{pc:?}"));
    let pc = pci(wrap(json!([task("t1", &[], &[], "code", &["src/a.py"]), task("t2", &[], &["AC-1 通過"], "code", &[]), task("t3", &[], &["AC-1 通過"], "research", &[])])), None);
    check!(
        "B5 plan_check：沒有驗收條件、會改檔卻沒有 owns 都列出；research 不要求 owns",
        has(&pc, "t1 沒有驗收條件") && has(&pc, "t2（code）沒有 owns") && !has(&pc, "t3"),
        format!("{pc:?}")
    );
    let spec_f = format!("{d}/spec-for-plan-check.md");
    put(&spec_f, "# spec\n- AC-1 可以登入\n- AC-2 可以登出\n- AC-MUT-3 不可變\n");
    let pc = pci(wrap(json!([task("t1", &[], &["AC-1 通過", "AC-9 通過"], "code", &["src/a.py"])])), Some(&spec_f));
    check!(
        "B5 plan_check：spec 有、沒人對應的驗收條件，與子任務引用了 spec 沒有的，都列出",
        has(&pc, "沒有任何子任務對應：AC-2、AC-MUT-3") && has(&pc, "spec 沒有的驗收條件：AC-9"),
        format!("{pc:?}")
    );
    check!(
        "B5 plan_check：子任務沒用 AC 編號（寫測試指令）時不比對；spec 檔不存在也不比對",
        pci(wrap(json!([task("t1", &[], &["pytest tests/test_a.py"], "code", &["src/a.py"])])), Some(&spec_f)).is_empty() && pci(wrap(json!([t("t1")])), Some(&format!("{d}/nope.md"))).is_empty()
    );
    let mut bad = wrap(json!([td("t1", &["t9"])]));
    bad["design"] = with(&bad["design"], json!({"summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": []}));
    let dpc = s.new_run(None, false, None);
    s.walk(&dpc, "design", false);
    let (_, o) = s.call(&["stage", &dpc, "design"], Some(&bad));
    let (c2, o2) = approve!(&dpc, "design", policy);
    check!("B5 plan_check：存檔時記進 run.json，政策核准被擋並說明", has(&strs(&s.stages(&dpc)["design"]["plan_check"]), "t9") && c2 != 0 && o2.contains("拆工檢查"), o + &o2);

    // B9：有交件（digest 對得回信封）但沒寫紀錄檔 → 依信封代寫，不算缺件；agent 自己寫的不覆寫；回傳換掉後舊的代寫檔不沿用
    let d = s.new_run(None, false, None);
    s.walk(&d, "design", false);
    let gd = format!("{d}/design");
    let _ = std::fs::create_dir_all(&gd);
    let digest = |e: &Value| json!({"status": e["status"], "summary": py::take(&py::s(&e["summary"]), 400)});
    let amb = json!([{"label": "challenge", "agent_type": SKEPTIC, "phase": "Challenge", "record": format!("{gd}/challenge.md"), "ok": true, "digest": digest(&ver)}]);
    let (_, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": with(&des, json!({"artifacts": []})), "challenge": ver, "trace": amb})));
    check!(
        "B9 代寫：digest 對到多個不同的信封 → 不猜，仍是缺件",
        des["summary"] == ver["summary"] && truthy(&s.stages(&d)["design"]["missing_records"]) && !exists(&format!("{gd}/challenge.md")),
        o
    );
    let ver = with(&ver, json!({"summary": "skeptic 判定：設計成立，沒有推翻的項目"}));
    put(&format!("{gd}/outline.md"), "# agent 自己寫的紀錄\n");
    let gt = json!([
        {"label": "outline", "agent_type": SPEC, "phase": "Outline", "record": format!("{gd}/outline.md"), "ok": true, "digest": digest(&des)},
        {"label": "challenge", "agent_type": SKEPTIC, "phase": "Challenge", "record": format!("{gd}/challenge.md"), "ok": true, "digest": digest(&ver)}]);
    let (c, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": gt})));
    let gbody = slurp(&format!("{gd}/challenge.md"));
    let gidx = slurp(&format!("{gd}/{INDEX_NAME}"));
    check!(
        "B9 代寫：有信封沒紀錄檔 → 依信封代寫、不算缺件、記 ghostwritten_records；agent 自己寫的紀錄不覆寫",
        c == 0
            && gbody.contains(GHOSTWRITTEN)
            && gbody.contains(&py::s(&ver["summary"]))
            && s.stages(&d)["design"].get("missing_records").is_none()
            && s.stages(&d)["design"]["ghostwritten_records"] == json!(["challenge"])
            && gidx.contains("agent 沒寫紀錄檔")
            && slurp(&format!("{gd}/outline.md")) == "# agent 自己寫的紀錄\n",
        o + &gbody + &gidx
    );
    let first = gbody;
    let (c, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": gt})));
    check!("B9 代寫：重存結果相同（可重現）", c == 0 && slurp(&format!("{gd}/challenge.md")) == first && s.stages(&d)["design"].get("missing_records").is_none(), o);
    let nodg = Value::Array(
        gt.as_array()
            .map(|a| {
                a.iter()
                    .map(|e| {
                        let mut m = e.as_object().cloned().unwrap_or_default();
                        m.shift_remove("digest");
                        Value::Object(m)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    let _ = std::fs::remove_file(format!("{gd}/challenge.md"));
    let (_, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": nodg})));
    check!("B9 代寫：對不回信封（沒有 digest、artifacts 也沒列）→ 仍是缺件", truthy(&s.stages(&d)["design"]["missing_records"]) && !exists(&format!("{gd}/challenge.md")), o);
    put(&format!("{gd}/challenge.md"), &first);
    let wrong = json!([gt[0], with(&gt[1], json!({"digest": {"status": "done", "summary": "別的信封的摘要"}}))]);
    let (_, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": wrong})));
    check!("B9 代寫：舊的代寫檔對不回這次的信封 → 缺件（不沿用）", has(&strs(&s.stages(&d)["design"]["missing_records"]), "代寫"), o);
    let d = s.new_run(None, false, None);
    s.walk(&d, "design", false);
    let mut bad_trace: Vec<Value> = trace.as_array().cloned().unwrap_or_default();
    bad_trace.push(json!({"label": "plan:a", "agent_type": SPEC, "phase": "Plan", "record": format!("{d}/design/plan-a.md"), "ok": true}));
    bad_trace.push(json!({"label": "plan:b", "agent_type": SPEC, "phase": "Plan", "record": format!("{d}/design/plan-b.md"), "ok": false}));
    for f in ["outline.md", "challenge.md"] {
        put(&format!("{d}/design/{f}"), "# 紀錄\n");
    }
    let bad_trace: Vec<Value> = bad_trace.iter().map(|e| with(e, json!({"record": format!("{d}/design/{}", pname(&py::s(&e["record"])))}))).collect();
    let (_, o) = s.call(&["stage", &d, "design"], Some(&json!({"design": des, "challenge": ver, "trace": bad_trace})));
    let miss = strs(&s.stages(&d)["design"]["missing_records"]);
    let (_, st_out) = s.call(&["status", &d], None);
    let idx = slurp(&format!("{d}/design/{INDEX_NAME}"));
    check!(
        "執行紀錄索引：缺紀錄檔（ok=true 但檔不存在 → missing_records、索引標缺件、status 警告；ok=false 也算缺件）",
        miss.len() == 2 && miss[0].contains("plan:a") && miss[1].contains("plan:b") && idx.contains("缺紀錄檔") && idx.contains("agent 沒有交件") && st_out.contains("警告"),
        o + &st_out + &idx
    );
    let (c, o) = approve!(&d, "design", policy);
    let (c2, o2) = approve!(&d, "design", human);
    check!("執行紀錄索引：policy 被擋（缺紀錄檔不能政策核准；人工核准可以但警告）", c != 0 && o.contains("執行紀錄缺件") && c2 == 0 && o2.contains("警告"), o + &o2);
    let d = s.new_run(None, false, None);
    let mut md = env(json!({"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": "a".repeat(40), "traceability": [], "out_of_scope": [],
        "followups": [], "risk_tier": "low", "gate_reasons": [], "merge_state": "merged"}));
    md["evidence"] = json!([{"claim": "gate 判 merge", "source": "merge_gate.py", "result": "exit 0"}]);
    let steward = format!("agent-workflow:{STEWARD}");
    let strace = json!([
        {"label": "analyze", "agent_type": steward, "phase": "Analyze", "record": format!("{d}/merge/analyze.md"), "ok": true},
        {"label": "execute:merge", "agent_type": steward, "phase": "Execute", "record": format!("{d}/merge/execute-merge.md"), "ok": true}]);
    let (_, o) = s.call(&["stage", &d, "merge"], Some(&json!({"stage": "merge", "analysis": md, "execute": md, "trace": strace})));
    let (f1, f2) = (format!("{d}/merge/analyze.md"), format!("{d}/merge/execute-merge.md"));
    let body = slurp(&f1);
    let idx = slurp(&format!("{d}/merge/{INDEX_NAME}"));
    check!(
        "執行紀錄索引：steward 代寫（沒有 Write 的 steward 由工具依信封代寫紀錄檔，不算缺件）",
        is_file(&f1) && is_file(&f2) && body.contains(GHOSTWRITTEN) && body.contains("gate 判 merge") && s.stages(&d)["merge"].get("missing_records").is_none() && idx.contains(GHOSTWRITTEN),
        o + &body + &idx
    );
    let d = s.new_run(None, false, None);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(None, None)));
    check!(
        "執行紀錄索引：回傳沒有 trace（舊 workflow）只警告，不判缺件",
        c == 0 && o.contains("沒有 trace") && !exists(&format!("{d}/plan/{INDEX_NAME}")) && s.stages(&d)["plan"].get("missing_records").is_none(),
        o
    );

    // ── SP2.6 hotfix 後續
    let hotfix = || Some(json!({"name": "hotfix"}));
    let d = s.new_run(hotfix(), true, Some(with_root));
    let (c, o) = s.call(&["close", &d, "--by", "human", "--at", "t"], None);
    let (c2, o2) = s.call(&["followup", &d, "--run", &format!("{proj}/_runs/沒有"), "--by", "human", "--at", "t"], None);
    check!("hotfix 後續：沒有 followup_run 不能 close（後續 run 不存在也不能記錄）", c != 0 && o.contains("followup_run") && c2 != 0 && s.load(&d).get("closed").is_none(), o + &o2);
    let d_follow = s.new_run(None, false, None);
    let (c, o) = s.call(&["followup", &d, "--run", &d_follow, "--by", "human", "--at", "t"], None);
    let (c2, o2) = s.call(&["close", &d, "--by", "human", "--at", "t"], None);
    check!(
        "hotfix 後續：有 followup_run 但流程還沒走完（build 之後都 pending）不能 close",
        c == 0 && c2 != 0 && o2.contains("流程還沒走完") && s.load(&d).get("closed").is_none(),
        o + &o2
    );
    s.finish(&d);
    let (c2, o2) = s.call(&["close", &d, "--by", "human", "--at", "t"], None);
    let data = s.load(&d);
    let d_feat = s.new_run(feature(), true, Some(with_root));
    let (c3, o3) = s.call(&["close", &d_feat, "--by", "human", "--at", "t"], None);
    s.finish(&d_feat);
    let (c4, o4) = s.call(&["close", &d_feat, "--by", "human", "--at", "t"], None);
    check!(
        "hotfix 後續：有 followup_run 可以 close（整條流程也要走完；feature 不需要後續 run，但同樣要走完）",
        c2 == 0 && data["followup_run"]["run"] == d_follow.as_str() && data["closed"] == json!({"by": "human", "at": "t"}) && c3 != 0 && c4 == 0,
        o2 + &o3 + &o4
    );
    // status --json（給外部程式讀的快照）
    let by_name = |j: &Value, name: &str| j["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name).cloned()).unwrap_or(Value::Null);
    let d_wait = s.new_run(bugfix(), false, Some(with_root));
    let (j1, e1) = s.snap(&d_wait);
    check!(
        "status --json：plan 存好還沒核准＝waiting_gate，下一步與等待的閘門都是 plan；recipe 沒有的段是 skipped",
        e1.is_empty()
            && j1["state"] == "waiting_gate"
            && j1["next"] == "plan"
            && j1["waiting_gate"] == "plan"
            && j1["recipe"]["name"] == "bugfix"
            && !truthy(&j1["recipe"]["locked"])
            && by_name(&j1, "design")["status"] == "skipped",
        format!("{e1:?}{j1}")
    );
    approve!(&d_wait, "plan", human);
    s.call(&["stage", &d_wait, "build"], Some(&s.traced(&d_wait, "build", json!({"x": build}))));
    let (j2, e2) = s.snap(&d_wait);
    let b2 = by_name(&j2, "build");
    let j2_next = py::s_or(&j2["next"], "");
    check!(
        "status --json：核准後 running，跳過 design 直接到下一段；段的派工數取自 trace",
        e2.is_empty()
            && truthy(&j2["recipe"]["locked"])
            && j2["waiting_gate"].is_null()
            && !["plan", "design", "build"].iter().any(|x| j2["next"] == *x)
            && b2["agents"].as_i64().unwrap_or(0) >= 1
            && b2["agents_missing"] == 0
            && by_name(&j2, "plan")["approved"] == json!({"by": "human", "at": "t"}),
        format!("{e2:?}{j2}")
    );
    let (j3, e3) = s.snap(&d_feat);
    check!("status --json：結案的 run 是 closed、沒有下一步", e3.is_empty() && j3["state"] == "closed" && j3["next"].is_null(), format!("{e3:?}{j3}"));
    let docs = || Some(json!({"name": "docs"}));
    let d_ab = s.new_run(docs(), false, Some(with_root));
    s.call(&["abandon", &d_ab, "--by", "human", "--at", "t", "--reason", "不做了"], None);
    let (j4, e4) = s.snap(&d_ab);
    check!("status --json：取消的 run 是 abandoned、沒有下一步", e4.is_empty() && j4["state"] == "abandoned" && j4["next"].is_null(), format!("{e4:?}{j4}"));
    // C1：結案與取消都寫摘要（過程保存）
    let (ar_feat, ar_ab) = (format!("{d_feat}/{ARCHIVE_DIR}/{ARCHIVE_NAME}"), format!("{d_ab}/{ARCHIVE_DIR}/{ARCHIVE_NAME}"));
    let tx = slurp(&ar_feat);
    check!(
        "摘要：close 寫 <run>/archive/結案摘要.md（狀態、recipe、各段與核准、成本的查法）",
        tx.contains("已結案（human @ t）") && tx.contains("recipe：feature") && tx.contains("| plan | done |") && tx.contains("| 段 | 狀態 | 核准 |") && tx.contains("awf cost"),
        py::take(&tx, 600).to_string()
    );
    let tx_ab = slurp(&ar_ab);
    check!("摘要：abandon 也寫，並記取消原因", tx_ab.contains("已取消（human @ t）：不做了"), py::take(&tx_ab, 300).to_string());
    check!(
        "摘要：可重現（重寫的內容逐字相同、沒有現在時間）",
        s.call(&["archive", &d_feat], None).0 == 0 && slurp(&ar_feat) == tx && archive_text(&cx, &d_feat, &s.load(&d_feat)).ok().as_deref() == Some(tx.as_str())
    );
    let blocker = format!("{d_wait}/{ARCHIVE_DIR}");
    put(&blocker, "佔住這個名字"); // archive 是檔案不是目錄：寫不成
    let w_ar = write_archive(&cx, &d_wait, &s.load(&d_wait));
    check!("摘要：寫不成只回警告，不丟例外（輔助功能不擋結案）", w_ar.contains("沒寫成"), w_ar);
    let _ = std::fs::remove_file(&blocker);
    let rich = s.new_run(docs(), false, Some(with_root));
    put(
        &format!("{rich}/test.json"),
        &json!({"verifier": {"status": "done", "summary": "s", "artifacts": ["specs/demo/verifier.md"], "open_questions": [{"question": "要不要補英文版", "blocking": false}],
            "payload": {"criteria": [{"id": "AC1", "result": "pass"}, {"id": "AC2", "result": "fail", "note": "匯出少一欄"}]}},
            "head": "abc1234", "trace": [{"label": "verify", "ok": true}, {"label": "review:bugs", "ok": false}]})
        .to_string(),
    );
    let mut dr = s.load(&rich);
    if let Some(m) = dr.get_mut("stages").and_then(Value::as_object_mut) {
        let cur = m.get("test").cloned().unwrap_or_else(|| json!({"status": "done", "gate": true}));
        m.insert("test".into(), with(&cur, json!({"status": "done"})));
    }
    let tr = archive_text(&cx, &rich, &dr).unwrap_or_else(|e| format!("例外：{}", e.msg));
    check!(
        "摘要：帶出產物、commit、驗證結果（幾條通過、哪幾條沒過）、過程中的問題、缺件的派工",
        ["specs/demo/verifier.md", "test：abc1234", "fail 1 條", "pass 1 條", "AC2（fail）匯出少一欄", "要不要補英文版", "review:bugs"].iter().all(|x| tr.contains(x)),
        tr
    );
    s.call(&["stage", &d_wait, &j2_next], Some(&json!({"halted": true, "halt_reason": "x"})));
    let (j5, e5) = s.snap(&d_wait);
    check!("status --json：halted 的段是 blocked", e5.is_empty() && j5["state"] == "blocked", format!("{e5:?}{j5}"));
    // ── 0.4：執行核心的 trace（升級重派、模型分級）與調研重用
    let base_profile = slurp(&profile_path);
    let models = json!({"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"}, "defaults": {"*": "standard"}, "floors": {}});
    let with_models = with(&py::loads(&base_profile).unwrap_or(Value::Null), json!({"models": models}));
    let none = || json!({});
    let d = s.k_run();
    let good = k_entry(&d, "intent", SPEC, "strong", "inherit");
    let failed = |d: &str| with(&k_entry(d, "locate:a", RES, "light", "inherit"), json!({"ok": false, "superseded": true}));
    let retry = with(&k_entry(&d, "locate:a", RES, "standard", "inherit"), json!({"attempt": 2, "escalated_from": "light"}));
    let (c, o) = s.k_stage(&d, json!([failed(&d), retry, good]), none());
    check!("升級重派：第一筆沒交件、後面同 label 的重派有交件 → 不算缺件", c == 0 && s.stages(&d)["plan"].get("missing_records").is_none(), o);
    let (_, o) = s.k_stage(&d, json!([failed(&d), good]), none());
    check!("升級重派：只有 superseded 旗標、後面沒有交件的重派 → 仍是缺件", truthy(&s.stages(&d)["plan"]["missing_records"]), o);
    let (_, o) = s.k_stage(&d, json!([failed(&d), k_entry(&d, "locate:b", RES, "standard", "inherit"), good]), none());
    check!("升級重派：後面交件的是別的 label → 仍是缺件", truthy(&s.stages(&d)["plan"]["missing_records"]), o);
    check!("模型分級：profile 沒有 models 時不檢查 trace 的等級", s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "light", "haiku")]), none()).0 == 0);
    put(&profile_path, &recipe::dumps(&with_models, None));
    let d = s.k_run();
    let (c, o) = s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "strong", "opus"), k_entry(&d, "research:a", RES, "standard", "sonnet"), k_entry(&d, "locate:a", RES, "light", "haiku")]), none());
    check!("模型分級：等級不低於下限、model 對得上 → 通過", c == 0 && s.stages(&d)["plan"]["status"] == "done", o);
    let (c, o) = s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "light", "haiku")]), none());
    check!("模型分級：plan 段的 dev-spec-author 用 light（低於下限 strong）→ invalid", c == 1 && s.stages(&d)["plan"]["status"] == "invalid" && o.contains("低於下限 strong"), o);
    let (c, o) = s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "strong", "haiku")]), none());
    check!("模型分級：說是 strong、model 卻是 haiku → invalid", c == 1 && o.contains("model 應該是 opus"), o);
    let mut t0 = k_entry(&d, "intent", SPEC, "strong", "opus");
    if let Some(m) = t0.as_object_mut() {
        m.shift_remove("tier");
        m.shift_remove("model");
    }
    let (c, o) = s.k_stage(&d, json!([t0]), none());
    check!("模型分級：profile 有 models，plan 的 trace 沒有 tier → invalid", c == 1 && o.contains("沒有 model／tier"), o);
    let (c, o) = s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "ultra", "opus")]), none());
    check!("模型分級：不認得的等級 → invalid", c == 1 && o.contains("不認得"), o);
    // 0.5：SDLC 每一段都經過執行核心
    let impl_entry = |extra: Value| with(&json!({"label": "Implement:t1", "agent_type": "agent-workflow:dev-implementer", "phase": "Implement", "record": "x.md", "ok": true}), extra);
    let merr = |st: &str, trace: Value| model_errors(st, &json!({"trace": trace}), &with_models, &json!([])).unwrap_or_else(|e| vec![format!("例外：{}", e.msg)]);
    check!(
        "模型分級（0.5）：build、test、deliver 等段的 trace 沒有 tier → 判錯（都要經過執行核心）",
        ["design", "build", "sync", "test", "deliver", "merge"].iter().all(|st| has(&merr(st, json!([impl_entry(none())])), "沒有 model／tier"))
    );
    check!(
        "模型分級（0.5）：build 段的 implementer 用 standard、model 對得上 → 通過；擴充 agent 沒有 tier 不檢查",
        merr("build", json!([impl_entry(json!({"tier": "standard", "model": "sonnet"})), {"label": "ext", "agent_type": "my:agent", "ok": true}])).is_empty()
    );
    check!(
        "模型分級（0.5）：merge 段的 steward 用 standard（低於下限 strong）→ 判錯",
        has(&merr("merge", json!([{"label": "execute", "agent_type": steward, "ok": true, "tier": "standard", "model": "sonnet"}])), "低於下限 strong")
    );
    let cap_run = s.k_run();
    let capped = json!({"trace": [{"label": "Implement:t9", "agent_type": "agent-workflow:dev-implementer", "phase": "Implement",
        "record": format!("{cap_run}/build/Implement-t9.md"), "ok": false, "capped": true, "max_agents": 40}]});
    let (_, rows, miss) = process_trace(&cx, &cap_run, "build", &capped, false).unwrap_or_default();
    let out_file = format!("{d}/outside.md");
    put(&out_file, "# 不是紀錄\n");
    let rc_run = s.k_run();
    put(&format!("{rc_run}/build/real.md"), "# 紀錄\n");
    let outside = record_candidates(&cx, &rc_run, "build", &out_file);
    check!(
        "紀錄檔路徑：trace 指到 run 目錄以外的檔不算紀錄；run 目錄底下的照舊",
        !outside.iter().any(|c| cx.is_file(c)) && record_candidates(&cx, &rc_run, "build", &format!("{rc_run}/build/real.md")).iter().any(|c| cx.is_file(c)),
        format!("{outside:?}")
    );
    check!(
        "派工數上限：到上限沒派出的列成缺件，並說明是上限（不是 agent 失敗）",
        miss.len() == 1 && miss[0].contains("派工數到上限（40") && rows.first().map(|r| r.status.as_str()) == Some("沒有派出（派工數到上限）"),
        format!("{miss:?}")
    );
    put(&profile_path, &recipe::dumps(&with(&with_models, json!({"models": with(&models, json!({"mode": "inherit"}))})), None));
    check!("模型分級：mode=inherit 不檢查", s.k_stage(&d, json!([k_entry(&d, "intent", SPEC, "", "inherit")]), none()).0 == 0);
    put(&profile_path, &base_profile);
    // 調研重用
    put(&format!("{proj}/repo/a.py"), "x = 1\n");
    let d = s.k_run();
    let r_env = with(&env(json!({"lens": "code-contracts", "findings": []})), json!({"summary": "上次的結論"}));
    let res_t = json!([k_entry(&d, "intent", SPEC, "strong", "inherit"), k_entry(&d, "research:code-contracts", RES, "standard", "inherit")]);
    let (c, o) = s.k_stage(
        &d,
        res_t,
        json!({"research": [r_env], "research_by_angle": {"code-contracts": r_env}, "locations": {"code-contracts": [{"path": "a.py", "start": 1, "end": 1, "why": "x"}]}, "reused": []}),
    );
    let store = format!("{d}/plan/reuse.json");
    check!(
        "調研重用：plan 存檔後記下來源檔的 sha1",
        c == 0 && is_file(&store) && py::loads(&slurp(&store)).map(|v| v["angles"].get("code-contracts").is_some()).unwrap_or(false),
        o
    );
    let good_t = json!([k_entry(&d, "intent", SPEC, "strong", "inherit")]);
    let reuse = |env: &Value, angle: &str| json!({"research": [env], "research_by_angle": {"code-contracts": env}, "locations": {}, "reused": [angle]});
    let (c, o) = s.k_stage(&d, good_t.clone(), reuse(&r_env, "code-contracts"));
    check!("調研重用：沿用的角度來源沒變、結論一致 → 通過（不需要 trace）", c == 0 && s.stages(&d)["plan"]["status"] == "done", o);
    let (c, o) = s.k_stage(&d, good_t.clone(), reuse(&with(&r_env, json!({"summary": "偷改"})), "code-contracts"));
    check!("調研重用：沿用的結論和紀錄不一樣 → invalid", c == 1 && o.contains("調研重用"), o);
    put(&format!("{proj}/repo/a.py"), "x = 2\n");
    let (c, o) = s.k_stage(&d, good_t.clone(), reuse(&r_env, "code-contracts"));
    check!("調研重用：來源變了還沿用 → invalid", c == 1 && o.contains("來源已經變了"), o);
    let (c, o) = s.k_stage(&d, good_t, reuse(&r_env, "specs-adr"));
    check!("調研重用：沿用沒有紀錄的角度 → invalid", c == 1 && o.contains("沒有紀錄"), o);
    // escalate 不能清掉後續義務（X3 I1）
    let tamper = |d: &str| recipe_tamper_errors(&cx, &load(&cx, d).unwrap_or(Value::Null), "").unwrap_or_else(|e| vec![format!("例外：{}", e.msg)]);
    let d = s.new_run(hotfix(), true, Some(with_root));
    s.call(&["escalate", &d, "--recipe", "bugfix", "--by", "human", "--at", "t", "--reason", "r"], None);
    s.finish(&d);
    let (c, o) = s.call(&["close", &d, "--by", "human", "--at", "t"], None);
    let tam = tamper(&d);
    let d_dm = s.new_run(docs(), true, Some(with_root));
    s.call(&["escalate", &d_dm, "--recipe", "bugfix", "--modifiers", "migration", "--by", "human", "--at", "t", "--reason", "r"], None);
    let tam2 = tamper(&d_dm);
    check!(
        "hotfix 後續：escalate 成 bugfix 之後仍要 followup_run 才能 close；合法的 escalate 不會被當成手改（docs → bugfix＋migration 也是）",
        s.load(&d)["recipe"]["resolved"]["followup"] == true && c != 0 && o.contains("followup_run") && tam.is_empty() && tam2.is_empty(),
        o + &format!("{tam:?}{tam2:?}")
    );
    // 後續 run 要是真的完整流程：已取消、或本身也是 hotfix 的不行
    let d_ab = s.new_run(None, false, None);
    s.call(&["abandon", &d_ab, "--reason", "x", "--by", "human", "--at", "t"], None);
    let d_hot = s.new_run(hotfix(), true, Some(with_root));
    let (c, o) = s.call(&["followup", &d, "--run", &d_ab, "--by", "human", "--at", "t"], None);
    let (c2, o2) = s.call(&["followup", &d, "--run", &d_hot, "--by", "human", "--at", "t"], None);
    check!("hotfix 後續：已取消的 run、或本身也是 hotfix 的 run 不能當後續 run", c != 0 && o.contains("已取消") && c2 != 0 && o2.contains("最短路徑"), o + &o2);
    // ── X3 B1：workflow 回傳的 recipe 要和 run.json 鎖定的一致；recipe 要求的角色要真的派
    let d = s.new_run(Some(json!({"name": "feature", "modifiers": ["security"]})), true, None);
    s.walk(&d, "test", true);
    let rec = s.load(&d)["recipe"].clone();
    let f = format!("{d}/test/verify.md");
    put(&f, "# 紀錄\n");
    let vt = json!({"label": "verify#1", "agent_type": "agent-workflow:dev-verifier", "phase": "Verify", "record": f, "ok": true});
    let st_ = json!({"label": "security", "agent_type": "agent-workflow:dev-security-reviewer", "phase": "Review", "record": f, "ok": true});
    let good = json!({"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]});
    let (c, o) = s.call(&["stage", &d, "test"], Some(&json!({"v": test, "trace": [vt], "recipe": {"name": "docs", "modifiers": [], "stages": ["plan", "build", "deliver", "merge"]}})));
    let (c2, o2) = approve!(&d, "test", policy);
    check!(
        "X3 B1：回傳的 recipe（docs）和 run.json 鎖定的（feature＋security）不一致 → invalid、不能核准",
        c != 0 && s.stages(&d)["test"]["status"] == "invalid" && o.contains("recipe 不一致") && c2 != 0,
        o + &o2
    );
    let (c, o) = s.call(&["stage", &d, "test"], Some(&json!({"v": test, "trace": [vt], "recipe": good})));
    let (c2, o2) = approve!(&d, "test", policy);
    let (c3, o3) = s.call(&["stage", &d, "test"], Some(&json!({"v": test, "trace": [vt, st_], "recipe": good})));
    let (c4, o4) = approve!(&d, "test", policy);
    check!(
        "X3 B1：security_review=always 卻沒派 dev-security-reviewer → 政策核准被擋；派了才放行",
        c == 0 && c2 != 0 && o2.contains("security_review") && c3 == 0 && c4 == 0,
        o + &o2 + &o3 + &o4
    );
    // ── X3 B3：有 recipe 的 run，回傳沒有 trace 算缺件
    let (_, o) = s.call(&["stage", &d, "test"], Some(&json!({"v": test, "recipe": good})));
    let (c2, o2) = approve!(&d, "test", policy);
    check!("X3 B3：有 recipe 的 run 回傳沒有 trace → missing_records、政策核准被擋", truthy(&s.stages(&d)["test"]["missing_records"]) && c2 != 0 && o2.contains("trace"), o + &o2);
    // ── X3 B2：runs_dir 底下放的 profile 不會被採用；不合規則的 profile 判錯
    let builtin = recipe::builtin_recipes();
    let hijack = json!({"recipes": with(builtin, json!({"docs": with(&builtin["docs"], json!({"build_kinds": ["code", "test", "doc"]}))})), "modifiers": recipe::builtin_modifiers()});
    put(&format!("{proj}/_runs/.claude/teams/methodology.json"), &recipe::dumps(&hijack, None));
    let (prof_v, perr) = load_profile_for(&cx, &[]);
    let badp = format!("{proj}/bad-profile.json");
    put(&badp, &json!({"recipes": {"x": with(&builtin["docs"], json!({"stages": ["build", "deliver", "merge"]}))}}).to_string());
    let (_, perr2) = load_profile_for(&cx, &["--profile".to_string(), badp]);
    check!(
        "X3 B2：不採用 run 目錄上層被放進去的 profile；--profile 不合規則（recipe 沒有 plan）→ 錯誤",
        perr.is_empty() && prof_v["recipes"]["docs"]["build_kinds"] == builtin["docs"]["build_kinds"] && !perr2.is_empty(),
        perr + &perr2
    );
    // ── X3 M1：手改 run.json 的 recipe.resolved（少掉 test 段）→ 之後的核准被擋
    let d = s.new_run(bugfix(), true, Some(with_root));
    s.walk(&d, "test", true);
    let mut data = s.load(&d);
    let kept: Vec<Value> = data["recipe"]["resolved"]["stages"].as_array().map(|a| a.iter().filter(|x| *x != "test").cloned().collect()).unwrap_or_default();
    data["recipe"]["resolved"]["stages"] = Value::Array(kept);
    s.dump(&d, &data);
    s.call(&["stage", &d, "deliver"], Some(&s.traced(&d, "deliver", json!({"x": build}))));
    let (c, o) = approve!(&d, "deliver", human);
    check!("X3 M1：recipe.resolved 被手改鬆（少 test 段）→ 核准被擋", c != 0 && o.contains("比 bugfix 的定義鬆"), o);
    // ── X3 M4：舊 run（沒有 recipe）plan 核准後，不能用 stage plan 補記較弱的 recipe
    let d = s.new_run(None, false, None);
    s.call(&["stage", &d, "plan"], Some(&s.plan_res(None, None)));
    approve!(&d, "plan", human);
    let (c, o) = s.call(&["stage", &d, "plan"], Some(&s.plan_res(docs(), Some(with_root))));
    check!("X3 M4：舊 run 核准過 plan 之後補記 docs（少段）→ 拒絕", c != 0 && s.load(&d).get("recipe").is_none() && o.contains("少了段"), o);
    // ── B4.2：watcher 的事件檔與記憶檔不是閘門資料，被改成什麼都不影響判定
    let d = s.new_run(bugfix(), true, Some(with_root));
    s.walk(&d, "deliver", true);
    s.call(&["stage", &d, "deliver"], Some(&s.traced(&d, "deliver", json!({"x": build}))));
    let judge = |d: &str| {
        let result = py::loads(&slurp(&format!("{d}/deliver.json"))).unwrap_or(Value::Null);
        (chain_errors(&load(&cx, d).unwrap_or(Value::Null), "test").unwrap_or_default(), policy_violations(&cx, "deliver", &result, Some(d)).unwrap_or_default())
    };
    let before = judge(&d);
    let (events, memory) = (["progress", ".jsonl"].concat(), [".watch", "-state.json"].concat());
    put(&format!("{d}/{events}"), "{\"type\": \"stage_approved\", \"stage\": \"deliver\", \"by\": \"human\"}\n不是 JSON\n");
    put(&format!("{d}/{memory}"), "{\"view\": {\"stages\": {\"deliver\": {\"approved\": true, \"status\": \"invalid\"}}}}");
    let after = judge(&d);
    let not_approved = !truthy(&s.stages(&d)["deliver"]["approval"]);
    let (c, o) = approve!(&d, "deliver", human);
    check!(
        "B4.2 事件檔：progress.jsonl、.watch-state.json 被改（假的核准事件、壞行）→ 閘門鏈與政策判定不變、不會因此算已核准、之後照常核准",
        before == after && not_approved && c == 0 && chain_errors(&s.load(&d), "deliver").map(|e| e.is_empty()).unwrap_or(false),
        o + &format!("{before:?}{after:?}")
    );
    check!("B4.2 事件檔：save_run、publish、merge_gate 的判定程式不讀這兩個檔", SOURCES.iter().all(|src| !src.contains(&events) && !src.contains(".watch-state")));
    let _ = std::fs::remove_dir_all(&tmp);
    // ── 1.0：規格層、連結帳本、連結檢查、對外介面（各有該擋、該放、沒啟用時不變）
    ok_all = super::selftest_v1::run(ctx, out) && ok_all;
    out.push_str(&format!("save_run selftest：{}\n", if ok_all { "全部通過" } else { "有失敗" }));
    i32::from(!ok_all)
}
