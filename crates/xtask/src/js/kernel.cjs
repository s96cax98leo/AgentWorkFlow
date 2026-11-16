// check-kernel 的測試本體：對執行核心片段（challenge、recipe、kernel）直接測。原本在 0.x 的 tests/check_kernel.py 裡。
// 用法：node -e <本檔> <片段檔…>；stdout 是失敗訊息的 JSON 陣列（空的＝通過）。
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const make = new Function('agent', 'log', src + '\nreturn { awfInit, awfKernelInit, awfAgent, awfTrace, awfCapped, challengeList, awfDispatch, awfConstraints, awfConstraintLines, awfPrefix, awfPathOverlap , awfInt, awfTune, awfRecordSections }')
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
  const T0 = JSON.stringify({ code_tools: ['graft', 'codegraph'], summary_chars: 400, evidence_items: 12 })
  need(JSON.stringify(k.awfTune(null)) === T0 && JSON.stringify(k.awfTune({})) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: 'graft' }, limits: { envelope: { summary_chars: 401, evidence_items: 2 } } })) === T0, `awfTune 的預設不對：${JSON.stringify(k.awfTune(null))}`)
  need(JSON.stringify(k.awfTune({ knowledge: { code_tools: ['a', 'a'] } })) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: ['x\ny'] } })) === T0 && JSON.stringify(k.awfTune({ knowledge: { code_tools: Array.from({ length: 9 }, (_, i) => `t${i}`) } })) === T0, 'code_tools 重複、含換行或超過 8 個要退回預設')
  need(JSON.stringify(k.awfTune({ knowledge: { code_tools: [] }, limits: { envelope: { summary_chars: 100, evidence_items: 3 } } })) === JSON.stringify({ code_tools: [], summary_chars: 100, evidence_items: 3 }), 'awfTune 沒有採用合法的設定')
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
  process.stdout.write(JSON.stringify(errs))
})().catch((e) => { process.stdout.write(JSON.stringify([`例外：${e && e.stack || e}`])) })
