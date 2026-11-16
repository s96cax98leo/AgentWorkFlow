//! 1.0 第二次獨立審查（新功能）守門端的回歸測試：每一則用審查的重現輸入，修之前是紅的。
//!
//! 每條新守門規則有該擋與該放，Bash 與 PowerShell 各一（`Sh::Both`＝兩邊都跑）。
//! 加案例：在對應的表加一列。`@WS@`＝暫存專案、`@HOME@`＝測試用的家目錄。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
const DEV: &str = "agent-workflow:dev-implementer";
const RES: &str = "agent-workflow:tpm-researcher";
const REV: &str = "agent-workflow:tpm-reviewer";
const MAIN: &str = "";

const BLOCK: i32 = 2;
const ALLOW: i32 = 0;

#[derive(Clone, Copy, PartialEq)]
enum Sh {
    Both,
    Bash,
    Ps,
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("awf-rvg-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建立暫存目錄");
        Tmp(d.canonicalize().expect("解開暫存目錄"))
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("建立目錄");
    std::fs::write(p, text).expect("寫檔");
}

/// 已 init 的專案（有 VERSION 與守門設定）。回（專案, 測試用的家目錄）。
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let ws = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(ws.join("_tpm/runs/r/plan/logs")).unwrap();
    std::fs::create_dir_all(ws.join("_tpm/kb/vault/notes")).unwrap();
    std::fs::create_dir_all(ws.join(".github/workflows")).unwrap();
    std::fs::create_dir_all(home.join(".claude/projects/-x-p/sess-1")).unwrap();
    put(&ws.join(".claude/teams/VERSION"), "1.0.0\n");
    put(&ws.join("_tpm/kb/graph.db"), "");
    let cfg = json!({
        "runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "writer_dirs": ["_tpm/runs", "docs"],
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "low"}, "merger_role": "dev-merge-steward"},
    });
    put(&ws.join(".claude/teams/guard.config.json"), &cfg.to_string());
    (ws, home)
}

struct Env<'a> {
    ws: &'a Path,
    home: &'a Path,
    /// `CLAUDE_PROJECT_DIR`；None＝不設。
    proj: Option<&'a Path>,
    /// 行程的目前目錄。
    cwd: &'a Path,
}

fn run_hook(e: &Env, mode: &str, data: &Value) -> Output {
    let mut c = Command::new(EXE);
    c.args(["hook", mode]).current_dir(e.cwd);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    c.env("HOME", e.home).env("USERPROFILE", e.home);
    if let Some(p) = e.proj {
        c.env("CLAUDE_PROJECT_DIR", p);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("啟動 awf");
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(data.to_string().as_bytes());
    }
    child.wait_with_output().expect("等 awf 結束")
}

fn shell(e: &Env, tool: &str, agent: &str, command: &str) -> Output {
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": s(e.cwd), "session_id": "abcdefgh12", "tool_input": {"command": command}});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    run_hook(e, "guard-shell", &d)
}

fn write_tool(e: &Env, tool: &str, agent: &str, file: &str) -> Output {
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": s(e.cwd), "session_id": "abcdefgh12", "tool_input": {"file_path": file, "content": "x", "old_string": "a", "new_string": "b"}});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    run_hook(e, "guard-paths", &d)
}

type Case = (&'static str, &'static str, i32, Sh);

fn fill(t: &str, e: &Env) -> String {
    t.replace("@WS@", &s(e.ws)).replace("@HOME@", &s(e.home))
}

/// 跑一張 shell 指令的表；回（失敗的說明, 該擋的數量, 該放的數量）。
fn run_table(e: &Env, table: &[Case]) -> (Vec<String>, usize, usize) {
    let mut bad = Vec::new();
    let (mut nb, mut na) = (0, 0);
    for (cmd, agent, want, sh) in table {
        let cmd = fill(cmd, e);
        for (tool, on) in [("Bash", *sh != Sh::Ps), ("PowerShell", *sh != Sh::Bash)] {
            if !on {
                continue;
            }
            if *want == BLOCK { nb += 1 } else { na += 1 }
            let r = shell(e, tool, agent, &cmd);
            let err = String::from_utf8_lossy(&r.stderr);
            let who = if agent.is_empty() { "主 session" } else { agent };
            if r.status.code() != Some(*want) || err.contains("panicked at") || err.contains("意外錯誤") {
                bad.push(format!("［{tool}／{who}］{cmd}\n    要 {want}，得到 {:?}；stderr：{}", r.status.code(), err.trim()));
            }
        }
    }
    (bad, nb, na)
}

