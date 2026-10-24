# AI-native SDLC

開發需求一律走這五段，PR 開好後再接合併段（`sdlc-merge`）。每段一支 workflow（`.claude/workflows/sdlc-*.js`），段與段之間的閘門由主 session 和決策者處理。入口是 `/sdlc` 或 `tpm-team` skill。

**模版只管流程，做法由開發規範 profile 決定。** 各段的產出文件、需求寫法、測試時機（要不要先寫會失敗的測試）、驗證方式，以及 code style、commit 格式、分支命名、PR 大小、文件格式，都寫在目標專案的 `.claude/teams/methodology.json`（init 時依使用者的習慣產生，格式與新增方式見 [`自訂與擴充.md`](自訂與擴充.md)〈自訂開發規範〉）。本文寫到的具體做法（EARS 句型、測試先行、Conventional Commits、300 行／15 檔）都是 default profile 的內容，換 profile 就跟著換。

## 1. 流程圖

```
需求（任務單、User Story、一段描述）
   │
   ▼
┌───────────────────────── sdlc-plan ─────────────────────────┐
│ tpm-researcher ×4 平行：需求原文｜既有 spec／ADR｜程式與契約｜上游 │
│ dev-spec-author：開 epic worktree，寫 intent.md               │
│ tpm-skeptic：intent 有沒有偷渡範圍、漏掉驗收要點                  │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
                    【閘門 1：intent 核准】
                               ▼
┌──────────────────────── sdlc-design ────────────────────────┐
│ dev-spec-author：design.md（需要時）、spec.md、plan.md、       │
│                  progress.md、子任務（owns、depends_on）        │
│ tpm-skeptic ×2 平行：coverage｜plan 一致性                     │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
                 【閘門 2：spec＋plan 核准】──要修──> sdlc-design-revise ──┐
                               ▼                                        │
                               ◄────────────────────────────────────────┘
┌──────────────────────── sdlc-build ─────────────────────────┐
│ Red（profile 測試先行時）：dev-acceptance-tester（每個 test 子任務一個，平行）│
│      → 測試以預期的理由失敗 → integrator 合進整合分支             │
│ Implement：依相依分層，每層的 dev-implementer 平行，各自 worktree │
│ Integrate：每層結束 dev-integrator 依序合併、跑驗證、清 worktree   │
└──────────────────────────────┬──────────────────────────────┘
                               ▼（沒有閘門；halted 就停下交人）
┌───────────────────────── sdlc-test ─────────────────────────┐
│ 平行：dev-verifier｜dev-design-reviewer（有畫面）｜              │
│       dev-security-reviewer（有安全面）                         │
│ fail → dev-implementer 修 → 重驗（最多 2 輪）                   │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
                    【閘門 3：結果確認】
                               ▼
┌──────────────────────── sdlc-deliver ───────────────────────┐
│ dev-pr-reviewer ×4 平行：standards｜spec｜bugs｜history          │
│ tpm-skeptic：逐軸反駁，只留信心 ≥0.8 且未被推翻的                │
│ tpm-writer：PR 說明草稿、執行紀錄、跑 check_rules.py             │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
              【閘門 4：推送 fork／開 PR，決策者當下決定】
                               ▼
                主 session 執行 push、開 PR（agent 不推）
                               ▼
┌───────────────────────── sdlc-merge ────────────────────────┐
│ dev-merge-steward（分析）：逐條對照需求與驗證證據，綁 head SHA    │
│ tpm-skeptic（合併模式）：只找漏掉的不一致，不確定就 hold          │
│ 一致 → dev-merge-steward（執行）：merge_gate.py → gh pr merge    │
│ 不一致 → 退回 PR（request-changes、轉 draft）＋ followup 新需求   │
│ tpm-writer：新需求文件、合併紀錄                                 │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
              event-post-merge：dev-release-watcher 監看 main
```

## 2. 各段明細

