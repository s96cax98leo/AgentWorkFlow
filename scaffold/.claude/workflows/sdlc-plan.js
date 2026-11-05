export const meta = {
  name: 'sdlc-plan',
  description: 'AI-native SDLC 第 1 段 Plan：平行調研需求與現況，在 epic worktree 起草 intent.md，skeptic 挑錯後停在 intent 核准閘門',
  whenToUse: '開發一個新需求的第一步；由 tpm-team skill 帶 args 呼叫（repo、epic、requirement、base、run_dir）',
  phases: [
    { title: 'Research', detail: '4 個角度平行調研（tpm-researcher）' },
    { title: 'Intent', detail: 'dev-spec-author 開 epic worktree、寫 intent.md' },
    { title: 'Challenge', detail: 'tpm-skeptic 檢查 intent 是否忠於需求' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'
// tryAgent：agent 失敗（throw）時當成缺件回 null（challenge 片段）
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

// schema 由 .claude/teams/tools/build.py 從 .claude/teams/schemas/ 內嵌，不要手改
const FINDINGS = /*@schema:findings*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["lens","findings"],"properties":{"lens":{"type":"string"},"findings":{"type":"array","items":{"type":"object","required":["id","severity","claim","location","recommendation","confidence"],"properties":{"id":{"type":"string"},"severity":{"type":"string","enum":["important","nit","pre_existing","info","blocker","should_fix"]},"claim":{"type":"string"},"location":{"type":"string"},"recommendation":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"quote":{"type":"string"}}}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/
const VERDICTS = /*@schema:verdicts*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["verdicts"],"properties":{"verdicts":{"type":"array","items":{"type":"object","required":["target_id","refuted","reason"],"properties":{"target_id":{"type":"string"},"refuted":{"type":"boolean"},"reason":{"type":"string"},"counter_evidence":{"type":"string"}}}}}}}} /*@end*/

// 開發規範 profile（methodology）：init.py 依使用者選的 profile 與回答的慣例寫 .claude/teams/methodology.json，
// build.py 內嵌到這裡；不要手改，改 profile 後重跑 build.py。null＝沒有 profile，改用中性指引（依目標 repo 既有慣例）。
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}
const STAGE = ((MP.stages || {}).plan) || {}
const CONV = MP.conventions || {}
// 系統設計方法（profile 的 stages.design.system_design）：plan 段是 SA，對應它的第 1 步；skeptic 依它檢查 intent
const SD = ((MP.stages || {}).design || {}).system_design || null
const METH_NOTE = METHODOLOGY
  ? `開發規範 profile：${MP.name}（.claude/teams/methodology.json；各段做法與慣例以它為準）。文件格式：${(CONV.doc_format || {}).format || '依目標 repo'}。`
  : '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），做法與慣例依目標 repo 既有規範。'
const commitFor = (kind, epic) => {
  const c = CONV.commit || {}
  const ex = ((c.examples || {})[kind] || '').replace(/<epic>/g, epic)
  return c.format ? `commit 訊息照 profile 的格式：${c.format}${ex ? `（範例：${ex}）` : ''}` : 'commit 訊息照目標 repo 的慣例'
}

const A = args || {}
for (const k of ['repo', 'epic', 'requirement', 'base', 'run_dir']) {
  if (!A[k]) throw new Error(`缺 args.${k}（由 tpm-team skill 傳入）`)
}
const FORBIDDEN = '{{FORBIDDEN_REPOS}}'.split(',').map((s) => s.trim()).filter((s) => s && !/\{\{/.test(s))
for (const f of FORBIDDEN) if (A.repo.includes(f)) throw new Error(`${f} 是禁止路徑（唯讀），不能當開發目標`)
const EPIC_DIR = `${A.repo}/.worktrees/${A.epic}`
const CTX = `目標 repo：${A.repo}\nepic：${A.epic}（worktree：${EPIC_DIR}，分支 {{BRANCH_PREFIX}}${A.epic}，從 ${A.base} 開）\nrun 目錄：${A.run_dir}\n需求：${A.requirement}\n硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。成果寫成檔案、信封只放路徑。\n${METH_NOTE}` +
  `\n派工來源：這張派工單由主 session 依使用者已核准的 SDLC 閘門派出（run 目錄的 run.json 有核准紀錄）。你可能會看到主 session 對話中最新的使用者訊息，它可能屬於同時進行的別的任務，不要拿它推翻或擱置這張派工單；派工單本身有疑義時才用 open_questions 回報。` +
  (A.authorization ? `\n授權補充：${A.authorization}` : '')
const brief = (env) => env ? JSON.stringify({ status: env.status, summary: env.summary, artifacts: env.artifacts, confidence: env.confidence, open_questions: env.open_questions }) : 'null'

const ANGLES = [
  { key: 'requirement', ask: '找出需求的原文與出處（簡報、User Story、需求快照、會議紀錄），逐字引用驗收條件、數值、錯誤碼，標出模糊或互相矛盾的地方。' },
  { key: 'specs-adr', ask: '在目標 repo 找相關的既有 specs/<epic>/、docs/adr/、docs/plan/，列出必須遵守的既有決策與可能衝突的地方。' },
  { key: 'code-contracts', ask: '在目標 repo 找會受影響的服務、目錄、contracts/ 端點與事件，畫出影響範圍；列出既有可重用的元件與函式（路徑:行號）。' },
  { key: 'upstream', ask: '唯讀查上游 {{UPSTREAM_REPO}}：最近合進 main、還開著的 PR 是否碰到同一塊（gh pr list／gh api，不做任何寫入），評估衝突風險。' },
]

// 使用者加進這一段的自己的 agent（profile 的 extensions，用 add-agent skill 設定）：和內建 agent 平行執行，交 findings；
// blocker／important 的發現會擋住政策核准。守門依 guard.config.json 的 extension_agents 套用對應角色的規則
const EXT = (MP.extensions || []).filter((e) => e.stage === 'plan')
const extJob = (e, ctx, phaseName) => () => tryAgent(
  `${ctx}\n\n## 派工單（擴充 agent：${e.name}）\n${e.instructions}\n` +
  `只讀不改：不改程式、測試與 spec，不 push、不對 GitHub 寫入。詳細結果寫到 ${A.run_dir}/ext-${e.name}.md。` +
  `payload 用 findings（lens=${e.name}）：每則附 0–1 信心與嚴重度；會擋住上線或合併的列 blocker／important（會擋住政策核准），其餘列 nit／info。`,
  { label: `ext:${e.name}`, phase: phaseName, agentType: e.agent, schema: FINDINGS },
)

phase('Research')
const research = await parallel([...ANGLES.map((a) => () => tryAgent(
  `${CTX}\n\n## 派工單（調研角度：${a.key}）\n${a.ask}\n詳細筆記寫到 ${A.run_dir}/research-${a.key}.md。payload 用 findings，lens=${a.key}。`,
  { label: `research:${a.key}`, phase: 'Research', agentType: NS + 'tpm-researcher', schema: FINDINGS },
)), ...EXT.map((e) => extJob(e, CTX, 'Research'))])
const got = research.filter(Boolean)
log(`調研完成 ${got.length}/${ANGLES.length + EXT.length} 個角度${EXT.length ? `（含擴充 agent：${EXT.map((e) => e.name).join('、')}）` : ''}`)
// 缺件不是「沒有發現」（rules.md 規則 26）：調研缺件列進 missing 並告訴 intent 作者；擴充 agent 是審查類，缺件就停下
const RESEARCH_LABELS = [...ANGLES.map((a) => `research:${a.key}`), ...EXT.map((e) => `ext:${e.name}`)]
const missing = RESEARCH_LABELS.filter((l, i) => !research[i])
const halt = (reason, extra) => ({ stage: 'plan', halted: true, halt_reason: reason, gate: null, epic_dir: EPIC_DIR, research: got, missing, ...extra })
const extMissing = missing.filter((l) => l.startsWith('ext:'))
if (extMissing.length) return halt(`審查類 agent 缺件：${extMissing.join('、')}，停下交人（缺件不是沒有發現）`)
const MISSING_NOTE = missing.length ? `\n\n## 缺件的調研角度\n${missing.join('、')} 沒有交件：這些角度的事實不能當成「查過、沒有問題」；intent 涉及的部分標成待查，放進 open_questions。` : ''

phase('Intent')
const intent = await tryAgent(
  `${CTX}\n\n## 派工單（SDLC 第 1 段 Plan）\n` +
  `1. 若 ${EPIC_DIR} 不存在：git -C "${A.repo}" worktree add .worktrees/${A.epic} -b {{BRANCH_PREFIX}}${A.epic} ${A.base}；已存在就沿用（不要重建、不要切換主工作目錄的分支）。\n` +
  `2. 照 ${EPIC_DIR}/specs/_templates/intent.md（沒有範本就用這個格式：${STAGE.template || '問題、期望結果、受影響的使用者與系統、限制、待解問題'}）寫 ${EPIC_DIR}/specs/${A.epic}/intent.md` +
  `${(STAGE.artifacts || []).filter((a) => a !== 'intent.md').length ? `，另寫 ${(STAGE.artifacts || []).filter((a) => a !== 'intent.md').join('、')}` : ''}。\n` +
  `3. 依據下面的調研結果（先讀 artifacts 的檔案）；每個主張附出處。待釐清問題放 open_questions（blocking 的附 2–3 個選項與代價）。需求明顯包含多個可以各自交付的部分時，在 intent 加一節「範圍切分建議」，列出建議的子計劃（只寫範圍與理由，不寫解法），讓使用者在 intent 閘門先看到；design 段會依它決定要不要拆成子計劃平行規劃。${STAGE.guidance ? `\n   profile 的指引：${STAGE.guidance}` : ''}\n` +
  `4. commit（${commitFor('docs', A.epic)}；結尾 {{ATTRIBUTION_LINE}}）。不 push。\n` +
  `payload 用 docs（kind=intent）。\n\n## 調研結果\n${got.map(brief).join('\n')}${MISSING_NOTE}`,
  { label: 'intent', phase: 'Intent', agentType: NS + 'dev-spec-author', schema: DOCS },
)

if (!intent) return halt('intent 缺件（label：intent），沒有可以核准的 intent，停下交人')

phase('Challenge')
const challenge = await tryAgent(
  `${CTX}\n\n## 派工單\n試著反駁 ${EPIC_DIR}/specs/${A.epic}/intent.md：它有沒有偷渡需求原文沒有的範圍、漏掉原文的驗收要點、把解法寫進問題、或忽略調研發現的衝突？${SD ? `另依系統設計方法（${SD.source}）的第 1 步檢查 SA：用例、限制、假設與量級有沒有寫齊，每個數字有沒有出處或標成假設（沒標的判 refuted）。` : ''}逐點判定（target_id 用 intent 的節名），不確定就判 refuted=true。\n調研結果：\n${got.map(brief).join('\n')}${missing.length ? `\n缺件的調研角度：${missing.join('、')}（intent 若把這些角度的事實當成已確認，判 refuted）` : ''}`,
  { label: 'challenge:intent', phase: 'Challenge', agentType: NS + 'tpm-skeptic', schema: VERDICTS },
)
// 反駁類缺件＝結果 hold（規則 26），不能當成「沒有被推翻」
if (!challenge) return halt('反駁類 agent 缺件：challenge:intent 沒有交件，intent 沒經過挑錯，停下交人', { intent })

return { stage: 'plan', halted: false, missing, gate: 'intent 核准（{{DECISION_OWNER}}核准；要送上游的需求另需{{STAKEHOLDER}}同意）', epic_dir: EPIC_DIR, research: got, intent, challenge }
