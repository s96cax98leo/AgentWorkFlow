//! 規格層：讀 OpenSpec 目錄（現行規格與變更）、穩定 id、validate、apply。1.0 的新功能，沒有 Python 版。
//!
//! 自己實作，不相依 OpenSpec 的套件；只讀它的目錄與 Markdown 格式。說明在 `docs/規格層.md`。
//!
//! - `read`：目錄 → 資料模型（`Tree`、`Capability`、`Requirement`、`Scenario`、`Change`、`Delta`）。
//! - `ids`：穩定 id 與 `ids.json`。
//! - `check`：validate。
//! - `apply`：把差異併回現行規格並歸檔（先算好、再一次寫入，失敗還原）。
//! - `diff`：unified diff。

pub mod apply;
pub mod check;
pub mod diff;
pub mod ids;
pub mod read;

pub use apply::{commit, merge_text, plan, FileWrite, Plan};
pub use check::{cross_check, format_problem, validate};
pub use ids::{assign, load_registry, registry_text, slugify, write_registry, IdOptions, IdReport, Pending, RegEntry, Registry};
pub use read::{load, Capability, Change, ChangeState, Delta, DeltaKind, Found, Problem, Rename, Requirement, Scenario, Span, TaskStats, Tree};

use read::clean;
use regex::Regex;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "規格層：讀 OpenSpec 目錄的現行規格與變更，給需求與情境穩定的 id，檢查差異，把完成的變更併回現行規格。

用法：
    awf spec status   [--root <目錄>] [--json]
    awf spec validate [--change <名>] [--root <目錄>] [--json]
    awf spec show <id> [--root <目錄>]
    awf spec ids      [--write] [--adopt <舊id>=<新標題>]… [--retire <id>]… [--root <目錄>] [--json]
    awf spec apply --change <名> [--write] [--date <YYYY-MM-DD>] [--root <目錄>]
    awf spec --selftest

共同選項：
    --root <目錄>         規格根目錄，預設 openspec（相對於目前目錄；可以給別的 repo 的絕對路徑）
    --id-pattern <正規式>  專案自己的需求編號樣式；標題符合時用它當代稱（有第 1 組用第 1 組）

apply 與 ids 預設只印出會有什麼變動，加 --write 才寫檔。
";

/// 讀好、配好 id 的規格目錄。
pub struct Opened {
    pub tree: Tree,
    pub ids: IdReport,
}

/// 讀規格目錄並配 id。知識圖、save_run 之類的接點用這個。
pub fn open(root: &Path, display: &str, opts: &IdOptions) -> Result<Opened, String> {
    let mut tree = load(root, display)?;
    let reg = load_registry(root)?;
    let ids = assign(&mut tree, reg.as_ref(), opts)?;
    Ok(Opened { tree, ids })
}

/// 路徑參數：不能是空的、不能有 `..`。絕對路徑可以（規格可以在另一個 repo）。
pub fn check_root_arg(root: &str) -> Result<(), String> {
    if root.is_empty() || root.contains('\0') {
        return Err("--root 不能是空的".to_string());
    }
    if root.split(['/', '\\']).any(|c| c == "..") {
        return Err(format!("--root 不能有「..」：{}", clean(root)));
    }
    Ok(())
}

/// 變更名稱：一個目錄名，不能有路徑分隔、不能是 `.`、`..`、`archive`，不能用 `.` 開頭。
pub fn check_change_name(name: &str) -> Result<(), String> {
    let bad = name.is_empty() || name.starts_with('.') || name == "archive" || name.chars().any(|c| c == '/' || c == '\\' || c == ':' || c.is_control());
    if bad {
        return Err(format!("變更名稱不合法：「{}」（只能是 changes/ 底下的一個目錄名）", clean(name)));
    }
    Ok(())
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// `YYYY-MM-DD` 而且是存在的日期。
pub fn check_date(s: &str) -> Result<(), String> {
    let parts: Vec<&str> = s.split('-').collect();
    let ok = match parts.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 && s.bytes().all(|b| b.is_ascii_digit() || b == b'-') => match (y.parse::<i64>(), m.parse::<i64>(), d.parse::<i64>()) {
            (Ok(y), Ok(m), Ok(d)) => (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m),
            _ => false,
        },
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("--date 要是 YYYY-MM-DD 的日期：「{}」", clean(s)))
    }
}

