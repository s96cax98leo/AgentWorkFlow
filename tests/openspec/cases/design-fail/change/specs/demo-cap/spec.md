## Purpose

匯出 PDF 報表並留下稽核紀錄的能力，讓使用者在每次匯出後都能追溯是誰在什麼時間匯出了哪一份報表內容。

## ADDED Requirements

### Requirement: 匯出報表
系統 SHALL 在使用者按下匯出時產生 PDF 報表。

#### Scenario: 正常匯出
- **WHEN** 使用者按下匯出
- **THEN** 下載資料夾出現 PDF 檔

#### Scenario: 磁碟已滿
- **GIVEN** 磁碟剩餘空間為 0
- **WHEN** 使用者按下匯出
- **THEN** 顯示錯誤碼 E_DISK_FULL

### Requirement: 匯出紀錄
系統應該把每次匯出寫入稽核紀錄。

#### Scenario: 寫入紀錄
- **WHEN** 匯出完成
- **THEN** 稽核表多一筆 export 事件
