//! 狀態機的對跑：照 0.x 的 Python 版錄下來的劇本（tests/parity/state.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作（copytree、copy、write、rm、utime）、指令（run：比 exit code、stdout、stderr 與這一步改了哪些檔）、
//! 只能在程式內呼叫的函式（call）與 selftest。暫存路徑換成 `@TMP@`、模版 repo 換成 `@SRC@` 再比。
//! 劇本裡的時間是固定的（每一步的 `now`），「今天」是劇本開頭的 `today`。
//!
//! 和 Python 不同的步驟（劇本裡帶 label）分兩種：[`STRICTER`]（這裡比較嚴：Python 放行、這裡拒絕）與
//! [`KNOWN`]（兩邊都拒絕，或都不影響判定，只是文字不同）。其餘要逐字相同。
#![cfg(unix)]

use awf_core::state::{self, Env, Fail, PyErr};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 這裡比 Python 嚴的步驟：標籤、原因。只要求這裡 exit 1（不產生腳本、不存檔）。
const STRICTER: [(&str, &str); 4] = [
    ("set-huge-int", "--set 給超出 64 位元的整數：Python 照整數放進 args；這裡放不下，當成寫錯的 --set 拒絕"),
    ("set-nan", "--set-json 給 NaN：Python 讀得進來（之後在摘要那一步才丟 TypeError）；這裡當成寫錯的 --set-json，一開始就拒絕"),
    ("output-nan", "done --output 的檔有 NaN：Python 讀得進來、照存；這裡整份當成讀不懂，拒絕存檔"),
    ("runjson-nan", "run.json 有 NaN：Python 讀得進來、照常往下；這裡整份不接受（ValueError），next、done、handoff、approve 都拒絕"),
];

/// 兩邊都拒絕、只是文字不同的步驟：標籤、原因。只比 exit code。
const KNOWN: [(&str, &str); 1] = [("set-deep", "--set-json 巢狀 3000 層：兩邊都是 RecursionError、exit 1（這裡超過 200 層就拒絕，Python 的上限大約 1000 層）；訊息文字不同，只比 exit code")];

fn repo_root() -> PathBuf {
    std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).expect("模版 repo")
}

fn expected() -> Value {
    let p = repo_root().join("tests/parity/state.expected.json");
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

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("建目錄");
    for e in std::fs::read_dir(from).unwrap_or_else(|e| panic!("讀不到 {}：{e}", from.display())).flatten() {
        let (p, name) = (e.path(), e.file_name());
        if name.to_str() == Some("__pycache__") {
            continue;
        }
        if p.is_dir() {
            copy_tree(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).expect("複製");
        }
    }
}

struct Stage {
    td: String,
    src: String,
    long: usize,
    roots: Vec<String>,
    skip: Vec<String>,
    today: String,
}

impl Stage {
    fn p(&self, rel: &str) -> PathBuf {
        Path::new(&self.td).join(rel)
    }

    fn abs(&self, text: &str) -> String {
        text.replace("@TMP@", &self.td).replace("@SRC@", &self.src)
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
        text.replace(&self.td, "@TMP@").replace(&self.src, "@SRC@")
    }

    fn norm_json(&self, v: &Value) -> Value {
        match v {
            Value::String(t) => Value::String(self.norm(t)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.norm_json(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (self.norm(k), self.norm_json(x))).collect()),
            other => other.clone(),
        }
    }

    fn walk(&self, dir: &Path, files: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten().map(|e| e.path()) {
            let rel = e.strip_prefix(&self.td).ok().and_then(|r| r.to_str()).unwrap_or_default().to_string();
            if e.is_dir() {
                if !self.skip.contains(&rel) && !rel.ends_with("__pycache__") {
                    self.walk(&e, files);
                }
            } else {
                let text = self.norm(&String::from_utf8_lossy(&std::fs::read(&e).unwrap_or_default()));
                let n = text.chars().count();
                let val = if n <= self.long { json!(text) } else { json!({"sha1": hex_bytes(&Sha1::digest(text.as_bytes())), "chars": n}) };
                files.insert(rel, val);
            }
        }
    }

