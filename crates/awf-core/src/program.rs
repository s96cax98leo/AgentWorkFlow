//! 多需求排程（`awf program`）：`<runs_dir>/program.json` 列出一批 epic、彼此的相依、各自對應的 run 或變更，
//! 這支只回答兩件事——每個 epic 現在的狀態與被誰擋住（`status`）、現在可以開工的是哪些與建議的下一個指令（`next`）。
//!
//! 只排順序與提示：**唯讀**。不派工、不核准、不碰閘門、不寫 run.json 也不寫 program.json（由人維護）。
//! 狀態只從各 run 的 run.json 讀；相依有循環、指到不存在的 epic、id 重複都是錯誤（exit 1），不猜。
//! 格式見 `plugin/scaffold/.claude/teams/schemas/program.schema.json`；語意規則在 [`semantic_errors`]。

use crate::kb::pyish;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

pub const FILE: &str = "program.json";
pub const SCHEMA_VERSION: u64 = 1;
const STAGES: [&str; 7] = ["plan", "design", "build", "sync", "test", "deliver", "merge"];

pub const USAGE: &str = "多需求排程：讀 <runs_dir>/program.json（一批 epic、相依、對應的 run 或變更），只排順序與提示。

用法：
    awf program status [--json]     每個 epic 的狀態（從對應 run 的 run.json 讀）、被誰擋住
    awf program next [--json]       現在可以開工或接著做的 epic，與建議的下一個指令
    awf program --selftest

共同選項：
    --root <目錄>    專案根目錄（預設 CLAUDE_PROJECT_DIR，再沒有就從目前目錄往上找最外層的 .claude/teams/VERSION，都沒有才用目前目錄）
    --file <檔>      program.json 的位置（預設 <runs_dir>/program.json）

