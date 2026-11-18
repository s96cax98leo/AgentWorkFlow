//! 用資料宣告的步驟（profile 的 `flow.stages[].steps` 與 `events.<名>.steps`）：規則、要由工具執行的指令、每一步該有的交件。
//!
//! 引擎在 JS 端（`plugin/workflow-templates/snippets/steps.js`，由 `flow-steps.js` 與 `event.js` 引用）；這裡是工具端的同一套規則：
//!
//! - [`errors`]：定義本身合不合規則（上限、角色、指令的形狀）。要和片段的 `awfStepsErrors` 一致；這裡另外擋不認得的欄位。
//! - [`commands`]：`check` 與 loop 的 `until` 指令。指令只來自 profile，文字原樣使用、不代入任何參數。
//! - [`trace_issues`]：照定義算出「每一步該有哪些交件」，對 workflow 回傳的 trace 核對（不採信回傳自己說的哪一步做完了）。
//!
//! 上限寫死，不開放調整（調大等於放寬派工數的硬上限）。

use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_STEPS: usize = 12;
pub const MAX_ANGLES: usize = 8;
pub const MAX_LOOP_STEPS: usize = 4;
pub const MAX_ROUNDS: u64 = 5;
pub const MAX_BATCH: u64 = 50;
pub const MAX_BATCHES: usize = 8;
pub const MAX_DATA: usize = 8;
pub const MAX_TEXT: usize = 4000;
pub const MAX_COMMAND: usize = 1000;
/// 指令沒寫 `timeout` 時的逾時（秒），和限制文件的檢查指令相同。
pub const DEFAULT_TIMEOUT: u64 = 300;
/// 唯讀步驟（fanout、challenge、loop 的判定）可以用的模版角色：都沒有 Edit，而且有 Bash。要和片段的 `awfStepsRoles` 一致。
pub const ROLES: [&str; 6] = ["tpm-researcher", "tpm-skeptic", "tpm-reviewer", "dev-verifier", "dev-design-reviewer", "dev-security-reviewer"];
pub const WRITER: &str = "tpm-writer";
const FANOUT_ROLE: &str = "tpm-researcher";
const CHALLENGE_ROLE: &str = "tpm-skeptic";
const UNTIL_ROLE: &str = "dev-verifier";

/// 步驟與角度的 id：小寫英數與連字號、英文字母開頭、32 字以內。
pub fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= 32 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn agent_part_ok(p: &str) -> bool {
    let b = p.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// 是不是模版角色的名字（最後一段以 `tpm-`／`dev-` 開頭）。
pub fn is_template(agent: &str) -> bool {
    let last = agent.rsplit(':').next().unwrap_or_default();
    last.starts_with("tpm-") || last.starts_with("dev-")
}

/// `role` 的值：[`ROLES`] 裡的角色名（不帶命名空間），或使用者自己的 agent（完整的 agent_type，不能是模版角色）。
pub fn role_ok(role: &str) -> bool {
    if ROLES.contains(&role) {
        return true;
    }
    let mut parts = role.splitn(2, ':');
    let (a, b) = (parts.next().unwrap_or_default(), parts.next());
    let shape = match b {
        Some(b) => agent_part_ok(a) && agent_part_ok(b),
        None => agent_part_ok(a),
    };
    shape && !is_template(role)
}

/// 派工數上限：profile 的 `limits.max_agents`（1–1000 的整數）；沒寫或不合法用 40。和 challenge 片段的 `awfMaxAgents` 相同。
pub fn max_agents(profile: &Value) -> usize {
    let v = profile.get("limits").and_then(|l| l.get("max_agents"));
    let n = v.and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)));
    n.filter(|n| (1..=1000).contains(n)).unwrap_or(40) as usize
}

fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

fn text_ok(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty() && len16(t) <= MAX_TEXT)
}

fn line_ok(v: Option<&Value>, max: usize) -> bool {
    v.and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty() && len16(t) <= max && !t.chars().any(|c| c.is_control()))
}

fn int_in(v: Option<&Value>, lo: u64, hi: u64) -> Option<u64> {
    let v = v?;
    let n = v.as_u64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64))?;
    (lo..=hi).contains(&n).then_some(n)
}

fn timeout_ok(v: Option<&Value>) -> bool {
    v.is_none() || int_in(v, 1, 3600).is_some()
}

fn role_field_ok(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::String(r)) => role_ok(r),
        Some(_) => false,
    }
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "agent".to_string() } else { t }
}

struct Check<'a> {
    errs: Vec<String>,
    kinds: Vec<(&'a str, &'a str)>,
    labels: Vec<String>,
}

