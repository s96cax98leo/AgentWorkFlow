//! 交付文件與工具先定位的對跑：照 0.x 的 Python 版錄下來的劇本（tests/parity/deliverables.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作、指令（比 exit code、stdout、stderr 與這一步改了哪些檔）和只能在程式內呼叫的函式（call）。
//! `probes` 是不碰檔案的函式探測。暫存路徑換成 `@TMP@` 再比。劇本用到 chmod 與 /bin/sh 的假 pandoc，所以只在 unix 上跑。
#![cfg(unix)]

use awf_core::{deliverables as dv, locate};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 比這長的檔案內容，劇本裡只記 sha1（和錄製腳本的 LONG 相同）。
const LONG: usize = 4000;

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/deliverables.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|k| u8::from_str_radix(h.get(k * 2..k * 2 + 2)?, 16).ok()).collect()
}

fn unhex_s(v: &Value) -> String {
    String::from_utf8_lossy(&unhex(s(v))).into_owned()
}

fn hex(t: &str) -> String {
    t.bytes().map(|b| format!("{b:02x}")).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default()
}

/// JSON 值相同（數字用數值比：Python 的 0 和這裡的 0.0 算相同）。
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q))),
        _ => a == b,
    }
}

struct Stage {
    td: String,
    real: String,
}

impl Stage {
    fn p(&self, rel: &str) -> PathBuf {
        Path::new(&self.td).join(rel)
    }

    fn abs(&self, text: &str) -> String {
        text.replace("@TMP@", &self.td)
    }

    fn norm(&self, text: &str) -> String {
        text.replace(&self.real, "@TMP@").replace(&self.td, "@TMP@")
    }

    fn walk(&self, base: &Path, dir: &Path, files: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten().map(|e| e.path()) {
            let name = e.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if e.is_dir() {
                if name != ".git" {
                    self.walk(base, &e, files);
                }
            } else {
                let rel = e.strip_prefix(base).ok().and_then(|r| r.to_str()).unwrap_or_default().to_string();
                let text = self.norm(&String::from_utf8_lossy(&std::fs::read(&e).unwrap_or_default()));
                let n = text.chars().count();
                let val = if n <= LONG { json!(text) } else { json!({"sha1": hex_bytes(&Sha1::digest(text.as_bytes())), "chars": n}) };
                files.insert(rel, val);
            }
        }
    }

    /// 整棵暫存目錄（略過 .git）：相對路徑 → 內容。
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

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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

fn opt_str(st: &Stage, v: &Value) -> Option<String> {
    v.as_str().map(|x| st.abs(x))
}

fn only_set(v: &Value) -> Option<BTreeSet<String>> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect())
}

/// 只能在程式內呼叫的函式。
fn call(st: &Stage, root: &str, fn_name: &str, a: &Value) -> Value {
    let run = st.abs(s(&a["run"]));
    let repo = opt_str(st, &a["repo"]);
    let reviewed = strings(&a["reviewed"]);
    let only = only_set(&a["only"]);
    match fn_name {
        "pack_args" => {
            let paths: Option<Vec<String>> = a["paths"].as_array().map(|_| strings(&a["paths"]));
            let (v, notes) = dv::pack_args(s(&a["stage"]), &run, root, repo.as_deref(), &a["data"], paths.as_deref());
            json!([v, notes])
        }
        "sync" => {
            let (code, msgs) = dv::sync(root, &run, repo.as_deref(), &reviewed, only.as_ref());
            json!([code, msgs])
        }
        "stamp" => {
            let cfg = dv::load_config(root).0.unwrap_or(Value::Null);
            let (done, stale) = dv::stamp(root, &cfg, &dv::load_catalog(root), None, &reviewed, only.as_ref());
            json!([done, stale])
        }
        "matrix_rows" => {
            let (rows, gaps) = dv::matrix_rows(&run);
            json!([rows, gaps])
        }
        "locate_all" => {
            let req = opt_str(st, &a["req_path"]);
            let got = locate::locate_all(opt_str(st, &a["repo"]).as_deref(), s(&a["text"]), req.as_deref(), &strings(&a["angles"]));
            Value::Array(got.iter().map(|(k, v)| json!([k, v.to_json()])).collect())
        }
        other => panic!("劇本有不認得的函式：{other}"),
    }
}

