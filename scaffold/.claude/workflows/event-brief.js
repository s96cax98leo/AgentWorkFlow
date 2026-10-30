export const meta = {
  name: 'event-brief',
  description: '處理一份簡報（有任務編號的任務）：tpm-planner 拆工 → 非程式子任務平行處理（程式子任務列出交 SDLC）→ skeptic 反駁 → writer 產出三件套 → 對簡報 §4 驗收標準自我驗收',
  whenToUse: '{{TASK_AUTHORITY}} 發來一份 {{BRIEFS_DIR}}/{{TASK_ID_PREFIX}}<編號>_*.md；args：brief、run_dir',
  phases: [
    { title: 'Plan', detail: 'tpm-planner 拆子任務' },
    { title: 'Work', detail: '依相依分層平行處理（tpm-researcher／tpm-upstream-analyst）' },
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
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","proposal","tasks","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/
const CRITERIA = /*@schema:criteria*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["commit","scope_covered","scope_skipped","criteria"],"properties":{"commit":{"type":"string"},"scope_covered":{"type":"string"},"scope_skipped":{"type":"string"},"criteria":{"type":"array","minItems":1,"items":{"type":"object","required":["id","criterion","command","result","evidence"],"properties":{"id":{"type":"string"},"criterion":{"type":"string"},"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","not_verifiable"]},"evidence":{"type":"string"},"repro":{"type":"string"}}}},"anti_cheat":{"type":"array","items":{"type":"string"}}}}}} /*@end*/

const A = args || {}
for (const k of ['brief', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `簡報（唯一指示來源）：${A.brief}\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}＋簡報 §1。不自編任務編號；衍生工作放 derived_work。`
const brief = (e) => e ? JSON.stringify({ status: e.status, summary: e.summary, artifacts: e.artifacts, open_questions: e.open_questions }) : 'null'

phase('Plan')
const plan = await agent(`${CTX}\n\n## 派工單\n讀簡報全文，拆成子任務（payload subtasks，epic 用簡報短名）。kind=code 的子任務只列出，標明「轉 SDLC」，本 workflow 不實作。不可逆節點（推 fork、寫上游、開 PR、刪資料、對外寄送）標在 acceptance。`,
  { label: 'plan', phase: 'Plan', agentType: NS + 'tpm-planner', schema: SUBTASKS })
if (!plan || plan.status !== 'done') return { stage: 'plan', halted: '拆工沒完成或有待決問題', plan }

const subtasks = plan.payload.subtasks
const toSdlc = subtasks.filter((s) => s.kind === 'code' || s.kind === 'test')
const work = subtasks.filter((s) => !toSdlc.includes(s))
const done = new Set(toSdlc.map((s) => s.id))
const layers = []
let rest = work.slice()
while (rest.length) {
  const ready = rest.filter((s) => (s.depends_on || []).every((d) => done.has(d)))
  if (!ready.length) throw new Error(`相依有循環：${rest.map((s) => s.id).join('、')}`)
  layers.push(ready); ready.forEach((s) => done.add(s.id)); rest = rest.filter((s) => !ready.includes(s))
}
log(`子任務 ${subtasks.length} 個：轉 SDLC ${toSdlc.length}、本流程分 ${layers.length} 層`)

const results = []
for (const layer of layers) {
  phase('Work')
  const prior = results.map(brief).join('\n')
  const out = await parallel(layer.map((s) => () => agent(
    `${CTX}\n\n## 派工單（子任務 ${s.id}：${s.title}，kind=${s.kind}）\n完成條件：${(s.acceptance || []).join('；')}\n可寫：${A.run_dir}/${s.id}-*.md。\n前面子任務的結果：\n${prior}\npayload 用 findings（lens=${s.kind}）。`,
    { label: `${s.kind}:${s.id}`, phase: 'Work', agentType: /上游|upstream|PR|衝突|merge/i.test(s.title) ? NS + 'tpm-upstream-analyst' : NS + 'tpm-researcher', schema: FINDINGS },
  )))
  results.push(...out.filter(Boolean))
}

phase('Challenge')
const claims = results.flatMap((r) => r.payload.findings || [])
const challenge = claims.length ? await agent(`${CTX}\n\n## 派工單\n逐則試著反駁下列發現（打開 location 查證），不確定就判 refuted=true：\n${JSON.stringify(claims)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
const refuted = new Set((challenge && challenge.payload.verdicts || []).filter((v) => v.refuted).map((v) => v.target_id))

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n依簡報 §3 指定的產出路徑寫文件（通常是給決策者版、research 版、執行紀錄三件套；執行紀錄照 {{BRIEFS_DIR}}/*_執行紀錄.md 的格式，沒有就用：產出、驗證、跟計畫不同的地方、刻意沒做的事、未能確認、衍生工作）。只用未被推翻的發現；被推翻的列在「未能確認」。轉 SDLC 的子任務列在執行紀錄並說明下一步。每份跑 check_rules.py（給決策者的加 --audience owner）。\n子任務：${JSON.stringify(subtasks)}\n結果：${results.map(brief).join('\n')}\n被推翻：${JSON.stringify([...refuted])}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

phase('Accept')
const accept = docs ? await agent(`${CTX}\n\n## 派工單\n對簡報 §4（驗收標準）逐條自我驗收：讀 ${JSON.stringify(docs.artifacts)}，每條給 pass／fail／not_verifiable 與證據。commit 填 n/a。payload 用 criteria。`,
  { label: 'accept', phase: 'Accept', agentType: NS + 'tpm-skeptic', schema: CRITERIA }) : null

return { event: 'brief', plan, to_sdlc: toSdlc, results, challenge, docs, accept }
