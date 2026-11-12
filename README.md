# AgentWorkFlow

AgentWorkFlow 是一套 Claude Code 的 agent team 工作流程模版，可以套用到任何專案。它只規定**工作流程**：怎麼拆工、怎麼平行處理、在哪裡停下來讓人核准、用什麼格式交件、誰能合併 PR。開發規範（TDD、DDD、spec 驅動……）和慣例（code style、commit 格式、分支命名、PR 大小、文件格式）都由使用者自己定義。

這份 README 先講**做法**（為什麼這樣設計、一個需求怎麼走完、誰把關），再講安裝與使用。

- **開發需求**走 AI-native SDLC：plan → design → build → sync → test → deliver → merge，段與段之間由人核准。
- **其他事件**（簡報、上游變動、會議、PR 審查、需求變動、決策落地、Demo 前檢查、合併後監看）各自平行處理後產出文件。
- **網站分析**：從技術架構、基礎設施、商業模式、效能、競品五個面向平行分析，再合成一份報告。

所有文件、註解與 agent 提示都用繁體中文。agent 的輸出語言可以設定（`{{OUTPUT_LANGUAGE}}`，預設繁體中文）。工具都是 Python 標準庫，在 macOS、Linux、Windows（PowerShell）上都能執行。目前版本 **0.4.1**，變更見 [`CHANGELOG.md`](CHANGELOG.md)。

## 做法

### 1. 設計原則

| 原則 | 意思 | 怎麼落實 |
|---|---|---|
| 只管流程，不管規範 | 模版不替你決定 TDD、commit 格式、分支命名 | 這些寫在專案的開發規範 profile（`methodology.json`），agent 與 workflow 只從 profile 讀 |
| 人在閘門決定，agent 在閘門之間做事 | agent 不會自己決定「可以進下一段」 | plan、design、test、deliver 結束時停下，重點列在對話裡，核准用 `save_run.py approve` 記錄；plan 一律由人核准 |
| 寫的人不驗、驗的人不改 | 同一個 agent 不能既實作又宣布通過 | 實作、驗證、審查、反駁、合併各是不同角色；唯讀角色沒有 Edit／Write 工具 |
| 不採信 agent 的說法 | agent 回報「通過了」不算數 | 閘門由確定性工具（Python）用實際資料重算：信封內容、head SHA、CI 結果、diff 形狀、執行紀錄檔在不在 |
| 缺件不是「沒有發現」 | agent 失敗、逾時、沒交件，不能當成沒問題 | workflow 把缺件列進 `missing`；審查、反駁、驗證類缺件一律停下交人 |
| gate 只會更嚴 | 工具遇到資料缺漏、讀不懂、對不上，一律 hold | 流程可以用 `save_run.py escalate` 加嚴，工具不會自動降級 |
| 輔助功能失敗不擋流程 | 知識庫、資料包、成本統計壞了只警告 | 這些工具不參與閘門判定；hook 自己跑不起來時放行，不鎖死使用者 |
| 不破壞既有設定 | 套用到既有專案只新增檔案 | `CLAUDE.md` 只追加一行引用，settings 深度合併；角色規則只套用在模版自己的 agent |

### 2. 一個需求怎麼走完

```
你：/sdlc <需求>
 │
 ├─ 主 session 列計畫（哪支 workflow、派幾個 agent、有哪些不可逆動作）──► 你核准才開始
 │
 ├─ plan     triage 判斷需求類型 → 定位行段 → 分析 → 寫 intent → skeptic 挑錯     ──► 閘門：人核准 intent 與 recipe
 ├─ design   大綱 →（太大就拆子計劃平行規劃）→ spec、plan、子任務與檔案所有權      ──► 閘門：核准 spec＋plan
 ├─ build    子任務依相依分層，同一層各開 worktree 平行實作，每層由 integrator 合併
 ├─ sync     確定性工具決定怎麼跟上 main（rebase／merge／中轉分支）
 ├─ test     verifier 在目前 HEAD 逐條驗收＋反作弊檢查；視需要加設計、安全審查       ──► 閘門：確認驗證結果
 ├─ deliver  依審查軸審整條分支，skeptic 逐則反駁，寫 PR 說明                        ──► 閘門：推送、開 PR 由你決定
 └─ merge    （預設關閉）steward 對照需求與證據，merge_gate.py 判定通過才合併        ──► 合併後監看 main
```

- 每一段是一支 workflow 腳本（`sdlc-*.js`），由主 session 用 Workflow 工具執行；腳本不能取時間、不能中途等人輸入，所以需要人決定的地方就拆成兩支。
- 每一段的回傳由 `save_run.py stage` 存進 run 目錄並驗證；下一段開跑前，整條閘門鏈都要通過。
- **不是每個需求都走滿七段**：triage 會建議一個 recipe（新功能、bug、CR、hotfix、重構、依賴升級、文件、補測試、效能、調研、回退），你在 plan 閘門確認後鎖定。細節見下面〈依需求類型分流〉。
- 不論走哪種流程，**intent、rules.md、plan 的人工核准、每個 agent 的執行紀錄**都不能省。

### 3. 誰做事、誰把關

