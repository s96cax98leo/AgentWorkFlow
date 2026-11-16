//! 自動接點：使用者與 agent 都不用記得下指令，由 `awf next`／`awf done`／`awf save-run close` 自己呼叫。
//!
//! - 每段開跑前更新知識圖（[`kg_before_stage`]）。
//! - merge 之後把規格差異併回現行規格（[`spec_apply`]）。
//! - 結案或取消後重寫 vault（[`refresh_vault`]）。
//! - 段被中斷時回收已完成的調研（[`recover_interrupted`]）。
//!
//! 共同規則：
//! - 全部是輔助。任何失敗只回一行警告，流程照舊走；閘門不讀這裡寫的任何東西
//!   （`reuse.json` 例外：它本來就由 `awf save-run stage plan` 的 `verify_reuse` 重新核對，只會更嚴）。
//! - 功能沒設定時什麼都不做，`awf next`／`awf done` 的輸出和沒有這個模組時逐字相同：
//!   沒有圖就不碰知識圖，需求來源不是 openspec 就沒有併回規格的動作，沒有等待中的 plan 段就不回收。
//! - 開關在 profile 的 `knowledge.auto`（[`AutoCfg`]），全部選填。

use super::args::stage_result;
use super::{find_wf_meta, is_dir, is_file, load_json, or_obj, s_or, setdefault_obj, transcripts_root, Env, Fail};
use crate::kb::pyish;
use crate::save_run::{self as sr, py::{self, pj, R}};
use crate::{kg, pack, recipe, schema, spec};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// 待更新清單的檔名（在 `knowledge.dir` 底下）：PostToolUse hook 每改一個檔附加一行，圖更新成功後清掉。
pub const DIRTY_NAME: &str = "dirty.txt";
/// 動作裡的試跑摘要最多幾個字。
pub const PREVIEW_CHARS: usize = 1500;
/// `awf next --skip <名>` 認得的名字。
pub const SKIP_SPEC_APPLY: &str = "spec-apply";

/// profile 的 `knowledge.auto`。沒寫的鍵用預設值；型別不對的鍵當成沒寫。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCfg {
    /// `update`：每段開跑前要不要更新圖。None（沒寫）＝圖已經存在才更新；true＝沒有圖也建；false＝不更新。
    pub update: Option<bool>,
    /// `timeout`：更新圖的時間上限（秒），預設 60。
    pub timeout: u64,
    /// `session`：開新 session 時注入幾行摘要，預設開。
    pub session: bool,
    /// `prompt`：提問時依訊息裡的 id、檔名、符號名查圖並注入位置，預設關。
    pub prompt: bool,
    /// `spec_apply`：merge 之後提醒把規格差異併回現行規格（需求來源是 openspec 才有），預設開。
    pub spec_apply: bool,
    /// `export`：結案或取消後重寫 vault（圖存在才做），預設開。
    pub export: bool,
}

impl Default for AutoCfg {
    fn default() -> Self {
        AutoCfg { update: None, timeout: kg::DEFAULT_TIMEOUT, session: true, prompt: false, spec_apply: true, export: true }
    }
}

/// 讀 profile 的 `knowledge.auto`。
pub fn auto_cfg(profile: &Value) -> AutoCfg {
    let mut c = AutoCfg::default();
    let Some(a) = profile.get("knowledge").and_then(|k| k.get("auto")).and_then(Value::as_object) else { return c };
    let flag = |k: &str| a.get(k).and_then(Value::as_bool);
    c.update = flag("update");
    if let Some(t) = a.get("timeout").and_then(Value::as_u64).filter(|t| (1..=3600).contains(t)) {
        c.timeout = t;
    }
    c.session = flag("session").unwrap_or(c.session);
    c.prompt = flag("prompt").unwrap_or(c.prompt);
    c.spec_apply = flag("spec_apply").unwrap_or(c.spec_apply);
    c.export = flag("export").unwrap_or(c.export);
    c
}

/// 專案的 profile（`<專案>/.claude/teams/methodology.json`）；讀不到、讀不懂都回 Null。hook 也用這個，所以不經過任何會印訊息的路徑。
pub fn read_profile(project: &str) -> Value {
    let file = pyish::pjoin(project, ".claude/teams/methodology.json");
    std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object).unwrap_or(Value::Null)
}

