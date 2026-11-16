---
name: init-workflow
description: 在新專案套用 AgentWorkFlow：先跑 awf init --detect 看專案現況，逐主題和使用者確認做法（開發規範、慣例、需求來源、模型分級、CI、去客戶化、連結、自動合併等），整理成答案檔，再跑 awf init --answers 並唸出對照表。已套用的專案改設定用 --reconfigure。使用者說「套用 AgentWorkFlow」「初始化 agent workflow」「init workflow」「設定開發規範」時使用。
argument-hint: <目標專案路徑（可省略＝目前目錄）>
---

# 套用 AgentWorkFlow（init-workflow）

模版只管工作流程（plan → design → build → sync → test → deliver → merge 與閘門）；開發規範與慣例由使用者決定，寫進專案的開發規範 profile（`.claude/teams/methodology.json`）。所以**先偵測、再逐主題確認、最後才寫設定**，不要直接套預設值，也不要把偵測到的東西當成使用者的決定。

使用者給的目標路徑：$ARGUMENTS（沒給就用目前工作目錄）。

每一步先說明要做什麼，等使用者同意再執行。提問用選項式（AskUserQuestion），每題附預設值，使用者可以直接接受。

**先用預設值套用（零設定起步）**：使用者說「先用預設值」「之後再調」時，不問第 3、4 步（之後可以用 `awf init --reconfigure` 補問）。做完第 1、2 步後，把會做的事列給使用者（新增 `.claude/teams/`、`CLAUDE.md` 追加一行引用、settings 深度合併；開發規範用 default profile；repo 與目錄用預設值），同意後執行 `awf init --target <目標專案> --defaults`，再做最後的驗證步驟。提醒使用者：`UPSTREAM_REPO`、`FORK_REPO` 等值是空的，推送與合併之前要補（改 `workflow.config` 後跑 `awf init --upgrade`），開發規範之後可以改 `.claude/teams/methodology.json`。

## 1. 確認位置
模版就是 plugin 目錄本身（底下有 `scaffold/`、`workflow-templates/`、`workflow.config.example`），`awf init` 會自己找到執行檔所在的 plugin，從 marketplace 安裝後不用另外 clone，也不用指定位置。下面幾步要讀模版裡的檔時，用這個 skill 所在的 plugin 目錄（`${CLAUDE_PLUGIN_ROOT}`，也就是這份 SKILL.md 往上兩層）。

只有兩種情況要指定：

1. 使用者要用自己 clone 的模版 repo（開發中的版本）：加 `--source <位置>`，給 repo 根目錄或它的 `plugin/` 都可以；也可以設環境變數 `AWF_TEMPLATE_DIR`。
2. `awf init` 回報「找不到模版」：plugin 沒裝好。請使用者跑 `/agent-workflow:doctor`，或重新安裝 plugin。

讀 plugin 目錄的 `.claude-plugin/plugin.json` 的 `version`，告訴使用者要套用的模版版本。

## 2. 檢查目標專案
目標專案已經有 `.claude/teams/` 時，它已 init 過：讀 `.claude/teams/VERSION`（沒有＝0.2.0 以前的版本），和第 1 步的模版版本比較，建議改走升級（第 5 步的 `--upgrade`），不要重新問一輪設定。

列出目標專案已存在的 `.claude/teams/`（含 `methodology.json`）、`.claude/workflows/`（0.4 以前的副本或使用者的覆寫檔）、`.claude/settings.json`、`CLAUDE.md`、`.mcp.json`。已存在的檔案**不覆蓋**，先列給使用者決定。awf init 對既有專案的預設做法：`.claude/agents`、`skills`、`commands` 不碰；既有的 `CLAUDE.md` 只在結尾追加一行 `@.claude/teams/agent-workflow.CLAUDE.md` 引用；既有的 `settings.json` 深度合併（使用者的值優先）；不是模版產生的同名 workflow 保留。使用者要整份換成模版版本時才加 `--overwrite-user-files`。也列出專案 `.claude/agents/` 裡叫 `dev-*`／`tpm-*` 的 agent，告訴使用者它們不會被當成模版角色（模版角色都帶 `agent-workflow:` 命名空間）。

