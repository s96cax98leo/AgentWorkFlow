//! check-recipes、check-kernel、check-patterns、check-budget、check-supply 的測試。
//!
//! 每支檢查兩種：對這個 repo 實跑要通過；在暫存副本裡弄壞一個輸入（golden 的預期、片段裡的上限檢查、基準數字、
//! 版控裡的檔案）要報失敗、exit 非零，而且訊息指得出是哪裡。
//! 自訂腳本的組裝結果和固定的預期檔比（`tests/fixtures/custom-script-frame.*.js`）。
#![cfg(unix)]

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use xtask::util::{self, TempDir};
use xtask::{check_budget, check_kernel, check_patterns, check_recipes, check_supply};

fn root() -> PathBuf {
    xtask::repo_root()
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for e in std::fs::read_dir(src).expect("read_dir").flatten() {
        let (from, to) = (e.path(), dst.join(e.file_name()));
        if from.is_dir() {
            if e.file_name() != "__pycache__" {
                copy_dir(&from, &to);
            }
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// 暫存的 repo 根目錄，裡面只有 `plugin/scaffold/.claude/teams`（schema、golden-set、內建 profile）與
/// `plugin/workflow-templates/snippets`（片段）。
fn teams_copy(tag: &str) -> TempDir {
    let tmp = TempDir::new(&format!("awf-xtask-{tag}-")).expect("暫存目錄");
    copy_dir(&util::teams_dir(&root()), &util::teams_dir(tmp.path()));
    copy_dir(&util::snippet_dir(&root()), &util::snippet_dir(tmp.path()));
    tmp
}

/// 把檔案裡的 `from` 換成 `to`；`from` 一定要出現（否則測試沒有改到東西）。
fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("讀檔");
    assert!(text.contains(from), "{} 裡找不到要改的地方：{from}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).expect("寫檔");
}

fn has_node() -> bool {
    util::which("node").is_some()
}

// ------------------------------------------------------------------ check-recipes

#[test]
fn recipes_pass_on_repo() {
    assert!(has_node(), "這些檢查需要 node");
    let (rc, out, err) = check_recipes::run_captured(&root());
    assert_eq!(rc, 0, "{out}{err}");
    assert!(out.starts_with("recipe 一致性檢查通過：golden ") && out.contains(" 個案例 rs＝js"), "{out}");
    let n = std::fs::read_dir(util::teams_dir(&root()).join("golden-set/recipes")).expect("golden").flatten().count();
    assert!(out.contains(&format!("golden {n} 個案例")), "案例數要等於 golden 檔數 {n}：{out}");
}

#[test]
fn recipes_fail_when_js_drifts() {
    let tmp = teams_copy("recipes-js");
    // JS 那一邊的 modifier 不再加段（比 Rust 鬆）
    edit(&util::snippet_dir(tmp.path()).join("recipe.js"), "r.stages = recipeOrder(recipeUnion(r.stages, md.add_stages))", "r.stages = recipeOrder(r.stages)");
    let (rc, out, _) = check_recipes::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("recipe 一致性檢查失敗：\n") && out.contains("recipe.js 和 awf_core::recipe 不一致") && out.contains("\"stages\""), "{out}");
    assert!(!out.contains("selftest 失敗") && !out.contains("modifiers 和內建"), "只有 JS 那一項該失敗：{out}");
}

#[test]
fn recipes_fail_when_golden_expectation_changes() {
    let tmp = teams_copy("recipes-golden");
    edit(&util::teams_dir(tmp.path()).join("golden-set/recipes/03-bugfix.json"), "\"plan_mode\": \"diagnose\"", "\"plan_mode\": \"feature\"");
    let (rc, out, _) = check_recipes::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("selftest 失敗") && out.contains("03-bugfix"), "{out}");
}

#[test]
fn recipes_fail_when_builtin_profile_drifts() {
    let tmp = teams_copy("recipes-profile");
    let prof = util::teams_dir(tmp.path()).join("methodologies/lightweight.json");
    let mut d = util::read_json(&prof).expect("profile");
    d["modifiers"].as_object_mut().expect("modifiers").shift_remove("migration");
    d["recipes"]["bugfix"]["plan_mode"] = json!("feature");
    d["recipes"].as_object_mut().expect("recipes").shift_remove("revert");
    std::fs::write(&prof, d.to_string()).expect("write");
    let (rc, out, _) = check_recipes::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("lightweight.json：modifiers 和內建的 modifiers 不一致"), "{out}");
    assert!(out.contains("lightweight.json：recipe bugfix 和內建的 recipe 不一致"), "{out}");
    assert!(out.contains("lightweight.json：recipes 應有 ['bugfix', 'change',") && !out.contains("default.json"), "{out}");
}

