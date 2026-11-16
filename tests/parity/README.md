# 回歸基準

這個目錄放 `awf` 各子指令的「行為基準」：一批固定的案例（輸入、exit code、stdout、stderr、寫出來的檔），`cargo test --workspace` 每次照著重跑一遍，結果要和基準逐字相同。行為或訊息一改，測試就會指出是哪個案例的哪一步。

來歷：0.x 的工具與 hook 是 Python 寫的，1.0 改成 Rust 的單一執行檔。改寫時同一批案例對兩個實作各跑一次、逐字比對，這些檔就是當時錄下來的結果；Python 版已經移除，檔案留下來當回歸基準。所以測試名稱與註解裡還看得到「和 0.x 的 Python 版相同」這類說法，指的是基準的出處。當時刻意和 0.x 不同的地方（只准更嚴）列在各測試檔的 `STRICTER`、`KNOWN`，附理由。

| 檔案 | 內容 | 讀它的測試（`crates/awf-core/tests/`） |
|---|---|---|
| `hooks.expected.jsonl` | 0.6 的 hook 測試台錄下的每次呼叫（模式、輸入雜湊、exit code、stdout、stderr）與每個案例的判定，共 1,335 個案例。`tests/hooks/cases.jsonl` 是從它轉出來並逐一核對過的；現在留作出處，不再更新 | （沒有測試讀它） |
| `schema.expected.json` | golden-set 每個範例與內建 profile 的完整錯誤清單，加探測案例 | `schema_parity` |
| `recipe.expected.json` | recipe 與模型分級：golden 案例、探測案例、CLI、內建常數 | `recipe_parity` |
| `kb.expected.json` | 知識層：固定文件集上的 412 步劇本（build、增量、query、lint，FTS5 與純 BM25 兩條路）與函式探測 | `kb_parity` |
| `small.expected.json` | run_brief、check_envelope、check_rules：222 步劇本（偵測、摘要、信封的 golden 範例、規則掃描與 commit 檢查） | `small_parity` |
| `gates.expected.json` | merge gate 298 例（selftest 轉出與 CLI）、純函式 465 例、sync-branch 86 例；和 0.x 不同的案例只准更嚴，列在 `gates_parity.rs` 的 `STRICTER` | `gates_parity` |
| `deliverables.expected.json` | 交付文件與工具先定位：625 步劇本（323 個指令）與 1,956 個函式探測（隱私規則、`body_hash`、過期判定） | `deliverables_parity` |
| `tools.expected.json` | awf_cost 357 步、awf_script 455 步、check_agents 125 次執行 | `tools_parity`、`src/check_agents.rs` 檔內的測試 |
| `build.expected.json` | 內嵌片段的 build 101 次執行、去客戶化掃描 41 次、文件核對 59 次（後兩支在 `crates/xtask`） | `build_parity`，`crates/xtask` 的 `xtask_parity` |
| `pack.expected.json` | 資料包、調研重用、決策記憶：585 步劇本與 1,389 個函式探測；已知差異 4 步列在 `pack_parity.rs` 的 `KNOWN` | `pack_parity` |
| `init.expected.json` | `awf init`：112 個案例、246 次執行（每個選項、既有專案的保護、升級的各種情況、互動問答），比輸出與整棵目標目錄 | `init_parity` |
| `save_run.expected.json` | 閘門：2,305 步劇本（每一條拒絕與違規理由各有案例）與 4,062 個函式探測；和 0.x 不同的 9 項列在 `save_run_parity.rs` 的 `STRICTER`（只會更嚴）與 `KNOWN` | `save_run_parity` |
| `state.expected.json` | 狀態機：1,019 步劇本（468 個指令、75 個函式呼叫；每種動作、每段與事件類型、續跑、政策核准、非標準結果、覆寫檔），12 支腳本的雜湊；不同的 8 項列在 `state_parity.rs` 的 `STRICTER` 與 `KNOWN` | `state_parity` |
| `publish.expected.json` | 推送工具：110 個指令、325 個函式探測、73 個 git 步驟（只用暫存 bare repo 與假 `gh`）；較嚴的 7 項列在 `publish_parity.rs` 的 `STRICTER` | `publish_parity` |
| `watch.expected.json` | watcher：83 次 tick（11 種事件都走到）、512 個函式探測、27 個指令；較嚴的 4 項列在 `watch_parity.rs` 的 `STRICTER` | `watch_parity` |
| `doctor.expected.json` | 安裝檢查：70 次檢查（項目、文字版、JSON 版、結束碼）與 25 個函式探測；0.x 才有的 `python`、`hooks-compile` 兩項在錄的時候就濾掉；1.0 加的體檢五項（只提示）重錄過 | `doctor_parity` |

hook 的案例不在這裡：在 `tests/hooks/`（`cases.jsonl`、`fixtures.json`，格式見那裡的 `README.md`），由 `crates/awf/tests/hooks_cases.rs` 重放。

## 格式

- 每個檔是一份 JSON：照順序做的劇本（檔案操作、指令、只能在程式內呼叫的函式）與不碰檔案的函式探測；各檔的欄位見對應測試檔開頭的說明。
- 會變動的內容換成記號再比：暫存路徑 `@TMP@`、模版的位置 `@SRC@`、耗時數字 `@MS@`、隨 git 版本變動的錯誤文字 `@GIT@`。
- 劇本裡的時間是固定的，測試不取系統時間。

## 怎麼更新

基準只在行為確實該變的時候改，commit 要說明哪些案例變了、為什麼。守門與閘門的判定只能變嚴。

| 基準 | 更新方式 |
|---|---|
| `init.expected.json` | `AWF_RECORD=1 cargo test -p awf-core --test init_parity`：照劇本重跑並把結果寫回。新增或改寫案例時直接改劇本的 `steps`（結果欄位留空）再重錄。重錄前後的差異要逐項看過 |
| `doctor.expected.json` | `AWF_RECORD=1 cargo test -p awf-core --test doctor_parity`：照劇本重跑，把每一步檢查的項目、文字版、JSON 版、結束碼寫回。差異要逐項看過 |
| hook 案例（`tests/hooks/cases.jsonl`） | `AWF_HOOKS_RECORD=<輸出檔> cargo test -p awf --test hooks_cases`：把這次實際的結果照同樣的格式寫到輸出檔，確認後把要改的那幾行抄回 `cases.jsonl` |
| `plugin/legacy-tools.json`（0.x 工具檔的指紋，不在這個目錄） | `AWF_GEN_LEGACY=1 cargo test -p awf-core --test init_v1 legacy_tools_json`，或 `cargo xtask gen-legacy --write` |
| `save_run.expected.json` | `AWF_SAVE_RUN_RECORD=<輸出檔> cargo test -p awf-core --test save_run_parity replay_script`：把「和基準不同、沒有列在 `STRICTER`／`KNOWN`」的指令這次實際的結果寫到輸出檔（一行一筆：第幾步、exit code、stdout、stderr、改到的檔）。逐筆確認是該變的之後，把那幾步的這四個欄位抄回基準（一步一行，格式照原樣） |
| `state.expected.json` | `AWF_STATE_RECORD=<輸出檔> cargo test -p awf-core --test state_parity`：同上，把和基準不同、沒有列名的指令這次實際的結果附加到輸出檔，逐筆確認後抄回基準 |
| 其他 | 沒有重錄的指令：行為確實該變時手動改對應案例的預期值（測試失敗時會印出案例名稱與實際結果），並在 commit 說明 |
