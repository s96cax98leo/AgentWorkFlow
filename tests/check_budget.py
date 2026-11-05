#!/usr/bin/env python3
"""token 預算檢查：派工數與派工單字數不能默默變多。

做法：在暫存目錄用預設 profile init 一個專案，乾跑全部 workflow（check_workflows.mjs 的 AWF_SCENARIO_REPORT），
算每支 workflow 預設情境的 agent 數與派工單總字數，另算每個內建 recipe 從 plan 到 deliver 的總派工數，
和 scaffold/.claude/teams/golden-set/token-budget.json 的基準比：超過基準 10% 就失敗。

限制：乾跑量的是「派出去的字」與「派幾個 agent」，量不到 agent 實際讀檔與輸出的長度（那要看真實執行的用量）。
派工單開頭的固定前綴（共同規則）在同一次 run 裡每張都一樣，可以被 prompt cache 命中，這裡另外列出長度。

用法：
    python3 tests/check_budget.py            # 比對基準
    python3 tests/check_budget.py --update   # 改了流程、確認變多是預期的之後，寫回基準
只用標準庫；需要 node。run_all.py 會自動發現並執行。
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
BUDGET = TEAMS / "golden-set" / "token-budget.json"
TOLERANCE = 0.10
STAGES = ["plan", "design", "build", "test", "deliver"]
sys.path.insert(0, str(TEAMS / "tools"))
import recipe  # noqa: E402


def measure() -> dict:
    env = {**os.environ, "PYTHONUTF8": "1"}
    with tempfile.TemporaryDirectory(prefix="awf-budget.") as tmp:
        proj = Path(tmp) / "proj"
        proj.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=proj, check=False)
        p = subprocess.run([sys.executable, str(ROOT / "init.py"), "--target", str(proj), "--defaults"], cwd=tmp, env=env,
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
        if p.returncode != 0:
            raise SystemExit(f"init.py 失敗：{(p.stdout + p.stderr)[-1500:]}")
        rep = Path(tmp) / "report.json"
        p = subprocess.run(["node", str(proj / ".claude/teams/tools/check_workflows.mjs"), "--agents-dir", str(ROOT / "plugin/agents")],
                           cwd=proj, env={**env, "AWF_SCENARIO_REPORT": str(rep)}, capture_output=True, text=True, encoding="utf-8", errors="replace")
        if not rep.is_file():
            raise SystemExit(f"check_workflows 沒有產出報告：{(p.stdout + p.stderr)[-1500:]}")
        data = json.loads(rep.read_text(encoding="utf-8"))
    out: dict = {"workflows": {}, "recipes": {}}
    for wf, v in sorted(data.items()):
        sc = v["scenarios"][0]
        ps = list(sc["prompts"].values())
        prefix = len(os.path.commonprefix(ps)) if len(ps) > 1 else 0
        out["workflows"][wf] = {"agents": len(sc["labels"]), "prompt_chars": sum(len(x) for x in ps), "shared_prefix_chars": prefix}
    bi = {"recipes": recipe.BUILTIN_RECIPES, "modifiers": recipe.BUILTIN_MODIFIERS}
    for name in recipe.BUILTIN_RECIPES:
        total = 0
        for st in recipe.resolve(bi, name)["stages"]:
            if st not in STAGES:
                continue
            sc = next((s for s in data.get(f"sdlc-{st}.js", {}).get("scenarios", []) if s["name"] == f"recipe:{name}"), None)
            if sc is None:
                total = None
                break
            total += len(sc["labels"])
        out["recipes"][name] = {"agents_plan_to_deliver": total}
    return out


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    if not shutil.which("node"):
        print("找不到 node，無法量測（fail-closed）")
        return 1
    cur = measure()
    if "--update" in sys.argv:
        BUDGET.write_text(json.dumps(cur, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"已寫回基準 {BUDGET.relative_to(ROOT)}")
        return 0
    if not BUDGET.is_file():
        print(f"沒有基準 {BUDGET.relative_to(ROOT)}；確認數字後用 --update 建立")
        return 1
    base = json.loads(BUDGET.read_text(encoding="utf-8"))
    errs = []
    for wf, b in base.get("workflows", {}).items():
        c = cur["workflows"].get(wf)
        if c is None:
            errs.append(f"{wf}：量不到（workflow 不見了？）")
            continue
        for k in ("agents", "prompt_chars"):
            if c[k] > b[k] * (1 + TOLERANCE):
                errs.append(f"{wf}：{k} {c[k]}，超過基準 {b[k]} 的 {int(TOLERANCE * 100)}%")
    for name, b in base.get("recipes", {}).items():
        c = cur["recipes"].get(name, {}).get("agents_plan_to_deliver")
        want = b.get("agents_plan_to_deliver")
        if want is not None and (c is None or c > want * (1 + TOLERANCE)):
            errs.append(f"recipe {name}：plan→deliver 派 {c} 個 agent，基準 {want}")
    feat = cur["recipes"].get("feature", {}).get("agents_plan_to_deliver")
    print("各 recipe plan→deliver 派工數：" + "、".join(f"{n} {v['agents_plan_to_deliver']}" for n, v in cur["recipes"].items()))
    if feat:
        print(f"（feature＝{feat}；乾跑量的是派工數與派工單字數，量不到 agent 實際讀寫的 token）")
    if errs:
        print("token 預算超標：")
        print("\n".join(f"  - {e}" for e in errs))
        print("確認是預期的變多後，用 python3 tests/check_budget.py --update 更新基準")
        return 1
    print("token 預算檢查通過")
    return 0


if __name__ == "__main__":
    sys.exit(main())
