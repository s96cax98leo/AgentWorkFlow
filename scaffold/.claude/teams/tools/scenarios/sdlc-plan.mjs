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
]
