//! 開發規範（methodology profile）：選哪個 profile、把答案檔或互動問答的回答套進去、升級時比對缺少的欄位。
//!
//! 模版只管工作流程，做法由使用者決定；這裡的題目與預設值照 0.x 的 Python 版 `init.py`。

use super::merge::{deep_merge, missing_fields, set_missing, short};
use super::{crash, die, read_text, read_text_sig, write_text_native, Console, Kv, Shown, Site, R};
use crate::merge_gate::pyutil::{self, ReadErr};
use crate::merge_gate::pyjson;
use crate::pyfmt;
use crate::schema::Schemas;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::OnceLock;

/// 限制文件可以套用的段。
pub const CONSTRAINT_STAGES: &[&str] = &["plan", "design", "build", "sync", "test", "deliver", "merge"];
/// 簡寫沒寫段時的預設。
pub const CONSTRAINT_DEFAULT_STAGES: &[&str] = &["design", "deliver"];
/// 讀取上限的模式。
pub const LIMIT_MODES: &[&str] = &["off", "warn", "enforce"];
/// 分支怎麼跟上 base。
pub const SYNC_STRATEGIES: &[&str] = &["auto", "rebase", "merge", "transit"];

fn empty() -> &'static Map<String, Value> {
    static E: OnceLock<Map<String, Value>> = OnceLock::new();
    E.get_or_init(Map::new)
}

/// `d.get(k, {})`，而且之後要當物件用：有這個鍵但不是物件時 0.x 的 Python 版會當掉。
fn sub<'a>(d: &'a Map<String, Value>, k: &str, what: &str) -> R<&'a Map<String, Value>> {
    match d.get(k) {
        None => Ok(empty()),
        Some(Value::Object(o)) => Ok(o),
        Some(_) => crash(format!("{what} 的 {k} 要是 JSON 物件")),
    }
}

/// `d.setdefault(k, {})`，而且之後要往裡面寫。
fn setdefault_obj<'a>(d: &'a mut Map<String, Value>, k: &str, what: &str) -> R<&'a mut Map<String, Value>> {
    if !d.contains_key(k) {
        d.insert(k.to_string(), Value::Object(Map::new()));
    }
    match d.get_mut(k).and_then(Value::as_object_mut) {
        Some(o) => Ok(o),
        None => crash(format!("{what} 的 {k} 要是 JSON 物件")),
    }
}

/// `Path(name).suffix == ".json"`。
fn has_json_suffix(path: &str) -> bool {
    let name = pyutil::name(path);
    name.len() > ".json".len() && name.ends_with(".json")
}

/// `Path(p).stem`。
fn stem(path: &str) -> String {
    let name = pyutil::name(path);
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name.get(..i).unwrap_or_default().to_string(),
        _ => name,
    }
}

/// 內建 profile 的名稱（`methodologies/*.json` 的檔名去掉副檔名），排序過。
pub fn profile_names(site: &Site) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(site.methodologies()).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".json"))
        .map(|n| stem(&n))
        .collect();
    out.sort();
    out
}

/// name 是內建 profile 名稱或 JSON 檔路徑。
pub fn load_profile(site: &Site, name: &str) -> R<Value> {
    let mut p = site.user_path(name);
    if !(has_json_suffix(&p.shown) && p.real.is_file()) {
        let shown = pyutil::join(&site.methodologies().to_string_lossy(), &format!("{name}.json"));
        p = Shown { real: Path::new(&shown).to_path_buf(), shown };
    }
    if !p.real.is_file() {
        return die(format!("找不到開發規範 profile「{name}」（內建：{}；也可以給 JSON 檔路徑）", profile_names(site).join("、")));
    }
    let text = match read_text(&p.real) {
        Ok(t) => t,
        Err(e) => return crash(format!("讀不到 profile {}：{}", p.shown, e.msg())),
    };
    match pyjson::loads(&text) {
        Ok(v) => Ok(v),
        Err(e) => die(format!("profile {} 不是合法 JSON：{e}", p.shown)),
    }
}

