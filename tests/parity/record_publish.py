#!/usr/bin/env python3
"""錄確定性推送工具（publish.py）的對跑基準。

用法：python3 tests/parity/record_publish.py > tests/parity/publish.expected.json

基準檔本身就是劇本：`steps` 依序是檔案操作（write、rm）、git 指令（git：參數、exit code、stdout）、
publish 的指令（run：argv、exit code、stdout、這一步改了哪些檔）與只能在程式內呼叫的函式（call）。
Rust 的測試（crates/awf-core/tests/publish_parity.rs）照同一份劇本做一遍，逐字比對。

絕對不碰真的遠端：
- remote 是暫存目錄裡的 bare repo（`remotes/acme-fork/app.git`），push 只會寫到那裡。
- `gh` 是暫存目錄 `bin/` 裡的假腳本（PATH 放最前面）：把收到的參數記到 `bin/gh.log`、PR 說明抄到 `bin/gh.body`、印一個假網址。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`（一開始就取實體路徑，只有一種寫法）、模版根 `@SRC@`、Python 路徑 `@PY@`。
- git 的 author／committer 與日期固定、不讀使用者的 git 設定，所以 SHA 固定；commit 的內容不含暫存路徑。
- publish 的時間戳都用 `--timestamp` 給；唯一不給的那一步（`now`）把時間換成 `@NOW@`。
- git 自己的錯誤文字（push 被拒、diff 失敗）隨 git 版本不同，換成 `@GIT@`，只比前面的理由。
- 像憑證的字串不寫在這支腳本與基準檔裡：用 `{"@join": […]}`、`{"@rep": [字, 次數]}` 在執行時組出來。
- run 目錄用 save_run.py 建好（和 publish.py 的 selftest 同一套做法），再把檔案內容錄成 write 步驟。
- 只會附加的檔（稽核紀錄、gh.log）只記這一步新增的部分（`{"append": …}`）。
- 用法訊息（指令名改成 `awf publish`）只比 exit code。Python 沒接住的例外記類別名：Rust 要一樣 exit 1、stderr 提到同一個類別，
  而且同樣不寫稽核紀錄。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import argparse
import contextlib
import copy
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
sys.path.insert(0, str(TOOLS))
os.environ.pop("CLAUDE_PROJECT_DIR", None)
import publish  # noqa: E402
import recipe as rcp  # noqa: E402
import save_run  # noqa: E402

TS = "2026-10-24T09:00:00+00:00"
NOW = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\+00:00")
GIT_TEXT = re.compile(r'git (push|diff) 失敗：(?:[^"\\]|\\.)*')
GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid", "GIT_AUTHOR_DATE": "2026-01-02T03:04:05+00:00",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid", "GIT_COMMITTER_DATE": "2026-01-02T03:04:05+00:00",
           "GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0"}
FAKE_GH = """#!/bin/sh
# 假的 gh：只記錄收到的參數，不連網。
d="$(dirname "$0")"
line=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--body-file" ]; then
    cp "$a" "$d/gh.body"
    case "$(basename "$a")" in publish-body-*) a="@TMPBODY@";; esac
  fi
  line="$line $a"
  prev="$a"
