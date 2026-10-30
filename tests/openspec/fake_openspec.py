#!/usr/bin/env python3
"""假的 openspec CLI：回放 record.py 用真 CLI（1.15.0）錄下的輸出，讓測試不必安裝 OpenSpec。

用法：AWF_OPENSPEC='["python3", "tests/openspec/fake_openspec.py"]' FAKE_OPENSPEC_CASE=<case 目錄> <要測的工具>
- 讀 <case>/replay.json：{"<argv 以空白連接>": {"stdout": …, "stderr": …, "exit": N}}。
- 找不到對應的 argv 時 exit 99，stderr 說明（測試會因此失敗，不會默默通過）。
- 環境變數 FAKE_OPENSPEC_VERSION 會蓋過 --version 的輸出；FAKE_OPENSPEC_SLEEP 秒數讓每次呼叫先睡（測逾時）。
只用標準庫。
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass
    if os.environ.get("FAKE_OPENSPEC_SLEEP"):
        time.sleep(float(os.environ["FAKE_OPENSPEC_SLEEP"]))
    key = " ".join(sys.argv[1:])
    if key == "--version" and os.environ.get("FAKE_OPENSPEC_VERSION"):
        print(os.environ["FAKE_OPENSPEC_VERSION"])
        return 0
    case = os.environ.get("FAKE_OPENSPEC_CASE")
    if not case:
        print("fake_openspec：沒有設 FAKE_OPENSPEC_CASE", file=sys.stderr)
        return 99
    replay = json.loads((Path(case) / "replay.json").read_text(encoding="utf-8"))
    if key not in replay:
        print(f"fake_openspec：{Path(case).name} 沒有錄到 `openspec {key}`", file=sys.stderr)
        return 99
    r = replay[key]
    sys.stdout.write(r.get("stdout", ""))
    sys.stderr.write(r.get("stderr", ""))
    return int(r.get("exit", 0))


if __name__ == "__main__":
    sys.exit(main())