| 做的事 | 角色 | 不能做的事 |
|---|---|---|
| 寫 intent、spec、拆任務 | `dev-spec-author` | 實作 |
| 寫驗收測試 | `dev-acceptance-tester` | 寫產品程式 |
| 實作一個子任務 | `dev-implementer` | 改別人擁有的檔、改驗收測試 |
| 合併子任務分支、跟上 main | `dev-integrator` | 夾帶改動 |
| 驗證、審查 | `dev-verifier`、`dev-design-reviewer`、`dev-security-reviewer`、`tpm-reviewer` | 改程式（唯讀） |
| 反駁別人的結論 | `tpm-skeptic` | 不確定時放行（不確定就判反駁成立） |
| 合併 PR | `dev-merge-steward` | 核准 PR、改任何檔 |
| 調研、寫文件 | `tpm-researcher`、`tpm-writer` | 寫上游、推送 |

- **每個發現都要過反駁**：reviewer 提出的發現，由 skeptic 逐則試著推翻，推翻不了的才採納。
- **沒有任何 agent 能核准 PR**，主 session 也不能。
- 這些限制不是靠提示詞：角色的工具清單沒有對應工具，再加上 hook 依角色擋寫入路徑與指令。

### 4. 確定性工具：閘門用資料判定

agent 之間用 JSON 信封交件（status、summary、artifacts、evidence、confidence、open_questions、derived_work、payload），格式由 schema 定義，交件時 hook 會驗。之後的判定都交給不含 LLM 的 Python 工具：

| 工具 | 判定什麼 |
|---|---|
| `save_run.py` | 存每段的回傳、驗信封、記核准。政策核准時讀實際內容：驗證有沒過的條目、反作弊有發現、有阻擋性問題、agent 缺件、紀錄檔缺件，都不能自動核准 |
| `recipe.py` | 這個需求該走哪些段；之後用實際 diff 反查（說是改文件卻動了程式、沒做 design 卻碰 migration）|
| `sync_branch.py` | 怎麼跟上 main，並檢查中轉分支沒有殘留衝突標記與夾帶改動 |
| `publish.py` | 只推 fork 的功能分支；推送前掃機敏值、檢查閘門鏈都已核准 |
| `merge_gate.py` | 需求對照表、審查時的 head SHA、required checks、測試數、風險等級、每日上限、白名單、凍結旗標；全部寫進稽核紀錄 |

workflow 自己也做確定性核對：子計劃範圍不能重疊、彙整後不能默默少掉子任務、彙整不能改驗證結果、各分片要驗同一個 HEAD。對不上就停下交人。

### 5. 每個 agent 都留紀錄

- 每張派工單指定一個紀錄檔 `<run 目錄>/<段>/<label>.md`：做了什麼、跑了哪些指令與結果、結論、不確定的地方。
- `save_run.py stage` 依 workflow 回傳的 trace 產生 `執行紀錄索引.md`；紀錄檔缺了算缺件，不能政策核准。
- run 目錄留著完整的過程：需求、recipe、每段的回傳、核准紀錄（誰、何時）、每個 agent 的紀錄、合併稽核。之後要追「這個決定是誰在什麼依據下做的」都查得到。

### 6. 省 token：限制讀進來的量，不限制寫出去的量

實測改版前同一個需求的 plan 段要 21～29M token，其中 **95% 是 context 重讀**（agent 整檔讀大檔，之後每一輪都把它重讀一次），輸出只佔幾個百分點。所以省 token 的做法是讓 agent **少讀、讀得準、用對模型**，不是叫它少寫。

**不限制的**：agent 的輸出 token（沒有設輸出上限，也不調低推理強度）；intent、spec、調研結論、審查報告、執行紀錄這些成果檔都沒有長度上限。

**可以限制的（都在 profile 調整）**。其中讀取預算**預設不啟用**：要不要開、門檻多少由你決定，init 時會問，之後也可以直接改 `methodology.json`。

| 機制 | 做什麼 | 設定 |
|---|---|---|
| 模型分級 | triage 建議各角色用 `light`／`standard`／`strong` 哪一級，工具守下限：合併、安全審查、plan 的 spec-author 一定用最強的，AI 降不下去。沒交件會用高一級重派一次 | `models` |
| 讀取預算（預設不啟用） | 啟用後，hook 擋模版角色整檔讀大檔（建議門檻：超過 400 行要分段讀），並給每個 agent 讀取額度，用完要求收斂交件 | `limits.mode`：不寫或 `off`＝不啟用、`warn`＝只提醒、`enforce`＝擋；門檻是 `whole_file_lines`、`read_budget` |
| 知識層 | `kb.py` 把文件索引成「哪個檔的哪幾行」，agent 先查再只讀那幾行；產出可以用 Obsidian 打開 | `knowledge` |
| 資料包 | 每段開跑前由工具（不是 agent）整理好需求、知識庫命中的行段、前面各段的紀錄 | `pack.py` |
| 兩段式調研 | 先「定位」行段，再由分析 agent 只讀那些行段。定位先由工具做（不花 token），不夠才派便宜的模型補 | 內建於 `sdlc-plan`；`locate.py` |
| 子任務資料包 | build 前為每個子任務整理好 spec 對應小節的行段、對得上的限制文件、相依子任務的介面；實作的 agent 不用整份讀 spec | `pack.py build`、子任務的 `spec_refs` |
| 決策記憶 | 人在閘門上答過的問題記下來，之後的資料包帶上相關的，agent 不再重問 | `save_run.py approve --answer`、`decide` |
| 調研重用 | plan 重跑時，來源檔沒變的調研角度直接沿用；閘門會重新核對來源的雜湊 | `pack.py --reuse-from`、`--keep-research` |
| 派工數上限 | 一支 workflow 最多派幾個 agent，到上限就停下交人，不靜默截斷 | `limits.max_agents` |

