# auto-merge 計畫 checklist

計畫全文：全自動 SDLC＋agent 自動 merge PR＋跨平台（Windows）重構。整合分支 `feature/auto-merge`。
決策（2026-10-24 拍板，見 docs/adr-agent-auto-merge.md〈決策〉）：可信任合併 agent 逐條對照需求與驗證結果，一致才合併，不一致就開新需求並退回 PR；上游取得授權後開放；fork approvals 0；新增確定性推送工具。模版只管工作流程：開發規範（TDD／DDD／SDD 等）、code style、commit 與文件格式等慣例由使用者定義，init 時先確認使用者習慣；需求與驗證來源由套用團隊設定。

規則：每一項都由驗證指令判定，**不能手動打勾**。更新方式：

```bash
python3 scaffold/.claude/teams/tools/checklist.py check plans/auto-merge/checklist.md --write   # 跑全部並改寫勾選框
python3 scaffold/.claude/teams/tools/checklist.py check plans/auto-merge/checklist.md --id SP1 # 只驗某個子計畫
python3 scaffold/.claude/teams/tools/checklist.py status plans/auto-merge/checklist.md          # 列出未完成
```

子計畫完成的定義：該子計畫的所有項目 `check --id SPn` 全部通過。整份完成的定義：`check` 全部通過。

## SP3 checklist 機制
- [x] SP3.1 checklist.py 能依指令結果打勾、拒絕沒有驗證指令的項目 — 驗證：`python3 scaffold/.claude/teams/tools/checklist.py --selftest`
- [x] SP3.2 本檔每一項都有驗證指令、SP0–SP7 都有項目 — 驗證：`python3 plans/auto-merge/verify.py SP3.2`

## SP0 Agent Teams 辯論
- [x] SP0.1 使用者已開 Agent Teams、_tpm/ 已排除在版控外 — 驗證：`python3 plans/auto-merge/verify.py SP0.1`
- [x] SP0.2 4 位 teammate（A 職責分離、B 事後防線、C 人工基準線、D 完整性稽核）的 option-n.json 都是合格信封 — 驗證：`python3 plans/auto-merge/verify.py SP0.2`
- [x] SP0.3 comparison.md 與 docs/adr-agent-auto-merge.md 已產出且通過 check_rules --audience owner — 驗證：`python3 plans/auto-merge/verify.py SP0.3`
- [x] SP0.4 使用者已拍板並以 save_run.py approve 記錄 — 驗證：`python3 plans/auto-merge/verify.py SP0.4`

## SP1 跨平台（移除 .sh）
- [x] SP1.1 Windows hook／launcher 查證結論與出處寫進踩坑手冊 — 驗證：`python3 plans/auto-merge/verify.py SP1.1`
- [x] SP1.2 repo 沒有任何 .sh；init.sh 改成 init.py — 驗證：`python3 plans/auto-merge/verify.py SP1.2`
- [x] SP1.3 hooks.json 與 project-hooks 範本不呼叫 bash，matcher 含 PowerShell — 驗證：`python3 plans/auto-merge/verify.py SP1.3`
- [x] SP1.4 PowerShell 工具送出的危險指令同樣被擋 — 驗證：`python3 plans/auto-merge/verify.py SP1.4`
- [x] SP1.5 本 repo CI 在 ubuntu／macos／windows（pwsh）跑 run_all.py — 驗證：`python3 plans/auto-merge/verify.py SP1.5`
- [x] SP1.6 全部測試（scrub、init 煙霧、hook、golden-set、build、workflows、agents）通過 — 驗證：`python3 tests/run_all.py`

## SP2 治理修補
- [x] SP2.1 save_run.py approve 強制「該段 done 且前一段已核准」 — 驗證：`python3 plans/auto-merge/verify.py SP2.1`
- [x] SP2.2 主 session 的 merge／approve／gh api merge 漏洞已封住，開 PR 到 fork 仍可用 — 驗證：`python3 plans/auto-merge/verify.py SP2.2`
- [x] SP2.4 政策核准讀實際驗證內容（verifier 有 fail 時 --by policy 不能核准） — 驗證：`python3 plans/auto-merge/verify.py SP2.4`
- [x] SP2.5 build 中途失敗記為 halted，不會被當成 done — 驗證：`python3 plans/auto-merge/verify.py SP2.5`
- [x] SP2.3 dev-pr-reviewer 信心改為 0–1 — 驗證：`python3 plans/auto-merge/verify.py SP2.3`

## SP4 確定性 merge gate
- [x] SP4.1 merge_gate.py 的 selftest（SHA 不符、測試數減少、新增 skip、改 CI 檔、超過每日上限都判 hold／escalate）通過 — 驗證：`python3 plans/auto-merge/verify.py SP4.1`
- [x] SP4.3 merge_gate 機械複核需求對照表（需求來源可插拔、不綁開發規範；每條需求都要對到 pass 證據，任何 mismatch／缺漏判 hold） — 驗證：`python3 plans/auto-merge/verify.py SP4.3`
- [x] SP4.2 merge-decision payload、golden-set 正反例、build.py --check 一致 — 驗證：`python3 plans/auto-merge/verify.py SP4.2`

