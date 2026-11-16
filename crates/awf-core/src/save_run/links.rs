//! 連結帳本（D1）與閘門端的連結檢查（D6）。profile 有 `links` 這個物件才啟用；沒設就完全不做事。
//!
//! 帳本 `<runs_dir>/links.jsonl`（格式 `schemas/link.schema.json`）：`awf save-run stage` 每段存檔時，把該段宣告的連結附加進去。
//! 只追加。重存同一段時不重複寫：內容和目前有效的一樣就不寫；不一樣先寫一筆 `op: "supersede"`
//! （這個 run 這一段先前的連結全部作廢），再寫新的。讀帳本用 [`active_links`]。寫失敗只警告。
//!
//! 連結檢查不讀帳本：每條 AC 有沒有 spec 小節、檔案、測試、commit，一律從各段存下來的結果（design.json、build.json、test.json）重算。
//! 不只看「有沒有宣告」，還看宣告的東西是不是真的（[`link_gaps`] 的說明列了保證什麼、不保證什麼）。
//!
//! 帳本的內容之後會被知識圖顯示給主 session：每個 id、目標、出處寫入前都過 [`safe`]。

use super::checks::{criteria_pattern, ids_with, result_subtasks};
use crate::hooks::re::Re;
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

/// 帳本裡一個字串最多幾個字。
pub const LEDGER_TEXT_MAX: usize = 200;

/// 寫進帳本的字串（id、目標、出處、run 名稱）：這些字出自 agent 交的結果，之後會被顯示給主 session。
/// 控制字元（含換行）換成空白，反引號與角括號拿掉，去頭尾空白，超過 [`LEDGER_TEXT_MAX`] 字截短。
pub fn safe(text: &str) -> String {
    let cleaned: String = text.chars().filter(|c| !matches!(c, '`' | '<' | '>')).map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') { ' ' } else { c }).collect();
    cleaned.trim().chars().take(LEDGER_TEXT_MAX).collect::<String>().trim_end().to_string()
}

fn node(kind: &str, id: &str) -> Value {
    json!({"type": kind, "id": safe(id)})
}

fn link(kind: &str, from: Value, to: Value, run: &str, stage: &str, source: &str) -> Value {
    let mut m = Map::new();
    m.insert("schema_version".into(), json!(LINK_SCHEMA_VERSION));
    m.insert("op".into(), json!("add"));
    m.insert("kind".into(), json!(kind));
    m.insert("from".into(), from);
    m.insert("to".into(), to);
    m.insert("run".into(), json!(safe(run)));
    m.insert("stage".into(), json!(stage));
    m.insert("source".into(), json!(safe(source)));
    m.insert("strength".into(), json!("strong"));
    Value::Object(m)
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// 連結檢查認驗收條件編號的樣式：profile 的 `stages.design.criteria_id_pattern`（和拆工檢查同一個來源，
/// [`criteria_pattern`]）。沒寫回 `None`（內建的「AC-…」）；寫壞了也回 `None`——profile 的檢查與拆工檢查會把它列成問題，
/// 這裡退回內建樣式只會讓自訂編號連不起來（列成缺口），不會少查。
pub fn id_pattern(profile: &Value) -> Option<Re> {
    criteria_pattern(profile).ok().flatten()
}

/// 訊息裡怎麼稱呼驗收條件的編號。
fn id_name(pat: Option<&Re>) -> &'static str {
    if pat.is_some() { "符合 stages.design.criteria_id_pattern 的編號" } else { "「AC-…」編號" }
}

/// 一條 acceptance 裡的驗收條件 id：開頭是規格 id（需求來源是 openspec 時）就用它，另外收「AC-…」這種編號。
pub fn ac_tokens(entry: &str, known: Option<&BTreeSet<String>>) -> Vec<String> {
    ac_tokens_with(entry, known, None)
}