唯一碰到輸出的是**交件信封**：summary 不超過 400 字、evidence 最多 12 則。它只管 agent 交回給 workflow 的那份 JSON；細節照樣完整寫在檔案裡，信封只放路徑。

啟用讀取預算的方式：init 問答選 `warn` 或 `enforce`，或在答案檔寫 `"read_limits": {"mode": "enforce", "whole_file_lines": 400, "read_budget": 800000}`，或直接在 `methodology.json` 的 `limits` 加 `mode`。額度用完會讓 agent 提早收斂，大型專案上可能影響調研的完整度，建議先用 `warn` 觀察再決定門檻。

在 `tests/bench/` 的虛構專案（67 個檔、約 6,100 行）實測 0.4.0 的 plan 段（讀取預算開 `enforce`）：首次 2.69M token、6.9 分鐘；同一個 run 重跑 0.76M（首次的 28%）。改版前的數字量自大很多的真實專案，兩者不是同一個情境，不能直接相除。完整數字與限制見 [`CHANGELOG.md`](CHANGELOG.md)。想看自己的 run 花了多少：

```bash
python3 .claude/teams/tools/awf_cost.py --project . --run-dir <run 目錄>
```

### 7. 信任邊界在伺服器端

- **hook 是本機的減速帶**：擋推送、寫上游、越權寫入、讀機敏檔，以及所有合併與核准路徑。它可以被繞過，所以不是信任邊界。
- **真正的信任邊界在 GitHub**：ruleset、gate App 的 required check、CODEOWNERS。自動合併預設關閉，啟用前要照 [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) 的檢查清單設定。
- **只推 fork**：上游與禁止的 repo 對所有人一律擋，包括主 session。

### 8. 兩個部分、兩條發佈管道

| 部分 | 內容 | 怎麼進到專案 | 怎麼更新 |
|---|---|---|---|
| `plugin/` | agents、skills、slash commands、hooks | 安裝 Claude Code plugin，每台機器裝一次，所有專案共用 | `claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins` |
| `scaffold/` | 硬規則、workflow、schema、開發規範 profile、工具、golden-set、settings、`CLAUDE.md` 範本 | `init.py` 複製進目標專案，同時換掉 `{{佔位符}}` | 在專案跑 `init.py --upgrade` |

0.5 起 workflow 的範本也在 plugin 裡（`plugin/workflow-templates/`），不複製進專案：狀態機 `awf.py next` 每段用範本加專案的設定產生腳本，plugin 更新後流程跟著更新。專案仍要跑一次 init，因為規則、守門設定、開發規範 profile 與工具是每個專案自己的。

你可以把自己的 agent 加進 plan、design、test、deliver 任一段（`/agent-workflow:add-agent`），也可以整份換掉開發規範 profile；見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)。

**加入自己的限制**：把要 agent 遵守的文件（架構 `ARCHITECTURE.md`、設計 `DESIGN.md`、前後端規範、審查 `REVIEW.md`…）登記在 profile 的 `constraints`，指定套用到哪些段、哪些路徑。工作流程只把它帶給對得上的 agent（前端規範不會發給只改後端的子任務），閘門可以要求逐條對照或跑你指定的檢查指令。

## 需求

- Claude Code
- Python 3.9 以上，只用標準庫。Windows 沒有 `python3` 指令時，改用 `py -3` 或 `python`。
- git；要開 PR 或合併的話另外需要 `gh`。
- Node.js 18 以上，只有乾跑驗證 workflow 時才需要。

## 安裝

### 1. 安裝 plugin（每台機器做一次）

在任何目錄開 Claude Code，執行：

```
/plugin marketplace add https://github.com/s96cax98leo/AgentWorkFlow
/plugin install agent-workflow@agentworkflow
```

也可以在終端機執行，效果相同：

```bash
claude plugin marketplace add https://github.com/s96cax98leo/AgentWorkFlow
claude plugin install agent-workflow@agentworkflow
```

- repo 是公開的，不需要登入 GitHub。網址也可以簡寫成 `s96cax98leo/AgentWorkFlow`。
- marketplace 名稱是 `agentworkflow`，plugin 名稱是 `agent-workflow`。
- 預設裝在使用者層級（scope: user），之後在任何目錄開 Claude Code 都能用。裝好後用 `/plugin` 確認 agent、skill、command 都有載入。

**裝不起來時**

| 症狀 | 原因 | 做法 |
|---|---|---|
| `Cannot add marketplace "agentworkflow": its source doesn't match its extraKnownMarketplaces entry…` | 這台機器之前用本機資料夾（例如 clone 下來的路徑）加過同名的 `agentworkflow`，使用者設定裡記的來源和這次的 GitHub 網址不同 | 先執行 `claude plugin marketplace remove agentworkflow`，再照上面重新 add 與 install。remove 會一併移除從它裝的 plugin，所以 install 要重做 |
| `/plugin marketplace list` 看到 `agentworkflow` 的 Source 是 `Folder (…)` | 同上：目前用的是本機資料夾版本，不會跟著 GitHub 更新 | 想改用 GitHub 版本就照上一列處理；想留本機版本就不用動 |
| 裝好了但看不到 agent 或 command | 還沒重新載入 | 執行 `/reload-plugins`，或重開 Claude Code |
| 在某個專案裡看不到 | 當初裝在專案層級（`--scope project` 或 `local`） | 用 `claude plugin install agent-workflow@agentworkflow --scope user` 裝到使用者層級 |

