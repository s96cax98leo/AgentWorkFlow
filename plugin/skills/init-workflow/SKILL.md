---
name: init-workflow
description: 在新專案套用 AgentWorkFlow：先在對話中確認使用者的開發習慣（開發規範、code style、commit 格式、分支、PR 大小、文件格式），整理成 workflow.config 與答案檔，再跑 init.py --answers 並驗證產出。使用者說「套用 AgentWorkFlow」「初始化 agent workflow」「init workflow」「設定開發規範」時使用。
argument-hint: <目標專案路徑（可省略＝目前目錄）>
---

# 套用 AgentWorkFlow（init-workflow）

模版只管工作流程（plan → design → build → test → deliver → merge 與閘門）；開發規範與慣例由使用者決定，寫進專案的開發規範 profile（`.claude/teams/methodology.json`）。所以**先問使用者的習慣，再寫設定**，不要直接套預設值。

使用者給的目標路徑：$ARGUMENTS（沒給就用目前工作目錄）。

每一步先說明要做什麼，等使用者同意再執行。提問用選項式（AskUserQuestion），每題附預設值，使用者可以直接接受。

## 1. 確認位置
找到 AgentWorkFlow 模版根目錄（有 `init.py`、`workflow.config.example`、`scaffold/` 的那個目錄；plugin 安裝在 `${CLAUDE_PLUGIN_ROOT}`，模版根目錄是它的上一層）。找不到就問使用者。

## 2. 檢查目標專案
列出目標專案已存在的 `.claude/teams/`（含 `methodology.json`）、`.claude/workflows/`、`.claude/settings.json`、`CLAUDE.md`、`.mcp.json`。已存在的檔案**不覆蓋**，先列給使用者決定。init.py 對既有專案的預設做法：`.claude/agents`、`skills`、`commands` 不碰；既有的 `CLAUDE.md` 只在結尾追加一行 `@.claude/teams/agent-workflow.CLAUDE.md` 引用；既有的 `settings.json` 深度合併（使用者的值優先）；不是模版產生的同名 workflow 保留。使用者要整份換成模版版本時才加 `--overwrite-user-files`。也列出專案 `.claude/agents/` 裡叫 `dev-*`／`tpm-*` 的 agent，告訴使用者它們不會被當成模版角色（模版角色都帶 `agent-workflow:` 命名空間）。

## 3. 問專案設定（workflow.config）
把 `workflow.config.example` 複製成目標專案的 `workflow.config`，逐項確認：
- 專案與客戶泛稱（`PROJECT_NAME`、`CLIENT_NAME`；範例一律用虛構的「示範客戶 ACME」）
- 目錄：`WORKSPACE_DIR`、`REPOS_DIR`、`RUNS_DIR`（預設 `_tpm/runs`）、`BRIEFS_DIR`（預設 `_tpm/briefs`）
- repo：`UPSTREAM_REPO`（唯讀）、`FORK_REPO`（推送目標）、`FORBIDDEN_REPOS`（絕對不能碰）
- 流程：`TASK_ID_PREFIX`、`TASK_AUTHORITY`、`DECISION_OWNER`、`STAKEHOLDER`
- 輸出：`OUTPUT_LANGUAGE`（預設繁體中文）、`ATTRIBUTION_LINE`（預設 `Assisted-by: Claude`）
- 設計系統：`DESIGN_SYSTEM_NAME`（沒有就留空）

## 4. 問開發習慣（開發規範 profile）
先讀 `scaffold/.claude/teams/methodologies/*.json`，把每個內建 profile 的 `description` 列給使用者看，再逐項問。使用者說「照我們 repo 現在的做法」時，先讀目標 repo 的 CLAUDE.md、CONTRIBUTING、lint 設定、最近的 commit 與 PR，整理成建議值再請使用者確認，不要猜。