// ------------------------------------------------------------------ check-kernel

#[test]
fn kernel_passes_on_repo() {
    assert!(has_node(), "這些檢查需要 node");
    let (rc, out, err) = check_kernel::run_captured(&root());
    assert_eq!((rc, out.as_str()), (0, "check_kernel：通過（label 去重、升級標對筆、digest、派工數上限、限制文件）\n"), "{err}");
}

#[test]
fn kernel_fails_without_agent_cap() {
    let tmp = teams_copy("kernel");
    // 拿掉派工數上限
    edit(&util::snippet_dir(tmp.path()).join("kernel.js"), "k.max = c.max_agents > 0 ? c.max_agents : 0;", "k.max = 0;");
    let (rc, out, _) = check_kernel::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("  [失敗] 上限沒擋住") && out.trim_end().ends_with("項失敗"), "{out}");
}

#[test]
fn kernel_fails_when_snippet_is_missing_or_broken() {
    let tmp = teams_copy("kernel-broken");
    std::fs::write(util::snippet_dir(tmp.path()).join("kernel.js"), "function awfKernelInit( {\n").expect("write");
    let (rc, out, _) = check_kernel::run_captured(tmp.path());
    assert_eq!(rc, 1);
    assert!(out.starts_with("check_kernel："), "{out}");
    std::fs::remove_file(util::snippet_dir(tmp.path()).join("kernel.js")).expect("rm");
    let (rc, out, _) = check_kernel::run_captured(tmp.path());
    assert_eq!(rc, 1);
    assert!(out.starts_with("check_kernel：node 執行失敗："), "{out}");
}

// ------------------------------------------------------------------ check-patterns

#[test]
fn patterns_pass_on_repo() {
    assert!(has_node(), "這些檢查需要 node");
    let (rc, out, err) = check_patterns::run_captured(&root());
    assert_eq!(
        (rc, out.as_str()),
        (0, "check_patterns：六個 pattern 的上限、正常、達上限、缺件，核心上限與角色白名單，以及自訂腳本端到端都正確\n"),
        "{err}"
    );
}

#[test]
fn patterns_fail_without_pattern_limit() {
    let tmp = teams_copy("patterns");
    // fan-out 超過上限時不再停下
    edit(&util::snippet_dir(tmp.path()).join("patterns.js"), "if (list.length > max) return awfPatternOver('fan-out', list.length, max, '派工單')", "");
    let (rc, out, _) = check_patterns::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("check_patterns：沒通過\n") && out.contains("fan-out"), "{out}");
}

#[test]
fn patterns_fail_when_kernel_cap_is_gone() {
    let tmp = teams_copy("patterns-e2e");
    // 執行核心不再擋派工數：自訂腳本端到端（上限 2）要抓到
    edit(&util::snippet_dir(tmp.path()).join("kernel.js"), "k.max = c.max_agents > 0 ? c.max_agents : 0;", "k.max = 0;");
    let (rc, out, _) = check_patterns::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.starts_with("check_patterns：自訂腳本端到端沒通過\n") && out.contains("自訂腳本（上限 2）halted 應該是 True") && out.contains("上限 2：只該派 2 個"), "{out}");
}

#[test]
fn patterns_fail_without_role_whitelist() {
    let tmp = teams_copy("patterns-roles");
    // 核心不再檢查角色白名單：pattern 的單元檢查要抓到
    let kernel = util::snippet_dir(tmp.path()).join("kernel.js");
    let text = std::fs::read_to_string(&kernel).expect("kernel.js");
    let line = text.lines().find(|l| l.contains("只能派登記過的角色")).expect("白名單那一行");
    std::fs::write(&kernel, text.replacen(line, "", 1)).expect("write");
    let (rc, out, _) = check_patterns::run_captured(tmp.path());
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("白名單以外的角色應該 throw") || out.contains("例外"), "{out}");
}

