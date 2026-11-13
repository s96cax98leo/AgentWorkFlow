//! watcher（watch、statusline）的對跑：照 0.x 的 Python 版錄下來的劇本（tests/parity/watch.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作（write、append、rm、mkdir、symlink、mtime）、建立 transcript 讀取器（tr）、看一次（tick：
//! 快照、事件、警告、送出去的通知、progress.jsonl 與 .watch-state.json 的內容）、只能在程式內呼叫的函式（call）、
//! 命令列（run）與讀檔（read）。暫存路徑換成 `@TMP@` 再比。
//!
//! - 「現在」都是劇本給的（tick 直接給；命令列用 `Env.now`）；長駐模式的 sleep 換成劇本的 `sleeps`。
//! - transcript 的根目錄每一步都明確給，不讀使用者家目錄。
//! - 外部指令（notify）在 tick 與 call 是替身；命令列那幾步跑暫存目錄 `bin/` 裡的小腳本，只寫暫存目錄裡的紀錄檔。
//!
//! 和 Python 不同的步驟帶 label，其餘要逐字相同：
//! - [`STRICTER`]：Python 讀得進來、這裡當成讀不懂（JSON 照閘門的規則讀）。
//! - [`KNOWN`]：兩邊都不接受、只是文字不同（目前沒有）。
//!
//! 兩張表都有「沒有過時」的檢查：表上的 label 在劇本裡要真的有一步和 Python 不同，劇本裡的 label 也都要在表上。
#![cfg(unix)]

use awf_core::save_run::Ctx;
use awf_core::watch::{self, Env, Notify, Transcripts};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Python 讀得進來、這裡當成讀不懂的步驟：標籤、原因。
const STRICTER: [(&str, &str); 4] = [
    ("nan-run", "run.json 有 NaN：Python 讀得進來、照常出快照；這裡當成讀不到 run.json（不寫事件，命令列 exit 1）"),
    ("huge-int-run", "run.json 有超出 64 位元範圍的整數：Python 照常出快照；這裡當成讀不到 run.json"),
    ("nan-state", ".watch-state.json 有 NaN：Python 沿用裡面的記憶（寫回去還是 NaN）；這裡當成讀不懂，從頭記（事件序號從 1 起，已記過的事件會再記一次）"),
    ("huge-int-state", ".watch-state.json 的序號超出 64 位元：Python 接著那個數往下編；這裡當成讀不懂，從頭記"),
];

/// 兩邊都不接受、只是文字不同的步驟。
const KNOWN: [(&str, &str); 0] = [];

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/watch.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn src_root() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("模版根目錄").to_string_lossy().into_owned()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// JSON 值相同（物件不看鍵的順序）。
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q))),
        _ => a == b,
    }
}

/// `{"@join": […]}`、`{"@rep": [字, 次數]}` 在執行時組成字串（像憑證的字串不寫在基準檔裡）。
fn dec(v: &Value) -> Value {
    match v {
        Value::Object(o) if o.len() == 1 && o.contains_key("@join") => {
            Value::String(o["@join"].as_array().map(|a| a.iter().map(|x| s(&dec(x)).to_string()).collect::<String>()).unwrap_or_default())
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("@rep") => Value::String(s(&o["@rep"][0]).repeat(o["@rep"][1].as_u64().unwrap_or(0) as usize)),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), dec(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(dec).collect()),
        other => other.clone(),
    }
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|k| u8::from_str_radix(h.get(k * 2..k * 2 + 2)?, 16).ok()).collect()
}

fn abs(td: &str, text: &str) -> String {
    text.replace("@TMP@", td)
}

fn abs_json(td: &str, v: &Value) -> Value {
    match v {
        Value::String(t) => Value::String(abs(td, t)),
        Value::Array(a) => Value::Array(a.iter().map(|x| abs_json(td, x)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), abs_json(td, x))).collect()),
        other => other.clone(),
    }
}

fn norm(td: &str, text: &str) -> String {
    text.replace(td, "@TMP@")
}

fn norm_json(td: &str, v: &Value) -> Value {
    match v {
        Value::String(t) => Value::String(norm(td, t)),
        Value::Array(a) => Value::Array(a.iter().map(|x| norm_json(td, x)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm_json(td, x))).collect()),
        other => other.clone(),
    }
}

/// 檔案的內容（讀不到、是目錄回 null）。
fn read(td: &str, path: &Path) -> Value {
    match std::fs::read(path) {
        Ok(b) => json!(norm(td, &String::from_utf8_lossy(&b))),
        Err(_) => Value::Null,
    }
}

