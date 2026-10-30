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
    run(P, 0, "擴充 agent（套 tpm-reviewer）可以寫 run 目錄", edit(XWS / "_tpm/runs/r/ext-db.md", "acme-db-reviewer", XWS), XWS)
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

    # ------------------------------------------------------------ 入口本身：出錯時不能回 0，也不能冒充 2
    # （守門類 hook 註冊時設 onFailure: block，所以「非 0 非 2」會被 Claude Code 當成失敗而擋下）
    for mode in ("guard-shell", "guard-paths"):
        r = hook(mode, "不是 JSON", WS)
        check(f"{mode}：輸入壞掉＝hook 失敗（非 0、非 2）", True, r.returncode not in (0, 2))
    r = subprocess.run([PY, str(ENTRY), "no-such-mode"], input=b"{}", capture_output=True)
    check("未知模式＝hook 失敗（非 0、非 2）", True, r.returncode not in (0, 2))

    # ------------------------------------------------------------ 註冊設定：不用 bash、涵蓋 PowerShell、守門 fail-closed
    def check_registration(label: str, cfg: dict) -> None:
        text = json.dumps(cfg, ensure_ascii=False)
        check(f"{label}：不呼叫 bash／.sh", True, "bash " not in text and ".sh" not in text)
        pre = cfg["hooks"]["PreToolUse"]
        by = {e["matcher"]: e["hooks"][0] for e in pre}
        check(f"{label}：shell 守門 matcher 是 Bash|PowerShell", True, "Bash|PowerShell" in by)
        for matcher, mode in (("Bash|PowerShell", "guard-shell"), ("Edit|Write|NotebookEdit", "guard-paths")):
            h = by.get(matcher, {})
            check(f"{label}：{mode} 走 awf_hook.py", True, f"awf_hook.py\" {mode}" in h.get("command", ""))
            check(f"{label}：{mode} 跑不起來時擋下（onFailure: block）", "block", h.get("onFailure"))
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
