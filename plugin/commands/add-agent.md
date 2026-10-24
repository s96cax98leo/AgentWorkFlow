---
description: 把自己的 agent 加進 SDLC workflow（plan、design、test、deliver 任一段），設定守門角色並驗證
argument-hint: <agent 名稱或想做的事（可省略）>
---

使用者想把自己的 agent 加進 workflow：

$ARGUMENTS

用 Skill 工具載入 `add-agent`（以 plugin 安裝時名稱是 `agent-workflow:add-agent`），照它的步驟執行：先確認專案已 init，再問要加在哪一段、套用哪個模版角色的守門規則、派工內容，必要時建立 agent 定義檔，寫進 `methodology.json` 的 `extensions` 與 `guard.config.json` 的 `extension_agents`，最後跑 `build.py` 與驗證。改檔前先給使用者看，使用者同意才寫。所有回覆用專案設定的輸出語言（預設繁體中文）。
