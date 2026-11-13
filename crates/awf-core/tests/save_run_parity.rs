//! 閘門工具（save_run）的對跑：照 Python 版錄下來的劇本（tests/parity/save_run.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作、指令（比 exit code、stdout、stderr 與這一步改了哪些檔）和只能在程式內呼叫的函式（call）。
//! `probes` 是不碰檔案的函式探測。暫存路徑換成 `@TMP@` 再比。
//!
//! - 不切換行程的工作目錄：相對路徑的基準用 `Ctx.cwd` 給（Python 錄的時候是切到暫存目錄）。
//! - save_run 不呼叫 git，外面的 `GIT_AUTHOR_*`／`GIT_COMMITTER_*` 影響不到；唯一的子程序是限制文件的檢查指令（劇本裡只用 shell 內建的指令）。
//! - 時間：唯一自己取時間的地方（決策記憶.md 判斷過期）用 `Ctx.today` 固定成劇本的 `today`。
//!
//! 和 Python 不同的步驟分兩種，劇本裡帶 label，其餘要逐字相同：
//! - [`STRICTER`]：Python 接受（或照常處理）、這裡拒絕。只會更嚴。
//! - [`KNOWN`]：兩邊都拒絕，只是文字或例外類別不同。
//!
//! 兩張表都有「沒有過時」的檢查：表上的 label 在劇本裡要真的有一步和 Python 不同，劇本裡的 label 也都要在表上。
#![cfg(unix)]

use awf_core::save_run::{self as sr, Ctx, PyErr};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// 比這長的檔案內容，劇本裡只記 sha1（和錄製腳本的 LONG 相同）。
const LONG: usize = 1500;

/// Python 接受、這裡拒絕的步驟：標籤、原因。要求：這裡的 exit code 不是 0、stdout 沒有「狀態 done」；
/// `stage` 以外的指令（核准、結案、記決定…）不能改到任何檔，`stage` 只能把該段記成 invalid。
const STRICTER: [(&str, &str); 8] = [
    ("nan-result", "workflow 回傳有 NaN／Infinity／1e999：Python 讀得進來、照常存檔；這裡當成讀不懂，不存檔、不改 run.json"),
    ("huge-int-result", "workflow 回傳有超出 64 位元範圍的整數：Python 照常存檔；這裡當成讀不懂"),
    ("deep-result", "workflow 回傳巢狀超過 200 層：Python 照常處理（判缺件）；這裡當成讀不懂"),
    ("nan-profile", "--profile 的檔有 NaN：Python 讀得進來、照用；這裡當成讀不懂，plan 判 invalid"),
    ("nan-stage-file", "政策核准時該段的結果檔有 NaN：Python 讀得進來再判違規；這裡當成讀不懂、直接失敗（一樣不核准）"),
    ("nan-run", "run.json 有 NaN：Python 讀得進來、照常執行各指令；這裡當成讀不懂，每個指令都失敗"),
    ("huge-int-run", "run.json 有超出 64 位元範圍的整數：Python 照常執行；這裡當成讀不懂，每個指令都失敗"),
    ("surrogate-run", "run.json 有落單的代理字元跳脫：Python 讀得進來（status、decide 照常，要把它寫回檔案的指令才丟 UnicodeEncodeError）；這裡讀的時候就拒絕，每個指令都失敗"),
];

/// 兩邊都拒絕、只是文字或例外類別不同的步驟：標籤、原因。要求：兩邊的 exit code 都不是 0，改到的檔相同。
const KNOWN: [(&str, &str); 1] = [
    ("surrogate-result", "workflow 回傳有落單的代理字元跳脫：Python 寫檔時丟 UnicodeEncodeError（結果檔已建立但是空的）；這裡讀的時候就拒絕，不建檔"),
];

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/save_run.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn teams() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin/scaffold/.claude/teams");
    p.canonicalize().expect("scaffold 的 teams 目錄").to_string_lossy().into_owned()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|k| u8::from_str_radix(h.get(k * 2..k * 2 + 2)?, 16).ok()).collect()
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default()
}

/// JSON 值相同（數字用數值比：Python 的 0 和這裡的 0.0 算相同；物件不看鍵的順序）。
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q))),
        _ => a == b,
    }
}

