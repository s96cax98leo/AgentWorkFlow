//! 子指令：init、stage、approve、decide、escalate、followup、close、abandon、archive、status。
//! 每一支都照 save_run.py 的 `main` 逐行移植：檢查的順序、寫檔的時機、印到 stdout／stderr 的文字都一樣。

use super::archive::{next_stage, recipe_summary, status_snapshot, write_archive, write_context, ARCHIVE_DIR, ARCHIVE_NAME};
use super::checks::{constraint_issues, intent_errors, plan_check_for};
use super::decisions::{decision_errors, record_decisions, split_answer};
use super::links::{decision_links, link_gaps, links_setting, stage_links, write_links, LEDGER};
use super::speccheck::{baseline_drift, intent_errors_spec, open_spec, spec_baseline, spec_issues, BASELINE_KEY};
use super::policy::{abandoned_error, chain_errors, escalate_affected, extra_gates, followup_run_errors, has_recipe, load_profile, mods, policy_violations, profile_missing, recipe_mismatch, recipe_tamper_errors};
use super::py::{self, get, get_opt, idx, iter_or, pj, pname, s, s_or, truthy, R, NULL};
use super::trace::{envelopes, model_errors, process_trace};
use super::{dump, load, opt, opts, Ctx, GATED, HUMAN_ONLY, INDEX_NAME, NO_ENVELOPE, SDLC};
use crate::kb::PyErr;
use crate::{pack, pyfmt, recipe, schema};
use serde_json::{json, Map, Value};

/// 用法（參數不夠或指令不認得時印）。
pub const USAGE: &str = "把 workflow 的回傳存進 run 目錄，並記錄 SDLC 閘門（Workflow 腳本不能寫檔，由主 session 呼叫）。

run 目錄：<runs_dir>/<YYYYMMDD>-<事件>-<短名>/
    run.json            事件、目標 repo、recipe、各段狀態、閘門核准紀錄、取消紀錄（abandoned）、後續 run（followup_run）、結案（closed）
    context.md          這次 run 的共用背景（目標 repo、事件、需求原文或路徑、recipe 一行摘要）；所有 agent 共用，派工單不再重貼
    <stage>.json        該段 workflow 的完整回傳（每個 agent 的信封）
    <stage>/執行紀錄索引.md  依回傳的 trace 產生：每次派工一列（label、角色、phase、紀錄檔、成果檔、狀態）

用法：
    awf save-run init <run_dir> --event sdlc --target <repo> --requirement <描述或路徑>
    awf save-run stage <run_dir> <stage> [--profile <methodology.json>] < result.json   # 存回傳並逐一驗證信封
        重存某一段會清掉該段的核准（舊核准對應的是舊結果，已失效，要重新核准）。
        一份信封都沒有（agent 缺件）記成 invalid，不能核准；回傳帶 halted 時一律記成 blocked。
        stage plan：回傳的 recipe 寫進 run.json（resolved 由本工具依專案的 profile 重算），並檢查 intent。
        其他段：recipe 沒有這一段 → invalid（要加段走 escalate）。
        回傳帶 trace 時寫 <run_dir>/<stage>/執行紀錄索引.md 並核對紀錄檔。
    awf save-run approve <run_dir> <stage> --by <誰> --at <ISO 時間> [--note <備註>] [--profile <檔>]
        [--answer \"<問題>=<答覆>\"]… [--answer-scope epic|project] [--answer-expires <YYYY-MM-DD>]
        只有狀態 done 的段能核准；recipe 裡前面的每一段都要 done，有閘門的都要已核准。已取消的 run 不能核准。
        --by policy 表示依專案政策自動核准，--note 必須寫政策名稱；plan 段一律由人核准。
    awf save-run decide <run_dir> --question <問題> --answer <答覆> --by <誰> --at <ISO 時間> [--scope epic|project] [--expires <YYYY-MM-DD>]
    awf save-run escalate <run_dir> --recipe <名> [--modifiers a,b] [--size s] --by <誰> --at <ISO 時間> --reason <理由> [--profile <檔>]
        升級 recipe（只能加嚴）；之後已 done 的段改成 stale 並清掉核准。
    awf save-run followup <run_dir> --run <後續 run 目錄> --by <誰> --at <ISO 時間>
    awf save-run close <run_dir> --by <誰> --at <ISO 時間>
    awf save-run abandon <run_dir> --reason <為什麼取消> --by <誰> --at <ISO 時間>
    awf save-run archive <run_dir>          # 重寫 <run_dir>/archive/結案摘要.md（close、abandon 會自動寫）
    awf save-run status <run_dir> [--json]      # --json：給外部程式讀的快照（schemas/run-status.schema.json）
    awf save-run --selftest       # 在暫存目錄跑，不碰 repo
";

fn line(buf: &mut String, text: impl AsRef<str>) {
    buf.push_str(text.as_ref());
    buf.push('\n');
}

fn or_empty_obj(v: &Value) -> Value {
    if truthy(v) { v.clone() } else { json!({}) }
}

/// 新的一段。`extra` 是 profile 的 `gates.extra`（[`extra_gates`]）：只會讓更多段有閘門。
fn pending(stage: &str, extra: &[&str]) -> Value {
    json!({"status": "pending", "gate": GATED.contains(&stage) || extra.contains(&stage)})
}

/// 這次用的 profile 另外加了閘門的段（讀不到、不合規則的 profile 當成沒有加；那種 profile 在別處已經報錯）。
fn gates_of(ctx: &Ctx, profile_arg: &str) -> Vec<&'static str> {
    extra_gates(&load_profile(ctx, profile_arg).0)
}

/// `data["stages"]`（可改）。
fn stages_mut(data: &mut Value) -> R<&mut Value> {
    match data {
        Value::Object(o) => o.get_mut("stages").ok_or_else(|| py::key_err("stages")),
        other => Err(py::type_err(format!("'{}' object is not subscriptable", py::tname(other)))),
    }
}

/// `data["stages"][stage]` 這個物件（可改）。
fn st_mut<'a>(data: &'a mut Value, stage: &str) -> R<&'a mut Map<String, Value>> {
    let stages = stages_mut(data)?;
    let st = match stages {
        Value::Object(o) => o.get_mut(stage).ok_or_else(|| py::key_err(stage))?,
        other => return Err(py::attr_err(other, "get")),
    };
    py::obj_mut(st)
}

/// 依 recipe 重建 run.json 的 stages：recipe 有的段沒有就補 pending（skipped 的改回 pending），沒有的段標 skipped。
fn rebuild_stages(data: &mut Value, resolved: &Value, extra: &[&str]) -> R<()> {
    let want = get(resolved, "stages")?.clone();
    let d = py::obj_mut(data)?;
    let stages = d.entry("stages").or_insert_with(|| json!({}));
    for st_name in SDLC {
        let st = get(stages, st_name)?;
        let wanted = truthy(&want) && py::contains(&want, st_name)?;
        let new = if wanted {
            if !st.is_object() || get(st, "status")?.as_str() == Some("skipped") { Some(pending(st_name, extra)) } else { None }
        } else {
            Some(json!({"status": "skipped", "gate": false}))
        };
        if let Some(v) = new {
            py::obj_mut(stages)?.insert(st_name.to_string(), v);
        }
    }
    let old = py::obj_mut(stages)?;
    let mut ordered = Map::new();
    for st_name in SDLC {
        ordered.insert(st_name.to_string(), old.get(st_name).cloned().unwrap_or(Value::Null));
    }
    for (k, v) in old.iter() {
        if !SDLC.contains(&k.as_str()) {
            ordered.insert(k.clone(), v.clone());
        }
    }
    *stages = Value::Object(ordered);
    Ok(())
}