| 項目 | 要問的 | 預設 | 答案檔鍵 |
|---|---|---|---|
| 開發規範 | 用哪個 profile 當基礎（default：spec 驅動、測試先行；lightweight：需求來自 issue、不強制測試先行；openspec：spec 交給 OpenSpec、核准前跑 spec_gate；或使用者自己的 profile JSON） | `default` | `methodology` |
| 測試時機 | build 段要不要先寫會失敗的測試再實作 | 依 profile | `test_first` |
| code style | 用什麼工具、提交前跑什麼檢查指令 | 照 repo 既有設定 | `code_style` |
| commit | commit 格式說明、範例、要不要用 regex 檢查標題 | 依 profile | `commit` |
| 分支 | epic 與子任務分支的前綴 | `feature/` | `branch_prefix` |
| PR 大小 | 上限行數、檔案數（可以不限） | 依 profile | `pr_size` |
| 文件格式 | specs 等文件用什麼格式 | `Markdown` | `doc_format` |
| 需求來源 | 需求從哪裡來（spec 檔、需求 JSON、issue 轉出的指令），merge 段的 gate 也用它 | 依 profile | `requirements` |
| 主 session 習慣 | 你既有的 skill 或習慣會不會在主 session 直接 push main、合併 PR？答會時放寬主 session 的 git／GitHub 限制（`main_session_git_rules`、`main_session_github_rules` 設 false），並提醒：啟用 auto_merge 後主 session 仍不能合併 | 不會 | `main_session_push_merge` |

整理成答案檔（例如目標專案的 `methodology.answers.json`），把內容貼給使用者確認：

```json
{
  "methodology": "lightweight",
  "test_first": false,
  "code_style": {"description": "提交前跑 ruff", "tool": "ruff", "command": "ruff format --check ."},
  "commit": {"format": "<type>: <摘要>", "examples": {"feat": "feat: <Story> 實作"}, "check_regex": "^(feat|fix|docs|test|chore): .+"},
  "branch_prefix": "feature/",
  "pr_size": {"max_lines": 400, "max_files": null},
  "doc_format": "Markdown"
}
```

- 沒問到或使用者接受預設的項目就不要寫進答案檔，沿用 profile 的值。
- `commit` 只給字串時，profile 原本的範例與檢查 regex 會清掉（避免跟新格式不符）。
- `pr_size` 的值寫 `null` 或 `0` 表示不限，deliver 段就不檢查 PR 大小。
- 要改 profile 其他欄位（例如某一段的指引文字）用 `overrides`，內容會深層合併進 profile。
- 分支前綴會同時改 `BRANCH_PREFIX`，並加進 `ALLOWED_BRANCH_PREFIXES`。

## 5. 跑 init
`python3 <模版根目錄>/init.py --target <目標專案> --config <目標專案>/workflow.config --answers <答案檔>`（Windows 用 `py -3` 或 `python`）。執行前把指令列給使用者看。可選參數：
- `--force`：覆蓋已存在的模版檔（`.claude/teams/` 等；只有在第 2 步使用者同意覆蓋時才加；沒加時已存在的 `methodology.json` 會保留）。使用者的 `CLAUDE.md`、`settings.json` 加 `--force` 也只會合併
- `--overwrite-user-files`：連使用者的檔案也整份覆蓋（只有使用者明確要求重設時才加）
- `--with-merge-gate`：放 merge gate CI（預設不放；沒有 gate App 時會讓每個 PR 失敗，只有使用者已完成 docs/自動合併信任模型.md 的檢查清單才加）
- `--hooks plugin|project`：hooks 用 plugin 內建的，或複製到專案（Windows 建議 project）
- `--with-mcp`：一併產生 `.mcp.json`

init 會把 profile 與答案合併成 `.claude/teams/methodology.json`，用 schema 驗過，再跑 `build.py` 內嵌進 `sdlc-*.js`。

## 6. 驗證
- 產出的檔案裡不應再有未替換的 `{{…}}`：`grep -rn '{{[A-Z_]*}}' <目標專案>/.claude <目標專案>/CLAUDE.md`。
- `.claude/teams/methodology.json` 的內容跟使用者確認的習慣一致；`python3 .claude/teams/tools/build.py --check` 通過。
- `.claude/teams/rules.md` 有〈專案規則〉段、`.claude/teams/guard.config.json` 是合法 JSON。
- 跑 golden-set、`check_workflows.mjs`、`check_agents.py`（指令見 init.py 最後印出的「下一步」）。

## 7. 後續
- 告訴使用者：之後要改開發規範或慣例，直接改 `.claude/teams/methodology.json`，再跑 `python3 .claude/teams/tools/build.py`；格式與新增 profile 的方式見模版的 `docs/自訂與擴充.md`〈自訂開發規範〉。
- 安裝 plugin（若尚未）：照模版 README 的安裝段。
- 問使用者有沒有自己的 agent 想加進 workflow（例如團隊自己的審查或調研 agent）：有的話接著跑 `/add-agent`（`add-agent` skill）幫他設定。
- 有設計正本時，複製 plugin 的 `design-system-template` skill 成專案自己的設計系統 skill，並登記在〈專案規則〉。
- 這些都只寫目標專案；不推送、不提交，提交由使用者決定。
