//! check-recipes、check-kernel、check-patterns、check-budget、check-supply 的測試。
//!
//! 每支檢查兩種：對這個 repo 實跑要通過；在暫存副本裡弄壞一個輸入（golden 的預期、片段裡的上限檢查、基準數字、
//! 版控裡的檔案）要報失敗、exit 非零，而且訊息指得出是哪裡。
//! 自訂腳本的組裝結果和固定的預期檔比（`tests/fixtures/custom-script-frame.*.js`）。
//! 另有一個和 Python 版直接對照的測試（供應鏈檢查的 import 清單）：Python 檔還在的期間才跑，
//! 找不到 python3 或對應的 Python 檔就略過；Python 移除後這個測試與它掃的東西一起沒有了。
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

/// 暫存的 repo 根目錄，裡面只有 `scaffold/.claude/teams`（片段、schema、golden-set、內建 profile）。
fn teams_copy(tag: &str) -> TempDir {
    let tmp = TempDir::new(&format!("awf-xtask-{tag}-")).expect("暫存目錄");
    copy_dir(&util::teams_dir(&root()), &util::teams_dir(tmp.path()));
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

const PASSING: &str = "[通過] 只用標準庫\n[通過] plugin 不連網\n[通過] action 釘 SHA\n[通過] 版控沒有快取、.env、私鑰\n\n供應鏈檢查通過\n";

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
    write(d, "init.py", "import os\nimport helper\nfrom schemalib import x\n");
    write(d, "helper.py", "import json\n");
    write(d, "plugin/hooks/guard.py", "import re, json\nfrom pathlib import Path\n");
    write(d, "plugin/skills/demo/tool.py", "import json\n");
    write(d, "scaffold/.claude/teams/tools/schemalib.py", "import json\nfrom . import sibling\n");
    write(d, ".github/workflows/ci.yml", &format!("jobs:\n  a:\n    steps:\n      - uses: actions/checkout@{sha} # v4\n      - uses: ./local\n"));
    write(d, "scaffold/.github/workflows/gate.yml.tmpl", &format!("steps:\n  - name: x\n    uses: \"actions/setup-node@{sha}\"\n"));
    write(d, ".env.example", "A=1\n");
    write(d, "README.md", "乾淨\n");
    git(d, &["init", "-q"]);
    git(d, &["add", "-A"]);
    let (rc, out, err) = check_supply::run_captured(d);
    assert_eq!((rc, out.as_str()), (0, PASSING), "{err}");

    // 逐類弄壞
    write(d, "init.py", "import os\nimport requests\ntry:\n    import yaml.parser as yp\nexcept ImportError:\n    pass\n");
    write(d, "scaffold/.claude/teams/tools/schemalib.py", "import importlib\nm = importlib.import_module('jsonschema')\n");
    write(d, "plugin/hooks/guard.py", "import re\nfrom http import client\n");
    write(d, "plugin/skills/demo/tool.py", "def f():\n    import urllib.request\n    return __import__('socket')\n");
    write(d, "plugin/skills/demo/broken.py", "x = 'abc\n");
    write(d, ".github/workflows/ci.yml", "jobs:\n  a:\n    steps:\n      - uses: actions/checkout@v4\n      - uses: docker://alpine:3\n");
    write(d, "scaffold/.github/workflows/gate.yml.tmpl", "steps:\n  - name: x\n    uses: 'actions/setup-node@0123456'\n");
    write(d, ".env", "A=1\n");
    write(d, "conf/.env.local", "A=1\n");
    write(d, "keys/id_rsa", "x\n");
    write(d, "keys/Server.PEM", "x\n");
    write(d, "pkg/__pycache__/a.txt", "x\n");
    write(d, "pkg/b.pyc", "x\n");
    // 私鑰標頭在執行時組出來，不以完整形狀寫在檔案裡
    let dashes = "-".repeat(5);
    write(d, "notes.txt", &format!("前面\n{dashes}{} RSA {} {}{dashes}\n後面\n", "BEGIN", "PRIVATE", "KEY"));
    git(d, &["add", "-A", "-f"]);
    let (rc, out, err) = check_supply::run_captured(d);
    assert_eq!(rc, 1, "{out}{err}");
    for want in [
        "[失敗] 只用標準庫\n",
        "  - scaffold/.claude/teams/tools/schemalib.py:2：import 了非標準庫模組 jsonschema\n",
        "  - init.py:2：import 了非標準庫模組 requests\n",
        "  - init.py:4：import 了非標準庫模組 yaml.parser\n",
        "[失敗] plugin 不連網\n",
        "  - plugin/hooks/guard.py:2：plugin 不得有網路呼叫（import http.client）\n",
        "  - plugin/skills/demo/broken.py：無法解析（",
        "  - plugin/skills/demo/tool.py:2：plugin 不得有網路呼叫（import urllib.request）\n",
        "  - plugin/skills/demo/tool.py:3：plugin 不得有網路呼叫（import socket）\n",
        "[失敗] action 釘 SHA\n",
        "  - .github/workflows/ci.yml:4：action 沒有釘 40 字元 commit SHA（actions/checkout@v4）\n",
        "  - scaffold/.github/workflows/gate.yml.tmpl:3：action 沒有釘 40 字元 commit SHA（actions/setup-node@0123456）\n",
        "[失敗] 版控沒有快取、.env、私鑰\n",
        "  - .env：版控裡不該有 .env 檔\n",
        "  - conf/.env.local：版控裡不該有 .env 檔\n",
        "  - keys/id_rsa：版控裡不該有私鑰形狀的檔案\n",
        "  - keys/Server.PEM：版控裡不該有私鑰形狀的檔案\n",
        "  - pkg/__pycache__/a.txt：版控裡不該有 Python 快取檔\n",
        "  - pkg/b.pyc：版控裡不該有 Python 快取檔\n",
        "  - notes.txt：內容含私鑰\n",
    ] {
        assert!(out.contains(want), "少了「{want}」：\n{out}");
    }
    assert_eq!(out.matches("  - ").count(), 16, "{out}");
    assert!(out.ends_with("\n供應鏈檢查失敗：16 條\n"), "{out}");
    assert!(!out.contains(".env.example") && !out.contains("docker://") && !out.contains("helper"), "{out}");
}

