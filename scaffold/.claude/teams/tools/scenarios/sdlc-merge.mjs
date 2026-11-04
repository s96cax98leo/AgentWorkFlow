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
  ]
