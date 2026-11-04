export const meta = {
  name: 'sdlc-sync',
  description: 'AI-native SDLC 同步段：sync_branch.py 判斷分支怎麼跟上 base（已跟上不動；未推送且無衝突 rebase；已推送且無衝突把 base merge 進來；有衝突從 base 開中轉分支，把分支 merge 進中轉分支解衝突）→ dev-integrator 執行並跑 verify → workflow 核對衝突清單；head 變了就要重跑 sdlc-test',
  whenToUse: 'sdlc-build 之後、sdlc-test 之前；或 sdlc-merge 因 PR 落後／衝突判 hold 之後。args：repo、epic、run_dir，可加 base、branch、strategy（蓋過 profile）、date（YYYYMMDD，沒給就從 run_dir 取）、pr',
  phases: [
    { title: 'Plan', detail: 'dev-integrator 跑 sync_branch.py plan（唯讀）：落後幾個 commit、有沒有推送過、預測衝突檔、選策略' },
    { title: 'Apply', detail: 'dev-integrator 跑 sync_branch.py apply；中轉分支有衝突時解衝突、分類、commit，再跑 check-transit；最後跑 verify' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const BUILD = /*@schema:build-result*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["task_id","branch","commits","files_changed","tests"],"properties":{"task_id":{"type":"string"},"branch":{"type":"string"},"worktree":{"type":"string"},"commits":{"type":"array","items":{"type":"string"}},"files_changed":{"type":"array","items":{"type":"string"}},"tests":{"type":"array","items":{"type":"object","required":["command","result"],"properties":{"command":{"type":"string"},"result":{"type":"string","enum":["pass","fail","red_as_expected","not_run"]},"summary":{"type":"string"}}}},"sync":{"type":"object","required":["strategy","conflicts"],"properties":{"strategy":{"type":"string","enum":["none","rebase","merge","transit"]},"behind":{"type":"integer","minimum":0},"published":{"type":"boolean"},"conflicts":{"type":"array","items":{"type":"string"}},"transit_branch":{"type":["string","null"]},"head_branch":{"type":"string"},"head_sha":{"type":"string"},"halted":{"type":"boolean"},"reason":{"type":"string"}}},"conflicts":{"type":"array","items":{"type":"object","required":["file","kind","resolution"],"properties":{"file":{"type":"string"},"kind":{"type":"string","enum":["mechanical","seam","judgement","implicit_compile","implicit_behavior","generated"]},"resolution":{"type":"string"}}}}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
const CONV = MP.conventions || {}
const SYNC = CONV.sync || {}
const METH_NOTE = METHODOLOGY
  ? `開發規範 profile：${MP.name}（.claude/teams/methodology.json；各段做法與慣例以它為準）。同步策略：${SYNC.strategy || 'auto'}${SYNC.transit_naming ? `；中轉分支命名 ${SYNC.transit_naming}` : ''}。`
  : '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），同步策略用 auto。'
const commitFor = (kind, epic) => {
  const c = CONV.commit || {}
  const ex = ((c.examples || {})[kind] || '').replace(/<epic>/g, epic)
  return c.format ? `commit 訊息照 profile 的格式：${c.format}${ex ? `（範例：${ex}）` : ''}` : 'commit 訊息照目標 repo 的慣例'
}

const A = args || {}
for (const k of ['repo', 'epic', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
// 腳本裡不能取時間：日期由主 session 給（args.date），或從 run_dir 名稱裡的 YYYYMMDD 取
const DATE = A.date || ((String(A.run_dir).match(/(20\d{6})/) || [])[1])
if (!DATE) throw new Error('缺 args.date（YYYYMMDD），run_dir 名稱裡也找不到日期')
const EPIC_DIR = `${A.repo}/.worktrees/${A.epic}`
const BASE = A.base || 'origin/main'
const BRANCH = A.branch || `{{BRANCH_PREFIX}}${A.epic}`
const TOOL = 'python3 .claude/teams/tools/sync_branch.py'
const COMMON = `--repo "${EPIC_DIR}" --branch ${BRANCH} --base ${BASE}`
const CTX = `目標 repo：${A.repo}\nepic worktree：${EPIC_DIR}（分支 ${BRANCH}，基準 ${BASE}）\nrun 目錄：${A.run_dir}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。不 push、不 force push；rebase 只能透過 sync_branch.py，而且只對沒推送過的分支。commit 結尾 {{ATTRIBUTION_LINE}}。\n${METH_NOTE}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。`

const sameSet = (a, b) => { const x = [...new Set(a)].sort(); const y = [...new Set(b)].sort(); return x.length === y.length && x.every((v, i) => v === y[i]) }
const out = (o) => ({ stage: 'sync', gate: null, branch: BRANCH, ...o })

phase('Plan')
const planRes = await agent(
  `${CTX}\n\n## 派工單（同步：判斷）\n在主 session 的專案根目錄跑：\n${TOOL} plan ${COMMON} --short ${A.epic} --date ${DATE} --config .claude/teams/guard.config.json --methodology .claude/teams/methodology.json${A.strategy ? ` --strategy ${A.strategy}` : ''}\n` +
  `只跑這個指令，不改任何東西。把它印出的 JSON 原樣抄進 payload.sync（strategy、behind、published、conflicts、transit_branch、halted、reason）。exit 1（halted）時 status 填 needs_decision，open_questions 寫 reason。\npayload 用 build-result（task_id=sync-plan、branch=${BRANCH}、commits、files_changed、tests 給空陣列）。`,
  { label: 'plan', phase: 'Plan', agentType: NS + 'dev-integrator', schema: BUILD },
)
const plan = planRes && planRes.payload && planRes.payload.sync
if (!plan) return out({ halted: true, halt_reason: `plan ${planRes ? '沒有回傳 payload.sync（沒照抄工具輸出）' : '缺件（agent 失敗、逾時或被中止）'}`, missing: planRes ? [] : ['plan'], next: null })
if (plan.halted || planRes.status !== 'done') return out({ halted: true, halt_reason: `sync_branch.py plan 停下：${plan.reason || planRes.summary}`, plan, next: '依 reason 處理（例如固定 rebase 但分支已推送：改 strategy 或交人決定）後重跑 sdlc-sync' })
log(`同步策略：${plan.strategy}（落後 ${plan.behind ?? '?'}、${plan.published ? '已推送' : '未推送'}、預測衝突 ${(plan.conflicts || []).length} 個）：${plan.reason || ''}`)
if (plan.strategy === 'none') return out({ strategy: 'none', plan, head_branch: BRANCH, next: '已跟上，照原流程往下' })
if (!['rebase', 'merge', 'transit'].includes(plan.strategy)) return out({ halted: true, halt_reason: `未知的同步策略：${plan.strategy}`, plan, next: null })
const predicted = plan.conflicts || []
if (plan.strategy !== 'transit' && predicted.length) return out({ halted: true, halt_reason: `策略 ${plan.strategy} 不能有衝突，但 plan 預測了 ${predicted.length} 個`, plan, next: null })

phase('Apply')
const VERIFY = `跑 rules.md 專案規則的 verify 指令（基準 ${BASE}；沒有就用該 repo 的對等指令），結果寫進 tests[]。`
const applyBrief = plan.strategy === 'transit'
  ? `1. ${TOOL} apply ${COMMON} --strategy transit --transit-branch ${plan.transit_branch} --date ${DATE} --config .claude/teams/guard.config.json\n` +
    `   它會從 ${BASE} 開中轉分支 ${plan.transit_branch}，再把 ${BRANCH} merge 進來。exit 0＝沒衝突；exit 2＝有衝突，留在工作目錄等你解。\n` +
    `2. 有衝突時只組合兩邊的意圖，不發明新行為；lock、產生檔重新產生，不手動合。只改有衝突的檔（預測：${predicted.join('、') || '無'}），改了別的檔 check-transit 會擋。每個衝突檔照六類（mechanical、seam、judgement、implicit_compile、implicit_behavior、generated）寫進 conflicts[]。judgement 類拿不準就以 needs_decision 交人，不要硬解。解完 git add 並 commit（${commitFor('fix', A.epic)}）。\n` +
    `3. ${TOOL} check-transit --repo "${EPIC_DIR}" --branch ${BRANCH} --base ${BASE} --expected "${predicted.join(',')}"，exit 1 就照它列的理由修正，修不了就 needs_decision。\n` +
    `4. ${VERIFY}\n` +
    `payload 用 build-result（task_id=sync、branch=${plan.transit_branch}、conflicts 照第 2 步）；payload.sync 照抄 plan 的欄位，另填 head_branch=${plan.transit_branch} 與 head_sha（git rev-parse HEAD）。`
  : `1. ${TOOL} apply ${COMMON} --strategy ${plan.strategy} --date ${DATE} --config .claude/teams/guard.config.json\n` +
    `   ${plan.strategy === 'rebase' ? '它會先留備份 ref（refs/backup/…）再 rebase 到 ' + BASE : '它會把 ' + BASE + ' merge --no-ff 進 ' + BRANCH}。exit 3＝執行時出現衝突、已自動中止：不要自己解，status 填 needs_decision，open_questions 寫「plan 沒預測到衝突，改走 transit 要人決定」。\n` +
    `2. ${VERIFY}\n` +
    `payload 用 build-result（task_id=sync、branch=${BRANCH}）；payload.sync 照抄 plan 的欄位，另填 head_branch=${BRANCH} 與 head_sha（git rev-parse HEAD）。`
const applyRes = await agent(`${CTX}\n\n## 派工單（同步：執行 ${plan.strategy}）\n在主 session 的專案根目錄操作：\n${applyBrief}`,
  { label: `apply:${plan.strategy}`, phase: 'Apply', agentType: NS + 'dev-integrator', schema: BUILD })

if (!applyRes || applyRes.status !== 'done') return out({ halted: true, halt_reason: `同步執行沒完成：${applyRes ? applyRes.summary : `apply:${plan.strategy} 缺件（agent 沒有回傳結果）`}`, missing: applyRes ? [] : [`apply:${plan.strategy}`], strategy: plan.strategy, plan, apply: applyRes, next: '交人處理；工作目錄可能停在解到一半的狀態' })
const p = applyRes.payload || {}
const headBranch = (p.sync && p.sync.head_branch) || (plan.strategy === 'transit' ? plan.transit_branch : BRANCH)
// 確定性核對：中轉分支回報的衝突檔要和 plan 預測的一致（不能多解、也不能漏記）
if (plan.strategy === 'transit') {
  const reported = (p.conflicts || []).map((c) => c.file)
  if (!sameSet(reported, predicted)) return out({ halted: true, halt_reason: `回報的衝突檔（${reported.join('、') || '無'}）和 plan 預測的（${predicted.join('、') || '無'}）不一致`, strategy: 'transit', plan, apply: applyRes, next: '交人核對中轉分支的解法' })
  if (headBranch !== plan.transit_branch) return out({ halted: true, halt_reason: `head_branch（${headBranch}）不是 plan 的中轉分支（${plan.transit_branch}）`, strategy: 'transit', plan, apply: applyRes, next: null })
}
const failed = (p.tests || []).filter((t) => t.result === 'fail')
if (failed.length) return out({ halted: true, halt_reason: `同步後 verify 失敗：${failed.map((t) => t.command).join('、')}`, strategy: plan.strategy, head_branch: headBranch, plan, apply: applyRes, next: '交人決定：修正（回 sdlc-test 的修正輪）或放棄這次同步' })

// 換了 head 分支：sdlc-test、sdlc-deliver（以及 merge 退回時的 sdlc-build）都帶 args.branch；
// 合併段不讀 args.branch，認的是 publish.py --supersedes 開出的新 PR 編號（args.pr）
const nextSteps = ['head 已變：驗證證據綁 head SHA，重跑 sdlc-test' + (headBranch !== BRANCH ? `（args 加 branch=${headBranch}；之後的 sdlc-deliver 與退回重做時的 sdlc-build 也帶同一個 branch）` : '')]
if (plan.strategy === 'transit') nextSteps.push(`驗證通過後用 publish.py 推 ${headBranch}、開新 PR${A.pr ? `（--supersedes ${A.pr}，原 PR 由人關閉）` : ''}；合併段以新 PR 的編號為準（args.pr=新 PR 編號）`)
else if (plan.published) nextSteps.push('驗證通過後推送更新（publish.py；不需要 force push）')
return out({ strategy: plan.strategy, head_branch: headBranch, head_sha: (p.sync || {}).head_sha || null, behind: plan.behind, conflicts: p.conflicts || [], plan, apply: applyRes, next: nextSteps.join('；') })
