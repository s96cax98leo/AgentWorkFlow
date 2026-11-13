#!/usr/bin/env python3
"""錄 merge gate 與 sync_branch 的對跑基準：兩支工具對一批輸入的實際輸出（判定 JSON、exit code、stdout、stderr、寫進稽核紀錄的那一列）。

用法：python3 tests/parity/record_gates.py > tests/parity/gates.expected.json
內容：
  - blobs：檔案內容表（案例用到的輸入檔，內容相同的只存一份；鍵是 sha1 前 12 碼）。
  - mg.selftest：merge_gate.py --selftest 的輸出。
  - mg.git_setup：selftest 建暫存 git repo 的步驟（rebase 後沿用 verdict 的案例要用；重播時照做一次）。
  - mg.cases：每個案例＝專案目錄的檔案快照＋指令列＋ Python 的實際結果。前面一段是 selftest 的每個案例
    （攔下它呼叫自己的那一次，連同當時的檔案一起記），後面是另外補的 CLI 案例。
  - mg.probes：純函式的輸入與輸出（parse_diff、風險等級、skip／壓制註解的偵測、上游授權、日期、JSON 錯誤訊息…）。
  - sb.selftest、sb.steps：sync_branch.py --selftest 的每一步（git 指令、寫檔、案例的參數與結果），加上另外補的步驟。
正規化：
  - 暫存目錄 → @TMP@；系統的 UTC 日期（今天／昨天／明天）→ @TODAY@／@YESTERDAY@／@TOMORROW@；
    沒給 --timestamp 時輸出的現在時間 → @NOW@；Python 執行檔 → @PY@；selftest 臨時寫的需求清單腳本指令 → "@GEN@"。
  - argparse 的錯誤（exit 2）只記 stderr 最後一行，程式名換成 @PROG@；Python 丟出沒接住的例外時記 crash，不記 traceback。
  - git 的 author／committer 日期與設定用環境變數固定，commit SHA 可重現。
同一個實作連錄兩次要逐字相同。這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
sys.path.insert(0, str(TOOLS))
import merge_gate  # noqa: E402
import recipe as rcp  # noqa: E402
import sync_branch  # noqa: E402

MG = str(TOOLS / "merge_gate.py")
GIT_ENV = {"GIT_AUTHOR_DATE": "2026-01-02T03:04:05+00:00", "GIT_COMMITTER_DATE": "2026-01-02T03:04:05+00:00",
           "GIT_CONFIG_NOSYSTEM": "1"}
os.environ.update(GIT_ENV)
os.environ.pop("CLAUDE_PROJECT_DIR", None)
EMPTY_GITCONFIG = tempfile.NamedTemporaryFile(prefix="awf-gitconfig-", delete=False)
EMPTY_GITCONFIG.close()
os.environ["GIT_CONFIG_GLOBAL"] = EMPTY_GITCONFIG.name

TODAY = datetime.now(timezone.utc).date()
DAYS = [((TODAY + timedelta(days=n)).isoformat(), tag) for n, tag in ((0, "@TODAY@"), (-1, "@YESTERDAY@"), (1, "@TOMORROW@"))]
NOW = re.compile(re.escape(TODAY.isoformat()) + r"T\d\d:\d\d:\d\d\+00:00")
real_run = subprocess.run
blobs = {}
dump = lambda x: json.dumps(x, ensure_ascii=False)  # noqa: E731


def norm(text, tmp):
    """輸出與輸入檔的正規化；tmp 是這個案例的暫存目錄。"""
    if not isinstance(text, str):
        return text
    for p in (os.path.realpath(tmp), str(tmp)):
        text = text.replace(dump([sys.executable, p + "/gen_reqs.py"]), '"@GEN@"')
        text = text.replace(p, "@TMP@")
    text = text.replace(sys.executable, "@PY@").replace(EMPTY_GITCONFIG.name, "@GITCONFIG@")
    text = NOW.sub("@NOW@", text)
    for day, tag in DAYS:
        text = text.replace(day, tag)
    return text


def blob(text):
    key = hashlib.sha1(text.encode("utf-8")).hexdigest()[:12]
    blobs[key] = text
    return key


def snapshot(proj):
    """專案目錄的檔案（不含暫存 git repo）。讀不成文字的檔不會出現在案例裡。"""
    out = {}
    for f in sorted(Path(proj).rglob("*")):
        rel = f.relative_to(proj).as_posix()
        if f.is_file() and not rel.startswith("gitrepo/"):
            out[rel] = blob(norm(f.read_bytes().decode("utf-8", "surrogateescape").encode("utf-8", "replace").decode("utf-8"), proj))
    return out


# ------------------------------------------------------------------ merge gate：案例
mg_cases = []
mg_git_setup = []


def mg_record(name, proj, argv, kwargs, group):
    before = snapshot(proj)
    r = real_run([sys.executable, MG, *argv], **kwargs)
    after = snapshot(proj)
    rec = {"name": name, "group": group, "files": before, "argv": [norm(a, proj) for a in argv], "code": r.returncode,
           "stdout": norm(r.stdout, proj)}
    if "--git-dir" in argv:
        rec["git"] = True
    if kwargs.get("input") is not None:
        rec["stdin"] = kwargs["input"]
    env = kwargs.get("env") or {}
    if env.get("CLAUDE_PROJECT_DIR"):
        rec["project_dir_env"] = norm(env["CLAUDE_PROJECT_DIR"], proj)
    if kwargs.get("cwd"):
        rec["cwd"] = norm(str(kwargs["cwd"]), proj)
    if "Traceback (most recent call last)" in r.stderr:
        rec["crash"] = r.stderr.strip().splitlines()[-1][:160]
    elif r.returncode == 2:
        rec["stderr_last"] = norm(r.stderr.strip().splitlines()[-1], proj).replace("merge_gate.py", "@PROG@")
    else:
        rec["stderr"] = norm(r.stderr, proj)
    changed = {k: v for k, v in after.items() if before.get(k) != v}
    if changed:
        rec["after"] = changed
    mg_cases.append(rec)
    return r


class Tap:
    """selftest 期間攔 subprocess.run 與 Path.write_text：工具呼叫自己的那一次記成案例，建 git repo 的步驟另外記。"""

    def __init__(self):
        self.names = []
        self.proj = None

    def run(self, cmd, *args, **kwargs):
        if isinstance(cmd, list) and len(cmd) > 1 and cmd[0] == sys.executable and cmd[1] == MG:
            argv = cmd[2:]
            self.proj = argv[argv.index("--project-dir") + 1]
            return mg_record(f"selftest#{len(mg_cases) + 1}", self.proj, argv, kwargs, "selftest")
        if isinstance(cmd, list) and cmd[:2] == ["git", "-C"] and self.proj and cmd[2] == self.proj + "/gitrepo":
            mg_git_setup.append({"git": cmd[3:]})
        return real_run(cmd, *args, **kwargs)


def mg_selftest():
    tap = Tap()
    real_write = Path.write_text

    def write_text(self, text, *a, **k):
        if tap.proj and str(self).startswith(tap.proj + "/gitrepo/"):
            mg_git_setup.append({"write": str(self)[len(tap.proj) + 1:], "text": text})
        return real_write(self, text, *a, **k)

    buf = io.StringIO()
    subprocess.run, Path.write_text = tap.run, write_text
    try:
        with contextlib.redirect_stdout(buf):
            code = merge_gate._selftest()
    finally:
        subprocess.run, Path.write_text = real_run, real_write
    # selftest 每個案例印一行；把案例名補回去（「name：判定（預期…）」的 name）
    lines = [l for l in buf.getvalue().splitlines() if "（預期 " in l]
    for rec, line in zip(mg_cases, lines):
        rec["name"] = line.split("] ", 1)[1].rsplit("：", 1)[0]
    return {"code": code, "stdout": re.sub(r"\b[0-9a-f]{12}\b", "@SHA@", buf.getvalue())}


SHA = "a" * 40
REQS = [{"id": "REQ-1", "text": "匯出按鈕下載 CSV", "source": "示範客戶 ACME Story"},
        {"id": "REQ-2", "text": "超過一萬筆時提示縮小範圍", "source": "示範客戶 ACME Story"}]
BASE_CFG = {"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app",
            "auto_merge": {"enabled": True, "repos": {"acme-fork/app": "medium", "acme-up/app": "low"}, "daily_cap": 2},
            "requirements": {"source": "json", "path": "reqs.json"}}
TS = "2099-10-24T09:00:00+00:00"


def row(rid, text="需求", result="match"):
    return {"requirement_id": rid, "requirement": text, "evidence": [f"results.json {rid}"], "result": result}


def payload(rows=None, dec="merge", followups=None, oos=None, sha=SHA, repo="acme-fork/app", pr=3, **extra):
    return {"decision": dec, "repo": repo, "pr": pr, "head_sha": sha, "requirements_source": "json",
            "traceability": [row(r["id"], r["text"]) for r in REQS] if rows is None else rows, "out_of_scope": oos or [],
            "followups": followups or [], "risk_tier": "low", "gate_reasons": [], **extra}


def envelope(**kw):
    return {"status": "done", "summary": "示範", "artifacts": [], "evidence": [], "confidence": 0.9,
            "open_questions": [], "derived_work": [], "payload": payload(**kw)}


GOOD_EV = [{"requirement_id": r["id"], "result": "pass", "evidence": "1 passed"} for r in REQS]


def mk_diff(path, plus, minus=()):
    return (f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{1 if minus else 0},{len(minus)} +1,{len(plus)} @@\n"
            + "".join(f"-{l}\n" for l in minus) + "".join(f"+{l}\n" for l in plus))


TEST_DIFF = mk_diff("tests/test_export.py", ["def test_csv():", "    assert export() == 'csv'"])
SRC_DIFF = mk_diff("src/export.py", ["def export():", "    return 'csv'"])
DROP = object()


def am(**kw):
    return {**BASE_CFG, "auto_merge": {**BASE_CFG["auto_merge"], **kw}}


def cli(name, cfg=None, files=None, args=None, drop=(), stdin=None, env=None, base_args=True, cwd_proj=False):
    """補的 CLI 案例：在新的暫存專案放檔案，跑一次 check。files 的值給 DROP 表示不放這個檔；drop 是要拿掉的預設參數。"""
    with tempfile.TemporaryDirectory(prefix="awf-gates-") as d:
        proj = Path(d)
        (proj / ".claude/teams").mkdir(parents=True)
        base = {".claude/teams/guard.config.json": BASE_CFG if cfg is None else cfg, "reqs.json": REQS,
                "decision.json": envelope(), "evidence.json": {"commit": SHA, "results": GOOD_EV},
                "checks.json": [{"name": "ci", "state": "SUCCESS"}], "diff.txt": TEST_DIFF, **(files or {})}
        for fn, body in base.items():
            if body is DROP:
                continue
            f = proj / fn
            f.parent.mkdir(parents=True, exist_ok=True)
            if isinstance(body, bytes):
                f.write_bytes(body)
            else:
                f.write_text(body if isinstance(body, str) else dump(body), encoding="utf-8", newline="")
        pairs = [("--project-dir", d), ("--decision", f"{d}/decision.json"), ("--repo", "acme-fork/app"), ("--pr", "3"),
                 ("--head-sha", SHA), ("--evidence", f"{d}/evidence.json"), ("--checks", f"{d}/checks.json"),
                 ("--diff", f"{d}/diff.txt"), ("--tests-before", "10"), ("--tests-after", "11"), ("--timestamp", TS)]
        argv = ["check"] if base_args else []
        if base_args:
            for k, v in pairs:
                if k not in drop:
                    argv += [k, v]
            if "--no-audit" not in drop:
                argv.append("--no-audit")
        argv += [a.replace("@P@", d) for a in (args or [])]
        kwargs = {"capture_output": True, "text": True, "encoding": "utf-8",
                  "env": {**os.environ, "PYTHONUTF8": "1", **{k: v.replace("@P@", d) for k, v in (env or {}).items()}}}
        if stdin is not None:
            kwargs["input"] = stdin
        if cwd_proj:
            kwargs["cwd"] = d
        mg_record(name, d, argv, kwargs, "cli")


def cfg_req(**kw):
    return {**BASE_CFG, "requirements": kw}


def mg_cli_cases():
    ev = lambda items, commit=SHA: {"commit": commit, "results": items}  # noqa: E731
    dec = lambda **kw: {"decision.json": envelope(**kw)}  # noqa: E731
    # ---- 旗標：--reviewed-sha
    cli("reviewed-sha 給兩個、一個不符", args=["--reviewed-sha", SHA, "--reviewed-sha", "c" * 40])
    cli("reviewed-sha 等於 head、沒有 findings", args=["--reviewed-sha", SHA])
    # ---- --merged-today
    for v in ("0", "-1", " 2 ", "1_0", "+1", "", "1.5", "0x2", "２"):
        cli(f"merged-today {v!r}", args=["--merged-today", v] if not v.startswith("-") else [f"--merged-today={v}"])
    # ---- --frozen
    for v in ("", "  ", "main 紅了"):
        cli(f"frozen {v!r}", args=["--frozen", v])
    # ---- --timestamp
    for v in ("not-a-date", "20991024", "2099-W43-6", "2099-10-24", "2099-13-01T00:00:00", "9999-12-31T23:59:59+00:00"):
        cli(f"timestamp {v}", args=["--timestamp", v])
    cli("timestamp 沒給（用系統時間）", drop=("--timestamp",))
    cli("timestamp 沒給、寫稽核紀錄", drop=("--timestamp", "--no-audit"))
    # ---- required checks 與 allowed_skipped_checks
    cli("checks 是物件", files={"checks.json": {"ci": "success", "e2e": "SKIPPED"}})
    cli("checks 是物件、允許清單是字串", cfg=am(allowed_skipped_checks="e2e, lint"), files={"checks.json": {"ci": " Success ", "e2e": "SKIPPED", "lint": "neutral"}})
    cli("checks 用 bucket／status／context", files={"checks.json": [{"context": "ci", "bucket": "pass"}, {"status": "skipping"}, {"name": "x", "status": "pending"}]})
    cli("checks 空清單", files={"checks.json": []})
    cli("checks 是數字", files={"checks.json": "[1, 2]"})
    cli("checks 不是清單也不是物件", files={"checks.json": '"ok"'})
    cli("checks 沒給", drop=("--checks",))
    cli("checks 檔不存在", args=["--checks", "@P@/沒有.json"], drop=("--checks",))
    cli("checks 是壞掉的 JSON", files={"checks.json": '[{"name": "ci", "state": "SUCCESS"},]'})
    cli("checks 包在 ```json 區塊", files={"checks.json": '說明\n```json\n{"ci": "success"}\n```\n'})
    cli("allowed_skipped_checks 是物件", cfg=am(allowed_skipped_checks={"e2e": 1}), files={"checks.json": {"ci": "success", "e2e": "skipped"}})
    cli("allowed_skipped_checks 是數字（Python 丟例外）", cfg=am(allowed_skipped_checks=5), files={"checks.json": {"ci": "success"}})
    # ---- 上游授權
    up = ["--repo", "acme-up/app"]
    for label, auth in (("字串", "示範維護者"), ("物件沒有 expires", {"by": "示範維護者", "link": "https://example.com/acme/1"}),
                        ("expires 是 YYYYMMDD（過期）", {"by": "示範維護者", "expires": "20991023"}),
                        ("expires 是週曆", {"by": "示範維護者", "expires": "2099-W43-6"}),
                        ("expires 帶時間", {"by": "示範維護者", "expires": "2099-10-24T00:00:00Z"}),
                        ("expires 是數字", {"by": "示範維護者", "expires": 20991231}),
                        ("expires 前後有空白", {"by": "示範維護者", "expires": " 2099-10-23 "}),
                        ("by 是數字", {"by": 7}), ("值是清單", ["示範維護者"]), ("值是 null", None), ("值是 true", True),
                        ("by 缺、有 expires", {"expires": "2099-12-31"}), ("expires 是 2 月 30 日", {"by": "x", "expires": "2099-02-30"})):
        cli(f"上游授權：{label}", cfg=am(upstream_authorized_by={"acme-up/app": auth}), files=dec(repo="acme-up/app"), args=up)
    cli("上游授權：upstream_authorized_by 不是物件", cfg=am(upstream_authorized_by="示範維護者"), files=dec(repo="acme-up/app"), args=up)
    cli("上游授權：upstream_repos 是清單", cfg={**am(), "upstream_repos": ["acme-up/app", " ", 3]}, files=dec(repo="acme-up/app"), args=up)
    cli("上游授權：upstream_repos 是佔位符", cfg={**am(), "upstream_repos": "{{UPSTREAM_REPOS}}"}, files=dec(repo="acme-up/app"), args=up)
    cli("上游授權：過期（--timestamp 是舊日期，判定日取系統日期）",
        cfg=am(upstream_authorized_by={"acme-up/app": {"by": "示範維護者", "expires": (TODAY - timedelta(days=1)).isoformat()}}),
        files=dec(repo="acme-up/app"), args=up + ["--timestamp", "2000-01-01T00:00:00+00:00"])
    cli("上游授權：今天到期仍有效（--timestamp 是舊日期）",
        cfg=am(upstream_authorized_by={"acme-up/app": {"by": "示範維護者", "expires": TODAY.isoformat()}}),
        files=dec(repo="acme-up/app"), args=up + ["--timestamp", "2000-01-01T00:00:00+00:00"])
    # ---- diff 的形狀
    ren = ("diff --git a/src/old name.py b/src/new name.py\nsimilarity index 90%\nrename from src/old name.py\nrename to src/new name.py\n"
           "--- a/src/old name.py\n+++ b/src/new name.py\n@@ -1,2 +1,2 @@\n a = 1\n-b = 2\n+b = 3\n")
    cli("diff：改名加修改（檔名有空白）", files={"diff.txt": ren})
    cli("diff：刪除一般檔", files={"diff.txt": "diff --git a/src/x.py b/src/x.py\ndeleted file mode 100644\nindex 1111111..0000000\n--- a/src/x.py\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n"})
    cli("diff：新增檔", files={"diff.txt": "diff --git a/docs/a.md b/docs/a.md\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/docs/a.md\n@@ -0,0 +1 @@\n+說明\n"})
    cli("diff：二進位檔", files={"diff.txt": "diff --git a/docs/a.png b/docs/a.png\nnew file mode 100644\nindex 0000000..1111111\nBinary files /dev/null and b/docs/a.png differ\n"})
    cli("diff：GIT binary patch", files={"diff.txt": "diff --git a/src/a.bin b/src/a.bin\nindex 1111111..2222222 100644\nGIT binary patch\nliteral 3\nKcmZQzU|?bb0RR910RR91\n\n"})
    cli("diff：只改檔案模式", files={"diff.txt": "diff --git a/src/run.sh b/src/run.sh\nold mode 100644\nnew mode 100755\n"})
    cli("diff：非 ASCII 路徑沒加引號", files={"diff.txt": mk_diff("docs/說明.md", ["內容"])})
    cli("diff：加引號的路徑（含 tab 與 octal）", files={"diff.txt": 'diff --git "a/docs/a\\tb.md" "b/docs/a\\tb.md"\n--- "a/docs/a\\tb.md"\n+++ "b/docs/a\\tb.md"\n@@ -0,0 +1 @@\n+x\n'})
    cli("diff：引號路徑的跳脫寫壞", files={"diff.txt": 'diff --git "a/docs/a\\qb.md" "b/docs/a\\qb.md"\n--- "a/docs/a\\qb.md"\n+++ "b/docs/a\\qb.md"\n@@ -0,0 +1 @@\n+x\n'})
    cli("diff：hunk 標頭解析不了", files={"diff.txt": "diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -x,1 +1 @@\n+x\n"})
    cli("diff：行數比標頭多", files={"diff.txt": TEST_DIFF + "+多出來的一行\n"})
    cli("diff：--src-prefix 不是 a/", files={"diff.txt": "diff --git i/src/a.py w/src/a.py\n--- i/src/a.py\n+++ w/src/a.py\n@@ -0,0 +1 @@\n+x\n"})
    cli("diff：沒有前綴（--no-prefix）", files={"diff.txt": "diff --git src/a.py src/a.py\n--- src/a.py\n+++ src/a.py\n@@ -0,0 +1 @@\n+x\n"})
    cli("diff：CRLF 內容", files={"diff.txt": "diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -0,0 +1,2 @@\n+x = 1\r\n+y = 2\r\n"})
    cli("diff：內容有落單的 CR", files={"diff.txt": "diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -0,0 +1 @@\n+x = 1\ry = 2\n"})
    cli("diff：不是 UTF-8", files={"diff.txt": b"diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -0,0 +1 @@\n+x = '\xff\xfe'\n"})
    cli("diff：沒有換行結尾的標記", files={"diff.txt": "diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n"})
    cli("diff：檔不存在", args=["--diff", "@P@/沒有.diff"], drop=("--diff",))
    cli("diff：沒給", drop=("--diff",))
    cli("diff：是目錄", args=["--diff", "@P@/.claude"], drop=("--diff",))
    cli("diff：hunk 標頭用全形數字（Python 收、另一個實作只會更嚴）", files={"diff.txt": "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -０,０ +１ @@\n+x = 1\n"})
    cli("diff：改名後路徑是空的引號（Python 丟 KeyError）", files={"diff.txt": 'diff --git a/src/a.py b/src/b.py\nrename from src/a.py\nrename to ""\n--- a/src/a.py\n+++ b/src/b.py\n@@ -0,0 +1 @@\n+x\n@@ -0,0 +1 @@\n+y\n'})
    # ---- 風險等級與設定
    cli("風險：文件（low）", files={"diff.txt": mk_diff("docs/guide.md", ["說明"])})
    cli("風險：文件與測試混在一起（medium）", files={"diff.txt": mk_diff("docs/guide.md", ["說明"]) + TEST_DIFF})
    cli("風險：small_lines 設成 1（測試檔 2 行變 medium）", cfg=am(small_lines=1))
    cli("風險：large_lines 設成字串 '1'", cfg=am(large_lines="1"), files={"diff.txt": SRC_DIFF})
    cli("風險：large_files 設成 1、改兩個檔", cfg=am(large_files=1), files={"diff.txt": SRC_DIFF + TEST_DIFF})
    cli("風險：large_lines 不是數字（Python 丟例外）", cfg=am(large_lines="很多"), files={"diff.txt": SRC_DIFF})
    cli("風險：repo 上限寫錯字（當成 low）", cfg=am(repos={"acme-fork/app": "urgent"}), files={"diff.txt": SRC_DIFF})
    cli("風險：repo 上限 high、改 CI", cfg=am(repos={"acme-fork/app": "high"}),
        files={"diff.txt": mk_diff(".github/workflows/ci.yml", ["on: push"])})
    cli("風險：repos 不是物件", cfg=am(repos=["acme-fork/app"]))
    cli("風險：auto_merge 不是物件", cfg={**BASE_CFG, "auto_merge": True})
    cli("風險：沒有 guard.config.json", files={".claude/teams/guard.config.json": DROP}, args=["--requirements", "@P@/reqs.json"])
    cli("風險：guard.config.json 壞掉", files={".claude/teams/guard.config.json": "{壞掉"}, args=["--requirements", "@P@/reqs.json"])
    cli("放寬閘門：改 tsconfig 與 .eslintrc", files={"diff.txt": mk_diff("tsconfig.build.json", ["{}"]) + mk_diff("web/.eslintrc.cjs", ["x"])})
    cli("放寬閘門：新增 @ts-ignore 與 nolint", files={"diff.txt": mk_diff("src/a.ts", ["// @ts-ignore", "x() //nolint", "y() // NOLINT(foo)"])})
    cli("放寬閘門：壓制註解超過五個只列五個", files={"diff.txt": mk_diff("src/a.py", [f"x{i} = 1  # noqa" for i in range(7)])})
    cli("skip：describe.skip、xit、t.Skip", files={"diff.txt": mk_diff("tests/a.test.js", ["describe.skip('x', () => {})", "xit ('y')", "  t.Skipf(\"z\")"])})
    cli("skip 與壓制同時出現（hold 優先）", files={"diff.txt": mk_diff("tests/test_a.py", ["@pytest.mark.xfail", "import os  # type: ignore"])})
    # ---- 每日上限
    for label, cap in (("0", 0), ("字串 3", "3"), ("字串 abc", "abc"), ("null", None), ("空字串", ""), ("1.9", 1.9), ("true", True), ("清單", [1]), ("負數", -1)):
        cli(f"每日上限：daily_cap {label}", cfg=am(daily_cap=cap), args=["--merged-today", "1"])
    audit = "_tpm/runs/merge-audit.jsonl"
    lines = [dump({"timestamp": "2099-10-24T01:00:00+00:00", "decision": "merge"}), "不是 JSON", "[1, 2]", "",
             dump({"timestamp": "2099-10-23T01:00:00+00:00", "decision": "merge"}), dump({"timestamp": "2099-10-24", "decision": "hold"}),
             dump({"timestamp": 20991024, "decision": "merge"}), '{"timestamp": "2099-10-24T05:00:00+00:00", "decision": "merge", "x": NaN}']
    cli("每日上限：稽核紀錄有壞行、別天、hold、NaN", files={audit: "\n".join(lines) + "\n"})
    cli("每日上限：稽核紀錄只有一筆今天的 merge", files={audit: lines[0] + "\n"})
    cli("每日上限：稽核紀錄不是 UTF-8（Python 丟例外）", files={audit: b"\xff\xfe\n"})
    cli("每日上限：--runs-dir 指到別處（看不到原本的紀錄）", files={audit: lines[0] + "\n" + lines[0] + "\n"}, args=["--runs-dir", "other/runs"])
    cli("每日上限：--runs-dir 是絕對路徑、寫稽核紀錄", args=["--runs-dir", "@P@/abs-runs/"], drop=("--no-audit",))
    cli("稽核紀錄：hold 也寫一行", args=["--tests-before", "99"], drop=("--no-audit",))
    cli("稽核紀錄：runs_dir 是佔位符（用預設）", cfg={**BASE_CFG, "runs_dir": "{{RUNS_DIR}}"}, drop=("--no-audit",))
    cli("稽核紀錄：runs_dir 前後有斜線與空白", cfg={**BASE_CFG, "runs_dir": " /logs/runs/ "}, drop=("--no-audit",))
    cli("runs_dir 是數字（Python 丟例外）", cfg={**BASE_CFG, "runs_dir": 5})
    # ---- 凍結旗標
    frz = "_tpm/runs/auto-merge.freeze.json"
    for label, body in (("frozen 是字串 false", '{"frozen": "false"}'), ("是清單", "[]"), ("沒有 frozen 鍵", '{"reason": "看一下"}'),
                        ("frozen 是 0", '{"frozen": 0}'), ("reason 是 null", '{"frozen": true, "reason": null}'), ("空檔", "")):
        cli(f"凍結旗標：{label}", files={frz: body})
    cli("凍結旗標：freeze_file 是絕對路徑", cfg=am(freeze_file="@ABS@"), files={"flag.json": "{}"})
    cli("凍結旗標：freeze_file 是佔位符（用預設路徑）", cfg=am(freeze_file="{{FREEZE}}"), files={frz: "{}"})
    cli("凍結旗標：路徑是目錄", cfg=am(freeze_file=".claude"))
    cli("凍結旗標與 --frozen 同時", files={frz: '{"frozen": true, "reason": "main 紅了"}'}, args=["--frozen", "repo variable"])
    # ---- 需求來源
    cli("需求：json 物件帶 requirements", files={"reqs.json": {"requirements": REQS}})
    cli("需求：id 重複", files={"reqs.json": REQS + [REQS[0]]})
    cli("需求：缺 id", files={"reqs.json": [REQS[0], {"text": "沒有 id"}]})
    cli("需求：項目不是物件", files={"reqs.json": ["REQ-1"]})
    cli("需求：不是清單", files={"reqs.json": {"items": []}})
    cli("需求：id 是數字、text 缺", files={"reqs.json": [{"id": 1}, {"id": " REQ-2 ", "text": None, "source": 0}]},
        **{"cfg": None}, args=[])
    cli("需求：檔不存在", cfg=cfg_req(source="json", path="沒有.json"))
    cli("需求：JSON 壞掉", files={"reqs.json": '[{"id": "REQ-1" "text": 1}]'})
    cli("需求：JSON 後面有多餘的字", files={"reqs.json": "[]\n多餘"})
    cli("需求：不認得的來源", cfg=cfg_req(source="xml", path="reqs.json"))
    cli("需求：來源是數字", cfg=cfg_req(source=5, path="reqs.json"))
    cli("需求：沒有 path 也沒有 --requirements", cfg=cfg_req(source="json"))
    cli("需求：path 是佔位符", cfg=cfg_req(source="json", path="{{REQUIREMENTS_PATH}}"))
    cli("需求：--requirements 覆蓋（.json 自動判斷）", cfg={**BASE_CFG, "requirements": {}}, args=["--requirements", "reqs.json"])
    cli("需求：--requirements 是 .md（spec-md）", cfg={**BASE_CFG, "requirements": {}}, files={"spec.md": "- REQ-1 匯出\n- REQ-2: 提示\n"}, args=["--requirements", "spec.md"])
    cli("需求：--requirements-source 覆蓋設定", cfg=cfg_req(source="spec-md", path="reqs.json"), args=["--requirements-source", "json"])
    cli("需求：--requirements 是絕對路徑", args=["--requirements", "@P@/reqs.json"])
    spec = "# 需求\n\n| REQ-1 | 匯出 |\n1. **REQ-2**：提示\n### `REQ-3` 另一條\n[REQ-4] 第四條\nREQ-1 重複不算\nreq-5 小寫不算\n- AC12、第五條\n"
    rows5 = [row(i) for i in ("REQ-1", "REQ-2", "REQ-3", "REQ-4", "AC12")]
    ev5 = ev([{"requirement_id": r["requirement_id"], "result": "pass", "evidence": "ok"} for r in rows5])
    cli("需求：spec-md 預設樣式的各種寫法", cfg=cfg_req(source="spec-md", path="spec.md"), files={"spec.md": spec, "decision.json": envelope(rows=rows5), "evidence.json": ev5})
    cli("需求：spec-md 自訂 id_pattern", cfg=cfg_req(source="spec-md", path="spec.md", id_pattern=r"^## (S\d+)"),
        files={"spec.md": "## S1 匯出\n內容\n## S2：提示\n", "decision.json": envelope(rows=[row("S1"), row("S2")]),
               "evidence.json": ev([{"requirement_id": "S1", "result": "pass"}, {"requirement_id": "S2", "result": "pass"}])})
    cli("需求：spec-md 找不到任何條目", cfg=cfg_req(source="spec-md", path="spec.md"), files={"spec.md": "沒有條目\n"})
    cli("需求：spec-md 檔不存在", cfg=cfg_req(source="spec-md", path="specs/沒有.md"))
    cli("需求：spec-md 的 id_pattern 寫壞（Python 丟例外）", cfg=cfg_req(source="spec-md", path="spec.md", id_pattern="("), files={"spec.md": "x\n"})
    one = [row("R-1")]
    ev1 = ev([{"requirement_id": "R-1", "result": "pass", "evidence": "ok"}])
    sh = {"decision.json": envelope(rows=one), "evidence.json": ev1}
    cli("需求：command 是字串（shell）", cfg=cfg_req(source="command", command='echo "[{\\"id\\": \\"R-1\\", \\"text\\": \\"$AWF_REPO#$AWF_PR@$AWF_HEAD_SHA\\"}]"'), files=sh)
    cli("需求：command 失敗", cfg=cfg_req(source="command", command="echo 壞了 >&2; exit 3"), files=sh)
    cli("需求：command 輸出不是 JSON", cfg=cfg_req(source="command", command="echo hello"), files=sh)
    cli("需求：command 沒設定", cfg=cfg_req(source="command"), files=sh)
    cli("需求：command 找不到程式", cfg=cfg_req(source="command", command=["/nonexistent/awf-gen-reqs"]), files=sh)
    cli("需求：command 在專案根執行", cfg=cfg_req(source="command", command="cat reqs.json"))
    cli("需求：取自 methodology.json", cfg={**BASE_CFG, "requirements": {"source": "json", "path": ""}},
        files={".claude/teams/methodology.json": {"requirements": {"source": "json", "path": "reqs.json"}}})
    cli("需求：methodology.json 壞掉（當成沒有）", cfg={**BASE_CFG, "requirements": {}}, files={".claude/teams/methodology.json": "{壞"})
    # ---- 驗證證據
    cli("證據：criteria 信封", files={"evidence.json": {"status": "done", "payload": {"commit": SHA, "anti_cheat": [], "criteria": [{"id": r["id"], "result": "pass", "evidence": "ok"} for r in REQS]}}})
    cli("證據：results 是 null（不看 criteria）", files={"evidence.json": {"commit": SHA, "results": None, "criteria": GOOD_EV}})
    cli("證據：項目不是物件", files={"evidence.json": ev(["REQ-1"])})
    cli("證據：缺 requirement_id", files={"evidence.json": ev([{"result": "pass"}])})
    cli("證據：空清單", files={"evidence.json": ev([])})
    cli("證據：not_verifiable 與重複條目", files={"evidence.json": ev(GOOD_EV + [{"requirement_id": "REQ-2", "result": "not_verifiable", "evidence": "環境沒有"}, {"requirement_id": "REQ-9", "result": "fail"}])})
    cli("證據：commit 是數字", files={"evidence.json": {"commit": 123, "results": GOOD_EV}})
    cli("證據：anti_cheat 是字串", files={"evidence.json": {"commit": SHA, "anti_cheat": "斷言被放寬了很多地方", "results": GOOD_EV}})
    cli("證據：anti_cheat 是物件（Python 丟例外）", files={"evidence.json": {"commit": SHA, "anti_cheat": {"a": 1}, "results": GOOD_EV}})
    cli("證據：沒給", drop=("--evidence",))
    cli("證據：沒給、對照表有 mismatch", drop=("--evidence",), files=dec(rows=[row("REQ-1"), row("REQ-2", result="mismatch")], dec="hold",
                                                              followups=[{"requirement_id": "REQ-2", "expected": "a", "actual": "b", "adjustment": "c"}]))
    # ---- steward 的 merge-decision
    cli("decision：只有 payload（不是信封）", files={"decision.json": payload()})
    cli("decision：payload 缺欄位", files={"decision.json": {"decision": "merge", "repo": "acme-fork/app", "pr": 3}})
    cli("decision：信封缺欄位", files={"decision.json": {"payload": payload()}})
    cli("decision：repo 與 pr 不同", files=dec(repo="acme-fork/other", pr=4))
    cli("decision：pr 是 3.0", files=dec(pr=3.0))
    cli("decision：包在 ```json 區塊的回報文字", files={"decision.json": "合併 agent 的回報：\n```json\n" + dump(envelope()) + "\n```\n"})
    cli("decision：回報文字裡沒有 JSON", files={"decision.json": "我覺得可以合併"})
    cli("decision：null", files={"decision.json": "null"})
    cli("decision：從 stdin", args=["--decision", "-"], drop=("--decision",), stdin=dump(envelope()))
    cli("decision：沒給", drop=("--decision",))
    cli("decision：檔不存在", args=["--decision", "@P@/沒有.json"], drop=("--decision",))
    cli("decision：JSON 壞掉", files={"decision.json": '{"payload": {"decision": "merge",}}'})
    cli("decision：是清單", files={"decision.json": "[1]"})
    cli("decision：不是 UTF-8（Python 丟例外）", files={"decision.json": b'{"payload": "\xff"}'})
    cli("decision：needs_sync 卻判 merge", files=dec(needs_sync=True))
    cli("decision：對照表同一條出現兩次、後面那筆是 mismatch", files=dec(rows=[row("REQ-1"), row("REQ-2"), row("REQ-2", result="mismatch")]))
    cli("decision：對照表的 requirement_id 是數字與 null", files=dec(rows=[row("REQ-1"), row("REQ-2"), {**row("x"), "requirement_id": 7}, {**row("y"), "requirement_id": None}]))
    cli("decision：out_of_scope 是字串", files=dec(oos="src/billing.py"))
    cli("decision：followups 已有同一條", files={"decision.json": envelope(rows=[row("REQ-1")], dec="hold", followups=[{"requirement_id": "REQ-2", "expected": "a", "actual": "b", "adjustment": "c"}])})
    cli("decision：head_sha 前後有空白的 --head-sha", args=["--head-sha", f" {SHA} "])
    cli("decision：--head-sha 是空字串", args=["--head-sha", ""])
    # ---- 審查發現
    f1 = {"lens": "bugs", "findings": [{"id": "B1", "severity": "blocker"}, {"id": "N1", "severity": "nit"}, "不是物件"]}
    f2 = {"status": "done", "payload": {"findings": [{"severity": "important"}]}}
    cli("findings：兩個檔（blocker 與信封裡的 important）", files={"f1.json": f1, "f2.json": f2},
        args=["--findings", "@P@/f1.json", "--findings", "@P@/f2.json", "--reviewed-sha", SHA])
    cli("findings：檔不存在", args=["--findings", "@P@/沒有.json", "--reviewed-sha", SHA])
    cli("findings：是清單", files={"f1.json": [{"id": "B2", "severity": "important"}]}, args=["--findings", "@P@/f1.json", "--reviewed-sha", SHA])
    cli("findings：不是清單也不是物件", files={"f1.json": '"沒事"'}, args=["--findings", "@P@/f1.json", "--reviewed-sha", SHA])
    # ---- 測試數
    cli("測試數：沒給 before", drop=("--tests-before",))
    cli("測試數：沒給 after", drop=("--tests-after",))
    cli("測試數：都沒給", drop=("--tests-before", "--tests-after"))
    cli("測試數：相等", args=["--tests-after", "10"])
    cli("測試數：負數", args=["--tests-before", "-5", "--tests-after", "-6"])
    # ---- recipe
    bi = {"recipes": rcp.BUILTIN_RECIPES, "modifiers": rcp.BUILTIN_MODIFIERS}

    def run_json(name, mods=(), locked=True, **patch):
        return {"stages": {}, "recipe": {"name": name, "modifiers": list(mods), "size": "", "locked": locked,
                                         "resolved": {**rcp.resolve(bi, name, list(mods)), **patch}}}
    meth = {".claude/teams/methodology.json": bi}
    cli("recipe：feature 加 security modifier、改 manifest", files={**meth, "run.json": run_json("feature", ["security"]), "diff.txt": mk_diff("src/a.py", ["x = 1"]) + mk_diff("package.json", ["{}"])},
        args=["--recipe", "@P@/run.json"])
    cli("recipe：bugfix 改 manifest（要安全審查）", files={**meth, "run.json": run_json("bugfix"), "diff.txt": mk_diff("package.json", ["{}"])}, args=["--recipe", "@P@/run.json"])
    cli("recipe：專案沒有 methodology.json（用工具旁的，沒有就當內建節）", files={"run.json": run_json("docs"), "diff.txt": mk_diff("docs/a.md", ["x"])}, args=["--recipe", "@P@/run.json"])
    cli("recipe：methodology.json 壞掉", files={".claude/teams/methodology.json": "{壞", "run.json": run_json("docs")}, args=["--recipe", "@P@/run.json"],
        cfg={**BASE_CFG})
    cli("recipe：methodology.json 不是物件", files={".claude/teams/methodology.json": "[]", "run.json": run_json("docs")}, args=["--recipe", "@P@/run.json"])
    cli("recipe：resolved 少了 design_lenses 的段（被改鬆）", files={**meth, "run.json": run_json("feature", stages=["plan", "build", "deliver", "merge"]), "diff.txt": SRC_DIFF}, args=["--recipe", "@P@/run.json"])
    cli("recipe：publishable 被改成 true", files={**meth, "run.json": run_json("spike", publishable=True), "diff.txt": mk_diff("docs/a.md", ["x"])}, args=["--recipe", "@P@/run.json"])
    cli("recipe：不能推送的 recipe（spike）", files={**meth, "run.json": run_json("spike"), "diff.txt": mk_diff("docs/a.md", ["x"])}, args=["--recipe", "@P@/run.json"])
    cli("recipe：名稱不存在（重算成 feature）", files={**meth, "run.json": {"stages": {}, "recipe": {"name": "nope", "locked": True, "resolved": rcp.resolve(bi, "docs")}}}, args=["--recipe", "@P@/run.json"])
    cli("recipe：resolved 的 stages 不是清單", files={**meth, "run.json": run_json("docs", stages="plan")}, args=["--recipe", "@P@/run.json"])
    cli("recipe：相符並寫稽核紀錄（輸出有 recipe 雜湊）", files={**meth, "run.json": run_json("bugfix"), "diff.txt": SRC_DIFF}, args=["--recipe", "@P@/run.json"], drop=("--no-audit",))
    # ---- 專案目錄
    cli("專案目錄：用 CLAUDE_PROJECT_DIR", drop=("--project-dir",), env={"CLAUDE_PROJECT_DIR": "@P@"})
    cli("專案目錄：相對路徑的輸入檔（cwd 在專案）", drop=("--project-dir", "--decision"), args=["--project-dir", ".", "--decision", "./decision.json"], cwd_proj=True)
    cli("專案目錄：--project-dir 不存在", drop=("--project-dir",), args=["--project-dir", "@P@/沒有/../這個目錄"])
    # ---- 參數錯誤
    cli("參數：沒有子指令", base_args=False, args=[])
    cli("參數：不認得的子指令", base_args=False, args=["verify"])
    cli("參數：缺必要參數", base_args=False, args=["check", "--repo", "acme-fork/app"])
    cli("參數：--pr 不是整數", args=["--pr", "三"])
    cli("參數：--tests-before 不是整數", args=["--tests-before", "1.5"])
    cli("參數：不認得的參數", args=["--force", "yes"])
    cli("參數：--requirements-source 不在選項裡", args=["--requirements-source", "xml"])
    cli("參數：前綴有歧義", args=["--re", "x"])
    cli("參數：前綴唯一（--head、--time=）", drop=("--head-sha", "--timestamp"), args=["--head", SHA, f"--time={TS}"])
    cli("參數：選項缺值", args=["--frozen"])
    cli("參數：--no-audit 帶值", args=["--no-audit=1"])
    cli("參數：-h", base_args=False, args=["check", "-h"])


def fix_abs(cases):
    """freeze_file 絕對路徑的案例：設定檔裡的 @ABS@ 在執行前要換成暫存目錄；錄的時候直接重做一次。"""


# ------------------------------------------------------------------ merge gate：純函式
mg_probes = []


def probe(fn, args, call):
    rec = {"fn": fn, "args": args}
    try:
        rec["out"] = call()
    except Exception as e:  # noqa: BLE001
        rec["raise"] = type(e).__name__
    mg_probes.append(rec)


def mg_function_probes():
    q = '"a/.github/\\346\\270\\254\\350\\251\\246.yml" "b/docs/\\346\\270\\254\\350\\251\\246.md"'
    diffs = [
        "", "\n", "不是 diff\n", TEST_DIFF, SRC_DIFF + TEST_DIFF, TEST_DIFF.replace("+1,2 @@", "+1,5 @@"), TEST_DIFF[:-1],
        f"diff --git {q}\nsimilarity index 100%\nrename from \".github/\\346\\270\\254\\350\\251\\246.yml\"\nrename to \"docs/\\346\\270\\254\\350\\251\\246.md\"\n",
        "diff --git a/a b.txt b/a b.txt\n--- a/a b.txt\n+++ b/a b.txt\n@@ -1 +1 @@\n-a\n+b\n",
        "diff --git a/x b/y b/x b/y\n--- a/x b/y\n+++ b/x b/y\n@@ -1 +1 @@\n-a\n+b\n",
        "diff --git a/x b/y.txt b/z.txt\nrename from x b/y.txt\nrename to z.txt\n",
        "diff --git a/x.txt b/y b/z.txt\nrename from x.txt\nrename to y b/z.txt\n",
        'diff --git a/plain.txt "b/quo\\"ted.txt"\nrename from plain.txt\nrename to "quo\\"ted.txt"\n',
        'diff --git "a/q\\\\.txt" b/plain.txt\ncopy from "q\\\\.txt"\ncopy to plain.txt\n',
        'diff --git "a/x.txt" "b/x.txt" extra\n', 'diff --git "a/x.txt"\n', 'diff --git "a/x.txt" \n', "diff --git a/ b/\n", "diff --git \n",
        "diff --git a/x.txt b/x.txt\n--- a/x.txt\t2026-01-01\n+++ b/x.txt\t2026-01-02\n@@ -1 +1 @@\n-a\n+b\n",
        "diff --git a/x.txt b/x.txt\n--- x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+b\n",
        "diff --git a/x.txt b/x.txt\n--- a/x.txt\n+++ x.txt\n@@ -1 +1 @@\n-a\n+b\n",
        "--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+b\n", "--- /dev/null\n+++ /dev/null\n", "+++ b/x.txt\n@@ -0,0 +1 @@\n+a\n",
        "--- /dev/null\n+++ /dev/null\n@@ -0,0 +1 @@\n+a\n",
        "+孤單的新增行\n", "@@ -1 +1 @@\n+a\n", "diff --git a/x b/x\n@@ -1 +1 @@\n-a\n+b\n", "diff --git a/x b/x\n@@ -1,0 +1,0 @@\n+b\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@ def f():\n a\n\n-b\n+c\n", "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n?b\n+c\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-a\n+++ b/假的標頭\n+--- 內容\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -2 +1,0 @@\n--- 被刪掉的內容\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+hunk 外的新增行\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -99999999999999999999999 +1 @@\n+b\n", "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,99999999999999999999999 +1 @@\n+b\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@ \n-a\n+b\n\x0c+換頁字元不是換行\n",
        "diff --git a/x b/x\nBinary files a/x and b/x differ\ndiff --git a/y b/y\nnew file mode 100644\ndiff --git a/z b/z\ndeleted file mode 100644\n",
        "new file mode 100644\nBinary files a/x and b/x differ\n", 'diff --git a/x b/x\nrename from \nrename to "x\n',
        'diff --git a/x b/x\nrename to ""\n+a\n', "diff --git a/x b/x\ncopy from a\tb\ncopy to c\n",
        "diff --git a/a\\b b/a\\b\n--- a/a\\b\n+++ b/a\\b\n@@ -0,0 +1 @@\n+x\n", 'diff --git "a/\\377\\376" "b/\\377\\376"\n', 'diff --git "a/\\400" "b/\\400"\n',
        'diff --git "a/CODEOWNERS\\n" "b/CODEOWNERS\\n"\n', "diff --git a/一 b/一\n--- a/一\n+++ b/一\n@@ -0,0 +1 @@\n+" + "長" * 300 + "\n",
        "diff --git " + "x" * 300 + "\n", "diff --git a/x b/x\n--- " + '"' + "y" * 300 + "\n",
    ]
    for t in diffs:
        probe("parse_diff", [t], lambda t=t: merge_gate.parse_diff(t))
    paths = [
        ".github/workflows/ci.yml", "sub/.github/x", "x.github/y", ".gitlab-ci.yml", "ci/Jenkinsfile", "Jenkinsfile.bak", ".circleci/config.yml",
        ".buildkite/p.yml", ".claude/settings.json", "a/.claude-plugin/plugin.json", ".husky/pre-commit", ".githooks/x", "hooks/a.py", "plugin/hooks/a.py",
        "src/hooks/a.py", ".pre-commit-config.yaml", "tools/merge_gate.py", "docs/merge_gate.md", "guard.config.json", "x/guard.config.json.bak",
        "release/notes.md", "releases.txt", "src/release-notes/a", "prerelease/a", ".releaserc.json", ".goreleaser.yaml", ".goreleaser.yml", ".release-please.json",
        "VERSION", "pkg/version.py", "pkg/_version.py", "version.txt", "VERSION.md", "aversion.py", "package-lock.json", "yarn.lock", "bun.lockb", "bun.lock",
        "x/poetry.lock", "go.sum", "foo.lock", "foo.locked", "package.json", "pyproject.toml", "requirements-dev.txt", "requirements/dev.txt", "go.mod",
        "Cargo.toml", "build.gradle", "build.gradle.kts", "pom.xml", "Pipfile", "db/migrations/001.sql", "migration/a.py", "migrations.py", "CODEOWNERS",
        "docs/CODEOWNERS", "CODEOWNERS.md", "CODEOWNERS\n", "src/auth/login.py", "src/auth.py", "src/oauth.ts", "src/authz/x", "src/user_auth.py",
        "src/user-oauth.go", "src/author.py", "src/auth.Py", "src/auth.py\n", "jest.config.js", "vitest.config.mts", "karma.conf.js", "pytest.ini", "tox.ini",
        ".coveragerc", "codecov.yml", "codecov.yaml", "mypy.ini", ".flake8", "ruff.toml", ".ruff.toml", ".rubocop.yml", ".nycrc", ".nycrc.json", ".eslintrc",
        ".eslintrc.cjs", "eslint.config.mjs", "tsconfig.json", "tsconfig.build.json", "web/tsconfig.json", ".golangci.yml", "phpunit.xml", "phpunit.xml.dist",
        ".stylelintrc.json", "playwright.config.ts", "cypress.config.js", "docs/a.md", "doc/a.png", "README.MD", "notes.txt", "a.rst", "a.adoc", "a.mdx",
        "a.md\n", "docs", "mydocs/a.py", "tests/test_a.py", "test/a.py", "src/__tests__/a.js", "spec/a_spec.rb", "e2e/a.ts", "test_a.py", "src/test_a.py",
        "a_test.go", "a.test.tsx", "a.spec.ts", "A_TEST.GO", "Tests/A.py", "src/a.py", "contest/a.py", "latest/a.py", "a_test.go\n", "src/a.PY", "ſrc/a.ſpec.tſ",
        "a.spec.tK", "İ.md", "說明.MD",
    ]
    for p in paths:
        probe("path_kind", [p], lambda p=p: [next((what for pat, what in merge_gate.SENSITIVE if re.search(pat, p)), None),
                                             bool(merge_gate.DOC.search(p)), bool(merge_gate.TEST.search(p))])
    tiers = [({"docs/a.md": [1, 0]}, {}), ({"docs/a.md": [100, 0]}, {}), ({"docs/a.md": [100, 1]}, {}), ({"docs/a.md": [1, 0], "tests/a.py": [1, 0]}, {}),
             ({"tests/a.py": [50, 50], "test_b.py": [0, 0]}, {}), ({}, {}), ({"src/a.py": [400, 0]}, {}), ({"src/a.py": [400, 1]}, {}),
             ({f"src/{i}.py": [0, 0] for i in range(20)}, {}), ({f"src/{i}.py": [0, 0] for i in range(21)}, {}),
             ({"src/a.py": [3, 0]}, {"large_lines": 2}), ({"src/a.py": [3, 0]}, {"large_lines": "2"}), ({"src/a.py": [3, 0]}, {"large_lines": 2.9}),
             ({"src/a.py": [3, 0]}, {"large_lines": True}), ({"src/a.py": [3, 0]}, {"large_lines": " 3 "}), ({"src/a.py": [3, 0]}, {"large_lines": []}),
             ({"src/a.py": [3, 0]}, {"large_lines": [3]}), ({"src/a.py": [3, 0]}, {"large_lines": "x"}), ({"src/a.py": [3, 0]}, {"small_lines": "x"}),
             ({"src/a.py": [3, 0]}, {"large_files": "x"}), ({"src/a.py": [500, 0]}, {"large_files": "x"}), ({".github/a": [1, 0]}, {"large_files": "x"}),
             ({"docs/a.md": [3, 0]}, {"small_lines": 2}), ({"docs/a.md": [3, 0]}, {"small_lines": 0}), ({"docs/a.md": [3, 0]}, {"small_lines": -1}),
             ({"src/a.py": [1, 0], "b/c": [1, 0]}, {"large_files": 1}), ({".github/a": [1, 0], "CODEOWNERS": [0, 0], "src/x": [9, 9]}, {})]
    for files, cfg in tiers:
        probe("risk_tier", [files, cfg], lambda files=files, cfg=cfg: list(merge_gate.risk_tier({"files": files}, cfg)))
    lines = [
        "it.skip('x')", "test.only(", "describe.fixme", "context.todo", "suite.skip", "it.skipped", "submit.skip", "it .skip", "xit('a')", "xit ('a')",
        "exit('a')", "xdescribe(", "fit(", "outfit(", "profit (", "fdescribe\t(", "xtest(", "@pytest.mark.skip", "@pytest.mark.skipif(x)", "@pytest.mark.skipper",
        "@pytest.mark.xfail", "pytest.skip('x')", "pytest.skip", "mypytest.skip(", "pytest.xfail (", "@unittest.skip('x')", "@unittest.skipIf", "@unittest.skipUnless(",
        "@unittest.expectedFailure", "@unittest.skipTest", "t.Skip()", "t.SkipNow()", "t.Skipf(\"x\")", "t.Skipped()", "it.Skip(", "@Disabled", "@Disabled(\"x\")",
        "@DisabledOnOs", "@Ignore", "@IgnoreRest", "test.fixme", "contest.fixme", "test.fixme2", "it.skip\u0301", "it.only\u00b2", "\u00e9it.only", "it.only_", "一it.only",
        "it.only一", "xit\u001c(", "xit\u00a0(", "xit\u200b(", "xit\u3000(", "_xit(", "it.skip\n", "def test(): pass", "",
        "// eslint-disable-next-line", "eslint-disabled", "@ts-ignore", "@ts-nocheck", "@ts-expect-error", "@ts-ignored", "x = 1  # type: ignore", "#type:ignore[x]",
        "# type : ignore", "# noqa", "#noqa: E501", "# noqas", "# NOQA", "pragma: no cover", "pragma:no  cover", "pragma: nocover", "/* istanbul ignore next */",
        "/* c8 ignore start */", "istanbulignore", "//nolint", "// nolint:errcheck", "// nolinter", "NOLINT", "NOLINTNEXTLINE", "// NOLINT(x)", "xNOLINT", "@SuppressWarnings(\"x\")",
        "@SuppressWarningsX", "# rubocop:disable Style/X", "rubocop: disable", "#\u001ctype:\u001cignore", "# noqa\u0301", "一NOLINT", "NOLINT一", "x = 1",
    ]
    for l in lines:
        probe("line_kind", [l], lambda l=l: [bool(merge_gate.SKIP.search(l)), bool(merge_gate.SUPPRESS.search(l))])
    auths = [({}, "r", "2099-10-24"), ({"upstream_authorized_by": {"r": "甲"}}, "r", "2099-10-24"), ({"upstream_authorized_by": {"r": " "}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "2099-10-24"}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "2099-10-23"}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "2099-10-23"}}}, "r", "讀不懂的判定日"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "2099/10/25"}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "20991025xx"}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": 0}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": ["甲"]}}}, "r", "2099-10-24"), ({"upstream_authorized_by": {"r": {}}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": 0}}, "r", "2099-10-24"), ({"upstream_authorized_by": {"r": 1.5}}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"other": "甲"}}, "r", "2099-10-24"), ({"upstream_authorized_by": ["r"]}, "r", "2099-10-24"),
             ({"upstream_authorized_by": {"r": {"by": "甲", "expires": "9999-12-31"}}}, "r", "9999-12-31T00:00")]
    for a, repo, today in auths:
        if today.startswith("讀不懂"):
            continue  # 判定日讀不懂時 Python 版改用系統日期，結果會隨錄製日變；不放進基準
        probe("upstream_auth", [a, repo, today], lambda a=a, repo=repo, today=today: list(merge_gate.upstream_auth(a, repo, today)))
    dates = ["2099-10-24", "2099-10-24T09:00:00+00:00", "20991024", "20991024xx", "2099-1024", "209910-24", "2099W43", "2099-W43", "2099-W43-6", "2099W436", "2099-W436",
             "2099W43-6", "2098-W53", "2099-W53-1", "2099-W00", "2099-W54", "2099-W01-0", "2099-W01-8", "9999-W52-7", "0001-W01-1", "0000-01-01", "0001-01-01",
             "9999-12-31", "2099-02-29", "2096-02-29", "2100-02-29", "2000-02-29", "2099-00-10", "2099-13-01", "2099-01-00", "2099-04-31", "2099-1-1", "99-10-24",
             "", "x", "明年", "2099-10-2", " 2099-10-24", "2099-10-24 ", "20991024é", "２０９９-10-24", "2099-10-2４", "+099-10-24", "2099-W1", "2099W1", "2020-W53-7",
             "2026-W53", "2015-W53", "2004-W53-1", "1999-W52-7"]
    for s in dates:
        probe("iso_date", [s], lambda s=s: date.fromisoformat(s[:10]).isoformat())
    texts = ["", " ", "{", "[", "[1", "[1,", "[1,]", "{\"a\"", "{\"a\":", "{\"a\": 1", "{\"a\": 1,", "{\"a\": 1,}", "{a: 1}", "{'a': 1}", "[1 2]", "{\"a\" 1}", "nul", "tru", "-", "-x",
             "01", "1.", "1.e3", "1e", "1e+", ".5", "+1", "0x10", "[1]x", "[1] [2]", "\"abc", "\"a\\", "\"a\\x\"", "\"a\\u12\"", "\"a\\u12g4\"", "\"a\\ud83d\\u12\"", "\"a\tb\"",
             "\"a\nb\"", "\ufeff[]", "[\n1,\n 2,\n  x]", "{\"a\": [1, {\"b\": }]}", "一", "[\"一\", 二]", "[] ", " \t\r\n[1]\n", "1", "-0", "1.50", "1E3", "1e-2", "-0.0",
             "123456789012345678", "18446744073709551615", "\"\\u00e9\\ud83d\\ude00\\/\\b\\f\\n\\r\\t\\\"\\\\\"", "\"\\ud83d\"", "\"\\ud83dx\"", "{\"a\": 1, \"b\": 2, \"a\": 3}",
             "[true, false, null]", "{\"\": {}}", "[[[[[[1]]]]]]", "[1e400]"]
    for t in texts:
        def call(t=t):
            try:
                v = json.loads(t)
            except json.JSONDecodeError as e:
                return {"err": str(e)}
            return {"ok": dump(v)}
        if t in ("[1e400]", "\"\\ud83d\"", "\"\\ud83dx\"", "\"a\\ud83d\\u12\""):
            continue  # 無限大與落單的代理字元：Python 收、另一個實作不收或換字元，列在差異裡，不放進基準
        probe("json", [t], call)
    for requested in ("auto", "rebase", "merge", "transit"):
        for behind in (0, 3):
            for published in (False, True):
                for conflicts in ([], ["a.txt"], [f"f{i}" for i in range(12)]):
                    def call(requested=requested, behind=behind, published=published, conflicts=conflicts):
                        try:
                            return list(sync_branch.decide(requested, behind, published, conflicts))
                        except sync_branch.Halt as e:
                            return {"halt": str(e)}
                    probe("decide", [requested, behind, published, conflicts], call)


# ------------------------------------------------------------------ sync_branch
sb_steps = []


class SbTap:
    def __init__(self):
        self.root = None
        self.inside = False
        self.real_execute = sync_branch.execute
        self.real_write = Path.write_text

    def n(self, s):
        return norm(s, self.root) if self.root else s

    def run(self, cmd, *args, **kwargs):
        if not self.inside and isinstance(cmd, list) and cmd and cmd[0] == "git":
            if self.root is None:  # 第一個指令是 git init --bare <root>/origin.git
                self.root = str(Path(cmd[-1]).parent)
            sb_steps.append({"run": [self.n(c) for c in cmd]})
        return real_run(cmd, *args, **kwargs)

    def execute(self, argv):
        self.inside = True
        try:
            code, rec = self.real_execute(argv)
        finally:
            self.inside = False
        sb_steps.append(self.case(argv, code, rec))
        return code, rec

    def case(self, argv, code, rec):
        text = self.n(json.dumps(rec, ensure_ascii=False, indent=2))
        step = {"case": [self.n(a) for a in argv], "code": code, "stdout": text}
        if rec.get("halted") and re.search(r"git .* 失敗：|讀不懂：\[Errno", rec.get("reason", "")) or code == 3 or "失敗" in rec.get("reason", ""):
            step["git_text"] = True  # reason 裡有 git 自己的訊息（隨 git 版本變）
        return step

    def write_text(self, path, text, *a, **k):
        if self.root and str(path).startswith(self.root + "/"):
            sb_steps.append({"write": str(path)[len(self.root) + 1:], "text": text})
        elif self.root is None and str(path).endswith("/gitconfig"):
            pass  # selftest 一開始寫的空 gitconfig：重播時由測試自己建
        return self.real_write(path, text, *a, **k)


def sb_all():
    tap = SbTap()
    buf = io.StringIO()
    subprocess.run, sync_branch.execute = tap.run, tap.execute
    Path.write_text = lambda self, text, *a, **k: tap.write_text(self, text, *a, **k)
    saved_cleanup = tempfile.TemporaryDirectory.cleanup
    keep = []
    # selftest 結束時暫存目錄先不要刪：後面補的步驟接著用同一個 repo
    tempfile.TemporaryDirectory.cleanup = lambda self: keep.append(self)
    try:
        with contextlib.redirect_stdout(buf):
            code = sync_branch._selftest()
        selftest = {"code": code, "stdout": buf.getvalue()}
        os.environ["GIT_CONFIG_GLOBAL"] = tap.root + "/gitconfig"  # selftest 結束時還原了環境變數，補回去
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        sb_extra(tap)
    finally:
        subprocess.run, sync_branch.execute, Path.write_text = real_run, tap.real_execute, tap.real_write
        tempfile.TemporaryDirectory.cleanup = saved_cleanup
        os.environ["GIT_CONFIG_GLOBAL"] = EMPTY_GITCONFIG.name
        for t in keep:
            t.cleanup()
    return selftest


def sb_extra(tap):
    """selftest 之後接著做的步驟：參數錯誤、設定檔的各種形狀、apply 的其他分支。"""
    root = Path(tap.root)
    repo = root / "app"
    R = ["--repo", str(repo)]

    def g(*args):
        r = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, encoding="utf-8")
        if r.returncode:
            raise RuntimeError(f"git {args}：{r.stderr}")

    def case(*argv):
        return sync_branch.execute(list(argv))

    def put(rel, body):
        (root / rel).write_text(body if isinstance(body, str) else dump(body), encoding="utf-8")
        return str(root / rel)

    # 參數
    case()
    case("status")
    case("plan", *R, "--branch", "feature/a", "--strategy", "squash")
    case("plan", *R, "--b", "feature/a")
    case("plan", *R, "--branch")
    case("plan", *R, "--branch", "feature/a", "extra", "--what")
    case("--verbose", "plan", *R, "--branch", "feature/a")
    case("apply", *R, "--branch", "feature/a")
    case("apply", *R, "--branch", "feature/a", "--strategy", "auto")
    case("check-transit", *R)
    case("plan", *R, "--branch=feature/up", "--base=main", "--no-fetch")
    case("plan", *R, "--bra", "feature/up", "--base", "main", "--no-f")
    case("plan", *R, "--branch", "feature/up", "--base", "main", "--no-fetch=1")
    # plan：分支、base、日期
    case("plan", *R, "--branch", "feature/沒有", "--no-fetch")
    case("plan", *R, "--branch", "feature/a", "--base", "origin/沒有", "--no-fetch")
    case("plan", "--repo", str(root / "不是repo"), "--branch", "feature/a", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "2026-11-03", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--short", "短名", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--short", "a..b", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--base", "refs/remotes/origin/main", "--no-fetch")
    case("plan", *R, "--branch", "feature/a", "--strategy", "transit", "--date", "20261103", "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--strategy", "rebase", "--no-fetch")
    case("plan", *R, "--branch", "feature/up", "--base", "main", "--strategy", "rebase", "--no-fetch")
    # plan：methodology 與 guard.config 的形狀
    for label, body in (("壞掉", "{壞"), ("是清單", "[]"), ("strategy 不認得", {"conventions": {"sync": {"strategy": "squash"}}}),
                        ("strategy 是 null", {"conventions": {"sync": {"strategy": None}}}), ("sync 是字串", {"conventions": {"sync": "merge"}}),
                        ("sync 是空的", {"conventions": {"sync": {}}}), ("conventions 是清單", {"conventions": ["sync"]}),
                        ("naming 不是字串", {"conventions": {"sync": {"transit_naming": 5}}}),
                        ("naming 有不認得的佔位符", {"conventions": {"sync": {"transit_naming": "merge/<名字>-<YYYYMMDD>"}}}),
                        ("naming 自訂", {"conventions": {"sync": {"strategy": "transit", "transit_naming": "feature/sync-<短名>-<base>-<YYYYMMDD>"}}}),
                        ("naming 沒有前綴", {"conventions": {"sync": {"transit_naming": "sync-<短名>"}}})):
        path = put(f"m-{len(sb_steps)}.json", body)
        case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--methodology", path, "--no-fetch")
    case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--methodology", str(root / "沒有.json"), "--no-fetch")
    for label, body in (("字串清單", {"allowed_branch_prefixes": "feature/, merge/ ,"}), ("佔位符", {"allowed_branch_prefixes": "{{ALLOWED_BRANCH_PREFIXES}}"}),
                        ("混了別的型別", {"allowed_branch_prefixes": ["merge/", 5, " ", "{{X}}"]}), ("壞掉", "不是 JSON"), ("是數字", "5"),
                        ("空清單", {"allowed_branch_prefixes": []}), ("是數字（Python 丟例外）", {"allowed_branch_prefixes": 5})):
        path = put(f"g-{len(sb_steps)}.json", body)
        case("plan", *R, "--branch", "feature/c", "--date", "20261103", "--config", path, "--no-fetch")
    # apply 的其他分支
    g("switch", "-q", "feature/c")
    case("apply", *R, "--branch", "feature/c", "--strategy", "transit")
    case("apply", *R, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", "merge/c-into-main-20261103")
    case("apply", *R, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", "wip/c")
    case("apply", *R, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", "merge/a b")
    case("apply", *R, "--branch", "feature/c", "--strategy", "rebase")
    case("apply", *R, "--branch", "feature/c", "--strategy", "rebase", "--date", "2026")
    case("apply", *R, "--branch", "feature/c", "--strategy", "merge", "--base", "origin/沒有")
    (repo / "髒.txt").write_text("x\n", encoding="utf-8")
    case("apply", *R, "--branch", "feature/c", "--strategy", "merge")
    (repo / "髒.txt").unlink()
    sb_steps.append({"remove": "app/髒.txt"})
    g("switch", "-q", "--detach", "feature/c")
    case("apply", *R, "--branch", "feature/c", "--strategy", "merge")
    # transit 乾淨合併（feature/a 已經 rebase 過，和 origin/main 沒衝突）＋ check-transit
    g("switch", "-q", "feature/a")
    case("apply", *R, "--branch", "feature/a", "--strategy", "transit", "--transit-branch", "merge/a-into-main-20261107")
    case("check-transit", *R, "--branch", "feature/a", "--base", "origin/main", "--expected", "")
    case("check-transit", *R, "--branch", "feature/a", "--base", "origin/main", "--expected", " a.txt , b\\c.txt,,a.txt")
    case("check-transit", *R, "--branch", "feature/沒有", "--base", "origin/main", "--expected", "")
    case("check-transit", *R, "--branch", "feature/a", "--base", "沒有", "--expected", "")
    case("check-transit", *R, "--branch", "feature/b", "--base", "origin/main", "--expected", "")
    # 二進位檔裡的標記不掃；CRLF 的標記要掃
    g("switch", "-q", "-c", "feature/d", "origin/main")
    (repo / "bin.dat").write_bytes(b"\0\n<<<<<<< x\n=======\n")
    sb_steps.append({"write_hex": "app/bin.dat", "hex": b"\0\n<<<<<<< x\n=======\n".hex()})
    (repo / "crlf.txt").write_bytes(b"a\r\n=======\r\nb\r\n")
    sb_steps.append({"write_hex": "app/crlf.txt", "hex": b"a\r\n=======\r\nb\r\n".hex()})
    (repo / "ok.txt").write_text("======= 不是標記\n<<<<<<<也不是\n", encoding="utf-8")
    g("add", "-A")
    g("commit", "-qm", "d")
    case("check-transit", *R, "--branch", "feature/d", "--base", "origin/main", "--expected", "")


def main():
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8")
    mg_selftest_out = mg_selftest()
    n_selftest = len(mg_cases)
    mg_cli_cases()
    mg_function_probes()
    sb_selftest_out = sb_all()
    os.unlink(EMPTY_GITCONFIG.name)
    git_version = real_run(["git", "--version"], capture_output=True, text=True).stdout.strip()
    out = {
        "meta": {"git_version": git_version, "git_env": GIT_ENV, "selftest_cases": n_selftest, "cli_cases": len(mg_cases) - n_selftest,
                 "probes": len(mg_probes), "sb_cases": sum(1 for s in sb_steps if "case" in s)},
        "blobs": dict(sorted(blobs.items())),
        "mg": {"selftest": mg_selftest_out, "git_setup": mg_git_setup, "cases": mg_cases, "probes": mg_probes},
        "sb": {"selftest": sb_selftest_out, "steps": sb_steps},
    }
    # 一筆一行，diff 看得出哪個案例變了
    w = sys.stdout.write
    w("{\n")
    w(f'"meta": {dump(out["meta"])},\n"blobs": {{\n')
    w(",\n".join(f"{dump(k)}: {dump(v)}" for k, v in out["blobs"].items()))
    w('\n},\n"mg": {\n')
    w(f'"selftest": {dump(out["mg"]["selftest"])},\n')
    for key in ("git_setup", "cases", "probes"):
        w(f'"{key}": [\n' + ",\n".join(dump(r) for r in out["mg"][key]) + ("\n],\n" if key != "probes" else "\n]\n"))
    w('},\n"sb": {\n')
    w(f'"selftest": {dump(out["sb"]["selftest"])},\n"steps": [\n' + ",\n".join(dump(r) for r in out["sb"]["steps"]) + "\n]\n}\n}\n")


if __name__ == "__main__":
    main()