program.json 由人維護，例如：
    {\"schema_version\": 1, \"epics\": [
      {\"id\": \"auth-sso\", \"title\": \"單一登入\", \"run\": \"2026-11-01-auth-sso\"},
      {\"id\": \"billing-export\", \"title\": \"匯出帳單\", \"change\": \"add-billing-export\", \"depends_on\": [\"auth-sso\"]}]}

狀態：not_started（還沒有 run）、in_progress、done（已結案）、abandoned（已取消）、unknown（run.json 讀不懂）。
相依的 epic 要 done（已結案）才算滿足。這支是唯讀的：不派工、不核准、不寫任何檔。
相依有循環、指到不存在的 epic、id 重複時報錯（exit 1）。
";

fn id_ok(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphanumeric()) && s.chars().count() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// run 目錄名：單純的名字（可以有中文），不能有路徑分隔、空白、控制字元，不以點開頭。
fn run_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 120 && !s.starts_with('.') && !s.chars().any(|c| matches!(c, '/' | '\\') || c.is_control() || c.is_whitespace())
}

fn clip(s: &str, n: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.trim();
    if flat.chars().count() <= n { flat.to_string() } else { format!("{}…", flat.chars().take(n).collect::<String>()) }
}

/// 形狀檢查（和 program.schema.json 同一組規則，執行時不依賴 schema 檔）。
pub fn shape_errors(doc: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(o) = doc.as_object() else { return vec!["program.json 要是一個物件".to_string()] };
    for k in o.keys() {
        if !["schema_version", "name", "epics"].contains(&k.as_str()) {
            errs.push(format!("不認得的欄位：{}", clip(k, 40)));
        }
    }
    if let Some(v) = o.get("schema_version") {
        if v.as_u64() != Some(SCHEMA_VERSION) {
            errs.push(format!("schema_version 要是 {SCHEMA_VERSION}"));
        }
    }
    if o.get("name").is_some_and(|n| !n.is_string()) {
        errs.push("name 要是字串".to_string());
    }
    let Some(epics) = o.get("epics").and_then(Value::as_array) else {
        errs.push("缺 epics（陣列）".to_string());
        return errs;
    };
    if epics.is_empty() {
        errs.push("epics 至少要有一個".to_string());
    }
    for (i, e) in epics.iter().enumerate() {
        let at = format!("epics[{i}]");
        let Some(e) = e.as_object() else {
            errs.push(format!("{at} 要是物件"));
            continue;
        };
        for k in e.keys() {
            if !["id", "title", "run", "change", "depends_on", "note"].contains(&k.as_str()) {
                errs.push(format!("{at} 有不認得的欄位：{}", clip(k, 40)));
            }
        }
        match e.get("id").and_then(Value::as_str) {
            Some(id) if id_ok(id) => {}
            _ => errs.push(format!("{at}.id 必填，只能用英數字、點、底線、連字號（開頭是英數字，最多 80 字）")),
        }
        for k in ["title", "change", "note"] {
            if e.get(k).is_some_and(|v| !v.as_str().is_some_and(|s| !s.trim().is_empty())) {
                errs.push(format!("{at}.{k} 要是非空字串"));
            }
        }
        if e.get("run").is_some_and(|v| !v.as_str().is_some_and(run_ok)) {
            errs.push(format!("{at}.run 要是 run 目錄的名字（在 runs_dir 底下；不含路徑分隔、空白，不以點開頭）"));
        }
        match e.get("depends_on") {
            None => {}
            Some(Value::Array(a)) if a.iter().all(|d| d.as_str().is_some_and(id_ok)) => {}
            Some(_) => errs.push(format!("{at}.depends_on 要是 epic id 的陣列")),
        }
    }
    errs
}

fn ids_of(doc: &Value) -> Vec<&str> {
    doc.get("epics").and_then(Value::as_array).into_iter().flatten().filter_map(|e| e.get("id").and_then(Value::as_str)).collect()
}

fn deps_of(e: &Value) -> Vec<&str> {
    e.get("depends_on").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect()
}

/// schema 表達不了的規則：id 不重複、run 不重複、相依要指到存在的 epic、不能依賴自己、不能有循環。
pub fn semantic_errors(doc: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    let ids = ids_of(doc);
    let mut seen = BTreeSet::new();
    for id in &ids {
        if !seen.insert(*id) {
            errs.push(format!("epic id 重複：{id}"));
        }
    }
    let epics: Vec<&Value> = doc.get("epics").and_then(Value::as_array).into_iter().flatten().collect();
    let mut runs: BTreeMap<&str, &str> = BTreeMap::new();
    for e in &epics {
        let (Some(id), Some(run)) = (e.get("id").and_then(Value::as_str), e.get("run").and_then(Value::as_str)) else { continue };
        if let Some(other) = runs.insert(run, id) {
            if other != id {
                errs.push(format!("{other} 與 {id} 指到同一個 run：{}", clip(run, 80)));
            }
        }
    }
    let mut graph: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in &epics {
        let Some(id) = e.get("id").and_then(Value::as_str) else { continue };
        let mut mine = Vec::new();
        let mut dup = BTreeSet::new();
        for d in deps_of(e) {
            if d == id {
                errs.push(format!("{id} 不能依賴自己"));
            } else if !seen.contains(d) {
                errs.push(format!("{id} 依賴的 {d} 不在 epics 裡"));
            } else if !dup.insert(d) {
                errs.push(format!("{id} 的 depends_on 重複列了 {d}"));
            } else {
                mine.push(d);
            }
        }
        graph.entry(id).or_insert(mine);
    }
    // 找循環：反覆拿掉沒有未完成相依的節點，剩下的就在循環裡（或依賴循環裡的節點）
    let mut left: BTreeMap<&str, Vec<&str>> = graph.clone();
    loop {
        let free: Vec<&str> = left.iter().filter(|(_, d)| d.iter().all(|x| !left.contains_key(x))).map(|(k, _)| *k).collect();
        if free.is_empty() {
            break;
        }
        for k in free {
            left.remove(k);
        }
    }
    if !left.is_empty() {
        // 只報真的在環上的（從它出發走得回自己）
        let on_cycle: Vec<&str> = left
            .keys()
            .filter(|start| {
                let mut stack: Vec<&str> = left.get(**start).cloned().unwrap_or_default();
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                while let Some(n) = stack.pop() {
                    if n == **start {
                        return true;
                    }
                    if seen.insert(n) {
                        stack.extend(left.get(n).cloned().unwrap_or_default());
                    }
                }
                false
            })
            .copied()
            .collect();
        errs.push(format!("相依有循環：{}", on_cycle.join("、")));
    }
    errs
}

/// 一個 epic 現在的狀態。
#[derive(Debug, Clone, PartialEq)]
pub struct Epic {
    pub id: String,
    pub title: String,
    pub run: String,
    pub change: String,
    pub depends_on: Vec<String>,
    /// not_started、in_progress、done、abandoned、unknown。
    pub status: &'static str,
    /// 一句話的細節（等哪一段核准、下一段是什麼）。
    pub detail: String,
    /// 還沒 done 的相依。
    pub blocked_by: Vec<String>,
    /// 建議的下一個指令（被擋住、已完成、已取消時是空的）。
    pub next: String,
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::pyfmt::truthy)
}

/// 從 run.json 判斷狀態與細節。
pub fn run_status(data: &Value) -> (&'static str, String) {
    let Some(o) = data.as_object() else { return ("unknown", "run.json 不是物件".to_string()) };
    if truthy(o.get("abandoned")) {
        return ("abandoned", "已取消".to_string());
    }
    if truthy(o.get("closed")) {
        return ("done", "已結案".to_string());
    }
    let Some(stages) = o.get("stages").and_then(Value::as_object) else { return ("unknown", "run.json 沒有 stages".to_string()) };
    let status = |s: &str| stages.get(s).and_then(|x| x.get("status")).and_then(Value::as_str).unwrap_or_default();
    let waiting = STAGES.iter().find(|s| status(s) == "done" && truthy(stages.get(**s).and_then(|x| x.get("gate"))) && !truthy(stages.get(**s).and_then(|x| x.get("approved"))));
    let detail = if let Some(s) = waiting {
        format!("等 {s} 段核准")
    } else if let Some(s) = STAGES.iter().find(|s| matches!(status(s), "blocked" | "invalid" | "needs_decision")) {
        format!("{s} 段停下了，等人處理")
    } else if let Some(s) = STAGES.iter().find(|s| stages.contains_key(**s) && !matches!(status(s), "done" | "skipped")) {
        format!("下一段 {s}")
    } else {
        "每一段都完成了，還沒結案（awf save-run close）".to_string()
    };
    ("in_progress", detail)
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace(['"', '`', '$', '\\', '\n', '\r'], " "))
}

