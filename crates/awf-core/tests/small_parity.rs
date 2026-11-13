//! 三支小工具（run_brief、check_envelope、check_rules）的對跑：照 0.x 的 Python 版錄下來的劇本
//! （tests/parity/small.expected.json）做一遍，逐字比對。劇本的指令用 sh 與 git，所以只在 unix 上跑。
//! 整份劇本放在同一個測試裡：它會改環境變數（AWF_LOG_DIR、git 的固定身分），不能和別的測試平行。
#![cfg(unix)]

use awf_core::{check_envelope, check_rules, run_brief};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Demo"), ("GIT_AUTHOR_EMAIL", "demo@example.com"), ("GIT_COMMITTER_NAME", "Demo"),
    ("GIT_COMMITTER_EMAIL", "demo@example.com"), ("GIT_AUTHOR_DATE", "2026-01-02T03:04:05+00:00"),
    ("GIT_COMMITTER_DATE", "2026-01-02T03:04:05+00:00"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_NOSYSTEM", "1"),
];

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn strs(v: &Value, tmp: &str) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| s(x).replace("@TMP@", tmp)).collect()).unwrap_or_default()
}

#[test]
fn small_tools_match_python() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("tests/parity/small.expected.json")).expect("讀不到基準");
    let doc: Value = serde_json::from_str(&text).expect("JSON");
    let teams = root.join("plugin/scaffold/.claude/teams");
    let base = std::env::temp_dir().join(format!("awf-small-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("建暫存目錄");
    let tmp_path = base.canonicalize().expect("canonicalize");
    let tmp = tmp_path.to_string_lossy().into_owned();
    for (k, v) in GIT_ENV {
        std::env::set_var(k, v);
    }
    let real = |x: &str| x.replace("@TMP@", &tmp);
    let norm = |x: &str| x.replace(&tmp, "@TMP@");
    let mut bad: Vec<String> = Vec::new();
    let mut diff = |i: usize, what: &str, want: &str, got: &str| {
        if want != got {
            bad.push(format!("步驟 {i} {what}\n  預期：{want:?}\n  實際：{got:?}"));
        }
    };
    let steps = doc["steps"].as_array().expect("steps");
    for (i, st) in steps.iter().enumerate() {
        match s(&st["op"]) {
            "write" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                std::fs::write(&p, s(&st["text"])).expect("write");
            }
            "git" => {
                let cwd = real(s(&st["cwd"]));
                std::fs::create_dir_all(&cwd).expect("mkdir");
                let out = Command::new("git").args(strs(&st["args"], &tmp)).current_dir(&cwd).output().expect("git");
                assert!(out.status.success(), "步驟 {i} git 失敗：{}", String::from_utf8_lossy(&out.stderr));
            }
            "detect" => {
                let (name, fails) = run_brief::detect(s(&st["text"]));
                diff(i, "detect", &st["want"].to_string(), &serde_json::json!([name, fails]).to_string());
            }
            "brief" => {
                let log = real(s(&st["log"]));
                let (code, text) = run_brief::brief(&strs(&st["cmd"], &tmp), &log, st["tail"].as_i64()).expect("brief");
                diff(i, "brief 結束碼", &st["code"].to_string(), &code.to_string());
                diff(i, "brief 摘要", s(&st["text"]), &norm(&text));
                diff(i, "brief log", s(&st["logged"]), &String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()));
            }
            "default_log" => {
                std::env::set_var("AWF_LOG_DIR", real("@TMP@/envlogs"));
                let p = run_brief::default_log(s(&st["name"])).expect("default_log");
                std::fs::write(&p, "x").expect("write");
                diff(i, "default_log", s(&st["want"]), &norm(&p));
            }
            op @ ("envelope" | "rules") => {
                let argv = strs(&st["argv"], &tmp);
                let (rc, out, err) = if op == "envelope" {
                    let input = s(&st["stdin"]).to_string();
                    check_envelope::run_captured(&argv, &teams, &mut || input.clone())
                } else {
                    check_rules::run_captured(&argv, Path::new(&real(s(&st["project"]))))
                };
                let label = format!("{op} {:?}", st["argv"]);
                diff(i, &format!("{label} rc"), &st["rc"].to_string(), &rc.to_string());
                diff(i, &format!("{label} stdout"), s(&st["out"]), &norm(&out));
                if st.get("err_any").is_none() {
                    diff(i, &format!("{label} stderr"), s(&st["err"]), &norm(&err));
                } else if err.trim().is_empty() {
                    diff(i, &format!("{label}：出錯時 stderr 要有說明"), "有說明", "空的");
                }
            }
            other => panic!("不認得的步驟 {other}"),
        }
    }
    std::env::remove_var("AWF_LOG_DIR");
    let _ = std::fs::remove_dir_all(&base);
    assert!(bad.is_empty(), "{} 處不同（共 {} 步）：\n{}", bad.len(), steps.len(), bad.join("\n"));
}
