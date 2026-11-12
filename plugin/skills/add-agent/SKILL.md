---
name: add-agent
description: 協助使用者把自己的 agent 加進 AgentWorkFlow 的 SDLC workflow：確認要加在哪一段、套用哪個模版角色的守門規則、派工內容，必要時建立 agent 定義檔，先在 guard.config.json 的 extension_agents 登記守門角色、再寫進 methodology.json 的 extensions，跑 build 與驗證，失敗就回滾。使用者說「加我自己的 agent」「把這個 agent 放進 workflow」「新增審查 agent」「add agent」時使用。
argument-hint: <agent 名稱或想做的事（可省略）>
---

# 把自己的 agent 加進 workflow（add-agent）

使用者想讓自己的 agent 參與 SDLC，例如 migration 審查、無障礙檢查、內部規範檢查、領域知識調研。加進去之後，它會和該段的內建 agent **平行執行**，交 `findings`；blocker／important 的發現會擋住政策核准，deliver 段的發現一樣經 skeptic 反駁。

使用者給的線索：$ARGUMENTS

每一步先說明要做什麼，等使用者同意再改檔。提問用選項式（AskUserQuestion），附預設值。

## 能加在哪裡（只有這四段）

| stage | 和誰平行 | 適合 |
|---|---|---|
| `plan` | 4 個調研角度（tpm-researcher） | 領域知識、內部文件、外部系統現況的調研；結果會餵給 intent |
| `design` | 一致性檢查（tpm-skeptic） | 架構規範、資料模型、介面契約、合規要求的設計審查 |
| `test` | verifier、設計審查、安全審查 | migration、效能、無障礙、授權條款等針對實作的檢查 |
| `deliver` | 四軸 PR 審查（之後經 skeptic 反駁） | 團隊自己的 review checklist、程式風格以外的慣例 |

**不開放**：build（寫程式有檔案所有權與 worktree 分工）、merge（信任邊界，只有 `dev-merge-steward` 與確定性 gate）。使用者要的是這兩段時，說明原因，建議改成 test 或 deliver 段的檢查。

## 1. 確認專案已經 init
讀 `.claude/teams/guard.config.json` 與 `.claude/teams/methodology.json`。讀不到就停下，請使用者先跑 `/init-workflow`。
`guard.config.json` 的 `auto_merge.enabled` 是 true 時，主 session 不能直接改這個檔（hook 會擋）：照常整理改動，第 5 步停在只產出 diff（`guard.config.json` 與 `methodology.json` 兩份一起），請使用者走 PR＋CODEOWNERS，**不要**先單獨寫 `methodology.json`（沒登記守門的擴充 agent 不能進 workflow）。

**找模版根目錄**（下面驗證指令的 `<AgentWorkFlow>`）：`${CLAUDE_PLUGIN_ROOT}` 是 plugin 快取，只有 plugin 目錄，不能往上一層找。依序試：
1. `~/.claude/plugins/known_marketplaces.json`（Windows：`%USERPROFILE%\.claude\plugins\known_marketplaces.json`）裡 `agentworkflow` 的 `installLocation`。
2. `~/.claude/plugins/marketplaces/agentworkflow/`。
3. 都找不到（或目錄裡沒有 `plugin/agents/`）就問使用者模版 repo 在哪裡。
`<AgentWorkFlow>/plugin/hooks/awf_hook.py` 也可以用 `${CLAUDE_PLUGIN_ROOT}/hooks/awf_hook.py` 代替。

## 2. 確認是哪個 agent
- 列出專案 `.claude/agents/*.md`，以及使用者安裝的其他 plugin 裡的 agent，問要用現成的還是新建。
- **agentType 寫法**：專案 `.claude/agents/<名>.md` 的寫 `<名>`（不含 .md）；其他 plugin 的寫 `<plugin>:<agent>`。
- 名稱不能和模版角色同名（`dev-spec-author`、`dev-implementer`、`tpm-reviewer` 等 16 個）：同名會直接套模版角色規則，還會被誤認。也建議不要用 `dev-`／`tpm-` 開頭，避免混淆。
- 一個 agent 可以加在多段：每段一筆 extension，`name` 各自不同（例如 `db-design`、`db-test`）。

## 3. 問設定
1. **stage**：上表四段選一（可多選，每段一筆）。
2. **套用哪個模版角色的守門規則**（寫進 `guard.config.json` 的 `extension_agents`）。只能選唯讀類：

   | 角色 | 能寫哪裡 | 適合 |
   |---|---|---|
   | `tpm-reviewer`（預設，最嚴） | 只能寫 run 目錄 | 大部分審查、檢查 |
   | `tpm-researcher` | 只能寫 run 目錄 | 調研 |
   | `tpm-skeptic` | 只能寫 run 目錄 | 反駁、挑錯 |
   | `dev-verifier`、`dev-security-reviewer`、`dev-design-reviewer` | 目標 repo 與 run 目錄 | 要在 repo 裡留報告的檢查 |

   `dev-pr-reviewer` 是舊名，已併入 `tpm-reviewer`，照 `tpm-reviewer` 套用（只能寫 run 目錄）。

   不論選哪個，都不能 push、不能對 GitHub 寫入、不能合併或核准；填其他值會被當成 `tpm-reviewer`。
