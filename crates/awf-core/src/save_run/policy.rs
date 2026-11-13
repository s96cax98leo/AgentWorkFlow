//! 政策核准的判定、recipe 的一致與竄改檢查、整條閘門鏈、開發規範 profile 的讀法。

use super::py::{self, get, get_opt, idx, iter_or, s, s_or, truthy, R, NULL};
use super::trace::{envelopes, missing_agents, process_trace};
use super::{load, opt, Ctx, BLOCKING_SEVERITIES, HUMAN_ONLY, NO_ENVELOPE, SDLC};
use crate::{pyfmt, recipe, schema};
use serde_json::{json, Value};

/// recipe 要求「一定要派」的角色：test 段依 `test.*_review=always`，deliver 段依 deliver_axes（label `review:<軸>`）。
pub const REVIEW_ROLES: [(&str, &str); 2] = [("security_review", "dev-security-reviewer"), ("design_review", "dev-design-reviewer")];

/// run.json 有沒有 recipe（有 `recipe.resolved` 物件才算）。
pub fn has_recipe(data: &Value) -> R<bool> {
    let rec = get(data, "recipe")?;
    Ok(rec.is_object() && get(rec, "resolved")?.is_object())
}

/// `_mods(v)`：字串用逗號切；每個元素轉成字串去頭尾空白，空的不要。
pub fn mods(v: &Value) -> R<Vec<Value>> {
    let items = match v {
        Value::String(t) => t.split(',').map(|x| Value::String(x.to_string())).collect(),
        other => iter_or(other)?,
    };
    Ok(items.iter().map(|x| py::strip(&s(x)).to_string()).filter(|x| !x.is_empty()).map(Value::String).collect())
}

/// 讀開發規範 profile：`given`（`--profile`）有給就用它，否則只認 [`Ctx::profile`]（專案的 `.claude/teams/methodology.json`）；
/// 找不到回（null, ""）＝用內建節。不從 run 目錄往上找：runs_dir 底下被放一份 profile 就能劫持 recipe 解析（X3 B2）。
/// 指定了卻讀不到、檔案不是 JSON 物件、或不符合 profile 的語意規則，回傳錯誤（fail-closed）。
pub fn load_profile(ctx: &Ctx, given: &str) -> (Value, String) {
    let path = if given.is_empty() { ctx.profile.clone() } else { py::pp(given) };
    if !ctx.is_file(&path) {
        return (Value::Null, if given.is_empty() { String::new() } else { format!("--profile {given} 不存在") });
    }
    let prof = match ctx.load_json(&path) {
        Ok(p) => p,
        Err(e) => return (Value::Null, format!("讀不到開發規範 profile {path}：{}", e.msg)),
    };
    if !prof.is_object() {
        return (Value::Null, format!("開發規範 profile {path} 不是 JSON 物件"));
    }
    let errs = schema::methodology_errors(&prof);
    if !errs.is_empty() {
        return (Value::Null, format!("開發規範 profile {path} 不合規則：{}", errs.iter().take(3).cloned().collect::<Vec<_>>().join("；")));
    }
    (prof, String::new())
}

/// `load_profile_for(run, argv)`：從 argv 取 `--profile`。
pub fn load_profile_for(ctx: &Ctx, argv: &[String]) -> (Value, String) {
    load_profile(ctx, opt(argv, "--profile", ""))
}

/// workflow 回傳的 recipe 要和 run.json 鎖定的 resolved 一致（name、modifiers、stages），
/// 不一致代表 workflow 用了別的 recipe 裁減（X3 B1）。
pub fn recipe_mismatch(data: &Value, result: &Value) -> R<Vec<String>> {
    let want = idx(data, "recipe")?;
    let empty = json!({});
    let res = match get(want, "resolved")? {
        v if truthy(v) => v,
        _ => &empty,
    };
    let got = if result.is_object() { get(result, "recipe")? } else { &NULL };
    if !got.is_object() {
        return Ok(vec![format!("回傳沒有 recipe：無法確認 workflow 依 run.json 鎖定的 recipe {} 裁減", s(get(want, "name")?))]);
    }
    let mut errs = Vec::new();
    if !pyfmt::eq(get(got, "name")?, get(want, "name")?) {
        errs.push(format!("回傳的 recipe 是 {}，run.json 鎖定的是 {}", s(get(got, "name")?), s(get(want, "name")?)));
    }
    let a = py::sorted(mods(get(got, "modifiers")?)?)?;
    let b = py::sorted(iter_or(get(want, "modifiers")?)?)?;
    if !pyfmt::eq(&Value::Array(a), &Value::Array(b)) {
        errs.push(format!("回傳的 modifiers {} 和 run.json 的 {} 不一致", s(get(got, "modifiers")?), s(get(want, "modifiers")?)));
    }
    let gs = iter_or(get(got, "stages")?)?;
    let rs = iter_or(get(res, "stages")?)?;
    if !pyfmt::eq(&Value::Array(gs), &Value::Array(rs)) {
        errs.push(format!("回傳的段 {} 和 run.json 的 {} 不一致", s(get(got, "stages")?), s(get(res, "stages")?)));
    }
    Ok(errs)
}

