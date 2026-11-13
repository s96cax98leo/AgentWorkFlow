//! 確定性推送工具（publish）的對跑：照 Python 版錄下來的劇本（tests/parity/publish.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作（write、rm）、git 指令、publish 的指令（比 exit code、stdout 與這一步改了哪些檔）
//! 和只能在程式內呼叫的函式（call）。暫存路徑換成 `@TMP@` 再比。
//!
//! 絕對不碰真的遠端：remote 是暫存目錄裡的 bare repo；`gh` 是暫存目錄 `bin/` 裡的假腳本（只加在子程序的 PATH 最前面）。
//!
//! - 不切換行程的工作目錄：相對路徑的基準用 `Ctx.cwd` 給。
//! - git 的身分與日期每個指令都明確給（劇本的 `git_env`），所以 SHA 和基準相同。
//! - git 自己的錯誤文字隨版本不同，換成 `@GIT@`；沒給 `--timestamp` 的那一步把時間換成 `@NOW@`。
//!
//! 和 Python 不同的步驟帶 label，其餘要逐字相同：
//! - [`STRICTER`]：Python 接受、這裡拒絕。只會更嚴。
//! - [`KNOWN`]：兩邊都拒絕，只是文字或例外類別不同（目前沒有）。
//!
//! 兩張表都有「沒有過時」的檢查：表上的 label 在劇本裡要真的有一步和 Python 不同，劇本裡的 label 也都要在表上。
#![cfg(unix)]

use awf_core::publish::{self as pb, Config, Env, Stop};
use awf_core::save_run::Ctx;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Python 接受、這裡拒絕的步驟：標籤、原因。要求：這裡不能是通過（指令 exit code 不是 0、沒有推送；函式回拒絕或例外）。
const STRICTER: [(&str, &str); 7] = [
    ("nan-run", "run.json 有 NaN／Infinity：Python 讀得進來、照常判定；這裡當成讀不懂，拒絕"),
    ("huge-int-run", "run.json 有超出 64 位元範圍的整數：Python 照常判定；這裡當成讀不懂，拒絕"),
    ("deep-run", "run.json 巢狀超過 200 層：Python 照常判定；這裡當成讀不懂（不寫稽核紀錄、exit 1）"),
    ("surrogate-run", "run.json 有落單的代理字元跳脫：Python 讀得進來；這裡讀的時候就拒絕"),
    ("nan-stage-file", "政策核准的段的結果檔有 NaN：Python 讀得進來再判；這裡當成讀不懂，拒絕"),
    ("nan-config", "guard.config.json 有 NaN：Python 讀得進來、照用；這裡當成壞掉的設定（空的），沒有 fork_repo 就拒絕"),
    ("supersedes-digits", "--supersedes 是全形數字或超出 64 位元的整數：Python 的 int() 收；這裡當成參數錯誤"),
];

/// 兩邊都拒絕、只是文字或例外類別不同的步驟。
const KNOWN: [(&str, &str); 0] = [];

static LOCK: Mutex<()> = Mutex::new(());

/// 這個檔的測試一次只跑一個，並拿掉外面帶進來的 git 身分與日期。
fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for k in ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_AUTHOR_DATE", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL", "GIT_COMMITTER_DATE"] {
        std::env::remove_var(k);
    }
    guard
}

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/publish.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn src_root() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("模版根目錄").to_string_lossy().into_owned()
}

