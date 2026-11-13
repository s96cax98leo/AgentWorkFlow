// check-patterns 的端到端：執行一支組裝好的自訂腳本，印出 {calls, out} 或 {error}。原本在 0.x 的 tests/check_patterns.py 裡。
// 用法：node -e <本檔> <組裝好的腳本>。
const fs = require('fs')
const body = fs.readFileSync(process.argv[1], 'utf8').replace(/^export const meta/, 'const meta')
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
const calls = []
const fn = new AsyncFunction('agent', 'log', 'parallel', 'args', body)
fn(async (p, o) => { calls.push(o.label); return { status: 'done', summary: 's', artifacts: [], evidence: [], confidence: 0.9, open_questions: [], derived_work: [], payload: { subtasks: [] } } },
   () => {}, async (ts) => Promise.all(ts.map((t) => t())), undefined)
  .then((out) => console.log(JSON.stringify({ calls, out })))
  .catch((e) => { console.log(JSON.stringify({ error: String(e.stack || e) })); process.exit(1) })
