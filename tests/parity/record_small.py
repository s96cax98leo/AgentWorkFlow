#!/usr/bin/env python3
"""錄三支小工具的對跑基準：run_brief、check_envelope、check_rules。

用法：python3 tests/parity/record_small.py > tests/parity/small.expected.json
內容是一份劇本（steps），另一個實作照做一遍、逐字比對：
  - detect：run_brief.detect(text) 的輸出。
  - brief：run_brief.brief(cmd, log, tail) 的結束碼、摘要、log 內容（指令用 sh，所以只在 unix 上對跑）。
  - default_log：AWF_LOG_DIR 底下的檔名（之後建立該檔，讓序號往下走）。
  - write／git：準備檔案與 git 歷史（author、committer、日期固定，SHA 可重現）。
  - envelope：check_envelope 的指令列（schema 用 scaffold 的）。
  - rules：check_rules 的指令列（專案在暫存目錄，各有自己的 guard.config.json）。
err_any 是 true 的步驟只比 exit code 與 stdout：Python 版在那裡直接當掉（traceback）或印用法（指令名不同），另一個實作印自己的說明。
暫存路徑換成 @TMP@。同一個實作連錄兩次要逐字相同。這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
TOOLS = TEAMS / "tools"
sys.path.insert(0, str(TOOLS))
import run_brief  # noqa: E402

TMP = Path(tempfile.mkdtemp(prefix="awf-small-")).resolve()
GIT_ENV = {
    "GIT_AUTHOR_NAME": "Demo", "GIT_AUTHOR_EMAIL": "demo@example.com", "GIT_COMMITTER_NAME": "Demo",
    "GIT_COMMITTER_EMAIL": "demo@example.com", "GIT_AUTHOR_DATE": "2026-01-02T03:04:05+00:00",
    "GIT_COMMITTER_DATE": "2026-01-02T03:04:05+00:00", "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
}
steps = []


def norm(s: str) -> str:
    return s.replace(str(TMP), "@TMP@")


def real(s: str) -> str:
    return s.replace("@TMP@", str(TMP))


def write(path: str, text: str) -> None:
    p = Path(real(path))
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(text.encode("utf-8"))
    steps.append({"op": "write", "path": path, "text": text})


def git(cwd: str, *args: str) -> None:
    Path(real(cwd)).mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", *args], cwd=real(cwd), env={**os.environ, **GIT_ENV}, check=True, capture_output=True)
    steps.append({"op": "git", "cwd": cwd, "args": list(args)})


def detect(text: str) -> None:
    name, fails = run_brief.detect(text)
    steps.append({"op": "detect", "text": text, "want": [name, fails]})


def brief(cmd: list, log: str, tail=None) -> None:
    code, text = run_brief.brief(cmd, Path(real(log)), tail)
    logged = Path(real(log)).read_bytes().decode("utf-8")
    steps.append({"op": "brief", "cmd": cmd, "log": log, "tail": tail, "code": code, "text": norm(text), "logged": logged})


def default_log(name: str) -> None:
    os.environ["AWF_LOG_DIR"] = real("@TMP@/envlogs")
    p = run_brief.default_log(name)
    p.write_text("x", encoding="utf-8")
    steps.append({"op": "default_log", "name": name, "want": norm(str(p))})


def cli(op: str, script: Path, argv: list, cwd: Path, stdin=None, err_any=False, **extra) -> None:
    p = subprocess.run([sys.executable, str(script), *[real(a) for a in argv]], cwd=cwd, input=stdin if stdin is not None else "",
                       capture_output=True, text=True, encoding="utf-8", env={**os.environ, **GIT_ENV})
    step = {"op": op, **extra, "argv": argv, "stdin": stdin, "rc": p.returncode, "out": norm(p.stdout)}
    if err_any:
        step["err_any"] = True
    else:
        step["err"] = norm(p.stderr)
    steps.append(step)


def envelope(argv: list, stdin=None, err_any=False) -> None:
    cli("envelope", TOOLS / "check_envelope.py", argv, ROOT, stdin, err_any)


def project(name: str, config=None) -> str:
    """暫存專案：把兩支工具複製進去（check_rules 依自己的位置找專案根目錄）。"""
    d = TMP / name / ".claude" / "teams" / "tools"
    d.mkdir(parents=True)
    for f in ("check_rules.py", "schemalib.py"):
        shutil.copy(TOOLS / f, d / f)
    if config is not None:
        write(f"@TMP@/{name}/.claude/teams/guard.config.json", json.dumps(config, ensure_ascii=False))
    return f"@TMP@/{name}"


def rules(proj: str, argv: list, err_any=False) -> None:
    cli("rules", Path(real(proj)) / ".claude" / "teams" / "tools" / "check_rules.py", argv, Path(real(proj)), None, err_any, project=proj)


# ---------------------------------------------------------------- run_brief
for sample in run_brief.SAMPLES.values():
    detect(sample)
detect("")
detect("nothing to see\nhere\n")
detect("Ran 3 tests in 0.01s\n\nFAIL: test_a (pkg.T)\nERROR: test_b (pkg.T)\nFAIL: test_a (pkg.T)\n")
detect("collected 2 items\nFAILED a::b\nERROR c::d - boom\nFAILED a::b\n== 2 failed in 0.1s ==\n")
detect(" Test Files  1 failed (1)\n × suite > case one  \n ✕ another\n FAIL  x.test.ts\n")
detect("ok  \texample.com/a\t0.1s\n--- FAIL: TestA (0.00s)\n--- FAIL: TestB/sub (0.00s)\n")
detect("PASS\n")
detect("=== 1 passed in 0.5s ===\nFAILED\x1cx y\n")
detect("Tests:\x1f 2 passed\n\x1f● spaced\x1fname\x1f\n")
detect("collected 1 items\r\nFAILED t::a\r\n")

SH = "sh"
brief([SH, "-c", "i=0; while [ $i -lt 50 ]; do echo \"row $i\"; i=$((i+1)); done; exit 3"], "@TMP@/logs/a.log")
brief([SH, "-c", "i=0; while [ $i -lt 50 ]; do echo \"row $i\"; i=$((i+1)); done"], "@TMP@/logs/b.log", 5)
brief([SH, "-c", "echo one; echo two; echo three"], "@TMP@/logs/c.log", 100)
brief([SH, "-c", "echo one; echo two; echo three; echo four"], "@TMP@/logs/d.log", -3)
brief([SH, "-c", "echo one; echo two"], "@TMP@/logs/./e//e.log", 0)
brief([SH, "-c", "echo out; echo err 1>&2; echo out2"], "@TMP@/logs/f.log")
brief([SH, "-c", "printf 'a\\r\\nb\\rc\\n\\n\\n'"], "@TMP@/logs/g.log")
brief([SH, "-c", "printf 'bad \\377 byte\\n'; printf '中文\\n'"], "@TMP@/logs/h.log")
brief([SH, "-c", "true"], "@TMP@/logs/i.log")
brief([SH, "-c", "echo 'collected 25 items'; i=0; while [ $i -lt 25 ]; do echo \"FAILED t.py::t$i - x\"; i=$((i+1)); done; echo '=== 25 failed in 1.00s ==='; exit 1"], "@TMP@/logs/j.log")
brief([SH, "-c", "echo '--- FAIL: TestA (0.00s)'; echo FAIL; exit 1"], "@TMP@/logs/k.log", 2)
brief([SH, "-c", "printf 'x\\013y\\034z\\n'"], "@TMP@/logs/l.log")
brief([SH, "-c", "kill -9 $$"], "@TMP@/logs/m.log")
brief(["awf-no-such-command-xyz", "arg"], "@TMP@/logs/n.log")
for n in ("demo", "demo", "a b/c", "///", "中文", "x.y_z-", "--"):
    default_log(n)
os.environ.pop("AWF_LOG_DIR", None)

# ---------------------------------------------------------------- check_envelope
gs = TEAMS / "golden-set"
n = 0
for kind in ("valid", "invalid"):
    for f in sorted((gs / kind).glob("*.json")):
        case = json.loads(f.read_text(encoding="utf-8"))
        if "payload" not in case or case.get("config"):
            continue
        n += 1
        path = f"@TMP@/env/{kind}-{f.stem}.json"
        write(path, json.dumps(case["data"], ensure_ascii=False, indent=1))
        envelope([path] + (["--payload", case["payload"]] if case.get("payload") else []))
OK_ENV = {"status": "done", "summary": "示範客戶 ACME 的交件", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": []}
fenced = "前言\n```json\n{\"壞\": \n```\n中間\n```json\n" + json.dumps(OK_ENV, ensure_ascii=False) + "\n```\n結尾\n"
envelope(["-"], fenced)
envelope(["-", "--payload", "findings"], fenced)
envelope(["-"], json.dumps(OK_ENV, ensure_ascii=False))
envelope(["-"], "沒有信封")
envelope(["-"], "null")
envelope(["-"], "[]")
envelope(["-"], "3")
envelope(["-"], "")
envelope([], err_any=True)  # 用法訊息：指令名改成 awf check-envelope
envelope(["@TMP@/env/沒有這個檔.json"], err_any=True)
envelope(["-", "--payload", "no-such-payload"], json.dumps(OK_ENV), err_any=True)
envelope(["-", "--payload"], json.dumps(OK_ENV), err_any=True)

# ---------------------------------------------------------------- check_rules
DOC = "\n".join([
    "# 示範客戶 ACME 的週報",
    "這項工作預估要 3 人天，另一項 2.5 man-days，還有約 5 小時 工時。",
    "規則：不寫人天。",
    "Co-Authored-By: someone",
    "co-authored-by: lower",
    "  Co-Authored-By: indented",
    "對方能力不足，而且很差勁。",
    "TASK-0001 已完成，TASK-9999 是新的；xTASK-0002 與 A-TASK-0003 不算。",
    "內部代號 int-12、req-7、Z5、xZ6。",
    "Signed-off-by: Demo",
    "Generated-By: tool",
    "一行有兩個：1 人月、2 person days",
    "",
]) + "分隔\x0b同一行\x1c另一行\n"
p1 = project("p1")
write("@TMP@/p1/doc.md", DOC)
write("@TMP@/p1/clean.md", "# 沒問題\n內容\n")
for aud in (None, "internal", "owner", "external", "nobody"):
    rules(p1, (["--audience", aud] if aud else []) + ["@TMP@/p1/doc.md"])
rules(p1, ["@TMP@/p1/clean.md"])
rules(p1, ["@TMP@/p1/clean.md", "--audience", "owner", "@TMP@/p1/doc.md"])
rules(p1, [])
rules(p1, ["@TMP@/p1/沒有這個檔.md"], err_any=True)
rules(p1, ["--audience"], err_any=True)
rules(p1, ["--git"], err_any=True)

p2 = project("p2", {
    "task_id_prefix": "TASK-", "backlog_file": "docs/backlog.md",
    "internal_id_patterns": ["int-\\d+", "(req)-(\\d+)", "(?<!x)Z(\\d)"],
    "attribution_line": " Signed-off-by: Demo <demo@example.com> ",
    "forbidden_trailers": ["Co-Authored-By", "signed-off-by", "Generated-By"],
})
write("@TMP@/p2/docs/backlog.md", "- id:TASK-0001 第一項\n- id:TASK-0002, id:other\n- id:xTASK-0005\n")
write("@TMP@/p2/doc.md", DOC)
write("@TMP@/p2/.claude/teams/methodology.json", json.dumps({"conventions": {"commit": {"check_regex": "^(feat|fix)(\\(.+\\))?: .+"}}}))
for aud in ("internal", "owner", "external"):
    rules(p2, ["--audience", aud, "@TMP@/p2/doc.md"])
repo = "@TMP@/p2/repo"
git(repo, "init", "-q", "-b", "main")
git(repo, "commit", "-q", "--allow-empty", "-m", "chore: 起點")
git(repo, "tag", "base")
git(repo, "commit", "-q", "--allow-empty", "-m", "feat(core): 合格的標題\n\n內文\n\nSigned-off-by: Demo <demo@example.com>")
git(repo, "commit", "-q", "--allow-empty", "-m", "隨便寫的標題，而且很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長很長\n\nCo-Authored-By: X <x@example.com>")
git(repo, "commit", "-q", "--allow-empty", "-m", "fix: 缺署名")
git(repo, "commit", "-q", "--allow-empty", "-m", "fix: 兩種都有\n\nsigned-off-by: demo\ngenerated-by: tool")
rules(p2, ["--git", repo, "base..HEAD"])
rules(p2, ["--git", repo, "HEAD..HEAD"])
rules(p2, ["--git", repo, "no-such-ref..HEAD"], err_any=True)
rules(p2, ["--git", repo], err_any=True)

p3 = project("p3", {"task_id_prefix": "{" * 2 + "TASK_ID_PREFIX" + "}" * 2, "attribution_line": "", "forbidden_trailers": []})
write("@TMP@/p3/.claude/teams/methodology.json", json.dumps({"conventions": {"commit": {"check_regex": "("}}}))
write("@TMP@/p3/doc.md", DOC)
rules(p3, ["@TMP@/p3/doc.md"])
rules(p3, ["--git", repo, "base..HEAD"])
for i, m in enumerate(["不是 JSON", json.dumps({"conventions": None}), json.dumps({"conventions": {"commit": {"check_regex": "  "}}}),
                       json.dumps({"conventions": {"commit": {"check_regex": 3}}}), json.dumps([1])]):
    write("@TMP@/p3/.claude/teams/methodology.json", m)
    rules(p3, ["--git", repo, "base..HEAD"])

p4 = project("p4", {"internal_id_patterns": ["("]})
write("@TMP@/p4/doc.md", DOC)
rules(p4, ["--audience", "owner", "@TMP@/p4/doc.md"], err_any=True)
p5 = project("p5")
write("@TMP@/p5/.claude/teams/guard.config.json", "壞掉的設定")
write("@TMP@/p5/doc.md", DOC)
rules(p5, ["--audience", "external", "@TMP@/p5/doc.md"])

shutil.rmtree(TMP, ignore_errors=True)
json.dump({"steps": steps}, sys.stdout, ensure_ascii=False, indent=1)
sys.stdout.write("\n")
