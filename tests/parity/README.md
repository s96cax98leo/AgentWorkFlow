# 對跑基準

1.0 把工具與 hook 從 Python 移植到 Rust。這個目錄放「行為基準」：同一批案例對兩個實作各跑一次，結果要逐字相同。Python 版移除之後，這些檔留著當回歸基準。

| 檔案 | 內容 | 怎麼重錄 |
|---|---|---|
| `hooks.expected.jsonl` | hook 測試的每次呼叫（模式、輸入雜湊、exit code、stdout、stderr）與每個案例的判定，共 1,335 個案例 | `python3 plugin/hooks/tests/run.py --record tests/parity/hooks.expected.jsonl` |
| `schema.expected.json` | golden-set 每個範例與內建 profile 的完整錯誤清單，加探測案例 | `python3 tests/parity/record_schema.py > tests/parity/schema.expected.json` |
| `recipe.expected.json` | recipe 與模型分級：golden 案例、探測案例、CLI、內建常數 | 見 `tests/parity/record_recipe.py` 開頭的用法 |
| `kb.expected.json` | 知識層：固定文件集上的 412 步劇本（build、增量、query、lint，FTS5 與純 BM25 兩條路）與函式探測 | 見 `tests/parity/record_kb.py` 開頭的用法 |

規則：

- 暫存路徑一律換成 `@TMP@`，耗時數字換成 `@MS@`。同一個實作連錄兩次要逐字相同，才算可以當基準。
- 測別的實作：`python3 plugin/hooks/tests/run.py --entry-cmd "<指令>" --record <輸出檔>`，再和基準比對。
- 基準只能在行為確實該變的時候重錄，並在 commit 說明哪些案例變了、為什麼。守門與閘門的判定只能變嚴。