/// `x[:n]` 之後用「；」接起來（清單的元素要是字串；字串會被拆成字）。
fn joined_head(v: &Value, n: usize) -> R<String> {
    py::join("；", &py::slice_head(v, n)?)
}

fn is_os(e: &PyErr) -> bool {
    matches!(
        e.class.as_str(),
        "OSError" | "FileNotFoundError" | "PermissionError" | "FileExistsError" | "IsADirectoryError" | "NotADirectoryError" | "InterruptedError" | "TimeoutError" | "BrokenPipeError"
    )
}

/// `SDLC.index(x)`
fn sdlc_index(v: &Value) -> R<usize> {
    SDLC.iter().position(|x| v.as_str() == Some(*x)).ok_or_else(|| PyErr::other("ValueError", format!("{} is not in list", pyfmt::repr(v))))
}

/// CLI 的本體：stdout 的文字寫進 `out`、stderr 的寫進 `err`，回傳 exit code。`stdin` 只有 `stage` 會讀。
pub fn run(ctx: &Ctx, argv: &[String], stdin: &mut dyn FnMut() -> R<String>, out: &mut String, err: &mut String) -> i32 {
    if argv.first().map(String::as_str) == Some("--selftest") {
        return super::selftest::selftest(ctx, out);
    }
    match dispatch(ctx, argv, stdin, out, err) {
        Ok(code) => code,
        Err(e) => {
            // 0.x 的 Python 版在這裡是沒接住的例外（traceback、exit 1）
            line(err, format!("awf save-run：{}：{}", e.class, e.msg));
            1
        }
    }
}

fn dispatch(ctx: &Ctx, argv: &[String], stdin: &mut dyn FnMut() -> R<String>, out: &mut String, err: &mut String) -> R<i32> {
    let (Some(cmd), Some(run_arg)) = (argv.first(), argv.get(1)) else {
        line(out, USAGE);
        return Ok(2);
    };
    let run = py::pp(run_arg);
    let run = run.as_str();
    if cmd == "init" {
        ctx.mkdir_p(run)?;
        let file = pj(run, "run.json");
        if ctx.exists(&file) {
            line(out, format!("已存在：{file}"));
            return Ok(0);
        }
        let event = opt(argv, "--event", "");
        let mut stages = Map::new();
        if event == "sdlc" {
            let extra = gates_of(ctx, "");
            for st in SDLC {
                stages.insert(st.to_string(), pending(st, &extra));
            }
        } else {
            stages.insert("run".to_string(), pending("run", &[]));
        }
        let mut data = Map::new();
        data.insert("event".into(), json!(event));
        data.insert("target_repo".into(), json!(opt(argv, "--target", "")));
        data.insert("requirement".into(), json!(opt(argv, "--requirement", "")));
        data.insert("stages".into(), Value::Object(stages));
        let data = Value::Object(data);
        dump(ctx, run, &data)?;
        write_context(ctx, run, &data)?;
        line(out, format!("已建立 {file} 與 {}", pj(run, "context.md")));
        return Ok(0);
    }
    let data = load(ctx, run)?;
    match cmd.as_str() {
        "stage" => {
            let Some(stage) = argv.get(2) else {
                line(out, USAGE);
                return Ok(2);
            };
            match py::loads(&stdin()?) {
                Ok(result) => stage_cmd(ctx, run, stage, &result, opt(argv, "--profile", ""), data, out, err),
                // 回傳讀不懂：不能當成「這次沒存」而留下舊的結果與舊核准（1.0 審查 M3）
                Err(e) => unreadable_cmd(ctx, run, stage, &format!("{}：{}", e.class, e.msg), data, out, err),
            }
        }
        "decide" => decide_cmd(ctx, run, argv, out, err),
        "approve" => {
            let Some(stage) = argv.get(2) else {
                line(out, USAGE);
                return Ok(2);
            };
            approve_cmd(ctx, run, stage, argv, data, out, err)
        }
        "escalate" => escalate_cmd(ctx, run, argv, data, out, err),
        "followup" => followup_cmd(ctx, run, argv, data, out, err),
        "close" => close_cmd(ctx, run, argv, data, out, err),
        "abandon" => abandon_cmd(ctx, run, argv, data, out, err),
        "archive" => {
            // 重寫摘要（結案或取消之後內容有補，或當時沒寫成）；只讀 run.json 與各段結果
            let warn = write_archive(ctx, run, &data);
            if warn.is_empty() {
                line(out, format!("已寫 {}/{ARCHIVE_DIR}/{ARCHIVE_NAME}", pname(run)));
                Ok(0)
            } else {
                line(err, format!("注意：{warn}"));
                Ok(1)
            }
        }
        "status" => status_cmd(ctx, run, argv, &data, out),
        _ => {
            line(out, USAGE);
            Ok(2)
        }
    }
}

/// 存一段的結果：寫 `<run>/<stage>.json`、驗證信封、依 trace 寫執行紀錄索引、更新 run.json。回傳 exit code（有錯是 1）。
/// `result` 是 workflow 的回傳；`profile` 是 `--profile` 的值（空字串＝用專案的 profile）。
pub fn stage(ctx: &Ctx, run: &str, stage: &str, result: &Value, profile: &str, out: &mut String, err: &mut String) -> R<i32> {
    let run = py::pp(run);
    let data = load(ctx, &run)?;
    stage_cmd(ctx, &run, stage, result, profile, data, out, err)
}

/// 這一段 workflow 的回傳不是工具讀得懂的 JSON（NaN、超出範圍的整數、巢狀太深、寫壞）。
/// 讀不懂不等於沒有重跑：這一段標成 invalid、舊核准失效、舊的結果檔換成一份說明，回傳 1。
/// 之後這一段不能核准（狀態不是 done），後面各段的核准也因為閘門鏈過不了；重跑或修正回傳後再存一次才會恢復。
/// run.json 沒有這一段時什麼都不改（不因為讀不懂的輸入多出一段）。
pub fn stage_unreadable(ctx: &Ctx, run: &str, stage: &str, why: &str, out: &mut String, err: &mut String) -> R<i32> {
    let run = py::pp(run);
    let data = load(ctx, &run)?;
    unreadable_cmd(ctx, &run, stage, why, data, out, err)
}