// ------------------------------------------------------------------ check-budget

#[test]
fn budget_on_repo_and_with_lowered_baseline() {
    assert!(has_node(), "這些檢查需要 node");
    let r = root();
    let cur = check_budget::measure(&r).expect("量得到");
    let real = util::teams_dir(&r).join("golden-set/token-budget.json");
    let base = util::read_json(&real).expect("基準");

    // 對這個 repo 的基準：通過
    let (ok, out) = check_budget::compare(&cur, &base).expect("格式正確");
    assert!(ok, "{out}");
    assert!(out.starts_with("各 recipe plan→deliver 派工數：feature ") && out.ends_with("token 預算檢查通過\n"), "{out}");
    assert_eq!(out.lines().count(), 3, "{out}");

    // 把一支 workflow 的字數基準調低、一個 recipe 的派工數基準調低：兩條都要報
    let mut low = base.clone();
    let have = cur["workflows"]["sdlc-plan.js"]["prompt_chars"].as_u64().expect("字數");
    let agents = cur["recipes"]["bugfix"]["agents_plan_to_deliver"].as_u64().expect("派工數");
    low["workflows"]["sdlc-plan.js"]["prompt_chars"] = json!(have / 2);
    low["recipes"]["bugfix"]["agents_plan_to_deliver"] = json!(agents - 2);
    let tmp = TempDir::new("awf-xtask-budget-").expect("暫存目錄");
    let file = tmp.path().join("low.json");
    std::fs::write(&file, low.to_string()).expect("write");
    let args = vec!["--budget".to_string(), file.to_string_lossy().into_owned()];
    let (rc, out, err) = check_budget::run_captured(&r, &args);
    assert_eq!(rc, 1, "{out}{err}");
    assert!(out.contains(&format!("  - sdlc-plan.js：prompt_chars {have}，超過基準 {} 的 10%\n", have / 2)), "{out}");
    assert!(out.contains(&format!("  - recipe bugfix：plan→deliver 派 {agents} 個 agent，基準 {}\n", agents - 2)), "{out}");
    assert!(out.contains("token 預算超標：\n") && !out.contains("token 預算檢查通過"), "{out}");

    // --update 寫出來的檔：再比一次要通過，格式是 indent=1、結尾一個換行
    let fresh = tmp.path().join("fresh.json");
    let args = vec!["--update".to_string(), format!("--budget={}", fresh.display())];
    let (rc, out, _) = check_budget::run_captured(&r, &args);
    assert_eq!(rc, 0, "{out}");
    let text = std::fs::read_to_string(&fresh).expect("基準");
    assert!(text.starts_with("{\n \"workflows\": {\n  \"event-brief.js\": {\n   \"agents\": ") && text.ends_with("\n}\n"), "{}", text.chars().take(120).collect::<String>());
    assert_eq!(serde_json::from_str::<Value>(&text).expect("JSON"), cur);

    // 沒有基準檔：失敗
    let args = vec!["--budget".to_string(), tmp.path().join("none.json").to_string_lossy().into_owned()];
    let (rc, out, _) = check_budget::run_captured(&r, &args);
    assert_eq!(rc, 1);
    assert!(out.starts_with("沒有基準 "), "{out}");
}

