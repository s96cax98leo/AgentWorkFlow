//! 安裝檢查。原本是 0.x 的 Python 工具 `awf_doctor.py`。
//!
//! 只讀、不改任何檔案、不連網。結束碼：有「問題」是 1，其餘 0（「注意」不算失敗）。
//! Claude Code 的 installed_plugins.json、known_marketplaces.json 格式沒有承諾穩定：讀不懂就略過那一項並說明。
//!
//! 和 0.x 的 Python 版不同的地方（對跑測試把這兩項濾掉再比）：
//! - 沒有 `python` 這一項：執行檔不需要 Python。
//! - 沒有 `hooks-compile` 這一項：那是「hook 的 .py 編譯得過」，hook 改註冊成執行檔之後換成執行檔的檢查。
//!
//! 訊息裡的指令已改成 `awf <子指令>`（`awf init --upgrade`、`awf next`），基準檔的對應字串一起換過。

use crate::kb::pyish;
use crate::save_run::py;
use crate::schema::Schemas;
use crate::state::{self, Env, Fail, MERGED_HINT, TEMPLATES};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

pub const PLUGIN_NAME: &str = "agent-workflow";
pub const OK: &str = "ok";
pub const WARN: &str = "warn";
pub const ERR: &str = "error";
pub const INFO: &str = "info";
/// 0.5 併進 event.js 的五支。
const MERGED: [&str; 5] = ["event-meeting", "event-decision", "event-demo-check", "event-reqs-change", "event-upstream"];

/// 一項檢查的結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub level: &'static str,
    pub msg: String,
    pub fix: String,
}

fn item(id: &str, level: &'static str, msg: impl Into<String>, fix: impl Into<String>) -> Item {
    Item { id: id.to_string(), level, msg: msg.into(), fix: fix.into() }
}

fn mark(level: &str) -> &'static str {
    match level {
        OK => "通過",
        WARN => "注意",
        ERR => "問題",
        _ => "說明",
    }
}

fn update_fix(key: &str) -> String {
    format!("claude plugin update {key}，再在 Claude Code 裡 /reload-plugins（只跑 marketplace update 不會更新 plugin）")
}

/// 單一檢查自己出錯：（類別名, 訊息）。主流程接住後列成一筆「注意」，不影響其他檢查。
type Part = Result<Vec<Item>, (String, String)>;

fn py_err(e: crate::kb::PyErr) -> (String, String) {
    (e.class, e.msg)
}

/// 讀 JSON（開頭的 BOM 略過）；讀不到或不是 JSON 回 None。
fn load_json(p: &str) -> Option<Value> {
    let bytes = std::fs::read(p).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
}

fn is_file(p: &str) -> bool {
    !p.contains('\0') && Path::new(p).is_file()
}

fn is_dir(p: &str) -> bool {
    !p.contains('\0') && Path::new(p).is_dir()
}

/// 版本比較用：只看開頭的數字段（0.4.1、0.3.0-x.1 → [0,4,1]、[0,3,0]）。
pub fn vtuple(v: &str) -> Vec<u64> {
    let t = v.trim_start().strip_prefix('v').unwrap_or(v.trim_start());
    let mut out = Vec::new();
    for (i, part) in t.split('.').enumerate() {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        match digits.parse() {
            Ok(n) => out.push(n),
            Err(_) => return Vec::new(),
        }
        // 這一段後面還有別的字（0.3.0-x.1 的 0-x）就到此為止
        if digits.len() != part.len() {
            break;
        }
        let _ = i;
    }
    out
}

