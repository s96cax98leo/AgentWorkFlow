#!/usr/bin/env python3
"""錄三支小工具的對跑基準：build.py（內嵌）、scrub_check.py（去客戶化掃描）、check_docs.py（文件核對）。

用法：python3 tests/parity/record_build.py > tests/parity/build.expected.json
內容是三份劇本（build、scrub、docs），另一個實作照做一遍、逐字比對：
  - write／write_hex／remove／symlink：準備暫存目錄裡的檔案。
  - build：在迷你的模版 repo（mode=template）或迷你專案（mode=project）跑 build.py；files 是跑完之後 workflow 目錄裡每支 .js 的內容。
  - build_real：對真的 plugin/workflow-templates/ 跑 --check（唯讀）。
  - scrub：對暫存樹呼叫 scrub_check.main（ROOT 與兩份清單的路徑指到暫存樹；清單都是這支腳本自己寫的，不讀本機清單）。
  - docs：在迷你 repo 跑 check_docs.py；docs_real：對目前的 repo 跑一次。
err_any 是 true 的步驟只比 exit code 與 stdout：Python 版在那裡直接當掉（traceback），另一個實作印自己的說明。
unordered 是 true 的步驟 stdout 逐行排序後再比：Python 版用 set 走訪，同一輪有兩則以上錯誤時順序不固定。
暫存路徑換成 @TMP@。同一個實作連錄兩次要逐字相同。這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
TMP = Path(tempfile.mkdtemp(prefix="awf-build-")).resolve()
ENV = {**os.environ, "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONIOENCODING": "utf-8"}
ENV.pop("CLAUDE_PROJECT_DIR", None)
BRACES = "{" * 2  # 佔位符的開頭；不直接寫在檔案裡
steps: list = []


def norm(s: str) -> str:
    return s.replace(str(TMP), "@TMP@")


def real(s: str) -> str:
    return s.replace("@TMP@", str(TMP))


def write(path: str, text: str) -> None:
    p = Path(real(path))
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(text.encode("utf-8"))
    steps.append({"op": "write", "path": path, "text": text})


def write_hex(path: str, data: bytes) -> None:
    p = Path(real(path))
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)
    steps.append({"op": "write_hex", "path": path, "hex": data.hex()})


def remove(path: str) -> None:
    p = Path(real(path))
    if p.is_dir() and not p.is_symlink():
        shutil.rmtree(p)
    else:
        p.unlink()
    steps.append({"op": "remove", "path": path})


def symlink(path: str, target: str) -> None:
    p = Path(real(path))
    p.parent.mkdir(parents=True, exist_ok=True)
    os.symlink(target, p)
    steps.append({"op": "symlink", "path": path, "target": target})


def run(cmd: list) -> tuple:
    r = subprocess.run(cmd, cwd=TMP, env=ENV, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.returncode, norm(r.stdout), norm(r.stderr)


def result(step: dict, rc: int, out: str, err: str, **more) -> None:
    step.update({"rc": rc, "out": out, **more})
    # Python 版當掉時 stderr 是 traceback，另一個實作不可能相同
    if "Traceback (most recent call last)" in err:
        step["err_any"] = True
    else:
        step["err"] = err
    steps.append(step)


# ------------------------------------------------------------------ build
def install_build(tools: str) -> None:
    d = Path(real(tools))
    d.mkdir(parents=True, exist_ok=True)
    for name in ("build.py", "schemalib.py"):
        shutil.copy(TOOLS / name, d / name)


def build(mode: str, root: str, *argv: str) -> None:
    tools = ("scaffold/" if mode == "template" else "") + ".claude/teams/tools"
    wf = Path(real(root)) / ("plugin/workflow-templates" if mode == "template" else ".claude/workflows")
    rc, out, err = run([sys.executable, f"{real(root)}/{tools}/build.py", *argv])
    files = {p.name: p.read_bytes().decode("utf-8", errors="replace") for p in sorted(wf.glob("*.js")) if p.is_file()} if wf.is_dir() else {}
    result({"op": "build", "mode": mode, "root": root, "argv": list(argv)}, rc, out, err, files=files)


def dumps(v) -> str:
    return json.dumps(v, ensure_ascii=False, indent=2) + "\n"


ENVELOPE = {
    "$id": "envelope", "title": "交件信封", "description": "示範用", "type": "object", "required": ["status", "payload"],
    "properties": {
        "status": {"enum": ["ok", "blocked"], "description": "狀態"},
        "title": {"type": "string", "title": "欄位名是 title，不能刪", "maxLength": 80},
        "confidence": {"type": "number", "minimum": 0.0, "maximum": 1.0},
        "payload": {"type": "object"},
    },
}
SUBTASKS = {
    "$id": "subtasks", "title": "子任務", "type": "object", "required": ["items"],
    "properties": {"items": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
        "description": {"type": "string", "description": "欄位名是 description"}, "pattern": {"type": "string", "pattern": "^[a-z\\\\]+\"$"},
        "weight": {"type": "number", "minimum": 1.5, "maximum": 1e22}}}}},
}
NOTES = {"type": "object", "description": "筆記", "properties": {"text": {"type": ["string", "null"]}}, "additionalProperties": False}
METH_SCHEMA = {
    "$id": "methodology", "type": "object", "required": ["name"],
    "properties": {"name": {"type": "string"}, "note": {"type": "string"}, "extensions": {"type": "array", "items": {"type": "object"}}},
}

W1 = """export const meta = { name: 'w1' }
const ENV = /*@schema:envelope*/ null /*@end*/
const SUB = /*@schema:subtasks*/ null /*@end*/, NOTES = /*@schema:notes*/ null /*@end*/
const MP = /*@methodology*/ null /*@end*/
const UPPER = /*@schema:Upper*/ null /*@end*/
/*@snippet:alpha*/
/*@end*/
/*@snippet:beta*/
舊的內容
跨了兩行
/*@end*/
const MP2 = /*@methodology*/
  null
