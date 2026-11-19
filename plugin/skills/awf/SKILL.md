---
name: awf
description: awf 命令列的總入口：AgentWorkFlow 的任何操作（開 run、下一步、存結果、核准、推送、合併閘門、同步分支、查知識庫與知識圖、資料包、交付文件、成本、進度、規格層、多需求排程、安裝檢查、設定、去客戶化掃描、CI 檔、checklist）該用哪個子指令、怎麼下、哪些要人先決定。使用者說「用 awf 做…」「awf 怎麼…」「這個要下什麼指令」「幫我跑 awf」「/agent-workflow:awf」，或你需要用 awf 卻不確定子指令與參數時使用。
argument-hint: <想做的事，或子指令名>
---

# awf 命令列（awf）

使用者要做的：$ARGUMENTS

`awf` 是單一執行檔，由 plugin 的 `bin/` 提供，直接叫 `awf <子指令>`，不帶路徑。除了 `init`、`doctor`、有 `--selftest` 的檢查之外，都在**已套用的專案根目錄**執行。

## 怎麼用這份 skill

1. 在下面的表找到對應的子指令。
2. 確切的參數以工具自己的說明為準：`awf <子指令> --help`（每一個子指令都只印說明、不執行）。常用寫法與注意事項在 `reference/指令.md`。
3. 看「誰決定」那一欄。標「人」的，**先把要做的事與指令給使用者看、得到明確同意才跑**；`--by` 填使用者本人，不能填 `policy` 以外的假名、也不能替使用者決定。
4. 輸出照實回報：exit code 不是 0、`halted`、`hold` 都原樣說明，不要換一種寫法重試到過為止。

## 子指令對照

| 想做的事 | 子指令 | 會寫入 | 誰決定 |
|---|---|---|---|
| 開一個新的 run | `awf start` | run 目錄 | 你 |
| 這個 run 的下一步是什麼 | `awf next` | run.json、資料包、腳本 | 你；`--retry --skip-subtask`、`--confirm-merge`、`--revise` 要人 |
| 收下一段 workflow 的回傳 | `awf done` | 段的結果 | 你；`--output` 手動給結果會標成非標準 |
| 核准一段閘門、回答問題 | `awf approve` | 核准紀錄、決策記憶 | **人** |
| 產生 workflow 腳本來看 | `awf render` | 輸出目錄 | 你 |
| 換 session 或換人接手 | `awf handoff` | 接手摘要 | 你 |
| 看進度 | `awf watch --once`、`awf statusline` | 進度事件 | 你 |
| 存結果、升級 recipe、結案、放棄、記決定 | `awf save-run` | run 目錄 | `stage`、`status`、`archive` 你；`approve`、`decide`、`escalate`、`close`、`abandon` **人** |
| 備資料包 | `awf pack` | 資料包 | 你（`awf next` 會自己跑） |
| 推分支、開 PR | `awf publish` | 遠端 | **人**（每次都要） |
| 這個 PR 能不能合併 | `awf merge-gate` | merge-audit | 你跑、結果照實；合併本身 **人** 或合併 agent |
| 分支怎麼跟上主線 | `awf sync-branch` | `plan` 只讀；`apply` 改分支 | `apply` **人** |
| 查專案文件 | `awf kb query` | — | 你；`kb build` 重建索引也可以 |
| 查知識圖（位置、追溯、影響） | `awf kg locate`／`trace`／`impact`／`neighbors`／`check` | — | 你；`kg build`、`kg export` 要人同意（耗時、寫檔） |
| 用需求的詞找相關的檔 | `awf locate` | — | 你 |
| 這種需求要走哪些段 | `awf recipe` | — | 你 |
| 多個需求的順序 | `awf program status`／`next` | — | 你 |
| 交付文件的設定、盤點、範本 | `awf deliverables` | `configure --write` 寫設定 | 設定 **人**；`status`、`check`、`affected`、`matrix` 你 |
| 花了多少 token 與時間 | `awf cost` | — | 你 |
| 規格層（OpenSpec） | `awf spec` | `apply --write`、`ids --write` 寫規格 | 寫入 **人** |
| 跑長輸出的指令只看重點 | `awf run-brief --log <檔> -- <指令>` | log 檔 | 你 |
| 計畫的 checklist 照實打勾 | `awf checklist check` | `--write` 改 checklist | 你（它會執行檔裡的驗證指令，先看過內容） |
| 檢查安裝與設定 | `awf doctor` | — | 你 |
| 重新內嵌 workflow | `awf build`、`awf build --check` | 覆寫檔 | 你 |
| 自訂腳本的檢查 | `awf script check` | — | 你 |
| 檢查 agent 檔、信封、產出 | `awf check-agents`、`awf check-envelope`、`awf check-rules`、`awf golden` | — | 你 |
| 設計系統的色碼與 tokens | `awf tokens check`／`verify` | — | 你 |
| 去客戶化掃描 | `awf scrub` | — | 你 |
| 產生專案的 CI 檔 | `awf ci render`、`awf ci check` | `--write` 寫 CI 檔 | 寫入 **人** |
| 套用、升級、改設定 | `awf init` | 專案設定 | **人**（用 `/agent-workflow:init-workflow`、`settings`、`settings-missing`） |
| hook 入口 | `awf hook` | — | 不手動跑 |
| 各工具的自我檢查 | `awf <子指令> --selftest`（狀態機是 `awf state --selftest`） | 暫存目錄 | 你 |

## 一定要守的

- **開發需求照 `/agent-workflow:tpm-team` 的迴圈**（`awf next` → 照做 → `awf done`）。不要自己組 `save-run stage`、手改 run.json 或產生的腳本去繞過：那樣存的結果會被標成非標準，之後不能政策核准。
- **閘門只會更嚴**：`awf` 回 `hold`、`halted`、`need_input`、`gate` 時交給使用者，不要改參數硬闖。
- **合併**：agent 的合併只走 PR 與 `awf merge-gate`；不跑 `gh pr merge`、不 approve。
- **team agent（`tpm-*`、`dev-*`）只能跑唯讀的查詢**（`kb query`、`kg locate`、`locate`、`deliverables status` 等），其餘由主 session 跑；派工時不要叫它們跑會寫入的 `awf`。
- **要查很多地方、或要重建索引時派資料助手**（主對話只拿結論，省 context）：`awf-lookup`（查詢員，只讀）查資料並整理，有五種模式：診斷（run 卡在哪、下一步）、閘門摘要（核准前一頁重點）、成本、交付盤點、設定體檢；`awf-refresh`（更新員）只能跑 `awf kb build`、`awf kg build`、`awf deliverables index`。兩個都不能核准、推送、改檔。
- 有對應的專門 skill 時優先用它：`tpm-team`（流程）、`init-workflow`／`settings`／`settings-missing`（設定）、`deliverables`、`kb`、`status`、`cost`、`doctor`、`add-agent`、`correct`。
