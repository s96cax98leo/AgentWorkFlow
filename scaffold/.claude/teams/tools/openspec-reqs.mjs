#!/usr/bin/env node
// OpenSpec 需求清單 adapter：把 change 的 delta 展開成「每個 Scenario 一筆」的需求清單，給 merge_gate.py
// 的 requirements.source=command 與 spec_gate.py 的 test 段使用。不呼叫 LLM。
//
// 用法：node .claude/teams/tools/openspec-reqs.mjs [<change id>]
//   change id 依序取 argv、環境變數 AWF_EPIC；都沒有就 exit 1（merge_gate 會判 hold，不猜是哪個 change）。
// 輸出（stdout）：{"requirements":[{"id":"<cap>/<slug(Requirement)>/<slug(Scenario)>","text":"<Requirement>：<Scenario>\n<rawText>","source":"openspec:<cap>"}]}
//   - 讀 openspec show <id> --json --deltas-only --type change（`openspec change show` 已 deprecated）。
//   - ADDED、MODIFIED 的每個 Scenario 一筆；REMOVED 不產生條目；RENAMED 本身沒有 Scenario，有改內容時會另有 MODIFIED（用新名稱）。
//   - 兩個 Scenario 算出同一個 id（slug 碰撞）時 exit 1，列出碰撞的 id。
// 失敗：exit 1，stdout 為空，stderr 帶 CLI 診斷陣列的 code（例如 show_error）。
// CLI 由環境變數 AWF_OPENSPEC 指定（JSON 陣列或單一路徑，和 spec_gate.py 相同），預設 PATH 上的 openspec。
import { spawnSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

const SLUG_MAX = 64

/**
 * slug：NFKC → 英文小寫 → 非 Unicode 字母／數字的連續字元換成單一 `-` → 去頭尾 `-`
 * → 超過 64 個字元（code point）時截斷並再去尾 `-`。中文保留原樣。
 * 規則以 tests/openspec/vectors/scenario-key-vectors.json 鎖定，其他語言的實作用同一份向量測試。
 */
export function slug(name) {
  const s = String(name)
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '')
  const cps = Array.from(s)
  if (cps.length <= SLUG_MAX) return s
  return cps.slice(0, SLUG_MAX).join('').replace(/-+$/, '')
}

/** scenarioKey＝`<capability 路徑>/<slug(Requirement)>/<slug(Scenario)>`；capability 已是 kebab-case，不再 slug。 */
export function scenarioKey(capability, requirement, scenario) {
  return `${capability}/${slug(requirement)}/${slug(scenario)}`
}

function cli() {
  const raw = (process.env.AWF_OPENSPEC || '').trim()
  if (raw.startsWith('[')) return JSON.parse(raw)
  return [raw || 'openspec']
}

/** delta 清單 → 需求清單；回傳 { requirements, collisions } */
export function expand(show) {
  const requirements = []
  const seen = new Map()
  const collisions = []
  for (const d of show.deltas || []) {
    if (!['ADDED', 'MODIFIED'].includes(d.operation)) continue
    const reqs = Array.isArray(d.requirements) && d.requirements.length ? d.requirements : d.requirement ? [d.requirement] : []
    for (const r of reqs) {
      for (const s of r.scenarios || []) {
        const id = scenarioKey(d.spec, r.name, s.name)
        if (seen.has(id)) {
          collisions.push(`${id}（「${seen.get(id)}」與「${r.name}：${s.name}」）`)
          continue
        }
        seen.set(id, `${r.name}：${s.name}`)
        requirements.push({ id, text: `${r.name}：${s.name}\n${s.rawText || ''}`, source: `openspec:${d.spec}` })
      }
    }
  }
  return { requirements, collisions }
}

function main() {
  const change = process.argv[2] || process.env.AWF_EPIC
  if (!change) {
    console.error('openspec-reqs：沒有 change id（用參數或環境變數 AWF_EPIC 指定）')
    return 1
  }
  const [cmd, ...pre] = cli()
  const r = spawnSync(cmd, [...pre, 'show', change, '--json', '--deltas-only', '--type', 'change'], {
    encoding: 'utf8',
    env: { ...process.env, NO_COLOR: '1', OPENSPEC_NO_ANIMATION: '1', OPENSPEC_TELEMETRY: '0' },
    shell: process.platform === 'win32' && pre.length === 0,
    timeout: 60000,
  })
  if (r.error) {
    console.error(`openspec-reqs：無法執行 ${cmd}：${r.error.message}`)
    return 1
  }
  let show
  try {
    show = JSON.parse(r.stdout)
  } catch {
    console.error(`openspec-reqs：openspec show 的輸出不是 JSON（exit ${r.status}）：${(r.stderr || r.stdout || '').slice(-300)}`)
    return 1
  }
  if (r.status !== 0 || Array.isArray(show.status)) {
    const diag = Array.isArray(show.status) ? show.status : []
    console.error(`openspec-reqs：openspec show ${change} 失敗（exit ${r.status}）：${diag.map((x) => `${x.code}：${x.message}`).join('；') || r.stderr}`)
    return 1
  }
  const { requirements, collisions } = expand(show)
  if (collisions.length) {
    console.error(`openspec-reqs：有 Scenario 算出同一個 id，請改名：${collisions.join('；')}`)
    return 1
  }
  process.stdout.write(JSON.stringify({ requirements }) + '\n')
  return 0
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exitCode = main()
