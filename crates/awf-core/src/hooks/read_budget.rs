//! 讀取預算：模版角色讀大檔要分段、每個 agent 有讀取額度。原本是 0.x 的 Python 工具 `read_budget.py`。
//!
//! 輔助功能，不是信任邊界：[`check_read`]、[`check_shell`] 任何失敗都回 None（放行）——狀態檔壞掉（重置）、沒有權限、
//! 路徑怪異、檔案不存在、目錄、二進位檔都放行。機敏檔等既有檢查由守門規則先做，不受這裡影響。
//!
//! 狀態檔：`<系統暫存目錄>/awf-read-budget/<session_id>/<agent_id>.json`（`AWF_READ_BUDGET_DIR` 可覆寫根目錄），
//! 內容 `{"used": 位元組, "warned": 是否已提醒過 80%, "reads": 次數}`，原子寫入。

use super::path;
use super::py;
use crate::config::{self, Config, Limits};
use crate::pyfmt;
use serde_json::{json, Map, Value};
use std::io::{Read, Write};

const READ_DEFAULT_LINES: i64 = 2000;
const BIG_FILE_BYTES: u64 = 5 * 1024 * 1024;
const BINARY_PROBE: usize = 8192;
const SAMPLE_BYTES: u64 = 64 * 1024;
const WARN_RATIO: f64 = 0.8;
const STALE_DAYS: u64 = 7;
const CLEANUP_PROBES: usize = 3;
const WARN_LOG_MAX: u64 = 1024 * 1024;
const MAX_SHELL_LEN: usize = 4096;
const MAX_SINGLE_READ: i64 = 100 * 1024;
const GREP_DEFAULT_LINES: i64 = 250;
const GREP_LINE_BYTES: i64 = 150;
const SHELL_GREP_BYTES: i64 = 16 * 1024;
const SKIP_EXT: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".tif", ".tiff", ".heic", ".avif", ".svgz", ".pdf", ".ipynb"];
const ENV_DIR: &str = "AWF_READ_BUDGET_DIR";

fn hint(n: u64) -> String {
    format!("先用 Grep 或 `awf kb query \"<問題>\"` 找到行段，再帶 offset／limit 讀，一次最多 {n} 行（設定：.claude/teams/methodology.json 的 limits）")
}

/// 模版角色回傳角色名（套過 ROLE_ALIASES）；主 session、使用者自己的 agent、擴充 agent 回空字串。
fn role(data: &Value, cfg: &Config) -> String {
    let Some(raw) = data.get("agent_type").and_then(Value::as_str) else { return String::new() };
    if raw.is_empty() || !config::is_team_agent(raw, cfg) {
        return String::new();
    }
    if !config::RESERVED_ROLES.contains(&config::role_name(raw)) && !config::extension_role(raw, cfg).is_empty() {
        return String::new(); // 登記在 extension_agents 的使用者 agent：不套讀取預算
    }
    config::team_role(raw, cfg)
}

fn to_int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if n.is_u64() {
                Some(i64::MAX)
            } else {
                let f = n.as_f64()?;
                if f.is_finite() && f == f.trunc() && f.abs() < 9e18 { Some(f as i64) } else { None }
            }
        }
        Value::String(s) => {
            let t = py::strip(s);
            if !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()) { Some(t.parse::<i64>().unwrap_or(i64::MAX)) } else { None }
        }
        _ => None,
    }
}

fn resolve(raw: &str, data: &Value, ws: &str) -> String {
    let mut p = raw.to_string();
    if cfg!(windows) {
        p = super::guard::norm(&p); // 反斜線換成 /，Git Bash 的 /c/… 換成 c:/…
    }
    let p = path::pp(&p);
    if path::is_abs(&p) {
        return p;
    }
    let base = match data.get("cwd") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => ws.to_string(),
    };
    path::join(&base, &p)
}

struct Info {
    size: i64,
    /// None＝檔案超過 BIG_FILE_BYTES（不逐行算）。
    lines: Option<i64>,
    avg: f64,
}

