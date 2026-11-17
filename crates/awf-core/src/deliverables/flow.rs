//! 受影響的文件、需求追溯矩陣、sync、configure、init、export 與命令列。

use super::core::*;
use super::py;
use super::*;
use crate::kb::pyish::{self, WS_CLASS};
use crate::pyfmt::{self, str_of, truthy};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::BTreeSet;

rx!(base_re, r"\A[\p{L}\p{N}_./@^~{}\-]+\z".to_string());
rx!(ws_run_re, format!(r"[{WS_CLASS}]+"));
rx!(pathish_re, r"(?:[A-Za-z]:)?(?:[\p{L}\p{N}_.~$\-]*[/\\])+([\p{L}\p{N}_.\-]+)".to_string());
rx!(h1_re, format!(r"\A#[{WS_CLASS}]+.*\n+"));
rx!(demote_re, format!(r"(?m)^(#{{1,3}})[{WS_CLASS}]"));
rx!(bullet_re, format!(r"\A[{WS_CLASS}]*(?:[-*•]|\p{{Nd}}+[.)、]|[（(]?[一二三四五六七八九十]+[)）、.])[{WS_CLASS}]*"));
rx!(bullet2_re, format!(r"\A[{WS_CLASS}]*(?:[-*•]|\p{{Nd}}+[.)、])[{WS_CLASS}]*"));
rx!(fname_re, r#"[\\/:*?"<>|]+"#.to_string());

// ------------------------------------------------------------------ 受影響的文件
/// 這次 run 的事實：改動檔、改動分類、modifiers、recipe、有哪些段、事件類型。
pub struct Facts {
    pub paths: Vec<String>,
    pub categories: BTreeSet<String>,
    pub modifiers: BTreeSet<String>,
    pub recipe: String,
    pub stages: BTreeSet<String>,
    pub event: String,
}

fn first_truthy<'a>(vals: &[&'a Value]) -> &'a Value {
    vals.iter().copied().find(|v| truthy(v)).unwrap_or(&NULL)
}

fn obj_or_null<'a>(v: &'a Value) -> &'a Value {
    if v.is_object() { v } else { &NULL }
}

pub fn run_facts(run: &str, paths: &[String]) -> Facts {
    let data = load_obj(&pyish::pjoin(run, "run.json"));
    let rec = obj_or_null(get(&data, "recipe"));
    let res = obj_or_null(get(rec, "resolved"));
    let mods = first_truthy(&[get(res, "modifiers"), get(rec, "modifiers")]);
    let stage_src = get(res, "stages");
    let stages: Vec<String> = if truthy(stage_src) { strs(stage_src) } else { get(&data, "stages").as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() };
    let plist: BTreeSet<String> = paths.iter().filter(|p| !pyish::is_blank(p)).map(|p| { let x = posix(p); dot_slash_re().replace(pyish::strip(&x), "").into_owned() }).collect();
    let mut cats = BTreeSet::new();
    for p in &plist {
        cats.extend(crate::recipe::classify(p).into_iter().map(str::to_string));
    }
    let name = first_truthy(&[get(res, "name"), get(rec, "name")]);
    let event = get(&data, "event");
    Facts {
        paths: plist.into_iter().collect(),
        categories: cats,
        modifiers: strs(mods).into_iter().collect(),
        recipe: if truthy(name) { str_of(name) } else { String::new() },
        stages: stages.into_iter().collect(),
        event: if truthy(event) { str_of(event) } else { String::new() },
    }
}

/// 一個觸發條件有沒有成立；成立回理由（空字串＝沒成立）。
fn token_hit(tok: &str, facts: &Facts) -> String {
    let tok = pyish::strip(tok);
    let when = |ok: bool, msg: String| if ok { msg } else { String::new() };
    if let Some(m) = tok.strip_prefix("mod:") {
        return when(facts.modifiers.contains(m), format!("這次有 {m}"));
    }
    if let Some(g) = tok.strip_prefix("path:") {
        return match_files(&facts.paths, &[json!(g)]).first().map(|h| format!("改到 {h}")).unwrap_or_default();
    }
    if let Some(s) = tok.strip_prefix("stage:") {
        return when(facts.stages.contains(s), format!("這次有 {s} 段"));
    }
    if let Some(r) = tok.strip_prefix("recipe:") {
        return when(r == facts.recipe, format!("需求類型是 {r}"));
    }
    if let Some(e) = tok.strip_prefix("event:") {
        return when(e == facts.event, format!("事件是 {e}"));
    }
    when(facts.categories.contains(tok), format!("改動裡有 {tok} 類的檔"))
}

fn why_affected(on: &[Value], facts: &Facts) -> String {
    on.iter().filter_map(Value::as_str).map(|t| token_hit(t, facts)).find(|w| !w.is_empty()).unwrap_or_default()
}

/// 這次 run 要更新哪些文件的哪些小節。回傳 {enabled, note, affected, deferred, hints, design_sections}。
/// 只看交付設定裡列的；沒列但對得上的放 hints（只提示，不自動加）。
pub fn affected(root: &str, run: &str, paths: &[String], milestone: bool, close: bool) -> Value {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else {
        return json!({"enabled": false, "note": note, "affected": [], "deferred": [], "hints": [], "design_sections": []});
    };
    let cat = load_catalog(root);
    let docs = resolve_docs(&cfg, &cat);
    let facts = run_facts(run, paths);
    let (mut aff, mut deferred, mut hints, mut design) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for d in &docs {
        let id = sget(d, "id");
        let title = get(d, "title");
        if truthy(get(d, "design_sections")) {
            design.push(json!({"id": id, "title": title, "sections": iter_any(get(d, "design_sections"))}));
        }
        if sget(d, "source") == "external" || id == MATRIX_ID || id == RECORD_ID {
            continue; // 外部文件不由流程產生；追溯矩陣與開發紀錄由工具寫（matrix、sync）
        }
        let why = why_affected(&iter_any(get(d, "on")), &facts);
        if why.is_empty() {
            continue;
        }
        let when = if truthy(get(d, "when")) { get(d, "when").clone() } else { json!("run") };
        if (when == "milestone" && !(milestone || close)) || (when == "close" && !close) {
            deferred.push(json!({"id": id, "title": title, "when": when, "why": why}));
            continue;
        }
        let sections = iter_any(get(d, "sections"));
        let mut secs: Vec<Value> = match get(d, "section_on").as_object().filter(|o| !o.is_empty()) {
            Some(sec_on) => sections.iter().filter(|s| s.as_str().and_then(|k| sec_on.get(k)).is_some_and(|on| !why_affected(&iter_any(on), &facts).is_empty())).cloned().collect(),
            None => Vec::new(),
        };
        if secs.is_empty() {
            secs = sections;
        }
        let depth = if truthy(get(d, "depth")) { get(d, "depth").clone() } else { json!("full") };
        aff.push(json!({"id": id, "title": title, "file": rel_to(&doc_path(&cfg, root, d), root), "depth": depth, "source": get(d, "source"), "why": why,
            "sections": secs, "sources": iter_any(get(d, "sources"))}));
    }
    for d in cat_docs(&cat) {
        let id = sget(d, "id");
        let sug = get(d, "suggest");
        if docs.iter().any(|x| sget(x, "id") == id) || sget(d, "source") == "external" || !sug.is_object() {
            continue;
        }
        let mut on: Vec<Value> = strs(get(sug, "modifiers")).iter().map(|m| json!(format!("mod:{m}"))).collect();
        on.extend(strs(get(sug, "paths")).iter().map(|g| json!(format!("path:{g}"))));
        let why = why_affected(&on, &facts);
        if !why.is_empty() {
            hints.push(json!({"id": id, "title": get(d, "title"), "why": why}));
        }
    }
    json!({"enabled": true, "note": note, "affected": aff, "deferred": deferred, "hints": hints, "design_sections": design})
}

