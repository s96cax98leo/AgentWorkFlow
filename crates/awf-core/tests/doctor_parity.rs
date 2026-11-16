//! doctor 的對跑：重放 `tests/parity/doctor.expected.json`（0.x 的 Python 版 awf_doctor 錄的），逐字比對。
//! 基準已經濾掉 Rust 版沒有的 `python`、`hooks-compile` 兩項。

use awf_core::doctor::{self, Item};
use awf_core::state::Env;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo 根目錄")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn items_json(items: &[Item]) -> Value {
    Value::Array(items.iter().map(|i| json!({"id": i.id, "level": i.level, "msg": i.msg, "fix": i.fix})).collect())
}

#[test]
fn replay_matches_python() {
    let src = repo();
    let base: Value = serde_json::from_str(&std::fs::read_to_string(src.join("tests/parity/doctor.expected.json")).expect("基準檔")).expect("基準是 JSON");
    let tmp = std::env::temp_dir().join(format!("awf-doctor-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("暫存目錄");
    let root = tmp.canonicalize().expect("暫存目錄").to_str().expect("UTF-8 路徑").replace('\\', "/");
    let src_s = src.to_str().expect("UTF-8 路徑").replace('\\', "/");
    let real = |t: &str| t.replace("@TMP@", &root);
    let norm = |t: &str| t.replace(&root, "@TMP@");
    let (mut bad, mut n_checks, mut n_fn) = (Vec::new(), 0, 0);
    // 「找不到 plugin」的步驟用的模版根：只有 schema、沒有 plugin/（0.x 的 Python 版是把 find_plugin 換掉，schema 照用模版的）
    let bare = format!("{root}/bare-template");
    let schemas = Path::new(&bare).join("scaffold/.claude/teams/schemas");
    std::fs::create_dir_all(schemas.join("payloads")).expect("建目錄");
    for sub in ["", "payloads"] {
        for f in std::fs::read_dir(src.join("plugin/scaffold/.claude/teams/schemas").join(sub)).expect("schema 目錄").flatten() {
            if f.path().is_file() {
                std::fs::copy(f.path(), schemas.join(sub).join(f.file_name())).expect("複製 schema");
            }
        }
    }

    // AWF_RECORD=1：照劇本重跑，把每一步 checks 的實際結果寫回基準（行為確實該變的時候用；差異要逐項看過）
    let record = std::env::var("AWF_RECORD").as_deref() == Ok("1");
    let mut recorded = base.clone();
    for (n, st) in base["steps"].as_array().expect("steps").iter().enumerate() {
        let path = tmp.join(s(&st["path"]));
        match s(&st["op"]) {
            "write" => {
                std::fs::create_dir_all(path.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&path, real(s(&st["text"]))).expect("寫檔");
            }
            "rm" => std::fs::remove_file(&path).expect("刪檔"),
            "rmdir" => std::fs::remove_dir(&path).expect("刪目錄"),
            "checks" => {
                n_checks += 1;
                let no_plugin = st["no_plugin"] == json!(true);
                let env = Env {
                    project: format!("{root}/{}", s(&st["project"])),
                    cwd: root.clone(),
                    home: format!("{root}/home"),
                    plugin_env: [None, None],
                    // 0.x 的 Python 版在模版 repo 裡跑，沒給 --plugin-dir 時用 repo 自己的 plugin；「找不到 plugin」的步驟是把那條路拿掉
                    template: Some(if no_plugin { bare.clone() } else { format!("{src_s}/plugin") }),
                    transcripts: None,
                    now: None,
                    today: None,
                };
                let tools: Vec<&str> = st["tools"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default();
                let plugin_dir = if s(&st["plugin_dir"]).is_empty() { String::new() } else { format!("{root}/{}", s(&st["plugin_dir"])) };
                let items = doctor::run_checks(&env, &plugin_dir, &|name| tools.contains(&name));
                let note = s(&st["note"]);
                let got: Value = serde_json::from_str(&norm(&items_json(&items).to_string())).expect("項目");
                if record {
                    let slot = &mut recorded["steps"][n];
                    slot["items"] = got.clone();
                    slot["text"] = json!(norm(&doctor::text_of(&items)));
                    slot["json"] = json!(norm(&doctor::json_of(&items)));
                    slot["code"] = json!(i64::from(items.iter().any(|i| i.level == doctor::ERR)));
                    continue;
                }
                if got != st["items"] {
                    bad.push(format!("第 {n} 步（{note}）項目不同：\n  Rust：{got}\n  基準：{}", st["items"]));
                    continue;
                }
                if norm(&doctor::text_of(&items)) != s(&st["text"]) {
                    bad.push(format!("第 {n} 步（{note}）文字版不同"));
                }
                if norm(&doctor::json_of(&items)) != s(&st["json"]) {
                    bad.push(format!("第 {n} 步（{note}）JSON 版不同"));
                }
                let code = i64::from(items.iter().any(|i| i.level == doctor::ERR));
                if json!(code) != st["code"] {
                    bad.push(format!("第 {n} 步（{note}）結束碼不同：{code}"));
                }
            }
            "fn" => {
                n_fn += 1;
                let args: Vec<String> = st["args"].as_array().map(|a| a.iter().map(|x| s(x).replace("@HOME@", &format!("{root}/home"))).collect()).unwrap_or_default();
                let got = match s(&st["name"]) {
                    "vtuple" => json!(doctor::vtuple(&args[0])),
                    "market_version" => json!(doctor::market_version(&args[0], &args[1])),
                    other => panic!("基準裡有沒處理的函式 {other}"),
                };
                if got != st["ret"] {
                    bad.push(format!("第 {n} 步 {}({:?})：Rust {got}，基準 {}", s(&st["name"]), st["args"], st["ret"]));
                }
            }
            other => panic!("基準裡有沒處理的步驟 {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    if record {
        // 基準檔的縮排是一格：把兩格縮排的輸出每行開頭的空白減半（字串裡的換行都跳脫過，不會在行首）
        let text: String = serde_json::to_string_pretty(&recorded).expect("JSON").lines()
            .map(|l| {
                let n = l.len() - l.trim_start_matches(' ').len();
                format!("{}{}", " ".repeat(n / 2), &l[n..])
            })
            .collect::<Vec<_>>().join("\n");
        std::fs::write(src.join("tests/parity/doctor.expected.json"), format!("{text}\n")).expect("寫回基準");
    }
    assert!(n_checks >= 70 && n_fn >= 25, "基準的步數變少了：checks {n_checks}、fn {n_fn}");
    assert!(bad.is_empty(), "{} 處不同：\n{}", bad.len(), bad.join("\n"));
}
