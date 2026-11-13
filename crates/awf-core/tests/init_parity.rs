//! init 的回歸基準：照劇本（tests/parity/init.expected.json）做一遍，逐字比對。
//!
//! 劇本原本是 Python 版 `init.py` 錄的；1.0 起 init 的行為不同了（工具不複製進專案、hook 改成執行檔、升級會整理舊工具、
//! merge gate 帶執行檔），基準改由這個實作重錄，當成回歸基準：行為或訊息一改，這裡就會指出是哪個案例的哪一步。
//!
//! 劇本的每個案例有自己的暫存目錄（`@TMP@`）；`steps` 是檔案操作與指令（run）。每個 run 比 exit code、stdout、stderr
//! 與「這一步改了哪些檔」（簽章＝有沒有執行權限＋內容雜湊；和 `base` 相同的只比數量與路徑清單的雜湊）。
//! 模版來源是把 repo 的 `template` 清單複製到暫存目錄（`@SRC@`），plugin.json 的版本固定，另外放一個假的
//! `plugin/bin/awf-linux-x64`（只有 ELF 檔頭）與對得上的 `checksums.json`；`--hooks project` 複製的執行檔也是假的。
//! run 可以帶 `awf_bin`（空字串＝找不到執行檔，其他＝改用這個路徑）。
//!
//! 重錄：`AWF_RECORD=1 cargo test -p awf-core --test init_parity`（舊名 `AWF_INIT_BLESS=1` 也認），
//! 會用這個實作的結果改寫基準檔裡每個 run 的結果、`base` 與 `counts`（`cmp` 標成不比的欄位不動）。
//! 重錄前後的差異要逐項看過：說不出理由的差異就是 bug。新增或改寫案例：直接改劇本的 `steps`（run 的結果欄位留空），再重錄。
#![cfg(unix)]

use awf_core::init;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn expected_path() -> PathBuf {
    repo().join("tests/parity/init.expected.json")
}

fn expected() -> Value {
    let p = expected_path();
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|k| u8::from_str_radix(h.get(k * 2..k * 2 + 2)?, 16).ok()).collect()
}

fn sha16(data: &[u8]) -> String {
    Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(16).collect()
}

fn text_of(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("建目錄");
        let mut names: Vec<_> = std::fs::read_dir(src).expect("讀目錄").flatten().map(|e| e.file_name()).collect();
        names.sort();
        for n in names {
            if n == "__pycache__" || n == ".DS_Store" {
                continue;
            }
            copy_tree(&src.join(&n), &dst.join(&n));
        }
    } else {
        std::fs::create_dir_all(dst.parent().expect("上層目錄")).expect("建目錄");
        std::fs::copy(src, dst).unwrap_or_else(|e| panic!("複製不了 {}：{e}", src.display()));
    }
}

/// 把 repo 的模版複製一份當模版根，plugin.json 的版本固定。
fn copy_template(exp: &Value, dst: &Path) {
    for rel in exp["template"].as_array().expect("template") {
        copy_tree(&repo().join(s(rel)), &dst.join(s(rel)));
    }
    let pj = dst.join("plugin/.claude-plugin/plugin.json");
    std::fs::write(&pj, format!("{{\"name\": \"agent-workflow\", \"version\": \"{}\"}}\n", s(&exp["version"]))).expect("寫 plugin.json");
    // 發佈版才有的 linux-x64 執行檔：放一個只有 ELF 檔頭的假檔與對得上的 checksums.json
    let bin = dst.join("plugin/bin");
    std::fs::create_dir_all(&bin).expect("建目錄");
    let elf = fake_elf();
    std::fs::write(bin.join("awf-linux-x64"), &elf).expect("寫假執行檔");
    let sum: String = Sha256::digest(&elf).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(bin.join("checksums.json"), format!("{{\n  \"files\": {{\n    \"awf-linux-x64\": \"{sum}\"\n  }},\n  \"source\": \"test\"\n}}\n")).expect("寫 checksums.json");
}

