// event-post-merge.js 的乾跑情境：缺件（規則 26）、凍結檔路徑取自設定、給決策者文件的 check_rules 核對（規則 12）。
import { ENV } from './_helpers.mjs'

const F = (id, severity) => ({ id, severity, claim: 'c', location: 'abc1234 https://ci/1', recommendation: 'r', confidence: 0.9 })
const halted = (label) => (r) => [
  ...(r.out && r.out.halted ? [] : ['應回 halted']),
  ...(new RegExp(label).test((r.out && r.out.halt_reason) || '') ? [] : [`halt_reason 沒寫出 ${label}`]),
]
const BLOCKER = { watch: ENV({ lens: 'post-merge', findings: [F('f1', 'blocker')] }), challenge: ENV({ verdicts: [{ target_id: 'f1', refuted: false, reason: 'ok' }] }) }

export default [
  { name: '缺件：watcher 沒交件', fakes: { watch: () => null }, expect: halted('watch') },
  {
    name: '缺件：skeptic 沒交件不能當成歸因全部成立',
    fakes: { watch: BLOCKER.watch, challenge: () => null },
    expect: (r) => [...halted('challenge')(r), ...(r.labels.includes('write') ? ['缺件還派了 writer'] : [])],
  },
  {
    name: 'main 正常：不派 skeptic',
    fakes: { watch: ENV({ lens: 'post-merge', findings: [F('f1', 'info')] }) },
    expect: (r) => (r.out.ok && !r.labels.includes('challenge') ? [] : ['main 正常時應直接結束']),
  },
  {
    name: 'blocker：凍結檔路徑取自 freeze_file，check_rules 通過',
    fakes: {
      ...BLOCKER,
      write: ENV({ documents: [{ path: '/tmp/run/給決策者.md', kind: 'for_owner', audience: 'owner' }, { path: '/tmp/runs/custom.freeze.json', kind: 'other', audience: 'agents', notes: '凍結旗標' }], rule_check: 'pass' }),
    },
    expect: (r) => [
      ...(r.out.halted ? [`不該 halted：${r.out.halt_reason}`] : []),
      ...(r.out.freeze_flag === '/tmp/runs/custom.freeze.json' ? [] : [`freeze_flag 應取 writer 回報的實際路徑：${r.out.freeze_flag}`]),
      ...(/freeze_file/.test(r.prompts.write || '') ? [] : ['writer 派工單沒要求讀 freeze_file']),
    ],
  },
  {
    name: 'blocker：writer 沒回報凍結檔路徑或 rule_check',
    fakes: { ...BLOCKER, write: ENV({ documents: [{ path: '/tmp/run/給決策者.md', kind: 'for_owner', audience: 'owner' }] }) },
    expect: halted('rule_check'),
  },
]