/// `Err`＝讀不到（OSError）；`Ok(None)`＝不是一般檔、二進位檔、圖片／PDF／notebook（不套用規則）。
fn inspect(p: &str) -> Result<Option<Info>, ()> {
    if SKIP_EXT.contains(&py::lower(&path::suffix(p)).as_str()) {
        return Ok(None);
    }
    if p.contains('\0') {
        return Err(());
    }
    let md = std::fs::metadata(p).map_err(|_| ())?;
    if !md.is_file() {
        return Ok(None);
    }
    let size = md.len();
    if size == 0 {
        return Ok(Some(Info { size: 0, lines: Some(0), avg: 0.0 }));
    }
    let cap = if size > BIG_FILE_BYTES { SAMPLE_BYTES } else { BIG_FILE_BYTES + 1 };
    let mut buf = Vec::new();
    std::fs::File::open(p).map_err(|_| ())?.take(cap).read_to_end(&mut buf).map_err(|_| ())?;
    if buf.iter().take(BINARY_PROBE).any(|b| *b == 0) {
        return Ok(None);
    }
    let nl = buf.iter().filter(|b| **b == b'\n').count() as i64;
    let len = buf.len() as i64;
    if size > BIG_FILE_BYTES || buf.len() as u64 > BIG_FILE_BYTES {
        return Ok(Some(Info { size: size as i64, lines: None, avg: len as f64 / nl.max(1) as f64 }));
    }
    let lines = nl + if buf.last() == Some(&b'\n') { 0 } else { 1 };
    Ok(Some(Info { size: len, lines: Some(lines), avg: len as f64 / lines.max(1) as f64 }))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// 把 session_id／agent_id 清成安全的檔名：只留英數、底線、連字號。
fn safe(name: Option<&Value>, fallback: &str) -> String {
    let raw = match name {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        _ => fallback,
    };
    let cleaned: String = raw.chars().take(80).map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let mut out = if cleaned.is_empty() { "_".to_string() } else { cleaned };
    if out != raw {
        // 清過的名字加上原字串的雜湊，避免不同 id 清完撞名
        out.push_str(&format!("-{:08x}", crc32(raw.as_bytes())));
    }
    let low = out.to_ascii_lowercase();
    let b = low.as_bytes();
    let reserved = matches!(low.as_str(), "con" | "prn" | "aux" | "nul")
        || (b.len() == 4 && (low.starts_with("com") || low.starts_with("lpt")) && b[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

pub fn state_root() -> String {
    match std::env::var_os(ENV_DIR).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty()) {
        Some(v) => path::pp(&v),
        None => path::join(&path::gettempdir(), "awf-read-budget"),
    }
}

fn state_path(data: &Value) -> String {
    let agent = match data.get("agent_id") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => format!("type-{}", match data.get("agent_type") {
            Some(v) if pyfmt::truthy(v) => pyfmt::str_of(v),
            _ => String::new(),
        }),
    };
    let dir = path::join(&state_root(), &safe(data.get("session_id"), "no-session"));
    path::join(&dir, &format!("{}.json", safe(Some(&Value::String(agent)), "no-agent")))
}

struct State {
    used: i64,
    warned: bool,
    reads: i64,
}

/// 回傳（狀態, 是不是新的）。內容壞掉就重置。
fn load_state(p: &str) -> (State, bool) {
    let fresh = || State { used: 0, warned: false, reads: 0 };
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) => return (fresh(), e.kind() == std::io::ErrorKind::NotFound),
    };
    let Some(st) = String::from_utf8(bytes).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return (fresh(), false) };
    let used = match st.get("used") {
        Some(Value::Number(n)) if st.is_object() => n.as_i64().filter(|u| *u >= 0),
        _ => None,
    };
    let warned = st.get("warned").and_then(Value::as_bool);
    let (Some(used), Some(warned)) = (used, warned) else { return (fresh(), false) };
    let reads = st.get("reads").and_then(Value::as_i64).unwrap_or(0);
    (State { used, warned, reads }, false)
}

fn mkdir_private(dir: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(dir).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path::is_dir(dir) => Ok(()),
        Err(e) => Err(e),
    }
}

fn save_state(p: &str, st: &State) -> std::io::Result<()> {
    mkdir_private(&path::parent(p))?;
    let tmp = format!("{}.{}.tmp", p, std::process::id());
    std::fs::write(&tmp, py::json_dumps(&json!({"used": st.used, "warned": st.warned, "reads": st.reads}), true))?;
    std::fs::rename(&tmp, p)
}