/// recipe 要求的角色有沒有在 trace 裡交件（ok=true）。
pub fn required_roles_missing(data: &Value, stage: &str, result: &Value) -> R<Vec<String>> {
    let empty = json!({});
    let res = match get(idx(data, "recipe")?, "resolved")? {
        v if truthy(v) => v,
        _ => &empty,
    };
    let no_trace: Vec<Value> = Vec::new();
    let trace = result.as_object().and_then(|r| r.get("trace")).and_then(Value::as_array).unwrap_or(&no_trace);
    let done: Vec<&Value> = trace.iter().filter(|t| t.as_object().is_some_and(|o| o.get("ok") == Some(&Value::Bool(true)))).collect();
    let agent = |x: &Value| s_or(x.get("agent_type").unwrap_or(&NULL), "");
    let mut errs = Vec::new();
    if stage == "test" {
        let t = match get(res, "test")? {
            v if truthy(v) => v,
            _ => &empty,
        };
        for (key, role) in REVIEW_ROLES {
            if get(t, key)?.as_str() == Some("always") && !done.iter().any(|x| agent(x).ends_with(role)) {
                errs.push(format!("recipe 要求 {key}=always，trace 裡沒有交件的 {role}"));
            }
        }
        if !done.iter().any(|x| agent(x).ends_with("dev-verifier")) {
            errs.push("trace 裡沒有交件的 dev-verifier".to_string());
        }
    }
    if stage == "deliver" {
        for axis in iter_or(get(res, "deliver_axes")?)? {
            let want = format!("review:{}", s(&axis));
            if !done.iter().any(|x| s_or(x.get("label").unwrap_or(&NULL), "") == want) {
                errs.push(format!("recipe 的 deliver 軸 {} 沒有交件的審查（trace 沒有 review:{}）", s(&axis), s(&axis)));
            }
        }
    }
    Ok(errs)
}

