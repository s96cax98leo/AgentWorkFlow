//! `--set`、每一段 workflow 的 args、資料包、交付文件同步、找 workflow 的回傳。

use super::{awf_err, event_required, is_dir, load_json, read_config, required_of, s_or, write, Env, A};
use crate::kb::pyish;
use crate::kb::PyErr;
use crate::save_run::{self as sr, py::{self, pj, R}};
use crate::{deliverables, pack, pyfmt};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::Mutex;

/// 切換工作目錄的地方一次只能有一個（資料包的 run 目錄是相對於專案根目錄的路徑）。
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn key_ok(k: &str) -> bool {
    let k = k.strip_suffix('\n').unwrap_or(k);
    let mut it = k.bytes();
    it.next().is_some_and(|b| b.is_ascii_lowercase() || b == b'_') && it.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `_coerce(v)`：true／false／整數，其餘照字串。超出 64 位元範圍的整數回 None（不接受）。
fn coerce(v: &str) -> Option<Value> {
    if v == "true" {
        return Some(json!(true));
    }
    if v == "false" {
        return Some(json!(false));
    }
    let t = v.strip_suffix('\n').unwrap_or(v);
    let digits = t.strip_prefix('-').unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse::<i64>().ok().map(|n| json!(n));
    }
    Some(json!(v))
}

/// `--set k=v`、`--set-json k=<JSON>` 記進 run.json 的 args（之後每一段都帶）。回傳錯誤。
pub fn apply_sets(data: &mut Value, argv: &[String]) -> R<Vec<String>> {
    if !data.is_object() {
        return Err(py::attr_err(data, "setdefault"));
    }
    let d = py::obj_mut(data)?;
    let store = d.entry("args".to_string()).or_insert_with(|| json!({}));
    let mut errs = Vec::new();
    for raw in sr::opts(argv, "--set") {
        match raw.split_once('=') {
            Some((k, v)) if key_ok(k) => {
                if k == "run_dir" {
                    errs.push("run_dir 由 --run 決定，不能用 --set 改".to_string());
                } else if let Some(val) = coerce(v) {
                    py::obj_mut(store)?.insert(k.to_string(), val);
                } else {
                    errs.push(format!("--set 要寫成 k=v：{raw}"));
                }
            }
            _ => errs.push(format!("--set 要寫成 k=v：{raw}")),
        }
    }
    for raw in sr::opts(argv, "--set-json") {
        let parsed = match raw.split_once('=') {
            Some((k, v)) if key_ok(k) && k != "run_dir" => match py::loads(v) {
                Ok(val) => Some((k.to_string(), val)),
                Err(e) if py::os_or_value(&e) => None,
                Err(e) => return Err(e),
            },
            _ => None,
        };
        match parsed {
            Some((k, val)) => {
                py::obj_mut(store)?.insert(k, val);
            }
            None => errs.push(format!("--set-json 要寫成 k=<JSON>：{}", pyish::take(&raw, 80))),
        }
    }
    Ok(errs)
}

/// 一段存下來的結果（`<run>/<段>.json`）；讀不到、不是物件都當成空的。
pub(crate) fn stage_result(env: &Env, run: &str, stage: &str) -> R<Value> {
    Ok(load_json(env, &pj(run, &format!("{stage}.json")))?.filter(Value::is_object).unwrap_or_else(|| json!({})))
}

fn nonempty_list(v: &Value) -> bool {
    v.as_array().is_some_and(|a| !a.is_empty())
}

/// 已存檔的拆工：design 的 payload.subtasks，否則 plan 的 subtasks。
fn subtasks(env: &Env, run: &str) -> R<Value> {
    let d = stage_result(env, run, "design")?;
    if let Some(Value::Object(design)) = d.get("design") {
        if let Some(Value::Object(p)) = design.get("payload") {
            if let Some(st) = p.get("subtasks").filter(|s| nonempty_list(s)) {
                return Ok(st.clone());
            }
        }
    }
    let p = stage_result(env, run, "plan")?;
    Ok(p.get("subtasks").filter(|s| s.is_array()).cloned().unwrap_or_else(|| json!([])))
}