fn int_value(n: i128) -> Value {
    if let Ok(i) = i64::try_from(n) {
        json!(i)
    } else if let Ok(u) = u64::try_from(n) {
        json!(u)
    } else {
        json!(n as f64)
    }
}

/// Python 的 `int(v)`（v 是 `json.loads` 得到的值）；轉不了回 None。
fn py_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::String(s) => pyutil::int_of_str(s),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i128::from(i))
            } else if let Some(u) = n.as_u64() {
                Some(i128::from(u))
            } else {
                n.as_f64().filter(|f| f.is_finite() && f.abs() < 1e30).map(|f| f.trunc() as i128)
            }
        }
        _ => None,
    }
}

fn int_or_none(v: Option<&Value>, what: &str) -> R<Value> {
    let Some(v) = v else { return Ok(Value::Null) };
    if v.is_null() || v.as_str() == Some("") {
        return Ok(Value::Null);
    }
    let text = pyutil::strip(&pyfmt::str_of(v)).to_lowercase();
    if ["0", "none", "null", "不限"].contains(&text.as_str()) {
        return Ok(Value::Null);
    }
    match py_int(v) {
        Some(n) if n > 0 => Ok(int_value(n)),
        Some(_) => Ok(Value::Null),
        None => die(format!("{what} 要是正整數或「不限」（收到 {}）", pyfmt::repr(v))),
    }
}

fn split_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("[+、 ]+").expect("固定的正規式"))
}

fn slug_rx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("[^a-z0-9]+").expect("固定的正規式"))
}

/// 答案 constraints：清單，每項是物件（照 schema），或簡寫字串「路徑」「路徑=段+段」（id 取自檔名）。
/// 字串也可以用逗號串起多項。格式由存檔前的 schema 與語意檢查把關，這裡只做簡寫展開。
pub fn parse_constraints(raw: &Value) -> R<Vec<Value>> {
    let items: Vec<Value> = match raw {
        Value::String(s) => s.split(',').map(|x| json!(pyutil::strip(x))).collect(),
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.keys().map(|k| json!(k)).collect(),
        other if !pyfmt::truthy(other) => Vec::new(),
        other => return crash(format!("constraints 要是清單或字串（收到 {}）", pyfmt::repr(other))),
    };
    let mut out: Vec<Value> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for it in items {
        if let Some(o) = it.as_object() {
            used.push(pyfmt::str_opt(o.get("id")));
            out.push(it);
            continue;
        }
        let full = pyfmt::str_of(&it);
        let text = pyutil::strip(&full);
        if text.is_empty() || text == "-" {
            continue;
        }
        let (path, st) = text.split_once('=').unwrap_or((text, ""));
        let path = pyutil::strip(path);
        let mut stages: Vec<String> = split_rx().split(st).map(pyutil::strip).filter(|x| !x.is_empty()).map(str::to_string).collect();
        if stages.is_empty() {
            stages = CONSTRAINT_DEFAULT_STAGES.iter().map(|x| x.to_string()).collect();
        }
        let bad: Vec<&str> = stages.iter().map(String::as_str).filter(|x| !CONSTRAINT_STAGES.contains(x)).collect();
        if !bad.is_empty() {
            return die(format!("限制文件 {path} 的段「{}」不認得（可用：{}）", bad.join("、"), CONSTRAINT_STAGES.join("、")));
        }
        let lowered = stem(path).to_lowercase();
        let slug = slug_rx().replace_all(&lowered, "-");
        let slug = slug.trim_matches('-');
        let base = if slug.is_empty() { "doc" } else { slug };
        let head = |n: usize| base.chars().take(n).collect::<String>();
        let (mut cid, mut n) = (head(40), 2);
        while used.contains(&cid) {
            cid = format!("{}-{n}", head(36));
            n += 1;
        }
        used.push(cid.clone());
        out.push(json!({"id": cid, "file": path.replace('\\', "/"), "stages": stages}));
    }
    Ok(out)
}

