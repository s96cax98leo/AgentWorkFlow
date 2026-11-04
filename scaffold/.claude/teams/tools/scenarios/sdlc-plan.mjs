// sdlc-plan.js 的乾跑情境：走預設乾跑碰不到的分支。
import { extScenario } from './_helpers.mjs'

const SD_PROFILE = { stages: { design: { system_design: { source: 'System Design Primer', steps: ['1 用例、限制與假設'], feature_scope: 'x', system_scope: 'x' } } } }

export default [
  extScenario('ext:db'),
  {
    name: 'profile 有系統設計方法：skeptic 檢查 SA',
    profile: SD_PROFILE,
    fakes: { 'challenge:intent': (base, prompt) => { if (!/檢查 SA/.test(prompt) || !/標成假設/.test(prompt)) throw new Error('plan 的 skeptic 沒有檢查 SA'); return base } },
    expect: (r) => (r.labels.includes('challenge:intent') ? [] : ['沒有派出 challenge:intent']),
  },
]