/// base 到 HEAD 改了哪些檔；base 不合理或 git 失敗回空清單。
pub fn changed_paths(repo: &str, base: &str) -> Vec<String> {
    if base.is_empty() || base.starts_with('-') || !base_re().is_match(base) {
        return Vec::new(); // 不讓 base 變成 git 的選項（--output=… 之類）
    }
    for rng in [format!("{base}...HEAD"), base.to_string()] {
        let Ok(o) = std::process::Command::new("git").args(["-C", repo, "diff", "--name-only", "--end-of-options", &rng]).stdin(std::process::Stdio::null()).output() else {
            return Vec::new();
        };
        if o.status.success() {
            let text = git_text(&o.stdout);
            return py::splitlines(&text).into_iter().filter(|x| !pyish::is_blank(x)).map(posix).collect();
        }
    }
    Vec::new()
}

/// 給資料包工具用：這一段的 workflow 需要的交付資訊。沒有交付設定回空的（流程不變）。
pub fn pack_args(stage: &str, run: &str, root: &str, repo: Option<&str>, data: &Value, paths: Option<&[String]>) -> (Value, Vec<String>) {
    let (v, notes, _) = pack_args_with(stage, run, root, repo, data, paths, None);
    (v, notes)
}

/// run 的 args 沒有 base 時的比對基準：這個 run 的 recipe 有專屬的主線（profile 的 `conventions.branch.bases`）就用它，否則是守門設定的主線。
fn default_base(root: &str, run: &Value) -> String {
    let lay = crate::layout::for_project(root);
    match crate::layout::run_base_for_project(root, run) {
        Some(b) => format!("{}/{b}", lay.base_remote),
        None => lay.base_ref(),
    }
}

/// [`pack_args`]，有知識圖時 deliver 段多列「改到的檔在圖裡連到的交付文件小節」（只會多列，不會少）。
/// `kg` 是（圖, 目標 repo 的檔在圖裡的前綴）；第三個回傳值是要寫進資料包的那一節（沒有多列就是空的）。
pub fn pack_args_with(stage: &str, run: &str, root: &str, repo: Option<&str>, data: &Value, paths: Option<&[String]>, kg: Option<(&crate::kg::Graph, Option<&str>)>) -> (Value, Vec<String>, Vec<String>) {
    let (cfg, _) = load_config(root);
    let Some(cfg) = cfg.filter(|_| stage == "design" || stage == "deliver") else { return (json!({}), vec![], vec![]) };
    let mut owned: Vec<String> = paths.map(<[String]>::to_vec).unwrap_or_default();
    if let (None, "deliver", Some(repo)) = (paths, stage, repo) {
        let base = get(obj_or_null(get(data, "args")), "base");
        owned = changed_paths(repo, &if truthy(base) { str_of(base) } else { default_base(root, data) });
    }
    let res = affected(root, run, &owned, false, false);
    let list = |k: &str| pyfmt::items(res.get(k)).to_vec();
    if stage == "design" {
        let ds = list("design_sections");
        if ds.is_empty() {
            return (json!({}), vec![], vec![]);
        }
        let n = ds.len();
        return (json!({"design_sections": ds}), vec![format!("交付文件：design.md 要有 {n} 份文件需要的小節")], vec![]);
    }
    let mut notes = Vec::new();
    let mut out = Map::new();
    // 有交付設定就寫交付摘要（給客戶看的這次交付說明）
    out.insert("summary".into(), json!(posix(&pyish::pjoin(run, &format!("deliver/{SUMMARY_NAME}")))));
    // 交付摘要的節與讀者（設定的 summary_sections、audience）：有寫而且合規則才帶；沒帶時範本用內建的三節與「客戶」，資料包逐字不變
    if let Some(v) = summary_sections(&cfg) {
        out.insert("summary_sections".into(), v);
    }
    if let Some(a) = audience(&cfg) {
        out.insert("audience".into(), json!(a));
    }
    let mut aff = list("affected");
    let mut glines = Vec::new();
    if let Some((g, prefix)) = kg {
        let (lines, n) = super::kgflow::extend_affected(g, prefix, &cfg, root, &owned, &mut aff);
        glines = lines;
        if n > 0 {
            notes.push(format!("知識圖：依連結多列 {n} 個受影響的交付文件小節"));
        }
    }
    if !aff.is_empty() {
        out.insert("affected".into(), Value::Array(aff.clone()));
        let cat = load_catalog(root);
        for a in &aff {
            // 文件還沒建立就先放骨架，writer 才有小節與來源標記可以改
            ensure_doc(&cfg, &cat, root, sget(a, "id"));
        }
        let parts: Vec<String> = aff.iter().map(|a| format!("{}（{}）", fmt(get(a, "title")), strs(get(a, "sections")).join("、"))).collect();
        notes.push(format!("交付文件：這次要更新 {}", parts.join("、")));
    }
    let hints = list("hints");
    for h in &hints {
        notes.push(format!("交付文件：建議把「{}」加進交付設定（{}）；要加請跑 awf deliverables configure --add {} --write", fmt(get(h, "title")), fmt(get(h, "why")), fmt(get(h, "id"))));
    }
    for d in list("deferred") {
        notes.push(format!("交付文件：「{}」受影響，但設定成 {} 才更新，這次不動", fmt(get(&d, "title")), if sget(&d, "when") == "milestone" { "里程碑" } else { "結案" }));
    }
    if !hints.is_empty() {
        out.insert("hints".into(), Value::Array(hints));
    }
    (Value::Object(out), notes, glines)
}

/// 一行短字：前後沒有空白、沒有換行與反引號、1–`max` 個字（規則同 challenge 片段的 `awfLine`，字數以 UTF-16 單位計，和範本一致）。
fn short_line(s: &str, max: usize) -> bool {
    let n = s.encode_utf16().count();
    n >= 1 && n <= max && s.trim() == s && !s.contains(['\r', '\n', '`'])
}

/// 交付摘要要有哪些節（設定的 `summary_sections`）：1–8 個、每個 40 字以內、不重複才採用；沒寫或不合規則回 `None`（範本用內建的三節）。
pub fn summary_sections(cfg: &Value) -> Option<Value> {
    let a = get(cfg, "summary_sections").as_array()?;
    let names: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
    let uniq: BTreeSet<&str> = names.iter().copied().collect();
    (!a.is_empty() && a.len() <= 8 && names.len() == a.len() && uniq.len() == a.len() && names.iter().all(|s| short_line(s, 40))).then(|| json!(names))
}

/// 交付文件的讀者（設定的 `audience`）：1–20 字的一行才採用；沒寫或不合規則回 `None`（範本用「客戶」）。
pub fn audience(cfg: &Value) -> Option<String> {
    get(cfg, "audience").as_str().filter(|s| short_line(s, 20)).map(str::to_string)
}

