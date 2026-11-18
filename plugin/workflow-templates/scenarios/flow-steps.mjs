// flow-steps.js 的乾跑情境：用資料宣告的步驟（profile 的 flow.stages[].steps，引擎是 steps 片段）。
// 每種步驟的正常、缺件、達上限；loop 的「第二輪過」與「跑滿輪數」；指令不代入參數；角色可換。共用的定義在 _steps-shared.mjs。
import { CRIT, crit } from './_helpers.mjs'
import { FIND, VERD, flowOf, FULL, labelsAre } from './_steps-shared.mjs'

const A = { stage: 'uat', epic: 'export' }
const run = (steps, extra = {}) => ({ profile: flowOf(steps, extra.stage || {}), args: { ...A, ...(extra.args || {}) } })
const angles = (n) => Array.from({ length: n }, (_, i) => ({ key: `a${i}`, ask: 'x' }))
const invalid = (name, steps, re, prof = {}) => ({
  name, profile: { ...flowOf(steps), ...prof }, args: A,
  expect: (r) => {
    const e = []
    if (!r.out || r.out.halted !== true || r.out.invalid_definition !== true) e.push(`定義不合規則應該 halted：${JSON.stringify(r.out && r.out.halt_reason)}`)
    else if (!re.test(r.out.halt_reason)) e.push(`理由不對（要符合 ${re}）：${r.out.halt_reason}`)
    if (r.labels.length) e.push(`不該派出任何 agent：${JSON.stringify(r.labels)}`)
    return e
  },
})

