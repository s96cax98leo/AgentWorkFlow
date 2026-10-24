---
description: Demo 前檢查：照腳本逐步實測目標環境，列出會出包的地方與備援
argument-hint: <目標（環境、分支或網址）；可附 demo 腳本路徑與日期>
---

使用者要做 demo 前檢查：

$ARGUMENTS

照 `tpm-team` skill 的規矩執行（先用 Skill 工具載入 `tpm-team`）：

1. **讀設定**：`.claude/teams/rules.md`、`.claude/teams/guard.config.json`。
2. **補齊 args**：`target`、`run_dir`（`<runs_dir>/<YYYYMMDD>-demo-check-<短名>`），可選 `script`（demo 腳本路徑）、`date`。缺漏就問。
3. **先列計畫，等核准**：要檢查哪些步驟、會啟動哪些服務、會不會動到使用者本機的既有環境。
   - 用 docker compose 時一律帶專屬專案名 `-p <demo 專屬名>`，避免撞到使用者本機同名專案；提醒環境變數 `COMPOSE_PROJECT_NAME` 會覆蓋設定。
   - `.env` 是 symlink 時，會寫 .env 的步驟（例如產生金鑰）會寫到原檔：先記 sha，收尾還原。
   - 機敏檔不讀、不印，只用 `grep -q` 判斷有無。
4. **執行**：`save_run.py init` → `Workflow({name: "event-demo-check", args: {...}})` → `save_run.py stage`。
5. **回報**：逐步的 pass／fail、會出包的地方（附重現與證據）、建議的備援（離線字體、假資料、錄影備份等）、需要使用者決定的事（選項式）。
6. 不修改上游；要改程式的項目列成 derived_work 或轉 `/sdlc`，不自行動手。
