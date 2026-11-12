---
name: deliverables
description: 設定與盤點交付文件：從情境組合、repo 實況或合約清單挑出這個專案要交哪些文件，確認後才寫入；之後看每份文件齊不齊、哪些小節過期、產生需求追溯矩陣。使用者說「要交哪些文件」「設定交付文件」「結案前盤點文件」「追溯矩陣」「/agent-workflow:deliverables」時使用。
argument-hint: <configure、status、check、matrix 或要做的事>
---

# 交付文件（deliverables）

使用者要做的：$ARGUMENTS

```bash
python3 .claude/teams/tools/deliverables.py configure [--preset <情境>] [--suggest] [--import <清單檔>] [--add a,b] [--remove a,b]   # 提案，不寫入
python3 .claude/teams/tools/deliverables.py configure <同上> --write --by <確認的人>   # 人確認後才寫 .claude/teams/deliverables.json
python3 .claude/teams/tools/deliverables.py init      # 依設定建文件骨架與總目錄
python3 .claude/teams/tools/deliverables.py status    # 每份文件：齊全、過期、缺原料、等人補
python3 .claude/teams/tools/deliverables.py check     # 檔頭、過期小節、禁用字、隱私字串、失效連結、總目錄
python3 .claude/teams/tools/deliverables.py sync --run <run_dir>   # deliver 之後：併交付摘要、寫追溯矩陣、記錄來源
```

1. **設定**：先問使用者是哪種情境（黑客松、概念驗證、內部工具、委外專案、維運移交），或有沒有合約／客戶給的交付清單（一行一項存成文字檔給 `--import`）。加 `--suggest` 讓工具依 repo 實況補建議。把提案（每份文件與理由、對不到型錄的項目、不建議的與理由）列給使用者看，**問過之後**才加 `--write --by <使用者>` 寫入。對不到型錄的項目由使用者決定要不要用 `--custom-unmatched` 新增成自訂文件。
2. 寫入後跑 `init`。
3. **盤點**：跑 `status` 與 `check`，照狀態分組回報。「過期」的小節：來源改了而內文沒動；內容確實不用改時，由使用者決定是否用 `sync --run <run_dir> --reviewed <文件id>#<小節>` 標成已看過。
4. 流程會自己用到交付設定：design 段要求 design.md 留對應的小節，deliver 段只更新受影響的小節並寫交付摘要。碰到新領域（例如第一次出現 migration）只會在閘門提示，要不要加進設定由使用者決定。

說明見 `docs/交付文件.md`。沒有交付設定時，流程不產生也不檢查交付文件。
