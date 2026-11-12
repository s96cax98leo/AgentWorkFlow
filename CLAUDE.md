# CLAUDE.md

這份文件是給在這個 repo 工作的 Claude Code 看的。給使用者看的說明在 `README.md`。

## 這個 repo 是什麼

AgentWorkFlow 是**模版本身**，不是套用了模版的專案。它分兩部分，透過兩條管道發佈：

| 部分 | 內容 | 發佈方式 | 更新方式 |
|---|---|---|---|
| `plugin/` | agents、skills、commands、hooks、workflow 範本（`workflow-templates/`）（plugin 名稱 `agent-workflow`，agent 命名空間 `agent-workflow:`） | `.claude-plugin/marketplace.json`（marketplace 名稱 `agentworkflow`），所有專案共用 | 使用者執行 `claude plugin update agent-workflow@agentworkflow`（或 `/plugin`），再 `/reload-plugins`；只跑 marketplace update 不會更新 plugin |
| `scaffold/` | `.claude/teams/`（rules、schemas、methodologies、tools、golden-set）、`.github/workflows/agent-merge-gate.yml.tmpl`、settings、`CLAUDE.md.tmpl`、`.mcp.json.tmpl`、`_runs/` | `init.py` 複製進目標專案：把 `{{佔位符}}` 換成 `workflow.config`、init 問答或 `--answers` 的值，並去掉 `.tmpl` 副檔名；另寫 `.claude/teams/VERSION` 與 `.claude/teams/workflow.config` | 已 init 的專案**不會**自動更新，要跑 `init.py --upgrade`（`--adopt-new-fields` 補 profile 新欄位） |

