#!/usr/bin/env python3
"""init 煙霧測試：在獨立的暫存目錄跑 init.py，檢查
  1. 佔位符全被替換（產出裡沒有 {{）
  2. 必要檔存在、沒有殘留 .tmpl、JSON 檔合法
  3. 在暫存專案裡跑 golden-set、build.py --check、check_workflows.mjs（有 node 才跑）、check_agents.py
  4. --hooks project 模式：hooks 複製進專案、settings.json 指向 awf_hook.py，並用專案裡那份 hooks 跑 hook 測試
  5. 開發規範 profile（methodology）：預設 profile 時 sdlc-build 仍走 Red（測試先行，行為不變）；
     用非預設 profile＋--answers 答案檔再 init 一次，確認答案寫進 methodology.json、build.py --check、
     check_workflows.mjs 乾跑時 Red 關掉改走 Tests、merge_gate selftest、check_rules 依 profile 的 commit regex 檢查；
     答案檔的 main_session_push_merge=true 時主 session 的 git／GitHub 限制放寬
  6. merge gate CI 預設不放，加 --with-merge-gate 才放
  7. 套用到既有專案：帶使用者 agent、skill、command、同名 workflow、settings、CLAUDE.md、CI 的暫存專案跑兩次 init，
     原檔逐位元保留、CLAUDE.md 只追加一段引用、settings 合併且不重複、使用者 agent 不受 hook 規則影響、預設沒有 merge gate CI
  8. 版本：plugin.json、marketplace.json（metadata 與 plugins[]）、init 寫的 .claude/teams/VERSION 三處一致
  9. 佔位符雙向一致：scaffold 用到的 ⊆ workflow.config.example（加 init 自己算的），且 example 的每一個 scaffold 都有用到
 10. 升級：init → 自訂 methodology／guard.config／〈專案規則〉並弄舊模版檔 → --upgrade → 自訂保留、模版檔更新、
     列出缺少的 profile 欄位、--adopt-new-fields 只補缺的，build.py --check 與 golden-set 通過；--force 也保留自訂；
     自訂 RUNS_DIR init 後跑沒給 --config 的 --force，workflow 的 runs 路徑與 workflow.config 紀錄都不變
用法：python3 tests/smoke_init.py [設定檔]   （預設 workflow.config.example）
環境變數 KEEP=1 時保留暫存目錄方便除錯。
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = sys.executable
WORKFLOWS = ("sdlc-plan", "sdlc-design", "sdlc-design-revise", "sdlc-build", "sdlc-sync", "sdlc-test", "sdlc-deliver", "sdlc-merge",
             "event-brief", "event-upstream", "event-meeting", "event-pr-review", "event-reqs-change", "event-decision", "event-demo-check", "event-post-merge")
REQUIRED = [
    "CLAUDE.md",
    ".claude/settings.json",
    ".claude/teams/README.md",
    ".claude/teams/rules.md",
    ".claude/teams/guard.config.json",
    ".claude/teams/schemas/envelope.schema.json",
    ".claude/teams/tools/save_run.py",
    ".claude/teams/tools/check_rules.py",
    ".claude/teams/tools/check_envelope.py",
    ".claude/teams/tools/build.py",
    ".claude/teams/golden-set/run.py",
    ".claude/teams/tools/merge_gate.py",
    ".claude/teams/tools/publish.py",
    ".claude/teams/tools/sync_branch.py",
    ".claude/teams/methodology.json",
    ".claude/teams/schemas/methodology.schema.json",
    ".claude/teams/methodologies/default.json",
    *[f".claude/workflows/{w}.js" for w in WORKFLOWS],
]

SELFTEST_REQUIRED = ("merge_gate.py", "publish.py", "sync_branch.py", "checklist.py")


def selftest_tools(tools: Path) -> list[Path]:
    """tools/ 裡有實作 --selftest 的 .py（原始碼出現 "--selftest" 字串）。"""
    return sorted(p for p in tools.glob("*.py") if '"--selftest"' in p.read_text(encoding="utf-8"))


fail = False
GH_EXPR = re.compile(r"\$\{\{.*?\}\}")  # GitHub Actions 的 ${{ … }} 運算式不是佔位符


def ok(msg: str) -> None:
    print(f"  [通過] {msg}")


def bad(msg: str, detail: str = "") -> None:
    global fail
    fail = True
    print(f"  [失敗] {msg}")
    for line in detail.strip().splitlines()[-20:]:
        print(f"    {line}")


def skip(msg: str) -> None:
    print(f"  [略過] {msg}")


def run(argv: list[str], cwd: Path, env: dict | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                          stdin=subprocess.DEVNULL, env={**os.environ, "PYTHONUTF8": "1", **(env or {})})


def last_line(s: str) -> str:
    lines = [l for l in s.strip().splitlines() if l.strip()]
    return lines[-1] if lines else ""


def read_kv(path: Path, key: str) -> str:
    val = ""
    for line in path.read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s.startswith(key) and s[len(key):].lstrip().startswith("="):
            val = s.split("=", 1)[1].strip().strip("\"'")
    return val


def init(target: Path, config: Path, *extra: str, env: dict) -> bool:
    r = run([PY, str(ROOT / "init.py"), "--target", str(target), "--config", str(config), *extra], cwd=target.parent, env=env)
    if r.returncode == 0:
        ok(f"init.py {' '.join(extra) or '(預設)'} 結束碼 0")
        return True
    bad(f"init.py {' '.join(extra)} 失敗，輸出如下：", r.stdout + r.stderr)
    return False


def check_output(target: Path, runs_dir: str) -> None:
    print("== 佔位符 ==")
    left = []
    for p in sorted(target.rglob("*")):
        if p.is_file():
            try:
                text = p.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            for i, line in enumerate(text.splitlines(), 1):
                if "{{" in GH_EXPR.sub("", line):
                    left.append(f"{p.relative_to(target).as_posix()}:{i}:{line.strip()[:120]}")
    if left:
        bad("殘留佔位符：", "\n".join(left[:50]))
    else:
        ok("沒有殘留 {{…}}")

    print("== 必要檔 ==")
    for f in REQUIRED:
        if (target / f).is_file():
            ok(f)
        else:
            bad(f"缺 {f}")
    if (target / runs_dir / "README.md").is_file():
        ok(f"run 目錄說明（{runs_dir}/README.md）")
    else:
        bad(f"缺 run 目錄說明（{runs_dir}/README.md）")
    if (target / ".mcp.json").is_file() or (target / ".mcp.json.example").is_file():
        ok(".mcp.json(.example)")
    else:
        bad("缺 .mcp.json 或 .mcp.json.example")
    tmpl = [p.relative_to(target).as_posix() for p in target.rglob("*.tmpl")]
    if tmpl:
        bad(f"殘留 .tmpl：{tmpl}")
    else:
        ok("沒有殘留 .tmpl")
    for j in (".claude/settings.json", ".mcp.json", ".mcp.json.example", ".claude/teams/guard.config.json"):
        p = target / j
        if not p.is_file():
            continue
        try:
            json.loads(p.read_text(encoding="utf-8"))
            ok(f"{j} 是合法 JSON")
        except json.JSONDecodeError as e:
            bad(f"{j} 不是合法 JSON：{e}")


def golden_argv() -> list[str]:
    try:
        import jsonschema  # noqa: F401
        return [PY, ".claude/teams/golden-set/run.py"]
    except ImportError:
        pass
    if shutil.which("uv"):
        return ["uv", "run", "--no-project", "--with", "jsonschema", "python3", ".claude/teams/golden-set/run.py"]
    return [PY, ".claude/teams/golden-set/run.py"]


def check_tools(target: Path) -> None:
    print("== 工具檢查 ==")
    if (target / ".claude/teams/golden-set/run.py").is_file():
        r = run(golden_argv(), cwd=target)
        out = r.stdout + r.stderr
        if r.returncode == 0:
            ok(f"golden-set：{last_line(out)}")
            if "未裝 jsonschema" in out:
                skip("沒有 jsonschema，golden-set 只跑了 schemalib（pip install jsonschema 或裝 uv）")
        else:
            bad("golden-set 失敗：", out)
    else:
        skip("沒有 golden-set/run.py")
    if (target / ".claude/teams/tools/build.py").is_file():
        r = run([PY, ".claude/teams/tools/build.py", "--check"], cwd=target)
        ok("build.py --check") if r.returncode == 0 else bad("build.py --check 失敗：", r.stdout + r.stderr)
    if (target / ".claude/teams/tools/check_workflows.mjs").is_file():
        node = shutil.which("node")
        if node:
            r = run([node, ".claude/teams/tools/check_workflows.mjs", "--agents-dir", str(ROOT / "plugin" / "agents")], cwd=target)
            ok(f"check_workflows.mjs：{last_line(r.stdout)}") if r.returncode == 0 else bad("check_workflows.mjs 失敗：", r.stdout + r.stderr)
        else:
            skip("沒有 node，略過 check_workflows.mjs")
    # 有 --selftest 的工具全部跑（自動發現：新增 selftest 不用改這裡）；必備的幾支缺了算失敗
    tools = target / ".claude/teams/tools"
    for must in SELFTEST_REQUIRED:
        if not (tools / must).is_file():
            bad(f"缺 .claude/teams/tools/{must}")
    for tool in selftest_tools(tools):
        r = run([PY, f".claude/teams/tools/{tool.name}", "--selftest"], cwd=target)
        ok(f"{tool.name} --selftest：{last_line(r.stdout)}") if r.returncode == 0 else bad(f"{tool.name} --selftest 失敗：", r.stdout + r.stderr)
    # plugin 安裝模式：agent 在模版的 plugin/agents，不在專案 .claude/agents
    if (target / ".claude/teams/tools/check_agents.py").is_file():
        r = run([PY, ".claude/teams/tools/check_agents.py", "--plugin-dir", str(ROOT / "plugin")], cwd=target)
        ok(f"check_agents.py：{last_line(r.stdout)}") if r.returncode == 0 else bad("check_agents.py 失敗：", r.stdout + r.stderr)


def dry_run_phases(target: Path, workflow: str) -> str | None:
    """check_workflows.mjs 乾跑輸出裡某支 workflow 經過的 phases（沒有 node 回 None）。"""
    node = shutil.which("node")
    if not node:
        return None
    r = run([node, ".claude/teams/tools/check_workflows.mjs", "--agents-dir", str(ROOT / "plugin" / "agents")], cwd=target)
    m = re.search(rf"^ok\s+{re.escape(workflow)}\.js.*phases: ([^）]*)）", r.stdout, re.M)
    return m.group(1) if m else ""


def check_default_methodology(target: Path) -> None:
    print("== 開發規範 profile（預設） ==")
    try:
        prof = json.loads((target / ".claude/teams/methodology.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        bad(f"methodology.json 讀不到：{e}")
        return
    good = prof.get("name") == "default" and prof["stages"]["build"]["test_first"] is True and prof["conventions"]["pr_size"]["max_lines"] == 300
    ok("預設 profile＝default（測試先行、PR 上限 300 行）") if good else bad(f"預設 profile 不對：{prof.get('name')}")
    gcfg = json.loads((target / ".claude/teams/guard.config.json").read_text(encoding="utf-8"))
    strict = gcfg.get("main_session_git_rules") is True and gcfg.get("main_session_github_rules") is True
    ok("預設：主 session 的 git／GitHub 限制都開著") if strict else bad("預設的 main_session_git_rules／main_session_github_rules 應為 true")
    ns = gcfg.get("team_agent_namespaces")
    ok(f"team_agent_namespaces＝{ns}") if ns == ["agent-workflow:"] else bad(f"team_agent_namespaces 不對：{ns}")
    phases = dry_run_phases(target, "sdlc-build")
    if phases is None:
        skip("沒有 node，略過 Red 階段檢查")
    elif phases.startswith("Red") and "Tests" not in phases:
        ok(f"預設 profile：sdlc-build 先走 Red（{phases}），既有行為不變")
    else:
        bad(f"預設 profile 的 sdlc-build 沒有先走 Red：{phases}")


ANSWERS = {
    "methodology": "lightweight",
    "code_style": {"description": "提交前跑 ruff", "tool": "ruff", "command": "ruff format --check ."},
    "commit": {"format": "<type>: <摘要>（示範客戶 ACME 的慣例）", "examples": {"feat": "feat: <Story> 實作"}, "check_regex": "^(feat|fix|docs|test|chore): .+"},
    "branch_prefix": "feat/",
    "sync_strategy": "transit",
    "pr_size": {"max_lines": 400, "max_files": None},
    "doc_format": "Markdown",
    "main_session_push_merge": True,
}


def check_answers_methodology(target: Path) -> None:
    print("== 開發規範 profile（lightweight＋--answers） ==")
    try:
        prof = json.loads((target / ".claude/teams/methodology.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        bad(f"methodology.json 讀不到：{e}")
        return
    conv = prof.get("conventions", {})
    want = {
        "name": prof.get("name") == "lightweight",
        "test_first=false": prof.get("stages", {}).get("build", {}).get("test_first") is False,
        "code_style.command": conv.get("code_style", {}).get("command") == "ruff format --check .",
        "commit.check_regex": conv.get("commit", {}).get("check_regex") == ANSWERS["commit"]["check_regex"],
        "branch.prefix": conv.get("branch", {}).get("prefix") == "feat/",
        "sync.strategy": conv.get("sync", {}).get("strategy") == "transit",
        "pr_size": conv.get("pr_size", {}).get("max_lines") == 400 and conv.get("pr_size", {}).get("max_files") is None,
    }
    miss = [k for k, v in want.items() if not v]
    ok("答案檔寫進 methodology.json（profile、code style、commit、分支、同步策略、PR 大小）") if not miss else bad(f"methodology.json 沒套用答案：{miss}")
    gcfg = json.loads((target / ".claude/teams/guard.config.json").read_text(encoding="utf-8"))
    loose = gcfg.get("main_session_git_rules") is False and gcfg.get("main_session_github_rules") is False
    ok("main_session_push_merge=true：主 session 的 git／GitHub 限制放寬") if loose else bad(f"答案檔的 main_session_push_merge 沒套用：{gcfg.get('main_session_git_rules')}, {gcfg.get('main_session_github_rules')}")
    ok("分支前綴加進 allowed_branch_prefixes") if "feat/" in gcfg.get("allowed_branch_prefixes", "") else bad(f"allowed_branch_prefixes 沒有 feat/：{gcfg.get('allowed_branch_prefixes')}")
    build = (target / ".claude/workflows/sdlc-build.js").read_text(encoding="utf-8")
    ok("sdlc-build.js 內嵌了 lightweight profile") if '"name":"lightweight"' in build else bad("sdlc-build.js 沒有內嵌 lightweight profile")
    ok("workflow 的分支用答案的前綴 feat/") if "feat/${A.epic}" in build else bad("sdlc-build.js 的分支前綴沒換成 feat/")
    r = run([PY, ".claude/teams/tools/build.py", "--check"], cwd=target)
    ok(f"build.py --check：{last_line(r.stdout)}") if r.returncode == 0 else bad("build.py --check 失敗：", r.stdout + r.stderr)
    phases = dry_run_phases(target, "sdlc-build")
    if phases is None:
        skip("沒有 node，略過 check_workflows.mjs")
    elif phases and "Red" not in phases.split("→") and phases.endswith("Tests"):
        ok(f"test_first=false：sdlc-build 跳過 Red，實作後走 Tests（{phases}）")
    else:
        bad(f"test_first=false 的 sdlc-build phases 不對：{phases!r}")
    node = shutil.which("node")
    if node:
        r = run([node, ".claude/teams/tools/check_workflows.mjs", "--agents-dir", str(ROOT / "plugin" / "agents")], cwd=target)
        ok(f"check_workflows.mjs：{last_line(r.stdout)}") if r.returncode == 0 else bad("check_workflows.mjs 失敗：", r.stdout + r.stderr)
    r = run([PY, ".claude/teams/tools/merge_gate.py", "--selftest"], cwd=target)
    ok(f"merge_gate.py --selftest：{last_line(r.stdout)}") if r.returncode == 0 else bad("merge_gate.py --selftest 失敗：", r.stdout + r.stderr)
    r = run(golden_argv(), cwd=target)
    ok(f"golden-set（含專案的 methodology.json）：{last_line(r.stdout + r.stderr)}") if r.returncode == 0 else bad("golden-set 失敗：", r.stdout + r.stderr)
    # check_rules --git：profile 設了 commit regex，就要抓出不符格式的 commit 標題
    if shutil.which("git"):
        repo = target / "demo-repo"
        repo.mkdir()
        gitenv = {"GIT_AUTHOR_NAME": "acme", "GIT_AUTHOR_EMAIL": "acme@example.com", "GIT_COMMITTER_NAME": "acme", "GIT_COMMITTER_EMAIL": "acme@example.com"}
        run(["git", "init", "-q"], cwd=repo, env=gitenv)
        attr = gcfg.get("attribution_line") or "Assisted-by: Claude"
        for msg in ("init", "feat: 匯出 CSV", "隨手修一下"):
            run(["git", "commit", "-q", "--allow-empty", "-m", msg, "-m", attr], cwd=repo, env=gitenv)
        r = run([PY, ".claude/teams/tools/check_rules.py", "--git", str(repo), "HEAD~2..HEAD"], cwd=target)
        good = r.returncode == 1 and "R6" in r.stdout and "隨手修一下" in r.stdout and "匯出 CSV" not in r.stdout
        ok("check_rules --git 依 profile 的 commit regex 抓出不符格式的標題") if good else bad("check_rules --git 沒有依 profile 檢查 commit 格式：", r.stdout + r.stderr)
    else:
        skip("沒有 git，略過 check_rules --git")


def check_extensions(target: Path, work: Path) -> None:
    """使用者自己的 agent 加進 workflow（/add-agent 做的事）：沒登記守門角色時 build.py 擋下；登記並放好定義檔後，
    build 通過、乾跑會派出擴充 agent，hook 照對應角色擋 push。在複本上測，不影響其他檢查。"""
    t = work / "project-ext"
    shutil.copytree(target, t)
    teams = t / ".claude/teams"
    meth = json.loads((teams / "methodology.json").read_text(encoding="utf-8"))
    meth["extensions"] = [{"name": "db", "agent": "acme-db-reviewer", "stage": "test", "instructions": "檢查 migration 能不能回滾"}]
    (teams / "methodology.json").write_text(json.dumps(meth, ensure_ascii=False), encoding="utf-8")
    r = run([PY, ".claude/teams/tools/build.py"], cwd=t)
    ok("擴充 agent：沒登記 extension_agents 時 build.py 擋下") if r.returncode == 1 and "extension_agents" in r.stdout else bad("擴充 agent：沒登記卻沒擋", r.stdout + r.stderr)
    guard = json.loads((teams / "guard.config.json").read_text(encoding="utf-8"))
    guard.setdefault("extension_agents", {})["acme-db-reviewer"] = "tpm-reviewer"
    (teams / "guard.config.json").write_text(json.dumps(guard, ensure_ascii=False), encoding="utf-8")
    (t / ".claude/agents").mkdir(parents=True, exist_ok=True)
    (t / ".claude/agents/acme-db-reviewer.md").write_text("---\nname: acme-db-reviewer\ndescription: 示範客戶 ACME 的 migration 審查\ntools: Read, Grep, Glob, Bash\n---\n檢查 migration。\n", encoding="utf-8")
    r = run([PY, ".claude/teams/tools/build.py"], cwd=t)
    ok("擴充 agent：登記後 build.py 通過") if r.returncode == 0 else bad("擴充 agent：登記後 build.py 失敗", r.stdout + r.stderr)
    node = shutil.which("node")
    if node:
        r = run([node, ".claude/teams/tools/check_workflows.mjs", "--agents-dir", str(ROOT / "plugin" / "agents")], cwd=t)
        ok("擴充 agent：workflow 乾跑通過") if r.returncode == 0 else bad("擴充 agent：workflow 乾跑失敗", r.stdout[-800:])
    else:
        skip("擴充 agent：沒有 node，略過 workflow 乾跑")
    data = {"tool_name": "Bash", "agent_type": "acme-db-reviewer", "cwd": str(t), "tool_input": {"command": "git push origin feature/x"}}
    hook = subprocess.run([PY, str(ROOT / "plugin/hooks/awf_hook.py"), "guard-shell"], input=json.dumps(data).encode("utf-8"),
                          capture_output=True, env={**os.environ, "CLAUDE_PROJECT_DIR": str(t)})
    ok("擴充 agent：hook 照 tpm-reviewer 規則擋 push") if hook.returncode == 2 else bad(f"擴充 agent：hook 沒擋 push（exit {hook.returncode}）")


def check_merge_gate_optin(target: Path, config: Path, env: dict) -> None:
    print("== merge gate CI（預設不放，--with-merge-gate 才放） ==")
    ci = target / ".github/workflows/agent-merge-gate.yml"
    if ci.exists():
        bad("預設不應放 .github/workflows/agent-merge-gate.yml（沒有 gate App 時會讓每個 PR 失敗）")
    else:
        ok("預設沒有 .github/workflows/agent-merge-gate.yml")
    if not init(target, config, "--with-merge-gate", env=env):
        return
    if not ci.is_file():
        bad("--with-merge-gate 後仍沒有 agent-merge-gate.yml")
        return
    t = ci.read_text(encoding="utf-8")
    need = ["merge_gate.py", "pull_request_target", "merge_group", "steps.pr.outputs.base_ref", "agent-merge-gate/decision"]
    miss = [k for k in need if k not in t]
    ok("--with-merge-gate：agent-merge-gate.yml 從 base 執行 gate、以 App 回報 check") if not miss else bad(f"agent-merge-gate.yml 缺 {miss}")


# 套用到既有專案：使用者原本的 agent、skill、command、workflow、CI、settings、CLAUDE.md
EXISTING_FILES = {
    ".claude/agents/dev-helper.md": "---\nname: dev-helper\ndescription: 使用者自己的 agent（示範客戶 ACME）\n---\n幫忙整理程式。\n",
    ".claude/agents/tpm-notes.md": "---\nname: tpm-notes\ndescription: 使用者自己的筆記 agent\n---\n記筆記。\n",
    ".claude/skills/my-skill/SKILL.md": "---\nname: my-skill\ndescription: 使用者自己的 skill\n---\n照做。\n",
    ".claude/commands/brief.md": "使用者自己的 /brief 指令（和 plugin 的 brief 同名）\n",
    ".claude/workflows/sdlc-plan.js": "// 使用者自己的同名 workflow\nexport const meta = { name: 'sdlc-plan' }\n",
    ".github/workflows/ci.yml": "name: mine\non: [push]\n",
}
EXISTING_CLAUDE = "# 示範客戶 ACME 的專案\r\n\r\n原本的說明，用 CRLF 換行、結尾沒有換行。"
EXISTING_SETTINGS = {
    "permissions": {"allow": ["Bash(npm test)"]},
    "env": {"MY_VAR": "1"},
    "model": "sonnet",
    "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo user-hook"}]}],
              "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}]},
}


def project_hook(target: Path, mode: str, data: dict) -> int:
    """用專案 .claude/hooks 裡的那份 hooks 跑一次，回傳 exit code。"""
    raw = json.dumps({"cwd": str(target), **data}, ensure_ascii=False)
    r = subprocess.run([PY, str(target / ".claude/hooks/awf_hook.py"), mode], input=raw.encode("utf-8"), capture_output=True,
                       env={**os.environ, "CLAUDE_PROJECT_DIR": str(target), "PYTHONUTF8": "1"})
    return r.returncode


def check_existing_project(work: Path, config: Path, env: dict) -> None:
    print("\n== 套用到既有專案（--hooks project，跑兩次，第二次加 --force） ==")
    target = work / "project-existing"
    for rel, body in EXISTING_FILES.items():
        (target / rel).parent.mkdir(parents=True, exist_ok=True)
        (target / rel).write_bytes(body.encode("utf-8"))
    (target / "CLAUDE.md").write_bytes(EXISTING_CLAUDE.encode("utf-8"))
    (target / ".claude/settings.json").write_text(json.dumps(EXISTING_SETTINGS, ensure_ascii=False), encoding="utf-8")
    if not (init(target, config, "--hooks", "project", env=env) and init(target, config, "--hooks", "project", "--force", env=env)):
        return
    changed = [rel for rel, body in EXISTING_FILES.items() if (target / rel).read_bytes() != body.encode("utf-8")]
    bad(f"既有專案：使用者的檔案被改動：{changed}") if changed else ok("既有專案：使用者的 agents、skills、commands、同名 workflow、CI 逐位元保留")

    got = (target / "CLAUDE.md").read_bytes().decode("utf-8")
    extra = got[len(EXISTING_CLAUDE):] if got.startswith(EXISTING_CLAUDE) else None
    imp = "@.claude/teams/agent-workflow.CLAUDE.md"
    if extra is None:
        bad("既有專案：CLAUDE.md 原內容被改動")
    elif got.count(imp) != 1 or len([l for l in extra.splitlines() if l.strip()]) > 3:
        bad(f"既有專案：CLAUDE.md 應只追加一段引用（重跑不重複），實際追加：{extra!r}")
    else:
        ok("既有專案：CLAUDE.md 原內容保留，只在結尾追加一段 @ 引用（跑兩次只有一段）")
    imported = target / ".claude/teams/agent-workflow.CLAUDE.md"
    ok("既有專案：模版說明寫在 .claude/teams/agent-workflow.CLAUDE.md") if imported.is_file() and "agent-workflow" in imported.read_text(encoding="utf-8") else bad("既有專案：缺 .claude/teams/agent-workflow.CLAUDE.md")

    try:
        s = json.loads((target / ".claude/settings.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        bad(f"既有專案：settings.json 讀不到：{e}")
        return
    kept = (s.get("permissions", {}).get("allow") == ["Bash(npm test)"] and s.get("env") == {"MY_VAR": "1"} and s.get("model") == "sonnet")
    ok("既有專案：settings.json 保留使用者的 permissions、env、其他欄位") if kept else bad(f"既有專案：settings.json 的使用者欄位沒保留：{s}")
    hooks = json.dumps(s.get("hooks", {}), ensure_ascii=False)
    good = ("echo user-hook" in hooks and "echo done" in hooks and hooks.count("guard-shell") == 1
            and hooks.count("guard-paths") == 1 and hooks.count("awf_hook.py") == 5)
    ok("既有專案：settings.json 保留使用者的 hooks，awf_hook.py 各模式只註冊一次") if good else bad(f"既有專案：settings.json 的 hooks 合併不對：{hooks}")

    if (target / ".github/workflows/agent-merge-gate.yml").exists():
        bad("既有專案：預設不應放 agent-merge-gate.yml")
    else:
        ok("既有專案：預設沒有 merge gate CI")

    bash = lambda c, a: {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": c}, "agent_type": a}
    write = lambda f, a: {"hook_event_name": "PreToolUse", "tool_name": "Write", "tool_input": {"file_path": str(target / f), "content": "x"}, "agent_type": a}
    stop = lambda a: {"hook_event_name": "SubagentStop", "agent_type": a, "last_assistant_message": "完成，沒有信封"}
    cases = [
        ("使用者 agent dev-helper push 功能分支", "guard-shell", bash("git push origin feature/x", "dev-helper"), 0),
        ("使用者 agent dev-helper 寫 src/", "guard-paths", write("src/a.py", "dev-helper"), 0),
        ("使用者 agent tpm-notes 寫 notes/", "guard-paths", write("notes/a.md", "tpm-notes"), 0),
        ("使用者 agent dev-helper 交件不需信封", "envelope", stop("dev-helper"), 0),
        ("plugin agent push 仍擋", "guard-shell", bash("git push origin feature/x", "agent-workflow:dev-implementer"), 2),
        ("plugin tpm-researcher 寫 src/ 仍擋", "guard-paths", write("src/a.py", "agent-workflow:tpm-researcher"), 2),
        ("plugin agent 交件仍需信封", "envelope", stop("agent-workflow:dev-verifier"), 2),
    ]
    wrong = [f"{name}（want {want} got {got}）" for name, mode, data, want in cases if (got := project_hook(target, mode, data)) != want]
    bad(f"既有專案：hook 規則不對：{wrong}") if wrong else ok("既有專案：使用者的 dev-helper／tpm-notes 不受模版角色規則影響，plugin 角色照舊")


def check_project_hooks(target: Path) -> None:
    print("== --hooks project ==")
    hooks = target / ".claude" / "hooks"
    for f in ("awf_hook.py", "guard.py", "envelope_hook.py", "awf_config.py"):
        ok(f".claude/hooks/{f}") if (hooks / f).is_file() else bad(f"缺 .claude/hooks/{f}")
    if (hooks / "hooks.json").exists():
        bad(".claude/hooks/hooks.json 不該複製（那是 plugin 的註冊檔）")
    sh_left = [p.name for p in hooks.rglob("*.sh")]
    bad(f"hooks 裡還有 bash 腳本：{sh_left}") if sh_left else ok("hooks 裡沒有 bash 腳本")
    settings = target / ".claude" / "settings.json"
    try:
        cfg = json.loads(settings.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        bad(f"settings.json 讀不到：{e}")
        return
    pre = {e["matcher"]: e["hooks"][0] for e in cfg.get("hooks", {}).get("PreToolUse", [])}
    shell = pre.get("Bash|PowerShell", {})
    cmd = shell.get("command", "")
    if "awf_hook.py" in cmd and cmd.rstrip().endswith("guard-shell"):
        ok(f"settings.json 的 shell 守門：{cmd}")
    else:
        bad(f"settings.json 的 shell 守門不對：{shell}")
    paths = pre.get("Edit|Write|NotebookEdit|Read|Grep", {})
    ok("settings.json 的 guard-paths 也守 Read、Grep") if paths.get("command", "").rstrip().endswith("guard-paths") else bad(f"settings.json 的 guard-paths matcher 不含 Read|Grep：{sorted(pre)}")
    for name, h in (("guard-shell", shell), ("guard-paths", paths)):
        exe = h.get("command", "").replace("& ", "", 1).strip().split('"')
        abs_py = len(exe) > 1 and Path(exe[1]).is_absolute()
        good = abs_py and h.get("timeout") == 10 and h.get("onFailure") == "block"
        ok(f"{name}：Python 絕對路徑、timeout 10、onFailure block") if good else bad(f"{name} 的 hook 設定不對（要 Python 絕對路徑、timeout 10、onFailure block）：{h}")
    if os.name == "nt":
        ok("Windows：hook 指定 PowerShell 執行") if shell.get("shell") == "powershell" else bad("Windows 的 hook 沒有指定 shell: powershell")
    # 用專案裡複製出去的那份 hooks 跑完整 hook 測試（也會檢查 settings.json 的註冊）
    r = run([PY, str(ROOT / "plugin" / "hooks" / "tests" / "run.py"), "--project", str(target), "--hooks-dir", str(hooks)], cwd=target)
    ok(f"專案內 hooks 的 hook 測試：{last_line(r.stdout)}") if r.returncode == 0 else bad("專案內 hooks 的 hook 測試失敗：", r.stdout + r.stderr)


def check_versions(target: Path) -> None:
    print("== 版本（plugin.json、marketplace.json、.claude/teams/VERSION） ==")
    try:
        pv = json.loads((ROOT / "plugin/.claude-plugin/plugin.json").read_text(encoding="utf-8")).get("version")
        mk = json.loads((ROOT / ".claude-plugin/marketplace.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        bad(f"讀不到 plugin.json 或 marketplace.json：{e}")
        return
    vf = target / ".claude/teams/VERSION"
    found = {"plugin.json": pv, "marketplace metadata": mk.get("metadata", {}).get("version"),
             **{f"marketplace plugins[{p.get('name')}]": p.get("version") for p in mk.get("plugins", [])},
             "VERSION": vf.read_text(encoding="utf-8").strip() if vf.is_file() else None}
    if pv and len(set(found.values())) == 1:
        ok(f"版本一致：{pv}（plugin.json、marketplace.json、.claude/teams/VERSION）")
    else:
        bad(f"版本不一致：{found}（發佈時三處要一起遞增，否則使用者收不到更新）")


def check_placeholders() -> None:
    print("== 佔位符雙向一致（scaffold ↔ workflow.config.example） ==")
    sys.path.insert(0, str(ROOT))
    import init as init_mod  # noqa: E402  只為了拿 COMPUTED 與 read_kv，不執行 main
    used: set[str] = set()
    for p in (ROOT / "scaffold").rglob("*"):
        if p.is_file() and "__pycache__" not in p.parts:
            try:
                used |= set(init_mod.PH.findall(p.read_text(encoding="utf-8")))
            except UnicodeDecodeError:
                continue
    conf = set(init_mod.read_kv(ROOT / "workflow.config.example"))
    computed = set(init_mod.COMPUTED)
    extra_used, unused = sorted(used - conf - computed), sorted(conf - used)
    ok(f"scaffold 用到的佔位符都在 workflow.config.example（{len(used - computed)} 個，另有 init 自算 {len(computed)} 個）") if not extra_used \
        else bad(f"scaffold 用到、workflow.config.example 沒有的佔位符：{extra_used}")
    ok("workflow.config.example 的每個鍵 scaffold 都有用到") if not unused else bad(f"workflow.config.example 有、scaffold 沒用到的佔位符：{unused}")


RULE_MARK = "- 示範專案規則：ACME 的 API 一律帶 trace id。"
CLAUDE_SPECIFIC = "- 示範專案特定：ACME 的部署窗口是週二。"


def check_upgrade(work: Path, config: Path, env: dict) -> None:
    print("\n== 升級（init → 自訂 → 弄舊模版檔 → --upgrade） ==")
    t = work / "project-upgrade"
    t.mkdir()
    if not init(t, config, env=env):
        return
    teams = t / ".claude/teams"
    meth_p, guard_p, rules_p = teams / "methodology.json", teams / "guard.config.json", teams / "rules.md"
    meth = json.loads(meth_p.read_text(encoding="utf-8"))
    meth["conventions"].pop("sync", None)  # 模擬舊版 profile 沒有這個欄位
    meth["conventions"]["pr_size"] = {"max_lines": 123, "max_files": 7}
    meth_p.write_text(json.dumps(meth, ensure_ascii=False, indent=2), encoding="utf-8")
    guard = json.loads(guard_p.read_text(encoding="utf-8"))
    guard["main_session_git_rules"] = False
    guard["auto_merge"]["upstream_authorized_by"] = {"acme/app": {"by": "示範決策者", "link": "https://example.com/acme", "scope": "feature/*", "expires": "2099-01-01"}}
    guard["extension_agents"]["acme-db-reviewer"] = "tpm-reviewer"
    guard["auto_merge"].pop("carry_verdict_on_rebase", None)  # 模擬舊版 guard.config 缺新鍵
    guard_p.write_text(json.dumps(guard, ensure_ascii=False, indent=2), encoding="utf-8")
    rules_p.write_text(rules_p.read_text(encoding="utf-8") + "\n" + RULE_MARK + "\n", encoding="utf-8")
    claude = t / "CLAUDE.md"
    claude.write_text(claude.read_text(encoding="utf-8") + "\n" + CLAUDE_SPECIFIC + "\n", encoding="utf-8")
    # 弄舊模版：工具檔被改過、通用規則被改過、少一支 workflow、多一個這版已不提供的工具、版本戳是舊的
    tool = teams / "tools/schemalib.py"
    tool.write_text(tool.read_text(encoding="utf-8") + "\n# 舊版殘留\n", encoding="utf-8")
    rules_p.write_text(rules_p.read_text(encoding="utf-8").replace("## 專案規則", "（舊版通用規則）\n\n## 專案規則", 1), encoding="utf-8")
    (t / ".claude/workflows/sdlc-sync.js").unlink()
    (teams / "tools/old_tool.py").write_text("# 舊版才有的工具\n", encoding="utf-8")
    (teams / "VERSION").write_text("0.1.0\n", encoding="utf-8")

    r = run([PY, str(ROOT / "init.py"), "--target", str(t), "--upgrade"], cwd=t.parent, env=env)
    out = r.stdout + r.stderr
    if r.returncode != 0:
        bad("init.py --upgrade 失敗：", out)
        return
    new_ver = json.loads((ROOT / "plugin/.claude-plugin/plugin.json").read_text(encoding="utf-8"))["version"]
    ok(f"--upgrade 結束碼 0，顯示 0.1.0 → {new_ver}") if f"0.1.0 → {new_ver}" in out else bad("--upgrade 沒顯示舊版 → 新版", out)
    ok("--upgrade 沒給 --config 時沿用 .claude/teams/workflow.config") if "teams/workflow.config" in out.replace("\\", "/") else bad("--upgrade 沒沿用 init 記下的設定值", out)
    vf = (teams / "VERSION").read_text(encoding="utf-8").strip()
    ok(f"升級後 VERSION＝{vf}") if vf == new_ver else bad(f"升級後 VERSION 是 {vf}，不是 {new_ver}")

    src_tool = (ROOT / "scaffold/.claude/teams/tools/schemalib.py").read_text(encoding="utf-8")
    good = tool.read_text(encoding="utf-8") == src_tool and (t / ".claude/workflows/sdlc-sync.js").is_file()
    ok("升級：被改過的工具檔換成新版、缺的 workflow 補上") if good else bad("升級：模版檔沒有更新")
    ok("升級：列出這版已不提供的舊檔（old_tool.py，不刪）") if "old_tool.py" in out and (teams / "tools/old_tool.py").is_file() else bad("升級：沒列出舊版殘留的工具檔", out)

    m2 = json.loads(meth_p.read_text(encoding="utf-8"))
    ok("升級：methodology.json 原樣保留") if m2 == meth else bad("升級：methodology.json 被改了")
    ok("升級：列出缺少的 profile 欄位 conventions.sync 與預設值") if "conventions.sync" in out and "--adopt-new-fields" in out else bad("升級：沒列出缺少的 conventions.sync", out)
    g2 = json.loads(guard_p.read_text(encoding="utf-8"))
    kept = (g2.get("main_session_git_rules") is False and g2["auto_merge"].get("upstream_authorized_by") == guard["auto_merge"]["upstream_authorized_by"]
            and g2.get("extension_agents", {}).get("acme-db-reviewer") == "tpm-reviewer")
    ok("升級：guard.config.json 的 main_session_git_rules、upstream_authorized_by、extension_agents 保留") if kept else bad(f"升級：guard.config.json 的自訂值不見了：{g2}")
    ok("升級：guard.config.json 補上新版新增的鍵（auto_merge.carry_verdict_on_rebase）") if g2["auto_merge"].get("carry_verdict_on_rebase") is False else bad("升級：guard.config.json 沒補上新鍵")
    rules = rules_p.read_text(encoding="utf-8")
    ok("升級：rules.md〈專案規則〉之後保留、前面的通用規則更新") if RULE_MARK in rules and "（舊版通用規則）" not in rules else bad("升級：rules.md 的〈專案規則〉沒保留或通用規則沒更新")
    ok("升級：CLAUDE.md〈專案特定〉之後保留") if CLAUDE_SPECIFIC in claude.read_text(encoding="utf-8") else bad("升級：CLAUDE.md 的〈專案特定〉不見了")

    r = run([PY, ".claude/teams/tools/build.py", "--check"], cwd=t)
    ok("升級後 build.py --check") if r.returncode == 0 else bad("升級後 build.py --check 失敗：", r.stdout + r.stderr)
    r = run(golden_argv(), cwd=t)
    ok(f"升級後 golden-set：{last_line(r.stdout + r.stderr)}") if r.returncode == 0 else bad("升級後 golden-set 失敗：", r.stdout + r.stderr)

    r = run([PY, str(ROOT / "init.py"), "--target", str(t), "--upgrade", "--adopt-new-fields"], cwd=t.parent, env=env)
    m3 = json.loads(meth_p.read_text(encoding="utf-8"))
    good = r.returncode == 0 and "sync" in m3["conventions"] and m3["conventions"]["pr_size"] == {"max_lines": 123, "max_files": 7}
    ok("升級：--adopt-new-fields 補上 conventions.sync，既有的 pr_size 不變") if good else bad("升級：--adopt-new-fields 不對：", r.stdout + r.stderr)
    r = run([PY, ".claude/teams/tools/build.py", "--check"], cwd=t)
    ok("--adopt-new-fields 後 build.py --check") if r.returncode == 0 else bad("--adopt-new-fields 後 build.py --check 失敗：", r.stdout + r.stderr)

    tool.write_text(tool.read_text(encoding="utf-8") + "\n# 舊版殘留\n", encoding="utf-8")
    if init(t, config, "--force", env=env):
        g4 = json.loads(guard_p.read_text(encoding="utf-8"))
        good = (json.loads(meth_p.read_text(encoding="utf-8")) == m3 and g4.get("main_session_git_rules") is False
                and g4.get("extension_agents", {}).get("acme-db-reviewer") == "tpm-reviewer" and RULE_MARK in rules_p.read_text(encoding="utf-8")
                and tool.read_text(encoding="utf-8") == src_tool)
        ok("--force：模版檔更新，methodology、guard.config、〈專案規則〉照升級規則保留") if good else bad("--force 蓋掉了使用者的設定")
    check_force_keeps_values(work, config, env)


def check_force_keeps_values(work: Path, config: Path, env: dict) -> None:
    """升級：用自訂 RUNS_DIR init，再跑沒給 --config 的 --force：佔位符沿用紀錄檔，workflow 裡的 runs 路徑與紀錄檔都不變。"""
    print("\n== 升級：--force 沒給 --config 時沿用 init 記下的佔位符值 ==")
    t = work / "project-force-values"
    t.mkdir()
    custom = work / "custom.config"
    custom_runs = "_acme/agent-runs"
    lines = [f"RUNS_DIR={custom_runs}" if l.strip().startswith("RUNS_DIR") and "=" in l else l
             for l in config.read_text(encoding="utf-8").splitlines()]
    custom.write_text("\n".join(lines) + "\n", encoding="utf-8")
    if not init(t, custom, env=env):
        return
    wf = t / ".claude/workflows/event-post-merge.js"
    record = t / ".claude/teams/workflow.config"
    want = f"const RUNS = '{custom_runs}'"
    if want not in wf.read_text(encoding="utf-8") or read_kv(record, "RUNS_DIR") != custom_runs:
        bad(f"自訂 RUNS_DIR 的 init 結果不對（{wf.name} 或 workflow.config 紀錄）")
        return
    r = run([PY, str(ROOT / "init.py"), "--target", str(t), "--force"], cwd=t.parent, env=env)
    if r.returncode != 0:
        bad("init.py --force（沒給 --config）失敗：", r.stdout + r.stderr)
        return
    text = wf.read_text(encoding="utf-8")
    ok(f"升級：--force 沒給 --config，workflow 的 runs 路徑仍是 {custom_runs}") if want in text else bad(
        f"升級：--force 沒給 --config，workflow 的 runs 路徑被換掉了：{[l for l in text.splitlines() if 'const RUNS' in l]}")
    got = read_kv(record, "RUNS_DIR")
    ok("升級：--force 沒給 --config，workflow.config 紀錄沒被範例預設值覆寫") if got == custom_runs else bad(f"升級：--force 後 workflow.config 紀錄的 RUNS_DIR 變成 {got}")
    g = json.loads((t / ".claude/teams/guard.config.json").read_text(encoding="utf-8"))
    ok("升級：--force 後 guard.config.json 的 runs_dir 仍是自訂值") if g.get("runs_dir") == custom_runs else bad(f"升級：--force 後 runs_dir 變成 {g.get('runs_dir')}")


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    config = Path(argv[0]).resolve() if argv else ROOT / "workflow.config.example"
    if not (ROOT / "init.py").is_file():
        print(f"找不到 {ROOT / 'init.py'}，無法測試", file=sys.stderr)
        return 2
    if not config.is_file():
        print(f"找不到設定檔 {config}", file=sys.stderr)
        return 2
    runs_dir = read_kv(config, "RUNS_DIR") or "_tpm/runs"

    # 踩坑手冊第 7 條：獨立 TMPDIR、固定 UTF-8
    work = Path(tempfile.mkdtemp(prefix="awf-smoke."))
    env = {"TMPDIR": str(work / "tmp"), "TEMP": str(work / "tmp"), "TMP": str(work / "tmp")}
    if os.name != "nt":
        env["LC_ALL"] = "en_US.UTF-8"
    (work / "tmp").mkdir()
    try:
        print("== 執行 init.py（--hooks plugin） ==")
        target = work / "project"
        target.mkdir()
        init(target, config, env=env)
        check_output(target, runs_dir)
        check_versions(target)
        check_placeholders()
        check_tools(target)
        check_default_methodology(target)
        check_merge_gate_optin(target, config, env)
        check_extensions(target, work)

        print("\n== 執行 init.py（非預設 methodology profile＋--answers 答案檔） ==")
        target3 = work / "project-methodology"
        target3.mkdir()
        answers = work / "answers.json"
        answers.write_text(json.dumps(ANSWERS, ensure_ascii=False), encoding="utf-8")
        if init(target3, config, "--answers", str(answers), env=env):
            check_output(target3, runs_dir)
            check_answers_methodology(target3)

        print("\n== 執行 init.py（--hooks project） ==")
        target2 = work / "project-hooks"
        target2.mkdir()
        if init(target2, config, "--hooks", "project", env=env):
            check_output(target2, runs_dir)
            check_project_hooks(target2)

        check_existing_project(work, config, env)
        check_upgrade(work, config, env)
    finally:
        if os.environ.get("KEEP") == "1":
            print(f"保留暫存目錄：{work}")
        else:
            shutil.rmtree(work, ignore_errors=True)

    if fail:
        print("init 煙霧測試：不合格", file=sys.stderr)
        return 1
    print("init 煙霧測試：通過")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
