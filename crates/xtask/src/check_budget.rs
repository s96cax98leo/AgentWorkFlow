//! token 預算檢查：派工數與派工單字數不能默默變多。由 `tests/check_budget.py` 移植。
//!
//! 做法：在暫存目錄用預設 profile init 一個專案，乾跑全部 workflow（check_workflows.mjs 的 AWF_SCENARIO_REPORT），
//! 算每支 workflow 的 agent 數（有 recipe:feature 情境就量它，否則量預設情境）與派工單總字數，另算每個內建 recipe
//! 從 plan 到 deliver 的總派工數，和 `plugin/scaffold/.claude/teams/golden-set/token-budget.json` 的基準比：超過基準 10% 就失敗。
//!
//! 限制：乾跑量的是「派出去的字」與「派幾個 agent」，量不到 agent 實際讀檔與輸出的長度。
//! 字數以 code point 計（同 Python 的 `len()`）。
//!
//! 用法：
//!   cargo xtask check-budget                    # 比對基準
//!   cargo xtask check-budget --update           # 改了流程、確認變多是預期的之後，寫回基準
//!   cargo xtask check-budget --budget <檔>      # 改用別的基準檔（測試用）
//!
//! 和 Python 版不同的地方（只會更嚴）：報告或基準的格式不對（缺欄位、不是數字），Python 版直接當掉，這裡印一行說明、
//! exit 1；報告裡有不成對的代理字元（serde_json 不收）同樣判失敗。

use crate::tempproj;
use crate::util::{self, Captured};
use awf_core::merge_gate::pyutil;
use awf_core::{pyfmt, recipe};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const TOLERANCE: f64 = 0.10;
const STAGES: [&str; 5] = ["plan", "design", "build", "test", "deliver"];

/// 乾跑全部 workflow，回 check_workflows.mjs 的情境報告。init 與產生腳本在行程內呼叫 awf-core（見 [`crate::tempproj`]）。
fn scenario_report(root: &Path) -> Result<Value, String> {
    let r = tempproj::rendered(root, "awf-budget.")?;
    let rep = r.tmp.path().join("report.json");
    let p = util::run(
        tempproj::check_workflows(root, &r.proj, &r.wf_dir).env("AWF_SCENARIO_REPORT", &rep),
        None,
    )?;
    if !rep.is_file() {
        return Err(format!("check_workflows 沒有產出報告：{}", pyutil::tail(&format!("{}{}", p.stdout, p.stderr), 1500)));
    }
    util::read_json(&rep)
}

/// 一組派工單的共同開頭有幾個字（`len(os.path.commonprefix(ps))`，以 code point 計）；只有一張時算 0。
fn shared_prefix(prompts: &[&str]) -> usize {
    let Some((first, rest)) = prompts.split_first() else { return 0 };
    if rest.is_empty() {
        return 0;
    }
    let mut n = first.chars().count();
    for p in rest {
        n = n.min(first.chars().zip(p.chars()).take_while(|(a, b)| a == b).count());
    }
    n
}

fn scenarios<'a>(wf: &str, v: &'a Value) -> Result<&'a [Value], String> {
    v.get("scenarios").and_then(Value::as_array).map(Vec::as_slice).ok_or_else(|| format!("報告的 {wf} 沒有 scenarios"))
}

fn sc_name(sc: &Value) -> &str {
    sc.get("name").and_then(Value::as_str).unwrap_or_default()
}

fn label_count(wf: &str, sc: &Value) -> Result<usize, String> {
    sc.get("labels").and_then(Value::as_array).map(Vec::len).ok_or_else(|| format!("報告的 {wf} 情境「{}」沒有 labels", sc_name(sc)))
}

