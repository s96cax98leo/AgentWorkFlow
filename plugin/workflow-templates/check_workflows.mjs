// 乾跑檢查 workflow 腳本（不呼叫真的 agent）：
//   1. meta 是純字面值，有 name／description，phase() 用到的標題都在 meta.phases
//   2. 每個 agent() 的 schema：根是 object、required ⊆ properties（Workflow 的要求）、agentType 是 tpm-*／dev-*（可帶 plugin 命名空間）且定義檔存在；
//      profile extensions 登記的使用者 agent 例外：沒有命名空間時檢查專案 .claude/agents/<名>.md 存在，其他 plugin 的不檢查定義檔
//   3. 用依 schema 產生的假回傳把腳本從頭跑到尾，確定沒有 runtime 錯誤
//   4. 每張派工單都有 rules.md（含〈專案規則〉）、輸出語言，最後一行是「執行紀錄檔：<路徑>.md」；回傳帶 trace，筆數＝實際派出數
//   5. 印出乾跑經過的 phase（例如 sdlc-build 依開發規範 profile 的 build.test_first 走 Red 或 Tests，smoke_init 用它確認開關有效）
//   6. 派工單的語言（profile 的 prompt_lang 不是 zh-TW，awf 把語言檔內嵌在 profile 的 _prompt_strings）：第 4 項改用字串表的字比對，
//      每張派工單要以那個語言的共同前綴開頭；情境的 expect 多半比對中文本文，這時略過（標 langSafe: true 的照跑），
//      改由 AWF_LANG_BASELINE=<預設語言乾跑的 AWF_SCENARIO_REPORT> 逐情境核對派出的 label、phase、模型、halted、回傳和預設語言完全相同
// 這支檔在 plugin 的 workflow-templates/ 裡，情境在旁邊的 scenarios/。
// 用法：node "<plugin>/workflow-templates/check_workflows.mjs" [--project <專案目錄>] [--wf-dir <產生好的 workflow 目錄>] [--agents-dir <dir>]
//   --project：專案根目錄（檢查擴充 agent 的定義檔 <專案>/.claude/agents/<名>.md 時用）；沒給用環境變數 CLAUDE_PROJECT_DIR，再沒有用目前目錄
//   agents 目錄預設依序找：--agents-dir、環境變數 AWF_PLUGIN_DIR／CLAUDE_PLUGIN_ROOT 的 agents/、這支檔所在 plugin 的 agents/、專案 .claude/agents
import { readFileSync, readdirSync, existsSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const projAt = process.argv.indexOf('--project')
const project = resolve(projAt > 0 && process.argv[projAt + 1] ? process.argv[projAt + 1] : (process.env.CLAUDE_PROJECT_DIR || process.cwd()))
const claude = join(project, '.claude')
const ownAgents = join(here, '..', 'agents')
const argAt = process.argv.indexOf('--agents-dir')
// workflow 腳本的位置：--wf-dir（例如 awf render --all 產生的目錄），沒給就是專案的 .claude/workflows（範本帶佔位符，不能直接乾跑）
const wfAt = process.argv.indexOf('--wf-dir')
const wfDir = wfAt > 0 ? process.argv[wfAt + 1] : join(claude, 'workflows')
if (!existsSync(wfDir)) { console.log(`找不到 workflow 目錄 ${wfDir}：先跑 awf render --all --out <目錄>，再加 --wf-dir <目錄>`); process.exit(1) }
const pluginDir = process.env.AWF_PLUGIN_DIR || process.env.CLAUDE_PLUGIN_ROOT
const agentsDir = argAt > 0 ? process.argv[argAt + 1] : (pluginDir && existsSync(join(pluginDir, 'agents')) ? join(pluginDir, 'agents') : existsSync(ownAgents) ? ownAgents : join(claude, 'agents'))
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

// 派工單的字串表：i18n/zh-TW.json（＝片段 awfText() 的內建字）疊上內嵌 profile 的 _prompt_strings（語言檔）與 prompt_overrides
const ZH = (() => { try { return JSON.parse(readFileSync(join(here, 'i18n', 'zh-TW.json'), 'utf8')) } catch { return {} } })()
// 片段 awfStrings 的規則：prefix.rules 要留 {rules}、{lang}，record.tail 要留 {record} 而且以它結尾，否則不採用
const keepsVars = (k, v) => k === 'prefix.rules' ? v.includes('{rules}') && v.includes('{lang}') : k === 'record.tail' ? v.endsWith('{record}') : true
const effStrings = (prof) => {
  const out = { ...ZH }
  for (const src of [prof && prof._prompt_strings, prof && prof.prompt_overrides]) {
    if (src && typeof src === 'object' && !Array.isArray(src)) for (const k of Object.keys(ZH)) if (typeof src[k] === 'string' && src[k] && keepsVars(k, src[k])) out[k] = src[k]
  }
  return out
}
const reEsc = (x) => x.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
// 字串表的一個字串 → 正規式：{變數} 換成 vars 給的樣式（沒給的換成任意字）
const tplRe = (tpl, vars) => String(tpl).split(/(\{[A-Za-z0-9_]+\})/).map((part) => {
  const m = part.match(/^\{([A-Za-z0-9_]+)\}$/)
  return m ? (vars[m[1]] || '[^\\n]*?') : reEsc(part)
}).join('')
// 第 4 項：字串表的 prefix.rules 與 record.tail 是內建的字時照原本的比對（訊息逐字不變）；換了語言或被蓋掉時依字串表比對，rules.md 一定要在
const promptRules = (S) => (S['prefix.rules'] === ZH['prefix.rules'] && S['record.tail'] === ZH['record.tail']) ? PROMPT_RULES : [
  ['rules.md', /\.claude\/teams\/rules\.md/],
  ['rules.md 與輸出語言（字串表 prefix.rules）', new RegExp(tplRe(S['prefix.rules'], { rules: '\\.claude\\/teams\\/rules\\.md', lang: '[^\\n]*' }))],
  ['最後一行指定執行紀錄檔（字串表 record.tail）', new RegExp(tplRe(S['record.tail'], { record: '\\S+\\/\\S+\\.md' }) + '\\s*$')],
]
const LANG_BASE = (() => { const f = process.env.AWF_LANG_BASELINE; if (!f) return null; try { return JSON.parse(readFileSync(f, 'utf8')) } catch (e) { console.log(`AWF_LANG_BASELINE 讀不懂：${e.message}`); process.exit(1) } })()

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
  // 使用者自己的 agent：profile 的 extensions（加進某一段）與 roles（換掉某一步）
  const prof = embedded(src) || {}
  const strings = effStrings(prof)
  const rules = promptRules(strings)
  const langMode = !!(prof._prompt_strings && typeof prof._prompt_strings === 'object')
  // 用步驟宣告的自訂段與事件（flow.stages[].steps、events.<名>.steps）裡換上去的 agent（role 不是 tpm-*／dev-* 的）也算
  const stepRoles = (list) => (Array.isArray(list) ? list : []).flatMap((s) => (s && typeof s === 'object' ? [s.role, (s.until || {}).role, ...stepRoles(s.steps)] : []))
  const holders = [...((prof.flow || {}).stages || []), ...Object.values(prof.events && typeof prof.events === 'object' ? prof.events : {})]
  const stepAgents = holders.flatMap((h) => stepRoles(h && h.steps)).filter((x) => typeof x === 'string' && !/^(tpm|dev)-/.test(x))
  const extAgents = new Set([...(prof.extensions || []).map((e) => e.agent), ...Object.values(prof.roles && typeof prof.roles === 'object' ? prof.roles : {}), ...stepAgents,
    // sdlc-merge 的合併角色（awf next 從 guard.config.json 的 auto_merge.merger_role 帶進 args）也可以是使用者自己的 agent
    ...(args && typeof args.merger_role === 'string' ? [args.merger_role] : [])])
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
        const lack = rules.filter(([, rx]) => !rx.test(prompt)).map(([k]) => k)
        if (lack.length) errs.push(`agent ${opts.label} 的派工單缺 ${lack.join('、')}（workflow 開頭要呼叫 awfInit，agent 一律透過 tryAgent 派）`)
        // 換了語言：共同前綴要是那個語言的字
        if (langMode && !prompt.startsWith(`${strings['prefix.heading']}\n`)) errs.push(`agent ${opts.label} 的派工單沒有用字串表的共同前綴開頭（${strings['prefix.heading']}）`)
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
  return { errs, agents, visited, labels, prompts, calls, out, langMode, strings }
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
let langSkipped = 0
let langCompared = 0
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
  if (/@schema:[a-z-]+\*\/\s*null/.test(src)) errs.push('schema 尚未內嵌（請跑 awf build）')
  if (/@snippet:[a-z0-9-]+\*\/\s*\/\*@end\*\//.test(src)) errs.push('片段尚未內嵌（請跑 awf build）')
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
    // 換了語言：expect 多半比對中文本文，只跑標 langSafe 的；其餘由 AWF_LANG_BASELINE 的逐情境核對頂替
    const runExpect = sc.expect && (!r.langMode || sc.langSafe)
    if (sc.expect && !runExpect) langSkipped++
    const exp = runExpect ? sc.expect(r) : []
    if (r.langMode && LANG_BASE) {
      const b = ((LANG_BASE[f] || {}).scenarios || []).find((x) => x.name === sc.name)
      if (!b) exp.push('預設語言的乾跑報告沒有這個情境')
      else {
        const cur = { labels: r.labels, visited: r.visited, calls: r.calls, halted: !!(r.out && r.out.halted), out: r.out && typeof r.out === 'object' ? JSON.parse(JSON.stringify(r.out)) : null }
        for (const k of Object.keys(cur)) if (JSON.stringify(cur[k]) !== JSON.stringify(b[k])) exp.push(`換了語言後 ${k} 和預設語言不同（語言只能改派工單的字）`)
        langCompared++
      }
    }
    const scErrs = [...r.errs, ...exp]
    errs.push(...exp.map((e) => `[情境：${sc.name}] ${e}`))
    report[f] = report[f] || { phases: [...titles], scenarios: [] }
    report[f].scenarios.push({ name: sc.name, ok: scErrs.length === 0, halted: !!(r.out && r.out.halted), labels: r.labels, visited: r.visited, prompts: r.prompts, calls: r.calls, out: r.out && typeof r.out === 'object' ? JSON.parse(JSON.stringify(r.out)) : null })
  }
  if (errs.length) { failed++; console.log(`FAIL ${f}\n  - ${[...new Set(errs)].join('\n  - ')}`) }
  else console.log(`ok   ${f}（乾跑 ${agents} 個 agent；phases: ${visited.join('→') || '-'}）`)
}
if (process.env.AWF_SCENARIO_REPORT) writeFileSync(process.env.AWF_SCENARIO_REPORT, JSON.stringify(report))
if (langSkipped || langCompared) console.log(`換了派工單語言：略過 ${langSkipped} 個比對中文本文的 expect，${langCompared} 個情境和預設語言的乾跑逐項核對`)
console.log(failed ? `workflow 檢查：${failed} 支有問題` : 'workflow 檢查：全部通過')
process.exit(failed ? 1 : 0)
