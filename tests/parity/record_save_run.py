#!/usr/bin/env python3
"""錄閘門工具（save_run.py）的對跑基準。

用法：python3 tests/parity/record_save_run.py > tests/parity/save_run.expected.json

基準檔本身就是劇本：`steps` 依序是檔案操作（write、rm、rmtree、mkdir）、指令（run：argv、stdin、exit code、stdout、stderr、
這一步改了哪些檔）與只能在程式內呼叫的函式（call）。`probes` 是不碰檔案的函式探測（含型別不對、缺鍵、空值的輸入）。
Rust 的測試（crates/awf-core/tests/save_run_parity.rs）照同一份劇本做一遍，逐字比對。
每一步的 `note` 寫它對應 save_run.py 的哪個函式、哪個分支。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`（實體路徑與原路徑都換）；劇本開始時把工作目錄切到暫存目錄（Rust 那邊不切，用參數給基準目錄）。
- 時間：save_run.py 的時間戳都從命令列的 `--at` 來，唯一自己取時間的地方是 `record_decisions` 的
  `datetime.date.today()`（重寫 決策記憶.md 時判斷哪些決定過期）。錄的時候把它固定成 TODAY；Rust 那邊用 `Ctx.today` 給同一天。
  run.json、各段結果、執行紀錄索引、結案摘要、decisions.jsonl 都沒有其他時間來源。
- 專案的 profile（save_run.PROJECT_PROFILE）與資料包的專案根目錄指到暫存目錄裡的虛構專案（示範客戶 ACME）。
- 限制文件的檢查指令只用 shell 內建的 exit／echo；逾時（300 秒）的分支沒有錄。save_run.py 不呼叫 git。
- 比這裡的 LONG 長的檔案內容只記 sha1 與長度。
- 用法訊息（指令名改成 `awf save-run`）只比 exit code。Python 沒接住的例外（traceback、exit 1）記類別名：
  Rust 要一樣 exit 1、stderr 提到同一個類別，這一步改到的檔也要相同。函式探測丟例外時只比類別名，不比訊息。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
import copy
import datetime
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
sys.path.insert(0, str(TOOLS))
os.environ.pop("CLAUDE_PROJECT_DIR", None)
import pack  # noqa: E402
import recipe as rcp  # noqa: E402
import save_run  # noqa: E402

LONG = 1500        # 比這長的檔案內容改記 sha1
TODAY = (2026, 11, 12)
AT = "2026-11-12T01:00:00Z"
NS = "agent-workflow:"
PROFILE = "proj/.claude/teams/methodology.json"
BASE = {"recipes": rcp.BUILTIN_RECIPES, "modifiers": rcp.BUILTIN_MODIFIERS}
HUMAN = ("--by", "owner", "--at", AT)
POLICY = ("--by", "policy", "--at", AT, "--note", "政策：全綠自動核准")
EPIC = "@TMP@/proj/repo/.worktrees/e"
MODES = ["feature", "diagnose", "impact", "invariants", "changelog", "research", "baseline", "revert"]


# 探測的參數裡一再出現的大物件：基準檔只存一次（`consts`），參數裡寫成 {"@const": 名稱}
CONSTS = {"RECIPES": rcp.BUILTIN_RECIPES, "MODIFIERS": rcp.BUILTIN_MODIFIERS}
CONSTS.update({f"R:{n}": rcp.resolve(BASE, n) for n in rcp.BUILTIN_RECIPES})


def fold(v):
    """把等於某個常數的子樹換成 {"@const": 名稱}。"""
    if isinstance(v, dict):
        if len(v) > 3:
            for name, c in CONSTS.items():
                if v == c and list(v) == list(c):
                    return {"@const": name}
        return {k: fold(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [fold(x) for x in v]
    return v


class FixedDate(datetime.date):
    @classmethod
    def today(cls):
        return cls(*TODAY)


@contextlib.contextmanager
def fixed_today():
    """record_decisions 在函式裡 import datetime 再取 today：這段期間換成固定日期的替身。"""
    real = sys.modules["datetime"]
    sys.modules["datetime"] = types.SimpleNamespace(date=FixedDate)
    try:
        yield
    finally:
        sys.modules["datetime"] = real


def enc(v):
    """函式的回傳值 → JSON（Path 轉成字串、tuple 轉成清單、set 排序）。"""
    if isinstance(v, Path):
        return str(v).replace("\\", "/")
    if isinstance(v, (list, tuple)):
        return [enc(x) for x in v]
    if isinstance(v, (set, frozenset)):
        return sorted(enc(x) for x in v)
    if isinstance(v, dict):
        return {str(k): enc(x) for k, x in v.items()}
    return v


def P(v):
    return Path(v) if v else None


def env(payload, status="done", **kw):
    """一份合格的信封。"""
    e = {"status": status, "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload}
    e.update(kw)
    return e


def _record_decisions(a):
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        n = save_run.record_decisions(Path(a["run"]), [tuple(x) for x in a["items"]], a["by"], a["at"], a["scope"], a["expires"], a.get("stage") or "")
    return [n, err.getvalue()]


CALLS = {
    "opt": lambda a: save_run.opt(a["argv"], a["name"], a.get("default") or ""),
    "opts": lambda a: save_run.opts(a["argv"], a["name"]),
    "split_answer": lambda a: save_run.split_answer(a["raw"]),
    "decision_errors": lambda a: save_run.decision_errors([tuple(x) for x in a["items"]], a["by"], a["scope"], a["expires"]),
    "is_envelope": lambda a: save_run.is_envelope(a["v"]),
    "envelopes": lambda a: list(save_run.envelopes(a["v"])),
    "missing_agents": lambda a: save_run.missing_agents(a["v"]),
    "md_cell": lambda a: save_run._md_cell(a["v"]),
    "record_candidates": lambda a: save_run.record_candidates(Path(a["run"]), a["stage"], a["record"]),
    "envelope_of": lambda a: save_run._envelope_of(a["entry"], a["stage"], a["envs"]),
    "retried_ok": lambda a: save_run._retried_ok(a["trace"], a["i"]),
    "model_errors": lambda a: save_run.model_errors(a["stage"], a["result"], a.get("profile"), a.get("modifiers")),
    "process_trace": lambda a: save_run.process_trace(Path(a["run"]), a["stage"], a["result"], a["write"]),
    "policy_violations": lambda a: save_run.policy_violations(a["stage"], a["result"], P(a.get("run"))),
    "recipe_mismatch": lambda a: save_run.recipe_mismatch(a["data"], a["result"]),
    "required_roles_missing": lambda a: save_run.required_roles_missing(a["data"], a["stage"], a["result"]),
    "has_recipe": lambda a: save_run.has_recipe(a["data"]),
    "constraint_file": lambda a: save_run.constraint_file(a["c"], P(a.get("repo"))),
    "constraint_rows": lambda a: save_run.constraint_rows(Path(a["run"]), a["stage"]),
    "ac_ids": lambda a: save_run.AC_ID.findall(a["text"]),
    "result_subtasks": lambda a: save_run._result_subtasks(a["result"]),
    "plan_check_issues": lambda a: save_run.plan_check_issues(a["result"], P(a.get("spec"))),
    "plan_check_for": lambda a: save_run._plan_check_for(Path(a["run"]), a["stage"], a["result"], a["data"]),
    "constraint_issues": lambda a: save_run.constraint_issues(Path(a["run"]), a["stage"], a.get("profile"), P(a.get("repo")), a.get("run_checks", True)),
    "load_profile_for": lambda a: save_run.load_profile_for(Path("."), a["argv"]),
    "mods": lambda a: save_run._mods(a["v"]),
    "recipe_summary": lambda a: save_run.recipe_summary(a["rec"]),
    "intent_errors": lambda a: save_run.intent_errors(a["result"], a.get("profile"), a["plan_mode"]),
    "recipe_tamper_errors": lambda a: save_run.recipe_tamper_errors(a["data"], ["--profile", a["profile"]] if a.get("profile") else []),
    "followup_run_errors": lambda a: save_run.followup_run_errors(Path(a["frun"])),
    "chain_errors": lambda a: save_run.chain_errors(a["data"], a["stage"]),
    "abandoned_error": lambda a: save_run.abandoned_error(a["data"]),
    "escalate_affected": lambda a: save_run._escalate_affected(a["new"], a["old"], a.get("profile")),
    "next_stage": lambda a: save_run.next_stage(a["data"]),
    "clip": lambda a: save_run._clip(a["s"], a["n"]),
    "archive_text": lambda a: save_run.archive_text(Path(a["run"]), a["data"]),
    "write_archive": lambda a: save_run.write_archive(Path(a["run"]), a["data"]),
    "status_snapshot": lambda a: save_run.status_snapshot(Path(a["run"]), a["data"]),
    "load": lambda a: save_run.load(Path(a["run"])),
    "record_decisions": _record_decisions,
}


def tried(fn, a):
    try:
        with contextlib.redirect_stdout(io.StringIO()), fixed_today():
            return {"ok": enc(CALLS[fn](a))}
    except RecursionError:
        raise
    except Exception as e:  # noqa: BLE001
        return {"raises": type(e).__name__}


class Rec:
    def __init__(self):
        self.td = tempfile.mkdtemp(prefix="save-run-parity-")
        self.real = os.path.realpath(self.td)
        self.steps = []
        self.n = 0

    def norm(self, s):
        return s.replace(self.real, "@TMP@").replace(self.td, "@TMP@")

    def p(self, rel):
        return os.path.join(self.td, rel)

    def abs(self, v):
        if isinstance(v, str):
            return v.replace("@TMP@", self.td)
        if isinstance(v, (list, tuple)):
            return [self.abs(x) for x in v]
        if isinstance(v, dict):
            return {k: self.abs(x) for k, x in v.items()}
        return v

    def read_json(self, d, name="run.json"):
        return json.loads(Path(self.abs(d), name).read_text(encoding="utf-8"))

    # ---- 檔案操作
    def write(self, rel, text="", raw=None):
        """raw（bytes）＝不是合法 utf-8 的內容：劇本裡用十六進位存。內容裡的 @TMP@ 會換成暫存目錄。"""
        rel = rel.replace("@TMP@/", "")
        step = {"op": "write", "path": rel}
        if raw is not None:
            step["hex"] = raw.hex()
            data = raw
        else:
            text = self.norm(text)
            step["text"] = text
            data = self.abs(text).encode("utf-8")
        self.steps.append(step)
        os.makedirs(os.path.dirname(self.p(rel)), exist_ok=True)
        with open(self.p(rel), "wb") as f:
            f.write(data)

    def write_json(self, rel, obj):
        self.write(rel, json.dumps(obj, ensure_ascii=False))

    def rm(self, rel):
        rel = rel.replace("@TMP@/", "")
        self.steps.append({"op": "rm", "path": rel})
        os.unlink(self.p(rel))

    def rmtree(self, rel):
        rel = rel.replace("@TMP@/", "")
        self.steps.append({"op": "rmtree", "path": rel})
        shutil.rmtree(self.p(rel))

    def mkdir(self, rel):
        rel = rel.replace("@TMP@/", "")
        self.steps.append({"op": "mkdir", "path": rel})
        os.makedirs(self.p(rel), exist_ok=True)

    def profile(self, obj):
        """換掉專案的 profile（None＝拿掉）。"""
        if obj is None:
            if os.path.exists(self.p(PROFILE)):
                self.rm(PROFILE)
        else:
            self.write_json(PROFILE, obj)

    def snapshot(self):
        out = {}
        for base, dirs, files in os.walk(self.td):
            dirs.sort()
            for f in sorted(files):
                full = os.path.join(base, f)
                rel = os.path.relpath(full, self.td).replace(os.sep, "/")
                text = self.norm(Path(full).read_bytes().decode("utf-8", errors="replace"))
                out[rel] = text if len(text) <= LONG else {"sha1": hashlib.sha1(text.encode("utf-8")).hexdigest(), "chars": len(text)}
        return out

    def changed(self, before):
        after = self.snapshot()
        return {k: after.get(k) for k in sorted(set(before) | set(after)) if before.get(k) != after.get(k)}

    # ---- 指令
    def sr(self, *argv, stdin=None, raw=None, label=None, note=""):
        """跑一次 save_run 的命令列。stdin 是要給 stage 的結果（物件）；raw 是原樣的 stdin 文字。
        label：已知和 Rust 不同的步驟（見 save_run_parity.rs 的 STRICTER／KNOWN）。"""
        before = self.snapshot()
        shown = raw if raw is not None else ("" if stdin is None else json.dumps(stdin, ensure_ascii=False))
        real = [self.abs(a) for a in argv]
        out, err = io.StringIO(), io.StringIO()
        step = {"op": "run", "argv": list(argv), "stdin": shown, "note": note}
        if label:
            step["label"] = label
        old_in = sys.stdin
        sys.stdin = io.StringIO(self.abs(shown))
        try:
            if real == ["--selftest"]:
                # selftest 用獨立的行程跑：錄製時改過的專案 profile、資料包的專案根、固定日期不能影響它
                p = subprocess.run([sys.executable, str(TOOLS / "save_run.py"), "--selftest"], capture_output=True, text=True, encoding="utf-8", cwd=str(ROOT / "scaffold"))
                code = p.returncode
                out.write(p.stdout)
                err.write(p.stderr)
            else:
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), fixed_today():
                    try:
                        code = save_run.main(real)
                    except Exception as e:  # noqa: BLE001  save_run.py 沒接住的例外：命令列會印 traceback、exit 1
                        code = 1
                        step["raises"] = type(e).__name__
        finally:
            sys.stdin = old_in
        stdout = self.norm(out.getvalue())
        if code == 2 and stdout.startswith(save_run.__doc__[:30]):
            step["usage"] = True
            stdout = ""
        step.update({"code": code, "stdout": stdout, "stderr": "" if step.get("raises") else self.norm(err.getvalue()), "files": self.changed(before)})
        self.steps.append(step)
        return code

    def call(self, fn, label=None, note="", **kw):
        before = self.snapshot()
        kw = json.loads(self.norm(json.dumps(kw, ensure_ascii=False)))
        res = json.loads(self.norm(json.dumps(tried(fn, self.abs(kw)), ensure_ascii=False)))
        step = {"op": "call", "fn": fn, "args": fold(kw), "note": note, "result": res, "files": self.changed(before)}
        if label:
            step["label"] = label
        self.steps.append(step)
        return res.get("ok")

    # ---- 劇本的積木
    def new(self, name=None, event="sdlc", target="@TMP@/proj/repo", req="匯出報表要有 CSV"):
        self.n += 1
        d = f"@TMP@/proj/_runs/{name or 'r'}{self.n}"
        self.sr("init", d, "--event", event, "--target", target, "--requirement", req, note="init：建立 run.json 與 context.md")
        return d

    def entry(self, d, stage, label, role, write=True, **extra):
        """一筆 trace，並寫好它的紀錄檔。"""
        f = f"{d}/{stage}/{label.replace(':', '-').replace('#', '-')}.md"
        if write:
            self.write(f, "# 紀錄\n")
        return {"label": label, "agent_type": NS + role, "phase": stage.capitalize(), "record": f, "ok": True, **extra}

    def echo(self, d):
        """workflow 回傳裡照抄 run.json 鎖定的 recipe。"""
        rec = self.read_json(d).get("recipe")
        if not isinstance(rec, dict):
            return None
        return {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]}

    def plan(self, d, recipe=None, intent=None, trace=True, **extra):
        mode = rcp.resolve(BASE, (recipe or {}).get("name") or "feature", save_run._mods((recipe or {}).get("modifiers")))["plan_mode"] if isinstance(recipe, dict) or recipe is None else "feature"
        r = {"stage": "plan", "epic_dir": EPIC,
             "intent": env({"documents": [{"path": intent or f"specs/e/intent-{mode}.md", "kind": "intent", "audience": "owner"}]})}
        if recipe is not None:
            r["recipe"] = recipe
        if trace:
            r["trace"] = [self.entry(d, "plan", "intent", "dev-spec-author")]
        r.update(extra)
        return r

    def result(self, d, stage, **extra):
        """某一段「全部合格」的回傳（政策核准會過）。"""
        rec = self.read_json(d).get("recipe") or {}
        res = rec.get("resolved") or rcp.resolve(None, "feature")
        if stage == "design":
            r = {"design": env({"epic": "e", "subtasks": [SUB]}), "challenge": env({"verdicts": [{"target_id": "x", "refuted": False, "reason": "成立"}]}),
                 "trace": [self.entry(d, stage, "outline", "dev-spec-author"), self.entry(d, stage, "challenge", "tpm-skeptic")]}
        elif stage == "build":
            r = {"implement": [env({"task_id": "t1", "branch": "feat/e-t1", "commits": [], "files_changed": [], "tests": []})],
                 "trace": [self.entry(d, stage, "Implement:t1", "dev-implementer"), self.entry(d, stage, "integrate", "dev-integrator", write=False, ok=False, superseded=True),
                           self.entry(d, stage, "integrate", "dev-integrator", attempt=2)]}
        elif stage == "sync":
            r = {"sync": env({"task_id": "sync", "branch": "feat/e", "commits": [], "files_changed": [], "tests": []}), "trace": [self.entry(d, stage, "sync", "dev-integrator")]}
        elif stage == "test":
            t = res.get("test") or {}
            r = {"verify": env({"commit": "a" * 40, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
                                "criteria": [{"id": "C1", "criterion": "c", "command": "x", "result": "pass", "evidence": "e"}]}),
                 "head": "a" * 40, "trace": [self.entry(d, stage, "verify#1", "dev-verifier")]}
            for key, role in save_run.REVIEW_ROLES.items():
                if t.get(key) == "always":
                    r[key] = env({"lens": key, "findings": []})
                    r["trace"].append(self.entry(d, stage, key, role))
        elif stage == "deliver":
            axes = res.get("deliver_axes") or []
            r = {"reviews": [env({"lens": a, "findings": [{"id": f"F-{a}", "severity": "nit", "claim": "小建議", "evidence": []}]}) for a in axes],
                 "trace": [self.entry(d, stage, f"review:{a}", "tpm-reviewer") for a in axes] or [self.entry(d, stage, "writer", "tpm-writer")]}
            if not axes:
                r["summary_doc"] = env({"documents": []})
        else:
            md = env(MERGE, evidence=[{"claim": "gate 判 merge", "source": "merge_gate.py", "result": "exit 0"}])
            r = {"analysis": md, "execute": md, "trace": [self.entry(d, stage, "analyze", "dev-merge-steward", write=False), self.entry(d, stage, "execute:merge", "dev-merge-steward", write=False)]}
        r = {"stage": stage, **r}
        if self.echo(d):
            r["recipe"] = self.echo(d)
        r.update(extra)
        return r

    def advance(self, d, upto, by=HUMAN):
        """把 run 從目前的位置往前走到 upto 之前（upto 這一段不存）。"""
        data = self.read_json(d)
        want = (data.get("recipe") or {}).get("resolved", {}).get("stages") or save_run.SDLC
        for st in save_run.SDLC:
            if st == upto:
                return
            cur = data["stages"].get(st) or {}
            if st not in want or (cur.get("status") == "done" and (cur.get("approved") or not cur.get("gate"))):
                continue
            if cur.get("status") != "done":
                self.sr("stage", d, st, stdin=self.plan(d, None if data.get("recipe") is None and st == "plan" and not data.get("_r") else None) if st == "plan" else self.result(d, st), note=f"stage {st}：合格的回傳")
            if st in save_run.GATED:
                self.sr("approve", d, st, *by, note=f"approve {st}")
            data = self.read_json(d)

    def ready(self, upto, recipe=None, name=None):
        """開一個新的 run：plan 用 recipe 存好、人核准，再走到 upto 之前。"""
        d = self.new(name)
        self.sr("stage", d, "plan", stdin=self.plan(d, recipe), note="stage plan：記下 recipe（工具重算 resolved）")
        if upto != "plan-approve":
            self.sr("approve", d, "plan", *HUMAN, note="approve plan：人核准，recipe 鎖定")
            self.advance(d, upto)
        return d


SUB = {"id": "t1", "title": "匯出 CSV", "kind": "code", "owns": ["src/export.py"], "depends_on": [], "acceptance": ["AC-1 下載 CSV"]}
MERGE = {"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": "a" * 40, "traceability": [], "out_of_scope": [], "followups": [], "risk_tier": "low",
         "gate_reasons": [], "merge_state": "merged"}


def fixture(w):
    """虛構專案（示範客戶 ACME）：profile 用內建 recipes；每種 plan_mode 一份節齊全的 intent。"""
    w.profile(BASE)
    w.write("proj/README.md", "# 示範客戶 ACME 的報表系統\n")
    w.write("proj/ARCHITECTURE.md", "# 架構\n\n分層：api → service → repository。\n")
    w.write("proj/repo/src/export.py", "def export():\n    return 'csv'\n")
    for mode in MODES:
        body = "".join(f"## {s}\n{s}的內容 AC-1\n" for s in rcp.required_sections(BASE, mode))
        w.write(f"proj/repo/.worktrees/e/specs/e/intent-{mode}.md", f"# intent（{mode}）\n{body}")


def scenario_cli(w):
    w.sr(note="main：參數不夠 → 用法、exit 2")
    w.sr("status", note="main：只有指令沒有 run 目錄 → 用法、exit 2")
    w.sr("--selftest", note="selftest：項目名稱與數量")
    w.sr("status", "@TMP@/proj/_runs/沒有", note="load：run.json 不存在 → 沒接住的 FileNotFoundError")
    d = w.new("cli")
    w.sr("init", d, "--event", "sdlc", note="init：run.json 已存在 → 不覆寫")
    w.sr("bogus", d, note="main：不認得的指令 → 用法、exit 2")
    w.sr("stage", d, note="stage：沒給段名 → 用法、exit 2")
    w.sr("approve", d, note="approve：沒給段名 → 用法、exit 2")
    w.sr("status", d, note="status：剛 init（沒有 recipe）")
    w.sr("status", d, "--json", note="status --json：剛 init")
    w.sr("init", "proj/_runs/rel", "--event", "sdlc", "--target", "repo", note="init：相對路徑的 run 目錄（以目前目錄為基準）")
    w.sr("status", "proj/_runs/rel", note="status：相對路徑")
    w.sr("init", "@TMP@/proj/_runs/bare", note="init：沒給 --event（不是 sdlc → 只有 run 一段）、沒給 target 與需求")
    w.sr("status", "@TMP@/proj/_runs/bare", note="status：非 SDLC run")
    w.write("proj/_runs/blocker", "佔位")
    w.sr("init", "@TMP@/proj/_runs/blocker/x", "--event", "sdlc", note="init：上層是檔案 → mkdir 沒接住的例外")
    # 非 SDLC 事件：只有 run 一段、沒有閘門
    d = w.new("meeting", event="meeting")
    w.sr("stage", d, "run", stdin={"minutes": env({"documents": []})}, note="stage：非 SDLC run 的 run 段（不看 recipe、沒有 trace 只警告）")
    w.sr("approve", d, "run", *HUMAN, note="approve：非 SDLC 段（chain_errors 回空）")
    w.sr("approve", d, "run", *POLICY, note="approve --by policy：非 SDLC 段")
    w.sr("status", d, note="status：非 SDLC run 走完")
    w.sr("status", d, "--json", note="status --json：非 SDLC run")
    w.sr("close", d, *HUMAN, note="close：非 SDLC run 當成 feature 判閘門鏈 → 缺段")
    w.sr("escalate", d, "--recipe", "feature", *HUMAN, "--reason", "x", note="escalate：不是 SDLC run")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}, trace=False), note="stage plan：非 SDLC run（sdlc_run=False，不記 recipe）")


def lifecycle(w, name, mods=(), by=POLICY, answers=()):
    """一個 recipe 的完整流程：init → plan（人核准）→ 各段（政策核准）→ merge →（hotfix 要後續 run）→ close → status／archive。"""
    d = w.new(f"life-{name}-")
    rec = {"name": name, "modifiers": list(mods), "size": "small", "reasons": [{"claim": "分類依據", "evidence": "需求原文"}]}
    w.sr("stage", d, "plan", stdin=w.plan(d, rec), note=f"stage plan：recipe {name}＋{list(mods)}，rebuild_stages 標 skipped")
    w.sr("status", d, note="status：plan 等核准")
    w.sr("approve", d, "plan", *HUMAN, *answers, note="approve plan：人核准、鎖定 recipe" + ("、記決定" if answers else ""))
    res = rcp.resolve(BASE, name, list(mods), "small")
    for st in res["stages"][1:]:
        w.sr("stage", d, st, stdin=w.result(d, st), note=f"stage {st}：{name} 的合格回傳")
        if st in save_run.GATED:
            w.sr("approve", d, st, *by, note=f"approve {st}：{'政策' if by is POLICY else '人'}核准")
    if res.get("followup"):
        w.sr("close", d, *HUMAN, note="close：recipe 要後續 run，還沒有 followup_run")
        f = w.ready("build", {"name": "bugfix"}, name="follow-")
        w.sr("followup", d, "--run", f, *HUMAN, note="followup：記錄後續 run")
    w.sr("close", d, *HUMAN, note="close：流程走完 → 結案並寫摘要")
    w.sr("status", d, note="status：已結案")
    w.sr("status", d, "--json", note="status --json：closed")
    w.sr("archive", d, note="archive：重寫摘要（內容相同）")
    return d


def scenario_lifecycle(w):
    for name in rcp.BUILTIN_RECIPES:
        lifecycle(w, name, answers=("--answer", "匯出檔的編碼用什麼？=UTF-8 不加 BOM", "--answer-scope", "project") if name == "feature" else ())
    lifecycle(w, "feature", ["ui", "security"])
    lifecycle(w, "bugfix", ["migration"], by=HUMAN)
    lifecycle(w, "docs", ["public_api"])
    lifecycle(w, "feature", ["large"], by=HUMAN)
    # 沒有 profile（內建 fallback）也走一次
    w.profile(None)
    lifecycle(w, "feature")
    lifecycle(w, "bugfix")
    w.profile(BASE)


def scenario_stage_input(w):
    """stage 的 stdin：不是 JSON、特殊數值、落單的代理字元、超大整數、不是物件。"""
    d = w.ready("design")
    good = json.dumps(w.result(d, "design"), ensure_ascii=False)
    for raw, note in [("", "空的"), ("不是 JSON", "不是 JSON"), ("[1, 2", "斷掉的 JSON"), ("﻿{}", "開頭有 BOM"), ("{} 多的", "後面多了東西")]:
        w.sr("stage", d, "design", raw=raw, note=f"stage：stdin {note} → 沒接住的 JSONDecodeError，不寫檔")
    w.sr("stage", d, "design", raw=good.replace('"summary": "s"', '"summary": "\\ud83d\\ude00"', 1), note="stage：成對的代理字元跳脫是正常的字")
    w.sr("stage", d, "design", raw=good.replace('"confidence": 0.9', '"confidence": 0.9, "pr": 18446744073709551615', 1), note="stage：64 位元無號整數的上限照常")
    w.sr("stage", d, "design", raw='{"a": ' * 150 + "null" + "}" * 150, note="stage：巢狀 150 層照常（零信封、缺件）")
    w.sr("stage", d, "design", raw='{"x": 1, "x": ' + json.dumps(env({"a": 1})) + "}", note="stage：重複的鍵取後面那個")
    # 下面這些 Python 和 Rust 的反應不同（Rust 更嚴）：各開一個新的 run，不影響後面的步驟
    conf, summ = '"confidence": 0.9', '"summary": "s"'
    odd = [(conf, '"confidence": NaN', "nan-result", "結果裡有 NaN（Python 的 json 讀得進來）"),
           (conf, '"confidence": Infinity', "nan-result", "結果裡有 Infinity"),
           (conf, '"confidence": -Infinity', "nan-result", "結果裡有 -Infinity"),
           (conf, '"confidence": 1e999', "nan-result", "結果裡有 1e999（Python 讀成無限大）"),
           (summ, '"summary": "\\ud800"', "surrogate-result", "結果裡有落單的代理字元跳脫（Python 寫檔時 UnicodeEncodeError）"),
           (conf, '"confidence": 0.9, "pr": 123456789012345678901234567890', "huge-int-result", "結果裡有超出 64 位元的整數"),
           (conf, '"confidence": 0.9, "pr": -99999999999999999999', "huge-int-result", "結果裡有超出 64 位元的負整數"),
           (None, "[" * 300 + "]" * 300, "deep-result", "巢狀 300 層（Python 還讀得進來）")]
    for old, new, label, note in odd:
        d2 = w.ready("design")
        raw = json.dumps(w.result(d2, "design"), ensure_ascii=False).replace(old, new, 1) if old else new
        w.sr("stage", d2, "design", raw=raw, label=label, note=f"stage：{note}")
    for res, note in [([], "空陣列"), ("x", "字串"), (None, "null"), (5, "數字"), (True, "true"), ([env({"a": 1})], "陣列裡有信封"), ({"a": [env({"a": 1}), None]}, "物件裡有信封與 null"),
                      ({}, "空物件")]:
        w.sr("stage", d, "design", stdin=res, note=f"stage：回傳是{note}（不是物件的分支：不比 recipe、不查模型、halted 當成沒有）")
        w.sr("approve", d, "design", *POLICY, note=f"approve --by policy：上一步（{note}）的結果")
    w.sr("status", d, note="status：design 重存多次之後")


def scenario_stage_envelopes(w):
    """信封的 schema、段的狀態（invalid／needs_decision／blocked／done）、halted。"""
    d = w.ready("design")
    ok = w.result(d, "design")

    def variant(note, **change):
        r = copy.deepcopy(ok)
        r["design"].update(change)
        w.sr("stage", d, "design", stdin=r, note=f"stage：{note}")

    variant("信封 status 不在列舉裡 → schema 錯、invalid", status="weird")
    variant("信封 confidence 超過 1 → invalid", confidence=2)
    variant("信封 artifacts 不是陣列 → invalid", artifacts="x")
    variant("信封 payload 是字串 → invalid", payload="x")
    variant("信封 evidence 的項目格式不對 → invalid", evidence=[{"claim": 1}])
    variant("信封 open_questions 的項目格式不對 → invalid", open_questions=["x"])
    variant("信封 summary 太長 → invalid", summary="長" * 5000)
    r = copy.deepcopy(ok)
    del r["design"]["summary"]
    w.sr("stage", d, "design", stdin=r, note="stage：信封少了 summary → invalid")
    variant("有信封 status=needs_decision → 段是 needs_decision", status="needs_decision")
    w.sr("approve", d, "design", *HUMAN, note="approve：狀態 needs_decision 不能核准")
    variant("有信封 status=blocked → 段是 blocked", status="blocked")
    w.sr("status", d, "--json", note="status --json：blocked")
    variant("有信封 status=partial → 仍是 done", status="partial")
    for halted, reason, note in [(True, "子計劃的範圍重疊", "halted=true 帶 halt_reason"), (True, None, "halted=true 沒有 halt_reason"), ("步驟對不上", None, "halted 是字串"),
                                 (False, "x", "halted=false 不算"), (0, None, "halted=0 不算"), ("", None, "halted 空字串不算"), ([], None, "halted 空陣列不算"),
                                 ([1], ["a"], "halted 是陣列、halt_reason 是陣列"), ({"k": 1}, 5, "halted 是物件、halt_reason 是數字")]:
        r = copy.deepcopy(ok)
        r["halted"] = halted
        if reason is not None:
            r["halt_reason"] = reason
        w.sr("stage", d, "design", stdin=r, note=f"stage：{note}")
        w.sr("approve", d, "design", *POLICY, note=f"approve --by policy：{note}")
    w.sr("stage", d, "design", stdin=ok, note="stage：合格 → done（之前的 halted 欄位清掉）")
    w.sr("approve", d, "design", *HUMAN, note="approve：人核准")
    w.sr("stage", d, "design", stdin=ok, note="stage：重存 → 清掉舊核准並說明")
    w.sr("stage", d, "extra", stdin={"x": env({"a": 1})}, note="stage：SDLC run 存一個不在 SDLC 的段名（setdefault 新段、沒有閘門）")
    w.sr("approve", d, "extra", *HUMAN, note="approve：不在 SDLC 的段（chain_errors 回空）")
    w.sr("approve", d, "nope", *HUMAN, note="approve：沒有這一段")
    w.sr("status", d, note="status：多了一個自訂段")
    # 舊 run（沒有 recipe）：回傳沒有 trace 只警告
    d = w.new("legacy")
    w.sr("stage", d, "plan", stdin=w.plan(d, None, trace=False), note="stage plan：回傳沒有 recipe（舊 workflow）→ 當 feature、警告沒有 trace")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：沒有 recipe 就不鎖定")
    w.sr("stage", d, "design", stdin={"design": env({"epic": "e", "subtasks": [SUB]})}, note="stage design：舊 run、沒有 trace → 警告、不產生索引")
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：舊 run 沒有 trace 不擋（沒有 recipe）")
    w.sr("stage", d, "build", stdin={"x": env({"a": 1})}, note="stage build：舊 run")
    w.sr("stage", d, "test", stdin={"v": env({"criteria": [{"id": "C1", "result": "pass"}]})}, note="stage test：舊 run 沒有 sync 段的結果")
    w.sr("approve", d, "test", *HUMAN, note="approve test：sync 還 pending → 閘門鏈擋（run.json 有 sync 段）")
    data = w.read_json(d)
    data["stages"].pop("sync")
    w.write_json(f"{d}/run.json", data)
    w.sr("approve", d, "test", *HUMAN, note="chain_errors：沒有 recipe 的舊 run 容許缺 sync")
    w.sr("status", d, note="status：舊 run（recipe 沒有記錄）")
    w.sr("close", d, *HUMAN, note="close：舊 run 流程沒走完")


def scenario_plan_recipe(w):
    """stage plan 的 recipe：名稱、modifier、鎖定、舊 run 補記。"""
    cases = [({"name": "nope"}, "不認得的 recipe → 當 feature（requested 記原名）"), ({"kind": "bugfix"}, "只有 kind"), ({"name": "", "kind": "docs"}, "name 空、用 kind"),
             ({"name": "feature", "modifiers": "ui, security"}, "modifiers 是逗號字串"), ({"name": "feature", "modifiers": ["zzz"]}, "不認得的 modifier → invalid"),
             ({"name": "feature", "modifiers": ["ui", "ui", " "]}, "modifiers 重複與空白"), ({"name": "feature", "size": "large"}, "size"),
             ({"name": "feature", "reasons": "不是陣列"}, "reasons 不是陣列 → 記成空陣列"), ({"name": "feature", "reasons": [{"claim": "c"}]}, "reasons 照記"),
             ({}, "recipe 是空物件"), ({"name": 5}, "name 是數字"), ({"name": ["feature"]}, "name 是陣列"), ({"name": "feature", "modifiers": 5}, "modifiers 是數字"),
             ({"name": "feature", "modifiers": {"ui": 1}}, "modifiers 是物件"), ({"name": "feature", "modifiers": [1, None, "ui"]}, "modifiers 混型別"),
             ({"name": "feature", "size": 5}, "size 是數字"), ({"name": "feature", "size": ["s"]}, "size 是陣列")]
    for rec, note in cases:
        d = w.new("rec")
        r = w.plan(d, {"name": "feature"})
        r["recipe"] = rec
        w.sr("stage", d, "plan", stdin=r, note=f"stage plan：{note}")
        w.sr("status", d, note=f"status：{note}")
    for rec, note in [("feature", "recipe 是字串"), (["feature"], "recipe 是陣列"), (5, "recipe 是數字"), (None, "recipe 是 null")]:
        d = w.new("rec")
        r = w.plan(d, {"name": "feature"})
        r["recipe"] = rec
        w.sr("stage", d, "plan", stdin=r, note=f"stage plan：{note} → 當成沒有 recipe")
    # 鎖定
    d = w.ready("design", {"name": "bugfix", "modifiers": ["ui"], "size": "small"})
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix", "modifiers": ["ui"], "size": "small"}), note="stage plan：鎖定後重存同一個 recipe → 可以（清掉核准）")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：重新核准")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "feature"}), note="stage plan：鎖定後換 recipe → invalid（plan 已核准過）")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix", "modifiers": ["ui"], "size": "large"}), note="stage plan：鎖定後換 size → invalid")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix", "modifiers": [], "size": "small"}), note="stage plan：鎖定後拿掉 modifier → invalid")
    d = w.ready("plan-approve", {"name": "bugfix"})
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}), note="stage plan：還沒核准（沒鎖定）→ 可以換 recipe")
    w.sr("escalate", d, "--recipe", "bugfix", *HUMAN, "--reason", "其實要改程式", note="escalate：plan 還沒核准時升級（history 記一筆、locked 仍是 false）")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}), note="stage plan：escalate 過（有 history）→ 不能換 recipe")
    # 舊 run 補記 recipe（X3 M4）
    d = w.new("legacy")
    w.sr("stage", d, "plan", stdin=w.plan(d, None, trace=False), note="stage plan：舊 run")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：舊 run")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}), note="stage plan：舊 run 核准過 plan 之後補記 docs（少段）→ 拒絕")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：上一步 invalid")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "change", "modifiers": ["security"]}), note="stage plan：舊 run 補記不比 feature 少段的 recipe → 記 merge_stricter 的結果")
    d = w.new("legacy")
    w.sr("stage", d, "plan", stdin=w.plan(d, None, trace=False), note="stage plan：舊 run")
    w.sr("stage", d, "design", stdin={"design": env({"epic": "e", "subtasks": [SUB]})}, note="stage design：舊 run 沒核准 plan 就存了 design")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix"}), note="stage plan：舊 run 已經往下跑（design 不是 pending）→ 補記少段的 recipe 被拒")
    d = w.new("legacy")
    w.sr("stage", d, "plan", stdin=w.plan(d, None, trace=False), note="stage plan：舊 run")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix"}), note="stage plan：舊 run 還沒核准、後面都 pending → 可以補記任何 recipe")
    # recipe 沒有的段
    d = w.ready("build", {"name": "bugfix"})
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage：recipe 沒有 design 段 → 只存檔、判錯、run.json 仍是 skipped")
    w.sr("approve", d, "design", *HUMAN, note="approve：skipped 的段不能核准")
    d = w.ready("build", {"name": "spike"})
    w.sr("stage", d, "build", stdin={"x": env({"a": 1})}, note="stage：spike 沒有 build 段")
    w.sr("close", d, *HUMAN, note="close：spike 只有 plan，核准後可以結案")


def scenario_intent(w):
    """intent_errors 的每個分支。"""
    sec = "".join(f"## {s}\n內容\n" for s in rcp.required_sections(BASE, "feature"))
    w.write("proj/repo/.worktrees/e/specs/e/no-ac.md", "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n")
    w.write("proj/repo/.worktrees/e/specs/e/empty.md", "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC-1\n")
    w.write("proj/repo/.worktrees/e/specs/e/numbered.md", "# intent\n## 1. 目標\nx\n### 1.1 範圍\nx\n## 2. 不做的事\nx\n## 3. 驗收條件\n- AC-1\n")
    w.write("proj/repo/.worktrees/e/specs/e/latin1.md", raw=b"# intent\n## \xa5\xd8\xbc\xd0\n\xff\xfe\n")
    w.write("proj/abs-intent.md", f"# intent\n{sec}")
    w.write("rel-intent.md", f"# intent\n{sec}")

    def docs(*ds, **extra):
        d = w.new("intent")
        r = w.plan(d, {"name": "feature"}, **extra)
        r["intent"]["payload"]["documents"] = list(ds)
        return d, r

    def doc(path, kind="intent"):
        return {"path": path, "kind": kind, "audience": "owner"}

    for ds, note in [((doc("specs/e/沒有.md"),), "intent 檔不存在"), ((doc("x.md", "spec"),), "沒有 kind=intent 的文件"), ((), "documents 是空的"), ((doc("  "),), "path 只有空白"),
                     ((doc("specs/e/no-ac.md"),), "少了驗收條件"), ((doc("specs/e/empty.md"),), "節只有標題"), ((doc("specs/e/numbered.md"),), "標題帶編號也算"),
                     ((doc("specs/e/latin1.md"),), "intent 檔不是 utf-8 → 讀不到"), ((doc("@TMP@/proj/abs-intent.md"),), "絕對路徑"), ((doc("rel-intent.md"),), "epic_dir 底下沒有、目前目錄有"),
                     ((doc("specs/e/intent-feature.md"), doc("specs/e/沒有.md"), doc("specs/e/intent-feature.md")), "多份 intent：重複的只看一次、缺的照報"),
                     ((doc(" specs/e/intent-feature.md "),), "path 前後空白會去掉"), (({"path": 5, "kind": "intent"}, "x", None), "documents 的項目格式不對")]:
        d, r = docs(*ds)
        w.sr("stage", d, "plan", stdin=r, note=f"intent_errors：{note}")
    d, r = docs(doc("specs/e/intent-feature.md"))
    del r["epic_dir"]
    w.sr("stage", d, "plan", stdin=r, note="intent_errors：回傳沒有 epic_dir → 相對路徑只以目前目錄為基準、找不到")
    d, r = docs(doc("specs/e/intent-feature.md"), epic_dir=5)
    w.sr("stage", d, "plan", stdin=r, note="intent_errors：epic_dir 不是字串")
    d, r = docs(doc("specs/e/intent-feature.md"))
    r["intent"]["payload"]["documents"] = "不是陣列"
    w.sr("stage", d, "plan", stdin=r, note="intent_errors：documents 是字串（Python 逐字走過、沒有 intent）")
    d, r = docs(doc("specs/e/intent-feature.md"))
    r["intent"]["payload"] = ["不是物件"]
    w.sr("stage", d, "plan", stdin=r, note="intent_errors：payload 不是物件")
    for name, mode in [("bugfix", "diagnose"), ("change", "impact"), ("refactor", "invariants"), ("deps", "changelog"), ("docs", "research"), ("perf", "baseline"), ("revert", "revert")]:
        d = w.new("intent")
        w.sr("stage", d, "plan", stdin=w.plan(d, {"name": name}, intent="specs/e/intent-feature.md"), note=f"intent_errors：plan_mode={mode} 專屬的節沒有 → invalid")


def scenario_profile(w):
    """load_profile_for：--profile 與專案 profile 的每個分支。"""
    w.write_json("proj/p-sections.json", {**BASE, "stages": {"plan": {"required_sections": ["問題"]}}})
    w.write_json("proj/p-modes.json", {**BASE, "plan_modes": {"feature": {"sections": ["風險"]}}})
    w.write("proj/p-broken.json", "{不是 JSON")
    w.write("proj/p-list.json", "[1]")
    w.write_json("proj/p-rules.json", {"recipes": {"x": {**rcp.BUILTIN_RECIPES["docs"], "stages": ["build", "deliver", "merge"]}}})
    w.write("proj/p-nan.json", '{"recipes": null, "x": NaN}')
    w.write("proj/p-latin1.json", raw=b'{"a": "\xff"}')
    w.mkdir("proj/p-dir.json")
    for f, note in [("p-sections.json", "覆寫必要節"), ("p-modes.json", "plan_modes 加節"), ("沒有.json", "不存在 → fail-closed"), ("p-broken.json", "不是 JSON"), ("p-list.json", "不是物件"),
                    ("p-rules.json", "不合語意規則（recipe 沒有 plan）"), ("p-latin1.json", "不是 utf-8"), ("p-dir.json", "是目錄 → 當成不存在")]:
        d = w.new("prof")
        w.sr("stage", d, "plan", "--profile", f"@TMP@/proj/{f}", stdin=w.plan(d, {"name": "feature"}), note=f"load_profile_for：--profile {note}")
        w.call("load_profile_for", argv=["--profile", f"@TMP@/proj/{f}"], note=f"load_profile_for：--profile {note}")
    d = w.new("prof")
    w.sr("stage", d, "plan", "--profile", "@TMP@/proj/p-nan.json", stdin=w.plan(d, {"name": "feature"}), label="nan-profile", note="load_profile_for：--profile 裡有 NaN（Python 讀得進來）")
    d = w.new("prof")
    w.sr("stage", d, "plan", "--profile", stdin=w.plan(d, {"name": "feature"}), note="opt：--profile 是最後一個參數（沒有值）→ 用專案的 profile")
    # 專案的 profile 本身有問題
    d0 = w.ready("design")
    for text, note in [("{壞的", "不是 JSON"), ("[]", "不是物件"), (json.dumps({"recipes": {"x": {"stages": ["build"]}}}), "不合規則")]:
        w.write(PROFILE, text)
        d = w.new("prof")
        w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "bugfix"}), note=f"load_profile_for：專案 profile {note} → plan invalid（recipe 用內建的算）")
        w.sr("approve", d0, "design", *HUMAN, note=f"recipe_tamper_errors：專案 profile {note} → 核准被擋")
        w.sr("escalate", d0, "--recipe", "feature", "--modifiers", "ui", *HUMAN, "--reason", "x", note=f"escalate：專案 profile {note} → 不能升級")
        w.call("load_profile_for", argv=[], note=f"load_profile_for：專案 profile {note}")
    w.profile(None)
    w.call("load_profile_for", argv=[], note="load_profile_for：專案沒有 profile → (None, 空字串)")
    w.sr("approve", d0, "design", *HUMAN, note="recipe_tamper_errors：沒有 profile 時用內建的重算，仍一致")
    w.profile(BASE)
    # runs_dir 底下被放了一份 profile（X3 B2）：不採用
    hijack = {"recipes": {**rcp.BUILTIN_RECIPES, "docs": {**rcp.BUILTIN_RECIPES["docs"], "build_kinds": ["code", "test", "doc"], "stages": ["plan", "merge"]}}, "modifiers": rcp.BUILTIN_MODIFIERS}
    w.write_json("proj/_runs/.claude/teams/methodology.json", hijack)
    d = w.new("hijack")
    w.write_json(f"{d}/.claude/teams/methodology.json", hijack)
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}), note="load_profile_for：run 目錄與它上層被放的 profile 不採用（docs 的段與 build_kinds 照專案的）")
    w.sr("status", d, note="status：docs 的段沒有被劫持")
    w.call("load_profile_for", argv=[], note="load_profile_for：只認專案的 profile")
    w.sr("stage", d, "plan", "--profile", "@TMP@/proj/_runs/.claude/teams/methodology.json", stdin=w.plan(d, {"name": "docs"}), note="--profile：明確指定時才用那一份")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：用被指定的 profile 存的 recipe")
    w.sr("stage", d, "merge", stdin=w.result(d, "merge"), note="stage merge：recipe（--profile 那份）只有 plan、merge")
    w.sr("close", d, *HUMAN, note="close：recipe_of_run 用 run.json 的 resolved")


def scenario_trace(w):
    """process_trace、_envelope_of、_ghostwrite、record_candidates 的分支。"""
    d = w.ready("design")
    ok = w.result(d, "design")
    spec, skeptic = NS + "dev-spec-author", NS + "tpm-skeptic"

    def stage(note, trace, approve=True, **extra):
        r = {**copy.deepcopy(ok), "trace": trace, **extra}
        w.sr("stage", d, "design", stdin=r, note=f"process_trace：{note}")
        if approve:
            w.sr("approve", d, "design", *POLICY, note=f"approve --by policy：{note}")
            w.call("process_trace", run=d, stage="design", result=r, write=False, note=f"process_trace（write=False）：{note}")

    base = ok["trace"]
    stage("紀錄齊全 → 產生索引", base)
    stage("trace 不是陣列", "x")
    stage("trace 是 null", None)
    stage("trace 是空陣列", [])
    stage("trace 的項目不是物件", [base[0], "x", None, 5, [1]])
    stage("ok=true 但沒有指定紀錄檔", [{"label": "outline", "agent_type": spec, "ok": True}])
    stage("ok=true、紀錄檔不存在、對不回信封 → 缺件", [{**base[0], "record": f"{d}/design/沒有.md"}])
    stage("ok=false → agent 沒有交件", [{**base[0], "ok": False}])
    stage("ok=false 且 capped → 派工數到上限", [{**base[0], "ok": False, "capped": True, "max_agents": 40}, {**base[1], "ok": False, "capped": True}])
    stage("ok=false、superseded、後面同 label 有交件 → 不算缺件", [{**base[0], "ok": False, "superseded": True, "tier": "light"}, base[0], base[1]])
    stage("ok=false、superseded、後面沒有重派 → 缺件", [{**base[0], "ok": False, "superseded": True}, base[1]])
    stage("ok=false、superseded、後面的重派是別的角色 → 缺件", [{**base[0], "ok": False, "superseded": True}, {**base[0], "agent_type": skeptic}])
    stage("ok=false、superseded 不是 true（是 1）→ 缺件", [{**base[0], "ok": False, "superseded": 1}, base[0]])
    stage("ok 不是 true／false（null、字串、1）→ 未完成", [{**base[0], "ok": None}, {**base[0], "ok": "true"}, {**base[0], "ok": 1}, {k: v for k, v in base[0].items() if k != "ok"}])
    stage("label、phase、agent_type 缺或不是字串", [{"record": base[0]["record"], "ok": True}, {"label": 5, "agent_type": ["x"], "phase": {"a": 1}, "record": base[0]["record"], "ok": True}])
    stage("label 有直線與換行（索引要跳脫）", [{**base[0], "label": "a|b\nc", "phase": ""}])
    stage("紀錄檔路徑是相對路徑（以目前目錄為基準）", [{**base[0], "record": f"{d}/design/outline.md".replace("@TMP@/", "")}])
    stage("紀錄檔只給檔名（找 <run>/<stage>/<檔名>）", [{**base[0], "record": "outline.md"}])
    w.write("proj/outside.md", "# 不是紀錄\n")
    stage("紀錄檔指到 run 目錄以外的檔 → 不算", [{**base[0], "record": "@TMP@/proj/outside.md"}])
    stage("紀錄檔用 .. 繞出 run 目錄 → 不算", [{**base[0], "record": f"{d}/design/../../../outside.md"}])
    stage("紀錄檔用反斜線", [{**base[0], "record": "design\\outline.md"}])
    stage("record 是數字", [{**base[0], "record": 5}])
    # 代寫（B9）
    art = copy.deepcopy(ok)
    art["challenge"]["artifacts"] = [f"{d}/design/ghost-a.md", "specs/e/design.md"]
    stage("有交件沒寫紀錄檔、信封的 artifacts 列了紀錄檔 → 代寫", [base[0], {**base[1], "record": f"{d}/design/ghost-a.md"}], challenge=art["challenge"])
    stage("同一份回傳重存 → 代寫檔內容相同、狀態標代寫", [base[0], {**base[1], "record": f"{d}/design/ghost-a.md"}], challenge=art["challenge"])
    stage("代寫檔還在、這次的回傳對不回信封 → 缺件（不沿用）", [base[0], {**base[1], "record": f"{d}/design/ghost-a.md"}])
    art["challenge"]["artifacts"] = ["design/ghost-b.md"]
    stage("artifacts 用 <stage>/<檔名> 結尾對到", [base[0], {**base[1], "record": f"{d}/design/ghost-b.md"}], challenge=art["challenge"])
    art["challenge"]["artifacts"] = ["x/design/ghost-c.md", 5, None]
    stage("artifacts 用 /<stage>/<檔名> 結尾對到；不是字串的略過", [base[0], {**base[1], "record": f"{d}/design/ghost-c.md"}], challenge=art["challenge"])
    dg = copy.deepcopy(ok)
    dg["challenge"]["summary"] = "skeptic 判定：設計成立"
    dg["challenge"]["open_questions"] = [{"question": "要不要補英文版", "blocking": False}, "x"]
    dg["challenge"]["evidence"] = [{"claim": "讀過 design.md", "source": "specs/e/design.md:1", "result": "成立"}, "x"]
    digest = {"status": "done", "summary": "skeptic 判定：設計成立"}
    stage("digest 剛好對到一個信封 → 代寫（evidence、open_questions 帶進去）", [base[0], {**base[1], "record": f"{d}/design/ghost-d.md", "digest": digest}], challenge=dg["challenge"])
    stage("digest 對到多個不同的信封（summary 都是 s）→ 不猜、缺件", [base[0], {**base[1], "record": f"{d}/design/ghost-e.md", "digest": {"status": "done", "summary": "s"}}])
    stage("digest 的 status 對不上 → 缺件", [base[0], {**base[1], "record": f"{d}/design/ghost-f.md", "digest": {"status": "partial", "summary": "skeptic 判定：設計成立"}}], challenge=dg["challenge"])
    stage("digest 的 summary 是空的、digest 不是物件", [{**base[0], "record": f"{d}/design/ghost-g.md", "digest": {"summary": ""}}, {**base[1], "record": f"{d}/design/ghost-h.md", "digest": "x"}])
    same = copy.deepcopy(ok)
    same["challenge"] = copy.deepcopy(ok["design"])
    stage("digest 對到兩個內容完全相同的信封 → 算同一個", [{**base[0], "record": f"{d}/design/ghost-i.md", "digest": {"status": "done", "summary": "s"}}], challenge=same["challenge"])
    long = copy.deepcopy(ok)
    long["challenge"]["summary"] = "很長的摘要" * 100
    stage("digest 只比 summary 的前 400 字", [base[0], {**base[1], "record": f"{d}/design/ghost-j.md", "digest": {"status": "done", "summary": ("很長的摘要" * 100)[:400]}}], challenge=long["challenge"])
    w.write(f"{d}/design/hand.md", f"# 執行紀錄：challenge（{save_run.GHOSTWRITTEN}）\n其實是手寫的\n")
    stage("紀錄檔第一行有代寫標記、對得回信封 → 狀態標代寫", [base[0], {**base[1], "record": f"{d}/design/hand.md", "digest": digest}], challenge=dg["challenge"])
    w.write(f"{d}/design/bin.md", raw=b"\xff\xfe\x00\n# x\n")
    stage("紀錄檔不是 utf-8（_head 用 replace 讀）", [{**base[0], "record": f"{d}/design/bin.md"}])
    w.mkdir(f"{d}/design/dir.md")
    stage("紀錄檔的位置是目錄 → 不算檔案", [{**base[0], "record": f"{d}/design/dir.md"}])
    # steward
    dm = w.ready("merge", {"name": "docs"})
    okm = w.result(dm, "merge")
    md = okm["analysis"]
    st = NS + "dev-merge-steward"

    def merge(note, trace, **extra):
        r = {"stage": "merge", "recipe": okm["recipe"], "trace": trace, **extra}
        w.sr("stage", dm, "merge", stdin=r, note=f"process_trace（steward）：{note}")
        w.call("process_trace", run=dm, stage="merge", result=r, write=False, note=f"process_trace（write=False，steward）：{note}")
        w.call("policy_violations", stage="merge", result=r, run=dm, note=f"policy_violations：merge 段，{note}")

    merge("兩筆 steward、兩份信封 → 依序代寫", okm["trace"], analysis=md, execute=md)
    merge("steward 的 trace 比信封多 → 多的那筆無法代寫", okm["trace"] + [{"label": "extra", "agent_type": st, "ok": True}], analysis=md, execute=md)
    merge("steward 沒有信封 → 無法代寫", okm["trace"][:1], other=env({"a": 1}))
    merge("steward 的 trace 沒有 record → 用 <label>.md", [{"label": "analyze2", "agent_type": st, "ok": True}], analysis=md)
    merge("steward ok=false → 沒有交件", [{**okm["trace"][0], "ok": False}], analysis=md)
    full = env({**MERGE, "traceability": [{"req": "R1"}], "followups": [{"t": 1}, {"t": 2}], "gate_reasons": ["風險低", 5]}, artifacts=["a.md"],
               open_questions=[{"question": "要不要等下一版"}])
    merge("代寫的內容：traceability、followups、gate_reasons、成果檔、待決問題", [{"label": "full", "agent_type": st, "ok": True, "record": f"{dm}/merge/full.md"}], analysis=full)
    w.rm(f"{dm}/merge/full.md")
    r = {"stage": "merge", "recipe": okm["recipe"], "trace": [{"label": "full", "agent_type": st, "ok": True, "record": f"{dm}/merge/full.md"}], "analysis": full}
    w.call("process_trace", run=dm, stage="merge", result=r, write=False, note="process_trace（write=False）：steward 的紀錄檔還沒代寫 → 缺件")
    w.call("process_trace", run=dm, stage="merge", result=r, write=True, note="process_trace（write=True）：代寫並寫索引")
    w.call("record_candidates", run=dm, stage="merge", record="", note="record_candidates：空字串 → 空清單")
    w.call("record_candidates", run=dm, stage="merge", record=f"{dm}/merge/full.md", note="record_candidates：run 目錄底下的絕對路徑")
    w.call("record_candidates", run=dm, stage="merge", record="@TMP@/proj/outside.md", note="record_candidates：run 目錄以外")
    w.call("record_candidates", run=dm, stage="merge", record="merge/full.md", note="record_candidates：相對路徑（以目前目錄為基準，不在 run 底下）")
    w.call("record_candidates", run="proj/_runs/rel", stage="plan", record="proj/_runs/rel/plan/x.md", note="record_candidates：run 與紀錄檔都是相對路徑")
    # 有 recipe 的 run 回傳沒有 trace
    d = w.ready("design")
    r = w.result(d, "design")
    del r["trace"]
    w.sr("stage", d, "design", stdin=r, note="stage：有 recipe 的 run 回傳沒有 trace → missing_records")
    w.sr("approve", d, "design", *POLICY, note="policy_violations：有 recipe 卻沒有 trace → 政策核准被擋")
    w.sr("approve", d, "design", *HUMAN, note="approve：人工核准照記，警告執行紀錄缺件")
    w.sr("status", d, note="status：警告執行紀錄缺件")


def scenario_recipe_checks(w):
    """recipe_mismatch、required_roles_missing、recipe_tamper_errors。"""
    d = w.ready("test", {"name": "feature", "modifiers": ["security", "ui"]})
    ok = w.result(d, "test")
    for rec, note in [(None, "回傳沒有 recipe"), ("feature", "recipe 不是物件"), ({"name": "docs", "modifiers": [], "stages": ["plan", "build", "deliver", "merge"]}, "name、modifiers、stages 都不一致"),
                      ({**ok["recipe"], "modifiers": ["security"]}, "modifiers 少一個"), ({**ok["recipe"], "modifiers": "ui,security"}, "modifiers 是逗號字串（順序不同也算一致）"),
                      ({**ok["recipe"], "stages": ok["recipe"]["stages"][:-1]}, "stages 少一段"), ({**ok["recipe"], "stages": None}, "stages 是 null"),
                      ({**ok["recipe"], "stages": "plan"}, "stages 是字串"), ({**ok["recipe"], "modifiers": None}, "modifiers 是 null")]:
        r = copy.deepcopy(ok)
        if rec is None:
            del r["recipe"]
        else:
            r["recipe"] = rec
        w.sr("stage", d, "test", stdin=r, note=f"recipe_mismatch：{note} → invalid")
        w.call("policy_violations", stage="test", result=r, run=d, note=f"policy_violations：recipe_mismatch {note}")
    for drop, note in [("security_review", "security_review=always 卻沒派 dev-security-reviewer"), ("design_review", "design_review=always 卻沒派 dev-design-reviewer"), ("verify#1", "沒有 dev-verifier")]:
        r = copy.deepcopy(ok)
        r["trace"] = [t for t in r["trace"] if t["label"] != drop]
        w.sr("stage", d, "test", stdin=r, note=f"stage test：{note}（存檔不擋）")
        w.sr("approve", d, "test", *POLICY, note=f"required_roles_missing：{note} → 政策核准被擋")
    r = copy.deepcopy(ok)
    r["trace"][1]["ok"] = False
    w.sr("stage", d, "test", stdin=r, note="stage test：reviewer 的 trace ok=false")
    w.sr("approve", d, "test", *POLICY, note="required_roles_missing：有派但沒交件也算沒有")
    w.sr("stage", d, "test", stdin=ok, note="stage test：角色齊全")
    w.sr("approve", d, "test", *POLICY, note="approve --by policy：角色齊全 → 核准")
    okd = w.result(d, "deliver")
    for drop in ["review:bugs", "review:history"]:
        r = copy.deepcopy(okd)
        r["trace"] = [t for t in r["trace"] if t["label"] != drop]
        w.sr("stage", d, "deliver", stdin=r, note=f"stage deliver：少了 {drop}")
        w.sr("approve", d, "deliver", *POLICY, note=f"required_roles_missing：deliver 軸沒有交件的審查（{drop}）")
    w.sr("stage", d, "deliver", stdin=okd, note="stage deliver：每一軸都有")
    w.sr("approve", d, "deliver", *POLICY, note="approve --by policy：deliver")
    # 手改 run.json 的 recipe.resolved（X3 M1）
    for change, note in [(lambda r: r["resolved"].update(stages=[s for s in r["resolved"]["stages"] if s != "test"]), "少 test 段"),
                         (lambda r: r["resolved"].update(deliver_axes=["bugs"]), "deliver 軸變少"),
                         (lambda r: r["resolved"]["test"].update(security_review="never"), "security_review 改成 never"),
                         (lambda r: r["resolved"].update(build_kinds=r["resolved"]["build_kinds"] + ["agent_config"]), "build_kinds 多了種類"),
                         (lambda r: r.update(name="docs"), "name 改成 docs（resolved 比 docs 嚴，不算鬆）"),
                         (lambda r: r.update(name="沒有這個"), "name 改成不認得的（解析成 feature）"),
                         (lambda r: r.update(modifiers=["zzz"]), "modifiers 改成不認得的"),
                         (lambda r: r.update(resolved={}), "resolved 清空"),
                         (lambda r: r.update(resolved={"stages": "plan"}), "resolved.stages 是字串")]:
        d = w.ready("deliver", {"name": "bugfix"})
        data = w.read_json(d)
        change(data["recipe"])
        w.write_json(f"{d}/run.json", data)
        w.call("recipe_tamper_errors", data=data, note=f"recipe_tamper_errors：{note}")
        w.sr("approve", d, "test", *HUMAN, note=f"approve：recipe 被手改（{note}）")
        w.sr("status", d, note=f"status：recipe 被手改（{note}）")
    for name, change, note in [("docs", lambda r: r["resolved"].update(publishable=True), "合法（不變）"), ("spike", lambda r: r["resolved"].update(publishable=True), "spike 的 publishable 被改成 true"),
                               ("hotfix", lambda r: r["resolved"].update(followup=False), "hotfix 的 followup 被拿掉")]:
        d = w.ready("build", {"name": name})
        data = w.read_json(d)
        change(data["recipe"])
        w.write_json(f"{d}/run.json", data)
        w.call("recipe_tamper_errors", data=data, note=f"recipe_tamper_errors：{note}")
        w.sr("approve", d, "plan", *HUMAN, note="approve plan：plan 段不查 recipe_tamper_errors")
    # profile 在 plan 之後被改鬆／改嚴
    d = w.ready("design", {"name": "feature"})
    w.profile({**BASE, "recipes": {**rcp.BUILTIN_RECIPES, "feature": {**rcp.BUILTIN_RECIPES["feature"], "deliver_axes": ["standards", "spec", "bugs", "history", "security"]}}})
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage design：profile 在 plan 之後變嚴")
    w.sr("approve", d, "design", *HUMAN, note="recipe_tamper_errors：profile 變嚴後，run.json 的 resolved 比重算結果鬆 → 擋")
    w.profile(BASE)
    w.sr("approve", d, "design", *HUMAN, note="recipe_tamper_errors：profile 改回來 → 可以")


def scenario_models(w):
    """model_errors：profile 有 models 時 trace 的等級與模型。"""
    models = {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"}, "defaults": {"*": "standard"}, "floors": {}}
    w.profile({**BASE, "models": models})
    d = w.new("model")

    def stage(note, *entries, mods=()):
        r = w.plan(d, {"name": "feature", "modifiers": list(mods)})
        r["trace"] = [{**w.entry(d, "plan", lab, role), "tier": tier, "model": model} for lab, role, tier, model in entries]
        w.sr("stage", d, "plan", stdin=r, note=f"model_errors：{note}")

    stage("等級不低於下限、model 對得上", ("intent", "dev-spec-author", "strong", "opus"), ("research:a", "tpm-researcher", "standard", "sonnet"), ("locate:a", "tpm-researcher", "light", "haiku"))
    stage("dev-spec-author 在 plan 用 light（低於下限 strong）", ("intent", "dev-spec-author", "light", "haiku"))
    stage("說是 strong、model 卻是 haiku", ("intent", "dev-spec-author", "strong", "haiku"))
    stage("不認得的等級", ("intent", "dev-spec-author", "ultra", "opus"))
    stage("tier 是空字串、null → 沒有 model／tier", ("intent", "dev-spec-author", "", "opus"), ("x", "tpm-skeptic", None, None))
    stage("擴充 agent（不是 tpm-／dev-）不檢查", ("intent", "dev-spec-author", "strong", "opus"), ("ext", "my-agent", "", ""))
    stage("modifier 的 model_floors（security：tpm-skeptic 要 strong）", ("intent", "dev-spec-author", "strong", "opus"), ("challenge", "tpm-skeptic", "standard", "sonnet"), mods=("security",))
    stage("tier 是數字、陣列", ("intent", "dev-spec-author", 5, "opus"), ("x", "tpm-skeptic", ["strong"], "opus"))
    w.sr("approve", d, "plan", *HUMAN, note="approve plan：模型分級的錯讓 plan 是 invalid")
    d2 = w.ready("design")
    r = w.result(d2, "design")
    w.sr("stage", d2, "design", stdin=r, note="model_errors：design 段的 trace 沒有 tier（profile 有 models）→ invalid")
    for t in r["trace"]:
        t.update(tier="standard", model="sonnet")
    w.sr("stage", d2, "design", stdin=r, note="model_errors：design 段都有 tier、model → 通過")
    w.profile({**BASE, "models": {**models, "mode": "inherit"}})
    stage("mode=inherit 不檢查", ("intent", "dev-spec-author", "", "inherit"))
    w.profile({**BASE, "models": {**models, "tiers": {"light": "haiku"}}})
    stage("tiers 沒有 strong → model 應該是 inherit", ("intent", "dev-spec-author", "strong", "inherit"), ("x", "tpm-skeptic", "strong", "opus"))
    w.profile({**BASE, "models": "auto"})
    stage("models 不是物件 → 不檢查", ("intent", "dev-spec-author", "light", "haiku"))
    w.profile({**BASE, "models": {**models, "floors": {"tpm-researcher": "strong"}}})
    stage("profile 的 floors", ("intent", "dev-spec-author", "strong", "opus"), ("research:a", "tpm-researcher", "standard", "sonnet"))
    w.profile(BASE)


def scenario_reuse(w):
    """調研重用：verify_reuse 不符判 invalid；record_reuse 記來源。"""
    d = w.new("reuse")
    r_env = env({"lens": "code-contracts", "findings": []}, summary="上次的結論")
    base = w.plan(d, {"name": "feature", "modifiers": [], "plan_mode": "feature"})
    research = w.entry(d, "plan", "research:code-contracts", "tpm-researcher")

    def stage(note, env_=r_env, **extra):
        r = {**copy.deepcopy(base), "research": [env_], "research_by_angle": {"code-contracts": env_}, **extra}
        w.sr("stage", d, "plan", stdin=r, note=f"調研重用：{note}")

    stage("plan 存檔後記下來源檔的 sha1（record_reuse）", locations={"code-contracts": [{"path": "src/export.py", "start": 1, "end": 2, "why": "x"}]}, reused=[],
          trace=base["trace"] + [research])
    stage("沿用的角度來源沒變、結論一致 → 通過", locations={}, reused=["code-contracts"])
    stage("沿用的結論和紀錄不一樣 → invalid", env_={**r_env, "summary": "偷改"}, locations={}, reused=["code-contracts"])
    stage("沿用沒有紀錄的角度 → invalid", locations={}, reused=["specs-adr"])
    stage("reused 格式不對 → invalid", locations={}, reused="code-contracts")
    stage("reused 的項目不是字串 → invalid", locations={}, reused=[1])
    w.write("proj/repo/src/export.py", "def export():\n    return 'xlsx'\n")
    stage("來源變了還沿用 → invalid", locations={}, reused=["code-contracts"])
    stage("來源讀不到 → 只警告（record_reuse）", locations={"code-contracts": [{"path": "src/沒有.py", "start": 1, "end": 1, "why": "x"}]}, reused=[])
    w.write(f"{d}/plan/reuse.json", "{壞的")
    stage("reuse.json 壞掉、又說沿用 → invalid", locations={}, reused=["code-contracts"])
    w.rm(f"{d}/plan/reuse.json")
    w.mkdir(f"{d}/plan/reuse.json")
    stage("reuse.json 寫不了（是目錄）→ 只警告、不擋", locations={"code-contracts": [{"path": "src/export.py", "start": 1, "end": 2, "why": "x"}]}, reused=[])


def scenario_constraints(w):
    """constraint_issues、constraint_rows、constraint_file。"""
    arch = {"id": "arch", "file": "ARCHITECTURE.md", "stages": ["design"], "trace": "table"}
    w.write("proj/repo/docs/API.md", "# API 慣例\n")

    def design(note, table, cons, approve=True, by=POLICY):
        w.profile({**BASE, "constraints": cons})
        d = w.ready("design")
        r = w.result(d, "design")
        w.write(r["trace"][0]["record"], "# 紀錄\n" + (f"\n## 限制對照\n\n| 限制 | 判定 | 依據 |\n|---|---|---|\n{table}\n" if table else ""))
        w.sr("stage", d, "design", stdin=r, note=f"constraint_issues：{note}")
        if approve:
            w.sr("approve", d, "design", *by, note=f"approve：{note}")
        return d, r

    design("文件在、對照列是符合 → 可以政策核准", "| arch | 符合 | design.md:12 |", [arch])
    d, _ = design("沒有對照列 → 記 constraint_issues、政策核准被擋", None, [arch])
    w.sr("approve", d, "design", *HUMAN, note="approve：人工核准照記，警告限制文件")
    w.sr("status", d, note="status：警告限制文件")
    design("對照列是偏離 → 要由人決定", "| arch | 偏離 | 理由見 design.md:40 |", [arch])
    design("對照列是不適用 → 可以", "| `arch` | 不適用 | 這次沒碰分層 |", [arch])
    design("對照列的 id 用方括號、前面有空白", "  | [arch] | 符合 | x |", [arch])
    design("對照列的判定不在三種裡 → 當成沒有對照列", "| arch | 大致符合 | x |", [arch])
    design("同一個限制多列：有一列偏離就擋", "| arch | 符合 | a |\n| arch | 偏離 | b |\n| other | 符合 | c |", [arch])
    design("文件不存在 → 擋", "| arch | 符合 | x |", [{**arch, "file": "docs/NOPE.md"}])
    design("文件在目標 repo（專案根目錄沒有）→ 找得到", "| api | 符合 | x |", [{"id": "api", "file": "docs/API.md", "stages": ["design"], "trace": "table"}])
    design("file 是空字串 → 當成不存在", None, [{"id": "nofile", "file": "", "stages": ["design"]}])
    d, _ = design("檢查指令 exit 3 → 記結果、擋", None, [{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "echo 有 3 個違規; echo 細節 >&2; exit 3"}])
    w.sr("approve", d, "design", *HUMAN, note="approve：人工核准照記，警告檢查指令沒過")
    design("檢查指令通過、輸出超過 300 字只留尾巴", None, [{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "printf 'x%.0s' $(seq 1 400); echo 尾巴"}])
    design("檢查指令在目標 repo 執行", None, [{"id": "cwd", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "test -f src/export.py"}])
    design("check 是空白、不是字串 → 不跑", None, [{"id": "a", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "  "}, {"id": "b", "file": "ARCHITECTURE.md", "stages": ["design"], "check": 5}])
    design("別段的限制、stages 缺、項目不是物件 → 不算", None, [{"id": "rev", "file": "docs/NOPE.md", "stages": ["deliver"], "trace": "table"}, {"id": "nostage", "file": "x"}, "x", None])
    design("profile 的 constraints 不是陣列裡的物件（空陣列）→ 行為不變", None, [])
    # 代寫的紀錄不算有對照
    w.profile({**BASE, "constraints": [arch]})
    d = w.ready("design")
    r = w.result(d, "design")
    r["challenge"]["summary"] = "| arch | 符合 | 我在 summary 裡自己說的 |"
    r["trace"][1] = {**r["trace"][1], "record": f"{d}/design/ghost.md", "digest": {"status": "done", "summary": r["challenge"]["summary"]}}
    w.sr("stage", d, "design", stdin=r, note="constraint_rows：代寫的紀錄（只來自信封）不算 agent 做過對照")
    w.call("constraint_rows", run=d, stage="design", note="constraint_rows：代寫的紀錄略過")
    w.write(f"{d}/design/extra.md", "# 另一份紀錄\n| arch | 符合 | a |\n|arch|偏離|b|\n| ARCH | 符合 | 大寫不算 |\n| arch | 符合\n")
    w.write(f"{d}/design/{save_run.INDEX_NAME}", "| arch | 符合 | 索引檔不算 |\n")
    w.write(f"{d}/design/note.txt", "| arch | 符合 | 不是 .md 不算 |\n")
    w.call("constraint_rows", run=d, stage="design", note="constraint_rows：多份紀錄、索引檔與非 .md 不算")
    w.call("constraint_rows", run=d, stage="沒有這段", note="constraint_rows：段的目錄不存在")
    w.call("constraint_issues", run=d, stage="design", profile={"constraints": [arch]}, repo="@TMP@/proj/repo", note="constraint_issues：有偏離的那一份")
    w.call("constraint_issues", run=d, stage="build", profile={"constraints": [{"id": "fe", "file": "ARCHITECTURE.md", "stages": ["build"], "paths": ["web/**"], "trace": "table"}]}, repo=None,
           note="constraint_issues：build 段有 paths 的限制不在這裡要求對照")
    w.call("constraint_issues", run=d, stage="build", profile={"constraints": [{"id": "fe", "file": "ARCHITECTURE.md", "stages": ["build"], "paths": [], "trace": "table"}]}, repo=None,
           note="constraint_issues：build 段、paths 是空的 → 要對照")
    w.call("constraint_issues", run=d, stage="design", profile={"constraints": [{"id": "c", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "exit 1"}]}, repo=None, run_checks=False,
           note="constraint_issues：run_checks=False 不跑檢查指令")
    w.call("constraint_issues", run=d, stage="design", profile={"constraints": [{"id": "c", "file": "ARCHITECTURE.md", "stages": ["design"], "check": "pwd"}]}, repo="@TMP@/proj/沒有這個目錄",
           note="constraint_issues：repo 不是目錄 → 在專案根目錄執行")
    for prof, note in [(None, "profile 是 None"), ({}, "沒有 constraints"), ({"constraints": None}, "constraints 是 null"), ({"constraints": "x"}, "constraints 是字串"),
                       ({"constraints": [{"id": 5, "file": 5, "stages": "design", "trace": "table"}]}, "id、file 是數字；stages 是字串（子字串比對）"),
                       ({"constraints": [{"stages": ["design"]}]}, "只有 stages"), ({"constraints": {"a": 1}}, "constraints 是物件"), ([], "profile 是陣列"), ("x", "profile 是字串")]:
        w.call("constraint_issues", run=d, stage="design", profile=prof, repo=None, note=f"constraint_issues：{note}")
    for c, repo, note in [({"file": "ARCHITECTURE.md"}, None, "專案根目錄有"), ({"file": "docs/API.md"}, "@TMP@/proj/repo", "目標 repo 有"), ({"file": "docs/API.md"}, None, "沒有 repo"),
                          ({"file": ""}, "@TMP@/proj/repo", "file 空"), ({}, None, "沒有 file"), ({"file": "@TMP@/proj/README.md"}, None, "絕對路徑"), ({"file": "repo"}, None, "是目錄")]:
        w.call("constraint_file", c=c, repo=repo, note=f"constraint_file：{note}")
    # 政策核准時重算（不採信 run.json 記的）
    d, r = design("存檔時符合", "| arch | 符合 | x |", [arch], approve=False)
    w.write(r["trace"][0]["record"], "# 紀錄\n| arch | 偏離 | 存檔之後改的 |\n")
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：重算 constraint_issues（存檔後紀錄被改成偏離）")
    w.profile(BASE)
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：profile 拿掉 constraints 之後 → 可以")


def scenario_plan_check(w):
    """plan_check_issues、_plan_check_for。"""
    w.write("proj/repo/.worktrees/e/specs/e/spec.md", "# spec\n- AC-1 可以下載\n- AC-2 可以選欄位\n- AC-MUT-3 不可變\n")
    T = lambda i, deps=(), acc=("AC-1 通過",), kind="code", owns=("src/a.py",): {"id": i, "title": i, "kind": kind, "owns": list(owns), "depends_on": list(deps), "acceptance": list(acc)}  # noqa: E731
    for subs, note in [([T("t1", acc=("AC-1", "AC-2", "AC-MUT-3"))], "乾淨"), ([T("t1", ["t9"]), T("t1"), T("t2", ["t3"]), T("t3", ["t2"])], "id 重複、相依不存在、循環"),
                       ([T("t1", acc=()), T("t2", owns=()), T("t3", kind="research", owns=())], "沒有驗收條件、沒有 owns"),
                       ([T("t1", acc=("AC-1 通過", "AC-9 通過"))], "spec 有沒人對應的、子任務引用 spec 沒有的"), ([T("t1", acc=("pytest tests/test_a.py",))], "子任務沒用 AC 編號 → 不比對")]:
        d = w.ready("design")
        r = w.result(d, "design")
        r["design"]["payload"]["subtasks"] = subs
        w.sr("stage", d, "design", stdin=r, note=f"plan_check_issues：{note}")
        w.sr("approve", d, "design", *POLICY, note=f"approve --by policy：拆工檢查（{note}）")
        w.sr("approve", d, "design", *HUMAN, note=f"approve：人工核准照記（{note}）")
        w.call("plan_check_for", run=d, stage="design", result=r, data=w.read_json(d), note=f"_plan_check_for：{note}")
        w.call("plan_check_for", run=d, stage="build", result=r, data=w.read_json(d), note="_plan_check_for：不是 plan／design → 不檢查")
    d = w.ready("design")
    r = w.result(d, "design")
    for data, note in [({"target_repo": ""}, "沒有 target_repo → 找不到 spec 目錄"), ({"target_repo": "@TMP@/proj/repo", "epic": "沒有"}, "epic 的 spec 目錄不存在"),
                       ({"target_repo": "@TMP@/proj/repo", "epic": "e"}, "run.json 沒有 epic 時 run_epic 從 plan.json 取"), ({"target_repo": "@TMP@/proj/repo", "args": {"epic": "e"}}, "args.epic"),
                       ({"target_repo": 5}, "target_repo 是數字"), ({"target_repo": "@TMP@/proj/repo", "args": "x"}, "args 不是物件")]:
        w.call("plan_check_for", run=d, stage="design", result=r, data=data, note=f"_plan_check_for：{note}")
    w.write("proj/repo/specs/e2/spec.md", "- AC-7 x\n")
    w.call("plan_check_for", run="@TMP@/proj/_runs/沒有", stage="plan", result={"subtasks": [T("t1", acc=("AC-8",))]}, data={"target_repo": "@TMP@/proj/repo", "args": {"epic": "e2"}},
           note="_plan_check_for：repo/specs/<epic>（沒有 worktree）")
    w.call("plan_check_issues", result={"subtasks": [T("t1", acc=("AC-8",))]}, spec="@TMP@/proj/repo/specs/e2/spec.md", note="plan_check_issues：有 spec 檔")
    w.call("plan_check_issues", result={"subtasks": [T("t1", acc=("AC-8",))]}, spec="@TMP@/proj/repo/specs/沒有.md", note="plan_check_issues：spec 檔不存在 → 不比對")
    w.call("plan_check_issues", result={"subtasks": [T("t1", acc=("AC-8",))]}, spec="@TMP@/proj/repo/specs", note="plan_check_issues：spec 是目錄 → 不比對")
    w.write("proj/repo/specs/e2/many.md", "".join(f"- AC-{i}\n" for i in range(1, 16)))
    w.call("plan_check_issues", result={"subtasks": [{**T("t1", acc=["AC-99"] + [f"AC-X{i}" for i in range(14)]), "story_ids": ["AC-1", 5]}]}, spec="@TMP@/proj/repo/specs/e2/many.md",
           note="plan_check_issues：超過 12 條只列前 12 條加 …；story_ids 也算")
    w.write("proj/repo/specs/e2/bin.md", raw=b"- AC-1 \xff\n- AC-2\n")
    w.call("plan_check_issues", result={"subtasks": [T("t1")]}, spec="@TMP@/proj/repo/specs/e2/bin.md", note="plan_check_issues：spec 不是 utf-8（用 replace 讀）")


def scenario_approve(w):
    """approve 的每個拒絕理由與警告。"""
    d = w.ready("design")
    w.sr("approve", d, "design", *HUMAN, note="approve：design 還是 pending → 只有 done 的段能核准")
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage design")
    w.sr("approve", d, "design", note="approve：缺 --by 與 --at")
    w.sr("approve", d, "design", "--by", "owner", note="approve：缺 --at")
    w.sr("approve", d, "design", "--at", AT, note="approve：缺 --by")
    w.sr("approve", d, "design", "--by", "policy", "--at", AT, note="approve：--by policy 沒有 --note")
    w.sr("approve", d, "design", "--by", "owner", "--at", note="approve：--at 是最後一個參數（沒有值）")
    w.sr("approve", d, "test", *HUMAN, note="approve：test 還 pending、前面的 design 沒核准（兩個理由）")
    w.sr("approve", d, "design", *HUMAN, "--note", "看過了", note="approve：人核准帶 --note")
    w.sr("approve", d, "design", *POLICY, note="approve：已經核准過再用政策核准一次（覆寫核准紀錄）")
    # 閘門鏈
    d = w.new("chain")
    w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "feature"}), note="stage plan")
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage design：plan 還沒核准就存 design（存檔不擋）")
    w.sr("approve", d, "design", *HUMAN, note="chain_errors：前面的 plan 段還沒核准")
    w.sr("approve", d, "plan", *HUMAN, note="approve plan")
    w.sr("approve", d, "design", *HUMAN, note="approve design：plan 核准後可以")
    for st in ["build", "sync"]:
        w.sr("stage", d, st, stdin={**w.result(d, st), "halted": True}, note=f"stage {st}：halted → blocked")
    w.sr("stage", d, "test", stdin=w.result(d, "test"), note="stage test")
    w.sr("approve", d, "test", *HUMAN, note="chain_errors：前面的 build、sync 狀態是 blocked")
    w.sr("approve", d, "test", *POLICY, note="chain_errors：政策核准一樣擋")
    data = w.read_json(d)
    data["stages"]["build"] = {"status": "skipped", "gate": False}
    data["stages"].pop("sync")
    data["stages"]["design"] = "done"
    w.write_json(f"{d}/run.json", data)
    w.sr("approve", d, "test", *HUMAN, note="chain_errors：recipe 有的段被標 skipped（不一致）、缺段、段不是物件")
    w.sr("status", d, note="status：段的值是字串（Python 沒接住的例外）")
    # 狀態 done 但 envelopes 是 0（run.json 被手改）
    d = w.ready("design")
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage design")
    data = w.read_json(d)
    data["stages"]["design"]["envelopes"] = 0
    w.write_json(f"{d}/run.json", data)
    w.sr("approve", d, "design", *HUMAN, note="approve：狀態 done 但 envelopes 是 0")
    del data["stages"]["design"]["status"]
    w.write_json(f"{d}/run.json", data)
    w.sr("approve", d, "design", *HUMAN, note="approve：段沒有 status 鍵 → 沒接住的 KeyError")
    w.sr("status", d, note="status：段沒有 status 鍵 → 沒接住的 KeyError")
    w.sr("status", d, "--json", note="status --json：段沒有 status 鍵 → 沒接住的 KeyError")
    # 政策核准要讀該段的實際結果
    d = w.ready("design")
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage design")
    w.rm(f"{d}/design.json")
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：找不到 design.json")
    w.write(f"{d}/design.json", "{壞的")
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：design.json 不是 JSON → 沒接住的例外")
    w.write(f"{d}/design.json", '{"x": NaN}')
    w.sr("approve", d, "design", *POLICY, label="nan-stage-file", note="approve --by policy：design.json 有 NaN（Python 讀得進來，判零信封）")
    w.sr("approve", d, "design", *HUMAN, note="approve：人工核准不讀 design.json")
    d = w.ready("plan-approve")
    w.rm(f"{d}/plan.json")
    w.sr("approve", d, "plan", *POLICY, note="approve --by policy：plan 段、又找不到 plan.json → 兩個理由（HUMAN_ONLY 另外補）")
    # run.json 記的缺件，人工核准時的警告
    d = w.ready("design")
    r = w.result(d, "design")
    r["trace"] = [{**t, "ok": False} for t in r["trace"]] * 3
    r["design"]["payload"]["subtasks"] = [{"id": "t1", "depends_on": ["t9"], "acceptance": []}]
    w.sr("stage", d, "design", stdin=r, note="stage design：6 筆缺件、拆工檢查有問題")
    w.sr("approve", d, "design", *HUMAN, note="approve：人工核准的警告只列前 5 個缺件；拆工檢查另外警告")
    w.sr("status", d, note="status：警告只列前 3 個")


def scenario_policy(w):
    """policy_violations 的每個理由（經由 approve --by policy 與直接呼叫）。"""
    d = w.ready("design")
    ok = w.result(d, "design")

    def case(note, stage="design", run=d, **change):
        r = copy.deepcopy(ok if stage == "design" else okt)
        for k, v in change.items():
            if v is DROP:
                r.pop(k, None)
            else:
                r[k] = v
        w.sr("stage", run, stage, stdin=r, note=f"stage：{note}")
        w.sr("approve", run, stage, *POLICY, note=f"policy_violations：{note}")
        w.call("policy_violations", stage=stage, result=r, run=run, note=f"policy_violations（直接呼叫）：{note}")
        w.call("policy_violations", stage=stage, result=r, note=f"policy_violations（沒給 run）：{note}")

    case("custom_script（自訂腳本）→ 一律由人核准", custom_script={"source": "my.js", "sha1": "abc"})
    case("override_script（覆寫檔或舊副本）→ 一律由人核准", override_script=".claude/workflows/sdlc-design.js")
    case("manual_output（done --output 手動給的結果）→ 一律由人核准", manual_output=True)
    case("三種非標準結果同時有", custom_script="x", override_script="y", manual_output=1)
    case("非標準標記是假值（空字串、0、false）→ 不算", custom_script="", override_script=0, manual_output=False)
    case("信封外有 null（analysis: [null]）→ agent 缺件", analysis=[None])
    case("信封外有 null（challenge: null）→ agent 缺件", challenge=None)
    case("NULL_OK_KEYS 的 null 不算缺件", gate=None, reason=None, problem=None, next=None, summary=None, rest=None, head_sha=None, halt_reason=None)
    case("巢狀結構裡的 null 也算", groups={"a": [{"b": None}], "gate": None})
    q = copy.deepcopy(ok["design"])
    q["open_questions"] = [{"question": "資料模型要不要拆表" * 20, "blocking": True}, {"question": "小事", "blocking": False}]
    case("信封有 blocking 的待決問題（問題只列前 80 字）", design=q)
    f = env({"lens": "plan", "findings": [{"id": "F1", "severity": "blocker", "claim": "少了回滾" * 30, "evidence": []}, {"id": "F2", "severity": "important", "claim": "c", "evidence": []},
                                         {"id": "F3", "severity": "nit", "claim": "c", "evidence": []}]})
    case("findings 有 blocker／important", review=f)
    case("skeptic 推翻（refuted=true）", challenge=env({"verdicts": [{"target_id": "x", "refuted": True, "reason": "缺資料模型"}]}))
    case("verdict 的 refuted 缺、是 null、是 0 → 都當推翻", challenge=env({"verdicts": [{"target_id": "a"}, {"target_id": "b", "refuted": None}, {"refuted": 0, "reason": "r" * 100}]}))
    case("verdicts 有格式不對的條目", challenge=env({"verdicts": ["x", None, 5]}))
    case("halted", halted=True, halt_reason="子計劃重疊")
    w.sr("stage", d, "design", stdin=ok, note="stage design：合格")
    w.sr("approve", d, "design", *POLICY, note="policy_violations：全部符合 → 可以政策核准")
    w.advance(d, "test")
    okt = w.result(d, "test")
    crit = lambda *c, **kw: env({"commit": "a" * 40, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": list(c), **kw})  # noqa: E731
    C = lambda i, res: {"id": i, "criterion": "c", "command": "x", "result": res, "evidence": "e"}  # noqa: E731
    case("test：找不到 verifier 的 criteria", stage="test", verify=env({"lens": "x", "findings": []}))
    case("test：criteria 是空的", stage="test", verify=crit())
    case("test：有 fail、not_verifiable", stage="test", verify=crit(C("C1", "pass"), C("C2", "fail"), C("C3", "not_verifiable")))
    case("test：criteria 的條目不是物件、沒有 id", stage="test", verify=crit("x", {"result": "fail"}, None))
    case("test：anti_cheat 有發現", stage="test", verify=crit(C("C1", "pass"), anti_cheat=[{"kind": "skip", "detail": "x"}, {"kind": "mock", "detail": "y"}]))
    case("test：多個 verifier 信封（分片）其中一個沒過", stage="test", verify=crit(C("a:C1", "pass")), shard=crit(C("b:C1", "fail")))
    w.call("policy_violations", stage="design", result=ok, run="@TMP@/proj/_runs/沒有", note="policy_violations：讀不到 run.json → fail-closed")
    w.write("proj/_runs/badrun/run.json", "{壞的")
    w.call("policy_violations", stage="design", result=ok, run="@TMP@/proj/_runs/badrun", note="policy_violations：run.json 不是 JSON → fail-closed")
    w.call("policy_violations", stage="plan", result=ok, run=d, note="policy_violations：plan 段一律由人核准")
    w.call("policy_violations", stage="自訂段", result=ok, run=d, note="policy_violations：不在 SDLC 的段不比 recipe")
    # B4.2：watcher 的事件檔與記憶檔被改，不影響判定
    d = w.ready("deliver", {"name": "bugfix"})
    r = w.result(d, "deliver")
    w.sr("stage", d, "deliver", stdin=r, note="stage deliver")
    w.call("policy_violations", stage="deliver", result=r, run=d, note="B4.2：改事件檔之前")
    w.write(f"{d}/progress.jsonl", '{"type": "stage_approved", "stage": "deliver", "by": "human"}\n不是 JSON\n')
    w.write(f"{d}/.watch-state.json", '{"view": {"stages": {"deliver": {"approved": true, "status": "invalid"}}}}')
    w.call("policy_violations", stage="deliver", result=r, run=d, note="B4.2：事件檔與記憶檔被改之後，政策判定不變")
    w.call("chain_errors", data=w.read_json(d), stage="merge", note="B4.2：閘門鏈不變（deliver 還沒核准）")
    w.sr("status", d, "--json", note="B4.2：status 不因事件檔而顯示已核准")
    w.sr("approve", d, "deliver", *POLICY, note="B4.2：之後照常核准")


def scenario_decisions(w):
    """決策記憶：decide、approve --answer、decision_errors、record_decisions。"""
    d = w.ready("plan-approve", {"name": "feature"}, name="dec")
    w.sr("approve", d, "plan", *HUMAN, "--answer", "匯出檔的編碼用什麼？=UTF-8 不加 BOM", "--answer", "要不要支援 xlsx＝這一版不要", "--answer-scope", "project", "--answer-expires", "2099-01-01",
         note="approve --answer：兩筆（半形與全形等號）、範圍 project、有到期日")
    w.sr("decide", d, "--question", "匯出檔的編碼用什麼？", "--answer", "改成加 BOM", "--by", "owner", "--at", AT, "--scope", "project", note="decide：同一題後面的答覆")
    w.sr("decide", d, "--question", "  欄位  順序\n照誰的  ", "--answer", " 照  報表頁 ", "--by", "owner", "--at", AT, note="decide：問題與答覆的空白會壓成一格；範圍預設 epic")
    w.sr("decide", d, "--question", "舊的決定", "--answer", "已經過期", "--by", "owner", "--at", AT, "--expires", "2020-01-01", note="decide：到期日在今天之前（檢視檔標過期）")
    w.sr("decide", d, "--question", "今天到期", "--answer", "a", "--by", "owner", "--at", AT, "--expires", "2026-11-12", note="decide：到期日就是今天")
    for args, note in [(("--question", "q", "--answer", "a"), "缺 --by 與 --at"), (("--question", "q", "--answer", "a", "--by", "policy", "--at", AT), "--by policy 不能記"),
                       (("--question", "q", "--answer", "a", *HUMAN, "--scope", "everywhere"), "範圍不認得"), (("--question", "q", "--answer", "a", *HUMAN, "--expires", "明年"), "到期日格式不對"),
                       (("--question", "q", "--answer", "a", *HUMAN, "--expires", "2026-13-45"), "到期日的形狀對、不是真的日期"), (("--question", "", "--answer", "a", *HUMAN), "問題是空的"),
                       (("--question", "q", "--answer", "  ", *HUMAN), "答覆只有空白"), (("--answer", "a", *HUMAN), "沒給 --question"),
                       (("--question", "q" * 301, "--answer", "a", *HUMAN), "問題超過 300 字"), (("--question", "q", "--answer", "a" * 601, *HUMAN), "答覆超過 600 字"),
                       (("--question", "q" * 300, "--answer", "a" * 600, *HUMAN), "剛好在上限")]:
        w.sr("decide", d, *args, note=f"decide：{note}")
    d2 = w.ready("plan-approve", {"name": "feature"}, name="dec")
    for args, note in [(("--answer", "沒有答覆"), "沒有等號"), (("--answer", "=只有答覆"), "問題是空的"), (("--answer", "q=a", "--answer-scope", "everywhere"), "範圍不認得"),
                       (("--answer", "q=a", "--answer-expires", "明年"), "到期日格式不對"), (("--answer",), "--answer 是最後一個參數")]:
        w.sr("approve", d2, "plan", *HUMAN, *args, note=f"approve --answer：{note} → 不記、不核准")
    w.sr("approve", d2, "plan", *HUMAN, "--answer", "a=b=c＝d", note="split_answer：只切第一個半形等號")
    d3 = w.ready("design")
    w.sr("stage", d3, "design", stdin=w.result(d3, "design"), note="stage design")
    w.sr("approve", d3, "design", *POLICY, "--answer", "q=a", note="approve --by policy 不能帶 --answer")
    # 寫不出來的時候
    w.mkdir("proj/_runs2")
    w.sr("init", "@TMP@/proj/_runs2/a", "--event", "sdlc", note="init：另一個 runs 目錄")
    w.mkdir(f"proj/_runs2/{pack.DECISIONS_VIEW}")
    w.sr("decide", "@TMP@/proj/_runs2/a", "--question", "q", "--answer", "a", *HUMAN, note="record_decisions：決策記憶.md 寫不出來（是目錄）→ 只警告，decisions.jsonl 已記錄")
    w.mkdir("proj/_runs3")
    w.sr("init", "@TMP@/proj/_runs3/a", "--event", "sdlc", note="init：第三個 runs 目錄")
    w.mkdir(f"proj/_runs3/{pack.DECISIONS}")
    w.sr("decide", "@TMP@/proj/_runs3/a", "--question", "q", "--answer", "a", *HUMAN, note="decide：decisions.jsonl 寫不了（是目錄）→ 不能記錄")
    w.sr("stage", "@TMP@/proj/_runs3/a", "plan", stdin=w.plan("@TMP@/proj/_runs3/a", {"name": "feature"}), note="stage plan")
    w.sr("approve", "@TMP@/proj/_runs3/a", "plan", *HUMAN, "--answer", "q=a", note="approve --answer：決定沒記成只警告，核准已記錄")
    w.call("record_decisions", run="@TMP@/proj/_runs2/a", items=[["q1", "a1"], ["q2", "a2"]], by="owner", at=AT, scope="epic", expires="", stage="design", note="record_decisions：直接呼叫（檢視檔寫不出來的警告）")
    w.call("record_decisions", run=d, items=[], by="owner", at=AT, scope="epic", expires="", note="record_decisions：沒有項目也重寫檢視檔")


def scenario_escalate(w):
    """escalate 的每個拒絕理由、加嚴後哪些段變 stale。"""
    d = w.ready("deliver", {"name": "bugfix"}, name="esc")
    for args, note in [((), "四個必填都沒給"), (("--recipe", "feature", "--by", "owner", "--at", AT), "缺 --reason"), (("--recipe", "docs", *HUMAN, "--reason", "想省事"), "降級（少段）"),
                       (("--recipe", "bugfix", "--modifiers", "zzz", *HUMAN, "--reason", "x"), "不認得的 modifier"), (("--recipe", "沒有這個", *HUMAN, "--reason", "x"), "不認得的 recipe → 當 feature（加嚴）"),
                       (("--recipe", "bugfix", *HUMAN, "--reason", "x", "--profile", "@TMP@/proj/沒有.json"), "--profile 讀不到")]:
        w.sr("escalate", d, *args, note=f"escalate：{note}")
        w.sr("status", d, note=f"status：escalate（{note}）之後")
    d = w.ready("deliver", {"name": "bugfix"}, name="esc")
    w.sr("escalate", d, "--recipe", "bugfix", "--modifiers", "migration", *HUMAN, "--reason", "碰到 migration", note="escalate：加 design 段 → 之後已 done 的 build、sync、test 變 stale")
    w.sr("status", d, "--json", note="status --json：escalate 之後")
    w.sr("stage", d, "deliver", stdin=w.result(d, "deliver"), note="stage deliver：前面有 stale 的段（存檔不擋）")
    w.sr("approve", d, "deliver", *HUMAN, note="chain_errors：design 還 pending、build 是 stale")
    w.sr("escalate", d, "--recipe", "bugfix", "--modifiers", "migration,ui", "--size", "large", *HUMAN, "--reason", "還有畫面", note="escalate：再升一次（history 兩筆、--size）")
    d = w.ready("deliver", {"name": "bugfix"}, name="esc")
    w.sr("escalate", d, "--recipe", "bugfix", "--modifiers", "security", *HUMAN, "--reason", "碰到權限", note="escalate：沒有新加的段，test 與 deliver 的設定變嚴 → test 變 stale")
    d = w.ready("merge", {"name": "docs"}, name="esc")
    w.sr("escalate", d, "--recipe", "docs", "--modifiers", "public_api", *HUMAN, "--reason", "對外契約", note="escalate：docs 加 design 段與 deliver 軸 → build、deliver 變 stale")
    w.sr("escalate", d, "--recipe", "change", *HUMAN, "--reason", "其實是改既有功能", note="escalate：docs → change（plan_mode 的必要節變多 → plan 也要重來）")
    w.sr("status", d, note="status：plan 變 stale 之後")
    w.sr("approve", d, "plan", *HUMAN, note="approve：stale 的段不能核准")
    d = w.new("esc")
    w.sr("escalate", d, "--recipe", "feature", "--modifiers", "ui", *HUMAN, "--reason", "補記", note="escalate：舊 run（沒有 recipe）→ 從 feature 加嚴，locked 看 plan 有沒有核准")
    w.sr("status", d, note="status：舊 run escalate 之後")
    d = w.ready("build", {"name": "hotfix"}, name="esc")
    w.sr("escalate", d, "--recipe", "bugfix", *HUMAN, "--reason", "不急了", note="escalate：hotfix → bugfix（followup 義務留著）")
    w.sr("status", d, note="status：需要後續 run")
    w.sr("abandon", d, "--reason", "不做了", *HUMAN, note="abandon")
    w.sr("escalate", d, "--recipe", "feature", *HUMAN, "--reason", "x", note="escalate：已取消的 run 不能升級")
    w.sr("escalate", "@TMP@/proj/_runs/bare", "--recipe", "feature", *HUMAN, "--reason", "x", note="escalate：不是 SDLC run（沒有 plan 段）")


def scenario_close(w):
    """followup、close、abandon、archive、status 的分支。"""
    d = w.ready("build", {"name": "hotfix"}, name="hot")
    for args, note in [((), "三個必填都沒給"), (("--run", "@TMP@/proj/_runs/沒有", *HUMAN), "後續 run 沒有 run.json"), (("--run", d, *HUMAN), "後續 run 不能是自己（先被 hotfix 擋）"),
                       (("--run", "@TMP@/proj/_runs/bare", *HUMAN), "後續 run 不是 SDLC run"), (("--run", "@TMP@/proj/_runs/badrun", *HUMAN), "後續 run 的 run.json 讀不懂")]:
        w.sr("followup", d, *args, note=f"followup：{note}")
    ab = w.new("gone")
    w.sr("abandon", ab, "--reason", "需求取消", *HUMAN, note="abandon：記錄並寫摘要")
    w.sr("followup", d, "--run", ab, *HUMAN, note="followup：後續 run 已取消")
    legacy = w.new("legacy")
    w.sr("followup", legacy, "--run", legacy, *HUMAN, note="followup：後續 run 不能是自己")
    w.sr("followup", legacy, "--run", legacy.replace("@TMP@/", "") + "/../" + legacy.rsplit("/", 1)[-1], *HUMAN, note="followup：換個寫法還是自己（resolve 後相同）")
    w.sr("followup", d, "--run", legacy, *HUMAN, note="followup：記錄（舊 run 也可以當後續 run）")
    w.sr("close", d, *HUMAN, note="close：有 followup_run 但流程還沒走完")
    w.sr("close", d, note="close：缺 --by 與 --at（加上流程沒走完）")
    w.advance(d, "merge")
    w.sr("close", d, *HUMAN, note="close：merge 段還沒 done")
    w.sr("stage", d, "merge", stdin=w.result(d, "merge"), note="stage merge")
    w.sr("abandon", legacy, "--reason", "不做了", *HUMAN, note="abandon：後續 run 之後被取消")
    w.sr("close", d, *HUMAN, note="close：followup_run 指的 run 已取消 → 不能結案")
    data = w.read_json(d)
    data["followup_run"] = {"by": "owner"}
    w.write_json(f"{d}/run.json", data)
    w.sr("close", d, *HUMAN, note="close：followup_run 沒有 run 鍵 → 讀不到")
    data["followup_run"] = "x"
    w.write_json(f"{d}/run.json", data)
    w.sr("close", d, *HUMAN, note="close：followup_run 是字串 → 沒接住的 AttributeError")
    f2 = w.ready("build", {"name": "bugfix"}, name="follow")
    w.sr("followup", d, "--run", f2, *HUMAN, note="followup：改記另一個後續 run")
    w.mkdir(f"{d}/archive/{save_run.ARCHIVE_NAME}")
    w.sr("close", d, *HUMAN, note="close：摘要寫不成（是目錄）→ 只提醒，結案照記")
    w.sr("archive", d, note="archive：摘要寫不成 → exit 1")
    w.rmtree(f"{d}/archive")
    w.sr("archive", d, note="archive：重寫成功")
    w.sr("close", d, *HUMAN, note="close：已結案再結一次（覆寫 closed）")
    w.sr("abandon", d, "--reason", "結案後取消", *HUMAN, note="abandon：結案後還是可以取消")
    w.sr("status", d, note="status：已取消又已結案")
    w.sr("status", d, "--json", note="status --json：abandoned 優先")
    # abandon
    d = w.ready("design", name="ab")
    for args, note in [((), "三個必填都沒給"), (("--reason", "x", "--by", "owner"), "缺 --at")]:
        w.sr("abandon", d, *args, note=f"abandon：{note}")
    w.sr("abandon", d, "--reason", "需求取消", *HUMAN, note="abandon：成功")
    w.sr("abandon", d, "--reason", "再取消一次", "--by", "別人", "--at", AT, note="abandon：已經取消過 → 保留原紀錄")
    w.sr("approve", d, "plan", *HUMAN, note="approve：已取消的 run 不能核准")
    w.sr("stage", d, "design", stdin=w.result(d, "design"), note="stage：已取消的 run 只存檔，並提醒")
    w.sr("approve", d, "design", *POLICY, note="approve --by policy：已取消")
    w.sr("close", d, *HUMAN, note="close：已取消的 run 不能結案")
    w.sr("followup", d, "--run", f2, *HUMAN, note="followup：已取消的 run 還是可以記後續 run")
    w.sr("status", d, note="status：已取消")
    w.sr("archive", d, note="archive：已取消的 run")
    data = w.read_json(d)
    data["abandoned"] = {"by": "owner"}
    w.write_json(f"{d}/run.json", data)
    w.sr("status", d, note="abandoned_error：缺 at、reason")
    data["abandoned"] = "取消了"
    w.write_json(f"{d}/run.json", data)
    w.sr("status", d, note="abandoned_error：abandoned 是字串 → 沒接住的 AttributeError")
    w.sr("approve", d, "plan", *HUMAN, note="approve：abandoned 是字串 → 沒接住的 AttributeError")
    # 摘要的內容
    d = w.ready("merge", {"name": "feature", "modifiers": ["security"]}, name="rich")
    t = w.result(d, "test")
    t["verify"]["payload"]["criteria"] += [{"id": "AC2", "criterion": "c", "command": "x", "result": "fail", "note": "匯出少一欄"}, {"id": "AC3", "criterion": "c", "command": "x", "result": "not_verifiable", "evidence": "沒有環境"}]
    t["verify"]["artifacts"] = ["specs\\e\\verifier.md", "", 5]
    t["verify"]["open_questions"] = [{"question": "要不要補英文版", "blocking": False}, {"question": "權限怎麼分" * 60, "blocking": True}, {"blocking": True}]
    t["trace"].append({"label": "review:bugs", "ok": False})
    w.write_json(f"{d}/test.json", t)
    w.write(f"{d}/build.json", "[1]")
    w.write(f"{d}/sync.json", "{壞的")
    w.call("archive_text", run=d, data=w.read_json(d), note="archive_text：產物、commit、驗證結果、問題、缺件的派工；段的結果不是物件或讀不懂當成空的")
    w.call("status_snapshot", run=d, data=w.read_json(d), note="status_snapshot：派工數取自各段結果的 trace")
    w.call("write_archive", run=d, data=w.read_json(d), note="write_archive：成功回空字串")
    w.call("write_archive", run=d, data={"stages": "x"}, note="write_archive：archive_text 丟例外 → 回警告文字")
    data = w.read_json(d)
    for change, note in [({"closed": {"by": "owner", "at": AT}}, "已結案"), ({"abandoned": {"by": "owner", "at": AT, "reason": "需求\n取消" * 80}}, "已取消（原因超過 200 字會截斷）"),
                         ({"requirement": "需 求" * 300}, "需求超過 500 字會截斷"), ({"followup_run": {"run": "proj\\_runs\\x", "note": "補完整流程"}}, "後續 run 與備註"),
                         ({"target_repo": "", "target": "備用的 target", "event": ""}, "沒有 target_repo 時用 target"), ({"stages": {}}, "沒有任何段"),
                         ({"recipe": {"name": "x", "modifiers": [], "resolved": {}}}, "recipe 沒有 stages")]:
        w.call("archive_text", run=d, data={**data, **change}, note=f"archive_text：{note}")
        w.call("status_snapshot", run=d, data={**data, **change}, note=f"status_snapshot：{note}")


def scenario_runjson(w):
    """run.json 本身有問題：每個指令的反應。"""
    cmds = [("status",), ("status", "--json"), ("approve", "plan", *HUMAN), ("decide", "--question", "q", "--answer", "a", *HUMAN), ("escalate", "--recipe", "feature", *HUMAN, "--reason", "x"),
            ("followup", "--run", "@TMP@/proj/_runs/rel", *HUMAN), ("close", *HUMAN), ("abandon", "--reason", "x", *HUMAN), ("archive",)]

    def each(d, note, label=None, stage=True):
        for c in cmds:
            argv = [c[0], d, *c[1:]]
            w.sr(*argv, label=label, note=f"{c[0]}：run.json {note}")
        if stage:
            w.sr("stage", d, "plan", stdin=w.plan(d, {"name": "docs"}, trace=False), label=label, note=f"stage：run.json {note}")

    for i, (text, note, label) in enumerate([("{壞的", "不是 JSON", None), ("", "是空檔", None), ("[]", "是陣列", None), ('"x"', "是字串", None), ("{}", "是空物件（沒有 stages）", None),
                                             ('{"stages": []}', "的 stages 是陣列", None), ('{"stages": {"plan": {}}}', "的 plan 段是空物件", None),
                                             ('{"stages": {"plan": {"status": 5, "gate": "yes"}}}', "的 status 是數字", None),
                                             ('{"stages": {"plan": {"status": null}}}', "的 status 是 null", None),
                                             ('{"stages": {"plan": {"status": "done", "gate": true, "envelopes": 1}}, "recipe": {"name": "docs", "resolved": []}}', "的 recipe.resolved 是陣列", None),
                                             ('{"stages": {"plan": {"status": "pending", "gate": true}}, "recipe": {"name": "docs", "modifiers": "ui", "resolved": {"stages": "plan"}}}', "的 recipe 欄位型別不對", None),
                                             ('{"event": "sdlc", "stages": {"plan": {"status": "pending", "gate": true}}, "x": NaN}', "有 NaN", "nan-run"),
                                             ('{"event": "sdlc", "stages": {"plan": {"status": "pending", "gate": true}}, "x": 123456789012345678901234567890}', "有超出 64 位元的整數", "huge-int-run"),
                                             ('{"event": "sdlc", "requirement": "\\ud800", "stages": {"plan": {"status": "pending", "gate": true}}}', "有落單的代理字元跳脫", "surrogate-run")]):
        d = f"@TMP@/proj/_bad{i}/r"
        w.write(f"{d}/run.json", text)
        each(d, note, label)
    w.write("proj/_runs/latin1/run.json", raw=b'{"stages": {}, "x": "\xff"}')
    each("@TMP@/proj/_runs/latin1", "不是 utf-8", stage=False)
    w.mkdir("proj/_runs/isdir/run.json")
    each("@TMP@/proj/_runs/isdir", "是目錄", stage=False)


def scenario_calls(w):
    """只能在程式內呼叫、要碰檔案的函式。"""
    d = w.ready("build", {"name": "bugfix"}, name="call")
    w.call("load", run=d, note="load：正常的 run.json")
    w.call("load", run="@TMP@/proj/_runs/沒有", note="load：不存在")
    w.call("load", run="@TMP@/proj/_runs/badrun", note="load：不是 JSON")
    w.call("followup_run_errors", frun=d, note="followup_run_errors：合格的 SDLC run")
    w.call("followup_run_errors", frun="@TMP@/proj/_runs/沒有", note="followup_run_errors：讀不到")
    w.call("followup_run_errors", frun="@TMP@/proj/_runs/bare", note="followup_run_errors：沒有 plan 段")
    for i, (data, note) in enumerate([({"stages": None}, "stages 是 null"), ({"stages": ["plan"]}, "stages 是陣列（裡面有 plan）"), ({"stages": "plan"}, "stages 是字串（子字串）"), ([], "run.json 是陣列"),
                                      ({"stages": {"plan": {}}, "abandoned": 1}, "abandoned 是 1"), ({"stages": {"plan": {}}, "recipe": {"name": "hotfix", "resolved": {"followup": True}}}, "recipe 是 hotfix"),
                                      ({"stages": {"plan": {}}, "recipe": {"resolved": {"followup": 1}}}, "followup 是 1、沒有 name")]):
        w.write_json(f"proj/_runs/fr{i}/run.json", data)
        w.call("followup_run_errors", frun=f"@TMP@/proj/_runs/fr{i}", note=f"followup_run_errors：{note}")
    plan = w.read_json(d, "plan.json")
    for prof, mode, note in [(None, "feature", "沒有 profile"), (BASE, "diagnose", "diagnose"), ({"stages": {"plan": {"required_sections": ["問題"]}}}, "feature", "profile 覆寫必要節"),
                             ({"plan_modes": {"feature": {"sections": ["風險"]}}}, "feature", "plan_modes 加節"), (BASE, "沒有這個 mode", "不認得的 plan_mode"), (BASE, None, "plan_mode 是 None")]:
        w.call("intent_errors", result=plan, profile=prof, plan_mode=mode, note=f"intent_errors：{note}")
    for res, note in [(None, "回傳是 None"), ([], "回傳是陣列"), ({"intent": plan["intent"]}, "沒有 epic_dir"), ([plan["intent"]], "陣列裡有 intent 信封（沒有 epic_dir）")]:
        w.call("intent_errors", result=res, profile=None, plan_mode="feature", note=f"intent_errors：{note}")


# ------------------------------------------------------------------ 探測（不碰檔案）
WEIRD = [None, True, False, 0, 1, -1, 1.5, "", "x", [], ["x"], [None], {}, {"a": 1}]
DROP = object()


def put(base, path, value):
    """base 的深層複本，path 那個位置換成 value（DROP＝拿掉那個鍵）。"""
    out = copy.deepcopy(base)
    cur = out
    for k in path[:-1]:
        cur = cur[k]
    if value is DROP:
        if isinstance(cur, dict):
            cur.pop(path[-1], None)
        else:
            cur.pop(path[-1])
    else:
        cur[path[-1]] = value
    return out


def mutations(base, paths, values=None):
    values = WEIRD + [DROP] if values is None else values
    for p in paths:
        for v in values:
            yield put(base, p, v)


def probes():
    out = []

    def probe(fn, **kw):
        out.append({"fn": fn, "args": fold(kw), "result": tried(fn, kw)})

    feature = rcp.resolve(BASE, "feature")
    locked = lambda name="feature", mods=(): {"name": name, "modifiers": list(mods), "size": "", "resolved": rcp.resolve(BASE, name, list(mods)), "locked": True, "history": []}  # noqa: E731
    # opt／opts
    for argv in [[], ["--by"], ["--by", "a"], ["--by", "a", "--by", "b"], ["x", "--by", "--at", "t"], ["--at", "--by"], ["--by", ""], ["--answer", "1", "--answer", "2", "--answer"]]:
        for name in ["--by", "--at", "--answer", ""]:
            probe("opt", argv=argv, name=name)
            probe("opt", argv=argv, name=name, default="預設")
            probe("opts", argv=argv, name=name)
    # split_answer／decision_errors
    for raw in ["", "q", "q=a", "q＝a", " q = a ", "=a", "q=", "＝", "a=b=c", "a＝b=c", "a=b＝c", "==", "q\n=\ta", "　q　=　a　"]:
        probe("split_answer", raw=raw)
    for items in [[], [["q", "a"]], [["", "a"]], [["q", ""]], [["", ""]], [[" ", "a"]], [["q" * 301, "a"]], [["q", "a" * 601]], [["q" * 300, "a" * 600]], [["", "a" * 70]], [["問" * 80, " "]],
                  [["q", "a"], ["", ""], ["q" * 400, "a"]]]:
        for by in ["owner", "policy", ""]:
            for scope in ["epic", "project", "", "Epic", "everywhere"]:
                for expires in ["", "2026-01-01", "2026-1-1", "明年", "2026-01-01 ", "2026-01-01x", "0000-00-00", "２０２６-01-01"]:
                    if (by, scope, expires) == ("owner", "epic", "") or items == [["q", "a"]]:
                        probe("decision_errors", items=items, by=by, scope=scope, expires=expires)
    # is_envelope／envelopes／missing_agents
    e1, e2 = env({"a": 1}), env({"b": None}, status="blocked")
    shapes = WEIRD + [e1, {"status": "done"}, {"payload": {}}, {"status": None, "payload": None}, [e1, e2], {"a": e1, "b": [e2, None], "c": {"d": e1}}, {"x": {"status": 1, "payload": 2, "inner": e1}},
                      {"a": None, "gate": None, "reason": None, "trace": None, "recipe": None}, {"a": {"trace": None, "recipe": None, "gate": None}}, {"trace": [None], "recipe": {"x": None}},
                      {"a": [[None, [None]], {"b": [None]}]}, [None, None], [[]], {"": None}, {"a.b": None, "c[0]": None}, {"status": "done", "payload": {}, "extra": None}, [{"gate": None, "x": None}]]
    for v in shapes:
        probe("is_envelope", v=v)
        probe("envelopes", v=v)
        probe("missing_agents", v=v)
        probe("md_cell", v=v)
    for v in ["a|b", "a\nb", "|", "\n", "—", " ", "a\r\nb", 0.0, -0.0, 1e20, 1e-7, 10 ** 15, True]:
        probe("md_cell", v=v)
    # _envelope_of
    envs = [env({"a": 1}, artifacts=["runs/r1/design/outline.md", "specs/e/design.md"]), env({"b": 1}, summary="第二份", artifacts=["design/challenge.md"]), env({"c": 1}, summary="第三份", status="partial"),
            env({"d": 1}, summary="x" * 500)]
    entries = [{"record": "runs/r1/design/outline.md"}, {"record": "runs\\r1\\design\\outline.md"}, {"record": "other/design/challenge.md"}, {"record": "challenge.md"}, {"record": "x/y/z.md"},
               {"record": ""}, {}, {"record": None}, {"record": "z.md", "digest": {"status": "done", "summary": "第二份"}}, {"record": "z.md", "digest": {"status": "partial", "summary": "第三份"}},
               {"record": "z.md", "digest": {"status": "done", "summary": "第三份"}}, {"record": "z.md", "digest": {"summary": "第二份"}}, {"record": "z.md", "digest": {"status": None, "summary": "第二份"}},
               {"record": "z.md", "digest": {"status": "done", "summary": "s"}}, {"record": "z.md", "digest": {"status": "done", "summary": "x" * 400}}, {"record": "z.md", "digest": {"status": "done", "summary": "x" * 500}},
               {"record": "z.md", "digest": {"status": "done", "summary": ""}}, {"record": "z.md", "digest": {"status": "done", "summary": 5}}, {"record": "z.md", "digest": "x"}, {"record": "z.md", "digest": None},
               {"record": 5}, {"record": ["a"]}, {"digest": {"status": 0, "summary": "第二份"}}]
    for e in entries:
        for stage in ["design", "plan", ""]:
            probe("envelope_of", entry=e, stage=stage, envs=envs)
        probe("envelope_of", entry=e, stage="design", envs=[])
        probe("envelope_of", entry=e, stage="design", envs=[envs[1], copy.deepcopy(envs[1])])
        probe("envelope_of", entry=e, stage="design", envs=[envs[1], {**envs[1], "confidence": 0.5}])
    for bad in [{"artifacts": "design/outline.md"}, {"artifacts": None}, {"artifacts": 5}, {"artifacts": [None, 5, ["x"]]}, {"summary": None}, {"summary": 5, "status": None}, {"status": 0, "summary": "第二份"},
                {"artifacts": {"design/outline.md": 1}}]:
        for e in entries[:4] + entries[8:10]:
            probe("envelope_of", entry=e, stage="design", envs=[{**envs[0], **bad}])
    for e in WEIRD:
        probe("envelope_of", entry=e, stage="design", envs=envs)
        probe("envelope_of", entry={"record": "runs/r1/design/outline.md", "digest": {"status": "done", "summary": "第二份"}}, stage="design", envs=[e])
    # _retried_ok
    t_fail = {"label": "a", "agent_type": "x", "ok": False, "superseded": True}
    for trace in [[t_fail], [t_fail, {"label": "a", "agent_type": "x", "ok": True}], [t_fail, {"label": "a", "agent_type": "y", "ok": True}], [t_fail, {"label": "b", "agent_type": "x", "ok": True}],
                  [t_fail, {"label": "a", "agent_type": "x", "ok": 1}], [t_fail, "x", None, {"label": "a", "agent_type": "x", "ok": True}], [{**t_fail, "superseded": 1}, {"label": "a", "agent_type": "x", "ok": True}],
                  [{"ok": False, "superseded": True}, {"ok": True}], [{"ok": False, "superseded": True, "label": None}, {"ok": True}], [{"label": "a", "agent_type": "x", "ok": True}, t_fail],
                  ["x", t_fail], [None], [5], [[]], [{}]]:
        for i in range(len(trace)):
            probe("retried_ok", trace=trace, i=i)
    # model_errors
    models = {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"}, "defaults": {"*": "standard"}, "floors": {}}
    prof = {**BASE, "models": models}
    te = lambda role="dev-implementer", tier="standard", model="sonnet", **kw: {"label": "L", "agent_type": NS + role, "ok": True, "tier": tier, "model": model, **kw}  # noqa: E731
    traces = [[te()], [te(tier="light", model="haiku")], [te("dev-merge-steward")], [te("dev-merge-steward", "strong", "opus")], [te("tpm-skeptic")], [te("dev-security-reviewer", "standard", "sonnet")],
              [te(tier=None)], [te(tier="")], [te(tier="ultra")], [te(model=None)], [te(model="opus")], [te("my-agent", "", "")], [{"label": "x"}], [{"agent_type": "dev-verifier", "tier": "strong", "model": "opus"}],
              [{"agent_type": "a:b:tpm-writer", "tier": "light", "model": "haiku"}], ["x", None, 5], [], [te(tier=5)], [te(tier=["strong"])], [te(tier={"a": 1})], [te(tier=True)], [te(tier=0)],
              [{"label": None, "agent_type": None, "tier": "light"}], [{"label": 5, "agent_type": 5, "tier": "light"}], [{"agent_type": ["dev-x"], "tier": "light"}], [te("dev-spec-author", "light", "haiku")],
              [te("dev-spec-author", "strong", "opus"), te("tpm-researcher", "light", "sonnet")]]
    for tr in traces:
        for stage in ["plan", "build", "merge", "run"]:
            probe("model_errors", stage=stage, result={"trace": tr}, profile=prof, modifiers=[])
        probe("model_errors", stage="test", result={"trace": tr}, profile=prof, modifiers=["security", "large"])
        probe("model_errors", stage="test", result={"trace": tr}, profile={**BASE, "models": {**models, "floors": {"dev-implementer": "strong", "tpm-skeptic@test": "strong"}}}, modifiers=None)
        probe("model_errors", stage="plan", result={"trace": tr}, profile={"models": {"tiers": {"strong": "opus"}}}, modifiers="security")
    for m in WEIRD + [{"mode": "inherit"}, {"mode": "auto"}, {"mode": None}, {"mode": "x", "tiers": None}, {"tiers": "x"}, {"tiers": {"standard": 5}}, {"tiers": {"standard": ""}}, {"floors": "x"}, {"defaults": None},
                      {"floors": {"dev-implementer": "ultra"}}, {"floors": {"dev-implementer": 5}}]:
        probe("model_errors", stage="build", result={"trace": [te(), te(tier="strong", model="opus")]}, profile={"models": m}, modifiers=[])
    for res in WEIRD + [{"trace": None}, {"trace": "x"}, {"trace": {"a": 1}}, {"trace": 5}]:
        probe("model_errors", stage="build", result=res, profile=prof, modifiers=[])
    for p in WEIRD:
        probe("model_errors", stage="build", result={"trace": [te()]}, profile=p, modifiers=[])
        probe("model_errors", stage="build", result={"trace": [te()]}, profile=prof, modifiers=p)
    # policy_violations（不給 run）
    design = {"stage": "design", "design": env({"epic": "e", "subtasks": [SUB]}, open_questions=[{"question": "q", "blocking": False}]),
              "challenge": env({"verdicts": [{"target_id": "x", "refuted": False, "reason": "成立"}]}),
              "review": env({"lens": "plan", "findings": [{"id": "F1", "severity": "nit", "claim": "c", "evidence": []}]})}
    test = {"stage": "test", "verify": env({"commit": "a" * 40, "anti_cheat": [], "criteria": [{"id": "C1", "result": "pass"}]})}
    for st in ["plan", "design", "build", "test", "deliver", "merge", "run", ""]:
        for res in WEIRD + [design, test, [design, test], {"a": design, "b": test}]:
            probe("policy_violations", stage=st, result=res)
    paths = [("halted",), ("halt_reason",), ("custom_script",), ("override_script",), ("manual_output",), ("design",), ("challenge",), ("review",), ("design", "open_questions"), ("design", "open_questions", 0),
             ("design", "open_questions", 0, "blocking"), ("design", "open_questions", 0, "question"), ("design", "payload"), ("design", "status"), ("challenge", "payload"), ("challenge", "payload", "verdicts"),
             ("challenge", "payload", "verdicts", 0), ("challenge", "payload", "verdicts", 0, "refuted"), ("challenge", "payload", "verdicts", 0, "target_id"), ("challenge", "payload", "verdicts", 0, "reason"),
             ("review", "payload", "findings"), ("review", "payload", "findings", 0), ("review", "payload", "findings", 0, "severity"), ("review", "payload", "findings", 0, "id"),
             ("review", "payload", "findings", 0, "claim")]
    for m in mutations(design, paths):
        probe("policy_violations", stage="design", result=m)
    for m in mutations({**design, "halted": True}, [("halt_reason",), ("halted",)], ["原因", "", None, 0, ["a"], {"k": 1}, "x" * 200]):
        probe("policy_violations", stage="design", result=m)
    for m in mutations(design, [("review", "payload", "findings", 0, "severity")], ["blocker", "important", "Blocker", "minor", "major", ["blocker"]]):
        probe("policy_violations", stage="deliver", result=m)
    tpaths = [("verify",), ("verify", "payload"), ("verify", "payload", "criteria"), ("verify", "payload", "criteria", 0), ("verify", "payload", "criteria", 0, "result"), ("verify", "payload", "criteria", 0, "id"),
              ("verify", "payload", "anti_cheat")]
    for m in mutations(test, tpaths):
        probe("policy_violations", stage="test", result=m)
        probe("policy_violations", stage="design", result=m)
    for m in mutations(test, [("verify", "payload", "criteria", 0, "result")], ["pass", "fail", "not_verifiable", "PASS", "pass "]):
        probe("policy_violations", stage="test", result=m)
    for m in mutations(test, [("verify", "payload", "anti_cheat")], [[{"kind": "skip"}], [1, 2, 3], "abc", {"a": 1, "b": 2}, 7]):
        probe("policy_violations", stage="test", result=m)
    # recipe_mismatch／required_roles_missing／has_recipe
    data = {"recipe": locked("feature", ["security", "ui"]), "stages": {}}
    echo = {"name": "feature", "modifiers": ["security", "ui"], "stages": feature["stages"]}
    for m in mutations({"recipe": echo}, [("recipe",), ("recipe", "name"), ("recipe", "modifiers"), ("recipe", "stages")],
                       WEIRD + [DROP, "feature", "ui,security", " ui , security ", ["ui", "security"], ["security"], ["security", "ui", "ui"], feature["stages"], feature["stages"][:-1], "plan", [1, 2]]):
        probe("recipe_mismatch", data=data, result=m)
    for r in WEIRD:
        probe("recipe_mismatch", data=data, result=r)
    for m in mutations(data, [("recipe",), ("recipe", "name"), ("recipe", "modifiers"), ("recipe", "resolved"), ("recipe", "resolved", "stages")]):
        probe("recipe_mismatch", data=m, result={"recipe": echo})
        probe("has_recipe", data=m)
        probe("required_roles_missing", data=m, stage="test", result={"trace": []})
    for d_ in WEIRD:
        probe("has_recipe", data=d_)
    tr = [{"label": "verify#1", "agent_type": NS + "dev-verifier", "ok": True}, {"label": "security", "agent_type": NS + "dev-security-reviewer", "ok": True},
          {"label": "design", "agent_type": "dev-design-reviewer", "ok": True}, {"label": "review:standards", "agent_type": "x", "ok": True}, {"label": "review:spec", "ok": True},
          {"label": "review:bugs", "ok": True}, {"label": "review:history", "ok": True}]
    for stage in ["test", "deliver", "design", "merge"]:
        for name, mods in [("feature", ()), ("feature", ("security", "ui")), ("deps", ()), ("docs", ()), ("spike", ()), ("hotfix", ("security",))]:
            dd = {"recipe": locked(name, mods)}
            probe("required_roles_missing", data=dd, stage=stage, result={"trace": tr})
            probe("required_roles_missing", data=dd, stage=stage, result={"trace": []})
            for k in range(len(tr) if stage in ("test", "deliver") else 0):
                probe("required_roles_missing", data=dd, stage=stage, result={"trace": tr[:k] + tr[k + 1:]})
                probe("required_roles_missing", data=dd, stage=stage, result={"trace": tr[:k] + [{**tr[k], "ok": 1}] + tr[k + 1:]})
        for res in WEIRD + [{"trace": None}, {"trace": "x"}, {"trace": ["x", None, {"ok": True}, {"ok": True, "agent_type": 5, "label": 5}, {"ok": True, "agent_type": None, "label": None}]}]:
            probe("required_roles_missing", data=data, stage=stage, result=res)
        for m in mutations(data, [("recipe", "resolved", "test"), ("recipe", "resolved", "test", "security_review"), ("recipe", "resolved", "deliver_axes")],
                           WEIRD + [DROP, "always", "ALWAYS", ["bugs", 5, None], "bugs"]):
            probe("required_roles_missing", data=m, stage=stage, result={"trace": tr})
    # _mods／recipe_summary
    for v in WEIRD + ["ui", "ui,security", " ui , , security ", ",", ["ui", " security ", "", None, 5], ("a", "b"), {"ui": 1, "security": 2}, 5.5, "ui，security", [["ui"]], [True, False]]:
        probe("mods", v=enc(v))
    full = {**locked("bugfix", ["ui"]), "size": "small", "resolved_from": "profile"}
    for m in list(mutations(full, [("name",), ("modifiers",), ("size",), ("resolved",), ("resolved", "stages"), ("resolved", "plan_mode"), ("resolved", "followup"), ("resolved_from",), ("locked",)])) + [
            locked("hotfix"), locked("spike"), {}, {"resolved": {"stages": ["plan"], "followup": True}}]:
        probe("recipe_summary", rec=m)
    for r in WEIRD:
        probe("recipe_summary", rec=r)
    # chain_errors
    st_ok = lambda gate: {"status": "done", "gate": gate, "approved": {"by": "o", "at": "t"}}  # noqa: E731
    full_stages = {s: st_ok(s in save_run.GATED) for s in save_run.SDLC}
    runs = [{"stages": full_stages}, {"stages": full_stages, "recipe": locked("bugfix")}, {"stages": full_stages, "recipe": locked("docs")}, {"stages": full_stages, "recipe": locked("spike")},
            {"stages": full_stages, "recipe": {"name": "x", "resolved": {}}}, {"stages": full_stages, "recipe": {"name": "x", "resolved": {"stages": None}}},
            {"stages": full_stages, "recipe": {"name": "x", "resolved": {"stages": "plan,design"}}}, {"stages": full_stages, "recipe": {"resolved": {"stages": ["design", "plan", "zzz"]}}}]
    for r in runs:
        for stage in save_run.SDLC + ["run", "", "PLAN"]:
            probe("chain_errors", data=r, stage=stage)
            probe("chain_errors", data=r["stages"], stage=stage)
        for prev in (["plan", "design", "sync", "test"] if r in runs[:3] else []):
            for v in WEIRD + [DROP, {"status": "skipped"}, {"status": "pending", "gate": True}, {"status": "done", "gate": True}, {"status": "done", "gate": False}, {"status": "done", "gate": 1, "approved": {}},
                              {"status": "done", "gate": "yes", "approved": "x"}, {"status": "stale"}, {"status": "invalid"}, {"status": None}, {"status": ["done"]}, {"gate": True}]:
                probe("chain_errors", data=put(r, ("stages", prev), v), stage="merge" if prev == "test" else "deliver")
    for v in WEIRD:
        probe("chain_errors", data=v, stage="test")
        probe("chain_errors", data={"stages": v}, stage="test")
        probe("chain_errors", data={"stages": full_stages, "recipe": v}, stage="test")
        probe("chain_errors", data={"stages": full_stages, "recipe": {"name": "x", "resolved": v}}, stage="test")
        probe("chain_errors", data={"stages": full_stages, "recipe": {"name": v, "resolved": {"stages": v}}}, stage="test")
    # abandoned_error／next_stage
    for v in WEIRD + [{"by": "o", "at": "t", "reason": "r"}, {"by": None, "at": None, "reason": None}, {"by": 5}, {"reason": ["x"]}]:
        probe("abandoned_error", data={"abandoned": v})
        probe("abandoned_error", data=v)
    for stages in [full_stages, {}, {"plan": {"status": "pending"}}, {"plan": {"status": "done", "gate": True}}, {"plan": {"status": "done", "gate": True, "approved": {}}},
                   {"plan": {"status": "skipped"}, "x": {"status": "invalid"}}, {"a": {"status": "done"}, "b": {"status": "done", "gate": 0}}, {"plan": {}}, {"plan": None}, {"plan": "done"}, {"plan": {"status": None}},
                   {"plan": {"status": "done", "gate": True, "approved": {"by": "o"}}, "design": {"status": "stale", "gate": True}}, {"": {"status": "x"}}]:
        probe("next_stage", data={"stages": stages})
    for v in WEIRD:
        probe("next_stage", data=v)
        probe("next_stage", data={"stages": v})
    # _escalate_affected
    names = list(rcp.BUILTIN_RECIPES)
    combos = [(n, ()) for n in names] + [("feature", ("ui",)), ("feature", ("security",)), ("bugfix", ("migration",)), ("docs", ("public_api",)), ("feature", ("large",)), ("bugfix", ("ui", "security", "migration"))]
    for a, am in combos:
        for b, bm in combos[::3]:
            new, old = rcp.resolve(BASE, a, list(am)), rcp.resolve(BASE, b, list(bm))
            probe("escalate_affected", new=rcp.merge_stricter(new, old), old=old, profile=BASE)
    for m in mutations(feature, [("stages",), ("design_lenses",), ("test",), ("test", "modes"), ("test", "design_review"), ("test", "security_review"), ("deliver_axes",), ("plan_mode",)],
                       WEIRD + [DROP, "always", "never", "auto", "zzz", ["x", "y"], "diagnose"]):
        probe("escalate_affected", new=m, old=feature, profile=None)
        probe("escalate_affected", new=feature, old=m, profile=BASE)
    for v in WEIRD:
        probe("escalate_affected", new=v, old=feature, profile=None)
        probe("escalate_affected", new=feature, old=v, profile=None)
        probe("escalate_affected", new=feature, old=feature, profile=v)
    # _clip／AC_ID／_result_subtasks／plan_check_issues
    for s in WEIRD + ["a  b\n\tc", "長" * 300, "長" * 301, "x" * 10, " 前後 ", "a　b", "a\xa0b", "a\x1cb", "a\x85b", "a b"]:
        for n in [300, 5, 1, 2]:
            probe("clip", s=s, n=n)
    for text in ["AC-1", "AC_1", "AC-1.2-b_c", "xAC-1", "-AC-1", "_AC-1", "中AC-1", "（AC-1）", "AC-", "AC-1.", "AC-1..2", "AC-MUT-3 與 AC-2、AC-2", "ac-1", "AC-１", "AC-一", "9AC-1", "AC-1AC-2", "AC-1 AC_2\nAC-3",
                 "", "AC", "ÉAC-1", "éAC-1", "AC-é", "#AC-1", "AC-1-", "AC-a.b.c", "AC-a__b"]:
        probe("ac_ids", text=text)
    T = lambda i="t1", **kw: {"id": i, "title": i, "kind": "code", "owns": ["a"], "depends_on": [], "acceptance": ["AC-1"], **kw}  # noqa: E731
    results = [{"design": {"payload": {"subtasks": [T()]}}}, {"payload": {"subtasks": [T()]}}, {"subtasks": [T()]}, {"design": {"payload": {"subtasks": []}}, "subtasks": [T("top")]},
               {"design": {"payload": {"subtasks": [T(), "x"]}}, "payload": {"subtasks": [T("p")]}}, {"design": "x", "subtasks": [T()]}, {"design": {"payload": "x"}}, {"design": {"payload": None}},
               {"design": None, "payload": None}, {"subtasks": "x"}, {"subtasks": {"a": 1}}, {"subtasks": [None]}, {"design": {"subtasks": [T()]}}, {"design": [], "payload": [], "subtasks": []}]
    for r in WEIRD + results:
        probe("result_subtasks", result=r)
        probe("plan_check_issues", result=r)
    subsets = [[T("a"), T("a"), T("b"), T("b"), T("")], [T("a", depends_on=["b"]), T("b", depends_on=["c"]), T("c", depends_on=["a"]), T("d", depends_on=["a"])], [T("a", depends_on=["a"])],
               [T("a", depends_on=["x", "y", 5, None])], [T("a", depends_on="bc"), T("b"), T("c")], [T(5), T(5.0), T(None), T(True)], [T("a", acceptance=[" ", "", None]), T("b", acceptance=[0])],
               [T("a", kind="doc", owns=[" "]), T("b", kind="test", owns=None), T("c", kind="research", owns=None), T("d", kind=None, owns=[]), T("e", kind=["code"], owns=[])],
               [{"id": "a"}], [{}], [{"depends_on": None, "acceptance": None, "owns": None, "kind": "code"}], [T("b", depends_on=["a"]), T("a")], [T("a", depends_on=["a", "b"]), T("a", depends_on=[]), T("b")]]
    for subs in subsets:
        probe("plan_check_issues", result={"subtasks": subs})
    for m in mutations({"subtasks": [T("a", depends_on=["b"], story_ids=["AC-1"]), T("b")]}, [("subtasks", 0, k) for k in ["id", "depends_on", "acceptance", "kind", "owns", "story_ids"]]):
        probe("plan_check_issues", result=m)
    return out


def main():
    r = Rec()
    prev = os.getcwd()
    saved = (save_run.PROJECT_PROFILE, pack.verify_reuse, pack.record_reuse, pack.spec_dir)
    root = Path(r.td) / "proj"
    save_run.PROJECT_PROFILE = Path(r.td) / PROFILE
    pack.verify_reuse = lambda run, repo, result, requirement=None, root_=root: saved[1](run, repo, result, requirement, root_)
    pack.record_reuse = lambda run, repo, result, requirement, root_=root: saved[2](run, repo, result, requirement, root_)
    pack.spec_dir = lambda repo, epic, raw="", root_=root: saved[3](repo, epic, raw, root_)
    os.chdir(r.td)
    try:
        fixture(r)
        scenario_cli(r)
        scenario_lifecycle(r)
        scenario_stage_input(r)
        scenario_stage_envelopes(r)
        scenario_plan_recipe(r)
        scenario_intent(r)
        scenario_profile(r)
        scenario_trace(r)
        scenario_recipe_checks(r)
        scenario_models(r)
        scenario_reuse(r)
        scenario_constraints(r)
        scenario_plan_check(r)
        scenario_approve(r)
        scenario_policy(r)
        scenario_decisions(r)
        scenario_escalate(r)
        scenario_close(r)
        scenario_runjson(r)
        scenario_calls(r)
        pr = probes()
    finally:
        os.chdir(prev)
        save_run.PROJECT_PROFILE, pack.verify_reuse, pack.record_reuse, pack.spec_dir = saved
        shutil.rmtree(r.td, ignore_errors=True)
    result = {"說明": "save_run 的對跑基準，由 tests/parity/record_save_run.py 產生；不要手改。", "today": "%04d-%02d-%02d" % TODAY, "steps": r.steps, "probes": pr}
    # 一步一行：檔案不會太大，差異也好讀
    one = lambda v: json.dumps(v, ensure_ascii=False)  # noqa: E731
    sys.stdout.write("{\n \"說明\": %s,\n \"today\": %s,\n \"consts\": %s,\n \"steps\": [\n%s\n ],\n \"probes\": [\n%s\n ]\n}\n" % (
        one(result["說明"]), one(result["today"]), one(CONSTS), ",\n".join("  " + one(x) for x in r.steps), ",\n".join("  " + one(x) for x in pr)))


if __name__ == "__main__":
    main()
