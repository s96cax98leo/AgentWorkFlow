# 對跑基準

1.0 把工具與 hook 從 Python 移植到 Rust。這個目錄放「行為基準」：同一批案例對兩個實作各跑一次，結果要逐字相同。Python 版移除之後，這些檔留著當回歸基準。

| 檔案 | 內容 | 怎麼重錄 |
|---|---|---|
| `hooks.expected.jsonl` | hook 測試的每次呼叫（模式、輸入雜湊、exit code、stdout、stderr）與每個案例的判定，共 1,335 個案例 | `python3 plugin/hooks/tests/run.py --record tests/parity/hooks.expected.jsonl` |
| `schema.expected.json` | golden-set 每個範例與內建 profile 的完整錯誤清單，加探測案例 | `python3 tests/parity/record_schema.py > tests/parity/schema.expected.json` |
| `recipe.expected.json` | recipe 與模型分級：golden 案例、探測案例、CLI、內建常數 | 見 `tests/parity/record_recipe.py` 開頭的用法 |
| `kb.expected.json` | 知識層：固定文件集上的 412 步劇本（build、增量、query、lint，FTS5 與純 BM25 兩條路）與函式探測 | 見 `tests/parity/record_kb.py` 開頭的用法 |
| `small.expected.json` | run_brief、check_envelope、check_rules：222 步劇本（偵測、摘要、信封的 golden 範例、規則掃描與 commit 檢查） | `python3 tests/parity/record_small.py > tests/parity/small.expected.json` |
| `gates.expected.json` | merge gate 298 例（selftest 轉出與 CLI）、純函式 465 例、sync-branch 86 例；和 Python 不同的案例只准更嚴，列在 `gates_parity.rs` 的 `STRICTER` | `python3 tests/parity/record_gates.py > tests/parity/gates.expected.json` |
| `deliverables.expected.json` | 交付文件與工具先定位：625 步劇本（323 個指令）與 1,956 個函式探測（隱私規則、`body_hash`、過期判定） | `python3 tests/parity/record_deliverables.py > tests/parity/deliverables.expected.json` |
| `tools.expected.json` | awf_cost 357 步、awf_script 455 步、check_agents 125 次執行 | `python3 tests/parity/record_tools.py > tests/parity/tools.expected.json` |
| `build.expected.json` | 內嵌片段的 build 101 次執行、去客戶化掃描 41 次、文件核對 59 次（後兩支在 `crates/xtask`） | `python3 tests/parity/record_build.py > tests/parity/build.expected.json` |
| `pack.expected.json` | 資料包、調研重用、決策記憶：585 步劇本與 1,389 個函式探測；已知差異 4 步列在 `pack_parity.rs` 的 `KNOWN` | `python3 tests/parity/record_pack.py > tests/parity/pack.expected.json` |
| `init.expected.json` | `awf init`：112 個案例、246 次執行（每個選項、既有專案的保護、升級的各種情況、互動問答），比輸出與整棵目標目錄 | `python3 tests/parity/record_init.py > tests/parity/init.expected.json` |

規則：

- 暫存路徑一律換成 `@TMP@`，耗時數字換成 `@MS@`。同一個實作連錄兩次要逐字相同，才算可以當基準。
- 測別的實作：`python3 plugin/hooks/tests/run.py --entry-cmd "<指令>" --record <輸出檔>`，再和基準比對。
- 基準只能在行為確實該變的時候重錄，並在 commit 說明哪些案例變了、為什麼。守門與閘門的判定只能變嚴。
