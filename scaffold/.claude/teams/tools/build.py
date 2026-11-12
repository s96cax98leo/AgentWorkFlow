"""把 schemas/ 內嵌進 workflow 腳本（Workflow 腳本不能 import 檔案）。

處理哪些檔：模版 repo 裡是 plugin/workflow-templates/*.js（範本）；已 init 的專案裡是 .claude/workflows/*.js
（使用者留在專案的覆寫檔，沒有就什麼都不做。0.5 起 workflow 由 awf.py next 依 plugin 的範本逐段產生，專案不必放副本）。

標記寫法（腳本內）：
    const SUBTASKS = /*@schema:subtasks*/ null /*@end*/
build 後變成 `/*@schema:subtasks*/ {...完整 JSON...} /*@end*/`。名稱是 payload 名；
`envelope` 代表 payload 不限形狀。

開發規範 profile 也用同樣方式內嵌：
    const METHODOLOGY = /*@methodology*/ null /*@end*/
build 後換成專案 .claude/teams/methodology.json 的內容（init.py 依選定的 profile 與使用者回答的慣例產生）。
沒有 methodology.json（例如模版本身的 scaffold）時保留 null，workflow 改用中性指引（依目標 repo 既有慣例）。
改了 methodology.json 就要重跑 build.py；--check 會抓到不一致。

多支 workflow 共用的程式放在 tools/snippets/<名>.js，用片段標記內嵌：
    /*@snippet:challenge*/
    /*@end*/
build 後中間換成片段檔的內容（標記保留，可重跑）。片段只定義函式（不能有頂層 await），
需要的值一律用參數傳進去，不讀 workflow 的區域變數。片段開頭可寫 `// @requires: <名>[, <名>]`，
引用它的 workflow 必須也引用這些片段。片段檔不存在、內容有 `/*@` 或佔位符（雙大括號）、頂層有 await 都會 exit 1。
改了片段就要重跑 build.py；--check 會抓到片段和 workflow 內嵌的內容不一致（漂移）。
profile 的 extensions（使用者自己的 agent）必須登記在 guard.config.json 的 extension_agents，否則不內嵌（exit 1）。

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

_HERE = Path(__file__).resolve()
_REPO_TEMPLATES = _HERE.parents[4] / "plugin" / "workflow-templates" if len(_HERE.parents) > 4 else None
# 模版 repo（scaffold/.claude/teams/tools/）→ plugin 的範本；專案（.claude/teams/tools/）→ 專案自己的 .claude/workflows/
WF_DIR = _REPO_TEMPLATES if _REPO_TEMPLATES is not None and _REPO_TEMPLATES.is_dir() and _HERE.parents[3].name == "scaffold" else _HERE.parents[2] / "workflows"
METHODOLOGY = schemalib.TEAMS_DIR / "methodology.json"
MARK = re.compile(r"/\*@schema:([a-z-]+)\*/.*?/\*@end\*/", re.S)
METH_MARK = re.compile(r"/\*@methodology\*/.*?/\*@end\*/", re.S)
SNIPPET_DIR = Path(__file__).resolve().parent / "snippets"
SNIP_MARK = re.compile(r"/\*@snippet:([a-z0-9-]+)\*/.*?/\*@end\*/", re.S)
REQUIRES = re.compile(r"^//\s*@requires:\s*(.+)$", re.M)


class SnippetError(Exception):
    pass


def load_snippet(name: str, cache: dict) -> str:
    """讀 snippets/<name>.js；不存在或內容不合規則就丟 SnippetError（main 轉成 exit 1）。"""
    if name in cache:
        return cache[name]
    p = SNIPPET_DIR / f"{name}.js"
    if not p.is_file():
        raise SnippetError(f"片段檔不存在：{p}")
    text = p.read_text(encoding="utf-8").replace("\r\n", "\n").strip("\n")
    if "/*@" in text:
        raise SnippetError(f"片段 {name} 不能含 /*@ 標記（會和內嵌標記混淆）")
    if "{" * 2 in text:
        raise SnippetError(f"片段 {name} 不能含佔位符（雙大括號）：片段不經 init 換值，workflow 用到的值一律用參數傳進去")
    top_await = [ln for ln in text.split("\n") if ln[:1] not in ("", " ", "\t", "/", "*", "}", ")", "]") and re.search(r"\bawait\b", ln)]
    if top_await:
        raise SnippetError(f"片段 {name} 有頂層 await（片段只定義函式，由 workflow 呼叫）：{top_await[0].strip()}")
    cache[name] = text
    return text


def snippet_requires(text: str) -> list[str]:
    return [n.strip() for m in REQUIRES.finditer(text) for n in m.group(1).split(",") if n.strip()]


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


def unregistered_extensions(methodology) -> list[str]:
    """profile 的 extensions 裡、沒有登記在 guard.config.json extension_agents 的 agent。沒有守門設定檔時不檢查。
    沒登記的擴充 agent 不套任何角色規則（和主 session 一樣能 push），所以一律擋下，不內嵌。"""
    exts = [e for e in (methodology or {}).get("extensions") or [] if isinstance(e, dict)]
    guard = schemalib.TEAMS_DIR / "guard.config.json"
    if not exts or not guard.is_file():
        return []
    try:
        registered = json.loads(guard.read_text(encoding="utf-8")).get("extension_agents") or {}
    except (json.JSONDecodeError, AttributeError):
        registered = {}
    return [f"{e.get('name')}（{e.get('agent')}）" for e in exts if e.get("agent") not in registered]


def render_snippets(text: str, cache: dict) -> str:
    used = SNIP_MARK.findall(text)
    for name in used:
        for req in snippet_requires(load_snippet(name, cache)):
            if req not in used:
                raise SnippetError(f"片段 {name} 需要片段 {req}，workflow 沒有引用 /*@snippet:{req}*/")
    return SNIP_MARK.sub(lambda m: f"/*@snippet:{m.group(1)}*/\n{load_snippet(m.group(1), cache)}\n/*@end*/", text)


def render(text: str, methodology=None, cache: dict | None = None) -> str:
    text = render_snippets(text, {} if cache is None else cache)

    def sub(m: re.Match) -> str:
        name = m.group(1)
        schema = schemalib.compose(None if name == "envelope" else name)
        return f"/*@schema:{name}*/ {json.dumps(schema, ensure_ascii=False, separators=(',', ':'))} /*@end*/"

    meth = json.dumps(methodology, ensure_ascii=False, separators=(",", ":"))
    return METH_MARK.sub(lambda _: f"/*@methodology*/ {meth} /*@end*/", MARK.sub(sub, text))


def main() -> int:
    check = "--check" in sys.argv
    stale = []
    files = sorted(WF_DIR.glob("*.js")) if WF_DIR.is_dir() else []
    methodology = load_methodology()
    missing = unregistered_extensions(methodology)
    if missing:
        print("methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents：" + "、".join(missing)
              + "（登記要套用哪個模版角色的規則，例如 \"tpm-reviewer\"；可以用 add-agent skill（/agent-workflow:add-agent）設定）")
        return 1
    cache: dict = {}
    for f in files:
        old = f.read_text(encoding="utf-8")
        try:
            new = render(old, methodology, cache)
        except SnippetError as e:
            print(f"{f.name}：{e}")
            return 1
        if new != old:
            stale.append(f.name)
            if not check:
                # newline=""：Windows 上不把 \n 轉成 \r\n，否則 check_workflows.mjs 的 meta 解析會失敗
                with open(f, "w", encoding="utf-8", newline="") as fh:
                    fh.write(new)
    if check and stale:
        print("workflow 內嵌的 schema、開發規範 profile 或片段跟 schemas/、methodology.json、tools/snippets/ 不一致：" + "、".join(stale) + "（請跑 build.py）")
        return 1
    prof = f"（開發規範 profile：{methodology.get('name')}）" if methodology else "（沒有 methodology.json，workflow 用中性指引）"
    print(("檢查" if check else "已更新") + f"：{len(files)} 支 workflow{prof}" + ("，全部一致" if check else f"，改了 {len(stale)} 支"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
