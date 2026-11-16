//! watcher：進度事件、通知、狀態列。原本是 0.x 的 Python 工具 `awf_watch.py`。
//!
//! 這支是輔助工具，不參與任何閘門：它壞掉、沒開、`progress.jsonl` 被改，流程與核准都不受影響。
//! 所以任何失敗都只警告；唯二不能放鬆的是送給外部指令的內容：只送 [`NOTIFY_FIELDS`] 的欄位，
//! 命中 [`SECRETISH`] 整筆不送。
//!
//! 快照與記憶都用 `serde_json::Value` 表示（鍵的順序和 Python 的 dict 相同），對記憶檔（任何人都寫得到）
//! 的每個操作照 Python 的語意回 [`PyErr`]，警告裡印的例外類別才會一樣。
//!
//! 和 0.x 的 Python 版的差別（都列在 `tests/watch_parity.rs`）：
//! - JSON 照閘門的規則讀（NaN、超出 64 位元的整數、巢狀太深、落單的代理字元都當成讀不懂）。
//! - 「現在」可以固定：[`Env::now`]（命令列讀環境變數 `AWF_NOW`），測試用。

use crate::cost::{self, Crash, TailState, Ts};
use crate::hooks::py::{json_dumps, shlex_split};
use crate::hooks::re::Re;
use crate::kb::pyish;
use crate::merge_gate::pyutil;
use crate::pyfmt;
use crate::save_run::py::{self, get, idx, pj, truthy, NULL, R};
use crate::save_run::{self, Ctx, PyErr};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Write;

pub const USAGE: &str = "watcher：把一個 run 的進度寫成外部程式看得懂的事件。

用法（在已 init 的專案根目錄）：
    awf watch --run <run_dir> [--interval 5] [--stall-seconds 900]   # 長駐：每隔幾秒寫 <run_dir>/progress.jsonl
    awf watch --run <run_dir> --once [--json]                       # 只看一次：印快照，順便補寫事件
    awf statusline                                                  # 一行狀態（給 Claude Code 的 statusLine 用）
    awf watch --selftest

三個來源，全部唯讀：
    1. run.json 與各段結果：段開始、存檔、等核准、核准、結束。
    2. 執行紀錄檔（<run_dir>/<段>/<label>.md）的出現：哪個 agent 交件了。
    3. Claude Code 的 workflow transcript：哪些 agent 在跑、用哪個模型、目前幾個 token、最後活動時間。
       這個格式官方沒有承諾穩定；讀不懂時事件只是少了 token 與模型，不會中斷。

事件格式見 schemas/progress-event.schema.json（一行一個 JSON）。外部程式 tail 這個檔就好，不需要網路。
workflow 還沒結束卻超過 --stall-seconds 沒有任何 agent 有動靜時，寫一筆 workflow_stalled（例如帳號額度用完，agent 全停住）。

這支不參與任何閘門：它壞掉、沒開、progress.jsonl 被改，流程與核准都不受影響（save_run、publish、merge_gate 不讀這些檔）。
profile 的 progress.notify 有設才會呼叫外部指令（預設不設）：每個事件呼叫一次，只送白名單欄位，內容像機敏值就不送。只用標準庫。
";

pub const PROGRESS: &str = "progress.jsonl";
pub const STATE: &str = ".watch-state.json";
pub const EVENT_TYPES: [&str; 11] = [
    "run_state", "stage_started", "stage_saved", "waiting_gate", "stage_approved", "record_written",
    "agent_started", "agent_progress", "workflow_finished", "workflow_stalled", "watch_note",
];
/// `progress.jsonl` 事件的格式版本：只加欄位或事件類型時不變，欄位改名、移除或換意思才遞增（相容承諾見 docs/監控與整合.md）。
pub const PROGRESS_SCHEMA_VERSION: u64 = 1;
/// 送給外部指令（notify）的欄位白名單：都是狀態與數字，沒有需求內容、摘要、問題、路徑。
pub const NOTIFY_FIELDS: [&str; 19] = [
    "v", "schema_version", "seq", "at", "type", "run", "stage", "status", "state", "by", "kind", "label", "role", "model", "tokens", "requests",
    "idle_seconds", "workflow", "wf_run",
];
pub static SECRETISH: Re = Re::new(
    r"ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16}|xox[abprs]-[A-Za-z0-9-]{10,}|-----BEGIN [A-Z ]*PRIVATE KEY|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}\.|(?i:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S{6,}",
    "",
);
/// 送給外部指令的文字欄位：不含路徑分隔、引號、換行（0.x 的 Python 版用 fullmatch；這裡錨在開頭、再看是不是吃到結尾）。
pub static SAFE_TEXT: Re = Re::new(r"^[\w:.#@+\- 一-鿿]{0,80}", "");
/// 同一個 agent 每多這麼多 token 才再記一筆 agent_progress。
pub const PROGRESS_STEP: i128 = 50_000;
/// 輪詢間隔與停滯門檻的預設（秒）。
pub const DEFAULT_INTERVAL: i64 = 5;
pub const DEFAULT_STALL: i64 = 900;

/// watcher 的節奏（profile 的 `progress.watch`）。預設＝原本寫死的值；沒寫或不合範圍的那一項用預設。
/// `interval`、`stall_seconds` 只是命令列旗標沒給時的預設，旗標有給以旗標為準。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchTune {
    pub interval: i64,
    pub stall_seconds: i64,
    /// 同一個 agent 每多用這麼多 token 記一筆 agent_progress。
    pub progress_step: i64,
}

impl Default for WatchTune {
    fn default() -> Self {
        WatchTune { interval: DEFAULT_INTERVAL, stall_seconds: DEFAULT_STALL, progress_step: PROGRESS_STEP as i64 }
    }
}

impl WatchTune {
    pub fn of(profile: &Value) -> WatchTune {
        use crate::tune::{at, int};
        let k = |name: &str| at(profile, &["progress", "watch", name]);
        WatchTune {
            interval: int(k("interval"), 1, 3600, DEFAULT_INTERVAL),
            stall_seconds: int(k("stall_seconds"), 0, 86_400, DEFAULT_STALL),
            progress_step: int(k("progress_step"), 1000, 10_000_000, PROGRESS_STEP as i64),
        }
    }
}

thread_local! {
    /// 這次執行的 agent_progress 間距。`awf watch` 依 profile 放進來；直接呼叫函式（測試、對跑基準）時是預設值。
    static STEP: std::cell::Cell<i128> = const { std::cell::Cell::new(PROGRESS_STEP) };
}

pub fn set_progress_step(n: i64) {
    STEP.with(|c| c.set(i128::from(n)));
}
pub const FINISHED: [&str; 7] = ["completed", "failed", "cancelled", "canceled", "error", "stopped", "killed"];

const DAY_US: i64 = 86_400_000_000;
const SKIPPED: &str = "(略)";

/// 外部指令的執行器：（argv, stdin）→ exit code；跑不起來回 Python 的例外類別名。
pub type Runner<'a> = &'a mut dyn FnMut(&[String], &str) -> Result<i32, String>;

/// 執行環境。
pub struct Env {
    /// 專案的 `.claude/teams`、profile；相對路徑的基準。
    pub ctx: Ctx,
    /// 專案根目錄（`--run` 的相對路徑接在它後面；statusline 從它找 runs_dir）。
    pub project: String,
    /// transcript 的根目錄（`AWF_TRANSCRIPTS_DIR`）；None 用 Claude Code 存這個專案的位置。
    pub transcripts: Option<String>,
    /// 固定的「現在」（ISO 8601、要帶時區）；None 用系統時間。命令列讀環境變數 `AWF_NOW`。
    pub now: Option<String>,
    /// 長駐模式兩次之間的等待；回 false 等於按了 Ctrl-C（結束、exit 0）。None 真的睡。
    pub sleep: Option<Box<dyn FnMut(u64) -> bool>>,
    /// 外部指令的執行器；None 真的跑。
    pub runner: Option<Box<dyn FnMut(&[String], &str) -> Result<i32, String>>>,
}

impl Env {
    /// 命令列用：專案根目錄照 [`crate::project`] 的解析，找不到就用目前目錄。
    pub fn real() -> Env {
        let ctx = Ctx::from_env();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Env { project: ctx.pack_root.clone(), ctx, transcripts: var("AWF_TRANSCRIPTS_DIR"), now: var("AWF_NOW"), sleep: None, runner: None }
    }

    /// （現在, 事件的時間戳）。
    pub fn clock(&self) -> (Ts, String) {
        let now = self.now.as_deref().and_then(parse_now).unwrap_or_else(real_now);
        let at = utc_stamp(&now).unwrap_or_default();
        (now, at)
    }
}

