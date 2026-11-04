// sdlc-design-revise.js 的乾跑情境：走預設乾跑碰不到的分支。

const SD_PROFILE = { stages: { design: { artifacts: ['spec.md', 'plan.md'], system_design: { source: 'System Design Primer', steps: ['1 用例、限制與假設', '4 擴展與取捨'], feature_scope: 'x', system_scope: 'x' } } } }

export default [
  {
    name: 'profile 有系統設計方法：複查加派 system-design lens',
    profile: SD_PROFILE,
    expect: (r) => (r.labels.includes('recheck:system-design') ? [] : [`沒有派出 recheck:system-design：${r.labels.join(',')}`]),
  },
]