/// `v in (None, "")`
fn blank(v: &Value) -> bool {
    v.is_null() || v.as_str() == Some("")
}

/// sdlc-merge 要跟著 guard.config.json 走的兩個值：`auto_merge.merger_role`、`auto_merge.freeze_file`。
/// 範本讀不到設定檔，所以由這裡帶進 args。只在設定檔有明寫、而且正規化後和範本的預設
/// （`<AGENT_NS>dev-merge-steward`、`<RUNS_DIR>/auto-merge.freeze.json`）不同時才帶：沒寫時 args 和以前逐字相同。
/// 設定檔讀不懂時不帶（hook 那邊照它自己的失敗語意擋），正規化用 hook 的同一個函式（`config::auto_merge`）。
fn merge_guard_args(env: &Env) -> R<Vec<(String, String)>> {
    let raw = super::load_json_or(env, &pj(&env.teams(), "guard.config.json"))?;
    let am = raw.get("auto_merge").filter(|v| v.is_object());
    let set = |k: &str| am.and_then(|a| a.get(k)).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    if !set("merger_role") && !set("freeze_file") {
        return Ok(Vec::new());
    }
    let Ok(cfg) = crate::config::load(&env.project) else { return Ok(Vec::new()) };
    let norm = crate::config::auto_merge(&cfg);
    let wc = read_config(env)?;
    let wc_str = |k: &str, d: &str| wc.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
    let single_line = |s: &str| !s.is_empty() && !s.contains(['\n', '\r', '`']);
    let mut out = Vec::new();
    let default_merger = format!("{}dev-merge-steward", wc_str("AGENT_NS", "agent-workflow:"));
    if set("merger_role") && single_line(&norm.merger_role) && norm.merger_role != default_merger {
        out.push(("merger_role".to_string(), norm.merger_role.clone()));
    }
    let default_freeze = format!("{}/auto-merge.freeze.json", wc_str("RUNS_DIR", "_tpm/runs").trim_end_matches('/'));
    if set("freeze_file") && single_line(&norm.freeze_file) && norm.freeze_file != default_freeze {
        out.push(("freeze_file".to_string(), norm.freeze_file.clone()));
    }
    Ok(out)
}