/// 答案 read_limits：字串（mode）或物件 {mode, whole_file_lines, read_budget, max_agents}。
/// read_budget 可以是整數（每個 agent 的預設額度）或 {default, roles}。沒回答的門檻沿用 profile 的建議值。
pub fn apply_read_limits(prof: &mut Map<String, Value>, rl: &Value) -> R<()> {
    let wrapped;
    let rl: &Map<String, Value> = match rl {
        Value::String(s) => {
            wrapped = json!({"mode": s});
            match wrapped.as_object() {
                Some(o) => o,
                None => empty(),
            }
        }
        Value::Object(o) => o,
        other => return die(format!("read_limits 要是 {} 其中之一，或物件（收到 {}）", LIMIT_MODES.join("、"), pyfmt::repr(other))),
    };
    let mode = match rl.get("mode") {
        Some(v) if pyfmt::truthy(v) => pyfmt::str_of(v),
        _ => "off".to_string(),
    };
    if !LIMIT_MODES.contains(&mode.as_str()) {
        return die(format!("read_limits.mode 要是 {} 其中之一（收到 {}）", LIMIT_MODES.join("、"), pyfmt::repr_str(&mode)));
    }
    let lim = setdefault_obj(prof, "limits", "profile")?;
    lim.insert("mode".to_string(), json!(mode));

    fn num(v: &Value, what: &str) -> R<i128> {
        let text = pyfmt::str_of(v);
        let digits = pyutil::strip(&text).trim_start_matches('-');
        let is_digit = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
        match py_int(v) {
            Some(n) if !v.is_boolean() && is_digit && n >= 0 => Ok(n),
            _ => die(format!("{what} 要是 0（不限）或正整數（收到 {}）", pyfmt::repr(v))),
        }
    }
    if let Some(v) = rl.get("whole_file_lines") {
        let n = num(v, "read_limits.whole_file_lines")?;
        lim.insert("whole_file_lines".to_string(), int_value(n));
    }
    if let Some(rb) = rl.get("read_budget") {
        let cur = setdefault_obj(lim, "read_budget", "limits")?;
        if let Some(rb) = rb.as_object() {
            if let Some(v) = rb.get("default") {
                let n = num(v, "read_limits.read_budget.default")?;
                cur.insert("default".to_string(), int_value(n));
            }
            if let Some(roles) = rb.get("roles").and_then(Value::as_object) {
                let mut m = Map::new();
                for (k, v) in roles {
                    m.insert(k.clone(), int_value(num(v, &format!("read_limits.read_budget.roles.{k}"))?));
                }
                cur.insert("roles".to_string(), Value::Object(m));
            }
        } else {
            let n = num(rb, "read_limits.read_budget")?;
            cur.insert("default".to_string(), int_value(n));
        }
    }
    if let Some(v) = rl.get("max_agents") {
        let n = num(v, "read_limits.max_agents")?;
        if n != 0 {
            lim.insert("max_agents".to_string(), int_value(n));
        } else {
            lim.shift_remove("max_agents");
        }
    }
    Ok(())
}