fn s_of(v: Option<&Value>) -> String {
    match v {
        Some(x) if py::truthy(x) => py::s(x),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ 環境
pub fn check_env(which: &dyn Fn(&str) -> bool) -> Vec<Item> {
    vec![
        if which("git") { item("git", OK, "找得到 git", "") } else { item("git", ERR, "找不到 git", "安裝 git 並確認在 PATH 裡") },
        if which("gh") {
            item("gh", OK, "找得到 gh", "")
        } else {
            item("gh", WARN, "找不到 gh（GitHub CLI）：推送後開 PR、合併、讀 CI 狀態都需要它", "安裝 GitHub CLI 並執行 gh auth login；只做到 deliver 以前可以先不裝")
        },
        if which("node") {
            item("node", OK, "找得到 node", "")
        } else {
            item("node", WARN, "找不到 node：只有乾跑驗證 workflow 腳本（check_workflows.mjs）需要它，平常執行不需要", "要改 workflow 或覆寫檔時再安裝 Node.js")
        },
    ]
}

// ------------------------------------------------------------------ 安裝來源
#[derive(Debug, Clone)]
struct Install {
    key: String,
    market: String,
    scope: String,
    version: String,
    path: String,
    project: String,
}

/// （agent-workflow 的所有安裝紀錄, 讀不懂時的說明）。
fn installs(home: &str) -> (Vec<Install>, String) {
    let f = pyish::pjoin(home, "plugins/installed_plugins.json");
    if !is_file(&f) {
        return (Vec::new(), String::new());
    }
    let d = load_json(&f);
    let Some(Value::Object(plugins)) = d.as_ref().and_then(Value::as_object).and_then(|d| d.get("plugins")) else {
        return (Vec::new(), "installed_plugins.json 讀不懂（Claude Code 的格式可能變了），略過安裝來源的檢查".to_string());
    };
    let mut out = Vec::new();
    for (key, entries) in plugins {
        if key.split('@').next() != Some(PLUGIN_NAME) {
            continue;
        }
        let list: Vec<&Value> = match entries {
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        };
        for e in list {
            let Some(e) = e.as_object() else { continue };
            out.push(Install {
                key: key.clone(),
                market: key.split_once('@').map(|(_, m)| m.to_string()).unwrap_or_default(),
                scope: s_of(e.get("scope")),
                version: s_of(e.get("version")),
                path: s_of(e.get("installPath")),
                project: s_of(e.get("projectPath")),
            });
        }
    }
    (out, String::new())
}

/// marketplace 目前提供的版本（本機的 marketplace 目錄；讀不到回空）。
pub fn market_version(home: &str, market: &str) -> String {
    let d = load_json(&pyish::pjoin(home, "plugins/known_marketplaces.json"));
    let loc = d.as_ref().and_then(Value::as_object).and_then(|d| d.get(market)).and_then(Value::as_object).and_then(|m| m.get("installLocation")).filter(|v| py::truthy(v));
    let mut bases: Vec<String> = Vec::new();
    if let Some(l) = loc {
        bases.push(py::s(l));
    }
    bases.push(pyish::pjoin(home, &format!("plugins/marketplaces/{market}")));
    for base in bases {
        let m = load_json(&pyish::pjoin(&base, ".claude-plugin/marketplace.json"));
        let Some(Value::Array(plugins)) = m.as_ref().and_then(Value::as_object).and_then(|m| m.get("plugins")) else { continue };
        for p in plugins {
            let Some(p) = p.as_object() else { continue };
            if p.get("name").and_then(Value::as_str) != Some(PLUGIN_NAME) {
                continue;
            }
            if let Some(v) = p.get("version").filter(|v| py::truthy(v)) {
                return py::s(v);
            }
            if let Some(src) = p.get("source").and_then(Value::as_str) {
                let pj = load_json(&pyish::pjoin(&pyish::pjoin(&base, src), ".claude-plugin/plugin.json"));
                if let Some(v) = pj.as_ref().and_then(Value::as_object).and_then(|o| o.get("version")).filter(|v| py::truthy(v)) {
                    return py::s(v);
                }
            }
        }
    }
    String::new()
}

fn same_place(a: &str, b: &str) -> bool {
    let r = |p: &str| pyish::resolve(p).unwrap_or_else(|_| p.to_string());
    r(a) == r(b)
}

pub fn check_installs(env: &Env) -> Vec<Item> {
    let (recs, note) = installs(&env.home);
    if !note.is_empty() {
        return vec![item("install", INFO, note, "")];
    }
    // 別的專案的安裝不影響這裡
    let mine: Vec<&Install> = recs.iter().filter(|r| r.project.is_empty() || same_place(&env.abs(&r.project), &env.project)).collect();
    if mine.is_empty() {
        return vec![item(
            "install",
            INFO,
            "Claude Code 沒有登記這個專案可用的 agent-workflow 安裝（用 --plugin-dir、環境變數或在模版 repo 裡執行時不需要）",
            "要安裝：claude plugin marketplace add <模版 repo>，再 claude plugin install agent-workflow@agentworkflow",
        )];
    }
    let mut out = Vec::new();
    let keys: BTreeSet<&str> = mine.iter().map(|r| r.key.as_str()).collect();
    if keys.len() > 1 {
        out.push(item(
            "install-sources",
            WARN,
            format!("同一個 plugin 從不只一個來源安裝：{}。Claude Code 會同時載入，agent、hook、skill 都會重複，版本不同時行為不可預期", keys.iter().copied().collect::<Vec<_>>().join("、")),
            "留一個就好：claude plugin uninstall <不要的那個>，再 /reload-plugins",
        ));
    }
    for k in &keys {
        let vs: BTreeSet<&str> = mine.iter().filter(|r| r.key == *k && !r.version.is_empty()).map(|r| r.version.as_str()).collect();
        if vs.len() > 1 {
            out.push(item("install-versions", WARN, format!("{k} 在不同範圍裝了不同版本：{}", vs.iter().copied().collect::<Vec<_>>().join("、")), format!("{}；或移除較舊的那個範圍的安裝", update_fix(k))));
        }
    }
    for r in &mine {
        if !r.path.is_empty() && !is_dir(&env.abs(&r.path)) {
            let scope = if r.scope.is_empty() { "?" } else { &r.scope };
            out.push(item("install-path", ERR, format!("{}（{scope}）登記的安裝目錄不存在", r.key), format!("重新安裝：claude plugin uninstall {}，再 claude plugin install {}", r.key, r.key)));
        }
        let mv = market_version(&env.home, &r.market);
        if !mv.is_empty() && !r.version.is_empty() && vtuple(&mv) > vtuple(&r.version) {
            out.push(item("install-stale", WARN, format!("{} 安裝的是 {}，marketplace 已經是 {mv}", r.key, r.version), update_fix(&r.key)));
        }
    }
    if out.is_empty() {
        out.push(item("install", OK, format!("安裝來源沒有衝突：{}", mine.iter().map(|r| format!("{} {}", r.key, r.version)).collect::<Vec<_>>().join("、")), ""));
    }
    out
}

// ------------------------------------------------------------------ plugin 與版本
fn check_plugin(env: &Env, plugin: Option<&str>, notes: &[String]) -> Part {
    let Some(plugin) = plugin else {
        return Ok(vec![item(
            "plugin",
            ERR,
            "找不到 agent-workflow plugin（awf next 沒辦法產生 workflow 腳本）",
            "安裝 plugin（claude plugin install agent-workflow@agentworkflow），或用 --plugin-dir／環境變數 AWF_PLUGIN_DIR 指定 plugin 目錄",
        )]);
    };
    let pj = load_json(&pyish::pjoin(plugin, ".claude-plugin/plugin.json"));
    let pv = s_of(pj.as_ref().and_then(Value::as_object).and_then(|o| o.get("version")));
    let mut out = vec![item("plugin", if pv.is_empty() { WARN } else { OK }, format!("plugin 版本 {}", if pv.is_empty() { "讀不到" } else { &pv }), "")];
    out.extend(notes.iter().map(|n| item("plugin-note", INFO, n.clone(), "")));
    let vf = pyish::pjoin(&env.teams(), "VERSION");
    let tv = if is_file(&vf) {
        let bytes = std::fs::read(&vf).map_err(|e| ("OSError".to_string(), e.to_string()))?;
        String::from_utf8(bytes).map_err(|e| ("UnicodeDecodeError".to_string(), e.to_string()))?.trim_matches(pyish::is_space).to_string()
    } else {
        String::new()
    };
    if tv.is_empty() {
        out.push(item("version", WARN, "專案沒有 .claude/teams/VERSION（0.2.0 以前套用的，或檔案被刪了）", "跑 awf init --target <這個專案> --upgrade"));
    } else if !pv.is_empty() && vtuple(&tv) < vtuple(&pv) {
        out.push(item("version", WARN, format!("專案的工具是 {tv}，plugin 是 {pv}：新版 workflow 的回傳，舊的閘門工具可能不認得"), "跑 awf init --target <這個專案> --upgrade（已套用的專案不會自動更新）"));
    } else if !pv.is_empty() && vtuple(&tv) > vtuple(&pv) {
        out.push(item("version", WARN, format!("專案的工具是 {tv}，plugin 還是 {pv}"), update_fix("agent-workflow@agentworkflow")));
    } else {
        out.push(item("version", OK, format!("專案的工具與 plugin 版本一致（{tv}）"), ""));
    }
    Ok(out)
}

// ------------------------------------------------------------------ hook
/// 專案 settings 裡註冊的是不是這套 hook：舊的 `awf_hook.py`，或執行檔的 `awf hook <種類>`（路徑可以加引號）。
fn project_hook_re() -> &'static regex::Regex {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r#"awf_hook\.py|awf(?:\.exe)?[\\"']*\s+hook\s+(?:guard-shell|guard-paths|envelope|session-start)\b"#).expect("固定的正規式"))
}

