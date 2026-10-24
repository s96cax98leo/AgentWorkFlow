---
name: design-system-template
description: 設計系統 skill 的骨架（範本，不是成品）。要把一份設計正本（HTML／Figma 匯出／設計規範文件）落地到產品前端時，複製這個目錄、改名成專案自己的設計系統 skill，再把 {{DESIGN_SYSTEM_NAME}} 與各「填空」換掉。使用者說「建設計系統 skill」「把設計稿變成規則」「做 token 檢查」時使用；填好之後的 skill 才給 dev-implementer、dev-design-reviewer 用。
---

# {{DESIGN_SYSTEM_NAME}} 設計系統（骨架）

> **這是範本。** 使用方式：
> 1. 把整個 `design-system-template/` 複製到專案的 `.claude/skills/<設計系統 skill 名>/`，frontmatter 的 `name` 改成那個名字，`description` 改成「{{DESIGN_SYSTEM_NAME}} 在 <產品／前端專案> 的落地規則…，改畫面、新增元件、design review 時使用」。
> 2. 設計正本放進 `references/`，填 `references/source.conf`，用 `shasum -a 256 references/<正本檔> > references/SHA256SUMS` 記指紋。
> 3. 逐檔把「【填空】」換掉；文件沒寫的一律寫「文件未定義」，不要自己補。
> 4. 在 `.claude/teams/rules.md`〈專案規則〉登記這個 skill 的名字，dev-implementer、dev-design-reviewer 會依它載入。
> 5. 刪掉本段說明。

唯一來源是 `references/<正本檔>`（下稱 **DS**，行號寫成 `DS:行`）。SHA-256 在 `references/SHA256SUMS`；設計方給新版時先比指紋，不同就整份重做 `tokens.css` 與本 skill 的對照。
**文件沒寫的一律標「文件未定義」，不要自己補色、補尺寸。** 本 skill 是否進 git repo：【填空：只放本機／進 repo】。

## 什麼時候用、先讀什麼

| 你要做的事 | 先讀 |
|---|---|
| 改顏色、圓角、陰影、動態、字體 | `references/tokens.css`、下方「導入步驟」 |
| 改或新增元件（按鈕、徽章、卡片、表格、表單、頁首、側欄、頂列…） | `references/components.md` |
| 顯示任何狀態（【填空：產品裡的狀態種類，例如任務、訂單、審核單】） | `references/status-colors.md` |
| 版面、斷點、無障礙、個資遮罩 | `references/rwd-a11y.md` |
| 品牌標誌、favicon、登入頁 | `references/brand-mark.md` |
| 字體檔、授權 | `references/fonts.md` |
| 交件前自查、design review | `scripts/checklist.md`（逐條打勾）、`scripts/check-tokens.py` |

## 設計原則（DS:【填空：行號】，用自己的話）
寫 3–5 條，每條一句原則＋一句「所以不做什麼」。範例格式：
1. **【原則名】**：【做法】；不【反例】。
2. **顏色只表達角色與狀態**：主色＝主操作與選中；語意色全站固定，不拿來裝飾。（常見原則，依 DS 調整）
3. **【互動回饋】**：【例如按壓縮放、hover 位移的規則】。
4. **【動態】**：互動 【填空】ms、進場 【填空】ms，曲線 【填空】；一律尊重 `prefers-reduced-motion`。

## ★ 正本與現況衝突的決定（{{DECISION_OWNER}} 核准，日期【填空】）

正本和產品現況不一致時，每一項都要列出來由決策者拍板，**不要自己選**。拍板前這一節維持空白，並在 open_questions 回報。

| # | 衝突 | 決定 |
|---|---|---|
| 1 | 主色：DS 是【填空 token 與色值】（DS:【行】）；產品現在是【填空】 | 【照 DS 改／保留現況＋理由】 |
| 2 | 外觀：DS 【只有淺色／深淺都有】；產品現在【填空】 | 【填空】 |
| 3 | 品牌標誌：DS 是【填空】；產品現在【填空】 | 【填空】 |
| 4 | 圖示：DS 用【圖示集、筆畫】；產品現在【填空】 | 【填空；建議保留單一 adapter 當切換點】 |
| 5 | 字體：DS 從【CDN／本機】載入；產品執行環境【網路是否可靠】 | 【填空；離線展示建議本機託管並附 LICENSE】 |
| … | 其他 | |

### 研究時另外定下的事
- **token 來源**：`tokens.css` 以 DS:【起–迄】逐字為準；正本 `:root` 以外的別名另列一段並註明。
- **撞名的 token**：產品既有變數與 DS 同名但值不同時，列出兩邊的值與以誰為準。
- **`check-tokens.py` 白名單**＝`tokens.css` 的所有色值＋`references/allowlist.txt`（例如品牌標誌專用色）＋`source.conf` 指定的正本行段。

