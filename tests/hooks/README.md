# hook 的案例

`awf hook <guard-shell|guard-paths|envelope|session-start>` 的守門案例，資料檔加一支重放測試，不需要 Python。

| 檔案 | 內容 |
|---|---|
| `cases.jsonl` | 一行一個步驟：搭環境（寫檔、git 指令）與案例（一次 hook 呼叫和預期結果）。 |
| `fixtures.json` | `blobs`：重複或很長的檔案內容，步驟用名稱引用；`python_stub`：假的 `python3`（見下面〈repo 自己的 guard〉）。 |
| `crates/awf/tests/hooks_cases.rs` | 重放：`cargo test -p awf --test hooks_cases`。exit code、stdout、stderr 逐字比對。 |

入口層（故障時的最小規則、壞輸入、延遲、沒 init 的專案、SessionStart、`hooks.json` 的形狀）在 `crates/awf/tests/hooks_entry.rs`。

## 格式

- `{"group": "<名稱>"}` 開始一組。**每一組有自己的暫存目錄，從空的開始，照檔案裡的順序執行**；組和組互不影響，測試平行跑各組。
  - 加了 `"independent": true` 的組，裡面的 hook 呼叫彼此獨立（沒有哪一次呼叫會留下狀態），測試會把它拆成幾份、各自重建環境後平行跑。
    要加「會留下狀態」的案例（讀取預算會記帳）或 `check` 步驟，就放進沒有這個標記的組，或開新的一組。
- `{"#": "…"}` 是註解。
- 字串裡的 `@TMP@` 是這一組的暫存目錄；`@REPEAT:<次數>:<單位字數>:<單位>@` 展開成重複的內容（例如 `@REPEAT:1000:4:A=1\n@`）。

搭環境的步驟：

| `op` | 欄位 | 作用 |
|---|---|---|
| `write` | `path`，加 `text`／`blob`（`fixtures.json` 裡的名稱）／`hex` 其中一個 | 寫檔，上層目錄自動建立 |
| `mkdir`、`rm` | `path` | 建目錄；刪檔或整個目錄 |
| `copy` | `from`（相對於 repo 根）、`to` | 複製整個目錄（專案的 schemas 取自 `scaffold/`） |
| `git` | `cwd`、`args` | 跑 git。作者與日期固定、不讀使用者的 git 設定；remote 只是設定裡的網址，不會連出去 |
| `mtime` | `path`、`days_ago` | 把修改時間調到幾天前 |

案例：

| `op` | 欄位 | 算幾個案例 |
|---|---|---|
| `hook` | `mode`、`proj`（`CLAUDE_PROJECT_DIR`）、`stdin`（物件）或 `stdin_raw`（原樣的字串）、`rc`；`stdout`、`stderr` 沒寫＝預期是空的；選填 `env`（多給的環境變數） | 有 `name` 算 1 個；`also` 裡每一項（`stderr_has`：stderr 含其中任一段字）各算 1 個；`rc_not` 另外要求 exit code 不在清單裡 |
| `check` | `exists`／`absent`；`json` 加 `expect`（欄位相等）或 `range`；`files`（`dir`、`recursive`、`prefix`、`suffix`、`contains`、`except`）加 `count`（`none`／`some`）；`jsonl` 加 `where` 與 `field`＋`set` 或 `count`；`hooks_file` 加 `rule`（註冊設定的形狀） | 1 個 |
| `perf` | `fn`（`check_read`／`check_shell`）、`times`、`stat`（`median`／`mean`）、`limit_ms`；`counted` 是量完之後的狀態檢查 | 1 個，有 `counted` 再加 1 個 |
| `na` | `name`、`want`、`why` | 1 個（Python 版才有意義的案例，只列名稱與原因，不執行） |

`hook` 的 `stdin` 會用 Python `json.dumps(…, ensure_ascii=False)` 的寫法送出（逗號與冒號後面各一個空白、鍵的順序照檔案），和原本的測試台一樣。
子行程的環境是固定的：`PATH`（最前面是假的 `python3`）、`HOME`（空目錄）、`TMPDIR`、`CLAUDE_PROJECT_DIR`、`AWF_READ_BUDGET_DIR=@TMP@/rb-state`，其他一律不帶。

## 新增案例

新增守門規則時補「一個該擋、一個該放，Bash 與 PowerShell 各一」：在合適的組最後加 `hook` 行（或開新的一組，先寫 `guard.config.json`）。例：

```json
{"op": "hook", "name": "guard-shell：示範該擋的指令", "mode": "guard-shell", "proj": "@TMP@/ws", "stdin": {"hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": "@TMP@/ws", "tool_input": {"command": "…"}}, "rc": 2, "stderr": "…"}
```

stderr 的原文不用手打：先只寫 `rc`，跑

```bash
AWF_HOOKS_RECORD=/tmp/cases.actual.jsonl cargo test -p awf --test hooks_cases
```

測試會失敗並列出差異，同時把這次實際的結果照同樣的格式寫到那個檔；確認訊息是你要的，再把那幾行抄回 `cases.jsonl`。全部通過時，寫出來的檔和 `cases.jsonl` 逐字相同。
既有案例的預期只在行為確實該變的時候改，commit 要說明哪些案例變了、為什麼；守門的判定只能變嚴。案例數只能加（測試會檢查不少於 1,335）。

## repo 自己的 guard

hook 會轉呼叫目標 repo 的 `.claude/hooks/guard-git.{py,sh}`、`guard-paths.{py,sh}`。`.py` 的要用 PATH 上的 Python 執行；
為了不依賴 Python，測試在 PATH 最前面放一個假的 `python3`（內容在 `fixtures.json` 的 `python_stub`），它把腳本交給 `sh`。
所以案例裡的 `guard-*.py` 內容其實是 sh 腳本：測的是轉呼叫的機制（找腳本、找直譯器、傳 stdin、依 exit code 與 stderr 判斷），不是 Python 本身。

## 來源

1,335 個案例從 0.6 的 Python 測試台（`plugin/hooks/tests/run.py`）轉過來，轉換時逐一核對過 `tests/parity/hooks.expected.jsonl`（每次呼叫的輸入雜湊、exit code、stdout、stderr，每個案例的名稱與判定）。其中 4 個是 `na`（把 `read_budget.py` 改壞後載入，單一執行檔沒有這種狀態）；「舊 session 目錄清理」3 個與「讀取預算耗時」3 個原本在 Python 行程內呼叫，這裡改成對執行檔與函式庫做同樣的檢查，門檻不變（50ms）。另外加了 12 個專案 settings 範本的註冊檢查（標 `"added": true`，不算在 1,335 裡）。
