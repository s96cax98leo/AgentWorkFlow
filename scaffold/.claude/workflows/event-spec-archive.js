export const meta = {
  name: 'event-spec-archive',
  description: '合併後歸檔 OpenSpec change（只在 profile 的 spec_engine=openspec 時）：確認 sdlc-merge 已合併且 event-post-merge 正常 → dev-integrator 從最新 main 開 chore/openspec-archive-<id>、檢查 tasks 全勾 → 跑 openspec archive 與兩次 validate、commit、列 diff → workflow 確定性檢查（錯誤碼、diff 只在 openspec/、原 PR head 不變）→ tpm-writer 寫只改 openspec/ 的小 PR 草稿；不 push、不開 PR、不合併',
  whenToUse: 'sdlc-merge 回傳 merged=true、event-post-merge 回傳 ok=true 之後；args：repo（本機 clone）、run_dir、change（OpenSpec change id，也可用 epic）、merge（sdlc-merge 的回傳，至少 merged、head_sha）、post_merge（event-post-merge 的回傳，至少 ok），可加 pr、gh_repo、base',
  phases: [
    { title: 'Check', detail: '確認已合併、post-merge 正常、profile 是 openspec' },
    { title: 'Prepare', detail: 'dev-integrator 從最新 main 開歸檔分支，讀 tasks 進度與原 PR head' },
    { title: 'Archive', detail: 'dev-integrator 跑 openspec archive --yes --json、validate --specs --strict、validate --archived --strict，commit 並列 diff' },
    { title: 'Draft', detail: 'tpm-writer 寫只改 openspec/ 的 PR 草稿（使用者同意後才開 PR）' },
  ],
}

// plugin 的 agent 帶命名空間（init.py 依 workflow.config 的 AGENT_NS 填入；agents 放專案 .claude/agents 時填空字串）
const NS = '{{AGENT_NS}}'

