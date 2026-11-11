# CLAUDE.md

這份文件是給在這個 repo 工作的 Claude Code 看的。給使用者看的說明在 `README.md`。

## 這個 repo 是什麼

AgentWorkFlow 是**模版本身**，不是套用了模版的專案。它分兩部分，透過兩條管道發佈：

| 部分 | 內容 | 發佈方式 | 更新方式 |
|---|---|---|---|
| `plugin/` | agents、skills、commands、hooks（plugin 名稱 `agent-workflow`，agent 命名空間 `agent-workflow:`） | `.claude-plugin/marketplace.json`（marketplace 名稱 `agentworkflow`），所有專案共用 | 使用者執行 `claude plugin update agent-workflow@agentworkflow`（或 `/plugin`），再 `/reload-plugins`；只跑 marketplace update 不會更新 plugin |
| `scaffold/` | `.claude/teams/`（rules、schemas、methodologies、tools、golden-set）、`.claude/workflows/*.js`、`.github/workflows/agent-merge-gate.yml.tmpl`、settings、`CLAUDE.md.tmpl`、`.mcp.json.tmpl`、`_runs/` | `init.py` 複製進目標專案：把 `{{佔位符}}` 換成 `workflow.config`、init 問答或 `--answers` 的值，並去掉 `.tmpl` 副檔名；另寫 `.claude/teams/VERSION` 與 `.claude/teams/workflow.config` | 已 init 的專案**不會**自動更新，要跑 `init.py --upgrade`（`--adopt-new-fields` 補 profile 新欄位） |

workflow 放在 `scaffold/` 而不放在 plugin 裡，因為 Workflow 工具只讀專案的 `.claude/workflows/`。

## 原則

- **模版只管工作流程。** 開發規範（TDD、DDD、SDD 等）和慣例（code style、commit 格式、分支命名、PR 大小、文件格式）由使用者定義，寫在 methodology profile。agent、workflow、rules 不寫死任何做法；特定做法只能從 profile 讀，或明寫為「預設 profile 範例」。
- **語言**：所有文件、註解、agent 提示都用繁體中文。agent 的輸出語言用佔位符 `{{OUTPUT_LANGUAGE}}`，不要寫死。
- **跨平台**：工具與測試全部用 Python，只有驗證 workflow 用 Node。要能在 macOS、Linux、Windows（PowerShell）執行，不新增 shell 腳本。
- **不破壞使用者的既有設定**：init 對既有專案只新增檔案。`CLAUDE.md` 只追加 `@` 引用，settings 與 `.mcp.json` 做深度合併。`--force` 只覆寫模版檔，整份覆寫使用者檔要加 `--overwrite-user-files`。merge gate CI 要加 `--with-merge-gate` 才會放。
- **去客戶化**：不寫任何真實客戶、人名、帳號、內部代號，範例一律用虛構的「示範客戶 ACME」。禁用清單有兩份，都是一行一個 ERE 正規式：
  - `tests/scrub_terms.txt`：公開的通用規則（本機路徑、憑證形狀、個人信箱）。
  - `tests/scrub_terms.local.txt`：真實名稱，**不進版控**。真實名稱只能寫在這份，寫進公開清單就等於公開了。
  - 掃描範圍是整個 repo 的檔案內容**與路徑名**，略過根目錄的本機目錄 `_tpm`、`graft`、`.claude`、`.idea`。
- **gate 只會更嚴**：確定性工具碰到輸入缺漏一律 hold 或 halted 交人，不做樂觀推定。

## 指令

在 repo 根目錄執行：

```bash
python3 tests/run_all.py            # 全部測試：去客戶化掃描 → init 煙霧測試 → hook 測試 → tests/check_*.py → check-tokens（每次改完都要跑）
python3 tests/scrub_check.py [路徑…]  # 只跑去客戶化掃描，可只掃指定路徑
python3 tests/smoke_init.py [設定檔]  # 在暫存目錄跑 init.py 並檢查產出；KEEP=1 保留暫存目錄
python3 plugin/hooks/tests/run.py [--project <已 init 的專案>]  # hook 測試（應擋＝exit 2）
python3 tests/check_docs.py          # 文件裡的角色、workflow、command 數量與名單要和實際檔案一致
python3 tests/bench/run_plan.py --selftest   # 基準實跑腳本不花 token 的部分；不帶 --selftest 會真的呼叫 claude（數百萬 token），只在發佈前手動跑
python3 init.py --target <專案> [--hooks plugin|project] [--answers <答案.json>] [--defaults] [--upgrade [--adopt-new-fields]]
```

