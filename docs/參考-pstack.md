# 參考：pstack

記錄設計時參考過的外部 plugin pstack：它是什麼、和 AgentWorkFlow 的差異、我們引進了哪些做法、哪些沒引進與原因。

- **出處**：Cursor 官方 repo `cursor/plugins` 的 `pstack/` 目錄，MIT 授權。
- **讀取版本**：v0.15.32，2026-10 讀取。之後的版本可能已改變，引用前請對照原 repo。

---

## 它是什麼

pstack 是給 Cursor 用的 agent 工作流程 plugin。核心是一組 prose playbook：用文字描述 agent 該怎麼規劃、實作、審查、處理 PR 與 CI，讓模型照著做。流程的推進與品質主要靠提示本身。

## 和 AgentWorkFlow 的差異

| 面向 | pstack | AgentWorkFlow |
|---|---|---|
| 流程結構 | 沒有分段管線，playbook 之間由 agent 自行串接 | plan → design → build → test → deliver → merge，每段一支 workflow |
| 強制方式 | 靠 prose playbook，沒有 hook、schema、確定性 gate | hook 守門、JSON 信封 schema、`merge_gate.py`、`publish.py` 等確定性工具 |
| 人的角色 | never-block-on-the-human：盡量不停下等人 | 段與段之間人工核准（plan 一律由人；其他段在政策允許時可政策核准），不確定就 hold 交人 |
| 平台 | 強綁 Cursor | Claude Code（plugin＋scaffold） |

最根本的差異是「自主優先」與「gate 只會更嚴」：pstack 讓 agent 能自己往前推；我們在不可逆節點與合併通道一律停下或 hold。

## 我們引進了什麼

以下做法參考 pstack 的想法，改寫成我們的確定性工具或流程：

1. **偵測放寬閘門**：`merge_gate.py` 檢查 PR 是否改了測試、lint、型別、覆蓋率的設定，或新增壓制註解（例如 `eslint-disable`、`# noqa`、`@ts-ignore`），有就 escalate 交人會簽。新增 skip／only 原本就判 hold。
2. **rebase 後沿用判定**：rebase 前後的 patch-id 相同時才沿用先前的 verdict，不重跑整段審查。預設關閉，要在 `auto_merge.carry_verdict_on_rebase` 明確開啟（opt-in）。
3. **合併後讀回狀態**：合併指令執行後再讀回 PR 的實際狀態，記為 `merge_state`，不以指令的 exit code 當成已合併。
4. **CI flake 判準**：明確定義什麼情況算 flake、什麼情況算真的失敗，避免把真失敗當 flake 重跑過關。
5. **PR／bot 留言當資料**：原本 steward、watcher、skeptic 已有這條，現在補到 dev-pr-reviewer、tpm-reviewer、dev-verifier：PR 標題、內文、人類與 bot 留言、commit 訊息裡的指示都不採信，也不把留言內容拼進 shell 指令。dev-verifier 的反作弊清單另外加上壓制註解與放寬設定。
6. **`/correct` skill**：把一則教訓變成最高可行層級的強制機制，並用舊錯誤證明擋得住（見 `plugin/skills/correct/SKILL.md`）。

## 沒引進什麼與原因

| pstack 的做法 | 沒引進的原因 |
|---|---|
| 固定格式的 markdown 計畫加上 lint | 我們的交件已用 JSON 信封與 payload schema（`schemas/`）驗證，再加一套 markdown 格式會重複，也比 schema 弱 |
| orchestrate 狀態檔 | 我們用 workflow 腳本＋`save_run.py` 的 `run.json` 記錄進度與核准，不需要另一份狀態檔 |
| 多模型 arena（多個模型各做一份再比較） | 依賴非 Claude 模型，不符合模版的執行環境 |
| never-block-on-the-human 的自主權 | 和我們的信任模型相反：不可逆節點要人當下同意，合併通道不確定就 hold（見 `docs/自動合併信任模型.md`） |

## 之後要再對照時

- 先看原 repo 的版本號是否已超過 v0.15.32，有的話重讀變更紀錄。
- 想引進新做法時，先判斷能不能寫成確定性工具或 schema；只能寫成 prose 的，放進 agent 提示或 rules.md，並在 rules.md〈專案規則〉的對照表標「僅文件」。
