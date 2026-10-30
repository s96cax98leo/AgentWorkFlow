# {{PROJECT_NAME}} agent team

主 session 本身就是執行者：把事件和開發需求派給 agent 平行處理，agent 之間用 JSON 信封交接，需要人決定的地方由主 session 停下來問。agents、skills、slash commands 和 hooks 都來自 `agent-workflow` plugin；這個目錄放專案自己的規則、schema、開發規範 profile、檢查工具與設定。入口是 `tpm-team` skill（也可以用 `/sdlc`、`/event`、`/brief`、`/demo-check`）。

## 結構

| 位置 | 內容 |
|---|---|
| plugin `agents/dev-*.md`（10 個） | 開發角色：spec-author、acceptance-tester、implementer、integrator、verifier、design-reviewer、security-reviewer、pr-reviewer、merge-steward（可信任的合併 agent）、release-watcher（合併後監看） |
| plugin `agents/tpm-*.md`（6 個） | 通用角色：planner、researcher、upstream-analyst、reviewer、skeptic、writer |
| plugin `hooks/` | 單一入口 `awf_hook.py`，有三個模式：`guard-shell`（Bash 與 PowerShell）、`guard-paths`（轉呼叫目標 repo 的 guard）、`envelope`。附測試 |
| `.claude/workflows/sdlc-*.js`（7 支） | AI-native SDLC：plan、design、design-revise、build、test、deliver、merge（Workflow 工具只讀專案的 `.claude/workflows`） |
| `.claude/workflows/event-*.js`（8 支） | 簡報、上游變動、會議、PR 審查、需求變動、決策落地、Demo 前檢查、合併後監看 |
| `.claude/teams/rules.md` | 硬規則：通用規則＋專案規則 |
| `.claude/teams/guard.config.json` | 守門設定，包括：<br>・目錄：run、repo<br>・repo：上游、fork、禁止路徑<br>・規範：允許的分支前綴、受保護分支、任務編號前綴、署名行<br>・開關：主 session 開關、`auto_merge` |
| `.claude/teams/methodology.json` | 本專案的開發規範 profile（init 依使用者的習慣產生）：各段做法、慣例、平行處理參數（`parallelism`）。`build.py` 會把它內嵌進 `sdlc-*.js`，改了要重跑 |
| `.claude/teams/methodologies/` | 模版內建的 profile：`default`（模版原本的做法）、`lightweight`（範例）。新增 profile 時從這裡複製 |
| `.claude/teams/schemas/` | 交件信封與各種 payload 的 schema（唯一來源），另有 `methodology.schema.json` |
| `.claude/teams/tools/` | 檢查、落檔與確定性工具，見〈工具〉 |
| `.claude/teams/golden-set/` | schema 的正反例，與官方 jsonschema 交叉比對 |
| `{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>/` | 每次執行的信封、筆記與 `run.json`（閘門核准紀錄），見 `{{RUNS_DIR}}/README.md` |

## 需求開發：AI-native SDLC

| 段 | workflow | 做什麼 | 結束時 |
|---|---|---|---|
| plan | `sdlc-plan` | 4 個 researcher 平行調研 → spec-author 寫 intent（需求能拆時附「範圍切分建議」）→ skeptic 挑錯 | 閘門：核准 intent |
| design | `sdlc-design` | spec-author 出大綱（`plan-outline`）判斷規模。<br>・不拆：一人寫 spec、plan 與子任務。<br>・拆：每個子計劃一個 spec-author 平行規劃，寫在 `specs/<epic>/sub/<id>/`（位置依 `spec_layout`），再由一人彙整、核對介面與檔案所有權並 commit。<br>最後 skeptic 平行檢查一致性 | 閘門：核准 spec＋plan（要修改時跑 `sdlc-design-revise`） |
| build | `sdlc-build` | 依 profile 決定先寫測試或後補測試。<br>子任務依相依分層，同一層各自開 worktree 平行實作，每層由 integrator 合併 | 中斷（halted）時交人 |
| test | `sdlc-test` | verifier 逐條驗收並做反作弊檢查；有子計劃時每個子計劃一個 verifier，另加一個整合分片，平行驗證後彙整成 `verifier.md`。<br>設計審查、安全審查同時進行。<br>失敗就修正，最多 2 輪；失敗分散在多個子計劃時平行修正，再合併 | 閘門：確認驗證結果 |
| deliver | `sdlc-deliver` | pr-reviewer 從四個面向審查，skeptic 逐則反駁，writer 寫 PR 說明草稿與執行紀錄 | 閘門：推送、開 PR 由決策者決定 |
| merge | `sdlc-merge` | `auto_merge` 啟用時，merge-steward 對照需求與證據，`merge_gate.py` 判定。<br>一致就合併；不一致就產出 followup 新需求並退回 PR | 合併後跑 `event-post-merge` |

- **平行參數**：在 `methodology.json` 的 `parallelism`。
  - `split_threshold`：預估子任務超過幾個就拆子計劃。
  - `max_sub_plans`：子計劃數上限。
  - `shard_verification`：有子計劃時驗證要不要分片。