/// 同 [`ac_tokens`]；`pat` 有給時編號照它認（沒給用內建的「AC-…」）。
pub fn ac_tokens_with(entry: &str, known: Option<&BTreeSet<String>>, pat: Option<&Re>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let tok = lead_token(entry);
    if known.is_some_and(|k| k.contains(tok)) {
        out.push(tok.to_string());
    }
    for id in ids_with(entry, pat) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// 子任務的驗收條件 id（照出現順序、不重複）。
fn task_acs(task: &Value, known: Option<&BTreeSet<String>>, pat: Option<&Re>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in strs(task.get("acceptance")) {
        for id in ac_tokens_with(&e, known, pat) {
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
fn row_matches(ac: &str, id: &str, bare: &str, pat: Option<&Re>) -> bool {
    ac == id || ac == bare || ids_with(id, pat).iter().any(|x| x == ac)
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
    stage_links_with(ctx, run, stage, result, sc, None)
}

/// 同 [`stage_links`]；`pat` 是驗收條件編號的樣式（[`id_pattern`]）。
pub fn stage_links_with(ctx: &Ctx, run: &str, stage: &str, result: &Value, sc: Option<&SpecCtx>, pat: Option<&Re>) -> Vec<Value> {
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
                for ac in task_acs(&t, known, pat) {
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
                    for ac in task_acs(t, known, pat) {
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
                for ac in task_acs(&t, known, pat) {
                    if !acs.contains(&ac) {
                        acs.push(ac);
                    }
                }
            }
            for (id, bare, command, commit) in criteria_rows(result) {
                // design 段宣告過的 AC 才連；沒有 design 段時，逐條結果的 id 本身是規格 id 也算
                let mut hit: Vec<String> = acs.iter().filter(|ac| row_matches(ac, &id, &bare, pat)).cloned().collect();
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
                    for ac in task_acs(&t, known, pat) {
                        push(link("ac_commit", node("ac", &ac), node("commit", &sha), &name, stage, "merge.json:head_sha"));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// 決策節點的 id：`decisions.jsonl` 那一行內容（不含換行）的 sha1 前 12 碼。知識圖讀決策記憶時用同一個算法，兩邊才對得起來。
pub fn decision_id(line: &str) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(line.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// 決策 → 需求：`decisions.jsonl` 最後 `count` 筆（剛記下的）各連到這個 run 的需求。
/// 決策節點的 id 是那一行內容的 sha1 前 12 碼（`decisions.jsonl` 沒有自己的 id）。`stage` 是 `approve:<段>` 或 `decide`。
pub fn decision_links(ctx: &Ctx, run: &str, stage: &str, count: usize, sc: Option<&SpecCtx>) -> Vec<Value> {
    let name = run_name(ctx, run);
    let reqs = run_requirements(ctx, run, sc);
    let Ok(resolved) = ctx.resolve(run) else { return Vec::new() };
    let text = std::fs::read_to_string(pj(&pparent(&resolved), crate::pack::DECISIONS)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out = Vec::new();
    for l in lines.iter().skip(lines.len().saturating_sub(count)) {
        let id = decision_id(l);
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
    let name = safe(&pname(&resolved));
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

/// 驗證指令是不是空操作：`true`、`:`、`echo …`、`printf …`、`exit …`、`sleep …`、空字串（與它們用 `&&`、`;`、`|` 接起來的），
/// 或結尾是 `|| <空操作>`（不管前面過不過都算過）。只擋最明顯的寫法，不是「這個指令真的驗了東西」的保證。
pub fn noop_command(command: &str) -> bool {
    fn seg_noop(seg: &str) -> bool {
        let t = seg.trim().trim_start_matches(['(', '{']).trim();
        if t.is_empty() || t.starts_with('#') {
            return true;
        }
        let word = t.split_whitespace().next().unwrap_or_default();
        let base = word.rsplit('/').next().unwrap_or(word).trim_end_matches([')', '}']);
        matches!(base, "true" | ":" | "echo" | "printf" | "exit" | "sleep")
    }
    let c = command.trim();
    if !real(c) {
        return true;
    }
    if c.rsplit_once("||").is_some_and(|(_, tail)| seg_noop(tail)) {
        return true;
    }
    c.split([';', '\n', '|', '&']).all(seg_noop)
}

/// 40 碼十六進位（完整的 commit SHA）。
pub fn full_sha(v: &str) -> bool {
    v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 一段結果裡每個信封回報的實際改動檔（`payload.files_changed`）。
fn files_changed(result: &Value) -> Vec<String> {
    envelopes(result).into_iter().filter_map(|e| e.get("payload")).flat_map(|p| strs(p.get("files_changed"))).collect()
}

/// test 段結果裡每一份逐條結果（彙整的與各分片的）記的 commit。
fn verified_heads(test: &Value) -> BTreeSet<String> {
    envelopes(test).into_iter().filter_map(|e| e.get("payload")).filter(|p| p.get("criteria").is_some_and(Value::is_array)).map(|p| p.get("commit").and_then(Value::as_str).unwrap_or_default().trim().to_string()).collect()
}

/// `owns` 的一筆是不是 repo 裡真的有的檔或目錄。絕對路徑、有 `..`、有萬用字元的不算（不拿它去碰檔案系統）。
fn exists_in_repo(repo: Option<&str>, own: &str) -> bool {
    let o = own.trim().replace('\\', "/");
    let bad = o.is_empty() || o.starts_with('/') || o.contains('\0') || o.contains(['*', '?', ':']) || o.split('/').any(|c| c == "..");
    match repo {
        Some(r) if !bad => std::path::Path::new(r).join(&o).exists(),
        _ => false,
    }
}

/// [`link_gaps_of`] 要的資料。
pub struct GapInput<'a> {
    /// design 段的回傳。
    pub design: &'a Value,
    /// test 段的回傳；None＝這個 run 沒有 test 段的結果（recipe 沒有 test 段），只看 spec 小節與檔案。
    pub test: Option<&'a Value>,
    /// build（與 test 段修正）實際改動的檔；None＝recipe 沒有 build 段，這一項只看有沒有宣告。
    pub built: Option<&'a [String]>,
    /// 目標 repo（絕對路徑）：`owns` 的檔不在改動清單時，repo 裡有也算。
    pub repo: Option<&'a str>,
}

/// 連結檢查：design 段宣告的每條 AC 有沒有 spec 小節、檔案（design 起），測試、commit（test 起）。
/// 只讀各段存下來的結果，不讀帳本。回傳缺的環節（空＝齊）。design 段只看宣告（還沒有東西可以對）；test 起核對真假：
/// - 檔案：子任務 `owns` 的檔要在 build 實際改動的清單裡，或在 repo 裡存在。
/// - 測試：驗證指令不能是空操作（[`noop_command`]）。
/// - commit：要是 40 碼十六進位，而且 test 段每一份逐條結果（彙整與各分片）記的是同一個。
/// - 子任務沒有 `acceptance`：需求來源是 openspec 而變更有條目時算缺；整份 design 一條可辨識的驗收條件都沒有也算缺。
pub fn link_gaps_of(stage: &str, inp: &GapInput, sc: Option<&SpecCtx>) -> Vec<String> {
    link_gaps_of_with(stage, inp, sc, None)
}

/// 同 [`link_gaps_of`]；`pat` 是驗收條件編號的樣式（[`id_pattern`]）。
pub fn link_gaps_of_with(stage: &str, inp: &GapInput, sc: Option<&SpecCtx>, pat: Option<&Re>) -> Vec<String> {
    if !GAP_STAGES.contains(&stage) {
        return Vec::new();
    }
    let known = sc.map(SpecCtx::known);
    let known = known.as_ref();
    let spec_items = sc.map(|s| s.items().len()).unwrap_or(0);
    let subs = result_subtasks(inp.design);
    let later = stage != "design";
    let mut out: Vec<String> = Vec::new();
    let mut acs: Vec<(String, bool, bool)> = Vec::new();
    for t in &subs {
        let ids = task_acs(t, known, pat);
        let declared = strs(t.get("acceptance"));
        if declared.is_empty() && spec_items > 0 {
            out.push(format!("子任務 {} 沒有 acceptance，而變更的規格有 {spec_items} 條要驗：寫明它負責哪幾條（或可判定的完成條件）", py::take(&task_id(t), 80)));
        } else if ids.is_empty() && !declared.is_empty() {
            out.push(format!("子任務 {} 的驗收條件沒有可辨識的 id（{}或規格 id），連不到 spec、測試與 commit", task_id(t), id_name(pat)));
        }
        let owns = strs(t.get("owns"));
        let has_spec = !strs(t.get("spec_refs")).is_empty();
        let has_file = match inp.built {
            Some(built) if later => owns.iter().any(|o| built.iter().any(|b| crate::pack::path_overlap(o, b)) || exists_in_repo(inp.repo, o)),
            _ => !owns.is_empty(),
        };
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
    if !subs.is_empty() && acs.is_empty() && out.is_empty() {
        out.push(format!("design 的子任務沒有任何可辨識的驗收條件 id（{}或規格 id）：沒有東西可以連，不能當成連結齊全", id_name(pat)));
    }
    // 沒有 test 段的結果（recipe 沒有 test 段）時只看 spec 小節與檔案
    let with_test = later && inp.test.is_some();
    let rows = if with_test { inp.test.map(criteria_rows).unwrap_or_default() } else { Vec::new() };
    let heads = if with_test { inp.test.map(verified_heads).unwrap_or_default() } else { BTreeSet::new() };
    if heads.len() > 1 {
        let list: Vec<String> = heads.iter().map(|h| py::take(h, 40).to_string()).collect();
        out.push(format!("test 段各份逐條結果記的 commit 不一樣（{}）：不是同一個 HEAD，commit 連結不算數", list.join("、")));
    }
    for (ac, has_spec, has_file) in &acs {
        let mut lack: Vec<&str> = Vec::new();
        if !has_spec {
            lack.push("spec 小節");
        }
        if !has_file {
            lack.push(if later && inp.built.is_some() { "檔案（owns 的檔不在 build 的改動清單、repo 裡也沒有）" } else { "檔案" });
        }
        if with_test {
            let mine: Vec<&(String, String, String, String)> = rows.iter().filter(|(id, bare, _, _)| row_matches(ac, id, bare, pat)).collect();
            if !mine.iter().any(|(_, _, command, _)| !noop_command(command)) {
                lack.push(if mine.iter().any(|(_, _, command, _)| real(command)) { "測試（驗證指令是空操作）" } else { "測試" });
            }
            if !mine.iter().any(|(_, _, _, commit)| full_sha(commit)) || heads.len() > 1 {
                lack.push(if mine.iter().any(|(_, _, _, commit)| real(commit)) { "commit（要是 40 碼的完整 SHA，各份結果一致）" } else { "commit" });
            }
        }
        if !lack.is_empty() {
            out.push(format!("{ac}：缺 {}", lack.join("、")));
        }
    }
    out
}

/// [`link_gaps_of`]，資料取自 run 目錄的 `design.json`、`build.json`、`test.json`（政策核准時重算用）；`data` 是 run.json。
/// recipe 有 design／build／test 段而結果檔讀不到（或不是物件）時回問題：讀不到不等於沒有缺口。
/// 這個 run 沒有 design 段時沒有宣告可以檢查，回空的；沒有 build 段時「檔案」只看有沒有宣告。
///
/// 保證的：通過時，每條 AC 在 design 有對到 spec 小節、`owns` 至少有一個檔是 build 回報改過的或 repo 裡真的有、
/// test 段有一條不是空操作的驗證指令、test 段記的是同一個 40 碼的 commit。
/// 不保證的：那個檔的內容真的實作了這條 AC、那條指令真的驗了這條 AC（只擋 `true`、`echo` 這類最明顯的空操作）、
/// 那個 commit 存在或就是分支的 HEAD（合併前由 merge gate 的 `--reviewed-sha` 對 PR 的 head 把關）。
/// build 與 test 的結果是 agent 交的，這裡核對的是它們彼此一致，不是替代驗證。
pub fn link_gaps(ctx: &Ctx, run: &str, stage: &str, sc: Option<&SpecCtx>, data: &Value) -> Vec<String> {
    link_gaps_with(ctx, run, stage, sc, data, None)
}

/// 同 [`link_gaps`]；`pat` 是驗收條件編號的樣式（[`id_pattern`]）。
pub fn link_gaps_with(ctx: &Ctx, run: &str, stage: &str, sc: Option<&SpecCtx>, data: &Value, pat: Option<&Re>) -> Vec<String> {
    if !GAP_STAGES.contains(&stage) {
        return Vec::new();
    }
    let has = |st: &str| data.get("stages").and_then(|s| s.get(st)).is_some_and(|d| d.is_object() && d.get("status").and_then(Value::as_str) != Some("skipped"));
    let read = |name: &str| ctx.load_json(&pj(run, name)).ok().filter(Value::is_object);
    let unreadable = |st: &str, what: &str| format!("讀不到 {st}.json（這個 run 有 {st} 段）：{what}；讀不到不算沒有缺口");
    let Some(design) = read("design.json") else {
        return if has("design") { vec![unreadable("design", "不知道有哪些驗收條件")] } else { Vec::new() };
    };
    let later = stage != "design";
    let mut out: Vec<String> = Vec::new();
    let test = if later { read("test.json") } else { None };
    if later && test.is_none() && has("test") {
        out.push(unreadable("test", "測試與 commit 無法核對"));
    }
    let build = if later { read("build.json") } else { None };
    if later && build.is_none() && has("build") {
        out.push(unreadable("build", "檔案無法核對"));
    }
    // 實際改動的檔：build 回報的，加上 test 段修正時回報的。recipe 沒有 build 段時是 None（只看宣告）
    let built: Option<Vec<String>> = (later && has("build")).then(|| build.iter().chain(test.iter()).flat_map(files_changed).collect());
    let repo = data.get("target_repo").and_then(Value::as_str).filter(|r| !r.trim().is_empty()).map(|r| ctx.abs(&py::pp(r)));
    out.extend(link_gaps_of_with(stage, &GapInput { design: &design, test: test.as_ref(), built: built.as_deref(), repo: repo.as_deref() }, sc, pat));
    out
}
