// 步驟引擎（steps 片段）的情境共用的東西：flow-steps.mjs 與 event.mjs 的 steps 情境都用。底線開頭，不會被當成某支 workflow 的情境檔。
import { ENV } from './_helpers.mjs'

// 一個角度交出的發現（id 是角度內的編號；引擎會加上 <步驟>:<角度>: 前綴）
export const FIND = (lens, ids) => ENV({ lens, findings: ids.map((id) => ({ id, severity: 'info', claim: 'c', location: 'a:1', recommendation: 'r', confidence: 0.9 })) })
// skeptic 的判定：[[target_id, refuted], …]
export const VERD = (rows) => ENV({ verdicts: rows.map(([target_id, refuted]) => ({ target_id, refuted, reason: 'x' })) })
// profile 的 flow：一個用步驟宣告的自訂段 uat（接在 test 後面）
export const flowOf = (steps, extra = {}) => ({ flow: { stages: [{ id: 'uat', after: 'test', run: 'steps', steps, ...extra }] } })
// 五種步驟各一：兩個角度的 fanout、反駁、最多兩輪的 loop、寫文件、確定性檢查。until 的指令故意帶 {epic}：指令不代入參數
export const FULL = [
  { id: 'survey', kind: 'fanout', angles: [{ key: 'api', ask: '看 API 的驗收條件' }, { key: 'ui', ask: '看畫面的驗收條件' }] },
  { id: 'refute', kind: 'challenge', ask: '逐則反駁，不確定判 refuted=true：' },
  { id: 'fix', kind: 'loop', max_rounds: 2, until: { command: 'npm test -- {epic}', timeout: 120 }, steps: [{ id: 'patch', kind: 'write', ask: '把沒過的地方整理成待修清單 {run_dir}/todo.md' }] },
  { id: 'report', kind: 'write', ask: '寫驗收報告 {run_dir}/report.md', data: [{ label: '成立', from: 'refute.kept' }, { label: '存疑', from: 'refute.doubted' }, { label: '判定', from: 'fix.until' }] },
  { id: 'lint', kind: 'check', command: 'npm run lint' },
]
export const labelsAre = (r, want) => (JSON.stringify(r.labels) === JSON.stringify(want) ? [] : [`派工不對：${JSON.stringify(r.labels)}（應該是 ${JSON.stringify(want)}）`])
