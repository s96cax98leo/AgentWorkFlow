#!/usr/bin/env python3
"""執行核心（snippets/kernel.js）的單元檢查：workflow 的情境走不到的行為直接對片段測。

用 node 載入 challenge、recipe、kernel 三個片段，給假的 agent()，檢查：
  1. label 重複時第二次起自動加序號（trace、紀錄檔、失敗升級都靠 label 對，不能撞）。
  2. label 重複又碰到失敗升級時，標 superseded 的是自己那一筆，不是另一個同名的派工。
  3. 交件的 status 與 summary 記進 trace 的 digest（save_run 靠它把信封對回派工）。
  4. 派工數到上限就不再派，label 記進 capped。
  5. 限制文件（challenge 片段的 awfConstraints）：依段與路徑挑、build 只給對得上的子任務、共同前綴帶出規則。
只用標準庫；沒有 node 時判失敗（不略過）。run_all.py 會自動發現並執行。
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SNIP = ROOT / "scaffold" / ".claude" / "teams" / "tools" / "snippets"

JS = r"""
const fs = require('fs')
const src = process.argv.slice(1).map((f) => fs.readFileSync(f, 'utf8')).join('\n')
const make = new Function('agent', 'log', src + '\nreturn { awfInit, awfKernelInit, awfAgent, awfTrace, awfCapped, awfConstraints, awfConstraintLines, awfPrefix, awfPathOverlap }')
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
  need(over === null && JSON.stringify(k.awfCapped()) === JSON.stringify(['two']) && k.awfTrace().length === 1, `上限沒擋住：${JSON.stringify(k.awfCapped())}`)

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
  process.stdout.write(JSON.stringify(errs))
})().catch((e) => { process.stdout.write(JSON.stringify([`例外：${e && e.stack || e}`])) })
"""


def main() -> int:
    node = shutil.which("node")
    if not node:
        print("check_kernel：找不到 node（fail-closed）")
        return 1
    files = [str(SNIP / f"{n}.js") for n in ("challenge", "recipe", "kernel")]
    q = subprocess.run([node, "-e", JS, *files], capture_output=True, text=True, encoding="utf-8")
    if q.returncode != 0:
        print(f"check_kernel：node 執行失敗：{q.stderr[-800:]}")
        return 1
    try:
        errs = json.loads(q.stdout)
    except ValueError:
        print(f"check_kernel：輸出讀不懂：{q.stdout[-400:]}")
        return 1
    for e in errs:
        print(f"  [失敗] {e}")
    print("check_kernel：" + ("通過（label 去重、升級標對筆、digest、派工數上限、限制文件）" if not errs else f"{len(errs)} 項失敗"))
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main())