## 3. 問專案設定（workflow.config）
把 plugin 目錄的 `workflow.config.example` 複製成目標專案的 `workflow.config`，逐項確認：
- 專案與客戶泛稱（`PROJECT_NAME`、`CLIENT_NAME`；範例一律用虛構的「示範客戶 ACME」）
- 目錄：`REPOS_DIR`、`RUNS_DIR`（預設 `_tpm/runs`）、`BRIEFS_DIR`（預設 `_tpm/briefs`）
- repo：`UPSTREAM_REPO`（唯讀）、`FORK_REPO`（推送目標）、`FORBIDDEN_REPOS`（絕對不能碰）
- 流程：`TASK_ID_PREFIX`、`TASK_AUTHORITY`、`DECISION_OWNER`、`STAKEHOLDER`、`NOTIFY_HOW`（需要人決策時怎麼通知，寫進 rules.md 的一句話；預設是推播到手機，使用者沒有這個習慣就照他說的寫，留空＝在對話中提問即可）
- 守門：`PROTECTED_BRANCHES`（受保護分支，預設 `main,master`）、`DOCS_DIRS`（tpm-writer 可以寫的文件目錄，預設 `docs`；只收工作區裡的相對路徑）。`BASE_BRANCH`、`BASE_REMOTE`（主線分支與它所在的 remote，預設 `main`、`origin`；`awf init --detect` 的守門主題會列出遠端的預設分支，不是 `main` 就照它設，並問要不要一起列進 `PROTECTED_BRANCHES`）。`SPECS_DIR`、`WORKTREES_DIR`、`SPEC_TEMPLATES_DIR`（目標 repo 裡的 spec 目錄、worktree 目錄、文件範本目錄，預設 `specs`、`.worktrees`、`specs/_templates`；專案已經把 spec 放在別處，例如 `docs/specs/`，`--detect` 會列出來，照它設；答案檔的寫法是 `guard.specs_dir`、`guard.worktrees_dir`、`guard.spec_templates_dir`）。這幾個和第 4 步的 `guard` 主題是同一件事：在這裡寫了，第 4 步就不用再寫進答案檔（答案檔的寫法是 `guard.base_branch`、`guard.base_remote`）
- 輸出：`OUTPUT_LANGUAGE`（預設繁體中文）、`ATTRIBUTION_LINE`（預設 `Assisted-by: Claude`）
- 設計系統：`DESIGN_SYSTEM_NAME`（沒有就留空）

## 4. 偵測現況，逐主題確認做法
先跑 `awf init --detect --target <目標專案> --json`。它只讀、不寫任何檔、不執行專案的程式、不讀機敏檔；輸出的 `topics` 是訪談主題，每個主題有 `ask`（要確認什麼）、`answer_keys`（答案檔的鍵）、`detected`（偵測到什麼與依據）、`suggest`（建議的答案）。

照 `topics` 的順序**一次確認一個主題**（AskUserQuestion，選項式）：

1. 先說偵測到什麼與依據（例如「package.json 有 lint 指令 `pnpm run lint`」）；沒偵測到就照實說沒有。
2. 問「照這個做法嗎」：選項放偵測到的建議、內建預設、「我自己說」。偵測到的只是建議，使用者沒選就不寫進答案檔。
3. 使用者回答後才進下一個主題。使用者說「其餘用預設」就停止提問。