const ARCHIVE = /*@schema:spec-archive*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["change","branch","branch_exists","pr_head_sha","progress","unchecked","steps","diff_files"],"properties":{"change":{"type":"string","minLength":1},"branch":{"type":"string","minLength":1},"branch_exists":{"type":"boolean"},"base_sha":{"type":"string"},"pr_head_sha":{"type":"string"},"progress":{"type":"object","required":["total","complete","remaining"],"properties":{"total":{"type":"integer","minimum":0},"complete":{"type":"integer","minimum":0},"remaining":{"type":"integer","minimum":0}}},"unchecked":{"type":"array","items":{"type":"object","required":["line","description"],"properties":{"line":{"type":"integer"},"description":{"type":"string"}}}},"steps":{"type":"array","items":{"type":"object","required":["name","command","exit","stdout"],"properties":{"name":{"type":"string","enum":["archive","validate_specs","validate_archived"]},"command":{"type":"string"},"exit":{"type":"integer"},"stdout":{"type":"string"}}}},"commit":{"type":"string"},"diff_files":{"type":"array","items":{"type":"string"}}}}}} /*@end*/
const DOCS = /*@schema:docs*/ {"type":"object","required":["status","summary","artifacts","evidence","confidence","open_questions","derived_work","payload"],"properties":{"status":{"type":"string","enum":["done","blocked","needs_decision"]},"summary":{"type":"string","minLength":1},"artifacts":{"type":"array","items":{"type":"string"}},"evidence":{"type":"array","items":{"type":"object","required":["claim","source","result"],"properties":{"claim":{"type":"string"},"source":{"type":"string"},"result":{"type":"string"}}}},"confidence":{"type":"number","minimum":0,"maximum":1},"open_questions":{"type":"array","items":{"type":"object","required":["question","blocking","options","recommendation","default_applied"],"properties":{"question":{"type":"string"},"blocking":{"type":"boolean"},"why_blocking":{"type":"string"},"options":{"type":"array","items":{"type":"object","required":["choice","cost"],"properties":{"choice":{"type":"string"},"cost":{"type":"string"}}}},"recommendation":{"type":"string"},"default_applied":{"type":"string"}}}},"derived_work":{"type":"array","items":{"type":"string"}},"payload":{"type":"object","required":["documents"],"properties":{"documents":{"type":"array","minItems":1,"items":{"type":"object","required":["path","kind","audience"],"properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["intent","design","spec","plan","progress","verifier","design_review","security_review","adr","exec_record","research","for_owner","pr_description","meeting_summary","proposal","tasks","other"]},"audience":{"type":"string","enum":["internal","owner","external","agents"]},"notes":{"type":"string"}}}},"rule_check":{"type":"string"}}}}} /*@end*/

// 開發規範 profile（methodology）：build.py 內嵌；只有 spec_engine=openspec 時才歸檔
const METHODOLOGY = /*@methodology*/ null /*@end*/
const MP = METHODOLOGY || {}

const A = args || {}
for (const k of ['repo', 'run_dir']) if (!A[k]) throw new Error(`缺 args.${k}`)
const ID = A.change || A.epic
if (!ID) throw new Error('缺 args.change（OpenSpec change id）')
const GH = A.gh_repo || '{{FORK_REPO}}'
const BASE = A.base || 'origin/main'
const BRANCH = `chore/openspec-archive-${ID}`
const WT = `.worktrees/openspec-archive-${ID}`
const PR = A.pr || (A.merge && A.merge.pr) || null
const M = A.merge || {}
const PM = A.post_merge || {}
const stop = (code, reason, detail = null) => ({ event: 'spec-archive', halted: true, code, reason, detail, change: ID, branch: BRANCH,
  next: code === 'ARCHIVE_BRANCH_EXISTS' || code === 'ARCHIVE_TARGET_EXISTS'
    ? '先由人確認既有的歸檔分支或 archive 目錄要保留還是移除，再重跑；不要覆寫。'
    : `分支 ${BRANCH} 保留現場（如果已建立）供人修正；修好後重跑 event-spec-archive。不開 PR。` })

// (1) 前置條件：都是確定性判斷，不派 agent
phase('Check')
if (MP.spec_engine !== 'openspec') return stop('ARCHIVE_NOT_OPENSPEC', '開發規範 profile 的 spec_engine 不是 openspec，沒有要歸檔的 OpenSpec change')
if (M.merged !== true) return { event: 'spec-archive', halted: true, reason: '尚未合併', detail: { merged: M.merged ?? null, queued: M.queued ?? null }, change: ID,
  next: M.queued ? '還在 merge queue；合進去、event-post-merge 正常之後再跑。' : '原 PR 沒有合併，不歸檔。' }
if (PM.ok !== true) return { event: 'spec-archive', halted: true, reason: PM.ok === false ? 'main 的 CI 有問題，先處理 revert' : '還沒有 event-post-merge 的結果', change: ID,
  next: PM.ok === false ? '照 event-post-merge 的 revert 提案處理，main 恢復正常後再跑。' : '先跑 event-post-merge，ok=true 之後再跑。' }
if (!M.head_sha) return stop('ARCHIVE_PR_HEAD_CHANGED', 'sdlc-merge 的回傳沒有 head_sha，無法確認原 PR 的 head 沒變（缺資料一律停下）')

const CTX = `目標 repo（本機 clone）：${A.repo}\nGitHub repo：${GH}${PR ? `；原 PR #${PR}` : ''}；基準 ${BASE}\nOpenSpec change：${ID}\n歸檔分支：${BRANCH}（worktree：${WT}）\nrun 目錄：${A.run_dir}\n` +
  `硬規則：.claude/teams/rules.md；輸出語言：{{OUTPUT_LANGUAGE}}。不 push、不開 PR、不合併、不改原 PR（不在原分支加 commit）。只改 openspec/ 底下的檔案。` +
  `openspec 指令一律加 --json，環境變數設 NO_COLOR=1、OPENSPEC_NO_ANIMATION=1；stdout 原文照抄進 payload，不要摘要或改寫。`

// (2) 準備：開分支、讀 tasks 進度與原 PR head；分支已存在就停，不覆寫
phase('Prepare')
const prep = await agent(
  `${CTX}\n\n## 派工單（準備）\n1. git -C ${A.repo} fetch origin。\n2. 檢查分支是否已存在：git -C ${A.repo} rev-parse --verify --quiet ${BRANCH}，以及 git -C ${A.repo} ls-remote --heads origin ${BRANCH}。任一有結果就 branch_exists=true，**不要**建立或覆寫，直接交件。\n` +
  `3. 不存在才建立：git -C ${A.repo} worktree add ${WT} -b ${BRANCH} ${BASE}；base_sha 填 git -C ${WT} rev-parse HEAD。\n` +
  `4. 在 ${WT} 執行 openspec instructions apply --change ${ID} --json：progress 照抄；tasks 裡 done=false 的逐項放 unchecked（line、description 照抄）。\n` +
  `5. pr_head_sha：${PR ? `gh pr view ${PR} --repo ${GH} --json headRefOid 的值` : '原 PR 的 head（gh pr list --repo ' + GH + ' --state merged --head <原分支> --json headRefOid）'}。\n` +
  `這一步不跑 archive。steps、diff_files 填空陣列。payload 用 spec-archive。`,
  { label: 'prepare', phase: 'Prepare', agentType: NS + 'dev-integrator', schema: ARCHIVE },
)
if (!prep) return stop('ARCHIVE_CLI_FAILED', '準備步驟沒有交件；狀態不明，交主 session 檢查分支與 worktree')
const p = prep.payload
if (p.branch_exists !== false) return stop('ARCHIVE_BRANCH_EXISTS', `分支 ${BRANCH} 已存在，不覆寫`)
const prog = p.progress || {}
if (prog.remaining !== 0 || (p.unchecked || []).length) {
  return stop('ARCHIVE_TASKS_INCOMPLETE', `tasks 還有 ${prog.remaining ?? '?'} 項未勾（CLI 在 --yes 下不會擋，所以先停下）`, { progress: prog, unchecked: p.unchecked || [] })
}

// (3) archive 與兩次 validate；CLI 輸出由 workflow 判定，不信 agent 的結論
phase('Archive')
const run = await agent(
  `${CTX}\n\n## 派工單（歸檔）\n在 ${WT}（分支 ${BRANCH}）依序執行，每一步都記一筆 steps（name、command、exit、stdout 原文）：\n` +
  `1. name=archive：openspec archive ${ID} --yes --json。exit 不是 0 就停在這一步，不要重試、不要加 --no-validate。\n` +
  `2. name=validate_specs：openspec validate --specs --strict --json。\n3. name=validate_archived：openspec validate --archived --strict --json。\n` +
  `4. 三步都 exit 0 才 commit（git -C ${WT} add openspec && git -C ${WT} commit），訊息照 profile 的 docs 格式寫「歸檔 OpenSpec change ${ID}」；commit 填 SHA。任何一步失敗就不 commit，保留現場。\n` +
  `5. diff_files：git -C ${WT} diff --name-only ${BASE}...HEAD（沒有 commit 時用 git -C ${WT} status --porcelain 列出的路徑）。\n` +
  `6. 再讀一次原 PR 的 head 填 pr_head_sha（同準備步驟）；branch_exists 填 false，progress、unchecked 照抄準備步驟。\n不 push。payload 用 spec-archive。`,
  { label: 'archive', phase: 'Archive', agentType: NS + 'dev-integrator', schema: ARCHIVE },
)
if (!run) return stop('ARCHIVE_CLI_FAILED', '歸檔步驟沒有交件；分支保留，交主 session 檢查')
const r = run.payload
const parse = (s) => { try { return JSON.parse(s) } catch { return null } }
const step = (name) => (r.steps || []).find((s) => s.name === name)
const arc = step('archive')
if (!arc || arc.exit !== 0) {
  const out = arc ? parse(arc.stdout) : null
  const codes = out && Array.isArray(out.status) ? out.status.map((d) => d.code) : []
  if (codes.includes('archive_target_exists')) return stop('ARCHIVE_TARGET_EXISTS', '同名的 archive 目錄已存在（同一天歸檔過同一個 change）', { stdout: arc.stdout })
  return stop('ARCHIVE_CLI_FAILED', arc ? `openspec archive exit ${arc.exit}` : '沒有 archive 步驟的紀錄', { stdout: arc ? arc.stdout : null })
}
const bad = []
for (const name of ['validate_specs', 'validate_archived']) {
  const s = step(name)
  const out = s ? parse(s.stdout) : null
  const failed = out && out.summary && out.summary.totals ? out.summary.totals.failed : null
  if (!s || s.exit !== 0 || failed !== 0) {
    const issues = out && Array.isArray(out.items) ? out.items.filter((i) => !i.valid).map((i) => ({ id: i.id, issues: i.issues })) : []
    bad.push({ step: name, exit: s ? s.exit : null, issues, stdout: s && !issues.length ? s.stdout : undefined })
  }
}
if (bad.length) return stop('ARCHIVE_VALIDATE_FAILED', `歸檔後的現行規格驗證沒過（${bad.map((b) => b.step).join('、')}），例如 Purpose 還是 TBD 佔位`, bad)
const files = r.diff_files || []
const outside = files.filter((f) => !/^openspec\/(specs|changes)\//.test(f))
if (!files.length || outside.length) return stop('ARCHIVE_DIFF_OUT_OF_SCOPE', files.length ? '分支 diff 有 openspec/specs、openspec/changes 以外的檔案' : '分支沒有任何 diff', { outside, files })
if (r.pr_head_sha !== M.head_sha) return stop('ARCHIVE_PR_HEAD_CHANGED', '原 PR 的 head 和合併時不同', { merged_head: M.head_sha, now: r.pr_head_sha })
log(`歸檔 ${ID}：${files.length} 個檔案，全部在 openspec/ 底下；兩次 validate 通過`)

// (4) PR 草稿：只寫文件，不開 PR
phase('Draft')
const DRAFT = `${A.run_dir}/archive-pr.md`
const draft = await agent(
  `${CTX}\n\n## 派工單\n寫歸檔 PR 的草稿到 ${DRAFT}：標題「歸檔 OpenSpec change ${ID}」、head ${BRANCH}、base ${BASE.replace(/^origin\//, '')}；` +
  `內文列出這次進現行規格的 Requirement（讀 ${WT} 的 openspec/specs/ 與 archive 目錄，依 ADDED／MODIFIED／REMOVED／RENAMED 分組）、archive 與兩次 validate 的結果、diff 檔案清單、原 PR${PR ? ` #${PR}` : ''} 的 head ${M.head_sha}（未變）。` +
  `最後寫明：開 PR 前要使用者同意，合併由使用者做。\narchive 輸出：${arc.stdout}\ndiff：${JSON.stringify(files)}\n寫完跑 python3 .claude/teams/tools/check_rules.py。payload 用 docs（kind=pr_description，audience=owner），rule_check 填結果。`,
  { label: 'pr-draft', phase: 'Draft', agentType: NS + 'tpm-writer', schema: DOCS },
)

return {
  event: 'spec-archive',
  ok: true,
  change: ID,
  branch: BRANCH,
  commit: r.commit || null,
  files,
  pr_draft: DRAFT,
  prepare: prep,
  archive: run,
  draft,
  next: `使用者同意後才開 PR：gh pr create --repo ${GH} --head ${BRANCH} --base ${BASE.replace(/^origin\//, '')} --body-file ${DRAFT}（先 push 分支）。合併由使用者做；合併後移除 worktree ${WT}。`,
}
