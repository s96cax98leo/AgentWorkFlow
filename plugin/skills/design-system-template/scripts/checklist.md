# {{DESIGN_SYSTEM_NAME}} 設計系統：交付前檢查表（骨架；design-reviewer 逐條打勾）

- A 段是 DS 自己的交付檢查表（DS:【行】，條文逐字）；DS 沒有檢查表時，用下面的預設條目並標「預設」。B 段是 SKILL.md 的衝突決定；C 段是 DON'T 裡 A 段沒涵蓋的；D 段是正本自己的已知例外。
- 每條填：✅ 通過／❌ 不符（附 `檔:行` 或截圖）／N/A（附理由）。不符依嚴重度分 Blocker／Should-fix／Nit。
- 驗收寬度 390／768／1024／1280／1440；外觀模式：【只有淺色／只有深色／兩者都測】。
- D 段的例外：產品不得照抄，也不要因為正本這樣做就判通過。

## A. DS 交付前檢查表

### 外觀
| # | 條文 | 怎麼驗 |
|---|---|---|
| A1 | 只用本文件的 token，沒有自創色票或灰色（預設） | `python3 scripts/check-tokens.py <src>`：本次變更不得新增違規（與 base 比）；新程式 0 違規 |
| A2 | 卡片樣式【填空：底色＋邊界做法＋圓角】 | getComputedStyle 量 `border-radius`、`box-shadow`、`border` |
| A3 | 按鈕／徽章形狀與按壓回饋【填空】 | 量 `border-radius`；`:active` 時的 transform |
| A4 | 圖示全部來自【圖示集】，筆畫【填空】 | `svg` 的 `stroke-width`；只有 adapter 直接 import 圖示庫 |
| A5 | 語意角色色與狀態色對照符合 `references/status-colors.md` | 逐頁對照 |

### RWD
| # | 條文 | 怎麼驗 |
|---|---|---|
| A6 | 各驗收寬度都沒有整頁橫向捲動（預設） | `document.documentElement.scrollWidth === clientWidth` |
| A7 | 寬表格在卡片內捲動、不逐字折行（預設） | 390 寬：外層 `overflow-x: auto`，`td` `white-space: nowrap` |
| A8 | 浮動面板在窄螢幕回到內容流（預設） | <1024 時 `position` 不是 `fixed` |
| A9 | 頁首 actions 可換行（預設） | 容器 `flex-wrap: wrap` |
| A10 | 頂列在各寬度不擠掉主要導覽（預設） | 各寬度頂列子項都在可視範圍內 |

### 無障礙
| # | 條文 | 怎麼驗 |
|---|---|---|
| A11 | 內文對比 ≥ 4.5:1（預設） | axe `color-contrast` 0 違規（大字 ≥3:1） |
| A12 | 觸控目標 ≥【40】px（預設） | `pointer: coarse` 模擬下量高度 |
| A13 | 圖示按鈕、連結有可讀名稱（預設） | axe `button-name`、`link-name` 0 違規 |
| A14 | 有可見的 focus 樣式（預設） | Tab 走過每個互動元件 |
| A15 | reduced-motion 時動效關閉（預設） | DevTools 模擬 `prefers-reduced-motion: reduce` |

### 內容
| # | 條文 | 怎麼驗 |
|---|---|---|
| A16 | 個資遮罩（預設） | 格式見 `references/rwd-a11y.md` §6 |
| A17 | 展示環境標示假資料（預設） | 展示環境有告示，文字與實際資料相符 |
| A18 | ID、時間等用 mono 字體【填空】 | computed `font-family` |
| A19 | 【填空：DS 其他條目】 | |

## B. 衝突決定（{{DECISION_OWNER}} 核准，日期【填空】）
| # | 決定 | 怎麼驗 |
|---|---|---|
| B1 | 【主色決定】 | 主要按鈕底色 computed 值；全站沒有舊主色 |
| B2 | 【外觀模式決定】 | |
| B3 | 【品牌標誌決定】 | 所有出口、390 寬也畫得出來 |
| B4 | 【圖示決定】 | 同 A4 |
| B5 | 【字體決定】 | 斷網時字體正確；有 LICENSE |

## C. DON'T 補充（A 段沒涵蓋的）
| # | 條文 | 怎麼驗 |
|---|---|---|
| C1 | 用 emoji 當 UI 圖示，或混用其他圖示集 | 文案與按鈕沒有 emoji 或符號當圖示 |
| C2 | hover 用 scale 造成版面位移 | hover 時版面不位移 |
| C3 | 在捲動容器上用 `backdrop-filter` | 只出現在 sticky 頂列與固定浮層 |
| C4 | 語意角色色拿來做裝飾，或同一概念跨頁換色 | 同一概念跨頁同色 |
| C5 | 【填空】 | |

## D. 已知例外（用本檢查表檢查正本本身的結果，日期【填空】）
檢查方式：【例如 靜態讀正本＋headless 瀏覽器開正本，量五個寬度的 scrollWidth，跑 axe-core（wcag2a/2aa/21a/21aa），對比用 WCAG 公式另算】。

| 條 | 正本結果 | 內容（附 DS:行） |
|---|---|---|
| 【A?】 | 【例外／部分／通過／N/A】 | |