fn unreadable_cmd(ctx: &Ctx, run: &str, stage: &str, why: &str, mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let known = matches!(get_opt(&data, "stages")?, Some(st) if st.get(stage).is_some_and(Value::is_object));
    if !known {
        line(err, format!("{stage}：workflow 的回傳讀不懂：{why}。run.json 沒有 {stage} 段，沒有存"));
        return Ok(1);
    }
    let note = format!("workflow 的回傳讀不懂：{why}");
    let placeholder = json!({"halted": true, "halt_reason": note, "unreadable": true});
    ctx.write_text(&pj(run, &format!("{stage}.json")), &format!("{}\n", py::dumps(&placeholder, Some(2))))?;
    let old = {
        let st = st_mut(&mut data, stage)?;
        st.insert("status".into(), json!("invalid"));
        st.insert("envelopes".into(), json!(0));
        st.insert("unreadable".into(), json!(note));
        st.shift_remove("halted");
        st.shift_remove("approved").unwrap_or(Value::Null)
    };
    dump(ctx, run, &data)?;
    line(err, format!("{stage}：{note}。不採用：這一段標成 invalid，舊的結果不再算數；重跑這一段，或修正回傳後再存一次"));
    if truthy(&old) {
        line(out, format!("{stage}：舊核准（{} @ {}）已失效，要重新核准", s(get(&old, "by")?), s(get(&old, "at")?)));
    }
    line(out, format!("{stage}：0 份信封，狀態 invalid"));
    Ok(1)
}

