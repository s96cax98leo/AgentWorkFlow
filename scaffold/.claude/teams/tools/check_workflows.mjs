// 乾跑檢查 .claude/workflows/*.js（不呼叫真的 agent）：
//   1. meta 是純字面值，有 name／description，phase() 用到的標題都在 meta.phases
//   2. 每個 agent() 的 schema：根是 object、required ⊆ properties（Workflow 的要求）、agentType 是 tpm-*／dev-*（可帶 plugin 命名空間）且定義檔存在；
//      profile extensions 登記的使用者 agent 例外：沒有命名空間時檢查專案 .claude/agents/<名>.md 存在，其他 plugin 的不檢查定義檔
//   3. 用依 schema 產生的假回傳把腳本從頭跑到尾，確定沒有 runtime 錯誤
//   4. 每張派工單都有 rules.md（含〈專案規則〉）、輸出語言，最後一行是「執行紀錄檔：<路徑>.md」；回傳帶 trace，筆數＝實際派出數
//   5. 印出乾跑經過的 phase（例如 sdlc-build 依開發規範 profile 的 build.test_first 走 Red 或 Tests，smoke_init 用它確認開關有效）
// 用法：node .claude/teams/tools/check_workflows.mjs [--agents-dir <dir>] [--wf-dir <產生好的 workflow 目錄>]
//   agents 目錄預設依序找：--agents-dir、環境變數 AWF_PLUGIN_DIR／CLAUDE_PLUGIN_ROOT 的 agents/、專案 .claude/agents
import { readFileSync, readdirSync, existsSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const claude = join(here, '..', '..')
const argAt = process.argv.indexOf('--agents-dir')
// workflow 腳本的位置：--wf-dir（例如 awf.py render --all 產生的目錄），沒給就是專案的 .claude/workflows（範本帶佔位符，不能直接乾跑）
const wfAt = process.argv.indexOf('--wf-dir')
const wfDir = wfAt > 0 ? process.argv[wfAt + 1] : join(claude, 'workflows')
if (!existsSync(wfDir)) { console.log(`找不到 workflow 目錄 ${wfDir}：先跑 python3 .claude/teams/tools/awf.py render --all --out <目錄>，再加 --wf-dir <目錄>`); process.exit(1) }
const pluginDir = process.env.AWF_PLUGIN_DIR || process.env.CLAUDE_PLUGIN_ROOT
const agentsDir = argAt > 0 ? process.argv[argAt + 1] : (pluginDir && existsSync(join(pluginDir, 'agents')) ? join(pluginDir, 'agents') : join(claude, 'agents'))
const role = (t) => String(t || '').split(':').pop()
const projectAgents = join(claude, 'agents')
const METH_RE = /\/\*@methodology\*\/ ([\s\S]*?) \/\*@end\*\//
const embedded = (src) => { const m = src.match(METH_RE); try { return m ? JSON.parse(m[1]) : null } catch { return null } }
// 情境可以覆寫內嵌的 profile（例如加 extensions），測擴充 agent 的分支；profile 給函式時拿到內嵌的 profile、回傳要覆寫的鍵（只改深層的一個值時用）
const withProfile = (src, patch) => patch ? src.replace(METH_RE, () => `/*@methodology*/ ${JSON.stringify({ ...(embedded(src) || {}), ...(typeof patch === 'function' ? patch(embedded(src) || {}) : patch) })} /*@end*/`) : src

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
  event: 'meeting', brief: 'briefs/sample_x.md', what: 'PR #999', source: 'meetings/x.md', target: 'PR 1', change: 'x', decision: 'x', script: 'x', date: '2026-10-12', pr: 1,
}

// 派工單的固定要求（I3：rules.md 與輸出語言；I6：最後一行是執行紀錄檔）
const PROMPT_RULES = [
  ['rules.md（含〈專案規則〉）', /\.claude\/teams\/rules\.md（含〈專案規則〉）/],
  ['輸出語言', /輸出語言：/],
  ['最後一行「執行紀錄檔：<路徑>.md」', /執行紀錄檔：\S+\/\S+\.md\s*$/],
]

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

