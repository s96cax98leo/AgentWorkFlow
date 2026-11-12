// event.js 的乾跑情境。五種內建事件類型共用一支腳本，情境各自放在 _event-<類型>.mjs（底線開頭的檔不會被當成 workflow 的情境檔），
// 這裡把它們合起來並補上 args.event；另外有「類型：<名>」（每種類型用預設假回傳跑一次，預算基準與合併前後的對照用）、
// 自訂事件類型（profile 的 events）與上限的情境。
import { ENV } from './_helpers.mjs'
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
  { name: '定義不完整：缺 write.ask、角度 key 重複都拒絕',
    profile: withEvents({ incident: { ...INCIDENT, write: {}, gather: { label: 'look', angles: [{ key: 'a', ask: 'x' }, { key: 'a', ask: 'y' }] } } }),
    args: { event: 'incident', incident: 'x' }, throws: /定義不完整.*key 不能重複.*write\.ask/ },
  {
    name: '上限：派工數到 limits.max_agents 就停下交人',
    profile: { limits: { max_agents: 3 } }, args: { event: 'meeting' },
    expect: (r) => (r.out && r.out.halted && (r.out.capped || []).length && !r.labels.includes('write') ? [] : [`到上限應該 halted 並列出沒派的：${JSON.stringify(r.out && { halted: r.out.halted, capped: r.out.capped })}`]),
  },
]
