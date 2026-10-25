---
name: correct
description: 把一則教訓變成強制機制：從踩坑手冊條目、執行紀錄裡的 followup、合併後監看的歸因或使用者描述的錯誤出發，找到真實發生過的例子，選最高可行的強制層級（架構／確定性工具 → schema／hook／lint → 測試 → 文件），用舊錯誤證明擋得住、用修正版證明會過，並在 rules.md〈專案規則〉維護「規則 ↔ 由什麼強制」對照表。使用者說「把這個錯誤變成做不到」「別再犯這個錯」「從踩坑手冊加強制」「這個 followup 又出現了」「/correct」時使用。
argument-hint: <錯誤描述、踩坑手冊條目、followup 路徑或 commit／PR（可省略）>
---

# 把錯誤變成做不到（correct）

一則教訓只寫在文件裡，下一個 agent 不一定會讀。這個 skill 把它往上推到「機器會擋」的層級，並留下證明：舊的錯誤真的會被擋、修正後的版本真的會過。

使用者給的線索：$ARGUMENTS

每一步先說明要做什麼，**動檔前列出完整計畫，等使用者同意才改**。提問用選項式（AskUserQuestion），附預設值。所有回覆用專案設定的輸出語言（預設繁體中文）。

## 原則

- **只用專案已有的工具。** 不規定用哪種 lint、測試框架或型別系統；專案沒有的工具不引進，最多列為選項請使用者決定。模版只管工作流程。
- **不憑空造例子。** 強制機制要對著真實發生過的錯誤寫，否則容易擋錯東西或擋不到。
- **只會更嚴。** 新增的檢查碰到輸入缺漏時 hold 或失敗，不做樂觀推定。
- **不碰信任邊界設定。** `.claude/teams/guard.config.json` agent 不能改（hook 會擋）；需要改 guard 設定時，產出 diff 交給使用者自己改，`auto_merge` 啟用時走 PR＋CODEOWNERS。

## 教訓的來源

| 來源 | 在哪裡 |
|---|---|
| 踩坑手冊 | 專案自己的踩坑紀錄；模版 repo 是 `docs/踩坑手冊.md` |
| merge 的 followup | `guard.config.json` 的 `runs_dir` 底下各 run 的 `followups/` |
| 合併後監看的歸因 | `event-post-merge` 的執行紀錄、revert 提案 |
| 審查發現 | 重複出現在 `findings`、`verifier.md`、`design-review.md`、`security-review.md` 的同一類問題 |
| 使用者描述 | 對話中直接說的錯誤 |

## 1. 找到真實發生過的例子

- 從線索找出錯誤實際發生的位置：commit SHA、PR、run 目錄裡的檔案、CI 紀錄。用 `git log -S`、`git log --grep`、`gh pr view` 或讀 run 紀錄查證。
- 列出：錯誤長什麼樣、在哪裡（`路徑:行號` 或 SHA）、後來怎麼修（修正 commit，沒有就寫「尚未修」）。
- **找不到就停下，請使用者提供**例子（SHA、PR、檔案片段）。不要自己編一個「大概長這樣」的例子。
- 同時確認這是「會重複發生」的錯誤：出現過不只一次，或一旦發生代價很高。只發生一次、代價低的，建議只補文件。

## 2. 選強制層級

由硬到軟列出四層，**選最高可行的一層**，並寫出為什麼不選更高層（例如「沒有型別系統」「這是判斷題，無法確定性檢查」「hook 看不到這個資訊」）。

| 層級 | 目標專案 | AgentWorkFlow 模版 repo 自己 |
|---|---|---|
| **1. 架構或確定性工具**：讓錯誤寫不出來或流程走不下去 | 專案自己的型別、模組邊界、建置步驟 | `merge_gate.py` 的判定、`schemalib.py` 的語意規則、workflow 裡的確定性核對（回傳 `halted: true`） |
| **2. schema／hook／lint**：錯誤寫得出來但會被立刻擋 | 專案既有 lint 的規則、pre-commit | 信封與 payload schema（`schemas/`）、`guard.py` 的 guard 規則、`check_rules.py` |
| **3. 測試**：錯誤會在跑測試時被抓 | 專案既有的測試 | `golden-set/valid|invalid/`（含 `semantic` 反例）、`check_workflows.mjs` 的 `SCENARIOS`、`plugin/hooks/tests/run.py` |
| **4. 文件**：只能靠人或 agent 讀到 | rules.md〈專案規則〉、agent 提示、專案 CLAUDE.md | `rules.md` 通用規則、agent 提示、`docs/踩坑手冊.md` |