workflow 範本放在 `plugin/workflow-templates/`（不叫 `workflows/`，避免被 Claude Code 當成 plugin workflow 自動登記），**不複製進專案**：專案的 `awf.py next` 每段用範本＋`workflow.config` 的佔位符值＋`methodology.json` 產生 `<run_dir>/.wf/<名>.js`，主 session 用 `Workflow({scriptPath})` 執行。工具仍然留在專案（agent 檔、hook 訊息、派工單都引用固定路徑 `.claude/teams/tools/`）。

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
python3 tests/check_compile.py       # 所有 Python 檔（含 hook）用現在的 Python 編譯得過
python3 tests/bench/run_plan.py --selftest   # 基準實跑腳本不花 token 的部分；不帶 --selftest 會真的呼叫 claude（數百萬 token），只在發佈前手動跑
python3 init.py --target <專案> [--hooks plugin|project] [--answers <答案.json>] [--defaults] [--upgrade [--adopt-new-fields]]
```

下面這些工具在 scaffold 裡，要在**已 init 的專案根目錄**執行。smoke_init 就是在暫存專案裡跑它們。

```bash
python3 .claude/teams/tools/awf.py --selftest                                              # 狀態機（start／next／done／approve／render）
python3 .claude/teams/tools/awf.py render --all --out <目錄> [--plugin-dir <AgentWorkFlow>/plugin]   # 產生 12 支 workflow 腳本
node .claude/teams/tools/check_workflows.mjs --agents-dir <AgentWorkFlow>/plugin/agents --wf-dir <目錄>   # 乾跑所有 workflow（含 tools/scenarios/*.mjs 的情境）
python3 .claude/teams/tools/build.py [--check]   # 只處理專案的覆寫檔（.claude/workflows/*.js）；在模版 repo 裡處理 plugin/workflow-templates/
python3 .claude/teams/tools/check_agents.py --plugin-dir <AgentWorkFlow>/plugin           # 檢查 tpm-*／dev-* agent 格式（16 個：14 個核心角色＋2 個舊名薄殼）
uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py              # schema 正反例
python3 .claude/teams/tools/merge_gate.py --selftest                                       # 確定性 merge gate
python3 .claude/teams/tools/publish.py --selftest                                          # 確定性推送工具
python3 .claude/teams/tools/sync_branch.py --selftest                                      # 跟上 base 的確定性工具
python3 .claude/teams/tools/pack.py --selftest                                             # 資料包與調研重用
python3 .claude/teams/tools/kb.py --selftest                                               # 知識層
python3 .claude/teams/tools/locate.py --selftest                                           # 工具先定位
python3 .claude/teams/tools/awf_cost.py --selftest                                         # workflow 的 token 與時間
python3 .claude/teams/tools/awf_watch.py --selftest                                        # watcher（進度事件、通知、狀態列）
python3 .claude/teams/tools/awf_doctor.py --selftest                                       # 安裝檢查（awf.py doctor）
```

在 repo 內也可以直接驗，例如 `python3 scaffold/.claude/teams/tools/build.py --check`（處理 `plugin/workflow-templates/`），路徑會依檔案位置推算。範本帶佔位符，不能直接乾跑；乾跑由 `tests/smoke_init.py` 在 init 過的暫存專案裡做（`awf.py render --all` 後 `check_workflows.mjs --wf-dir`）。

計畫型的工作用可驗證 checklist：`python3 scaffold/.claude/teams/tools/checklist.py check <checklist.md> [--id <前綴>] [--write]`。打勾以驗證指令的實際執行結果為準。checklist 放在根目錄的 `plans/`（本機，不進版控）。

## 架構

### agent（`plugin/agents/`，agent 檔共 22 個）

14 個核心角色（`check_agents.py` 的 `CORE`）、2 個舊名薄殼（`ALIASES`）、6 個網站分析 agent。

- **`dev-*`（9 個）**：spec-author、acceptance-tester、implementer、integrator、verifier、design-reviewer、security-reviewer、merge-steward、release-watcher。AI-native SDLC，依序 plan → design → build → sync → test → deliver → merge，各段對應一支 `sdlc-*.js`。
  - plan、design、test、deliver 之間有核准閘門，用 `save_run.py approve` 記錄。`--by policy` 會讀信封的實際內容（`policy_violations`：未通過條目、反作弊發現、blocking 問題、blocker／important 發現、skeptic 推翻、agent 缺件、halted），不只看 status；plan 一律由人核准。重存某段會清掉該段核准，核准要整條閘門鏈都通過。需求取消用 `save_run.py abandon`，之後不能核准也不能推送。
  - `dev-merge-steward`：可信任的合併 agent。逐條對照需求與驗證證據（綁 head SHA），一致而且 `merge_gate.py` 通過才合併；不一致就產出 followup 新需求並退回 PR（request-changes、轉 draft）。沒有 Edit／Write，永遠不 approve。
  - `dev-release-watcher`：合併後監看，歸因失敗，產出 revert 提案與凍結建議，只讀。
  - `sdlc-sync`（dev-integrator 執行）：build 之後、test 之前，以及 merge 因 PR 落後或衝突判 hold（`needs_sync`）之後跑。由 `sync_branch.py` 決定策略（profile 的 `conventions.sync`，預設 auto：已跟上不動、沒推送且沒衝突 rebase、已推送且沒衝突 merge base、有衝突走中轉分支）。中轉分支從 base 開（命名依 `conventions.sync.transit_naming`）、把分支 merge 進來解衝突，`check-transit` 擋殘留衝突標記與夾帶改動，workflow 核對回報的衝突檔與預測一致；之後用 `publish.py --supersedes` 開 PR 取代原 PR。head 變了一律重跑 `sdlc-test`，後續各段用 `branch` arg 帶中轉分支名。
- **`tpm-*`（5 個）**：planner、researcher、reviewer、skeptic、writer。事件處理，對應 `event.js`（會議、決策落地、demo 前檢查、需求變動、上游變動與 profile 自訂的類型共用）與 `event-brief.js`、`event-pr-review.js`、`event-post-merge.js`；`tpm-reviewer` 另有 deliver 模式（`sdlc-deliver` 的四軸審查），`tpm-researcher` 另有上游模式（`event.js` 的 upstream 類型）。
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

- **執行核心**：`tools/snippets/kernel.js`（`@requires: challenge, recipe`）。`awfAgent` 包住 `tryAgent`，做模型解析、失敗升級一次（`mode: auto` 才升；被取代的 trace 標 `superseded`）、派工數上限。**12 支 workflow 的派工都走它**（`save_run.KERNEL_STAGES` 是全部七段）；片段裡的派工（`challengeList`、`verifyFindings`）用 challenge 片段的 `awfDispatch`，啟用核心後也經過核心。上限＝`limits.max_agents`（沒寫用 40），design 另加 `3×max_sub_plans`、build 另加 `4×子任務數`、test 另加 `4×子計劃數`；到上限不再派、trace 記一筆 `capped: true`，`save_run.process_trace` 列成缺件並說明。`AWF_NO_RETRY`（dev-integrator、dev-merge-steward：有副作用）與 `retry: false` 的派工不自動重派。新寫的派工一律用 `awfAgent`。
- **模型分級**：profile 的 `models`。解析在 `recipe.py` 的 `resolve_model` 與 `recipe.js` 的 `resolveModel`，兩邊必須一致（golden 的 `model_cases`）。結果＝max(AI 建議或預設, floor, modifier 的 `model_floors`)；內建 floor（`BUILTIN_MODEL_FLOORS`）profile 蓋不掉。`save_run.model_errors` 用 profile 重算，trace 的模型低於 floor 判 invalid。agent 檔的 frontmatter 仍然是 `model: inherit`，模型由派工時指定。
- **兩段式調研**：`locate:<角度>`（light，payload `locations`）→ `research:<角度>`（只讀定位到的行段）。定位缺件時該角度列 missing、不派分析。triage 的 `angles` 只能從 recipe 自己的角度挑，modifier 帶進來的不能省。
- **資料包與重用**：`tools/pack.py` 寫 `<run>/<段>/pack.md` 與 `pack-args.json`（無時間戳、可重現）；plan 段的 `reuse.json` 記各角度來源檔的 sha1，`save_run.py stage plan` 用 `verify_reuse` 重新核對、存檔後 `record_reuse`。這三個檔與 `pack.py` 只有主 session 能寫、能跑（`guard.py` 的 `PACK_STATE`、`SHARED_CONTEXT_NAMES`）。
- **決策記憶（0.5）**：`save_run.py approve --answer`／`decide` 把人的答覆附加到 `<runs_dir>/decisions.jsonl` 並重寫 `決策記憶.md`（`record_decisions`、`decision_errors`：只有人能記）；讀取與篩選在 `pack.py`（`relevant_decisions`：範圍、過期、同題取代、和需求的用詞比對），由 `build_pack` 寫進〈既有決定〉。兩個檔在 `guard.py` 的 `SHARED_CONTEXT_NAMES`，team agent 不能寫。
- **子任務資料包（0.5）**：`pack.py build` 為每個子任務寫 `<run>/build/<id>/pack.md`（spec 小節行段取自子任務的 `spec_refs`、依路徑對到的限制文件、相依子任務的介面），路徑放 `pack-args.json` 的 `task_packs`；`sdlc-build` 的 `taskPack` 只採用「自己那一份、在 build 目錄底下」的路徑。`spec_refs` 對不到記 `spec_ref_errors`，`sdlc-build` 開工前 halted。`pack.path_overlap` 要和 challenge 片段的 `awfPathOverlap` 判斷一致。
- **工具先定位（0.5）**：`tools/locate.py` 用需求全文的詞對 repo 的檔案視窗做 BM25（分詞同 kb.py），由 `pack.py plan` 呼叫，結果放 `pack-args.json` 的 `located`（`{角度: {locations, enough}}`）。文件角度（specs-adr、requirement）命中夠（至少 3 段、涵蓋三成查詢詞）就不派 `locate:*`；程式角度（`SEED_ONLY`）只當種子並沿引用關係擴展一層，定位 agent 照派但上限減半。`sdlc-plan` 的 `toolLocated` 會丟掉絕對路徑、`..`、行號不對的項目；回傳多 `located_by`。可重現、任何錯誤都回空。
- **知識層**：`tools/kb.py`（FTS5，退回純 Python BM25；中文 bigram）。輸出包在資料標籤裡；卡片只放標題樹與行號，不放內文。
- **讀取預算**：`plugin/hooks/read_budget.py`，由 `guard.read_budget_check` 呼叫。只套模版角色；**預設不啟用**（`limits` 沒寫 `mode` 就關閉，內建 profile 不預設 `mode`，由使用者在 init 問答、答案檔的 `read_limits` 或 `methodology.json` 決定要不要開與門檻）；任何例外都放行（I9）。狀態檔在系統暫存目錄的 `awf-read-budget/<session_id>/<agent_id>.json`（`AWF_READ_BUDGET_DIR` 可覆寫，測試用）。
- **量成本**：`tools/awf_cost.py` 讀 Claude Code 的 workflow transcript（格式沒承諾穩定，解析集中在 `read_agent`、`read_wf_meta`）。基準實跑在 `tests/bench/`（`make_fixture.py` 產生虛構專案、`run_plan.py` 真的呼叫 claude，會花 token，不在 run_all 裡）。
- **doctor（0.5）**：`tools/awf_doctor.py`（`awf.py doctor` 轉給它）。只讀、不連網：環境、plugin 與專案版本、安裝來源（`installed_plugins.json`、`known_marketplaces.json`，格式沒承諾穩定，讀不懂就略過）、hook 編譯與重複註冊、`schemalib.check_doc` 驗 methodology、覆寫檔版本。每項 `{id, level, msg, fix}`；有 error 才 exit 1。別的專案的安裝不算衝突（和 `awf.find_plugin` 一致）。
- **watcher（0.5）**：`tools/awf_watch.py`（`awf.py watch`、`awf.py statusline` 轉給它）。唯讀合併 run.json、執行紀錄檔、workflow transcript（`awf_cost.tail_agent` 增量讀），把前後兩份快照的差異（`diff_events`）附加到 `<run>/progress.jsonl`（格式 `schemas/progress-event.schema.json`），記憶在 `<run>/.watch-state.json`。`workflow_stalled` 只回報、不處理。profile 的 `progress.notify` 預設不設；有設時只送 `NOTIFY_FIELDS` 的欄位，命中 `SECRETISH` 整筆不送。閘門工具不讀這兩個檔（`save_run.py --selftest` 的 B4.2 案例）。整合方式見 `docs/監控與整合.md`。
- 這些都是輔助：失敗只警告、不擋流程；gate 判定不看它們的輸出（調研重用例外：`verify_reuse` 不符會判 invalid，只會更嚴）。

### 狀態機與 workflow 範本（0.5）

- **`tools/awf.py`**：主 session 每段只做 `next`（回一個動作：`run_workflow`、`gate`、`need_input`、`manual`、`halted`、`done`）與 `done`（找到 Workflow 的回傳、轉給 `save_run.py stage`）。`approve` 轉給 `save_run.py approve`、時間自動取。它只排順序、組參數（`stage_args`）、跑 `pack.py`、產生腳本，**不放寬任何閘門**。recipe 的 `gate_mode`（`recipe.gate_mode()`，只有 Python 版，workflow 用不到）是 plan-only 時，`try_policy` 在非 plan 的閘門先試 `save_run.py approve --by policy`（條件完全由 save_run 判），不過就交人；政策核准的段與沒人看過的暫定做法（`awf.auto_approved`、`awf.unconfirmed`）由 `carry` 帶到下一次要人出面的動作。等待中的段記在 run.json 的 `awf.pending`，Workflow 的 run id 記在 `stages[段].wf_run`（同一個 session 重跑 halted 的段時輸出 `resume_from`）。
- **產生腳本**：`render` 只內嵌 methodology（片段與 schema 已由 `build.py` 內嵌在範本裡），args 內嵌成 `Object.assign(<JSON>, args || {})`。缺佔位符值、擴充 agent 沒登記都拒絕。
- **找 plugin**：`--plugin-dir` → `AWF_PLUGIN_DIR`／`CLAUDE_PLUGIN_ROOT` → 模版 repo 自己 → `~/.claude/plugins/installed_plugins.json`（別的專案的安裝不算）。
- **覆寫檔**：專案 `.claude/workflows/<名>.js` 結尾有 `// awf-override: <版本>` 才採用（版本不同會警告）；沒有標記的同名檔是舊副本，不採用（只有找不到 plugin 時才退回用）。`init.py --with-workflows` 放覆寫檔。
- **舊副本整理**：`plugin/workflow-templates/legacy.json` 記已發佈各版範本的分段指紋（`tests/gen_legacy_templates.py` 從 git 歷史產生，發佈新版後重跑）；`init.py --upgrade` 的 `tidy_old_workflows` 移除原樣的舊副本、保留改過的並說明，要在 `build.py` 之前做。
- **拆工檢查**：`save_run.plan_check_issues`（design／plan 存檔時算、`approve --by policy` 重算；人工核准只警告）。
- **接手摘要**：`awf.py handoff` 寫 `<run>/接手摘要.md`（`handoff_text`：只讀、沒有時間戳、可重現）。
- **守門**：team agent 不能寫 `.claude/workflows/` 與 `.wf/`（`guard.team_config_path`、`TEAM_CONFIG_TARGET`），不能跑 `awf.py`（`--selftest` 除外）。
- **測試**：`awf.py --selftest`、`smoke_init.py` 的 `check_with_workflows`（產生的腳本和 init＋build.py 內嵌的逐字相同）與升級四種情況、hook 測試的 B1 案例。

