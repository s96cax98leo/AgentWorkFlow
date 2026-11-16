# CLAUDE.md

這份文件是給在這個 repo 工作的 Claude Code 看的。給使用者看的說明在 `README.md`。

## 這個 repo 是什麼

AgentWorkFlow 是**模版本身**，不是套用了模版的專案。全部要發佈的東西都在 `plugin/`（從 marketplace 安裝的 plugin 只有這個目錄，它同時是 `awf init` 的模版根目錄），分兩部分、兩條管道進到使用者手上：

| 部分 | 內容 | 發佈方式 | 更新方式 |
|---|---|---|---|
| `plugin/`（`scaffold/` 以外） | agents、skills、commands、hooks、workflow 範本（`workflow-templates/`，含共用片段 `snippets/`、乾跑情境 `scenarios/`、乾跑腳本 `check_workflows.mjs`）、`workflow.config.example`、`legacy-tools.json`、執行檔 `awf`（`bin/`：啟動器＋各平台的執行檔）（plugin 名稱 `agent-workflow`，agent 命名空間 `agent-workflow:`） | `.claude-plugin/marketplace.json`（marketplace 名稱 `agentworkflow`），所有專案共用 | 使用者執行 `claude plugin update agent-workflow@agentworkflow`（或 `/plugin`），再 `/reload-plugins`；只跑 marketplace update 不會更新 plugin |
| `plugin/scaffold/` | `.claude/teams/`（rules、schemas、methodologies、deliverables、golden-set、守門設定）、`.github/workflows/agent-merge-gate.yml.tmpl`、settings、`CLAUDE.md.tmpl`、`.mcp.json.tmpl`、`_runs/` | `awf init` 複製進目標專案：把 `{{佔位符}}` 換成 `workflow.config`、init 問答或 `--answers` 的值，並去掉 `.tmpl` 副檔名；另寫 `.claude/teams/VERSION` 與 `.claude/teams/workflow.config` | 已 init 的專案**不會**自動更新，要跑 `awf init --upgrade`（`--adopt-new-fields` 補 profile 新欄位） |

workflow 範本放在 `plugin/workflow-templates/`（不叫 `workflows/`，避免被 Claude Code 當成 plugin workflow 自動登記），**不複製進專案**：專案的 `awf next` 每段用範本＋`workflow.config` 的佔位符值＋`methodology.json` 產生 `<run_dir>/.wf/<名>.js`，主 session 用 `Workflow({scriptPath})` 執行。工具不在專案裡：全部是單一執行檔 `awf`（Rust，原始碼在 `crates/`：`awf-core` 放全部邏輯、`awf` 是入口、`xtask` 是建置與檢查），由 plugin 的 `bin/` 提供；agent 檔、hook 訊息、派工單一律寫 `awf <子指令>`，不帶路徑。init 只放設定、schema、profile、rules、golden-set。

「模版在哪」只有一套解析（`crates/awf-core/src/template.rs`）：模版根目錄＝plugin 目錄；`AWF_TEMPLATE_DIR`（給 plugin 目錄或 repo 根都可以）→ `AWF_PLUGIN_DIR`／`CLAUDE_PLUGIN_ROOT` → 從執行檔往上找（`<plugin>/bin/` 底下用那個 plugin；開發時的 `target/…/awf` 用 repo 的 `plugin/`）。要找模版的地方一律呼叫它，不要各寫各的。

repo 裡沒有 Python：工具、測試、維護檢查都是 Rust（`crates/`），只有驗證 workflow 用 Node。`tests/` 只放資料（回歸基準 `parity/`、hook 案例 `hooks/`、去客戶化清單、基準實跑的需求）。建置與發佈見 `docs/建置與發佈.md`。

## 原則

- **模版只管工作流程。** 開發規範（TDD、DDD、SDD 等）和慣例（code style、commit 格式、分支命名、PR 大小、文件格式）由使用者定義，寫在 methodology profile。agent、workflow、rules 不寫死任何做法；特定做法只能從 profile 讀，或明寫為「預設 profile 範例」。
- **語言**：所有文件、註解、agent 提示都用繁體中文。agent 的輸出語言用佔位符 `{{OUTPUT_LANGUAGE}}`，不要寫死。
- **跨平台**：工具與測試是 Rust，只有驗證 workflow 用 Node。要能在 macOS、Linux、Windows（PowerShell）執行，不新增 shell 腳本（`plugin/bin/awf` 啟動器是唯一例外）。
- **不破壞使用者的既有設定**：init 對既有專案只新增檔案。`CLAUDE.md` 只追加 `@` 引用，settings 與 `.mcp.json` 做深度合併。`--force` 只覆寫模版檔，整份覆寫使用者檔要加 `--overwrite-user-files`。merge gate CI 要加 `--with-merge-gate` 才會放。
- **去客戶化**：不寫任何真實客戶、人名、帳號、內部代號，範例一律用虛構的「示範客戶 ACME」。禁用清單有兩份，都是一行一個 ERE 正規式：
  - `tests/scrub_terms.txt`：公開的通用規則（本機路徑、憑證形狀、個人信箱）。
  - `tests/scrub_terms.local.txt`：真實名稱，**不進版控**。真實名稱只能寫在這份，寫進公開清單就等於公開了。
  - 掃描範圍是整個 repo 的檔案內容**與路徑名**，略過根目錄的本機目錄 `_tpm`、`graft`、`.claude`、`.idea`。
- **gate 只會更嚴**：確定性工具碰到輸入缺漏一律 hold 或 halted 交人，不做樂觀推定。

## 指令

在 repo 根目錄執行（用 cargo 前先 `. "$HOME/.cargo/env"`）。**全部檢查＝下面前兩個指令，兩個都過才算通過，每次改完都要跑**：

```bash
cargo test --workspace              # 單元測試、回歸基準（tests/parity/）、hook 案例（tests/hooks/）、init 煙霧測試（crates/awf/tests/smoke_init.rs）
cargo xtask check-all               # 下面各項 xtask 檢查、片段漂移（build --check）、check-agents、各子指令的 selftest、bench --selftest、workflow 乾跑；[--verbose] [--only <名稱片段>]
cargo build --all-targets           # 不能有警告
```

`cargo xtask <子指令>`（只給這個 repo 用的維護工具，原始碼在 `crates/xtask/`）：

```bash
cargo xtask scrub [路徑…]           # 去客戶化掃描（檔案內容與路徑名），可只掃指定路徑
cargo xtask golden [--verbose]      # golden set：schema 的正例要零錯誤、反例要有錯誤（專案裡用 awf golden）
cargo xtask check-docs              # 文件裡的角色、workflow、command 數量與名單要和實際檔案一致
cargo xtask check-recipes           # recipe 解析：Rust 與 snippets/recipe.js 對 golden-set/recipes 的結果相同
cargo xtask check-kernel            # 執行核心片段的單元檢查
cargo xtask check-patterns          # pattern 庫與自訂腳本端到端
cargo xtask check-budget [--update] # 派工數與派工單字數對 golden-set/token-budget.json（超過 10% 失敗）
cargo xtask check-supply            # 供應鏈：action 釘 SHA、版控沒有機敏檔、相依來源與允許清單、Node 腳本只用內建模組
cargo xtask check-launchers         # 啟動器（plugin/bin/awf、awf.cmd）
cargo xtask gen-legacy [--write]    # 舊版指紋（plugin/workflow-templates/legacy.json、plugin/legacy-tools.json）；預設只比對，要完整的 git 歷史
cargo xtask bench --selftest        # 基準實跑不花 token 的部分；不帶 --selftest 會真的呼叫 claude（數百萬 token），只在發佈前手動跑
cargo xtask dist [--target <triple>]            # 建置發佈用的執行檔
cargo xtask checksums [--write|--check]          # 執行檔的 sha256 與原始碼雜湊
```

`awf` 的子指令。在這個 repo 開發時，`awf` 換成 `cargo run -q -p awf --`，或 `cargo build --release` 後把 `AWF_BIN` 指到 `target/release/awf`。有 `--selftest` 的子指令不需要已 init 的專案：在這個 repo 裡從 `plugin/scaffold/` 底下跑（有幾支要讀 `.claude/teams/` 的 schema 與 profile；`cargo xtask check-all` 就是這樣全部跑一遍）。其餘要在**已 init 的專案根目錄**執行。