// ------------------------------------------------------------------ 需求追溯矩陣
/// 從 test 段的結果裡找逐條驗收結果（信封、payload 或直接的物件都接受）。
fn criteria(obj: &Value) -> Option<&Value> {
    let o = obj.as_object()?;
    if get(obj, "criteria").is_array() && o.contains_key("commit") {
        return Some(obj);
    }
    ["verifier", "payload", "result", "combined"].iter().find_map(|k| criteria(get(obj, k)))
}

fn subtasks(obj: &Value) -> Vec<Value> {
    if !obj.is_object() {
        return Vec::new();
    }
    if let Some(a) = get(obj, "subtasks").as_array() {
        return a.clone();
    }
    ["payload", "result", "plan", "design", "spec"].iter().map(|k| subtasks(get(obj, k))).find(|g| !g.is_empty()).unwrap_or_default()
}

/// `str(v if v is not None else "")`
fn text_of(v: &Value) -> String {
    if v.is_null() { String::new() } else { str_of(v) }
}

fn cell_s(s: &str, limit: usize) -> String {
    let t = ws_run_re().replace_all(s, " ").replace('|', "｜");
    let t = pyish::strip(&t);
    if pyfmt::len(t) <= limit { t.to_string() } else { format!("{}…", pyish::take(t, limit.saturating_sub(1))) }
}

fn cell(v: &Value, limit: usize) -> String {
    cell_s(&text_of(v), limit)
}

/// 每條驗收條件一列：條件 ↔ spec 小節 ↔ 驗證指令 ↔ 結果與證據 ↔ commit。缺的環節列在 gaps。
pub fn matrix_rows(run: &str) -> (Vec<Value>, Vec<String>) {
    let test_doc = load_obj(&pyish::pjoin(run, "test.json"));
    let mut tasks = subtasks(&load_obj(&pyish::pjoin(run, "design.json")));
    if tasks.is_empty() {
        tasks = subtasks(&load_obj(&pyish::pjoin(run, "build.json")));
    }
    let Some(test) = criteria(&test_doc) else {
        return (vec![], vec!["這個 run 沒有驗證結果（test.json 裡找不到逐條驗收結果）".to_string()]);
    };
    let commit = if truthy(get(test, "commit")) { str_of(get(test, "commit")) } else { String::new() };
    let (mut rows, mut gaps) = (Vec::new(), Vec::new());
    for c in pyfmt::items(test.get("criteria")) {
        if !c.is_object() {
            continue;
        }
        let cid = if truthy(get(c, "id")) { str_of(get(c, "id")) } else { String::new() };
        // 分片驗證的 id 是 <分片>:<原 id>
        let bare = cid.split_once(':').map(|x| x.1).unwrap_or(&cid);
        let owns = |a: &Value| a.as_str().is_some_and(|a| !bare.is_empty() && (a == bare || [" ", "：", ":"].iter().any(|sep| a.starts_with(&format!("{bare}{sep}")))));
        let owners: Vec<&Value> = tasks.iter().filter(|t| t.is_object() && iter_any(get(t, "acceptance")).iter().any(owns)).collect();
        let collect = |key: &str| -> Vec<String> { owners.iter().flat_map(|t| strs(get(t, key))).collect::<BTreeSet<String>>().into_iter().collect() };
        let (refs, stories) = (collect("spec_refs"), collect("story_ids"));
        let (command, evidence, result) = (get(c, "command"), get(c, "evidence"), get(c, "result"));
        let mut miss: Vec<String> = Vec::new();
        for (name, ok) in [("spec 小節", !refs.is_empty()), ("驗證指令", truthy(command)), ("證據", truthy(evidence)), ("commit", !(commit.is_empty() || commit == "n/a"))] {
            if !ok {
                miss.push(name.to_string());
            }
        }
        if result != "pass" {
            miss.push(format!("結果是 {}", fmt(result)));
        }
        if !miss.is_empty() {
            let parts: Vec<String> = miss.iter().map(|m| if m.starts_with("結果") { m.clone() } else { format!("缺{m}") }).collect();
            gaps.push(format!("{}：{}", if cid.is_empty() { "（沒有 id）" } else { &cid }, parts.join("、")));
        }
        let task_ids: Vec<String> = owners.iter().map(|t| fmt(get(t, "id"))).collect();
        rows.push(json!({"id": cid, "criterion": get(c, "criterion"), "stories": stories, "spec": refs, "tasks": task_ids,
            "command": command, "result": result, "evidence": evidence, "commit": commit, "gaps": miss}));
    }
    (rows, gaps)
}

/// 證據與指令裡的路徑只留檔名：交付文件不需要 run 目錄、暫存目錄或本機路徑。
pub fn strip_paths(v: &Value) -> String {
    pathish_re().replace_all(&text_of(v), |m: &regex::Captures| m.get(1).map(|x| x.as_str().to_string()).unwrap_or_default()).into_owned()
}

/// 追溯矩陣的一段（標題與表格）與缺的環節。
pub fn matrix_block(run: &str, title: &str) -> (String, Vec<String>) {
    let (rows, gaps) = matrix_rows(run);
    let mut lines: Vec<String> = vec![format!("### {title}"), String::new(), "| 驗收條件 | 內容 | 需求／Story | spec 小節 | 驗證指令 | 結果 | 證據 | commit | 缺的環節 |".into(), "|---|---|---|---|---|---|---|---|---|".into()];
    let dash = |s: String, d: &str| if s.is_empty() { d.to_string() } else { s };
    for r in &rows {
        let command = if truthy(get(r, "command")) { format!("`{}`", cell_s(&strip_paths(get(r, "command")), 80).replace('`', "'")) } else { "—".to_string() };
        let result = match get(r, "result").as_str() {
            Some("pass") => "通過".to_string(),
            Some("fail") => "未通過".to_string(),
            Some("not_verifiable") => "無法驗證".to_string(),
            _ => cell(get(r, "result"), 20),
        };
        lines.push(format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            cell(get(r, "id"), 40), cell(get(r, "criterion"), 120), cell_s(&dash(strs(get(r, "stories")).join("、"), "—"), 60), cell_s(&dash(strs(get(r, "spec")).join("、"), "—"), 80),
            command, result, cell_s(&strip_paths(get(r, "evidence")), 120), cell_s(&dash(pyish::take(sget(r, "commit"), 12).to_string(), "—"), 12),
            cell_s(&dash(strs(get(r, "gaps")).join("、"), "無"), 80)));
    }
    if rows.is_empty() {
        lines.push(format!("| — | {} | — | — | — | — | — | — | — |", cell_s(gaps.first().map(String::as_str).unwrap_or("沒有資料"), 120)));
    }
    (format!("{}\n", lines.join("\n")), gaps)
}

/// run 目錄名稱的雜湊（文件裡不寫 run 目錄名稱）。
pub fn entry_key(run: &str) -> String {
    pyish::take(&sha1_text(&name_of(run)), 12).to_string()
}