#[allow(clippy::too_many_arguments)]
fn stage_cmd(ctx: &Ctx, run: &str, stage: &str, result: &Value, profile_arg: &str, mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    ctx.write_text(&pj(run, &format!("{stage}.json")), &format!("{}\n", py::dumps(result, Some(2))))?;
    let mut errs: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let envs = envelopes(result);
    if envs.is_empty() {
        errs.push(NO_ENVELOPE.to_string());
    }
    let schemas = schema::Schemas::new(ctx.abs(&ctx.teams));
    for (i, env) in envs.iter().enumerate() {
        let found = schemas.check(env, None).map_err(|m| PyErr::other("OSError", m))?;
        errs.extend(found.into_iter().map(|e| format!("信封 {i}：{e}")));
    }
    let has_status = |want: &str| envs.iter().any(|e| e.get("status").and_then(Value::as_str) == Some(want));
    let sdlc_run = match get_opt(&data, "stages")? {
        Some(stages) => py::contains(stages, "plan")?,
        None => false,
    };
    let in_sdlc = SDLC.contains(&stage);
    // run.json 已有 recipe 的非 plan 段：專案的 profile 找不到或不能用，當下就拒絕。
    // recipe 是用那份 profile 解析的；沒有它，這一段的規格檢查、連結檢查、限制文件都做不了，不能等到核准才發現
    if in_sdlc && sdlc_run && stage != "plan" && has_recipe(&data)? {
        if profile_arg.is_empty() && !ctx.is_file(&ctx.profile) {
            errs.push(profile_missing(&ctx.profile));
        } else {
            let (_, perr) = load_profile(ctx, profile_arg);
            if !perr.is_empty() {
                errs.push(perr);
            }
        }
    }
    // 規格層（需求來源是 openspec）與連結（profile 有 links）：沒設定就都是 None，後面完全不做事
    let (sprof, _) = if in_sdlc && sdlc_run { load_profile(ctx, profile_arg) } else { (Value::Null, String::new()) };
    let sc = if sprof.is_object() { open_spec(ctx, run, &data, &sprof) } else { None };
    let links_on = links_setting(&sprof);
    if stage == "plan" && sdlc_run {
        let (profile, perr) = load_profile(ctx, profile_arg);
        if !perr.is_empty() {
            errs.push(perr);
        }
        let inc = if result.is_object() { get(result, "recipe")? } else { &NULL };
        let cur = if has_recipe(&data)? { Some(idx(&data, "recipe")?.clone()) } else { None };
        if inc.is_object() {
            let name = if truthy(get(inc, "name")?) { get(inc, "name")? } else { get(inc, "kind")? };
            let size = if truthy(get(inc, "size")?) { get(inc, "size")?.clone() } else { Value::Null };
            let r = recipe::resolve(&profile, name, &Value::Array(mods(get(inc, "modifiers")?)?), &size).map_err(py::from_recipe)?;
            errs.extend(py::iter(idx(&r, "errors")?)?.iter().map(|e| format!("recipe：{}", s(e))));
            let reasons = match get(inc, "reasons")? {
                v @ Value::Array(_) => v.clone(),
                _ => json!([]),
            };
            let entry = |resolved: Value| -> R<Value> {
                let mut m = Map::new();
                m.insert("name".into(), idx(&r, "name")?.clone());
                m.insert("modifiers".into(), idx(&r, "modifiers")?.clone());
                m.insert("size".into(), idx(&r, "size")?.clone());
                m.insert("reasons".into(), reasons.clone());
                m.insert("resolved_from".into(), idx(&r, "resolved_from")?.clone());
                m.insert("resolved".into(), resolved);
                m.insert("locked".into(), json!(false));
                m.insert("history".into(), json!([]));
                Ok(Value::Object(m))
            };
            let stages = idx(&data, "stages")?;
            let locked = match &cur {
                Some(c) => truthy(get(c, "locked")?) || truthy(get(c, "history")?),
                None => false,
            };
            if let (Some(c), true) = (&cur, locked) {
                let new_key = json!([idx(&r, "name")?, py::sorted(py::iter(idx(&r, "modifiers")?)?)?, idx(&r, "size")?]);
                let size = if truthy(get(c, "size")?) { get(c, "size")?.clone() } else { json!("") };
                let old_key = json!([get(c, "name")?, py::sorted(iter_or(get(c, "modifiers")?)?)?, size]);
                if !pyfmt::eq(&new_key, &old_key) {
                    let why = if truthy(get(c, "locked")?) { "plan 已核准過" } else { "escalate 過" };
                    errs.push(format!("recipe 已鎖定（{why}）：不能從 {} 換成 {}，要改走 awf save-run escalate", s(get(c, "name")?), s(idx(&r, "name")?)));
                }
            } else if cur.is_none() && {
                let plan = or_empty_obj(get(stages, "plan")?);
                let mut moved = truthy(get(&plan, "approved")?);
                if !moved {
                    for x in SDLC.iter().skip(1) {
                        let st = or_empty_obj(get(stages, x)?);
                        let status = get(&st, "status")?;
                        if !(status.is_null() || matches!(status.as_str(), Some("pending" | "skipped"))) {
                            moved = true;
                            break;
                        }
                    }
                }
                moved
            } {
                // 舊 run（沒有 recipe，當成 feature）已核准過 plan 或已經往下跑：補記 recipe 不能比 feature 少段（X3 M4：不能繞過 escalate 降級）
                let feat = recipe::resolve(&profile, &json!("feature"), &NULL, &NULL).map_err(py::from_recipe)?;
                let lost = recipe::escalate_errors(&r, &feat).map_err(py::from_recipe)?;
                if let Some(first) = lost.first() {
                    errs.push(format!("這個舊 run 已經當成 feature 往下跑：{first}"));
                } else {
                    let merged = recipe::merge_stricter(&r, &feat).map_err(py::from_recipe)?;
                    let rec = entry(merged)?;
                    py::obj_mut(&mut data)?.insert("recipe".into(), rec);
                }
            } else {
                let rec = entry(r.clone())?;
                py::obj_mut(&mut data)?.insert("recipe".into(), rec);
            }
        } else if cur.is_none() {
            warns.push("回傳沒有 recipe（舊 workflow）：這個 run 當成 feature 的完整流程".to_string());
        }
        let res = recipe::recipe_of_run(&data).map_err(py::from_recipe)?;
        let mode = get_opt(&res, "plan_mode")?.cloned().unwrap_or_else(|| json!("feature"));
        match &sc {
            // 需求來源是 openspec：proposal 就是 intent（節名用對照表對應，必要內容一樣不能少）
            Some(sc) => errs.extend(intent_errors_spec(ctx, sc, result, &profile, &mode)?),
            None => errs.extend(intent_errors(ctx, result, &profile, &mode)?),
        }
        if has_recipe(&data)? {
            let resolved = idx(idx(&data, "recipe")?, "resolved")?.clone();
            rebuild_stages(&mut data, &resolved, &gates_of(ctx, profile_arg))?;
            write_context(ctx, run, &data)?;
        }
    } else if in_sdlc && has_recipe(&data)? && {
        let resolved = idx(idx(&data, "recipe")?, "resolved")?;
        match get_opt(resolved, "stages")? {
            Some(v) => !py::contains(v, stage)?,
            None => true,
        }
    } {
        // recipe 沒有的段：只存檔、判錯，run.json 仍標 skipped（不讓它看起來像該段存在）
        line(err, format!("不能記錄：recipe {} 沒有 {stage} 段（要加段走 awf save-run escalate）；回傳只存成 {stage}.json", s(get(idx(&data, "recipe")?, "name")?)));
        return Ok(1);
    }
    if in_sdlc && stage != "plan" && has_recipe(&data)? && result.is_object() {
        errs.extend(recipe_mismatch(&data, result)?.into_iter().map(|e| format!("recipe 不一致：{e}")));
    }
    if in_sdlc && sdlc_run && result.is_object() {
        let (mprof, _) = load_profile(ctx, profile_arg);
        let mods_v = if has_recipe(&data)? {
            let m = get(idx(&data, "recipe")?, "modifiers")?;
            if truthy(m) { m.clone() } else { json!([]) }
        } else {
            let rec = get(result, "recipe")?;
            Value::Array(if rec.is_object() { mods(get(rec, "modifiers")?)? } else { Vec::new() })
        };
        errs.extend(model_errors(stage, result, &mprof, &mods_v)?.into_iter().map(|e| format!("模型分級：{e}")));
    }
    let target_v = get(&data, "target_repo")?;
    let target = if truthy(target_v) { Some(py::pp(&s(target_v))) } else { None };
    let target_abs = target.as_deref().map(|t| ctx.abs(t));
    let requirement = s_or(get(&data, "requirement")?, "");
    if stage == "plan" && result.is_object() {
        // 調研重用：不採信 workflow 的說法，重新核對沿用的角度（來源 hash、信封、紀錄檔）
        let bad = pack::verify_reuse(&ctx.abs(run), target_abs.as_deref(), result, Some(&requirement), &ctx.pack_root);
        errs.extend(bad.into_iter().map(|e| format!("調研重用：{e}")));
    }
    // st = data["stages"].setdefault(stage, {...})
    match stages_mut(&mut data)? {
        Value::Object(o) => {
            if !o.contains_key(stage) {
                o.insert(stage.to_string(), pending(stage, &[]));
            }
        }
        other => return Err(py::attr_err(other, "setdefault")),
    }
    // profile 的 gates.extra（只能加閘門）：存檔當下 profile 有列的段一律標成有閘門，
    // 就算這個 run 是在加這項設定之前建立的。拿掉設定不會把已經標上的閘門拿掉。
    if in_sdlc && sdlc_run && gates_of(ctx, profile_arg).contains(&stage) {
        st_mut(&mut data, stage)?.insert("gate".into(), json!(true));
    }
    let halted = if result.is_object() { get(result, "halted")?.clone() } else { Value::Null };
    let status = if !errs.is_empty() {
        "invalid"
    } else if has_status("needs_decision") {
        "needs_decision"
    } else if has_status("blocked") || truthy(&halted) {
        "blocked"
    } else {
        "done"
    };
    let old = {
        let st = st_mut(&mut data, stage)?;
        st.insert("status".into(), json!(status));
        st.insert("envelopes".into(), json!(envs.len()));
        st.shift_remove("unreadable");
        let old = st.shift_remove("approved");
        if truthy(&halted) {
            let reason = get(result, "halt_reason")?;
            let text = s(if truthy(reason) { reason } else { &halted });
            line(err, format!("{stage}：workflow 中斷（halted）：{text}"));
            st.insert("halted".into(), json!(text));
        } else {
            st.shift_remove("halted");
        }
        old.unwrap_or(Value::Null)
    };
    let (has_trace, rows, mut miss) = process_trace(ctx, run, stage, result, true)?;
    if in_sdlc {
        let (cprof, _) = load_profile(ctx, profile_arg);
        let (c_issues, c_checks) = constraint_issues(ctx, run, stage, &cprof, target.as_deref(), true)?;
        let pc = {
            let st = st_mut(&mut data, stage)?;
            if c_issues.is_empty() {
                st.shift_remove("constraint_issues");
            } else {
                for m in &c_issues {
                    line(err, format!("限制文件：{m}（不能政策核准）"));
                }
                st.insert("constraint_issues".into(), json!(c_issues));
            }
            if c_checks.is_empty() {
                st.shift_remove("constraint_checks");
            } else {
                st.insert("constraint_checks".into(), Value::Array(c_checks));
            }
            plan_check_for(ctx, run, stage, result, &data)?
        };
        let st = st_mut(&mut data, stage)?;
        if pc.is_empty() {
            st.shift_remove("plan_check");
        } else {
            for m in &pc {
                line(err, format!("拆工檢查：{m}（不能政策核准；人工核准前請先確認）"));
            }
            st.insert("plan_check".into(), json!(pc));
        }
        if let Some(sc) = &sc {
            let si = spec_issues(stage, result, sc, &data)?;
            let st = st_mut(&mut data, stage)?;
            if si.is_empty() {
                st.shift_remove("spec_issues");
            } else {
                for m in &si {
                    line(err, format!("規格檢查：{m}（不能政策核准；人工核准前請先確認）"));
                }
                st.insert("spec_issues".into(), json!(si));
            }
        }
        if let Some(enforce) = links_on {
            let gaps = link_gaps(ctx, run, stage, sc.as_ref().and_then(|r| r.as_ref().ok()), &data);
            let st = st_mut(&mut data, stage)?;
            if gaps.is_empty() {
                st.shift_remove("link_gaps");
            } else {
                let tail = if enforce { "不能政策核准；人工核准前請先確認" } else { "links.enforce 沒開，只提醒" };
                for m in &gaps {
                    line(err, format!("連結檢查：{m}（{tail}）"));
                }
                st.insert("link_gaps".into(), json!(gaps));
            }
        }
    }
    let with_recipe = has_recipe(&data)?;
    {
        let st = st_mut(&mut data, stage)?;
        if !has_trace && with_recipe && in_sdlc {
            miss = vec!["這個 run 有 recipe，回傳卻沒有 trace（無法核對每個 agent 的紀錄）".to_string()];
            st.insert("missing_records".into(), json!(miss));
            line(err, format!("執行紀錄缺件：{}（不能政策核准）", miss.first().map_or("", String::as_str)));
        } else if !has_trace {
            warns.push("回傳沒有 trace（舊 workflow）：不產生執行紀錄索引、不核對紀錄檔".to_string());
            st.shift_remove("missing_records");
        } else {
            if miss.is_empty() {
                st.shift_remove("missing_records");
            } else {
                st.insert("missing_records".into(), json!(miss));
            }
            line(out, format!("{stage}：執行紀錄索引 {}（{} 列）", pj(&pj(run, stage), INDEX_NAME), rows.len()));
            let ghost: Vec<&str> = rows.iter().filter(|r| r.status.contains("agent 沒寫紀錄檔")).map(|r| r.label.as_str()).collect();
            if ghost.is_empty() {
                st.shift_remove("ghostwritten_records");
            } else {
                st.insert("ghostwritten_records".into(), json!(ghost));
                line(out, format!("{stage}：{} 個 agent 有交件但沒寫紀錄檔，已依信封代寫（內容較簡略）：{}", ghost.len(), ghost.iter().take(8).copied().collect::<Vec<_>>().join("、")));
            }
            for m in &miss {
                line(err, format!("執行紀錄缺件：{m}（不能政策核准）"));
            }
        }
    }
    dump(ctx, run, &data)?;
    if links_on.is_some() && in_sdlc {
        // 連結帳本是輔助：寫不了只警告（閘門的連結檢查自己從各段結果重算，不讀帳本）
        let links = stage_links(ctx, run, stage, result, sc.as_ref().and_then(|r| r.as_ref().ok()));
        if let Err(e) = write_links(ctx, run, stage, &links, true) {
            warns.push(format!("連結帳本 {LEDGER} 寫不了（{}：{}）；這一段的結果已存", e.class, e.msg));
        }
    }
    if stage == "plan" && errs.is_empty() && result.is_object() {
        // 記下這次調研讀過的來源檔（sha1），下次重跑時來源沒變的角度可以沿用；記不了只警告（I9）
        match pack::record_reuse(&ctx.abs(run), target_abs.as_deref(), result, &requirement, &ctx.pack_root) {
            Ok(ws) => warns.extend(ws.into_iter().map(|w| format!("調研重用：{w}"))),
            Err(e) => warns.push(format!("調研重用的紀錄寫不了（{}：{}），下次會全部重做", e.class, e.msg)),
        }
    }
    for e in &errs {
        line(err, e);
    }
    for w in &warns {
        line(err, format!("警告：{w}"));
    }
    if truthy(&old) {
        line(out, format!("{stage}：重存結果，舊核准（{} @ {}）已失效，要重新核准", s(get(&old, "by")?), s(get(&old, "at")?)));
    }
    if truthy(get(&data, "abandoned")?) {
        line(out, format!("注意：{}，這一段的結果只存檔，不能核准", abandoned_error(&data)?));
    }
    let st = get(idx(&data, "stages")?, stage)?;
    let needs = truthy(get(st, "gate")?) && idx(st, "status")?.as_str() == Some("done");
    line(out, format!("{stage}：{} 份信封，狀態 {}{}", envs.len(), s(idx(st, "status")?), if needs { "（需要使用者核准才能進下一段）" } else { "" }));
    Ok(if errs.is_empty() { 0 } else { 1 })
}