/// 系統日期（UTC）。只用在歸檔目錄名。
fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Default)]
struct Args {
    root: Option<String>,
    change: Option<String>,
    date: Option<String>,
    id_pattern: Option<String>,
    adopt: Vec<String>,
    retire: Vec<String>,
    json: bool,
    write: bool,
    pos: Vec<String>,
}

const VALUE_FLAGS: &[&str] = &["--root", "--change", "--date", "--id-pattern", "--adopt", "--retire"];
const BOOL_FLAGS: &[&str] = &["--json", "--write"];

fn parse_args(sub: &str, rest: &[String], allowed: &[&str]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            a.pos.push(arg.clone());
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        let known = VALUE_FLAGS.contains(&flag) || BOOL_FLAGS.contains(&flag);
        if !known || !(allowed.contains(&flag) || flag == "--root" || flag == "--id-pattern") {
            return Err(format!("awf spec {sub} 不認得選項 {}", clean(flag)));
        }
        if BOOL_FLAGS.contains(&flag) {
            if inline.is_some() {
                return Err(format!("{flag} 不接值"));
            }
            match flag {
                "--json" => a.json = true,
                _ => a.write = true,
            }
            continue;
        }
        let value = match inline {
            Some(v) => v,
            None => it.next().cloned().ok_or_else(|| format!("{flag} 後面要給值"))?,
        };
        match flag {
            "--root" => a.root = Some(value),
            "--change" => a.change = Some(value),
            "--date" => a.date = Some(value),
            "--id-pattern" => a.id_pattern = Some(value),
            "--adopt" => a.adopt.push(value),
            _ => a.retire.push(value),
        }
    }
    Ok(a)
}

