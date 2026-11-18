// check-patterns 的測試本體之二：步驟引擎（challenge、recipe、kernel、steps 四個片段）。
// 每種步驟（fanout、challenge、write、loop、check）測正常、達上限、缺件；loop 另測「第二輪過」與「跑滿輪數」。
// 用法：node -e <本檔> <片段檔…>；通過印 ok，沒通過印理由並 exit 1。
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const NAMES = 'awfInit, awfKernelInit, awfTrace, awfCapped, awfStepsRun, awfStepsErrors, awfStepsChecks, awfStepsLimits, awfStepsRoles, awfStepsFill'
const make = new Function('agent', 'log', 'parallel', 'phase', src + `\nreturn { ${NAMES} }`)
const PROFILE = { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'standard' }, floors: {} } }
const NS = 'agent-workflow:'
const env = (p) => ({ status: 'done', summary: 's', payload: p })
const finds = (ids) => env({ lens: 'x', findings: ids.map((id) => ({ id, severity: 'info', claim: 'c', location: 'a:1', recommendation: 'r', confidence: 0.9 })) })
const verds = (rows) => env({ verdicts: rows.map(([target_id, refuted]) => ({ target_id, refuted, reason: 'x' })) })
const judge = (result) => env({ commit: 'abc', scope_covered: 'x', scope_skipped: 'none', anti_cheat: [], criteria: [{ id: 'until', criterion: 'c', command: 'x', result, evidence: 'exit 1' }] })
const docs = env({ documents: [{ path: 'a.md', kind: 'other', audience: 'internal' }] })
const errs = []
const need = (c, m) => { if (!c) errs.push(m) }
const par = async (thunks) => Promise.all(thunks.map((t) => t()))
// fake：依 label 回傳；null＝缺件；'THROW'＝agent 失敗；沒寫的 label 依 schema 名給一個合理的交件
function kit(fakes, cfg) {
  const calls = []
  const prompts = {}
  const phases = []
  const k = make(async (prompt, o) => {
    calls.push(o.label)
    prompts[o.label] = prompt
    const f = (fakes || {})[o.label]
    if (f === 'THROW') throw new Error('boom')
    if (f === null) return null
    if (f) return typeof f === 'function' ? f(prompt, o) : f
    return o.schema === 'F' ? finds([]) : o.schema === 'V' ? verds([]) : o.schema === 'C' ? judge('pass') : docs
  }, () => {}, par, (t) => phases.push(t))
  k.awfInit({ run_dir: '_runs/r1', stage: 'uat', lang: 'zh-TW' })
  k.awfKernelInit({ profile: (cfg || {}).profile || PROFILE, stage: 'uat', modifiers: [], max_agents: (cfg || {}).max_agents || 40 })
  k.calls = calls
  k.prompts = prompts
  k.phases = phases
  k.run = (steps, vars) => k.awfStepsRun({ steps }, { ns: NS, vars: vars || {}, context: '背景', cwd: '專案根目錄', profile: (cfg || {}).profile || PROFILE, schemas: { findings: 'F', verdicts: 'V', docs: 'D', criteria: 'C' } })
  return k
}
const FAN = { id: 'survey', kind: 'fanout', angles: [{ key: 'a', ask: '看 {what|全部}' }, { key: 'b', ask: '看 b' }] }
const CHAL = { id: 'refute', kind: 'challenge', ask: '反駁' }
const WRITE = { id: 'report', kind: 'write', ask: '寫 {what}', data: [{ label: '成立', from: 'refute.kept' }] }
const LOOP = { id: 'fix', kind: 'loop', max_rounds: 3, until: { command: 'make test ARG={what}', timeout: 60 }, steps: [{ id: 'patch', kind: 'write', ask: '整理' }] }
const CHECK = { id: 'lint', kind: 'check', command: 'make lint' }
const many = (n) => Array.from({ length: n }, (_, i) => ({ key: `k${i}`, ask: 'x' }))
const invalid = async (name, steps, re, cfg) => {
  const k = kit({}, cfg)
  const r = await k.run(steps)
  need(r.halted === true && r.invalid_definition === true && re.test(r.halt_reason) && k.calls.length === 0, `${name}：定義不合規則要 halted、不派工：${JSON.stringify([r.halt_reason, k.calls])}`)
}

