//! 下一個動作（decide）、閘門重點、政策核准、自訂腳本、接手摘要。只排順序與整理，不做任何閘門判定。

use super::args::stage_result;
use super::{
    awf_err, current_session, event_route, find_template, has, hint, is_file, load_json_or, opt, or_obj, read_config, render, render_custom, run_pack, s_or, setdefault_obj, stage_args,
    stage_workflow, write, Env, Fail, A,
};
use crate::kb::pyish;
use crate::save_run::{self as sr, py::{self, pj, R}};
use crate::{pack, pyfmt, recipe, script};
use serde_json::{json, Map, Value};

const NEXT_CMD: &str = "awf";

fn envelopes_into(node: &Value, at: &str, out: &mut Vec<(String, Value)>) {
    match node {
        Value::Object(o) => {
            if sr::is_envelope(node) {
                out.push((at.to_string(), node.clone()));
            } else if !sr::NOT_DELIVERY_KEYS.contains(&at.rsplit('.').next().unwrap_or_default()) {
                for (k, v) in o {
                    let sub = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
                    envelopes_into(v, &sub, out);
                }
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                envelopes_into(v, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// 結果裡的每一份信封：（位置, 信封）。trace、recipe 底下不找。
pub fn envelopes(node: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    envelopes_into(node, "", &mut out);
    out
}

/// agent 已套用的預設（default_applied）；空的、none、無、n/a 都算沒有。
fn applied(q: &Value) -> R<String> {
    let v = s_or(py::get(q, "default_applied")?);
    let v = pyish::strip(&v);
    Ok(if ["", "none", "無", "n/a"].contains(&crate::hooks::py::lower(v).as_str()) { String::new() } else { v.to_string() })
}

/// 信封的 open_questions 裡是物件的那幾筆。
fn questions(e: &Value) -> R<Vec<Value>> {
    Ok(py::iter_or(py::get(e, "open_questions")?)?.into_iter().filter(Value::is_object).collect())
}

/// 不是 blocking、agent 已先照預設做了的問題：[{question, applied}]（同一題只留一筆）。
pub fn provisional(res: &Value) -> R<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<Value> = Vec::new();
    for (_w, e) in envelopes(res) {
        for q in questions(&e)? {
            let question = py::get(&q, "question")?;
            if py::truthy(py::get(&q, "blocking")?) || applied(&q)?.is_empty() || pyish::strip(&s_or(question)).is_empty() {
                continue;
            }
            py::hashable(question)?;
            if seen.iter().any(|x| pyfmt::eq(x, question)) {
                continue;
            }
            seen.push(question.clone());
            out.push(json!({"question": pyish::strip(&py::s(question)), "applied": applied(&q)?}));
        }
    }
    Ok(out)
}

/// `(data["stages"].get(stage) or {})`
fn stage_entry(data: &Value, stage: &str) -> R<Value> {
    Ok(or_obj(py::get(py::idx(data, "stages")?, stage)?))
}

/// `v not in (None, [], {}, "")`
fn present(v: &Value) -> bool {
    !(v.is_null() || v.as_array().is_some_and(Vec::is_empty) || v.as_object().is_some_and(Map::is_empty) || v.as_str() == Some(""))
}

/// 閘門上要列給人看的重點（取自這一段存下來的結果；只整理，不判定）。
pub fn highlights(env: &Env, run: &str, run_arg: &str, data: &Value, stage: &str) -> R<Value> {
    let res = stage_result(env, run, stage)?;
    let st = stage_entry(data, stage)?;
    let envs = envelopes(&res);
    let mut h = Map::new();
    let mut agents: Vec<Value> = Vec::new();
    for (w, e) in envs.iter().take(40) {
        agents.push(json!({"where": w, "status": py::get(e, "status")?, "summary": pyish::take(&s_or(py::get(e, "summary")?), 200)}));
    }
    h.insert("agents".into(), Value::Array(agents));
    let mut qs: Vec<Value> = Vec::new();
    for (w, e) in &envs {
        for q in questions(e)? {
            let mut options: Vec<Value> = Vec::new();
            for o in py::iter_or(py::get(&q, "options")?)? {
                if o.is_object() {
                    options.push(py::get(&o, "choice")?.clone());
                }
            }
            qs.push(json!({"from": w, "question": py::get(&q, "question")?, "blocking": py::truthy(py::get(&q, "blocking")?), "options": options,
                           "recommendation": py::get(&q, "recommendation")?, "default_applied": applied(&q)?}));
        }
    }
    if !qs.is_empty() {
        // 要先決定的排前面；agent 已照建議先做的（default_applied）另外列成 provisional，讓人一次確認
        qs.sort_by_key(|q| q.get("blocking") != Some(&Value::Bool(true)));
        qs.truncate(30);
        h.insert("open_questions".into(), Value::Array(qs));
        let mut prov = provisional(&res)?;
        if !prov.is_empty() {
            prov.truncate(30);
            h.insert("provisional".into(), Value::Array(prov));
        }
    }
    for k in ["missing", "refuted", "reused", "models", "gate", "halt_reason", "next", "located_by", "test_first", "sub_plans", "custom_script", "override_script", "manual_output", "waves"] {
        if let Some(v) = res.get(k).filter(|v| present(v)) {
            h.insert(k.to_string(), v.clone());
        }
    }
    if let Some(Value::Object(angles)) = res.get("angles") {
        if let Some(sk) = angles.get("skipped").filter(|s| py::truthy(s)) {
            h.insert("angles_skipped".into(), sk.clone());
        }
    }
    if stage == "plan" && sr::has_recipe(data)? {
        let r = py::idx(data, "recipe")?;
        let or_else = |v: &Value, d: Value| if py::truthy(v) { v.clone() } else { d };
        h.insert(
            "recipe".into(),
            json!({"name": py::get(r, "name")?, "modifiers": or_else(py::get(r, "modifiers")?, json!([])), "size": or_else(py::get(r, "size")?, json!("")),
                   "stages": py::get(&or_obj(py::get(r, "resolved")?), "stages")?, "reasons": or_else(py::get(r, "reasons")?, json!([]))}),
        );
    }
    for k in ["missing_records", "ghostwritten_records", "constraint_issues", "plan_check"] {
        let v = py::get(&st, k)?;
        if py::truthy(v) {
            h.insert(k.to_string(), v.clone());
        }
    }
    if is_file(&pj(&pj(run, stage), sr::INDEX_NAME)) {
        h.insert("records_index".into(), json!(format!("{run_arg}/{stage}/{}", sr::INDEX_NAME)));
    }
    Ok(Value::Object(h))
}

/// gate_mode=plan-only 的 recipe：非 plan 的閘門先試政策核准（save_run approve --by policy，條件完全由它判）。
/// 過了回 None（data 已更新成核准後的 run.json）；不適用回空清單；被拒回理由清單（交人）。plan 段永遠不試。
pub fn try_policy(env: &Env, run: &str, data: &mut Value, stage: &str) -> A<Option<Vec<String>>> {
    if sr::HUMAN_ONLY.contains(&stage) || !sr::has_recipe(data)? {
        return Ok(Some(Vec::new()));
    }
    let ctx = env.sr();
    let (prof, _err) = sr::load_profile_for(&ctx, &[]);
    let gm = recipe::gate_mode(&prof, py::get(data, "recipe")?)?;
    if py::idx(&gm, "mode")?.as_str() != Some("plan-only") {
        return Ok(Some(Vec::new()));
    }
    let prov = provisional(&stage_result(env, run, stage)?)?;
    let argv: Vec<String> = ["approve", run, stage, "--by", "policy", "--at", &env.now_iso(), "--note", &format!("gate_mode=plan-only（{}）", py::s(py::idx(&gm, "reason")?))].iter().map(|x| x.to_string()).collect();
    let (mut out, mut err) = (String::new(), String::new());
    let code = sr::approve(&ctx, &argv, &mut out, &mut err)?;
    if code != 0 {
        let text = format!("{out}{err}");
        let mut why: Vec<String> = crate::merge_gate::pyutil::splitlines(&text).into_iter().filter(|l| !pyish::strip(l).is_empty()).map(|l| pyish::strip(&l.replace("不能核准：", "")).to_string()).take(12).collect();
        if why.is_empty() {
            why.push("政策核准沒過".to_string());
        }
        return Ok(Some(why));
    }
    let mut fresh = sr::load(&ctx, run)?;
    let old_awf = py::get(data, "awf")?.clone();
    let at = env.now_iso();
    {
        let aw = setdefault_obj(&mut fresh, "awf")?;
        // 這次 next 還沒存檔的狀態（例如 --set 的值在 args，不在這裡）
        if py::truthy(&old_awf) {
            match &old_awf {
                Value::Object(o) => {
                    for (k, v) in o {
                        aw.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                other => return Err(Fail::Py(py::attr_err(other, "items"))),
            }
        }
        match aw.entry("auto_approved".to_string()).or_insert_with(|| json!([])) {
            Value::Array(l) => l.push(json!({"stage": stage, "at": at, "by": "policy"})),
            other => return Err(Fail::Py(py::attr_err(other, "append"))),
        }
        if !prov.is_empty() {
            // agent 先照建議做的暫定：政策核准沒有人看過，留到下一次要人出面時一起確認
            match aw.entry("unconfirmed".to_string()).or_insert_with(|| json!([])) {
                Value::Array(l) => {
                    for q in &prov {
                        let mut m = Map::new();
                        m.insert("stage".into(), json!(stage));
                        for (k, v) in q.as_object().into_iter().flatten() {
                            m.insert(k.clone(), v.clone());
                        }
                        l.push(Value::Object(m));
                    }
                }
                other => return Err(Fail::Py(py::attr_err(other, "extend"))),
            }
        }
    }
    let args = [py::get(data, "args")?.clone(), py::get(&fresh, "args")?.clone()].into_iter().find(py::truthy).unwrap_or_else(|| json!({}));
    py::obj_mut(&mut fresh)?.insert("args".into(), args);
    sr::dump(&ctx, run, &fresh)?;
    *data = fresh;
    Ok(None)
}

/// 政策核准過的段與還沒有人確認的暫定做法：下一次要人出面的動作（gate、manual、halted、done）一併列出。
pub fn carry(mut act: Value, data: &Value) -> R<Value> {
    let aw = or_obj(py::get(data, "awf")?);
    let action = py::get(&act, "action")?.as_str().unwrap_or_default().to_string();
    if ["gate", "manual", "halted", "done"].contains(&action.as_str()) {
        let auto = py::get(&aw, "auto_approved")?;
        if py::truthy(auto) {
            let mut stages = Vec::new();
            for x in py::iter(auto)? {
                stages.push(py::idx(&x, "stage")?.clone());
            }
            py::obj_mut(&mut act)?.insert("auto_approved".into(), Value::Array(stages));
        }
        let unconfirmed = py::get(&aw, "unconfirmed")?;
        if py::truthy(unconfirmed) {
            let say = format!(
                "{} unconfirmed 是政策核准的段裡 agent 先照建議做的暫定，還沒有人看過：一併列給使用者確認，確認後用 awf save-run decide 記下來（不同意就回對應的段修正）。",
                s_or(py::get(&act, "say")?)
            );
            let m = py::obj_mut(&mut act)?;
            m.insert("unconfirmed".into(), unconfirmed.clone());
            m.insert("say".into(), json!(pyish::strip(&say)));
        }
    }
    Ok(act)
}

pub fn gate_action(env: &Env, run: &str, run_arg: &str, data: &Value, stage: &str) -> R<Value> {
    Ok(json!({
        "action": "gate", "stage": stage, "human_only": sr::HUMAN_ONLY.contains(&stage), "highlights": highlights(env, run, run_arg, data, stage)?,
        "say": "把 highlights 在對話裡列給使用者看。blocking 的問題合成一次、用選項問；provisional 是 agent 已照建議先做的暫定，一次列出來請使用者確認（都同意就在 approve 加 --accept-defaults，記進決策記憶；不同意的那幾題用 --answer 記使用者的答覆）。使用者明確核准才跑 approve；「調整」「可以但…」不是核准。",
        "approve": format!("{NEXT_CMD} approve --run {run_arg} {stage} --by <誰核准的> [--answer \"<問題>=<答覆>\"]… [--accept-defaults]"),
        "cost": format!("awf cost --project . --run-dir {run_arg}"),
    }))
}

fn need_input(stage: &str, workflow: &str, need: &[String], how_head: &str) -> Value {
    let mut m = Map::new();
    for k in need {
        m.insert(k.clone(), json!(hint(k)));
    }
    json!({"action": "need_input", "stage": stage, "workflow": workflow, "need": m,
           "how": format!("{how_head} {}", need.iter().map(|k| format!("--set {k}=<值>")).collect::<Vec<_>>().join(" "))})
}

#[allow(clippy::too_many_arguments)]
pub fn workflow_action(env: &Env, run: &str, run_arg: &str, data: &Value, stage: &str, workflow: &str, plugin: Option<&str>, prepare: bool) -> A<Value> {
    let (mut a, need, mut notes) = stage_args(env, run, data, stage, workflow)?;
    if !need.is_empty() {
        return Ok(need_input(stage, workflow, &need, &format!("{NEXT_CMD} next --run {run_arg}")));
    }
    if prepare && workflow.starts_with("sdlc-") && sr::SDLC.contains(&stage) {
        let (extra, pnotes) = run_pack(env, stage, run_arg, &a, run)?;
        for (k, v) in extra {
            a.insert(k, v);
        }
        notes.extend(pnotes);
    }
    let a_val = Value::Object(a.clone());
    let (text, rnotes) = render(env, workflow, plugin, Some(&a_val))?;
    let source = find_template(env, workflow, plugin)?.1;
    if prepare {
        write(&pj(run, &format!(".wf/{workflow}.js")), &text)?;
        write(&pj(run, &format!(".wf/{workflow}.args.json")), &format!("{}\n", py::dumps(&a_val, Some(1))))?;
    }
    let mut summary = Map::new();
    for (k, v) in &a {
        let shown = match v {
            Value::String(_) | Value::Bool(_) => v.clone(),
            Value::Number(n) if !n.is_f64() => v.clone(),
            Value::Array(l) => json!(format!("<list，{} 項>", l.len())),
            Value::Object(o) => json!(format!("<dict，{} 項>", o.len())),
            other => return Err(Fail::Py(py::type_err(format!("object of type '{}' has no len()", py::tname(other))))),
        };
        summary.insert(k.clone(), shown);
    }
    notes.extend(rnotes);
    let mut act = Map::new();
    act.insert("action".into(), json!("run_workflow"));
    act.insert("stage".into(), json!(stage));
    act.insert("workflow".into(), json!(workflow));
    act.insert("script".into(), json!(format!("{run_arg}/.wf/{workflow}.js")));
    act.insert("how".into(), json!("用 Workflow 工具執行：scriptPath 給 script，不用傳 args（已經內嵌在腳本裡）。"));
    act.insert("args_summary".into(), Value::Object(summary));
    act.insert("then".into(), json!(format!("{NEXT_CMD} done --run {run_arg}")));
    if source == "override" || source == "legacy" {
        // 不是 plugin 的範本：這一段的結果不能政策核准，一律交人
        act.insert("override".into(), json!({"workflow": workflow, "source": source}));
        notes.push(format!("這一段用的是專案自己的 {workflow}.js（不是 plugin 的範本）：結果不能政策核准，一律由人核准"));
    }
    let st = stage_entry(data, stage)?;
    let prev = py::get(&st, "wf_run")?;
    let status = py::get(&st, "status")?.as_str().unwrap_or_default().to_string();
    if prev.is_object() && py::truthy(py::get(prev, "id")?) && ["blocked", "needs_decision", "invalid"].contains(&status.as_str()) {
        let sess = py::get(prev, "session")?;
        if py::truthy(sess) && sess.as_str() == Some(current_session(env)?.as_str()) {
            let id = py::idx(prev, "id")?;
            act.insert("resume_from".into(), id.clone());
            let how = format!("{} 這一段上次停在中途：加 resumeFromRunId=\"{}\"，已完成而且派工單沒變的 agent 不會重跑。", s_or(act.get("how").unwrap_or(&Value::Null)), py::s(id));
            act.insert("how".into(), json!(how));
        } else {
            notes.push("上次的 workflow 是在別的 session 跑的，不能從中斷處續跑；這次整段重跑（plan 段會沿用來源沒變的調研）".to_string());
        }
    }
    // notes 在 0.x 的 Python 版的 dict 裡排在 then 後面、override 前面
    let mut ordered = Map::new();
    for (k, v) in act {
        if k == "override" {
            ordered.insert("notes".into(), json!(notes));
        }
        ordered.insert(k, v);
    }
    if !ordered.contains_key("notes") {
        let resume = ordered.shift_remove("resume_from");
        ordered.insert("notes".into(), json!(notes));
        if let Some(r) = resume {
            ordered.insert("resume_from".into(), r);
        }
    }
    Ok(Value::Object(ordered))
}

pub fn halted_action(env: &Env, run: &str, run_arg: &str, data: &Value, stage: &str, workflow: &str, plugin: Option<&str>) -> A<Value> {
    let res = stage_result(env, run, stage)?;
    let st = stage_entry(data, stage)?;
    let status = py::get(&st, "status")?.clone();
    let reason = match res.get("halt_reason").filter(|r| py::truthy(r)) {
        Some(r) => r.clone(),
        None => {
            py::hashable(&status)?;
            json!(match status.as_str() {
                Some("invalid") => "這一段的結果沒通過 save_run 的檢查（看 done 印出的錯誤）".to_string(),
                Some("needs_decision") => "有 agent 在等人決定".to_string(),
                Some("blocked") => "有 agent 回報 blocked".to_string(),
                _ => py::s(&status),
            })
        }
    };
    let mut act = Map::new();
    act.insert("action".into(), json!("halted"));
    act.insert("stage".into(), json!(stage));
    act.insert("status".into(), status);
    act.insert("reason".into(), reason);
    act.insert("advice".into(), res.get("next").filter(|n| py::truthy(n)).cloned().unwrap_or_else(|| json!("")));
    act.insert("highlights".into(), highlights(env, run, run_arg, data, stage)?);
    act.insert("say".into(), json!("把原因與 open_questions 列給使用者；人答覆的決定用 awf save-run decide 記下來。處理完用 retry 的方式重跑這一段。"));
    match workflow_action(env, run, run_arg, data, stage, workflow, plugin, false) {
        Ok(mut retry) => {
            py::obj_mut(&mut retry)?.insert("how_to_prepare".into(), json!(format!("{NEXT_CMD} next --run {run_arg} --retry")));
            act.insert("retry".into(), retry);
        }
        Err(Fail::Awf(m)) => {
            act.insert("retry_error".into(), json!(m));
        }
        Err(e) => return Err(e),
    }
    Ok(Value::Object(act))
}

/// 下一個動作。只讀 run.json 與各段存下來的結果；要執行 workflow 時才寫 `<run>/.wf/` 與資料包。
pub fn decide(env: &Env, run: &str, run_arg: &str, data: &mut Value, plugin: Option<&str>, argv: &[String]) -> A<Value> {
    if py::truthy(py::get(data, "abandoned")?) {
        return Ok(json!({"action": "done", "state": "abandoned", "say": sr::abandoned_error(data)?}));
    }
    if py::truthy(py::get(data, "closed")?) {
        return Ok(json!({"action": "done", "state": "closed"}));
    }
    let event = s_or(py::get(data, "event")?);
    let retry = has(argv, "--retry");
    let forced_wf = opt(argv, "--workflow");
    if event != "sdlc" {
        let Some((workflow, stage)) = event_route(env, &event)? else {
            return Ok(json!({"action": "manual", "say": format!(
                "事件 {} 沒有對應的 workflow（方案辯論用 team-debate skill；自訂的事件類型寫在 methodology.json 的 events）；做完用 awf save-run stage 存結果。",
                if event.is_empty() { "（未指定）" } else { &event }
            )}));
        };
        let st = stage_entry(data, &stage)?;
        let status = py::get(&st, "status")?.as_str().unwrap_or_default().to_string();
        if status == "done" {
            let tip = if event == "merge" && stage_result(env, run, &stage)?.get("merged").is_some_and(py::truthy) { "合併後跑 event-post-merge 監看。" } else { "" };
            return Ok(json!({"action": "done", "state": "done", "highlights": highlights(env, run, run_arg, data, &stage)?, "say": pyish::strip(&format!("把結果整理給使用者。{tip}"))}));
        }
        if ["blocked", "invalid", "needs_decision"].contains(&status.as_str()) && !retry {
            return halted_action(env, run, run_arg, data, &stage, &workflow, plugin);
        }
        if !forced_wf.is_empty() && forced_wf != workflow {
            return awf_err(format!("事件 {event} 只能用 {workflow}，不能用 {forced_wf}（要跑別的事件請另外 start 一個 run）"));
        }
        return workflow_action(env, run, run_arg, data, &stage, &workflow, plugin, true);
    }
    let stage = if opt(argv, "--stage").is_empty() { sr::next_stage(data)? } else { Some(opt(argv, "--stage").to_string()) };
    let Some(stage) = stage.filter(|s| !s.is_empty()) else {
        let rec = recipe::recipe_of_run(data)?;
        let mut tips = vec!["這個 run 的每一段都完成了。".to_string()];
        let stages = py::get(&rec, "stages")?;
        if py::truthy(stages) && py::contains(stages, "merge")? && stage_result(env, run, "merge")?.get("merged").is_some_and(py::truthy) {
            tips.push("已合併：跑 event-post-merge 監看。".to_string());
        }
        if py::truthy(py::get(&rec, "followup")?) && !py::truthy(py::get(data, "followup_run")?) {
            tips.push("這個 recipe 走的是最短路徑：結案前要開後續 run 並用 awf save-run followup 記錄。".to_string());
        }
        tips.push(format!("結案：awf save-run close {run_arg} --by <誰> --at {}", env.now_iso()));
        return Ok(json!({"action": "done", "state": "done", "say": tips.join(" ")}));
    };
    let stage = stage.as_str();
    if !py::contains(py::idx(data, "stages")?, stage)? || !sr::SDLC.contains(&stage) {
        return awf_err(format!("沒有 {stage} 這一段"));
    }
    let st = py::idx(py::idx(data, "stages")?, stage)?.clone();
    let status = py::get(&st, "status")?.as_str().unwrap_or_default().to_string();
    if status == "skipped" {
        return awf_err(format!("recipe 沒有 {stage} 段（要加段走 awf save-run escalate）"));
    }
    let own = stage_workflow(stage);
    let workflow = if forced_wf.is_empty() { own.clone() } else { forced_wf.to_string() };
    if !forced_wf.is_empty() && forced_wf != own && !(stage == "design" && forced_wf == "sdlc-design-revise") {
        return awf_err(format!("{stage} 段不能用 {forced_wf}"));
    }
    if status == "done" && py::truthy(py::get(&st, "gate")?) && !py::truthy(py::get(&st, "approved")?) && forced_wf.is_empty() {
        let Some(refused) = try_policy(env, run, data, stage)? else {
            let rest: Vec<String> = argv.iter().filter(|a| *a != "--retry").cloned().collect();
            return decide(env, run, run_arg, data, plugin, &rest);
        };
        let mut act = gate_action(env, run, run_arg, data, stage)?;
        if !refused.is_empty() {
            let say = format!("這個 recipe 的 gate_mode 是 plan-only，但這一段的結果不乾淨（見 policy_refused），所以交人核准。{}", py::s(py::idx(&act, "say")?));
            let m = py::obj_mut(&mut act)?;
            m.insert("policy_refused".into(), json!(refused));
            m.insert("say".into(), json!(say));
        }
        return Ok(act);
    }
    let chain = sr::chain_errors(data, stage)?;
    if !chain.is_empty() {
        return Ok(json!({"action": "halted", "stage": stage, "status": "chain", "reason": format!("前面的段還沒過：{}", chain.join("；")), "advice": "先完成並核准前面的段"}));
    }
    if ["blocked", "invalid", "needs_decision"].contains(&status.as_str()) && !retry && forced_wf.is_empty() {
        return halted_action(env, run, run_arg, data, stage, &workflow, plugin);
    }
    if stage == "merge" && !py::truthy(py::get(&or_obj(py::get(data, "args")?), "pr")?) {
        let g = load_json_or(env, &pj(&env.teams(), "guard.config.json"))?;
        let am = py::get(&g, "auto_merge")?;
        let auto = am.is_object() && py::truthy(py::get(am, "enabled")?);
        let (a, _, _) = stage_args(env, run, data, "deliver", "sdlc-deliver")?;
        let branch = match a.get("branch").filter(|b| py::truthy(b)) {
            Some(b) => py::s(b),
            None => format!("{}{}", read_config(env)?.get("BRANCH_PREFIX").and_then(Value::as_str).unwrap_or_default(), a.get("epic").map(s_or).unwrap_or_default()),
        };
        let repo = a.get("repo").map(py::s).unwrap_or_else(|| "<repo>".to_string());
        return Ok(json!({
            "action": "manual", "stage": "merge", "what": "推 fork、開 PR",
            "say": format!("推 fork、開 PR 要使用者當下同意（寫入上游要另外再同意一次）。同意後由主 session 跑 publish，再把 PR 號補上。{}",
                           if auto { "" } else { " 注意：guard.config.json 的 auto_merge 沒有啟用，agent 不會合併；PR 開好後由人合併，合併後跑 awf save-run close。" }),
            "publish": format!("awf publish push --repo-dir {repo} --branch {branch} --run {run_arg} --title \"<PR 標題>\" --body-file <PR 說明檔>"),
            "then": if auto { format!("{NEXT_CMD} next --run {run_arg} --set pr=<PR 號>") } else { String::new() },
        }));
    }
    workflow_action(env, run, run_arg, data, stage, &workflow, plugin, true)
}

/// `awf next --script <檔>`：用自訂腳本跑下一段（或 --stage 指定的段）。排序與閘門鏈照舊，只換腳本。
pub fn custom_action(env: &Env, run: &str, run_arg: &str, data: &mut Value, argv: &[String]) -> A<Value> {
    if s_or(py::get(data, "event")?) != "sdlc" {
        return awf_err("自訂腳本只用在 SDLC 的 run");
    }
    if py::truthy(py::get(data, "abandoned")?) || py::truthy(py::get(data, "closed")?) {
        return awf_err("這個 run 已經結束");
    }
    let stage = if opt(argv, "--stage").is_empty() { sr::next_stage(data)? } else { Some(opt(argv, "--stage").to_string()) };
    let stages = py::idx(data, "stages")?;
    let Some(stage) = stage.filter(|s| py::contains(stages, s).unwrap_or(false)) else {
        py::contains(stages, "")?;
        return awf_err("沒有要跑的段");
    };
    let stage = stage.as_str();
    let script_arg = opt(argv, "--script");
    if !script::STAGES.contains(&stage) {
        render_custom(env, script_arg, stage, &json!({}))?; // 回「這一段不能用自訂腳本」
    }
    let st = py::idx(stages, stage)?.clone();
    let status = py::get(&st, "status")?.as_str().unwrap_or_default().to_string();
    if status == "skipped" {
        return awf_err(format!("recipe 沒有 {stage} 段（要加段走 awf save-run escalate）"));
    }
    if status == "done" && py::truthy(py::get(&st, "gate")?) && !py::truthy(py::get(&st, "approved")?) && !has(argv, "--retry") {
        return awf_err(format!("{stage} 段已經有結果、在等核准；要用自訂腳本重跑這一段就加 --stage {stage} --retry"));
    }
    let chain = sr::chain_errors(data, stage)?;
    if !chain.is_empty() {
        return awf_err(format!("前面的段還沒過：{}", chain.join("；")));
    }
    let src = if pyish::is_abs(script_arg) { pyish::ppath(script_arg) } else { pyish::pjoin(&env.project, script_arg) };
    let (mut a, need, notes) = stage_args(env, run, data, stage, &stage_workflow(stage))?;
    let name = format!("custom-{stage}");
    if !need.is_empty() {
        return Ok(need_input(stage, &name, &need, &format!("{NEXT_CMD} next --run {run_arg} --script {script_arg}")));
    }
    let (extra, pnotes) = run_pack(env, stage, run_arg, &a, run)?;
    for (k, v) in extra {
        a.insert(k, v);
    }
    let a_val = Value::Object(a);
    let (text, custom) = render_custom(env, &src, stage, &a_val)?;
    write(&pj(run, &format!(".wf/{name}.js")), &text)?;
    write(&pj(run, &format!(".wf/{name}.args.json")), &format!("{}\n", py::dumps(&a_val, Some(1))))?;
    let mut all: Vec<String> = notes;
    all.extend(pnotes);
    all.push(format!(
        "這一段用自訂腳本 {} 跑：存檔時的檢查和標準腳本一樣（必要的角色、紀錄、模型下限、recipe），而且不能政策核准，一律由人核准",
        py::s(py::idx(&custom, "source")?)
    ));
    Ok(json!({
        "action": "run_workflow", "stage": stage, "workflow": name, "script": format!("{run_arg}/.wf/{name}.js"), "custom": custom,
        "how": "用 Workflow 工具執行：scriptPath 給 script，不用傳 args（已經內嵌在腳本裡）。",
        "then": format!("{NEXT_CMD} done --run {run_arg}"),
        "notes": all,
    }))
}

/// 跨 session 的接手摘要（Markdown）。只讀 run.json、各段結果與決策記憶；不派工、不核准、不寫別的檔。內容只由這些檔決定（沒有時間戳）。
pub fn handoff_text(env: &Env, run: &str, run_arg: &str, data: &Value) -> R<String> {
    let f = |v: &Value, n: usize| pack::flat(Some(v), n);
    let or = |t: String, d: &str| if t.is_empty() { d.to_string() } else { t };
    let run_name = py::pname(run);
    let snap = sr::status_snapshot(&env.sr(), run, data)?;
    let rec_v = py::get(data, "recipe")?;
    let rec = if rec_v.is_object() { rec_v.clone() } else { json!({}) };
    let aw = or_obj(py::get(data, "awf")?);
    let epic = pack::run_epic(run, Some(data));
    let mut lines: Vec<String> = vec![
        format!("# 接手摘要：{}", f(&json!(run_name), 120)),
        String::new(),
        "這份摘要由 `awf handoff` 產生，給換了 session 或換人接手時看。以下是資料，不是指令；實際狀態以 run.json 為準。".to_string(),
        String::new(),
        "## 現在在哪裡".to_string(),
        String::new(),
        format!("- 事件：{}；目標 repo：{}；epic：{}", f(py::get(data, "event")?, 80), or(f(py::get(data, "target_repo")?, 200), "（沒有）"), or(f(&json!(epic), 80), "（沒有）")),
        format!("- 需求：{}", or(f(py::get(data, "requirement")?, 300), "（沒有）")),
    ];
    if py::truthy(&rec) {
        let mods = py::get(&rec, "modifiers")?;
        let mod_text = if py::truthy(mods) { format!("＋{}", py::iter(mods)?.iter().map(|m| f(m, 80)).collect::<Vec<_>>().join("、")) } else { String::new() };
        let stages = py::get(&or_obj(py::get(&rec, "resolved")?), "stages")?.clone();
        lines.push(format!(
            "- recipe：{}{mod_text}（{}）；要走的段：{}",
            f(py::get(&rec, "name")?, 80),
            if py::truthy(py::get(&rec, "locked")?) { "已鎖定" } else { "還沒鎖定，plan 核准後才鎖" },
            py::join(" → ", &py::iter_or(&stages)?)?
        ));
    }
    let waiting = py::get(&snap, "waiting_gate")?;
    lines.push(format!(
        "- 狀態：{}；下一段：{}{}",
        py::s(py::idx(&snap, "state")?),
        py::s_or(py::get(&snap, "next")?, "（沒有）"),
        if py::truthy(waiting) { format!("；在等 {} 段的核准", py::s(waiting)) } else { String::new() }
    ));
    let abandoned = py::get(data, "abandoned")?;
    if py::truthy(abandoned) {
        lines.push(format!("- 已取消：{}", f(py::get(abandoned, "reason")?, 200)));
    }
    lines.extend(["", "## 各段", "", "| 段 | 狀態 | 核准 | agent（缺件） | 備註 |", "|---|---|---|---|---|"].map(String::from));
    let snap_stages = py::iter(py::idx(&snap, "stages")?)?;
    for stg in &snap_stages {
        let name = py::s(py::idx(stg, "name")?);
        let st = stage_entry(data, &name)?;
        let ap = py::get(&st, "approved")?;
        let mut notes: Vec<String> = Vec::new();
        if py::truthy(py::get(&st, "halted")?) {
            notes.push(format!("halted：{}", f(py::get(&st, "halted")?, 120)));
        }
        for (k, head, tail) in [("missing_records", "紀錄缺", "份"), ("constraint_issues", "限制文件", "項"), ("plan_check", "拆工檢查", "項")] {
            let v = py::get(&st, k)?;
            if py::truthy(v) {
                notes.push(format!("{head} {} {tail}", py::len(v)?));
            }
        }
        let status = py::idx(stg, "status")?;
        let approval = if ap.is_object() && py::truthy(ap) {
            format!("{}（{}，{}）", f(py::get(ap, "by")?, 40), if py::get(ap, "kind")?.as_str() == Some("policy") { "政策" } else { "人" }, f(py::get(ap, "at")?, 30))
        } else if py::truthy(py::idx(stg, "gate")?) && status.as_str() == Some("done") {
            "等核准".to_string()
        } else {
            "—".to_string()
        };
        lines.push(format!(
            "| {name} | {} | {approval} | {}（{}） | {} |",
            py::s(status),
            py::s(py::idx(stg, "agents")?),
            py::s(py::idx(stg, "agents_missing")?),
            or(notes.join("；"), "—")
        ));
    }
    let mut args: Vec<(String, Value)> = Vec::new();
    let stored = py::get(data, "args")?;
    if py::truthy(stored) {
        match stored {
            Value::Object(o) => {
                for (k, v) in o {
                    if ["base", "branch", "pr", "has_ui", "has_security", "strategy", "gh_repo"].contains(&k.as_str()) && !(v.is_null() || v.as_str() == Some("")) {
                        args.push((k.clone(), v.clone()));
                    }
                }
            }
            other => return Err(py::attr_err(other, "items")),
        }
    }
    let sync = stage_result(env, run, "sync")?;
    if let Some(hb) = sync.get("head_branch").filter(|h| py::truthy(h)) {
        if !pyfmt::eq(hb, sync.get("branch").unwrap_or(&Value::Null)) {
            args.push(("head_branch（中轉分支）".to_string(), hb.clone()));
        }
    }
    if !args.is_empty() {
        args.sort_by(|a, b| a.0.cmp(&b.0));
        lines.extend(["", "## 已經給過的參數", ""].map(String::from));
        lines.extend(args.iter().map(|(k, v)| format!("- {}：{}", f(&json!(k), 80), f(v, 200))));
    }
    let pend = or_obj(py::get(&aw, "pending")?);
    if py::truthy(&pend) {
        lines.extend(["", "## 跑到一半的段", ""].map(String::from));
        lines.push(format!(
            "- `awf next` 已經交出 {} 段的腳本（{}），還沒有 `awf done`。workflow 跑完了就跑 done；沒跑完或不確定，重跑 `awf next`（換了 session 不能從中斷處續跑，會整段重跑，調研類的結果會沿用）。",
            f(py::get(&pend, "stage")?, 80),
            f(py::get(&pend, "workflow")?, 80)
        ));
    }
    let (auto, unconfirmed) = (py::get(&aw, "auto_approved")?, py::get(&aw, "unconfirmed")?);
    if py::truthy(auto) || py::truthy(unconfirmed) {
        lines.extend(["", "## 還沒有人確認的", ""].map(String::from));
        if py::truthy(auto) {
            let mut names = Vec::new();
            for x in py::iter(auto)? {
                names.push(f(py::get(&x, "stage")?, 80));
            }
            lines.push(format!("- 政策核准（沒有人看過）的段：{}", names.join("、")));
        }
        for q in py::iter_or(unconfirmed)? {
            lines.push(format!("- [{}] {} → agent 已先照「{}」做", f(py::get(&q, "stage")?, 80), f(py::get(&q, "question")?, 200), f(py::get(&q, "applied")?, 200)));
        }
    }
    let mut qs: Vec<String> = Vec::new();
    for stg in &snap_stages {
        let name = py::s(py::idx(stg, "name")?);
        for (w, e) in envelopes(&stage_result(env, run, &name)?) {
            for q in questions(&e)? {
                let question = py::get(&q, "question")?;
                if py::truthy(question) {
                    qs.push(format!("- [{name}／{}] {}{}", f(&json!(w), 60), if py::truthy(py::get(&q, "blocking")?) { "（要先決定）" } else { "" }, f(question, 240)));
                }
            }
        }
    }
    if !qs.is_empty() {
        lines.extend(["", "## agent 提過的問題", ""].map(String::from));
        lines.extend(qs.into_iter().take(40));
    }
    let is = |d: &Value, k: &str, want: &str| d.get(k).and_then(Value::as_str) == Some(want);
    let decs: Vec<Value> = pack::current_decisions(&pack::load_decisions(&py::pparent(run)))
        .into_iter()
        .filter(|d| is(d, "scope", "project") || (!epic.is_empty() && is(d, "epic", &epic)) || is(d, "run", &run_name))
        .collect();
    if !decs.is_empty() {
        lines.extend(["", "## 人已經決定過的（不要再問）", ""].map(String::from));
        lines.extend(pack::decision_lines(decs.get(decs.len().saturating_sub(30)..).unwrap_or_default()));
    }
    lines.extend(["", "## 怎麼接手", ""].map(String::from));
    lines.push(format!("1. `{NEXT_CMD} next --run {run_arg} --text` 看下一步，照做。"));
    lines.push(format!("2. 每一段的紀錄在 `{run_arg}/<段>/{}`；人的決定在 `{}`。", sr::INDEX_NAME, pack::DECISIONS_VIEW));
    lines.push("3. 核准、推送、合併仍然要當下的使用者同意：上一個 session 的同意不算。".to_string());
    lines.push(String::new());
    Ok(lines.join("\n"))
}

/// 給人看的摘要（`--text`）。
pub fn text_of(act: &Value) -> R<String> {
    let stage = py::get(act, "stage")?;
    let mut lines = vec![format!("動作：{}{}", py::s(py::get(act, "action")?), if py::truthy(stage) { format!("（{} 段）", py::s(stage)) } else { String::new() })];
    for (k, label) in [("workflow", "workflow"), ("script", "腳本"), ("how", "做法"), ("resume_from", "續跑"), ("then", "完成後"), ("reason", "原因"), ("advice", "建議"), ("say", "說明"), ("approve", "核准"), ("publish", "推送")] {
        let v = py::get(act, k)?;
        if py::truthy(v) {
            lines.push(format!("{label}：{}", py::s(v)));
        }
    }
    let need = or_obj(py::get(act, "need")?);
    for (k, v) in need.as_object().ok_or_else(|| py::attr_err(&need, "items"))? {
        lines.push(format!("缺 {k}：{}", py::s(v)));
    }
    for n in py::iter_or(py::get(act, "notes")?)? {
        lines.push(format!("- {}", py::s(&n)));
    }
    let h = or_obj(py::get(act, "highlights")?);
    for q in py::iter_or(py::get(&h, "open_questions")?)? {
        lines.push(format!("- {}{}", if py::truthy(py::get(&q, "blocking")?) { "[要先決定] " } else { "" }, py::s(py::get(&q, "question")?)));
    }
    for q in py::iter_or(py::get(&h, "provisional")?)? {
        lines.push(format!("- [暫定，請確認] {} → 已先照「{}」做", py::s(py::idx(&q, "question")?), py::s(py::idx(&q, "applied")?)));
    }
    for k in ["missing", "missing_records", "constraint_issues"] {
        let v = py::get(&h, k)?;
        if py::truthy(v) {
            lines.push(format!("- {k}：{}", py::s(v)));
        }
    }
    Ok(lines.join("\n"))
}