下面這些工具在 scaffold 裡，要在**已 init 的專案根目錄**執行。smoke_init 就是在暫存專案裡跑它們。

```bash
python3 .claude/teams/tools/build.py [--check]   # 把 schemas 與 methodology profile 內嵌進 workflows/*.js
node .claude/teams/tools/check_workflows.mjs --agents-dir <AgentWorkFlow>/plugin/agents   # 乾跑所有 workflow（含 tools/scenarios/*.mjs 的情境）
python3 .claude/teams/tools/check_agents.py --plugin-dir <AgentWorkFlow>/plugin           # 檢查 tpm-*／dev-* agent 格式（16 個：14 個核心角色＋2 個舊名薄殼）
uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py              # schema 正反例
python3 .claude/teams/tools/merge_gate.py --selftest                                       # 確定性 merge gate
python3 .claude/teams/tools/publish.py --selftest                                          # 確定性推送工具
python3 .claude/teams/tools/sync_branch.py --selftest                                      # 跟上 base 的確定性工具
python3 .claude/teams/tools/pack.py --selftest                                             # 資料包與調研重用
python3 .claude/teams/tools/kb.py --selftest                                               # 知識層
python3 .claude/teams/tools/awf_cost.py --selftest                                         # workflow 的 token 與時間
```

在 repo 內也可以直接驗 scaffold，例如 `python3 scaffold/.claude/teams/tools/build.py --check`，路徑會依檔案位置推算。

計畫型的工作用可驗證 checklist：`python3 scaffold/.claude/teams/tools/checklist.py check <checklist.md> [--id <前綴>] [--write]`。打勾以驗證指令的實際執行結果為準。checklist 放在根目錄的 `plans/`（本機，不進版控）。

## 架構

### agent（`plugin/agents/`，agent 檔共 22 個）

14 個核心角色（`check_agents.py` 的 `CORE`）、2 個舊名薄殼（`ALIASES`）、6 個網站分析 agent。

- **`dev-*`（9 個）**：spec-author、acceptance-tester、implementer、integrator、verifier、design-reviewer、security-reviewer、merge-steward、release-watcher。AI-native SDLC，依序 plan → design → build → sync → test → deliver → merge，各段對應一支 `sdlc-*.js`。
  - plan、design、test、deliver 之間有核准閘門，用 `save_run.py approve` 記錄。`--by policy` 會讀信封的實際內容（`policy_violations`：未通過條目、反作弊發現、blocking 問題、blocker／important 發現、skeptic 推翻、agent 缺件、halted），不只看 status；plan 一律由人核准。重存某段會清掉該段核准，核准要整條閘門鏈都通過。需求取消用 `save_run.py abandon`，之後不能核准也不能推送。
  - `dev-merge-steward`：可信任的合併 agent。逐條對照需求與驗證證據（綁 head SHA），一致而且 `merge_gate.py` 通過才合併；不一致就產出 followup 新需求並退回 PR（request-changes、轉 draft）。沒有 Edit／Write，永遠不 approve。
  - `dev-release-watcher`：合併後監看，歸因失敗，產出 revert 提案與凍結建議，只讀。
  - `sdlc-sync`（dev-integrator 執行）：build 之後、test 之前，以及 merge 因 PR 落後或衝突判 hold（`needs_sync`）之後跑。由 `sync_branch.py` 決定策略（profile 的 `conventions.sync`，預設 auto：已跟上不動、沒推送且沒衝突 rebase、已推送且沒衝突 merge base、有衝突走中轉分支）。中轉分支從 base 開（命名依 `conventions.sync.transit_naming`）、把分支 merge 進來解衝突，`check-transit` 擋殘留衝突標記與夾帶改動，workflow 核對回報的衝突檔與預測一致；之後用 `publish.py --supersedes` 開 PR 取代原 PR。head 變了一律重跑 `sdlc-test`，後續各段用 `branch` arg 帶中轉分支名。