fn open_from(a: &Args, cwd: &Path) -> Result<Opened, String> {
    let root_s = a.root.clone().unwrap_or_else(|| "openspec".to_string());
    check_root_arg(&root_s)?;
    let root: PathBuf = if Path::new(&root_s).is_absolute() { PathBuf::from(&root_s) } else { cwd.join(&root_s) };
    let pattern = match &a.id_pattern {
        Some(p) => Some(Regex::new(p).map_err(|e| format!("--id-pattern 不是合法的正規式：{e}"))?),
        None => None,
    };
    let mut adopt = Vec::new();
    for s in &a.adopt {
        let (id, title) = s.split_once('=').ok_or_else(|| format!("--adopt 要寫成 <舊id>=<新標題>：「{}」", clean(s)))?;
        adopt.push((id.trim().to_string(), title.to_string()));
    }
    open(&root, &root_s, &IdOptions { pattern, adopt, retire: a.retire.clone() })
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

fn skipped_note(tree: &Tree, err: &mut String) {
    for s in &tree.skipped {
        err.push_str(&format!("略過 {}/{}（符號連結指到規格目錄外，或檔名讀不懂）\n", tree.display, clean(s)));
    }
}

fn problem_json(display: &str, p: &Problem) -> Value {
    json!({"path": format!("{display}/{}", p.file), "line": p.line, "code": p.code, "message": p.message})
}

fn cmd_validate(a: &Args, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    if let Some(c) = &a.change {
        check_change_name(c)?;
    }
    let o = open_from(a, cwd)?;
    skipped_note(&o.tree, err);
    if let Some(c) = &a.change {
        if o.tree.change(c, ChangeState::Active).is_none() {
            return Err(match o.tree.change(c, ChangeState::Archived) {
                Some(ar) => format!("變更「{}」已歸檔（{}/{}），沒有東西可以檢查", clean(c), o.tree.display, ar.dir),
                None => format!("找不到變更「{}」（{}/changes/ 底下沒有這個目錄）", clean(c), o.tree.display),
            });
        }
    }
    let problems = validate(&o.tree, &o.ids, a.change.as_deref());
    if a.json {
        let list: Vec<Value> = problems.iter().map(|p| problem_json(&o.tree.display, p)).collect();
        out.push_str(&pretty(&json!({"ok": problems.is_empty(), "problems": list})));
    } else if problems.is_empty() {
        let active = o.tree.changes.iter().filter(|c| c.state == ChangeState::Active).count();
        match &a.change {
            Some(c) => out.push_str(&format!("變更 {} 檢查通過\n", clean(c))),
            None => out.push_str(&format!("規格檢查通過（{} 個能力、{} 個進行中的變更）\n", o.tree.capabilities.len(), active)),
        }
    } else {
        for p in &problems {
            out.push_str(&clean(&format_problem(&o.tree.display, p)));
            out.push('\n');
        }
        err.push_str(&format!("共 {} 個問題\n", problems.len()));
    }
    Ok(i32::from(!problems.is_empty()))
}

/// 變更會動到的需求：（區塊、id、標題）。id 是空的代表現行規格找不到。
fn touched(ch: &Change) -> Vec<(&'static str, String, String)> {
    let mut v = Vec::new();
    for d in &ch.deltas {
        for r in &d.renamed {
            v.push(("RENAMED", r.id.clone(), r.to.clone()));
        }
        for (label, list) in [("REMOVED", &d.removed), ("MODIFIED", &d.modified), ("ADDED", &d.added)] {
            for r in list {
                v.push((label, r.id.clone(), r.title.clone()));
            }
        }
    }
    v
}

fn pending_json(display: &str, p: &Pending) -> Value {
    json!({"id": p.id, "title": p.title, "candidates": p.candidates, "path": format!("{display}/{}", p.file), "line": p.line})
}

fn cmd_status(a: &Args, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    let o = open_from(a, cwd)?;
    skipped_note(&o.tree, err);
    let t = &o.tree;
    let active: Vec<&Change> = t.changes.iter().filter(|c| c.state == ChangeState::Active).collect();
    let archived: Vec<&Change> = t.changes.iter().filter(|c| c.state == ChangeState::Archived).collect();
    if a.json {
        let caps: Vec<Value> = t
            .capabilities
            .iter()
            .map(|c| json!({"name": c.name, "path": format!("{}/{}", t.display, c.file), "requirements": c.requirements.len(), "scenarios": c.requirements.iter().map(|r| r.scenarios.len()).sum::<usize>()}))
            .collect();
        let chs: Vec<Value> = active
            .iter()
            .map(|c| {
                let reqs: Vec<Value> = touched(c).into_iter().map(|(op, id, title)| json!({"op": op, "id": id, "title": title})).collect();
                json!({"name": c.name, "path": format!("{}/{}", t.display, c.dir), "tasks": {"done": c.tasks.done, "total": c.tasks.total}, "requirements": reqs})
            })
            .collect();
        let arch: Vec<Value> = archived.iter().map(|c| json!({"name": c.name, "date": c.archived_date, "path": format!("{}/{}", t.display, c.dir)})).collect();
        let pend: Vec<Value> = o.ids.pending.iter().map(|p| pending_json(&t.display, p)).collect();
        out.push_str(&pretty(&json!({"root": t.display, "capabilities": caps, "changes": chs, "archived": arch, "pending_ids": pend, "ids_file": o.ids.existed, "unregistered_id_changes": o.ids.changes.len()})));
        return Ok(0);
    }
    out.push_str(&format!("規格根目錄：{}\n", t.display));
    out.push_str(&format!("能力（{}）\n", t.capabilities.len()));
    for c in &t.capabilities {
        let scn: usize = c.requirements.iter().map(|r| r.scenarios.len()).sum();
        out.push_str(&format!("  {}　需求 {}、情境 {}　{}/{}\n", clean(&c.name), c.requirements.len(), scn, t.display, clean(&c.file)));
    }
    out.push_str(&format!("進行中的變更（{}）\n", active.len()));
    for c in &active {
        let pct = if c.tasks.total == 0 { 0 } else { c.tasks.done * 100 / c.tasks.total };
        out.push_str(&format!("  {}　tasks {}/{}（{pct}%）\n", clean(&c.name), c.tasks.done, c.tasks.total));
        for (op, id, title) in touched(c) {
            let id = if id.is_empty() { "（現行規格找不到）".to_string() } else { id };
            out.push_str(&format!("    {op}　{}　「{}」\n", clean(&id), clean(&title)));
        }
    }
    out.push_str(&format!("已歸檔的變更：{}\n", archived.len()));
    out.push_str(&format!("待確認的 id（{}）\n", o.ids.pending.len()));
    for p in &o.ids.pending {
        pending_text(&t.display, p, out);
    }
    if !o.ids.changes.is_empty() {
        let hint = if o.ids.existed { "和 ids.json 有出入" } else { "ids.json 還沒建立" };
        out.push_str(&format!("{hint}：{} 項還沒登記，用 awf spec ids 看、加 --write 寫入\n", o.ids.changes.len()));
    }
    Ok(0)
}

fn pending_text(display: &str, p: &Pending, out: &mut String) {
    let cands: Vec<String> = p.candidates.iter().map(|c| format!("「{}」", clean(c))).collect();
    out.push_str(&format!("  {}　登記的標題「{}」找不到了；可能改成了 {}（{display}/{}:{}）\n", clean(&p.id), clean(&p.title), cands.join("、"), clean(&p.file), p.line));
    out.push_str(&format!("    是改名：awf spec ids --adopt '{}=<新標題>' --write\n", clean(&p.id)));
    out.push_str(&format!("    舊的不要了：awf spec ids --retire '{}' --write\n", clean(&p.id)));
}

fn cmd_show(a: &Args, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    let [id] = a.pos.as_slice() else { return Err("用法：awf spec show <id>".to_string()) };
    let o = open_from(a, cwd)?;
    skipped_note(&o.tree, err);
    match o.tree.find(id) {
        Some(f) => {
            out.push_str(&format!("{}/{}:{}-{}　{}：{}\n", o.tree.display, clean(&f.file), f.span.start, f.span.end, if f.is_scenario { "情境" } else { "需求" }, clean(&f.title)));
            Ok(0)
        }
        None => Err(format!("找不到 id「{}」（用 awf spec status 或 awf spec ids 看有哪些）", clean(id))),
    }
}

fn cmd_ids(a: &Args, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    let o = open_from(a, cwd)?;
    skipped_note(&o.tree, err);
    let t = &o.tree;
    let text = registry_text(&o.ids.registry);
    let current = std::fs::read(t.root.join(ids::IDS_FILE)).ok();
    let differs = current.as_deref() != Some(text.as_bytes());
    let blockers: Vec<&Problem> = t.problems.iter().chain(o.ids.problems.iter()).filter(|p| matches!(p.code, "dup-title" | "dup-scenario" | "ids-dup" | "unclosed-fence" | "read")).filter(|p| !p.scope.starts_with("change:")).collect();
    let mut written = false;
    let mut code = i32::from(!o.ids.pending.is_empty());
    if a.write {
        if !blockers.is_empty() {
            for p in &blockers {
                err.push_str(&clean(&format_problem(&t.display, p)));
                err.push('\n');
            }
            err.push_str("規格有上面的問題，id 會配錯，沒有寫入 ids.json\n");
            code = 1;
        } else if differs {
            write_registry(&t.root, &o.ids.registry)?;
            written = true;
        }
    }
    if a.json {
        let pend: Vec<Value> = o.ids.pending.iter().map(|p| pending_json(&t.display, p)).collect();
        out.push_str(&pretty(&json!({"path": format!("{}/{}", t.display, ids::IDS_FILE), "changes": o.ids.changes, "pending": pend, "written": written})));
        return Ok(code);
    }
    if !o.ids.existed {
        out.push_str(&format!("{}/{} 還不存在\n", t.display, ids::IDS_FILE));
    }
    out.push_str(&format!("變動（{}）\n", o.ids.changes.len()));
    for c in &o.ids.changes {
        out.push_str(&format!("  {c}\n"));
    }
    out.push_str(&format!("待確認（{}）\n", o.ids.pending.len()));
    for p in &o.ids.pending {
        pending_text(&t.display, p, out);
    }
    if written {
        out.push_str(&format!("已寫入 {}/{}\n", t.display, ids::IDS_FILE));
    } else if a.write && blockers.is_empty() {
        out.push_str("ids.json 已經是最新的，沒有寫入\n");
    } else if !a.write && differs {
        out.push_str("（預覽，沒有寫任何檔；加 --write 才寫入 ids.json）\n");
    }
    Ok(code)
}

fn cmd_apply(a: &Args, cwd: &Path, out: &mut String, err: &mut String) -> Result<i32, String> {
    let name = a.change.as_deref().ok_or("apply 要給 --change <名>")?;
    check_change_name(name)?;
    let date = match &a.date {
        Some(d) => {
            check_date(d)?;
            d.clone()
        }
        None => today(),
    };
    let o = open_from(a, cwd)?;
    skipped_note(&o.tree, err);
    let t = &o.tree;
    if t.change(name, ChangeState::Active).is_none() {
        return Err(match t.change(name, ChangeState::Archived) {
            Some(ar) => format!("變更「{}」已歸檔（{}/{}），不重做", clean(name), t.display, ar.dir),
            None => format!("找不到變更「{}」（{}/changes/ 底下沒有這個目錄）", clean(name), t.display),
        });
    }
    let problems = validate(t, &o.ids, Some(name));
    if !problems.is_empty() {
        for p in &problems {
            err.push_str(&clean(&format_problem(&t.display, p)));
            err.push('\n');
        }
        err.push_str(&format!("validate 沒過（{} 個問題），拒絕 apply，沒有寫任何檔\n", problems.len()));
        return Ok(1);
    }
    let p = plan(t, name, &date)?;
    out.push_str(&apply::render(&p, &t.display));
    if a.write {
        commit(&t.root, &p)?;
        out.push_str(&format!("已寫入 {} 個規格檔，變更已歸檔\n", p.writes.len()));
    } else {
        out.push_str("（預覽，沒有寫任何檔；加 --write 才寫入）\n");
    }
    Ok(0)
}

/// 執行 `awf spec …`，回（exit code、stdout、stderr）。`cwd` 是相對路徑的基準。
pub fn run_captured(argv: &[String], cwd: &Path) -> (i32, String, String) {
    let (mut out, mut err) = (String::new(), String::new());
    let Some(sub) = argv.first().map(String::as_str) else {
        return (2, out, USAGE.to_string());
    };
    let rest = argv.get(1..).unwrap_or(&[]);
    let res = match sub {
        "-h" | "--help" | "help" => {
            out.push_str(USAGE);
            Ok(0)
        }
        "--selftest" => return selftest(),
        "validate" => parse_args(sub, rest, &["--change", "--json"]).map_err(usage).and_then(|a| no_pos(sub, a)).and_then(|a| cmd_validate(&a, cwd, &mut out, &mut err).map_err(fail)),
        "status" => parse_args(sub, rest, &["--json"]).map_err(usage).and_then(|a| no_pos(sub, a)).and_then(|a| cmd_status(&a, cwd, &mut out, &mut err).map_err(fail)),
        "show" => parse_args(sub, rest, &[]).map_err(usage).and_then(|a| cmd_show(&a, cwd, &mut out, &mut err).map_err(fail)),
        "ids" => parse_args(sub, rest, &["--write", "--adopt", "--retire", "--json"]).map_err(usage).and_then(|a| no_pos(sub, a)).and_then(|a| cmd_ids(&a, cwd, &mut out, &mut err).map_err(fail)),
        "apply" => parse_args(sub, rest, &["--change", "--write", "--date"]).map_err(usage).and_then(|a| no_pos(sub, a)).and_then(|a| cmd_apply(&a, cwd, &mut out, &mut err).map_err(fail)),
        other => Err((2, format!("awf spec 沒有「{}」這個子指令\n\n{USAGE}", clean(other)))),
    };
    match res {
        Ok(code) => (code, out, err),
        Err((code, msg)) => {
            err.push_str(&format!("awf spec：{msg}\n"));
            (code, out, err)
        }
    }
}

fn usage(msg: String) -> (i32, String) {
    (2, msg)
}

fn fail(msg: String) -> (i32, String) {
    (1, msg)
}

fn no_pos(sub: &str, a: Args) -> Result<Args, (i32, String)> {
    match a.pos.first() {
        Some(p) => Err((2, format!("awf spec {sub} 不接受多的參數「{}」", clean(p)))),
        None => Ok(a),
    }
}

/// `awf spec …`
pub fn main(args: &[String]) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (code, out, err) = run_captured(args, &cwd);
    let _ = std::io::stdout().write_all(out.as_bytes());
    let _ = std::io::stderr().write_all(err.as_bytes());
    code
}

