---
description: 照一份簡報（上層系統給編號的任務）執行：拆工、平行處理、反駁、產出文件，程式子任務轉 SDLC
argument-hint: <簡報檔路徑或任務編號>
---

使用者要執行這份簡報：

$ARGUMENTS

照 `tpm-team` skill 的規矩執行（先用 Skill 工具載入 `tpm-team`）：

1. **讀設定與簡報**：`.claude/teams/rules.md`、`.claude/teams/guard.config.json`；簡報在 `briefs_dir`（預設 `_tpm/briefs`）。只給編號時，在 `briefs_dir` 找檔名以該編號開頭的檔；找不到或有多個就問使用者。**不自編任務編號**。
2. **讀簡報全文**，特別是硬規矩與驗收標準段；不明確的地方列出來用選項式提問。
3. **先列計畫，等核准**：workflow `event-brief`、args（`brief`、`run_dir`＝`<runs_dir>/<YYYYMMDD>-brief-<短名>`）、預計開幾個 agent、哪些子任務是程式（會轉 SDLC，各自再過閘門）、不可逆節點。簡報建議的分支名不符合 `allowed_branch_prefixes` 時，改用允許的前綴並在計畫裡註明。
4. **執行**：`save_run.py init` → `Workflow({name: "event-brief", args: {...}})` → `save_run.py stage` 驗證信封。
5. **回報**：每個 agent 的 status／summary、所有 open_questions（blocking 的用 AskUserQuestion）、skeptic 推翻的發現、對簡報驗收標準的自我驗收結果、derived_work（只列，不編號，交上層系統）。
6. 不可逆動作一律停下等使用者當下同意。
