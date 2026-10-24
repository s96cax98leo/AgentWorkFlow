---
description: 啟動需求開發的 AI-native SDLC（plan → design → build → test → deliver → merge，段與段之間由人核准；merge 由可信任的合併 agent 對照需求後執行）
argument-hint: <需求描述或需求檔路徑；可附 repo、epic、base>
---

使用者要開發這個需求：

$ARGUMENTS

照 `tpm-team` skill 的規矩執行（先用 Skill 工具載入 `tpm-team`；以 plugin 安裝時名稱是 `agent-workflow:tpm-team`）：

1. **先讀設定**：`.claude/teams/rules.md`（含〈專案規則〉段）、`.claude/teams/guard.config.json` 與開發規範 profile `.claude/teams/methodology.json`（各段做法與慣例；workflow 已由 build.py 內嵌，列計畫時說明用哪個 profile）。讀不到就停下，請使用者先跑 `/init-workflow`。
2. **補齊 args**：`repo`（只能是 `repos_dir` 下我方的 clone，`forbidden_paths` 一律拒絕）、`epic`、`requirement`、`base`、`run_dir`（`<runs_dir>/<YYYYMMDD>-sdlc-<短名>`，日期由你填）。缺漏或模糊的用 AskUserQuestion 選項式提問，不要猜。
3. **先列計畫，等核准**：列出事件、workflow 順序、args、預計開幾個 agent、會碰到的不可逆動作（推 fork、開 PR、寫上游），經使用者核准才執行。
4. **逐段執行**：`save_run.py init` → `sdlc-plan`。每段結束都依 tpm-team skill 的閘門表，把重點**列在對話裡**（不只依賴工具狀態），等使用者核准後用 `save_run.py approve` 記錄，才進下一段。使用者回「調整」不是核准。
5. **不可逆一律停下**：推送、開 PR、寫入上游都要使用者當下同意；agent 不推、不核准。需要決策時用 PushNotification 推到手機。
6. **合併段**：deliver 之後、PR 已開，使用者同意進入合併段時跑 `sdlc-merge`。只有 `dev-merge-steward` 能合併，條件是需求逐條對照一致而且 `merge_gate.py` 判 merge；不一致時 PR 會被退回（request-changes、轉回 draft），新需求寫在 `<run_dir>/followups/`，照回傳的 `next` 走 `event-reqs-change` 或回到 SDLC 對應段落。合併後跑 `event-post-merge` 監看 main。
7. 所有派工單與回覆用專案設定的輸出語言（預設繁體中文）。
