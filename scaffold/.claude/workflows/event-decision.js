export const meta = {
  name: 'event-decision',
  description: '{{DECISION_OWNER}}定案一件事：找出所有受影響的文件、規範、agent／skill、程式與待辦，四個面向平行產出修改建議 → skeptic 反駁 → writer 寫待改清單與草稿；不直接改共用檔',
  whenToUse: '有決策定案（例如設計正本、主線分支策略、AI 署名方式）；args：decision（決策原文與出處）、run_dir',
  phases: [{ title: 'Find', detail: '四個面向平行' }, { title: 'Challenge' }, { title: 'Write' }],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const ITEMS = /*@schema:items*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["source","items"],"properties":{"source":{"type":"string"},"items":{"type":"array","items":{"type":"object","required":["id","type","text","owner","status","refs"],"properties":{"id":{"type":"string"},"type":{"type":"string","enum":["action","decision","risk","issue","dependency","doc_update","impact","change","check","todo"]},"text":{"type":"string"},"owner":{"type":"string"},"due":{"type":"string"},"status":{"type":"string","enum":["open","done","waiting","red","yellow","green","n_a"]},"refs":{"type":"array","items":{"type":"string"}},"target_files":{"type":"array","items":{"type":"string"}}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","proposal","tasks","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['decision', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `決策：${A.decision}\nrun 目錄：${A.run_dir}\n只找、只建議：不直接改共用檔、不碰 guard.config.json 的 forbidden_paths。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`
const AREAS = [
  { key: 'tpm-docs', ask: '工作區文件：{{BRIEFS_DIR}}（待決事項）、docs/、run 紀錄與對外交付文件中提到這個待決事項或與它衝突的地方。' },
  { key: 'repo-process', ask: '目標 repo 的共用層：CLAUDE.md、流程文件、REVIEW.md、.github/、specs/_templates/ 中「待決定」的段落。' },
  { key: 'agents-skills', ask: '工作區與目標 repo 的 .claude/agents、.claude/skills、.claude/hooks、.claude/teams 中需要跟著改的規則。' },
  { key: 'code-backlog', ask: '程式（例如設計 token、圖示、分支設定）與 backlog（位置見 rules.md 的專案規則）中受影響的項目。' },
]
const found = await pipeline(AREAS, (a) => agent(`${CTX}\n\n## 派工單（${a.key}）\n${a.ask}\n每處一個 type=change 的事項：target_files 寫路徑:行號，text 寫建議怎麼改（附改後的文字草稿）。payload 用 items。`,
  { label: `find:${a.key}`, phase: 'Find', agentType: NS + 'tpm-researcher', schema: ITEMS }))
const items = found.filter(Boolean).flatMap((r) => r.payload.items || [])

phase('Challenge')
const challenge = items.length ? await agent(`${CTX}\n\n## 派工單\n逐項反駁：真的受這個決策影響嗎？建議的改法有沒有超出決策範圍？不確定判 refuted=true：\n${JSON.stringify(items)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
const refuted = new Set((challenge && challenge.payload.verdicts || []).filter((v) => v.refuted).map((v) => v.target_id))

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫待改清單 ${A.run_dir}/changes.md（依檔案分組：現況 → 建議 → 草稿），存疑另列；共用檔的修改要走目標 repo 的 {{BRANCH_PREFIX}}sdlc-* 分支，列為下一步由使用者決定。跑 check_rules.py。\n事項：${JSON.stringify(items.filter((i) => !refuted.has(i.id)))}\n存疑：${JSON.stringify([...refuted])}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

return { event: 'decision', items, refuted: [...refuted], docs }
