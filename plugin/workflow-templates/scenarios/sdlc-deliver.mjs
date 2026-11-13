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
  {
    // C2：有交付設定時（資料包工具放進 args.deliverables），多派一個 writer 只更新受影響的小節並寫交付摘要
    name: '交付文件：diff 有 migration 時只更新資料庫設計文件的對應小節',
    args: { deliverables: { summary: '_tpm/runs/x/deliver/交付摘要.md', affected: [
      { id: 'db-design', title: '資料庫設計文件', file: 'docs/deliverables/db-design.md', depth: 'full', why: '改動裡有 migration 類的檔', sections: ['資料表與欄位', '資料遷移'], sources: ['specs/**/design.md', '**/migrations/**'] },
    ] } },
    expect: (r) => {
      const p = r.prompts.deliverables || ''
      const o = (r.out || {}).deliverables || {}
      return [
        ...(r.labels.filter((l) => l === 'deliverables').length === 1 ? [] : ['應該多派一個 deliverables']),
        ...(/資料庫設計文件（docs\/deliverables\/db-design\.md；寫完整；改動裡有 migration 類的檔）：小節〈資料表與欄位〉〈資料遷移〉/.test(p) ? [] : ['派工單沒列出受影響的文件與小節']),
        ...(/API 規格書|api-spec/.test(p) ? ['沒受影響的文件不該出現在派工單'] : []),
        ...(/只改列出的小節，其他小節與其他文件不動/.test(p) && /來源標記/.test(p) && /不寫內部編號/.test(p) ? [] : ['派工單缺規則（只改列出的小節、保留來源標記、不寫內部資訊）']),
        ...(/交付摘要到 _tpm\/runs\/x\/deliver\/交付摘要\.md/.test(p) && /這次交付了什麼/.test(p) ? [] : ['派工單沒要求寫交付摘要']),
        ...(JSON.stringify(o.updated) === JSON.stringify([{ id: 'db-design', file: 'docs/deliverables/db-design.md', sections: ['資料表與欄位', '資料遷移'] }]) && o.summary ? [] : [`回傳的 deliverables 不對：${JSON.stringify(o)}`]),
        ...(((r.out || {}).missing || []).length ? ['不該有缺件'] : []),
      ]
    },
  },
  {
    name: '交付文件：有 public_api 時更新 API 規格書，資料庫設計文件不動',
    args: { deliverables: { summary: '_tpm/runs/x/deliver/交付摘要.md', affected: [
      { id: 'api-spec', title: 'API 規格書', file: 'docs/deliverables/api-spec.md', depth: 'summary', why: '這次有 public_api', sections: ['端點清單'], sources: [] },
    ] } },
    expect: (r) => {
      const p = r.prompts.deliverables || ''
      return [
        ...(/API 規格書（docs\/deliverables\/api-spec\.md；寫摘要；這次有 public_api）：小節〈端點清單〉/.test(p) ? [] : ['派工單沒列出 API 規格書']),
        ...(/資料庫設計文件|db-design/.test(p) ? ['資料庫設計文件不該出現'] : []),
      ]
    },
  },
  {
    name: '交付文件：沒有交付設定時不多派、回傳不變',
    expect: (r) => [
      ...(r.labels.includes('deliverables') ? ['沒有交付設定不該派 deliverables'] : []),
      ...('deliverables' in (r.out || {}) ? ['沒有交付設定時回傳不該有 deliverables'] : []),
    ],
  },
  {
    name: '交付文件：只有交付摘要（沒有受影響的小節）',
    args: { deliverables: { summary: '_tpm/runs/x/deliver/交付摘要.md' } },
    expect: (r) => [
      ...(/這次沒有受影響的文件小節，只寫交付摘要/.test(r.prompts.deliverables || '') ? [] : ['應該只寫交付摘要']),
    ],
  },
  {
    name: '交付文件：writer 缺件列進 missing；不合理的路徑不採用',
    args: { deliverables: { summary: '_tpm/runs/x/deliver/交付摘要.md', affected: [
      { id: 'x', title: '壞路徑', file: '../../outside.md', sections: ['a'] },
      { id: 'y', title: '絕對路徑', file: '/etc/passwd', sections: ['a'] },
      { id: 'db-design', title: '資料庫設計文件', file: 'docs/deliverables/db-design.md', sections: ['資料遷移'] },
    ] } },
    fakes: { deliverables: 'THROW' },
    expect: (r) => [
      ...(((r.out || {}).missing || []).includes('deliverables') ? [] : [`交付文件缺件要列進 missing：${JSON.stringify((r.out || {}).missing)}`]),
      ...(/outside\.md|\/etc\/passwd/.test(r.prompts.deliverables || '') ? ['跳出專案的路徑不該進派工單'] : []),
      ...((((r.out || {}).deliverables || {}).updated || []).length === 1 ? [] : ['只該採用路徑合理的那一份']),
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
