//! recipe 解析與模型分級的對跑：和 0.x 的 Python 版錄下來的基準（tests/parity/recipe.expected.json）逐字比對。
//!
//! 基準的格式見 `tests/parity/README.md`。比對一律比序列化後的文字，所以物件的鍵順序、
//! 整數與浮點數的差別（1 和 1.0）也算在內。

use awf_core::recipe::{self, GitOut, PyErr};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use std::sync::OnceLock;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn teams() -> PathBuf {
    root().join("plugin/scaffold/.claude/teams")
}

fn expected() -> &'static Value {
    static V: OnceLock<Value> = OnceLock::new();
    V.get_or_init(|| {
        let p = root().join("tests/parity/recipe.expected.json");
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
    })
}

/// 和 Python 的 `json.dumps(x, ensure_ascii=False)` 相同的文字。
fn text(v: &Value) -> String {
    recipe::dumps(v, None)
}

/// 還原參數：`{"$v": 編號}` 查 values 表，`{"$put": [資料, 路徑, 值]}`、`{"$drop": [資料, 路徑]}` 改一個欄位。
fn decode(arg: &Value) -> Value {
    let Some(o) = arg.as_object().filter(|o| o.len() == 1) else { return arg.clone() };
    if let Some(i) = o.get("$v") { return expected()["values"][i.as_u64().unwrap() as usize].clone(); }
    let (spec, put) = match (o.get("$put"), o.get("$drop")) {
        (Some(x), _) => (x, true),
        (_, Some(x)) => (x, false),
        _ => return arg.clone(),
    };
    let mut doc = decode(&spec[0]);
    let keys: Vec<&str> = spec[1].as_str().unwrap().split('.').collect();
    let mut cur = &mut doc;
    for k in &keys[..keys.len() - 1] { cur = &mut cur[*k]; }
    let last = keys[keys.len() - 1];
    if put { cur[last] = spec[2].clone(); } else { cur.as_object_mut().unwrap().shift_remove(last); }
    doc
}

fn strings(v: Vec<String>) -> Value {
    Value::Array(v.into_iter().map(Value::String).collect())
}

fn call(name: &str, a: &[Value]) -> Result<Value, PyErr> {
    Ok(match name {
        "resolve" => recipe::resolve(&a[0], &a[1], &a[2], &a[3])?,
        "resolve_model" => recipe::resolve_model(&a[0], &a[1], &a[2], &a[3], &a[4])?,
        "tier_model" => Value::String(recipe::tier_model(&a[0], &a[1])?),
        "model_up" => Value::String(recipe::model_up(&a[0])),
        "stricter_or_equal" => strings(recipe::stricter_or_equal(&a[0], &a[1])?),
        "merge_stricter" => recipe::merge_stricter(&a[0], &a[1])?,
        "escalate_errors" => strings(recipe::escalate_errors(&a[0], &a[1])?),
        // Python 的 classify 收任意值，不是字串就丟 AttributeError；Rust 的型別是 &str，這裡補上同樣的判斷
        "classify" => match &a[0] {
            Value::String(p) => json!(recipe::classify(p)),
            _ => return Err(PyErr { kind: "AttributeError", msg: "不是字串".into() }),
        },
        "diff_shortfalls" => strings(recipe::diff_shortfalls(&a[0], &a[1])?),
        "gate_mode" => recipe::gate_mode(&a[0], &a[1])?,
        "recipe_of_run" => recipe::recipe_of_run(&a[0])?,
        "required_sections" => Value::Array(recipe::required_sections(&a[0], &a[1])?),
        "missing_sections" => Value::Array(recipe::missing_sections(a[0].as_str().unwrap(), &a[1])?),
        "case_profile" => recipe::case_profile(&a[0])?,
        "run_case" => recipe::run_case(&a[0])?,
        other => panic!("基準裡有不認得的函式：{other}"),
    })
}

