export const meta = { name: 'my-waves', description: '分兩波打地基：agent() 這個字寫在字串裡沒關係' }

// ── 以下由 awf 接上（自訂腳本，build 段）：執行核心、pattern 庫、這一段的參數。不要手改 ──
const NS = "agent-workflow:"
const A = Object.assign({"run_dir":"_runs/r1","epic":"demo","recipe":"feature","modifiers":[]}, args || {})
const METHODOLOGY = «METHODOLOGY»
const MP = METHODOLOGY || {}
// ── 片段 recipe ──
«片段 recipe»
// ── 片段 challenge ──
«片段 challenge»
// ── 片段 layers ──
«片段 layers»
// ── 片段 kernel ──
«片段 kernel»
// ── 片段 patterns ──
«片段 patterns»
const R = resolveRecipe(METHODOLOGY, A.recipe, [].concat(A.modifiers || []), A.size)
if (R.errors.length) return { stage: "build", halted: true, halt_reason: 'recipe 解析有錯：' + R.errors.join('；'), missing: [], recipe: { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages }, trace: [] }
const RECIPE = { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages, summary: recipeSummary(R) }
awfInit({ run_dir: A.run_dir, stage: "build", lang: "繁體中文", rules: '.claude/teams/rules.md', constraints: awfConstraints(MP, "build") })
awfKernelInit({ profile: METHODOLOGY, stage: "build", modifiers: [].concat(A.modifiers || []), max_agents: 40, roles: ["agent-workflow:tpm-planner","agent-workflow:tpm-researcher","agent-workflow:tpm-reviewer","agent-workflow:tpm-skeptic","agent-workflow:tpm-writer","agent-workflow:dev-spec-author","agent-workflow:dev-acceptance-tester","agent-workflow:dev-implementer","agent-workflow:dev-integrator","agent-workflow:dev-verifier","agent-workflow:dev-design-reviewer","agent-workflow:dev-security-reviewer","agent-workflow:dev-release-watcher"] })

// ── 自訂腳本的內容（包成函式：遮蔽 agent 等名字；回傳的 trace 由外層取自執行核心）──
const __awfBody = async (agent, tryAgent, workflow, awfState, awfKernel, awfInit, awfKernelInit, awfSetModifiers, awfSetModelPlan, awfDispatch) => {
const SUBTASKS = /*@schema:subtasks*/ «schema» /*@end*/
const waves = [['t1'], ['t2', 't3']]
const done = []
for (const [i, wave] of waves.entries()) {
  const r = await awfFanOut(wave.map((id) => ({ prompt: `做 ${id}（不要 while 迴圈）`, label: `Implement:${id}`, agentType: NS + 'dev-implementer', schema: SUBTASKS })), { max: 4 })
  if (r.halted) return { stage: 'build', halted: true, halt_reason: r.reason, missing: r.missing, recipe: RECIPE, trace: awfTrace() }
  done.push(...r.results)
}
for (let i = 0; i < 2; i++) { done.push(i) }
const pick = (xs) => { return xs[0] }
const ext = await awfAgent('看一下', { label: 'ext', agentType: NS + 'tpm-reviewer', schema: SUBTASKS })
return {
  stage: 'build', halted: false, results: done, first: pick(done), ext,
  recipe: RECIPE,
  trace: awfTrace(),
}
}
const __awfOut = await __awfBody()
if (!__awfOut || typeof __awfOut !== 'object' || Array.isArray(__awfOut)) return { stage: "build", halted: true, halt_reason: '自訂腳本沒有回傳物件', missing: [], recipe: RECIPE, trace: awfTrace() }
return { ...__awfOut, stage: "build", recipe: RECIPE, trace: awfTrace() }