| 主題（`id`） | 要確認的 | 沒回答時 | 答案檔鍵 |
|---|---|---|---|
| `methodology` | 用哪個 profile 當基礎（先讀 `scaffold/.claude/teams/methodologies/*.json` 的 `description` 列給使用者看）、要不要測試先行 | `default` | `methodology`、`test_first`、`overrides` |
| `conventions` | code style 檢查指令、commit 格式與檢查 regex、分支前綴、落後主線怎麼跟上、PR 大小上限、文件格式 | 依 profile | `code_style`、`commit`、`commit_regex`、`branch_prefix`、`sync_strategy`、`pr_size`、`doc_format` |
| `requirements` | 需求從哪來（spec 檔、需求 JSON、指令、規格層 openspec），merge gate 也用它 | 依 profile | `requirements` |
| `constraints` | 有沒有要 agent 遵守的文件；每份問套用到哪些段、哪些路徑、要不要逐條對照、有沒有檢查指令 | 沒有 | `constraints` |
| `models` | `inherit`（全部沿用主對話模型）、`auto`（依工作輕重分級，審查與合併有下限）、`fixed` | 依 profile | `models` |
| `read_limits` | 要不要限制模版 agent 的讀取量與門檻（建議先 `warn`） | 不啟用 | `read_limits` |
| `parallelism` | 超過幾個子任務拆子計劃、最多幾個子計劃、每波做完要不要停 | 依 profile | `parallelism` |
| `scrub` | 要不要做去客戶化掃描、規則檔放哪、什麼時候掃 | 不啟用 | `scrub` |
| `knowledge` | 知識庫來源；要不要建知識圖、流程要不要自動更新（`knowledge.auto.*`）、跨 repo（`knowledge.repos`） | 依 profile | `knowledge` |
| `links` | 要不要記連結帳本；`links.enforce`：不齊時要不要擋政策核准 | 不擋 | `links` |
| `deliverables` | 要不要產交付文件（細項之後用 `/agent-workflow:deliverables`） | 不啟用 | `deliverables` |
| `ci` | 要不要由 `awf ci render` 產生專案的測試／lint CI、跑哪些指令、哪些必過 | 不啟用 | `ci` |
| `build_loop` | build 的修正迴圈要不要只跑受影響的測試（test 段閘門照舊全跑） | 關 | `affected_tests_only` |
| `auto_merge` | 要不要啟用自動合併（啟用前要完成信任模型的檢查清單） | 關 | `auto_merge` |
| `main_session` | 會不會在主 session 直接 push 主線、合併 PR（會＝放寬主 session 的 git／GitHub 限制；auto_merge 啟用後主 session 仍不能合併） | 不會 | `main_session_push_merge` |
| `hooks` | hook 由 plugin 註冊，或註冊在專案的 settings.json（Windows 建議 `project`） | `plugin` | `hooks` |
| `guard` | 受保護分支、驗收測試目錄、文件目錄等 `guard.config.json` 的值 | 範本預設 | `guard` |

限制類的功能（讀取預算、去客戶化掃描、連結強制、交付文件、CI、自動合併）**預設都不啟用**：使用者明確說要才寫 `enabled: true`。

整理成答案檔（例如目標專案的 `methodology.answers.json`），把內容貼給使用者確認：

```json
{
  "methodology": "lightweight",
  "code_style": {"description": "提交前跑 lint", "tool": "pnpm", "command": "pnpm run lint"},
  "commit": {"format": "<type>: <摘要>", "check_regex": "^(feat|fix|docs|test|chore): .+"},
  "branch_prefix": "feat/",
  "pr_size": {"max_lines": 400, "max_files": null},
  "models": "auto",
  "ci": {"enabled": true, "jobs": [{"id": "test", "name": "測試", "run": "pnpm run test", "required": true}]},
  "auto_merge": false
}
```

- 沒問到或使用者接受預設的項目就不要寫進答案檔，沿用 profile 的值。
- 答案檔不放機敏值（token、密碼、金鑰）；init 存答案時像機敏的字串會略去。
- `models`、`scrub`、`knowledge`、`links`、`deliverables`、`ci`、`parallelism` 的物件原樣合併進 `methodology.json` 的同名鍵（`models` 也可以只寫模式字串）；`guard` 的物件合併進 `guard.config.json`。
- `commit` 只給字串時，profile 原本的範例與檢查 regex 會清掉（避免跟新格式不符）。
- `pr_size` 的值寫 `null` 或 `0` 表示不限，deliver 段就不檢查 PR 大小。
- 要改 profile 其他欄位（例如某一段的指引文字）用 `overrides`，內容會深層合併進 profile。
- 分支前綴會同時改 `BRANCH_PREFIX`，並加進 `ALLOWED_BRANCH_PREFIXES`。
- `sync_strategy` 是 `auto` 或 `transit` 時會用到中轉分支（預設前綴 `merge/`），`ALLOWED_BRANCH_PREFIXES` 要保留它。

