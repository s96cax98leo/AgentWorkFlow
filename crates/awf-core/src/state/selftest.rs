//! `awf state --selftest`：項目名稱與順序同 Python 版的 `awf.py --selftest`。
//!
//! Python 版的工具從自己所在的目錄讀 schema、片段、內建 profile 與交付文件型錄；這裡沒有「工具所在的目錄」，
//! 改從專案的 `.claude/teams`（已 init 的專案）或模版 repo 的 `scaffold/.claude/teams` 讀，並把 schema 與片段放進暫存專案。
//! 在模版 repo 裡跑（資源取自模版）時，和 Python 版一樣多驗兩項真的範本、少驗一項從登記檔找 plugin。

use super::{
    deliverables_sync, event_required, event_route, find_plugin, highlights, is_dir, is_file, load_json, plugin::ph_re, plugin_candidates, render, run_captured, stage_args, Env, Fail, ARGS_LINE,
    EVENT_TYPES, HANDOFF, REQUIRED, TEMPLATES,
};
use crate::kb::pyish;
use crate::save_run::{self as sr};
use crate::{check_agents, pack, recipe, script};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 假範本（Python 版的 FAKE_TEMPLATE；佔位符在這裡組出來，原始碼裡不寫真的佔位符）。
fn fake_template(name: &str) -> String {
    let (l, r) = ("{".repeat(2), "}".repeat(2));
    format!(
        "export const meta = {{ name: '{name}', description: 'x' }}\nconst A = args || {{}}\nconst NS = '{l}AGENT_NS{r}'\nconst MP = /*@methodology*/ null /*@end*/\nreturn {{ stage: 'x', lang: '{l}OUTPUT_LANGUAGE{r}', branch: '{l}BRANCH_PREFIX{r}' + A.epic }}\n"
    )
}

fn put(path: &str, text: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text.as_bytes());
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn pj(a: &str, b: &str) -> String {
    pyish::pjoin(a, b)
}

