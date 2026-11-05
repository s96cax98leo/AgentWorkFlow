// check_workflows.mjs 的情境共用的假回傳與 profile 片段。檔名以 _ 開頭的不是情境檔。
// 走預設乾跑碰不到的分支：拆子計劃、分片驗證、平行修正
export const ENV = (payload, extra = {}) => (base) => ({ ...base, status: 'done', open_questions: [], ...extra, payload })
export const SUB_PLANS = [
  { id: 'api', title: '後端', scope: 'API', owns: ['services/'], depends_on: [], interfaces: ['GET /x'] },
  { id: 'ui', title: '前端', scope: '畫面', owns: ['apps/'], depends_on: ['api'], interfaces: ['GET /x'] },
]
export const crit = (id, result) => ({ id, criterion: 'c', command: 'x', result, evidence: 'e' })
export const CRIT = (rows, cheats = []) => ENV({ commit: 'abc', scope_covered: 'x', scope_skipped: 'none', criteria: rows, anti_cheat: cheats })
// 擴充 agent 的情境：一個沒有命名空間的專案 agent、一個其他 plugin 的 agent，各段都要被派出
export const EXT_PROFILE = { extensions: [
  { name: 'db', agent: 'acme-db-reviewer', stage: 'plan', instructions: 'x' },
  { name: 'db2', agent: 'acme-db-reviewer', stage: 'design', instructions: 'x' },
  { name: 'db3', agent: 'acme-db-reviewer', stage: 'test', instructions: 'x' },
  { name: 'a11y', agent: 'my-plugin:a11y', stage: 'deliver', instructions: 'x' },
] }
export const extScenario = (label) => ({
  name: '擴充 agent', args: {}, fakes: {}, profile: EXT_PROFILE, virtualAgents: ['acme-db-reviewer'],
  expect: (r) => (r.labels.includes(label) ? [] : [`沒有派出擴充 agent ${label}：${r.labels.join(',')}`]),
})
// merge：只信讀回的狀態（merge_state），gh pr merge 回報成功時可能只是進了 merge queue
export const EXEC = (state) => (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge', merge_state: state } })
// 對照全部一致、skeptic 沒推翻：才會走到執行模式的合併分支
export const CONSISTENT = {
  analyze: (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge', head_sha: 'abc1234',
    traceability: [{ requirement_id: 'R1', requirement: 'x', evidence: ['e'], result: 'match' }], out_of_scope: [], followups: [] } }),
  challenge: ENV({ verdicts: [{ target_id: 'R1', refuted: false, reason: 'ok' }, { target_id: 'out_of_scope', refuted: false, reason: 'ok' }] }),
}
// sync：plan 的策略決定走哪條路；中轉分支回報的衝突檔要和 plan 預測的一致
export const SYNC_PLAN = (sync) => ENV({ task_id: 'sync-plan', branch: 'feature/demo', commits: [], files_changed: [], tests: [], sync })
export const SYNC_APPLY = (sync, conflicts = []) => ENV({ task_id: 'sync', branch: sync.head_branch, commits: ['abc merge'], files_changed: [], tests: [{ command: 'verify', result: 'pass' }], conflicts, sync })
export const TRANSIT = { strategy: 'transit', behind: 3, published: true, conflicts: ['src/a.ts'], transit_branch: 'merge/demo-into-main-20261012' }
// 派工單去掉 challenge 片段加的共同前綴（## 共同規則…）與最後一行「執行紀錄檔：…」，只留 workflow 自己寫的本文
export const taskBody = (p) => String(p || '').replace(/^## 共同規則[\s\S]*?\n\n/, '').replace(/\n*執行紀錄檔：[^\n]*\s*$/, '')
