# AgentWorkFlow

AgentWorkFlow 是一套 Claude Code 的 agent team 工作流程模版，可以套用到任何專案。它只規定**工作流程**：怎麼拆工、怎麼平行處理、在哪裡停下來讓人核准、用什麼格式交件、誰能合併 PR。開發規範（TDD、DDD、spec 驅動……）和慣例（code style、commit 格式、分支命名、PR 大小、文件格式）都由使用者自己定義。

- **AI-native SDLC**：開發需求依序走 plan → design → build → test → deliver → merge 六段。前四段之間有核准閘門；計劃太大會拆成子計劃平行規劃，沒有相依的子任務會平行實作與驗證，最後再合併。
- **可信任的合併 agent**：`dev-merge-steward` 逐條對照需求與驗證證據，一致而且確定性 gate 通過才合併；不一致就開新需求、退回 PR。這個功能預設關閉。
- **事件處理**：簡報、上游變動、會議、PR 審查、需求變動、決策落地、Demo 前檢查、合併後監看，各自平行處理後產出文件。
- **網站分析**：從技術架構、基礎設施、商業模式、效能、競品五個面向平行分析，再合成一份報告。
- **守門 hooks**：擋推送、寫上游、越權寫入，以及所有合併與核准路徑；每個 agent 交件時驗證 JSON 信封。
- **加入自己的 agent**：用 `/add-agent` 把團隊自己的 agent 放進 plan、design、test、deliver 任一段，和內建 agent 平行執行，守門規則一併套用。

所有文件、註解與 agent 提示都用繁體中文寫。agent 的輸出語言可以設定（`{{OUTPUT_LANGUAGE}}`，預設繁體中文）。工具都是 Python，在 macOS、Linux、Windows（PowerShell）上都能執行。

## 運作方式

AgentWorkFlow 分兩部分，透過兩條管道發佈：

| 部分 | 內容 | 怎麼進到專案 | 怎麼更新 |
|---|---|---|---|
| `plugin/` | agents、skills、slash commands、hooks | 安裝 Claude Code plugin，所有專案共用 | `/plugin marketplace update agentworkflow` |
| `scaffold/` | 硬規則、workflow、schema、開發規範 profile、工具、golden-set、settings、`CLAUDE.md` 範本 | `init.py` 複製進目標專案，同時換掉 `{{佔位符}}` | 重跑 `init.py --force` |

workflow 放在 `scaffold/` 而不放在 plugin 裡，因為 Workflow 工具只讀專案的 `.claude/workflows/`。

```
AgentWorkFlow/
├── .claude-plugin/marketplace.json   # marketplace（名稱 agentworkflow）
├── plugin/                           # plugin 本體（名稱 agent-workflow）
│   ├── agents/                       # dev-*、tpm-*、網站分析組，共 22 個
│   ├── skills/                       # tpm-team、init-workflow、add-agent、correct、team-debate、site-report 系列、design-system-template
│   ├── commands/                     # /sdlc、/event、/brief、/demo-check、/add-agent、/correct
│   └── hooks/                        # hooks.json＋awf_hook.py（guard-shell、guard-paths、envelope）
├── scaffold/                         # init.py 複製進目標專案的內容
│   ├── .claude/teams/                # rules.md、schemas/、methodologies/、tools/、golden-set/、guard.config.json.tmpl
│   ├── .claude/workflows/            # sdlc-*.js、event-*.js
│   ├── .github/workflows/            # agent-merge-gate.yml.tmpl（選用）
│   ├── CLAUDE.md.tmpl、.mcp.json.tmpl
│   └── _runs/                        # run 目錄說明
├── init.py                           # 套用模版（互動問答或讀設定檔）
├── LICENSE                           # MIT
├── workflow.config.example           # 佔位符的值
├── docs/                             # 說明文件
└── tests/                            # 去客戶化掃描、init 煙霧測試、全部測試入口
```

## 需求

- Claude Code
- Python 3.9 以上，只用標準庫。Windows 沒有 `python3` 指令時，改用 `py -3` 或 `python`。
- git；要開 PR 或合併的話另外需要 `gh`。
- Node.js 18 以上，只有乾跑驗證 workflow 時才需要。

## 安裝

**1. 安裝 plugin**（每台機器做一次）

在 Claude Code 裡執行：

```
/plugin marketplace add s96cax98leo/AgentWorkFlow
/plugin install agent-workflow@agentworkflow
```

