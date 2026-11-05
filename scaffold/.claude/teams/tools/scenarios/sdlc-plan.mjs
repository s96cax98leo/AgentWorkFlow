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
  {
    name: '缺件：skeptic 沒交件',
    fakes: { 'challenge:intent': () => null },
    expect: (r) => (r.out && r.out.halted && /challenge:intent/.test(r.out.halt_reason || '') ? [] : [`反駁缺件要 halted 並寫出 label：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
  },
  {
    name: '缺件：intent 沒交件',
    fakes: { intent: () => null },
    expect: (r) => [
      ...(r.out && r.out.halted && /intent/.test(r.out.halt_reason || '') ? [] : ['intent 缺件要 halted']),
      ...(r.labels.includes('challenge:intent') ? ['intent 缺件不該再派 skeptic'] : []),
    ],
  },
  {
    // 調研不是審查類：不中斷，但要列進 missing，並告訴 intent 作者哪些角度沒查
    name: '調研角度沒交件：列進 missing',
    fakes: { 'research:upstream': () => null },
    expect: (r) => [
      ...(r.out && !r.out.halted && (r.out.missing || []).includes('research:upstream') ? [] : [`調研缺件要列進 missing：${JSON.stringify(r.out && r.out.missing)}`]),
      ...(/缺件的調研角度[\s\S]*research:upstream/.test(r.prompts.intent || '') ? [] : ['intent 派工單沒提缺件的調研角度']),
    ],
  },
]