fn decide_cmd(ctx: &Ctx, run: &str, argv: &[String], out: &mut String, err: &mut String) -> R<i32> {
    let (q, a, by, at) = (opt(argv, "--question", ""), opt(argv, "--answer", ""), opt(argv, "--by", ""), opt(argv, "--at", ""));
    let (scope, exp) = (opt(argv, "--scope", "epic"), opt(argv, "--expires", ""));
    let items = vec![(q.to_string(), a.to_string())];
    let mut errs: Vec<String> = if by.is_empty() || at.is_empty() { vec!["--by 與 --at 都必填".to_string()] } else { Vec::new() };
    errs.extend(decision_errors(&items, by, scope, exp));
    if !errs.is_empty() {
        for e in errs {
            line(err, format!("不能記錄：{e}"));
        }
        return Ok(1);
    }
    match record_decisions(ctx, run, &items, by, at, scope, exp, "", err) {
        Ok(_) => {}
        Err(e) if is_os(&e) => {
            line(err, format!("不能記錄：{}", e.msg));
            return Ok(1);
        }
        Err(e) => return Err(e),
    }
    line(out, format!("已記 1 筆決定（{}；範圍 {scope}）", pack::DECISIONS));
    note_decision_links(ctx, run, "decide", 1, opt(argv, "--profile", ""), err);
    Ok(0)
}

/// 決策 → 需求的連結（profile 有 links 才記）。輔助：任何失敗只警告。
fn note_decision_links(ctx: &Ctx, run: &str, stage: &str, count: usize, profile_arg: &str, err: &mut String) {
    let (prof, _) = load_profile(ctx, profile_arg);
    if links_setting(&prof).is_none() {
        return;
    }
    let Ok(data) = load(ctx, run) else { return };
    let sc = open_spec(ctx, run, &data, &prof);
    let links = decision_links(ctx, run, stage, count, sc.as_ref().and_then(|r| r.as_ref().ok()));
    if let Err(e) = write_links(ctx, run, stage, &links, false) {
        line(err, format!("警告：連結帳本 {LEDGER} 寫不了（{}：{}）；決定已記錄", e.class, e.msg));
    }
}

/// 核准一段（`argv` 是完整的命令列：`approve <run> <stage> --by … --at … [--note …] [--profile …] [--answer …]…`）。
/// 回傳 exit code：0＝已記錄核准；1＝拒絕（理由印在 `err`，每行以「不能核准：」開頭）。
pub fn approve(ctx: &Ctx, argv: &[String], out: &mut String, err: &mut String) -> R<i32> {
    let (Some(run), Some(stage)) = (argv.get(1), argv.get(2)) else {
        line(out, USAGE);
        return Ok(2);
    };
    let run = py::pp(run);
    let data = load(ctx, &run)?;
    approve_cmd(ctx, &run, stage, argv, data, out, err)
}

