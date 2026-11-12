#!/usr/bin/env python3
"""pattern 庫（snippets/patterns.js）的單元檢查。

用 node 載入 challenge、recipe、kernel、patterns 四個片段，給假的 agent()，六個 pattern 各檢查：
  - 沒給上限（max）會 throw。
  - 正常：結果正確，派工都經過執行核心（trace 每筆有 tier）。
  - 達上限：要做的事超過 max 時不派任何 agent、回 halted；loop 跑滿輪數回 halted。
  - 缺件：有 agent 沒交件回 halted，missing 列出 label。
另檢查：執行核心的派工數上限（max_agents）在 pattern 裡也生效；角色白名單（roles）以外的 agent 會 throw。
只用標準庫；沒有 node 時判失敗（不略過）。run_all.py 會自動發現並執行。
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SNIP = ROOT / "scaffold" / ".claude" / "teams" / "tools" / "snippets"

JS = r"""
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const NAMES = 'awfInit, awfKernelInit, awfAgent, awfTrace, awfCapped, awfFanOut, awfClassifyAct, awfAdversarial, awfGenerateFilter, awfTournament, awfLoop'
const make = new Function('agent', 'log', 'parallel', src + `\nreturn { ${NAMES} }`)
const PROFILE = { models: { mode: 'auto', tiers: { light: 'haiku', standard: 'sonnet', strong: 'opus' }, defaults: { '*': 'standard' }, floors: {} } }
const NS = 'agent-workflow:'
const env = (p) => ({ status: 'done', summary: 's', payload: p })
const errs = []
const need = (c, m) => { if (!c) errs.push(m) }
const par = async (thunks) => Promise.all(thunks.map((t) => t()))
// fake：依 label 回傳；函式就呼叫它；沒寫的 label 回一個空 payload 的交件
function kit(fakes, cfg) {
  const calls = []
  const k = make(async (prompt, o) => {
    calls.push(o.label)
    const f = (fakes || {})[o.label]
    if (f === 'THROW') throw new Error('boom')
    if (f === null) return null
    return typeof f === 'function' ? f(prompt, o) : (f || env({ ok: true }))
  }, () => {}, par)
  k.awfInit({ run_dir: '_runs/r1', stage: 'build', lang: 'zh-TW' })
  k.awfKernelInit({ profile: PROFILE, stage: 'build', modifiers: [], max_agents: 0, ...(cfg || {}) })
  k.calls = calls
  return k
}
const job = (label, role) => ({ prompt: `做 ${label}`, label, agentType: NS + (role || 'tpm-researcher'), schema: {} })
const throws = async (fn) => { try { await fn(); return '' } catch (e) { return e.message } }
const viaKernel = (k) => k.awfTrace().length > 0 && k.awfTrace().every((t) => t.tier === 'standard' && t.model === 'sonnet')