- **`tpm-*`（5 個）**：planner、researcher、reviewer、skeptic、writer。事件處理，對應 `event-*.js`（包括 `event-post-merge.js`）；`tpm-reviewer` 另有 deliver 模式（`sdlc-deliver` 的四軸審查），`tpm-researcher` 另有上游模式（`event-upstream`）。
- **2 個舊名薄殼**：舊名 `dev-pr-reviewer` 已併入 `tpm-reviewer`、舊名 `tpm-upstream-analyst` 已併入 `tpm-researcher`。相容一版、下一版移除；`awf_config.py` 的 `ROLE_ALIASES` 讓舊名照新角色套規則。新 workflow 一律派新名。
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

多支 workflow 共用的程式放在 `scaffold/.claude/teams/tools/snippets/<名>.js`（目前有 `challenge`、`review-skeptic`、`layers`、`recipe`、`kernel`），workflow 裡寫片段標記 `/*@snippet:<名>*/ … /*@end*/`，由 `build.py` 把片段內容內嵌在標記之間（標記保留，可重跑）。`build.py --check` 會偵測片段和 workflow 內嵌內容的漂移。片段的規則見下一節〈Workflow 腳本的限制〉。

### methodology profile

- 格式由 `schemas/methodology.schema.json` 定義：
  - `stages`：各段的做法。
  - `requirements`：需求來源。
  - `conventions`：慣例。
  - `parallelism`：選填的工作流程參數。
- 內建兩份：`methodologies/default.json`（現行做法）與 `lightweight.json`。init 問答或 `--answers` 的結果會合併成專案的 `.claude/teams/methodology.json`。
- `stages.design.system_design`（選填）：SA／SD 方法。內建 profile 依 System Design Primer（`docs/系統設計方法.md`，CC BY 4.0，用自己的話改寫並註明出處，不貼原文）：每個需求做功能層 SD，有系統面時四步完整；sdlc-design 照它派工，並多派 skeptic 的 `system-design` lens。
- 需求來源 adapter（json、command，以及 spec-md 範例）和 `merge_gate.py` 共用。`guard.config.json` 的 `requirements` 有設時以它為準。

### 依需求類型分流（recipe）

- **定義**：profile 的 `recipes`（內建 11 個：feature、bugfix、change、hotfix、refactor、deps、docs、test-only、perf、spike、revert）與 `modifiers`（ui、security、migration、public_api、large）。每個 recipe 定義 stages、plan_mode、research_angles、design_lenses、test、deliver_axes、build_kinds、publishable、followup。
- **解析**：Python 版 `tools/recipe.py`（save_run、publish、merge_gate 用）與 JS 片段 `tools/snippets/recipe.js`（workflow 用）必須一致，`tests/check_recipes.py` 拿 `golden-set/recipes/*.json` 兩邊對跑。規則：未知 recipe 或沒有 `recipes` 的舊 profile 一律當 feature；modifier 只能加嚴；未知 modifier 回錯誤、呼叫端 halted。
- **鎖定與升級**：`sdlc-plan` 的 Triage 只給建議，人在 plan 閘門核准後鎖在 run.json；`save_run.py escalate` 只准加嚴（`merge_stricter`），新增的段之後已完成的段改成 stale。工具不自動降級。
- **profile 位置**：save_run 只認工具所在專案的 `.claude/teams/methodology.json`（`save_run.PROJECT_PROFILE`），不從 run 目錄往上找，避免 runs_dir 底下被放一份 profile 劫持解析；`--profile` 可指定，載入後跑 `schemalib.methodology_errors`。
- **回傳核對**：plan 以外的段，workflow 回傳的 recipe 要和 run.json 鎖定的一致（`save_run.recipe_mismatch`），必須帶 trace，recipe 要求的角色要在 trace 裡交件（`required_roles_missing`）。核准、publish、merge_gate 都用 profile 重算 recipe，`recipe.resolved` 比重算結果鬆就拒絕（`recipe_tamper_errors`）。
- **diff 反查**：`recipe.diff_shortfalls` 依實際改動檔分類（code、test、doc、migration、manifest、agent_config），認不出來的一律算 code；`publish.py` 拒推、`merge_gate.py --recipe <run.json>` 判 hold（只接受 plan 已核准的 run.json）。CI 範本（`agent-merge-gate.yml.tmpl`）沒有 run.json，不帶 `--recipe`，所以 diff 反查只在本機 publish／steward 的 gate 生效。
- **不可省**：每個 recipe 都有 plan 段與 intent（四個必要節加 plan_mode 專屬節，`recipe.required_sections`），plan 一律人核准，派工單一律以共同前綴引用 rules.md（challenge 片段的 `awfPrefix`）。
- **執行紀錄**：`tryAgent` 為每張派工單指定 `<run_dir>/<段>/<label>.md` 並記 trace，workflow 一律回傳 `trace: awfTrace()`；`save_run.py stage` 依 trace 產生 `執行紀錄索引.md`，紀錄檔缺了算缺件。沒 Write 的角色由信封內容代寫；tpm-reviewer、tpm-skeptic、tpm-planner、dev-release-watcher 有 Write，但 hook（`RECORD_ONLY_ROLES`）只准它們寫 runs_dir 下的 `.md`。
- **token 精簡**：派工單是「共同前綴＋任務」，共用背景放 `<run_dir>/context.md`；信封有長度上限；長輸出用 `tools/run_brief.py --log` 濃縮；`tests/check_budget.py` 對 `golden-set/token-budget.json` 檢查派工數與字數（超過 10% 失敗）。