### 事件 workflow（0.5）

- **`event.js`**：形狀相同的五種事件（meeting、decision、demo-check、reqs-change、upstream）共用一支。事件類型是資料：範本裡的 `BUILTIN_EVENTS`，加上 profile 的 `events`（同名時整筆取代）。一筆定義＝`args`、`context`、`before`（前置步驟）、`gather`（各角度平行）、`challenge`、`after`、`write`、`returns`；文字裡的 `{參數名}`、`{參數名|預設}` 由 `fill` 換掉。角色固定是 tpm-researcher → tpm-skeptic → tpm-writer，派工都走 `awfAgent`。角度最多 8 個、前後步驟各 3 步，超過就拒絕執行。
- **狀態機**：`awf.py` 的 `EVENT_TYPES`（內建類型的必要參數，要和 `BUILTIN_EVENTS` 一致，selftest 對照）、`event_route`、`event_required`、`profile_events`。事件類型就是段名，`stage_args` 自動帶 `event=<類型>`。自訂類型不能蓋掉 brief、pr-review、post-merge、merge（`schemalib.EVENT_RESERVED`）。
- **schema 與語意規則**：`methodology.schema.json` 的 `events`；`schemalib.methodology_errors`（保留名、角度 key 與步驟 label 重複、`data.from` 指不到、回傳鍵撞名）。
- **情境**：`tools/scenarios/event.mjs` 合併 `_event-<類型>.mjs`（底線開頭的檔不會被當成某支 workflow 的情境檔）並補 `args.event`；「類型：<名>」情境給 `check_budget.py` 量（基準的鍵是 `event.js#<類型>`）。情境可以用 `throws: /正規式/` 宣告「這組參數應該被拒絕執行」。
- **舊副本**：plugin 找得到但已經沒有同名範本時（例如併掉的五支），`find_template` 不退回用沒有標記的舊副本，直接報錯並說明改用 `event.js`。

