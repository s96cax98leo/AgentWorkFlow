#!/usr/bin/env python3
"""agent-workflow hooks 的腳本層測試：應擋＝exit 2，應放行＝exit 0。

用法：python3 plugin/hooks/tests/run.py [--project <已 init 的專案目錄>] [--hooks-dir <hooks 目錄>]
  測試在暫存目錄建一個獨立工作區（自己的 guard.config.json、一個假 repo 與 worktree），不碰任何真的 repo。
  schema 與 schemalib 取自 --project 的 .claude/teams/；沒給時取 AgentWorkFlow repo 的 scaffold/.claude/teams/。
  --hooks-dir 指定要測的 hooks（預設是本檔上一層，也就是 plugin/hooks；測 --hooks project 複製出去的那份時用）。
只用標準庫，macOS、Linux、Windows 都能跑；假 repo 的 guard 用 Python 寫，bash 版的轉呼叫只在有 bash 的 POSIX 系統上測。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent.parent
PY = sys.executable

ap = argparse.ArgumentParser()
ap.add_argument("--project")
ap.add_argument("--hooks-dir", default=str(HERE.parent))
args = ap.parse_args()
HOOKS = Path(args.hooks_dir).resolve()
ENTRY = HOOKS / "awf_hook.py"
SRC_TEAMS = Path(args.project) / ".claude" / "teams" if args.project else REPO_ROOT / "scaffold" / ".claude" / "teams"
if not (SRC_TEAMS / "tools" / "schemalib.py").is_file():
    print(f"找不到 {SRC_TEAMS / 'tools' / 'schemalib.py'}")
    sys.exit(1)
if not ENTRY.is_file():
    print(f"找不到 {ENTRY}")
    sys.exit(1)
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass

passed = failed = 0


def check(name: str, want: int, got: int, detail: str = "") -> None:
    global passed, failed
    if got == want:
        passed += 1
    else:
        failed += 1
        print(f"FAIL［{name}］want={want} got={got}" + (f"\n    {detail.strip()[:400]}" if detail else ""))


def git(*a: str, cwd: Path | None = None) -> None:
    subprocess.run(["git", *a], cwd=cwd, check=True, capture_output=True)


def hook(mode: str, data: dict | str, proj: Path) -> subprocess.CompletedProcess:
    raw = data if isinstance(data, str) else json.dumps(data, ensure_ascii=False)
    env = {**os.environ, "CLAUDE_PROJECT_DIR": str(proj), "PYTHONUTF8": "1"}
    return subprocess.run([PY, str(ENTRY), mode], input=raw.encode("utf-8"), capture_output=True, env=env)


def run(mode: str, want: int, name: str, data: dict | str, proj: Path | None = None) -> None:
    r = hook(mode, data, proj or WS)
    check(f"{mode}：{name}", want, r.returncode, r.stderr.decode("utf-8", "replace"))


def sh_in(cwd: Path, command: str, agent: str = "", tool: str = "Bash") -> dict:
    d = {"hook_event_name": "PreToolUse", "tool_name": tool, "cwd": str(cwd), "tool_input": {"command": command}}
    if agent:
        d["agent_type"] = agent
    return d


def ps_in(cwd: Path, command: str, agent: str = "") -> dict:
    return sh_in(cwd, command, agent, tool="PowerShell")


def edit(path: Path | str, agent: str = "", cwd: Path | None = None) -> dict:
    d = {"hook_event_name": "PreToolUse", "tool_name": "Write", "cwd": str(cwd or WS), "tool_input": {"file_path": str(path)}}
    if agent:
        d["agent_type"] = agent
    return d


tmp = Path(tempfile.mkdtemp(prefix="awf-hooktest."))
RB_STATE = tmp / "rb-state"  # 讀取預算的狀態目錄：放在暫存目錄裡，測完一起刪
os.environ["AWF_READ_BUDGET_DIR"] = str(RB_STATE)
try:
    WS = tmp / "ws"
    BARE = tmp / "bare"
    (WS / ".claude" / "teams").mkdir(parents=True)
    BARE.mkdir()
    for sub in ("schemas", "tools"):
        shutil.copytree(SRC_TEAMS / sub, WS / ".claude" / "teams" / sub, ignore=shutil.ignore_patterns("__pycache__"))
    (WS / ".claude" / "teams" / "guard.config.json").write_text(json.dumps({
        "runs_dir": "_tpm/runs",
        "briefs_dir": "_tpm/briefs",
        "repos_dir": "repos",
        "upstream_repos": "acme-up/app",
        "fork_repo": "acme-fork/app",
        "forbidden_paths": "repos/readonly-ref",
        "allowed_branch_prefixes": "feature/,bugfix/,hotfix/,release/,merge/",
        "writer_dirs": ["_tpm/runs", "_tpm/briefs", "docs"],
        "task_id_prefix": "TASK-",
        "task_authority": "任務管理系統",
    }, ensure_ascii=False, indent=2), encoding="utf-8")

    # 假 repo：自己有 guard-git.py／guard-paths.py（模擬 repo 的共用層），外加一個 worktree
    REPO = WS / "repos" / "app"
    (REPO / ".claude" / "hooks").mkdir(parents=True)
    (WS / "repos" / "readonly-ref").mkdir(parents=True)
    (REPO / ".claude" / "hooks" / "guard-git.py").write_text('''# 假的 repo guard：verifier 不能 checkout、pr-reviewer 不能 commit、任何人不能推 main
import json, re, sys
d = json.load(sys.stdin)
a, c = d.get("agent_type", ""), d["tool_input"]["command"]
if a == "verifier" and "git checkout" in c:
    print("repo guard：verifier 不能 checkout", file=sys.stderr); sys.exit(2)
if a == "pr-reviewer" and "git commit" in c:
    print("repo guard：pr-reviewer 不能 commit", file=sys.stderr); sys.exit(2)
if re.search(r"git push [^ ]+ main", c):
    print("repo guard：不推 main", file=sys.stderr); sys.exit(2)
''', encoding="utf-8")
    (REPO / ".claude" / "hooks" / "guard-paths.py").write_text('''# 假的 repo guard：implementer 不改 e2e/、verifier 只寫 verifier.md、spec-author 不改 intent.md、主 session 不改 contracts/
import json, sys
d = json.load(sys.stdin)
a, f = d.get("agent_type", ""), d["tool_input"]["file_path"].replace("\\\\", "/")
def no(m):
    print(m, file=sys.stderr); sys.exit(2)
if a == "implementer" and "/e2e/" in f: no("repo guard：implementer 不改 e2e")
if a == "verifier" and not f.endswith("/verifier.md"): no("repo guard：verifier 只寫 verifier.md")
if a == "spec-author" and f.endswith("/intent.md"): no("repo guard：intent 已核准")
if a == "" and "/contracts/" in f: no("repo guard：契約產物不手改")
''', encoding="utf-8")
    git("init", "-q", "-b", "main", cwd=REPO)
    git("add", "-A", cwd=REPO)
    git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init", cwd=REPO)
    git("remote", "add", "upstream", "https://github.com/acme-up/app.git", cwd=REPO)
    git("remote", "add", "origin", "https://github.com/acme-fork/app.git", cwd=REPO)
    git("worktree", "add", "-q", str(REPO / ".worktrees" / "w1"), "-b", "feature/x--t1", cwd=REPO)

    # ------------------------------------------------------------ Bash 工具
    S = "guard-shell"
    run(S, 2, "推上游 remote", sh_in(REPO, "git push upstream feature/x"))
    run(S, 2, "推上游 URL", sh_in(REPO, "git push https://github.com/acme-up/app.git HEAD:feature/x"))
    run(S, 0, "主 session 推 fork 的 feature", sh_in(REPO, "git push origin feature/x"))
    run(S, 2, "推 fork main（repo guard-git 擋）", sh_in(REPO, "git push origin main"))
    run(S, 2, "agent 不能 push", sh_in(REPO, "git push origin feature/x", "agent-workflow:dev-implementer"))
    run(S, 2, "plugin 命名空間的 agent 不能 push", sh_in(REPO, "git push origin feature/x", "agent-workflow:dev-implementer"))
    run(S, 2, "force push", sh_in(REPO, "git push -f origin feature/x"))
    run(S, 2, "force-with-lease", sh_in(REPO, "git push --force-with-lease origin feature/x"))
    run(S, 2, "+refspec", sh_in(REPO, "git push origin +feature/x"))
    run(S, 2, "--no-verify", sh_in(REPO, "git commit --no-verify -m x"))
    run(S, 2, "gh pr create 上游", sh_in(WS, "gh pr create --repo acme-up/app --title x"))
    run(S, 2, "gh pr create 沒寫 repo", sh_in(REPO, "gh pr create --title x --body y"))
    run(S, 0, "gh pr create 明確寫 fork", sh_in(WS, "gh pr create --repo acme-fork/app --title x"))
    run(S, 0, "gh pr create 明確寫 fork（標題含引號內的 & 與 ;）", sh_in(WS, "gh pr create --title 'a & b; c' --repo acme-fork/app"))
    run(S, 0, "gh api 讀上游", sh_in(WS, "gh api repos/acme-up/app/pulls/1 --jq .head.sha"))
    run(S, 2, "gh api POST 上游", sh_in(WS, "gh api -X POST repos/acme-up/app/issues/1/comments -f body=x"))
    run(S, 2, "gh api -f 隱含 POST", sh_in(WS, "gh api repos/acme-up/app/issues/1/comments -f body=x"))
    run(S, 0, "gh pr view 上游", sh_in(WS, "gh pr view 1 --repo acme-up/app"))
    run(S, 2, "審查者留 PR 留言", sh_in(WS, "gh pr comment 1 --repo acme-fork/app -b x", "agent-workflow:tpm-reviewer"))
    run(S, 2, "agent gh pr update-branch（伺服器端改寫 PR 分支）", sh_in(WS, "gh pr update-branch 3 --repo acme-fork/app --rebase", "agent-workflow:dev-integrator"))
    run(S, 2, "主 session gh pr update-branch 上游", sh_in(WS, "gh pr update-branch 9 --repo acme-up/app"))
    run(S, 2, "主 session gh pr update-branch 沒寫 repo", sh_in(REPO, "gh pr update-branch 3"))
    run(S, 0, "主 session gh pr view（update-branch 規則不誤擋讀取）", sh_in(WS, "gh pr view 3 --repo acme-fork/app"))
    run(S, 2, "rm 禁止路徑", sh_in(WS, "rm -rf repos/readonly-ref/tmp"))
    run(S, 0, "讀禁止路徑", sh_in(WS, "cat repos/readonly-ref/README.md"))
    run(S, 2, "分支前綴不合規（agent）", sh_in(REPO, "git checkout -b ui/x", "agent-workflow:dev-implementer"))
    run(S, 2, "分支前綴不合規（主 session）", sh_in(REPO, "git switch -c wip/x"))
    run(S, 2, "worktree add 分支前綴不合規", sh_in(REPO, "git worktree add .worktrees/y -b tmp/y main"))
    run(S, 0, "分支前綴合規", sh_in(REPO, "git checkout -b feature/y", "agent-workflow:dev-implementer"))
    run(S, 0, "git branch -d 不算新分支", sh_in(REPO, "git branch -d old/x"))
    run(S, 2, "轉呼叫：reviewer commit（git -C）", sh_in(WS, f'git -C "{REPO.as_posix()}" commit -m x', "agent-workflow:tpm-reviewer"))
    run(S, 2, "轉呼叫：verifier checkout", sh_in(REPO, "git checkout main", "agent-workflow:dev-verifier"))
    run(S, 0, "非 git 目錄的一般指令", sh_in(WS, "ls _tpm"))
    run(S, 0, "讀狀態", sh_in(REPO, "git status -sb"))

    # ------------------------------------------------------------ 合併／核准／直接 API 寫入（主 session 也擋）與受保護分支
    # 沒有自己 guard 的 repo，確認擋下的是 plugin hook 本身而不是 repo 的 guard-git
    PLAIN = WS / "repos" / "plain"
    PLAIN.mkdir(parents=True)
    git("init", "-q", "-b", "main", cwd=PLAIN)
    git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init", cwd=PLAIN)
    git("remote", "add", "origin", "https://github.com/acme-fork/app.git", cwd=PLAIN)
    run(S, 2, "主 session gh pr merge fork", sh_in(WS, "gh pr merge 3 --repo acme-fork/app --squash"))
    run(S, 2, "主 session gh pr merge --auto", sh_in(WS, "gh pr merge 3 --auto --repo acme-fork/app"))
    run(S, 2, "主 session gh pr review --approve", sh_in(WS, "gh pr review 3 --approve --repo acme-fork/app"))
    run(S, 2, "主 session gh pr review -a", sh_in(WS, "gh pr review 3 -a --repo acme-fork/app"))
    run(S, 2, "主 session gh pr review -ab 合併短旗標", sh_in(WS, "gh pr review 3 -ab ok --repo acme-fork/app"))
    run(S, 0, "主 session gh pr review --comment", sh_in(WS, "gh pr review 3 --comment -b 看過了 --repo acme-fork/app"))
    run(S, 2, "gh api -X PUT merge", sh_in(WS, "gh api -X PUT repos/acme-fork/app/pulls/3/merge"))
    run(S, 2, "gh api merge，-X PUT 放在 URL 後面", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/merge -X PUT"))
    run(S, 2, "gh api merge，--method=PUT", sh_in(WS, "gh api --method=PUT repos/acme-fork/app/pulls/3/merge"))
    run(S, 2, "gh api merge，URL 帶引號與 &", sh_in(WS, 'gh api "repos/acme-fork/app/pulls/3/merge?a=1&b=2" -X PUT'))
    run(S, 2, "gh api merge，URL 帶未加引號的 &", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/merge?a=1&b=2 -X PUT"))
    run(S, 2, "gh api merge，-f 隱含寫入", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/merge -f merge_method=squash"))
    run(S, 2, "gh api merge，變數拼 URL", sh_in(WS, "U=repos/acme-fork/app/pulls/3/merge; gh api $U -X PUT"))
    run(S, 2, "gh api POST merges（直接合併分支）", sh_in(WS, "gh api repos/acme-fork/app/merges -f base=main -f head=feature/x"))
    run(S, 2, "gh api graphql mergePullRequest", sh_in(WS, "gh api graphql -f query='mutation{mergePullRequest(input:{pullRequestId:\"x\"}){clientMutationId}}'"))
    run(S, 2, "gh api reviews 帶 APPROVE", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews -f event=APPROVE"))
    run(S, 2, "gh api reviews 用 --input（看不到內容）", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews --input r.json"))
    run(S, 0, "gh api reviews 留 COMMENT", sh_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews -f event=COMMENT -f body=x"))
    run(S, 0, "gh api 讀 PR", sh_in(WS, "gh api repos/acme-fork/app/pulls/3 --jq .mergeable"))
    run(S, 2, "curl PUT merge", sh_in(WS, "curl -s -H 'Authorization: token x' https://api.github.com/repos/acme-fork/app/pulls/3/merge -X PUT"))
    run(S, 2, "curl -d 隱含 POST", sh_in(WS, "curl https://api.github.com/repos/acme-fork/app/issues/1/comments -d '{\"body\":\"x\"}'"))
    run(S, 2, "curl --request PATCH", sh_in(WS, "curl --request PATCH https://api.github.com/repos/acme-fork/app/pulls/3 --data-raw x"))
    run(S, 0, "curl 讀 GitHub API", sh_in(WS, "curl -s https://api.github.com/repos/acme-fork/app/pulls/3 | jq .state"))
    run(S, 0, "curl POST 非 GitHub", sh_in(WS, "curl -X POST https://example.com/hook -d x"))
    run(S, 2, "agent gh pr merge", sh_in(WS, "gh pr merge 3 --repo acme-fork/app", "agent-workflow:dev-integrator"))
    run(S, 2, "主 session push fork main（hook 本身擋）", sh_in(PLAIN, "git push origin main"))
    run(S, 2, "主 session push HEAD:main", sh_in(PLAIN, "git push origin HEAD:main"))
    run(S, 2, "主 session push feature:refs/heads/master", sh_in(PLAIN, "git push origin feature/x:refs/heads/master"))
    run(S, 2, "主 session push 沒寫 refspec（目前在 main）", sh_in(PLAIN, "git push origin"))
    run(S, 2, "主 session push --all", sh_in(PLAIN, "git push --all origin"))
    run(S, 0, "主 session push 功能分支", sh_in(PLAIN, "git push -u origin feature/x"))
    run(S, 0, "主 session push 功能分支 HEAD:feature", sh_in(PLAIN, "git push origin HEAD:feature/x"))

    # ------------------------------------------------------------ PowerShell 工具（同一套規則）
    run(S, 2, "PS：推上游 remote", ps_in(REPO, "git push upstream feature/x"))
    run(S, 2, "PS：推上游 URL", ps_in(REPO, "git push https://github.com/acme-up/app.git HEAD:feature/x"))
    run(S, 0, "PS：主 session 推 fork 的 feature", ps_in(REPO, "git push origin feature/x"))
    run(S, 2, "PS：推 fork main（repo guard-git 擋）", ps_in(REPO, "git push origin main"))
    run(S, 2, "PS：agent 不能 push", ps_in(REPO, "git push origin feature/x", "agent-workflow:dev-implementer"))
    run(S, 2, "PS：git.exe 絕對路徑也算 git", ps_in(REPO, "& 'C:\\Program Files\\Git\\cmd\\git.exe' push origin feature/x", "agent-workflow:dev-implementer"))
    run(S, 2, "PS：force push", ps_in(REPO, "git push -f origin feature/x"))
    run(S, 2, "PS：--no-verify", ps_in(REPO, "git commit --no-verify -m x"))
    run(S, 2, "PS：分號後用 & 呼叫 gh merge 上游", ps_in(WS, "git status; & gh pr merge 3 --repo acme-up/app"))
    run(S, 2, "PS：&& 串接", ps_in(WS, "git status && gh pr create --repo acme-up/app --title x"))
    run(S, 2, "PS：管線後的寫入", ps_in(WS, "Get-Content body.md | gh pr comment 1 --repo acme-up/app -F -"))
    run(S, 2, "PS：& 呼叫帶空白路徑的 gh.exe", ps_in(WS, "& 'C:\\Program Files\\GitHub CLI\\gh.exe' pr create --repo acme-up/app --title x"))
    run(S, 2, "PS：反引號續行", ps_in(WS, "gh pr create `\n  --repo acme-up/app `\r\n  --title x"))
    run(S, 2, "PS：反引號拆字", ps_in(REPO, "git pu`sh upstream feature/x"))
    run(S, 2, "PS：Start-Process 呼叫 gh", ps_in(WS, "Start-Process gh -ArgumentList 'pr','create','--repo','acme-up/app'"))
    run(S, 2, "PS：Invoke-Expression 包 push", ps_in(REPO, 'Invoke-Expression "git push upstream feature/x"'))
    run(S, 2, "PS：字串裡的 $() 子運算式", ps_in(WS, 'Write-Output "done: $(gh pr comment 1 --repo acme-up/app -b x)"'))
    run(S, 2, "PS：pwsh -Command 包一層", ps_in(REPO, 'pwsh -NoProfile -Command "git push upstream feature/x"'))
    run(S, 2, "PS：cmd /c 包一層", ps_in(WS, 'cmd /c "gh pr merge 1 --repo acme-up/app"'))
    run(S, 2, "PS：引號內的 & 不拆段（gh api POST）", ps_in(WS, 'gh api -X POST "repos/acme-up/app/issues/1/comments?a=1&b=2" -f body=x'))
    run(S, 0, "PS：gh api 讀上游", ps_in(WS, "gh api repos/acme-up/app/pulls/1 --jq .head.sha"))
    run(S, 0, "PS：gh pr view 上游", ps_in(WS, "gh pr view 1 --repo acme-up/app"))
    run(S, 0, "PS：gh pr create 明確寫 fork（標題含 & 與 ;）", ps_in(WS, 'gh pr create --title "a & b; c" --repo acme-fork/app'))
    run(S, 2, "PS：gh pr create 沒寫 repo", ps_in(REPO, "gh pr create --title x --body y"))
    run(S, 2, "PS：審查者留 PR 留言", ps_in(WS, "gh pr comment 1 --repo acme-fork/app -b x", "agent-workflow:tpm-reviewer"))
    run(S, 2, "PS：agent gh pr update-branch", ps_in(WS, "gh pr update-branch 3 --repo acme-fork/app --rebase", "agent-workflow:dev-integrator"))
    run(S, 2, "PS：主 session gh pr update-branch 上游", ps_in(WS, "gh pr update-branch 9 --repo acme-up/app"))
    run(S, 0, "PS：主 session gh pr view", ps_in(WS, "gh pr view 3 --repo acme-fork/app"))
    run(S, 2, "PS：Remove-Item 禁止路徑", ps_in(WS, "Remove-Item -Recurse -Force repos/readonly-ref/tmp"))
    run(S, 2, "PS：del 別名", ps_in(WS, "del repos\\readonly-ref\\x.md"))
    run(S, 2, "PS：Set-Content 寫禁止路徑", ps_in(WS, "Set-Content -Path repos/readonly-ref/x.md -Value y"))
    run(S, 0, "PS：讀禁止路徑", ps_in(WS, "Get-Content repos/readonly-ref/README.md"))
    run(S, 2, "PS：分支前綴不合規", ps_in(REPO, "git checkout -b wip/x"))
    run(S, 0, "PS：分支前綴合規", ps_in(REPO, "git checkout -b feature/y", "agent-workflow:dev-implementer"))
    run(S, 2, "PS：Set-Location 後轉呼叫 repo guard", ps_in(WS, "Set-Location repos/app; git checkout main", "agent-workflow:dev-verifier"))
    run(S, 0, "PS：讀狀態", ps_in(REPO, "git status -sb"))
    run(S, 0, "PS：一般指令與重新導向", ps_in(WS, "Get-ChildItem _tpm 2>&1 | Select-Object Name"))
    run(S, 2, "PS：主 session gh pr merge fork", ps_in(WS, "gh pr merge 3 --repo acme-fork/app --squash"))
    run(S, 2, "PS：主 session gh pr merge --auto", ps_in(WS, "gh pr merge 3 --auto --repo acme-fork/app"))
    run(S, 2, "PS：主 session gh pr review --approve", ps_in(WS, "gh pr review 3 --approve --repo acme-fork/app"))
    run(S, 2, "PS：主 session gh pr review -a", ps_in(WS, "gh pr review 3 -a --repo acme-fork/app"))
    run(S, 2, "PS：gh pr review -ab 合併短旗標", ps_in(WS, "gh pr review 3 -ab ok --repo acme-fork/app"))
    run(S, 0, "PS：主 session gh pr review --comment", ps_in(WS, "gh pr review 3 --comment -b 看過了 --repo acme-fork/app"))
    run(S, 2, "PS：gh api -X PUT merge", ps_in(WS, "gh api -X PUT repos/acme-fork/app/pulls/3/merge"))
    run(S, 2, "PS：gh api merge，-X PUT 放在 URL 後面", ps_in(WS, "gh api repos/acme-fork/app/pulls/3/merge -X PUT"))
    run(S, 2, "PS：gh api merge，--method=PUT", ps_in(WS, "gh api --method=PUT repos/acme-fork/app/pulls/3/merge"))
    run(S, 2, "PS：gh api merge，URL 帶引號與 &", ps_in(WS, 'gh api "repos/acme-fork/app/pulls/3/merge?a=1&b=2" -X PUT'))
    run(S, 2, "PS：gh api merge，-f 隱含寫入", ps_in(WS, "gh api repos/acme-fork/app/pulls/3/merge -f merge_method=squash"))
    run(S, 2, "PS：gh api merge，變數拼 URL", ps_in(WS, "$u = 'repos/acme-fork/app/pulls/3/merge'; gh api $u -X PUT"))
    run(S, 2, "PS：gh api POST merges（直接合併分支）", ps_in(WS, "gh api repos/acme-fork/app/merges -f base=main -f head=feature/x"))
    run(S, 2, "PS：gh api graphql mergePullRequest", ps_in(WS, "gh api graphql -f query='mutation{mergePullRequest(input:{pullRequestId:\"x\"}){clientMutationId}}'"))
    run(S, 2, "PS：gh api reviews 帶 APPROVE", ps_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews -f event=APPROVE"))
    run(S, 2, "PS：gh api reviews 用 --input（看不到內容）", ps_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews --input r.json"))
    run(S, 0, "PS：gh api reviews 留 COMMENT", ps_in(WS, "gh api repos/acme-fork/app/pulls/3/reviews -f event=COMMENT -f body=x"))
    run(S, 0, "PS：gh api 讀 PR", ps_in(WS, "gh api repos/acme-fork/app/pulls/3 --jq .mergeable"))
    run(S, 2, "PS：Invoke-RestMethod PUT merge", ps_in(WS, "Invoke-RestMethod -Uri https://api.github.com/repos/acme-fork/app/pulls/3/merge -Method Put -Headers $h"))
    run(S, 2, "PS：irm -Body 隱含寫入", ps_in(WS, "irm https://api.github.com/repos/acme-fork/app/issues/1/comments -Method Post -Body $b"))
    run(S, 2, "PS：Invoke-WebRequest PATCH", ps_in(WS, "Invoke-WebRequest -Uri 'https://api.github.com/repos/acme-fork/app/pulls/3' -Method Patch -Body $b"))
    run(S, 2, "PS：curl.exe PUT merge", ps_in(WS, "curl.exe -s https://api.github.com/repos/acme-fork/app/pulls/3/merge -X PUT"))
    run(S, 2, "PS：curl -d 隱含 POST", ps_in(WS, "curl.exe https://api.github.com/repos/acme-fork/app/issues/1/comments -d x"))
    run(S, 0, "PS：Invoke-RestMethod 讀 GitHub API", ps_in(WS, "Invoke-RestMethod https://api.github.com/repos/acme-fork/app/pulls/3 | Select-Object state"))
    run(S, 0, "PS：Invoke-RestMethod POST 非 GitHub", ps_in(WS, "Invoke-RestMethod -Uri https://example.com/hook -Method Post -Body x"))
    run(S, 2, "PS：agent gh pr merge", ps_in(WS, "gh pr merge 3 --repo acme-fork/app", "agent-workflow:dev-integrator"))
    run(S, 2, "PS：主 session push fork main（hook 本身擋）", ps_in(PLAIN, "git push origin main"))
    run(S, 2, "PS：主 session push HEAD:main", ps_in(PLAIN, "git push origin HEAD:main"))
    run(S, 2, "PS：主 session push feature:refs/heads/master", ps_in(PLAIN, "git push origin feature/x:refs/heads/master"))
    run(S, 2, "PS：主 session push 沒寫 refspec（目前在 main）", ps_in(PLAIN, "git push origin"))
    run(S, 2, "PS：主 session push --all", ps_in(PLAIN, "git push --all origin"))
    run(S, 0, "PS：主 session push 功能分支", ps_in(PLAIN, "git push -u origin feature/x"))
    run(S, 0, "PS：主 session push 功能分支 HEAD:feature", ps_in(PLAIN, "git push origin HEAD:feature/x"))

    # ------------------------------------------------------------ Edit／Write
    P = "guard-paths"
    run(P, 2, "寫禁止路徑", edit(WS / "repos/readonly-ref/x.md"))
    run(P, 0, "writer 寫簡報", edit(WS / "_tpm/briefs/x.md", "agent-workflow:tpm-writer"))
    run(P, 0, "writer 寫 docs", edit(WS / "docs/x.md", "agent-workflow:tpm-writer"))
    run(P, 2, "writer 改程式", edit(REPO / "apps/x.ts", "agent-workflow:tpm-writer"))
    run(P, 0, "writer 寫 repo 的 spec", edit(REPO / "specs/demo-x/spec.md", "agent-workflow:tpm-writer"))
    run(P, 0, "writer 寫 worktree 的 spec", edit(REPO / ".worktrees/w1/specs/demo-x/spec.md", "agent-workflow:tpm-writer"))
    run(P, 0, "researcher 寫 run 目錄", edit(WS / "_tpm/runs/r1/notes.md", "agent-workflow:tpm-researcher"))
    run(P, 2, "researcher 寫簡報", edit(WS / "_tpm/briefs/x.md", "agent-workflow:tpm-researcher"))
    run(P, 2, "skeptic 寫程式", edit(REPO / "apps/x.ts", "agent-workflow:tpm-skeptic"))
    run(P, 2, "轉呼叫：implementer 改 e2e", edit(REPO / "e2e/tests/a.spec.ts", "agent-workflow:dev-implementer"))
    run(P, 0, "轉呼叫：implementer 改程式", edit(REPO / "apps/web/src/x.tsx", "agent-workflow:dev-implementer"))
    run(P, 2, "implementer 寫 docs", edit(WS / "docs/x.md", "agent-workflow:dev-implementer"))
    run(P, 2, "implementer 改 .claude", edit(WS / ".claude/teams/rules.md", "agent-workflow:dev-implementer"))
    run(P, 0, "轉呼叫：verifier 寫 verifier.md", edit(REPO / "specs/demo-x/verifier.md", "agent-workflow:dev-verifier"))
    run(P, 2, "轉呼叫：verifier 改程式", edit(REPO / "apps/x.ts", "agent-workflow:dev-verifier"))
    run(P, 2, "轉呼叫：spec-author 改 intent", edit(REPO / "specs/demo-x/intent.md", "agent-workflow:dev-spec-author"))
    run(P, 2, "worktree：implementer 改 e2e", edit(REPO / ".worktrees/w1/e2e/tests/a.spec.ts", "agent-workflow:dev-implementer"))
    run(P, 0, "worktree：implementer 改程式", edit(REPO / ".worktrees/w1/apps/a.ts", "agent-workflow:dev-implementer"))
    git("worktree", "add", "-q", "--detach", str(REPO / ".worktrees" / "w2"), cwd=REPO)
    shutil.rmtree(REPO / ".worktrees" / "w2" / ".claude")
    run(P, 2, "worktree 沒有 guard：改用主 repo 的", edit(REPO / ".worktrees/w2/e2e/tests/a.spec.ts", "agent-workflow:dev-implementer"))
    run(P, 2, "主 session 改契約產物（repo 規則）", edit(REPO / "contracts/api.openapi.yaml"))
    run(P, 0, "主 session 寫工作區文件", edit(WS / "_tpm/briefs/x.md"))
    run(P, 0, "相對路徑以 cwd 解析", edit("notes.md", "agent-workflow:tpm-researcher", WS / "_tpm/runs/r1"))

    # ------------------------------------------------------------ SP6：auto_merge 的有條件開放（Bash 與 PowerShell）
    AMWS = tmp / "ws-am"
    (AMWS / ".claude" / "teams").mkdir(parents=True)
    for sub in ("schemas", "tools"):
        shutil.copytree(SRC_TEAMS / sub, AMWS / ".claude" / "teams" / sub, ignore=shutil.ignore_patterns("__pycache__"))
    STW = "agent-workflow:dev-merge-steward"
    WATCH = "agent-workflow:dev-release-watcher"
    SHA_A, SHA_B, SHA_C = "a" * 40, "b" * 40, "c" * 40
    (AMWS / ".claude" / "teams" / "guard.config.json").write_text(json.dumps({
        "runs_dir": "_tpm/runs",
        "upstream_repos": "acme-up/app,acme-up/lib",
        "fork_repo": "acme-fork/app",
        "auto_merge": {
            "enabled": True,
            "repos": {"acme-fork/app": "low", "acme-up/app": "low", "acme-up/lib": "low"},
            "merger_role": STW,
            "daily_cap": 5,
            "upstream_authorized_by": {"acme-up/app": {"by": "上游維護者（示範客戶 ACME）", "link": "https://example.invalid/auth", "expires": "2999-12-31"},
                                       "acme-up/lib": {"by": "過期的授權", "expires": "2000-01-01"}},
        },
    }, ensure_ascii=False), encoding="utf-8")
    (AMWS / "_tpm" / "runs").mkdir(parents=True)
    AUDIT = AMWS / "_tpm" / "runs" / "merge-audit.jsonl"
    AUDIT.write_text("".join(json.dumps(r) + "\n" for r in (
        {"repo": "acme-fork/app", "pr": 3, "head_sha": SHA_A, "decision": "merge"},
        {"repo": "acme-fork/app", "pr": 4, "head_sha": SHA_A, "decision": "merge"},
        {"repo": "acme-fork/app", "pr": 4, "head_sha": SHA_A, "decision": "hold"},   # 最後一筆是 hold
        {"repo": "acme-up/app", "pr": 9, "head_sha": SHA_C, "decision": "merge"},
        {"repo": "acme-up/lib", "pr": 2, "head_sha": SHA_C, "decision": "merge"},
        {"repo": "acme-fork/other", "pr": 1, "head_sha": SHA_A, "decision": "merge"},
    )), encoding="utf-8")
    FREEZE = AMWS / "_tpm" / "runs" / "auto-merge.freeze.json"
    WRITER = "agent-workflow:tpm-writer"
    M3 = f"gh pr merge 3 --repo acme-fork/app --squash --match-head-commit {SHA_A}"

    def am(want: int, name: str, command: str, agent: str = STW) -> None:
        run(S, want, f"auto_merge：{name}", sh_in(AMWS, command, agent), AMWS)
        run(S, want, f"auto_merge PS：{name}", ps_in(AMWS, command, agent), AMWS)

    am(0, "steward 有 gate 紀錄、SHA 相符 → 合併", M3)
    am(0, "steward 用 --auto 並綁 SHA", f"gh pr merge 3 --auto --squash --repo acme-fork/app --match-head-commit={SHA_A}")
    am(0, "steward 用同 repo 的 PR URL", f"gh pr merge https://github.com/acme-fork/app/pull/3 --squash --match-head-commit {SHA_A}")
    am(0, "上游已授權且有 gate 紀錄", f"gh pr merge 9 --repo acme-up/app --squash --match-head-commit {SHA_C}")
    am(0, "steward 退回 PR（request-changes）", "gh pr review 3 --request-changes --body 需求R2未滿足 --repo acme-fork/app")
    am(0, "steward 留言", "gh pr comment 3 --repo acme-fork/app --body 已開新需求")
    am(0, "steward 寫暫存檔再貼 gate 輸入留言", 'T=$(mktemp) && printf x > "$T" && gh pr comment 3 --repo acme-fork/app --body-file "$T"')
    am(2, "steward 對白名單外的 repo 留言", "gh pr comment 3 --repo acme-fork/other --body x")
    am(0, "steward 轉回 draft", "gh pr ready 3 --undo --repo acme-fork/app")
    am(2, "steward approve", "gh pr review 3 --approve --repo acme-fork/app")
    am(2, "steward review --comment 不在允許清單", "gh pr review 3 --comment -b x --repo acme-fork/app")
    am(2, "steward 關 PR", "gh pr close 3 --repo acme-fork/app")
    am(2, "steward 改 PR", "gh pr edit 3 --repo acme-fork/app --title x")
    am(2, "steward ready 沒有 --undo", "gh pr ready 3 --repo acme-fork/app")
    am(2, "steward 對白名單外的 repo 留言", "gh pr comment 1 --repo acme-fork/other --body x")
    am(2, "steward 沒有 --match-head-commit", "gh pr merge 3 --repo acme-fork/app --squash")
    am(2, "steward head SHA 與 gate 紀錄不符", M3.replace(SHA_A, SHA_B))
    am(2, "steward SHA 不是 40 字元", "gh pr merge 3 --repo acme-fork/app --match-head-commit aaaaaaa")
    am(2, "steward 沒有 gate 紀錄的 PR", f"gh pr merge 5 --repo acme-fork/app --match-head-commit {SHA_A}")
    am(2, "steward 最後一次 gate 判定是 hold", f"gh pr merge 4 --repo acme-fork/app --match-head-commit {SHA_A}")
    am(2, "steward 用 --admin", M3 + " --admin")
    am(2, "steward 合併白名單外的 repo", f"gh pr merge 1 --repo acme-fork/other --match-head-commit {SHA_A}")
    am(2, "steward 用 PR URL 但 --repo 不同", f"gh pr merge https://github.com/acme-fork/other/pull/1 --repo acme-fork/app --match-head-commit {SHA_A}")
    am(2, "steward 用 gh api 合併", "gh api -X PUT repos/acme-fork/app/pulls/3/merge")
    am(2, "steward 合併後再串 API 合併", M3 + " ; gh api -X PUT repos/acme-fork/app/pulls/4/merge")
    am(2, "上游授權已過期", f"gh pr merge 2 --repo acme-up/lib --match-head-commit {SHA_C}")
    am(2, "沒有命名空間的同名 agent（冒充）", M3, "dev-merge-steward")
    am(2, "其他命名空間的同名 agent（冒充）", M3, "other-plugin:dev-merge-steward")
    am(2, "其他 agent 合併", M3, "agent-workflow:dev-integrator")
    am(2, "主 session 合併", M3, "")
    am(2, "主 session approve", "gh pr review 3 --approve --repo acme-fork/app", "")
    am(2, "agent 執行 publish.py", "python3 .claude/teams/tools/publish.py push --repo-dir repos/app --branch feature/x --dry-run", "agent-workflow:dev-integrator")
    am(0, "主 session 執行 publish.py", "python3 .claude/teams/tools/publish.py push --repo-dir repos/app --branch feature/x --dry-run", "")
    am(2, "agent 重新導向寫 merge-audit.jsonl", "echo '{}' >> _tpm/runs/merge-audit.jsonl", "agent-workflow:dev-implementer")
    am(2, "agent 用 tee 寫 guard.config.json", "echo '{}' | tee .claude/teams/guard.config.json", "agent-workflow:dev-implementer")
    am(2, "agent 刪凍結檔", "rm -f _tpm/runs/auto-merge.freeze.json", WRITER)
    am(2, "steward 沒寫合併方式", f"gh pr merge 3 --repo acme-fork/app --match-head-commit {SHA_A}")
    am(2, "steward 合併方式和 merge_method 不同", f"gh pr merge 3 --repo acme-fork/app --rebase --match-head-commit {SHA_A}")
    # A-5：同一旗標重複出現（gh 取最後一個值）一律拒絕，不能只看第一個
    am(2, "A-5 steward --repo 重複（gh 會合併第二個 repo）", f"gh pr merge 3 --repo acme-fork/app --repo acme-fork/other --squash --match-head-commit {SHA_A}")
    am(2, "A-5 steward -R 與 --repo= 混用", f"gh pr merge 3 -R acme-fork/app --repo=acme-fork/other --squash --match-head-commit {SHA_A}")
    am(2, "A-5 steward --repo 與 -R 黏寫值混用", f"gh pr merge 3 --repo acme-fork/app -Racme-fork/other --squash --match-head-commit {SHA_A}")
    am(2, "A-5 steward --match-head-commit 兩次", f"gh pr merge 3 --repo acme-fork/app --squash --match-head-commit {SHA_A} --match-head-commit {SHA_B}")
    am(2, "A-5 steward 留言 --repo 重複", "gh pr comment 3 --repo acme-fork/app --repo acme-fork/other --body x")
    am(0, "A-5 steward 只寫一次 -R 放行", f"gh pr merge 3 -R acme-fork/app --squash --match-head-commit {SHA_A}")
    am(0, "A-5 steward 留言只寫一次 -R 放行", "gh pr comment 3 -R acme-fork/app --body x")
    am(2, "watcher 重跑 CI（gh run rerun）", "gh run rerun 123 --repo acme-fork/app", WATCH)
    am(2, "watcher 留言", "gh pr comment 3 --repo acme-fork/app --body x", WATCH)
    am(2, "watcher 合併", M3, WATCH)
    am(2, "主 session 重新導向寫 merge-audit.jsonl", "echo '{}' >> _tpm/runs/merge-audit.jsonl", "")
    am(0, "agent 讀 merge-audit.jsonl", "tail -n 3 _tpm/runs/merge-audit.jsonl 2>/dev/null", "agent-workflow:dev-implementer")
    am(0, "agent 讀 guard.config.json", "cat .claude/teams/guard.config.json", STW)
    run(S, 2, "auto_merge PS：agent 用 Set-Content 寫 guard.config.json",
        ps_in(AMWS, "Set-Content -Path .claude/teams/guard.config.json -Value '{}'", "agent-workflow:dev-implementer"), AMWS)
    run(S, 2, "auto_merge PS：agent 用 Out-File 寫 merge-audit.jsonl",
        ps_in(AMWS, "'{}' | Out-File -Append _tpm/runs/merge-audit.jsonl", "agent-workflow:dev-implementer"), AMWS)
    FREEZE.write_text(json.dumps({"frozen": True, "reason": "main CI 在合併後轉紅（示範）"}, ensure_ascii=False), encoding="utf-8")
    am(2, "凍結中，steward 合併被擋", M3)
    am(0, "凍結中，steward 仍可退回 PR", "gh pr review 3 --request-changes --body 凍結中 --repo acme-fork/app")
    FREEZE.write_text("壞掉的內容", encoding="utf-8")
    am(2, "凍結檔讀不懂（fail-closed），steward 合併被擋", M3)
    FREEZE.write_text(json.dumps({"frozen": False, "reason": "人已解凍"}, ensure_ascii=False), encoding="utf-8")
    am(0, "凍結檔 frozen=false（人已解凍），steward 可合併", M3)
    FREEZE.unlink()

    def wfreeze(agent: str, body: dict | str, tool: str = "Write") -> dict:
        d = edit(FREEZE, agent, AMWS)
        d["tool_name"] = tool
        d["tool_input"]["content"] = body if isinstance(body, str) else json.dumps(body, ensure_ascii=False)
        return d

    run(P, 0, "auto_merge：tpm-writer 寫凍結旗標 frozen=true", wfreeze(WRITER, {"frozen": True, "reason": "x"}), AMWS)
    run(P, 2, "auto_merge：tpm-writer 不能寫 frozen=false（解凍由人做）", wfreeze(WRITER, {"frozen": False}), AMWS)
    run(P, 2, "auto_merge：tpm-writer 寫的凍結旗標不是 JSON", wfreeze(WRITER, "frozen"), AMWS)
    run(P, 2, "auto_merge：tpm-writer 用 Edit 改凍結檔", wfreeze(WRITER, {"frozen": True}, "Edit"), AMWS)
    run(P, 2, "auto_merge：沒有命名空間的 tpm-writer 不能寫凍結檔", wfreeze("tpm-writer", {"frozen": True}), AMWS)
    run(P, 2, "auto_merge：其他 agent 不能寫凍結檔", wfreeze("agent-workflow:dev-implementer", {"frozen": True}), AMWS)
    run(P, 2, "auto_merge：agent 不能寫 guard.config.json", edit(AMWS / ".claude/teams/guard.config.json", STW, AMWS), AMWS)
    run(P, 2, "auto_merge：tpm agent 不能寫 run 目錄裡的 merge-audit.jsonl", edit(AUDIT, "agent-workflow:tpm-researcher", AMWS), AMWS)
    run(P, 2, "auto_merge：agent 不能寫 publish-audit.jsonl", edit(AMWS / "_tpm/runs/publish-audit.jsonl", "agent-workflow:dev-integrator", AMWS), AMWS)
    run(P, 2, "auto_merge：主 session 不直接改 merge-audit.jsonl", edit(AUDIT, "", AMWS), AMWS)
    run(P, 2, "auto_merge：啟用時主 session 不直接改 guard.config.json", edit(AMWS / ".claude/teams/guard.config.json", "", AMWS), AMWS)
    run(P, 0, "auto_merge：tpm agent 仍可寫 run 目錄的其他檔", edit(AMWS / "_tpm/runs/r1/notes.md", "agent-workflow:tpm-researcher", AMWS), AMWS)
    run(P, 0, "未啟用 auto_merge：主 session 可以改 guard.config.json", edit(WS / ".claude/teams/guard.config.json"))
    run(P, 2, "未啟用 auto_merge：agent 仍不能寫 guard.config.json", edit(WS / ".claude/teams/guard.config.json", "agent-workflow:tpm-writer"))
    run(S, 2, "未啟用 auto_merge：steward 也不能合併", sh_in(WS, M3, STW))
    run(S, 2, "未啟用 auto_merge：PS steward 也不能合併", ps_in(WS, M3, STW))

    # ------------------------------------------------------------ 沒有設定檔的專案：只套用角色規則
    run(S, 0, "無設定：主 session gh 寫入不擋", sh_in(BARE, "gh pr create --title x"), BARE)
    run(S, 0, "無設定：主 session 任意分支名", sh_in(BARE, "git checkout -b wip/x"), BARE)
    run(S, 2, "無設定：agent 仍不能 merge", sh_in(BARE, "gh pr merge 3 --repo acme-fork/app", "agent-workflow:dev-integrator"), BARE)
    run(S, 2, "無設定：PS agent 仍不能 approve", ps_in(BARE, "gh pr review 3 --approve --repo acme-fork/app", "agent-workflow:tpm-reviewer"), BARE)
    run(S, 2, "無設定：agent 不能 curl 寫 GitHub API", sh_in(BARE, "curl -X POST https://api.github.com/repos/acme-fork/app/issues -d x", "agent-workflow:tpm-writer"), BARE)
    run(S, 2, "無設定：agent 仍不能 push", sh_in(BARE, "git push origin x", "agent-workflow:tpm-writer"), BARE)
    run(S, 2, "無設定：PS agent 仍不能 push", ps_in(BARE, "git push origin x", "agent-workflow:tpm-writer"), BARE)
    run(P, 2, "無設定：researcher 只能寫 run 目錄", edit(BARE / "src/a.py", "agent-workflow:tpm-researcher", BARE), BARE)

    # ------------------------------------------------------------ SP9：使用者自己的 agent 不套模版角色規則；主 session 開關
    def mkws(name: str, cfg: dict, with_git: bool = False) -> Path:
        w = tmp / name
        (w / ".claude" / "teams").mkdir(parents=True)
        for sub in ("schemas", "tools"):
            shutil.copytree(SRC_TEAMS / sub, w / ".claude" / "teams" / sub, ignore=shutil.ignore_patterns("__pycache__"))
        base = {"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app", "fork_repo": "acme-fork/app"}
        (w / ".claude" / "teams" / "guard.config.json").write_text(json.dumps({**base, **cfg}, ensure_ascii=False), encoding="utf-8")
        if with_git:  # 目前在 main 的 repo，用來測 push 受保護分支
            git("init", "-q", "-b", "main", cwd=w)
            git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init", cwd=w)
            git("remote", "add", "origin", "https://github.com/acme-fork/app.git", cwd=w)
        return w

    def both(want: int, name: str, w: Path, command: str, agent: str = "") -> None:
        run(S, want, name, sh_in(w, command, agent), w)
        run(S, want, f"PS：{name}", ps_in(w, command, agent), w)

    def stop(agent: str) -> dict:
        return {"hook_event_name": "SubagentStop", "agent_type": agent, "last_assistant_message": "完成，沒有信封"}

    # ------------------------------------------------------------ 0.5 B5：linked_repos（工作區以外的我方 repo）與 session 的 scratchpad
    LINKED, OTHER = tmp / "linked-app", tmp / "not-linked"
    for d_ in (LINKED / "src", LINKED / "e2e", LINKED / "specs" / "demo", LINKED / ".claude" / "teams", OTHER / "src"):
        d_.mkdir(parents=True)
    LWS = mkws("ws-linked", {"linked_repos": [str(LINKED), "", "/"], "acceptance_test_dirs": ["e2e"]})
    IMPL, VER = "agent-workflow:dev-implementer", "agent-workflow:dev-verifier"
    run(P, 0, "B5 linked_repos：implementer 可以改登記過的 repo", edit(LINKED / "src/a.py", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：沒登記的工作區外目錄照舊擋", edit(OTHER / "src/a.py", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：設定裡的「/」不會把整台機器放行", edit(tmp / "anywhere.py", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：linked repo 的 .claude/ 仍不能改", edit(LINKED / ".claude/settings.json", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：linked repo 的 .claude/teams 仍不能改", edit(LINKED / ".claude/teams/rules.md", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：implementer 仍不能改 linked repo 的驗收測試", edit(LINKED / "e2e/a.spec.ts", IMPL, LWS), LWS)
    run(P, 2, "B5 linked_repos：verifier 在 linked repo 仍只能寫自己的報告", edit(LINKED / "src/a.py", VER, LWS), LWS)
    run(P, 0, "B5 linked_repos：verifier 可以寫 linked repo 的 verifier.md", edit(LINKED / "specs/demo/verifier.md", VER, LWS), LWS)
    run(P, 0, "B5 linked_repos：writer 可以寫 linked repo 的 specs/<epic>/*.md", edit(LINKED / "specs/demo/spec.md", "agent-workflow:tpm-writer", LWS), LWS)
    run(P, 2, "B5 linked_repos：writer 不能改 linked repo 的程式", edit(LINKED / "src/a.py", "agent-workflow:tpm-writer", LWS), LWS)
    run(P, 2, "B5 linked_repos：researcher 仍只能寫 run 目錄", edit(LINKED / "src/a.py", "agent-workflow:tpm-researcher", LWS), LWS)
    run(P, 2, "B5 linked_repos：沒設定時工作區外一律擋", edit(LINKED / "src/a.py", IMPL, UWS_PRE := mkws("ws-nolink", {})), UWS_PRE)
    both(2, "B5 linked_repos：agent 在 linked repo 用 shell 寫守門設定仍擋", LWS, f"echo x > {LINKED.as_posix()}/.claude/teams/guard.config.json", IMPL)
    both(2, "B5 linked_repos：agent 在 linked repo 推送仍擋", LWS, f"git -C {LINKED.as_posix()} push origin feature/x", IMPL)
    both(0, "B5 linked_repos：agent 在 linked repo 跑測試放行", LWS, f"git -C {LINKED.as_posix()} status", IMPL)
    SID = "0f3c2a9e-1111-2222-3333-444455556666"
    SCR = tmp / "claude-501" / "-proj" / SID / "scratchpad"
    (SCR / ".claude" / "teams").mkdir(parents=True)
    (tmp / "claude-501" / "-proj" / "other-session-9999" / "scratchpad").mkdir(parents=True)

    def scr(path: Path, agent: str, sid: str = SID) -> dict:
        return {**edit(path, agent, LWS), "session_id": sid}
    run(P, 0, "B5 scratchpad：researcher 可以寫這個 session 的 scratchpad", scr(SCR / "notes.md", "agent-workflow:tpm-researcher"), LWS)
    run(P, 0, "B5 scratchpad：唯讀角色（skeptic）也可以", scr(SCR / "tmp/out.json", "agent-workflow:tpm-skeptic"), LWS)
    run(P, 0, "B5 scratchpad：implementer 可以", scr(SCR / "probe.py", IMPL), LWS)
    run(P, 2, "B5 scratchpad：別的 session 的 scratchpad 不行", scr(tmp / "claude-501" / "-proj" / "other-session-9999" / "scratchpad" / "a.md", "agent-workflow:tpm-researcher"), LWS)
    run(P, 2, "B5 scratchpad：沒有 session_id 時照舊擋", edit(SCR / "notes.md", "agent-workflow:tpm-researcher", LWS), LWS)
    run(P, 2, "B5 scratchpad：暫存目錄的其他位置照舊擋", scr(tmp / "claude-501" / "x.md", "agent-workflow:tpm-researcher"), LWS)
    run(P, 2, "B5 scratchpad：裡面的 .claude/teams、guard.config.json 這類路徑仍擋", scr(SCR / ".claude/teams/guard.config.json", IMPL), LWS)
    run(P, 2, "B5 scratchpad：run 目錄不是 scratchpad，run.json 照舊擋", scr(LWS / "_tpm/runs/x/run.json", IMPL), LWS)

    UWS = mkws("ws-user", {})
    both(0, "使用者 agent dev-helper push 功能分支", UWS, "git push origin feature/x", "dev-helper")
    both(0, "使用者 agent tpm-notes 開 PR（帶 --repo）", UWS, "gh pr create --repo acme-fork/app --title t --body b", "tpm-notes")
    both(2, "使用者 agent 仍不能推上游", UWS, "git push https://github.com/acme-up/app.git HEAD:feature/x", "dev-helper")
    both(2, "使用者 agent 合併照主 session 規則擋（預設）", UWS, "gh pr merge 3 --repo acme-fork/app --squash", "dev-helper")
    both(0, "其他命名空間的 dev-* 不是模版角色，可以 push 功能分支", UWS, "git push origin feature/x", "other-plugin:dev-x")
    both(2, "plugin agent push 照舊擋", UWS, "git push origin feature/x", "agent-workflow:dev-implementer")
    run(P, 0, "使用者 agent dev-helper 寫 src/", edit(UWS / "src/a.py", "dev-helper", UWS), UWS)
    run(P, 0, "使用者 agent tpm-notes 寫 notes/", edit(UWS / "notes/a.md", "tpm-notes", UWS), UWS)
    # 和模版角色同名（RESERVED_ROLES）但沒有命名空間：fail-closed，照模版角色規則
    run(P, 2, "沒有命名空間但和模版角色同名的 tpm-writer 寫 src/ 照擋", edit(UWS / "src/a.py", "tpm-writer", UWS), UWS)
    both(2, "沒有命名空間但和模版角色同名的 dev-implementer push 照擋", UWS, "git push origin feature/x", "dev-implementer")
    both(2, "其他命名空間但和模版角色同名的 dev-implementer push 照擋", UWS, "git push origin feature/x", "other-plugin:dev-implementer")
    run("envelope", 2, "沒有命名空間但和模版角色同名的 dev-verifier 交件要信封", stop("dev-verifier"), UWS)
    run(P, 2, "plugin tpm-researcher 寫 src/ 照舊擋", edit(UWS / "src/a.py", "agent-workflow:tpm-researcher", UWS), UWS)
    run(P, 2, "使用者 agent 也不能寫 guard.config.json", edit(UWS / ".claude/teams/guard.config.json", "dev-helper", UWS), UWS)
    run("envelope", 0, "使用者 agent dev-helper 交件不需信封", stop("dev-helper"), UWS)
    run("envelope", 0, "使用者 agent tpm-notes 交件不需信封", stop("tpm-notes"), UWS)
    run("envelope", 2, "plugin agent 交件仍需信封", stop("agent-workflow:dev-verifier"), UWS)

    LWS = mkws("ws-legacy", {"team_agent_namespaces": ["agent-workflow:", ""]})
    both(2, "team_agent_namespaces 含空字串：dev-helper 回到舊行為（不能 push）", LWS, "git push origin feature/x", "dev-helper")
    both(2, "team_agent_namespaces 含空字串：plugin agent 照舊擋", LWS, "git push origin feature/x", "agent-workflow:dev-implementer")
    both(0, "team_agent_namespaces 含空字串：其他命名空間仍不算", LWS, "git push origin feature/x", "other-plugin:dev-x")
    run(P, 2, "team_agent_namespaces 含空字串：tpm-notes 只能寫 run 目錄", edit(LWS / "src/a.py", "tpm-notes", LWS), LWS)
    run("envelope", 2, "team_agent_namespaces 含空字串：dev-helper 交件要信封", stop("dev-helper"), LWS)
    run(S, 2, "無設定：plugin agent 仍不能 push", sh_in(BARE, "git push origin x", "agent-workflow:dev-implementer"), BARE)
    run(S, 0, "無設定：使用者 agent dev-helper 可以 push", sh_in(BARE, "git push origin x", "dev-helper"), BARE)
    run(P, 0, "無設定：使用者 agent tpm-notes 可以寫 src/", edit(BARE / "src/a.py", "tpm-notes", BARE), BARE)

    # 擴充 agent：使用者自己的 agent 登記在 extension_agents 後，照對應的模版角色規則
    XWS = mkws("ws-ext", {"extension_agents": {"_說明": "x", "acme-db-reviewer": "tpm-reviewer", "my-plugin:sec-check": "dev-security-reviewer",
                                               "acme-sneaky": "dev-merge-steward", "acme-writer": "dev-implementer"}})
    both(2, "擴充 agent（套 tpm-reviewer）不能 push", XWS, "git push origin feature/x", "acme-db-reviewer")
    both(2, "擴充 agent（套 tpm-reviewer）不能開 PR", XWS, "gh pr create --repo acme-fork/app --title t --body b", "acme-db-reviewer")
    both(0, "擴充 agent（套 tpm-reviewer）可以唯讀查 git", XWS, "git log --oneline -5", "acme-db-reviewer")
    run(P, 2, "擴充 agent（套 tpm-reviewer）不能寫 src/", edit(XWS / "src/a.py", "acme-db-reviewer", XWS), XWS)
    run(P, 0, "擴充 agent（套 tpm-reviewer）可以寫 run 目錄", edit(XWS / "_tpm/runs/r/deliver/ext-db.md", "acme-db-reviewer", XWS), XWS)
    both(2, "其他 plugin 的擴充 agent（套 dev-security-reviewer）不能 push", XWS, "git push origin feature/x", "my-plugin:sec-check")
    both(2, "擴充 agent 對應到 merge-steward 不生效：改套 tpm-reviewer，不能合併", XWS, "gh pr merge 3 --repo acme-fork/app --squash --match-head-commit abc", "acme-sneaky")
    run(P, 2, "擴充 agent 對應到 dev-implementer 不生效：改套 tpm-reviewer，不能寫 src/", edit(XWS / "src/a.py", "acme-writer", XWS), XWS)
    run("envelope", 2, "擴充 agent 交件要信封", stop("acme-db-reviewer"), XWS)
    both(0, "沒登記的使用者 agent 照主 session 規則，可以 push 功能分支", XWS, "git push origin feature/x", "dev-helper")
    run("envelope", 0, "沒登記的使用者 agent 交件不需信封", stop("dev-helper"), XWS)

    loose = {"main_session_git_rules": False, "main_session_github_rules": False}
    OWS = mkws("ws-loose", loose, with_git=True)
    both(0, "放寬：主 session gh pr merge", OWS, "gh pr merge 3 --squash")
    both(0, "放寬：主 session gh pr review --approve", OWS, "gh pr review 3 --approve --repo acme-fork/app")
    both(0, "放寬：主 session gh pr create 不寫 --repo", OWS, "gh pr create --title t --body b")
    both(0, "放寬：主 session push main", OWS, "git push origin main")
    both(0, "放寬：主 session 任意分支名", OWS, "git checkout -b wip/x")
    run(S, 0, "放寬：主 session curl PUT merge", sh_in(OWS, "curl https://api.github.com/repos/acme-fork/app/pulls/3/merge -X PUT"), OWS)
    both(0, "放寬：使用者 agent（主 session 規則）gh pr merge", OWS, "gh pr merge 3 --squash", "dev-helper")
    both(2, "放寬：上游一律擋（gh 寫入）", OWS, "gh pr create --repo acme-up/app --title t")
    both(2, "放寬：上游一律擋（push）", OWS, "git push https://github.com/acme-up/app.git HEAD:feature/x")
    run(S, 2, "放寬：上游一律擋（curl）", sh_in(OWS, "curl -X POST https://api.github.com/repos/acme-up/app/issues -d x"), OWS)
    both(2, "放寬：plugin agent 合併照舊擋", OWS, "gh pr merge 3 --repo acme-fork/app --squash", "agent-workflow:dev-integrator")
    both(2, "放寬：plugin agent push 照舊擋", OWS, "git push origin feature/x", "agent-workflow:dev-implementer")
    both(2, "放寬：plugin agent 分支名照舊檢查", OWS, "git checkout -b wip/x", "agent-workflow:dev-implementer")
    GWS = mkws("ws-github-only", {"main_session_github_rules": False})
    both(0, "只放寬 GitHub：主 session gh pr merge", GWS, "gh pr merge 3 --squash")
    both(2, "只放寬 GitHub：主 session 分支名仍檢查", GWS, "git checkout -b wip/x")
    AMLOOSE = mkws("ws-am-loose", {**loose, "auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low"}}}, with_git=True)
    both(2, "放寬但 auto_merge 啟用：主 session 仍不能合併", AMLOOSE, "gh pr merge 3 --squash --repo acme-fork/app")
    both(2, "放寬但 auto_merge 啟用：主 session 仍不能核准", AMLOOSE, "gh pr review 3 --approve --repo acme-fork/app")
    both(2, "放寬但 auto_merge 啟用：主 session 不能直接 push main", AMLOOSE, "git push origin main")
    run(S, 2, "放寬但 auto_merge 啟用：主 session 不能 API 合併", sh_in(AMLOOSE, "gh api -X PUT repos/acme-fork/app/pulls/3/merge"), AMLOOSE)
    both(0, "放寬但 auto_merge 啟用：主 session gh pr create 不寫 --repo", AMLOOSE, "gh pr create --title t --body b")

    # ------------------------------------------------------------ H1：審查實測過的繞過與 fail-open（Bash 與 PowerShell 各一）
    H1W = mkws("ws-h1", {"repos_dir": "repos"})
    H1R = H1W / "repos" / "app"
    H1R.mkdir(parents=True)
    git("init", "-q", "-b", "main", cwd=H1R)
    git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init", cwd=H1R)
    git("remote", "add", "upstream", "https://github.com/acme-up/app.git", cwd=H1R)
    git("remote", "add", "origin", "https://github.com/acme-fork/app.git", cwd=H1R)
    IMP, STW1, WRT = "agent-workflow:dev-implementer", "agent-workflow:dev-merge-steward", "agent-workflow:tpm-writer"

    def pair(want: int, name: str, bash: str, ps: str | None = None, agent: str = "", cwd: Path | None = None, w: Path | None = None) -> None:
        w = w or H1W
        run(S, want, f"H1 {name}", sh_in(cwd or w, bash, agent), w)
        run(S, want, f"H1 PS：{name}", ps_in(cwd or w, bash if ps is None else ps, agent), w)

    def tool(want: int, name: str, tool_name: str, inp: dict, agent: str = "", w: Path | None = None) -> None:
        w = w or H1W
        d = {"hook_event_name": "PreToolUse", "tool_name": tool_name, "cwd": str(w), "tool_input": inp}
        if agent:
            d["agent_type"] = agent
        run(P, want, f"H1 {name}", d, w)

    # H1.1 子 shell、pushd、--git-dir、-C 解析成實際目錄
    pair(2, "子 shell 推上游", "(cd repos/app && git push upstream HEAD:feature/x)", "Push-Location repos/app; git push upstream HEAD:feature/x")
    pair(2, "子 shell 推 main（refspec 不會變成 main)）", "(cd repos/app && git push origin HEAD:main)", "Set-Location repos/app; git push origin HEAD:main")
    pair(2, "pushd 推上游", "pushd repos/app && git push upstream HEAD:feature/x", "pushd repos/app; git push upstream HEAD:feature/x")
    pair(2, "--git-dir 推上游", "git --git-dir=repos/app/.git push upstream HEAD:feature/x")
    pair(2, "-C 推 main", "git -C repos/app push origin HEAD:main")
    pair(0, "子 shell 唯讀放行", "(cd repos/app && git status)", "Push-Location repos/app; git status; Pop-Location")
    pair(0, "子 shell 推 fork 功能分支放行", "(cd repos/app && git push origin HEAD:feature/x)", "Push-Location repos/app; git push origin HEAD:feature/x")
    pair(0, "git -C repo status 放行", "git -C repos/app status")
    # H1.2 send-pack、gh 帳號層級寫入、--repo 夾在子指令中間
    pair(2, "agent send-pack", "git send-pack https://github.com/acme-fork/app.git HEAD:refs/heads/feature/x", agent=IMP, cwd=H1R)
    pair(2, "主 session send-pack 推上游", "git send-pack https://github.com/acme-up/app.git HEAD:refs/heads/feature/x", cwd=H1R)
    pair(2, "agent gh alias set", "gh alias set m 'pr merge'", agent=IMP)
    pair(2, "主 session gh alias set（可繞過守門）", "gh alias set m 'pr merge'")
    pair(0, "主 session gh alias list 放行", "gh alias list")
    pair(2, "agent gh extension install", "gh extension install acme/ext", agent=IMP)
    pair(2, "agent gh ssh-key add", "gh ssh-key add k.pub", agent=IMP)
    pair(2, "agent gh gist create", "gh gist create a.txt", agent=IMP)
    pair(2, "agent gh pr --repo X merge", "gh pr --repo acme-fork/app merge 3", agent=IMP)
    pair(2, "主 session gh pr -R X merge", "gh pr -R acme-fork/app merge 3")
    pair(2, "主 session gh --repo X pr merge", "gh --repo acme-fork/app pr merge 3")
    pair(0, "agent gh pr view 放行", "gh pr view 3 --repo acme-fork/app", agent=IMP)
    # H1.3 直譯器打 merge／reviews 端點
    PYM = "python3 -c \"import urllib.request as u; u.urlopen(u.Request('https://api.github.com/repos/acme-fork/app/pulls/3/merge', method='PUT'))\""
    pair(2, "agent python -c 打 merge", PYM, agent=IMP)
    pair(2, "主 session python -c 打 merge", PYM)
    pair(2, "agent node -e 打 reviews", "node -e \"fetch('https://api.github.com/repos/acme-fork/app/pulls/3/reviews',{method:'POST'})\"", agent=IMP)
    pair(2, "heredoc／管線餵給直譯器打 reviews",
         "python3 - <<'EOF'\nimport urllib.request as u\nu.urlopen('https://api.github.com/repos/acme-fork/app/pulls/3/reviews', b'{}')\nEOF",
         "\"import urllib.request as u; u.urlopen('https://api.github.com/repos/acme-fork/app/pulls/3/reviews', b'{}')\" | python -", agent=IMP)
    pair(0, "python -c 一般計算放行", "python3 -c \"print(1+1)\"", agent=IMP)
    # H1.4 機敏檔：Read、Grep、cat／Get-Content 擋；grep -q 放行
    tool(2, "Read .env（主 session）", "Read", {"file_path": str(H1R / ".env")})
    tool(2, "Read .env（agent）", "Read", {"file_path": str(H1R / ".env")}, IMP)
    tool(2, "Read 私鑰", "Read", {"file_path": str(H1R / "deploy/server.pem")})
    tool(2, "Grep path=.env", "Grep", {"pattern": "X", "path": str(H1R / ".env.local")})
    tool(2, "Grep glob=.env*", "Grep", {"pattern": "X", "glob": ".env*"}, IMP)
    tool(0, "Read .env.example 放行", "Read", {"file_path": str(H1R / ".env.example")})
    tool(0, "agent Read 一般檔放行（不套寫入範圍）", "Read", {"file_path": str(H1W / ".claude/teams/rules.md")}, IMP)
    tool(0, "tpm agent Grep 產品程式放行", "Grep", {"pattern": "X", "path": str(H1R / "src")}, "agent-workflow:tpm-reviewer")
    pair(2, "cat .env", "cat .env", "Get-Content .env", cwd=H1R)
    pair(2, "agent type／tail .env", "tail -n 5 .env", "type .env", agent=IMP, cwd=H1R)
    pair(2, "grep 印出 .env 內容", "grep KEY .env", "Select-String KEY .env", cwd=H1R)
    pair(2, "head 私鑰", "head -n 1 ~/.ssh/id_rsa", "Get-Content -Path ~/.ssh/id_rsa -TotalCount 1")
    pair(0, "grep -q 只判有無放行", "grep -q X .env", "Select-String -Quiet X .env", cwd=H1R)
    pair(0, "cat 一般檔放行", "cat README.md", "Get-Content README.md", cwd=H1R)
    # H1.5 設定檔壞掉：fail-closed
    BRK = mkws("ws-broken", {})
    (BRK / ".claude/teams/guard.config.json").write_text("{ 壞掉的 json", encoding="utf-8")
    pair(2, "設定壞掉：主 session 合併", "gh pr merge 3 --repo acme-fork/app", w=BRK)
    pair(2, "設定壞掉：主 session 核准", "gh pr review 3 --approve --repo acme-fork/app", w=BRK)
    pair(2, "設定壞掉：主 session 寫 merge-audit", "echo x >> _tpm/runs/merge-audit.jsonl", "'x' | Out-File -Append _tpm/runs/merge-audit.jsonl", w=BRK)
    pair(0, "設定壞掉：一般指令放行", "git status", w=BRK)
    tool(2, "設定壞掉：主 session Write merge-audit", "Write", {"file_path": str(BRK / "_tpm/runs/merge-audit.jsonl")}, w=BRK)
    tool(0, "設定壞掉：主 session 可以修設定檔", "Write", {"file_path": str(BRK / ".claude/teams/guard.config.json")}, w=BRK)
    # H1.6 主 session 不能解凍
    FZ = "_tpm/runs/auto-merge.freeze.json"
    pair(2, "主 session rm 凍結檔", f"rm {FZ}", f"Remove-Item {FZ}")
    UNFREEZE, REFREEZE = '{"frozen": false}', '{"frozen": true, "reason": "x"}'
    pair(2, "主 session 寫 frozen:false", f"echo '{UNFREEZE}' > {FZ}", f"Set-Content -Path {FZ} -Value '{UNFREEZE}'")
    pair(0, "主 session 寫 frozen:true（只會更嚴）放行", f"echo '{REFREEZE}' > {FZ}", f"Set-Content -Path {FZ} -Value '{REFREEZE}'")
    pair(0, "主 session 讀凍結檔放行", f"cat {FZ}", f"Get-Content {FZ}")
    tool(2, "主 session Write frozen:false", "Write", {"file_path": str(H1W / FZ), "content": '{"frozen": false}'})
    tool(2, "主 session Edit 凍結檔", "Edit", {"file_path": str(H1W / FZ), "old_string": "true", "new_string": "false"})
    tool(0, "主 session Write frozen:true 放行", "Write", {"file_path": str(H1W / FZ), "content": '{"frozen": true, "reason": "x"}'})
    # H1.7 merge_gate check 只限 steward；agent 不 approve／abandon、不寫 run.json 與稽核紀錄
    pair(2, "implementer 跑 merge_gate check", "python3 .claude/teams/tools/merge_gate.py check --pr 3", agent=IMP)
    pair(0, "steward 跑 merge_gate check 放行", "python3 .claude/teams/tools/merge_gate.py check --pr 3", agent=STW1)
    pair(2, "agent save_run approve", "python3 .claude/teams/tools/save_run.py approve _tpm/runs/x plan --by human --at t", agent=WRT)
    pair(2, "agent save_run abandon", "python3 .claude/teams/tools/save_run.py abandon _tpm/runs/x --reason r --by a --at t", agent=WRT)
    pair(0, "主 session save_run approve 放行", "python3 .claude/teams/tools/save_run.py approve _tpm/runs/x plan --by human --at t")
    # 0.5 B1：狀態機與 workflow 腳本。agent 改得到腳本就等於改了下一段的流程；awf.py approve 會轉給 save_run。
    pair(2, "B1 agent 跑 awf.py approve", "python3 .claude/teams/tools/awf.py approve --run _tpm/runs/x --by human", agent=WRT)
    pair(2, "B1 agent 跑 awf.py done", "python3 .claude/teams/tools/awf.py done --run _tpm/runs/x", agent=WRT)
    pair(2, "B1 agent 跑 awf.py next", "python3 .claude/teams/tools/awf.py next --run _tpm/runs/x", agent=WRT)
    pair(0, "B1 agent 跑 awf.py --selftest 放行", "python3 .claude/teams/tools/awf.py --selftest", agent=WRT)
    pair(0, "B1 agent 讀 awf.py 放行", "grep -n next .claude/teams/tools/awf.py", agent=WRT)
    pair(0, "B1 主 session 跑 awf.py next 放行", "python3 .claude/teams/tools/awf.py next --run _tpm/runs/x")
    pair(2, "B1 agent 用重新導向寫覆寫檔", "echo 'x' > .claude/workflows/sdlc-plan.js", agent=WRT)
    pair(2, "B1 agent 用 cp 蓋掉產生的腳本", "cp /tmp/evil.js _tpm/runs/x/.wf/sdlc-design.js", agent=WRT)
    pair(0, "B1 agent 讀產生的腳本放行", "cat _tpm/runs/x/.wf/sdlc-design.js", agent=WRT)
    pair(0, "B1 主 session 寫覆寫檔放行", "cp a.js .claude/workflows/sdlc-plan.js")
    run(P, 2, "B1 agent 用 Write 寫覆寫檔", edit(WS / ".claude/workflows/sdlc-plan.js", WRT))
    run(P, 2, "B1 agent 用 Write 寫產生的腳本", edit(WS / "_tpm/runs/x/.wf/sdlc-design.js", "agent-workflow:dev-implementer"))
    run(P, 2, "B1 agent 用 Write 寫產生的腳本（大小寫不同）", edit(WS / "_tpm/runs/x/.WF/sdlc-design.js", "agent-workflow:tpm-skeptic"))
    run(P, 0, "B1 主 session 用 Write 寫覆寫檔放行", edit(WS / ".claude/workflows/sdlc-plan.js"))
    run(P, 0, "B1 使用者自己的 agent 寫自己的 workflow 放行", edit(WS / ".claude/workflows/my-flow.js", "my-helper"))
    pair(2, "agent 拆字串寫 merge-audit", "python3 -c \"open('_tpm/runs/merge-'+'audit.jsonl','a').write('x')\"", agent=WRT)
    pair(2, "agent 拆字串寫 run.json", "python3 -c \"open('_tpm/runs/x/run'+'.json','w').write('{}')\"", agent=WRT)
    pair(0, "agent 用 python 讀 run 目錄放行", "python3 -c \"print(open('_tpm/runs/x/notes.md').read())\"", agent=WRT)
    tool(2, "agent Write run.json", "Write", {"file_path": str(H1W / "_tpm/runs/x/run.json")}, WRT)
    tool(0, "agent Write run 目錄其他檔放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/notes.md")}, WRT)
    # H1.8 --project-dir、--runs-dir 只能指本專案
    pair(2, "publish --project-dir 外部", "python3 .claude/teams/tools/publish.py push --project-dir /tmp/fake")
    pair(0, "publish --project-dir 本專案放行", "python3 .claude/teams/tools/publish.py push --project-dir .")
    pair(2, "merge_gate --project-dir 外部", "python3 .claude/teams/tools/merge_gate.py check --project-dir /tmp/fake", agent=STW1)
    pair(2, "merge_gate --runs-dir 外部", "python3 .claude/teams/tools/merge_gate.py check --runs-dir /tmp/fake", agent=STW1)
    # A-5：旗標重複出現時每個值都要檢查，gh 寫入的 --repo／-R 只能寫一次
    pair(2, "A-5 publish --project-dir 重複（argparse 取最後一個）", "python3 .claude/teams/tools/publish.py push --project-dir . --project-dir /tmp/fake")
    pair(2, "A-5 merge_gate --runs-dir 重複", "python3 .claude/teams/tools/merge_gate.py check --runs-dir _tpm/runs --runs-dir /tmp/fake", agent=STW1)
    pair(2, "A-5 主 session gh pr create --repo 重複", "gh pr create --repo acme-fork/app --repo acme-fork/other --title x")
    pair(2, "A-5 主 session gh pr create -R 與 --repo 混用", "gh pr create -R acme-fork/app --repo=acme-fork/other --title x")
    pair(0, "A-5 主 session gh pr create -R 只寫一次放行", "gh pr create -R acme-fork/app --title x")
    # H1.9 唯讀審查角色只寫自己的報告；implementer 不寫驗收測試
    for want, name, ag, rel in [
        (2, "verifier 寫 src", "dev-verifier", "src/a.py"),
        (0, "verifier 寫 verifier.md", "dev-verifier", "specs/e/verifier.md"),
        (0, "verifier 寫分片報告", "dev-verifier", "specs/e/verifier-integration.md"),
        (2, "security-reviewer 寫 tests", "dev-security-reviewer", "tests/t.py"),
        (0, "security-reviewer 寫 security-review.md", "dev-security-reviewer", "specs/e/security-review.md"),
        (2, "design-reviewer 寫 src", "dev-design-reviewer", "src/a.py"),
        (0, "design-reviewer 寫 evidence", "dev-design-reviewer", "specs/e/evidence/a.md"),
        (2, "implementer 寫 tests/acceptance", "dev-implementer", "tests/acceptance/a.spec.ts"),
        (2, "implementer 寫 e2e", "dev-implementer", "e2e/a.spec.ts"),
        (0, "implementer 寫 src", "dev-implementer", "src/a.py"),
        (0, "implementer 寫單元測試", "dev-implementer", "tests/unit/a_test.py"),
    ]:
        tool(want, name, "Write", {"file_path": str(H1R / rel)}, "agent-workflow:" + ag)
    # H1.10 轉呼叫的 repo guard 自己出錯（exit 1）判擋
    ERR = H1W / "repos" / "err"
    (ERR / ".claude" / "hooks").mkdir(parents=True)
    for g in ("guard-git", "guard-paths"):
        (ERR / ".claude" / "hooks" / f"{g}.py").write_text("raise SystemExit(1)\n", encoding="utf-8")
    git("init", "-q", "-b", "main", cwd=ERR)
    pair(2, "轉呼叫的 guard-git exit 1", "git commit -m x", agent=IMP, cwd=ERR)
    tool(2, "轉呼叫的 guard-paths exit 1", "Write", {"file_path": str(ERR / "src/a.py")}, IMP)
    pair(0, "沒有 repo guard 的 repo 照常", "git commit -m x", agent=IMP, cwd=H1R)

    # A-6：大小寫不敏感的檔案系統（macOS、Windows）上，同名異寫指到同一個檔；擋人用的比對一律不分大小寫
    pair(2, "A-6 主 session 重新導向寫 MERGE-AUDIT.jsonl", "echo x >> _tpm/runs/MERGE-AUDIT.jsonl", "'x' | Out-File -Append _tpm/runs/MERGE-AUDIT.jsonl")
    pair(2, "A-6 主 session rm Auto-Merge.Freeze.json（解凍）", "rm _tpm/runs/Auto-Merge.Freeze.json", "Remove-Item _tpm/runs/Auto-Merge.Freeze.json")
    pair(2, "A-6 agent tee 寫 .claude/teams/GUARD.CONFIG.JSON", "echo '{}' | tee .claude/teams/GUARD.CONFIG.JSON",
         "Set-Content -Path .claude/teams/GUARD.CONFIG.JSON -Value '{}'", agent=IMP)
    pair(2, "A-6 cat .ENV", "cat .ENV", "Get-Content .ENV", cwd=H1R)
    pair(2, "A-6 agent head ID_RSA", "head -n 1 ~/.ssh/ID_RSA", "Get-Content -Path ~/.ssh/ID_RSA -TotalCount 1", agent=IMP)
    pair(0, "A-6 讀 MERGE-AUDIT.jsonl 放行", "tail -n 3 _tpm/runs/MERGE-AUDIT.jsonl", "Get-Content _tpm/runs/MERGE-AUDIT.jsonl -Tail 3")
    pair(0, "A-6 cat 一般檔 README.ENV.md 放行", "cat README.ENV.md", "Get-Content README.ENV.md", cwd=H1R)
    tool(2, "A-6 主 session Write MERGE-AUDIT.jsonl", "Write", {"file_path": str(H1W / "_tpm/runs/MERGE-AUDIT.jsonl")})
    tool(2, "A-6 agent Write Publish-Audit.JSONL", "Write", {"file_path": str(H1W / "_tpm/runs/Publish-Audit.JSONL")}, WRT)
    tool(2, "A-6 主 session Write Auto-Merge.Freeze.json frozen:false", "Write",
         {"file_path": str(H1W / "_tpm/runs/Auto-Merge.Freeze.json"), "content": '{"frozen": false}'})
    tool(2, "A-6 agent Write .CLAUDE/teams/guard.config.json", "Write", {"file_path": str(H1W / ".CLAUDE/teams/guard.config.json")}, WRT)
    tool(2, "A-6 Read .ENV", "Read", {"file_path": str(H1R / ".ENV")})
    tool(2, "A-6 Read Server.PEM", "Read", {"file_path": str(H1R / "deploy/Server.PEM")}, IMP)
    tool(2, "A-6 agent Write run 目錄的 RUN.JSON", "Write", {"file_path": str(H1W / "_tpm/runs/x/RUN.JSON")}, WRT)
    tool(2, "A-6 implementer 寫 Tests/Acceptance", "Write", {"file_path": str(H1R / "Tests/Acceptance/a.spec.ts")}, IMP)
    tool(0, "A-6 Read 一般檔 Notes.MD 放行", "Read", {"file_path": str(H1R / "Notes.MD")})
    CWS = mkws("ws-case", {"forbidden_paths": "vendor/readonly-ref"})  # 沒有 repos_dir：dev 角色可寫工作區（.claude/ 除外）
    run(P, 2, "A-6 dev agent 寫 .CLAUDE/hooks/x", edit(CWS / ".CLAUDE/hooks/x.py", IMP, CWS), CWS)
    run(P, 2, "A-6 dev agent 寫 .Claude/settings.json", edit(CWS / ".Claude/settings.json", IMP, CWS), CWS)
    run(P, 0, "A-6 dev agent 寫 src/ 放行", edit(CWS / "src/a.py", IMP, CWS), CWS)
    run(P, 2, "A-6 寫禁止路徑 Vendor/ReadOnly-Ref", edit(CWS / "Vendor/ReadOnly-Ref/a.txt", "", CWS), CWS)
    both(2, "A-6 rm 禁止路徑 VENDOR/readonly-ref", CWS, "rm -rf VENDOR/readonly-ref/tmp")
    both(0, "A-6 讀禁止路徑 VENDOR/readonly-ref 放行", CWS, "cat VENDOR/readonly-ref/README.md")

    # ------------------------------------------------------------ R3：角色合併後的舊名（別名）與凍結檔寫入
    # 別名：dev-pr-reviewer → tpm-reviewer、tpm-upstream-analyst → tpm-researcher，舊名照新角色的規則套用
    for old, new in (("dev-pr-reviewer", "tpm-reviewer"), ("tpm-upstream-analyst", "tpm-researcher")):
        for who in (old, new):
            a = "agent-workflow:" + who
            tag = f"{who}（{'舊名，套 ' + new if who == old else '新名'}）"
            pair(2, f"{tag} 不能 push", "git push origin feature/x", agent=a, cwd=H1R)
            pair(0, f"{tag} 唯讀查 git 放行", "git log --oneline -1", agent=a, cwd=H1R)
            pair(2, f"{tag} 不能對 GitHub 寫入", "gh pr comment 3 --repo acme-fork/app --body x", agent=a)
            pair(0, f"{tag} 讀 PR 放行", "gh pr view 3 --repo acme-fork/app", agent=a)
            pair(2, f"{tag} 不能跑 merge_gate check", "python3 .claude/teams/tools/merge_gate.py check --pr 3", agent=a)
            pair(2, f"{tag} 不能寫 run.json", "python3 -c \"open('_tpm/runs/x/run'+'.json','w').write('{}')\"", agent=a)
            tool(0, f"{tag} 寫 run 目錄放行", "Write", {"file_path": str(H1W / "_tpm/runs/r1/deliver/notes.md")}, a)
            tool(2, f"{tag} 不能寫產品程式", "Write", {"file_path": str(H1R / "src/a.py")}, a)
            tool(2, f"{tag} 不能寫 docs/", "Write", {"file_path": str(H1W / "docs/a.md")}, a)
            tool(2, f"{tag} 不能寫 run.json", "Write", {"file_path": str(H1W / "_tpm/runs/x/run.json")}, a)
        # 轉呼叫 repo guard：舊名和新名一樣對應成 pr-reviewer（假 repo 的 guard 擋 pr-reviewer commit）
        for who in (old, new):
            run(S, 2, f"R3 轉呼叫：{who} commit 照 pr-reviewer 擋", sh_in(REPO, "git commit -m x", "agent-workflow:" + who))
            run(S, 2, f"R3 轉呼叫 PS：{who} commit 照 pr-reviewer 擋", ps_in(REPO, "git commit -m x", "agent-workflow:" + who))
            run(S, 0, f"R3 轉呼叫：{who} git status 放行", sh_in(REPO, "git status -sb", "agent-workflow:" + who))
            run(S, 0, f"R3 轉呼叫 PS：{who} git status 放行", ps_in(REPO, "git status -sb", "agent-workflow:" + who))
    # 擴充 agent 對應到舊名：照新角色（tpm-reviewer）套用，不能寫產品程式
    AXW = mkws("ws-ext-alias", {"extension_agents": {"acme-old-review": "dev-pr-reviewer"}})
    run(P, 2, "R3 擴充 agent 對應舊名 dev-pr-reviewer：照 tpm-reviewer 不能寫 src/", edit(AXW / "src/a.py", "acme-old-review", AXW), AXW)
    run(P, 0, "R3 擴充 agent 對應舊名 dev-pr-reviewer：可以寫 run 目錄", edit(AXW / "_tpm/runs/r/deliver/x.md", "acme-old-review", AXW), AXW)
    both(2, "R3 擴充 agent 對應舊名 dev-pr-reviewer：不能 push", AXW, "git push origin feature/x", "acme-old-review")

    # freeze_file 設在 writer_dirs 以外：tpm-writer 照實際設定的路徑凍結；dev-release-watcher 不再是 freeze_writer
    FWS = mkws("ws-freeze", {"auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low"}, "freeze_file": "ops/merge.freeze.json"}})
    FF = FWS / "ops" / "merge.freeze.json"

    def ftool(want: int, name: str, path: Path, agent: str, content, tool_name: str = "Write") -> None:
        body = content if isinstance(content, str) else json.dumps(content)
        tool(want, name, tool_name, {"file_path": str(path), "content": body}, agent, w=FWS)

    ftool(0, "R3 tpm-writer 寫自訂 freeze_file（frozen=true）", FF, WRT, {"frozen": True, "reason": "x"})
    ftool(2, "R3 tpm-writer 寫自訂 freeze_file 的 frozen=false", FF, WRT, {"frozen": False})
    ftool(2, "R3 tpm-writer 用 Edit 改自訂 freeze_file", FF, WRT, {"frozen": True}, "Edit")
    ftool(2, "R3 tpm-writer 寫同名但不是設定路徑的檔（writer_dirs 以外）", FWS / "src" / "merge.freeze.json", WRT, {"frozen": True})
    ftool(2, "R3 tpm-writer 寫預設路徑以外的一般檔（ops/ 不在 writer_dirs）", FWS / "ops" / "notes.md", WRT, "x")
    ftool(2, "R3 dev-release-watcher 不再能寫凍結檔", FF, "agent-workflow:dev-release-watcher", {"frozen": True})
    ftool(2, "R3 tpm-researcher 不能寫凍結檔", FF, "agent-workflow:tpm-researcher", {"frozen": True})
    pair(2, "R3 tpm-writer 用 shell 寫自訂 freeze_file", "echo '{\"frozen\": true}' > ops/merge.freeze.json",
         "Set-Content -Path ops/merge.freeze.json -Value '{\"frozen\": true}'", agent=WRT, w=FWS)
    pair(0, "R3 tpm-writer 讀自訂 freeze_file", "cat ops/merge.freeze.json", "Get-Content ops/merge.freeze.json", agent=WRT, w=FWS)

    # ------------------------------------------------------------ I6：只寫執行紀錄的角色（awf_config.RECORD_ONLY_ROLES）
    # tpm-reviewer、tpm-skeptic、tpm-planner、dev-release-watcher 有 Write 只為了寫 <runs_dir>/<段>/<label>.md；
    # 其他一律擋（repo 檔、run.json、merge-audit.jsonl、凍結檔、其他 .json），shell 寫檔照唯讀規則擋
    IWS = mkws("ws-records", {"repos_dir": "repos", "auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low"}}})
    (IWS / "repos" / "app").mkdir(parents=True)
    RV, SK, PL, RW = (f"agent-workflow:{r}" for r in ("tpm-reviewer", "tpm-skeptic", "tpm-planner", "dev-release-watcher"))

    def rec(want: int, name: str, path: Path, agent: str, content: str = "# 紀錄\n") -> None:
        tool(want, name, "Write", {"file_path": str(path), "content": content}, agent, w=IWS)

    rec(0, "I6 紀錄 tpm-reviewer 寫 runs_dir 的 .md 放行", IWS / "_tpm/runs/r1/deliver/review-spec.md", RV)
    rec(0, "I6 紀錄 tpm-skeptic 寫 runs_dir 的 .md 放行", IWS / "_tpm/runs/r1/merge/skeptic.md", SK)
    rec(0, "I6 紀錄 tpm-planner 寫 runs_dir 的 .md 放行", IWS / "_tpm/runs/r1/plan/planner.md", PL)
    rec(0, "I6 紀錄 dev-release-watcher 寫 runs_dir 的 .md 放行", IWS / "_tpm/runs/r1/post-merge/watcher.md", RW)
    rec(2, "I6 紀錄 dev-release-watcher 寫 repo 檔擋", IWS / "repos/app/src/a.py", RW)
    rec(2, "I6 紀錄 dev-release-watcher 寫 repo 的 .md 擋", IWS / "repos/app/docs/a.md", RW)
    rec(2, "I6 紀錄 tpm-reviewer 寫 repo 檔擋", IWS / "repos/app/src/a.py", RV)
    rec(2, "I6 紀錄 tpm-planner 寫 docs/ 擋", IWS / "docs/a.md", PL)
    rec(2, "I6 紀錄 tpm-skeptic 寫 run.json 擋", IWS / "_tpm/runs/r1/run.json", SK, "{}")
    rec(2, "I6 紀錄 tpm-reviewer 寫 runs_dir 的其他 .json 擋", IWS / "_tpm/runs/r1/deliver/verdict.json", RV, "{}")
    rec(2, "I6 紀錄 tpm-reviewer 寫 merge-audit.jsonl 擋", IWS / "_tpm/runs/merge-audit.jsonl", RV, "{}\n")
    rec(2, "I6 紀錄 dev-release-watcher 寫 merge-audit.jsonl 擋", IWS / "_tpm/runs/merge-audit.jsonl", RW, "{}\n")
    rec(2, "I6 紀錄 dev-release-watcher 寫凍結檔擋（即使 frozen=true）", IWS / "_tpm/runs/auto-merge.freeze.json", RW, '{"frozen": true}')
    rec(2, "I6 紀錄 tpm-skeptic 寫凍結檔擋", IWS / "_tpm/runs/auto-merge.freeze.json", SK, '{"frozen": true}')
    rec(2, "I6 紀錄 tpm-planner 用 ../ 跳出 runs_dir 擋", IWS / "_tpm/runs/r1/../../../repos/app/x.md", PL)
    rec(2, "I6 紀錄 tpm-reviewer 寫 .MD（大小寫不同，白名單照原樣比對）擋", IWS / "_tpm/runs/r1/x.MD", RV)
    tool(2, "I6 紀錄 tpm-reviewer 用 Edit 改 repo 檔擋", "Edit", {"file_path": str(IWS / "repos/app/src/a.py")}, RV, w=IWS)
    for who in (RV, SK, PL, RW):
        short = who.rsplit(":", 1)[-1]
        run(S, 2, f"I6 紀錄 {short} 用 Bash 重導寫檔擋", sh_in(IWS, "echo x > _tpm/runs/r1/x.md", who), IWS)
        run(S, 2, f"I6 紀錄 PS：{short} 用 Set-Content 寫檔擋", ps_in(IWS, "Set-Content -Path _tpm/runs/r1/x.md -Value x", who), IWS)
    run(S, 2, "I6 紀錄 tpm-reviewer 用 Bash tee 寫 log 擋", sh_in(IWS, "npm test 2>&1 | tee test.log", RV), IWS)
    run(S, 2, "I6 紀錄 tpm-skeptic 用 Bash >> 附加寫檔擋", sh_in(IWS, "git log -1 >> notes.txt", SK), IWS)
    run(S, 2, "I6 紀錄 PS：dev-release-watcher 用 Out-File 寫檔擋", ps_in(IWS, "gh run list | Out-File -FilePath runs.txt", RW), IWS)
    run(S, 2, "I6 紀錄 PS：tpm-planner 用 > 重導寫檔擋", ps_in(IWS, "Get-Date > _tpm/runs/r1/x.md", PL), IWS)
    run(S, 0, "I6 紀錄 tpm-reviewer 丟 /dev/null 與 2>&1 放行", sh_in(IWS, "npm test > /dev/null 2>&1", RV), IWS)
    run(S, 0, "I6 紀錄 PS：tpm-reviewer 丟 $null 放行", ps_in(IWS, "npm test > $null", RV), IWS)
    run(S, 0, "I6 紀錄 tpm-skeptic 引號內的 > 不是重導，放行", sh_in(IWS, "jq '.items | length > 1' a.json", SK), IWS)
    run(S, 0, "I6 紀錄 dev-release-watcher 讀 CI 狀態放行", sh_in(IWS, "gh run list --repo acme-fork/app --limit 5", RW), IWS)
    run(S, 0, "I6 紀錄 對照：tpm-writer 不在名單，shell 規則不變", sh_in(IWS, "echo x > _tpm/runs/r1/x.md", WRT), IWS)
    run(S, 0, "I6 紀錄 對照：tpm-researcher 不在名單，shell 規則不變", sh_in(IWS, "echo x > _tpm/runs/r1/x.md", "agent-workflow:tpm-researcher"), IWS)
    rec(0, "I6 紀錄 舊名 dev-pr-reviewer（套 tpm-reviewer）寫 .md 放行", IWS / "_tpm/runs/r1/deliver/pr.md", "agent-workflow:dev-pr-reviewer")
    rec(2, "I6 紀錄 舊名 dev-pr-reviewer（套 tpm-reviewer）寫 .json 擋", IWS / "_tpm/runs/r1/deliver/x.json", "agent-workflow:dev-pr-reviewer", "{}")

    # ------------------------------------------------------------ 審查修正：I2 save_run 寫入子指令、I3 紀錄角色 shell 寫檔、
    # I4 紀錄角色 Write 範圍、B2 .claude/teams 與 methodology.json、I5 runs_dir 的 <段>.json
    SR = "python3 .claude/teams/tools/save_run.py"
    RSR = "agent-workflow:tpm-researcher"
    # I2：team agent 對 save_run.py 只能跑 status、--selftest
    for sub in ("stage _tpm/runs/x test", "escalate _tpm/runs/x --recipe feature --by a --at t --reason r",
                "followup _tpm/runs/x --run _tpm/runs/y --by a --at t", "close _tpm/runs/x --by a --at t", "init _tpm/runs/x --event sdlc"):
        pair(2, f"I2 agent save_run {sub.split()[0]} 擋", f"{SR} {sub}", agent=WRT)
    pair(2, "I2 tpm-researcher save_run stage（< 輸入）擋", f"{SR} stage _tpm/runs/x test < r.json", agent=RSR)
    pair(2, "I2 agent 用 python -c import save_run 擋", "python3 -c \"import sys; sys.path.insert(0,'t'); import save_run; save_run.main(['stage'])\"", agent=IMP)
    pair(0, "I2 agent save_run status 放行", f"{SR} status _tpm/runs/x", agent=WRT)
    pair(0, "I2 agent save_run --selftest 放行", f"{SR} --selftest", agent=IMP)
    pair(0, "I2 agent 讀 save_run.py 原始碼放行", "grep -n stage .claude/teams/tools/save_run.py", "Select-String stage .claude/teams/tools/save_run.py", agent=RSR)
    pair(0, "I2 主 session save_run stage 放行", f"{SR} stage _tpm/runs/x test < r.json", f"Get-Content r.json | {SR} stage _tpm/runs/x test")
    pair(0, "I2 主 session save_run escalate 放行", f"{SR} escalate _tpm/runs/x --recipe feature --by a --at t --reason r")

    # I3：紀錄角色（tpm-reviewer、tpm-skeptic、tpm-planner、dev-release-watcher）在 shell 一律不能寫檔
    for want, name, bash, ps in [
        (2, "cp 到 src", "cp x src/app.py", "Copy-Item x src/app.py"),
        (2, "mv", "mv a.py b.py", "Move-Item a.py b.py"),
        (2, "python -c 寫檔", "python3 -c 'open(\"f\",\"w\").write(\"1\")'", "python3 -c 'open(\"f\",\"w\").write(\"1\")'"),
        (2, "node -e 寫檔", "node -e \"require('fs').writeFileSync('f','x')\"", "node -e \"require('fs').writeFileSync('f','x')\""),
        (2, "sed -i", "sed -i 's/a/b/' src/a.py", "sed -i 's/a/b/' src/a.py"),
        (2, "perl -pi", "perl -pi -e 's/a/b/' src/a.py", "perl -pi -e 's/a/b/' src/a.py"),
        (2, "install／rsync", "install -m 644 x src/a.py", "rsync x src/a.py"),
        (2, "dd of=", "dd if=/dev/zero of=src/a.py count=1", "dd if=x of=src/a.py"),
        (2, "truncate／touch", "truncate -s 0 src/a.py", "touch src/a.py"),
        (2, "ln", "ln -sf /etc/passwd src/a.py", "ln -s x y"),
        (2, "git checkout／restore 寫工作樹", "git checkout -- src/a.py", "git restore src/a.py"),
        (2, "git apply", "git apply fix.patch", "git apply fix.patch"),
        (2, "bash -c 包一層 cp", "bash -c 'cp x src/a.py'", "bash -c 'cp x src/a.py'"),
        (2, "Set-Content／New-Item", "tee src/a.py < x", "New-Item -Path src/a.py -Value x"),
        (2, "PS Out-File／Rename-Item", "echo x | tee -a src/a.py", "Rename-Item a.py b.py"),
        (2, "run_brief --log 不在 runs_dir", "python3 .claude/teams/tools/run_brief.py --log src/x.log -- npm test",
         "python3 .claude/teams/tools/run_brief.py --log src/x.log -- npm test"),
        (2, "run_brief --log 不是 .log", "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/x.md -- npm test",
         "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/x.md -- npm test"),
        (2, "run_brief 沒帶 --log", "python3 .claude/teams/tools/run_brief.py -- npm test", "python3 .claude/teams/tools/run_brief.py -- npm test"),
        (2, "run_brief 內層指令寫檔", "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/a.log -- cp x src/a.py",
         "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/a.log -- cp x src/a.py"),
        (0, "跑測試放行", "npm test", "npm test"),
        (0, "run_brief --log 在 runs_dir 的 .log 放行", "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/a.log -- npm test",
         "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/r1/test/logs/a.log -- npm test"),
        (0, "唯讀 git 放行", "git diff --stat HEAD~1", "git log --oneline -3"),
        (0, "git apply --check 放行", "git apply --check fix.patch", "git apply --stat fix.patch"),
        (0, "python -c 讀檔放行", "python3 -c \"print(open('a.txt').read())\"", "python3 -c \"print(open('a.txt').read())\""),
    ]:
        for who in (RV, RW):
            short = who.rsplit(":", 1)[-1]
            run(S, want, f"I3 {short} {name}", sh_in(IWS, bash, who), IWS)
            run(S, want, f"I3 PS：{short} {name}", ps_in(IWS, ps, who), IWS)
    run(S, 2, "I3 tpm-skeptic heredoc 餵 python 寫檔", sh_in(IWS, "python3 - <<'EOF'\nopen('src/a.py','w').write('x')\nEOF", SK), IWS)
    run(S, 0, "I3 對照：dev-implementer 用 cp 不受紀錄角色規則限制", sh_in(IWS, "cp x repos/app/src/a.py", IMP), IWS)
    run(S, 0, "I3 PS：對照：dev-implementer 用 Copy-Item 不受紀錄角色規則限制", ps_in(IWS, "Copy-Item x repos/app/src/a.py", IMP), IWS)

    # I4：紀錄角色的 Write 只能寫 <runs_dir>/<run>/<段>/<檔名>.md
    rec(0, "I4 紀錄 tpm-skeptic 寫 <run>/test/<label>.md 放行", IWS / "_tpm/runs/r1/test/skeptic-1.md", SK)
    rec(0, "I4 紀錄 tpm-reviewer 寫 <run>/pr-review/<label>.md 放行", IWS / "_tpm/runs/r1/pr-review/lens-bugs.md", RV)
    rec(0, "I4 紀錄 tpm-planner 寫 <run>/design-revise/<label>.md 放行", IWS / "_tpm/runs/r1/design-revise/p.md", PL)
    rec(2, "I4 紀錄 tpm-reviewer 寫 runs_dir 根的 .md 擋", IWS / "_tpm/runs/notes.md", RV)
    rec(2, "I4 紀錄 tpm-reviewer 寫 <run>/x.md（少一層）擋", IWS / "_tpm/runs/r1/x.md", RV)
    rec(2, "I4 紀錄 tpm-skeptic 寫不是段名的目錄擋", IWS / "_tpm/runs/r1/evil/x.md", SK)
    rec(2, "I4 紀錄 tpm-planner 寫 context.md 擋", IWS / "_tpm/runs/r1/plan/context.md", PL)
    rec(2, "I4 紀錄 dev-release-watcher 寫 執行紀錄索引.md 擋", IWS / "_tpm/runs/r1/post-merge/執行紀錄索引.md", RW)
    rec(2, "I4 紀錄 tpm-reviewer 寫 <run>/context.md 擋", IWS / "_tpm/runs/r1/context.md", RV)

    # B2：team agent 寫任何位置的 .claude/teams/、methodology.json、guard.config.json 一律擋；主 session 不受影響
    for who in (RSR, WRT, IMP, RV):
        short = who.rsplit(":", 1)[-1]
        tool(2, f"B2 {short} 寫 runs_dir 下的 .claude/teams/methodology.json", "Write",
             {"file_path": str(H1W / "_tpm/runs/.claude/teams/methodology.json"), "content": "{}"}, who)
    tool(2, "B2 dev-implementer 寫 repo 裡的 .claude/teams/rules.md", "Write", {"file_path": str(H1R / ".claude/teams/rules.md")}, IMP)
    tool(2, "B2 tpm-writer 寫 docs/methodology.json", "Write", {"file_path": str(H1W / "docs/methodology.json"), "content": "{}"}, WRT)
    tool(2, "B2 tpm-researcher 用 Edit 改 runs_dir 下的 guard.config.json", "Edit", {"file_path": str(H1W / "_tpm/runs/x/guard.config.json")}, RSR)
    tool(2, "B2 dev-implementer 用 NotebookEdit 寫 .Claude/Teams/ 底下（大小寫不同）", "NotebookEdit",
         {"notebook_path": str(H1R / ".Claude/Teams/x.ipynb")}, IMP)
    tool(0, "B2 tpm-researcher 寫 runs_dir 一般 .md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/research/a.md")}, RSR)
    tool(0, "B2 dev-implementer 寫 repo 一般檔放行", "Write", {"file_path": str(H1R / "src/a.py")}, IMP)
    tool(0, "B2 主 session 寫 .claude/teams/methodology.json 放行", "Write", {"file_path": str(H1W / ".claude/teams/methodology.json"), "content": "{}"})
    pair(2, "B2 tpm-researcher 用 shell 寫 runs_dir 下的 methodology.json",
         "mkdir -p _tpm/runs/.claude/teams && echo '{}' > _tpm/runs/.claude/teams/methodology.json",
         "Set-Content -Path _tpm/runs/.claude/teams/methodology.json -Value '{}'", agent=RSR)
    pair(2, "B2 dev-implementer 用 cp 蓋 .claude/teams/ 底下的檔", "cp x.md repos/app/.claude/teams/rules.md",
         "Copy-Item x.md repos/app/.claude/teams/rules.md", agent=IMP)
    pair(0, "B2 dev-implementer 跑 .claude/teams/tools 的工具放行",
         "python3 .claude/teams/tools/run_brief.py --log _tpm/runs/x/build/logs/a.log -- npm install", agent=IMP)
    pair(0, "B2 tpm-researcher 讀 methodology.json 放行", "cat .claude/teams/methodology.json", "Get-Content .claude/teams/methodology.json", agent=RSR)
    pair(0, "B2 主 session 用 shell 寫 methodology.json 放行", "echo '{}' > .claude/teams/methodology.json",
         "Set-Content -Path .claude/teams/methodology.json -Value '{}'")

    # I5：runs_dir 底下的 run.json、<段>.json、merge-audit.jsonl、凍結檔，所有 team agent 都不能寫
    for who in (RSR, WRT):
        short = who.rsplit(":", 1)[-1]
        for f in ("x/test.json", "x/deliver.json", "x/plan.json", "x/post-merge.json"):
            tool(2, f"I5 {short} Write {f}", "Write", {"file_path": str(H1W / "_tpm/runs" / f), "content": "{}"}, who)
        tool(2, f"I5 {short} Edit x/run.json", "Edit", {"file_path": str(H1W / "_tpm/runs/x/run.json")}, who)
        tool(2, f"I5 {short} Write merge-audit.jsonl", "Write", {"file_path": str(H1W / "_tpm/runs/merge-audit.jsonl")}, who)
    tool(2, "I5 tpm-researcher Write 凍結檔", "Write", {"file_path": str(H1W / "_tpm/runs/auto-merge.freeze.json"), "content": '{"frozen": true}'}, RSR)
    tool(0, "I5 tpm-researcher Write runs_dir 的一般 .json 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/research/data.json"), "content": "{}"}, RSR)
    tool(0, "I5 dev-implementer 寫 repo 的 test.json 放行（不在 runs_dir）", "Write", {"file_path": str(H1R / "fixtures/test.json"), "content": "{}"}, IMP)
    tool(0, "I5 主 session Write runs_dir 的 test.json 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/test.json"), "content": "{}"})
    pair(2, "I5 tpm-researcher 用 shell 寫 test.json", "echo '{}' > _tpm/runs/x/test.json", "Set-Content -Path _tpm/runs/x/test.json -Value '{}'", agent=RSR)
    pair(2, "I5 tpm-writer 用 cp 蓋 deliver.json", "cp a.json _tpm/runs/x/deliver.json", "Copy-Item a.json _tpm/runs/x/deliver.json", agent=WRT)
    pair(2, "I5 tpm-writer cd 進 run 目錄再寫 test.json", "cd _tpm/runs/x && echo '{}' > test.json",
         "Set-Location _tpm/runs/x; Set-Content -Path test.json -Value '{}'", agent=WRT)
    pair(0, "I5 tpm-researcher 讀 test.json 放行", "cat _tpm/runs/x/test.json", "Get-Content _tpm/runs/x/test.json", agent=RSR)
    pair(0, "I5 主 session 用 shell 寫 test.json 放行", "echo '{}' > _tpm/runs/x/test.json", "Set-Content -Path _tpm/runs/x/test.json -Value '{}'")

    # I7：runs_dir 底下的 context.md 與任何位置的 執行紀錄索引.md，所有 team agent 都不能寫；主 session 放行
    for who in (WRT, IMP):
        short = who.rsplit(":", 1)[-1]
        tool(2, f"I7 {short} Write context.md", "Write", {"file_path": str(H1W / "_tpm/runs/x/context.md")}, who)
        tool(2, f"I7 {short} Edit context.md", "Edit", {"file_path": str(H1W / "_tpm/runs/x/context.md")}, who)
        tool(2, f"I7 {short} Write 執行紀錄索引.md", "Write", {"file_path": str(H1W / "_tpm/runs/x/執行紀錄索引.md")}, who)
        tool(2, f"I7 {short} Write repo 裡的 執行紀錄索引.md", "Write", {"file_path": str(H1R / "docs/執行紀錄索引.md")}, who)
        pair(2, f"I7 {short} 用 shell 寫 context.md", "echo x >> _tpm/runs/x/context.md",
             "Add-Content -Path _tpm/runs/x/context.md -Value x", agent=who)
        pair(2, f"I7 {short} cd 進 run 目錄再寫 context.md", "cd _tpm/runs/x && echo x > context.md",
             "Set-Location _tpm/runs/x; Set-Content -Path context.md -Value x", agent=who)
        pair(2, f"I7 {short} 用 shell 寫 執行紀錄索引.md", "cp a.md repos/app/執行紀錄索引.md",
             "Copy-Item a.md repos/app/執行紀錄索引.md", agent=who)
        pair(0, f"I7 {short} 讀 context.md 放行", "cat _tpm/runs/x/context.md", "Get-Content _tpm/runs/x/context.md", agent=who)
    # B13 決策記憶：decisions.jsonl 與 決策記憶.md 只由主 session 跑 save_run.py 寫（資料包會把它帶給之後的 agent）
    for who in (WRT, IMP):
        short = who.rsplit(":", 1)[-1]
        tool(2, f"B13 {short} Write decisions.jsonl", "Write", {"file_path": str(H1W / "_tpm/runs/decisions.jsonl")}, who)
        tool(2, f"B13 {short} Edit 決策記憶.md", "Edit", {"file_path": str(H1W / "_tpm/runs/決策記憶.md")}, who)
        pair(2, f"B13 {short} 用 shell 寫 decisions.jsonl", "echo '{}' >> _tpm/runs/decisions.jsonl",
             "Add-Content -Path _tpm/runs/decisions.jsonl -Value '{}'", agent=who)
        pair(2, f"B13 {short} 跑 save_run.py decide", "python3 .claude/teams/tools/save_run.py decide _tpm/runs/x --question q --answer a --by human --at t",
             "python .claude/teams/tools/save_run.py decide _tpm/runs/x --question q --answer a --by human --at t", agent=who)
        pair(0, f"B13 {short} 讀 decisions.jsonl 放行", "cat _tpm/runs/decisions.jsonl", "Get-Content _tpm/runs/decisions.jsonl", agent=who)
    tool(0, "B13 主 session Write decisions.jsonl 放行", "Write", {"file_path": str(H1W / "_tpm/runs/decisions.jsonl")})
    pair(0, "B13 主 session 跑 save_run.py decide 放行", "python3 .claude/teams/tools/save_run.py decide _tpm/runs/x --question q --answer a --by human --at t",
         "python .claude/teams/tools/save_run.py decide _tpm/runs/x --question q --answer a --by human --at t")
    tool(0, "B13 tpm-writer 寫 <run>/decision/決策紀錄.md 放行（事件產出，不是決策記憶檔）", "Write", {"file_path": str(H1W / "_tpm/runs/x/decision/決策紀錄.md")}, WRT)
    tool(0, "I7 tpm-writer 寫 runs_dir/<run>/deliver/x.md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/deliver/x.md")}, WRT)
    tool(0, "I7 dev-implementer 寫 repo 的 context.md 放行（不在 runs_dir）", "Write", {"file_path": str(H1R / "docs/context.md")}, IMP)
    tool(0, "I7 主 session Write context.md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/context.md")})
    tool(0, "I7 主 session Write 執行紀錄索引.md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/執行紀錄索引.md")})
    pair(0, "I7 tpm-writer 用 shell 寫 runs_dir/<run>/deliver/x.md 放行", "echo x > _tpm/runs/x/deliver/x.md",
         "Set-Content -Path _tpm/runs/x/deliver/x.md -Value x", agent=WRT)
    pair(0, "I7 主 session 用 shell 寫 context.md 放行", "echo x > _tpm/runs/x/context.md", "Set-Content -Path _tpm/runs/x/context.md -Value x")

    # 0.4：資料包（pack.md）、調研重用紀錄（reuse.json、pack-args.json）只由主 session 跑 pack.py 產生
    for who in (WRT, RSR):
        short = who.rsplit(":", 1)[-1]
        tool(2, f"I8 {short} Write pack.md", "Write", {"file_path": str(H1W / "_tpm/runs/x/plan/pack.md")}, who)
        tool(2, f"I8 {short} Write reuse.json", "Write", {"file_path": str(H1W / "_tpm/runs/x/plan/reuse.json")}, who)
        tool(2, f"I8 {short} Edit pack-args.json", "Edit", {"file_path": str(H1W / "_tpm/runs/x/plan/pack-args.json")}, who)
        pair(2, f"I8 {short} 用 shell 寫 reuse.json", "echo '{}' > _tpm/runs/x/plan/reuse.json",
             "Set-Content -Path _tpm/runs/x/plan/reuse.json -Value '{}'", agent=who)
        pair(2, f"I8 {short} 用 shell 寫 pack.md", "echo x >> _tpm/runs/x/plan/pack.md",
             "Add-Content -Path _tpm/runs/x/plan/pack.md -Value x", agent=who)
        pair(2, f"I8 {short} 跑 pack.py", "python3 .claude/teams/tools/pack.py plan --run _tpm/runs/x",
             "python .claude/teams/tools/pack.py plan --run _tpm/runs/x", agent=who)
        pair(0, f"I8 {short} 讀 pack.md 放行", "cat _tpm/runs/x/plan/pack.md", "Get-Content _tpm/runs/x/plan/pack.md", agent=who)
        pair(0, f"I8 {short} 跑 kb.py query 放行", 'python3 .claude/teams/tools/kb.py query "匯出 日期"',
             'python .claude/teams/tools/kb.py query "匯出 日期"', agent=who)
        pair(2, f"I8 {short} 跑 kb.py build", "python3 .claude/teams/tools/kb.py build --dir docs",
             "python .claude/teams/tools/kb.py build --dir docs", agent=who)
        pair(0, f"I8 {short} 跑 kb.py lint 放行", "python3 .claude/teams/tools/kb.py lint", "python .claude/teams/tools/kb.py lint", agent=who)
        pair(0, f"I8 {short} 跑 pack.py --selftest 放行", "python3 .claude/teams/tools/pack.py --selftest",
             "python .claude/teams/tools/pack.py --selftest", agent=who)
    tool(0, "I8 tpm-researcher 寫自己的紀錄 <run>/plan/research-a.md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/plan/research-a.md")}, RSR)
    tool(0, "I8 dev-implementer 寫 repo 的 pack.md 放行（不在 runs_dir）", "Write", {"file_path": str(H1R / "docs/pack.md")}, IMP)
    tool(0, "I8 主 session Write pack.md 放行", "Write", {"file_path": str(H1W / "_tpm/runs/x/plan/pack.md")})
    pair(0, "I8 主 session 跑 kb.py build 放行", "python3 .claude/teams/tools/kb.py build", "python .claude/teams/tools/kb.py build")
    pair(0, "I8 主 session 跑 pack.py 放行", "python3 .claude/teams/tools/pack.py plan --run _tpm/runs/x",
         "python .claude/teams/tools/pack.py plan --run _tpm/runs/x")

    # ------------------------------------------------------------ 交件信封
    E = "envelope"
    ok_env = '{"status":"done","summary":"s","artifacts":[],"evidence":[],"confidence":0.9,"open_questions":[],"derived_work":[],"payload":{}}'
    blk_env = '{"status":"done","summary":"s","artifacts":[],"evidence":[],"confidence":0.9,"open_questions":[{"question":"q","blocking":true,"options":[{"choice":"A","cost":"x"},{"choice":"B","cost":"y"}],"recommendation":"A","default_applied":"none"}],"derived_work":[],"payload":{}}'
    tnum_env = '{"status":"done","summary":"s","artifacts":[],"evidence":[],"confidence":0.9,"open_questions":[],"derived_work":["TASK-0399 補"],"payload":{}}'

    def hb(agent: str, msg: str) -> dict:
        return {"hook_event_name": "PreToolUse", "tool_name": "SubagentHandback", "agent_type": agent, "tool_input": {"message": msg}}

    def so(env: str) -> dict:
        return {"hook_event_name": "PreToolUse", "tool_name": "StructuredOutput", "agent_type": "agent-workflow:dev-implementer", "tool_input": json.loads(env)}

    def st(agent: str, msg: str, active: bool = False) -> dict:
        d = {"hook_event_name": "SubagentStop", "agent_type": agent, "stop_hook_active": active}
        if msg:
            d["last_assistant_message"] = msg
        return d

    def tc(desc: str) -> dict:
        return {"hook_event_name": "TaskCompleted", "cwd": str(WS), "task_id": "1", "task_name": "option-1", "task_description": desc}

    fence = "```"
    run(E, 0, "Handback 合格", hb("agent-workflow:tpm-writer", f"完成。\n{fence}json\n{ok_env}\n{fence}"))
    run(E, 2, "Handback 沒附信封", hb("agent-workflow:tpm-writer", "做完了"))
    run(E, 2, "Handback 命名空間角色沒附信封", hb("agent-workflow:tpm-writer", "做完了"))
    run(E, 2, "Handback done 但有 blocking", hb("agent-workflow:dev-implementer", f"{fence}json\n{blk_env}\n{fence}"))
    run(E, 0, "Handback 非 team 角色", hb("Explore", "找到了"))
    run(E, 2, "StructuredOutput 自編任務編號", so(tnum_env))
    run(E, 0, "StructuredOutput 合格", so(ok_env))
    run(E, 0, "StructuredOutput 非信封", so('{"findings":[]}'))
    run(E, 0, "SubagentStop 已擋過一次", st("agent-workflow:tpm-writer", "做完了", True))
    run(E, 0, "SubagentStop 沒有訊息", st("agent-workflow:tpm-writer", ""))
    run(E, 2, "SubagentStop 沒附信封", st("agent-workflow:tpm-writer", "做完了"))
    run(E, 0, "未 init 的專案不檢查", st("agent-workflow:tpm-writer", "做完了"), BARE)
    (WS / "_tpm/runs/x").mkdir(parents=True)
    (WS / "_tpm/runs/x/ok.json").write_text(ok_env, encoding="utf-8")
    (WS / "_tpm/runs/x/bad.json").write_text(blk_env, encoding="utf-8")
    run(E, 0, "TaskCompleted 非 team 任務", tc("一般任務"))
    run(E, 0, "TaskCompleted 產出合格", tc("output: _tpm/runs/x/ok.json"))
    run(E, 2, "TaskCompleted 產出不合格", tc("output: _tpm/runs/x/bad.json"))
    run(E, 2, "TaskCompleted 找不到產出", tc("output: _tpm/runs/x/none.json"))
    run(E, 2, "TaskCompleted 沒寫 output", tc("_tpm/runs/x 方案一"))

    # ------------------------------------------------------------ A2 讀取預算（read_budget.py；設定在 methodology.json 的 limits）
    RES = "agent-workflow:tpm-researcher"
    NO_FILE = object()
    rb_off_count = 0

    def rbws(name: str, profile, ext: dict | None = None) -> Path:
        """profile：dict＝methodology.json 的內容；str＝原樣寫入（測讀不懂）；NO_FILE＝不放 methodology.json。"""
        w = tmp / name
        (w / ".claude" / "teams").mkdir(parents=True)
        (w / ".claude" / "teams" / "guard.config.json").write_text(
            json.dumps({"runs_dir": "_tpm/runs", **({"extension_agents": ext} if ext else {})}), encoding="utf-8")
        if profile is not NO_FILE:
            (w / ".claude" / "teams" / "methodology.json").write_text(
                profile if isinstance(profile, str) else json.dumps(profile, ensure_ascii=False), encoding="utf-8")
        (w / "big.md").write_text("".join(f"第 {i} 行\n" for i in range(1000)), encoding="utf-8")
        (w / "small.md").write_text("".join(f"第 {i} 行\n" for i in range(50)), encoding="utf-8")
        (w / "chunk.txt").write_bytes((b"x" * 99 + b"\n") * 300)  # 300 行、30000 位元組：不超過門檻，整檔讀一次算 30000
        (w / ".env").write_text("A=1\n" * 1000, encoding="utf-8")
        return w

    def rd(w: Path, name: str, agent: str = RES, aid: str = "a1", sid: str = "", **kw) -> dict:
        d = {"hook_event_name": "PreToolUse", "tool_name": "Read", "cwd": str(w), "session_id": sid or w.name,
             "tool_input": {"file_path": str(w / name), **kw}}
        if agent:
            d["agent_type"] = agent
            d["agent_id"] = aid
        return d

    def rb(want: int, name: str, w: Path, data: dict, mode: str = "guard-paths", expect: str = "") -> None:
        r = hook(mode, data, w)
        err = r.stderr.decode("utf-8", "replace")
        check(f"讀取預算：{name}", want, r.returncode, err)
        if expect:
            check(f"讀取預算：{name}（訊息含「{expect}」）", True, expect in err, err)

    def rb_sh(w: Path, command: str, agent: str = RES, tool: str = "Bash") -> dict:
        d = sh_in(w, command, agent, tool)
        d["session_id"] = w.name
        if agent:
            d["agent_id"] = "a1"
        return d

    def rb_limits(mode: str = "enforce", lines=400, default=0, roles: dict | None = None) -> dict:
        return {"limits": {"mode": mode, "whole_file_lines": lines, "read_budget": {"default": default, "roles": roles or {}}}}

    # 第 1 條：大檔要分段讀（額度不限，只看行數）
    R1 = rbws("rb1", rb_limits(), ext={"acme-db-reviewer": "tpm-reviewer"})
    rb(2, "大檔整檔讀擋", R1, rd(R1, "big.md"), expect="共 1000 行")
    rb(2, "大檔整檔讀的訊息提示 kb.py", R1, rd(R1, "big.md"), expect="kb.py query")
    rb(0, "大檔帶 offset＋合理 limit 放", R1, rd(R1, "big.md", offset=1, limit=200))
    rb(2, "limit 超過門檻擋", R1, rd(R1, "big.md", offset=1, limit=1000), expect="一次最多 400 行")
    rb(2, "只帶 offset、剩下的行數仍超過門檻擋", R1, rd(R1, "big.md", offset=100))
    rb(0, "只帶 offset、剩下的行數不超過門檻放", R1, rd(R1, "big.md", offset=700))
    rb(0, "小檔整檔讀放", R1, rd(R1, "small.md"))
    rb(0, "小檔帶超大 limit 放（實際只有 50 行）", R1, rd(R1, "small.md", limit=2000))
    rb(2, "舊名 dev-pr-reviewer 照模版角色擋", R1, rd(R1, "big.md", "agent-workflow:dev-pr-reviewer"))
    rb(2, "沒有命名空間的同名角色照模版角色擋", R1, rd(R1, "big.md", "dev-implementer"))
    rb(0, "其他 plugin 的 agent 大檔整檔讀放", R1, rd(R1, "big.md", "my-plugin:foo"))
    rb(0, "登記過的擴充 agent 大檔整檔讀放", R1, rd(R1, "big.md", "acme-db-reviewer"))
    rb(0, "使用者自己的 agent dev-helper 大檔整檔讀放", R1, rd(R1, "big.md", "dev-helper"))
    rb(0, "主 session 大檔整檔讀放", R1, rd(R1, "big.md", ""))
    rb(2, "機敏檔 .env 照舊擋（模版角色）", R1, rd(R1, ".env"), expect="機敏檔")
    rb(2, "機敏檔 .env 照舊擋（主 session）", R1, rd(R1, ".env", ""), expect="機敏檔")
    (R1 / "blob.dat").write_bytes(b"\0\1\2\n" * 1000)
    (R1 / "shot.png").write_text("x\n" * 1000, encoding="utf-8")
    (R1 / "nb.ipynb").write_text("{}\n" * 1000, encoding="utf-8")
    rb(0, "二進位檔不套用", R1, rd(R1, "blob.dat"))
    rb(0, "圖片不套用", R1, rd(R1, "shot.png"))
    rb(0, "notebook 不套用", R1, rd(R1, "nb.ipynb"))
    rb(0, "檔案不存在放行", R1, rd(R1, "nope.md"))
    rb(0, "目錄放行", R1, rd(R1, ".claude"))
    rb(0, "offset／limit 型別怪異不崩潰（小檔）", R1, rd(R1, "small.md", offset="abc", limit=[1]))
    # 第 3 條：shell 的 cat 類讀取（Bash 與 PowerShell）
    BIG, SMALL = (R1 / "big.md").as_posix(), (R1 / "small.md").as_posix()
    rb(2, "cat 大檔擋", R1, rb_sh(R1, f"cat {BIG}"), S, expect="共 1000 行")
    rb(2, "cat -n 大檔擋（相對路徑）", R1, rb_sh(R1, "cat -n big.md"), S)
    rb(0, "cat 大檔 | head -50 放", R1, rb_sh(R1, f"cat {BIG} | head -50"), S)
    rb(0, "cat 小檔放", R1, rb_sh(R1, f"cat {SMALL}"), S)
    rb(0, "head -50 大檔放", R1, rb_sh(R1, f"head -50 {BIG}"), S)
    rb(0, "cat 大檔帶導向不算（不是單一簡單指令，放）", R1, rb_sh(R1, f"cat {BIG} 2>&1"), S)
    rb(0, "主 session cat 大檔放", R1, rb_sh(R1, f"cat {BIG}", ""), S)
    (R1 / "LONG~1").mkdir(exist_ok=True)
    (R1 / "LONG~1" / "big.md").write_text("x\n" * 1000, encoding="utf-8")
    rb(2, "cat 路徑中間有 ~（8.3 短路徑）照樣擋", R1, rb_sh(R1, "cat LONG~1/big.md"), S, expect="共 1000 行")
    rb(0, "cat 字首 ~（家目錄展開，看不懂）放", R1, rb_sh(R1, "cat ~/big.md"), S)
    rb(2, "PS：Get-Content 路徑中間有 ~ 照樣擋", R1, rb_sh(R1, "Get-Content LONG~1/big.md", tool="PowerShell"), S, expect="共 1000 行")
    rb(0, "PS：Get-Content 字首 ~ 放", R1, rb_sh(R1, "Get-Content ~/big.md", tool="PowerShell"), S)
    rb(0, "擴充 agent cat 大檔放", R1, rb_sh(R1, f"cat {BIG}", "acme-db-reviewer"), S)
    rb(2, "PS：Get-Content 大檔擋", R1, rb_sh(R1, f'Get-Content "{R1 / "big.md"}"', tool="PowerShell"), S, expect="共 1000 行")
    rb(2, "PS：gc -Path 大檔 -Raw 擋", R1, rb_sh(R1, "gc -Path big.md -Raw", tool="PowerShell"), S)
    rb(0, "PS：Get-Content 大檔 -TotalCount 50 放", R1, rb_sh(R1, f'Get-Content "{R1 / "big.md"}" -TotalCount 50', tool="PowerShell"), S)
    rb(0, "PS：Get-Content 大檔 -Tail 20 放", R1, rb_sh(R1, "Get-Content big.md -Tail 20", tool="PowerShell"), S)
    rb(0, "PS：Get-Content 大檔 | Select-Object -First 5 放", R1, rb_sh(R1, "Get-Content big.md | Select-Object -First 5", tool="PowerShell"), S)
    rb(0, "PS：Get-Content 小檔放", R1, rb_sh(R1, "Get-Content small.md", tool="PowerShell"), S)
    rb(0, "PS：主 session Get-Content 大檔放", R1, rb_sh(R1, "Get-Content big.md", "", tool="PowerShell"), S)

    # 第 2 條：讀取額度（default 100000；tpm-planner 專屬 20000；chunk.txt 一次 30000）
    R2 = rbws("rb2", rb_limits(default=100000, roles={"tpm-planner": 20000}))
    rb(0, "額度：第 1 次讀（30%）放", R2, rd(R2, "chunk.txt"))
    rb(0, "額度：第 2 次讀（60%）放", R2, rd(R2, "chunk.txt"))
    rb(2, "額度：跨過 80% 擋一次", R2, rd(R2, "chunk.txt"), expect="已用 80%")
    rb(0, "額度：80% 提醒後重試放行", R2, rd(R2, "chunk.txt"))
    rb(0, "額度：還沒用完（90%）放，不再提醒", R2, rd(R2, "chunk.txt"))
    rb(2, "額度：累計超過擋", R2, rd(R2, "chunk.txt"), expect="open_questions")
    rb(2, "額度：用完後小檔也擋", R2, rd(R2, "small.md"), expect="執行紀錄檔")
    rb(0, "額度：另一個 agent_id 各自計算", R2, rd(R2, "chunk.txt", aid="a2"))
    rb(0, "額度：另一個 session 各自計算", R2, rd(R2, "chunk.txt", sid="rb2-other"))
    rb(0, "額度：主 session 不計", R2, rd(R2, "chunk.txt", ""))
    state_a1 = RB_STATE / "rb2" / "a1.json"
    try:
        st_a1 = json.loads(state_a1.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        st_a1 = {}
    check("讀取預算：狀態檔在 <狀態目錄>/<session_id>/<agent_id>.json，記了用量與提醒", True,
          st_a1.get("used") == 120000 and st_a1.get("warned") is True, str(st_a1))
    check("讀取預算：沒留下暫存檔", [], [x.name for x in (RB_STATE / "rb2").glob("*.tmp")])
    PLN = "agent-workflow:tpm-planner"
    rb(2, "角色專屬額度：tpm-planner 第 1 次就跨過 80%", R2, rd(R2, "chunk.txt", PLN, "p1"), expect="已用 80%")
    rb(0, "角色專屬額度：重試放行", R2, rd(R2, "chunk.txt", PLN, "p1"))
    rb(2, "角色專屬額度：tpm-planner 用完擋", R2, rd(R2, "chunk.txt", PLN, "p1"), expect="額度已用完")
    state_a1.write_text("垃圾｛不是 JSON", encoding="utf-8")
    rb(0, "狀態檔寫入垃圾內容後放行（重置）", R2, rd(R2, "chunk.txt"))
    state_a1.write_text('{"used": "很多", "warned": 1}', encoding="utf-8")
    rb(0, "狀態檔欄位型別不對後放行（重置）", R2, rd(R2, "chunk.txt"))
    rb(0, "分段讀只算讀到的行數", R2, rd(R2, "chunk.txt", aid="a3", offset=1, limit=100))
    try:
        used_a3 = json.loads((RB_STATE / "rb2" / "a3.json").read_text(encoding="utf-8")).get("used", 0)
    except (OSError, ValueError):
        used_a3 = -1
    check("讀取預算：分段讀 100 行約算 10000 位元組", True, 9900 <= used_a3 <= 10100, str(used_a3))
    rb(0, "session_id／agent_id 帶路徑字元不會跳出狀態目錄", R2, rd(R2, "chunk.txt", aid="../../evil", sid="../../../evil-session"))
    check("讀取預算：狀態檔只寫在狀態目錄裡", [], [str(x) for x in tmp.rglob("*evil*") if RB_STATE not in x.parents])
    check("讀取預算：帶路徑字元的 id 仍有記帳", True, any(RB_STATE.rglob("*evil*.json")))

    # 沒有 agent_id：分不出是哪一個 agent，不記帳（不然同型別的 agent 共用額度，後派的一開工就被擋）
    no_id = {k: v for k, v in rd(R2, "chunk.txt", PLN).items() if k != "agent_id"}
    for i in range(3):
        rb(0, f"沒有 agent_id 不記帳、不擋（第 {i + 1} 次）", R2, no_id)
    check("讀取預算：沒有 agent_id 時沒有寫出 type- 的狀態檔", [], [x.name for x in (RB_STATE / "rb2").glob("type-*")])
    # B12.1：Grep 的 content 模式與 shell 的 grep 類也計入額度（估計值）；只列檔名、計數、-q、管線不計
    def gp(w: Path, agent: str = RES, aid: str = "g1", **kw) -> dict:
        d = {"hook_event_name": "PreToolUse", "tool_name": "Grep", "cwd": str(w), "session_id": w.name, "tool_input": {"pattern": "第 1", "path": str(w), **kw}}
        if agent:
            d["agent_type"] = agent
            d["agent_id"] = aid
        return d
    RG = rbws("rb-grep", rb_limits(default=30000))
    rb(0, "Grep 只列檔名（預設模式）不計", RG, gp(RG))
    rb(0, "Grep count 模式不計", RG, gp(RG, output_mode="count"))
    rb(2, "Grep content：估計量跨過 80% 先提醒", RG, gp(RG, output_mode="content"), expect="已用 80%")
    rb(0, "Grep content：重試放行並計入", RG, gp(RG, output_mode="content"))
    rb(2, "Grep content：額度用完後擋", RG, gp(RG, output_mode="content"), expect="額度已用完")
    rb(0, "Grep 只列檔名：額度用完後照樣放（讓 agent 能收尾）", RG, gp(RG, output_mode="files_with_matches"))
    rb(2, "Grep content 用掉的額度和 Read 共用", RG, rd(RG, "small.md", aid="g1"), expect="額度已用完")
    rb(0, "Grep content：主 session 不計", RG, gp(RG, "", output_mode="content"))
    rb(0, "Grep content：使用者自己的 agent 不計", RG, gp(RG, "dev-helper", output_mode="content"))
    rb(0, "Grep content：帶小的 head_limit 估得少（另一個 agent 不會一次跨過 80%）", RG, gp(RG, aid="g2", output_mode="content", head_limit=20))
    RGS = rbws("rb-grep-sh", rb_limits(default=20000))
    rb(2, "shell grep 印內容：跨過 80% 先提醒", RGS, rb_sh(RGS, "grep -n 第 big.md"), S, expect="已用 80%")
    rb(0, "shell grep 印內容：重試放行並計入", RGS, rb_sh(RGS, "grep -n 第 big.md"), S)
    rb(0, "shell rg 印內容：再計一次", RGS, rb_sh(RGS, "rg 第 big.md"), S)
    rb(2, "shell grep 印內容：額度用完後擋", RGS, rb_sh(RGS, "grep 第 big.md"), S, expect="額度已用完")
    rb(0, "shell grep -q 不計也不擋", RGS, rb_sh(RGS, "grep -q 第 big.md"), S)
    rb(0, "shell grep -l 不計也不擋", RGS, rb_sh(RGS, "grep -rl 第 ."), S)
    rb(0, "shell grep -c 不計也不擋", RGS, rb_sh(RGS, "grep -c 第 big.md"), S)
    rb(0, "shell grep 接管線（不是單一簡單指令）不計", RGS, rb_sh(RGS, "grep 第 big.md | head -5"), S)
    RGP = rbws("rb-grep-ps", rb_limits(default=20000))
    rb(2, "PS：Select-String 印內容：跨過 80% 先提醒", RGP, rb_sh(RGP, "Select-String -Pattern 第 big.md", tool="PowerShell"), S, expect="已用 80%")
    rb(0, "PS：Select-String 重試放行並計入", RGP, rb_sh(RGP, "Select-String -Pattern 第 big.md", tool="PowerShell"), S)
    rb(0, "PS：sls 再計一次", RGP, rb_sh(RGP, "sls 第 big.md", tool="PowerShell"), S)
    rb(2, "PS：Select-String 額度用完後擋", RGP, rb_sh(RGP, "Select-String -Pattern 第 big.md", tool="PowerShell"), S, expect="額度已用完")
    rb(0, "PS：Select-String -Quiet 不計也不擋", RGP, rb_sh(RGP, "Select-String -Pattern 第 big.md -Quiet", tool="PowerShell"), S)
    rb(0, "PS：Select-String 接管線不計", RGP, rb_sh(RGP, "Select-String -Pattern 第 big.md | Select-Object -First 3", tool="PowerShell"), S)
    RGW = rbws("rb-grep-off", {"limits": {"whole_file_lines": 400, "read_budget": {"default": 1, "roles": {}}}})
    rb(0, "沒啟用（沒寫 mode）：Grep content 不計不擋", RGW, gp(RGW, output_mode="content"))
    rb(0, "沒啟用（沒寫 mode）：shell grep 不計不擋", RGW, rb_sh(RGW, "grep 第 big.md"), S)

    # 單行的大檔：行數過得了門檻，但一次最多記 100KB，不會一次把額度扣光
    (R2 / "oneline.txt").write_text("x" * 900000, encoding="utf-8")
    R2B = rbws("rb2b", rb_limits(default=1000000))
    (R2B / "oneline.txt").write_text("x" * 900000, encoding="utf-8")
    rb(0, "單行大檔第 1 次放", R2B, rd(R2B, "oneline.txt", aid="o1"))
    try:
        used_o1 = json.loads((RB_STATE / "rb2b" / "o1.json").read_text(encoding="utf-8")).get("used", 0)
    except (OSError, ValueError):
        used_o1 = -1
    check("讀取預算：單次讀取最多記 100KB", True, 0 < used_o1 <= 100 * 1024, str(used_o1))

    # mode：warn 不擋但留紀錄；off、沒有 limits、沒有 methodology.json、讀不懂都放
    R3 = rbws("rb3", rb_limits("warn", default=40000))
    rb(0, "warn：大檔整檔讀不擋", R3, rd(R3, "big.md", aid="w0"))
    rb(0, "warn：跨過 80% 不擋", R3, rd(R3, "chunk.txt"))
    rb(0, "warn：再讀（還沒用完）不擋", R3, rd(R3, "chunk.txt"))
    rb(0, "warn：額度用完不擋", R3, rd(R3, "chunk.txt"))
    rb(0, "warn：cat 大檔不擋", R3, rb_sh(R3, "cat big.md"), S)
    rb(0, "warn：PS Get-Content 大檔不擋", R3, rb_sh(R3, "Get-Content big.md", tool="PowerShell"), S)
    try:
        wl = [json.loads(x) for x in (RB_STATE / "warn.log").read_text(encoding="utf-8").splitlines()]
    except (OSError, ValueError):
        wl = []
    check("warn：warn.log 一行一筆 JSON，記了本來會擋的規則", {"whole_file", "budget_80", "budget_exhausted", "shell_whole_file"},
          {x.get("rule") for x in wl if isinstance(x, dict) and x.get("session_id") == "rb3"}, str(wl)[:300])
    check("warn：enforce 的工作區不寫 warn.log", [], [x for x in wl if isinstance(x, dict) and x.get("session_id") in ("rb1", "rb2")])
    for label, prof in (("mode: off", rb_limits("off", default=1)), ("沒有 limits 鍵", {"stages": {}}),
                        ("沒寫 mode（內建 profile 的樣子：有門檻、沒啟用）", {"limits": {"whole_file_lines": 400, "read_budget": {"default": 1, "roles": {}}}}), ("沒有 methodology.json", NO_FILE),
                        ("methodology.json 讀不懂", "{不是 JSON"), ("methodology.json 不是物件", "[1, 2]"),
                        ("limits 不是物件", {"limits": "enforce"}), ("mode 不認得", rb_limits("strict", default=1)),
                        ("whole_file_lines 型別不對", rb_limits(lines="400")), ("whole_file_lines 是 0", rb_limits(lines=0)),
                        ("數值是布林", rb_limits(lines=True, default=True))):
        rb_off_count += 1
        w = rbws(f"rb-off-{rb_off_count}", prof)
        rb(0, f"{label}：大檔整檔讀放", w, rd(w, "big.md"))
        rb(0, f"{label}：再讀一次也放（不計額度）", w, rd(w, "big.md"))
        rb(0, f"{label}：cat 大檔放", w, rb_sh(w, "cat big.md"), S)
        rb(0, f"{label}：PS Get-Content 大檔放", w, rb_sh(w, "Get-Content big.md", tool="PowerShell"), S)
        rb(2, f"{label}：機敏檔照舊擋", w, rd(w, ".env"))
    R4 = rbws("rb4", rb_limits(default=0, roles={"tpm-researcher": 20000, "tpm-skeptic": 0}))
    rb(2, "roles 有設、default 是 0：有設的角色照額度", R4, rd(R4, "chunk.txt"), expect="已用 80%")
    for i in range(3):
        rb(0, f"roles 的值 0＝該角色不限（第 {i + 1} 次）", R4, rd(R4, "chunk.txt", "agent-workflow:tpm-skeptic", "k1"))
        rb(0, f"default 0＝其他角色不限（第 {i + 1} 次）", R4, rd(R4, "chunk.txt", "agent-workflow:dev-implementer", "i1"))

    # read_budget.py 壞掉（載入失敗）：放行，既有檢查照舊
    broken = tmp / "hooks-broken"
    shutil.copytree(HOOKS, broken, ignore=shutil.ignore_patterns("__pycache__", "tests"))
    (broken / "read_budget.py").write_text("這不是 Python (\n", encoding="utf-8")

    def broken_hook(mode: str, data: dict) -> int:
        env = {**os.environ, "CLAUDE_PROJECT_DIR": str(R1), "PYTHONUTF8": "1"}
        return subprocess.run([PY, str(broken / "awf_hook.py"), mode], input=json.dumps(data).encode("utf-8"), capture_output=True, env=env).returncode

    check("讀取預算：模組載入失敗時 Read 大檔放行", 0, broken_hook("guard-paths", rd(R1, "big.md")))
    check("讀取預算：模組載入失敗時 cat 大檔放行", 0, broken_hook(S, rb_sh(R1, "cat big.md")))
    check("讀取預算：模組載入失敗時機敏檔照舊擋", 2, broken_hook("guard-paths", rd(R1, ".env")))
    check("讀取預算：模組載入失敗時 push 照舊擋", 2, broken_hook(S, rb_sh(R1, "git push origin feature/x")))

    # 行程內：舊 session 目錄清理、效能（讀設定＋數行＋讀寫狀態檔，每次不超過 50ms）
    import time as _time
    sys.path.insert(0, str(HOOKS))
    saved_env = os.environ.get("CLAUDE_PROJECT_DIR")
    try:
        import awf_config as _cfg
        import read_budget as _rb
        old_root = tmp / "rb-old"
        for d in ("stale", "fresh", "keep"):
            (old_root / d).mkdir(parents=True)
            (old_root / d / "a.json").write_text("{}", encoding="utf-8")
        (old_root / "keep" / "別人的檔案.txt").write_text("x", encoding="utf-8")
        long_ago = _time.time() - 8 * 86400
        for d in ("stale", "keep"):
            os.utime(old_root / d, (long_ago, long_ago))
        _rb._cleanup(old_root)
        check("讀取預算：超過 7 天的 session 目錄清掉", False, (old_root / "stale").exists())
        check("讀取預算：新的 session 目錄留著", True, (old_root / "fresh" / "a.json").is_file())
        check("讀取預算：清理不刪不是自己寫的檔案", True, (old_root / "keep" / "別人的檔案.txt").is_file())

        PERF = rbws("rb-perf", rb_limits(default=10 ** 9))
        (PERF / "f400.py").write_text("".join(f"x = {i}  # 一行大約四十個位元組的程式碼\n" for i in range(400)), encoding="utf-8")
        os.environ["CLAUDE_PROJECT_DIR"] = str(PERF)
        d_read = rd(PERF, "f400.py")
        took, res = [], []
        for _ in range(30):
            t0 = _time.perf_counter()
            res.append(_rb.check_read(d_read, _cfg.load()))
            took.append(_time.perf_counter() - t0)
        took.sort()
        median_ms = took[len(took) // 2] * 1000
        try:
            n_reads = json.loads((RB_STATE / "rb-perf" / "a1.json").read_text(encoding="utf-8")).get("reads")
        except (OSError, ValueError):
            n_reads = None
        check("讀取預算：效能量測走的是計數路徑（30 次都放行並記帳）", True, res == [None] * 30 and n_reads == 30, f"{res[:3]} reads={n_reads}")
        check(f"讀取預算：計數路徑每次 ≤50ms（中位數 {median_ms:.2f}ms）", True, median_ms <= 50)
        d_cat = rb_sh(PERF, "cat f400.py")
        t0 = _time.perf_counter()
        for _ in range(30):
            _rb.check_shell(d_cat, _cfg.load())
        shell_ms = (_time.perf_counter() - t0) / 30 * 1000
        check(f"讀取預算：shell 檢查每次 ≤50ms（平均 {shell_ms:.2f}ms）", True, shell_ms <= 50)
        print(f"讀取預算效能：Read 計數路徑中位數 {median_ms:.2f}ms、shell 檢查平均 {shell_ms:.2f}ms")
    finally:
        sys.path.remove(str(HOOKS))
        if saved_env is None:
            os.environ.pop("CLAUDE_PROJECT_DIR", None)
        else:
            os.environ["CLAUDE_PROJECT_DIR"] = saved_env

    # ------------------------------------------------------------ 入口本身：出錯時不能回 0，也不能冒充 2
    # 守門類輸入壞掉一律不放行（exit 2，或 hook 失敗；H2 之後統一為 2），也不能印 traceback
    for mode in ("guard-shell", "guard-paths"):
        r = hook(mode, "不是 JSON", WS)
        check(f"{mode}：輸入壞掉不放行", True, r.returncode != 0)
    r = subprocess.run([PY, str(ENTRY), "no-such-mode"], input=b"{}", capture_output=True)
    check("未知模式＝hook 失敗（非 0、非 2）", True, r.returncode not in (0, 2))

    # ------------------------------------------------------------ 註冊設定：不用 bash、涵蓋 PowerShell、守門 fail-closed
    def check_registration(label: str, cfg: dict) -> None:
        text = json.dumps(cfg, ensure_ascii=False)
        check(f"{label}：不呼叫 bash／.sh", True, "bash " not in text and ".sh" not in text)
        pre = cfg["hooks"]["PreToolUse"]
        by = {e["matcher"]: e["hooks"][0] for e in pre}
        check(f"{label}：shell 守門 matcher 是 Bash|PowerShell", True, "Bash|PowerShell" in by)
        # 守門類 onFailure 依 plans 的 hook-runtime 決定：plugin 的 hooks.json 是 continue（hook 跑不起來時不鎖死所有工具），
        # 專案 settings（--hooks project，Python 絕對路徑）是 block
        want_fail = ("continue",) if label == "hooks.json" else ("block",)
        for tools, mode in ((("Bash", "PowerShell"), "guard-shell"), (("Edit", "Write", "NotebookEdit"), "guard-paths")):
            ent = next((e for e in pre if f"awf_hook.py\" {mode}" in e["hooks"][0].get("command", "")), None)
            check(f"{label}：{mode} 走 awf_hook.py", True, ent is not None)
            ms = set((ent or {}).get("matcher", "").split("|"))
            check(f"{label}：{mode} 涵蓋 {'／'.join(tools)}", True, set(tools) <= ms)
            check(f"{label}：{mode} 的 onFailure 是 {'／'.join(want_fail)}", True, ((ent or {}).get("hooks") or [{}])[0].get("onFailure") in want_fail)
            if mode == "guard-paths" and ms & {"Read", "Grep"}:
                check(f"{label}：Read／Grep 走 guard-paths", True, {"Read", "Grep"} <= ms)
        h = by.get("SubagentHandback|StructuredOutput", {})
        check(f"{label}：交件檢查出錯不擋（onFailure: continue）", "continue", h.get("onFailure"))
        for ev in ("SubagentStop", "TaskCompleted"):
            check(f"{label}：{ev} 走 awf_hook.py envelope", True, "awf_hook.py\" envelope" in cfg["hooks"][ev][0]["hooks"][0]["command"])

    hooks_json = HOOKS / "hooks.json"
    if hooks_json.is_file():
        check_registration("hooks.json", json.loads(hooks_json.read_text(encoding="utf-8")))
    proj_settings = Path(args.project) / ".claude" / "settings.json" if args.project else None
    if proj_settings and proj_settings.is_file():
        cfg = json.loads(proj_settings.read_text(encoding="utf-8"))
        if "hooks" in cfg:
            check_registration("專案 settings.json", cfg)

    # ------------------------------------------------------------ 轉呼叫 bash 寫的 repo guard（只在有 bash 的 POSIX 系統測）
    bash = shutil.which("bash")
    if bash and os.name != "nt":
        repo2 = WS / "repos" / "legacy"
        (repo2 / ".claude" / "hooks").mkdir(parents=True)
        g = repo2 / ".claude" / "hooks" / ("guard-git" + ".sh")
        g.write_text('#!/usr/bin/env bash\nin=$(cat)\ncase "$in" in *"git push origin main"*) echo "repo guard：不推 main" >&2; exit 2 ;; esac\nexit 0\n', encoding="utf-8")
        git("init", "-q", "-b", "main", cwd=repo2)
        run(S, 2, "轉呼叫 bash 版 repo guard：擋", sh_in(repo2, "git push origin main"))
        run(S, 0, "轉呼叫 bash 版 repo guard：放", sh_in(repo2, "git push origin feature/x"))
    else:
        print("略過：沒有 bash（或是 Windows），不測 bash 版 repo guard 的轉呼叫")
finally:
    shutil.rmtree(tmp, ignore_errors=True)

print(f"通過 {passed}，失敗 {failed}")
sys.exit(0 if failed == 0 else 1)
