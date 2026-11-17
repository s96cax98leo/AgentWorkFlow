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
  {
    // 修正輪和第一輪（sdlc-design）用同一套規則：以前修正輪看不到 profile 的指引，重拆的子任務也沒被要求標 spec_refs／expected_red
    name: '修正輪的規則和第一輪一致：profile 的指引、產出文件、拆子任務的規則（spec_refs、expected_red）都在派工單上',
    profile: { stages: { design: { artifacts: ['design.md', 'spec.md', 'plan.md'], requirement_style: '示範客戶 ACME 的寫法', guidance: '示範客戶 ACME：架構取捨寫 ADR。' } } },
    expect: (r) => {
      const p = r.prompts.revise || ''
      return [
        ...(/修正 \S+ 的 design、spec、plan，並重新拆子任務/.test(p) && !/ADR 草稿/.test(p) ? [] : ['要修正的文件要照 stages.design.artifacts，不寫死 ADR']),
        ...(/- 產出文件（放在 \S+\/）：design\.md、spec\.md、plan\.md/.test(p) && /- 需求／驗收條件的寫法：示範客戶 ACME 的寫法。/.test(p) && /- profile 的指引：示範客戶 ACME：架構取捨寫 ADR。/.test(p) ? [] : ['修正輪要帶 design 的產出文件、需求寫法與 profile 的指引']),
        ...(/- 重新拆的子任務照第一輪的規則：每個子任務獨佔 owns 的檔案/.test(p) && /spec_refs/.test(p) && /expected_red/.test(p) ? [] : ['修正輪重拆子任務要帶第一輪的規則（spec_refs、expected_red）']),
        ...(/delta spec/.test(p) ? ['不是變更需求不該出現 delta spec 的規則'] : []),
      ]
    },
  },
  {
    name: '修正輪：變更需求（plan_mode=impact）帶 delta spec 的規則；不測試先行帶「code 不要依賴 test」',
    profile: (p) => ({ ...p, stages: { ...(p.stages || {}), build: { ...((p.stages || {}).build || {}), test_first: false, guidance: 'x' } } }),
    args: { recipe: 'change' },
    expect: (r) => { const p = r.prompts.revise || ''; return /plan_mode=impact）：寫 delta spec/.test(p) && /code 子任務不要依賴 test 子任務/.test(p) ? [] : ['變更需求的修正輪要帶 delta spec 規則與 test_first=false 的說明'] },
  },
]
