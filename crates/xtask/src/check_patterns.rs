//! pattern 庫（`snippets/patterns.js`）的單元檢查。由 `tests/check_patterns.py` 移植。
//!
//! 用 node 載入 challenge、recipe、kernel、patterns 四個片段，給假的 agent()（本體在 `js/patterns.cjs`，原樣搬來），
//! 六個 pattern 各檢查：沒給上限會 throw、正常、達上限回 halted、缺件回 halted；另檢查執行核心的派工數上限在 pattern
//! 裡也生效、角色白名單以外的 agent 會 throw。
//!
//! 自訂腳本端到端：組裝出來的腳本真的能跑（片段之間沒有撞名）、派工經過核心、上限在執行時生效，而且靜態檢查被繞過
//! 之後包裝仍然擋得住。檢查與組裝用 [`awf_core::script`]（`check`、`assemble`）與 [`awf_core::build`]（片段、schema）。
//!
//! 沒有 node 時判失敗（不略過）。和 Python 版不同的地方（只會更嚴）：node 回的結果缺 `calls`／`out`、格式不對時，
//! Python 版直接當掉，這裡列成一條失敗。

use crate::util::{self, Captured, TempDir};
use awf_core::build::{self, dumps_compact};
use awf_core::merge_gate::pyutil;
use awf_core::{pyfmt, recipe, script};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

const JS: &str = include_str!("js/patterns.cjs");
const RUN_JS: &str = include_str!("js/patterns_run.cjs");
const SNIPPETS: [&str; 4] = ["challenge", "recipe", "kernel", "patterns"];
const NS: &str = "agent-workflow:";
const LANG: &str = "繁體中文";

/// 檢查並組裝一支自訂腳本，回可以執行的腳本文字。
///
/// TODO(v1.0 第 7 批)：這是 `awf.py` 的 `render_custom` 接 preamble 的那一段（狀態機還沒有 Rust 版）照抄過來的，
/// 專案設定固定成測試用的值（AGENT_NS、輸出語言、沒有擴充 agent）。狀態機移植後改成直接呼叫 awf-core 的版本，
/// 不要留兩份 preamble。
fn render_custom(root: &Path, text: &str, stage: &str, args: &Value, methodology: &Value) -> Result<String, String> {
    let snippet_dir = util::snippet_dir(root);
    let mut cache = build::SnippetCache::new();
    let mut snippets: Vec<(&str, String)> = Vec::new();
    for n in script::SNIPPETS {
        let body = build::load_snippet(n, &snippet_dir, &mut cache).map_err(|e| format!("片段 {n} 載入失敗：{e:?}"))?;
        snippets.push((n, body));
    }
    let roles: Vec<String> = script::ROLES.iter().map(|r| format!("{NS}{r}")).collect();
    let allowed: BTreeSet<String> = roles.iter().cloned().collect();
    let errs = script::check(text, &allowed, NS, &script::defined_names(snippets.iter().map(|(_, s)| s.as_str())));
    if !errs.is_empty() {
        let head: Vec<&str> = errs.iter().take(20).map(String::as_str).collect();
        return Err(format!("自訂腳本沒通過檢查，不執行：\n  - {}", head.join("\n  - ")));
    }
    let cap = methodology.get("limits").and_then(|l| l.get("max_agents")).and_then(Value::as_i64).filter(|n| *n > 0).unwrap_or(40);
    let js = |v: &Value| dumps_compact(v);
    let s = js(&Value::String(stage.to_string()));
    let mut pre: Vec<String> = vec![
        format!("// ── 以下由 awf.py 接上（自訂腳本，{stage} 段）：執行核心、pattern 庫、這一段的參數。不要手改 ──"),
        format!("const NS = {}", js(&Value::String(NS.to_string()))),
        format!("const A = Object.assign({}, args || {{}})", js(args)),
        format!("const METHODOLOGY = {}", js(methodology)),
        "const MP = METHODOLOGY || {}".to_string(),
    ];
    pre.extend(snippets.iter().map(|(n, body)| format!("// ── 片段 {n} ──\n{body}")));
    let roles_js = js(&Value::Array(roles.into_iter().map(Value::String).collect()));
    pre.extend([
        "const R = resolveRecipe(METHODOLOGY, A.recipe, [].concat(A.modifiers || []), A.size)".to_string(),
        format!(
            "if (R.errors.length) return {{ stage: {s}, halted: true, halt_reason: 'recipe 解析有錯：' + R.errors.join('；'), missing: [], \
             recipe: {{ name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages }}, trace: [] }}"
        ),
        "const RECIPE = { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages, summary: recipeSummary(R) }".to_string(),
        format!(
            "awfInit({{ run_dir: A.run_dir, stage: {s}, lang: {}, rules: '.claude/teams/rules.md', constraints: awfConstraints(MP, {s}) }})",
            js(&Value::String(LANG.to_string()))
        ),
        format!("awfKernelInit({{ profile: METHODOLOGY, stage: {s}, modifiers: [].concat(A.modifiers || []), max_agents: {cap}, roles: {roles_js} }})"),
    ]);
    let out = script::assemble(text, &pre.join("\n"), stage).map_err(|e| format!("組裝失敗：{}", e.0))?;
    build::embed_schemas(&out, &util::teams_dir(root)).map_err(|e| format!("schema 內嵌失敗：{e:?}"))
}