impl<'a> Check<'a> {
    fn kind_of(&self, id: &str) -> Option<&'a str> {
        self.kinds.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
    }

    fn unknown(&mut self, w: &str, o: &Map<String, Value>, allowed: &[&str]) {
        for k in o.keys().filter(|k| !["id", "kind", "heading"].contains(&k.as_str()) && !allowed.contains(&k.as_str())) {
            self.errs.push(format!("{w} 不認得的欄位：{k}"));
        }
    }

    fn data(&mut self, w: &str, d: Option<&Value>) {
        let Some(d) = d else { return };
        let Some(list) = d.as_array().filter(|a| a.len() <= MAX_DATA) else {
            self.errs.push(format!("{w}.data 要是陣列（最多 {MAX_DATA} 筆）"));
            return;
        };
        for x in list {
            if !x.is_object() || !line_ok(x.get("label"), 40) {
                self.errs.push(format!("{w}.data 的每一筆要有 label（一行、40 字以內）"));
                continue;
            }
            let from = x.get("from").and_then(Value::as_str).unwrap_or_default();
            let mut it = from.split('.');
            let (id, part, more) = (it.next().unwrap_or_default(), it.next(), it.next());
            let ok = more.is_none()
                && match self.kind_of(id) {
                    Some("fanout") => part.is_none(),
                    Some("challenge") => matches!(part, Some("kept" | "doubted")),
                    Some("loop") => part == Some("until"),
                    _ => false,
                };
            if !ok {
                self.errs.push(format!("{w}.data 的 from「{from}」指不到前面的步驟（fanout 寫 <id>；challenge 寫 <id>.kept 或 <id>.doubted；loop 寫 <id>.until）"));
            }
        }
    }

    /// 回這一步最多可能派幾個 agent。
    fn one(&mut self, s: &'a Value, place: &str, pre: &str, in_loop: bool) -> usize {
        let Some(o) = s.as_object() else {
            self.errs.push(format!("{place} 要是物件"));
            return 0;
        };
        let id = o.get("id").and_then(Value::as_str).filter(|i| id_ok(i) && *i != "until");
        let w = id.map_or_else(|| place.to_string(), |i| format!("步驟 {i}"));
        let kind = o.get("kind").and_then(Value::as_str).unwrap_or_default();
        let fresh = match id {
            None => {
                self.errs.push(format!("{place}.id 要是小寫英數與連字號、英文字母開頭、32 字以內，而且不能是 until"));
                false
            }
            Some(i) if self.kind_of(i).is_some() => {
                self.errs.push(format!("步驟的 id「{i}」重複"));
                false
            }
            Some(_) => true,
        };
        if o.contains_key("heading") && !line_ok(o.get("heading"), 200) {
            self.errs.push(format!("{w}.heading 要是一行字（200 字以內）"));
        }
        let roles = ROLES.join("、");
        let sid = id.unwrap_or_default();
        let n = match kind {
            "fanout" => {
                self.unknown(&w, o, &["angles", "body", "role", "data"]);
                let angles = o.get("angles").and_then(Value::as_array).filter(|a| !a.is_empty() && a.len() <= MAX_ANGLES);
                match angles {
                    None => self.errs.push(format!("{w}.angles 要有 1～{MAX_ANGLES} 個角度")),
                    Some(a) => {
                        let good = |x: &Value| {
                            x.as_object().is_some_and(|ao| ao.get("key").and_then(Value::as_str).is_some_and(id_ok) && text_ok(ao.get("ask")) && ao.values().all(Value::is_string))
                        };
                        if !a.iter().all(good) {
                            self.errs.push(format!("{w} 的每個角度都要有 key（小寫英數與連字號）與 ask，其他欄位只能是字串"));
                        }
                        let keys: Vec<&str> = a.iter().filter_map(|x| x.get("key").and_then(Value::as_str)).collect();
                        if keys.iter().collect::<BTreeSet<_>>().len() != a.len() {
                            self.errs.push(format!("{w} 的角度 key 不能重複"));
                        }
                        self.labels.extend(keys.iter().map(|k| format!("{pre}{sid}:{k}")));
                    }
                }
                if o.contains_key("body") && !text_ok(o.get("body")) {
                    self.errs.push(format!("{w}.body 要是字串（{MAX_TEXT} 字以內）"));
                }
                if !role_field_ok(o.get("role")) {
                    self.errs.push(format!("{w}.role 只能是 {roles}，或登記過的擴充 agent"));
                }
                self.data(&w, o.get("data"));
                angles.map_or(0, Vec::len)
            }
            "challenge" => {
                self.unknown(&w, o, &["ask", "batch", "optional", "from", "role"]);
                if !text_ok(o.get("ask")) {
                    self.errs.push(format!("{w}.ask 必填"));
                }
                if o.contains_key("batch") && int_in(o.get("batch"), 1, MAX_BATCH).is_none() {
                    self.errs.push(format!("{w}.batch 要是 1～{MAX_BATCH} 的整數"));
                }
                if o.get("optional").is_some_and(|v| !v.is_boolean()) {
                    self.errs.push(format!("{w}.optional 要是 true 或 false"));
                }
                match o.get("from") {
                    None => {
                        if !self.kinds.iter().any(|(_, k)| *k == "fanout") {
                            self.errs.push(format!("{w} 前面沒有 fanout 步驟，沒有東西可以反駁"));
                        }
                    }
                    Some(f) => {
                        let ok = f.as_array().is_some_and(|a| !a.is_empty() && a.len() <= MAX_STEPS && a.iter().all(|x| x.as_str().is_some_and(|x| self.kind_of(x) == Some("fanout"))));
                        if !ok {
                            self.errs.push(format!("{w}.from 要是前面 fanout 步驟的 id"));
                        }
                    }
                }
                if !role_field_ok(o.get("role")) {
                    self.errs.push(format!("{w}.role 只能是 {roles}，或登記過的擴充 agent"));
                }
                self.labels.push(format!("{pre}{sid}"));
                if o.contains_key("batch") {
                    self.labels.push(format!("{pre}{sid}:b1"));
                    MAX_BATCHES
                } else {
                    1
                }
            }
            "write" => {
                self.unknown(&w, o, &["ask", "data", "owner_check", "role"]);
                if !text_ok(o.get("ask")) {
                    self.errs.push(format!("{w}.ask 必填"));
                }
                if o.contains_key("role") {
                    self.errs.push(format!("{w}：write 固定由 {WRITER} 做，不能寫 role"));
                }
                if o.get("owner_check").is_some_and(|v| !v.is_boolean()) {
                    self.errs.push(format!("{w}.owner_check 要是 true 或 false"));
                }
                self.data(&w, o.get("data"));
                self.labels.push(format!("{pre}{sid}"));
                1
            }
            "loop" => {
                if in_loop {
                    self.errs.push(format!("{w}：loop 裡不能再放 loop"));
                    return 0;
                }
                self.unknown(&w, o, &["max_rounds", "until", "steps"]);
                // 先佔住 id（裡面的步驟不能和它同名，也不能引用還沒跑完的自己）
                let slot = fresh.then(|| {
                    self.kinds.push((sid, "pending"));
                    self.kinds.len() - 1
                });
                let rounds = int_in(o.get("max_rounds"), 1, MAX_ROUNDS);
                if rounds.is_none() {
                    self.errs.push(format!("{w}.max_rounds 必填，要是 1～{MAX_ROUNDS} 的整數"));
                }
                match o.get("until").and_then(Value::as_object).filter(|u| line_ok(u.get("command"), MAX_COMMAND)) {
                    None => self.errs.push(format!("{w}.until.command 必填：一行的指令（{MAX_COMMAND} 字以內）")),
                    Some(u) => {
                        for k in u.keys().filter(|k| !["command", "timeout", "role"].contains(&k.as_str())) {
                            self.errs.push(format!("{w}.until 不認得的欄位：{k}"));
                        }
                        if !timeout_ok(u.get("timeout")) {
                            self.errs.push(format!("{w}.until.timeout 要是 1～3600 的整數（秒）"));
                        }
                        if !role_field_ok(u.get("role")) {
                            self.errs.push(format!("{w}.until.role 只能是 {roles}，或登記過的擴充 agent"));
                        }
                    }
                }
                let mut inner = 0;
                match o.get("steps").and_then(Value::as_array).filter(|a| !a.is_empty() && a.len() <= MAX_LOOP_STEPS) {
                    None => self.errs.push(format!("{w}.steps 要有 1～{MAX_LOOP_STEPS} 步")),
                    Some(a) => {
                        let p = format!("{sid}.r1:");
                        for (i, x) in a.iter().enumerate() {
                            inner += self.one(x, &format!("{w}.steps[{i}]"), &p, true);
                        }
                    }
                }
                self.labels.push(format!("{sid}.r1:until"));
                if let Some(i) = slot {
                    self.kinds[i].1 = "loop";
                }
                (rounds.unwrap_or(MAX_ROUNDS) as usize) * (inner + 1)
            }
            "check" => {
                if in_loop {
                    self.errs.push(format!("{w}：check 不能放在 loop 裡（loop 的結束條件寫在 until）"));
                    return 0;
                }
                self.unknown(&w, o, &["command", "timeout"]);
                if !line_ok(o.get("command"), MAX_COMMAND) {
                    self.errs.push(format!("{w}.command 必填：一行的指令（{MAX_COMMAND} 字以內）"));
                }
                if !timeout_ok(o.get("timeout")) {
                    self.errs.push(format!("{w}.timeout 要是 1～3600 的整數（秒）"));
                }
                0
            }
            _ => {
                self.errs.push(format!("{w}.kind 要是 fanout、challenge、write、loop、check 其中之一"));
                0
            }
        };
        if fresh && ["fanout", "challenge", "write", "check"].contains(&kind) {
            self.kinds.push((sid, kind));
        }
        n
    }
}

