// event-reqs-change.js 的乾跑情境：缺件（規則 26）與真的沒有差異。
import { ENV } from './_helpers.mjs'

const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['缺件時應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
  ...(r.labels.includes('write') ? ['缺件還派了 writer'] : []),
]

export default [
  { name: '缺件：diff 沒交件不能當成沒有差異', fakes: { diff: () => null }, expect: halted('diff') },
  { name: '缺件：skeptic 沒交件', fakes: { challenge: () => null }, expect: halted('challenge') },
  { name: '缺件：影響面沒交件', fakes: { 'impact:deliverables': () => null }, expect: halted('impact:deliverables') },
  {
    name: 'diff 交件且沒有差異：正常結束',
    fakes: { diff: ENV({ source: 'x', items: [] }) },
    expect: (r) => (!r.out.halted && r.out.note === '沒有差異' && !r.labels.some((l) => l.startsWith('impact:')) ? [] : ['沒有差異時應正常結束、不派影響分析']),
  },
]