repo 是公開的，不需要登入 GitHub。要從本機 clone 安裝的話，把 `s96cax98leo/AgentWorkFlow` 換成 clone 的路徑。裝好後用 `/plugin` 確認 agent、skill、command 都有載入。

**2. 在目標專案套用模版**（每個專案做一次）

```bash
git clone https://github.com/s96cax98leo/AgentWorkFlow.git ~/tools/AgentWorkFlow
python3 ~/tools/AgentWorkFlow/init.py --target ~/work/my-project
```

init 會先逐一詢問佔位符的值，再確認你的開發習慣：

- 開發規範 profile
- 測試要不要先行
- code style 的工具或指令
- commit 格式
- 分支前綴
- PR 大小上限
- 文件格式

每題都有預設值，直接按 Enter 就套用。也可以在 Claude Code 裡執行 `/init-workflow`，讓它在對話中問完再幫你跑 init。

要驗證產出，在目標專案根目錄執行：

```bash
python3 .claude/teams/tools/build.py --check
node .claude/teams/tools/check_workflows.mjs --agents-dir ~/tools/AgentWorkFlow/plugin/agents
python3 .claude/teams/tools/check_agents.py --plugin-dir ~/tools/AgentWorkFlow/plugin
python3 ~/tools/AgentWorkFlow/plugin/hooks/tests/run.py --project .
```

## 快速開始

```
/sdlc 在訂單列表加上「匯出 CSV」按鈕，匯出目前篩選後的結果
```

主 session 會先列出計畫，包括事件類型、workflow、參數、預計派幾個 agent、有哪些不可逆的動作。**你核准後才開始**，之後依序執行：

| 段 | workflow | 做什麼 | 結束時 |
|---|---|---|---|
| 1 plan | `sdlc-plan` | 4 個 researcher 平行調研，spec-author 寫 `intent.md`，skeptic 挑錯 | 閘門：核准 intent |
| 2 design | `sdlc-design` | 先出大綱判斷規模；太大就拆成子計劃，由多個 spec-author 平行規劃再彙整。產出 spec、plan、子任務與檔案所有權，skeptic 平行檢查一致性 | 閘門：核准 spec＋plan（要修改時走 `sdlc-design-revise`） |
| 3 build | `sdlc-build` | 依 profile 決定先寫測試或後補測試；子任務依相依分層，同一層各自開 worktree 平行實作，每層由 integrator 合併 | 中斷時交人 |
| 4 test | `sdlc-test` | verifier 逐條驗收並做反作弊檢查（有子計劃時分片平行驗證），視需要加設計與安全審查；失敗就修正，最多 2 輪，可平行修正 | 閘門：確認驗證結果 |
| 5 deliver | `sdlc-deliver` | 從四個面向審查 PR，skeptic 逐則反駁，寫 PR 說明草稿與執行紀錄 | 閘門：推送、開 PR 由你當下決定 |
| 6 merge | `sdlc-merge` | 啟用 auto_merge 時，steward 對照需求與證據，`merge_gate.py` 判定能不能合併 | 合併後跑 `event-post-merge` 監看 |

完整走一遍的範例見 [`docs/套用範例.md`](docs/套用範例.md)。

## 核心概念

**閘門**：plan、design、test、deliver 結束時，主 session 把重點列在對話裡，等決策者核准，核准用 `save_run.py approve` 記錄。全自動流程可以改用政策核准（`--by policy`），這時工具會讀該段的實際結果：驗證有沒通過的條目、反作弊有發現、有阻擋性的待決問題，或 workflow 中斷，就不能自動核准。

**交件信封**：每個 `dev-*`／`tpm-*` agent 都用 JSON 信封交件，欄位是 status、summary、artifacts、evidence、confidence、open_questions、derived_work、payload。成果寫成檔案，信封只放路徑。hook 會驗證信封，不合格就交不出去。

**開發規範 profile**：各段的做法和專案慣例寫在目標專案的 `.claude/teams/methodology.json`。內建兩份：`default`（spec 驅動、測試先行、Conventional Commits）與 `lightweight`（需求來自 issue、實作後才補測試、格式自由）。要換做法，改這個檔後跑 `build.py`。

**平行處理**：觸發條件寫在 profile 的 `parallelism`：
- `split_threshold`：預估子任務超過幾個就拆子計劃，預設 8。
- `max_sub_plans`：子計劃數量上限，預設 4。
- `shard_verification`：有子計劃時驗證要不要分片平行，預設開。

