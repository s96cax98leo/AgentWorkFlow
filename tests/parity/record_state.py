#!/usr/bin/env python3
"""錄狀態機（awf.py）的對跑基準。

用法：python3 tests/parity/record_state.py > tests/parity/state.expected.json

基準檔本身就是劇本：`steps` 依序是檔案操作（copytree、write、rm、utime）、指令（run：argv、環境、固定的時間、exit code、
stdout、stderr、這一步改了哪些檔）與只能在程式內呼叫的函式（call）。Rust 的測試（crates/awf-core/tests/state_parity.rs）
照同一份劇本做一遍，逐字比對。

做法：在暫存目錄組一個虛構的專案（示範客戶 ACME）：`proj/.claude/teams/` 從 scaffold 複製（工具、schema、內建 profile、
交付文件型錄），`plugin/` 放真的 12 支範本，`home/` 是 Claude Code 的設定目錄，`tr/` 是 workflow 紀錄。
awf.py 從專案裡那一份載入（所以它認的專案根目錄就是 `proj`，不會把模版 repo 當成 plugin 來源）。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`（用實體路徑）、模版 repo 換成 `@SRC@`；工作目錄切到 `proj`。
- 時間：awf.py 的 `datetime` 換成固定時鐘（每跑一個指令前進一秒，記在步驟的 `now`）；pack 與 save_run 的「今天」固定成 TODAY。
  workflow 紀錄與 session 檔的修改時間一律用 utime 明確設定。
- 快照只看 `proj`（略過複製進來的工具、schema、知識庫索引）與 `out`；比 LONG 長的檔案內容只記 sha1 與長度
  （產生的 workflow 腳本因此是逐檔比雜湊）。
- 用法訊息只比 exit code。Python 沒接住的例外（traceback、exit 1）記類別名：Rust 要一樣 exit 1、stderr 提到同一個類別。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
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
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
os.environ.pop("CLAUDE_PROJECT_DIR", None)

LONG = 1500
NOW0 = 1794445200.0          # 2026-11-12T01:00:00Z
TODAY = (2026, 11, 12)
NS = "agent-workflow:"
COPIES = [("scaffold/.claude/teams/tools", "proj/.claude/teams/tools"), ("scaffold/.claude/teams/schemas", "proj/.claude/teams/schemas"),
          ("scaffold/.claude/teams/methodologies", "proj/.claude/teams/methodologies"), ("scaffold/.claude/teams/deliverables", "proj/.claude/teams/deliverables"),
          ("plugin/workflow-templates", "plugin/workflow-templates")]
SNAP_ROOTS = ["proj", "out"]
SNAP_SKIP = ["proj/.claude/teams/tools", "proj/.claude/teams/schemas", "proj/.claude/teams/methodologies", "proj/.claude/teams/deliverables", "proj/_tpm/kb"]
CONFIG = 'AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE="繁體中文"\nBRANCH_PREFIX=feature/\nFORK_REPO=\nBRIEFS_DIR=briefs\nDECISION_OWNER=示範客戶 ACME 的負責人\nREPOS_DIR=repos\nTASK_AUTHORITY=briefs\nUPSTREAM_REPO=acme/app\nFORBIDDEN_REPOS=\nTASK_ID_PREFIX=TASK-\nSTAKEHOLDER=需求方\nRUNS_DIR=_runs\nATTRIBUTION_LINE=Assisted-by: Claude\n'
REQ = "匯出報表要有 CSV"

awf = save_run = pack = rcp = awf_script = None
DEFAULT = None


class FixedDate(datetime.date):
    @classmethod
    def today(cls):
        return cls(*TODAY)


def fake_datetime():
    """整個 datetime 模組照舊，只有 date.today() 固定（執行中才 import datetime 的程式也拿得到完整的模組）。"""
    fake = types.ModuleType("datetime")
    fake.__dict__.update({k: v for k, v in datetime.__dict__.items() if not k.startswith("__")})
    fake.date = FixedDate
    return fake


@contextlib.contextmanager
def fixed_today():
    real = sys.modules["datetime"]
    sys.modules["datetime"] = fake_datetime()
    try:
        yield
    finally:
        sys.modules["datetime"] = real


def env(status="done", summary="做完", **kw):
    e = {"status": status, "summary": summary, "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": {}}
    e.update(kw)
    return e


def enc(v):
    if isinstance(v, Path):
        return str(v).replace("\\", "/")
    if isinstance(v, (list, tuple)):
        return [enc(x) for x in v]
    if isinstance(v, (set, frozenset)):
        return sorted(enc(x) for x in v)
    if isinstance(v, dict):
        return {str(k): enc(x) for k, x in v.items()}
    return v


class Rec:
    def __init__(self):
        self.td = os.path.realpath(tempfile.mkdtemp(prefix="state-parity-"))
        self.proj = os.path.join(self.td, "proj")
        self.steps = []
        self.now = NOW0
        self.env = {"plugin_env": ["@TMP@/plugin", None], "home": "@TMP@/home", "transcripts": "@TMP@/tr"}
        self.last = None

    def norm(self, s):
        return s.replace(self.td, "@TMP@").replace(str(ROOT), "@SRC@")

    def abs(self, v):
        if isinstance(v, str):
            return v.replace("@TMP@", self.td).replace("@SRC@", str(ROOT))
        if isinstance(v, (list, tuple)):
            return [self.abs(x) for x in v]
        if isinstance(v, dict):
            return {k: self.abs(x) for k, x in v.items()}
        return v

    def p(self, rel):
        return os.path.join(self.td, rel)

    # ---- 檔案操作（路徑相對於暫存目錄）
    def copytree(self, src, dst):
        self.steps.append({"op": "copytree", "from": src, "to": dst})
        shutil.copytree(ROOT / src, self.p(dst), ignore=shutil.ignore_patterns("__pycache__"))

    def write(self, rel, text="", raw=None):
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

    def write_json(self, rel, obj, indent=None):
        self.write(rel, json.dumps(obj, ensure_ascii=False, indent=indent) + ("\n" if indent else ""))

    def copy(self, src, rel):
        """把模版 repo 的一個檔複製進來（內容不放進基準）。"""
        self.steps.append({"op": "copy", "from": src, "to": rel})
        os.makedirs(os.path.dirname(self.p(rel)), exist_ok=True)
        shutil.copyfile(ROOT / src, self.p(rel))

    def rm(self, rel):
        self.steps.append({"op": "rm", "path": rel})
        if os.path.isdir(self.p(rel)):
            shutil.rmtree(self.p(rel))
        elif os.path.exists(self.p(rel)):
            os.unlink(self.p(rel))

    def utime(self, rel, offset=0.0):
        """修改時間設成「現在」加 offset 秒。"""
        t = self.now + offset
        self.steps.append({"op": "utime", "path": rel, "mtime": t})
        os.utime(self.p(rel), (t, t))

    def snapshot(self):
        out = {}
        for root in SNAP_ROOTS:
            for base, dirs, files in os.walk(self.p(root)):
                rel_base = os.path.relpath(base, self.td).replace(os.sep, "/")
                dirs[:] = sorted(d for d in dirs if f"{rel_base}/{d}" not in SNAP_SKIP and d != "__pycache__")
                for f in sorted(files):
                    full = os.path.join(base, f)
                    text = self.norm(Path(full).read_bytes().decode("utf-8", errors="replace"))
                    out[f"{rel_base}/{f}"] = text if len(text) <= LONG else {"sha1": hashlib.sha1(text.encode("utf-8")).hexdigest(), "chars": len(text)}
        return out

    def changed(self, before):
        after = self.snapshot()
        return {k: after.get(k) for k in sorted(set(before) | set(after)) if before.get(k) != after.get(k)}

    # ---- 指令
    def aw(self, *argv, note="", label=None, tick=1.0):
        """跑一次 awf.py（在行程內，專案＝proj）。回傳 exit code；stdout 留在 self.last。"""
        before = self.snapshot()
        self.now += tick
        real = [self.abs(a) for a in argv]
        e = self.abs(self.env)
        for k, v in (("AWF_PLUGIN_DIR", e["plugin_env"][0]), ("CLAUDE_PLUGIN_ROOT", e["plugin_env"][1]), ("AWF_TRANSCRIPTS_DIR", e["transcripts"])):
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        step = {"op": "run", "argv": list(argv), "env": json.loads(json.dumps(self.env)), "now": self.now, "note": note}
        if label:
            step["label"] = label
        out, err = io.StringIO(), io.StringIO()
        clock(self.now)
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), fixed_today():
            try:
                code = awf.main(real, Path(self.proj), Path(e["home"]))
            except Exception as ex:  # noqa: BLE001  awf.py 沒接住的例外：命令列會印 traceback、exit 1
                code = 1
                step["raises"] = type(ex).__name__
        stdout = self.norm(out.getvalue())
        if stdout.startswith(awf.__doc__[:30]):
            step["usage"] = True
            stdout = ""
        step.update({"code": code, "stdout": stdout, "stderr": "" if step.get("raises") else self.norm(err.getvalue()), "files": self.changed(before)})
        self.steps.append(step)
        self.last = stdout
        return code

    def act(self):
        """上一個指令印的動作（JSON）。"""
        t = self.last or ""
        try:
            return json.loads(t[t.index("{"):])
        except ValueError:
            return {}

    def call(self, fn, note="", label=None, **kw):
        before = self.snapshot()
        kw = json.loads(self.norm(json.dumps(kw, ensure_ascii=False)))
        e = self.abs(self.env)
        os.environ["AWF_TRANSCRIPTS_DIR"] = e["transcripts"]
        clock(self.now)
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()), fixed_today():
                res = {"ok": enc(CALLS[fn](self, self.abs(kw)))}
        except awf.AwfError as ex:
            res = {"awf": str(ex)}
        except Exception as ex:  # noqa: BLE001
            res = {"raises": type(ex).__name__}
        res = json.loads(self.norm(json.dumps(res, ensure_ascii=False)))
        step = {"op": "call", "fn": fn, "args": kw, "env": json.loads(json.dumps(self.env)), "now": self.now, "note": note, "result": res, "files": self.changed(before)}
        if label:
            step["label"] = label
        self.steps.append(step)
        return res.get("ok")

    # ---- 劇本的積木
    def run_json(self, d):
        return json.loads(Path(self.proj, d, "run.json").read_text(encoding="utf-8"))

    def put_run(self, d, data):
        self.write(f"proj/{d}/run.json", json.dumps(data, ensure_ascii=False, indent=2) + "\n")

    def edit(self, d, fn):
        data = self.run_json(d)
        fn(data)
        self.put_run(d, data)

    def start(self, d, *extra, event="sdlc", note=""):
        base = ["--repo", "repos/app", "--epic", "demo", "--requirement", REQ] if event == "sdlc" else []
        return self.aw("start", "--run", d, "--event", event, *base, *extra, note=note)

    def lock(self, d, recipe, mods=(), locked=True, upto=None, approve=True):
        """把 run.json 改成「recipe 已定、upto 以前的段都做完（並核准）」。"""
        res = rcp.resolve(DEFAULT, recipe, list(mods))
        data = self.run_json(d)
        data["recipe"] = {"name": recipe, "modifiers": list(mods), "size": "", "resolved": res, "locked": locked, "history": []}
        save_run.rebuild_stages(data, res)
        for st in res["stages"]:
            if st == upto:
                break
            data["stages"][st].update({"status": "done", "envelopes": 1})
            if approve and data["stages"][st].get("gate"):
                data["stages"][st]["approved"] = {"by": "owner", "at": "t", "kind": "human"}
            self.write_json(f"proj/{d}/{st}.json", {"stage": st, "review": env(), "subtasks": [{"id": "t1", "title": "x", "owns": ["a.py"]}],
                                                    "recipe": {"name": recipe, "modifiers": list(mods), "stages": res["stages"]}})
        self.put_run(d, data)
        return res

    def wf(self, name, script, result, offset=0.0, sess="sess-1", status="completed", run_id=True):
        """一筆 workflow 紀錄（<session>/workflows/<name>.json），修改時間＝現在＋offset。"""
        meta = {"scriptPath": script, "status": status, "result": result}
        if run_id:
            meta["runId"] = name
        rel = f"tr/{sess}/workflows/{name}.json"
        self.write_json(rel, meta)
        self.utime(rel, offset)

    def session(self, sess, offset=0.0):
        self.write(f"tr/{sess}.jsonl", "{}\n")
        self.utime(f"tr/{sess}.jsonl", offset)

    def script_of(self, d, wfname):
        return f"@TMP@/proj/{d}/.wf/{wfname}.js"


def clock(now):
    """awf.py 的時鐘固定在 now。"""
    class Fixed(datetime.datetime):
        @classmethod
        def now(cls, tz=None):
            return datetime.datetime.fromtimestamp(now, tz)
    awf.datetime = types.SimpleNamespace(datetime=Fixed, timezone=datetime.timezone)


def _find_template(w, a):
    text, source, notes = awf.find_template(a["name"], Path(w.proj), Path(a["plugin"]) if a.get("plugin") else None)
    return [hashlib.sha1(text.encode("utf-8")).hexdigest(), source, notes]


def _find_wf_meta(w, a):
    meta, f = awf.find_wf_meta(Path(w.proj), a["script"], a.get("since") or 0.0)
    return [meta.get("runId") if meta else None, f]


def _stage_args(w, a):
    run = Path(w.proj, a["run"])
    data = a["data"] if "data" in a else save_run.load(run)
    return awf.stage_args(run, a["run"], data, a["stage"], a["workflow"], Path(w.proj))


CALLS = {
    "read_config": lambda w, a: awf.read_config(Path(w.proj)),
    "plugin_candidates": lambda w, a: [{**c, "path": str(c["path"])} for c in awf.plugin_candidates(Path(w.proj), Path(a["home"]))],
    "find_plugin": lambda w, a: awf.find_plugin(Path(w.proj), a.get("explicit") or "", Path(a["home"])),
    "event_route": lambda w, a: awf.event_route(Path(w.proj), a["event"]),
    "event_required": lambda w, a: awf.event_required(Path(w.proj), a["event"]),
    "find_template": _find_template,
    "find_wf_meta": _find_wf_meta,
    "current_session": lambda w, a: awf.current_session(Path(w.proj)),
    "stage_args": _stage_args,
    "provisional": lambda w, a: awf.provisional(a["res"]),
    "envelopes": lambda w, a: [x[0] for x in awf._envelopes(a["res"])],
    "text_of": lambda w, a: awf.text_of(a["act"]),
    "custom_roles": lambda w, a: awf.custom_roles(Path(w.proj)),
    "workflow_names": lambda w, a: awf.workflow_names(Path(w.proj), Path(a["plugin"]) if a.get("plugin") else None),
}


# ------------------------------------------------------------------ 劇本
def fixture(w):
    for src, dst in COPIES:
        w.copytree(src, dst)
    w.write_json("plugin/.claude-plugin/plugin.json", {"name": "agent-workflow", "version": "9.9.9"})
    w.write("proj/.claude/teams/workflow.config", CONFIG)
    w.copy("scaffold/.claude/teams/methodologies/default.json", "proj/.claude/teams/methodology.json")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}})
    w.write("proj/.claude/teams/VERSION", "9.9.9\n")
    w.write("proj/README.md", "# 示範客戶 ACME 的報表系統\n\n## 匯出\n\n報表可以匯出 CSV，欄位順序照畫面。\n")
    w.write("proj/repos/app/README.md", "# app\n\n報表匯出的程式在 src/export.py。\n")
    w.write("proj/repos/app/src/export.py", "def export_csv(rows):\n    return '\\n'.join(','.join(r) for r in rows)\n")
    w.write("proj/repos/app/specs/demo/spec.md", "# spec\n\n## 匯出\n\n- AC-1 匯出報表要有 CSV\n")
    w.write("home/.keep", "")
    w.session("sess-1", -100)


def scenario_cli(w):
    w.aw(note="沒有參數：印用法、exit 2")
    w.aw("-h", note="-h：印用法、exit 0")
    w.aw("--help")
    w.aw("nope", "--run", "_runs/x", note="不認得的指令（run.json 也不存在：先回找不到）")
    w.aw("next", note="缺 --run")
    w.aw("next", "--run", "", note="--run 給空字串")
    w.aw("next", "--run", "_runs/nope", note="找不到 run.json")
    w.aw("start", "--run", "_runs/c1", note="start 缺 --event")
    w.start("_runs/c1")
    w.start("_runs/c1", note="start 兩次：save_run init 回已存在，args 照樣更新")
    w.aw("nope", "--run", "_runs/c1", note="不認得的指令")
    w.aw("start", "--run", "_runs/c2/", "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", REQ, "--base", "main", "--recipe", "bugfix",
         "--modifiers", "ui,security", "--size", "s", "--set", "n=12", "--set", "neg=-3", "--set", "yes=true", "--set", "no=false", "--set", "s=a=b",
         "--set-json", 'obj={"a":[1,2.5,null]}', note="start 帶全部選項；--run 結尾的斜線去掉")
    w.aw("start", "--run", "_runs/c3", "--event", "sdlc", "--set", "沒有等號", "--set", "run_dir=/x", "--set", "Bad=1", "--set-json", "x={壞", "--set-json", "run_dir=1",
         "--set-json", "noeq", note="--set／--set-json 寫錯：全部列出，不寫 args")
    w.aw("start", "--run", "@TMP@/proj/_runs/c4", "--event", "meeting", note="--run 給絕對路徑")
    w.aw("next", "--run", "_runs/c1", "--set", "big=123456789012345678901234567890", label="set-huge-int", note="--set 給超出 64 位元的整數")
    w.aw("next", "--run", "_runs/c1", "--set-json", "deep=" + "[" * 3000 + "]" * 3000, label="set-deep", note="--set-json 巢狀太深")
    w.aw("next", "--run", "_runs/c1", "--set-json", "x=NaN", label="set-nan", note="--set-json 給 NaN")


def scenario_plugin(w):
    d = "_runs/p1"
    w.start(d)
    saved = json.loads(json.dumps(w.env))
    w.env["plugin_env"] = [None, None]
    w.aw("next", "--run", d, note="找不到 plugin（環境變數沒有、登記檔沒有）")
    w.aw("render", "--all", "--out", "@TMP@/out/none", note="render --all：沒有 plugin 也沒有覆寫檔")
    w.aw("next", "--run", d, "--plugin-dir", "@TMP@/nope", note="--plugin-dir 底下沒有 workflow-templates")
    w.aw("next", "--run", d, "--plugin-dir", "@TMP@/plugin", note="--plugin-dir 指定")
    w.env["plugin_env"] = ["@TMP@/nope", "@TMP@/plugin"]
    w.aw("next", "--run", d, note="AWF_PLUGIN_DIR 不對、CLAUDE_PLUGIN_ROOT 對")
    w.env["plugin_env"] = [None, None]
    # 舊副本（沒有 plugin 時退回用）
    w.write("proj/.claude/workflows/sdlc-plan.js", "export const meta = { name: 'sdlc-plan', description: '舊副本' }\nconst A = args || {}\nconst MP = /*@methodology*/ null /*@end*/\nreturn { stage: 'plan' }\n")
    w.aw("next", "--run", d, note="找不到 plugin、專案有沒標記的舊副本：退回用它（legacy），結果不能政策核准")
    w.call("find_template", name="sdlc-plan", plugin=None)
    w.call("find_template", name="sdlc-plan", plugin="@TMP@/plugin", note="plugin 在：舊副本不採用並說明")
    w.call("find_template", name="Bad Name", plugin="@TMP@/plugin")
    w.call("find_template", name="nope", plugin="@TMP@/plugin", note="plugin 沒有這支")
    w.call("find_template", name="nope", plugin=None)
    w.write("proj/.claude/workflows/event-meeting.js", "// 0.4 的舊副本\nconst A = args || {}\n")
    w.call("find_template", name="event-meeting", plugin="@TMP@/plugin", note="plugin 這版沒有、專案有舊副本：不採用")
    w.rm("proj/.claude/workflows/event-meeting.js")
    # 登記檔
    w.write("other/workflow-templates/x.js", "//\n")
    reg = {"version": 2, "plugins": {
        "agent-workflow@a": [{"installPath": "@TMP@/plugin", "version": "9.9.9", "lastUpdated": "2026-01-01"}],
        "agent-workflow@b": [{"installPath": "@TMP@/other", "version": "1.0.0", "lastUpdated": "2026-06-01"}],
        "agent-workflow@c": [{"installPath": "@TMP@/other", "version": "9.9.9", "projectPath": "@TMP@/elsewhere"}],
        "agent-workflow@d": {"installPath": "@TMP@/nope"},
        "something-else@a": [{"installPath": "@TMP@/other"}]}}
    w.write_json("home/plugins/installed_plugins.json", reg)
    w.call("plugin_candidates", home="@TMP@/home")
    w.call("plugin_candidates", home="@TMP@/nope", note="登記檔不存在")
    w.call("find_plugin", home="@TMP@/home", note="版本和專案相同的優先；多個來源會提醒")
    w.call("find_plugin", home="@TMP@/home", explicit="@TMP@/other")
    w.call("find_plugin", home="@TMP@/home", explicit="@TMP@/nope")
    w.aw("next", "--run", d, note="從登記檔找到 plugin，說明裝了不只一個來源；舊副本不採用")
    reg["plugins"]["agent-workflow@e"] = [{"installPath": "@TMP@/other", "version": "0.1.0", "projectPath": "@TMP@/proj"}]
    w.write_json("home/plugins/installed_plugins.json", reg)
    w.call("find_plugin", home="@TMP@/home", note="裝在這個專案的優先")
    w.write("home/plugins/installed_plugins.json", "{壞掉")
    w.call("plugin_candidates", home="@TMP@/home", note="登記檔壞掉")
    w.write_json("home/plugins/installed_plugins.json", {"plugins": []})
    w.call("plugin_candidates", home="@TMP@/home")
    w.rm("home/plugins/installed_plugins.json")
    w.env = saved
    # 覆寫檔
    body = Path(ROOT, "plugin/workflow-templates/sdlc-plan.js").read_text(encoding="utf-8")
    w.write("proj/.claude/workflows/sdlc-plan.js", body + "\n// awf-override: 9.9.9\n")
    w.aw("next", "--run", d, note="覆寫檔（版本相同）：動作帶 override、記進 nonstandard")
    w.write("proj/.claude/workflows/sdlc-plan.js", body + "\n// awf-override: 0.1.0\n")
    w.aw("next", "--run", d, note="覆寫檔版本和 plugin 不同：警告")
    w.call("workflow_names", plugin="@TMP@/plugin")
    w.write("proj/.claude/workflows/mine.js", "export const meta = { name: 'mine', description: 'x' }\nreturn { ok: '{{OUTPUT_LANGUAGE}}' }\n// awf-override: 9.9.9  \n")
    w.call("workflow_names", plugin=None)
    w.aw("render", "mine", "--out", "@TMP@/out/mine", note="render 單一支（覆寫檔，沒有 methodology 標記）")
    w.write("proj/.claude/workflows/sdlc-plan.js", "export const meta = { name: 'sdlc-plan', description: 'x' }\nreturn {}\n// awf-override: 9.9.9\n")
    w.aw("next", "--run", d, note="覆寫檔沒有 args 那一行：沒辦法內嵌參數")
    w.rm("proj/.claude/workflows")
    w.aw("next", "--run", d, note="回到 plugin 的範本；nonstandard 的標記還在")


def scenario_render(w):
    w.aw("render", "--all", "--out", "@TMP@/out/all", note="render --all：12 支逐檔比雜湊")
    w.aw("render", "sdlc-plan", "--out", "out/rel", note="--out 給相對路徑（相對於目前目錄＝proj）")
    w.aw("render", "--all", note="缺 --out")
    w.aw("render", "--out", "@TMP@/out/x", note="沒給名字")
    w.aw("render", "nope", "--out", "@TMP@/out/x")
    w.aw("render", "Bad", "--out", "@TMP@/out/x")
    w.write("proj/.claude/teams/workflow.config", "AGENT_NS=agent-workflow:\n# 註解\nlower=1\nOUTPUT_LANGUAGE=\"\n")
    w.call("read_config")
    w.aw("render", "sdlc-plan", "--out", "@TMP@/out/x", note="workflow.config 缺值：拒絕")
    w.write("proj/.claude/teams/workflow.config", "﻿" + CONFIG.replace("\n", "\r\n") + 'EMPTY=""\nQ="a"b"\nEQ=a=b\n')
    w.call("read_config", note="BOM、CRLF、引號")
    w.rm("proj/.claude/teams/workflow.config")
    w.call("read_config", note="沒有 workflow.config")
    w.write("proj/.claude/teams/workflow.config", CONFIG)
    w.write("proj/.claude/teams/VERSION", "0.4.1\n")
    w.aw("render", "sdlc-test", "--out", "@TMP@/out/ver", note="plugin 與專案工具版本不同：提醒")
    w.write("proj/.claude/teams/VERSION", "9.9.9\n")
    w.write("proj/.claude/teams/methodology.json", "[1]")
    w.aw("render", "sdlc-test", "--out", "@TMP@/out/x", note="methodology.json 不是物件")
    w.write("proj/.claude/teams/methodology.json", "{壞")
    w.aw("render", "sdlc-test", "--out", "@TMP@/out/x", note="methodology.json 不是 JSON")
    w.write_json("proj/.claude/teams/methodology.json", {**DEFAULT, "extensions": [{"name": "x", "agent": "acme:a11y", "stage": "test", "instructions": "i"}, {"name": "y", "agent": "acme:b", "stage": "test", "instructions": "i"}]})
    w.aw("render", "sdlc-test", "--out", "@TMP@/out/x", note="擴充 agent 沒登記")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}, "extension_agents": {"acme:a11y": "tpm-reviewer", "acme:b": "tpm-reviewer"}})
    w.aw("render", "sdlc-test", "--out", "@TMP@/out/ext", note="擴充 agent 都登記了")
    w.call("custom_roles")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}})
    w.rm("proj/.claude/teams/methodology.json")
    w.aw("render", "sdlc-plan", "--out", "@TMP@/out/nometh", note="沒有 methodology.json：內嵌 null")
    w.copy("scaffold/.claude/teams/methodologies/default.json", "proj/.claude/teams/methodology.json")


def walk(w, name, recipe, mods=(), fail_at=""):
    """照 selftest 的走法：每個 recipe 從 start 走到 done（各段的結果直接寫檔）。"""
    d = f"_runs/{name}"
    w.start(d)
    failed = set()
    for _ in range(40):
        if w.aw("next", "--run", d):
            break
        a = w.act()
        if a["action"] == "run_workflow":
            s = a["stage"]
            data = w.run_json(d)
            if s == "plan":
                res = rcp.resolve(DEFAULT, recipe, list(mods))
                data["recipe"] = {"name": recipe, "modifiers": list(mods), "size": "", "resolved": res, "locked": False, "history": []}
                save_run.rebuild_stages(data, res)
            bad = fail_at == s and s not in failed
            failed.add(s)
            data["stages"][s].update({"status": "blocked" if bad else "done", "envelopes": 1})
            w.write_json(f"proj/{d}/{s}.json", {"stage": s, "halted": bad, "halt_reason": "缺件：x" if bad else None, "subtasks": [{"id": "t1"}] if s == "plan" else None, "merged": s == "merge"})
            w.put_run(d, data)
        elif a["action"] == "gate":
            w.edit(d, lambda data: data["stages"][a["stage"]].update({"approved": {"by": "human", "at": "t"}}))
        elif a["action"] == "need_input":
            w.aw("next", "--run", d, *[x for k in a["need"] for x in ("--set", f"{k}=true")])
            w.edit(d, lambda data: data.get("awf", {}).pop("pending", None))
        elif a["action"] == "manual":
            w.aw("next", "--run", d, "--set", "pr=7")
            w.edit(d, lambda data: data.get("awf", {}).pop("pending", None))
        elif a["action"] == "halted":
            w.aw("next", "--run", d, "--retry")
            w.aw("next", "--run", d, "--retry", "--text")

            def fix(data):
                data["stages"][a["stage"]]["status"] = "done"
            w.edit(d, fix)
            w.write_json(f"proj/{d}/{a['stage']}.json", {"stage": a["stage"], "subtasks": [{"id": "t1"}], "merged": a["stage"] == "merge"})
        else:
            break
    return d


def scenario_walk(w):
    for r in sorted(DEFAULT.get("recipes") or {}):
        walk(w, f"w-{r}", r)
    walk(w, "w-bugfix-migration", "bugfix", ("migration",))
    d = walk(w, "w-bugfix-f", "bugfix", fail_at="build")
    w.aw("handoff", "--run", d, note="handoff：寫接手摘要")
    w.aw("handoff", "--run", d, note="handoff 重跑：內容相同")
    w.aw("handoff", "--run", d, "--print")
    w.aw("handoff", "--run", "_runs/nope")
    w.aw("next", "--run", d, "--text", note="每段都完成：done 的文字版")
    w.aw("next", "--run", d, "--stage", "nope", note="--stage 指定不存在的段")
    w.aw("next", "--run", "_runs/w-docs", "--stage", "build", note="recipe 沒有的段（skipped）")
    w.aw("next", "--run", d, "--stage", "build", "--workflow", "sdlc-test", note="--workflow 和段不合")
    w.aw("next", "--run", d, "--stage", "build", note="--stage 指定已完成的段：重跑")


def scenario_args(w):
    d = "_runs/args"
    w.aw("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", REQ, "--recipe", "bugfix", "--set", "base=main", "--modifiers", "ui,,security")
    w.aw("next", "--run", d, note="plan：帶 repo、epic、requirement、run_dir、recipe、--set 的值、資料包；modifiers 字串拆成清單")
    w.aw("next", "--run", d, "--text")
    res = rcp.resolve(DEFAULT, "feature", ["ui"])

    def setup(data):
        data["recipe"] = {"name": "feature", "modifiers": ["ui"], "size": "m", "resolved": res, "locked": True, "history": []}
        save_run.rebuild_stages(data, res)
        for s in ("plan", "design", "build", "sync"):
            data["stages"][s].update({"status": "done", "approved": {"by": "h", "at": "t"}})
    w.edit(d, setup)
    subs = [{"id": "t1", "title": "匯出 CSV", "kind": "code", "owns": ["src/export.py"], "depends_on": [], "acceptance": ["AC-1"], "spec_refs": ["匯出"]},
            {"id": "t2", "title": "文件", "kind": "doc", "owns": ["docs/x.md"], "depends_on": ["t1"], "acceptance": ["AC-1"], "wave": 2}]
    w.write_json(f"proj/{d}/design.json", {"design": {"status": "done", "summary": "s", "payload": {"epic": "demo", "subtasks": subs}}, "sub_plans": [{"id": "api"}]})
    w.write_json(f"proj/{d}/sync.json", {"head_branch": "merge/demo-transit", "branch": "feature/demo"})
    w.aw("next", "--run", d, "--set", "recipe=docs", note="test 缺 has_security（modifier ui 讓 has_ui=true）；recipe 以鎖定的為準")
    w.aw("next", "--run", d, "--set", "has_security=false", "--set", "branch=mine", "--set-json", 'sub_plans=[{"id":"evil"}]', note="test：sub_plans、branch 以前段結果為準（--set 蓋不掉）")
    w.call("stage_args", run=d, stage="build", workflow="sdlc-build", note="build 的 subtasks 取自 design")
    w.aw("next", "--run", d, "--stage", "build", "--set-json", 'subtasks=[{"id":"evil"}]', note="build：--set 的 subtasks 蓋不掉；子任務資料包")
    w.write_json(f"proj/{d}/build.json", {"halted": True, "wave_pause": True, "remaining_subtasks": [subs[1]], "integrated": ["t1"]})
    w.call("stage_args", run=d, stage="build", workflow="sdlc-build", note="build halted 後續跑（wave_pause）")
    w.aw("next", "--run", d, "--stage", "build", note="續跑 build：只帶 remaining_subtasks 與 integrated")
    w.write_json(f"proj/{d}/build.json", {"halted": True, "remaining_subtasks": [], "integrated": None})
    w.call("stage_args", run=d, stage="build", workflow="sdlc-build")
    w.call("stage_args", run=d, stage="merge", workflow="sdlc-merge", note="merge 不帶 branch、缺 pr")
    w.call("stage_args", run=d, stage="deliver", workflow="sdlc-deliver")
    w.call("stage_args", run=d, stage="design", workflow="sdlc-design-revise")
    w.aw("next", "--run", d, "--stage", "design", "--workflow", "sdlc-design-revise", note="退回 design：缺 decisions、issues")
    w.aw("next", "--run", d, "--stage", "design", "--workflow", "sdlc-design-revise", "--set-json", 'decisions=["照原型"]', "--set-json", 'issues=[{"target_id":"t1","reason":"r"}]')
    w.aw("next", "--run", d, "--stage", "test", "--workflow", "sdlc-design-revise", note="別段不能用 design-revise")
    w.aw("next", "--run", d, "--set-json", "f=1.5", note="args 裡有浮點數：Python 在 args_summary 丟 TypeError")
    w.call("stage_args", run=d, stage="plan", workflow="sdlc-plan", data={"args": [1], "stages": {}}, note="args 不是物件")
    w.call("stage_args", run=d, stage="x", workflow="other", data={"args": None, "target_repo": "r", "requirement": "q"}, note="不在 REQUIRED 的 workflow：只要 run_dir")
    w.call("stage_args", run=d, stage="test", workflow="sdlc-test", data={"args": {"modifiers": "security,x", "epic": "e", "repo": "r", "branch": ""}}, note="沒有 recipe：modifiers 字串")
    w.call("stage_args", run=d, stage="test", workflow="sdlc-test", data={"args": {"modifiers": 5, "epic": "e", "repo": "r"}}, note="modifiers 是數字")
    # merge：先推送
    d2 = "_runs/merge"
    w.start(d2)
    w.lock(d2, "bugfix", upto="merge")
    w.aw("next", "--run", d2, note="merge 前：manual（auto_merge 啟用）")
    w.aw("next", "--run", d2, "--text")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": False}})
    w.aw("next", "--run", d2, note="auto_merge 沒啟用：提醒由人合併")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": True})
    w.aw("next", "--run", d2, note="auto_merge 不是物件")
    w.write_json("proj/.claude/teams/guard.config.json", [1])
    w.aw("next", "--run", d2, note="guard.config.json 不是物件")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}})
    w.write_json(f"proj/{d2}/sync.json", {"head_branch": "merge/x"})
    w.aw("next", "--run", d2, note="sync 走過中轉分支：publish 指令帶中轉分支")
    w.aw("next", "--run", d2, "--set", "pr=12", note="補上 PR 號：跑 sdlc-merge")
    # chain
    d3 = "_runs/chain"
    w.start(d3)
    w.lock(d3, "feature", upto="build")
    w.edit(d3, lambda data: data["stages"]["design"].pop("approved"))
    w.aw("next", "--run", d3, "--stage", "build", note="前面的段沒核准：halted（chain）")
    w.aw("next", "--run", d3, "--stage", "build", "--text")


def scenario_gate(w):
    d = "_runs/gate"
    w.start(d)
    w.edit(d, lambda data: data["stages"]["plan"].update({"status": "done", "envelopes": 1, "missing_records": ["research:x"], "constraint_issues": ["c1"], "plan_check": ["p1"], "ghostwritten_records": ["g"]}))
    e = env(summary="調研完成 " + "長" * 300, open_questions=[
        {"question": "編碼用什麼？", "blocking": True, "options": [{"choice": "UTF-8", "cost": "無"}, "x"], "recommendation": "UTF-8", "default_applied": ""},
        {"question": "欄位順序？", "blocking": False, "options": [{"choice": "照原型", "cost": "無"}], "recommendation": "照原型", "default_applied": " 照原型 "},
        {"question": "欄位順序？", "blocking": False, "options": [], "recommendation": "照原型", "default_applied": "照原型"},
        {"question": "要不要加註解？", "blocking": False, "options": [], "recommendation": "不加", "default_applied": "None"},
        {"question": "a=b 要嗎？", "blocking": False, "options": None, "recommendation": "要", "default_applied": "要"},
        "不是物件"])
    w.write_json(f"proj/{d}/plan.json", {"stage": "plan", "missing": ["research:y"], "research": [e, env(open_questions=None)], "reused": ["specs-adr"], "trace": [{"label": "x", "env": e}],
                                         "recipe": {"x": e}, "angles": {"skipped": ["upstream"]}, "gate": False, "models": {}, "next": "", "located_by": {"specs-adr": "tool"}, "waves": 0})
    w.write(f"proj/{d}/plan/執行紀錄索引.md", "# 索引\n")
    w.aw("next", "--run", d, note="閘門：重點（agents、open_questions 排序、provisional、缺件、紀錄索引…）")
    w.aw("next", "--run", d, "--text")
    w.call("provisional", res={"a": env(open_questions=[{"question": ["x"], "default_applied": "y"}])}, note="question 不能雜湊")
    w.call("provisional", res={"a": env(open_questions=5)}, note="open_questions 不是清單")
    w.call("envelopes", res={"a": [env(), {"trace": env(), "recipe": {"b": env()}, "c": {"trace": [env()]}}], "trace": {"x": env()}})
    w.aw("approve", "--run", d, note="approve 缺 --by")
    w.aw("approve", "--run", d, "--by", "human", "--answer", "編碼用什麼？=UTF-8", "--accept-defaults", note="核准：暫定的做法記進決策記憶")
    w.aw("handoff", "--run", d, "--print", note="handoff：agent 提過的問題、人已經決定過的")
    w.aw("approve", "--run", d, "--by", "human", note="沒有在等核准的段")
    w.edit(d, lambda data: data["stages"]["design"].update({"status": "done", "envelopes": 0}))
    w.aw("approve", "--run", d, "--by", "human", note="核准仍由 save_run 把關（沒有信封）")
    w.aw("approve", "--run", d, "design", "--by", "design", "--note", "n", "--answer-scope", "project", "--answer-expires", "2027-01-01", "--answer", "q=a", note="指定段；選項的值和段同名時不當成段")
    w.aw("approve", "--run", d, "nope", "--by", "x")
    d2 = "_runs/gate2"
    w.start(d2)
    w.lock(d2, "feature", upto="test")
    w.edit(d2, lambda data: data["stages"]["test"].update({"status": "done", "envelopes": 1}))
    w.write_json(f"proj/{d2}/test.json", {"stage": "test", "verify": env()})
    w.aw("next", "--run", d2, note="test 的閘門（strict）")
    w.aw("approve", "--run", d2, "--by", "owner", "--note", "看過了", note="核准正在等的段")
    w.aw("next", "--run", d2, "--stage", "test", "--workflow", "sdlc-test", note="--workflow 指定時不停在閘門")


def gm_run(w, name, recipe, mods=(), env_extra=None):
    d = f"_runs/{name}"
    w.start(d)
    res = rcp.resolve(DEFAULT, recipe, list(mods))
    data = w.run_json(d)
    data["recipe"] = {"name": recipe, "modifiers": list(mods), "size": "", "resolved": res, "locked": True, "history": []}
    save_run.rebuild_stages(data, res)
    e = env(summary="審完", **(env_extra or {}))
    for st in res["stages"]:
        if st == "merge":
            break
        data["stages"][st].update({"status": "done", "envelopes": 1})
        trace = []
        for axis in (res.get("deliver_axes") or []) if st == "deliver" else []:
            rec = f"@TMP@/proj/{d}/{st}/review-{axis}.md"
            w.write(f"proj/{d}/{st}/review-{axis}.md", "# 紀錄\n")
            trace.append({"label": f"review:{axis}", "agent_type": NS + "tpm-reviewer", "phase": "Review", "record": rec, "ok": True})
        w.write_json(f"proj/{d}/{st}.json", {"stage": st, "review": e, "subtasks": [{"id": "t1"}], "trace": trace, "recipe": {"name": recipe, "modifiers": list(mods), "stages": res["stages"]}})
        if st == "plan" or (st != "deliver" and data["stages"][st].get("gate")):
            data["stages"][st]["approved"] = {"by": "human", "at": "t", "kind": "human"}
    w.put_run(d, data)
    return d


def scenario_policy(w):
    soft = {"open_questions": [{"question": "標題用哪個？", "blocking": False, "options": [], "recommendation": "沿用", "default_applied": "沿用舊標題"}]}
    d = gm_run(w, "gm-clean", "docs", env_extra=soft)
    w.aw("next", "--run", d, note="docs（plan-only）的 deliver 結果乾淨：政策核准、直接到下一步；carry 帶 auto_approved 與 unconfirmed")
    w.aw("next", "--run", d, "--text")
    w.aw("handoff", "--run", d, "--print", note="handoff：政策核准的段與還沒確認的暫定")
    w.aw("next", "--run", d, "--set", "pr=3")
    hard = {"open_questions": [{"question": "要不要改對外文件？", "blocking": True, "options": [{"choice": "改", "cost": "x"}], "recommendation": "改", "default_applied": "none"}]}
    d = gm_run(w, "gm-dirty", "docs", env_extra=hard)
    w.aw("next", "--run", d, note="結果不乾淨：policy_refused，交人")
    d = gm_run(w, "gm-mod", "docs", mods=("security",))
    w.aw("next", "--run", d, note="帶 modifier：strict")
    d = gm_run(w, "gm-strict", "bugfix")
    w.aw("next", "--run", d, note="strict 的 recipe")
    d = gm_run(w, "gm-testonly", "test-only")
    w.aw("next", "--run", d, note="test-only（plan-only）")
    d = "_runs/gm-plan"
    w.start(d, "--recipe", "docs")
    w.lock(d, "docs", locked=False, upto="plan")
    w.edit(d, lambda data: data["stages"]["plan"].update({"status": "done", "envelopes": 1}))
    w.write_json(f"proj/{d}/plan.json", {"stage": "plan", "intent": env(summary="s")})
    w.aw("next", "--run", d, note="plan 段永遠等人核准")
    d = gm_run(w, "gm-awf", "docs", env_extra=soft)
    w.edit(d, lambda data: data.update({"awf": {"auto_approved": [{"stage": "x"}], "nonstandard": {}}}))
    w.aw("next", "--run", d, "--set", "k=v", note="政策核准時保留這次 next 還沒存檔的 awf 狀態與 args")


def scenario_custom(w):
    def cs_run(name, upto, recipe="feature"):
        d = f"_runs/{name}"
        w.start(d, "--requirement", "分波打地基")
        w.lock(d, recipe, upto=upto)
        return d
    good = awf_script.GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'")
    w.write("proj/scripts/waves.js", good)
    w.write("proj/scripts/direct.js", good.replace("const done = []", "const done = [await agent('x', {})]"))
    w.write("proj/scripts/ext.js", awf_script.GOOD)
    w.write("proj/scripts/bad.js", b"".decode(), raw=b"export const meta = {}\n\xff\xfe\n")
    d = cs_run("custom", "build")
    w.aw("next", "--run", d, "--script", "scripts/waves.js", note="合格的自訂腳本：run_workflow、custom（來源與雜湊）")
    w.aw("next", "--run", d, "--script", "@TMP@/proj/scripts/waves.js", "--text", note="腳本給絕對路徑")
    for bad in ("direct.js", "ext.js", "nope.js", "bad.js"):
        d_bad = cs_run("custom-" + bad[:-3], "build")
        w.aw("next", "--run", d_bad, "--script", f"scripts/{bad}", note=f"{bad}：不執行")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}, "extension_agents": {"acme:a11y": "tpm-reviewer"}})
    d_ext = cs_run("custom-ext-ok", "build")
    w.aw("next", "--run", d_ext, "--script", "scripts/ext.js", note="登記過的擴充 agent 可以派")
    w.write_json("proj/.claude/teams/guard.config.json", {"auto_merge": {"enabled": True}})
    for stg in ("plan", "test", "merge", "design", "deliver"):
        d_no = cs_run("custom-st-" + stg, stg)
        w.aw("next", "--run", d_no, "--script", "scripts/waves.js", note=f"{stg} 段" + ("不開放" if stg in ("plan", "test", "merge") else "開放"))
    d_chain = cs_run("custom-chain", "build")
    w.edit(d_chain, lambda data: data["stages"]["design"].pop("approved"))
    w.aw("next", "--run", d_chain, "--script", "scripts/waves.js", "--stage", "build", note="不能跳段")
    w.aw("next", "--run", d_chain, "--script", "scripts/waves.js", "--stage", "nope")
    w.aw("next", "--run", d_chain, "--script", "scripts/waves.js", "--stage", "design", note="design 已有結果、在等核准")
    w.aw("next", "--run", d_chain, "--script", "scripts/waves.js", "--stage", "design", "--retry")
    w.aw("next", "--run", "_runs/w-docs", "--script", "scripts/waves.js", "--stage", "build", note="recipe 沒有的段")
    w.start("_runs/ev-cs", event="meeting")
    w.aw("next", "--run", "_runs/ev-cs", "--script", "scripts/waves.js", note="事件的 run 不能用")
    w.edit(d_chain, lambda data: data.update({"closed": {"by": "x"}}))
    w.aw("next", "--run", d_chain, "--script", "scripts/waves.js", note="已結束的 run")
    d_need = "_runs/custom-need"
    w.aw("start", "--run", d_need, "--event", "sdlc", "--requirement", "x")
    w.lock(d_need, "feature", upto="design")
    w.aw("next", "--run", d_need, "--script", "scripts/waves.js", note="缺參數：need_input")
    w.write_json("proj/.claude/teams/methodology.json", {**DEFAULT, "limits": {"max_agents": 7}})
    d_lim = cs_run("custom-limit", "build")
    w.aw("next", "--run", d_lim, "--script", "scripts/waves.js", note="limits.max_agents 帶進執行核心")
    w.copy("scaffold/.claude/teams/methodologies/default.json", "proj/.claude/teams/methodology.json")
    # 存檔：結果帶 custom_script；重跑要明講
    rec_dir = f"@TMP@/proj/{d}/build"
    res_rcp = {"name": "feature", "modifiers": [], "stages": rcp.resolve(DEFAULT, "feature", [])["stages"]}
    w.write_json("in/cs-notrace.json", {"stage": "build", "halted": False, "results": [env()], "recipe": res_rcp})
    w.aw("done", "--run", d, "--output", "@TMP@/in/cs-notrace.json", "--no-next", note="自訂腳本的回傳沒有 trace：紀錄缺件")
    w.aw("next", "--run", d, "--script", "scripts/waves.js", note="下一段是 sync：不開放")
    w.aw("next", "--run", d, "--script", "scripts/waves.js", "--stage", "build", "--retry")
    trace = []
    for label, role in (("Tests:t1", "dev-acceptance-tester"), ("Implement:t1", "dev-implementer"), ("Integrate", "dev-integrator")):
        f = label.replace(":", "-") + ".md"
        w.write(f"proj/{d}/build/{f}", "# 紀錄\n")
        trace.append({"label": label, "agent_type": NS + role, "phase": "Build", "record": f"{rec_dir}/{f}", "ok": True, "tier": "standard", "model": "sonnet"})
    w.write_json("in/cs-good.json", {"stage": "build", "halted": False, "results": [env()], "recipe": res_rcp, "trace": trace})
    w.aw("done", "--run", d, "--output", "@TMP@/in/cs-good.json", note="存下來的結果帶 custom_script 與 manual_output；印下一步")
    w.aw("next", "--run", d, "--stage", "build", note="閘門重點列出 custom_script")


def scenario_done(w):
    halted = {"stage": "plan", "halted": True, "halt_reason": "缺件：intent", "trace": [], "missing": ["intent"], "next": "補 intent 再重跑"}
    d = "_runs/done"
    w.start(d)
    w.aw("done", "--run", d, note="沒跑過 next 又沒指定段")
    w.aw("next", "--run", d)
    w.aw("done", "--run", d, note="找不到回傳")
    w.wf("wf_old", w.script_of(d, "sdlc-design"), {"stage": "design"})
    w.wf("wf_new", w.script_of(d, "sdlc-plan"), halted)
    w.aw("done", "--run", d, note="找到回傳：存檔、記 wf_run、清 pending、印下一步（halted）")
    w.aw("next", "--run", d, note="halted：原因、建議、retry")
    w.aw("next", "--run", d, "--text")
    w.session("sess-1", 0)
    w.call("current_session")
    w.aw("next", "--run", d, "--retry", note="同一個 session：帶 resume_from")
    w.session("sess-2", 50)
    w.call("current_session")
    w.aw("next", "--run", d, "--retry", note="換了 session：不帶，說明整段重跑")
    w.rm("tr/sess-2.jsonl")
    w.write_json("in/out.json", halted)
    w.aw("done", "--run", d, "--output", "@TMP@/in/out.json", "--stage", "plan", note="--output：多 manual_output 標記、wf_run 拿掉")
    w.aw("done", "--run", d, "--output", "@TMP@/in/nope.json", "--stage", "plan", note="--output 讀不到")
    w.write("in/bad.json", "{壞")
    w.aw("done", "--run", d, "--output", "@TMP@/in/bad.json", "--stage", "plan")
    w.write("in/null.json", "null")
    w.aw("done", "--run", d, "--output", "@TMP@/in/null.json", "--stage", "plan", note="--output 是 null")
    w.write("in/list.json", "[1, 2]")
    w.aw("done", "--run", d, "--output", "@TMP@/in/list.json", "--stage", "plan", "--no-next", note="--output 不是物件：包成 halted")
    w.write("in/nan.json", '{"stage": "plan", "x": NaN}')
    w.aw("done", "--run", d, "--output", "@TMP@/in/nan.json", "--stage", "plan", "--no-next", label="output-nan", note="--output 有 NaN")
    # 未來時間、舊的、兩筆
    d5 = "_runs/done5"
    w.start(d5)
    w.aw("next", "--run", d5)
    real = {"stage": "plan", "halted": True, "halt_reason": "真的", "trace": [], "missing": []}
    w.wf("wf_real5", w.script_of(d5, "sdlc-plan"), real)
    w.wf("wf_fake5", w.script_of(d5, "sdlc-plan"), {"stage": "plan", "halted": False, "trace": []}, offset=5000)
    w.wf("wf_stale5", w.script_of(d5, "sdlc-plan"), {"stage": "plan", "halted": False, "trace": []}, offset=-50)
    w.wf("wf_edge5", "_runs/other/.wf/sdlc-plan.js", real)
    w.call("find_wf_meta", script=w.script_of(d5, "sdlc-plan"), since=0, note="since=0：不看時間下限，取最新的一筆（未來的仍不採用）")
    w.call("find_wf_meta", script="", since=0)
    w.aw("done", "--run", d5, "--no-next", note="未來與 next 之前的紀錄不採用")
    d6 = "_runs/done6"
    w.start(d6)
    w.aw("next", "--run", d6)
    w.wf("wf_a6", w.script_of(d6, "sdlc-plan"), real, offset=0.2)
    w.wf("wf_b6", f"{d6}/.wf/sdlc-plan.js", real, offset=0.4)
    w.aw("done", "--run", d6, "--no-next", note="同一支腳本兩筆（一筆用相對路徑）：不猜，交人")
    w.rm("tr/sess-1/workflows/wf_a6.json")
    w.wf("wf_b6", f"{d6}/.wf/sdlc-plan.js", json.dumps(real, ensure_ascii=False), offset=0.4, run_id=False)
    w.aw("done", "--run", d6, note="result 是 JSON 字串、沒有 runId（用檔名）")
    for i, (res, note) in enumerate(((("還在跑"), "result 是一般字串"), (None, "result 是 null"), (7, "result 是數字"), ([env()], "result 是清單"))):
        dn = f"_runs/done-r{i}"
        w.start(dn)
        w.aw("next", "--run", dn)
        w.wf(f"wf_r{i}", w.script_of(dn, "sdlc-plan"), res, status="running" if res is None else "completed")
        w.aw("done", "--run", dn, "--no-next", note=note)
    # 覆寫檔跑的段：標記不會因為重跑 next 而掉；標準腳本的回傳取自紀錄後解除
    body = Path(ROOT, "plugin/workflow-templates/sdlc-plan.js").read_text(encoding="utf-8")
    w.write("proj/.claude/workflows/sdlc-plan.js", body + "\n// awf-override: 9.9.9\n")
    d7 = "_runs/done7"
    w.start(d7)
    w.aw("next", "--run", d7, note="用覆寫檔：override、nonstandard")
    w.wf("wf_ov7", w.script_of(d7, "sdlc-plan"), real)
    w.aw("done", "--run", d7, "--no-next", note="覆寫檔的回傳取自紀錄：帶 override_script、沒有 manual_output")
    w.rm("proj/.claude/workflows")
    w.aw("next", "--run", d7, "--retry", note="重跑 next：pending 被標準腳本蓋掉")
    w.aw("done", "--run", d7, "--output", "@TMP@/in/out.json", "--no-next", note="--output：覆寫檔的標記還在")
    w.aw("next", "--run", d7, "--retry")
    w.wf("wf_ok7", w.script_of(d7, "sdlc-plan"), real)
    w.aw("done", "--run", d7, note="標準腳本、回傳取自紀錄：標記解除")
    # 存檔輸出的先後（stderr 與 stdout 混在一起）：有 trace、代寫、限制文件、halted
    d8 = "_runs/done8"
    w.start(d8)
    w.lock(d8, "feature", upto="design")
    w.write_json("proj/.claude/teams/methodology.json", {**DEFAULT, "constraints": [{"id": "c-arch", "file": "docs/nope.md", "stages": ["design"]}]})
    w.aw("next", "--run", d8)
    recd = f"@TMP@/proj/{d8}/design"
    w.write(f"proj/{d8}/design/outline.md", "# 紀錄\n")
    tr = [{"label": "outline", "agent_type": NS + "dev-spec-author", "phase": "Design", "record": f"{recd}/outline.md", "ok": True, "tier": "strong", "model": "inherit"},
          {"label": "design", "agent_type": NS + "dev-spec-author", "phase": "Design", "record": f"{recd}/design.md", "ok": True, "tier": "strong", "model": "inherit"},
          {"label": "skeptic", "agent_type": NS + "tpm-skeptic", "phase": "Design", "record": f"{recd}/skeptic.md", "ok": False}]
    res8 = {"stage": "design", "halted": True, "halt_reason": "缺件：skeptic", "outline": env(), "design": env(summary="設計"), "skeptic": None, "trace": tr,
            "recipe": {"name": "feature", "modifiers": [], "stages": rcp.resolve(DEFAULT, "feature", [])["stages"]}}
    w.wf("wf_d8", w.script_of(d8, "sdlc-design"), res8)
    w.aw("done", "--run", d8, note="存檔輸出的先後：halted、限制文件 → 索引、代寫 → 缺件、錯誤 → 其餘")
    w.aw("next", "--run", d8, "--text")
    w.aw("handoff", "--run", d8, "--print", note="handoff：halted、紀錄缺件、限制文件")
    w.copy("scaffold/.claude/teams/methodologies/default.json", "proj/.claude/teams/methodology.json")
    # 事件的 done：正常完成
    d9 = "_runs/done9"
    w.start(d9, "--set", "source=meetings/a.md", event="meeting")
    w.aw("next", "--run", d9)
    w.wf("wf_m9", w.script_of(d9, "event"), {"stage": "meeting", "summary": env(summary="會議摘要")})
    w.aw("done", "--run", d9, note="事件的 done：完成，下一步是 done")
    w.aw("next", "--run", d9)
    # deliver：交付文件同步
    d10 = "_runs/done10"
    w.start(d10)
    w.lock(d10, "feature", upto="deliver")
    w.aw("next", "--run", d10)
    res10 = {"stage": "deliver", "review": env(), "deliverables": {"updated": [{"id": "dev-record", "sections": ["這次交付了什麼", 5]}, "x"]},
             "recipe": {"name": "feature", "modifiers": [], "stages": rcp.resolve(DEFAULT, "feature", [])["stages"]}}
    w.wf("wf_d10", w.script_of(d10, "sdlc-deliver"), res10)
    w.aw("done", "--run", d10, "--no-next", note="deliver 存檔沒過（沒有 trace）：不同步交付文件")
    w.write_json("proj/.claude/teams/deliverables.json", {"documents": [{"id": "dev-record"}]})
    w.write(f"proj/{d10}/deliver/交付摘要.md", "# 交付摘要\n\n## 這次交付了什麼\n\n匯出 CSV。\n")
    d11 = "_runs/done11"
    w.aw("start", "--run", d11, "--event", "deliver-ish")
    w.write(f"proj/{d11}/deliver/交付摘要.md", "# 交付摘要\n\n## 這次交付了什麼\n\n匯出 CSV。\n")
    w.write_json("in/deliver.json", {"stage": "deliver", "review": env(), "deliverables": {"updated": [{"id": "dev-record", "sections": ["這次交付了什麼"]}]}})
    w.aw("done", "--run", d11, "--stage", "deliver", "--output", "@TMP@/in/deliver.json", "--no-next", note="deliver 存檔通過：同步交付文件")
    w.write_json("in/deliver2.json", {"stage": "deliver", "review": env(), "deliverables": {"updated": [{"id": "dev-record", "sections": 5}]}})
    w.aw("done", "--run", d11, "--stage", "deliver", "--output", "@TMP@/in/deliver2.json", "--no-next", note="updated 的 sections 形狀不對：只警告")
    w.write("proj/.claude/teams/deliverables.json", "{壞掉")
    w.aw("done", "--run", d11, "--stage", "deliver", "--output", "@TMP@/in/deliver.json", "--no-next", note="交付設定壞掉：只說明")
    w.rm("proj/.claude/teams/deliverables.json")
    w.aw("done", "--run", d11, "--stage", "deliver", "--output", "@TMP@/in/deliver.json", note="沒有交付設定；下一步算不出來時的說明")


def scenario_events(w):
    need = {"meeting": ["source=meetings/a.md"], "decision": ["decision=用 CSV"], "demo-check": ["target=staging"], "reqs-change": ["change=加欄位"], "upstream": ["what=PR 12", "repo=repos/app"],
            "brief": ["brief=briefs/a.md"], "pr-review": ["target=12", "repo=repos/app"], "post-merge": ["repo=repos/app"], "merge": ["repo=repos/app", "pr=12"]}
    for ev, sets in need.items():
        d = f"_runs/ev-{ev}"
        w.start(d, event=ev)
        w.aw("next", "--run", d, note=f"事件 {ev}：缺必要參數")
        w.aw("next", "--run", d, *[x for s in sets for x in ("--set", s)], note=f"事件 {ev}：參數齊了")
        w.call("event_route", event=ev)
        w.call("event_required", event=ev)
    w.aw("next", "--run", "_runs/ev-meeting", "--workflow", "sdlc-merge", note="事件的 run 不能用 --workflow 換成別支")
    w.aw("next", "--run", "_runs/ev-meeting", "--workflow", "event", "--text")
    d = "_runs/ev-merge"
    w.edit(d, lambda data: data["stages"].update({"merge": {"status": "done", "gate": False}}))
    w.write_json(f"proj/{d}/merge.json", {"stage": "merge", "merged": True, "steward": env(summary="已合併")})
    w.aw("next", "--run", d, note="merge 事件完成：提示跑 event-post-merge")
    d = "_runs/ev-brief"
    w.edit(d, lambda data: data["stages"].update({"brief": {"status": "invalid", "gate": False}}))
    w.aw("next", "--run", d, note="事件 halted（invalid）")
    w.edit(d, lambda data: data["stages"].update({"brief": {"status": "needs_decision", "gate": False}}))
    w.aw("next", "--run", d)
    w.edit(d, lambda data: data["stages"].update({"brief": {"status": "blocked", "gate": False}}))
    w.aw("next", "--run", d)
    w.aw("next", "--run", d, "--retry")
    custom = {"args": ["incident", 5], "context": "事故：{incident}", "gather": {"angles": [{"key": "cause", "ask": "找原因"}]}, "challenge": {"ask": "反駁"}, "write": {"ask": "寫回顧"}}
    w.write_json("proj/.claude/teams/methodology.json", {**DEFAULT, "events": {"incident": custom, "merge": custom, "upstream": {**custom, "args": ["what"]}, "bad": 5, "noargs": {**custom, "args": None}}})
    for ev in ("incident", "merge", "upstream", "bad", "noargs", "meeting", "debate", ""):
        w.call("event_route", event=ev)
        w.call("event_required", event=ev)
    w.start("_runs/ev-inc", event="incident")
    w.aw("next", "--run", "_runs/ev-inc", note="自訂事件：缺它要的參數")
    w.aw("next", "--run", "_runs/ev-inc", "--set", "incident=TASK-1", note="自訂事件：跑 event，args 帶 event=<類型>")
    w.copy("scaffold/.claude/teams/methodologies/default.json", "proj/.claude/teams/methodology.json")
    w.start("_runs/ev3", event="debate")
    w.aw("next", "--run", "_runs/ev3", note="沒有對應 workflow 的事件：manual")
    w.aw("next", "--run", "_runs/ev3", "--text")
    w.put_run("_runs/ev4", {"stages": {}})
    w.aw("next", "--run", "_runs/ev4", note="run.json 沒有 event")
    d = "_runs/ev-meeting"
    w.edit(d, lambda data: data.update({"abandoned": {"by": "h", "at": "t", "reason": "取消"}}))
    w.aw("next", "--run", d, note="已取消的 run")
    w.aw("handoff", "--run", d, "--print")
    w.edit(d, lambda data: (data.pop("abandoned"), data.update({"closed": {"by": "h", "at": "t"}})))
    w.aw("next", "--run", d, note="已結案的 run")


def scenario_runjson(w):
    d = "_runs/odd"
    for text, note, label in (("{壞", "run.json 不是 JSON", None), ("[1]", "run.json 是清單", None), ("{}", "run.json 是空物件", None), ('{"event": "sdlc"}', "沒有 stages", None),
                              ('{"event": "sdlc", "stages": {"plan": 5}}', "段不是物件", None), ('{"event": "sdlc", "stages": {"plan": {"status": "pending"}}, "awf": 5}', "awf 不是物件", None),
                              ('{"event": "sdlc", "stages": {"plan": {"status": "pending"}}, "args": 5}', "args 不是物件", None),
                              ('{"event": "sdlc", "stages": {"plan": {"status": "pending"}}, "x": NaN}', "run.json 有 NaN", "runjson-nan")):
        w.write(f"proj/{d}/run.json", text)
        for cmd in (("next",), ("done", "--stage", "plan", "--output", "@TMP@/in/out.json", "--no-next"), ("handoff", "--print"), ("approve", "--by", "x")):
            w.aw(cmd[0], "--run", d, *cmd[1:], note=f"{note}：{cmd[0]}", label=label)


def scenario_misc(w):
    w.call("text_of", act={"action": "gate", "stage": "plan", "need": {"a": "b"}, "notes": ["n1"], "highlights": {"missing": ["x"], "missing_records": ["m"], "constraint_issues": ["c"],
                                                                                                        "open_questions": [{"question": "q", "blocking": 1}], "provisional": [{"question": "p", "applied": "a"}]}})
    w.call("text_of", act={"action": "done"})
    w.call("text_of", act={"action": "x", "highlights": {"provisional": [{}]}}, note="provisional 缺鍵")
    w.call("text_of", act={"action": "x", "need": [1]})
    p = subprocess.run([sys.executable, str(Path(w.proj, ".claude/teams/tools/awf.py")), "--selftest"], capture_output=True, text=True, encoding="utf-8", cwd=w.proj,
                       env={k: v for k, v in os.environ.items() if k not in ("AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT", "AWF_TRANSCRIPTS_DIR", "CLAUDE_PROJECT_DIR")})
    w.steps.append({"op": "selftest", "code": p.returncode, "stdout": w.norm(p.stdout), "note": "awf.py --selftest（在已 init 的專案裡跑的那一種）"})


def main():
    global awf, save_run, pack, rcp, awf_script, DEFAULT
    w = Rec()
    prev = os.getcwd()
    try:
        fixture(w)
        sys.path.insert(0, str(Path(w.proj, ".claude/teams/tools")))
        sys.dont_write_bytecode = True
        import awf as awf_mod
        import awf_doctor  # noqa: F401
        import awf_script as script_mod
        import awf_watch  # noqa: F401
        import deliverables  # noqa: F401
        import kb  # noqa: F401
        import locate  # noqa: F401
        import pack as pack_mod
        import recipe as rcp_mod
        import save_run as sr_mod
        awf, save_run, pack, rcp, awf_script = awf_mod, sr_mod, pack_mod, rcp_mod, script_mod
        pack.datetime = fake_datetime()
        DEFAULT = json.loads((TEAMS / "methodologies" / "default.json").read_text(encoding="utf-8"))
        os.chdir(w.proj)
        for sc in (scenario_cli, scenario_plugin, scenario_render, scenario_walk, scenario_args, scenario_gate, scenario_policy, scenario_custom, scenario_done, scenario_events,
                   scenario_runjson, scenario_misc):
            sc(w)
    finally:
        os.chdir(prev)
        shutil.rmtree(w.td, ignore_errors=True)
    one = lambda v: json.dumps(v, ensure_ascii=False)  # noqa: E731
    sys.stdout.write("{\n \"說明\": %s,\n \"today\": %s,\n \"long\": %d,\n \"snapshot\": %s,\n \"steps\": [\n%s\n ]\n}\n" % (
        one("狀態機（awf.py）的對跑基準，由 tests/parity/record_state.py 產生；不要手改。"), one("%04d-%02d-%02d" % TODAY), LONG,
        one({"roots": SNAP_ROOTS, "skip": SNAP_SKIP}), ",\n".join("  " + one(x) for x in w.steps)))


if __name__ == "__main__":
    main()
