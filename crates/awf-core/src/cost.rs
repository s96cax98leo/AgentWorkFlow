//! workflow 的成本與時間：讀 Claude Code 存的 workflow transcript，依 workflow、角色、模型加總 token。
//! 由 `scaffold/.claude/teams/tools/awf_cost.py` 移植；輸出文字是對跑基準的一部分（`tests/parity/tools.expected.json`）。
//!
//! 資料來源（Claude Code 的內部格式，官方沒有承諾穩定，所以解析集中在 [`read_agent`]、[`tail_agent`]、[`read_wf_meta`]）：
//!   `<session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.jsonl`      每筆 assistant 訊息有 message.usage、message.model
//!   `<session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.meta.json`  agentType、description（＝label）、workflowPhase、model
//!   `<session 目錄>/workflows/wf_<id>.json`                                 workflowName、status、durationMs、scriptPath、agentCount
//! 同一個請求的 usage 會重複出現在多行，依 requestId 去重（沒有就用 message.id）。
//!
//! Python 版在幾種輸入下會直接當掉（traceback、exit 1）：naive 與 aware 的時間戳互比、requestId 是清單或物件、
//! wf 目錄的路徑不到三層。這裡用 [`Crash`] 表示，CLI 印一行說明後同樣 exit 1。
//!
//! 和 Python 版的已知差異（都是極端輸入）：
//! - 超過 64 位元的整數 token 數當成「不是整數」（算 0）；加總超過 i64 時停在上限。
//! - JSON 巢狀超過 200 層的行當成讀不懂（Python 要到約一千層才丟 RecursionError 當掉）。
//! - NaN／Infinity 當成 null（Python 的 NaN 是真值、而且每個都不相等）。
//! - `~使用者名` 開頭的路徑不展開，回 [`Crash`]（Python 會查系統帳號）。
//! - 路徑一律當成 `/` 分隔的字串處理。

use crate::hooks::py::json_dumps;
use crate::kb::pyish::{self, ppath};
use crate::merge_gate::{pyjson, pyutil};
use crate::pyfmt;
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

pub const USAGE: &str = "workflow 的成本與時間：讀 Claude Code 存的 workflow transcript，依 workflow、角色、模型加總 token。

為什麼要有：使用者問「這段花了多少 token 與時間」時只能手算；要省 token 也得先量得到。

資料來源（Claude Code 的內部格式，官方沒有承諾穩定，所以解析集中在 read_agent／read_wf_meta 兩個轉接函式）：
  <session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.jsonl      每筆 assistant 訊息有 message.usage、message.model
  <session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.meta.json  agentType、description（＝label）、workflowPhase、model
  <session 目錄>/workflows/wf_<id>.json                                 workflowName、status、durationMs、scriptPath、agentCount
同一個請求的 usage 會重複出現在多行，依 requestId 去重（沒有就用 message.id）。

用法：
  awf cost <路徑>… [--run-dir <run 目錄>] [--json]
      <路徑> 可以是 wf_<id> 目錄、session 目錄，或 ~/.claude/projects/<專案> 目錄（往下找所有 wf_*）。
      --run-dir：只算派工單提到這個 run 目錄的 workflow（派工單的共同前綴含 run_dir）。
  awf cost --project <專案根目錄> [--run-dir …] [--json]
      由專案路徑推算 ~/.claude/projects/ 底下的目錄。
  awf cost --selftest

讀不懂格式（找不到任何 usage）時明說並 exit 0：這支工具只做估算，不參與任何閘門判定。
";

pub const KINDS: [&str; 4] = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"];
pub const SHORT: [&str; 4] = ["input", "cache_write", "cache_read", "output"];
/// 比對 run_dir 時每個 agent 最多掃的開頭字數。
pub const SCAN_PROMPT_BYTES: usize = 200_000;

/// Python 版會直接當掉的地方；內容是例外的類別與訊息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash(pub String);

impl std::fmt::Display for Crash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ------------------------------------------------------------------ 時間戳（datetime.fromisoformat、isoformat）

/// 一個 `datetime`：`naive` 是「牆上時間」從 0001-01-01 起算的微秒，`offset` 是時區位移（微秒；None＝naive）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ts {
    naive: i64,
    offset: Option<i64>,
}

const DAY_US: i64 = 86_400_000_000;

impl Ts {
    /// `a < b`、`a > b`：naive 和 aware 互比時 Python 丟 TypeError。
    pub fn cmp(&self, other: &Ts) -> Result<Ordering, Crash> {
        match (self.offset, other.offset) {
            (None, None) => Ok(self.naive.cmp(&other.naive)),
            (Some(a), Some(b)) => Ok((self.naive - a).cmp(&(other.naive - b))),
            _ => Err(Crash("TypeError: can't compare offset-naive and offset-aware datetimes".into())),
        }
    }

    /// `(self - other)` 的微秒數。
    fn micros_since(&self, other: &Ts) -> Result<i64, Crash> {
        match (self.offset, other.offset) {
            (None, None) => Ok(self.naive - other.naive),
            (Some(a), Some(b)) => Ok((self.naive - a) - (other.naive - b)),
            _ => Err(Crash("TypeError: can't subtract offset-naive and offset-aware datetimes".into())),
        }
    }

    /// `datetime.isoformat()`
    pub fn isoformat(&self) -> String {
        let days = self.naive.div_euclid(DAY_US);
        let rem = self.naive.rem_euclid(DAY_US);
        let (y, m, d) = pyutil::from_ordinal(days).unwrap_or((1, 1, 1));
        let (secs, us) = (rem / 1_000_000, rem % 1_000_000);
        let mut out = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60);
        if us != 0 {
            out.push_str(&format!(".{us:06}"));
        }
        if let Some(off) = self.offset {
            let (sign, a) = if off < 0 { ('-', -off) } else { ('+', off) };
            let (hh, rest) = (a / 3_600_000_000, a % 3_600_000_000);
            let (mm, rest) = (rest / 60_000_000, rest % 60_000_000);
            out.push_str(&format!("{sign}{hh:02}:{mm:02}"));
            if rest != 0 {
                out.push_str(&format!(":{:02}", rest / 1_000_000));
                if rest % 1_000_000 != 0 {
                    out.push_str(&format!(".{:06}", rest % 1_000_000));
                }
            }
        }
        out
    }
}