;(async () => {
  let k, r

  // ── 沒給上限一律 throw ──
  k = kit()
  const nomax = {
    'fan-out': () => k.awfFanOut([job('a')], {}),
    classify: () => k.awfClassifyAct(['x'], { classify: () => job('c'), pick: () => 'a', routes: { a: async () => 1 }, fallback: 'a' }),
    adversarial: () => k.awfAdversarial([job('s1', 'tpm-skeptic')], { quorum: 1, refuted: () => false }),
    generate: () => k.awfGenerateFilter({ generate: (i) => job(`g${i}`), key: () => 'k' }),
    tournament: () => k.awfTournament([job('e1'), job('e2')], { judge: () => job('j'), pick: () => 'a' }),
    loop: () => k.awfLoop({ step: () => job('l'), done: () => true }),
  }
  for (const [name, fn] of Object.entries(nomax)) {
    const m = await throws(fn)
    need(/要給上限 max/.test(m), `${name} 沒給上限應該 throw：${m || '沒有 throw'}`)
  }
  need(k.calls.length === 0, `沒給上限不該派出任何 agent：${k.calls}`)
  for (const bad of [0, -1, 1.5, '3', null]) need(/要給上限 max/.test(await throws(() => k.awfLoop({ max: bad, step: () => job('l'), done: () => true }))), `max=${JSON.stringify(bad)} 應該 throw`)

  // ── 1. fan-out ──
  k = kit({ a: env({ n: 1 }), b: env({ n: 2 }), sum: env({ total: 3 }) })
  r = await k.awfFanOut([job('a'), job('b')], { max: 3, synth: (rs) => ({ ...job('sum', 'tpm-writer'), prompt: `彙整 ${rs.map((x) => x.payload.n).join('+')}` }) })
  need(!r.halted && r.results.length === 2 && r.results[1].payload.n === 2 && r.synthesis.payload.total === 3, `fan-out 正常：${JSON.stringify(r)}`)
  need(k.calls.join() === 'a,b,sum' && viaKernel(k), `fan-out 要經過執行核心：${JSON.stringify(k.awfTrace())}`)
  k = kit()
  r = await k.awfFanOut([job('a'), job('b'), job('c')], { max: 2 })
  need(r.halted && r.over_limit && /超過上限 2/.test(r.reason) && k.calls.length === 0, `fan-out 超過上限要 halted 而且不派：${JSON.stringify(r)}`)
  k = kit({ b: null })
  r = await k.awfFanOut([job('a'), job('b')], { max: 2, synth: () => job('sum') })
  need(r.halted && r.missing.join() === 'b' && /缺件：b/.test(r.reason) && !k.calls.includes('sum'), `fan-out 缺件要 halted、不彙整：${JSON.stringify(r)}`)
  k = kit({ a: 'THROW' })
  r = await k.awfFanOut([job('a')], { max: 1 })
  need(r.halted && r.missing.join() === 'a', `fan-out：agent throw 當缺件：${JSON.stringify(r)}`)
  k = kit({ sum: null })
  r = await k.awfFanOut([job('a')], { max: 1, synth: () => job('sum') })
  need(r.halted && r.missing.join() === 'sum', `fan-out：彙整缺件要 halted：${JSON.stringify(r)}`)

  // ── 2. classify & act ──
  const acted = []
  const routes = { small: async (it) => { acted.push(`small:${it}`); return 's' }, strict: async (it) => { acted.push(`strict:${it}`); return 'x' } }
  const copt = { max: 3, classify: (it, i) => job(`cls:${i}`), pick: (res) => res.payload.kind, routes, fallback: 'strict' }
  k = kit({ 'cls:0': env({ kind: 'small' }), 'cls:1': env({ kind: '沒這類' }), 'cls:2': env({ kind: 'constructor' }) })
  r = await k.awfClassifyAct(['甲', '乙', '丙'], copt)
  need(!r.halted && r.results.map((x) => x.route).join() === 'small,strict,strict' && acted.join() === 'small:甲,strict:乙,strict:丙', `classify 正常（認不出來的走 fallback）：${JSON.stringify([r.results.map((x) => x.route), acted])}`)
  need(viaKernel(k), 'classify 要經過執行核心')
  k = kit()
  r = await k.awfClassifyAct(['1', '2', '3', '4'], copt)
  need(r.halted && r.over_limit && k.calls.length === 0, `classify 超過上限：${JSON.stringify(r)}`)
  acted.length = 0
  k = kit({ 'cls:1': null, 'cls:0': env({ kind: 'small' }) })
  r = await k.awfClassifyAct(['甲', '乙'], copt)
  need(r.halted && r.missing.join() === 'cls:1' && acted.length === 0, `classify 缺件要 halted、不做任何處理：${JSON.stringify([r, acted])}`)
  need(/fallback/.test(await throws(() => k.awfClassifyAct(['甲'], { ...copt, fallback: '沒有' }))), 'classify 沒有合法的 fallback 應該 throw')

  // ── 3. adversarial ──
  const lenses = [job('sk:a', 'tpm-skeptic'), job('sk:b', 'tpm-skeptic'), job('sk:c', 'tpm-skeptic')]
  const aopt = { max: 3, quorum: 2, refuted: (res) => res.payload.refuted }
  k = kit({ 'sk:a': env({ refuted: false }), 'sk:b': env({ refuted: true }), 'sk:c': env({ refuted: false }) })
  r = await k.awfAdversarial(lenses, aopt)
  need(!r.halted && r.upheld === true && r.votes.map((v) => v.refuted).join() === 'false,true,false' && viaKernel(k), `adversarial 正常（兩人維持）：${JSON.stringify(r.votes)}`)
  k = kit({ 'sk:a': env({ refuted: false }), 'sk:b': env({ refuted: true }), 'sk:c': env({}) })
  r = await k.awfAdversarial(lenses, aopt)
  need(!r.halted && r.upheld === false, `adversarial：判不出來的算推翻，只剩一人維持 → 站不住：${JSON.stringify(r.votes.map((v) => v.refuted))}`)
  k = kit()
  r = await k.awfAdversarial(lenses, { ...aopt, max: 2 })
  need(r.halted && r.over_limit && k.calls.length === 0, `adversarial 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'sk:b': null, 'sk:a': env({ refuted: false }), 'sk:c': env({ refuted: false }) })
  r = await k.awfAdversarial(lenses, aopt)
  need(r.halted && r.upheld === false && r.missing.join() === 'sk:b', `adversarial 缺件要 halted（不當成沒推翻）：${JSON.stringify(r)}`)
  need(/quorum/.test(await throws(() => k.awfAdversarial(lenses, { ...aopt, quorum: 4 }))), 'adversarial 的 quorum 超過 lens 數應該 throw')

  // ── 4. generate & filter ──
  const gopt = { max: 4, generate: (i) => job(`gen:${i}`, 'dev-spec-author'), key: (res) => res.payload.title, keep: (res) => res.payload.score >= 3 }
  k = kit({ 'gen:0': env({ title: '甲', score: 5 }), 'gen:1': env({ title: '甲', score: 4 }), 'gen:2': env({ title: '乙', score: 1 }), 'gen:3': env({ title: '丙', score: 3 }) })
  r = await k.awfGenerateFilter(gopt)
  need(!r.halted && r.candidates.length === 4 && r.kept.map((x) => x.payload.title).join() === '甲,丙' && r.dropped === 2 && viaKernel(k), `generate 正常（去重、篩選）：${JSON.stringify(r.kept)}`)
  need(k.calls.length === 4, `generate 的篩選不派 agent：${k.calls}`)
  k = kit()
  r = await k.awfGenerateFilter({ ...gopt, count: 5 })
  need(r.halted && r.over_limit && k.calls.length === 0, `generate 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'gen:2': null })
  r = await k.awfGenerateFilter(gopt)
  need(r.halted && r.missing.join() === 'gen:2' && r.kept.length === 0, `generate 缺件要 halted：${JSON.stringify(r)}`)

  // ── 5. tournament ──
  const entries = [job('e:1', 'dev-spec-author'), job('e:2', 'dev-spec-author'), job('e:3', 'dev-spec-author')]
  const topt = { max: 3, judge: (a, b, round, i) => ({ ...job(`judge:${round}:${i}`, 'tpm-reviewer'), prompt: `${a.label} 對 ${b.label}` }), pick: (res) => res.payload.winner }
  k = kit({ 'judge:1:0': env({ winner: 'b' }), 'judge:2:0': env({ winner: 'a' }) })
  r = await k.awfTournament(entries, topt)
  need(!r.halted && r.winner.label === 'e:2' && r.rounds.length === 2 && r.rounds[0][0].winner === 'e:2' && r.rounds[1][0].b === 'e:3' && viaKernel(k), `tournament 正常（三份、兩輪、輪空晉級）：${JSON.stringify(r.rounds)}`)
  need(k.calls.filter((l) => l.startsWith('judge')).length === 2, `tournament 的評審數應該是參賽數減一：${k.calls}`)
  k = kit()
  r = await k.awfTournament(entries, { ...topt, max: 2 })
  need(r.halted && r.over_limit && k.calls.length === 0, `tournament 超過上限：${JSON.stringify(r)}`)
  k = kit({ 'e:2': null })
  r = await k.awfTournament(entries, topt)
  need(r.halted && r.missing.join() === 'e:2' && r.winner === null && !k.calls.some((l) => l.startsWith('judge')), `tournament 參賽作品缺件：${JSON.stringify(r)}`)
  k = kit({ 'judge:1:0': null })
  r = await k.awfTournament(entries, topt)
  need(r.halted && r.missing.join() === 'judge:1:0' && r.winner === null, `tournament 評審缺件：${JSON.stringify(r)}`)
  k = kit({ 'judge:1:0': env({ winner: '都好' }) })
  r = await k.awfTournament(entries, topt)
  need(r.halted && /沒有明確選/.test(r.reason) && r.winner === null, `tournament 評審沒選 a／b 要 halted：${JSON.stringify(r)}`)

  // ── 6. loop until done ──
  const lopt = { max: 3, step: (round, prev) => ({ ...job(`fix:${round}`, 'dev-implementer'), prompt: `第 ${round} 輪${prev ? '，上一輪剩 ' + prev.payload.left : ''}` }), done: (res) => res.payload.left === 0 }
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': env({ left: 0 }) })
  r = await k.awfLoop(lopt)
  need(!r.halted && r.done && r.rounds === 2 && r.history.length === 2 && k.calls.join() === 'fix:1,fix:2' && viaKernel(k), `loop 正常（第二輪完成就停）：${JSON.stringify(r)}`)
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': env({ left: 2 }), 'fix:3': env({ left: 1 }) })
  r = await k.awfLoop(lopt)
  need(r.halted && r.over_limit && !r.done && r.rounds === 3 && /跑滿 3 輪/.test(r.reason) && k.calls.length === 3, `loop 跑滿上限要 halted、不當成完成：${JSON.stringify(r)}`)
  k = kit({ 'fix:1': env({ left: 2 }), 'fix:2': null })
  r = await k.awfLoop(lopt)
  need(r.halted && !r.done && r.missing.join() === 'fix:2' && k.calls.length === 3, `loop 缺件要 halted（standard 失敗會升一級重派一次）：${JSON.stringify([r.missing, k.calls])}`)

  // ── 執行核心的派工數上限在 pattern 裡也生效，原因寫明是上限 ──
  k = kit({}, { max_agents: 2 })
  r = await k.awfFanOut([job('a'), job('b'), job('c')], { max: 5 })
  need(r.halted && r.missing.join() === 'c' && /派工數到上限/.test(r.reason) && r.capped.join() === 'c' && k.calls.length === 2, `核心的上限：${JSON.stringify(r)}`)
  need(k.awfTrace().some((t) => t.label === 'c' && t.capped === true), '到上限沒派的要在 trace 記 capped')

  // ── 角色白名單：清單以外的 agent 直接 throw，不派 ──
  k = kit({}, { roles: [NS + 'tpm-researcher'] })
  r = await k.awfFanOut([job('a')], { max: 1 })
  need(!r.halted, '白名單裡的角色要能派')
  let m = await throws(() => k.awfFanOut([job('b', 'dev-merge-steward')], { max: 1 }))
  need(/只能派登記過的角色/.test(m) && !k.calls.includes('b'), `白名單以外的角色應該 throw：${m || '沒有 throw'}`)
  m = await throws(() => k.awfAgent('p', { label: 'x' }))
  need(/只能派登記過的角色/.test(m), `沒給 agentType 應該 throw：${m || '沒有 throw'}`)
  need(/prompt、label、agentType/.test(await throws(() => k.awfFanOut([{ prompt: 'p', label: 'x' }], { max: 1 }))), '派工單缺 agentType 應該 throw')

  if (errs.length) { console.log(errs.join('\n')); process.exit(1) }
  console.log('ok')
})().catch((e) => { console.log(`例外：${e.stack || e}`); process.exit(1) })
"""


# 自訂腳本端到端：awf.py 組裝出來的腳本真的能跑（片段之間沒有撞名）、派工經過核心、上限在執行時生效
RUN_JS = r"""
const fs = require('fs')
const body = fs.readFileSync(process.argv[1], 'utf8').replace(/^export const meta/, 'const meta')
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
const calls = []
const fn = new AsyncFunction('agent', 'log', 'parallel', 'args', body)
fn(async (p, o) => { calls.push(o.label); return { status: 'done', summary: 's', artifacts: [], evidence: [], confidence: 0.9, open_questions: [], derived_work: [], payload: { subtasks: [] } } },
   () => {}, async (ts) => Promise.all(ts.map((t) => t())), undefined)
  .then((out) => console.log(JSON.stringify({ calls, out })))
  .catch((e) => { console.log(JSON.stringify({ error: String(e.stack || e) })); process.exit(1) })
