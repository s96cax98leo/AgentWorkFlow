export const meta = {
  name: 'sdlc-test',
  description: 'AI-native SDLC 第 4 段 Test：verifier 逐條驗收＋反作弊（有子計劃時每個子計劃一個 verifier 平行驗證，再彙整成 verifier.md），同時平行跑設計審查與安全審查；fail 時修正並重驗（最多 2 輪，多個子計劃都有失敗時平行修正再合併），停在驗證結果確認閘門',
  whenToUse: 'sdlc-build 完成後；args 加 has_ui、has_security（布林），有子計劃時加 sub_plans（sdlc-design 回傳的 sub_plans）',
  phases: [
    { title: 'Verify', detail: 'dev-verifier（有子計劃時分片平行）＋dev-design-reviewer＋dev-security-reviewer 平行' },
    { title: 'Consolidate', detail: '分片驗證時：dev-verifier 把各分片報告彙整成 verifier.md，workflow 確定性核對' },
    { title: 'Fix', detail: 'fail 的驗收條件交 dev-implementer 修，最多 2 輪；多個分片都有失敗時各自 worktree 平行修' },
    { title: 'Integrate', detail: '平行修正後 dev-integrator 合併' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const CRITERIA = /*@schema:criteria*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["commit","scope_covered","scope_skipped","criteria"],"properties":{"commit":{"type":"string"},"scope_covered":{"type":"string"},"scope_skipped":{"type":"string"},"criteria":{"type":"array","minItems":1,"items":{"type":"object","required":["id","criterion","command","result","evidence"],"properties":{"id":{"type":"string"},"criterion":{"type":"string"},"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","not_verifiable"]},"evidence":{"type":"string"},"repro":{"type":"string"}}}},"anti_cheat":{"type":"array","items":{"type":"string"}}}}}} /*@end*/
const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const BUILD = /*@schema:build-result*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["task_id","branch","commits","files_changed","tests"],"properties":{"task_id":{"type":"string"},"branch":{"type":"string"},"worktree":{"type":"string"},"commits":{"type":"array","items":{"type":"string"}},"files_changed":{"type":"array","items":{"type":"string"}},"tests":{"type":"array","items":{"type":"object","required":["command","result"],"properties":{"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","red_as_expected","not_run"]},"summary":{"type":"string"}}}},"conflicts":{"type":"array","items":{"type":"object","required":["file","kind","resolution"],"properties":{"file":{"type":"string"},"kind":{"type":"string","enum":["mechanical","seam","judgement","implicit_compile","implicit_behavior","generated"]},"resolution":{"type":"string"}}}}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
// spec 文件的位置與檔名：profile 的 spec_layout（選用，可以只設部分鍵，沒設的用預設）。
// 預設值就是原本寫死的 specs/<epic>/intent.md 等，所以沒設 spec_layout 時派工單一字不變。
const DEFAULT_LAYOUT = { root: 'specs/{epic}', intent: 'intent.md', outline: 'outline.md', spec: 'spec.md', plan: 'plan.md', progress: 'progress.md', verifier: 'verifier.md', verifier_shard: 'verifier-{shard}.md', sub_dir: 'sub', templates: 'specs/_templates' }
const LAYOUT = { ...DEFAULT_LAYOUT, ...(MP.spec_layout || {}) }
const specRoot = (epic) => LAYOUT.root.replace(/\{epic\}/g, () => epic)
const STAGE = ((MP.stages || {}).test) || {}
const CONV = MP.conventions || {}
const PAR = MP.parallelism || {}
const METH_NOTE = METHODOLOGY
  ? `開發規範 profile：${MP.name}（.claude/teams/methodology.json；各段做法與慣例以它為準）。文件格式：${(CONV.doc_format || {}).format || '依目標 repo'}。`
  : '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），做法與慣例依目標 repo 既有規範。'
const commitFor = (kind, epic) => {
  const c = CONV.commit || {}
  const ex = ((c.examples || {})[kind] || '').replace(/<epic>/g, epic)
  return c.format ? `commit 訊息照 profile 的格式：${c.format}${ex ? `（範例：${ex}）` : ''}` : 'commit 訊息照目標 repo 的慣例'
}

const A = args || {}
for (const k of ['repo', 'epic', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const EPIC_DIR = `${A.repo}/.worktrees/${A.epic}`
const SPEC = `${EPIC_DIR}/${specRoot(A.epic)}`
const shardReport = (id) => `${SPEC}/${LAYOUT.verifier_shard.replace(/\{shard\}/g, () => id)}`
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}（分支 {{BRANCH_PREFIX}}${A.epic}，比對基準 ${A.base || 'origin/main'}）\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。\n${METH_NOTE}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')
const MAX_FIX = 2

// 分片驗證：有 2 個以上子計劃時，每個子計劃一個分片，另加一個整合分片（總覽 spec 的跨子計劃驗收條件＋整條 diff 的反作弊）
const SUBS = (A.sub_plans || []).filter((s) => s && s.id)
const SHARDS = SUBS.length >= 2 && PAR.shard_verification !== false
  ? [
      ...SUBS.map((s) => ({ id: s.id, spec: `${SPEC}/${LAYOUT.sub_dir}/${s.id}/${LAYOUT.spec}`, owns: s.owns || [], report: shardReport(s.id) })),
      { id: 'integration', spec: `${SPEC}/${LAYOUT.spec}`, owns: null, report: shardReport('integration') },
    ]
  : null

const HOW = `驗證方式照 profile：${STAGE.verification || '逐條驗證驗收條件（pass／fail／not_verifiable）'}${STAGE.guidance ? `\nprofile 的 test 指引：${STAGE.guidance}` : ''}`
const verifyAll = (round) => agent(
  `${CTX}\n\n## 派工單（驗證，第 ${round} 輪）\n在 ${EPIC_DIR} 的目前 HEAD 驗證 ${SPEC}/${LAYOUT.spec} 的驗收條件。${HOW}\n不論 profile 怎麼寫，都要對 diff 做反作弊檢查，照範本寫 ${SPEC}/${LAYOUT.verifier}（repo 有 verifier 檢查腳本就跑）。payload 用 criteria。`,
  { label: `verify#${round}`, phase: 'Verify', agentType: NS + 'dev-verifier', schema: CRITERIA },
)
const verifyShard = (sh, round) => agent(
  `${CTX}\n\n## 派工單（分片驗證：${sh.id}，第 ${round} 輪）\n其他分片由別的 dev-verifier 同時驗證，你只負責這一片。\n` +
  `在 ${EPIC_DIR} 的目前 HEAD 驗證 ${sh.spec} 的驗收條件（條目 id 照原文，不要改名）。${HOW}\n` +
  (sh.owns
    ? `反作弊檢查只看這個子計劃範圍內的 diff（git diff ${A.base || 'origin/main'}...HEAD -- ${sh.owns.join(' ')}）。\n`
    : `這是整合分片：驗證跨子計劃的整合驗收條件，反作弊檢查看整條 diff（git diff ${A.base || 'origin/main'}...HEAD）。\n`) +
  `報告寫到 ${sh.report}（只寫這個檔）。payload 用 criteria，commit 填驗證當下的 HEAD。`,
  { label: `verify:${sh.id}#${round}`, phase: 'Verify', agentType: NS + 'dev-verifier', schema: CRITERIA },
)

// 分片結果的確定性彙整：條目不能遺失、結果不能被改、各分片要驗同一個 HEAD
function combine(shardRes) {
  const crit = []
  const cheats = []
  shardRes.forEach((r, i) => {
    ;(r.payload.criteria || []).forEach((c) => crit.push({ ...c, shard: SHARDS[i].id }))
    ;(r.payload.anti_cheat || []).forEach((x) => cheats.push(`[${SHARDS[i].id}] ${x}`))
  })
  return { crit, cheats, commits: [...new Set(shardRes.map((r) => r.payload.commit))] }
}
function consolidationProblems(merged, c) {
  const errs = []
  if (c.commits.length > 1) errs.push(`各分片驗的 HEAD 不一致：${c.commits.join('、')}`)
  const got = new Map((merged.payload.criteria || []).map((x) => [x.id, x.result]))
  c.crit.forEach((x) => {
    if (!got.has(x.id)) errs.push(`彙整後少了條目 ${x.id}`)
    else if (got.get(x.id) !== x.result) errs.push(`條目 ${x.id} 的結果被改了（分片 ${x.result}、彙整 ${got.get(x.id)}）`)
  })
  if ((merged.payload.anti_cheat || []).length < c.cheats.length) errs.push(`反作弊發現少了（分片 ${c.cheats.length} 項、彙整 ${(merged.payload.anti_cheat || []).length} 項）`)
  return errs
}

// 使用者加進這一段的自己的 agent（profile 的 extensions，用 /add-agent 設定）：和內建 agent 平行執行，交 findings；
// blocker／important 的發現會擋住政策核准。守門依 guard.config.json 的 extension_agents 套用對應角色的規則
const EXT = (MP.extensions || []).filter((e) => e.stage === 'test')
const extJob = (e, ctx, phaseName) => () => agent(
  `${ctx}\n\n## 派工單（擴充 agent：${e.name}）\n${e.instructions}\n` +
  `只讀不改：不改程式、測試與 spec，不 push、不對 GitHub 寫入。詳細結果寫到 ${A.run_dir}/ext-${e.name}.md。` +
  `payload 用 findings（lens=${e.name}）：每則附 0–1 信心與嚴重度；會擋住上線或合併的列 blocker／important（會擋住政策核准），其餘列 nit／info。`,
  { label: `ext:${e.name}`, phase: phaseName, agentType: e.agent, schema: FINDINGS },
)

const shardRuns = []
async function verify(round) {
  if (!SHARDS) {
    const v = await verifyAll(round)
    return { combined: v, problem: v ? null : '驗證沒有回傳結果' }
  }
  const res = await parallel(SHARDS.map((sh) => () => verifyShard(sh, round)))
  shardRuns.push({ round, shards: res })
  const lost = SHARDS.filter((sh, i) => !res[i])
  if (lost.length) return { combined: null, problem: `分片 ${lost.map((s) => s.id).join('、')} 沒有回傳驗證結果` }
  const c = combine(res)
  phase('Consolidate')
  const merged = await agent(
    `${CTX}\n\n## 派工單（彙整分片驗證，第 ${round} 輪）\n${SHARDS.length} 個分片已平行驗證完成，報告：${SHARDS.map((s) => s.report).join('、')}。\n` +
    `把它們彙整成 ${SPEC}/${LAYOUT.verifier}（照範本；每條註明出自哪個分片）。不重新判定結果：條目與 result 照分片原樣，反作弊發現全部列入；只有各分片的 HEAD 不一致時以 needs_decision 回報。payload 用 criteria（commit 填各分片共同的 HEAD）。\n\n` +
    `## 分片結果\n${JSON.stringify({ commits: c.commits, criteria: c.crit, anti_cheat: c.cheats })}`,
    { label: `consolidate#${round}`, phase: 'Consolidate', agentType: NS + 'dev-verifier', schema: CRITERIA },
  )
  if (!merged) return { combined: null, problem: '彙整分片驗證沒有回傳結果' }
  const errs = consolidationProblems(merged, c)
  return { combined: merged, problem: errs.length ? `彙整結果和分片不一致：${errs.join('；')}` : null, crit: c.crit }
}

phase('Verify')
const jobs = [() => verify(1)]
if (A.has_ui) jobs.push(() => agent(`${CTX}\n\n## 派工單\n設計審查 {{BRANCH_PREFIX}}${A.epic} 的畫面改動，寫 ${SPEC}/design-review.md 與 ${SPEC}/evidence/。payload 用 findings（lens=design）。`, { label: 'design-review', phase: 'Verify', agentType: NS + 'dev-design-reviewer', schema: FINDINGS }))
if (A.has_security) jobs.push(() => agent(`${CTX}\n\n## 派工單\n安全審查 {{BRANCH_PREFIX}}${A.epic}（git diff ${A.base || 'origin/main'}...HEAD），寫 ${SPEC}/security-review.md。payload 用 findings（lens=security）。`, { label: 'security-review', phase: 'Verify', agentType: NS + 'dev-security-reviewer', schema: FINDINGS }))
EXT.forEach((e) => jobs.push(extJob(e, `${CTX}\n驗證對象：${EPIC_DIR} 的目前 HEAD（git diff ${A.base || 'origin/main'}...HEAD），spec 在 ${SPEC}/。`, 'Verify')))
const [first, ...reviews] = await parallel(jobs)

const fixes = []
const finish = (current, extra) => {
  const summary = current ? (current.payload.criteria || []).reduce((m, c) => (m[c.result] = (m[c.result] || 0) + 1, m), {}) : null
  return { stage: 'test', gate: '驗證結果確認（fail 或 not_verifiable 由人決定）', verifier: current, verifier_shards: shardRuns, reviews: reviews.filter(Boolean), fixes, summary, ...extra }
}
if (!first) return finish(null, { halted: true, halt_reason: '驗證沒有回傳結果，停下交人' })
if (first.problem) return finish(first.combined, { halted: true, halt_reason: first.problem })

const fixBrief = (failed, cheats) => `測試失敗就改程式、不改測試，不加 skip／retry、不放寬斷言。\n失敗：${JSON.stringify(failed)}\n反作弊發現：${JSON.stringify(cheats)}`
async function fixSerial(round, failed, cheats) {
  return agent(
    `${CTX}\n\n## 派工單（修正第 ${round} 輪）\n在 ${EPIC_DIR}（{{BRANCH_PREFIX}}${A.epic}）修正下列失敗；${fixBrief(failed, cheats)}\n修完 commit（${commitFor('fix', A.epic)}）。\npayload 用 build-result（task_id=fix${round}、branch={{BRANCH_PREFIX}}${A.epic}、worktree=${EPIC_DIR}）。`,
    { label: `fix#${round}`, phase: 'Fix', agentType: NS + 'dev-implementer', schema: BUILD },
  )
}
// 多個子計劃都有失敗：各自在 worktree 平行修（只改該子計劃範圍），再由 integrator 合併；整合分片的失敗跨子計劃，合併後再單獨修
async function fixParallel(round, groups) {
  const res = await parallel(groups.map((g) => () => {
    const br = `{{BRANCH_PREFIX}}${A.epic}--fix${round}-${g.id}`
    const wt = `${A.repo}/.worktrees/${A.epic}--fix${round}-${g.id}`
    return agent(
      `${CTX}\n\n## 派工單（修正第 ${round} 輪，子計劃 ${g.id}）\n其他子計劃的失敗由別的 agent 同時修，你只負責這一個。\n` +
      `在自己的 worktree 修：git -C "${A.repo}" worktree add .worktrees/${A.epic}--fix${round}-${g.id} -b ${br} {{BRANCH_PREFIX}}${A.epic}（已存在就沿用）。worktree：${wt}。\n` +
      `只能改這些範圍：${g.owns.join('、')}。修正需要改範圍外的檔案時不要改，以 needs_decision 回報。\n${fixBrief(g.failed, g.cheats)}\n` +
      `修完 commit（${commitFor('fix', A.epic)}）。payload 用 build-result（task_id=fix${round}-${g.id}、branch=${br}、worktree=${wt}）。`,
      { label: `fix:${g.id}#${round}`, phase: 'Fix', agentType: NS + 'dev-implementer', schema: BUILD },
    )
  }))
  fixes.push(...res)
  const ok = res.filter((r) => r && r.status === 'done')
  if (ok.length < groups.length) return null
  phase('Integrate')
  const ir = await agent(
    `${CTX}\n\n## 派工單（整合修正第 ${round} 輪）\n依序把這些分支 git merge --no-ff 進 {{BRANCH_PREFIX}}${A.epic}（在 ${EPIC_DIR} 操作）：\n` +
    ok.map((r) => `- ${r.payload.branch}（${r.payload.task_id}）`).join('\n') +
    `\n有衝突照目標 repo 的分支與合併規範用中轉分支解並分類。合完跑 rules.md 專案規則的 verify 指令（基準 ${A.base || 'origin/main'}；沒有就用該 repo 的對等指令），全綠後移除這些修正的 worktree 與分支。\npayload 用 build-result（task_id=fix${round}、branch={{BRANCH_PREFIX}}${A.epic}）。`,
    { label: `integrate:fix#${round}`, phase: 'Integrate', agentType: NS + 'dev-integrator', schema: BUILD },
  )
  fixes.push(ir)
  return ir
}

let state = first
for (let round = 1; round <= MAX_FIX && state.combined; round++) {
  const current = state.combined
  const failed = (current.payload.criteria || []).filter((c) => c.result === 'fail')
  const cheats = current.payload.anti_cheat || []
  if (!failed.length && !cheats.length) break
  phase('Fix')
  log(`第 ${round} 輪修正：fail ${failed.length} 條、反作弊 ${cheats.length} 項`)
  let fix
  // 依分片分組：只有子計劃分片（不含整合分片）的失敗、且分散在 2 個以上子計劃時才平行修
  const shardOf = new Map((state.crit || []).map((c) => [c.id, c.shard]))
  const groups = SHARDS ? SHARDS.filter((sh) => sh.owns).map((sh) => ({
    id: sh.id, owns: sh.owns,
    failed: failed.filter((c) => shardOf.get(c.id) === sh.id),
    cheats: cheats.filter((x) => x.startsWith(`[${sh.id}]`)),
  })).filter((g) => g.failed.length || g.cheats.length) : []
  const rest = SHARDS ? { failed: failed.filter((c) => !groups.some((g) => g.failed.includes(c))), cheats: cheats.filter((x) => !groups.some((g) => g.cheats.includes(x))) } : null
  if (groups.length >= 2) {
    log(`平行修正 ${groups.length} 個子計劃：${groups.map((g) => g.id).join('、')}`)
    fix = await fixParallel(round, groups)
    if (fix && fix.status === 'done' && (rest.failed.length || rest.cheats.length)) {
      fix = await fixSerial(round, rest.failed, rest.cheats)
      fixes.push(fix)
    }
  } else {
    fix = await fixSerial(round, failed, cheats)
    fixes.push(fix)
  }
  if (!fix || fix.status !== 'done') break
  phase('Verify')
  state = await verify(round + 1)
  if (state.problem) return finish(state.combined, { halted: true, halt_reason: state.problem })
}

return finish(state.combined, { halted: false })
