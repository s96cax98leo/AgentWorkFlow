// check-patterns 的測試本體：pattern 庫（challenge、recipe、kernel、patterns 四個片段）。由 tests/check_patterns.py 原樣搬來。
// 用法：node -e <本檔> <片段檔…>；通過印 ok，沒通過印理由並 exit 1。
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const NAMES = 'awfInit, awfKernelInit, awfAgent, awfTrace, awfCapped, awfFanOut, awfClassifyAct, awfAdversarial, awfGenerateFilter, awfTournament, awfLoop'
const make = new Function('agent', 'log', 'parallel', src + `\nreturn { ${NAMES} }`)
const PROFILE = { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'standard' }, floors: {} } }
const NS = 'agent-workflow:'
const env = (p) => ({ status: 'done', summary: 's', payload: p })
const errs = []
const need = (c, m) => { if (!c) errs.push(m) }
const par = async (thunks) => Promise.all(thunks.map((t) => t()))
// fake：依 label 回傳；函式就呼叫它；沒寫的 label 回一個空 payload 的交件
function kit(fakes, cfg) {
  const calls = []
  const k = make(async (prompt, o) => {
    calls.push(o.label)
    const f = (fakes || {})[o.label]
    if (f === 'THROW') throw new Error('boom')
    if (f === null) return null
    return typeof f === 'function' ? f(prompt, o) : (f || env({ ok: true }))
  }, () => {}, par)
  k.awfInit({ run_dir: '_runs/r1', stage: 'build', lang: 'zh-TW' })
  k.awfKernelInit({ profile: PROFILE, stage: 'build', modifiers: [], max_agents: 0, ...(cfg || {}) })
  k.calls = calls
  return k
}
const job = (label, role) => ({ prompt: `做 ${label}`, label, agentType: NS + (role || 'tpm-researcher'), schema: {} })
const throws = async (fn) => { try { await fn(); return '' } catch (e) { return e.message } }
const viaKernel = (k) => k.awfTrace().length > 0 && k.awfTrace().every((t) => t.tier === 'standard' && t.model === 'sonnet')