```bash
awf init --target <專案> [--hooks plugin|project] [--answers <答案.json>] [--defaults] [--upgrade [--adopt-new-fields]]
awf init --detect --target <專案> [--json]    # 只偵測、只印（訪談用）；awf init --target <專案> --reconfigure [--answers <檔>] 改已 init 專案的設定 [--source <模版>]
awf render --all --out <目錄> [--plugin-dir <AgentWorkFlow>/plugin]   # 產生 12 支 workflow 腳本
node plugin/workflow-templates/check_workflows.mjs --project <專案> --wf-dir <目錄>   # 乾跑所有 workflow（含 plugin/workflow-templates/scenarios/*.mjs 的情境）
awf build [--check]   # 只處理專案的覆寫檔（.claude/workflows/*.js）；在模版 repo 裡處理 plugin/workflow-templates/
awf check-agents --plugin-dir <AgentWorkFlow>/plugin           # 檢查 tpm-*／dev-* agent 格式（16 個：14 個核心角色＋2 個舊名薄殼）
awf state --selftest                                              # 狀態機（start／next／done／approve／render）
awf save-run --selftest                                         # run 的存檔、核准與閘門
awf merge-gate --selftest                                       # 確定性 merge gate
awf publish --selftest                                          # 確定性推送工具
awf sync-branch --selftest                                      # 跟上 base 的確定性工具
awf recipe --selftest                                           # recipe 與模型分級的解析
awf pack --selftest                                             # 資料包與調研重用
awf kb --selftest                                               # 知識層
awf locate --selftest                                           # 工具先定位
awf deliverables --selftest                                     # 交付文件
awf cost --selftest                                         # workflow 的 token 與時間
awf watch --selftest                                        # watcher（進度事件、通知、狀態列）
awf doctor --selftest                                       # 安裝檢查（awf doctor）
awf script --selftest                                       # 自訂腳本的靜態檢查
awf program --selftest                                      # 多需求排程（awf program status／next）
awf checklist --selftest、awf spec --selftest、awf run-brief --selftest、awf tokens --selftest   # 其餘有 selftest 的子指令
awf golden [--dir <golden-set 目錄>] [--verbose]                 # 專案的 golden-set（schema 正反例）
awf tokens check [--all] <檔案或目錄>…、awf tokens verify [tokens.css] [正本檔]   # 設計系統 skill 的色碼掃描與 tokens 核對
awf ci render [--write]、awf ci check、awf ci --selftest          # 專案的測試／lint CI（profile 的 ci；預設不啟用；--write 只由主 session 跑）
awf scrub [路徑…] [--json]、awf scrub --selftest                 # 專案自己訂的去客戶化掃描（profile 的 scrub；預設不啟用）
```

在 repo 內也可以直接驗，例如 `awf build --check`（處理 `plugin/workflow-templates/`），路徑會依檔案位置推算。範本帶佔位符，不能直接乾跑；乾跑由 `cargo xtask check-all` 的最後一步與 `crates/awf/tests/smoke_init.rs` 在 init 過的暫存專案裡做（`awf render --all` 後 `check_workflows.mjs --project <專案> --wf-dir <目錄>`）。

回歸基準（`tests/parity/*.expected.json`）與 hook 案例（`tests/hooks/cases.jsonl`）只在行為確實該變的時候改，更新方式見 `tests/parity/README.md` 與 `tests/hooks/README.md`；守門與閘門的判定只能變嚴。

計畫型的工作用可驗證 checklist：`awf checklist check <checklist.md> [--id <前綴>] [--write]`。打勾以驗證指令的實際執行結果為準。checklist 放在根目錄的 `plans/`（本機，不進版控）。

## 架構

### agent（`plugin/agents/`，agent 檔共 22 個）

14 個核心角色（`awf check-agents` 的 `CORE`）、2 個舊名薄殼（`ALIASES`）、6 個網站分析 agent。

- **`dev-*`（9 個）**：spec-author、acceptance-tester、implementer、integrator、verifier、design-reviewer、security-reviewer、merge-steward、release-watcher。AI-native SDLC，依序 plan → design → build → sync → test → deliver → merge，各段對應一支 `sdlc-*.js`。
  - plan、design、test、deliver 之間有核准閘門，用 `awf save-run approve` 記錄。`--by policy` 會讀信封的實際內容（`policy_violations`：未通過條目、反作弊發現、blocking 問題、blocker／important 發現、skeptic 推翻、agent 缺件、halted），不只看 status；plan 一律由人核准。重存某段會清掉該段核准，核准要整條閘門鏈都通過。需求取消用 `awf save-run abandon`，之後不能核准也不能推送。
  - `dev-merge-steward`：可信任的合併 agent。逐條對照需求與驗證證據（綁 head SHA），一致而且 `awf merge-gate` 通過才合併；不一致就產出 followup 新需求並退回 PR（request-changes、轉 draft）。沒有 Edit／Write，永遠不 approve。
  - `dev-release-watcher`：合併後監看，歸因失敗，產出 revert 提案與凍結建議，只讀。
  - `sdlc-sync`（dev-integrator 執行）：build 之後、test 之前，以及 merge 因 PR 落後或衝突判 hold（`needs_sync`）之後跑。由 `awf sync-branch` 決定策略（profile 的 `conventions.sync`，預設 auto：已跟上不動、沒推送且沒衝突 rebase、已推送且沒衝突 merge base、有衝突走中轉分支）。中轉分支從 base 開（命名依 `conventions.sync.transit_naming`）、把分支 merge 進來解衝突，`check-transit` 擋殘留衝突標記與夾帶改動，workflow 核對回報的衝突檔與預測一致；之後用 `awf publish --supersedes` 開 PR 取代原 PR。head 變了一律重跑 `sdlc-test`，後續各段用 `branch` arg 帶中轉分支名。
- **`tpm-*`（5 個）**：planner、researcher、reviewer、skeptic、writer。事件處理，對應 `event.js`（會議、決策落地、demo 前檢查、需求變動、上游變動與 profile 自訂的類型共用）與 `event-brief.js`、`event-pr-review.js`、`event-post-merge.js`；`tpm-reviewer` 另有 deliver 模式（`sdlc-deliver` 的四軸審查），`tpm-researcher` 另有上游模式（`event.js` 的 upstream 類型）。
- **2 個舊名薄殼**：舊名 `dev-pr-reviewer` 已併入 `tpm-reviewer`、舊名 `tpm-upstream-analyst` 已併入 `tpm-researcher`。相容一版、下一版移除；`crates/awf-core/src/config.rs` 的 `ROLE_ALIASES` 讓舊名照新角色套規則。新 workflow 一律派新名。
- **網站分析組（6 個）**：tech-architect、infra-analyst、biz-analyst、perf-auditor、competitor-scout、lead-synthesizer。由 skills 驅動，**不交信封**，也不受角色寫入範圍限制。

### 交件信封

`tpm-*`／`dev-*` 一律用 JSON 信封回報：status、summary、artifacts、evidence、confidence、open_questions、derived_work、payload。

- 信封的定義在 `plugin/scaffold/.claude/teams/schemas/envelope.schema.json`，各種 payload 的定義在 `schemas/payloads/*.schema.json`，例如 `subtasks`、`plan-outline`、`criteria`、`merge-decision`。
- 驗證分兩處：
  - `awf hook envelope`（`crates/awf-core/src/hooks/envelope.rs`）在 SubagentStop、TaskCompleted 時驗證。
  - `crates/awf-core/src/schema.rs` 是 hook、save-run、merge-gate 共用的驗證器。schema 表達不了的語意規則也寫在這裡，例如子任務的 owns 不能重疊、子計劃的範圍不能重疊或相依循環、decision=merge 時不能有 mismatch。

### schema 與 profile 內嵌

Workflow 腳本不能 import 檔案，所以 workflow 裡只寫標記：`/*@schema:<payload>*/ null /*@end*/` 和 `/*@methodology*/ null /*@end*/`，再由 `awf build` 換成完整的 JSON。profile 取自專案的 `.claude/teams/methodology.json`，scaffold 裡保持 null。

**改了 schema 或 profile 就要重跑 `awf build`**，並在 `golden-set/valid|invalid/` 補正例與反例。不要手改內嵌進去的 JSON。

多支 workflow 共用的程式放在 `plugin/workflow-templates/snippets/<名>.js`（目前有 `challenge`、`review-skeptic`、`layers`、`recipe`、`kernel`、`patterns`），workflow 裡寫片段標記 `/*@snippet:<名>*/ … /*@end*/`，由 `awf build` 把片段內容內嵌在標記之間（標記保留，可重跑）。`awf build --check` 會偵測片段和 workflow 內嵌內容的漂移。片段的規則見下一節〈Workflow 腳本的限制〉。

### methodology profile

- 格式由 `schemas/methodology.schema.json` 定義：
  - `stages`：各段的做法。
  - `requirements`：需求來源。
  - `conventions`：慣例。
  - `parallelism`：選填的工作流程參數。
- 內建兩份：`methodologies/default.json`（現行做法）與 `lightweight.json`。init 問答或 `--answers` 的結果會合併成專案的 `.claude/teams/methodology.json`。
- `stages.design.system_design`（選填）：SA／SD 方法。內建 profile 依 System Design Primer（`docs/系統設計方法.md`，CC BY 4.0，用自己的話改寫並註明出處，不貼原文）：每個需求做功能層 SD，有系統面時四步完整；sdlc-design 照它派工，並多派 skeptic 的 `system-design` lens。
- 需求來源 adapter（json、command，以及 spec-md 範例）和 `awf merge-gate` 共用。`guard.config.json` 的 `requirements` 有設時以它為準。