/// 圖所在的目錄（絕對路徑）：profile 的 `knowledge.dir`，沒有就是 `_tpm/kb`。只做字串運算，不碰檔案系統。
pub fn knowledge_dir(project: &str, profile: &Value) -> String {
    let d = profile.get("knowledge").and_then(|k| k.get("dir")).and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()).unwrap_or("_tpm/kb").replace('\\', "/");
    if pyish::is_abs(&d) { pyish::ppath(&d) } else { pyish::pjoin(project, &d) }
}

/// 圖檔的路徑。
pub fn graph_db(project: &str, profile: &Value) -> String {
    pyish::pjoin(&knowledge_dir(project, profile), kg::store::DB_NAME)
}

fn clip(text: &str, n: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= n { t.to_string() } else { format!("{}…", t.chars().take(n).collect::<String>()) }
}

// ------------------------------------------------------------------ D4：每段開跑前更新圖
/// 更新知識圖並回要加進動作的說明（成功時沒有說明）。
/// 只有圖已經存在、或 `knowledge.auto.update` 明確開啟時才做；逾時或失敗回一行警告，這一段照常跑。
pub fn kg_before_stage(env: &Env) -> Vec<String> {
    let profile = read_profile(&env.project);
    let cfg = auto_cfg(&profile);
    if cfg.update == Some(false) {
        return Vec::new();
    }
    let dir = knowledge_dir(&env.project, &profile);
    if cfg.update != Some(true) && !is_file(&pyish::pjoin(&dir, kg::store::DB_NAME)) {
        return Vec::new();
    }
    let up = kg::update(Some(&env.project), Some(cfg.timeout));
    match up.status {
        kg::UpdateStatus::Updated | kg::UpdateStatus::Unchanged => {
            // 待更新清單只是「有東西變了」的訊號：圖跟上之後清掉
            let _ = std::fs::remove_file(pyish::pjoin(&dir, DIRTY_NAME));
            Vec::new()
        }
        kg::UpdateStatus::TimedOut | kg::UpdateStatus::Failed => vec![format!("警告：{}；這一段照常跑，資料包用上一版的圖", clip(&up.note, 300))],
    }
}

// ------------------------------------------------------------------ close／abandon 之後重寫 vault
/// 結案或取消之後重寫 vault。沒有圖、或 `knowledge.auto.export` 關掉時回 None（呼叫端什麼都不印）。
/// 回（成功與否, 一行說明）；失敗只是警告。
pub fn refresh_vault(project: &str) -> Option<(bool, String)> {
    let profile = read_profile(project);
    let cfg = auto_cfg(&profile);
    if !cfg.export || !is_file(&graph_db(project, &profile)) {
        return None;
    }
    if cfg.update != Some(false) {
        // 先讓圖收進這次的結案摘要；更新不了就用上一版的圖匯出
        let _ = kg::update(Some(project), Some(cfg.timeout));
    }
    let done = kg::layout(Some(project), None, None, None).and_then(|lay| {
        let g = kg::open(&lay)?;
        kg::export::export(&g, &lay, None)
    });
    Some(match done {
        Ok(r) => (true, format!("知識圖的 vault 已重寫：{} 頁（notes/ 沒有動）", r.pages)),
        Err(e) => (false, format!("知識圖的 vault 沒有重寫（{}）；可以之後手動跑 awf kg export", clip(&e.replace('\n', " "), 200))),
    })
}

// ------------------------------------------------------------------ merge 之後併回規格
/// [`spec_apply`] 的結果。
pub enum SpecApply {
    /// 不適用，或已經做過、略過。
    Nothing,
    /// 要人確認的動作（`manual`）。
    Action(Value),
    /// 試跑不過：只在結束的動作裡提醒一句。
    Tip(String),
}

/// merge 段真的合併了：結果的 `merged` 是 true，而且信封裡的 `merge_state`（有的話）都是 merged（queued 不算）。
fn merged(env: &Env, run: &str, data: &Value) -> R<bool> {
    let st = or_obj(py::get(py::idx(data, "stages")?, "merge")?);
    if py::get(&st, "status")?.as_str() != Some("done") {
        return Ok(false);
    }
    let res = stage_result(env, run, "merge")?;
    if !res.get("merged").is_some_and(py::truthy) {
        return Ok(false);
    }
    Ok(sr::envelopes(&res).iter().filter_map(|e| e.get("payload").and_then(|p| p.get("merge_state")).and_then(Value::as_str)).all(|s| s == "merged"))
}