#[test]
fn budget_compare_rules() {
    let cur = json!({
        "workflows": {"a.js": {"agents": 11, "prompt_chars": 1100, "shared_prefix_chars": 5}, "b.js": {"agents": 1, "prompt_chars": 10, "shared_prefix_chars": 0}},
        "recipes": {"feature": {"agents_plan_to_deliver": 11}, "docs": {"agents_plan_to_deliver": null}, "spike": {"agents_plan_to_deliver": 0}},
    });
    // 剛好 110%：不算超過（和 Python 一樣用浮點數比，10 * 1.1 = 11.000000000000002）
    let base = json!({"workflows": {"a.js": {"agents": 10, "prompt_chars": 1000}}, "recipes": {"feature": {"agents_plan_to_deliver": 10}}});
    let (ok, out) = check_budget::compare(&cur, &base).expect("ok");
    assert!(ok, "{out}");
    assert!(out.starts_with("各 recipe plan→deliver 派工數：feature 11、docs None、spike 0\n（feature＝11；"), "{out}");
    // 超過 10%、workflow 不見了、量不到的 recipe、基準沒有數字的 recipe（略過）
    let base = json!({
        "workflows": {"a.js": {"agents": 9, "prompt_chars": 999}, "gone.js": {"agents": 1, "prompt_chars": 1}},
        "recipes": {"feature": {"agents_plan_to_deliver": 9}, "docs": {"agents_plan_to_deliver": 3}, "spike": {"agents_plan_to_deliver": null}, "revert": {"agents_plan_to_deliver": 2}},
    });
    let (ok, out) = check_budget::compare(&cur, &base).expect("ok");
    assert!(!ok);
    for want in [
        "  - a.js：agents 11，超過基準 9 的 10%\n",
        "  - a.js：prompt_chars 1100，超過基準 999 的 10%\n",
        "  - gone.js：量不到（workflow 不見了？）\n",
        "  - recipe feature：plan→deliver 派 11 個 agent，基準 9\n",
        "  - recipe docs：plan→deliver 派 None 個 agent，基準 3\n",
        "  - recipe revert：plan→deliver 派 None 個 agent，基準 2\n",
    ] {
        assert!(out.contains(want), "少了「{want}」：{out}");
    }
    assert_eq!(out.matches("  - ").count(), 6, "{out}");
    // 格式不對：不能當成通過
    assert!(check_budget::compare(&cur, &json!({"workflows": {"a.js": {"agents": "x", "prompt_chars": 1}}})).is_err());
    assert!(check_budget::compare(&cur, &json!({"workflows": {"a.js": {"agents": 1}}})).is_err());
    assert!(check_budget::compare(&cur, &json!({"recipes": {"feature": 3}})).is_err());
    assert!(check_budget::compare(&cur, &json!({"workflows": []})).is_err());
}

#[test]
fn budget_summarize_counts_code_points() {
    let data = json!({
        "sdlc-plan.js": {"scenarios": [
            {"name": "預設", "labels": ["x"], "prompts": {"x": "不量這個"}},
            {"name": "recipe:feature", "labels": ["a", "b"], "prompts": {"a": "共同前綴：甲", "b": "共同前綴：乙丙"}},
        ]},
        "event.js": {"scenarios": [
            {"name": "預設", "labels": ["w"], "prompts": {"w": "單張"}},
            {"name": "類型：meeting", "labels": ["p", "q", "r"], "prompts": {"p": "同A", "q": "同B", "r": "同"}},
        ]},
    });
    let got = check_budget::summarize(&data).expect("ok");
    assert_eq!(got["workflows"]["sdlc-plan.js"], json!({"agents": 2, "prompt_chars": 13, "shared_prefix_chars": 5}));
    assert_eq!(got["workflows"]["event.js"], json!({"agents": 1, "prompt_chars": 2, "shared_prefix_chars": 0}));
    assert_eq!(got["workflows"]["event.js#meeting"], json!({"agents": 3, "prompt_chars": 5, "shared_prefix_chars": 1}));
    let order: Vec<&String> = got["workflows"].as_object().expect("物件").keys().collect();
    assert_eq!(order, ["event.js", "event.js#meeting", "sdlc-plan.js"]);
    // 少了別段的情境：recipe 的總數是 null（量不到），不是只算有的那幾段
    assert_eq!(got["recipes"]["feature"], json!({"agents_plan_to_deliver": null}));
    assert!(check_budget::summarize(&json!({"a.js": {"scenarios": []}})).is_err());
    assert!(check_budget::summarize(&json!({"a.js": {"scenarios": [{"name": "x", "labels": [], "prompts": {"a": 1}}]}})).is_err());
}

// ------------------------------------------------------------------ check-supply

fn write(base: &Path, rel: &str, text: &str) {
    let p = base.join(rel);
    std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(st.status.success(), "git {args:?}：{}", String::from_utf8_lossy(&st.stderr));
}

