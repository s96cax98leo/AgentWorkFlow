// 乾跑檢查 .claude/workflows/*.js（不呼叫真的 agent）：
//   1. meta 是純字面值，有 name／description，phase() 用到的標題都在 meta.phases
//   2. 每個 agent() 的 schema：根是 object、required ⊆ properties（Workflow 的要求）、agentType 是 tpm-*／dev-*（可帶 plugin 命名空間）且定義檔存在；
//      profile extensions 登記的使用者 agent 例外：沒有命名空間時檢查專案 .claude/agents/<名>.md 存在，其他 plugin 的不檢查定義檔
//   3. 用依 schema 產生的假回傳把腳本從頭跑到尾，確定沒有 runtime 錯誤
//   4. 印出乾跑經過的 phase（例如 sdlc-build 依開發規範 profile 的 build.test_first 走 Red 或 Tests，smoke_init 用它確認開關有效）
// 用法：node .claude/teams/tools/check_workflows.mjs [--agents-dir <dir>]
//   agents 目錄預設依序找：--agents-dir、環境變數 AWF_PLUGIN_DIR／CLAUDE_PLUGIN_ROOT 的 agents/、專案 .claude/agents
import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const claude = join(here, '..', '..')
const wfDir = join(claude, 'workflows')
const argAt = process.argv.indexOf('--agents-dir')
const pluginDir = process.env.AWF_PLUGIN_DIR || process.env.CLAUDE_PLUGIN_ROOT
const agentsDir = argAt > 0 ? process.argv[argAt + 1] : (pluginDir && existsSync(join(pluginDir, 'agents')) ? join(pluginDir, 'agents') : join(claude, 'agents'))
const role = (t) => String(t || '').split(':').pop()
const projectAgents = join(claude, 'agents')
const METH_RE = /\/\*@methodology\*\/ ([\s\S]*?) \/\*@end\*\//
const embedded = (src) => { const m = src.match(METH_RE); try { return m ? JSON.parse(m[1]) : null } catch { return null } }
// 情境可以覆寫內嵌的 profile（例如加 extensions），測擴充 agent 的分支
const withProfile = (src, patch) => patch ? src.replace(METH_RE, () => `/*@methodology*/ ${JSON.stringify({ ...(embedded(src) || {}), ...patch })} /*@end*/`) : src

const SAMPLE_ARGS = {
  repo: '/tmp/repo', epic: 'demo-999-sample', requirement: '範例需求', base: 'origin/main', run_dir: '/tmp/run',
  subtasks: [
    { id: 't0', title: '紅燈測試', kind: 'test', owns: ['e2e/tests/a.spec.ts'], depends_on: [], acceptance: ['T1 紅燈'] },
    { id: 't1', title: '後端', kind: 'code', owns: ['services/a.py'], depends_on: ['t0'], acceptance: ['pytest a'] },
    { id: 't2', title: '前端', kind: 'code', owns: ['apps/b.tsx'], depends_on: [], acceptance: ['vitest b'] },
    { id: 't3', title: '整合畫面', kind: 'code', owns: ['apps/c.tsx'], depends_on: ['t1', 't2'], acceptance: ['e2e c'] },
  ],
  has_ui: true, has_security: true,
  decisions: ['同意只新增的測試修改'], issues: [{ target_id: 'spec §2', reason: '缺驗收條件' }],
  brief: 'briefs/sample_x.md', what: 'PR #999', source: 'meetings/x.md', target: 'PR 1', change: 'x', decision: 'x', script: 'x', date: '2026-10-12', pr: 1,
}

function fake(schema) {
  if (!schema) return 'text'
  if (schema.enum) return schema.enum[0]
  switch (schema.type) {
    case 'object': {
      const o = {}
      for (const [k, s] of Object.entries(schema.properties || {})) if ((schema.required || []).includes(k)) o[k] = fake(s)
      return o
    }
    case 'array': return schema.items ? [fake(schema.items)] : []
    case 'string': return 'x'
    case 'number': return Math.min(schema.maximum ?? 0.9, 0.9)
    case 'integer': return 1
    case 'boolean': return false
    default: return null
  }
}