/// 參數裡的 `{"@const": 名稱}` 換回劇本的 `consts`。
fn unfold(v: &Value, consts: &Value) -> Value {
    match v {
        Value::Object(o) if o.len() == 1 && o.contains_key("@const") => consts[s(&o["@const"])].clone(),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), unfold(x, consts))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|x| unfold(x, consts)).collect()),
        other => other.clone(),
    }
}

struct Stage {
    td: String,
    real: String,
    ctx: Ctx,
    /// 快照的快取：路徑 →（修改時間、長度、內容）。沒動過的檔不重讀。
    seen: RefCell<HashMap<PathBuf, (Option<std::time::SystemTime>, u64, Value)>>,
}

impl Stage {
    fn new(tag: &str, today: &str) -> Stage {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("save-run-parity-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建立暫存目錄");
        let td = dir.to_string_lossy().into_owned();
        let real = dir.canonicalize().expect("暫存目錄").to_string_lossy().into_owned();
        // Python 錄的時候：目前目錄是暫存目錄（getcwd 回實體路徑），專案的 profile 與資料包的專案根用原路徑
        let ctx = Ctx { teams: teams(), profile: format!("{td}/proj/.claude/teams/methodology.json"), pack_root: format!("{td}/proj"), cwd: real.clone(), today: Some(today.to_string()) };
        Stage { td, real, ctx, seen: RefCell::new(HashMap::new()) }
    }

    fn p(&self, rel: &str) -> PathBuf {
        Path::new(&self.td).join(rel)
    }

    fn abs(&self, text: &str) -> String {
        text.replace("@TMP@", &self.td)
    }

    fn abs_json(&self, v: &Value) -> Value {
        match v {
            Value::String(t) => Value::String(self.abs(t)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.abs_json(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.abs_json(x))).collect()),
            other => other.clone(),
        }
    }

    fn norm(&self, text: &str) -> String {
        text.replace(&self.real, "@TMP@").replace(&self.td, "@TMP@")
    }

    fn norm_json(&self, v: &Value) -> Value {
        match v {
            Value::String(t) => Value::String(self.norm(t)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.norm_json(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (self.norm(k), self.norm_json(x))).collect()),
            other => other.clone(),
        }
    }

    fn walk(&self, base: &Path, dir: &Path, files: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten().map(|e| e.path()) {
            if e.is_dir() {
                self.walk(base, &e, files);
            } else {
                let rel = e.strip_prefix(base).ok().and_then(|r| r.to_str()).unwrap_or_default().to_string();
                let meta = std::fs::metadata(&e).ok();
                let key = (meta.as_ref().and_then(|m| m.modified().ok()), meta.as_ref().map_or(0, std::fs::Metadata::len));
                if let Some((t, n, v)) = self.seen.borrow().get(&e) {
                    if key.0.is_some() && (*t, *n) == key {
                        files.insert(rel, v.clone());
                        continue;
                    }
                }
                let text = self.norm(&String::from_utf8_lossy(&std::fs::read(&e).unwrap_or_default()));
                let n = text.chars().count();
                let val = if n <= LONG { json!(text) } else { json!({"sha1": hex_bytes(&Sha1::digest(text.as_bytes())), "chars": n}) };
                self.seen.borrow_mut().insert(e.clone(), (key.0, key.1, val.clone()));
                files.insert(rel, val);
            }
        }
    }

    fn snapshot(&self) -> Map<String, Value> {
        let base = PathBuf::from(&self.td);
        let mut files = Map::new();
        self.walk(&base, &base, &mut files);
        files
    }

    /// 這一步改了哪些檔（刪掉的記 null）。
    fn changed(&self, before: &Map<String, Value>) -> Value {
        let after = self.snapshot();
        let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        let mut out = Map::new();
        for k in keys {
            if before.get(k) != after.get(k) {
                out.insert(k.clone(), after.get(k).cloned().unwrap_or(Value::Null));
            }
        }
        Value::Object(out)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.td);
    }
}

/// 兩份「改了哪些檔」哪裡不同。
fn files_diff(want: &Value, got: &Value) -> Option<String> {
    let (wo, go) = (want.as_object()?, got.as_object()?);
    let keys: BTreeSet<&String> = wo.keys().chain(go.keys()).collect();
    let bad: Vec<String> = keys
        .into_iter()
        .filter(|k| wo.get(*k) != go.get(*k))
        .map(|k| format!("{k}\n      要：{}\n      得：{}", wo.get(k).map_or("（沒動）".to_string(), Value::to_string), go.get(k).map_or("（沒動）".to_string(), Value::to_string)))
        .collect();
    if bad.is_empty() { None } else { Some(bad.join("\n    ")) }
}