// ───────────────────────── selftest ─────────────────────────

const ST_ORDERS: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n\n### Requirement: 取消訂單\n系統 SHALL 讓會員在出貨前取消訂單。\n\n#### Scenario: 出貨前\n- **WHEN** 訂單還沒出貨\n- **THEN** 可以取消\n\n### Requirement: 傳真下單\n系統 SHALL 接受傳真訂單。\n\n#### Scenario: 收到傳真\n- **WHEN** 收到傳真\n- **THEN** 建立訂單\n";
const ST_LOGIN: &str = "# login Specification\r\n\r\n## Purpose\r\n示範客戶 ACME 的會員登入。\r\n\r\n## Requirements\r\n\r\n### Requirement: 密碼登入\r\n系統 SHALL 驗證帳號密碼。\r\n\r\n```md\r\n### Requirement: 圍欄裡的不算\r\n```\r\n\r\n#### Scenario: 密碼正確\r\n- **WHEN** 密碼正確\r\n- **THEN** 進入首頁\r\n";
const ST_DELTA_ORDERS: &str = "# orders 的差異\n\n## ADDED Requirements\n\n### Requirement: 申請退款\n系統 SHALL 讓會員在七天內申請退款。\n\n#### Scenario: 七天內\n- **WHEN** 收貨未滿七天\n- **THEN** 受理退款\n\n## MODIFIED Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單並寄出確認信。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立並寄出確認信\n\n## REMOVED Requirements\n\n### Requirement: 傳真下單\n**Reason**: 已經沒有人用傳真。\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: 取消訂單`\n- TO: `### Requirement: 撤回訂單`\n";
const ST_DELTA_BILLING: &str = "## ADDED Requirements\n\n### Requirement: 開立發票\n系統 SHALL 在付款後開立發票。\n\n#### Scenario: 付款完成\n- **WHEN** 付款完成\n- **THEN** 開立發票\n";