/// 假的 linux x86-64 執行檔：ELF 檔頭（64 位元、little-endian、機器類型 62）加一行字。
fn fake_elf() -> Vec<u8> {
    let mut v = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 62, 0];
    v.extend_from_slice("\nfake awf linux-x64\n".as_bytes());
    v
}

/// `--hooks project` 要複製的執行檔（假的）。
fn fake_awf(root: &Path) -> PathBuf {
    let p = root.join("fake-bin").join("awf");
    if !p.is_file() {
        std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("建目錄");
        std::fs::write(&p, "#!/bin/sh\n# 假的 awf 執行檔（init 對跑用）\n").expect("寫假執行檔");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("改權限");
    }
    p
}

struct Stage<'a> {
    exp: &'a Value,
    dir: String,
    real: String,
    src: String,
    src_real: String,
    prev: Map<String, Value>,
}

impl Stage<'_> {
    fn p(&self, rel: &str) -> PathBuf {
        Path::new(&self.dir).join(rel)
    }

    fn abs(&self, text: &str) -> String {
        text.replace("@TMP@", &self.dir).replace("@SRC@", &self.src)
    }

    fn norm(&self, text: &str) -> String {
        text.replace(&self.src_real, "@SRC@").replace(&self.src, "@SRC@").replace(&self.real, "@TMP@").replace(&self.dir, "@TMP@")
    }

    fn walk(&self, base: &Path, dir: &Path, sigs: &mut Map<String, Value>, texts: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let long = self.exp["long"].as_u64().unwrap_or(2000) as usize;
        for e in rd.flatten().map(|e| e.path()) {
            let rel = e.strip_prefix(base).ok().map(text_of).unwrap_or_default();
            if e.is_dir() {
                if rel == "src" {
                    continue;
                }
                sigs.insert(format!("{rel}/"), json!("d"));
                self.walk(base, &e, sigs, texts);
            } else {
                let mut data = std::fs::read(&e).unwrap_or_default();
                if let Ok(t) = std::str::from_utf8(&data) {
                    let t = self.norm(t);
                    if t.chars().count() <= long {
                        texts.insert(rel.clone(), json!(t));
                    }
                    data = t.into_bytes();
                }
                let exec = std::fs::metadata(&e).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false);
                sigs.insert(rel, json!(format!("{}:{}", if exec { "x" } else { "-" }, sha16(&data))));
            }
        }
    }

    /// 整棵暫存目錄（略過最上層的 src）：相對路徑 → 簽章，以及短文字檔的內容。
    fn snapshot(&self) -> (Map<String, Value>, Map<String, Value>) {
        let base = PathBuf::from(&self.dir);
        let (mut sigs, mut texts) = (Map::new(), Map::new());
        self.walk(&base, &base, &mut sigs, &mut texts);
        (sigs, texts)
    }

    /// 這一步改了哪些檔：（逐檔列出的, [和 base 相同的數量, 路徑清單的雜湊]）。
    fn changed(&mut self, base_at: &str) -> (Value, Value) {
        let (after, texts) = self.snapshot();
        let keys: BTreeSet<&String> = self.prev.keys().chain(after.keys()).collect();
        let mut same: Vec<String> = Vec::new();
        let mut files = Map::new();
        let prefix = format!("{base_at}/");
        for k in keys {
            if self.prev.get(k) == after.get(k) {
                continue;
            }
            let as_base = k.strip_prefix(&prefix).map(|rest| format!("proj/{rest}"));
            match after.get(k) {
                Some(sig) if as_base.as_ref().is_some_and(|b| self.exp["base"].get(b) == Some(sig)) => same.push(k.clone()),
                Some(sig) => {
                    let v = match texts.get(k) {
                        Some(t) => json!([sig, t]),
                        None => sig.clone(),
                    };
                    files.insert(k.clone(), v);
                }
                None => {
                    files.insert(k.clone(), Value::Null);
                }
            }
        }
        self.prev = after;
        (Value::Object(files), json!([same.len(), sha16(same.join("\n").as_bytes())]))
    }
}

