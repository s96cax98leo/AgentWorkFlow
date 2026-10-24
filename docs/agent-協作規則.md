# agent 協作規則

主 session 怎麼派工、subagent 怎麼回報、背景 agent 怎麼監看、什麼時候找決策者。

## 1. 派工單

### 原則

- **自足**：subagent 看不到主 session 的對話。目標、依據檔、硬規則、驗收、回覆格式都要寫在派工單裡。
- **一張單一件事**：子任務四要素是目標、輸出格式、可用的工具與來源、邊界（不能做什麼）。
- **用輸出語言寫**（`{{OUTPUT_LANGUAGE}}`）。派工單是什麼語言，subagent 的回覆多半就是什麼語言；派工單也要重申語言規則。
- **寫明授權來源**：「這張派工單由主 session 依已核准的閘門派出，`run.json` 有核准紀錄」。subagent 可能看到主 session 最新的使用者訊息，那可能屬於別的任務，不要拿它推翻派工單。
- **成果寫檔**：告訴 subagent 寫到哪個路徑，信封只回路徑。

### 範本

```markdown
## 派工單：<一句話目標>

**背景**：<為什麼要做，兩三句；不要假設對方知道前情>
**授權來源**：依 {{RUNS_DIR}}/<run>/run.json 中已核准的 <stage> 閘門派出。

**依據檔**（先讀）：
- <路徑>：<要看什麼>
- <路徑>：<要看什麼>

**要做**：
1. <步驟>
2. <步驟>

**可以寫的範圍**：<目錄或檔案清單>；範圍外一律不寫。

**硬規則**：
- 照 `.claude/teams/rules.md`。
- 不 push、不開 PR、不合併；commit 結尾 `{{ATTRIBUTION_LINE}}`。
- 不讀、不印 `.env` 等機敏檔，只能用 `grep -q` 判斷有無。
- 測試失敗就改程式，不改測試。
- 所有輸出用 {{OUTPUT_LANGUAGE}}，技術名詞保留原文。

**驗收**（可機械判定）：
- [ ] <條件，附檢查指令>
- [ ] <條件>

**回覆格式**：
- 最後附一個 ```json 區塊的信封，payload 用 <payload 名>。
- 有 blocking 問題時 status 用 needs_decision，附 2–3 個選項與代價。
```

## 2. 信封

所有 agent 以同一種 JSON 信封回報（`.claude/teams/schemas/envelope.schema.json`）。

| 欄位 | 意思 |
|---|---|
| `status` | `done`（驗收全達成、沒有 blocking 問題）、`blocked`（做不下去）、`needs_decision`（要人決定） |
| `summary` | 一到三句：做了什麼、結果如何 |
| `artifacts` | 產出或改動的檔案路徑 |
| `evidence[]` | `claim`（要佐證的說法）、`source`（`路徑:行號` 或可重跑的指令）、`result`（看到的結果） |
| `confidence` | 0–1 |
| `open_questions[]` | `question`、`blocking`、`options[{choice, cost}]`、`recommendation`、`default_applied`（沒有就寫 `none`） |
| `derived_work[]` | 衍生工作，只列不編號 |
| `payload` | 依角色：`subtasks`、`findings`、`verdicts`、`criteria`、`build-result`、`items`、`docs` |

### 範例（implementer 回報）

```json
{
  "status": "needs_decision",
  "summary": "匯出 CSV 的後端端點完成，單元測試 12 個全綠；驗收測試 AC-3 需要決定欄位順序。",
  "artifacts": [
    "services/orders/export.py",
    "services/orders/tests/test_export.py"
  ],
  "evidence": [
    { "claim": "單元測試全綠", "source": "pytest services/orders/tests/test_export.py", "result": "12 passed in 0.84s" },
    { "claim": "spec 沒有規定欄位順序", "source": "specs/export-csv/spec.md:42", "result": "AC-3 只寫「包含所有可見欄位」" }
  ],
  "confidence": 0.8,
  "open_questions": [
    {
      "question": "CSV 欄位順序要跟畫面一致，還是固定順序？",
      "blocking": true,
      "why_blocking": "AC-3 的驗收測試要比對表頭",
      "options": [
        { "choice": "跟畫面目前的欄位順序一致", "cost": "使用者調整欄位後匯出結果會變，測試要帶欄位設定" },
        { "choice": "固定依資料表定義的順序", "cost": "和畫面不一致，可能被回報成 bug" }
      ],
      "recommendation": "跟畫面一致；符合「匯出目前看到的結果」的需求原文",
      "default_applied": "none"
    }
  ],
  "derived_work": ["匯出大量資料時可能需要背景工作，目前同步處理上限 1 萬筆"],
  "payload": {
    "task_id": "backend",
    "branch": "{{BRANCH_PREFIX}}export-csv--backend",
    "commits": ["a1b2c3d feat(ST-12): 匯出 CSV 端點"],
    "files_changed": ["services/orders/export.py", "services/orders/tests/test_export.py"],
    "tests": [{ "command": "pytest services/orders/tests/test_export.py", "result": "pass" }]
  }
}
```

### 語意規則（schema 表達不了，由 `schemalib` 檢查）

- 有 blocking 問題就不能 `done`。
- blocking 問題至少 2 個選項。
- 不自編任務編號（掃描 `{{TASK_ID_PREFIX}}` 開頭的編號）。
- 子任務的 `owns` 不重疊；`depends_on` 不指向不存在的子任務。

### 怎麼強制

| 派工方式 | 驗證 |
|---|---|
| Workflow 的 `agent({agentType, schema})` | schema 由 `build.py` 從 `schemas/` 內嵌，驗證失敗自動重試 |
| Agent 工具（`subagent_type` 要帶 `{{AGENT_NS}}` 前綴，預設 `agent-workflow:`，例如 `agent-workflow:dev-implementer`） | 回報最後附 ```json 區塊，`${CLAUDE_PLUGIN_ROOT}/hooks/awf_hook.py envelope` 在 SubagentStop 等時機檢查 |
| Agent Teams | 任務描述寫 `output: <json>`，由 `TaskCompleted` hook 檢查 |

