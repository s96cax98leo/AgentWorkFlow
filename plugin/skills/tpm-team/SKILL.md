---
name: tpm-team
description: TPM agent team 的入口。開發需求一律走 AI-native SDLC（plan → design → build → test → deliver → merge，段與段之間由人核准；merge 由可信任的合併 agent 對照需求後執行）；其他事件平行處理後產出文件。使用者說「開發這個需求」「照簡報做某個任務編號」「跑簡報」「上游有新 PR／main 動了／CI 紅了」「整理這場會議」「審這個 PR／分支」「合併這個 PR」「合併後 main 紅了」「需求更新了」「決策者決定了 X」「demo 前檢查」「用 agent team 處理」時使用。
argument-hint: <事件描述，例如「開發配額告警需求」「跑簡報 <任務編號>」「審 feature/x」>
---

# TPM agent team 入口

說明與角色見 `.claude/teams/README.md`，硬規則見 `.claude/teams/rules.md`。這個 session 就是執行者（主 session），由它派工、彙整、過閘門。

**佔位符**：本 skill 的 `{{…}}` 是專案設定值，實際值以專案 `.claude/teams/rules.md` 的〈專案規則〉段與 `.claude/teams/guard.config.json`（hooks 讀的機器可讀版）為準，兩者都由 `init.py` 依 `workflow.config` 產生。讀不到〈專案規則〉時先停下，請使用者跑 `/init-workflow`。

| 佔位符 | 意思 | 預設 |
|---|---|---|
| `{{REPOS_DIR}}` | 我方 clone 所在目錄 | — |
| `{{FORK_REPO}}` | 我方 fork（推送目標） | — |
| `{{UPSTREAM_REPO}}` | 上游（不可寫入） | — |
| `{{FORBIDDEN_REPOS}}` | 絕對不能碰的目錄或 repo | — |
| `{{RUNS_DIR}}` | run 紀錄目錄 | `_tpm/runs` |
| `{{BRIEFS_DIR}}` | 簡報目錄 | `_tpm/briefs` |
| `{{BRANCH_PREFIX}}` | 整合分支前綴 | `feature/` |
| `{{TASK_ID_PREFIX}}`／`{{TASK_AUTHORITY}}` | 任務編號前綴／給編號的上層系統 | — |
| `{{DECISION_OWNER}}` | 拍板的人 | 使用者 |
| `{{OUTPUT_LANGUAGE}}` | 輸出語言 | 繁體中文 |
| `{{AGENT_NS}}` | plugin agent 的命名空間前綴 | `agent-workflow:` |

用 Agent 工具派 plugin 的角色時，`subagent_type` 帶前綴，例如 `{{AGENT_NS}}tpm-planner`。

## 1. 判斷事件，先列計畫給使用者看

| 使用者說的 | 事件 | workflow | 必要 args |
|---|---|---|---|
| 開發、實作、照 Story／需求做 | 需求開發（SDLC） | `sdlc-plan` → `sdlc-design` → `sdlc-build` → `sdlc-test` → `sdlc-deliver` → `sdlc-merge` | repo、epic、requirement、base、run_dir |
| 跑簡報、照 `{{TASK_ID_PREFIX}}xxxx` 做 | 簡報 | `event-brief`（裡面的程式子任務再走 SDLC） | brief、run_dir |
| 上游、main 前進、CI 失敗 | 上游變動 | `event-upstream` | what、repo、run_dir |
| 會議、逐字稿 | 會議 | `event-meeting` | source、run_dir |
| 審 PR、審分支 | PR 審查 | `event-pr-review` | target、repo、run_dir（可加 lenses） |
| 需求更新、Story 改了 | 需求變動 | `event-reqs-change` | change、run_dir（可加 repo） |
| 決策者決定了、定案 | 決策落地 | `event-decision` | decision、run_dir |
| demo 前、彩排 | Demo 前檢查 | `event-demo-check` | target、run_dir（可加 script、date） |
| 要比較 ≥3 個方案、要給決策者選 | 方案辯論 | `team-debate` skill | — |
| design 段被退回要改 | 設計修訂 | `sdlc-design-revise` | repo、epic、run_dir、decisions、issues |
| 合併這個 PR、PR 開好了要合 | 合併（SDLC 合併段） | `sdlc-merge` | repo、pr、run_dir（可加 gh_repo、base、epic、evidence、requirements、findings、tests_before、tests_after） |
| 合併後、main 紅了是不是合併造成的 | 合併後監看 | `event-post-merge` | repo、run_dir（可加 gh_repo、merge_sha、since） |

- 判斷不出來、或 args 缺漏時，用 AskUserQuestion 問（選項式），不要猜。
- **先列計畫給使用者看**：事件、workflow、args、預計開幾個 agent、不可逆的動作，經使用者核准才執行。這是硬規則「先 plan、後動手」。
- `run_dir`：`{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>`。日期由主 session 填，workflow 腳本裡不能取時間。
- 簡報檔在 `{{BRIEFS_DIR}}/`；任務編號照簡報原文，**不自編編號**（編號由 `{{TASK_AUTHORITY}}` 給）。
- 目標 repo 只能是 `{{REPOS_DIR}}/` 下我方的 clone（例如 `{{FORK_REPO}}` 的本機 clone）；`{{FORBIDDEN_REPOS}}` 一律拒絕。
- 客戶資料邊界：不同客戶的目錄不得交叉引用。

## 2. 執行

```bash
python3 .claude/teams/tools/save_run.py init <run_dir> --event <sdlc|brief|upstream|meeting|pr-review|reqs-change|decision|demo-check|debate|merge|post-merge> --target <repo> --requirement "<描述>"
```