// 乾跑一次：fakes 依 agent label 覆寫假回傳（例如讓大綱判 split=true），沒覆寫的照 schema 產生最小合法值；
// fake 給 'THROW' 或會 throw 的函式時，agent() 會 throw（模擬 agent 失敗）
async function dryRun(src, titles, args, fakes, virtualAgents = []) {
  const errs = []
  const extAgents = new Set(((embedded(src) || {}).extensions || []).map((e) => e.agent))
  let agents = 0
  const visited = []
  const labels = []
  const prompts = {}
  // 每次派工的 label 與 model（同一個 label 重派時 prompts 只留最後一次，calls 全留）
  const calls = []
  let out
  const api = {
    agent: async (prompt, opts = {}) => {
      agents++
      labels.push(opts.label)
      prompts[opts.label] = typeof prompt === 'string' ? prompt : ''
      calls.push({ label: opts.label, model: opts.model || '', agentType: opts.agentType || '' })
      if (typeof prompt !== 'string' || !prompt) errs.push('agent() 的 prompt 空白')
      else {
        // I3／I6：每張派工單都要有 rules.md（含〈專案規則〉）、輸出語言，最後一行指定執行紀錄檔（challenge 片段的 awfInit／tryAgent 會加）
        const lack = PROMPT_RULES.filter(([, rx]) => !rx.test(prompt)).map(([k]) => k)
        if (lack.length) errs.push(`agent ${opts.label} 的派工單缺 ${lack.join('、')}（workflow 開頭要呼叫 awfInit，agent 一律透過 tryAgent 派）`)
      }
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
      // fake 是 'THROW'（或會 throw 的函式）：模擬 agent 失敗（逾時、被中止、交件驗證錯誤），workflow 要走缺件流程
      if (o === 'THROW') throw new Error(`假 agent ${opts.label} throw`)
      return o ? (typeof o === 'function' ? o(base, prompt) : { ...base, ...o }) : base
    },
    parallel: async (thunks) => Promise.all(thunks.map((t) => t().catch((e) => { errs.push(`parallel 內錯誤：${e.message}`); return null }))),
    // 和 parallel 一樣：某一項 throw 時記成錯誤、該項回 null（workflow 該自己用 tryAgent 接住，走到這裡就是漏接）
    pipeline: async (items, ...stages) => Promise.all(items.map(async (it, i) => { let r = it; for (const s of stages) { r = await s(r === it && s === stages[0] ? it : r, it, i) } return r }).map((p) => p.catch((e) => { errs.push(`pipeline 內錯誤：${e.message}`); return null }))),
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
    // I6：每個 return（含 halted、提早結束）都帶 trace，筆數＝實際派出數（缺件也列，ok=false）
    else if (out && typeof out === 'object') {
      if (!Array.isArray(out.trace)) errs.push('回傳沒有 trace（每個 return 都要帶 trace: awfTrace()）')
      else if (out.trace.filter((t) => !t.capped).length !== agents) errs.push(`trace ${out.trace.filter((t) => !t.capped).length} 筆（不含到上限沒派的），實際派出 ${agents} 個 agent`)
      else if (out.trace.some((t) => !t.record)) errs.push('trace 有沒有執行紀錄檔路徑的項目')
    }
  } catch (e) { errs.push(`乾跑失敗：${e.message}`) }
  return { errs, agents, visited, labels, prompts, calls, out }
}

// 情境放在 scenarios/<workflow>.mjs（export default 陣列），每支 workflow 一檔，平行修改不互相衝突
const SCENARIOS = {}
const scDir = join(here, 'scenarios')
if (existsSync(scDir)) for (const f of readdirSync(scDir).filter((x) => x.endsWith('.mjs') && !x.startsWith('_')).sort()) {
  SCENARIOS[f.replace(/\.mjs$/, '.js')] = (await import(pathToFileURL(join(scDir, f)).href)).default
}

// AWF_SCENARIO_REPORT=<檔>：把每個情境的結果（派出的 label、派工單、halted）寫成 JSON，給計畫的驗證腳本用
const report = {}
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
  if (/@snippet:[a-z0-9-]+\*\/\s*\/\*@end\*\//.test(src)) errs.push('片段尚未內嵌（請跑 build.py）')
  if (/Date\.now\(\)|Math\.random\(\)|new Date\(\)/.test(src)) errs.push('不能用 Date.now()／Math.random()／new Date()')
  const titles = new Set((meta && meta.phases || []).map((p) => p.title))
  let agents = 0
  const visited = []
  const scenarios = [{ name: '', args: {}, fakes: {} }, ...(SCENARIOS[f] || [])]
  for (const sc of scenarios) {
    const r = await dryRun(withProfile(src, sc.profile), titles, { ...SAMPLE_ARGS, ...sc.args }, sc.fakes || {}, sc.virtualAgents)
    // 情境可以宣告「這組參數應該被拒絕執行」（throws：比對錯誤訊息的正規式）：有丟出相符的錯誤才算過
    if (sc.throws) {
      const hit = r.errs.filter((e) => e.startsWith('乾跑失敗：') && sc.throws.test(e))
      r.errs = hit.length ? r.errs.filter((e) => !hit.includes(e)) : [...r.errs, `應該拒絕執行（${sc.throws}），實際沒有`]
    }
    agents += r.agents
    r.visited.forEach((t) => { if (!visited.includes(t)) visited.push(t) })
    errs.push(...r.errs.map((e) => (sc.name ? `[情境：${sc.name}] ${e}` : e)))
    const scErrs = [...r.errs, ...(sc.expect ? sc.expect(r) : [])]
    if (sc.expect) errs.push(...sc.expect(r).map((e) => `[情境：${sc.name}] ${e}`))
    report[f] = report[f] || { phases: [...titles], scenarios: [] }
    report[f].scenarios.push({ name: sc.name, ok: scErrs.length === 0, halted: !!(r.out && r.out.halted), labels: r.labels, visited: r.visited, prompts: r.prompts, calls: r.calls, out: r.out && typeof r.out === 'object' ? JSON.parse(JSON.stringify(r.out)) : null })
  }
  if (errs.length) { failed++; console.log(`FAIL ${f}\n  - ${[...new Set(errs)].join('\n  - ')}`) }
  else console.log(`ok   ${f}（乾跑 ${agents} 個 agent；phases: ${visited.join('→') || '-'}）`)
}
if (process.env.AWF_SCENARIO_REPORT) writeFileSync(process.env.AWF_SCENARIO_REPORT, JSON.stringify(report))
console.log(failed ? `workflow 檢查：${failed} 支有問題` : 'workflow 檢查：全部通過')
process.exit(failed ? 1 : 0)