### 2. 在目標專案套用模版（每個專案做一次）

```bash
git clone https://github.com/s96cax98leo/AgentWorkFlow.git ~/tools/AgentWorkFlow
python3 ~/tools/AgentWorkFlow/init.py --target ~/work/my-project
```

init 會先逐一詢問佔位符的值，再確認你的開發習慣：開發規範 profile、測試要不要先行、code style 的工具或指令、commit 格式、分支前綴、PR 大小上限、文件格式。每題都有預設值，直接按 Enter 就套用。也可以在 Claude Code 裡執行 `/agent-workflow:init-workflow`，讓它在對話中問完再幫你跑 init。

init 會在專案寫下 `.claude/teams/VERSION`（套用的模版版本）與 `.claude/teams/workflow.config`（這次的佔位符值），之後 `--upgrade` 與 `--force` 沒給 `--config` 時都會沿用。

要驗證產出，在目標專案根目錄執行：

```bash
python3 .claude/teams/tools/awf.py render --all --out _tpm/wf-check --plugin-dir ~/tools/AgentWorkFlow/plugin
node .claude/teams/tools/check_workflows.mjs --wf-dir _tpm/wf-check --agents-dir ~/tools/AgentWorkFlow/plugin/agents
python3 .claude/teams/tools/check_agents.py --plugin-dir ~/tools/AgentWorkFlow/plugin
python3 ~/tools/AgentWorkFlow/plugin/hooks/tests/run.py --project .
```

## 快速開始

```
/sdlc 在訂單列表加上「匯出 CSV」按鈕，匯出目前篩選後的結果
```

主 session 會先列出計畫：事件類型、workflow、參數、預計派幾個 agent、有哪些不可逆的動作。**你核准後才開始。**

`sdlc-plan` 第一步是 triage：判斷這個需求屬於哪一類（附證據），建議一個 recipe。你在 plan 閘門確認 intent 與 recipe，之後各段照 recipe 執行。以 `feature` 為例：

| 段 | workflow | 做什麼 | 結束時 |
|---|---|---|---|
| 1 plan | `sdlc-plan` | triage 分類；各調研角度先定位行段、再分析（來源沒變的角度沿用上次結論）；spec-author 寫 `intent.md`；skeptic 挑錯 | 閘門：核准 intent 與 recipe |
| 2 design | `sdlc-design` | 先出大綱判斷規模，太大就拆子計劃平行規劃再彙整；產出 spec、plan、子任務與檔案所有權，skeptic 平行檢查一致性 | 閘門：核准 spec＋plan（要修改時走 `sdlc-design-revise`） |
| 3 build | `sdlc-build` | 依 profile 先寫測試或後補測試；子任務依相依分層，同一層各自開 worktree 平行實作，每層由 integrator 合併 | 中斷時交人，用剩下的子任務重跑 |
| 3.5 sync | `sdlc-sync` | `sync_branch.py` 決定怎麼跟上 main：已跟上不動；沒推送過且沒衝突用 rebase；已推送且沒衝突 merge main；有衝突走中轉分支並開 PR 取代原 PR | head 變了就由 test 重新驗證 |
| 4 test | `sdlc-test` | verifier 逐條驗收並做反作弊檢查，視 recipe 加設計與安全審查；失敗就修正，最多 2 輪 | 閘門：確認驗證結果 |
| 5 deliver | `sdlc-deliver` | 依 recipe 的審查軸審 PR，skeptic 逐則反駁，寫 PR 說明草稿 | 閘門：推送、開 PR 由你當下決定 |
| 6 merge | `sdlc-merge` | 啟用 auto_merge 時，steward 對照需求與證據（綁審查時的 head），`merge_gate.py` 判定能不能合併 | 合併後跑 `event-post-merge` 監看 |

完整走一遍的範例見 [`docs/套用範例.md`](docs/套用範例.md)。

## 依需求類型分流（recipe）

內建 11 個 recipe，定義在 profile 的 `recipes`，可以覆寫：

| recipe | 用在 | 走的段 | plan 的重點 | 實測派工數（plan→deliver） |
|---|---|---|---|---|
| `feature` | 新功能 | 7 段全走 | 一般 intent | 31 |
| `bugfix` | 錯誤行為 | 不走 design | 現象、重現、根因 | 17 |
| `change` | CR、改既有功能 | 7 段全走 | 既有 AC 要改、留、刪哪些 | 30 |
| `hotfix` | 線上事故 | 不走 design、sync | 同 bugfix，一定要開後續 run | 13 |
| `refactor` | 不改行為的重構 | 不走 design | 不可改變的行為 | 22 |
| `deps` | 依賴升級 | 不走 design，強制安全審查 | 上游變更與破壞性變更 | 21 |
| `docs` | 只改文件、設定、文案 | plan、build、deliver、merge | 調研 | 12 |
| `test-only` | 補測試、修 flaky | 不走 design、sync | 調研 | 16 |
| `perf` | 效能 | 7 段全走 | 量測基線 | 28 |
| `spike` | 調研、可行性 | 只有 plan，不能推送 | 調研問題與判準 | 8 |
| `revert` | 合併後回退 | 不走 design、sync | 回退原因、範圍、恢復條件 | 16 |

