# 變更紀錄

格式依 [Keep a Changelog](https://keepachangelog.com/zh-TW/1.1.0/)，版本號依 [語意化版本](https://semver.org/lang/zh-TW/)。

## [Unreleased]

## [0.3.0]

這一版讓 SDLC 依需求類型分流、每個 agent 都留下執行紀錄，並精簡 token。起因：每次都跑完整 7 段很耗時間與 token，修一個 bug 也派 25–35 個 agent。

### 破壞性變更

- **publish.py 依 recipe 檢查**：推送前用實際 diff 反查 run 的 recipe（docs 改了程式、碰到 migration 沒有 design、改依賴沒有安全審查、spike 要推送），不夠就拒推，要先 `save_run.py escalate`。閘門鏈改依 recipe 判斷：recipe 裡有、run.json 卻沒有的段一律判錯（原本會略過）。
- **4 個唯讀角色新增 Write**：tpm-reviewer、tpm-skeptic、dev-release-watcher、tpm-planner 要寫自己的執行紀錄；hook 只准它們用 Write 寫當前 run 某一段底下的 `.md`（`<runs_dir>/<run>/<段>/`），shell 一律不能寫檔（重導向、cp、sed -i、直譯器內嵌寫檔、PowerShell 寫檔 cmdlet 等；`run_brief.py --log` 指到 runs_dir 的 .log 例外）。dev-merge-steward 仍沒有 Write。
- **hook 對所有 team agent 更嚴**：`save_run.py` 只能跑 `status`；不能寫任何位置的 `.claude/teams/`、`methodology.json`、`guard.config.json`（用這套流程開發會改到 `.claude/teams/` 的專案，這部分要由主 session 改）；不能寫 runs_dir 的 `run.json`、`<段>.json`、`context.md` 與任何 `執行紀錄索引.md`。
- **信封有長度上限**：summary 400 字、evidence 12 則、evidence.result 300 字；超過的交件會被 schema 與 hook 擋下。
- **intent 必要節**：`save_run.py stage plan` 檢查 intent.md 有沒有目標、範圍、不做的事、驗收條件（加上 plan_mode 專屬的節），缺節不能核准。內建 profile 的 intent 範本已改成這四節。
- **閘門依 recipe 核對（獨立審查後補強）**：
  - 有 recipe 的 run：workflow 回傳的 recipe（name、modifiers、段）要和 run.json 鎖定的一致，不一致記成 invalid；回傳一定要帶 trace，trace 的 `ok` 不是 true 算缺件；recipe 要求的角色（`security_review`／`design_review=always` 的審查者、dev-verifier、每條 deliver 軸）沒交件就不能政策核准。
  - 開發規範 profile 只認工具所在專案的 `.claude/teams/methodology.json`，不再從 run 目錄往上找；profile 不合規則（例如 recipe 沒有 plan）一律判錯。
  - 核准（plan 以外）、publish、`merge_gate.py --recipe` 會用 profile 重算 recipe，run.json 的 `recipe.resolved` 比重算結果鬆（少段、多 build_kinds、拿掉 followup 等）就拒絕。
  - `merge_gate.py --recipe` 只接受 run.json，而且 plan 要已核准；稽核紀錄多記 recipe 名稱與雜湊。
  - `save_run.py close` 要整條流程走完（各段 done、有閘門的已核准、merge done）；hotfix 的後續義務不會因 escalate 消失；後續 run 不能是已取消或本身也是 hotfix 的 run。
  - 舊 run（沒有 recipe）plan 核准後，不能用 `stage plan` 補記比 feature 少段的 recipe。
  - diff 分類更嚴：`.txt` 只有在 docs/ 底下才算文件；巢狀的 `spec/`、`test/` 目錄不算測試；agent 與 CI 設定（`.claude/`、`.github/`、`CLAUDE.md`）要安全審查；補上多種語言的 migration 與依賴宣告；migration 與依賴宣告也算產品程式。
- **workflow 回傳多了 `trace`**，派工單開頭多了一段固定的共同規則、最後一行是執行紀錄檔路徑；「派工來源」那段改放 rules.md 規則 27。

### 新增

- **recipe 分流**：profile 新增 `recipes`、`modifiers`、`plan_modes`、`stages.plan.required_sections`。內建 11 個 recipe（feature、bugfix、change、hotfix、refactor、deps、docs、test-only、perf、spike、revert）與 5 個 modifier（ui、security、migration、public_api、large）。`sdlc-plan` 先派 tpm-planner 做 triage（payload `triage`），由人在 plan 閘門選定；之後只能用 `save_run.py escalate` 加嚴，降級一律拒絕。
- **save_run.py**：`escalate`、`followup`、`close`（hotfix 沒記錄後續 run 不能結案）；`init` 寫 `context.md` 給所有 agent 共用；`stage` 依 trace 產生 `執行紀錄索引.md`。
- **merge_gate.py `--recipe`**：依 diff 形狀判斷 recipe 不夠時 hold。
- **tools/recipe.py、snippets/recipe.js**：同一套解析規則，`golden-set/recipes/` 兩邊都跑。
- **tools/run_brief.py**：先濃縮指令輸出再給 agent 讀（Read less），完整 log 另存。
- **rules.md**：規則 27 派工來源、28 執行紀錄、29 intent 與 rules 不可省、30 語體（只用於 agent 之間的信封，參考 caveman；派工單與給人看的文件不壓縮語體）。
- **tests/check_budget.py**、**tests/check_recipes.py**。

### 升級步驟

1. `claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins`。
2. 在專案根目錄跑 `python3 <AgentWorkFlow>/init.py --target . --upgrade`，看它列出的缺少欄位（`recipes`、`modifiers`）；要用分流就加 `--adopt-new-fields` 補上，不補的話一律走 feature 完整流程。
3. 跑 `python3 .claude/teams/tools/build.py`。
4. 自訂過 intent 範本的專案：確認範本有目標、範圍、不做的事、驗收條件四節，或在 `stages.plan.required_sections` 寫你自己的節名。
5. 進行中的 run（0.2.0 建的）沒有 recipe，一律當成 feature 繼續跑。

## [0.2.0]

這一版是整體強化：可以升級既有專案、hook 更不容易鎖死使用者也更難繞過、角色與 command 精簡。

### 破壞性變更

- **角色合併**：`dev-pr-reviewer` 併入 `tpm-reviewer`（新增 deliver 模式）；`tpm-upstream-analyst` 併入 `tpm-researcher`（新增上游模式）。舊名保留**一版**相容（薄殼 agent，`RESERVED_ROLES` 仍認得），下一版移除。自訂 workflow、擴充 agent 設定或 `guard.config.json` 裡用到舊名的，請改用新名。
- **刪除同名 command**：移除 `/add-agent`、`/correct` 這兩支 command，避免和同名 skill 衝突，command 剩 4 支（`/sdlc`、`/event`、`/brief`、`/demo-check`）。功能不變，改由 `add-agent`、`correct` skill 提供，用 `/agent-workflow:add-agent`、`/agent-workflow:correct` 叫用（沒有其他同名指令時也可以省略前綴）。
- **merge gate 缺審查 head 一律 hold**：`merge_gate.py check` 有 `--findings` 卻沒有 `--reviewed-sha`（或和 head 不符）時判 hold；`sdlc-merge` 多了 `reviewed_sha` arg。
- **skipped／neutral 的 required check 不再算通過**：要放行的 check 名稱列進 `auto_merge.allowed_skipped_checks`。
- **CI 端凍結改讀 repo variable**：merge gate CI 範本讀 `AGENT_AUTOMERGE_FROZEN`（沒設或 `false` 才算未凍結），本機的凍結檔 CI 看不到；要凍結時兩邊都要設。
- **workflow 缺件一律 halted**：任何 agent 沒交件都記為 missing，審查、反駁、比對類步驟缺件時 workflow 回傳 `halted: true`，不再當成「沒有發現」。event workflow 的回傳欄位有調整（`missing`、brief 的 `waiting`／`next`、upstream 的 `claims`／`refuted`）。
- **hook 註冊方式改變**：plugin 註冊的守門 hook 改為「跑不起來時放行」（`onFailure: continue`、timeout 10 秒），需要更嚴格的專案改用 project 模式（見 `SECURITY.md`）。
- **scaffold 檔案有更新**：已 init 的專案要跑 `init.py --upgrade` 才會拿到新的 workflow、schema 與工具。

### 新增

- **升級機制**：`init.py --upgrade` 更新已 init 專案的模版檔，不動使用者的檔案；`--adopt-new-fields` 把新版 profile 新增的欄位併入專案的 `methodology.json`。
- **SA／SD**：`stages.design.system_design` 依 System Design Primer，每個需求做功能層系統設計，有系統面時走完整四步；`sdlc-design` 照它派工，並加派 skeptic 的 `system-design` lens。
- **sdlc-sync**：build 之後、test 之前，以及 merge 因 PR 落後或衝突判 hold 之後，讓分支跟上 base。由 `sync_branch.py` 依情況選 rebase、merge 或中轉分支，中轉分支用 `publish.py --supersedes` 開 PR 取代原 PR。
- **schema 驗證**：補強信封與 payload 的 schema 驗證與語意規則，golden-set 補正例與反例。
- **merge gate 新參數**：`--merged-today`（CI 用 GitHub API 算當日由 `AGENT_MERGER_BOT` 合併的 PR 數，和本機 audit 取大者）、`--frozen <理由>`、`--reviewed-sha`；`upstream_authorized_by` 可寫 `{by, link, scope, expires}`，過期即失效，hook 與 gate 一致。
- **新設定**：`guard.config.json` 的 `acceptance_test_dirs`（dev-implementer 不能寫驗收測試）、`auto_merge.allowed_skipped_checks`；profile 的 `conventions.sync`（`strategy`、`transit_naming`）。
- **`save_run.py abandon`**：需求取消時記錄，之後不能核准、`publish.py` 拒絕推送。政策核准另外擋 skeptic 推翻、agent 缺件，plan 段一律由人核准；重存某段會清掉該段核准，核准要整條閘門鏈都通過。
- **hook 擋 Read／Grep 讀機敏檔**：`guard-paths` 涵蓋 Read、Grep，`.env`、金鑰、憑證一律擋，主 session 也擋。
- **文件一致性檢查**：`tests/check_docs.py` 核對 README、CLAUDE.md 等文件寫的角色、workflow、command 數量與名單和實際檔案一致。
- **repo 安全與發佈**：`SECURITY.md`、本變更紀錄、`tests/check_supply.py`（只用標準庫、plugin 不連網、actions 釘 SHA、版控沒有機敏檔）、Dependabot 每週檢查 GitHub Actions。

### 變更

- **hook 高可用與守門強化**：hook 跑得起來時，輸入壞掉或 shell 指令超過 128KB 一律擋，guard 模組壞掉改用內建最小規則，交件 hook 出錯時放行；合併、核准、推送路徑的守門規則加強（子 shell、pushd、`-C`／`--git-dir` 等）。hook 仍可被繞過，信任邊界在伺服器端，見 `docs/自動合併信任模型.md`〈hook 是減速帶，可以被繞過〉。
- **網站分析 agent 瘦身**：方法論細節收進對應的 skill。
- **commands 改成薄殼**：流程集中到 `tpm-team` skill。
- **文件**：README 補〈更新與釘版本〉；`docs/自訂與擴充.md` 補〈升級已 init 的專案〉；佔位符 `WORKSPACE_DIR` 已移除（工作區根目錄就是 `--target`）。
- **閘門紀錄修正**：核准與 merge gate 的紀錄寫法修正，稽核紀錄與實際決策一致。
- **CI**：三平台跑 Python 3.12，另在 ubuntu 跑 3.9（ubuntu-22.04，latest 已沒有 3.9）與 3.13；所有 action 釘 commit SHA；頂層 `permissions: contents: read`。

### 升級步驟

1. 更新 plugin：執行 `claude plugin update agent-workflow@agentworkflow`，或在 `/plugin` 介面更新。只跑 `/plugin marketplace update agentworkflow` 只會更新目錄清單，**不會**更新 plugin 本身。
2. 在 Claude Code 裡執行 `/reload-plugins` 載入新版。
3. 每個已 init 的專案，在專案根目錄執行 `python3 <AgentWorkFlow>/init.py --target . --upgrade`；要採用新版 profile 的新欄位，加 `--adopt-new-fields`。
4. 把自訂設定裡的 `dev-pr-reviewer`、`tpm-upstream-analyst` 改成 `tpm-reviewer`、`tpm-researcher`。
5. 有放 merge gate CI 的專案：在 repo 設好 variable `AGENT_MERGER_BOT`；要凍結自動合併時設 `AGENT_AUTOMERGE_FROZEN=true`。有 required check 會被 skipped 的，列進 `auto_merge.allowed_skipped_checks`。
6. 在專案根目錄跑 `python3 .claude/teams/tools/build.py --check`，確認內嵌的 schema 與 profile 是最新的。

## [0.1.0]

首次發佈：AI-native SDLC 的 `dev-*` agent 與 `sdlc-*` workflow、事件處理的 `tpm-*` agent 與 `event-*` workflow、網站分析組、交件信封與守門 hook、確定性的 merge gate 與推送工具，以及 `init.py` 套用模版。