/// 位元組；超出範圍回 0（C 字串結尾的 NUL）。
fn byte(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// `parse_digits`：累加到 `var`（不歸零）。
fn digits(b: &[u8], mut p: usize, var: &mut i64, n: usize) -> Option<usize> {
    for _ in 0..n {
        let t = byte(b, p).wrapping_sub(b'0');
        p += 1;
        if t > 9 {
            return None;
        }
        *var = *var * 10 + i64::from(t);
    }
    Some(p)
}

/// `_find_isoformat_datetime_separator`
fn find_separator(b: &[u8]) -> Option<usize> {
    let len = b.len();
    if len == 7 {
        return Some(7);
    }
    if byte(b, 4) == b'-' {
        if byte(b, 5) == b'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && byte(b, 8) == b'-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && byte(b, 10).is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if byte(b, 4) == b'W' {
        let mut idx = 7;
        while idx < len && byte(b, idx).is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// `iso_to_ymd`；回 0001-01-01 起算的序數（可能超出年份範圍，由呼叫端檢查）。
fn iso_week_ordinal(year: i64, week: i64, day: i64) -> Option<i64> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    let jan1 = pyutil::to_ordinal((year as i32, 1, 1));
    let first_weekday = (jan1 + 6) % 7;
    if week <= 0 || week >= 53 {
        let ok = week == 53 && (first_weekday == 3 || (first_weekday == 2 && pyutil::is_leap(year as i32)));
        if !ok {
            return None;
        }
    }
    if day <= 0 || day >= 8 {
        return None;
    }
    let mut week1_monday = jan1 - first_weekday;
    if first_weekday > 3 {
        week1_monday += 7;
    }
    Some(week1_monday + (week - 1) * 7 + day - 1)
}

/// `parse_isoformat_date`；回序數。
fn parse_date(b: &[u8], sep: usize) -> Option<i64> {
    let mut year = 0;
    let mut cur = digits(b, 0, &mut year, 4)?;
    let uses_sep = byte(b, cur) == b'-';
    if uses_sep {
        cur += 1;
    }
    if byte(b, cur) == b'W' {
        let (mut week, mut day) = (0, 0);
        cur = digits(b, cur + 1, &mut week, 2)?;
        if sep > cur {
            if uses_sep {
                let c = byte(b, cur);
                cur += 1;
                if c != b'-' {
                    return None;
                }
            }
            digits(b, cur, &mut day, 1)?;
        } else {
            day = 1;
        }
        let ord = iso_week_ordinal(year, week, day)?;
        return pyutil::from_ordinal(ord).map(|_| ord);
    }
    let (mut month, mut day) = (0, 0);
    cur = digits(b, cur, &mut month, 2)?;
    if uses_sep {
        let c = byte(b, cur);
        cur += 1;
        if c != b'-' {
            return None;
        }
    }
    digits(b, cur, &mut day, 2)?;
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let (y, m) = (year as i32, month as u32);
    if day < 1 || day > i64::from(pyutil::days_in_month(y, m)) {
        return None;
    }
    Some(pyutil::to_ordinal((y, m, day as u32)))
}

/// `parse_hh_mm_ss_ff`：回（後面還有東西, [時, 分, 秒, 微秒]）；格式錯回 None。
fn parse_hms(b: &[u8], start: usize, end: usize) -> Option<(bool, [i64; 4])> {
    let mut vals = [0i64; 4];
    let mut p = start;
    let mut has_sep = true;
    for i in 0..3 {
        let mut v = 0;
        p = digits(b, p, &mut v, 2)?;
        if let Some(slot) = vals.get_mut(i) {
            *slot = v;
        }
        let c = byte(b, p);
        p += 1;
        if i == 0 {
            has_sep = c == b':';
        }
        if p >= end {
            return Some((c != 0, vals));
        } else if has_sep && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            break;
        } else if !has_sep {
            p -= 1;
        } else {
            return None;
        }
    }
    let to_parse = end.saturating_sub(p).min(6);
    let mut us = 0;
    p = digits(b, p, &mut us, to_parse)?;
    const CORRECTION: [i64; 5] = [100_000, 10_000, 1_000, 100, 10];
    if to_parse < 6 {
        us *= CORRECTION.get(to_parse.wrapping_sub(1)).copied().unwrap_or(1);
    }
    vals[3] = us;
    while byte(b, p).is_ascii_digit() {
        p += 1;
    }
    Some((byte(b, p) != 0, vals))
}

/// `datetime.fromisoformat(s)`（CPython 3.12 的行為，連它的寬鬆處都照做）；讀不懂回 None。
pub fn fromisoformat(s: &str) -> Option<Ts> {
    let b = s.as_bytes();
    if b.len() < 4 {
        return None;
    }
    let sep = find_separator(b)?;
    let ord = parse_date(b, sep)?;
    let mut t = [0i64; 4];
    let mut offset = None;
    if b.len() > sep {
        // 分隔字元可以是任何一個字（UTF-8 的長度看第一個位元組）
        let first = byte(b, sep);
        let p = sep + if first & 0x80 == 0 { 1 } else { match first & 0xf0 { 0xe0 => 3, 0xf0 => 4, _ => 2 } };
        let p_end = b.len();
        let mut tz = p;
        loop {
            if matches!(byte(b, tz), b'Z' | b'+' | b'-') {
                break;
            }
            tz += 1;
            if tz >= p_end {
                break;
            }
        }
        let (more, vals) = parse_hms(b, p, tz)?;
        t = vals;
        if tz >= p_end {
            if more {
                return None;
            }
        } else if byte(b, tz) == b'Z' {
            if byte(b, tz + 1) != 0 {
                return None;
            }
            offset = Some(0);
        } else {
            let sign = if byte(b, tz) == b'-' { -1 } else { 1 };
            let (more, z) = parse_hms(b, tz + 1, p_end)?;
            if more {
                return None;
            }
            // 秒數是 0 時 CPython 直接回 UTC，位移裡的微秒被丟掉
            let secs = z[0] * 3600 + z[1] * 60 + z[2];
            let off = if secs == 0 { 0 } else { sign * (secs * 1_000_000 + z[3]) };
            if off <= -DAY_US || off >= DAY_US {
                return None;
            }
            offset = Some(off);
        }
    }
    if t[0] > 23 || t[1] > 59 || t[2] > 59 {
        return None;
    }
    Some(Ts { naive: ord * DAY_US + (t[0] * 3600 + t[1] * 60 + t[2]) * 1_000_000 + t[3], offset })
}

/// `parse_ts(s)`：`datetime.fromisoformat(str(s).replace("Z", "+00:00"))`。
pub fn parse_ts(v: Option<&Value>) -> Option<Ts> {
    fromisoformat(&pyfmt::str_opt(v).replace('Z', "+00:00"))
}

fn earlier(cur: Option<Ts>, t: Ts) -> Result<Option<Ts>, Crash> {
    Ok(match cur {
        Some(c) if t.cmp(&c)? != Ordering::Less => Some(c),
        _ => Some(t),
    })
}

fn later(cur: Option<Ts>, t: Ts) -> Result<Option<Ts>, Crash> {
    Ok(match cur {
        Some(c) if t.cmp(&c)? != Ordering::Greater => Some(c),
        _ => Some(t),
    })
}

// ------------------------------------------------------------------ 讀 transcript

/// 可以放進 set／dict 的值（Python 的雜湊相等：True == 1 == 1.0）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Null,
    Str(String),
    Int(i128),
    Float(u64),
}

/// 清單與物件不能雜湊（Python 丟 TypeError）。
fn key_of(v: &Value) -> Result<Key, Crash> {
    match v {
        Value::Null => Ok(Key::Null),
        Value::Bool(b) => Ok(Key::Int(i128::from(*b))),
        Value::String(s) => Ok(Key::Str(s.clone())),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Key::Int(i128::from(i)))
            } else if let Some(u) = n.as_u64() {
                Ok(Key::Int(i128::from(u)))
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if f.fract() == 0.0 && f.abs() < 1e30 {
                    Ok(Key::Int(f as i128))
                } else {
                    Ok(Key::Float(f.to_bits()))
                }
            }
        }
        Value::Array(_) => Err(Crash("TypeError: unhashable type: 'list'".into())),
        Value::Object(_) => Err(Crash("TypeError: unhashable type: 'dict'".into())),
    }
}