- **加入自己的 agent**：
  - 用 `/add-agent` 設定，寫在 `methodology.json` 的 `extensions` 與 `guard.config.json` 的 `extension_agents`。
  - 擴充 agent 和該段的內建 agent 平行執行，交 findings，結果寫在 `<run_dir>/ext-<name>.md`。
  - 只能套用唯讀類角色；沒登記守門角色時 `build.py` 會拒絕內嵌。
  - build、merge 段不開放擴充。
- **halted 時停下交人**：子計劃範圍重疊或相依循環、彙整後默默少了子任務、彙整時改了驗證結果或各分片驗的不是同一個 HEAD，workflow 都會回傳 `halted: true`。
- **worktree**：
  - epic：`<repo>/.worktrees/<epic>`，分支 `{{BRANCH_PREFIX}}<epic>`。
  - 子任務：`<repo>/.worktrees/<epic>--<task>`，分支 `{{BRANCH_PREFIX}}<epic>--<task>`。
  - 平行修正：`<repo>/.worktrees/<epic>--fix<輪>-<子計劃>`。
  - `.worktrees/` 要加進目標 repo 的 `.gitignore`。
- **產出**：寫在目標 repo 的 `specs/<epic>/`，有 `specs/_templates/` 就照範本。位置與檔名可以用 `methodology.json` 的 `spec_layout` 改（例如寫進 OpenSpec 的 `openspec/changes/<epic>/`），沒設就是這裡寫的預設。產出哪些文件照 profile 決定，例如 intent、spec、plan、progress、verifier、design-review、security-review；拆子計劃時另有 `outline.md` 和 `sub/<id>/`。
- **閘門紀錄**：`python3 .claude/teams/tools/save_run.py init|stage|approve|status <run_dir> …`。全自動流程用 `approve --by policy` 時，工具會讀該段的實際結果：有沒通過的條目、反作弊發現、阻擋性問題、halted，都不能自動核准。
- **維護**：上游變動、需求變動、決策落地、Demo 前檢查發現要開發的事，會產出 intent 草稿，回到 `sdlc-plan`。

## 交接：JSON 信封

- **信封格式**（`schemas/envelope.schema.json`）：
  - `status`：done、blocked、needs_decision。
  - `summary`、`artifacts`。
  - `evidence[{claim, source, result}]`。
  - `confidence`：0–1。
  - `open_questions[{question, blocking, options[{choice, cost}], recommendation, default_applied}]`。
  - `derived_work`：只列，不編號。
  - `payload`：依角色而定。
- **payload**：
  - `subtasks`：拆工結果，有子計劃時帶 `sub_plan`。
  - `plan-outline`：要不要拆子計劃。
  - `findings`、`verdicts`、`criteria`、`build-result`、`items`、`docs`。
  - `merge-decision`：steward 的需求對照表。
  - `spec-archive`：event-spec-archive 的歸檔紀錄（分支、tasks 進度、CLI 原始輸出、diff 範圍）。
