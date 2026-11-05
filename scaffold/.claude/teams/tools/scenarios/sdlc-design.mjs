// sdlc-design.js 的乾跑情境：走預設乾跑碰不到的分支。
import { ENV, SUB_PLANS, extScenario } from './_helpers.mjs'

export default [
    extScenario('ext:db2'),
    {
      name: 'profile 有系統設計方法',
      profile: { stages: { design: { artifacts: ['design.md', 'spec.md', 'plan.md'], requirement_style: 'x', guidance: 'x',
        system_design: { source: 'System Design Primer', steps: ['1 用例、限制與假設', '4 擴展與取捨'], feature_scope: '功能層要寫', system_scope: '系統層要寫', topics: ['CAP'] } } } },
      fakes: { design: (base, prompt) => { if (!/系統設計（SA／SD/.test(prompt) || !/4 擴展與取捨/.test(prompt)) throw new Error('design 派工單沒帶系統設計方法'); return ENV(base.payload)(base) } },
      expect: (r) => (r.labels.includes('analyze:system-design') ? [] : [`沒有派出 system-design lens：${r.labels.join(',')}`]),
    },
    {
      name: '拆成子計劃',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: '預估 12 個子任務', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['@api'], acceptance: ['x'] }] }),
        consolidate: (base, prompt) => {
          if (!prompt.includes('"depends_on":["api-t1"]')) throw new Error('@api 相依沒有展開成 api-t1')
          return ENV({ epic: 'demo', subtasks: [
            { id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'], sub_plan: 'api' },
            { id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['api-t1'], acceptance: ['x'], sub_plan: 'ui' },
          ] })(base)
        },
      },
      expect: (r) => [
        ...(['subplan:api', 'subplan:ui', 'consolidate', 'analyze:sub-plans'].every((l) => r.labels.includes(l)) ? [] : [`沒有走完子計劃流程：${r.labels.join(',')}`]),
        ...(r.out && r.out.halted ? [`不該中斷：${r.out.halt_reason}`] : []),
      ],
    },
    {
      name: '子計劃範圍重疊要停下',
      fakes: { outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: [SUB_PLANS[0], { ...SUB_PLANS[1], owns: ['services/web/'] }] }) },
      expect: (r) => (r.out && r.out.halted && !r.labels.some((l) => l.startsWith('subplan:')) ? [] : ['範圍重疊卻沒有在平行規劃前停下']),
    },
    {
      name: '彙整掉了子任務要停下',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: [], acceptance: ['x'] }] }),
        consolidate: ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
      },
      expect: (r) => (r.out && r.out.halted ? [] : ['彙整少了 ui-t1 卻沒有停下']),
    },
    {
      name: '缺件：design 沒交件',
      fakes: { design: () => null },
      expect: (r) => [
        ...(r.out && r.out.halted && /design/.test(r.out.halt_reason || '') && (r.out.missing || []).includes('design') ? [] : [`design 缺件要 halted：${JSON.stringify(r.out && { halted: r.out.halted, missing: r.out.missing })}`]),
        ...(r.labels.some((l) => l.startsWith('analyze:')) ? ['沒有設計不該派一致性檢查'] : []),
      ],
    },
    {
      name: '缺件：一致性檢查沒交件',
      fakes: { 'analyze:plan': () => null },
      expect: (r) => (r.out && r.out.halted && /analyze:plan/.test(r.out.halt_reason || '') ? [] : ['analyze:plan 缺件要 halted 並寫出 label']),
    },
    {
      name: '缺件：彙整沒交件',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'a', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'b', kind: 'code', owns: ['apps/b.tsx'], depends_on: [], acceptance: ['x'] }] }),
        consolidate: () => null,
      },
      expect: (r) => (r.out && r.out.halted && /consolidate/.test(r.out.halt_reason || '') ? [] : ['consolidate 缺件要 halted']),
    },
    {
      name: 'profile 設 pr_size：第一輪就規劃拆 PR',
      profile: { conventions: { pr_size: { max_lines: 400, max_files: 20, scope: 'git diff --numstat' } } },
      expect: (r) => (/PR 大小[\s\S]*400 行[\s\S]*拆 PR/.test(r.prompts.design || '') ? [] : ['design 派工單沒帶 profile 的 PR 大小與拆 PR 規劃']),
    },
    {
      // B-7：@<子計劃> 查不到該子計劃：不能默默變成沒有相依
      name: '子計劃相依查不到要停下',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'x', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'x', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['@nope'], acceptance: ['x'] }] }),
      },
      expect: (r) => (r.out && r.out.halted && /@nope/.test(r.out.halt_reason || '') && !r.labels.includes('consolidate') ? [] : [`@nope 查不到要在彙整前停下：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      // B-7：@<子計劃> 指向的子計劃沒有子任務：展開後是空的
      name: '子計劃相依展開為空要停下',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'x', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['@api'], acceptance: ['x'] }] }),
      },
      expect: (r) => (r.out && r.out.halted && /@api/.test(r.out.halt_reason || '') && /空/.test(r.out.halt_reason || '') && !r.labels.includes('consolidate') ? [] : [`@api 展開為空要停下：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      // B-7：@<子計劃> 指向自己所在的子計劃
      name: '子計劃相依指向自己要停下',
      fakes: {
        outline: ENV({ epic: 'demo', split: true, reason: 'x', sub_plans: SUB_PLANS }),
        'subplan:api': ENV({ epic: 'demo', subtasks: [{ id: 'api-t1', title: 'x', kind: 'code', owns: ['services/a.py'], depends_on: ['@api'], acceptance: ['x'] }] }),
        'subplan:ui': ENV({ epic: 'demo', subtasks: [{ id: 'ui-t1', title: 'x', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['@api'], acceptance: ['x'] }] }),
      },
      expect: (r) => (r.out && r.out.halted && /指向自己/.test(r.out.halt_reason || '') && !r.labels.includes('consolidate') ? [] : [`@api 指向自己要停下：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
]
