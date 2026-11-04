// event-meeting.js 的乾跑情境：缺件（規則 26）與多角度事項 id 撞號。
import { ENV } from './_helpers.mjs'

const IT = (id, type) => ({ id, type, text: 't', owner: 'o', status: 'open', refs: ['m.md:1'] })
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['缺件時應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
  ...(r.labels.includes('write') ? ['缺件還派了 writer'] : []),
]

export default [
  { name: '缺件：skeptic 沒交件不能當成全部沒被推翻', fakes: { challenge: () => null }, expect: halted('challenge') },
  {
    name: '缺件：抽取角度沒交件',
    fakes: { 'extract:risk': () => null },
    expect: (r) => [...halted('extract:risk')(r), ...(r.labels.includes('challenge') ? ['抽取缺件還往下跑'] : [])],
  },
  {
    name: '撞號：兩個角度都有 i1，verdict 只推翻 action:i1',
    fakes: {
      'extract:action': ENV({ source: 'm.md', items: [IT('i1', 'action')] }),
      'extract:decision': ENV({ source: 'm.md', items: [IT('i1', 'decision')] }),
      'extract:risk': ENV({ source: 'm.md', items: [] }),
      'extract:doc': ENV({ source: 'm.md', items: [] }),
      'extract:todo': ENV({ source: 'm.md', items: [] }),
      challenge: ENV({ verdicts: [{ target_id: 'action:i1', refuted: true, reason: '原文沒有' }, { target_id: 'decision:i1', refuted: false, reason: 'ok' }] }),
    },
    expect: (r) => {
      const e = []
      if (r.out.halted) e.push('不該 halted')
      if (JSON.stringify(r.out.refuted) !== JSON.stringify(['action:i1'])) e.push(`refuted 應只有 action:i1，實際 ${JSON.stringify(r.out.refuted)}`)
      const [keptPart, doubtPart] = (r.prompts.write || '').split('存疑：')
      if (!/"decision:i1"/.test(keptPart) || /"action:i1"/.test(keptPart)) e.push('writer 的事項應只有 decision:i1')
      if (!/"action:i1"/.test(doubtPart || '')) e.push('writer 的存疑應有 action:i1')
      if (!/action:i1/.test(r.prompts.challenge || '')) e.push('skeptic 拿到的事項沒有角度前綴')
      return e
    },
  },
]
