#!/usr/bin/env python3
"""錄 watcher（awf_watch.py）的對跑基準。

用法：python3 tests/parity/record_watch.py > tests/parity/watch.expected.json

基準檔本身就是劇本：`steps` 依序是檔案操作（write、append、rm、mkdir、symlink、mtime）、建立一個 transcript 讀取器（tr）、
看一次（tick：快照、事件、警告、送出去的通知、progress.jsonl 與 .watch-state.json 的內容）、只能在程式內呼叫的函式（call）
與命令列（run：exit code、stdout、stderr、睡了幾次）。Rust 的測試（crates/awf-core/tests/watch_parity.rs）照同一份劇本做一遍，逐字比對。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`（一開始就取實體路徑，只有一種寫法）。
- 「現在」都是給的：tick 直接給 now 與 at；命令列把模組裡的 datetime 換成固定時間（Rust 用 `Env.now`），sleep 換成劇本
  （`sleeps`：每睡一次做一組檔案操作，做完了就等於按 Ctrl-C）。
- 狀態列看檔案的修改時間：用 mtime 步驟明確設定。
- 外部指令（notify）在 tick 與 call 是替身（`runner`：ok、exit:N、raise:類別）；命令列那幾步跑暫存目錄 `bin/` 裡的小腳本，
  只把收到的內容附加到 `bin/notify.log`，不連網。
- transcript 的根目錄每一步都明確給（不讀使用者家目錄的 ~/.claude）。
- 像憑證的字串不寫在這支腳本與基準檔裡：用 `{"@join": […]}`、`{"@rep": [字, 次數]}` 在執行時組出來。
- Python 沒接住的例外記類別名：Rust 要一樣 exit 1、stderr 提到同一個類別。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
import datetime
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
import awf_watch as aw  # noqa: E402
import save_run  # noqa: E402

NOTIFY_SH = """#!/bin/sh
# 假的通知指令：只把收到的內容記下來，不連網。
d="$(dirname "$0")"
cat >> "$d/notify.log"
echo >> "$d/notify.log"
if [ -f "$d/notify.fail" ]; then exit 3; fi
"""
RUN = "@TMP@/proj/_runs/20260101-sdlc-demo"
WF = "tr/sess-1/subagents/workflows/wf_a1"


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


def ts(sec):
    """2026-01-01T00:00:00Z 加幾秒（不跨日）。"""
    return "2026-01-01T%02d:%02d:%02d.000Z" % (sec // 3600, sec % 3600 // 60, sec % 60)


def when(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def line(sec=None, rid=None, tok=None, model="claude-sonnet-5-5", content=None, stamp=None, **extra):
    """transcript 的一行。"""
    e = {}
    if stamp is not None or sec is not None:
        e["timestamp"] = stamp if stamp is not None else ts(sec)
    if rid is not None:
        e["requestId"] = rid
    if tok is not None:
        e["message"] = {"model": model, "usage": {"input_tokens": tok, "output_tokens": 0}}
    elif content is not None:
        e["message"] = {"role": "user", "content": content}
    e.update(extra)
    return json.dumps(e) + "\n"


def runner_of(mode, sent):
    def run(argv, input="", **kw):  # noqa: A002
        sent.append([list(argv), json.loads(input)])
        if mode.startswith("raise:"):
            name = mode.split(":", 1)[1]
            if name == "TimeoutExpired":
                raise subprocess.TimeoutExpired(argv, 10)
            raise {"OSError": OSError, "FileNotFoundError": FileNotFoundError, "PermissionError": PermissionError, "ValueError": ValueError}[name]("x")
        return types.SimpleNamespace(returncode=int(mode.split(":", 1)[1]) if mode.startswith("exit:") else 0)
    return run


class Rec:
    def __init__(self):
        self.td = os.path.realpath(tempfile.mkdtemp(prefix="watch-parity-"))
        self.steps = []
        self.trs = {}

    def norm(self, v):
        if isinstance(v, str):
            return v.replace(self.td, "@TMP@")
        if isinstance(v, list):
            return [self.norm(x) for x in v]
        if isinstance(v, dict):
            return {k: self.norm(x) for k, x in v.items()}
        return v

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

    def read(self, path):
        try:
            return self.norm(Path(path).read_bytes().decode("utf-8", "replace"))
        except OSError:
            return None

    # ---------------------------------------------------------- 檔案
    def do(self, st):
        """做一個檔案操作（劇本的一步，或 sleeps 裡的一步）。"""
        f = Path(self.p(st["path"]))
        op = st["op"]
        if op in ("write", "append"):
            f.parent.mkdir(parents=True, exist_ok=True)
            data = bytes.fromhex(st["hex"]) if "hex" in st else self.abs(dec(st["text"])).encode("utf-8")
            with f.open("ab" if op == "append" else "wb") as fh:
                fh.write(data)
        elif op == "rm":
            f.unlink(missing_ok=True)
        elif op == "mkdir":
            f.mkdir(parents=True, exist_ok=True)
        elif op == "symlink":
            f.parent.mkdir(parents=True, exist_ok=True)
            os.symlink(self.abs(st["to"]), f)
        elif op == "mtime":
            os.utime(f, (st["secs"], st["secs"]))
        else:
            raise ValueError(op)

    def add(self, st):
        self.do(st)
        self.steps.append(st)

    def w(self, rel, text):
        self.add({"op": "write", "path": rel, "text": text})

    def wj(self, rel, obj):
        self.w(rel, json.dumps(obj, ensure_ascii=False))

    def wb(self, rel, data):
        self.add({"op": "write", "path": rel, "hex": data.hex()})

    def ap(self, rel, text):
        self.add({"op": "append", "path": rel, "text": text})

    def rm(self, rel):
        self.add({"op": "rm", "path": rel})

    def mkdir(self, rel):
        self.add({"op": "mkdir", "path": rel})

    def symlink(self, rel, to):
        self.add({"op": "symlink", "path": rel, "to": to})

    def mtime(self, rel, secs):
        self.add({"op": "mtime", "path": rel, "secs": secs})

    # ---------------------------------------------------------- watcher
    def tr(self, tid, root, run=RUN):
        self.trs[tid] = aw.Transcripts(Path(self.abs(root)) if root else None, Path(self.abs(run)))
        self.steps.append({"op": "tr", "id": tid, "root": root, "run": run})

    def tick(self, note, sec, run=RUN, tr=None, stall=600, cfg=None, runner="ok", label=None):
        sent = []
        st = {"op": "tick", "note": note, "run": run, "tr": tr, "stall": stall, "now": ts(sec), "at": ts(sec), "cfg": cfg, "runner": runner}
        rp = Path(self.abs(run))
        try:
            snap, ev, warns = aw.tick(rp, self.trs[tr] if tr else None, stall, cfg, now=when(ts(sec)), at=ts(sec), runner=runner_of(runner, sent))
            st["result"] = {"ok": self.norm({"snap": snap, "events": ev, "warns": warns, "text": aw.text_of(snap) if snap is not None else None, "sent": sent})}
        except Exception as e:  # noqa: BLE001
            st["result"] = {"raises": type(e).__name__}
        st["progress"] = self.read(rp / aw.PROGRESS)
        st["state"] = self.read(rp / aw.STATE)
        if label:
            st["label"] = label
        self.steps.append(st)
        return st

    def call(self, fn, note="", label=None, **args):
        a = self.abs(dec(args))
        try:
            res = {"ok": self.norm(CALLS[fn](self, a))}
        except Exception as e:  # noqa: BLE001
            res = {"raises": type(e).__name__}
        st = {"op": "call", "fn": fn, "args": args, "result": res}
        if note:
            st["note"] = note
        if label:
            st["label"] = label
        self.steps.append(st)
        return st

    def run(self, note, *argv, project="proj", transcripts="none", now=600, sleeps=(), label=None):
        """命令列：在行程內呼叫 main，時間與 sleep 換成固定的。"""
        st = {"op": "run", "note": note, "argv": list(argv), "project": project, "transcripts": transcripts, "now": ts(now), "sleeps": [list(x) for x in sleeps]}
        fixed = when(ts(now))

        class Fixed(datetime.datetime):
            @classmethod
            def now(cls, tz=None):
                return fixed

        pending = [list(x) for x in sleeps]
        slept = []

        def fake_sleep(n):
            slept.append(n)
            if not pending:
                raise KeyboardInterrupt
            for s in pending.pop(0):
                self.do(s)

        real_dt, real_time, real_profile = aw.datetime, aw.time, save_run.PROJECT_PROFILE
        aw.datetime = types.SimpleNamespace(datetime=Fixed, timezone=datetime.timezone, timedelta=datetime.timedelta)
        aw.time = types.SimpleNamespace(sleep=fake_sleep)
        save_run.PROJECT_PROFILE = Path(self.p(project)) / ".claude" / "teams" / "methodology.json"
        os.environ["AWF_TRANSCRIPTS_DIR"] = self.p(transcripts)
        out, err = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                st["code"] = aw.main(self.abs(list(argv)), Path(self.p(project)))
        except Exception as e:  # noqa: BLE001
            st["code"] = 1
            st["raises"] = type(e).__name__
        finally:
            aw.datetime, aw.time, save_run.PROJECT_PROFILE = real_dt, real_time, real_profile
        if "raises" not in st:
            st["stdout"] = self.norm(out.getvalue())
            st["stderr"] = self.norm(err.getvalue())
        st["slept"] = slept
        if label:
            st["label"] = label
        self.steps.append(st)
        return st

    def read_step(self, rel):
        self.steps.append({"op": "read", "path": rel, "text": self.read(self.p(rel))})


def _notify(r, a):
    sent = []
    ret = aw.notify(a["cfg"], a["event"], runner_of(a.get("runner") or "ok", sent))
    return {"ret": ret, "sent": sent}


def _diff(r, a):
    ev, mem = aw.diff_events(a["prev"], a["cur"], a["stall"])
    return {"events": ev, "mem": mem}


CALLS = {
    "diff_events": _diff,
    "notify_config": lambda r, a: aw.notify_config(a["profile"]),
    "notify_payload": lambda r, a: aw.notify_payload(a["event"]),
    "notify": _notify,
    "text_of": lambda r, a: aw.text_of(a["snap"]),
    "run_view": lambda r, a: aw.run_view(Path(a["run"])),
    "snapshot": lambda r, a: aw.snapshot(Path(a["run"]), r.trs[a["tr"]] if a.get("tr") else None, when(a["now"])),
    "status_line": lambda r, a: aw.status_line(Path(a["project"])),
}


def P(**kw):
    return {"status": "pending", "gate": True, **kw}


def write_run(r, stages, run="proj/_runs/20260101-sdlc-demo", **extra):
    r.wj(run + "/run.json", {"event": "sdlc", "requirement": "機密需求內容", "stages": stages, **extra})


def wf_meta(r, status, root="tr", wf="wf_a1", run=RUN, **extra):
    r.wj(f"{root}/sess-1/workflows/{wf}.json", {"runId": wf, "scriptPath": run + "/.wf/sdlc-plan.js", "status": status, "workflowName": "sdlc-plan", **extra})


def agent(r, aid, role, label, wf=WF, run=RUN, **meta):
    r.w(f"{wf}/agent-{aid}.jsonl", line(0, content="run 目錄：" + run))
    r.wj(f"{wf}/agent-{aid}.meta.json", {"agentType": f"agent-workflow:{role}", "description": label, "workflowPhase": "Research", **meta})


# ------------------------------------------------------------------ 劇本
def scenario_flow(r):
    """和 selftest 同一條線：每種事件各出現一次以上。"""
    write_run(r, {"plan": P(), "build": P(gate=False)}, awf={"pending": {"stage": "plan", "workflow": "sdlc-plan"}})
    r.mkdir("tr/sess-1/subagents/workflows/wf_a1")
    r.mkdir("tr/sess-1/workflows")
    r.tr("main", "@TMP@/tr")
    r.tick("run 開始、plan 段開跑", 1, tr="main")
    r.tick("沒有變化就不重複記", 2, tr="main")
    wf_meta(r, "running")
    agent(r, "r1", "tpm-researcher", "research:code")
    r.ap(WF + "/agent-r1.jsonl", line(10, "q1", 30_000))
    r.tick("agent_started", 20, tr="main")
    r.ap(WF + "/agent-r1.jsonl", line(30, "q2", 40_000))
    r.tick("token 增加不到門檻", 40, tr="main")
    r.ap(WF + "/agent-r1.jsonl", line(50, "q3", 60_000) + line(50, "q3", 60_000))
    r.tick("agent_progress（同一個 requestId 不重複算）", 60, tr="main")
    r.w("proj/_runs/20260101-sdlc-demo/plan/research-code.md", "# 紀錄\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/" + save_run.INDEX_NAME, "# 索引\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/pack.md", "# 資料包\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/.md", "隱藏檔也算（glob 不分）\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/notes.txt", "不是紀錄\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/dir.md/x", "目錄不算\n")
    r.w("proj/_runs/20260101-sdlc-demo/plan/大寫.MD", "副檔名大小寫不同不算\n")
    r.tick("record_written（索引與資料包不算）", 70, tr="main")
    r.tick("停住：還沒超過門檻", 50 + 599, tr="main")
    r.tick("停住：stall 是 0 就不判", 50 + 601, tr="main", stall=0)
    r.tick("workflow_stalled", 50 + 601, tr="main")
    r.tick("停住只記一次", 50 + 700, tr="main")
    r.ap(WF + "/agent-r1.jsonl", line(900, "q4", 1_000))
    r.tick("恢復動靜", 905, tr="main")
    r.tick("又停住，再記一次", 900 + 601, tr="main")
    # 第二個 agent：meta 缺欄位、模型只有 meta 有、label 超過 120 字、agentType 沒有冒號、description 不是字串
    r.w(WF + "/agent-w2.jsonl", line(1500, content="寫文件") + line(1501, tok=None, message={"id": "m1", "usage": {"input_tokens": 5, "cache_creation_input_tokens": 7, "cache_read_input_tokens": 11, "output_tokens": 13}}))
    r.wj(WF + "/agent-w2.meta.json", {"agentType": "tpm-writer", "description": "長" * 130, "model": "claude-opus-5-0"})
    r.w(WF + "/agent-n3.jsonl", line(1502, 1, 100) + line(1502, True, 100) + line(1503, 2.5, 100, model=None) + "還沒寫完的一行")
    r.wj(WF + "/agent-n3.meta.json", {"agentType": ["x"], "description": 42, "workflowPhase": None})
    r.w(WF + "/agent-.jsonl", line(stamp="不是時間", tok=3))
    r.w(WF + "/agent-a.b.jsonl", "")
    r.w(WF + "/agentx.jsonl", line(1, "zz", 999_999))
    r.w(WF + "/agent-z.JSONL", line(1, "zz", 999_999))
    r.mkdir(WF + "/agent-dir.jsonl")
    r.tick("更多 agent：meta 缺欄位、id 是空的、目錄、名字不合的檔", 1510, tr="main")
    r.ap(WF + "/agent-n3.jsonl", "\n")
    r.tick("沒寫完的那一行補完了（不是 JSON，略過）", 1511, tr="main")
    wf_meta(r, "completed")
    write_run(r, {"plan": P(status="done"), "build": P(gate=False)})
    r.wj("proj/_runs/20260101-sdlc-demo/plan.json", {"trace": [{"ok": True}, {"ok": False}, {"ok": 1}, "x"]})
    r.tick("workflow 結束、段存檔、等核准", 1600, tr="main")
    r.tick("結束之後不再記 stalled", 9000, tr="main")
    write_run(r, {"plan": P(status="done", approved={"by": "human", "at": "t", "kind": "human"}), "build": P(gate=False)})
    r.tick("核准", 9100, tr="main")
    r.tr("restart", "@TMP@/tr")
    r.tick("重啟 watcher：不會把舊事件再記一次", 9200, tr="restart")
    r.call("snapshot", note="符號連結還不存在", run="@TMP@/link/_runs/20260101-sdlc-demo", tr=None, now=ts(0))
    r.symlink("link", "@TMP@/proj")
    r.tr("linked", "@TMP@/tr", run="@TMP@/link/_runs/20260101-sdlc-demo")
    r.call("snapshot", note="經過符號連結的 run 路徑：用解開後的路徑比對", run="@TMP@/link/_runs/20260101-sdlc-demo", tr="linked", now=ts(9300))
    # 政策核准、build 段開跑、blocked、全部做完、結案、取消
    write_run(r, {"plan": P(status="done", approved={"by": "policy", "at": "t", "kind": "policy"}), "build": P(gate=False, status="blocked")},
              awf={"pending": {"stage": "build", "workflow": "sdlc-build"}})
    r.tick("build 段開跑又停下", 9400, tr="restart")
    write_run(r, {"plan": P(status="done", approved={"by": "human", "at": "t"}), "build": P(gate=False, status="done")})
    r.tick("全部做完", 9500, tr="restart")
    write_run(r, {"plan": P(status="done", approved={"by": "human", "at": "t"}), "build": P(gate=False, status="done")}, closed={"at": "t"})
    r.tick("結案", 9600, tr="restart")
    write_run(r, {"plan": P(status="done", approved={"by": "human", "at": "t"}), "build": P(gate=False, status="done")}, abandoned={"at": "t", "reason": "不做了"})
    r.tick("取消", 9700, tr="restart")
    r.tick("沒有 transcript 讀取器", 9800)
    r.tr("nodir", "@TMP@/nope")
    r.tick("找不到 transcript 目錄", 9900, tr="nodir")
    r.tr("noroot", None)
    r.tick("沒有 transcript 根目錄", 9950, tr="noroot")
    r.tick("run.json 讀不到：不寫任何檔", 1, run="@TMP@/nope-run")


def scenario_transcripts(r):
    """歸屬判斷與讀不懂的 transcript（watch_note）。"""
    run2 = "@TMP@/proj/_runs/20260102-示範"
    write_run(r, {"plan": P()}, run="proj/_runs/20260102-示範")
    # 別的 run 的 workflow；還沒有 agent 檔也沒有 meta 的目錄；meta 對得上但還沒有 agent
    r.w("tr/sess-1/subagents/workflows/wf_other/agent-x.jsonl", line(0, content="別的 run：/somewhere/else"))
    r.mkdir("tr/sess-1/subagents/workflows/wf_empty")
    r.mkdir("tr/sess-2/subagents/workflows/wf_meta_only")
    r.wj("tr/sess-2/workflows/wf_meta_only.json", {"scriptPath": RUN + "/.wf/sdlc-build.js", "status": "Running"})
    # 派工單裡的路徑是 JSON 跳脫過的（非 ASCII 的 run 名）
    r.w("tr/sess-3/subagents/workflows/wf_esc/agent-e1.jsonl", line(100, content="run 目錄：" + run2) + line(120, "q1", 10))
    r.w("tr/sess-3/subagents/workflows/wf_esc/agent-e2.jsonl", line(stamp="2026-01-01T08:02:00+08:00", rid="q2", tok=20))
    r.wj("tr/sess-3/workflows/wf_esc.json", {"status": "FAILED", "scriptPath": "/x/y/custom.flow.js"})
    r.tr("t1", "@TMP@/tr")
    r.call("snapshot", note="主 run：別的 run 不算、只有 meta 的算", run=RUN, tr="t1", now=ts(9999))
    r.tr("t2", "@TMP@/tr", run=run2)
    r.call("snapshot", note="非 ASCII 的 run 名：比對跳脫過的路徑；workflow 名取腳本檔名", run=run2, tr="t2", now=ts(130))
    r.call("snapshot", note="現在比最後活動還早：idle 當成 0", run=run2, tr="t2", now=ts(5))
    # 第七個 agent 檔才提到 run：不看（只看前六個）
    for i in range(6):
        r.w(f"tr6/s/subagents/workflows/wf_late/agent-a{i}.jsonl", line(1, content="沒提到"))
    r.w("tr6/s/subagents/workflows/wf_late/agent-a6.jsonl", line(1, content="run 目錄：" + RUN))
    r.tr("t6", "@TMP@/tr6")
    r.call("snapshot", note="只看前六個 agent 檔的開頭", run=RUN, tr="t6", now=ts(10))
    # 讀不懂：沒帶時區、同一個檔混用、兩個檔混用、requestId 不能當鍵、時間超出範圍
    cases = {
        "naive": {"a": line(stamp="2026-01-01T00:00:10", rid="q", tok=5, content=None)},
        "mixed-one": {"a": line(10, "q1", 5) + line(stamp="2026-01-01T00:00:20", rid="q2", tok=5)},
        "mixed-two": {"a": line(10, "q1", 5), "b": line(stamp="2026-01-01T00:00:20", rid="q2", tok=5)},
        "unhashable": {"a": line(10, ["x"], 5)},
        "overflow": {"a": line(stamp="0001-01-01T00:00:00+08:00", rid="q", tok=5)},
    }
    for name, files in cases.items():
        for aid, body in files.items():
            r.w(f"trb/{name}/s/subagents/workflows/wf_{name.replace('-', '_')}/agent-{aid}.jsonl", line(content="run 目錄：" + RUN) + body)
        r.tr("b-" + name, f"@TMP@/trb/{name}")
        r.call("snapshot", note=f"讀不懂的 transcript：{name}", run=RUN, tr="b-" + name, now=ts(100))
        r.call("snapshot", note=f"讀不懂的 transcript：{name}（再看一次）", run=RUN, tr="b-" + name, now=ts(200))
    # 先讀得懂、後來才壞：已經讀進去的留著，note 也留著
    r.w("trb/later/s/subagents/workflows/wf_later/agent-a.jsonl", line(0, content="run 目錄：" + RUN) + line(10, "q1", 5))
    r.tr("b-later", "@TMP@/trb/later")
    r.call("snapshot", note="後來才壞：第一次讀得懂", run=RUN, tr="b-later", now=ts(100))
    r.ap("trb/later/s/subagents/workflows/wf_later/agent-a.jsonl", line(stamp="2026-01-01T00:00:20", rid="q2", tok=7) + line(30, "q3", 9))
    r.call("snapshot", note="後來才壞：這次丟例外", run=RUN, tr="b-later", now=ts(200))
    r.call("snapshot", note="後來才壞：之後沒有新內容，workflow 又看得到（note 留著）", run=RUN, tr="b-later", now=ts(300))
    # watch_note 事件：寫進 progress、之後同一句不重複
    write_run(r, {"plan": P()}, run="proj/_runs/note")
    r.w("trb/note/s/subagents/workflows/wf_note/agent-a.jsonl", line(0, content="run 目錄：@TMP@/proj/_runs/note") + line(stamp="2026-01-01T00:00:10", rid="q", tok=5))
    r.tr("b-note", "@TMP@/trb/note", run="@TMP@/proj/_runs/note")
    r.tick("watch_note", 100, run="@TMP@/proj/_runs/note", tr="b-note")
    r.tick("watch_note 不重複", 200, run="@TMP@/proj/_runs/note", tr="b-note")


def scenario_run_view(r):
    """run.json 的各種形狀。"""
    ok_stage = {"plan": P(status="done")}
    cases = [
        ("not-json", "{壞掉", None), ("list", "[1, 2]", None), ("no-stages", '{"event": "sdlc"}', None), ("stages-list", '{"stages": []}', None),
        ("stages-null", '{"stages": null}', None), ("empty-stages", '{"stages": {}}', None), ("nan", '{"stages": {"plan": {"status": "done", "gate": true}}, "x": NaN}', "nan-run"),
        ("huge-int", '{"stages": {"plan": {"status": "done", "gate": true}}, "x": 99999999999999999999999}', "huge-int-run"),
        ("stage-null", '{"stages": {"plan": null, "build": 0, "test": "", "deliver": []}}', None),
        ("stage-no-status", '{"stages": {"plan": {"gate": 1}}}', None), ("stage-str", '{"stages": {"plan": "done"}}', None), ("stage-num", '{"stages": {"plan": 3}}', None),
        ("stage-list", '{"stages": {"plan": [1]}}', None), ("status-types", '{"stages": {"plan": {"status": 3}, "build": {"status": ["x"]}, "test": {"status": null}}}', None),
        ("awf-str", json.dumps({"stages": ok_stage, "awf": "x"}), None), ("awf-list", json.dumps({"stages": ok_stage, "awf": []}), None),
        ("pending-str", json.dumps({"stages": ok_stage, "awf": {"pending": "plan"}}), None), ("pending-empty", json.dumps({"stages": ok_stage, "awf": {"pending": {}}}), None),
        ("pending-types", json.dumps({"stages": ok_stage, "awf": {"pending": {"stage": 3, "workflow": None}}}), None),
        ("event-types", json.dumps({"stages": ok_stage, "event": ["sdlc", 1]}), None), ("event-num", json.dumps({"stages": ok_stage, "event": 0}), None),
        ("approved-empty", json.dumps({"stages": {"plan": P(status="done", approved={})}}), None),
        ("approved-str", json.dumps({"stages": {"plan": P(status="done", approved="human")}}), None),
        ("approved-types", json.dumps({"stages": {"plan": P(status="done", approved={"by": None, "kind": 0}), "build": P(status="done", approved={"by": ["a"], "kind": "policy"})}}), None),
        ("recipe", json.dumps({"stages": ok_stage, "recipe": {"name": "bugfix", "modifiers": ["ui"], "size": "S", "locked": True}}), None),
        ("recipe-str", json.dumps({"stages": ok_stage, "recipe": "bugfix"}), None),
        ("blocked", json.dumps({"stages": {"plan": P(status="invalid")}}), None), ("skipped", json.dumps({"stages": {"plan": P(status="skipped"), "build": P()}}), None),
        ("missing-records", json.dumps({"stages": {"plan": P(status="done", missing_records=["a", "b"])}}), None),
        ("abandoned", json.dumps({"stages": ok_stage, "abandoned": {"at": "t"}, "closed": {"at": "t"}}), None),
        ("empty-name", json.dumps({"stages": {"": P(status="done")}}), None),
        ("name-with-slash", json.dumps({"stages": {"a/b": P(status="done"), "../x": P()}}), None),
    ]
    for name, body, label in cases:
        r.w(f"rv/{name}/run.json", body)
        if name == "empty-name":
            r.w(f"rv/{name}/root-note.md", "段名是空字串時，紀錄目錄就是 run 目錄\n")
        if name == "name-with-slash":
            r.w(f"rv/{name}/a/b/deep.md", "x\n")
            r.w("rv/x/outside.md", "x\n")
        r.call("run_view", note=name, run=f"@TMP@/rv/{name}", **({"label": label} if label else {}))
    r.wb("rv/not-utf8/run.json", b'{"stages": {"plan": {"status": "\xff"}}}')
    r.call("run_view", note="not-utf8", run="@TMP@/rv/not-utf8")
    r.mkdir("rv/dir/run.json")
    r.call("run_view", note="run.json 是目錄", run="@TMP@/rv/dir")
    r.w("rv/trace/run.json", json.dumps({"stages": {"plan": P(status="done"), "build": P(), "test": P(), "deliver": P()}}))
    r.wj("rv/trace/plan.json", {"trace": [{"ok": True}, {"ok": True}, {"ok": None}]})
    r.w("rv/trace/build.json", "[1]")
    r.w("rv/trace/test.json", "{壞")
    r.wj("rv/trace/deliver.json", {"trace": "x"})
    r.call("run_view", note="各段結果的 trace：agent 數與缺件數", run="@TMP@/rv/trace")
    r.call("run_view", note="相對路徑寫法（多餘的斜線與 .）", run="@TMP@/rv//trace/./")


CUR = {"v": 1, "run": "r", "event": "sdlc", "state": "running", "next": "build", "waiting_gate": None, "pending": {"stage": "build", "workflow": "sdlc-build"},
       "stages": [{"name": "plan", "status": "done", "gate": True, "approved": {"by": "human", "kind": "human"}, "agents": 2, "agents_missing": 0, "records": ["research-code", "triage"]},
                  {"name": "build", "status": "pending", "gate": False, "approved": None, "agents": 0, "agents_missing": 0, "records": []}],
       "workflows": [{"wf_run": "wf_a1", "workflow": "sdlc-plan", "status": "running", "finished": False, "idle_seconds": 700, "tokens": 60000,
                      "agents": [{"id": "r1", "role": "tpm-researcher", "label": "research:code", "phase": "", "model": "m", "tokens": 60000, "requests": 3, "last": "t"}]}]}


def scenario_diff(r):
    """記憶檔任何人都寫得到：各種改壞的記憶。"""
    fin = json.loads(json.dumps(CUR))
    fin["workflows"][0].update(finished=True, status="completed")
    quiet = json.loads(json.dumps(CUR))
    quiet["workflows"][0]["idle_seconds"] = None
    gate = json.loads(json.dumps(CUR))
    gate.update(state="waiting_gate", waiting_gate="plan", pending=None, note="注意事項" * 60)
    gate["workflows"] = []
    bare = {"state": "done", "stages": []}
    ok_wf = {"agents": {"r1": {"tokens": 60000}}, "finished": False, "stalled": True}
    prevs = [
        ("空的", {}), ("狀態相同", {"state": "running", "pending": "build", "waiting_gate": None, "note": ""}), ("狀態的型別不同", {"state": 1, "pending": True, "waiting_gate": 0, "note": None}),
        ("stages 是清單", {"stages": []}), ("stages 是 null", {"stages": None}), ("stages 是字串", {"stages": "x"}), ("stages 是數字", {"stages": 3}),
        ("某段是數字", {"stages": {"plan": 3}}), ("某段是字串", {"stages": {"plan": "done"}}), ("某段是清單", {"stages": {"plan": [1]}}), ("某段是 0", {"stages": {"plan": 0}}),
        ("records 是數字", {"stages": {"plan": {"status": "done", "records": 5}}}), ("records 是 true", {"stages": {"plan": {"records": True}}}),
        ("records 是字串（看子字串）", {"stages": {"plan": {"status": "done", "records": "research-code", "approved": 1}}}),
        ("records 是物件（看鍵）", {"stages": {"plan": {"status": "done", "records": {"triage": 1}, "approved": "yes"}}}),
        ("records 有別的型別", {"stages": {"plan": {"status": "done", "records": [1, None, "triage"], "approved": []}}}),
        ("狀態一樣、紀錄一樣", {"stages": {"plan": {"status": "done", "records": ["research-code", "triage"], "approved": True}, "build": {"status": "pending", "records": [], "approved": False}}}),
        ("workflows 是清單", {"workflows": []}), ("workflows 是 null", {"workflows": None}), ("workflows 是字串", {"workflows": "s"}),
        ("某個 workflow 是 null", {"workflows": {"wf_a1": None}}), ("某個 workflow 是清單", {"workflows": {"wf_a1": []}}), ("某個 workflow 是字串", {"workflows": {"wf_a1": "s"}}),
        ("某個 workflow 是數字", {"workflows": {"wf_a1": 5}}), ("某個 workflow 是空物件", {"workflows": {"wf_a1": {}}}),
        ("agents 是清單", {"workflows": {"wf_a1": {"agents": [], "finished": False, "stalled": False}}}), ("agents 是 null", {"workflows": {"wf_a1": {"agents": None}}}),
        ("某個 agent 是 0", {"workflows": {"wf_a1": {"agents": {"r1": 0}, "finished": False, "stalled": False}}}),
        ("某個 agent 是 null（當成新的）", {"workflows": {"wf_a1": {"agents": {"r1": None}, "finished": False, "stalled": False}}}),
        ("某個 agent 是字串", {"workflows": {"wf_a1": {"agents": {"r1": "x"}, "finished": False, "stalled": False}}}),
        ("tokens 是數字字串", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": " 5_000 "}}, "finished": False, "stalled": False}}}),
        ("tokens 不是數字的字串", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": "x"}}, "finished": False, "stalled": False}}}),
        ("tokens 是清單", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": [1]}}, "finished": False, "stalled": False}}}),
        ("tokens 是小數", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 9999.9}}, "finished": False, "stalled": False}}}),
        ("tokens 是 true", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": True}}, "finished": False, "stalled": False}}}),
        ("tokens 差一點到門檻", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 10001}}, "finished": False, "stalled": False}}}),
        ("tokens 剛好到門檻", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 10000}}, "finished": False, "stalled": False}}}),
        ("tokens 比現在多", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 999999}}, "finished": False, "stalled": False}}}),
        ("agent 沒有 tokens", {"workflows": {"wf_a1": {"agents": {"r1": {}}, "finished": False, "stalled": False}}}),
        ("沒有 finished", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 60000}}, "stalled": False}}}),
        ("沒有 stalled", {"workflows": {"wf_a1": {"agents": {"r1": {"tokens": 60000}}, "finished": False}}}),
        ("已經記過停住", {"workflows": {"wf_a1": ok_wf}}), ("已經記過結束", {"workflows": {"wf_a1": {**ok_wf, "finished": "yes", "stalled": 0}}}),
        ("別的 workflow 留著", {"workflows": {"wf_old": {"agents": {}, "finished": True, "stalled": False}}, "extra": {"k": [1, 2.5, None]}}),
    ]
    for note, prev in prevs:
        for cname, cur, stall in (("執行中", CUR, 600), ("結束", fin, 600), ("沒有動靜的資料", quiet, 600)):
            r.call("diff_events", note=f"{note}／{cname}", prev=prev, cur=cur, stall=stall)
    for note, prev in prevs[:3] + prevs[-4:]:
        for cname, cur, stall in (("stall 是 0", CUR, 0), ("stall 是負的", CUR, -5), ("還沒到門檻", CUR, 701), ("剛好到門檻", CUR, 700), ("等核准、有 note", gate, 600), ("最少的快照", bare, 600)):
            r.call("diff_events", note=f"{note}／{cname}", prev=prev, cur=cur, stall=stall)
    r.call("diff_events", note="note 一樣不重複", prev={"note": ("注意事項" * 60)}, cur=gate, stall=600)
    r.call("diff_events", note="等核准記過了", prev={"waiting_gate": "plan", "state": "waiting_gate"}, cur=gate, stall=600)
    for snap in (CUR, fin, quiet, gate):
        r.call("text_of", snap=snap)
    big = json.loads(json.dumps(fin))
    big["workflows"][0].update(tokens=1234567, workflow="", status="")
    big["stages"][0].update(approved=None, agents_missing=1)
    big["stages"][1].update(status="skipped")
    r.call("text_of", note="千分位、沒有 workflow 名、等核准", snap=big)


def scenario_state(r):
    """.watch-state.json 的各種形狀（tick 不能中斷）。"""
    states = [
        ("不是 JSON", "{壞", None), ("清單", "[1]", None), ("v 是 2", '{"v": 2, "seq": 9, "mem": {}}', None), ("v 是 true", '{"v": true, "seq": 9, "mem": {}}', None),
        ("v 是 1.0", '{"v": 1.0, "seq": 9, "mem": {}, "extra": "留著"}', None), ("v 是字串", '{"v": "1", "seq": 9, "mem": {}}', None),
        ("seq 是字串", '{"v": 1, "seq": "x", "mem": {"stages": []}}', None), ("seq 是負的", '{"v": 1, "seq": -1, "mem": {}}', None), ("seq 是 true", '{"v": 1, "seq": true, "mem": {}}', None),
        ("seq 是小數", '{"v": 1, "seq": 1.5, "mem": {}}', None), ("沒有 seq", '{"v": 1, "mem": {}}', None), ("seq 很大", '{"v": 1, "seq": 4611686018427387904, "mem": {}}', None),
        ("seq 超出 64 位元", '{"v": 1, "seq": 99999999999999999999999, "mem": {"state": "waiting_gate"}}', "huge-int-state"),
        ("有 NaN", '{"v": 1, "seq": 9, "mem": {"state": "waiting_gate", "x": NaN}}', "nan-state"),
        ("mem 是空清單（當成空的）", '{"v": 1, "seq": 3, "mem": []}', None), ("mem 是清單", '{"v": 1, "seq": 3, "mem": [1]}', None), ("mem 是字串", '{"v": 1, "seq": 3, "mem": "x"}', None),
        ("mem 是 null", '{"v": 1, "seq": 3, "mem": null}', None), ("沒有 mem", '{"v": 1, "seq": 3}', None),
        ("mem 的 stages 是清單", '{"v": 1, "seq": 3, "mem": {"stages": []}}', None), ("mem 的 workflows 是字串", '{"v": 1, "seq": 3, "mem": {"workflows": "x", "state": "waiting_gate"}}', None),
        ("mem 的某段是數字", '{"v": 1, "seq": 3, "mem": {"stages": {"plan": 7}}}', None), ("記過了", '{"v": 1, "seq": 3, "mem": {"state": "waiting_gate", "pending": "", "stages": {"plan": {"status": "done", "records": [], "approved": false}}, "waiting_gate": "plan", "workflows": {}, "note": ""}}', None),
    ]
    for i, (note, body, label) in enumerate(states):
        run = f"st/s{i:02d}"
        write_run(r, {"plan": P(status="done")}, run=run)
        r.w(run + "/" + aw.STATE, body)
        r.tick("記憶檔：" + note, 100, run="@TMP@/" + run, **({"label": label} if label else {}))
        r.tick("記憶檔：" + note + "（再看一次）", 200, run="@TMP@/" + run, **({"label": label} if label else {}))
    # 寫不出去
    write_run(r, {"plan": P(status="done")}, run="st/progress-dir")
    r.mkdir("st/progress-dir/" + aw.PROGRESS)
    r.tick("progress.jsonl 寫不出去（是目錄）", 100, run="@TMP@/st/progress-dir")
    write_run(r, {"plan": P(status="done")}, run="st/state-dir")
    r.mkdir("st/state-dir/" + aw.STATE)
    r.tick("記憶檔寫不出去（是目錄）：事件照寫，下次會重複", 100, run="@TMP@/st/state-dir")
    r.tick("記憶檔寫不出去（是目錄）：再看一次", 200, run="@TMP@/st/state-dir")
    write_run(r, {"plan": "done"}, run="st/crash")
    r.tick("某段不是物件：Python 版在這裡是沒接住的例外", 100, run="@TMP@/st/crash")


def scenario_notify(r):
    A30 = rep("a", 30)
    profiles = [
        None, {}, [], "x", {"progress": None}, {"progress": []}, {"progress": "x"}, {"progress": {}}, {"progress": {"notify": None}}, {"progress": {"notify": []}},
        {"progress": {"notify": {}}}, {"progress": {"notify": {"command": ""}}}, {"progress": {"notify": {"command": []}}}, {"progress": {"notify": {"command": ["notify-cmd", "--x"]}}},
        {"progress": {"notify": {"command": [1, None, "a b"]}}}, {"progress": {"notify": {"command": "notify-cmd --x 'a b' \"c d\" e\\ f"}}}, {"progress": {"notify": {"command": "   "}}},
        {"progress": {"notify": {"command": "a 'b"}}}, {"progress": {"notify": {"command": "a \\"}}}, {"progress": {"notify": {"command": 5}}}, {"progress": {"notify": {"command": True}}},
        {"progress": {"notify": {"command": "x", "events": ["workflow_stalled", "waiting_gate", "nope", 3, None, ["run_state"], "run_state"]}}},
        {"progress": {"notify": {"command": "x", "events": "run_state"}}}, {"progress": {"notify": {"command": "x", "events": {"run_state": 1, "zz": 2}}}},
        {"progress": {"notify": {"command": "x", "events": 5}}}, {"progress": {"notify": {"command": "x", "events": None}}}, {"progress": {"notify": {"command": "x", "events": []}}},
        {"progress": {"notify": {"command": {"a": 1}}}},
    ]
    for p in profiles:
        r.call("notify_config", profile=p)
    base = {"v": 1, "seq": 1, "at": "2026-01-01T00:00:00Z", "run": "20260101-sdlc-demo", "type": "record_written", "stage": "plan"}
    labels = [
        "research:code#2", "../../etc/passwd", "a/b", "a\\b", "a\"b", "a'b", "", " ", "調研 一", "ａｂｃ１２３", "e\u0301", "caf\u00e9", "x\u00b2", "\u2167", "a_b", "a\tb", "a\nb", "a\n", "a\r", "a\x1c",
        "😀", "a.b:c#d@e+f-g", "a,b", "a(b)", "a=b", "a;b", "a*b", "\u3000", "\u9fff", "\ua000", "\u4dff", "\u30a2", "x" * 80, "x" * 81, "長" * 80, "長" * 81, "a\u200db", "a\u0300", "١٢٣", "\u00bd",
        3, None, True, ["a"], {"a": 1}, 1.5,
    ]
    for v in labels:
        r.call("notify_payload", event={**base, "label": v})
    secrets = [
        J("gh", "p_", rep("a", 20)), J("gh", "p_", rep("a", 19)), J("github", "_pat_", rep("b", 20)), J("sk", "-", rep("c", 16)), J("sk", "-", rep("c", 15)), J("AK", "IA", rep("Q", 16)),
        J("ak", "ia", rep("q", 16)), J("xo", "xb", "-", rep("1", 10)), J("xo", "xz", "-", rep("1", 10)), J("-----BEGIN RSA ", "PRIVATE KEY"), J("-----BEGIN ", "PRIVATE KEY"), "-----BEGIN PUBLIC KEY",
        J("ey", "J", rep("a", 20), ".", rep("b", 10), "."), J("ey", "J", rep("a", 20), ".", rep("b", 10)), J("pass", "word = ", rep("p", 6)), J("pass", "word = ", rep("p", 5)),
        J("PASS", "WD:", rep("p", 8)), J("Sec", "ret\t=\t", rep("s", 8)), J("api", "_key=", rep("k", 8)), J("api", "-key:", rep("k", 8)), J("api", "key = ", rep("k", 8)), J("tok", "en\x1c=\x1c", rep("t", 8)),
        J("tok", "en\u3000=\u3000", rep("t", 8)), J("TOK", "EN=", rep("t", 6)), J("to", "ken", " ", rep("t", 8)), J("t\u0131ken", "=", rep("t", 8)), J("TOKEN"), J("\u0130", "token=", rep("t", 8)),
    ]
    for s in secrets:
        for field in ("label", "status", "run", "at", "requirement"):
            r.call("notify_payload", event={**base, field: s})
    r.call("notify_payload", note="機敏值在數字欄位旁邊的字串", event={**base, "tokens": 5, "model": J("gh", "p_", A30)})
    r.call("notify_payload", note="watch_note 的訊息不送", event={**base, "type": "watch_note", "status": "OSError: /srv/demo/repo/x.json"})
    r.call("notify_payload", note="watch_note 的訊息像機敏值：整筆不送", event={**base, "type": "watch_note", "status": J("pass", "word=", rep("p", 9))})
    r.call("notify_payload", note="白名單以外的欄位都拿掉", event={**base, "requirement": "機密需求內容", "summary": "內部摘要", "path": "/secret/path", "phase": "x", "id": "r1"})
    for k in ("status", "stage", "workflow", "role", "model", "by", "kind", "state", "wf_run", "run", "at", "type"):
        r.call("notify_payload", note=f"{k} 含路徑字元", event={**base, k: "a/b c"})
        r.call("notify_payload", note=f"{k} 不是字串", event={**base, k: 7})
    r.call("notify_payload", note="數字欄位照送", event={**base, "tokens": 130000, "requests": 3, "idle_seconds": 601, "seq": 99, "v": 1})
    r.call("notify_payload", note="空的事件", event={})
    cfg = {"argv": ["notify-cmd", "--x"], "events": ["workflow_stalled", "waiting_gate"]}
    every = {"argv": ["notify-cmd"], "events": []}
    gate = {**base, "type": "waiting_gate"}
    r.call("notify", note="沒有設定", cfg=None, event=gate)
    r.call("notify", note="類型不在清單", cfg=cfg, event=base)
    r.call("notify", note="類型在清單", cfg=cfg, event=gate)
    r.call("notify", note="清單是空的＝全部", cfg=every, event=base)
    r.call("notify", note="事件沒有 type", cfg=cfg, event={"v": 1})
    r.call("notify", note="事件沒有 type、清單是空的", cfg=every, event={"v": 1})
    r.call("notify", note="像機敏值：不送並回警告", cfg=cfg, event={**gate, "label": J("tok", "en=gh", "p_", A30)})
    for mode in ("exit:3", "exit:-9", "exit:255", "raise:OSError", "raise:FileNotFoundError", "raise:PermissionError", "raise:TimeoutExpired", "raise:ValueError"):
        r.call("notify", note=f"指令 {mode}", cfg=cfg, event=gate, runner=mode)
    # tick 帶 notify：每個事件呼叫一次、警告不重複
    for name, runner, c in (("ok", "ok", every), ("fail", "exit:3", every), ("boom", "raise:FileNotFoundError", every), ("filtered", "ok", cfg)):
        run = "nt/" + name
        write_run(r, {"plan": P(status="done"), "build": P(gate=False)}, run=run, awf={"pending": {"stage": "build", "workflow": "sdlc-build"}})
        r.w(run + "/plan/research-code.md", "x\n")
        r.tick("notify：" + name, 100, run="@TMP@/" + run, cfg=c, runner=runner)
    write_run(r, {"plan": P(status="done")}, run="nt/" + "secretish")
    r.w("nt/secretish/plan/" + "x.md", "x\n")
    r.tick("notify：一般事件照送", 100, run="@TMP@/nt/secretish", cfg=every)


def scenario_statusline(r):
    r.call("status_line", note="沒有設定檔、沒有 run", project="@TMP@/sl/none")
    r.wj("sl/a/.claude/teams/guard.config.json", {"runs_dir": "_runs"})
    r.call("status_line", note="runs_dir 不存在", project="@TMP@/sl/a")
    write_run(r, {"plan": P(status="done"), "build": P(gate=False)}, run="sl/a/_runs/20260101-old")
    r.mtime("sl/a/_runs/20260101-old/run.json", 1_700_000_000)
    r.call("status_line", note="等核准", project="@TMP@/sl/a")
    write_run(r, {"plan": P(status="done", approved={"by": "human"}), "build": P(gate=False)}, run="sl/a/_runs/20260102-new", awf={"pending": {"stage": "build", "workflow": "sdlc-build"}})
    r.wj("sl/a/_runs/20260102-new/build.json", {"trace": [{"ok": True}, {"ok": True}, {"ok": False}]})
    r.mtime("sl/a/_runs/20260102-new/run.json", 1_700_000_100)
    r.call("status_line", note="最近動過的 run、進行中的段與 agent 數", project="@TMP@/sl/a")
    r.mtime("sl/a/_runs/20260101-old/run.json", 1_700_000_200)
    r.call("status_line", note="修改時間比較新的那個", project="@TMP@/sl/a")
    r.w("sl/a/_runs/readme.md", "不是 run\n")
    r.mkdir("sl/a/_runs/empty-dir")
    r.mkdir("sl/a/_runs/dir-run/run.json")
    write_run(r, {"plan": P(status="blocked")}, run="sl/a/_runs/.hidden")
    r.mtime("sl/a/_runs/.hidden/run.json", 1_700_000_300)
    r.call("status_line", note="隱藏目錄也算；blocked", project="@TMP@/sl/a")
    write_run(r, {"plan": P(status="done", approved={"by": "human"})}, run="sl/a/_runs/20260103-closed", closed={"at": "t"})
    r.mtime("sl/a/_runs/20260103-closed/run.json", 1_700_000_400)
    r.call("status_line", note="最近的 run 已結案：空的", project="@TMP@/sl/a")
    write_run(r, {"plan": P(status="done", approved={"by": "human"})}, run="sl/a/_runs/20260104-done")
    r.mtime("sl/a/_runs/20260104-done/run.json", 1_700_000_500)
    r.call("status_line", note="全部做完：印狀態", project="@TMP@/sl/a")
    r.w("sl/a/_runs/20260105-bad/run.json", "{壞")
    r.mtime("sl/a/_runs/20260105-bad/run.json", 1_700_000_600)
    r.call("status_line", note="最近的 run.json 讀不懂：空的", project="@TMP@/sl/a")
    r.w("sl/a/_runs/20260106-crash/run.json", '{"stages": {"plan": "done"}}')
    r.mtime("sl/a/_runs/20260106-crash/run.json", 1_700_000_700)
    r.call("status_line", note="某段不是物件：空的（不丟例外）", project="@TMP@/sl/a")
    write_run(r, {"plan": P(status="done")}, run="sl/elsewhere/20260101-abs")
    r.mtime("sl/elsewhere/20260101-abs/run.json", 1_700_000_000)
    for name, body in (("list", "[1]"), ("str", '"x"'), ("bad", "{壞"), ("num-dir", '{"runs_dir": 5}'), ("empty", "{}"), ("abs", '{"runs_dir": "@TMP@/sl/elsewhere"}'), ("null", "null")):
        r.w(f"sl/{name}/.claude/teams/guard.config.json", body)
        write_run(r, {"plan": P()}, run=f"sl/{name}/_tpm/runs/20260101-default")
        r.mtime(f"sl/{name}/_tpm/runs/20260101-default/run.json", 1_700_000_000)
        write_run(r, {"plan": P()}, run=f"sl/{name}/5/20260101-five")
        r.mtime(f"sl/{name}/5/20260101-five/run.json", 1_700_000_000)
        r.call("status_line", note=f"設定檔：{name}", project=f"@TMP@/sl/{name}")


def scenario_cli(r):
    r.w("bin/notify.sh", NOTIFY_SH)
    run = "cli/_runs/20260101-cli"
    write_run(r, {"plan": P(status="done"), "build": P(gate=False)}, run=run)
    r.wj("cli/.claude/teams/guard.config.json", {"runs_dir": "_runs"})
    r.w("cli/_runs/20260101-cli/plan/research-code.md", "x\n")
    r.w("clitr/s/subagents/workflows/wf_c1/agent-a.jsonl", line(0, content="run 目錄：@TMP@/cli/_runs/20260101-cli") + line(100, "q1", 1234))
    r.wj("clitr/s/subagents/workflows/wf_c1/agent-a.meta.json", {"agentType": "agent-workflow:tpm-researcher", "description": "research:code"})
    r.run("沒有參數：印用法", project="cli")
    r.run("--run 沒有值", "--run", project="cli")
    r.run("--run 是空字串", "--run", "", project="cli")
    r.run("run 不存在", "--run", "_runs/nope", "--once", project="cli")
    r.run("--stall-seconds 不是整數", "--run", run[4:], "--once", "--stall-seconds", "x", project="cli")
    r.run("--interval 不是整數", "--run", run[4:], "--once", "--interval", "1.5", project="cli")
    r.run("--interval 沒有值（用預設）", "--run", run[4:], "--once", "--interval", project="cli")
    r.run("--once（文字）", "--run", run[4:], "--once", project="cli", transcripts="clitr")
    r.read_step(run + "/" + aw.PROGRESS)
    r.read_step(run + "/" + aw.STATE)
    r.run("--once --json（沒有新事件）", "--run", run[4:], "--once", "--json", project="cli", transcripts="clitr")
    r.run("絕對路徑的 --run、整數前後有空白", "--run", "@TMP@/" + run, "--once", "--stall-seconds", " 100 ", "--interval", "+3", project="cli", transcripts="clitr")
    r.read_step(run + "/" + aw.PROGRESS)
    r.run("--stall-seconds 是負的（當成 0）", "--run", run[4:], "--once", "--stall-seconds", "-5", "--json", project="cli", transcripts="clitr", now=5000)
    r.run("statusline", "statusline", project="cli")
    r.run("statusline：沒有 run", "statusline", project="sl/none")
    r.run("statusline 後面的參數不看", "statusline", "--run", "x", project="cli")
    # 長駐：每次睡之間檔案有變
    loop = "cli/_runs/20260102-loop"
    write_run(r, {"plan": P(), "build": P(gate=False)}, run=loop, awf={"pending": {"stage": "plan", "workflow": "sdlc-plan"}})
    approved = json.dumps({"event": "sdlc", "stages": {"plan": P(status="done", approved={"by": "human", "at": "t", "kind": "human"}), "build": P(gate=False, status="done")}}, ensure_ascii=False)
    closed = json.dumps({"event": "sdlc", "stages": {"plan": P(status="done", approved={"by": "human", "at": "t", "kind": "human"}), "build": P(gate=False, status="done")}, "closed": {"at": "t"}}, ensure_ascii=False)
    r.run("長駐：睡兩次之後結案就結束", "--run", loop[4:], "--interval", "7", project="cli", sleeps=[
        [{"op": "write", "path": loop + "/plan/triage.md", "text": "x\n"}],
        [{"op": "write", "path": loop + "/run.json", "text": approved}],
        [{"op": "write", "path": loop + "/run.json", "text": closed}],
    ])
    r.read_step(loop + "/" + aw.PROGRESS)
    r.run("長駐：已經結案的 run 看一次就結束", "--run", loop[4:], project="cli")
    loop2 = "cli/_runs/20260103-loop"
    write_run(r, {"plan": P()}, run=loop2)
    r.run("長駐：按 Ctrl-C 結束（exit 0）；--interval 0 當成 1", "--run", loop2[4:], "--interval", "0", project="cli", sleeps=[[]])
    r.run("長駐：run.json 中途不見了", "--run", loop2[4:], project="cli", sleeps=[[{"op": "rm", "path": loop2 + "/run.json"}]])
    # profile 的 notify：真的跑暫存目錄裡的小腳本
    for name, prof in (("argv", {"progress": {"notify": {"command": ["sh", "@TMP@/bin/notify.sh"], "events": ["waiting_gate", "stage_saved"]}}}),
                       ("text", {"progress": {"notify": {"command": "sh '@TMP@/bin/notify.sh'"}}}),
                       ("missing", {"progress": {"notify": {"command": ["@TMP@/bin/no-such-command"]}}}),
                       ("invalid", {"progress": {"notify": {"command": ["sh", "@TMP@/bin/notify.sh"], "events": ["nope"]}}}),
                       ("not-object", [1])):
        proj = "np/" + name
        r.wj(proj + "/.claude/teams/methodology.json", prof)
        write_run(r, {"plan": P(status="done")}, run=proj + "/_runs/20260101-n")
        r.run("profile 的 notify：" + name, "--run", "_runs/20260101-n", "--once", project=proj)
        r.read_step("bin/notify.log")
    r.w("bin/notify.fail", "x\n")
    write_run(r, {"plan": P(status="done")}, run="np/argv/_runs/20260102-n")
    r.run("profile 的 notify：指令失敗只警告", "--run", "_runs/20260102-n", "--once", project="np/argv")
    r.read_step("bin/notify.log")
    r.rm("bin/notify.fail")
    r.wj("np/quote/.claude/teams/methodology.json", {"progress": {"notify": {"command": "sh 'x"}}})
    write_run(r, {"plan": P(status="done")}, run="np/quote/_runs/20260101-n")
    r.run("profile 的 notify：引號沒關（Python 版沒接住的例外）", "--run", "_runs/20260101-n", "--once", project="np/quote")
    r.w("cli/_runs/20260104-crash/run.json", '{"stages": {"plan": "done"}}')
    r.run("某段不是物件（Python 版沒接住的例外）", "--run", "_runs/20260104-crash", "--once", project="cli")
    r.w("cli/_runs/20260105-nan/run.json", '{"stages": {"plan": {"status": "done", "gate": true}}, "x": NaN}')
    r.run("run.json 有 NaN", "--run", "_runs/20260105-nan", "--once", project="cli", label="nan-run")


def main():
    r = Rec()
    saved_env = dict(os.environ)
    selftest = subprocess.run([sys.executable, str(TOOLS / "awf_watch.py"), "--selftest"], capture_output=True, text=True, encoding="utf-8", cwd=str(ROOT / "scaffold"))
    try:
        scenario_flow(r)
        scenario_transcripts(r)
        scenario_run_view(r)
        scenario_diff(r)
        scenario_state(r)
        scenario_notify(r)
        scenario_statusline(r)
        scenario_cli(r)
    finally:
        os.environ.clear()
        os.environ.update(saved_env)
        shutil.rmtree(r.td, ignore_errors=True)
    one = lambda v: json.dumps(v, ensure_ascii=False)  # noqa: E731
    sys.stdout.write("{\n \"說明\": %s,\n \"usage\": %s,\n \"selftest\": %s,\n \"steps\": [\n%s\n ]\n}\n" % (
        one("watch 的對跑基準，由 tests/parity/record_watch.py 產生；不要手改。"), one(aw.__doc__),
        one({"code": selftest.returncode, "stdout": selftest.stdout}), ",\n".join("  " + one(x) for x in r.steps)))


if __name__ == "__main__":
    main()