export default [
  {
    name: '全流程：fanout → challenge → loop（第二輪過）→ write → check',
    ...run(FULL, { stage: { context: '驗收 {epic}（{who|示範客戶 ACME}）' } }),
    fakes: {
      'survey:api': FIND('api', ['f1', 'f2']), 'survey:ui': FIND('ui', ['f1']),
      refute: VERD([['survey:api:f1', false], ['survey:api:f2', true]]),
      'fix.r1:until': CRIT([crit('until', 'fail')]), 'fix.r2:until': CRIT([crit('until', 'pass')]),
    },
    expect: (r) => {
      const e = labelsAre(r, ['survey:api', 'survey:ui', 'refute', 'fix.r1:patch', 'fix.r1:until', 'fix.r2:patch', 'fix.r2:until', 'report'])
      const o = r.out || {}
      if (o.halted !== false || o.stage !== 'uat') e.push(`不該 halted、stage 要是段名：${JSON.stringify([o.halted, o.stage, o.halt_reason])}`)
      const s = o.steps || {}
      if (!s.survey || s.survey.findings !== 3) e.push('fanout 的發現數不對')
      if (!s.refute || JSON.stringify(s.refute.kept) !== JSON.stringify(['survey:api:f1']) || JSON.stringify(s.refute.doubted) !== JSON.stringify(['survey:api:f2', 'survey:ui:f1'])) e.push(`被推翻與沒判到的都要列存疑：${JSON.stringify(s.refute)}`)
      if (!s.fix || s.fix.rounds !== 2 || s.fix.passed !== true) e.push(`loop 應該第二輪過：${JSON.stringify(s.fix && [s.fix.rounds, s.fix.passed])}`)
      if (!s.lint || s.lint.deferred !== true) e.push('check 在 workflow 裡只是記號')
      if (JSON.stringify(o.checks) !== JSON.stringify([{ id: 'fix', kind: 'until' }, { id: 'lint', kind: 'check' }])) e.push(`checks 不對：${JSON.stringify(o.checks)}`)
      if (JSON.stringify(o).includes('npm run lint')) e.push('回傳不該帶 check 的指令文字（工具只認 profile 寫的）')
      if (!/驗收 export（示範客戶 ACME）/.test(r.prompts['survey:api'] || '')) e.push('背景的 {參數名} 與 {參數名|預設} 沒有代入')
      if (!/自訂段：uat/.test(r.prompts.report || '')) e.push('派工單要帶段名')
      if (!/\nnpm test -- \{epic\}\n/.test(r.prompts['fix.r1:until'] || '')) e.push('until 的指令要原樣出現（不代入參數）')
      if (!/上一輪（第 1 輪）結束條件的指令沒過/.test(r.prompts['fix.r2:patch'] || '') || /上一輪/.test(r.prompts['fix.r1:patch'] || '')) e.push('第二輪的派工單要帶上一輪的結果，第一輪沒有')
      if (!/成立：\[\{[^\n]*survey:api:f1/.test(r.prompts.report || '') || !/判定：\{"round":2,"result":"pass"/.test(r.prompts.report || '')) e.push('write 的 data 沒有帶到前面步驟的結果')
      const types = Object.fromEntries(r.calls.map((c) => [c.label, c.agentType.replace(/^.*:/, '')]))
      if (types['survey:api'] !== 'tpm-researcher' || types.refute !== 'tpm-skeptic' || types['fix.r1:until'] !== 'dev-verifier' || types.report !== 'tpm-writer' || types['fix.r1:patch'] !== 'tpm-writer') e.push(`預設角色不對：${JSON.stringify(types)}`)
      if (JSON.stringify(r.visited) !== JSON.stringify(['Gather', 'Challenge', 'Write', 'Check', 'Write', 'Check', 'Write'])) e.push(`phase 不對：${JSON.stringify(r.visited)}`)
      return e
    },
  },
  {
    name: 'loop：跑滿輪數還沒過 → halted（不默默當成完成）',
    ...run(FULL),
    fakes: { 'survey:api': FIND('api', ['f1']), 'survey:ui': FIND('ui', []), refute: VERD([['survey:api:f1', false]]), 'fix.r1:until': CRIT([crit('until', 'fail')]), 'fix.r2:until': CRIT([crit('until', 'fail')]) },
    expect: (r) => {
      const e = labelsAre(r, ['survey:api', 'survey:ui', 'refute', 'fix.r1:patch', 'fix.r1:until', 'fix.r2:patch', 'fix.r2:until'])
      const o = r.out || {}
      if (o.halted !== true || o.over_limit !== true || !/跑滿 2 輪/.test(o.halt_reason || '')) e.push(`應該因為跑滿輪數 halted：${JSON.stringify(o.halt_reason)}`)
      if (!o.steps || !o.steps.fix || o.steps.fix.passed !== false) e.push('loop 的結果要記沒過')
      return e
    },
  },
  {
    name: 'loop：判定交了空的 criteria 算沒過（判不出來不當成通過）',
    ...run([{ id: 'fix', kind: 'loop', max_rounds: 1, until: { command: 'true' }, steps: [{ id: 'patch', kind: 'write', ask: 'x' }] }]),
    fakes: { 'fix.r1:until': CRIT([]) },
    expect: (r) => (r.out && r.out.halted === true && r.out.over_limit === true ? [] : [`空的 criteria 應該算沒過：${JSON.stringify(r.out)}`]),
  },
  {
    name: 'loop：判定缺件 → halted',
    ...run(FULL),
    fakes: { 'survey:api': FIND('api', ['f1']), 'survey:ui': FIND('ui', []), refute: VERD([['survey:api:f1', false]]), 'fix.r1:until': 'THROW' },
    expect: (r) => (r.out && r.out.halted === true && /缺件：fix\.r1:until/.test(r.out.halt_reason || '') && !r.labels.includes('report') ? [] : [`判定缺件應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'fanout：一個角度缺件 → halted，不往下',
    ...run(FULL),
    fakes: { 'survey:api': FIND('api', ['f1']), 'survey:ui': 'THROW' },
    expect: (r) => {
      // 缺件的那一個會由執行核心升級模型重派一次，所以 label 出現兩次；重派還是缺件才停下
      const e = labelsAre({ labels: [...new Set(r.labels)] }, ['survey:api', 'survey:ui'])
      if (!r.out || r.out.halted !== true || !/缺件：survey:ui/.test(r.out.halt_reason || '') || r.out.stage !== 'survey') e.push(`應該因為缺件 halted：${JSON.stringify(r.out && r.out.halt_reason)}`)
      if (JSON.stringify(r.out && r.out.missing) !== JSON.stringify(['survey:ui'])) e.push('missing 要列出缺件的 label')
      return e
    },
  },
  {
    name: 'challenge：缺件 → halted（不是「全部沒被推翻」）',
    ...run(FULL),
    fakes: { 'survey:api': FIND('api', ['f1']), 'survey:ui': FIND('ui', []), refute: 'THROW' },
    expect: (r) => (r.out && r.out.halted === true && /缺件：refute/.test(r.out.halt_reason || '') ? [] : [`反駁缺件應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'challenge：沒有發現、不是 optional → halted',
    ...run([FULL[0], { id: 'refute', kind: 'challenge', ask: '反駁' }]),
    fakes: { 'survey:api': FIND('api', []), 'survey:ui': FIND('ui', []) },
    expect: (r) => (r.out && r.out.halted === true && /沒有東西可以反駁/.test(r.out.halt_reason || '') && !r.labels.includes('refute') ? [] : [`沒有發現時應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'challenge：沒有發現、optional → 跳過，繼續',
    ...run([FULL[0], { id: 'refute', kind: 'challenge', ask: '反駁', optional: true }, { id: 'report', kind: 'write', ask: '寫', data: [{ label: '存疑', from: 'refute.doubted' }] }]),
    fakes: { 'survey:api': FIND('api', []), 'survey:ui': FIND('ui', []) },
    expect: (r) => {
      const e = labelsAre(r, ['survey:api', 'survey:ui', 'report'])
      if (!r.out || r.out.halted !== false || !r.out.steps.refute || r.out.steps.refute.skipped !== true) e.push('optional 的反駁應該跳過')
      if (!/存疑：\[\]/.test(r.prompts.report || '')) e.push('跳過時 data 是空清單')
      return e
    },
  },
  {
    name: 'challenge：batch 分批（3 則、每批 2 則 → 兩批）',
    ...run([FULL[0], { id: 'refute', kind: 'challenge', ask: '反駁', batch: 2 }]),
    fakes: { 'survey:api': FIND('api', ['f1', 'f2']), 'survey:ui': FIND('ui', ['f1']), 'refute:b1': VERD([['survey:api:f1', false], ['survey:api:f2', false]]), 'refute:b2': VERD([['survey:ui:f1', false]]) },
    expect: (r) => {
      const e = labelsAre(r, ['survey:api', 'survey:ui', 'refute:b1', 'refute:b2'])
      if (!r.out || r.out.halted !== false || (r.out.steps.refute.kept || []).length !== 3) e.push(`三則都沒被推翻：${JSON.stringify(r.out && r.out.steps.refute)}`)
      return e
    },
  },
  {
    name: 'challenge：沒寫 batch 就是全部一批',
    ...run([{ id: 'survey', kind: 'fanout', angles: [{ key: 'api', ask: 'x' }] }, { id: 'refute', kind: 'challenge', ask: '反駁' }]),
    fakes: { 'survey:api': FIND('api', Array.from({ length: 30 }, (_, i) => `f${i}`)) },
    expect: (r) => labelsAre(r, ['survey:api', 'refute']),
  },
  {
    name: 'challenge：超過 8 批 → halted（不截斷）',
    ...run([{ id: 'survey', kind: 'fanout', angles: [{ key: 'api', ask: 'x' }] }, { id: 'refute', kind: 'challenge', ask: '反駁', batch: 1 }]),
    fakes: { 'survey:api': FIND('api', Array.from({ length: 9 }, (_, i) => `f${i}`)) },
    expect: (r) => (r.out && r.out.halted === true && r.out.over_limit === true && /超過上限 8 批/.test(r.out.halt_reason || '') && r.labels.length === 1 ? [] : [`超過批數上限應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'write：缺件 → halted',
    ...run([{ id: 'report', kind: 'write', ask: '寫' }]),
    fakes: { report: 'THROW' },
    expect: (r) => (r.out && r.out.halted === true && /缺件：report/.test(r.out.halt_reason || '') ? [] : [`write 缺件應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'write：owner_check 沒回報 rule_check → halted',
    ...run([{ id: 'report', kind: 'write', ask: '寫', owner_check: true }]),
    expect: (r) => (r.out && r.out.halted === true && /rule_check/.test(r.out.halt_reason || '') ? [] : [`owner_check 沒過應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'check：只有 check 的定義不派任何 agent，回傳列出要由工具執行的步驟',
    ...run([{ id: 'lint', kind: 'check', command: 'npm run lint' }, { id: 'unit', kind: 'check', command: 'npm test', timeout: 60 }]),
    expect: (r) => {
      const e = labelsAre(r, [])
      if (!r.out || r.out.halted !== false || JSON.stringify(r.out.checks) !== JSON.stringify([{ id: 'lint', kind: 'check' }, { id: 'unit', kind: 'check' }])) e.push(`checks 不對：${JSON.stringify(r.out)}`)
      return e
    },
  },
  {
    name: '角色：唯讀步驟可以換成名單裡的唯讀角色與登記過的擴充 agent',
    ...run([
      { id: 'survey', kind: 'fanout', role: 'dev-security-reviewer', angles: [{ key: 'api', ask: 'x' }] },
      { id: 'refute', kind: 'challenge', role: 'acme:perf-bot', ask: '反駁' },
      { id: 'fix', kind: 'loop', max_rounds: 1, until: { command: 'true', role: 'tpm-reviewer' }, steps: [{ id: 'patch', kind: 'write', ask: 'x' }] },
    ]),
    fakes: { 'survey:api': FIND('api', ['f1']), refute: VERD([['survey:api:f1', false]]), 'fix.r1:until': CRIT([crit('until', 'pass')]) },
    expect: (r) => {
      const types = Object.fromEntries(r.calls.map((c) => [c.label, c.agentType]))
      const ok = /:dev-security-reviewer$/.test(types['survey:api']) && types.refute === 'acme:perf-bot' && /:tpm-reviewer$/.test(types['fix.r1:until']) && /:tpm-writer$/.test(types['fix.r1:patch'])
      const tr = ((r.out || {}).trace || []).find((t) => t.label === 'refute') || {}
      return [...(ok ? [] : [`角色不對：${JSON.stringify(types)}`]), ...(tr.agent_type === 'acme:perf-bot' ? [] : [`trace 要記實際的 agent_type：${JSON.stringify(tr)}`])]
    },
  },
  invalid('上限：步驟超過 12 步', Array.from({ length: 13 }, (_, i) => ({ id: `c${i}`, kind: 'check', command: 'true' })), /1～12 步/),
  invalid('上限：角度超過 8 個', [{ id: 'survey', kind: 'fanout', angles: angles(9) }], /1～8 個角度/),
  invalid('上限：loop 裡超過 4 步', [{ id: 'fix', kind: 'loop', max_rounds: 1, until: { command: 'true' }, steps: Array.from({ length: 5 }, (_, i) => ({ id: `w${i}`, kind: 'write', ask: 'x' })) }], /1～4 步/),
  invalid('上限：max_rounds 超過 5', [{ id: 'fix', kind: 'loop', max_rounds: 6, until: { command: 'true' }, steps: [{ id: 'w', kind: 'write', ask: 'x' }] }], /max_rounds 必填，要是 1～5/),
  invalid('上限：max_rounds 沒寫', [{ id: 'fix', kind: 'loop', until: { command: 'true' }, steps: [{ id: 'w', kind: 'write', ask: 'x' }] }], /max_rounds 必填/),
  invalid('上限：不能巢狀 loop', [{ id: 'fix', kind: 'loop', max_rounds: 1, until: { command: 'true' }, steps: [{ id: 'in', kind: 'loop', max_rounds: 1, until: { command: 'true' }, steps: [{ id: 'w', kind: 'write', ask: 'x' }] }] }], /loop 裡不能再放 loop/),
  invalid('上限：check 不能放在 loop 裡', [{ id: 'fix', kind: 'loop', max_rounds: 1, until: { command: 'true' }, steps: [{ id: 'c', kind: 'check', command: 'true' }] }], /check 不能放在 loop 裡/),
  invalid('上限：最多可能的派工數超過 limits.max_agents', [{ id: 'fix', kind: 'loop', max_rounds: 5, until: { command: 'true' }, steps: [{ id: 'survey', kind: 'fanout', angles: angles(8) }] }], /最多可能派 45 個 agent，超過上限 40/),
  invalid('上限：limits.max_agents 調小之後同一份定義被擋', FULL, /超過上限 5/, { limits: { max_agents: 5 } }),
  invalid('角色：write 不能換角色', [{ id: 'report', kind: 'write', ask: 'x', role: 'tpm-researcher' }], /write 固定由 tpm-writer 做/),
  invalid('角色：會寫程式的角色不能用', [{ id: 'survey', kind: 'fanout', role: 'dev-implementer', angles: angles(1) }], /role 只能是/),
  invalid('指令：多行的指令不收', [{ id: 'lint', kind: 'check', command: 'true\nrm -rf x' }], /一行的指令/),
  {
    name: '段名對不到 profile 的宣告 → halted（不派工）',
    profile: flowOf(FULL), args: { stage: 'nope' },
    expect: (r) => (r.out && r.out.halted === true && r.labels.length === 0 ? [] : ['找不到宣告應該 halted']),
  },
  {
    name: '步驟只認 profile：args 帶 steps 也不採用',
    profile: flowOf([{ id: 'lint', kind: 'check', command: 'true' }]), args: { ...A, steps: [{ id: 'report', kind: 'write', ask: '寫' }], context: '別的背景' },
    expect: (r) => (r.out && r.out.halted === false && r.labels.length === 0 && r.out.steps.lint ? [] : [`args 的 steps 不該被採用：${JSON.stringify(r.labels)}`]),
  },
]
