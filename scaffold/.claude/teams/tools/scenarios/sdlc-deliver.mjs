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
  {
    // 同一則發現判了多次：每一則都 refuted 才算推翻（只會更嚴）
    name: 'skeptic 對同一發現判定不一：保留',
    fakes: {
      'review:bugs': () => ({ status: 'done', summary: 'x', payload: { findings: [{ id: 'f1', severity: 'important', confidence: 0.9, title: '邊界', file: 'a.py', line: 1, evidence: 'a.py:1' }] } }),
      'verify:bugs': () => ({ status: 'done', summary: 'x', payload: { verdicts: [{ target_id: 'f1', refuted: true, reason: 'a' }, { target_id: 'f1', refuted: false, reason: 'b' }] } }),
    },
    expect: (r) => {
      const ids = ((r.out || {}).findings || []).map((f) => f.id)
      return ids.includes('f1') && (r.out || {}).dropped === 0 ? [] : [`判定不一時 f1 應保留：${JSON.stringify({ ids, dropped: (r.out || {}).dropped })}`]
    },
  },
]