/// `v if isinstance(v, int) else 0`（bool 也是 int）。
fn int_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|_| i64::MAX)),
        _ => None,
    }
}

/// `e.get("requestId") or m.get("id")`；回 None 表示 Python 的 None（不去重）。
fn request_id<'a>(e: &'a Map<String, Value>, m: &'a Map<String, Value>) -> Option<&'a Value> {
    match e.get("requestId") {
        Some(v) if pyfmt::truthy(v) => Some(v),
        _ => m.get("id").filter(|v| !v.is_null()),
    }
}

/// 一行 → JSON 物件；讀不懂或不是物件回 None。
fn parse_line(line: &str) -> Option<Map<String, Value>> {
    match pyjson::loads_lenient(line) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// 有 usage 的 message。
fn usage_of(e: &Map<String, Value>) -> Option<(&Map<String, Value>, &Map<String, Value>)> {
    let m = e.get("message")?.as_object()?;
    Some((m, m.get("usage")?.as_object()?))
}

/// 一個 agent 的 transcript 讀出來的東西。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentData {
    /// 依 [`KINDS`] 的順序。
    pub tokens: [i64; 4],
    pub requests: i64,
    /// 模型名與次數（依第一次出現的順序）。
    pub models: Vec<(String, i64)>,
    pub first: Option<Ts>,
    pub last: Option<Ts>,
    /// 檔案開頭（最多 [`SCAN_PROMPT_BYTES`] 個字），比對 run_dir 用。
    pub head: String,
}

/// 轉接函式：一個 agent 的 transcript → tokens、requests、models、first、last、head。格式變了只改這裡。
pub fn read_agent(path: &str) -> Result<AgentData, Crash> {
    let mut d = AgentData::default();
    let Ok(raw) = std::fs::read(path) else {
        return Ok(d);
    };
    let text = pyutil::universal_newlines(&String::from_utf8_lossy(&raw));
    let mut seen: HashSet<Key> = HashSet::new();
    let mut head_len = 0usize;
    for line in text.split_inclusive('\n') {
        if head_len < SCAN_PROMPT_BYTES {
            for c in line.chars().take(SCAN_PROMPT_BYTES - head_len) {
                d.head.push(c);
                head_len += 1;
            }
        }
        let Some(e) = parse_line(line) else { continue };
        if let Some(t) = parse_ts(e.get("timestamp")) {
            d.first = earlier(d.first, t)?;
            d.last = later(d.last, t)?;
        }
        let Some((m, usage)) = usage_of(&e) else { continue };
        if let Some(rid) = request_id(&e, m) {
            if !seen.insert(key_of(rid)?) {
                continue;
            }
        }
        for (slot, k) in d.tokens.iter_mut().zip(KINDS) {
            *slot = slot.saturating_add(int_of(usage.get(k)).unwrap_or(0));
        }
        if let Some(Value::String(model)) = m.get("model") {
            match d.models.iter_mut().find(|(name, _)| name == model) {
                Some((_, n)) => *n += 1,
                None => d.models.push((model.clone(), 1)),
            }
        }
    }
    let by_model: i64 = d.models.iter().map(|(_, n)| *n).sum();
    d.requests = if by_model != 0 { by_model } else { seen.len() as i64 };
    Ok(d)
}

