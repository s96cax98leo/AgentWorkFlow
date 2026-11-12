---
name: tpm-team
description: TPM agent team 的入口。開發需求走 AI-native SDLC（段與段之間由人核准），其他事件（簡報、上游變動、會議、PR 審查、合併、需求變動、決策落地、demo 前檢查）平行處理後產出文件。使用者說「開發這個需求」「跑簡報」「整理這場會議」「審這個 PR」「合併這個 PR」「用 agent team 處理」時使用。
argument-hint: <事件描述，例如「開發配額告警需求」「跑簡報 <任務編號>」「審 feature/x」>
---

# TPM agent team 入口

這個 session 是執行者（主 session）：派工、彙整、過閘門。硬規則在 `.claude/teams/rules.md`；讀不到它或 `.claude/teams/guard.config.json` 就停下，請使用者先跑 `/agent-workflow:init-workflow`。

流程由狀態機 `awf.py` 排：每一段只做 `next`（照它說的做）和 `done`（存結果）。參數、資料包、時間、存檔與驗證都由它處理，不要自己組。

## 1. 開一個 run

1. 判斷事件與 recipe（對照表在 `reference/事件.md`、`reference/recipe.md`）。判斷不出來就用 AskUserQuestion 問，不要猜。
2. **先列計畫給使用者看**（事件、要走的段、預計派幾個 agent、不可逆的動作），核准後才執行。
3. 開 run：`run_dir` 用 `<runs_dir>/<YYYYMMDD>-<事件>-<短名>`。

```bash
python3 .claude/teams/tools/awf.py start --run <run_dir> --event <事件> [--repo <repo>] [--epic <短名>] [--requirement "<原文或路徑>"] [--base <base>] [--recipe <名>] [--modifiers a,b] [--set k=v]…
```

目標 repo 只能是我方的 clone；禁止路徑一律拒絕；任務編號照原文，不自編。

## 2. 迴圈

```bash
python3 .claude/teams/tools/awf.py next --run <run_dir>     # 回一個動作（JSON；加 --text 是給人看的摘要）
```

| `action` | 怎麼做 |
|---|---|
| `run_workflow` | `Workflow({scriptPath: <script>})`，args 已內嵌、不用再傳；有 `resume_from` 就當 `resumeFromRunId` 帶上。跑完執行 `then`（`awf.py done --run <run_dir>`），它會存檔、驗證並回下一個動作 |
| `gate` | **把 `highlights` 在對話裡列給使用者看**（未決問題、skeptic 推翻了什麼、缺件、執行紀錄索引的路徑）。blocking 的問題合成一次、用 AskUserQuestion 問；`provisional`（agent 已照建議先做的暫定）一次列出請使用者確認。使用者核准才跑 `approve` 的指令：答覆用 `--answer "<問>=<答>"` 記下來（照原話、不延伸），暫定的都同意就加 `--accept-defaults` |
| `need_input` | 照 `need` 的說明問使用者或從 spec 判斷，用 `next --set <名>=<值>` 補上 |
| `manual` | 要人當下同意的步驟（推 fork、開 PR）。同意後照 `how` 做，再跑 `then` |
| `halted` | 把 `reason`、`advice` 講給使用者聽，處理完用 `retry` 重跑。不進下一段 |
| `done` | 回報結果與成本（`awf_cost.py --project . --run-dir <run_dir>`），做 `reference/閘門.md` 的收尾 |

## 3. 不能省的

- plan 段一律由人核准；「調整」「可以但…」不是核准，附帶的意見要先處理。其他閘門預設也等人；只有 recipe 的 `gate_mode` 是 plan-only（內建：docs、test-only；帶任何 modifier 就不算）時，非 plan 的閘門會先試政策核准：條件由 `save_run.py approve --by policy` 判，結果不乾淨就照常交人（`reference/閘門.md`）。
- 推 fork、開 PR、寫上游、合併，各自要使用者當下同意；上游一律唯讀。只有 `dev-merge-steward` 能合併，而且要 `merge_gate.py` 判 merge。
- 缺件不等於沒有發現：`missing` 一定講明，問要不要重跑。
- 需要決策者拍板時用 PushNotification 推到手機。`derived_work` 只列出來，不編號。
- 派工單與 agent 間訊息用專案設定的輸出語言。

## 4. 例外

- design 被退回、merge 回 `needs_sync`、test 失敗退回 build、需求取消（abandon）、升級 recipe（escalate）、hotfix 的後續 run：見 `reference/閘門.md` 與 `reference/recipe.md`。指定段或 workflow 用 `next --stage <段> [--workflow <名>]`。
- 事件的回傳與轉 SDLC 的交接：`reference/事件.md`。
- `awf.py` 用不了，或涉及授權判斷的敏感流程要改用 Agent 工具逐段派：`reference/手動執行.md`。
- 換 session 或換人接手：`awf.py handoff --run <run_dir>` 寫 `接手摘要.md`；新的 session 先讀它再跑 `next`。
- 要比較三個以上的方案：`team-debate` skill。