### 執行核心、模型分級與省 context（0.4）

- **執行核心**：`tools/snippets/kernel.js`（`@requires: challenge, recipe`）。`awfAgent` 包住 `tryAgent`，做模型解析、失敗升級一次（`mode: auto` 才升；被取代的 trace 標 `superseded`）、派工數上限（`limits.max_agents`，到上限 workflow 回 halted）。**目前只有 `sdlc-plan` 用它**（`save_run.KERNEL_STAGES`），其餘 workflow 下一版改；新寫的派工一律用 `awfAgent`。
- **模型分級**：profile 的 `models`。解析在 `recipe.py` 的 `resolve_model` 與 `recipe.js` 的 `resolveModel`，兩邊必須一致（golden 的 `model_cases`）。結果＝max(AI 建議或預設, floor, modifier 的 `model_floors`)；內建 floor（`BUILTIN_MODEL_FLOORS`）profile 蓋不掉。`save_run.model_errors` 用 profile 重算，trace 的模型低於 floor 判 invalid。agent 檔的 frontmatter 仍然是 `model: inherit`，模型由派工時指定。
- **兩段式調研**：`locate:<角度>`（light，payload `locations`）→ `research:<角度>`（只讀定位到的行段）。定位缺件時該角度列 missing、不派分析。triage 的 `angles` 只能從 recipe 自己的角度挑，modifier 帶進來的不能省。
- **資料包與重用**：`tools/pack.py` 寫 `<run>/<段>/pack.md` 與 `pack-args.json`（無時間戳、可重現）；plan 段的 `reuse.json` 記各角度來源檔的 sha1，`save_run.py stage plan` 用 `verify_reuse` 重新核對、存檔後 `record_reuse`。這三個檔與 `pack.py` 只有主 session 能寫、能跑（`guard.py` 的 `PACK_STATE`、`SHARED_CONTEXT_NAMES`）。
- **知識層**：`tools/kb.py`（FTS5，退回純 Python BM25；中文 bigram）。輸出包在資料標籤裡；卡片只放標題樹與行號，不放內文。
- **讀取預算**：`plugin/hooks/read_budget.py`，由 `guard.read_budget_check` 呼叫。只套模版角色；**預設不啟用**（`limits` 沒寫 `mode` 就關閉，內建 profile 不預設 `mode`，由使用者在 init 問答、答案檔的 `read_limits` 或 `methodology.json` 決定要不要開與門檻）；任何例外都放行（I9）。狀態檔在系統暫存目錄的 `awf-read-budget/<session_id>/<agent_id>.json`（`AWF_READ_BUDGET_DIR` 可覆寫，測試用）。
- **量成本**：`tools/awf_cost.py` 讀 Claude Code 的 workflow transcript（格式沒承諾穩定，解析集中在 `read_agent`、`read_wf_meta`）。基準實跑在 `tests/bench/`（`make_fixture.py` 產生虛構專案、`run_plan.py` 真的呼叫 claude，會花 token，不在 run_all 裡）。
- 這些都是輔助：失敗只警告、不擋流程；gate 判定不看它們的輸出（調研重用例外：`verify_reuse` 不符會判 invalid，只會更嚴）。

### 平行處理

