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
    ("awf deliverables matrix --project", DEV, BLOCK, Sh::Both),
    ("awf deliverables matrix --project", MAIN, ALLOW, Sh::Both),
    ("awf deliverables sync", MAIN, ALLOW, Sh::Both),
    // kb：只能 query／lint
    ("awf kb build", DEV, BLOCK, Sh::Both),
    ("awf kb query \"匯出 日期\"", DEV, ALLOW, Sh::Both),
    ("awf kb lint", DEV, ALLOW, Sh::Both),
    ("awf kb build", MAIN, ALLOW, Sh::Both),
    // kg：只能跑五個查詢，而且不能帶寫入的選項；圖與 vault 寫不到
    ("awf kg build", DEV, BLOCK, Sh::Both),
    ("awf kg build --full --timeout 5", DEV, BLOCK, Sh::Both),
    ("awf kg export", DEV, BLOCK, Sh::Both),
    ("awf kg export --out docs/vault", REV, BLOCK, Sh::Both),
    ("awf kg locate x --out docs/a.md", DEV, BLOCK, Sh::Both),
    ("awf kg check --full", DEV, BLOCK, Sh::Both),
    ("awf kg relink --write", DEV, BLOCK, Sh::Both),
    ("awf kg locate \"匯出 發票\"", DEV, ALLOW, Sh::Both),
    ("awf kg trace billing/export-invoice --json", DEV, ALLOW, Sh::Both),
    ("awf kg impact src/a.ts", DEV, ALLOW, Sh::Both),
    ("awf kg impact --diff main", REV, ALLOW, Sh::Both),
    ("awf kg check --run _tpm/runs/r", DEV, ALLOW, Sh::Both),
    ("awf kg neighbors src/a.ts#main --depth 3", DEV, ALLOW, Sh::Both),
    ("awf kg --selftest", DEV, ALLOW, Sh::Both),
    ("awf kg build", MAIN, ALLOW, Sh::Both),
    // program：team agent 只能看狀態；開工建議給主 session 與人
    ("awf program status", DEV, ALLOW, Sh::Both),
    ("awf program status --json", REV, ALLOW, Sh::Both),
    ("awf program --selftest", DEV, ALLOW, Sh::Both),
    ("awf program next", DEV, BLOCK, Sh::Both),
    ("awf program next --json", REV, BLOCK, Sh::Both),
    ("awf program next", MAIN, ALLOW, Sh::Both),
    ("awf kg export", MAIN, ALLOW, Sh::Both),
    ("rm _tpm/kb/graph.db", DEV, BLOCK, Sh::Bash),
    ("Remove-Item _tpm/kb/graph.db", DEV, BLOCK, Sh::Ps),
    ("echo x > _tpm/kb/vault/notes/idea.md", DEV, BLOCK, Sh::Bash),
    ("Set-Content _tpm\\kb\\vault\\notes\\idea.md x", DEV, BLOCK, Sh::Ps),
    ("cp a.md _tpm/kb/vault/需求/a.md", DEV, BLOCK, Sh::Bash),
    ("cat _tpm/kb/vault/hot.md", DEV, ALLOW, Sh::Bash),
    ("Get-Content _tpm/kb/vault/hot.md", DEV, ALLOW, Sh::Ps),
    ("ls -l _tpm/kb/graph.db", DEV, ALLOW, Sh::Bash),
    ("rm _tpm/kb/graph.db", MAIN, ALLOW, Sh::Bash),
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
    // scrub：只讀的掃描，team agent 可以跑；接在後面的寫入指令照擋
    ("awf scrub", DEV, ALLOW, Sh::Both),
    ("awf scrub docs src --json", REV, ALLOW, Sh::Both),
    ("awf scrub --selftest", DEV, ALLOW, Sh::Both),
    ("awf scrub; awf publish push --branch feature/x", DEV, BLOCK, Sh::Both),
    ("awf scrub docs; awf save-run approve r", REV, BLOCK, Sh::Both),
    // ci：render（只印）與 check 可以跑；--write 會寫 CI 檔與 guard.config，只由主 session 跑
    ("awf ci render", DEV, ALLOW, Sh::Both),
    ("awf ci check --root .", REV, ALLOW, Sh::Both),
    ("awf ci --selftest", DEV, ALLOW, Sh::Both),
    ("awf ci render --write", DEV, BLOCK, Sh::Both),
    ("awf ci render --root . --write", REV, BLOCK, Sh::Both),
    ("awf ci render; awf ci render --write", DEV, BLOCK, Sh::Both),
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
        ("awf kg build", DEV, "不能跑 awf kg build"),
        ("awf kg trace billing/export --out x", DEV, "不能跑 awf kg trace --out"),
        ("rm _tpm/kb/graph.db", DEV, "不能寫知識圖的檔"),
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
    // 上萬個指令替換：超過展開上限，team agent 往嚴走（擋），主 session 照舊；兩邊都不能拖過時限
    for (unit, glue) in [("echo $(awf kb lint)", " && "), ("\"$(awf kb lint)\"", " "), ("$(echo $(echo $(echo $(echo $(date)))))", " ")] {
        let n = (127 * 1024) / (unit.len() + glue.len());
        let cmd = format!("echo {}", vec![unit; n].join(glue));
        for (agent, want) in [(DEV, BLOCK), (MAIN, ALLOW)] {
            let t0 = Instant::now();
            let r = hook(&ws, "Bash", agent, &cmd);
            let ms = t0.elapsed().as_millis();
            let err = String::from_utf8_lossy(&r.stderr);
            println!("長指令（{unit} × {n}，Bash，{}）：{ms}ms", if agent.is_empty() { "主 session" } else { agent });
            assert_eq!(r.status.code(), Some(want), "{unit} × {n}：{err}");
            assert!(ms < limit_ms, "{unit} × {n} 花了 {ms}ms，超過 {limit_ms}ms");
        }
    }
    // 最後一段才是不允許的：也要擋得到
    let n = (120 * 1024) / 6;
    let cmd = format!("{} && awf save-run approve r", vec!["ls"; n].join(" && "));
    let r = hook(&ws, "Bash", DEV, &cmd);
    assert_eq!(r.status.code(), Some(2), "{}", String::from_utf8_lossy(&r.stderr));
}

