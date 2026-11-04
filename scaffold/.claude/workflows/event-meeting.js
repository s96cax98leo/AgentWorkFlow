export const meta = {
  name: 'event-meeting',
  description: '一場會議落盤：交辦、決議、風險與依賴、要更新的文件、給 {{TASK_AUTHORITY}} 的待辦五個角度平行抽取 → skeptic 對照原文 → writer 寫會議摘要與待辦清單',
  whenToUse: '會議紀錄或逐字稿進來；args：source（會議檔路徑）、run_dir',
  phases: [{ title: 'Extract', detail: '五個角度平行（tpm-researcher）' }, { title: 'Challenge' }, { title: 'Write' }],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const ITEMS = /*@schema:items*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["source","items"],"properties":{"source":{"type":"string"},"items":{"type":"array","items":{"type":"object","required":["id","type","text","owner","status","refs"],"properties":{"id":{"type":"string"},"type":{"type":"string","enum":["action","decision","risk","issue","dependency","doc_update","impact","change","check","todo"]},"text":{"type":"string"},"owner":{"type":"string"},"due":{"type":"string"},"status":{"type":"string","enum":["open","done","waiting","red","yellow","green","n_a"]},"refs":{"type":"array","items":{"type":"string"}},"target_files":{"type":"array","items":{"type":"string"}}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['source', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `會議來源：${A.source}\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。每個事項都附原文位置（路徑:行號或時間戳）；不自編任務編號；不評價人。`
// 規則 26：agent 沒交件記為 missing，不當成空清單或「沒有被推翻」；缺件一律 halted，halt_reason 寫出 label
/*@snippet:challenge*/
// ── 片段 challenge（.claude/teams/tools/snippets/challenge.js）：skeptic 的 verdicts 對回事項，判定哪些被推翻、哪些沒驗證到 ──
// build.py 內嵌；不要在 workflow 裡手改，改片段後重跑 build.py。只定義函式，需要的值都用參數傳入。
// 缺件（rules.md 規則 26）：agent 回 null 或交件無效時把 label 記進 missing；
// 反駁類缺件不是「全部沒被推翻」，呼叫端要回 halted:true，halt_reason 寫出 label（haltMissing）。

// 缺件時的回傳：{ ...base, halted: true, stage, halt_reason: '缺件：<label>、…', missing, ...extra }
function haltMissing(base, stage, missing, extra = {}) {
  return { ...base, halted: true, stage, halt_reason: `缺件：${missing.join('、')}`, missing: [...missing], ...extra }
}

// 把 verdicts（target_id 對事項 id）對回 list。
// 預設：skeptic 沒判到的事項當成未驗證，和被推翻的一起列 doubted；同一 target_id 判了多次時以最後一則為準。
// opts.unansweredKept＝true：只有明確 refuted 的才列 doubted（沒判到的保留）；同一 target_id 判了多次時，每一則都 refuted 才算推翻（只會更嚴）。
// 回 { kept, doubted, unanswered }，unanswered 是沒被判到的事項 id。
function judgeVerdicts(list, challenge, opts = {}) {
  const verdicts = (((challenge || {}).payload || {}).verdicts) || []
  const last = new Map(verdicts.map((x) => [x.target_id, x]))
  const upheld = new Set(verdicts.filter((x) => !x.refuted).map((x) => x.target_id))
  const ok = opts.unansweredKept
    ? (i) => !last.has(i.id) || upheld.has(i.id)
    : (i) => last.has(i.id) && !last.get(i.id).refuted
  return { kept: list.filter(ok), doubted: list.filter((i) => !ok(i)), unanswered: list.filter((i) => !last.has(i.id)).map((i) => i.id) }
}

// 派一個 skeptic 反駁整批事項，再用 judgeVerdicts 判定。list 是空的就不派（challenge=null、lacking=false）。
// o：{ prompt, label='challenge', phase='Challenge', agentType, schema, missing（缺件 label 記進這個陣列）, unansweredKept }
// skeptic 缺件：label 記進 o.missing，回 lacking=true（kept 為空、全部列 doubted），呼叫端要 halted。
async function challengeList(list, o) {
  const label = o.label || 'challenge'
  if (!list.length) return { challenge: null, lacking: false, ...judgeVerdicts(list, null, o) }
  const challenge = await agent(o.prompt, { label, phase: o.phase || 'Challenge', agentType: o.agentType, schema: o.schema })
  if (!challenge || !challenge.payload) {
    if (o.missing) o.missing.push(label)
    return { challenge: null, lacking: true, kept: [], doubted: list.slice(), unanswered: list.map((i) => i.id) }
  }
  return { challenge, lacking: false, ...judgeVerdicts(list, challenge, o) }
}
/*@end*/
const missing = []
const halt = (stage, extra = {}) => haltMissing({ event: 'meeting' }, stage, missing, extra)
const ANGLES = [
  { key: 'action', type: 'action', ask: '交辦：誰、做什麼、期限（原文沒說期限就留空）。' },
  { key: 'decision', type: 'decision', ask: '已拍板的決議與由誰拍板；只列明確拍板的，討論中的不算。' },
  { key: 'risk', type: 'risk／dependency／issue', ask: '風險、依賴、問題；標影響範圍。' },
  { key: 'doc', type: 'doc_update', ask: '因這場會議需要更新的既有文件（專案文件、對外交付文件、規格、backlog），target_files 寫路徑，text 寫要改什麼。' },
  { key: 'todo', type: 'todo', ask: '要交給 {{TASK_AUTHORITY}} 編號的新工作（不編號），和既有 backlog 重複的標出既有 id。' },
]

phase('Extract')
const extracted = await pipeline(ANGLES, (a) => agent(`${CTX}\n\n## 派工單（${a.key}）\n${a.ask}\n只抽 type=${a.type} 的事項。payload 用 items（source=${A.source}）。`,
  { label: `extract:${a.key}`, phase: 'Extract', agentType: NS + 'tpm-researcher', schema: ITEMS }))
// 各角度的事項 id 各自編號，合併前加角度前綴（<角度>:<id>），skeptic 的 target_id 才不會撞號
const all = []
extracted.forEach((r, i) => {
  const k = ANGLES[i].key
  if (!r) missing.push(`extract:${k}`)
  else for (const it of r.payload.items || []) all.push({ ...it, id: `${k}:${it.id}` })
})
if (missing.length) return halt('extract', { items: all })

phase('Challenge')
// skeptic 沒判到的事項當成未驗證，和被推翻的一起列存疑（challenge 片段）
const judged = await challengeList(all, { prompt: `${CTX}\n\n## 派工單\n逐項對照會議原文，反駁不忠於原文、張冠李戴、把討論當決議的事項，不確定判 refuted=true。每個事項都要判，target_id 照抄事項的 id（含角度前綴，例如 action:i1）：\n${JSON.stringify(all)}`,
  agentType: NS + 'tpm-skeptic', schema: VERDICTS, missing })
if (judged.lacking) return halt('challenge', { items: all })
const { kept, doubted } = judged

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫會議摘要到 ${A.run_dir}/meeting-summary.md 與待辦清單 ${A.run_dir}/todo-for-task-authority.md（要搬到專案的會議目錄由使用者決定）。被推翻或沒被驗證的事項列「存疑」。跑 check_rules.py。\n事項：${JSON.stringify(kept)}\n存疑：${JSON.stringify(doubted)}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })
if (!docs) { missing.push('write'); return halt('write', { items: all, refuted: doubted.map((i) => i.id) }) }

return { event: 'meeting', items: all, refuted: doubted.map((i) => i.id), docs }
