// sdlc-build.js 的乾跑情境：走預設乾跑碰不到的分支。

import { FIX_SUBTASKS, SAMPLE_SUBTASKS, recipeScenarios } from './_helpers.mjs'

const NONE = () => null
const out = (r) => r.out || {}
// 缺件情境不覆寫 profile：smoke_init 會用乾跑經過的 phases 檢查 test_first 開關，斷言要在兩種順序下都成立

export default [
  {
    // 波次（0.5）：同一波做完、合進整合分支才做下一波；波次裡面照相依分層
    name: '波次：t1 第 1 波、t2 與 t3 第 2 波 → t1 先做完並整合，才做 t2，再做 t3；回傳帶 waves',
    args: { subtasks: SAMPLE_SUBTASKS().map((s) => ({ ...s, ...(s.id === 't1' ? { wave: 1 } : s.kind === 'code' ? { wave: 2 } : {}) })) },
    expect: (r) => {
      const errs = []
      const at = (l) => r.labels.indexOf(l)
      if (out(r).halted) errs.push(`不該 halted：${out(r).halt_reason}`)
      if (!(at('Implement:t1') >= 0 && at('Implement:t1') < at('integrate:layer1') && at('integrate:layer1') < at('Implement:t2') && at('Implement:t2') < at('Implement:t3'))) errs.push(`順序不對：${r.labels.join(' → ')}`)
      if (JSON.stringify(out(r).waves) !== '[1,2]') errs.push(`回傳要帶 waves：${JSON.stringify(out(r).waves)}`)
      if ((out(r).layers || []).length !== 3) errs.push(`應該分 3 層（第 1 波 1 層、第 2 波 2 層）：${(out(r).layers || []).length}`)
      return errs
    },
  },
  {
    name: '波次：wave_pause 開啟時第 1 波做完停下交人，回傳 remaining_subtasks 與 integrated 供續跑',
    profile: { parallelism: { wave_pause: true } },
    args: { subtasks: SAMPLE_SUBTASKS().map((s) => ({ ...s, ...(s.id === 't1' ? { wave: 1 } : s.kind === 'code' ? { wave: 2 } : {}) })) },
    expect: (r) => {
      const errs = []
      const o = out(r)
      const rem = (o.remaining_subtasks || []).map((s) => s.id)
      if (!o.halted || !o.wave_pause || o.wave_done !== 1 || !/第 1 波做完/.test(o.halt_reason || '')) errs.push(`第 1 波做完要停下：${JSON.stringify({ halted: o.halted, wave_done: o.wave_done, why: o.halt_reason })}`)
      if (r.labels.includes('Implement:t2') || r.labels.includes('Implement:t3')) errs.push(`第 2 波不該開工：${r.labels.join(' → ')}`)
      if (!rem.includes('t2') || !rem.includes('t3') || rem.includes('t1') || !(o.integrated || []).includes('t1')) errs.push(`續跑資料不對：remaining=${rem}、integrated=${o.integrated}`)
      if ((o.missing || []).length || (o.failures || []).length) errs.push(`這不是缺件，也沒有失敗：${JSON.stringify([o.missing, o.failures])}`)
      return errs
    },
  },
  {
    name: '波次：續跑只剩最後一波時一路做完，不再停',
    profile: { parallelism: { wave_pause: true } },
    args: { subtasks: SAMPLE_SUBTASKS().filter((s) => s.id === 't2' || s.id === 't3').map((s) => ({ ...s, wave: 2 })), integrated: ['t0', 't1'] },
    expect: (r) => (!out(r).halted && r.labels.includes('Implement:t2') && r.labels.includes('Implement:t3') && !r.labels.includes('Implement:t1') ? [] : [`續跑要做完剩下的波次：${JSON.stringify({ halted: out(r).halted, why: out(r).halt_reason, labels: r.labels })}`]),
  },
  {
    name: '波次：依賴比自己晚的波次 → 開工前停下，不派任何 agent',
    args: { subtasks: SAMPLE_SUBTASKS().map((s) => ({ ...s, ...(s.id === 't1' ? { wave: 2 } : s.id === 't3' ? { wave: 1 } : {}) })) },
    expect: (r) => (out(r).halted && /比自己晚的波次/.test(out(r).halt_reason || '') && (out(r).bad_waves || []).length === 1 && r.calls.length === 0 ? [] : [`要開工前停下：${JSON.stringify({ why: out(r).halt_reason, bad: out(r).bad_waves, calls: r.calls.length })}`]),
  },
  {
    // 執行核心（0.5）：子任務的 complexity 是建議的模型等級，trace 每筆都有 model 與 tier
    name: '執行核心：子任務標 complexity=strong 時 implementer 用 strong，其他照預設；trace 都有 tier',
    profile: { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'standard' }, floors: {} } },
    args: { subtasks: SAMPLE_SUBTASKS().map((s) => (s.id === 't1' ? { ...s, complexity: 'strong', complexity_reason: '跨模組' } : s)) },
    expect: (r) => {
      const errs = []
      const tr = out(r).trace || []
      const of = (label) => tr.find((t) => t.label === label) || {}
      if (of('Implement:t1').tier !== 'strong' || of('Implement:t1').model !== 'opus') errs.push(`t1 應該用 strong／opus：${JSON.stringify(of('Implement:t1'))}`)
      if (of('Implement:t2').tier !== 'standard' || of('Implement:t2').model !== 'sonnet') errs.push(`t2 應該用 standard／sonnet：${JSON.stringify(of('Implement:t2'))}`)
      if (tr.some((t) => !t.tier)) errs.push(`trace 有沒有 tier 的項目：${tr.filter((t) => !t.tier).map((t) => t.label).join('、')}`)
      if (!r.calls.some((c) => c.label === 'Implement:t1' && c.model === 'opus')) errs.push('派 t1 的時候沒有帶 model=opus')
      return errs
    },
  },
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
    // 限制文件（profile 的 constraints）：有 paths 的只給擁有的路徑對得上的子任務；沒有 paths 的整段都帶（共同前綴）
    name: '限制文件：前端規範只發給前端子任務，整段適用的每張派工單都有',
    profile: { constraints: [
      { id: 'arch', file: 'ARCHITECTURE.md', note: '分層', stages: ['design', 'build'], trace: 'table' },
      { id: 'frontend', file: 'docs/fe.md', stages: ['build'], paths: ['apps/**'] },
      { id: 'backend', file: 'docs/be.md', stages: ['build'], paths: ['services/**'] },
      { id: 'review', file: 'REVIEW.md', stages: ['deliver'] },
    ] },
    expect: (r) => {
      const errs = []
      const be = r.prompts['Implement:t1'] || ''
      const fe = r.prompts['Implement:t2'] || ''
      if (!/\[backend\] docs\/be\.md/.test(be) || /\[frontend\]/.test(be)) errs.push('後端子任務要收到後端規範、不能收到前端規範')
      if (!/\[frontend\] docs\/fe\.md/.test(fe) || /\[backend\]/.test(fe)) errs.push('前端子任務要收到前端規範、不能收到後端規範')
      const all = Object.entries(r.prompts).filter(([, v]) => v)
      const noArch = all.filter(([, v]) => !/\[arch\] ARCHITECTURE\.md：分層（要對照）/.test(v)).map(([k]) => k)
      if (noArch.length) errs.push(`整段適用的限制每張派工單都要有：${noArch.join(',')}`)
      if (all.some(([, v]) => /\[review\]/.test(v))) errs.push('只套用到 deliver 的限制不該出現在 build')
      if (out(r).halted) errs.push(`不該中斷：${out(r).halt_reason}`)
      return errs
    },
  },
  {
    // 子任務資料包（pack.py build）：派工單引用自己那一份；路徑不對的不採用
    name: '子任務資料包：派工單引用自己的資料包，路徑不對的不採用',
    args: { task_packs: { t1: '_runs/demo/build/t1/pack.md', t2: '_runs/demo/build/../../secrets/pack.md', t3: '_runs/demo/build/t1/pack.md' } },
    expect: (r) => {
      const errs = []
      const all = Object.entries(r.prompts).filter(([, v]) => v)
      const t1 = all.filter(([k]) => /:t1(#|$)/.test(k))
      if (!t1.length || t1.some(([, v]) => !/子任務資料包（開工先讀這份[^\n]*：_runs\/demo\/build\/t1\/pack\.md/.test(v))) errs.push('t1 的派工單要引用自己的資料包')
      if (all.some(([k, v]) => !/:t1(#|$)/.test(k) && /子任務資料包（開工先讀/.test(v))) errs.push('路徑含 .. 或不是自己那一份的資料包不該被引用')
      if (out(r).halted) errs.push(`不該中斷：${out(r).halt_reason}`)
      return errs
    },
  },
  {
    name: '子任務資料包：spec_refs 對不到 spec 就停下，不派任何 agent',
    args: { spec_ref_errors: ['t1：spec.md#不存在的小節：spec.md 裡沒有這個標題'] },
    expect: (r) => {
      const errs = []
      if (!out(r).halted || !/spec_refs 對不到 spec/.test(out(r).halt_reason || '')) errs.push(`應該停下：${JSON.stringify(out(r).halt_reason)}`)
      if (Object.values(r.prompts).some((v) => v)) errs.push('停下前不該派工')
      if (!Array.isArray(out(r).trace)) errs.push('halted 也要帶 trace')
      return errs
    },
  },
  {
    // 預期內的紅：負責的子任務還沒合進來時，整合派工單列出來（不當成失敗）；合進來的那一層之後不再列
    name: '預期內的紅：等後面的子任務的檢查不算整合失敗，合進來之後照常擋',
    args: { subtasks: [
      { id: 't1', title: '後端', kind: 'code', owns: ['services/a.py'], depends_on: [], acceptance: ['pytest a'] },
      { id: 't2', title: '換截圖', kind: 'code', owns: ['apps/web/a.tsx'], depends_on: ['t1'], acceptance: ['vitest a'], expected_red: ['截圖比對 shell.png'] },
    ] },
    expect: (r) => {
      const errs = []
      const l1 = r.prompts['integrate:layer1'] || ''
      const l2 = r.prompts['integrate:layer2'] || ''
      if (!/截圖比對 shell\.png（等子任務 t2）/.test(l1) || !/不算失敗/.test(l1)) errs.push('t2 還沒合進來時，layer1 的整合派工單要列出預期內的紅')
      if (/截圖比對 shell\.png/.test(l2)) errs.push('t2 合進來的那一層不能再把它當成預期內的紅')
      if (out(r).halted) errs.push(`不該中斷：${out(r).halt_reason}`)
      return errs
    },
  },
  {
    // profile 的 design 產出文件沒列 progress.md：build 不要求看它、更新它
    name: 'progress.md：profile 的產出文件沒列就不要求',
    // 只換 design 的產出文件，其餘照專案的 profile（不動 test_first：smoke_init 用乾跑經過的 phases 檢查它）
    profile: (p) => ({ stages: { ...(p.stages || {}), design: { ...((p.stages || {}).design || {}), artifacts: ['design.md', 'spec.md', 'plan.md'] } } }),
    expect: (r) => {
      const hit = Object.entries(r.prompts).filter(([, v]) => /progress\.md/.test(v || '')).map(([k]) => k)
      return hit.length ? [`這些派工單還在提 progress.md：${hit.join('、')}`] : []
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
