//! merge gate 與 sync-branch 的對跑：和 0.x 的 Python 版錄下來的基準（tests/parity/gates.expected.json）逐字比對。
//!
//! 基準的格式與正規化規則見 `tests/parity/README.md`。每個案例在自己的暫存目錄重建輸入檔，
//! 用同樣的參數跑 Rust 版，比 exit code、stdout、stderr（參數錯誤只比最後一行）與寫出來的檔（稽核紀錄）。
//!
//! 和 0.x 的 Python 版不一樣的地方只准更嚴，全部列在 `STRICTER`（附理由）；0.x 的 Python 版丟出沒接住的例外的案例
//! （基準裡記 crash），這裡只要求「不放行」。

#![cfg(unix)]

use awf_core::merge_gate::{self as mg, pyjson, pyutil};
use awf_core::{recipe, sync_branch as sb};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// 和 0.x 的 Python 版判定不同、但只會更嚴的案例（案例名 → 理由）。
const STRICTER: &[(&str, &str)] = &[
    ("merged-today '２'", "全形數字：Python 的 int() 收，這裡只收 ASCII 數字 → hold"),
    ("diff：hunk 標頭用全形數字（Python 收、另一個實作只會更嚴）", "hunk 標頭只收 ASCII 數字 → diff 解析不了、hold"),
];

/// 只有說明文字不同的案例：exit code 要相同，文字不比（argparse 自動產生的英文說明沒有照抄）。
const HELP_TEXT: &[&str] = &["參數：-h"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn teams() -> PathBuf {
    root().join("plugin/scaffold/.claude/teams")
}

fn expected() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| {
        let p = root().join("tests/parity/gates.expected.json");
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
    })
}

fn text(v: &Value) -> String {
    recipe::dumps(v, None)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("基準裡這個值應該是字串：{v}"))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default()
}

/// 會改目前目錄的案例與其他案例不能同時跑。
fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ 暫存目錄與正規化

struct Tmp {
    raw: String,
    real: String,
}

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("awf-gates-parity-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建暫存目錄");
        let real = std::fs::canonicalize(&dir).expect("暫存目錄").to_string_lossy().into_owned();
        Tmp { raw: dir.to_string_lossy().trim_end_matches('/').to_string(), real }
    }

    fn path(&self, rel: &str) -> PathBuf {
        Path::new(&self.raw).join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).expect("建目錄");
        }
        std::fs::write(&p, bytes).unwrap_or_else(|e| panic!("寫 {}：{e}", p.display()));
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.raw);
    }
}

/// 系統的 UTC 日期：今天、昨天、明天（和錄製腳本的 DAYS 同順序）。
fn days() -> Vec<(String, &'static str)> {
    let today = pyutil::today_utc();
    [(0, "@TODAY@"), (-1, "@YESTERDAY@"), (1, "@TOMORROW@")].into_iter()
        .filter_map(|(n, tag)| pyutil::add_days(today, n).map(|d| (pyutil::iso(d), tag))).collect()
}

/// 需求清單指令（對應 0.x 的 Python 版 selftest 臨時寫的 gen_reqs.py）。
fn gen_cmd() -> Value {
    json!(["sh", "-c", "printf '{\"requirements\": [{\"id\": \"ISSUE-7\", \"text\": \"登入頁顯示錯誤訊息\", \"source\": \"issue #7 of %s\"}]}\\n' \"$AWF_REPO\""])
}

/// 基準裡的文字 → 實際的輸入（把佔位符換回去）。
fn fill(t: &str, tmp: &Tmp) -> String {
    let mut out = t.replace("\"@GEN@\"", &text(&gen_cmd())).replace("@TMP@", &tmp.raw);
    // 輸入檔裡「今天的某個時間」（selftest 預先寫的稽核紀錄）：時分秒不影響判定
    out = out.replace("@NOW@", &format!("{}T00:00:01+00:00", pyutil::iso(pyutil::today_utc())));
    for (day, tag) in days() {
        out = out.replace(tag, &day);
    }
    out
}