// ------------------------------------------------------------------ 時間

/// 系統時間（UTC）。
pub fn real_now() -> Ts {
    let us = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0);
    Ts { naive: 719_163 * DAY_US + us, offset: Some(0) }
}

/// 固定的「現在」：要帶時區（`Z` 或 `+08:00`），讀不懂回 None。
pub fn parse_now(s: &str) -> Option<Ts> {
    cost::fromisoformat(&s.replace('Z', "+00:00")).filter(|t| t.offset.is_some())
}

/// `t.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")`（t 帶時區）。
pub fn utc_stamp(t: &Ts) -> R<String> {
    let u = t.naive - t.offset.unwrap_or(0);
    let ord = u.div_euclid(DAY_US);
    let date = if (1..=3_652_059).contains(&ord) { pyutil::from_ordinal(ord) } else { None };
    let Some((y, m, d)) = date else {
        return Err(PyErr::other("OverflowError", "date value out of range"));
    };
    let secs = u.rem_euclid(DAY_US) / 1_000_000;
    Ok(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60))
}

fn crash(c: Crash) -> PyErr {
    match c.0.split_once(": ") {
        Some((class, msg)) => PyErr::other(class, msg),
        None => PyErr::other("Exception", c.0),
    }
}

// ------------------------------------------------------------------ 小工具

fn opt<'a>(argv: &'a [String], name: &str, default: &'a str) -> &'a str {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).map_or(default, String::as_str)
}

/// 自己組出來的快照取欄位（缺了當 None）。
fn f<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&NULL)
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// `x or ""` 之後 `str(...)`
fn s_or_empty(v: &Value) -> String {
    py::s_or(v, "")
}

/// `load_json(p)`：讀不到、不是看得懂的 JSON 都回 None。
fn load_json(ctx: &Ctx, p: &str) -> Option<Value> {
    ctx.load_json(p).ok()
}

fn is_dir(p: &str) -> bool {
    !p.contains('\0') && std::path::Path::new(p).is_dir()
}

fn is_file(p: &str) -> bool {
    !p.contains('\0') && std::path::Path::new(p).is_file()
}

/// 目錄裡的檔名（讀不到當成空的；不是 UTF-8 的名字略過）。
fn names_in(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect()
}

/// `sorted(wf_dir.glob("agent-*.jsonl"))` 的檔名。
fn agent_files(wf_dir: &str) -> Vec<String> {
    let mut out: Vec<String> =
        names_in(wf_dir).into_iter().filter(|n| n.len() >= "agent-.jsonl".len() && n.starts_with("agent-") && n.ends_with(".jsonl")).collect();
    out.sort();
    out
}

/// `Path(p).stem`
fn stem(p: &str) -> String {
    let name = py::pname(p);
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name.get(..i).unwrap_or_default().to_string(),
        _ => name,
    }
}

/// `f"{n:,}"`
fn commas(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// `int(v or 0)`
fn py_int(v: &Value) -> R<i128> {
    if !truthy(v) {
        return Ok(0);
    }
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Number(n) => Ok(n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)).unwrap_or_else(|| n.as_f64().unwrap_or(0.0).trunc() as i128)),
        Value::String(t) => pyutil::int_of_str(t).ok_or_else(|| PyErr::other("ValueError", format!("invalid literal for int() with base 10: {}", pyfmt::repr_str(t)))),
        other => Err(py::type_err(format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", py::tname(other)))),
    }
}