fn check_hooks(env: &Env, plugin: Option<&str>, has_install: bool) -> Part {
    let mut out = Vec::new();
    let any_dir = plugin.is_some_and(|p| is_dir(&pyish::pjoin(p, "hooks"))) || is_dir(&pyish::pjoin(&env.project, ".claude/hooks"));
    let mut registered = false;
    for name in ["settings.json", "settings.local.json"] {
        let s = load_json(&pyish::pjoin(&env.project, &format!(".claude/{name}")));
        if let Some(h) = s.as_ref().and_then(Value::as_object).and_then(|o| o.get("hooks")).filter(|h| py::truthy(h)) {
            registered = registered || serde_json::to_string(h).is_ok_and(|t| project_hook_re().is_match(&t));
        }
    }
    if registered && has_install {
        out.push(item(
            "hooks-double",
            WARN,
            "hook 註冊了兩次：專案的 settings 有 awf hook，plugin 也會註冊，每次工具呼叫會檢查兩遍",
            "擇一：留專案的（較嚴，適合 Windows）就停用 plugin 的 hook；留 plugin 的就把專案 settings 裡 awf hook 的 hooks 拿掉",
        ));
    } else if !registered && !has_install && !any_dir {
        out.push(item("hooks", WARN, "找不到任何 hook 註冊：守門與交件檢查沒有在跑", "安裝 plugin，或跑 awf init --target <這個專案> --upgrade --hooks project"));
    }
    Ok(out)
}

// ------------------------------------------------------------------ 設定
/// 驗 methodology 用的 schema 目錄：專案自己的，沒有就用模版的。
fn schema_teams(env: &Env) -> Option<String> {
    let has = |t: &str| is_file(&pyish::pjoin(t, "schemas/methodology.schema.json"));
    let own = env.teams();
    if has(&own) {
        return Some(own);
    }
    let tpl = crate::template::teams_str(env.template.as_deref()?);
    has(&tpl).then_some(tpl)
}