- **只能加嚴**：triage 只給建議，人在 plan 閘門核准後鎖在 run.json；之後只能用 `save_run.py escalate` 加嚴，不能降級。modifiers（`ui`、`security`、`migration`、`public_api`、`large`）疊上去也只會加嚴，例如 `migration` 會強制加 design 段與回滾檢查。
- **不可省**：intent（四個必要節加上該類型的專屬節）、plan 的人工核准、rules.md、每個 agent 的執行紀錄、merge_gate、只推 fork。
- **用 diff 反查**：`publish.py` 與 `merge_gate.py --recipe` 會依實際改動的檔案判斷 recipe 夠不夠，例如 docs 卻改了程式、沒有 design 卻碰到 migration、改了 `.claude/` 或 `.github/` 卻沒做安全審查，一律擋下並提示用 escalate。CI 範本沒有 run.json，所以這項只在本機 publish 與 steward 的 gate 生效。
- **執行紀錄**：每個 agent 把紀錄寫在 `<run_dir>/<段>/<label>.md`，`save_run.py stage` 產生 `執行紀錄索引.md`。測試等長輸出用 `run_brief.py` 先濃縮再讀，完整 log 另存。
- **量測方式與限制**：派工數來自 `tests/check_budget.py` 的乾跑（假回傳，bugfix、hotfix 用 2 個子任務，docs 用 1 個，其他用 4 個），實際數字會隨子任務數、審查發現數而變。乾跑量的是「派幾個 agent」和「派工單有多少字」，量不到 agent 實際讀檔與輸出的 token。

細節見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)〈依需求類型分流〉。

## 核心概念

**閘門**：plan、design、test、deliver 結束時，主 session 把重點列在對話裡，等決策者核准，核准用 `save_run.py approve` 記錄。plan 一律由人核准；其他段可以改用政策核准（`--by policy`），這時工具會讀該段的實際結果：驗證有沒通過的條目、反作弊有發現、有阻擋性的待決問題、agent 缺件、執行紀錄缺件、workflow 中斷，都不能自動核准。需求取消時用 `save_run.py abandon` 記錄，之後任何一段都不能核准，也不能推送。

**交件信封**：每個 `dev-*`／`tpm-*` agent 都用 JSON 信封交件，欄位是 status、summary、artifacts、evidence、confidence、open_questions、derived_work、payload。成果寫成檔案，信封只放路徑；summary 不超過 400 字、evidence 最多 12 則。hook 會驗證信封，不合格就交不出去。

**開發規範 profile**：各段的做法、慣例與 recipe 寫在目標專案的 `.claude/teams/methodology.json`。內建兩份：`default`（spec 驅動、測試先行、Conventional Commits）與 `lightweight`（需求來自 issue、實作後才補測試、格式自由）。改了這個檔下一段開跑時就生效。

**平行處理**：觸發條件寫在 profile 的 `parallelism`：`split_threshold`（預估子任務超過幾個就拆子計劃，預設 8）、`max_sub_plans`（子計劃上限，預設 4）、`shard_verification`（有子計劃時驗證要不要分片平行，預設開）。子計劃範圍重疊、彙整後默默少了子任務、彙整時改了驗證結果，workflow 都會停下交人。

**可信任的自動合併**（預設關閉）：
- `dev-merge-steward` 逐條對照需求與證據，證據要綁定 head SHA；對照不一致時產出 followup 新需求並退回 PR。
- `merge_gate.py` 以確定性規則複核對照表、required checks、測試數、風險等級、每日上限、白名單和凍結旗標，結果寫進稽核紀錄。
- 沒有任何 agent 能核准 PR。
- 真正的信任邊界在伺服器端：GitHub ruleset、gate App 的 required check、CODEOWNERS。啟用前請照 [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) 的檢查清單完成設定。

**加入自己的 agent**：在 Claude Code 裡跑 `/agent-workflow:add-agent`，它會問要加在哪一段、套用哪個模版角色的守門規則、派工內容是什麼，需要的話幫你建立 agent 定義檔。設定寫在 `methodology.json` 的 `extensions` 與 `guard.config.json` 的 `extension_agents`。擴充 agent 和該段的內建 agent 平行執行，它的 blocker／important 發現會擋住政策核准。只能套用唯讀類角色；build 段和 merge 段不開放擴充。

