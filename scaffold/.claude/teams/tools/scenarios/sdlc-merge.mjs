// sdlc-merge.js 的乾跑情境：走預設乾跑碰不到的分支。
import { CONSISTENT, EXEC } from './_helpers.mjs'

export default [
    {
      name: 'PR 落後 base：先同步',
      args: {},
      fakes: { analyze: (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'hold', needs_sync: true } }) },
      expect: (r) => [
        ...(r.out && r.out.needs_sync === true && r.out.merged === false && /sdlc-sync/.test(r.out.next || '') ? [] : ['needs_sync 要回傳 needs_sync=true 並指向 sdlc-sync']),
        ...(r.labels.some((l) => l.startsWith('execute') || l === 'challenge') ? [`needs_sync 不能進挑戰或執行：${r.labels.join(',')}`] : []),
      ],
    },
    {
      name: '讀回已合併',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('merged') },
      expect: (r) => (r.labels.includes('execute:merge') && r.out && r.out.merged === true && !r.out.queued ? [] : [`讀回 merged 應該 merged=true：${JSON.stringify(r.out && { merged: r.out.merged, queued: r.out.queued })}`]),
    },
    {
      name: '只進 merge queue',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('queued') },
      expect: (r) => [
        ...(r.out && r.out.merged === false && r.out.queued === true ? [] : [`排隊中不能算已合併：${JSON.stringify(r.out && { merged: r.out.merged, queued: r.out.queued })}`]),
        ...(r.labels.includes('followup') ? ['排隊中不該產出 followup 文件'] : []),
      ],
    },
    {
      name: 'gate 判 merge 但沒讀回狀態',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': (base) => ({ ...base, status: 'done', open_questions: [], payload: { ...base.payload, decision: 'merge' } }) },
      expect: (r) => (r.labels.includes('execute:merge') && r.out && r.out.merged === false ? [] : ['沒有 merge_state 不能算已合併']),
    },
    {
      // merge_gate.py 有 --findings 卻沒有 --reviewed-sha 一律 hold：gate 指令兩個都要帶，reviewed-sha 取審查／驗證當下的 head
      name: 'gate 指令帶 reviewed-sha',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('merged') },
      expect: (r) => {
        const p = r.prompts['execute:merge'] || ''
        return /--findings/.test(p) && /--reviewed-sha <審查／驗證當下的 head[^>]*verifier\.payload\.commit[^>]*不要拿目前 head 頂替/.test(p) ? [] : ['execute:merge 的 gate 指令沒帶 --findings 與取自驗證證據的 --reviewed-sha']
      },
    },
    {
      name: 'args.reviewed_sha 直接帶進 gate（退回也要帶）',
      args: { reviewed_sha: ['d'.repeat(40), 'e'.repeat(40)] },
      expect: (r) => {
        const p = r.prompts['execute:return'] || r.prompts['execute:merge'] || ''
        return p.includes(`--reviewed-sha ${'d'.repeat(40)} --reviewed-sha ${'e'.repeat(40)}`) ? [] : ['args.reviewed_sha 沒有逐一帶成 --reviewed-sha']
      },
    },
    {
      // skeptic 缺件不是「判出不一致」：不退回 PR、不做任何 gh 寫入，停下交人
      name: '缺件：合併模式 skeptic 沒交件',
      args: {},
      fakes: { analyze: CONSISTENT.analyze, challenge: () => null },
      expect: (r) => [
        ...(r.out && r.out.halted && /challenge/.test(r.out.halt_reason || '') && r.out.merged === false ? [] : ['skeptic 缺件要 halted 且不合併']),
        ...(r.labels.some((l) => l.startsWith('execute')) ? [`skeptic 缺件不該進執行模式：${r.labels.join(',')}`] : []),
      ],
    },
    {
      name: '缺件：分析沒交件',
      args: {},
      fakes: { analyze: () => null },
      expect: (r) => (r.out && r.out.halted && /analyze/.test(r.out.halt_reason || '') && r.labels.length === 1 ? [] : ['analyze 缺件要 halted 並停在分析']),
    },
    {
      name: '缺件：執行模式沒交件',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': () => null },
      expect: (r) => (r.out && r.out.halted && r.out.merged === false && (r.out.missing || []).includes('execute:merge') ? [] : ['execute 缺件要 halted、merged=false']),
    },
    {
      // B-8：執行模式 throw 可能發生在 gh pr merge 之後：halted，寫明 PR 狀態未知、先查 PR 狀態，不能當成沒合併
      name: '缺件：執行模式 throw',
      args: {},
      fakes: { ...CONSISTENT, 'execute:merge': 'THROW' },
      expect: (r) => {
        const o = r.out || {}
        return [
          ...(o.halted && (o.missing || []).includes('execute:merge') ? [] : [`執行 throw 要 halted 並列進 missing：${JSON.stringify({ halted: o.halted, missing: o.missing })}`]),
          ...(/PR 狀態未知/.test(o.halt_reason || '') && /先查 PR 狀態/.test(o.halt_reason || '') ? [] : [`halt_reason 要寫明 PR 狀態未知、先查 PR 狀態：${o.halt_reason}`]),
          ...(o.merged ? ['執行 throw 不能算已合併'] : []),
        ]
      },
    },
    {
      // B-8：分析模式 throw 也是缺件：不合併、不退回
      name: '缺件：分析模式 throw',
      args: {},
      fakes: { analyze: 'THROW' },
      expect: (r) => (r.out && r.out.halted && (r.out.missing || []).includes('analyze') && !r.labels.some((l) => l.startsWith('execute')) ? [] : [`analyze throw 要 halted 且不執行：${JSON.stringify(r.out && { halted: r.out.halted, missing: r.out.missing })}`]),
    },
    {
      // recipe 沒有 design 段：steward 對照 intent.md 的驗收條件；gate 帶 --recipe（run.json）
      name: 'recipe:bugfix',
      args: { recipe: 'bugfix' },
      fakes: { ...CONSISTENT, 'execute:merge': EXEC('merged') },
      expect: (r) => {
        const e = []
        const a = r.prompts.analyze || ''
        if (!/intent\.md/.test(a) || /spec\.md/.test(a)) e.push('bugfix 的 analyze 要對照 intent.md、不提 spec.md')
        if (!/--recipe \/tmp\/run\/run\.json/.test(r.prompts['execute:merge'] || '')) e.push('merge_gate 指令要帶 --recipe <run_dir>/run.json')
        if (!r.out.merged) e.push(`一致時應合併：${JSON.stringify(r.out.decision)}`)
        return e
      },
    },
]