fn res<T: Into<Value>>(r: Result<T, PyErr>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(e) => json!({"raises": e.class}),
    }
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array().map(|a| a.iter().map(|x| (s(&x[0]).to_string(), s(&x[1]).to_string())).collect()).unwrap_or_default()
}

/// 只能在程式內呼叫的函式；回傳 `{"ok": …}` 或 `{"raises": 類別}`。參數的對應和錄製腳本的 CALLS 相同。
fn call(ctx: &Ctx, fn_name: &str, a: &Value) -> Value {
    let st = |k: &str| s(&a[k]).to_string();
    let path = |k: &str| a[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    let (run, stage) = (st("run"), st("stage"));
    let ok = |v: Value| json!({"ok": v});
    let list = |k: &str| a[k].as_array().cloned().unwrap_or_default();
    match fn_name {
        "opt" => ok(json!(sr::opt(&strings(&a["argv"]), &st("name"), a["default"].as_str().unwrap_or("")))),
        "opts" => ok(json!(sr::opts(&strings(&a["argv"]), &st("name")))),
        "split_answer" => {
            let (q, ans) = sr::split_answer(&st("raw"));
            ok(json!([q, ans]))
        }
        "decision_errors" => ok(json!(sr::decision_errors(&pairs(&a["items"]), &st("by"), &st("scope"), &st("expires")))),
        "is_envelope" => ok(json!(sr::is_envelope(&a["v"]))),
        "envelopes" => ok(json!(sr::envelopes(&a["v"]))),
        "missing_agents" => ok(json!(sr::missing_agents(&a["v"]))),
        "md_cell" => ok(json!(sr::md_cell(&a["v"]))),
        "record_candidates" => ok(json!(sr::record_candidates(ctx, &run, &stage, &st("record")))),
        "envelope_of" => {
            let envs = list("envs");
            let refs: Vec<&Value> = envs.iter().collect();
            res(sr::envelope_of(&a["entry"], &stage, &refs).map(|e| e.cloned().unwrap_or(Value::Null)))
        }
        "retried_ok" => res(sr::retried_ok(&list("trace"), a["i"].as_u64().unwrap_or(0) as usize)),
        "model_errors" => res(sr::model_errors(&stage, &a["result"], &a["profile"], &a["modifiers"])),
        "process_trace" => res(sr::process_trace(ctx, &run, &stage, &a["result"], a["write"].as_bool().unwrap_or(false)).map(|(has, rows, missing)| {
            let rows: Vec<Value> = rows
                .iter()
                .map(|r| json!({"label": r.label, "agent_type": r.agent_type, "phase": r.phase, "record": r.record, "artifacts": r.artifacts, "status": r.status}))
                .collect();
            json!([has, rows, missing])
        })),
        "policy_violations" => res(sr::policy_violations(ctx, &stage, &a["result"], path("run").as_deref())),
        "recipe_mismatch" => res(sr::recipe_mismatch(&a["data"], &a["result"])),
        "required_roles_missing" => res(sr::required_roles_missing(&a["data"], &stage, &a["result"])),
        "has_recipe" => res(sr::has_recipe(&a["data"])),
        "constraint_file" => res(sr::constraint_file(ctx, &a["c"], path("repo").as_deref())),
        "constraint_rows" => {
            let rows: BTreeMap<String, Vec<(String, String)>> = sr::constraint_rows(ctx, &run, &stage).into_iter().collect();
            ok(json!(rows))
        }
        "ac_ids" => ok(json!(sr::ac_ids(&st("text")))),
        "result_subtasks" => ok(json!(sr::result_subtasks(&a["result"]))),
        "plan_check_issues" => res(sr::plan_check_issues(ctx, &a["result"], path("spec").as_deref())),
        "plan_check_for" => res(sr::plan_check_for(ctx, &run, &stage, &a["result"], &a["data"])),
        "constraint_issues" => {
            res(sr::constraint_issues(ctx, &run, &stage, &a["profile"], path("repo").as_deref(), a["run_checks"].as_bool().unwrap_or(true)).map(|(issues, checks)| json!([issues, checks])))
        }
        "load_profile_for" => {
            let (profile, err) = sr::load_profile_for(ctx, &strings(&a["argv"]));
            ok(json!([profile, err]))
        }
        "mods" => res(sr::mods(&a["v"])),
        "recipe_summary" => res(sr::recipe_summary(&a["rec"])),
        "intent_errors" => res(sr::intent_errors(ctx, &a["result"], &a["profile"], &a["plan_mode"])),
        "recipe_tamper_errors" => res(sr::recipe_tamper_errors(ctx, &a["data"], a["profile"].as_str().unwrap_or(""))),
        "followup_run_errors" => res(sr::followup_run_errors(ctx, &st("frun"))),
        "chain_errors" => res(sr::chain_errors(&a["data"], &stage)),
        "abandoned_error" => res(sr::abandoned_error(&a["data"])),
        "escalate_affected" => res(sr::escalate_affected(&a["new"], &a["old"], &a["profile"]).map(|(added, stale)| json!([added, stale]))),
        "next_stage" => res(sr::next_stage(&a["data"])),
        "clip" => ok(json!(sr::clip(&a["s"], a["n"].as_u64().unwrap_or(0) as usize))),
        "archive_text" => res(sr::archive_text(ctx, &run, &a["data"])),
        "write_archive" => ok(json!(sr::write_archive(ctx, &run, &a["data"]))),
        "status_snapshot" => res(sr::status_snapshot(ctx, &run, &a["data"])),
        "load" => res(sr::load(ctx, &run)),
        "record_decisions" => {
            let mut err = String::new();
            let r = sr::record_decisions(ctx, &run, &pairs(&a["items"]), &st("by"), &st("at"), &st("scope"), &st("expires"), &st("stage"), &mut err);
            res(r.map(|n| json!([n, err])))
        }
        other => json!({"unknown": other}),
    }
}

/// 函式的結果和劇本相同：`ok` 逐值比；丟例外時只比類別。
fn result_same(want: &Value, got: &Value) -> bool {
    match (want.get("raises"), got.get("raises")) {
        (Some(w), Some(g)) => w == g,
        (None, None) => same(want, got),
        _ => false,
    }
}

fn label_kind(label: &str) -> Option<&'static str> {
    if STRICTER.iter().any(|(l, _)| *l == label) {
        Some("stricter")
    } else if KNOWN.iter().any(|(l, _)| *l == label) {
        Some("known")
    } else {
        None
    }
}