/// 政策核准（`--by policy`）的確定性判定：讀該段 workflow 回傳的實際內容，回傳不能自動核准的理由（空清單＝可以）。
/// 人工核准不呼叫這個函式。publish 推送前也重跑這個函式（同一套判定）。
///
/// - plan 段一律由人核准（rules.md 規則 16），政策核准一律不行。
/// - 一份信封都沒有、或回傳裡有 null（agent 缺件）不行：缺件不是「沒有發現」（rules.md 規則 26）。
/// - 回傳標了 halted（流程中斷）不行。
/// - 回傳帶 custom_script（自訂腳本）、override_script（覆寫檔或舊副本）、manual_output（手動給的結果）不行：一律由人核准。
/// - 任何信封的 open_questions 有 blocking=true 不行。
/// - 任何 findings payload 有 blocker／important 等級的發現不行。
/// - 任何 verdicts payload 有 refuted 不是 false（skeptic 推翻了該段的產出）不行。
/// - test 段：必須有 verifier 的 criteria payload；任何一條 result 不是 pass 或 anti_cheat 非空都不行。
/// - 給了 run 目錄而回傳帶 trace 時：重新核對紀錄檔（不採信 run.json 的 missing_records），缺了不行。
pub fn policy_violations(ctx: &Ctx, stage: &str, result: &Value, run: Option<&str>) -> R<Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    if HUMAN_ONLY.contains(&stage) {
        errs.push(format!("{stage} 段一律由人核准（rules.md 規則 16），不能政策核准"));
    }
    if let Some(o) = result.as_object() {
        let field = |k: &str| o.get(k).unwrap_or(&NULL);
        if truthy(field("halted")) {
            let why = if truthy(field("halt_reason")) { field("halt_reason") } else { field("halted") };
            errs.push(format!("回傳標了 halted：{}", s(why)));
        }
        if truthy(field("custom_script")) {
            errs.push("這一段是用自訂腳本跑的（awf.py next --script）：流程沒有經過標準腳本的核對，要由人核准".to_string());
        }
        if truthy(field("override_script")) {
            errs.push("這一段用的是專案自己的 workflow 腳本（覆寫檔或舊副本），不是 plugin 的範本：要由人核准".to_string());
        }
        if truthy(field("manual_output")) {
            errs.push("這一段的結果是用 awf.py done --output 手動給的（不是 awf 從 workflow 紀錄取得的）：要由人核准".to_string());
        }
    }
    let envs = envelopes(result);
    if envs.is_empty() {
        errs.push(NO_ENVELOPE.to_string());
    }
    for p in missing_agents(result) {
        errs.push(format!("agent 缺件：{p} 是 null（缺件不是「沒有發現」）"));
    }
    let empty = json!({});
    for (i, env) in envs.iter().enumerate() {
        for q in iter_or(get(env, "open_questions")?)? {
            if q.is_object() && truthy(get(&q, "blocking")?) {
                let text = get_opt(&q, "question")?.map(s).unwrap_or_default();
                errs.push(format!("信封 {i} 有 blocking 的待決問題：{}", py::take(&text, 80)));
            }
        }
        let payload = match get(env, "payload")? {
            v @ Value::Object(_) => v,
            _ => &empty,
        };
        for f in iter_or(get(payload, "findings")?)? {
            if !f.is_object() {
                continue;
            }
            let sev = get(&f, "severity")?;
            py::hashable(sev)?;
            if sev.as_str().is_some_and(|x| BLOCKING_SEVERITIES.contains(&x)) {
                let id = get_opt(&f, "id")?.map(s).unwrap_or_default();
                let claim = get_opt(&f, "claim")?.map(s).unwrap_or_default();
                errs.push(format!("信封 {i} 有 {} 發現 {id}：{}", s(sev), py::take(&claim, 80)));
            }
        }
        for v in iter_or(get(payload, "verdicts")?)? {
            if !v.is_object() {
                errs.push(format!("信封 {i} 的 verdicts 有格式不對的條目"));
            } else if get(&v, "refuted")? != &Value::Bool(false) {
                let target = get_opt(&v, "target_id")?.map(s).unwrap_or_else(|| "?".to_string());
                let reason = get_opt(&v, "reason")?.map(s).unwrap_or_default();
                errs.push(format!("信封 {i} 的 skeptic 推翻了 {target}：{}", py::take(&reason, 80)));
            }
        }
    }
    if stage == "test" {
        let verifiers: Vec<&Value> = envs.iter().filter_map(|e| e.get("payload")).filter(|p| p.as_object().is_some_and(|o| o.contains_key("criteria"))).collect();
        if verifiers.is_empty() {
            errs.push("test 段找不到 verifier 的 criteria 結果".to_string());
        }
        for p in verifiers {
            let crit = iter_or(get(p, "criteria")?)?;
            if crit.is_empty() {
                errs.push("verifier 的 criteria 是空的".to_string());
            }
            for c in &crit {
                let pass = c.as_object().is_some_and(|o| o.get("result").and_then(Value::as_str) == Some("pass"));
                if !pass {
                    let cid = if c.is_object() { get_opt(c, "id")?.map(s).unwrap_or_else(|| "?".to_string()) } else { "?".to_string() };
                    let res = if c.is_object() { s(get(c, "result")?) } else { s(c) };
                    errs.push(format!("驗收條件 {cid} 結果是 {res}，不是 pass"));
                }
            }
            let ac = get(p, "anti_cheat")?;
            if truthy(ac) {
                errs.push(format!("反作弊有 {} 項發現", py::len(ac)?));
            }
        }
    }
    if let Some(run) = run {
        let (has_trace, _, miss) = process_trace(ctx, run, stage, result, false)?;
        errs.extend(miss.iter().map(|m| format!("執行紀錄缺件：{m}")));
        let data = match load(ctx, run) {
            Ok(d) => Some(d),
            Err(e) if py::os_or_value(&e) => {
                errs.push("讀不到 run.json，無法依 recipe 核對（fail-closed）".to_string());
                None
            }
            Err(e) => return Err(e),
        };
        if let Some(data) = data {
            if has_recipe(&data)? && SDLC.contains(&stage) && stage != "plan" {
                if !has_trace {
                    errs.push("執行紀錄缺件：這個 run 有 recipe，回傳卻沒有 trace（無法核對每個 agent 的紀錄）".to_string());
                }
                errs.extend(recipe_mismatch(&data, result)?);
                errs.extend(required_roles_missing(&data, stage, result)?);
            }
        }
    }
    Ok(errs)
}

