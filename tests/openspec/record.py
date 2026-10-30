#!/usr/bin/env python3
"""用真的 OpenSpec CLI（1.15.x）把每個 case 的輸出錄成 replay.json，給 fake_openspec.py 回放。

用法：python3 tests/openspec/record.py [case 名稱…]     （要先裝 openspec，例如 npm i -g @fission-ai/openspec@1.15）
每個 case 目錄：
  change/   要放進 openspec/changes/<CHANGE>/ 的檔案（沒有這個目錄＝只跑 `openspec new change`，只有 .openspec.yaml）
  specs/    （選用）放進 openspec/specs/ 的現行規格
  case.json （選用）{"change": "<change id>", "commands": [[…argv…], …]}；預設 change=add-x、跑 COMMANDS
在暫存目錄 `openspec init --tools none` 後執行；輸出裡的暫存路徑換成 <root>，不留本機路徑。
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES = HERE / "cases"
ENV = {**os.environ, "NO_COLOR": "1", "OPENSPEC_NO_ANIMATION": "1", "OPENSPEC_TELEMETRY": "0"}


def commands(cid: str) -> list[list[str]]:
    return [
        ["--version"],
        ["status", "--change", cid, "--json"],
        ["validate", cid, "--strict", "--json"],
        ["instructions", "apply", "--change", cid, "--json"],
        ["show", cid, "--json", "--deltas-only", "--type", "change"],
    ]


def record(case: Path, exe: str) -> None:
    meta = json.loads((case / "case.json").read_text(encoding="utf-8")) if (case / "case.json").is_file() else {}
    cid = meta.get("change", "add-x")
    with tempfile.TemporaryDirectory() as d:
        root = Path(d).resolve()
        subprocess.run([exe, "init", "--tools", "none"], cwd=root, env=ENV, capture_output=True, check=True)
        if meta.get("create", True):
            subprocess.run([exe, "new", "change", cid], cwd=root, env=ENV, capture_output=True, check=True)
        if (case / "change").is_dir():
            shutil.copytree(case / "change", root / "openspec" / "changes" / cid, dirs_exist_ok=True)
        if (case / "specs").is_dir():
            shutil.copytree(case / "specs", root / "openspec" / "specs", dirs_exist_ok=True)
        out = {}
        for argv in meta.get("commands") or commands(cid):
            r = subprocess.run([exe, *argv], cwd=root, env=ENV, capture_output=True, text=True, encoding="utf-8")
            clean = lambda s: s.replace(str(root), "<root>").replace("/private<root>", "<root>")  # noqa: E731
            out[" ".join(argv)] = {"stdout": clean(r.stdout), "stderr": clean(r.stderr), "exit": r.returncode}
        (case / "replay.json").write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"{case.name}：錄了 {len(out)} 個指令")


def main() -> int:
    exe = shutil.which("openspec")
    if not exe:
        print("找不到 openspec（要用真 CLI 錄）", file=sys.stderr)
        return 2
    names = sys.argv[1:] or sorted(p.name for p in CASES.iterdir() if p.is_dir())
    for n in names:
        record(CASES / n, exe)
    return 0


if __name__ == "__main__":
    sys.exit(main())