/// 把一段內容放進文件的某個小節底下；同一個 key 已經有就整段換掉（重跑結果相同）。
pub fn upsert_entry(text: &str, key: &str, block: &str, heading: &str) -> Result<String, String> {
    let norm = text.replace("\r\n", "\n");
    let lines: Vec<&str> = norm.trim_end_matches('\n').split('\n').collect();
    let start = format!("<!-- awf:entry {key} -->");
    let end = format!("<!-- awf:entry-end {key} -->");
    let block_lines: Vec<&str> = block.trim_end_matches('\n').split('\n').collect();
    let mut body: Vec<String> = vec![start.clone()];
    body.extend(block_lines.iter().map(|s| s.to_string()));
    body.push(end.clone());
    let mut out: Vec<String> = Vec::new();
    let (mut i, mut placed) = (0, false);
    while let Some(ln) = lines.get(i) {
        if pyish::strip(ln) == start {
            let mut j = i;
            while lines.get(j).is_some_and(|l| pyish::strip(l) != end) {
                j += 1;
            }
            if j >= lines.len() {
                return Err(format!("文件裡有 {start} 卻沒有對應的結尾標記，不寫入（避免蓋掉後面的內容）"));
            }
            out.extend(body.iter().cloned());
            placed = true;
            i = j + 1;
            continue;
        }
        out.push(ln.to_string());
        i += 1;
    }
    if !placed {
        let want = format!("## {heading}");
        match out.iter().position(|ln| pyish::strip(ln) == want) {
            Some(h) => {
                let nxt = (h + 1..out.len()).find(|n| out.get(*n).is_some_and(|l| l.starts_with("## "))).unwrap_or(out.len());
                let mut seg: Vec<String> = out.get(h + 1..nxt).unwrap_or(&[]).iter().filter(|ln| pyish::strip(ln) != PLACEHOLDER).cloned().collect();
                while seg.last().is_some_and(|l| pyish::is_blank(l)) {
                    seg.pop();
                }
                let tail: Vec<String> = out.get(nxt..).unwrap_or(&[]).to_vec();
                out.truncate(h + 1);
                out.extend(seg);
                out.push(String::new());
                out.extend(body);
                out.push(String::new());
                out.extend(tail);
            }
            None => {
                out.extend([String::new(), want, String::new()]);
                out.extend(body);
            }
        }
    }
    Ok(format!("{}\n", out.join("\n").trim_end_matches('\n')))
}

fn epic_of(data: &Value) -> String {
    let v = first_truthy(&[get(obj_or_null(get(data, "args")), "epic"), get(data, "epic")]);
    if truthy(v) { pyish::strip(&str_of(v)).to_string() } else { String::new() }
}

fn title_of(data: &Value) -> String {
    let t = epic_of(data);
    if t.is_empty() { "這次交付".to_string() } else { t }
}

/// 設定裡的這份文件與它的路徑；文件還沒建立就先放骨架。設定裡沒有回 None。
pub fn ensure_doc(cfg: &Value, cat: &Value, root: &str, doc_id: &str) -> Option<(Value, String)> {
    let doc = resolve_docs(cfg, cat).into_iter().find(|d| sget(d, "id") == doc_id)?;
    let p = doc_path(cfg, root, &doc);
    if !is_file(&p) {
        write_lf(&p, &skeleton(&doc));
    }
    Some((doc, p))
}

fn first_section(doc: &Value, default: &str) -> String {
    strs(get(doc, "sections")).into_iter().next().unwrap_or_else(|| default.to_string())
}

/// 把這個 run 的追溯矩陣寫進需求追溯矩陣文件。
pub fn write_matrix(root: &str, run: &str) -> (i32, Vec<String>) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (0, vec![note]) };
    let cat = load_catalog(root);
    let Some((doc, p)) = ensure_doc(&cfg, &cat, root, MATRIX_ID) else {
        return (0, vec![format!("交付設定沒有列需求追溯矩陣（{MATRIX_ID}）；要加請跑 awf deliverables configure --add {MATRIX_ID} --write")]);
    };
    let data = load_obj(&pyish::pjoin(run, "run.json"));
    let (block, gaps) = matrix_block(run, &title_of(&data));
    let hits = privacy_hits(&block, &allow_list(&cfg), &[run_name_rule(root)]);
    if !hits.is_empty() {
        return (1, vec![format!("追溯矩陣的內容有隱私字串，沒有寫入：{}", hits.iter().take(5).cloned().collect::<Vec<_>>().join("；"))]);
    }
    let sec = first_section(&doc, "追溯矩陣");
    match upsert_entry(&read_text(&p).unwrap_or_default(), &entry_key(run), &block, &sec) {
        Ok(text) => write_lf(&p, &text),
        Err(e) => return (1, vec![format!("需求追溯矩陣沒有寫入：{e}")]),
    }
    let mut notes = vec![format!("需求追溯矩陣：{}", rel_to(&p, root))];
    notes.extend(gaps.iter().map(|g| format!("  [缺] {g}")));
    (0, notes)
}

// ------------------------------------------------------------------ sync
/// 功能開發紀錄的一段：交付摘要（標題降階）與核准紀錄。
pub fn record_block(data: &Value, summary: &str, show_approvers: bool) -> String {
    let mut lines: Vec<String> = vec![format!("### {}", title_of(data)), String::new()];
    let norm = summary.replace("\r\n", "\n");
    let body = h1_re().replace(pyish::strip(&norm), "");
    // 標題降階，不打亂文件結構
    let body = demote_re().replace_all(&body, |m: &regex::Captures| format!("{} ", "#".repeat((m.get(1).map(|x| x.as_str().len()).unwrap_or(0) + 3).min(6))));
    lines.push(if body.is_empty() { "（沒有交付摘要）".to_string() } else { body.into_owned() });
    lines.extend([String::new(), "#### 核准紀錄".into(), String::new(), "| 階段 | 核准者 | 時間 |".into(), "|---|---|---|".into()]);
    let name = |st: &str| -> String {
        match st {
            "plan" => "規劃", "design" => "設計", "build" => "實作", "sync" => "同步", "test" => "驗證", "deliver" => "交付", "merge" => "合併",
            other => return cell_s(other, 20),
        }.to_string()
    };
    let mut any_row = false;
    if let Some(stages) = get(data, "stages").as_object() {
        for (st, rec) in stages {
            let ap = get(rec, "approved");
            if !ap.is_object() {
                continue;
            }
            let who = if get(ap, "kind") == "policy" || get(ap, "by") == "policy" {
                "依政策自動核准".to_string()
            } else if show_approvers {
                cell(get(ap, "by"), 40)
            } else {
                "人工核准".to_string()
            };
            let at = if truthy(get(ap, "at")) { str_of(get(ap, "at")) } else { String::new() };
            lines.push(format!("| {} | {who} | {} |", name(st), cell_s(pyish::take(&at, 10), 12)));
            any_row = true;
        }
    }
    if !any_row {
        lines.push("| — | — | — |".into());
    }
    format!("{}\n", lines.join("\n"))
}

