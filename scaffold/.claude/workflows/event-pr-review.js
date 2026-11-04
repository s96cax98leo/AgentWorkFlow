export const meta = {
  name: 'event-pr-review',
  description: '審一個 PR 或分支（不是自己 SDLC 開發的）：standards、spec、bugs、security、design、history 六個 lens 平行 → 去重 → 每批由 skeptic 反駁 → 只留信心 ≥0.8 且未被推翻的；只產報告、不留言',
  whenToUse: '要審 PR 或分支；args：target（PR 編號＋repo 或分支名）、repo（本機 clone 路徑）、run_dir、lenses（可省略）',
  phases: [{ title: 'Review', detail: '每個 lens 一個 tpm-reviewer' }, { title: 'Verify', detail: 'tpm-skeptic' }, { title: 'Write' }],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['target', 'repo', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const LENSES = A.lenses || ['standards', 'spec', 'bugs', 'security', 'design', 'history']
const CTX = `審查目標：${A.target}\n本機 repo：${A.repo}\nrun 目錄：${A.run_dir}\n只讀：取 diff 用 gh pr diff／git diff，不 approve、不 merge、不 comment。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`
const key = (f) => `${(f.location || '').replace(/:\d+.*/, '')}|${(f.claim || '').slice(0, 40)}`
// 規則 26：lens 沒交件不是「這個 lens 很乾淨」，skeptic 沒交件也不是「全部沒被推翻」；缺件一律 halted，halt_reason 寫出 label
const missing = []
const halt = (stage, extra = {}) => ({ event: 'pr-review', halted: true, stage, halt_reason: `缺件：${missing.join('、')}`, missing: [...missing], ...extra })

phase('Review')
const reviews = await parallel(LENSES.map((lens) => () => agent(`${CTX}\n\n## 派工單（lens：${lens}）\n只用 ${lens} 觀點審查，依目標 repo 的 REVIEW.md 分級，每則附 file:line 與 0–1 信心，只列 ≥0.8。payload 用 findings（lens=${lens}）。`,
  { label: `review:${lens}`, phase: 'Review', agentType: NS + 'tpm-reviewer', schema: FINDINGS })))
// 各 lens 的發現 id 各自編號，加 lens 前綴（<lens>:<id>），skeptic 的 target_id 才不會撞號
const batches = []
reviews.forEach((r, i) => {
  const lens = LENSES[i]
  if (!r) { missing.push(`review:${lens}`); return }
  batches.push({ lens, fs: (r.payload.findings || []).filter((f) => f.confidence >= 0.8).map((f) => ({ ...f, id: `${lens}:${f.id}` })) })
})
if (missing.length) return halt('review')

phase('Verify')
const toVerify = batches.filter((b) => b.fs.length)
const verdicts = await parallel(toVerify.map((b) => () => agent(`${CTX}\n\n## 派工單\n逐則反駁 ${b.lens} 的發現（打開 location 看實際程式），不確定判 refuted=true。每則都要判，target_id 照抄發現的 id（含 lens 前綴）：\n${JSON.stringify(b.fs)}`,
  { label: `verify:${b.lens}`, phase: 'Verify', agentType: NS + 'tpm-skeptic', schema: VERDICTS })))
const reviewed = []
toVerify.forEach((b, i) => {
  const vr = verdicts[i]
  if (!vr) { missing.push(`verify:${b.lens}`); return }
  // skeptic 沒判到的發現當成未驗證，不採納
  const v = new Map((vr.payload.verdicts || []).map((x) => [x.target_id, x]))
  reviewed.push({ lens: b.lens, kept: b.fs.filter((f) => v.has(f.id) && !v.get(f.id).refuted), dropped: b.fs.filter((f) => !v.has(f.id) || v.get(f.id).refuted) })
})
if (missing.length) return halt('verify')
const seen = new Set(), kept = []
for (const r of reviewed) for (const f of r.kept) { const k = key(f); if (!seen.has(k)) { seen.add(k); kept.push({ lens: r.lens, ...f }) } }
log(`保留 ${kept.length} 則（去重後）`)

phase('Write')
const docs = await agent(`${CTX}\n\n## 派工單\n寫審查報告 ${A.run_dir}/review.md：依 lens 分節，每則「[Important|Nit|Pre-existing] file:line — 問題 — 建議 — 信心」，Nit 最多 5 則。要不要貼到 PR 由使用者決定。跑 check_rules.py。\n發現：${JSON.stringify(kept)}\npayload 用 docs。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })
if (!docs) { missing.push('write'); return halt('write', { findings: kept }) }

return { event: 'pr-review', findings: kept, dropped: reviewed.flatMap((r) => r.dropped.map((f) => f.id)), docs }
