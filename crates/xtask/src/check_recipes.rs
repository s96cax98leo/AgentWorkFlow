//! recipe 解析的一致性檢查。由 `tests/check_recipes.py` 移植，對跑的一邊從 Python 版 `recipe.py` 換成 Rust 版
//! [`awf_core::recipe`]：
//!   1. `awf_core::recipe` 的 selftest 通過。
//!   2. `snippets/recipe.js`（workflow 用）和 `awf_core::recipe` 對 `golden-set/recipes/*.json` 的每個案例解析結果
//!      完全相同（用 node 跑 JS 那一邊，本體在 `js/recipes.cjs`）。
//!   3. 兩份內建 profile（`methodologies/*.json`）的 recipes／modifiers 和內建的一致
//!      （lightweight 的 feature 可以有自己的調研角度，其餘相同）。
//!
//! 沒有 node 時第 2 項判失敗（不略過）。和 Python 版不同的地方（只會更嚴）：golden 案例或 profile 讀不懂、
//! 解析時出錯，Python 版直接當掉（traceback、exit 1），這裡列成一條失敗；node 回的案例數和送去的不同也算失敗
//! （Python 版的 zip 會默默截短）。

use crate::util::{self, Captured};
use awf_core::merge_gate::pyutil;
use awf_core::{pyfmt, recipe};
use serde_json::{Map, Value};
use std::path::Path;

const JS: &str = include_str!("js/recipes.cjs");
const KEYS: [&str; 13] = [
    "name", "stages", "plan_mode", "research_angles", "design_lenses", "test", "deliver_axes", "build_kinds",
    "publishable", "followup", "split", "modifiers", "errors",
];

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 送給 node 的案例：profile 展開成實際的那一份，其餘欄位照抄（沒有的給 null）。
fn payload(cases: &[(std::path::PathBuf, Value)], errs: &mut Vec<String>) -> Vec<Value> {
    let mut out = Vec::new();
    for (path, c) in cases {
        let mut o = Map::new();
        match recipe::case_profile(c) {
            Ok(p) => o.insert("profile".into(), p),
            Err(e) => {
                errs.push(format!("{}：案例讀不懂（{e}）", file_name(path)));
                o.insert("profile".into(), Value::Null)
            }
        };
        for k in ["recipe", "modifiers", "size"] {
            o.insert(k.into(), c.get(k).cloned().unwrap_or(Value::Null));
        }
        if let Some(m) = c.get("model_cases") {
            o.insert("model_cases".into(), m.clone());
        }
        out.push(Value::Object(o));
    }
    out
}

/// 第 2 項：每個案例 Rust 與 JS 的結果逐欄比（Python 的 `==`）。
fn compare(cases: &[(std::path::PathBuf, Value)], js: &[Value], errs: &mut Vec<String>) {
    if js.len() != cases.len() {
        errs.push(format!("recipe.js 回了 {} 個案例的結果，應該是 {} 個", js.len(), cases.len()));
    }
    for ((path, c), got) in cases.iter().zip(js) {
        let rs = match recipe::run_case(c) {
            Ok(v) => v,
            Err(e) => {
                errs.push(format!("{}：Rust 版解析出錯（{e}）", file_name(path)));
                continue;
            }
        };
        let mut keys: Vec<&str> = KEYS.to_vec();
        if rs.get("models").is_some() {
            keys.push("models");
        }
        let mut diff = Map::new();
        for k in keys {
            let mine = rs.get(k).cloned().unwrap_or(Value::Null);
            let theirs = got.get(k).cloned().unwrap_or(Value::Null);
            if !pyfmt::eq(&mine, &theirs) {
                let mut d = Map::new();
                d.insert("rs".into(), mine);
                d.insert("js".into(), theirs);
                diff.insert(k.into(), Value::Object(d));
            }
        }
        if !diff.is_empty() {
            let text = recipe::dumps(&Value::Object(diff), None);
            errs.push(format!("{}：recipe.js 和 awf_core::recipe 不一致 {}", file_name(path), pyutil::head(&text, 500)));
        }
    }
}