### 限制文件（0.5）

- profile 的 `constraints[]`（`id`、`file`、`stages`、`paths`、`trace`、`check`、`note`）：使用者指定哪份文件套用到哪些段、哪些路徑。語意規則在 `schemalib.methodology_errors`（id 不重複、file 與 paths 不能是絕對路徑或含 `..`、不能是機敏檔）。
- **派工端**在 challenge 片段：`awfConstraints(profile, stage[, owns])`、`awfConstraintLines`、`awfConstraintRule`。八支 sdlc workflow 的 `awfInit` 都帶 `constraints: awfConstraints(MP, '<段>')`，由共同前綴列出；`sdlc-build` 的 `taskConstraints` 把有 `paths` 的限制只給擁有的路徑對得上的子任務。
- **閘門端**在 `save_run.py`：`constraint_issues`（文件不存在、`trace: table` 缺〈限制對照〉列或有「偏離」、`check` 指令沒過）。`stage` 存檔時算並記 `constraint_issues`／`constraint_checks`；`approve --by policy` 重算（不採信 run.json）；人工核准只警告。代寫的紀錄不算有對照。
- 測試：`tests/check_kernel.py`（片段）、`tools/scenarios/sdlc-build.mjs` 與 `sdlc-design.mjs` 的「限制文件」情境、`save_run.py --selftest` 的 B7 案例、golden 的 `methodology-constraints*`。

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