fn check(tag: &str, table: &[Case]) {
    let tmp = Tmp::new(tag);
    let (ws, home) = workspace(&tmp.0);
    let e = Env { ws: &ws, home: &home, proj: Some(&ws), cwd: &ws };
    let (bad, nb, na) = run_table(&e, table);
    println!("{tag}：該擋 {nb}、該放 {na}");
    assert!(nb > 0 && na > 0, "{tag}：要有該擋與該放的案例");
    assert!(bad.is_empty(), "{} 個案例不符：\n{}", bad.len(), bad.join("\n"));
}

/// Write／Edit 的表：（路徑, 角色, 預期）。Write 與 Edit 各跑一次。
fn check_paths(tag: &str, table: &[(&str, &str, i32)]) {
    let tmp = Tmp::new(tag);
    let (ws, home) = workspace(&tmp.0);
    let e = Env { ws: &ws, home: &home, proj: Some(&ws), cwd: &ws };
    let mut bad = Vec::new();
    for (file, agent, want) in table {
        let file = fill(file, &e);
        for tool in ["Write", "Edit"] {
            let r = write_tool(&e, tool, agent, &file);
            let err = String::from_utf8_lossy(&r.stderr);
            if r.status.code() != Some(*want) || err.contains("panicked at") || err.contains("意外錯誤") {
                bad.push(format!("［{tool}／{agent}］{file}\n    要 {want}，得到 {:?}；stderr：{}", r.status.code(), err.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{} 個案例不符：\n{}", bad.len(), bad.join("\n"));
}

// ───────── 專案根：hook 和閘門看同一個專案 ─────────

/// 沒有 `CLAUDE_PROJECT_DIR` 時，hook 從目前目錄往上找最外層的 `.claude/teams/VERSION`（和 merge-gate 一樣），
/// 不是直接拿目前目錄：在子目錄裡執行，主 session 的規則（設定檔裡開的）照樣生效。
#[test]
fn hook_finds_the_project_root_like_the_gates() {
    let tmp = Tmp::new("root");
    let (ws, home) = workspace(&tmp.0);
    let sub = ws.join("repos/app/src");
    std::fs::create_dir_all(&sub).unwrap();
    // 子目錄自帶一份（PR 的 checkout）：auto_merge 沒開、沒有主 session 的規則
    put(&ws.join("repos/app/.claude/teams/VERSION"), "1.0.0\n");
    put(&ws.join("repos/app/.claude/teams/guard.config.json"), &json!({"runs_dir": "_tpm/runs", "main_session_github_rules": false}).to_string());
    for cwd in [&sub, &ws.join("repos/app")] {
        let e = Env { ws: &ws, home: &home, proj: None, cwd };
        for tool in ["Bash", "PowerShell"] {
            let r = shell(&e, tool, MAIN, "gh pr merge 3 --repo acme-fork/app --squash");
            assert_eq!(r.status.code(), Some(BLOCK), "［{tool}］在 {} 執行：{}", cwd.display(), String::from_utf8_lossy(&r.stderr));
        }
        // team agent 寫真專案的 run.json：照真專案的 runs_dir 擋
        let r = write_tool(&e, "Write", DEV, &format!("{}/_tpm/runs/r/run.json", s(&ws)));
        assert_eq!(r.status.code(), Some(BLOCK), "{}", String::from_utf8_lossy(&r.stderr));
    }
    // 有設 CLAUDE_PROJECT_DIR 時以它為準（原本的行為）
    let e = Env { ws: &ws, home: &home, proj: Some(&ws), cwd: &sub };
    assert_eq!(shell(&e, "Bash", MAIN, "gh pr merge 3 --repo acme-fork/app --squash").status.code(), Some(BLOCK));
    // 往上都沒有 VERSION（沒套用的專案）：照舊用目前目錄、不擋主 session
    let plain = tmp.0.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let e = Env { ws: &plain, home: &home, proj: None, cwd: &plain };
    assert_eq!(shell(&e, "Bash", MAIN, "gh pr merge 3 --repo acme-fork/app --squash").status.code(), Some(ALLOW));
    assert_eq!(shell(&e, "Bash", MAIN, "ls").status.code(), Some(ALLOW));
}