"""


def custom_end_to_end(node: str) -> list[str]:
    import json
    import tempfile
    tools = SNIP.parent
    sys.path.insert(0, str(tools))
    import awf  # noqa: E402
    import awf_script  # noqa: E402
    errs: list[str] = []
    default = json.loads((tools.parent / "methodologies" / "default.json").read_text(encoding="utf-8"))
    for cap, want_halt in ((None, False), (2, True)):
        with tempfile.TemporaryDirectory(prefix="awf-cs.") as tmp:
            proj = Path(tmp)
            teams = proj / ".claude" / "teams"
            teams.mkdir(parents=True)
            (teams / "workflow.config").write_text('AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE="繁體中文"\n', encoding="utf-8")
            prof = dict(default)
            if cap:
                prof["limits"] = {**(prof.get("limits") or {}), "max_agents": cap}
            (teams / "methodology.json").write_text(json.dumps(prof, ensure_ascii=False), encoding="utf-8")
            src = proj / "waves.js"
            src.write_text(awf_script.GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'"), encoding="utf-8")
            text, _ = awf.render_custom(src, "build", proj, {"run_dir": "_runs/r1", "epic": "demo", "recipe": "feature", "modifiers": []})
            out_js = proj / "custom-build.js"
            out_js.write_text(text, encoding="utf-8")
            r = subprocess.run([node, "-e", RUN_JS, str(out_js)], capture_output=True, text=True, encoding="utf-8")
            try:
                res = json.loads(r.stdout.strip().splitlines()[-1])
            except (ValueError, IndexError):
                errs.append(f"自訂腳本（上限 {cap}）跑不起來：{(r.stdout + r.stderr)[:600]}")
                continue
            if res.get("error"):
                errs.append(f"自訂腳本（上限 {cap}）執行錯誤：{res['error'][:600]}")
                continue
            out, calls = res["out"] or {}, res["calls"]
            trace = out.get("trace") or []
            if bool(out.get("halted")) != want_halt:
                errs.append(f"自訂腳本（上限 {cap}）halted 應該是 {want_halt}：{json.dumps(out, ensure_ascii=False)[:400]}")
            if (out.get("recipe") or {}).get("name") != "feature" or out.get("stage") != "build":
                errs.append(f"自訂腳本的回傳要帶 stage 與 recipe：{json.dumps({k: out.get(k) for k in ('stage', 'recipe')}, ensure_ascii=False)[:300]}")
            if not want_halt:
                if calls != ["Implement:t1", "Implement:t2", "Implement:t3", "ext"] or len(trace) != 4 or any(not x.get("tier") or not x.get("record") for x in trace):
                    errs.append(f"自訂腳本的派工要經過核心（trace 有 tier 與紀錄檔）：{calls}、{json.dumps(trace, ensure_ascii=False)[:400]}")
            else:
                if len(calls) != 2 or not any(x.get("capped") for x in trace) or "派工數到上限" not in str(out.get("halt_reason")):
                    errs.append(f"上限 2：只該派 2 個、其餘記 capped 並停下：{calls}、{out.get('halt_reason')}")
    return errs


def main() -> int:
    node = shutil.which("node")
    if not node:
        print("check_patterns：找不到 node（workflow 片段要用 node 檢查）")
        return 1
    e2e = custom_end_to_end(node)
    if e2e:
        print("check_patterns：自訂腳本端到端沒通過")
        print("\n".join(e2e))
        return 1
    files = [str(SNIP / f"{n}.js") for n in ("challenge", "recipe", "kernel", "patterns")]
    r = subprocess.run([node, "-e", JS, *files], capture_output=True, text=True, encoding="utf-8")
    out = (r.stdout + r.stderr).strip()
    if r.returncode != 0:
        print("check_patterns：沒通過")
        print(out)
        return 1
    print("check_patterns：六個 pattern 的上限、正常、達上限、缺件，核心上限與角色白名單，以及自訂腳本端到端都正確")
    return 0


if __name__ == "__main__":
    sys.exit(main())
