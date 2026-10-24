---
name: team-debate
description: 方案辯論：簡報要求 ≥3 個方案、SDLC Design 段遇到架構取捨（寫 ADR 要比較 ≥2 個真實選項）、或要做給決策者的取捨時使用。每個方案一位 teammate 負責並互相挑戰，lead 彙整成比較表。使用者說「比較方案」「幫我辯一下」「A 還是 B」「做取捨給決策者」時使用。
argument-hint: <要決定的問題與候選方案>
---

# 方案辯論（Agent Teams；未開啟時改用 Workflow judge panel）

本 skill 的 `{{…}}` 佔位符，值以專案 `.claude/teams/rules.md` 的〈專案規則〉段為準。

## 0. 先確認 Agent Teams 有沒有開
- 要在 `.claude/settings.local.json` 的 `env` 設 `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`，而且只能在**互動式 session** 使用，`-p` 不行。
- **副作用**：開啟後，在這個工作區有名字的 subagent 會自動變成 teammate，site-report team 也一樣。所以**開不開由使用者決定**，不要自行修改設定。
- 沒有開：跳到第 3 節。

## 1. 建隊（Agent Teams）
1. 建 run 目錄：`python3 .claude/teams/tools/save_run.py init {{RUNS_DIR}}/<YYYYMMDD>-debate-<短名> --event debate`。
2. 每個方案建一個任務，任務描述**一定要有**這兩行，`TaskCompleted` hook 會照這個檢查：
   ```
   {{RUNS_DIR}}/<run>/
   output: {{RUNS_DIR}}/<run>/option-<n>.json
   ```
3. 每個方案開一位 teammate，用 `tpm-researcher` 的角色定義（plugin 安裝後的名稱是 `{{AGENT_NS}}tpm-researcher`，預設 `agent-workflow:tpm-researcher`），3–5 位為宜。派工內容（用 `{{OUTPUT_LANGUAGE}}` 寫）：
   - 負責哪個方案。
   - 要回答的問題。
   - 評估面向：代價、前提、需要 `{{DECISION_OWNER}}`／`{{STAKEHOLDER}}` 配合什麼、什麼情況下它才是對的、風險。
   - 證據要附 `路徑:行號` 或出處。
4. 產出寫成 `option-<n>.json`：交件信封，payload 用 `findings`（lens=`option-<n>`）。

## 2. 辯論規則
- **訊息只用來辯論**：挑戰別的方案的前提與證據。**JSON 不走 mailbox**，結果一律寫檔。
- 每位 teammate 至少挑戰另一個方案一次，回應時要附證據。
- 不替使用者或決策者做決定；lead 可以標建議並附理由。

## 3. 沒有 Agent Teams 時：Workflow judge panel
用 Workflow 跑同樣的結構：
1. 每個方案一個 `tpm-researcher`，平行執行，schema 用 `findings`。
2. 每個方案再由一個 `tpm-skeptic` 反駁，schema 用 `verdicts`。
3. 最後由 `tpm-writer` 寫比較表。

腳本照 `.claude/workflows/event-pr-review.js` 的 pipeline 寫法，schema 用 `/*@schema:…*/ null /*@end*/` 標記，再跑 `python3 .claude/teams/tools/build.py` 把 schema 填進去。

## 4. 彙整
`tpm-writer` 寫 `{{RUNS_DIR}}/<run>/comparison.md`，給決策者的版本另寫到 `docs/`（或 guard.config.json 的 `writer_dirs` 內其他目錄），並跑 `check_rules.py --audience owner`：
- 表格：方案、做法、代價、前提、需要決策者配合、什麼情況下才對。
- 被成功挑戰的論點，列在「存疑」。
- 決策者拍板後，走 `event-decision` 落地（見 `tpm-team` skill）。
