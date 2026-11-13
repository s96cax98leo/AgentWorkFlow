//! build（內嵌 schema、methodology 與片段）的對跑：照 Python 版錄下來的劇本（tests/parity/build.expected.json 的 `build`）
//! 做一遍，逐字比對 exit code、stdout、stderr 與跑完之後每支 workflow 的內容。
//! err_any 的步驟只比 exit code 與 stdout：Python 版在那裡直接當掉（traceback），這裡要有自己的說明。
#![cfg(unix)]

use awf_core::build::{self, Paths};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|i| h.get(i * 2..i * 2 + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

fn js_files(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".js"))
        .map(|n| {
            let text = String::from_utf8_lossy(&std::fs::read(dir.join(&n)).unwrap_or_default()).into_owned();
            (n, text)
        })
        .collect()
}

#[test]
fn build_matches_python() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("tests/parity/build.expected.json")).expect("讀不到基準");
    let doc: Value = serde_json::from_str(&text).expect("JSON");
    let base = std::env::temp_dir().join(format!("awf-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("建暫存目錄");
    let tmp_path = base.canonicalize().expect("canonicalize");
    let tmp = tmp_path.to_string_lossy().into_owned();
    let real = |x: &str| x.replace("@TMP@", &tmp);
    let norm = |x: &str| x.replace(&tmp, "@TMP@");
    let mut bad: Vec<String> = Vec::new();
    let (mut runs, mut loose) = (0, 0);
    let steps = doc["build"].as_array().expect("build");
    for (i, st) in steps.iter().enumerate() {
        let mut diff = |what: &str, want: &str, got: &str| {
            if want != got {
                bad.push(format!("步驟 {i} {what}\n  預期：{want:?}\n  實際：{got:?}"));
            }
        };
        match s(&st["op"]) {
            "write" | "write_hex" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                let data = if st.get("hex").is_some() { unhex(s(&st["hex"])) } else { s(&st["text"]).as_bytes().to_vec() };
                std::fs::write(&p, data).expect("write");
            }
            "remove" => {
                let p = real(s(&st["path"]));
                if Path::new(&p).is_dir() {
                    std::fs::remove_dir_all(&p).expect("rmtree");
                } else {
                    std::fs::remove_file(&p).expect("unlink");
                }
            }
            op @ ("build" | "build_real") => {
                runs += 1;
                let argv: Vec<String> = st["argv"].as_array().map(|a| a.iter().map(|x| s(x).to_string()).collect()).unwrap_or_default();
                let paths = if op == "build_real" {
                    Paths::template(&root.canonicalize().expect("repo 根目錄").join("plugin"))
                } else if s(&st["mode"]) == "template" {
                    Paths::template(&Path::new(&real(s(&st["root"]))).join("plugin"))
                } else {
                    Paths::project(Path::new(&real(s(&st["root"]))))
                };
                let (rc, out, err) = build::run_captured(&argv, &paths);
                let label = format!("{op} {:?}", st["argv"]);
                diff(&format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
                diff(&format!("{label} stdout"), s(&st["out"]), &norm(&out));
                if st.get("err_any").is_none() {
                    diff(&format!("{label} stderr"), s(&st["err"]), &norm(&err));
                } else {
                    loose += 1;
                    if err.trim().is_empty() {
                        diff(&format!("{label}：出錯時 stderr 要有說明"), "有說明", "空的");
                    }
                }
                if let Some(want) = st.get("files").and_then(Value::as_object) {
                    let got = js_files(&paths.wf_dir);
                    let want: BTreeMap<String, String> = want.iter().map(|(k, v)| (k.clone(), s(v).to_string())).collect();
                    let names = |m: &BTreeMap<String, String>| m.keys().cloned().collect::<Vec<_>>().join(",");
                    diff(&format!("{label} 檔名"), &names(&want), &names(&got));
                    for (name, text) in &want {
                        diff(&format!("{label} {name} 的內容"), text, got.get(name).map(String::as_str).unwrap_or_default());
                    }
                }
            }
            other => panic!("不認得的步驟 {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(runs > 100, "劇本太短：{runs}");
    println!("build：{runs} 次執行，{} 次逐字相同，{loose} 次只比 exit code 與 stdout（Python 版當掉）", runs - loose);
    assert!(bad.is_empty(), "{} 處不同（共 {} 步）：\n{}", bad.len(), steps.len(), bad.join("\n"));
}

/// 偵測模版 repo 與專案（Python 版依工具檔的位置判斷，這裡依目錄內容）。
#[test]
fn detect_paths() {
    let base = std::env::temp_dir().join(format!("awf-build-detect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let mk = |p: &str| std::fs::create_dir_all(base.join(p)).expect("mkdir");
    mk("tpl/plugin/workflow-templates");
    mk("tpl/plugin/scaffold/.claude/teams");
    mk("tpl/crates/deep");
    mk("tpl/_tpm/proj/.claude/teams");
    mk("plain/sub");
    let base = base.canonicalize().expect("canonicalize");
    let tpl = base.join("tpl");
    assert_eq!(Paths::detect(&tpl).wf_dir, tpl.join("plugin/workflow-templates"));
    assert_eq!(Paths::detect(&tpl).teams_dir, tpl.join("plugin/scaffold/.claude/teams"));
    assert_eq!(Paths::detect(&tpl).snippet_dir, tpl.join("plugin/workflow-templates/snippets"));
    // 模版 repo 的子目錄：往上找
    assert_eq!(Paths::detect(&tpl.join("crates/deep")).wf_dir, tpl.join("plugin/workflow-templates"));
    // 模版 repo 底下的已 init 專案：當專案
    assert_eq!(Paths::detect(&tpl.join("_tpm/proj")).wf_dir, tpl.join("_tpm/proj/.claude/workflows"));
    // 都不是：當專案（沒有 workflow 目錄就什麼都不做）
    let plain = Paths::detect(&base.join("plain/sub"));
    assert_eq!(plain.wf_dir, base.join("plain/sub/.claude/workflows"));
    assert_eq!(plain.teams_dir, base.join("plain/sub/.claude/teams"));
    let _ = std::fs::remove_dir_all(&base);
}
