//! 1.0 的兩個狀態機接點：
//! - sdlc-merge 的合併角色與凍結檔跟 guard.config.json 的 `auto_merge`（`stage_args` 帶進 args，`--set` 蓋不掉）。
//! - profile 的 `roles`（把某一步換成自己的 agent）沒登記在 `extension_agents` 時，產生腳本與 `awf build` 都拒絕。

use awf_core::state::{render, stage_args, Env};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Proj {
    root: PathBuf,
    env: Env,
    plugin: String,
}

impl Drop for Proj {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn proj(tag: &str) -> Proj {
    let root = std::env::temp_dir().join(format!("awf-state-roles-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let teams = root.join(".claude/teams");
    std::fs::create_dir_all(&teams).expect("建立暫存專案");
    std::fs::create_dir_all(root.join("_runs/r1")).expect("建立 run 目錄");
    std::fs::write(teams.join("workflow.config"), "AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE=\"繁體中文\"\nBRANCH_PREFIX=feature/\nRUNS_DIR=_runs\nFORK_REPO=\nUPSTREAM_REPO=acme/demo\nFORBIDDEN_REPOS=\nSTAKEHOLDER=示範客戶 ACME\nDECISION_OWNER=示範客戶 ACME\nATTRIBUTION_LINE=\n").expect("寫 workflow.config");
    std::fs::write(teams.join("VERSION"), "9.9.9\n").expect("寫 VERSION");
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin").canonicalize().expect("模版的 plugin 目錄");
    let p = root.to_string_lossy().to_string();
    let env = Env { project: p.clone(), cwd: p.clone(), home: format!("{p}/home"), plugin_env: [None, None], template: Some(plugin.to_string_lossy().to_string()), transcripts: None, now: None, today: None };
    let plugin = plugin.to_string_lossy().to_string();
    Proj { root, env, plugin }
}

fn put(p: &Proj, rel: &str, v: &Value) {
    std::fs::write(p.root.join(rel), serde_json::to_string_pretty(v).expect("JSON")).expect("寫檔");
}

fn merge_args(p: &Proj, run_args: Value) -> (serde_json::Map<String, Value>, Vec<String>) {
    let run = p.root.join("_runs/r1").to_string_lossy().to_string();
    let data = json!({"epic": "demo", "args": run_args, "stages": {}});
    let (a, _, notes) = stage_args(&p.env, &run, &data, "merge", "sdlc-merge").unwrap_or_else(|e| panic!("stage_args 失敗：{e:?}"));
    (a, notes)
}

#[test]
fn merge_args_follow_guard_config() {
    // 沒有設定檔、沒寫、寫的就是預設值：args 不多任何鍵（和以前逐字相同）
    let p = proj("merge");
    let none = |a: &serde_json::Map<String, Value>| !a.contains_key("merger_role") && !a.contains_key("freeze_file");
    let (a, notes) = merge_args(&p, json!({"pr": 7}));
    assert!(none(&a) && notes.is_empty(), "沒有 guard.config.json 時不該帶：{a:?}");
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"enabled": true}}));
    let (a, _) = merge_args(&p, json!({"pr": 7}));
    assert!(none(&a), "auto_merge 沒寫這兩個值時不該帶：{a:?}");
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"enabled": true, "merger_role": "agent-workflow:dev-merge-steward", "freeze_file": "_runs/auto-merge.freeze.json"}}));
    let (a, notes) = merge_args(&p, json!({"pr": 7}));
    assert!(none(&a) && notes.is_empty(), "寫的就是預設值時不該帶：{a:?}");

    // 有明寫而且和預設不同：帶進 args，並說明
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"enabled": true, "merger_role": "my-plugin:dev-merge-steward", "freeze_file": "ops/auto-merge.freeze.json"}}));
    let (a, notes) = merge_args(&p, json!({"pr": 7}));
    assert_eq!(a.get("merger_role"), Some(&json!("my-plugin:dev-merge-steward")));
    assert_eq!(a.get("freeze_file"), Some(&json!("ops/auto-merge.freeze.json")));
    assert_eq!(notes.iter().filter(|n| n.contains("auto_merge")).count(), 2, "{notes:?}");

    // --set（run.json 的 args）給的不採用：以 guard.config.json 為準；設定檔沒寫時整個拿掉
    let (a, _) = merge_args(&p, json!({"pr": 7, "merger_role": "acme:someone", "freeze_file": "x.json"}));
    assert_eq!(a.get("merger_role"), Some(&json!("my-plugin:dev-merge-steward")), "--set 不能蓋掉合併角色");
    assert_eq!(a.get("freeze_file"), Some(&json!("ops/auto-merge.freeze.json")));
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"enabled": true}}));
    let (a, _) = merge_args(&p, json!({"pr": 7, "merger_role": "acme:someone", "freeze_file": "x.json"}));
    assert!(none(&a), "設定檔沒寫時 --set 給的也不採用：{a:?}");

    // 不是字串、空白：當作沒寫
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"merger_role": 7, "freeze_file": "  "}}));
    let (a, _) = merge_args(&p, json!({"pr": 7}));
    assert!(none(&a), "型別不對或空白時不該帶：{a:?}");
}