/// [`tail_agent`] 的累計狀態；第一次用 `TailState::default()`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TailState {
    pub offset: u64,
    pub tokens: i64,
    pub seen: HashSet<Key>,
    pub requests: i64,
    pub model: String,
    pub first: Option<Ts>,
    pub last: Option<Ts>,
}

/// `bytes.splitlines()`：只認 `\n`、`\r`、`\r\n`。
fn bytes_splitlines(b: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        let c = byte(b, i);
        if c == b'\n' || c == b'\r' {
            out.push(b.get(start..i).unwrap_or_default());
            if c == b'\r' && byte(b, i + 1) == b'\n' {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start < b.len() {
        out.push(b.get(start..).unwrap_or_default());
    }
    out
}

/// 轉接函式（增量版，watcher 用）：從上次讀到的位置往後讀，累加到 `st`。
/// 只讀完整的行（最後一行還沒寫完就留到下次）。
pub fn tail_agent(path: &str, st: &mut TailState) -> Result<(), Crash> {
    let mut chunk = Vec::new();
    let read = std::fs::File::open(path).and_then(|mut fh| {
        fh.seek(SeekFrom::Start(st.offset))?;
        fh.read_to_end(&mut chunk)
    });
    if read.is_err() {
        return Ok(());
    }
    let Some(end) = chunk.iter().rposition(|c| *c == b'\n') else {
        return Ok(());
    };
    st.offset += end as u64 + 1;
    for raw in bytes_splitlines(chunk.get(..=end).unwrap_or_default()) {
        let Some(e) = parse_line(&String::from_utf8_lossy(raw)) else { continue };
        if let Some(t) = parse_ts(e.get("timestamp")) {
            st.first = earlier(st.first, t)?;
            st.last = later(st.last, t)?;
        }
        let Some((m, usage)) = usage_of(&e) else { continue };
        if let Some(rid) = request_id(&e, m) {
            if !st.seen.insert(key_of(rid)?) {
                continue;
            }
        }
        st.requests += 1;
        for k in KINDS {
            st.tokens = st.tokens.saturating_add(int_of(usage.get(k)).unwrap_or(0));
        }
        if let Some(Value::String(model)) = m.get("model") {
            st.model = model.clone();
        }
    }
    Ok(())
}

/// 讀一個 JSON 物件檔；讀不到、不是 UTF-8、不是 JSON、不是物件都回空的。
pub fn read_meta(path: &str) -> Map<String, Value> {
    let Some(text) = std::fs::read(path).ok().and_then(|b| String::from_utf8(b).ok()) else {
        return Map::new();
    };
    match pyjson::loads_lenient(&text) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

/// 路徑的各段（`Path(p).parts` 去掉根）。
fn parts(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty() && *s != ".").collect()
}

/// `Path(p).name`
fn name_of(p: &str) -> &str {
    parts(p).last().copied().unwrap_or_default()
}

/// 轉接函式：`<session>/workflows/wf_<id>.json`（wf_dir 是 `<session>/subagents/workflows/wf_<id>`）。
/// 路徑不到三層時 Python 的 `parents[2]` 丟 IndexError。
pub fn read_wf_meta(wf_dir: &str) -> Result<Map<String, Value>, Crash> {
    let p = ppath(wf_dir);
    let segs = parts(&p);
    if segs.len() < 3 {
        return Err(Crash("IndexError: 2".into()));
    }
    let lead = if p.starts_with('/') { "/" } else { "" };
    let session = segs.get(..segs.len() - 3).unwrap_or_default().join("/");
    let base = format!("{lead}{session}");
    let base = if base.is_empty() { ".".to_string() } else { base };
    Ok(read_meta(&ppath(&format!("{base}/workflows/{}.json", name_of(&p)))))
}

fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// 目錄裡的名字（讀不到當成空的）；`real_dir` 表示本身是目錄（不跟符號連結）。
fn list_dir(p: &str) -> Vec<(String, bool)> {
    let Ok(rd) = std::fs::read_dir(p) else {
        return Vec::new();
    };
    rd.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false))).collect()
}

fn child(parent: &str, name: &str) -> String {
    if parent == "/" { format!("/{name}") } else if parent == "." { name.to_string() } else { format!("{parent}/{name}") }
}

/// `p.glob("**/subagents/workflows/wf_*")`：`**` 不跟符號連結。
fn glob_wf(base: &str, out: &mut Vec<String>) {
    let wfs = child(&child(base, "subagents"), "workflows");
    if is_dir(&wfs) {
        for (name, _) in list_dir(&wfs) {
            if name.starts_with("wf_") {
                out.push(child(&wfs, &name));
            }
        }
    }
    for (name, real_dir) in list_dir(base) {
        if real_dir {
            glob_wf(&child(base, &name), out);
        }
    }
}

/// 路徑底下所有的 `wf_*` 目錄（給的就是 wf 目錄時原樣收下），去重。
pub fn find_wf_dirs(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let p = ppath(p);
        if name_of(&p).starts_with("wf_") && is_dir(&p) {
            out.push(p);
        } else if is_dir(&p) {
            let mut found = Vec::new();
            glob_wf(&p, &mut found);
            found.retain(|d| is_dir(d));
            found.sort_by(|a, b| parts(a).cmp(&parts(b)));
            found.dedup();
            out.extend(found);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    out.into_iter().filter(|d| seen.insert(d.clone())).collect()
}

// ------------------------------------------------------------------ 統計

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub name: Value,
    pub agents: i64,
    pub tokens: i64,
    pub requests: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRow {
    pub id: String,
    pub role: String,
    pub label: Value,
    pub phase: Value,
    pub model: Value,
    pub tokens: i64,
    pub requests: i64,
}

/// 一個 workflow run 的統計。
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub run_id: String,
    pub workflow: Value,
    pub status: Value,
    pub start: String,
    pub end: String,
    pub minutes: Option<f64>,
    pub agents: i64,
    pub requests: i64,
    /// 依 [`SHORT`] 的順序。
    pub tokens: [i64; 4],
    pub total_tokens: i64,
    pub avg_context: i64,
    pub by_role: Vec<Group>,
    pub by_model: Vec<Group>,
    pub agent_list: Vec<AgentRow>,
}

