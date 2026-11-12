#!/usr/bin/env python3
"""自訂 workflow 腳本的靜態檢查與組裝（awf.py next --script 用）。

標準的 sdlc-*.js 不合用時（例如要照自己的波次排工），主 session 可以為 design、build、deliver 段寫一支自己的腳本。
這支工具先檢查、再把執行核心接上去。檢查不過就不產生腳本。

腳本的寫法：
  - 開頭是 `export const meta = { name, description, … }`（純字面值）。
  - 之後直接寫流程。awf.py 會在 meta 後面接上這些，腳本直接用、不要自己宣告：
      A（這一段的參數）、NS、METHODOLOGY、MP、RECIPE、
      awfAgent（派工）、awfTrace、awfPrefix、awfConstraints、layersOf 等片段函式、
      六個 pattern：awfFanOut、awfClassifyAct、awfAdversarial、awfGenerateFilter、awfTournament、awfLoop。
  - 信封的 schema 用標記：`const FINDINGS = /*@schema:findings*/ null /*@end*/`。
  - 每個結束點都要 `return { stage: '<段>', …, recipe: RECIPE, trace: awfTrace() }`。

檢查的規則（每一條都對應一種繞過執行核心的寫法）：
  1. 不能直接用 agent()、tryAgent()、workflow()：派工一律走 awfAgent 或 pattern。
  2. agentType 要寫成 `NS + '<角色>'` 或完整的字串，而且是登記過的角色（模版角色，或 guard.config.json 的 extension_agents）。
     不能派 dev-merge-steward。
  3. 迴圈要有上限：只准 `for (… of …)`、`for (… in …)` 與有條件、有遞增的 `for (;;)`；`while`、`do` 不准（改用 awfLoop）。
  4. 每個流程層級的 return 都要帶 `trace: awfTrace()`。
  5. 不能重新定義或碰核心的內部（awfState、awfKernel、awfInit、awfKernelInit…），不能用 eval、Function、import、require、globalThis。

這是事前檢查，幫忙早點發現寫錯；它擋不住所有寫法。真正的把關在兩個地方：
執行核心在執行時再擋一次（角色白名單、派工數上限），閘門驗的是產出（save_run.py stage 的條件和標準腳本完全一樣，
而且自訂腳本跑的段不能政策核准，一律交人）。

只用標準庫。`python3 awf_script.py --selftest`。
"""
from __future__ import annotations

import re
import sys

STAGES = ("design", "build", "deliver")
# 自訂腳本可以派的模版角色：check_agents.py 的 CORE 扣掉 dev-merge-steward（合併是信任邊界，只有 sdlc-merge 能派）
ROLES = ("tpm-planner", "tpm-researcher", "tpm-reviewer", "tpm-skeptic", "tpm-writer",
         "dev-spec-author", "dev-acceptance-tester", "dev-implementer", "dev-integrator", "dev-verifier",
         "dev-design-reviewer", "dev-security-reviewer", "dev-release-watcher")
SNIPPETS = ("recipe", "challenge", "layers", "kernel", "patterns")
PREAMBLE_NAMES = ("NS", "A", "METHODOLOGY", "MP", "R", "RECIPE")
FORBIDDEN = {
    "agent": "不能直接用 agent()：派工一律走 awfAgent 或 pattern",
    "tryAgent": "不能直接用 tryAgent()：派工一律走 awfAgent 或 pattern（才有模型分級與派工數上限）",
    "workflow": "不能呼叫別的 workflow",
    "awfState": "不能碰執行核心的內部狀態（awfState）",
    "awfKernel": "不能碰執行核心的內部狀態（awfKernel）",
    "awfInit": "不要自己呼叫 awfInit：awf.py 已經接好了",
    "awfKernelInit": "不要自己呼叫 awfKernelInit：awf.py 已經接好了（上限與角色白名單由它帶）",
    "awfSetModifiers": "不能改 modifiers（只會由 run.json 鎖定的 recipe 帶入）",
    "eval": "不能用 eval",
    "Function": "不能用 Function 建構式",
    "globalThis": "不能用 globalThis",
    "require": "不能用 require",
    "import": "不能用 import",
}
CONTROL = ("if", "for", "while", "switch", "catch", "with")


class ScriptError(Exception):
    pass