/// 步驟定義不合規則的地方（空清單＝可以跑）。`who`＝錯誤訊息的開頭（例如 `flow.stages[uat].steps`）；`cap`＝派工數上限（[`max_agents`]）。
pub fn errors(steps: &Value, who: &str, cap: usize) -> Vec<String> {
    let Some(list) = steps.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_STEPS) else {
        return vec![format!("{who} 要是 1～{MAX_STEPS} 步的陣列")];
    };
    let mut c = Check { errs: Vec::new(), kinds: Vec::new(), labels: Vec::new() };
    let mut total = 0;
    for (i, s) in list.iter().enumerate() {
        total += c.one(s, &format!("steps[{i}]"), "", false);
    }
    if c.errs.is_empty() {
        let slugs: Vec<String> = c.labels.iter().map(|l| slug(l)).collect();
        let clash: BTreeSet<&String> = slugs.iter().enumerate().filter(|(i, x)| slugs.iter().position(|y| y == *x) != Some(*i)).map(|(_, x)| x).collect();
        if !clash.is_empty() {
            c.errs.push(format!("步驟與角度的名字組起來會撞名（紀錄檔 {}）：換一個 id 或 key", clash.into_iter().cloned().collect::<Vec<_>>().join("、")));
        }
        if total > cap {
            c.errs.push(format!("這份定義最多可能派 {total} 個 agent，超過上限 {cap}（limits.max_agents）：減少角度、輪數或步驟，或調高上限"));
        }
    }
    c.errs.into_iter().map(|e| format!("{who}：{e}")).collect()
}

