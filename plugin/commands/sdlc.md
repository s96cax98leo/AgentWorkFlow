---
description: 啟動需求開發的 AI-native SDLC（plan → design → build → sync → test → deliver → merge，段與段之間由人核准；merge 由可信任的合併 agent 對照需求後執行）
argument-hint: <需求描述或需求檔路徑；可附 repo、epic、base>
---

使用者要開發這個需求：

$ARGUMENTS

用 Skill 工具載入 `tpm-team`（以 plugin 安裝時名稱是 `agent-workflow:tpm-team`），把這件事當成「需求開發（SDLC）」事件交給它處理。

交給 skill 前先整理好輸入：
- 需求：使用者的描述原話保留；給的是檔案路徑就照用。
- 使用者有提到的 repo、epic、base、是否為 hotfix、期限，逐一列出。
- 缺漏或模糊的地方，用選項式提問問清楚再交出去，不要猜。

各段流程、閘門、核准與合併規則一律照 `tpm-team` skill，這支指令不另外規定。所有回覆用專案設定的輸出語言（預設繁體中文）。