/// 帶 label 的指令和 Python 不同時，要符合那張表的要求；不符合回理由。
fn labelled_ok(kind: &str, step: &Value, code: i32, out: &str, files: &Value) -> Option<String> {
    let want_files = &step["files"];
    if code == 0 {
        return Some(format!("exit code 是 0（{kind} 的步驟這裡一定要拒絕）"));
    }
    if kind == "known" {
        if step["code"].as_i64() == Some(0) {
            return Some("KNOWN 的步驟 Python 那邊也要是拒絕".to_string());
        }
        // 落單的代理字元：Python 寫到一半才失敗，會留下空的結果檔；這裡不建檔。其他檔要相同
        let extra: Vec<&String> = files.as_object().map(|o| o.keys().filter(|k| want_files.get(k.as_str()) != o.get(k.as_str())).collect()).unwrap_or_default();
        if !extra.is_empty() {
            return Some(format!("改到 Python 沒改的檔：{extra:?}"));
        }
        return None;
    }
    // STRICTER：stage 可以把該段記成 invalid（stdout 不能說 done）；其他指令拒絕就是什麼都不記
    if out.contains("狀態 done") {
        return Some("stdout 說這一段是 done".to_string());
    }
    let touched = files.as_object().is_some_and(|o| !o.is_empty());
    if s(&step["argv"][0]) != "stage" && touched { Some(format!("拒絕的指令卻改了檔：{files}")) } else { None }
}