/// 做一個檔案操作（劇本的一步，或 sleeps 裡的一步）。
fn do_file(td: &str, step: &Value) {
    let path = Path::new(td).join(s(&step["path"]));
    let parent = || std::fs::create_dir_all(path.parent().expect("上層目錄")).expect("mkdir");
    match s(&step["op"]) {
        op @ ("write" | "append") => {
            parent();
            let data = match step.get("hex") {
                Some(h) => unhex(s(h)),
                None => abs(td, s(&dec(&step["text"]))).into_bytes(),
            };
            if op == "append" {
                use std::io::Write;
                std::fs::OpenOptions::new().append(true).create(true).open(&path).and_then(|mut fh| fh.write_all(&data)).expect("append");
            } else {
                std::fs::write(&path, data).expect("write");
            }
        }
        "rm" => {
            let _ = std::fs::remove_file(&path);
        }
        "mkdir" => std::fs::create_dir_all(&path).expect("mkdir"),
        "symlink" => {
            parent();
            std::os::unix::fs::symlink(abs(td, s(&step["to"])), &path).expect("symlink");
        }
        "mtime" => {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(step["secs"].as_u64().expect("secs"));
            std::fs::OpenOptions::new().write(true).open(&path).and_then(|fh| fh.set_modified(t)).expect("mtime");
        }
        other => panic!("不認得的檔案操作 {other}"),
    }
}

fn notify_of(v: &Value) -> Option<Notify> {
    let strings = |x: &Value| x.as_array().map(|a| a.iter().map(|i| s(i).to_string()).collect::<Vec<_>>()).unwrap_or_default();
    v.as_object().map(|_| Notify { argv: strings(&v["argv"]), events: strings(&v["events"]) })
}

/// 替身的外部指令：把收到的東西記下來，照 `mode` 回 exit code 或例外類別。
fn fake_runner<'a>(mode: &'a str, sent: &'a RefCell<Vec<Value>>) -> impl FnMut(&[String], &str) -> Result<i32, String> + 'a {
    move |argv: &[String], input: &str| {
        sent.borrow_mut().push(json!([argv, serde_json::from_str::<Value>(input).expect("通知的內容是 JSON")]));
        if let Some(class) = mode.strip_prefix("raise:") {
            return Err(class.to_string());
        }
        Ok(mode.strip_prefix("exit:").and_then(|n| n.parse().ok()).unwrap_or(0))
    }
}

fn class_or<T>(r: Result<T, awf_core::save_run::PyErr>, ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": ok(v)}),
        Err(e) => json!({"raises": e.class}),
    }
}

/// 函式的結果和劇本相同：丟例外時只比類別。
fn result_same(want: &Value, got: &Value) -> bool {
    match (want.get("raises"), got.get("raises")) {
        (Some(w), Some(g)) => w == g,
        (None, None) => same(want, got),
        _ => false,
    }
}

fn label_known(label: &str) -> bool {
    STRICTER.iter().chain(KNOWN.iter()).any(|(l, _)| *l == label)
}

struct Stage {
    td: String,
    ctx: Ctx,
    trs: HashMap<String, Transcripts>,
}

impl Stage {
    fn new() -> Stage {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("watch-parity-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建立暫存目錄");
        let td = dir.canonicalize().expect("暫存目錄").to_string_lossy().into_owned();
        Stage { ctx: Ctx::new(&format!("{td}/proj"), &td), td, trs: HashMap::new() }
    }

    fn tick(&mut self, step: &Value) -> (Value, Value, Value) {
        let run = abs(&self.td, s(&step["run"]));
        let now = watch::parse_now(s(&step["now"])).expect("now");
        let cfg = notify_of(&step["cfg"]);
        let sent = RefCell::new(Vec::new());
        let mut runner = fake_runner(s(&step["runner"]), &sent);
        let tr = step["tr"].as_str().map(|id| self.trs.get_mut(id).expect("tr"));
        let stall = step["stall"].as_i64().expect("stall");
        let r = watch::tick(&self.ctx, &run, tr, stall, cfg.as_ref(), &now, s(&step["at"]), &mut runner);
        let got = class_or(r, |t| {
            let text = t.snap.as_ref().map_or(Value::Null, |snap| json!(watch::text_of(snap)));
            json!({"snap": t.snap, "events": t.events, "warns": t.warns, "text": text, "sent": sent.borrow().clone()})
        });
        let file = |name: &str| read(&self.td, &Path::new(&run).join(name));
        (norm_json(&self.td, &got), file(watch::PROGRESS), file(watch::STATE))
    }