/// 算每個 epic 的狀態。`runs`＝runs_dir 的絕對路徑，`runs_rel`＝指令裡要寫的寫法。先過 [`shape_errors`]、[`semantic_errors`] 再呼叫。
pub fn evaluate(doc: &Value, runs: &str, runs_rel: &str) -> Vec<Epic> {
    let mut out: Vec<Epic> = Vec::new();
    for e in doc.get("epics").and_then(Value::as_array).into_iter().flatten() {
        let text = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let (id, run) = (text("id"), text("run"));
        let (status, detail) = if run.is_empty() {
            ("not_started", "還沒有對應的 run".to_string())
        } else {
            let file = pyish::pjoin(runs, &format!("{run}/run.json"));
            match std::fs::read(&file) {
                Err(_) => ("not_started", "run 還沒建立".to_string()),
                Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
                    Ok(data) => run_status(&data),
                    Err(_) => ("unknown", "run.json 讀不懂".to_string()),
                },
            }
        };
        out.push(Epic { id, title: text("title"), run, change: text("change"), depends_on: deps_of(e).into_iter().map(str::to_string).collect(), status, detail, blocked_by: Vec::new(), next: String::new() });
    }
    let done: BTreeSet<String> = out.iter().filter(|e| e.status == "done").map(|e| e.id.clone()).collect();
    for e in &mut out {
        e.blocked_by = e.depends_on.iter().filter(|d| !done.contains(*d)).cloned().collect();
        if !e.blocked_by.is_empty() {
            continue;
        }
        let run_arg = format!("{runs_rel}/{}", if e.run.is_empty() { "<run 目錄名>" } else { &e.run });
        e.next = match e.status {
            "not_started" => {
                let req = if e.title.is_empty() { "<需求原文或路徑>".to_string() } else { quote(&clip(&e.title, 200)) };
                let change = if e.change.is_empty() { String::new() } else { format!(" --set change={}", e.change.replace(|c: char| c.is_whitespace() || matches!(c, '"' | '`' | '$' | '\\' | ';' | '&' | '|'), "")) };
                format!("awf start --run {run_arg} --event sdlc --epic {} --requirement {req}{change}", e.id)
            }
            "in_progress" => format!("awf next --run {run_arg}"),
            _ => String::new(),
        };
    }
    out
}