/// `x or 預設`
fn or_str(v: Option<&Value>, default: &str) -> Value {
    match v {
        Some(x) if pyfmt::truthy(x) => x.clone(),
        _ => Value::String(default.to_string()),
    }
}

fn sum(xs: impl Iterator<Item = i64>) -> i64 {
    xs.fold(0i64, i64::saturating_add)
}

fn group(items: &[(Value, i64, i64)]) -> Result<Vec<Group>, Crash> {
    let mut keys: Vec<Key> = Vec::new();
    let mut out: Vec<Group> = Vec::new();
    for (name, tokens, requests) in items {
        let k = key_of(name)?;
        let i = match keys.iter().position(|x| *x == k) {
            Some(i) => i,
            None => {
                keys.push(k);
                out.push(Group { name: name.clone(), agents: 0, tokens: 0, requests: 0 });
                out.len() - 1
            }
        };
        if let Some(g) = out.get_mut(i) {
            g.agents += 1;
            g.tokens = g.tokens.saturating_add(*tokens);
            g.requests += requests;
        }
    }
    out.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    Ok(out)
}

/// 一個 workflow run 的統計；run_dir 有給而且沒有任何派工單提到它就回 None。
pub fn summarize(wf_dir: &str, run_dir: &str) -> Result<Option<Row>, Crash> {
    let wf_dir = ppath(wf_dir);
    let mut mentions = run_dir.is_empty();
    let needles: Vec<String> = if run_dir.is_empty() {
        Vec::new()
    } else {
        let v = Value::String(run_dir.to_string());
        let inner = |s: String| s.get(1..s.len().saturating_sub(1)).unwrap_or_default().to_string();
        vec![run_dir.to_string(), inner(json_dumps(&v, false)), inner(json_dumps(&v, true))]
    };
    let mut files: Vec<String> = list_dir(&wf_dir).into_iter().map(|(n, _)| n)
        .filter(|n| n.len() >= "agent-.jsonl".len() && n.starts_with("agent-") && n.ends_with(".jsonl")).collect();
    files.sort();
    struct Agent {
        row: AgentRow,
        tokens: [i64; 4],
        first: Option<Ts>,
        last: Option<Ts>,
    }
    let mut agents: Vec<Agent> = Vec::new();
    for name in &files {
        let stem = name.get(..name.len() - ".jsonl".len()).unwrap_or_default();
        let a = read_agent(&child(&wf_dir, name))?;
        let meta = read_meta(&child(&wf_dir, &format!("{stem}.meta.json")));
        if !mentions && needles.iter().any(|n| a.head.contains(n.as_str())) {
            mentions = true;
        }
        let mut best: Option<&(String, i64)> = None;
        for m in &a.models {
            if best.is_none_or(|b| m.1 > b.1) {
                best = Some(m);
            }
        }
        let model = match best {
            Some((name, _)) => Value::String(name.clone()),
            None => or_str(meta.get("model"), "?"),
        };
        let agent_type = pyfmt::str_of(&or_str(meta.get("agentType"), "?"));
        agents.push(Agent {
            row: AgentRow {
                id: stem.get("agent-".len()..).unwrap_or_default().to_string(),
                role: agent_type.rsplit(':').next().unwrap_or_default().to_string(),
                label: or_str(meta.get("description"), ""),
                phase: or_str(meta.get("workflowPhase"), ""),
                model,
                tokens: sum(a.tokens.iter().copied()),
                requests: a.requests,
            },
            tokens: a.tokens,
            first: a.first,
            last: a.last,
        });
    }
    let wf = read_wf_meta(&wf_dir)?;
    if !mentions && !run_dir.is_empty() && pyfmt::str_of(&or_str(wf.get("scriptPath"), "")).contains(run_dir) {
        mentions = true;
    }
    if !mentions || agents.is_empty() {
        return Ok(None);
    }
    let mut total = [0i64; 4];
    for a in &agents {
        for (t, v) in total.iter_mut().zip(a.tokens) {
            *t = t.saturating_add(v);
        }
    }
    let reqs: i64 = agents.iter().map(|a| a.row.requests).sum();
    let (mut first, mut last): (Option<Ts>, Option<Ts>) = (None, None);
    for a in &agents {
        if let Some(t) = a.first {
            first = earlier(first, t)?;
        }
    }
    for a in &agents {
        if let Some(t) = a.last {
            last = later(last, t)?;
        }
    }
    let minutes = match (first, last) {
        (Some(f), Some(l)) => Some(pyish::round_nd(l.micros_since(&f)? as f64 / 1e6 / 60.0, 1)),
        _ => None,
    };
    let by_role = group(&agents.iter().map(|a| (Value::String(a.row.role.clone()), a.row.tokens, a.row.requests)).collect::<Vec<_>>())?;
    let by_model = group(&agents.iter().map(|a| (a.row.model.clone(), a.row.tokens, a.row.requests)).collect::<Vec<_>>())?;
    let ctx = total[0].saturating_add(total[1]).saturating_add(total[2]);
    Ok(Some(Row {
        run_id: name_of(&wf_dir).to_string(),
        workflow: or_str(wf.get("workflowName"), ""),
        status: or_str(wf.get("status"), ""),
        start: first.map(|t| t.isoformat()).unwrap_or_default(),
        end: last.map(|t| t.isoformat()).unwrap_or_default(),
        minutes,
        agents: agents.len() as i64,
        requests: reqs,
        tokens: total,
        total_tokens: sum(total.iter().copied()),
        avg_context: if reqs != 0 { (ctx as f64 / reqs as f64).round_ties_even() as i64 } else { 0 },
        by_role,
        by_model,
        agent_list: agents.into_iter().map(|a| a.row).collect(),
    }))
}

// ------------------------------------------------------------------ 輸出

