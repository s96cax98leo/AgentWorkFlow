"""可驗證的 checklist：每一項都帶驗證指令，打勾與否以指令實際結果為準，不以人或 agent 的宣稱為準。

格式（Markdown，一行一項；沒有驗證指令的項目一律算未完成）：
    - [ ] SP1.3 初始化腳本改成跨平台的 init.py — 驗證：`python3 init.py --help`
    - [x] SP3.1 checklist 工具 — 驗證：`python3 scaffold/.claude/teams/tools/checklist.py --selftest`

用法：
    python3 checklist.py status <檔>                     # 列出未完成的項目
    python3 checklist.py check <檔> [--id <前綴>] [--write] [--cwd <目錄>] [--timeout <秒>]
        實際執行驗證指令。--write 時依結果改寫勾選框（通過打勾、失敗取消）。
        有任何一項（或 --id 篩出的項目）失敗就 exit 1。
    python3 checklist.py --selftest
SDLC 的 progress.md 也可以用同樣格式，由主 session 在閘門前跑 check。
"""

from __future__ import annotations

import re
import subprocess
import sys
import tempfile
from pathlib import Path

ITEM = re.compile(r"^(?P<pre>\s*[-*] \[)(?P<box>[ xX])(?P<mid>\] )(?P<id>\S+)\s+(?P<rest>.*)$")
VERIFY = re.compile(r"驗證[:：]\s*`(?P<cmd>[^`]+)`")


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv else default


def parse(text: str) -> list[dict]:
    items = []
    for no, line in enumerate(text.splitlines()):
        m = ITEM.match(line)
        if not m:
            continue
        v = VERIFY.search(m["rest"])
        items.append({"line": no, "id": m["id"], "done": m["box"] in "xX", "cmd": v["cmd"] if v else "", "desc": m["rest"]})
    return items


def run(cmd: str, cwd: Path, timeout: int) -> tuple[bool, str]:
    try:
        p = subprocess.run(cmd, shell=True, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        return False, f"逾時（>{timeout} 秒）"
    tail = (p.stdout + p.stderr).strip().splitlines()[-3:]
    return p.returncode == 0, f"exit {p.returncode}" + (f"｜{' / '.join(tail)}" if tail else "")


def check(path: Path, argv: list[str]) -> int:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    cwd = Path(opt(argv, "--cwd", ".")).resolve()
    timeout = int(opt(argv, "--timeout", "600"))
    prefix = opt(argv, "--id")
    failed = 0
    for it in parse(text):
        if prefix and not it["id"].startswith(prefix):
            continue
        if not it["cmd"]:
            ok, why = False, "缺驗證指令"
        else:
            ok, why = run(it["cmd"], cwd, timeout)
        failed += not ok
        print(f"{'通過' if ok else '失敗'}  {it['id']}  {why}")
        if "--write" in argv:
            m = ITEM.match(lines[it["line"]])
            lines[it["line"]] = f"{m['pre']}{'x' if ok else ' '}{m['mid']}{m['id']} {m['rest']}"
    if "--write" in argv:
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"共 {failed} 項失敗" if failed else "全部通過")
    return 1 if failed else 0


def status(path: Path) -> int:
    items = parse(path.read_text(encoding="utf-8"))
    todo = [it for it in items if not it["done"]]
    for it in todo:
        print(f"未完成  {it['id']}  {it['desc']}")
    print(f"{len(items) - len(todo)}/{len(items)} 完成")
    return 0


def selftest() -> int:
    py = f'"{sys.executable}"'
    with tempfile.TemporaryDirectory() as d:
        f = Path(d) / "c.md"
        f.write_text(
            f"# 測試\n- [ ] A1 會過 — 驗證：`{py} -c \"pass\"`\n"
            f"- [x] A2 宣稱完成但會失敗 — 驗證：`{py} -c \"raise SystemExit(3)\"`\n"
            "- [x] A3 沒有驗證指令\n",
            encoding="utf-8",
        )
        assert check(f, ["--write", "--cwd", d]) == 1
        got = {it["id"]: it["done"] for it in parse(f.read_text(encoding="utf-8"))}
        assert got == {"A1": True, "A2": False, "A3": False}, got
        assert check(f, ["--id", "A1", "--cwd", d]) == 0
    print("selftest 通過")
    return 0


def main(argv: list[str]) -> int:
    if argv[:1] == ["--selftest"]:
        return selftest()
    if len(argv) < 2 or argv[0] not in ("status", "check"):
        print(__doc__)
        return 2
    path = Path(argv[1])
    return status(path) if argv[0] == "status" else check(path, argv[2:])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
