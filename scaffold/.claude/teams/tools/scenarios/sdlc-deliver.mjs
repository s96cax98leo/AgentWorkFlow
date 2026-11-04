// sdlc-deliver.js 的乾跑情境：走預設乾跑碰不到的分支。
import { extScenario } from './_helpers.mjs'

export default [
  extScenario('ext:a11y'),
  {
    name: '缺件：審查軸沒交件',
    fakes: { 'review:bugs': () => null },
    expect: (r) => [
      ...(r.out && r.out.halted && /review:bugs/.test(r.out.halt_reason || '') && (r.out.missing || []).includes('review:bugs') ? [] : [`審查缺件要 halted 並列出 label：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
      ...(r.labels.includes('write') ? ['審查缺件不該寫 PR 說明'] : []),
    ],
  },
  {
    name: '缺件：skeptic 沒交件',
    fakes: { 'verify:spec': () => null },
    expect: (r) => (r.out && r.out.halted && /verify:spec/.test(r.out.halt_reason || '') ? [] : ['反駁缺件要 halted 並寫出 label']),
  },
  {
    name: 'writer 沒交件：列進 missing',
    fakes: { write: () => null },
    expect: (r) => (r.out && !r.out.halted && (r.out.missing || []).includes('write') ? [] : ['writer 缺件要列進 missing']),
  },
]
