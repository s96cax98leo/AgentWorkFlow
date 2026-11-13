//! schema 驗證的對跑：和 0.x 的 Python 版錄下來的基準（tests/parity/schema.expected.json）逐字比對。

use awf_core::schema::{self, Schemas};
use serde_json::Value;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 使用者寫壞的正規式，錯誤說明由引擎產生，兩個實作的文字不同：只比到「正規式：」為止。
fn normalize(e: &str) -> String {
    match e.split_once("不是合法的正規式：") {
        Some((head, _)) => format!("{head}不是合法的正規式："),
        None => e.to_string(),
    }
}

fn load(rel: &str) -> Value {
    let p = root().join(rel);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

#[test]
fn golden_cases_match_python() {
    let expected = load("tests/parity/schema.expected.json");
    let teams = root().join("plugin/scaffold/.claude/teams");
    let mut bad = Vec::new();
    let cases = expected["cases"].as_array().expect("cases");
    assert!(cases.len() >= 100, "案例數不對：{}", cases.len());
    for c in cases {
        let file = c["file"].as_str().unwrap();
        let raw = load(&format!("plugin/scaffold/.claude/teams/{file}"));
        let case = if c["kind"] == "profile" { serde_json::json!({"schema": "methodology", "data": raw}) } else { raw };
        let mut s = Schemas::new(&teams);
        s.config_override = case.get("config").and_then(Value::as_object).cloned();
        let (errs, sch) = match case.get("schema").and_then(Value::as_str) {
            Some(name) => (s.check_doc(&case["data"], name).unwrap(), s.doc_schema(name).unwrap()),
            None => {
                let p = case.get("payload").and_then(Value::as_str);
                (s.check(&case["data"], p).unwrap(), s.compose(p).unwrap())
            }
        };
        let want: Vec<String> = c["errors"].as_array().unwrap().iter().map(|e| normalize(e.as_str().unwrap())).collect();
        let got: Vec<String> = errs.iter().map(|e| normalize(e)).collect();
        if want != got { bad.push(format!("{file}\n  要：{want:?}\n  得：{got:?}")); }
        let unsup: Vec<String> = c["unsupported"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect();
        if unsup != schema::unsupported_keywords(&sch, "$") { bad.push(format!("{file}：unsupported_keywords 不同")); }
    }
    assert!(bad.is_empty(), "{} 個案例不同：\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn probes_match_python() {
    let expected = load("tests/parity/schema.expected.json");
    for p in expected["probes"].as_array().unwrap() {
        let got = schema::validate(&p["data"], &p["schema"], "$");
        let want: Vec<String> = p["errors"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect();
        assert_eq!(want, got, "schema={} data={}", p["schema"], p["data"]);
    }
}

#[test]
fn extract_json_matches_python() {
    let expected = load("tests/parity/schema.expected.json");
    for p in expected["extract"].as_array().unwrap() {
        let got = schema::extract_json(p["text"].as_str().unwrap()).unwrap_or(Value::Null);
        assert_eq!(p["value"], got, "text={}", p["text"]);
    }
}
