---
name: doctor
description: 檢查 agent-workflow 裝得對不對：環境（python、git、gh、node）、plugin 與專案的版本、同一個 plugin 從多個來源安裝、hook 能不能編譯與有沒有重複註冊、methodology.json 與 guard.config.json。每一項附修法。使用者說「workflow 怪怪的」「hook 沒有作用」「版本對嗎」「檢查安裝」「/agent-workflow:doctor」時使用。
argument-hint: （不用參數）
---

# 檢查安裝（doctor）

只讀、不改任何檔案、不連網。Claude Code 自己也有 `/doctor`，這支要用完整名稱 `/agent-workflow:doctor` 叫用。

1. 專案已套用（有 `.claude/teams/tools/awf.py`）時執行：

   ```bash
   python3 .claude/teams/tools/awf.py doctor
   ```

   Windows 沒有 `python3` 時改用 `python`。

2. 專案還沒套用時，這支工具不在專案裡。告訴使用者這個專案還沒套用，問要不要用 `/agent-workflow:init-workflow` 套用；不要自己找別的路徑執行。

3. 把結果照原樣列給使用者：先講「問題」，再講「注意」，每一項帶工具給的修法。「通過」的項目一句話帶過。

4. 修法裡的指令（`init.py --upgrade`、`claude plugin update`、`claude plugin uninstall`）會改使用者的專案或安裝，**先問過再執行**。`/reload-plugins` 要使用者自己在 Claude Code 裡輸入。

結束碼 1 代表有「問題」；只有「注意」是 0。細節見工具開頭的說明（`.claude/teams/tools/awf_doctor.py`）。