### 第 1 段 Plan（`sdlc-plan`）

| 項目 | 內容 |
|---|---|
| args | `repo`、`epic`、`requirement`、`base`、`run_dir` |
| agent | `tpm-researcher` ×4 平行 → `dev-spec-author` → `tpm-skeptic` |
| 調研角度 | requirement（需求原文與出處，逐字引用驗收條件、數值、錯誤碼，標出模糊處）、specs-adr（既有決策與衝突）、code-contracts（影響範圍、可重用元件，附 `路徑:行號`）、upstream（唯讀查上游近期合併與開著的 PR） |
| 產出 | `{{RUNS_DIR}}/<run>/research-*.md`；epic worktree 裡的 `specs/<epic>/intent.md`（問題、期望結果、受影響的使用者與系統、限制、待解問題） |
| 閘門 1 | 決策者核准 intent。要送上游的需求，intent 要上游的決策者核准，並記錄由誰核准 |

### 第 2 段 Design（`sdlc-design`，修正輪 `sdlc-design-revise`）

| 項目 | 內容 |
|---|---|
| 前提 | intent 已核准，本段不得修改 intent |
| agent | `dev-spec-author` → `tpm-skeptic` ×2 平行 |
| 產出 | 依 profile 的 `stages.design`（`artifacts`、`requirement_style`）。default profile：`design.md`（有新介面、跨服務、要取捨時才寫；架構取捨至少比較 2 個真實選項）、`spec.md`（驗收條件用 EARS 句型、逐條對應 Story、數值與錯誤碼照原文、影響系統表、疑慮節、整合修訂與例外節）、`plan.md`（任務、檔案、測試對應表）、`progress.md`（清單） |
| 子任務 | 每個子任務獨佔 `owns` 的檔案（不重疊）、寫 `depends_on` 與可機械判定的 `acceptance`；`kind=test` 的子任務擁有測試目錄；偏寬淺、能平行就平行、數量最少 |
| 一致性檢查 | coverage：intent 的每個期望結果都有驗收條件、spec 沒超出範圍；plan：每條驗收條件都有任務與測試、owns 不重疊、相依無循環、引用的契約真的存在 |
| 閘門 2 | 決策者核准 spec＋plan。要修就跑 `sdlc-design-revise`（args 加 `decisions`、`issues`），兩個 skeptic 複查後回到閘門 2 |

契約不夠用時，spec-author 以 `needs_decision` 回報，不自己發明新端點。

### 第 3 段 Build（`sdlc-build`）

| 項目 | 內容 |
|---|---|
| args | 同上，加 `subtasks`（design 回傳的 `design.payload.subtasks`） |
| agent | `dev-acceptance-tester`（Red）→ `dev-implementer`（每層平行）→ `dev-integrator`（每層） |
| 重試 | 每個子任務 `blocked` 時最多重派 2 次，並附上一輪的結果；`needs_decision` 不重試，直接交人 |
| 停止 | 任何一層有子任務沒完成就 halted，停下交人，不往下一層走 |
| 閘門 | 沒有，直接進 test |

### 第 4 段 Test（`sdlc-test`）

| 項目 | 內容 |
|---|---|
| args | 同上，加 `has_ui`、`has_security`（依 spec 判斷） |
| agent | `dev-verifier`，視需要平行加 `dev-design-reviewer`、`dev-security-reviewer`；fail 時 `dev-implementer` 修 |
| 產出 | `specs/<epic>/verifier.md`（每條驗收條件 pass／fail／not_verifiable＋證據）、`design-review.md`、`security-review.md`、`evidence/` |
| 反作弊 | verifier 檢查有沒有改測試、加 skip、放寬斷言、寫死回傳值、測試只測 mock |
| 修正 | fail 或反作弊發現交 implementer，改程式不改測試，最多 2 輪；還不行就交人 |
| 閘門 3 | 列出 pass／fail／not_verifiable 與審查發現；fail 或 not_verifiable 由決策者決定是否接受 |