/// 把使用者的回答（答案檔或互動）套進 profile。只改回答到的項目，其餘沿用 profile。
pub fn apply_answers(profile: &Value, ans: &Map<String, Value>) -> R<Map<String, Value>> {
    let Some(mut prof) = profile.as_object().cloned() else {
        return crash("開發規範 profile 要是 JSON 物件");
    };
    {
        let conv = setdefault_obj(&mut prof, "conventions", "profile")?;
        if let Some(cs) = ans.get("code_style") {
            let extra = match cs {
                Value::Object(o) => o.clone(),
                other => json!({"description": pyfmt::str_of(other), "command": null, "tool": null}).as_object().cloned().unwrap_or_default(),
            };
            let merged = deep_merge(sub(conv, "code_style", "conventions")?, &extra);
            conv.insert("code_style".to_string(), Value::Object(merged));
        }
        if let Some(c) = ans.get("commit") {
            let new = match c {
                Value::Object(o) => Value::Object(deep_merge(sub(conv, "commit", "conventions")?, o)),
                // 只改格式說明時，profile 的範例與檢查 regex 可能跟新格式不符，一併清掉
                other => json!({"format": pyfmt::str_of(other), "examples": {}, "check_regex": null}),
            };
            conv.insert("commit".to_string(), new);
        }
        if let Some(rx) = ans.get("commit_regex") {
            let v = if pyfmt::truthy(rx) { rx.clone() } else { Value::Null };
            setdefault_obj(conv, "commit", "conventions")?.insert("check_regex".to_string(), v);
        }
        if let Some(bp) = ans.get("branch_prefix").filter(|v| pyfmt::truthy(v)) {
            setdefault_obj(conv, "branch", "conventions")?.insert("prefix".to_string(), json!(pyfmt::str_of(bp)));
        }
        if let Some(st) = ans.get("sync_strategy").filter(|v| pyfmt::truthy(v)) {
            let st = pyfmt::str_of(st);
            if !SYNC_STRATEGIES.contains(&st.as_str()) {
                return die(format!("sync_strategy 要是 {} 其中之一（收到 {}）", SYNC_STRATEGIES.join("、"), pyfmt::repr_str(&st)));
            }
            setdefault_obj(conv, "sync", "conventions")?.insert("strategy".to_string(), json!(st));
        }
        if let Some(ps) = ans.get("pr_size") {
            let ps = match ps {
                Value::Object(o) => o,
                other if !pyfmt::truthy(other) => empty(),
                other => return crash(format!("pr_size 要是物件（收到 {}）", pyfmt::repr(other))),
            };
            let mut extra = Map::new();
            extra.insert("max_lines".to_string(), int_or_none(ps.get("max_lines"), "pr_size.max_lines")?);
            extra.insert("max_files".to_string(), int_or_none(ps.get("max_files"), "pr_size.max_files")?);
            let merged = deep_merge(sub(conv, "pr_size", "conventions")?, &extra);
            conv.insert("pr_size".to_string(), Value::Object(merged));
        }
        if let Some(d) = ans.get("doc_format") {
            let extra = match d {
                Value::Object(o) => o.clone(),
                other => json!({"format": pyfmt::str_of(other)}).as_object().cloned().unwrap_or_default(),
            };
            let merged = deep_merge(sub(conv, "doc_format", "conventions")?, &extra);
            conv.insert("doc_format".to_string(), Value::Object(merged));
        }
    }
    if let Some(tf) = ans.get("test_first") {
        let stages = setdefault_obj(&mut prof, "stages", "profile")?;
        setdefault_obj(stages, "build", "stages")?.insert("test_first".to_string(), json!(pyfmt::truthy(tf)));
    }
    if let Some(req) = ans.get("requirements").filter(|v| v.is_object()) {
        prof.insert("requirements".to_string(), req.clone());
    }
    if let Some(rl) = ans.get("read_limits") {
        apply_read_limits(&mut prof, rl)?;
    }
    if let Some(c) = ans.get("constraints") {
        prof.insert("constraints".to_string(), Value::Array(parse_constraints(c)?));
    }
    // 1.0 新增的主題（models、scrub、knowledge、links、deliverables、ci、parallelism、affected_tests_only）
    super::interview::apply_topic_answers(&mut prof, ans)?;
    if let Some(ov) = ans.get("overrides").and_then(Value::as_object) {
        prof = deep_merge(&prof, ov);
    }
    Ok(prof)
}

fn starts_y(s: &str) -> bool {
    s.to_lowercase().starts_with('y')
}

/// `x or 預設`，格式化成字串（給題目的預設值用）。
fn shown_or(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(x) if pyfmt::truthy(x) => pyfmt::str_of(x),
        _ => default.to_string(),
    }
}