fn check_config(env: &Env) -> Part {
    let mut out = Vec::new();
    let teams = env.teams();
    let mf = pyish::pjoin(&teams, "methodology.json");
    let meth = load_json(&mf);
    if !is_file(&mf) {
        out.push(item("methodology", ERR, "沒有 .claude/teams/methodology.json（開發規範 profile）", "跑 awf init --target <這個專案> --upgrade，或用 /agent-workflow:init-workflow 重新設定"));
    } else if !meth.as_ref().is_some_and(Value::is_object) {
        out.push(item("methodology", ERR, "methodology.json 不是合法的 JSON 物件", "修正 JSON 語法（常見：多了結尾逗號、少了引號）"));
    } else if let Some(m) = &meth {
        let checked = match schema_teams(env) {
            Some(t) => Schemas::new(t).check_doc(m, "methodology"),
            None => Err("找不到 schemas/methodology.schema.json".to_string()),
        };
        match checked {
            Err(e) => out.push(item("methodology", WARN, format!("methodology.json 沒辦法驗證：驗證器跑不起來（{e}）"), "跑 awf init --target <這個專案> --upgrade 補齊工具與 schema")),
            Ok(errs) if !errs.is_empty() => out.push(item(
                "methodology",
                ERR,
                format!("methodology.json 有 {} 個問題：{}{}", errs.len(), errs.iter().take(5).cloned().collect::<Vec<_>>().join("；"), if errs.len() > 5 { "…" } else { "" }),
                "照訊息修正；欄位說明見模版的 docs/自訂與擴充.md",
            )),
            Ok(_) => out.push(item("methodology", OK, "methodology.json 通過 schema 與語意規則", "")),
        }
    }
    let gf = pyish::pjoin(&teams, "guard.config.json");
    let guard = load_json(&gf);
    if !is_file(&gf) {
        out.push(item("guard", WARN, "沒有 .claude/teams/guard.config.json：hook 只套用安全預設（只管 tpm-／dev- 角色，不擋主 session）", "跑 awf init --target <這個專案> --upgrade"));
    } else if let Some(Value::Object(g)) = &guard {
        let text = String::from_utf8_lossy(&std::fs::read(&gf).map_err(|e| ("OSError".to_string(), e.to_string()))?).into_owned();
        if text.contains(&"{".repeat(2)) {
            out.push(item("guard", ERR, "guard.config.json 還有沒換掉的佔位符（兩個左大括號開頭的那種）", "補上 workflow.config 的值後跑 awf init --target <這個專案> --upgrade"));
        } else {
            let am = g.get("auto_merge").and_then(Value::as_object).and_then(|a| a.get("enabled")).is_some_and(py::truthy);
            out.push(item("guard", OK, format!("guard.config.json 讀得懂{}", if am { "；auto_merge 已啟用" } else { "" }), ""));
        }
    } else {
        out.push(item("guard", ERR, "guard.config.json 不是合法的 JSON 物件：hook 讀不到設定", "修正 JSON 語法"));
    }
    if !is_file(&pyish::pjoin(&teams, "workflow.config")) {
        out.push(item("config", WARN, "沒有 .claude/teams/workflow.config：workflow 範本的佔位符沒有值可以換，awf next 會拒絕", "跑 awf init --target <這個專案> --upgrade"));
    }
    if meth.as_ref().is_some_and(Value::is_object) {
        let miss = state::unregistered(env, meth.as_ref()).map_err(py_err)?;
        if !miss.is_empty() {
            out.push(item("extensions", ERR, format!("methodology.json 的 extensions 有 agent 沒登記守門角色：{}", miss.join("、")), "用 /agent-workflow:add-agent 設定，或在 guard.config.json 的 extension_agents 補上"));
        }
    }
    Ok(out)
}

fn check_overrides(env: &Env, plugin: Option<&str>) -> Part {
    let d = pyish::pjoin(&env.project, ".claude/workflows");
    if !is_dir(&d) {
        return Ok(Vec::new());
    }
    let pv = state::plugin_version(env, plugin).map_err(py_err)?;
    let mut names: Vec<String> = std::fs::read_dir(&d)
        .map_err(|e| ("OSError".to_string(), e.to_string()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".js") && !n.starts_with('.'))
        .collect();
    names.sort();
    let (mut out, mut old) = (Vec::new(), Vec::new());
    for name in names {
        let text = String::from_utf8_lossy(&std::fs::read(pyish::pjoin(&d, &name)).map_err(|e| ("OSError".to_string(), e.to_string()))?).into_owned();
        let ver = state::override_re().captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        let stem = name.strip_suffix(".js").unwrap_or(&name);
        let gone = plugin.is_some_and(|p| !is_file(&pyish::pjoin(&pyish::pjoin(p, TEMPLATES), &name))) && MERGED.contains(&stem);
        match ver {
            Some(_) if gone => out.push(item(
                "override-merged",
                WARN,
                format!("覆寫檔 {name} 用不到了：{MERGED_HINT}"),
                "要保留自己的做法：把內容改寫成 methodology.json 的 events 裡的一筆（見模版的 docs/事件處理.md），或依現在的 event.js 另寫覆寫檔",
            )),
            None => old.push(name),
            Some(v) if !pv.is_empty() && v != pv => out.push(item(
                "override",
                WARN,
                format!("覆寫檔 {name} 是依 {v} 版的範本，plugin 現在是 {pv}：新版的修正不會自動帶進來"),
                "對照 plugin 的 workflow-templates/ 更新覆寫檔，再把結尾的 awf-override 版本改成新的",
            )),
            Some(_) => out.push(item("override", INFO, format!("採用專案的覆寫檔 {name}"), "")),
        }
    }
    if !old.is_empty() {
        out.push(item(
            "override-old",
            INFO,
            format!(".claude/workflows/ 有沒有標記的同名檔（舊版複製的副本，不會被採用）：{}{}", old.iter().take(8).cloned().collect::<Vec<_>>().join("、"), if old.len() > 8 { "…" } else { "" }),
            "確認沒在用：跑 awf init --target <這個專案> --upgrade 會移除沒改過的、保留改過的並說明",
        ));
    }
    Ok(out)
}