以下情況 workflow 會停下交人：子計劃範圍重疊、彙整後默默少了子任務、彙整時改了驗證結果。

**可信任的自動合併**（預設關閉）：
- `dev-merge-steward` 逐條對照需求與證據，證據要綁定 head SHA。
- `merge_gate.py` 以確定性規則複核對照表、required checks、測試數、風險等級、每日上限、白名單和凍結旗標，結果寫進稽核紀錄。
- 對照不一致時，steward 產出 followup 新需求並退回 PR。
- 沒有任何 agent 能核准 PR。
- 真正的信任邊界在伺服器端：GitHub ruleset、gate App 的 required check、CODEOWNERS。啟用前請照 [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) 的檢查清單完成設定。

**加入自己的 agent**：
- 在 Claude Code 裡跑 `/add-agent`，它會一步步問：要加在哪一段、套用哪個模版角色的守門規則、派工內容是什麼；需要的話幫你建立 agent 定義檔，最後驗證。
- 設定寫在兩個地方：`methodology.json` 的 `extensions`，以及 `guard.config.json` 的 `extension_agents`。
- 擴充 agent 和該段的內建 agent 平行執行，交 `findings`；它的 blocker／important 發現會擋住政策核准。
- 只能套用唯讀類角色，不能 push、不能對 GitHub 寫入、不能合併。
- build 段和 merge 段不開放擴充。
- 細節見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)〈把自己的 agent 加進 workflow〉。

**守門 hooks**：入口只有一個，`awf_hook.py`。
- `guard-shell` 同時涵蓋 Bash 與 PowerShell 工具，擋推送、寫上游、推受保護分支，以及所有合併與核准路徑：`gh pr merge`、`--approve`、`gh api`、GraphQL、curl、Invoke-RestMethod。
- `guard-paths` 擋越權寫入。
- `envelope` 驗證交件信封。
- 角色規則只套用在模版自己的 agent 上，你原本的 agent 不受影響。

## 指令與事件

| 你說的 | workflow 或 skill | 必要參數 |
|---|---|---|
| `/sdlc <需求>`、開發、實作 | `sdlc-plan` → `sdlc-design` → `sdlc-build` → `sdlc-test` → `sdlc-deliver` → `sdlc-merge` | repo、epic、requirement、base、run_dir |
| `/brief <簡報>`、照任務單做 | `event-brief`（程式子任務轉進 SDLC） | brief、run_dir |
| 上游有新 PR、main 前進、CI 紅了 | `event-upstream` | what、repo、run_dir |
| 整理會議、逐字稿 | `event-meeting` | source、run_dir |
| 審 PR、審分支 | `event-pr-review` | target、repo、run_dir |
| 需求更新了 | `event-reqs-change` | change、run_dir |
| 某某決定了、定案 | `event-decision` | decision、run_dir |
| `/demo-check`、demo 前、彩排 | `event-demo-check` | target、run_dir |
| 合併這個 PR | `sdlc-merge` | repo、pr、run_dir |
| 合併後 main 紅了 | `event-post-merge` | repo、run_dir |
| 合併後歸檔 OpenSpec change（openspec profile） | `event-spec-archive` | repo、run_dir、change、merge、post_merge |
| `/add-agent`、加我自己的 agent | `add-agent` skill | — |
| `/correct`、把這個錯誤變成做不到 | `correct` skill | 錯誤描述或出處 |
| 比較方案、A 還是 B | `team-debate` skill | — |
| 分析某個網站 | `site-report` skill | 網址 |

`/event <描述>` 會自動判斷事件類型。事件判斷與閘門的完整規則寫在 `tpm-team` skill。

## 角色

