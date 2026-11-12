---
name: cost
description: 看一個 run 花了多少 token 與時間：依段、角色、模型加總，列出最花的 agent 與平均 context。使用者說「花了多少 token」「哪一段最貴」「為什麼這麼慢」「成本」「/agent-workflow:cost」時使用。
argument-hint: <run 目錄（可省略＝最近動過的 run）>
---

# 看成本（cost）

只讀。Claude Code 自己也有 `/cost`（整個 session 的花費），這支看的是某一個 run 的 workflow，要用完整名稱 `/agent-workflow:cost` 叫用。

使用者指定的 run：$ARGUMENTS（沒給就用最近動過的那個）。

```bash
python3 .claude/teams/tools/awf_cost.py --project . --run-dir <run_dir>
```

回報時：

1. 先講總數：總 token、其中 cache 讀取佔多少、時間。
2. 再講最花的三個 agent（角色、模型、token、平均 context）。平均 context 很大（十萬以上）通常是整檔讀大檔，可以建議開讀取上限（`methodology.json` 的 `limits`，預設不啟用，由使用者決定）。
3. 數字是從 Claude Code 的 workflow 執行紀錄估的，不是帳單。工具說「這版格式讀不了」時照實轉述，不要自己估。

這是輔助功能：讀不到資料不影響流程，閘門也不看這些數字。
