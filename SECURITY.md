# 安全政策

## 回報漏洞

請用 GitHub 的 **private vulnerability reporting** 私下回報：到本 repo 的 **Security** 分頁，按 **Report a vulnerability** 填表。回報內容只有維護者看得到。

- 請不要開公開 issue、PR 或討論串描述尚未修正的漏洞。
- 回報時請附：受影響的版本（`plugin/.claude-plugin/plugin.json` 的 `version`）、作業系統與 hook 註冊方式（plugin 或 project）、重現步驟，以及你判斷的影響範圍。
- 範例資料請用虛構內容（例如「示範客戶 ACME」），不要附上真實的憑證、token 或客戶資料。

收到後會在 GitHub 的安全通報裡回覆、討論修正方式，修好後發佈新版本並在 `CHANGELOG.md` 註明。

## 支援的版本

只有**最新的 minor 版本**會收到安全修正。舊版請先升級（步驟見 `CHANGELOG.md` 各版本的「升級步驟」）。

| 版本 | 支援 |
|---|---|
| 最新 minor（目前 0.2.x） | 是 |
| 更舊的版本 | 否 |

已 init 的專案不會自動更新 scaffold 的檔案；升級 plugin 後要在專案裡跑 `init.py --upgrade`，scaffold 的修正才會生效。

## 威脅模型摘要

### hook 是 client 端的減速帶，不是信任邊界

`plugin/hooks/` 的守門 hook（`awf_hook.py guard-shell|guard-paths|envelope`）在使用者的電腦上執行，用來擋下 agent 的誤操作：推送受保護分支、寫上游、越權寫入、合併與核准 PR 等。它**可以被繞過**：使用者可以停用 plugin、改設定，有權限的程序也可以不經過 Claude Code 直接操作 git 與 GitHub。

所以真正的信任邊界在**伺服器端**：

- base 分支的 ruleset（或 branch protection）：禁止直接 push、要求 PR、要求 review。
- required status check：`scaffold/.github/workflows/agent-merge-gate.yml.tmpl` 產生的 merge gate CI，從受保護的 base 執行確定性 gate。
- 合併用的身分（GitHub App 或專用帳號）只給需要的權限，搭配 CODEOWNERS。

自動合併的完整信任模型見 `docs/自動合併信任模型.md` 與 `docs/adr-agent-auto-merge.md`。

### hook 跑不起來時放行

以 user scope 安裝的 plugin，hook 會套用在使用者的**所有專案**，包括沒有 init 過的專案。如果「找不到 Python 就擋下所有工具」，使用者會被整個鎖死，所以 plugin 註冊的 hook 採取以下取捨：

- **hook 本身跑不起來**（找不到 Python、timeout、Windows 只有 PowerShell 等）：放行（`"onFailure": "continue"`，timeout 10 秒）。
- **hook 跑得起來**：輸入壞掉一律擋（exit 2）；guard 模組載入失敗時改用內建的最小規則，不會因此全部放行。

### 要更嚴格：project 模式

需要「hook 跑不起來就擋」的專案（例如啟用 `auto_merge`），用 project 模式：

```bash
python3 init.py --target <專案> --hooks project
```

hook 會複製進專案的 `.claude/hooks/`，在專案的 settings 註冊，寫入 Python 的絕對路徑，並可設 `"onFailure": "block"`。即使如此，信任邊界仍然在伺服器端。

### plugin 在所有專案執行，所以 hook 的範圍要小

- **只用標準庫**：不需要安裝任何第三方套件，沒有第三方依賴的供應鏈風險。
- **不連網**：hook 與 plugin 內的 Python 程式不做任何網路呼叫。
- **不讀機敏檔**：hook 只讀 hook 輸入與專案的 `.claude/teams/guard.config.json`、執行紀錄，不讀 `.env`、憑證或私鑰。

以上由 `tests/check_supply.py` 在 CI 強制檢查（另外檢查 GitHub Actions 都釘 commit SHA、版控裡沒有 `.env`、私鑰與 `__pycache__`）。

## 安裝建議

- **釘版本**：要控制更新時機的話，clone 本 repo、checkout 要用的版本 tag，從本機路徑加入 marketplace（見 `README.md` 的安裝段）。
- **review 後再更新**：第三方 marketplace 的自動更新預設是關閉的，建議維持關閉。更新前先讀 `CHANGELOG.md` 與版本之間的 diff，特別是 `plugin/hooks/` 的改動，確認後再執行 `claude plugin update agent-workflow@agentworkflow`（或用 `/plugin` 介面），然後 `/reload-plugins`。
- **伺服器端設定**：照上面「威脅模型摘要」設好 ruleset 與 required check，不要只靠 hook。