**守門 hooks**：入口只有一個，`awf_hook.py`。
- `guard-shell` 同時涵蓋 Bash 與 PowerShell 工具，擋推送、寫上游、推受保護分支，以及所有合併與核准路徑：`gh pr merge`、`--approve`、`gh api`、GraphQL、curl、Invoke-RestMethod。
- `guard-paths` 擋越權寫入，以及用 Read、Grep 讀機敏檔（`.env`、金鑰、憑證）。只能寫紀錄的角色只准寫 runs_dir 裡的 `.md`；所有模版 agent 都不能改 `.claude/teams/`、run.json 與執行紀錄索引。
- `envelope` 驗證交件信封。
- 角色規則只套用在模版自己的 agent 上，你原本的 agent 不受影響。
- 失敗時的取捨：hook 跑不起來（找不到 Python、逾時）時放行，不鎖死工具；跑得起來但輸入壞掉或指令超過 128KB 時擋下；guard 模組壞掉時改用內建的最小規則。要「跑不起來也擋」請用 `init.py --hooks project`。
- hook 只是本機的減速帶，可以被繞過；真正的信任邊界在伺服器端，見 [`SECURITY.md`](SECURITY.md)。

## 指令與事件

| 你說的 | workflow 或 skill | 必要參數 |
|---|---|---|
| `/sdlc <需求>`、開發、修 bug、改需求 | `sdlc-plan` → `sdlc-design` → `sdlc-build` → `sdlc-sync` → `sdlc-test` → `sdlc-deliver` → `sdlc-merge`（實際走哪幾段依 recipe） | repo、epic、requirement、base、run_dir |
| `/brief <簡報>`、照任務單做 | `event-brief`（程式子任務轉進 SDLC） | brief、run_dir |
| 上游有新 PR、main 前進、CI 紅了 | `event-upstream` | what、repo、run_dir |
| 整理會議、逐字稿 | `event-meeting` | source、run_dir |
| 審 PR、審分支 | `event-pr-review` | target、repo、run_dir |
| 需求更新了 | `event-reqs-change` | change、run_dir |
| 某某決定了、定案 | `event-decision` | decision、run_dir |
| `/demo-check`、demo 前、彩排 | `event-demo-check` | target、run_dir |
| 分支落後 main、PR 有衝突 | `sdlc-sync` | repo、epic、run_dir |
| 合併這個 PR | `sdlc-merge` | repo、pr、run_dir |
| 合併後 main 紅了 | `event-post-merge` | repo、run_dir |
| `/agent-workflow:add-agent`、加我自己的 agent | `add-agent` skill | — |
| `/agent-workflow:correct`、把這個錯誤變成做不到 | `correct` skill | 錯誤描述或出處 |
| `/agent-workflow:status`、現在跑到哪 | `status` skill | run 目錄（可省略） |
| `/agent-workflow:cost`、花了多少 token | `cost` skill | run 目錄（可省略） |
| `/agent-workflow:kb`、文件裡哪裡有寫 | `kb` skill | 要查的問題 |
| `/agent-workflow:doctor`、檢查安裝 | `doctor` skill | — |
| 比較方案、A 還是 B | `team-debate` skill | — |
| 分析某個網站 | `site-report` skill | 網址 |

`/event <描述>` 會自動判斷事件類型。事件判斷與閘門的完整規則寫在 `tpm-team` skill。

plugin 提供 4 支 command：`/sdlc`、`/event`、`/brief`、`/demo-check`。其他功能是 skill，用 `/agent-workflow:<skill 名>` 叫用（例如 `/agent-workflow:add-agent`）；依 [Claude Code 文件](https://code.claude.com/docs/en/skills)，沒有其他指令同名時，也可以省略 `agent-workflow:` 前綴。`status`、`cost`、`doctor` 和 Claude Code 內建指令同名，要用完整名稱。

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
| | `dev-merge-steward` | 可信任的合併 agent：對照需求與證據，gate 通過才合併，不一致就退回 PR；永遠不核准 |
| | `dev-release-watcher` | 合併後監看：歸因失敗，提出 revert 與凍結建議，只寫自己的執行紀錄 |
| 事件 | `tpm-planner` | 把事件拆成子任務與相依圖；在 sdlc-plan 裡另做 triage，判斷需求類型 |
| | `tpm-researcher` | 針對一個具體問題調研，結論附 `路徑:行號` 或指令；上游模式唯讀分析上游：預測衝突、影響範圍、與我方分支的交集 |
| | `tpm-reviewer` | 依指定面向審查 PR 或分支，只留建議；deliver 模式依 recipe 的審查軸審整條整合分支，只報信心 ≥0.8 的發現 |
| | `tpm-skeptic` | 專門反駁別人的發現，不確定時判反駁成立 |
| | `tpm-writer` | 把定稿的素材寫成文件，寫完跑 `check_rules.py` |
| 網站分析 | `tech-architect`、`infra-analyst`、`biz-analyst`、`perf-auditor`、`competitor-scout`、`lead-synthesizer` | 分別負責技術架構、基礎設施、商業模式、效能、競品，最後由 lead-synthesizer 交叉驗證、合成總報告 |

上面是 14 個核心角色（dev 9 個、tpm 5 個）與 6 個網站分析 agent。另有 2 個舊名薄殼，相容一版、下一版移除：舊名 `dev-pr-reviewer` 已併入 `tpm-reviewer`，舊名 `tpm-upstream-analyst` 已併入 `tpm-researcher`。還在用舊名的專案請跑 `init.py --upgrade`。

## 更新與釘版本

**更新 plugin**：

```bash
claude plugin update agent-workflow@agentworkflow   # 或在 Claude Code 的 /plugin 介面更新
```

更新完在 Claude Code 裡執行 `/reload-plugins` 載入新版。只跑 `/plugin marketplace update agentworkflow` 只會更新 marketplace 的目錄清單，**不會**更新已安裝的 plugin。

**更新已 init 的專案**：plugin 更新不會動到專案裡的 scaffold 檔案，每個專案要另外跑：

```bash
git -C ~/tools/AgentWorkFlow pull
python3 ~/tools/AgentWorkFlow/init.py --target . --upgrade [--adopt-new-fields]
```

`--upgrade` 會換掉模版檔、保留你的設定，並列出 profile 缺少的新欄位；`--adopt-new-fields` 把這些欄位補進你的 `methodology.json`。從 0.2.x 升上來要用 recipe 分流，或從 0.3.x 升上來要用模型分級、讀取預算、知識層，就要加 `--adopt-new-fields`。細節見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)〈升級已 init 的專案〉。