/// 要由工具執行的一個指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    /// 步驟的 id。
    pub id: String,
    /// `check` 或 `until`（loop 的結束條件）。
    pub kind: &'static str,
    pub command: String,
    pub timeout: u64,
}

/// 這份定義裡由工具執行的指令（`check` 與 loop 的 `until`），照宣告順序。指令文字原樣取自定義，不代入任何東西。
pub fn commands(steps: &Value) -> Vec<Cmd> {
    let mut out = Vec::new();
    for s in steps.as_array().into_iter().flatten() {
        let id = s.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let (kind, holder) = match s.get("kind").and_then(Value::as_str) {
            Some("check") => ("check", Some(s)),
            Some("loop") => ("until", s.get("until")),
            _ => continue,
        };
        let Some(h) = holder else { continue };
        let Some(cmd) = h.get("command").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) else { continue };
        out.push(Cmd { id, kind, command: cmd.to_string(), timeout: int_in(h.get("timeout"), 1, 3600).unwrap_or(DEFAULT_TIMEOUT) });
    }
    out
}

/// 定義裡換上去的使用者自己的 agent（`role` 不是 [`ROLES`] 裡的角色名），不重複、照出現順序。要登記在 guard.config.json 的 `extension_agents`。
pub fn custom_agents(steps: &Value) -> Vec<String> {
    fn walk(list: Option<&Value>, out: &mut Vec<String>) {
        for s in list.and_then(Value::as_array).into_iter().flatten() {
            let mut put = |v: Option<&Value>| {
                if let Some(r) = v.and_then(Value::as_str).filter(|r| !ROLES.contains(r) && !r.is_empty()) {
                    if !out.iter().any(|x| x == r) {
                        out.push(r.to_string());
                    }
                }
            };
            put(s.get("role"));
            put(s.get("until").and_then(|u| u.get("role")));
            walk(s.get("steps"), out);
        }
    }
    let mut out = Vec::new();
    walk(Some(steps), &mut out);
    out
}