/// 互動模式：逐項確認使用者的開發規範與慣例習慣，回傳（profile 名稱, 回答）。
/// `seed`＝先有的答案（偵測到的建議，或 `--reconfigure` 時存下來的答案）：當每題的預設值，Enter 就採用。
pub fn ask_methodology(con: &mut Console, site: &Site, default_name: &str, branch_prefix: &str, seed: &Map<String, Value>) -> R<(String, Map<String, Value>)> {
    con.say("\n== 開發規範與慣例（模版只管工作流程，做法由你決定；Enter＝用預設值） ==");
    for n in profile_names(site) {
        let p = load_profile(site, &n)?;
        let Some(o) = p.as_object() else { return crash(format!("內建 profile {n} 要是 JSON 物件")) };
        let desc = o.get("description").map(pyfmt::str_of).unwrap_or_default();
        con.say(&format!("  - {n}：{desc}"));
    }
    let name = con.ask("開發規範 profile（上面的名稱，或自己的 profile JSON 路徑）", if default_name.is_empty() { "default" } else { default_name });
    let loaded = load_profile(site, &name)?;
    // 題目的預設值＝profile 套上先有的答案；回答和它不同才記成新的答案
    let seeded;
    let prof = if seed.is_empty() {
        let Some(p) = loaded.as_object() else { return crash("開發規範 profile 要是 JSON 物件") };
        p
    } else {
        let mut s = seed.clone();
        s.shift_remove("overrides");
        seeded = apply_answers(&loaded, &s)?;
        &seeded
    };
    let conv = sub(prof, "conventions", "profile")?;
    let mut ans: Map<String, Value> = seed.clone();
    let build = sub(sub(prof, "stages", "profile")?, "build", "stages")?;
    let tf_now = build.get("test_first").map(pyfmt::truthy).unwrap_or(true);
    let tf = con.ask("build 段要不要先寫會失敗的測試再實作（y／n）", if tf_now { "y" } else { "n" });
    if starts_y(&tf) != tf_now {
        ans.insert("test_first".to_string(), json!(starts_y(&tf)));
    }
    let cs = sub(conv, "code_style", "conventions")?;
    let cmd = con.ask_opt("code style 檢查指令或工具（例如 ruff format --check .；- ＝沒有，照 repo 既有設定）", &shown_or(cs.get("command"), "-"));
    if let Some(cmd) = cmd.filter(|c| !c.is_empty() && cs.get("command").and_then(Value::as_str) != Some(c.as_str())) {
        let tool = cmd.split(pyutil::is_py_space).find(|x| !x.is_empty()).unwrap_or_default().to_string();
        ans.insert("code_style".to_string(), json!({"command": cmd, "tool": tool, "description": format!("提交前跑 {cmd}，不得新增違規。")}));
    }
    let cm = sub(conv, "commit", "conventions")?;
    let fmt_now = cm.get("format").cloned().unwrap_or_else(|| json!(""));
    if let Some(fmt) = con.ask_opt("commit 格式說明", &pyfmt::str_of(&fmt_now)) {
        if fmt_now.as_str() != Some(fmt.as_str()) {
            ans.insert("commit".to_string(), json!(fmt));
        }
    }
    let rx_now = cm.get("check_regex").filter(|v| pyfmt::truthy(v));
    match con.ask_opt("commit 標題檢查 regex（- ＝不檢查）", &shown_or(rx_now, "-")) {
        Some(rx) => {
            let same = match rx_now {
                Some(v) => v.as_str() == Some(rx.as_str()),
                None => rx.is_empty(),
            };
            if !same || ans.contains_key("commit") {
                ans.insert("commit_regex".to_string(), if rx.is_empty() { Value::Null } else { json!(rx) });
            }
        }
        None => {
            if ans.contains_key("commit") {
                ans.insert("commit_regex".to_string(), rx_now.cloned().unwrap_or(Value::Null));
            }
        }
    }
    let bp_default = if branch_prefix.is_empty() {
        sub(conv, "branch", "conventions")?.get("prefix").cloned().unwrap_or_else(|| json!("feature/"))
    } else {
        json!(branch_prefix)
    };
    let bp = match con.ask_opt("分支前綴", &pyfmt::str_of(&bp_default)) {
        Some(s) => json!(s),
        None => bp_default,
    };
    ans.insert("branch_prefix".to_string(), bp);
    let st_now = sub(conv, "sync", "conventions")?.get("strategy").cloned().unwrap_or_else(|| json!("auto"));
    if let Some(st) = con.ask_opt("分支落後 main 時怎麼跟上（auto＝依情況選 rebase／merge／中轉分支；也可固定 rebase、merge、transit）", &pyfmt::str_of(&st_now)) {
        if st_now.as_str() != Some(st.as_str()) {
            ans.insert("sync_strategy".to_string(), json!(st));
        }
    }
    let ps = sub(conv, "pr_size", "conventions")?;
    let ml = con.ask("PR 大小上限：行數（0 ＝不限）", &shown_or(ps.get("max_lines"), "0"));
    let mf = con.ask("PR 大小上限：檔案數（0 ＝不限）", &shown_or(ps.get("max_files"), "0"));
    ans.insert("pr_size".to_string(), json!({"max_lines": ml, "max_files": mf}));
    let df_have = sub(conv, "doc_format", "conventions")?.get("format");
    let df_default = df_have.cloned().unwrap_or_else(|| json!("Markdown"));
    match con.ask_opt("文件格式", &pyfmt::str_of(&df_default)) {
        Some(df) => {
            if df_have.and_then(Value::as_str) != Some(df.as_str()) {
                ans.insert("doc_format".to_string(), json!(df));
            }
        }
        None => {
            if df_have.is_none() {
                ans.insert("doc_format".to_string(), df_default);
            }
        }
    }
    con.say("  限制文件（可以不設）：你要 agent 遵守的文件，例如 ARCHITECTURE.md、DESIGN.md、REVIEW.md。寫法：路徑，或 路徑=段+段（段：plan、design、build、test、deliver…；沒寫＝design+deliver）。");
    let cs_keep = if ans.contains_key("constraints") { "沿用" } else { "-" };
    let cs_in = con.ask("限制文件（逗號分隔；- ＝沒有）", cs_keep);
    if cs_in == "-" || cs_in.is_empty() {
        ans.shift_remove("constraints");
    } else if cs_in != "沿用" {
        ans.insert("constraints".to_string(), json!(cs_in));
    }
    let lim = sub(prof, "limits", "profile")?;
    con.say("  讀取上限（省 token 用，可以不開）：限制模版 agent 整檔讀大檔與每個 agent 的讀取量；不限制 agent 的輸出。");
    let lm = con.ask("要不要啟用讀取上限（off＝不啟用；warn＝只提醒不擋；enforce＝擋）", &shown_or(lim.get("mode"), "off"));
    if lm != "off" {
        let rb = sub(lim, "read_budget", "limits")?;
        let lines = con.ask("超過幾行的檔要分段讀（0 ＝不限）", &shown_or(lim.get("whole_file_lines"), "0"));
        let budget = con.ask("每個 agent 一次派工最多讀幾個位元組（0 ＝不限；各角色的額度之後可在 methodology.json 調）", &shown_or(rb.get("default"), "0"));
        ans.insert("read_limits".to_string(), json!({"mode": lm, "whole_file_lines": lines, "read_budget": {"default": budget}}));
    } else if ans.contains_key("read_limits") {
        ans.insert("read_limits".to_string(), json!("off"));
    }
    Ok((name, ans))
}