    /// 專案與輸出目錄（略過複製進來的工具與知識庫索引）：相對路徑 → 內容。
    fn snapshot(&self) -> Map<String, Value> {
        let mut files = Map::new();
        for r in &self.roots {
            self.walk(&self.p(r), &mut files);
        }
        files
    }

    /// 這一步改了哪些檔（刪掉的記 null）；做完之後的快照留在 `keep`，下一步接著用（中間沒有檔案操作時不用重掃）。
    fn changed(&self, before: &Map<String, Value>, keep: &mut Option<Map<String, Value>>) -> Value {
        let after = self.snapshot();
        let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        let mut out = Map::new();
        for k in keys {
            if before.get(k) != after.get(k) {
                out.insert(k.clone(), after.get(k).cloned().unwrap_or(Value::Null));
            }
        }
        *keep = Some(after);
        Value::Object(out)
    }

    /// 這一步的環境（專案＝暫存專案；不在模版 repo 裡）。
    fn env(&self, step: &Value) -> Env {
        let e = &step["env"];
        let opt = |v: &Value| v.as_str().map(|t| self.abs(t));
        Env {
            project: self.p("proj").to_string_lossy().into_owned(),
            cwd: self.p("proj").to_string_lossy().into_owned(),
            home: self.abs(s(&e["home"])),
            plugin_env: [opt(&e["plugin_env"][0]), opt(&e["plugin_env"][1])],
            template: None,
            transcripts: opt(&e["transcripts"]),
            now: step["now"].as_f64(),
            today: Some(self.today.clone()),
        }
    }
}

/// 兩份「改了哪些檔」哪裡不同。
fn files_diff(want: &Value, got: &Value) -> Option<String> {
    let (wo, go) = (want.as_object()?, got.as_object()?);
    let keys: BTreeSet<&String> = wo.keys().chain(go.keys()).collect();
    let cut = |v: Option<&Value>| v.map_or("（沒動）".to_string(), |x| x.to_string().chars().take(1800).collect());
    let bad: Vec<String> = keys.into_iter().filter(|k| wo.get(*k) != go.get(*k)).map(|k| format!("{k}\n      要：{}\n      得：{}", cut(wo.get(k)), cut(go.get(k)))).collect();
    if bad.is_empty() { None } else { Some(bad.join("\n    ")) }
}

fn fail_json(f: Fail) -> Value {
    match f {
        Fail::Awf(m) => json!({"awf": m}),
        Fail::Py(e) => json!({"raises": e.class}),
    }
}

fn py<T: Into<Value>>(r: Result<T, PyErr>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(e) => json!({"raises": e.class}),
    }
}

fn awf<T: Into<Value>>(r: Result<T, Fail>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.into()}),
        Err(f) => fail_json(f),
    }
}