/// 清掉超過 STALE_DAYS 天沒動的 session 目錄。每次隨機看幾個；只刪自己寫的 .json／.tmp，目錄裡有別的東西就留著。失敗一律忽略。
fn cleanup(root: &str) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut names: Vec<std::path::PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    let now = std::time::SystemTime::now();
    let mut seed = now.duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64 ^ d.as_secs()).unwrap_or(1) | 1;
    for _ in 0..CLEANUP_PROBES.min(names.len()) {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let idx = ((seed >> 33) as usize) % names.len();
        let d = names.swap_remove(idx);
        let Ok(md) = std::fs::symlink_metadata(&d) else { continue };
        if !md.is_dir() {
            continue;
        }
        let stale = md.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age.as_secs() > STALE_DAYS * 86_400);
        if !stale {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&d) else { continue };
        let mut failed = false;
        for f in files.filter_map(|e| e.ok()).take(200) {
            let n = f.file_name().to_string_lossy().into_owned();
            if (n.ends_with(".json") || n.ends_with(".tmp")) && std::fs::remove_file(f.path()).is_err() {
                failed = true;
                break;
            }
        }
        if !failed {
            let _ = std::fs::remove_dir(&d);
        }
    }
}

/// mode=warn：把「本來會擋」記一行 JSON 到 <狀態目錄>/warn.log。寫不進去就算了。
fn warn_log(data: &Value, role: &str, rule: &str, extra: Vec<(&str, Value)>) {
    let root = state_root();
    if mkdir_private(&root).is_err() {
        return;
    }
    let log = path::join(&root, "warn.log");
    if std::fs::metadata(&log).map(|m| m.len() > WARN_LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(&log, path::join(&root, "warn.log.1"));
    }
    let mut rec = Map::new();
    rec.insert("ts".into(), json!(py::utc_stamp()));
    rec.insert("rule".into(), json!(rule));
    rec.insert("role".into(), json!(role));
    for k in ["agent_type", "agent_id", "session_id"] {
        rec.insert(k.into(), data.get(k).cloned().unwrap_or(Value::Null));
    }
    for (k, v) in extra {
        rec.insert(k.into(), v);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&log) {
        let _ = f.write_all(format!("{}\n", py::json_dumps(&Value::Object(rec), false)).as_bytes());
    }
}

fn kb(n: i64) -> String {
    format!("{:.0} KB", n.max(0) as f64 / 1000.0)
}

/// 把這次讀取的估計量記進這個 agent 的額度。回傳擋下的訊息；None＝放行。
fn charge(data: &Value, role: &str, lim: &Limits, est: i64, what: &str) -> Option<String> {
    let budget = lim.roles.iter().find(|(r, _)| r == role).map(|(_, b)| *b).unwrap_or(lim.default_budget);
    if budget == 0 {
        return None;
    }
    let budget = budget.min(i64::MAX as u64) as i64;
    match data.get("agent_id") {
        Some(Value::String(s)) if !s.is_empty() => {}
        _ => return None, // 沒有 agent_id 就分不出是哪一個 agent：不記帳
    }
    let est = est.min(MAX_SINGLE_READ); // Read 一次回傳的內容有上限
    let sp = state_path(data);
    let (mut st, is_new) = load_state(&sp);
    if is_new {
        cleanup(&path::parent(&path::parent(&sp)));
    }
    if st.used >= budget {
        if lim.enforce {
            return Some(format!("{role} 的讀取額度已用完（已讀約 {}，額度 {}）。\
請把目前結論寫進執行紀錄檔並交件；不確定的地方寫進 open_questions。\
（設定：.claude/teams/methodology.json 的 limits.read_budget）", kb(st.used), kb(budget)));
        }
        warn_log(data, role, "budget_exhausted", vec![("file", json!(what)), ("used", json!(st.used)), ("budget", json!(budget))]);
    } else if !st.warned && (st.used.saturating_add(est)) as f64 >= budget as f64 * WARN_RATIO {
        st.warned = true;
        if lim.enforce {
            save_state(&sp, &st).ok()?; // 這次被擋的讀取不計入額度
            return Some(format!("{role} 的讀取額度已用 80%（已讀約 {}，這次約 {}，額度 {}，\
剩約 {}）。這次讀取重試一次就會放行；請開始收斂、把已知結論寫進執行紀錄檔。\
（這個提醒每個 agent 只出現一次）", kb(st.used), kb(est), kb(budget), kb(budget - st.used)));
        }
        warn_log(data, role, "budget_80", vec![("file", json!(what)), ("used", json!(st.used)), ("estimate", json!(est)), ("budget", json!(budget))]);
    }
    st.used = st.used.saturating_add(est);
    st.reads = st.reads.saturating_add(1);
    let _ = save_state(&sp, &st);
    None
}