def mask(text: str) -> str:
    """把字串與註解的內容換成空白（長度與換行不變），樣板字串裡 ${…} 的程式碼保留。讀不懂（沒收尾）就丟 ScriptError。"""
    out = list(text)
    i, n = 0, len(text)
    stack: list[str] = []     # 'T'＝樣板字串裡；'C'＝樣板字串的 ${ } 裡（值是進來時的大括號深度）
    depth = 0
    cdepth: list[int] = []

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    def template(start: int) -> int:
        """從反引號後面開始掃樣板字串的文字；遇到 ${ 回到程式碼模式。回傳下一個要處理的位置。"""
        nonlocal depth
        j = start
        while j < n:
            c = text[j]
            if c == "\\":
                blank(j, min(j + 2, n))
                j += 2
                continue
            if c == "`":
                stack.pop()
                return j + 1
            if c == "$" and j + 1 < n and text[j + 1] == "{":
                stack.append("C")
                cdepth.append(depth)
                depth += 1
                return j + 2
            if c != "\n":
                out[j] = " "
            j += 1
        raise ScriptError("樣板字串沒有收尾")

    while i < n:
        c = text[i]
        two = text[i:i + 2]
        if two == "//":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif two == "/*":
            j = text.find("*/", i + 2)
            if j < 0:
                raise ScriptError("區塊註解沒有收尾")
            blank(i, j + 2)
            i = j + 2
        elif c in "'\"":
            j = i + 1
            while j < n and text[j] != c:
                if text[j] == "\n":
                    raise ScriptError(f"第 {text.count(chr(10), 0, i) + 1} 行的字串沒有收尾（腳本裡的正規式字面值如果含引號，請改用 new RegExp 以外的寫法或拆開）")
                j += 2 if text[j] == "\\" else 1
            if j >= n:
                raise ScriptError("字串沒有收尾")
            blank(i + 1, j)
            i = j + 1
        elif c == "`":
            stack.append("T")
            i = template(i + 1)
        elif c == "{":
            depth += 1
            i += 1
        elif c == "}":
            depth -= 1
            if stack and stack[-1] == "C" and cdepth and depth == cdepth[-1]:
                stack.pop()
                cdepth.pop()
                i = template(i + 1)
            else:
                i += 1
        else:
            i += 1
    if stack:
        raise ScriptError("樣板字串沒有收尾")
    if depth != 0:
        raise ScriptError("大括號沒有成對")
    return "".join(out)


def _match(m: str, i: int) -> int:
    """m[i] 是開括號，回傳對應的閉括號位置；找不到回 -1。"""
    pair = {"(": ")", "{": "}", "[": "]"}
    want = [pair[m[i]]]
    j = i + 1
    while j < len(m) and want:
        c = m[j]
        if c in pair:
            want.append(pair[c])
        elif c in ")}]":
            if c != want.pop():
                return -1
        j += 1
    return j - 1 if not want else -1


def meta_end(text: str, m: str) -> int:
    """`export const meta = {…}` 結束的位置（閉大括號之後）。開頭不是它就丟 ScriptError。"""
    mm = re.match(r"\s*export\s+const\s+meta\s*=\s*\{", m)
    if not mm:
        raise ScriptError("腳本要以 export const meta = {…} 開頭")
    end = _match(m, mm.end() - 1)
    if end < 0:
        raise ScriptError("meta 的大括號沒有成對")
    return end + 1