fn without_angles(name: &str, r: &Map<String, Value>) -> Value {
    Value::Object(r.iter().filter(|(k, _)| !(name == "feature" && (*k == "research_angles" || *k == "small"))).map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn sorted_keys_repr(o: &Map<String, Value>) -> String {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.sort();
    pyfmt::repr(&Value::Array(keys.into_iter().map(|k| Value::String(k.clone())).collect()))
}

/// 第 3 項：內建 profile 和內建 recipe／modifier 一致。
fn check_profiles(teams: &Path, errs: &mut Vec<String>) {
    let dir = teams.join("methodologies");
    let mut files: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".json")))
        .collect();
    files.sort();
    let empty = Map::new();
    let builtin = recipe::builtin_recipes().as_object().unwrap_or(&empty);
    for prof in files {
        let name = file_name(&prof);
        let d = match util::read_json(&prof) {
            Ok(Value::Object(d)) => d,
            Ok(_) => {
                errs.push(format!("{name}：不是 JSON 物件"));
                continue;
            }
            Err(e) => {
                errs.push(format!("{name}：讀不懂（{e}）"));
                continue;
            }
        };
        // `d.get(k) or {}`：沒有或是空值都當成空的；有值但不是物件就是格式不對
        let section = |k: &str| -> Result<Map<String, Value>, ()> {
            match d.get(k) {
                Some(Value::Object(o)) => Ok(o.clone()),
                Some(v) if pyfmt::truthy(v) => Err(()),
                _ => Ok(Map::new()),
            }
        };
        let (Ok(rec), Ok(mods)) = (section("recipes"), section("modifiers")) else {
            errs.push(format!("{name}：recipes 或 modifiers 不是物件"));
            continue;
        };
        if rec.len() != builtin.len() || rec.keys().any(|k| !builtin.contains_key(k)) {
            errs.push(format!("{name}：recipes 應有 {}，實際 {}", sorted_keys_repr(builtin), sorted_keys_repr(&rec)));
        }
        if !pyfmt::eq(&Value::Object(mods), recipe::builtin_modifiers()) {
            errs.push(format!("{name}：modifiers 和內建的 modifiers 不一致"));
        }
        for (n, r) in &rec {
            let Some(want) = builtin.get(n).and_then(Value::as_object) else { continue };
            let same = r.as_object().is_some_and(|r| pyfmt::eq(&without_angles(n, r), &without_angles(n, want)));
            if !same {
                errs.push(format!("{name}：recipe {n} 和內建的 recipe 不一致（只有 feature 的調研角度可以不同）"));
            }
        }
    }
}

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path) -> Captured {
    let teams = util::teams_dir(root);
    let mut errs: Vec<String> = Vec::new();
    let mut st = String::new();
    if recipe::selftest(&teams, &mut st) != 0 {
        errs.push(format!("awf_core::recipe 的 selftest 失敗：\n{}", pyutil::tail(&st, 1500)));
    }
    let cases = match recipe::golden_cases(&teams) {
        Ok(c) => c,
        Err(e) => {
            errs.push(format!("golden 案例讀不懂（{e}）"));
            Vec::new()
        }
    };
    match util::which("node") {
        None => errs.push("找不到 node，無法比對 recipe.js（fail-closed）".into()),
        Some(node) => {
            let input = Value::Array(payload(&cases, &mut errs)).to_string();
            let keys = Value::Array(KEYS.iter().map(|k| Value::String(k.to_string())).collect()).to_string();
            let js = JS.replace("KEYS", &keys);
            let file = util::snippet_dir(root).join("recipe.js");
            match util::node_eval(&node, &js, &[file], Some(&input)) {
                Err(e) => errs.push(format!("node 跑 recipe.js 失敗：{e}")),
                Ok(q) if q.code != 0 => errs.push(format!("node 跑 recipe.js 失敗：{}", pyutil::tail(&q.stderr, 800))),
                Ok(q) => match serde_json::from_str::<Value>(&q.stdout) {
                    Ok(Value::Array(js)) => compare(&cases, &js, &mut errs),
                    _ => errs.push(format!("node 跑 recipe.js 的輸出讀不懂：{}", pyutil::tail(&q.stdout, 400))),
                },
            }
        }
    }
    check_profiles(&teams, &mut errs);
    if !errs.is_empty() {
        let mut out = String::from("recipe 一致性檢查失敗：\n");
        for e in &errs {
            out.push_str(&format!("  - {e}\n"));
        }
        return (1, out, String::new());
    }
    (0, format!("recipe 一致性檢查通過：golden {} 個案例 rs＝js，內建 profile 與 awf_core::recipe 一致\n", cases.len()), String::new())
}
