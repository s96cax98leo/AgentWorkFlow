"""把 schemas/ 內嵌進 .claude/workflows/*.js（Workflow 腳本不能 import 檔案）。

標記寫法（腳本內）：
    const SUBTASKS = /*@schema:subtasks*/ null /*@end*/
build 後變成 `/*@schema:subtasks*/ {...完整 JSON...} /*@end*/`。名稱是 payload 名；
`envelope` 代表 payload 不限形狀。

開發規範 profile 也用同樣方式內嵌：
    const METHODOLOGY = /*@methodology*/ null /*@end*/
build 後換成專案 .claude/teams/methodology.json 的內容（init.py 依選定的 profile 與使用者回答的慣例產生）。
沒有 methodology.json（例如模版本身的 scaffold）時保留 null，workflow 改用中性指引（依目標 repo 既有慣例）。
改了 methodology.json 就要重跑 build.py；--check 會抓到不一致。

用法：
    python3 .claude/teams/tools/build.py          # 寫回
    python3 .claude/teams/tools/build.py --check  # 不一致就 exit 1（驗證用）
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402

WF_DIR = Path(__file__).resolve().parents[2] / "workflows"
METHODOLOGY = schemalib.TEAMS_DIR / "methodology.json"
MARK = re.compile(r"/\*@schema:([a-z-]+)\*/.*?/\*@end\*/", re.S)
METH_MARK = re.compile(r"/\*@methodology\*/.*?/\*@end\*/", re.S)


def load_methodology():
    """專案的開發規範 profile；沒有檔案回 None。格式不合 schema 就停下（不內嵌壞掉的 profile）。"""
    if not METHODOLOGY.is_file():
        return None
    try:
        data = json.loads(METHODOLOGY.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise SystemExit(f"{METHODOLOGY} 不是合法 JSON：{e}")
    errs = schemalib.check_doc(data, "methodology")
    if errs:
        raise SystemExit(f"{METHODOLOGY} 不符合 schemas/methodology.schema.json：" + "；".join(errs[:5]))
    return data


def render(text: str, methodology=None) -> str:
    def sub(m: re.Match) -> str:
        name = m.group(1)
        schema = schemalib.compose(None if name == "envelope" else name)
        return f"/*@schema:{name}*/ {json.dumps(schema, ensure_ascii=False, separators=(',', ':'))} /*@end*/"

    meth = json.dumps(methodology, ensure_ascii=False, separators=(",", ":"))
    return METH_MARK.sub(lambda _: f"/*@methodology*/ {meth} /*@end*/", MARK.sub(sub, text))


def main() -> int:
    check = "--check" in sys.argv
    stale = []
    files = sorted(WF_DIR.glob("*.js"))
    methodology = load_methodology()
    for f in files:
        old = f.read_text(encoding="utf-8")
        new = render(old, methodology)
        if new != old:
            stale.append(f.name)
            if not check:
                # newline=""：Windows 上不把 \n 轉成 \r\n，否則 check_workflows.mjs 的 meta 解析會失敗
                with open(f, "w", encoding="utf-8", newline="") as fh:
                    fh.write(new)
    if check and stale:
        print("workflow 內嵌的 schema 或開發規範 profile 跟 schemas/、methodology.json 不一致：" + "、".join(stale) + "（請跑 build.py）")
        return 1
    prof = f"（開發規範 profile：{methodology.get('name')}）" if methodology else "（沒有 methodology.json，workflow 用中性指引）"
    print(("檢查" if check else "已更新") + f"：{len(files)} 支 workflow{prof}" + ("，全部一致" if check else f"，改了 {len(stale)} 支"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