#[test]
fn supply_fails_outside_git() {
    let tmp = TempDir::new("awf-xtask-supply-nogit-").expect("暫存目錄");
    write(tmp.path(), "init.py", "import os\n");
    // 上層剛好是 git repo 時 ls-files 會成功；用 GIT_CEILING_DIRECTORIES 擋不了子程序的環境，所以只在確定不在 repo 裡時才驗
    let inside = Command::new("git").args(["rev-parse", "--is-inside-work-tree"]).current_dir(tmp.path()).output().map(|o| o.status.success()).unwrap_or(false);
    if inside {
        return;
    }
    let (rc, out, _) = check_supply::run_captured(tmp.path());
    assert_eq!(rc, 1);
    assert!(out.contains("[失敗] 版控沒有快取、.env、私鑰\n  - git ls-files 失敗："), "{out}");
}

// ------------------------------------------------------------------ 和 Python 版直接對照（Python 版還在的期間）

/// 跑一段 Python，回 stdout 的 JSON；沒有 python3 回 None。
fn python_json(code: &str, args: &[&Path]) -> Option<Value> {
    let py = util::which("python3")?;
    let out = Command::new(py).arg("-c").arg(code).args(args).env("PYTHONUTF8", "1").output().ok()?;
    assert!(out.status.success(), "Python 那一邊失敗：{}", String::from_utf8_lossy(&out.stderr));
    Some(serde_json::from_slice(&out.stdout).expect("Python 那一邊的 JSON"))
}

/// 供應鏈檢查看的每個 Python 檔：Rust 找到的 import 和 Python `ast` 找到的相同（行號與模組名）。
#[test]
fn supply_imports_match_python_ast() {
    let r = root();
    if !r.join("tests/check_supply.py").is_file() {
        eprintln!("略過：Python 版已經不在");
        return;
    }
    let code = "import sys, json\nsys.path.insert(0, sys.argv[1] + '/tests')\nimport check_supply as c\n\
                print(json.dumps({c.rel(p): sorted(c.imports(p)) for p in c.files(c.STDLIB_ONLY_GLOBS) + c.files(['plugin/**/*.py'])}))";
    let Some(py) = python_json(code, &[&r]) else {
        eprintln!("略過：找不到 python3");
        return;
    };
    let py = py.as_object().expect("物件");
    let files = check_supply::python_files(&r);
    let mut theirs: Vec<&String> = py.keys().collect();
    let mut mine: Vec<&String> = files.iter().collect();
    theirs.sort();
    mine.sort();
    assert_eq!(mine, theirs, "要看的檔案清單不同");
    let mut total = 0;
    for f in &files {
        let mut got = check_supply::imports_in(&r.join(f)).unwrap_or_else(|e| panic!("{f}：{e}"));
        got.sort();
        let got: Vec<Value> = got.into_iter().map(|(l, m)| json!([l, m])).collect();
        total += got.len();
        assert_eq!(Value::Array(got), py[f.as_str()], "{f} 的 import 不同");
    }
    assert!(total > 300, "import 太少，可能沒掃到：{total}");
}

/// 自訂腳本的組裝結果沒有走樣：骨架（狀態機接上去的那一段與外層包裝；片段、profile、schema 的內文換成記號，
/// 見 `check_patterns::script_frame`）和固定的預期檔逐字相同。
///
/// 預期檔的來歷：Python 版還在的時候，這個測試直接拿 `awf.py` 的 `render_custom` 對照，全文逐字相同（只差標題列的
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