/// 實際的輸出 → 基準的寫法。
fn norm(t: &str, tmp: &Tmp) -> String {
    static NOW: OnceLock<Regex> = OnceLock::new();
    let today = pyutil::iso(pyutil::today_utc());
    let now = NOW.get_or_init(|| Regex::new(r"T\d\d:\d\d:\d\d\+00:00").expect("固定的正規式"));
    let mut out = t.replace(&text(&gen_cmd()), "\"@GEN@\"").replace(&tmp.real, "@TMP@").replace(&tmp.raw, "@TMP@");
    // 「今天的日期＋時間」才是現在時間（--timestamp 給的固定時間不換）
    let mut done = String::new();
    let mut rest = out.as_str();
    while let Some(i) = rest.find(&today) {
        let after = &rest[i + today.len()..];
        done.push_str(&rest[..i]);
        match now.find(after).filter(|m| m.start() == 0) {
            Some(m) => {
                done.push_str("@NOW@");
                rest = &after[m.end()..];
            }
            None => {
                done.push_str(&today);
                rest = after;
            }
        }
    }
    done.push_str(rest);
    out = done;
    for (day, tag) in days() {
        out = out.replace(&day, tag);
    }
    out
}

fn git_env(gitconfig: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = expected()["meta"]["git_env"].as_object().expect("git_env").iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
    env.push(("GIT_CONFIG_GLOBAL".into(), gitconfig.to_string()));
    env
}

fn git(dir: &str, args: &[String], env: &[(String, String)]) {
    let _ = Command::new("git").arg("-C").arg(dir).args(args).envs(env.iter().cloned()).output();
}

fn same_git() -> bool {
    let have = Command::new("git").arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    have == s(&expected()["meta"]["git_version"])
}

fn blob(key: &str) -> Vec<u8> {
    match &expected()["blobs"][key] {
        Value::String(t) => t.clone().into_bytes(),
        v => {
            let hex = s(&v["hex"]);
            (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect()
        }
    }
}

fn blob_filled(key: &str, tmp: &Tmp) -> Vec<u8> {
    match &expected()["blobs"][key] {
        Value::String(t) => fill(t, tmp).into_bytes(),
        _ => blob(key),
    }
}

/// 專案目錄的檔案（不含暫存 git repo）：相對路徑 → 內容。
fn snapshot(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
        if rel == "gitrepo" || rel == ".gitconfig-parity" {
            continue;
        }
        if p.is_dir() {
            snapshot(&p, base, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            out.insert(rel, bytes);
        }
    }
}

fn short(t: &str) -> String {
    let cut: String = t.chars().take(700).collect();
    if cut.len() < t.len() { format!("{cut}…") } else { cut }
}

/// 兩段文字第一個不同的地方附近。
fn first_diff(want: &str, got: &str) -> String {
    let (w, g): (Vec<char>, Vec<char>) = (want.chars().collect(), got.chars().collect());
    let i = w.iter().zip(&g).position(|(a, b)| a != b).unwrap_or(w.len().min(g.len()));
    let from = i.saturating_sub(80);
    let part = |v: &[char]| v.iter().skip(from).take(240).collect::<String>();
    format!("第 {i} 個字起不同\n      基準：{}\n      實際：{}", part(&w), part(&g))
}

// ------------------------------------------------------------------ merge gate：案例

struct Got {
    code: i32,
    stdout: String,
    stderr: String,
    after: BTreeMap<String, String>,
}

fn run_mg_case(c: &Value) -> Got {
    let tmp = Tmp::new();
    let mut before: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (rel, key) in c["files"].as_object().expect("files") {
        let bytes = blob_filled(s(key), &tmp);
        tmp.write(rel, &bytes);
        before.insert(rel.clone(), bytes);
    }
    tmp.write(".gitconfig-parity", b"");
    let env = git_env(&format!("{}/.gitconfig-parity", tmp.raw));
    if c["git"] == json!(true) {
        let repo = format!("{}/gitrepo", tmp.raw);
        std::fs::create_dir_all(&repo).expect("gitrepo");
        for step in expected()["mg"]["git_setup"].as_array().expect("git_setup") {
            match step.get("git") {
                Some(args) => git(&repo, &strings(args), &env),
                None => tmp.write(s(&step["write"]), s(&step["text"]).as_bytes()),
            }
        }
    }
    let argv: Vec<String> = strings(&c["argv"]).iter().map(|a| fill(a, &tmp)).collect();
    let ctx = mg::Ctx {
        teams: teams(),
        project_dir_env: c.get("project_dir_env").map(|v| fill(s(v), &tmp)),
        env,
        stdin: Some(c.get("stdin").map(|v| s(v).to_string()).unwrap_or_default()),
    };
    let (mut out, mut errs) = (String::new(), String::new());
    let _guard = cwd_lock();
    let home = std::env::current_dir().expect("cwd");
    if let Some(cwd) = c.get("cwd") {
        std::env::set_current_dir(fill(s(cwd), &tmp)).expect("chdir");
    }
    let code = mg::run(&ctx, &argv, &mut out, &mut errs);
    std::env::set_current_dir(&home).expect("chdir 回去");
    drop(_guard);
    let mut now = BTreeMap::new();
    snapshot(Path::new(&tmp.raw), Path::new(&tmp.raw), &mut now);
    let after = now.into_iter().filter(|(k, v)| before.get(k) != Some(v)).map(|(k, v)| (k, norm(&String::from_utf8_lossy(&v), &tmp))).collect();
    Got { code, stdout: norm(&out, &tmp), stderr: norm(&errs, &tmp), after }
}

/// 回傳和基準不同的地方（空＝逐字相同）。
fn mg_diffs(c: &Value, got: &Got) -> Vec<String> {
    let mut d = Vec::new();
    if json!(got.code) != c["code"] {
        d.push(format!("exit code：基準 {}、實際 {}", c["code"], got.code));
    }
    if got.stdout != s(&c["stdout"]) {
        d.push(format!("stdout {}", first_diff(s(&c["stdout"]), &got.stdout)));
    }
    if let Some(last) = c.get("stderr_last") {
        let mine = got.stderr.trim().lines().last().unwrap_or("").replace("awf merge-gate", "@PROG@");
        if mine != s(last) {
            d.push(format!("stderr 最後一行：基準 {last}、實際 {mine:?}"));
        }
    } else if got.stderr != s(&c["stderr"]) {
        d.push(format!("stderr {}", first_diff(s(&c["stderr"]), &got.stderr)));
    }
    let want: BTreeMap<String, String> = c.get("after").and_then(Value::as_object).map(|o| {
        o.iter().map(|(k, key)| (k.clone(), String::from_utf8_lossy(&blob(s(key))).into_owned())).collect()
    }).unwrap_or_default();
    if want != got.after {
        let w = want.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
        let g = got.after.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
        d.push(format!("寫出來的檔 {}", first_diff(&w, &g)));
    }
    d
}

/// 有案例會切換整個行程的目前目錄，所以這個檔的測試一次只跑一個。
/// 另外拿掉外面帶進來的 git 身分與日期：工具自己跑的 rebase 會沿用它們，commit SHA 就和基準不同。
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for k in ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_AUTHOR_DATE", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL", "GIT_COMMITTER_DATE"] {
        std::env::remove_var(k);
    }
    guard
}

