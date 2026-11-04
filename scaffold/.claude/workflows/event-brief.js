export const meta = {
  name: 'event-brief',
  description: '處理一份簡報（有任務編號的任務）：tpm-planner 拆工 → 非程式子任務平行處理（程式與測試子任務列出交 SDLC，依賴它們的子任務等 SDLC 完成）→ skeptic 反駁 → writer 產出三件套 → 對簡報 §4 驗收標準自我驗收',
  whenToUse: '{{TASK_AUTHORITY}} 發來一份 {{BRIEFS_DIR}}/{{TASK_ID_PREFIX}}<編號>_*.md；args：brief、run_dir',
  phases: [
    { title: 'Plan', detail: 'tpm-planner 拆子任務' },
    { title: 'Work', detail: '依相依分層平行處理（tpm-researcher；上游相關的子任務用上游模式）' },
    { title: 'Challenge', detail: 'tpm-skeptic 反駁' },
    { title: 'Write', detail: 'tpm-writer 三件套' },
    { title: 'Accept', detail: '對簡報 §4 逐條自我驗收' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const SUBTASKS = /*@schema:subtasks*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["epic","subtasks"],"properties":{"epic":{"type":"string"},"subtasks":{"type":"array","minItems":1,"items":{"type":"object","required":["id","title","kind","owns","depends_on","acceptance"],"properties":{"id":{"type":"string"},"title":{"type":"string"},"kind":{"type":"string","enum":["code","test","doc","research","check","decision"]},"owns":{"type":"array","items":{"type":"string"}},"depends_on":{"type":"array","items":{"type":"string"}},"acceptance":{"type":"array","items":{"type":"string"}},"story_ids":{"type":"array","items":{"type":"string"}},"sub_plan":{"type":"string"}}}}}}}} /*@end*/
const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/
const CRITERIA = /*@schema:criteria*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["commit","scope_covered","scope_skipped","criteria"],"properties":{"commit":{"type":"string"},"scope_covered":{"type":"string"},"scope_skipped":{"type":"string"},"criteria":{"type":"array","minItems":1,"items":{"type":"object","required":["id","criterion","command","result","evidence"],"properties":{"id":{"type":"string"},"criterion":{"type":"string"},"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","not_verifiable"]},"evidence":{"type":"string"},"repro":{"type":"string"}}}},"anti_cheat":{"type":"array","items":{"type":"string"}}}}}} /*@end*/

const A = args || {}
for (const k of ['brief', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `簡報（唯一指示來源）：${A.brief}\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}＋簡報 §1。不自編任務編號；衍生工作放 derived_work。`
const brief = (e) => e ? JSON.stringify({ status: e.status, summary: e.summary, artifacts: e.artifacts, open_questions: e.open_questions }) : 'null'
// 規則 26：agent 沒交件記為 missing，不當成空清單或「沒有被推翻」；缺件一律 halted，halt_reason 寫出 label
const missing = []
const halt = (stage, extra = {}) => ({ event: 'brief', halted: true, stage, halt_reason: `缺件：${missing.join('、')}`, missing: [...missing], ...extra })
// 寫程式的子任務（code、test）轉 SDLC，本 workflow 不實作；和 tpm-planner 的 kind 定義一致
const SDLC_KINDS = ['code', 'test']

phase('Plan')
const plan = await agent(`${CTX}\n\n## 派工單\n讀簡報全文，拆成子任務（payload subtasks，epic 用簡報短名）。kind=code 與 kind=test 的子任務只列出，標明「轉 SDLC」，本 workflow 不實作；依賴它們的其他子任務會等 SDLC 完成後再跑。不可逆節點（推 fork、寫上游、開 PR、刪資料、對外寄送）標在 acceptance。`,
  { label: 'plan', phase: 'Plan', agentType: NS + 'tpm-planner', schema: SUBTASKS })
if (!plan) { missing.push('plan'); return halt('plan') }
if (plan.status !== 'done') return { event: 'brief', stage: 'plan', halted: true, halt_reason: '拆工沒完成或有待決問題', plan }

const subtasks = plan.payload.subtasks || []
const ids = new Set(subtasks.map((s) => s.id))
const unknown = subtasks.flatMap((s) => (s.depends_on || []).filter((d) => !ids.has(d)).map((d) => `${s.id}→${d}`))
if (unknown.length) return { event: 'brief', stage: 'plan', halted: true, halt_reason: `相依指向不存在的子任務：${unknown.join('、')}`, plan }
const toSdlc = subtasks.filter((s) => SDLC_KINDS.includes(s.kind))
const sdlcIds = new Set(toSdlc.map((s) => s.id))
const work = subtasks.filter((s) => !sdlcIds.has(s.id))
// 直接或間接依賴轉 SDLC 子任務的，SDLC 還沒完成前不能跑：標為 waiting，列在回傳
const waitIds = new Set()
for (let changed = true; changed;) {
  changed = false
  for (const s of work) if (!waitIds.has(s.id) && (s.depends_on || []).some((d) => sdlcIds.has(d) || waitIds.has(d))) { waitIds.add(s.id); changed = true }
}
const waiting = work.filter((s) => waitIds.has(s.id))
const runnable = work.filter((s) => !waitIds.has(s.id))
const done = new Set()
const layers = []
let rest = runnable.slice()
while (rest.length) {
  const ready = rest.filter((s) => (s.depends_on || []).every((d) => done.has(d)))
  if (!ready.length) throw new Error(`相依有循環：${rest.map((s) => s.id).join('、')}`)
  layers.push(ready); ready.forEach((s) => done.add(s.id)); rest = rest.filter((s) => !ready.includes(s))
}
log(`子任務 ${subtasks.length} 個：轉 SDLC ${toSdlc.length}、等 SDLC ${waiting.length}、本流程分 ${layers.length} 層`)

// 上游相關的子任務：tpm-researcher 用上游模式（merge-tree 衝突預測、影響範圍、時間線）
const UPSTREAM_RE = /上游|upstream|PR|衝突|merge/i
const results = []
const claims = []
for (const layer of layers) {
  phase('Work')
  const prior = results.map(brief).join('\n')
  const out = await parallel(layer.map((s) => () => agent(
    `${CTX}\n\n## 派工單（子任務 ${s.id}：${s.title}，kind=${s.kind}${UPSTREAM_RE.test(s.title) ? '；上游模式' : ''}）\n完成條件：${(s.acceptance || []).join('；')}\n可寫：${A.run_dir}/${s.id}-*.md。\n前面子任務的結果：\n${prior}\npayload 用 findings（lens=${s.kind}）。`,
    { label: `work:${s.kind}:${s.id}`, phase: 'Work', agentType: NS + 'tpm-researcher', schema: FINDINGS },
  )))
  out.forEach((r, i) => {
    const s = layer[i]
    if (!r) { missing.push(`work:${s.kind}:${s.id}`); return }
    results.push(r)
    // 各子任務的發現 id 各自編號，加子任務前綴（<子任務 id>:<id>），skeptic 的 target_id 才不會撞號
    for (const f of r.payload.findings || []) claims.push({ ...f, id: `${s.id}:${f.id}` })
  })
  // 後面的層依賴這一層：缺件就停
  if (missing.length) return halt('work', { plan, to_sdlc: toSdlc, waiting, results })
}

phase('Challenge')
const challenge = claims.length ? await agent(`${CTX}\n\n## 派工單\n逐則試著反駁下列發現（打開 location 查證），不確定就判 refuted=true。每則都要判，target_id 照抄發現的 id（含子任務前綴）：\n${JSON.stringify(claims)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
if (claims.length && !challenge) { missing.push('challenge'); return halt('challenge', { plan, to_sdlc: toSdlc, waiting, results }) }
// skeptic 沒判到的發現當成未驗證，和被推翻的一起列「未能確認」
const v = new Map((challenge ? challenge.payload.verdicts || [] : []).map((x) => [x.target_id, x]))
const refuted = claims.filter((f) => !v.has(f.id) || v.get(f.id).refuted).map((f) => f.id)

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n依簡報 §3 指定的產出路徑寫文件（通常是給決策者版、research 版、執行紀錄三件套；執行紀錄照 {{BRIEFS_DIR}}/*_執行紀錄.md 的格式，沒有就用：產出、驗證、跟計畫不同的地方、刻意沒做的事、未能確認、衍生工作）。只用未被推翻的發現；被推翻或沒被驗證的列在「未能確認」。轉 SDLC 的子任務列在執行紀錄並說明下一步；等 SDLC 的子任務列為「等待中」，寫明要等哪些 SDLC 子任務完成後再跑。每份跑 check_rules.py（給決策者的加 --audience owner）。\n子任務：${JSON.stringify(subtasks)}\n等 SDLC：${JSON.stringify(waiting.map((s) => s.id))}\n結果：${results.map(brief).join('\n')}\n未能確認：${JSON.stringify(refuted)}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })
if (!docs) { missing.push('write'); return halt('write', { plan, to_sdlc: toSdlc, waiting, results, challenge }) }

phase('Accept')
const accept = await agent(`${CTX}\n\n## 派工單\n對簡報 §4（驗收標準）逐條自我驗收：讀 ${JSON.stringify(docs.artifacts)}，每條給 pass／fail／not_verifiable 與證據；等 SDLC 才能完成的條目判 not_verifiable 並註明。commit 填 n/a。payload 用 criteria。`,
  { label: 'accept', phase: 'Accept', agentType: NS + 'tpm-skeptic', schema: CRITERIA })
if (!accept) { missing.push('accept'); return halt('accept', { plan, to_sdlc: toSdlc, waiting, results, challenge, docs }) }

return {
  event: 'brief', plan, to_sdlc: toSdlc, waiting, results, challenge, docs, accept,
  ...(waiting.length ? { next: `子任務 ${waiting.map((s) => s.id).join('、')} 依賴轉 SDLC 的子任務，等 SDLC 合併後重跑 event-brief（或主 session 另派）處理。` } : {}),
}
