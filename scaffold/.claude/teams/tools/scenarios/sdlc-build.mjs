// sdlc-build.js 的乾跑情境：走預設乾跑碰不到的分支。

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
]