def _line(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def _prev_token(m: str, i: int) -> tuple[str, int]:
    """m[:i] 最後一個非空白的 token（一個識別字或一個符號）與它的起點。"""
    j = i - 1
    while j >= 0 and m[j].isspace():
        j -= 1
    if j < 0:
        return "", -1
    if re.match(r"[\w$]", m[j]):
        k = j
        while k >= 0 and re.match(r"[\w$]", m[k]):
            k -= 1
        return m[k + 1:j + 1], k + 1
    if m[j] == ">" and j > 0 and m[j - 1] == "=":
        return "=>", j - 1
    return m[j], j


def _function_spans(m: str) -> list[tuple[int, int]]:
    """函式本體的大括號範圍。`=> {`，或 `) {` 而且那組小括號前面不是 if／for／while／switch／catch。"""
    spans = []
    for i, c in enumerate(m):
        if c != "{":
            continue
        tok, at = _prev_token(m, i)
        is_fn = tok == "=>"
        if tok == ")":
            depth, j = 0, at
            while j >= 0:
                if m[j] == ")":
                    depth += 1
                elif m[j] == "(":
                    depth -= 1
                    if depth == 0:
                        break
                j -= 1
            before, _ = _prev_token(m, j) if j >= 0 else ("", -1)
            is_fn = before not in CONTROL
        if is_fn:
            end = _match(m, i)
            if end > 0:
                spans.append((i, end))
    return spans


def check(text: str, allowed_types: set[str], ns: str, reserved: set[str]) -> list[str]:
    """回傳不合格的理由（空清單＝合格）。"""
    if "/*@snippet" in text or "/*@methodology" in text:
        return ["腳本不能自己放片段或 profile 的標記（awf.py 會接上）；只有 /*@schema:<名>*/ null /*@end*/ 可以用"]
    if "{" * 2 in text:
        return ["腳本不能有雙大括號的佔位符（要用的值從 A、NS、METHODOLOGY 取）"]
    try:
        m = mask(text)
        start = meta_end(text, m)
    except ScriptError as e:
        return [f"讀不懂這支腳本：{e}"]
    errs: list[str] = []
    body = m[start:]
    off = start

    def at(pos: int) -> str:
        return f"第 {_line(text, off + pos)} 行"

    if re.search(r"(?<![\w$.])export\b", body):
        errs.append("除了開頭的 meta，不能再有 export")
    # 1、5：禁用的識別字
    for name, why in FORBIDDEN.items():
        for mm in re.finditer(r"(?<![\w$.])" + re.escape(name) + r"(?![\w$])(?!\s*:)", body):
            errs.append(f"{at(mm.start())}：{why}")
            break
    if re.search(r"\.\s*constructor\b", body):
        errs.append("不能用 .constructor（可以拿來建構函式、繞過檢查）")
    # 5：不能重新定義 awf.py 接上的名字
    names = sorted(reserved | set(PREAMBLE_NAMES))
    for mm in re.finditer(r"(?<![\w$.])(?:function\s*\*?\s*|(?:const|let|var)\s+)([\w$]+)", body):
        if mm.group(1) in names or mm.group(1).startswith("awf"):
            errs.append(f"{at(mm.start())}：不能重新定義 {mm.group(1)}（awf.py 接上的名字；awf 開頭的名字保留給執行核心）")
    for mm in re.finditer(r"(?<![\w$.])([\w$]+)\s*(?:=(?![=>])|\+=|-=|\|\|=|&&=|\?\?=)", body):
        if mm.group(1) in names or mm.group(1).startswith("awf"):
            pre = body[max(0, mm.start() - 8):mm.start()]
            if not re.search(r"(?:const|let|var)\s+$", pre):
                errs.append(f"{at(mm.start())}：不能改寫 {mm.group(1)}")
    # 2：agentType
    seen_type = False
    for mm in re.finditer(r"(?<![\w$.])agentType\b", body):
        seen_type = True
        pos = off + mm.end()
        lit = re.match(r"\s*:\s*(NS\s*\+\s*)?(['\"])([^'\"\n]*)\2", text[pos:])
        nxt = re.match(r"\s*([^\s])", m[pos + lit.end():]) if lit else None
        if not lit or (nxt and nxt.group(1) not in ",})"):
            errs.append(f"{at(mm.start())}：agentType 要寫成 NS + '<角色>' 或完整的字串（不能是變數或組出來的值）")
            continue
        full = (ns if lit.group(1) else "") + lit.group(3)
        if full not in allowed_types:
            errs.append(f"{at(mm.start())}：{full} 不是登記過的角色（模版角色，或 guard.config.json 的 extension_agents；dev-merge-steward 不能由自訂腳本派）")
    if not seen_type:
        errs.append("腳本沒有派任何 agent（找不到 agentType）")
    # 3：迴圈
    for mm in re.finditer(r"(?<![\w$.])(while|do)(?![\w$])", body):
        errs.append(f"{at(mm.start())}：不能用 {mm.group(1)} 迴圈（沒有上限）；改用 pattern 的 awfLoop（要給 max）")
    for mm in re.finditer(r"(?<![\w$.])for(?![\w$])", body):
        rest = body[mm.end():]
        op = re.match(r"\s*\(", rest)
        end = _match(body, mm.end() + op.end() - 1) if op else -1
        if end < 0:
            errs.append(f"{at(mm.start())}：這個 for 讀不懂（for await 不准）")
            continue
        head = body[mm.end() + op.end():end]
        parts, depth, cur = [], 0, ""
        for ch in head:
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
            if ch == ";" and depth == 0:
                parts.append(cur)
                cur = ""
            else:
                cur += ch
        parts.append(cur)
        ok = (len(parts) == 1 and re.match(r"\s*(?:const|let|var)\s+[\w$\[\]{},\s.:]+?\s+(?:of|in)\s+\S", head)) or \
             (len(parts) == 3 and re.search(r"[<>]", parts[1]) and parts[2].strip())
        if not ok:
            errs.append(f"{at(mm.start())}：for 迴圈要有上限（for…of、for…in，或有條件、有遞增的 for）")
    # 4：流程層級的 return 都要帶 trace
    spans = _function_spans(body)
    top = [mm for mm in re.finditer(r"(?<![\w$.])return(?![\w$])", body) if not any(a < mm.start() < b for a, b in spans)]
    if not top:
        errs.append("腳本沒有 return：每個結束點都要回傳 { …, trace: awfTrace() }")
    for mm in top:
        j, depth = mm.end(), 0
        while j < len(body):
            ch = body[j]
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                if depth == 0:
                    break
                depth -= 1
            elif ch in ";\n" and depth == 0 and body[mm.end():j].strip():
                break
            j += 1
        if not re.search(r"(?<![\w$.])trace\s*:\s*awfTrace\s*\(\s*\)", body[mm.end():j]):
            errs.append(f"{at(mm.start())}：這個 return 沒有帶 trace: awfTrace()（閘門靠 trace 核對派了誰、紀錄齊不齊）")
    return errs


def defined_names(snippets: dict) -> set[str]:
    """片段定義的頂層名字（函式與常數）。"""
    out: set[str] = set()
    for src in snippets.values():
        out |= set(re.findall(r"^(?:async\s+)?function\s*\*?\s*([\w$]+)", src, re.M))
        out |= set(re.findall(r"^(?:const|let|var)\s+([\w$]+)", src, re.M))
    return out


def assemble(text: str, preamble: str) -> str:
    """把 preamble 接在 meta 後面。"""
    end = meta_end(text, mask(text))
    return text[:end].rstrip() + "\n\n" + preamble.rstrip("\n") + "\n\n// ── 自訂腳本的內容 ──" + ("" if text[end:].startswith("\n") else "\n") + text[end:]


# ------------------------------------------------------------------ selftest

GOOD = """export const meta = { name: 'my-waves', description: '分兩波打地基：agent() 這個字寫在字串裡沒關係' }
const SUBTASKS = /*@schema:subtasks*/ null /*@end*/
const waves = [['t1'], ['t2', 't3']]
const done = []
for (const [i, wave] of waves.entries()) {
  const r = await awfFanOut(wave.map((id) => ({ prompt: `做 ${id}（不要 while 迴圈）`, label: `Implement:${id}`, agentType: NS + 'dev-implementer', schema: SUBTASKS })), { max: 4 })
  if (r.halted) return { stage: 'build', halted: true, halt_reason: r.reason, missing: r.missing, recipe: RECIPE, trace: awfTrace() }
  done.push(...r.results)
}
for (let i = 0; i < 2; i++) { done.push(i) }
const pick = (xs) => { return xs[0] }
const ext = await awfAgent('看一下', { label: 'ext', agentType: 'acme:a11y', schema: SUBTASKS })
return {
  stage: 'build', halted: false, results: done, first: pick(done), ext,
  recipe: RECIPE,
  trace: awfTrace(),
}
"""


def selftest() -> int:
    ok = True

    def expect(name, cond, detail=""):
        nonlocal ok
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{str(detail)[:500]}"))
        ok = ok and bool(cond)

    ns = "agent-workflow:"
    allowed = {ns + r for r in ROLES} | {"acme:a11y"}
    reserved = {"awfAgent", "tryAgent", "resolveRecipe", "layersOf"}
    run = lambda t: check(t, allowed, ns, reserved)  # noqa: E731
    print("awf_script selftest")
    expect("合格的腳本：沒有任何錯誤", run(GOOD) == [], run(GOOD))
    mk = mask("const a = 'agent(1)' // agent()\nconst b = `x ${ agent(2) } y` /* while */")
    expect("遮罩：字串與註解的內容換成空白，樣板字串裡 ${ } 的程式碼保留", "agent(1)" not in mk and "agent(2)" in mk and "while" not in mk and len(mk) == len("const a = 'agent(1)' // agent()\nconst b = `x ${ agent(2) } y` /* while */"), mk)

    def bad(name, edit, want):
        t = edit(GOOD)
        errs = run(t)
        expect(name, t != GOOD and any(re.search(want, e) for e in errs), errs)

    bad("直接呼叫 agent() 被擋", lambda t: t.replace("const done = []", "const done = [await agent('x', {})]"), r"不能直接用 agent\(\)")
    bad("藏在樣板字串 ${ } 裡的 agent() 也被擋", lambda t: t.replace("做 ${id}", "做 ${await agent('x', {})}"), r"不能直接用 agent\(\)")
    bad("把 agent 另存成別名被擋", lambda t: t.replace("const done = []", "const done = []\nconst go = agent"), r"不能直接用 agent\(\)")
    bad("直接呼叫 tryAgent() 被擋", lambda t: t.replace("await awfAgent('看一下'", "await tryAgent('看一下'"), r"不能直接用 tryAgent")
    bad("未登記的角色被擋", lambda t: t.replace("'acme:a11y'", "'acme:unknown'"), r"acme:unknown 不是登記過的角色")
    bad("dev-merge-steward 不能派", lambda t: t.replace("NS + 'dev-implementer'", "NS + 'dev-merge-steward'"), r"dev-merge-steward 不是登記過的角色")
    bad("agentType 是變數被擋", lambda t: t.replace("agentType: 'acme:a11y'", "agentType: who"), r"agentType 要寫成")
    bad("agentType 是組出來的值被擋", lambda t: t.replace("NS + 'dev-implementer'", "NS + 'dev-' + kind"), r"agentType 要寫成")
    bad("while 迴圈被擋", lambda t: t.replace("for (let i = 0; i < 2; i++) { done.push(i) }", "while (true) { done.push(1) }"), r"不能用 while 迴圈")
    bad("do 迴圈被擋", lambda t: t.replace("for (let i = 0; i < 2; i++) { done.push(i) }", "do { done.push(1) } while (done.length < 3)"), r"不能用 do 迴圈")
    bad("沒有條件的 for 被擋", lambda t: t.replace("for (let i = 0; i < 2; i++)", "for (;;)"), r"for 迴圈要有上限")
    bad("沒有遞增的 for 被擋", lambda t: t.replace("for (let i = 0; i < 2; i++)", "for (let i = 0; i < 2;)"), r"for 迴圈要有上限")
    bad("有一個 return 沒帶 trace 被擋", lambda t: t.replace("missing: r.missing, recipe: RECIPE, trace: awfTrace() }", "missing: r.missing, recipe: RECIPE }"), r"第 7 行：這個 return 沒有帶 trace")
    bad("最後的 return 沒帶 trace 被擋", lambda t: t.replace("  trace: awfTrace(),\n", ""), r"第 13 行：這個 return 沒有帶 trace")
    bad("trace 不是 awfTrace() 被擋", lambda t: t.replace("  trace: awfTrace(),\n", "  trace: [],\n"), r"沒有帶 trace")
    bad("完全沒有 return 被擋", lambda t: t[:t.index("return {\n  stage")].replace("if (r.halted) return { stage: 'build', halted: true, halt_reason: r.reason, missing: r.missing, recipe: RECIPE, trace: awfTrace() }", ""), r"腳本沒有 return")
    bad("重新定義 awfAgent 被擋", lambda t: t.replace("const done = []", "const done = []\nfunction awfAgent() { return {} }"), r"不能重新定義 awfAgent")
    bad("改寫 awfTrace 被擋", lambda t: t.replace("const done = []", "const done = []\nawfTrace = () => []"), r"不能改寫 awfTrace")
    bad("重新宣告 RECIPE 被擋", lambda t: t.replace("const done = []", "const done = []\nconst RECIPE = {}"), r"不能重新定義 RECIPE")
    bad("碰核心內部狀態被擋", lambda t: t.replace("const done = []", "const done = []\nawfState().trace.push({})"), r"awfState")
    bad("自己呼叫 awfKernelInit 被擋", lambda t: t.replace("const done = []", "const done = []\nawfKernelInit({ max_agents: 0 })"), r"不要自己呼叫 awfKernelInit")
    bad("eval 被擋", lambda t: t.replace("const done = []", "const done = [eval('1')]"), r"不能用 eval")
    bad(".constructor 被擋", lambda t: t.replace("const done = []", "const done = [(() => 1).constructor('return 1')]"), r"\.constructor")
    bad("自己放片段標記被擋", lambda t: t.replace("const done = []", "/*@snippet:kernel*/ null /*@end*/\nconst done = []"), r"不能自己放片段")
    bad("開頭不是 meta 被擋", lambda t: "const x = 1\n" + t, r"要以 export const meta")
    bad("字串沒收尾：讀不懂就擋", lambda t: t.replace("'看一下'", "'看一下"), r"讀不懂這支腳本")
    expect("函式裡的 return 不用帶 trace（只管流程層級的）", not any("return" in e for e in run(GOOD)))
    out = assemble(GOOD, "const NS = 'x:'\n")
    expect("組裝：preamble 接在 meta 後面、內容之前，meta 仍在最前面", out.startswith("export const meta = {") and out.index("const NS = 'x:'") < out.index("const SUBTASKS") and out.count("export const meta") == 1, out[:300])
    expect("片段定義的名字", defined_names({"a": "function f1() {}\nasync function f2() {}\nconst C1 = 1\n  function inner() {}"}) == {"f1", "f2", "C1"})
    print("awf_script selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    print(__doc__)
    sys.exit(2)