#[test]
fn steps_match_python() {
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("deliv-parity-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&td).expect("建暫存目錄");
    let st = Stage { td: td.to_string_lossy().into_owned(), real: std::fs::canonicalize(&td).expect("暫存目錄").to_string_lossy().into_owned() };
    let root = st.p("proj").to_string_lossy().into_owned();
    let mut bad: Vec<String> = Vec::new();
    let (mut runs, mut same_n) = (0usize, 0usize);
    let steps = exp["steps"].as_array().expect("steps");
    for (i, step) in steps.iter().enumerate() {
        let path = s(&step["path"]);
        match s(&step["op"]) {
            "write" => {
                let once = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None => s(&step["text"]).as_bytes().to_vec(),
                };
                let data = once.repeat(step.get("repeat").and_then(Value::as_u64).unwrap_or(1) as usize);
                let full = st.p(path);
                std::fs::create_dir_all(full.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&full, data).unwrap_or_else(|e| panic!("寫不進 {path}：{e}"));
            }
            "rm" => std::fs::remove_file(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "rmtree" => std::fs::remove_dir_all(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "chmod" => {
                use std::os::unix::fs::PermissionsExt;
                let mode = step["mode"].as_u64().expect("mode") as u32;
                std::fs::set_permissions(st.p(path), std::fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("改不了 {path} 的權限：{e}"));
            }
            "git" => {
                let cwd = st.p(s(&step["cwd"]));
                std::fs::create_dir_all(&cwd).expect("建目錄");
                let mail = format!("awf{}example.invalid", '@');
                let ok = std::process::Command::new("git")
                    .args(strings(&step["args"]))
                    .current_dir(&cwd)
                    .env("GIT_AUTHOR_NAME", "awf")
                    .env("GIT_AUTHOR_EMAIL", &mail)
                    .env("GIT_COMMITTER_NAME", "awf")
                    .env("GIT_COMMITTER_EMAIL", &mail)
                    .env("GIT_AUTHOR_DATE", "2026-01-02T03:04:05Z")
                    .env("GIT_COMMITTER_DATE", "2026-01-02T03:04:05Z")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                assert!(ok, "第 {i} 步的 git 失敗：{}", step["args"]);
            }
            "pandoc" => dv::set_pandoc_override(match &step["value"] {
                Value::Null => Some(None),
                v if v == "default" => None,
                v => Some(Some(st.abs(s(v)))),
            }),
            "run" => {
                runs += 1;
                let shown = strings(&step["argv"]);
                let argv: Vec<String> = shown.iter().map(|a| st.abs(a)).collect();
                let before = st.snapshot();
                let mut out = String::new();
                let code = if s(&step["tool"]) == "deliverables" { dv::run(&root, &argv, &mut out) } else { locate::run(&argv, &mut out) };
                let out = st.norm(&out);
                let mut diff = Vec::new();
                if Some(i64::from(code)) != step["code"].as_i64() {
                    diff.push(format!("exit code 要 {}、得 {code}", step["code"]));
                }
                if out != s(&step["stdout"]) {
                    diff.push(format!("stdout\n    要：{:?}\n    得：{out:?}", s(&step["stdout"])));
                }
                if !s(&step["stderr"]).is_empty() {
                    diff.push(format!("stderr 要 {:?}（這一版不寫 stderr）", s(&step["stderr"])));
                }
                if let Some(d) = files_diff(&step["files"], &st.changed(&before)) {
                    diff.push(format!("改到的檔\n    {d}"));
                }
                if diff.is_empty() {
                    same_n += 1;
                } else {
                    bad.push(format!("第 {i} 步 {} {shown:?}\n  {}", s(&step["tool"]), diff.join("\n  ")));
                }
            }
            "call" => {
                runs += 1;
                let before = st.snapshot();
                let name = s(&step["fn"]);
                let raw = call(&st, &root, name, &step["args"]);
                let got: Value = serde_json::from_str(&st.norm(&raw.to_string())).unwrap_or(Value::Null);
                let mut diff = Vec::new();
                if !same(&step["result"], &got) {
                    diff.push(format!("回傳\n    要：{}\n    得：{got}", step["result"]));
                }
                if let Some(d) = files_diff(&step["files"], &st.changed(&before)) {
                    diff.push(format!("改到的檔\n    {d}"));
                }
                if diff.is_empty() {
                    same_n += 1;
                } else {
                    bad.push(format!("第 {i} 步 {name} {}\n  {}", step["args"], diff.join("\n  ")));
                }
            }
            other => panic!("劇本有不認得的步驟：{other}"),
        }
    }
    dv::set_pandoc_override(None);
    let _ = std::fs::remove_dir_all(&td);
    println!("deliverables／locate 對跑：{runs} 步（指令與函式），{same_n} 步逐字相同");
    assert!(runs >= 300, "步數不對：{runs}");
    assert!(bad.is_empty(), "{} 步不同（共 {runs} 步）：\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
}

fn parsed_json(p: &dv::Parsed, marker: bool) -> Value {
    let secs: Vec<Value> = p
        .sections
        .iter()
        .map(|x| {
            let mut m = json!({"title": x.title, "sources": x.sources, "body": x.body, "line": x.line});
            if marker {
                m["marker"] = json!(x.marker);
            }
            m
        })
        .collect();
    json!({"meta": p.meta, "sections": secs})
}

#[test]
fn probes_match_python() {
    let exp = expected();
    let pr = &exp["probes"];
    let cat = dv::builtin_catalog();
    let mut bad: Vec<String> = Vec::new();
    let mut n = 0usize;
    let mut check = |what: &str, want: &Value, got: Value| {
        n += 1;
        if !same(want, &got) {
            bad.push(format!("{what}\n  要：{want}\n  得：{got}"));
        }
    };
    let list = |k: &str| -> &Vec<Value> { pr[k].as_array().unwrap_or_else(|| panic!("探測少了 {k}")) };
    for c in list("body_hash") {
        let t = unhex_s(&c[0]);
        check(&format!("body_hash {t:?}"), &c[1], json!(dv::body_hash(&t)));
    }
    for c in list("privacy_text") {
        let t = unhex_s(&c[0]);
        check(&format!("privacy_text {t:?}"), &c[1], json!(hex(&dv::privacy_text(&t))));
    }
    for c in list("privacy_hits") {
        let t = unhex_s(&c["text"]);
        let allow: Vec<Value> = c["allow"].as_array().cloned().unwrap_or_default();
        let extra: Vec<dv::ExtraRule> = c["names"].as_array().map(|_| vec![dv::ExtraRule::literals("內部 run 目錄", &strings(&c["names"]))]).unwrap_or_default();
        // 不印內容本身：只印十六進位
        check(&format!("privacy_hits {} allow={}", s(&c["text"]), c["allow"]), &c["hits"], json!(dv::privacy_hits(&t, &allow, &extra)));
    }
    for c in list("parse_doc") {
        let head: String = s(&c["text"]).chars().take(40).collect();
        check(&format!("parse_doc {head:?}"), &c["doc"], parsed_json(&dv::parse_doc(s(&c["text"])), false));
    }
    for c in list("doc_sections") {
        let head: String = s(&c["text"]).chars().take(40).collect();
        let doc = if c["sources"].is_null() { json!({}) } else { json!({"sources": c["sources"]}) };
        check(&format!("doc_sections {head:?} {}", c["sources"]), &c["doc"], parsed_json(&dv::doc_sections(&doc, s(&c["text"])), true));
    }
    for c in list("match_title") {
        check(&format!("match_title {}", c[0]), &c[1], json!(dv::match_title(s(&c[0]), &cat)));
    }
    for c in list("strip_paths") {
        check(&format!("strip_paths {}", c[0]), &c[1], json!(dv::strip_paths(&c[0])));
    }
    for c in list("glob") {
        let rx = dv::glob_re(s(&c["pattern"]));
        for p in c["paths"].as_array().expect("paths") {
            check(&format!("glob {} 對 {}", c["pattern"], p[0]), &p[1], json!(rx.as_ref().is_some_and(|r| r.is_match(s(&p[0])))));
        }
    }
    for c in list("safe_rel") {
        check(&format!("safe_rel {}", c[0]), &c[1], json!(dv::safe_rel(&c[0])));
    }
    for c in list("config_errors") {
        check(&format!("config_errors {}", c[0]), &c[1], json!(dv::config_errors(&c[0])));
    }
    for c in list("skeleton") {
        check(&format!("skeleton {}", c[0]), &c[1], json!(dv::skeleton(&c[0])));
    }
    for c in list("index_text") {
        check(&format!("index_text {}", c[0]), &c[1], json!(dv::index_text(&c[0], &cat)));
    }
    for c in list("resolve_docs") {
        check(&format!("resolve_docs {}", c[0]), &c[1], json!(dv::resolve_docs(&c[0], &cat)));
    }
    for c in list("upsert_entry") {
        let a = &c["args"];
        let got = match dv::upsert_entry(s(&a[0]), s(&a[1]), s(&a[2]), s(&a[3])) {
            Ok(t) => json!({"ok": t}),
            Err(e) => json!({"err": e}),
        };
        let want = if c.get("ok").is_some() { json!({"ok": c["ok"]}) } else { json!({"err": c["err"]}) };
        check(&format!("upsert_entry {a}"), &want, got);
    }
    for c in list("record_block") {
        let got = dv::record_block(&c["data"], s(&c["summary"]), c["show"].as_bool().unwrap_or(false));
        check(&format!("record_block {} {:?} {}", c["data"], s(&c["summary"]), c["show"]), &c["out"], json!(got));
    }
    for c in list("entry_key") {
        check(&format!("entry_key {}", c[0]), &c[1], json!(dv::entry_key(s(&c[0]))));
    }
    println!("deliverables 探測案例：{n} 個");
    assert!(n >= 1800, "探測案例數不對：{n}");
    assert!(bad.is_empty(), "{} 個探測案例不同（共 {n} 個）：\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}
