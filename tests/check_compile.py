#!/usr/bin/env python3
"""所有會在使用者機器上執行的 Python 檔，都要能用「現在這個 Python」編譯。

hook 的檔案有語法錯誤時，守門會退回最小規則（或整個失效）；工具有語法錯誤時閘門跑不起來。
CI 在支援範圍的兩端（3.9 與最新版）各跑一次 run_all.py，所以用了舊版不支援的語法會在這裡被抓到。

用法：python3 tests/check_compile.py
只編譯、不執行、不寫 .pyc。只用標準庫。
"""
from __future__ import annotations

import sys
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIRS = ("plugin", "scaffold", "tests")
FILES = ("init.py",)


def main() -> int:
    files = [ROOT / f for f in FILES] + [p for d in DIRS for p in sorted((ROOT / d).rglob("*.py")) if "__pycache__" not in p.parts]
    bad = []
    hooks = 0
    with warnings.catch_warnings():
        warnings.simplefilter("error", SyntaxWarning)     # 例如字串裡寫錯的跳脫字元，之後的版本會變成錯誤
        for f in files:
            hooks += "hooks" in f.relative_to(ROOT).parts
            try:
                compile(f.read_text(encoding="utf-8"), str(f), "exec")
            except (SyntaxError, SyntaxWarning, ValueError, OSError) as e:
                bad.append(f"{f.relative_to(ROOT).as_posix()}：{type(e).__name__} {e}")
    for b in bad:
        print(f"[失敗] {b}")
    if hooks < 5:
        print(f"[失敗] 只找到 {hooks} 個 hook 檔，檢查範圍可能壞了")
        return 1
    v = ".".join(str(x) for x in sys.version_info[:3])
    print(f"編譯檢查{'有失敗' if bad else '通過'}：{len(files)} 個檔（其中 hook {hooks} 個），Python {v}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
