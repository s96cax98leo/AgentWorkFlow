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
// 內建 recipe 的段（和 tools/recipe.py 的 BUILTIN_RECIPES 一致）：各 sdlc-<段>.mjs 用 recipeScenarios 對包含該段的 recipe 各產生一個「recipe:<名>」情境
export const RECIPE_STAGES = {
  feature: ['plan', 'design', 'build', 'sync', 'test', 'deliver', 'merge'],
  bugfix: ['plan', 'build', 'sync', 'test', 'deliver', 'merge'],
  change: ['plan', 'design', 'build', 'sync', 'test', 'deliver', 'merge'],
  hotfix: ['plan', 'build', 'test', 'deliver', 'merge'],
  refactor: ['plan', 'build', 'sync', 'test', 'deliver', 'merge'],
  deps: ['plan', 'build', 'sync', 'test', 'deliver', 'merge'],
  docs: ['plan', 'build', 'deliver', 'merge'],
  'test-only': ['plan', 'build', 'test', 'deliver', 'merge'],
  perf: ['plan', 'design', 'build', 'sync', 'test', 'deliver', 'merge'],
  spike: ['plan'],
  revert: ['plan', 'build', 'test', 'deliver', 'merge'],
}
// 和 check_workflows.mjs 的 SAMPLE_ARGS.subtasks 相同；kind 可整批換（docs 只能 doc、test-only 只能 test）
export const SAMPLE_SUBTASKS = (kind) => [
  { id: 't0', title: '紅燈測試', kind: 'test', owns: ['e2e/tests/a.spec.ts'], depends_on: [], acceptance: ['T1 紅燈'] },
  { id: 't1', title: '後端', kind: 'code', owns: ['services/a.py'], depends_on: ['t0'], acceptance: ['pytest a'] },
  { id: 't2', title: '前端', kind: 'code', owns: ['apps/b.tsx'], depends_on: [], acceptance: ['vitest b'] },
  { id: 't3', title: '整合畫面', kind: 'code', owns: ['apps/c.tsx'], depends_on: ['t1', 't2'], acceptance: ['e2e c'] },
].map((s) => (kind ? { ...s, kind } : s))
// recipe 的下一段（沒有就 null）
export const recipeNextOf = (name, stage) => { const s = RECIPE_STAGES[name]; const i = s.indexOf(stage); return i >= 0 && i + 1 < s.length ? s[i + 1] : null }
// 回傳的 next 要指向 recipe 的下一段（merge 不檢查字面），也不能提到 recipe 沒有的 design／sync
export const checkNext = (name, stage, out) => {
  const errs = []
  const got = String((out && out.next) || '')
  const want = recipeNextOf(name, stage)
  if (want && want !== 'merge' && !got.includes(`sdlc-${want}`)) errs.push(`next 應指向 sdlc-${want}：${got.slice(0, 200)}`)
  for (const o of ['design', 'sync']) if (o !== stage && !RECIPE_STAGES[name].includes(o) && got.includes(`sdlc-${o}`)) errs.push(`next 指向 recipe 沒有的 sdlc-${o}：${got.slice(0, 200)}`)
  return errs
}
// 每個包含 stage 的內建 recipe 一個情境：不 halted、next 依 recipe；extra(name) 可補 args、fakes、expect
export const recipeScenarios = (stage, extra = () => ({})) => Object.keys(RECIPE_STAGES).filter((n) => RECIPE_STAGES[n].includes(stage)).map((n) => {
  const x = extra(n) || {}
  return {
    name: `recipe:${n}`, args: { recipe: n, ...(x.args || {}) }, fakes: x.fakes || {},
    expect: (r) => {
      const o = r.out || {}
      const errs = o.halted ? [`recipe:${n} 不該 halted：${o.halt_reason}`] : []
      errs.push(...checkNext(n, stage, o))
      if (x.expect) errs.push(...x.expect(r))
      return errs
    },
  }
})