fn int_of(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

// ------------------------------------------------------------------ 來源 1、2：run 目錄

/// 一段的紀錄檔（`<run>/<段>/*.md` 去掉索引與資料包），照名字排。
fn records(ctx: &Ctx, run: &str, name: &str) -> Vec<String> {
    if name.contains('\0') {
        return Vec::new();
    }
    let d = ctx.abs(&pj(run, name));
    if !is_dir(&d) {
        return Vec::new();
    }
    let mut out: Vec<String> = names_in(&d)
        .into_iter()
        .filter(|n| n.ends_with(".md") && n != save_run::INDEX_NAME && n != "pack.md" && is_file(&pyish::pjoin(&d, n)))
        .map(|n| n.get(..n.len() - 3).unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

/// run.json 與紀錄檔整理成一份快照的前半（沒有 transcript 的部分）。run.json 讀不到回 None。
pub fn run_view(ctx: &Ctx, run: &str) -> R<Option<Value>> {
    let Some(data) = load_json(ctx, &pj(run, "run.json")) else { return Ok(None) };
    let Some(all) = data.as_object().and_then(|o| o.get("stages")).and_then(Value::as_object) else { return Ok(None) };
    let snap = match save_run::status_snapshot(ctx, run, &data) {
        Ok(s) => s,
        Err(_) => {
            // 快照算不出來時退回最少的資訊
            let mut rows = Vec::new();
            for (k, v) in all {
                let empty = json!({});
                let st = if truthy(v) { v } else { &empty };
                rows.push(json!({"name": k, "status": py::s(get(st, "status")?), "gate": truthy(get(st, "gate")?), "approved": null,
                                 "agents": 0, "agents_missing": 0, "missing_records": 0}));
            }
            json!({"state": "running", "next": null, "waiting_gate": null, "stages": rows})
        }
    };
    let mut stages = Vec::new();
    for s in f(&snap, "stages").as_array().map(Vec::as_slice).unwrap_or_default() {
        let name = text(f(s, "name"));
        let empty = json!({});
        let st = all.get(name).filter(|v| truthy(v)).unwrap_or(&empty);
        let ap = get(st, "approved")?;
        let approved = if ap.is_object() && truthy(ap) {
            json!({"by": py::s(get(ap, "by")?), "kind": py::s_or(get(ap, "kind")?, "human")})
        } else {
            Value::Null
        };
        stages.push(json!({"name": name, "status": f(s, "status"), "gate": f(s, "gate"), "approved": approved,
                           "agents": f(s, "agents"), "agents_missing": f(s, "agents_missing"), "records": records(ctx, run, name)}));
    }
    let awf = get(&data, "awf")?;
    let pend = if truthy(awf) { get(awf, "pending")? } else { &NULL };
    let pending = if truthy(pend) {
        json!({"stage": s_or_empty(get(pend, "stage")?), "workflow": s_or_empty(get(pend, "workflow")?)})
    } else {
        Value::Null
    };
    Ok(Some(json!({"v": 1, "run": py::pname(run), "event": s_or_empty(get(&data, "event")?), "state": f(&snap, "state"), "next": f(&snap, "next"),
                   "waiting_gate": f(&snap, "waiting_gate"), "pending": pending, "stages": stages, "workflows": []})))
}

// ------------------------------------------------------------------ 來源 3：workflow transcript

/// 這個 run 的 workflow transcript。哪些 wf_* 目錄屬於這個 run 只判一次；每個 agent 檔只讀新增的部分。
pub struct Transcripts {
    root: Option<String>,
    needles: Vec<String>,
    matched: HashMap<String, bool>,
    tails: HashMap<String, TailState>,
    /// 讀 transcript 時碰到的問題（留著不清，快照的 note 會帶出來）。
    pub broken: String,
}

impl Transcripts {
    /// `run` 要是絕對路徑，或相對於行程的目前目錄。
    pub fn new(root: Option<&str>, run: &str) -> Transcripts {
        let mut needles = vec![pyish::ppath(run)];
        if let Ok(r) = pyish::resolve(run) {
            if !needles.contains(&r) {
                needles.push(r);
            }
        }
        Transcripts { root: root.map(pyish::ppath), needles, matched: HashMap::new(), tails: HashMap::new(), broken: String::new() }
    }

    fn mine(&mut self, wf_dir: &str) -> R<bool> {
        if let Some(hit) = self.matched.get(wf_dir) {
            return Ok(*hit);
        }
        let meta = cost::read_wf_meta(wf_dir).map_err(crash)?;
        let script = s_or_empty(meta.get("scriptPath").unwrap_or(&NULL)).replace('\\', "/");
        let mut hit = self.needles.iter().any(|n| !n.is_empty() && script.contains(n.as_str()));
        let files = agent_files(wf_dir);
        if !hit {
            // 不是 awf 產生的腳本：看派工單有沒有提到這個 run 目錄（只看每個 agent 的開頭）
            for name in files.iter().take(6) {
                let Some(head) = read_head(&pyish::pjoin(wf_dir, name)) else { continue };
                if self.needles.iter().any(|n| head.contains(n.as_str()) || head.contains(escaped(n).as_str())) {
                    hit = true;
                    break;
                }
            }
        }
        // 還沒有任何 agent 檔、也沒有 meta 的目錄先不下結論（下次再看）
        if hit || !files.is_empty() {
            self.matched.insert(wf_dir.to_string(), hit);
        }
        Ok(hit)
    }

    /// 這個 run 的 workflow 清單（快照的 `workflows`）。任何一個 workflow 讀不懂只記在 `broken`，其他照常。
    pub fn view(&mut self, now: &Ts) -> Vec<Value> {
        let Some(root) = self.root.clone() else { return Vec::new() };
        if !is_dir(&root) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for wf_dir in cost::find_wf_dirs(&[root]) {
            match self.one(&wf_dir, now) {
                Ok(Some(w)) => out.push(w),
                Ok(None) => {}
                // 格式變了：這個 workflow 少了細節，其他照常
                Err(e) => self.broken = format!("transcript 讀不懂（{}：{}）；事件會少 token 與模型", e.class, e.msg),
            }
        }
        out
    }

    fn one(&mut self, wf_dir: &str, now: &Ts) -> R<Option<Value>> {
        if !self.mine(wf_dir)? {
            return Ok(None);
        }
        let meta = cost::read_wf_meta(wf_dir).map_err(crash)?;
        let m_get = |m: &Map<String, Value>, k: &str| s_or_empty(m.get(k).unwrap_or(&NULL));
        let mut agents = Vec::new();
        let mut lasts: Vec<Ts> = Vec::new();
        let mut total: i64 = 0;
        for name in agent_files(wf_dir) {
            let path = pyish::pjoin(wf_dir, &name);
            // 0.x 的 Python 版把狀態原地累加：第一次讀就丟例外時狀態不留，之後才丟的留著已經讀進去的部分
            let existed = self.tails.contains_key(&path);
            let mut st = self.tails.remove(&path).unwrap_or_default();
            let read = cost::tail_agent(&path, &mut st);
            let (model, tokens, requests, last) = (st.model.clone(), st.tokens, st.requests, st.last);
            if existed || read.is_ok() {
                self.tails.insert(path, st);
            }
            read.map_err(crash)?;
            let base = name.get(..name.len() - ".jsonl".len()).unwrap_or_default();
            let m = cost::read_meta(&pyish::pjoin(wf_dir, &format!("{base}.meta.json")));
            let mut last_text = String::new();
            if let Some(t) = last {
                lasts.push(t);
                // 沒帶時區的時間 Python 照本機時區換算；這裡不算，反正後面和「現在」相減時就會丟 TypeError
                if t.offset.is_some() {
                    last_text = utc_stamp(&t)?;
                }
            }
            let role = m_get(&m, "agentType");
            total = total.saturating_add(tokens);
            agents.push(json!({
                "id": base.get("agent-".len()..).unwrap_or_default(),
                "role": role.rsplit(':').next().unwrap_or_default(),
                "label": pyish::take(&m_get(&m, "description"), 120),
                "phase": m_get(&m, "workflowPhase"),
                "model": if model.is_empty() { m_get(&m, "model") } else { model },
                "tokens": tokens, "requests": requests, "last": last_text,
            }));
        }
        let status = m_get(&meta, "status");
        let mut idle = Value::Null;
        let mut newest: Option<Ts> = None;
        for t in &lasts {
            newest = match newest {
                Some(cur) if t.cmp(&cur).map_err(crash)? != std::cmp::Ordering::Greater => Some(cur),
                _ => Some(*t),
            };
        }
        if let Some(t) = newest {
            let secs = now.micros_since(&t).map_err(crash)? / 1_000_000;
            idle = json!(secs.max(0));
        }
        let workflow = match meta.get("workflowName").filter(|v| truthy(v)) {
            Some(v) => py::s(v),
            None => stem(&m_get(&meta, "scriptPath")),
        };
        let finished = FINISHED.contains(&crate::hooks::py::lower(&status).as_str());
        Ok(Some(json!({"wf_run": py::pname(wf_dir), "workflow": workflow, "status": status, "finished": finished,
                       "idle_seconds": idle, "tokens": total, "agents": agents})))
    }
}

/// 檔案開頭的 60,000 個字（`open(encoding="utf-8", errors="replace").read(60_000)`）；讀不到回 None。
fn read_head(path: &str) -> Option<String> {
    use std::io::Read;
    let fh = std::fs::File::open(path).ok()?;
    let mut raw = Vec::new();
    fh.take(60_000 * 4 + 4).read_to_end(&mut raw).ok()?;
    let t = pyutil::universal_newlines(&String::from_utf8_lossy(&raw));
    Some(pyish::take(&t, 60_000).to_string())
}

/// `json.dumps(n)[1:-1]`
fn escaped(n: &str) -> String {
    let t = json_dumps(&Value::String(n.to_string()), true);
    t.get(1..t.len().saturating_sub(1)).unwrap_or_default().to_string()
}

/// 一份快照；run.json 讀不到回 None。
pub fn snapshot(ctx: &Ctx, run: &str, tr: Option<&mut Transcripts>, now: &Ts) -> R<Option<Value>> {
    let Some(mut view) = run_view(ctx, run)? else { return Ok(None) };
    if let (Some(tr), Some(o)) = (tr, view.as_object_mut()) {
        o.insert("workflows".into(), Value::Array(tr.view(now)));
        if !tr.broken.is_empty() {
            o.insert("note".into(), json!(tr.broken));
        }
    }
    Ok(Some(view))
}

// ------------------------------------------------------------------ 事件：前後兩份快照的差異

fn event(kind: &str, fields: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(kind));
    for (k, v) in fields {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

/// `d.setdefault(k, default)`
fn setdefault<'a>(d: &'a mut Value, k: &str, default: Value) -> R<&'a mut Value> {
    match d {
        Value::Object(o) => Ok(o.entry(k.to_string()).or_insert(default)),
        other => Err(py::attr_err(other, "setdefault")),
    }
}

/// `d[k] = v`
fn set(d: &mut Value, k: &str, v: Value) -> R<()> {
    py::obj_mut(d)?.insert(k.to_string(), v);
    Ok(())
}

/// （新事件, 更新後的記憶）。prev 是上一次留下的記憶（第一次給 `{}`）；同一件事只會記一次。
/// 記憶檔任何人都寫得到：形狀不對時回 Python 會丟的例外，由 [`tick`] 重置後重算。
pub fn diff_events(prev: &Value, cur: &Value, stall_seconds: i64) -> R<(Vec<Value>, Value)> {
    let mut mem = if truthy(prev) { prev.clone() } else { json!({}) };
    let mut ev: Vec<Value> = Vec::new();
    let state = idx(cur, "state")?;
    if !pyfmt::eq(get(&mem, "state")?, state) {
        ev.push(event("run_state", &[("state", state.clone())]));
        set(&mut mem, "state", state.clone())?;
    }
    let cur_pending = get(cur, "pending")?;
    let of_pending = |k: &str| -> R<Value> {
        let v = if truthy(cur_pending) { get(cur_pending, k)? } else { &NULL };
        Ok(if truthy(v) { v.clone() } else { json!("") })
    };
    let pend = of_pending("stage")?;
    if truthy(&pend) && !pyfmt::eq(get(&mem, "pending")?, &pend) {
        ev.push(event("stage_started", &[("stage", pend.clone()), ("workflow", of_pending("workflow")?)]));
    }
    set(&mut mem, "pending", pend)?;
    {
        let ms = setdefault(&mut mem, "stages", json!({}))?;
        for s in py::iter(idx(cur, "stages")?)? {
            let name = py::s(idx(&s, "name")?);
            let status = idx(&s, "status")?;
            let recs = idx(&s, "records")?;
            let approved = idx(&s, "approved")?;
            let old = match get(ms, &name)? {
                v if truthy(v) => v.clone(),
                _ => json!({}),
            };
            if !pyfmt::eq(get(&old, "status")?, status) && !matches!(status.as_str(), Some("pending" | "skipped")) {
                ev.push(event("stage_saved", &[("stage", json!(name)), ("status", status.clone())]));
            }
            let old_recs = get(&old, "records")?;
            for label in py::iter(recs)? {
                if !truthy(old_recs) || !py::contains(old_recs, text(&label))? {
                    ev.push(event("record_written", &[("stage", json!(name)), ("label", label.clone())]));
                }
            }
            if truthy(approved) && !truthy(get(&old, "approved")?) {
                ev.push(event("stage_approved", &[("stage", json!(name)), ("by", idx(approved, "by")?.clone()), ("kind", idx(approved, "kind")?.clone())]));
            }
            set(ms, &name, json!({"status": status, "records": recs, "approved": truthy(approved)}))?;
        }
    }
    let gate = get(cur, "waiting_gate")?;
    if truthy(gate) && !pyfmt::eq(get(&mem, "waiting_gate")?, gate) {
        ev.push(event("waiting_gate", &[("stage", gate.clone())]));
    }
    set(&mut mem, "waiting_gate", gate.clone())?;
    {
        let mw = setdefault(&mut mem, "workflows", json!({}))?;
        for w in py::iter_or(get(cur, "workflows")?)? {
            let wf_run = py::s(idx(&w, "wf_run")?);
            let (workflow, tokens, finished) = (idx(&w, "workflow")?, idx(&w, "tokens")?, truthy(idx(&w, "finished")?));
            let ow = setdefault(mw, &wf_run, json!({"agents": {}, "finished": false, "stalled": false}))?;
            for a in py::iter(idx(&w, "agents")?)? {
                let id = py::s(idx(&a, "id")?);
                let a_tokens = idx(&a, "tokens")?;
                let oa = get(idx(ow, "agents")?, &id)?.clone();
                let base = [
                    ("wf_run", json!(wf_run)), ("workflow", workflow.clone()), ("label", idx(&a, "label")?.clone()), ("role", idx(&a, "role")?.clone()),
                    ("model", idx(&a, "model")?.clone()), ("tokens", a_tokens.clone()), ("requests", idx(&a, "requests")?.clone()),
                ];
                if oa.is_null() {
                    ev.push(event("agent_started", &base));
                    if let Some(ags) = ow.get_mut("agents") {
                        set(ags, &id, json!({"tokens": a_tokens}))?;
                    }
                } else if py_int(a_tokens)? - py_int(get(&oa, "tokens")?)? >= STEP.with(std::cell::Cell::get) {
                    ev.push(event("agent_progress", &base));
                    if let Some(slot) = ow.get_mut("agents").and_then(|ags| ags.get_mut(&id)) {
                        set(slot, "tokens", a_tokens.clone())?;
                    }
                }
            }
            if finished && !truthy(idx(ow, "finished")?) {
                ev.push(event("workflow_finished", &[("wf_run", json!(wf_run)), ("workflow", workflow.clone()), ("status", idx(&w, "status")?.clone()), ("tokens", tokens.clone())]));
                set(ow, "finished", json!(true))?;
            }
            let idle = get(&w, "idle_seconds")?;
            if !finished && !idle.is_null() && stall_seconds > 0 && py_int(idle)? >= i128::from(stall_seconds) {
                if !truthy(idx(ow, "stalled")?) {
                    ev.push(event("workflow_stalled", &[("wf_run", json!(wf_run)), ("workflow", workflow.clone()), ("idle_seconds", idle.clone()), ("tokens", tokens.clone())]));
                    set(ow, "stalled", json!(true))?;
                }
            } else {
                set(ow, "stalled", json!(false))?; // 又有動靜了：之後再停住會再記一次
            }
        }
    }
    let note = match get(cur, "note")? {
        v if truthy(v) => py::s(v),
        _ => String::new(),
    };
    if !note.is_empty() && !pyfmt::eq(get(&mem, "note")?, &json!(note)) {
        ev.push(event("watch_note", &[("status", json!(pyish::take(&note, 200)))]));
    }
    set(&mut mem, "note", json!(note))?;
    Ok((ev, mem))
}

// ------------------------------------------------------------------ notify（預設不設）

/// profile 的 `progress.notify` 整理過的樣子。
#[derive(Debug, Clone, PartialEq)]
pub struct Notify {
    pub argv: Vec<String>,
    /// 只送這些類型；空的＝全部。
    pub events: Vec<String>,
}

impl Notify {
    pub fn to_value(&self) -> Value {
        json!({"argv": self.argv, "events": self.events})
    }
}

/// profile 沒設（或設了空的指令）回 None。
pub fn notify_config(profile: &Value) -> R<Option<Notify>> {
    let empty = json!({});
    let prof = if truthy(profile) { profile } else { &empty };
    let progress = get(prof, "progress")?;
    if !progress.is_object() {
        return Ok(None);
    }
    let n = get(progress, "notify")?;
    if !n.is_object() || !truthy(get(n, "command")?) {
        return Ok(None);
    }
    let argv: Vec<String> = match get(n, "command")? {
        Value::Array(a) => a.iter().map(py::s).collect(),
        other => shlex_split(&py::s(other)).map_err(|_| PyErr::other("ValueError", "No closing quotation"))?,
    };
    let events: Vec<String> = py::iter_or(get(n, "events")?)?.iter().filter_map(Value::as_str).filter(|e| EVENT_TYPES.contains(e)).map(str::to_string).collect();
    Ok(if argv.is_empty() { None } else { Some(Notify { argv, events }) })
}

fn safe_text(t: &str) -> bool {
    SAFE_TEXT.find(t) == Some((0, t.len()))
}

/// 要送出去的內容：只留白名單欄位；任何欄位看起來像機敏值就整筆不送（回 None）。
pub fn notify_payload(event: &Value) -> Option<Value> {
    let src = event.as_object()?;
    let mut out = Map::new();
    for k in NOTIFY_FIELDS {
        if let Some(v) = src.get(k) {
            out.insert(k.to_string(), v.clone());
        }
    }
    // 先看原始內容：像機敏值就整筆不送
    if SECRETISH.is_match(&py::dumps(&Value::Object(out.clone()), None)) {
        return None;
    }
    // 自由文字不送：watch_note 的 status 是例外訊息（可能帶路徑）；label 是 agent 取的名字，只留單純的字元
    if out.get("type").and_then(Value::as_str) == Some("watch_note") {
        out.shift_remove("status");
    }
    for k in ["label", "status", "stage", "workflow", "role", "model", "by", "kind", "state"] {
        if let Some(v) = out.get_mut(k) {
            if !v.as_str().is_some_and(safe_text) {
                *v = json!(SKIPPED);
            }
        }
    }
    let out = Value::Object(out);
    if SECRETISH.is_match(&py::dumps(&out, None)) { None } else { Some(out) }
}

/// 真的跑外部指令：內容從 stdin 給，最多等 10 秒。
pub fn real_runner(argv: &[String], input: &str) -> Result<i32, String> {
    let Some(prog) = argv.first() else { return Err("IndexError".into()) };
    if argv.iter().any(|a| a.contains('\0')) {
        return Err("ValueError".into());
    }
    let mut cmd = std::process::Command::new(prog);
    cmd.args(argv.get(1..).unwrap_or_default());
    match pyutil::run(&mut cmd, Some(input.as_bytes().to_vec()), Some(std::time::Duration::from_secs(10))) {
        Ok(o) => Ok(o.code),
        Err(pyutil::RunErr::Timeout) => Err("TimeoutExpired".into()),
        Err(pyutil::RunErr::Os(e)) => Err(pyish::os_class(&e).to_string()),
    }
}

/// 回空字串＝送了或不用送；其他是警告文字（不擋流程）。
pub fn notify(cfg: Option<&Notify>, event: &Value, runner: Runner) -> String {
    let Some(cfg) = cfg else { return String::new() };
    if !cfg.events.is_empty() && !event.get("type").and_then(Value::as_str).is_some_and(|t| cfg.events.iter().any(|e| e == t)) {
        return String::new();
    }
    let Some(payload) = notify_payload(event) else {
        return "notify：這筆事件看起來有機敏值，沒有送出".into();
    };
    match runner(&cfg.argv, &py::dumps(&payload, None)) {
        Ok(0) => String::new(),
        Ok(code) => format!("notify 指令 exit {code}"),
        Err(class) => format!("notify 指令跑不起來（{class}）"),
    }
}

// ------------------------------------------------------------------ 寫事件

/// [`tick`] 的結果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tick {
    /// 快照；run.json 讀不到是 None。
    pub snap: Option<Value>,
    /// 這次新增的事件。
    pub events: Vec<Value>,
    pub warns: Vec<String>,
}

fn fresh() -> Value {
    json!({"v": 1, "seq": 0, "mem": {}})
}

fn seq_ok(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.as_u64().is_some())
}

