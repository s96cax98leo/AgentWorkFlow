//! 守門認得 `awf <子指令>` 的整合測試（1.0：工具從專案裡的 `.py` 改成單一執行檔 `awf`）。
//!
//! 每一條比對 `.py` 工具的舊規則，對 `awf` 的寫法都有對等的一條；這裡用資料表逐條測：
//! 每條規則至少一個該擋、一個該放，Bash 與 PowerShell 各跑一次；每一種執行檔叫法至少一個該擋；
//! 包一層 shell 與轉呼叫的案例；最後量一條 127KB、上萬段的指令。
//!
//! 加案例：在對應的表加一列即可。`@WS@` 會換成暫存專案的路徑。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use serde_json::json;

const EXE: &str = env!("CARGO_BIN_EXE_awf");
/// 一般的 team agent。
const DEV: &str = "agent-workflow:dev-implementer";
/// 只寫執行紀錄的唯讀角色。
const REV: &str = "agent-workflow:tpm-reviewer";
/// auto_merge 的 merger_role。
const STEW: &str = "agent-workflow:dev-merge-steward";
/// 主 session。
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
        let d = std::env::temp_dir().join(format!("awf-cli-{}-{}", std::process::id(), tag));
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

/// 已 init 的專案：auto_merge 啟用（才有 merger_role 可以比）。
fn workspace(root: &Path) -> PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".claude/teams")).unwrap();
    std::fs::create_dir_all(ws.join("_tpm/runs/r/plan/logs")).unwrap();
    let cfg = json!({
        "runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "writer_dirs": ["_tpm/runs", "docs"],
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "low"}, "merger_role": "dev-merge-steward"},
    });
    std::fs::write(ws.join(".claude/teams/guard.config.json"), cfg.to_string()).unwrap();
    ws
}

fn hook(ws: &Path, tool: &str, agent: &str, command: &str) -> Output {
    let mut c = Command::new(EXE);
    c.args(["hook", "guard-shell"]);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    c.env("CLAUDE_PROJECT_DIR", ws);
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": ws.to_string_lossy(), "tool_input": {"command": command}});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("啟動 awf");
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(d.to_string().as_bytes());
    }
    child.wait_with_output().expect("等 awf 結束")
}

type Case = (&'static str, &'static str, i32, Sh);

/// 跑一張表；回傳失敗的說明（全部跑完再一起報）。
fn run_table(ws: &Path, table: &[Case]) -> Vec<String> {
    let mut bad = Vec::new();
    let mut n = 0;
    for (cmd, agent, want, sh) in table {
        let cmd = cmd.replace("@WS@", &ws.to_string_lossy());
        for (tool, on) in [("Bash", *sh != Sh::Ps), ("PowerShell", *sh != Sh::Bash)] {
            if !on {
                continue;
            }
            n += 1;
            let r = hook(ws, tool, agent, &cmd);
            let err = String::from_utf8_lossy(&r.stderr);
            let who = if agent.is_empty() { "主 session" } else { agent };
            if r.status.code() != Some(*want) || err.contains("panicked at") || err.contains("意外錯誤") {
                bad.push(format!("［{tool}／{who}］{cmd}\n    要 {want}，得到 {:?}；stderr：{}", r.status.code(), err.trim()));
            }
        }
    }
    assert!(n > 0);
    bad
}

fn check(tag: &str, table: &[Case]) {
    let tmp = Tmp::new(tag);
    let ws = workspace(&tmp.0);
    let bad = run_table(&ws, table);
    assert!(bad.is_empty(), "{} 個案例不符：\n{}", bad.len(), bad.join("\n"));
}

// ───────── 1. 每一條規則：該擋與該放 ─────────