/*@end*/
return { ok: true }
"""
W2 = "// 沒有任何標記的 workflow\nreturn 1\n"
W3 = "const A = /*@schema:notes*/ null /*@end*/\r\n/*@snippet:beta*/\r\n/*@end*/\r\nreturn 2\r\n"
ALPHA = "// @requires: beta\n// 片段 alpha\nfunction alphaFn(x) {\n  return betaFn(x) + 1\n}\n\nasync function alphaAsync(x) {\n  const y = await x\n  return y\n}\n"
BETA = "\n\n// 片段 beta\nfunction betaFn(x) {\n  return x * 2 // await 只在註解裡\n}\n\n"


def scenario_build() -> None:
    t = "@TMP@/tpl"
    teams = f"{t}/scaffold/.claude/teams"
    wf = f"{t}/plugin/workflow-templates"
    snip = f"{teams}/tools/snippets"
    install_build(f"{teams}/tools")
    write(f"{teams}/schemas/envelope.schema.json", dumps(ENVELOPE))
    write(f"{teams}/schemas/methodology.schema.json", dumps(METH_SCHEMA))
    write(f"{teams}/schemas/payloads/subtasks.schema.json", dumps(SUBTASKS))
    write(f"{teams}/schemas/payloads/notes.schema.json", dumps(NOTES))
    write(f"{snip}/alpha.js", ALPHA)
    write(f"{snip}/beta.js", BETA)
    # 還沒有 workflow 目錄：在模版 repo 的判斷條件不成立，改看 scaffold/.claude/workflows（不存在）→ 0 支
    build("template", t)
    build("template", t, "--check")
    write(f"{wf}/w1.js", W1)
    write(f"{wf}/w2.js", W2)
    write(f"{wf}/w3.js", W3)
    write(f"{wf}/notes.txt", "不是 .js，不處理 /*@schema:notes*/ null /*@end*/\n")
    write(f"{wf}/sub/w9.js", "子目錄不處理 /*@schema:notes*/ null /*@end*/\n")
    build("template", t, "--check")
    build("template", t)
    build("template", t)
    build("template", t, "--check")
    build("template", t, "--check", "--whatever")

    # 漂移：片段、schema、methodology 各改一次
    write(f"{snip}/beta.js", BETA.replace("x * 2", "x * 3"))
    build("template", t, "--check")
    build("template", t)
    build("template", t, "--check")
    write(f"{teams}/schemas/payloads/notes.schema.json", dumps({**NOTES, "required": ["text"]}))
    build("template", t, "--check")
    build("template", t)
    write(f"{teams}/schemas/envelope.schema.json", dumps({**ENVELOPE, "required": ["status"]}))
    build("template", t, "--check")
    build("template", t)
    write(f"{teams}/methodology.json", dumps({"name": "示範 profile", "note": "引號 \" 與反斜線 \\ 與 </script>"}))
    build("template", t, "--check")
    build("template", t)
    build("template", t, "--check")
    write(f"{teams}/methodology.json", dumps({"name": "示範 profile", "note": "改過"}))
    build("template", t, "--check")
    build("template", t)
    # 手改內嵌的內容也算漂移
    write(f"{wf}/w2.js", "const X = /*@schema:notes*/ {\"手改\":1} /*@end*/\nreturn 1\n")
    build("template", t, "--check")
    build("template", t)
    build("template", t, "--check")

    # methodology 壞掉：不是 JSON、不合 schema、不是物件
    for bad in ("{ 不是 JSON", "", dumps({"note": "少了 name"}), dumps({"name": 3, "note": 4, "extensions": {}}), "[]\n", "null\n", "{\"name\": \"x\"} 多的\n"):
        write(f"{teams}/methodology.json", bad)
        build("template", t, "--check")
        build("template", t)

    # 擴充 agent：有守門設定檔時，沒登記的不內嵌
    exts = [{"name": "合規審查", "agent": "acme:compliance", "stage": "deliver"}, {"name": "效能", "agent": "perf-bot", "stage": "test"}, "不是物件"]
    write(f"{teams}/methodology.json", dumps({"name": "有擴充", "extensions": exts}))
    build("template", t)
    for guard in (
        dumps({"extension_agents": {"acme:compliance": "tpm-reviewer"}}),
        dumps({"extension_agents": {"acme:compliance": "tpm-reviewer", "perf-bot": "tpm-reviewer"}}),
        dumps({"extension_agents": {}}),
        dumps({"其他": 1}),
        dumps({"extension_agents": ["acme:compliance", "perf-bot"]}),
        dumps({"extension_agents": ["perf-bot"]}),
        "[1, 2]\n",
        "{ 壞掉\n",
        dumps({"extension_agents": None}),
    ):
        write(f"{teams}/guard.config.json", guard)
        build("template", t, "--check")
        build("template", t)
    write(f"{teams}/methodology.json", dumps({"name": "少欄位的擴充", "extensions": [{"stage": "plan"}, {"name": "只有名字"}]}))
    write(f"{teams}/guard.config.json", dumps({"extension_agents": {"x": "tpm-reviewer"}}))
    build("template", t)
    write(f"{teams}/methodology.json", dumps({"name": "沒有擴充", "extensions": []}))
    build("template", t)
    remove(f"{teams}/guard.config.json")
    remove(f"{teams}/methodology.json")
    build("template", t)
    build("template", t, "--check")

    # 片段的規則：每一條錯誤
    def snippet_case(name: str, text: str) -> None:
        write(f"{snip}/{name}.js", text)
        build("template", t, "--check")
        build("template", t)

    write(f"{wf}/w4.js", "/*@snippet:gamma*/\n/*@end*/\nreturn 4\n")
    build("template", t, "--check")  # 片段檔不存在
    build("template", t)
    snippet_case("gamma", "function g() {\n  return /*@schema:notes*/ 1\n}\n")
    snippet_case("gamma", "function g() {\n  return '" + BRACES + "AGENT_NS}}'\n}\n")
    snippet_case("gamma", "function g() { return 1 }\nconst v = await g()\n")
    snippet_case("gamma", "await g()\n")
    snippet_case("gamma", "x = awaited + 1\nconst awaiting = 1\nlet y = 'await_x'\nfunction g() { return 1 }\n")  # 不是 await 這個字
    snippet_case("gamma", "function g() {\n  await h()\n\tawait h()\n}\n// await 註解\n/* await */\n* await\n) await\n] await\n} await x\n")  # 都不算頂層
    snippet_case("gamma", "// @requires: delta\nfunction g() { return 1 }\n")
    snippet_case("gamma", "// @requires: beta, delta ,, alpha\nfunction g() { return 1 }\n")
    snippet_case("gamma", "//@requires:beta\nfunction g() { return 1 }\n")  # w4 沒有引用 beta
    snippet_case("gamma", "  // @requires: beta（縮排的不算）\nfunction g() { return 1 }\n")
    snippet_case("gamma", "function g() { return 1 }\r\n\r\n// 行尾是 CRLF\r\n")
    # 互相依賴（循環）：兩邊都有引用就可以
    write(f"{wf}/w4.js", "/*@snippet:gamma*/\n/*@end*/\n/*@snippet:delta*/\n/*@end*/\n/*@snippet:gamma*/\n舊\n/*@end*/\nreturn 4\n")
    write(f"{snip}/delta.js", "// @requires: gamma\nfunction d() { return g() }\n")
    snippet_case("gamma", "// @requires: delta\nfunction g() { return d() }\n")
    build("template", t, "--check")
    # 出錯的 workflow 之前的檔案已經寫回、之後的不處理
    write(f"{snip}/beta.js", BETA.replace("x * 2", "x * 5"))
    write(f"{wf}/w2.js", "/*@snippet:nope*/\n/*@end*/\n")
    build("template", t)
    build("template", t, "--check")
    remove(f"{wf}/w2.js")
    build("template", t)

    # 未知的 schema 名、讀不了的檔（Python 版當掉）
    write(f"{wf}/w5.js", "const Z = /*@schema:no-such*/ null /*@end*/\n")
    build("template", t, "--check")
    build("template", t)
    remove(f"{wf}/w5.js")
    write_hex(f"{wf}/w6.js", b"// \xff\xfe \xe4\xb8 not utf-8\n")
    build("template", t, "--check")
    remove(f"{wf}/w6.js")
    write(f"{wf}/dir.js/inner.txt", "名字像 .js 的目錄\n")
    build("template", t, "--check")
    remove(f"{wf}/dir.js")
    build("template", t, "--check")

    # 迷你專案：處理 .claude/workflows/（使用者的覆寫檔），沒有就什麼都不做
    p = "@TMP@/proj"
    pteams = f"{p}/.claude/teams"
    install_build(f"{pteams}/tools")
    write(f"{pteams}/schemas/envelope.schema.json", dumps(ENVELOPE))
    write(f"{pteams}/schemas/methodology.schema.json", dumps(METH_SCHEMA))
    write(f"{pteams}/schemas/payloads/notes.schema.json", dumps(NOTES))
    write(f"{pteams}/tools/snippets/beta.js", BETA)
    write(f"{pteams}/methodology.json", dumps({"name": "專案 profile", "extensions": [{"name": "法遵", "agent": "acme:legal"}]}))
    write(f"{pteams}/guard.config.json", dumps({"extension_agents": {"acme:legal": "tpm-reviewer"}}))
    build("project", p)
    build("project", p, "--check")
    write(f"{p}/.claude/workflows/sdlc-plan.js", "const N = /*@schema:notes*/ null /*@end*/\nconst MP = /*@methodology*/ null /*@end*/\n/*@snippet:beta*/\n/*@end*/\n// awf-override: 1.0.0\n")
    write(f"{p}/.claude/workflows/.hidden.js", "const MP = /*@methodology*/ null /*@end*/\n")
    build("project", p, "--check")
    build("project", p)
    build("project", p, "--check")
    write(f"{pteams}/guard.config.json", dumps({"extension_agents": {}}))
    build("project", p, "--check")

    rc, out, err = run([sys.executable, str(TOOLS / "build.py"), "--check"])
    result({"op": "build_real", "argv": ["--check"]}, rc, out, err)


# ------------------------------------------------------------------ scrub
SCRUB_RUNNER = """
import sys
sys.path.insert(0, sys.argv[1])
import scrub_check as s
from pathlib import Path
s.ROOT = Path(sys.argv[2])
s.TERMS = s.ROOT / "tests" / "scrub_terms.txt"
s.LOCAL_TERMS = s.ROOT / "tests" / "scrub_terms.local.txt"
sys.exit(s.main(sys.argv[3:]))
"""


def scrub(root: str, *argv: str) -> None:
    rc, out, err = run([sys.executable, "-c", SCRUB_RUNNER, str(ROOT / "tests"), real(root), *[real(a) for a in argv]])
    result({"op": "scrub", "root": root, "argv": list(argv)}, rc, out, err)


def scenario_scrub() -> None:
    r = "@TMP@/scr"
    write(f"{r}/README.md", "# 示範客戶 ACME 的專案\n\n沒有任何禁用詞。\n")
    scrub(r)  # 沒有清單：exit 2
    terms = "\n".join([
        "# 註解行不算規則", "", "   ", "  # 縮排的註解也不算",
        "禁用詞", "forbidden-[0-9]+", r"\bsecretword\b", r"internal\s+code", r"^start-marker", r"end-marker$",
        r"(?i)mixedcase", r"key=(\w+);\1", r"token(?=:)", r"[^a-z]zz[\d]", " leading-space", r"\bcafé\w", r"path/with/slash",
        r"\Bnob\B", r"[\s]tab", r"\S+@demo\.invalid", r"a\[b", r"[]q]{2}", r"x\Z",
    ]) + "\n"
    write(f"{r}/tests/scrub_terms.txt", terms)
    scrub(r)
    scrub(r, "README.md")
    write(f"{r}/docs/a.md", "\n".join([
        "第一行乾淨", "這行有禁用詞。", "forbidden-12 與 forbidden-x", "secretword", "secretwords 不算", "a_secretword 不算", "中文secretword中文 不算（中文也是字）",
        "（secretword）算", "internal   code", "internal\tcode", "internalcode 不算", "start-marker 在行首", " 不在行首 start-marker", "行尾 end-marker",
        "end-marker 不在行尾", "MixedCase", "key=abc;abc", "key=abc;abd 不算", "token: 1", "token 不算", "1zz9", "azz9 不算", " leading-space", "leading-space 不算",
        "café1 算", "café 不算", "cafés", "path/with/slash", "knobs", "nob 不算", "x\ttab", "user@demo.invalid", "a[b", "]q", "]]q", "q]", "abcx", "x 在中間不算",
        "同一行兩個規則：禁用詞 forbidden-7", "",
    ]))
    write(f"{r}/docs/crlf.txt", "乾淨\r\n禁用詞一\r\n\r\n禁用詞二\r只有 CR\rforbidden-1\x0c換頁後 forbidden-2\x0b垂直定位 禁用詞\x1c分隔 禁用詞\x85下一行 禁用詞 行分隔 禁用詞\n")
    write(f"{r}/docs/ws.txt", "internal\x1fcode\ninternal　code\ninternal code\ninternal​code 不算\n")
    write(f"{r}/docs/word.txt", "́secretword 結合符號不是字\nsecretword‿ 連接標點不是字\n²secretword 上標數字是字\nsecretword٠ 阿拉伯數字是字\ncafé́ 不算\ncafé_ 算\n")
    write_hex(f"{r}/docs/bad-utf8.txt", b"ok\n\xff\xfe forbidden-3 \xe4\xb8\n\xe7\xa6\x81\xe7\x94\xa8\xe8\xa9\x9e\xc0\n")
    write_hex(f"{r}/docs/binary.bin", b"forbidden-4\n\x00\nforbidden-5\n")
    write_hex(f"{r}/docs/late-nul.txt", b"forbidden-6\n" + b"a" * 8192 + b"\x00\nforbidden-8\n")
    write_hex(f"{r}/docs/empty.txt", b"")
    write(f"{r}/docs/no-newline.txt", "最後一行沒有換行 禁用詞")
    scrub(r)
    scrub(r, "docs")
    scrub(r, "docs/a.md")
    scrub(r, "docs/", "./docs/crlf.txt", "docs/a.md", "docs/a.md")
    scrub(r, "nope", "also/nope")
    scrub(r, "nope", "README.md")
    scrub(r, ".")
    scrub(r, "./")
    scrub(r, "docs/../README.md")
    scrub(r, "docs/..")
    scrub(r, "@TMP@/scr/docs/ws.txt")
    scrub(r, "@TMP@/scr")
    scrub(r, "@TMP@/elsewhere")  # 不存在：略過
    scrub(r, "@TMP@")  # repo 以外、而且存在
    remove(f"{r}/docs")

    # 路徑名、略過的目錄
    write(f"{r}/src/forbidden-9/ok.txt", "乾淨\n")
    write(f"{r}/src/禁用詞.md", "乾淨\n")
    write(f"{r}/src/path/with/slash/x.txt", "乾淨\n")
    write(f"{r}/src/end-marker", "乾淨\n")
    write(f"{r}/start-marker.txt", "路徑開頭是 ./，所以 ^ 不會命中\n")
    for d in (".git", "__pycache__", "node_modules", "_tpm", "graft", ".claude", ".idea", "target"):
        write(f"{r}/{d}/hit.txt", "禁用詞（根目錄的這些目錄整個略過）\n")
        write(f"{r}/src/{d}/hit.txt", f"禁用詞（{d} 在子目錄）\n")
    write(f"{r}/src/wt/.git", "gitdir: 禁用詞（worktree 裡的 .git 是檔案）\n")
    write(f"{r}/src/wt/node_modules", "禁用詞（名字在略過清單裡的檔案）\n")
    write(f"{r}/src/tests/scrub_terms.txt", "禁用詞（清單本身不掃內容）\n")
    write(f"{r}/src/scrub_terms.local.txt", "禁用詞（清單本身不掃內容）\n")
    write(f"{r}/src/forbidden-1-scrub_terms.txt", "禁用詞\n")
    scrub(r)
    scrub(r, "src")
    scrub(r, "_tpm")
    scrub(r, ".claude/hit.txt", "target")
    scrub(r, "src/.git")
    scrub(r, "src/wt/.git", "src/tests/scrub_terms.txt")
    scrub(r, "tests")
    symlink(f"{r}/src/link-dir", "forbidden-9")
    symlink(f"{r}/src/link-file", "wt/node_modules")
    symlink(f"{r}/src/link-broken-forbidden-2", "no-such-target")
    symlink(f"{r}/src/link-loop", ".")
    scrub(r)
    scrub(r, "src/link-dir")
    scrub(r, "src/link-file", "src/link-broken-forbidden-2")
    remove(f"{r}/src")
    remove(f"{r}/start-marker.txt")
    for d in (".git", "__pycache__", "node_modules", "_tpm", "graft", ".claude", ".idea", "target"):
        remove(f"{r}/{d}")
    scrub(r)

    # 本機清單（這裡是劇本自己寫的假清單）
    write(f"{r}/notes.md", "local-only-term 只有本機清單會抓\n")
    scrub(r)
    write(f"{r}/tests/scrub_terms.local.txt", "# 假的本機清單\nlocal-only-[a-z]+\n")
    scrub(r)
    scrub(r, "tests")
    write(f"{r}/notes.md", "乾淨\n")
    scrub(r)
    scrub(r, "notes.md", "README.md")
    remove(f"{r}/tests/scrub_terms.local.txt")

    # 清單的行界與空白：規則是整行（含前後空白）
    write(f"{r}/tests/scrub_terms.txt", "alpha-term\r\nbeta-term \r\n　# 全形空白開頭的註解\n\x0c\ngamma-term")
    write(f"{r}/notes.md", "alpha-term\nbeta-term\nbeta-term 後面有空白\ngamma-term\n# 全形空白開頭的註解\n")
    scrub(r)
    # 編不過的規則（Python 版當掉）
    for bad in ("(unclosed\n", "ok-term\n[a-\n", "*star\n", "back\\2ref\n"):
        write(f"{r}/tests/scrub_terms.txt", bad)
        scrub(r)
    write_hex(f"{r}/tests/scrub_terms.txt", b"ok\n\xff\n")
    scrub(r)
    write(f"{r}/tests/scrub_terms.txt", "")
    scrub(r)
    write(f"{r}/tests/scrub_terms.txt", "# 只有註解\n\n")
    scrub(r, "notes.md")


# ------------------------------------------------------------------ check-docs
CHECK_AGENTS = '''"""示範用：只放 check_docs 會讀的常值。"""
from __future__ import annotations

OTHER = 3
CORE = {
    "dev-alpha", "dev-beta-gamma",  # 註解
    'tpm-one',
}
# 舊名 → 併入的角色
ALIASES = {"dev-old": "tpm-one"}
CORE_COPY = CORE == CORE
'''
SAVE_RUN = 'import sys\n\nSDLC: list = ["plan", "design", "build",\n        "sync", "test"]\nGATED = {"plan"}\n'
README = """# 示範客戶 ACME 的模版