#[test]
fn merge_gate_cases_match_python() {
    let _serial = serial();
    let cases = expected()["mg"]["cases"].as_array().expect("cases");
    let (mut same, mut crash, mut stricter, mut help) = (0, 0, 0, 0);
    let mut bad = Vec::new();
    for c in cases {
        let name = s(&c["name"]);
        let got = run_mg_case(c);
        if c.get("crash").is_some() {
            // 0.x 的 Python 版在這裡丟例外（traceback、exit 1、沒有判定）：只要求不放行、也不輸出 merge
            crash += 1;
            if got.code == 0 || got.stdout.contains("\"decision\": \"merge\"") {
                bad.push(format!("{name}：0.x 的 Python 版丟例外的案例，這裡卻放行（exit {}）", got.code));
            }
            continue;
        }
        let d = mg_diffs(c, &got);
        if d.is_empty() {
            same += 1;
        } else if HELP_TEXT.contains(&name) {
            help += 1;
            if json!(got.code) != c["code"] {
                bad.push(format!("{name}：exit code 基準 {}、實際 {}", c["code"], got.code));
            }
        } else if STRICTER.iter().any(|(n, _)| *n == name) {
            stricter += 1;
            if got.code == 0 {
                bad.push(format!("{name}：列為「只會更嚴」的案例卻放行"));
            }
        } else {
            bad.push(format!("{name}（{}）\n    {}", s(&c["group"]), d.iter().map(|x| short(x)).collect::<Vec<_>>().join("\n    ")));
        }
    }
    println!("merge gate 案例 {}：逐字相同 {same}、Python 丟例外（這裡不放行）{crash}、只會更嚴 {stricter}、只有說明文字不同 {help}、不符 {}", cases.len(), bad.len());
    assert!(bad.is_empty(), "{} 個案例和基準不同：\n{}", bad.len(), bad.join("\n"));
    assert_eq!(expected()["meta"]["selftest_cases"].as_u64().unwrap() + expected()["meta"]["cli_cases"].as_u64().unwrap(), cases.len() as u64);
}

