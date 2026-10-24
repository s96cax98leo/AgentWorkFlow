---
name: infra-forensics
description: 基礎設施與設備底層資源分析 — ASN／IP 前綴自有與否、資料中心與 PoP 地理分布、anycast 判定、路由路徑、虛擬化與隔離層（KVM/容器/cgroup）、硬體世代與規格推斷、儲存與網路後端、電力與 PUE、資本支出結構。當任務涉及「設備底層資源」「機房」「硬體規格」「伺服器資源」「基礎設施」「產能」時使用。
---

# Infra Forensics — 底層資源與設備分析

## 邊界
被動觀測 + 公開資料（RIR whois、BGP 公開資料、供應商自述、狀態頁、招聘職缺、專利與新聞）。
**禁止**：入侵性掃描、對目標主機做效能壓測、租用後探測他人租戶。若在自有帳號內量測，須事先取得使用者確認並遵守供應商 AUP。

## 分析軸線

### 1. 網路自主性（自營 vs 轉租的分水嶺）
```bash
whois -h whois.cymru.com " -v $(dig +short example.com | head -1)"   # IP → ASN/前綴/國別
whois AS<number> | sed -n '1,60p'                                     # ASN 持有者
whois <ip> | egrep -i 'netname|orgname|country|descr|route'
traceroute -q1 -w2 -m20 example.com                                   # 路徑與 PoP 落點
```
判讀：
- **自有 ASN + 自有 PIR/PA 前綴** → 有網路工程能力、可做 BGP anycast、對 transit 有議價權
- **IP 屬於 AWS/GCP/Azure/Hetzner/OVH** → 轉租雲，成本結構受制於人
- **混合**（行銷站在 Cloudflare、產品在自有 ASN）→ 分層策略
- ASN 的 peer 數量與 IX 出現位置 → 傳輸成本與延遲能力

### 2. 地理分布與 anycast
- 從多地量測同一 hostname 解析到的 IP：相同 IP 但延遲低 → anycast；不同 IP → GeoDNS
- CDN 標頭中的 PoP 代碼（`cf-ray` 尾綴、`x-served-by`）即為實際服務節點
- 對照供應商官網的資料中心清單，逐點記錄：城市、供應商（自建 / 託管 colo / 雲）、上線年份
- 產出**地理覆蓋表**：地區 | 節點數 | 自建或託管 | 對應延遲

### 3. 虛擬化與資源隔離層（最關鍵的一層）
若能取得該服務的實機（自有帳號、試用、或公開評測資料），檢查：
```bash
systemd-detect-virt; lscpu | egrep 'Model name|MHz|Socket|Core|Thread|Hypervisor'
cat /sys/devices/virtual/dmi/id/product_name 2>/dev/null
cat /proc/cpuinfo | grep -m1 'model name'
free -h; nproc; cat /sys/fs/cgroup/cpu.max /sys/fs/cgroup/memory.max 2>/dev/null
lsblk -o NAME,ROTA,SIZE,MODEL; cat /sys/block/*/queue/rotational
```
判讀：
- `product_name` = `KVM`/`OpenStack Nova`/`Standard PC (Q35)` → KVM；`LXC`/cgroup 限制 → 容器化共享主機
- `cpu.max` 有配額 → **CPU 是 burst 配額而非獨佔核**，這是共享主機超賣的核心機制
- `memory.max` vs 廣告 RAM 落差 → 是否含 swap／是否可 burst
- CPU model name（如 EPYC 7003/9004、Xeon Gold 6xxx）→ **硬體世代與每核成本**
- `rotational=0` + NVMe 型號 → 儲存等級

無法取得實機時，改用公開評測、供應商規格頁、社群 benchmark，並明確標註為二手資料。

### 4. 超賣比與單位經濟（推估模型）
以「單機容納帳號數」反推：
```
單機月成本 ≈ 硬體攤提(3-5年) + 機櫃電力 + 頻寬 + 授權(cPanel/OS) + 人力攤提
單機月收入 ≈ 每帳號 ARPU × 每機帳號數 × (1 - 退款率)
超賣比    ≈ Σ(承諾資源) / 實體資源
```
共享主機的商業模式**本質就是統計多工**：靠絕大多數帳號長期閒置來攤薄成本。要量化的是：承諾 vs 實際可用（inode 上限、entry processes、I/O ops、CPU 配額）——這些通常寫在服務條款或知識庫，而非定價頁。

### 5. 儲存、備援與資料韌性
- 備份頻率與保留期（免費 vs 付費）→ 儲存成本轉嫁策略
- RAID / 分散式儲存（Ceph）/ 複本數 → 硬體冗餘成本
- 狀態頁歷史事故 → 真實可用性 vs SLA 承諾的差距（把兩者並列，這是最有說服力的一張表）

### 6. 電力、機櫃與永續
自建 vs colo、PUE 揭露、再生能源承諾、機櫃功率密度（kW/rack）→ 反映硬體世代與散熱設計。

### 7. 供應鏈訊號（硬體供應商讀者的重點）
從招聘職缺、新聞稿、供應商案例研究、專利中萃取：ODM/OEM 夥伴、伺服器機型、網通設備品牌、擴建節奏與資本支出量級。
**這一節對硬體製造商最有價值**——直接對應可切入的供應機會。

## 輸出格式
寫入 `artifacts/<target>/infrastructure.md`：
1. 網路自主性判定（ASN 表 + 結論）
2. 全球節點地圖（表格 + mermaid 或地區分組）
3. 運算層剖析：虛擬化技術 | 隔離機制 | 硬體世代 | 超賣機制
4. 資源承諾 vs 實際可用對照表
5. 單機經濟推估（列出全部假設與敏感度）
6. 供應鏈與採購訊號
7. 資料來源分級：**A=直接量測 / B=供應商官方揭露 / C=第三方報導 / D=合理推估**

## 鐵律
每個數字標註來源等級。推估必須寫出算式與假設，讓人能替換參數重算。
不要把 D 級推估寫得像 A 級量測——這是這份分析唯一不可犯的錯。