/// 只能在程式內呼叫的函式；回傳 `{"ok": …}`、`{"awf": 訊息}` 或 `{"raises": 類別}`。
fn call(env: &Env, fn_name: &str, a: &Value) -> Value {
    let st = |k: &str| s(&a[k]).to_string();
    let opt = |k: &str| a[k].as_str().filter(|x| !x.is_empty()).map(str::to_string);
    match fn_name {
        "read_config" => py(state::read_config(env).map(Value::Object)),
        "plugin_candidates" => py(state::plugin_candidates(env, Some(&st("home"))).map(|cs| {
            Value::Array(cs.iter().map(|c| json!({"key": c.key, "path": c.path, "version": c.version, "mine": c.mine, "other_project": c.other_project, "updated": c.updated})).collect())
        })),
        "find_plugin" => {
            let mut e2 = Env { home: st("home"), ..clone_env(env) };
            e2.plugin_env = env.plugin_env.clone();
            awf(state::find_plugin(&e2, &st("explicit")).map(|(p, notes)| json!([p, notes])))
        }
        "event_route" => py(state::event_route(env, &st("event")).map(|r| r.map_or(Value::Null, |(w, stg)| json!([w, stg])))),
        "event_required" => py(state::event_required(env, &st("event")).map(|v| json!(v))),
        "find_template" => awf(state::find_template(env, &st("name"), opt("plugin").as_deref()).map(|(text, source, notes)| json!([hex_bytes(&Sha1::digest(text.as_bytes())), source, notes]))),
        "find_wf_meta" => awf(state::find_wf_meta(env, &st("script"), a["since"].as_f64().unwrap_or(0.0)).map(|r| match r {
            Some((meta, f)) => json!([meta.get("runId").cloned().unwrap_or(Value::Null), f]),
            None => json!([null, null]),
        })),
        "current_session" => py(state::current_session(env).map(|v| json!(v))),
        "stage_args" => {
            let run = format!("{}/{}", env.project, st("run"));
            let data = match a.get("data") {
                Some(d) => Ok(d.clone()),
                None => awf_core::save_run::load(&env.sr(), &run),
            };
            py(data.and_then(|d| state::stage_args(env, &run, &d, &st("stage"), &st("workflow"))).map(|(m, missing, notes)| json!([m, missing, notes])))
        }
        "provisional" => py(state::provisional(&a["res"]).map(Value::Array)),
        "envelopes" => json!({"ok": state::envelopes(&a["res"]).into_iter().map(|(name, _)| name).collect::<Vec<_>>()}),
        "text_of" => py(state::text_of(&a["act"]).map(|v| json!(v))),
        "custom_roles" => py(state::custom_roles(env).map(|v| json!(v))),
        "workflow_names" => py(state::workflow_names(env, opt("plugin").as_deref()).map(|v| json!(v))),
        other => panic!("劇本有不認得的函式：{other}"),
    }
}

fn clone_env(e: &Env) -> Env {
    Env { project: e.project.clone(), cwd: e.cwd.clone(), home: e.home.clone(), plugin_env: e.plugin_env.clone(), template: e.template.clone(), transcripts: e.transcripts.clone(), now: e.now, today: e.today.clone() }
}

fn set_mtime(p: &Path, t: f64) {
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(t);
    std::fs::OpenOptions::new().write(true).open(p).and_then(|f| f.set_modified(when)).unwrap_or_else(|e| panic!("改不了 {} 的修改時間：{e}", p.display()));
}

struct Tally {
    runs: usize,
    calls: usize,
    same: usize,
    stricter: usize,
    known: usize,
    bad: Vec<String>,
}

