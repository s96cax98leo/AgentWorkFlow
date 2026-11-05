// event-upstream.js 的乾跑情境：缺件（規則 26）、撞號、給決策者文件的 check_rules 核對（規則 12）。
import { ENV } from './_helpers.mjs'

const F = (id) => ({ id, severity: 'important', claim: 'c', location: 'a.ts:1', recommendation: 'r', confidence: 0.9 })
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
]
const noWrite = (r) => (r.labels.includes('write') ? ['缺件還派了 writer'] : [])

export default [
  { name: '缺件：開工快照沒交件（無法證明零寫入）', fakes: { 'snapshot:before': () => null }, expect: (r) => [...halted('snapshot:before')(r), ...noWrite(r)] },
  { name: '缺件：skeptic 沒交件不能當成全部沒被推翻', fakes: { challenge: () => null }, expect: (r) => [...halted('challenge')(r), ...noWrite(r)] },
  { name: '缺件：收尾快照沒交件', fakes: { 'snapshot:after': () => null }, expect: (r) => [...halted('snapshot:after')(r), ...noWrite(r)] },
  {
    name: '撞號：兩個角度都有 f1，writer 落檔且 check_rules 通過',
    fakes: {
      'analyze:conflict': ENV({ lens: 'conflict', findings: [F('f1')] }),
      'analyze:impact': ENV({ lens: 'impact', findings: [F('f1')] }),
      challenge: ENV({ verdicts: [{ target_id: 'conflict:f1', refuted: true, reason: 'merge-tree 沒衝突' }, { target_id: 'impact:f1', refuted: false, reason: 'ok' }] }),
      write: ENV({ documents: [{ path: '/tmp/run/給決策者.md', kind: 'for_owner', audience: 'owner' }], rule_check: 'pass' }),
    },
    expect: (r) => {
      const e = []
      if (r.out.halted) e.push(`不該 halted：${r.out.halt_reason}`)
      if (!(r.out.refuted || []).includes('conflict:f1') || (r.out.refuted || []).includes('impact:f1')) e.push(`refuted 應含 conflict:f1、不含 impact:f1：${JSON.stringify(r.out.refuted)}`)
      // tpm-researcher 上游模式不寫檔：派工單不能叫它寫進 run 目錄（落檔由 tpm-writer 做）
      for (const l of ['analyze:conflict', 'snapshot:before', 'snapshot:after']) if ((r.prompts[l] || '').includes('/tmp/run/')) e.push(`${l}（沒有 Write）仍被要求寫檔`)
      if (!/upstream-before\.md/.test(r.prompts.write || '')) e.push('writer 沒有落上游快照')
      return e
    },
  },
]
