// sdlc-sync.js 的乾跑情境：走預設乾跑碰不到的分支。
import { SYNC_APPLY, SYNC_PLAN, TRANSIT } from './_helpers.mjs'

export default [
    {
      name: '已跟上',
      fakes: { plan: SYNC_PLAN({ strategy: 'none', behind: 0, conflicts: [] }) },
      expect: (r) => (r.out && r.out.strategy === 'none' && !r.out.halted && !r.labels.some((l) => l.startsWith('apply')) ? [] : [`已跟上不該執行：${JSON.stringify(r.out && { strategy: r.out.strategy, halted: r.out.halted })}`]),
    },
    {
      name: '已推送、沒衝突：merge',
      fakes: { plan: SYNC_PLAN({ strategy: 'merge', behind: 2, published: true, conflicts: [] }), 'apply:merge': SYNC_APPLY({ strategy: 'merge', conflicts: [], head_branch: 'feature/demo-999-sample', head_sha: 'a'.repeat(40) }) },
      expect: (r) => (r.out && r.out.strategy === 'merge' && !r.out.halted && /重跑 sdlc-test/.test(r.out.next || '') ? [] : [`merge 後要提示重跑 sdlc-test：${JSON.stringify(r.out && { halted: r.out.halted, next: r.out.next })}`]),
    },
    {
      name: '有衝突：transit',
      fakes: { plan: SYNC_PLAN(TRANSIT), 'apply:transit': SYNC_APPLY({ ...TRANSIT, head_branch: TRANSIT.transit_branch, head_sha: 'b'.repeat(40) }, [{ file: 'src/a.ts', kind: 'seam', resolution: 'x' }]) },
      expect: (r) => (r.out && !r.out.halted && r.out.head_branch === TRANSIT.transit_branch && /publish\.py/.test(r.out.next || '') && /--supersedes 1/.test(r.out.next || '') ? [] : [`transit 要換 head_branch 並提示開新 PR：${JSON.stringify(r.out && { halted: r.out.halted, head_branch: r.out.head_branch, next: r.out.next, why: r.out.halt_reason })}`]),
    },
    {
      name: '中轉分支多解了沒預測的檔',
      fakes: { plan: SYNC_PLAN(TRANSIT), 'apply:transit': SYNC_APPLY({ ...TRANSIT, head_branch: TRANSIT.transit_branch }, [{ file: 'src/a.ts', kind: 'seam', resolution: 'x' }, { file: 'src/other.ts', kind: 'mechanical', resolution: 'x' }]) },
      expect: (r) => (r.out && r.out.halted === true && /不一致/.test(r.out.halt_reason || '') ? [] : ['衝突清單和 plan 不一致時要 halted']),
    },
    {
      name: '固定 rebase 但已推送：工具停下',
      fakes: { plan: (base) => ({ ...SYNC_PLAN({ strategy: 'rebase', behind: 1, published: true, conflicts: [], halted: true, reason: '已推送的分支 rebase 要 force push' })(base), status: 'needs_decision' }) },
      expect: (r) => (r.out && r.out.halted === true && !r.labels.some((l) => l.startsWith('apply')) ? [] : ['plan halted 時不能執行']),
    },
  ]
