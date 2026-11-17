// check-kernel 的測試本體：對執行核心片段（challenge、recipe、kernel）直接測。原本在 0.x 的 tests/check_kernel.py 裡。
// 用法：node -e <本檔> <片段檔…>；stdout 是失敗訊息的 JSON 陣列（空的＝通過）。
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const make = new Function('agent', 'log', src + '\nreturn { awfInit, awfKernelInit, awfAgent, awfTrace, awfCapped, challengeList, awfDispatch, awfConstraints, awfConstraintLines, awfPrefix, awfPathOverlap , awfInt, awfTune, awfRecordSections, awfDefs, awfLine, awfBranchPattern, awfBranch, awfStep, awfSwappable, awfConstraintFormat, awfConstraintRule, awfIntegrate, awfQuestionOptions, awfMaxAgents, awfMethNote, awfCommitFor, awfExtJob, awfDesignDocs, awfSubtaskRules, awfSdRules, awfDesignDocRules, awfDeltaRule, awfIntegrateConflicts }')
const PROFILE = { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'inherit' }, defaults: { '*': 'standard' }, floors: {} } }
const env = (s) => ({ status: 'done', summary: s, payload: { ok: true } })
const errs = []
const need = (c, m) => { if (!c) errs.push(m) }
;(async () => {
  // 1、3：label 重複 → 加序號；digest 記進 trace
  let n = 0
  let k = make(async () => env(`第 ${++n} 次交件`), () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'plan', modifiers: [], max_agents: 0 })
  await k.awfAgent('a', { label: 'ext:acme', agentType: 'agent-workflow:tpm-researcher' })
  await k.awfAgent('b', { label: 'ext:acme', agentType: 'agent-workflow:tpm-researcher' })
  await k.awfAgent('c', { label: 'other', agentType: 'agent-workflow:tpm-researcher' })
  let t = k.awfTrace()
  need(JSON.stringify(t.map((x) => x.label)) === JSON.stringify(['ext:acme', 'ext:acme#2', 'other']), `label 沒有加序號：${t.map((x) => x.label)}`)
  need(new Set(t.map((x) => x.record)).size === 3, `紀錄檔路徑撞了：${t.map((x) => x.record)}`)
  need(t.every((x) => x.digest && x.digest.status === 'done') && t[1].digest.summary === '第 2 次交件', `digest 沒記對：${JSON.stringify(t.map((x) => x.digest))}`)

  // 2：兩個同名派工，第二個第一次失敗後升級重派 → superseded 標在第二個的第一筆，第一個不受影響
  const seq = [env('甲交件'), null, env('乙升級後交件')]
  k = make(async () => seq.shift(), () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'plan', modifiers: [], max_agents: 0 })
  await k.awfAgent('a', { label: 'dup', agentType: 'agent-workflow:tpm-researcher' })
  const r2 = await k.awfAgent('b', { label: 'dup', agentType: 'agent-workflow:tpm-researcher' })
  t = k.awfTrace()
  need(r2 && r2.summary === '乙升級後交件', '第二個派工升級後沒有拿到交件')
  need(t.length === 3 && t[0].label === 'dup' && t[0].ok === true && !t[0].superseded, `第一個同名派工被動到了：${JSON.stringify(t[0])}`)
  need(t[1].label === 'dup#2' && t[1].ok === false && t[1].superseded === true, `superseded 沒標在自己那一筆：${JSON.stringify(t[1])}`)
  need(t[2].label === 'dup#2' && t[2].ok === true && t[2].attempt === 2, `重派那筆不對：${JSON.stringify(t[2])}`)

  // 4：派工數上限
  k = make(async () => env('ok'), () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'plan', modifiers: [], max_agents: 1 })
  await k.awfAgent('a', { label: 'one', agentType: 'agent-workflow:tpm-researcher' })
  const over = await k.awfAgent('b', { label: 'two', agentType: 'agent-workflow:tpm-researcher' })
  t = k.awfTrace()
  need(over === null && JSON.stringify(k.awfCapped()) === JSON.stringify(['two']) && t.length === 2, `上限沒擋住：${JSON.stringify(k.awfCapped())}`)
  need(t[1].label === 'two' && t[1].ok === false && t[1].capped === true && t[1].max_agents === 1 && /two\.md$/.test(t[1].record), `到上限沒派的要記一筆 capped 的 trace：${JSON.stringify(t[1])}`)

  // 6：片段裡的派工（challengeList）在啟用核心後也經過它：帶模型、算進上限；沒啟用核心時照舊直接派
  let seen = []
  k = make(async (p, o) => { seen.push(o); return { status: 'done', summary: 's', payload: { verdicts: [{ target_id: 'a', refuted: false, reason: 'ok' }] } } }, () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'meeting', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'meeting', modifiers: [], max_agents: 5 })
  let j = await k.challengeList([{ id: 'a' }], { prompt: 'p', agentType: 'agent-workflow:tpm-skeptic', schema: {} })
  t = k.awfTrace()
  need(!j.lacking && j.kept.length === 1 && seen[0].model === 'sonnet' && t[0].tier === 'standard' && t[0].model === 'sonnet', `challengeList 沒有經過核心：${JSON.stringify([seen[0], t[0]])}`)
  k.awfKernelInit({ profile: PROFILE, stage: 'meeting', modifiers: [], max_agents: 1 })
  await k.awfAgent('a', { label: 'one', agentType: 'agent-workflow:tpm-researcher' })
  const missing = []
  j = await k.challengeList([{ id: 'a' }], { prompt: 'p', agentType: 'agent-workflow:tpm-skeptic', schema: {}, missing })
  need(j.lacking && missing.join() === 'challenge' && k.awfCapped().join() === 'challenge', `skeptic 也要算進派工數上限：${JSON.stringify([j.lacking, missing, k.awfCapped()])}`)
  seen = []
  k.awfInit({ run_dir: '_runs/r1', stage: 'meeting', lang: 'zh-TW' })     // 重新 init 而且沒有啟用核心
  await k.challengeList([{ id: 'a' }], { prompt: 'p', agentType: 'agent-workflow:tpm-skeptic', schema: {} })
  need(seen.length === 1 && seen[0].model === undefined && k.awfTrace()[0].tier === undefined, `沒啟用核心時不該指定模型：${JSON.stringify(seen[0])}`)

  // 7：會動到共用分支的角色（integrator、steward）與 retry:false 的派工，失敗不自動升級重派
  for (const [role, opt, want] of [['dev-integrator', {}, 1], ['dev-merge-steward', {}, 1], ['dev-implementer', { retry: false }, 1], ['dev-implementer', {}, 2]]) {
    let n = 0
    k = make(async () => { n++; return null }, () => {})
    k.awfInit({ run_dir: '_runs/r1', stage: 'build', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'build', modifiers: [], max_agents: 9 })
    const r = await k.awfAgent('x', { label: 'w', agentType: 'agent-workflow:' + role, ...opt })
    need(r === null && n === want && k.awfTrace().length === want && k.awfTrace().every((x) => x.ok === false), `${role} ${JSON.stringify(opt)}：應該派 ${want} 次，實際 ${n}`)
  }

  // 5：限制文件。整段適用／build 只給路徑對得上的子任務／格式不對的略過／前綴帶出規則與對照表格式
  const CP = { constraints: [
    { id: 'arch', file: 'ARCHITECTURE.md', note: '分層', stages: ['design', 'build', 'deliver'], trace: 'table' },
    { id: 'frontend', file: 'docs/fe.md', stages: ['build', 'deliver'], paths: ['web/**', '**/*.tsx'] },
    { id: 'backend', file: 'docs/be.md', stages: ['build'], paths: ['services/api/**'] },
    { id: 'bad', stages: ['design'] }, 'x', { id: 'nostage', file: 'a.md' },
  ] }
  k = make(async () => env('ok'), () => {})
  const ids = (l) => l.map((c) => c.id).join(',')
  need(ids(k.awfConstraints(CP, 'design')) === 'arch', `design 段：${ids(k.awfConstraints(CP, 'design'))}`)
  need(ids(k.awfConstraints(CP, 'deliver')) === 'arch,frontend', `deliver 段整段適用（含有 paths 的）：${ids(k.awfConstraints(CP, 'deliver'))}`)
  need(ids(k.awfConstraints(CP, 'build')) === 'arch', `build 段整段只取沒有 paths 的：${ids(k.awfConstraints(CP, 'build'))}`)
  need(ids(k.awfConstraints(CP, 'build', ['web/src/App.tsx'])) === 'frontend', `前端子任務：${ids(k.awfConstraints(CP, 'build', ['web/src/App.tsx']))}`)
  need(ids(k.awfConstraints(CP, 'build', ['services/api/orders.py'])) === 'backend', `後端子任務不該收到前端規範：${ids(k.awfConstraints(CP, 'build', ['services/api/orders.py']))}`)
  need(ids(k.awfConstraints(CP, 'build', ['services/'])) === 'backend', `子任務擁有上層目錄也算對得上：${ids(k.awfConstraints(CP, 'build', ['services/']))}`)
  need(ids(k.awfConstraints(CP, 'build', ['docs/readme.md'])) === '', `無關的子任務：${ids(k.awfConstraints(CP, 'build', ['docs/readme.md']))}`)
  need(ids(k.awfConstraints(CP, 'plan')) === '' && ids(k.awfConstraints(null, 'design')) === '' && ids(k.awfConstraints({ constraints: 'x' }, 'design')) === '', '沒有對得上的或 profile 沒有 constraints 要回空')
  need(k.awfPathOverlap('lib/*.py', 'lib/a.py') && !k.awfPathOverlap('lib/*.py', 'src/a.py') && k.awfPathOverlap('**/*.tsx', 'a/b/C.tsx') && !k.awfPathOverlap('**/*.tsx', 'a/b/c.py'), 'glob 比對不對')
  k.awfInit({ run_dir: '_runs/r1', stage: 'deliver', lang: 'zh-TW', constraints: k.awfConstraints(CP, 'deliver') })
  const pre = k.awfPrefix()
  need(/\[arch\] ARCHITECTURE\.md：分層（要對照）/.test(pre) && /\[frontend\] docs\/fe\.md（只適用於碰到 web\/\*\*、\*\*\/\*\.tsx 的部分）/.test(pre) && /\| <id> \| 符合／偏離／不適用 \|/.test(pre), `前綴沒有帶出限制文件：${pre.slice(-400)}`)
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', constraints: [] })
  need(!/限制文件/.test(k.awfPrefix()), '沒有限制時前綴不該多出規則')
  // 可調的值（1.0）：沒寫或不合法一律用預設；前綴在沒有 tune、沒有 kg 時逐字不變
  need(k.awfInt({ n: 5 }, 'n', 1, 9, 3) === 5 && k.awfInt({ n: 0 }, 'n', 1, 9, 3) === 3 && k.awfInt({ n: 10 }, 'n', 1, 9, 3) === 3 && k.awfInt({ n: 2.5 }, 'n', 1, 9, 3) === 3 && k.awfInt({ n: '5' }, 'n', 1, 9, 3) === 3 && k.awfInt(null, 'n', 1, 9, 3) === 3 && k.awfInt('x', 'n', 1, 9, 3) === 3, 'awfInt 的範圍或型別判斷不對')
  const T0 = JSON.stringify({ code_tools: ['graft', 'codegraph'], summary_chars: 400, evidence_items: 12, constraints_format: null, read_hook: false, kb: false })
  need(JSON.stringify(k.awfTune(null)) === T0 && JSON.stringify(k.awfTune({})) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: 'graft' }, limits: { envelope: { summary_chars: 401, evidence_items: 2 } } })) === T0, `awfTune 的預設不對：${JSON.stringify(k.awfTune(null))}`)
  need(JSON.stringify(k.awfTune({ knowledge: { code_tools: ['a', 'a'] } })) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: ['x\ny'] } })) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: Array.from({ length: 9 }, (_, i) => `t${i}`) } })) === T0, 'code_tools 重複、含換行或超過 8 個要退回預設')
  need(JSON.stringify(k.awfTune({ knowledge: { code_tools: [] }, limits: { envelope: { summary_chars: 100, evidence_items: 3 } } })) === JSON.stringify({ code_tools: [], summary_chars: 100, evidence_items: 3, constraints_format: null, read_hook: false, kb: false }), 'awfTune 沒有採用合法的設定')
  // 1.1：讀取預算與知識層沒啟用時，前綴不寫「hook 會擋超量讀取」與 awf kb query
  const flags = (p) => { const t = k.awfTune(p); return `${t.read_hook},${t.kb}` }
  need(flags({ limits: { mode: 'enforce' }, knowledge: { sources: ['docs/**/*.md'] } }) === 'true,true' && flags({ limits: { mode: 'warn' } }) === 'true,false' && flags({ limits: { mode: 'off' }, knowledge: { sources: [] } }) === 'false,false' && flags({ limits: { mode: 'x' }, knowledge: { sources: 'docs' } }) === 'false,false' && flags({ knowledge: { sources: [' ', 7] } }) === 'false,false', 'awfTune 的 read_hook／kb 判斷不對')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' })
  need(/- 省 context：大檔帶 offset／limit 分段讀。/.test(k.awfPrefix()) && /，再用 Grep 找行段，只讀那幾行/.test(k.awfPrefix()) && !/超量讀取|awf kb query/.test(k.awfPrefix()), '沒啟用讀取預算、沒設知識層時前綴不該提 hook 與 awf kb query')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', tune: k.awfTune({ limits: { mode: 'warn' }, knowledge: { sources: ['docs/**/*.md'] } }) })
  need(/- 省 context（hook 會擋超量讀取）：大檔帶/.test(k.awfPrefix()) && /，再用 awf kb query "<問題>" 或 Grep 找行段，只讀那幾行/.test(k.awfPrefix()), '啟用讀取預算、有知識層來源時前綴要照寫')
  const TL11 = [{ id: 'c1', file: 'docs/a.md', note: '', paths: [], table: false }]
  need(/先用 awf kb query 或 Grep 找相關行段再讀/.test(k.awfConstraintRule(TL11)) && /先用 awf kb query 或 Grep 找相關行段再讀/.test(k.awfConstraintRule(TL11, null, true)) && /；先用 Grep 找相關行段再讀/.test(k.awfConstraintRule(TL11, null, false)), 'awfConstraintRule 的 kb 參數不對')
  const SIX = '產出、驗證'
  const rs = (v) => k.awfRecordSections({ stages: { deliver: { record_sections: v } } }, SIX)
  need(rs(['甲', '乙']) === '甲、乙' && rs([]) === SIX && rs(['甲', '甲']) === SIX && rs(['甲\n乙']) === SIX && rs([' 甲']) === SIX && rs(['x'.repeat(41)]) === SIX && rs('甲') === SIX && rs(Array.from({ length: 13 }, (_, i) => `節${i}`)) === SIX && k.awfRecordSections(null, SIX) === SIX, 'awfRecordSections 的判斷不對')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' })
  const plain = k.awfPrefix()
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', kg: false, tune: k.awfTune(null), constraints: [] })
  need(k.awfPrefix() === plain, '帶預設的 tune、kg=false 時前綴要和沒帶時逐字相同')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', kg: 'true' })
  need(k.awfPrefix() === plain, 'kg 不是布林 true 時前綴不該變')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', kg: true })
  need(k.awfPrefix().startsWith(plain) && /awf kg locate "<文字>"/.test(k.awfPrefix().slice(plain.length)) && /awf kg impact <檔>/.test(k.awfPrefix()) && /_runs\/r1\/plan\/pack\.md 列了行段/.test(k.awfPrefix()), 'kg=true 時前綴要在最後多一行查圖的指示')
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW', tune: k.awfTune({ knowledge: { code_tools: ['ast-grep'] }, limits: { envelope: { summary_chars: 200, evidence_items: 6 } } }) })
  need(/程式碼有 ast-grep 就先用。/.test(k.awfPrefix()) && /summary 不超過 200 字、evidence 最多 6 則/.test(k.awfPrefix()) && !/graft/.test(k.awfPrefix()), 'tune 的值沒有寫進前綴')
  // 限制對照的用語（constraints_format）、合併方式、提問選項數：沒寫或不合法一律是內建的字（規則要和工具端的 constraint_format_errors 一致）
  const cf = (v) => k.awfConstraintFormat({ constraints_format: v })
  const EN = { heading: 'Constraint trace', verdicts: { ok: 'compliant', deviation: 'deviates', na: 'n/a' } }
  need(JSON.stringify(cf(EN)) === JSON.stringify({ heading: 'Constraint trace', ok: 'compliant', deviation: 'deviates', na: 'n/a' }), `awfConstraintFormat 沒有採用合法的設定：${JSON.stringify(cf(EN))}`)
  need(JSON.stringify(cf({ verdicts: { deviation: 'deviates' } })) === JSON.stringify({ heading: '限制對照', ok: '符合', deviation: 'deviates', na: '不適用' }), '只給部分的詞時，沒給的要用內建的')
  const BAD = [null, 'x', ['x'], {}, { extra: 1 }, { heading: '' }, { heading: ' x' }, { heading: 'a|b' }, { heading: 'a`b' }, { heading: 'a\nb' }, { heading: 'x'.repeat(41) }, { heading: 7 }, { heading: null }, { verdicts: 'x' }, { verdicts: { yes: 'y' } },
    { verdicts: { ok: 7 } }, { verdicts: { ok: 'same', na: 'same' } }, { verdicts: { ok: '偏離' } }, { verdicts: { na: '偏離' } }, { verdicts: { deviation: '符合' } }, { verdicts: { deviation: '不適用' } }, { verdicts: { ok: '不適用' } }, { verdicts: { na: '符合' } },
    { heading: 'ok', verdicts: { ok: 'a|b' } }, { heading: '限制對照', verdicts: { ok: '符合', deviation: '偏離', na: '不適用' } }]
  const kept = BAD.filter((v) => cf(v) !== null)
  need(kept.length === 0 && k.awfConstraintFormat(null) === null && k.awfConstraintFormat({}) === null, `awfConstraintFormat 對不合法或等於內建的設定要回 null：${JSON.stringify(kept)}`)
  need(cf({ heading: '字'.repeat(40) }) !== null, '40 個字的節名要採用（長度照字元數算）')
  const TL = [{ id: 'arch', file: 'A.md', note: '', paths: [], table: true }]
  need(k.awfConstraintRule(TL, null) === k.awfConstraintRule(TL) && k.awfConstraintRule(TL, 'x') === k.awfConstraintRule(TL) && /〈限制對照〉.*\| <id> \| 符合／偏離／不適用 \|/.test(k.awfConstraintRule(TL)), '沒給用語時規則要是內建的字')
  need(/〈Constraint trace〉一節.*\| <id> \| compliant／deviates／n\/a \|/.test(k.awfConstraintRule(TL, cf(EN))) && !/符合／偏離／不適用/.test(k.awfConstraintRule(TL, cf(EN))), `設定的用語沒有寫進規則：${k.awfConstraintRule(TL, cf(EN))}`)
  k.awfInit({ run_dir: '_runs/r1', stage: 'deliver', lang: 'en', constraints: TL })
  const preZh = k.awfPrefix()
  k.awfInit({ run_dir: '_runs/r1', stage: 'deliver', lang: 'en', constraints: TL, tune: k.awfTune({ constraints_format: { heading: ' bad' } }) })
  need(k.awfPrefix() === preZh, '用語不合法時前綴要和沒設定時逐字相同')
  k.awfInit({ run_dir: '_runs/r1', stage: 'deliver', lang: 'en', constraints: TL, tune: k.awfTune({ constraints_format: EN }) })
  need(/〈Constraint trace〉/.test(k.awfPrefix()) && /compliant／deviates／n\/a/.test(k.awfPrefix()) && !/〈限制對照〉/.test(k.awfPrefix()), 'awfTune 沒有把限制對照的用語帶進前綴')
  const ig = (v) => k.awfIntegrate({ conventions: { branch: { integrate: v } } })
  need(ig('merge') === 'merge' && ig('merge --no-ff') === 'merge --no-ff' && ig('merge --squash') === 'merge --no-ff' && ig('rebase') === 'merge --no-ff' && ig(7) === 'merge --no-ff' && ig(' merge') === 'merge --no-ff' && k.awfIntegrate(null) === 'merge --no-ff' && k.awfIntegrate({ conventions: 'x' }) === 'merge --no-ff', 'awfIntegrate 只該認 merge 與 merge --no-ff')
  const qo = (v) => k.awfQuestionOptions({ conventions: { questions: { options: v } } })
  need(qo('至少 2') === '至少 2' && qo('') === '2–3' && qo('a\nb') === '2–3' && qo('x'.repeat(21)) === '2–3' && qo(3) === '2–3' && k.awfQuestionOptions(null) === '2–3' && k.awfQuestionOptions({ conventions: { questions: [] } }) === '2–3', 'awfQuestionOptions 的判斷不對')
  // 名稱表、短字、分支樣式、換角色（1.0）：沒寫或不合法一律是原本的值
  const D = k.awfDefs({ ok: { ask: '問法' }, 'Bad Name': { ask: 'x' }, n1: 'x', n2: { ask: 7 }, n3: { ask: '  ' }, n4: { ask: 'x'.repeat(2001) }, n5: ['x'], __proto__: { ask: 'x' } }, 'ask')
  need(Object.keys(D).join(',') === 'ok' && Object.getPrototypeOf(D) === null, `awfDefs 只該收合法的項目：${Object.keys(D).join(',')}`)
  need(Object.keys(k.awfDefs(null, 'ask')).length === 0 && Object.keys(k.awfDefs(['x'], 'ask')).length === 0 && Object.keys(k.awfDefs('x', 'ask')).length === 0, 'awfDefs 對不是物件的表要回空')
  need(Object.keys(k.awfDefs(Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`k${i}`, { ask: 'x' }])), 'ask')).length === 24 && Object.keys(k.awfDefs({ a: { ask: 'x' }, b: { ask: 'x' }, c: { ask: 'x' } }, 'ask', 2)).length === 2, 'awfDefs 的個數上限不對')
  need(k.awfLine('§2', 40, '§1') === '§2' && k.awfLine(' §2', 40, '§1') === '§1' && k.awfLine('a\nb', 40, '§1') === '§1' && k.awfLine('a`b', 40, '§1') === '§1' && k.awfLine('', 40, '§1') === '§1' && k.awfLine(7, 40, '§1') === '§1' && k.awfLine('x'.repeat(41), 40, '§1') === '§1', 'awfLine 的判斷不對')
  const bp = (kind, v) => k.awfBranchPattern({ conventions: { branch: { patterns: { [kind]: v } } } }, kind)
  const TD = '<prefix><epic>--<task>'; const FD = '<prefix><epic>--fix<round>-<group>'
  need(k.awfBranchPattern(null, 'task') === TD && k.awfBranchPattern({}, 'fix') === FD && k.awfBranchPattern({ conventions: { branch: { patterns: ['x'] } } }, 'task') === TD, '分支樣式沒寫時要用內建的')
  need(bp('task', '<prefix><epic>.task-<task>') === '<prefix><epic>.task-<task>' && bp('fix', '<prefix><epic>_r<round>_<group>') === '<prefix><epic>_r<round>_<group>', '合法的分支樣式沒有被採用')
  for (const bad of ['<prefix><epic>/<task>', '<prefix><epic><task>', '<epic>--<task>', 'x<prefix><epic>--<task>', '<prefix><epic>--task', '<prefix><epic>--<task>-<task>', '<prefix><epic>--<task>..x', '<prefix><epic>--<task>.', '<prefix><epic>--<task>.lock', '<prefix><epic>--<task> x', '<prefix><epic>--<task>-<round>', '<prefix><epic>--<task>~1', '<prefix><epic>-' + 'x'.repeat(120) + '<task>', 7]) need(bp('task', bad) === TD, `不合法的 task 樣式要退回內建：${bad}`)
  for (const bad of ['<prefix><epic>--fix<round>', '<prefix><epic>--<group>', '<prefix><epic>--<round><group>', '<prefix><epic>/fix<round>-<group>']) need(bp('fix', bad) === FD, `不合法的 fix 樣式要退回內建：${bad}`)
  need(k.awfBranch(null, 'task', { prefix: 'agent/', epic: 'demo-1', task: 't1' }) === 'agent/demo-1--t1' && k.awfBranch(null, 'fix', { prefix: 'agent/', epic: 'demo-1', round: 2, group: 'api' }) === 'agent/demo-1--fix2-api', '內建樣式展開後要和原本寫死的分支名相同')
  need(k.awfBranch({ conventions: { branch: { patterns: { fix: '<prefix><epic>.r<round>.<group>' } } } }, 'fix', { prefix: 'agent/', epic: 'demo-1', round: 2, group: 'api' }) === 'agent/demo-1.r2.api', '自訂樣式展開不對')
  const st = (v, step) => JSON.stringify(k.awfStep({ roles: { [step || 'plan.research']: v } }, 'plan.research', 'agent-workflow:'))
  const ST0 = JSON.stringify({ agentType: 'agent-workflow:tpm-researcher' })
  need(JSON.stringify(k.awfStep(null, 'plan.research', 'agent-workflow:')) === ST0 && JSON.stringify(k.awfStep({ roles: ['x'] }, 'plan.research', 'agent-workflow:')) === ST0, '沒設定 roles 時要是原本的模版角色、不帶 roleAs')
  need(st('acme-own-agent') === JSON.stringify({ agentType: 'acme-own-agent', roleAs: 'tpm-researcher' }) && st('my-plugin:own.agent_1') === JSON.stringify({ agentType: 'my-plugin:own.agent_1', roleAs: 'tpm-researcher' }), '合法的 roles 沒有被採用')
  for (const bad of ['agent-workflow:tpm-writer', 'dev-implementer', 'x:dev-merge-steward', 'has space', 'a:b:c', '', 7, 'x\ny', 'x'.repeat(65)]) need(st(bad) === ST0, `不合法的 roles 值要照原本的角色：${JSON.stringify(bad)}`)
  need(st('acme-own-agent', 'plan.locate') === ST0, '別的步驟的設定不該影響這一步')
  let threw = false; try { k.awfStep({}, 'merge.analyze', 'agent-workflow:') } catch (e) { threw = true }
  need(threw && !Object.keys(k.awfSwappable()).some((x) => /^(merge|build|test)\./.test(x)), 'merge、build、test 的步驟不開放換角色')
  // 換上去的 agent 照被換掉的角色套模型分級與下限（roleAs）；模版角色自己帶 roleAs 不影響
  seen = []
  k = make(async (p, o) => { seen.push(o); return env('ok') }, () => {})
  const FP = { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'light', 'tpm-skeptic': 'strong' }, floors: {} } }
  k.awfInit({ run_dir: '_runs/r1', stage: 'design', lang: 'zh-TW' }); k.awfKernelInit({ profile: FP, stage: 'design', modifiers: [], max_agents: 9 })
  await k.awfAgent('x', { label: 'own', agentType: 'acme-own-agent', roleAs: 'tpm-skeptic' })
  await k.awfAgent('x', { label: 'tpl', agentType: 'agent-workflow:tpm-skeptic' })
  await k.awfAgent('x', { label: 'plain', agentType: 'acme-own-agent' })
  await k.awfAgent('x', { label: 'spoof', agentType: 'agent-workflow:tpm-skeptic', roleAs: 'tpm-writer' })
  const tr = Object.fromEntries(k.awfTrace().map((t) => [t.label, t]))
  need(tr.own.tier === tr.tpl.tier && tr.own.model === tr.tpl.model && tr.own.role_as === 'tpm-skeptic', `換上去的 agent 要和被換掉的角色同一級：${JSON.stringify([tr.own, tr.tpl])}`)
  need(tr.plain.role_as === undefined && tr.tpl.role_as === undefined, '沒有換角色的派工不該記 role_as')
  need(tr.spoof.tier === tr.tpl.tier && tr.spoof.model === tr.tpl.model, '模版角色帶 roleAs 不能改變它自己的模型分級')
  // 頂替不重派角色的 agent（roleAs 是 dev-merge-steward、dev-integrator）失敗時不升級重派；頂替別的角色的照常重派一次
  let calls = []
  k = make(async (p, o) => { calls.push(o.label); return null }, () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'merge', lang: 'zh-TW' }); k.awfKernelInit({ profile: FP, stage: 'merge', modifiers: [], max_agents: 9 })
  await k.awfAgent('x', { label: 'merger', agentType: 'acme:release-merger', roleAs: 'dev-merge-steward' })
  await k.awfAgent('x', { label: 'integ', agentType: 'acme:own-integrator', roleAs: 'dev-integrator' })
  await k.awfAgent('x', { label: 'swapped', agentType: 'acme-own-agent', roleAs: 'tpm-researcher' })
  const cnt = (l) => calls.filter((x) => x === l).length
  need(cnt('merger') === 1 && cnt('integ') === 1 && cnt('swapped') === 2, `頂替不重派角色的 agent 不該重派：${JSON.stringify(calls)}`)
  const mt = k.awfTrace().find((t) => t.label === 'merger')
  need(mt.role_as === 'dev-merge-steward' && mt.tier === 'strong' && mt.ok === false, `頂替合併角色的 agent 要照 dev-merge-steward 的下限：${JSON.stringify(mt)}`)
  // 1.1：八支 sdlc 範本共用的函式（派工數上限、profile 那一行、commit 寫法、擴充 agent 的派工單）
  const ma = (v, x) => k.awfMaxAgents({ limits: { max_agents: v } }, x)
  need(ma(12) === 12 && ma(1) === 1 && ma(1000) === 1000 && ma(0) === 40 && ma(-3) === 40 && ma(1001) === 40 && ma(2.5) === 40 && ma('12') === 40 && k.awfMaxAgents(null) === 40 && k.awfMaxAgents({}) === 40 && k.awfMaxAgents({ limits: 7 }) === 40, 'awfMaxAgents 的範圍判斷不對')
  need(ma(12, 8) === 20 && ma(undefined, 16) === 56 && ma(12, -4) === 12 && ma(12, 1.5) === 12 && ma(12, '8') === 12, 'awfMaxAgents 的另加額度不對')
  need(k.awfMethNote(null) === '開發規範 profile：未設定（沒有 .claude/teams/methodology.json），做法與慣例依目標 repo 既有規範。' && k.awfMethNote({ name: 'acme' }) === '開發規範 profile：acme（.claude/teams/methodology.json；各段做法與慣例以它為準）。文件格式：依目標 repo。' && /文件格式：Markdown。$/.test(k.awfMethNote({ name: 'acme', conventions: { doc_format: { format: 'Markdown' } } })), 'awfMethNote 的字不對')
  const cf11 = (c) => k.awfCommitFor({ conventions: { commit: c } }, 'docs', 'acme-1')
  need(k.awfCommitFor(null, 'docs', 'e') === 'commit 訊息照目標 repo 的慣例' && cf11({}) === 'commit 訊息照目標 repo 的慣例' && cf11({ format: '<type>: <摘要>' }) === 'commit 訊息照 profile 的格式：<type>: <摘要>' && cf11({ format: 'F', examples: { docs: 'docs(<epic>): x <epic>' } }) === 'commit 訊息照 profile 的格式：F（範例：docs(acme-1): x acme-1）' && cf11({ format: 'F', examples: { feat: 'y' } }) === 'commit 訊息照 profile 的格式：F', 'awfCommitFor 的字不對')
  seen = []
  k = make(async (p, o) => { seen.push([p, o]); return env('ok') }, () => {})
  k.awfInit({ run_dir: '_runs/r1', stage: 'plan', lang: 'zh-TW' }); k.awfKernelInit({ profile: PROFILE, stage: 'plan', modifiers: [], max_agents: 1 })
  const ej11 = k.awfExtJob({ name: 'acme-audit', agent: 'acme:auditor', instructions: '查示範客戶 ACME 的規範' }, '背景', 'Research', '_runs/r1', { type: 'object' })
  need(typeof ej11 === 'function' && seen.length === 0, 'awfExtJob 要回函式、呼叫前不派工')
  await ej11()
  t = k.awfTrace()
  need(seen.length === 1 && seen[0][1].label === 'ext:acme-audit' && seen[0][1].agentType === 'acme:auditor' && seen[0][1].phase === 'Research' && seen[0][1].schema.type === 'object' && seen[0][0].includes('## 派工單（擴充 agent：acme-audit）\n查示範客戶 ACME 的規範\n只讀不改') && seen[0][0].includes('_runs/r1/ext-acme-audit.md') && seen[0][0].includes('lens=acme-audit'), `awfExtJob 的派工單或 opts 不對：${JSON.stringify(seen[0] && seen[0][1])}`)
  need(t.length === 1 && (await ej11()) === null && k.awfCapped().join() === 'ext:acme-audit#2', 'awfExtJob 要經過執行核心（派工數上限）')
  // 1.1：design 兩支範本共用的規則文字
  const dd = (a, w) => k.awfDesignDocs({ stages: { design: { artifacts: a } } }, w).join('、')
  need(k.awfDesignDocs(null).join('、') === 'intent、design、spec、plan、progress' && k.awfDesignDocs(null, false).join('、') === 'design、spec、plan、progress' && dd(['spec.md', 'plan.md']) === 'intent、spec、plan' && dd(['spec.md', 'plan.md'], false) === 'spec、plan' && dd(['intent.md', 'SPEC.MD', 'adr']) === 'intent、SPEC、adr' && dd([]) === 'intent、design、spec、plan、progress' && dd(['a\nb']) === 'intent、design、spec、plan、progress' && dd('spec.md') === 'intent、design、spec、plan、progress' && dd([7]) === 'intent、design、spec、plan、progress', 'awfDesignDocs 的判斷不對')
  need(/數量最小化。$/.test(k.awfSubtaskRules(null)) && /spec_refs/.test(k.awfSubtaskRules({})) && /expected_red/.test(k.awfSubtaskRules({})) && /code 子任務不要依賴 test 子任務。$/.test(k.awfSubtaskRules({ stages: { build: { test_first: false } } })) && !/不要求測試先行/.test(k.awfSubtaskRules({ stages: { build: { test_first: true } } })), 'awfSubtaskRules 的字不對')
  const sd11 = { source: 'S', steps: ['一', '二'], feature_scope: 'F', system_scope: 'Y' }
  need(k.awfSdRules(null) === '' && k.awfSdRules({ stages: { design: { system_design: null } } }) === '' && k.awfSdRules({ stages: { design: { system_design: sd11 } } }) === '- 系統設計（SA／SD，方法出處：S）：\n  步驟：一；二\n  功能層：F\n  系統層：Y\n  寫在：design.md 的「系統設計」節，第一行寫這次是功能層還是系統層與判斷理由。\n' && /第 4 步逐項檢查的主題：甲、乙\n  寫在：D。\n$/.test(k.awfSdRules({ stages: { design: { system_design: { ...sd11, topics: ['甲', '乙'], document: 'D' } } } })), 'awfSdRules 的字不對')
  need(k.awfDesignDocRules(null, 'specs/x') === '- 產出文件（放在 specs/x/）：spec.md、plan.md（profile 沒列的不必寫）；架構取捨怎麼記錄依 profile 的指引，沒寫就依目標 repo 慣例。\n- 需求／驗收條件的寫法：照目標 repo 既有的 spec 寫法；每條可機械判定，逐條對到需求 ID，數值與錯誤碼照原文。\n' && /：a\.md（profile 沒列的不必寫）[^\n]*\n- 需求／驗收條件的寫法：R。\n- profile 的指引：G\n- 系統設計/.test(k.awfDesignDocRules({ stages: { design: { artifacts: ['a.md'], requirement_style: 'R', guidance: 'G', system_design: sd11 } } }, 'd')), 'awfDesignDocRules 的字不對')
  need(k.awfDeltaRule('change', 'impact').startsWith('- 這是變更需求（recipe change，plan_mode=impact）：寫 delta spec') && k.awfDeltaRule('feature', 'feature') === '' && k.awfDeltaRule('change', undefined) === '', 'awfDeltaRule 的判斷不對')
  const ic = (v) => k.awfIntegrateConflicts({ conventions: { branch: { integrate_conflicts: v } } })
  const TRANSIT = '有衝突照目標 repo 的分支與合併規範用中轉分支解並分類'
  need(ic('transit') === TRANSIT && ic(undefined) === TRANSIT && ic('rebase') === TRANSIT && ic(' stop') === TRANSIT && ic(1) === TRANSIT && k.awfIntegrateConflicts(null) === TRANSIT && /^有衝突就在整合分支上直接解並分類/.test(ic('in-place')) && /^有衝突不要自己解：git merge --abort/.test(ic('stop')), 'awfIntegrateConflicts 的判斷不對')
  process.stdout.write(JSON.stringify(errs))
})().catch((e) => { process.stdout.write(JSON.stringify([`例外：${e && e.stack || e}`])) })