| 組 | 角色 | 職責 |
|---|---|---|
| SDLC | `dev-spec-author` | 寫 intent；出大綱、規劃或彙整子計劃；寫 spec、plan，拆子任務與檔案所有權 |
| | `dev-acceptance-tester` | 依驗收條件寫測試（測試先行或後補由 profile 決定），只寫測試 |
| | `dev-implementer` | 在自己的 worktree 實作一個子任務，只改自己擁有的檔案 |
| | `dev-integrator` | 依相依順序把子任務分支合進整合分支，衝突經中轉分支處理並分類 |
| | `dev-verifier` | 在目前 HEAD 逐條驗證並做反作弊檢查，只回報 |
| | `dev-design-reviewer` | 有畫面改動時，用瀏覽器實測設計與無障礙，只審不改 |
| | `dev-security-reviewer` | 有安全面時，做 threat model、權限、secrets、供應鏈審查，只審不改 |
| | `dev-pr-reviewer` | 從規範、spec、bug、歷史四個面向審整條分支，只報信心 ≥0.8 的發現 |
| | `dev-merge-steward` | 可信任的合併 agent：對照需求與證據，gate 通過才合併，不一致就退回 PR；永遠不核准 |
| | `dev-release-watcher` | 合併後監看：歸因失敗，提出 revert 與凍結建議，只讀 |
| 事件 | `tpm-planner` | 把事件拆成子任務與相依圖，每個節點是一張自足的派工單 |
| | `tpm-researcher` | 針對一個具體問題調研，結論附 `路徑:行號` 或指令 |
| | `tpm-upstream-analyst` | 唯讀分析上游：預測衝突、影響範圍、與我方分支的交集 |
| | `tpm-reviewer` | 依指定面向審查 PR 或分支，只留建議 |
| | `tpm-skeptic` | 專門反駁別人的發現，不確定時判反駁成立 |
| | `tpm-writer` | 把定稿的素材寫成文件，寫完跑 `check_rules.py` |
| 網站分析 | `tech-architect`、`infra-analyst`、`biz-analyst`、`perf-auditor`、`competitor-scout`、`lead-synthesizer` | 分別負責技術架構、基礎設施、商業模式、效能、競品，最後由 lead-synthesizer 交叉驗證、合成總報告 |

## init 的選項

```
python3 init.py --target <專案> [--config workflow.config] [--answers <答案.json>] [--defaults]
                [--hooks plugin|project] [--python <路徑>] [--with-mcp] [--with-merge-gate]
                [--force] [--overwrite-user-files]
```

| 選項 | 說明 |
|---|---|
| `--config` | 從設定檔讀佔位符（`cp workflow.config.example workflow.config` 後修改），不給就互動詢問 |
| `--answers` | 開發規範與慣例的答案檔（JSON），不用互動，適合 CI；格式見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md) |
| `--defaults` | 全部用預設值，使用 default profile |
| `--hooks plugin` | 預設。hooks 由 plugin 註冊 |
| `--hooks project` | 把 hooks 複製到專案的 `.claude/hooks/`，由 `settings.json` 註冊，並寫入 Python 的絕對路徑。**Windows 建議用這個。** 和 plugin 註冊只能擇一 |
| `--with-mcp` | 產生正式的 `.mcp.json`，不加時只產生 `.mcp.json.example` |
| `--with-merge-gate` | 放 `.github/workflows/agent-merge-gate.yml`。預設不放，因為還沒設好 gate App 時它會讓每個 PR 失敗 |
| `--force` | 覆寫已存在的模版檔，用來更新已經 init 過的專案；使用者的檔案仍然走合併 |
| `--overwrite-user-files` | 連使用者的檔案也整份覆寫，只在想重設時使用 |

init 會產生這些檔案：
- `.claude/teams/`，包含 `guard.config.json`、`methodology.json`
- `.claude/workflows/`
- `.claude/settings.json`
- `CLAUDE.md`
- `.mcp.json.example`
- run 目錄

### 佔位符

| 佔位符 | 意思 | 預設 |
|---|---|---|
| `{{PROJECT_NAME}}`、`{{CLIENT_NAME}}` | 專案名稱、客戶泛稱 | `my-project`、`示範客戶` |
| `{{WORKSPACE_DIR}}`、`{{REPOS_DIR}}` | 工作區根目錄、我方 clone 所在的目錄 | `--target`、`repos` |
| `{{UPSTREAM_REPO}}`、`{{FORK_REPO}}` | 上游（不可寫入）、我方 fork（推送目標） | — |
| `{{FORBIDDEN_REPOS}}` | 絕對不能碰的目錄或 repo | — |
| `{{OUTPUT_LANGUAGE}}` | agent 的輸出語言 | 繁體中文 |
| `{{TASK_ID_PREFIX}}`、`{{TASK_AUTHORITY}}` | 任務編號前綴、負責發編號的上層系統 | `TASK-`、任務管理系統 |
| `{{DECISION_OWNER}}`、`{{STAKEHOLDER}}` | 拍板的人、需求方 | 使用者、需求方 |
| `{{BRANCH_PREFIX}}`、`{{ALLOWED_BRANCH_PREFIXES}}` | 開發分支前綴、允許的新分支前綴 | `feature/`；`feature/,bugfix/,hotfix/,release/,merge/` |
| `{{RUNS_DIR}}`、`{{BRIEFS_DIR}}` | run 目錄、簡報目錄 | `_tpm/runs`、`_tpm/briefs` |
| `{{ATTRIBUTION_LINE}}` | commit 結尾的署名行 | `Assisted-by: Claude` |
| `{{DESIGN_SYSTEM_NAME}}` | 設計系統名稱 | `my-design-system` |
| `{{AGENT_NS}}` | 呼叫 agent 時的命名空間前綴 | `agent-workflow:` |
| `{{METHODOLOGY}}` | 開發規範 profile：內建名稱或自己的 JSON 路徑 | `default` |
| `{{AWF_HOOK_CMD}}`、`{{AWF_HOOK_SHELL}}` | `--hooks project` 的啟動指令與 shell，由 init 自動計算 | — |

