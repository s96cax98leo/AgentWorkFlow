#!/usr/bin/env python3
"""產生 plugin/workflow-templates/legacy.json：舊版 init 複製進專案的 workflow 副本長什麼樣。

0.5 起 workflow 不再複製進專案（由 awf.py 依 plugin 的範本逐段產生）。init.py --upgrade 要能分辨專案裡的
.claude/workflows/*.js 是「舊版原樣的副本」（可以移除）還是「使用者改過的」（保留）。做法：把各版的範本依佔位符
與開發規範 profile 的內嵌區切成固定片段，只記每段的長度與雜湊；升級時拿專案的檔逐段比對，佔位符的位置要剛好是
專案 workflow.config 記的值。全部對得上才算原樣。

只在發佈新版、而且那一版曾經把 workflow 複製進專案時才需要重跑（0.5 之後不再複製，所以這份清單不會再變長）：
    python3 tests/gen_legacy_templates.py [--check]
版本與 commit 寫在下面的 RELEASES。只用標準庫與 git。
"""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "plugin" / "workflow-templates" / "legacy.json"
RELEASES = [("0.3.0", "v0.3.0"), ("0.4.0", "7ab673c"), ("0.4.1", "d81cb5c")]
OLD_DIR = "scaffold/.claude/workflows"
HOLE = re.compile(r"\{\{([A-Z_]+)\}\}|(/\*@methodology\*/.*?/\*@end\*/)", re.S)


def git(*a: str) -> str:
    return subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=True).stdout


def split(text: str) -> dict:
    keys, segs, pos = [], [], 0
    for m in HOLE.finditer(text):
        seg = text[pos:m.start()]
        segs.append([len(seg), hashlib.sha1(seg.encode("utf-8")).hexdigest()[:16]])
        keys.append(m.group(1) or "@METH")
        pos = m.end()
    seg = text[pos:]
    segs.append([len(seg), hashlib.sha1(seg.encode("utf-8")).hexdigest()[:16]])
    return {"keys": keys, "segs": segs}


def build() -> dict:
    out: dict = {}
    for ver, rev in RELEASES:
        for name in git("ls-tree", "--name-only", rev, OLD_DIR + "/").split():
            if not name.endswith(".js"):
                continue
            entry = split(git("show", f"{rev}:{name}").replace("\r\n", "\n"))
            vs = out.setdefault(Path(name).name, [])
            same = next((v for v in vs if v["keys"] == entry["keys"] and v["segs"] == entry["segs"]), None)
            if same:
                same["versions"].append(ver)
            else:
                vs.append({"versions": [ver], **entry})
    return {"_說明": "舊版 init 複製進專案的 workflow 副本的指紋（init.py --upgrade 用來分辨原樣的副本與使用者改過的檔）。由 tests/gen_legacy_templates.py 產生，不要手改。",
            "files": out}


def main() -> int:
    text = json.dumps(build(), ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n"
    if "--check" in sys.argv:
        ok = OUT.is_file() and OUT.read_text(encoding="utf-8") == text
        print("legacy.json：" + ("一致" if ok else "和 git 歷史算出來的不一致（重跑 tests/gen_legacy_templates.py）"))
        return 0 if ok else 1
    OUT.write_text(text, encoding="utf-8")
    d = json.loads(text)["files"]
    print(f"已寫 {OUT.relative_to(ROOT)}：{len(d)} 支 workflow、{sum(len(v) for v in d.values())} 個版本的指紋，{len(text) // 1024} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