#[test]
fn merge_args_only_for_merge() {
    // 別的段不受影響：--set 的同名參數原樣留著（只有 sdlc-merge 認這兩個鍵）
    let p = proj("other");
    put(&p, ".claude/teams/guard.config.json", &json!({"auto_merge": {"merger_role": "my-plugin:dev-merge-steward"}}));
    let run = p.root.join("_runs/r1").to_string_lossy().to_string();
    let data = json!({"epic": "demo", "args": {"merger_role": "x"}, "stages": {}});
    let (a, _, _) = stage_args(&p.env, &run, &data, "deliver", "sdlc-deliver").expect("stage_args");
    assert_eq!(a.get("merger_role"), Some(&json!("x")));
}

fn profile(roles: Value) -> Value {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/scaffold/.claude/teams/methodologies/default.json");
    let mut m: Value = serde_json::from_str(&std::fs::read_to_string(src).expect("讀內建 profile")).expect("JSON");
    m["roles"] = roles;
    m
}

#[test]
fn swapped_roles_must_be_registered() {
    let p = proj("roles");
    put(&p, ".claude/teams/methodology.json", &profile(json!({"plan.research": "acme-researcher"})));
    put(&p, ".claude/teams/guard.config.json", &json!({"extension_agents": {}}));
    let err = render(&p.env, "sdlc-plan", Some(&p.plugin), None).err().map(|e| format!("{e:?}")).unwrap_or_default();
    assert!(err.contains("acme-researcher") && err.contains("extension_agents"), "沒登記要拒絕產生腳本：{err}");
    let teams = p.root.join(".claude/teams");
    let m = profile(json!({"plan.research": "acme-researcher"}));
    let missing = awf_core::build::unregistered_extensions(Some(&m), &teams).expect("讀設定");
    assert_eq!(missing, vec!["roles.plan.research（acme-researcher）".to_string()], "awf build 也要列出來");

    // 登記之後照常產生，腳本裡帶著 profile 的 roles
    put(&p, ".claude/teams/guard.config.json", &json!({"extension_agents": {"acme-researcher": "tpm-researcher"}}));
    let (text, _) = render(&p.env, "sdlc-plan", Some(&p.plugin), None).unwrap_or_else(|e| panic!("登記後應該產生得出來：{e:?}"));
    assert!(text.contains("\"plan.research\": \"acme-researcher\"") || text.contains("\"plan.research\":\"acme-researcher\""), "腳本要內嵌 roles");
    assert!(awf_core::build::unregistered_extensions(Some(&m), &teams).expect("讀設定").is_empty());

    // 沒有 roles 的 profile：不需要 guard.config 有 extension_agents
    put(&p, ".claude/teams/methodology.json", &profile(Value::Null));
    put(&p, ".claude/teams/guard.config.json", &json!({}));
    assert!(render(&p.env, "sdlc-plan", Some(&p.plugin), None).is_ok(), "沒有 roles 時照舊");
}

/// 自訂腳本可以派的 agent：`auto_merge.merger_role` 指定的合併角色就算登記成擴充 agent 也不在裡面
/// （合併是信任邊界，只有 sdlc-merge 能派）。沒指定、或指定的是內建的 dev-merge-steward 時清單和以前相同。
#[test]
fn custom_scripts_cannot_dispatch_the_configured_merger() {
    let p = proj("script-merger");
    let ext = json!({"acme:release-merger": "tpm-reviewer", "acme-reviewer": "tpm-reviewer"});
    let roles = |p: &Proj| awf_core::state::custom_roles(&p.env).unwrap_or_else(|e| panic!("custom_roles 失敗：{e:?}"));
    put(&p, ".claude/teams/guard.config.json", &json!({"extension_agents": ext}));
    let before = roles(&p);
    assert!(before.contains(&"acme:release-merger".to_string()) && before.contains(&"acme-reviewer".to_string()), "{before:?}");
    assert!(!before.iter().any(|r| r.ends_with("dev-merge-steward")), "內建的合併角色本來就不能派：{before:?}");

    put(&p, ".claude/teams/guard.config.json", &json!({"extension_agents": ext, "auto_merge": {"merger_role": "agent-workflow:dev-merge-steward"}}));
    assert_eq!(roles(&p), before, "指定的是預設值時清單不變");

    put(&p, ".claude/teams/guard.config.json", &json!({"extension_agents": ext, "auto_merge": {"merger_role": "acme:release-merger"}}));
    let after = roles(&p);
    assert!(!after.contains(&"acme:release-merger".to_string()), "合併角色不能由自訂腳本派：{after:?}");
    assert!(after.contains(&"acme-reviewer".to_string()) && after.len() == before.len() - 1, "其他擴充 agent 不受影響：{after:?}");
}