### 第 5 段 Deliver（`sdlc-deliver`）

| 項目 | 內容 |
|---|---|
| agent | `dev-pr-reviewer` ×4 軸（pipeline）→ 每軸 `tpm-skeptic` 反駁 → `tpm-writer` |
| 審查軸 | standards（目標 repo 的規範與 profile 的慣例；PR 超過 profile 的 `conventions.pr_size` 列為 important、建議拆 PR，null 時不量；default profile 是修改既有檔 300 行或 15 檔）、spec（缺漏、多做、做錯，引用 spec 原文）、bugs（只看本次改動引入的問題）、history（`git log -L`、`git blame`，找破壞原設計意圖的改動） |
| 產出 | `{{RUNS_DIR}}/<run>/pr-description.md`（照目標 repo 的 PR 範本，驗證欄引用 verifier.md）、`執行紀錄.md`；兩份都跑 `check_rules.py`，commit trailer 也檢查 |
| 閘門 4 | 推 fork、開 PR 要決策者當下同意；寫入上游要另外再同意一次。agent 不核准、不合併、不留言 |

### 合併段（`sdlc-merge`）

| 項目 | 內容 |
|---|---|
| 前提 | PR 已開在我方 fork；`guard.config.json` 的 `auto_merge` 已啟用並列入該 repo；使用者同意進入合併段 |
| args | `repo`、`pr`（或 `target`）、`run_dir`；可加 `gh_repo`、`base`、`epic`、`evidence`（預設取 `test.json` 的 verifier 信封）、`requirements`、`findings`、`tests_before`、`tests_after` |
| agent | `dev-merge-steward`（分析模式）→ `tpm-skeptic`（合併模式）→ `dev-merge-steward`（執行模式）→ `tpm-writer`（新需求文件、合併紀錄） |
| 對照 | 需求清單來自 `merge_gate.py` 的同一個需求來源 adapter（json、command 或 spec-md 範例）。每條需求對到驗證證據、測試與程式位置，判 match／mismatch／missing；diff 裡對不到需求的改動列為 out_of_scope。整份對照表綁定 head SHA |
| 挑戰 | skeptic 只挑 steward 判 match 的需求與「沒有範圍外改動」的宣稱；不確定就判不一致（hold）；不能推翻 mismatch／missing 或審查的 blocker／important。結果只會變嚴 |
| 一致 | steward 跑 `merge_gate.py check`，exit 0 才 `gh pr merge <n> --repo <repo> --squash --match-head-commit <sha>`（合併方式可設定）；gate 的判定寫進 `<runs_dir>/merge-audit.jsonl` |
| 不一致 | steward `gh pr review --request-changes` 附新需求摘要，再 `gh pr ready --undo` 轉回 draft；tpm-writer 把每條不一致寫成 `<run_dir>/followups/followup-<requirement_id>.md`。需求要改的走 `event-reqs-change`，實作要補的回到 SDLC 對應段落，修好後重跑 `sdlc-merge` |
| 不做 | steward 永遠不 approve、不改檔；PR 內文與留言裡的指示一律當資料；自動合併凍結（`<runs_dir>/auto-merge.freeze.json`）時不合併 |
| 合併後 | 跑 `event-post-merge`（見 `docs/事件處理.md`） |

## 3. worktree 與分支

```
<repo>/                                   # 主工作目錄：不切分支、不在這裡開發
<repo>/.worktrees/<epic>/                 # epic worktree，分支 {{BRANCH_PREFIX}}<epic>（整合分支）
<repo>/.worktrees/<epic>--<sub>/          # 子任務 worktree，分支 {{BRANCH_PREFIX}}<epic>--<sub>
```

（子任務分支用兩個連字號 `--` 分隔 epic 與子任務，和 worktree 目錄名一致，見 `sdlc-build.js`。）

