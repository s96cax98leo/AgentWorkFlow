---
name: status
description: 看一個 run 現在跑到哪：目前的段、等哪個閘門、各段派了幾個 agent 與缺件、哪些 agent 還在跑。使用者說「現在進度」「跑到哪了」「還在等什麼」「卡住了嗎」「/agent-workflow:status」時使用。
argument-hint: <run 目錄（可省略＝最近動過的 run）>
---

# 看進度（status）

只讀，不影響流程。Claude Code 自己也有 `/status`，這支要用完整名稱 `/agent-workflow:status` 叫用。

使用者指定的 run：$ARGUMENTS（沒給就用 `guard.config.json` 的 `runs_dir` 底下最近動過的那個；不只一個在進行時列出來讓使用者選）。

```bash
python3 .claude/teams/tools/awf.py watch --run <run_dir> --once        # 給人看的快照
python3 .claude/teams/tools/awf.py next --run <run_dir> --text         # 下一步要做什麼
```

回報時講四件事：

1. 現在在哪一段、狀態是什麼（在跑、等核准、停下了、做完了）。
2. 等人的事：等哪個閘門、有哪些未決問題。
3. 缺件：哪些 agent 沒交件。缺件不等於沒有發現，要講明。
4. 還在跑的 agent 與它們的 token。快照出現「沒有動靜」時提醒使用者：workflow 可能停住了（例如帳號額度用完），要停掉再跑 `awf.py next`。

換 session 接手時改用 `awf.py handoff --run <run_dir>`（寫 `接手摘要.md`）。想讓外部程式持續收到進度，見模版的 `docs/監控與整合.md`。
