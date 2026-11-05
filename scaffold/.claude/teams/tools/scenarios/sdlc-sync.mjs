// sdlc-sync.js 的乾跑情境：走預設乾跑碰不到的分支。
import { SYNC_APPLY, SYNC_PLAN, TRANSIT } from './_helpers.mjs'

// merge／rebase 留在原分支：head_branch 取派工單的 --branch（分支前綴依 init 設定，不寫死）
const ON_BRANCH = (sync) => (base, prompt) => SYNC_APPLY({ ...sync, head_branch: (String(prompt).match(/--branch (\S+)/) || [])[1] })(base)

export default [
    {
      name: '已跟上',
      fakes: { plan: SYNC_PLAN({ strategy: 'none', behind: 0, conflicts: [] }) },
      expect: (r) => (r.out && r.out.strategy === 'none' && !r.out.halted && !r.labels.some((l) => l.startsWith('apply')) ? [] : [`已跟上不該執行：${JSON.stringify(r.out && { strategy: r.out.strategy, halted: r.out.halted })}`]),
    },
    {
      name: '已推送、沒衝突：merge',
      fakes: { plan: SYNC_PLAN({ strategy: 'merge', behind: 2, published: true, conflicts: [] }), 'apply:merge': ON_BRANCH({ strategy: 'merge', conflicts: [], head_sha: 'a'.repeat(40) }) },
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
    {
      name: '缺件：執行沒交件',
      fakes: { plan: SYNC_PLAN({ strategy: 'merge', behind: 2, published: true, conflicts: [] }), 'apply:merge': () => null },
      expect: (r) => (r.out && r.out.halted && (r.out.missing || []).includes('apply:merge') ? [] : ['apply 缺件要 halted 並列出 label']),
    },
    {
      // 合併段不讀 branch：走過中轉分支後要指向新 PR 編號，不能叫 sdlc-merge 帶 branch
      name: '中轉分支的下一步：合併段用新 PR 編號',
      args: { pr: 7 },
      fakes: { plan: SYNC_PLAN(TRANSIT), 'apply:transit': SYNC_APPLY({ ...TRANSIT, head_branch: TRANSIT.transit_branch, head_sha: 'b'.repeat(40) }, [{ file: 'src/a.ts', kind: 'seam', resolution: 'x' }]) },
      expect: (r) => {
        const n = (r.out && r.out.next) || ''
        return [
          ...(/sdlc-deliver/.test(n) && /branch=merge\//.test(n) ? [] : [`next 要叫 sdlc-test、sdlc-deliver 帶 branch：${n}`]),
          ...(/--supersedes 7/.test(n) && /新 PR 的編號/.test(n) ? [] : [`next 要說合併段用新 PR 編號：${n}`]),
          ...(/sdlc-merge[^。；]*branch/.test(n) ? [`next 不能叫 sdlc-merge 帶 branch：${n}`] : []),
        ]
      },
    },
    {
      // B-9：plan 判 transit 卻沒有中轉分支名：不能執行
      name: 'transit 缺中轉分支名要停下',
      fakes: { plan: SYNC_PLAN({ ...TRANSIT, transit_branch: undefined }) },
      expect: (r) => (r.out && r.out.halted && /transit_branch/.test(r.out.halt_reason || '') && !r.labels.some((l) => l.startsWith('apply')) ? [] : [`缺 transit_branch 要在 apply 前停下：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
    {
      // B-9：merge／rebase 都在原分支上做，回報的 head_branch 換了分支就要停下
      name: 'merge 回報的 head_branch 不是原分支要停下',
      fakes: { plan: SYNC_PLAN({ strategy: 'merge', behind: 2, published: true, conflicts: [] }), 'apply:merge': SYNC_APPLY({ strategy: 'merge', conflicts: [], head_branch: 'feature/other', head_sha: 'a'.repeat(40) }) },
      expect: (r) => (r.out && r.out.halted && /feature\/other/.test(r.out.halt_reason || '') ? [] : [`head_branch 不是原分支要 halted：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
    },
]
