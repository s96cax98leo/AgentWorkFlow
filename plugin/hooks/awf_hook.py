"""agent-workflow hooks 的單一入口（取代舊的 bash 外殼，macOS、Linux、Windows 都直接用 Python 跑）。

用法：python3 awf_hook.py <guard-shell|guard-paths|envelope> < hook-input.json

  guard-shell  PreToolUse（matcher: Bash|PowerShell）：擋 agent push、agent 的 GitHub 寫入、寫入上游、動禁止路徑、
               不合規的分支名；PowerShell 指令先解析成段落再套同一套規則（guard.py），之後轉呼叫目標 repo 自己的 guard-git。
  guard-paths  PreToolUse（matcher: Edit|Write|NotebookEdit）：擋寫禁止路徑、各角色寫入範圍以外的檔案（guard.py）。
  envelope     PreToolUse（SubagentHandback、StructuredOutput）、SubagentStop、TaskCompleted：檢查交件信封（envelope_hook.py）。

結束碼：0 放行；2 擋下（stderr 回給 Claude）；其他＝hook 本身出錯。
守門類 hook 在 hooks.json／settings.json 設了 "onFailure": "block"，所以 hook 跑不起來（找不到 Python、
腳本壞掉、輸入不是 JSON）時會擋下而不是悄悄放行；交件檢查設 "continue"，出錯不擋（擋了 agent 會無法交件）。
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
MODES = ("guard-shell", "guard-paths", "envelope")


def _utf8() -> None:
    """Windows 的 Python 預設用 cp950 等地區編碼；hook 的輸入輸出與子行程一律 UTF-8。"""
    os.environ.setdefault("PYTHONUTF8", "1")
    os.environ.setdefault("PYTHONIOENCODING", "utf-8")
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass


def read_input() -> dict:
    raw = sys.stdin.buffer.read().decode("utf-8-sig", errors="replace")
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError("hook 輸入不是 JSON 物件")
    return data


def main(argv: list[str]) -> int:
    _utf8()
    if len(argv) != 1 or argv[0] not in MODES:
        print(f"用法：awf_hook.py <{'|'.join(MODES)}> < hook-input.json", file=sys.stderr)
        return 1
    mode = argv[0]
    try:
        data = read_input()
    except (ValueError, UnicodeDecodeError) as e:
        print(f"agent-workflow hook：讀不到 hook 輸入（{e}）", file=sys.stderr)
        return 1
    if mode == "envelope":
        import envelope_hook

        envelope_hook.run(data)
    else:
        import guard

        (guard.check_shell if mode == "guard-shell" else guard.check_paths)(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
