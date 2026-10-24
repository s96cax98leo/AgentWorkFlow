#!/usr/bin/env python3
"""串起所有測試：去客戶化掃描 → init 煙霧測試（含暫存專案裡的 golden-set、build.py --check、
check_workflows.mjs、check_agents.py、--hooks project）→ plugin hook 測試 → 設計系統 token 掃描器。

用法：python3 tests/run_all.py   （Windows：py -3 tests/run_all.py 或 python tests/run_all.py）
任何一項失敗，最後以 1 退出；每一項都會跑完，方便一次看到所有問題。只用標準庫。
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = sys.executable


def check_tokens_smoke(env: dict) -> subprocess.CompletedProcess:
    """check-tokens.py 的煙霧測試：寫死的色碼要被抓（exit 1），用 var() 的要放行（exit 0）。"""
    script = ROOT / "plugin" / "skills" / "design-system-template" / "scripts" / "check-tokens.py"
    with tempfile.TemporaryDirectory() as d:
        hard, ok = Path(d) / "hard.css", Path(d) / "ok.css"
        hard.write_text("a { color: #123456; }\n", encoding="utf-8")
        ok.write_text("a { color: var(--fg); }\n", encoding="utf-8")
        cases = [(hard, 1, "寫死色碼要被抓"), (ok, 0, "var() 要放行")]
        code = 0
        for f, want, what in cases:
            r = subprocess.run([PY, str(script), str(f)], cwd=ROOT, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace")
            mark = "通過" if r.returncode == want else "失敗"
            print(f"  [{mark}] {what}（want={want} got={r.returncode}）")
            if r.returncode != want:
                code = 1
                print(r.stdout + r.stderr)
    return subprocess.CompletedProcess([], code)


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    env = {**os.environ, "PYTHONUTF8": "1"}
    steps = [
        ("去客戶化掃描", [PY, str(ROOT / "tests" / "scrub_check.py")]),
        ("init 煙霧測試", [PY, str(ROOT / "tests" / "smoke_init.py")]),
        ("hook 測試", [PY, str(ROOT / "plugin" / "hooks" / "tests" / "run.py")]),
        ("check-tokens 掃描器", None),
    ]
    results, status = [], 0
    for name, argv in steps:
        print(f"\n################ {name} ################", flush=True)
        r = subprocess.run(argv, cwd=ROOT, env=env) if argv else check_tokens_smoke(env)
        if r.returncode == 0:
            results.append(f"通過  {name}")
        else:
            results.append(f"失敗  {name}（exit {r.returncode}）")
            status = 1
    print("\n================ 總結 ================")
    print("\n".join(results))
    return status


if __name__ == "__main__":
    sys.exit(main())