const RULES: &[Case] = &[
    // pack：資料包只由主 session 產生
    ("awf pack plan _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf pack build _tpm/runs/r", REV, BLOCK, Sh::Both),
    ("awf pack --selftest", DEV, ALLOW, Sh::Both),
    ("awf pack plan _tpm/runs/r", MAIN, ALLOW, Sh::Both),
    // deliverables：只能 status／check／affected
    ("awf deliverables sync", DEV, BLOCK, Sh::Both),
    ("awf deliverables configure --write --by x", DEV, BLOCK, Sh::Both),
    ("awf deliverables status", DEV, ALLOW, Sh::Both),
    ("awf deliverables check", DEV, ALLOW, Sh::Both),
    ("awf deliverables affected", DEV, ALLOW, Sh::Both),
    ("awf deliverables sync", MAIN, ALLOW, Sh::Both),
    // kb：只能 query／lint
    ("awf kb build", DEV, BLOCK, Sh::Both),
    ("awf kb query \"匯出 日期\"", DEV, ALLOW, Sh::Both),
    ("awf kb lint", DEV, ALLOW, Sh::Both),
    ("awf kb build", MAIN, ALLOW, Sh::Both),
    // save-run：只能 status、--selftest
    ("awf save-run approve _tpm/runs/r --stage plan --by human", DEV, BLOCK, Sh::Both),
    ("awf save-run stage _tpm/runs/r plan", DEV, BLOCK, Sh::Both),
    ("awf save-run close _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf save-run abandon _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf save-run escalate _tpm/runs/r", STEW, BLOCK, Sh::Both),
    ("awf save-run --help", DEV, BLOCK, Sh::Both),
    ("awf save-run status _tpm/runs/r", DEV, ALLOW, Sh::Both),
    ("awf save-run --selftest", DEV, ALLOW, Sh::Both),
    ("awf save-run approve _tpm/runs/r --stage plan --by human", MAIN, ALLOW, Sh::Both),
    // 狀態機：只能 awf state --selftest
    ("awf start \"需求\"", DEV, BLOCK, Sh::Both),
    ("awf next _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf done _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf approve _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf render --all --out x", DEV, BLOCK, Sh::Both),
    ("awf handoff _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf next --selftest", DEV, BLOCK, Sh::Both),
    ("awf state next _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf state", DEV, BLOCK, Sh::Both),
    ("awf state --selftest", DEV, ALLOW, Sh::Both),
    ("awf next _tpm/runs/r", MAIN, ALLOW, Sh::Both),
    // publish：team agent 不能跑；--project-dir 只能是本專案（主 session 也一樣）
    ("awf publish --branch feature/x", DEV, BLOCK, Sh::Both),
    ("awf publish --selftest", DEV, BLOCK, Sh::Both),
    ("awf publish --branch feature/x", STEW, BLOCK, Sh::Both),
    ("awf publish --project-dir /tmp/other --branch feature/x", MAIN, BLOCK, Sh::Both),
    ("awf publish --project-dir=/tmp/other --branch feature/x", MAIN, BLOCK, Sh::Both),
    ("awf publish --project-dir @WS@ --project-dir /tmp/other", MAIN, BLOCK, Sh::Both),
    ("awf publish --project-dir @WS@ --branch feature/x", MAIN, ALLOW, Sh::Both),
    ("awf publish --branch feature/x", MAIN, ALLOW, Sh::Both),
    // merge-gate：check 只有 merger_role 能跑；--project-dir、--runs-dir 只能是本專案
    ("awf merge-gate check --repo acme-fork/app --pr 3", DEV, BLOCK, Sh::Both),
    ("awf merge-gate --repo acme-fork/app --pr 3", DEV, BLOCK, Sh::Both),
    ("awf merge-gate check --repo acme-fork/app --pr 3", STEW, ALLOW, Sh::Both),
    ("awf merge-gate --selftest", DEV, ALLOW, Sh::Both),
    ("awf merge-gate check --runs-dir /tmp/elsewhere", STEW, BLOCK, Sh::Both),
    ("awf merge-gate check --project-dir /tmp/other", STEW, BLOCK, Sh::Both),
    ("awf merge-gate check --runs-dir /tmp/elsewhere", MAIN, BLOCK, Sh::Both),
    ("awf merge-gate check --runs-dir _tpm/runs", MAIN, ALLOW, Sh::Both),
    ("awf merge-gate check --project-dir @WS@", MAIN, ALLOW, Sh::Both),
    // 1.0 才有的：init、build、spec 的寫入、持續監看
    ("awf init --target . --upgrade", DEV, BLOCK, Sh::Both),
    ("awf build", DEV, BLOCK, Sh::Both),
    ("awf build --check", DEV, BLOCK, Sh::Both),
    ("awf spec apply --change add-refund --write", DEV, BLOCK, Sh::Both),
    ("awf spec ids --write", DEV, BLOCK, Sh::Both),
    ("awf spec ids --adopt orders/a=b", DEV, BLOCK, Sh::Both),
    ("awf spec ids --root openspec --retire orders/a", DEV, BLOCK, Sh::Both),
    ("awf spec frobnicate", DEV, BLOCK, Sh::Both),
    ("awf watch", DEV, BLOCK, Sh::Both),
    ("awf watch _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("awf frobnicate", DEV, BLOCK, Sh::Both),
    ("awf init --target . --upgrade", MAIN, ALLOW, Sh::Both),
    ("awf build", MAIN, ALLOW, Sh::Both),
    ("awf spec apply --change add-refund --write", MAIN, ALLOW, Sh::Both),
    ("awf spec status", DEV, ALLOW, Sh::Both),
    ("awf spec validate --change add-refund", DEV, ALLOW, Sh::Both),
    ("awf spec show orders/a", DEV, ALLOW, Sh::Both),
    ("awf spec ids", DEV, ALLOW, Sh::Both),
    ("awf spec ids --root openspec --json", DEV, ALLOW, Sh::Both),
    ("awf doctor", DEV, ALLOW, Sh::Both),
    // golden、tokens：只讀的檢查，team agent 可以跑
    ("awf golden", DEV, ALLOW, Sh::Both),
    ("awf golden --dir .claude/teams/golden-set --verbose", REV, ALLOW, Sh::Both),
    ("awf tokens check src", DEV, ALLOW, Sh::Both),
    ("awf tokens check --all --ref .claude/skills/acme-ds/references src", REV, ALLOW, Sh::Both),
    ("awf tokens verify", DEV, ALLOW, Sh::Both),
    ("awf tokens --selftest", DEV, ALLOW, Sh::Both),
    ("awf watch --once", DEV, ALLOW, Sh::Both),
    ("awf watch _tpm/runs/r --once", DEV, ALLOW, Sh::Both),
    ("awf statusline", DEV, ALLOW, Sh::Both),
    ("awf hook guard-shell", DEV, ALLOW, Sh::Both),
    ("awf --version", DEV, ALLOW, Sh::Both),
    ("awf", DEV, ALLOW, Sh::Both),
    // 沒有限制的唯讀工具
    ("awf sync-branch plan --base main", DEV, ALLOW, Sh::Both),
    ("awf checklist check plans/x.md", DEV, ALLOW, Sh::Both),
    ("awf recipe resolve feature", DEV, ALLOW, Sh::Both),
    ("awf locate --selftest", DEV, ALLOW, Sh::Both),
    ("awf cost _tpm/runs/r", DEV, ALLOW, Sh::Both),
    ("awf script check x.js", DEV, ALLOW, Sh::Both),
    ("awf check-agents --plugin-dir plugin", DEV, ALLOW, Sh::Both),
    ("awf check-envelope out.json", DEV, ALLOW, Sh::Both),
    ("awf check-rules docs/x.md", DEV, ALLOW, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- npm test", DEV, ALLOW, Sh::Both),
    // 只寫執行紀錄的角色：run-brief 的 --log 要在 runs_dir 底下，內層指令照同一套規則
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- npm test", REV, ALLOW, Sh::Both),
    ("awf run-brief --selftest", REV, ALLOW, Sh::Both),
    ("awf run-brief --log /tmp/a.log -- npm test", REV, BLOCK, Sh::Both),
    ("awf run-brief --log docs/a.log -- npm test", REV, BLOCK, Sh::Both),
    ("awf run-brief -- npm test", REV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- cp a b", REV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log --log /tmp/b.log -- npm test", REV, BLOCK, Sh::Both),
    ("xargs awf run-brief --log _tpm/runs/r/plan/logs/a.log --", REV, BLOCK, Sh::Bash),
];

#[test]
fn every_rule_blocks_and_allows() {
    check("rules", RULES);
}

// ───────── 2. 執行檔的各種叫法：都要擋 ─────────

const SPELLINGS: &[Case] = &[
    ("awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("./awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("plugin/bin/awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("/opt/agent-workflow/bin/awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("\"/opt/agent workflow/bin/awf\" save-run approve r", DEV, BLOCK, Sh::Bash),
    ("'/opt/agent workflow/bin/awf' save-run approve r", DEV, BLOCK, Sh::Bash),
    ("awf.cmd save-run approve r", DEV, BLOCK, Sh::Both),
    ("awf.exe save-run approve r", DEV, BLOCK, Sh::Both),
    ("AWF.EXE save-run approve r", DEV, BLOCK, Sh::Both),
    ("plugin/bin/awf-darwin-arm64 save-run approve r", DEV, BLOCK, Sh::Both),
    ("plugin/bin/awf-linux-x64 save-run approve r", DEV, BLOCK, Sh::Both),
    ("plugin/bin/awf-windows-x64.exe save-run approve r", DEV, BLOCK, Sh::Both),
    ("\"${CLAUDE_PLUGIN_ROOT}/bin/awf\" save-run approve r", DEV, BLOCK, Sh::Bash),
    ("X=1 awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("env X=1 awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("env -i awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("command awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("exec awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("nohup awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("timeout 30 awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("sh plugin/bin/awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("a\"w\"f save-run approve r", DEV, BLOCK, Sh::Bash),
    ("a\\wf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("awf \"save-run\" approve r", DEV, BLOCK, Sh::Both),
    ("(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("echo $(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("echo `awf save-run approve r`", DEV, BLOCK, Sh::Bash),
    ("true && awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("ls; awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("ls | awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("if awf save-run approve r; then echo ok; fi", DEV, BLOCK, Sh::Bash),
    // 看不到實際執行檔：後面接的是 awf 的子指令就照它判
    ("$AWF_BIN save-run approve r", DEV, BLOCK, Sh::Both),
    ("\"$AWF_BIN\" save-run approve r", DEV, BLOCK, Sh::Bash),
    ("$(which awf) save-run approve r", DEV, BLOCK, Sh::Bash),
    ("plugin/bin/aw? save-run approve r", DEV, BLOCK, Sh::Bash),
    ("plugin/bin/awf-* save-run approve r", DEV, BLOCK, Sh::Bash),
    ("awf $SUB approve r", DEV, BLOCK, Sh::Both),
    // PowerShell 的寫法
    ("& awf save-run approve r", DEV, BLOCK, Sh::Ps),
    ("& \"C:\\tools\\agent-workflow\\bin\\awf.cmd\" save-run approve r", DEV, BLOCK, Sh::Ps),
    ("& 'C:\\tools\\agent-workflow\\bin\\awf-windows-x64.exe' save-run approve r", DEV, BLOCK, Sh::Ps),
    (".\\bin\\awf.cmd save-run approve r", DEV, BLOCK, Sh::Ps),
    ("& .\\bin\\awf.exe save-run approve r", DEV, BLOCK, Sh::Ps),
    ("& $awf save-run approve r", DEV, BLOCK, Sh::Ps),
    ("$env:X = 1; awf save-run approve r", DEV, BLOCK, Sh::Ps),
    ("Start-Process awf -ArgumentList 'save-run','approve','r'", DEV, BLOCK, Sh::Ps),
    ("Start-Process -FilePath awf.cmd -ArgumentList 'save-run approve r'", DEV, BLOCK, Sh::Ps),
    // 其他被擋的子指令換個叫法也一樣
    ("plugin/bin/awf.cmd next _tpm/runs/r", DEV, BLOCK, Sh::Both),
    ("env X=1 plugin/bin/awf-linux-arm64 publish --branch feature/x", DEV, BLOCK, Sh::Bash),
    ("& .\\bin\\awf.exe init --target .", DEV, BLOCK, Sh::Ps),
    ("./awf.exe merge-gate check --pr 3", DEV, BLOCK, Sh::Both),
    ("/opt/x/awf publish --project-dir /tmp/other", MAIN, BLOCK, Sh::Both),
    // 同樣的叫法跑唯讀子指令：放行
    ("plugin/bin/awf save-run status r", DEV, ALLOW, Sh::Both),
    ("env X=1 awf.cmd kb query x", DEV, ALLOW, Sh::Bash),
    ("AWF_BIN=target/debug/awf awf kb query x", DEV, ALLOW, Sh::Bash),
    ("AWF_BIN=target/debug/awf plugin/bin/awf kb build", DEV, BLOCK, Sh::Bash),
    ("& \"C:\\tools\\bin\\awf.cmd\" kb query x", DEV, ALLOW, Sh::Ps),
    ("$AWF_BIN kb query x", DEV, ALLOW, Sh::Both),
    ("plugin/bin/awf-darwin-arm64 doctor", DEV, ALLOW, Sh::Both),
];

#[test]
fn every_spelling_is_recognised() {
    check("spell", SPELLINGS);
}

// ───────── 3. 包一層 shell、轉呼叫、內嵌程式 ─────────

const WRAPPED: &[Case] = &[
    ("bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Both),
    ("bash -lc \"exec awf save-run approve r\"", DEV, BLOCK, Sh::Bash),
    ("sh -c \"bash -c 'awf save-run approve r'\"", DEV, BLOCK, Sh::Bash),
    ("bash -c 'cd x && plugin/bin/awf.cmd next r'", DEV, BLOCK, Sh::Bash),
    ("eval awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("eval 'a''wf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("eval \"awf publish --branch feature/x\"", DEV, BLOCK, Sh::Bash),
    ("pwsh -Command \"awf save-run approve r\"", DEV, BLOCK, Sh::Both),
    ("powershell -NoProfile -Command \"& awf.cmd save-run approve r\"", DEV, BLOCK, Sh::Both),
    ("cmd /c awf.cmd save-run approve r", DEV, BLOCK, Sh::Both),
    ("iex 'awf save-run approve r'", DEV, BLOCK, Sh::Ps),
    ("Invoke-Expression \"awf pack plan r\"", DEV, BLOCK, Sh::Ps),
    ("xargs awf save-run", DEV, BLOCK, Sh::Bash),
    ("echo approve r | xargs awf save-run", DEV, BLOCK, Sh::Bash),
    ("echo save-run approve r | xargs awf", DEV, BLOCK, Sh::Bash),
    ("echo r | xargs -I{} awf save-run approve {}", DEV, BLOCK, Sh::Bash),
    ("find . -name run.json -exec awf save-run approve {} \\;", DEV, BLOCK, Sh::Bash),
    ("parallel awf save-run approve ::: r", DEV, BLOCK, Sh::Bash),
    ("ionice -c3 awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- awf save-run approve r", DEV, BLOCK, Sh::Both),
    ("python3 -c \"import subprocess; subprocess.run(['awf', 'save-run', 'approve', 'r'])\"", DEV, BLOCK, Sh::Bash),
    ("node -e \"require('child_process').execSync('awf save-run approve r')\"", DEV, BLOCK, Sh::Bash),
    ("bash -c 'awf publish --project-dir /tmp/other'", MAIN, BLOCK, Sh::Bash),
    // 轉呼叫跑唯讀子指令、包一層跑唯讀子指令：放行
    ("xargs awf kb query", DEV, ALLOW, Sh::Bash),
    ("bash -c 'awf save-run status r'", DEV, ALLOW, Sh::Both),
    ("pwsh -Command \"awf kb lint\"", DEV, ALLOW, Sh::Both),
    // 只是提到 awf，不是執行：放行
    ("cat plugin/bin/awf", DEV, ALLOW, Sh::Bash),
    ("ls -l plugin/bin/awf plugin/bin/awf.cmd", DEV, ALLOW, Sh::Bash),
    ("which awf", DEV, ALLOW, Sh::Bash),
    ("command -v awf", DEV, ALLOW, Sh::Bash),
    ("echo awf save-run approve r", DEV, ALLOW, Sh::Bash),
    ("grep -rn \"awf save-run approve\" docs", DEV, ALLOW, Sh::Bash),
    ("git log --oneline -- plugin/bin/awf", DEV, ALLOW, Sh::Bash),
    ("awf kb query \"awf save-run approve 怎麼用\"", DEV, ALLOW, Sh::Both),
    ("awf kb query awf build", DEV, ALLOW, Sh::Both),
    ("cargo build -p awf", DEV, ALLOW, Sh::Bash),
    ("Get-Command awf", DEV, ALLOW, Sh::Ps),
    ("Get-Content plugin\\bin\\awf.cmd", DEV, ALLOW, Sh::Ps),
];

#[test]
fn wrapped_and_forwarded_calls() {
    check("wrap", WRAPPED);
}

// ───────── 4. 舊的 .py 規則還在 ─────────

const LEGACY: &[Case] = &[
    ("python3 .claude/teams/tools/save_run.py approve r", DEV, BLOCK, Sh::Both),
    ("python3 .claude/teams/tools/awf.py next r", DEV, BLOCK, Sh::Both),
    ("python3 .claude/teams/tools/pack.py plan r", DEV, BLOCK, Sh::Both),
    ("python3 .claude/teams/tools/publish.py --branch feature/x", DEV, BLOCK, Sh::Both),
    ("python3 .claude/teams/tools/merge_gate.py check --pr 3", DEV, BLOCK, Sh::Both),
    ("python3 .claude/teams/tools/save_run.py status r", DEV, ALLOW, Sh::Both),
];

#[test]
fn legacy_py_rules_still_apply() {
    check("legacy", LEGACY);
}

// ───────── 5. 訊息對得上使用者打的指令 ─────────

#[test]
fn messages_name_the_awf_command() {
    let tmp = Tmp::new("msg");
    let ws = workspace(&tmp.0);
    for (cmd, agent, needle) in [
        ("awf save-run approve r", DEV, "不能跑 awf save-run approve"),
        ("python3 .claude/teams/tools/save_run.py close r", DEV, "不能跑 awf save-run close"),
        ("awf next r", DEV, "不能跑 awf next"),
        ("awf pack plan r", DEV, "不能跑 awf pack"),
        ("awf kb build", DEV, "只能跑 awf kb query／lint"),
        ("awf deliverables sync", DEV, "只能跑 awf deliverables status／check／affected"),
        ("awf publish --branch feature/x", DEV, "不能執行 awf publish"),
        ("awf merge-gate check --pr 3", DEV, "不能跑 awf merge-gate check"),
        ("awf init --target .", DEV, "不能跑 awf init"),
        ("awf build", DEV, "不能跑 awf build"),
        ("awf spec apply --change x", DEV, "不能跑 awf spec apply"),
        ("awf watch", DEV, "只能跑 awf watch --once"),
        ("awf publish --project-dir /tmp/other", MAIN, "awf publish 的 --project-dir 只能是本專案"),
        ("awf run-brief --log /tmp/a.log -- npm test", REV, "awf run-brief --log 只能是"),
    ] {
        for tool in ["Bash", "PowerShell"] {
            let r = hook(&ws, tool, agent, cmd);
            let err = String::from_utf8_lossy(&r.stderr);
            assert_eq!(r.status.code(), Some(2), "［{tool}］{cmd}：{err}");
            assert!(err.contains(needle), "［{tool}］{cmd} 的訊息要有「{needle}」：{err}");
            assert!(!err.contains(".py"), "［{tool}］{cmd} 的訊息不該再提 .py：{err}");
        }
    }
}

// ───────── 6. 長指令不能拖過時限 ─────────

/// 127KB、上萬段 `&&` 的指令：要在 2 秒內判完（除錯版的執行檔慢很多，只要求不超過內部期限）。
#[test]
fn long_chained_command_stays_fast() {
    let tmp = Tmp::new("perf");
    let ws = workspace(&tmp.0);
    let limit_ms: u128 = if cfg!(debug_assertions) { 7000 } else { 2000 };
    for (unit, want) in [("ls", ALLOW), ("awf kb query x", ALLOW), ("cat plugin/bin/awf", ALLOW), ("$X kb lint", ALLOW), ("bash -c 'awf kb lint'", ALLOW)] {
        let n = (127 * 1024) / (unit.len() + 4);
        let cmd = vec![unit; n].join(" && ");
        assert!(cmd.len() <= 128 * 1024 && cmd.len() > 120 * 1024);
        for (tool, agent) in [("Bash", DEV), ("PowerShell", DEV), ("Bash", REV)] {
            let t0 = Instant::now();
            let r = hook(&ws, tool, agent, &cmd);
            let ms = t0.elapsed().as_millis();
            let err = String::from_utf8_lossy(&r.stderr);
            println!("長指令（{unit} × {n}，{tool}，{agent}）：{ms}ms");
            assert_eq!(r.status.code(), Some(want), "［{tool}］{unit} × {n}：{err}");
            assert!(ms < limit_ms, "［{tool}］{unit} × {n} 花了 {ms}ms，超過 {limit_ms}ms");
        }
    }
    // 最後一段才是不允許的：也要擋得到
    let n = (120 * 1024) / 6;
    let cmd = format!("{} && awf save-run approve r", vec!["ls"; n].join(" && "));
    let r = hook(&ws, "Bash", DEV, &cmd);
    assert_eq!(r.status.code(), Some(2), "{}", String::from_utf8_lossy(&r.stderr));
}
