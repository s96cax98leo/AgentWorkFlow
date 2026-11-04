export const meta = {
  name: 'event-upstream',
  description: '上游（{{UPSTREAM_REPO}}）變動（新 PR、main 前進、CI 失敗）：四個角度平行唯讀分析 → skeptic 反駁 → writer 寫報告、給決策者版與 intent 草稿；開工與收尾記錄上游狀態證明零寫入',
  whenToUse: '上游有新 PR、main 前進或 CI 失敗；args：what（描述或 PR 編號）、repo（我方 fork 路徑）、run_dir',
  phases: [
    { title: 'Baseline', detail: '記錄上游狀態' },
    { title: 'Analyze', detail: 'tpm-researcher（上游模式）四個角度平行' },
    { title: 'Challenge' }, { title: 'Write' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

const A = args || {}
for (const k of ['what', 'repo', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const CTX = `事件：${A.what}\n我方 fork：${A.repo}\nrun 目錄：${A.run_dir}\n全程唯讀：不對上游做任何寫入，不切換 fork 主工作目錄的分支（用 git fetch 到唯讀 ref、git merge-tree 預測）。硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。`
// 規則 26：agent 沒交件記為 missing，不當成空清單或「沒有被推翻」；缺件一律 halted，halt_reason 寫出 label
/*@snippet:challenge*/
// ── 片段 challenge（.claude/teams/tools/snippets/challenge.js）：skeptic 的 verdicts 對回事項，判定哪些被推翻、哪些沒驗證到 ──
// build.py 內嵌；不要在 workflow 裡手改，改片段後重跑 build.py。只定義函式，需要的值都用參數傳入。
// 缺件（rules.md 規則 26）：agent 回 null 或交件無效時把 label 記進 missing；
// 反駁類缺件不是「全部沒被推翻」，呼叫端要回 halted:true，halt_reason 寫出 label（haltMissing）。

// 派 agent，失敗（throw：逾時、被中止、交件驗證錯誤等）一律當成缺件回 null，讓呼叫端走缺件流程（規則 26）；
// workflow 裡所有 agent() 都改走這個，不要讓 throw 繞過缺件處理或把整支 workflow 打斷。
function tryAgent(prompt, opts) {
  try {
    return Promise.resolve(agent(prompt, opts)).catch((e) => { log(`agent ${(opts || {}).label || ''} 失敗，當成缺件：${(e && e.message) || e}`); return null })
  } catch (e) {
    log(`agent ${(opts || {}).label || ''} 失敗，當成缺件：${(e && e.message) || e}`)
    return Promise.resolve(null)
  }
}

// 缺件時的回傳：{ ...base, halted: true, stage, halt_reason: '缺件：<label>、…', missing, ...extra }
function haltMissing(base, stage, missing, extra = {}) {
  return { ...base, halted: true, stage, halt_reason: `缺件：${missing.join('、')}`, missing: [...missing], ...extra }
}

// 把 verdicts（target_id 對事項 id）對回 list。兩種模式都往嚴的方向判，但「嚴」的方向相反：
// 預設：kept 是被採信的結論（例如抽出的決議、事項），要每一則判定都沒推翻才採信：
//   skeptic 沒判到的當成未驗證，和被推翻的一起列 doubted；同一 target_id 判了多次時，只要有一則 refuted 就列 doubted。
// opts.unansweredKept＝true：kept 是保留下來、會擋住的問題（例如審查發現），要每一則判定都推翻才拿掉：
//   只有明確 refuted 的才列 doubted（沒判到的保留）；同一 target_id 判了多次時，每一則都 refuted 才算推翻。
// 回 { kept, doubted, unanswered }，unanswered 是沒被判到的事項 id。
function judgeVerdicts(list, challenge, opts = {}) {
  const verdicts = (((challenge || {}).payload || {}).verdicts) || []
  const answered = new Set(verdicts.map((x) => x.target_id))
  const upheld = new Set(verdicts.filter((x) => !x.refuted).map((x) => x.target_id))
  const refuted = new Set(verdicts.filter((x) => x.refuted).map((x) => x.target_id))
  const ok = opts.unansweredKept
    ? (i) => !answered.has(i.id) || upheld.has(i.id)
    : (i) => answered.has(i.id) && !refuted.has(i.id)
  return { kept: list.filter(ok), doubted: list.filter((i) => !ok(i)), unanswered: list.filter((i) => !answered.has(i.id)).map((i) => i.id) }
}

// 派一個 skeptic 反駁整批事項，再用 judgeVerdicts 判定。list 是空的就不派（challenge=null、lacking=false）。
// o：{ prompt, label='challenge', phase='Challenge', agentType, schema, missing（缺件 label 記進這個陣列）, unansweredKept }
// skeptic 缺件（回 null、交件無效或 throw）：label 記進 o.missing，回 lacking=true（kept 為空、全部列 doubted），呼叫端要 halted。
async function challengeList(list, o) {
  const label = o.label || 'challenge'
  if (!list.length) return { challenge: null, lacking: false, ...judgeVerdicts(list, null, o) }
  const challenge = await tryAgent(o.prompt, { label, phase: o.phase || 'Challenge', agentType: o.agentType, schema: o.schema })
  if (!challenge || !challenge.payload) {
    if (o.missing) o.missing.push(label)
    return { challenge: null, lacking: true, kept: [], doubted: list.slice(), unanswered: list.map((i) => i.id) }
  }
  return { challenge, lacking: false, ...judgeVerdicts(list, challenge, o) }
}
/*@end*/
const missing = []
const halt = (stage, extra = {}) => haltMissing({ event: 'upstream' }, stage, missing, extra)
// 規則 12：給決策者的文件要過 check_rules --audience owner；writer 沒回報或沒過就 halted 交人
const ruleCheck = (d) => {
  const rc = String(d.payload.rule_check || '').trim()
  if (!rc) return 'writer 沒有回報 rule_check（check_rules.py 的結果）'
  if (!/^pass\b/i.test(rc)) return `check_rules.py 沒過：${rc}`
  if (!(d.payload.documents || []).some((x) => x.audience === 'owner')) return 'writer 沒有交出給決策者版（documents 沒有 audience=owner）'
  return null
}
// tpm-researcher 上游模式不寫檔：分析與快照只交信封，由 tpm-writer 落檔（快照前後對照要由同一份落檔證明零寫入）
const snap = (tag) => tryAgent(`${CTX}\n\n## 派工單（上游模式）\n記錄上游狀態（${tag}）：main 的 head SHA、開著的 PR 數與我方 PR 的 head SHA 與 updated_at、分支數。不寫檔，全部放在信封：每項一則 finding（severity=info，location 寫查詢指令）。payload 用 findings（lens=upstream）。`,
  { label: `snapshot:${tag}`, phase: tag === 'before' ? 'Baseline' : 'Write', agentType: NS + 'tpm-researcher', schema: FINDINGS })

phase('Baseline')
const before = await snap('before')
if (!before) { missing.push('snapshot:before'); return halt('baseline') }

const ANGLES = [
  { key: 'conflict', ask: '用 git merge-tree 預測這次變動和我方每條進行中分支的衝突檔，分類（機械式、需判斷、隱性風險）。' },
  { key: 'impact', ask: '這次變動改了哪些目錄、契約、migration、設計 token；對我方 demo、fork 開發與規範檔有什麼影響。' },
  { key: 'timeline', ask: '重建時間線（絕對時間、含時區）：誰在什麼時候合了什麼、CI 狀態；不推論意圖。' },
  { key: 'action', ask: '依前面的事實列出該通知誰、需要誰決定什麼、我方下一步選項（附代價）；不替人做決定。' },
]
phase('Analyze')
const analyzed = await pipeline(ANGLES, (a) => tryAgent(`${CTX}\n\n## 派工單（上游模式：${a.key}）\n${a.ask}\n不寫檔，筆記全部放在信封（findings 的 claim、location、recommendation）。payload 用 findings（lens=${a.key}）。`,
  { label: `analyze:${a.key}`, phase: 'Analyze', agentType: NS + 'tpm-researcher', schema: FINDINGS }))
// 各角度的發現 id 各自編號，合併前加角度前綴（<角度>:<id>），skeptic 的 target_id 才不會撞號
const claims = []
analyzed.forEach((r, i) => {
  const k = ANGLES[i].key
  if (!r) missing.push(`analyze:${k}`)
  else for (const f of r.payload.findings || []) claims.push({ ...f, id: `${k}:${f.id}` })
})
if (missing.length) return halt('analyze', { before, claims })

phase('Challenge')
const judged = await challengeList(claims, { prompt: `${CTX}\n\n## 派工單\n逐則反駁（重跑唯讀指令查證），不確定判 refuted=true。每則都要判，target_id 照抄發現的 id（含角度前綴，例如 conflict:f1）：\n${JSON.stringify(claims)}`,
  agentType: NS + 'tpm-skeptic', schema: VERDICTS, missing })
if (judged.lacking) return halt('challenge', { before, claims })
// skeptic 沒判到的發現當成未驗證，和被推翻的一起列「未能確認」
const { challenge, kept, doubted } = judged

phase('Write')
const after = await snap('after')
if (!after) { missing.push('snapshot:after'); return halt('write', { before, claims }) }
const docs = await tryAgent(`${CTX}\n\n## 派工單\n分析 agent 沒有寫檔權限，由你落檔：\n` +
  `1. 寫上游狀態 ${A.run_dir}/upstream-before.md 與 ${A.run_dir}/upstream-after.md，並在報告附兩者對照（證明零寫入；對不上就在報告最前面標出）。\n` +
  `2. 寫 ${A.run_dir}/upstream-report.md：事實、衝突預測、影響、時間線、選項；被推翻或沒被驗證的發現列「未能確認」。\n` +
  `3. 寫給決策者版 ${A.run_dir}/給決策者.md：發生什麼、對我方的影響、需要誰決定什麼、每個選項的代價。\n` +
  `4. 我方需要開發（例如跟上上游、修衝突）的，在 ${A.run_dir}/intent-draft-<主題>.md 起草 intent 草稿交 SDLC（sdlc-plan），一個主題一份；不需要就在報告寫明「不需開發」。\n` +
  `5. 每份跑 python3 .claude/teams/tools/check_rules.py（給決策者版加 --audience owner），全過才算完成。payload 用 docs：documents 列出每份（給決策者版 audience=owner、intent 草稿 kind=intent），rule_check 填「pass」或「fail：<沒過的規則>」。\n` +
  `成立的發現：${JSON.stringify(kept)}\n未能確認：${JSON.stringify(doubted)}\n上游狀態（開工）：${JSON.stringify(before.payload.findings || [])}\n上游狀態（收尾）：${JSON.stringify(after.payload.findings || [])}`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS })
if (!docs) { missing.push('write'); return halt('write', { before, after, claims }) }
const ruleIssue = ruleCheck(docs)
if (ruleIssue) return { event: 'upstream', halted: true, stage: 'write', halt_reason: ruleIssue, before, after, claims, docs }

return { event: 'upstream', before, after, claims, refuted: doubted.map((f) => f.id), challenge, docs }