fn teams() -> String {
    format!("{}/plugin/scaffold/.claude/teams", src_root())
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default()
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

struct Stage {
    td: String,
    env: Env,
    git_env: Vec<(String, String)>,
    git_text: regex::Regex,
    now: regex::Regex,
    timestamp: String,
    src: String,
}

impl Stage {
    fn new(exp: &Value) -> Stage {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("publish-parity-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建立暫存目錄");
        let td = dir.canonicalize().expect("暫存目錄").to_string_lossy().into_owned();
        let mut git_env: Vec<(String, String)> = exp["git_env"].as_object().expect("git_env").iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
        git_env.push(("GIT_CONFIG_GLOBAL".into(), format!("{td}/gitconfig")));
        let mut env = git_env.clone();
        // 假的 gh 只加在子程序的 PATH 最前面，不動這個行程的環境
        env.push(("PATH".into(), format!("{td}/bin:{}", std::env::var("PATH").unwrap_or_default())));
        let ctx = Ctx { teams: teams(), profile: format!("{td}/proj/.claude/teams/methodology.json"), pack_root: format!("{td}/proj"), cwd: td.clone(), today: None };
        let env = Env { ctx, project: format!("{td}/proj"), git: vec!["git".into()], gh: vec!["gh".into()], env };
        Stage {
            td,
            env,
            git_env,
            git_text: regex::Regex::new(r#"git (push|diff) 失敗：(?:[^"\\]|\\.)*"#).expect("固定的正規式"),
            now: regex::Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\+00:00").expect("固定的正規式"),
            timestamp: s(&exp["timestamp"]).to_string(),
            src: src_root(),
        }
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
        let t = text.replace(&self.td, "@TMP@").replace(&self.src, "@SRC@");
        self.git_text.replace_all(&t, "git $1 失敗：@GIT@").into_owned()
    }

    fn norm_json(&self, v: &Value) -> Value {
        match v {
            Value::String(t) => Value::String(self.norm(t)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.norm_json(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.norm_json(x))).collect()),
            other => other.clone(),
        }
    }

    /// 不是劇本固定時間戳的時間換成 `@NOW@`。
    fn fix_now(&self, text: &str) -> String {
        self.now.replace_all(text, |c: &regex::Captures| if c[0] == self.timestamp { c[0].to_string() } else { "@NOW@".to_string() }).into_owned()
    }

    fn walk(&self, dir: &Path, top: bool, files: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten().map(|e| e.path()) {
            let name = e.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if e.is_dir() {
                if name != ".git" && !(top && name == "remotes") {
                    self.walk(&e, false, files);
                }
            } else {
                let rel = e.strip_prefix(&self.td).ok().and_then(|r| r.to_str()).unwrap_or_default().to_string();
                files.insert(rel, json!(self.norm(&String::from_utf8_lossy(&std::fs::read(&e).unwrap_or_default()))));
            }
        }
    }

    fn snapshot(&self) -> Map<String, Value> {
        let mut files = Map::new();
        self.walk(Path::new(&self.td), true, &mut files);
        files
    }

    /// 這一步改了哪些檔；只往後附加的檔記 `{"append": 新增的部分}`。
    fn changed(&self, before: &Map<String, Value>, now: bool) -> Value {
        let after = self.snapshot();
        let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        let fix = |t: &str| if now { self.fix_now(t) } else { t.to_string() };
        let mut out = Map::new();
        for k in keys {
            if before.get(k) == after.get(k) {
                continue;
            }
            let val = match (before.get(k).and_then(Value::as_str), after.get(k).and_then(Value::as_str)) {
                (Some(a), Some(b)) if b.starts_with(a) => json!({"append": fix(b.get(a.len()..).unwrap_or_default())}),
                (_, Some(b)) => json!(fix(b)),
                _ => Value::Null,
            };
            out.insert(k.clone(), val);
        }
        Value::Object(out)
    }

    fn git(&self, cwd: &str, args: &[String]) -> (i32, String) {
        let out = Command::new("git").args(args.iter().map(|a| self.abs(a))).current_dir(self.p(cwd)).envs(self.git_env.iter().cloned()).stdin(std::process::Stdio::null()).output().expect("git");
        (out.status.code().unwrap_or(-1), self.norm(&String::from_utf8_lossy(&out.stdout)))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.td);
    }
}

fn config(v: &Value) -> Config {
    let list = |k: &str, default: &[&str]| match v.get(k) {
        Some(x) => strings(x),
        None => default.iter().map(|x| x.to_string()).collect(),
    };
    Config {
        fork_repo: s(&v["fork_repo"]).to_string(),
        upstream_repos: list("upstream_repos", &[]),
        forbidden: list("forbidden", &[]),
        prefixes: list("prefixes", &pb::DEFAULT_PREFIXES),
        protected: list("protected", &pb::DEFAULT_PROTECTED),
        runs_dir: v.get("runs_dir").map_or("_tpm/runs", s).to_string(),
    }
}

/// 例外訊息開頭的類別名（「類別：訊息」；parse_diff 的是「KeyError：…」）。
fn class_of(msg: &str) -> String {
    msg.split('：').next().unwrap_or_default().to_string()
}

fn stop(r: Result<String, Stop>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(Stop::Refused(m)) => json!({"refused": m}),
        Err(Stop::Crash(m)) => json!({"raises": class_of(&m)}),
    }
}

fn unit(r: Result<(), String>) -> Value {
    match r {
        Ok(()) => json!({"ok": null}),
        Err(m) => json!({"refused": m}),
    }
}

