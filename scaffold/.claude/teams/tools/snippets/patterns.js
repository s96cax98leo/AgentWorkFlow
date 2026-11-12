// ── 片段 patterns（.claude/teams/tools/snippets/patterns.js）：六個動態工作流程 pattern ──
// @requires: challenge, recipe, kernel
// build.py 內嵌；不要在 workflow 裡手改，改片段後重跑 build.py。只定義函式，需要的值都用參數傳入。
// 出處：Claude Code workflows 文件與〈A harness for every task〉列的六個 pattern。這裡的版本有三條共同規則：
//   1. 派工一律走執行核心的 awfAgent（模型分級、失敗升級、派工數上限、trace）。
//   2. 每個 pattern 都要給上限（max，正整數）；沒給就 throw。要做的事超過上限時不派任何 agent、回 halted（I8：不靜默截斷）。
//   3. 缺件不是「沒有結果」：有 agent 沒交件就回 halted，並在 missing 列出 label，由呼叫端停下交人。
// job（一張派工單）：{ prompt, label, agentType, schema, phase?, tier?, planKey?, valid?, retry? }。
// 每個函式都回 { halted, reason, missing, capped, … }；halted=false 才能用其餘欄位。
function awfPatternMax(name, o) {
  const v = o && o.max
  if (!Number.isInteger(v) || v < 1) throw new Error(`pattern ${name} 要給上限 max（正整數）`)
  return v
}
function awfPatternJob(name, job) {
  if (!job || typeof job !== 'object' || typeof job.prompt !== 'string' || !job.label || !job.agentType) throw new Error(`pattern ${name} 的派工單要有 prompt、label、agentType`)
  const { prompt, ...opts } = job
  return awfAgent(prompt, opts)
}
function awfPatternEnd(missing, extra) {
  const capped = awfCapped()
  const halted = missing.length > 0
  const reason = !halted ? '' : (missing.some((l) => capped.some((c) => c === l || c.startsWith(`${l}#`))) ? `派工數到上限，沒派出：${missing.join('、')}` : `缺件：${missing.join('、')}`)
  return { halted, reason, missing: [...missing], capped: capped.filter((c) => missing.some((l) => c === l || c.startsWith(`${l}#`))), ...extra }
}
function awfPatternOver(name, n, max, what) {
  return { halted: true, reason: `${name}：${what} ${n} 個，超過上限 ${max}（不截斷，交人決定要拆開還是調高上限）`, missing: [], capped: [], over_limit: true }
}

// 1. Fan-out & synthesize：多張派工單平行跑，再（選填）由一個 agent 彙整。
//    o：{ max, synth?: (results) => job }。回傳多 results（依 jobs 的順序）、synthesis。
async function awfFanOut(jobs, o) {
  const max = awfPatternMax('fan-out', o)
  const list = Array.isArray(jobs) ? jobs : []
  if (list.length > max) return awfPatternOver('fan-out', list.length, max, '派工單')
  const results = await parallel(list.map((j) => () => awfPatternJob('fan-out', j)))
  const missing = list.filter((_, i) => !results[i]).map((j) => j.label)
  if (missing.length || typeof (o || {}).synth !== 'function') return awfPatternEnd(missing, { results, synthesis: null })
  const sj = o.synth(results)
  const synthesis = await awfPatternJob('fan-out', sj)
  return awfPatternEnd(synthesis ? [] : [sj.label], { results, synthesis })
}

