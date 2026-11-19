---
name: settings
description: 已套用 AgentWorkFlow 的專案把全部設定重新問一輪：每個主題（開發規範、慣例、需求來源、模型分級、知識庫與知識圖、交付文件與範本、CI、合併方式、守門…）逐題確認，預設值是上次的答案，只改有變的設定。使用者說「重新設定」「全部設定問一遍」「檢查所有參數」「/agent-workflow:settings」時使用。只想補還沒設定的用 /agent-workflow:settings-missing。
argument-hint: <目標專案路徑（可省略＝目前目錄）；也可以給一個主題 id 只問那一個>
---

# 全部設定問一輪（settings）

使用者給的：$ARGUMENTS（沒給路徑就用目前工作目錄）。

升級不會洗掉設定；這支是使用者想整個檢查或調整時用的。**只問、確認後才寫**，偵測到的東西不能當成使用者的決定。

1. 跑 `awf init --detect --target <專案> --json`，拿到全部主題（`id`、`title`、`ask`、`answer_keys`、偵測到的現況、建議的答案）。讀 `<專案>/.claude/teams/init-answers.json` 當上次的答案（沒有就當沒回答過）。
2. 照主題的順序逐題問（AskUserQuestion，選項式）：每題列「上次的答案／偵測到的現況／預設」，使用者可以直接接受不改。一次問一個主題，不要一口氣丟全部。
3. **交付文件**這個主題多問兩件事（它們不進答案檔）：
   - 要交哪些文件：照 `/agent-workflow:deliverables` 的做法（`awf deliverables configure`）。
   - 有沒有固定格式的範本檔（Markdown，放在專案裡）：有就問是哪一份文件的，跑 `awf deliverables configure --template <文件id>=<範本檔> [--summary-template <範本檔>]`，提案給使用者看過再加 `--write --by <使用者>`。
4. 把**有變的答案**寫成一個 JSON 檔（只寫要改的鍵；值寫 `null`＝拿掉那個答案），跑 `awf init --target <專案> --reconfigure --answers <檔>`。把輸出的「設定對照表」與「會改 N 項」唸給使用者，和他說的不一樣就改檔重跑。
5. 使用者只給了一個主題 id 時：只問那一個，用 `awf init --target <專案> --reconfigure <主題> --answers <檔>`（可以單獨問的主題見 `awf init --help`）。
6. 最後跑 `awf doctor`，有問題照它的修法處理。

每次執行才決定的值（查什麼主題、這次用哪個範本）不在這裡設：自訂段與自訂事件的文字裡寫 `{參數名}` 或 `{參數名|預設}`，執行時用 `awf next --set 參數名=值` 帶。
