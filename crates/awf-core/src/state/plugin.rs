//! 找 plugin、讀 workflow.config、找範本、產生可以執行的 workflow 腳本（含自訂腳本的檢查與組裝）。

use super::{awf_err, is_dir, is_file, load_json, load_json_or, s_or, Env, Fail, A, ARGS_LINE, MERGED_HINT, PLUGIN_NAME, TEMPLATES};
use crate::kb::pyish::{self, WS_CLASS};
use crate::kb::PyErr;
use crate::save_run::py::{self, pj, R};
use crate::{build, script};
use regex::Regex;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub(crate) fn override_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"(?m)^// awf-override: *([^{WS_CLASS}]+)[{WS_CLASS}]*$")).expect("固定的正規式"))
}

pub(crate) fn ph_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{([A-Z_]+)\}\}").expect("固定的正規式"))
}

/// `Path(p).read_text(encoding="utf-8")`（絕對路徑）。
fn read_text(env: &Env, p: &str) -> R<String> {
    env.sr().read_text(p)
}

pub(crate) fn version(env: &Env, plugin: Option<&str>) -> R<String> {
    let Some(plugin) = plugin else { return Ok(String::new()) };
    match load_json(env, &pj(plugin, ".claude-plugin/plugin.json"))? {
        Some(d @ Value::Object(_)) => Ok(s_or(py::get(&d, "version")?)),
        _ => Ok(String::new()),
    }
}

/// Claude Code 登記的一筆 agent-workflow 安裝。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub path: String,
    pub version: String,
    pub mine: bool,
    pub other_project: bool,
    pub updated: String,
}