fn approve_cmd(ctx: &Ctx, run: &str, stage: &str, argv: &[String], mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let (by, at, note) = (opt(argv, "--by", ""), opt(argv, "--at", ""), opt(argv, "--note", ""));
    let profile_arg = opt(argv, "--profile", "");
    let st = get(idx(&data, "stages")?, stage)?.clone();
    let mut errs: Vec<String> = Vec::new();
    if truthy(get(&data, "abandoned")?) {
        errs.push(abandoned_error(&data)?);
    }
    if st.is_null() {
        errs.push(format!("沒有 {stage} 這一段"));
    } else if idx(&st, "status")?.as_str() != Some("done") {
        errs.push(format!("{stage} 狀態是 {}，只有 done 的段能核准", s(idx(&st, "status")?)));
    } else if !truthy(get(&st, "envelopes")?) {
        errs.push(format!("{stage} {NO_ENVELOPE}"));
    }
    if by.is_empty() || at.is_empty() {
        errs.push("--by 與 --at 都必填".to_string());
    }
    if stage != "plan" {
        errs.extend(recipe_tamper_errors(ctx, &data, profile_arg)?);
    }
    if by == "policy" && note.is_empty() {
        errs.push("--by policy 時 --note 要寫依據的政策名稱".to_string());
    }
    let answers: Vec<(String, String)> = opts(argv, "--answer").iter().map(|a| split_answer(a)).collect();
    let (a_scope, a_exp) = (opt(argv, "--answer-scope", "epic"), opt(argv, "--answer-expires", ""));
    if !answers.is_empty() {
        errs.extend(decision_errors(&answers, by, a_scope, a_exp));
    }
    if by == "policy" && !st.is_null() {
        let f = pj(run, &format!("{stage}.json"));
        if !ctx.is_file(&f) {
            errs.push(format!("找不到 {}，政策核准要讀該段的實際結果", pname(&f)));
        } else {
            let result = ctx.load_json(&f)?;
            errs.extend(policy_violations(ctx, stage, &result, Some(run))?.into_iter().map(|e| format!("政策判定：{e}")));
            let (cprof, cperr) = load_profile(ctx, profile_arg);
            if !cperr.is_empty() {
                errs.push(cperr);
            }
            let target = get(&data, "target_repo")?;
            let crepo = if truthy(target) { Some(py::pp(&s(target))) } else { None };
            errs.extend(constraint_issues(ctx, run, stage, &cprof, crepo.as_deref(), true)?.0.into_iter().map(|e| format!("限制文件：{e}")));
            let result = ctx.load_json(&f)?;
            errs.extend(plan_check_for(ctx, run, stage, &result, &data)?.into_iter().map(|e| format!("拆工檢查：{e}")));
            if cprof.is_object() {
                // 規格檢查與連結檢查都重算，不採信 run.json 記的結果
                let sc = open_spec(ctx, run, &data, &cprof);
                if let Some(sc) = &sc {
                    errs.extend(spec_issues(stage, &result, sc, &data)?.into_iter().map(|e| format!("規格檢查：{e}")));
                }
                if links_setting(&cprof) == Some(true) {
                    errs.extend(link_gaps(ctx, run, stage, sc.as_ref().and_then(|r| r.as_ref().ok()), &data).into_iter().map(|e| format!("連結檢查：{e}")));
                }
            }
        }
        if HUMAN_ONLY.contains(&stage) && !errs.iter().any(|e| e.contains("一律由人核准")) {
            errs.push(format!("{stage} 段一律由人核准，不能政策核准"));
        }
    }
    errs.extend(chain_errors(&data, stage)?);
    if !errs.is_empty() {
        for e in errs {
            line(err, format!("不能核准：{e}"));
        }
        return Ok(1);
    }
    let mut approved = Map::new();
    approved.insert("by".into(), json!(by));
    approved.insert("at".into(), json!(at));
    approved.insert("note".into(), json!(note));
    approved.insert("kind".into(), json!(if by == "policy" { "policy" } else { "human" }));
    // 規格層（需求來源是 openspec 才有；沒選就什麼都不做）：plan、design 核准時把規格現在的樣子記成基準，
    // 之後各段重算時和它比。人工核准 test、deliver 時也重算一次，和基準不同只警告（政策核准在上面已經擋了）。
    let mut spec_warns: Vec<String> = Vec::new();
    if SDLC.contains(&stage) {
        let (bprof, _) = load_profile(ctx, profile_arg);
        let bsc = if bprof.is_object() { open_spec(ctx, run, &data, &bprof) } else { None };
        if matches!(stage, "plan" | "design") {
            match spec_baseline(bsc.as_ref()) {
                Some(Ok(b)) => {
                    approved.insert(BASELINE_KEY.into(), b);
                }
                Some(Err(e)) => spec_warns.push(format!("{stage} 核准了，但規格基準沒記成（{e}）；之後各段的規格檢查會把「沒有基準」當成問題")),
                None => {}
            }
        } else if by != "policy" {
            if let Some(Ok(sc)) = &bsc {
                if let Some(c) = sc.change.as_deref() {
                    let known = iter_or(get(&st, "spec_issues")?)?.iter().map(s).collect::<Vec<_>>();
                    for m in baseline_drift(stage, &data, &sc.conf, &sc.opened, c).into_iter().filter(|m| !known.contains(m)) {
                        spec_warns.push(format!("{stage} 存檔之後規格又變了（人工核准照記，但請先確認）：{m}"));
                    }
                }
            }
        }
    }
    st_mut(&mut data, stage)?.insert("approved".into(), Value::Object(approved));
    let lock = stage == "plan" && has_recipe(&data)?;
    if lock {
        if let Some(rec) = data.get_mut("recipe") {
            py::obj_mut(rec)?.insert("locked".into(), json!(true));
        }
    }
    dump(ctx, run, &data)?;
    line(out, format!("{stage} 已記錄核准：{by} @ {at}"));
    for w in &spec_warns {
        line(err, format!("警告：{w}"));
    }
    if !answers.is_empty() {
        // 決策記憶是輔助：寫不出來不影響核准
        match record_decisions(ctx, run, &answers, by, at, a_scope, a_exp, stage, err) {
            Ok(n) => {
                line(out, format!("已記 {n} 筆決定（{}；範圍 {a_scope}）", pack::DECISIONS));
                note_decision_links(ctx, run, &format!("approve:{stage}"), n, profile_arg, err);
            }
            Err(e) if is_os(&e) => line(err, format!("警告：決定沒記成（{}）；核准已記錄", e.msg)),
            Err(e) => return Err(e),
        }
    }
    if lock {
        line(out, format!("recipe 已鎖定：{}；之後要改只能 awf save-run escalate（只能加嚴）", recipe_summary(idx(&data, "recipe")?)?));
    }
    let st = get(idx(&data, "stages")?, stage)?;
    for (key, text) in [("missing_records", "有 {n} 個執行紀錄缺件"), ("constraint_issues", "有 {n} 項限制文件的問題")] {
        let v = get(st, key)?;
        if truthy(v) {
            line(err, format!("警告：{stage} {}（人工核准照記，但請先確認）：{}", text.replace("{n}", &py::len(v)?.to_string()), joined_head(v, 5)?));
        }
    }
    let v = get(st, "plan_check")?;
    if truthy(v) {
        line(err, format!("警告：{stage} 的拆工檢查有 {} 項問題（人工核准照記，但請先確認）：{}", py::len(v)?, joined_head(v, 5)?));
    }
    for (key, text) in [("spec_issues", "的規格檢查有 {n} 項問題"), ("link_gaps", "有 {n} 條連結不齊")] {
        let v = get(st, key)?;
        if truthy(v) {
            line(err, format!("警告：{stage} {}（人工核准照記，但請先確認）：{}", text.replace("{n}", &py::len(v)?.to_string()), joined_head(v, 5)?));
        }
    }
    Ok(0)
}