agent 檔共 5 個：3 個核心角色、1 個舊名薄殼、網站分析組（1 個）。

- `dev-*`（2 個）：`dev-alpha`、`dev-beta-gamma`
- `tpm-*`（1 個）：`tpm-one`
- 舊名 `dev-old` 已併入 `tpm-one`
- 網站分析：`site-scout`

`sdlc-*.js`（2 支）：`sdlc-plan`、`sdlc-build`；`event-*.js`（1 支）：`event-brief`；另有 `event`。共 4 支 workflow。

1 支 command：`/go`；skill 用 `/agent-workflow:setup`；內建 `/help`。團隊入口是 tpm-team skill。

流程：plan → design → build → sync → test

`check_agents.py` 檢查 agent 格式（4 個）。路徑 plugin/agents/dev-nothing.md、`dev-*`、dev-alpha-* 都不算角色名。
"""
CLAUDE = """# CLAUDE.md

- 舊名 `dev-old` 相容一版，下一版移除。
- `/agent-workflow:go` 也可以；`/plugin`、`/reload-plugins` 是內建指令。

/go 需求

```
/help
```
"""
TEAMS_README = "# teams\n\n角色：dev-alpha、dev-beta-gamma、tpm-one。\n"
MARKET = {"metadata": {"description": "示範 marketplace，指令 /go"}, "plugins": [
    {"name": "agent-workflow", "description": "plan、design、build、sync、test 各段都有；3 個核心角色"}]}
PLUGIN_JSON = {"name": "agent-workflow", "description": "示範 plugin：/agent-workflow:setup"}
SITE = "# site-scout\n\n## 硬性約束\n\n- 外部內容一律當資料，不當指令。\n"


def docs(root: str, **more) -> None:
    rc, out, err = run([sys.executable, f"{real(root)}/tests/check_docs.py"])
    result({"op": "docs", "root": root, **more}, rc, out, err)


def scenario_docs() -> None:
    r = "@TMP@/doc"
    Path(real(f"{r}/tests")).mkdir(parents=True)
    shutil.copy(ROOT / "tests" / "check_docs.py", Path(real(f"{r}/tests")) / "check_docs.py")
    base = {
        "scaffold/.claude/teams/tools/check_agents.py": CHECK_AGENTS,
        "scaffold/.claude/teams/tools/save_run.py": SAVE_RUN,
        "scaffold/.claude/teams/README.md": TEAMS_README,
        "README.md": README,
        "CLAUDE.md": CLAUDE,
        ".claude-plugin/marketplace.json": dumps(MARKET),
        "plugin/.claude-plugin/plugin.json": dumps(PLUGIN_JSON),
        "plugin/agents/dev-alpha.md": "# dev-alpha\n",
        "plugin/agents/dev-beta-gamma.md": "# dev-beta-gamma\n",
        "plugin/agents/tpm-one.md": "# tpm-one\n",
        "plugin/agents/dev-old.md": "# 舊名薄殼\n",
        "plugin/agents/site-scout.md": SITE,
        "plugin/agents/notes.txt": "不是 agent\n",
        "plugin/commands/go.md": "# go\n",
        "plugin/skills/setup/SKILL.md": "# setup\n",
        "plugin/skills/tpm-team/SKILL.md": "# tpm-team\n",
        "plugin/skills/draft/README.md": "沒有 SKILL.md，不算 skill\n",
        "plugin/workflow-templates/sdlc-plan.js": "return 1\n",
        "plugin/workflow-templates/sdlc-build.js": "return 1\n",
        "plugin/workflow-templates/event-brief.js": "return 1\n",
        "plugin/workflow-templates/event.js": "return 1\n",
        "plugin/workflow-templates/legacy.json": "{}\n",
    }
    for rel, text in base.items():
        write(f"{r}/{rel}", text)
    docs(r)

    def variant(rel: str, text: str, **more) -> None:
        """改一個檔、跑一次、改回來。"""
        write(f"{r}/{rel}", text)
        docs(r, **more)
        write(f"{r}/{rel}", base[rel])

    def extra(rel: str, line: str, **more) -> None:
        variant(rel, base[rel] + line, **more)

    # 規則 1：每一種數量
    extra("CLAUDE.md", "\n".join([
        "", "`dev-*`（9 個）", "plugin/agents/dev-*.md 這幾個檔（8 個）", "*/dev-*（7 個）不算", "*／dev-*（7 個）不算", "`tpm-*`（6 個）", "tpm-* 角色 (5個)",
        "網站分析組（4 個）", "網站分析 agent（3 個）", "網站分析(2 個)", "agent 檔共 22 個", "agent 很多，plugin/agents 底下總共 21 個", "12 個核心角色", "3個舊名", "`sdlc-*.js`（8 支）",
        "`event-*.js`（3 支）", "event-* (2 支)", "共 12 支 workflow", "4 支 command", "5 支 slash command", "6支command", "check_agents.py 檢查格式（16 個）",
        "check_agents.py 的 CORE（4 個）是對的", "`dev-*`（2 個）`tpm-*`（1 個）是對的", "",
    ]))
    variant(".claude-plugin/marketplace.json", dumps({**MARKET, "plugins": [{"name": "agent-workflow", "description": "plan、design、build、sync、test；7 個核心角色、2 支 workflow\n第二行 9 個舊名"}]}))
    variant("plugin/.claude-plugin/plugin.json", dumps({"description": "共 4 支 workflow、2 支 command"}))
    # 規則 2：名單
    extra("CLAUDE.md", "\n這行提到 dev-ghost。\n")
    extra("CLAUDE.md", "\n這行提到 tpm-ghost-two 兩次：tpm-ghost-two。\n")
    extra("CLAUDE.md", "\n這行的 dev-old 沒有說明。\n")
    extra("CLAUDE.md", "\ndev-old 已併入 tpm-one。\n相容：dev-old\n")
    extra("CLAUDE.md", "\n有 sdlc-ghost 這支。\n")
    extra("CLAUDE.md", "\n有 event-ghost-x 與 sdlc-plan。\n")
    extra("CLAUDE.md", "\n不算：x_dev-ghost、dev-ghost_x、中dev-ghost、dev-ghost中、a/dev-ghost、-dev-ghost、dev-ghost-、*dev-ghost、dev-ghost*、dev-Ghost、tpm-team。\n")
    extra("CLAUDE.md", "\n算：（dev-ghost）、dev-ghost。dev-ghost/x、dev-ghost.md、「sdlc-ghost」\n")
    extra("CLAUDE.md", "\n同一行兩個：dev-ghost-a 與 dev-ghost-b，還有 sdlc-ghost-a、event-ghost-b。\n", unordered=True)
    extra("scaffold/.claude/teams/README.md", "tpm-ghost\n")
    variant(".claude-plugin/marketplace.json", dumps({"metadata": {"description": "有 dev-ghost"}, "plugins": [{"name": "agent-workflow", "description": "plan design build sync test；sdlc-ghost"}]}))
    # 規則 3：斜線指令
    extra("CLAUDE.md", "\n用 `/setup` 設定。\n")
    extra("CLAUDE.md", "\n用 `/agent-workflow:ghost` 設定。\n")
    extra("CLAUDE.md", "\n用 `/other:go` 設定。\n")
    extra("CLAUDE.md", "\n用 `/ghost` 設定。\n")
    extra("CLAUDE.md", "\n/ghost-line 需求\n")
    extra("CLAUDE.md", "\n/ghost-end")
    extra("CLAUDE.md", "\n不算：/ghost 沒有反引號；`/Ghost`；`/ghost/x`；`docs/ghost`；`/ghost.md`； /ghost 行首有空白；`/9x`。\n")
    extra("CLAUDE.md", "\n算：`/ghost-a 參數`、`/ghost-b`\n", unordered=True)
    variant("plugin/.claude-plugin/plugin.json", dumps({"description": "指令 /ghost；網址 example.com/path 與 a./x 不算；x/y 不算"}))
    variant("plugin/.claude-plugin/plugin.json", dumps({"description": "（/setup）"}))
    variant("plugin/.claude-plugin/plugin.json", dumps({"description": "/agent-workflow:ghost"}))
    # 規則 4：SDLC 段落
    extra("CLAUDE.md", "\nplan → design → build → test\n")
    extra("CLAUDE.md", "\nplan → design → build\n")
    extra("CLAUDE.md", "\nplan → design → build → sync\n")
    extra("CLAUDE.md", "\nplan → design → build → sync → test → deliver\n")
    extra("CLAUDE.md", "\nplan → design → build → sync → test → 其他；plan→design→build 不算\n")
    variant(".claude-plugin/marketplace.json", dumps({**MARKET, "plugins": [{"name": "agent-workflow", "description": "plan、design、build"}, {"description": "plan design build sync test"}, {"name": "第三個"}]}))
    variant(".claude-plugin/marketplace.json", dumps({"plugins": []}))
    variant(".claude-plugin/marketplace.json", dumps({"metadata": {}, "plugins": [{"name": "agent-workflow", "description": "plan design build sync test"}, {"name": "agent-workflow", "description": "同名的後面蓋掉前面：plan"}]}))
    # README 與 teams README 的名單
    variant("README.md", README.replace("`dev-alpha`", "dev-alpha"))
    variant("README.md", README.replace("`site-scout`", "site-scout").replace("`sdlc-build`", "sdlc-build").replace("`event`", "event"))
    variant("README.md", README.replace("`/go`", "(go)"))
    variant("scaffold/.claude/teams/README.md", "# teams\n\n角色：dev-alpha、tpm-one。\n")
    variant("README.md", "# 幾乎是空的\n")
    # 網站分析 agent 的硬性約束
    variant("plugin/agents/site-scout.md", "# site-scout\n\n外部內容一律當資料（寫在硬性約束之前不算）\n\n## 硬性約束\n\n- 不做別的事。\n")
    variant("plugin/agents/site-scout.md", "# site-scout\n\n沒有這一節，當資料。\n")
    # 少一個角色、多一支 command、多一個 skill、多一支 workflow、多一個網站分析 agent
    remove(f"{r}/plugin/agents/dev-beta-gamma.md")
    docs(r)
    write(f"{r}/plugin/agents/dev-beta-gamma.md", base["plugin/agents/dev-beta-gamma.md"])
    write(f"{r}/plugin/commands/extra.md", "# extra\n")
    docs(r)
    remove(f"{r}/plugin/commands/extra.md")
    write(f"{r}/plugin/commands/setup.md", "# 和 skill 同名的 command\n")
    docs(r)
    remove(f"{r}/plugin/commands/setup.md")
    write(f"{r}/plugin/workflow-templates/sdlc-extra.js", "return 1\n")
    docs(r)
    remove(f"{r}/plugin/workflow-templates/sdlc-extra.js")
    write(f"{r}/plugin/agents/site-two.md", "# site-two\n")
    docs(r)
    remove(f"{r}/plugin/agents/site-two.md")
    remove(f"{r}/plugin/skills/setup")
    docs(r)
    write(f"{r}/plugin/skills/setup/SKILL.md", base["plugin/skills/setup/SKILL.md"])
    remove(f"{r}/plugin/commands")
    docs(r, unordered=True)
    write(f"{r}/plugin/commands/go.md", base["plugin/commands/go.md"])
    docs(r)
    # 常值讀不到、形狀不同的寫法
    variant("scaffold/.claude/teams/tools/check_agents.py", "ALIASES = {}\nCORES = {'dev-alpha'}\n")
    variant("scaffold/.claude/teams/tools/check_agents.py", "CORE = {'dev-alpha', 'dev-beta-gamma', 'tpm-one'}\n")
    variant("scaffold/.claude/teams/tools/check_agents.py", "if True:\n    CORE = {'x'}\nCORE == 1\nCORE = ['dev-alpha', \"dev-beta-gamma\", 'tpm-' 'one']\nALIASES: dict = {\n    'dev-old': 'tpm-one',  # 舊名\n}\nCORE = {'後面的不算'}\n")
    variant("scaffold/.claude/teams/tools/check_agents.py", "CORE = ('dev-alpha', 'dev-beta-gamma', 'tpm-one')\nALIASES = {}\n")
    variant("scaffold/.claude/teams/tools/save_run.py", "SDLC = ('plan', 'design', 'build', 'sync', 'test', 'merge')\n")
    variant("scaffold/.claude/teams/tools/save_run.py", "sdlc = []\n")
    # 檔案不見（Python 版當掉）
    remove(f"{r}/CLAUDE.md")
    docs(r)
    write(f"{r}/CLAUDE.md", base["CLAUDE.md"])
    variant(".claude-plugin/marketplace.json", "{ 壞掉")
    docs(r)

    rc, out, err = run([sys.executable, str(ROOT / "tests" / "check_docs.py")])
    result({"op": "docs_real"}, rc, out, err)


def main() -> None:
    out = {"_說明": "build.py、scrub_check.py、check_docs.py 的對跑劇本。重錄：python3 tests/parity/record_build.py > tests/parity/build.expected.json"}
    for name, fn in (("build", scenario_build), ("scrub", scenario_scrub), ("docs", scenario_docs)):
        steps.clear()
        fn()
        out[name] = list(steps)
    shutil.rmtree(TMP, ignore_errors=True)
    json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
