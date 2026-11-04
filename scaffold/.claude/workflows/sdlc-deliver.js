export const meta = {
  name: 'sdlc-deliver',
  description: 'AI-native SDLC 第 5 段 Deploy：四軸平行審查 → 每則發現由 skeptic 反駁 → 只留信心 ≥0.8 且未被推翻的 → 寫 PR 說明草稿與執行紀錄；推送與開 PR 由使用者決定',
  whenToUse: 'sdlc-test 的結果確認之後；sdlc-sync 走了中轉分支時 args 加 branch（中轉分支名）',
  phases: [
    { title: 'Review', detail: 'tpm-reviewer（deliver 模式）四軸平行（standards、spec、bugs、history）' },
    { title: 'Verify', detail: 'tpm-skeptic 逐則反駁' },
    { title: 'Write', detail: 'tpm-writer 寫 PR 說明草稿與執行紀錄、跑 check_rules' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
const STAGE = ((MP.stages || {}).deliver) || {}
const CONV = MP.conventions || {}
const METH_NOTE = METHODOLOGY
  ? `開發規範 profile：${MP.name}（.claude/teams/methodology.json；各段做法與慣例以它為準）。文件格式：${(CONV.doc_format || {}).format || '依目標 repo'}。`
  : '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），做法與慣例依目標 repo 既有規範。'
const commitFor = (kind, epic) => {
  const c = CONV.commit || {}
  const ex = ((c.examples || {})[kind] || '').replace(/<epic>/g, epic)
  return c.format ? `commit 訊息照 profile 的格式：${c.format}${ex ? `（範例：${ex}）` : ''}` : 'commit 訊息照目標 repo 的慣例'
}
const PR_SIZE = CONV.pr_size || {}
const COMMIT = CONV.commit || {}

const A = args || {}
for (const k of ['repo', 'epic', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const EPIC_DIR = `${A.repo}/.worktrees/${A.epic}`
// sdlc-sync 走中轉分支後 head 換成中轉分支，用 args.branch 帶進來
const BRANCH = A.branch || `{{BRANCH_PREFIX}}${A.epic}`
const BASE = A.base || 'origin/main'
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}（分支 ${BRANCH}，基準 ${BASE}）\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。只留建議：不 approve、不 merge、不 comment、不 push。\n${METH_NOTE}${STAGE.guidance ? `\nprofile 的 deliver 指引：${STAGE.guidance}` : ''}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')

const AXES = [
  { key: 'standards', ask: `規範：目標 repo 的 CLAUDE.md、REVIEW.md、分支與合併規範，以及開發規範 profile 的慣例（code style：${(CONV.code_style || {}).description || '依目標 repo'}；commit：${COMMIT.format || '依目標 repo'}；分支：${(CONV.branch || {}).naming || '依目標 repo'}）。` +
    (PR_SIZE.max_lines || PR_SIZE.max_files
      ? `另外量 PR 大小（profile 的上限：${[PR_SIZE.max_lines ? `${PR_SIZE.max_lines} 行` : '', PR_SIZE.max_files ? `${PR_SIZE.max_files} 個檔` : ''].filter(Boolean).join('或')}；量法：${PR_SIZE.scope || `git diff --numstat ${BASE}...HEAD`}），超過就列一則 important，建議拆成依序可合的小 PR。`
      : 'profile 沒有設 PR 大小上限，不檢查 PR 大小。') +
    ((STAGE.review_axes || []).length ? `\nprofile 補充的審查重點：${STAGE.review_axes.join('；')}` : '') },
  { key: 'spec', ask: `Spec：對照 specs/${A.epic}/spec.md 的驗收條件與影響系統表，找缺漏、多做、做錯；每則引用 spec 原文（quote）。` },
  { key: 'bugs', ask: '只看這次改動引入的邏輯、邊界、回歸、併發與錯誤處理問題。' },
  { key: 'history', ask: '對改動處 git log -L／git blame，找出破壞原本設計意圖的改動。' },
]
// 使用者加進這一段的自己的 agent（profile 的 extensions，用 /add-agent 設定）：和內建 agent 平行執行，交 findings；
// blocker／important 的發現會擋住政策核准。守門依 guard.config.json 的 extension_agents 套用對應角色的規則
const EXT = (MP.extensions || []).filter((e) => e.stage === 'deliver')
const extJob = (e, ctx, phaseName) => () => agent(
  `${ctx}\n\n## 派工單（擴充 agent：${e.name}）\n${e.instructions}\n` +
  `只讀不改：不改程式、測試與 spec，不 push、不對 GitHub 寫入。詳細結果寫到 ${A.run_dir}/ext-${e.name}.md。` +
  `payload 用 findings（lens=${e.name}）：每則附 0–1 信心與嚴重度；會擋住上線或合併的列 blocker／important（會擋住政策核准），其餘列 nit／info。`,
  { label: `ext:${e.name}`, phase: phaseName, agentType: e.agent, schema: FINDINGS },
)
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
// opts.unansweredKept＝true：只有明確 refuted 的才列 doubted（沒判到的保留），同一 target_id 只要有一則 refuted 就算推翻。
// 回 { kept, doubted, unanswered }，unanswered 是沒被判到的事項 id。
function judgeVerdicts(list, challenge, opts = {}) {
  const verdicts = (((challenge || {}).payload || {}).verdicts) || []
  const last = new Map(verdicts.map((x) => [x.target_id, x]))
  const anyRefuted = new Set(verdicts.filter((x) => x.refuted).map((x) => x.target_id))
  const ok = opts.unansweredKept
    ? (i) => !anyRefuted.has(i.id)
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
/*@snippet:review-skeptic*/
// @requires: challenge
// ── 片段 review-skeptic（.claude/teams/tools/snippets/review-skeptic.js）：審查 agent 交 findings 後，派 skeptic 逐則反駁 ──
// build.py 內嵌；不要在 workflow 裡手改，改片段後重跑 build.py。只定義函式，需要的值都用參數傳入。
// 用到 challenge 片段的 judgeVerdicts，引用本片段的 workflow 也要引用 challenge 片段。
// 缺件（rules.md 規則 26）：審查或反駁的 agent 回 null 或交件無效時記進 missing（回傳的 missing 欄位是 label）；
// 缺件不是「沒有發現」也不是「沒被推翻」，呼叫端要回 halted:true，halt_reason 寫出 label。

// 審查交件 → 信心 ≥ minConfidence（預設 0.8）的發現；prefix 有給時 id 改成 <prefix>:<id>（多批合併時 skeptic 的 target_id 才不會撞號）。
// 缺件回 { fs: null, missing: label }，有給 missing 陣列時也記進去。
function takeFindings(r, label, o = {}) {
  if (!r || !r.payload) {
    if (o.missing) o.missing.push(label)
    return { fs: null, missing: label }
  }
  const min = o.minConfidence === undefined ? 0.8 : o.minConfidence
  const fs = (r.payload.findings || []).filter((f) => f.confidence >= min).map((f) => (o.prefix ? { ...f, id: `${o.prefix}:${f.id}` } : f))
  return { fs, missing: null }
}

// 派一個 skeptic 反駁一批發現；fs 是空的就不派。回 Promise<{ kept, dropped, missing }>，missing 是缺件的 label（沒缺件為 null）。
// o：{ prompt, label, phase='Verify', agentType, schema, unansweredKept（見 judgeVerdicts）, keepOnMissing }
// keepOnMissing＝true：反駁缺件時發現全部留在 kept（不能當成被推翻）；否則 kept 為空。兩種都要由呼叫端依 missing 停下。
function verifyFindings(fs, o) {
  if (!fs.length) return Promise.resolve({ kept: [], dropped: [], missing: null })
  return agent(o.prompt, { label: o.label, phase: o.phase || 'Verify', agentType: o.agentType, schema: o.schema })
    .then((v) => {
      if (!v || !v.payload) return { kept: o.keepOnMissing ? fs : [], dropped: [], missing: o.label }
      const j = judgeVerdicts(fs, v, { unansweredKept: o.unansweredKept })
      return { kept: j.kept, dropped: j.doubted, missing: null }
    })
}

// 多批反駁後的發現去重（同檔、同主張前 40 字算同一則），保留先出現的；每則加上 field 欄位（例如 lens、axis）標出來自哪一批。
function dedupeFindings(groups, field) {
  const seen = new Set()
  const kept = []
  for (const g of groups) {
    for (const f of g.kept) {
      const k = `${(f.location || '').replace(/:\d+.*/, '')}|${(f.claim || '').slice(0, 40)}`
      if (!seen.has(k)) { seen.add(k); kept.push({ [field]: g[field], ...f }) }
    }
  }
  return kept
}
/*@end*/

// 擴充 agent 和四軸一起審，發現一樣經 skeptic 反駁
const reviewed = await pipeline(
  [...AXES, ...EXT.map((e) => ({ key: e.name, ext: e }))],
  (a) => a.ext ? extJob(a.ext, `${CTX}\n審查對象：${EPIC_DIR}（git diff ${BASE}...HEAD）。每則附 0–1 信心，只列 ≥0.8 的。`, 'Review')() : agent(`${CTX}\n\n## 派工單（deliver 模式，審查軸：${a.key}）\n${a.ask}\n每則附 0–1 信心，只列 ≥0.8 的；先回頭讀實際行為再確認。payload 用 findings（lens=${a.key}）。`,
    { label: `review:${a.key}`, phase: 'Review', agentType: NS + 'tpm-reviewer', schema: FINDINGS }),
  (r, a) => {
    // 缺件不是「沒有發現」（rules.md 規則 26）：審查軸缺件記下 label，不能當成空清單
    const { fs, missing: lacking } = takeFindings(r, a.ext ? `ext:${a.key}` : `review:${a.key}`)
    if (!fs) return { axis: a.key, kept: [], dropped: [], missing: lacking }
    if (!fs.length) return { axis: a.key, kept: [], dropped: [] }
    // 反駁缺件：發現全部保留（keepOnMissing，不能當成被推翻），但這一軸沒經過反駁，整段停下
    // 這一段只把明確 refuted 的發現拿掉（unansweredKept），沒被判到的保留
    return verifyFindings(fs, {
      prompt: `${CTX}\n\n## 派工單\n逐則試著反駁下列 ${a.key} 軸的發現（打開 location 看實際程式），不確定就判 refuted=true：\n${JSON.stringify(fs)}`,
      label: `verify:${a.key}`, phase: 'Verify', agentType: NS + 'tpm-skeptic', schema: VERDICTS, unansweredKept: true, keepOnMissing: true,
    }).then((res) => ({ axis: a.key, ...res }))
  },
)
const AXIS_LABELS = [...AXES.map((a) => `review:${a.key}`), ...EXT.map((e) => `ext:${e.name}`)]
const missing = reviewed.map((r, i) => (r ? r.missing : AXIS_LABELS[i])).filter(Boolean)
const kept = dedupeFindings(reviewed.filter(Boolean), 'axis')
const dropped = reviewed.filter(Boolean).reduce((n, r) => n + r.dropped.length, 0)
log(`保留 ${kept.length} 則；被 skeptic 推翻 ${dropped} 則`)
// 審查或反駁缺件：不寫 PR 說明（避免草稿寫成「審過、沒有發現」），停下交人
if (missing.length) return { stage: 'deliver', halted: true, halt_reason: `審查／反駁類 agent 缺件：${missing.join('、')}（缺件不是沒有發現），停下交人`, missing, gate: null, findings: kept, dropped, docs: null }

phase('Write')
const docs = await agent(
  `${CTX}\n\n## 派工單\n1. 照目標 repo 的 .github/pull_request_template.md 寫 PR 說明草稿到 ${A.run_dir}/pr-description.md（驗證欄引用 specs/${A.epic}/verifier.md）。\n2. 寫執行紀錄到 ${A.run_dir}/執行紀錄.md（產出、驗證、跟計畫不同的地方、刻意沒做的事、未能確認、衍生工作（不編號））。\n3. 審查發現（已通過 skeptic）：${JSON.stringify(kept)}\n4. 兩份都跑 python3 .claude/teams/tools/check_rules.py，並對 ${A.repo} 跑 --git ${A.repo} ${BASE}..${BRANCH} 檢查 commit（署名行${COMMIT.check_regex ? '，以及 profile 的 commit 標題格式' : ''}），修到通過；commit 不符時只回報，不改寫歷史。payload 用 docs，rule_check 填結果。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS },
)

// writer 不是審查類：缺件列進 missing（PR 說明與執行紀錄沒寫），閘門照常但要補寫
return { stage: 'deliver', halted: false, missing: docs ? [] : ['write'], gate: '推 fork／開 PR 由使用者當下決定；寫入上游另需再次同意；agent 不核准、不合併', findings: kept, dropped, docs }