/// 人讀的數字：`1.23M`、`45k`、`678`。
pub fn fmt(n: i64) -> String {
    let f = n as f64;
    if f >= 1e6 {
        format!("{:.2}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0}k", f / 1e3)
    } else {
        n.to_string()
    }
}

fn groups_json(gs: &[Group]) -> Value {
    Value::Array(gs.iter().map(|g| json!({"name": g.name, "agents": g.agents, "tokens": g.tokens, "requests": g.requests})).collect())
}

impl Row {
    /// `--json` 輸出的一筆（鍵的順序和 Python 版相同）。
    pub fn to_json(&self) -> Value {
        let mut tokens = Map::new();
        for (k, v) in SHORT.iter().zip(self.tokens) {
            tokens.insert((*k).to_string(), json!(v));
        }
        let mut o = Map::new();
        o.insert("run_id".into(), json!(self.run_id));
        o.insert("workflow".into(), self.workflow.clone());
        o.insert("status".into(), self.status.clone());
        o.insert("start".into(), json!(self.start));
        o.insert("end".into(), json!(self.end));
        o.insert("minutes".into(), self.minutes.map(|m| json!(m)).unwrap_or(Value::Null));
        o.insert("agents".into(), json!(self.agents));
        o.insert("requests".into(), json!(self.requests));
        o.insert("tokens".into(), Value::Object(tokens));
        o.insert("total_tokens".into(), json!(self.total_tokens));
        o.insert("avg_context".into(), json!(self.avg_context));
        o.insert("by_role".into(), groups_json(&self.by_role));
        o.insert("by_model".into(), groups_json(&self.by_model));
        o.insert("agent_list".into(), Value::Array(self.agent_list.iter().map(|a| {
            json!({"id": a.id, "role": a.role, "label": a.label, "phase": a.phase, "model": a.model, "tokens": a.tokens, "requests": a.requests})
        }).collect()));
        Value::Object(o)
    }
}

/// `json.dumps(v, ensure_ascii=False, indent=1)`
pub fn dumps_indent1(v: &Value) -> String {
    fn go(v: &Value, level: usize, out: &mut String) {
        match v {
            Value::Array(a) if !a.is_empty() => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&" ".repeat(level + 1));
                    go(x, level + 1, out);
                }
                out.push('\n');
                out.push_str(&" ".repeat(level));
                out.push(']');
            }
            Value::Object(o) if !o.is_empty() => {
                out.push('{');
                for (i, (k, x)) in o.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&" ".repeat(level + 1));
                    out.push_str(&pyish::json_str(k));
                    out.push_str(": ");
                    go(x, level + 1, out);
                }
                out.push('\n');
                out.push_str(&" ".repeat(level));
                out.push('}');
            }
            other => out.push_str(&json_dumps(other, false)),
        }
    }
    let mut out = String::new();
    go(v, 0, &mut out);
    out
}

fn take_chars(s: &str, n: usize) -> &str {
    pyish::take(s, n)
}

/// 人讀的輸出。
pub fn render(rows: &[Row]) -> String {
    let mut out: Vec<String> = Vec::new();
    let minutes = |m: Option<f64>| m.map(pyfmt::float).unwrap_or_else(|| "None".into());
    for r in rows {
        let workflow = if pyfmt::truthy(&r.workflow) { pyfmt::str_of(&r.workflow) } else { "（名稱未知）".to_string() };
        let status = if pyfmt::truthy(&r.status) { pyfmt::str_of(&r.status) } else { String::new() };
        out.push(format!("{}　{workflow}　{status}　開始 {}", r.run_id, take_chars(&r.start, 16).replace('T', " ")));
        out.push(format!(
            "  時長 {} 分｜agent {} 個｜請求 {} 次｜總 token {}（cache 讀取 {}、cache 寫入 {}、input {}、output {}）｜每次請求平均 context {}",
            minutes(r.minutes), r.agents, r.requests, fmt(r.total_tokens), fmt(r.tokens[2]), fmt(r.tokens[1]), fmt(r.tokens[0]), fmt(r.tokens[3]),
            fmt(r.avg_context)
        ));
        out.push(format!("  依角色：{}", r.by_role.iter().map(|g| format!("{}×{}＝{}", pyfmt::str_of(&g.name), g.agents, fmt(g.tokens))).collect::<Vec<_>>().join("、")));
        out.push(format!("  依模型：{}", r.by_model.iter().map(|g| format!("{}＝{}（{} 次）", pyfmt::str_of(&g.name), fmt(g.tokens), g.requests)).collect::<Vec<_>>().join("、")));
    }
    if rows.len() > 1 {
        // sum(r['minutes'] or 0 …)：沒有時長與 0.0 都換成整數 0，全部都是時印 0，不是 0.0
        let total = if rows.iter().any(|r| r.minutes.is_some_and(|m| m != 0.0)) {
            pyfmt::float(pyish::round_nd(rows.iter().fold(0.0, |acc, r| acc + r.minutes.unwrap_or(0.0)), 1))
        } else {
            "0".to_string()
        };
        out.push(format!("合計：{} 次 workflow，總 token {}，時長 {total} 分", rows.len(), fmt(sum(rows.iter().map(|r| r.total_tokens)))));
    }
    out.push("（估算：依 Claude Code 的 transcript 加總，格式沒有官方承諾；不參與任何閘門判定）".to_string());
    out.join("\n")
}

// ------------------------------------------------------------------ CLI

fn opt(argv: &[String], name: &str) -> String {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned().unwrap_or_default()
}

fn home() -> String {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    match std::env::var(var) {
        Ok(h) if !h.is_empty() => h.replace('\\', "/"),
        _ => "/".to_string(),
    }
}

/// `str(Path(a).expanduser())`
fn expanduser(a: &str) -> Result<String, Crash> {
    let p = ppath(a);
    if p == "~" {
        return Ok(ppath(&home()));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return Ok(ppath(&format!("{}/{rest}", home())));
    }
    if p.starts_with('~') {
        return Err(Crash(format!("RuntimeError: 不支援展開 {}（請寫完整路徑）", parts(&p).first().copied().unwrap_or_default())));
    }
    Ok(p)
}

