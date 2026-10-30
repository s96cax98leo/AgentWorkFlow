## MODIFIED Requirements

### Requirement: 匯出報表
系統 SHALL 在使用者按下匯出時產生 PDF 報表，並顯示進度。

#### Scenario: 正常匯出
- **WHEN** 使用者按下匯出
- **THEN** 下載資料夾出現 PDF 檔

#### Scenario: 顯示進度
- **WHEN** 匯出進行中
- **THEN** 顯示進度條

## REMOVED Requirements

### Requirement: 匯出紀錄
**Reason**: 改由別的 capability 負責
**Migration**: 無

## RENAMED Requirements

- FROM: `### Requirement: 舊名稱`
- TO: `### Requirement: 新名稱`