/// 看一次。事件附加到 `<run>/progress.jsonl`，記憶存在 `<run>/.watch-state.json`。`now` 用來算停了多久，`at` 是事件的時間戳。
#[allow(clippy::too_many_arguments)]
pub fn tick(ctx: &Ctx, run: &str, tr: Option<&mut Transcripts>, stall_seconds: i64, cfg: Option<&Notify>, now: &Ts, at: &str, runner: Runner) -> R<Tick> {
    let Some(snap) = snapshot(ctx, run, tr, now)? else {
        return Ok(Tick { snap: None, events: Vec::new(), warns: vec![format!("讀不到 {}", pj(run, "run.json"))] });
    };
    let mut mem = match load_json(ctx, &pj(run, STATE)) {
        Some(m) if m.is_object() && pyfmt::eq(f(&m, "v"), &json!(1)) => m,
        _ => fresh(),
    };
    let mut warns: Vec<String> = Vec::new();
    let inner_ok = |m: &Value| !truthy(f(m, "mem")) || f(m, "mem").is_object();
    if !seq_ok(f(&mem, "seq")) || !inner_ok(&mem) {
        warns.push(format!("{STATE} 的內容不對，重新開始記（事件序號從 1 起）"));
        mem = fresh();
    }
    let prev = if truthy(f(&mem, "mem")) { f(&mem, "mem").clone() } else { json!({}) };
    let (events, new_mem) = match diff_events(&prev, &snap, stall_seconds) {
        Ok(r) => r,
        Err(e) => {
            // 記憶檔被改壞（任何人都寫得到）：重置後重算，不讓 watcher 中斷
            warns.push(format!("{STATE} 讀不懂（{}），重新開始記", e.class));
            mem = fresh();
            diff_events(&json!({}), &snap, stall_seconds)?
        }
    };
    let mut seq = f(&mem, "seq").as_u64().unwrap_or(0);
    let mut lines: Vec<Value> = Vec::new();
    for e in &events {
        seq = seq.saturating_add(1);
        let mut m = Map::new();
        m.insert("v".into(), json!(1));
        m.insert("schema_version".into(), json!(PROGRESS_SCHEMA_VERSION));
        m.insert("seq".into(), json!(seq));
        m.insert("at".into(), json!(at));
        m.insert("run".into(), f(&snap, "run").clone());
        for (k, v) in e.as_object().into_iter().flatten() {
            m.insert(k.clone(), v.clone());
        }
        lines.push(Value::Object(m));
    }
    if !lines.is_empty() {
        set(&mut mem, "seq", json!(seq))?;
        let path = pj(run, PROGRESS);
        let body: String = lines.iter().map(|e| format!("{}\n", py::dumps(e, None))).collect();
        let wrote = std::fs::OpenOptions::new().append(true).create(true).open(ctx.abs(&path)).and_then(|mut fh| fh.write_all(body.as_bytes()));
        if let Err(e) = wrote {
            return Ok(Tick { snap: Some(snap), events: Vec::new(), warns: vec![format!("寫不出 {PROGRESS}（{}）", PyErr::os(&e, &path).msg)] });
        }
    }
    set(&mut mem, "mem", new_mem)?;
    if let Err(e) = ctx.write_text(&pj(run, STATE), &py::dumps(&mem, None)) {
        warns.push(format!("寫不出 {STATE}（{}）；下次可能重複記事件", e.msg));
    }
    for e in &lines {
        let w = notify(cfg, e, runner);
        if !w.is_empty() && !warns.contains(&w) {
            warns.push(w);
        }
    }
    Ok(Tick { snap: Some(snap), events: lines, warns })
}