/// 主 session 會不會直接 push main、合併 PR：答案檔的 main_session_push_merge > 互動問答 > 預設（不會）。
pub fn ask_main_session(con: &mut Console, answers: Option<&Shown>, interactive: bool) -> R<bool> {
    if let Some(a) = answers {
        let text = match read_text_sig(&a.real) {
            Ok(t) => t,
            Err(ReadErr::Os(_)) => return Ok(false),
            Err(ReadErr::Decode(m)) => return crash(format!("答案檔 {} 不是 UTF-8：{m}", a.shown)),
        };
        let v = pyjson::loads(&text).ok();
        return Ok(match v.as_ref().and_then(Value::as_object).and_then(|o| o.get("main_session_push_merge")) {
            Some(Value::String(s)) => ["y", "yes", "true", "1", "會"].contains(&pyutil::strip(s).to_lowercase().as_str()),
            Some(Value::Bool(true)) => true,
            _ => false,
        });
    }
    if !interactive {
        return Ok(false);
    }
    con.say("\n== 主 session 的習慣 ==");
    let a = con.ask("你既有的 skill 或習慣會不會在主 session 直接 push main、合併 PR（y／n）", "n");
    Ok(starts_y(pyutil::strip(&a)) || pyutil::strip(&a) == "會")
}

