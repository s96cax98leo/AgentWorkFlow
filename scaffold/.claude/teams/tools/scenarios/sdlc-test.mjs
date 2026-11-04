// sdlc-test.js 的乾跑情境：走預設乾跑碰不到的分支。
import { CRIT, SUB_PLANS, crit, extScenario } from './_helpers.mjs'

export default [
    extScenario('ext:db3'),
    {
      name: '分片驗證＋平行修正',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('A1', 'fail'), crit('U1', 'fail'), crit('I1', 'pass')]),
        'verify:api#2': CRIT([crit('A1', 'pass')]),
        'verify:ui#2': CRIT([crit('U1', 'pass')]),
        'verify:integration#2': CRIT([crit('I1', 'pass')]),
        'consolidate#2': CRIT([crit('A1', 'pass'), crit('U1', 'pass'), crit('I1', 'pass')]),
      },
      expect: (r) => [
        ...(['verify:api#1', 'verify:ui#1', 'verify:integration#1', 'consolidate#1', 'fix:api#1', 'fix:ui#1', 'integrate:fix#1', 'consolidate#2'].every((l) => r.labels.includes(l)) ? [] : [`沒有走完分片與平行修正：${r.labels.join(',')}`]),
        ...(r.out && r.out.halted ? [`不該中斷：${r.out.halt_reason}`] : []),
        ...(r.out && r.out.summary && r.out.summary.pass === 3 ? [] : ['第 2 輪後應該全部 pass']),
      ],
    },
    {
      name: '彙整竄改結果要停下',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'consolidate#1': CRIT([crit('A1', 'pass')]),
      },
      expect: (r) => (r.out && r.out.halted ? [] : ['彙整把 fail 改成 pass 卻沒有停下']),
    },
  ]
