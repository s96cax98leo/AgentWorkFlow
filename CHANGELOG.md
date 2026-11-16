# 變更紀錄

格式依 [Keep a Changelog](https://keepachangelog.com/zh-TW/1.1.0/)，版本號依 [語意化版本](https://semver.org/lang/zh-TW/)。

## [Unreleased]

## [1.0.0]

這一版把全部工具改成一個執行檔，並把規格、知識、程式三層用連結接起來。起因：專案端要另外裝 Python、工具複製進每個專案後各自漂移；agent 有程式索引卻不用、需求整份讀；專案都在用 OpenSpec 而模版不認得。

### 破壞性變更

- **不再需要 Python，也不再有 `.claude/teams/tools/`**。全部工具是單一執行檔 `awf`（Rust），由 plugin 的 `bin/` 提供。指令對照是機械的：

  | 0.x | 1.0 |
  |---|---|
  | `python3 .claude/teams/tools/awf.py next` | `awf next` |
  | `save_run.py <子指令>` | `awf save-run <子指令>` |
  | `merge_gate.py`、`publish.py`、`sync_branch.py` | `awf merge-gate`、`awf publish`、`awf sync-branch` |
  | `pack.py`、`kb.py`、`locate.py`、`recipe.py` | `awf pack`、`awf kb`、`awf locate`、`awf recipe` |
  | `deliverables.py`、`checklist.py`、`check_agents.py` | `awf deliverables`、`awf checklist`、`awf check-agents` |
  | `awf_cost.py`、`awf_watch.py`、`awf_doctor.py`、`awf_script.py` | `awf cost`、`awf watch`、`awf doctor`、`awf script` |
  | `run_brief.py`、`check_envelope.py`、`check_rules.py`、`build.py` | `awf run-brief`、`awf check-envelope`、`awf check-rules`、`awf build` |
  | `python3 <模版 repo>/init.py …` | `awf init …`（不用另外 clone 模版 repo） |
  | `golden-set/run.py` | `awf golden` |
  | 設計系統 skill 的 `scripts/check-tokens.py`、`verify-tokens.py` | `awf tokens check`、`awf tokens verify` |
  | `<工具>.py --selftest` | `awf <子指令> --selftest` |

  子指令的參數與旗標不變。資料格式（run.json、信封、trace、稽核紀錄、決策記憶、交付設定）不變，進行中的 run 升級後可以繼續。
- **hook 的註冊改成執行檔**：`awf hook <guard-shell|guard-paths|envelope|session-start|mark-dirty|prompt-context>`。`--hooks project` 的專案要重跑 `awf init --upgrade`。
- **merge gate 的 CI 範本要重新產生**：改成跑專案 `.claude/teams/bin/` 裡的執行檔，執行前核對 sha256。
- **模版檔搬進 `plugin/`**（`plugin/scaffold/`、`plugin/workflow-templates/` 的片段、情境與 `check_workflows.mjs`）。自己寫腳本引用舊路徑的要改。
- **閘門類指令要找得到專案根**：`--project-dir` → `CLAUDE_PROJECT_DIR` → 從目前目錄往上找最外層含 `.claude/teams/VERSION` 的目錄。找不到時 `awf merge-gate`、`awf save-run stage／approve／escalate`、`awf publish`、`awf done／approve` 會報錯，不退回目前目錄。
- **hook 內部出錯時的行為變了**：team agent 一律擋；主 session 與你自己的 agent 維持最小規則（擋 push、合併、approve、GitHub API 寫入）。0.x 是全部退到最小規則。
- **比 0.x 嚴的判定**（原本會過、現在不會）：
  - 交件或合併決定的文字有兩個 JSON 區塊時只看最後一個，讀不懂就當沒有。
  - 子任務的 `owns`、`depends_on` 這類欄位型別不對是錯誤，不再當成空的。
  - 重存一段時回傳讀不懂，該段標成 invalid 並清掉核准。
  - run.json 已有 recipe 而專案沒有 `methodology.json` 時，核准、推送、merge gate 都拒絕。
  - `guard.config.json` 讀不懂時，team agent 的寫入全擋。
  - `awf run-brief --log` 只能寫 run 目錄底下的紀錄檔。
  - 守門多擋：指令替換（`X=$(awf save-run approve …)`）、`sudo -u`／`find -exec`／`xargs -I{}` 後面的 `bash -c`、`git bisect run`、`trap`、ANSI-C 引號與大括號展開改寫的子指令。

### 新增

- **規格層 `awf spec`**：讀 OpenSpec 的目錄（現行規格與變更的四種差異），需求與情境有穩定 id，`validate` 檢查、`apply` 把變更併回現行規格（預設只印差異）。需求來源可以設成 `openspec`：proposal 就是 intent，design 與 test 存檔時核對規格 id，merge gate 共用。見 `docs/規格層.md`。
- **知識圖 `awf kg`**：把規格、文件、決策、連結帳本、程式索引（tree-sitter：TypeScript／TSX、JavaScript、Python、Go、Java、Rust、PHP、Ruby）接成一張圖。`locate`、`trace`、`impact`、`check`、`neighbors` 五個查詢只回 `路徑:起訖行` 加理由；`export` 寫成 Obsidian 可以開的 vault。`knowledge.repos` 可以收別的 repo（唯讀）。見 `docs/知識圖.md`。
- **連結**：profile 有 `links` 時，每段存檔把宣告的連結（需求→run、驗收條件→規格小節／檔案／測試／commit、決策→需求、改動→交付文件小節）記進 `<runs_dir>/links.jsonl`；`links.enforce` 開啟時連結不齊不能政策核准。
- **自動接點**（都能在 `knowledge.auto` 關掉；失敗只警告）：`awf next` 每段開跑前更新圖；資料包帶圖查到的行段，派工單多一行「只讀這些行段」；merge 之後提醒把規格差異併回現行規格；結案後重寫 vault；開 session 帶幾行摘要；提問時帶位置（預設關）。預設不會自動建第一份圖。
- **中斷回收**：plan 段被中斷時，條件全部核對得過的調研結果會被沿用，不用整段重來。
- **`awf program`**：`<runs_dir>/program.json` 記多個需求的相依，`status`、`next` 只排順序與提示。
- **init 訪談**：`awf init --detect` 先偵測專案（語言、測試與 lint 指令、commit 樣式、有沒有 `openspec/`、既有 CI）；寫入前列「偵測到／你的回答／會寫成」對照表；答案存成 `init-answers.json`，`--reconfigure [主題]` 可重問。
- **由你決定的設定**（預設都不啟用或等於原值）：
  - `scrub` 與 `awf scrub`：自己訂規則的去客戶化掃描，`when: ["publish"]` 時命中就不推。輸出不印規則與命中的字串。
  - `ci` 與 `awf ci render`：專案的測試／lint CI，預設只印不寫；必過的 job 併進 merge gate 的 required checks。
  - 原本寫死的值改成可設定：分類信心門檻、修正迴圈輪數、審查信心門檻與 Nit 上限、審查 lens、限制檢查的逾時、查詢量與調研上限、事件的角度與步驟上限、紀錄的必要節、備份 ref 樣式、程式查詢工具、信封長度。會放寬閘門或交件驗證的方向只准調嚴。
  - `stages.build.affected_tests_only`：build 的修正迴圈只跑受影響的測試（test 段照舊全跑）。
- **doctor 體檢**：`models.mode` 是 inherit、知識庫沒建、有 `openspec/` 但需求來源沒設、需求來源在工作區外、最近的 run 沒走 `awf next`、CI 檔與 profile 漂移。
- **對外介面**：`awf save-run status --json` 與進度事件有 `schema_version`；recipe 跳過的段標 `skipped`。相容承諾寫在 `docs/監控與整合.md`。
- `awf golden`、`awf tokens`。

### 安全

- 移植完成後與新功能完成後各做一次獨立審查，發現的問題在這一版修掉；擋不住的寫法列在 `SECURITY.md`〈守門擋不住的寫法〉（守門是輔助，信任邊界在伺服器端）。
- team agent 不能寫：`.github/workflows/`、連結帳本、知識圖與 vault、`program.json`、`knowledge.repos` 列的 repo；不能跑 `awf` 會寫入的子指令。
- 執行檔的來源：`plugin/bin/checksums.json` 記每個平台執行檔的 sha256 與原始碼雜湊，CI 核對。這抓得到過期與損壞，證明不了執行檔確實由這份原始碼建出；要自己建置見 `docs/建置與發佈.md`。

### 沒做

- 基準實跑（plan 段的 token 與中斷後重跑的成本）還沒量。
- Windows 上沒有實機驗證過，只靠 CI。
- C# 與 Kotlin 的程式索引沒有放進預設的執行檔（放了會超過大小目標），要自己建置時加 feature。
- 寫死值的盤點還有約三十項沒處理（角色名單、派工單語言、agent 檔裡的細節等）。

### 升級步驟

1. `claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins`。
2. 在專案跑 `awf init --target . --upgrade`：移除沒改過的舊工具副本（改過的保留並列出）、改寫 hook 註冊、補新的設定鍵。0.x 的 run 目錄不用轉換。
3. 用了 merge gate CI 的專案：`awf init --target . --upgrade --with-merge-gate` 重新產生 CI 檔與專案裡的執行檔。
4. 自己的腳本、CI、文件裡的 `python3 .claude/teams/tools/…` 照上面的對照表改。
5. `awf doctor` 確認安裝；要用知識圖先跑一次 `awf kg build`。

## [0.6.0]

這一版讓過程留下來，並且能變成要交給客戶的文件。起因：另一個專案自己寫了交付文件腳本，而且每個 run 結束後的資料沒有人整理，之後也查不到。

### 破壞性變更

沒有。沒有交付設定（`.claude/teams/deliverables.json`，或 profile 的 `deliverables` 設 `enabled: true`）的專案，流程完全不變：不多派 agent、派工單不變、不產生也不檢查交付文件。

### 新增

- **結案摘要**：`save_run.py close` 與 `abandon` 寫 `<run_dir>/archive/結案摘要.md`（需求、recipe、各段結果與核准、產物、commit、驗證結果、過程中的問題、執行紀錄連結）。內容可重現，寫不出來只警告。`save_run.py archive <run_dir>` 可以重寫。
- **知識層收錄過程**：runs_dir 底下有 run 時，`kb.py` 預設來源多了結案摘要，之後規劃時查得到。
- **交付文件型錄**：八組、56 種文件，每種標來源類型（`auto` 從 SDLC 產物產生、`assisted` 人補內容、`external` 外部證據只檢查有沒有附），另有五個情境組合（黑客松、概念驗證、內部工具、委外專案、維運移交）。
- **`deliverables.py`**：
  - `configure`：從情境組合、repo 實況（有 migration 才建議資料庫設計文件等，每項附理由）或合約清單挑出要交的文件；只印提案，加 `--write --by <誰>` 才寫入。
  - `init`、`status`（齊全、過期、缺原料、等人補）、`check`（檔頭、過期小節、禁用字、隱私字串、失效連結、總目錄）、`index`、`export`（有 pandoc 就轉 docx）。
  - 每個小節記著原料來源；原料改了而內文沒動，那一節就是過期。人可以用 `--reviewed` 標成看過、不用改。
  - **需求追溯矩陣**：每條驗收條件對到 Story、spec 小節、驗證指令、結果、證據、commit，缺一環就標出來。
- **流程接上交付文件**（只在有交付設定時）：design 段要求 design.md 留下對應的小節；deliver 段多派一個 writer，只改這次受影響的小節，另寫 `交付摘要.md`；`awf.py done` 存完 deliver 段自動把交付摘要與核准紀錄併進功能開發紀錄、寫追溯矩陣、記錄來源。碰到新領域（例如第一次出現 migration）只提示，不自動加進設定。
- `/agent-workflow:deliverables` skill；說明見 `docs/交付文件.md`。
- profile 新增選填的 `deliverables`；兩份內建 profile 只放範例、不啟用（預設 profile 的範例是業務規則、功能規格、系統架構、資料庫、API、批次作業六份）。

### 安全

- team agent 不能寫 run 目錄的 `archive/`、`deliverables.json`、`deliverables.state.json`，只能跑 `deliverables.py status`／`check`／`affected`。
- **修正既有的守門漏洞**：用 `bash -c`、`sh -c`、`eval`、`pwsh -Command` 包一層，或經 `xargs` 執行，就能跑 `save_run.py`（包括核准）、`pack.py`、`kb.py` 的寫入子指令；現在內層指令照同一套規則檢查。寫 `.claude//teams/` 這種連續斜線，或先 `cd .claude` 再寫 `teams/`，也會擋。
- 交付文件與交付摘要掃隱私字串（電子郵件、本機路徑與暫存目錄、金鑰與 token、密碼的值、內部 agent 名稱、內網位址與主機、run 目錄名稱）。比對前先把全形轉半形、去掉零寬字元，也抓拆成兩行的情況。命中只印種類與行號，不印內容；`sync` 命中就不寫。
- 追溯矩陣的證據與指令只留檔名；核准紀錄預設不寫核准者名字（`show_approvers: true` 才寫）。
- 過期判定不能被繞過：小節的來源標記只能加、不能減；只改空白或標點不算改寫；流程自動同步只記這次受影響的小節，清單外的改動交人。

### 沒做

- 0.5.0 留下的三項實跑數字（定位 agent 的 token 佔比、續跑不重花 token、整條流程），以及這一版的整條流程實跑（帶 migration 與 public_api 的需求走到 deliver），改在實際專案量，量到後補進下一版。

### 升級步驟

1. `claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins`。
2. 在專案跑 `python3 <AgentWorkFlow>/init.py --target . --upgrade`：會補上 `deliverables.py`、型錄與 schema。
3. 要用交付文件時，跑 `/agent-workflow:deliverables`（或 `deliverables.py configure`）挑文件，確認後 `--write --by <你>`，再 `deliverables.py init`。

## [0.5.0]

這一版讓流程順一點。起因有兩個：主 session 每段要手動做五、六步；另一個專案回報一個需求跑了兩天，build 重跑 11 輪，多半是停下來問人的小事。

### 破壞性變更

- **workflow 範本搬進 plugin，init 不再複製 workflow 到專案。**
  - 範本在 plugin 的 `workflow-templates/`。每段由 `awf.py next` 用範本、`workflow.config` 與 `methodology.json` 產生 `<run_dir>/.wf/<名>.js`。
  - 專案的 `.claude/workflows/<名>.js` 只有結尾帶 `// awf-override: <版本>` 才採用。沒有標記的同名檔是舊副本，不採用。
  - `init.py --upgrade` 會移除沒改過的舊副本；改過的保留，並列出來。
- **五支事件 workflow 併成 `event.js`**：`event-meeting`、`event-decision`、`event-demo-check`、`event-reqs-change`、`event-upstream`。事件類型變成資料，可以在 profile 的 `events` 加自己的類型。這五支的舊覆寫檔不再生效，`awf.py` 會報錯並說明。workflow 從 16 支變 12 支。
- **所有 workflow 的派工都走執行核心。** profile 有 `models` 時：
  - design 之後各段的模版角色改由模型分級決定模型（預設 `standard`），不再沿用主對話的模型。要維持原樣設 `models.mode: inherit`。
  - `save_run.py stage` 對每一段都檢查模型等級：trace 沒有 `tier`、或模型低於下限判 invalid。自己改過 workflow 的專案要改用 `awfAgent` 派工。
- **每支 workflow 有派工數上限**：`limits.max_agents`（沒寫用 40；design、build、test 依子計劃數與子任務數另加額度）。到上限停下交人。
- **`docs`、`test-only` 兩個 recipe 預設 `gate_mode: plan-only`**：plan 仍由人核准，之後的段結果乾淨就政策核准，不乾淨才交人。要每段都人核准，把 recipe 的 `gate_mode` 設成 `strict`。
- **多一個 SessionStart hook**：plugin 版本和專案的 `.claude/teams/VERSION` 不一致時印一行提示。只提示，不擋。
- **team agent 多三件不能做的事**：寫 `.claude/workflows/` 與 run 目錄的 `.wf/`、執行 `awf.py`（`--selftest` 除外）、寫決策記憶檔。
- `dev-integrator`、`dev-merge-steward` 失敗時不再自動升級重派，交人判斷。

### 新增

- **狀態機 `awf.py`**：主 session 每段只做 `next`（回一個動作）與 `done`（存結果）。跨段的參數（中轉分支、剩餘子任務、PR 號）由它帶。另有 `approve`、`handoff`（接手摘要）、`render`。
- **少停下來問人**：
  - 問題分級。不是 blocking 的問題，agent 照建議先做並記成暫定，下一次要人出面時一次確認。
  - 子任務可以宣告預期內的紅（`expected_red`），整合時不擋。
  - test 的分片驗到不同 HEAD 時，差異只在文件就自動重驗，不停下。
  - 決策記憶：閘門上答過的問題記在 `decisions.jsonl`，之後的資料包會帶出相關的決定，不再重問。
- **限制文件**（profile 的 `constraints`）：指定哪份文件套用到哪些段與路徑，可以要求對照表或跑檢查指令。
- **工具先定位**（`locate.py`）：文件類的調研角度命中夠就不派定位 agent。
- **子任務資料包**：實作的 agent 只拿到 spec 對應的小節、對得上的限制文件、相依子任務的介面。
- **build 分波**：子任務可標 `wave`；`parallelism.wave_pause` 開啟時每波做完停下交人。
- **pattern 庫與自訂腳本**：六個 pattern（fan-out、classify、adversarial、generate & filter、tournament、loop），都要給上限。`awf.py next --script <檔>` 讓 design、build、deliver 段改用自己寫的腳本；先做靜態檢查，存檔的檢查和標準腳本相同，而且不能政策核准。
- **watcher**：`awf.py watch` 把進度寫成 `<run_dir>/progress.jsonl`，`awf.py statusline` 給狀態列用，`progress.notify` 可以接自己的通知指令（預設不設）。這些都不參與閘門。
- **doctor**：`awf.py doctor` 檢查環境、版本、安裝來源衝突、hook、設定，每項附修法。
- **閘門**：`gate_mode`、design 存檔時的拆工檢查、`linked_repos`（工作區以外的我方 repo）、模版角色可以寫本 session 的 scratchpad。
- **續跑**：同一個 session 裡 halted 的段重跑時帶 `resume_from`，已完成的 agent 不重跑。
- 執行紀錄：agent 漏寫紀錄檔時由工具依信封代寫，索引會標出哪些是代寫的。
- 四個薄殼 skill：`/agent-workflow:status`、`cost`、`kb`、`doctor`。
- 專案還沒 init 時，`/sdlc` 會提議用預設值套用。

### 修正

- `sdlc-sync` 策略是 none（已跟上 base）時，回傳沒帶 plan agent 的信封，存檔被判缺件。
- 派工單裡的 run 目錄與資料包改給絕對路徑，agent 不會把紀錄寫到別的 repo。
- 0.4 的三個已知限制：Grep 的 content 模式計入讀取額度、調研重用會看目錄有沒有新增或刪除檔案、派工 label 重複時自動加序號。

### 安全（發佈前獨立審查的修正）

- team agent 先 `cd` 進 `.claude/workflows/`、`.wf/`、`.claude/teams/` 再寫檔會被擋（原本只看指令文字）。
- 覆寫檔或舊副本跑的段、`awf.py done --output` 手動給的結果，都不能政策核准，一律由人核准。
- `awf.py done` 不採用修改時間在未來的 workflow 紀錄；同一支腳本有兩筆以上紀錄時不猜，交人指定。
- 自訂腳本的內容包成函式執行，拿不到 `agent` 與核心的內部狀態；回傳的 trace 一律取自執行核心。
- 執行紀錄的路徑只認 run 目錄底下的檔。
- `--set` 蓋不掉由前段結果推導的 `subtasks`、`sub_plans`、`branch`；事件的 run 不能用 `--workflow` 換成別支。
- 自訂事件類型的 `<類型>.json` 和內建的一樣不准 team agent 寫。
- watcher 的記憶檔被改壞時重置並警告，不中斷；通知不送例外訊息與含路徑字元的名稱。

### 沒做

- CI 依改動範圍決定跑多少、每段開始先檢查主機負載：這兩項是專案的開發規範，不由模版決定。
- 內建的 12 支範本沒有改用 pattern 庫的函式（各自的流程已經寫好而且有情境測試）；pattern 庫目前給自訂腳本用。
- 工具（`.claude/teams/tools/`）仍留在專案，只有 workflow 範本搬進 plugin：agent 檔、hook 訊息、派工單都引用固定路徑。

### 升級步驟

1. 更新 plugin：`claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins`。
2. 在專案跑 `python3 <AgentWorkFlow>/init.py --target . --upgrade`。它會更新工具、移除沒改過的舊 workflow 副本、補 `guard.config.json` 的新鍵。要 profile 的新欄位（`limits`、`models` 等）加 `--adopt-new-fields`。
3. 跑 `python3 .claude/teams/tools/awf.py doctor`，照它列的修法處理。
4. 自己改過 workflow 的專案：對照 plugin 現在的範本重做一份覆寫檔，結尾加 `// awf-override: 0.5.0`；改過那五支事件 workflow 的，改成在 profile 的 `events` 定義。
5. 進行中的 run 可以繼續：`python3 .claude/teams/tools/awf.py next --run <run_dir>`。
6. 不想讓 design 之後的段改用 `standard` 模型：在 `methodology.json` 設 `"models": {"mode": "inherit"}`。

## [0.4.1]

### 變更

- **讀取預算改成預設不啟用，由使用者決定要不要用、門檻多少。**
  - 內建 profile 的 `limits` 不再預設 `mode`，只留建議門檻（`whole_file_lines`、`read_budget`、`max_agents`）。
  - `limits` 沒寫 `mode` 時不啟用（0.4.0 是當成 `enforce`）。要啟用就明寫 `mode: warn` 或 `enforce`。
  - init 問答多一題「要不要啟用讀取上限」，選啟用才接著問門檻；答案檔多一個鍵 `read_limits`。
  - `init.py --upgrade --adopt-new-fields` 補進 `limits` 時只補門檻，不再自動設 `warn`。
- 派工數上限（`limits.max_agents`）不受 `mode` 影響，照舊生效。

### 修正

- `pack.py` 在 Python 3.9 無法執行（用到 3.10 才有的寫檔參數）。
- Windows 上路徑含 `~`（8.3 短路徑）時，讀取預算沒擋到 `cat`／`Get-Content` 整檔讀大檔。

### 升級步驟

- 0.4.0 已經在用讀取預算的專案：`methodology.json` 的 `limits` 裡本來就有 `mode` 的不受影響；自己刪掉 `mode` 靠預設值的，要補回 `"mode": "enforce"`。
- 其餘照 0.4.0 的升級步驟。

## [0.4.0]

這一版先量、再省。起因：實測同一個需求的三次 plan 段各花 21～29M token、21～28 分鐘，95% 是 context 重讀（agent 整檔讀大檔，之後每一輪都重讀），而且每個 agent 都用最貴的模型。

### 實測

基準是 `tests/bench/` 的虛構專案（67 個檔、約 6,100 行）加一份固定需求，主對話用 opus，跑 `sdlc-plan`（feature，三個調研角度）：

| | token | 時間 | 平均 context | agent 數 |
|---|---|---|---|---|
| 首次 | 2.69M | 6.9 分 | 44.7k | 9 |
| 同一個 run 重跑（三個調研角度全部沿用） | 0.76M（首次的 28%） | 3.6 分 | 39.1k | 3 |

- 目標是首次不超過 5M token、10 分鐘、平均 context 40k，重跑不超過首次的 30%。**平均 context 沒有達標**（44.7k；修正前的另一次實跑是 39.7k，在門檻上下浮動）：每個 agent 一開工就有 25～31k 的固定 context，取決於使用者裝了多少 plugin、MCP 與 skill，模版管不到。
- 改版前的數字（21～29M token、21～28 分、平均 context 約 12 萬）量自一個規模大很多的真實專案，和上表不是同一個情境，不能直接相除。能直接比的是形狀：改版前調研 agent 佔九成、每次請求重讀 12 萬 context；改版後分析 agent 每個約 10 萬 token、3 輪。
- 首次的 token 有六成花在三個定位 agent（輕量模型，各 9～11 輪）。這是下一版要壓的地方。


### 破壞性變更

- **`sdlc-plan` 整支重寫**（用新的執行核心 `snippets/kernel.js`）。回傳多了 `models`、`angles`、`reused`、`locations`；調研從「每個角度一個 agent」改成「定位（輕量模型，只回行段）→ 分析（只讀行段）」，所以 plan 段的派工數變多（feature 從 6 個變 9 個）、每個 agent 讀的東西變少。args 多了選填的 `pack`、`reuse`（由 `pack.py` 產生）。
- **profile 有 `models` 時，`save_run.py stage plan` 會檢查模型等級**：trace 每筆要有 `model`、`tier`，低於下限判 invalid。自己改過 `sdlc-plan.js` 的專案要改用 `awfAgent` 派工，或把 `models.mode` 設成 `inherit`。
- **hook 多擋三件事（只對模版角色）**：
  - profile 有 `limits` 而且 `mode` 是 `enforce` 時，整檔讀超過 `whole_file_lines`（預設 400 行）的檔會被擋，讀取量超過額度後 Read 會被擋。新 init 的專案預設啟用；升級的專案用 `--adopt-new-fields` 補進來時先設成 `warn`。
  - team agent 不能寫 runs_dir 底下的 `pack.md`、`reuse.json`、`pack-args.json`。
  - team agent 不能執行 `pack.py`（`--selftest` 除外），`kb.py` 只能跑 `query`／`lint`。
- **共同前綴多兩句**（省 context 的讀法、有 blocking 問題時 status 不能是 done），所有派工單各多約 200 字。

### 新增

- **模型分級**：profile 的 `models`（`mode`、`tiers`、`defaults`、`floors`）。triage 回傳 `model_plan`，工具取 max(AI 建議或預設, 下限, modifier 的 `model_floors`)；內建下限 profile 蓋不掉（`dev-merge-steward`、`dev-security-reviewer`、`tpm-skeptic@merge`、`dev-spec-author@plan` 一律 `strong`，`tpm-skeptic@plan` 至少 `standard`；內建 modifier 的 `model_floors` 也拿不掉）。`tiers` 的對應不能倒掛。內建對應 light＝haiku、standard＝sonnet、strong＝沿用主對話模型。`auto` 模式下沒交件會用高一級重派一次。解析在 `recipe.py` 與 `recipe.js`，golden 兩邊對跑。
- **執行核心 `snippets/kernel.js`**：`awfAgent` 統一做模型解析、失敗升級、派工數上限（`limits.max_agents`，到上限回 halted）。這一版只有 `sdlc-plan` 用它，其餘 workflow 下一版改。
- **讀取預算**：`plugin/hooks/read_budget.py` 與 profile 的 `limits`。只套模版角色；hook 出錯一律放行。
- **知識層 `tools/kb.py`**：`build`／`query`／`lint`，sqlite FTS5（沒有就用純 Python BM25），中文 bigram。產出可以用 Obsidian 打開（`INDEX.md`、`cards/`、`hot.md`）。工作區底下放目標 repo 時（`guard.config.json` 的 `repos_dir`），同一組來源也套到每個 repo。
- **資料包 `tools/pack.py`**：每段開跑前產生 `<run_dir>/<段>/pack.md` 與 `pack-args.json`，不花 LLM token。
- **調研重用**：`<run_dir>/plan/reuse.json` 記各角度來源檔的 sha1；重跑時來源沒變的角度不派工，`save_run.py stage plan` 重新核對。`pack.py --reuse-from <舊 run>` 可以跨 run 沿用，`--keep-research` 在需求文字改了時仍沿用。
- **角度數由 triage 決定**：`angles` 從 recipe 的調研角度裡挑；modifier 帶進來的不能省。
- **量成本 `tools/awf_cost.py`**：讀 workflow transcript，依角色與模型加總 token、時間、平均 context。`save_run.py status --json` 給外部程式讀目前狀態。
- **基準 `tests/bench/`**：`make_fixture.py` 產生虛構專案（可重現），`run_plan.py` 實跑並對照門檻。
- schema：`payloads/locations`、triage 的 `angles` 與 `model_plan`、subtasks 的 `complexity`、methodology 的 `models`／`limits`／`knowledge` 與 modifier 的 `model_floors`。

### 已知限制

- 讀取預算是省 token 的機制，不是安全邊界：`head`、`sed -n`、Grep 的 content 模式不計入額度，UTF-16 的檔不擋。
- 調研重用看的是「上次定位到的檔沒變」，repo 裡新增的檔不會觸發重做。
- 等級較低的模型偶爾會漏寫執行紀錄檔，`save_run.py` 會列為缺件（plan 段由人核准，不影響閘門）。
- `awf_cost.py` 讀的是 Claude Code 的內部 transcript 格式，官方沒有承諾穩定；讀不懂時會明說，不影響流程。
- 每個 agent 一開工就有一段固定的 context（系統提示、工具與 skill 清單、CLAUDE.md），大小取決於使用者裝了多少 plugin 與 MCP，模版管不到。

### 升級步驟

1. `claude plugin update agent-workflow@agentworkflow`，再 `/reload-plugins`。
2. 在每個已 init 的專案跑 `python3 <AgentWorkFlow>/init.py --target . --upgrade`。
3. 要用模型分級、讀取預算、知識層：加 `--adopt-new-fields` 再跑一次，會把 `models`、`limits`（先用 `warn`）、`knowledge` 補進 `methodology.json`。觀察一陣子沒有誤擋，再把 `limits.mode` 改成 `enforce`。
4. 不補這些欄位也能用：沒有 `models` 就全部沿用主對話模型，沒有 `limits` 就不啟用讀取預算。
5. `python3 .claude/teams/tools/build.py --check` 確認內嵌一致。

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
