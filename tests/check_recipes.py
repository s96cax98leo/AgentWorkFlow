#!/usr/bin/env python3
"""recipe 解析的一致性檢查：
  1. tools/recipe.py --selftest 通過。
  2. snippets/recipe.js（workflow 用）和 recipe.py 對 golden-set/recipes/*.json 的每個案例解析結果完全相同（用 node 跑）。
  3. 兩份內建 profile（methodologies/*.json）的 recipes／modifiers 和 recipe.py 的 BUILTIN_RECIPES／BUILTIN_MODIFIERS 一致
     （lightweight 的 feature 可以有自己的調研角度，其餘相同）。
只用標準庫；沒有 node 時第 2 項判失敗（不略過）。run_all.py 會自動發現並執行。
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
TOOLS = TEAMS / "tools"
sys.path.insert(0, str(TOOLS))
import recipe  # noqa: E402

KEYS = ["name", "stages", "plan_mode", "research_angles", "design_lenses", "test", "deliver_axes", "build_kinds",
        "publishable", "followup", "split", "modifiers", "errors"]

JS = r"""
const fs = require('fs')
const src = fs.readFileSync(process.argv[1], 'utf8')
const cases = JSON.parse(fs.readFileSync(0, 'utf8'))
const [resolveRecipe, resolveModel] = new Function(src + '\nreturn [resolveRecipe, resolveModel]')()
const out = cases.map((c) => {
  const r = resolveRecipe(c.profile, c.recipe, c.modifiers, c.size)
  const o = Object.fromEntries(KEYS.map((k) => [k, r[k]]))
  if (c.model_cases) o.models = c.model_cases.map((m) => { const x = resolveModel(c.profile, m.role, m.stage, m.suggested, r.modifiers); delete x.reason; return x })
  return o
})
process.stdout.write(JSON.stringify(out))
""".replace("KEYS", json.dumps(KEYS))


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    errs: list[str] = []
    p = subprocess.run([sys.executable, str(TOOLS / "recipe.py"), "--selftest"], capture_output=True, text=True, encoding="utf-8")
    if p.returncode != 0:
        errs.append("recipe.py --selftest 失敗：\n" + p.stdout[-1500:])
    cases = recipe.golden_cases()
    node = shutil.which("node")
    if not node:
        errs.append("找不到 node，無法比對 recipe.js（fail-closed）")
    else:
        payload = [{"profile": recipe.case_profile(c), "recipe": c.get("recipe"), "modifiers": c.get("modifiers"), "size": c.get("size"),
                    **({"model_cases": c["model_cases"]} if "model_cases" in c else {})} for _, c in cases]
        q = subprocess.run([node, "-e", JS, str(TOOLS / "snippets" / "recipe.js")], input=json.dumps(payload, ensure_ascii=False),
                           capture_output=True, text=True, encoding="utf-8")
        if q.returncode != 0:
            errs.append(f"node 跑 recipe.js 失敗：{q.stderr[-800:]}")
        else:
            js = json.loads(q.stdout)
            for (path, c), got in zip(cases, js):
                py = recipe.run_case(c)
                diff = {k: {"py": py[k], "js": got.get(k)} for k in [*KEYS, *(["models"] if "models" in py else [])] if py[k] != got.get(k)}
                if diff:
                    errs.append(f"{path.name}：recipe.js 和 recipe.py 不一致 {json.dumps(diff, ensure_ascii=False)[:500]}")
    for prof in sorted((TEAMS / "methodologies").glob("*.json")):
        d = json.loads(prof.read_text(encoding="utf-8"))
        rec, mods = d.get("recipes") or {}, d.get("modifiers") or {}
        if set(rec) != set(recipe.BUILTIN_RECIPES):
            errs.append(f"{prof.name}：recipes 應有 {sorted(recipe.BUILTIN_RECIPES)}，實際 {sorted(rec)}")
        if mods != recipe.BUILTIN_MODIFIERS:
            errs.append(f"{prof.name}：modifiers 和 recipe.BUILTIN_MODIFIERS 不一致")
        for n, r in rec.items():
            want = recipe.BUILTIN_RECIPES.get(n)
            if want is None:
                continue
            a = {k: v for k, v in r.items() if not (n == "feature" and k in ("research_angles", "small"))}
            b = {k: v for k, v in want.items() if not (n == "feature" and k in ("research_angles", "small"))}
            if a != b:
                errs.append(f"{prof.name}：recipe {n} 和 recipe.BUILTIN_RECIPES 不一致（只有 feature 的調研角度可以不同）")
    if errs:
        print("recipe 一致性檢查失敗：")
        print("\n".join(f"  - {e}" for e in errs))
        return 1
    print(f"recipe 一致性檢查通過：golden {len(cases)} 個案例 py＝js，內建 profile 與 recipe.py 一致")
    return 0


if __name__ == "__main__":
    sys.exit(main())
