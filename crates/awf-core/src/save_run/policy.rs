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
    // 繼承（extends）：解析失敗＝profile 讀不懂（fail-closed），不當作沒有 extends
    let prof = match crate::profile::resolve_str(&ctx.abs(&path), prof) {
        Ok(p) => p,
        Err(e) => return (Value::Null, format!("讀不到開發規範 profile {path}：{e}")),
    };
    if !prof.is_object() {
        return (Value::Null, format!("開發規範 profile {path} 不是 JSON 物件"));
    }
    let errs = schema::methodology_errors(&prof);
    let errs = if errs.is_empty() { errs } else { crate::profile::blame_path(std::path::Path::new(&ctx.abs(&path)), errs) };
    if !errs.is_empty() {
        return (Value::Null, format!("開發規範 profile {path} 不合規則：{}", errs.iter().take(3).cloned().collect::<Vec<_>>().join("；")));
    }
    (prof, String::new())
}

/// profile 的 `gates.extra` 可以加閘門的段。plan、design、test、deliver 本來就有；merge 由確定性的 merge gate 判定，不設人工閘門。
pub use crate::stages::EXTRA_GATE as EXTRA_GATE_STAGES;

/// profile 的 `gates.extra`：另外要等核准才能往下走的段。只能加（[`super::GATED`] 的四段拿不掉）；
/// 不是 [`EXTRA_GATE_STAGES`] 的值、型別不對、profile 是 null 都當成沒寫。
pub fn extra_gates(profile: &Value) -> Vec<&'static str> {
    let listed = profile.get("gates").and_then(|g| g.get("extra")).and_then(Value::as_array);
    EXTRA_GATE_STAGES.iter().copied().filter(|st| listed.is_some_and(|l| l.iter().any(|x| x.as_str() == Some(*st)))).collect()
}