/// Claude Code 把專案路徑裡不是英數的字元都換成 `-` 當目錄名。
pub fn project_dir(root: &str) -> Result<String, Crash> {
    let resolved = pyish::resolve(&expanduser(root)?).map_err(|e| Crash(format!("{}: {}", e.class, e.msg)))?;
    let name: String = resolved.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    Ok(ppath(&format!("{}/.claude/projects/{name}", home())))
}

fn run(argv: &[String]) -> Result<(i32, String), Crash> {
    let run_dir = opt(argv, "--run-dir");
    let mut paths: Vec<String> = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        let after_opt = i > 0 && argv.get(i - 1).is_some_and(|p| p == "--run-dir" || p == "--project");
        if !a.starts_with("--") && !after_opt {
            paths.push(expanduser(a)?);
        }
    }
    if argv.iter().any(|a| a == "--project") {
        paths.push(project_dir(&opt(argv, "--project"))?);
    }
    if paths.is_empty() {
        return Ok((2, format!("{USAGE}\n")));
    }
    let mut rows: Vec<Row> = Vec::new();
    for d in find_wf_dirs(&paths) {
        if let Some(r) = summarize(&d, &run_dir)? {
            rows.push(r);
        }
    }
    rows.sort_by(|a, b| a.start.cmp(&b.start));
    let usable = rows.iter().any(|r| r.requests != 0);
    if argv.iter().any(|a| a == "--json") {
        let doc = json!({"ok": usable, "runs": rows.iter().map(Row::to_json).collect::<Vec<_>>()});
        return Ok((0, format!("{}\n", dumps_indent1(&doc))));
    }
    if rows.is_empty() {
        return Ok((0, "找不到 workflow transcript（路徑底下沒有 subagents/workflows/wf_*，或沒有派工單提到指定的 run 目錄）\n".into()));
    }
    if !usable {
        return Ok((0, "這版 transcript 格式讀不了：找到 workflow 目錄，但沒有任何一筆 usage。Claude Code 的內部格式可能改了，請回報版本；這不影響流程。\n".into()));
    }
    Ok((0, format!("{}\n", render(&rows))))
}

/// 回（exit code，stdout，stderr）。`--selftest` 的輸出也收在 stdout。
pub fn run_captured(argv: &[String]) -> (i32, String, String) {
    if argv.iter().any(|a| a == "--selftest") {
        let (code, out) = selftest_captured();
        return (code, out, String::new());
    }
    match run(argv) {
        Ok((code, out)) => (code, out, String::new()),
        Err(e) => (1, String::new(), format!("awf cost：讀到這版工具處理不了的資料（{e}）\n")),
    }
}

/// `awf cost …`
pub fn main(args: &[String]) -> i32 {
    let (code, out, err) = run_captured(args);
    print!("{out}");
    eprint!("{err}");
    code
}

// ------------------------------------------------------------------ selftest

fn usage_line(rid: &str, model: &str, u: &Value, ts: &str) -> String {
    json!({"timestamp": ts, "requestId": rid, "message": {"model": model, "usage": u}}).to_string()
}