fn label(status: &str) -> &'static str {
    match status {
        "not_started" => "還沒開始",
        "in_progress" => "進行中",
        "done" => "已完成",
        "abandoned" => "已取消",
        _ => "狀態不明",
    }
}

fn epic_json(e: &Epic) -> Value {
    json!({"id": e.id, "title": e.title, "run": e.run, "change": e.change, "depends_on": e.depends_on, "status": e.status, "detail": e.detail, "blocked_by": e.blocked_by, "next": e.next})
}

fn why_blocked(e: &Epic, all: &[Epic]) -> String {
    e.blocked_by
        .iter()
        .map(|d| match all.iter().find(|x| &x.id == d) {
            Some(x) => format!("{d}（{}）", label(x.status)),
            None => d.clone(),
        })
        .collect::<Vec<_>>()
        .join("、")
}

fn head(e: &Epic) -> String {
    if e.title.is_empty() { e.id.clone() } else { format!("{}　{}", e.id, clip(&e.title, 60)) }
}

pub fn status_text(epics: &[Epic]) -> String {
    let mut out = String::new();
    let count = |s: &str| epics.iter().filter(|e| e.status == s).count();
    out.push_str(&format!("共 {} 個 epic：已完成 {}、進行中 {}、還沒開始 {}、已取消 {}\n", epics.len(), count("done"), count("in_progress"), count("not_started"), count("abandoned")));
    for e in epics {
        out.push_str(&format!("- {}：{}（{}）", head(e), label(e.status), e.detail));
        if !e.run.is_empty() {
            out.push_str(&format!("　run：{}", e.run));
        }
        if !e.change.is_empty() {
            out.push_str(&format!("　變更：{}", clip(&e.change, 60)));
        }
        out.push('\n');
        if !e.blocked_by.is_empty() && !matches!(e.status, "done" | "abandoned") {
            out.push_str(&format!("    被擋住：{}\n", why_blocked(e, epics)));
        }
    }
    out
}

pub fn next_text(epics: &[Epic]) -> String {
    let mut out = String::new();
    let mut section = |title: &str, rows: Vec<String>| {
        if !rows.is_empty() {
            out.push_str(&format!("{title}\n"));
            for r in rows {
                out.push_str(&r);
            }
        }
    };
    let open = |e: &&Epic| !matches!(e.status, "done" | "abandoned");
    section(
        "可以開工：",
        epics.iter().filter(|e| e.status == "not_started" && e.blocked_by.is_empty()).map(|e| {
            let hint = if e.run.is_empty() { "\n    （program.json 還沒寫這個 epic 的 run：開工後把 run 目錄名補上，之後才看得到狀態）" } else { "" };
            format!("- {}\n    {}{hint}\n", head(e), e.next)
        }).collect(),
    );
    section("接著做：", epics.iter().filter(|e| e.status == "in_progress" && e.blocked_by.is_empty()).map(|e| format!("- {}（{}）\n    {}\n", head(e), e.detail, e.next)).collect());
    section(
        "被擋住（相依的 epic 要先結案）：",
        epics.iter().filter(open).filter(|e| !e.blocked_by.is_empty()).map(|e| {
            let early = if e.status == "in_progress" { "；注意：它已經開工了，相依還沒完成" } else { "" };
            format!("- {}：等 {}{early}\n", head(e), why_blocked(e, epics))
        }).collect(),
    );
    section("要人看：", epics.iter().filter(|e| e.status == "unknown").map(|e| format!("- {}：{}\n", head(e), e.detail)).collect());
    if out.is_empty() {
        out.push_str("沒有可以開工或接著做的 epic（全部完成或取消了）。\n");
    }
    out
}

