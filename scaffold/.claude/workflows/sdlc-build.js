export const meta = {
  name: 'sdlc-build',
  description: 'AI-native SDLC 第 3 段 Build：依開發規範 profile（methodology）決定測試時機（測試先行＝先寫會失敗的測試；否則實作後補驗證測試）→ 依相依分層、每層子任務在各自 worktree 平行實作 → integrator 合併到 {{BRANCH_PREFIX}}<epic>',
  whenToUse: 'sdlc-design 的 spec＋plan 核准之後；args 加 subtasks（sdlc-design 回傳的 design.payload.subtasks）',
  phases: [
    { title: 'Red', detail: 'profile 的 build.test_first=true 時：dev-acceptance-tester 每個 test 子任務一個，平行寫會失敗的測試，合進整合分支後才實作' },
    { title: 'Implement', detail: 'dev-implementer：同一層的子任務平行，各自 worktree' },
    { title: 'Integrate', detail: 'dev-integrator：每層合併並跑驗證' },
    { title: 'Tests', detail: 'profile 的 build.test_first=false 時：實作合併後，dev-acceptance-tester 平行補驗證測試' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const BUILD = /*@schema:build-result*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["task_id","branch","commits","files_changed","tests"],"properties":{"task_id":{"type":"string"},"branch":{"type":"string"},"worktree":{"type":"string"},"commits":{"type":"array","items":{"type":"string"}},"files_changed":{"type":"array","items":{"type":"string"}},"tests":{"type":"array","items":{"type":"object","required":["command","result"],"properties":{"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","red_as_expected","not_run"]},"summary":{"type":"string"}}}},"conflicts":{"type":"array","items":{"type":"object","required":["file","kind","resolution"],"properties":{"file":{"type":"string"},"kind":{"type":"string","enum":["mechanical","seam","judgement","implicit_compile","implicit_behavior","generated"]},"resolution":{"type":"string"}}}}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
const STAGE = ((MP.stages || {}).build) || {}
const CONV = MP.conventions || {}
const METH_NOTE = METHODOLOGY
  ? `開發規範 profile：${MP.name}（.claude/teams/methodology.json；各段做法與慣例以它為準）。文件格式：${(CONV.doc_format || {}).format || '依目標 repo'}。`
  : '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），做法與慣例依目標 repo 既有規範。'
const commitFor = (kind, epic) => {
  const c = CONV.commit || {}
  const ex = ((c.examples || {})[kind] || '').replace(/<epic>/g, epic)
  return c.format ? `commit 訊息照 profile 的格式：${c.format}${ex ? `（範例：${ex}）` : ''}` : 'commit 訊息照目標 repo 的慣例'
}
// 沒有 profile 時沿用測試先行（模版原本的做法）
const TEST_FIRST = STAGE.test_first !== false
const CODE_STYLE = CONV.code_style ? `code style：${CONV.code_style.description}${CONV.code_style.command ? `（檢查指令：${CONV.code_style.command}）` : ''}` : 'code style：照目標 repo 既有設定'

const A = args || {}
for (const k of ['repo', 'epic', 'run_dir', 'subtasks']) if (!A[k]) throw new Error(`缺 args.${k}`)
const EPIC_DIR = `${A.repo}/.worktrees/${A.epic}`
const SPEC = `${EPIC_DIR}/specs/${A.epic}`
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}（分支 {{BRANCH_PREFIX}}${A.epic}）\nspec：${SPEC}/spec.md、plan.md、progress.md\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。不 push。commit 結尾 {{ATTRIBUTION_LINE}}。\n${METH_NOTE}\n${CODE_STYLE}${STAGE.guidance ? `\nprofile 的 build 指引：${STAGE.guidance}` : ''}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')
const MAX_RETRY = 2

const all = A.subtasks
const ids = new Set(all.map((s) => s.id))
const tests = all.filter((s) => s.kind === 'test')
const work = all.filter((s) => s.kind === 'code' || s.kind === 'doc')
const skipped = all.filter((s) => !['test', 'code', 'doc'].includes(s.kind))
if (skipped.length) log(`不屬於 Build 的子任務不處理：${skipped.map((s) => `${s.id}(${s.kind})`).join('、')}`)

// 依相依分層。測試先行：test 子任務在 Red 先做完，視為已滿足；
// 不測試先行：test 子任務排在實作之後（Tests），code 子任務對 test 子任務的相依一律忽略
const done = new Set(tests.map((s) => s.id))
const layers = []
let rest = work.slice()
while (rest.length) {
  const ready = rest.filter((s) => (s.depends_on || []).every((d) => done.has(d) || !ids.has(d)))
  if (!ready.length) throw new Error(`子任務相依有循環或缺項：${rest.map((s) => s.id).join('、')}`)
  layers.push(ready)
  ready.forEach((s) => done.add(s.id))
  rest = rest.filter((s) => !ready.includes(s))
}
log(`${TEST_FIRST ? 'Red（測試先行）' : 'Tests（實作後補驗證測試）'} ${tests.length} 個；實作分 ${layers.length} 層：${layers.map((l) => l.map((s) => s.id).join('+')).join(' → ')}`)

const taskBrief = (s) => `子任務 ${s.id}「${s.title}」\n擁有（只能改）：${(s.owns || []).join('、')}\n完成條件：${(s.acceptance || []).join('；')}\nStory：${(s.story_ids || []).join('、') || '見 spec'}`
const wtFor = (s) => `${A.repo}/.worktrees/${A.epic}--${s.id}`

function runTask(s, role, phaseName, extra) {
  return (async () => {
    let res = null
    for (let attempt = 1; attempt <= MAX_RETRY + 1; attempt++) {
      res = await agent(
        `${CTX}\n\n## 派工單（${phaseName}，第 ${attempt} 次）\n${taskBrief(s)}\n` +
        `在自己的 worktree 做：git -C "${A.repo}" worktree add .worktrees/${A.epic}--${s.id} -b {{BRANCH_PREFIX}}${A.epic}--${s.id} {{BRANCH_PREFIX}}${A.epic}（已存在就沿用）。worktree：${wtFor(s)}。\n${extra}\n` +
        (attempt > 1 && res ? `上一次的結果（請修正）：${JSON.stringify({ status: res.status, summary: res.summary, open_questions: res.open_questions })}\n` : '') +
        `payload 用 build-result（task_id=${s.id}、branch={{BRANCH_PREFIX}}${A.epic}--${s.id}、worktree=${wtFor(s)}）。`,
        { label: `${phaseName}:${s.id}${attempt > 1 ? `#${attempt}` : ''}`, phase: phaseName, agentType: role, schema: BUILD },
      )
      if (!res || res.status !== 'blocked') break // done 或 needs_decision 都不重試；needs_decision 交人
    }
    return res
  })()
}

async function integrate(batch, label) {
  return agent(
    `${CTX}\n\n## 派工單（整合：${label}）\n依序把這些分支 git merge --no-ff 進 {{BRANCH_PREFIX}}${A.epic}（在 ${EPIC_DIR} 操作）：\n` +
    batch.map((r) => `- ${r.payload.branch}（${r.payload.task_id}）`).join('\n') +
    `\n有衝突照目標 repo 的分支與合併規範用中轉分支解並分類。合完跑 rules.md 專案規則的 verify 指令（基準 ${A.base || 'origin/main'}；沒有就用該 repo 的對等指令），全綠後移除這些子任務的 worktree 與分支。\npayload 用 build-result（task_id=${label}、branch={{BRANCH_PREFIX}}${A.epic}）。`,
    { label: `integrate:${label}`, phase: 'Integrate', agentType: NS + 'dev-integrator', schema: BUILD },
  )
}

const results = { tests: [], layers: [], integrations: [], failures: [] }

// 失敗（null）或沒交 done 的 agent 不能默默丟掉：記進 failures，並以 halted: true 停下，不進下一層。
// save_run.py 讀到 halted 就把 build 段記成 blocked。
const halt = (reason) => ({ stage: 'build', halted: true, halt_reason: reason, gate: null, next: null, ...results })
function settle(subs, res, where) {
  const ok = []
  res.forEach((r, i) => {
    if (r && r.status === 'done') ok.push(r)
    else results.failures.push({ where, task_id: subs[i].id, result: r ? r.status : 'no_result', summary: r ? r.summary : 'agent 沒有回傳結果（失敗、逾時或被中止）' })
  })
  return ok
}
async function integrateOrHalt(ok, label) {
  const ir = await integrate(ok, label)
  results.integrations.push(ir)
  if (ir && ir.status === 'done') return null
  results.failures.push({ where: `integrate:${label}`, task_id: label, result: ir ? ir.status : 'no_result', summary: ir ? ir.summary : 'integrator 沒有回傳結果' })
  return halt(`整合 ${label} 沒完成（${ir ? ir.status : '沒有回傳'}），停下交人`)
}

async function runTests(phaseName, extra, label) {
  const res = await parallel(tests.map((s) => () => runTask(s, NS + 'dev-acceptance-tester', phaseName, extra)))
  results.tests = res
  const ok = settle(tests, res, phaseName)
  if (ok.length) {
    const h = await integrateOrHalt(ok, label)
    if (h) return h
  }
  if (ok.length < tests.length) return halt(`${phaseName} 有 ${tests.length - ok.length} 個子任務沒完成，停下交人`)
  return null
}

if (TEST_FIRST && tests.length) {
  phase('Red')
  const h = await runTests('Red',
    `profile 要求測試先行：依 spec 的驗收條件寫會失敗的測試；跑一次確認「以預期的理由失敗」（tests[].result=red_as_expected）。${commitFor('test', A.epic)}。`, 'red')
  if (h) return h
}

for (let i = 0; i < layers.length; i++) {
  phase('Implement')
  const layer = layers[i]
  const res = await parallel(layer.map((s) => () => runTask(s, NS + 'dev-implementer', 'Implement',
    `${TEST_FIRST ? '讓已合進整合分支的驗收測試轉綠，不得修改驗收測試' : '依 spec 的驗收條件實作（驗證測試在實作後由 dev-acceptance-tester 補）'}；` +
    `單元測試：${STAGE.unit_tests || '依目標 repo 慣例'}；只改擁有的檔案；本子任務範圍的 verify 指令全綠才 commit（${commitFor('feat', A.epic)}）；更新 progress.md 對應項。`)))
  results.layers.push(res)
  const ok = settle(layer, res, `layer${i + 1}`)
  phase('Integrate')
  if (ok.length) {
    const h = await integrateOrHalt(ok, `layer${i + 1}`)
    if (h) return h
  }
  if (ok.length < layer.length) return halt(`第 ${i + 1} 層有 ${layer.length - ok.length} 個子任務沒完成，停下交人`)
}

if (!TEST_FIRST && tests.length) {
  phase('Tests')
  const h = await runTests('Tests',
    `profile 不要求測試先行：實作已合進 {{BRANCH_PREFIX}}${A.epic}，依 spec 的驗收條件補驗證測試，斷言實作的實際行為；跑過並回報結果（tests[].result=pass／fail）。測試失敗代表實作不符驗收條件：不改測試遷就實作，以 needs_decision 回報失敗的條目。${commitFor('test', A.epic)}。`, 'tests')
  if (h) return h
}

return { stage: 'build', halted: false, test_first: TEST_FIRST, gate: null, next: 'sdlc-test', ...results, red: TEST_FIRST ? results.tests : [] }