/// 用模版自己的 schema 驗 profile（`schemalib.check_doc(prof, "methodology")`）。
pub fn check_methodology(site: &Site, prof: &Map<String, Value>) -> R<Vec<String>> {
    match Schemas::new(site.scaffold_teams()).check_doc(&Value::Object(prof.clone()), "methodology") {
        Ok(errs) => Ok(errs),
        Err(m) => crash(m),
    }
}

fn first5(errs: &[String]) -> String {
    errs.iter().take(5).map(String::as_str).collect::<Vec<_>>().join("；")
}

/// 收集開發規範的答案：答案檔 > 互動問答 > 沒有（用 workflow.config 的 METHODOLOGY；`--defaults` 用 default）。
/// 回（profile 名稱, 答案）。`seed` 見 [`ask_methodology`]（只有互動問答用）。
pub fn collect_answers(con: &mut Console, site: &Site, vals: &Kv, answers: Option<&Shown>, interactive: bool, defaults: bool, seed: &Map<String, Value>) -> R<(String, Map<String, Value>)> {
    let mut name = if defaults && answers.is_none() {
        "default".to_string()
    } else {
        vals.get("METHODOLOGY").filter(|s| !s.is_empty()).unwrap_or("default").to_string()
    };
    let mut ans: Map<String, Value> = Map::new();
    if let Some(a) = answers {
        let text = match read_text_sig(&a.real) {
            Ok(t) => t,
            Err(ReadErr::Os(m)) => return die(format!("讀不到答案檔 {}：{m}", a.shown)),
            Err(ReadErr::Decode(m)) => return crash(format!("答案檔 {} 不是 UTF-8：{m}", a.shown)),
        };
        let v = match pyjson::loads(&text) {
            Ok(v) => v,
            Err(e) => return die(format!("讀不到答案檔 {}：{e}", a.shown)),
        };
        let Value::Object(o) = v else { return die(format!("答案檔 {} 要是 JSON 物件", a.shown)) };
        ans = o;
        if let Some(m) = ans.get("methodology").filter(|v| pyfmt::truthy(v)) {
            name = pyfmt::str_of(m);
        }
    } else if interactive {
        let bp = vals.get("BRANCH_PREFIX").unwrap_or_default().to_string();
        (name, ans) = ask_methodology(con, site, &name, &bp, seed)?;
    }
    Ok((name, ans))
}

/// 把答案套進 profile、和 BRANCH_PREFIX 對齊、驗 schema。回專案的開發規範 profile。
pub fn finish_methodology(site: &Site, vals: &mut Kv, name: &str, ans: &Map<String, Value>) -> R<Map<String, Value>> {
    let mut prof = apply_answers(&load_profile(site, name)?, ans)?;
    // 分支前綴：回答了就改 BRANCH_PREFIX（workflow 的分支名用它）；沒回答就讓 profile 跟 BRANCH_PREFIX 一致
    {
        let conv = setdefault_obj(&mut prof, "conventions", "profile")?;
        let branch = setdefault_obj(conv, "branch", "conventions")?;
        if pyfmt::truthy_opt(ans.get("branch_prefix")) {
            let Some(prefix) = branch.get("prefix").and_then(Value::as_str).map(str::to_string) else {
                return crash("conventions.branch.prefix 要是字串");
            };
            vals.set("BRANCH_PREFIX", &prefix);
            let allowed: Vec<String> = vals.get("ALLOWED_BRANCH_PREFIXES").unwrap_or_default().split(',')
                .map(pyutil::strip).filter(|x| !x.is_empty()).map(str::to_string).collect();
            if !allowed.contains(&prefix) {
                let mut all = vec![prefix];
                all.extend(allowed);
                vals.set("ALLOWED_BRANCH_PREFIXES", &all.join(","));
            }
        } else if let Some(bp) = vals.get("BRANCH_PREFIX").filter(|s| !s.is_empty()) {
            branch.insert("prefix".to_string(), json!(bp));
        }
    }
    let final_name = shown_or(prof.get("name"), name);
    vals.set("METHODOLOGY", &final_name);
    let errs = check_methodology(site, &prof)?;
    if !errs.is_empty() {
        return die(format!("開發規範 profile 不符合 schemas/methodology.schema.json：{}", first5(&errs)));
    }
    Ok(prof)
}