fn files_diff(want: &Value, got: &Value) -> Option<String> {
    let (wo, go) = (want.as_object()?, got.as_object()?);
    let keys: BTreeSet<&String> = wo.keys().chain(go.keys()).collect();
    let show = |v: Option<&Value>| v.map_or("（沒動）".to_string(), |x| x.to_string().chars().take(1500).collect());
    let bad: Vec<String> = keys.into_iter().filter(|k| wo.get(*k) != go.get(*k))
        .map(|k| format!("{k}\n      要：{}\n      得：{}", show(wo.get(k)), show(go.get(k))))
        .collect();
    if bad.is_empty() { None } else { Some(bad.join("\n    ")) }
}

fn first_diff(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.split('\n').collect(), got.split('\n').collect());
    let i = (0..w.len().max(g.len())).find(|&i| w.get(i) != g.get(i)).unwrap_or(0);
    format!("第 {} 行\n      要：{:?}\n      得：{:?}", i + 1, w.get(i).unwrap_or(&"（沒有這一行）"), g.get(i).unwrap_or(&"（沒有這一行）"))
}

fn last_line(t: &str) -> &str {
    t.trim_end_matches('\n').rsplit('\n').next().unwrap_or_default()
}

/// 跑一個案例。回（每個 run 實際的結果——重錄用, 不合的地方, run 的數量）。
fn run_case(exp: &Value, case: &Value, root: &Path, index: usize, shared_src: &Path, awf: &Path) -> (Vec<Value>, Vec<String>, usize) {
    let dir = root.join(format!("c{index:03}"));
    std::fs::create_dir_all(dir.join("cwd")).expect("建目錄");
    std::fs::create_dir_all(dir.join("h")).expect("建目錄");
    let src = if case["own_src"].as_bool().unwrap_or(false) {
        let own = dir.join("src");
        copy_template(exp, &own);
        own
    } else {
        shared_src.to_path_buf()
    };
    let mut st = Stage {
        exp,
        dir: text_of(&dir),
        real: text_of(&dir.canonicalize().expect("暫存目錄")),
        src_real: text_of(&src.canonicalize().expect("模版")),
        src: text_of(&src),
        prev: Map::new(),
    };
    st.prev = st.snapshot().0;
    let name = s(&case["name"]);
    let (mut actual, mut bad, mut runs) = (Vec::new(), Vec::new(), 0usize);
    for (i, step) in case["steps"].as_array().expect("steps").iter().enumerate() {
        let path = st.p(s(&step["path"]));
        match s(&step["op"]) {
            "write" => {
                let data = match step.get("hex") {
                    Some(h) => unhex(s(h)),
                    None if step["abs"].as_bool().unwrap_or(false) => st.abs(s(&step["text"])).into_bytes(),
                    None => s(&step["text"]).as_bytes().to_vec(),
                };
                std::fs::create_dir_all(path.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&path, data).unwrap_or_else(|e| panic!("寫不進 {}：{e}", path.display()));
            }
            "mkdir" => std::fs::create_dir_all(&path).expect("建目錄"),
            "rm" => std::fs::remove_file(&path).unwrap_or_else(|e| panic!("{name}：刪不掉 {}：{e}", path.display())),
            "rmtree" => std::fs::remove_dir_all(&path).unwrap_or_else(|e| panic!("{name}：刪不掉 {}：{e}", path.display())),
            "chmod" => {
                let mode = step["mode"].as_u64().expect("mode") as u32;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("改權限");
            }
            "sub" => {
                let keep = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}：讀不到 {}：{e}", path.display()));
                assert!(keep.contains(s(&step["old"])), "{name}：{} 裡沒有要換的字串 {:?}", path.display(), s(&step["old"]));
                std::fs::write(&path, keep.replacen(s(&step["old"]), s(&step["new"]), 1)).expect("寫檔");
            }
            "append" => {
                let mut data = std::fs::read(&path).unwrap_or_default();
                data.extend_from_slice(s(&step["text"]).as_bytes());
                std::fs::write(&path, data).expect("寫檔");
            }
            "run" => {
                runs += 1;
                let argv: Vec<String> = step["argv"].as_array().expect("argv").iter().map(|a| st.abs(s(a))).collect();
                let cwd = st.p(s(&step["cwd"])).canonicalize().expect("cwd");
                let awf_bin = match step.get("awf_bin").map(s) {
                    None => Some(awf.to_path_buf()),
                    Some("") => None,
                    Some(other) => Some(PathBuf::from(st.abs(other))),
                };
                let env = init::Env { source: Some(PathBuf::from(&st.src)), cwd, home: Some(text_of(&st.p("h"))), awf_bin };
                let tty = step.get("tty").is_some();
                let fed = if tty { s(&step["tty"]) } else { step.get("stdin").map(s).unwrap_or_default() };
                let mut input = std::io::Cursor::new(fed.as_bytes().to_vec());
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let code = init::run(&argv, &env, &mut out, &mut err, &mut input, tty);
                let out = st.norm(&String::from_utf8_lossy(&out));
                let err = st.norm(&String::from_utf8_lossy(&err));
                let base_at = step.get("base_at").map(s).unwrap_or("proj");
                let (files, as_base) = st.changed(base_at);
                let cmp = |field: &str| step.get("cmp").and_then(|c| c.get(field)).map(s).unwrap_or("exact");
                let mut why: Vec<String> = Vec::new();
                if step["code"].as_i64() != Some(i64::from(code)) {
                    why.push(format!("exit code 要 {} 得 {code}\n    stderr：{err}", step["code"]));
                }
                if cmp("stdout") == "exact" && s(&step["stdout"]) != out {
                    why.push(format!("stdout 不同：{}", first_diff(s(&step["stdout"]), &out)));
                }
                let err_same = match cmp("stderr") {
                    "skip" => true,
                    "last" => last_line(s(&step["stderr"])) == last_line(&err),
                    _ => s(&step["stderr"]) == err,
                };
                if !err_same {
                    why.push(format!("stderr 不同：{}", first_diff(s(&step["stderr"]), &err)));
                }
                if let Some(d) = files_diff(&step["files"], &files) {
                    why.push(format!("改到的檔不同：\n    {d}"));
                }
                if step["as_base"] != as_base {
                    why.push(format!("和 base 相同的檔：要 {} 得 {as_base}", step["as_base"]));
                }
                if !why.is_empty() {
                    bad.push(format!("【{name}】第 {} 步 argv={}\n  {}", i + 1, step["argv"], why.join("\n  ")));
                }
                let mut got = step.clone();
                if let Some(o) = got.as_object_mut() {
                    o.insert("code".into(), json!(code));
                    if cmp("stdout") == "exact" {
                        o.insert("stdout".into(), json!(out));
                    }
                    if cmp("stderr") != "skip" {
                        o.insert("stderr".into(), json!(err));
                    }
                    o.insert("files".into(), files);
                    o.insert("as_base".into(), as_base);
                }
                actual.push(got);
                continue;
            }
            other => panic!("劇本有不認得的操作：{other}"),
        }
        actual.push(step.clone());
    }
    if index == 0 {
        // 第一個案例訂出 base：整棵目錄逐檔比
        if let Some(d) = files_diff(&exp["base"], &Value::Object(st.prev.clone())) {
            bad.push(format!("【{name}】整棵目錄和 base 不同：\n    {d}"));
        }
        let sorted: std::collections::BTreeMap<String, Value> = st.prev.clone().into_iter().collect();
        actual.push(Value::Object(sorted.into_iter().collect()));
    }
    (actual, bad, runs)
}

