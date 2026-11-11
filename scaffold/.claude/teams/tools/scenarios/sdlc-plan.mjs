// sdlc-plan.js 的乾跑情境：走預設乾跑碰不到的分支。
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { extScenario, ENV } from './_helpers.mjs'

const SD_PROFILE = { stages: { design: { system_design: { source: 'System Design Primer', steps: ['1 用例、限制與假設'], feature_scope: 'x', system_scope: 'x' } } } }

// recipe 情境用內建 default profile 的 recipes／modifiers（scaffold 本身沒有內嵌 profile，init 過的專案可能自訂過）
const DEF = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'methodologies', 'default.json'), 'utf8'))
const RP = { recipes: DEF.recipes, modifiers: DEF.modifiers }

const TRIAGE = (kind, confidence) => (base) => ({ ...base, status: 'done', open_questions: [],
  payload: { ...base.payload, kind, confidence, size: 'medium', modifiers: [], alternatives: [] } })
const sub = (kind) => ({ id: 't1', title: 'x', kind, owns: ['x'], depends_on: [], acceptance: ['x'] })
// tasks 的假回傳：子任務 kind 用 recipe 允許的第一種
const TASKS = (name) => ENV({ epic: 'demo-999-sample', subtasks: [sub(((DEF.recipes[name] || {}).build_kinds || ['code'])[0] || 'code')] })
const recipeOf = (r) => (r.out && r.out.recipe && r.out.recipe.name) || null
const notHalted = (r) => (r.out && !r.out.halted ? [] : [`不該 halted：${JSON.stringify(r.out && r.out.halt_reason)}`])

const triageCase = (name, kind, want, extra = []) => ({
  name, profile: RP, fakes: { triage: TRIAGE(kind, 0.9), tasks: TASKS(want) },
  expect: (r) => [
    ...(r.labels.includes('triage') ? [] : ['沒有派 triage']),
    ...(recipeOf(r) === want ? [] : [`recipe 應為 ${want}，實際 ${recipeOf(r)}`]),
    ...(r.out && r.out.triage && r.out.triage.kind === kind ? [] : ['回傳沒有 triage']),
    ...notHalted(r),
    ...extra.flatMap((f) => f(r)),
  ],
})

// 每種 plan_mode 挑一個 recipe（baseline 用 perf）
const MODE_RECIPE = { feature: 'feature', diagnose: 'bugfix', impact: 'change', invariants: 'refactor', changelog: 'deps', baseline: 'perf', research: 'spike', revert: 'revert' }
const SECTIONS = {
  feature: [], diagnose: ['現象', '重現步驟', '預期行為', '根因'], impact: ['既有驗收條件異動'], invariants: ['不可改變的行為'],
  changelog: ['上游變更與破壞性變更'], baseline: ['量測基線'], research: ['調研問題', '判準'], revert: ['回退原因', '回退範圍', '恢復條件'],
}
const BASE = ['目標', '範圍', '不做的事', '驗收條件']

// 既有情境沒有覆寫 triage 時用：依 schema 產生的假 triage 會帶不認得的 modifier「x」而停下
const FEATURE_TRIAGE = { triage: TRIAGE('feature', 0.9) }
const ext = extScenario('ext:db')

// ── 0.4：模型分級、兩段式調研、角度數、重用、派工上限 ──
const MODELS = { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'standard' }, floors: {} }
const MP = { ...RP, models: MODELS }
const LOC = (k, locs) => ENV({ angle: k, locations: locs, queries: ['kb.py query x'], gaps: [] })
const ONE_LOC = [{ path: 'services/a.py', start: 10, end: 40, why: '匯出欄位順序' }]
const tr = (r) => (r.out && r.out.trace) || []
const byLabel = (r, l) => tr(r).filter((t) => t.label === l)
// 先失敗 n 次再成功的假回傳
const flaky = (n, okFn = (b) => b) => { let i = 0; return (base, prompt) => (i++ < n ? null : okFn(base, prompt)) }
const TRI = (kind, extra) => (base) => ({ ...base, status: 'done', open_questions: [],
  payload: { ...base.payload, kind, confidence: 0.9, size: 'medium', modifiers: [], alternatives: [], ...extra } })