/// Claude Code 登記的 agent-workflow 安裝（installed_plugins.json；格式沒承諾穩定，讀不懂就回空）。
pub fn plugin_candidates(env: &Env, home: Option<&str>) -> R<Vec<Candidate>> {
    let home = home.unwrap_or(&env.home);
    let d = load_json(env, &env.abs(&pj(home, "plugins/installed_plugins.json")))?;
    let mut out = Vec::new();
    let Some(Value::Object(plugins)) = d.as_ref().and_then(|d| d.as_object()).and_then(|d| d.get("plugins")) else { return Ok(out) };
    let mine_root = pyish::resolve(&env.project).unwrap_or_else(|_| env.project.clone());
    for (key, entries) in plugins {
        if key.split('@').next() != Some(PLUGIN_NAME) {
            continue;
        }
        let list: Vec<&Value> = match entries {
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        };
        for e in list {
            let Some(o) = e.as_object() else { continue };
            let ip = o.get("installPath").filter(|v| py::truthy(v)).map(py::s);
            let Some(ip) = ip.filter(|ip| is_dir(&env.abs(&pj(ip, TEMPLATES)))) else { continue };
            let pp = o.get("projectPath").filter(|v| py::truthy(v)).map(py::s);
            let mine = pp.as_ref().is_some_and(|p| pyish::resolve(&env.abs(p)).unwrap_or_else(|_| p.clone()) == mine_root);
            out.push(Candidate {
                key: key.clone(),
                path: pyish::ppath(&ip),
                version: o.get("version").map(s_or).unwrap_or_default(),
                mine,
                other_project: pp.is_some() && !mine,
                updated: o.get("lastUpdated").map(s_or).unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// plugin 目錄（要有 workflow-templates/）：--plugin-dir、環境變數、執行檔自己所在的 plugin（或模版 repo 的）、Claude Code 登記的安裝。
/// 回傳（plugin 目錄或 None, 說明）。
pub fn find_plugin(env: &Env, explicit: &str) -> A<(Option<String>, Vec<String>)> {
    let mut notes: Vec<String> = Vec::new();
    for raw in [Some(explicit), env.plugin_env[0].as_deref(), env.plugin_env[1].as_deref()].into_iter().flatten() {
        if !raw.is_empty() && is_dir(&env.abs(&pj(raw, TEMPLATES))) {
            return Ok((Some(env.abs(&pyish::ppath(raw))), notes));
        }
    }
    if !explicit.is_empty() {
        return awf_err(format!("--plugin-dir {explicit} 底下沒有 {TEMPLATES}/"));
    }
    if let Some(p) = env.template_plugin() {
        // 執行檔自己所在的 plugin，或在模版 repo 裡直接跑
        return Ok((Some(p), notes));
    }
    let mut cands: Vec<Candidate> = plugin_candidates(env, None)?.into_iter().filter(|c| !c.other_project).collect();
    if cands.is_empty() {
        return Ok((None, notes));
    }
    let vfile = pj(&env.teams(), "VERSION");
    let want = if is_file(&vfile) { pyish::strip(&read_text(env, &vfile)?).to_string() } else { String::new() };
    // sort(reverse=True)：相同的維持原本的先後
    cands.sort_by(|a, b| (b.mine, b.version == want, &b.updated).cmp(&(a.mine, a.version == want, &a.updated)));
    let keys: BTreeSet<&str> = cands.iter().map(|c| c.key.as_str()).collect();
    let first = cands.first().cloned();
    if keys.len() > 1 {
        notes.push(format!(
            "裝了不只一個來源的 agent-workflow（{}），這次用 {}；用 --plugin-dir 可以指定",
            keys.into_iter().collect::<Vec<_>>().join("、"),
            first.as_ref().map(|c| c.key.as_str()).unwrap_or_default()
        ));
    }
    Ok((first.map(|c| env.abs(&c.path)), notes))
}

/// init 記下的佔位符值（KEY=value）。
pub fn read_config(env: &Env) -> R<Map<String, Value>> {
    let mut out = Map::new();
    let text = match read_text(env, &pj(&env.teams(), "workflow.config")) {
        Ok(t) => t,
        Err(e) if e.class == "UnicodeDecodeError" => return Err(e),
        Err(_) => return Ok(out),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    for line in crate::merge_gate::pyutil::splitlines(text) {
        let klen = line.bytes().take_while(|b| b.is_ascii_uppercase() || *b == b'_').count();
        if klen == 0 || line.as_bytes().get(klen) != Some(&b'=') {
            continue;
        }
        let line = line.trim_end_matches('\r');
        let (k, v) = (line.get(..klen).unwrap_or_default(), line.get(klen + 1..).unwrap_or_default());
        let v = if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
            v.get(1..v.len() - 1).unwrap_or_default()
        } else if v == "\"" {
            ""
        } else {
            v
        };
        out.insert(k.to_string(), json!(v));
    }
    Ok(out)
}

fn cfg_str(cfg: &Map<String, Value>, k: &str, default: &str) -> String {
    cfg.get(k).and_then(Value::as_str).unwrap_or(default).to_string()
}

fn name_ok(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]{0,63}\n?\z").expect("固定的正規式")).is_match(name)
}

/// （腳本文字, 來源, 說明）。來源：override（專案的覆寫檔）、plugin（範本）、legacy（找不到 plugin 時退回專案裡的舊副本）。
pub fn find_template(env: &Env, name: &str, plugin: Option<&str>) -> A<(String, &'static str, Vec<String>)> {
    if !name_ok(name) {
        return awf_err(format!("workflow 名稱不對：{name}"));
    }
    let mut notes: Vec<String> = Vec::new();
    let tpl = plugin.map(|p| pj(&pj(p, TEMPLATES), &format!("{name}.js")));
    let ov = pj(&env.project, &format!(".claude/workflows/{name}.js"));
    if is_file(&ov) {
        let text = read_text(env, &ov)?;
        if let Some(m) = override_re().captures(&text) {
            let tv = m.get(1).map(|g| g.as_str()).unwrap_or_default();
            let pv = version(env, plugin)?;
            if !pv.is_empty() && tv != pv {
                notes.push(format!("用的是專案的覆寫檔 .claude/workflows/{name}.js（依 {tv} 版的範本，plugin 現在是 {pv}）：新版的修正不會自動帶進來，請對照更新"));
            } else {
                notes.push(format!("用的是專案的覆寫檔 .claude/workflows/{name}.js"));
            }
            return Ok((text, "override", notes));
        }
        let Some(tpl) = &tpl else {
            notes.push(format!("找不到 plugin 的範本，改用專案裡的 .claude/workflows/{name}.js（舊副本）；請確認 plugin 已安裝，或用 --plugin-dir 指定"));
            return Ok((text, "legacy", notes));
        };
        if !is_file(tpl) {
            // plugin 在、但這版已經沒有這支（例如 0.5 併進 event.js 的五支）：舊副本不採用，免得跑到舊流程
            return awf_err(format!(
                "plugin 這一版沒有 {name}.js；專案裡的 .claude/workflows/{name}.js 是沒有 awf-override 標記的舊副本，不採用。{MERGED_HINT}要用自己的版本，請依現在的範本改寫並在結尾加上 awf-override 標記"
            ));
        }
        notes.push(format!(".claude/workflows/{name}.js 沒有 awf-override 標記（舊版 init 複製的副本），不採用，改用 plugin 的範本；確認沒在用就可以刪掉"));
    }
    match tpl.filter(|t| is_file(t)) {
        Some(t) => Ok((read_text(env, &t)?, "plugin", notes)),
        None => awf_err(format!(
            "找不到 workflow 範本 {name}.js：{}",
            if plugin.is_some() { format!("plugin 沒有這支。{MERGED_HINT}") } else { "找不到 agent-workflow plugin（先安裝 plugin，或用 --plugin-dir 指定）".to_string() }
        )),
    }
}

/// 專案的開發規範 profile；檔案在、卻不是 JSON 物件就拒絕。
fn methodology(env: &Env) -> A<Option<Value>> {
    let file = pj(&env.teams(), "methodology.json");
    let m = load_json(env, &file)?;
    if is_file(&file) && !m.as_ref().is_some_and(Value::is_object) {
        return awf_err(format!("{file} 讀不懂（不是 JSON 物件）"));
    }
    Ok(m)
}

/// profile 的 extensions（加進某一段的 agent）與 roles（換掉某一步的 agent）裡、沒有登記在 guard.config.json `extension_agents` 的 agent。
pub(crate) fn unregistered(env: &Env, methodology: Option<&Value>) -> R<Vec<String>> {
    let prof = methodology.and_then(|m| m.as_object());
    let no_ext: Vec<Value> = Vec::new();
    let ext = match prof.and_then(|m| m.get("extensions")) {
        Some(Value::Array(a)) => a,
        _ => &no_ext,
    };
    // roles 的值：不是字串的由 profile 的語意規則擋，這裡只看字串
    let swapped: Vec<&str> = prof.and_then(|m| m.get("roles")).and_then(Value::as_object)
        .map(|r| r.values().filter_map(Value::as_str).filter(|a| !a.is_empty()).collect()).unwrap_or_default();
    if ext.is_empty() && swapped.is_empty() {
        return Ok(Vec::new());
    }
    let g = load_json_or(env, &pj(&env.teams(), "guard.config.json"))?;
    let reg = py::get(&g, "extension_agents")?;
    let mut out: BTreeSet<String> = BTreeSet::new();
    for e in ext {
        let Some(agent) = e.as_object().and_then(|e| e.get("agent")).filter(|a| py::truthy(a)) else { continue };
        py::hashable(agent)?;
        if !reg.as_object().is_some_and(|r| agent.as_str().is_some_and(|a| r.contains_key(a))) {
            out.insert(py::s(agent));
        }
    }
    for a in swapped {
        if !reg.as_object().is_some_and(|r| r.contains_key(a)) {
            out.insert(format!("{a}（roles 換上去的）"));
        }
    }
    Ok(out.into_iter().collect())
}

/// 產生可以執行的 workflow 腳本：換佔位符、內嵌開發規範 profile；有 args 時一併內嵌（主 session 不用再傳）。
pub fn render(env: &Env, name: &str, plugin: Option<&str>, args: Option<&Value>) -> A<(String, Vec<String>)> {
    let (text, source, mut notes) = find_template(env, name, plugin)?;
    let mut cfg = read_config(env)?;
    // 主線分支的佔位符一律取守門設定（guard.config.json）的值：工具讀的是同一份；沒寫或不合規則就是預設
    for (k, v) in crate::layout::placeholders(&crate::layout::for_project(&env.project)) {
        cfg.insert(k.to_string(), json!(v));
    }
    let text = ph_re().replace_all(&text, |c: &regex::Captures| {
        let k = c.get(1).map(|g| g.as_str()).unwrap_or_default();
        cfg.get(k).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| c.get(0).map(|g| g.as_str()).unwrap_or_default().to_string())
    });
    let left: BTreeSet<&str> = ph_re().captures_iter(&text).filter_map(|c| c.get(1).map(|g| g.as_str())).collect();
    if !left.is_empty() {
        return awf_err(format!(
            "{name}.js 還有沒換掉的佔位符（.claude/teams/workflow.config 缺這些值；跑 awf init --upgrade 補上）：{}",
            left.into_iter().collect::<Vec<_>>().join("、")
        ));
    }
    let meth = methodology(env)?;
    let missing = unregistered(env, meth.as_ref())?;
    if !missing.is_empty() {
        return awf_err(format!("methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents：{}（用 /agent-workflow:add-agent 設定）", missing.join("、")));
    }
    let mut text = build::embed_methodology(&text, meth.as_ref());
    if let Some(a) = args {
        let line = format!("\n{ARGS_LINE}\n");
        if text.matches(&line).count() != 1 {
            return awf_err(format!("{name}.js 沒有唯一的一行「{ARGS_LINE}」，沒辦法內嵌參數（覆寫檔請保留這一行）"));
        }
        text = text.replacen(&line, &format!("\nconst A = Object.assign({}, args || {{}})\n", build::dumps_compact(a)), 1);
    }
    let pv = version(env, plugin)?;
    let tv = match read_text(env, &pj(&env.teams(), "VERSION")) {
        Ok(t) => pyish::strip(&t).to_string(),
        Err(e) if e.class == "UnicodeDecodeError" => return Err(Fail::Py(e)),
        Err(_) => String::new(),
    };
    if source == "plugin" && !pv.is_empty() && !tv.is_empty() && pv != tv {
        notes.push(format!("plugin 是 {pv}，專案的工具是 {tv}：跑 awf init --upgrade 讓兩邊一致（不一致時閘門工具可能不認得新版 workflow 的回傳）"));
    }
    Ok((text, notes))
}

/// 自訂腳本可以派的 agent_type：模版角色（不含 dev-merge-steward）加上 guard.config.json 登記過的擴充 agent。
/// `auto_merge.merger_role` 指定的合併角色就算登記成擴充 agent 也不算（合併是信任邊界，只有 sdlc-merge 能派）。
pub fn custom_roles(env: &Env) -> R<Vec<String>> {
    let ns = cfg_str(&read_config(env)?, "AGENT_NS", &format!("{PLUGIN_NAME}:"));
    let g = load_json_or(env, &pj(&env.teams(), "guard.config.json"))?;
    let mut out: Vec<String> = script::ROLES.iter().map(|r| format!("{ns}{r}")).collect();
    if let Value::Object(ext) = py::get(&g, "extension_agents")? {
        let mut keys: Vec<String> = ext.keys().cloned().collect();
        keys.sort();
        let merger = crate::config::custom_merger(&env.project);
        out.extend(keys.into_iter().filter(|k| merger.as_deref() != Some(k.as_str())));
    }
    Ok(out)
}

/// 片段的目錄：專案的 `.claude/teams/tools/snippets`（0.x 的專案才有）；沒有時用模版的 `workflow-templates/snippets`。
fn snippet_dir(env: &Env) -> String {
    let own = pj(&env.teams(), "tools/snippets");
    if is_dir(&own) {
        return own;
    }
    match &env.template {
        Some(t) => crate::template::snippets_str(t),
        None => own,
    }
}

/// 檢查並組裝自訂腳本：（可以執行的腳本, {source, sha1}）。檢查不過回 `Fail::Awf`，不產生任何東西。
pub fn render_custom(env: &Env, src: &str, stage: &str, args: &Value) -> A<(String, Value)> {
    if !script::STAGES.contains(&stage) {
        return awf_err(format!(
            "{stage} 段不能用自訂腳本（只開放 {}）：plan 的調研重用、sync 的衝突核對、test 的分片核對都寫在標準腳本裡，merge 是信任邊界",
            script::STAGES.join("、")
        ));
    }
    let text = match env.sr().read_text(src) {
        Ok(t) => t,
        Err(e) => return awf_err(format!("讀不到自訂腳本 {src}：{}", e.msg)),
    };
    let cfg = read_config(env)?;
    let ns = cfg_str(&cfg, "AGENT_NS", &format!("{PLUGIN_NAME}:"));
    let mut cache = build::SnippetCache::new();
    let sdir = snippet_dir(env);
    let mut snippets: Vec<(&str, String)> = Vec::new();
    for n in script::SNIPPETS {
        match build::load_snippet(n, std::path::Path::new(&sdir), &mut cache) {
            Ok(t) => snippets.push((n, t)),
            Err(build::BuildError::Snippet(m)) => return awf_err(m),
            Err(build::BuildError::Other(m)) => return Err(Fail::Py(PyErr::other("OSError", m))),
        }
    }
    let roles = custom_roles(env)?;
    let allowed: BTreeSet<String> = roles.iter().cloned().collect();
    let errs = script::check(&text, &allowed, &ns, &script::defined_names(snippets.iter().map(|(_, t)| t.as_str())));
    if !errs.is_empty() {
        let name = py::pname(src);
        return awf_err(format!("自訂腳本 {name} 沒通過檢查，不執行：\n  - {}", errs.iter().take(20).cloned().collect::<Vec<_>>().join("\n  - ")));
    }
    let meth = methodology(env)?;
    if !unregistered(env, meth.as_ref())?.is_empty() {
        return awf_err("methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents（用 /agent-workflow:add-agent 設定）");
    }
    let cap = match meth.as_ref().and_then(|m| m.get("limits")).and_then(Value::as_object).and_then(|l| l.get("max_agents")) {
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Number(n)) if !n.is_f64() && n.as_i64().is_some_and(|x| x > 0) || n_is_big(n) => n.to_string(),
        _ => "40".to_string(),
    };
    let js = build::dumps_compact;
    let s_js = js(&json!(stage));
    let meth_js = meth.as_ref().map(js).unwrap_or_else(|| "null".to_string());
    let mut pre: Vec<String> = vec![
        format!("// ── 以下由 awf 接上（自訂腳本，{stage} 段）：執行核心、pattern 庫、這一段的參數。不要手改 ──"),
        format!("const NS = {}", js(&json!(ns))),
        format!("const A = Object.assign({}, args || {{}})", js(args)),
        format!("const METHODOLOGY = {meth_js}"),
        "const MP = METHODOLOGY || {}".to_string(),
    ];
    for (n, t) in &snippets {
        pre.push(format!("// ── 片段 {n} ──\n{t}"));
    }
    pre.push("const R = resolveRecipe(METHODOLOGY, A.recipe, [].concat(A.modifiers || []), A.size)".to_string());
    pre.push(format!(
        "if (R.errors.length) return {{ stage: {s_js}, halted: true, halt_reason: 'recipe 解析有錯：' + R.errors.join('；'), missing: [], recipe: {{ name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages }}, trace: [] }}"
    ));
    pre.push("const RECIPE = { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages, summary: recipeSummary(R) }".to_string());
    pre.push(format!(
        "awfInit({{ run_dir: A.run_dir, stage: {s_js}, lang: {}, rules: '.claude/teams/rules.md', constraints: awfConstraints(MP, {s_js}), kg: A.kg === true, tune: awfTune(MP) }})",
        js(&json!(cfg_str(&cfg, "OUTPUT_LANGUAGE", "")))
    ));
    pre.push(format!(
        "awfKernelInit({{ profile: METHODOLOGY, stage: {s_js}, modifiers: [].concat(A.modifiers || []), max_agents: {cap}, roles: {} }})",
        js(&json!(roles))
    ));
    let out = script::assemble(&text, &pre.join("\n"), stage).map_err(|e| Fail::Py(PyErr::other("ScriptError", e.0)))?;
    let out = build::embed_schemas(&out, std::path::Path::new(&env.teams())).map_err(|e| Fail::Py(PyErr::other("OSError", e.to_string())))?;
    let sha: String = Sha1::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok((out, json!({"source": py::pname(src).replace('\\', "/"), "sha1": sha})))
}

/// 超出 i64 的正整數（Python 的 int 沒有上限）。
fn n_is_big(n: &serde_json::Number) -> bool {
    !n.is_f64() && n.as_i64().is_none() && n.as_u64().is_some()
}

/// plugin 的範本名加上專案裡有 awf-override 標記的覆寫檔名（排序）。
pub fn workflow_names(env: &Env, plugin: Option<&str>) -> R<Vec<String>> {
    let stems = |dir: &str| -> Vec<(String, String)> {
        std::fs::read_dir(dir).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".js"))
            .map(|n| (if n == ".js" { n.clone() } else { n.strip_suffix(".js").unwrap_or(&n).to_string() }, pj(dir, &n)))
            .collect()
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    if let Some(p) = plugin {
        names.extend(stems(&pj(p, TEMPLATES)).into_iter().map(|(s, _)| s));
    }
    let ovd = pj(&env.project, ".claude/workflows");
    if is_dir(&ovd) {
        for (stem, path) in stems(&ovd) {
            if override_re().is_match(&env.sr().read_text_lossy(&path)?) {
                names.insert(stem);
            }
        }
    }
    Ok(names.into_iter().collect())
}