fn st_write(root: &Path, rel: &str, text: &str) -> Result<(), String> {
    let p = read::rel_path(root, rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, text.as_bytes()).map_err(|e| e.to_string())
}

fn st_fixture(base: &Path) -> Result<(), String> {
    st_write(base, "openspec/specs/orders/spec.md", ST_ORDERS)?;
    st_write(base, "openspec/specs/login/spec.md", ST_LOGIN)?;
    st_write(base, "openspec/changes/add-refund/proposal.md", "# 加上退款\n\n示範客戶 ACME 要讓會員退款。\n")?;
    st_write(base, "openspec/changes/add-refund/tasks.md", "## 1. 實作\n- [x] 1.1 退款 API\n- [ ] 1.2 發票\n- [ ] 1.3 文件\n")?;
    st_write(base, "openspec/changes/add-refund/specs/orders/spec.md", ST_DELTA_ORDERS)?;
    st_write(base, "openspec/changes/add-refund/specs/billing/spec.md", ST_DELTA_BILLING)
}

fn st_snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.strip_prefix(dir).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if p.is_dir() {
                out.push((name, Vec::new()));
                stack.push(p);
            } else {
                out.push((name, std::fs::read(&p).unwrap_or_default()));
            }
        }
    }
    out.sort();
    out
}