const V04 = [
  {
    name: '兩段式調研：先定位再分析，分析只收到行段；upstream 不分段',
    profile: RP, args: { recipe: 'feature' },
    fakes: { 'locate:code-contracts': LOC('code-contracts', ONE_LOC) },
    expect: (r) => {
      const p = r.prompts['research:code-contracts'] || ''
      const order = (a, b) => r.labels.indexOf(a) >= 0 && r.labels.indexOf(a) < r.labels.indexOf(b)
      return [
        ...(['requirement', 'specs-adr', 'code-contracts'].every((k) => order(`locate:${k}`, `research:${k}`)) ? [] : [`每個角度要先 locate 再 research：${r.labels.join(',')}`]),
        ...(r.labels.includes('locate:upstream') ? ['upstream 不查本機檔案，不該派 locate'] : []),
        ...(r.labels.includes('research:upstream') ? [] : ['沒有派 research:upstream']),
        ...(/services\/a\.py:10-40 ｜ 匯出欄位順序/.test(p) && /只讀下面定位好的行段/.test(p) ? [] : ['分析的派工單沒有列出定位到的行段']),
        ...(/不下結論/.test(r.prompts['locate:code-contracts'] || '') ? [] : ['定位的派工單要寫明只找行段、不下結論']),
        ...(r.out && r.out.locations && (r.out.locations['code-contracts'] || []).length === 1 ? [] : ['回傳沒有 locations（重用要記來源檔）']),
        ...notHalted(r),
      ]
    },
  },
  {
    name: '兩段式調研：派工單有上限與停止條件',
    profile: RP, args: { recipe: 'bugfix' }, fakes: { tasks: TASKS('bugfix') },
    expect: (r) => [
      ...(/最多 12 次查詢/.test(r.prompts['locate:code-contracts'] || '') && /連續 3 次查詢沒有新的相關行段就停/.test(r.prompts['locate:code-contracts'] || '') ? [] : ['定位沒有寫上限與停止條件']),
      ...(/最多 15 輪工具呼叫/.test(r.prompts['research:code-contracts'] || '') && /連續 3 次查詢沒有新發現就收斂/.test(r.prompts['research:code-contracts'] || '') ? [] : ['分析沒有寫上限與停止條件']),
      ...(/邊做邊寫進執行紀錄檔/.test(r.prompts['research:code-contracts'] || '') ? [] : ['分析沒有要求邊做邊寫紀錄']),
    ],
  },
  {
    name: '定位缺件：不派分析，該角度列缺件，不中斷',
    profile: RP, args: { recipe: 'bugfix' },
    fakes: { 'locate:code-contracts': () => null, tasks: TASKS('bugfix') },
    expect: (r) => [
      ...(r.labels.includes('research:code-contracts') ? ['定位缺件不該再派分析（沒有行段會變成整檔亂讀）'] : []),
      ...((r.out && r.out.missing || []).includes('research:code-contracts') ? [] : [`定位缺件要列進 missing：${JSON.stringify(r.out && r.out.missing)}`]),
      ...(/缺件的調研角度[\s\S]*research:code-contracts/.test(r.prompts.intent || '') ? [] : ['intent 派工單沒提缺件的角度']),
      ...notHalted(r),
    ],
  },
  {
    name: '資料包：args 帶 pack 時派工單引用它',
    profile: RP, args: { recipe: 'bugfix', pack: '/tmp/run/plan/pack.md' }, fakes: { tasks: TASKS('bugfix') },
    expect: (r) => (['locate:code-contracts', 'research:code-contracts', 'intent', 'challenge:intent'].every((l) => (r.prompts[l] || '').includes('資料包：/tmp/run/plan/pack.md')) ? [] : ['派工單沒有引用資料包']),
  },
  {
    name: '角度數取自 triage：只做挑的角度',
    profile: RP, fakes: { triage: TRI('feature', { angles: ['code-contracts'] }) },
    expect: (r) => {
      const res = r.labels.filter((l) => l.startsWith('research:'))
      return [
        ...(res.length === 1 && res[0] === 'research:code-contracts' ? [] : [`只該調研 code-contracts：${res.join(',')}`]),
        ...(r.out && r.out.angles && r.out.angles.by === 'triage' && r.out.angles.skipped.length === 3 ? [] : [`回傳的 angles：${JSON.stringify(r.out && r.out.angles)}`]),
        ...(/這次沒做的調研角度/.test(r.prompts.intent || '') ? [] : ['intent 派工單沒提沒做的角度']),
        ...notHalted(r),
      ]
    },
  },
  {
    name: '角度數：modifier 帶進來的角度不能省',
    profile: RP, fakes: { triage: TRI('bugfix', { angles: ['code-contracts'], modifiers: ['large'] }), tasks: TASKS('bugfix') },
    expect: (r) => {
      const res = r.labels.filter((l) => l.startsWith('research:')).sort()
      // bugfix 自己的角度是 requirement、code-contracts；large 另外帶進 specs-adr、upstream
      return res.join(',') === 'research:code-contracts,research:specs-adr,research:upstream' ? [] : [`large 帶進來的角度要保留、bugfix 自己的只留挑的：${res.join(',')}`]
    },
  },
  {
    name: '角度數：triage 挑的角度不在 recipe 裡就全做',
    profile: RP, fakes: { triage: TRI('bugfix', { angles: ['nope'] }), tasks: TASKS('bugfix') },
    expect: (r) => (r.labels.filter((l) => l.startsWith('research:')).length === 2 && r.out.angles.by === 'recipe' ? [] : [`挑不到就全做：${r.labels.join(',')}`]),
  },
  {
    name: '調研重用：來源沒變的角度不派工',
    profile: RP,
    args: { recipe: 'bugfix', reuse: { 'code-contracts': { plan_mode: 'diagnose', envelope: { status: 'done', summary: '上次的結論', artifacts: ['/tmp/run/plan/research-code-contracts.md'], evidence: [], confidence: 0.9, open_questions: [], derived_work: [], payload: { lens: 'code-contracts', findings: [] } } } } },
    fakes: { tasks: TASKS('bugfix') },
    expect: (r) => [
      ...(r.labels.some((l) => /code-contracts/.test(l)) ? [`沿用的角度不該派工：${r.labels.join(',')}`] : []),
      ...(r.labels.includes('research:requirement') ? [] : ['沒沿用的角度要照派']),
      ...((r.out && r.out.reused || []).join(',') === 'code-contracts' ? [] : [`回傳的 reused：${JSON.stringify(r.out && r.out.reused)}`]),
      ...(/上次的結論/.test(r.prompts.intent || '') ? [] : ['intent 派工單沒有沿用的結論']),
      ...(tr(r).length === r.labels.length ? [] : ['沿用的角度不該有 trace']),
      ...notHalted(r),
    ],
  },
  {
    name: '調研重用：plan_mode 不同就不沿用',
    profile: RP,
    args: { recipe: 'bugfix', reuse: { 'code-contracts': { plan_mode: 'feature', envelope: { status: 'done', summary: 'x', payload: { lens: 'code-contracts', findings: [] } } } } },
    fakes: { tasks: TASKS('bugfix') },
    expect: (r) => (r.labels.includes('research:code-contracts') && !(r.out.reused || []).length ? [] : ['plan_mode 不同不能沿用']),
  },
  {
    name: '模型分級：每次派工都帶 model，trace 有 model、tier、reason',
    profile: MP, args: { recipe: 'bugfix' }, fakes: { tasks: TASKS('bugfix') },
    expect: (r) => {
      const noModel = r.calls.filter((c) => !c.model).map((c) => c.label)
      const badTrace = tr(r).filter((t) => !t.model || !t.tier || !t.reason).map((t) => t.label)
      const m = (l) => (r.calls.find((c) => c.label === l) || {}).model
      return [
        ...(noModel.length ? [`沒帶 model 的派工：${noModel.join(',')}`] : []),
        ...(badTrace.length ? [`trace 缺 model／tier／reason：${badTrace.join(',')}`] : []),
        ...(m('locate:code-contracts') === 'haiku' ? [] : [`定位要用 light（haiku）：${m('locate:code-contracts')}`]),
        ...(m('research:code-contracts') === 'sonnet' ? [] : [`分析預設 standard（sonnet）：${m('research:code-contracts')}`]),
        ...(m('intent') === 'opus' && m('tasks') === 'opus' ? [] : [`plan 段的 dev-spec-author 下限是 strong：${m('intent')}／${m('tasks')}`]),
        ...((r.out.models || []).some((x) => x.key === 'locate' && x.tier === 'light') ? [] : ['回傳沒有 models（閘門要看用了哪些模型）']),
      ]
    },
  },
  {
    name: '模型分級：AI 建議低於下限會被往上調、建議更高會採用',
    profile: MP,
    fakes: { triage: TRI('bugfix', { model_plan: [{ role: 'dev-spec-author', tier: 'light', reason: '很簡單' }, { role: 'locate', tier: 'standard', reason: '程式分散' }, { role: 'agent-workflow:tpm-skeptic', tier: 'light', reason: '小修正' }] }), tasks: TASKS('bugfix') },
    expect: (r) => {
      const m = (l) => (r.calls.find((c) => c.label === l) || {}).model
      return [
        ...(m('intent') === 'opus' ? [] : [`dev-spec-author@plan 建議 light 要被調成 strong：${m('intent')}`]),
        ...(m('locate:code-contracts') === 'sonnet' ? [] : [`locate 建議 standard 要採用：${m('locate:code-contracts')}`]),
        ...(m('challenge:intent') === 'haiku' ? [] : [`skeptic 建議 light（沒有下限）要採用：${m('challenge:intent')}`]),
        ...(/往上調成 strong/.test((byLabel(r, 'intent')[0] || {}).reason || '') ? [] : ['trace 的 reason 要寫出被往上調']),
      ]
    },
  },
  {
    name: '模型分級：security modifier 把 skeptic 的下限拉到 strong',
    profile: MP,
    fakes: { triage: TRI('bugfix', { modifiers: ['security'], model_plan: [{ role: 'tpm-skeptic', tier: 'light', reason: 'x' }] }), tasks: TASKS('bugfix') },
    expect: (r) => ((r.calls.find((c) => c.label === 'challenge:intent') || {}).model === 'opus' ? [] : ['security 時 skeptic 要用 strong']),
  },
  {
    name: '模型分級：低信心改走 feature 時不採 AI 的 model_plan',
    profile: MP,
    fakes: { triage: (b) => ({ ...TRI('bugfix', { model_plan: [{ role: 'tpm-researcher', tier: 'light', reason: 'x' }] })(b), payload: { ...TRI('bugfix', { model_plan: [{ role: 'tpm-researcher', tier: 'light', reason: 'x' }] })(b).payload, confidence: 0.4 } }) },
    expect: (r) => ((r.calls.find((c) => c.label === 'research:code-contracts') || {}).model === 'sonnet' ? [] : ['低信心時分析要用預設 standard']),
  },
  {
    name: '模型分級：mode=inherit 不指定 model',
    profile: { ...RP, models: { ...MODELS, mode: 'inherit' } }, args: { recipe: 'bugfix' }, fakes: { tasks: TASKS('bugfix') },
    expect: (r) => [
      ...(r.calls.every((c) => !c.model) ? [] : ['inherit 不該帶 model']),
      ...(tr(r).every((t) => t.model === 'inherit' && t.tier === '') ? [] : ['inherit 時 trace 的 model 記 inherit']),
    ],
  },
  {
    name: '失敗升級：低等級沒交件，升一級重派一次',
    profile: MP, args: { recipe: 'bugfix' },
    fakes: { 'locate:code-contracts': flaky(1, LOC('code-contracts', ONE_LOC)), tasks: TASKS('bugfix') },
    expect: (r) => {
      const t = byLabel(r, 'locate:code-contracts')
      const c = r.calls.filter((x) => x.label === 'locate:code-contracts').map((x) => x.model)
      return [
        ...(c.join(',') === 'haiku,sonnet' ? [] : [`要先 haiku 再 sonnet：${c.join(',')}`]),
        ...(t.length === 2 && t[0].ok === false && t[0].superseded === true && t[1].ok === true && t[1].attempt === 2 && t[1].escalated_from === 'light' ? [] : [`trace 要記兩筆（第一筆 superseded）：${JSON.stringify(t)}`]),
        ...(r.labels.includes('research:code-contracts') ? [] : ['升級成功後要繼續分析']),
        ...notHalted(r),
      ]
    },
  },
  {
    name: '失敗升級：升級後再失敗就是缺件（triage → halted），最多重派一次',
    profile: MP, fakes: { triage: 'THROW' },
    expect: (r) => [
      ...(r.calls.filter((c) => c.label === 'triage').length === 2 ? [] : [`triage 只能重派一次：${r.calls.length}`]),
      ...(r.out && r.out.halted && /triage/.test(r.out.halt_reason || '') ? [] : ['再失敗要 halted']),
      ...(byLabel(r, 'triage').every((t) => t.ok === false) ? [] : ['兩筆 trace 都要是 ok=false']),
    ],
  },
  {
    name: '失敗升級：已經是最高等級不重派（intent → halted）',
    profile: MP, args: { recipe: 'bugfix' }, fakes: { intent: () => null },
    expect: (r) => [
      ...(r.calls.filter((c) => c.label === 'intent').length === 1 ? [] : ['strong 沒有更高一級，不該重派']),
      ...(r.out && r.out.halted ? [] : ['intent 缺件要 halted']),
    ],
  },
  {
    name: '失敗升級：mode=fixed 不重派',
    profile: { ...RP, models: { ...MODELS, mode: 'fixed' } }, args: { recipe: 'bugfix' },
    fakes: { 'research:requirement': () => null, tasks: TASKS('bugfix') },
    expect: (r) => (r.calls.filter((c) => c.label === 'research:requirement').length === 1 ? [] : ['fixed 不該升級重派']),
  },
  {
    name: '派工數上限：到上限就 halted，不靜默截斷',
    profile: { ...RP, limits: { max_agents: 3 } }, args: { recipe: 'feature' },
    expect: (r) => [
      ...(r.labels.length === 3 ? [] : [`只能派 3 個：${r.labels.length}`]),
      ...(r.out && r.out.halted && /上限 3/.test(r.out.halt_reason || '') ? [] : [`到上限要 halted 並寫明：${JSON.stringify(r.out && r.out.halt_reason)}`]),
      ...(r.out && r.out.gate === null ? [] : ['到上限不能給閘門']),
    ],
  },
]