/// 更新來源紀錄。內文改過的小節記下現在的來源；來源改了而內文沒動的不記（仍然過期），
/// 除非人用 --reviewed <文件 id>#<小節> 表示「看過了，內容不用改」。回傳（記錄了哪些, 仍然過期的）。
/// only：只記這些；其他改過的小節列為「清單外改動」，不記（流程自動同步時用 workflow 回傳的受影響清單）。
pub fn stamp(root: &str, cfg: &Value, cat: &Value, repo: Option<&str>, reviewed: &[String], only: Option<&BTreeSet<String>>) -> (Vec<String>, Vec<String>) {
    let src_root = source_root(cfg, root, repo);
    let files = repo_files(&src_root);
    let mut state = load_state(root);
    let (mut done, mut stale) = (Vec::new(), Vec::new());
    // 有知識圖時，記錄小節連到的需求與符號的指紋（之後它們變了而內文沒動，這一節算過期）
    let links = super::kgflow::Links::open(root);
    for doc in resolve_docs(cfg, cat) {
        if sget(&doc, "source") == "external" {
            continue;
        }
        let id = sget(&doc, "id");
        let p = doc_path(cfg, root, &doc);
        let Some(text) = (if is_file(&p) { read_text(&p) } else { None }) else { continue };
        for s in doc_sections(&doc, &text).sections {
            let (st, why) = super::kgflow::linked_state(section_state(id, &s, &state, &files, &src_root), id, &s, &state, links.as_ref());
            let key = format!("{id}#{}", s.title);
            let ok = reviewed.contains(&key);
            if only.is_some_and(|o| st == "unstamped" && !o.contains(&key) && !ok) {
                stale.push(format!("{key}：清單外改動（這次沒有要更新這一節，內文卻改了；人看過後用 --reviewed 記錄）"));
                continue;
            }
            if st == "unstamped" || (st == "stale" && ok) {
                let mut rec = json!({"sources": section_sources(&s.sources, &files, &src_root), "body": body_hash(&s.body)});
                if let Some(l) = links.as_ref().and_then(|l| l.record(&rel_to(&p, root), &s.title)) {
                    rec["links"] = l;
                }
                if let Some(m) = state.as_object_mut() {
                    let slot = m.entry(id.to_string()).or_insert_with(|| json!({}));
                    if !slot.is_object() {
                        *slot = json!({});
                    }
                    if let Some(d) = slot.as_object_mut() {
                        d.insert(s.title.clone(), rec);
                    }
                }
                done.push(key);
            } else if st == "stale" {
                stale.push(format!("{key}：{why}"));
            }
        }
    }
    if !done.is_empty() {
        save_state(root, &state);
    }
    (done, stale)
}

/// 併交付摘要、寫追溯矩陣、更新來源紀錄、重寫總目錄。
pub fn sync(root: &str, run: &str, repo: Option<&str>, reviewed: &[String], only: Option<&BTreeSet<String>>) -> (i32, Vec<String>) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (0, vec![note]) };
    let cat = load_catalog(root);
    let run_json = pyish::pjoin(run, "run.json");
    let Some(data) = load_json(&run_json).filter(Value::is_object) else { return (1, vec![format!("讀不到 {}", posix(&run_json))]) };
    let mut notes: Vec<String> = Vec::new();
    let allow = allow_list(&cfg);
    let sp = pyish::pjoin(run, &format!("deliver/{SUMMARY_NAME}"));
    if let Some((doc, p)) = ensure_doc(&cfg, &cat, root, RECORD_ID) {
        let summary = if is_file(&sp) { read_text(&sp).unwrap_or_default() } else { String::new() };
        if summary.is_empty() {
            notes.push(format!("這個 run 沒有交付摘要（{}），功能開發紀錄只記核准紀錄", posix(&sp)));
        }
        let block = record_block(&data, &summary, get(&cfg, "show_approvers") == &Value::Bool(true));
        let hits = privacy_hits(&block, &allow, &[run_name_rule(root)]);
        if !hits.is_empty() {
            return (1, vec![format!("交付摘要有隱私字串，沒有寫入功能開發紀錄：{}", hits.iter().take(5).cloned().collect::<Vec<_>>().join("；"))]);
        }
        let sec = first_section(&doc, "開發紀錄");
        match upsert_entry(&read_text(&p).unwrap_or_default(), &entry_key(run), &block, &sec) {
            Ok(text) => write_lf(&p, &text),
            Err(e) => return (1, vec![format!("功能開發紀錄沒有寫入：{e}")]),
        }
        notes.push(format!("功能開發紀錄：{}", rel_to(&p, root)));
    }
    let docs = resolve_docs(&cfg, &cat);
    if docs.iter().any(|d| sget(d, "id") == MATRIX_ID) {
        let (code, mnotes) = write_matrix(root, run);
        notes.extend(mnotes);
        if code != 0 {
            return (code, notes);
        }
    }
    // 工具自己寫的兩份（開發紀錄、追溯矩陣）也算在這次要更新的範圍
    let only: Option<BTreeSet<String>> = only.map(|o| {
        let mut all = o.clone();
        for d in docs.iter().filter(|d| [RECORD_ID, MATRIX_ID].contains(&sget(d, "id"))) {
            all.extend(strs(get(d, "sections")).into_iter().map(|t| format!("{}#{t}", sget(d, "id"))));
        }
        all
    });
    let (done, stale) = stamp(root, &cfg, &cat, repo, reviewed, only.as_ref());
    if !done.is_empty() {
        notes.push(format!("記錄來源：{}{}", done.iter().take(12).cloned().collect::<Vec<_>>().join("、"), if done.len() > 12 { "…" } else { "" }));
    }
    notes.extend(stale.iter().map(|s| format!("  [過期] {s}")));
    write_lf(&pyish::pjoin(&out_dir(&cfg, root), INDEX_NAME), &index_text(&cfg, &cat));
    (0, notes)
}

// ------------------------------------------------------------------ configure
fn norm(v: &Value) -> String {
    let s = if truthy(v) { str_of(v) } else { String::new() };
    norm_s(&s)
}

fn norm_s(s: &str) -> String {
    let kept: String = s.chars().filter(|c| !(pyish::is_space(*c) || "-_／/、（）()[]【】「」:：.。·".contains(*c))).collect();
    kept.to_lowercase()
}

/// 合約或客戶清單的一行 → 型錄的文件 id；對不到回 None。
pub fn match_title(line: &str, cat: &Value) -> Option<String> {
    let key = norm_s(&bullet_re().replace(line, ""));
    if key.is_empty() {
        return None;
    }
    let klen = pyfmt::len(&key);
    let mut best: Option<(usize, String)> = None;
    for d in cat_docs(cat) {
        let id = fmt(get(d, "id"));
        let mut names = vec![norm(get(d, "title"))];
        names.extend(iter_any(get(d, "aliases")).iter().map(norm));
        for n in names {
            if n.is_empty() {
                continue;
            }
            if n == key {
                return Some(id);
            }
            let nlen = pyfmt::len(&n);
            if nlen >= 2 && (key.contains(&n) || (klen >= 3 && n.contains(&key))) && best.as_ref().map_or(true, |b| nlen > b.0) {
                best = Some((nlen, id.clone()));
            }
        }
    }
    best.map(|b| b.1)
}

fn runs_dir(root: &str) -> String {
    let guard = load_json(&pyish::pjoin(root, ".claude/teams/guard.config.json")).unwrap_or(Value::Null);
    let rd = safe_rel(get(&guard, "runs_dir"));
    pyish::pjoin(root, if rd.is_empty() { "_tpm/runs" } else { &rd })
}

fn past_modifiers(root: &str) -> BTreeSet<String> {
    let runs = runs_dir(root);
    let mut names: Vec<String> = std::fs::read_dir(&runs)
        .map(|d| d.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect())
        .unwrap_or_default();
    names.retain(|n| std::path::Path::new(&format!("{runs}/{n}/run.json")).exists());
    names.sort();
    let skip = names.len().saturating_sub(200);
    let mut mods = BTreeSet::new();
    for n in names.iter().skip(skip) {
        let data = load_obj(&format!("{runs}/{n}/run.json"));
        let rec = get(&data, "recipe");
        if !rec.is_object() {
            continue;
        }
        let res = obj_or_null(get(rec, "resolved"));
        mods.extend(strs(first_truthy(&[get(res, "modifiers"), get(rec, "modifiers")])));
    }
    mods
}

