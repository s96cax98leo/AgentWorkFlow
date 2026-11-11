"""agent-workflow hooks 的單一入口（macOS、Linux、Windows 都直接用 Python 跑）。

用法：python3 awf_hook.py <guard-shell|guard-paths|envelope> < hook-input.json

  guard-shell  PreToolUse（matcher: Bash|PowerShell）：擋 agent push、agent 的 GitHub 寫入、寫入上游、動禁止路徑、
               不合規的分支名；PowerShell 指令先解析成段落再套同一套規則（guard.py），之後轉呼叫目標 repo 自己的 guard-git。
  guard-paths  PreToolUse（matcher: Edit|Write|NotebookEdit|Read|Grep）：擋寫禁止路徑、各角色寫入範圍以外的檔案（guard.py）。
               Read 另套讀取預算（read_budget.py，設定在 methodology.json 的 limits）：模版角色讀大檔要分段、每個 agent 有讀取額度；
               guard-shell 也擋模版角色用 cat／Get-Content 整檔印出大檔。
  envelope     PreToolUse（SubagentHandback、StructuredOutput）、SubagentStop、TaskCompleted：檢查交件信封（envelope_hook.py）。

結束碼：0 放行；2 擋下（stderr 回給 Claude）。

失敗語意（plugin 以 user scope 安裝時會套在使用者所有專案，所以「hook 壞掉就鎖死所有工具」不能接受）：
  1. hook 跑不起來（找不到 Python、Microsoft Store 的 python3 空殼、Windows 只有 PowerShell 時 sh 語法錯誤、逾時）：
     hooks.json 設 "onFailure": "continue"，放行。要更嚴格就用 project 模式在專案 settings 寫 "onFailure": "block"
     與 Python 的絕對路徑；真正的信任邊界在伺服器端（ruleset、required check）。
  2. hook 跑得起來，但輸入壞掉（空、不是 JSON、缺 tool_name／tool_input、shell 指令超過 MAX_SHELL_COMMAND）：
     守門類一律擋（exit 2），交件類放行（exit 0）。
  3. guard.py 或 awf_config.py 載入失敗（語法錯誤、缺檔、import 錯誤）：改用本檔內建的最小規則（_fallback_shell），
     只擋 push／send-pack、gh pr merge、--approve、gh api 寫入、curl 等打 merge／reviews；其他放行，guard-paths 一律放行。
     stderr 印一行提示。內建規則刻意不依賴其他模組。
  4. 守門模組執行中出現意外錯誤：守門類擋（exit 2），交件類放行（exit 0）。任何情況都不印 Python traceback。
  5. 讀取預算（read_budget.py）是輔助功能，不是守門：模組載入失敗、設定讀不懂、狀態檔壞掉、任何例外都放行，
     不影響前面幾條既有檢查的結果（機敏檔照樣先擋）。
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MODES = ("guard-shell", "guard-paths", "envelope")
GUARD_MODES = ("guard-shell", "guard-paths")
# guard.py 的規則對指令長度大約是平方成長（512KB 約 2 秒），超長指令會拖到 timeout，而 timeout＝onFailure continue＝放行。
# 為了不讓「把指令灌長」變成繞過守門的方法，超過上限的 shell 指令直接擋。正常的 Bash／PowerShell 指令遠小於此。
MAX_SHELL_COMMAND = 128 * 1024


def _utf8() -> None:
    """Windows 的 Python 預設用 cp950 等地區編碼；hook 的輸入輸出與子行程一律 UTF-8。"""
    os.environ.setdefault("PYTHONUTF8", "1")
    os.environ.setdefault("PYTHONIOENCODING", "utf-8")
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass


def _say(msg: str) -> None:
    try:
        print(msg, file=sys.stderr)
    except Exception:  # stderr 壞掉也不能讓 hook 崩潰
        pass


class BadInput(ValueError):
    pass


def read_input(mode: str) -> dict:
    raw = sys.stdin.buffer.read().decode("utf-8-sig", errors="replace")
    if not raw.strip():
        raise BadInput("輸入是空的")
    try:
        data = json.loads(raw)
    except ValueError as e:
        raise BadInput(f"不是合法 JSON：{e.__class__.__name__}") from None
    if not isinstance(data, dict):
        raise BadInput("不是 JSON 物件")
    if mode in GUARD_MODES:
        if not isinstance(data.get("tool_name"), str) or not isinstance(data.get("tool_input"), dict):
            raise BadInput("缺 tool_name 或 tool_input")
        cmd = data["tool_input"].get("command")
        if mode == "guard-shell" and isinstance(cmd, str) and len(cmd) > MAX_SHELL_COMMAND:
            raise BadInput(f"指令長度 {len(cmd)} 字元，超過守門可在時限內檢查的上限 {MAX_SHELL_COMMAND}；請拆成較短的指令或改寫成腳本檔")
    return data


# ───────── 內建最小規則：guard.py 載入失敗時用，刻意只用標準庫、不 import 本目錄任何模組 ─────────

_FB_SEG = re.compile(r"[;&|\n]+")
_FB_GIT = re.compile(r"(?:^|[\s/\\(])git(?:\.exe)?(?=\s|$)", re.I)
_FB_PUSH = re.compile(r"(?:^|\s)(?:push|send-pack)(?=\s|$)|\bgit-send-pack\b", re.I)
_FB_MERGE = re.compile(r"\bgh(?:\.exe)?\s+pr\s+merge\b", re.I)
_FB_APPROVE = re.compile(r"(?:^|\s)--approve\b|\bgh(?:\.exe)?\s+pr\s+review\b.*\s-[A-Za-z]*a[A-Za-z]*(?=\s|$)|\baddPullRequestReview\b", re.I)
_FB_GH_API = re.compile(r"\bgh(?:\.exe)?\s+api\b", re.I)
_FB_API_WRITE = re.compile(r"(?:-X\s*|--method[\s=]+)[\"']?(?:POST|PUT|PATCH|DELETE)\b|\s(?:-f|-F|--field|--raw-field|--input)[\s=]", re.I)
_FB_WEB = re.compile(r"(?:^|[\s/\\(])(?:curl|wget|invoke-restmethod|irm|invoke-webrequest|iwr)(?:\.exe)?(?=\s|$)", re.I)
_FB_PR_WRITE = re.compile(r"/merges?(?![\w-])|/reviews\b|mergePullRequest|enablePullRequestAutoMerge|mergeBranch", re.I)


def _fallback_shell(data: dict) -> str | None:
    """回傳擋下的理由；None＝放行。"""
    cmd = str((data.get("tool_input") or {}).get("command") or "")
    for seg in _FB_SEG.split(cmd):
        if (_FB_GIT.search(seg) and _FB_PUSH.search(seg)) or "git-send-pack" in seg:
            return "push／send-pack"
    if _FB_MERGE.search(cmd):
        return "gh pr merge"
    if _FB_APPROVE.search(cmd):
        return "核准 PR（--approve）"
    if _FB_GH_API.search(cmd) and _FB_API_WRITE.search(cmd):
        return "gh api 寫入"
    if _FB_WEB.search(cmd) and _FB_PR_WRITE.search(cmd):
        return "用 HTTP 客戶端合併或審查 PR"
    return None


def _fallback(mode: str, data: dict, err: BaseException) -> int:
    _say(f"agent-workflow hook：守門模組載入失敗（{err.__class__.__name__}），改用最小規則；請更新或重裝 agent-workflow plugin。")
    if mode != "guard-shell":
        return 0
    why = _fallback_shell(data)
    if why:
        _say(f"agent-workflow 守門（最小規則）：擋下 {why}。守門模組恢復前，這類操作請由人在終端機自己執行。")
        return 2
    return 0


# ───────── 主流程 ─────────


def _exit_code(e: SystemExit) -> int:
    c = e.code
    if c is None:
        return 0
    if isinstance(c, int):
        return c
    _say(str(c))
    return 1


def _dispatch(mode: str) -> int:
    try:
        data = read_input(mode)
    except BadInput as e:
        if mode in GUARD_MODES:
            _say(f"agent-workflow 守門：hook 輸入有問題（{e}），為了安全先擋下。")
            return 2
        _say(f"agent-workflow 交件檢查：hook 輸入有問題（{e}），略過檢查。")
        return 0

    if str(HERE) not in sys.path:
        sys.path.insert(0, str(HERE))
    if mode == "envelope":
        try:
            import envelope_hook
        except Exception as e:  # noqa: BLE001  交件檢查壞掉不能讓 agent 交不了件
            _say(f"agent-workflow 交件檢查：模組載入失敗（{e.__class__.__name__}），略過檢查。")
            return 0
        envelope_hook.run(data)
        return 0

    try:
        import guard
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001  SyntaxError、ImportError、缺檔、設定讀取錯誤都走最小規則
        return _fallback(mode, data, e)
    (guard.check_shell if mode == "guard-shell" else guard.check_paths)(data)
    return 0


def main(argv: list[str]) -> int:
    _utf8()
    if len(argv) != 1 or argv[0] not in MODES:
        _say(f"用法：awf_hook.py <{'|'.join(MODES)}> < hook-input.json")
        return 1
    mode = argv[0]
    try:
        return _dispatch(mode)
    except SystemExit as e:  # guard.block()、envelope_hook.fail() 用 sys.exit(2)
        return _exit_code(e)
    except BaseException as e:  # noqa: BLE001  最外層：不印 traceback
        if mode in GUARD_MODES:
            _say(f"agent-workflow 守門：檢查時發生意外錯誤（{e.__class__.__name__}：{str(e)[:200]}），為了安全先擋下。")
            return 2
        _say(f"agent-workflow 交件檢查：發生意外錯誤（{e.__class__.__name__}），略過檢查。")
        return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
