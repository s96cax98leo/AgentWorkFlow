// sdlc-design-revise.js 的乾跑情境：走預設乾跑碰不到的分支。
import { ENV, SUB_PLANS, kgScenarios, roleScenarios } from './_helpers.mjs'

// 只重做受影響的子計劃：上一輪存下來的子任務（SUB_PLANS 是 api、ui，ui 依賴 api）
const sub = (id, sub_plan, owns, depends_on) => ({ id, title: id, kind: 'code', ...(sub_plan ? { sub_plan } : {}), owns, depends_on, acceptance: [`${id} 通過`] })
const UI1 = sub('ui-t1', 'ui', ['apps/a.tsx'], ['api-t1'])
const INT1 = sub('int-t1', 'integration', ['e2e/a.spec.ts'], ['api-t2', 'ui-t1'])
const PREV = [sub('api-t1', 'api', ['services/a.py'], []), sub('api-t2', 'api', ['services/b.py'], ['api-t1']), UI1, INT1]

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
  {
    // 小節資料包（awf pack design 在已經有 design.json 時產生）：修正與複查的派工單指到資料包，明寫不用整份讀 design.json 與 spec.md
    name: '小節資料包：有 → 修正輪指到總覽與子計劃的資料包、寫明不用整份讀；複查也指到總覽',
    args: { sub_plans: SUB_PLANS, revise_pack: '_runs/demo/design/revise/pack.md', revise_packs: { api: '_runs/demo/design/revise/api/pack.md', ui: '_runs/demo/design/revise/../x/design/revise/ui/pack.md' } },
    expect: (r) => {
      const p = r.prompts.revise || ''
      return [
        ...(/\n修正輪資料包（開工先讀這份[^\n]*）：\/tmp\/run\/design\/revise\/pack\.md\n/.test(p) ? [] : ['修正輪的派工單要指到總覽的資料包（run_dir 底下）']),
        ...(/子計劃各有一份資料包[^\n]*：api → \/tmp\/run\/design\/revise\/api\/pack\.md\n/.test(p) && !/ui → /.test(p) ? [] : ['只列路徑合規則的子計劃資料包（ui 的含 ..，要丟掉）']),
        ...(/資料包已經附上要改的小節，不用整份讀 \/tmp\/run\/design\.json 與 spec\.md；要看其他小節時照資料包最後列的行號範圍讀。/.test(p) ? [] : ['要寫明不用整份讀 design.json 與 spec.md']),
        ...Object.entries(r.prompts).filter(([k, v]) => k.startsWith('recheck:') && !/\n修正輪資料包（修正前整理的[^\n]*）：\/tmp\/run\/design\/revise\/pack\.md/.test(v)).map(([k]) => `${k} 的派工單要指到總覽的資料包`),
        ...(/這一輪只重做/.test(p) ? ['沒有 prev_subtasks 不該縮小範圍'] : []),
      ]
    },
  },
  {
    name: '小節資料包：沒有 → 派工單沒有這幾句',
    args: { sub_plans: SUB_PLANS },
    expect: (r) => Object.entries(r.prompts).filter(([, v]) => /修正輪資料包|資料包已經附上/.test(v)).map(([k]) => `${k} 不該出現資料包的句子`),
  },
  {
    name: '小節資料包：路徑不合規則（別段的）被丟掉；總覽不合規則時子計劃的也不列',
    args: { sub_plans: SUB_PLANS, revise_pack: '_runs/demo/build/t1/pack.md', revise_packs: { api: '_runs/demo/design/revise/api/pack.md' } },
    expect: (r) => Object.entries(r.prompts).filter(([, v]) => /修正輪資料包|資料包已經附上|各有一份資料包/.test(v)).map(([k]) => `${k} 不該引用不合規則的資料包`),
  },
  {
    // 只重做受影響的子計劃：問題指到 ui（ui 依賴 api，api 沒被指到）→ 只重做 ui 與整合，api 的子任務由 workflow 接回
    name: '只重做受影響的子計劃：只動 ui → 派工單寫明範圍，api 沿用、子任務原樣在結果裡，@api 展開',
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, decisions: ['ui 的錯誤訊息改成中文'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    fakes: { revise: ENV({ subtasks: [UI1, sub('ui-t2', 'ui', ['apps/b.tsx'], ['@api']), INT1] }) },
    expect: (r) => {
      const o = r.out || {}
      const tasks = ((o.design || {}).payload || {}).subtasks || []
      const ids = tasks.map((t) => t.id)
      const p = r.prompts.revise || ''
      return [
        ...(o.halted ? [`不該中斷：${o.halt_reason}`] : []),
        ...(/這一輪只重做子計劃 ui（[^\n]*子計劃 api 沒被指到，文件與子任務沿用上一輪的：不要改它們的文件，也不用讀。/.test(p) && /沿用的子計劃的子任務不要交/.test(p) && /（api-t1、api-t2）/.test(p) ? [] : ['修正輪的派工單要寫明只重做 ui、api 沿用、可以依賴哪些沿用的子任務']),
        ...(JSON.stringify(ids) === JSON.stringify(['api-t1', 'api-t2', 'ui-t1', 'ui-t2', 'int-t1']) ? [] : [`結果要是沿用的 api 子任務加上交回的：${ids.join(',')}`]),
        ...(JSON.stringify(tasks.filter((t) => t.sub_plan === 'api')) === JSON.stringify(PREV.filter((t) => t.sub_plan === 'api')) ? [] : ['沿用的子任務要原樣接回']),
        ...(JSON.stringify((tasks.find((t) => t.id === 'ui-t2') || {}).depends_on) === JSON.stringify(['api-t1', 'api-t2']) ? [] : ['@api 要展開成沿用的 api 子任務']),
        ...(JSON.stringify(o.revised_sub_plans) === '["ui"]' && JSON.stringify(o.reused_sub_plans) === '["api"]' ? [] : ['回傳要標出重做與沿用的子計劃']),
        ...Object.entries(r.prompts).filter(([k, v]) => k.startsWith('recheck:') && !(/"id":"api-t1"/.test(v) && /這一輪只重做了子計劃 ui，api 的文件與子任務沿用上一輪/.test(v))).map(([k]) => `${k} 要拿到接回後的完整子任務清單與範圍說明`),
      ]
    },
  },
  {
    name: '只重做受影響的子計劃：判斷不出項目屬於哪個子計劃 → 全部重做（派工單與回傳照舊）',
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, decisions: ['整體改用另一種寫法'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    fakes: { revise: ENV({ subtasks: [UI1] }) },
    expect: (r) => {
      const o = r.out || {}
      return [
        ...(/這一輪只重做/.test(r.prompts.revise || '') ? ['有一項指不到子計劃就不該縮小範圍'] : []),
        ...(JSON.stringify((((o.design || {}).payload || {}).subtasks || []).map((t) => t.id)) === '["ui-t1"]' && !('reused_sub_plans' in o) ? [] : ['全部重做時結果就是修正輪交回的那一份，不接回上一輪的']),
      ]
    },
  },
  {
    name: '只重做受影響的子計劃：指到被別人依賴的 api → 依賴它的 ui 也要重做，等於全部重做',
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, decisions: ['api 的回傳格式改成分頁'], issues: [{ target_id: 'api-t1', reason: '缺驗收條件' }] },
    expect: (r) => (/這一輪只重做/.test(r.prompts.revise || '') ? ['api 被 ui 依賴，兩個都要重做時不該縮小範圍'] : []),
  },
  {
    name: '只重做受影響的子計劃：revise_all＝true → 全部重做',
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, revise_all: true, decisions: ['ui 的錯誤訊息改成中文'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    expect: (r) => (/這一輪只重做/.test(r.prompts.revise || '') ? ['revise_all 時不該縮小範圍'] : []),
  },
  {
    name: '只重做受影響的子計劃：上一輪有對不到子計劃的子任務 → 全部重做',
    args: { sub_plans: SUB_PLANS, prev_subtasks: [...PREV, sub('misc', '', ['docs/x.md'], [])], decisions: ['ui 的錯誤訊息改成中文'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    expect: (r) => (/這一輪只重做/.test(r.prompts.revise || '') ? ['上一輪的子任務對不到子計劃時不該縮小範圍'] : []),
  },
  ...[
    ['重做的子計劃少了子任務又沒說明', [INT1, sub('ui-t9', 'ui', ['apps/z.tsx'], [])], /修正後少了子任務 ui-t1，open_questions 沒有說明/],
    ['交回了沿用的子計劃的子任務', [UI1, INT1, sub('api-t1', 'api', ['services/a.py'], [])], /交回了不在這一輪範圍的子任務 api-t1/],
    ['重做的子計劃沒有交回任何子任務', [INT1], /子計劃 ui 沒有交回任何子任務/],
    ['和沿用的子任務搶同一個檔', [{ ...UI1, owns: ['services/a.py'] }, INT1], /同時擁有 services\/a\.py/],
    ['@<子計劃> 查不到', [{ ...UI1, depends_on: ['@nope'] }, INT1], /相依 @nope 查不到這個子計劃/],
    ['依賴不存在的子任務', [{ ...UI1, depends_on: ['api-t9'] }, INT1], /子任務 ui-t1 依賴不存在的 api-t9/],
    ['相依有循環', [{ ...UI1, depends_on: ['int-t1'] }, INT1], /子任務相依有循環/],
  ].map(([what, subtasks, re]) => ({
    name: `只重做受影響的子計劃：${what} → 停下交人，不派複查`,
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, decisions: ['ui 的錯誤訊息改成中文'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    fakes: { revise: ENV({ subtasks }) },
    expect: (r) => [
      ...(r.out && r.out.halted && re.test(r.out.halt_reason || '') ? [] : [`應該停下並說明：${JSON.stringify(r.out && r.out.halt_reason)}`]),
      ...(r.labels.some((l) => l.startsWith('recheck:')) ? ['清單不一致不該派複查'] : []),
      ...(Array.isArray(r.out && r.out.trace) ? [] : ['halted 也要帶 trace']),
    ],
  })),
  {
    name: '只重做受影響的子計劃：修正輪回 needs_decision（要動到沿用的子計劃）→ 停下交人',
    args: { sub_plans: SUB_PLANS, prev_subtasks: PREV, decisions: ['ui 的錯誤訊息改成中文'], issues: [{ target_id: 'ui-t1', reason: '缺驗收條件' }] },
    fakes: { revise: ENV({ subtasks: [UI1, INT1] }, { status: 'needs_decision', open_questions: ['api 也要改'] }) },
    expect: (r) => (r.out && r.out.halted && /needs_decision/.test(r.out.halt_reason || '') && /revise_all/.test(r.out.next || '') ? [] : ['needs_decision 要停下並提示 revise_all']),
  },
]
