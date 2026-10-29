# CLAUDE.md

這份文件是給在這個 repo 工作的 Claude Code 看的。給使用者看的說明在 `README.md`。

## 這個 repo 是什麼

AgentWorkFlow 是**模版本身**，不是套用了模版的專案。它分兩部分，透過兩條管道發佈：

| 部分 | 內容 | 發佈方式 | 更新方式 |
|---|---|---|---|
| `plugin/` | agents、skills、commands、hooks（plugin 名稱 `agent-workflow`，agent 命名空間 `agent-workflow:`） | `.claude-plugin/marketplace.json`（marketplace 名稱 `agentworkflow`），所有專案共用 | 使用者執行 `/plugin marketplace update agentworkflow` |
| `scaffold/` | `.claude/teams/`（rules、schemas、methodologies、tools、golden-set）、`.claude/workflows/*.js`、`.github/workflows/agent-merge-gate.yml.tmpl`、settings、`CLAUDE.md.tmpl`、`.mcp.json.tmpl`、`_runs/` | `init.py` 複製進目標專案：把 `{{佔位符}}` 換成 `workflow.config`、init 問答或 `--answers` 的值，並去掉 `.tmpl` 副檔名 | 已 init 的專案**不會**自動更新，要重跑 `init.py --force` |

workflow 放在 `scaffold/` 而不放在 plugin 裡，因為 Workflow 工具只讀專案的 `.claude/workflows/`。

## 原則

- **模版只管工作流程。** 開發規範（TDD、DDD、SDD 等）和慣例（code style、commit 格式、分支命名、PR 大小、文件格式）由使用者定義，寫在 methodology profile。agent、workflow、rules 不寫死任何做法；特定做法只能從 profile 讀，或明寫為「預設 profile 範例」。
- **語言**：所有文件、註解、agent 提示都用繁體中文。agent 的輸出語言用佔位符 `{{OUTPUT_LANGUAGE}}`，不要寫死。
- **跨平台**：工具與測試全部用 Python，只有驗證 workflow 用 Node。要能在 macOS、Linux、Windows（PowerShell）執行，不新增 shell 腳本。
- **不破壞使用者的既有設定**：init 對既有專案只新增檔案。`CLAUDE.md` 只追加 `@` 引用，settings 與 `.mcp.json` 做深度合併。`--force` 只覆寫模版檔，整份覆寫使用者檔要加 `--overwrite-user-files`。merge gate CI 要加 `--with-merge-gate` 才會放。
- **去客戶化**：不寫任何真實客戶、人名、帳號、內部代號，範例一律用虛構的「示範客戶 ACME」。禁用清單有兩份，都是一行一個 ERE 正規式：
  - `tests/scrub_terms.txt`：公開的通用規則（本機路徑、憑證形狀、個人信箱）。
  - `tests/scrub_terms.local.txt`：真實名稱，**不進版控**。真實名稱只能寫在這份，寫進公開清單就等於公開了。
  - 掃描範圍是整個 repo 的檔案內容**與路徑名**，略過根目錄的本機目錄 `_tpm`、`graft`、`.claude`。
- **gate 只會更嚴**：確定性工具碰到輸入缺漏一律 hold 或 halted 交人，不做樂觀推定。

## 指令

在 repo 根目錄執行：

```bash
python3 tests/run_all.py            # 全部測試：去客戶化掃描 → init 煙霧測試 → hook 測試 → check-tokens（每次改完都要跑）
python3 tests/scrub_check.py [路徑…]  # 只跑去客戶化掃描，可只掃指定路徑
python3 tests/smoke_init.py [設定檔]  # 在暫存目錄跑 init.py 並檢查產出；KEEP=1 保留暫存目錄
python3 plugin/hooks/tests/run.py [--project <已 init 的專案>]  # hook 測試（應擋＝exit 2）
python3 init.py --target <專案> [--hooks plugin|project] [--answers <答案.json>] [--defaults]
```

下面這些工具在 scaffold 裡，要在**已 init 的專案根目錄**執行。smoke_init 就是在暫存專案裡跑它們。

