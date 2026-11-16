//! 連結帳本（D1）與閘門端的連結檢查（D6）。profile 有 `links` 這個物件才啟用；沒設就完全不做事。
//!
//! 帳本 `<runs_dir>/links.jsonl`（格式 `schemas/link.schema.json`）：`awf save-run stage` 每段存檔時，把該段宣告的連結附加進去。
//! 只追加。重存同一段時不重複寫：內容和目前有效的一樣就不寫；不一樣先寫一筆 `op: "supersede"`
//! （這個 run 這一段先前的連結全部作廢），再寫新的。讀帳本用 [`active_links`]。寫失敗只警告。
//!
//! 連結檢查不讀帳本：每條 AC 有沒有 spec 小節、檔案、測試、commit，一律從各段存下來的結果（design.json、test.json）重算。

use super::checks::{ac_ids, result_subtasks};
use super::py::{self, pj, pname, pparent, R};
use super::speccheck::{criteria_rows, lead_token, SpecCtx};
use super::trace::envelopes;
use super::Ctx;
use crate::kb::PyErr;
use crate::spec;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::LazyLock;

pub const LEDGER: &str = "links.jsonl";
pub const LINK_SCHEMA_VERSION: u64 = 1;
/// 連結種類（計畫 D1 的表）。
pub const LINK_KINDS: [&str; 7] = ["req_run", "ac_spec", "ac_file", "ac_test", "ac_commit", "decision_req", "change_deliverable"];
/// 連結檢查在哪些段算。
pub const GAP_STAGES: [&str; 3] = ["design", "test", "deliver"];

/// profile 的 `links`：沒有這個物件回 None（帳本與檢查都不啟用）；有就回 `enforce`（預設 false）。
pub fn links_setting(profile: &Value) -> Option<bool> {
    let l = profile.get("links").filter(|v| v.is_object())?;
    Some(l.get("enforce") == Some(&Value::Bool(true)))
}

fn node(kind: &str, id: &str) -> Value {
    json!({"type": kind, "id": id})
}