/// run.json 已經有 recipe、專案卻沒有 profile 檔時的錯誤訊息（save-run、publish、merge-gate 共用）。
/// recipe 是 plan 段用專案的 profile 解析、鎖定的；profile 不見了通常代表專案根找錯了（或檔案被刪），
/// 這時退回內建節重算會把 profile 定得更嚴的部分與限制文件一起漏掉，所以不採信。沒有 recipe 的舊 run 不受影響。
pub fn profile_missing(path: &str) -> String {
    format!("run.json 已有 recipe，但找不到專案的開發規範 profile（{path}）：無法重算，不採信（專案根找錯了？在專案根執行、設 CLAUDE_PROJECT_DIR，或用 --profile 指定）")
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
/// - 回傳帶 custom_script（自訂腳本）、override_script（覆寫檔或舊副本）、manual_output（手動給的結果）、
///   edited_script（產生的腳本被改過）、edited_output（存的結果和實際回傳不一致）不行：一律由人核准。
/// - 任何信封的 open_questions 有 blocking=true 不行。
/// - 任何 findings payload 有 blocker／important 等級的發現不行。
/// - 任何 verdicts payload 有 refuted 不是 false（skeptic 推翻了該段的產出）不行。
/// - test 段：必須有 verifier 的 criteria payload；任何一條 result 不是 pass 或 anti_cheat 非空都不行。
/// - 給了 run 目錄而回傳帶 trace 時：重新核對紀錄檔（不採信 run.json 的 missing_records），缺了不行。
pub fn policy_violations(ctx: &Ctx, stage: &str, result: &Value, run: Option<&str>) -> R<Vec<String>> {
    policy_violations_with(ctx, stage, result, run, "")
}

/// 同 [`policy_violations`]；`profile` 是 `--profile` 的值（空字串＝用專案的 profile）。只有自訂段用得到 profile：
/// 那一段在 profile 是 `run: "steps"`＋`gate: "policy"` 時，改查 [`super::flowsteps::flow_policy_errors`]（含由工具重跑指令）；
/// 其餘的自訂段（含 profile 讀不到、讀不懂）照舊一律由人核准。
pub fn policy_violations_with(ctx: &Ctx, stage: &str, result: &Value, run: Option<&str>, profile: &str) -> R<Vec<String>> {
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
            errs.push("這一段是用自訂腳本跑的（awf next --script）：流程沒有經過標準腳本的核對，要由人核准".to_string());
        }
        if truthy(field("override_script")) {
            errs.push("這一段用的是專案自己的 workflow 腳本（覆寫檔或舊副本），不是 plugin 的範本：要由人核准".to_string());
        }
        if truthy(field("manual_output")) {
            errs.push("這一段的結果是用 awf done --output 手動給的（不是 awf 從 workflow 紀錄取得的）：要由人核准".to_string());
        }
        if truthy(field("edited_script")) {
            let why = field("edited_script").get("why").and_then(Value::as_str).unwrap_or_default();
            errs.push(format!("這一段跑的腳本在 awf next 產生後被改過（{}）：要由人核准；要回到標準結果，用 awf next --retry 重新產生腳本再跑", py::take(why, 80)));
        }
        if truthy(field("edited_output")) {
            errs.push(format!("這一段存下來的結果和 workflow 實際回傳的不一致（{}）：要由人核准", py::take(&s(field("edited_output")), 160)));
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
            // 人答覆（或照建議先做）blocking 問題後往下走的段、有子任務被略過的 run：一律由人核准（沒有這些紀錄時不多做事）
            if data.get("stages").and_then(|x| x.get(stage)).and_then(|x| x.get(super::human::DECIDED)).is_some_and(truthy) {
                errs.push("這一段停在 needs_decision 之後是靠答覆往下走的（run.json 的 decided）：要由人核准".to_string());
            }
            // 子任務是在 build 段略過的：build 與它之後的段都不能政策核准（之前已經核准的 plan、design 不受影響）
            let skipped = if ["plan", "design"].contains(&stage) { Vec::new() } else { super::human::skipped_subtasks(&data) };
            if !skipped.is_empty() {
                errs.push(format!("這個 run 有子任務被略過（{}）：要由人核准", skipped.join("、")));
            }
            if !super::human::auto_unconfirmed(&data).is_empty() {
                errs.push("還有照建議先做、沒有人確認過的項目：要由人核准".to_string());
            }
            // 自訂段（profile 的 flow）：一律由人核准；recipe、trace 的核對和內建段一樣
            let flow_stage = !SDLC.contains(&stage)
                && data.get("stages").is_some_and(|s| s.get("plan").is_some())
                && (crate::stages::run_customs(&data).iter().any(|c| c == stage)
                    || data.get("stages").and_then(|s| s.get(stage)).and_then(|s| s.get("flow")) == Some(&Value::Bool(true)));
            if flow_stage {
                let (prof, perr) = load_profile(ctx, profile);
                let def = if perr.is_empty() { crate::stages::Resolved::of(&prof).custom(stage).filter(|d| d.policy).cloned() } else { None };
                match def {
                    Some(def) => {
                        let repo = super::flowsteps::target_of(&data)?;
                        errs.extend(super::flowsteps::flow_policy_errors(ctx, stage, &def, &prof, result, repo.as_deref()).into_iter().map(|e| format!("步驟：{e}")));
                    }
                    None => errs.push(format!("{stage} 是自訂段，一律由人核准，不能政策核准")),
                }
            }
            if has_recipe(&data)? && (SDLC.contains(&stage) || flow_stage) && stage != "plan" {
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
    if profile.is_empty() && !ctx.is_file(&ctx.profile) {
        return Ok(vec![profile_missing(&ctx.profile)]);
    }
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
    // 自訂段（profile 的 flow）：點名的段要還在 profile 裡、順序要照 profile（resolved 沒有自訂段時不做事）
    errs.extend(crate::stages::flow_tamper(&prof, get(res, "stages")?));
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
    chain_errors_with(run_or_stages, stage, &NULL)
}

/// 同 [`chain_errors`]；`profile` 是現在的 methodology.json（讀不到給 null）。只影響一件事：前面的自訂段的核准紀錄是政策核准時，
/// 那一段在 profile 要是 `run: "steps"`＋`gate: "policy"` 才採信；其餘（含 null 的 profile）和 [`chain_errors`] 一樣不採信。
pub fn chain_errors_with(run_or_stages: &Value, stage: &str, profile: &Value) -> R<Vec<String>> {
    let flow = crate::stages::Resolved::of(profile);
    // 這個 run 的自訂段（profile 的 flow）：鎖定的 recipe 點名了、而且存檔時標了 flow 的段。
    // recipe 裡其他不認得的名字照舊略過（和原本逐字相同）；「點名了卻沒有標記或沒核准」由 stages::flow_unapproved 另外擋（核准、推送、merge gate 都會查）
    let marked = |c: &str| run_or_stages.get("stages").and_then(|s| s.get(c)).and_then(|s| s.get("flow")) == Some(&Value::Bool(true));
    let customs: Vec<String> = crate::stages::run_customs(run_or_stages).into_iter().filter(|c| marked(c)).collect();
    if !SDLC.contains(&stage) && !customs.iter().any(|c| c == stage) {
        return Ok(Vec::new());
    }
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
    // 這個 run 的完整順序：SDLC 加上 recipe 點名的自訂段（沒有自訂段時就是 SDLC）
    let order: Vec<String> = crate::stages::run_order(want).into_iter().filter(|x| SDLC.contains(&x.as_str()) || customs.iter().any(|c| c == x)).collect();
    let Some(pos) = order.iter().position(|x| x == stage) else { return Ok(Vec::new()) };
    let mut errs = Vec::new();
    // 自訂段一定在 merge 之前：排到 merge 後面的（run.json 被手改）不採信
    let merge_at = order.iter().position(|x| x == crate::stages::FLOW_NO_ANCHOR).unwrap_or(order.len());
    for late in order.iter().skip(merge_at + 1) {
        errs.push(format!("自訂段 {late} 排在 merge 之後（run.json 的 recipe 被手改過？），不採信"));
    }
    for prev in order.iter().take(pos) {
        let prev = &prev.as_str();
        let custom = !SDLC.contains(prev);
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
        } else if (custom || truthy(get(ps, "gate")?)) && !truthy(get(ps, "approved")?) {
            // 自訂段一律有閘門：不看 run.json 自己記的 gate（被改成 false 也一樣要核准）
            errs.push(format!("前面的 {prev} 段還沒核准"));
        } else if custom
            && (get(get(ps, "approved")?, "by")?.as_str() == Some("policy") || get(get(ps, "approved")?, "kind")?.as_str() == Some("policy"))
            && !flow.custom(prev).is_some_and(|d| d.policy)
        {
            errs.push(format!("前面的 {prev} 段是自訂段，核准紀錄卻是政策核准（自訂段一律由人核准），不採信"));
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