function checkSchema(s, path, errs) {
  if (!s || typeof s !== 'object') return
  if (s.type === 'object') {
    for (const r of s.required || []) if (!(r in (s.properties || {}))) errs.push(`${path}: required「${r}」不在 properties`)
    for (const [k, v] of Object.entries(s.properties || {})) checkSchema(v, `${path}.${k}`, errs)
  }
  if (s.type === 'array') checkSchema(s.items, `${path}[]`, errs)
}

// 乾跑一次：fakes 依 agent label 覆寫假回傳（例如讓大綱判 split=true），沒覆寫的照 schema 產生最小合法值
async function dryRun(src, titles, args, fakes, virtualAgents = []) {
  const errs = []
  const extAgents = new Set(((embedded(src) || {}).extensions || []).map((e) => e.agent))
  let agents = 0
  const visited = []
  const labels = []
  let out
  const api = {
    agent: async (prompt, opts = {}) => {
      agents++
      labels.push(opts.label)
      if (typeof prompt !== 'string' || !prompt) errs.push('agent() 的 prompt 空白')
      if (opts.phase && !titles.has(opts.phase)) errs.push(`opts.phase「${opts.phase}」不在 meta.phases`)
      if (opts.agentType && extAgents.has(opts.agentType)) {
        if (!opts.agentType.includes(':') && !virtualAgents.includes(opts.agentType) && !existsSync(join(projectAgents, `${opts.agentType}.md`)))
          errs.push(`擴充 agent ${opts.agentType} 的定義檔不存在（${projectAgents}）`)
      } else if (!opts.agentType || !/^(tpm|dev)-/.test(role(opts.agentType))) errs.push(`agent ${opts.label} 沒有指定 tpm-*／dev-* agentType`)
      else if (!existsSync(join(agentsDir, `${role(opts.agentType)}.md`))) errs.push(`agentType ${opts.agentType} 的定義檔不存在（${agentsDir}）`)
      if (!opts.schema) errs.push(`agent ${opts.label} 沒有 schema（交接要結構化）`)
      else { if (opts.schema.type !== 'object') errs.push(`agent ${opts.label} 的 schema 根不是 object`); checkSchema(opts.schema, opts.label, errs) }
      const base = fake(opts.schema)
      const o = fakes[opts.label]
      return o ? (typeof o === 'function' ? o(base, prompt) : { ...base, ...o }) : base
    },
    parallel: async (thunks) => Promise.all(thunks.map((t) => t().catch((e) => { errs.push(`parallel 內錯誤：${e.message}`); return null }))),
    pipeline: async (items, ...stages) => Promise.all(items.map(async (it, i) => { let r = it; for (const s of stages) { r = await s(r === it && s === stages[0] ? it : r, it, i) } return r })),
    phase: (t) => { if (!titles.has(t)) errs.push(`phase('${t}') 不在 meta.phases`); if (visited[visited.length - 1] !== t) visited.push(t) },
    log: () => {},
    budget: { total: null, spent: () => 0, remaining: () => Infinity },
    workflow: async () => null,
  }
  try {
    const body = src.replace(/^export const meta = /m, 'const meta = ')
    const fn = new (Object.getPrototypeOf(async function () {}).constructor)('args', ...Object.keys(api), body)
    out = await fn(args, ...Object.values(api))
    if (out === undefined) errs.push('腳本沒有 return 結果')
  } catch (e) { errs.push(`乾跑失敗：${e.message}`) }
  return { errs, agents, visited, labels, out }
}

