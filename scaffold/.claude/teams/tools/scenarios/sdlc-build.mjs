// sdlc-build.js 的乾跑情境：走預設乾跑碰不到的分支。

import { FIX_SUBTASKS, SAMPLE_SUBTASKS, recipeScenarios } from './_helpers.mjs'

const NONE = () => null
const out = (r) => r.out || {}
// 缺件情境不覆寫 profile：smoke_init 會用乾跑經過的 phases 檢查 test_first 開關，斷言要在兩種順序下都成立

export default [
  {
    // 走過中轉分支後 merge 退回 build：子任務分支要從中轉分支開、integrator 要合回中轉分支
    name: 'branch：args.branch 帶中轉分支',
    args: { branch: 'merge/demo-transit' },
    expect: (r) => {
      const errs = []
      const impl = r.prompts['Implement:t1'] || ''
      if (!/worktree add \S+ -b \S+ merge\/demo-transit/.test(impl)) errs.push('Implement 派工單的子任務分支沒有從 merge/demo-transit 開')
      const integ = r.prompts['integrate:layer1'] || ''
      if (!/merge --no-ff 進 merge\/demo-transit/.test(integ)) errs.push('integrator 沒有合回 merge/demo-transit')
      if (out(r).branch !== 'merge/demo-transit') errs.push(`回傳的 branch 應為 merge/demo-transit：${out(r).branch}`)
      if (out(r).halted) errs.push(`不該中斷：${out(r).halt_reason}`)
      return errs
    },
  },
  {
    name: '缺件：實作子任務沒交件',
    fakes: { 'Implement:t2': NONE },
    expect: (r) => {
      const o = out(r)
      const errs = []
      if (!o.halted) errs.push('子任務缺件卻沒有 halted')
      if (!(o.missing || []).some((l) => l.includes('t2'))) errs.push(`missing 沒列出 t2：${JSON.stringify(o.missing)}`)
      const rem = (o.remaining_subtasks || []).map((s) => s.id)
      if (!rem.includes('t2') || !rem.includes('t3') || rem.includes('t1')) errs.push(`remaining_subtasks 應含 t2、t3，不含已合進去的 t1：${rem.join(',')}`)
      if (!/remaining_subtasks/.test(o.next || '')) errs.push(`next 沒說明怎麼續跑：${o.next}`)
      if (r.labels.some((l) => l === 'Implement:t3')) errs.push('缺件後不該進下一層')
      return errs
    },
  },
  {
    name: '缺件：integrator 沒交件',
    fakes: { 'integrate:layer1': NONE },
    expect: (r) => {
      const o = out(r)
      const rem = (o.remaining_subtasks || []).map((s) => s.id)
      return o.halted && (o.missing || []).includes('integrate:layer1') && rem.includes('t1') && rem.includes('t2') ? [] : [`integrator 缺件應 halted 並把沒合進去的 t1、t2 留在 remaining：${JSON.stringify({ halted: o.halted, missing: o.missing, rem })}`]
    },
  },
  {
    // 相依指向清單外的子任務：一律停下，不派任何實作
    name: '相依指向不存在的子任務：halted',
    args: { subtasks: [{ id: 't2', title: '前端', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['tX'], acceptance: ['vitest b'] }] },
    expect: (r) => {
      const o = out(r)
      const errs = []
      if (!o.halted || !/tX/.test(o.halt_reason || '')) errs.push(`應 halted 並寫出 tX：${JSON.stringify({ halted: o.halted, why: o.halt_reason })}`)
      if (r.labels.some((l) => l.startsWith('Implement:'))) errs.push('停下前不該派實作')
      return errs
    },
  },
  {
    // 續跑：只傳 remaining_subtasks，已合進去的 t1 用 args.integrated 列出，t3 才排得進去
    name: '續跑：args.integrated 列出已合進去的子任務',
    args: { subtasks: [{ id: 't3', title: '整合畫面', kind: 'code', owns: ['apps/c.tsx'], depends_on: ['t1', 't2'], acceptance: ['e2e c'] }], integrated: ['t1', 't2'] },
    expect: (r) => {
      const o = out(r)
      const errs = []
      if (o.halted) errs.push(`不該中斷：${o.halt_reason}`)
      if (!r.labels.includes('Implement:t3')) errs.push(`沒有派 t3：${r.labels.join(',')}`)
      return errs
    },
  },
  {
    // B-6：依賴不屬於 Build 的子任務（research）：明確 halted，不是循環、不 throw，回傳續跑資訊
    name: '依賴不屬於 Build 的子任務：halted',
    args: { subtasks: [
      { id: 'r1', title: '調研', kind: 'research', owns: [], depends_on: [], acceptance: ['x'] },
      { id: 't2', title: '前端', kind: 'code', owns: ['apps/b.tsx'], depends_on: ['r1'], acceptance: ['vitest b'] },
    ] },
    expect: (r) => {
      const o = out(r)
      const errs = []
      if (!o.halted || !/依賴不屬於 Build 的子任務/.test(o.halt_reason || '') || !/r1/.test(o.halt_reason || '')) errs.push(`應 halted 並寫出依賴不屬於 Build 的子任務 r1：${JSON.stringify({ halted: o.halted, why: o.halt_reason })}`)
      if (!Array.isArray(o.integrated) || !(o.remaining_subtasks || []).some((s) => s.id === 't2') || !o.branch || !o.next) errs.push(`回傳要帶 integrated、remaining_subtasks、branch、next：${JSON.stringify({ integrated: o.integrated, branch: o.branch, next: o.next })}`)
      if (r.labels.length) errs.push(`停下前不該派任何 agent：${r.labels.join(',')}`)
      return errs
    },
  },
  {
    // B-6：相依循環：halted（不 throw），回傳續跑資訊
    name: '相依循環：halted',
    args: { subtasks: [
      { id: 'c1', title: 'a', kind: 'code', owns: ['a.py'], depends_on: ['c2'], acceptance: ['x'] },
      { id: 'c2', title: 'b', kind: 'code', owns: ['b.py'], depends_on: ['c1'], acceptance: ['x'] },
    ] },
    expect: (r) => {
      const o = out(r)
      const errs = []
      if (!o.halted || !/循環/.test(o.halt_reason || '')) errs.push(`循環應 halted：${JSON.stringify({ halted: o.halted, why: o.halt_reason })}`)
      if (!Array.isArray(o.integrated) || (o.remaining_subtasks || []).length !== 2 || !o.branch || !o.next) errs.push(`回傳要帶 integrated、remaining_subtasks、branch、next：${JSON.stringify({ integrated: o.integrated, branch: o.branch, next: o.next })}`)
      if (r.labels.length) errs.push(`停下前不該派任何 agent：${r.labels.join(',')}`)
      return errs
    },
  },
  {
    // B-8：實作 agent throw：記成缺件、halted、留下續跑資訊
    name: '缺件：實作子任務 throw',
    fakes: { 'Implement:t2': 'THROW' },
    expect: (r) => {
      const o = out(r)
      const rem = (o.remaining_subtasks || []).map((s) => s.id)
      return o.halted && (o.missing || []).some((l) => l.includes('t2')) && rem.includes('t2') ? [] : [`實作 throw 要 halted、missing 列出 t2：${JSON.stringify({ halted: o.halted, missing: o.missing, rem })}`]
    },
  },
  // recipe：每個包含 build 的內建 recipe 一個情境；子任務種類要符合 build_kinds（docs 只有 doc、test-only 只有 test）
  ...recipeScenarios('build', (n) => ({
    args: n === 'bugfix' || n === 'hotfix' ? { subtasks: FIX_SUBTASKS() } : n === 'docs' ? { subtasks: [{ id: 't0', title: '改文件', kind: 'doc', owns: ['docs/a.md'], depends_on: [], acceptance: ['連結檢查通過'] }] } : n === 'test-only' ? { subtasks: SAMPLE_SUBTASKS('test') } : n === 'revert' ? { revert_commits: ['abc1234'] } : {},
    expect: n === 'revert'
      ? (r) => Object.entries(r.prompts).filter(([l]) => l.startsWith('Implement')).every(([, p]) => /git revert --no-edit abc1234/.test(p) && !/讓已合進整合分支的驗收測試轉綠/.test(p)) ? [] : ['revert 的實作派工單要只跑 git revert abc1234']
      : undefined,
  })),
  {
    name: 'recipe:docs 有 code 子任務',
    args: { recipe: 'docs' },
    expect: (r) => (r.out && r.out.halted && /kinds/.test(r.out.halt_reason || '') && !r.labels.length ? [] : [`docs 有 code 子任務要開工前 halted（kinds）：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason, labels: r.labels })}`]),
  },
  {
    name: 'recipe:test-only 有 code 子任務',
    args: { recipe: 'test-only' },
    expect: (r) => (r.out && r.out.halted && /kinds/.test(r.out.halt_reason || '') && !r.labels.length ? [] : [`test-only 有 code 子任務要開工前 halted（kinds）：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason, labels: r.labels })}`]),
  },
  {
    name: 'recipe:不認得的 modifier：halted',
    args: { recipe: 'bugfix', modifiers: ['nope'] },
    expect: (r) => (r.out && r.out.halted && /nope/.test(r.out.halt_reason || '') && !r.labels.length ? [] : [`不認得的 modifier 要 halted：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
]