// ------------------------------------------------------------------ 主流程
pub fn run_checks(env: &Env, plugin_dir: &str, which: &dyn Fn(&str) -> bool) -> Vec<Item> {
    let mut out = check_env(which);
    if !is_dir(&env.teams()) {
        out.push(item(
            "project",
            ERR,
            "這個專案還沒套用 agent-workflow（沒有 .claude/teams/）",
            "在 Claude Code 裡用 /agent-workflow:init-workflow（會先問開發習慣）；想先用預設值：awf init --target . --defaults",
        ));
        out.extend(check_installs(env));
        return out;
    }
    let (plugin, notes) = match state::find_plugin(env, plugin_dir) {
        Ok(v) => v,
        Err(Fail::Awf(msg)) => {
            out.push(item("plugin", ERR, msg, "確認 --plugin-dir 指到 plugin 目錄（裡面有 workflow-templates/）"));
            return out;
        }
        Err(Fail::Py(e)) => {
            out.push(item("doctor", WARN, format!("有一項檢查自己出錯了（{}：{}），略過", e.class, e.msg), ""));
            (None, Vec::new())
        }
    };
    let plugin = plugin.as_deref();
    let inst = check_installs(env);
    let has_install = inst.iter().any(|i| i.id != "install" || i.level == OK);
    let parts: [&dyn Fn() -> Part; 5] = [&|| check_plugin(env, plugin, &notes), &|| Ok(inst.clone()), &|| check_hooks(env, plugin, has_install), &|| check_config(env), &|| check_overrides(env, plugin)];
    for part in parts {
        // 單一檢查出錯不影響其他檢查
        match part() {
            Ok(items) => out.extend(items),
            Err((class, msg)) => out.push(item("doctor", WARN, format!("有一項檢查自己出錯了（{class}：{msg}），略過"), "")),
        }
    }
    out
}

pub fn text_of(items: &[Item]) -> String {
    let mut lines = Vec::new();
    for i in items {
        lines.push(format!("[{}] {}", mark(i.level), i.msg));
        if !i.fix.is_empty() && i.level != OK {
            lines.push(format!("       修法：{}", i.fix));
        }
    }
    let n_err = items.iter().filter(|i| i.level == ERR).count();
    let n_warn = items.iter().filter(|i| i.level == WARN).count();
    lines.push(if n_err > 0 || n_warn > 0 { format!("doctor：{n_err} 個問題、{n_warn} 個注意") } else { "doctor：都正常".to_string() });
    lines.join("\n")
}

pub fn json_of(items: &[Item]) -> String {
    let checks: Vec<Value> = items.iter().map(|i| json!({"id": i.id, "level": i.level, "msg": i.msg, "fix": i.fix})).collect();
    serde_json::to_string_pretty(&json!({"ok": !items.iter().any(|i| i.level == ERR), "checks": checks})).unwrap_or_default()
}

fn opt<'a>(argv: &'a [String], name: &str) -> &'a str {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).map(String::as_str).unwrap_or("")
}

/// 回（exit code，stdout）。
pub fn run_captured(argv: &[String], env: &Env, which: &dyn Fn(&str) -> bool) -> (i32, String) {
    let items = run_checks(env, opt(argv, "--plugin-dir"), which);
    let out = if argv.iter().any(|a| a == "--json") { json_of(&items) } else { text_of(&items) };
    (i32::from(items.iter().any(|i| i.level == ERR)), format!("{out}\n"))
}

/// PATH 裡找不找得到這個指令（Windows 另試 PATHEXT 的副檔名）。
pub fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()).split(';').map(str::to_string).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).any(|d| exts.iter().any(|e| d.join(format!("{name}{e}")).is_file()))
}

/// `awf doctor …`
pub fn main(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--selftest") {
        return selftest();
    }
    let (code, out) = run_captured(args, &Env::from_env(), &on_path);
    print!("{out}");
    code
}