/// 依 repo 實況建議要交的文件。回傳（建議, 不建議），每筆都有理由。
pub fn suggest(root: &str, cat: &Value, repo: Option<&str>) -> (Vec<Value>, Vec<Value>) {
    let files = repo_files(repo.unwrap_or(root));
    let mods = past_modifiers(root);
    let (mut yes, mut no) = (Vec::new(), Vec::new());
    for d in cat_docs(cat) {
        let sug = get(d, "suggest");
        if !sug.is_object() {
            continue;
        }
        let paths = iter_any(get(sug, "paths"));
        let hit = match_files(&files, &paths);
        let wanted: BTreeSet<String> = strs(get(sug, "modifiers")).into_iter().collect();
        let used: Vec<String> = wanted.intersection(&mods).cloned().collect();
        let reason_or = |default: &str| if truthy(get(sug, "reason")) { fmt(get(sug, "reason")) } else { default.to_string() };
        let (list, reason) = if truthy(get(sug, "always")) {
            (&mut yes, reason_or(""))
        } else if let Some(h) = hit.first() {
            (&mut yes, format!("{}（例如 {h}）", reason_or("repo 有相關的檔")))
        } else if !used.is_empty() {
            (&mut yes, format!("過去的 run 用過 {}", used.join("、")))
        } else {
            let mut shown: Vec<String> = paths.iter().take(2).map(fmt).collect();
            if paths.is_empty() {
                shown.push("—".to_string());
            }
            (&mut no, format!("repo 裡找不到相關的檔（{}）", shown.join("、")))
        };
        list.push(json!({"id": get(d, "id"), "title": get(d, "title"), "reason": reason}));
    }
    (yes, no)
}