fn escalate_cmd(ctx: &Ctx, run: &str, argv: &[String], mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let (name, by, at, reason) = (opt(argv, "--recipe", ""), opt(argv, "--by", ""), opt(argv, "--at", ""), opt(argv, "--reason", ""));
    let mut errs: Vec<String> = Vec::new();
    if name.is_empty() || by.is_empty() || at.is_empty() || reason.is_empty() {
        errs.push("--recipe、--by、--at、--reason 都必填".to_string());
    }
    if truthy(get(&data, "abandoned")?) {
        errs.push(abandoned_error(&data)?);
    }
    let sdlc_run = match get_opt(&data, "stages")? {
        Some(stages) => py::contains(stages, "plan")?,
        None => false,
    };
    if !sdlc_run {
        errs.push("不是 SDLC run（沒有 plan 段），沒有 recipe 可升級".to_string());
    }
    let (profile, perr) = load_profile(ctx, opt(argv, "--profile", ""));
    if !perr.is_empty() {
        errs.push(perr);
    }
    if !errs.is_empty() {
        for e in errs {
            line(err, format!("不能升級：{e}"));
        }
        return Ok(1);
    }
    let size = opt(argv, "--size", "");
    let size = if size.is_empty() { Value::Null } else { json!(size) };
    let new = recipe::resolve(&profile, &json!(name), &Value::Array(mods(&json!(opt(argv, "--modifiers", "")))?), &size).map_err(py::from_recipe)?;
    let old_rec = if has_recipe(&data)? { idx(&data, "recipe")?.clone() } else { json!({}) };
    let old = recipe::recipe_of_run(&data).map_err(py::from_recipe)?;
    let mut errs: Vec<String> = py::iter(idx(&new, "errors")?)?.iter().map(|e| format!("recipe：{}", s(e))).collect();
    errs.extend(recipe::escalate_errors(&new, &old).map_err(py::from_recipe)?);
    if !errs.is_empty() {
        for e in errs {
            line(err, format!("不能升級：{e}"));
        }
        return Ok(1);
    }
    let merged = recipe::merge_stricter(&new, &old).map_err(py::from_recipe)?;
    let (added, changed) = escalate_affected(&merged, &old, &profile)?;
    let mut first = SDLC.len();
    let mut added_idx: Vec<usize> = Vec::new();
    for a in &added {
        added_idx.push(sdlc_index(a)?);
    }
    for c in &changed {
        first = first.min(sdlc_index(&json!(c))?);
    }
    first = added_idx.iter().copied().fold(first, usize::min);
    let mut stale: Vec<&str> = Vec::new();
    let extra_g = extra_gates(&profile);
    for (i, st_name) in SDLC.iter().enumerate() {
        let stages = stages_mut(&mut data)?;
        let st = get(stages, st_name)?;
        if added.iter().any(|a| a.as_str() == Some(*st_name)) {
            py::obj_mut(stages)?.insert(st_name.to_string(), pending(st_name, &extra_g));
        } else if st.is_object() && get(st, "status")?.as_str() == Some("done") && (added_idx.iter().any(|a| i > *a) || changed.iter().any(|c| c == st_name) || i > first) {
            let st = st_mut(&mut data, st_name)?;
            st.insert("status".into(), json!("stale"));
            st.shift_remove("approved");
            stale.push(st_name);
        }
    }
    rebuild_stages(&mut data, &merged, &extra_g)?;
    let label = |r: &Value| -> R<String> {
        let m = get(r, "modifiers")?;
        Ok(format!("{}{}", s(get(r, "name")?), if truthy(m) { format!("+{}", py::join("+", &py::iter(m)?)?) } else { String::new() }))
    };
    let (frm, to) = (label(&old)?, label(&merged)?);
    let mut hist = iter_or(get(&old_rec, "history")?)?;
    hist.push(json!({"from": frm, "to": to, "by": by, "at": at, "reason": reason}));
    let plan = or_empty_obj(get(idx(&data, "stages")?, "plan")?);
    let locked = truthy(get(&old_rec, "locked")?) || truthy(get(&plan, "approved")?);
    let reasons = get(&old_rec, "reasons")?;
    let mut rec = Map::new();
    rec.insert("name".into(), idx(&merged, "name")?.clone());
    rec.insert("modifiers".into(), idx(&merged, "modifiers")?.clone());
    rec.insert("size".into(), get_opt(&merged, "size")?.cloned().unwrap_or_else(|| json!("")));
    rec.insert("reasons".into(), if truthy(reasons) { reasons.clone() } else { json!([]) });
    rec.insert("resolved_from".into(), idx(&new, "resolved_from")?.clone());
    rec.insert("resolved".into(), merged.clone());
    rec.insert("locked".into(), json!(locked));
    rec.insert("history".into(), Value::Array(hist));
    py::obj_mut(&mut data)?.insert("recipe".into(), Value::Object(rec));
    dump(ctx, run, &data)?;
    write_context(ctx, run, &data)?;
    let added_text = py::join("、", &added)?;
    let changed_text = changed.join("、");
    line(out, format!("recipe 已升級：{frm} → {to}；新加的段：{}；設定變嚴的段：{}", if added_text.is_empty() { "無" } else { &added_text }, if changed_text.is_empty() { "無" } else { &changed_text }));
    if !stale.is_empty() {
        line(out, format!("改成 stale（核准已清除，要重跑）：{}", stale.join("、")));
    }
    Ok(0)
}

fn followup_cmd(ctx: &Ctx, run: &str, argv: &[String], mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let (frun, by, at) = (opt(argv, "--run", ""), opt(argv, "--by", ""), opt(argv, "--at", ""));
    if frun.is_empty() || by.is_empty() || at.is_empty() {
        line(err, "不能記錄：--run、--by、--at 都必填");
        return Ok(1);
    }
    let fpath = py::pp(frun);
    if !ctx.is_file(&pj(&fpath, "run.json")) {
        line(err, format!("不能記錄：後續 run {frun} 沒有 run.json（先 awf save-run init）"));
        return Ok(1);
    }
    let ferr = followup_run_errors(ctx, &fpath)?;
    if !ferr.is_empty() {
        line(err, format!("不能記錄：{ferr}"));
        return Ok(1);
    }
    if ctx.resolve(&fpath)? == ctx.resolve(run)? {
        line(err, "不能記錄：後續 run 不能是自己");
        return Ok(1);
    }
    py::obj_mut(&mut data)?.insert("followup_run".into(), json!({"run": frun, "by": by, "at": at}));
    dump(ctx, run, &data)?;
    line(out, format!("已記錄後續 run：{frun}（{by} @ {at}）"));
    Ok(0)
}