// 2. Classify & act：每個項目先分類，再走那一類的處理。
//    o：{ max, classify: (item, i) => job, pick: (交件, item) => 類別, routes: { 類別: async (item, 交件) => 任何值 }, fallback: 類別 }。
//    分類結果不在 routes 裡的走 fallback（必填：要明講認不出來時走哪一條，通常是最嚴的那條）。回傳多 results：[{ item, route, result }]。
async function awfClassifyAct(items, o) {
  const max = awfPatternMax('classify', o)
  const c = o || {}
  if (typeof c.classify !== 'function' || typeof c.pick !== 'function' || !c.routes || typeof c.routes !== 'object') throw new Error('pattern classify 要給 classify、pick、routes')
  if (typeof c.fallback !== 'string' || typeof c.routes[c.fallback] !== 'function') throw new Error('pattern classify 要給 fallback，而且 routes 要有它')
  const list = Array.isArray(items) ? items : []
  if (list.length > max) return awfPatternOver('classify', list.length, max, '要分類的項目')
  const jobs = list.map((it, i) => c.classify(it, i))
  const sorted = await parallel(jobs.map((j) => () => awfPatternJob('classify', j)))
  const missing = jobs.filter((_, i) => !sorted[i]).map((j) => j.label)
  if (missing.length) return awfPatternEnd(missing, { results: [] })
  const results = []
  for (let i = 0; i < list.length; i++) {
    const want = c.pick(sorted[i], list[i])
    const route = typeof want === 'string' && typeof c.routes[want] === 'function' && Object.prototype.hasOwnProperty.call(c.routes, want) ? want : c.fallback
    results.push({ item: list[i], route, classified: want, result: await c.routes[route](list[i], sorted[i]) })
  }
  return awfPatternEnd([], { results })
}

// 3. Adversarial verification：多個不同 lens 的反駁者各自試著推翻同一個結論。
//    o：{ max, quorum, refuted: (交件) => boolean }；lenses 是派工單陣列（每個 lens 一張）。
//    沒被推翻的人數達 quorum 才算站得住（upheld）；有反駁者缺件就 halted（缺件不當成「沒推翻」）。回傳多 upheld、votes。
async function awfAdversarial(lenses, o) {
  const max = awfPatternMax('adversarial', o)
  const c = o || {}
  const list = Array.isArray(lenses) ? lenses : []
  if (typeof c.refuted !== 'function') throw new Error('pattern adversarial 要給 refuted')
  if (!Number.isInteger(c.quorum) || c.quorum < 1 || c.quorum > list.length) throw new Error('pattern adversarial 的 quorum 要是 1 到 lens 數之間的整數')
  if (list.length > max) return awfPatternOver('adversarial', list.length, max, '反駁者')
  const got = await parallel(list.map((j) => () => awfPatternJob('adversarial', j)))
  const missing = list.filter((_, i) => !got[i]).map((j) => j.label)
  if (missing.length) return awfPatternEnd(missing, { upheld: false, votes: [] })
  const votes = list.map((j, i) => ({ label: j.label, refuted: c.refuted(got[i]) !== false, result: got[i] }))   // 判不出來（不是明確的 false）一律算推翻
  return awfPatternEnd([], { upheld: votes.filter((v) => !v.refuted).length >= c.quorum, votes })
}

// 4. Generate & filter：平行產生多個候選，純 JS 去重與篩選（不派 agent 當篩子），留下來的交給呼叫端（通常是閘門上讓人選）。
//    o：{ max, generate: (i) => job, count?: 幾個（預設＝max）, key: (交件) => 字串（去重用）, keep?: (交件) => boolean }。
//    回傳多 candidates（去重前，依序）、kept（去重並篩過）、dropped（數量）。
async function awfGenerateFilter(o) {
  const max = awfPatternMax('generate', o)
  const c = o || {}
  if (typeof c.generate !== 'function' || typeof c.key !== 'function') throw new Error('pattern generate 要給 generate、key')
  const n = c.count === undefined ? max : c.count
  if (!Number.isInteger(n) || n < 1) throw new Error('pattern generate 的 count 要是正整數')
  if (n > max) return awfPatternOver('generate', n, max, '候選')
  const jobs = []
  for (let i = 0; i < n; i++) jobs.push(c.generate(i))
  const candidates = await parallel(jobs.map((j) => () => awfPatternJob('generate', j)))
  const missing = jobs.filter((_, i) => !candidates[i]).map((j) => j.label)
  if (missing.length) return awfPatternEnd(missing, { candidates, kept: [], dropped: 0 })
  const seen = {}
  const kept = []
  for (const x of candidates) {
    const k = String(c.key(x))
    if (Object.prototype.hasOwnProperty.call(seen, k)) continue
    seen[k] = true
    if (typeof c.keep === 'function' && !c.keep(x)) continue
    kept.push(x)
  }
  return awfPatternEnd([], { candidates, kept, dropped: candidates.length - kept.length })
}

