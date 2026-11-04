// event-decision.js 的乾跑情境：缺件（規則 26）與給決策者文件的 check_rules 核對（規則 12）。
import { ENV } from './_helpers.mjs'

const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
]
const CH = (id) => ({ id, type: 'change', text: 't', owner: 'o', status: 'open', refs: ['a.md:1'] })
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
  {
    // B-3：預設模式（kept 是被採信的結論）同一事項判了多次，只要有一則 refuted 就列存疑。
    // i1 判 [沒推翻, 推翻]、i2 判 [推翻, 沒推翻]：「以最後一則為準」的實作會把 i2 採信，這個情境就會失敗
    name: 'skeptic 對同一事項判定不一：列存疑',
    fakes: {
      'find:tpm-docs': ENV({ source: 'x', items: [CH('i1'), CH('i2'), CH('i3')] }),
      'find:repo-process': ENV({ source: 'x', items: [] }),
      'find:agents-skills': ENV({ source: 'x', items: [] }),
      'find:code-backlog': ENV({ source: 'x', items: [] }),
      challenge: ENV({ verdicts: [
        { target_id: 'tpm-docs:i1', refuted: false, reason: 'ok' }, { target_id: 'tpm-docs:i1', refuted: true, reason: '超出決策範圍' },
        { target_id: 'tpm-docs:i2', refuted: true, reason: '沒受影響' }, { target_id: 'tpm-docs:i2', refuted: false, reason: 'ok' },
        { target_id: 'tpm-docs:i3', refuted: false, reason: 'ok' },
      ] }),
      write: OWNER_DOCS('pass'),
    },
    expect: (r) => {
      const ref = ((r.out || {}).refuted || []).slice().sort()
      return JSON.stringify(ref) === JSON.stringify(['tpm-docs:i1', 'tpm-docs:i2']) ? [] : [`判定不一的 i1、i2 都要列存疑，i3 採信：${JSON.stringify(ref)}`]
    },
  },
  {
    // B-8：pipeline 裡的面向 agent throw，要走缺件流程
    name: '缺件：面向 agent throw',
    fakes: { 'find:code-backlog': 'THROW' },
    expect: (r) => [...halted('find:code-backlog')(r), ...(r.labels.includes('challenge') ? ['面向 throw 還往下跑'] : [])],
  },
]