/// 結案或取消之後重寫知識圖的 vault（自動接點；沒有圖時什麼都不做、什麼都不印；失敗只警告）。
fn vault_after(ctx: &Ctx, out: &mut String, err: &mut String) {
    match crate::state::auto::refresh_vault(&ctx.abs(&ctx.project_root())) {
        Some((true, msg)) => line(out, msg),
        Some((false, msg)) => line(err, format!("注意：{msg}")),
        None => {}
    }
}

fn close_cmd(ctx: &Ctx, run: &str, argv: &[String], mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let (by, at) = (opt(argv, "--by", ""), opt(argv, "--at", ""));
    let mut errs: Vec<String> = Vec::new();
    if by.is_empty() || at.is_empty() {
        errs.push("--by 與 --at 都必填".to_string());
    }
    let r = recipe::recipe_of_run(&data).map_err(py::from_recipe)?;
    let needs_followup = truthy(get(&r, "followup")?);
    let fr = get(&data, "followup_run")?;
    if needs_followup && !truthy(fr) {
        errs.push(format!("recipe {} 要求事後開後續 run 補完整流程，run.json 沒有 followup_run（先 awf save-run followup）", s(get(&r, "name")?)));
    } else if needs_followup {
        let ferr = followup_run_errors(ctx, &py::pp(&s_or(get(fr, "run")?, "")))?;
        if !ferr.is_empty() {
            errs.push(ferr);
        }
    }
    // 結案要整條流程走完（X3 I1）：recipe 的段都 done、有閘門的都已核准、有 merge 段的要 merge done
    if truthy(get(&data, "abandoned")?) {
        errs.push(abandoned_error(&data)?);
    }
    errs.extend(chain_errors(&data, "merge")?.into_iter().map(|e| format!("流程還沒走完：{e}")));
    let want = get(&r, "stages")?;
    if truthy(want) && py::contains(want, "merge")? {
        let empty = json!({});
        let stages = get_opt(&data, "stages")?.unwrap_or(&empty);
        let merge = or_empty_obj(get(stages, "merge")?);
        if get(&merge, "status")?.as_str() != Some("done") {
            errs.push("流程還沒走完：merge 段還沒 done".to_string());
        }
    }
    if !errs.is_empty() {
        for e in errs {
            line(err, format!("不能結案：{e}"));
        }
        return Ok(1);
    }
    py::obj_mut(&mut data)?.insert("closed".into(), json!({"by": by, "at": at}));
    dump(ctx, run, &data)?;
    line(out, format!("已結案：{by} @ {at}"));
    let warn = write_archive(ctx, run, &data);
    if warn.is_empty() {
        line(out, format!("摘要：{}/{ARCHIVE_DIR}/{ARCHIVE_NAME}（知識庫下次 build 會收進去）", pname(run)));
    } else {
        line(err, format!("注意：{warn}"));
    }
    vault_after(ctx, out, err);
    Ok(0)
}

fn abandon_cmd(ctx: &Ctx, run: &str, argv: &[String], mut data: Value, out: &mut String, err: &mut String) -> R<i32> {
    let (reason, by, at) = (opt(argv, "--reason", ""), opt(argv, "--by", ""), opt(argv, "--at", ""));
    if reason.is_empty() || by.is_empty() || at.is_empty() {
        line(err, "不能取消：--reason、--by、--at 都必填");
        return Ok(1);
    }
    if truthy(get(&data, "abandoned")?) {
        line(out, format!("已經取消過：{}（保留原紀錄）", abandoned_error(&data)?));
        return Ok(0);
    }
    py::obj_mut(&mut data)?.insert("abandoned".into(), json!({"by": by, "at": at, "reason": reason}));
    dump(ctx, run, &data)?;
    line(out, format!("已取消：{by} @ {at}：{reason}；之後不能核准、不能推送"));
    let warn = write_archive(ctx, run, &data);
    if warn.is_empty() {
        line(out, format!("摘要：{}/{ARCHIVE_DIR}/{ARCHIVE_NAME}", pname(run)));
    } else {
        line(err, format!("注意：{warn}"));
    }
    vault_after(ctx, out, err);
    Ok(0)
}

/// `f"{v:15}"`：字串靠左補空白；數字靠右；其他型別 Python 會丟 TypeError。
fn pad15(v: &Value) -> R<String> {
    match v {
        Value::String(t) => Ok(format!("{t:<15}")),
        Value::Bool(b) => Ok(format!("{:>15}", i32::from(*b))),
        Value::Number(_) => Ok(format!("{:>15}", s(v))),
        other => Err(py::type_err(format!("unsupported format string passed to {}.__format__", py::tname(other)))),
    }
}

fn status_cmd(ctx: &Ctx, run: &str, argv: &[String], data: &Value, out: &mut String) -> R<i32> {
    if argv.iter().any(|a| a == "--json") {
        line(out, py::dumps(&status_snapshot(ctx, run, data)?, Some(1)));
        return Ok(0);
    }
    let abandoned = truthy(get(data, "abandoned")?);
    if abandoned {
        line(out, format!("已取消：{}", abandoned_error(data)?));
    }
    if has_recipe(data)? {
        line(out, format!("recipe：{}", recipe_summary(idx(data, "recipe")?)?));
    } else {
        line(out, "recipe：（沒有記錄，當成 feature 的完整流程）");
    }
    let stages = match idx(data, "stages")? {
        Value::Object(o) => o,
        other => return Err(py::attr_err(other, "items")),
    };
    for (name, st) in stages {
        let gate = if truthy(get(st, "gate")?) { "閘門" } else { "" };
        let ok = if truthy(get(st, "approved")?) { format!("核准 {}", s(idx(idx(st, "approved")?, "by")?)) } else { String::new() };
        line(out, format!("{name:<8} {} {gate} {ok}", pad15(idx(st, "status")?)?));
        for (key, text) in [("missing_records", "執行紀錄缺件 {n} 個"), ("constraint_issues", "限制文件 {n} 項")] {
            let v = get(st, key)?;
            if truthy(v) {
                line(out, format!("         警告：{}（不能政策核准）：{}", text.replace("{n}", &py::len(v)?.to_string()), joined_head(v, 3)?));
            }
        }
        let v = get(st, "spec_issues")?;
        if truthy(v) {
            line(out, format!("         警告：規格檢查 {} 項（不能政策核准）：{}", py::len(v)?, joined_head(v, 3)?));
        }
        let v = get(st, "link_gaps")?;
        if truthy(v) {
            line(out, format!("         警告：連結不齊 {} 項（links.enforce 開啟時不能政策核准）：{}", py::len(v)?, joined_head(v, 3)?));
        }
    }
    let fr = get(data, "followup_run")?;
    if truthy(fr) {
        line(out, format!("後續 run：{}", s(get(fr, "run")?)));
    }
    let closed = get(data, "closed")?;
    if truthy(closed) {
        line(out, format!("已結案：{} @ {}", s(get(closed, "by")?), s(get(closed, "at")?)));
    }
    if abandoned {
        line(out, "下一步：無（已取消）");
        return Ok(0);
    }
    let nxt = next_stage(data)?.filter(|n| !n.is_empty());
    line(out, format!("下一步：{}", nxt.as_deref().unwrap_or("全部完成")));
    Ok(0)
}