### 依需求類型分流（recipe）

- **定義**：profile 的 `recipes`（內建 11 個：feature、bugfix、change、hotfix、refactor、deps、docs、test-only、perf、spike、revert）與 `modifiers`（ui、security、migration、public_api、large）。每個 recipe 定義 stages、plan_mode、research_angles、design_lenses、test、deliver_axes、build_kinds、publishable、followup。
- **解析**：工具端 `crates/awf-core/src/recipe.rs`（`awf recipe`；save-run、publish、merge-gate 用）與 JS 片段 `plugin/workflow-templates/snippets/recipe.js`（workflow 用）必須一致，`cargo xtask check-recipes` 拿 `golden-set/recipes/*.json` 兩邊對跑。規則：未知 recipe 或沒有 `recipes` 的舊 profile 一律當 feature；modifier 只能加嚴；未知 modifier 回錯誤、呼叫端 halted。
- **鎖定與升級**：`sdlc-plan` 的 Triage 只給建議，人在 plan 閘門核准後鎖在 run.json；`awf save-run escalate` 只准加嚴（`merge_stricter`），新增的段之後已完成的段改成 stale。工具不自動降級。
- **profile 位置**：save_run 只認工具所在專案的 `.claude/teams/methodology.json`（`save_run::Ctx` 的 `profile`），不從 run 目錄往上找，避免 runs_dir 底下被放一份 profile 劫持解析；`--profile` 可指定，載入後跑 `schema::methodology_errors`。
- **回傳核對**：plan 以外的段，workflow 回傳的 recipe 要和 run.json 鎖定的一致（`save_run::recipe_mismatch`），必須帶 trace，recipe 要求的角色要在 trace 裡交件（`required_roles_missing`）。核准、publish、merge_gate 都用 profile 重算 recipe，`recipe.resolved` 比重算結果鬆就拒絕（`recipe_tamper_errors`）。
- **diff 反查**：`recipe::diff_shortfalls` 依實際改動檔分類（code、test、doc、migration、manifest、agent_config），認不出來的一律算 code；`awf publish` 拒推、`awf merge-gate --recipe <run.json>` 判 hold（只接受 plan 已核准的 run.json）。CI 範本（`agent-merge-gate.yml.tmpl`）沒有 run.json，不帶 `--recipe`，所以 diff 反查只在本機 publish／steward 的 gate 生效。
- **不可省**：每個 recipe 都有 plan 段與 intent（四個必要節加 plan_mode 專屬節，`recipe::required_sections`），plan 一律人核准，派工單一律以共同前綴引用 rules.md（challenge 片段的 `awfPrefix`）。
- **執行紀錄**：`tryAgent` 為每張派工單指定 `<run_dir>/<段>/<label>.md` 並記 trace，workflow 一律回傳 `trace: awfTrace()`；`awf save-run stage` 依 trace 產生 `執行紀錄索引.md`，紀錄檔缺了算缺件。沒 Write 的角色由信封內容代寫；tpm-reviewer、tpm-skeptic、tpm-planner、dev-release-watcher 有 Write，但 hook（`RECORD_ONLY_ROLES`）只准它們寫 runs_dir 下的 `.md`。
- **token 精簡**：派工單是「共同前綴＋任務」，共用背景放 `<run_dir>/context.md`；信封有長度上限；長輸出用 `awf run-brief --log` 濃縮；`cargo xtask check-budget` 對 `golden-set/token-budget.json` 檢查派工數與字數（超過 10% 失敗）。

### 執行核心、模型分級與省 context（0.4）

- **執行核心**：`plugin/workflow-templates/snippets/kernel.js`（`@requires: challenge, recipe`）。`awfAgent` 包住 `tryAgent`，做模型解析、失敗升級一次（`mode: auto` 才升；被取代的 trace 標 `superseded`）、派工數上限。**12 支 workflow 的派工都走它**（`save_run::KERNEL_STAGES` 是全部七段）；片段裡的派工（`challengeList`、`verifyFindings`）用 challenge 片段的 `awfDispatch`，啟用核心後也經過核心。上限＝`limits.max_agents`（沒寫用 40），design 另加 `3×max_sub_plans`、build 另加 `4×子任務數`、test 另加 `4×子計劃數`；到上限不再派、trace 記一筆 `capped: true`，`save_run::process_trace` 列成缺件並說明。`AWF_NO_RETRY`（dev-integrator、dev-merge-steward：有副作用）與 `retry: false` 的派工不自動重派。新寫的派工一律用 `awfAgent`。
- **pattern 庫（0.5）**：`plugin/workflow-templates/snippets/patterns.js`（`@requires: challenge, recipe, kernel`）。六個函式 `awfFanOut`、`awfClassifyAct`、`awfAdversarial`、`awfGenerateFilter`、`awfTournament`、`awfLoop`，都走 `awfAgent`、都要給 `max`（沒給 throw）；超過上限、缺件、跑滿輪數一律回 `halted`。內建的 12 支範本沒有引用它（各自的流程已經寫好），目前給自訂腳本用。測試在 `cargo xtask check-patterns`。
- **自訂腳本（0.5）**：`awf next --script <檔>`（`custom_action`、`render_custom`）只開放 design、build、deliver（`script::STAGES`）。`awf script` 做靜態檢查（`check`：直接用 `agent()`、未登記角色、無上限迴圈、return 沒帶 trace、改寫核心）並把 preamble 接在 meta 後面；核心的 `roles` 白名單在執行時再擋一次。`assemble` 把腳本內容包成函式：用參數遮蔽 `agent`、`tryAgent`、`awfState` 等名字（`SHADOWED`），回傳的 `stage`、`recipe`、`trace` 由外層覆寫（靜態檢查只是提早發現，這層才是擋）。`awf done` 把 `custom_script`（來源與 sha1）放進結果，`save_run::policy_violations` 看到就拒絕政策核准。排序與閘門鏈照舊。
- **非標準結果一律交人（0.5）**：覆寫檔或舊副本跑的段（`override_script`）、`done --output` 手動給的結果（`manual_output`）、自訂腳本（`custom_script`）都不能政策核准。標記記在 run.json 的 `awf.nonstandard[段]`，要等標準腳本的回傳從 workflow 紀錄取得才解除。`find_wf_meta` 不採用修改時間在未來的紀錄，同一支腳本在這次 next 之後有兩筆以上就不猜、交人。`stage_args` 對 `subtasks`、`sub_plans`、`branch` 以前段結果為準（`--set` 蓋不掉）。
- **build 波次（0.5）**：子任務的 `wave`（`subtasks.schema.json`；`schema::semantic_errors` 擋「依賴比自己晚的波次」）。`sdlc-build` 依波次、波次內依相依分層（`layerWave`）；`parallelism.wave_pause` 開啟時每波做完回 halted（`wave_pause: true`、`remaining_subtasks`），由 `awf` 既有的續跑接下去。沒有標 wave 時行為不變。
- **模型分級**：profile 的 `models`。解析在 `recipe.rs` 的 `resolve_model` 與 `recipe.js` 的 `resolveModel`，兩邊必須一致（golden 的 `model_cases`）。結果＝max(AI 建議或預設, floor, modifier 的 `model_floors`)；內建 floor（`BUILTIN_MODEL_FLOORS`）profile 蓋不掉。`save_run::model_errors` 用 profile 重算，trace 的模型低於 floor 判 invalid。agent 檔的 frontmatter 仍然是 `model: inherit`，模型由派工時指定。
- **兩段式調研**：`locate:<角度>`（light，payload `locations`）→ `research:<角度>`（只讀定位到的行段）。定位缺件時該角度列 missing、不派分析。triage 的 `angles` 只能從 recipe 自己的角度挑，modifier 帶進來的不能省。
- **資料包與重用**：`awf pack` 寫 `<run>/<段>/pack.md` 與 `pack-args.json`（無時間戳、可重現）；plan 段的 `reuse.json` 記各角度來源檔的 sha1，`awf save-run stage plan` 用 `verify_reuse` 重新核對、存檔後 `record_reuse`。這三個檔與 `awf pack` 只有主 session 能寫、能跑（`crates/awf-core/src/hooks/guard.rs` 的 `PACK_STATE`、`SHARED_CONTEXT_NAMES`）。
- **決策記憶（0.5）**：`awf save-run approve --answer`／`decide` 把人的答覆附加到 `<runs_dir>/decisions.jsonl` 並重寫 `決策記憶.md`（`record_decisions`、`decision_errors`：只有人能記）；讀取與篩選在 `awf pack`（`relevant_decisions`：範圍、過期、同題取代、和需求的用詞比對），由 `build_pack` 寫進〈既有決定〉。兩個檔在 `crates/awf-core/src/hooks/guard.rs` 的 `SHARED_CONTEXT_NAMES`，team agent 不能寫。
- **子任務資料包（0.5）**：`awf pack build` 為每個子任務寫 `<run>/build/<id>/pack.md`（spec 小節行段取自子任務的 `spec_refs`、依路徑對到的限制文件、相依子任務的介面），路徑放 `pack-args.json` 的 `task_packs`；`sdlc-build` 的 `taskPack` 只採用「自己那一份、在 build 目錄底下」的路徑。`spec_refs` 對不到記 `spec_ref_errors`，`sdlc-build` 開工前 halted。`pack::path_overlap` 要和 challenge 片段的 `awfPathOverlap` 判斷一致。
- **工具先定位（0.5）**：`awf locate` 用需求全文的詞對 repo 的檔案視窗做 BM25（分詞同 awf kb），由 `awf pack plan` 呼叫，結果放 `pack-args.json` 的 `located`（`{角度: {locations, enough}}`）。文件角度（specs-adr、requirement）命中夠（至少 3 段、涵蓋三成查詢詞）就不派 `locate:*`；程式角度（`SEED_ONLY`）只當種子並沿引用關係擴展一層，定位 agent 照派但上限減半。`sdlc-plan` 的 `toolLocated` 會丟掉絕對路徑、`..`、行號不對的項目；回傳多 `located_by`。可重現、任何錯誤都回空。
- **知識層**：`awf kb`（FTS5，退回內建的 BM25；中文 bigram）。輸出包在資料標籤裡；卡片只放標題樹與行號，不放內文。
- **讀取預算**：`crates/awf-core/src/hooks/read_budget.rs`（`check_read`、`check_shell`），由 hook 的入口（`crates/awf-core/src/hooks/mod.rs`）呼叫。只套模版角色；**預設不啟用**（`limits` 沒寫 `mode` 就關閉，內建 profile 不預設 `mode`，由使用者在 init 問答、答案檔的 `read_limits` 或 `methodology.json` 決定要不要開與門檻）；任何例外都放行（I9）。狀態檔在系統暫存目錄的 `awf-read-budget/<session_id>/<agent_id>.json`（`AWF_READ_BUDGET_DIR` 可覆寫，測試用）。
- **量成本**：`awf cost` 讀 Claude Code 的 workflow transcript（格式沒承諾穩定，解析集中在 `read_agent`、`read_wf_meta`）。基準實跑是 `cargo xtask bench`（`crates/xtask/src/bench_fixture.rs` 產生虛構專案、`bench.rs` 真的呼叫 claude，會花 token，check-all 只跑它的 `--selftest`；需求在 `tests/bench/requirement.md`）。
- **doctor（0.5）**：`awf doctor`（`crates/awf-core/src/doctor.rs`）。只讀、不連網：環境、plugin 與專案版本、安裝來源（`installed_plugins.json`、`known_marketplaces.json`，格式沒承諾穩定，讀不懂就略過）、hook 編譯與重複註冊、`schema::check_doc` 驗 methodology、覆寫檔版本。每項 `{id, level, msg, fix}`；有 error 才 exit 1。別的專案的安裝不算衝突（和 `state::find_plugin` 一致）。1.0 另有體檢五項，只提示、附修法（info 或 warn，沒事時不列）：`models.mode` 是 inherit、知識庫沒建（只在 profile 有 `knowledge` 或專案有 `docs/`、`openspec/` 時才提示）、有 `openspec/` 但需求來源不是它、需求來源在工作區外而 `linked_repos` 沒涵蓋、最近的 run 沒有走 `awf next`。
- **watcher（0.5）**：`awf watch`、`awf statusline`（`crates/awf-core/src/watch.rs`）。唯讀合併 run.json、執行紀錄檔、workflow transcript（`cost::tail_agent` 增量讀），把前後兩份快照的差異（`diff_events`）附加到 `<run>/progress.jsonl`（格式 `schemas/progress-event.schema.json`），記憶在 `<run>/.watch-state.json`。`workflow_stalled` 只回報、不處理。profile 的 `progress.notify` 預設不設；有設時只送 `NOTIFY_FIELDS` 的欄位，命中 `SECRETISH` 整筆不送。閘門工具不讀這兩個檔（`awf save-run --selftest` 的 B4.2 案例）。整合方式見 `docs/監控與整合.md`。
- 這些都是輔助：失敗只警告、不擋流程；gate 判定不看它們的輸出（調研重用例外：`verify_reuse` 不符會判 invalid，只會更嚴）。

