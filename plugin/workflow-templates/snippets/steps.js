// ── 片段 steps（workflow-templates/snippets/steps.js）：用資料宣告步驟的引擎（自訂段 run: "steps" 與事件的 steps 共用）──
// @requires: challenge, recipe, kernel
// awf build 內嵌；不要在 workflow 裡手改，改片段後重跑 awf build。只定義函式，需要的值都用參數傳入。
// 使用者在 profile 用資料宣告一串步驟（不用寫 JS），這裡照順序跑。五種步驟：
//   fanout     多個角度平行，各一張派工單，交 findings。
//   challenge  對前面 fanout 的發現反駁（verdicts）；batch＝每批幾則（沒寫＝全部一批）；optional＝沒有東西可反駁時跳過（沒寫＝停下交人）。
//   write      tpm-writer 把素材寫成文件（docs）。
//   loop       把一組步驟（fanout／challenge／write）重複最多 max_rounds 輪；每輪最後由一個唯讀角色原樣執行 until.command 並回報結束碼，
//              過了就結束，跑滿還沒過就 halted。agent 的回報只決定「要不要再跑一輪」；存檔時工具會自己重跑同一個指令核對。
//   check      確定性指令：這裡不派 agent、不執行，只留記號；由 awf save-run stage 在存檔時執行。
// 共同規則（沿用 patterns 片段的慣例）：
//   1. 派工一律走執行核心的 awfAgent（模型分級、失敗升級、派工數上限、trace）。
//   2. 上限寫死在 awfStepsLimits；定義不合規則、最多可能的派工數超過 limits.max_agents 時不派任何 agent、回 halted（不靜默截斷）。
//   3. 缺件不是「沒有結果」：有 agent 沒交件就回 halted，並在 missing 列出 label。
//   4. 指令（until.command、check.command）只來自 profile，文字原樣使用：不代入任何參數（args 與 agent 的輸出都進不了指令字串）。
// 規則要和工具端（crates/awf-core/src/steps.rs）一致：工具依同一份定義算「每一步該有哪些交件」並核對 trace。
function awfStepsLimits() { return { steps: 12, angles: 8, loop_steps: 4, rounds: 5, batch: 50, batches: 8, data: 8, text: 4000, command: 1000 } }
// 唯讀步驟（fanout、challenge、loop 的判定）可以用的模版角色：都沒有 Edit，而且有 Bash（判定要跑指令）。write 固定 tpm-writer。
function awfStepsRoles() { return ['tpm-researcher', 'tpm-skeptic', 'tpm-reviewer', 'dev-verifier', 'dev-design-reviewer', 'dev-security-reviewer'] }
// {名稱} 換成值；{名稱|預設} 在沒有值時用預設；不認得又沒有預設的原樣留著（和 event.js 的 fill 相同）。只處理樣板本身，不處理之後接上去的資料
function awfStepsFill(tpl, vars) {
  return String(tpl || '').replace(/\{([A-Za-z_][A-Za-z0-9_]*)(?:\|([^{}]*))?\}/g, (m, k, d) => {
    const v = (vars || {})[k]
    return v === undefined || v === null || v === '' ? (d !== undefined ? d : m) : String(v)
  })
}
// role 可以是 awfStepsRoles 裡的角色名（不帶命名空間），或使用者自己的 agent（完整的 agent_type；要登記在 guard.config.json 的 extension_agents，
// 由 awf 產生腳本時核對，沒登記就拒絕）。模版角色的其他寫法（帶命名空間、不在名單裡的 tpm-／dev-）一律不收。
function awfStepsRoleOk(role) {
  if (role === undefined) return true
  if (typeof role !== 'string') return false
  if (awfStepsRoles().includes(role)) return true
  return /^([A-Za-z0-9][A-Za-z0-9._-]{0,63}:)?[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(role) && !/^(tpm|dev)-/.test(role.split(':').pop())
}
// 這一步要派的 agent：{ agentType[, roleAs] }。換成使用者自己的 agent 時 roleAs＝這一步預設的模版角色（模型下限照它算）。
function awfStepsAgent(role, dflt, ns) {
  if (role === undefined || role === null) return { agentType: String(ns || '') + dflt }
  if (awfStepsRoles().includes(role)) return { agentType: String(ns || '') + role }
  return { agentType: role, roleAs: dflt }
}
// 一輪裡會用到的 label（loop 裡的步驟以第 1 輪為例）：檢查紀錄檔名（awfSlug）會不會撞在一起
function awfStepsLabels(steps, pre) {
  const out = []
  for (const s of Array.isArray(steps) ? steps : []) {
    if (!s || typeof s !== 'object') continue
    const p = String(pre || '')
    if (s.kind === 'fanout') for (const a of Array.isArray(s.angles) ? s.angles : []) out.push(`${p}${s.id}:${a && a.key}`)
    else if (s.kind === 'challenge') { out.push(`${p}${s.id}`); if (s.batch !== undefined) out.push(`${p}${s.id}:b1`) }
    else if (s.kind === 'write') out.push(`${p}${s.id}`)
    else if (s.kind === 'loop') { out.push(...awfStepsLabels(s.steps, `${s.id}.r1:`)); out.push(`${s.id}.r1:until`) }
  }
  return out
}
// 步驟定義不合規則的地方（空陣列＝可以跑）。profile＝內嵌的 profile（只用它的 limits.max_agents）。
function awfStepsErrors(steps, profile) {
  const L = awfStepsLimits()
  const errs = []
  if (!Array.isArray(steps) || !steps.length || steps.length > L.steps) return [`steps 要是 1～${L.steps} 步的陣列`]
  const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k)
  const idRe = /^[a-z][a-z0-9-]{0,31}$/
  const text = (v) => typeof v === 'string' && v.trim() !== '' && v.length <= L.text
  const line = (v, max) => typeof v === 'string' && v.trim() !== '' && v.length <= max && !/[\u0000-\u001f\u007f]/.test(v)
  const timeout = (v) => v === undefined || (Number.isInteger(v) && v >= 1 && v <= 3600)
  const kinds = {}
  const refOk = (from) => {
    if (typeof from !== 'string') return false
    const [id, part, more] = from.split('.')
    if (more !== undefined || !has(kinds, id)) return false
    if (kinds[id] === 'fanout') return part === undefined
    if (kinds[id] === 'challenge') return part === 'kept' || part === 'doubted'
    if (kinds[id] === 'loop') return part === 'until'
    return false
  }
  const dataOk = (w, d) => {
    if (d === undefined) return
    if (!Array.isArray(d) || d.length > L.data) { errs.push(`${w}.data 要是陣列（最多 ${L.data} 筆）`); return }
    for (const x of d) {
      if (!x || typeof x !== 'object' || !line(x.label, 40)) errs.push(`${w}.data 的每一筆要有 label（一行、40 字以內）`)
      else if (!refOk(x.from)) errs.push(`${w}.data 的 from「${String(x.from)}」指不到前面的步驟（fanout 寫 <id>；challenge 寫 <id>.kept 或 <id>.doubted；loop 寫 <id>.until）`)
    }
  }
  const one = (s, where, inLoop) => {
    if (!s || typeof s !== 'object' || Array.isArray(s)) { errs.push(`${where} 要是物件`); return 0 }
    const idOk = typeof s.id === 'string' && idRe.test(s.id) && s.id !== 'until'
    const w = idOk ? `步驟 ${s.id}` : where
    if (!idOk) errs.push(`${where}.id 要是小寫英數與連字號、英文字母開頭、32 字以內，而且不能是 until`)
    else if (has(kinds, s.id)) errs.push(`步驟的 id「${s.id}」重複`)
    const put = () => { if (idOk && !has(kinds, s.id)) kinds[s.id] = s.kind }
    if (s.heading !== undefined && !line(s.heading, 200)) errs.push(`${w}.heading 要是一行字（200 字以內）`)
    if (s.kind === 'fanout') {
      const angles = s.angles
      if (!Array.isArray(angles) || !angles.length || angles.length > L.angles) errs.push(`${w}.angles 要有 1～${L.angles} 個角度`)
      else {
        if (angles.some((a) => !a || typeof a !== 'object' || typeof a.key !== 'string' || !idRe.test(a.key) || !text(a.ask))) errs.push(`${w} 的每個角度都要有 key（小寫英數與連字號）與 ask`)
        if (new Set(angles.map((a) => a && a.key)).size !== angles.length) errs.push(`${w} 的角度 key 不能重複`)
      }
      if (s.body !== undefined && !text(s.body)) errs.push(`${w}.body 要是字串（${L.text} 字以內）`)
      if (!awfStepsRoleOk(s.role)) errs.push(`${w}.role 只能是 ${awfStepsRoles().join('、')}，或登記過的擴充 agent`)
      dataOk(w, s.data)
      put()
      return Array.isArray(angles) ? angles.length : 0
    }
    if (s.kind === 'challenge') {
      if (!text(s.ask)) errs.push(`${w}.ask 必填`)
      if (s.batch !== undefined && !(Number.isInteger(s.batch) && s.batch >= 1 && s.batch <= L.batch)) errs.push(`${w}.batch 要是 1～${L.batch} 的整數`)
      if (s.optional !== undefined && typeof s.optional !== 'boolean') errs.push(`${w}.optional 要是 true 或 false`)
      if (s.from !== undefined && !(Array.isArray(s.from) && s.from.length >= 1 && s.from.length <= L.steps && s.from.every((f) => typeof f === 'string' && kinds[f] === 'fanout' && has(kinds, f)))) errs.push(`${w}.from 要是前面 fanout 步驟的 id`)
      if (s.from === undefined && !Object.keys(kinds).some((k) => kinds[k] === 'fanout')) errs.push(`${w} 前面沒有 fanout 步驟，沒有東西可以反駁`)
      if (!awfStepsRoleOk(s.role)) errs.push(`${w}.role 只能是 ${awfStepsRoles().join('、')}，或登記過的擴充 agent`)
      put()
      return s.batch === undefined ? 1 : L.batches
    }
    if (s.kind === 'write') {
      if (!text(s.ask)) errs.push(`${w}.ask 必填`)
      if (s.role !== undefined) errs.push(`${w}：write 固定由 tpm-writer 做，不能寫 role`)
      if (s.owner_check !== undefined && typeof s.owner_check !== 'boolean') errs.push(`${w}.owner_check 要是 true 或 false`)
      dataOk(w, s.data)
      put()
      return 1
    }
    if (s.kind === 'loop') {
      if (inLoop) { errs.push(`${w}：loop 裡不能再放 loop`); return 0 }
      // 先佔住 id（裡面的步驟不能和它同名，也不能引用還沒跑完的自己）
      const mine = idOk && !has(kinds, s.id)
      if (mine) kinds[s.id] = 'pending'
      const rounds = Number.isInteger(s.max_rounds) && s.max_rounds >= 1 && s.max_rounds <= L.rounds
      if (!rounds) errs.push(`${w}.max_rounds 必填，要是 1～${L.rounds} 的整數`)
      const u = s.until
      if (!u || typeof u !== 'object' || Array.isArray(u) || !line(u.command, L.command)) errs.push(`${w}.until.command 必填：一行的指令（${L.command} 字以內）`)
      else {
        if (!timeout(u.timeout)) errs.push(`${w}.until.timeout 要是 1～3600 的整數（秒）`)
        if (!awfStepsRoleOk(u.role)) errs.push(`${w}.until.role 只能是 ${awfStepsRoles().join('、')}，或登記過的擴充 agent`)
      }
      let inner = 0
      if (!Array.isArray(s.steps) || !s.steps.length || s.steps.length > L.loop_steps) errs.push(`${w}.steps 要有 1～${L.loop_steps} 步`)
      else s.steps.forEach((x, i) => { inner += one(x, `${w}.steps[${i}]`, true) })
      if (mine) kinds[s.id] = 'loop'
      return (rounds ? s.max_rounds : L.rounds) * (inner + 1)
    }
    if (s.kind === 'check') {
      if (inLoop) { errs.push(`${w}：check 不能放在 loop 裡（loop 的結束條件寫在 until）`); return 0 }
      if (!line(s.command, L.command)) errs.push(`${w}.command 必填：一行的指令（${L.command} 字以內）`)
      if (!timeout(s.timeout)) errs.push(`${w}.timeout 要是 1～3600 的整數（秒）`)
      put()
      return 0
    }
    errs.push(`${w}.kind 要是 fanout、challenge、write、loop、check 其中之一`)
    return 0
  }
  let total = 0
  steps.forEach((s, i) => { total += one(s, `steps[${i}]`, false) })
  if (errs.length) return errs
  const slugs = awfStepsLabels(steps, '').map((l) => awfSlug(l))
  const clash = slugs.filter((x, i) => slugs.indexOf(x) !== i)
  if (clash.length) errs.push(`步驟與角度的名字組起來會撞名（紀錄檔 ${[...new Set(clash)].join('、')}）：換一個 id 或 key`)
  const cap = awfMaxAgents(profile)
  if (total > cap) errs.push(`這份定義最多可能派 ${total} 個 agent，超過上限 ${cap}（limits.max_agents）：減少角度、輪數或步驟，或調高上限`)
  return errs
}
// 這份定義裡由工具端執行的指令（check 與 loop 的 until），照宣告順序：[{ id, kind }]。指令文字不放進回傳（工具只認 profile 寫的）。
function awfStepsChecks(steps) {
  return (Array.isArray(steps) ? steps : []).filter((s) => s && (s.kind === 'check' || s.kind === 'loop')).map((s) => ({ id: s.id, kind: s.kind === 'loop' ? 'until' : 'check' }))
}
// 規則 12：給決策者的文件要過 check-rules --audience owner；writer 沒回報或沒過回理由（null＝通過）
function awfStepsRuleCheck(d) {
  const rc = String(((d || {}).payload || {}).rule_check || '').trim()
  if (!rc) return 'writer 沒有回報 rule_check（awf check-rules 的結果）'
  if (!/^pass\b/i.test(rc)) return `awf check-rules 沒過：${rc}`
  if (!(d.payload.documents || []).some((x) => x.audience === 'owner')) return 'writer 沒有交出給決策者版（documents 沒有 audience=owner）'
  return null
}
// 跑一份步驟定義。def：{ steps }；o：{ ns, vars（文字樣板的參數，通常是 args）, context（每張派工單開頭的背景，已經代入好）, cwd（給人看的工作目錄說明）,
//   profile, schemas: { findings, verdicts, docs, criteria } }。
// 回 { halted, stage?, halt_reason?, missing, capped?, steps: { <id>: 這一步的結果 }, checks: [{ id, kind }], docs? }；
// 回傳裡不放 null（awf save-run 把信封以外的 null 當成 agent 缺件）。loop 裡的步驟只留最後一輪的結果（每一輪的紀錄都在 trace 與紀錄檔）。
async function awfStepsRun(def, o) {
  const c = o || {}
  const steps = (def || {}).steps
  const checks = awfStepsChecks(steps)
  const errs = awfStepsErrors(steps, c.profile)
  if (errs.length) return { halted: true, stage: 'steps', halt_reason: `步驟定義不合規則（沒有派出任何 agent）：${errs.join('；')}`, missing: [], steps: {}, checks: [], invalid_definition: true }
  const L = awfStepsLimits()
  const S = c.schemas || {}
  const vars = c.vars || {}
  const ctx = String(c.context || '')
  const ns = String(c.ns || '')
  const cwd = String(c.cwd || '專案根目錄')
  const pool = {}
  // challenge 沒寫 from 時反駁「宣告在它前面的 fanout」（照宣告順序，loop 裡的也算；和第幾輪無關）
  const before = {}
  {
    const fans = []
    const walk = (list) => { for (const s of list) { if (s.kind === 'fanout') fans.push(s.id); else if (s.kind === 'challenge') before[s.id] = [...fans]; else if (s.kind === 'loop') walk(s.steps) } }
    walk(steps)
  }
  const out = {}
  const missing = []
  let docs
  const done = (extra) => ({ ...extra, missing: [...missing], ...(awfCapped().length ? { capped: awfCapped() } : {}), steps: out, checks, ...(docs ? { docs } : {}) })
  const stop = (stage, reason, extra) => done({ halted: true, stage, halt_reason: reason, ...(extra || {}) })
  const lacking = (stage) => {
    const cap = awfCapped()
    const capped = missing.some((l) => cap.some((x) => x === l || x.startsWith(`${l}#`)))
    return stop(stage, capped ? `派工數到上限，沒派出：${missing.join('、')}` : `缺件：${missing.join('、')}`)
  }
  const dataLines = (specs) => (Array.isArray(specs) ? specs : []).map((d) => `${d.label}：${JSON.stringify(pool[d.from] === undefined ? [] : pool[d.from])}`).join('\n')
  // 每個步驟的執行函式回 null＝繼續；回物件＝停下（就是整支的回傳）
  const fanout = async (s, pre, note) => {
    phase('Gather')
    const who = awfStepsAgent(s.role, 'tpm-researcher', ns)
    const extra = dataLines(s.data)
    const got = await parallel(s.angles.map((a) => () => {
      // 角度的 ask 先代入參數，再放進 body 的 {ask}（body 裡的 {ask} 只換一次，ask 換出來的字不會再被當成樣板）
      const v = { ...vars, ...a, ask: awfStepsFill(a.ask, vars) }
      return awfAgent(`${ctx}\n\n## ${awfStepsFill(s.heading || '派工單（{key}）', v)}\n${awfStepsFill(s.body || '{ask}', v)}${extra ? `\n${extra}` : ''}${note}\npayload 用 findings（lens=${a.key}）。`,
        { label: `${pre}${s.id}:${a.key}`, phase: 'Gather', ...who, schema: S.findings })
    }))
    // 各角度的發現各自編號，合併前加上步驟與角度的前綴（<步驟>:<角度>:<id>），反駁的 target_id 才不會撞號
    const list = []
    got.forEach((r, i) => {
      const k = s.angles[i].key
      if (!r) missing.push(`${pre}${s.id}:${k}`)
      else for (const f of r.payload.findings || []) list.push({ ...f, id: `${s.id}:${k}:${f.id}` })
    })
    if (missing.length) return lacking(s.id)
    out[s.id] = { kind: 'fanout', results: got, findings: list.length }
    pool[s.id] = list
    return null
  }
  const challenge = async (s, pre, note) => {
    phase('Challenge')
    const from = Array.isArray(s.from) && s.from.length ? s.from : before[s.id]
    const targets = from.flatMap((id) => pool[id] || [])
    if (!targets.length) {
      if (s.optional !== true) return stop(s.id, `步驟 ${s.id}：沒有東西可以反駁（前面的步驟沒有交出任何發現）。確定這種情況可以跳過，就在這一步寫 optional: true`)
      out[s.id] = { kind: 'challenge', skipped: true, targets: 0 }
      pool[`${s.id}.kept`] = []
      pool[`${s.id}.doubted`] = []
      return null
    }
    const size = Number.isInteger(s.batch) ? s.batch : targets.length
    const batches = []
    for (let i = 0; i < targets.length; i += size) batches.push(targets.slice(i, i + size))
    if (batches.length > L.batches) return stop(s.id, `步驟 ${s.id}：${targets.length} 則要分 ${batches.length} 批，超過上限 ${L.batches} 批（不截斷，交人決定要調大 batch 還是拆開）`, { over_limit: true })
    const who = awfStepsAgent(s.role, 'tpm-skeptic', ns)
    const labels = batches.map((_, i) => (batches.length > 1 ? `${pre}${s.id}:b${i + 1}` : `${pre}${s.id}`))
    const got = await parallel(batches.map((b, i) => () => awfAgent(
      `${ctx}\n\n## ${awfStepsFill(s.heading || '派工單', vars)}\n${awfStepsFill(s.ask, vars)}${note}\n每一則都要判，target_id 照抄 id（含前綴）；不確定判 refuted=true。payload 用 verdicts。\n${JSON.stringify(b)}`,
      { label: labels[i], phase: 'Challenge', ...who, schema: S.verdicts })))
    got.forEach((r, i) => { if (!r) missing.push(labels[i]) })
    if (missing.length) return lacking(s.id)
    // 沒判到的當成未驗證，和被推翻的一起列存疑（challenge 片段的 judgeVerdicts）
    const judged = judgeVerdicts(targets, { payload: { verdicts: got.flatMap((r) => r.payload.verdicts || []) } })
    out[s.id] = { kind: 'challenge', results: got, targets: targets.length, kept: judged.kept.map((x) => x.id), doubted: judged.doubted.map((x) => x.id) }
    pool[`${s.id}.kept`] = judged.kept
    pool[`${s.id}.doubted`] = judged.doubted
    return null
  }
  const write = async (s, pre, note) => {
    phase('Write')
    const data = dataLines(s.data)
    const label = `${pre}${s.id}`
    const r = await awfAgent(`${ctx}\n\n## ${awfStepsFill(s.heading || '派工單', vars)}\n${awfStepsFill(s.ask, vars)}${data ? `\n${data}` : ''}${note}\npayload 用 docs。`,
      { label, phase: 'Write', agentType: ns + 'tpm-writer', schema: S.docs })
    if (!r) { missing.push(label); return lacking(s.id) }
    out[s.id] = { kind: 'write', result: r }
    docs = r
    const issue = s.owner_check === true ? awfStepsRuleCheck(r) : null
    return issue ? stop(s.id, issue) : null
  }
  const inner = (s, pre, note) => (s.kind === 'fanout' ? fanout(s, pre, note) : s.kind === 'challenge' ? challenge(s, pre, note) : write(s, pre, note))
  const loop = async (s) => {
    let last = null
    for (let round = 1; round <= s.max_rounds; round++) {
      const pre = `${s.id}.r${round}:`
      const note = last ? `\n上一輪（第 ${round - 1} 輪）結束條件的指令沒過：${JSON.stringify(last)}` : ''
      for (const x of s.steps) {
        const halted = await inner(x, pre, note)
        if (halted) return halted
      }
      phase('Check')
      const label = `${pre}until`
      const secs = Number.isInteger(s.until.timeout) ? s.until.timeout : 300
      const judge = await awfAgent(
        `${ctx}\n\n## 派工單（判定：${s.id} 第 ${round} 輪）\n在${cwd}原樣執行下面這一行指令（不要改寫、不要加參數、不要為了讓它通過而改任何東西），回報它的結束碼：\n${s.until.command}\n` +
        `超過 ${secs} 秒算沒過。payload 用 criteria：只放一條，id 寫 until、criterion 寫「指令的結束碼是 0」、command 照抄、結束碼是 0 才寫 result=pass（其他一律 fail）、evidence 寫「exit <結束碼>」與輸出的最後幾行；commit 寫當下的 HEAD（不是 git 就寫 n/a）、scope_covered 寫這個指令、scope_skipped 寫 none。\n` +
        `這個結果只用來決定要不要再跑一輪：存檔時工具會自己重跑同一個指令核對，回報和實際不一致會被擋下。`,
        { label, phase: 'Check', ...awfStepsAgent(s.until.role, 'dev-verifier', ns), schema: S.criteria })
      if (!judge) { missing.push(label); return lacking(s.id) }
      const crit = Array.isArray(judge.payload.criteria) ? judge.payload.criteria : []
      // 判不出來（沒有條目、有任何一條不是 pass）一律算沒過
      const passed = crit.length >= 1 && crit.every((x) => x && x.result === 'pass')
      last = { round, result: passed ? 'pass' : 'fail', evidence: String((crit[0] || {}).evidence || '').slice(0, 600) }
      pool[`${s.id}.until`] = last
      out[s.id] = { kind: 'loop', rounds: round, passed, until: judge }
      if (passed) return null
    }
    return stop(s.id, `步驟 ${s.id}：跑滿 ${s.max_rounds} 輪，結束條件的指令還沒過（不默默當成完成，交人決定）`, { over_limit: true })
  }
  for (const s of steps) {
    let halted = null
    if (s.kind === 'loop') halted = await loop(s)
    else if (s.kind === 'check') out[s.id] = { kind: 'check', deferred: true }
    else halted = await inner(s, '', '')
    if (halted) return halted
  }
  return done({ halted: false })
}