/// 比對專案的 methodology.json 與它的來源內建 profile（看 name；不是內建的名稱就比 default），
/// 列出缺少的欄位與預設值；adopt 時只補缺的（既有值不改），補完要過 schema 才寫回。回傳結束碼。
pub fn profile_upgrade(con: &mut Console, site: &Site, meth: &Path, adopt: bool) -> R<i32> {
    let shown = meth.to_string_lossy().into_owned();
    let parsed = match read_text_sig(meth) {
        Ok(t) => pyjson::loads(&t).map_err(|e| e.to_string()),
        Err(ReadErr::Os(m)) => Err(m),
        Err(ReadErr::Decode(m)) => return crash(format!("{shown} 不是 UTF-8：{m}")),
    };
    let proj = match parsed {
        Ok(v) => v,
        Err(e) => {
            con.say(&format!("警告：{shown} 讀不到或不是合法 JSON（{e}），沒有比對 profile 欄位"));
            return Ok(1);
        }
    };
    let Some(proj) = proj.as_object() else {
        con.say(&format!("警告：{shown} 不是 JSON 物件，沒有比對 profile 欄位"));
        return Ok(1);
    };
    let names = profile_names(site);
    let name = match proj.get("name").and_then(Value::as_str) {
        Some(n) if names.iter().any(|x| x == n) => n.to_string(),
        _ => "default".to_string(),
    };
    let base = load_profile(site, &name)?;
    let Some(base) = base.as_object() else { return crash(format!("內建 profile {name} 要是 JSON 物件")) };
    let miss: Vec<(String, Value)> = missing_fields(base, proj, "", true).into_iter().filter(|(p, _)| p != "name" && p != "description").collect();
    if miss.is_empty() {
        con.say(&format!("開發規範 profile：保留 methodology.json，和內建 {name} 比對沒有缺少的欄位"));
    } else {
        con.say(&format!("開發規範 profile：保留 methodology.json；這版內建 {name} 有、你的 profile 沒有的欄位 {} 個：", miss.len()));
        for (p, v) in &miss {
            con.say(&format!("  - {p}：預設 {}", short(v, 160)));
        }
        if !adopt {
            con.say("  加 --adopt-new-fields 補上（只補缺的，既有值不改）；不補時 workflow 照缺省行為走。");
        }
    }
    if adopt && !miss.is_empty() {
        let mut new = proj.clone();
        for (p, v) in &miss {
            set_missing(&mut new, p, v);
        }
        // 讀取上限要不要啟用由使用者決定：補進來的只有建議門檻，沒有 mode（＝不啟用）
        if miss.iter().any(|(p, _)| p == "limits") {
            if let Some(lim) = new.get_mut("limits").and_then(Value::as_object_mut) {
                lim.shift_remove("mode");
                con.say("  limits 只補建議門檻，沒有啟用。要用讀取上限：在 methodology.json 的 limits 加 \"mode\": \"warn\"（只提醒）或 \"enforce\"（擋），門檻自己調。");
            }
        }
        let errs = check_methodology(site, &new)?;
        if !errs.is_empty() {
            con.say(&format!("錯誤：補上欄位後不符合 schemas/methodology.schema.json，沒有寫回：{}", first5(&errs)));
            return Ok(1);
        }
        write_text_native(meth, &format!("{}\n", crate::recipe::dumps(&Value::Object(new), Some(2))))?;
        con.say(&format!("  已補上 {} 個欄位（--adopt-new-fields），既有值沒改。", miss.len()));
        return Ok(0);
    }
    let errs = check_methodology(site, proj)?;
    if !errs.is_empty() {
        con.say(&format!("警告：你的 methodology.json 不符合這版 schema（awf build 會失敗）：{}", first5(&errs)));
        return Ok(1);
    }
    Ok(0)
}
