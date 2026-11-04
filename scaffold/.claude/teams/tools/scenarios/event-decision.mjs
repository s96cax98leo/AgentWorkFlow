// event-decision.js 的乾跑情境：缺件（規則 26）與給決策者文件的 check_rules 核對（規則 12）。
import { ENV } from './_helpers.mjs'

const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
]
const OWNER_DOCS = (rule_check) => ENV({ documents: [{ path: '/tmp/run/給決策者.md', kind: 'for_owner', audience: 'owner' }, { path: '/tmp/run/intent-draft-x.md', kind: 'intent', audience: 'internal' }], rule_check })

export default [
  {
    name: '缺件：skeptic 沒交件不能當成全部沒被推翻',
    fakes: { challenge: () => null },
    expect: (r) => [...halted('challenge')(r), ...(r.labels.includes('write') ? ['缺件還派了 writer'] : [])],
  },
  { name: '缺件：面向沒交件', fakes: { 'find:agents-skills': () => null }, expect: halted('find:agents-skills') },
  {
    name: 'writer 回報 check_rules 通過：正常結束',
    fakes: { write: OWNER_DOCS('pass：3 份全過') },
    expect: (r) => [
      ...(r.out.halted ? [`不該 halted：${r.out.halt_reason}`] : []),
      ...(/--audience owner/.test(r.prompts.write || '') && /intent-draft-/.test(r.prompts.write || '') ? [] : ['writer 派工單缺 --audience owner 或 intent-draft']),
    ],
  },
  { name: 'writer 回報 check_rules 沒過：halted', fakes: { write: OWNER_DOCS('fail：R3 內部編號') }, expect: halted('check_rules') },
]