fn opt(argv: &[String], name: &str) -> String {
    match argv.iter().position(|a| a == name) {
        Some(i) if i + 1 < argv.len() => argv.get(i + 1).cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

fn opts(argv: &[String], name: &str) -> Vec<String> {
    argv.iter().enumerate().filter(|(i, a)| *a == name && i + 1 < argv.len()).filter_map(|(i, _)| argv.get(i + 1).cloned()).collect()
}

fn has(argv: &[String], name: &str) -> bool {
    argv.iter().any(|a| a == name)
}

fn csv(s: &str) -> Vec<String> {
    s.split(',').map(pyish::strip).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

fn set_default(m: &mut Map<String, Value>, k: &str, v: Value) {
    if !m.contains_key(k) {
        m.insert(k.to_string(), v);
    }
}

/// 交付設定的提案（或寫入）。回傳（exit code, 結果, 說明）。
pub fn configure(root: &str, argv: &[String]) -> (i32, Value, Vec<String>) {
    let cat = load_catalog(root);
    let idx = catalog_index(&cat);
    let cfg_path = pyish::pjoin(root, &format!(".claude/teams/{CONFIG_NAME}"));
    let cur = load_obj(&cfg_path);
    let mut chosen: Map<String, Value> = Map::new();
    for d in iter_any(get(&cur, "documents")) {
        if let (true, Some(id)) = (d.is_object(), get(&d, "id").as_str()) {
            chosen.insert(id.to_string(), d.clone());
        }
    }
    let mut reasons: Map<String, Value> = Map::new();
    let mut notes: Vec<String> = Vec::new();
    let mut out = Map::new();
    out.insert("unmatched".into(), json!([]));
    out.insert("suggested".into(), json!([]));
    out.insert("not_suggested".into(), json!([]));
    let presets = pyfmt::items(cat.get("presets"));
    let preset = opt(argv, "--preset");
    if !preset.is_empty() {
        let Some(p) = presets.iter().find(|x| get(x, "id") == preset.as_str()) else {
            let names: Vec<String> = presets.iter().map(|x| fmt(get(x, "id"))).collect();
            return (2, Value::Object(out), vec![format!("沒有這個情境組合：{preset}（有：{}）", names.join("、"))]);
        };
        for i in strs(get(p, "documents")) {
            set_default(&mut chosen, &i, json!({"id": i}));
            set_default(&mut reasons, &i, json!(format!("情境組合「{}」", fmt(get(p, "title")))));
        }
    }
    let repo_raw = opt(argv, "--repo");
    let repo: Option<String> = if repo_raw.is_empty() { None } else { Some(pyish::pjoin(root, &repo_raw)) };
    if has(argv, "--suggest") {
        let (yes, no) = suggest(root, &cat, repo.as_deref());
        for s in &yes {
            let id = fmt(get(s, "id"));
            set_default(&mut chosen, &id, json!({"id": id}));
            set_default(&mut reasons, &id, get(s, "reason").clone());
        }
        out.insert("suggested".into(), Value::Array(yes));
        out.insert("not_suggested".into(), Value::Array(no));
    }
    let imp = opt(argv, "--import");
    if !imp.is_empty() {
        let imp_path = pyish::ppath(&imp);
        let raw = match pyish::read_text(&imp_path) {
            Ok(t) => t,
            Err(e) => return (2, Value::Object(out), vec![format!("清單檔讀不到（{}）", py::os_err(&e, &imp_path))]),
        };
        let mut unmatched: Vec<Value> = Vec::new();
        for ln in py::splitlines(&raw) {
            let st = pyish::strip(ln);
            if st.is_empty() || st.starts_with('#') {
                continue;
            }
            if let Some(got) = match_title(ln, &cat) {
                set_default(&mut chosen, &got, json!({"id": got}));
                set_default(&mut reasons, &got, json!(format!("清單裡的「{}」", pyish::take(st, 40))));
            } else {
                unmatched.push(json!(st));
                if has(argv, "--custom-unmatched") {
                    let cid = format!("custom-{}", chosen.keys().filter(|k| k.starts_with("custom-")).count() + 1);
                    let title = bullet2_re().replace(st, "");
                    chosen.insert(cid.clone(), json!({"id": cid, "title": pyish::take(&title, 80), "source": "assisted", "sections": ["內容"]}));
                    reasons.insert(cid, json!("清單裡對不到型錄的項目，新增成自訂文件"));
                }
            }
        }
        out.insert("unmatched".into(), Value::Array(unmatched));
    }
    for i in csv(&opt(argv, "--add")) {
        if !idx.contains_key(&i) && !chosen.contains_key(&i) {
            notes.push(format!("型錄沒有 {i}；自訂文件請直接在 {CONFIG_NAME} 裡寫 title、source、sections"));
            continue;
        }
        set_default(&mut chosen, &i, json!({"id": i}));
        set_default(&mut reasons, &i, json!("指定加入"));
    }
    for i in csv(&opt(argv, "--remove")) {
        chosen.shift_remove(&i);
    }
    let order = |id: &str| cat_docs(&cat).iter().rposition(|d| sget(d, "id") == id).unwrap_or(1_000_000);
    let mut docs: Vec<Value> = chosen.values().cloned().collect();
    docs.sort_by(|a, b| { let (x, y) = (sget(a, "id"), sget(b, "id")); order(x).cmp(&order(y)).then_with(|| x.cmp(y)) });
    let mut new = Map::new();
    if let Some(c) = cur.as_object() {
        for (k, v) in c {
            if k != "documents" && k != "confirmed_by" {
                new.insert(k.clone(), v.clone());
            }
        }
    }
    let dir = opt(argv, "--dir");
    if !dir.is_empty() {
        new.insert("dir".into(), json!(dir));
    }
    set_default(&mut new, "dir", json!(DEFAULT_DIR));
    new.insert("documents".into(), Value::Array(docs.clone()));
    let errs = config_errors(&Value::Object(new.clone()));
    if !errs.is_empty() {
        return (2, Value::Object(out), errs);
    }
    let pick = |d: &Value, key: &str| -> Value {
        if truthy(get(d, key)) { get(d, key).clone() } else { idx.get(sget(d, "id")).map(|c| get(c, key).clone()).unwrap_or(Value::Null) }
    };
    let shown: Vec<Value> = docs.iter().map(|d| json!({"id": get(d, "id"), "title": pick(d, "title"), "source": pick(d, "source"),
        "reason": reasons.get(sget(d, "id")).cloned().unwrap_or_else(|| json!("已在交付設定裡"))})).collect();
    out.insert("config".into(), Value::Object(new.clone()));
    out.insert("documents".into(), Value::Array(shown));
    if has(argv, "--write") {
        let by = opt(argv, "--by");
        if by.is_empty() {
            return (2, Value::Object(out), vec!["--write 要帶 --by <誰確認的>：交付設定要有人確認才寫入".to_string()]);
        }
        new.insert("confirmed_by".into(), json!(by));
        write_lf(&cfg_path, &format!("{}\n", py::dumps(&Value::Object(new.clone()), Some(1))));
        out.insert("config".into(), Value::Object(new));
        out.insert("written".into(), json!(true));
        notes.push(format!("已寫入 .claude/teams/{CONFIG_NAME}（{} 份文件）。接著跑 awf deliverables init 建骨架。", docs.len()));
    } else {
        out.insert("written".into(), json!(false));
        notes.push("這是提案，還沒寫入。確認後加 --write --by <誰> 重跑同一個指令。".to_string());
    }
    (0, Value::Object(out), notes)
}

// ------------------------------------------------------------------ init／export
/// 依交付設定建目錄與文件骨架（已有的檔不動），重寫總目錄。
pub fn init_docs(root: &str) -> (i32, Vec<String>) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (0, vec![note]) };
    let cat = load_catalog(root);
    let d_out = out_dir(&cfg, root);
    let mut made: Vec<String> = Vec::new();
    for d in resolve_docs(&cfg, &cat) {
        if sget(&d, "source") == "external" {
            let ext = pyish::pjoin(&d_out, &format!("external/{}", sget(&d, "id")));
            if !is_dir(&ext) {
                write_lf(&format!("{ext}/.gitkeep"), "");
                made.push(format!("{}/", rel_to(&ext, root)));
            }
            continue;
        }
        let p = doc_path(&cfg, root, &d);
        if !is_file(&p) {
            write_lf(&p, &skeleton(&d));
            made.push(rel_to(&p, root));
        }
    }
    let idx = pyish::pjoin(&d_out, INDEX_NAME);
    write_lf(&idx, &index_text(&cfg, &cat));
    let first = if made.is_empty() {
        "文件都已經在，沒有新增".to_string()
    } else {
        format!("建立 {} 個：{}{}", made.len(), made.iter().take(20).cloned().collect::<Vec<_>>().join("、"), if made.len() > 20 { "…" } else { "" })
    };
    (0, vec![first, format!("總目錄：{}", rel_to(&idx, root))])
}

thread_local! {
    static PANDOC: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
}

/// 測試用：指定 pandoc 的位置（`Some(None)`＝當成沒有裝；`None`＝恢復成在 PATH 裡找）。
pub fn set_pandoc_override(v: Option<Option<String>>) {
    PANDOC.with(|p| *p.borrow_mut() = v);
}

/// `shutil.which(name)`
fn which(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path.split(sep) {
        let dir = if dir.is_empty() { "." } else { dir };
        let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
        for ext in exts {
            let cand = format!("{}/{name}{ext}", dir.trim_end_matches('/'));
            let Ok(m) = std::fs::metadata(&cand) else { continue };
            #[cfg(unix)]
            let exec = { use std::os::unix::fs::PermissionsExt; m.permissions().mode() & 0o111 != 0 };
            #[cfg(not(unix))]
            let exec = true;
            if m.is_file() && exec {
                return Some(cand);
            }
        }
    }
    None
}

/// 有 pandoc 就把每份文件轉成 docx；`pandoc`：`None`＝在 PATH 裡找，`Some(None)`＝當成沒有。
pub fn export(root: &str, out: &str, pandoc: Option<Option<String>>) -> (i32, Vec<String>) {
    let (cfg, note) = load_config(root);
    let Some(cfg) = cfg else { return (0, vec![note]) };
    let exe = match pandoc.or_else(|| PANDOC.with(|p| p.borrow().clone())) {
        Some(v) => v,
        None => which("pandoc"),
    };
    let Some(exe) = exe.filter(|e| !e.is_empty()) else {
        return (0, vec!["找不到 pandoc，沒有轉檔。安裝方式：macOS `brew install pandoc`、Windows `winget install JohnMacFarlane.Pandoc`、\
Linux 用套件管理工具安裝 pandoc（https://pandoc.org/installing.html）。Markdown 檔本身可以直接交付。".to_string()]);
    };
    let cat = load_catalog(root);
    let dest = if out.is_empty() { pyish::pjoin(&out_dir(&cfg, root), "_export") } else { pyish::ppath(out) };
    let _ = std::fs::create_dir_all(&dest);
    let mut notes = Vec::new();
    for d in resolve_docs(&cfg, &cat) {
        let id = sget(&d, "id");
        let p = doc_path(&cfg, root, &d);
        if sget(&d, "source") == "external" || !is_file(&p) {
            continue;
        }
        let title = if truthy(get(&d, "title")) { fmt(get(&d, "title")) } else { id.to_string() };
        let target = pyish::pjoin(&dest, &format!("{}.docx", fname_re().replace_all(&title, "-")));
        match std::process::Command::new(&exe).args([p.as_str(), "-f", "markdown", "-o", target.as_str()]).stdin(std::process::Stdio::null()).output() {
            Ok(r) if r.status.success() => notes.push(format!("{id} → {}", posix(&target))),
            Ok(r) => notes.push(format!("{id} 轉檔失敗：{}", pyish::take(pyish::strip(&git_text(&r.stderr)), 200))),
            Err(e) => notes.push(format!("{id} 轉檔失敗：{}", py::os_err(&e, &exe))),
        }
    }
    if notes.is_empty() {
        notes.push("沒有可以轉的文件".to_string());
    }
    (0, notes)
}

// ------------------------------------------------------------------ CLI
/// `--repo`（沒給時用 run.json 的 target_repo）；相對路徑接在專案根目錄後面。
pub fn repo_of(root: &str, argv: &[String], run: Option<&str>) -> Option<String> {
    let mut raw = opt(argv, "--repo");
    if raw.is_empty() {
        if let Some(run) = run {
            let data = load_obj(&pyish::pjoin(run, "run.json"));
            let t = get(&data, "target_repo");
            raw = if truthy(t) { str_of(t) } else { String::new() };
        }
    }
    if raw.is_empty() { None } else { Some(pyish::pjoin(root, &raw)) }
}

fn status_text(k: &str) -> &'static str {
    STATUS_TEXT.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or("")
}