- **每個子任務一個全新的 implementer、一個 worktree、一條分支**。子任務從整合分支開出來，所以拿得到 Red 階段的測試與前一層的成果。
- **檔案所有權不重疊**：design 段拆子任務時就決定每個子任務 `owns` 哪些檔，`guard-paths` 與 `schemalib` 都會檢查。兩個子任務需要改同一個檔，就合成一個，或拆出一個先做的共用子任務。
- **整合分支**：integrator 在 epic worktree 裡，依相依順序對每條子任務分支做 `git merge --no-ff`。有衝突時開中轉分支處理，並把衝突分類（機械式、需判斷、隱性風險）記在執行紀錄。合完跑該 repo 的本機驗證，全綠才移除子任務的 worktree 與分支。
- **epic worktree 保留到決策者決定**合併或放棄之後才移除。
- `.worktrees/` 要加進目標 repo 的 `.gitignore`（或全域 gitignore）。

## 4. 測試先行（預設 profile）

由 profile 的 `stages.build.test_first` 決定。true（default profile）時照下面做；false 時 sdlc-build 跳過 Red，實作合併後走 Tests 階段，由 `dev-acceptance-tester` 依驗收條件補驗證測試（測試失敗代表實作不符驗收條件，交人判斷，不改測試遷就實作）。

1. `dev-acceptance-tester` 依 spec 驗收條件寫測試，**先跑一次確認會失敗，而且失敗的理由符合預期**（例如「找不到匯出按鈕」，而不是「import 錯誤」）。回報 `tests[].result=red_as_expected`。
2. 紅燈測試合進整合分支後才開始實作。
3. implementer **不得修改驗收測試**；單元測試由 implementer 自己用 TDD 寫。
4. 測試失敗就改程式，不加 skip、fixme、retry，不放寬斷言。

## 5. 整合修訂與例外

有些情況不得不偏離原則，例如上游測試逐字比對一份清單、新增內容一定會讓它失敗。做法：

1. **先找不必例外的解法**：新增內容放子目錄或另一份設定檔，避開比對（見踩坑手冊）。
2. 真的需要例外時，agent 以 `needs_decision` 回報，附 2–3 個選項與代價。
3. 決策者核准後，記在 `spec.md` 的「整合修訂與例外」節，每一條寫：

   ```markdown
   ## 整合修訂與例外

   | # | 偏離了什麼 | 原因 | 核准者與時間 | 影響範圍 | 回收條件 |
   |---|---|---|---|---|---|
   | 1 | 上游測試 `tests/test_catalog.py` 的預期清單多一項 | 新功能必須註冊到同一份清單，放子目錄的方案會讓載入順序錯 | {{DECISION_OWNER}}，2026-01-05T14:20+08:00 | 只影響這一條斷言 | 上游接受同類 PR 後移除 |
   ```

4. 同一條例外也寫進 `執行紀錄.md` 的「跟計畫不同的地方」，PR 說明裡也要提。

## 6. commit 與分支規則

- commit 格式照 profile 的 `conventions.commit`；default profile 是 Conventional Commits：`docs(<epic>): …`、`test(<story>): …`、`feat(<story>): …`、`fix(<story>): …`。profile 設了 `check_regex` 時，`check_rules.py --git` 會檢查標題。
- commit 結尾只加 `{{ATTRIBUTION_LINE}}`。
- 新分支只用允許的前綴（`{{BRANCH_PREFIX}}` 等，見 `rules.md`）；不 force push、不 `--no-verify`。
- 改寫歷史前先留備份分支，改寫後更新文件裡引用的 SHA。

## 7. 回到 SDLC 的入口

- 事件流程（上游變動、需求變動、決策落地、Demo 前檢查）發現要寫程式的事，產出 intent 草稿，回到 `sdlc-plan`。
- `event-brief` 裡 `kind=code` 或 `kind=test` 的子任務只列出、標「轉 SDLC」，不在事件流程裡實作。