/// 這一段 workflow 的 args：（args, 缺的必要參數, 說明）。來源：run.json（需求、recipe、--set 記的值）與前面各段的結果。
pub fn stage_args(env: &Env, run: &str, data: &Value, stage: &str, workflow: &str) -> R<(Map<String, Value>, Vec<String>, Vec<String>)> {
    let store = py::get(data, "args")?;
    let mut notes: Vec<String> = Vec::new();
    let mut a: Map<String, Value> = Map::new();
    if py::truthy(store) {
        match store {
            Value::Object(o) => {
                for (k, v) in o {
                    if !blank(v) {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
            other => return Err(py::type_err(format!("'{}' object is not iterable", py::tname(other)))),
        }
    }
    // run 目錄給絕對路徑：agent 常常 cd 進目標 repo 的 worktree 工作，相對路徑會讓紀錄與 log 寫到別的目錄
    a.insert("run_dir".into(), json!(pyish::resolve(run)?));
    let target = py::get(data, "target_repo")?;
    if py::truthy(target) && !a.contains_key("repo") {
        a.insert("repo".into(), target.clone());
    }
    let req = py::get(data, "requirement")?;
    if py::truthy(req) && (workflow == "sdlc-plan" || workflow == "sdlc-design") && !a.contains_key("requirement") {
        a.insert("requirement".into(), req.clone());
    }
    if sr::has_recipe(data)? {
        // 鎖在 run.json 的 recipe 優先（--set 改不了）
        let rec = py::idx(data, "recipe")?;
        a.insert("recipe".into(), py::get(rec, "name")?.clone());
        a.insert("modifiers".into(), Value::Array(py::iter_or(py::get(rec, "modifiers")?)?));
        let size = py::get(rec, "size")?;
        if py::truthy(size) {
            a.insert("size".into(), size.clone());
        } else {
            a.shift_remove("size");
        }
    } else if let Some(Value::String(m)) = a.get("modifiers") {
        let list: Vec<Value> = m.split(',').filter(|x| !x.is_empty()).map(|x| json!(x)).collect();
        a.insert("modifiers".into(), Value::Array(list));
    }
    if workflow.starts_with("sdlc-") && workflow != "sdlc-merge" {
        let sync = stage_result(env, run, "sync")?;
        let default = format!("{}{}", read_config(env)?.get("BRANCH_PREFIX").and_then(Value::as_str).unwrap_or_default(), a.get("epic").map(s_or).unwrap_or_default());
        if let Some(head) = sync.get("head_branch").and_then(Value::as_str).filter(|h| !h.is_empty() && *h != default) {
            if ["build", "test", "deliver", "sync"].contains(&stage) {
                if let Some(b) = a.get("branch").filter(|b| !blank(b) && b.as_str() != Some(head)) {
                    notes.push(format!("忽略 --set 的 branch={}：sync 走過中轉分支，以它回報的 {head} 為準", py::s(b)));
                }
                a.insert("branch".into(), json!(head));
                notes.push(format!("sync 走過中轉分支：這一段帶 branch={head}"));
            }
        }
    }
    if workflow == "sdlc-merge" {
        a.shift_remove("branch"); // merge 用 PR 號，不帶 branch
        // 合併角色與凍結檔跟 guard.config.json 的 auto_merge（hook 與 awf merge-gate 讀的同一份）：--set 給的不採用
        a.shift_remove("merger_role");
        a.shift_remove("freeze_file");
        for (k, v) in merge_guard_args(env)? {
            notes.push(format!("guard.config.json 的 auto_merge 有設 {k}：這一段帶 {k}={v}"));
            a.insert(k, json!(v));
        }
    }
    if workflow == "sdlc-build" {
        let prev = stage_result(env, run, "build")?;
        let remaining = prev.get("remaining_subtasks").filter(|r| nonempty_list(r));
        match remaining.filter(|_| prev.get("halted").is_some_and(py::truthy)) {
            Some(rem) => {
                a.insert("subtasks".into(), rem.clone());
                a.insert("integrated".into(), prev.get("integrated").filter(|i| py::truthy(i)).cloned().unwrap_or_else(|| json!([])));
                notes.push(format!("續跑 build：只做還沒合進整合分支的 {} 個子任務", rem.as_array().map_or(0, Vec::len)));
            }
            None => {
                let derived = subtasks(env, run)?;
                if nonempty_list(&derived) {
                    // 有已存檔的拆工就以它為準（--set 改不了：build 做的要是核准過的那一份）
                    if a.get("subtasks").is_some_and(|s| !pyfmt::eq(s, &derived)) {
                        notes.push("忽略 --set 的 subtasks：以 design（沒有 design 段時是 plan）存下來的子任務為準".to_string());
                    }
                    a.insert("subtasks".into(), derived);
                }
            }
        }
        if !a.get("subtasks").is_some_and(py::truthy) {
            a.shift_remove("subtasks");
        }
    }
    if workflow == "sdlc-test" || workflow == "sdlc-design-revise" {
        let design = stage_result(env, run, "design")?;
        if let Some(sp) = design.get("sub_plans").filter(|s| nonempty_list(s)) {
            if a.get("sub_plans").is_some_and(|s| !pyfmt::eq(s, sp)) {
                notes.push("忽略 --set 的 sub_plans：以 design 存下來的子計劃為準".to_string());
            }
            a.insert("sub_plans".into(), sp.clone());
        }
    }
    let mut required = required_of(workflow);
    if workflow == "event" {
        a.insert("event".into(), json!(stage)); // 事件類型就是段名（執行紀錄寫在 <run_dir>/<類型>/）
        for k in event_required(env, stage)? {
            if !required.contains(&k) {
                required.push(k);
            }
        }
    }
    let empty = |v: Option<&Value>| v.is_none_or(|v| blank(v) || v.as_array().is_some_and(Vec::is_empty));
    let mut need: Vec<String> = required.into_iter().filter(|k| empty(a.get(k))).collect();
    if workflow == "sdlc-test" {
        let mods = a.get("modifiers").cloned().unwrap_or(Value::Null);
        let mut set: Vec<Value> = Vec::new();
        for m in py::iter_or(&mods)? {
            py::hashable(&m)?;
            set.push(m);
        }
        for (k, m) in [("has_ui", "ui"), ("has_security", "security")] {
            if !a.contains_key(k) && set.iter().any(|x| x.as_str() == Some(m)) {
                a.insert(k.to_string(), json!(true));
            }
            if !a.contains_key(k) {
                need.push(k.to_string());
            }
        }
    }
    Ok((a, need, notes))
}

/// 備資料包（輔助：失敗只少一份資料包）。回傳（要併進 args 的值, 說明）。
pub fn run_pack(env: &Env, stage: &str, run_arg: &str, a: &Map<String, Value>, run: &str) -> R<(Map<String, Value>, Vec<String>)> {
    let mut argv: Vec<String> = vec![stage.to_string(), "--run".into(), run_arg.to_string()];
    if stage == "build" && a.get("subtasks").is_some_and(py::truthy) {
        let f = pj(run, ".wf/build.subtasks.json");
        write(&f, &py::dumps(a.get("subtasks").unwrap_or(&Value::Null), None))?;
        argv.push("--subtasks".into());
        argv.push(f);
        if let Some(e) = a.get("epic").filter(|e| py::truthy(e)) {
            argv.push("--epic".into());
            argv.push(py::s(e));
        }
    }
    if let Some(t) = &env.today {
        argv.push("--today".into());
        argv.push(t.clone());
    }
    let failed = |class: &str, msg: &str| (Map::new(), vec![format!("資料包沒有產生（{class}：{msg}）；這一段照常跑")]);
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::current_dir().ok();
    // run_dir 是相對於專案根目錄的路徑
    if let Err(e) = std::env::set_current_dir(&env.project) {
        let pe = PyErr::os(&e, &env.project);
        return Ok(failed(&pe.class, &pe.msg));
    }
    let mut out = String::new();
    let prev = crate::kb::swap_err_sink(Some(String::new()));
    let res = pack::run_pack(&argv, &env.project, &mut out);
    let errs = crate::kb::swap_err_sink(prev).unwrap_or_default();
    let extra = load_json(env, &pj(&pj(run, stage), pack::ARGS));
    if let Some(d) = old {
        let _ = std::env::set_current_dir(d);
    }
    if let Err(e) = res {
        return Ok(failed(&e.class, &e.msg));
    }
    let mut extra = match extra? {
        Some(Value::Object(o)) => o,
        _ => Map::new(),
    };
    // 資料包路徑也給絕對路徑（和 run_dir 同理）；pack-args.json 本身維持相對路徑，內容可重現
    let absp = |v: &Value| -> Value {
        match v {
            Value::String(p) if !p.is_empty() && !pyish::is_abs(p) => json!(pyish::resolve(&pyish::pjoin(&env.project, p)).unwrap_or_else(|_| pyish::pjoin(&env.project, p))),
            other => other.clone(),
        }
    };
    if let Some(p) = extra.get("pack").cloned() {
        extra.insert("pack".into(), absp(&p));
    }
    if let Some(Value::Object(tp)) = extra.get("task_packs").cloned() {
        extra.insert("task_packs".into(), Value::Object(tp.iter().map(|(k, v)| (k.clone(), absp(v))).collect()));
    }
    let text = format!("{errs}{out}");
    let notes: Vec<String> = crate::merge_gate::pyutil::splitlines(&text).into_iter().map(pyish::strip).filter(|l| !l.is_empty()).take(6).map(str::to_string).collect();
    Ok((extra, notes))
}

/// deliver 段存檔後：把交付摘要併進功能開發紀錄、寫需求追溯矩陣、記錄交付文件各小節的來源。
/// `updated`＝workflow 回傳的受影響清單（[{id, sections}]）：只記錄這些小節的來源，清單外的改動列出來交人。
/// 最後跑一次 check，把問題列出來。輔助功能：失敗只警告，不影響閘門。
pub fn deliverables_sync(env: &Env, run: &str, data: &Value, updated: Option<&Value>) -> String {
    let inner = || -> R<String> {
        let repo_raw = s_or(py::get(data, "target_repo")?);
        let repo = if repo_raw.is_empty() { None } else { Some(pyish::pjoin(&env.project, &repo_raw)) };
        let mut only: Option<BTreeSet<String>> = None;
        if let Some(Value::Array(list)) = updated {
            let mut set = BTreeSet::new();
            for u in list.iter().filter(|u| u.is_object()) {
                for t in py::iter_or(py::get(u, "sections")?)? {
                    if let Some(t) = t.as_str() {
                        set.insert(format!("{}#{t}", py::s(py::get(u, "id")?)));
                    }
                }
            }
            only = Some(set);
        }
        let (code, notes) = deliverables::sync(&env.project, run, repo.as_deref(), &[], only.as_ref());
        let head = if code == 0 { "交付文件已同步" } else { "交付文件沒有同步（要人處理）" };
        let mut lines: Vec<String> = vec![format!("{head}：")];
        lines.extend(notes.iter().take(20).map(|n| format!("  {n}")));
        let (errs, _) = deliverables::check_all(&env.project, repo.as_deref());
        if let Some(errs) = errs.filter(|e| !e.is_empty()) {
            lines.push(format!("交付文件檢查有 {} 個問題（結案或交付前要處理）：", errs.len()));
            lines.extend(errs.iter().take(12).map(|e| format!("  [問題] {e}")));
        }
        Ok(lines.join("\n"))
    };
    match inner() {
        Ok(t) => t,
        Err(e) => format!("交付文件沒有同步（{}：{}）；可以之後手動跑 awf deliverables sync --run <run_dir>", e.class, e.msg),
    }
}

// ------------------------------------------------------------------ 找 workflow 的回傳
/// workflow 紀錄的根目錄：`AWF_TRANSCRIPTS_DIR`，否則 Claude Code 的專案目錄。
pub fn transcripts_root(env: &Env) -> R<String> {
    match &env.transcripts {
        Some(t) => Ok(env.abs(t)),
        None => crate::cost::project_dir(&env.project).map_err(|c| PyErr::other("RuntimeError", c.0)),
    }
}

fn mtime(p: &std::path::Path) -> Option<f64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(match m.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    })
}

fn names_in(dir: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
    v.sort();
    v
}

/// 最近一次用這支腳本跑的 workflow 紀錄（`<session>/workflows/wf_<id>.json`；格式沒承諾穩定）：（內容, 檔案路徑）。
/// 只採用可信的：修改時間不在未來；這次 next 之後用同一支腳本跑的只有一筆。有兩筆以上時分不出哪一筆是真的，回 `Fail::Awf` 交人。
pub fn find_wf_meta(env: &Env, script: &str, since: f64) -> A<Option<(Value, String)>> {
    let want = if script.is_empty() { None } else { Some(pyish::resolve(&env.abs(script))?) };
    let root = transcripts_root(env)?;
    let now = env.now_ts();
    let mut hits: Vec<(f64, Value, String)> = Vec::new();
    if is_dir(&root) {
        for sess in names_in(&root) {
            let wf_dir = pj(&pj(&root, &sess), "workflows");
            if !is_dir(&wf_dir) {
                continue;
            }
            for name in names_in(&wf_dir).into_iter().filter(|n| n.starts_with("wf_") && n.ends_with(".json")) {
                let f = pj(&wf_dir, &name);
                let Some(mt) = mtime(std::path::Path::new(&f)) else { continue };
                // 修改時間在未來的不採用（正常寫出來的檔不會這樣）
                if mt + 1.0 < since || mt > now + 120.0 {
                    continue;
                }
                let Some(meta) = load_json(env, &f)? else { continue };
                let (Some(sp), Some(want)) = (meta.as_object().and_then(|m| m.get("scriptPath")).and_then(Value::as_str), want.as_ref()) else { continue };
                let same = pyish::resolve(&pyish::pjoin(&env.project, sp)).is_ok_and(|p| &p == want);
                if same {
                    hits.push((mt, meta, f));
                }
            }
        }
    }
    if hits.is_empty() {
        return Ok(None);
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if since != 0.0 && hits.len() > 1 {
        let stems: Vec<String> = hits.iter().skip(hits.len().saturating_sub(4)).map(|h| { let n = py::pname(&h.2); n.strip_suffix(".json").unwrap_or(&n).to_string() }).collect();
        return awf_err(format!(
            "這次 next 之後，用同一支腳本跑的 workflow 紀錄有 {} 筆（{}），分不出哪一筆才是這一段的結果。確認之後把正確的回傳存成 JSON 檔，用 --output <檔> 指定（這樣存的段不能政策核准，要由人核准）",
            hits.len(),
            stems.join("、")
        ));
    }
    Ok(hits.pop().map(|(_, m, f)| (m, f)))
}

/// 這次 next 之後有沒有「讀不懂、但內容提到這支腳本」的 workflow 紀錄檔：
/// [`find_wf_meta`] 只採用讀得懂的紀錄，整份讀不懂（NaN、超大整數、沒寫完）的會被當成沒有。
/// 這裡把它找出來，讓 `awf done` 分得出「找不到」與「找到但讀不懂」。任何錯誤都回 false。
pub fn unreadable_wf_meta(env: &Env, script: &str, since: f64) -> bool {
    if script.is_empty() {
        return false;
    }
    let abs = env.abs(script);
    let mut needles: Vec<String> = vec![abs.clone()];
    needles.extend(pyish::resolve(&abs).ok());
    // 紀錄檔裡的路徑是 JSON 字串：原樣與跳脫過的都找
    let escaped: Vec<String> = needles.iter().filter_map(|n| serde_json::to_string(n).ok()).map(|q| q.trim_matches('"').to_string()).collect();
    needles.extend(escaped);
    let Ok(root) = transcripts_root(env) else { return false };
    let now = env.now_ts();
    for sess in names_in(&root) {
        let wf_dir = pj(&pj(&root, &sess), "workflows");
        for name in names_in(&wf_dir).into_iter().filter(|n| n.starts_with("wf_") && n.ends_with(".json")) {
            let f = pj(&wf_dir, &name);
            let Some(mt) = mtime(std::path::Path::new(&f)) else { continue };
            if mt + 1.0 < since || mt > now + 120.0 || !matches!(load_json(env, &f), Ok(None)) {
                continue;
            }
            let Ok(raw) = std::fs::read(&f) else { continue };
            let text = String::from_utf8_lossy(&raw);
            if needles.iter().any(|n| text.contains(n.as_str())) {
                return true;
            }
        }
    }
    false
}

/// 目前這個 session 的 id（猜的：專案 transcript 目錄裡最近寫過的那個）。猜不到回空字串。
pub fn current_session(env: &Env) -> R<String> {
    let root = transcripts_root(env)?;
    let mut best: Option<(f64, String)> = None;
    for name in names_in(&root).into_iter().filter(|n| n.ends_with(".jsonl") && n != ".jsonl" || n == ".jsonl") {
        let Some(mt) = mtime(std::path::Path::new(&pj(&root, &name))) else { return Ok(String::new()) };
        let stem = if name == ".jsonl" { name.clone() } else { name.strip_suffix(".jsonl").unwrap_or(&name).to_string() };
        let cand = (mt, stem);
        if best.as_ref().is_none_or(|b| (cand.0, &cand.1) > (b.0, &b.1)) {
            best = Some(cand);
        }
    }
    Ok(best.map(|b| b.1).unwrap_or_default())
}