### 狀態機與 workflow 範本（0.5）

- **狀態機 `awf`（`crates/awf-core/src/state/`）**：主 session 每段只做 `next`（回一個動作：`run_workflow`、`gate`、`need_input`、`manual`、`halted`、`done`）與 `done`（找到 Workflow 的回傳、轉給 `awf save-run stage`）。`approve` 轉給 `awf save-run approve`、時間自動取。它只排順序、組參數（`stage_args`）、跑 `awf pack`、產生腳本，**不放寬任何閘門**。recipe 的 `gate_mode`（`recipe::gate_mode()`，只有工具端有，workflow 用不到）是 plan-only 時，`try_policy` 在非 plan 的閘門先試 `awf save-run approve --by policy`（條件完全由 save_run 判），不過就交人；政策核准的段與沒人看過的暫定做法（`awf.auto_approved`、`awf.unconfirmed`）由 `carry` 帶到下一次要人出面的動作。等待中的段記在 run.json 的 `awf.pending`，Workflow 的 run id 記在 `stages[段].wf_run`（同一個 session 重跑 halted 的段時輸出 `resume_from`）。
- **產生腳本**：`render` 只內嵌 methodology（片段與 schema 已由 `awf build` 內嵌在範本裡），args 內嵌成 `Object.assign(<JSON>, args || {})`。缺佔位符值、擴充 agent 沒登記都拒絕。
- **找 plugin**：`--plugin-dir` → `AWF_PLUGIN_DIR`／`CLAUDE_PLUGIN_ROOT` → 模版 repo 自己 → `~/.claude/plugins/installed_plugins.json`（別的專案的安裝不算）。
- **覆寫檔**：專案 `.claude/workflows/<名>.js` 結尾有 `// awf-override: <版本>` 才採用（版本不同會警告）；沒有標記的同名檔是舊副本，不採用（只有找不到 plugin 時才退回用）。`awf init --with-workflows` 放覆寫檔。
- **舊副本整理**：`plugin/workflow-templates/legacy.json` 記已發佈各版範本的分段指紋（`cargo xtask gen-legacy --write` 從 git 歷史產生，發佈新版後重跑）；`awf init --upgrade` 的 `tidy_old_workflows` 移除原樣的舊副本、保留改過的並說明，要在 `awf build` 之前做。
- **拆工檢查**：`save_run::plan_check_issues`（design／plan 存檔時算、`approve --by policy` 重算；人工核准只警告）。
- **接手摘要**：`awf handoff` 寫 `<run>/接手摘要.md`（`handoff_text`：只讀、沒有時間戳、可重現）。
- **守門**：team agent 不能寫 `.claude/workflows/` 與 `.wf/`（`hooks::guard_paths::team_config_path`、`TEAM_CONFIG_TARGET`），不能跑 `awf`（`--selftest` 除外）。
- **測試**：`awf state --selftest`、`crates/awf/tests/smoke_init.rs` 的 `with_workflows`（產生的腳本和 init＋awf build 內嵌的逐字相同）與升級四種情況、hook 測試的 B1 案例。

### 事件 workflow（0.5）

- **`event.js`**：形狀相同的五種事件（meeting、decision、demo-check、reqs-change、upstream）共用一支。事件類型是資料：範本裡的 `BUILTIN_EVENTS`，加上 profile 的 `events`（同名時整筆取代）。一筆定義＝`args`、`context`、`before`（前置步驟）、`gather`（各角度平行）、`challenge`、`after`、`write`、`returns`；文字裡的 `{參數名}`、`{參數名|預設}` 由 `fill` 換掉。角色固定是 tpm-researcher → tpm-skeptic → tpm-writer，派工都走 `awfAgent`。角度最多 8 個、前後步驟各 3 步，超過就拒絕執行。
- **狀態機**：`crates/awf-core/src/state/mod.rs` 的 `EVENT_TYPES`（內建類型的必要參數，要和 `BUILTIN_EVENTS` 一致，selftest 對照）、`event_route`、`event_required`、`profile_events`。事件類型就是段名，`stage_args` 自動帶 `event=<類型>`。自訂類型不能蓋掉 brief、pr-review、post-merge、merge（`schema::EVENT_RESERVED`）。
- **schema 與語意規則**：`methodology.schema.json` 的 `events`；`schema::methodology_errors`（保留名、角度 key 與步驟 label 重複、`data.from` 指不到、回傳鍵撞名）。
- **情境**：`plugin/workflow-templates/scenarios/event.mjs` 合併 `_event-<類型>.mjs`（底線開頭的檔不會被當成某支 workflow 的情境檔）並補 `args.event`；「類型：<名>」情境給 `cargo xtask check-budget` 量（基準的鍵是 `event.js#<類型>`）。情境可以用 `throws: /正規式/` 宣告「這組參數應該被拒絕執行」。
- **舊副本**：plugin 找得到但已經沒有同名範本時（例如併掉的五支），`find_template` 不退回用沒有標記的舊副本，直接報錯並說明改用 `event.js`。

