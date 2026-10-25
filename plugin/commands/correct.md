---
description: 把一則教訓（踩坑手冊、followup、合併後歸因、使用者描述的錯誤）變成強制機制，並證明擋得住
argument-hint: <錯誤描述、踩坑手冊條目、followup 路徑或 commit／PR（可省略）>
---

使用者想把一個錯誤變成做不到：

$ARGUMENTS

用 Skill 工具載入 `correct`（以 plugin 安裝時名稱是 `agent-workflow:correct`），照它的步驟執行：先找到這個錯誤真實發生過的例子（找不到就請使用者提供），選最高可行的強制層級，列計畫給使用者確認，再實作並用舊錯誤與修正版各跑一次證明，最後更新 `.claude/teams/rules.md`〈專案規則〉的對照表。改檔前先給使用者看，使用者同意才寫。所有回覆用專案設定的輸出語言（預設繁體中文）。