fn mark_spec_apply(env: &Env, data: &mut Value, state: &str, change: &str) -> R<()> {
    let at = env.now_iso();
    setdefault_obj(data, "awf")?.insert("spec_apply".into(), json!({"state": state, "change": change, "at": at}));
    Ok(())
}

/// 需求來源是 openspec、merge 段已經合併時：把這次變更的規格差異併回現行規格的動作。
/// 做過（變更已歸檔）或使用者略過之後記在 run.json 的 `awf.spec_apply`，不再提。
pub fn spec_apply(env: &Env, run: &str, run_arg: &str, data: &mut Value) -> R<SpecApply> {
    if py::truthy(py::get(&or_obj(py::get(data, "awf")?), "spec_apply")?) || !merged(env, run, data)? {
        return Ok(SpecApply::Nothing);
    }
    let ctx = env.sr();
    let (profile, _err) = sr::load_profile_for(&ctx, &[]);
    if !auto_cfg(&profile).spec_apply {
        return Ok(SpecApply::Nothing);
    }
    let sc = match sr::open_spec(&ctx, run, data, &profile) {
        None => return Ok(SpecApply::Nothing),
        Some(Err(e)) => return Ok(SpecApply::Tip(format!("規格差異還沒併回現行規格（{}）。", clip(&e, 200)))),
        Some(Ok(sc)) => sc,
    };
    let Some(change) = sc.change.clone() else {
        // 進行中的變更裡找不到：已經歸檔的就是併過了，記下來不再提；其餘情況沒有東西可以併
        if let Some(done) = sc.tried.iter().find(|n| sc.opened.tree.change(n, spec::ChangeState::Archived).is_some()) {
            mark_spec_apply(env, data, "done", done)?;
        }
        return Ok(SpecApply::Nothing);
    };
    let root = sc.conf.root.to_string_lossy().replace('\\', "/");
    let argv: Vec<String> = ["apply", "--change", &change, "--root", &root].iter().map(|x| x.to_string()).collect();
    let (code, out, err) = spec::run_captured(&argv, std::path::Path::new(&env.project));
    if code != 0 {
        return Ok(SpecApply::Tip(format!(
            "規格差異還沒併回現行規格：試跑沒過（{}）。修好之後跑 awf spec apply --change {change} --root {}。",
            clip(&format!("{out}{err}").replace('\n', " "), 300),
            sc.conf.display
        )));
    }
    Ok(SpecApply::Action(json!({
        "action": "manual", "stage": "merge", "what": "把規格差異併回現行規格",
        "say": format!(
            "這個 run 已經合併。變更「{change}」的規格差異還沒併回現行規格：preview 是試跑的結果（沒有寫任何檔），列給使用者看。\
             使用者同意後由主 session 跑 apply 的指令（寫入現行規格並把變更歸檔），再把改動 commit、照專案的慣例開後續 PR（推送照常要使用者當下同意）。\
             使用者說先不要，就跑 skip 的指令，之後不再提。做完再跑 then。"
        ),
        "preview": clip(&out, PREVIEW_CHARS),
        "apply": format!("awf spec apply --change {change} --root {} --write", sc.conf.display),
        "skip": format!("awf next --run {run_arg} --skip {SKIP_SPEC_APPLY}"),
        "then": format!("awf next --run {run_arg}"),
    })))
}

/// `awf next --skip spec-apply`：記下使用者略過，之後不再提。
pub fn skip(env: &Env, data: &mut Value, what: &str) -> Result<(), Fail> {
    if what != SKIP_SPEC_APPLY {
        return super::awf_err(format!("--skip 只認得 {SKIP_SPEC_APPLY}"));
    }
    if !py::truthy(py::get(&or_obj(py::get(data, "awf")?), "spec_apply")?) {
        mark_spec_apply(env, data, "skipped", "")?;
    }
    Ok(())
}

