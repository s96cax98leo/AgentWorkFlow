// sdlc-design-revise.js 的乾跑情境：走預設乾跑碰不到的分支。
import { kgScenarios, roleScenarios } from './_helpers.mjs'

const SD_PROFILE = { stages: { design: { artifacts: ['spec.md', 'plan.md'], system_design: { source: 'System Design Primer', steps: ['1 用例、限制與假設', '4 擴展與取捨'], feature_scope: 'x', system_scope: 'x' } } } }

export default [
  {
    name: 'profile 有系統設計方法：複查加派 system-design lens',
    profile: SD_PROFILE,
    expect: (r) => (r.labels.includes('recheck:system-design') ? [] : [`沒有派出 recheck:system-design：${r.labels.join(',')}`]),
  },
  {
    name: '缺件：複查沒交件',
    fakes: { 'recheck:resolved': () => null },
    expect: (r) => (r.out && r.out.halted && /recheck:resolved/.test(r.out.halt_reason || '') && (r.out.missing || []).includes('recheck:resolved') ? [] : ['複查缺件要 halted 並列出 label']),
  },
  {
    name: '缺件：修正輪沒交件',
    fakes: { revise: () => null },
    expect: (r) => [
      ...(r.out && r.out.halted ? [] : ['revise 缺件要 halted']),
      ...(r.labels.some((l) => l.startsWith('recheck:')) ? ['沒有修正不該派複查'] : []),
    ],
  },
  {
    // 複查 lens 的定義（profile 的 lenses.design_revise）：同名的換問法，新名稱一律加派
    name: 'lens 定義：lenses.design_revise 改寫 resolved、新增 pr-split → 照設定的問法派，新 lens 加派',
    profile: { lenses: { design_revise: { resolved: { ask: '示範客戶 ACME 的複查問法：逐條對照閘門紀錄。' }, 'pr-split': { ask: '修正後的 plan 有沒有照決定拆成兩包 PR？' } } } },
    expect: (r) => [
      ...(r.out && r.out.halted ? [`不該 halted：${r.out.halt_reason}`] : []),
      ...(/示範客戶 ACME 的複查問法/.test(r.prompts['recheck:resolved'] || '') && !/不採信對照表的自述/.test(r.prompts['recheck:resolved'] || '') ? [] : ['resolved 應該換成 profile 的問法']),
      ...(/照決定拆成兩包 PR/.test(r.prompts['recheck:pr-split'] || '') ? [] : [`沒有加派自訂的複查 lens：${r.labels.join(',')}`]),
      ...(/修正有沒有造成新的落差/.test(r.prompts['recheck:regression'] || '') ? [] : ['沒改寫的 regression 要照內建的問法']),
    ],
  },
  {
    name: 'lens 定義不合法（ask 空白、不是物件）→ 只派內建的、問法照內建',
    profile: { lenses: { design_revise: { resolved: { ask: '   ' }, extra: '不是物件' } } },
    expect: (r) => [
      ...(r.labels.filter((l) => l.startsWith('recheck:')).every((l) => ['recheck:resolved', 'recheck:regression', 'recheck:system-design'].includes(l)) && r.labels.includes('recheck:resolved') ? [] : [`只該派內建的複查：${r.labels.join(',')}`]),
      ...(/不採信對照表的自述/.test(r.prompts['recheck:resolved'] || '') ? [] : ['不合法的定義要退回內建的問法']),
    ],
  },
  ...roleScenarios('design.recheck', 'tpm-skeptic', /^recheck:/),
  ...kgScenarios(),
]
