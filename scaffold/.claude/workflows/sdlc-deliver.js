export const meta = {
  name: 'sdlc-deliver',
  description: 'AI-native SDLC 第 5 段 Deploy：四軸平行審查 → 每則發現由 skeptic 反駁 → 只留信心 ≥0.8 且未被推翻的 → 寫 PR 說明草稿與執行紀錄；推送與開 PR 由使用者決定',
  whenToUse: 'sdlc-test 的結果確認之後',
  phases: [
    { title: 'Review', detail: 'dev-pr-reviewer 四軸平行（standards、spec、bugs、history）' },
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
// spec 文件的位置與檔名：profile 的 spec_layout（選用，可以只設部分鍵，沒設的用預設）。
// 預設值就是原本寫死的 specs/<epic>/intent.md 等，所以沒設 spec_layout 時派工單一字不變。
const DEFAULT_LAYOUT = { root: 'specs/{epic}', intent: 'intent.md', outline: 'outline.md', spec: 'spec.md', plan: 'plan.md', progress: 'progress.md', verifier: 'verifier.md', verifier_shard: 'verifier-{shard}.md', sub_dir: 'sub', templates: 'specs/_templates' }
const LAYOUT = { ...DEFAULT_LAYOUT, ...(MP.spec_layout || {}) }
const specRoot = (epic) => LAYOUT.root.replace(/\{epic\}/g, () => epic)
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
const BASE = A.base || 'origin/main'
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}（分支 {{BRANCH_PREFIX}}${A.epic}，基準 ${BASE}）\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。只留建議：不 approve、不 merge、不 comment、不 push。\n${METH_NOTE}${STAGE.guidance ? `\nprofile 的 deliver 指引：${STAGE.guidance}` : ''}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')

const AXES = [
  { key: 'standards', ask: `規範：目標 repo 的 CLAUDE.md、REVIEW.md、分支與合併規範，以及開發規範 profile 的慣例（code style：${(CONV.code_style || {}).description || '依目標 repo'}；commit：${COMMIT.format || '依目標 repo'}；分支：${(CONV.branch || {}).naming || '依目標 repo'}）。` +
    (PR_SIZE.max_lines || PR_SIZE.max_files
      ? `另外量 PR 大小（profile 的上限：${[PR_SIZE.max_lines ? `${PR_SIZE.max_lines} 行` : '', PR_SIZE.max_files ? `${PR_SIZE.max_files} 個檔` : ''].filter(Boolean).join('或')}；量法：${PR_SIZE.scope || `git diff --numstat ${BASE}...HEAD`}），超過就列一則 important，建議拆成依序可合的小 PR。`
      : 'profile 沒有設 PR 大小上限，不檢查 PR 大小。') +
    ((STAGE.review_axes || []).length ? `\nprofile 補充的審查重點：${STAGE.review_axes.join('；')}` : '') },
  { key: 'spec', ask: `Spec：對照 ${specRoot(A.epic)}/${LAYOUT.spec} 的驗收條件與影響系統表，找缺漏、多做、做錯；每則引用 spec 原文（quote）。` },
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
const key = (f) => `${(f.location || '').replace(/:\d+.*/, '')}|${(f.claim || '').slice(0, 40)}`

// 擴充 agent 和四軸一起審，發現一樣經 skeptic 反駁
const reviewed = await pipeline(
  [...AXES, ...EXT.map((e) => ({ key: e.name, ext: e }))],
  (a) => a.ext ? extJob(a.ext, `${CTX}\n審查對象：${EPIC_DIR}（git diff ${BASE}...HEAD）。每則附 0–1 信心，只列 ≥0.8 的。`, 'Review')() : agent(`${CTX}\n\n## 派工單（審查軸：${a.key}）\n${a.ask}\n每則附 0–1 信心，只列 ≥0.8 的；先回頭讀實際行為再確認。payload 用 findings（lens=${a.key}）。`,
    { label: `review:${a.key}`, phase: 'Review', agentType: NS + 'dev-pr-reviewer', schema: FINDINGS }),
  (r, a) => {
    const fs = (r && r.payload.findings || []).filter((f) => f.confidence >= 0.8)
    if (!fs.length) return { axis: a.key, kept: [], dropped: [] }
    return agent(`${CTX}\n\n## 派工單\n逐則試著反駁下列 ${a.key} 軸的發現（打開 location 看實際程式），不確定就判 refuted=true：\n${JSON.stringify(fs)}`,
      { label: `verify:${a.key}`, phase: 'Verify', agentType: NS + 'tpm-skeptic', schema: VERDICTS })
      .then((v) => {
        const refuted = new Set((v && v.payload.verdicts || []).filter((x) => x.refuted).map((x) => x.target_id))
        return { axis: a.key, kept: fs.filter((f) => !refuted.has(f.id)), dropped: fs.filter((f) => refuted.has(f.id)) }
      })
  },
)
const seen = new Set()
const kept = []
for (const r of reviewed.filter(Boolean)) for (const f of r.kept) { const k = key(f); if (!seen.has(k)) { seen.add(k); kept.push({ axis: r.axis, ...f }) } }
const dropped = reviewed.filter(Boolean).reduce((n, r) => n + r.dropped.length, 0)
log(`保留 ${kept.length} 則；被 skeptic 推翻 ${dropped} 則`)

phase('Write')
const docs = await agent(
  `${CTX}\n\n## 派工單\n1. 照目標 repo 的 .github/pull_request_template.md 寫 PR 說明草稿到 ${A.run_dir}/pr-description.md（驗證欄引用 ${specRoot(A.epic)}/${LAYOUT.verifier}）。\n2. 寫執行紀錄到 ${A.run_dir}/執行紀錄.md（產出、驗證、跟計畫不同的地方、刻意沒做的事、未能確認、衍生工作（不編號））。\n3. 審查發現（已通過 skeptic）：${JSON.stringify(kept)}\n4. 兩份都跑 python3 .claude/teams/tools/check_rules.py，並對 ${A.repo} 跑 --git ${A.repo} ${BASE}..{{BRANCH_PREFIX}}${A.epic} 檢查 commit（署名行${COMMIT.check_regex ? '，以及 profile 的 commit 標題格式' : ''}），修到通過；commit 不符時只回報，不改寫歷史。payload 用 docs，rule_check 填結果。`,
  { label: 'write', phase: 'Write', agentType: NS + 'tpm-writer', schema: DOCS },
)

return { stage: 'deliver', gate: '推 fork／開 PR 由使用者當下決定；寫入上游另需再次同意；agent 不核准、不合併', findings: kept, dropped, docs }
