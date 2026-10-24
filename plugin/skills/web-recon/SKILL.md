---
name: web-recon
description: 被動式網站技術棧偵察 — 判定 CDN／WAF／邊緣運算層、來源伺服器指紋、HTTP 協定版本、TLS 設定、框架與前端建構工具、API 形態（REST/GraphQL/tRPC）、第三方 SaaS 依賴。當任務要「分析某網站的技術架構」「這網站用什麼技術」「技術棧盤點」「架構拆解」時使用。
---

# Web Recon — 技術棧偵察

## 邊界（先讀）
只做**被動觀測**：讀取公開回應標頭、公開 DNS 記錄、公開頁面與其自行載入的資源。

**禁止**：port scan、目錄爆破、漏洞掃描、壓力測試、任何帳號嘗試、繞過 rate limit。
發現疑似漏洞時只記錄「暴露面觀察」，不驗證、不利用。全程遵守 robots.txt 與合理請求速率（每主機併發 ≤ 4，間隔 ≥ 200ms）。

## 標準流程

### 1. DNS 與網路邊界
```bash
dig +noall +answer A     example.com
dig +noall +answer AAAA  example.com
dig +noall +answer NS    example.com
dig +noall +answer MX    example.com
dig +noall +answer TXT   example.com     # SPF/DMARC/驗證記錄 → 洩漏所用 SaaS
dig +noall +answer CNAME www.example.com # CNAME 指向 → CDN 供應商
dig +noall +answer CAA   example.com     # 憑證授權 → 憑證供應鏈
```
判讀重點：
- CNAME 尾綴 → CDN（`.cloudflare.net` / `.akamaiedge.net` / `.fastly.net` / `.cloudfront.net` / `.edgekey.net`）
- TXT 記錄的 `-site-verification` 前綴 → 正在用哪些 SaaS（Google、Atlassian、Stripe、Zoom…）
- MX → 郵件基礎設施（Google Workspace / M365 / 自建）
- NS 是否自營 → 是否有自家 DNS 團隊

### 2. HTTP 層指紋
```bash
curl -sSIL --max-time 20 https://example.com | sed -n '1,200p'
curl -sSI --http2 https://example.com | head -1      # 是否支援 h2
curl -sSI --http3 https://example.com | head -1      # 是否支援 h3/QUIC（curl 需 h3 支援）
curl -sS -o /dev/null -w 'dns:%{time_namelookup} tcp:%{time_connect} tls:%{time_appconnect} ttfb:%{time_starttransfer} total:%{time_total} ip:%{remote_ip} proto:%{http_version}\n' https://example.com
```
標頭判讀對照：
| 標頭 | 推論 |
|---|---|
| `server:` | 來源／邊緣（nginx, cloudflare, AmazonS3, envoy, LiteSpeed…） |
| `cf-ray`, `cf-cache-status` | Cloudflare，`-ray` 後綴為 PoP 機場代碼 |
| `x-amz-cf-id`, `via: CloudFront` | AWS CloudFront |
| `x-served-by`, `x-cache` | Fastly / Varnish（`x-served-by` 含 PoP 代碼） |
| `x-vercel-id`, `x-nextjs-*` | Vercel + Next.js |
| `x-powered-by` | PHP/Express/ASP.NET 版本 |
| `set-cookie` 名稱 | 框架（`PHPSESSID`, `laravel_session`, `_session_id`(Rails), `JSESSIONID`, `__cf_bm`） |
| `content-security-policy` | 白名單網域 = **完整第三方依賴清單**（最高價值來源） |
| `strict-transport-security`, `permissions-policy` | 安全成熟度 |
| `x-request-id`/`x-trace-id` 格式 | 是否有分散式追蹤（OTel/Datadog） |

### 3. TLS 與憑證供應鏈
```bash
echo | openssl s_client -connect example.com:443 -servername example.com 2>/dev/null \
  | openssl x509 -noout -issuer -subject -dates -ext subjectAltName
```
SAN 清單常直接洩漏內部子網域與併購來的品牌。憑證簽發者（Let's Encrypt / DigiCert / Google Trust）反映成本取向。

### 4. 前端框架與建構產物
用 Chrome 工具（`mcp__claude-in-chrome__*`）或 curl 取 HTML：
- `__NEXT_DATA__` / `id="__nuxt"` / `ng-version` / `data-reactroot` / `wp-content` → 框架
- `/_next/static/` `/_nuxt/` `/assets/index-<hash>.js` → 建構工具（Next/Nuxt/Vite/Webpack）
- `<link rel=preconnect>` 清單 → 關鍵路徑上的第三方
- source map（`//# sourceMappingURL`）存在與否 → 建置紀律
- 用 `read_network_requests` 抓實際請求瀑布，分類：自家 API / CDN 靜態資源 / 分析追蹤 / 廣告 / A B 測試 / 客服 widget

### 5. API 形態
從 network 面板辨識：
- `/api/` `/v1/` REST；`/graphql` GraphQL；`/trpc/` tRPC；`.rpc`/protobuf content-type → gRPC-web
- 回應標頭的 `x-ratelimit-*` → 是否有 API gateway
- WebSocket / SSE 連線 → 即時架構

### 6. 多資產盤點
主站以外務必分別偵察（常是不同技術棧）：
`www` / 登入或控制台（`my.`, `app.`, `console.`）/ 文件（`docs.`, `support.`）/ 部落格 / 狀態頁 / API 網域 / 靜態資產網域。
控制台與行銷站通常由**不同團隊、不同棧**負責，這個落差本身就是重要發現。

## 輸出格式
寫入 `artifacts/<target>/tech-stack.md`：
1. **架構分層圖**（mermaid：使用者 → DNS → 邊緣/WAF → 應用 → 資料層）
2. **技術棧表**：層別 | 判定 | 證據（實際標頭／URL） | 信心（高/中/低）
3. **第三方依賴清單**（來自 CSP + 網路請求），標註用途與資料流向
4. **暴露面觀察**（僅記錄，不驗證）
5. **無法判定的項目**——明說是黑箱，不臆測

## 鐵律
每一條技術判定都要附**可覆核的原始證據**（實際標頭字串、URL、DNS 記錄）。
沒有證據的推論一律標記為「推測」，不得寫成事實。
