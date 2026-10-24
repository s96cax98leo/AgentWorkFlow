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
const ANGLES = [
  { key: 'action', type: 'action', ask: '交辦：誰、做什麼、期限（原文沒說期限就留空）。' },
  { key: 'decision', type: 'decision', ask: '已拍板的決議與由誰拍板；只列明確拍板的，討論中的不算。' },
  { key: 'risk', type: 'risk／dependency／issue', ask: '風險、依賴、問題；標影響範圍。' },
  { key: 'doc', type: 'doc_update', ask: '因這場會議需要更新的既有文件（專案文件、對外交付文件、規格、backlog），target_files 寫路徑，text 寫要改什麼。' },
  { key: 'todo', type: 'todo', ask: '要交給 {{TASK_AUTHORITY}} 編號的新工作（不編號），和既有 backlog 重複的標出既有 id。' },
]
const extracted = await pipeline(ANGLES, (a) => agent(`${CTX}\n\n## 派工單（${a.key}）\n${a.ask}\n只抽 type=${a.type} 的事項。payload 用 items（source=${A.source}）。`,
  { label: `extract:${a.key}`, phase: 'Extract', agentType: NS + 'tpm-researcher', schema: ITEMS }))
const got = extracted.filter(Boolean)
const all = got.flatMap((r) => r.payload.items || [])

phase('Challenge')
const challenge = all.length ? await agent(`${CTX}\n\n## 派工單\n逐項對照會議原文，反駁不忠於原文、張冠李戴、把討論當決議的事項，不確定判 refuted=true：\n${JSON.stringify(all)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
const refuted = new Set((challenge && challenge.payload.verdicts || []).filter((v) => v.refuted).map((v) => v.target_id))

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫會議摘要到 ${A.run_dir}/meeting-summary.md 與待辦清單 ${A.run_dir}/todo-for-task-authority.md（要搬到專案的會議目錄由使用者決定）。被推翻的事項列「存疑」。跑 check_rules.py。\n事項：${JSON.stringify(all.filter((i) => !refuted.has(i.id)))}\n存疑：${JSON.stringify(all.filter((i) => refuted.has(i.id)))}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

return { event: 'meeting', items: all, refuted: [...refuted], docs }