const PASSING: &str = "[通過] action 釘 SHA\n[通過] 版控沒有快取、.env、私鑰\n[通過] Cargo.lock 在版控裡\n\
[通過] Rust 相依都來自 crates.io\n[通過] Rust 相依和允許清單一致\n[通過] Node 腳本只用內建模組\n\n供應鏈檢查通過\n";
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// 迷你 workspace：一個成員 demo、一個來自 crates.io 的相依 serde，允許清單只有 serde。
fn write_cargo(d: &Path) {
    write(d, "Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"crates/demo\"]\n");
    write(d, "crates/demo/Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n");
    write(d, "Cargo.lock", &format!("version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\ndependencies = [\n \"serde\",\n]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\nchecksum = \"00\"\n"));
    write(d, "crates/xtask/allowed-crates.txt", "# 說明\n\nserde\n");
    write(d, "plugin/workflow-templates/check_workflows.mjs", "import { readFileSync } from 'node:fs'\nimport {\n  join,\n} from 'path'\nconst s = await import(somewhere)\n// 字串裡的 from 'left-pad' 不算\n");
    write(d, "plugin/workflow-templates/scenarios/a.mjs", "import { X } from './_helpers.mjs'\nexport default []\n");
    write(d, "crates/xtask/src/js/t.cjs", "const fs = require('fs')\n");
}

#[test]
fn supply_passes_on_repo() {
    let (rc, out, err) = check_supply::run_captured(&root());
    assert_eq!((rc, out.as_str()), (0, PASSING), "{err}");
}

#[test]
fn supply_catches_each_kind() {
    let tmp = TempDir::new("awf-xtask-supply-").expect("暫存目錄");
    let d = tmp.path();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    write(d, ".github/workflows/ci.yml", &format!("jobs:\n  a:\n    steps:\n      - uses: actions/checkout@{sha} # v4\n      - uses: ./local\n"));
    write(d, "plugin/scaffold/.github/workflows/gate.yml.tmpl", &format!("steps:\n  - name: x\n    uses: \"actions/setup-node@{sha}\"\n"));
    write(d, ".env.example", "A=1\n");
    write(d, "README.md", "乾淨\n");
    write_cargo(d);
    git(d, &["init", "-q"]);
    git(d, &["add", "-A"]);
    let (rc, out, err) = check_supply::run_captured(d);
    assert_eq!((rc, out.as_str()), (0, PASSING), "{err}");

    // 逐類弄壞
    write(d, ".github/workflows/ci.yml", "jobs:\n  a:\n    steps:\n      - uses: actions/checkout@v4\n      - uses: docker://alpine:3\n");
    write(d, "plugin/scaffold/.github/workflows/gate.yml.tmpl", "steps:\n  - name: x\n    uses: 'actions/setup-node@0123456'\n");
    write(d, ".env", "A=1\n");
    write(d, "conf/.env.local", "A=1\n");
    write(d, "keys/id_rsa", "x\n");
    write(d, "keys/Server.PEM", "x\n");
    write(d, "pkg/__pycache__/a.txt", "x\n");
    write(d, "pkg/b.pyc", "x\n");
    // 私鑰標頭在執行時組出來，不以完整形狀寫在檔案裡
    let dashes = "-".repeat(5);
    write(d, "notes.txt", &format!("前面\n{dashes}{} RSA {} {}{dashes}\n後面\n", "BEGIN", "PRIVATE", "KEY"));
    // Rust 相依：git 來源、workspace 以外的 path 相依、清單沒有的 crate、清單多出來的名字
    let lock = std::fs::read_to_string(d.join("Cargo.lock")).expect("Cargo.lock");
    write(d, "Cargo.lock", &format!(
        "{lock}\n[[package]]\nname = \"evil\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/evil#abc\"\n\n[[package]]\nname = \"local-dep\"\nversion = \"0.1.0\"\n\n\
         [[package]]\nname = \"extra\"\nversion = \"2.0.0\"\nsource = \"{CRATES_IO}\"\n"
    ));
    write(d, "crates/xtask/allowed-crates.txt", "serde\ngone\n");
    // Node：外部套件、網路模組
    write(d, "plugin/workflow-templates/scenarios/a.mjs", "import axios from 'axios'\nexport { y } from \"../up.mjs\"\n");
    write(d, "crates/xtask/src/js/t.cjs", "const fs = require('fs')\nconst h = require('node:https')\n");
    git(d, &["add", "-A", "-f"]);
    let (rc, out, err) = check_supply::run_captured(d);
    assert_eq!(rc, 1, "{out}{err}");
    for want in [
        "[失敗] action 釘 SHA\n",
        "  - .github/workflows/ci.yml:4：action 沒有釘 40 字元 commit SHA（actions/checkout@v4）\n",
        "  - plugin/scaffold/.github/workflows/gate.yml.tmpl:3：action 沒有釘 40 字元 commit SHA（actions/setup-node@0123456）\n",
        "[失敗] 版控沒有快取、.env、私鑰\n",
        "  - .env：版控裡不該有 .env 檔\n",
        "  - conf/.env.local：版控裡不該有 .env 檔\n",
        "  - keys/id_rsa：版控裡不該有私鑰形狀的檔案\n",
        "  - keys/Server.PEM：版控裡不該有私鑰形狀的檔案\n",
        "  - pkg/__pycache__/a.txt：版控裡不該有 Python 快取檔\n",
        "  - pkg/b.pyc：版控裡不該有 Python 快取檔\n",
        "  - notes.txt：內容含私鑰\n",
        "[通過] Cargo.lock 在版控裡\n",
        "[失敗] Rust 相依都來自 crates.io\n",
        "  - evil：來源不是 crates.io（git+https://example.com/evil#abc）\n",
        "  - local-dep：沒有來源（workspace 以外的 path 相依）\n",
        "[失敗] Rust 相依和允許清單一致\n",
        "  - evil：Cargo.lock 有、允許清單沒有（確認要用它之後加進 crates/xtask/allowed-crates.txt）\n",
        "  - extra：Cargo.lock 有、允許清單沒有（確認要用它之後加進 crates/xtask/allowed-crates.txt）\n",
        "  - gone：允許清單有、Cargo.lock 沒有（已經不用就從 crates/xtask/allowed-crates.txt 拿掉）\n",
        "[失敗] Node 腳本只用內建模組\n",
        "  - plugin/workflow-templates/scenarios/a.mjs:1：只能 import 內建模組（不含網路）或相對路徑（axios）\n",
        "  - crates/xtask/src/js/t.cjs:2：只能 import 內建模組（不含網路）或相對路徑（node:https）\n",
    ] {
        assert!(out.contains(want), "少了「{want}」：\n{out}");
    }
    assert_eq!(out.matches("  - ").count(), 16, "{out}");
    assert!(out.ends_with("\n供應鏈檢查失敗：16 條\n"), "{out}");
    assert!(!out.contains(".env.example") && !out.contains("docker://"), "{out}");
}

