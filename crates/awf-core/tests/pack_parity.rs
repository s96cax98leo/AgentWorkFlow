//! 資料包的對跑：照 0.x 的 Python 版錄下來的劇本（tests/parity/pack.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作、指令（比 exit code、stdout、stderr 與這一步改了哪些檔）和只能在程式內呼叫的函式（call）。
//! `probes` 是不碰檔案的函式探測。暫存路徑換成 `@TMP@` 再比。劇本有幾步用相對路徑，所以整份在同一個測試裡跑（會切換工作目錄）。
//!
//! 和 Python 不同的步驟列在 [`KNOWN`]（都不放寬判定），其餘要逐字相同。
#![cfg(unix)]

use awf_core::pack;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 比這長的檔案內容，劇本裡只記 sha1（和錄製腳本的 LONG 相同）。
const LONG: usize = 4000;

/// 已知和 Python 不同的步驟（劇本裡帶 label）：標籤、原因。
/// - `nan-run`：run.json 裡有 NaN。Python 讀得進來（需求變成字串 nan）；這裡整份當成讀不懂（沒有需求）。只比 exit code。
/// - `nan-store`：reuse.json 裡有 NaN。Python 讀得進來、逐條列出不符；這裡整份當成沒有紀錄。兩邊都判不符，只是理由不同：
///   只要求 `verify_reuse` 回傳的理由不是空的。
/// - `lone-surrogate`：reuse.json 的原文有落單的代理字元跳脫。Python 讀得進來、判信封不一致；這裡整份當成沒有紀錄。同上。
/// - `huge-int`：信封裡有超出 64 位元範圍的整數。Python 照整數比、判相符；這裡分不出原本是不是同一個數，判不符（更嚴）。
const KNOWN: [(&str, &str); 5] = [
    ("nan-run", "run.json 有 NaN：整份當成讀不懂"),
    ("nan-store", "reuse.json 有 NaN：整份當成沒有紀錄（一樣判不符）"),
    ("lone-surrogate", "reuse.json 有落單的代理字元跳脫：整份當成沒有紀錄（一樣判不符）"),
    ("angle-path", "沿用的角度名稱帶路徑分隔或 ..：一律判不符（Python 會拿它組紀錄檔路徑，可能對到 run 目錄外而判相符，這裡更嚴）"),
    ("huge-int", "信封裡有超出 64 位元範圍的整數：判不符（Python 判相符，這裡更嚴）"),
];

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pack.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
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

    /// 整個 JSON 值裡的 `@TMP@` 都換成暫存目錄。
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
            let name = e.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if e.is_dir() {
                // 知識庫的索引目錄（_tpm/kb）不比：kb.sqlite 是二進位檔、hot.md 會印修改時間
                let in_tpm = dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_tpm"));
                if name != ".git" && !(name == "kb" && in_tpm) {
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

    /// 整棵暫存目錄（略過 .git 與知識庫索引）：相對路徑 → 內容。
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

fn py_result<T: Into<Value>>(r: Result<T, pack::PyErr>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(e) => json!({"raises": e.class, "msg": e.msg}),
    }
}

fn headings_json(heads: &[pack::Heading]) -> Value {
    Value::Array(heads.iter().map(|h| json!({"level": h.level, "title": h.title, "start": h.start, "end": h.end})).collect())
}

/// 只能在程式內呼叫的函式；回傳 `{"ok": …}` 或 `{"raises": 類別, "msg": 訊息}`。
fn call(fn_name: &str, a: &Value) -> Value {
    let st = |k: &str| s(&a[k]).to_string();
    let opt = |k: &str| a[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    let (repo, root) = (opt("repo"), opt("root"));
    let (repo, root_opt) = (repo.as_deref(), root.as_deref());
    let root = root_opt.unwrap_or_default();
    let ok = |v: Value| json!({"ok": v});
    match fn_name {
        "sha1_file" => ok(json!(pack::sha1_file(&st("p")))),
        "dir_listing_sha1" => ok(json!(pack::dir_listing_sha1(&st("d")))),
        "resolve_source" => ok(json!(pack::resolve_source(repo, &st("raw"), root_opt))),
        "requirement_file" => ok(json!(pack::requirement_file(&st("raw"), root))),
        "requirement_key" => ok(json!(pack::requirement_key(&st("raw"), root))),
        "requirement_text" => ok(json!(pack::requirement_text(&st("raw"), root))),
        "load_store" => ok(Value::Object(pack::load_store(&st("run")))),
        "source_dirs" => ok(json!(pack::source_dirs(repo, a["srcs"].as_array().map_or(&[][..], Vec::as_slice), root_opt))),
        "sources_changed" => ok(json!(pack::sources_changed(repo, &a["entry"], root_opt))),
        "reuse_candidates" => py_result(pack::reuse_candidates(&st("run"), repo, &st("requirement"), &a["plan_mode"], &strings(&a["extra_runs"]), a["keep"].as_bool().unwrap_or(false), root).map(|(m, n)| json!([m, n]))),
        "verify_reuse" => ok(json!(pack::verify_reuse(&st("run"), repo, &a["result"], a["requirement"].as_str(), root))),
        "record_reuse" => py_result(pack::record_reuse(&st("run"), repo, &a["result"], &st("requirement"), root)),
        "load_decisions" => ok(json!(pack::load_decisions(&st("runs")))),
        "relevant_decisions" => py_result(pack::relevant_decisions(&st("runs"), &st("run_name"), &st("epic"), &st("requirement"), &st("today"), &strings(&a["family"]))),
        "decisions_view" => ok(json!(pack::decisions_view(&st("runs"), &st("today")))),
        "run_epic" => ok(json!(pack::run_epic(&st("run"), Some(&a["data"]).filter(|d| !d.is_null())))),
        "spec_dir" => ok(json!(pack::spec_dir(repo, &st("epic"), &st("raw"), root))),
        "find_subtasks" => {
            let (subs, epic) = pack::find_subtasks(&st("run"), &st("raw"));
            ok(json!([subs, epic]))
        }
        "md_headings" => ok(headings_json(&pack::md_headings(&st("path")))),
        "resolve_spec_ref" => ok(match pack::resolve_spec_ref(&st("spec"), &st("ref")) {
            Ok(h) => json!({"hit": {"file": h.file, "title": h.title, "start": h.start, "end": h.end}, "err": ""}),
            Err(e) => json!({"hit": null, "err": e}),
        }),
        "build_task_pack" => {
            let all = a["all"].as_array().cloned().unwrap_or_default();
            let spec = opt("spec");
            py_result(pack::build_task_pack(&a["task"], &all, spec.as_deref(), &st("label"), a.get("profile"), &strings(&a["bases"])).map(|(t, e)| json!([t, e])))
        }
        "build_pack" => {
            let hits = a["hits"].as_array().cloned().unwrap_or_default();
            let decs = a["decisions"].as_array().cloned().unwrap_or_default();
            ok(json!(pack::build_pack(&st("stage"), &st("run"), root, &st("requirement"), &st("recipe_line"), &hits, &st("kb_note"), &strings(&a["notes"]), &decs)))
        }
        other => panic!("劇本有不認得的函式：{other}"),
    }
}

fn run_steps(exp: &Value, st: &Stage) -> (usize, usize, usize, Vec<String>) {
    let selftest_tmp = regex::Regex::new(r#"/[^\s"：（]*?awf-pack\.[A-Za-z0-9_]+"#).expect("regex");
    let mut bad: Vec<String> = Vec::new();
    let (mut runs, mut same_n, mut known_n) = (0usize, 0usize, 0usize);
    let steps = exp["steps"].as_array().expect("steps");
    for (i, step) in steps.iter().enumerate() {
        let path = s(&step["path"]);
        let label = step.get("label").and_then(Value::as_str);
        if let Some(l) = label {
            assert!(KNOWN.iter().any(|(k, _)| *k == l), "第 {i} 步的標籤 {l} 沒有列在 KNOWN");
        }
        match s(&step["op"]) {
            "write" => {
                let data = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None => st.abs(s(&step["text"])).into_bytes(),
                };
                let full = st.p(path);
                std::fs::create_dir_all(full.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&full, data).unwrap_or_else(|e| panic!("寫不進 {path}：{e}"));
            }
            "rm" => std::fs::remove_file(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "rmtree" => std::fs::remove_dir_all(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "mkdir" => std::fs::create_dir_all(st.p(path)).unwrap_or_else(|e| panic!("建不了 {path}：{e}")),
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
            "run" => {
                runs += 1;
                let shown = strings(&step["argv"]);
                let argv: Vec<String> = shown.iter().map(|a| st.abs(a)).collect();
                let root = st.abs(s(&step["root"]));
                let before = st.snapshot();
                let (mut out, mut err) = (String::new(), String::new());
                let code = pack::run(&root, &argv, &mut out, &mut err);
                let is = |k: &str| step.get(k).is_some();
                let out = if is("selftest") { selftest_tmp.replace_all(&out, "@TMP@").into_owned() } else { st.norm(&out) };
                let err = st.norm(&err);
                let mut diff = Vec::new();
                if Some(i64::from(code)) != step["code"].as_i64() {
                    diff.push(format!("exit code 要 {}、得 {code}", step["code"]));
                }
                // 用法訊息（指令名改成 awf pack）與已知差異只比 exit code
                if !is("usage") && label.is_none() {
                    if out != s(&step["stdout"]) {
                        diff.push(format!("stdout\n    要：{:?}\n    得：{out:?}", s(&step["stdout"])));
                    }
                    match step.get("raises").and_then(Value::as_str) {
                        // Python 沒接住的例外：那邊是 traceback，這裡印「類別：訊息」
                        Some(class) => {
                            if !err.contains(class) {
                                diff.push(format!("stderr 要提到 {class}，得 {err:?}"));
                            }
                        }
                        None => {
                            if err != s(&step["stderr"]) {
                                diff.push(format!("stderr\n    要：{:?}\n    得：{err:?}", s(&step["stderr"])));
                            }
                        }
                    }
                    if let Some(d) = files_diff(&step["files"], &st.changed(&before)) {
                        diff.push(format!("改到的檔\n    {d}"));
                    }
                }
                if label.is_some() && diff.is_empty() {
                    known_n += 1;
                } else if diff.is_empty() {
                    same_n += 1;
                } else {
                    bad.push(format!("第 {i} 步 pack {shown:?}\n  {}", diff.join("\n  ")));
                }
            }
            "call" => {
                runs += 1;
                let before = st.snapshot();
                let name = s(&step["fn"]);
                let got = st.norm_json(&call(name, &st.abs_json(&step["args"])));
                let mut diff = Vec::new();
                let rejected = |v: &Value| v["ok"].as_array().is_some_and(|a| !a.is_empty());
                if matches!(label, Some("nan-store" | "lone-surrogate")) {
                    // 兩邊都判不符，理由不同：只要求有理由
                    if !rejected(&got) || !rejected(&step["result"]) {
                        diff.push(format!("要判不符（理由不是空的），得 {got}"));
                    }
                } else if label == Some("angle-path") {
                    // Python 可能判相符（紀錄檔對到 run 目錄外），這裡一律判不符
                    if !rejected(&got) {
                        diff.push(format!("要判不符，得 {got}"));
                    }
                } else if label == Some("huge-int") {
                    // Python 判相符，這裡更嚴
                    if !rejected(&got) || rejected(&step["result"]) {
                        diff.push(format!("要判不符（Python 判相符），得 {got}"));
                    }
                } else if !same(&step["result"], &got) {
                    diff.push(format!("回傳\n    要：{}\n    得：{got}", step["result"]));
                }
                if let Some(d) = files_diff(&step["files"], &st.changed(&before)) {
                    diff.push(format!("改到的檔\n    {d}"));
                }
                if label.is_some() && diff.is_empty() {
                    known_n += 1;
                } else if diff.is_empty() {
                    same_n += 1;
                } else {
                    bad.push(format!("第 {i} 步 {name} {}\n  {}", step["args"], diff.join("\n  ")));
                }
            }
            other => panic!("劇本有不認得的步驟：{other}"),
        }
    }
    (runs, same_n, known_n, bad)
}

fn tried<T: Into<Value>>(r: Result<T, pack::PyErr>) -> Value {
    py_result(r)
}

fn run_probes(exp: &Value) -> (usize, Vec<String>) {
    let pr = &exp["probes"];
    let mut bad: Vec<String> = Vec::new();
    let mut n = 0usize;
    let mut check = |what: String, want: &Value, got: Value| {
        n += 1;
        if !same(want, &got) {
            bad.push(format!("{what}\n  要：{want}\n  得：{got}"));
        }
    };
    let list = |k: &str| -> &Vec<Value> { pr[k].as_array().unwrap_or_else(|| panic!("探測少了 {k}")) };
    for c in list("path_overlap") {
        check(format!("path_overlap {} {}", c[0], c[1]), &c[2], json!(pack::path_overlap(s(&c[0]), s(&c[1]))));
    }
    for c in list("flat") {
        let limit = c[1].as_u64().unwrap_or(80) as usize;
        check(format!("flat {} {limit}", c[0]), &c[2], json!(pack::flat(Some(&c[0]), limit)));
    }
    for c in list("norm_q") {
        check(format!("norm_q {}", c[0]), &c[1], json!(pack::norm_q(s(&c[0]))));
    }
    for c in list("norm_title") {
        check(format!("norm_title {}", c[0]), &c[1], json!(pack::norm_title(s(&c[0]))));
    }
    for c in list("decision_expired") {
        let d = json!({"expires": c[0]});
        check(format!("decision_expired {} {}", c[0], c[1]), &c[2], json!(pack::decision_expired(&d, s(&c[1]))));
    }
    for c in list("date") {
        check(format!("DATE {}", c[0]), &c[1], json!(pack::DATE.is_match(s(&c[0]))));
    }
    for c in list("current_decisions") {
        let decs = c[0].as_array().cloned().unwrap_or_default();
        check(format!("current_decisions {}", c[0]), &c[1], json!(pack::current_decisions(&decs)));
    }
    for c in list("decision_lines") {
        let decs = c[0].as_array().cloned().unwrap_or_default();
        check(format!("decision_lines {}", c[0]), &c[1], json!(pack::decision_lines(&decs)));
    }
    for c in list("recipe_line_of") {
        check(format!("recipe_line_of {}", c[0]), &c[1], tried(pack::recipe_line_of(&c[0])));
    }
    for c in list("task_constraints") {
        let got = pack::task_constraints(Some(&c[0]), Some(&c[1])).map(|cs| Value::Array(cs.iter().map(|x| x.get("id").cloned().unwrap_or(Value::Null)).collect()));
        check(format!("task_constraints {} {}", c[0], c[1]), &c[2], tried(got));
    }
    for c in list("sensitive") {
        check(format!("sensitive {}", c[0]), &c[1], json!(pack::sensitive(s(&c[0]))));
    }
    (n, bad)
}

#[test]
fn pack_matches_python() {
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("pack-parity-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&td).expect("建暫存目錄");
    let st = Stage { td: td.to_string_lossy().into_owned(), real: std::fs::canonicalize(&td).expect("暫存目錄").to_string_lossy().into_owned() };
    let prev = std::env::current_dir().expect("工作目錄");
    std::env::set_current_dir(&td).expect("切換工作目錄");
    let (runs, same_n, known_n, bad) = run_steps(&exp, &st);
    let (probes, bad_probes) = run_probes(&exp);
    let _ = std::env::set_current_dir(prev);
    let _ = std::fs::remove_dir_all(&td);
    println!("pack 對跑：{runs} 步（指令與函式），{same_n} 步逐字相同，{known_n} 步是列出的已知差異；探測 {probes} 個，{} 個相同", probes - bad_probes.len());
    for (label, why) in KNOWN {
        println!("  已知差異 {label}：{why}");
    }
    assert!(runs >= 500, "步數不對：{runs}");
    assert!(probes >= 1300, "探測案例數不對：{probes}");
    assert!(bad.is_empty(), "{} 步不同（共 {runs} 步）：\n{}", bad.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
    assert!(bad_probes.is_empty(), "{} 個探測案例不同（共 {probes} 個）：\n{}", bad_probes.len(), bad_probes.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
    assert_eq!(same_n + known_n, runs);
}