- **入口**：只有一個，`awf_hook.py <guard-shell|guard-paths|envelope|session-start>`。設定一律透過 `awf_config.py` 讀 `${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json`，不要寫死路徑。
- **失敗語意**（`awf_hook.py` 開頭的說明為準）：
  - hook 跑不起來（找不到 Python、逾時、Windows 沒有 Git Bash）：plugin 的 `hooks.json` 設 `onFailure: continue`、timeout 10 秒，放行，不鎖死使用者。要更嚴就用 `--hooks project`。
  - 跑得起來但輸入壞掉（空、不是 JSON、缺欄位、shell 指令超過 128KB）：守門類擋（exit 2），交件類放行。128KB 上限是因為 guard 規則耗時隨指令長度約平方成長，拖到逾時就等於放行。
  - `guard.py` 或 `awf_config.py` 載入失敗：改用 `awf_hook.py` 內建的最小規則（擋 push、`gh pr merge`、`--approve`、GitHub API 寫入），其他放行。
  - 交件 hook（`envelope`）自己出錯時刻意放行，避免卡住所有 subagent；信封仍由 workflow 的 `schema` 與 `save_run.py stage` 驗證。
- **`session-start`（0.5）**：SessionStart 時比對 plugin 版本與專案 `.claude/teams/VERSION`，不一致印一行 JSON（`systemMessage` 與 `additionalContext`）。只是提示：永遠 exit 0，沒套用、讀不到、輸入壞掉、任何例外都不印；版本字串只收單純的版本格式（會進對話）。複製進專案的 hook 旁邊沒有 plugin.json，不提示。測試在 `tests/test_entry.py` 的第 6 段。
- **`guard-paths` 的範圍**：Edit、Write、NotebookEdit、Read、Grep。`guard.config.json` 的 `linked_repos`（工作區以外的我方 repo，`guard.linked_rel`）讓 dev-* 可以改裡面的檔、tpm-writer 可以寫它的 specs，其餘規則照舊；模版角色可以寫這個 session 的 scratchpad（`guard.in_scratchpad`：系統暫存目錄底下的 `<session_id>/scratchpad/`）。Read、Grep 讀機敏檔（`.env`、金鑰、憑證）一律擋，主 session 也擋；`acceptance_test_dirs` 裡的檔案 dev-implementer 不能寫。
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
- `plugin/skills/tpm-team/SKILL.md` 只寫 `awf.py next` → 照做 → `done` 的迴圈（不超過 60 行、描述不超過 200 字，`VP B1.5` 檢查）；事件對照表、recipe、閘門規則與手動執行在同目錄的 `reference/*.md`。
- plugin 有 4 支 command：`/sdlc`、`/event`、`/brief`、`/demo-check`，都是薄殼。
- 其他入口是 skill，用 `/agent-workflow:<skill>` 叫用（沒有同名指令時可省略前綴）。不要再新增和 skill 同名的 command。
  - `/agent-workflow:init-workflow`：先在對話中確認使用者的習慣，再跑 `init.py --answers`。
  - `/agent-workflow:add-agent`：把使用者自己的 agent 加進 SDLC。
  - `/agent-workflow:correct`：把一則教訓變成最高可行層級的強制機制，並在 rules.md〈專案規則〉維護「規則 ↔ 強制方式」對照表。
  - `/agent-workflow:status`、`cost`、`kb`、`doctor`：薄殼，各自只叫 `awf.py watch --once`、`awf_cost.py`、`kb.py`、`awf.py doctor` 並說明怎麼回報。`status`、`cost`、`doctor` 和 Claude Code 內建指令同名，文件一律寫完整名稱。