/// `awf cost --selftest`：回（exit code，輸出）。
pub fn selftest_captured() -> (i32, String) {
    let mut ok = true;
    let mut out = String::new();
    let mut check = |name: &str, cond: bool, detail: &str| {
        ok = ok && cond;
        let extra = if !cond && !detail.is_empty() { format!("：{detail}") } else { String::new() };
        out.push_str(&format!("  [{}] {name}{extra}\n", if cond { "通過" } else { "失敗" }));
    };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("awf-cost-selftest-{}-{nanos}", std::process::id()));
    let td_s = td.to_string_lossy().replace('\\', "/");
    let sess = format!("{td_s}/proj/sess1");
    let wf = format!("{sess}/subagents/workflows/wf_abc");
    let other = format!("{sess}/subagents/workflows/wf_other");
    let empty = format!("{sess}/subagents/workflows/wf_empty");
    let write = |path: String, text: String| {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, text);
    };
    write(format!("{sess}/workflows/wf_abc.json"), json!({"workflowName": "sdlc-plan", "status": "completed", "scriptPath": "/x/sdlc-plan.js"}).to_string());
    let u1 = json!({"input_tokens": 10, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 5});
    let u2 = json!({"input_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 3000, "output_tokens": 15});
    let prompt = json!({"timestamp": "2026-01-01T00:00:00Z", "type": "user", "message": {"content": "共同規則…背景在 _tpm/runs/示範-r1/context.md"}}).to_string();
    write(format!("{wf}/agent-a1.jsonl"), [
        prompt,
        usage_line("r1", "claude-opus-x", &u1, "2026-01-01T00:01:00Z"),
        usage_line("r1", "claude-opus-x", &u1, "2026-01-01T00:01:01Z"),
        "不是 JSON 的一行".to_string(),
        usage_line("r2", "claude-opus-x", &u2, "2026-01-01T00:11:00Z"),
    ].join("\n") + "\n");
    write(format!("{wf}/agent-a1.meta.json"), json!({"agentType": "agent-workflow:tpm-researcher", "description": "research:code", "workflowPhase": "Research"}).to_string());
    write(format!("{wf}/agent-a2.jsonl"), usage_line("r3", "claude-haiku-x", &u1, "2026-01-01T00:05:00Z") + "\n");
    write(format!("{wf}/agent-a2.meta.json"), "壞掉的 meta".to_string());
    write(format!("{other}/agent-b1.jsonl"), usage_line("r9", "claude-opus-x", &u1, "2026-01-02T00:00:00Z") + "\n");
    write(format!("{empty}/agent-c1.jsonl"), json!({"timestamp": "2026-01-03T00:00:00Z", "message": {"content": "x"}}).to_string() + "\n");

    let r = summarize(&wf, "").ok().flatten();
    let dbg = format!("{r:?}");
    check("依 requestId 去重後加總（重複的 r1 只算一次）", r.as_ref().is_some_and(|r| r.tokens == [40, 200, 5000, 25] && r.requests == 3), &dbg);
    check("時長取最早與最晚的 timestamp（含派工單那一行）", r.as_ref().is_some_and(|r| r.minutes == Some(11.0)), &dbg);
    check("每次請求平均 context＝(input＋cache 讀寫)／請求數", r.as_ref().is_some_and(|r| r.avg_context == 1747), &dbg);
    let tokens_of = |gs: &[Group], name: &str| gs.iter().find(|g| g.name == json!(name)).map(|g| g.tokens);
    let by_role = r.as_ref().map(|r| r.by_role.clone()).unwrap_or_default();
    let by_model = r.as_ref().map(|r| r.by_model.clone()).unwrap_or_default();
    check("角色取自 meta 的 agentType（去掉命名空間）；meta 壞掉的記成 ?", tokens_of(&by_role, "tpm-researcher") == Some(4150) && tokens_of(&by_role, "?") == Some(1115), &format!("{by_role:?}"));
    check("依模型分組", by_model.len() == 2 && tokens_of(&by_model, "claude-opus-x") == Some(4150) && tokens_of(&by_model, "claude-haiku-x") == Some(1115), &format!("{by_model:?}"));
    check("workflow 名稱與狀態取自 workflows/wf_<id>.json", r.as_ref().is_some_and(|r| r.workflow == json!("sdlc-plan") && r.status == json!("completed")), "");
    check("label 與 phase 取自 meta", r.as_ref().and_then(|r| r.agent_list.first()).is_some_and(|a| a.label == json!("research:code") && a.phase == json!("Research")), "");
    check("--run-dir：派工單提到 run 目錄才算（中文路徑）",
          matches!(summarize(&wf, "_tpm/runs/示範-r1"), Ok(Some(_))) && matches!(summarize(&other, "_tpm/runs/示範-r1"), Ok(None)), "");
    let found: Vec<String> = find_wf_dirs(std::slice::from_ref(&sess)).iter().map(|d| name_of(d).to_string()).collect();
    check("從 session 目錄往下找到所有 wf_*", found == ["wf_abc", "wf_empty", "wf_other"], &format!("{found:?}"));

    let (code, text, _) = run_captured(&[empty]);
    check("沒有任何 usage 時明說格式讀不了並 exit 0", code == 0 && text.contains("格式讀不了"), &text);
    let (code, text, _) = run_captured(&[format!("{td_s}/nowhere")]);
    check("路徑不存在時說明並 exit 0", code == 0 && text.contains("找不到"), &text);
    let (code, text, _) = run_captured(&[sess.clone(), "--json".to_string()]);
    let j: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let runs: Vec<Value> = j.get("runs").and_then(Value::as_array).cloned().unwrap_or_default();
    let ids: HashSet<&str> = runs.iter().filter_map(|x| x.get("run_id").and_then(Value::as_str)).collect();
    let has_keys = runs.first().is_some_and(|x| ["total_tokens", "minutes", "by_role", "by_model", "avg_context"].iter().all(|k| x.get(k).is_some()));
    check("--json 輸出可解析，含每個 run 與合計需要的欄位",
          code == 0 && j.get("ok") == Some(&json!(true)) && ids == HashSet::from(["wf_abc", "wf_other", "wf_empty"]) && has_keys, take_chars(&text, 300));
    let (_, text, _) = run_captured(&[sess]);
    check("人讀的輸出有估算聲明與合計", text.contains("估算") && text.contains("合計："), &text);
    let name = |root: &str| project_dir(root).map(|p| name_of(&p).to_string()).unwrap_or_default();
    check("專案路徑換成 projects 目錄名（非英數一律換成 -）", name("/tmp/示範_專案 a").ends_with("-------a") && !name("/tmp/x").is_empty() && !name("/tmp/x").contains('/'), "");
    let _ = std::fs::remove_dir_all(&td);
    out.push_str(&format!("awf_cost selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    (if ok { 0 } else { 1 }, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_match_python() {
        // f"{n / 1e3:.0f}k" 是四捨六入五成雙
        assert_eq!(fmt(2500), "2k");
        assert_eq!(fmt(3500), "4k");
        assert_eq!(fmt(999), "999");
        assert_eq!(fmt(1000), "1k");
        assert_eq!(fmt(999_999), "1000k");
        assert_eq!(fmt(1_000_000), "1.00M");
        assert_eq!(fmt(1_005_000), "1.00M");
        assert_eq!(fmt(-5), "-5");
    }

    #[test]
    fn timestamps_match_python() {
        let iso = |s: &str| fromisoformat(s).map(|t| t.isoformat());
        assert_eq!(iso("2026-01-01T00:01:00+00:00").as_deref(), Some("2026-01-01T00:01:00+00:00"));
        assert_eq!(iso("2026-01-01").as_deref(), Some("2026-01-01T00:00:00"));
        assert_eq!(iso("20260101T010203.5-0530").as_deref(), Some("2026-01-01T01:02:03.500000-05:30"));
        assert_eq!(iso("2026-W01-1").as_deref(), Some("2025-12-29T00:00:00"));
        assert_eq!(iso("2026-13-01"), None);
        assert_eq!(iso("None"), None);
        let (a, b) = (fromisoformat("2026-01-01T00:00:00"), fromisoformat("2026-01-01T00:00:00+00:00"));
        assert!(a.zip(b).is_some_and(|(a, b)| a.cmp(&b).is_err()));
    }

    #[test]
    fn json_indent_matches_python() {
        let v = json!({"ok": true, "runs": [], "x": {"a": [1, 2.0, "中"], "b": {}}});
        assert_eq!(dumps_indent1(&v), "{\n \"ok\": true,\n \"runs\": [],\n \"x\": {\n  \"a\": [\n   1,\n   2.0,\n   \"中\"\n  ],\n  \"b\": {}\n }\n}");
    }
}
