export const meta = {
  name: 'event-demo-check',
  description: 'Demo 前檢查：環境、資料、腳本與流程、畫面、備案五個面向平行檢查（只讀或本機操作）→ skeptic 反駁綠燈 → writer 寫紅綠燈清單；需要開發的產出 intent 草稿',
  whenToUse: 'demo 前；args：target（demo 用的 repo 路徑與分支）、script（demo 腳本或流程檔）、date、run_dir',
  phases: [{ title: 'Check', detail: '五個面向平行' }, { title: 'Challenge' }, { title: 'Write' }],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const ITEMS = /*@schema:items*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["source","items"],"properties":{"source":{"type":"string"},"items":{"type":"array","items":{"type":"object","required":["id","type","text","owner","status","refs"],"properties":{"id":{"type":"string"},"type":{"type":"string","enum":["action","decision","risk","issue","dependency","doc_update","impact","change","check","todo"]},"text":{"type":"string"},"owner":{"type":"string"},"due":{"type":"string"},"status":{"type":"string","enum":["open","done","waiting","red","yellow","green","n_a"]},"refs":{"type":"array","items":{"type":"string"}},"target_files":{"type":"array","items":{"type":"string"}}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['target', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `demo 目標：${A.target}\ndemo 腳本：${A.script || '（未提供，先找 {{BRIEFS_DIR}} 中最新的 demo 簡報）'}\ndemo 日：${A.date || '未提供'}\nrun 目錄：${A.run_dir}\n只讀或本機操作：不推、不改上游、不刪資料；compose 指令一律帶 -p <demo 專屬專案名>（注意環境變數 COMPOSE_PROJECT_NAME 會覆蓋）；不讀 .env 的值。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`
const AREAS = [
  { key: 'env', ask: '環境：分支與 commit、服務能不能起（just up 或對等指令）、健康檢查、版本是否和 demo 腳本一致。' },
  { key: 'data', ask: '資料：demo 需要的帳號、租戶、種子資料、模型與 MCP 設定是否就緒（只檢查有沒有值，不印金鑰）。' },
  { key: 'flow', ask: '腳本與流程：逐步走 demo 腳本，每步標 green／yellow／red 與證據（指令輸出或截圖路徑）。' },
  { key: 'ui', ask: '畫面：demo 會出現的頁面在常用桌機尺寸與專案支援的色彩模式下有沒有破版、錯字、console error（截圖存 run 目錄）。' },
  { key: 'fallback', ask: '備案：網路、模型、外部服務失效時的備案是否可用（錄影、靜態頁、離線資料）。' },
]
const checked = await pipeline(AREAS, (a) => agent(`${CTX}\n\n## 派工單（${a.key}）\n${a.ask}\n每個檢查點一個 type=check 的事項，status 用 green／yellow／red，refs 放證據。payload 用 items。`,
  { label: `check:${a.key}`, phase: 'Check', agentType: NS + 'tpm-researcher', schema: ITEMS }))
const items = checked.filter(Boolean).flatMap((r) => r.payload.items || [])

phase('Challenge')
const greens = items.filter((i) => i.status === 'green')
const challenge = greens.length ? await agent(`${CTX}\n\n## 派工單\n逐項反駁「green」判定：證據真的支持嗎？是不是只看設定沒實際跑？不確定判 refuted=true：\n${JSON.stringify(greens)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null
const refuted = new Set((challenge && challenge.payload.verdicts || []).filter((v) => v.refuted).map((v) => v.target_id))

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫紅綠燈清單 ${A.run_dir}/demo-check.md：先列 red 與被推翻的 green（降為 yellow），再列其他；每項附證據與建議處理。需要改程式的在 ${A.run_dir}/intent-draft-<短名>.md 起草 intent 交 SDLC。跑 check_rules.py。\n檢查：${JSON.stringify(items)}\n被推翻的 green：${JSON.stringify([...refuted])}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

return { event: 'demo-check', items, downgraded: [...refuted], docs }