/// 列為「只會更嚴」的案例必須真的存在於基準（改名後不會默默失效）。
#[test]
fn stricter_list_is_live() {
    let _serial = serial();
    let names: Vec<&str> = expected()["mg"]["cases"].as_array().expect("cases").iter().map(|c| s(&c["name"])).collect();
    for (n, why) in STRICTER {
        assert!(names.contains(n), "STRICTER 的案例不在基準裡：{n}（{why}）");
    }
    for n in HELP_TEXT {
        assert!(names.contains(n), "HELP_TEXT 的案例不在基準裡：{n}");
    }
}

// ------------------------------------------------------------------ merge gate：純函式

fn probe(fn_name: &str, a: &[Value]) -> Result<Value, String> {
    Ok(match fn_name {
        "parse_diff" => mg::parse_diff(s(&a[0]))?.to_value(),
        "path_kind" => {
            let (what, doc, test) = mg::path_kind(s(&a[0]));
            json!([what, doc, test])
        }
        "line_kind" => {
            let (skip, suppress) = mg::line_kind(s(&a[0]));
            json!([skip, suppress])
        }
        "risk_tier" => {
            let files = a[0].as_object().expect("files").iter().map(|(p, c)| (p.clone(), [c[0].as_u64().unwrap_or(0), c[1].as_u64().unwrap_or(0)])).collect();
            let (tier, reasons) = mg::risk_tier(&mg::Diff::from_files(files), &a[1])?;
            json!([tier, reasons])
        }
        "upstream_auth" => {
            let (by, reason) = mg::upstream_auth(&a[0], s(&a[1]), s(&a[2]));
            json!([by, reason])
        }
        "iso_date" => json!(pyutil::iso(pyutil::parse_iso_date(pyutil::head(s(&a[0]), 10)).ok_or("ValueError")?)),
        "json" => match pyjson::loads(s(&a[0])) {
            Ok(v) => json!({"ok": text(&v)}),
            Err(e) => json!({"err": e.to_string()}),
        },
        "decide" => match sb::decide_text(s(&a[0]), a[1].as_u64().unwrap_or(0), a[2] == json!(true), &strings(&a[3])) {
            Ok((strategy, reason)) => json!([strategy, reason]),
            Err(m) => json!({"halt": m}),
        },
        other => panic!("基準裡有不認得的函式：{other}"),
    })
}

#[test]
fn function_probes_match_python() {
    let _serial = serial();
    let probes = expected()["mg"]["probes"].as_array().expect("probes");
    let mut bad = Vec::new();
    for p in probes {
        let (name, args) = (s(&p["fn"]), p["args"].as_array().expect("args"));
        let got = probe(name, args);
        let ok = match (p.get("out"), p.get("raise"), &got) {
            (Some(want), _, Ok(v)) => text(want) == text(v),
            (None, Some(_), Err(_)) => true,
            _ => false,
        };
        if !ok {
            bad.push(format!("{name}{}\n    基準：{}\n    實際：{}", short(&text(&p["args"])), short(&p.get("out").map(text).unwrap_or_else(|| format!("丟 {}", p["raise"]))),
                short(&match &got { Ok(v) => text(v), Err(e) => format!("錯誤：{e}") })));
        }
    }
    println!("純函式 {}：不符 {}", probes.len(), bad.len());
    assert!(bad.is_empty(), "{} 個和基準不同：\n{}", bad.len(), bad.join("\n"));
}

// ------------------------------------------------------------------ selftest

fn sha_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b[0-9a-f]{12}\b").expect("固定的正規式"))
}