```bash
python3 .claude/teams/tools/build.py [--check]   # 把 schemas 與 methodology profile 內嵌進 workflows/*.js
node .claude/teams/tools/check_workflows.mjs --agents-dir <AgentWorkFlow>/plugin/agents   # 乾跑所有 workflow（含 SCENARIOS）
python3 .claude/teams/tools/check_agents.py --plugin-dir <AgentWorkFlow>/plugin           # 檢查 tpm-*/dev-* agent 格式（16 個）
uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py              # schema 正反例
python3 .claude/teams/tools/merge_gate.py --selftest                                       # 確定性 merge gate
python3 .claude/teams/tools/publish.py --selftest                                          # 確定性推送工具
```

在 repo 內也可以直接驗 scaffold，例如 `python3 scaffold/.claude/teams/tools/build.py --check`，路徑會依檔案位置推算。

計畫型的工作用可驗證 checklist：`python3 scaffold/.claude/teams/tools/checklist.py check <checklist.md> [--id <前綴>] [--write]`。打勾以驗證指令的實際執行結果為準。checklist 放在根目錄的 `plans/`（本機，不進版控）。

## 架構

### agent（`plugin/agents/`，共 22 個）

- **`dev-*`（10 個）**：AI-native SDLC，依序 plan → design → build → test → deliver → merge，各段對應一支 `sdlc-*.js`。
  - plan、design、test、deliver 之間有核准閘門，用 `save_run.py approve` 記錄。`--by policy` 會讀信封的實際內容（`policy_violations`），不只看 status。
  - `dev-merge-steward`：可信任的合併 agent。逐條對照需求與驗證證據（綁 head SHA），一致而且 `merge_gate.py` 通過才合併；不一致就產出 followup 新需求並退回 PR（request-changes、轉 draft）。沒有 Edit／Write，永遠不 approve。
  - `dev-release-watcher`：合併後監看，歸因失敗，產出 revert 提案與凍結建議，只讀。
- **`tpm-*`（6 個）**：事件處理，對應 `event-*.js`（包括 `event-post-merge.js`）。
- **網站分析組（6 個）**：tech-architect、infra-analyst、biz-analyst、perf-auditor、competitor-scout、lead-synthesizer。由 skills 驅動，**不交信封**，也不受角色寫入範圍限制。

### 交件信封

`tpm-*`／`dev-*` 一律用 JSON 信封回報：status、summary、artifacts、evidence、confidence、open_questions、derived_work、payload。

- 信封的定義在 `scaffold/.claude/teams/schemas/envelope.schema.json`，各種 payload 的定義在 `schemas/payloads/*.schema.json`，例如 `subtasks`、`plan-outline`、`criteria`、`merge-decision`。
- 驗證分兩處：
  - `plugin/hooks/awf_hook.py envelope` 轉給 `envelope_hook.py`，在 SubagentStop、TaskCompleted 時驗證。
  - `tools/schemalib.py` 是 hook、golden-set、merge_gate 共用的驗證器。schema 表達不了的語意規則也寫在這裡，例如子任務的 owns 不能重疊、子計劃的範圍不能重疊或相依循環、decision=merge 時不能有 mismatch。

### schema 與 profile 內嵌

Workflow 腳本不能 import 檔案，所以 workflow 裡只寫標記：`/*@schema:<payload>*/ null /*@end*/` 和 `/*@methodology*/ null /*@end*/`，再由 `build.py` 換成完整的 JSON。profile 取自專案的 `.claude/teams/methodology.json`，scaffold 裡保持 null。

**改了 schema 或 profile 就要重跑 `build.py`**，並在 `golden-set/valid|invalid/` 補正例與反例。不要手改內嵌進去的 JSON。

### methodology profile

- 格式由 `schemas/methodology.schema.json` 定義：
  - `stages`：各段的做法。
  - `requirements`：需求來源。
  - `conventions`：慣例。
  - `parallelism`：選填的工作流程參數。
- 內建兩份：`methodologies/default.json`（現行做法）與 `lightweight.json`。init 問答或 `--answers` 的結果會合併成專案的 `.claude/teams/methodology.json`。
- 需求來源 adapter（json、command，以及 spec-md 範例）和 `merge_gate.py` 共用。`guard.config.json` 的 `requirements` 有設時以它為準。

### 平行處理