- **`sdlc-design`**：先由 dev-spec-author 出大綱（`plan-outline`）。預估子任務超過 `parallelism.split_threshold`，或有多個可以各自規劃的區塊時，拆成最多 `max_sub_plans` 個子計劃。每個子計劃由一個 dev-spec-author 平行規劃，寫在 `specs/<epic>/sub/<id>/`，不 commit；最後由一個 dev-spec-author 彙整並 commit。跨子計劃的相依寫成 `@<子計劃 id>`，由 workflow 展開；指向不存在、沒有子任務或自己的子計劃時 halted。
- **`sdlc-build`**：依子任務相依分層，同一層各自開 worktree 平行實作，每層由 dev-integrator 合併。
- **`sdlc-test`**：有子計劃時，每個子計劃一個 dev-verifier，另加一個整合分片，平行驗證後彙整成 `verifier.md`；條目 id 帶分片前綴（`<分片>:<原 id>`），彙整後的 commit 必須等於各分片共同的 HEAD（merge 的 `--reviewed-sha` 取自它）。失敗分散在多個子計劃時，各自開 worktree 平行修正，再合併。
- workflow 會對以下幾點做確定性核對，不一致就回傳 `halted: true`：子計劃的範圍與相依、彙整後的子任務清單（不能默默少掉）、彙整後的驗證結果（不能漏條目或改結果，各分片要驗同一個 HEAD）。
- 預設乾跑走不到這些分支，所以 `check_workflows.mjs` 載入 `tools/scenarios/<workflow>.mjs`（每支 workflow 一檔）用假回傳把它們走過一遍，另有各 agent 缺件的情境。改了這些流程，對應的 scenarios 檔要一起改。

### 使用者自己的 agent（擴充）

- **設定位置**：使用者用 `/agent-workflow:add-agent`（`plugin/skills/add-agent`）把自己的 agent 加進 plan、design、test、deliver 任一段，設定寫兩個地方：
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
- `agent()` 一律透過 challenge 片段的 `tryAgent` 呼叫，失敗或 throw 都當成缺件（回 null）。乾跑情境的 fake 可以給 `'THROW'` 模擬 agent 失敗。
- 多個 agent 共用同一個 worktree 時不能各自 commit（會搶 index.lock），要由最後一步統一 commit。
- 共用片段（`tools/snippets/*.js`）只定義函式：不能有頂層 `await`，不能用 `{{佔位符}}`，也不讀 workflow 的區域變數，需要的值一律用參數傳進去。片段之間有依賴時，在片段開頭寫 `// @requires: <名>`，引用它的 workflow 也必須引用被依賴的片段。違反時 `build.py` exit 1。

### 自動合併

決策記錄在 `docs/adr-agent-auto-merge.md`，信任模型寫在 `docs/自動合併信任模型.md`。

- `merge_gate.py`：確定性 gate。複核需求對照、head SHA、required checks、測試數、skip、風險等級、每日上限、白名單、凍結旗標，並寫入 `<runs_dir>/merge-audit.jsonl`。
  - `--reviewed-sha`：審查／驗證當下的 head，缺它或和 head 不符一律 hold。
  - `--merged-today`：CI 用 GitHub API 算的當日合併數（只計 `AGENT_MERGER_BOT` 合併的），和本機 audit 取大者；不是非負整數就 hold。
  - `--frozen <理由>`：CI 從 repo variable `AGENT_AUTOMERGE_FROZEN` 帶入，有給就 hold。本機的凍結檔 CI 看不到。
  - `auto_merge.allowed_skipped_checks`：skipped／neutral 預設不算通過，列在這裡的才算。
  - `upstream_authorized_by` 的值可以是字串或 `{by, link, scope, expires}`；過期或日期讀不懂都和沒有授權一樣 escalate，和 hook 一致。放寬閘門（改測試／lint／覆蓋率設定、新增壓制註解）一律 escalate；`auto_merge.carry_verdict_on_rebase`（預設關閉）開啟時，rebase 後 patch-id 相同才沿用 verdict。
