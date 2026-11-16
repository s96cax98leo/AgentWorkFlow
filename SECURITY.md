# 安全政策

## 回報漏洞

請用 GitHub 的 **private vulnerability reporting** 私下回報：到本 repo 的 **Security** 分頁，按 **Report a vulnerability** 填表。回報內容只有維護者看得到。

- 請不要開公開 issue、PR 或討論串描述尚未修正的漏洞。
- 回報時請附：受影響的版本（`plugin/.claude-plugin/plugin.json` 的 `version`）、作業系統與 hook 註冊方式（plugin 或 project）、重現步驟，以及你判斷的影響範圍。
- 範例資料請用虛構內容（例如「示範客戶 ACME」），不要附上真實的憑證、token 或客戶資料。

收到後會在 GitHub 的安全通報裡回覆、討論修正方式，修好後發佈新版本並在 `CHANGELOG.md` 註明。

## 支援的版本

目前只支援**最新的主版本**：安全修正只發佈在它最新的那一版（版本號見 `CHANGELOG.md` 最上面一節）。舊版請先升級（步驟見 `CHANGELOG.md` 各版本的「升級步驟」）。

| 版本 | 支援 |
|---|---|
| 最新的主版本（它最新的一版） | 是 |
| 更舊的版本 | 否 |

已 init 的專案不會自動更新 scaffold 的檔案；升級 plugin 後要在專案裡跑 `awf init --upgrade`，scaffold 的修正才會生效。

## 威脅模型摘要

### hook 是 client 端的減速帶，不是信任邊界

`plugin/hooks/` 的守門 hook（`awf hook guard-shell|guard-paths|envelope`）在使用者的電腦上執行，用來擋下 agent 的誤操作：推送受保護分支、寫上游、越權寫入、合併與核准 PR 等。它**可以被繞過**：使用者可以停用 plugin、改設定，有權限的程序也可以不經過 Claude Code 直接操作 git 與 GitHub。

所以真正的信任邊界在**伺服器端**（哪些寫法擋不住，見下面〈守門擋不住的寫法〉）：

- base 分支的 ruleset（或 branch protection）：禁止直接 push、要求 PR、要求 review。
- required status check：`plugin/scaffold/.github/workflows/agent-merge-gate.yml.tmpl` 產生的 merge gate CI，從受保護的 base 執行確定性 gate。
- 合併用的身分（GitHub App 或專用帳號）只給需要的權限，搭配 CODEOWNERS。

自動合併的完整信任模型見 `docs/自動合併信任模型.md` 與 `docs/adr-agent-auto-merge.md`。

### 守門擋不住的寫法

`awf hook guard-shell` 看的是「這次要執行的指令文字」。直接寫出來的 `git push`、`gh pr merge`、`gh api`、`awf save-run approve` 這類指令它認得；`bash -c`、`eval`、`pwsh -Command` 包住的內層指令會展開來看，`xargs` 這類看不到子指令的轉呼叫當成寫入處理。但是下面幾種寫法，從字面看不出最後會執行什麼，hook 擋不住，或只能擋到一部分：

- **複製或改名執行檔再執行**：把 `awf`、`gh`、`git` 複製到別的路徑或換個檔名，再用新名字執行。
- **alias、shell function、符號連結**：先替指令取別名，之後只出現別名。
- **從 stdin 把指令餵給 shell 或直譯器**：指令內容不在指令列上（管線、here-doc、讀檔轉給 `sh`）。
- **先寫成腳本檔再執行**：寫檔和執行分成兩步，各自看起來都無害。
- **透過別的程式的設定間接執行**：git 的 alias 與 hook、`make` 的 target、套件管理工具的 scripts、編輯器或測試工具的設定檔等，讓另一個程式代為執行。

這些不是待修的個別漏洞，而是「在使用者電腦上檢查指令文字」這種做法的極限：同一台機器上有 shell 權限的程序，總找得到 hook 沒看過的寫法。所以：

