//! `awf tokens check`／`verify` 的對跑：照 Python 版（設計系統 skill 原本的 check-tokens.py、verify-tokens.py）錄下來的
//! 基準（tests/parity/tokens.expected.json）每個案例重建一次檔案、跑一次，逐字比對 exit code、stdout、stderr。
//!
//! 和 Python 版的差別只有兩處，比對時換掉：用法訊息的程式名（`check-tokens.py` → `awf tokens check`）；
//! Python 版以 Traceback 結束的案例（`crash`）只比 exit code 與 stdout，stderr 要是一行說明、不能是空的。
//! 案例用到符號連結，所以只在 unix 上跑。
#![cfg(unix)]

use awf_core::tokens;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("上層目錄")).expect("mkdir");
    std::fs::write(path, bytes).expect("write");
}

#[test]
fn tokens_match_python() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("tests/parity/tokens.expected.json")).expect("讀不到基準");
    let doc: Value = serde_json::from_str(&text).expect("JSON");
    let base = std::env::temp_dir().join(format!("awf-tokens-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("建暫存目錄");
    let base = base.canonicalize().expect("canonicalize");
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() >= 50, "基準案例變少了：{}", cases.len());
    let mut bad: Vec<String> = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let tmp = base.join(format!("c{i}"));
        let t = tmp.to_string_lossy().into_owned();
        let (refdir, proj) = (tmp.join("skill/references"), tmp.join("proj"));
        std::fs::create_dir_all(&refdir).expect("mkdir");
        std::fs::create_dir_all(&proj).expect("mkdir");
        for (dir, key) in [(&refdir, "ref"), (&proj, "proj")] {
            for (rel, body) in c[key].as_object().expect("檔案表") {
                write(&dir.join(rel), s(body).as_bytes());
            }
        }
        for (rel, bytes) in c["binary"].as_object().expect("binary") {
            let data: Vec<u8> = bytes.as_array().expect("位元組").iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect();
            write(&tmp.join(rel), &data);
        }
        for (rel, target) in c["symlinks"].as_object().expect("symlinks") {
            let p = tmp.join(rel);
            std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("mkdir");
            std::os::unix::fs::symlink(s(target), &p).expect("symlink");
        }
        let argv: Vec<String> = c["argv"].as_array().expect("argv").iter().map(|a| s(a).replace("@TMP@", &t)).collect();
        let (code, out, err) = match s(&c["tool"]) {
            "check" => tokens::run_check(&argv, &refdir, &proj),
            "verify" => tokens::run_verify(&argv, &refdir, &proj),
            other => panic!("不認得的 tool：{other}"),
        };
        let (out, err) = (out.replace(&t, "@TMP@"), err.replace(&t, "@TMP@"));
        let name = s(&c["name"]);
        let mut diff = |what: &str, want: &str, got: &str| {
            if want != got {
                bad.push(format!("［{name}］{what}\n  預期：{want:?}\n  實際：{got:?}"));
            }
        };
        diff("exit code", &c["code"].to_string(), &code.to_string());
        diff("stdout", s(&c["stdout"]), &out);
        if c["crash"].as_bool().unwrap_or(false) {
            let one_line = err.ends_with('\n') && err.trim_end().lines().count() == 1 && err.starts_with("awf tokens ");
            if !one_line || err.contains("Traceback") {
                bad.push(format!("［{name}］Python 版當掉的案例要印一行說明，得到：{err:?}"));
            }
        } else {
            diff("stderr", &s(&c["stderr"]).replace("用法：check-tokens.py ", "用法：awf tokens check "), &err);
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(bad.is_empty(), "{} 處和 Python 版不同：\n{}", bad.len(), bad.join("\n"));
}

/// 入口：`--ref`、自動找 references/、子指令與用法錯誤。
#[test]
fn tokens_entry_finds_references() {
    let base = std::env::temp_dir().join(format!("awf-tokens-entry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    let skill = |name: &str, color: &str| {
        write(&base.join(format!("proj/.claude/skills/{name}/references/source.conf")), b"SOURCE=\n");
        write(&base.join(format!("proj/.claude/skills/{name}/references/tokens.css")), format!(":root {{ --c: {color}; }}\n").as_bytes());
    };
    write(&base.join("proj/src/a.css"), b"a { color: #111111; }\n");
    let proj = base.join("proj");

    // 專案裡沒有設計系統 skill：說明並 exit 2
    let (code, _, err) = tokens::run_captured(&v(&["check", "src"]), &proj, &proj);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--ref"), "{err}");
    // 剛好一個：自動用它
    skill("acme-ds", "#111111");
    let (code, out, err) = tokens::run_captured(&v(&["check", "src"]), &proj, &proj);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert!(out.contains("白名單 1 個色值；掃描 1 個檔；違規 0 處"), "{out}");
    let (code, out, _) = tokens::run_captured(&v(&["verify"]), &proj, &proj);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("沒有設定正本"), "{out}");
    // 在 skill 目錄裡跑：用目前目錄的 references/
    let here = proj.join(".claude/skills/acme-ds");
    let (code, out, _) = tokens::run_captured(&v(&["check", "../../../src"]), &here, &proj);
    assert_eq!(code, 0, "{out}");
    // 兩個：不猜，要人指定；--ref（兩種寫法、放哪裡都可以）指定後照它的白名單
    skill("other-ds", "#222222");
    let (code, _, err) = tokens::run_captured(&v(&["check", "src"]), &proj, &proj);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("acme-ds") && err.contains("other-ds"), "{err}");
    let (code, out, _) = tokens::run_captured(&v(&["check", "--ref", ".claude/skills/other-ds/references", "src"]), &proj, &proj);
    assert_eq!(code, 1, "{out}");
    assert!(out.starts_with("src/a.css:1: #111111\n"), "{out}");
    let (code, out, _) = tokens::run_captured(&v(&["--ref=.claude/skills/acme-ds/references", "check", "src"]), &proj, &proj);
    assert_eq!(code, 0, "{out}");
    // 用法
    assert_eq!(tokens::run_captured(&v(&["check"]), &proj, &proj).0, 2);
    assert_eq!(tokens::run_captured(&v(&[]), &proj, &proj).0, 2);
    assert_eq!(tokens::run_captured(&v(&["frobnicate"]), &proj, &proj).0, 2);
    assert_eq!(tokens::run_captured(&v(&["check", "src", "--ref"]), &proj, &proj).0, 2);
    let (code, out, _) = tokens::run_captured(&v(&["--help"]), &proj, &proj);
    assert!(code == 0 && out.contains("awf tokens verify"), "{out}");
    let _ = std::fs::remove_dir_all(&base);
}
