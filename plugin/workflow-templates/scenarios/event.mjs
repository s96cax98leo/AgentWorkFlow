// event.js 的乾跑情境。五種內建事件類型共用一支腳本，情境各自放在 _event-<類型>.mjs（底線開頭的檔不會被當成 workflow 的情境檔），
// 這裡把它們合起來並補上 args.event；另外有「類型：<名>」（每種類型用預設假回傳跑一次，預算基準與合併前後的對照用）、
// 自訂事件類型（profile 的 events）與上限的情境。
import { ENV } from './_helpers.mjs'
import { eventPrefixScenarios } from './_event-shared.mjs'
import { FIND, VERD, labelsAre } from './_steps-shared.mjs'
import meeting from './_event-meeting.mjs'
import decision from './_event-decision.mjs'
import demoCheck from './_event-demo-check.mjs'
import reqsChange from './_event-reqs-change.mjs'
import upstream from './_event-upstream.mjs'

const TYPES = { meeting, decision, 'demo-check': demoCheck, 'reqs-change': reqsChange, upstream }
const typed = Object.entries(TYPES).flatMap(([t, list]) => [
  { name: `類型：${t}`, args: { event: t }, expect: (r) => (r.out && r.out.event === t && r.labels.includes('write') ? [] : [`${t} 用預設假回傳應該走到 writer`]) },
  ...list.map((sc) => ({ ...sc, name: `${t}｜${sc.name}`, args: { ...(sc.args || {}), event: t } })),
])

// 自訂事件類型：使用者只在 profile 加一筆資料
const INCIDENT = {
  title: '事故回顧',
  args: ['incident'],
  context: '事故：{incident}\nrun 目錄：{run_dir}\n環境：{env|正式環境}\n只讀。',
  payload: 'items',
  gather: { label: 'look', angles: [{ key: 'timeline', ask: '重建時間線。' }, { key: 'cause', ask: '列出可能的原因與證據。' }] },
  challenge: { ask: '逐項反駁，不確定判 refuted=true：' },
  write: { ask: '寫事故回顧 {run_dir}/incident-review.md。payload 用 docs。', data: [{ label: '成立', from: 'kept' }, { label: '存疑', from: 'doubted' }] },
}
const withEvents = (events) => ({ events })
// 用步驟宣告的事件類型
const AUDIT = {
  title: '稽核',
  args: ['target'],
  context: '稽核對象：{target}\nrun 目錄：{run_dir}\n範圍：{scope|全部}\n只讀。',
  steps: [
    { id: 'scan', kind: 'fanout', angles: [{ key: 'docs', ask: '文件和實作不一致的地方。' }, { key: 'code', ask: '沒有測試覆蓋的地方。' }] },
    { id: 'refute', kind: 'challenge', ask: '逐則反駁，不確定判 refuted=true：' },
    { id: 'report', kind: 'write', ask: '寫稽核報告 {run_dir}/audit.md。', data: [{ label: '成立', from: 'refute.kept' }, { label: '存疑', from: 'refute.doubted' }] },
  ],
}
const item = (id) => ({ id, type: 'risk', text: 't', owner: 'o', status: 'open', refs: ['a:1'] })