fn link(kind: &str, from: Value, to: Value, run: &str, stage: &str, source: &str) -> Value {
    let mut m = Map::new();
    m.insert("schema_version".into(), json!(LINK_SCHEMA_VERSION));
    m.insert("op".into(), json!("add"));
    m.insert("kind".into(), json!(kind));
    m.insert("from".into(), from);
    m.insert("to".into(), to);
    m.insert("run".into(), json!(run));
    m.insert("stage".into(), json!(stage));
    m.insert("source".into(), json!(source));
    m.insert("strength".into(), json!("strong"));
    Value::Object(m)
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// 一條 acceptance 裡的驗收條件 id：開頭是規格 id（需求來源是 openspec 時）就用它，另外收「AC-…」這種編號。
pub fn ac_tokens(entry: &str, known: Option<&BTreeSet<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let tok = lead_token(entry);
    if known.is_some_and(|k| k.contains(tok)) {
        out.push(tok.to_string());
    }
    for id in ac_ids(entry) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// 子任務的驗收條件 id（照出現順序、不重複）。
fn task_acs(task: &Value, known: Option<&BTreeSet<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in strs(task.get("acceptance")) {
        for id in ac_tokens(&e, known) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

fn task_id(task: &Value) -> String {
    task.get("id").and_then(Value::as_str).unwrap_or("?").to_string()
}

/// design 段存下來的子任務（`<run>/design.json`）；讀不到是空的。
fn design_subtasks(ctx: &Ctx, run: &str) -> Vec<Value> {
    ctx.load_json(&pj(run, "design.json")).map(|d| result_subtasks(&d)).unwrap_or_default()
}

static PATHISH: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[\w@.\-]+(?:/[\w@.\-]+)*\.[A-Za-z][A-Za-z0-9]{0,5}").ok());

/// 驗證指令裡長得像檔案的部分（`tests/test_export.py::test_csv` → `tests/test_export.py`）；找不到就用整個指令。
pub fn test_targets(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(re) = PATHISH.as_ref() {
        for m in re.find_iter(command) {
            let t = m.as_str().trim_start_matches("./").to_string();
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    if out.is_empty() && !command.trim().is_empty() {
        out.push(py::take(command.trim(), 200).to_string());
    }
    out
}

/// 逐條結果的 id 對到哪一條 AC：整個 id、去掉分片前綴的 id，或裡面的「AC-…」編號。
fn row_matches(ac: &str, id: &str, bare: &str) -> bool {
    ac == id || ac == bare || ac_ids(id).iter().any(|x| x == ac)
}

fn real(v: &str) -> bool {
    let t = v.trim();
    !t.is_empty() && !t.eq_ignore_ascii_case("n/a") && !t.eq_ignore_ascii_case("none")
}

/// 這個 run 的需求節點：需求來源是 openspec 時是變更與它動到的需求，否則是 epic（沒有 epic 就沒有）。
fn run_requirements(ctx: &Ctx, run: &str, sc: Option<&SpecCtx>) -> Vec<Value> {
    if let Some(change) = sc.and_then(|s| s.change.as_deref().map(|c| (s, c))) {
        let (s, c) = change;
        let mut out = vec![node("change", c)];
        out.extend(spec::change_requirements(&s.opened.tree, c).iter().map(|id| node("requirement", id)));
        return out;
    }
    match super::run_epic(ctx, run) {
        Ok(e) if !e.is_empty() => vec![node("requirement", &e)],
        _ => Vec::new(),
    }
}

/// 這一段宣告的連結（照固定順序、不重複）。`result` 是這一段 workflow 的回傳。
pub fn stage_links(ctx: &Ctx, run: &str, stage: &str, result: &Value, sc: Option<&SpecCtx>) -> Vec<Value> {
    let name = run_name(ctx, run);
    let known = sc.map(SpecCtx::known);
    let known = known.as_ref();
    let mut out: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut push = |v: Value| {
        if seen.insert(py::dumps_sorted(&v)) {
            out.push(v);
        }
    };
    match stage {
        "plan" => {
            for req in run_requirements(ctx, run, sc) {
                push(link("req_run", req, node("run", &name), &name, stage, "run.json:requirement"));
            }
        }
        "design" => {
            for t in result_subtasks(result) {
                let tid = task_id(&t);
                for ac in task_acs(&t, known) {
                    if known.is_some_and(|k| k.contains(&ac)) {
                        push(link("ac_spec", node("ac", &ac), node("spec", &ac), &name, stage, &format!("design.json:subtasks[{tid}].acceptance")));
                    }
                    for r in strs(t.get("spec_refs")) {
                        let target = if known.is_some_and(|k| k.contains(lead_token(&r))) { lead_token(&r).to_string() } else { r };
                        push(link("ac_spec", node("ac", &ac), node("spec", &target), &name, stage, &format!("design.json:subtasks[{tid}].spec_refs")));
                    }
                    for f in strs(t.get("owns")) {
                        push(link("ac_file", node("ac", &ac), node("file", &f), &name, stage, &format!("design.json:subtasks[{tid}].owns")));
                    }
                }
            }
        }
        "build" => {
            let subs = design_subtasks(ctx, run);
            for p in envelopes(result).into_iter().filter_map(|e| e.get("payload")) {
                let (Some(tid), files) = (p.get("task_id").and_then(Value::as_str), strs(p.get("files_changed"))) else { continue };
                for t in subs.iter().filter(|t| task_id(t) == tid) {
                    for ac in task_acs(t, known) {
                        for f in &files {
                            push(link("ac_file", node("ac", &ac), node("file", f), &name, stage, &format!("build.json:files_changed[{tid}]")));
                        }
                    }
                }
            }
        }
        "test" => {
            let mut acs: Vec<String> = Vec::new();
            for t in design_subtasks(ctx, run) {
                for ac in task_acs(&t, known) {
                    if !acs.contains(&ac) {
                        acs.push(ac);
                    }
                }
            }
            for (id, bare, command, commit) in criteria_rows(result) {
                // design 段宣告過的 AC 才連；沒有 design 段時，逐條結果的 id 本身是規格 id 也算
                let mut hit: Vec<String> = acs.iter().filter(|ac| row_matches(ac, &id, &bare)).cloned().collect();
                if hit.is_empty() {
                    hit = [&bare, &id].into_iter().filter(|x| known.is_some_and(|k| k.contains(*x))).take(1).cloned().collect();
                }
                for ac in hit {
                    for target in test_targets(&command) {
                        push(link("ac_test", node("ac", &ac), node("test", &target), &name, stage, &format!("test.json:criteria[{id}].command")));
                    }
                    if real(&commit) {
                        push(link("ac_commit", node("ac", &ac), node("commit", &commit), &name, stage, "test.json:commit"));
                    }
                }
            }
        }
        "deliver" => {
            let updated = result.get("deliverables").and_then(|d| d.get("updated")).and_then(Value::as_array);
            for d in updated.into_iter().flatten().filter(|d| d.is_object()) {
                let Some(id) = d.get("id").and_then(Value::as_str).filter(|x| !x.is_empty()) else { continue };
                let sections: Vec<String> = d.get("sections").and_then(Value::as_array).into_iter().flatten().map(|x| match x {
                    Value::String(t) => t.trim().to_string(),
                    other => other.get("title").or_else(|| other.get("id")).and_then(Value::as_str).unwrap_or_default().trim().to_string(),
                }).filter(|x| !x.is_empty()).collect();
                for sec in sections {
                    push(link("change_deliverable", node("run", &name), node("deliverable", &format!("{id}#{sec}")), &name, stage, "deliver.json:deliverables.updated"));
                }
            }
        }
        "merge" => {
            let merged = result.get("merged") == Some(&Value::Bool(true));
            let sha = result.get("head_sha").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            if merged && real(&sha) {
                for t in design_subtasks(ctx, run) {
                    for ac in task_acs(&t, known) {
                        push(link("ac_commit", node("ac", &ac), node("commit", &sha), &name, stage, "merge.json:head_sha"));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// 決策 → 需求：`decisions.jsonl` 最後 `count` 筆（剛記下的）各連到這個 run 的需求。
/// 決策節點的 id 是那一行內容的 sha1 前 12 碼（`decisions.jsonl` 沒有自己的 id）。`stage` 是 `approve:<段>` 或 `decide`。
pub fn decision_links(ctx: &Ctx, run: &str, stage: &str, count: usize, sc: Option<&SpecCtx>) -> Vec<Value> {
    use sha1::{Digest, Sha1};
    let name = run_name(ctx, run);
    let reqs = run_requirements(ctx, run, sc);
    let Ok(resolved) = ctx.resolve(run) else { return Vec::new() };
    let text = std::fs::read_to_string(pj(&pparent(&resolved), crate::pack::DECISIONS)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out = Vec::new();
    for l in lines.iter().skip(lines.len().saturating_sub(count)) {
        let id: String = Sha1::digest(l.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
        for req in &reqs {
            out.push(link("decision_req", node("decision", &id), req.clone(), &name, stage, crate::pack::DECISIONS));
        }
    }
    out
}

fn run_name(ctx: &Ctx, run: &str) -> String {
    ctx.resolve(run).map(|r| pname(&r)).unwrap_or_else(|_| pname(run))
}

/// 帳本裡目前有效的連結：照順序讀，遇到 `op: "supersede"` 就把同一個 run、同一段先前的連結拿掉。讀不懂的行略過。
pub fn active_links(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
        if !v.is_object() {
            continue;
        }
        if v.get("op").and_then(Value::as_str) == Some("supersede") {
            out.retain(|x| x.get("run") != v.get("run") || x.get("stage") != v.get("stage"));
        } else {
            out.push(v);
        }
    }
    out
}

/// 把連結附加到 `<runs_dir>/links.jsonl`。回傳寫了幾行（0＝內容沒變，不寫）。
/// `replace`：這批是這個 run 這一段的全部連結（重存時取代舊的）；false 時只補上還沒有的（決策連結）。
pub fn write_links(ctx: &Ctx, run: &str, stage: &str, links: &[Value], replace: bool) -> R<usize> {
    use std::io::Write;
    let resolved = ctx.resolve(run)?;
    let file = pj(&pparent(&resolved), LEDGER);
    let name = pname(&resolved);
    let old_text = match std::fs::read(&file) {
        Ok(raw) => String::from_utf8_lossy(&raw).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(PyErr::os(&e, &file)),
    };
    let key = |v: &Value| py::dumps_sorted(v);
    let current: Vec<String> =
        active_links(&old_text).iter().filter(|v| v.get("run").and_then(Value::as_str) == Some(&name) && v.get("stage").and_then(Value::as_str) == Some(stage)).map(key).collect();
    let new: Vec<String> = links.iter().map(key).collect();
    let mut lines: Vec<String> = Vec::new();
    if replace {
        if current == new {
            return Ok(0);
        }
        if !current.is_empty() {
            lines.push(py::dumps_sorted(&json!({"schema_version": LINK_SCHEMA_VERSION, "op": "supersede", "run": name, "stage": stage})));
        }
        lines.extend(new);
    } else {
        lines.extend(new.into_iter().filter(|l| !current.contains(l)));
    }
    if lines.is_empty() {
        return Ok(0);
    }
    let mut body = String::new();
    if !old_text.is_empty() && !old_text.ends_with('\n') {
        body.push('\n');
    }
    for l in &lines {
        body.push_str(l);
        body.push('\n');
    }
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&file).map_err(|e| PyErr::os(&e, &file))?;
    f.write_all(body.as_bytes()).map_err(|e| PyErr::os(&e, &file))?;
    Ok(lines.len())
}

/// 連結檢查：design 段宣告的每條 AC 有沒有 spec 小節、檔案（design 起），測試、commit（test 起）。
/// 只讀各段存下來的結果（`design` 是 design 段的回傳，`test` 是 test 段的回傳），不讀帳本。回傳缺的環節（空＝齊）。
pub fn link_gaps_of(stage: &str, design: &Value, test: Option<&Value>, sc: Option<&SpecCtx>) -> Vec<String> {
    if !GAP_STAGES.contains(&stage) {
        return Vec::new();
    }
    let known = sc.map(SpecCtx::known);
    let known = known.as_ref();
    let subs = result_subtasks(design);
    let mut out: Vec<String> = Vec::new();
    let mut acs: Vec<(String, bool, bool)> = Vec::new();
    for t in &subs {
        let ids = task_acs(t, known);
        if ids.is_empty() && !strs(t.get("acceptance")).is_empty() {
            out.push(format!("子任務 {} 的驗收條件沒有可辨識的 id（「AC-…」編號或規格 id），連不到 spec、測試與 commit", task_id(t)));
        }
        let (has_spec, has_file) = (!strs(t.get("spec_refs")).is_empty(), !strs(t.get("owns")).is_empty());
        for id in ids {
            let in_spec = known.is_some_and(|k| k.contains(&id));
            match acs.iter_mut().find(|(x, _, _)| *x == id) {
                Some(slot) => {
                    slot.1 = slot.1 || has_spec || in_spec;
                    slot.2 = slot.2 || has_file;
                }
                None => acs.push((id, has_spec || in_spec, has_file)),
            }
        }
    }
    // 沒有 test 段的結果（recipe 沒有 test 段）時只看 spec 小節與檔案
    let with_test = stage != "design" && test.is_some();
    let rows = if with_test { test.map(criteria_rows).unwrap_or_default() } else { Vec::new() };
    for (ac, has_spec, has_file) in &acs {
        let mut lack: Vec<&str> = Vec::new();
        if !has_spec {
            lack.push("spec 小節");
        }
        if !has_file {
            lack.push("檔案");
        }
        if with_test {
            let mine: Vec<&(String, String, String, String)> = rows.iter().filter(|(id, bare, _, _)| row_matches(ac, id, bare)).collect();
            if !mine.iter().any(|(_, _, command, _)| real(command)) {
                lack.push("測試");
            }
            if !mine.iter().any(|(_, _, _, commit)| real(commit)) {
                lack.push("commit");
            }
        }
        if !lack.is_empty() {
            out.push(format!("{ac}：缺 {}", lack.join("、")));
        }
    }
    out
}

/// [`link_gaps_of`]，資料取自 run 目錄的 `design.json`、`test.json`（政策核准時重算用）。
pub fn link_gaps(ctx: &Ctx, run: &str, stage: &str, sc: Option<&SpecCtx>) -> Vec<String> {
    if !GAP_STAGES.contains(&stage) {
        return Vec::new();
    }
    let Ok(design) = ctx.load_json(&pj(run, "design.json")) else { return Vec::new() };
    let test = ctx.load_json(&pj(run, "test.json")).ok();
    link_gaps_of(stage, &design, test.as_ref(), sc)
}