## 5. 跑 init
`awf init --target <目標專案> --config <目標專案>/workflow.config --answers <答案檔>`。執行前把指令列給使用者看。init 寫入前會印「設定對照表」（設定｜偵測到｜你的回答｜會寫成）：**把這張表唸給使用者**，和他確認的不一樣就改答案檔重跑。答案會存在 `.claude/teams/init-answers.json`。可選參數：
- `--reconfigure <主題>`：已套用的專案只改某一類設定時用（例如使用者說「改受保護分支」「之後才要開知識圖」）：只重問那一個主題，其他主題不問、不動。可以單獨問的主題：`requirements`、`models`、`scrub`、`links`、`ci`、`build_loop`、`auto_merge`、`hooks`、`knowledge`、`deliverables`、`parallelism`、`guard`、`main_session`；`methodology`、`conventions`、`constraints`、`read_limits` 的題目互相有關，要不帶主題問一整輪。在對話中代問時不要用互動模式：先 `awf init --detect --target <目標專案> --json` 看那個主題的 `ask` 與 `answer_keys`，和使用者確認後寫一份只含那個主題的鍵的答案檔，跑 `awf init --target <目標專案> --reconfigure <主題> --answers <答案檔>`（答案檔有別的主題的鍵會被拒絕）
- `--reconfigure`：已套用的專案之後要改設定時用。讀 `init-answers.json` 當上次的答案，`--answers` 只寫要改的鍵（值寫 `null`＝拿掉那個答案），只改答案有變的設定，使用者手改過的其他鍵不動；會列出「會改 N 項」。不能和 `--upgrade`、`--force` 一起用
- `--upgrade`：升級已 init 的專案（第 2 步發現舊版時用）。模版檔換成新版；`methodology.json` 不動、`guard.config.json` 保留使用者的值只補新鍵、`rules.md` 的〈專案規則〉段之後保留；沒給 `--config` 時沿用 init 記下的 `.claude/teams/workflow.config`。輸出會列出新版內建 profile 有、專案 profile 沒有的欄位，給使用者看過、同意後再加 `--adopt-new-fields` 重跑補上（只補缺的，既有值不改）
- `--force`：覆蓋已存在的模版檔（`.claude/teams/` 等；只有在第 2 步使用者同意覆蓋時才加）。使用者設定的保留規則同 `--upgrade`：`methodology.json`、`guard.config.json`、〈專案規則〉不會被整份換掉。沒給 `--config` 時一樣沿用 `.claude/teams/workflow.config`。使用者的 `CLAUDE.md`、`settings.json` 加 `--force` 也只會合併
- `--overwrite-user-files`：連使用者的檔案也整份覆蓋，包括 `methodology.json`、`guard.config.json`、〈專案規則〉（只有使用者明確要求重設時才加）
- `--with-merge-gate`：放 merge gate CI（預設不放；沒有 gate App 時會讓每個 PR 失敗，只有使用者已完成 docs/自動合併信任模型.md 的檢查清單才加）
- `--hooks plugin|project`：hooks 用 plugin 內建的，或複製到專案（Windows 建議 project）
- `--with-mcp`：一併產生 `.mcp.json`

init 會把 profile 與答案合併成 `.claude/teams/methodology.json`，用 schema 驗過（workflow 腳本不複製進專案，之後由 `awf next` 用 plugin 的範本加這份 profile 逐段產生）；並寫入 `.claude/teams/VERSION`（模版版本）與 `.claude/teams/workflow.config`（這次用的佔位符值，之後 `--upgrade`、`--force` 沒給 `--config` 時沿用）。

## 6. 驗證
- 產出的檔案裡不應再有未替換的 `{{…}}`：`grep -rn '{{[A-Z_]*}}' <目標專案>/.claude <目標專案>/CLAUDE.md`。
- `.claude/teams/methodology.json` 的內容跟使用者確認的習慣一致；`awf render --all --out _tpm/wf-check` 能產生 12 支腳本。
- `.claude/teams/rules.md` 有〈專案規則〉段、`.claude/teams/guard.config.json` 是合法 JSON。
- 跑 `awf doctor`、`awf render --all`、`awf check-agents`（指令見 awf init 最後印出的「下一步」）。

## 7. 後續
- 告訴使用者：之後要改開發規範或慣例，直接改 `.claude/teams/methodology.json`，下一段開跑時就生效（不用再跑 `awf build`）；格式與新增 profile 的方式見模版的 `docs/自訂與擴充.md`〈自訂開發規範〉。
- 安裝 plugin（若尚未）：照模版 README 的安裝段。
- 問使用者有沒有自己的 agent 想加進 workflow（例如團隊自己的審查或調研 agent）：有的話接著跑 `/agent-workflow:add-agent`（`add-agent` skill）幫他設定。
- 有設計正本時，複製 plugin 的 `design-system-template` skill 成專案自己的設計系統 skill，並登記在〈專案規則〉。
- 這些都只寫目標專案；不推送、不提交，提交由使用者決定。