/// run.json 的 recipe.resolved 不能比「用專案 profile 依 name、modifiers、size 重算」的結果鬆（X3 M1：手改 resolved 少段）。
/// escalate 只會加嚴，所以合法的 resolved 一定 stricter_or_equal 於重算結果；profile 在 plan 之後被改鬆也會在這裡被擋（fail-closed）。
/// `profile` 是 `--profile` 的值（空字串＝用專案的 profile）。
pub fn recipe_tamper_errors(ctx: &Ctx, data: &Value, profile: &str) -> R<Vec<String>> {
    if !has_recipe(data)? {
        return Ok(Vec::new());
    }
    let rec = idx(data, "recipe")?;
    let (prof, perr) = load_profile(ctx, profile);
    if !perr.is_empty() {
        return Ok(vec![perr]);
    }
    let or = |v: &Value, default: Value| if truthy(v) { v.clone() } else { default };
    let base = recipe::resolve(&prof, get(rec, "name")?, &or(get(rec, "modifiers")?, json!([])), &or(get(rec, "size")?, Value::Null)).map_err(py::from_recipe)?;
    let name = s(get(rec, "name")?);
    let mut errs: Vec<String> = py::iter(idx(&base, "errors")?)?.iter().map(|e| format!("run.json 的 recipe 和重算結果不一致：{}", s(e))).collect();
    if !pyfmt::eq(idx(&base, "name")?, get(rec, "name")?) {
        errs.push(format!("run.json 的 recipe {name} 用目前的 profile 解析成 {}（profile 改過？），不採信", s(idx(&base, "name")?)));
    }
    let res = idx(rec, "resolved")?;
    let mut loose = recipe::stricter_or_equal(res, &base).map_err(py::from_recipe)?;
    let base_kinds = get(&base, "build_kinds")?;
    let mut extra: Vec<Value> = Vec::new();
    for k in iter_or(get(res, "build_kinds")?)? {
        if !truthy(base_kinds) || !py::value_in(&k, base_kinds)? {
            extra.push(k);
        }
    }
    if !extra.is_empty() {
        // escalate 時 build_kinds 照新 recipe，所以合法的 resolved 不會多出重算結果沒有的種類
        loose.push(format!("build_kinds 多了 {}", py::join("、", &extra)?));
    }
    let flag = |v: &Value| -> R<bool> { Ok(get_opt(v, "publishable")?.map(truthy).unwrap_or(true)) };
    if flag(res)? && !flag(&base)? {
        loose.push("publishable 被改成 true".to_string());
    }
    if truthy(get(&base, "followup")?) && !truthy(get(res, "followup")?) {
        loose.push("followup 被拿掉".to_string());
    }
    errs.extend(loose.into_iter().map(|e| format!("run.json 的 recipe.resolved 比 {name} 的定義鬆（手改過？）：{e}")));
    Ok(errs)
}

/// hotfix 的後續 run 要是真的 SDLC run：有 run.json、有 plan 段、沒取消、recipe（有定的話）不是 hotfix。回傳錯誤（空字串＝可以）。
pub fn followup_run_errors(ctx: &Ctx, frun: &str) -> R<String> {
    let f = match ctx.load_json(&py::pj(frun, "run.json")) {
        Ok(f) => f,
        Err(e) if py::os_or_value(&e) => return Ok(format!("後續 run {frun} 讀不到 run.json")),
        Err(e) => return Err(e),
    };
    let stages = get(&f, "stages")?;
    if !truthy(stages) || !py::contains(stages, "plan")? {
        return Ok(format!("後續 run {frun} 不是 SDLC run（沒有 plan 段）"));
    }
    if truthy(get(&f, "abandoned")?) {
        return Ok(format!("後續 run {frun} 已取消，不能當成後續 run"));
    }
    if has_recipe(&f)? && truthy(get(&recipe::recipe_of_run(&f).map_err(py::from_recipe)?, "followup")?) {
        return Ok(format!("後續 run {frun} 的 recipe 是 {}，本身也只是最短路徑，不能當成補完整流程的後續 run", s(get(idx(&f, "recipe")?, "name")?)));
    }
    Ok(String::new())
}