#[test]
fn replay_script() {
    let exp = expected();
    let st = Stage::new("script", s(&exp["today"]));
    let consts = &exp["consts"];
    let steps = exp["steps"].as_array().expect("steps");
    let (mut bad, mut runs, mut calls, mut exact, mut usage) = (Vec::new(), 0usize, 0usize, 0usize, 0usize);
    let mut differs: BTreeMap<String, usize> = BTreeMap::new();
    let mut unknown_labels: BTreeSet<String> = BTreeSet::new();
    for (i, step) in steps.iter().enumerate() {
        let op = s(&step["op"]);
        let rel = s(&step["path"]);
        match op {
            "write" => {
                let p = st.p(rel);
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d).expect("mkdir");
                }
                let data = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None => st.abs(s(&step["text"])).into_bytes(),
                };
                std::fs::write(&p, data).expect("write");
                continue;
            }
            "rm" => {
                std::fs::remove_file(st.p(rel)).expect("rm");
                continue;
            }
            "rmtree" => {
                std::fs::remove_dir_all(st.p(rel)).expect("rmtree");
                continue;
            }
            "mkdir" => {
                std::fs::create_dir_all(st.p(rel)).expect("mkdir");
                continue;
            }
            _ => {}
        }
        let label = step.get("label").and_then(Value::as_str);
        if let Some(l) = label {
            if label_kind(l).is_none() {
                unknown_labels.insert(l.to_string());
            }
        }
        let before = st.snapshot();
        let head = format!("第 {i} 步 {}（{}）", if op == "run" { step["argv"].to_string() } else { format!("{} {}", s(&step["fn"]), step["args"]) }, s(&step["note"]));
        if op == "call" {
            calls += 1;
            let args = st.abs_json(&unfold(&step["args"], consts));
            let got = st.norm_json(&call(&st.ctx, s(&step["fn"]), &args));
            let files = st.changed(&before);
            let mut why = Vec::new();
            if !result_same(&step["result"], &got) {
                why.push(format!("結果\n      要：{}\n      得：{got}", step["result"]));
            }
            if let Some(d) = files_diff(&step["files"], &files) {
                why.push(format!("檔案 {d}"));
            }
            if why.is_empty() {
                exact += 1;
            } else if let Some(l) = label.filter(|l| label_kind(l).is_some()) {
                *differs.entry(l.to_string()).or_default() += 1;
                if got.get("raises").is_none() {
                    bad.push(format!("{head}\n    帶 label {l} 的函式呼叫和 Python 不同時，這裡要是丟例外\n    {}", why.join("\n    ")));
                }
            } else {
                bad.push(format!("{head}\n    {}", why.join("\n    ")));
            }
            continue;
        }
        runs += 1;
        let argv: Vec<String> = strings(&step["argv"]).iter().map(|x| st.abs(x)).collect();
        let (code, out, err) = sr::run_captured(&argv, &st.abs(s(&step["stdin"])), &st.ctx);
        let (out, err) = (st.norm(&out), st.norm(&err));
        let files = st.changed(&before);
        let want_code = step["code"].as_i64().unwrap_or(-1) as i32;
        let mut why = Vec::new();
        if step.get("usage").is_some() {
            // 用法訊息的指令名不同：只比 exit code
            usage += 1;
            if code != want_code {
                why.push(format!("exit code 要 {want_code}、得 {code}"));
            }
        } else if let Some(class) = step.get("raises").and_then(Value::as_str) {
            // Python 沒接住的例外：這裡一樣 exit 1、stderr 提到同一個類別
            if code != 1 {
                why.push(format!("exit code 要 1（Python 丟 {class}）、得 {code}\n      stdout：{out:?}\n      stderr：{err:?}"));
            } else if !err.contains(&format!("：{class}：")) {
                why.push(format!("stderr 要提到 {class}：{err:?}"));
            }
        } else {
            if code != want_code {
                why.push(format!("exit code 要 {want_code}、得 {code}"));
            }
            if out != s(&step["stdout"]) {
                why.push(format!("stdout\n      要：{:?}\n      得：{out:?}", s(&step["stdout"])));
            }
            if err != s(&step["stderr"]) {
                why.push(format!("stderr\n      要：{:?}\n      得：{err:?}", s(&step["stderr"])));
            }
        }
        if let Some(d) = files_diff(&step["files"], &files) {
            why.push(format!("檔案 {d}"));
        }
        if why.is_empty() {
            exact += 1;
            continue;
        }
        match label.and_then(|l| label_kind(l).map(|k| (l, k))) {
            Some((l, kind)) => {
                *differs.entry(l.to_string()).or_default() += 1;
                if let Some(m) = labelled_ok(kind, step, code, &out, &files) {
                    bad.push(format!("{head}\n    label {l}：{m}\n    {}", why.join("\n    ")));
                }
            }
            None => bad.push(format!("{head}\n    {}", why.join("\n    "))),
        }
    }
    let labelled: usize = differs.values().sum();
    println!("save_run 劇本：{} 步（指令 {runs}、函式 {calls}、其餘是檔案操作）；逐字相同 {exact}（其中用法訊息只比 exit code {usage} 步）、列名的差異 {labelled}：{differs:?}", steps.len());
    assert!(unknown_labels.is_empty(), "劇本裡有不在 STRICTER／KNOWN 的 label：{unknown_labels:?}");
    assert!(bad.is_empty(), "和 Python 不同的有 {} 步：\n\n{}", bad.len(), bad.iter().take(25).cloned().collect::<Vec<_>>().join("\n\n"));
    // 兩張表沒有過時：每個 label 都真的有一步和 Python 不同
    let stale: Vec<&str> = STRICTER.iter().chain(KNOWN.iter()).map(|(l, _)| *l).filter(|l| !differs.contains_key(*l)).collect();
    assert!(stale.is_empty(), "STRICTER／KNOWN 列了已經和 Python 相同的項目（過時）：{stale:?}");
    assert!(runs > 1000 && calls > 150, "劇本的步數不對：指令 {runs}、函式 {calls}");
}