#[test]
fn merge_gate_selftest_matches_python() {
    let _serial = serial();
    let tmp = Tmp::new();
    tmp.write("gitconfig", b"");
    let ctx = mg::Ctx { teams: teams(), project_dir_env: None, env: git_env(&format!("{}/gitconfig", tmp.raw)), stdin: Some(String::new()) };
    let mut out = String::new();
    let code = mg::selftest(&ctx, &gen_cmd(), &mut out);
    let want = &expected()["mg"]["selftest"];
    let got = sha_re().replace_all(&out, "@SHA@").into_owned();
    assert_eq!(json!(code), want["code"], "{out}");
    assert!(got == s(&want["stdout"]), "selftest 的輸出和 0.x 的 Python 版不同：{}", first_diff(s(&want["stdout"]), &got));
}

#[test]
fn sync_branch_selftest_matches_python() {
    let _serial = serial();
    let ctx = sb::Ctx { env: git_env("/dev/null") };
    let mut out = String::new();
    let code = sb::selftest(&ctx, &mut out);
    let want = &expected()["sb"]["selftest"];
    assert_eq!(json!(code), want["code"], "{out}");
    assert!(out == s(&want["stdout"]), "selftest 的輸出和 0.x 的 Python 版不同：{}", first_diff(s(&want["stdout"]), &out));
}

// ------------------------------------------------------------------ sync-branch：照錄下來的步驟重播

#[test]
fn sync_branch_steps_match_python() {
    let _serial = serial();
    let tmp = Tmp::new();
    tmp.write("gitconfig", b"");
    let env = git_env(&format!("{}/gitconfig", tmp.raw));
    let ctx = sb::Ctx { env: env.clone() };
    let exact_git = same_git();
    let steps = expected()["sb"]["steps"].as_array().expect("steps");
    let (mut cases, mut same, mut crash, mut loose) = (0, 0, 0, 0);
    let mut bad = Vec::new();
    for step in steps {
        if let Some(cmd) = step.get("run") {
            let cmd: Vec<String> = strings(cmd).iter().map(|a| fill(a, &tmp)).collect();
            let _ = Command::new(&cmd[0]).args(&cmd[1..]).envs(env.iter().cloned()).output();
        } else if let Some(rel) = step.get("write") {
            tmp.write(s(rel), fill(s(&step["text"]), &tmp).as_bytes());
        } else if let Some(rel) = step.get("write_hex") {
            let hex = s(&step["hex"]);
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            tmp.write(s(rel), &bytes);
        } else if let Some(rel) = step.get("remove") {
            let _ = std::fs::remove_file(tmp.path(s(rel)));
        } else if let Some(argv) = step.get("case") {
            cases += 1;
            let argv: Vec<String> = strings(argv).iter().map(|a| fill(a, &tmp)).collect();
            let label = text(&step["case"]);
            let Some((code, rec)) = sb::execute(&ctx, &argv) else {
                bad.push(format!("{label}：這裡當成 --help"));
                continue;
            };
            if step.get("crash").is_some() {
                // 0.x 的 Python 版丟例外：這裡只要求停下
                crash += 1;
                if code == 0 {
                    bad.push(format!("{label}：0.x 的 Python 版丟例外的案例，這裡卻 exit 0"));
                }
                continue;
            }
            let got = norm(&recipe::dumps(&rec, Some(2)), &tmp);
            let want = s(&step["stdout"]);
            if json!(code) == step["code"] && got == want {
                same += 1;
            } else if !exact_git && step.get("git_text").is_some() && json!(code) == step["code"] {
                // reason 裡有 git 自己的訊息，git 版本和錄製時不同就只比其他欄位
                let strip = |t: &str| serde_json::from_str::<Value>(t).ok().map(|mut v| {
                    if let Some(o) = v.as_object_mut() {
                        o.shift_remove("reason");
                    }
                    text(&v)
                });
                if strip(&got) == strip(want) { loose += 1 } else { bad.push(format!("{label}\n    {}", first_diff(want, &got))) }
            } else {
                bad.push(format!("{label}\n    exit code：基準 {}、實際 {code}\n    {}", step["code"], first_diff(want, &got)));
            }
        }
    }
    println!("sync-branch 案例 {cases}：逐字相同 {same}、Python 丟例外（這裡停下）{crash}、git 版本不同只比欄位 {loose}、不符 {}", bad.len());
    assert!(bad.is_empty(), "{} 個案例和基準不同：\n{}", bad.len(), bad.join("\n"));
    assert_eq!(expected()["meta"]["sb_cases"].as_u64().unwrap(), cases as u64);
}
