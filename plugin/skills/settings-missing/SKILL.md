---
name: settings-missing
description: 已套用 AgentWorkFlow 的專案只補還沒設定過的項目：列出答案檔裡沒有回答過的主題（升級後新版多出來的也算），只問那些，已經設定過的不問、不動。使用者說「補沒設定的」「還有哪些沒設定」「升級後要補什麼設定」「/agent-workflow:settings-missing」時使用。要全部重問一輪用 /agent-workflow:settings。
argument-hint: <目標專案路徑（可省略＝目前目錄）>
---

# 只補還沒設定的（settings-missing）

使用者給的：$ARGUMENTS（沒給路徑就用目前工作目錄）。

1. 跑 `awf init --detect --unset --target <專案> --json`：只會列出**還沒設定過**的主題（答案檔 `.claude/teams/init-answers.json` 裡沒有那個主題的答案；交付文件另看有沒有 `deliverables.json`）。`topics` 是空的就告訴使用者「每個主題都設定過了」，結束。
2. 先把清單列給使用者（主題名稱與每題要確認什麼），問他要補哪些；不想設的可以跳過，維持預設。
3. 對要補的主題逐題問（AskUserQuestion，選項式），每題列偵測到的現況與預設值。
4. **交付文件**在清單裡時：照 `/agent-workflow:deliverables` 設定要交哪些文件；有固定格式的範本檔就用 `awf deliverables configure --template <文件id>=<範本檔> [--summary-template <範本檔>]` 掛上去（提案給使用者看過再加 `--write --by <使用者>`）。交付文件已經設定過、只是還沒掛範本時，這支不會列出它：使用者提到範本就直接跑那個指令。
5. 把答案寫成一個 JSON 檔，跑 `awf init --target <專案> --reconfigure --unset --answers <檔>`。帶 `--unset` 時答案檔只能有還沒設定的主題的鍵，寫到已經設定過的會被拒絕（不會誤改）。把「設定對照表」與「會改 N 項」唸給使用者。
6. 直接手改 `methodology.json`、沒經過訪談的設定，這裡仍然會列成還沒設定：告訴使用者那一項檔案裡已經有值，要不要照現在的值記成答案由他決定。
7. 最後跑 `awf doctor`。

什麼時候用：`awf init --upgrade` 之後（新版多了主題）、當初用預設值套用想之後再補、或想知道還有哪些可以設定。
