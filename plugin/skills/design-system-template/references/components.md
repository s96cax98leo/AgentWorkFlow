# 元件對照：DS 規格 → 產品實作（骨架）

- DS 行號：前面是示範 HTML，括號內是 CSS。產品路徑相對【填空：前端 src 目錄】（註明調查時的分支與 commit）。
- 數值全部出自 DS；DS 沒寫的標「文件未定義」。顏色一律用 token（`references/tokens.css`）；DS 元件 CSS 裡寫死的色值照抄 DS 值並加進 `allowlist.txt`，不另調。
- 框架 utility 名稱（例如 `rounded-card`、`shadow-card`）來自 SKILL.md「導入步驟 1」的 theme 映射。

每個元件一節，固定三欄：

| DS class | 規格（數值＋DS 行號） | 產品元件寫法 |
|---|---|---|

## 0. 共通
| 項目 | 規格 | 出處 |
|---|---|---|
| pill 圓角 | 【填空】 | DS:【行】 |
| 卡片圓角 | 【填空】 | DS:【行】 |
| 輸入框圓角 | 【填空】 | DS:【行】 |
| 互動動態 | 【時長＋曲線；只動 transform／opacity／顏色】 | DS:【行】 |
| 焦點樣式 | 【例如 outline 2px 主色、offset 2px】 | DS:【行】 |
| 圖示 | 【圖示集、筆畫、預設尺寸】 | DS:【行】 |

## 1. 按鈕 → 【產品元件路徑】
列出每個變體（primary、secondary、outline、danger…）與尺寸，以及按壓、停用、hover 狀態。
**用法規則**：【例如每畫面最多一個 solid primary；破壞性操作在列表用 outline，確認對話框才用 solid】。

## 2. 徽章、Chip → 【路徑】
每個 tone 用哪一階色見 `status-colors.md` §2。

## 3. 頁首（Eyebrow／H1／Lede） → 【路徑】
## 4. 卡片、統計卡、空狀態、告示 → 【路徑】
## 5. 分段控制／Tabs → 【路徑】
## 6. 頂列 → 【路徑】
## 7. 側欄 → 【路徑】
## 8. 表格 → 【路徑】
## 9. 表單（input、select、textarea） → 【路徑】
## 10. 事件列表／Timeline → 【路徑】
## 11. Footer 告示 → 【路徑】
## 12. 字級（H1–H3、內文、說明、mono）
## 13. 進場動效與背景
## 14. DS 有、產品目前沒對應功能的（不做，除非之後有需求）