fn jd(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn copy_dir(from: &str, to: &str) {
    let _ = std::fs::create_dir_all(to);
    let Ok(rd) = std::fs::read_dir(from) else { return };
    for e in rd.flatten() {
        let (p, name) = (e.path(), e.file_name().to_string_lossy().into_owned());
        if p.is_dir() {
            copy_dir(&pj(from, &name), &pj(to, &name));
        } else {
            let _ = std::fs::copy(&p, pj(to, &name));
        }
    }
}

fn tmp_dir(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let p = std::env::temp_dir().join(format!("{prefix}{}-{nanos}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn set_mtime(path: &str, secs: Option<f64>) {
    let when = match secs {
        Some(t) => std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(t),
        None => std::time::SystemTime::now(),
    };
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(when);
    }
}

/// 動作（JSON）：stdout 從第一個 `{` 到結尾。
fn act_of(out: &str) -> Value {
    out.find('{').and_then(|i| serde_json::from_str(out.get(i..)?).ok()).unwrap_or(Value::Null)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn any_has(v: &Value, needle: &str) -> bool {
    strs(v).iter().any(|x| x.contains(needle))
}

/// `d.update(extra)`
fn update(target: &mut Value, extra: Value) {
    if let (Some(t), Value::Object(e)) = (target.as_object_mut(), extra) {
        for (k, v) in e {
            t.insert(k, v);
        }
    }
}

/// save_run 的 `rebuild_stages`（那邊沒有公開）：recipe 有的段補成 pending、沒有的段標 skipped，順序照 SDLC。
fn rebuild_stages(data: &mut Value, resolved: &Value) {
    let want = strs(&resolved["stages"]);
    let Some(d) = data.as_object_mut() else { return };
    let stages = d.entry("stages").or_insert_with(|| json!({}));
    let Some(old) = stages.as_object() else { return };
    let mut ordered = Map::new();
    for st in sr::SDLC {
        let cur = old.get(st).cloned().unwrap_or(Value::Null);
        let v = if want.iter().any(|w| w == st) {
            if !cur.is_object() || cur["status"] == "skipped" { json!({"status": "pending", "gate": sr::GATED.contains(&st)}) } else { cur }
        } else {
            json!({"status": "skipped", "gate": false})
        };
        ordered.insert(st.to_string(), v);
    }
    for (k, v) in old {
        if !sr::SDLC.contains(&k.as_str()) {
            ordered.insert(k.clone(), v.clone());
        }
    }
    *stages = Value::Object(ordered);
}

struct T<'a> {
    ok: bool,
    n: usize,
    out: &'a mut String,
}

impl T<'_> {
    fn check(&mut self, name: &str, cond: bool, detail: String) {
        self.n += 1;
        let tail = if cond { String::new() } else { format!("：{}", detail.chars().take(600).collect::<String>()) };
        self.out.push_str(&format!("  [{}] {name}{tail}\n", if cond { "通過" } else { "失敗" }));
        self.ok = self.ok && cond;
    }
}

/// 暫存專案與它的環境。
struct W {
    env: Env,
    ctx: sr::Ctx,
    proj: String,
    plugin: String,
    profile: Value,
}

impl W {
    /// 跑一次指令：（exit code, stdout＋stderr, stdout）。start、approve 以外都帶 `--plugin-dir`。
    fn call(&self, a: &[&str]) -> (i32, String, String) {
        let mut argv: Vec<String> = a.iter().map(|x| x.to_string()).collect();
        if !matches!(a.first(), Some(&"approve" | &"start")) {
            argv.push("--plugin-dir".to_string());
            argv.push(self.plugin.clone());
        }
        let (code, out, err) = run_captured(&argv, &self.env);
        (code, format!("{out}{err}"), out)
    }

    fn act(&self, a: &[&str]) -> Value {
        act_of(&self.call(a).2)
    }

    fn run(&self, d: &str) -> String {
        pj(&self.proj, d)
    }

    fn load(&self, d: &str) -> Value {
        sr::load(&self.ctx, &self.run(d)).unwrap_or(Value::Null)
    }

    fn dump(&self, d: &str, data: &Value) {
        let _ = sr::dump(&self.ctx, &self.run(d), data);
    }

    fn resolve(&self, name: &str, mods: &[&str]) -> Value {
        recipe::resolve(&self.profile, &json!(name), &json!(mods), &Value::Null).unwrap_or(Value::Null)
    }

    fn start(&self, d: &str, requirement: &str, extra: &[&str]) -> i32 {
        let mut a = vec!["start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", requirement];
        a.extend_from_slice(extra);
        self.call(&a).0
    }

    fn lock(&self, data: &mut Value, name: &str, mods: &[&str], locked: bool) -> Value {
        let res = self.resolve(name, mods);
        data["recipe"] = json!({"name": name, "modifiers": mods, "size": "", "resolved": res, "locked": locked, "history": []});
        rebuild_stages(data, &res);
        res
    }

    /// 一個 recipe 從 start 走到 done：動作序列。
    fn walk(&self, name: &str, mods: &[&str], fail_at: &str) -> Vec<String> {
        let d = format!("_runs/{name}{}{}", if mods.is_empty() { String::new() } else { format!("-{}", mods.join("-")) }, if fail_at.is_empty() { "" } else { "-f" });
        self.start(&d, "做一件事", &[]);
        let run = self.run(&d);
        let mut seq: Vec<String> = Vec::new();
        for _ in 0..40 {
            let (c, merged, out) = self.call(&["next", "--run", &d]);
            if c != 0 {
                seq.push(format!("ERR:{}", merged.trim().chars().take(120).collect::<String>()));
                break;
            }
            let a = act_of(&out);
            let (action, stage) = (a["action"].as_str().unwrap_or_default().to_string(), a["stage"].as_str().unwrap_or_default().to_string());
            seq.push(if stage.is_empty() { action.clone() } else { format!("{action}:{stage}") });
            let mut data = self.load(&d);
            match action.as_str() {
                "run_workflow" => {
                    if !is_file(&pj(&run, &format!(".wf/{}.js", a["workflow"].as_str().unwrap_or_default()))) {
                        seq.push("ERR:no script".to_string());
                        break;
                    }
                    if stage == "plan" {
                        self.lock(&mut data, name, mods, false);
                    }
                    let bad_now = fail_at == stage && !data["stages"][&stage]["failed_once"].as_bool().unwrap_or(false);
                    update(&mut data["stages"][&stage], json!({"status": if bad_now { "blocked" } else { "done" }, "envelopes": 1, "failed_once": true}));
                    let res = json!({"stage": stage, "halted": bad_now, "halt_reason": if bad_now { json!("缺件：x") } else { Value::Null },
                        "subtasks": if stage == "plan" { json!([{"id": "t1"}]) } else { Value::Null }, "merged": stage == "merge"});
                    put(&pj(&run, &format!("{stage}.json")), &jd(&res));
                    self.dump(&d, &data);
                }
                "gate" => {
                    data["stages"][&stage]["approved"] = json!({"by": "human", "at": "t"});
                    self.dump(&d, &data);
                }
                "need_input" => {
                    let sets: Vec<String> = a["need"].as_object().map(|m| m.keys().map(|k| format!("{k}=true")).collect()).unwrap_or_default();
                    let mut argv = vec!["next", "--run", d.as_str()];
                    for s in &sets {
                        argv.push("--set");
                        argv.push(s);
                    }
                    self.call(&argv);
                    seq.pop();
                }
                "manual" => {
                    self.call(&["next", "--run", &d, "--set", "pr=7"]);
                }
                "halted" => {
                    let a2 = self.act(&["next", "--run", &d, "--retry"]);
                    seq.push(if a2["action"] == "run_workflow" { format!("retry:{}", a2["stage"].as_str().unwrap_or_default()) } else { "ERR:retry".to_string() });
                    let mut data = self.load(&d);
                    data["stages"][&stage]["status"] = json!("done");
                    put(&pj(&run, &format!("{stage}.json")), &jd(&json!({"stage": stage, "subtasks": [{"id": "t1"}], "merged": stage == "merge"})));
                    self.dump(&d, &data);
                }
                _ => break,
            }
        }
        seq
    }

    fn expected(&self, name: &str, mods: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for s in strs(&self.resolve(name, mods)["stages"]) {
            if s == "merge" {
                out.push("manual:merge".to_string());
            }
            out.push(format!("run_workflow:{s}"));
            if sr::GATED.contains(&s.as_str()) {
                out.push(format!("gate:{s}"));
            }
        }
        out.push("done".to_string());
        out
    }

    /// recipe 已鎖定、merge 以前的段都做完的 run（deliver 留給政策核准）。
    fn gm_run(&self, name: &str, rcp: &str, mods: &[&str], env_extra: Option<Value>) -> String {
        let d = format!("_runs/{name}");
        self.start(&d, "改文件", &[]);
        let run = self.run(&d);
        let mut data = self.load(&d);
        let res = self.lock(&mut data, rcp, mods, true);
        let mut envl = json!({"status": "done", "summary": "審完", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": []});
        if let Some(extra) = env_extra {
            update(&mut envl, extra);
        }
        for st in strs(&res["stages"]) {
            if st == "merge" {
                break;
            }
            update(&mut data["stages"][&st], json!({"status": "done", "envelopes": 1}));
            let mut trace = Vec::new();
            if st == "deliver" {
                for axis in strs(&res["deliver_axes"]) {
                    let rec = pj(&run, &format!("{st}/review-{axis}.md"));
                    put(&rec, "# 紀錄\n");
                    trace.push(json!({"label": format!("review:{axis}"), "agent_type": "agent-workflow:tpm-reviewer", "phase": "Review", "record": rec, "ok": true}));
                }
            }
            let result = json!({"stage": st, "review": envl, "subtasks": [{"id": "t1"}], "trace": trace, "recipe": {"name": rcp, "modifiers": mods, "stages": res["stages"]}});
            put(&pj(&run, &format!("{st}.json")), &jd(&result));
            if st == "plan" || (st != "deliver" && data["stages"][&st]["gate"].as_bool().unwrap_or(false)) {
                data["stages"][&st]["approved"] = json!({"by": "human", "at": "t", "kind": "human"});
            }
        }
        self.dump(&d, &data);
        d
    }

    /// recipe 已鎖定、`upto` 以前的段都做完並核准的 run。
    fn cs_run(&self, name: &str, upto: &str) -> String {
        let d = format!("_runs/{name}");
        self.start(&d, "分波打地基", &[]);
        let run = self.run(&d);
        let mut data = self.load(&d);
        let res = self.lock(&mut data, "feature", &[], true);
        for st in strs(&res["stages"]) {
            if st == upto {
                break;
            }
            update(&mut data["stages"][&st], json!({"status": "done", "envelopes": 1, "approved": {"by": "human", "at": "t", "kind": "human"}}));
            put(&pj(&run, &format!("{st}.json")), &jd(&json!({"stage": st, "subtasks": [{"id": "t1", "title": "x", "owns": ["a.py"]}]})));
        }
        self.dump(&d, &data);
        d
    }
}

fn fail_text<X>(r: Result<X, Fail>) -> String {
    match r {
        Ok(_) => String::new(),
        Err(Fail::Awf(m)) => m,
        Err(Fail::Py(e)) => format!("{}：{}", e.class, e.msg),
    }
}

/// 讀資源的 `.claude/teams`：專案自己的（已 init），不然用模版 repo 的 scaffold。第二個值＝是不是取自模版 repo。
fn source_teams(env: &Env) -> (String, bool) {
    let own = env.teams();
    if is_file(&pj(&own, "methodologies/default.json")) && is_dir(&pj(&own, "schemas")) {
        return (own, false);
    }
    match &env.template {
        Some(t) => (pj(t, "scaffold/.claude/teams"), true),
        None => (own, false),
    }
}

pub fn selftest(env: &Env, out: &mut String) -> i32 {
    out.push_str("awf selftest（暫存目錄，不碰 repo）\n");
    let (src, in_template) = source_teams(env);
    let mut t = T { ok: true, n: 0, out };
    let root = tmp_dir("awf-sm.");
    main_part(env, &src, in_template, &root, &mut t);
    let _ = std::fs::remove_dir_all(&root);
    // C2：deliver 存檔後同步交付文件（輔助：沒有交付設定時只說明、出錯只警告）
    let droot = tmp_dir("awf-deliv.");
    deliverables_part(env, &src, &droot, &mut t);
    let _ = std::fs::remove_dir_all(&droot);
    let (ok, n) = (t.ok, t.n);
    out.push_str(&format!("awf selftest：{}（{n} 項）\n", if ok { "全部通過" } else { "有失敗" }));
    i32::from(!ok)
}

fn deliverables_part(env: &Env, src: &str, proj: &str, t: &mut T) {
    let e2 = Env { project: proj.to_string(), cwd: proj.to_string(), home: env.home.clone(), plugin_env: [None, None], template: env.template.clone(), transcripts: None, now: None, today: None };
    let run = pj(proj, "_tpm/runs/r1");
    put(&pj(&run, "run.json"), &jd(&json!({"stages": {}, "args": {"epic": "export"}})));
    let msg = deliverables_sync(&e2, &run, &json!({}), None);
    t.check("交付文件同步：沒有交付設定時說明、不當掉", msg.contains("沒有交付設定"), msg.clone());
    put(&pj(proj, ".claude/teams/deliverables/catalog.json"), &read(&pj(src, "deliverables/catalog.json")));
    put(&pj(proj, ".claude/teams/deliverables.json"), &jd(&json!({"documents": [{"id": "dev-record"}]})));
    put(&pj(&run, "deliver/交付摘要.md"), "# 交付摘要\n\n## 這次交付了什麼\n\n匯出 CSV。\n");
    let msg = deliverables_sync(&e2, &run, &json!({}), None);
    let rec = pj(proj, "docs/deliverables/dev-record.md");
    t.check("交付文件同步：交付摘要併進功能開發紀錄", msg.contains("已同步") && is_file(&rec) && read(&rec).contains("匯出 CSV。"), msg.clone());
    put(&pj(proj, ".claude/teams/deliverables.json"), "{壞掉");
    let msg = deliverables_sync(&e2, &run, &json!({}), None);
    t.check("交付文件同步：設定壞掉時只說明、不當掉", msg.contains("讀不懂"), msg.clone());
}

#[allow(clippy::too_many_lines)]
fn main_part(env: &Env, src: &str, in_template: bool, root: &str, t: &mut T) {
    let (proj, plugin, home, tr) = (pj(root, "proj"), pj(root, "plugin"), pj(root, "home"), pj(root, "transcripts"));
    let teams = pj(&proj, ".claude/teams");
    let cfg_full = "AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE=\"繁體中文\"\nBRANCH_PREFIX=feature/\n";
    put(&pj(&teams, "workflow.config"), &format!("{cfg_full}FORK_REPO=\n"));
    let inner = Env { project: proj.clone(), cwd: env.cwd.clone(), home: home.clone(), plugin_env: [None, None], template: env.template.clone(), transcripts: Some(tr.clone()), now: None, today: None };
    let default_profile = load_json(&inner, &pj(src, "methodologies/default.json")).ok().flatten().unwrap_or_else(|| json!({}));
    let profile_text = jd(&default_profile);
    put(&pj(&teams, "methodology.json"), &profile_text);
    let guard = json!({"auto_merge": {"enabled": true}});
    put(&pj(&teams, "guard.config.json"), &jd(&guard));
    put(&pj(&teams, "VERSION"), "9.9.9\n");
    // Python 版從工具所在的目錄讀 schema 與片段；這裡放進暫存專案
    copy_dir(&pj(src, "schemas"), &pj(&teams, "schemas"));
    copy_dir(&pj(src, "tools/snippets"), &pj(&teams, "tools/snippets"));
    put(&pj(&plugin, ".claude-plugin/plugin.json"), &jd(&json!({"version": "9.9.9"})));
    for (w, _) in REQUIRED {
        put(&pj(&plugin, &format!("{TEMPLATES}/{w}.js")), &fake_template(w));
    }
    let w = W { ctx: inner.sr(), env: inner, proj: proj.clone(), plugin: plugin.clone(), profile: default_profile.clone() };
    let pl = Some(plugin.as_str());

    // ── 範本與產生
    let (text, notes) = render(&w.env, "sdlc-plan", pl, Some(&json!({"epic": "demo", "run_dir": "_runs/r"}))).unwrap_or_default();
    t.check(
        "產生：佔位符換成 workflow.config 的值（去掉引號）、profile 內嵌、args 內嵌",
        text.contains("'agent-workflow:'")
            && text.contains("'繁體中文'")
            && !ph_re().is_match(&text)
            && text.contains("\"name\":\"default\"")
            && text.contains("const A = Object.assign({\"epic\":\"demo\",\"run_dir\":\"_runs/r\"}, args || {})")
            && notes.is_empty(),
        text.chars().take(400).collect(),
    );
    let (plain, _) = render(&w.env, "sdlc-plan", pl, None).unwrap_or_default();
    t.check("產生：不給 args 時保留原本那一行（和 init 產生的覆寫檔相同）", plain.contains(&format!("\n{ARGS_LINE}\n")), String::new());
    put(&pj(&teams, "workflow.config"), "AGENT_NS=agent-workflow:\n");
    let bad = fail_text(render(&w.env, "sdlc-plan", pl, None));
    t.check("產生：workflow.config 缺值時拒絕（不產生帶佔位符的腳本）", bad.contains("BRANCH_PREFIX") && bad.contains("OUTPUT_LANGUAGE"), bad.clone());
    put(&pj(&teams, "workflow.config"), cfg_full);
    let ovd = pj(&proj, ".claude/workflows");
    let ov_file = pj(&ovd, "sdlc-plan.js");
    put(&ov_file, &fake_template("sdlc-plan").replace("stage: 'x'", "stage: 'old-copy'"));
    let (t2, n2) = render(&w.env, "sdlc-plan", pl, None).unwrap_or_default();
    t.check("覆寫：沒有 awf-override 標記的同名檔不採用（用 plugin 的範本）並說明", !t2.contains("old-copy") && n2.iter().any(|x| x.contains("不採用")), format!("{n2:?}"));
    put(&ov_file, &format!("{}// awf-override: 9.9.9\n", fake_template("sdlc-plan").replace("stage: 'x'", "stage: 'mine'")));
    let (t3, n3) = render(&w.env, "sdlc-plan", pl, None).unwrap_or_default();
    t.check(
        "覆寫：有標記的覆寫檔生效；profile 照樣內嵌",
        t3.contains("stage: 'mine'") && t3.contains("\"name\":\"default\"") && n3.iter().any(|x| x.contains("覆寫檔")) && !n3.iter().any(|x| x.contains("plugin 現在是")),
        format!("{n3:?}"),
    );
    put(&ov_file, &format!("{}// awf-override: 0.1.0\n", fake_template("sdlc-plan")));
    let n4 = render(&w.env, "sdlc-plan", pl, None).unwrap_or_default().1;
    t.check("覆寫：版本和 plugin 不同會警告", n4.iter().any(|x| x.contains("plugin 現在是 9.9.9")), String::new());
    let _ = std::fs::remove_file(&ov_file);
    put(&pj(&teams, "VERSION"), "0.4.1\n");
    let n5 = render(&w.env, "sdlc-plan", pl, None).unwrap_or_default().1;
    t.check("版本：plugin 和專案的工具版本不同會提醒跑 init.py --upgrade", n5.iter().any(|x| x.contains("init.py --upgrade")), String::new());
    put(&pj(&teams, "VERSION"), "9.9.9\n");
    let mut with_ext = default_profile.clone();
    update(&mut with_ext, json!({"extensions": [{"name": "x", "agent": "my-agent", "stage": "test", "instructions": "i"}]}));
    put(&pj(&teams, "methodology.json"), &jd(&with_ext));
    let bad = fail_text(render(&w.env, "sdlc-test", pl, None));
    t.check("產生：擴充 agent 沒登記守門角色就拒絕（和 build.py 一樣）", bad.contains("my-agent"), bad.clone());
    put(&pj(&teams, "methodology.json"), &profile_text);

    // ── 找 plugin
    let other = pj(root, "other-plugin");
    let _ = std::fs::create_dir_all(pj(&other, TEMPLATES));
    put(
        &pj(&home, "plugins/installed_plugins.json"),
        &jd(&json!({"version": 2, "plugins": {
            "agent-workflow@a": [{"installPath": plugin, "version": "9.9.9", "lastUpdated": "2026-01-01"}],
            "agent-workflow@b": [{"installPath": other, "version": "1.0.0", "lastUpdated": "2026-06-01"}],
            "agent-workflow@c": [{"installPath": other, "version": "9.9.9", "projectPath": pj(root, "elsewhere")}],
            "something-else@a": [{"installPath": other}]}})),
    );
    if in_template {
        // 在模版 repo 裡跑時會先用 repo 自己的 plugin，從登記檔找的那一項只在已 init 的專案驗
        let cands = plugin_candidates(&w.env, Some(&home)).unwrap_or_default();
        t.check("找 plugin：installed_plugins.json 的 agent-workflow 安裝都列出來，別的專案的標出來", cands.len() == 3 && cands.iter().filter(|c| c.other_project).count() == 1, format!("{}", cands.len()));
    } else {
        let (got, gn) = find_plugin(&w.env, "").unwrap_or_default();
        t.check(
            "找 plugin：從 installed_plugins.json 找，版本和專案相同的優先，別的專案的安裝不算，多個來源會提醒",
            got.as_deref() == Some(plugin.as_str()) && gn.iter().any(|x| x.contains("不只一個來源")),
            format!("{got:?} {gn:?}"),
        );
    }
    t.check("找 plugin：登記檔壞掉或不存在當成沒有（不丟例外）", plugin_candidates(&w.env, Some(&pj(root, "nope"))).is_ok_and(|c| c.is_empty()), String::new());

    // ── 狀態機：每個 recipe 從 start 走到 done
    let mut names: Vec<String> = default_profile["recipes"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    names.sort();
    let bad: Vec<String> = names.iter().filter_map(|r| Some((r, w.walk(r, &[], ""), w.expected(r, &[]))).filter(|(_, a, b)| a != b).map(|(r, a, b)| format!("{r}: {a:?} != {b:?}"))).collect();
    t.check(
        &format!("狀態機：{} 個 recipe 從 start 走到 done，動作序列＝recipe 的段＋有閘門的段等核准＋merge 前先推送；沒有的段直接跳過", names.len()),
        names.len() >= 11 && bad.is_empty(),
        bad.join("；"),
    );
    let exp_mig = w.expected("bugfix", &["migration"]);
    t.check("狀態機：modifier 加回的段也照走（bugfix＋migration 有 design）", w.walk("bugfix", &["migration"], "") == exp_mig && exp_mig.iter().any(|x| x == "run_workflow:design"), String::new());
    let seq = w.walk("bugfix", &[], "build");
    let at = seq.iter().position(|x| x == "run_workflow:build");
    t.check(
        "狀態機：halted 的段先回 halted（附原因），--retry 才重跑，之後照常往下",
        at.is_some_and(|i| seq.get(i + 1).map(String::as_str) == Some("halted:build") && seq.get(i + 2).map(String::as_str) == Some("retry:build"))
            && seq.last().map(String::as_str) == Some("done")
            && seq.iter().any(|x| x == "run_workflow:sync"),
        format!("{seq:?}"),
    );

    // ── handoff：跨 session 的接手摘要
    let d = "_runs/bugfix-f"; // 上面走過的 run：有 halted、有核准
    let (c, out, _) = w.call(&["handoff", "--run", d]);
    let hf = pj(&w.run(d), HANDOFF);
    let t1 = read(&hf);
    t.check(
        "handoff：寫出接手摘要，有狀態、各段與核准、怎麼接手",
        c == 0 && t1.contains("## 現在在哪裡") && t1.contains("| plan | done |") && t1.contains("recipe：bugfix") && t1.contains(&format!("awf.py next --run {d}")),
        format!("{out}{}", t1.chars().take(400).collect::<String>()),
    );
    let before = read(&pj(&w.run(d), "run.json"));
    w.call(&["handoff", "--run", d]);
    t.check("handoff：重跑內容相同、不改 run.json、沒有時間戳以外會變的東西", read(&hf) == t1 && read(&pj(&w.run(d), "run.json")) == before, String::new());
    t.check("handoff --print：只印不寫；找不到 run 時回錯誤", w.call(&["handoff", "--run", d, "--print"]).1.starts_with("# 接手摘要") && w.call(&["handoff", "--run", "_runs/nope"]).0 != 0, String::new());

    // ── gate_mode：plan-only 的 recipe，非 plan 的閘門先試政策核准
    let soft = json!({"open_questions": [{"question": "標題用哪個？", "blocking": false, "options": [], "recommendation": "沿用", "default_applied": "沿用舊標題"}]});
    let d = w.gm_run("gm-clean", "docs", &[], Some(soft));
    let a = w.act(&["next", "--run", &d]);
    let ap = w.load(&d)["stages"]["deliver"]["approved"].clone();
    t.check(
        "gate_mode：docs（plan-only）的 deliver 結果乾淨 → 政策核准、直接到下一步（推送仍要人同意）",
        a["action"] == "manual" && a["stage"] == "merge" && ap["kind"] == "policy" && ap["note"].as_str().unwrap_or_default().contains("gate_mode=plan-only"),
        format!("{} {} {ap}", a["policy_refused"], a["action"]),
    );
    t.check(
        "gate_mode：政策核准的段與沒有人看過的暫定做法，在下一次要人出面時列出來",
        a["auto_approved"] == json!(["deliver"])
            && a["unconfirmed"] == json!([{"stage": "deliver", "question": "標題用哪個？", "applied": "沿用舊標題"}])
            && a["say"].as_str().unwrap_or_default().contains("unconfirmed"),
        a.to_string(),
    );
    let ht = w.call(&["handoff", "--run", &d, "--print"]).1;
    t.check(
        "handoff：列出政策核准（沒有人看過）的段與還沒確認的暫定做法",
        ht.contains("政策核准（沒有人看過）的段：deliver") && ht.contains("標題用哪個？ → agent 已先照「沿用舊標題」做") && ht.contains("（政策，"),
        ht.chars().rev().take(600).collect::<String>().chars().rev().collect(),
    );
    let hard = json!({"open_questions": [{"question": "要不要改對外文件？", "blocking": true, "options": [{"choice": "改", "cost": "x"}], "recommendation": "改", "default_applied": "none"}]});
    let d = w.gm_run("gm-dirty", "docs", &[], Some(hard));
    let a = w.act(&["next", "--run", &d]);
    t.check(
        "gate_mode：結果不乾淨（有 blocking 問題）→ 不核准，交人並說明原因",
        a["action"] == "gate" && a["stage"] == "deliver" && any_has(&a["policy_refused"], "blocking") && !sr::py::truthy(&w.load(&d)["stages"]["deliver"]["approved"]),
        a.to_string(),
    );
    let d = w.gm_run("gm-mod", "docs", &["security"], None);
    let a = w.act(&["next", "--run", &d]);
    let a_stage = a["stage"].as_str().unwrap_or_default().to_string();
    t.check(
        "gate_mode：帶 modifier 一律 strict（照舊等人核准，不試政策核准）",
        a["action"] == "gate" && a.get("policy_refused").is_none() && !sr::py::truthy(&w.load(&d)["stages"][&a_stage]["approved"]),
        a.to_string(),
    );
    let d = w.gm_run("gm-strict", "bugfix", &[], None);
    let a = w.act(&["next", "--run", &d]);
    t.check("gate_mode：strict 的 recipe（bugfix）照舊等人核准", a["action"] == "gate" && a.get("policy_refused").is_none(), a.to_string());
    let d = "_runs/gm-plan";
    w.start(d, "改文件", &["--recipe", "docs"]);
    let mut data = w.load(d);
    w.lock(&mut data, "docs", &[], false);
    update(&mut data["stages"]["plan"], json!({"status": "done", "envelopes": 1}));
    put(
        &pj(&w.run(d), "plan.json"),
        &jd(&json!({"stage": "plan", "intent": {"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": []}})),
    );
    w.dump(d, &data);
    let a = w.act(&["next", "--run", d]);
    t.check(
        "gate_mode：plan 段永遠等人核准（plan-only 也不試政策核准）",
        a["action"] == "gate" && a["stage"] == "plan" && a["human_only"] == true && a.get("policy_refused").is_none() && !sr::py::truthy(&w.load(d)["stages"]["plan"]["approved"]),
        a.to_string(),
    );

    // ── 參數
    let d = "_runs/args";
    w.start(d, "需求", &["--recipe", "bugfix", "--set", "base=main"]);
    let run = w.run(d);
    let a = w.act(&["next", "--run", d]);
    let emb: Value = serde_json::from_str(&read(&pj(&run, ".wf/sdlc-plan.args.json"))).unwrap_or(Value::Null);
    let run_real = pyish::resolve(&run).unwrap_or_else(|_| run.clone());
    t.check(
        "參數：plan 段帶 repo、epic、requirement、run_dir、使用者指定的 recipe 與 --set 的值，並備好資料包",
        a["action"] == "run_workflow"
            && emb["repo"] == "repos/app"
            && emb["epic"] == "demo"
            && emb["requirement"] == "需求"
            && emb["run_dir"] == json!(run_real)
            && emb["recipe"] == "bugfix"
            && emb["base"] == "main"
            && emb["pack"] == json!(format!("{run_real}/plan/pack.md"))
            && is_file(&pj(&run, "plan/pack.md")),
        emb.to_string(),
    );
    t.check(
        "參數：腳本裡內嵌同一份 args，next 記下等待中的段",
        read(&pj(&run, ".wf/sdlc-plan.js")).contains(&crate::build::dumps_compact(&emb)) && w.load(d)["awf"]["pending"]["stage"] == "plan",
        String::new(),
    );
    let mut data = w.load(d);
    w.lock(&mut data, "feature", &["ui"], true);
    for s in ["plan", "design", "build", "sync"] {
        update(&mut data["stages"][s], json!({"status": "done", "approved": {"by": "h", "at": "t"}}));
    }
    w.dump(d, &data);
    let subs = json!([{"id": "t1", "title": "a", "kind": "code", "owns": ["a.py"], "depends_on": [], "acceptance": ["x"]}]);
    put(&pj(&run, "design.json"), &jd(&json!({"design": {"status": "done", "summary": "s", "payload": {"epic": "demo", "subtasks": subs}}, "sub_plans": [{"id": "api"}]})));
    put(&pj(&run, "sync.json"), &jd(&json!({"head_branch": "merge/demo-transit"})));
    let a = w.act(&["next", "--run", d, "--set", "recipe=docs"]);
    t.check(
        "參數：test 段缺 has_security 時回 need_input（modifier ui 已經讓 has_ui=true）",
        a["action"] == "need_input" && a["need"].as_object().is_some_and(|m| m.keys().map(String::as_str).collect::<Vec<_>>() == ["has_security"]),
        a.to_string(),
    );
    w.call(&["next", "--run", d, "--set", "has_security=false"]);
    let emb: Value = serde_json::from_str(&read(&pj(&run, ".wf/sdlc-test.args.json"))).unwrap_or(Value::Null);
    t.check(
        "參數：test 段帶 has_ui、has_security、sub_plans、中轉分支；recipe 以 run.json 鎖定的為準（--set 改不了）",
        emb["has_ui"] == true && emb["has_security"] == false && emb["sub_plans"] == json!([{"id": "api"}]) && emb["branch"] == "merge/demo-transit" && emb["recipe"] == "feature" && emb["modifiers"] == json!(["ui"]),
        emb.to_string(),
    );
    let (a_build, need, _) = stage_args(&w.env, &run, &w.load(d), "build", "sdlc-build").unwrap_or_default();
    t.check("參數：build 段的 subtasks 取自 design 的結果", a_build.get("subtasks") == Some(&subs) && need.is_empty(), String::new());
    put(&pj(&run, "build.json"), &jd(&json!({"halted": true, "remaining_subtasks": [{"id": "t2"}], "integrated": ["t1"]})));
    let (a_build, _, _) = stage_args(&w.env, &run, &w.load(d), "build", "sdlc-build").unwrap_or_default();
    t.check("參數：build halted 後續跑只帶 remaining_subtasks 與 integrated", a_build.get("subtasks") == Some(&json!([{"id": "t2"}])) && a_build.get("integrated") == Some(&json!(["t1"])), String::new());
    let (a_merge, need, _) = stage_args(&w.env, &run, &w.load(d), "merge", "sdlc-merge").unwrap_or_default();
    t.check("參數：merge 段不帶 branch，缺 pr", !a_merge.contains_key("branch") && need == ["pr"], String::new());
    t.check("參數：--set 寫錯、想改 run_dir 都拒絕", w.call(&["next", "--run", d, "--set", "沒有等號"]).0 != 0 && w.call(&["next", "--run", d, "--set", "run_dir=/tmp/x"]).0 != 0, String::new());
    let a = w.act(&["next", "--run", d, "--stage", "design", "--workflow", "sdlc-design-revise"]);
    t.check(
        "退回 design：--workflow sdlc-design-revise 缺 decisions、issues 時回 need_input；別段不能用它",
        a["action"] == "need_input"
            && a["need"].as_object().is_some_and(|m| m.keys().map(String::as_str).collect::<BTreeSet<_>>() == BTreeSet::from(["decisions", "issues"]))
            && w.call(&["next", "--run", d, "--stage", "test", "--workflow", "sdlc-design-revise"]).0 != 0,
        a.to_string(),
    );

    // ── 閘門重點
    let d = "_runs/gate";
    w.start(d, "需求", &[]);
    let run = w.run(d);
    let mut data = w.load(d);
    update(&mut data["stages"]["plan"], json!({"status": "done", "envelopes": 1, "missing_records": ["research:x"]}));
    w.dump(d, &data);
    let envl = json!({"status": "done", "summary": "調研完成", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {},
        "open_questions": [{"question": "編碼用什麼？", "blocking": true, "options": [{"choice": "UTF-8", "cost": "無"}], "recommendation": "UTF-8", "default_applied": ""},
                           {"question": "欄位順序？", "blocking": false, "options": [{"choice": "照原型", "cost": "無"}], "recommendation": "照原型", "default_applied": "照原型"},
                           {"question": "要不要加註解？", "blocking": false, "options": [], "recommendation": "不加", "default_applied": "none"}]});
    put(&pj(&run, "plan.json"), &jd(&json!({"stage": "plan", "missing": ["research:y"], "research": [envl], "reused": ["specs-adr"], "trace": [{"label": "x"}]})));
    let a = w.act(&["next", "--run", d]);
    let h = a["highlights"].clone();
    t.check(
        "閘門：plan 等人核准，重點列出各 agent 的摘要、所有 open_questions、缺件、沿用的調研、紀錄缺件",
        a["action"] == "gate"
            && a["human_only"] == true
            && h["agents"][0]["summary"] == "調研完成"
            && h["open_questions"][0]["blocking"] == true
            && h["open_questions"][0]["options"] == json!(["UTF-8"])
            && h["missing"] == json!(["research:y"])
            && h["reused"] == json!(["specs-adr"])
            && h["missing_records"] == json!(["research:x"]),
        a.to_string(),
    );
    let txt = w.call(&["next", "--run", d, "--text"]).1;
    t.check("閘門：--text 給人看的摘要有動作、核准指令與要先決定的問題", txt.contains("動作：gate（plan 段）") && w.call(&["next", "--run", d, "--text"]).1.contains("[要先決定] 編碼用什麼？"), String::new());
    t.check(
        "閘門：agent 已照建議先做的問題列成 provisional（default_applied 是 none 的不算），要先決定的排最前面",
        h["provisional"] == json!([{"question": "欄位順序？", "applied": "照原型"}]) && h["open_questions"][0]["blocking"] == true && w.call(&["next", "--run", d, "--text"]).1.contains("[暫定，請確認] 欄位順序？"),
        h.to_string(),
    );
    let (c, out, _) = w.call(&["approve", "--run", d, "--by", "human", "--answer", "編碼用什麼？=UTF-8", "--accept-defaults"]);
    let dec: BTreeMap<String, String> =
        pack::load_decisions(&pj(&proj, "_runs")).iter().map(|x| (x["question"].as_str().unwrap_or_default().to_string(), x["answer"].as_str().unwrap_or_default().to_string())).collect();
    let got = |k: &str| dec.get(k).map(String::as_str);
    t.check(
        "approve --accept-defaults：暫定的做法記進決策記憶；另外用 --answer 答的照人的答覆",
        c == 0 && got("欄位順序？") == Some("照原型") && got("編碼用什麼？") == Some("UTF-8") && !dec.contains_key("要不要加註解？"),
        format!("{out} {dec:?}"),
    );
    let st = w.load(d)["stages"]["plan"].clone();
    let at = st["approved"]["at"].as_str().unwrap_or_default();
    let at_ok = at.len() > 10 && at.char_indices().take(11).all(|(i, ch)| match i {
        4 | 7 => ch == '-',
        10 => ch == 'T',
        _ => ch.is_ascii_digit(),
    });
    t.check("approve：不指定段時核准正在等的那一段、時間自動取、--answer 記進決策記憶", c == 0 && st["approved"]["by"] == "human" && at_ok && got("編碼用什麼？") == Some("UTF-8"), out.clone());
    let mut data = w.load(d);
    update(&mut data["stages"]["design"], json!({"status": "done", "envelopes": 0}));
    w.dump(d, &data);
    let (c, out, _) = w.call(&["approve", "--run", d, "--by", "human"]);
    t.check(
        "approve：核准仍由 save_run 把關（沒有信封的段照樣拒絕，狀態機不放寬閘門）",
        c != 0 && out.contains("沒有任何信封") && !sr::py::truthy(&w.load(d)["stages"]["design"]["approved"]),
        out.clone(),
    );
    t.check("approve：沒有在等核准的段、缺 --by 都拒絕", w.call(&["approve", "--run", "_runs/args", "--by", "x"]).0 != 0 && w.call(&["approve", "--run", d]).0 != 0, String::new());

    // ── 自訂腳本（next --script）：先檢查、再接上執行核心；閘門條件不變，而且不能政策核准
    let good_js = script::GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'");
    put(&pj(&proj, "scripts/waves.js"), &good_js);
    put(&pj(&proj, "scripts/direct.js"), &good_js.replace("const done = []", "const done = [await agent('x', {})]"));
    put(&pj(&proj, "scripts/ext.js"), script::GOOD);
    let d = w.cs_run("custom", "build");
    let (c, out, so) = w.call(&["next", "--run", &d, "--script", "scripts/waves.js"]);
    let a = if c == 0 { act_of(&so) } else { json!({}) };
    let body = read(&pj(&w.run(&d), ".wf/custom-build.js"));
    t.check(
        "自訂腳本：合格的可以執行（run_workflow，腳本寫在 <run>/.wf/custom-<段>.js，記下來源與雜湊）",
        a["action"] == "run_workflow"
            && a["stage"] == "build"
            && a["script"] == json!(format!("{d}/.wf/custom-build.js"))
            && a["custom"]["source"] == "waves.js"
            && a["custom"]["sha1"].as_str().is_some_and(|x| x.len() == 40)
            && w.load(&d)["awf"]["pending"]["custom"] == a["custom"],
        out.clone(),
    );
    let roles_line = |text: &str| text.split("roles:").nth(1).and_then(|x| x.split('\n').next()).unwrap_or_default().to_string();
    t.check(
        "自訂腳本：接上執行核心（角色白名單、派工數上限、這一段的參數、recipe、片段與 schema），meta 仍在最前面",
        body.starts_with("export const meta = {")
            && body.contains("function awfAgent(")
            && body.contains("function awfLoop(")
            && body.contains("function awfInit(")
            && body.contains("awfKernelInit({ profile: METHODOLOGY, stage: \"build\"")
            && body.contains("\"agent-workflow:dev-implementer\"")
            && !roles_line(&body).contains("\"agent-workflow:dev-merge-steward\"")
            && body.contains("max_agents: 40")
            && body.contains("\"epic\":\"demo\"")
            && body.contains("/*@schema:subtasks*/ {")
            && !body.contains("null /*@end*/")
            && !ph_re().is_match(&body),
        body.chars().take(300).collect(),
    );
    for (bad_name, why) in [("direct.js", "不能直接用 agent()"), ("ext.js", "acme:a11y 不是登記過的角色")] {
        let d_bad = w.cs_run(&format!("custom-{}", bad_name.trim_end_matches(".js")), "build");
        let (c, out, _) = w.call(&["next", "--run", &d_bad, "--script", &format!("scripts/{bad_name}")]);
        t.check(
            &format!("自訂腳本：{why} → 不執行（不產生腳本、不記等待中的段）"),
            c != 0 && out.contains(why) && out.contains("沒通過檢查") && !std::path::Path::new(&pj(&w.run(&d_bad), ".wf/custom-build.js")).exists() && w.load(&d_bad)["awf"].get("pending").is_none(),
            out.clone(),
        );
    }
    let mut g2 = guard.clone();
    update(&mut g2, json!({"extension_agents": {"acme:a11y": "tpm-reviewer"}}));
    put(&pj(&teams, "guard.config.json"), &jd(&g2));
    let d_ext = w.cs_run("custom-ext-ok", "build");
    let (c, out, _) = w.call(&["next", "--run", &d_ext, "--script", "scripts/ext.js"]);
    t.check(
        "自訂腳本：guard.config.json 登記過的擴充 agent 可以派，而且進角色白名單",
        c == 0 && roles_line(&read(&pj(&w.run(&d_ext), ".wf/custom-build.js"))).contains("\"acme:a11y\""),
        out.clone(),
    );
    put(&pj(&teams, "guard.config.json"), &jd(&guard));
    for stg in ["plan", "test", "merge"] {
        let d_no = w.cs_run(&format!("custom-no-{stg}"), stg);
        let (c, out, _) = w.call(&["next", "--run", &d_no, "--script", "scripts/waves.js"]);
        t.check(&format!("自訂腳本：{stg} 段不開放"), c != 0 && out.contains(&format!("{stg} 段不能用自訂腳本")), out.clone());
    }
    let d_chain = w.cs_run("custom-chain", "build");
    let mut dd = w.load(&d_chain);
    if let Some(m) = dd["stages"]["design"].as_object_mut() {
        m.shift_remove("approved");
    }
    w.dump(&d_chain, &dd);
    let (c, out, _) = w.call(&["next", "--run", &d_chain, "--script", "scripts/waves.js", "--stage", "build"]);
    t.check("自訂腳本：不能跳段（前面的段沒核准就拒絕）", c != 0 && out.contains("前面的段還沒過"), out.clone());
    let (c, out) = if w.call(&["start", "--run", "_runs/ev-cs", "--event", "meeting"]).0 == 0 {
        let r = w.call(&["next", "--run", "_runs/ev-cs", "--script", "scripts/waves.js"]);
        (r.0, r.1)
    } else {
        (1, "自訂腳本只用在 SDLC 的 run".to_string())
    };
    t.check("自訂腳本：事件的 run 不能用", c != 0 && out.contains("只用在 SDLC"), out.clone());
    // 存檔：條件和標準腳本一樣（沒有 trace 就是 invalid），結果帶 custom_script，政策核准一律拒絕
    let envl = json!({"status": "done", "summary": "做完", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": []});
    let rec_dir = pj(&w.run(&d), "build");
    let _ = std::fs::create_dir_all(&rec_dir);
    let mk = |label: &str, role: &str| {
        json!({"label": label, "agent_type": format!("agent-workflow:{role}"), "phase": "Build", "record": pj(&rec_dir, &format!("{}.md", label.replace(':', "-"))), "ok": true, "tier": "standard", "model": "sonnet"})
    };
    let res_rcp = json!({"name": "feature", "modifiers": [], "stages": w.resolve("feature", &[])["stages"]});
    let notrace = pj(root, "cs-notrace.json");
    put(&notrace, &jd(&json!({"stage": "build", "halted": false, "results": [envl], "recipe": res_rcp})));
    let (_, out, _) = w.call(&["done", "--run", &d, "--output", &notrace, "--no-next"]);
    let st_n = w.load(&d)["stages"]["build"].clone();
    t.check("自訂腳本：回傳沒有 trace → 和標準腳本一樣記成紀錄缺件（閘門條件不變）", out.contains("回傳卻沒有 trace") && sr::py::truthy(&st_n["missing_records"]), format!("{out} {st_n}"));
    let (c, out, _) = w.call(&["next", "--run", &d, "--script", "scripts/waves.js"]);
    t.check("自訂腳本：build 存檔後下一段是 sync，不開放自訂腳本；要重跑 build 得明講 --stage build --retry", c != 0 && out.contains("sync 段不能用自訂腳本"), out.clone());
    w.call(&["next", "--run", &d, "--script", "scripts/waves.js", "--stage", "build", "--retry"]);
    let full_trace = vec![mk("Tests:t1", "dev-acceptance-tester"), mk("Implement:t1", "dev-implementer"), mk("Integrate", "dev-integrator")];
    for x in &full_trace {
        put(x["record"].as_str().unwrap_or_default(), "# 紀錄\n");
    }
    let good = pj(root, "cs-good.json");
    put(&good, &jd(&json!({"stage": "build", "halted": false, "results": [envl], "recipe": res_rcp, "trace": full_trace})));
    let (_, out, _) = w.call(&["done", "--run", &d, "--output", &good, "--no-next"]);
    let saved = load_json(&w.env, &pj(&w.run(&d), "build.json")).ok().flatten().unwrap_or_else(|| json!({}));
    let st_b = w.load(&d)["stages"]["build"].clone();
    t.check(
        "自訂腳本：存下來的結果帶 custom_script（來源與雜湊）",
        !a["custom"].is_null() && saved["custom_script"] == a["custom"] && matches!(st_b["status"].as_str(), Some("done" | "invalid")),
        format!("{out} {st_b}"),
    );
    let mut without = saved.clone();
    if let Some(m) = without.as_object_mut() {
        m.shift_remove("custom_script");
    }
    let viol = |v: &Value| sr::policy_violations(&w.ctx, "build", v, Some(&w.run(&d))).unwrap_or_default();
    t.check("自訂腳本：政策核准一律拒絕（要由人核准）", viol(&saved).iter().any(|e| e.contains("自訂腳本")) && !viol(&without).iter().any(|e| e.contains("自訂腳本")), String::new());
    let hl = highlights(&w.env, &w.run(&d), &d, &w.load(&d), "build").unwrap_or(Value::Null);
    t.check("自訂腳本：閘門重點列出這一段用的是自訂腳本", hl["custom_script"]["source"] == "waves.js", hl.to_string());
    let core: BTreeSet<&str> = check_agents::CORE.iter().copied().filter(|r| *r != "dev-merge-steward").collect();
    let roles: BTreeSet<&str> = script::ROLES.iter().copied().collect();
    t.check("自訂腳本：可派的模版角色＝check_agents 的 CORE 扣掉 dev-merge-steward", !core.is_empty() && roles == core, format!("{:?}", core.symmetric_difference(&roles).collect::<Vec<_>>()));

    // ── done：自己找 workflow 的回傳
    let d = "_runs/done";
    w.start(d, "需求", &[]);
    let run = w.run(d);
    w.call(&["next", "--run", d]);
    let sess = pj(&tr, "sess-1/workflows");
    put(&pj(&tr, "sess-1.jsonl"), "{}\n");
    let result = json!({"stage": "plan", "halted": true, "halt_reason": "缺件：intent", "trace": [], "missing": ["intent"]});
    let rec = |name: &str, sp: &str, res: &Value| put(&pj(&sess, &format!("{name}.json")), &jd(&json!({"runId": name, "scriptPath": sp, "status": "completed", "result": res})));
    rec("wf_old", &pj(&run, ".wf/sdlc-design.js"), &json!({"stage": "design"}));
    rec("wf_new", &pj(&run, ".wf/sdlc-plan.js"), &result);
    let (_, out, _) = w.call(&["done", "--run", d]);
    let data = w.load(d);
    t.check(
        "done：找到用這支腳本跑的 workflow 回傳，交給 save_run stage 存檔（內容原樣），記下 workflow 的 run id，清掉等待中的段",
        load_json(&w.env, &pj(&run, "plan.json")).ok().flatten().as_ref() == Some(&result)
            && data["stages"]["plan"]["wf_run"] == json!({"id": "wf_new", "session": "sess-1"})
            && data["awf"].get("pending").is_none()
            && matches!(data["stages"]["plan"]["status"].as_str(), Some("blocked" | "invalid")),
        out.clone(),
    );
    t.check("done：印出下一步（這一段 halted → 原因與建議）", out.contains("下一步：") && out.contains("\"action\": \"halted\"") && out.contains("缺件：intent"), out.clone());
    let a = w.act(&["next", "--run", d, "--retry"]);
    t.check(
        "續跑：同一個 session 裡 halted 後重跑，帶 resume_from（＝resumeFromRunId）",
        a["action"] == "run_workflow" && a["resume_from"] == "wf_new" && a["how"].as_str().unwrap_or_default().contains("resumeFromRunId"),
        a.to_string(),
    );
    put(&pj(&tr, "sess-2.jsonl"), "{}\n");
    set_mtime(&pj(&tr, "sess-2.jsonl"), Some(2e9));
    let a = w.act(&["next", "--run", d, "--retry"]);
    t.check("續跑：換了 session 就不帶，並說明整段重跑", a.get("resume_from").is_none() && any_has(&a["notes"], "別的 session"), a.to_string());
    let out_file = pj(root, "out.json");
    put(&out_file, &jd(&result));
    let d2 = "_runs/done2";
    w.start(d2, "需求", &[]);
    sr::run_captured(&["stage".to_string(), w.run(d2), "plan".to_string()], &jd(&result), &w.ctx);
    let manual = read(&pj(&w.run(d2), "plan.json"));
    let same_auto = read(&pj(&run, "plan.json")) == manual;
    let rc_out = w.call(&["done", "--run", d, "--output", &out_file, "--stage", "plan"]).0;
    let by_out = load_json(&w.env, &pj(&run, "plan.json")).ok().flatten().unwrap_or_else(|| json!({}));
    let mut rest = by_out.clone();
    if let Some(m) = rest.as_object_mut() {
        m.shift_remove("manual_output");
    }
    t.check(
        "done：從 workflow 紀錄取得的結果和手動跑 save_run.py stage 的一樣；--output 給的結果多一個 manual_output 標記、其餘相同",
        same_auto && (rc_out == 0 || rc_out == 1) && by_out["manual_output"] == true && serde_json::from_str::<Value>(&manual).is_ok_and(|m| m == rest),
        by_out.to_string(),
    );
    let pv = |v: &Value| sr::policy_violations(&w.ctx, "design", v, None).unwrap_or_default();
    t.check("審查 6：--output 手動給的結果不能政策核准", pv(&by_out).iter().any(|e| e.contains("--output")), String::new());
    // 審查 3：假的 workflow 紀錄
    let d5 = "_runs/done5";
    w.start(d5, "需求", &[]);
    w.call(&["next", "--run", d5]);
    let sp5 = pj(&w.run(d5), ".wf/sdlc-plan.js");
    let real = json!({"stage": "plan", "halted": true, "halt_reason": "真的", "trace": [], "missing": []});
    rec("wf_real5", &sp5, &real);
    rec("wf_fake5", &sp5, &json!({"stage": "plan", "halted": false, "trace": []}));
    set_mtime(&pj(&sess, "wf_fake5.json"), Some(4e9));
    let (_, out, _) = w.call(&["done", "--run", d5, "--no-next"]);
    t.check(
        "審查 3：修改時間在未來的 workflow 紀錄不採用（取真的那一筆）",
        load_json(&w.env, &pj(&w.run(d5), "plan.json")).ok().flatten().is_some_and(|v| v["halt_reason"] == "真的"),
        out.clone(),
    );
    let d6 = "_runs/done6";
    w.start(d6, "需求", &[]);
    w.call(&["next", "--run", d6]);
    let sp6 = pj(&w.run(d6), ".wf/sdlc-plan.js");
    for nm in ["wf_a6", "wf_b6"] {
        rec(nm, &sp6, &real);
    }
    let (c, out, _) = w.call(&["done", "--run", d6, "--no-next"]);
    t.check("審查 3：同一支腳本在這次 next 之後有兩筆紀錄 → 不猜，交人用 --output 指定", c != 0 && out.contains("有 2 筆") && !std::path::Path::new(&pj(&w.run(d6), "plan.json")).exists(), out.clone());
    // 審查 2：覆寫檔跑的段留標記、不能政策核准；審查 6：標記不會因為重跑 next 而掉
    put(&ov_file, &format!("{}\n// awf-override: 9.9.9\n", fake_template("sdlc-plan")));
    let d7 = "_runs/done7";
    w.start(d7, "需求", &[]);
    let a7 = w.act(&["next", "--run", d7]);
    t.check(
        "審查 2：用覆寫檔時 next 的動作帶 override，並記在 run.json",
        a7["override"] == json!({"workflow": "sdlc-plan", "source": "override"}) && w.load(d7)["awf"]["nonstandard"]["plan"]["override"]["source"] == "override" && any_has(&a7["notes"], "不能政策核准"),
        a7.to_string(),
    );
    let _ = std::fs::remove_file(&ov_file);
    w.call(&["next", "--run", d7]); // 重跑 next：pending 被標準腳本蓋掉
    w.call(&["done", "--run", d7, "--output", &out_file, "--no-next"]);
    let r7 = load_json(&w.env, &pj(&w.run(d7), "plan.json")).ok().flatten().unwrap_or_else(|| json!({}));
    t.check(
        "審查 6：之後重跑 next 再用 --output 存，覆寫檔的標記還在",
        r7["override_script"]["source"] == "override" && r7["manual_output"] == true && pv(&r7).iter().any(|e| e.contains("覆寫檔")),
        r7.to_string(),
    );
    let sp7 = pj(&w.run(d7), ".wf/sdlc-plan.js");
    w.call(&["next", "--run", d7, "--retry"]);
    rec("wf_ok7", &sp7, &real);
    set_mtime(&pj(&sess, "wf_ok7.json"), None);
    w.call(&["done", "--run", d7, "--no-next"]);
    let r7b = load_json(&w.env, &pj(&w.run(d7), "plan.json")).ok().flatten().unwrap_or_else(|| json!({}));
    t.check(
        "審查 6：改用標準腳本、回傳取自 workflow 紀錄後，標記解除",
        r7b.get("override_script").is_none() && r7b.get("manual_output").is_none() && w.load(d7)["awf"]["nonstandard"].get("plan").is_none(),
        r7b.to_string(),
    );
    // 審查 8、9
    w.call(&["start", "--run", "_runs/ev8", "--event", "meeting", "--set", "source=notes.md"]);
    let (c, out, _) = w.call(&["next", "--run", "_runs/ev8", "--workflow", "sdlc-merge"]);
    t.check("審查 8：事件的 run 不能用 --workflow 換成別支", c != 0 && out.contains("只能用 event"), out.clone());
    let d9 = w.cs_run("set9", "build");
    let mut data9 = w.load(&d9);
    if !data9["args"].is_object() {
        data9["args"] = json!({});
    }
    data9["args"]["subtasks"] = json!([{"id": "evil"}]);
    let (a9, _, n9) = stage_args(&w.env, &w.run(&d9), &data9, "build", "sdlc-build").unwrap_or_default();
    t.check(
        "審查 9：--set 的 subtasks 蓋不掉已存檔的拆工",
        a9.get("subtasks").and_then(Value::as_array).is_some_and(|l| l.iter().map(|s| s["id"].as_str().unwrap_or_default()).collect::<Vec<_>>() == ["t1"]) && n9.iter().any(|x| x.contains("忽略 --set 的 subtasks")),
        format!("{:?} {n9:?}", a9.get("subtasks")),
    );
    let d3 = "_runs/done3";
    w.start(d3, "需求", &[]);
    w.call(&["next", "--run", d3]);
    let (c, out, _) = w.call(&["done", "--run", d3]);
    t.check("done：找不到回傳時明說，並提示用 --output", c != 0 && out.contains("--output"), out.clone());
    t.check("done：沒跑過 next 又沒指定段時拒絕", w.call(&["done", "--run", d2]).0 != 0, String::new());

    // ── 事件與結束狀態
    let d = "_runs/ev";
    w.call(&["start", "--run", d, "--event", "meeting", "--set", "source=meetings/a.md"]);
    let a = w.act(&["next", "--run", d]);
    let ev_real = pyish::resolve(&w.run(d)).unwrap_or_else(|_| w.run(d));
    t.check(
        "事件：meeting 用 event（共用的事件 workflow），帶 event、source 與 run_dir",
        a["action"] == "run_workflow"
            && a["workflow"] == "event"
            && a["stage"] == "meeting"
            && serde_json::from_str::<Value>(&read(&pj(&w.run(d), ".wf/event.args.json"))).is_ok_and(|v| v == json!({"source": "meetings/a.md", "run_dir": ev_real, "event": "meeting"})),
        a.to_string(),
    );
    w.call(&["start", "--run", "_runs/ev-brief", "--event", "brief", "--set", "brief=briefs/a.md"]);
    t.check("事件：形狀不同的事件（brief）仍用自己的 workflow", w.act(&["next", "--run", "_runs/ev-brief"])["workflow"] == "event-brief", String::new());
    let meth_file = pj(&teams, "methodology.json");
    let meth_saved = read(&meth_file);
    let custom = json!({"args": ["incident"], "context": "事故：{incident}", "gather": {"angles": [{"key": "cause", "ask": "找原因"}]}, "challenge": {"ask": "反駁"}, "write": {"ask": "寫回顧"}});
    let mut up = custom.clone();
    up["args"] = json!(["what"]);
    let mut meth: Value = serde_json::from_str(&meth_saved).unwrap_or_else(|_| json!({}));
    update(&mut meth, json!({"events": {"incident": custom, "merge": custom, "upstream": up}}));
    put(&meth_file, &jd(&meth));
    w.call(&["start", "--run", "_runs/ev-inc", "--event", "incident"]);
    let a = w.act(&["next", "--run", "_runs/ev-inc"]);
    let need_set = |a: &Value| a["need"].as_object().map(|m| m.keys().cloned().collect::<BTreeSet<String>>()).unwrap_or_default();
    t.check("自訂事件：profile 的 events 加一筆就有這個事件；缺它要的參數回 need_input", a["action"] == "need_input" && need_set(&a) == BTreeSet::from(["incident".to_string()]), a.to_string());
    let a = w.act(&["next", "--run", "_runs/ev-inc", "--set", "incident=TASK-1"]);
    t.check(
        "自訂事件：參數齊了就跑 event，args 帶 event=<類型>",
        a["action"] == "run_workflow"
            && a["workflow"] == "event"
            && a["stage"] == "incident"
            && serde_json::from_str::<Value>(&read(&pj(&proj, "_runs/ev-inc/.wf/event.args.json"))).is_ok_and(|v| v["event"] == "incident"),
        a.to_string(),
    );
    let route = |ev: &str| event_route(&w.env, ev).ok().flatten();
    t.check(
        "自訂事件：可以取代內建類型（必要參數照 profile）；不能蓋掉 merge 這種不是 event.js 的事件",
        event_required(&w.env, "upstream").is_ok_and(|v| v == ["what"]) && route("upstream") == Some(("event".to_string(), "upstream".to_string())) && route("merge") == Some(("sdlc-merge".to_string(), "merge".to_string())),
        String::new(),
    );
    put(&meth_file, &meth_saved);
    w.call(&["start", "--run", "_runs/ev2", "--event", "upstream"]);
    let a = w.act(&["next", "--run", "_runs/ev2"]);
    t.check(
        "事件：缺必要參數回 need_input 並說明怎麼補",
        a["action"] == "need_input" && need_set(&a) == BTreeSet::from(["what".to_string(), "repo".to_string()]) && a["how"].as_str().unwrap_or_default().contains("--set what="),
        a.to_string(),
    );
    w.call(&["start", "--run", "_runs/ev3", "--event", "debate"]);
    t.check("事件：沒有對應 workflow 的事件回 manual", w.act(&["next", "--run", "_runs/ev3"])["action"] == "manual", String::new());
    let mut data = w.load(d);
    data["abandoned"] = json!({"by": "h", "at": "t", "reason": "取消"});
    w.dump(d, &data);
    t.check("已取消的 run：next 回 done（abandoned），不再派工", w.act(&["next", "--run", d])["state"] == "abandoned", String::new());
    t.check("找不到 run.json、缺 --run、不認得的指令都回錯誤", w.call(&["next", "--run", "_runs/nope"]).0 != 0 && w.call(&["next"]).0 != 0 && w.call(&["nope", "--run", d]).0 != 0, String::new());

    // ── render --all
    let outd = pj(root, "rendered");
    let (c, out, _) = w.call(&["render", "--all", "--out", &outd]);
    let js_in = |dir: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".js")).map(str::to_string)).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let rendered = js_in(&outd);
    t.check(
        "render --all：每支範本都產生，沒有殘留佔位符",
        c == 0 && rendered.len() == REQUIRED.len() && !rendered.iter().any(|n| ph_re().is_match(&read(&pj(&outd, &format!("{n}.js"))))),
        out.clone(),
    );
    let real_dir = env.template.as_ref().map(|tp| pj(tp, &format!("plugin/{TEMPLATES}")));
    if let Some(real_dir) = real_dir.filter(|p| in_template && is_dir(p)) {
        let names = js_in(&real_dir);
        let known = |n: &str| REQUIRED.iter().any(|(k, _)| *k == n);
        let miss: Vec<String> = names.iter().filter(|n| !known(n)).cloned().chain(REQUIRED.iter().filter(|(k, _)| !names.iter().any(|n| n == k)).map(|(k, _)| k.to_string())).collect();
        let line = format!("\n{ARGS_LINE}\n");
        let bad: Vec<String> = names.iter().filter(|n| read(&pj(&real_dir, &format!("{n}.js"))).matches(&line).count() != 1).cloned().collect();
        t.check("真的範本：每一支都在 REQUIRED 裡、都有唯一的一行可以內嵌 args", miss.is_empty() && bad.is_empty(), format!("{miss:?} {bad:?}"));
        let ev_src = read(&pj(&real_dir, "event.js"));
        let found: BTreeMap<String, Vec<String>> = regex::Regex::new(r"(?m)^  '?([a-z][a-z-]*)'?: \{\n    title: [^\n]*\n    args: \[([^\]]*)\]")
            .map(|re| {
                re.captures_iter(&ev_src)
                    .map(|m| (m[1].to_string(), m[2].split(',').filter(|x| !x.trim().is_empty()).map(|x| x.trim_matches(|ch| ch == ' ' || ch == '\'').to_string()).collect()))
                    .collect()
            })
            .unwrap_or_default();
        let want: BTreeMap<String, Vec<String>> = EVENT_TYPES.iter().map(|(k, v)| (k.to_string(), v.iter().map(|x| x.to_string()).collect())).collect();
        t.check("真的範本：event.js 內建的事件類型與必要參數和 EVENT_TYPES 一致", found == want, format!("{found:?}"));
    }
}