    fn call(&mut self, name: &str, a: &Value) -> Value {
        let st = |k: &str| s(&a[k]).to_string();
        let got = match name {
            "diff_events" => class_or(watch::diff_events(&a["prev"], &a["cur"], a["stall"].as_i64().expect("stall")), |(ev, mem)| json!({"events": ev, "mem": mem})),
            "notify_config" => class_or(watch::notify_config(&a["profile"]), |c| c.map_or(Value::Null, |c| c.to_value())),
            "notify_payload" => json!({"ok": watch::notify_payload(&a["event"])}),
            "notify" => {
                let sent = RefCell::new(Vec::new());
                let mode = a["runner"].as_str().unwrap_or("ok").to_string();
                let mut runner = fake_runner(&mode, &sent);
                let ret = watch::notify(notify_of(&a["cfg"]).as_ref(), &a["event"], &mut runner);
                json!({"ok": {"ret": ret, "sent": sent.borrow().clone()}})
            }
            "text_of" => json!({"ok": watch::text_of(&a["snap"])}),
            "run_view" => class_or(watch::run_view(&self.ctx, &st("run")), |v| v.unwrap_or(Value::Null)),
            "snapshot" => {
                let now = watch::parse_now(&st("now")).expect("now");
                let tr = a["tr"].as_str().map(|id| self.trs.get_mut(id).expect("tr"));
                class_or(watch::snapshot(&self.ctx, &st("run"), tr, &now), |v| v.unwrap_or(Value::Null))
            }
            "status_line" => json!({"ok": watch::status_line(&self.ctx, &st("project"))}),
            other => json!({"unknown": other}),
        };
        norm_json(&self.td, &got)
    }

    /// 命令列：回（exit code, stdout, stderr, 睡了幾次）。
    fn run(&self, step: &Value) -> (i32, String, String, Vec<u64>) {
        let project = format!("{}/{}", self.td, s(&step["project"]));
        let slept = Rc::new(RefCell::new(Vec::new()));
        let mut pending: Vec<Value> = step["sleeps"].as_array().cloned().unwrap_or_default();
        pending.reverse();
        let (td, log) = (self.td.clone(), slept.clone());
        let sleep = move |secs: u64| -> bool {
            log.borrow_mut().push(secs);
            let Some(batch) = pending.pop() else { return false };
            for st in batch.as_array().into_iter().flatten() {
                do_file(&td, st);
            }
            true
        };
        let mut env = Env {
            ctx: Ctx::new(&project, &self.td),
            project,
            transcripts: Some(format!("{}/{}", self.td, s(&step["transcripts"]))),
            now: Some(s(&step["now"]).to_string()),
            sleep: Some(Box::new(sleep)),
            runner: None,
        };
        let argv: Vec<String> = step["argv"].as_array().into_iter().flatten().map(|a| abs(&self.td, s(a))).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = watch::run(&mut env, &argv, &mut out, &mut err);
        let text = |b: Vec<u8>| norm(&self.td, &String::from_utf8_lossy(&b));
        let n = slept.borrow().clone();
        (code, text(out), text(err), n)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.td);
    }
}