// ------------------------------------------------------------------ selftest
pub fn selftest() -> i32 {
    let root = std::env::temp_dir().join(format!("awf-doctor-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let code = selftest_in(&root);
    let _ = std::fs::remove_dir_all(&root);
    code
}

fn selftest_in(root: &Path) -> i32 {
    let mut ok = true;
    let mut check = |name: &str, cond: bool, detail: String| {
        ok = ok && cond;
        let tail: String = detail.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
        println!("  [{}] {name}{}", if cond { "通過" } else { "失敗" }, if cond { String::new() } else { format!("：{tail}") });
    };
    let ids = |items: &[Item], level: Option<&str>| -> Vec<String> { items.iter().filter(|i| level.is_none_or(|l| i.level == l)).map(|i| i.id.clone()).collect() };
    let w = |p: &str, text: &str| {
        let f = root.join(p);
        let _ = std::fs::create_dir_all(f.parent().unwrap_or(root));
        let _ = std::fs::write(&f, text);
    };
    let rm = |p: &str| {
        let _ = std::fs::remove_file(root.join(p));
    };
    let all = |_: &str| true;
    let none = |_: &str| false;
    let _ = std::fs::create_dir_all(root);
    let rs = root.canonicalize().ok().and_then(|p| p.to_str().map(|s| s.replace('\\', "/"))).unwrap_or_default();
    let p = |rel: &str| format!("{rs}/{rel}");
    println!("awf doctor selftest（暫存目錄，不碰 repo 與使用者的 ~/.claude）");

    // ── 環境
    let e = check_env(&all);
    check("環境：git、gh、node 都在 → 三項通過", ids(&e, Some(OK)) == ["git", "gh", "node"], format!("{e:?}"));
    let e = check_env(&|n| n == "git");
    check("環境：缺 gh、node 只是注意，各附修法", ids(&e, Some(WARN)) == ["gh", "node"] && e.iter().all(|i| i.level == OK || !i.fix.is_empty()), format!("{e:?}"));
    check("環境：缺 git 是問題", ids(&check_env(&none), Some(ERR)).contains(&"git".to_string()), String::new());

    // ── 假的 plugin 與專案
    let from = Env::from_env();
    let good = from.template.as_deref().map(crate::template::teams_str).filter(|t| is_dir(t)).or_else(|| Some(from.teams()).filter(|t| is_dir(t)));
    let Some(src_teams) = good else {
        println!("awf doctor selftest：找不到內建 profile 與 schema（要在模版 repo 或已 init 的專案裡跑）");
        return 1;
    };
    let good_text = std::fs::read_to_string(pyish::pjoin(&src_teams, "methodologies/default.json")).or_else(|_| std::fs::read_to_string(pyish::pjoin(&src_teams, "methodology.json"))).unwrap_or_default();
    let good: Value = serde_json::from_str(&good_text).unwrap_or(Value::Null);
    w("plugin/.claude-plugin/plugin.json", &json!({"name": PLUGIN_NAME, "version": "0.5.0"}).to_string());
    w("plugin/workflow-templates/sdlc-plan.js", "// x\n");
    w("plugin/hooks/hooks.json", "{}");
    w("proj/.claude/teams/methodology.json", &good.to_string());
    w("proj/.claude/teams/guard.config.json", &json!({"runs_dir": "_tpm/runs"}).to_string());
    w("proj/.claude/teams/workflow.config", "PROJECT_NAME=\"示範客戶 ACME\"\n");
    w("proj/.claude/teams/VERSION", "0.5.0\n");
    let _ = std::fs::create_dir_all(root.join("home/plugins"));
    // schema 用來源的那一份（專案自己沒有時 doctor 會退回模版的）
    let env = Env { project: p("proj"), cwd: rs.clone(), home: p("home"), plugin_env: [None, None], template: from.template.clone().or_else(|| Some(p("no-template"))), transcripts: None, now: None, today: None };
    let env = if schema_teams(&env).is_some() {
        env
    } else {
        // 已 init 的專案裡跑：把 schema 複製進暫存專案
        if let Ok(rd) = std::fs::read_dir(pyish::pjoin(&src_teams, "schemas")) {
            for f in rd.flatten() {
                if let (Some(n), Ok(t)) = (f.file_name().to_str(), std::fs::read_to_string(f.path())) {
                    w(&format!("proj/.claude/teams/schemas/{n}"), &t);
                }
            }
        }
        env
    };
    let plugin = p("plugin");
    let doctor = || run_checks(&env, &plugin, &all);
    let find = |items: &[Item], id: &str| items.iter().find(|i| i.id == id).cloned().unwrap_or_else(|| item("", INFO, "", ""));

    let r = doctor();
    check("乾淨的專案：沒有問題也沒有注意", ids(&r, Some(ERR)).is_empty() && ids(&r, Some(WARN)).is_empty(), format!("{r:?}"));
    let oks = ids(&r, Some(OK));
    check("乾淨的專案：版本一致、profile 通過、設定讀得懂都列出來", ["version", "methodology", "guard"].iter().all(|k| oks.iter().any(|o| o == k)), format!("{oks:?}"));
    check("輸出：文字版每個不通過的項目下一行是修法；結尾有總結", text_of(&r).contains("doctor：都正常") && text_of(&check_env(&none)).contains("修法："), text_of(&r));

    // ── 還沒套用
    let empty = Env { project: p("empty"), ..env.clone() };
    let r = run_checks(&empty, "", &all);
    let pr = find(&r, "project");
    check("還沒套用的專案：說明並給兩種套用方式，不往下檢查設定", ids(&r, Some(ERR)) == ["project"] && pr.fix.contains("init-workflow") && pr.fix.contains("--defaults") && !ids(&r, None).contains(&"methodology".to_string()), format!("{r:?}"));

    // ── 版本
    w("proj/.claude/teams/VERSION", "0.4.1\n");
    let r = doctor();
    check("版本：專案的工具比 plugin 舊 → 注意，修法是 --upgrade", ids(&r, Some(WARN)).contains(&"version".to_string()) && find(&r, "version").fix.contains("--upgrade"), format!("{r:?}"));
    w("proj/.claude/teams/VERSION", "0.6.0\n");
    let r = doctor();
    let v = find(&r, "version");
    check("版本：plugin 比專案舊 → 注意，修法是 plugin update 加 /reload-plugins", v.level == WARN && v.fix.contains("plugin update") && v.fix.contains("/reload-plugins"), format!("{r:?}"));
    rm("proj/.claude/teams/VERSION");
    check("版本：沒有 VERSION 檔 → 注意", find(&doctor(), "version").level == WARN, String::new());
    w("proj/.claude/teams/VERSION", "0.5.0\n");
    check("版本比較：0.10.0 比 0.9.9 新；帶後綴的版本只看數字段", vtuple("0.10.0") > vtuple("0.9.9") && vtuple("0.3.0-x.1") == [0, 3, 0] && vtuple("").is_empty() && vtuple("v1.2") == [1, 2], String::new());

    // ── 找不到 plugin
    let lost = Env { template: Some(p("no-template")), ..env.clone() };
    let r = run_checks(&lost, "", &all);
    let pl = find(&r, "plugin");
    check("找不到 plugin → 問題，修法提到安裝與 --plugin-dir", pl.level == ERR && pl.fix.contains("--plugin-dir"), format!("{r:?}"));
    let r = run_checks(&env, &p("nope"), &all);
    check("--plugin-dir 指錯 → 問題並說明", find(&r, "plugin").level == ERR, format!("{r:?}"));

    // ── 安裝來源
    w("cache/a/workflow-templates/x.js", "");
    w("cache/b/workflow-templates/x.js", "");
    let (inst_dir, other_dir) = (p("cache/a"), p("cache/b"));
    let installed = |plugins: Value| w("home/plugins/installed_plugins.json", &json!({"version": 2, "plugins": plugins}).to_string());
    installed(json!({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": inst_dir, "version": "0.5.0"}]}));
    let r = check_installs(&env);
    check("安裝來源：只有一個來源、一個版本 → 通過", ids(&r, Some(OK)) == ["install"], format!("{r:?}"));
    installed(json!({
        "agent-workflow@agentworkflow": [{"scope": "user", "installPath": inst_dir, "version": "0.5.0"}],
        "agent-workflow@my-fork": [{"scope": "user", "installPath": other_dir, "version": "0.3.0"}],
        "other-plugin@x": [{"scope": "user", "installPath": other_dir, "version": "9"}]}));
    let r = check_installs(&env);
    check(
        "安裝來源：同一個 plugin 從兩個 marketplace 安裝 → 注意，列出兩個來源，修法是移除其中一個",
        ids(&r, Some(WARN)) == ["install-sources"] && r[0].msg.contains("agent-workflow@my-fork") && r[0].msg.contains("agent-workflow@agentworkflow") && r[0].fix.contains("uninstall"),
        format!("{r:?}"),
    );
    installed(json!({
        "agent-workflow@agentworkflow": [{"scope": "user", "installPath": inst_dir, "version": "0.5.0"}],
        "agent-workflow@my-fork": [{"scope": "project", "projectPath": p("別的專案"), "installPath": other_dir, "version": "0.3.0"}]}));
    check("安裝來源：另一個來源只裝在別的專案 → 不算衝突", ids(&check_installs(&env), Some(OK)) == ["install"], format!("{:?}", check_installs(&env)));
    installed(json!({"agent-workflow@agentworkflow": [
        {"scope": "user", "installPath": inst_dir, "version": "0.5.0"},
        {"scope": "project", "projectPath": p("proj"), "installPath": other_dir, "version": "0.4.1"}]}));
    check("安裝來源：同一來源在兩個範圍裝了不同版本 → 注意", ids(&check_installs(&env), Some(WARN)) == ["install-versions"], format!("{:?}", check_installs(&env)));
    installed(json!({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": p("不見了"), "version": "0.5.0"}]}));
    check("安裝來源：登記的安裝目錄不存在 → 問題", ids(&check_installs(&env), Some(ERR)) == ["install-path"], format!("{:?}", check_installs(&env)));
    w("market/.claude-plugin/marketplace.json", &json!({"name": "agentworkflow", "plugins": [{"name": PLUGIN_NAME, "source": "./plugin", "version": "0.5.0"}]}).to_string());
    w("home/plugins/known_marketplaces.json", &json!({"agentworkflow": {"installLocation": p("market")}}).to_string());
    installed(json!({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": inst_dir, "version": "0.4.1"}]}));
    let r = check_installs(&env);
    check(
        "安裝來源：marketplace 已經是新版、安裝的還是舊版 → 注意，修法是 plugin update（不是 marketplace update）",
        ids(&r, Some(WARN)) == ["install-stale"] && r[0].msg.contains("0.4.1") && r[0].msg.contains("0.5.0") && r[0].fix.contains("claude plugin update agent-workflow@agentworkflow"),
        format!("{r:?}"),
    );
    w("market/.claude-plugin/marketplace.json", &json!({"plugins": [{"name": PLUGIN_NAME, "source": "./plugin"}]}).to_string());
    w("market/plugin/.claude-plugin/plugin.json", &json!({"version": "0.6.0"}).to_string());
    check("安裝來源：marketplace.json 沒寫版本時讀 plugin.json", market_version(&env.home, "agentworkflow") == "0.6.0", market_version(&env.home, "agentworkflow"));
    w("home/plugins/installed_plugins.json", "{壞掉");
    let r = check_installs(&env);
    check("安裝來源：登記檔讀不懂 → 只說明、略過，不算問題", ids(&r, None) == ["install"] && r[0].level == INFO && r[0].msg.contains("讀不懂"), format!("{r:?}"));
    rm("home/plugins/installed_plugins.json");
    check("安裝來源：沒有登記檔 → 說明（用 --plugin-dir 時不需要）", check_installs(&env)[0].level == INFO, String::new());

    // ── hook
    w("proj/.claude/settings.json", &json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "python3 .claude/hooks/awf_hook.py guard-shell"}]}]}}).to_string());
    installed(json!({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": inst_dir, "version": "0.5.0"}]}));
    w("home/plugins/known_marketplaces.json", "{}");
    check("hook：專案 settings 與 plugin 都註冊 → 注意（重複註冊），修法是擇一", ids(&doctor(), Some(WARN)).contains(&"hooks-double".to_string()), format!("{:?}", doctor()));
    rm("home/plugins/installed_plugins.json");
    check("hook：只有專案註冊（沒有 plugin 安裝紀錄）→ 不算重複", !ids(&doctor(), None).contains(&"hooks-double".to_string()), String::new());
    rm("proj/.claude/settings.json");

    // ── 設定
    w("proj/.claude/teams/methodology.json", "{壞掉");
    check("設定：methodology.json 不是 JSON → 問題", find(&doctor(), "methodology").level == ERR, String::new());
    let (code, _) = run_captured(&["--plugin-dir".to_string(), plugin.clone()], &env, &all);
    check("設定：有問題時 doctor 的結束碼是 1", code == 1, code.to_string());
    let with = |k: &str, v: Value| {
        let mut m = good.clone();
        if let Some(o) = m.as_object_mut() {
            o.insert(k.to_string(), v);
        }
        m.to_string()
    };
    w("proj/.claude/teams/methodology.json", &with("constraints", json!([{"id": "a", "file": "/etc/passwd", "stages": ["design"]}])));
    let m = find(&doctor(), "methodology");
    check("設定：methodology.json 違反語意規則（限制文件用絕對路徑）→ 問題並帶出訊息", m.level == ERR && m.msg.contains("constraints"), format!("{m:?}"));
    w("proj/.claude/teams/methodology.json", &with("extensions", json!([{"name": "x", "agent": "my:agent", "stage": "plan", "instructions": "看一下"}])));
    let x = find(&doctor(), "extensions");
    check("設定：擴充 agent 沒登記守門角色 → 問題，修法提到 add-agent", x.level == ERR && x.fix.contains("add-agent"), format!("{:?}", doctor()));
    w("proj/.claude/teams/methodology.json", &good.to_string());
    w("proj/.claude/teams/guard.config.json", &format!("{{\"runs_dir\": \"{}RUNS_DIR}}}}\"}}", "{".repeat(2)));
    check("設定：guard.config.json 還有佔位符 → 問題", find(&doctor(), "guard").level == ERR, format!("{:?}", find(&doctor(), "guard")));
    w("proj/.claude/teams/guard.config.json", "[]");
    check("設定：guard.config.json 不是物件 → 問題", find(&doctor(), "guard").level == ERR, String::new());
    rm("proj/.claude/teams/guard.config.json");
    rm("proj/.claude/teams/workflow.config");
    let r = doctor();
    check("設定：缺 guard.config.json、workflow.config → 注意（各一筆）", find(&r, "guard").level == WARN && find(&r, "config").level == WARN, format!("{r:?}"));
    w("proj/.claude/teams/guard.config.json", &json!({"runs_dir": "_tpm/runs"}).to_string());
    w("proj/.claude/teams/workflow.config", "PROJECT_NAME=\"示範客戶 ACME\"\n");

    // ── 覆寫檔
    w("proj/.claude/workflows/sdlc-plan.js", "// 我的版本\n// awf-override: 0.4.1\n");
    w("proj/.claude/workflows/sdlc-test.js", "// 現行\n// awf-override: 0.5.0\n");
    w("proj/.claude/workflows/sdlc-build.js", "// 舊副本，沒有標記\n");
    w("proj/.claude/workflows/event-meeting.js", "// 我的會議流程\n// awf-override: 0.4.1\n");
    let r = doctor();
    let om = find(&r, "override-merged");
    check("覆寫檔：0.5 併進 event.js 的 workflow 的覆寫檔 → 注意（用不到了），修法提到 events", om.level == WARN && om.fix.contains("events"), format!("{r:?}"));
    rm("proj/.claude/workflows/event-meeting.js");
    let r = doctor();
    let ov: Vec<&Item> = r.iter().filter(|i| i.id.starts_with("override")).collect();
    check(
        "覆寫檔：依舊版範本的覆寫檔 → 注意；版本相同的只說明；沒標記的舊副本列出來並說明不會被採用",
        ov.iter().map(|i| i.id.as_str()).collect::<Vec<_>>() == ["override", "override", "override-old"] && ov[0].level == WARN && ov[1].level == INFO && ov[2].msg.contains("sdlc-build.js"),
        format!("{ov:?}"),
    );

    // ── 不改檔、可以輸出 JSON
    fn snapshot(d: &Path, out: &mut Vec<(String, u64)>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    snapshot(&path, out);
                } else if let Ok(m) = e.metadata() {
                    out.push((path.to_string_lossy().into_owned(), m.len()));
                }
            }
        }
    }
    let (mut before, mut after) = (Vec::new(), Vec::new());
    snapshot(root, &mut before);
    doctor();
    snapshot(root, &mut after);
    before.sort();
    after.sort();
    check("只讀：跑完沒有新增或改動任何檔案", before == after, String::new());
    let (code, out) = run_captured(&["--json".to_string(), "--plugin-dir".to_string(), plugin.clone()], &env, &all);
    let d: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let shape = d["checks"].as_array().is_some_and(|a| a.iter().all(|i| i.as_object().is_some_and(|o| o.keys().map(String::as_str).collect::<Vec<_>>() == ["id", "level", "msg", "fix"])));
    check("--json：輸出 {ok, checks[]}，每筆有 id、level、msg、fix；沒有問題時 exit 0", code == 0 && d["ok"] == json!(true) && shape, out.chars().take(300).collect());
    println!("awf doctor selftest：{}", if ok { "全部通過" } else { "有失敗" });
    i32::from(!ok)
}
