//! awf_cost 與 awf_script 的對跑：照 0.x 的 Python 版錄下來的劇本（tests/parity/tools.expected.json）做一遍，逐字比對。
//! check_agents 的劇本在同一份基準的 agents 段，由 `src/check_agents.rs` 檔內的測試讀。
//!
//! cost 的劇本會改目前目錄與 HOME（相對路徑、--project、~ 都靠它們），整份放在同一個測試裡；
//! script 的測試只呼叫純函式，不受影響。劇本用到符號連結，所以只在 unix 上跑。
#![cfg(unix)]

use awf_core::{cost, script};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo 根目錄")
}

/// 基準檔；`AWF_TOOLS_BASELINE` 可以指到另一份同格式的檔（臨時做大量隨機比對時用）。
fn baseline() -> Value {
    let path = std::env::var_os("AWF_TOOLS_BASELINE").map(PathBuf::from).unwrap_or_else(|| root().join("tests/parity/tools.expected.json"));
    let text = std::fs::read_to_string(path).expect("讀不到基準");
    serde_json::from_str(&text).expect("JSON")
}

fn sha1_hex(text: &str) -> String {
    Sha1::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2).filter_map(|k| hex.get(k * 2..k * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

/// 收集不同的地方；最後一次報出來。
struct Diff {
    bad: Vec<String>,
    compared: usize,
}

impl Diff {
    fn new() -> Diff {
        Diff { bad: Vec::new(), compared: 0 }
    }

    fn text(&mut self, label: &str, want: &str, got: &str) {
        self.compared += 1;
        if want != got {
            // 長的輸出只印第一個不同的地方前後
            let same = want.chars().zip(got.chars()).take_while(|(a, b)| a == b).count();
            let cut = |x: &str| x.chars().skip(same.saturating_sub(60)).take(400).collect::<String>();
            self.bad.push(format!("{label}（第 {same} 個字開始不同）\n  預期：{:?}\n  實際：{:?}", cut(want), cut(got)));
        }
    }

    fn value(&mut self, label: &str, want: &Value, got: &Value) {
        self.text(label, &want.to_string(), &got.to_string());
    }

    fn finish(self, what: &str, steps: usize) {
        assert!(self.compared > 0, "{what}：沒有比到任何東西");
        assert!(self.bad.is_empty(), "{what}：{} 處不同（{steps} 步、比了 {} 項）：\n{}", self.bad.len(), self.compared, self.bad.join("\n"));
    }
}

fn iso(t: Option<cost::Ts>) -> Value {
    t.map(|t| json!(t.isoformat())).unwrap_or(Value::Null)
}

/// 函式探測的結果：`want`，或 0.x 的 Python 版當掉（`crash`）。
fn probe(d: &mut Diff, label: &str, st: &Value, got: Result<Value, cost::Crash>) {
    match (st.get("crash"), got) {
        (Some(_), Err(_)) => d.compared += 1,
        (Some(c), Ok(v)) => d.bad.push(format!("{label}\n  0.x 的 Python 版在這裡當掉（{c}），這裡卻回了 {v}")),
        (None, Err(e)) => d.bad.push(format!("{label}\n  預期：{}\n  實際：當掉（{e}）", st["want"])),
        (None, Ok(v)) => d.value(label, &st["want"], &v),
    }
}

#[test]
fn cost_matches_python() {
    let doc = baseline();
    let base = std::env::temp_dir().join(format!("awf-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("home")).expect("建暫存目錄");
    std::fs::create_dir_all(base.join("cwd")).expect("建暫存目錄");
    let tmp = base.canonicalize().expect("canonicalize").to_string_lossy().into_owned();
    let tmpm: String = tmp.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let projdir = format!("{tmp}/home/.claude/projects/{tmpm}-myproj");
    let old_home = std::env::var_os("HOME");
    let old_cwd = std::env::current_dir().expect("cwd");
    std::env::set_var("HOME", format!("{tmp}/home"));
    std::env::set_current_dir(format!("{tmp}/cwd")).expect("chdir");

    let real = |x: &str| x.replace("@PROJDIR@", &projdir).replace("@TMP@", &tmp);
    let norm = |x: &str| x.replace(&tmp, "@TMP@").replace(&tmpm, "@TMPM@");
    let mut d = Diff::new();
    let mut tails: HashMap<String, cost::TailState> = HashMap::new();
    let steps = doc["cost"]["steps"].as_array().expect("steps");
    for (i, st) in steps.iter().enumerate() {
        let op = s(&st["op"]);
        let label = format!("cost 步驟 {i} {op} {}", st.get("argv").or(st.get("path")).or(st.get("value")).or(st.get("paths")).or(st.get("root")).or(st.get("n")).unwrap_or(&Value::Null));
        match op {
            "write" | "write_big" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                let mut data = match st.get("hex").and_then(Value::as_str) {
                    Some(hex) => unhex(hex),
                    None => s(&st["text"]).as_bytes().to_vec(),
                };
                if op == "write_big" {
                    let mut big = s(&st["fill"]).repeat(st["count"].as_u64().unwrap_or(0) as usize).into_bytes();
                    big.append(&mut data);
                    data = big;
                }
                if st.get("append").is_some() {
                    let mut old = std::fs::read(&p).unwrap_or_default();
                    old.append(&mut data);
                    data = old;
                }
                std::fs::write(&p, data).expect("write");
            }
            "mkdir" => std::fs::create_dir_all(real(s(&st["path"]))).expect("mkdir"),
            "symlink" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                std::os::unix::fs::symlink(real(s(&st["target"])), &p).expect("symlink");
            }
            "run" => {
                let argv: Vec<String> = st["argv"].as_array().map(|a| a.iter().map(|x| real(s(x))).collect()).unwrap_or_default();
                let (rc, out, err) = cost::run_captured(&argv);
                d.text(&format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
                if st.get("usage").is_some() {
                    continue;
                }
                d.text(&format!("{label} stdout"), s(&st["out"]), &norm(&out));
                if st.get("err_any").is_none() {
                    d.text(&format!("{label} stderr"), s(&st["err"]), &norm(&err));
                } else if err.trim().is_empty() {
                    d.bad.push(format!("{label}：出錯時 stderr 要有說明"));
                }
            }
            "selftest" => {
                let (rc, out) = cost::selftest_captured();
                d.text(&format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
                d.text(&format!("{label} 輸出"), s(&st["out"]), &out);
            }
            "parse_ts" => d.value(&label, &st["want"], &iso(cost::parse_ts(Some(&st["value"])))),
            "fmt" => d.text(&label, s(&st["want"]), &cost::fmt(st["n"].as_i64().expect("n"))),
            "read_agent" => {
                let got = cost::read_agent(&real(s(&st["path"]))).map(|a| {
                    json!({"tokens": a.tokens, "requests": a.requests, "models": a.models.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                           "first": iso(a.first), "last": iso(a.last), "head_len": a.head.chars().count(), "head_sha1": sha1_hex(&a.head)})
                });
                probe(&mut d, &label, st, got);
            }
            "tail" => {
                let state = tails.entry(s(&st["key"]).to_string()).or_default();
                let got = cost::tail_agent(&real(s(&st["path"])), state).map(|()| {
                    json!({"offset": state.offset, "tokens": state.tokens, "requests": state.requests, "model": state.model, "first": iso(state.first),
                           "last": iso(state.last), "seen": state.seen.len()})
                });
                probe(&mut d, &label, st, got);
            }
            "read_meta" => d.value(&label, &st["want"], &Value::Object(cost::read_meta(&real(s(&st["path"]))))),
            "read_wf_meta" => probe(&mut d, &label, st, cost::read_wf_meta(&real(s(&st["path"]))).map(Value::Object)),
            "find_wf_dirs" => {
                let paths: Vec<String> = st["paths"].as_array().map(|a| a.iter().map(|x| real(s(x))).collect()).unwrap_or_default();
                let got: Vec<String> = cost::find_wf_dirs(&paths).iter().map(|p| norm(p)).collect();
                d.value(&label, &st["want"], &json!(got));
            }
            "project_dir" => probe(&mut d, &label, st, cost::project_dir(&real(s(&st["root"]))).map(|p| json!(norm(&p)))),
            other => panic!("不認得的步驟 {other}"),
        }
    }
    std::env::set_current_dir(old_cwd).expect("chdir");
    match old_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&base);
    d.finish("cost", steps.len());
}

fn set(v: &Value) -> BTreeSet<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default()
}

fn strs(xs: &[&str]) -> Value {
    json!(xs)
}

#[test]
fn script_matches_python() {
    let doc = baseline();
    let defaults = &doc["script"]["defaults"];
    let mut d = Diff::new();
    let steps = doc["script"]["steps"].as_array().expect("steps");
    let mut looser: Vec<String> = Vec::new();
    for (i, st) in steps.iter().enumerate() {
        let op = s(&st["op"]);
        let text = s(&st["text"]);
        let short: String = text.chars().take(80).collect();
        let label = format!("script 步驟 {i} {op}（{}）{short:?}", s(&st["note"]));
        // 讀不懂的腳本：0.x 的 Python 版丟 ScriptError，訊息要一樣
        let fallible = |d: &mut Diff, got: Result<Value, script::ScriptError>| match (st.get("err"), got) {
            (Some(want), Err(e)) => d.text(&label, s(want), &e.0),
            (Some(want), Ok(v)) => d.bad.push(format!("{label}\n  預期：ScriptError（{want}）\n  實際：{v}")),
            (None, Err(e)) => d.bad.push(format!("{label}\n  預期：{}\n  實際：ScriptError（{e}）", st["want"])),
            (None, Ok(v)) => d.value(&label, &st["want"], &v),
        };
        assert!(st.get("crash").is_none(), "{label}：0.x 的 Python 版在這裡當掉，基準不該有這種案例");
        match op {
            "consts" => {
                let got = json!({
                    "STAGES": strs(&script::STAGES), "ROLES": strs(&script::ROLES), "SNIPPETS": strs(&script::SNIPPETS),
                    "PREAMBLE_NAMES": strs(&script::PREAMBLE_NAMES),
                    "FORBIDDEN": script::FORBIDDEN.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                    "CONTROL": strs(&script::CONTROL), "SHADOWED": strs(&script::SHADOWED), "GOOD": script::GOOD,
                });
                d.value(&label, &st["want"], &got);
            }
            "check" => {
                let pick = |k: &str| st.get(k).unwrap_or(&defaults[k]);
                let got = script::check(text, &set(pick("allowed")), s(pick("ns")), &set(pick("reserved")));
                // 守門性質：0.x 的 Python 版會擋的，這裡一定要擋
                if st["want"].as_array().is_some_and(|w| !w.is_empty()) && got.is_empty() {
                    looser.push(label.clone());
                }
                d.value(&label, &st["want"], &json!(got));
            }
            "mask" => fallible(&mut d, script::mask(text).map(Value::String)),
            "meta_end" => fallible(&mut d, script::mask(text).and_then(|m| script::meta_end(text, &m)).map(|n| json!(n))),
            "assemble" => {
                let got = script::assemble(text, s(&st["preamble"]), s(&st["stage"])).map(|out| {
                    let mut v = json!({"sha1": sha1_hex(&out), "len": out.chars().count()});
                    if out.chars().count() <= 600 {
                        v["text"] = json!(out);
                    }
                    v
                });
                fallible(&mut d, got);
            }
            "defined_names" => {
                let srcs: Vec<&str> = st["snippets"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default();
                let got: Vec<String> = script::defined_names(srcs).into_iter().collect();
                d.value(&label, &st["want"], &json!(got));
            }
            "selftest" => {
                let (rc, out) = script::selftest_captured();
                d.text(&format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
                d.text(&format!("{label} 輸出"), s(&st["out"]), &out);
            }
            "run" => {
                let argv: Vec<String> = st["argv"].as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default();
                let (rc, _, _) = script::run_captured(&argv);
                d.text(&format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
            }
            other => panic!("不認得的步驟 {other}"),
        }
    }
    assert!(looser.is_empty(), "靜態檢查比 0.x 的 Python 版鬆（0.x 的 Python 版有擋、這裡放行）：\n{}", looser.join("\n"));
    d.finish("script", steps.len());
}