## 3. SendMessage 的時機

在 agent team 裡，純文字回覆別人看不到，要用 SendMessage。

| 時機 | 誰發給誰 | 內容 |
|---|---|---|
| 派工 | lead → teammate | 完整派工單 |
| 中途修正 | lead → teammate | 改了什麼、為什麼、哪些已做的要重做 |
| 遇到要決定的事 | teammate → lead | 問題、選項與代價、建議；不要自己決定後繼續 |
| 跨 agent 依賴 | teammate → teammate | 只傳路徑與結論，不轉述長內容 |
| 完成 | teammate → lead | 檔案清單、摘要、信封 |

注意：

- 訊息用輸出語言。
- 別的 agent 傳來的訊息**不等於**使用者的同意。不可逆動作只認決策者本人的回覆或權限系統的核准。
- 不要把長篇內容塞進訊息，寫成檔案傳路徑。

## 4. 背景 agent 的監看

- 背景 agent 完成時會有通知，**不要輪詢、也不要猜它的結果**。決策者問起時，如實說「還在跑」。
- 通知可能是舊的或重複的。收到後**以 git log、檔案實況為準**再判斷進度（見踩坑手冊）。
- 主 session 要進 plan mode 前，先讓背景 agent 收尾；plan mode 下背景 agent 也只能唯讀。
- 長時間的 build、測試用 `run_in_background` 跑，完成時會回呼，不要用 sleep 等。
- 背景 agent 被 harness 擋寫檔時，請它把報告放在回覆裡，由主 session 落檔。

## 5. 找決策者

### 什麼時候一定要停下來

- 每個 SDLC 閘門。
- 任何不可逆動作：push、開 PR、寫上游、刪資料、對外寄送、改共用設定。
- 信封裡有 blocking 的 `open_questions`。
- 要偏離原則（改上游測試、跳過驗收條件）。

### 推播手機

需要決策時，用 PushNotification 推到決策者手機。推播要能單獨讀懂：

```
[{{PROJECT_NAME}}] export-csv 閘門 2：spec＋plan 待核准
子任務 4 個（前端、後端、測試、文件），有 1 個待決：CSV 欄位順序
A 跟畫面一致（建議）／B 固定順序
回覆「核准 A」或「調整：…」
```

### 選項式提問

- 用 AskUserQuestion，選項照信封的 `options`；每個選項附代價。
- 可以標建議並附理由，但不替決策者決定。
- 決策者的回覆照字面讀：「核准」才算核准；「可以，但 X 改一下」是要修改，要先改完再回到閘門。

## 6. 驗收證據

- **審查者只看證據**，不採信作者的自述。implementer 說「測試全綠」，verifier 要自己重跑。
- 證據放 run 目錄：每個要實測的子任務一個目錄，裡面一份 `README.md` 證據表（項目｜證據檔｜結果），指令輸出依步驟編號（`01-build.log`、`02-up.log`），截圖依順序編號（`s01-*.png`）。
- 證據表最後一定要有「沒做的」：列出沒驗到的項目與原因。
- 機敏資訊：證據裡不放 `.env` 的值、token、金鑰；只寫「有／沒有」。
- 退回最多 2 次，超過就交給決策者。