export default [
  { ...ext, fakes: { ...ext.fakes, ...FEATURE_TRIAGE } },
  {
    name: 'profile 有系統設計方法：skeptic 檢查 SA',
    profile: SD_PROFILE,
    fakes: { ...FEATURE_TRIAGE, 'challenge:intent': (base, prompt) => { if (!/檢查 SA/.test(prompt) || !/標成假設/.test(prompt)) throw new Error('plan 的 skeptic 沒有檢查 SA'); return base } },
    expect: (r) => (r.labels.includes('challenge:intent') ? [] : ['沒有派出 challenge:intent']),
  },
  {
    name: '缺件：skeptic 沒交件',
    fakes: { ...FEATURE_TRIAGE, 'challenge:intent': () => null },
    expect: (r) => (r.out && r.out.halted && /challenge:intent/.test(r.out.halt_reason || '') ? [] : [`反駁缺件要 halted 並寫出 label：${JSON.stringify(r.out && { halted: r.out.halted, why: r.out.halt_reason })}`]),
  },
  {
    name: '缺件：intent 沒交件',
    fakes: { ...FEATURE_TRIAGE, intent: () => null },
    expect: (r) => [
      ...(r.out && r.out.halted && /intent/.test(r.out.halt_reason || '') ? [] : ['intent 缺件要 halted']),
      ...(r.labels.includes('challenge:intent') ? ['intent 缺件不該再派 skeptic'] : []),
    ],
  },
  {
    // 調研不是審查類：不中斷，但要列進 missing，並告訴 intent 作者哪些角度沒查
    name: '調研角度沒交件：列進 missing',
    profile: RP,
    fakes: { ...FEATURE_TRIAGE, 'research:upstream': () => null },
    expect: (r) => [
      ...(r.out && !r.out.halted && (r.out.missing || []).includes('research:upstream') ? [] : [`調研缺件要列進 missing：${JSON.stringify(r.out && r.out.missing)}`]),
      ...(/缺件的調研角度[\s\S]*research:upstream/.test(r.prompts.intent || '') ? [] : ['intent 派工單沒提缺件的調研角度']),
    ],
  },

  // ── triage（intake）──
  triageCase('triage:bug→bugfix', 'bugfix', 'bugfix', [
    (r) => (r.labels.includes('tasks') && (r.out.subtasks || []).length ? [] : ['bugfix 沒有 design 段，plan 要派 tasks 並回傳 subtasks']),
    (r) => (/^sdlc-build（args 帶 recipe=bugfix/.test(r.out.next || '') ? [] : [`next 應指向 sdlc-build：${r.out.next}`]),
  ]),
  triageCase('triage:CR→change', 'change', 'change', [
    (r) => (!r.labels.includes('tasks') && /^sdlc-design/.test(r.out.next || '') ? [] : [`change 有 design 段，不派 tasks、next 指向 sdlc-design：${r.out.next}`]),
  ]),
  triageCase('triage:docs→docs', 'docs', 'docs'),
  triageCase('triage:incident→hotfix', 'hotfix', 'hotfix'),
  {
    name: 'triage:低信心→feature',
    profile: RP,
    fakes: { triage: TRIAGE('bugfix', 0.5) },
    expect: (r) => [
      ...(recipeOf(r) === 'feature' ? [] : [`低信心要改用 feature，實際 ${recipeOf(r)}`]),
      ...(((r.out && r.out.triage && r.out.triage.alternatives) || []).some((a) => a.kind === 'bugfix') ? [] : ['triage.alternatives 要含原本建議的 bugfix']),
      ...notHalted(r),
    ],
  },
  {
    name: 'args 帶 recipe：不派 triage',
    profile: RP,
    args: { recipe: 'bugfix', modifiers: ['ui'] },
    fakes: { tasks: TASKS('bugfix') },
    expect: (r) => [
      ...(r.labels.includes('triage') ? ['args 帶 recipe 不該派 triage'] : []),
      ...(recipeOf(r) === 'bugfix' && (r.out.recipe.modifiers || []).includes('ui') ? [] : [`應沿用 bugfix＋ui：${JSON.stringify(r.out && r.out.recipe)}`]),
      ...(r.out && !('triage' in r.out) ? [] : ['沒派 triage 時回傳不該有 triage 鍵']),
      ...notHalted(r),
    ],
  },
  {
    name: '不認得的 modifier：halted',
    profile: RP,
    args: { recipe: 'bugfix', modifiers: ['nope'] },
    expect: (r) => (r.out && r.out.halted && /modifier/.test(r.out.halt_reason || '') && !r.labels.includes('intent') ? [] : [`不認得的 modifier 要 halted：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'tasks 的 kind 超出 recipe：halted',
    profile: RP,
    args: { recipe: 'docs' },
    fakes: { tasks: ENV({ epic: 'demo-999-sample', subtasks: [sub('code')] }) },
    expect: (r) => (r.out && r.out.halted && /kinds/.test(r.out.halt_reason || '') ? [] : [`docs 拆出 code 子任務要 halted：${JSON.stringify(r.out && r.out.halt_reason)}`]),
  },
  {
    name: 'spike：沒有後續段',
    profile: RP,
    args: { recipe: 'spike' },
    expect: (r) => [
      ...(r.out && r.out.next === '結案：spike 沒有後續段' ? [] : [`spike 的 next：${r.out && r.out.next}`]),
      ...(r.labels.includes('tasks') ? ['spike 沒有 build，不該派 tasks'] : []),
      ...(r.out && r.out.estimated_agents && r.out.estimated_agents.plan > 0 && !('build' in r.out.estimated_agents) ? [] : [`estimated_agents：${JSON.stringify(r.out && r.out.estimated_agents)}`]),
    ],
  },
  {
    name: 'trace：每次派工都記錄、紀錄檔在 run_dir/plan/',
    profile: RP,
    args: { recipe: 'bugfix' },
    fakes: { tasks: TASKS('bugfix') },
    expect: (r) => {
      const t = (r.out && r.out.trace) || []
      return [
        ...(t.length === r.labels.length ? [] : [`trace ${t.length} 筆，派工 ${r.labels.length} 次`]),
        ...(t.every((x) => /^\/tmp\/run\/plan\//.test(x.record)) ? [] : ['trace 的紀錄檔不在 run_dir/plan/']),
        ...(/執行紀錄檔：\/tmp\/run\/plan\/intent\.md$/.test(r.prompts.intent || '') ? [] : ['intent 派工單最後一行沒有指定紀錄檔']),
      ]
    },
  },
  {
    name: '缺件：triage 沒交件',
    fakes: { triage: () => null },
    expect: (r) => [
      ...(r.out && r.out.halted && /triage/.test(r.out.halt_reason || '') ? [] : ['triage 缺件要 halted']),
      ...(r.labels.some((l) => /^research:|^intent$/.test(l)) ? ['triage 缺件不該再派調研或 intent'] : []),
      ...(r.out && Array.isArray(r.out.trace) ? [] : ['halted 也要帶 trace']),
    ],
  },
  {
    name: '缺件：triage throw',
    fakes: { triage: 'THROW' },
    expect: (r) => (r.out && r.out.halted && /triage/.test(r.out.halt_reason || '') ? [] : ['triage throw 要 halted']),
  },

  // ── plan_mode：intent 派工單要求必要節、skeptic 依 mode 反駁並檢查驗收條件可驗證 ──
  ...Object.entries(MODE_RECIPE).map(([mode, rec]) => ({
    name: `plan_mode:${mode}`,
    profile: RP,
    args: { recipe: rec },
    fakes: { tasks: TASKS(rec) },
    expect: (r) => {
      const p = r.prompts.intent || ''
      const c = r.prompts['challenge:intent'] || ''
      const miss = [...BASE, ...SECTIONS[mode]].filter((s) => !p.includes(s))
      return [
        ...(r.out && r.out.recipe && r.out.recipe.plan_mode === mode ? [] : [`${rec} 的 plan_mode 不是 ${mode}`]),
        ...(miss.length ? [`intent 派工單沒要求：${miss.join('、')}`] : []),
        ...(/可驗證/.test(c) ? [] : ['skeptic 沒檢查驗收條件可驗證']),
        ...(mode === 'diagnose' && !(/根因/.test(c) && /重現/.test(c) && /證據/.test(c)) ? ['diagnose 的 skeptic 沒反駁根因的重現與證據'] : []),
        ...(mode === 'diagnose' && !/git log -L/.test(r.prompts['research:code-contracts'] || '') ? ['diagnose 的 code-contracts 角度沒改問錯誤路徑'] : []),
        ...notHalted(r),
      ]
    },
  })),

  // ── 11 個 recipe 都走得完 plan 段 ──
  ...Object.keys(DEF.recipes).map((name) => ({
    name: `recipe:${name}`,
    profile: RP,
    args: { recipe: name },
    fakes: { tasks: TASKS(name) },
    expect: (r) => [
      ...notHalted(r),
      ...(recipeOf(r) === name ? [] : [`recipe 應為 ${name}，實際 ${recipeOf(r)}`]),
      ...(r.labels.includes('intent') && r.labels.includes('challenge:intent') ? [] : ['要派 intent 與 challenge:intent']),
      ...(typeof (r.out && r.out.next) === 'string' && r.out.next ? [] : ['沒有 next']),
    ],
  })),
  ...V04,
]
