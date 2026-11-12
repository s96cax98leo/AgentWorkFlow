// event-demo-check.js 的乾跑情境：缺件（規則 26）與 skeptic 沒判到的 green 降級。
import { ENV } from './_helpers.mjs'

const GREEN = (id) => ({ id, type: 'check', text: 't', owner: 'o', status: 'green', refs: ['log:1'] })
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['缺件時應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
  ...(r.labels.includes('write') ? ['缺件還派了 writer'] : []),
]

export default [
  { name: '缺件：檢查面向沒交件', fakes: { 'check:env': () => null }, expect: halted('check:env') },
  {
    name: '缺件：skeptic 沒交件不能讓 green 全部過關',
    fakes: { 'check:flow': ENV({ source: 'x', items: [GREEN('c1')] }), challenge: () => null },
    expect: halted('challenge'),
  },
  {
    name: '撞號：兩個面向都有 c1，skeptic 沒判到的 green 降為 yellow',
    fakes: {
      'check:env': ENV({ source: 'x', items: [GREEN('c1')] }),
      'check:flow': ENV({ source: 'x', items: [GREEN('c1')] }),
      challenge: ENV({ verdicts: [{ target_id: 'env:c1', refuted: false, reason: 'ok' }] }),
    },
    expect: (r) => (JSON.stringify(r.out.downgraded) === JSON.stringify(['flow:c1']) ? [] : [`應只降級 flow:c1，實際 ${JSON.stringify(r.out.downgraded)}`]),
  },
]