// ───────── 7. 知識圖的檔 team agent 寫不到（Write／Edit） ─────────

fn paths_hook(ws: &Path, tool: &str, agent: &str, file: &Path) -> Output {
    let mut c = Command::new(EXE);
    c.args(["hook", "guard-paths"]);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    c.env("CLAUDE_PROJECT_DIR", ws);
    let mut d = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": ws.to_string_lossy(),
        "tool_input": {"file_path": file.to_string_lossy(), "content": "x", "old_string": "a", "new_string": "b"}});
    if !agent.is_empty() {
        d["agent_type"] = json!(agent);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("啟動 awf");
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(d.to_string().as_bytes());
    }
    child.wait_with_output().expect("等 awf 結束")
}

#[test]
fn graph_and_vault_are_not_writable_by_team_agents() {
    let tmp = Tmp::new("kg");
    let ws = workspace(&tmp.0);
    std::fs::create_dir_all(ws.join("_tpm/kb/vault/notes")).unwrap();
    std::fs::write(ws.join("_tpm/kb/graph.db"), "x").unwrap();
    std::fs::create_dir_all(ws.join("docs/vault")).unwrap();
    for (rel, agent, want) in [
        ("_tpm/kb/graph.db", DEV, BLOCK),
        ("_tpm/kb/graph.db.tmp-1", DEV, BLOCK),
        ("_tpm/kb/vault/hot.md", DEV, BLOCK),
        ("_tpm/kb/vault/notes/idea.md", DEV, BLOCK),
        ("_tpm/kb/vault/notes/idea.md", "agent-workflow:tpm-writer", BLOCK),
        ("_tpm/kb/vault/notes/idea.md", MAIN, ALLOW),
        // 專案自己叫 vault 的目錄（旁邊沒有圖）不受影響
        ("docs/vault/readme.md", "agent-workflow:tpm-writer", ALLOW),
    ] {
        for tool in ["Write", "Edit"] {
            let r = paths_hook(&ws, tool, agent, &ws.join(rel));
            let err = String::from_utf8_lossy(&r.stderr);
            assert_eq!(r.status.code(), Some(want), "［{tool}／{agent}］{rel}：{err}");
            if want == BLOCK && agent == DEV {
                assert!(err.contains("知識圖"), "［{tool}］{rel} 的訊息：{err}");
            }
        }
    }
}

// ───────── 8. 指令替換、process substitution 裡的指令（審查 H2） ─────────

/// 使用者自己的 agent（不是模版角色）。
const MINE: &str = "my-helper";

/// 一般的 commit 寫法：訊息放在加引號的 heredoc 裡，內文不是指令。
const COMMIT_HEREDOC: &str = "git commit -m \"$(cat <<'EOF'\nfix: awf save-run approve r 的守門說明\n\n不要 cat .env\nEOF\n)\"";