## 改動時的檢查清單

- **新增佔位符**：`workflow.config.example`、`init.py` 和 README 的佔位符表要一起改。smoke_init 會檢查產出裡有沒有殘留的 `{{`。
- **新增 `dev-*`／`tpm-*` agent**：
  - 照既有段落寫：切分理由、職責、硬性約束、工作方式、交件格式。
  - 設 `model: inherit` 和 `skills`。只讀的角色不給 Edit／Write。
  - `check_agents.py` 的 `CORE` 與 `awf_config.py` 的 `RESERVED_ROLES` 要一起改（`check_agents.py` 會核對 CORE ⊆ RESERVED_ROLES）；需要套用 guard 時，在 `role_map` 加對應。
  - 合併或改名角色時，舊名留在 `ALIASES`、`ROLE_ALIASES` 與 `RESERVED_ROLES` 相容一版，並留薄殼 agent 檔。
  - 更新 README、本檔、`scaffold/.claude/teams/README.md` 的角色數與名單，`tests/check_docs.py` 會核對。
- **新增或刪除 command**：`tests/check_docs.py` 會核對 README 等文件的 command 名單；不要和 skill 同名。
- **新增事件類型**：形狀是「多角度 → skeptic → writer」的，在 `event.js` 的 `BUILTIN_EVENTS` 加一筆（使用者則在 profile 的 `events` 加），並改 `awf.py` 的 `EVENT_TYPES`、`reference/事件.md`、`docs/事件處理.md`，在 `tools/scenarios/` 加 `_event-<類型>.mjs` 並登記到 `event.mjs`；形狀不同才另寫一支。
- **新增 workflow**：範本放 `plugin/workflow-templates/`；在 `tpm-team` skill 的 `reference/事件.md` 對照表加一列，加進 `tests/smoke_init.py` 的清單與 `awf.py` 的 `EVENTS`／`REQUIRED`；重跑 `tests/gen_legacy_templates.py` 不需要（它只記已發佈的舊版）。
- **改分支流程（scenarios）**：乾跑走不到的分支，在 `scaffold/.claude/teams/tools/scenarios/<workflow>.mjs` 補情境（`check_workflows.mjs` 自動載入，共用的 helper 在 `scenarios/_helpers.mjs`）。
- **改 snippet**：改了 `scaffold/.claude/teams/tools/snippets/*.js`（workflow 共用片段）要重跑 `build.py`（`--check` 會抓漂移），不要手改內嵌進 workflow 的內容；並確認 `tools/scenarios/*.mjs` 用到的 agent label 沒變（情境靠 label 覆寫假回傳，label 一變情境就默默失效）。
- **新增或修改 payload schema**：重跑 `build.py`；在 `golden-set/valid|invalid/` 補正例、反例，有語意規則的另補 `semantic` 反例；語意規則寫在 `schemalib.py`。
- **改 awf.py**：`--selftest`；動作的欄位名（`action`、`script`、`then`、`approve`、`retry`、`resume_from`、`need`、`highlights`）改了，`plugin/skills/tpm-team/SKILL.md` 的動作表要一起改；改了 `render` 要確認 `smoke_init.py` 的逐字相同檢查仍通過。
- **改擴充 agent 機制**：四支 sdlc workflow 的 `EXT` 片段要一致；`tools/scenarios/sdlc-*.mjs` 的擴充情境、`smoke_init.py` 的 `check_extensions`、`plugin/hooks/tests/run.py` 的擴充 agent 案例要一起改。
- **新增 hook 規則**：在 `plugin/hooks/tests/run.py` 補一個該擋、一個該放的測試，Bash 與 PowerShell 各一。
- **改 profile 格式**：`methodology.schema.json`、兩份內建 profile、`docs/自訂與擴充.md` 的欄位表要一起改。
- **改 recipe 或 modifier**：`tools/recipe.py` 的 `BUILTIN_RECIPES`／`BUILTIN_MODIFIERS`、`tools/snippets/recipe.js`、兩份內建 profile、`tools/scenarios/_helpers.mjs` 的 `RECIPE_STAGES`（手抄的段表）要一起改；在 `golden-set/recipes/` 補案例，跑 `tests/check_recipes.py`。新增的 recipe 不能省 plan 段，會寫程式的要有 build、deliver、merge（`schemalib.methodology_errors`）。
- **改模型分級**：`recipe.py`（`BUILTIN_TIER_MODELS`、`BUILTIN_MODEL_FLOORS`、`resolve_model`）與 `snippets/recipe.js` 一起改，在 `golden-set/recipes/` 補 `model_cases`，跑 `tests/check_recipes.py`；floor 只能加、不能拿掉。
- **改執行核心（kernel 片段）**：重跑 `build.py`；`tools/scenarios/sdlc-plan.mjs` 的 `V04` 情境（升級、上限、重用、角度數）要一起改；`save_run.py` 的 `_retried_ok`、`model_errors` 依賴 trace 的 `model`、`tier`、`superseded`、`attempt` 欄位。
- **改讀取預算**：`plugin/hooks/tests/run.py` 的 A2 一段補該擋與該放（Bash 與 PowerShell 各一），並確認效能檢查（計數路徑 50ms 內）仍通過；任何新的失敗路徑都要放行。
- **改 pack.py／kb.py／locate.py**：各自的 `--selftest`；改了子任務資料包的路徑或 `pack-args.json` 的鍵，`sdlc-build.js` 的 `taskPack`／`REF_ERRS` 與 `tools/scenarios/sdlc-build.mjs` 的「子任務資料包」情境要一起改；改了 `locate.py` 的輸出格式，`sdlc-plan.js` 的 `toolLocated` 與 `tools/scenarios/sdlc-plan.mjs` 的「工具先定位」情境要一起改；`pack.md` 的內容不能有時間戳或絕對路徑以外會變動的東西（可重現是 `tests/bench/run_plan.py --selftest` 的檢查項）。
- **改 doctor**：`awf_doctor.py --selftest`；新的檢查項要有修法（`fix`），而且單項出錯不能讓整支失敗。所有 Python 檔的語法由 `tests/check_compile.py` 檢查（CI 在 3.9 與最新版各跑一次）。
- **改 watcher**：`awf_watch.py --selftest`；事件類型或欄位改了，`schemas/progress-event.schema.json`、`methodology.schema.json` 的 `progress.notify.events`、`docs/監控與整合.md` 的事件表要一起改；送給外部指令的欄位只能加進 `NOTIFY_FIELDS`（不放需求內容、摘要、路徑）；任何失敗都只警告。
- **改派工單或派工數**：跑 `tests/check_budget.py`；變多是預期的，才用 `--update` 更新 `golden-set/token-budget.json`，並在 commit 說明理由。
- **新增會寫紀錄的唯讀角色**：加進 `awf_config.py` 的 `RECORD_ONLY_ROLES` 與 `check_agents.py` 的對應檢查，hook 測試補該擋（寫 repo、run.json、merge-audit）與該放（寫 runs_dir 的 `.md`）。
- **新增 guard.config 欄位**：`guard.config.json.tmpl` 的 `_說明`、`awf_config.py` 的預設值、`docs/自動合併信任模型.md` 或 `docs/自訂與擴充.md` 一起改；`init.py --upgrade` 會把新鍵補進既有專案。
- **版本（VERSION）**：發佈前遞增 `plugin/.claude-plugin/plugin.json` 與 `.claude-plugin/marketplace.json`（`metadata.version` 與 `plugins[].version`）。init 寫的 `.claude/teams/VERSION` 取自 plugin.json，三處一致由 `tests/smoke_init.py` 檢查。
- **CHANGELOG**：每次發佈在 `CHANGELOG.md` 補一節，破壞性變更（角色改名、command 刪除、hook 語意）與升級步驟要寫清楚。
- **完成前**：每次都跑 `python3 tests/run_all.py`。

完整的擴充說明見 `docs/自訂與擴充.md`，實戰教訓見 `docs/踩坑手冊.md`。