- **強制方式**：
  - Workflow 裡用 `agent({agentType, schema})` 派出：schema 由 `build.py` 從 `schemas/` 內嵌，驗證失敗會自動重試。
  - 用 Agent 工具派出：回報最後要附一個 ```json 區塊，`awf_hook.py envelope` 會在 SubagentStop 檢查。
  - Agent Teams：任務描述寫 `output: {{RUNS_DIR}}/<run>/<檔>.json`，由 `TaskCompleted` 檢查。
- **語意規則**：schema 表達不了的部分寫在 `tools/schemalib.py`：
  - 有 blocking 問題就不能 done；blocking 問題要附 ≥2 個選項。
  - 不自編任務編號。
  - 子任務的檔案所有權不能重疊，相依不能指向不存在的子任務。
  - 子計劃的範圍不能重疊、相依不能循環。
  - decision=merge 時不能有不一致的需求。

## 守門（guard.config.json）

- **agent 的限制**：
  - `tpm-*`／`dev-*` 不能 push，也不能對 GitHub 寫入；推送一律由主 session 在決策者同意後執行。
  - 只有命名空間在 `team_agent_namespaces`（預設 `agent-workflow:`）裡的才算模版角色，專案 `.claude/agents/` 裡使用者自己的 agent 不套用角色規則。
  - 名字和模版角色完全相同的 agent 例外，照樣套用。
  - 登記在 `extension_agents` 的擴充 agent 也算，套用登記的角色規則；只能對應唯讀類角色，其他值改套 `tpm-reviewer`。
- **上游與禁止路徑**：
  - 擋推到 `upstream_repos`、擋對它做 gh 寫入；gh 寫入要明確寫 `--repo <fork_repo>`。
  - 擋寫入、移動、刪除 `forbidden_paths`。
- **分支**：新分支只能用 `allowed_branch_prefixes`；不 force push、不用 `--no-verify`；不直接推 `protected_branches`。
- **主 session**：由兩個開關控制。
  - `main_session_git_rules`：分支規則。
  - `main_session_github_rules`：合併、核准、gh 寫入要帶 `--repo`。
  - `auto_merge` 啟用時，主 session 一律不能合併或核准。
- **合併與核准**：
  - 核准（`--approve`、API 送 APPROVE）對任何人一律擋。
  - 合併只放行 `auto_merge.merger_role`（預設 `agent-workflow:dev-merge-steward`），而且必須全部符合：在白名單內、帶 `--match-head-commit`、merge-audit 有相同 SHA 的 merge 紀錄、沒有凍結；上游 repo 另外要有 `upstream_authorized_by`。
  - 真正的信任邊界在伺服器端（ruleset、gate App 的 required check、CODEOWNERS）。
- **角色寫入範圍**：
  - tpm-writer：`writer_dirs` 與 repo 的 `specs/<epic>/*.md`；`methodology.json` 設了 `spec_layout.root` 時，另放行該目錄底下任一層的 `*.md`（讀不到或格式不對時等於沒設）。
  - 其他 tpm-*：`runs_dir`。
  - dev-*：`repos_dir`（空字串＝工作區本身，`.claude/` 除外）與 `runs_dir`。
- **轉呼叫目標 repo 的 guard**：目標 repo 或 worktree 有自己的 `guard-git`、`guard-paths` 時會轉呼叫，先找 `.py`，再找 bash 腳本。
  - 角色名依 `role_map` 轉換，例如 `dev-implementer` → `implementer`。
  - 轉呼叫前去掉 git 的 `-C` 等全域選項。
  - worktree 裡沒有 guard 時，改用主 repo 的。
- **沒有設定檔時**：只套用角色規則，不擋主 session。

## hooks 只註冊一次

hooks 有兩種安裝方式，**只能選一種**，不然同一個事件會檢查兩次：

| 方式 | hooks 從哪裡來 | 專案 `.claude/settings.json` |
|---|---|---|
| plugin（預設，`init.py --hooks plugin`） | `agent-workflow` plugin 的 `hooks/hooks.json` | 只放權限與專案設定，**不放 hooks** |
| 專案內（`init.py --hooks project`，Windows 建議） | 複製到專案的 `.claude/hooks/` | 註冊 hooks；**不要再啟用 plugin 的 hooks** |

檢查方式：在 Claude Code 裡跑 `/hooks`，`awf_hook.py guard-shell` 應該只出現一次。細節見 `.claude/settings.README.md`。

## 工具

| 工具 | 用途 |
|---|---|
| `build.py [--check]` | 把 schema 與 profile 內嵌進 workflow；`--check` 只檢查有沒有一致 |
| `save_run.py` | run 目錄與閘門紀錄：init、stage（存回傳並驗證信封）、approve、status |
| `check_envelope.py` | 單獨驗證一個信封 |
| `check_rules.py` | 檢查文件與 commit 是否符合硬規則（`--audience owner|external`、`--git`） |
| `merge_gate.py` | 確定性的 merge gate：複核需求對照、head SHA、checks、測試數、風險、上限、白名單、凍結，並寫入 `merge-audit.jsonl` |
| `publish.py` | 確定性推送：只推 fork 的功能分支，推送前掃機敏值，依閘門判定後開 PR，並寫入 `publish-audit.jsonl` |
| `checklist.py` | 可驗證的 checklist：以驗證指令的實際結果打勾 |
| `check_workflows.mjs` | 乾跑所有 workflow，包括子計劃、分片驗證、平行修正等情境 |
| `check_agents.py` | 檢查 `tpm-*`／`dev-*` agent 的格式 |
| `schemalib.py` | hook、golden-set、merge gate 共用的驗證器 |

```bash
python3 .claude/teams/tools/build.py --check
uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py
node .claude/teams/tools/check_workflows.mjs --agents-dir <plugin>/agents
python3 .claude/teams/tools/check_agents.py --plugin-dir <plugin>
python3 <plugin>/hooks/tests/run.py --project .
python3 .claude/teams/tools/merge_gate.py --selftest
python3 .claude/teams/tools/publish.py --selftest
```

## 新增一種事件
1. 照 `event-*.js` 的骨架寫 workflow：拆工或固定角度 → 平行處理 → skeptic → writer。schema 用 `/*@schema:<payload>*/ null /*@end*/` 標記，agentType 寫 `NS + 'tpm-…'`。
2. 跑 `build.py` 和 `check_workflows.mjs`。
3. 在 `tpm-team` skill 的事件對照表加一列。
4. 需要新的 payload 時，放到 `schemas/payloads/`，並在 `golden-set/` 補正例和反例。

## 已知限制
- **Workflow 跑到一半不能等人輸入**：所以 SDLC 每段一支 workflow，閘門由主 session 處理。
- **Workflow 腳本不能取時間、不能寫檔**：日期由主 session 放進 `run_dir`，落檔由主 session 呼叫 `save_run.py`。
- **共用 worktree 不能各自 commit**：多個 agent 同時 commit 會搶 index.lock，所以平行規劃子計劃時不 commit，由彙整步驟統一 commit。
- **Workflow 會把使用者訊息當授權轉給 subagent**：敏感流程（推送、對外）改用 Agent 工具逐段派。
- **Agent Teams 是實驗功能**：預設不開。開了之後，有名字的 subagent 會變成 teammate，也不能用 `-p`。