### 限制文件（0.5）

- profile 的 `constraints[]`（`id`、`file`、`stages`、`paths`、`trace`、`check`、`note`）：使用者指定哪份文件套用到哪些段、哪些路徑。語意規則在 `schema::methodology_errors`（id 不重複、file 與 paths 不能是絕對路徑或含 `..`、不能是機敏檔）。
- **派工端**在 challenge 片段：`awfConstraints(profile, stage[, owns])`、`awfConstraintLines`、`awfConstraintRule`。八支 sdlc workflow 的 `awfInit` 都帶 `constraints: awfConstraints(MP, '<段>')`，由共同前綴列出；`sdlc-build` 的 `taskConstraints` 把有 `paths` 的限制只給擁有的路徑對得上的子任務。
- **閘門端**在 `awf save-run`：`constraint_issues`（文件不存在、`trace: table` 缺〈限制對照〉列或有「偏離」、`check` 指令沒過）。`stage` 存檔時算並記 `constraint_issues`／`constraint_checks`；`approve --by policy` 重算（不採信 run.json）；人工核准只警告。代寫的紀錄不算有對照。
- 測試：`cargo xtask check-kernel`（片段）、`plugin/workflow-templates/scenarios/sdlc-build.mjs` 與 `sdlc-design.mjs` 的「限制文件」情境、`awf save-run --selftest` 的 B7 案例、golden 的 `methodology-constraints*`。

### 過程保存與交付文件（0.6）

- **結案摘要**：`awf save-run close`／`abandon` 寫 `<run>/archive/結案摘要.md`（`archive_text`：可重現、不讀 transcript；失敗只警告），`awf save-run archive <run_dir>` 重寫。`awf kb` 在 runs_dir 底下有 run 時，預設來源加上 `*/archive/結案摘要.md`（`run_sources`；調研紀錄是 agent 寫的、不收）。team agent 不能寫 run 目錄的 `archive/`（守門規則）。摘要開頭註明哪些節是 agent 原文。
- **型錄**：`plugin/scaffold/.claude/teams/deliverables/catalog.json`（八組、56 種；`source` 分 auto／assisted／external；`on` 觸發條件、`section_on`、`design_sections`、`suggest`、`aliases`；`presets` 五個情境組合）。
- **交付設定**：專案的 `.claude/teams/deliverables.json`（schema `schemas/deliverables.schema.json`），沒有時看 profile 的 `deliverables`（要 `enabled: true`；內建 profile 只放範例、不啟用）。語意規則在 `schema::deliverables_errors`。**沒有交付設定時流程完全不變**。
- **`awf deliverables`**：`configure`（情境組合、`--suggest` 依 repo 實況、`--import` 對照清單；`--write --by` 才寫入）、`init`、`status`（齊全／過期／缺原料／等人補）、`check`、`index`、`affected`、`matrix`（需求追溯矩陣）、`sync`、`export`。小節的來源標記只寫 glob，雜湊在 `.claude/teams/deliverables.state.json`；實際比對的來源＝文件層級 `sources` ∪ 標記（標記只能加）；`stamp` 只記內文改過的小節（`body_hash` 不看空白與標點），人用 `--reviewed` 才能放行過期小節；`awf done` 的自動同步只記 workflow 回傳的 `deliverables.updated`，清單外的改動列出來交人，最後跑一次 `check_all`。隱私規則（`privacy_rules`、`privacy_text` 先正規化、`run_name_rule`）只能加不能拿掉；追溯矩陣的證據只留檔名（`strip_paths`），核准者名字預設不寫（`show_approvers`）。
- **流程接點**：`awf pack` 在 design／deliver 段呼叫 `deliverables::pack_args`，結果放 `pack-args.json` 的 `deliverables`；`sdlc-design` 的 `DELIV_NOTE` 要求 design.md 留小節；`sdlc-deliver` 多派一個 `deliverables`（tpm-writer）只改受影響的小節並寫 `交付摘要.md`，路徑先過 `dRel`；`awf done` 存完 deliver 段自動跑 `deliverables_sync`。新領域只提示（`hints`），不自動加。
- **守門**：team agent 只能跑 `awf deliverables status`／`check`／`affected`（`save_run_write` 會展開 `bash -c`、`eval`、`pwsh -Command` 的內層指令，`xargs` 這類轉呼叫看不到子指令就當寫入；save_run、pack、kb 同樣適用）；`deliverables.json`、`deliverables.state.json` 在 `.claude/teams/`，team agent 寫不到。
- 測試：`awf deliverables --selftest`、`awf pack --selftest` 與 `awf state --selftest` 的交付案例、`plugin/workflow-templates/scenarios/sdlc-deliver.mjs`／`sdlc-design.mjs` 的「交付文件」情境、hook 測試的 C1／C2 案例、golden 的 `deliverables*`、`methodology-deliverables*`。說明在 `docs/交付文件.md`。

### 規格層與連結接進存檔與閘門（1.0）

- **需求來源 `openspec`**：`crates/awf-core/src/spec/source.rs`（`source_cfg`、`change_items`、`known_ids`、`proposal_missing`、`DEFAULT_SECTION_MAP`）是 save-run 與 merge-gate 共用的介面；`merge_gate::adapter_openspec` 取需求清單，`save_run/speccheck.rs` 做 intent（proposal 對必要節）與 design／test 的 id 核對（`spec_issues`）。只有實際採用的需求來源是 openspec 時才啟用；`approve --by policy` 重算、人工核准只警告。
- **連結帳本**：`save_run/links.rs`。`<runs_dir>/links.jsonl` 只附加，格式 `schemas/link.schema.json`；重存有變時先寫 `op: supersede` 再寫新的，一樣就不寫。寫入失敗只警告，閘門不讀它。檔名在 `hooks/guard.rs` 的 `SHARED_CONTEXT_NAMES`，team agent 不能寫。
- **連結檢查**：`links::link_gaps` 只讀 design.json、test.json、run.json（不讀帳本）。profile 有 `links` 物件才做；`links.enforce` 開啟時有缺口不能政策核准。
- **對外介面**：`awf save-run status --json` 與 `progress.jsonl` 帶 `schema_version`（`STATUS_SCHEMA_VERSION`、`watch::PROGRESS_SCHEMA_VERSION`），同版本內只加不改；status 的每段有 `skipped`。承諾寫在 `docs/監控與整合.md`〈相容承諾〉。
- 測試：`awf save-run --selftest` 的 1.0 案例（`save_run/selftest_v1.rs`）、golden 的 `methodology-openspec*`、`methodology-links*`、`link*`、hook 案例的「D1 連結帳本」。

### 知識圖接進資料包與交付文件（1.0）

- **原則**：有圖（`awf kg build` 建的）而且能開才用；沒有圖、開不了、查不到時，`pack.md`、`pack-args.json`、交付文件的狀態和沒有這個功能時逐字相同（`pack_parity`、`deliverables_parity` 不動）。流程不替使用者建圖。
- **讀圖的共用函式**：`crates/awf-core/src/kg/flow.rs`（`open_root`、`plan_facts`、`file_links`、`callers_and_deps`、`doc_sections`、`section_links`、`requirement_rows`；條數上限是那裡的常數）。
- **資料包**：`pack/graph.rs`。plan、design 的一節接在 `pack.md` 最後，build 接在每份子任務資料包最後；plan 的 `located.code-contracts` 在 `PlanFacts::code_enough` 與「和工具定位有重疊」都成立時標成夠用。deliver 走 `deliverables::pack_args_with`（`deliverables/kgflow.rs` 的 `extend_affected`：只會多列）。`--no-kg` 關閉。新內容都寫在 `pack.md`，沒有加範本要讀的新鍵。
- **規格層的 id**：需求來源是 openspec 時，子任務的 `spec_refs` 可以寫穩定 id（`pack::SpecIds`，變更的差異優先於現行規格）；找不到的才進 `spec_ref_errors`。
- **別的 repo**：profile 的 `knowledge.repos`、`spec_root`、`timeout`（`kg::layout`、`build.rs` 的 `repos_of`／`scan_repo`）；語意規則在 `schema::methodology_errors`，但建圖時再驗一次（不合規則的整筆略過並說明）。
- **交付文件**：`deliverables/kgflow.rs`。`Links`（小節連到的需求、情境、符號、程式檔的指紋，記在 `deliverables.state.json` 的 `links`；`linked_state` 只把 fresh 改成 stale 或 unstamped）、`project_matrix`（`awf deliverables matrix --project`，只讀、沒有圖回非零）。指紋存在圖的 `fps` 表（`store::put_fp`）。
- **結案摘要**：`save_run/archive.rs` 的 `archive_text` 只多一個呼叫點（`kg::links::run_section`）；帳本不存在或這個 run 沒有連結時沒有〈連結〉一節。
- 測試：`awf pack --selftest`、`awf deliverables --selftest` 的 1.0 案例（各自的 `selftest_v1.rs`，只有命令列會接著跑，原本那一段的輸出是對跑基準、不動）、`awf kg --selftest` 的「別的 repo」案例、golden 的 `methodology-knowledge-*`。