/// Cargo.lock 沒進版控、不存在、workspace 成員沒跟上都要失敗。
#[test]
fn supply_checks_rust_side() {
    let tmp = TempDir::new("awf-xtask-supply-rs-").expect("暫存目錄");
    let d = tmp.path();
    write_cargo(d);
    write(d, "README.md", "乾淨\n");
    git(d, &["init", "-q"]);
    git(d, &["add", "-A"]);
    let (rc, out, err) = check_supply::run_captured(d);
    assert_eq!((rc, out.as_str()), (0, PASSING), "{err}");

    git(d, &["rm", "-q", "--cached", "Cargo.lock"]);
    let (rc, out, _) = check_supply::run_captured(d);
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("[失敗] Cargo.lock 在版控裡\n  - Cargo.lock 不在版控裡（git ls-files 沒有它）\n"), "{out}");
    assert!(out.ends_with("\n供應鏈檢查失敗：1 條\n"), "{out}");

    git(d, &["add", "Cargo.lock"]);
    write(d, "Cargo.toml", "[workspace]\nmembers = [\n  \"crates/demo\",\n  \"crates/other\",\n]\n");
    write(d, "crates/other/Cargo.toml", "[package]\nname = \"other\"\n");
    let (rc, out, _) = check_supply::run_captured(d);
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("  - other：workspace 成員不在 Cargo.lock 裡（Cargo.lock 沒有跟上）\n"), "{out}");

    std::fs::remove_file(d.join("Cargo.lock")).expect("rm");
    let (rc, out, _) = check_supply::run_captured(d);
    assert_eq!(rc, 1, "{out}");
    for want in ["  - Cargo.lock 不存在\n", "[失敗] Rust 相依都來自 crates.io\n  - 讀不到 Cargo.lock\n", "[失敗] Rust 相依和允許清單一致\n  - 讀不到 Cargo.lock\n"] {
        assert!(out.contains(want), "少了「{want}」：\n{out}");
    }
}

