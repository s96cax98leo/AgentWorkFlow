# SDLC 依需求類型分流（recipe）

完整 SDLC 很耗時間與 token，所以 `sdlc-plan` 第一步先理解需求（triage：派 1 個 `tpm-planner`），建議這次走哪個 recipe；**由人在 plan 閘門選定，工具只准往上升級**。recipe 定義在開發規範 profile 的 `recipes`／`modifiers`（使用者可改），內建 11 個：

| recipe | 情境 | 走的段 | plan 段做法（plan_mode） |
|---|---|---|---|
| feature | 新功能 | plan → design → build → sync → test → deliver → merge | feature（size=small 時調研 2 個角度、不拆子計劃） |
| bugfix | 回報 bug、錯誤行為 | plan → build → sync → test → deliver → merge | diagnose：重現、根因（附 檔案:行號）；修正＋回歸測試 |
| change | 需求變更（CR）、改既有功能 | 全部 7 段 | impact：先盤點既有驗收條件要改、留、刪哪些；design 只寫差異；test 加跑受影響舊驗收條件的回歸 |
| hotfix | 線上事故 | plan → build → test → deliver → merge | diagnose（1 個調研角度）；**結案前一定要開後續 run** |
| refactor | 不改行為的重構 | plan → build → sync → test → deliver → merge | invariants：列出不能改變的行為，驗行為等價 |
| deps | 依賴升級 | plan → build → sync → test → deliver → merge | changelog：破壞性變更；一定做安全審查 |
| docs | 只改文件、說明、文案 | plan → build → deliver → merge | research（1 個）；build 只能有 doc 子任務 |
| test-only | 補測試、修 flaky | plan → build → test → deliver → merge | research（1 個）；不能改產品程式 |
| perf | 效能 | 全部 7 段 | baseline：先量基線，前後對照 |
| spike | 調研、可行性 | plan | research；不能推送 |
| revert | 合併後回退 | plan → build → test → deliver → merge | revert：讀 `event-post-merge` 的提案；build 只跑 git revert |

modifiers 疊在 recipe 上、只會加嚴：`ui`（一定設計審查）、`security`（一定安全審查）、`migration`（加回 design 段與回滾計畫）、`public_api`（加回 design 段檢查相容性）、`large`（四個調研角度、可拆子計劃）。

- **不管哪個 recipe，都不能省**：plan 段的 intent（必要節由 `save_run.py stage plan` 檢查，缺節不能核准）、plan 閘門的人工核准、每張派工單引用 `rules.md`、每個 agent 的執行紀錄、`merge_gate.py`、只推 fork。
- **使用者已經講明類型**（例如「修這個 bug」「CR：…」）時，args 直接帶 `recipe`（與 `modifiers`、`size`），`sdlc-plan` 不派 triage。**續跑**既有的 run 時，從 `run.json` 的 `recipe` 帶入，不重新分類。
- **plan 閘門要列**：triage 的理解與理由（附出處）、信心與替代選項、選定的 recipe 與要走的段、預估派幾個 agent、執行紀錄索引的路徑。triage 信心低（< 0.7）時 workflow 會先用 feature，請使用者在閘門選。
- **之後的段**：args 一律帶同一組 `recipe`、`modifiers`、`size`（取自 `run.json`）；各段回傳的 `next` 依 recipe 指向下一段，沒有的段不跑。沒有 design 段的 recipe，`sdlc-plan` 會回傳 `subtasks`，直接交給 `sdlc-build`；驗收條件以 intent.md 為準。
- **升級（escalate）**：中途發現範圍比想的大（例如 docs 其實要改程式、bugfix 碰到 migration），用
  `python3 .claude/teams/tools/save_run.py escalate <run_dir> --recipe <名> [--modifiers a,b] --by <誰> --at <ISO 時間> --reason "<理由>"`。
  只准加嚴；新加的段之後已做完的段會改成 stale，要重跑。降級一律拒絕，要降級只能 abandon 後重開 run。`publish.py` 與 `merge_gate.py --recipe` 會用實際的 diff 反查，recipe 不夠就拒推／hold，提示用 escalate。

## hotfix

hotfix 用 `recipe=hotfix`：plan → build → test → deliver → merge（沒有 design、sync），範圍縮到修這一個問題：
- intent 只寫這個缺陷的現象、重現步驟、預期行為、根因與驗收條件（缺節不能核准）；deliver 只審 bugs 軸。
- **不能省的閘門**：intent 核准（一律由人）、test 結果確認（要有重現缺陷的測試，修正後轉綠）、推 fork／開 PR 的當下同意，以及 merge 段的需求對照與 `merge_gate.py`。不用 `--by policy` 跳過人。
- **後續 run**：hotfix 跳過的 design、sync 與完整審查，要另開一個 run（通常 recipe=bugfix 或 change）補齊。記錄：`python3 .claude/teams/tools/save_run.py followup <run_dir> --run <後續 run 目錄> --by <誰> --at <ISO 時間>`；沒有記錄後續 run 時 `save_run.py close` 會拒絕結案。
- 合併後一律跑 `event-post-merge` 監看。凍結期間不跑合併，hotfix 也一樣，由人解凍或由人合併。