#[test]
fn steps_match_baseline() {
    let mut exp = expected();
    let bless = ["AWF_RECORD", "AWF_INIT_BLESS"].iter().any(|k| std::env::var(k).is_ok_and(|v| v == "1"));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("init-parity-{}-{nanos}", std::process::id()));
    let shared = root.join("src");
    copy_template(&exp, &shared);
    let awf = fake_awf(&root);
    let awf = awf.as_path();
    let cases: Vec<Value> = exp["cases"].as_array().expect("cases").clone();

    // 重錄時先跑第一個案例訂出 base，其餘案例才有東西可比
    if bless {
        let (mut first, _, _) = run_case(&exp, &cases[0], &root, 0, &shared, awf);
        exp["base"] = first.pop().expect("base");
        std::fs::remove_dir_all(root.join("c000")).expect("清掉 c000");
    }

    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let exp_ref = &exp;
    let mut results: Vec<(usize, Vec<Value>, Vec<String>, usize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                let (cases, root, shared) = (&cases, &root, &shared);
                scope.spawn(move || {
                    cases.iter().enumerate().filter(|(i, _)| i % workers == w)
                        .map(|(i, c)| {
                            let (actual, bad, runs) = run_case(exp_ref, c, root, i, shared, awf);
                            (i, actual, bad, runs)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("案例執行緒")).collect()
    });
    results.sort_by_key(|r| r.0);
    let _ = std::fs::remove_dir_all(&root);

    let runs: usize = results.iter().map(|r| r.3).sum();
    let bad: Vec<String> = results.iter().flat_map(|r| r.2.iter().cloned()).collect();
    if bless {
        for (i, actual, _, _) in &mut results {
            if *i == 0 {
                actual.pop();
            }
            exp["cases"][*i]["steps"] = Value::Array(std::mem::take(actual));
        }
        exp["counts"] = json!({"cases": cases.len(), "runs": runs});
        exp["_說明"] = json!("init 的回歸基準（劇本）。案例的 steps 可以手改（run 的結果欄位留空），結果一律用 AWF_RECORD=1 cargo test -p awf-core --test init_parity 重錄，不要手改。");
        std::fs::write(expected_path(), format!("{}\n", awf_core::recipe::dumps(&exp, Some(1)))).expect("寫基準檔");
        eprintln!("init 對跑：已重錄 {} 個案例、{runs} 次執行（重錄前有 {} 處不同）", cases.len(), bad.len());
        return;
    }
    assert_eq!(exp["counts"]["cases"].as_u64(), Some(cases.len() as u64), "案例數和基準檔的 counts 不符");
    assert_eq!(exp["counts"]["runs"].as_u64(), Some(runs as u64), "執行次數和基準檔的 counts 不符");
    eprintln!("init 對跑：{} 個案例、{runs} 次執行，不同 {} 處", cases.len(), bad.len());
    assert!(bad.is_empty(), "和基準不同（{} 處），前幾處：\n{}", bad.len(), bad.iter().take(6).map(|x| x.as_str()).collect::<Vec<_>>().join("\n\n"));
}

/// `--source` 與找不到模版時的訊息（劇本的模版來源固定由環境給，測不到這兩件事）。
#[test]
fn source_option_and_missing_template() {
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("init-source-{}-{nanos}", std::process::id()));
    let src = root.join("tpl");
    copy_template(&exp, &src);
    std::fs::create_dir_all(root.join("cwd")).expect("建目錄");
    let cwd = root.join("cwd").canonicalize().expect("cwd");
    let run = |args: &[&str], source: Option<PathBuf>| {
        let env = init::Env { source, cwd: cwd.clone(), home: None, awf_bin: None };
        let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let (mut out, mut err, mut input) = (Vec::new(), Vec::new(), std::io::Cursor::new(Vec::new()));
        let code = init::run(&argv, &env, &mut out, &mut err, &mut input, false);
        (code, String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned())
    };
    // 沒有模版來源
    let (code, out, err) = run(&["--target", "p0", "--defaults"], None);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.starts_with("awf init：找不到模版"), "{err}");
    assert!(!root.join("cwd/p0").exists(), "找不到模版時不應該建立目標目錄");
    // --source（相對路徑）蓋過環境給的來源
    let (code, out, err) = run(&["--target", "p1", "--defaults", "--source", "../tpl"], Some(root.join("nope")));
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("/plugin marketplace add {}", src.canonicalize().expect("模版").display())), "{out}");
    assert_eq!(std::fs::read_to_string(root.join("cwd/p1/.claude/teams/VERSION")).ok().as_deref(), Some("9.9.9\n"));
    // 環境給的來源不存在
    let (code, _, err) = run(&["--target", "p2", "--defaults"], Some(root.join("nope")));
    assert_eq!(code, 1);
    assert!(err.starts_with("awf init：找不到 ") && err.trim_end().ends_with("nope/scaffold"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}