/// 一個情境的量測值：agent 數、派工單總字數、共同前綴字數。
fn scenario_numbers(wf: &str, sc: &Value) -> Result<Value, String> {
    let bad = || format!("報告的 {wf} 情境「{}」的 prompts 格式不對", sc_name(sc));
    let prompts = sc.get("prompts").and_then(Value::as_object).ok_or_else(bad)?;
    let ps: Vec<&str> = prompts.values().map(|p| p.as_str().ok_or_else(bad)).collect::<Result<_, _>>()?;
    let chars: usize = ps.iter().map(|p| p.chars().count()).sum();
    Ok(json!({"agents": label_count(wf, sc)?, "prompt_chars": chars, "shared_prefix_chars": shared_prefix(&ps)}))
}

/// 把情境報告整理成和基準同一個形狀：`{"workflows": {...}, "recipes": {...}}`。
pub fn summarize(data: &Value) -> Result<Value, String> {
    let data = data.as_object().ok_or("情境報告不是 JSON 物件")?;
    let mut names: Vec<&String> = data.keys().collect();
    names.sort();
    let mut workflows = Map::new();
    for wf in names {
        let all = scenarios(wf, &data[wf.as_str()])?;
        // 有 recipe:feature 情境就量它（完整走一遍）；沒有才用預設情境（預設乾跑的假回傳可能在開頭就 halted）
        let sc = all.iter().find(|x| sc_name(x) == "recipe:feature").or_else(|| all.first()).ok_or_else(|| format!("報告的 {wf} 沒有任何情境"))?;
        workflows.insert(wf.clone(), scenario_numbers(wf, sc)?);
        // 一支腳本處理多種類型時（event.js），名字是「類型：<名>」的情境各量一次，記成 <檔名>#<類型>
        for x in all {
            if let Some(kind) = sc_name(x).strip_prefix("類型：") {
                workflows.insert(format!("{wf}#{kind}"), scenario_numbers(wf, x)?);
            }
        }
    }
    let bi = recipe::builtin_profile();
    let mut recipes = Map::new();
    for name in recipe::builtin_recipes().as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
        let r = recipe::resolve(&bi, &Value::String(name.clone()), &Value::Null, &Value::Null).map_err(|e| format!("recipe {name} 解析出錯：{e}"))?;
        let mut total: Option<usize> = Some(0);
        for st in pyfmt::items(r.get("stages")) {
            let Some(st) = st.as_str().filter(|s| STAGES.contains(s)) else { continue };
            let wf = format!("sdlc-{st}.js");
            let want = format!("recipe:{name}");
            let found = match data.get(&wf) {
                Some(v) => v.get("scenarios").and_then(Value::as_array).and_then(|a| a.iter().find(|s| sc_name(s) == want)),
                None => None,
            };
            match (found, total) {
                (Some(sc), Some(t)) => total = Some(t + label_count(&wf, sc)?),
                _ => {
                    total = None;
                    break;
                }
            }
        }
        recipes.insert(name, json!({"agents_plan_to_deliver": total}));
    }
    Ok(json!({"workflows": workflows, "recipes": recipes}))
}

fn num(v: Option<&Value>, what: &str) -> Result<f64, String> {
    v.and_then(Value::as_f64).ok_or_else(|| format!("{what} 不是數字"))
}

fn section<'a>(v: &'a Value, key: &str, what: &str) -> Result<Option<&'a Map<String, Value>>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Object(o)) => Ok(Some(o)),
        Some(_) => Err(format!("{what}的 {key} 不是物件")),
    }
}