fn run_probes(probes: &[Value], consts: &Value, ctx: &Ctx) -> Vec<String> {
    let mut bad = Vec::new();
    for (i, p) in probes.iter().enumerate() {
        let got = call(ctx, s(&p["fn"]), &unfold(&p["args"], consts));
        if !result_same(&p["result"], &got) {
            bad.push(format!("第 {i} 個 {} {}\n    要：{}\n    得：{got}", s(&p["fn"]), p["args"], p["result"]));
        }
    }
    bad
}

#[test]
fn replay_probes() {
    let exp = expected();
    let st = Stage::new("probes", s(&exp["today"]));
    let probes = exp["probes"].as_array().expect("probes");
    let bad = run_probes(probes, &exp["consts"], &st.ctx);
    println!("save_run 探測：{} 個，不同 {}", probes.len(), bad.len());
    assert!(bad.is_empty(), "和 Python 不同的探測有 {} 個：\n\n{}", bad.len(), bad.iter().take(30).cloned().collect::<Vec<_>>().join("\n\n"));
    assert!(probes.len() > 3000, "探測的數量不對：{}", probes.len());
}

/// 一次性的隨機對照（不進基準）：`AWF_SAVE_RUN_FUZZ=<檔>` 指到 Python 那邊產生的探測（同 `probes` 的格式，一行一個）。
#[test]
#[ignore]
fn replay_fuzz_file() {
    let Ok(path) = std::env::var("AWF_SAVE_RUN_FUZZ") else { return };
    let st = Stage::new("fuzz", "2026-11-12");
    let text = std::fs::read_to_string(&path).expect("讀探測檔");
    let probes: Vec<Value> = text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("JSON")).collect();
    let bad = run_probes(&probes, &json!({}), &st.ctx);
    println!("save_run 隨機對照：{} 個，不同 {}", probes.len(), bad.len());
    assert!(bad.is_empty(), "不同的有 {} 個：\n\n{}", bad.len(), bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n\n"));
}

