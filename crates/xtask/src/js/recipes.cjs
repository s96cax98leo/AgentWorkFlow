// check-recipes 的 node 那一邊：對每個 golden 案例跑 snippets/recipe.js。原本在 0.x 的 tests/check_recipes.py 裡；
// KEYS 由 xtask 換成欄位清單（和 Rust 那邊比對的同一份）。用法：node -e <本檔> <recipe.js>，案例從 stdin 給。
const fs = require('fs')
const src = fs.readFileSync(process.argv[1], 'utf8')
const cases = JSON.parse(fs.readFileSync(0, 'utf8'))
const [resolveRecipe, resolveModel] = new Function(src + '\nreturn [resolveRecipe, resolveModel]')()
const out = cases.map((c) => {
  const r = resolveRecipe(c.profile, c.recipe, c.modifiers, c.size)
  const o = Object.fromEntries(KEYS.map((k) => [k, r[k]]))
  if (c.model_cases) o.models = c.model_cases.map((m) => { const x = resolveModel(c.profile, m.role, m.stage, m.suggested, r.modifiers); delete x.reason; return x })
  return o
})
process.stdout.write(JSON.stringify(out))