- steward 合併後要讀回 PR 狀態填 `merge_state`，`sdlc-merge` 只在 `merged` 時算已合併（merge queue 可能只是排隊）。
- `publish.py`：確定性推送工具。只推 fork 的功能分支，推送前掃機敏值（含二進位與空檔的機敏檔名）、檢查整條閘門鏈到 deliver 都已核准（`--stage` 只能指定 deliver 或更後面）、開 PR，並寫稽核紀錄。
- `merge_gate.py` 解析 diff 時，rename／刪除的舊路徑也算進風險判斷，加引號的路徑會還原；標頭或行數解析不了一律 hold，所以 diff 要用 `--src-prefix=a/ --dst-prefix=b/` 產生。判定的「今天」取 `--timestamp` 與系統 UTC 日期較晚者。
- `scaffold/.github/workflows/agent-merge-gate.yml.tmpl`：給目標專案用的 CI，從受保護的 base 執行 gate，設為 required check。
- 真正的信任邊界在伺服器端（ruleset、GitHub App、CODEOWNERS），hook 只是輔助，可以被繞過。`auto_merge` 預設關閉。安全政策見 `SECURITY.md`。

### Hooks（`plugin/hooks/`）

- **入口**：只有一個，`awf_hook.py <guard-shell|guard-paths|envelope>`。設定一律透過 `awf_config.py` 讀 `${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json`，不要寫死路徑。
- **失敗語意**（`awf_hook.py` 開頭的說明為準）：
  - hook 跑不起來（找不到 Python、逾時、Windows 沒有 Git Bash）：plugin 的 `hooks.json` 設 `onFailure: continue`、timeout 10 秒，放行，不鎖死使用者。要更嚴就用 `--hooks project`。
  - 跑得起來但輸入壞掉（空、不是 JSON、缺欄位、shell 指令超過 128KB）：守門類擋（exit 2），交件類放行。128KB 上限是因為 guard 規則耗時隨指令長度約平方成長，拖到逾時就等於放行。
  - `guard.py` 或 `awf_config.py` 載入失敗：改用 `awf_hook.py` 內建的最小規則（擋 push、`gh pr merge`、`--approve`、GitHub API 寫入），其他放行。
  - 交件 hook（`envelope`）自己出錯時刻意放行，避免卡住所有 subagent；信封仍由 workflow 的 `schema` 與 `save_run.py stage` 驗證。
- **`guard-paths` 的範圍**：Edit、Write、NotebookEdit、Read、Grep。Read、Grep 讀機敏檔（`.env`、金鑰、憑證）一律擋，主 session 也擋；`acceptance_test_dirs` 裡的檔案 dev-implementer 不能寫。
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
- plugin 有 4 支 command：`/sdlc`、`/event`、`/brief`、`/demo-check`，都是薄殼。
- 其他入口是 skill，用 `/agent-workflow:<skill>` 叫用（沒有同名指令時可省略前綴）。不要再新增和 skill 同名的 command。
  - `/agent-workflow:init-workflow`：先在對話中確認使用者的習慣，再跑 `init.py --answers`。
  - `/agent-workflow:add-agent`：把使用者自己的 agent 加進 SDLC。
  - `/agent-workflow:correct`：把一則教訓變成最高可行層級的強制機制，並在 rules.md〈專案規則〉維護「規則 ↔ 強制方式」對照表。

## 改動時的檢查清單

- **新增佔位符**：`workflow.config.example`、`init.py` 和 README 的佔位符表要一起改。smoke_init 會檢查產出裡有沒有殘留的 `{{`。
- **新增 `dev-*`／`tpm-*` agent**：
  - 照既有段落寫：切分理由、職責、硬性約束、工作方式、交件格式。
  - 設 `model: inherit` 和 `skills`。只讀的角色不給 Edit／Write。
  - `check_agents.py` 的 `CORE` 與 `awf_config.py` 的 `RESERVED_ROLES` 要一起改（`check_agents.py` 會核對 CORE ⊆ RESERVED_ROLES）；需要套用 guard 時，在 `role_map` 加對應。
  - 合併或改名角色時，舊名留在 `ALIASES`、`ROLE_ALIASES` 與 `RESERVED_ROLES` 相容一版，並留薄殼 agent 檔。
  - 更新 README、本檔、`scaffold/.claude/teams/README.md` 的角色數與名單，`tests/check_docs.py` 會核對。