### 自動接點與多需求排程（1.0）

- **設定**：profile 的 `knowledge.auto`（`update`、`timeout`、`session`、`prompt`、`spec_apply`、`export`），讀法在 `state/auto.rs` 的 `auto_cfg`。全部是輔助：失敗只警告、閘門不讀；沒有圖（`<knowledge.dir>/graph.db`）又沒明確開啟時，`awf next`／`done` 的輸出逐字不變（`state_parity` 不能因此改）。
- **每段開跑前更新圖**：`machine::workflow_action`、`custom_action` 在 `run_pack` 之前呼叫 `auto::kg_before_stage`（`kg::update`，有時限）。預設不自動建第一份圖。
- **merge 之後併回規格**：`auto::spec_apply`（由 `decide` 在沒有下一段時呼叫）。沿用 `manual` 動作，多 `preview`、`apply`、`skip` 三個欄位；狀態記在 run.json 的 `awf.spec_apply`，略過用 `awf next --skip spec-apply`。
- **結案後重寫 vault**：`save_run/cli.rs` 的 `vault_after`（close、abandon 之後）呼叫 `auto::refresh_vault`；沒有圖時輸出不變。
- **hook**：`hooks/auto.rs`（`session_context`、`mark_dirty`、`prompt_context`）；`awf hook mark-dirty`（PostToolUse）與 `awf hook prompt-context`（UserPromptSubmit）在 `plugin/hooks/hooks.json` 註冊。永遠 exit 0、注入有字數上限、包在資料標籤裡、不注入 subagent；輸出格式的依據（官方文件網址）寫在模組開頭。
- **中斷回收**：`auto::recover_interrupted`（`cmd_next` 在 `decide` 之前呼叫）。只收 plan 段的調研，條件見函式說明；寫入走 `pack::record_reuse`，紀錄在 run.json 的 `awf.recovered`。
- **多需求排程**：`awf program`（`crates/awf-core/src/program.rs`）。唯讀：`status`、`next`；`<runs_dir>/program.json` 的格式在 `schemas/program.schema.json`，語意規則 `program::semantic_errors`（`schema::check_doc` 也會呼叫）。team agent 只能跑 `status`（`hooks/awf_cli.rs`）。
- 測試：`crates/awf/tests/auto_e2e.rs`（假的段結果從開工走到結案、全關與刪掉圖、中斷回收的該收與不該收）、`crates/awf/tests/hooks_auto.rs`（三個 hook 的該注入、不該注入、出錯不注入）、`awf program --selftest`、golden 的 `program*`、`methodology-knowledge-auto*`。說明在 `docs/知識圖.md`〈自動接點〉與 `docs/監控與整合.md`〈多需求排程〉。

### 平行處理

- **`sdlc-design`**：先由 dev-spec-author 出大綱（`plan-outline`）。預估子任務超過 `parallelism.split_threshold`，或有多個可以各自規劃的區塊時，拆成最多 `max_sub_plans` 個子計劃。每個子計劃由一個 dev-spec-author 平行規劃，寫在 `specs/<epic>/sub/<id>/`，不 commit；最後由一個 dev-spec-author 彙整並 commit。跨子計劃的相依寫成 `@<子計劃 id>`，由 workflow 展開；指向不存在、沒有子任務或自己的子計劃時 halted。
- **`sdlc-build`**：依子任務相依分層，同一層各自開 worktree 平行實作，每層由 dev-integrator 合併。
- **`sdlc-test`**：有子計劃時，每個子計劃一個 dev-verifier，另加一個整合分片，平行驗證後彙整成 `verifier.md`；條目 id 帶分片前綴（`<分片>:<原 id>`），彙整後的 commit 必須等於各分片共同的 HEAD（merge 的 `--reviewed-sha` 取自它）。失敗分散在多個子計劃時，各自開 worktree 平行修正，再合併。
- workflow 會對以下幾點做確定性核對，不一致就回傳 `halted: true`：子計劃的範圍與相依、彙整後的子任務清單（不能默默少掉）、彙整後的驗證結果（不能漏條目或改結果，各分片要驗同一個 HEAD）。
- 預設乾跑走不到這些分支，所以 `check_workflows.mjs` 載入 `plugin/workflow-templates/scenarios/<workflow>.mjs`（每支 workflow 一檔）用假回傳把它們走過一遍，另有各 agent 缺件的情境。改了這些流程，對應的 scenarios 檔要一起改。

### 使用者自己的 agent（擴充）

- **設定位置**：使用者用 `/agent-workflow:add-agent`（`plugin/skills/add-agent`）把自己的 agent 加進 plan、design、test、deliver 任一段，設定寫兩個地方：
  - `methodology.json` 的 `extensions`：寫 name、agent、stage、instructions。
  - `guard.config.json` 的 `extension_agents`：`{完整 agent_type: 要套用的模版角色}`。
- **workflow 怎麼派**：四支 sdlc workflow 裡各有 `EXT`／`extJob`，把擴充 agent 和內建 agent 平行派出。`agentType` 用使用者給的原值，不加 `NS`；一律交 `findings`。
- **守門**：
  - `config::extension_role()` 讓登記過的擴充 agent 算模版角色，套用對應角色的規則。
  - 只能對應 `EXTENSION_ROLES`（唯讀類），其他值改套 `tpm-reviewer`。
  - 合併的放行判斷（`merge_refusal`）與 freeze 仍比對完整 agent_type，擴充 agent 不會因此能合併。
- **沒登記就不內嵌**：`awf build` 遇到沒登記守門角色的擴充 agent 會拒絕內嵌（exit 1）。
- **不開放的段**：build（有檔案所有權）與 merge（信任邊界）。

### Workflow 腳本的限制

- 腳本裡不能取時間，跑到一半也不能等人輸入。日期由主 session 放進 `run_dir`；需要人決定的地方，就把 workflow 拆成兩支，例如 `sdlc-design` 和 `sdlc-design-revise`。
- 每個 `agent()` 都要帶 `agentType: NS + '<agent 名>'` 和 `schema`。`NS` 取自 `'{{AGENT_NS}}'`，預設 `agent-workflow:`。
- `agent()` 一律透過 challenge 片段的 `tryAgent` 呼叫，失敗或 throw 都當成缺件（回 null）。乾跑情境的 fake 可以給 `'THROW'` 模擬 agent 失敗。
- 多個 agent 共用同一個 worktree 時不能各自 commit（會搶 index.lock），要由最後一步統一 commit。
- 共用片段（`plugin/workflow-templates/snippets/*.js`）只定義函式：不能有頂層 `await`，不能用 `{{佔位符}}`，也不讀 workflow 的區域變數，需要的值一律用參數傳進去。片段之間有依賴時，在片段開頭寫 `// @requires: <名>`，引用它的 workflow 也必須引用被依賴的片段。違反時 `awf build` exit 1。

### 自動合併

決策記錄在 `docs/adr-agent-auto-merge.md`，信任模型寫在 `docs/自動合併信任模型.md`。

- `awf merge-gate`：確定性 gate。複核需求對照、head SHA、required checks、測試數、skip、風險等級、每日上限、白名單、凍結旗標，並寫入 `<runs_dir>/merge-audit.jsonl`。
  - `--reviewed-sha`：審查／驗證當下的 head，缺它或和 head 不符一律 hold。
  - `--merged-today`：CI 用 GitHub API 算的當日合併數（只計 `AGENT_MERGER_BOT` 合併的），和本機 audit 取大者；不是非負整數就 hold。
  - `--frozen <理由>`：CI 從 repo variable `AGENT_AUTOMERGE_FROZEN` 帶入，有給就 hold。本機的凍結檔 CI 看不到。
  - `auto_merge.allowed_skipped_checks`：skipped／neutral 預設不算通過，列在這裡的才算。
  - `upstream_authorized_by` 的值可以是字串或 `{by, link, scope, expires}`；過期或日期讀不懂都和沒有授權一樣 escalate，和 hook 一致。放寬閘門（改測試／lint／覆蓋率設定、新增壓制註解）一律 escalate；`auto_merge.carry_verdict_on_rebase`（預設關閉）開啟時，rebase 後 patch-id 相同才沿用 verdict。