/// 整條閘門鏈：recipe 裡 SDLC 排在 stage 前面的每一段都要 done，有閘門的都要已核准。
///
/// 參數可以是整份 run.json（依它的 recipe 判斷），或只有 stages（當成沒有 recipe 的舊 run）。
/// - recipe 沒有的段不檢查（run.json 標 skipped）。
/// - recipe 有、run.json 卻沒有的段判錯（缺段）；只有沒有 recipe 的舊 run 容許缺 sync（加 sync 段之前建立的 run）。
/// - recipe 有、run.json 卻標 skipped 也判錯（兩邊不一致，可能被手改）。
pub fn chain_errors(run_or_stages: &Value, stage: &str) -> R<Vec<String>> {
    let Some(pos) = SDLC.iter().position(|x| *x == stage) else { return Ok(Vec::new()) };
    let is_run = get(run_or_stages, "stages")?.is_object();
    let wrapped;
    let data = if is_run {
        run_or_stages
    } else {
        wrapped = json!({ "stages": run_or_stages });
        &wrapped
    };
    let stages = idx(data, "stages")?;
    let legacy = !has_recipe(data)?;
    let r = recipe::recipe_of_run(data).map_err(py::from_recipe)?;
    let want = get(&r, "stages")?;
    let name = s(get(&r, "name")?);
    let mut errs = Vec::new();
    for prev in SDLC.iter().take(pos) {
        if !truthy(want) || !py::contains(want, prev)? {
            continue;
        }
        let ps = get(stages, prev)?;
        if !ps.is_object() {
            if legacy && *prev == "sync" {
                continue; // 加 sync 段之前建立的 run 沒有 sync
            }
            errs.push(format!("缺段：recipe {name} 要有 {prev} 段，run.json 卻沒有"));
        } else if get(ps, "status")?.as_str() == Some("skipped") {
            errs.push(format!("recipe {name} 要有 {prev} 段，run.json 卻標成 skipped（不一致）"));
        } else if get(ps, "status")?.as_str() != Some("done") {
            errs.push(format!("前面的 {prev} 段狀態是 {}，還沒 done", s(get(ps, "status")?)));
        } else if truthy(get(ps, "gate")?) && !truthy(get(ps, "approved")?) {
            errs.push(format!("前面的 {prev} 段還沒核准"));
        }
    }
    Ok(errs)
}

/// run 已取消時的說明（沒取消回空字串）。
pub fn abandoned_error(data: &Value) -> R<String> {
    let ab = get(data, "abandoned")?;
    if !truthy(ab) {
        return Ok(String::new());
    }
    let field = |k: &str, default: &str| -> R<String> { Ok(get_opt(ab, k)?.map(s).unwrap_or_else(|| default.to_string())) };
    Ok(format!("run 已取消（{} @ {}：{}）", field("by", "?")?, field("at", "?")?, field("reason", "")?))
}

fn level(v: Option<&Value>) -> R<i32> {
    let Some(v) = v else { return Ok(1) };
    py::hashable(v)?;
    Ok(match v.as_str() {
        Some("never") => 0,
        Some("always") => 2,
        _ => 1,
    })
}

/// escalate 後要重跑的範圍：（新加的段、設定變嚴的段）。
pub fn escalate_affected(new: &Value, old: &Value, profile: &Value) -> R<(Vec<Value>, Vec<String>)> {
    let new_stages = get(new, "stages")?;
    let old_stages = get(old, "stages")?;
    let mut added: Vec<Value> = Vec::new();
    for st in iter_or(new_stages)? {
        if !truthy(old_stages) || !py::value_in(&st, old_stages)? {
            added.push(st);
        }
    }
    let mut changed: Vec<&str> = Vec::new();
    if py::set_gains(get(new, "design_lenses")?, get(old, "design_lenses")?)? {
        changed.push("design");
    }
    let empty = json!({});
    let pick = |v: &'_ Value| -> Value { if truthy(v) { v.clone() } else { empty.clone() } };
    let (nt, ot) = (pick(get(new, "test")?), pick(get(old, "test")?));
    let mut test_changed = py::set_gains(get(&nt, "modes")?, get(&ot, "modes")?)?;
    if !test_changed {
        for k in ["design_review", "security_review"] {
            if level(get_opt(&nt, k)?)? > level(get_opt(&ot, k)?)? {
                test_changed = true;
                break;
            }
        }
    }
    if test_changed {
        changed.push("test");
    }
    if py::set_gains(get(new, "deliver_axes")?, get(old, "deliver_axes")?)? {
        changed.push("deliver");
    }
    let feature = json!("feature");
    let mode = |v: &Value| -> R<Value> { Ok(get_opt(v, "plan_mode")?.cloned().unwrap_or_else(|| feature.clone())) };
    let ns = recipe::required_sections(profile, &mode(new)?).map_err(py::from_recipe)?;
    let os = recipe::required_sections(profile, &mode(old)?).map_err(py::from_recipe)?;
    if py::set_gains(&Value::Array(ns), &Value::Array(os))? {
        changed.push("plan");
    }
    let mut kept = Vec::new();
    for c in changed {
        if truthy(new_stages) && py::contains(new_stages, c)? {
            kept.push(c.to_string());
        }
    }
    Ok((added, kept))
}
