export const meta = {
  name: 'sdlc-design',
  description: 'AI-native SDLC 第 2 段 Design：intent 已核准後先出大綱判斷規模；範圍太大就拆成子計劃，多個 dev-spec-author 平行規劃再彙整，否則一人寫 design／spec／plan／progress；拆子任務後兩個（有子計劃時三個）skeptic 平行檢查一致性，停在 spec＋plan 核准閘門',
  whenToUse: 'sdlc-plan 的 intent 核准之後；args 同 sdlc-plan',
  phases: [
    { title: 'Outline', detail: 'dev-spec-author 預估規模，決定要不要拆子計劃（plan-outline）' },
    { title: 'Design', detail: '不拆：dev-spec-author 寫 design、spec、plan、progress 並拆子任務' },
    { title: 'SubPlan', detail: '拆：每個子計劃一個 dev-spec-author 平行規劃（specs/<epic>/sub/<id>/）' },
    { title: 'Consolidate', detail: '拆：dev-spec-author 彙整子計劃、核對介面與檔案所有權、寫總覽並 commit' },
    { title: 'Analyze', detail: 'tpm-skeptic 平行檢查一致性' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const SUBTASKS = /*@schema:subtasks*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["epic","subtasks"],"properties":{"epic":{"type":"string"},"subtasks":{"type":"array","minItems":1,"items":{"type":"object","required":["id","title","kind","owns","depends_on","acceptance"],"properties":{"id":{"type":"string"},"title":{"type":"string"},"kind":{"type":"string","enum":["code","test","doc","research","check","decision"]},"owns":{"type":"array","items":{"type":"string"}},"depends_on":{"type":"array","items":{"type":"string"}},"acceptance":{"type":"array","items":{"type":"string"}},"story_ids":{"type":"array","items":{"type":"string"}},"sub_plan":{"type":"string"}}}}}}}} /*@end*/
const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const OUTLINE = /*@schema:plan-outline*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["epic","split","reason","sub_plans"],"properties":{"epic":{"type":"string"},"split":{"type":"boolean"},"reason":{"type":"string","minLength":1},"estimated_subtasks":{"type":"integer","minimum":1},"sub_plans":{"type":"array","items":{"type":"object","required":["id","title","scope","owns","depends_on"],"properties":{"id":{"type":"string","pattern":"^[a-z0-9][a-z0-9-]*$"},"title":{"type":"string"},"scope":{"type":"string","minLength":1},"owns":{"type":"array","minItems":1,"items":{"type":"string"}},"depends_on":{"type":"array","items":{"type":"string"}},"story_ids":{"type":"array","items":{"type":"string"}},"interfaces":{"type":"array","items":{"type":"string"}}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
const STAGE = ((MP.stages || {}).design) || {}
const CONV = MP.conventions || {}
// 工作流程參數：計劃太大時拆子計劃平行規劃（profile 的 parallelism，省略時用預設值）
const PAR = MP.parallelism || {}
const SPLIT_AT = PAR.split_threshold || 8
const MAX_SUB = PAR.max_sub_plans || 4
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
const SPEC = `${EPIC_DIR}/specs/${A.epic}`
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。intent.md 已由使用者核准，不得修改。\n${METH_NOTE}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')

const TEST_NOTE = ((MP.stages || {}).build || {}).test_first === false ? '這個 profile 不要求測試先行：test 子任務會在實作完成後才做，code 子任務不要依賴 test 子任務。' : ''
const SUBTASK_RULES = `每個子任務獨佔 owns 的檔案（不重疊），寫 depends_on 與可機械判定的 acceptance；kind=test 的子任務擁有測試目錄（例如 e2e/tests 與各服務 tests/）；偏寬淺、沒有相依的子任務會由多個 agent 平行實作，能並行就並行，數量最小化。${TEST_NOTE}`
const DOC_RULES = (dir) =>
  `- 產出文件（放在 ${dir}/）：${(STAGE.artifacts || ['spec.md', 'plan.md']).join('、')}（profile 沒列的不必寫）；架構取捨寫 ADR（至少 2 個真實選項；repo 有 adr skill 就用）。\n` +
  `- 需求／驗收條件的寫法：${STAGE.requirement_style || '照目標 repo 既有的 spec 寫法；每條可機械判定，逐條對到需求 ID，數值與錯誤碼照原文'}。\n` +
  (STAGE.guidance ? `- profile 的指引：${STAGE.guidance}\n` : '')
const COMMIT = `commit（${commitFor('docs', A.epic)}；結尾 {{ATTRIBUTION_LINE}}）。不 push。`
// 使用者加進這一段的自己的 agent（profile 的 extensions，用 /add-agent 設定）：和內建 agent 平行執行，交 findings；
// blocker／important 的發現會擋住政策核准。守門依 guard.config.json 的 extension_agents 套用對應角色的規則
const EXT = (MP.extensions || []).filter((e) => e.stage === 'design')
const extJob = (e, ctx, phaseName) => () => agent(
  `${ctx}\n\n## 派工單（擴充 agent：${e.name}）\n${e.instructions}\n` +
  `只讀不改：不改程式、測試與 spec，不 push、不對 GitHub 寫入。詳細結果寫到 ${A.run_dir}/ext-${e.name}.md。` +
  `payload 用 findings（lens=${e.name}）：每則附 0–1 信心與嚴重度；會擋住上線或合併的列 blocker／important（會擋住政策核准），其餘列 nit／info。`,
  { label: `ext:${e.name}`, phase: phaseName, agentType: e.agent, schema: FINDINGS },
)
const halt = (reason, extra) => ({ stage: 'design', halted: true, halt_reason: reason, gate: null, epic_dir: EPIC_DIR, ...extra })

phase('Outline')
const outline = await agent(
  `${CTX}\n\n## 派工單（SDLC 第 2 段 Design：先出大綱）\n` +
  `讀 ${SPEC}/intent.md 與目標 repo 的影響範圍，預估要拆成幾個子任務、有哪些可以各自交付的區塊。\n` +
  `- 預估超過 ${SPLIT_AT} 個子任務，或有 2 個以上彼此不共用檔案、可以各自規劃的區塊時：split=true，切成 2–${MAX_SUB} 個子計劃（sub plan），之後每個子計劃由一個 agent 平行規劃。\n` +
  `- 每個子計劃寫 scope（負責與不負責什麼）、owns（它的子任務能碰的檔案或目錄；子計劃之間不得重疊，共用的檔案歸給一個子計劃，其他子計劃透過 interfaces 依賴它）、depends_on（只能單向，不得循環）、interfaces（子計劃之間的端點、事件、型別）。\n` +
  `- 範圍夠小就 split=false、sub_plans 留空，由一個人規劃。\n` +
  `- 拆的話把大綱寫到 ${SPEC}/outline.md；這一步不 commit（後面彙整時一起 commit）。\n` +
  `payload 用 plan-outline（epic=${A.epic}）。`,
  { label: 'outline', phase: 'Outline', agentType: NS + 'dev-spec-author', schema: OUTLINE },
)
const SUBS = outline && outline.payload.split ? outline.payload.sub_plans : []

// 子計劃的結構在 workflow 裡先做確定性檢查（和 schemalib 的 plan-outline 語意規則一致）：數量、id、相依、範圍重疊
function outlineProblems(subs) {
  const errs = []
  if (subs.length > MAX_SUB) errs.push(`子計劃 ${subs.length} 個，超過上限 ${MAX_SUB}`)
  const ids = subs.map((s) => s.id)
  ids.filter((x, i) => ids.indexOf(x) !== i).forEach((x) => errs.push(`子計劃 id ${x} 重複`))
  const covers = (a, b) => { a = a.replace(/\/+$/, ''); b = b.replace(/\/+$/, ''); return a === b || b.startsWith(a + '/') }
  subs.forEach((a, i) => {
    ;(a.depends_on || []).forEach((d) => { if (d === a.id || !ids.includes(d)) errs.push(`子計劃 ${a.id} 依賴不存在或是自己：${d}`) })
    subs.slice(i + 1).forEach((b) => (a.owns || []).forEach((pa) => (b.owns || []).forEach((pb) => {
      if (covers(pa, pb) || covers(pb, pa)) errs.push(`子計劃 ${a.id} 與 ${b.id} 的範圍重疊：${pa}／${pb}`)
    })))
  })
  let left = subs.map((s) => s.id)
  while (left.length) {
    const ready = left.filter((id) => (subs.find((s) => s.id === id).depends_on || []).every((d) => !left.includes(d)))
    if (!ready.length) { errs.push(`子計劃相依有循環：${left.join('、')}`); break }
    left = left.filter((id) => !ready.includes(id))
  }
  return errs
}

let design = null
let subDesigns = []
if (!SUBS.length) {
  if (outline) log(`不拆子計劃：${outline.payload.reason}`)
  else log('大綱沒有回傳結果，改由一個 dev-spec-author 規劃')
  phase('Design')
  design = await agent(
    `${CTX}\n\n## 派工單（SDLC 第 2 段 Design）\n依 ${SPEC}/intent.md 與 ${EPIC_DIR}/specs/_templates/：\n` +
    DOC_RULES(SPEC) +
    `- 拆子任務（payload subtasks）：${SUBTASK_RULES}\n- ${COMMIT}\n` +
    `契約不夠用就以 needs_decision 回報，不自己設計新端點。`,
    { label: 'design', phase: 'Design', agentType: NS + 'dev-spec-author', schema: SUBTASKS },
  )
} else {
  const problems = outlineProblems(SUBS)
  if (problems.length) return halt(`子計劃大綱有結構問題，停下交人：${problems.join('；')}`, { outline })
  log(`拆成 ${SUBS.length} 個子計劃平行規劃：${SUBS.map((s) => s.id).join('、')}（${outline.payload.reason}）`)

  phase('SubPlan')
  const subBrief = (s) => `子計劃 ${s.id}「${s.title}」\n範圍：${s.scope}\n可擁有的檔案或目錄：${s.owns.join('、')}\n依賴的子計劃：${(s.depends_on || []).join('、') || '無'}\n介面：${(s.interfaces || []).join('；') || '無'}\nStory：${(s.story_ids || []).join('、') || '見 intent'}`
  subDesigns = await parallel(SUBS.map((s) => () => agent(
    `${CTX}\n\n## 派工單（子計劃規劃：${s.id}）\n${subBrief(s)}\n\n` +
    `完整大綱：${SPEC}/outline.md（其他子計劃同時由別的 agent 規劃，你只負責這一個）。依 ${SPEC}/intent.md 與 ${EPIC_DIR}/specs/_templates/：\n` +
    DOC_RULES(`${SPEC}/sub/${s.id}`) +
    `- 只寫 ${SPEC}/sub/${s.id}/ 底下的檔案；不 commit（多個 agent 共用同一個 worktree，由彙整步驟統一 commit）。\n` +
    `- 拆子任務（payload subtasks）：${SUBTASK_RULES}子任務 id 一律以「${s.id}-」開頭，sub_plan=${s.id}，owns 只能在本子計劃的範圍內。依賴其他子計劃時，depends_on 寫「@<子計劃 id>」（例如 @${(s.depends_on || [])[0] || 'api'}），彙整時會展開成該子計劃的子任務。\n` +
    `- 需要跨子計劃的介面但大綱沒寫時，照大綱的 interfaces 假設並放進 open_questions，不要擅自改別的子計劃的範圍。`,
    { label: `subplan:${s.id}`, phase: 'SubPlan', agentType: NS + 'dev-spec-author', schema: SUBTASKS },
  )))
  const missing = SUBS.filter((s, i) => !subDesigns[i] || subDesigns[i].status !== 'done')
  if (missing.length) return halt(`子計劃 ${missing.map((s) => s.id).join('、')} 沒有規劃完成，停下交人`, { outline, sub_plans: SUBS, sub_designs: subDesigns })

  // 先在 workflow 裡把子任務接起來：展開 @<子計劃> 相依，交給彙整步驟核對
  const draft = []
  SUBS.forEach((s, i) => subDesigns[i].payload.subtasks.forEach((t) => draft.push({ ...t, sub_plan: s.id })))
  const bySub = (id) => draft.filter((t) => t.sub_plan === id).map((t) => t.id)
  draft.forEach((t) => { t.depends_on = [...new Set((t.depends_on || []).flatMap((d) => (d.startsWith('@') ? bySub(d.slice(1)) : [d])))] })

  phase('Consolidate')
  const merged = await agent(
    `${CTX}\n\n## 派工單（彙整子計劃）\n${SUBS.length} 個子計劃已平行規劃完成，文件在 ${SPEC}/sub/<id>/，大綱在 ${SPEC}/outline.md。\n` +
    `1. 核對子計劃之間的介面（interfaces、契約、共用型別）是否一致，檔案所有權有沒有重疊或漏掉的檔案；有落差就修正對應子計劃的文件。\n` +
    `2. 在 ${SPEC}/ 寫總覽：${(STAGE.artifacts || ['spec.md', 'plan.md']).join('、')}（profile 沒列的不必寫）。總覽的 spec 只放跨子計劃的整合驗收條件，並連到各子計劃的 spec；plan 寫子計劃的順序與相依。\n` +
    `3. payload subtasks 交「完整的最終子任務清單」：下面草稿的子任務一個都不能少（可以修正內容；要拿掉必須在 open_questions 寫明子任務 id 與理由），保留 sub_plan 欄位；需要跨子計劃的整合子任務時，id 以「int-」開頭、sub_plan=integration。${SUBTASK_RULES}\n` +
    `4. ${COMMIT}（commit 內容包含 outline.md 與全部子計劃文件。）\n` +
    `契約不夠用就以 needs_decision 回報，不自己設計新端點。\n\n` +
    `## 子計劃\n${SUBS.map(subBrief).join('\n\n')}\n\n## 子任務草稿（相依已展開）\n${JSON.stringify(draft)}`,
    { label: 'consolidate', phase: 'Consolidate', agentType: NS + 'dev-spec-author', schema: SUBTASKS },
  )
  if (merged) {
    // 確定性複核：草稿的子任務不得默默消失，最終清單 id 唯一、owns 不重疊、相依都存在
    const finalIds = merged.payload.subtasks.map((t) => t.id)
    const dropped = draft.filter((t) => !finalIds.includes(t.id)).map((t) => t.id)
    const errs = []
    const explained = JSON.stringify(merged.open_questions || [])
    const silent = dropped.filter((id) => !explained.includes(id))
    if (silent.length) errs.push(`彙整後少了子任務 ${silent.join('、')}，open_questions 沒有說明`)
    finalIds.filter((x, i) => finalIds.indexOf(x) !== i).forEach((x) => errs.push(`子任務 id ${x} 重複`))
    const owner = {}
    merged.payload.subtasks.forEach((t) => {
      ;(t.owns || []).forEach((f) => { if (owner[f] && owner[f] !== t.id) errs.push(`子任務 ${owner[f]} 與 ${t.id} 同時擁有 ${f}`); owner[f] = t.id })
      ;(t.depends_on || []).forEach((d) => { if (!finalIds.includes(d)) errs.push(`子任務 ${t.id} 依賴不存在的 ${d}`) })
    })
    if (errs.length) return halt(`彙整後的子任務清單不一致，停下交人：${errs.join('；')}`, { outline, sub_plans: SUBS, sub_designs: subDesigns, design: merged })
  }
  design = merged
}

phase('Analyze')
const LENSES = [
  { key: 'coverage', ask: `intent 的每個期望結果都有 spec 驗收條件？驗收條件是否照 profile 的需求寫法（${STAGE.requirement_style || '目標 repo 既有寫法'}）、對到需求 ID、原文數值與錯誤碼？spec 有沒有超出 intent 的範圍？` },
  { key: 'plan', ask: `plan 與子任務：每條驗收條件都有任務與測試？子任務的 owns 有沒有重疊、有沒有漏掉 spec 影響系統表外的檔案？depends_on 有沒有循環？引用的 contracts 端點是否真的存在（路徑:行號）？` },
]
if (SUBS.length) LENSES.push({ key: 'sub-plans', ask: `子計劃之間：各子計劃的 spec 合起來有沒有涵蓋 intent 的全部範圍、有沒有重複或互相矛盾的驗收條件？介面（端點、事件、型別）兩邊的寫法一致嗎？跨子計劃的相依有沒有反映在子任務的 depends_on？總覽 spec 的整合驗收條件夠不夠？` })
const checks = design ? await parallel([...LENSES.map((l) => () => agent(
  `${CTX}\n\n## 派工單（一致性檢查：${l.key}）\n讀 ${SPEC}/ 下的 intent、design、spec、plan、progress${SUBS.length ? `、outline.md 與 sub/<id>/ 的子計劃文件` : ''}與子任務：\n${JSON.stringify(design.payload)}\n${l.ask}\n每個問題一則 verdict（target_id 寫文件與節名），refuted=true 表示「這份設計在此處有問題」。`,
  { label: `analyze:${l.key}`, phase: 'Analyze', agentType: NS + 'tpm-skeptic', schema: VERDICTS },
)), ...EXT.map((e) => extJob(e, `${CTX}\n設計文件在 ${SPEC}/（intent、spec、plan${SUBS.length ? '、outline.md、sub/<id>/' : ''}），子任務：${JSON.stringify(design.payload)}`, 'Analyze'))]) : []
const analysis = checks.slice(0, LENSES.length)
const extensions = checks.slice(LENSES.length)

return { stage: 'design', halted: false, gate: 'spec＋plan 核准', epic_dir: EPIC_DIR, outline, sub_plans: SUBS, sub_designs: subDesigns.filter(Boolean), design, analysis: analysis.filter(Boolean), extensions: extensions.filter(Boolean) }