## SP5 新角色與 workflow
- [x] SP5.1 dev-merge-steward（無 Edit／Write）、dev-release-watcher 通過 check_agents（16 個角色） — 驗證：`python3 plans/auto-merge/verify.py SP5.1`
- [x] SP5.3 dev-merge-steward 對照需求與成果，不一致時產出新需求並退回 PR（request-changes），sdlc-merge.js 把不一致轉成 followup — 驗證：`python3 plans/auto-merge/verify.py SP5.3`
- [x] SP5.4 確定性推送工具 publish.py 的 selftest 通過 — 驗證：`python3 plans/auto-merge/verify.py SP5.4`
- [x] SP5.2 sdlc-merge.js、event-post-merge.js 存在且列入 tpm-team 事件表 — 驗證：`python3 plans/auto-merge/verify.py SP5.2`

## SP6 hook 開放條件、CI 強制層、規則
- [x] SP6.1 啟用 auto_merge 後：只有 steward 能 merge、steward 也不能 approve、上游無授權一律擋（Bash 與 PowerShell） — 驗證：`python3 plans/auto-merge/verify.py SP6.1`
- [x] SP6.4 steward 合併須有綁定 head SHA 的 gate 紀錄與 --match-head-commit、冒充擋、上游需授權人（Bash 與 PowerShell） — 驗證：`python3 plans/auto-merge/verify.py SP6.4`
- [x] SP6.2 目標專案 CI 範本把 merge_gate.py 當 required check — 驗證：`python3 plans/auto-merge/verify.py SP6.2`
- [x] SP6.3 rules.md 改為風險分級、信任模型文件與 guard.config 範本欄位齊全 — 驗證：`python3 plans/auto-merge/verify.py SP6.3`

## SP8 SDLC 各段可自訂開發規範與慣例（模版只管工作流程；前置：SP2、SP4、SP5 合併後才開工，因為會改同一批 workflow 與 agent）
- [x] SP8.1 開發規範 profile：schema、default.json（重現現行做法）與至少一個不同的範例 profile，golden-set 正反例通過 — 驗證：`python3 plans/auto-merge/verify.py SP8.1`
- [x] SP8.2 sdlc-*.js 不寫死開發規範（紅燈先行、EARS 等改由 profile 決定） — 驗證：`python3 plans/auto-merge/verify.py SP8.2`
- [x] SP8.3 dev-* agent 照派工單帶來的 profile 工作，不把特定規範寫成硬性要求 — 驗證：`python3 plans/auto-merge/verify.py SP8.3`
- [x] SP8.4 merge_gate 的需求來源與 profile 共用同一份設定 — 驗證：`python3 plans/auto-merge/verify.py SP8.4`
- [x] SP8.5 METHODOLOGY 佔位符、自訂與擴充文件、非預設 profile 的 init 煙霧測試 — 驗證：`python3 plans/auto-merge/verify.py SP8.5`
- [x] SP8.6 慣例（code style、commit 格式、分支命名、PR 大小、文件格式）寫進 profile，agent／workflow／rules 不再寫死 — 驗證：`python3 plans/auto-merge/verify.py SP8.6`
- [x] SP8.7 init 先確認使用者習慣（互動詢問或 --answers 答案檔），寫進專案 profile；init-workflow skill 先問再寫 — 驗證：`python3 plans/auto-merge/verify.py SP8.7`

## SP9 不影響使用者既有的 agent、skill 與設定
- [x] SP9.1 使用者自己的 agent（沒有 plugin 命名空間）即使叫 dev-*／tpm-* 也不套模版規則；plugin 角色照舊 — 驗證：`python3 plans/auto-merge/verify.py SP9.1`
- [x] SP9.2 主 session（使用者 skill）的 git／GitHub 限制可由設定放寬，啟用 auto_merge 時主 session 仍不能合併 — 驗證：`python3 plans/auto-merge/verify.py SP9.2`
- [x] SP9.3 套用到既有專案：原檔保留、CLAUDE.md 只加引用、hooks 合併進既有 settings、預設不放 merge gate CI — 驗證：`python3 plans/auto-merge/verify.py SP9.3`
- [x] SP9.4 煙霧測試與文件涵蓋既有專案相容性 — 驗證：`python3 plans/auto-merge/verify.py SP9.4`

## SP7 整合驗收
- [x] SP7.1 整合分支上全部測試通過 — 驗證：`python3 plans/auto-merge/verify.py SP7.1`
- [x] SP7.2 CLAUDE.md 已更新（不再提 .sh、列出新角色） — 驗證：`python3 plans/auto-merge/verify.py SP7.2`
- [x] SP7.3 端到端演練：低風險 PR 自動合併、改 CI 檔的 PR 被 escalate、revert 演練成功 — 驗證：`python3 plans/auto-merge/verify.py SP7.3`