;(async () => {
  let k, r

  // ── 上限寫死、和工具端一致 ──
  k = kit()
  need(JSON.stringify(k.awfStepsLimits()) === JSON.stringify({ steps: 12, angles: 8, loop_steps: 4, rounds: 5, batch: 50, batches: 8, data: 8, text: 4000, command: 1000 }), `上限不對：${JSON.stringify(k.awfStepsLimits())}`)
  need(k.awfStepsRoles().join() === 'tpm-researcher,tpm-skeptic,tpm-reviewer,dev-verifier,dev-design-reviewer,dev-security-reviewer', `唯讀角色名單不對：${k.awfStepsRoles()}`)

  // ── 1. fanout ──
  k = kit({ 'survey:a': finds(['f1']), 'survey:b': finds(['f1', 'f2']) })
  r = await k.run([FAN], { what: 'API' })
  need(!r.halted && r.steps.survey.kind === 'fanout' && r.steps.survey.findings === 3 && r.steps.survey.results.length === 2 && k.calls.join() === 'survey:a,survey:b', `fanout 正常：${JSON.stringify([r.halted, r.steps, k.calls])}`)
  need(/看 API/.test(k.prompts['survey:a']) && k.awfTrace().every((t) => t.agent_type === NS + 'tpm-researcher' && t.tier === 'standard'), 'fanout 要代入參數、經過執行核心、預設派 tpm-researcher')
  k = kit()
  r = await k.run([FAN])
  need(/看 全部/.test(k.prompts['survey:a']), '沒給的參數用預設值')
  await invalid('fanout 角度超過 8 個', [{ ...FAN, angles: many(9) }], /1～8 個角度/)
  await invalid('fanout 沒有角度', [{ ...FAN, angles: [] }], /1～8 個角度/)
  k = kit({ 'survey:b': null })
  r = await k.run([FAN, { id: 'report', kind: 'write', ask: 'x' }])
  need(r.halted && r.missing.join() === 'survey:b' && /缺件：survey:b/.test(r.halt_reason) && !k.calls.includes('report'), `fanout 缺件要 halted、不往下：${JSON.stringify([r.halt_reason, k.calls])}`)
  k = kit({ 'survey:a': 'THROW' })
  r = await k.run([FAN])
  need(r.halted && r.missing.join() === 'survey:a', `fanout：agent throw 當缺件：${JSON.stringify(r.missing)}`)

  // ── 2. challenge ──
  k = kit({ 'survey:a': finds(['f1']), 'survey:b': finds(['f1']), refute: verds([['survey:a:f1', false], ['survey:b:f1', true]]) })
  r = await k.run([FAN, CHAL])
  need(!r.halted && r.steps.refute.targets === 2 && r.steps.refute.kept.join() === 'survey:a:f1' && r.steps.refute.doubted.join() === 'survey:b:f1' && k.calls.join() === 'survey:a,survey:b,refute', `challenge 正常（預設全部一批）：${JSON.stringify(r.steps.refute)}`)
  need(k.awfTrace().find((t) => t.label === 'refute').agent_type === NS + 'tpm-skeptic' && /survey:a:f1/.test(k.prompts.refute), 'challenge 預設派 tpm-skeptic、發現帶前綴')
  k = kit({ 'survey:a': finds(['f1']), 'survey:b': finds([]), refute: verds([]) })
  r = await k.run([FAN, CHAL])
  need(!r.halted && r.steps.refute.doubted.join() === 'survey:a:f1', 'skeptic 沒判到的算存疑')
  k = kit({ 'survey:a': finds(['f1', 'f2', 'f3']), 'refute:b1': verds([['survey:a:f1', false], ['survey:a:f2', false]]), 'refute:b2': verds([['survey:a:f3', false]]) })
  r = await k.run([FAN, { ...CHAL, batch: 2 }])
  need(!r.halted && k.calls.slice(2).join() === 'refute:b1,refute:b2' && r.steps.refute.kept.length === 3, `challenge 分批：${JSON.stringify(k.calls)}`)
  k = kit({ 'survey:a': finds(Array.from({ length: 9 }, (_, i) => `f${i}`)) })
  r = await k.run([FAN, { ...CHAL, batch: 1 }])
  need(r.halted && r.over_limit && /超過上限 8 批/.test(r.halt_reason) && k.calls.length === 2, `challenge 超過批數上限要 halted、不截斷：${JSON.stringify([r.halt_reason, k.calls])}`)
  await invalid('challenge 的 batch 超過 50', [FAN, { ...CHAL, batch: 51 }], /batch 要是 1～50/)
  await invalid('challenge 前面沒有 fanout', [CHAL], /沒有東西可以反駁/)
  k = kit({ 'survey:a': finds(['f1']), refute: null })
  r = await k.run([FAN, CHAL])
  need(r.halted && r.missing.join() === 'refute' && /缺件：refute/.test(r.halt_reason), `challenge 缺件要 halted（不是全部沒被推翻）：${JSON.stringify(r.halt_reason)}`)
  k = kit()
  r = await k.run([FAN, CHAL])
  need(r.halted && /沒有東西可以反駁/.test(r.halt_reason) && !k.calls.includes('refute'), 'challenge 沒有目標、不是 optional 要 halted')
  k = kit()
  r = await k.run([FAN, { ...CHAL, optional: true }])
  need(!r.halted && r.steps.refute.skipped === true && !k.calls.includes('refute'), 'challenge 沒有目標、optional 就跳過')

  // ── 3. write ──
  k = kit({ 'survey:a': finds(['f1']), refute: verds([['survey:a:f1', false]]) })
  r = await k.run([FAN, CHAL, WRITE], { what: '報告' })
  need(!r.halted && r.steps.report.kind === 'write' && r.docs && /寫 報告\n成立：\[\{/.test(k.prompts.report) && k.awfTrace().find((t) => t.label === 'report').agent_type === NS + 'tpm-writer', `write 正常：${JSON.stringify([r.halted, k.prompts.report])}`)
  await invalid('write 不能換角色', [{ id: 'report', kind: 'write', ask: 'x', role: 'tpm-skeptic' }], /固定由 tpm-writer/)
  await invalid('write 的 data 超過 8 筆', [FAN, { id: 'report', kind: 'write', ask: 'x', data: Array.from({ length: 9 }, () => ({ label: 'l', from: 'survey' })) }], /最多 8 筆/)
  k = kit({ report: null })
  r = await k.run([{ id: 'report', kind: 'write', ask: 'x' }, CHECK])
  need(r.halted && r.missing.join() === 'report' && !r.steps.lint, `write 缺件要 halted：${JSON.stringify(r.halt_reason)}`)
  k = kit()
  r = await k.run([{ id: 'report', kind: 'write', ask: 'x', owner_check: true }])
  need(r.halted && /rule_check/.test(r.halt_reason), 'write 的 owner_check 沒回報要 halted')

  // ── 4. loop ──
  k = kit({ 'fix.r1:until': judge('fail'), 'fix.r2:until': judge('pass') })
  r = await k.run([LOOP], { what: '不該被代入' })
  need(!r.halted && r.steps.fix.rounds === 2 && r.steps.fix.passed === true && k.calls.join() === 'fix.r1:patch,fix.r1:until,fix.r2:patch,fix.r2:until', `loop 第二輪過就停：${JSON.stringify([r.steps.fix && r.steps.fix.rounds, k.calls])}`)
  need(/\nmake test ARG=\{what\}\n/.test(k.prompts['fix.r1:until']) && !/不該被代入/.test(k.prompts['fix.r1:until']), 'until 的指令原樣放進派工單，不代入參數')
  need(/上一輪（第 1 輪）結束條件的指令沒過/.test(k.prompts['fix.r2:patch']) && k.awfTrace().find((t) => t.label === 'fix.r1:until').agent_type === NS + 'dev-verifier', '第二輪帶上一輪的結果；判定預設派 dev-verifier')
  need(k.phases.includes('Check'), 'loop 的判定在 Check phase')
  k = kit({ 'fix.r1:until': judge('fail'), 'fix.r2:until': judge('fail'), 'fix.r3:until': judge('fail') })
  r = await k.run([LOOP, CHECK])
  need(r.halted && r.over_limit && /跑滿 3 輪/.test(r.halt_reason) && r.steps.fix.passed === false && k.calls.length === 6 && !r.steps.lint, `loop 跑滿輪數要 halted、不當成完成：${JSON.stringify([r.halt_reason, k.calls.length])}`)
  k = kit({ 'fix.r1:until': env({ commit: 'a', scope_covered: 'x', scope_skipped: 'none', anti_cheat: [], criteria: [] }) })
  r = await k.run([{ ...LOOP, max_rounds: 1 }])
  need(r.halted && r.over_limit, 'loop：判定沒有條目算沒過')
  await invalid('loop 的 max_rounds 超過 5', [{ ...LOOP, max_rounds: 6 }], /max_rounds 必填，要是 1～5/)
  await invalid('loop 沒寫 max_rounds', [{ ...LOOP, max_rounds: undefined }], /max_rounds 必填/)
  await invalid('loop 裡超過 4 步', [{ ...LOOP, steps: Array.from({ length: 5 }, (_, i) => ({ id: `w${i}`, kind: 'write', ask: 'x' })) }], /1～4 步/)
  await invalid('loop 不能巢狀', [{ ...LOOP, steps: [{ ...LOOP, id: 'inner' }] }], /loop 裡不能再放 loop/)
  await invalid('loop 沒有 until', [{ ...LOOP, until: undefined }], /until\.command 必填/)
  k = kit({ 'fix.r1:until': null })
  r = await k.run([LOOP])
  need(r.halted && r.missing.join() === 'fix.r1:until' && /缺件：fix\.r1:until/.test(r.halt_reason), `loop 判定缺件要 halted：${JSON.stringify(r.halt_reason)}`)
  k = kit({ 'fix.r1:until': judge('fail'), 'fix.r2:patch': null })
  r = await k.run([LOOP])
  need(r.halted && r.missing.join() === 'fix.r2:patch', `loop 裡的步驟缺件要 halted：${JSON.stringify(r.missing)}`)

  // ── 5. check ──
  k = kit()
  r = await k.run([CHECK, { id: 'unit', kind: 'check', command: 'make test', timeout: 30 }])
  need(!r.halted && k.calls.length === 0 && r.steps.lint.deferred === true && JSON.stringify(r.checks) === JSON.stringify([{ id: 'lint', kind: 'check' }, { id: 'unit', kind: 'check' }]) && !JSON.stringify(r).includes('make'), `check 正常：不派工、只留記號、回傳不帶指令文字：${JSON.stringify(r)}`)
  await invalid('check 的指令超過 1000 字', [{ ...CHECK, command: 'x'.repeat(1001) }], /一行的指令（1000 字以內）/)
  await invalid('check 的指令不能多行', [{ ...CHECK, command: 'a\nb' }], /一行的指令/)
  await invalid('check 的 timeout 超過 3600', [{ ...CHECK, timeout: 3601 }], /timeout 要是 1～3600/)
  await invalid('check 沒有指令（缺件）', [{ id: 'lint', kind: 'check' }], /command 必填/)
  await invalid('check 不能放在 loop 裡', [{ ...LOOP, steps: [CHECK] }], /check 不能放在 loop 裡/)

  // ── 整份定義的上限 ──
  await invalid('步驟超過 12 步', Array.from({ length: 13 }, (_, i) => ({ id: `c${i}`, kind: 'check', command: 'true' })), /1～12 步/)
  await invalid('最多可能的派工數超過 limits.max_agents', [{ ...LOOP, max_rounds: 5, steps: [{ ...FAN, angles: many(8) }] }], /最多可能派 45 個 agent，超過上限 40/)
  await invalid('limits.max_agents 調小', [FAN, CHAL, WRITE], /超過上限 3/, { profile: { ...PROFILE, limits: { max_agents: 3 } }, max_agents: 3 })
  await invalid('id 重複', [FAN, { ...CHAL, id: 'survey' }], /重複/)
  await invalid('不認得的種類', [{ id: 'x', kind: 'shell', command: 'true' }], /kind 要是/)
  await invalid('會改檔的角色不能用', [{ ...FAN, role: 'dev-implementer' }], /role 只能是/)
  await invalid('合併角色不能用', [{ ...FAN, role: 'dev-merge-steward' }], /role 只能是/)
  await invalid('帶命名空間的模版角色不能用', [{ ...FAN, role: NS + 'tpm-researcher' }], /role 只能是/)

  // ── 角色可換：名單裡的唯讀角色、使用者自己的 agent（完整的 agent_type，不加命名空間）──
  k = kit({ 'survey:a': finds(['f1']) })
  r = await k.run([{ ...FAN, role: 'dev-security-reviewer', angles: [FAN.angles[0]] }, { ...CHAL, role: 'acme:perf-bot' }])
  const types = Object.fromEntries(k.awfTrace().map((t) => [t.label, t.agent_type]))
  need(types['survey:a'] === NS + 'dev-security-reviewer' && types.refute === 'acme:perf-bot', `角色沒換對：${JSON.stringify(types)}`)

  // ── 每個回傳都不帶 null（awf save-run 把信封以外的 null 當成缺件）──
  const hasNull = (v) => v === null || (v && typeof v === 'object' && Object.values(v).some(hasNull))
  k = kit({ 'fix.r1:until': judge('pass') })
  r = await k.run([FAN, { ...CHAL, optional: true }, LOOP, CHECK])
  need(!r.halted && !hasNull(r), `回傳不該有 null：${JSON.stringify(r)}`)

  if (errs.length) { console.log(errs.join('\n')); process.exit(1) }
  console.log('ok')
})().catch((e) => { console.log(`例外：${e.stack || e}`); process.exit(1) })