fn ti_of(data: &Value) -> Option<Map<String, Value>> {
    match data.get("tool_input") {
        Some(Value::Object(o)) => Some(o.clone()),
        Some(v) if pyfmt::truthy(v) => None,
        _ => Some(Map::new()),
    }
}

fn check_grep(data: &Value, cfg: &Config, ws: &str) -> Option<String> {
    let ti = ti_of(data)?;
    if ti.get("output_mode").and_then(Value::as_str) != Some("content") {
        return None; // 只列檔名或計數：輸出很小，不計
    }
    let role = role(data, cfg);
    if role.is_empty() {
        return None;
    }
    let lim = config::limits(ws)?;
    let lines = match to_int(ti.get("head_limit")) {
        Some(h) if h > 0 => h.min(GREP_DEFAULT_LINES * 8),
        _ => GREP_DEFAULT_LINES,
    };
    let mut ctx: i64 = 0;
    for k in ["-A", "-B"] {
        ctx = ctx.saturating_add(to_int(ti.get(k)).filter(|v| *v > 0).unwrap_or(0));
    }
    for k in ["-C", "context"] {
        ctx = ctx.saturating_add(to_int(ti.get(k)).filter(|v| *v > 0).map(|v| v.saturating_mul(2)).unwrap_or(0));
    }
    let est = (lines * (1 + ctx.min(20)) * GREP_LINE_BYTES).min(MAX_SINGLE_READ);
    let pat = match ti.get("pattern") {
        Some(v) if pyfmt::truthy(v) => pyfmt::str_of(v),
        _ => String::new(),
    };
    charge(data, &role, &lim, est, &format!("Grep content：{}", py::head(&pat, 60)))
}

fn check_read_inner(data: &Value, cfg: &Config, ws: &str) -> Option<String> {
    if data.get("tool_name").and_then(Value::as_str) != Some("Read") {
        return None;
    }
    let role = role(data, cfg);
    if role.is_empty() {
        return None;
    }
    let lim = config::limits(ws)?;
    let ti = ti_of(data)?;
    let fp = ti.get("file_path").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let info = inspect(&resolve(fp, data, ws)).ok()??;
    let offset = to_int(ti.get("offset")).filter(|v| *v != 0).unwrap_or(1).max(1);
    let limit = to_int(ti.get("limit")).filter(|v| *v > 0);
    let size = info.size;
    let est_part = |want: i64| -> i64 { size.min((want as f64 * info.avg) as i64 + 1) };
    let (want, est, total_txt, total_json) = match info.lines {
        None => {
            // 超過 5MB：不知道總行數
            let want = limit.unwrap_or(READ_DEFAULT_LINES).min(READ_DEFAULT_LINES);
            (want, est_part(want), format!("超過 {}MB，行數未計", BIG_FILE_BYTES / (1024 * 1024)), Value::Null)
        }
        Some(total) => {
            let remaining = (total.saturating_sub(offset).saturating_add(1)).max(0);
            let want = limit.unwrap_or(remaining).min(remaining).min(READ_DEFAULT_LINES);
            let est = if want >= total { size } else { est_part(want) };
            (want, est, format!("共 {total} 行"), json!(total))
        }
    };
    let n = lim.whole_file_lines;
    if n != 0 && want as u64 > n && want > 0 {
        if lim.enforce {
            return Some(format!("{role} 讀 {fp}：這次要讀 {want} 行，超過一次讀取的門檻 {n} 行（檔案{total_txt}）。{}", hint(n)));
        }
        warn_log(data, &role, "whole_file", vec![("file", json!(fp)), ("lines", json!(want)), ("total_lines", total_json), ("threshold", json!(n))]);
    }
    charge(data, &role, &lim, est, fp)
}