/// merge_gate 裡有一份私有的 `recipe_tamper_errors`／`load_profile_for`（從測試呼叫不到）。
/// 這裡經由 `merge_gate::run --recipe <run.json>` 間接對照：它把前三個理由用「；」接起來放進「輸入錯誤：…」。
#[test]
fn tamper_errors_agree_with_merge_gate() {
    use awf_core::{merge_gate as mg, recipe};
    let st = Stage::new("tamper", "2026-11-12");
    let proj = format!("{}/proj", st.td);
    let run = format!("{proj}/_runs/r1");
    std::fs::create_dir_all(&run).expect("mkdir");
    std::fs::create_dir_all(format!("{proj}/.claude/teams")).expect("mkdir");
    let resolve = |name: &str, mods: Value| recipe::resolve(&Value::Null, &json!(name), &mods, &Value::Null).expect("resolve");
    let rec = |name: &str, mods: Value, resolved: Value| json!({"name": name, "modifiers": mods, "size": "", "resolved": resolved, "locked": true, "history": []});
    let edit = |mut v: Value, f: &dyn Fn(&mut Value)| {
        f(&mut v);
        v
    };
    let strict = json!({"recipes": {"bugfix": {"stages": ["plan", "design", "build", "sync", "test", "deliver", "merge"]}}});
    // （專案的 profile 文字, run.json 的 recipe）
    let cases: Vec<(Option<String>, Value)> = vec![
        (None, rec("bugfix", json!([]), resolve("bugfix", json!([])))),
        (None, rec("feature", json!(["ui", "security"]), resolve("feature", json!(["ui", "security"])))),
        (None, rec("bugfix", json!([]), edit(resolve("bugfix", json!([])), &|r| r["stages"] = json!(["plan", "build", "sync", "deliver", "merge"])))),
        (None, rec("bugfix", json!([]), edit(resolve("bugfix", json!([])), &|r| r["deliver_axes"] = json!(["bugs"])))),
        (None, rec("bugfix", json!([]), edit(resolve("bugfix", json!([])), &|r| r["build_kinds"] = json!(["code", "test", "doc", "agent_config", "migration"])))),
        (None, rec("spike", json!([]), edit(resolve("spike", json!([])), &|r| r["publishable"] = json!(true)))),
        (None, rec("hotfix", json!([]), edit(resolve("hotfix", json!([])), &|r| r["followup"] = json!(false)))),
        (None, rec("沒有這個", json!([]), resolve("feature", json!([])))),
        (None, rec("feature", json!(["zzz"]), resolve("feature", json!([])))),
        (None, rec("docs", json!([]), resolve("feature", json!([])))),
        (Some("{壞的".to_string()), rec("bugfix", json!([]), resolve("bugfix", json!([])))),
        (Some("[]".to_string()), rec("bugfix", json!([]), resolve("bugfix", json!([])))),
        (Some(json!({"recipes": {"x": {"stages": ["build"]}}}).to_string()), rec("bugfix", json!([]), resolve("bugfix", json!([])))),
        (Some(strict.to_string()), rec("bugfix", json!([]), resolve("bugfix", json!([])))),
    ];
    let mgctx = mg::Ctx { teams: PathBuf::from(teams()), project_dir_env: None, env: Vec::new(), stdin: Some(String::new()) };
    let (mut with_errors, mut clean) = (0, 0);
    for (i, (profile, recipe_v)) in cases.iter().enumerate() {
        let pf = format!("{proj}/.claude/teams/methodology.json");
        match profile {
            Some(text) => std::fs::write(&pf, text).expect("write"),
            None => {
                let _ = std::fs::remove_file(&pf);
            }
        }
        let data = json!({"event": "sdlc", "stages": {"plan": {"status": "done", "gate": true, "approved": {"by": "owner", "at": "t"}}}, "recipe": recipe_v});
        std::fs::write(format!("{run}/run.json"), data.to_string()).expect("write");
        let ours = sr::recipe_tamper_errors(&st.ctx, &data, "").unwrap_or_else(|e| panic!("案例 {i}：{}：{}", e.class, e.msg));
        let argv: Vec<String> = ["check", "--repo", "acme-fork/app", "--pr", "3", "--head-sha", &"a".repeat(40), "--project-dir", &proj, "--recipe", &format!("{run}/run.json"), "--no-audit"]
            .iter()
            .map(|x| x.to_string())
            .collect();
        let (mut out, mut errs) = (String::new(), String::new());
        mg::run(&mgctx, &argv, &mut out, &mut errs);
        let result: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("案例 {i}：merge_gate 的輸出不是 JSON（{e}）：{out}\n{errs}"));
        let reasons: Vec<String> = strings(&result["reasons"]).iter().map(|r| st.norm(r)).collect();
        let ours: Vec<String> = ours.iter().map(|r| st.norm(r)).collect();
        let theirs: Vec<&String> = reasons.iter().filter(|r| r.contains("recipe") || r.contains("profile")).collect();
        if ours.is_empty() {
            clean += 1;
            assert!(theirs.is_empty(), "案例 {i}：save_run 判沒問題，merge_gate 卻有 {theirs:?}");
        } else {
            with_errors += 1;
            let joined = format!("輸入錯誤：{}", ours.iter().take(3).cloned().collect::<Vec<_>>().join("；"));
            assert!(reasons.contains(&joined), "案例 {i}：merge_gate 的理由沒有 {joined:?}\n實際：{reasons:?}");
        }
    }
    assert!(with_errors >= 8 && clean >= 2, "案例的分布不對：有錯 {with_errors}、沒錯 {clean}");
}
