"""基準實跑：在虛構專案上跑 sdlc-plan，量 token、時間、平均 context，以及重跑的比例。

**這支腳本會真的呼叫 claude，一次要花數百萬 token**，所以不在 run_all 裡，只在發佈前手動跑。

步驟（--step，可分開跑；預設依序全部）：
  setup   產生虛構專案（make_fixture.py）→ init.py（--defaults --hooks project，讓這版的 hook 生效）→ 建知識庫 → 建 run 目錄
  plan    備資料包（pack.py）→ `claude -p` 用 Workflow 跑 .claude/workflows/sdlc-plan.js → 讀 workflow 的回傳存成 plan 段 → 量成本
  rerun   同一個 run 再跑一次 plan（來源沒變的調研角度會沿用）→ 量成本
  report  彙整成 <work>/bench-result.json 並印出對照表

用法：
  python3 tests/bench/run_plan.py --work <空目錄> [--model opus] [--step setup|plan|rerun|report] [--timeout 分鐘]
  python3 tests/bench/run_plan.py --selftest      只測不花 token 的部分（setup、備資料包、組 args、彙整）

主對話的模型（--model）決定 strong 等級用什麼（內建的 strong 是「沿用主對話模型」）；和 0.3.0 的實測比較時用同一個模型。
agent 一律用這個 repo 的 plugin（--plugin-dir），不是使用者已安裝的版本。
claude 以 -p 執行並只允許 Workflow、Read、Write、Edit、Bash、Grep、Glob、Skill；工作目錄是 <work>/ws（工作區），目標 repo 在它底下的 repos/acme-orders。

驗收門檻（計畫 A9.2）：首次 plan 不超過 5M token、10 分鐘、平均 context 不超過 40k；重跑不超過首次的 30%。
只用標準庫。
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
PY = sys.executable
RUN_REL = "_tpm/runs/20260101-sdlc-export-range"   # 日期固定：workflow 與工具都不取時間，派工單才會逐次相同
EPIC = "export-range"
REPO_REL = "repos/acme-orders"
LIMITS = {"total_tokens": 5_000_000, "minutes": 10, "avg_context": 40_000, "rerun_ratio": 0.30}
ALLOWED = ["Workflow", "Read", "Write", "Edit", "Bash", "Grep", "Glob", "Skill"]

sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "scaffold" / ".claude" / "teams" / "tools"))
import awf_cost  # noqa: E402
import make_fixture  # noqa: E402


def sh(cmd: list[str], cwd: Path, timeout: int | None = None, check: bool = True) -> subprocess.CompletedProcess:
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
    if check and r.returncode != 0:
        raise SystemExit(f"指令失敗（exit {r.returncode}）：{' '.join(cmd[:4])}…\n{(r.stdout + r.stderr)[-2000:]}")
    return r


def tool(proj: Path, name: str) -> str:
    return str(proj / ".claude" / "teams" / "tools" / name)


def setup(work: Path) -> Path:
    """工作區（init 在這裡）底下放目標 repo：<work>/ws/repos/acme-orders。這是模版預設的配置（guard.config.json 的 repos_dir）。"""
    proj = work / "ws"
    repo = proj / REPO_REL
    repo.mkdir(parents=True, exist_ok=True)
    make_fixture.write(repo)
    make_fixture.git_init(repo)
    sh([PY, str(REPO / "init.py"), "--target", str(proj), "--defaults", "--hooks", "project"], REPO)
    req = proj / "_tpm" / "requirement.md"
    req.parent.mkdir(parents=True, exist_ok=True)
    req.write_bytes((HERE / "requirement.md").read_bytes())
    sh([PY, tool(proj, "save_run.py"), "init", RUN_REL, "--event", "sdlc", "--target", REPO_REL, "--requirement", "_tpm/requirement.md"], proj)
    return proj


def plan_args(proj: Path) -> dict:
    """備資料包並組出 sdlc-plan 的 args。"""
    sh([PY, tool(proj, "pack.py"), "plan", "--run", RUN_REL], proj)
    extra = json.loads((proj / RUN_REL / "plan" / "pack-args.json").read_text(encoding="utf-8"))
    return {"repo": REPO_REL, "epic": EPIC, "requirement": "_tpm/requirement.md", "base": "main", "run_dir": RUN_REL, **extra}


def prompt_for(proj: Path, args: dict) -> str:
    script = proj / ".claude" / "workflows" / "sdlc-plan.js"
    return ("用 Workflow 工具執行一支 workflow：scriptPath 是 " + str(script) + "，args 是下面這個 JSON 物件（原樣傳物件，不要轉成字串、不要增刪欄位）：\n"
            + json.dumps(args, ensure_ascii=False) + "\n等 workflow 完成的通知到了之後，只回覆一行「done」。不要自己做 workflow 裡的工作，也不要讀或整理它的回傳。")


def run_claude(proj: Path, args: dict, model: str, timeout_min: int) -> str:
    sid = str(uuid.uuid4())
    cmd = ["claude", "-p", prompt_for(proj, args), "--model", model, "--session-id", sid, "--plugin-dir", str(REPO / "plugin"),
           "--output-format", "json", "--allowedTools", *ALLOWED]
    r = sh(cmd, proj, timeout=timeout_min * 60, check=False)
    (proj / "_tpm" / f"claude-{sid}.log").write_text(r.stdout + "\n--- stderr ---\n" + r.stderr, encoding="utf-8")
    if r.returncode != 0:
        raise SystemExit(f"claude 結束碼 {r.returncode}；輸出在 {proj / '_tpm' / f'claude-{sid}.log'}")
    return sid


def session_dir(proj: Path, sid: str) -> Path:
    return awf_cost.project_dir(str(proj)) / sid


def wf_result(proj: Path, sid: str) -> tuple[dict, dict]:
    """讀 workflow 的回傳（Claude Code 存在 <session>/workflows/wf_<id>.json 的 result）。回傳（中繼資料, result）。"""
    metas = sorted((session_dir(proj, sid) / "workflows").glob("wf_*.json"))
    if not metas:
        raise SystemExit(f"找不到 workflow 的紀錄：{session_dir(proj, sid) / 'workflows'}（Workflow 沒有被呼叫，或這版 Claude Code 的存放位置變了）")
    meta = json.loads(metas[-1].read_text(encoding="utf-8"))
    res = meta.get("result")
    if isinstance(res, str):
        try:
            res = json.loads(res)
        except ValueError:
            pass
    if not isinstance(res, dict):
        raise SystemExit(f"workflow 沒有回傳物件（status={meta.get('status')}）：{metas[-1]}")
    return meta, res


def cost(proj: Path, sid: str) -> dict:
    rows = [awf_cost.summarize(d) for d in awf_cost.find_wf_dirs([session_dir(proj, sid)])]
    rows = [r for r in rows if r]
    if not rows:
        raise SystemExit("讀不到 workflow 的 transcript（這版格式讀不了）")
    return rows[-1]


def measure(proj: Path, model: str, timeout_min: int, name: str) -> dict:
    args = plan_args(proj)
    sid = run_claude(proj, args, model, timeout_min)
    meta, res = wf_result(proj, sid)
    out = proj / RUN_REL / f"{name}-result.json"
    out.write_text(json.dumps(res, ensure_ascii=False, indent=1), encoding="utf-8")
    with out.open("rb") as f:
        saved = subprocess.run([PY, tool(proj, "save_run.py"), "stage", RUN_REL, "plan"], cwd=proj, stdin=f, capture_output=True,
                               text=True, encoding="utf-8", errors="replace")
    c = cost(proj, sid)
    row = summary_row(c, res, args)
    row.update({"session": sid, "wf_status": meta.get("status"), "save_run_exit": saved.returncode,
                "save_run_tail": (saved.stdout + saved.stderr).strip().splitlines()[-6:]})
    (proj / RUN_REL / f"{name}-cost.json").write_text(json.dumps({"summary": row, "cost": c}, ensure_ascii=False, indent=1), encoding="utf-8")
    return row


def summary_row(c: dict, res: dict, args: dict) -> dict:
    return {"total_tokens": c["total_tokens"], "minutes": c["minutes"], "avg_context": c["avg_context"], "agents": c["agents"],
            "requests": c["requests"], "tokens": c["tokens"],
            "by_role": [{"name": r["name"], "agents": r["agents"], "tokens": r["tokens"]} for r in c["by_role"]],
            "by_model": [{"name": r["name"], "agents": r["agents"], "tokens": r["tokens"]} for r in c["by_model"]],
            "halted": bool(res.get("halted")), "halt_reason": res.get("halt_reason") or "", "missing": res.get("missing") or [],
            "reused": sorted(res.get("reused") or []) if isinstance(res.get("reused"), (list, dict)) else [],
            "reuse_offered": sorted((args.get("reuse") or {}).keys()), "recipe": (res.get("recipe") or {}).get("name")}


def verdict(first: dict | None, rerun: dict | None) -> dict:
    """對照驗收門檻。沒跑的項目是 None（不算通過）。"""
    v: dict = {}
    if first:
        v["total_tokens"] = first["total_tokens"] <= LIMITS["total_tokens"]
        v["minutes"] = first["minutes"] is not None and first["minutes"] <= LIMITS["minutes"]
        v["avg_context"] = first["avg_context"] <= LIMITS["avg_context"]
        v["completed"] = not first["halted"] and first.get("save_run_exit") == 0
    if first and rerun:
        v["rerun_ratio"] = rerun["total_tokens"] <= first["total_tokens"] * LIMITS["rerun_ratio"]
        v["rerun_completed"] = not rerun["halted"] and rerun.get("save_run_exit") == 0
    return v


def report(proj: Path, work: Path, model: str) -> dict:
    def load(name: str):
        p = proj / RUN_REL / f"{name}-cost.json"
        return json.loads(p.read_text(encoding="utf-8"))["summary"] if p.is_file() else None

    first, rerun = load("first"), load("rerun")
    v = verdict(first, rerun)
    out = {"model": model, "limits": LIMITS, "first": first, "rerun": rerun,
           "rerun_ratio": round(rerun["total_tokens"] / first["total_tokens"], 3) if first and rerun and first["total_tokens"] else None,
           "verdict": v, "pass": bool(v) and all(v.get(k) for k in ("total_tokens", "minutes", "avg_context", "completed", "rerun_ratio", "rerun_completed"))}
    (work / "bench-result.json").write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
    for name, row in (("首次", first), ("重跑", rerun)):
        if row:
            print(f"{name}：{row['total_tokens'] / 1e6:.2f}M token、{row['minutes']} 分、平均 context {row['avg_context'] / 1000:.1f}k、"
                  f"{row['agents']} 個 agent{'、halted：' + row['halt_reason'] if row['halted'] else ''}{'、沿用 ' + '、'.join(row['reused']) if row['reused'] else ''}")
    if out["rerun_ratio"] is not None:
        print(f"重跑／首次：{out['rerun_ratio'] * 100:.0f}%")
    print("門檻：" + "、".join(f"{k}={'過' if ok else '沒過'}" for k, ok in v.items()) + f" → {'達標' if out['pass'] else '未達標'}")
    return out


def selftest() -> int:
    ok = True

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        ok = ok and bool(cond)
        print(("通過  " if cond else "失敗  ") + name + (f"：{detail}" if detail and not cond else ""))

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        proj = setup(work)
        check("setup：專案已 init、有 run.json 與知識庫", (proj / RUN_REL / "run.json").is_file() and (proj / ".claude/workflows/sdlc-plan.js").is_file()
              and (proj / ".claude/hooks/read_budget.py").is_file() and (proj / REPO_REL / ".git").is_dir())
        a1 = plan_args(proj)
        pack1 = (proj / RUN_REL / "plan" / "pack.md").read_bytes()
        a2 = plan_args(proj)
        check("args 有必填欄位與資料包", all(a1.get(k) for k in ("repo", "epic", "requirement", "base", "run_dir", "pack")), str(a1))
        check("資料包可重現（兩次內容相同）", pack1 == (proj / RUN_REL / "plan" / "pack.md").read_bytes() and a1 == a2)
        check("資料包有知識庫命中目標 repo 的行段", REPO_REL + "/docs/" in pack1.decode("utf-8"), pack1.decode("utf-8")[:400])
        p = prompt_for(proj, a1)
        check("提示帶 scriptPath 與 args", "sdlc-plan.js" in p and RUN_REL in p)
        good = {"total_tokens": 4_000_000, "minutes": 8.0, "avg_context": 30_000, "halted": False, "save_run_exit": 0}
        check("verdict：達標", all(verdict(good, dict(good, total_tokens=1_000_000)).values()))
        check("verdict：重跑超過 30% 沒過", verdict(good, dict(good, total_tokens=1_300_000))["rerun_ratio"] is False)
        check("verdict：超過 5M 沒過", verdict(dict(good, total_tokens=5_000_001), None)["total_tokens"] is False)
        check("verdict：halted 不算完成", verdict(dict(good, halted=True), None)["completed"] is False)
        check("report：沒跑過時不算達標", report(proj, work, "x")["pass"] is False)
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    work_raw = awf_cost.opt(argv, "--work")
    if not work_raw:
        print(__doc__)
        return 2
    work = Path(work_raw).resolve()
    model = awf_cost.opt(argv, "--model", "opus")
    step = awf_cost.opt(argv, "--step", "all")
    timeout_min = int(awf_cost.opt(argv, "--timeout", "45"))
    proj = work / "ws"
    if step in ("all", "setup"):
        proj = setup(work)
        print(f"工作區：{proj}（目標 repo：{REPO_REL}）")
    if step in ("all", "plan"):
        print(json.dumps(measure(proj, model, timeout_min, "first"), ensure_ascii=False, indent=1))
    if step in ("all", "rerun"):
        print(json.dumps(measure(proj, model, timeout_min, "rerun"), ensure_ascii=False, indent=1))
    if step in ("all", "report"):
        return 0 if report(proj, work, model)["pass"] else 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