/// 快照的文字版（`watch --once` 印的）。
pub fn text_of(snap: &Value) -> String {
    let opt_part = |k: &str, pre: &str, post: &str| if truthy(f(snap, k)) { format!("{pre}{}{post}", py::s(f(snap, k))) } else { String::new() };
    let mut lines = vec![format!("{}：{}{}{}", py::s(f(snap, "run")), py::s(f(snap, "state")), opt_part("waiting_gate", "；等 ", " 段核准"), opt_part("next", "；下一段 ", ""))];
    for s in f(snap, "stages").as_array().into_iter().flatten() {
        let status = f(s, "status");
        if matches!(status.as_str(), Some("pending" | "skipped")) {
            continue;
        }
        let ap = if truthy(f(s, "approved")) {
            format!("，{} 核准", py::s(f(f(s, "approved"), "by")))
        } else if truthy(f(s, "gate")) && status.as_str() == Some("done") {
            "，等核准".to_string()
        } else {
            String::new()
        };
        let (agents, missing) = (int_of(f(s, "agents")), int_of(f(s, "agents_missing")));
        let n_recs = f(s, "records").as_array().map_or(0, Vec::len);
        lines.push(format!("  {}：{}{ap}；agent {}/{agents}；紀錄 {n_recs} 份", py::s(f(s, "name")), py::s(status), agents - missing));
    }
    for w in f(snap, "workflows").as_array().into_iter().flatten() {
        let idle = if truthy(f(w, "finished")) || f(w, "idle_seconds").is_null() { String::new() } else { format!("；{} 秒沒有動靜", py::s(f(w, "idle_seconds"))) };
        let name = if truthy(f(w, "workflow")) { f(w, "workflow") } else { f(w, "wf_run") };
        let status = if truthy(f(w, "status")) { py::s(f(w, "status")) } else { "執行中".to_string() };
        let n_agents = f(w, "agents").as_array().map_or(0, Vec::len);
        lines.push(format!("  workflow {}：{status}；{n_agents} 個 agent、{} token{idle}", py::s(name), commas(int_of(f(w, "tokens")))));
    }
    if truthy(f(snap, "note")) {
        lines.push(format!("  注意：{}", py::s(f(snap, "note"))));
    }
    lines.join("\n")
}

/// 一行狀態：最近動過的 run。任何問題都回空字串（狀態列不能壞）。
pub fn status_line(ctx: &Ctx, project: &str) -> String {
    status_line_inner(ctx, project).unwrap_or_default()
}

fn mtime(p: &str) -> Option<f64> {
    let d = std::fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as f64 + 1e-9 * f64::from(d.subsec_nanos()))
}

fn status_line_inner(ctx: &Ctx, project: &str) -> R<String> {
    let cfg = match load_json(ctx, &pj(project, ".claude/teams/guard.config.json")) {
        Some(c) if truthy(&c) => c,
        _ => json!({}),
    };
    let runs = pj(project, &py::s_or(get(&cfg, "runs_dir")?, "_tpm/runs"));
    let runs_abs = ctx.abs(&runs);
    let mut best: Option<(f64, String)> = None;
    for name in names_in(&runs_abs) {
        let p = pyish::pjoin(&pyish::pjoin(&runs_abs, &name), "run.json");
        if !is_file(&p) {
            continue;
        }
        let Some(t) = mtime(&p) else { return Ok(String::new()) };
        if best.as_ref().is_none_or(|(b, _)| t > *b) {
            best = Some((t, name));
        }
    }
    let Some((_, name)) = best else { return Ok(String::new()) };
    let Some(v) = run_view(ctx, &pj(&runs, &name))? else { return Ok(String::new()) };
    let state = text(f(&v, "state"));
    if matches!(state, "closed" | "abandoned") {
        return Ok(String::new());
    }
    let cur = [f(&v, "waiting_gate"), f(f(&v, "pending"), "stage"), f(&v, "next")].into_iter().find(|x| truthy(x)).map(py::s).unwrap_or_default();
    let st = f(&v, "stages").as_array().into_iter().flatten().find(|s| text(f(s, "name")) == cur);
    let part = match st {
        Some(s) if truthy(f(s, "agents")) => format!(" · {}/{}", int_of(f(s, "agents")) - int_of(f(s, "agents_missing")), int_of(f(s, "agents"))),
        _ => String::new(),
    };
    let tail = if truthy(f(&v, "waiting_gate")) { " · 等核准" } else if state == "blocked" { " · 停下了" } else { "" };
    Ok(format!("{name} · {}{part}{tail}", if cur.is_empty() { state } else { &cur }))
}

// ------------------------------------------------------------------ CLI

/// `main(argv, project)`。`argv` 是 `watch` 後面的參數，或以 `statusline` 開頭。
pub fn run(env: &mut Env, argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run_inner(env, argv, out, err) {
        Ok(code) => code,
        Err(e) => {
            // 0.x 的 Python 版在這裡是沒接住的例外（traceback、exit 1）
            let _ = writeln!(err, "awf watch：{}：{}", e.class, e.msg);
            1
        }
    }
}

