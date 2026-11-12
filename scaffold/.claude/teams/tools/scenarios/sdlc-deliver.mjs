// sdlc-deliver.js 的乾跑情境：走預設乾跑碰不到的分支。
import { ENV, extScenario, recipeScenarios } from './_helpers.mjs'

export default [
  {
    // 執行核心（0.5）：派工數到上限就不再派，沒派出的在 trace 記 capped，workflow 照缺件停下交人（I8：不靜默截斷）
    name: '執行核心：派工數到上限（limits.max_agents=2）停下交人，trace 記 capped',
    profile: { limits: { max_agents: 2 } },
    expect: (r) => {
      const errs = []
      const o = r.out || {}
      const tr = o.trace || []
      const capped = tr.filter((t) => t.capped)
      if (!o.halted) errs.push('到上限要 halted')
      if (r.calls.length !== 2) errs.push(`只該派出 2 個 agent，實際 ${r.calls.length}`)
      if (!capped.length || capped.some((t) => t.ok !== false || t.max_agents !== 2)) errs.push(`沒派出的要記 capped：${JSON.stringify(capped)}`)
      if (r.labels.includes('write')) errs.push('到上限不該寫 PR 說明')
      return errs
    },
  },
  extScenario('ext:a11y'),
  {
    name: '缺件：審查軸沒交件',
    fakes: { 'review:bugs': () => null },
    expect: (r) => [
      ...(r.out && r.out.halted && /review:bugs/.test(r.out.halt_reason || '') && (r.out.missing || []).includes('review:bugs') ? [] : [`審查缺件要 halted 並列出 label：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
      ...(r.labels.includes('write') ? ['審查缺件不該寫 PR 說明'] : []),
    ],
  },
  {
    name: '缺件：skeptic 沒交件',
    fakes: { 'verify:spec': () => null },
    expect: (r) => (r.out && r.out.halted && /verify:spec/.test(r.out.halt_reason || '') ? [] : ['反駁缺件要 halted 並寫出 label']),
  },
  {
    name: 'writer 沒交件：列進 missing',
    fakes: { write: () => null },
    expect: (r) => (r.out && !r.out.halted && (r.out.missing || []).includes('write') ? [] : ['writer 缺件要列進 missing']),
  },
  {
    // 同一則發現判了多次：每一則都 refuted 才算推翻（只會更嚴）。
    // 順序是 [沒推翻, 推翻]：「以最後一則為準」的實作會把 f1 拿掉，這個情境就會失敗
    name: 'skeptic 對同一發現判定不一：保留',
    fakes: {
      'review:bugs': ENV({ lens: 'bugs', findings: [{ id: 'f1', severity: 'important', claim: '邊界值沒處理', location: 'a.py:1', recommendation: '補邊界檢查', confidence: 0.9 }] }),
      'verify:bugs': ENV({ verdicts: [{ target_id: 'f1', refuted: false, reason: 'b' }, { target_id: 'f1', refuted: true, reason: 'a' }] }),
    },
    expect: (r) => {
      const ids = ((r.out || {}).findings || []).map((f) => f.id)
      return ids.includes('f1') && (r.out || {}).dropped === 0 ? [] : [`判定不一時 f1 應保留：${JSON.stringify({ ids, dropped: (r.out || {}).dropped })}`]
    },
  },
  {
    // B-8：pipeline 裡的審查 agent throw，要走缺件流程（halted、列出 label），不能讓 pipeline 整個失敗
    name: '缺件：審查軸 throw',
    fakes: { 'review:spec': 'THROW' },
    expect: (r) => [
      ...(r.out && r.out.halted && (r.out.missing || []).includes('review:spec') ? [] : [`審查 throw 要 halted 並列進 missing：${JSON.stringify(r.out && { halted: r.out.halted, missing: r.out.missing })}`]),
      ...(r.labels.includes('write') ? ['審查 throw 不該寫 PR 說明'] : []),
    ],
  },
  // recipe：審查軸取自 recipe.deliver_axes；沒有 design 段時 spec 軸對照 intent.md
  ...recipeScenarios('deliver', (n) => ({
    expect: (r) => {
      const e = []
      const axes = { feature: ['bugs', 'history', 'spec', 'standards'], change: ['bugs', 'history', 'spec', 'standards'], perf: ['bugs', 'history', 'spec', 'standards'],
        bugfix: ['bugs', 'spec'], hotfix: ['bugs'], refactor: ['bugs', 'history', 'standards'], deps: ['bugs', 'standards'], docs: ['spec', 'standards'], 'test-only': ['bugs', 'standards'], revert: ['bugs'] }[n]
      const got = r.labels.filter((l) => l.startsWith('review:')).map((l) => l.slice(7)).sort()
      if (JSON.stringify(got) !== JSON.stringify(axes)) e.push(`${n} 的審查軸應為 ${axes.join('、')}：${got.join('、')}`)
      const spec = r.prompts['review:spec']
      if (spec && (['feature', 'change', 'perf'].includes(n) ? !/spec\.md/.test(spec) : !/intent\.md/.test(spec))) e.push(`${n} 的 spec 軸對照來源不對`)
      return e
    },
  })),
]
