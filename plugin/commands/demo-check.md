---
description: Demo 前檢查：照腳本逐步實測目標環境，列出會出包的地方與備援
argument-hint: <目標（環境、分支或網址）；可附 demo 腳本路徑與日期>
---

使用者要做 demo 前檢查：

$ARGUMENTS

用 Skill 工具載入 `tpm-team`（以 plugin 安裝時名稱是 `agent-workflow:tpm-team`），把這件事當成「Demo 前檢查」事件交給它處理。

交給 skill 前先整理好輸入：
- 目標：demo 用的 repo 路徑與分支，或網址。
- demo 腳本路徑與 demo 日期（使用者有給才帶）。
- 使用者特別擔心的步驟或環境限制，原話保留。
- 目標不明或缺的資訊，用選項式提問問清楚再交出去，不要猜。

流程、環境操作的注意事項、閘門與回報方式一律照 `tpm-team` skill 與 workflow，這支指令不另外規定。所有回覆用專案設定的輸出語言（預設繁體中文）。