export default [
  ...typed,
  {
    name: '自訂類型：profile 加一筆 events 就能跑，派工單帶出背景、角度與預設值',
    profile: withEvents({ incident: INCIDENT }), args: { event: 'incident', incident: 'INC-1 匯出逾時' },
    fakes: {
      'look:timeline': ENV({ source: 'x', items: [item('i1')] }), 'look:cause': ENV({ source: 'x', items: [item('i1')] }),
      challenge: ENV({ verdicts: [{ target_id: 'timeline:i1', refuted: false, reason: 'ok' }] }),
    },
    expect: (r) => {
      const e = []
      if (!r.out || r.out.halted) e.push(`不該 halted：${r.out && r.out.halt_reason}`)
      if (JSON.stringify(r.labels) !== JSON.stringify(['look:timeline', 'look:cause', 'challenge', 'write'])) e.push(`派工不對：${JSON.stringify(r.labels)}`)
      if (!/事故：INC-1 匯出逾時/.test(r.prompts['look:cause'] || '') || !/環境：正式環境/.test(r.prompts['look:cause'] || '')) e.push('派工單沒有帶出背景或選填參數的預設值')
      if (!/\/tmp\/run\/incident-review\.md/.test(r.prompts.write || '')) e.push('writer 的派工單沒有換掉 {run_dir}')
      if (JSON.stringify(r.out.refuted) !== JSON.stringify(['cause:i1'])) e.push(`skeptic 沒判到的 cause:i1 應列存疑：${JSON.stringify(r.out.refuted)}`)
      if (r.out.event !== 'incident' || !(r.out.trace || []).every((t) => /\/tmp\/run\/incident\//.test(t.record))) e.push('執行紀錄應寫在 <run_dir>/incident/')
      return e
    },
  },
  { name: '自訂類型：缺必要參數就不跑', profile: withEvents({ incident: INCIDENT }), args: { event: 'incident' }, throws: /缺 args\.incident/ },
  { name: '自訂類型：可以整筆取代內建的類型', profile: withEvents({ meeting: { ...INCIDENT, args: ['source'], context: '來源：{source}' } }), args: { event: 'meeting' },
    expect: (r) => (JSON.stringify(r.labels) === JSON.stringify(['look:timeline', 'look:cause', 'challenge', 'write']) ? [] : [`應該用 profile 的定義：${JSON.stringify(r.labels)}`]) },
  { name: '不認得的事件類型：拒絕執行並列出內建的類型', args: { event: 'nope' }, throws: /不認得的事件類型「nope」.*meeting/ },
  { name: '上限：角度超過 8 個就不跑（不靜默截斷）',
    profile: withEvents({ incident: { ...INCIDENT, gather: { label: 'look', angles: Array.from({ length: 9 }, (_, i) => ({ key: `a${i}`, ask: 'x' })) } } }),
    args: { event: 'incident', incident: 'x' }, throws: /gather\.angles 要有 1～8 個/ },
  { name: '可設定：limits.event.max_angles 調到 10 → 9 個角度可以跑',
    profile: { limits: { event: { max_angles: 10 } }, ...withEvents({ incident: { ...INCIDENT, gather: { label: 'look', angles: Array.from({ length: 9 }, (_, i) => ({ key: `a${i}`, ask: 'x' })) } } }) },
    args: { event: 'incident', incident: 'x' },
    expect: (r) => (r.labels.filter((l) => l.startsWith('look:')).length === 9 && r.labels.includes('write') ? [] : [`9 個角度都要派出並走到 writer：${JSON.stringify(r.labels)}`]) },
  { name: '可設定：limits.event.max_angles 調到 1 → 2 個角度就不跑；錯誤訊息寫現在的上限',
    profile: { limits: { event: { max_angles: 1 } }, ...withEvents({ incident: INCIDENT }) },
    args: { event: 'incident', incident: 'x' }, throws: /gather\.angles 要有 1～1 個/ },
  { name: '可設定：limits.event.max_steps 調到 0 → 有前置步驟就不跑',
    profile: { limits: { event: { max_steps: 0 } }, ...withEvents({ incident: { ...INCIDENT, before: [{ name: 'facts', label: 'facts', ask: '列出已知事實。' }] } }) },
    args: { event: 'incident', incident: 'x' }, throws: /before、after 各最多 0 步/ },
  { name: '可設定：limits.event 不合法（超出 1–16）時照預設 8',
    profile: { limits: { event: { max_angles: 99, max_steps: -1 } }, ...withEvents({ incident: { ...INCIDENT, gather: { label: 'look', angles: Array.from({ length: 9 }, (_, i) => ({ key: `a${i}`, ask: 'x' })) } } }) },
    args: { event: 'incident', incident: 'x' }, throws: /gather\.angles 要有 1～8 個/ },
  { name: '定義不完整：缺 write.ask、角度 key 重複都拒絕',
    profile: withEvents({ incident: { ...INCIDENT, write: {}, gather: { label: 'look', angles: [{ key: 'a', ask: 'x' }, { key: 'a', ask: 'y' }] } } }),
    args: { event: 'incident', incident: 'x' }, throws: /定義不完整.*key 不能重複.*write\.ask/ },
  {
    name: '上限：派工數到 limits.max_agents 就停下交人',
    profile: { limits: { max_agents: 3 } }, args: { event: 'meeting' },
    expect: (r) => (r.out && r.out.halted && (r.out.capped || []).length && !r.labels.includes('write') ? [] : [`到上限應該 halted 並列出沒派的：${JSON.stringify(r.out && { halted: r.out.halted, capped: r.out.capped })}`]),
  },
  // 預設的上限是 40（和文件、其他範本一致；原本這支是 20）：6 步前置＋16 個角度＋skeptic＋writer＝24 個要派得完
  ...[['沒寫', {}], ['不合法（0）', { max_agents: 0 }], ['不合法（負數）', { max_agents: -5 }]].map(([what, lim]) => ({
    name: `上限：limits.max_agents ${what} → 用 40，24 個 agent 派得完`,
    profile: { limits: { ...lim, event: { max_angles: 16, max_steps: 6 } }, ...withEvents({ incident: { ...INCIDENT,
      before: Array.from({ length: 6 }, (_, i) => ({ name: `b${i}`, label: `b${i}`, ask: 'x' })),
      gather: { label: 'look', angles: Array.from({ length: 16 }, (_, i) => ({ key: `a${i}`, ask: 'x' })) } } }) },
    args: { event: 'incident', incident: 'x' },
    expect: (r) => (r.labels.length === 24 && r.labels.includes('write') && !(r.out.capped || []).length ? [] : [`24 個都要派出、不能到上限：${r.labels.length} 個，capped=${JSON.stringify(r.out && r.out.capped)}`]),
  })),
  {
    name: '可設定：challenge.heading、write.heading → skeptic 與 writer 的派工單標題照設定（可以用參數名）；沒寫時是「派工單」',
    profile: withEvents({ incident: { ...INCIDENT, challenge: { ...INCIDENT.challenge, heading: '派工單（反駁：{incident}）' }, write: { ...INCIDENT.write, heading: '派工單（事故回顧）' } }, plain: INCIDENT }),
    args: { event: 'incident', incident: 'INC-1' },
    fakes: { 'look:timeline': ENV({ source: 'x', items: [item('i1')] }), 'look:cause': ENV({ source: 'x', items: [item('i1')] }) },
    expect: (r) => [
      ...(/\n\n## 派工單（反駁：INC-1）\n逐項反駁/.test(r.prompts.challenge || '') ? [] : ['skeptic 的派工單標題沒有照設定']),
      ...(/\n\n## 派工單（事故回顧）\n寫事故回顧/.test(r.prompts.write || '') ? [] : ['writer 的派工單標題沒有照設定']),
    ],
  },
  {
    name: '可設定：沒寫 heading 時 skeptic 與 writer 的標題是「派工單」',
    profile: withEvents({ incident: INCIDENT }), args: { event: 'incident', incident: 'INC-1' },
    fakes: { 'look:timeline': ENV({ source: 'x', items: [item('i1')] }), 'look:cause': ENV({ source: 'x', items: [item('i1')] }) },
    expect: (r) => (/\n\n## 派工單\n逐項反駁/.test(r.prompts.challenge || '') && /\n\n## 派工單\n寫事故回顧/.test(r.prompts.write || '') ? [] : ['預設的標題要是「派工單」']),
  },
  // 限制文件與知識圖進共同前綴：內建類型與自訂類型各跑一次（段名就是事件類型）
  ...eventPrefixScenarios('meeting', { args: { event: 'meeting' } }).map((sc) => ({ ...sc, name: `meeting｜${sc.name}` })),
  ...eventPrefixScenarios('incident', { args: { event: 'incident', incident: 'x' }, profile: withEvents({ incident: INCIDENT }) }).map((sc) => ({ ...sc, name: `自訂類型｜${sc.name}` })),
  // steps：事件也可以用資料宣告步驟（同一個引擎；和 gather／challenge／write 的寫法二選一）
  {
    name: 'steps：用步驟宣告的事件類型（fanout → challenge → write）',
    profile: withEvents({ audit: AUDIT }), args: { event: 'audit', target: '報表頁' },
    fakes: { 'scan:docs': FIND('docs', ['f1']), 'scan:code': FIND('code', ['f1']), refute: VERD([['scan:docs:f1', false], ['scan:code:f1', true]]) },
    expect: (r) => {
      const e = labelsAre(r, ['scan:docs', 'scan:code', 'refute', 'report'])
      const o = r.out || {}
      if (o.halted !== false || o.event !== 'audit') e.push(`不該 halted、event 要是類型名：${JSON.stringify([o.halted, o.event, o.halt_reason])}`)
      if (!/稽核對象：報表頁/.test(r.prompts['scan:docs'] || '') || !/範圍：全部/.test(r.prompts['scan:docs'] || '')) e.push('背景沒有代入參數與預設值')
      if (!/成立：\[\{[^\n]*scan:docs:f1/.test(r.prompts.report || '') || !/存疑：\[\{[^\n]*scan:code:f1/.test(r.prompts.report || '')) e.push('writer 的 data 沒有帶到反駁的結果')
      if (!(o.trace || []).every((t) => /\/tmp\/run\/audit\//.test(t.record))) e.push('執行紀錄應寫在 <run_dir>/audit/')
      return e
    },
  },
  {
    name: 'steps：缺件 → halted',
    profile: withEvents({ audit: AUDIT }), args: { event: 'audit', target: 'x' },
    fakes: { 'scan:docs': FIND('docs', ['f1']), 'scan:code': 'THROW' },
    expect: (r) => (r.out && r.out.halted === true && /缺件：scan:code/.test(r.out.halt_reason || '') && !r.labels.includes('report') ? [] : [`缺件應該停下：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'steps：定義超過上限 → halted（不派工）',
    profile: withEvents({ audit: { ...AUDIT, steps: [{ id: 'scan', kind: 'fanout', angles: Array.from({ length: 9 }, (_, i) => ({ key: `a${i}`, ask: 'x' })) }] } }), args: { event: 'audit', target: 'x' },
    expect: (r) => (r.out && r.out.halted === true && r.out.invalid_definition === true && r.labels.length === 0 ? [] : ['超過上限應該 halted、不派工']),
  },
  { name: 'steps：缺必要參數就不跑', profile: withEvents({ audit: AUDIT }), args: { event: 'audit', target: '' }, throws: /缺 args\.target/ },
  { name: 'steps：和 gather／challenge／write 混用就拒絕執行', profile: withEvents({ audit: { ...AUDIT, write: { ask: '寫' } } }), args: { event: 'audit', target: 'x' }, throws: /同時寫了 steps 與 write：兩種寫法只能選一種/ },
  // extends：以內建類型為底，只覆寫有寫的欄位（物件深度合併、陣列取代）
  {
    name: 'extends：只改 writer 的問法，其餘沿用內建的 meeting',
    profile: withEvents({ meeting: { extends: 'meeting', write: { ask: '只寫一份摘要 {run_dir}/summary.md。payload 用 docs。' } } }), args: { event: 'meeting' },
    expect: (r) => {
      const e = labelsAre(r, ['extract:action', 'extract:decision', 'extract:risk', 'extract:doc', 'extract:todo', 'challenge', 'write'])
      if (!/只寫一份摘要 \/tmp\/run\/summary\.md/.test(r.prompts.write || '')) e.push('writer 的問法沒有被覆寫')
      if (!/\n事項：/.test(r.prompts.write || '') || !/\n存疑：/.test(r.prompts.write || '')) e.push('沒覆寫的 write.data 要沿用底的（物件深度合併）')
      if (!/每個事項都附原文位置/.test(r.prompts['extract:action'] || '')) e.push('背景要沿用底的')
      return e
    },
  },
  {
    name: 'extends：陣列整個取代（角度只剩自己寫的）；新的類型名也可以',
    profile: withEvents({ standup: { extends: 'meeting', title: '站會', gather: { angles: [{ key: 'blocker', type: 'issue', ask: '只列卡住的事。' }] } } }), args: { event: 'standup', source: 'standup.md' },
    expect: (r) => {
      const e = labelsAre(r, ['extract:blocker', 'challenge', 'write'])
      if (!/只抽 type=issue 的事項/.test(r.prompts['extract:blocker'] || '')) e.push('底的 gather.body 要沿用（只有 angles 被取代）')
      if (!/會議來源：standup\.md/.test(r.prompts['extract:blocker'] || '')) e.push('底的背景與必要參數要沿用')
      if (r.out.event !== 'standup') e.push('event 要是自己的類型名')
      return e
    },
  },
  { name: 'extends：底的必要參數也要給', profile: withEvents({ standup: { extends: 'meeting' } }), args: { event: 'standup', source: '' }, throws: /缺 args\.source/ },
  { name: 'extends：只能指內建的類型', profile: withEvents({ incident: INCIDENT, standup: { extends: 'incident' } }), args: { event: 'standup' }, throws: /extends「incident」不是內建的事件類型/ },
  {
    name: 'extends＋steps：沿用底的背景與參數，流程改用步驟',
    profile: withEvents({ meeting: { extends: 'meeting', steps: [{ id: 'scan', kind: 'fanout', angles: [{ key: 'todo', ask: '只列待辦' }] }, { id: 'report', kind: 'write', ask: '寫待辦清單' }] } }), args: { event: 'meeting' },
    fakes: { 'scan:todo': FIND('todo', ['f1']) },
    expect: (r) => {
      const e = labelsAre(r, ['scan:todo', 'report'])
      if (!/每個事項都附原文位置/.test(r.prompts['scan:todo'] || '')) e.push('背景要沿用底的')
      if (!r.out || r.out.halted !== false) e.push('不該 halted')
      return e
    },
  },
]