;(async () => {
  let k, r

  // ── 沒給上限一律 throw ──
  k = kit()
  const nomax = {
    'fan-out': () => k.awfFanOut([job('a')], {}),
    classify: () => k.awfClassifyAct(['x'], { classify: () => job('c'), pick: () => 'a', routes: { a: async () => 1 }, fallback: 'a' }),
    adversarial: () => k.awfAdversarial([job('s1', 'tpm-skeptic')], { quorum: 1, refuted: () => false }),
    generate: () => k.awfGenerateFilter({ generate: (i) => job(`g${i}`), key: () => 'k' }),
    tournament: () => k.awfTournament([job('e1'), job('e2')], { judge: () => job('j'), pick: () => 'a' }),
    loop: () => k.awfLoop({ step: () => job('l'), done: () => true }),
  }
  for (const [name, fn] of Object.entries(nomax)) {
    const m = await throws(fn)
    need(/要給上限 max/.test(m), `${name} 沒給上限應該 throw：${m || '沒有 throw'}`)
  }
  need(k.calls.length === 0, `沒給上限不該派出任何 agent：${k.calls}`)
  for (const bad of [0, -1, 1.5, '3', null]) need(/要給上限 max/.test(await throws(() => k.awfLoop({ max: bad, step: () => job('l'), done: () => true }))), `max=${JSON.stringify(bad)} 應該 throw`)

  // ── 1. fan-out ──
  k = kit({ a: env({ n: 1 }), b: env({ n: 2 }), sum: env({ total: 3 }) })
  r = await k.awfFanOut([job('a'), job('b')], { max: 3, synth: (rs) => ({ ...job('sum', 'tpm-writer'), prompt: `彙整 ${rs.map((x) => x.payload.n).join('+')}` }) })
  need(!r.halted && r.results.length === 2 && r.results[1].payload.n === 2 && r.synthesis.payload.total === 3, `fan-out 正常：${JSON.stringify(r)}`)
  need(k.calls.join() === 'a,b,sum' && viaKernel(k), `fan-out 要經過執行核心：${JSON.stringify(k.awfTrace())}`)
  k = kit()
  r = await k.awfFanOut([job('a'), job('b'), job('c')], { max: 2 })
  need(r.halted && r.over_limit && /超過上限 2/.test(r.reason) && k.calls.length === 0, `fan-out 超過上限要 halted 而且不派：${JSON.stringify(r)}`)
  k = kit({ b: null })
  r = await k.awfFanOut([job('a'), job('b')], { max: 2, synth: () => job('sum') })
  need(r.halted && r.missing.join() === 'b' && /缺件：b/.test(r.reason) && !k.calls.includes('sum'), `fan-out 缺件要 halted、不彙整：${JSON.stringify(r)}`)
  k = kit({ a: 'THROW' })
  r = await k.awfFanOut([job('a')], { max: 1 })
  need(r.halted && r.missing.join() === 'a', `fan-out：agent throw 當缺件：${JSON.stringify(r)}`)
  k = kit({ sum: null })
  r = await k.awfFanOut([job('a')], { max: 1, synth: () => job('sum') })
  need(r.halted && r.missing.join() === 'sum', `fan-out：彙整缺件要 halted：${JSON.stringify(r)}`)

  // ── 2. classify & act ──
  const acted = []
  const routes = { small: async (it) => { acted.push(`small:${it}`); return 's' }, strict: async (it) => { acted.push(`strict:${it}`); return 'x' } }
  const copt = { max: 3, classify: (it, i) => job(`cls:${i}`), pick: (res) => res.payload.kind, routes, fallback: 'strict' }
  k = kit({ 'cls:0': env({ kind: 'small' }), 'cls:1': env({ kind: '沒這類' }), 'cls:2': env({ kind: 'constructor' }) })
  r = await k.awfClassifyAct(['甲', '乙', '丙'], copt)
  need(!r.halted && r.results.map((x) => x.route).join() === 'small,strict,strict' && acted.join() === 'small:甲,strict:乙,strict:丙', `classify 正常（認不出來的走 fallback）：${JSON.stringify([r.results.map((x) => x.route), acted])}`)
  need(viaKernel(k), 'classify 要經過執行核心')
  k = kit()
  r = await k.awfClassifyAct(['1', '2', '3', '4'], copt)
  need(r.halted && r.over_limit && k.calls.length === 0, `classify 超過上限：${JSON.stringify(r)}`)
  acted.length = 0
  k = kit({ 'cls:1': null, 'cls:0': env({ kind: 'small' }) })
  r = await k.awfClassifyAct(['甲', '乙'], copt)
  need(r.halted && r.missing.join() === 'cls:1' && acted.length === 0, `classify 缺件要 halted、不做任何處理：${JSON.stringify([r, acted])}`)
  need(/fallback/.test(await throws(() => k.awfClassifyAct(['甲'], { ...copt, fallback: '沒有' }))), 'classify 沒有合法的 fallback 應該 throw')

  // ── 3. adversarial ──
  const lenses = [job('sk:a', 'tpm-skeptic'), job('sk:b', 'tpm-skeptic'), job('sk:c', 'tpm-skeptic')]
  const aopt = { max: 3, quorum: 2, refuted: (res) => res.payload.refuted }
  k = kit({ 'sk:a': env({ refuted: false }), 'sk:b': env({ refuted: true }), 'sk:c': env({ refuted: false }) })
  r = await k.awfAdversarial(lenses, aopt)
  need(!r.halted && r.upheld === true && r.votes.map((v) => v.refuted).join() === 'false,true,false' && viaKernel(k), `adversarial 正常（兩人維持）：${JSON.stringify(r.votes)}`)
  k = kit({ 'sk:a': env({ refuted: false }), 'sk:b': env({ refuted: true }), 'sk:c': env({}) })
  r = await k.awfAdversarial(lenses, aopt)
  need(!r.halted && r.upheld === false, `adversarial：判不出來的算推翻，只剩一人維持 → 站不住：${JSON.stringify(r.votes.map((v) => v.refuted))}`)
  k = kit()
  r = await k.awfAdversarial(lenses, { ...aopt, max: 2 })
  need(r.halted && r.over_limit && k.calls.length === 0, `adversarial 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'sk:b': null, 'sk:a': env({ refuted: false }), 'sk:c': env({ refuted: false }) })
  r = await k.awfAdversarial(lenses, aopt)
  need(r.halted && r.upheld === false && r.missing.join() === 'sk:b', `adversarial 缺件要 halted（不當成沒推翻）：${JSON.stringify(r)}`)
  need(/quorum/.test(await throws(() => k.awfAdversarial(lenses, { ...aopt, quorum: 4 }))), 'adversarial 的 quorum 超過 lens 數應該 throw')

  // ── 4. generate & filter ──
  const gopt = { max: 4, generate: (i) => job(`gen:${i}`, 'dev-spec-author'), key: (res) => res.payload.title, keep: (res) => res.payload.score >= 3 }
  k = kit({ 'gen:0': env({ title: '甲', score: 5 }), 'gen:1': env({ title: '甲', score: 4 }), 'gen:2': env({ title: '乙', score: 1 }), 'gen:3': env({ title: '丙', score: 3 }) })
  r = await k.awfGenerateFilter(gopt)
  need(!r.halted && r.candidates.length === 4 && r.kept.map((x) => x.payload.title).join() === '甲,丙' && r.dropped === 2 && viaKernel(k), `generate 正常（去重、篩選）：${JSON.stringify(r.kept)}`)
  need(k.calls.length === 4, `generate 的篩選不派 agent：${k.calls}`)
  k = kit()
  r = await k.awfGenerateFilter({ ...gopt, count: 5 })
  need(r.halted && r.over_limit && k.calls.length === 0, `generate 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'gen:2': null })
  r = await k.awfGenerateFilter(gopt)
  need(r.halted && r.missing.join() === 'gen:2' && r.kept.length === 0, `generate 缺件要 halted：${JSON.stringify(r)}`)

  // ── 5. tournament ──
  const entries = [job('e:1', 'dev-spec-author'), job('e:2', 'dev-spec-author'), job('e:3', 'dev-spec-author')]
  const topt = { max: 3, judge: (a, b, round, i) => ({ ...job(`judge:${round}:${i}`, 'tpm-reviewer'), prompt: `${a.label} 對 ${b.label}` }), pick: (res) => res.payload.winner }
  k = kit({ 'judge:1:0': env({ winner: 'b' }), 'judge:2:0': env({ winner: 'a' }) })
  r = await k.awfTournament(entries, topt)
  need(!r.halted && r.winner.label === 'e:2' && r.rounds.length === 2 && r.rounds[0][0].winner === 'e:2' && r.rounds[1][0].b === 'e:3' && viaKernel(k), `tournament 正常（三份、兩輪、輪空晉級）：${JSON.stringify(r.rounds)}`)
  need(k.calls.filter((l) => l.startsWith('judge')).length === 2, `tournament 的評審數應該是參賽數減一：${k.calls}`)
  k = kit()
  r = await k.awfTournament(entries, { ...topt, max: 2 })
  need(r.halted && r.over_limit && k.calls.length === 0, `tournament 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'e:2': null })
  r = await k.awfTournament(entries, topt)
  need(r.halted && r.missing.join() === 'e:2' && r.winner === null && !k.calls.some((l) => l.startsWith('judge')), `tournament 參賽作品缺件：${JSON.stringify(r)}`)
  k = kit({ 'judge:1:0': null })
  r = await k.awfTournament(entries, topt)
  need(r.halted && r.missing.join() === 'judge:1:0' && r.winner === null, `tournament 評審缺件：${JSON.stringify(r)}`)
  k = kit({ 'judge:1:0': env({ winner: '都好' }) })
  r = await k.awfTournament(entries, topt)
  need(r.halted && /沒有明確選/.test(r.reason) && r.winner === null, `tournament 評審沒選 a／b 要 halted：${JSON.stringify(r)}`)

  // ── 6. loop until done ──
  const lopt = { max: 3, step: (round, prev) => ({ ...job(`fix:${round}`, 'dev-implementer'), prompt: `第 ${round} 輪${prev ? '，上一輪剩 ' + prev.payload.left : ''}` }), done: (res) => res.payload.left === 0 }
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': env({ left: 0 }) })
  r = await k.awfLoop(lopt)
  need(!r.halted && r.done && r.rounds === 2 && r.history.length === 2 && k.calls.join() === 'fix:1,fix:2' && viaKernel(k), `loop 正常（第二輪完成就停）：${JSON.stringify(r)}`)
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': env({ left: 2 }), 'fix:3': env({ left: 1 }) })
  r = await k.awfLoop(lopt)
  need(r.halted && r.over_limit && !r.done && r.rounds === 3 && /跑滿 3 輪/.test(r.reason) && k.calls.length === 3, `loop 跑滿上限要 halted、不當成完成：${JSON.stringify(r)}`)
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': null })
  r = await k.awfLoop(lopt)
  need(r.halted && !r.done && r.missing.join() === 'fix:2' && k.calls.length === 3, `loop 缺件要 halted（standard 失敗會升一級重派一次）：${JSON.stringify([r.missing, k.calls])}`)

  // ── 執行核心的派工數上限在 pattern 裡也生效，原因寫明是上限 ──
  k = kit({}, { max_agents: 2 })
  r = await k.awfFanOut([job('a'), job('b'), job('c')], { max: 5 })
  need(r.halted && r.missing.join() === 'c' && /派工數到上限/.test(r.reason) && r.capped.join() === 'c' && k.calls.length === 2, `核心的上限：${JSON.stringify(r)}`)
  need(k.awfTrace().some((t) => t.label === 'c' && t.capped === true), '到上限沒派的要在 trace 記 capped')

  // ── 角色白名單：清單以外的 agent 直接 throw，不派 ──
  k = kit({}, { roles: [NS + 'tpm-researcher'] })
  r = await k.awfFanOut([job('a')], { max: 1 })
  need(!r.halted, '白名單裡的角色要能派')
  let m = await throws(() => k.awfFanOut([job('b', 'dev-merge-steward')], { max: 1 }))
  need(/只能派登記過的角色/.test(m) && !k.calls.includes('b'), `白名單以外的角色應該 throw：${m || '沒有 throw'}`)
  m = await throws(() => k.awfAgent('p', { label: 'x' }))
  need(/只能派登記過的角色/.test(m), `沒給 agentType 應該 throw：${m || '沒有 throw'}`)
  need(/prompt、label、agentType/.test(await throws(() => k.awfFanOut([{ prompt: 'p', label: 'x' }], { max: 1 }))), '派工單缺 agentType 應該 throw')

  if (errs.length) { console.log(errs.join('\n')); process.exit(1) }
  console.log('ok')
})().catch((e) => { console.log(`例外：${e.stack || e}`); process.exit(1) })
