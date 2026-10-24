#!/usr/bin/env python3
"""去客戶化掃描：用 tests/scrub_terms.txt（公開的通用規則）與 tests/scrub_terms.local.txt（本機的真實名稱，不進版控；存在才讀）
掃描檔案內容與路徑名，兩份都是一行一個 ERE 正規式。

用法：
  python3 tests/scrub_check.py                       # 掃整個 AgentWorkFlow
  python3 tests/scrub_check.py README.md docs tests  # 只掃指定的檔案或目錄（相對 repo 根目錄）
任何命中就以 1 退出，並列出「檔案:行號:內容」。排除 .git、__pycache__、node_modules、根目錄的本機目錄（_tpm、graft、.claude）、兩份掃描清單本身與二進位檔。
清單裡的 ERE 只用到 Python re 也支援的語法（沒有 [[:space:]] 這類 POSIX 字元類）。
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TERMS = ROOT / "tests" / "scrub_terms.txt"
LOCAL_TERMS = ROOT / "tests" / "scrub_terms.local.txt"
SKIP_DIRS = {".git", "__pycache__", "node_modules"}
# repo 根目錄下只存在本機、不進版控的目錄（run 紀錄、graft 快取、本機 settings 與 agent worktree）
LOCAL_ROOT_DIRS = {"_tpm", "graft", ".claude"}


def load_patterns() -> list[re.Pattern]:
    if not TERMS.is_file():
        print(f"找不到掃描清單：{TERMS}", file=sys.stderr)
        sys.exit(2)
    pats = []
    for f in (TERMS, LOCAL_TERMS):
        if not f.is_file():
            continue
        for line in f.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            pats.append(re.compile(line))
    return pats


def walk(target: Path):
    """依序產生 (顯示用相對路徑, 實際路徑, 是否為檔案)；目錄本身也產生，用來掃路徑名。"""
    rel = lambda p: "./" + p.relative_to(ROOT).as_posix() if p != ROOT else "."  # noqa: E731
    if target.is_file():
        yield rel(target), target, True
        return
    yield rel(target), target, False
    for root, dirs, files in os.walk(target):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not (Path(root) == ROOT and d in LOCAL_ROOT_DIRS))
        r = Path(root)
        for d in dirs:
            yield rel(r / d), r / d, False
        for f in sorted(files):
            if f in SKIP_DIRS:  # worktree 裡的 .git 是檔案
                continue
            yield rel(r / f), r / f, True


def is_binary(data: bytes) -> bool:
    return b"\0" in data[:8192]


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    pats = load_patterns()
    targets = [ROOT / a for a in argv] if argv else [ROOT]
    content_hits, path_hits = [], []
    for t in targets:
        if not t.exists():
            continue
        for shown, p, is_file in walk(t):
            if any(pat.search(shown) for pat in pats):
                path_hits.append(shown)
            if not is_file or p.name in (TERMS.name, LOCAL_TERMS.name):
                continue
            try:
                data = p.read_bytes()
            except OSError:
                continue
            if is_binary(data):
                continue
            text = data.decode("utf-8", errors="replace")
            for i, line in enumerate(text.splitlines(), 1):
                if any(pat.search(line) for pat in pats):
                    content_hits.append(f"{shown}:{i}:{line}")
    if content_hits:
        print("== 內容命中 ==")
        print("\n".join(content_hits))
    if path_hits:
        print("== 路徑名命中 ==")
        print("\n".join(path_hits))
    hits = len(content_hits) + len(path_hits)
    if hits:
        print(f"去客戶化掃描：不合格，共 {hits} 處命中", file=sys.stderr)
        return 1
    local = "含本機清單" if LOCAL_TERMS.is_file() else "沒有本機清單，只用公開規則"
    print(f"去客戶化掃描：通過（掃描範圍：{' '.join(argv) if argv else '.'}；{local}）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