- steward 合併後要讀回 PR 狀態填 `merge_state`，`sdlc-merge` 只在 `merged` 時算已合併（merge queue 可能只是排隊）。
- `awf publish`：確定性推送工具。只推 fork 的功能分支，推送前掃機敏值（含二進位與空檔的機敏檔名）、檢查整條閘門鏈到 deliver 都已核准（`--stage` 只能指定 deliver 或更後面）、開 PR，並寫稽核紀錄。
- `awf merge-gate` 解析 diff 時，rename／刪除的舊路徑也算進風險判斷，加引號的路徑會還原；標頭或行數解析不了一律 hold，所以 diff 要用 `--src-prefix=a/ --dst-prefix=b/` 產生。判定的「今天」取 `--timestamp` 與系統 UTC 日期較晚者。
- `plugin/scaffold/.github/workflows/agent-merge-gate.yml.tmpl`：給目標專案用的 CI，從受保護的 base 執行 gate，設為 required check。
- 真正的信任邊界在伺服器端（ruleset、GitHub App、CODEOWNERS），hook 只是輔助，可以被繞過。`auto_merge` 預設關閉。安全政策見 `SECURITY.md`。

### Hooks（`plugin/hooks/hooks.json` 註冊，規則在 `crates/awf-core/src/hooks/`）

- **入口**：只有一個，`awf hook <guard-shell|guard-paths|envelope|session-start|mark-dirty|prompt-context>`。設定一律透過 `crates/awf-core/src/config.rs` 讀 `${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json`，不要寫死路徑。
- **失敗語意**（`crates/awf-core/src/hooks/mod.rs` 開頭的說明為準）：
  - hook 跑不起來（找不到執行檔、逾時）：plugin 的 `hooks.json` 設 `onFailure: continue`、timeout 10 秒，放行，不鎖死使用者。要更嚴就用 `--hooks project`。
  - 跑得起來但輸入壞掉（空、不是 JSON、缺欄位、shell 指令超過 128KB）：守門類擋（exit 2），交件類放行。128KB 上限是因為 guard 規則耗時隨指令長度約平方成長，拖到逾時就等於放行。
  - 守門設定載入失敗，或程式內部 panic：改用 `awf hook` 內建的最小規則（`fallback_shell`）（擋 push、`gh pr merge`、`--approve`、GitHub API 寫入），其他放行。
  - 交件 hook（`envelope`）自己出錯時刻意放行，避免卡住所有 subagent；信封仍由 workflow 的 `schema` 與 `awf save-run stage` 驗證。
- **`session-start`（0.5）**：SessionStart 時比對 plugin 版本與專案 `.claude/teams/VERSION`，不一致印一行 JSON（`systemMessage` 與 `additionalContext`）。只是提示：永遠 exit 0，沒套用、讀不到、輸入壞掉、任何例外都不印；版本字串只收單純的版本格式（會進對話）。找不到 plugin.json 時不提示。測試在 `crates/awf/tests/hooks_entry.rs` 的第 6 段。
- **`guard-paths` 的範圍**：Edit、Write、NotebookEdit、Read、Grep。`guard.config.json` 的 `linked_repos`（工作區以外的我方 repo，`hooks::guard::linked_rel`）讓 dev-* 可以改裡面的檔、tpm-writer 可以寫它的 specs，其餘規則照舊；模版角色可以寫這個 session 的 scratchpad（`hooks::guard::in_scratchpad`：系統暫存目錄底下的 `<session_id>/scratchpad/`）。Read、Grep 讀機敏檔（`.env`、金鑰、憑證）一律擋，主 session 也擋；`acceptance_test_dirs` 裡的檔案 dev-implementer 不能寫。
- **`knowledge.repos`（1.0）**：profile 的 `knowledge.repos[].path` 底下對模版角色唯讀（`config::profile_bits` 和自訂事件段名同一次讀檔；`hooks::guard::kg_repos`／`kg_repo_hit`、`guard_shell` 的 `shell_writes_kg_repo`）。讀照既有規則（工作區外的 Read／Grep 本來就放行，機敏檔一律擋）；Edit／Write 與 shell 寫入都擋，同時登記在 `linked_repos` 也擋。根目錄、家目錄、專案自己或上層目錄的項目不算；profile 讀不懂當作沒設定。shell 那一條先找目標再比對寫法（寫法的樣式大，先比對會讓每個指令多 30ms 以上）。
- **CI 檔（1.0）**：模版角色不能寫任何 repo 的 `.github/workflows/`（`guard_paths::ci_workflow_path`、`guard_shell` 的 `shell_writes_ci`／`CI_TARGET`；Edit／Write 與 shell 寫入都擋，讀可以）。CI 檔由人或主 session 改（`awf ci render --write`）。這是事前擋；`awf merge-gate` 對 diff 動到 `.github/` 一律 escalate 是事後的另一層，兩邊不衝突。主 session 與使用者自己的 agent 不受影響。
- **`guard-shell` 擋什麼**：同時處理 Bash 與 PowerShell 工具。擋推送、寫上游、推受保護分支、越權寫入，以及所有合併與核准路徑：`gh pr merge`、`--approve`、`gh api`、GraphQL、curl、Invoke-RestMethod。
- **steward 合併什麼時候放行**：由 `crates/awf-core/src/hooks/guard_shell.rs` 的 `merge_refusal` 判斷，以下全部成立才放行：
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
  - plugin 註冊：`awf init --hooks plugin`，預設。
  - 註冊在專案的 `.claude/settings.json`：`--hooks project`，Windows 建議用這個，init 會寫入執行檔的絕對路徑。範本（`settings.project-hooks.json.tmpl`）註冊七筆：守門兩筆 `onFailure: block`，交件三筆，加上輔助的 `mark-dirty`（PostToolUse）與 `prompt-context`（UserPromptSubmit）兩筆 `onFailure: continue`。`--upgrade` 時 settings.json 已經有 1.0 寫法的註冊就自動照 project 模式升級（補缺的註冊、更新執行檔複本）；`init::merge::merge_hooks` 讓每個種類在每個事件只留一份。`awf doctor` 的重複註冊檢查（`project_hook_re`）認得全部六個種類。

### Slash commands 與 skills

- `plugin/commands/` 只負責把使用者的話整理成 `tpm-team` skill 的輸入。流程寫在 skill 和 workflow 裡，不要在 command 裡再寫一份。
- `plugin/skills/tpm-team/SKILL.md` 只寫 `awf next` → 照做 → `done` 的迴圈（不超過 60 行、描述不超過 200 字，`VP B1.5` 檢查）；事件對照表、recipe、閘門規則與手動執行在同目錄的 `reference/*.md`。
- plugin 有 4 支 command：`/sdlc`、`/event`、`/brief`、`/demo-check`，都是薄殼。
- 其他入口是 skill，用 `/agent-workflow:<skill>` 叫用（沒有同名指令時可省略前綴）。不要再新增和 skill 同名的 command。
  - `/agent-workflow:init-workflow`：先跑 `awf init --detect`，逐主題在對話中確認使用者的做法，再跑 `awf init --answers` 並唸出對照表（訪談在 `crates/awf-core/src/init/detect.rs`、`interview.rs`；答案存 `.claude/teams/init-answers.json`，`--reconfigure` 只改有變的設定；`--reconfigure <主題>` 只重問一個主題，可以單獨問的主題是 `interview::single_topics()`）。
  - `/agent-workflow:add-agent`：把使用者自己的 agent 加進 SDLC。
  - `/agent-workflow:correct`：把一則教訓變成最高可行層級的強制機制，並在 rules.md〈專案規則〉維護「規則 ↔ 強制方式」對照表。
  - `/agent-workflow:deliverables`：薄殼，在對話中走交付設定（提案 → 人確認 → `--write`）與盤點（`status`、`check`）。
  - `/agent-workflow:status`、`cost`、`kb`、`doctor`：薄殼，各自只叫 `awf watch --once`、`awf cost`、`awf kb`、`awf doctor` 並說明怎麼回報。`status`、`cost`、`doctor` 和 Claude Code 內建指令同名，文件一律寫完整名稱。

## 改動時的檢查清單

- **新增佔位符**：`workflow.config.example`、`awf init` 和 README 的佔位符表要一起改。smoke_init 會檢查產出裡有沒有殘留的 `{{`。
- **新增 `dev-*`／`tpm-*` agent**：
  - 照既有段落寫：切分理由、職責、硬性約束、工作方式、交件格式。
  - 設 `model: inherit` 和 `skills`。只讀的角色不給 Edit／Write。
  - `crates/awf-core/src/check_agents.rs` 的 `CORE` 與 `config.rs` 的 `RESERVED_ROLES` 要一起改（`awf check-agents` 會核對 CORE ⊆ RESERVED_ROLES）；需要套用 guard 時，在 `role_map` 加對應。
  - 合併或改名角色時，舊名留在 `ALIASES`、`ROLE_ALIASES` 與 `RESERVED_ROLES` 相容一版，並留薄殼 agent 檔。
  - 更新 README、本檔、`plugin/scaffold/.claude/teams/README.md` 的角色數與名單，`cargo xtask check-docs` 會核對。
