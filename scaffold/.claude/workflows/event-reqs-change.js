export const meta = {
  name: 'event-reqs-change',
  description: '需求變動（User Story 或需求來源更新）：先抽出差異，再對 specs、backlog 與 program、對外交付文件、能力對照四個面向平行做影響分析 → skeptic 反駁 → writer 寫影響清單；必要時產出新 intent 草稿交 SDLC',
  whenToUse: '需求快照或 User Story 有更新；args：change（新舊檔路徑或描述）、repo（目標 repo，可省略）、run_dir',
  phases: [{ title: 'Diff', detail: '抽出差異' }, { title: 'Impact', detail: '四個面向平行' }, { title: 'Challenge' }, { title: 'Write' }],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const ITEMS = /*@schema:items*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["source","items"],"properties":{"source":{"type":"string"},"items":{"type":"array","items":{"type":"object","required":["id","type","text","owner","status","refs"],"properties":{"id":{"type":"string"},"type":{"type":"string","enum":["action","decision","risk","issue","dependency","doc_update","impact","change","check","todo"]},"text":{"type":"string"},"owner":{"type":"string"},"due":{"type":"string"},"status":{"type":"string","enum":["open","done","waiting","red","yellow","green","n_a"]},"refs":{"type":"array","items":{"type":"string"}},"target_files":{"type":"array","items":{"type":"string"}}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","proposal","tasks","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['change', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `需求變動：${A.change}\n目標 repo：${A.repo || '{{REPOS_DIR}}（見 rules.md 的專案規則）'}\nrun 目錄：${A.run_dir}\n只讀（需求來源永遠唯讀）。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`

phase('Diff')
const diff = await agent(`${CTX}\n\n## 派工單\n逐條列出新增、修改、刪除的需求（Story ID、欄位、舊值、新值），type=change。payload 用 items。`,
  { label: 'diff', phase: 'Diff', agentType: NS + 'tpm-researcher', schema: ITEMS })
const changes = diff ? diff.payload.items : []
if (!changes.length) return { event: 'reqs-change', diff, note: '沒有差異' }

const AREAS = [
  { key: 'specs', ask: '目標 repo 的 specs/、契約、測試（e2e/tests 的 Story ID）。' },
  { key: 'program', ask: 'backlog 與專案計畫文件（scope、roadmap、能力對照、風險）。' },
  { key: 'deliverables', ask: '對外交付的文件與表格（md／docx／xlsx 成對時兩邊都要改）。' },
  { key: 'capability', ask: 'WBS、能力矩陣或能力對照表；已排入的 demo 範圍。' },
]
const impact = await pipeline(AREAS, (a) => agent(`${CTX}\n\n## 派工單（影響面：${a.key}）\n範圍：${a.ask}\n對每條變動找受影響的檔案與段落（target_files 寫路徑:行號），type=impact。\n變動：${JSON.stringify(changes)}\npayload 用 items。`,
  { label: `impact:${a.key}`, phase: 'Impact', agentType: NS + 'tpm-researcher', schema: ITEMS }))
const items = impact.filter(Boolean).flatMap((r) => r.payload.items || [])

phase('Challenge')
const challenge = items.length ? await agent(`${CTX}\n\n## 派工單\n逐項反駁（打開 target_files 查證），不確定判 refuted=true：\n${JSON.stringify(items)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
const refuted = new Set((challenge && challenge.payload.verdicts || []).filter((v) => v.refuted).map((v) => v.target_id))

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫影響清單 ${A.run_dir}/impact.md（變動 → 受影響檔案 → 建議動作；存疑另列）。需要開發的，在 ${A.run_dir}/intent-draft-<短名>.md 起草 intent 草稿交 SDLC（Maintain → Plan）。跑 check_rules.py。\n變動：${JSON.stringify(changes)}\n影響：${JSON.stringify(items.filter((i) => !refuted.has(i.id)))}\n存疑：${JSON.stringify([...refuted])}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

return { event: 'reqs-change', changes, impact: items, refuted: [...refuted], docs }