- **新增或刪除 command**：`tests/check_docs.py` 會核對 README 等文件的 command 名單；不要和 skill 同名。
- **新增 workflow**：在 `tpm-team` skill 的事件對照表加一列，並加進 `tests/smoke_init.py` 的清單。
- **改分支流程（scenarios）**：乾跑走不到的分支，在 `scaffold/.claude/teams/tools/scenarios/<workflow>.mjs` 補情境（`check_workflows.mjs` 自動載入，共用的 helper 在 `scenarios/_helpers.mjs`）。
- **改 snippet**：改了 `scaffold/.claude/teams/tools/snippets/*.js`（workflow 共用片段）要重跑 `build.py`（`--check` 會抓漂移），不要手改內嵌進 workflow 的內容；並確認 `tools/scenarios/*.mjs` 用到的 agent label 沒變（情境靠 label 覆寫假回傳，label 一變情境就默默失效）。
- **新增或修改 payload schema**：重跑 `build.py`；在 `golden-set/valid|invalid/` 補正例、反例，有語意規則的另補 `semantic` 反例；語意規則寫在 `schemalib.py`。
- **改擴充 agent 機制**：四支 sdlc workflow 的 `EXT` 片段要一致；`tools/scenarios/sdlc-*.mjs` 的擴充情境、`smoke_init.py` 的 `check_extensions`、`plugin/hooks/tests/run.py` 的擴充 agent 案例要一起改。
- **新增 hook 規則**：在 `plugin/hooks/tests/run.py` 補一個該擋、一個該放的測試，Bash 與 PowerShell 各一。
- **改 profile 格式**：`methodology.schema.json`、兩份內建 profile、`docs/自訂與擴充.md` 的欄位表要一起改。
- **改 recipe 或 modifier**：`tools/recipe.py` 的 `BUILTIN_RECIPES`／`BUILTIN_MODIFIERS`、`tools/snippets/recipe.js`、兩份內建 profile、`tools/scenarios/_helpers.mjs` 的 `RECIPE_STAGES`（手抄的段表）要一起改；在 `golden-set/recipes/` 補案例，跑 `tests/check_recipes.py`。新增的 recipe 不能省 plan 段，會寫程式的要有 build、deliver、merge（`schemalib.methodology_errors`）。
- **改模型分級**：`recipe.py`（`BUILTIN_TIER_MODELS`、`BUILTIN_MODEL_FLOORS`、`resolve_model`）與 `snippets/recipe.js` 一起改，在 `golden-set/recipes/` 補 `model_cases`，跑 `tests/check_recipes.py`；floor 只能加、不能拿掉。
- **改執行核心（kernel 片段）**：重跑 `build.py`；`tools/scenarios/sdlc-plan.mjs` 的 `V04` 情境（升級、上限、重用、角度數）要一起改；`save_run.py` 的 `_retried_ok`、`model_errors` 依賴 trace 的 `model`、`tier`、`superseded`、`attempt` 欄位。
- **改讀取預算**：`plugin/hooks/tests/run.py` 的 A2 一段補該擋與該放（Bash 與 PowerShell 各一），並確認效能檢查（計數路徑 50ms 內）仍通過；任何新的失敗路徑都要放行。
- **改 pack.py／kb.py**：各自的 `--selftest`；`pack.md` 的內容不能有時間戳或絕對路徑以外會變動的東西（可重現是 `tests/bench/run_plan.py --selftest` 的檢查項）。
- **改派工單或派工數**：跑 `tests/check_budget.py`；變多是預期的，才用 `--update` 更新 `golden-set/token-budget.json`，並在 commit 說明理由。
- **新增會寫紀錄的唯讀角色**：加進 `awf_config.py` 的 `RECORD_ONLY_ROLES` 與 `check_agents.py` 的對應檢查，hook 測試補該擋（寫 repo、run.json、merge-audit）與該放（寫 runs_dir 的 `.md`）。
- **新增 guard.config 欄位**：`guard.config.json.tmpl` 的 `_說明`、`awf_config.py` 的預設值、`docs/自動合併信任模型.md` 或 `docs/自訂與擴充.md` 一起改；`init.py --upgrade` 會把新鍵補進既有專案。
- **版本（VERSION）**：發佈前遞增 `plugin/.claude-plugin/plugin.json` 與 `.claude-plugin/marketplace.json`（`metadata.version` 與 `plugins[].version`）。init 寫的 `.claude/teams/VERSION` 取自 plugin.json，三處一致由 `tests/smoke_init.py` 檢查。
- **CHANGELOG**：每次發佈在 `CHANGELOG.md` 補一節，破壞性變更（角色改名、command 刪除、hook 語意）與升級步驟要寫清楚。
- **完成前**：每次都跑 `python3 tests/run_all.py`。

完整的擴充說明見 `docs/自訂與擴充.md`，實戰教訓見 `docs/踩坑手冊.md`。
