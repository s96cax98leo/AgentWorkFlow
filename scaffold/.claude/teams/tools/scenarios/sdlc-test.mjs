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
    {
      // has_security 但安全審查缺件：不能讓閘門以為「審過、沒有發現」
      name: '缺件：安全審查沒交件',
      fakes: { 'security-review': () => null },
      expect: (r) => (r.out && r.out.halted && /security-review/.test(r.out.halt_reason || '') && (r.out.missing || []).includes('security-review') ? [] : [`安全審查缺件要 halted 並列出 label：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      name: '缺件：修正沒交件',
      fakes: { 'verify#1': CRIT([crit('C1', 'fail')]), 'fix#1': () => null },
      expect: (r) => (r.out && r.out.halted && /fix#1/.test(r.out.halt_reason || '') ? [] : ['fix#1 缺件要 halted']),
    },
    {
      name: '平行修正有一組失敗',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('A1', 'fail'), crit('U1', 'fail'), crit('I1', 'pass')]),
        'fix:ui#1': (base) => ({ ...base, status: 'needs_decision', summary: '要改範圍外的檔' }),
      },
      expect: (r) => {
        const o = r.out || {}
        const left = o.left_worktrees || []
        return [
          ...(o.halted ? [] : ['平行修正有一組沒交 done 卻沒有 halted']),
          ...(left.length === 2 && left.every((x) => x.worktree && x.branch) && left.some((x) => x.group === 'ui' && x.result === 'needs_decision') ? [] : [`回傳要列出留下的 worktree 與分支：${JSON.stringify(left)}`]),
          ...(/worktree/.test(o.halt_reason || '') ? [] : ['halt_reason 要提到留下的 worktree']),
          ...(r.labels.includes('integrate:fix#1') ? ['有一組沒完成不該整合'] : []),
        ]
      },
    },
    {
      name: '平行修正整合失敗',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('A1', 'fail'), crit('U1', 'fail'), crit('I1', 'pass')]),
        'integrate:fix#1': () => null,
      },
      expect: (r) => (r.out && r.out.halted && (r.out.left_worktrees || []).length === 2 ? [] : ['平行修正整合缺件要 halted 並列出 worktree']),
    },
  ]