**釘版本、安全安裝**：

1. 更新前先讀 [`CHANGELOG.md`](CHANGELOG.md)，特別是〈破壞性變更〉與〈升級步驟〉，以及 `plugin/hooks/` 的改動。
2. 要控制更新時機，就不要加 GitHub 上的 marketplace，改成 clone 後 checkout 版本 tag，從本機路徑加入：

   ```bash
   git clone https://github.com/s96cax98leo/AgentWorkFlow.git ~/tools/AgentWorkFlow
   git -C ~/tools/AgentWorkFlow checkout v0.3.0   # 換成你要的 tag；`git tag` 列出已發佈的版本
   claude plugin marketplace add ~/tools/AgentWorkFlow
   claude plugin install agent-workflow@agentworkflow
   ```

   要升級時 checkout 新的 tag，再執行 `claude plugin marketplace update agentworkflow` 與 `claude plugin update agent-workflow@agentworkflow`。本機路徑和 GitHub 網址只能擇一，要換來源先 `claude plugin marketplace remove agentworkflow`（見〈安裝〉的〈裝不起來時〉）。
3. 第三方 marketplace 的自動更新維持關閉。
4. 安全政策與漏洞回報方式見 [`SECURITY.md`](SECURITY.md)。

## init 的選項

```
python3 init.py --target <專案> [--config workflow.config] [--answers <答案.json>] [--defaults]
                [--hooks plugin|project] [--python <路徑>] [--with-mcp] [--with-merge-gate]
                [--upgrade] [--adopt-new-fields] [--force] [--overwrite-user-files]
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
| `--upgrade` | 升級已 init 的專案（建議的更新方式）：模版檔換成這版，使用者的設定保留，列出 profile 缺少的新欄位。沒給 `--config` 時沿用 `.claude/teams/workflow.config`（`--force` 也一樣） |
| `--adopt-new-fields` | 搭配 `--upgrade`：把缺少的 profile 欄位補進 `methodology.json`，既有值不改 |
| `--force` | 覆寫已存在的模版檔；使用者設定的保留規則和 `--upgrade` 相同 |
| `--overwrite-user-files` | 連使用者的檔案（`CLAUDE.md`、`settings.json`、`methodology.json`、`guard.config.json`、〈專案規則〉）也整份覆寫，只在想重設時使用 |

init 會產生：`.claude/teams/`（含 `guard.config.json`、`methodology.json`、`VERSION`、`workflow.config`）、`.claude/settings.json`、`CLAUDE.md`、`.mcp.json.example`、run 目錄。

### 佔位符

| 佔位符 | 意思 | 預設 |
|---|---|---|
| `{{PROJECT_NAME}}`、`{{CLIENT_NAME}}` | 專案名稱、客戶泛稱 | `my-project`、`示範客戶` |
| `{{REPOS_DIR}}` | 我方 clone 所在的目錄（相對於 `--target`） | `repos` |
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

另有 `AWF_HOOK_CMD`、`AWF_HOOK_SHELL` 兩個佔位符，是 `--hooks project` 的啟動指令與 shell，由 init 自動計算，不用填。

### 套用到既有專案

init 對既有專案只新增檔案，你原本的 agent、skill、command 和設定都能照常使用：

- 不動 `.claude/agents`、`skills`、`commands`。
- 已有的 `CLAUDE.md` 不覆寫，只在結尾追加一行 `@.claude/teams/agent-workflow.CLAUDE.md`。
- `settings.json` 和 `.mcp.json` 深度合併，你的值優先。
- 角色規則只套用在帶 `agent-workflow:` 命名空間的 agent，以及和模版角色完全同名的 agent。

主 session 的限制由 `guard.config.json` 的兩個開關控制：`main_session_git_rules`、`main_session_github_rules`。不管開關怎麼設，上游與禁止的 repo 一律擋；`auto_merge` 啟用時，主 session 也一律不能合併或核准。詳見 [`docs/自訂與擴充.md`](docs/自訂與擴充.md)〈套用到既有專案〉。

## repo 結構

```
AgentWorkFlow/
├── .claude-plugin/marketplace.json   # marketplace（名稱 agentworkflow）
├── .github/                          # 本 repo 的 CI（ci.yml）與 Dependabot 設定
├── plugin/                           # plugin 本體（名稱 agent-workflow）
│   ├── agents/                       # agent 共 22 個：14 個核心角色、2 個舊名薄殼、6 個網站分析 agent
│   ├── skills/                       # tpm-team、init-workflow、add-agent、correct、status、cost、kb、doctor、team-debate、site-report 系列、design-system-template
│   ├── commands/                     # 4 支 command：/sdlc、/event、/brief、/demo-check
│   ├── workflow-templates/           # workflow 範本（sdlc 8 支、event 8 支）與 legacy.json（舊副本指紋）；awf.py next 逐段產生腳本
│   └── hooks/                        # hooks.json＋awf_hook.py（guard-shell、guard-paths、envelope）、read_budget.py
├── scaffold/                         # init.py 複製進目標專案的內容
│   ├── .claude/teams/                # rules.md、schemas/、methodologies/、tools/（含 recipe.py、run_brief.py、pack.py、kb.py、locate.py、awf_cost.py、awf.py）、golden-set/
│   ├── .github/workflows/            # agent-merge-gate.yml.tmpl（選用）
│   ├── CLAUDE.md.tmpl、.mcp.json.tmpl
│   └── _runs/                        # run 目錄說明
├── init.py                           # 套用模版（互動問答或讀設定檔）
├── workflow.config.example           # 佔位符的值
├── CHANGELOG.md、SECURITY.md、LICENSE
├── docs/                             # 說明文件
└── tests/                            # 去客戶化掃描、init 煙霧測試、recipe 與派工預算檢查、全部測試入口；bench/ 是基準實跑
```

## 文件

| 文件 | 內容 |
|---|---|
| [`docs/開發模式總覽.md`](docs/開發模式總覽.md) | 整套工作方式總覽：角色、閘門、派工、信封、手機決策 |
| [`docs/ai-native-sdlc.md`](docs/ai-native-sdlc.md) | SDLC 各段的 agent、輸入輸出、閘門、worktree 與整合分支 |
| [`docs/系統設計方法.md`](docs/系統設計方法.md) | SA／SD 的做法：依 System Design Primer 的四步、功能層與系統層、取捨檢查主題 |
| [`docs/事件處理.md`](docs/事件處理.md) | event-* workflow、方案辯論、網站分析 |
| [`docs/agent-協作規則.md`](docs/agent-協作規則.md) | 派工單範本、信封格式、背景 agent、驗收證據 |
| [`docs/自動合併信任模型.md`](docs/自動合併信任模型.md) | 自動合併的防線分層、啟用前檢查清單、revert 演練 |
| [`docs/adr-agent-auto-merge.md`](docs/adr-agent-auto-merge.md) | 自動合併的架構決策紀錄 |
| [`docs/自訂與擴充.md`](docs/自訂與擴充.md) | 開發規範 profile、recipe、平行參數、把自己的 agent 加進 workflow、新增 agent／skill／hook／workflow、套用到既有專案、升級 |
| [`docs/監控與整合.md`](docs/監控與整合.md) | 從外面看進度與成本：`awf.py watch`、事件流、通知、狀態列、OTel |
| [`docs/踩坑手冊.md`](docs/踩坑手冊.md) | 實戰教訓：症狀、原因、做法 |
| [`docs/參考-pstack.md`](docs/參考-pstack.md) | 參考過的外部 plugin（pstack）：差異、引進與沒引進的做法 |
| [`docs/套用範例.md`](docs/套用範例.md) | 用虛構的「示範客戶 ACME」從 init 走到 deliver |
| [`CHANGELOG.md`](CHANGELOG.md) | 各版本的變更與升級步驟 |
| [`SECURITY.md`](SECURITY.md) | 漏洞回報、威脅模型摘要、安裝建議 |

## 開發這個模版

```bash
python3 tests/run_all.py              # 全部：去客戶化掃描 → init 煙霧測試 → hook 測試 → tests/check_*.py → check-tokens
python3 tests/scrub_check.py [路徑…]   # 只跑去客戶化掃描
python3 tests/smoke_init.py           # 只跑 init 煙霧測試；KEEP=1 保留暫存目錄
python3 plugin/hooks/tests/run.py     # 只跑 hook 測試（Bash 與 PowerShell 案例）
python3 tests/check_docs.py           # 文件一致性（角色、workflow、command 的數量與名單）
python3 tests/check_recipes.py        # recipe 的 Python 與 JS 解析結果一致
python3 tests/check_budget.py         # 派工數與派工單字數不超過基準 10%
```

- **CI**（`.github/workflows/ci.yml`）：每個 push 與 PR 都跑 `tests/run_all.py`，矩陣是 ubuntu-latest（Python 3.12、3.13）、ubuntu-22.04（Python 3.9）、macos-latest、windows-latest（pwsh）。CI 用到的 action 都釘 commit SHA。
- **Dependabot**（`.github/dependabot.yml`）：每週檢查 CI 用到的 GitHub Actions 有沒有新版，有就自動開 PR，同時更新 SHA 與版本註解。這些 PR 由 GitHub 的 `dependabot[bot]` 開，是這個 repo 自己的設定，不是外人提交的；CI 綠了、讀過上游 release notes 再決定要不要合併。
- **去客戶化掃描**讀兩份清單：`tests/scrub_terms.txt`（公開的通用規則：本機路徑、憑證格式、個人信箱）與 `tests/scrub_terms.local.txt`（要擋的真實名稱，不進版控）。範例一律用虛構的「示範客戶 ACME」。

給 Claude Code 的開發規則寫在 [`CLAUDE.md`](CLAUDE.md)。

## 授權

MIT，見 [`LICENSE`](LICENSE)。