// 5. Tournament：多份參賽作品兩兩由評審比較，單淘汰到剩一份。
//    o：{ max, judge: (a, b, round, i) => job, pick: (交件) => 'a' | 'b' }；entries 是派工單陣列（每份作品一張）。
//    評審數最多 entries−1。評審缺件或答不出 a／b 就 halted（不替它猜）。回傳多 winner、entries、rounds：[[{ a, b, winner }]]。
async function awfTournament(entries, o) {
  const max = awfPatternMax('tournament', o)
  const c = o || {}
  const list = Array.isArray(entries) ? entries : []
  if (typeof c.judge !== 'function' || typeof c.pick !== 'function') throw new Error('pattern tournament 要給 judge、pick')
  if (list.length < 2) throw new Error('pattern tournament 至少要兩份參賽作品')
  if (list.length > max) return awfPatternOver('tournament', list.length, max, '參賽作品')
  const made = await parallel(list.map((j) => () => awfPatternJob('tournament', j)))
  const missing = list.filter((_, i) => !made[i]).map((j) => j.label)
  if (missing.length) return awfPatternEnd(missing, { winner: null, entries: made, rounds: [] })
  let alive = made.map((r, i) => ({ label: list[i].label, result: r }))
  const rounds = []
  for (let round = 1; alive.length > 1 && round <= list.length; round++) {
    const pairs = []
    for (let i = 0; i + 1 < alive.length; i += 2) pairs.push([alive[i], alive[i + 1]])
    const bye = alive.length % 2 ? [alive[alive.length - 1]] : []
    const jobs = pairs.map((p, i) => c.judge(p[0], p[1], round, i))
    const got = await parallel(jobs.map((j) => () => awfPatternJob('tournament', j)))
    const lost = jobs.filter((_, i) => !got[i]).map((j) => j.label)
    if (lost.length) return awfPatternEnd(lost, { winner: null, entries: made, rounds })
    const picks = got.map((g) => c.pick(g))
    const unclear = jobs.filter((_, i) => picks[i] !== 'a' && picks[i] !== 'b').map((j) => j.label)
    if (unclear.length) return { halted: true, reason: `評審沒有明確選 a 或 b：${unclear.join('、')}`, missing: [], capped: [], winner: null, entries: made, rounds }
    rounds.push(pairs.map((p, i) => ({ a: p[0].label, b: p[1].label, winner: (picks[i] === 'a' ? p[0] : p[1]).label })))
    alive = pairs.map((p, i) => (picks[i] === 'a' ? p[0] : p[1])).concat(bye)
  }
  return awfPatternEnd([], { winner: alive[0], entries: made, rounds })
}

// 6. Loop until done：一輪一輪做到完成為止，最多 max 輪。
//    o：{ max, step: (round, 上一輪的交件) => job, done: (交件, round) => boolean }。
//    到上限還沒完成就 halted（reason 寫明輪數），不默默當成完成。回傳多 done、rounds（跑了幾輪）、last、history。
async function awfLoop(o) {
  const max = awfPatternMax('loop', o)
  const c = o || {}
  if (typeof c.step !== 'function' || typeof c.done !== 'function') throw new Error('pattern loop 要給 step、done')
  const history = []
  let last = null
  for (let round = 1; round <= max; round++) {
    const job = c.step(round, last)
    last = await awfPatternJob('loop', job)
    if (!last) return awfPatternEnd([job.label], { done: false, rounds: round, last: null, history })
    history.push(last)
    if (c.done(last, round) === true) return awfPatternEnd([], { done: true, rounds: round, last, history })
  }
  return { halted: true, reason: `loop：跑滿 ${max} 輪還沒完成（不默默當成完成，交人決定）`, missing: [], capped: [], over_limit: true, done: false, rounds: max, last, history }
}
