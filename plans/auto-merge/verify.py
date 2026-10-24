"""auto-merge 計畫的結構性驗證：checklist.md 每一項的驗證指令都呼叫這支，用法 python3 plans/auto-merge/verify.py <ID>。

通過 exit 0，失敗 exit 1 並印出原因。只讀不寫（只在暫存目錄建測試用檔案）。
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "scaffold/.claude/teams/tools"
DEBATE = ROOT / "_tpm/runs/20261024-debate-auto-merge"
PY = sys.executable


def fail(msg: str) -> None:
    print(f"失敗：{msg}")
    sys.exit(1)


def read(rel: str) -> str:
    p = ROOT / rel
    if not p.is_file():
        fail(f"找不到 {rel}")
    return p.read_text(encoding="utf-8")


def sh(*args: str, cwd: Path = ROOT, stdin: str = "", env: dict | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(list(args), cwd=cwd, input=stdin, capture_output=True, text=True, encoding="utf-8", env={**os.environ, "PYTHONUTF8": "1", **(env or {})})


def tracked(pattern: str) -> list[str]:
    return [l for l in sh("git", "ls-files", pattern).stdout.splitlines() if l]


def envelope_ok(path: Path) -> None:
    sys.path.insert(0, str(TOOLS))
    import schemalib
    errs = schemalib.check(json.loads(path.read_text(encoding="utf-8")))
    if errs:
        fail(f"{path.name} 信封不合格：{errs[:3]}")


def hook(tool: str, command: str, agent: str = "", cfg: dict | None = None, files: dict | None = None) -> int:
    """用暫存工作區跑 plugin 的 shell 守門 hook，回傳 exit code。"""
    entry = ROOT / "plugin/hooks/awf_hook.py"
    if not entry.is_file():
        fail("找不到 plugin/hooks/awf_hook.py（SP1 尚未完成）")
    with tempfile.TemporaryDirectory() as d:
        teams = Path(d) / ".claude/teams"
        teams.mkdir(parents=True)
        for sub in ("schemas", "tools"):
            subprocess.run([PY, "-c", f"import shutil;shutil.copytree(r'{TOOLS.parent / sub}', r'{teams / sub}')"], check=True)
        conf = {"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app", "fork_repo": "acme-fork/app", **(cfg or {})}
        (teams / "guard.config.json").write_text(json.dumps(conf), encoding="utf-8")
        for rel, body in (files or {}).items():
            f = Path(d) / rel
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text(body, encoding="utf-8")
        payload = {"tool_name": tool, "tool_input": {"command": command}, "cwd": d}
        if agent:
            payload["agent_type"] = agent
        return sh(PY, str(entry), "guard-shell", stdin=json.dumps(payload), env={"CLAUDE_PROJECT_DIR": d}).returncode


def expect(code: int, want: int, what: str) -> None:
    if code != want:
        fail(f"{what}：預期 exit {want}，實際 {code}")


# ------------------------------------------------------------------ 各項檢查
def SP3_2() -> None:
    sys.path.insert(0, str(TOOLS))
    import checklist
    items = checklist.parse(read("plans/auto-merge/checklist.md"))
    missing = [it["id"] for it in items if not it["cmd"]]
    if not items or missing:
        fail(f"沒有項目或缺驗證指令：{missing}")
    ids = {it["id"] for it in items}
    for sp in ("SP0", "SP1", "SP2", "SP3", "SP4", "SP5", "SP6", "SP7", "SP8", "SP9"):
        if not any(i.startswith(sp + ".") for i in ids):
            fail(f"{sp} 沒有任何項目")


def SP0_1() -> None:
    p = ROOT / ".claude/settings.local.json"
    env = json.loads(p.read_text(encoding="utf-8")).get("env", {}) if p.is_file() else {}
    if str(env.get("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", os.environ.get("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", ""))) != "1":
        fail("Agent Teams 未開啟（需由使用者在 .claude/settings.local.json 的 env 設 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1）")
    if "_tpm/" not in read(".gitignore"):
        fail(".gitignore 沒有排除 _tpm/")


def SP0_2() -> None:
    files = [DEBATE / f"option-{n}.json" for n in range(1, 5)]
    for f in files:
        if not f.is_file():
            fail(f"缺 {f.relative_to(ROOT)}")
        envelope_ok(f)


def SP0_3() -> None:
    if not (DEBATE / "comparison.md").is_file():
        fail("缺 comparison.md")
    adr = ROOT / "docs/adr-agent-auto-merge.md"
    if not adr.is_file():
        fail("缺 docs/adr-agent-auto-merge.md")
    r = sh(PY, str(TOOLS / "check_rules.py"), "--audience", "owner", str(adr))
    if r.returncode:
        fail(f"check_rules 不通過：{r.stdout[-300:]}")


def SP0_4() -> None:
    run = json.loads((DEBATE / "run.json").read_text(encoding="utf-8")) if (DEBATE / "run.json").is_file() else {}
    st = run.get("stages", {}).get("run", {})
    if not st.get("approved") or st["approved"].get("by") in ("", "policy"):
        fail("使用者尚未拍板（save_run.py approve <run> run --by <使用者>）")


def SP1_1() -> None:
    t = read("docs/踩坑手冊.md")
    if "PowerShell" not in t or "http" not in t:
        fail("踩坑手冊沒有 PowerShell／launcher 的查證結論與出處")


def SP1_2() -> None:
    sh_files = tracked("*.sh")
    if sh_files:
        fail(f"仍有 .sh：{sh_files}")
    if (ROOT / "init.sh").exists() or not (ROOT / "init.py").is_file():
        fail("init.sh 未移除或 init.py 不存在")


def SP1_3() -> None:
    for rel in ("plugin/hooks/hooks.json", "scaffold/.claude/settings.project-hooks.json.tmpl"):
        t = read(rel)
        if 'bash \\"' in t or "bash \"" in t or ".sh" in t:
            fail(f"{rel} 仍呼叫 bash／.sh")
        if "PowerShell" not in t:
            fail(f"{rel} 的 matcher 沒有 PowerShell")


def SP1_4() -> None:
    expect(hook("PowerShell", "git push origin feature/x", agent="dev-implementer"), 2, "PowerShell：agent push")
    expect(hook("PowerShell", "git push https://github.com/acme-up/app.git feature/x"), 2, "PowerShell：push 上游")
    expect(hook("PowerShell", "git status; & gh pr merge 3 --repo acme-up/app"), 2, "PowerShell：merge 上游")
    expect(hook("PowerShell", "git status"), 0, "PowerShell：讀取放行")
    expect(hook("Bash", "git push origin feature/x", agent="dev-implementer"), 2, "Bash：agent push")


def SP1_5() -> None:
    t = read(".github/workflows/ci.yml")
    for k in ("windows-latest", "macos-latest", "ubuntu-latest", "run_all.py", "pwsh"):
        if k not in t:
            fail(f"ci.yml 缺 {k}")


def SP2_1() -> None:
    with tempfile.TemporaryDirectory() as d:
        run = Path(d) / "r"
        sp = str(TOOLS / "save_run.py")
        sh(PY, sp, "init", str(run), "--event", "sdlc")
        if sh(PY, sp, "approve", str(run), "plan", "--by", "u", "--at", "t").returncode == 0:
            fail("plan 還沒 done 就能核准")
        env = {"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": [], "payload": {}}
        sh(PY, sp, "stage", str(run), "design", stdin=json.dumps([env]))
        if sh(PY, sp, "approve", str(run), "design", "--by", "u", "--at", "t").returncode == 0:
            fail("plan 未核准就能核准 design")


def SP2_2() -> None:
    expect(hook("Bash", "gh pr merge 3 --repo acme-fork/app"), 2, "主 session 直接 merge（未啟用 auto_merge）")
    expect(hook("Bash", "gh pr review 3 --approve --repo acme-fork/app"), 2, "主 session approve")
    expect(hook("Bash", "gh api -X PUT repos/acme-fork/app/pulls/3/merge"), 2, "gh api merge 繞路")
    expect(hook("Bash", "gh pr create --repo acme-fork/app --title t --body b"), 0, "主 session 開 PR 到 fork 仍放行")


def SP2_3() -> None:
    t = read("plugin/agents/dev-pr-reviewer.md")
    if re.search(r"0[–-]100|≥\s*80\b", t):
        fail("dev-pr-reviewer 仍用 0–100 信心")


def SP4_1() -> None:
    r = sh(PY, str(TOOLS / "merge_gate.py"), "--selftest")
    if r.returncode:
        fail(f"merge_gate selftest 失敗：{(r.stdout + r.stderr)[-400:]}")


def SP4_2() -> None:
    read("scaffold/.claude/teams/schemas/payloads/merge-decision.schema.json")
    for kind in ("valid", "invalid"):
        if not (ROOT / f"scaffold/.claude/teams/golden-set/{kind}/merge-decision.json").is_file():
            fail(f"golden-set/{kind} 缺 merge-decision.json")
    r = sh(PY, str(TOOLS / "build.py"), "--check")
    if r.returncode:
        fail(f"build.py --check：{r.stdout[-300:]}")


def SP5_1() -> None:
    for a in ("dev-merge-steward", "dev-release-watcher"):
        t = read(f"plugin/agents/{a}.md")
        if a == "dev-merge-steward" and re.search(r"^tools:.*\b(Edit|Write)\b", t, re.M):
            fail("dev-merge-steward 不應有 Edit／Write")
    r = sh(PY, str(TOOLS / "check_agents.py"), "--plugin-dir", str(ROOT / "plugin"))
    if r.returncode or "16" not in r.stdout:
        fail(f"check_agents：{r.stdout[-300:]}")


def SP5_2() -> None:
    for w in ("sdlc-merge", "event-post-merge"):
        read(f"scaffold/.claude/workflows/{w}.js")
    if "sdlc-merge" not in read("plugin/skills/tpm-team/SKILL.md"):
        fail("tpm-team 事件表沒有 sdlc-merge")


def SP6_1() -> None:
    am = {"auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low"}, "merger_role": "dev-merge-steward", "daily_cap": 5}}
    expect(hook("Bash", "gh pr merge 3 --repo acme-fork/app", agent="dev-implementer", cfg=am), 2, "非 steward merge")
    expect(hook("Bash", "gh pr merge 3 --repo acme-fork/app", cfg=am), 2, "主 session 繞過 steward")
    expect(hook("Bash", "gh pr review 3 --approve --repo acme-fork/app", agent="dev-merge-steward", cfg=am), 2, "steward approve")
    expect(hook("Bash", "gh pr merge 3 --repo acme-up/app", agent="dev-merge-steward", cfg=am), 2, "上游無授權")
    expect(hook("PowerShell", "gh pr merge 3 --repo acme-fork/app", agent="dev-implementer", cfg=am), 2, "PowerShell 非 steward merge")


def SP6_2() -> None:
    t = read("scaffold/.github/workflows/agent-merge-gate.yml.tmpl")
    if "merge_gate.py" not in t:
        fail("CI 範本沒有跑 merge_gate.py")


def SP6_3() -> None:
    t = read("scaffold/.claude/teams/rules.md")
    if "核准與合併 PR 只能由人做" in t or "auto_merge" not in t:
        fail("rules.md 尚未改成風險分級規則")
    read("docs/自動合併信任模型.md")
    for k in ("auto_merge", "upstream_authorized_by"):
        if k not in read("scaffold/.claude/teams/guard.config.json.tmpl"):
            fail(f"guard.config.json.tmpl 缺 {k}")


def SP7_1() -> None:
    r = sh(PY, "tests/run_all.py")
    if r.returncode:
        fail(f"run_all：{r.stdout[-600:]}")


def SP7_2() -> None:
    t = read("CLAUDE.md")
    if re.search(r"\.sh\b", t) or "dev-merge-steward" not in t:
        fail("CLAUDE.md 仍提到 .sh 或沒有新角色")


def SP7_3() -> None:
    recs = sorted((ROOT / "_tpm/runs").glob("*e2e-auto-merge*/e2e.json"))
    if not recs:
        fail("沒有端到端演練紀錄（需要一個拋棄式 ACME 示範 fork）")
    e = json.loads(recs[-1].read_text(encoding="utf-8"))
    if not (e.get("low_risk") == "merged" and e.get("ci_change") == "escalated" and e.get("revert") == "ok"):
        fail(f"端到端結果不符：{e}")


STEWARD = "agent-workflow:dev-merge-steward"
SHA = "a" * 40


def SP2_4() -> None:
    """政策核准要讀 payload：verifier 有 fail 時 --by policy 不能核准。"""
    with tempfile.TemporaryDirectory() as d:
        run = Path(d) / "r"
        sp = str(TOOLS / "save_run.py")
        sh(PY, sp, "init", str(run), "--event", "sdlc")
        base = {"summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": []}
        for st in ("plan", "design", "build"):
            sh(PY, sp, "stage", str(run), st, stdin=json.dumps([{**base, "status": "done", "payload": {}}]))
            sh(PY, sp, "approve", str(run), st, "--by", "u", "--at", "t")
        def crit(result: str) -> dict:
            return {"commit": SHA, "scope_covered": "全部", "scope_skipped": "none", "anti_cheat": [],
                    "criteria": [{"id": "R1", "criterion": "c", "command": "true", "result": result, "evidence": "e"}]}
        sh(PY, sp, "stage", str(run), "test", stdin=json.dumps([{**base, "status": "done", "payload": crit("fail")}]))
        if sh(PY, sp, "approve", str(run), "test", "--by", "policy", "--note", "p", "--at", "t").returncode == 0:
            fail("verifier 有 fail 仍能 --by policy 核准")
        # 對照組：全部 pass 時政策核准要能過，證明上面是被政策判定擋下而不是信封不合格
        sh(PY, sp, "stage", str(run), "test", stdin=json.dumps([{**base, "status": "done", "payload": crit("pass")}]))
        r = sh(PY, sp, "approve", str(run), "test", "--by", "policy", "--note", "p", "--at", "t")
        if r.returncode:
            fail(f"全部 pass 時政策核准卻被擋：{(r.stdout + r.stderr)[-200:]}")


def SP2_5() -> None:
    t = read("scaffold/.claude/workflows/sdlc-build.js")
    if "halted" not in read("scaffold/.claude/teams/tools/save_run.py"):
        fail("save_run.py 不讀 halted")
    if re.search(r"\.filter\(Boolean\)", t) and "halted" not in t:
        fail("sdlc-build.js 仍會丟掉失敗的 agent 而不標 halted")


def SP4_3() -> None:
    r = sh(PY, str(TOOLS / "merge_gate.py"), "--selftest")
    out = r.stdout + r.stderr
    for k in ("traceability", "mismatch", "command 需求來源"):
        if k not in out:
            fail(f"merge_gate selftest 沒有涵蓋需求對照（輸出缺 {k}）")
    if r.returncode:
        fail(out[-300:])


def SP5_3() -> None:
    t = read("plugin/agents/dev-merge-steward.md")
    for k in ("需求", "對照", "新需求", "request-changes", "退回"):
        if k not in t:
            fail(f"dev-merge-steward 沒寫到「{k}」")
    if re.search(r"^tools:.*\b(Edit|Write)\b", t, re.M):
        fail("dev-merge-steward 不應有 Edit／Write")
    if "followup" not in read("scaffold/.claude/workflows/sdlc-merge.js"):
        fail("sdlc-merge.js 沒有把不一致轉成新需求（followup）")


def SP5_4() -> None:
    r = sh(PY, str(TOOLS / "publish.py"), "--selftest")
    if r.returncode:
        fail(f"publish.py selftest：{(r.stdout + r.stderr)[-300:]}")


def SP6_4() -> None:
    am = {"auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low", "acme-up/app": "low"}, "merger_role": STEWARD, "daily_cap": 5, "upstream_authorized_by": {}}}
    audit = json.dumps({"repo": "acme-fork/app", "pr": 3, "head_sha": SHA, "decision": "merge"}) + "\n"
    files = {"_tpm/runs/merge-audit.jsonl": audit}
    cmd = f"gh pr merge 3 --repo acme-fork/app --squash --match-head-commit {SHA}"
    expect(hook("Bash", cmd, agent=STEWARD, cfg=am, files=files), 0, "steward 有 gate 紀錄、SHA 相符")
    expect(hook("PowerShell", cmd, agent=STEWARD, cfg=am, files=files), 0, "PowerShell：steward 有 gate 紀錄")
    expect(hook("Bash", cmd, agent=STEWARD, cfg=am), 2, "steward 沒有 gate 紀錄")
    expect(hook("Bash", cmd.replace(SHA, "b" * 40), agent=STEWARD, cfg=am, files=files), 2, "head SHA 與 gate 紀錄不符")
    expect(hook("Bash", "gh pr merge 3 --repo acme-fork/app", agent=STEWARD, cfg=am, files=files), 2, "沒有 --match-head-commit")
    expect(hook("Bash", cmd, agent="dev-merge-steward", cfg=am, files=files), 2, "沒有命名空間的同名 agent（冒充）")
    expect(hook("Bash", "gh pr review 3 --request-changes --body x --repo acme-fork/app", agent=STEWARD, cfg=am), 0, "steward 退回 PR")
    up = json.dumps({"repo": "acme-up/app", "pr": 9, "head_sha": SHA, "decision": "merge"}) + "\n"
    ucmd = f"gh pr merge 9 --repo acme-up/app --squash --match-head-commit {SHA}"
    expect(hook("Bash", ucmd, agent=STEWARD, cfg=am, files={"_tpm/runs/merge-audit.jsonl": up}), 2, "上游沒有授權人")
    am["auto_merge"]["upstream_authorized_by"] = {"acme-up/app": "上游維護者（書面授權）"}
    expect(hook("Bash", ucmd, agent=STEWARD, cfg=am, files={"_tpm/runs/merge-audit.jsonl": up}), 0, "上游已授權且有 gate 紀錄")


METH_WORDS = re.compile(r"紅燈|EARS|TDD|DDD|SDD")


def SP8_1() -> None:
    """開發規範 profile：schema、預設 profile（重現現行做法）與至少一個不同的範例 profile。"""
    teams = ROOT / "scaffold/.claude/teams"
    read("scaffold/.claude/teams/schemas/methodology.schema.json")
    profiles = sorted((teams / "methodologies").glob("*.json"))
    if len(profiles) < 2 or not (teams / "methodologies/default.json").is_file():
        fail("scaffold/.claude/teams/methodologies/ 需要 default.json 與至少一個其他 profile")
    for kind in ("valid", "invalid"):
        if not (teams / f"golden-set/{kind}/methodology.json").is_file():
            fail(f"golden-set/{kind} 缺 methodology.json")
    r = sh("uv", "run", "--no-project", "--with", "jsonschema", "python3", str(teams / "golden-set/run.py"))
    if r.returncode:
        fail(f"golden-set：{(r.stdout + r.stderr)[-300:]}")


def SP8_2() -> None:
    """sdlc-*.js 不寫死開發規範，做法由 profile 決定。"""
    hits = []
    for f in sorted((ROOT / "scaffold/.claude/workflows").glob("sdlc-*.js")):
        body = re.sub(r"/\*@schema:.*?/\*@end\*/", "", f.read_text(encoding="utf-8"), flags=re.S)
        if "methodology" not in body:
            hits.append(f"{f.name} 沒有讀 methodology")
        for no, line in enumerate(body.splitlines(), 1):
            if METH_WORDS.search(line):
                hits.append(f"{f.name}:{no} {line.strip()[:60]}")
    if hits:
        fail("sdlc workflow 仍寫死開發規範：" + "；".join(hits[:5]))


def SP8_3() -> None:
    """dev-* agent 照派工單帶來的 profile 工作，不自帶規範。"""
    hits = []
    for f in sorted((ROOT / "plugin/agents").glob("dev-*.md")):
        t = f.read_text(encoding="utf-8")
        for no, line in enumerate(t.splitlines(), 1):
            if METH_WORDS.search(line) and not re.search(r"profile|範例|預設", line):
                hits.append(f"{f.name}:{no}")
    if hits:
        fail("agent 仍把特定規範寫成硬性要求：" + "、".join(hits[:8]))


def SP8_4() -> None:
    """merge_gate 的需求來源與 profile 共用同一份設定。"""
    if "methodolog" not in read("scaffold/.claude/teams/tools/merge_gate.py"):
        fail("merge_gate.py 沒有讀 methodology profile 的需求來源設定")


def SP8_5() -> None:
    if "METHODOLOGY" not in read("workflow.config.example") or "METHODOLOGY" not in read("README.md"):
        fail("workflow.config.example／README 佔位符表缺 METHODOLOGY")
    if "自訂開發規範" not in read("docs/自訂與擴充.md"):
        fail("docs/自訂與擴充.md 缺「自訂開發規範」一節")
    if "methodolog" not in read("tests/smoke_init.py"):
        fail("smoke_init 沒有用非預設 profile 跑一次 init 與 workflow 乾跑")


CONV_WORDS = re.compile(r"Conventional Commits|feat\(<|EARS|300 ?行|≤ ?300|15 ?檔")


def SP8_6() -> None:
    """慣例（code style、commit、分支、PR 大小、文件格式）由使用者定義，模版只留預設值。"""
    sch = read("scaffold/.claude/teams/schemas/methodology.schema.json")
    for k in ("code_style", "commit", "branch", "pr_size", "doc_format"):
        if k not in sch:
            fail(f"methodology schema 缺慣例欄位 {k}")
    hits = []
    files = list((ROOT / "plugin/agents").glob("dev-*.md")) + list((ROOT / "scaffold/.claude/workflows").glob("*.js"))
    files.append(ROOT / "scaffold/.claude/teams/rules.md")
    for f in sorted(files):
        body = re.sub(r"/\*@schema:.*?/\*@end\*/", "", f.read_text(encoding="utf-8"), flags=re.S)
        for no, line in enumerate(body.splitlines(), 1):
            if CONV_WORDS.search(line) and not re.search(r"profile|範例|預設|methodolog", line):
                hits.append(f"{f.name}:{no}")
    if hits:
        fail("仍寫死慣例：" + "、".join(hits[:8]))


def SP8_7() -> None:
    """init 先確認使用者習慣：互動詢問、或用答案檔（非互動、CI 可用），結果寫進專案的 profile。"""
    src = read("init.py")
    for k in ("--answers", "methodolog"):
        if k not in src:
            fail(f"init.py 缺 {k}")
    if "methodolog" not in read("plugin/skills/init-workflow/SKILL.md") or "習慣" not in read("plugin/skills/init-workflow/SKILL.md"):
        fail("init-workflow skill 沒有先詢問使用者習慣再寫 profile")
    if "--answers" not in read("tests/smoke_init.py"):
        fail("smoke_init 沒有用答案檔跑一次 init")


def _hook_raw(mode: str, payload: dict, cfg: dict | None = None) -> int:
    """在暫存專案跑任意 hook 模式（guard-shell、guard-paths、envelope），回傳 exit code。"""
    entry = ROOT / "plugin/hooks/awf_hook.py"
    with tempfile.TemporaryDirectory() as d:
        teams = Path(d) / ".claude/teams"
        teams.mkdir(parents=True)
        for sub in ("schemas", "tools"):
            subprocess.run([PY, "-c", f"import shutil;shutil.copytree(r'{TOOLS.parent / sub}', r'{teams / sub}')"], check=True)
        conf = {"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", **(cfg or {})}
        (teams / "guard.config.json").write_text(json.dumps(conf), encoding="utf-8")
        p = {**payload, "cwd": d}
        if "tool_input" in p and "file_path" in p["tool_input"]:
            p["tool_input"] = {**p["tool_input"], "file_path": str(Path(d) / p["tool_input"]["file_path"])}
        return sh(PY, str(entry), mode, stdin=json.dumps(p), env={"CLAUDE_PROJECT_DIR": d}).returncode


def SP9_1() -> None:
    """使用者自己的 agent（沒有 plugin 命名空間）即使叫 dev-*／tpm-* 也不套模版角色規則；plugin 的角色照舊。"""
    bash = lambda c, a: {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": c}, "agent_type": a}
    write = lambda f, a: {"hook_event_name": "PreToolUse", "tool_name": "Write", "tool_input": {"file_path": f, "content": "x"}, "agent_type": a}
    stop = lambda a: {"hook_event_name": "SubagentStop", "agent_type": a, "last_assistant_message": "完成，沒有信封"}
    expect(_hook_raw("guard-shell", bash("git push origin feature/x", "dev-helper")), 0, "使用者 agent dev-helper push 功能分支")
    expect(_hook_raw("guard-paths", write("src/a.py", "dev-helper")), 0, "使用者 agent dev-helper 寫 src/")
    expect(_hook_raw("envelope", stop("dev-helper")), 0, "使用者 agent dev-helper 交件不需信封")
    expect(_hook_raw("envelope", stop("tpm-notes")), 0, "使用者 agent tpm-notes 交件不需信封")
    expect(_hook_raw("guard-shell", bash("git push origin feature/x", "agent-workflow:dev-implementer")), 2, "plugin agent push 仍擋")
    expect(_hook_raw("guard-paths", write("src/a.py", "agent-workflow:tpm-researcher")), 2, "plugin tpm-researcher 寫 src/ 仍擋")
    expect(_hook_raw("envelope", stop("agent-workflow:dev-verifier")), 2, "plugin agent 交件仍需信封")
    # 想讓沒有命名空間的同名 agent 也套規則（舊行為）時，可以在設定打開
    legacy = {"team_agent_namespaces": ["agent-workflow:", ""]}
    expect(_hook_raw("guard-shell", bash("git push origin feature/x", "dev-helper"), legacy), 2, "設定 team_agent_namespaces 含空字串時套用舊行為")


def SP9_2() -> None:
    """主 session（使用者自己的 skill）的 GitHub／git 限制可以由設定放寬；啟用 auto_merge 時主 session 仍不能合併。"""
    bash = lambda c: {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": c}}
    loose = {"main_session_git_rules": False, "main_session_github_rules": False}
    expect(_hook_raw("guard-shell", bash("gh pr merge 3 --squash"), loose), 0, "放寬後主 session 可以 gh pr merge")
    expect(_hook_raw("guard-shell", bash("git push origin main"), loose), 0, "放寬後主 session 可以 push main")
    expect(_hook_raw("guard-shell", bash("gh pr create --title t --body b"), loose), 0, "放寬後 gh pr create 不必寫 --repo")
    expect(_hook_raw("guard-shell", bash("gh pr merge 3 --squash")), 2, "預設仍擋主 session 合併")
    am = {**loose, "auto_merge": {"enabled": True, "repos": {"acme-fork/app": "low"}}}
    expect(_hook_raw("guard-shell", bash("gh pr merge 3 --squash --repo acme-fork/app"), am), 2, "啟用 auto_merge 時主 session 一律不能合併")
    for k in ("main_session_github_rules",):
        if k not in read("scaffold/.claude/teams/guard.config.json.tmpl"):
            fail(f"guard.config.json.tmpl 缺 {k}")


def SP9_3() -> None:
    """套用到既有專案：原檔逐位元保留、CLAUDE.md 只加一行引用（重跑不重複）、project hooks 合併進既有 settings、預設不放 merge gate CI。"""
    with tempfile.TemporaryDirectory() as d:
        proj = Path(d) / "p"
        (proj / ".claude/agents").mkdir(parents=True)
        (proj / ".claude/skills/my-skill").mkdir(parents=True)
        (proj / ".github/workflows").mkdir(parents=True)
        files = {
            ".claude/agents/dev-helper.md": "---\nname: dev-helper\ndescription: 使用者的 agent\n---\nx\n",
            ".claude/skills/my-skill/SKILL.md": "---\nname: my-skill\ndescription: 使用者的 skill\n---\nx\n",
            ".github/workflows/ci.yml": "name: mine\n",
            "CLAUDE.md": "# 我的專案\n\n原本的內容。\n",
        }
        settings = {"permissions": {"allow": ["Bash(npm test)"]}, "env": {"MY_VAR": "1"},
                    "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo user-hook"}]}]}}
        for rel, body in files.items():
            (proj / rel).write_text(body, encoding="utf-8")
        (proj / ".claude/settings.json").write_text(json.dumps(settings), encoding="utf-8")
        for _ in range(2):  # 重跑一次，確認不重複
            r = sh(PY, str(ROOT / "init.py"), "--target", str(proj), "--defaults", "--hooks", "project")
            if r.returncode:
                fail(f"init 失敗：{(r.stdout + r.stderr)[-300:]}")
        for rel, body in files.items():
            got = (proj / rel).read_text(encoding="utf-8")
            if rel == "CLAUDE.md":
                if not got.startswith(body):
                    fail("CLAUDE.md 原內容被改動")
                extra = got[len(body):]
                if extra.count("agent-workflow") < 1 or len([l for l in extra.splitlines() if l.strip()]) > 3:
                    fail(f"CLAUDE.md 應只追加一段很短的引用，實際追加：{extra!r}")
            elif got != body:
                fail(f"{rel} 被改動")
        s = json.loads((proj / ".claude/settings.json").read_text(encoding="utf-8"))
        if s.get("permissions", {}).get("allow") != ["Bash(npm test)"] or s.get("env", {}).get("MY_VAR") != "1":
            fail("settings.json 的使用者 permissions／env 沒保留")
        cmds = json.dumps(s.get("hooks", {}), ensure_ascii=False)
        if "echo user-hook" not in cmds or cmds.count("awf_hook.py") < 2:
            fail("settings.json 沒有同時保留使用者 hook 與加入 awf_hook.py")
        if cmds.count("guard-shell") != 1:
            fail(f"重跑 init 後 guard-shell 註冊了 {cmds.count('guard-shell')} 次")
        if (proj / ".github/workflows/agent-merge-gate.yml").exists():
            fail("預設不應放 agent-merge-gate.yml（沒有 GitHub App 時會讓每個 PR 失敗）")
        r = sh(PY, str(ROOT / "init.py"), "--target", str(proj), "--defaults", "--with-merge-gate")
        if not (proj / ".github/workflows/agent-merge-gate.yml").exists():
            fail("--with-merge-gate 時要放 agent-merge-gate.yml")


def SP9_4() -> None:
    if "既有專案" not in read("tests/smoke_init.py"):
        fail("smoke_init 沒有「套用到既有專案」的相容性案例")
    if "既有專案" not in read("docs/自訂與擴充.md") and "既有專案" not in read("README.md"):
        fail("文件沒有說明套用到既有專案時，使用者的 agent／skill／settings 怎麼保留")


if __name__ == "__main__":
    fn = globals().get(sys.argv[1].replace(".", "_")) if len(sys.argv) > 1 else None
    if not callable(fn):
        print(__doc__)
        sys.exit(2)
    fn()
    print("通過")