### 套用到既有專案

init 對既有專案只新增檔案，你原本的 agent、skill、command 和設定都能照常使用：

- 不動 `.claude/agents`、`skills`、`commands`。
- 已有的 `CLAUDE.md` 不覆寫，只在結尾追加一行 `@.claude/teams/agent-workflow.CLAUDE.md`。
- `settings.json` 和 `.mcp.json` 深度合併，你的值優先。
- 角色規則只套用在帶 `agent-workflow:` 命名空間的 agent，以及和模版角色完全同名的 agent。

主 session 的限制由 `guard.config.json` 的兩個開關控制：`main_session_git_rules`、`main_session_github_rules`。不管開關怎麼設，上游與禁止的 repo 一律擋；`auto_merge` 啟用時，主 session 也一律不能合併或核准。詳見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)〈套用到既有專案〉。

## 文件

| 文件 | 內容 |
|---|---|
| [`docs/開發模式總覽.md`](docs/開發模式總覽.md) | 整套工作方式總覽：角色、閘門、派工、信封、手機決策 |
| [`docs/ai-native-sdlc.md`](docs/ai-native-sdlc.md) | SDLC 各段的 agent、輸入輸出、閘門、worktree 與整合分支 |
| [`docs/事件處理.md`](docs/事件處理.md) | event-* workflow、方案辯論、網站分析 |
| [`docs/agent-協作規則.md`](docs/agent-協作規則.md) | 派工單範本、信封格式、背景 agent、驗收證據 |
| [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) | 自動合併的防線分層、啟用前檢查清單、revert 演練 |
| [`docs/adr-agent-auto-merge.md`](docs/adr-agent-auto-merge.md) | 自動合併的架構決策紀錄 |
| [`docs/自訂與擴充.md`](docs/自訂與擴充.md) | 開發規範 profile、平行參數、把自己的 agent 加進 workflow、新增 agent／skill／hook／workflow、套用到既有專案 |
| [`docs/踩坑手冊.md`](docs/踩坑手冊.md) | 實戰教訓：症狀、原因、做法 |
| [`docs/參考-pstack.md`](docs/參考-pstack.md) | 參考過的外部 plugin（pstack）：差異、引進與沒引進的做法 |
| [`docs/套用範例.md`](docs/套用範例.md) | 用虛構的「示範客戶 ACME」從 init 走到 deliver |

## 開發這個模版

```bash
python3 tests/run_all.py              # 全部：去客戶化掃描 → init 煙霧測試 → hook 測試 → check-tokens
python3 tests/scrub_check.py [路徑…]   # 只跑去客戶化掃描
python3 tests/smoke_init.py           # 只跑 init 煙霧測試；KEEP=1 保留暫存目錄
python3 plugin/hooks/tests/run.py     # 只跑 hook 測試（Bash 與 PowerShell 案例）
```

CI（`.github/workflows/ci.yml`）在 ubuntu、macos、windows（pwsh）三個平台都跑 `tests/run_all.py`。

去客戶化掃描會讀兩份清單：
- `tests/scrub_terms.txt`：公開的通用規則，包括本機路徑、憑證格式、個人信箱。
- `tests/scrub_terms.local.txt`：要擋的真實名稱，不進版控，只放在自己電腦上。

範例一律用虛構的「示範客戶 ACME」。

給 Claude Code 的開發規則寫在 [`CLAUDE.md`](CLAUDE.md)。

## 授權

MIT，見 [`LICENSE`](LICENSE)。