1. 用 `Workflow({name: "<workflow>", args: {...}})` 執行。args 傳 JSON 物件，不要傳字串。
   - **例外**：涉及授權判斷的敏感流程（例如要 agent 執行它可能拒做的操作），改用 Agent 工具逐段派，不要用 Workflow 一次轉發；Workflow 會把使用者訊息當授權轉給 subagent，容易造成誤解或拒做。
2. 回傳的 JSON 寫成檔案存起來：
   ```bash
   python3 .claude/teams/tools/save_run.py stage <run_dir> <stage> < <檔>
   ```
   它會逐一驗證信封，並更新 `run.json`。
3. 把結果整理給使用者：
   - 每個 agent 的 status 與 summary。
   - **所有 `open_questions`**：blocking 的用 AskUserQuestion 問，選項照信封裡的 options；需要決策者拍板時用 PushNotification 推播到手機。
   - skeptic 推翻了哪些發現。
   - `derived_work`：只列出來，不編號，交 `{{TASK_AUTHORITY}}`。
4. 派工單、SendMessage、agent 間訊息一律用 `{{OUTPUT_LANGUAGE}}`。

## 3. SDLC 的閘門（不可略過）

| 段 | 結束後 | 下一段的條件 |
|---|---|---|
| plan | 把 intent.md 的重點與 skeptic 的質疑列給使用者 | 使用者核准 intent。要送上游的需求，intent 要 `{{DECISION_OWNER}}` 核准，並記錄由誰核准 |
| design | 把 spec 的驗收條件、子任務與相依圖、analyze 的發現列給使用者；有拆子計劃時一併列出子計劃（範圍、相依）與拆的理由 | 使用者核准 spec＋plan（退回時走 `sdlc-design-revise`） |
| build | 沒有閘門，直接進 test。halted 時停下交人 | — |
| test | 列出 pass／fail／not_verifiable 與審查發現 | 使用者確認結果（fail 或 not_verifiable 要人決定） |
| deliver | 列出審查發現、PR 說明草稿、執行紀錄 | **推 fork、開 PR 要使用者當下同意**；寫入上游要另外再同意一次；agent 不核准 |
| merge | 列出需求對照表、skeptic 判出的不一致、gate 理由；未合併時列出新需求文件（`<run_dir>/followups/`） | 進入 merge 段要使用者同意（guard.config.json 的 `auto_merge` 要已啟用）。只有 `dev-merge-steward` 能合併，而且要 `merge_gate.py` 判 merge；不一致時 PR 已退回，新需求走 `event-reqs-change`（需求要改）或回到 SDLC 對應段落（實作要補），修好後重跑 `sdlc-merge`。合併後跑 `event-post-merge` |

- 閘門重點一定要**在對話裡列出來**，不能只依賴工具狀態；工具可能把計畫記成「已核准」但決策者沒看到。
- 決策者的回覆照字面讀：「調整」「可以但…」不是核准，附帶的意見要先處理。

核准時記錄：

```bash
python3 .claude/teams/tools/save_run.py approve <run_dir> <stage> --by <誰> --at <ISO 時間>
```

`sdlc-merge` 的回傳：`merged`、`decision`、`followups`、`next`。`event-post-merge` 回傳 `freeze: true` 時，用 PushNotification 推播決策者（附 revert 提案路徑）；revert PR 由人決定、由人合併，凍結期間不跑合併。

SDLC 各段的做法（需求寫法、測試先行與否、驗證方式）與慣例（code style、commit、分支、PR 大小、文件格式）由開發規範 profile `.claude/teams/methodology.json` 決定，已由 `build.py` 內嵌進 `sdlc-*.js`；列計畫時寫出用的是哪個 profile，使用者要改做法就改這個檔再跑 `python3 .claude/teams/tools/build.py`，不要在派工時另外指定。`sdlc-build` 回傳的 `test_first` 表示這次走 Red（測試先行）還是 Tests（實作後補驗證測試）。

`sdlc-build` 的 args 要加 `subtasks`（`design` 回傳的 `design.payload.subtasks`）。`sdlc-test` 的 args 要加 `has_ui`、`has_security`，依 spec 判斷。`design` 回傳的 `sub_plans` 不是空的時，`sdlc-test` 與 `sdlc-design-revise` 的 args 都要帶 `sub_plans`。

**平行處理**：計劃太大時 `sdlc-design` 會拆成子計劃，多個 dev-spec-author 平行規劃再彙整；`sdlc-build` 依相依分層，同一層沒有相依的子任務各自 worktree 平行實作，再由 integrator 合併；`sdlc-test` 有子計劃時每個子計劃一個 verifier 平行驗證，多個子計劃都有失敗時平行修正再合併。門檻與上限在 profile 的 `parallelism`（`split_threshold`、`max_sub_plans`、`shard_verification`）。workflow 回傳 `halted: true`（例如子計劃範圍重疊、彙整掉了子任務、彙整竄改驗證結果）時停下交人。

## 4. 收尾
- **推送前檢查機敏值**：`git diff <base>...HEAD | grep -qF -f <機敏值清單>`，只報「有／沒有」，不印出內容；`.env` 等機敏檔不讀、不印。
- **推送**：使用者同意後，由主 session 執行 `git -C <repo> push origin {{BRANCH_PREFIX}}<epic>`。agent 不能推，hook 會擋。
- **worktree**：epic worktree 在 `<repo>/.worktrees/<epic>`，等使用者決定合併或放棄後再移除。
- **commit 署名**：結尾一行 `{{ATTRIBUTION_LINE}}`（預設 `Assisted-by: Claude`）。
- **工具副作用**：索引或程式圖工具若改了 `.gitignore`、多出自己的快取目錄，不提交，先還原。
- **plan mode**：主 session 要進 plan mode 前，先讓背景 agent 收尾；plan mode 下背景 subagent 也只能唯讀，收尾會卡住。