fn opt<'a>(argv: &'a [String], name: &str) -> &'a str {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).map(String::as_str).unwrap_or("")
}

/// 跑一次：（exit code, stdout, stderr）。`default_root`＝沒給 `--root` 時的專案根目錄。
pub fn run_captured(argv: &[String], default_root: &str) -> (i32, String, String) {
    let sub = argv.first().map(String::as_str).unwrap_or("");
    if matches!(sub, "" | "-h" | "--help") {
        return (if sub.is_empty() { 2 } else { 0 }, USAGE.to_string(), String::new());
    }
    if !matches!(sub, "status" | "next") {
        return (2, String::new(), format!("awf program：沒有「{}」這個子指令\n\n{USAGE}", clip(sub, 40)));
    }
    let known = ["--json", "--root", "--file"];
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        if !known.contains(&a) {
            return (2, String::new(), format!("awf program：不認得的參數「{}」\n", clip(a, 40)));
        }
        i += if a == "--json" { 1 } else { 2 };
    }
    let root = if opt(argv, "--root").is_empty() { default_root.to_string() } else { opt(argv, "--root").replace('\\', "/") };
    let runs_rel = crate::kb::guard_dir(&root, "runs_dir", "_tpm/runs");
    if runs_rel.is_empty() {
        return (1, String::new(), "awf program：guard.config.json 的 runs_dir 不能用（要是專案底下的相對路徑）\n".to_string());
    }
    let runs = pyish::pjoin(&root, &runs_rel);
    let file = if opt(argv, "--file").is_empty() { pyish::pjoin(&runs, FILE) } else { opt(argv, "--file").replace('\\', "/") };
    let shown = if opt(argv, "--file").is_empty() { format!("{runs_rel}/{FILE}") } else { clip(&file, 200) };
    let raw = match std::fs::read(&file) {
        Ok(r) => r,
        Err(_) => return (1, String::new(), format!("awf program：找不到 {shown}。這個檔由人維護：列出 epic、相依與對應的 run（格式見 awf program --help）\n")),
    };
    let doc: Value = match serde_json::from_slice(&raw) {
        Ok(d) => d,
        Err(e) => return (1, String::new(), format!("awf program：{shown} 不是合法的 JSON（{}）\n", clip(&e.to_string(), 120))),
    };
    let mut errs = shape_errors(&doc);
    if errs.is_empty() {
        errs = semantic_errors(&doc);
    }
    if !errs.is_empty() {
        let mut err = format!("awf program：{shown} 有 {} 個問題，沒有排程：\n", errs.len());
        for e in errs.iter().take(30) {
            err.push_str(&format!("  - {e}\n"));
        }
        return (1, String::new(), err);
    }
    let epics = evaluate(&doc, &runs, &runs_rel);
    let json_out = argv.iter().any(|a| a == "--json");
    let out = match (sub, json_out) {
        ("status", true) => format!("{}\n", serde_json::to_string_pretty(&json!({"schema_version": SCHEMA_VERSION, "epics": epics.iter().map(epic_json).collect::<Vec<_>>()})).unwrap_or_default()),
        ("status", false) => status_text(&epics),
        (_, true) => {
            let ready: Vec<Value> = epics.iter().filter(|e| !e.next.is_empty()).map(epic_json).collect();
            let blocked: Vec<Value> = epics.iter().filter(|e| !e.blocked_by.is_empty() && !matches!(e.status, "done" | "abandoned")).map(epic_json).collect();
            format!("{}\n", serde_json::to_string_pretty(&json!({"schema_version": SCHEMA_VERSION, "ready": ready, "blocked": blocked})).unwrap_or_default())
        }
        (_, false) => next_text(&epics),
    };
    (0, out, String::new())
}