/// Read／Grep：回傳擋下的訊息；None＝放行。任何失敗都放行。
pub fn check_read(data: &Value, cfg: &Config, ws: &str) -> Option<String> {
    if data.get("tool_name").and_then(Value::as_str) == Some("Grep") {
        return check_grep(data, cfg, ws);
    }
    check_read_inner(data, cfg, ws)
}

fn rb_cmd_name(tok: &str) -> String {
    let t = tok.replace('\\', "/");
    let name = py::lower(t.rsplit('/').next().unwrap_or(""));
    match name.strip_suffix(".exe") {
        Some(n) => n.to_string(),
        None => name,
    }
}

/// PowerShell 的簡單切詞：空白分隔，單雙引號包住的算一個（不處理跳脫；引號沒關回 None）。
fn ps_split(cmd: &str) -> Option<Vec<String>> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut quote = '\0';
    let mut started = false;
    for ch in cmd.chars() {
        if quote != '\0' {
            if ch == quote {
                quote = '\0';
            } else {
                cur.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = ch;
            started = true;
        } else if py::is_space(ch) {
            if started {
                toks.push(std::mem::take(&mut cur));
                started = false;
            }
        } else {
            cur.push(ch);
            started = true;
        }
    }
    if quote != '\0' {
        return None;
    }
    if started {
        toks.push(cur);
    }
    Some(toks)
}

fn grep_quiet(t: &str) -> bool {
    if matches!(t, "--quiet" | "--silent" | "--files-with-matches" | "--files-without-match" | "--count" | "--count-matches" | "--files") {
        return true;
    }
    match t.strip_prefix('-') {
        Some(b) => !b.is_empty() && b.bytes().all(|c| c.is_ascii_alphabetic()) && b.bytes().any(|c| matches!(c, b'q' | b'l' | b'L' | b'c')),
        None => false,
    }
}

/// 單一簡單指令的 grep 類搜尋而且會印出符合的內容。看不懂一律回 false＝不計。
fn is_content_grep(tool: &str, cmd: &str) -> bool {
    if cmd.chars().count() > MAX_SHELL_LEN {
        return false;
    }
    if tool == "PowerShell" {
        if cmd.chars().any(|c| matches!(c, '|' | ';' | '&' | '<' | '>' | '\n' | '\r' | '(' | ')' | '{' | '}' | '$' | '`')) {
            return false;
        }
        let Some(toks) = ps_split(cmd) else { return false };
        let Some(first) = toks.first() else { return false };
        return matches!(rb_cmd_name(first).as_str(), "select-string" | "sls" | "findstr")
            && !toks[1..].iter().any(|t| matches!(py::lower(t).as_str(), "-quiet" | "-list" | "/m"));
    }
    if cmd.chars().any(|c| matches!(c, '|' | ';' | '&' | '<' | '>' | '\n' | '\r' | '`')) || cmd.contains("$(") {
        return false;
    }
    let Ok(toks) = py::shlex_split(cmd) else { return false };
    let Some(first) = toks.first() else { return false };
    matches!(rb_cmd_name(first).as_str(), "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack") && !toks[1..].iter().any(|t| grep_quiet(t))
}