/// 只能在程式內呼叫的函式。參數的對應和錄製腳本的 CALLS 相同。
fn call(ctx: &Ctx, fn_name: &str, a: &Value) -> Value {
    let st = |k: &str| s(&a[k]).to_string();
    match fn_name {
        "slug_of" => json!({"ok": pb::slug_of(&st("url"))}),
        "secret_file" => json!({"ok": pb::secret_file(&st("path"))}),
        "scan_secrets" => match pb::scan_secrets(&st("diff"), &strings(&a["literals"]), &strings(&a["names"])) {
            Ok(hits) => json!({"ok": hits}),
            Err(m) => json!({"raises": class_of(&m)}),
        },
        "check_target" => unit(pb::check_target(&config(&a["cfg"]), &st("branch"), &st("base"))),
        "check_urls" => unit(pb::check_urls(&strings(&a["urls"]), &st("repo"), &st("remote"), &config(&a["cfg"]))),
        "load_config" => match pb::load_config(&st("proj")) {
            Ok(c) => json!({"ok": c.to_value()}),
            Err(e) => json!({"raises": e.class}),
        },
        "positive_int" => match pb::positive_int(&st("v")) {
            Ok(n) => json!({"ok": n}),
            Err(m) => json!({"refused": m}),
        },
        "supersedes_line" => json!({"ok": pb::supersedes_line(a["n"].as_i64().unwrap_or(0))}),
        "check_policy" => stop(pb::check_policy(ctx, a["run"].as_str().filter(|r| !r.is_empty()), &st("stage"))),
        "check_shape" => stop(pb::check_shape(ctx, &st("run"), &strings(&a["paths"]))),
        other => json!({"unknown": other}),
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

fn label_kind(label: &str) -> Option<&'static str> {
    if STRICTER.iter().any(|(l, _)| *l == label) {
        Some("stricter")
    } else if KNOWN.iter().any(|(l, _)| *l == label) {
        Some("known")
    } else {
        None
    }
}

#[test]
fn script_matches_python() {
    let _serial = serial();
    let exp = expected();
    let st = Stage::new(&exp);
    let steps = exp["steps"].as_array().expect("steps");
    let mut bad: Vec<String> = Vec::new();
    let mut differing: BTreeSet<String> = BTreeSet::new();
    let mut labels: BTreeSet<String> = BTreeSet::new();
    let mut counts = [0usize; 3];
    for (i, step) in steps.iter().enumerate() {
        let op = s(&step["op"]);
        let label = step["label"].as_str();
        if let Some(l) = label {
            labels.insert(l.to_string());
            assert!(label_kind(l).is_some(), "第 {i} 步的 label「{l}」不在 STRICTER／KNOWN 表上");
        }
        let mut fail = |why: String| bad.push(format!("第 {i} 步（{op} {}）：{why}", step.get("note").map_or_else(|| step.get("fn").map_or(String::new(), |f| s(f).to_string()), |n| s(n).to_string())));
        match op {
            "write" => {
                let path = st.p(s(&step["path"]));
                std::fs::create_dir_all(path.parent().expect("上層目錄")).expect("mkdir");
                let data = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None => st.abs(s(&dec(&step["text"]))).into_bytes(),
                };
                std::fs::write(&path, data).expect("write");
                if step["exec"].as_bool() == Some(true) {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
                }
            }
            "rm" => {
                let _ = std::fs::remove_file(st.p(s(&step["path"])));
            }
            "git" => {
                counts[0] += 1;
                let (code, out) = st.git(s(&step["cwd"]), &strings(&step["args"]));
                if Some(code as i64) != step["code"].as_i64() || out != s(&step["out"]) {
                    fail(format!("git {:?}\n    要：{} {:?}\n    得：{code} {out:?}", step["args"], step["code"], s(&step["out"])));
                }
            }
            "run" => {
                counts[1] += 1;
                let now = step["now"].as_bool() == Some(true);
                let before = st.snapshot();
                let argv: Vec<String> = strings(&step["argv"]).iter().map(|a| st.abs(a)).collect();
                let (mut out, mut err) = (String::new(), String::new());
                let code = pb::run(&st.env, &argv, &mut out, &mut err);
                let files = st.changed(&before, now);
                let mut out = st.norm(&out);
                if now {
                    out = st.fix_now(&out);
                }
                let want_code = step["code"].as_i64().unwrap_or(-1);
                let files_same = same(&files, &step["files"]);
                let exact = if step["usage"].as_bool() == Some(true) {
                    code as i64 == want_code && files_same
                } else if let Some(class) = step["raises"].as_str() {
                    code == 1 && err.contains(class) && files_same
                } else {
                    code as i64 == want_code && out == s(&step["stdout"]) && files_same && err.is_empty()
                };
                if exact {
                    continue;
                }
                match label {
                    Some(l) => {
                        differing.insert(l.to_string());
                        // 只會更嚴：這裡一定是拒絕，而且沒有推送、沒有開 PR
                        let touched: Vec<&String> = files.as_object().map(|o| o.keys().filter(|k| !k.ends_with("publish-audit.jsonl")).collect()).unwrap_or_default();
                        if code == 0 || out.contains("\"decision\": \"pushed\"") || out.contains("\"decision\": \"published\"") || out.contains("\"decision\": \"dry-run\"") || !touched.is_empty() {
                            fail(format!("label {l}：這裡要拒絕（exit {code}）\n{out}{err}"));
                        }
                    }
                    None => fail(format!("argv {:?}\n    要：exit {want_code} {}\n    {}\n    得：exit {code}\n    {out}{err}\n    檔：要 {}\n        得 {files}", step["argv"], step["raises"], s(&step["stdout"]), step["files"])),
                }
            }
            "call" => {
                counts[2] += 1;
                let args = st.abs_json(&dec(&step["args"]));
                let got = st.norm_json(&call(&st.env.ctx, s(&step["fn"]), &args));
                if result_same(&step["result"], &got) {
                    continue;
                }
                match label {
                    Some(l) => {
                        differing.insert(l.to_string());
                        let refused = got.get("refused").is_some() || got.get("raises").is_some() || got["ok"]["fork_repo"] == "";
                        if step["result"].get("ok").is_none() || !refused {
                            fail(format!("label {l}：Python 要是接受、這裡要是拒絕\n    Python：{}\n    這裡：{got}", step["result"]));
                        }
                    }
                    None => fail(format!("{}\n    要：{}\n    得：{got}", step["args"], step["result"])),
                }
            }
            other => panic!("第 {i} 步：不認得的 op {other}"),
        }
    }
    assert!(bad.is_empty(), "{} 步和 Python 版不同：\n{}", bad.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
    // 兩張表沒有過時：表上的 label 都有一步真的不同
    for (l, _) in STRICTER.iter().chain(KNOWN.iter()) {
        assert!(labels.contains(*l), "表上的 label「{l}」劇本裡沒有");
        assert!(differing.contains(*l), "label「{l}」的步驟現在都和 Python 相同，該從表上拿掉");
    }
    eprintln!("publish 對跑：git {} 步、指令 {} 步、函式 {} 步", counts[0], counts[1], counts[2]);
    assert!(counts[1] >= 100 && counts[2] >= 300, "劇本的步數變少了：{counts:?}");
}

#[test]
fn selftest_matches_python() {
    let _serial = serial();
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("publish-parity-fakes-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建立暫存目錄");
    // 替身：push 只記參數，不會真的推送；gh 只記參數
    let fake_git = dir.join("fake_git.sh");
    std::fs::write(&fake_git, "base=\"$1\"\nshift\nfor a in \"$@\"; do\n  if [ \"$a\" = push ]; then echo \"$*\" >> \"$base.log\"; exit 0; fi\ndone\nexec git \"$@\"\n").expect("write");
    let fake_gh = dir.join("fake_gh.sh");
    std::fs::write(&fake_gh, "base=\"$1\"\nshift\necho \"$*\" >> \"$base.log\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--body-file\" ]; then cp \"$a\" \"$base.body\"; fi\n  prev=\"$a\"\ndone\necho \"https://github.com/acme-fork/app/pull/7\"\n").expect("write");
    let t = teams();
    let ctx = Ctx { profile: format!("{t}/methodology.json"), pack_root: format!("{}/plugin/scaffold", src_root()), cwd: dir.to_string_lossy().into_owned(), teams: t, today: None };
    let env = Env { ctx, project: format!("{}/plugin/scaffold", src_root()), git: vec!["git".into()], gh: vec!["gh".into()], env: Vec::new() };
    let sh = |p: &Path| vec!["sh".to_string(), p.to_string_lossy().into_owned()];
    let mut out = String::new();
    let code = pb::selftest_with(&env, &sh(&fake_git), &sh(&fake_gh), &mut out);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out, s(&exp["selftest"]["stdout"]), "selftest 的輸出和 Python 版不同");
    assert_eq!(Some(code as i64), exp["selftest"]["code"].as_i64());
    assert!(out.lines().filter(|l| l.contains("[通過]")).count() >= 35 && !out.contains("[失敗]"));
}