pub fn main(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("--selftest") {
        let (code, out) = selftest();
        print!("{out}");
        return code;
    }
    let (code, out, err) = run_captured(args, &crate::deliverables::default_root());
    let _ = std::io::stdout().write_all(out.as_bytes());
    let _ = std::io::stderr().write_all(err.as_bytes());
    code
}

/// 目錄底下所有檔的（路徑, 內容）：確認唯讀用。
fn snapshot(dir: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_string()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path().to_string_lossy().replace('\\', "/");
            if e.path().is_dir() {
                stack.push(p);
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap_or_default()));
            }
        }
    }
    out.sort();
    out
}

/// `awf program --selftest`：回（exit code, 輸出）。在自己的暫存目錄跑。
pub fn selftest() -> (i32, String) {
    let mut ok = true;
    let mut n = 0;
    let mut out = String::from("awf program selftest（暫存目錄，不碰 repo）\n");
    let mut check = |name: &str, cond: bool, detail: &str| {
        n += 1;
        ok = ok && cond;
        let tail = if cond { String::new() } else { format!("：{}", detail.chars().take(600).collect::<String>()) };
        out.push_str(&format!("  [{}] {name}{tail}\n", if cond { "通過" } else { "失敗" }));
    };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("awf-program.{}{nanos}", std::process::id())).to_string_lossy().replace('\\', "/");
    let runs = format!("{root}/_tpm/runs");
    let put = |rel: &str, v: &Value| {
        let p = format!("{root}/{rel}");
        if let Some(parent) = std::path::Path::new(&p).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, v.to_string());
    };
    let call = |a: &[&str]| run_captured(&a.iter().map(|x| x.to_string()).collect::<Vec<_>>(), &root);
    let _ = std::fs::create_dir_all(&runs);

    let (c, o, e) = call(&["status"]);
    check("沒有 program.json：說明怎麼建、exit 1", c == 1 && o.is_empty() && e.contains("找不到 _tpm/runs/program.json"), &e);

    let done_run = json!({"event": "sdlc", "closed": {"by": "h", "at": "t"}, "stages": {"plan": {"status": "done", "gate": true, "approved": {"by": "h"}}}});
    let gate_run = json!({"event": "sdlc", "stages": {"plan": {"status": "done", "gate": true}, "design": {"status": "pending", "gate": true}}});
    let mid_run = json!({"event": "sdlc", "stages": {"plan": {"status": "done", "gate": true, "approved": {"by": "h"}}, "design": {"status": "pending", "gate": true}}});
    let full_run = json!({"event": "sdlc", "stages": {"plan": {"status": "done", "gate": true, "approved": {"by": "h"}}, "merge": {"status": "done"}}});
    let dead_run = json!({"event": "sdlc", "abandoned": {"by": "h", "at": "t", "reason": "不做了"}, "stages": {}});
    put("_tpm/runs/r-auth/run.json", &done_run);
    put("_tpm/runs/r-billing/run.json", &gate_run);
    put("_tpm/runs/r-report/run.json", &mid_run);
    put("_tpm/runs/r-merged/run.json", &full_run);
    put("_tpm/runs/r-dead/run.json", &dead_run);
    let _ = std::fs::create_dir_all(format!("{runs}/r-broken"));
    let _ = std::fs::write(format!("{runs}/r-broken/run.json"), "{壞掉");
    let program = json!({"schema_version": 1, "name": "示範客戶 ACME 第一期", "epics": [
        {"id": "auth", "title": "單一登入", "run": "r-auth"},
        {"id": "billing", "title": "匯出帳單", "run": "r-billing", "depends_on": ["auth"]},
        {"id": "report", "title": "月報表", "run": "r-report", "depends_on": ["billing"]},
        {"id": "notify", "title": "到期\"通知\"", "change": "add-notify", "depends_on": ["auth"]},
        {"id": "audit", "title": "稽核紀錄", "run": "r-later", "depends_on": ["billing", "auth"]},
        {"id": "merged", "run": "r-merged"},
        {"id": "after-merged", "depends_on": ["merged"]},
        {"id": "dead", "run": "r-dead"},
        {"id": "after-dead", "depends_on": ["dead"]},
        {"id": "broken", "run": "r-broken"}
    ]});
    put("_tpm/runs/program.json", &program);
    let before = snapshot(&root);
    let (c, o, e) = call(&["status", "--json"]);
    let j: Value = serde_json::from_str(&o).unwrap_or(Value::Null);
    let of = |id: &str| j["epics"].as_array().and_then(|a| a.iter().find(|x| x["id"] == id)).cloned().unwrap_or(Value::Null);
    check(
        "status：狀態從 run.json 讀（已結案＝done、等核准與做到一半＝in_progress、沒有 run＝not_started、已取消、讀不懂）",
        c == 0 && e.is_empty() && j["schema_version"] == 1 && of("auth")["status"] == "done" && of("billing")["status"] == "in_progress" && of("billing")["detail"] == "等 plan 段核准"
            && of("report")["detail"] == "下一段 design" && of("notify")["status"] == "not_started" && of("audit")["detail"] == "run 還沒建立" && of("dead")["status"] == "abandoned" && of("broken")["status"] == "unknown",
        &o,
    );
    check(
        "status：被誰擋住＝還沒結案的相依；每一段做完但沒結案、已取消的相依都還算擋住",
        of("billing")["blocked_by"] == json!([]) && of("report")["blocked_by"] == json!(["billing"]) && of("audit")["blocked_by"] == json!(["billing"]) && of("merged")["status"] == "in_progress"
            && of("after-merged")["blocked_by"] == json!(["merged"]) && of("after-dead")["blocked_by"] == json!(["dead"]),
        &o,
    );
    let (c, o, _) = call(&["status"]);
    check(
        "status（文字）：一行一個 epic，被擋住的列出是誰與它的狀態",
        c == 0 && o.contains("共 10 個 epic：已完成 1、進行中 3、還沒開始 4、已取消 1") && o.contains("- report　月報表：進行中（下一段 design）") && o.contains("    被擋住：billing（進行中）") && o.contains("    被擋住：dead（已取消）"),
        &o,
    );
    let (c, o, _) = call(&["next"]);
    check(
        "next：可以開工的給 awf start（帶 epic、需求、變更名），進行中的給 awf next，被擋住的說明在等誰",
        c == 0 && o.contains("可以開工：\n- notify") && o.contains("awf start --run _tpm/runs/<run 目錄名> --event sdlc --epic notify --requirement \"到期 通知 \" --set change=add-notify")
            && o.contains("接著做：\n- billing　匯出帳單（等 plan 段核准）\n    awf next --run _tpm/runs/r-billing") && o.contains("- audit　稽核紀錄：等 billing（進行中）")
            && o.contains("- report　月報表：等 billing（進行中）；注意：它已經開工了，相依還沒完成") && o.contains("要人看：\n- broken：run.json 讀不懂") && !o.contains("- auth"),
        &o,
    );
    let (c, o, _) = call(&["next", "--json"]);
    let j: Value = serde_json::from_str(&o).unwrap_or(Value::Null);
    let ids = |k: &str| -> Vec<String> { j[k].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str().map(str::to_string)).collect() };
    check("next --json：ready 與 blocked 兩組", c == 0 && ids("ready") == ["billing", "notify", "merged"] && ids("blocked") == ["report", "audit", "after-merged", "after-dead"], &o);
    check("唯讀：status 與 next 都不寫任何檔（run.json、program.json 原封不動，沒有多出檔案）", snapshot(&root) == before, "");

    let bad = |doc: Value| {
        put("_tpm/runs/program.json", &doc);
        let (c, o, e) = call(&["next"]);
        (c, o, e)
    };
    let (c, o, e) = bad(json!({"epics": [{"id": "a", "depends_on": ["b"]}, {"id": "b", "depends_on": ["c"]}, {"id": "c", "depends_on": ["a"]}, {"id": "d", "depends_on": ["a"]}]}));
    check("相依有循環：報錯、不排程（只列在環上的）", c == 1 && o.is_empty() && e.contains("相依有循環：a、b、c") && !e.contains("、d"), &e);
    let (c, _, e) = bad(json!({"epics": [{"id": "a", "depends_on": ["zzz"]}]}));
    check("相依指到不存在的 epic：報錯", c == 1 && e.contains("a 依賴的 zzz 不在 epics 裡"), &e);
    let (c, _, e) = bad(json!({"epics": [{"id": "a"}, {"id": "a"}, {"id": "b", "depends_on": ["b"]}, {"id": "c", "run": "r1"}, {"id": "d", "run": "r1"}]}));
    check("id 重複、依賴自己、兩個 epic 指到同一個 run：報錯", c == 1 && e.contains("epic id 重複：a") && e.contains("b 不能依賴自己") && e.contains("c 與 d 指到同一個 run"), &e);
    let (c, _, e) = bad(json!({"epics": [{"id": "../x"}, {"id": "ok", "run": "../../etc"}, {"id": "ok2", "extra": 1}], "more": true}));
    check(
        "形狀不對（id 含路徑、run 指到 runs_dir 外、不認得的欄位）：報錯",
        c == 1 && e.contains("epics[0].id") && e.contains("epics[1].run") && e.contains("epics[2] 有不認得的欄位") && e.contains("不認得的欄位：more"),
        &e,
    );
    let _ = std::fs::write(format!("{runs}/program.json"), "{壞掉");
    let (c, _, e) = call(&["status"]);
    check("program.json 不是 JSON：報錯", c == 1 && e.contains("不是合法的 JSON"), &e);
    let (c, _, e) = call(&["write"]);
    let (c2, _, e2) = call(&["status", "--write"]);
    check("沒有會寫入的子指令或選項（不認得的一律拒絕）", c == 2 && e.contains("沒有「write」這個子指令") && c2 == 2 && e2.contains("不認得的參數"), &format!("{e}{e2}"));
    // schema 檔與執行時的檢查是同一組規則：golden-set 的正反例兩邊都要對（在模版裡跑時才有 schema 檔）
    if let Some(teams) = crate::template::find_str().map(|t| crate::template::teams_str(&t)).filter(|t| std::path::Path::new(&format!("{t}/schemas/program.schema.json")).is_file()) {
        let schemas = crate::schema::Schemas::new(teams);
        let good = schemas.check_doc(&program, "program").unwrap_or_else(|e| vec![e]);
        let cyc = schemas.check_doc(&json!({"epics": [{"id": "a", "depends_on": ["b"]}, {"id": "b", "depends_on": ["a"]}]}), "program").unwrap_or_default();
        let shape = schemas.check_doc(&json!({"epics": [{"id": "../x"}]}), "program").unwrap_or_default();
        check("schema：program.schema.json 收正例、擋形狀不對的；語意規則（循環）由 check_doc 一起擋", good.is_empty() && cyc.iter().any(|m| m.contains("循環")) && !shape.is_empty(), &format!("{good:?}{cyc:?}{shape:?}"));
    }
    let _ = std::fs::remove_dir_all(&root);
    out.push_str(&format!("awf program selftest：{}（{n} 項）\n", if ok { "全部通過" } else { "有失敗" }));
    (i32::from(!ok), out)
}