/// 指令是「cat 類整檔印出」時回傳檔案清單；不是、或看不懂，回空清單。
fn cat_targets(tool: &str, cmd: &str) -> Vec<String> {
    let not_simple = |c: char| matches!(c, '|' | ';' | '&' | '<' | '>' | '\n' | '\r' | '(' | ')' | '{' | '}' | '$' | '`' | '*' | '?' | '[' | ']' | '!' | '#');
    if cmd.chars().count() > MAX_SHELL_LEN || cmd.chars().any(not_simple) {
        return Vec::new();
    }
    if tool == "PowerShell" {
        let Some(toks) = ps_split(cmd) else { return Vec::new() };
        let Some(first) = toks.first() else { return Vec::new() };
        if !matches!(rb_cmd_name(first).as_str(), "get-content" | "gc" | "cat" | "type") {
            return Vec::new();
        }
        let mut files = Vec::new();
        let mut i = 1;
        while i < toks.len() {
            let t = &toks[i];
            let low = py::lower(t);
            let value_flag = matches!(low.as_str(), "-path" | "-literalpath" | "-lp");
            if value_flag || low == "-encoding" {
                if i + 1 >= toks.len() {
                    return Vec::new();
                }
                if value_flag {
                    files.push(toks[i + 1].clone());
                }
                i += 2;
                continue;
            }
            if low == "-raw" {
                i += 1;
                continue;
            }
            if t.starts_with('-') || t.contains(',') || t.starts_with('~') {
                return Vec::new(); // 其他參數、陣列、家目錄：放行
            }
            files.push(t.clone());
            i += 1;
        }
        return files;
    }
    let Ok(toks) = py::shlex_split(cmd) else { return Vec::new() };
    let Some(first) = toks.first() else { return Vec::new() };
    if rb_cmd_name(first) != "cat" {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut opts = true;
    for t in &toks[1..] {
        if opts && t == "--" {
            opts = false;
        } else if opts && t.starts_with('-') {
            let b = &t[1..];
            if b.is_empty() || !b.bytes().all(|c| b"nbsAETvetu".contains(&c)) {
                return Vec::new();
            }
        } else if t.starts_with('~') {
            return Vec::new(); // 家目錄展開：看不懂就放行
        } else {
            files.push(t.clone());
        }
    }
    files
}

/// shell 的 cat 類整檔印出與 grep 類搜尋：回傳擋下的訊息；None＝放行。任何失敗都放行。
pub fn check_shell(data: &Value, cfg: &Config, ws: &str) -> Option<String> {
    let tool = data.get("tool_name").and_then(Value::as_str)?;
    let ti = ti_of(data)?;
    let cmd = ti.get("command").and_then(Value::as_str)?;
    if !(tool == "Bash" || tool == "PowerShell") || py::strip(cmd).is_empty() {
        return None;
    }
    let cmd = py::strip(cmd);
    let role = role(data, cfg);
    if role.is_empty() {
        return None;
    }
    let files = cat_targets(tool, cmd);
    if files.is_empty() {
        if is_content_grep(tool, cmd) {
            let lim = config::limits(ws)?;
            return charge(data, &role, &lim, SHELL_GREP_BYTES, &format!("shell：{}", py::head(cmd, 60)));
        }
        return None;
    }
    let lim = config::limits(ws)?;
    let n = lim.whole_file_lines;
    if n == 0 {
        return None;
    }
    for f in &files {
        let Ok(info) = inspect(&resolve(f, data, ws)) else { continue };
        let Some(info) = info else { continue };
        if info.lines.is_some_and(|l| l <= n.min(i64::MAX as u64) as i64) {
            continue;
        }
        let total_txt = match info.lines {
            Some(l) => format!("共 {l} 行"),
            None => format!("超過 {}MB，行數未計", BIG_FILE_BYTES / (1024 * 1024)),
        };
        if !lim.enforce {
            warn_log(data, &role, "shell_whole_file", vec![("file", json!(f)), ("total_lines", info.lines.map(|l| json!(l)).unwrap_or(Value::Null)), ("threshold", json!(n))]);
            return None;
        }
        return Some(format!("{role} 用 shell 整檔印出 {f}：檔案{total_txt}，超過一次讀取的門檻 {n} 行。{}", hint(n)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(safe(Some(&json!("abc-1")), "x"), "abc-1");
        assert_eq!(safe(Some(&json!("a/b")), "x"), format!("a_b-{:08x}", crc32(b"a/b")));
        assert_eq!(safe(None, "no-session"), "no-session");
        assert_eq!(safe(Some(&json!("NUL")), "x"), "_NUL");
        assert_eq!(kb(2500), "2 KB");
        assert_eq!(kb(3500), "4 KB");
        assert_eq!(kb(1499), "1 KB");
    }

    #[test]
    fn shell_parsing() {
        assert_eq!(cat_targets("Bash", "cat -n a.md b.md"), vec!["a.md", "b.md"]);
        assert!(cat_targets("Bash", "cat a.md | head").is_empty());
        assert!(cat_targets("Bash", "head a.md").is_empty());
        assert_eq!(cat_targets("PowerShell", "Get-Content -Path 'a b.md' -Raw"), vec!["a b.md"]);
        assert!(cat_targets("PowerShell", "Get-Content a.md -TotalCount 5").is_empty());
        assert!(is_content_grep("Bash", "grep -n foo src"));
        assert!(!is_content_grep("Bash", "grep -c foo src"));
        assert!(!is_content_grep("Bash", "grep foo src | wc -l"));
        assert!(is_content_grep("PowerShell", "Select-String foo a.md"));
    }
}
