// 四支事件範本共用的情境（底線開頭的檔不會被當成某支 workflow 的情境檔）。
import { kgScenarios } from './_helpers.mjs'

// 限制文件與知識圖進事件的共同前綴（1.1）：profile 的 constraints 把 stages 寫成這個事件的段名時，每張派工單都列出那份文件；
// 套用到別段的不出現；沒有 constraints、沒有 args.kg 時前綴逐字不變（由預設情境前後對照）。
// stage＝這支事件的段名（brief、pr-review、post-merge，或 event.js 的事件類型）；x＝{ args, fakes, profile }（要走到每一步時給假回傳）
export const eventPrefixScenarios = (stage, x = {}) => {
  const base = typeof x.profile === 'function' ? null : (x.profile || {})
  const withCons = (cons) => (base ? { ...base, constraints: cons } : (p) => ({ ...x.profile(p), constraints: cons }))
  return [
    {
      name: `限制文件：stages 寫 ${stage} → 每張派工單的共同前綴列出這份文件與對照表格式；只套用到別段的不出現`,
      args: x.args || {}, fakes: x.fakes || {},
      profile: withCons([
        { id: 'tone', file: 'docs/對外用語.md', note: '對外文件的用語', stages: [stage], trace: 'table' },
        { id: 'arch', file: 'ARCHITECTURE.md', stages: ['design'] },
      ]),
      expect: (r) => {
        const ps = Object.entries(r.prompts)
        return [
          ...(ps.length ? [] : ['沒有派出任何 agent']),
          ...ps.filter(([, p]) => !/\[tone\] docs\/對外用語\.md：對外文件的用語（要對照）/.test(p) || !/符合／偏離／不適用/.test(p)).map(([l]) => `${l} 的派工單沒有列出限制文件`),
          ...ps.filter(([, p]) => /\[arch\]/.test(p)).map(([l]) => `${l} 的派工單不該有只套用到 design 的限制`),
        ]
      },
    },
    {
      name: `限制文件：constraints 格式不對（stages 不是陣列）→ 略過，前綴沒有限制文件那一條`,
      args: x.args || {}, fakes: x.fakes || {},
      profile: withCons([{ id: 'tone', file: 'docs/對外用語.md', stages: stage }]),
      expect: (r) => Object.entries(r.prompts).filter(([, p]) => /這一段要遵守的限制文件/.test(p)).map(([l]) => `${l} 的派工單不該有限制文件`),
    },
    ...kgScenarios(x),
  ]
}