#[test]
fn builtins_match_python() {
    let b = &expected()["builtins"];
    let same = |name: &str, got: Value| assert_eq!(text(&b[name]), text(&got), "{name}");
    same("SDLC", json!(recipe::SDLC));
    same("PLAN_MODES", json!(recipe::PLAN_MODES));
    same("ALL_ANGLES", json!(recipe::ALL_ANGLES));
    same("ALL_AXES", json!(recipe::ALL_AXES));
    same("CODE_KINDS", json!(recipe::CODE_KINDS));
    same("BASE_SECTIONS", json!(recipe::BASE_SECTIONS));
    same("TIERS", json!(recipe::TIERS));
    same("GATE_MODES", json!(recipe::GATE_MODES));
    same("MODE_SECTIONS", recipe::mode_sections().clone());
    same("FALLBACK_FEATURE", recipe::fallback_feature().clone());
    same("BUILTIN_RECIPES", recipe::builtin_recipes().clone());
    same("BUILTIN_MODIFIERS", recipe::builtin_modifiers().clone());
    let pairs = |p: &[(&str, &str)]| Value::Object(p.iter().map(|(k, v)| (k.to_string(), json!(v))).collect());
    same("BUILTIN_TIER_MODELS", pairs(recipe::BUILTIN_TIER_MODELS));
    same("BUILTIN_MODEL_FLOORS", pairs(recipe::BUILTIN_MODEL_FLOORS));
    // LEVEL 沒有公開成常數：從 stricter_or_equal 的行為核對 never < auto < always
    assert_eq!(text(&b["LEVEL"]), r#"{"never": 0, "auto": 1, "always": 2}"#);
    for (lo, hi) in [("never", "auto"), ("auto", "always"), ("never", "always")] {
        let r = |x: &str| json!({"test": {"design_review": x}});
        assert!(recipe::stricter_or_equal(&r(hi), &r(lo)).unwrap().is_empty());
        assert_eq!(recipe::stricter_or_equal(&r(lo), &r(hi)).unwrap(), vec![format!("design_review 從 {hi} 降成 {lo}")]);
    }
}

#[test]
fn golden_cases_match_python() {
    let want = expected()["golden"].as_array().unwrap();
    let cases = recipe::golden_cases(&teams()).unwrap();
    assert!(want.len() >= 31, "案例數不對：{}", want.len());
    assert_eq!(want.len(), cases.len(), "golden-set/recipes 的檔案數和基準不同（重錄基準）");
    for (w, (path, case)) in want.iter().zip(&cases) {
        let file = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(w["file"], file);
        let got = recipe::run_case(case).unwrap_or_else(|e| panic!("{file}：{e}"));
        assert_eq!(text(&w["out"]), text(&got), "{file}");
        // 案例自己寫的期望值（Python ↔ JS 對跑用的那一份）也要相符
        for (k, v) in case["expect"].as_object().unwrap() {
            assert_eq!(text(v), text(&got[k]), "{file} 的 expect.{k}");
        }
    }
}

#[test]
fn probes_match_python() {
    let probes = expected()["probes"].as_array().unwrap();
    assert!(probes.len() >= 5000, "案例數不對：{}", probes.len());
    let mut bad = Vec::new();
    for (i, p) in probes.iter().enumerate() {
        let name = p["fn"].as_str().unwrap();
        let args: Vec<Value> = p["args"].as_array().unwrap().iter().map(decode).collect();
        let got = call(name, &args);
        let want = if let Some(k) = p.get("raise") {
            format!("raise {}", k.as_str().unwrap())
        } else if let Some(h) = p.get("ok_sha1") {
            format!("sha1 {}", h.as_str().unwrap())
        } else {
            text(&p["ok"])
        };
        let have = match &got {
            Err(e) => format!("raise {}", e.kind),
            Ok(v) if p.get("ok_sha1").is_some() => format!("sha1 {}", Sha1::digest(text(v).as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()),
            Ok(v) => text(v),
        };
        if want != have {
            let detail = match &got {
                Ok(v) => text(v),
                Err(e) => e.to_string(),
            };
            bad.push(format!("#{i} {name}({})\n  要：{want}\n  得：{have}\n  　　{detail}", args.iter().map(text).collect::<Vec<_>>().join(", ")));
        }
    }
    assert!(bad.is_empty(), "{} 筆不同（共 {} 筆）：\n{}", bad.len(), probes.len(), bad.iter().take(25).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn cli_matches_python() {
    let root_dir = root().canonicalize().unwrap();
    let root_text = root_dir.to_str().unwrap().replace('\\', "/");
    for (i, c) in expected()["cli"].as_array().unwrap().iter().enumerate() {
        let tmp = std::env::temp_dir().join(format!("awf-recipe-parity-{}-{i}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        if let Some(body) = c["run_json"].as_str() { std::fs::write(tmp.join("run.json"), body).unwrap(); }
        let tmp_text = tmp.to_str().unwrap().to_string();
        let argv: Vec<String> = c["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().replace("@ROOT@", &root_text).replace("@RUN@", &tmp_text)).collect();
        let seen = std::cell::RefCell::new(Vec::new());
        let git = |args: &[String]| -> Result<GitOut, String> {
            let mut cmd = vec![json!("git")];
            cmd.extend(args.iter().map(|a| json!(a.replace(&tmp_text, "@RUN@"))));
            seen.borrow_mut().push(Value::Array(cmd));
            let g = &c["git"];
            Ok(GitOut { code: g["code"].as_i64().unwrap() as i32, stdout: g["stdout"].as_str().unwrap().into(), stderr: g["stderr"].as_str().unwrap().into() })
        };
        let (mut out, mut errs) = (String::new(), String::new());
        let code = recipe::run(&teams(), &argv, &git, &mut out, &mut errs);
        let _ = std::fs::remove_dir_all(&tmp);
        let label = format!("#{i} {}", c["argv"]);
        assert_eq!(c["code"], code, "{label}（stderr：{errs}）");
        assert_eq!(c["stdout"].as_str().unwrap(), out.replace(&tmp_text, "@RUN@"), "{label}");
        assert_eq!(text(&c["git_argv"]), text(&Value::Array(seen.into_inner())), "{label}");
        // Python 沒接住的例外印的是 traceback，文字不比；其餘 stderr 要逐字相同
        if c.get("crash").is_none() { assert_eq!(c["stderr"].as_str().unwrap(), errs.replace(&tmp_text, "@RUN@"), "{label}"); } else { assert!(!errs.is_empty(), "{label}：出錯要有訊息"); }
    }
}

#[test]
fn usage_and_selftest_match_python() {
    let (mut out, mut errs) = (String::new(), String::new());
    let no_git = |_: &[String]| -> Result<GitOut, String> { Err("不該呼叫 git".into()) };
    assert_eq!(recipe::run(&teams(), &[], &no_git, &mut out, &mut errs), 2);
    assert_eq!(format!("{}\n", expected()["builtins"]["USAGE"].as_str().unwrap()), out);
    let mut out = String::new();
    let code = recipe::selftest(&teams(), &mut out);
    assert_eq!(expected()["selftest"]["stdout"].as_str().unwrap(), out);
    assert_eq!(expected()["selftest"]["code"], code);
}

/// 真的呼叫 git 的那一段（基準裡 git 是假的）：在暫存 repo 走一次 check-diff。
#[test]
fn check_diff_with_real_git() {
    let tmp = std::env::temp_dir().join(format!("awf-recipe-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("docs")).unwrap();
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git").arg("-C").arg(&tmp)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]).args(args).output().expect("git");
        assert!(o.status.success(), "git {args:?}：{}", String::from_utf8_lossy(&o.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(tmp.join("app.py"), "x = 1\n".repeat(20)).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["checkout", "-q", "-b", "work"]);
    std::fs::rename(tmp.join("app.py"), tmp.join("docs/app.py")).unwrap();
    std::fs::write(tmp.join("docs/a.md"), "說明\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "work"]);
    let docs = recipe::resolve(&recipe::builtin_profile(), &json!("docs"), &Value::Null, &Value::Null).unwrap();
    std::fs::write(tmp.join("run.json"), json!({"recipe": {"resolved": docs}}).to_string()).unwrap();
    let dir = tmp.to_str().unwrap().to_string();
    let run = |base: &str| {
        let argv: Vec<String> = ["check-diff", "--run", &dir, "--repo", &dir, "--base", base].iter().map(|x| x.to_string()).collect();
        // main 用真的 git；輸出直接印出來，這裡只看 exit code
        recipe::main(&argv)
    };
    // 改名的舊路徑（app.py）也算進來：docs 不能改產品程式
    assert_eq!(run("main"), 1);
    assert_eq!(run("work"), 0);
    assert_eq!(run("沒有這個分支"), 1);
    let _ = std::fs::remove_dir_all(&tmp);
}