const SUBSTITUTED: &[Case] = &[
    // 回歸（0.x 對同寫法的 .py 會擋）
    ("X=$(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("X=`awf save-run approve r`", DEV, BLOCK, Sh::Bash),
    ("out=$(awf publish --pr 3)", DEV, BLOCK, Sh::Bash),
    ("x=$(awf build)", DEV, BLOCK, Sh::Bash),
    ("x=$(awf kg build)", DEV, BLOCK, Sh::Bash),
    ("x=$(awf ci render --write)", DEV, BLOCK, Sh::Bash),
    // 兩版都放行過、字面看得到 awf 與子指令
    ("echo \"$(awf save-run approve r)\"", DEV, BLOCK, Sh::Bash),
    ("test -n \"$(awf save-run approve r plan)\"", DEV, BLOCK, Sh::Bash),
    ("git commit -m \"$(awf save-run approve r)\"", DEV, BLOCK, Sh::Bash),
    ("$(git rev-parse --show-toplevel)/plugin/bin/awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("echo x > >(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    // 同一類的其他寫法
    ("`git rev-parse --show-toplevel`/plugin/bin/awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("${CLAUDE_PLUGIN_ROOT}/bin/awf-$(uname -s)-arm64 save-run approve r", DEV, BLOCK, Sh::Bash),
    ("export X=$(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("local x=$(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("out=\"$(awf publish)\"", DEV, BLOCK, Sh::Bash),
    ("result=`awf next r`", DEV, BLOCK, Sh::Bash),
    ("echo \"`awf save-run approve r`\"", DEV, BLOCK, Sh::Bash),
    ("[ \"$(awf done r)\" ]", DEV, BLOCK, Sh::Bash),
    ("echo ${x:-$(awf save-run approve r)}", DEV, BLOCK, Sh::Bash),
    (": \"${x:=$(awf save-run approve r)}\"", DEV, BLOCK, Sh::Bash),
    ("printf '%s' \"$(awf init --target .)\"", DEV, BLOCK, Sh::Bash),
    ("echo \"$(cd x && awf save-run approve r)\"", DEV, BLOCK, Sh::Bash),
    ("echo \"x$( awf save-run approve r )\"", DEV, BLOCK, Sh::Bash),
    ("cat <(awf save-run approve r)", DEV, BLOCK, Sh::Bash),
    ("tee >(awf save-run approve r) < /dev/null", DEV, BLOCK, Sh::Bash),
    ("awf kb query \"$(awf save-run approve r)\"", DEV, BLOCK, Sh::Bash),
    ("echo \"$(echo \"$(awf save-run approve r)\")\"", DEV, BLOCK, Sh::Bash),
    ("bash -c 'X=$(awf save-run approve r)'", DEV, BLOCK, Sh::Both),
    ("eval 'echo \"$(awf deliverables sync)\"'", DEV, BLOCK, Sh::Bash),
    // 內層指令本身以賦值開頭（整個是一個長得像賦值的 token）
    ("bash -c 'X=1 awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("bash -c 'X=1 awf kb query x'", DEV, ALLOW, Sh::Bash),
    ("cat <<EOF\n$(awf save-run approve r)\nEOF", DEV, BLOCK, Sh::Bash),
    ("X=$(cat <<EOF\n$(awf save-run approve r)\nEOF\n)", DEV, BLOCK, Sh::Bash),
    // 巢狀超過上限：看不清楚就不允許
    ("echo $(echo $(echo $(echo $(echo $(awf save-run approve r)))))", DEV, BLOCK, Sh::Bash),
    // 舊的 .py 規則也一樣
    ("echo \"$(python3 .claude/teams/tools/save_run.py approve r)\"", DEV, BLOCK, Sh::Bash),
    ("cat <(python3 .claude/teams/tools/pack.py plan r)", DEV, BLOCK, Sh::Bash),
    // 其他比對 token 的規則：內層照樣檢查
    ("echo \"$(git push origin main)\"", MAIN, BLOCK, Sh::Bash),
    ("X=$(git push origin main)", MAIN, BLOCK, Sh::Bash),
    ("echo \"$(git push --force origin feature/x)\"", MAIN, BLOCK, Sh::Bash),
    ("X=$(cat .env)", MAIN, BLOCK, Sh::Bash),
    ("echo \"$(cat .env)\"", DEV, BLOCK, Sh::Bash),
    ("cat <(cat .env)", DEV, BLOCK, Sh::Bash),
    ("echo \"$(git checkout -b wrongname)\"", MAIN, BLOCK, Sh::Bash),
    ("out=$(awf publish --project-dir /tmp/other)", MAIN, BLOCK, Sh::Bash),
    ("echo \"$(rm -rf src)\"", REV, BLOCK, Sh::Bash),
    // PowerShell 的對應寫法
    ("$x = $(awf save-run approve r)", DEV, BLOCK, Sh::Ps),
    ("$x = (awf save-run approve r)", DEV, BLOCK, Sh::Ps),
    ("Write-Host \"$(awf save-run approve r)\"", DEV, BLOCK, Sh::Ps),
    // 唯讀查詢的結果存進變數、放進字串：放行
    ("X=$(awf kg locate \"匯出 發票\")", DEV, ALLOW, Sh::Bash),
    ("HITS=\"$(awf kg locate export --json)\"", DEV, ALLOW, Sh::Bash),
    ("X=`awf kb query 發票`", DEV, ALLOW, Sh::Bash),
    ("echo \"$(awf save-run status r)\"", DEV, ALLOW, Sh::Bash),
    ("out=$(awf deliverables status)", DEV, ALLOW, Sh::Bash),
    ("test -n \"$(awf kb query x)\"", DEV, ALLOW, Sh::Bash),
    ("x=$(awf state --selftest)", DEV, ALLOW, Sh::Bash),
    ("$(git rev-parse --show-toplevel)/plugin/bin/awf save-run status r", DEV, ALLOW, Sh::Bash),
    ("diff <(awf kg trace a) <(awf kg trace b)", DEV, ALLOW, Sh::Bash),
    ("X=$(awf kg locate \"$(basename \"$PWD\")\")", DEV, ALLOW, Sh::Bash),
    ("X=$(date +%s); echo \"$X $(git status --short | wc -l)\"", DEV, ALLOW, Sh::Bash),
    ("X=$(cat .env.example)", DEV, ALLOW, Sh::Bash),
    ("echo '$(awf save-run approve r)'", DEV, ALLOW, Sh::Bash),
    ("grep -n 'X=$(awf save-run approve r)' docs/a.md", DEV, ALLOW, Sh::Bash),
    (COMMIT_HEREDOC, DEV, ALLOW, Sh::Bash),
    (COMMIT_HEREDOC, MAIN, ALLOW, Sh::Bash),
    ("$x = $(awf kg locate export)", DEV, ALLOW, Sh::Ps),
    ("Write-Host \"$(awf save-run status r)\"", DEV, ALLOW, Sh::Ps),
    // 主 session：照舊
    ("X=$(awf save-run approve r plan --by human)", MAIN, ALLOW, Sh::Bash),
    ("echo \"$(awf build --check)\"", MAIN, ALLOW, Sh::Bash),
    ("X=$(git push origin feature/x)", MAIN, ALLOW, Sh::Bash),
    ("echo \"$(git status)\" \"$(cat README.md)\"", MAIN, ALLOW, Sh::Bash),
    // 使用者自己的 agent：不套 team agent 的規則
    ("X=$(awf save-run approve r)", MINE, ALLOW, Sh::Bash),
];

#[test]
fn substituted_commands_are_checked() {
    check("subst", SUBSTITUTED);
}

// ───────── 9. 不認得的包裝加子指令改寫（ANSI-C 引號、大括號展開） ─────────

const REWRITTEN: &[Case] = &[
    ("caffeinate awf $'save-run' approve r", DEV, BLOCK, Sh::Bash),
    ("arch -arm64 awf {save-run,approve,r}", DEV, BLOCK, Sh::Bash),
    ("{awf,save-run,approve,r}", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf {save-run,approve,r}", DEV, BLOCK, Sh::Bash),
    ("arch -arm64 awf $'save-run' approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf $'save\\x2drun' approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf $'\\163ave-run' approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf $\"save-run\" approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate $'\\x61wf' save-run approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate {plugin/bin/awf,save-run,approve,r}", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf {publish,--branch,feature/x}", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf save-{run,} approve r", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf {save-run,{approve,r}}", DEV, BLOCK, Sh::Bash),
    ("bash -c $'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("bash -c '{awf,save-run,approve,r}'", DEV, BLOCK, Sh::Both),
    // 不認得的包裝後面的 awf，子指令是變數：看不到就不允許
    ("caffeinate awf $SUB approve r", DEV, BLOCK, Sh::Bash),
    ("arch -arm64 plugin/bin/awf \"$1\" r", DEV, BLOCK, Sh::Bash),
    // 同樣的寫法跑唯讀子指令：放行
    // 子指令寫成 $'…' 的，唯讀的也不放（和直接寫 awf $'kb' query x 原本就擋一致：請直接寫子指令）
    ("caffeinate awf $'kb' query x", DEV, BLOCK, Sh::Bash),
    ("awf $'kb' query x", DEV, BLOCK, Sh::Bash),
    ("caffeinate awf kb query $'發票\\t匯出'", DEV, ALLOW, Sh::Bash),
    ("arch -arm64 awf {kb,query,x}", DEV, ALLOW, Sh::Bash),
    ("{awf,kb,query,x}", DEV, ALLOW, Sh::Bash),
    ("caffeinate awf {save-run,status,r}", DEV, ALLOW, Sh::Bash),
    ("caffeinate awf save-run status \"$RUN\"", DEV, ALLOW, Sh::Bash),
    // 只是提到 awf：放行
    ("echo {awf,save-run,approve,r}", DEV, ALLOW, Sh::Bash),
    ("ls plugin/bin/{awf,awf.cmd}", DEV, ALLOW, Sh::Bash),
    ("mkdir -p docs/{awf,save-run}", DEV, ALLOW, Sh::Bash),
    ("git commit -m $'awf save-run approve r 的說明'", DEV, ALLOW, Sh::Bash),
    ("grep -rn $'awf\\tsave-run' docs", DEV, ALLOW, Sh::Bash),
    ("echo \"${AWF_BIN:-awf} save-run approve\"", DEV, ALLOW, Sh::Bash),
    ("cargo build -p awf", DEV, ALLOW, Sh::Bash),
    ("cargo test -p awf \"$FILTER\"", DEV, ALLOW, Sh::Bash),
    ("cargo build -p awf $CARGO_FLAGS", DEV, ALLOW, Sh::Bash),
    ("caffeinate -i cargo test -p awf --test hooks_entry", DEV, ALLOW, Sh::Bash),
    // 主 session 與使用者自己的 agent：照舊
    ("caffeinate awf $'save-run' approve r plan --by human", MAIN, ALLOW, Sh::Bash),
    ("{awf,save-run,approve,r}", MINE, ALLOW, Sh::Bash),
];

#[test]
fn rewritten_subcommands_are_recognised() {
    check("rewrite", REWRITTEN);
}

// ───────── 10. bash -c 藏在帶值旗標的包裝或關鍵字後面 ─────────

const HIDDEN_SHELL: &[Case] = &[
    ("sudo -u root bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("env -u FOO bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("timeout -s KILL 5 bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("if true; then bash -c 'awf save-run approve r'; fi", DEV, BLOCK, Sh::Bash),
    ("! bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("bash -c -- 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("xargs -I{} bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("find . -maxdepth 0 -exec bash -c 'awf save-run approve r' \\;", DEV, BLOCK, Sh::Bash),
    ("flock /tmp/l bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("caffeinate bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("arch -arm64 bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("script -q /dev/null bash -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("nice -n 5 sh -c 'awf publish --branch feature/x'", DEV, BLOCK, Sh::Bash),
    ("while true; do zsh -c 'awf next r'; done", DEV, BLOCK, Sh::Bash),
    ("bash -x -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("bash -c -x 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("sudo -u root pwsh -Command \"awf save-run approve r\"", DEV, BLOCK, Sh::Bash),
    ("sudo -u root eval 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("sudo -u root bash -c 'X=$(awf save-run approve r)'", DEV, BLOCK, Sh::Bash),
    ("if ($true) { bash -c 'awf save-run approve r' }", DEV, BLOCK, Sh::Ps),
    // 同樣的寫法跑唯讀子指令或別的指令：放行
    ("sudo -u root bash -c 'awf kb query x'", DEV, ALLOW, Sh::Bash),
    ("timeout -s KILL 5 bash -c 'awf save-run status r'", DEV, ALLOW, Sh::Bash),
    ("if true; then bash -c 'awf kg locate x'; fi", DEV, ALLOW, Sh::Bash),
    ("xargs -I{} bash -c 'awf kb query {}'", DEV, ALLOW, Sh::Bash),
    ("find . -name '*.md' -exec bash -c 'wc -l \"$1\"' _ {} \\;", DEV, ALLOW, Sh::Bash),
    ("timeout -s KILL 5 bash -c 'cargo test'", DEV, ALLOW, Sh::Bash),
    ("if ($true) { bash -c 'awf kb lint' }", DEV, ALLOW, Sh::Ps),
    // 只是提到：放行
    ("echo bash -c 'awf save-run approve r'", DEV, ALLOW, Sh::Bash),
    ("grep -rn \"bash -c 'awf save-run approve r'\" docs", DEV, ALLOW, Sh::Bash),
    // 唯讀角色的「不能用 shell 寫檔」也看得到藏起來的內層
    ("sudo -u root bash -c 'rm -rf src'", REV, BLOCK, Sh::Bash),
    ("timeout -s KILL 5 bash -c 'touch x'", REV, BLOCK, Sh::Bash),
    ("if true; then bash -c 'rm -rf src'; fi", REV, BLOCK, Sh::Bash),
    ("timeout -s KILL 5 bash -c 'git status'", REV, ALLOW, Sh::Bash),
    ("if true; then bash -c 'ls src | wc -l'; fi", REV, ALLOW, Sh::Bash),
    ("echo bash -c 'rm -rf src'", REV, ALLOW, Sh::Bash),
    // 主 session：照舊
    ("sudo -u root bash -c 'awf save-run approve r plan --by human'", MAIN, ALLOW, Sh::Bash),
];

#[test]
fn shell_hidden_behind_wrappers_is_expanded() {
    check("hidden", HIDDEN_SHELL);
}

// ───────── 11. 會執行參數的指令 ─────────

const EXEC_ARGS: &[Case] = &[
    ("git bisect run awf save-run approve r", DEV, BLOCK, Sh::Bash),
    ("awk 'BEGIN{system(\"awf save-run approve r\")}'", DEV, BLOCK, Sh::Bash),
    ("trap 'awf save-run approve r' EXIT", DEV, BLOCK, Sh::Bash),
    ("osascript -e 'do shell script \"awf save-run approve r\"'", DEV, BLOCK, Sh::Bash),
    ("git bisect run sh -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("git rebase -x 'awf save-run approve r' main", DEV, BLOCK, Sh::Bash),
    ("git rebase --exec='awf publish --branch feature/x' main", DEV, BLOCK, Sh::Bash),
    ("git submodule foreach 'awf build'", DEV, BLOCK, Sh::Bash),
    ("git -c alias.x='!awf save-run approve r' x", DEV, BLOCK, Sh::Bash),
    ("gawk 'BEGIN { \"awf save-run approve r\" | getline }'", DEV, BLOCK, Sh::Bash),
    ("timeout 5 awk 'BEGIN{system(\"awf next r\")}'", DEV, BLOCK, Sh::Bash),
    ("trap \"awf deliverables sync\" ERR", DEV, BLOCK, Sh::Bash),
    ("watch 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("watch -n 5 'cd x && awf next r'", DEV, BLOCK, Sh::Bash),
    ("su -c 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("env -S \"awf save-run approve r\"", DEV, BLOCK, Sh::Bash),
    ("ssh localhost 'awf save-run approve r'", DEV, BLOCK, Sh::Bash),
    ("parallel 'awf save-run approve {}' ::: r", DEV, BLOCK, Sh::Bash),
    ("Invoke-Command -ScriptBlock { awf save-run approve r }", DEV, BLOCK, Sh::Ps),
    ("Register-EngineEvent PowerShell.Exiting -Action { awf save-run approve r }", DEV, BLOCK, Sh::Ps),
    // 同樣的寫法跑唯讀子指令或別的指令：放行
    ("git bisect run awf kb query x", DEV, ALLOW, Sh::Bash),
    ("git bisect run cargo test", DEV, ALLOW, Sh::Bash),
    ("awk 'BEGIN{system(\"awf kb query x\")}'", DEV, ALLOW, Sh::Bash),
    ("trap 'awf save-run status r' EXIT", DEV, ALLOW, Sh::Bash),
    ("trap 'rm -f \"$TMP\"' EXIT", DEV, ALLOW, Sh::Bash),
    ("git rebase -x 'cargo test' main", DEV, ALLOW, Sh::Bash),
    ("watch -n 5 'awf watch --once'", DEV, ALLOW, Sh::Bash),
    ("ssh build-host 'awf doctor'", DEV, ALLOW, Sh::Bash),
    ("Invoke-Command -ScriptBlock { awf kb query x }", DEV, ALLOW, Sh::Ps),
    // 只是提到（文字、樣式、訊息）：放行
    ("awk '/awf save-run approve/ {print $2}' docs/a.md", DEV, ALLOW, Sh::Bash),
    ("awk -F'|' '{print $1}' docs/awf-save-run.md", DEV, ALLOW, Sh::Bash),
    ("osascript -e 'display notification \"awf save-run approve r 完成\"'", DEV, ALLOW, Sh::Bash),
    ("git log --grep 'awf save-run approve'", DEV, ALLOW, Sh::Bash),
    ("git commit -m \"trap 'awf save-run approve r' EXIT 要擋\"", DEV, ALLOW, Sh::Bash),
    ("sed -n 's/awf save-run approve/x/p' docs/a.md", DEV, ALLOW, Sh::Bash),
    // 主 session：照舊
    ("git bisect run awf save-run approve r", MAIN, ALLOW, Sh::Bash),
    ("trap 'awf save-run approve r plan --by human' EXIT", MAIN, ALLOW, Sh::Bash),
];

#[test]
fn commands_that_run_their_arguments() {
    check("execarg", EXEC_ARGS);
}

// ───────── 12. awf run-brief --log 的目標 ─────────

const BRIEF_LOG: &[Case] = &[
    // team agent 不能藉 --log 寫守門設定、稽核紀錄、run 狀態、共用背景、CI 檔
    ("awf run-brief --log .claude/teams/guard.config.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/merge-audit.jsonl -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/run.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log .claude/teams/methodology.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/context.md -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log .github/workflows/ci.yml -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/auto-merge.freeze.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log ./_tpm/runs/../runs/r/run.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --name x --log @WS@/_tpm/runs/publish-audit.jsonl -- echo hi", DEV, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log --log _tpm/runs/r/run.json -- echo hi", DEV, BLOCK, Sh::Both),
    ("plugin/bin/awf.cmd run-brief --log _tpm/runs/r/run.json -- echo hi", "agent-workflow:tpm-writer", BLOCK, Sh::Both),
    ("AWF_LOG_DIR=.claude/teams awf run-brief --name guard -- echo hi", DEV, BLOCK, Sh::Bash),
    ("bash -c 'awf run-brief --log _tpm/runs/r/run.json -- echo hi'", DEV, BLOCK, Sh::Both),
    ("X=$(awf run-brief --log _tpm/runs/r/run.json -- echo hi)", DEV, BLOCK, Sh::Bash),
    ("cd _tpm/runs/r && awf run-brief --log run.json -- echo hi", DEV, BLOCK, Sh::Both),
    // 主 session：稽核紀錄、凍結檔、（auto_merge 啟用時的）guard.config.json 本來就不能直接改
    ("awf run-brief --log _tpm/runs/merge-audit.jsonl -- echo hi", MAIN, BLOCK, Sh::Both),
    ("awf run-brief --log .claude/teams/guard.config.json -- echo hi", MAIN, BLOCK, Sh::Both),
    ("awf run-brief --log _tpm/runs/auto-merge.freeze.json -- echo hi", MAIN, BLOCK, Sh::Both),
    // 一般用法：放行
    ("awf run-brief --log _tpm/runs/r/test/logs/unit.log -- cargo test", DEV, ALLOW, Sh::Both),
    ("awf run-brief --log \"$RUN/test/logs/unit.log\" -- npm test", DEV, ALLOW, Sh::Bash),
    ("awf run-brief -- npm test", DEV, ALLOW, Sh::Both),
    ("awf run-brief --name unit --tail 50 -- npm test", DEV, ALLOW, Sh::Both),
    ("awf run-brief --log build/out.log -- npm test", DEV, ALLOW, Sh::Both),
    ("awf run-brief --log build/out.log -- grep -c --log docs/run.json.md", DEV, ALLOW, Sh::Both),
    ("AWF_LOG_DIR=build/logs awf run-brief --name unit -- npm test", DEV, ALLOW, Sh::Bash),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- npm test", REV, ALLOW, Sh::Both),
    ("awf run-brief --log out.log -- npm test", MAIN, ALLOW, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/test/logs/unit.log -- npm test", MAIN, ALLOW, Sh::Both),
    ("awf run-brief --log out.log -- npm test", MINE, ALLOW, Sh::Both),
];

#[test]
fn run_brief_log_target_is_guarded() {
    check("brieflog", BRIEF_LOG);
}

// ───────── 13. guard.config.json 讀得懂與讀不懂（審查 M4） ─────────

/// 專案的設定檔換成指定的內容。
fn workspace_with(root: &Path, config: &str) -> PathBuf {
    let ws = workspace(root);
    std::fs::write(ws.join(".claude/teams/guard.config.json"), config).unwrap();
    ws
}

/// Python 的 `json.loads` 讀得懂、嚴格的 JSON 解析器不收的寫法：設定照樣要生效。
const ODD_BUT_VALID: &str = r#"{
  "runs_dir": "work/runs",
  "forbidden_paths": ["repos/readonly-ref"],
  "extension_agents": {"my:auditor": "tpm-reviewer"},
  "team_agent_namespaces": ["acme:"],
  "_note": {"ratio": NaN, "limit": Infinity, "low": -Infinity, "big": 1e999,
            "id": 123456789012345678901234567890123456789012345678901234567890, "half": "\ud800"}
}"#;

const ODD_CONFIG: &[Case] = &[
    // forbidden_paths 生效
    ("rm -rf repos/readonly-ref/src", MAIN, BLOCK, Sh::Both),
    // extension_agents 生效：登記的擴充 agent 套唯讀角色的規則
    ("awf save-run approve r", "my:auditor", BLOCK, Sh::Both),
    ("echo x > src/a.txt", "my:auditor", BLOCK, Sh::Bash),
    // team_agent_namespaces 生效
    ("awf save-run approve r", "acme:dev-helper", BLOCK, Sh::Both),
    // 自訂的 runs_dir 生效（唯讀角色的 log 要在它底下）
    ("awf run-brief --log work/runs/r/plan/logs/a.log -- npm test", REV, ALLOW, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- npm test", REV, BLOCK, Sh::Both),
    // 其餘照常
    ("echo x > src/a.txt", DEV, ALLOW, Sh::Bash),
    ("rm -rf build", MAIN, ALLOW, Sh::Both),
    ("awf save-run approve r", MINE, ALLOW, Sh::Both),
];

#[test]
fn config_python_reads_stays_effective() {
    let tmp = Tmp::new("oddcfg");
    let ws = workspace_with(&tmp.0, ODD_BUT_VALID);
    let bad = run_table(&ws, ODD_CONFIG);
    assert!(bad.is_empty(), "{} 個案例不符：\n{}", bad.len(), bad.join("\n"));
    // 沒有被當成讀不懂
    let r = hook(&ws, "Bash", MAIN, "rm -rf repos/readonly-ref/src");
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(err.contains("禁止路徑") && !err.contains("讀不懂"), "{err}");
}

/// 真的不是 JSON：模版角色的寫入全擋，主 session 與使用者自己的 agent 照舊。
const BROKEN_CONFIG: &[Case] = &[
    ("echo x > src/a.txt", DEV, BLOCK, Sh::Both),
    ("touch src/a.txt", DEV, BLOCK, Sh::Bash),
    ("git commit -m x", DEV, BLOCK, Sh::Both),
    ("sed -i s/a/b/ src/a.txt", DEV, BLOCK, Sh::Bash),
    ("bash -c 'rm -rf src'", DEV, BLOCK, Sh::Bash),
    ("X=$(touch src/a.txt)", DEV, BLOCK, Sh::Bash),
    ("python3 -c \"open('src/a.txt', 'w').write('x')\"", DEV, BLOCK, Sh::Bash),
    ("'x' | Out-File src/a.txt", DEV, BLOCK, Sh::Ps),
    ("Remove-Item src/a.txt", "agent-workflow:tpm-writer", BLOCK, Sh::Ps),
    // 唯讀的用法照常
    ("git status", DEV, ALLOW, Sh::Both),
    ("ls -la src", DEV, ALLOW, Sh::Bash),
    ("cat README.md | head -5", DEV, ALLOW, Sh::Bash),
    ("cargo test 2>&1 | tail -20", DEV, ALLOW, Sh::Bash),
    ("awf kg locate export", DEV, ALLOW, Sh::Both),
    ("X=$(awf kg locate export)", DEV, ALLOW, Sh::Bash),
    ("awf save-run status r", DEV, ALLOW, Sh::Both),
    ("awf deliverables status", DEV, ALLOW, Sh::Both),
    ("awf state --selftest", DEV, ALLOW, Sh::Both),
    ("awf run-brief --log _tpm/runs/r/plan/logs/a.log -- npm test", DEV, ALLOW, Sh::Both),
    ("Get-Content README.md", DEV, ALLOW, Sh::Ps),
    // 主 session 與使用者自己的 agent：不鎖死
    ("echo x > src/a.txt", MAIN, ALLOW, Sh::Both),
    ("git commit -m x", MAIN, ALLOW, Sh::Both),
    ("touch src/a.txt", MINE, ALLOW, Sh::Bash),
    ("gh pr merge 3 --repo acme-fork/app", MAIN, BLOCK, Sh::Both),
];

#[test]
fn broken_config_blocks_team_writes_only() {
    let tmp = Tmp::new("badcfg");
    let ws = workspace_with(&tmp.0, "{ 壞掉的 json");
    let bad = run_table(&ws, BROKEN_CONFIG);
    assert!(bad.is_empty(), "{} 個案例不符：\n{}", bad.len(), bad.join("\n"));
    let r = hook(&ws, "Bash", DEV, "echo x > src/a.txt");
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(err.contains("讀不懂") && err.contains("JSONDecodeError"), "{err}");
    // Edit／Write：模版角色擋，讀照常；主 session 與使用者自己的 agent 照舊（主 session 要能修設定檔）
    for (tool, rel, agent, want) in [
        ("Write", "src/a.rs", DEV, BLOCK),
        ("Edit", "src/a.rs", DEV, BLOCK),
        ("Write", "_tpm/runs/r/plan/review.md", REV, BLOCK),
        ("Write", "docs/a.md", "agent-workflow:tpm-writer", BLOCK),
        ("Read", "src/a.rs", DEV, ALLOW),
        ("Grep", "src", DEV, ALLOW),
        ("Write", "src/a.rs", MAIN, ALLOW),
        ("Edit", ".claude/teams/guard.config.json", MAIN, ALLOW),
        ("Write", "src/a.rs", MINE, ALLOW),
    ] {
        let r = paths_hook(&ws, tool, agent, &ws.join(rel));
        let err = String::from_utf8_lossy(&r.stderr);
        assert_eq!(r.status.code(), Some(want), "［{tool}／{agent}］{rel}：{err}");
        if want == BLOCK {
            assert!(err.contains("讀不懂"), "［{tool}／{agent}］{rel} 的訊息：{err}");
        }
    }
}