- **新增或刪除 command**：`cargo xtask check-docs` 會核對 README 等文件的 command 名單；不要和 skill 同名。
- **新增事件類型**：形狀是「多角度 → skeptic → writer」的，在 `event.js` 的 `BUILTIN_EVENTS` 加一筆（使用者則在 profile 的 `events` 加），並改狀態機（`crates/awf-core/src/state/mod.rs`）的 `EVENT_TYPES`、`reference/事件.md`、`docs/事件處理.md`，在 `plugin/workflow-templates/scenarios/` 加 `_event-<類型>.mjs` 並登記到 `event.mjs`；形狀不同才另寫一支。
- **新增 workflow**：範本放 `plugin/workflow-templates/`；在 `tpm-team` skill 的 `reference/事件.md` 對照表加一列，加進 `crates/awf/tests/smoke_init.rs` 的清單與狀態機（`state/mod.rs`）的 `FIXED_EVENTS`／`REQUIRED`；重跑 `cargo xtask gen-legacy --write` 不需要（它只記已發佈的舊版）。
- **改分支流程（scenarios）**：乾跑走不到的分支，在 `plugin/workflow-templates/scenarios/<workflow>.mjs` 補情境（`check_workflows.mjs` 自動載入，共用的 helper 在 `scenarios/_helpers.mjs`）。
- **改 snippet**：改了 `plugin/workflow-templates/snippets/*.js`（workflow 共用片段）要重跑 `awf build`（`--check` 會抓漂移），不要手改內嵌進 workflow 的內容；並確認 `plugin/workflow-templates/scenarios/*.mjs` 用到的 agent label 沒變（情境靠 label 覆寫假回傳，label 一變情境就默默失效）。
- **新增或修改 payload schema**：重跑 `awf build`；在 `golden-set/valid|invalid/` 補正例、反例，有語意規則的另補 `semantic` 反例；語意規則寫在 `crates/awf-core/src/schema.rs`。
- **改狀態機（`crates/awf-core/src/state/`）**：`awf state --selftest`；動作的欄位名（`action`、`script`、`then`、`approve`、`retry`、`resume_from`、`need`、`highlights`）改了，`plugin/skills/tpm-team/SKILL.md` 的動作表要一起改；改了 `render` 要確認 `crates/awf/tests/smoke_init.rs` 的逐字相同檢查仍通過。
- **改擴充 agent 機制**：四支 sdlc workflow 的 `EXT` 片段要一致；`plugin/workflow-templates/scenarios/sdlc-*.mjs` 的擴充情境、`crates/awf/tests/smoke_init.rs` 的 `extensions`、`tests/hooks/cases.jsonl` 的擴充 agent 案例要一起改。
- **新增 hook 規則**：在 `tests/hooks/cases.jsonl` 補一個該擋、一個該放的測試，Bash 與 PowerShell 各一。
- **改 profile 格式**：`methodology.schema.json`、兩份內建 profile、`docs/自訂與擴充.md` 的欄位表要一起改。
- **改 recipe 或 modifier**：`crates/awf-core/src/recipe.rs` 的 `builtin_recipes()`／`builtin_modifiers()`、`plugin/workflow-templates/snippets/recipe.js`、兩份內建 profile、`plugin/workflow-templates/scenarios/_helpers.mjs` 的 `RECIPE_STAGES`（手抄的段表）要一起改；在 `golden-set/recipes/` 補案例，跑 `cargo xtask check-recipes`。新增的 recipe 不能省 plan 段，會寫程式的要有 build、deliver、merge（`schema::methodology_errors`）。
- **改模型分級**：`recipe.rs`（`BUILTIN_TIER_MODELS`、`BUILTIN_MODEL_FLOORS`、`resolve_model`）與 `snippets/recipe.js` 一起改，在 `golden-set/recipes/` 補 `model_cases`，跑 `cargo xtask check-recipes`；floor 只能加、不能拿掉。
- **改 pattern 庫或自訂腳本**：`cargo xtask check-patterns`（每個 pattern 的正常、達上限、缺件，以及組裝後實際執行）、`awf script --selftest`、`awf state --selftest` 的自訂腳本案例；`script::ROLES` 要等於 `awf check-agents` 的 `CORE` 扣掉 dev-merge-steward（selftest 會核對）；開放新的段之前先確認那一段的確定性核對不在 workflow 腳本裡；`docs/自訂與擴充.md`〈自訂腳本〉的規則表一起改。scaffold 底下的檔案（含片段與情境）不能出現雙大括號（init 會當佔位符）。
- **改執行核心（kernel 片段）**：重跑 `awf build`；`plugin/workflow-templates/scenarios/sdlc-plan.mjs` 的 `V04` 情境（升級、上限、重用、角度數）要一起改；`awf save-run` 的 `retried_ok`、`model_errors` 依賴 trace 的 `model`、`tier`、`superseded`、`attempt` 欄位。
- **改讀取預算**：`tests/hooks/cases.jsonl` 的 A2 一段補該擋與該放（Bash 與 PowerShell 各一），並確認效能檢查（計數路徑 50ms 內）仍通過；任何新的失敗路徑都要放行。
- **改 awf pack／awf kb／awf locate**：各自的 `--selftest`；改了子任務資料包的路徑或 `pack-args.json` 的鍵，`sdlc-build.js` 的 `taskPack`／`REF_ERRS` 與 `plugin/workflow-templates/scenarios/sdlc-build.mjs` 的「子任務資料包」情境要一起改；改了 `awf locate` 的輸出格式，`sdlc-plan.js` 的 `toolLocated` 與 `plugin/workflow-templates/scenarios/sdlc-plan.mjs` 的「工具先定位」情境要一起改；`pack.md` 的內容不能有時間戳或絕對路徑以外會變動的東西（可重現是 `cargo xtask bench --selftest` 的檢查項）。
- **改交付文件**：`awf deliverables --selftest`；改了型錄的欄位，`schemas/deliverables.schema.json`、`methodology.schema.json` 的 `deliverables`、`docs/交付文件.md` 一起改；改了 `pack_args` 的輸出鍵，`sdlc-design.js`／`sdlc-deliver.js` 與 `plugin/workflow-templates/scenarios/` 的「交付文件」情境一起改；隱私規則只能加不能拿掉（交付文件給外部的人看）。
- **改規格檢查、連結或對外介面**：`awf save-run --selftest`；沒啟用時輸出要逐字相同（`tests/parity/` 的基準不該變）；連結種類或欄位改了，`schemas/link.schema.json` 與 `docs/自訂與擴充.md`〈連結〉一起改；`status --json` 或進度事件拿掉、改名欄位要遞增 `schema_version` 並寫 CHANGELOG。
- **改 doctor**：`awf doctor --selftest`；新的檢查項要有修法（`fix`），而且單項出錯不能讓整支失敗。體檢類（`models`、`kb`、`openspec`、`requirements-outside`、`runs`）只用 info 或 warn、沒事時不列；項目有增減時用 `AWF_RECORD=1 cargo test -p awf-core --test doctor_parity` 重錄基準並逐項看差異。
- **改 watcher**：`awf watch --selftest`；事件類型或欄位改了，`schemas/progress-event.schema.json`、`methodology.schema.json` 的 `progress.notify.events`、`docs/監控與整合.md` 的事件表要一起改；送給外部指令的欄位只能加進 `NOTIFY_FIELDS`（不放需求內容、摘要、路徑）；任何失敗都只警告。
- **改派工單或派工數**：跑 `cargo xtask check-budget`；變多是預期的，才用 `--update` 更新 `golden-set/token-budget.json`，並在 commit 說明理由。
- **新增會寫紀錄的唯讀角色**：加進 `config.rs` 的 `RECORD_ONLY_ROLES` 與 `check_agents.rs` 的對應檢查，hook 測試補該擋（寫 repo、run.json、merge-audit）與該放（寫 runs_dir 的 `.md`）。
- **新增 guard.config 欄位**：`guard.config.json.tmpl` 的 `_說明`、`config.rs` 的預設值、`docs/自動合併信任模型.md` 或 `docs/自訂與擴充.md` 一起改；`awf init --upgrade` 會把新鍵補進既有專案。
- **版本（VERSION）**：發佈前遞增 `plugin/.claude-plugin/plugin.json` 與 `.claude-plugin/marketplace.json`（`metadata.version` 與 `plugins[].version`）。init 寫的 `.claude/teams/VERSION` 取自 plugin.json，三處一致由 `crates/awf/tests/smoke_init.rs` 檢查。
- **CHANGELOG**：每次發佈在 `CHANGELOG.md` 補一節，破壞性變更（角色改名、command 刪除、hook 語意）與升級步驟要寫清楚。
- **完成前**：每次都跑 `cargo test --workspace` 與 `cargo xtask check-all`（兩個都過才算），`cargo build --all-targets` 不能有警告。

完整的擴充說明見 `docs/自訂與擴充.md`，實戰教訓見 `docs/踩坑手冊.md`。