/// 一份定義的指紋（sha1；`awf next` 產生腳本時記下、政策核准時重算，定義在跑完之後改過就對不上）。
pub fn def_sha(def: &Value) -> String {
    Sha1::digest(serde_json::to_string(def).unwrap_or_default().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn agent_matches(agent: &str, spec: Option<&str>, dflt: &str) -> bool {
    match spec {
        Some(r) if !ROLES.contains(&r) => agent == r,
        other => agent.rsplit(':').next() == Some(other.unwrap_or(dflt)),
    }
}

struct Trace {
    /// label → agent_type（ok=true 的派工）。
    done: BTreeMap<String, String>,
    used: BTreeSet<String>,
    issues: Vec<String>,
}

impl Trace {
    fn has(&self, label: &str) -> bool {
        self.done.contains_key(label)
    }

    fn need(&mut self, step: &str, label: &str, spec: Option<&str>, dflt: &str) {
        match self.done.get(label) {
            None => self.issues.push(format!("步驟 {step} 沒有交件：trace 裡沒有交件的 {label}")),
            Some(agent) => {
                if !agent_matches(agent, spec, dflt) {
                    self.issues.push(format!("步驟 {step} 的 {label} 是 {agent} 交的，照定義應該是 {}", spec.unwrap_or(dflt)));
                }
                self.used.insert(label.to_string());
            }
        }
    }
}

/// 回傳裡某個 fanout 步驟交出的發現數（`steps.<id>.results` 的信封）；讀不出來回 None。
fn findings_of(result: &Value, id: &str) -> Option<usize> {
    let rs = result.get("steps")?.get(id)?.get("results")?.as_array()?;
    let mut n = 0;
    for r in rs {
        n += r.get("payload")?.get("findings")?.as_array()?.len();
    }
    Some(n)
}

fn fanouts_before(steps: &[Value], target: &str) -> Vec<String> {
    fn walk(list: &[Value], target: &str, fans: &mut Vec<String>) -> bool {
        for s in list {
            let id = s.get("id").and_then(Value::as_str).unwrap_or_default();
            match s.get("kind").and_then(Value::as_str) {
                Some("fanout") => fans.push(id.to_string()),
                Some("challenge") if id == target => return true,
                Some("loop") => {
                    if walk(s.get("steps").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), target, fans) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    let mut fans = Vec::new();
    walk(steps, target, &mut fans);
    fans
}

/// 一個不在 loop 裡（`pre` 是空的）或 loop 某一輪裡的步驟。`exact`＝回傳裡留著這一輪的結果，可以核對反駁的批數。
fn expect_step(t: &mut Trace, all: &[Value], s: &Value, pre: &str, exact: bool, result: &Value) {
    let id = s.get("id").and_then(Value::as_str).unwrap_or_default();
    let role = s.get("role").and_then(Value::as_str);
    match s.get("kind").and_then(Value::as_str) {
        Some("fanout") => {
            for a in s.get("angles").and_then(Value::as_array).into_iter().flatten() {
                let key = a.get("key").and_then(Value::as_str).unwrap_or_default();
                t.need(id, &format!("{pre}{id}:{key}"), role, FANOUT_ROLE);
            }
        }
        Some("write") => t.need(id, &format!("{pre}{id}"), None, WRITER),
        Some("challenge") => {
            let plain = format!("{pre}{id}");
            let optional = s.get("optional") == Some(&Value::Bool(true));
            let batch = int_in(s.get("batch"), 1, MAX_BATCH);
            if !exact {
                // 前面幾輪的結果不在回傳裡：只要求有交件（或這一步可以跳過）
                let mut n = 0;
                while t.has(&format!("{plain}:b{}", n + 1)) {
                    n += 1;
                    t.need(id, &format!("{plain}:b{n}"), role, CHALLENGE_ROLE);
                }
                if t.has(&plain) {
                    t.need(id, &plain, role, CHALLENGE_ROLE);
                } else if n == 0 && !optional {
                    t.issues.push(format!("步驟 {id} 沒有交件：trace 裡沒有交件的 {plain}"));
                }
                return;
            }
            let from: Vec<String> = match s.get("from").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                None => fanouts_before(all, id),
            };
            let counts: Option<Vec<usize>> = from.iter().map(|f| findings_of(result, f)).collect();
            let Some(targets) = counts.map(|c| c.iter().sum::<usize>()) else {
                t.issues.push(format!("步驟 {id}：回傳裡讀不到前面 fanout 步驟的結果，無法核對有沒有該反駁的發現"));
                return;
            };
            if targets == 0 {
                if !optional {
                    t.issues.push(format!("步驟 {id}：沒有東西可以反駁，而且這一步不是 optional（應該停下交人）"));
                }
                return;
            }
            let n = batch.map_or(1, |b| targets.div_ceil(b as usize));
            if n == 1 {
                t.need(id, &plain, role, CHALLENGE_ROLE);
            } else {
                for i in 1..=n {
                    t.need(id, &format!("{plain}:b{i}"), role, CHALLENGE_ROLE);
                }
            }
        }
        _ => {}
    }
}

/// 照定義核對 workflow 回傳的 trace：每一步都要有交件（ok=true、角色對得上），不能有定義以外的派工；
/// loop 要每一輪都做完、最後一輪的判定回報通過。空清單＝對得上。只看 trace 與信封，不採信回傳自己標的完成狀態。
pub fn trace_issues(steps: &Value, result: &Value) -> Vec<String> {
    let Some(list) = steps.as_array() else { return vec!["步驟定義讀不懂（不是陣列）".to_string()] };
    let Some(trace) = result.get("trace").and_then(Value::as_array) else { return vec!["回傳沒有 trace，無法核對每一步的交件".to_string()] };
    let mut t = Trace { done: BTreeMap::new(), used: BTreeSet::new(), issues: Vec::new() };
    for (i, e) in trace.iter().enumerate() {
        let label = e.get("label").and_then(Value::as_str).unwrap_or_default().to_string();
        let agent = e.get("agent_type").and_then(Value::as_str).unwrap_or_default().to_string();
        if e.get("ok") == Some(&Value::Bool(true)) {
            if t.done.insert(label.clone(), agent).is_some() {
                t.issues.push(format!("trace 裡 {label} 交件了兩次"));
            }
            continue;
        }
        let retried = e.get("superseded") == Some(&Value::Bool(true))
            && trace.iter().skip(i + 1).any(|x| x.get("ok") == Some(&Value::Bool(true)) && x.get("label").and_then(Value::as_str) == Some(label.as_str()) && x.get("agent_type").and_then(Value::as_str) == Some(agent.as_str()));
        if !retried {
            t.issues.push(format!("{label} 沒有交件（失敗、交件無效，或派工數到上限）"));
        }
    }
    for s in list {
        let id = s.get("id").and_then(Value::as_str).unwrap_or_default();
        if s.get("kind").and_then(Value::as_str) != Some("loop") {
            expect_step(&mut t, list, s, "", true, result);
            continue;
        }
        let max = int_in(s.get("max_rounds"), 1, MAX_ROUNDS).unwrap_or(0);
        let mut rounds = 0u64;
        while t.has(&format!("{id}.r{}:until", rounds + 1)) {
            rounds += 1;
        }
        if rounds == 0 {
            t.issues.push(format!("步驟 {id}（loop）沒有任何一輪做完：trace 裡沒有交件的 {id}.r1:until"));
            continue;
        }
        if rounds > max {
            t.issues.push(format!("步驟 {id}（loop）跑了 {rounds} 輪，超過定義的 max_rounds（{max}）"));
        }
        let until_role = s.get("until").and_then(|u| u.get("role")).and_then(Value::as_str);
        for r in 1..=rounds {
            let pre = format!("{id}.r{r}:");
            for x in s.get("steps").and_then(Value::as_array).into_iter().flatten() {
                expect_step(&mut t, list, x, &pre, r == rounds, result);
            }
            t.need(id, &format!("{pre}until"), until_role, UNTIL_ROLE);
        }
        let got = result.get("steps").and_then(|x| x.get(id));
        let crit = got.and_then(|g| g.get("until")).and_then(|u| u.get("payload")).and_then(|p| p.get("criteria")).and_then(Value::as_array);
        let passed = crit.is_some_and(|c| !c.is_empty() && c.iter().all(|x| x.get("result").and_then(Value::as_str) == Some("pass")));
        if !passed || got.and_then(|g| g.get("passed")) != Some(&Value::Bool(true)) {
            t.issues.push(format!("步驟 {id}（loop）最後一輪的判定沒有回報通過"));
        }
        if got.and_then(|g| g.get("rounds")).and_then(Value::as_u64) != Some(rounds) {
            t.issues.push(format!("步驟 {id}（loop）回傳記的輪數和 trace 的 {rounds} 輪對不上"));
        }
    }
    let extra: Vec<&String> = t.done.keys().filter(|l| !t.used.contains(*l)).collect();
    if !extra.is_empty() {
        t.issues.push(format!("trace 有定義以外的派工：{}（定義在跑完之後改過，或結果不是這份定義跑出來的）", extra.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join("、")));
    }
    t.issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!([
            {"id": "survey", "kind": "fanout", "angles": [{"key": "api", "ask": "看 API"}, {"key": "ui", "ask": "看畫面"}]},
            {"id": "refute", "kind": "challenge", "ask": "逐則反駁", "batch": 2, "optional": true},
            {"id": "fix", "kind": "loop", "max_rounds": 3, "until": {"command": "true", "timeout": 30}, "steps": [{"id": "report", "kind": "write", "ask": "寫報告"}]},
            {"id": "lint", "kind": "check", "command": "echo ok"},
        ])
    }

    fn env(payload: Value) -> Value {
        json!({"status": "done", "summary": "s", "payload": payload})
    }

    fn row(label: &str, role: &str) -> Value {
        json!({"label": label, "agent_type": format!("agent-workflow:{role}"), "ok": true})
    }

    fn good_result() -> Value {
        let f = |n: usize| env(json!({"lens": "x", "findings": (0..n).map(|i| json!({"id": format!("f{i}")})).collect::<Vec<_>>()}));
        json!({
            "steps": {
                "survey": {"kind": "fanout", "results": [f(2), f(1)]},
                "fix": {"kind": "loop", "rounds": 2, "passed": true, "until": env(json!({"criteria": [{"id": "until", "result": "pass"}]}))},
            },
            "trace": [
                row("survey:api", "tpm-researcher"), row("survey:ui", "tpm-researcher"),
                row("refute:b1", "tpm-skeptic"), row("refute:b2", "tpm-skeptic"),
                row("fix.r1:report", "tpm-writer"), row("fix.r1:until", "dev-verifier"),
                row("fix.r2:report", "tpm-writer"), row("fix.r2:until", "dev-verifier"),
            ],
        })
    }

    #[test]
    fn valid_definition_and_commands() {
        assert_eq!(errors(&sample(), "steps", 40), Vec::<String>::new());
        let c = commands(&sample());
        assert_eq!(c.iter().map(|x| (x.id.as_str(), x.kind, x.command.as_str(), x.timeout)).collect::<Vec<_>>(), [("fix", "until", "true", 30), ("lint", "check", "echo ok", 300)]);
        assert!(custom_agents(&sample()).is_empty());
    }

    #[test]
    fn limits_and_shapes() {
        let bad = |v: Value, want: &str| {
            let e = errors(&v, "s", 40);
            assert!(e.iter().any(|m| m.contains(want)), "要有「{want}」：{e:?}");
        };
        bad(json!([]), "1～12 步");
        bad(json!((0..13).map(|i| json!({"id": format!("c{i}"), "kind": "check", "command": "true"})).collect::<Vec<_>>()), "1～12 步");
        bad(json!([{"id": "a", "kind": "fanout", "angles": (0..9).map(|i| json!({"key": format!("k{i}"), "ask": "x"})).collect::<Vec<_>>()}]), "1～8 個角度");
        bad(json!([{"id": "a", "kind": "loop", "max_rounds": 6, "until": {"command": "true"}, "steps": [{"id": "w", "kind": "write", "ask": "x"}]}]), "max_rounds");
        bad(json!([{"id": "a", "kind": "loop", "until": {"command": "true"}, "steps": [{"id": "w", "kind": "write", "ask": "x"}]}]), "max_rounds 必填");
        bad(json!([{"id": "a", "kind": "loop", "max_rounds": 2, "until": {"command": "true"}, "steps": [{"id": "b", "kind": "loop", "max_rounds": 1, "until": {"command": "true"}, "steps": []}]}]), "不能再放 loop");
        bad(json!([{"id": "a", "kind": "loop", "max_rounds": 2, "until": {"command": "true"}, "steps": (0..5).map(|i| json!({"id": format!("w{i}"), "kind": "write", "ask": "x"})).collect::<Vec<_>>()}]), "1～4 步");
        bad(json!([{"id": "a", "kind": "loop", "max_rounds": 2, "until": {"command": "a\nb"}, "steps": [{"id": "w", "kind": "write", "ask": "x"}]}]), "until.command");
        bad(json!([{"id": "a", "kind": "check", "command": ""}]), "command 必填");
        bad(json!([{"id": "a", "kind": "check", "command": "true", "timeout": 0}]), "timeout");
        bad(json!([{"id": "a", "kind": "write", "ask": "x", "role": "tpm-writer"}]), "不能寫 role");
        bad(json!([{"id": "a", "kind": "fanout", "role": "dev-implementer", "angles": [{"key": "k", "ask": "x"}]}]), "role 只能是");
        bad(json!([{"id": "a", "kind": "fanout", "role": "agent-workflow:tpm-researcher", "angles": [{"key": "k", "ask": "x"}]}]), "role 只能是");
        bad(json!([{"id": "a", "kind": "challenge", "ask": "x"}]), "沒有東西可以反駁");
        bad(json!([{"id": "a", "kind": "fanout", "angles": [{"key": "k", "ask": "x"}]}, {"id": "a", "kind": "write", "ask": "x"}]), "重複");
        bad(json!([{"id": "a", "kind": "write", "ask": "x", "surprise": 1}]), "不認得的欄位");
        bad(json!([{"id": "a", "kind": "write", "ask": "x", "data": [{"label": "l", "from": "nope"}]}]), "指不到前面的步驟");
        bad(json!([{"id": "until", "kind": "write", "ask": "x"}]), "不能是 until");
        bad(json!([{"id": "a-b", "kind": "fanout", "angles": [{"key": "c", "ask": "x"}]}, {"id": "a", "kind": "fanout", "angles": [{"key": "b-c", "ask": "x"}]}]), "撞名");
        // 最多可能的派工數：5 輪 ×（8 個角度＋判定）＝45 > 40
        let big = json!([{"id": "l", "kind": "loop", "max_rounds": 5, "until": {"command": "true"}, "steps": [{"id": "f", "kind": "fanout", "angles": (0..8).map(|i| json!({"key": format!("k{i}"), "ask": "x"})).collect::<Vec<_>>()}]}]);
        bad(big.clone(), "最多可能派 45 個");
        assert!(errors(&big, "s", 45).is_empty());
        // 使用者自己的 agent 可以寫，名字會列出來給產生腳本的地方核對登記
        let ext = json!([{"id": "a", "kind": "fanout", "role": "acme:perf-bot", "angles": [{"key": "k", "ask": "x"}]}]);
        assert!(errors(&ext, "s", 40).is_empty());
        assert_eq!(custom_agents(&ext), ["acme:perf-bot"]);
    }

    #[test]
    fn trace_matches_definition() {
        assert_eq!(trace_issues(&sample(), &good_result()), Vec::<String>::new());
    }

    #[test]
    fn trace_mismatches_are_reported() {
        let drop = |label: &str| {
            let mut r = good_result();
            r["trace"].as_array_mut().unwrap().retain(|t| t["label"] != label);
            trace_issues(&sample(), &r)
        };
        assert!(drop("survey:ui")[0].contains("survey:ui"));
        assert!(drop("refute:b2").iter().any(|m| m.contains("refute:b2")), "三則、每批兩則要有兩批");
        assert!(drop("fix.r2:report").iter().any(|m| m.contains("fix.r2:report")));
        assert!(drop("fix.r2:until").iter().any(|m| m.contains("輪數")), "最後一輪的判定不見了，輪數對不上");
        // 判定回報沒過
        let mut r = good_result();
        r["steps"]["fix"]["until"]["payload"]["criteria"][0]["result"] = json!("fail");
        assert!(trace_issues(&sample(), &r).iter().any(|m| m.contains("沒有回報通過")));
        // 角色不對
        let mut r = good_result();
        r["trace"][0]["agent_type"] = json!("agent-workflow:dev-implementer");
        assert!(trace_issues(&sample(), &r).iter().any(|m| m.contains("照定義應該是 tpm-researcher")));
        // 定義以外的派工、沒交件的派工
        let mut r = good_result();
        r["trace"].as_array_mut().unwrap().push(row("extra", "tpm-writer"));
        r["trace"].as_array_mut().unwrap().push(json!({"label": "x", "agent_type": "a", "ok": false}));
        let e = trace_issues(&sample(), &r);
        assert!(e.iter().any(|m| m.contains("定義以外的派工：extra")) && e.iter().any(|m| m.contains("x 沒有交件")), "{e:?}");
        // 沒有發現時，optional 的反駁可以不派；不是 optional 就不行
        let mut r = good_result();
        r["steps"]["survey"]["results"] = json!([env(json!({"lens": "x", "findings": []}))]);
        r["trace"].as_array_mut().unwrap().retain(|t| !t["label"].as_str().unwrap().starts_with("refute"));
        assert_eq!(trace_issues(&sample(), &r), Vec::<String>::new());
        let mut strict = sample();
        strict[1].as_object_mut().unwrap().remove("optional");
        assert!(trace_issues(&strict, &r).iter().any(|m| m.contains("不是 optional")));
        assert!(trace_issues(&sample(), &json!({})).iter().any(|m| m.contains("沒有 trace")));
    }

    #[test]
    fn roles_and_caps() {
        for ok in ["tpm-skeptic", "dev-verifier", "perf-bot", "acme:perf-bot"] {
            assert!(role_ok(ok), "{ok}");
        }
        for bad in ["tpm-writer", "dev-implementer", "dev-merge-steward", "agent-workflow:tpm-skeptic", "", "a:b:c", "x y"] {
            assert!(!role_ok(bad), "{bad}");
        }
        assert_eq!(max_agents(&Value::Null), 40);
        assert_eq!(max_agents(&json!({"limits": {"max_agents": 12}})), 12);
        assert_eq!(max_agents(&json!({"limits": {"max_agents": 0}})), 40);
        assert_ne!(def_sha(&json!({"a": 1})), def_sha(&json!({"a": 2})));
    }
}