/// 測試用的自訂腳本與組裝結果（給對照用）：上限 `cap`（None＝profile 原樣）。
pub fn sample_script(root: &Path, cap: Option<i64>) -> Result<String, String> {
    let default = util::read_json(&util::teams_dir(root).join("methodologies").join("default.json"))?;
    render_custom(root, &good_source(), "build", &sample_args(), &with_cap(&default, cap))
}

fn good_source() -> String {
    script::GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'")
}

fn sample_args() -> Value {
    json!({"run_dir": "_runs/r1", "epic": "demo", "recipe": "feature", "modifiers": []})
}

/// `prof["limits"] = {**limits, "max_agents": cap}`
fn with_cap(default: &Value, cap: Option<i64>) -> Value {
    let mut prof = default.clone();
    if let (Some(cap), Some(o)) = (cap, prof.as_object_mut()) {
        let mut limits = match o.get("limits") {
            Some(Value::Object(l)) => l.clone(),
            _ => Map::new(),
        };
        limits.insert("max_agents".into(), json!(cap));
        o.insert("limits".into(), Value::Object(limits));
    }
    prof
}

/// 用 node 執行一支組裝好的腳本，回最後一行的 JSON（`{calls, out}` 或 `{error}`）；讀不懂回 `Err(輸出的開頭)`。
fn run_script(node: &Path, file: &Path, text: &str, limit: usize) -> Result<Value, String> {
    std::fs::write(file, text).map_err(|e| format!("寫不了 {}：{e}", file.display()))?;
    let r = util::node_eval(node, RUN_JS, &[file.to_path_buf()], None)?;
    let last = pyutil::splitlines(pyutil::strip(&r.stdout)).last().copied().unwrap_or_default();
    serde_json::from_str::<Value>(last).map_err(|_| pyutil::head(&format!("{}{}", r.stdout, r.stderr), limit).to_string())
}

fn short(v: &Value, n: usize) -> String {
    pyutil::head(&recipe::dumps(v, None), n).to_string()
}

fn obj_or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(x) if pyfmt::truthy(x) => x.clone(),
        _ => json!({}),
    }
}

