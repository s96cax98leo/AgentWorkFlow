# {{PROJECT_NAME}} agent team 硬規則

所有 `tpm-*`、`dev-*` agent 與主 session 都遵守。能機械檢查的由 agent-workflow 的 hooks（設定：`.claude/teams/guard.config.json`）與 `.claude/teams/tools/check_rules.py` 強制。

本檔分兩部分：
- **通用規則**：模版內建，適用任何專案；要放寬某一條，請在「專案規則」寫明例外與核准人，不要直接刪。
- **專案規則**：init 後由專案填寫（目標 repo、禁區、審查規則、設計系統、verify 指令）。

---

## 通用規則

### 寫入與權限
1. **不寫入上游（{{UPSTREAM_REPO}}）**：不 push、不開 PR、不留言、不改狀態。只有{{DECISION_OWNER}}在當下明確同意的那一次例外，並由{{DECISION_OWNER}}自己執行。由 `awf_hook.py guard-shell` 擋（`upstream_repos`；Bash 與 PowerShell 都擋）。
2. **推到我方 fork（{{FORK_REPO}}）依路徑分兩種，agent 一律不 push。** (a) 一般情況：{{DECISION_OWNER}}當下同意後由主 session 推送。(b) 無人值守：主 session 只能經由確定性工具 `python3 .claude/teams/tools/publish.py push`（只推 fork 的功能分支、推送前掃機敏值、依 run.json 的閘門與政策判定、寫 `publish-audit.jsonl`）推送並開 PR；核准依據是該 run 已核准的 deliver 段，核准人記在 run.json。兩種情況主 session 都不直接推到 `guard.config.json` 的 `protected_branches`（預設 main、master），改推功能分支再開 PR。gh 寫入動作要明確寫 `--repo {{FORK_REPO}}`；不用 curl、Invoke-RestMethod、Invoke-WebRequest 直接對 GitHub API 寫入。由 `awf_hook.py guard-shell` 擋（agent 執行 publish.py 也擋）；伺服器端由 ruleset 禁止直接推 main。
3. **不可逆節點一律停下**：寫上游、刪資料、對外寄送、force push，都要{{DECISION_OWNER}}當下同意；先前的同意不延伸到下一次。push 與開 PR 依規則 2；合併依規則 6。無人值守時遇到需要人同意的節點，改成 park（記錄、推播，不等待）。
4. **禁止路徑只能讀**：`guard.config.json` 的 `forbidden_paths`（{{FORBIDDEN_REPOS}}）不得寫入、移動、刪除。由 `awf_hook.py` 的 `guard-paths`、`guard-shell` 擋。
5. **機敏檔不讀、不印**：`.env`、金鑰、憑證不讀值、不印出、不提交，也不在本機搜尋或借用別處的 API key、token。要判斷有無只能用 `grep -q`；push 前用 `grep -qF -f <機敏值清單> <diff>` 比對 diff 是否含機敏值，只報「有／沒有」。
6. **核准與合併 PR 依風險分級**：(a) **核准**：agent 與主 session 一律不核准（`gh pr review --approve`、API 送 APPROVE 都算），沒有例外。(b) **自動合併**：只有 `guard.config.json` 的 `auto_merge.enabled` 為 true 時，`auto_merge.merger_role`（預設 `agent-workflow:dev-merge-steward`，比對含命名空間的完整 agent_type）才能對 `auto_merge.repos` 白名單內、風險等級不超過上限的 PR 執行 `gh pr merge --match-head-commit <sha>`，而且 `merge_gate.py` 對同一個 head SHA 的最後判定必須是 merge、沒有凍結旗標（`auto_merge.freeze_file`）、上游 repo 要有 `upstream_authorized_by` 授權人。auto_merge 的設定變更只走 PR＋CODEOWNERS，由人核准。(c) **其餘一律由人合併**：auto_merge 未啟用、gate 判 hold／escalate、高風險路徑（CI、`.github/`、`.claude/`、hooks、依賴、migration、release、auth、CODEOWNERS）、上游未授權。主 session 一樣不合併；`gh api` 或 curl 打 merge 端點也算。由 `awf_hook.py guard-shell` 擋（Bash 與 PowerShell 都擋）；hook 只是輔助，真正的邊界是 GitHub ruleset 與綁定 gate App 的 required check，見 `docs/自動合併信任模型.md`（AgentWorkFlow 模版）。
7. 改目標 repo 時，那個 repo 自己的規範（CLAUDE.md、分支規範、REVIEW.md、hooks）照樣適用；hooks 會轉呼叫 repo 自己的 `guard-git`、`guard-paths`（`.py` 或 bash 腳本）。

### 內容
8. **不寫人天、人月、工時估算。** 費用只列項目與官方定價出處。
9. **不自編任務編號**：任務編號（`{{TASK_ID_PREFIX}}…`）只由 {{TASK_AUTHORITY}} 發。衍生工作寫進信封的 `derived_work` 或執行紀錄的「衍生工作（不編號，交 {{TASK_AUTHORITY}} 編）」。
10. **每個數字和結論都要能回查**：附 `路徑:行號` 或可重跑的指令；查不到就寫「未能確認」。
11. **不替{{DECISION_OWNER}}或{{STAKEHOLDER}}做決定**：列選項與代價，可以標建議並附理由。需要決策時用選項式提問，並推播到{{DECISION_OWNER}}的手機（PushNotification）。
12. **給決策者或對外的文件**：不寫內部編號，不評價人，技術名詞保留原文。`check_rules.py --audience owner|external` 檢查。
13. **客戶資料邊界**：不同客戶的目錄、資料、文件不得交叉引用。
14. 抓外部網站前先看 robots.txt；不做任何會改變對方狀態的動作。
15. **輸出語言（強約束）**：所有輸出一律使用 **{{OUTPUT_LANGUAGE}}**，包括給使用者的回覆、agent 之間的訊息與派工單、SendMessage、信封的 summary／open_questions／derived_work、文件、commit 訊息內文、推播通知。程式碼識別字、指令、路徑、技術專有名詞保留原文。