// ------------------------------------------------------------------ E1：中斷回收
/// 一個 agent 的 transcript 裡最後交的信封（`StructuredOutput`／`SubagentHandback` 的輸入，或最後一段文字裡的 JSON）。
/// transcript 是 Claude Code 的內部格式，沒有承諾穩定：讀不懂就回 None（不收）。
pub fn last_envelope(path: &str) -> Option<Value> {
    let raw = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let mut last = None;
    for line in text.lines() {
        let Ok(Value::Object(e)) = serde_json::from_str::<Value>(line) else { continue };
        let Some(msg) = e.get("message").filter(|m| m.get("role").and_then(Value::as_str) != Some("user")) else { continue };
        match msg.get("content") {
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("tool_use") if matches!(b.get("name").and_then(Value::as_str), Some("StructuredOutput" | "SubagentHandback")) => {
                            if let Some(input) = b.get("input").filter(|i| sr::is_envelope(i)) {
                                last = Some(input.clone());
                            }
                        }
                        Some("text") => {
                            if let Some(v) = b.get("text").and_then(Value::as_str).and_then(schema::extract_json).filter(sr::is_envelope) {
                                last = Some(v);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(Value::String(t)) => {
                if let Some(v) = schema::extract_json(t).filter(sr::is_envelope) {
                    last = Some(v);
                }
            }
            _ => {}
        }
    }
    last
}

fn mtime(p: &str) -> Option<f64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn names_in(dir: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
    v.sort();
    v
}

/// 角度名稱會拿去組紀錄檔路徑：只收單純的名字。
fn angle_ok(a: &str) -> bool {
    !a.is_empty() && a.chars().count() <= 60 && a.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// 一個角度在被中斷的 workflow 裡留下的東西：（修改時間, 信封）。
#[derive(Default)]
struct Found {
    research: Option<(f64, Value)>,
    locate: Option<(f64, Value)>,
}

/// 掃 workflow transcript：這次 next 之後、用這支腳本（或提到這個 run 目錄）跑的 workflow 裡，調研與定位 agent 最後交的信封。
fn scan_transcripts(env: &Env, run: &str, script: &str, since: f64) -> R<BTreeMap<String, Found>> {
    let mut out: BTreeMap<String, Found> = BTreeMap::new();
    let root = transcripts_root(env)?;
    if !is_dir(&root) {
        return Ok(out);
    }
    let now = env.now_ts();
    let run_abs = pyish::resolve(run).unwrap_or_else(|_| run.to_string());
    for sess in names_in(&root) {
        let wfs = pj(&pj(&root, &sess), "subagents/workflows");
        if !is_dir(&wfs) {
            continue;
        }
        for wf in names_in(&wfs).into_iter().filter(|n| n.starts_with("wf_")) {
            let dir = pj(&wfs, &wf);
            // 這支 workflow 是不是這一段的：有紀錄檔就看 scriptPath，沒有就看派工單有沒有提到這個 run 目錄
            let meta = load_json(env, &pj(&pj(&root, &sess), &format!("workflows/{wf}.json"))).ok().flatten();
            let by_script = meta.as_ref().and_then(|m| m.get("scriptPath")).and_then(Value::as_str).map(|sp| pyish::resolve(&pyish::pjoin(&env.project, sp)).is_ok_and(|p| p == script));
            if by_script == Some(false) {
                continue;
            }
            for name in names_in(&dir).into_iter().filter(|n| n.starts_with("agent-") && n.ends_with(".jsonl")) {
                let file = pj(&dir, &name);
                let Some(mt) = mtime(&file) else { continue };
                if mt + 1.0 < since || mt > now + 120.0 {
                    continue;
                }
                let stem = name.strip_suffix(".jsonl").unwrap_or(&name);
                let m = crate::cost::read_meta(&pj(&dir, &format!("{stem}.meta.json")));
                let label = m.get("description").and_then(Value::as_str).unwrap_or_default();
                let role = m.get("agentType").and_then(Value::as_str).unwrap_or_default();
                if role.rsplit(':').next() != Some("tpm-researcher") {
                    continue;
                }
                let Some((kind, angle)) = label.split_once(':').filter(|(k, a)| matches!(*k, "research" | "locate") && angle_ok(a)) else { continue };
                if by_script.is_none() {
                    let head: String = std::fs::read(&file).map(|b| String::from_utf8_lossy(&b[..b.len().min(crate::cost::SCAN_PROMPT_BYTES)]).into_owned()).unwrap_or_default();
                    if !head.contains(&run_abs) {
                        continue;
                    }
                }
                let Some(envl) = last_envelope(&file) else { continue };
                let slot = out.entry(angle.to_string()).or_default();
                let cell = if kind == "research" { &mut slot.research } else { &mut slot.locate };
                // 同一張派工單重派過（升級重試）：取比較晚的那一份
                if cell.as_ref().is_none_or(|(old, _)| mt >= *old) {
                    *cell = Some((mt, envl));
                }
            }
        }
    }
    Ok(out)
}

fn locations_of(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array).map(|a| a.iter().filter(|l| l.get("path").and_then(Value::as_str).is_some_and(|p| !p.is_empty())).cloned().collect()).unwrap_or_default()
}

/// 這次會用的 plan_mode：run.json 鎖定的，否則用 args 的 recipe 解析（沒給就是 feature）。
/// 猜錯沒有壞處：workflow 只沿用 plan_mode 相同的結論，不同就照常重做。
fn guess_plan_mode(profile: &Value, data: &Value) -> String {
    if let Some(m) = data.get("recipe").and_then(|r| r.get("resolved")).and_then(|r| r.get("plan_mode")).and_then(Value::as_str).filter(|m| !m.is_empty()) {
        return m.to_string();
    }
    let args = data.get("args").cloned().unwrap_or(Value::Null);
    let name = args.get("recipe").filter(|n| py::truthy(n)).cloned().unwrap_or_else(|| json!("feature"));
    let mods = match args.get("modifiers") {
        Some(Value::String(m)) => json!(m.split(',').filter(|x| !x.is_empty()).collect::<Vec<_>>()),
        Some(v @ Value::Array(_)) => v.clone(),
        _ => json!([]),
    };
    recipe::resolve(profile, &name, &mods, args.get("size").unwrap_or(&Value::Null)).ok().and_then(|r| r.get("plan_mode").and_then(Value::as_str).map(str::to_string)).filter(|m| !m.is_empty()).unwrap_or_else(|| "feature".to_string())
}

/// plan 段在等待中、但 workflow 沒有結果（被中斷）時，回收已經做完的調研角度，記進 `reuse.json`。回傳回收的角度。
///
/// 只收調研（`research:<角度>`，連同它的 `locate:<角度>`）。每個角度以下全部成立才收，有一項不確定就不收：
/// 1. 這一段用的是標準腳本（不是覆寫檔、不是自訂腳本），而且找不到它的回傳。
/// 2. 紀錄檔 `<run>/plan/research-<角度>.md` 在、不是空的、不是代寫的。
/// 3. 調研信封通過 schema（信封＋findings）而且 status 是 done。
/// 4. 知道它讀了哪些來源（定位信封通過 schema，或這一段的 `pack-args.json` 有工具定位的結果），
///    每個來源檔都讀得到，而且在調研交件之後沒有再改過。
/// 5. `reuse.json` 還沒有這個角度的紀錄（不蓋掉既有的）。
///
/// 寫入用 [`pack::record_reuse`]（和正常存檔同一條路）；之後 `awf pack` 與 `awf save-run stage plan` 照常重新核對來源雜湊。
pub fn recover_interrupted(env: &Env, run: &str, data: &mut Value) -> R<Vec<String>> {
    let pend = or_obj(py::get(&or_obj(py::get(data, "awf")?), "pending")?);
    if s_or(py::get(data, "event")?) != "sdlc" || py::get(&pend, "stage")?.as_str() != Some("plan") || py::get(&pend, "workflow")?.as_str() != Some("sdlc-plan") {
        return Ok(Vec::new());
    }
    if py::truthy(py::get(&pend, "custom")?) || py::truthy(py::get(&pend, "override")?) {
        return Ok(Vec::new());
    }
    let script = match py::get(&pend, "script")? {
        Value::String(t) if !t.is_empty() => pyish::resolve(&if pyish::is_abs(t) { t.clone() } else { pyish::pjoin(&env.project, t) })?,
        _ => return Ok(Vec::new()),
    };
    let since = py::get(&pend, "since")?.as_f64().unwrap_or(0.0);
    if since <= 0.0 {
        return Ok(Vec::new());
    }
    // 有回傳（跑完了只是還沒 done）或分不出是哪一筆：都不是「被中斷」，不回收
    match find_wf_meta(env, &script, since) {
        Ok(None) => {}
        Ok(Some((m, _))) => {
            let r = m.get("result");
            let has_result = r.is_some_and(|r| r.is_object() || r.is_array() || r.as_str().is_some_and(|t| py::loads(t).is_ok_and(|v| v.is_object() || v.is_array())));
            if has_result {
                return Ok(Vec::new());
            }
        }
        Err(_) => return Ok(Vec::new()),
    }
    let found = scan_transcripts(env, run, &script, since)?;
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let ctx = env.sr();
    let (profile, perr) = sr::load_profile_for(&ctx, &[]);
    if !perr.is_empty() {
        return Ok(Vec::new());
    }
    let schemas = schema::Schemas::new(ctx.abs(&ctx.teams));
    let repo_raw = s_or(py::get(data, "target_repo")?);
    let repo = (!repo_raw.is_empty()).then(|| pyish::pjoin(&env.project, &repo_raw));
    let requirement = s_or(py::get(data, "requirement")?);
    let tool_located = load_json(env, &pj(&pj(run, "plan"), pack::ARGS)).ok().flatten().and_then(|v| v.get("located").cloned()).unwrap_or(Value::Null);
    let store = pack::load_store(run);
    let (mut envs, mut locs) = (Map::new(), Map::new());
    for (angle, f) in &found {
        let Some((at, envl)) = &f.research else { continue };
        if store.contains_key(angle) || envl.get("status").and_then(Value::as_str) != Some("done") {
            continue;
        }
        if !schemas.check(envl, Some("findings")).is_ok_and(|errs| errs.is_empty()) {
            continue;
        }
        let rec = pack::record_file(run, angle);
        let rec_text = std::fs::read(&rec).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if rec_text.trim().is_empty() || rec_text.contains(sr::GHOSTWRITTEN) {
            continue;
        }
        // 來源取聯集（定位 agent 交的＋工具定位的）：只會比 workflow 實際給調研 agent 的多，核對只會更嚴
        let mut sources = Vec::new();
        if let Some((_, loc)) = &f.locate {
            if loc.get("status").and_then(Value::as_str) != Some("done") || !schemas.check(loc, Some("locations")).is_ok_and(|errs| errs.is_empty()) {
                continue;
            }
            sources.extend(locations_of(loc.get("payload").and_then(|p| p.get("locations"))));
        }
        sources.extend(locations_of(tool_located.get(angle).and_then(|t| t.get("locations"))));
        if sources.is_empty() {
            continue;
        }
        let fresh = sources.iter().all(|l| {
            let raw = l.get("path").and_then(Value::as_str).unwrap_or_default();
            pack::resolve_source(repo.as_deref(), raw, Some(&env.project)).and_then(|p| mtime(&p)).is_some_and(|m| m <= *at + 1.0)
        });
        if !fresh {
            continue;
        }
        envs.insert(angle.clone(), envl.clone());
        locs.insert(angle.clone(), Value::Array(sources));
    }
    if envs.is_empty() {
        return Ok(Vec::new());
    }
    let synthetic = json!({"research_by_angle": envs, "locations": locs, "recipe": {"plan_mode": guess_plan_mode(&profile, data)}});
    pack::record_reuse(run, repo.as_deref(), &synthetic, &requirement, &env.project)?;
    // 以實際寫進去的為準（來源讀不到的角度 record_reuse 不會記）
    let after = pack::load_store(run);
    let got: Vec<String> = envs.keys().filter(|a| after.contains_key(*a)).cloned().collect();
    if !got.is_empty() {
        let at = env.now_iso();
        setdefault_obj(data, "awf")?.insert("recovered".into(), json!({"stage": "plan", "angles": got, "at": at}));
    }
    Ok(got)
}

/// 閘門重點要列的「中斷回收」：記在 run.json 的回收角度裡，這一段的結果真的沿用了的那些。
pub fn recovered_used(data: &Value, stage: &str, res: &Value) -> Vec<Value> {
    let rec = data.get("awf").and_then(|a| a.get("recovered")).filter(|r| r.get("stage").and_then(Value::as_str) == Some(stage));
    let reused: Vec<&str> = res.get("reused").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    rec.and_then(|r| r.get("angles")).and_then(Value::as_array).map(|a| a.iter().filter(|x| x.as_str().is_some_and(|x| reused.contains(&x))).cloned().collect()).unwrap_or_default()
}