- hook 只當輔助，用來擋下 agent 的誤操作與明顯的越權，不拿來當唯一的防線。
- 合併、核准、推受保護分支能不能成功，由伺服器端決定（下一段的 ruleset、required check、CODEOWNERS、合併身分的權限）。本機的紀錄（`run.json`、`merge-audit.jsonl`）也不是信任來源：merge gate 的 CI 從受保護的 base 分支重新判定，不採信 PR 或本機帶來的結論。
- 本機的閘門不拿「目前目錄」當專案：專案根是 `--project-dir`、`CLAUDE_PROJECT_DIR`，或從目前目錄往上找到的最外層 `.claude/teams/VERSION`，找不到就報錯。在 PR 的 checkout 裡執行，裡面自帶的 `.claude/teams/`（設定、schema、profile）不會被採用。
- 發現 hook 該擋而沒擋的**直接寫法**，仍然歡迎照上面的方式回報。

### hook 跑不起來時放行

以 user scope 安裝的 plugin，hook 會套用在使用者的**所有專案**，包括沒有 init 過的專案。如果「找不到執行檔就擋下所有工具」，使用者會被整個鎖死，所以 plugin 註冊的 hook 採取以下取捨：

- **hook 本身跑不起來**（找不到執行檔、timeout 等）：放行（`"onFailure": "continue"`，timeout 10 秒）。
- **hook 跑得起來**：輸入壞掉一律擋（exit 2）；守門設定載入失敗或程式內部出錯時改用內建的最小規則，不會因此全部放行。

### 要更嚴格：project 模式

需要「hook 跑不起來就擋」的專案（例如啟用 `auto_merge`），用 project 模式：

```bash
awf init --target <專案> --hooks project
```

hook 改在專案的 settings 註冊，寫入執行檔的絕對路徑，並可設 `"onFailure": "block"`。即使如此，信任邊界仍然在伺服器端。

### plugin 在所有專案執行，所以 hook 的範圍要小

- **單一執行檔**：hook 與工具都是 plugin 內附的 `awf`（Rust），執行時不需要安裝任何直譯器或套件，plugin 裡也沒有會被執行的腳本（啟動器 `bin/awf`、`bin/awf.cmd` 只負責挑對應平台的執行檔）。
- **相依來源固定**：編譯時用到的 crate 鎖在 `Cargo.lock`，只能來自 crates.io（不收 git 來源與 workspace 以外的 path 相依），名單要和允許清單 `crates/xtask/allowed-crates.txt` 完全一致，新增或間接帶進一個 crate 都要有人明確加進清單。
- **發佈檔可核對**：發佈的執行檔附 sha256 與原始碼雜湊（`plugin/bin/checksums.json`，發佈時產生），核對方式見 `docs/建置與發佈.md`。merge gate 的 CI 執行 gate 之前也會先核對執行檔的 sha256。
- **不連網**：hook 不做任何網路呼叫；`awf` 只有在你執行會碰 GitHub 的子指令（例如 `awf publish`）時才透過 `git`／`gh` 連線。乾跑 workflow 用的 Node 腳本只 import 內建模組與相對路徑，而且不用網路模組。
- **不讀機敏檔**：hook 只讀 hook 輸入與專案的 `.claude/teams/guard.config.json`、執行紀錄，不讀 `.env`、憑證或私鑰。
- **CI 的 action 釘版本**：這個 repo 的 CI 與給專案用的 merge gate CI 範本，action 一律釘 40 字元 commit SHA。

相依來源、允許清單、Node 腳本的 import、action 釘 SHA，以及版控裡沒有 `.env`、私鑰與直譯器快取檔，由 `cargo xtask check-supply` 在 CI 強制檢查。

## 安裝建議

- **釘版本**：要控制更新時機的話，clone 本 repo、checkout 要用的版本 tag，從本機路徑加入 marketplace（見 `README.md` 的安裝段）。
- **review 後再更新**：第三方 marketplace 的自動更新預設是關閉的，建議維持關閉。更新前先讀 `CHANGELOG.md` 與版本之間的 diff，特別是 `plugin/hooks/` 的改動，確認後再執行 `claude plugin update agent-workflow@agentworkflow`（或用 `/plugin` 介面），然後 `/reload-plugins`。
- **伺服器端設定**：照上面「威脅模型摘要」設好 ruleset 與 required check，不要只靠 hook。
