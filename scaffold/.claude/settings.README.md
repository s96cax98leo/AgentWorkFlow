# `.claude/settings.json` 與 hooks

`awf init` 依 `--hooks` 從兩份範本挑一份產生 `.claude/settings.json`：

| `--hooks` | 範本 | 內容 | 前提 |
|---|---|---|---|
| `plugin`（預設） | `settings.json.tmpl` | 只有權限 | 安裝並啟用 `agent-workflow` plugin，hooks 由 plugin 的 `hooks/hooks.json` 註冊（跑 plugin 的 `bin/awf hook <種類>`） |
| `project` | `settings.project-hooks.json.tmpl` | 權限＋hooks（init 把 `awf` 執行檔複製到 `.claude/hooks/bin/`，用這份複本的絕對路徑跑 `awf hook <種類>`） | **不要再啟用 plugin 的 hooks** |

專案原本就有 `.claude/settings.json` 時，`awf init` 不覆蓋，改成深度合併：使用者的欄位、permissions、env、自己的 hooks 都保留，只補上模版沒有的鍵，並把 `awf hook` 的 hooks 加進對應事件（每個種類只註冊一次，重跑時就地更新指令；0.x 註冊的 `awf_hook.py` 會換成新的，不留兩份）。`--upgrade`／`--force` 時，0.x 範本放的權限（`Bash(python3 .claude/teams/tools/<工具>.py:*)`）換成 `Bash(awf <子指令>:*)`；你自己加的權限與 hooks 不動。

**同一個 hook 只能註冊一次。** 兩邊都註冊時，每次 Bash／Edit／交件都會檢查兩次，擋下訊息也會重複出現。檢查方式：在 Claude Code 裡跑 `/hooks`，`awf hook guard-shell` 應該只出現一次。

hooks 是同一支執行檔（`awf hook <guard-shell|guard-paths|envelope>`），不需要 Python。shell 守門的 matcher 是 `Bash|PowerShell`，Windows 上 PowerShell 工具送出的指令一樣會被檢查。兩種註冊方式在 hook 跑不起來時的行為不同：

- `plugin`：plugin 的指令要經 `sh` 執行啟動器 `bin/awf`。找不到這個平台的執行檔、逾時，或 Windows 沒有 Git Bash（這一行跑不了）時**放行**（`onFailure: continue`），因為 plugin 會套在使用者所有專案，不能鎖死工具。
- `project`：守門類 hook 設了 `"onFailure": "block"`，執行檔跑不起來時**擋下**。指令直接寫專案裡那份複本（`.claude/hooks/bin/awf`，Windows 是 `awf.exe`）的絕對路徑，不經 `sh`；Windows（尤其沒有 Git Bash 時）建議用這個。執行檔和平台有關，不建議進版控（把 `.claude/hooks/bin/` 加進 `.gitignore`；init 只提示、不改你的 `.gitignore`）：每台機器各自跑一次 `awf init --target <專案> --upgrade --hooks project`。plugin 更新、換機器或專案搬位置之後也用同一個指令重跑（會換掉複本並就地更新 settings.json 裡的路徑）。要複製別的執行檔時設環境變數 `AWF_BIN`。

個人的權限放 `.claude/settings.local.json`（不提交），不要改這份共用設定。

## 如何放寬讀取限制

`permissions.deny` 預設擋 `.env*`、`*.pem`、`*.key`，安全優先。如果專案有非機敏的 `.key` 檔要讓 Claude 讀（例如本地化字串、範例金鑰格式說明）：

1. **優先用個人設定**：在 `.claude/settings.local.json`（不提交）加 `"allow": ["Read(./path/to/sample.key)"]`，只放行那個檔，不要刪共用的 deny。
2. 要全團隊放寬時，改共用 `settings.json`，把 `Read(**/*.key)` 換成更窄的規則（例如 `Read(./secrets/**/*.key)`），並在 rules.md 的「專案規則」記下原因與核准人。
3. `.env` 與 `*.pem` 不要放寬；要判斷有無機敏值只用 `grep -q`。