#[test]
fn script_matches_python() {
    let exp = expected();
    let mut st = Stage::new();
    let steps = exp["steps"].as_array().expect("steps");
    let mut bad: Vec<String> = Vec::new();
    let mut differing: BTreeSet<String> = BTreeSet::new();
    let mut labels: BTreeSet<String> = BTreeSet::new();
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut counts = [0usize; 3];
    for (i, step) in steps.iter().enumerate() {
        let op = s(&step["op"]);
        let label = step["label"].as_str();
        if let Some(l) = label {
            labels.insert(l.to_string());
            assert!(label_known(l), "第 {i} 步的 label「{l}」不在 STRICTER／KNOWN 表上");
        }
        let title = step.get("note").or_else(|| step.get("fn")).map_or(String::new(), |n| s(n).to_string());
        let mut fail = |why: String| bad.push(format!("第 {i} 步（{op} {title}）：{why}"));
        match op {
            "write" | "append" | "rm" | "mkdir" | "symlink" | "mtime" => do_file(&st.td, step),
            "tr" => {
                let root = step["root"].as_str().map(|r| abs(&st.td, r));
                st.trs.insert(s(&step["id"]).to_string(), Transcripts::new(root.as_deref(), &abs(&st.td, s(&step["run"]))));
            }
            "read" => {
                let got = read(&st.td, &Path::new(&st.td).join(s(&step["path"])));
                if got != step["text"] {
                    fail(format!("檔案內容不同\n    要：{}\n    得：{got}", step["text"]));
                }
            }
            "tick" => {
                counts[0] += 1;
                let (got, progress, state) = st.tick(step);
                for e in got["ok"]["events"].as_array().into_iter().flatten() {
                    kinds.insert(s(&e["type"]).to_string());
                }
                if result_same(&step["result"], &got) && progress == step["progress"] && state == step["state"] {
                    continue;
                }
                match label {
                    Some(l) => {
                        differing.insert(l.to_string());
                        // 當成讀不懂、從頭記：不能中斷，序號從 1 起
                        let fresh = got["ok"]["events"].as_array().is_some_and(|ev| ev.iter().enumerate().all(|(k, e)| e["seq"] == json!(k + 1)));
                        if !fresh {
                            fail(format!("label {l}：這裡要當成讀不懂、從頭記\n    得：{got}"));
                        }
                    }
                    None => fail(format!("要：{}\n    得：{got}\n    progress 要：{}\n    progress 得：{progress}\n    state 要：{}\n    state 得：{state}", step["result"], step["progress"], step["state"])),
                }
            }
            "call" => {
                counts[1] += 1;
                let args = abs_json(&st.td, &dec(&step["args"]));
                let got = st.call(s(&step["fn"]), &args);
                if result_same(&step["result"], &got) {
                    continue;
                }
                match label {
                    Some(l) => {
                        differing.insert(l.to_string());
                        if got != json!({"ok": null}) || step["result"]["ok"].is_null() {
                            fail(format!("label {l}：Python 要讀得進來、這裡要當成讀不到\n    Python：{}\n    這裡：{got}", step["result"]));
                        }
                    }
                    None => fail(format!("{}\n    要：{}\n    得：{got}", step["args"], step["result"])),
                }
            }
            "run" => {
                counts[2] += 1;
                let (code, out, err, slept) = st.run(step);
                let slept_same = json!(slept) == step["slept"];
                let exact = match step["raises"].as_str() {
                    Some(class) => code == 1 && err.contains(class) && slept_same,
                    None => Some(i64::from(code)) == step["code"].as_i64() && out == s(&step["stdout"]) && err == s(&step["stderr"]) && slept_same,
                };
                if exact {
                    continue;
                }
                match label {
                    Some(l) => {
                        differing.insert(l.to_string());
                        if code != 1 || !err.contains("讀不到") || !out.is_empty() {
                            fail(format!("label {l}：這裡要當成讀不到 run.json（exit 1）\n    得：exit {code}\n{out}{err}"));
                        }
                    }
                    None => fail(format!("argv {}\n    要：exit {} {}\n{}{}\n    睡 {}\n    得：exit {code}\n{out}{err}\n    睡 {slept:?}", step["argv"], step["code"], step["raises"], s(&step["stdout"]), s(&step["stderr"]), step["slept"])),
                }
            }
            other => panic!("第 {i} 步：不認得的 op {other}"),
        }
    }
    assert!(bad.is_empty(), "{} 步和 0.x 的 Python 版不同：\n{}", bad.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
    // 兩張表沒有過時：表上的 label 都有一步真的不同
    for (l, _) in STRICTER.iter().chain(KNOWN.iter()) {
        assert!(labels.contains(*l), "表上的 label「{l}」劇本裡沒有");
        assert!(differing.contains(*l), "label「{l}」的步驟現在都和 Python 相同，該從表上拿掉");
    }
    // 每一種事件都走到了
    for kind in watch::EVENT_TYPES {
        assert!(kinds.contains(kind), "劇本沒有走到事件 {kind}");
    }
    eprintln!("watch 對跑：tick {} 步、函式 {} 步、命令列 {} 步", counts[0], counts[1], counts[2]);
    assert!(counts[0] >= 80 && counts[1] >= 500 && counts[2] >= 25, "劇本的步數變少了：{counts:?}");
}

#[test]
fn selftest_matches_python() {
    let exp = expected();
    let scaffold = format!("{}/plugin/scaffold", src_root());
    let ctx = Ctx::new(&scaffold, &scaffold);
    let mut out = String::new();
    let code = watch::selftest(&ctx, &mut out).map_err(|e| format!("{}：{}", e.class, e.msg)).expect("selftest 不該中斷");
    assert_eq!(out, s(&exp["selftest"]["stdout"]), "selftest 的輸出和 0.x 的 Python 版不同");
    assert_eq!(Some(i64::from(code)), exp["selftest"]["code"].as_i64());
    assert!(out.lines().filter(|l| l.contains("[通過]")).count() >= 30 && !out.contains("[失敗]"));
}

#[test]
fn usage_matches_python() {
    assert_eq!(watch::USAGE, s(&expected()["usage"]));
}