#[test]
fn supply_fails_outside_git() {
    let tmp = TempDir::new("awf-xtask-supply-nogit-").expect("暫存目錄");
    write(tmp.path(), "README.md", "乾淨\n");
    // 上層剛好是 git repo 時 ls-files 會成功；用 GIT_CEILING_DIRECTORIES 擋不了子程序的環境，所以只在確定不在 repo 裡時才驗
    let inside = Command::new("git").args(["rev-parse", "--is-inside-work-tree"]).current_dir(tmp.path()).output().map(|o| o.status.success()).unwrap_or(false);
    if inside {
        return;
    }
    let (rc, out, _) = check_supply::run_captured(tmp.path());
    assert_eq!(rc, 1);
    assert!(out.contains("[失敗] 版控沒有快取、.env、私鑰\n  - git ls-files 失敗："), "{out}");
}

/// 自訂腳本的組裝結果沒有走樣：骨架（狀態機接上去的那一段與外層包裝；片段、profile、schema 的內文換成記號，
/// 見 `check_patterns::script_frame`）和固定的預期檔逐字相同。
///
/// 預期檔的來歷：0.x 的 Python 版還在的時候，這個測試直接拿 `awf.py` 的 `render_custom` 對照，全文逐字相同（只差標題列的
/// 「awf.py」改成「awf」）；預期檔就是那一次的結果取骨架存下來的。之後行為確實該變時用
/// `AWF_UPDATE_EXPECTED=1 cargo test -p xtask --test checks custom_script` 重寫，並在 commit 說明哪裡變了、為什麼。
#[test]
fn custom_script_assembly_matches_recorded_frame() {
    let r = root();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let update = std::env::var_os("AWF_UPDATE_EXPECTED").is_some_and(|v| !v.is_empty());
    for (key, cap) in [("None", None), ("2", Some(2))] {
        let full = check_patterns::sample_script(&r, cap).expect("組得起來");
        assert!(full.len() > 20_000, "腳本太短：{}", full.len());
        let mine = check_patterns::script_frame(&r, cap).expect("取得到骨架");
        assert!(mine.matches("// ── 以下由 awf 接上（").count() == 1, "標題列應該剛好一處");
        let file = dir.join(format!("custom-script-frame.{key}.js"));
        if update {
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(&file, &mine).expect("寫預期檔");
            continue;
        }
        let want = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("讀不到預期檔 {}：{e}", file.display()));
        assert!(mine == want, "上限 {key}：組裝結果的骨架和預期檔 {} 不同\n--- 現在 ---\n{mine}\n--- 預期 ---\n{want}", file.display());
    }
}

// ------------------------------------------------------------------ golden

#[test]
fn golden_passes_on_repo() {
    let (code, out, _) = xtask::golden::run_captured(&root(), &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("失敗 0"), "{out}");
}

#[test]
fn golden_fails_when_an_invalid_case_is_filed_as_valid() {
    let tmp = teams_copy("golden-swap");
    let set = util::teams_dir(tmp.path()).join("golden-set");
    let bad = std::fs::read_dir(set.join("invalid")).expect("invalid").flatten().map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .expect("至少一個反例");
    std::fs::rename(&bad, set.join("valid").join("zz-moved.json")).expect("搬檔");
    let (code, out, _) = xtask::golden::run_captured(tmp.path(), &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("FAIL valid/zz-moved.json"), "{out}");
}

#[test]
fn golden_fails_when_a_semantic_case_loses_its_mark() {
    let tmp = teams_copy("golden-semantic");
    let dir = util::teams_dir(tmp.path()).join("golden-set").join("invalid");
    let mark = "\"semantic\": true";
    let file = std::fs::read_dir(&dir).expect("invalid").flatten().map(|e| e.path())
        .find(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains(mark)))
        .expect("至少一個語意反例");
    edit(&file, mark, "\"semantic\": false");
    let (code, out, _) = xtask::golden::run_captured(tmp.path(), &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("schema 應該擋但通過"), "{out}");
}

#[test]
fn golden_fails_on_empty_set() {
    let tmp = TempDir::new("awf-xtask-golden-empty-").expect("暫存目錄");
    let (code, out, _) = xtask::golden::run_captured(tmp.path(), &[]);
    assert_eq!(code, 1, "{out}");
}
