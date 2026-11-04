---
description: 判斷事件類型（上游變動、會議、PR 審查、合併後監看、需求變動、決策落地、demo 檢查、方案辯論）並跑對應的 workflow
argument-hint: <事件描述，例如「上游 main 前進了」「審 feature/x」「決策者決定採方案 B」>
---

使用者回報了一個事件：

$ARGUMENTS

用 Skill 工具載入 `tpm-team`（以 plugin 安裝時名稱是 `agent-workflow:tpm-team`），把事件描述交給它判斷類型與對應的 workflow。

交給 skill 前先整理好輸入：
- 事件描述照使用者原話保留，不改寫成自己的判斷。
- 使用者提到的 repo、分支、PR 號、檔案路徑、會議逐字稿、決策內容，逐一列出。
- 判斷不出事件類型或缺資訊時，用選項式提問問清楚再交出去，不要猜。

事件對照、流程、閘門與回報方式一律照 `tpm-team` skill，這支指令不另外規定。所有回覆用專案設定的輸出語言（預設繁體中文）。