- 一條規則可以同時用兩層（例如第 2 層擋、第 3 層證明），對照表記最高的那層。
- 判斷題（需要讀懂語意才能決定的）通常停在第 4 層，或交給審查 agent 的 lens；這時標「僅文件」，不要硬寫一個會誤擋的規則。

**在模版 repo 自己身上用時的對應**（照 `CLAUDE.md`〈改動時的檢查清單〉）：
- 語意規則：寫在 `schemalib.py`，在 `golden-set/invalid/` 補 `semantic` 反例、`valid/` 補正例；改了 schema 重跑 `build.py`。
- guard 規則：寫在 `plugin/hooks/` 的 guard，在 `plugin/hooks/tests/run.py` 補一個該擋、一個該放的案例，Bash 與 PowerShell 各一。
- workflow 核對：寫在 `scaffold/.claude/workflows/*.js`，乾跑走不到的分支在 `check_workflows.mjs` 的 `SCENARIOS` 補情境。

## 3. 列計畫，等使用者同意

計畫要寫：
1. 錯誤的真實例子（第 1 步的結果）。
2. 選哪一層、改哪些檔、為什麼不選更高層。
3. 兩個證明指令：
   - **擋得住**：對舊的錯誤版本跑，預期失敗（exit 非 0、hook exit 2、gate 判 hold／escalate、workflow `halted: true`）。
   - **會放行**：對修正後版本跑，預期通過。
4. 要不要更新對照表、踩坑手冊。
5. 有沒有要交給使用者自己改的部分（`guard.config.json` 等）。

使用者同意才進下一步。使用者說「調整」不是同意，先改計畫。

## 4. 實作並證明

- 照計畫改檔。改到 `.claude/teams/` 的 schema 或 methodology profile 時，重跑 `python3 .claude/teams/tools/build.py`。
- 重現舊錯誤的方法，依序選：
  1. 直接用舊 commit 的檔案（`git show <sha>:<路徑>` 存到暫存目錄）。
  2. 用舊錯誤的實際內容做成反例檔（golden-set 反例、hook 測試案例、`SCENARIOS` 假回傳）。
  3. 前兩者都不行時，在暫存目錄重建最小重現，並註明和原錯誤的差異。
- **兩個證明指令都要實際跑過**，回報指令與輸出摘要（exit code、關鍵訊息）。只寫「應該會擋」不算完成。
- 擋不住或誤擋修正版時，回第 2 步重新選層，不要放寬檢查來讓修正版通過。
- 跑專案既有的完整驗證（rules.md〈專案規則〉的 verify 指令；模版 repo 是 `python3 tests/run_all.py`），確認沒有誤擋其他東西。

## 5. 更新對照表

在 `.claude/teams/rules.md`〈專案規則〉的「規則 ↔ 強制方式」對照表加一列（同一條規則已有就更新）：

| 欄位 | 寫什麼 |
|---|---|
| 規則 | 一句話，寫成「不得…」或「必須…」 |
| 強制方式 | 檔案與函式、規則名或測試案例，附路徑 |
| 層級 | 1 架構／確定性工具、2 schema／hook／lint、3 測試、4 文件 |
| 證明擋得住的指令 | 第 4 步跑過的「擋得住」指令 |

只能停在文件層的，強制方式寫「僅文件」並寫出寫在哪裡。來源是踩坑手冊條目時，在該條目的「做法」補一行「已由…強制（見 rules.md 對照表）」。

## 6. 交件

在對話裡列出：
- 真實例子與出處。
- 選的層級與不選更高層的理由。
- 改了哪些檔。
- 兩個證明指令與輸出摘要。
- 對照表新增或更新的那一列。
- 交給使用者自己處理的部分（例如 `guard.config.json` 的 diff）。

不 commit、不 push；由使用者決定。
