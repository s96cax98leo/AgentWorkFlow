# 執行紀錄目錄（{{RUNS_DIR}}）

每次跑 workflow 或處理一個事件，就在這裡開一個 run 目錄：

```
{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>/
├── run.json          # 事件、目標 repo、各段狀態、閘門核准紀錄（save_run.py 維護）
├── <stage>.json      # 該段 workflow 的完整回傳（每個 agent 的信封）
├── research-*.md     # 調研筆記（tpm-researcher）
├── *.md              # 其他 agent 產出：審查報告、影響清單、紅綠燈清單、PR 說明草稿、執行紀錄
├── followups/        # 合併段對照不一致時產出的新需求（sdlc-merge）
└── ...

{{RUNS_DIR}}/merge-audit.jsonl     # merge_gate.py 每次判定的稽核紀錄（綁 head SHA）
{{RUNS_DIR}}/publish-audit.jsonl   # publish.py 每次推送的稽核紀錄
{{RUNS_DIR}}/auto-merge.freeze.json  # 凍結旗標：存在且 frozen=true 時不跑自動合併
```

- `<事件>`：`sdlc`、`brief`、`upstream`、`meeting`、`pr-review`、`reqs-change`、`decision`、`demo-check`、`merge`、`post-merge`。
- `<短名>`：epic 名或事件短名（小寫、`-` 連接）；不要用任務編號以外的自編編號。

## 常用指令

```bash
python3 .claude/teams/tools/save_run.py init {{RUNS_DIR}}/20260101-sdlc-sample --event sdlc --target <repo> --requirement <描述或路徑>
python3 .claude/teams/tools/save_run.py stage {{RUNS_DIR}}/20260101-sdlc-sample plan < plan-result.json   # 存回傳並驗證信封
python3 .claude/teams/tools/save_run.py approve {{RUNS_DIR}}/20260101-sdlc-sample plan --by "{{DECISION_OWNER}}" --at 2026-01-01T10:00:00+08:00
python3 .claude/teams/tools/save_run.py status {{RUNS_DIR}}/20260101-sdlc-sample
```

## 規則
- Workflow 腳本不能寫檔，由主 session 呼叫 `save_run.py` 落檔。
- tpm-* agent（writer 以外）只能寫這個目錄；dev-* 可以寫這裡與目標 repo。
- 閘門核准一定要有 `approve` 紀錄，且在對話裡跟 {{DECISION_OWNER}} 確認過重點。
- run 目錄可能含內部資訊；要不要提交進版控由專案決定（寫進 rules.md 的專案規則）。