fn selftest() -> (i32, String, String) {
    let mut out = String::new();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let base = std::env::temp_dir().join(format!("awf-spec-selftest-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    if let Err(e) = st_fixture(&base) {
        let _ = std::fs::remove_dir_all(&base);
        return (1, out, format!("awf spec --selftest：建不了暫存目錄：{e}\n"));
    }
    let run = |args: &[&str]| run_captured(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &base);
    let read = |rel: &str| std::fs::read_to_string(read::rel_path(&base, rel)).unwrap_or_default();
    let (mut pass, mut total) = (0, 0);
    let mut check = |name: &str, ok: bool, detail: &str| {
        total += 1;
        if ok {
            pass += 1;
            out.push_str(&format!("  [通過] {name}\n"));
        } else {
            out.push_str(&format!("  [失敗] {name}\n{detail}\n"));
        }
    };

    let (c, o, e) = run(&["status"]);
    check("status：列出能力、變更的 tasks 比例與會動到的需求 id", c == 0 && o.contains("orders　需求 3、情境 3") && o.contains("login　需求 1、情境 1") && o.contains("add-refund　tasks 1/3") && o.contains("MODIFIED　orders/建立訂單") && o.contains("RENAMED　orders/取消訂單") && o.contains("ADDED　billing/開立發票"), &format!("{o}{e}"));
    let (c, o, _) = run(&["status", "--json"]);
    check("status --json：是合法的 JSON", c == 0 && serde_json::from_str::<Value>(&o).is_ok_and(|v| v["capabilities"].as_array().is_some_and(|a| a.len() == 2)), &o);
    let (c, o, e) = run(&["validate"]);
    check("validate：乾淨的規格與變更通過", c == 0, &format!("{o}{e}"));
    let (c, o, _) = run(&["show", "login/密碼登入#密碼正確"]);
    check("show：印出 路徑:起訖行 與標題（CRLF 與圍欄不影響行號）", c == 0 && o.starts_with("openspec/specs/login/spec.md:15-17　情境：密碼正確"), &o);

    let before = st_snapshot(&base);
    let (c, o, _) = run(&["ids"]);
    check("ids：預設只預覽，不寫檔", c == 0 && o.contains("+ orders/建立訂單") && st_snapshot(&base) == before, &o);
    let (c, _, e) = run(&["ids", "--write"]);
    let ids1 = read("openspec/ids.json");
    let _ = std::fs::remove_file(base.join("openspec/ids.json"));
    let (c2, _, _) = run(&["ids", "--write"]);
    check("ids --write：寫出 ids.json，重跑逐位元相同", c == 0 && c2 == 0 && !ids1.is_empty() && ids1 == read("openspec/ids.json") && ids1.ends_with("}\n"), &e);

    let (c, o, e) = run(&["apply", "--change", "add-refund", "--date", "2026-01-02"]);
    let before = st_snapshot(&base);
    let (_, o2, _) = run(&["apply", "--change", "add-refund", "--date", "2026-01-02"]);
    check("apply：預設只印 unified diff，目錄逐位元不變，重跑輸出相同", c == 0 && o.contains("--- /dev/null\n+++ b/openspec/specs/billing/spec.md") && o.contains("-### Requirement: 傳真下單") && o.contains("+### Requirement: 撤回訂單") && o == o2 && st_snapshot(&base) == before, &format!("{o}{e}"));
    let (c, _, e) = run(&["apply", "--change", "../add-refund"]);
    let (c2, _, _) = run(&["status", "--root", "../openspec"]);
    check("越界：變更名含路徑分隔、--root 含 .. 都被拒", c != 0 && c2 != 0 && st_snapshot(&base) == before, &e);
    let (c, _, e) = run(&["apply", "--change", "add-refund", "--date", "2026-13-40"]);
    check("apply：不存在的日期被拒", c != 0 && e.contains("--date"), &e);

    let (c, o, e) = run(&["apply", "--change", "add-refund", "--date", "2026-01-02", "--write"]);
    let orders = read("openspec/specs/orders/spec.md");
    let ok = c == 0
        && orders.contains("### Requirement: 申請退款")
        && orders.contains("並寄出確認信。")
        && !orders.contains("傳真下單")
        && orders.contains("### Requirement: 撤回訂單")
        && read("openspec/specs/billing/spec.md").starts_with("# billing Specification\n\n## Purpose\n")
        && base.join("openspec/changes/archive/2026-01-02-add-refund/proposal.md").is_file()
        && !base.join("openspec/changes/add-refund").exists();
    check("apply --write：ADDED／MODIFIED／REMOVED／RENAMED 都併入，新能力建檔，變更歸檔", ok, &format!("{o}{e}{orders}"));
    let (c, o, e) = run(&["validate"]);
    check("apply 之後 validate 通過", c == 0, &format!("{o}{e}"));
    let (c, _, e) = run(&["apply", "--change", "add-refund", "--write"]);
    check("apply 第二次：明確回報已歸檔，不重做", c == 1 && e.contains("已歸檔"), &e);
    let (c, o, _) = run(&["ids"]);
    check("RENAMED 沿用舊 id（不列待確認）", c == 0 && o.contains("~ orders/取消訂單　標題「取消訂單」→「撤回訂單」") && o.contains("待確認（0）"), &o);
    let (c, _, _) = run(&["ids", "--write"]);
    let (c2, o, _) = run(&["show", "orders/取消訂單"]);
    check("改名後用舊 id 找得到新標題", c == 0 && c2 == 0 && o.contains("需求：撤回訂單"), &o);

    let orders = read("openspec/specs/orders/spec.md");
    let _ = st_write(&base, "openspec/specs/orders/spec.md", &orders.replace("Requirement: 申請退款", "Requirement: 申請退款與退貨"));
    let (c, o, _) = run(&["ids"]);
    check("手動改標題：列成待確認，不自動猜", c == 1 && o.contains("待確認（1）") && o.contains("orders/申請退款"), &o);
    let (c, o, e) = run(&["ids", "--adopt", "orders/申請退款=申請退款與退貨", "--write"]);
    let (c2, o2, _) = run(&["show", "orders/申請退款"]);
    check("ids --adopt：人確認後 id 不變", c == 0 && c2 == 0 && o2.contains("需求：申請退款與退貨"), &format!("{o}{e}"));

    let _ = st_write(&base, "openspec/specs/login/spec.md", "### Requirement: 密碼登入\n沒有情境。\n\n### Requirement: 密碼登入\n#### Scenario: 一\n");
    let _ = st_write(&base, "openspec/changes/bad/specs/orders/spec.md", "## MODIFIED Requirements\n### Requirement: 不存在的需求\n#### Scenario: 一\n\n## ADDED Requirements\n### Requirement: 建立訂單\n\n## RENAMED Requirements\n- FROM: `### Requirement: 撤回訂單`\n");
    let (c, o, _) = run(&["validate"]);
    let ok = c == 1
        && o.contains("openspec/specs/login/spec.md:1：需求「密碼登入」沒有情境")
        && o.contains("openspec/specs/login/spec.md:4：需求「密碼登入」重複")
        && o.contains("openspec/changes/bad/specs/orders/spec.md:2：MODIFIED 指到的需求「不存在的需求」")
        && o.contains("spec.md:6：ADDED 的需求「建立訂單」在現行規格")
        && o.contains("spec.md:6：需求「建立訂單」沒有情境")
        && o.contains("spec.md:9：FROM 沒有對應的 TO")
        && o.contains("openspec/changes/bad/proposal.md:1：變更缺 proposal.md")
        && o.contains("openspec/changes/bad/tasks.md:1：變更缺 tasks.md");
    check("validate：缺情境、同名、指不到、已存在、FROM／TO 不成對、缺檔，各一行 路徑:行號：說明", ok, &o);
    let (c, _, e) = run(&["apply", "--change", "bad", "--write"]);
    check("apply：validate 沒過就拒絕", c == 1 && e.contains("拒絕 apply") && base.join("openspec/changes/bad").is_dir(), &e);

    let _ = std::fs::remove_dir_all(&base);
    out.push_str(&format!("awf spec --selftest：{pass}/{total} 通過\n"));
    (i32::from(pass != total), out, String::new())
}
