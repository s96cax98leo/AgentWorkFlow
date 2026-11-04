#!/usr/bin/env python3
"""awf_hook.py 入口層的高可用測試（H2）：失敗語意、啟動方式、timeout 與延遲、沒 init 的專案、壞輸入。

用法：python3 plugin/hooks/tests/test_entry.py [--hooks-dir <hooks 目錄>]
  1. guard 模組壞掉（語法錯誤、缺 awf_config.py）：改用內建最小規則，一般指令與寫檔放行，push／merge／approve 擋。
  2. hooks.json：守門 hook 用 `command -v python3` 判斷、不用 `||`、onFailure=continue、每個 hook 都有 timeout；
     POSIX 上實際用 /bin/sh 跑 hooks.json 的指令，模擬 PATH 只有 python、python3 是 Microsoft Store 空殼、兩者都沒有。
  3. 量 guard-shell 典型指令的延遲並印出 p95。門檻預設 300ms；CI（環境變數 CI 有值）放寬到 1000ms，
     因為共用 runner 的行程啟動常態就慢上數倍；也可用 AWF_HOOK_P95_MS 自訂。
  4. 沒有 guard.config.json 的專案：主 session 一般指令、commit、寫檔放行；tpm-／dev- agent 推送仍擋。
  5. 壞輸入（空、非 JSON、殘缺 JSON、非物件、缺欄位、非 UTF-8、2MB）：守門 exit 2、交件 exit 0，都不能有 traceback。
只用標準庫，macOS、Linux、Windows 都能跑（/bin/sh 的部分只在 POSIX 跑）。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
PY = sys.executable

ap = argparse.ArgumentParser()
ap.add_argument("--hooks-dir", default=str(HERE.parent))
args = ap.parse_args()
HOOKS = Path(args.hooks_dir).resolve()
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass

passed = failed = 0
TMP = Path(tempfile.mkdtemp(prefix="awf-entry-"))
AGENT = "agent-workflow:dev-implementer"


def check(name: str, ok: bool, detail: str = "") -> None:
    global passed, failed
    if ok:
        passed += 1
    else:
        failed += 1
        print(f"FAIL［{name}］" + (f"\n    {detail.strip()[:400]}" if detail else ""))


def env_for(ws: Path) -> dict:
    e = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE_")}
    e.update({"CLAUDE_PROJECT_DIR": str(ws), "PYTHONUTF8": "1"})
    return e


def hook(mode: str, ws: Path, data, hooks_dir: Path = HOOKS) -> subprocess.CompletedProcess:
    raw = data if isinstance(data, bytes) else (data if isinstance(data, str) else json.dumps(data, ensure_ascii=False)).encode("utf-8")
    return subprocess.run([PY, str(hooks_dir / "awf_hook.py"), mode], input=raw, capture_output=True, env=env_for(ws), timeout=60)


def sh(cwd: Path, command: str, agent: str = "") -> dict:
    d = {"hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": str(cwd), "tool_input": {"command": command}}
    if agent:
        d["agent_type"] = agent
    return d


def write(cwd: Path, path: Path, agent: str = "") -> dict:
    d = {"hook_event_name": "PreToolUse", "tool_name": "Write", "cwd": str(cwd), "tool_input": {"file_path": str(path), "content": "x"}}
    if agent:
        d["agent_type"] = agent
    return d


def stderr(r: subprocess.CompletedProcess) -> str:
    return r.stderr.decode("utf-8", errors="replace")


def expect(name: str, r: subprocess.CompletedProcess, want: int) -> None:
    err = stderr(r)
    check(name, r.returncode == want and "Traceback" not in err, f"want={want} got={r.returncode} {err}")


def git(*a: str, cwd: Path) -> None:
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *a], cwd=cwd, check=True, capture_output=True)


def init_ws() -> tuple[Path, Path]:
    """已 init 的工作區：guard.config.json 加一個有 origin／upstream 的假 repo。"""
    ws = TMP / "ws"
    (ws / ".claude" / "teams").mkdir(parents=True)
    (ws / ".claude/teams/guard.config.json").write_text(json.dumps({
        "runs_dir": "_tpm/runs", "repos_dir": "repos", "upstream_repos": "acme-up/app", "fork_repo": "acme-fork/app",
        "allowed_branch_prefixes": "feature/,bugfix/", "writer_dirs": ["_tpm/runs", "docs"],
    }), encoding="utf-8")
    repo = ws / "repos" / "app"
    repo.mkdir(parents=True)
    git("init", "-q", "-b", "main", cwd=repo)
    git("commit", "-q", "--allow-empty", "-m", "i", cwd=repo)
    git("remote", "add", "upstream", "https://github.com/acme-up/app.git", cwd=repo)
    git("remote", "add", "origin", "https://github.com/acme-fork/app.git", cwd=repo)
    return ws, repo


def broken_copy(name: str, breaker) -> Path:
    d = TMP / name / "hooks"
    shutil.copytree(HOOKS, d, ignore=shutil.ignore_patterns("__pycache__", "tests"))
    breaker(d)
    return d


# ───────── 1. guard 模組壞掉：內建最小規則 ─────────


def t_fallback(ws: Path, repo: Path) -> None:
    variants = {
        "guard 語法錯誤": lambda d: (d / "guard.py").write_text((d / "guard.py").read_text(encoding="utf-8") + "\n    \"\n", encoding="utf-8"),
        "缺 awf_config.py": lambda d: (d / "awf_config.py").unlink(),
        "guard 載入時丟例外": lambda d: (d / "guard.py").write_text("raise RuntimeError('boom')\n", encoding="utf-8"),
    }
    for label, breaker in variants.items():
        d = broken_copy(label.replace(" ", "_"), breaker)
        allow = ["ls -la", "git status", "git commit -m x", "git log --oneline", "gh pr view 3", "gh api repos/acme-fork/app/pulls",
                 "curl https://example.com/merge-sort.html"]
        block = [
            ("git push origin HEAD:feature/x", AGENT), ("git push origin HEAD:feature/x", ""),
            ("cd repos/app && git -C . push --force", AGENT), ("git send-pack origin main", AGENT),
            ("gh pr merge 3 --squash", ""), ("gh pr review 3 --approve", ""), ("gh pr review 3 -a", AGENT),
            ("gh api -X PUT repos/acme-fork/app/pulls/3/merge", AGENT), ("gh api graphql -f query='mutation{mergePullRequest}'", AGENT),
            ("curl -X PUT https://api.github.com/repos/acme-fork/app/pulls/3/merge", AGENT),
            ("curl -d '{\"event\":\"APPROVE\"}' https://api.github.com/repos/a/b/pulls/3/reviews", ""),
        ]
        for c in allow:
            r = hook("guard-shell", ws, sh(repo, c), d)
            expect(f"{label}：放行 {c}", r, 0)
        for c, agent in block:
            r = hook("guard-shell", ws, sh(repo, c, agent), d)
            expect(f"{label}：擋 {c}（{agent or '主 session'}）", r, 2)
        r = hook("guard-shell", ws, sh(repo, "git push origin HEAD:feature/x", AGENT), d)
        check(f"{label}：stderr 有最小規則提示", "最小規則" in stderr(r), stderr(r))
        expect(f"{label}：guard-paths 一般寫檔放行", hook("guard-paths", ws, write(repo, repo / "a.txt", AGENT), d), 0)
        expect(f"{label}：guard-paths 主 session 寫檔放行", hook("guard-paths", ws, write(repo, repo / "a.txt"), d), 0)
        expect(f"{label}：壞輸入仍擋", hook("guard-shell", ws, "not json", d), 2)

    d = broken_copy("envelope_broken", lambda d: (d / "envelope_hook.py").write_text("def run(:\n", encoding="utf-8"))
    r = hook("envelope", ws, {"hook_event_name": "SubagentStop", "agent_type": AGENT, "last_assistant_message": "x"}, d)
    expect("envelope 模組壞掉：放行", r, 0)


# ───────── 2. hooks.json 的啟動方式 ─────────


def t_launcher(ws: Path, repo: Path) -> None:
    hj = json.loads((HOOKS / "hooks.json").read_text(encoding="utf-8"))
    every = [(ev, h) for ev, ms in hj["hooks"].items() for m in ms for h in m["hooks"]]
    guards = [h for ev, h in every if ev == "PreToolUse" and "guard-" in h["command"]]
    check("hooks.json 有 guard-shell 與 guard-paths", all(any(f" {m};" in h["command"] for h in guards) for m in ("guard-shell", "guard-paths")), str(guards))
    for ev, h in every:
        t = h.get("timeout")
        check(f"{ev} 有 timeout（1–30 秒）", isinstance(t, int) and 1 <= t <= 30, str(h))
    for h in guards:
        c = h["command"]
        check("守門 hook 用 command -v python3 判斷", "command -v python3" in c and "else python " in c, c)
        check("守門 hook 不用 ||", "||" not in c, c)
        check("守門 hook onFailure=continue", h.get("onFailure") == "continue", str(h))
        check("守門 hook timeout ≤ 10 秒", h.get("timeout", 99) <= 10, str(h))
    paths = [m["matcher"] for m in hj["hooks"]["PreToolUse"] if any("guard-paths" in h["command"] for h in m["hooks"])]
    check("guard-paths 涵蓋 Read、Grep", all(t in "|".join(paths).split("|") for t in ("Edit", "Write", "NotebookEdit", "Read", "Grep")), str(paths))

    if os.name == "nt" or not Path("/bin/sh").exists():
        print("略過 /bin/sh 啟動測試（非 POSIX）")
        return
    cmd = guards[0]["command"].replace("${CLAUDE_PLUGIN_ROOT}", str(HOOKS.parent))
    real = Path(PY).resolve()

    def run_with(binname: str, setup) -> subprocess.CompletedProcess:
        b = TMP / binname
        b.mkdir()
        setup(b)
        for tool in ("git",):
            w = shutil.which(tool)
            if w:
                (b / tool).symlink_to(w)
        data = json.dumps(sh(repo, "git push origin HEAD:feature/x", AGENT)).encode("utf-8")
        e = {"PATH": str(b), "CLAUDE_PROJECT_DIR": str(ws), "PYTHONUTF8": "1"}
        return subprocess.run(["/bin/sh", "-c", cmd], input=data, capture_output=True, env=e, timeout=60)

    r = run_with("only_python", lambda b: (b / "python").symlink_to(real))
    check("PATH 只有 python：agent push 仍擋（exit 2）", r.returncode == 2, f"got {r.returncode} {stderr(r)}")
    r = run_with("both", lambda b: ((b / "python3").symlink_to(real), (b / "python").symlink_to(real)))
    check("PATH 有 python3：agent push 擋（exit 2）", r.returncode == 2, f"got {r.returncode} {stderr(r)}")

    def store_stub(b: Path) -> None:  # Microsoft Store 的 python3 空殼：存在但不跑腳本，回傳 9009
        p = b / "python3"
        p.write_text("#!/bin/sh\nexit 9009\n", encoding="utf-8")
        p.chmod(0o755)
    r = run_with("store_stub", store_stub)
    check("python3 是商店空殼：不是 exit 2（跑不起來＝onFailure continue 放行，不鎖死）", r.returncode not in (0, 2), f"got {r.returncode}")
    r = run_with("no_python", lambda b: None)
    check("沒有任何 Python：不是 exit 2（跑不起來＝放行，不鎖死）", r.returncode not in (0, 2), f"got {r.returncode}")


# ───────── 3. 延遲 ─────────


def t_latency(ws: Path, repo: Path) -> None:
    cmds = [("ls -la", ""), ("git status", ""), ("git commit -m x", AGENT), ("npm test", AGENT), ("git push origin HEAD:feature/x", AGENT)]
    samples = []
    for _ in range(6):
        for c, agent in cmds:
            t0 = time.perf_counter()
            hook("guard-shell", ws, sh(repo, c, agent))
            samples.append((time.perf_counter() - t0) * 1000)
    samples.sort()
    p95 = samples[max(0, int(round(len(samples) * 0.95)) - 1)]
    limit = float(os.environ.get("AWF_HOOK_P95_MS") or (1000 if os.environ.get("CI") else 300))
    print(f"guard-shell 延遲：n={len(samples)} p50={samples[len(samples) // 2]:.0f}ms p95={p95:.0f}ms（門檻 {limit:.0f}ms）")
    check(f"guard-shell p95 ≤ {limit:.0f}ms", p95 <= limit, f"p95={p95:.0f}ms")


# ───────── 4. 沒 init 的專案 ─────────


def t_no_init() -> None:
    proj = TMP / "noinit"
    proj.mkdir()
    git("init", "-q", "-b", "main", cwd=proj)
    for c in ("ls -la", "git commit -m x", "git checkout -b anything", "rm -rf build"):
        expect(f"沒 init：主 session {c}", hook("guard-shell", proj, sh(proj, c)), 0)
    expect("沒 init：主 session 寫檔", hook("guard-paths", proj, write(proj, proj / "a.txt")), 0)
    expect("沒 init：主 session 寫 .claude/ 下的檔", hook("guard-paths", proj, write(proj, proj / ".claude" / "x.md")), 0)
    expect("沒 init：使用者自己的 agent 推送", hook("guard-shell", proj, sh(proj, "git push origin HEAD", "my-own-agent")), 0)
    for agent in (AGENT, "agent-workflow:tpm-writer", "dev-implementer"):
        expect(f"沒 init：{agent} push 仍擋", hook("guard-shell", proj, sh(proj, "git push origin HEAD:feature/x", agent)), 2)


# ───────── 5. 壞輸入 ─────────


def t_bad_input(ws: Path) -> None:
    bad = {
        "空字串": b"", "只有空白": b"  \n", "非 JSON": b"not json", "殘缺 JSON": b"{", "JSON 陣列": b"[]", "JSON 字串": b'"x"',
        "非 UTF-8": b"\xff\xfe\x00{", "缺 tool_input": json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash"}).encode(),
        "tool_input 不是物件": json.dumps({"tool_name": "Bash", "tool_input": "ls"}).encode(),
        "2MB": json.dumps({"x": "y" * 2_000_000}).encode(),
    }
    for label, raw in bad.items():
        for mode, want in (("guard-shell", 2), ("guard-paths", 2), ("envelope", 0)):
            r = hook(mode, ws, raw)
            expect(f"壞輸入 {label}：{mode} exit {want}", r, want)
            if want == 2:
                check(f"壞輸入 {label}：{mode} 訊息清楚", "hook 輸入有問題" in stderr(r), stderr(r))
    big = hook("guard-shell", ws, sh(ws, "echo " + "a" * 2_000_000))  # 不擋會拖到 timeout＝放行，等於繞過守門
    expect("2MB 指令：guard-shell 直接擋（不拖到 timeout）", big, 2)
    check("2MB 指令：訊息說明長度上限", "上限" in stderr(big), stderr(big))
    expect("100KB 指令：guard-shell 在上限內照常檢查", hook("guard-shell", ws, sh(ws, "echo " + "a" * 100_000)), 0)
    expect("2MB Write 內容：guard-paths 只看路徑、放行", hook("guard-paths", ws, {**write(ws, ws / "docs" / "a.md"), "tool_input": {"file_path": str(ws / "docs" / "a.md"), "content": "y" * 2_000_000}}), 0)
    expect("用法錯誤：exit 1（非阻擋）", subprocess.run([PY, str(HOOKS / "awf_hook.py"), "nope"], capture_output=True, input=b"{}"), 1)


def main() -> int:
    try:
        ws, repo = init_ws()
        t_fallback(ws, repo)
        t_launcher(ws, repo)
        t_latency(ws, repo)
        t_no_init()
        t_bad_input(ws)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
    print(f"test_entry：{passed} 通過，{failed} 失敗")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