### 流程
16. **先 plan、後動手**：開發走 AI-native SDLC：intent → design → spec → plan →（測試與實作，先後依開發規範 profile）→ verifier →（設計／安全審查）→ PR 審查 → 人核准。各段做法（產出文件、需求寫法、測試時機、驗證方式）與專案慣例（code style、commit、分支命名、PR 大小、文件格式）由開發規範 profile（`.claude/teams/methodology.json`，init 時依使用者的習慣產生）決定，模版只管流程與閘門。plan、design、test、deliver 四個閘門預設都要{{DECISION_OWNER}}核准才進下一段（plan 閘門一律由人核准，可批次、非同步；design、test、deliver 在專案政策允許時可用政策核准）；之後的 merge 段不設人工閘門，由確定性的 `merge_gate.py` 判定，判定不是 merge 就交{{DECISION_OWNER}}；核准用 `save_run.py approve` 記錄，並在對話裡列重點，不只依賴工具狀態。全自動流程用 `--by policy` 時，工具會讀該段的實際結果：verifier 有不是 pass 的條目、反作弊有發現、有 blocking 待決問題或 blocker／important 發現、workflow 回報 halted，都不能政策核准，要交{{DECISION_OWNER}}。
17. **決策者的回覆照字面讀**：「調整」不是「核准」；附帶意見要先處理再請求核准。
18. **commit**：格式照開發規範 profile 的 `conventions.commit`（預設 profile 是 Conventional Commits、scope 放 Story ID）；結尾署名行 `{{ATTRIBUTION_LINE}}`。`check_rules.py --git` 檢查署名行；profile 有設 `check_regex` 時也檢查標題格式。
19. **分支**：新分支只用 `guard.config.json` 的 `allowed_branch_prefixes`（{{ALLOWED_BRANCH_PREFIXES}}）；子任務分支 `{{BRANCH_PREFIX}}<epic>--<sub>`，各開 worktree，檔案所有權不重疊；整合用整合分支，依相依順序合併。不 force push、不 `--no-verify`。建議的分支名不合規時改用允許的前綴並註明。
20. **測試失敗就改程式，不改測試**；不加 skip、fixme、retry，不放寬斷言。上游測試不改；例外要{{DECISION_OWNER}}核准，並記錄在 spec 的「整合修訂與例外」。
21. **改寫 commit 歷史前留備份分支**；改寫後更新文件中引用的 SHA。

### 交件
22. **每個 agent 都以 JSON 信封交件**（`.claude/teams/schemas/envelope.schema.json`）：
    - Workflow 裡由 `schema` 強制。
    - 用 Agent 工具派出時，回報最後要附一個 ```json 區塊，`awf_hook.py envelope` 會驗證。
23. **成果一律寫成檔案，信封只放路徑。** 有 blocking 問題時 `status` 不能是 `done`；blocking 問題要附 2–3 個選項與代價。
24. **審查者只讀，只看證據，不採信作者的自述。** 慣例類的審查標準（PR 大小、code style、commit 格式、分支命名）引用開發規範 profile 的 `conventions`，profile 設為 null 的項目不檢查。 退回最多 2 次，超過交給人；無人值守時改為 park（寫進 run.json 並推播{{DECISION_OWNER}}，不等待）。合併通道裡不確定就 hold：agent 的發現只能讓結果變嚴，skeptic 不得推翻 blocker／important。
25. **派工單自足**：目標、依據檔、硬規則、驗收、回覆格式都寫在派工單裡，使用輸出語言。

---

## 專案規則

> init 後由專案填寫。每一項都寫「是什麼、出處、誰核准」。沒有的項目寫「無」，不要留空。

- **目標 repo**：〈例：`{{REPOS_DIR}}/<repo>`，主線分支 `main`，上游 {{UPSTREAM_REPO}}，我方 fork {{FORK_REPO}}〉
- **禁區**：〈例：{{FORBIDDEN_REPOS}}；只能讀、不得寫入的目錄或 repo，與原因〉
- **需求來源**：〈例：簡報目錄 {{BRIEFS_DIR}}、User Story 來源、需求快照位置；一律唯讀〉
- **backlog 位置**：〈例：`_tpm/tasks/backlog.md`，格式 `id:{{TASK_ID_PREFIX}}0001`；對應 guard.config.json 的 `backlog_file`〉
- **審查規則**：〈例：目標 repo 的 REVIEW.md 分級、必審的 lens；PR 大小上限、code style、commit 格式寫在 `.claude/teams/methodology.json` 的 conventions，不在這裡重寫（預設 profile：修改既有檔 300 行或 15 檔）〉
- **設計系統 skill**：〈例：`{{DESIGN_SYSTEM_NAME}}`（由 design-system-template 建立的 skill 名稱）；有畫面改動時 dev-design-reviewer 預載〉
- **verify 指令**：〈例：`just verify --base origin/main`，或 `pnpm test && pytest`；integrator 合併後與 implementer commit 前都跑〉
- **demo 環境**：〈例：compose 專案名（指令一律帶 `-p`）、測試用獨立 TMPDIR 與 `LC_ALL=en_US.UTF-8`〉
- **其他專案特定約定**：〈例：既有定案不推翻的事項、只能由使用者自己執行的指令〉
