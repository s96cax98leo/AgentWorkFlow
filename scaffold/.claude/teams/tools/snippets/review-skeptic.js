// @requires: challenge
// ── 片段 review-skeptic（.claude/teams/tools/snippets/review-skeptic.js）：審查 agent 交 findings 後，派 skeptic 逐則反駁 ──
// build.py 內嵌；不要在 workflow 裡手改，改片段後重跑 build.py。只定義函式，需要的值都用參數傳入。
// 用到 challenge 片段的 judgeVerdicts 與 awfDispatch（有執行核心就經過它），引用本片段的 workflow 也要引用 challenge 片段。
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
  return awfDispatch(o.prompt, { label: o.label, phase: o.phase || 'Verify', agentType: o.agentType, schema: o.schema })
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
