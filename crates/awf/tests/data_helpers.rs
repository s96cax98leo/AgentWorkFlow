//! 資料助手（1.2.2）：`awf-lookup`（查詢員）與 `awf-refresh`（更新員）的守門與交件。
//!
//! - 兩個都被固定當成唯讀角色：寫入、核准、推送、狀態機都擋；設定檔改不掉。
//! - 更新員只多放行「整條指令逐字相符」的幾個更新指令；夾帶任何東西都擋。
//! - 都不交信封。
//! - PowerShell 算出來的指令名（`& (…)`、`iex $x`）對模版角色一律擋。
//!
//! 每一列 Bash 與 PowerShell 各跑一次。加放行的更新指令時，在 `REFRESH_OK` 加該放、在 `REFRESH_BAD` 加夾帶的寫法。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const LOOKUP: &str = "agent-workflow:awf-lookup";
const REFRESH: &str = "agent-workflow:awf-refresh";
const RES: &str = "agent-workflow:tpm-researcher";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_dir(&src, &dst) } else { std::fs::copy(&src, &dst).map(|_| ()).unwrap() }
    }
}

/// 已套用的專案。`extra` 併進 guard.config.json。
fn workspace(tag: &str, extra: Value) -> PathBuf {
    let d = std::env::temp_dir().join(format!("awf-helpers-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("_tpm/runs/r1")).unwrap();
    std::fs::create_dir_all(d.join("docs")).unwrap();
    let d = d.canonicalize().unwrap();
    std::fs::create_dir_all(d.join(".claude/teams")).unwrap();
    std::fs::write(d.join(".claude/teams/VERSION"), "1.2.2\n").unwrap();
    copy_dir(&repo().join("plugin/scaffold/.claude/teams/schemas"), &d.join(".claude/teams/schemas"));
    let mut cfg = json!({"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "writer_dirs": ["_tpm/runs", "docs"]});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        cfg[k] = v;
    }
    std::fs::write(d.join(".claude/teams/guard.config.json"), cfg.to_string()).unwrap();
    d
}

fn hook(ws: &Path, mode: &str, data: &Value) -> i32 {
    let mut c = Command::new(EXE);
    c.args(["hook", mode]).current_dir(ws);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    c.env("CLAUDE_PROJECT_DIR", ws);
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(data.to_string().as_bytes()).unwrap();
    child.wait().unwrap().code().unwrap_or(-1)
}

fn shell(ws: &Path, tool: &str, agent: &str, cmd: &str) -> i32 {
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": ws.to_string_lossy(), "session_id": "helpers123", "agent_id": "a1", "tool_input": {"command": cmd}});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    hook(ws, "guard-shell", &d)
}

/// 每一列在 Bash 與 PowerShell 各跑一次，回不符預期的說明。
fn expect(ws: &Path, agent: &str, cmds: &[&str], want: i32, bad: &mut Vec<String>) {
    for tool in ["Bash", "PowerShell"] {
        for c in cmds {
            let rc = shell(ws, tool, agent, c);
            if rc != want {
                bad.push(format!("{agent} {tool} {c:?}：rc={rc}，預期 {want}"));
            }
        }
    }
}

/// 唯讀的查詢：兩個資料助手都能跑。
const READS: &[&str] = &[
    "awf kb query 發票", "awf kg locate 匯出", "awf kg impact src/lib.rs", "awf locate 匯出", "awf save-run status _tpm/runs/r1 --json",
    "awf watch --run _tpm/runs/r1 --once", "awf doctor", "awf program status", "awf deliverables status", "awf deliverables check",
    "awf cost --project .", "awf spec status", "awf kg check", "git status", "git log --oneline -5",
];

/// 會改狀態的：兩個資料助手都不能跑。
const WRITES: &[&str] = &[
    "awf approve --run _tpm/runs/r1 plan --by me", "awf next --run _tpm/runs/r1", "awf done --run _tpm/runs/r1",
    "awf save-run close _tpm/runs/r1 --by x --at t", "awf publish push --repo-dir . --branch f", "awf init --target . --upgrade",
    "awf deliverables configure --write --by x", "awf deliverables sync --run _tpm/runs/r1", "awf deliverables init", "awf build",
    "awf pack plan --run _tpm/runs/r1", "awf kg export --out x", "git push origin main", "gh pr merge 1", "echo x > docs/a.md",
];

const REFRESH_OK: &[&str] = &["awf kb build", "awf kg build", "awf kg build --full", "awf kg build --timeout 600", "awf kg build --full --timeout 900", "awf deliverables index"];

/// 更新指令夾帶別的東西：都要擋。
const REFRESH_BAD: &[&str] = &[
    "awf kb build && awf approve --run r plan --by me", "awf kb build; rm -rf docs", "awf kb build | tee x", "awf  kb build", "awf kb build ",
    " awf kb build", "awf kb build\nawf approve --run r plan --by x", "awf kb build\n", "awf kb build --dir /tmp", "awf kg build --timeout 99999",
    "awf kg build --timeout 600 --full", "X=1 awf kb build", "./awf kb build", "/tmp/awf kb build", "awf kb build #x", "awf kb build > f",
    "awf kb build &", "AWF KB BUILD", "awf deliverables index --out /tmp/x",
];

#[test]
fn lookup_is_read_only() {
    let ws = workspace("lookup", json!({}));
    let mut bad = Vec::new();
    for agent in [LOOKUP, REFRESH, "awf-lookup", "awf-refresh"] {
        expect(&ws, agent, READS, 0, &mut bad);
        expect(&ws, agent, WRITES, 2, &mut bad);
    }
    // 查詢員、其他模版角色都不能跑更新指令
    expect(&ws, LOOKUP, REFRESH_OK, 2, &mut bad);
    expect(&ws, RES, REFRESH_OK, 2, &mut bad);
    // 沒有 Edit／Write 的工具；就算有，寫專案的檔與 run.json 也擋
    for agent in [LOOKUP, REFRESH] {
        for (tool, file) in [("Write", "src/x.rs"), ("Edit", "_tpm/runs/r1/run.json"), ("Write", "docs/a.md")] {
            let d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": ws.to_string_lossy(), "session_id": "helpers123", "agent_type": agent,
                "tool_input": {"file_path": ws.join(file).to_string_lossy(), "content": "x", "old_string": "a", "new_string": "b"}});
            let rc = hook(&ws, "guard-paths", &d);
            if rc != 2 {
                bad.push(format!("{agent} {tool} {file}：rc={rc}，預期 2"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&ws);
    assert!(bad.is_empty(), "不符預期：\n{}", bad.join("\n"));
}

#[test]
fn refresh_runs_only_exact_commands() {
    let ws = workspace("refresh", json!({}));
    let mut bad = Vec::new();
    expect(&ws, REFRESH, REFRESH_OK, 0, &mut bad);
    expect(&ws, REFRESH, REFRESH_BAD, 2, &mut bad);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(bad.is_empty(), "不符預期：\n{}", bad.join("\n"));
}

#[test]
fn config_cannot_loosen_helpers() {
    // 設定檔把資料助手對應到會寫程式的角色、或把命名空間拿掉：仍然照唯讀角色擋
    let ws = workspace("cfg", json!({"extension_agents": {LOOKUP: "dev-implementer", REFRESH: "dev-integrator"}, "team_agent_namespaces": ["other:"]}));
    let mut bad = Vec::new();
    for agent in ["other:awf-lookup", "other:awf-refresh", "awf-lookup", "awf-refresh"] {
        expect(&ws, agent, WRITES, 2, &mut bad);
    }
    // 別的 plugin 的同名 agent 不是模版的資料助手：不套角色規則，也沒有更新員的放行可言（它本來就不受限）
    assert_eq!(shell(&ws, "Bash", "someone-else:awf-refresh", "awf kb build"), 0);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(bad.is_empty(), "不符預期：\n{}", bad.join("\n"));
}

#[test]
fn helpers_do_not_hand_in_envelopes() {
    let ws = workspace("env", json!({}));
    let stop = |agent: &str| hook(&ws, "envelope", &json!({"hook_event_name": "SubagentStop", "agent_type": agent, "last_assistant_message": "結論：查到三處，出處如下。"}));
    assert_eq!(stop(LOOKUP), 0, "查詢員以文字回報，不交信封");
    assert_eq!(stop(REFRESH), 0, "更新員以文字回報，不交信封");
    assert_eq!(stop(RES), 2, "其他模版角色照舊要交信封");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn powershell_computed_command_names_are_blocked() {
    let ws = workspace("ps", json!({}));
    let mut bad = Vec::new();
    let blocked = [
        "& (echo git) push origin main", "& (echo gh) pr merge 3", "& (echo awf) approve --run r plan --by me", "git status; & $(echo git) push origin main",
        ". (Join-Path $PSScriptRoot x.ps1)", "Invoke-Expression (\"aw\"+\"f approve --run r\")", "iex $c", "if ($true) { & (echo git) push origin main }",
    ];
    let allowed = [
        "(Get-Content README.md).Count", "git log --oneline -5 | Select-Object -First 3", "if (Test-Path docs) { git status }",
        "Get-ChildItem docs | ForEach-Object { $_.Name }", "(git rev-parse HEAD) -eq \"x\"", "awf kb query \"a & (b)\"", "iex \"git status\"",
    ];
    for agent in [RES, LOOKUP, REFRESH] {
        for c in blocked {
            let rc = shell(&ws, "PowerShell", agent, c);
            if rc != 2 { bad.push(format!("{agent} 該擋：{c:?} rc={rc}")); }
        }
        for c in allowed {
            let rc = shell(&ws, "PowerShell", agent, c);
            if rc != 0 { bad.push(format!("{agent} 該放：{c:?} rc={rc}")); }
        }
    }
    // 主 session 與使用者自己的 agent 不受這條影響
    assert_eq!(shell(&ws, "PowerShell", "", "& (echo git) status"), 0);
    assert_eq!(shell(&ws, "PowerShell", "my-agent", "& (echo git) status"), 0);
    let _ = std::fs::remove_dir_all(&ws);
    assert!(bad.is_empty(), "不符預期：\n{}", bad.join("\n"));
}