fn run_steps(exp: &Value, st: &Stage) -> Tally {
    let mut t = Tally { runs: 0, calls: 0, same: 0, stricter: 0, known: 0, bad: Vec::new() };
    let steps = exp["steps"].as_array().expect("steps");
    // 上一步做完之後的快照；檔案操作會讓它失效
    let mut kept: Option<Map<String, Value>> = None;
    for (i, step) in steps.iter().enumerate() {
        let path = s(&step["path"]);
        if !matches!(s(&step["op"]), "run" | "call") {
            kept = None;
        }
        let label = step.get("label").and_then(Value::as_str);
        let stricter = label.is_some_and(|l| STRICTER.iter().any(|(k, _)| *k == l));
        let known = label.is_some_and(|l| KNOWN.iter().any(|(k, _)| *k == l));
        if let Some(l) = label {
            assert!(stricter || known, "第 {i} 步的標籤 {l} 沒有列在 STRICTER 或 KNOWN");
        }
        match s(&step["op"]) {
            "copytree" => copy_tree(&Path::new(&st.src).join(s(&step["from"])), &st.p(s(&step["to"]))),
            "copy" => {
                let to = st.p(s(&step["to"]));
                std::fs::create_dir_all(to.parent().expect("上層目錄")).expect("建目錄");
                std::fs::copy(Path::new(&st.src).join(s(&step["from"])), &to).expect("複製");
            }
            "write" => {
                let data = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None => st.abs(s(&step["text"])).into_bytes(),
                };
                let full = st.p(path);
                std::fs::create_dir_all(full.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&full, data).unwrap_or_else(|e| panic!("寫不進 {path}：{e}"));
            }
            "rm" => {
                let full = st.p(path);
                if full.is_dir() {
                    std::fs::remove_dir_all(&full).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}"));
                } else if full.exists() {
                    std::fs::remove_file(&full).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}"));
                }
            }
            "utime" => set_mtime(&st.p(path), step["mtime"].as_f64().expect("mtime")),
            "run" => {
                t.runs += 1;
                let shown = strings(&step["argv"]);
                let argv: Vec<String> = shown.iter().map(|a| st.abs(a)).collect();
                let before = kept.take().unwrap_or_else(|| st.snapshot());
                let (code, out, err) = state::run_captured(&argv, &st.env(step));
                let (out, err) = (st.norm(&out), st.norm(&err));
                let is = |k: &str| step.get(k).is_some();
                let mut diff = Vec::new();
                if stricter {
                    // Python 放行或晚一步才出錯；這裡一開始就拒絕
                    if code != 1 {
                        diff.push(format!("要拒絕（exit 1），得 {code}"));
                    }
                    if let Some(d) = files_diff(&json!({}), &st.changed(&before, &mut kept)) {
                        diff.push(format!("拒絕時不該改到檔案\n    {d}"));
                    }
                } else {
                    if Some(i64::from(code)) != step["code"].as_i64() {
                        diff.push(format!("exit code 要 {}、得 {code}\n    stderr：{err:?}", step["code"]));
                    }
                    // 用法訊息與已知差異只比 exit code
                    if !is("usage") && !known {
                        if out != s(&step["stdout"]) {
                            diff.push(format!("stdout\n    要：{:?}\n    得：{out:?}", s(&step["stdout"])));
                        }
                        match step.get("raises").and_then(Value::as_str) {
                            // Python 沒接住的例外：那邊是 traceback，這裡印「awf：類別：訊息」
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
                        if let Some(d) = files_diff(&step["files"], &st.changed(&before, &mut kept)) {
                            diff.push(format!("改到的檔\n    {d}"));
                        }
                    }
                }
                if !diff.is_empty() {
                    t.bad.push(format!("第 {i} 步 awf {shown:?}（{}）\n  {}", s(&step["note"]), diff.join("\n  ")));
                } else if stricter {
                    t.stricter += 1;
                } else if known {
                    t.known += 1;
                } else {
                    t.same += 1;
                }
            }
            "call" => {
                t.calls += 1;
                let before = kept.take().unwrap_or_else(|| st.snapshot());
                let name = s(&step["fn"]);
                let got = st.norm_json(&call(&st.env(step), name, &st.abs_json(&step["args"])));
                let mut diff = Vec::new();
                if !same(&step["result"], &got) {
                    diff.push(format!("回傳\n    要：{}\n    得：{got}", step["result"]));
                }
                if let Some(d) = files_diff(&step["files"], &st.changed(&before, &mut kept)) {
                    diff.push(format!("改到的檔\n    {d}"));
                }
                if diff.is_empty() {
                    t.same += 1;
                } else {
                    t.bad.push(format!("第 {i} 步 {name} {}（{}）\n  {}", step["args"].to_string().chars().take(300).collect::<String>(), s(&step["note"]), diff.join("\n  ")));
                }
            }
            "selftest" => {
                t.runs += 1;
                let mut env = st.env(step);
                env.plugin_env = [None, None];
                env.now = None;
                env.today = None;
                let (code, out, _) = state::run_captured(&["--selftest".to_string()], &env);
                let mut diff = Vec::new();
                if Some(i64::from(code)) != step["code"].as_i64() {
                    diff.push(format!("exit code 要 {}、得 {code}", step["code"]));
                }
                let out = st.norm(&out);
                if out != s(&step["stdout"]) {
                    let (want, got): (Vec<&str>, Vec<&str>) = (s(&step["stdout"]).lines().collect(), out.lines().collect());
                    let at = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
                    diff.push(format!("selftest 的輸出從第 {} 行開始不同（要 {} 行、得 {} 行）\n    要：{:?}\n    得：{:?}", at + 1, want.len(), got.len(), want.get(at), got.get(at)));
                }
                if diff.is_empty() {
                    t.same += 1;
                } else {
                    t.bad.push(format!("第 {i} 步 selftest\n  {}", diff.join("\n  ")));
                }
            }
            other => panic!("劇本有不認得的步驟：{other}"),
        }
    }
    t
}

