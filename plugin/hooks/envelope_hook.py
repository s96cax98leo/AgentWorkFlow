"""agent-workflow 的交件信封檢查（由 awf_hook.py envelope 呼叫）。只檢查 team 角色（plugin 命名空間下的 tpm-*／dev-*，見 awf_config.is_team_agent）與 team 任務。

schema 與驗證邏輯讀專案的 ${CLAUDE_PROJECT_DIR}/.claude/teams/（schemas/ 與 tools/schemalib.py，由 init.py 放進專案）。
專案還沒 init（找不到 schemalib.py）時不檢查、直接放行。

掛在四個事件：
  - PreToolUse SubagentHandback：子 agent 用 SubagentHandback 交件時，回報在 tool_input.message。這是主要攔截點
  - PreToolUse StructuredOutput：Workflow 裡帶 schema 的 agent 交件時，用這個檢查 schema 管不到的語意規則
  - SubagentStop：子 agent 以文字結束時；stop_hook_active 為 true（已擋過一次）就放行，避免無限迴圈
  - TaskCompleted（Agent Teams）：輸入只有 task_name／task_description，所以 team 任務的描述要寫
    `output: <runs_dir>/<run>/<檔>.json`，這裡讀那個檔驗證；描述裡沒有 runs_dir 的任務不檢查
exit 2 擋下（stderr 回給 agent）。
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import awf_config  # noqa: E402

CFG = awf_config.load()
WORKSPACE = awf_config.workspace()
TOOLS = WORKSPACE / ".claude" / "teams" / "tools"


def load_schemalib():
    if not (TOOLS / "schemalib.py").exists():
        return None
    sys.path.insert(0, str(TOOLS))
    import schemalib  # noqa: E402

    return schemalib


def fail(errs: list[str], where: str) -> None:
    print(f"交件信封不合格（{where}，格式見 .claude/teams/schemas/envelope.schema.json）：", file=sys.stderr)
    for e in errs:
        print(f"- {e}", file=sys.stderr)
    sys.exit(2)


def run(data: dict) -> None:
    schemalib = load_schemalib()
    if schemalib is None:
        return
    event = data.get("hook_event_name", "")
    team = awf_config.is_team_agent(data.get("agent_type") or "", CFG)  # 完整 agent_type；使用者自己的 agent 不檢查信封

    if event == "PreToolUse":
        tool = data.get("tool_name", "")
        ti = data.get("tool_input") or {}
        if tool == "StructuredOutput":
            env = ti
            if isinstance(env, dict) and "status" in env and "payload" in env:
                errs = schemalib.semantic_errors(env)
                if errs:
                    fail(errs, "StructuredOutput")
            return
        if not team:
            return
        env = schemalib.extract_json(ti.get("message", ""))
        if env is None:
            fail(["回報最後要附一個 ```json 區塊（交件信封）"], "SubagentHandback")
        errs = schemalib.check(env)
        if errs:
            fail(errs, "SubagentHandback")
        return

    if event == "SubagentStop":
        if data.get("stop_hook_active") or not team:
            return
        msg = data.get("last_assistant_message")
        if not msg:  # 用 SubagentHandback 交件時沒有這個欄位；已在 PreToolUse 檢查過
            return
        env = schemalib.extract_json(msg)
        errs = ["回報最後要附一個 ```json 區塊（交件信封）"] if env is None else schemalib.check(env)
        if errs:
            fail(errs, "SubagentStop")
        return

    if event == "TaskCompleted":
        runs = CFG["runs_dir"] + "/"
        desc = f"{data.get('task_name', '')}\n{data.get('task_description', '')}"
        if runs not in desc:
            return
        m = re.search(r"output:\s*(\S+\.json)", desc)
        if not m:
            fail([f"team 任務的描述要寫 `output: {runs}<run>/<檔>.json`"], "TaskCompleted")
        out = Path(m.group(1))
        if not out.is_absolute():
            out = Path(data.get("cwd") or WORKSPACE) / out
        if not out.exists():
            fail([f"找不到產出檔 {out}"], "TaskCompleted")
        try:
            env = json.loads(out.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            fail([f"{out} 不是合法 JSON：{e}"], "TaskCompleted")
        errs = schemalib.check(env)
        if errs:
            fail(errs, f"TaskCompleted {out.name}")


def main() -> None:
    run(json.load(sys.stdin))


if __name__ == "__main__":
    main()
    sys.exit(0)