- **`sdlc-design`**：先由 dev-spec-author 出大綱（`plan-outline`）。預估子任務超過 `parallelism.split_threshold`，或有多個可以各自規劃的區塊時，拆成最多 `max_sub_plans` 個子計劃。每個子計劃由一個 dev-spec-author 平行規劃，寫在 `specs/<epic>/sub/<id>/`，不 commit；最後由一個 dev-spec-author 彙整並 commit。跨子計劃的相依寫成 `@<子計劃 id>`，由 workflow 展開。
- **`sdlc-build`**：依子任務相依分層，同一層各自開 worktree 平行實作，每層由 dev-integrator 合併。
- **`sdlc-test`**：有子計劃時，每個子計劃一個 dev-verifier，另加一個整合分片，平行驗證後彙整成 `verifier.md`。失敗分散在多個子計劃時，各自開 worktree 平行修正，再合併。
- workflow 會對以下幾點做確定性核對，不一致就回傳 `halted: true`：子計劃的範圍與相依、彙整後的子任務清單（不能默默少掉）、彙整後的驗證結果（不能漏條目或改結果，各分片要驗同一個 HEAD）。
- 預設乾跑走不到這些分支，所以 `check_workflows.mjs` 的 `SCENARIOS` 用假回傳把它們走過一遍。改了這些流程，`SCENARIOS` 要一起改。

### 使用者自己的 agent（擴充）

- **設定位置**：使用者用 `/add-agent`（`plugin/skills/add-agent`）把自己的 agent 加進 plan、design、test、deliver 任一段，設定寫兩個地方：
  - `methodology.json` 的 `extensions`：寫 name、agent、stage、instructions。
  - `guard.config.json` 的 `extension_agents`：`{完整 agent_type: 要套用的模版角色}`。
- **workflow 怎麼派**：四支 sdlc workflow 裡各有 `EXT`／`extJob`，把擴充 agent 和內建 agent 平行派出。`agentType` 用使用者給的原值，不加 `NS`；一律交 `findings`。
- **守門**：
  - `awf_config.extension_role()` 讓登記過的擴充 agent 算模版角色，套用對應角色的規則。
  - 只能對應 `EXTENSION_ROLES`（唯讀類），其他值改套 `tpm-reviewer`。
  - `merge_exception` 與 freeze 仍比對完整 agent_type，擴充 agent 不會因此能合併。
- **沒登記就不內嵌**：`build.py` 遇到沒登記守門角色的擴充 agent 會拒絕內嵌（exit 1）。
- **不開放的段**：build（有檔案所有權）與 merge（信任邊界）。

### Workflow 腳本的限制

- 腳本裡不能取時間，跑到一半也不能等人輸入。日期由主 session 放進 `run_dir`；需要人決定的地方，就把 workflow 拆成兩支，例如 `sdlc-design` 和 `sdlc-design-revise`。
- 每個 `agent()` 都要帶 `agentType: NS + '<agent 名>'` 和 `schema`。`NS` 取自 `'{{AGENT_NS}}'`，預設 `agent-workflow:`。
- 多個 agent 共用同一個 worktree 時不能各自 commit（會搶 index.lock），要由最後一步統一 commit。

### 自動合併

決策記錄在 `docs/adr-agent-auto-merge.md`，信任模型寫在 `docs/自動合併信任模型.md`。

- `merge_gate.py`：確定性 gate。複核需求對照、head SHA、required checks、測試數、skip、風險等級、每日上限、白名單、凍結旗標，並寫入 `<runs_dir>/merge-audit.jsonl`。放寬閘門（改測試／lint／覆蓋率設定、新增壓制註解）一律 escalate；`auto_merge.carry_verdict_on_rebase`（預設關閉）開啟時，rebase 後 patch-id 相同才沿用 verdict。
- steward 合併後要讀回 PR 狀態填 `merge_state`，`sdlc-merge` 只在 `merged` 時算已合併（merge queue 可能只是排隊）。
- `publish.py`：確定性推送工具。只推 fork 的功能分支，推送前掃機敏值、檢查閘門、開 PR，並寫稽核紀錄。
- `scaffold/.github/workflows/agent-merge-gate.yml.tmpl`：給目標專案用的 CI，從受保護的 base 執行 gate，設為 required check。
- 真正的信任邊界在伺服器端（ruleset、GitHub App、CODEOWNERS），hook 只是輔助。`auto_merge` 預設關閉。

### Hooks（`plugin/hooks/`）