/// 產生的 12 支腳本用 node 乾跑（check_workflows.mjs 載入 plugin/workflow-templates/scenarios/ 的情境）。沒有 node 就略過。
fn dry_run(st: &Stage) -> Option<String> {
    if std::process::Command::new("node").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        return None;
    }
    let src = Path::new(&st.src);
    let o = std::process::Command::new("node")
        .arg(src.join("plugin/workflow-templates/check_workflows.mjs"))
        .arg("--agents-dir")
        .arg(src.join("plugin/agents"))
        .arg("--wf-dir")
        .arg(st.p("out/all"))
        .current_dir(st.p("proj"))
        .output()
        .expect("node");
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success(), "產生的腳本乾跑沒過：\n{}", text.chars().rev().take(3000).collect::<String>().chars().rev().collect::<String>());
    Some(text.lines().last().unwrap_or_default().to_string())
}

#[test]
fn state_matches_python() {
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("state-parity-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&td).expect("建暫存目錄");
    let td = std::fs::canonicalize(&td).expect("暫存目錄");
    let st = Stage {
        td: td.to_string_lossy().into_owned(),
        src: repo_root().to_string_lossy().into_owned(),
        long: exp["long"].as_u64().expect("long") as usize,
        roots: strings(&exp["snapshot"]["roots"]),
        skip: strings(&exp["snapshot"]["skip"]),
        today: s(&exp["today"]).to_string(),
    };
    let prev = std::env::current_dir().expect("工作目錄");
    std::fs::create_dir_all(st.p("proj")).expect("建專案目錄");
    std::env::set_current_dir(st.p("proj")).expect("切換工作目錄");
    let t = run_steps(&exp, &st);
    let rendered = std::fs::read_dir(st.p("out/all")).map(|rd| rd.flatten().count()).unwrap_or(0);
    let dry = if t.bad.is_empty() { dry_run(&st) } else { None };
    let _ = std::env::set_current_dir(prev);
    let _ = std::fs::remove_dir_all(&td);
    println!(
        "state 對跑：指令 {} 步、函式 {} 個；{} 個逐字相同，{} 個比 Python 嚴（STRICTER），{} 個是列出的已知差異（KNOWN）",
        t.runs, t.calls, t.same, t.stricter, t.known
    );
    for (label, why) in STRICTER {
        println!("  比較嚴 {label}：{why}");
    }
    for (label, why) in KNOWN {
        println!("  已知差異 {label}：{why}");
    }
    match &dry {
        Some(line) => println!("  render --all 的 {rendered} 支腳本用 node 乾跑通過：{line}"),
        None => println!("  render --all 的 {rendered} 支腳本：沒有 node 或前面有步驟不同，沒有乾跑"),
    }
    assert!(t.runs >= 400, "指令步數不對：{}", t.runs);
    assert!(t.calls >= 70, "函式呼叫數不對：{}", t.calls);
    assert!(t.bad.is_empty(), "{} 步不同（共 {} 步）：\n{}", t.bad.len(), t.runs + t.calls, t.bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    assert_eq!(rendered, 12, "render --all 要產生 12 支腳本");
}
