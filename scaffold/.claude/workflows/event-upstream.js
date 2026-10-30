export const meta = {
  name: 'event-upstream',
  description: '上游（{{UPSTREAM_REPO}}）變動（新 PR、main 前進、CI 失敗）：四個角度平行唯讀分析 → skeptic 反駁 → writer 寫報告；開工與收尾記錄上游狀態證明零寫入',
  whenToUse: '上游有新 PR、main 前進或 CI 失敗；args：what（描述或 PR 編號）、repo（我方 fork 路徑）、run_dir',
  phases: [
    { title: 'Baseline', detail: '記錄上游狀態' },
    { title: 'Analyze', detail: 'tpm-upstream-analyst 四個角度平行' },
    { title: 'Challenge' }, { title: 'Write' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","proposal","tasks","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['what', 'repo', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `事件：${A.what}\n我方 fork：${A.repo}\nrun 目錄：${A.run_dir}\n全程唯讀：不對上游做任何寫入，不切換 fork 主工作目錄的分支（用 git fetch 到唯讀 ref、git merge-tree 預測）。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`
const snap = (tag) => agent(`${CTX}\n\n## 派工單\n記錄上游狀態（${tag}）：main 的 head SHA、開著的 PR 數與我方 PR 的 head SHA 與 updated_at、分支數。寫到 ${A.run_dir}/upstream-${tag}.md。payload 用 findings（lens=upstream）。`,
  { label: `snapshot:${tag}`, phase: 'Baseline', agentType: NS + 'tpm-upstream-analyst', schema: FINDINGS })

phase('Baseline')
const before = await snap('before')
const ANGLES = [
  { key: 'conflict', ask: '用 git merge-tree 預測這次變動和我方每條進行中分支的衝突檔，分類（機械式、需判斷、隱性風險）。' },
  { key: 'impact', ask: '這次變動改了哪些目錄、契約、migration、設計 token；對我方 demo、fork 開發與規範檔有什麼影響。' },
  { key: 'timeline', ask: '重建時間線（絕對時間、含時區）：誰在什麼時候合了什麼、CI 狀態；不推論意圖。' },
  { key: 'action', ask: '依前面的事實列出該通知誰、需要誰決定什麼、我方下一步選項（附代價）；不替人做決定。' },
]
const analyzed = await pipeline(ANGLES, (a) => agent(`${CTX}\n\n## 派工單（${a.key}）\n${a.ask}\n筆記寫到 ${A.run_dir}/${a.key}.md。payload 用 findings（lens=${a.key}）。`,
  { label: `analyze:${a.key}`, phase: 'Analyze', agentType: NS + 'tpm-upstream-analyst', schema: FINDINGS }))
const got = analyzed.filter(Boolean)

phase('Challenge')
const claims = got.flatMap((r) => r.payload.findings || [])
const challenge = claims.length ? await agent(`${CTX}\n\n## 派工單\n逐則反駁（重跑唯讀指令查證），不確定判 refuted=true：\n${JSON.stringify(claims)}`,
  { label: 'challenge', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS }) : null

phase('Write')
const after = await snap('after')
const docs = await agent(`${CTX}\n\n## 派工單\n寫 ${A.run_dir}/upstream-report.md：事實、衝突預測、影響、時間線、選項；被推翻的發現列「未能確認」；附開工與收尾的上游狀態對照（證明零寫入）。跑 check_rules.py。\n分析：${JSON.stringify(got.map((r) => ({ s: r.summary, a: r.artifacts })))}\n反駁：${JSON.stringify(challenge && challenge.payload)}\n上游狀態：${JSON.stringify([before && before.artifacts, after && after.artifacts])}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })

return { event: 'upstream', before, after, analyzed: got, challenge, docs }
