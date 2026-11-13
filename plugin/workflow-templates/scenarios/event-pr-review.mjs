// event-pr-review.js 的乾跑情境：缺件（規則 26）——security lens 掛了不能看起來乾淨。
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['缺件時應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
  ...(r.labels.includes('write') ? ['缺件還派了 writer'] : []),
]

export default [
  {
    name: '缺件：security lens 沒交件不能當成沒有發現',
    fakes: { 'review:security': () => null },
    expect: (r) => [...halted('review:security')(r), ...(r.labels.some((l) => l.startsWith('verify:')) ? ['lens 缺件還往下跑'] : [])],
  },
  { name: '缺件：skeptic 沒交件不能當成全部沒被推翻', fakes: { 'verify:bugs': () => null }, expect: halted('verify:bugs') },
]
