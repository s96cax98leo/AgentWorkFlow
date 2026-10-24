---
description: 判斷事件類型（上游變動、會議、PR 審查、合併後監看、需求變動、決策落地、demo 檢查、方案辯論）並跑對應的 workflow
argument-hint: <事件描述，例如「上游 main 前進了」「審 feature/x」「決策者決定採方案 B」>
---

使用者回報了一個事件：

$ARGUMENTS

照 `tpm-team` skill 的規矩執行（先用 Skill 工具載入 `tpm-team`）：

1. **讀設定**：`.claude/teams/rules.md`、`.claude/teams/guard.config.json`。
2. **判斷事件**：對照 tpm-team skill 第 1 節的事件表，決定 workflow 與必要 args：

   | 事件 | workflow |
   |---|---|
   | 上游新 PR、main 前進、CI 失敗 | `event-upstream` |
   | 會議、逐字稿 | `event-meeting` |
   | 審 PR／分支 | `event-pr-review` |
   | 合併後監看、main 紅了是不是合併造成 | `event-post-merge` |
   | 合併已開好的 PR | `sdlc-merge`（SDLC 合併段，見 `/sdlc`） |
   | 需求更新 | `event-reqs-change` |
   | 決策者定案 | `event-decision` |
   | demo 前檢查 | `event-demo-check`（也可用 `/demo-check`） |
   | 要比較多個方案 | `team-debate` skill |
   | 開發需求／跑簡報 | 改用 `/sdlc`／`/brief` |

   判斷不出來或 args 缺漏時，用 AskUserQuestion 選項式提問，不要猜。
3. **先列計畫，等核准**：事件、workflow、args（`run_dir`＝`<runs_dir>/<YYYYMMDD>-<事件>-<短名>`）、預計開幾個 agent、不可逆的動作。
4. **執行與回報**：`save_run.py init` → Workflow → `save_run.py stage`；整理 status、open_questions、skeptic 推翻的發現、derived_work（不編號）。
5. 上游一律唯讀；不可逆動作一律停下等使用者當下同意。
