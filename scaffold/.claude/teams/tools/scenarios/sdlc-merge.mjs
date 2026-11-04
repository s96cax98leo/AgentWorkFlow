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
  ]