3. **派工內容**（`instructions`）：要檢查或調研什麼、依據哪些檔、什麼算 blocker／important、什麼只算 nit。寫成一段自足的派工單，不要依賴對話脈絡。
4. **name**：短名（小寫英數與 `-`），也是 findings 的 lens 與報告檔名 `ext-<name>.md`。

## 4. 新建 agent（使用者選新建時）
在專案 `.claude/agents/<名>.md` 建立，給使用者看過再寫：

```markdown
---
name: <名>
description: <一句話：做什麼、在 SDLC 哪一段被派出>
tools: Read, Grep, Glob, Bash
model: inherit
---

## 職責
<要檢查或調研什麼>

## 硬性約束
- 只讀不改：不改程式、測試與 spec，不 push、不對 GitHub 寫入。
- 只報有證據的發現，附 `路徑:行號` 或可重跑的指令。

## 交件格式
- Workflow 裡用 StructuredOutput 交，schema 會強制格式。
- 用 Agent 工具派出時，回報最後附一個 ```json 區塊，裡面是信封（`.claude/teams/schemas/envelope.schema.json`），payload 用 `findings`（`schemas/payloads/findings.schema.json`）。
- 輸出語言照 `.claude/teams/rules.md`。
```

- 要寫報告檔才給 `Write`，並提醒使用者守門會依第 3 步選的角色限制寫入範圍。
- 使用者選現成的 agent 時，不改它的檔案；只檢查它有沒有交件格式的說明，沒有就建議補上（使用者同意才改）。

## 5. 寫設定（先登記守門，再寫 profile）
順序不能反：先寫 profile 的話，中途失敗會留下一個已排進 workflow、卻沒有任何角色規則的 agent（和主 session 一樣能 push）。

0. **備份**：改之前把 `.claude/teams/guard.config.json`、`.claude/teams/methodology.json` 原內容存起來（例如複製到 run 目錄的 `add-agent.backup/`，或確認兩檔在 git 裡沒有未提交的改動，之後可用 `git checkout -- <檔>` 還原）。
1. **先登記守門**：`guard.config.json` 的 `extension_agents` 加一筆：`"acme-db-reviewer": "tpm-reviewer"`。
   沒登記的話 `awf.py` 會拒絕產生 workflow 腳本，因為沒登記的 agent 不套任何角色規則。寫完確認它仍是合法 JSON，並用第 6 步的 hook 指令確認這個 agentType 已經被擋 push，再往下。
   `auto_merge.enabled` 是 true 時：不寫檔，把這兩份改動整理成 diff 交給使用者走 PR，流程到此為止（第 6 步等 PR 合併後再跑）。
2. **再寫 profile**：`methodology.json` 的 `extensions` 加一筆：
   ```json
   {"name": "db-migration", "agent": "acme-db-reviewer", "stage": "test", "instructions": "檢查 migration 能不能回滾……"}
   ```
3. 不用另外內嵌：`awf.py next` 每次產生腳本時讀 profile。專案有 workflow 覆寫檔（`.claude/workflows/*.js`，init 加 `--with-workflows` 才有）時才要跑 `python3 .claude/teams/tools/build.py`。

**回滾**（第 1–3 步或第 6 步驗證任何一項失敗時）：
1. 先把 `methodology.json` 還原成備份（移除剛加的 extension），再還原 `guard.config.json`；順序和寫入相反，過程中不會出現「在 workflow 裡卻沒登記守門」的狀態。
2. 有 workflow 覆寫檔時重跑 `python3 .claude/teams/tools/build.py`，再跑 `build.py --check` 確認回到原狀；沒有覆寫檔不用做。
3. 第 4 步新建的 agent 定義檔問使用者要不要留著。
4. 把失敗的訊息與已還原的檔案回報給使用者。

## 6. 驗證
```bash
python3 .claude/teams/tools/awf.py render --all --out _tpm/wf-check   # 用 plugin 的範本＋專案設定產生 12 支腳本
node .claude/teams/tools/check_workflows.mjs --wf-dir _tpm/wf-check --agents-dir <AgentWorkFlow>/plugin/agents   # 確認擴充 agent 會被派出、定義檔存在
uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py              # methodology.json 合法
```
再用 hook 確認守門生效（應該 exit 2，被擋）：
```bash
echo '{"tool_name":"Bash","agent_type":"<agentType>","cwd":".","tool_input":{"command":"git push origin feature/x"}}' \
  | CLAUDE_PROJECT_DIR="$PWD" python3 <AgentWorkFlow>/plugin/hooks/awf_hook.py guard-shell; echo "exit=$?"
```
Windows（PowerShell）把 `CLAUDE_PROJECT_DIR=...` 換成先執行 `$env:CLAUDE_PROJECT_DIR = (Get-Location).Path`。

## 7. 回報
列出：加在哪一段、agentType、套用的角色、派工內容摘要、改了哪些檔、驗證結果。
說明怎麼移除：先刪掉 `methodology.json` 的 `extensions` 那一筆（有 workflow 覆寫檔時跑 `build.py`），再刪 `guard.config.json` 的 `extension_agents` 那一筆（和加入的順序相反，守門最後才拿掉）。