// 走預設乾跑碰不到的分支：拆子計劃、分片驗證、平行修正
const ENV = (payload, extra = {}) => (base) => ({ ...base, status: 'done', open_questions: [], ...extra, payload })
const SUB_PLANS = [
  { id: 'api', title: '後端', scope: 'API', owns: ['services/'], depends_on: [], interfaces: ['GET /x'] },
  { id: 'ui', title: '前端', scope: '畫面', owns: ['apps/'], depends_on: ['api'], interfaces: ['GET /x'] },
]
const crit = (id, result) => ({ id, criterion: 'c', command: 'x', result, evidence: 'e' })
const CRIT = (rows, cheats = []) => ENV({ commit: 'abc', scope_covered: 'x', scope_skipped: 'none', criteria: rows, anti_cheat: cheats })
// 擴充 agent 的情境：一個沒有命名空間的專案 agent、一個其他 plugin 的 agent，各段都要被派出
const EXT_PROFILE = { extensions: [
  { name: 'db', agent: 'acme-db-reviewer', stage: 'plan', instructions: 'x' },
  { name: 'db2', agent: 'acme-db-reviewer', stage: 'design', instructions: 'x' },
  { name: 'db3', agent: 'acme-db-reviewer', stage: 'test', instructions: 'x' },
  { name: 'a11y', agent: 'my-plugin:a11y', stage: 'deliver', instructions: 'x' },
] }
const extScenario = (label) => ({
  name: '擴充 agent', args: {}, fakes: {}, profile: EXT_PROFILE, virtualAgents: ['acme-db-reviewer'],
  expect: (r) => (r.labels.includes(label) ? [] : [`沒有派出擴充 agent ${label}：${r.labels.join(',')}`]),
})
// merge：只信讀回的狀態（merge_state），gh pr merge 回報成功時可能只是進了 merge queue
const EXEC = (state) => (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge', merge_state: state } })
// 對照全部一致、skeptic 沒推翻：才會走到執行模式的合併分支
const CONSISTENT = {
  analyze: (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge', head_sha: 'abc1234',
    traceability: [{ requirement_id: 'R1', requirement: 'x', evidence: ['e'], result: 'match' }], out_of_scope: [], followups: [] } }),
  challenge: ENV({ verdicts: [{ target_id: 'R1', refuted: false, reason: 'ok' }, { target_id: 'out_of_scope', refuted: false, reason: 'ok' }] }),
}
const SCENARIOS = {
  'sdlc-merge.js': [
    {
      name: '讀回已合併',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('merged') },
      expect: (r) => (r.labels.includes('execute:merge') && r.out && r.out.merged === true && !r.out.queued ? [] : [`讀回 merged 應該 merged=true：${JSON.stringify(r.out && { merged: r.out.merged, queued: r.out.queued })}`]),
    },
    {
      name: '只進 merge queue',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('queued') },
      expect: (r) => [
        ...(r.out && r.out.merged === false && r.out.queued === true ? [] : [`排隊中不能算已合併：${JSON.stringify(r.out && { merged: r.out.merged, queued: r.out.queued })}`]),
        ...(r.labels.includes('followup') ? ['排隊中不該產出 followup 文件'] : []),
      ],
    },
    {
      name: 'gate 判 merge 但沒讀回狀態',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge' } }) },
      expect: (r) => (r.labels.includes('execute:merge') && r.out && r.out.merged === false ? [] : ['沒有 merge_state 不能算已合併']),
    },
  ],
  'sdlc-plan.js': [extScenario('ext:db')],
  'sdlc-deliver.js': [extScenario('ext:a11y')],
  'sdlc-design.js': [
    extScenario('ext:db2'),
    {
      name: '拆成子計劃',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: '預估 12 個子任務', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['@api'], acceptance: ['x'] }] }),
        consolidate: (base, prompt) => {
          if (!prompt.includes('"depends_on":["api-t1"]')) throw new Error('@api 相依沒有展開成 api-t1')
          return ENV({ epic: 'demo', subtasks: [
            { id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'], sub_plan: 'api' },
            { id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['api-t1'], acceptance: ['x'], sub_plan: 'ui' },
          ] })(base)
        },
      },
      expect: (r) => [
        ...(['subplan:api', 'subplan:ui', 'consolidate', 'analyze:sub-plans'].every((l) => r.labels.includes(l)) ? [] : [`沒有走完子計劃流程：${r.labels.join(',')}`]),
        ...(r.out && r.out.halted ? [`不該中斷：${r.out.halt_reason}`] : []),
      ],
    },
    {
      name: '子計劃範圍重疊要停下',
      fakes: { outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: [SUB_PLANS[0], { ...SUB_PLANS[1], owns: ['services/web/'] }] }) },
      expect: (r) => (r.out && r.out.halted && !r.labels.some((l) => l.startsWith('subplan:')) ? [] : ['範圍重疊卻沒有在平行規劃前停下']),
    },
    {
      name: '彙整掉了子任務要停下',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: [], acceptance: ['x'] }] }),
        consolidate: ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
      },
      expect: (r) => (r.out && r.out.halted ? [] : ['彙整少了 ui-t1 卻沒有停下']),
    },
  ],
  'sdlc-test.js': [
    extScenario('ext:db3'),
    {
      name: '分片驗證＋平行修正',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('A1', 'fail'), crit('U1', 'fail'), crit('I1', 'pass')]),
        'verify:api#2': CRIT([crit('A1', 'pass')]),
        'verify:ui#2': CRIT([crit('U1', 'pass')]),
        'verify:integration#2': CRIT([crit('I1', 'pass')]),
        'consolidate#2': CRIT([crit('A1', 'pass'), crit('U1', 'pass'), crit('I1', 'pass')]),
      },
      expect: (r) => [
        ...(['verify:api#1', 'verify:ui#1', 'verify:integration#1', 'consolidate#1', 'fix:api#1', 'fix:ui#1', 'integrate:fix#1', 'consolidate#2'].every((l) => r.labels.includes(l)) ? [] : [`沒有走完分片與平行修正：${r.labels.join(',')}`]),
        ...(r.out && r.out.halted ? [`不該中斷：${r.out.halt_reason}`] : []),
        ...(r.out && r.out.summary && r.out.summary.pass === 3 ? [] : ['第 2 輪後應該全部 pass']),
      ],
    },
    {
      name: '彙整竄改結果要停下',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'consolidate#1': CRIT([crit('A1', 'pass')]),
      },
      expect: (r) => (r.out && r.out.halted ? [] : ['彙整把 fail 改成 pass 卻沒有停下']),
    },
  ],
}