fn has_error(res: &Value) -> bool {
    pyfmt::truthy_opt(res.get("error"))
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// 自訂腳本端到端：回失敗的理由（空的＝通過）。
fn custom_end_to_end(root: &Path, node: &Path) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    let default = match util::read_json(&util::teams_dir(root).join("methodologies").join("default.json")) {
        Ok(d) => d,
        Err(e) => return vec![format!("讀不到預設 profile：{e}")],
    };
    let tmp = match TempDir::new("awf-cs.") {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    let src = good_source();
    let args = sample_args();
    for (cap, want_halt) in [(None, false), (Some(2), true)] {
        let cap_s = cap.map(|c: i64| c.to_string()).unwrap_or_else(|| "None".into());
        let text = match render_custom(root, &src, "build", &args, &with_cap(&default, cap)) {
            Ok(t) => t,
            Err(e) => {
                errs.push(format!("自訂腳本（上限 {cap_s}）組不起來：{e}"));
                continue;
            }
        };
        let res = match run_script(node, &tmp.path().join(format!("custom-build-{cap_s}.js")), &text, 600) {
            Ok(r) => r,
            Err(e) => {
                errs.push(format!("自訂腳本（上限 {cap_s}）跑不起來：{e}"));
                continue;
            }
        };
        if has_error(&res) {
            errs.push(format!("自訂腳本（上限 {cap_s}）執行錯誤：{}", pyutil::head(&pyfmt::str_opt(res.get("error")), 600)));
            continue;
        }
        let (Some(out_raw), Some(calls)) = (res.get("out"), res.get("calls")) else {
            errs.push(format!("自訂腳本（上限 {cap_s}）的結果缺 out 或 calls：{}", short(&res, 400)));
            continue;
        };
        let out = obj_or_empty(Some(out_raw));
        let trace: Vec<Value> = pyfmt::items(out.get("trace")).to_vec();
        if pyfmt::truthy_opt(out.get("halted")) != want_halt {
            errs.push(format!("自訂腳本（上限 {cap_s}）halted 應該是 {}：{}", py_bool(want_halt), short(&out, 400)));
        }
        let recipe_name = out.get("recipe").filter(|r| pyfmt::truthy(r)).and_then(|r| r.get("name"));
        if recipe_name != Some(&json!("feature")) || out.get("stage") != Some(&json!("build")) {
            let shown = json!({"stage": out.get("stage"), "recipe": out.get("recipe")});
            errs.push(format!("自訂腳本的回傳要帶 stage 與 recipe：{}", short(&shown, 300)));
        }
        if !want_halt {
            let want_calls = json!(["Implement:t1", "Implement:t2", "Implement:t3", "ext"]);
            let through_kernel = trace.iter().all(|x| pyfmt::truthy_opt(x.get("tier")) && pyfmt::truthy_opt(x.get("record")));
            if *calls != want_calls || trace.len() != 4 || !through_kernel {
                errs.push(format!("自訂腳本的派工要經過核心（trace 有 tier 與紀錄檔）：{}、{}", pyfmt::repr(calls), short(&Value::Array(trace.clone()), 400)));
            }
        } else {
            let n_calls = calls.as_array().map(Vec::len);
            let capped = trace.iter().any(|x| pyfmt::truthy_opt(x.get("capped")));
            if n_calls != Some(2) || !capped || !pyfmt::str_opt(out.get("halt_reason")).contains("派工數到上限") {
                errs.push(format!("上限 2：只該派 2 個、其餘記 capped 並停下：{}、{}", pyfmt::repr(calls), pyfmt::str_opt(out.get("halt_reason"))));
            }
        }
    }
    // 靜態檢查被繞過之後（這裡直接改組裝好的腳本來模擬）：包裝仍然擋得住
    let text = match render_custom(root, &src, "build", &args, &default) {
        Ok(t) => t,
        Err(e) => {
            errs.push(format!("自訂腳本組不起來：{e}"));
            return errs;
        }
    };
    let go = |name: &str, edited: String, errs: &mut Vec<String>| -> Value {
        if edited == text {
            errs.push(format!("{name}：測試沒有改到腳本"));
        }
        match run_script(node, &tmp.path().join(format!("{name}.js")), &edited, 400) {
            Ok(v) => v,
            Err(e) => json!({"error": e}),
        }
    };
    let anchor = "const waves = [['t1'], ['t2', 't3']]";
    for (name, inject) in [
        ("direct-agent", "const sneaky = await agent('p', { label: 'sneak', agentType: 'agent-workflow:dev-merge-steward' })"),
        ("direct-tryagent", "const sneaky = await tryAgent('p', { label: 'sneak', agentType: 'agent-workflow:dev-merge-steward' })"),
        ("fake-trace", "awfState().trace.push({ label: 'fake', ok: true })"),
        ("reinit", "awfKernelInit({ max_agents: 0 })"),
    ] {
        let res = go(name, text.replace(anchor, &format!("{anchor}\n{inject}")), &mut errs);
        if !has_error(&res) || res.to_string().contains("sneak") {
            errs.push(format!("包裝沒擋住 {name}：{}", short(&res, 300)));
        }
    }
    let res = go(
        "forged",
        text.replace("  trace: awfTrace(),\n}", "  trace: [{ label: 'review:bugs', ok: true }], stage: 'deliver', recipe: { name: 'docs' },\n}"),
        &mut errs,
    );
    let out = obj_or_empty(res.get("out"));
    let trace = pyfmt::items(out.get("trace"));
    let recipe_name = out.get("recipe").filter(|r| pyfmt::truthy(r)).and_then(|r| r.get("name"));
    if has_error(&res)
        || out.get("stage") != Some(&json!("build"))
        || recipe_name != Some(&json!("feature"))
        || trace.len() != 4
        || trace.iter().any(|x| x.get("label") == Some(&json!("review:bugs")))
    {
        errs.push(format!("腳本自己給的 stage、recipe、trace 要被外層覆寫：{}", short(&res, 400)));
    }
    let head = "return {\n  stage: 'build', halted: false, results: done, first: pick(done), ext,";
    let res = go("no-object", text.replace(head, &format!("return 1\n{head}")), &mut errs);
    let out = obj_or_empty(res.get("out"));
    if !pyfmt::truthy_opt(out.get("halted")) || !pyfmt::str_opt(out.get("halt_reason")).contains("沒有回傳物件") || !out.get("trace").is_some_and(Value::is_array) {
        errs.push(format!("腳本沒有回傳物件要 halted：{}", short(&res, 300)));
    }
    errs
}

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path) -> Captured {
    let Some(node) = util::which("node") else {
        return (1, "check_patterns：找不到 node（workflow 片段要用 node 檢查）\n".into(), String::new());
    };
    let e2e = custom_end_to_end(root, &node);
    if !e2e.is_empty() {
        return (1, format!("check_patterns：自訂腳本端到端沒通過\n{}\n", e2e.join("\n")), String::new());
    }
    let dir = util::snippet_dir(root);
    let files: Vec<_> = SNIPPETS.iter().map(|n| dir.join(format!("{n}.js"))).collect();
    let r = match util::node_eval(&node, JS, &files, None) {
        Ok(r) => r,
        Err(e) => return (1, format!("check_patterns：沒通過\n{e}\n"), String::new()),
    };
    if r.code != 0 {
        let out = format!("{}{}", r.stdout, r.stderr);
        return (1, format!("check_patterns：沒通過\n{}\n", pyutil::strip(&out)), String::new());
    }
    (0, "check_patterns：六個 pattern 的上限、正常、達上限、缺件，核心上限與角色白名單，以及自訂腳本端到端都正確\n".into(), String::new())
}
