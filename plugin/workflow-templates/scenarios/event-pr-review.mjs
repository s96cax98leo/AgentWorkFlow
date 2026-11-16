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
  {
    // 審查的預設（profile 的 review）：lens 清單、Nit 上限、信心門檻（只能調低）
    name: '可設定：review.lenses、max_nits、min_confidence → 只派指定的 lens，派工單與報告的數字跟著變，0.6 的發現留下來',
    profile: { review: { lenses: ['bugs', 'security'], max_nits: 3, min_confidence: 0.5 } },
    fakes: { 'review:bugs': (base) => ({ ...base, payload: { ...base.payload, findings: [{ id: 'f1', severity: 'important', claim: 'x', location: 'a.py:1', recommendation: 'y', confidence: 0.6 }] } }) },
    expect: (r) => {
      const errs = []
      const lenses = r.labels.filter((l) => l.startsWith('review:')).sort().join(',')
      if (lenses !== 'review:bugs,review:security') errs.push(`只該派 bugs、security：${lenses}`)
      if (!/只列 ≥0\.5。/.test(r.prompts['review:bugs'] || '')) errs.push('lens 派工單的門檻沒有跟著設定')
      if (!r.labels.includes('verify:bugs')) errs.push(`門檻 0.5 時信心 0.6 的發現要送去反駁：${r.labels.join(' → ')}`)
      if (!/Nit 最多 3 則。/.test(r.prompts.write || '')) errs.push('報告的 Nit 上限沒有跟著設定')
      return errs
    },
  },
  {
    name: '可設定：沒設 review 時信心 0.6 的發現不留（門檻 0.8）；min_confidence 高於 0.8 不接受、照 0.8',
    profile: { review: { min_confidence: 0.95 } },
    fakes: { 'review:bugs': (base) => ({ ...base, payload: { ...base.payload, findings: [
      { id: 'f1', severity: 'important', claim: 'x', location: 'a.py:1', recommendation: 'y', confidence: 0.6 },
      { id: 'f2', severity: 'important', claim: 'x', location: 'a.py:2', recommendation: 'y', confidence: 0.85 }] } }) },
    expect: (r) => {
      const errs = []
      if (r.labels.filter((l) => l.startsWith('review:')).length !== 6) errs.push('沒設 lenses 時要派六個 lens')
      if (!/只列 ≥0\.8。/.test(r.prompts['review:bugs'] || '')) errs.push('高於 0.8 的門檻不能採用（會放掉發現）')
      const p = r.prompts['verify:bugs'] || ''
      if (!p || !/f2/.test(p) || /bugs:f1/.test(p)) errs.push('0.85 的發現要留、0.6 的不留')
      if (!/Nit 最多 5 則。/.test(r.prompts.write || '')) errs.push('Nit 上限預設 5')
      return errs
    },
  },
  {
    name: '可設定：conventions.repo_docs.review → 各 lens 照設定的審查規範分級；不合法照 REVIEW.md',
    profile: (p) => ({ conventions: { ...(p.conventions || {}), repo_docs: { review: 'docs/CODE_REVIEW.md' } } }),
    expect: (r) => { const ps = Object.entries(r.prompts).filter(([l]) => l.startsWith('review:')); return [...(ps.length ? [] : ['沒有派出審查']), ...ps.filter(([, p]) => !/依目標 repo 的 docs\/CODE_REVIEW\.md 分級/.test(p)).map(([l]) => `${l} 沒有照設定的審查規範`)] },
  },
  {
    name: '可設定：conventions.repo_docs.review 不合法（含 ..）→ 照 REVIEW.md',
    profile: (p) => ({ conventions: { ...(p.conventions || {}), repo_docs: { review: '../REVIEW.md' } } }),
    expect: (r) => Object.entries(r.prompts).filter(([l, p]) => l.startsWith('review:') && !/依目標 repo 的 REVIEW\.md 分級/.test(p)).map(([l]) => `${l} 要退回 REVIEW.md`),
  },
]