/// 量到的值和基準比。回（是否通過、要印的文字）；格式不對回 `Err`。
pub fn compare(cur: &Value, base: &Value) -> Result<(bool, String), String> {
    let empty = Map::new();
    let cur_wf = section(cur, "workflows", "量測結果")?.unwrap_or(&empty);
    let cur_rec = section(cur, "recipes", "量測結果")?.unwrap_or(&empty);
    let mut errs: Vec<String> = Vec::new();
    for (wf, b) in section(base, "workflows", "基準")?.unwrap_or(&empty) {
        let Some(c) = cur_wf.get(wf) else {
            errs.push(format!("{wf}：量不到（workflow 不見了？）"));
            continue;
        };
        for k in ["agents", "prompt_chars"] {
            let have = num(c.get(k), &format!("量測結果 {wf} 的 {k}"))?;
            let want = num(b.get(k), &format!("基準 {wf} 的 {k}"))?;
            if have > want * (1.0 + TOLERANCE) {
                errs.push(format!("{wf}：{k} {}，超過基準 {} 的 {}%", pyfmt::str_opt(c.get(k)), pyfmt::str_opt(b.get(k)), (TOLERANCE * 100.0) as i64));
            }
        }
    }
    let agents = |v: &'_ Value| v.get("agents_plan_to_deliver").cloned().filter(|x| !x.is_null());
    for (name, b) in section(base, "recipes", "基準")?.unwrap_or(&empty) {
        if !b.is_object() {
            return Err(format!("基準 recipe {name} 不是物件"));
        }
        let Some(want) = agents(b) else { continue };
        let c = cur_rec.get(name).and_then(agents);
        let want_n = num(Some(&want), &format!("基準 recipe {name} 的派工數"))?;
        let over = match &c {
            None => true,
            Some(v) => num(Some(v), &format!("量測結果 recipe {name} 的派工數"))? > want_n * (1.0 + TOLERANCE),
        };
        if over {
            errs.push(format!("recipe {name}：plan→deliver 派 {} 個 agent，基準 {}", pyfmt::str_opt(c.as_ref()), pyfmt::str_of(&want)));
        }
    }
    let shown: Vec<String> = cur_rec.iter().map(|(n, v)| format!("{n} {}", pyfmt::str_opt(v.get("agents_plan_to_deliver")))).collect();
    let mut out = format!("各 recipe plan→deliver 派工數：{}\n", shown.join("、"));
    if let Some(feat) = cur_rec.get("feature").and_then(|v| v.get("agents_plan_to_deliver")).filter(|v| pyfmt::truthy(v)) {
        out.push_str(&format!("（feature＝{}；乾跑量的是派工數與派工單字數，量不到 agent 實際讀寫的 token）\n", pyfmt::str_of(feat)));
    }
    if !errs.is_empty() {
        out.push_str("token 預算超標：\n");
        for e in &errs {
            out.push_str(&format!("  - {e}\n"));
        }
        out.push_str("確認是預期的變多後，用 cargo xtask check-budget --update 更新基準\n");
        return Ok((false, out));
    }
    out.push_str("token 預算檢查通過\n");
    Ok((true, out))
}

/// 量一次：乾跑全部 workflow 並整理成和基準同一個形狀。
pub fn measure(root: &Path) -> Result<Value, String> {
    summarize(&scenario_report(root)?)
}

/// 指令本體：`root` 是 repo 根目錄，`args` 是 `--update`、`--budget <檔>`。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    let fail = |msg: String| (1, String::new(), format!("{msg}\n"));
    let (custom, rest) = match util::take_opt(args, "--budget") {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let update = rest.iter().any(|a| a == "--update");
    let default_rel = "plugin/scaffold/.claude/teams/golden-set/token-budget.json";
    let (budget, shown) = match custom {
        Some(p) => (PathBuf::from(&p), p),
        None => (util::teams_dir(root).join("golden-set").join("token-budget.json"), default_rel.to_string()),
    };
    if util::which("node").is_none() {
        return (1, "找不到 node，無法量測（fail-closed）\n".into(), String::new());
    }
    let cur = match measure(root) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if update {
        let text = format!("{}\n", recipe::dumps(&cur, Some(1)));
        return match std::fs::write(&budget, text) {
            Ok(()) => (0, format!("已寫回基準 {shown}\n"), String::new()),
            Err(e) => fail(format!("寫不了基準 {shown}：{e}")),
        };
    }
    if !budget.is_file() {
        return (1, format!("沒有基準 {shown}；確認數字後用 --update 建立\n"), String::new());
    }
    let base = match util::read_json(&budget) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    match compare(&cur, &base) {
        Ok((ok, out)) => (if ok { 0 } else { 1 }, out, String::new()),
        Err(e) => fail(e),
    }
}
