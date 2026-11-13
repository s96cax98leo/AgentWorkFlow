// event-brief.js 的乾跑情境：依賴程式子任務的非程式子任務要等 SDLC、缺件（規則 26）。
import { ENV } from './_helpers.mjs'

const ST = (id, kind, depends_on = []) => ({ id, title: `子任務 ${id}`, kind, owns: [], depends_on, acceptance: ['a'] })
const PLAN = (subtasks) => ENV({ epic: 'sample', subtasks })
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
  ...(r.labels.includes('write') ? ['缺件還派了 writer'] : []),
]

export default [
  {
    name: '相依程式：依賴 code／test 子任務的 doc 等 SDLC，不執行',
    fakes: { plan: PLAN([ST('c1', 'code'), ST('t1', 'test'), ST('d1', 'doc', ['c1']), ST('d2', 'doc', ['d1']), ST('d3', 'doc', ['t1']), ST('r1', 'research')]) },
    expect: (r) => {
      const e = []
      const ran = r.labels.filter((l) => l.startsWith('work:'))
      if (JSON.stringify(ran) !== JSON.stringify(['work:research:r1'])) e.push(`只應執行 r1，實際：${ran.join(',')}`)
      const waiting = (r.out.waiting || []).map((s) => s.id).sort()
      if (JSON.stringify(waiting) !== JSON.stringify(['d1', 'd2', 'd3'])) e.push(`waiting 應為 d1、d2、d3，實際：${waiting.join(',')}`)
      if ((r.out.to_sdlc || []).map((s) => s.id).join(',') !== 'c1,t1') e.push('code、test 都要轉 SDLC')
      if (r.out.halted) e.push(`不該 halted：${r.out.halt_reason}`)
      return e
    },
  },
  {
    name: '缺件：skeptic 沒交件不能當成全部沒被推翻',
    fakes: { plan: PLAN([ST('r1', 'research')]), challenge: () => null },
    expect: halted('challenge'),
  },
  {
    name: '缺件：子任務沒交件',
    fakes: { plan: PLAN([ST('r1', 'research'), ST('r2', 'research', ['r1'])]), 'work:research:r1': () => null },
    expect: (r) => [...halted('work:research:r1')(r), ...(r.labels.includes('work:research:r2') ? ['前一層缺件還跑下一層'] : [])],
  },
  {
    name: '相依指向不存在的子任務：halted',
    fakes: { plan: PLAN([ST('r1', 'research', ['nope'])]) },
    expect: (r) => (r.out.halted && /nope/.test(r.out.halt_reason) ? [] : ['未知相依應 halted']),
  },
]