- **入口**：只有一個，`awf_hook.py <guard-shell|guard-paths|envelope>`。設定一律透過 `awf_config.py` 讀 `${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json`，不要寫死路徑。
- **`guard-shell` 擋什麼**：同時處理 Bash 與 PowerShell 工具。擋推送、寫上游、推受保護分支、越權寫入，以及所有合併與核准路徑：`gh pr merge`、`--approve`、`gh api`、GraphQL、curl、Invoke-RestMethod。
- **steward 合併什麼時候放行**：由 `guard.py` 的 `merge_exception` 判斷，以下全部成立才放行：
  - `auto_merge` 已啟用。
  - 完整的 agent_type 等於 `merger_role`。
  - 指令帶 `--match-head-commit`，而且 merge-audit 有相符的 merge 紀錄。
  - 沒有凍結。
  - 上游 repo 另外要有 `upstream_authorized_by`。
- **`--approve`**：對任何人一律擋。
- **角色規則套用範圍**：只套用在帶 plugin 命名空間（`team_agent_namespaces`，預設 `agent-workflow:`）的 agent，以及和模版角色完全同名的 agent（`RESERVED_ROLES`）。使用者自己的 agent 不受影響。
- **主 session**：限制由 `main_session_git_rules`、`main_session_github_rules` 兩個開關控制。啟用 `auto_merge` 時，主 session 仍然不能合併。
- **沒有設定檔時**：採安全預設，只套用 `tpm-`／`dev-` 的角色規則，不擋主 session，所以還沒 init 的專案不會被誤擋。
- **註冊方式**：兩種只能擇一。
  - plugin 註冊：`init.py --hooks plugin`，預設。
  - 複製進專案的 `.claude/hooks/`：`--hooks project`，Windows 建議用這個，init 會寫入 Python 的絕對路徑。

### Slash commands 與 skills

- `plugin/commands/` 只負責把使用者的話整理成 `tpm-team` skill 的輸入。流程寫在 skill 和 workflow 裡，不要在 command 裡再寫一份。
- 事件對照表與閘門規則寫在 `plugin/skills/tpm-team/SKILL.md`。
- `/init-workflow` 由 `init-workflow` skill 提供，會先在對話中確認使用者的習慣，再跑 `init.py --answers`。
- `/correct` 由 `correct` skill 提供，把一則教訓變成最高可行層級的強制機制，並在 rules.md〈專案規則〉維護「規則 ↔ 強制方式」對照表。

## 改動時的檢查清單

- **新增佔位符**：`workflow.config.example`、`init.py` 和 README 的佔位符表要一起改。smoke_init 會檢查產出裡有沒有殘留的 `{{`。
- **新增 `dev-*`／`tpm-*` agent**：
  - 照既有段落寫：切分理由、職責、硬性約束、工作方式、交件格式。
  - 設 `model: inherit` 和 `skills`。只讀的角色不給 Edit／Write。
  - 改 `check_agents.py` 的角色數。
  - 把角色名加進 `awf_config.py` 的 `RESERVED_ROLES`；需要套用 guard 時，在 `role_map` 加對應。
- **新增 workflow**：在 `tpm-team` skill 的事件對照表加一列，並加進 `tests/smoke_init.py` 的清單。分支有乾跑走不到的，在 `check_workflows.mjs` 的 `SCENARIOS` 補情境。
- **新增或修改 payload schema**：重跑 `build.py`；在 `golden-set/valid|invalid/` 補正例、反例，有語意規則的另補 `semantic` 反例；語意規則寫在 `schemalib.py`。
- **改擴充 agent 機制**：四支 sdlc workflow 的 `EXT` 片段要一致；`check_workflows.mjs` 的擴充情境、`smoke_init.py` 的 `check_extensions`、`plugin/hooks/tests/run.py` 的擴充 agent 案例要一起改。
- **新增 hook 規則**：在 `plugin/hooks/tests/run.py` 補一個該擋、一個該放的測試，Bash 與 PowerShell 各一。
- **改 profile 格式**：`methodology.schema.json`、兩份內建 profile、`docs/自訂與擴充.md` 的欄位表要一起改。
- **完成前**：每次都跑 `python3 tests/run_all.py`。

完整的擴充說明見 `docs/自訂與擴充.md`，實戰教訓見 `docs/踩坑手冊.md`。