fn run_inner(env: &mut Env, argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> R<i32> {
    let has = |name: &str| argv.iter().any(|a| a == name);
    if has("--selftest") {
        let mut text = String::new();
        let code = selftest(&env.ctx, &mut text);
        let _ = out.write_all(text.as_bytes());
        return code;
    }
    if argv.first().is_some_and(|a| a == "statusline") {
        let line = status_line(&env.ctx, &env.project);
        if !line.is_empty() {
            let _ = writeln!(out, "{line}");
        }
        return Ok(0);
    }
    let run_arg = opt(argv, "--run", "");
    if run_arg.is_empty() {
        let _ = writeln!(out, "{USAGE}");
        return Ok(1);
    }
    let run = pj(&env.project, run_arg);
    // 節奏的預設讀 profile（progress.watch）；沒設定就是原本的 900 與 5，命令列旗標有給以旗標為準
    let (prof, _) = save_run::load_profile_for(&env.ctx, &[]);
    let wt = WatchTune::of(&prof);
    set_progress_step(wt.progress_step);
    let (d_stall, d_interval) = (wt.stall_seconds.to_string(), wt.interval.to_string());
    let (Some(stall), Some(interval)) = (pyutil::int_of_str(opt(argv, "--stall-seconds", &d_stall)), pyutil::int_of_str(opt(argv, "--interval", &d_interval))) else {
        let _ = writeln!(err, "--stall-seconds 與 --interval 要是整數");
        return Ok(1);
    };
    let stall = stall.clamp(0, i128::from(i64::MAX)) as i64;
    let interval = interval.clamp(1, i128::from(u32::MAX)) as u64;
    let root = match &env.transcripts {
        Some(t) => Some(t.clone()),
        None => cost::project_dir(&env.project).ok(),
    };
    let mut tr = Transcripts::new(root.as_deref(), &env.ctx.abs(&run));
    let cfg = notify_config(&prof)?;
    let once = has("--once");
    loop {
        let (now, at) = env.clock();
        let mut real = real_runner;
        let runner: Runner = match env.runner.as_mut() {
            Some(r) => r.as_mut(),
            None => &mut real,
        };
        let t = tick(&env.ctx, &run, Some(&mut tr), stall, cfg.as_ref(), &now, &at, runner)?;
        for w in &t.warns {
            let _ = writeln!(err, "警告：{w}");
        }
        let Some(snap) = t.snap else { return Ok(1) };
        if once {
            let _ = writeln!(out, "{}", if has("--json") { py::dumps(&snap, Some(2)) } else { text_of(&snap) });
            return Ok(0);
        }
        for e in &t.events {
            let _ = writeln!(out, "{}", py::dumps(e, None));
        }
        let _ = out.flush();
        if matches!(text(f(&snap, "state")), "closed" | "abandoned") {
            return Ok(0);
        }
        match env.sleep.as_mut() {
            Some(sleep) => {
                if !sleep(interval) {
                    return Ok(0);
                }
            }
            None => std::thread::sleep(std::time::Duration::from_secs(interval)),
        }
    }
}

/// `awf watch …`、`awf statusline`（`args[0]` 是子指令名）。
pub fn main(args: &[String]) -> i32 {
    let argv = match args.first().map(String::as_str) {
        Some("watch") => args.get(1..).unwrap_or_default(),
        _ => args,
    };
    let mut env = Env::real();
    run(&mut env, argv, &mut std::io::stdout(), &mut std::io::stderr())
}

// ------------------------------------------------------------------ selftest

struct Tester<'a> {
    out: &'a mut String,
    ok: bool,
}

impl Tester<'_> {
    fn check(&mut self, name: &str, cond: bool, detail: String) {
        self.ok = self.ok && cond;
        self.out.push_str(&format!("  [{}] {name}{}\n", if cond { "通過" } else { "失敗" }, if cond { String::new() } else { format!("：{detail}") }));
    }
}

