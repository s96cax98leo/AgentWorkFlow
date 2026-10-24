---
name: perf-audit
description: 前端效能與交付效率量測 — TTFB、Core Web Vitals、資源瀑布、bundle 體積、快取命中率、圖片格式與壓縮、第三方腳本負擔、多地延遲比較。當任務要「量效能」「網站快不快」「載入速度」「Web Vitals」「資源分析」時使用。
---

# Perf Audit — 效能與交付量測

## 邊界
只量測公開頁面，正常瀏覽速率。**不做壓測、不做併發灌流**。單頁重複量測 ≤ 5 次取中位數。

## 量測流程

### 1. 網路層時序（多次取中位數）
```bash
for i in 1 2 3 4 5; do
  curl -sS -o /dev/null -w '%{time_namelookup} %{time_connect} %{time_appconnect} %{time_starttransfer} %{time_total} %{size_download} %{http_version} %{remote_ip}\n' \
    https://example.com
done
```
欄位：DNS / TCP / TLS / TTFB / Total / bytes / proto / IP。
拆解 TTFB = 網路往返 + 邊緣處理 + 來源運算。若 `cf-cache-status: HIT` 而 TTFB 仍高 → 邊緣配置問題；MISS 且高 → 來源運算慢。

### 2. 快取行為
```bash
curl -sSI https://example.com | egrep -i 'cache-control|age|etag|last-modified|cf-cache-status|x-cache|vary'
```
比對首次與重複請求的 `age` 值成長 → 推估邊緣 TTL。
靜態資源是否有 immutable + 長 TTL + 內容雜湊檔名 → 建置紀律指標。

### 3. 瀏覽器層（Chrome 工具）
用 `mcp__claude-in-chrome__navigate` 開頁後，以 `javascript_tool` 取得：
```js
const n = performance.getEntriesByType('navigation')[0];
const paints = performance.getEntriesByType('paint');
const res = performance.getEntriesByType('resource');
const byType = res.reduce((a,r)=>{a[r.initiatorType]=(a[r.initiatorType]||0)+ (r.transferSize||0);return a;},{});
const thirdParty = res.filter(r=>new URL(r.name).host !== location.host)
  .reduce((a,r)=>{const h=new URL(r.name).host;a[h]=(a[h]||0)+(r.transferSize||0);return a;},{});
console.log(JSON.stringify({
  ttfb:n.responseStart-n.requestStart, dom:n.domContentLoadedEventEnd, load:n.loadEventEnd,
  transfer:n.transferSize, paints:paints.map(p=>[p.name,p.startTime]),
  reqCount:res.length, byType, thirdParty
},null,2));
```
LCP / CLS 用 PerformanceObserver：
```js
new PerformanceObserver(l=>l.getEntries().forEach(e=>console.log('LCP',e.startTime,e.element?.tagName))).observe({type:'largest-contentful-paint',buffered:true});
let cls=0;new PerformanceObserver(l=>l.getEntries().forEach(e=>{if(!e.hadRecentInput)cls+=e.value;console.log('CLS',cls)})).observe({type:'layout-shift',buffered:true});
```
也可用 `read_network_requests` 直接取瀑布。

### 4. 第三方負擔（關鍵發現常在這）
按 host 彙總 transferSize 與請求數，分類：分析追蹤 / A B 測試 / 廣告 / 客服 / 標籤管理。
**行銷站的效能問題九成來自第三方腳本，不是自家程式碼。** 明確算出「移除第三方後的理論體積」。

### 5. 圖片與字體
格式（AVIF/WebP/JPEG）、是否響應式 `srcset`、是否 lazy load、字體是否 preload 與 `font-display: swap`。

### 6. 多地延遲
若可用不同出口（VPN/雲端節點），對同一 URL 量 TTFB 並記錄回應的 PoP 代碼，做地區延遲表。無法多地時明說單一觀測點，不外推。

## 輸出格式
寫入 `artifacts/<target>/performance.md`：
1. 量測條件（日期、地點、網路、瀏覽器、次數）——**沒有這段的效能數字沒有意義**
2. 時序表（中位數 + 全距）
3. 資源瀑布摘要：總體積、請求數、依類型與第一/第三方拆分
4. Core Web Vitals 與瓶頸歸因
5. 快取策略評估
6. 具體優化機會（依「影響/成本」排序）

## 鐵律
單次量測不成結論。報告任何數字都要附樣本數與量測點。
把「量到的」與「推論的」分開寫。
