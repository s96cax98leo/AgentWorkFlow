//! 信封、agent 缺件、執行紀錄（trace）的核對與代寫、執行紀錄索引、模型分級。

use super::py::{self, get, get_opt, iter_or, norm, pj, pname, s, s_or, truthy, R};
use super::{Ctx, GHOSTWRITTEN, INDEX_NAME, KERNEL_STAGES, NOT_DELIVERY_KEYS, NULL_OK_KEYS, STEWARD};
use crate::kb::pyish;
use crate::{pack, pyfmt, recipe};
use serde_json::{Map, Value};

pub fn is_envelope(node: &Value) -> bool {
    node.as_object().is_some_and(|o| o.contains_key("status") && o.contains_key("payload"))
}

/// 在回傳結構裡找出所有信封（有 status＋payload 的物件），照走訪順序。
pub fn envelopes(node: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        match node {
            Value::Object(o) => {
                if is_envelope(node) {
                    out.push(node);
                }
                for v in o.values() {
                    walk(v, out);
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

/// 回傳結構裡 agent 缺件的位置：信封之外出現的 null（例如 `analysis: [null]`、`challenge: null`）。
/// 信封內部的欄位由 schema 驗證，不在這裡看；`NULL_OK_KEYS` 是 workflow 自己填的非交件欄位。
pub fn missing_agents(node: &Value) -> Vec<String> {
    fn walk(node: &Value, path: &str, out: &mut Vec<String>) {
        if node.is_null() {
            out.push(path.to_string());
            return;
        }
        if is_envelope(node) {
            return;
        }
        match node {
            Value::Object(o) => {
                for (k, v) in o {
                    if (v.is_null() && NULL_OK_KEYS.contains(&k.as_str())) || (path == "$" && NOT_DELIVERY_KEYS.contains(&k.as_str())) {
                        continue;
                    }
                    walk(v, &format!("{path}.{k}"), out);
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    walk(v, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(node, "$", &mut out);
    out
}

// ------------------------------------------------------------------ 執行紀錄（trace）
/// `_md_cell(v)`
pub fn md_cell(v: &Value) -> String {
    let empty = v.is_null() || v.as_str() == Some("");
    let t = if empty { "—".to_string() } else { s(v) };
    t.replace('|', "\\|").replace('\n', " ")
}

pub fn md_cell_str(t: &str) -> String {
    md_cell(&Value::String(t.to_string()))
}

/// 紀錄檔可能的位置：trace 給的路徑（相對路徑以基準目錄＝專案根為基準），以及 `<run>/<stage>/<檔名>`。
/// trace 給的路徑只在 run 目錄底下才採用：拿 run 以外隨便一個存在的檔當紀錄不算。
pub fn record_candidates(ctx: &Ctx, run: &str, stage: &str, record: &str) -> Vec<String> {
    if record.is_empty() {
        return Vec::new();
    }
    let p = py::pp(record);
    let given = if pyish::is_abs(&p) { p.clone() } else { pj(&ctx.cwd, &p) };
    let mut out = vec![pj(&pj(run, stage), &pname(&p))];
    if !record.contains('\0') && !run.contains('\0') {
        if let (Ok(g), Ok(r)) = (ctx.resolve(&given), ctx.resolve(run)) {
            if pyish::is_under(&g, &r) {
                out.insert(0, given);
            }
        }
    }
    out
}

/// 信封的 artifacts 列了這個 agent 的紀錄檔，就當成它的信封（agent 依派工前綴把紀錄檔列進 artifacts）。
/// 對不到時用 trace 的 digest（執行核心記下的 status＋summary）找：剛好只有一個信封相符才算，零個或多個都不猜。
pub fn envelope_of<'a>(entry: &Value, stage: &str, envs: &[&'a Value]) -> R<Option<&'a Value>> {
    let rec = norm(&s_or(get(entry, "record")?, ""));
    if !rec.is_empty() {
        let tail = format!("{stage}/{}", pname(&rec));
        let suffix = format!("/{tail}");
        for env in envs {
            for a in iter_or(get(env, "artifacts")?)? {
                if let Some(a) = a.as_str() {
                    let a = norm(a);
                    if a == rec || a.ends_with(&suffix) || a == tail {
                        return Ok(Some(env));
                    }
                }
            }
        }
    }
    let dg = get(entry, "digest")?;
    if let Some(sum) = dg.as_object().and_then(|d| d.get("summary")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
        let dstatus = s_or(get(dg, "status")?, "");
        let mut same: Vec<&'a Value> = Vec::new();
        for e in envs {
            if let Some(es) = get(e, "summary")?.as_str() {
                if py::take(es, 400) == sum && s_or(get(e, "status")?, "") == dstatus {
                    same.push(e);
                }
            }
        }
        if let Some(first) = same.first().copied() {
            if same.len() == 1 || same.iter().all(|e| std::ptr::eq(*e, first) || pack::py_eq(e, first)) {
                return Ok(Some(first));
            }
        }
    }
    Ok(None)
}

/// `d.get(k, "")` 再轉成字串。
fn text_of(d: &Value, k: &str) -> R<String> {
    Ok(get_opt(d, k)?.map(s).unwrap_or_default())
}

/// 依信封代寫紀錄檔：dev-merge-steward（沒有 Write 工具），以及有交件但漏寫紀錄檔的 agent。內容只來自信封，可重現。
pub fn ghostwrite(ctx: &Ctx, path: &str, entry: &Value, env: &Value) -> R<()> {
    let empty = Value::Object(Map::new());
    let p = match get(env, "payload")? {
        v @ Value::Object(_) => v,
        _ => &empty,
    };
    let steward = py::contains(p, "traceability")?;
    let why = if steward { "沒有 Write 工具" } else { "有交件但沒有寫紀錄檔" };
    let mut lines: Vec<String> = vec![
        format!("# 執行紀錄：{}（{GHOSTWRITTEN}）", text_of(entry, "label")?),
        String::new(),
        format!("> {} {why}，這份紀錄由 awf save-run 依它交回的信封代寫，內容只來自信封。", text_of(entry, "agent_type")?),
        String::new(),
        "## 任務摘要".to_string(),
        String::new(),
        s_or(get(env, "summary")?, "（信封沒有 summary）"),
        String::new(),
        "## 做了什麼與證據".to_string(),
        String::new(),
    ];
    let mut ev: Vec<String> = Vec::new();
    for e in iter_or(get(env, "evidence")?)? {
        if e.is_object() {
            ev.push(format!("- {}：{}（出處：{}）", text_of(&e, "claim")?, text_of(&e, "result")?, text_of(&e, "source")?));
        }
    }
    if ev.is_empty() {
        ev.push("- （信封沒有 evidence）".to_string());
    }
    lines.extend(ev);
    lines.extend([String::new(), "## 結論".to_string(), String::new()]);
    if steward {
        let reasons: Vec<String> = iter_or(get(p, "gate_reasons")?)?.iter().map(s).collect();
        let reasons = reasons.join("；");
        lines.extend([
            format!("- decision：{}", text_of(p, "decision")?),
            format!("- head_sha：{}", text_of(p, "head_sha")?),
            format!("- merge_state：{}", text_of(p, "merge_state")?),
            format!("- 需求對照：{} 條；followups：{} 則", py::len_or(get(p, "traceability")?)?, py::len_or(get(p, "followups")?)?),
            format!("- gate_reasons：{}", if reasons.is_empty() { "（無）" } else { &reasons }),
        ]);
    } else {
        let mut keys: Vec<String> = p.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        keys.sort();
        let keys = keys.join("、");
        lines.extend([
            format!("- status：{}", text_of(env, "status")?),
            format!("- confidence：{}", text_of(env, "confidence")?),
            format!("- payload 的欄位：{}", if keys.is_empty() { "（無）" } else { &keys }),
        ]);
    }
    lines.extend([String::new(), "## 成果檔".to_string(), String::new()]);
    let arts: Vec<String> = iter_or(get(env, "artifacts")?)?.iter().map(|a| format!("- {}", s(a))).collect();
    if arts.is_empty() {
        lines.push("- （無）".to_string());
    } else {
        lines.extend(arts);
    }
    lines.extend([String::new(), "## 不確定的地方".to_string(), String::new()]);
    let mut qs: Vec<String> = Vec::new();
    for q in iter_or(get(env, "open_questions")?)? {
        if q.is_object() {
            qs.push(format!("- {}", text_of(&q, "question")?));
        }
    }
    if qs.is_empty() {
        qs.push("- （信封沒有待決問題）".to_string());
    }
    lines.extend(qs);
    ctx.mkdir_p(&py::pparent(path))?;
    ctx.write_text(path, &format!("{}\n", lines.join("\n")))
}

/// 檔案的第一行（讀不到回空字串；不是 utf-8 的位元組換掉）。
fn head_line(ctx: &Ctx, path: &str) -> String {
    match std::fs::read(ctx.abs(path)) {
        Ok(raw) => {
            let end = raw.iter().position(|b| *b == b'\n' || *b == b'\r').unwrap_or(raw.len());
            String::from_utf8_lossy(raw.get(..end).unwrap_or(&raw)).into_owned()
        }
        Err(_) => String::new(),
    }
}

/// trace[i] 標了 superseded，而且後面有同一個 label、同一個角色、ok=true 的重派（不只看旗標，要真的有後面那筆）。
pub fn retried_ok(trace: &[Value], i: usize) -> R<bool> {
    let Some(t) = trace.get(i) else { return Ok(false) };
    if get(t, "superseded")? != &Value::Bool(true) {
        return Ok(false);
    }
    let (label, agent) = (get(t, "label")?, get(t, "agent_type")?);
    Ok(trace.iter().skip(i + 1).any(|x| {
        x.as_object().is_some_and(|o| {
            o.get("ok") == Some(&Value::Bool(true)) && pyfmt::eq(o.get("label").unwrap_or(&py::NULL), label) && pyfmt::eq(o.get("agent_type").unwrap_or(&py::NULL), agent)
        })
    }))
}

/// 模型分級（I7）：trace 記的等級不能低於下限，model 要等於該等級對應的模型。
/// profile 沒有 models 或 mode=inherit 時不檢查；trace 沒有 tier 的舊 workflow 只在 `KERNEL_STAGES` 判錯。
/// `profile` 是 null 表示沒有 profile。
pub fn model_errors(stage: &str, result: &Value, profile: &Value, modifiers: &Value) -> R<Vec<String>> {
    model_errors_with(stage, result, profile, modifiers, None)
}

/// profile 的 `roles` 開放的七個步驟：這一段、這個 label 是哪一步，以及那一步原本的模版角色。
/// label 的寫法照範本（`locate:<角度>`、`review:<軸>`…）；執行核心替重複的 label 加的序號（`#2`）先去掉。
/// 步驟清單要和 [`crate::schema::SWAPPABLE_STEPS`]、challenge 片段的 `awfSwappable` 一致。
pub fn swapped_step(stage: &str, label: &str) -> Option<(&'static str, &'static str)> {
    let base = match label.rsplit_once('#') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => label,
    };
    const R: &str = "tpm-researcher";
    const K: &str = "tpm-skeptic";
    match stage {
        "plan" if base.starts_with("locate:") => Some(("plan.locate", R)),
        "plan" if base.starts_with("research:") => Some(("plan.research", R)),
        "plan" if base == "challenge:intent" => Some(("plan.challenge", K)),
        "design" if base.starts_with("analyze:") => Some(("design.analyze", K)),
        "design" if base.starts_with("recheck:") => Some(("design.recheck", K)),
        "deliver" if base.starts_with("review:") => Some(("deliver.review", "tpm-reviewer")),
        "deliver" if base.starts_with("verify:") => Some(("deliver.verify", K)),
        _ => None,
    }
}

/// 同 [`model_errors`]，另外處理「派的不是模版角色，但這一步其實是模版角色的工作」：
///
/// 1. `merger`（`guard.config.json` 明寫的合併角色，[`crate::config::custom_merger`]）：照 `dev-merge-steward` 的下限算。
/// 2. profile 的 `roles` 把這一步換成這個 agent（依這一段與 label 反查，[`swapped_step`]）：照被換掉的角色算。
///    trace 自己記的 `role_as` 不採信；和反查的結果不一樣另外列一則。
/// 3. 以上都對不到、但 trace 記了 `role_as`：照它說的角色算（只會多查，不會少查）；不是模版角色的名字算錯。
/// 4. 其餘是使用者自己的 agent（擴充 agent）：執行核心不替它指定模型，也沒有下限。
pub fn model_errors_with(stage: &str, result: &Value, profile: &Value, modifiers: &Value, merger: Option<&str>) -> R<Vec<String>> {
    let models = if truthy(profile) { get(profile, "models")? } else { &py::NULL };
    let Some(mo) = models.as_object() else { return Ok(Vec::new()) };
    if mo.get("mode").is_some_and(|m| m.as_str() == Some("inherit")) {
        return Ok(Vec::new());
    }
    let empty: Vec<Value> = Vec::new();
    let trace = result.as_object().and_then(|r| r.get("trace")).and_then(Value::as_array).unwrap_or(&empty);
    let mut errs: Vec<String> = Vec::new();
    let tier_index = |t: &Value| recipe::TIERS.iter().position(|x| t.as_str() == Some(*x));
    let roles = profile.get("roles").and_then(Value::as_object);
    for t in trace {
        if !t.is_object() {
            continue;
        }
        let label = s_or(get(t, "label")?, "");
        let agent = s_or(get(t, "agent_type")?, "");
        let mut role = agent.rsplit(':').next().unwrap_or_default().to_string();
        let tier = get(t, "tier")?;
        let is_template = |r: &str| r.starts_with("tpm-") || r.starts_with("dev-");
        if !is_template(&role) {
            let claimed = get(t, "role_as")?;
            let mapped = swapped_step(stage, &label).filter(|(step, _)| !agent.is_empty() && roles.and_then(|r| r.get(*step)).and_then(Value::as_str) == Some(agent.as_str()));
            let actual: Option<String> = if !agent.is_empty() && merger == Some(agent.as_str()) {
                Some(super::STEWARD.to_string())
            } else {
                mapped.map(|(_, r)| r.to_string())
            };
            match (actual, claimed.as_str()) {
                (Some(want), said) => {
                    if said.is_some_and(|x| x != want) || (said.is_none() && !claimed.is_null()) {
                        errs.push(format!("{label}（{agent}）的 trace 記 role_as={}，但這一步照設定是 {want} 的工作：下限照 {want} 算", s(claimed)));
                    }
                    role = want;
                }
                (None, Some(said)) if is_template(said) => role = said.to_string(),
                (None, _) if !claimed.is_null() => {
                    errs.push(format!("{label}（{agent}）的 role_as「{}」不是模版角色", s(claimed)));
                    continue;
                }
                // 使用者自己的 agent（擴充 agent）：執行核心不替它指定模型，也沒有下限
                (None, _) => continue,
            }
        }
        if tier.is_null() || tier.as_str() == Some("") {
            // 自訂段（profile 的 flow）的腳本一樣由執行核心派工
            if KERNEL_STAGES.contains(&stage) || crate::stages::Resolved::of(profile).is_custom(stage) {
                errs.push(format!("{label} 的 trace 沒有 model／tier（profile 有設 models，這一段的派工都要經過執行核心）"));
            }
            continue;
        }
        let Some(ti) = tier_index(tier) else {
            errs.push(format!("{label} 的等級「{}」不認得", s(tier)));
            continue;
        };
        let resolved = recipe::resolve_model(profile, &Value::String(role.clone()), &Value::String(stage.to_string()), &py::NULL, modifiers).map_err(py::from_recipe)?;
        let floor = py::idx(&resolved, "floor")?;
        if truthy(floor) {
            let fi = tier_index(floor).ok_or_else(|| value_err(floor))?;
            if ti < fi {
                errs.push(format!("{label}（{role}）用了 {}，低於下限 {}", s(tier), s(floor)));
            }
        }
        let want = recipe::tier_model(profile, tier).map_err(py::from_recipe)?;
        let want = if want.is_empty() { "inherit".to_string() } else { want };
        let model = get(t, "model")?;
        if model.as_str() != Some(want.as_str()) {
            errs.push(format!("{label} 的等級是 {}，model 應該是 {want}，trace 記的是 {}", s(tier), s(model)));
        }
    }
    Ok(errs)
}

fn value_err(v: &Value) -> crate::kb::PyErr {
    crate::kb::PyErr::other("ValueError", format!("{} is not in list", pyfmt::repr(v)))
}

/// 執行紀錄索引的一列。
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// `t.get("agent_type") or ""`
    pub agent_type: Value,
    /// `t.get("phase") or ""`
    pub phase: Value,
    pub record: String,
    pub artifacts: Vec<String>,
    pub status: String,
}

fn or_empty(v: &Value) -> Value {
    if truthy(v) { v.clone() } else { Value::String(String::new()) }
}

/// 依回傳的 trace 核對紀錄檔。回傳（有沒有 trace、索引列、缺件理由）。
/// `write=true` 時代寫紀錄檔並寫 `<run>/<stage>/執行紀錄索引.md`；`write=false` 只核對（政策判定用）。
pub fn process_trace(ctx: &Ctx, run: &str, stage: &str, result: &Value, write: bool) -> R<(bool, Vec<Row>, Vec<String>)> {
    let Some(ro) = result.as_object() else { return Ok((false, Vec::new(), Vec::new())) };
    let Some(trace) = ro.get("trace") else { return Ok((false, Vec::new(), Vec::new())) };
    let Some(trace) = trace.as_array() else {
        return Ok((true, Vec::new(), vec!["trace 格式不對（不是陣列），無法核對執行紀錄".to_string()]));
    };
    let stripped = Value::Object(ro.iter().filter(|(k, _)| k.as_str() != "trace").map(|(k, v)| (k.clone(), v.clone())).collect());
    let envs = envelopes(&stripped);
    let mut stewards = envs.iter().copied().filter(|e| e.get("payload").and_then(Value::as_object).is_some_and(|p| p.contains_key("traceability")));
    let mut rows: Vec<Row> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let dir = pj(run, stage);
    // guard.config.json 明寫的合併角色（角色名不是 dev-merge-steward 的）：只認設定檔，不認 trace 的說法
    let merger = crate::config::custom_merger(&ctx.project_root());
    for (i, t) in trace.iter().enumerate() {
        if !t.is_object() {
            missing.push(format!("trace[{i}] 格式不對"));
            continue;
        }
        let label = s_or(get(t, "label")?, "");
        let mut record = s_or(get(t, "record")?, "");
        let okv = get(t, "ok")?;
        let mut env = envelope_of(t, stage, &envs)?;
        let agent_type = s_or(get(t, "agent_type")?, "");
        let named = agent_type.ends_with(STEWARD);
        let custom = !named && !agent_type.is_empty() && merger.as_deref() == Some(agent_type.as_str());
        // 設定檔指定的合併角色可能有 Write 工具、自己寫了紀錄檔：那就照一般角色核對，不拿代寫的蓋掉；沒寫才和 dev-merge-steward 一樣依信封代寫。
        // 兩種情況都照順序取用 merge-decision 信封，後面的派工才對得到自己的那一份
        let own_record = custom && record_candidates(ctx, run, stage, &record).into_iter().any(|c| ctx.is_file(&c) && !head_line(ctx, &c).contains(GHOSTWRITTEN));
        let is_steward = named || (custom && !own_record);
        if own_record && okv == &Value::Bool(true) {
            let next = stewards.next();
            env = env.or(next);
        }
        let mut status = "完成".to_string();
        if okv == &Value::Bool(true) && is_steward {
            env = stewards.next();
            let name = if record.is_empty() { format!("{label}.md") } else { record.clone() };
            let target = pj(&dir, &pname(&name));
            match env {
                None => {
                    status = "缺紀錄檔（找不到 steward 的信封，無法代寫）".to_string();
                    missing.push(format!("{label} 的紀錄檔無法代寫：回傳裡找不到對應的 merge-decision 信封"));
                }
                Some(e) => {
                    if write {
                        ghostwrite(ctx, &target, t, e)?;
                    }
                    if record.is_empty() {
                        record = target.clone();
                    }
                    if ctx.is_file(&target) {
                        status = format!("完成（{GHOSTWRITTEN}紀錄）");
                    } else {
                        status = "缺紀錄檔".to_string();
                        missing.push(format!("{label} 的紀錄檔 {target} 不存在（steward 的紀錄要由 save_run stage 代寫）"));
                    }
                }
            }
        } else if okv == &Value::Bool(true) {
            let found = record_candidates(ctx, run, stage, &record).into_iter().find(|c| ctx.is_file(c));
            if record.is_empty() {
                status = "缺紀錄檔（trace 沒有指定紀錄檔）".to_string();
                missing.push(format!("{label} 的 trace 沒有指定紀錄檔"));
            } else if let Some(found) = found {
                let ghost = head_line(ctx, &found).contains(GHOSTWRITTEN);
                if env.is_none() && ghost {
                    // 紀錄檔是之前代寫的，這次的回傳卻對不回信封（回傳被換過）：不沿用舊的代寫檔
                    status = "缺紀錄檔（代寫的紀錄對不回這次的信封）".to_string();
                    missing.push(format!("{label} 的紀錄檔是代寫的，但這次的回傳裡找不到對應的信封"));
                } else if ghost {
                    status = format!("完成（{GHOSTWRITTEN}紀錄：agent 沒寫紀錄檔）");
                }
            } else {
                let target = pj(&dir, &pname(&record));
                if let (Some(e), true) = (env, write) {
                    ghostwrite(ctx, &target, t, e)?;
                }
                if env.is_some() && ctx.is_file(&target) {
                    status = format!("完成（{GHOSTWRITTEN}紀錄：agent 沒寫紀錄檔）");
                } else {
                    status = "缺紀錄檔".to_string();
                    missing.push(format!("{label} 的紀錄檔 {record} 不存在{}", if env.is_none() { "" } else { "（還沒代寫：先跑 awf save-run stage）" }));
                }
            }
        } else if okv == &Value::Bool(false) && retried_ok(trace, i)? {
            // 執行核心的失敗升級：這一筆沒交件，後面同一個 label、同一個角色的重派有交件，不算缺件
            status = format!("沒有交件，已升一級重派（{} → 見下一筆）", s_or(get(t, "tier")?, "?"));
        } else if okv == &Value::Bool(false) && get(t, "capped")? == &Value::Bool(true) {
            status = "沒有派出（派工數到上限）".to_string();
            missing.push(format!("{label} 沒有派出：派工數到上限（{}；profile 的 limits.max_agents），要人決定是調高上限還是拆小", s_or(get(t, "max_agents")?, "?")));
        } else if okv == &Value::Bool(false) {
            status = "agent 沒有交件（失敗或交件無效）".to_string();
            missing.push(format!("{label} 沒有交件（失敗或交件無效）"));
        } else {
            status = "未完成（ok 不是 true／false）".to_string();
            missing.push(format!("{label} 未完成（trace 的 ok 不是 true）"));
        }
        let mut arts: Vec<String> = Vec::new();
        if let Some(e) = env.filter(|e| truthy(e)) {
            let nrec = norm(&record);
            let suffix = format!("/{stage}/{}", pname(&record));
            for a in iter_or(get(e, "artifacts")?)? {
                if let Some(a) = a.as_str() {
                    if norm(a) != nrec && !norm(a).ends_with(&suffix) {
                        arts.push(a.to_string());
                    }
                }
            }
        }
        rows.push(Row { label, agent_type: or_empty(get(t, "agent_type")?), phase: or_empty(get(t, "phase")?), record, artifacts: arts, status });
    }
    if write {
        ctx.mkdir_p(&dir)?;
        let mut lines: Vec<String> = vec![
            format!("# 執行紀錄索引：{stage}"),
            String::new(),
            format!(
                "由 awf save-run 依 workflow 回傳的 trace 產生，每列是一次派工（共 {} 次）。agent 有交件但沒寫紀錄檔時，由 save_run 依信封代寫（狀態欄會標明）；連信封都對不回才算缺件，不能政策核准。成果檔取自該 agent 信封的 artifacts。",
                rows.len()
            ),
            String::new(),
            "| label | 角色 | phase | 紀錄檔 | 成果檔 | 狀態 |".to_string(),
            "|---|---|---|---|---|---|".to_string(),
        ];
        for r in &rows {
            let arts = if r.artifacts.is_empty() { "—".to_string() } else { r.artifacts.join("<br>") };
            let cells = [md_cell_str(&r.label), md_cell(&r.agent_type), md_cell(&r.phase), md_cell_str(&r.record), md_cell_str(&arts), md_cell_str(&r.status)];
            lines.push(format!("| {} |", cells.join(" | ")));
        }
        if !missing.is_empty() {
            lines.extend([String::new(), "## 缺件".to_string(), String::new()]);
            lines.extend(missing.iter().map(|m| format!("- {m}")));
        }
        ctx.write_text(&pj(&dir, INDEX_NAME), &format!("{}\n", lines.join("\n")))?;
    }
    Ok((true, rows, missing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(roles: Value) -> Value {
        json!({"models": {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"}, "defaults": {"*": "light"}, "floors": {}}, "roles": roles})
    }

    fn errs(stage: &str, p: &Value, entry: Value, merger: Option<&str>) -> Vec<String> {
        model_errors_with(stage, &json!({"trace": [entry]}), p, &json!([]), merger).unwrap_or_else(|e| vec![format!("例外：{}", e.msg)])
    }

    fn entry(label: &str, agent: &str, tier: &str, model: &str, extra: Value) -> Value {
        let mut t = json!({"label": label, "agent_type": agent, "ok": true, "tier": tier, "model": model});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            t[k] = v;
        }
        t
    }

    #[test]
    fn swapped_step_follows_the_templates_labels() {
        assert_eq!(swapped_step("plan", "locate:requirement"), Some(("plan.locate", "tpm-researcher")));
        assert_eq!(swapped_step("plan", "research:upstream#2"), Some(("plan.research", "tpm-researcher")));
        assert_eq!(swapped_step("plan", "challenge:intent"), Some(("plan.challenge", "tpm-skeptic")));
        assert_eq!(swapped_step("design", "analyze:coverage"), Some(("design.analyze", "tpm-skeptic")));
        assert_eq!(swapped_step("design", "recheck:plan"), Some(("design.recheck", "tpm-skeptic")));
        assert_eq!(swapped_step("deliver", "review:bugs"), Some(("deliver.review", "tpm-reviewer")));
        assert_eq!(swapped_step("deliver", "verify:bugs"), Some(("deliver.verify", "tpm-skeptic")));
        // 別段的同名 label、不開放的步驟都不是
        for (stage, label) in [("test", "verify:api#1"), ("design", "review:bugs"), ("plan", "challenge:other"), ("merge", "analyze"), ("build", "Implement:t1"), ("plan", "intent")] {
            assert_eq!(swapped_step(stage, label), None, "{stage} {label}");
        }
        // 步驟清單和 profile 檢查認的同一份
        let mine = [("plan", "locate:x"), ("plan", "research:x"), ("plan", "challenge:intent"), ("design", "analyze:x"), ("design", "recheck:x"), ("deliver", "review:x"), ("deliver", "verify:x")];
        let steps: Vec<&str> = mine.iter().filter_map(|(s, l)| swapped_step(s, l).map(|x| x.0)).collect();
        assert_eq!(steps, crate::schema::SWAPPABLE_STEPS);
    }

    /// 換上去的 agent 照被換掉的角色算下限：低於下限判錯、剛好通過；model 也要對得上那一級。
    #[test]
    fn swapped_agent_keeps_the_floor_of_the_role_it_replaces() {
        let p = profile(json!({"plan.challenge": "acme-skeptic"}));
        let role_as = json!({"role_as": "tpm-skeptic"});
        let low = errs("plan", &p, entry("challenge:intent", "acme-skeptic", "light", "haiku", role_as.clone()), None);
        assert!(low.len() == 1 && low[0].contains("（tpm-skeptic）用了 light，低於下限 standard"), "{low:?}");
        assert!(errs("plan", &p, entry("challenge:intent", "acme-skeptic", "standard", "sonnet", role_as.clone()), None).is_empty());
        assert!(errs("plan", &p, entry("challenge:intent", "acme-skeptic", "strong", "opus", role_as.clone()), None).is_empty());
        let wrong_model = errs("plan", &p, entry("challenge:intent", "acme-skeptic", "standard", "haiku", role_as), None);
        assert!(wrong_model.len() == 1 && wrong_model[0].contains("model 應該是 sonnet"), "{wrong_model:?}");
    }

    /// trace 不帶 role_as、或謊報成下限較低的角色：照 profile 的 roles 反查，不採信 trace。
    #[test]
    fn role_as_in_the_trace_is_not_trusted() {
        let p = profile(json!({"plan.challenge": "acme-skeptic"}));
        let missing = errs("plan", &p, entry("challenge:intent", "acme-skeptic", "light", "haiku", json!({})), None);
        assert!(missing.len() == 1 && missing[0].contains("低於下限 standard"), "{missing:?}");
        let lied = errs("plan", &p, entry("challenge:intent", "acme-skeptic", "light", "haiku", json!({"role_as": "tpm-researcher"})), None);
        assert!(lied.iter().any(|e| e.contains("role_as=tpm-researcher") && e.contains("tpm-skeptic 的工作")) && lied.iter().any(|e| e.contains("（tpm-skeptic）用了 light，低於下限 standard")), "{lied:?}");
        let odd = errs("plan", &p, entry("challenge:intent", "acme-skeptic", "standard", "sonnet", json!({"role_as": 7})), None);
        assert!(odd.len() == 1 && odd[0].contains("role_as=7"), "{odd:?}");
        // 沒有經過執行核心（沒有 tier）：和模版角色一樣判錯
        let bare = errs("plan", &p, json!({"label": "challenge:intent", "agent_type": "acme-skeptic", "ok": true}), None);
        assert!(bare.len() == 1 && bare[0].contains("沒有 model／tier"), "{bare:?}");
        // 核心替重複的 label 加的序號不影響反查
        let nth = errs("plan", &p, entry("challenge:intent#2", "acme-skeptic", "light", "haiku", json!({})), None);
        assert!(nth.len() == 1 && nth[0].contains("低於下限 standard"), "{nth:?}");
    }

    /// profile 沒有把這一步換成這個 agent：沒有 role_as 就是擴充 agent（和原本一樣不檢查）；
    /// 有 role_as 就照它說的角色查（只會多查），不是模版角色的名字算錯。
    #[test]
    fn unmapped_agents_are_checked_only_when_they_claim_a_role() {
        let none = profile(json!({}));
        let other = profile(json!({"plan.challenge": "someone-else"}));
        for p in [&none, &other] {
            assert!(errs("plan", p, json!({"label": "challenge:intent", "agent_type": "acme-skeptic", "ok": true}), None).is_empty());
            assert!(errs("plan", p, json!({"label": "ext:db", "agent_type": "my:agent", "ok": true}), None).is_empty());
            let claimed = errs("plan", p, entry("challenge:intent", "acme-skeptic", "light", "haiku", json!({"role_as": "tpm-skeptic"})), None);
            assert!(claimed.len() == 1 && claimed[0].contains("低於下限 standard"), "{claimed:?}");
            let junk = errs("plan", p, entry("x", "acme-skeptic", "light", "haiku", json!({"role_as": "nobody"})), None);
            assert!(junk.len() == 1 && junk[0].contains("不是模版角色"), "{junk:?}");
        }
        // 模版角色自己帶 role_as 不影響：照自己的角色名算
        let own = errs("plan", &none, entry("challenge:intent", "agent-workflow:tpm-skeptic", "light", "haiku", json!({"role_as": "tpm-researcher"})), None);
        assert!(own.len() == 1 && own[0].contains("（tpm-skeptic）用了 light，低於下限 standard"), "{own:?}");
        // profile 沒有 models：全部不檢查（和原本一樣）
        assert!(errs("plan", &json!({"roles": {"plan.challenge": "acme-skeptic"}}), entry("challenge:intent", "acme-skeptic", "light", "haiku", json!({})), None).is_empty());
    }

    /// guard.config 指定的合併角色照 dev-merge-steward 的下限算，不管 trace 記了什麼；沒指定時這個 agent 就是擴充 agent。
    #[test]
    fn configured_merger_gets_the_steward_floor() {
        let p = profile(json!({}));
        let m = Some("acme:release-merger");
        for extra in [json!({}), json!({"role_as": "dev-merge-steward"})] {
            let low = errs("merge", &p, entry("execute:merge", "acme:release-merger", "standard", "sonnet", extra.clone()), m);
            assert!(low.len() == 1 && low[0].contains("（dev-merge-steward）用了 standard，低於下限 strong"), "{low:?}");
            assert!(errs("merge", &p, entry("execute:merge", "acme:release-merger", "strong", "opus", extra), m).is_empty());
        }
        let lied = errs("merge", &p, entry("analyze", "acme:release-merger", "light", "haiku", json!({"role_as": "tpm-writer"})), m);
        assert!(lied.iter().any(|e| e.contains("role_as=tpm-writer")) && lied.iter().any(|e| e.contains("低於下限 strong")), "{lied:?}");
        let bare = errs("merge", &p, json!({"label": "analyze", "agent_type": "acme:release-merger", "ok": true}), m);
        assert!(bare.len() == 1 && bare[0].contains("沒有 model／tier"), "{bare:?}");
        // 設定檔沒指定：和原本一樣不檢查；別的 agent 不受影響
        assert!(errs("merge", &p, json!({"label": "analyze", "agent_type": "acme:release-merger", "ok": true}), None).is_empty());
        assert!(errs("merge", &p, json!({"label": "ext:x", "agent_type": "acme:other", "ok": true}), m).is_empty());
    }
}