/// 暫存目錄裡把每種事件、停住、重啟、notify、狀態列走一遍。`ctx.teams` 要有 schema（在 `scaffold/` 或已 init 的專案根目錄跑）。
/// schema 讀不到時 0.x 的 Python 版是沒接住的例外，這裡回錯誤。
pub fn selftest(ctx: &Ctx, out: &mut String) -> R<i32> {
    out.push_str("awf_watch selftest（暫存目錄，不碰 repo）\n");
    let schema = ctx.load_json(&pj(&ctx.teams, "schemas/progress-event.schema.json"))?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("awf-watch.{}-{nanos}", std::process::id()));
    let root_text = root.to_string_lossy().replace('\\', "/");
    let mut t = Tester { out, ok: true };
    let done = std::fs::create_dir_all(&root).map_err(|e| PyErr::os(&e, &root_text)).and_then(|_| selftest_body(ctx, &schema, &root_text, &mut t));
    let _ = std::fs::remove_dir_all(&root);
    let ok = t.ok;
    done?;
    out.push_str(&format!("awf_watch selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    Ok(if ok { 0 } else { 1 })
}

fn write(path: &str, body: &str) -> R<()> {
    if let Some(i) = path.rfind('/') {
        pyish::mkdir_p(path.get(..i).unwrap_or_default())?;
    }
    std::fs::write(path, body.as_bytes()).map_err(|e| PyErr::os(&e, path))
}

fn append(path: &str, body: &str) -> R<()> {
    std::fs::OpenOptions::new().append(true).create(true).open(path).and_then(|mut fh| fh.write_all(body.as_bytes())).map_err(|e| PyErr::os(&e, path))
}

/// selftest 的時間：2026-01-01T00:00:00Z 加幾秒（不跨日）。
fn ts(sec: i64) -> String {
    format!("2026-01-01T{:02}:{:02}:{:02}.000Z", sec / 3600, sec % 3600 / 60, sec % 60)
}

fn no_runner(_: &[String], _: &str) -> Result<i32, String> {
    Ok(0)
}

fn selftest_body(tool: &Ctx, schema: &Value, root: &str, t: &mut Tester) -> R<()> {
    let ctx = Ctx::new(&pj(root, "proj"), root);
    let _ = tool;
    let run = pj(root, "proj/_runs/20260101-sdlc-demo");
    pyish::mkdir_p(&run)?;
    let trd = pj(root, "transcripts");
    let wf = pj(&trd, "sess-1/subagents/workflows/wf_a1");
    pyish::mkdir_p(&wf)?;
    pyish::mkdir_p(&pj(&trd, "sess-1/workflows"))?;
    let at = |sec: i64| parse_now(&ts(sec)).unwrap_or_else(real_now);

    let write_run = |stages: Value, extra: Option<(&str, Value)>| -> R<()> {
        let mut doc = json!({"event": "sdlc", "requirement": "機密需求內容", "stages": stages});
        if let (Some((k, v)), Some(o)) = (extra, doc.as_object_mut()) {
            o.insert(k.to_string(), v);
        }
        write(&pj(&run, "run.json"), &py::dumps(&doc, None))
    };
    let agent = |aid: &str, role: &str, label: &str| -> R<()> {
        let line = json!({"timestamp": ts(0), "message": {"role": "user", "content": format!("run 目錄：{run}")}});
        write(&pj(&wf, &format!("agent-{aid}.jsonl")), &format!("{}\n", json_dumps(&line, true)))?;
        let meta = json!({"agentType": format!("agent-workflow:{role}"), "description": label, "workflowPhase": "Research"});
        write(&pj(&wf, &format!("agent-{aid}.meta.json")), &json_dumps(&meta, true))
    };
    // transcript 只會往後附加
    let more = |aid: &str, rid: &str, sec: i64, tok: i64| -> R<()> {
        let line = json!({"timestamp": ts(sec), "requestId": rid, "message": {"model": "claude-sonnet-5-5", "usage": {"input_tokens": tok, "output_tokens": 0}}});
        append(&pj(&wf, &format!("agent-{aid}.jsonl")), &format!("{}\n", json_dumps(&line, true)))
    };
    let meta = |status: &str| -> R<()> {
        let doc = json!({"runId": "wf_a1", "scriptPath": pj(&run, ".wf/sdlc-plan.js"), "status": status, "workflowName": "sdlc-plan"});
        write(&pj(&trd, "sess-1/workflows/wf_a1.json"), &json_dumps(&doc, true))
    };
    let p = |extra: Value| -> Value {
        let mut m = json!({"status": "pending", "gate": true});
        if let (Some(o), Some(x)) = (m.as_object_mut(), extra.as_object()) {
            for (k, v) in x {
                o.insert(k.clone(), v.clone());
            }
        }
        m
    };
    let labels = |ev: &[Value]| -> Vec<String> {
        ev.iter()
            .map(|e| {
                let tag = [f(e, "label"), f(e, "stage"), f(e, "state")].into_iter().find(|x| truthy(x)).map(py::s).unwrap_or_default();
                format!("{}:{tag}", text(f(e, "type")))
            })
            .collect()
    };
    let mut tr = Transcripts::new(Some(&trd), &run);
    let mut seq_all: Vec<Value> = Vec::new();
    let step = |tr: &mut Transcripts, seq_all: &mut Vec<Value>, sec: i64| -> R<Vec<String>> {
        let r = tick(&ctx, &run, Some(tr), 600, None, &at(sec), &ts(sec), &mut no_runner)?;
        seq_all.extend(r.events.iter().cloned());
        Ok(labels(&r.events))
    };
    let last = |seq_all: &[Value]| seq_all.last().cloned().unwrap_or(Value::Null);
    let none: Vec<String> = Vec::new();

    write_run(json!({"plan": p(json!({})), "build": p(json!({"gate": false}))}), Some(("awf", json!({"pending": {"stage": "plan", "workflow": "sdlc-plan"}}))))?;
    let got = step(&mut tr, &mut seq_all, 1)?;
    t.check("事件：run 開始、plan 段開跑", got == ["run_state:running", "stage_started:plan"], format!("{got:?}"));
    let got = step(&mut tr, &mut seq_all, 2)?;
    t.check("事件：沒有變化就不重複記", got == none, format!("{got:?}"));
    meta("running")?;
    agent("r1", "tpm-researcher", "research:code")?;
    more("r1", "q1", 10, 30_000)?;
    let got = step(&mut tr, &mut seq_all, 20)?;
    let e = last(&seq_all);
    t.check(
        "事件：transcript 出現 agent → agent_started，帶角色、模型與 token",
        got == ["agent_started:research:code"] && text(f(&e, "role")) == "tpm-researcher" && text(f(&e, "model")) == "claude-sonnet-5-5" && f(&e, "tokens") == &json!(30_000),
        format!("{got:?} {e}"),
    );
    more("r1", "q2", 30, 40_000)?;
    let got = step(&mut tr, &mut seq_all, 40)?;
    t.check("事件：token 增加不到門檻不記 agent_progress", got == none, format!("{got:?}"));
    more("r1", "q3", 50, 60_000)?;
    more("r1", "q3", 50, 60_000)?;
    let got = step(&mut tr, &mut seq_all, 60)?;
    let e = last(&seq_all);
    t.check("事件：token 超過門檻記一筆 agent_progress（同一個 requestId 不重複算）", got == ["agent_progress:research:code"] && f(&e, "tokens") == &json!(130_000), format!("{got:?} {e}"));
    write(&pj(&run, "plan/research-code.md"), "# 紀錄\n")?;
    write(&pj(&run, &format!("plan/{}", save_run::INDEX_NAME)), "# 索引\n")?;
    write(&pj(&run, "plan/pack.md"), "# 資料包\n")?;
    let got = step(&mut tr, &mut seq_all, 70)?;
    t.check("事件：紀錄檔出現 → record_written（索引與資料包不算）", got == ["record_written:research-code"], format!("{got:?}"));
    let got = step(&mut tr, &mut seq_all, 50 + 599)?;
    t.check("停住：還沒超過門檻不記", got == none, format!("{got:?}"));
    let got = step(&mut tr, &mut seq_all, 50 + 601)?;
    let e = last(&seq_all);
    let again = step(&mut tr, &mut seq_all, 50 + 700)?;
    t.check(
        "停住：workflow 沒結束而且超過門檻沒有動靜 → workflow_stalled（只記一次）",
        got == ["workflow_stalled:"] && f(&e, "idle_seconds") == &json!(601) && again == none,
        format!("{got:?} {e}"),
    );
    more("r1", "q4", 900, 1_000)?;
    step(&mut tr, &mut seq_all, 905)?;
    let got = step(&mut tr, &mut seq_all, 900 + 601)?;
    t.check("停住：恢復動靜之後又停住，會再記一次", got == ["workflow_stalled:"], format!("{got:?}"));
    meta("completed")?;
    write_run(json!({"plan": p(json!({"status": "done"})), "build": p(json!({"gate": false}))}), None)?;
    let got = step(&mut tr, &mut seq_all, 1600)?;
    t.check("事件：workflow 結束、段存檔、等核准", got == ["run_state:waiting_gate", "stage_saved:plan", "waiting_gate:plan", "workflow_finished:"], format!("{got:?}"));
    let got = step(&mut tr, &mut seq_all, 9000)?;
    t.check("停住：workflow 結束之後不再記 stalled", got == none, format!("{got:?}"));
    write_run(json!({"plan": p(json!({"status": "done", "approved": {"by": "human", "at": "t", "kind": "human"}})), "build": p(json!({"gate": false}))}), None)?;
    let got = step(&mut tr, &mut seq_all, 9100)?;
    t.check("事件：核准", got == ["run_state:running", "stage_approved:plan"] && text(f(&last(&seq_all), "by")) == "human", format!("{got:?}"));
    let progress = ctx.read_text(&pj(&run, PROGRESS))?;
    let lines: Vec<Value> = pyutil::splitlines(&progress).into_iter().map(py::loads).collect::<R<Vec<Value>>>()?;
    let seqs_ok = lines.iter().enumerate().all(|(i, e)| f(e, "seq") == &json!(i + 1));
    let invalid: Vec<Vec<String>> = lines.iter().map(|e| crate::schema::validate(e, schema, "$")).filter(|v| !v.is_empty()).take(2).collect();
    t.check("progress.jsonl：一行一個事件、seq 連續、每筆都符合 schema", seqs_ok && lines.len() == seq_all.len() && invalid.is_empty(), format!("{invalid:?}"));
    t.check("progress.jsonl：沒有需求內容、摘要或路徑", !progress.contains("機密需求內容") && !progress.contains(run.as_str()), String::new());
    let mut tr2 = Transcripts::new(Some(&trd), &run);
    let r = tick(&ctx, &run, Some(&mut tr2), 600, None, &at(9200), &ts(9200), &mut no_runner)?;
    let snap = r.snap.clone().unwrap_or(Value::Null);
    let wf0 = f(&snap, "workflows").get(0).cloned().unwrap_or(Value::Null);
    t.check("重啟 watcher：記憶在 .watch-state.json，不會把舊事件再記一次", r.events.is_empty() && f(&wf0, "tokens") == &json!(131_000), format!("{:?}", r.events));
    let stage0 = f(&snap, "stages").get(0).cloned().unwrap_or(Value::Null);
    let agent0 = f(&wf0, "agents").get(0).cloned().unwrap_or(Value::Null);
    t.check(
        "快照：有狀態、各段、紀錄與 workflow 的 agent",
        text(f(&snap, "state")) == "running" && f(&stage0, "records") == &json!(["research-code"]) && text(f(&agent0, "role")) == "tpm-researcher"
            && format!("{}{}", text_of(&snap), py::dumps(&snap, None)).contains("research:code"),
        snap.to_string(),
    );

    // transcript 讀不懂、不存在
    write(&pj(&wf, "agent-bad.jsonl"), "這不是 JSON\n{\"message\": 3}\n[1,2]\n")?;
    let r = tick(&ctx, &run, Some(&mut Transcripts::new(Some(&trd), &run)), 600, None, &at(0), &ts(0), &mut no_runner)?;
    let snap = r.snap.clone().unwrap_or(Value::Null);
    let has_bad = f(f(&snap, "workflows").get(0).unwrap_or(&NULL), "agents").as_array().into_iter().flatten().any(|a| text(f(a, "id")) == "bad" && f(a, "tokens") == &json!(0));
    t.check("transcript 有讀不懂的行：不中斷，那個 agent 只是沒有 token", r.snap.is_some() && has_bad, snap.to_string());
    let r = tick(&ctx, &run, Some(&mut Transcripts::new(Some(&pj(root, "nope")), &run)), 600, None, &at(0), &ts(0), &mut no_runner)?;
    let snap = r.snap.clone().unwrap_or(Value::Null);
    t.check("找不到 transcript 目錄：照常出 run 的狀態，workflows 是空的", r.snap.is_some() && f(&snap, "workflows") == &json!([]) && text(f(&snap, "state")) == "running", snap.to_string());
    let other = pj(&trd, "sess-1/subagents/workflows/wf_other");
    let line = json!({"timestamp": ts(0), "message": {"role": "user", "content": "別的 run：/somewhere/else"}});
    write(&pj(&other, "agent-x.jsonl"), &format!("{}\n", json_dumps(&line, true)))?;
    let snap = snapshot(&ctx, &run, Some(&mut Transcripts::new(Some(&trd), &run)), &at(0))?.unwrap_or(Value::Null);
    let names: Vec<&str> = f(&snap, "workflows").as_array().into_iter().flatten().map(|w| text(f(w, "wf_run"))).collect();
    t.check("別的 run 的 workflow 不算進來", names == ["wf_a1"], format!("{names:?}"));
    let (now, stamp) = (real_now(), utc_stamp(&real_now()).unwrap_or_default());
    let r = tick(&ctx, &pj(root, "nope"), None, 600, None, &now, &stamp, &mut no_runner)?;
    t.check("run.json 讀不到：回錯誤、不寫任何檔", r.snap.is_none() && !std::path::Path::new(&pj(root, "nope")).exists(), String::new());

    // notify
    let line0 = lines.first().cloned().unwrap_or(Value::Null);
    t.check(
        "notify：profile 沒設就不呼叫（預設關）",
        notify_config(&Value::Null)?.is_none() && notify_config(&json!({"progress": {}}))?.is_none() && notify(None, &line0, &mut no_runner).is_empty(),
        String::new(),
    );
    let mut sent: Vec<(Vec<String>, Value)> = Vec::new();
    let cfg = notify_config(&json!({"progress": {"notify": {"command": ["notify-cmd", "--x"], "events": ["workflow_stalled", "waiting_gate", "nope"]}}}))?;
    let cfg_events = cfg.as_ref().map(|c| c.events.clone()).unwrap_or_default();
    {
        let mut fake = |argv: &[String], input: &str| -> Result<i32, String> {
            sent.push((argv.to_vec(), py::loads(input).unwrap_or(Value::Null)));
            Ok(0)
        };
        for e in &lines {
            let mut e2 = e.clone();
            if let Some(o) = e2.as_object_mut() {
                o.insert("requirement".into(), json!("機密需求內容"));
                o.insert("summary".into(), json!("內部摘要"));
                o.insert("path".into(), json!("/secret/path"));
            }
            notify(cfg.as_ref(), &e2, &mut fake);
        }
    }
    let mut kinds: Vec<&str> = sent.iter().map(|(_, pl)| text(f(pl, "type"))).collect();
    kinds.sort_unstable();
    kinds.dedup();
    t.check(
        "notify：只送指定的事件類型；不認得的類型忽略",
        cfg_events == ["workflow_stalled", "waiting_gate"] && kinds == ["waiting_gate", "workflow_stalled"] && sent.first().is_some_and(|(a, _)| a == &["notify-cmd", "--x"]),
        format!("{:?}", sent.iter().take(2).collect::<Vec<_>>()),
    );
    let all_sent = py::dumps(&Value::Array(sent.iter().map(|(a, pl)| json!([a, pl])).collect()), None);
    let only_listed = sent.iter().all(|(_, pl)| pl.as_object().is_some_and(|o| o.keys().all(|k| NOTIFY_FIELDS.contains(&k.as_str()))));
    t.check("notify：只送白名單欄位（需求、摘要、路徑都不送）", only_listed && !all_sent.contains("機密") && !all_sent.contains("/secret"), String::new());
    let n0 = sent.len();
    let secret_label = format!("token={}{}", "ghp_", "a".repeat(30));
    let w = {
        let mut fake = |argv: &[String], input: &str| -> Result<i32, String> {
            sent.push((argv.to_vec(), py::loads(input).unwrap_or(Value::Null)));
            Ok(0)
        };
        notify(cfg.as_ref(), &json!({"type": "waiting_gate", "v": 1, "seq": 1, "at": "t", "run": "r", "stage": "plan", "label": secret_label}), &mut fake)
    };
    t.check("notify：內容像機敏值就整筆不送，並回警告", sent.len() == n0 && w.contains("沒有送出"), w.clone());
    let pl = notify_payload(&json!({"type": "watch_note", "v": 1, "seq": 2, "at": "t", "run": "r", "status": "OSError: /srv/demo/repo/x.json"})).unwrap_or(Value::Null);
    let pl2 = notify_payload(&json!({"type": "record_written", "v": 1, "seq": 3, "at": "t", "run": "r", "stage": "plan", "label": "../../etc/passwd"})).unwrap_or(Value::Null);
    let pl3 = notify_payload(&json!({"type": "record_written", "v": 1, "seq": 4, "at": "t", "run": "r", "stage": "plan", "label": "research:code#2"})).unwrap_or(Value::Null);
    t.check(
        "notify：watch_note 的訊息不送；label 含路徑字元就不送原文；正常的 label 照送",
        pl.is_object() && pl.get("status").is_none() && text(f(&pl2, "label")) == SKIPPED && text(f(&pl3, "label")) == "research:code#2",
        format!("{pl} {pl2} {pl3}"),
    );
    let bad_run = pj(root, "bad-state");
    write(&pj(&bad_run, "run.json"), &ctx.read_text(&pj(&run, "run.json"))?)?;
    write(&pj(&bad_run, STATE), "{\"v\":1,\"seq\":\"x\",\"mem\":{\"stages\":[]}}")?;
    let mut crashed = false;
    let warns_of = |crashed: &mut bool| -> Vec<String> {
        let (now, stamp) = (real_now(), utc_stamp(&real_now()).unwrap_or_default());
        match tick(&ctx, &bad_run, None, 900, None, &now, &stamp, &mut no_runner) {
            Ok(r) => r.warns,
            Err(e) => {
                *crashed = true;
                vec![format!("{}：{}", e.class, e.msg)]
            }
        }
    };
    let w_bad = warns_of(&mut crashed);
    write(&pj(&bad_run, STATE), "{\"v\":1,\"seq\":3,\"mem\":{\"stages\":[]}}")?;
    let w_bad2 = warns_of(&mut crashed);
    t.check(
        "記憶檔被改壞：不中斷，重新開始記並警告",
        !crashed && w_bad.iter().any(|x| x.contains("重新開始記")) && w_bad2.iter().any(|x| x.contains("重新開始記")),
        format!("{w_bad:?} {w_bad2:?}"),
    );
    let mut boom = |_: &[String], _: &str| -> Result<i32, String> { Err("OSError".into()) };
    let mut gate_event = line0.clone();
    if let Some(o) = gate_event.as_object_mut() {
        o.insert("type".into(), json!("waiting_gate"));
    }
    t.check("notify：指令跑不起來只回警告（不丟例外）", notify(cfg.as_ref(), &gate_event, &mut boom).contains("跑不起來"), String::new());
    let from_text = notify_config(&json!({"progress": {"notify": {"command": "notify-cmd --x"}}}))?;
    t.check("notify：command 可以寫字串", from_text.is_some_and(|c| c.argv == ["notify-cmd", "--x"]), String::new());

    // statusline
    let proj = pj(root, "proj");
    write(&pj(&proj, ".claude/teams/guard.config.json"), &json_dumps(&json!({"runs_dir": "_runs"}), true))?;
    write_run(json!({"plan": p(json!({"status": "done"})), "build": p(json!({"gate": false}))}), None)?;
    let line = status_line(&ctx, &proj);
    t.check("statusline：一行、有 run 名與在等的段", line == "20260101-sdlc-demo · plan · 等核准", line.clone());
    t.check("statusline：沒有 run、設定壞掉都回空字串（不丟例外）", status_line(&ctx, &pj(root, "nope")).is_empty() && status_line(&ctx, root).is_empty(), String::new());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(SECRETISH.compiles());
        assert!(SAFE_TEXT.compiles());
    }

    #[test]
    fn safe_text_is_fullmatch() {
        assert!(safe_text(""));
        assert!(safe_text("research:code#2"));
        assert!(safe_text("調研 一"));
        assert!(!safe_text("a/b"));
        assert!(!safe_text("a\n"));
        assert!(!safe_text("a\"b"));
        assert!(safe_text(&"a".repeat(80)));
        assert!(!safe_text(&"a".repeat(81)));
    }

    #[test]
    fn stamp_and_commas() {
        let t = parse_now("2026-01-01T08:00:01+08:00").expect("帶時區");
        assert_eq!(utc_stamp(&t).expect("範圍內"), "2026-01-01T00:00:01Z");
        assert!(parse_now("2026-01-01T00:00:00").is_none());
        assert_eq!(commas(131_000), "131,000");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_234_567), "1,234,567");
        assert_eq!(stem("/a/b/sdlc-plan.js"), "sdlc-plan");
        assert_eq!(stem(".hidden"), ".hidden");
    }
}
