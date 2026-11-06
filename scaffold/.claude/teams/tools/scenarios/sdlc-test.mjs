// sdlc-test.js 的乾跑情境：走預設乾跑碰不到的分支。
import { CRIT, SUB_PLANS, crit, extScenario, recipeScenarios } from './_helpers.mjs'

// 三個分片都驗 abc（CRIT 一律填 abc）、彙整照抄：B-1／B-5 的情境以它為基礎只改一處
const SHARD1 = {
  'verify:api#1': CRIT([crit('A1', 'fail')]),
  'verify:ui#1': CRIT([crit('U1', 'pass')]),
  'verify:integration#1': CRIT([crit('I1', 'pass')]),
  'consolidate#1': CRIT([crit('api:A1', 'fail'), crit('ui:U1', 'pass'), crit('integration:I1', 'pass')]),
}
// 把 CRIT 假回傳的 commit 換掉
const atCommit = (f, commit) => (base) => { const r = f(base); return { ...r, payload: { ...r.payload, commit } } }

export default [
    extScenario('ext:db3'),
    {
      name: '分片驗證＋平行修正',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('api:A1', 'fail'), crit('ui:U1', 'fail'), crit('integration:I1', 'pass')]),
        'verify:api#2': CRIT([crit('A1', 'pass')]),
        'verify:ui#2': CRIT([crit('U1', 'pass')]),
        'verify:integration#2': CRIT([crit('I1', 'pass')]),
        'consolidate#2': CRIT([crit('api:A1', 'pass'), crit('ui:U1', 'pass'), crit('integration:I1', 'pass')]),
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
      // 三個分片都驗同一個 commit（CRIT 的 abc），彙整也填 abc：只有「結果被改」這個問題，斷言要寫出它
      fakes: {
        'verify:api#1': CRIT([crit('A1', 'fail')]),
        'verify:ui#1': CRIT([crit('U1', 'pass')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('api:A1', 'pass'), crit('ui:U1', 'pass'), crit('integration:I1', 'pass')]),
      },
      expect: (r) => {
        const why = (r.out && r.out.halt_reason) || ''
        return [
          ...(r.out && r.out.halted ? [] : ['彙整把 fail 改成 pass 卻沒有停下']),
          ...(/api:A1 的結果被改了/.test(why) ? [] : [`halt_reason 要寫出 api:A1 的結果被改了：${why}`]),
          ...(/HEAD 不一致|commit/.test(why) ? [`各分片與彙整的 commit 一致，halt_reason 不該提 HEAD／commit：${why}`] : []),
        ]
      },
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
        'consolidate#1': CRIT([crit('api:A1', 'fail'), crit('ui:U1', 'fail'), crit('integration:I1', 'pass')]),
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
        'consolidate#1': CRIT([crit('api:A1', 'fail'), crit('ui:U1', 'fail'), crit('integration:I1', 'pass')]),
        'integrate:fix#1': () => null,
      },
      expect: (r) => (r.out && r.out.halted && (r.out.left_worktrees || []).length === 2 ? [] : ['平行修正整合缺件要 halted 並列出 worktree']),
    },
    {
      // B-5：分片驗的 HEAD 不一致（ui 驗的是另一個 commit），彙整再怎麼照抄都不能用
      name: '分片 HEAD 不一致要停下',
      args: { sub_plans: SUB_PLANS },
      fakes: { ...SHARD1, 'verify:ui#1': atCommit(CRIT([crit('U1', 'pass')]), 'def') },
      expect: (r) => (r.out && r.out.halted && /HEAD 不一致/.test(r.out.halt_reason || '') ? [] : [`分片 HEAD 不一致要 halted 並寫出：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      // B-1：彙整填的 commit 和分片共同的 HEAD 不同（sdlc-merge 的 --reviewed-sha 取自它），不能放行
      name: '彙整 commit 與分片不同要停下',
      args: { sub_plans: SUB_PLANS },
      fakes: { ...SHARD1, 'consolidate#1': atCommit(CRIT([crit('api:A1', 'fail'), crit('ui:U1', 'pass'), crit('integration:I1', 'pass')]), 'zzz') },
      expect: (r) => (r.out && r.out.halted && /彙整的 commit（zzz）/.test(r.out.halt_reason || '') ? [] : [`彙整 commit 與分片不同要 halted 並寫出：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      // B-2：兩個子計劃都有 AC-1（api 的 fail、ui 的 pass），不能誤判成「結果被改了」
      name: '跨子計劃撞號：不誤判竄改',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('AC-1', 'fail')]),
        'verify:ui#1': CRIT([crit('AC-1', 'pass')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('api:AC-1', 'fail'), crit('ui:AC-1', 'pass'), crit('integration:I1', 'pass')]),
        'verify:api#2': CRIT([crit('AC-1', 'pass')]),
        'verify:ui#2': CRIT([crit('AC-1', 'pass')]),
        'verify:integration#2': CRIT([crit('I1', 'pass')]),
        'consolidate#2': CRIT([crit('api:AC-1', 'pass'), crit('ui:AC-1', 'pass'), crit('integration:I1', 'pass')]),
      },
      expect: (r) => [
        ...(r.out && !r.out.halted ? [] : [`撞號不該停下：${r.out && r.out.halt_reason}`]),
        ...(r.labels.includes('fix#1') ? [] : [`只有 api 失敗，應走單一修正 fix#1：${r.labels.join(',')}`]),
        ...(/api:AC-1/.test(r.prompts['fix#1'] || '') && !/ui:AC-1/.test(r.prompts['fix#1'] || '') ? [] : ['fix#1 的失敗清單應只有 api:AC-1']),
      ],
    },
    {
      // B-2：兩個子計劃的 AC-1 都失敗，要各自分到自己的子計劃平行修（owns 不能分錯）
      name: '跨子計劃撞號：分對 owns',
      args: { sub_plans: SUB_PLANS },
      fakes: {
        'verify:api#1': CRIT([crit('AC-1', 'fail')]),
        'verify:ui#1': CRIT([crit('AC-1', 'fail')]),
        'verify:integration#1': CRIT([crit('I1', 'pass')]),
        'consolidate#1': CRIT([crit('api:AC-1', 'fail'), crit('ui:AC-1', 'fail'), crit('integration:I1', 'pass')]),
        'verify:api#2': CRIT([crit('AC-1', 'pass')]),
        'verify:ui#2': CRIT([crit('AC-1', 'pass')]),
        'verify:integration#2': CRIT([crit('I1', 'pass')]),
        'consolidate#2': CRIT([crit('api:AC-1', 'pass'), crit('ui:AC-1', 'pass'), crit('integration:I1', 'pass')]),
      },
      expect: (r) => {
        const api = r.prompts['fix:api#1'] || ''
        const ui = r.prompts['fix:ui#1'] || ''
        return [
          ...(r.out && !r.out.halted ? [] : [`不該停下：${r.out && r.out.halt_reason}`]),
          ...(api && ui ? [] : [`兩個子計劃都失敗應平行修：${r.labels.join(',')}`]),
          ...(/api:AC-1/.test(api) && !/ui:AC-1/.test(api) && /services\//.test(api) ? [] : ['fix:api#1 應只拿到 api:AC-1，範圍是 services/']),
          ...(/ui:AC-1/.test(ui) && !/api:AC-1/.test(ui) && /apps\//.test(ui) ? [] : ['fix:ui#1 應只拿到 ui:AC-1，範圍是 apps/']),
        ]
      },
    },
    {
      // B-8：verifier throw（逾時、被中止）要走缺件流程，不能讓整支 workflow 打斷
      name: '缺件：verifier throw',
      fakes: { 'verify#1': 'THROW' },
      expect: (r) => (r.out && r.out.halted && (r.out.missing || []).includes('verify#1') ? [] : [`verify#1 throw 要 halted 並列進 missing：${JSON.stringify(r.out && { halted: r.out.halted, missing: r.out.missing })}`]),
    },
      // recipe：設計／安全審查依 recipe.test、verifier 依 modes 加回歸／等價／基線；沒有 design 段時驗收條件來源是 intent.md
    ...recipeScenarios('test', (n) => ({
      args: { has_ui: false, has_security: false },
      expect: (r) => {
        const e = []
        const v = Object.entries(r.prompts).filter(([l]) => l.startsWith('verify')).map(([, p]) => p).join('\n')
        const sec = n === 'deps'
        if (sec !== r.labels.includes('security-review')) e.push(`${n}（has_security=false）${sec ? '一定要' : '不該'}派 security-review：${r.labels.join(',')}`)
        if (r.labels.includes('design-review')) e.push(`${n}（has_ui=false）不該派 design-review`)
        const modes = { bugfix: '回歸', change: '回歸', hotfix: '回歸', deps: '回歸', revert: '回歸', refactor: '等價', perf: '基線' }
        if (modes[n] && !v.includes(modes[n])) e.push(`${n} 的驗證要加「${modes[n]}」`)
        if (!modes[n] && /加跑受影響舊驗收條件|characterization|量測基線/.test(v)) e.push(`${n} 不該加額外驗證模式`)
        const design = ['feature', 'change', 'perf'].includes(n)
        if (design ? !/spec\.md/.test(v) : (!/intent\.md/.test(v) || /spec\.md/.test(v))) e.push(`${n} 的驗收條件來源應為 ${design ? 'spec.md' : 'intent.md（不提 spec.md）'}`)
        if (!/run_brief\.py/.test(v)) e.push('verifier 派工單要求用 run_brief.py')
        return e
      },
    })),
    {
      name: 'recipe:bugfix+ui',
      args: { recipe: 'bugfix', modifiers: ['ui'], has_ui: false, has_security: false },
      expect: (r) => (r.labels.includes('design-review') && !r.out.halted ? [] : [`ui modifier 在 has_ui=false 時也要派 design-review：${r.labels.join(',')}`]),
    },
    {
      name: 'recipe:test-only 有畫面也不做設計審查',
      args: { recipe: 'test-only', has_ui: true, has_security: true },
      expect: (r) => (!r.labels.includes('design-review') && r.labels.includes('security-review') ? [] : [`test-only：design_review=never、security_review=auto：${r.labels.join(',')}`]),
    },
]