let failed = 0
for (const f of readdirSync(wfDir).filter((x) => x.endsWith('.js')).sort()) {
  // 換行統一成 \n：Windows 上的檔案可能是 CRLF
  const src = readFileSync(join(wfDir, f), 'utf8').replace(/\r\n/g, '\n')
  const errs = []
  const m = src.match(/^export const meta = (\{[\s\S]*?\n\})\n/m)
  if (!m) { errs.push('找不到 export const meta = {...}') }
  let meta = null
  if (m) {
    if (/[`$]|\.\.\.|\w\(/.test(m[1].replace(/'[^']*'/g, "''"))) errs.push('meta 必須是純字面值（不能有變數、函式、樣板字串、spread）')
    try { meta = Function(`return (${m[1]})`)() } catch (e) { errs.push(`meta 解析失敗：${e.message}`) }
    if (meta && (!meta.name || !meta.description)) errs.push('meta 缺 name 或 description')
    if (meta && meta.name !== f.replace(/\.js$/, '')) errs.push(`meta.name「${meta.name}」與檔名不同`)
  }
  if (/@schema:[a-z-]+\*\/\s*null/.test(src)) errs.push('schema 尚未內嵌（請跑 build.py）')
  if (/Date\.now\(\)|Math\.random\(\)|new Date\(\)/.test(src)) errs.push('不能用 Date.now()／Math.random()／new Date()')
  const titles = new Set((meta && meta.phases || []).map((p) => p.title))
  let agents = 0
  const visited = []
  const scenarios = [{ name: '', args: {}, fakes: {} }, ...(SCENARIOS[f] || [])]
  for (const sc of scenarios) {
    const r = await dryRun(withProfile(src, sc.profile), titles, { ...SAMPLE_ARGS, ...sc.args }, sc.fakes, sc.virtualAgents)
    agents += r.agents
    r.visited.forEach((t) => { if (!visited.includes(t)) visited.push(t) })
    errs.push(...r.errs.map((e) => (sc.name ? `[情境：${sc.name}] ${e}` : e)))
    if (sc.expect) errs.push(...sc.expect(r).map((e) => `[情境：${sc.name}] ${e}`))
  }
  if (errs.length) { failed++; console.log(`FAIL ${f}\n  - ${[...new Set(errs)].join('\n  - ')}`) }
  else console.log(`ok   ${f}（乾跑 ${agents} 個 agent；phases: ${visited.join('→') || '-'}）`)
}
console.log(failed ? `workflow 檢查：${failed} 支有問題` : 'workflow 檢查：全部通過')
process.exit(failed ? 1 : 0)