### 正本自己沒做到的地方（照抄前要知道）
用 `scripts/checklist.md` 檢查正本本身，結果寫在 checklist 的 D 段「已知例外」。照抄到產品時**不要把例外一起抄過去**。常見的：文字對比未達 WCAG AA 4.5:1、窄螢幕元素消失、圖示連結沒有可讀名稱、觸控目標只寫在文字規則沒有 CSS。
修正原則（預設，可由決策者改）：**改用同一色階中較深的一階，不自創色票；每個替換記在採用它的 epic spec。**

## 導入步驟（【填空：前端技術棧，例如 React／Tailwind／元件庫】）

產品路徑：【填空：前端專案路徑】；元件庫設定：【填空】。

### 1. token
1. 把 `references/tokens.css` 整段貼進 primitive 層（取代舊色階；**不要改值**）。
2. 語意層改指到 DS token。對照表：

   | 產品變數 | 改成 | 依據 |
   |---|---|---|
   | `--background`／卡片底 | `var(【DS 表面 token】)` | DS:【行】 |
   | 頁底 | `var(【DS 畫布 token】)` | DS:【行】 |
   | 內文字色 | `var(【填空】)` | DS:【行】 |
   | 次要說明字色 | `var(【填空】)` | DS:【行】 |
   | 主色／主色 hover | `var(【填空】)` | DS:【行】 |
   | 邊框、分隔線 | `var(【填空】)` | DS:【行】 |
   | 圓角（卡片／輸入框／pill） | 【填空】 | DS:【行】 |
   | 陰影 | 【填空】 | DS:【行】 |
   | 動態時長與曲線 | 【填空】 | DS:【行】 |

3. 框架的 theme 映射（例如 Tailwind `@theme inline`）：把上表的語意 token 映射成 utility 名稱，寫在這裡讓元件引用。
4. 外觀模式：依決定 2 處理深色（拿掉或補齊）。
5. `body` 的底色、字色、字級、行高、字距：照 DS:【行】。
6. 動態：撞名 token 依「研究時另外定下的事」處理；全域 reduced-motion 規則保留。

### 2. 字體
照 `references/fonts.md`。

### 3. 元件
細節與數值在 `references/components.md`。每個元件寫：DS class → 規格 → 產品元件的變體寫法；DS 沒有的變體標「文件未定義」，保留現有行為。

### 4. 圖示
全站只從一個 adapter 匯出圖示（例如 `@/components/icons`），預設筆畫在 adapter 設定，不靠 context provider（測試或沒包 provider 的樹會退回預設值）。直接 import 圖示庫的檔案改走 adapter，建議用 lint 規則禁止 adapter 以外 import。不用 emoji 當圖示。

### 5. 品牌標誌
照 `references/brand-mark.md`。所有出口（側欄、登入頁、favicon、錯誤頁、信件）用同一組圖形與色值。

### 6. 驗證
- `python3 scripts/check-tokens.py <前端 src 目錄>`：列出寫死的色碼；**本次變更不得新增違規**。
- `python3 scripts/verify-tokens.py`：tokens.css 與正本一字不差（改 tokens.css 後必跑）。
- `scripts/checklist.md`：交件前逐條打勾，design-reviewer 以它為審查標準。

## DO／DON'T（DS:【行】，規則條文逐條）
**DO**（以下是常見的通用條目，依 DS 原文替換或增刪）
1. 內文對比 ≥ 4.5:1（大字 ≥ 3:1）。
2. 觸控裝置（`pointer: coarse`）可點元件 ≥ 【40】px 高。
3. 圖示按鈕一定要 `aria-label`；選中導覽加 `aria-current="page"`。
4. 狀態同時用顏色＋文字，不單靠顏色。
5. 個資（姓名、電話、證號）一律遮罩，格式見 `references/rwd-a11y.md`。
6. 動效都包在 `prefers-reduced-motion` 的關閉規則裡。

**DON'T**
1. 【填空：例如把最淡的灰用在內文】。
2. 用 emoji 當 UI 圖示，或混用其他圖示集。
3. hover 用 scale 造成版面位移。
4. 在捲動容器上用 `backdrop-filter`。
5. 把語意角色色拿來做裝飾，或讓同一概念在不同頁換色。
6. 【填空：DS 其他明文禁止的事】。

## 交付前
照 `scripts/checklist.md` 逐條打勾（A 段 DS 的檢查表＋B 段衝突決定＋C 段 DON'T 補充），附證據。
