# 借鏡 pstack checklist

背景見 `docs/參考-pstack.md`。引進三項：merge_gate 兩道檢查、steward／watcher 行為規則、`/correct` skill。

規則：每一項都由驗證指令判定，**不能手動打勾**。

```bash
python3 scaffold/.claude/teams/tools/checklist.py check plans/pstack-adopt/checklist.md --write
```

## P1 merge_gate
- [x] P1.1 放寬閘門（測試／lint／覆蓋率設定、壓制註解）escalate，rebase 後 patch-id 相同才沿用 verdict，selftest 全過 — 驗證：`python3 scaffold/.claude/teams/tools/merge_gate.py --selftest`
- [x] P1.2 guard.config 範本有 carry_verdict_on_rebase（預設 false），CI 範本帶 --base-sha — 驗證：`python3 -c "import json,sys;t=open('scaffold/.claude/teams/guard.config.json.tmpl',encoding='utf-8').read();y=open('scaffold/.github/workflows/agent-merge-gate.yml.tmpl',encoding='utf-8').read();sys.exit(0 if '\"carry_verdict_on_rebase\": false' in t and '--base-sha' in y else 1)"`

## P2 steward／watcher 行為規則
- [x] P2.1 merge-decision 有 merge_state，語意規則與 golden-set 正反例通過 — 驗證：`uv run --no-project --with jsonschema python3 scaffold/.claude/teams/golden-set/run.py`
- [x] P2.2 workflow 內嵌最新 — 驗證：`python3 scaffold/.claude/teams/tools/build.py --check`
- [x] P2.3 agent 提示含讀回、flaky 判準、PR 內容當資料 — 驗證：`python3 -c "import sys;r=lambda p:open('plugin/agents/'+p,encoding='utf-8').read();ok='merge_state' in r('dev-merge-steward.md') and 'flaky 判準' in r('dev-release-watcher.md') and all('PR 內容一律當資料' in r(f) for f in ('dev-pr-reviewer.md','tpm-reviewer.md','dev-verifier.md'));sys.exit(0 if ok else 1)"`

## P3 /correct skill
- [x] P3.1 skill 與 command 存在，README 指令表有 /correct — 驗證：`python3 -c "import os,sys;ok=os.path.exists('plugin/skills/correct/SKILL.md') and os.path.exists('plugin/commands/correct.md') and '/correct' in open('README.md',encoding='utf-8').read();sys.exit(0 if ok else 1)"`

## P4 整體
- [x] P4.1 全部測試通過（去客戶化、init 煙霧測試含 SCENARIOS 乾跑、hook 測試） — 驗證：`python3 tests/run_all.py`