/// CLI 的本體：`root` 是專案根目錄，stdout 的文字寫進 `out`。回傳 exit code。
pub fn run(root: &str, argv: &[String], out: &mut String) -> i32 {
    let say = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    let Some(cmd) = argv.first().map(String::as_str) else {
        say(out, DOC);
        return 0;
    };
    if cmd == "-h" || cmd == "--help" {
        say(out, DOC);
        return 0;
    }
    if cmd == "--selftest" {
        return super::selftest::selftest(out);
    }
    let root = pyish::ppath(root);
    let root = root.as_str();
    let rest = argv.get(1..).unwrap_or(&[]);
    let as_json = has(rest, "--json");
    let run_raw = opt(rest, "--run");
    let run_dir: Option<String> = if run_raw.is_empty() { None } else { Some(pyish::ppath(&run_raw)) };
    let run_dir = run_dir.as_deref();
    let need_run = || run_dir.unwrap_or("");
    if cmd == "matrix" && has(rest, "--project") {
        // 專案層：每個現行需求目前由哪些檔案、測試、commit 支撐（從知識圖；只讀、印到標準輸出）
        return match super::kgflow::project_matrix(root) {
            Ok((text, js)) => {
                say(out, if as_json { py::dumps(&js, Some(1)) } else { text.trim_end_matches('\n').to_string() }.as_str());
                0
            }
            Err(why) => {
                say(out, &why);
                1
            }
        };
    }
    if ["affected", "matrix", "sync"].contains(&cmd) && run_dir.is_none() {
        say(out, "缺 --run <run 目錄>");
        return 2;
    }
    let (code, notes): (i32, Vec<String>) = match cmd {
        "configure" => {
            let (code, res, notes) = configure(root, rest);
            if as_json {
                let mut m = res.as_object().cloned().unwrap_or_default();
                m.insert("notes".into(), json!(notes));
                say(out, &py::dumps(&Value::Object(m), Some(1)));
                return code;
            }
            for d in pyfmt::items(res.get("documents")) {
                say(out, &format!("  {:<24} {}（{}）— {}", fmt(get(d, "id")), fmt(get(d, "title")), fmt(get(d, "source")), fmt(get(d, "reason"))));
            }
            for s in pyfmt::items(res.get("not_suggested")) {
                say(out, &format!("  [不建議] {}：{}", fmt(get(s, "title")), fmt(get(s, "reason"))));
            }
            for u in pyfmt::items(res.get("unmatched")) {
                say(out, &format!("  [對不到型錄] {}", fmt(u)));
            }
            (code, notes)
        }
        "init" => init_docs(root),
        "index" => match load_config(root) {
            (None, note) => (0, vec![note]),
            (Some(cfg), _) => {
                let idx = pyish::pjoin(&out_dir(&cfg, root), INDEX_NAME);
                write_lf(&idx, &index_text(&cfg, &load_catalog(root)));
                (0, vec![format!("總目錄：{}", rel_to(&idx, root))])
            }
        },
        "status" => {
            let (res, note) = status_all(root, repo_of(root, rest, run_dir).as_deref());
            if as_json {
                say(out, &py::dumps(&json!({"enabled": res.is_some(), "note": note, "documents": res.unwrap_or_default()}), Some(1)));
                return 0;
            }
            let Some(res) = res else {
                say(out, &note);
                return 0;
            };
            for d in &res {
                let detail = sget(d, "detail");
                say(out, &format!("  [{}] {}（{}）{}", status_text(sget(d, "status")), fmt(get(d, "title")), sget(d, "id"), if detail.is_empty() { String::new() } else { format!("：{detail}") }));
            }
            let counts: Vec<String> = STATUS_TEXT.iter().map(|(k, t)| format!("{t} {}", res.iter().filter(|d| sget(d, "status") == *k).count())).collect();
            say(out, &format!("共 {} 份：{}", res.len(), counts.join("、")));
            return 0;
        }
        "check" => {
            let (errs, note) = check_all(root, repo_of(root, rest, run_dir).as_deref());
            let Some(errs) = errs else {
                say(out, &note);
                return 0;
            };
            for e in &errs {
                say(out, &format!("  [問題] {e}"));
            }
            say(out, &format!("交付文件檢查：{}", if errs.is_empty() { "通過".to_string() } else { format!("{} 個問題", errs.len()) }));
            return if errs.is_empty() { 0 } else { 1 };
        }
        "affected" => {
            let pf = opt(rest, "--paths-file");
            let paths: Vec<String> = if !pf.is_empty() {
                let pf = pyish::ppath(&pf);
                match pyish::read_text(&pf) {
                    Ok(t) => py::splitlines(&t).into_iter().filter(|x| !pyish::is_blank(x)).map(str::to_string).collect(),
                    Err(e) => {
                        say(out, &format!("改動清單讀不到（{}）", py::os_err(&e, &pf)));
                        return 2;
                    }
                }
            } else {
                match repo_of(root, rest, run_dir) {
                    Some(repo) => {
                        let data = load_obj(&pyish::pjoin(need_run(), "run.json"));
                        let base = get(obj_or_null(get(&data, "args")), "base");
                        let mut b = opt(rest, "--base");
                        if b.is_empty() {
                            b = if truthy(base) { str_of(base) } else { default_base(root, &data) };
                        }
                        changed_paths(&repo, &b)
                    }
                    None => Vec::new(),
                }
            };
            let res = affected(root, need_run(), &paths, has(rest, "--milestone"), has(rest, "--close"));
            if as_json {
                say(out, &py::dumps(&res, Some(1)));
                return 0;
            }
            if get(&res, "enabled") != &Value::Bool(true) {
                say(out, sget(&res, "note"));
                return 0;
            }
            let list = |k: &str| pyfmt::items(res.get(k));
            for a in list("affected") {
                say(out, &format!("  [更新] {}：{}（{}）", fmt(get(a, "title")), strs(get(a, "sections")).join("、"), fmt(get(a, "why"))));
            }
            for d in list("deferred") {
                say(out, &format!("  [這次不動] {}（{} 才更新）", fmt(get(d, "title")), fmt(get(d, "when"))));
            }
            for h in list("hints") {
                say(out, &format!("  [建議加入] {}（{}）", fmt(get(h, "title")), fmt(get(h, "why"))));
            }
            if list("affected").is_empty() && list("deferred").is_empty() && list("hints").is_empty() {
                say(out, "這次沒有受影響的交付文件");
            }
            return 0;
        }
        "matrix" => {
            if has(rest, "--stdout") {
                let data = load_obj(&pyish::pjoin(need_run(), "run.json"));
                let (block, gaps) = matrix_block(need_run(), &title_of(&data));
                say(out, &block);
                for g in gaps {
                    say(out, &format!("[缺] {g}"));
                }
                return 0;
            }
            write_matrix(root, need_run())
        }
        "sync" => sync(root, need_run(), repo_of(root, rest, run_dir).as_deref(), &opts(rest, "--reviewed"), None),
        "export" => export(root, &opt(rest, "--out"), None),
        other => {
            say(out, &format!("不認得的指令：{other}"));
            return 2;
        }
    };
    for n in notes {
        say(out, &n);
    }
    code
}