done
printf '%s\\n' "$line" >> "$d/gh.log"
if [ -f "$d/gh.fail" ]; then echo "fake gh: boom" >&2; exit 1; fi
if [ -f "$d/gh.out" ]; then cat "$d/gh.out"; else echo "https://github.com/acme-fork/app/pull/7"; fi
"""
INTENT = "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n## 驗收條件\n- AC-1\n## 調研問題\nx\n## 判準\nx\n## 現象\nx\n## 重現步驟\nx\n## 預期行為\nx\n## 根因\nx\n"


def J(*parts):
    """執行時才組出來的字串。"""
    return {"@join": list(parts)}


def rep(ch, n):
    return {"@rep": [ch, n]}


def dec(v):
    if isinstance(v, dict):
        if set(v) == {"@join"}:
            return "".join(dec(x) for x in v["@join"])
        if set(v) == {"@rep"}:
            return v["@rep"][0] * v["@rep"][1]
        return {k: dec(x) for k, x in v.items()}
    if isinstance(v, list):
        return [dec(x) for x in v]
    return v


KEY = J("AK", "IA", rep("Q", 16))


class Rec:
    def __init__(self):
        self.td = os.path.realpath(tempfile.mkdtemp(prefix="publish-parity-"))
        self.steps = []

    def norm(self, s):
        s = s.replace(self.td, "@TMP@").replace(str(ROOT), "@SRC@").replace(sys.executable, "@PY@")
        return GIT_TEXT.sub(lambda m: f"git {m.group(1)} 失敗：@GIT@", s)

    def abs(self, v):
        if isinstance(v, str):
            return v.replace("@TMP@", self.td)
        if isinstance(v, list):
            return [self.abs(x) for x in v]
        if isinstance(v, dict):
            return {k: self.abs(x) for k, x in v.items()}
        return v

    def p(self, rel):
        return os.path.join(self.td, rel)

    # ---------------------------------------------------------- 檔案
    def w(self, rel, text, exe=False):
        """text 可以是字串或 @join 的結構；內容裡的 @TMP@ 換成暫存路徑。"""
        f = Path(self.p(rel))
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_bytes(self.abs(dec(text)).encode("utf-8"))
        if exe:
            f.chmod(0o755)
        st = {"op": "write", "path": rel, "text": text}
        if exe:
            st["exec"] = True
        self.steps.append(st)

    def wj(self, rel, obj):
        self.w(rel, json.dumps(obj, ensure_ascii=False))

    def wb(self, rel, data):
        f = Path(self.p(rel))
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_bytes(data)
        self.steps.append({"op": "write", "path": rel, "hex": data.hex()})

    def rm(self, rel):
        Path(self.p(rel)).unlink(missing_ok=True)
        self.steps.append({"op": "rm", "path": rel})

    def snapshot(self):
        out = {}
        for d, dirs, files in os.walk(self.td):
            dirs[:] = sorted(x for x in dirs if x != ".git" and not (d == self.td and x == "remotes"))
            for n in sorted(files):
                f = os.path.join(d, n)
                out[os.path.relpath(f, self.td)] = self.norm(Path(f).read_bytes().decode("utf-8", "replace"))
        return out

    def changed(self, before):
        after = self.snapshot()
        out = {}
        for k in sorted(set(before) | set(after)):
            if before.get(k) != after.get(k):
                a, b = before.get(k), after.get(k)
                out[k] = {"append": b[len(a):]} if a is not None and b is not None and b.startswith(a) else b
        return out

    # ---------------------------------------------------------- git
    def git(self, cwd, *args):
        r = subprocess.run(["git", *self.abs(list(args))], cwd=self.p(cwd), capture_output=True, text=True, encoding="utf-8", errors="replace",
                           stdin=subprocess.DEVNULL)
        self.steps.append({"op": "git", "cwd": cwd, "args": list(args), "code": r.returncode, "out": self.norm(r.stdout)})
        return r

    def commit(self, rel, text, msg, binary=None):
        if binary is not None:
            self.wb("proj/repos/app/" + rel, binary)
        else:
            self.w("proj/repos/app/" + rel, text)
        self.git("proj/repos/app", "add", "-A")
        self.git("proj/repos/app", "commit", "-qm", msg)

    # ---------------------------------------------------------- publish
    def run(self, note, *argv, label=None, now=False):
        before = self.snapshot()
        out, err = io.StringIO(), io.StringIO()
        st = {"op": "run", "note": note, "argv": list(argv)}
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                st["code"] = publish.main(self.abs(list(argv)))
        except SystemExit as e:
            st["code"] = e.code if isinstance(e.code, int) else 1
            st["usage"] = True
        except Exception as e:  # noqa: BLE001
            st["code"] = 1
            st["raises"] = type(e).__name__
        text = self.norm(out.getvalue())
        files = self.changed(before)
        if now:
            fix = lambda s: NOW.sub(lambda m: m.group(0) if m.group(0) == TS else "@NOW@", s)  # noqa: E731
            text = fix(text)
            files = {k: ({"append": fix(v["append"])} if isinstance(v, dict) else fix(v)) for k, v in files.items()}
            st["now"] = True
        if not st.get("usage"):
            st["stdout"] = text
        st["files"] = files
        if label:
            st["label"] = label
        self.steps.append(st)
        return st

    def push(self, note, *extra, drop=(), **kw):
        """預設參數的 push（和 selftest 的 case 一樣），extra 蓋掉或加上去。"""
        base = {"--repo-dir": "@TMP@/proj/repos/app", "--branch": "feature/export", "--run": "@TMP@/proj/_tpm/runs/r-good", "--title": "docs: 匯出說明",
                "--body-file": "@TMP@/body.md", "--timestamp": TS}
        flags = []
        it = iter(extra)
        for k in it:
            if k in ("--draft", "--no-pr", "--dry-run"):
                flags.append(k)
            else:
                base[k] = next(it)
        for k in drop:
            base.pop(k, None)
        argv = ["push"]
        for k, v in base.items():
            argv += [k, v]
        return self.run(note, *argv, *flags, **kw)

    def call(self, fn, note="", label=None, **args):
        st = {"op": "call", "fn": fn, "args": args, "result": tried(fn, self.abs(dec(args)), self)}
        if note:
            st["note"] = note
        if label:
            st["label"] = label
        self.steps.append(st)
        return st


def cfg_of(d):
    return {"fork_repo": d.get("fork_repo", ""), "upstream_repos": d.get("upstream_repos", []), "forbidden": d.get("forbidden", []),
            "prefixes": d.get("prefixes", publish.DEFAULT_PREFIXES), "protected": d.get("protected", publish.DEFAULT_PROTECTED), "runs_dir": d.get("runs_dir", "_tpm/runs")}


def _check_urls(a):
    real = publish.git
    publish.git = lambda repo, *args: subprocess.CompletedProcess(args, 0, " ".join(a["urls"]), "")
    try:
        publish.check_remote(Path(a["repo"]), a["remote"], cfg_of(a["cfg"]))
    finally:
        publish.git = real
    return None


CALLS = {
    "slug_of": lambda a: publish.slug_of(a["url"]),
    "secret_file": lambda a: publish.secret_file(a["path"]),
    "scan_secrets": lambda a: publish.scan_secrets(a["diff"], a.get("literals") or [], a.get("names") or []),
    "check_target": lambda a: publish.check_target(cfg_of(a["cfg"]), a["branch"], a["base"]),
    "check_urls": _check_urls,
    "load_config": lambda a: publish.load_config(Path(a["proj"])),
    "positive_int": lambda a: publish.positive_int(a["v"]),
    "supersedes_line": lambda a: publish.supersedes_line(a["n"]),
    "check_policy": lambda a: publish.check_policy(Path(a["run"]) if a.get("run") else None, a["stage"]),
    "check_shape": lambda a: publish.check_shape(Path(a["run"]), a["paths"]),
}


def tried(fn, a, r):
    try:
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            v = CALLS[fn](a)
        return {"ok": json.loads(r.norm(json.dumps(v, ensure_ascii=False)))}
    except (publish.Refused, argparse.ArgumentTypeError) as e:
        return {"refused": r.norm(str(e))}
    except RecursionError:
        raise
    except Exception as e:  # noqa: BLE001
        return {"raises": type(e).__name__}


# ------------------------------------------------------------------ 劇本
def fixture(r):
    r.w("gitconfig", "")
    r.w("bin/gh", FAKE_GH, exe=True)
    r.w("body.md", "示範 PR 說明\n")
    cfg = {"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "upstream_repos": "acme-up/app", "forbidden_paths": ["acme-banned/app", "docs/"],
           "allowed_branch_prefixes": "feature/,bugfix/", "protected_branches": ["main", "master"]}
    r.wj("proj/.claude/teams/guard.config.json", cfg)
    recipes = copy.deepcopy(rcp.BUILTIN_RECIPES)
    r.wj("proj/.claude/teams/methodology.json", {"recipes": recipes, "modifiers": rcp.BUILTIN_MODIFIERS})
    r.w("proj/epic/specs/e/intent.md", INTENT)
    r.w("proj/repos/app/README.md", "# 示範客戶 ACME\n")
    for slug in ("acme-fork/app", "acme-up/app", "someone/app"):
        r.git(".", "-c", "init.defaultBranch=main", "init", "-q", "--bare", f"remotes/{slug}.git")
    g = lambda *a: r.git("proj/repos/app", *a)  # noqa: E731
    g("-c", "init.defaultBranch=main", "init", "-q")
    g("add", "-A")
    g("commit", "-qm", "init")
    g("remote", "add", "origin", "@TMP@/remotes/acme-fork/app.git")
    g("remote", "add", "upstream", "@TMP@/remotes/acme-up/app.git")
    g("remote", "add", "other", "@TMP@/remotes/someone/app.git")
    g("remote", "add", "two", "@TMP@/remotes/acme-fork/app.git")
    g("remote", "set-url", "--add", "--push", "two", "@TMP@/remotes/acme-fork/app.git")
    g("remote", "set-url", "--add", "--push", "two", "@TMP@/remotes/someone/app.git")
    g("remote", "add", "sub", "@TMP@/remotes/acme-up/app.git/../../acme-fork/app.git")
    g("remote", "add", "weird", "weird")
    g("push", "-q", "origin", "main")
    g("update-ref", "refs/remotes/upstream/main", "HEAD")
    g("update-ref", "refs/remotes/other/main", "HEAD")
    g("update-ref", "refs/remotes/two/main", "HEAD")
    g("branch", "feature/same")
    g("checkout", "-qb", "feature/export")
    r.commit("docs/export.md", "# 匯出 CSV\n", "docs: 匯出說明")
    g("branch", "wip/x")
    g("branch", "bugfix/docs", "feature/export")
    g("checkout", "-qb", "feature/leak")
    r.commit("config.py", J("AWS_KEY = '", KEY, "'\n"), "feat: 設定")
    for br, fn, text, data in (("feature/p12", "certs/client.p12", None, b"\x00\x01\x02\xff binary"), ("feature/emptyenv", ".env", None, b""),
                               ("feature/plus3", "notes.md", J("++ ", KEY, "\n"), None), ("feature/envsample", ".env.example", "A=1\n", None),
                               ("feature/code", "src/export.py", "def export():\n    return 'csv'\n", None),
                               ("feature/migration", "db/migrations/0002_add.sql", "ALTER TABLE t ADD c INT;\n", None)):
        g("checkout", "-q", "main")
        g("checkout", "-qb", br)
        r.commit(fn, text, f"feat: {fn}", binary=data)
    # 改名：舊路徑是機敏檔名（刪掉的那邊不算）、新路徑是機敏檔名（算）
    g("checkout", "-q", "main")
    g("checkout", "-qb", "feature/rename")
    r.commit("docs/a.txt", "一\n二\n三\n四\n五\n", "docs: a")
    g("update-ref", "refs/remotes/origin/renbase", "HEAD")
    g("mv", "docs/a.txt", "docs/server.key")
    g("commit", "-qm", "docs: 改名")
    # 沒有共同祖先的分支：git diff A...B 失敗
    g("checkout", "-q", "--orphan", "feature/orphan")
    r.commit("docs/o.md", "孤兒\n", "docs: 孤兒")
    # 遠端已經有不同的 commit：不 force 就推不上去
    g("checkout", "-q", "main")
    g("checkout", "-qb", "feature/diverged")
    r.commit("docs/d.md", "第一版\n", "docs: d1")
    g("push", "-q", "origin", "feature/diverged")
    g("commit", "-q", "--amend", "-m", "docs: d1（改）")
    g("checkout", "-q", "main")
    g("rev-parse", "main", "feature/export", "feature/code")


def make_runs(r):
    """用 save_run.py 建 run（做法同 publish.py 的 selftest），再把檔案錄成 write 步驟。"""
    proj = Path(r.p("proj"))
    epic = proj / "epic"
    sp = str(TOOLS / "save_run.py")
    prof = ["--profile", str(proj / ".claude/teams/methodology.json")]
    base = {"summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": []}

    def sub(*a, stdin=""):
        return subprocess.run([sys.executable, sp, *a], input=stdin, capture_output=True, text=True, encoding="utf-8", cwd=r.td)

    def make_run(name, deliver_by="使用者", deliver_payload=None, deliver_extra=None, recipe=None):
        rd = proj / "_tpm/runs" / name
        sub("init", str(rd), "--event", "sdlc")
        for stage in save_run.SDLC:
            if stage != "plan" and json.loads((rd / "run.json").read_text(encoding="utf-8"))["stages"][stage]["status"] == "skipped":
                continue
            payload = deliver_payload if (stage == "deliver" and deliver_payload is not None) else {}
            if stage == "test":
                payload = {"commit": "a" * 40, "scope_covered": "全部", "scope_skipped": "none", "anti_cheat": [],
                           "criteria": [{"id": "AC-1", "criterion": "c", "command": "true", "result": "pass", "evidence": "e"}]}
            res = {"x": [{**base, "status": "done", "payload": payload}]}
            if stage == "plan":
                res = {"epic_dir": str(epic), "intent": {**base, "status": "done", "payload": {
                    "documents": [{"path": "specs/e/intent.md", "kind": "intent", "audience": "owner"}]}}}
                if recipe:
                    res["recipe"] = {"name": recipe, "modifiers": [], "size": "small"}
            if stage == "deliver" and deliver_extra:
                res.update(deliver_extra)
            if recipe and stage != "plan":
                rec = json.loads((rd / "run.json").read_text(encoding="utf-8"))["recipe"]
                rv = rec["resolved"]
                res["recipe"] = {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rv["stages"]}
                roles = [("agent", "agent-workflow:dev-implementer")]
                if stage == "test":
                    roles = [("verify#1", "agent-workflow:dev-verifier")] + [
                        (k, "agent-workflow:" + save_run.REVIEW_ROLES[k]) for k in save_run.REVIEW_ROLES if rv["test"].get(k) == "always"]
                if stage == "deliver":
                    roles = [(f"review:{a}", "agent-workflow:tpm-reviewer") for a in rv.get("deliver_axes") or []]
                (rd / stage).mkdir(parents=True, exist_ok=True)
                res["trace"] = []
                for label, at in roles:
                    f = rd / stage / (label.replace(":", "-").replace("#", "-") + ".md")
                    f.write_text("# 紀錄\n", encoding="utf-8")
                    res["trace"].append({"label": label, "agent_type": at, "phase": "p", "record": str(f), "ok": True})
            sub("stage", str(rd), stage, *prof, stdin=json.dumps(res))
            if stage == "deliver" and deliver_by is None:
                break
            if stage == "deliver" and deliver_by == "policy-forged":
                data = json.loads((rd / "run.json").read_text(encoding="utf-8"))
                data["stages"]["deliver"]["approved"] = {"by": "policy", "at": "t", "note": "p", "kind": "policy"}
                (rd / "run.json").write_text(json.dumps(data), encoding="utf-8")
                break
            sub("approve", str(rd), stage, "--by", deliver_by if stage == "deliver" else "使用者", "--at", "t", "--note", "p", *prof)
        return rd

    make_run("r-good")
    make_run("r-pending", deliver_by=None)
    finding = {"lens": "bugs", "findings": [{"id": "B1", "severity": "important", "claim": "c", "location": "a:1", "recommendation": "r", "confidence": 0.9}]}
    make_run("r-forged", deliver_by="policy-forged", deliver_payload=finding)
    make_run("r-skeptic", deliver_by="policy-forged", deliver_payload={"verdicts": [{"target_id": "PR 說明", "refuted": True, "reason": "漏列驗收條件"}]})
    make_run("r-lost", deliver_by="policy-forged", deliver_extra={"docs": None})
    make_run("r-clean", deliver_by="policy-forged")
    make_run("r-docs", recipe="docs")
    make_run("r-bugfix", recipe="bugfix")
    make_run("r-feature", recipe="feature")
    make_run("r-spike", deliver_by=None, recipe="spike")
    dropped = make_run("r-abandon")
    sub("abandon", str(dropped), "--reason", "需求取消", "--by", "使用者", "--at", "t")
    runs = proj / "_tpm/runs"
    files = sorted(f for f in runs.rglob("*") if f.is_file())
    texts = [(str(f.relative_to(r.td)), r.norm(f.read_text(encoding="utf-8"))) for f in files]
    shutil.rmtree(runs)
    for rel, text in texts:
        r.w(rel, text)


def variant(r, name, src, edit, files=()):
    """從既有的 run 複製 run.json 並改一處；files 是要一起複製的結果檔。"""
    data = json.loads(Path(r.p(f"proj/_tpm/runs/{src}/run.json")).read_text(encoding="utf-8"))
    out = edit(data)
    r.wj(f"proj/_tpm/runs/{name}/run.json", data if out is None else out)
    for f in files:
        r.w(f"proj/_tpm/runs/{name}/{f}", r.norm(Path(r.p(f"proj/_tpm/runs/{src}/{f}")).read_text(encoding="utf-8")))
    return f"@TMP@/proj/_tpm/runs/{name}"


def scenario_policy(r):
    R = lambda n: f"@TMP@/proj/_tpm/runs/{n}"  # noqa: E731
    pol = lambda run, stage="deliver", **kw: r.call("check_policy", run=run, stage=stage, **kw)  # noqa: E731
    for stage in ("deliver", "merge", "test", "plan", "design", "build", "sync", "run", "", "Deliver"):
        pol(R("r-good"), stage)
    pol(None)
    pol("")
    pol(R("r-nope"))
    for n in ("r-pending", "r-forged", "r-skeptic", "r-lost", "r-clean", "r-docs", "r-bugfix", "r-feature", "r-spike", "r-abandon"):
        pol(R(n))
        pol(R(n), "merge")

    def S(fn):
        def edit(d):
            fn(d["stages"])
        return edit

    def sets(path, value):
        def edit(d):
            cur = d
            for k in path[:-1]:
                cur = cur[k]
            cur[path[-1]] = value
        return edit

    def drops(*path):
        def edit(d):
            cur = d
            for k in path[:-1]:
                cur = cur[k]
            cur.pop(path[-1], None)
        return edit

    # run.json 的形狀
    r.w("proj/_tpm/runs/v-badjson/run.json", "{壞")
    pol(R("v-badjson"))
    r.wb("proj/_tpm/runs/v-notutf8/run.json", b'{"stages": "\xff"}')
    pol(R("v-notutf8"))
    for name, body in (("v-list", []), ("v-str", "run"), ("v-null", None), ("v-empty", {}), ("v-stages-list", {"stages": []}), ("v-stages-str", {"stages": "x"})):
        r.wj(f"proj/_tpm/runs/{name}/run.json", body)
        pol(R(name))
        pol(R(name), "merge")
    # 已取消的各種寫法
    for name, ab in (("v-ab-true", True), ("v-ab-str", "取消"), ("v-ab-partial", {"by": "owner"}), ("v-ab-empty", {}), ("v-ab-zero", 0), ("v-ab-list", ["x"])):
        pol(variant(r, name, "r-good", sets(["abandoned"], ab)))
    # 少段
    pol(variant(r, "v-no-deliver-design", "r-good", S(lambda s: (s.pop("deliver"), s.pop("design")))))
    pol(variant(r, "v-no-deliver", "r-good", drops("stages", "deliver")))
    pol(variant(r, "v-no-plan", "r-good", drops("stages", "plan")))
    pol(variant(r, "v-no-test", "r-good", drops("stages", "test")))
    pol(variant(r, "v-no-build", "r-good", drops("stages", "build")))
    pol(variant(r, "v-no-sync", "r-good", drops("stages", "sync")), note="沒有 recipe 的舊 run 容許沒有 sync")
    pol(variant(r, "v-no-merge", "r-good", drops("stages", "merge")), "merge")
    pol(variant(r, "v-deliver-str", "r-good", sets(["stages", "deliver"], "done")))
    pol(variant(r, "v-design-null", "r-good", sets(["stages", "design"], None)))
    # 狀態與核准
    for st in ("pending", "blocked", "invalid", "skipped", None, 1):
        pol(variant(r, f"v-deliver-{st}", "r-good", sets(["stages", "deliver", "status"], st)))
    pol(variant(r, "v-deliver-unapproved", "r-good", drops("stages", "deliver", "approved")))
    pol(variant(r, "v-deliver-nogate", "r-good", S(lambda s: (s["deliver"].pop("approved"), s["deliver"].update(gate=False)))), note="deliver 一定要核准，gate 寫成 false 也一樣")
    pol(variant(r, "v-deliver-approved-false", "r-good", sets(["stages", "deliver", "approved"], False)))
    for st in ("pending", "blocked", None):
        pol(variant(r, f"v-merge-{st}", "r-good", sets(["stages", "merge"], {"status": st, "gate": False})), "merge")
    pol(variant(r, "v-merge-gated", "r-good", sets(["stages", "merge"], {"status": "done", "gate": True})), "merge")
    pol(variant(r, "v-merge-gated-ok", "r-good", sets(["stages", "merge"], {"status": "done", "gate": True, "approved": {"by": "owner", "at": "t", "kind": "human"}})), "merge")
    # 閘門鏈
    pol(variant(r, "v-test-unapproved", "r-good", drops("stages", "test", "approved")))
    pol(variant(r, "v-plan-unapproved", "r-good", drops("stages", "plan", "approved")))
    pol(variant(r, "v-build-pending", "r-good", sets(["stages", "build", "status"], "pending")))
    pol(variant(r, "v-sync-blocked", "r-good", sets(["stages", "sync", "status"], "blocked")))
    pol(variant(r, "v-design-skipped", "r-good", sets(["stages", "design", "status"], "skipped")))
    pol(variant(r, "v-many-bad", "r-good", S(lambda s: [s[k].update(status="pending") for k in ("plan", "design", "build", "sync", "test")])), note="只列前五條")
    pol(variant(r, "v-bugfix-test-unapproved", "r-bugfix", drops("stages", "test", "approved")))
    pol(variant(r, "v-bugfix-no-build", "r-bugfix", drops("stages", "build")))
    pol(variant(r, "v-bugfix-design-done", "r-bugfix", sets(["stages", "design"], {"status": "pending", "gate": True})), note="recipe 沒有的段不檢查")
    pol(variant(r, "v-docs-no-test", "r-docs", drops("stages", "test")))
    # 核准紀錄的格式
    pol(variant(r, "v-design-approved-str", "r-good", sets(["stages", "design", "approved"], "yes")))
    pol(variant(r, "v-deliver-approved-str", "r-good", sets(["stages", "deliver", "approved"], "yes")))
    pol(variant(r, "v-deliver-approved-list", "r-good", sets(["stages", "deliver", "approved"], [1, "二"])))
    pol(variant(r, "v-deliver-approved-num", "r-good", sets(["stages", "deliver", "approved"], 1)))
    pol(variant(r, "v-deliver-approved-min", "r-good", sets(["stages", "deliver", "approved"], {"x": 1})), note="by、kind、at 都沒有")
    pol(variant(r, "v-deliver-approved-null-kind", "r-good", sets(["stages", "deliver", "approved"], {"by": 5, "kind": None, "at": None})))
    # 政策核准：重跑判定
    policy = {"by": "policy", "at": "t", "note": "p", "kind": "policy"}
    pol(variant(r, "v-policy-nofile", "r-good", sets(["stages", "deliver", "approved"], policy)))
    v = variant(r, "v-policy-badfile", "r-good", sets(["stages", "deliver", "approved"], policy))
    r.w("proj/_tpm/runs/v-policy-badfile/deliver.json", "不是 JSON")
    pol(v)
    v = variant(r, "v-policy-notutf8", "r-good", sets(["stages", "deliver", "approved"], policy))
    r.wb("proj/_tpm/runs/v-policy-notutf8/deliver.json", b"\xff\xfe")
    pol(v)
    pol(variant(r, "v-policy-kind-only", "r-good", sets(["stages", "deliver", "approved"], {"by": "owner", "at": "t", "kind": "policy"}), files=["deliver.json"]))
    pol(variant(r, "v-policy-by-only", "r-good", sets(["stages", "deliver", "approved"], {"by": "policy", "at": "t"}), files=["deliver.json"]))
    pol(variant(r, "v-policy-test", "r-good", sets(["stages", "test", "approved"], policy), files=["test.json"]), note="前面的段是政策核准也重跑")
    pol(variant(r, "v-policy-plan", "r-good", sets(["stages", "plan", "approved"], policy), files=["plan.json"]), note="plan 不能政策核准")
    v = variant(r, "v-policy-halted", "r-good", sets(["stages", "deliver", "approved"], policy))
    r.wj("proj/_tpm/runs/v-policy-halted/deliver.json", {"halted": True, "x": []})
    pol(v)
    v = variant(r, "v-policy-list", "r-good", sets(["stages", "deliver", "approved"], policy))
    r.wj("proj/_tpm/runs/v-policy-list/deliver.json", [])
    pol(v)
    pol(variant(r, "v-policy-merge", "r-good", sets(["stages", "merge", "approved"], policy), files=["merge.json"]), "merge")
    pol(variant(r, "v-policy-merge-not-asked", "r-good", sets(["stages", "merge", "approved"], policy)), note="--stage deliver 時不看 merge 段")
    # recipe
    pol(variant(r, "v-tamper-stages", "r-docs", sets(["recipe", "resolved", "stages"], ["plan", "deliver", "merge"])))
    pol(variant(r, "v-tamper-publishable", "r-spike", sets(["recipe", "resolved", "publishable"], True)))
    pol(variant(r, "v-tamper-name", "r-docs", sets(["recipe", "name"], "nope")))
    pol(variant(r, "v-recipe-str", "r-docs", sets(["recipe"], "docs")), note="recipe 不是物件：當成沒有 recipe 的舊 run")
    pol(variant(r, "v-recipe-noresolved", "r-docs", drops("recipe", "resolved")))
    pol(variant(r, "v-recipe-stricter", "r-docs", sets(["recipe", "resolved", "stages"], ["plan", "design", "build", "sync", "test", "deliver", "merge"])), note="比重算結果嚴：採信，但缺段")
    pol(variant(r, "v-feature-nan-extra", "r-feature", sets(["extra"], 1)))
    # 只會更嚴：這些 run.json Python 讀得進來
    nan = Path(r.p("proj/_tpm/runs/r-good/run.json")).read_text(encoding="utf-8")
    head = nan[: nan.rindex("}")]
    r.w("proj/_tpm/runs/s-nan/run.json", head + ', "x": NaN}')
    pol(R("s-nan"), label="nan-run")
    r.w("proj/_tpm/runs/s-inf/run.json", head + ', "x": -Infinity}')
    pol(R("s-inf"), label="nan-run")
    r.w("proj/_tpm/runs/s-huge/run.json", head + ', "x": 123456789012345678901234567890}')
    pol(R("s-huge"), label="huge-int-run")
    r.w("proj/_tpm/runs/s-deep/run.json", head + ', "x": ' + "[" * 300 + "]" * 300 + "}")
    pol(R("s-deep"), label="deep-run")
    r.w("proj/_tpm/runs/s-surrogate/run.json", head + ', "x": "\\ud800"}')
    pol(R("s-surrogate"), label="surrogate-run")
    v = variant(r, "s-nan-stage", "r-good", sets(["stages", "deliver", "approved"], policy))
    clean = Path(r.p("proj/_tpm/runs/r-clean/deliver.json")).read_text(encoding="utf-8").rstrip()
    r.w("proj/_tpm/runs/s-nan-stage/deliver.json", r.norm(clean[: clean.rindex("}")]) + ', "n": NaN}')
    pol(v, label="nan-stage-file")
    # diff 形狀
    shape = lambda run, *paths, **kw: r.call("check_shape", run=R(run), paths=list(paths), **kw)  # noqa: E731
    shape("r-docs", "docs/a.md")
    shape("r-docs", "src/a.py", "docs/a.md")
    shape("r-docs")
    shape("r-good", "src/a.py", "tests/test_a.py", "docs/a.md")
    shape("r-good", "db/migrations/0001.sql")
    shape("r-good", "package.json", ".github/workflows/ci.yml", ".claude/agents/x.md")
    shape("r-bugfix", "db/migrations/0001.sql", "requirements.txt")
    shape("r-spike", "docs/a.md")
    shape("r-nope", "docs/a.md")
    shape("v-badjson", "docs/a.md")
    shape("v-notutf8", "docs/a.md")
    shape("v-list", "docs/a.md")
    shape("v-empty", "src/a.py")
    shape("s-nan", "docs/a.md", label="nan-run")
    shape("r-docs", *[f"src/f{i}.py" for i in range(8)], note="只列前五個")


def scenario_cli(r):
    cfgp = "proj/.claude/teams/guard.config.json"
    base_cfg = json.loads(Path(r.p(cfgp)).read_text(encoding="utf-8"))
    R = lambda n: f"@TMP@/proj/_tpm/runs/{n}"  # noqa: E731
    refs = lambda: r.git(".", "--git-dir", "remotes/acme-fork/app.git", "for-each-ref", "--format=%(refname) %(objectname)")  # noqa: E731
    # 用法
    r.run("沒有子指令")
    r.run("不認得的子指令", "pull")
    r.run("--help", "--help")
    r.run("push --help", "push", "--help")
    r.run("缺 --repo-dir、--branch", "push")
    r.run("缺 --branch", "push", "--repo-dir", "@TMP@/proj/repos/app")
    r.run("不認得的選項", "push", "--repo-dir", "x", "--branch", "feature/x", "--force")
    r.run("選項縮寫有歧義", "push", "--repo-dir", "x", "--branch", "feature/x", "--b", "main")
    for v in ("0", "-1", "x", "1.5", ""):
        r.push(f"--supersedes {v!r} 不是正整數", "--supersedes", v, "--dry-run")
    # 通過的路徑
    r.push("dry-run：檢查全過但不推送", "--dry-run")
    refs()
    r.run("選項縮寫（--dry、--repo、--tim）", "push", "--repo", "@TMP@/proj/repos/app", "--bra", "feature/export", "--run", R("r-good"), "--tit", "t", "--body", "@TMP@/body.md",
          "--tim", TS, "--dry")
    r.push("沒給 --timestamp：用系統時間", "--dry-run", drop=["--timestamp"], now=True)
    r.push("推送並開 PR")
    refs()
    r.push("再推一次（遠端已經是同一個 commit）")
    r.push("--supersedes 12：PR 說明末尾加一行", "--supersedes", "12")
    r.push("--supersedes ' 1_2 '（int() 收的寫法）", "--supersedes", " 1_2 ", "--dry-run")
    r.push("--draft", "--draft")
    r.push("--no-pr：只推分支", "--no-pr", "--branch", "bugfix/docs")
    refs()
    r.push("--no-pr 不需要 --title 與 --body-file", "--no-pr", drop=["--title", "--body-file"])
    r.push("--stage merge", "--stage", "merge", "--dry-run")
    r.push("--project-dir 明寫", "--project-dir", "@TMP@/proj", "--dry-run")
    r.push("--run 寫相對路徑", "--run", "proj/_tpm/runs/r-good", "--repo-dir", "proj/repos/app", "--body-file", "body.md", "--dry-run")
    r.push("bugfix 改程式", "--run", R("r-bugfix"), "--branch", "feature/code", "--dry-run")
    r.push("docs recipe 只改文件", "--run", R("r-docs"), "--dry-run")
    # gh 的各種回應
    r.w("bin/gh.out", "Creating pull request\n\nhttps://github.com/acme-fork/app/pull/9\n\n")
    r.push("gh 印多行：取最後一行")
    r.w("bin/gh.out", "")
    r.push("gh 沒印東西：pr_url 是空的")
    r.rm("bin/gh.out")
    r.w("bin/gh.fail", "")
    r.push("gh 失敗：已推送但 PR 沒開成")
    r.push("gh 失敗（--supersedes）：暫存的說明檔照樣刪掉", "--supersedes", "3")
    r.rm("bin/gh.fail")
    # 開 PR 的條件
    r.push("沒有 --title", drop=["--title"])
    r.push("--title 是空的", "--title", "")
    r.push("沒有 --body-file", drop=["--body-file"])
    r.push("--body-file 不存在", "--body-file", "@TMP@/nope.md")
    r.push("--body-file 是目錄", "--body-file", "@TMP@/proj")
    r.push("沒有 --title 的 dry-run 也拒絕", "--dry-run", drop=["--title"])
    # 目標與分支
    r.push("受保護分支 main", "--branch", "main")
    r.push("分支就是 PR 的 base", "--branch", "feature/export", "--base", "feature/export")
    r.push("前綴不在白名單（wip/）", "--branch", "wip/x")
    for b in ("", "-x", "+feature/x", "refs/heads/feature/x", "feature/a:b", "feature/a..b", "feature/'引號'"):
        r.push(f"分支名 {b!r}", "--branch", b)
    r.push("本機沒有這個分支", "--branch", "feature/nope")
    r.push("找不到 base", "--base", "nope")
    r.push("相對 base 沒有改動", "--branch", "feature/same")
    r.push("沒有共同祖先：git diff 失敗", "--branch", "feature/orphan")
    r.push("--repo-dir 不存在", "--repo-dir", "@TMP@/nope")
    r.push("--repo-dir 不是 git repo", "--repo-dir", "@TMP@/proj/epic")
    # remote
    r.push("remote 不存在", "--remote", "nope")
    r.push("remote 指向上游", "--remote", "upstream")
    r.push("remote 指向別人的 repo", "--remote", "other")
    r.push("remote 有兩個 push URL，其中一個不是 fork", "--remote", "two")
    r.push("remote 的 URL 裡有上游的名字", "--remote", "sub")
    r.push("remote 的 URL 認不出 owner/name", "--remote", "weird")
    # 遠端已經有不同的 commit
    r.push("不 force：遠端分支有別的 commit 就推不上去", "--branch", "feature/diverged", "--no-pr")
    refs()
    # 機敏值
    r.push("diff 有 AWS 金鑰樣式", "--branch", "feature/leak")
    r.push("二進位憑證檔（.p12）", "--branch", "feature/p12")
    r.push("空的 .env", "--branch", "feature/emptyenv")
    r.push("「++ 」開頭的金鑰行", "--branch", "feature/plus3")
    r.push(".env.example 不算機敏檔", "--branch", "feature/envsample", "--dry-run")
    r.push("改名成機敏檔名", "--branch", "feature/rename", "--base", "renbase")
    r.w("secrets.txt", "\n  匯出 CSV  \n\n不會出現的值\n")
    r.push("--secrets-file 列的值出現在 diff", "--secrets-file", "@TMP@/secrets.txt")
    r.w("secrets-none.txt", "不會出現的值\n\n")
    r.push("--secrets-file 的值都沒出現", "--secrets-file", "@TMP@/secrets-none.txt", "--dry-run")
    r.push("--secrets-file 是空字串：當成沒給", "--secrets-file", "", "--dry-run")
    r.push("--secrets-file 不存在", "--secrets-file", "@TMP@/nope.txt")
    r.push("--secrets-file 是目錄", "--secrets-file", "@TMP@/proj")
    r.wb("secrets-bad.txt", b"\xff\xfe\n")
    r.push("--secrets-file 不是 UTF-8", "--secrets-file", "@TMP@/secrets-bad.txt")
    # 政策（完整流程，稽核紀錄要記到）
    r.push("沒有 --run", drop=["--run"])
    r.push("--run 是空字串", "--run", "")
    r.push("--stage test", "--stage", "test", "--run", R("r-pending"))
    r.push("--stage run", "--stage", "run")
    r.push("run.json 讀不到", "--run", R("r-nope"))
    r.push("run.json 不是 UTF-8", "--run", R("v-notutf8"))
    r.push("run.json 是清單", "--run", R("v-list"))
    r.push("deliver 還沒核准", "--run", R("r-pending"))
    r.push("政策核准但有 important 發現", "--run", R("r-forged"))
    r.push("政策核准但 skeptic 推翻", "--run", R("r-skeptic"))
    r.push("政策核准但 agent 缺件", "--run", R("r-lost"))
    r.push("政策核准、結果乾淨", "--run", R("r-clean"), "--dry-run")
    r.push("run 已取消", "--run", R("r-abandon"))
    r.push("spike 拒推", "--run", R("r-spike"))
    r.push("少了有閘門的段", "--run", R("v-no-deliver-design"))
    r.push("merge 段還沒 done", "--run", R("v-merge-pending"), "--stage", "merge")
    r.push("bugfix 的 test 沒核准", "--run", R("v-bugfix-test-unapproved"), "--branch", "feature/code")
    r.push("recipe 被改鬆", "--run", R("v-tamper-stages"))
    r.push("run.json 有 NaN", "--run", R("s-nan"), "--dry-run", label="nan-run")
    r.push("run.json 巢狀太深", "--run", R("s-deep"), "--dry-run", label="deep-run")
    # diff 形狀
    r.push("docs recipe 改了程式", "--run", R("r-docs"), "--branch", "feature/code")
    r.push("沒有 design 卻碰 migration", "--run", R("r-bugfix"), "--branch", "feature/migration")
    r.push("feature 碰 migration（沒有回滾計畫）", "--run", R("r-feature"), "--branch", "feature/migration")
    # 設定
    for note, patch in (("沒有 fork_repo", {"fork_repo": ""}), ("fork_repo 是佔位符", {"fork_repo": "{" * 2 + "FORK_REPO" + "}" * 2}), ("fork_repo 列在上游", {"fork_repo": "acme-up/app"}),
                        ("fork_repo 列在禁止清單（大小寫不同）", {"fork_repo": "ACME-Banned/App"}), ("fork_repo 大小寫不同", {"fork_repo": "Acme-Fork/APP"}),
                        ("前綴用預設（merge/ 等）", {"allowed_branch_prefixes": ""}), ("受保護分支加上 feature/export", {"protected_branches": "main, feature/export"}),
                        ("runs_dir 改地方", {"runs_dir": "/out//audit/"}), ("runs_dir 是空白", {"runs_dir": "  "}),
                        ("upstream_repos 是數字", {"upstream_repos": 5}), ("upstream_repos 是物件", {"upstream_repos": {"acme-fork/app": 1}})):
        r.wj(cfgp, {**base_cfg, **patch})
        r.push(note, "--dry-run")
    r.w(cfgp, "{壞")
    r.push("guard.config.json 壞掉：當成空的", "--dry-run")
    r.wj(cfgp, base_cfg)
    r.push("沒有 guard.config.json 的專案", "--project-dir", "@TMP@/proj2", "--dry-run")
    r.push("最後再確認一次能過", "--dry-run")


def scenario_probes(r):
    for url in ("https://github.com/acme-fork/app.git", "https://github.com/Acme-Fork/App", "git@github.com:acme-fork/app.git", "ssh://git@example.invalid:22/acme-fork/app.git/",
                "  https://example.invalid/a/b/c.git\n", "/srv/git/acme-fork/app.git", "file:///srv/acme-fork/app", "acme-fork/app", ":acme-fork/app", "app.git", "", "https://example.invalid/",
                "https://example.invalid/a/b.git.git", "https://example.invalid/a/.git", "https://example.invalid/a b/c", "https://example.invalid/團隊/專案.git", "https://example.invalid/a/b//",
                "https://example.invalid/a/b?x=1", "x:y/z", "https://example.invalid/a/b.git\nmore"):
        r.call("slug_of", url=url)
    for p in (".env", "a/.env", ".env.local", ".env.example", ".env.sample", ".env.template", "a/.env.example.bak", ".env.", ".envrc", "env", "id_rsa", "a/id_ed25519", "id_rsa.pub", "my_id_rsa",
              "x.pem", "certs/client.p12", "a.pfx", "server.key", "a.keystore", "a.jks", "a.keys", ".pem", "pem", ".npmrc", "a/.pypirc", "npmrc", "credentials", "credentials.json", "a/credentials.json",
              "credentials.yaml", "my-credentials", "", "/dev/null", "x.pem\n", "x.pem\n\n", ".env.example\n", ".env.x-y_z", ".env.a.b", "dir.pem/readme.md", "A.PEM", "鑰匙.key", "a/.env.樣本"):
        r.call("secret_file", path=p)

    def diff(path, *lines):
        return J(f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -0,0 +1,{len(lines)} @@\n", *[J("+", ln, "\n") for ln in lines])

    A36, A60, A20, A32 = rep("a", 36), rep("b", 60), rep("c", 20), rep("d", 32)
    hits = [
        ("AWS", J("AK", "IA", rep("Q", 16))), ("AWS ASIA", J("AS", "IA", rep("7", 16))), ("AWS 太短", J("AK", "IA", rep("Q", 15))), ("AWS 前面黏字", J("xAK", "IA", rep("Q", 16))),
        ("AWS 小寫不算", J("AK", "IA", rep("q", 16))), ("GitHub ghp", J("gh", "p_", A36)), ("GitHub ghs", J("gh", "s_", A36, "zz")), ("GitHub 太短", J("gh", "p_", rep("a", 35))),
        ("GitHub pat", J("github", "_pat_", A60)), ("GitLab", J("gl", "pat-", A20)), ("Slack token", J("xo", "xb", "-", rep("1", 10))), ("Slack webhook", J("https://hooks.", "slack.com/services/", rep("T", 20))),
        ("Google", J("AI", "za", rep("x", 35))), ("Stripe", J("sk", "_live_", A20)), ("Stripe rk", J("rk", "_live_", A20)), ("OpenAI 類", J("sk", "-", A32)), ("Anthropic 類", J("sk", "-", "ant-", A32)),
        ("proj 類", J("sk", "-", "proj-", A32)), ("私鑰", J("-----BEGIN ", "PRIVATE KEY", "-----")), ("RSA 私鑰", J("-----BEGIN RSA ", "PRIVATE KEY", "-----")),
        ("OPENSSH 私鑰", J("-----BEGIN OPENSSH ", "PRIVATE KEY", "-----")), ("公鑰不算", "-----BEGIN PUBLIC KEY-----"), ("JWT", J("ey", "J", rep("a", 10), ".ey", "J", rep("b", 10), ".", rep("c", 10))),
        ("連線字串", J("postgres://user:", "hunter2secret", "@db.example.invalid/app")), ("連線字串（密碼太短）", "postgres://user:abc@db.example.invalid/app"),
        ("連線字串（沒有密碼）", "https://user@example.invalid/x"), ("寫死的密碼", J("password = '", rep("p", 12), "'")), ("寫死的密碼（大寫、雙引號）", J('API_KEY: "', rep("k", 14), '"')),
        ("寫死的密碼（太短）", "password = 'short'"), ("寫死的密碼（沒有引號）", J("password = ", rep("p", 12))), ("access-token", J("access-token\" : \"", rep("t", 12), "\"")),
        ("private_key", J("private_key='", rep("z", 20), "'")), ("\\x1c 當空白", J("secret\x1c=\x1c'", rep("s", 12), "'")), ("全形空白", J("secret\u3000=\u3000'", rep("s", 12), "'")),
        ("結合符號接在前面", J("e\u0301AK", "IA", rep("Q", 16))), ("上標數字接在前面", J("\u00b2AK", "IA", rep("Q", 16))), ("底線接在前面", J("_AK", "IA", rep("Q", 16))),
        ("中文接在前面", J("金鑰AK", "IA", rep("Q", 16))), ("兩種一起", J("AK", "IA", rep("Q", 16), " password='", rep("p", 12), "'")), ("沒有機敏值", "匯出 CSV"),
    ]
    for note, body in hits:
        r.call("scan_secrets", note=note, diff=diff("src/a.py", body))
    r.call("scan_secrets", note="--secrets-file 的值", diff=diff("docs/a.md", "匯出 CSV", "別的"), literals=["匯出", "", "沒有"])
    r.call("scan_secrets", note="--secrets-file 的值都沒出現", diff=diff("docs/a.md", "匯出 CSV"), literals=["沒有"])
    r.call("scan_secrets", note="同一檔同一樣式只報一次、結果排序", diff=J(diff("b.py", KEY, KEY), diff("a.py", KEY)))
    r.call("scan_secrets", note="names 另外比對", diff=diff("docs/a.md", "x"), names=["a/.env", "docs/a.md", "x.pem", ".env.example"])
    r.call("scan_secrets", note="空的 diff", diff="")
    r.call("scan_secrets", note="二進位檔", diff="diff --git a/certs/c.p12 b/certs/c.p12\nnew file mode 100644\nindex 0000000..1111111\nBinary files /dev/null and b/certs/c.p12 differ\n")
    r.call("scan_secrets", note="刪掉的機敏檔不算", diff="diff --git a/.env b/.env\ndeleted file mode 100644\nindex 1111111..0000000\n--- a/.env\n+++ /dev/null\n@@ -1 +0,0 @@\n-A=1\n")
    r.call("scan_secrets", note="改名成機敏檔名", diff="diff --git a/docs/a.txt b/docs/server.key\nsimilarity index 100%\nrename from docs/a.txt\nrename to docs/server.key\n")
    r.call("scan_secrets", note="從機敏檔名改成別的", diff="diff --git a/server.key b/docs/a.txt\nsimilarity index 100%\nrename from server.key\nrename to docs/a.txt\n")
    r.call("scan_secrets", note="引號路徑", diff='diff --git "a/\\351\\221\\260.pem" "b/\\351\\221\\260.pem"\nnew file mode 100644\n')
    r.call("scan_secrets", note="解析不了的標頭", diff="diff --git x y\n+++ z\n")
    r.call("scan_secrets", note="hunk 被截斷", diff="diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,3 @@\n a\n")
    r.call("scan_secrets", note="hunk 外的新增行照樣掃", diff=J("diff --git a/a b/a\n--- a/a\n+++ b/a\n+", KEY, "\n"))
    r.call("scan_secrets", note="不屬於任何檔案的新增行", diff=J("+", KEY, "\n"))
    r.call("scan_secrets", note="沒有前綴的 diff（diff.noprefix）", diff="diff --git docs/a.md docs/a.md\n--- docs/a.md\n+++ docs/a.md\n@@ -0,0 +1 @@\n+x\n")
    r.call("scan_secrets", note="「++ 」開頭的新增行", diff=J("diff --git a/n.md b/n.md\n--- a/n.md\n+++ b/n.md\n@@ -0,0 +1 @@\n+++ ", KEY, "\n"))

    C = {"fork_repo": "acme-fork/app", "upstream_repos": ["acme-up/app"], "forbidden": ["acme-banned/app"], "prefixes": ["feature/", "bugfix/"], "protected": ["main", "master"]}
    for branch, base in (("feature/x", "main"), ("main", "main"), ("master", "main"), ("feature/x", "feature/x"), ("wip/x", "main"), ("", "main"), ("-f", "main"), ("+x", "main"), ("refs/heads/feature/x", "main"),
                         ("feature/a:b", "main"), ("feature/..", "main"), ("feature/", "main"), ("bugfix/甲", "main"), ("Feature/x", "main"), ("feature/x\n", "main")):
        r.call("check_target", cfg=C, branch=branch, base=base)
    for patch in ({"fork_repo": ""}, {"fork_repo": "ACME-UP/app"}, {"fork_repo": "Acme-Banned/APP"}, {"prefixes": []}, {"prefixes": [""]}, {"protected": ["feature/x"]}, {"upstream_repos": [], "forbidden": []}):
        r.call("check_target", cfg={**C, **patch}, branch="feature/x", base="main")
    for urls in (["https://github.com/acme-fork/app.git"], [], ["https://github.com/acme-up/app.git"], ["https://github.com/ACME-FORK/APP"], ["https://github.com/acme-fork/app.git", "https://github.com/x/y.git"],
                 ["https://example.invalid/acme-up/app/../../acme-fork/app.git"], ["https://github.com/Acme-Banned/App.git"], ["weird"], ["https://github.com/acme-fork/app-2.git"],
                 ["git@github.com:acme-fork/app.git", "ssh://example.invalid/acme-fork/app/"]):
        r.call("check_urls", urls=urls, repo="@TMP@/proj/repos/app", remote="origin", cfg=C)
    r.call("check_urls", urls=["https://github.com/acme-fork/app.git"], repo="repo", remote="r", cfg={**C, "fork_repo": "Acme-Fork/App"})
    r.call("check_urls", urls=["https://github.com/acme-fork/app.git"], repo="repo", remote="r", cfg={**C, "upstream_repos": ["github.com"]})

    ph = "{" * 2 + "UPSTREAM_REPOS" + "}" * 2
    configs = [None, "{壞", [], {}, {"fork_repo": "  acme-fork/app \n", "upstream_repos": " a/b , c/d ,, ", "forbidden_paths": "x/y,docs/,z", "allowed_branch_prefixes": ["feature/", " ", 5, None, "fix/"],
                                    "protected_branches": [], "runs_dir": " /out/runs/ "},
               {"fork_repo": 5, "runs_dir": 0}, {"fork_repo": ["a/b"], "runs_dir": ["x"]}, {"fork_repo": None, "runs_dir": None, "upstream_repos": None},
               {"upstream_repos": ph, "forbidden_paths": ["a/b", ph, f"x/{ph}"], "runs_dir": ph}, {"upstream_repos": 5}, {"upstream_repos": True}, {"upstream_repos": 0}, {"upstream_repos": False},
               {"upstream_repos": {"a/b": 1, "c/d": 2}}, {"upstream_repos": [["a/b"], {"c": 1}, "e/f"]}, {"forbidden_paths": ["a/b/c", "a b/c", "團隊/專案", "a/b\n", "/a/b", "a/", "x/y"]},
               {"runs_dir": "///"}, {"runs_dir": "a//b/./c/"}, {"runs_dir": "../out"}, {"allowed_branch_prefixes": 1.5}, {"protected_branches": "main,,release"}, {"fork_repo": True}]
    for i, c in enumerate(configs):
        if c is not None:
            r.w(f"cfgs/{i}/.claude/teams/guard.config.json", c if isinstance(c, str) else json.dumps(c, ensure_ascii=False))
        r.call("load_config", proj=f"@TMP@/cfgs/{i}")
    r.w("cfgs/nan/.claude/teams/guard.config.json", '{"fork_repo": "acme-fork/app", "x": NaN}')
    r.call("load_config", proj="@TMP@/cfgs/nan", label="nan-config")
    for v in ("1", "12", " 7 ", "+3", "1_000", "0", "-1", "", "x", "1.0", "1e3", "0x10", "９", "1__0", "_1", "99999999999999999999999"):
        r.call("positive_int", v=v, **({"label": "supersedes-digits"} if v in ("９", "99999999999999999999999") else {}))
    for n in (1, 12, 99999):
        r.call("supersedes_line", n=n)


def main():
    r = Rec()
    prev = os.getcwd()
    saved_env = dict(os.environ)
    saved_profile = save_run.PROJECT_PROFILE
    selftest = subprocess.run([sys.executable, str(TOOLS / "publish.py"), "--selftest"], capture_output=True, text=True, encoding="utf-8", cwd=str(ROOT / "scaffold"))
    os.environ.update(GIT_ENV)
    os.environ["GIT_CONFIG_GLOBAL"] = r.p("gitconfig")
    os.environ["PATH"] = r.p("bin") + os.pathsep + os.environ.get("PATH", "")
    os.environ["CLAUDE_PROJECT_DIR"] = r.p("proj")
    save_run.PROJECT_PROFILE = Path(r.p("proj/.claude/teams/methodology.json"))
    os.chdir(r.td)
    try:
        fixture(r)
        make_runs(r)
        scenario_policy(r)
        scenario_probes(r)
        scenario_cli(r)
    finally:
        os.chdir(prev)
        os.environ.clear()
        os.environ.update(saved_env)
        save_run.PROJECT_PROFILE = saved_profile
        shutil.rmtree(r.td, ignore_errors=True)
    one = lambda v: json.dumps(v, ensure_ascii=False)  # noqa: E731
    sys.stdout.write("{\n \"說明\": %s,\n \"timestamp\": %s,\n \"git_env\": %s,\n \"selftest\": %s,\n \"steps\": [\n%s\n ]\n}\n" % (
        one("publish 的對跑基準，由 tests/parity/record_publish.py 產生；不要手改。"), one(TS), one(GIT_ENV),
        one({"code": selftest.returncode, "stdout": selftest.stdout}), ",\n".join("  " + one(x) for x in r.steps)))


if __name__ == "__main__":
    main()
