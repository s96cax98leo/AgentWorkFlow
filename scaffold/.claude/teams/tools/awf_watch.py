#!/usr/bin/env python3
"""watcher：把一個 run 的進度寫成外部程式看得懂的事件（awf.py watch 會轉到這裡）。

用法（在已 init 的專案根目錄）：
    python3 .claude/teams/tools/awf.py watch --run <run_dir> [--interval 5] [--stall-seconds 900]   # 長駐：每隔幾秒寫 <run_dir>/progress.jsonl
    python3 .claude/teams/tools/awf.py watch --run <run_dir> --once [--json]                       # 只看一次：印快照，順便補寫事件
    python3 .claude/teams/tools/awf.py statusline                                                  # 一行狀態（給 Claude Code 的 statusLine 用）
    python3 .claude/teams/tools/awf_watch.py --selftest

三個來源，全部唯讀：
    1. run.json 與各段結果：段開始、存檔、等核准、核准、結束。
    2. 執行紀錄檔（<run_dir>/<段>/<label>.md）的出現：哪個 agent 交件了。
    3. Claude Code 的 workflow transcript：哪些 agent 在跑、用哪個模型、目前幾個 token、最後活動時間。
       這個格式官方沒有承諾穩定；讀不懂時事件只是少了 token 與模型，不會中斷。

事件格式見 schemas/progress-event.schema.json（一行一個 JSON）。外部程式 tail 這個檔就好，不需要網路。
workflow 還沒結束卻超過 --stall-seconds 沒有任何 agent 有動靜時，寫一筆 workflow_stalled（例如帳號額度用完，agent 全停住）。

這支不參與任何閘門：它壞掉、沒開、progress.jsonl 被改，流程與核准都不受影響（save_run、publish、merge_gate 不讀這些檔）。
profile 的 progress.notify 有設才會呼叫外部指令（預設不設）：每個事件呼叫一次，只送白名單欄位，內容像機敏值就不送。只用標準庫。
"""
from __future__ import annotations

import datetime
import json
import os
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import awf_cost  # noqa: E402
import save_run as sr  # noqa: E402

PROGRESS = "progress.jsonl"
STATE = ".watch-state.json"
EVENT_TYPES = ("run_state", "stage_started", "stage_saved", "waiting_gate", "stage_approved", "record_written",
               "agent_started", "agent_progress", "workflow_finished", "workflow_stalled", "watch_note")
# 送給外部指令（notify）的欄位白名單：都是狀態與數字，沒有需求內容、摘要、問題、路徑
NOTIFY_FIELDS = ("v", "seq", "at", "type", "run", "stage", "status", "state", "by", "kind", "label", "role", "model", "tokens", "requests",
                 "idle_seconds", "workflow", "wf_run")
SECRETISH = re.compile(r"ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16}|xox[abprs]-[A-Za-z0-9-]{10,}"
                       r"|-----BEGIN [A-Z ]*PRIVATE KEY|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}\.|(?i:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S{6,}")
PROGRESS_STEP = 50_000      # 同一個 agent 每多這麼多 token 才再記一筆 agent_progress
FINISHED = ("completed", "failed", "cancelled", "canceled", "error", "stopped", "killed")


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def now_iso() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load_json(p: Path):
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


# ------------------------------------------------------------------ 來源 1、2：run 目錄
def run_view(run: Path) -> dict | None:
    """run.json 與紀錄檔整理成一份快照的前半（沒有 transcript 的部分）。run.json 讀不到回 None。"""
    data = load_json(run / "run.json")
    if not isinstance(data, dict) or not isinstance(data.get("stages"), dict):
        return None
    try:
        snap = sr.status_snapshot(run, data)
    except Exception:  # noqa: BLE001  快照算不出來時退回最少的資訊
        snap = {"state": "running", "next": None, "waiting_gate": None,
                "stages": [{"name": k, "status": str((v or {}).get("status")), "gate": bool((v or {}).get("gate")), "approved": None,
                            "agents": 0, "agents_missing": 0, "missing_records": 0} for k, v in data["stages"].items()]}
    stages = []
    for s in snap["stages"]:
        st = data["stages"].get(s["name"]) or {}
        ap = st.get("approved") if isinstance(st.get("approved"), dict) else None
        d = run / s["name"]
        try:
            recs = sorted(f.name[:-3] for f in d.glob("*.md") if f.is_file() and f.name not in (sr.INDEX_NAME, "pack.md")) if d.is_dir() else []
        except OSError:
            recs = []
        stages.append({"name": s["name"], "status": s["status"], "gate": s["gate"],
                       "approved": {"by": str(ap.get("by")), "kind": str(ap.get("kind") or "human")} if ap else None,
                       "agents": s["agents"], "agents_missing": s["agents_missing"], "records": recs})
    pend = (data.get("awf") or {}).get("pending") or {}
    return {"v": 1, "run": run.name, "event": str(data.get("event") or ""), "state": snap["state"], "next": snap.get("next"),
            "waiting_gate": snap.get("waiting_gate"), "pending": {"stage": str(pend.get("stage") or ""), "workflow": str(pend.get("workflow") or "")} if pend else None,
            "stages": stages, "workflows": []}


# ------------------------------------------------------------------ 來源 3：workflow transcript
class Transcripts:
    """這個 run 的 workflow transcript。哪些 wf_* 目錄屬於這個 run 只判一次；每個 agent 檔只讀新增的部分。"""

    def __init__(self, root: Path | None, run: Path):
        self.root, self.run = root, run
        self.match: dict[str, bool] = {}
        self.tails: dict[str, dict] = {}
        self.broken = ""
        try:
            self.needles = {run.as_posix(), run.resolve().as_posix()}
        except OSError:
            self.needles = {run.as_posix()}

    def _mine(self, wf_dir: Path) -> bool:
        key = str(wf_dir)
        if key not in self.match:
            script = str(awf_cost.read_wf_meta(wf_dir).get("scriptPath") or "").replace("\\", "/")
            hit = any(n and n in script for n in self.needles)
            if not hit:      # 不是 awf.py 產生的腳本：看派工單有沒有提到這個 run 目錄（只看每個 agent 的開頭）
                for f in sorted(wf_dir.glob("agent-*.jsonl"))[:6]:
                    try:
                        with f.open(encoding="utf-8", errors="replace") as fh:
                            head = fh.read(60_000)
                    except OSError:
                        continue
                    if any(n in head or json.dumps(n)[1:-1] in head for n in self.needles):
                        hit = True
                        break
            # 還沒有任何 agent 檔、也沒有 meta 的目錄先不下結論（下次再看）
            if hit or any(wf_dir.glob("agent-*.jsonl")):
                self.match[key] = hit
            return hit
        return self.match[key]

    def view(self, now: datetime.datetime) -> list[dict]:
        if self.root is None or not self.root.is_dir():
            return []
        out = []
        try:
            dirs = awf_cost.find_wf_dirs([self.root])
        except OSError as e:
            self.broken = f"讀不到 transcript 目錄（{e}）"
            return []
        for wf_dir in dirs:
            try:
                if not self._mine(wf_dir):
                    continue
                meta = awf_cost.read_wf_meta(wf_dir)
                agents, lasts = [], []
                for f in sorted(wf_dir.glob("agent-*.jsonl")):
                    st = self.tails[str(f)] = awf_cost.tail_agent(f, self.tails.get(str(f)))
                    m = awf_cost.read_meta(f.with_name(f.name[: -len(".jsonl")] + ".meta.json"))
                    last = st["last"]
                    if last is not None:
                        lasts.append(last)
                    agents.append({"id": f.stem[len("agent-"):], "role": str(m.get("agentType") or "").split(":")[-1], "label": str(m.get("description") or "")[:120],
                                   "phase": str(m.get("workflowPhase") or ""), "model": st["model"] or str(m.get("model") or ""),
                                   "tokens": int(st["tokens"]), "requests": int(st["requests"]),
                                   "last": last.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ") if last else ""})
                status = str(meta.get("status") or "")
                idle = int((now - max(lasts)).total_seconds()) if lasts else None
                out.append({"wf_run": wf_dir.name, "workflow": str(meta.get("workflowName") or Path(str(meta.get("scriptPath") or "")).stem or ""),
                            "status": status, "finished": status.lower() in FINISHED, "idle_seconds": idle if idle is None or idle >= 0 else 0,
                            "tokens": sum(a["tokens"] for a in agents), "agents": agents})
            except Exception as e:  # noqa: BLE001  格式變了：這個 workflow 少了細節，其他照常
                self.broken = f"transcript 讀不懂（{type(e).__name__}：{e}）；事件會少 token 與模型"
        return out


def snapshot(run: Path, tr: Transcripts | None, now: datetime.datetime | None = None) -> dict | None:
    view = run_view(run)
    if view is None:
        return None
    if tr is not None:
        view["workflows"] = tr.view(now or datetime.datetime.now(datetime.timezone.utc))
        if tr.broken:
            view["note"] = tr.broken
    return view


# ------------------------------------------------------------------ 事件：前後兩份快照的差異
def diff_events(prev: dict, cur: dict, stall_seconds: int) -> tuple[list[dict], dict]:
    """（新事件, 更新後的記憶）。prev 是上一次留下的記憶（第一次給 {}）；同一件事只會記一次。"""
    mem = json.loads(json.dumps(prev)) if prev else {}
    ev: list[dict] = []
    if mem.get("state") != cur["state"]:
        ev.append({"type": "run_state", "state": cur["state"]})
        mem["state"] = cur["state"]
    pend = (cur.get("pending") or {}).get("stage") or ""
    if pend and mem.get("pending") != pend:
        ev.append({"type": "stage_started", "stage": pend, "workflow": (cur.get("pending") or {}).get("workflow") or ""})
    mem["pending"] = pend
    ms = mem.setdefault("stages", {})
    for s in cur["stages"]:
        old = ms.get(s["name"]) or {}
        if old.get("status") != s["status"] and s["status"] not in ("pending", "skipped"):
            ev.append({"type": "stage_saved", "stage": s["name"], "status": s["status"]})
        for label in s["records"]:
            if label not in (old.get("records") or []):
                ev.append({"type": "record_written", "stage": s["name"], "label": label})
        if s["approved"] and not old.get("approved"):
            ev.append({"type": "stage_approved", "stage": s["name"], "by": s["approved"]["by"], "kind": s["approved"]["kind"]})
        ms[s["name"]] = {"status": s["status"], "records": list(s["records"]), "approved": bool(s["approved"])}
    if cur.get("waiting_gate") and mem.get("waiting_gate") != cur["waiting_gate"]:
        ev.append({"type": "waiting_gate", "stage": cur["waiting_gate"]})
    mem["waiting_gate"] = cur.get("waiting_gate")
    mw = mem.setdefault("workflows", {})
    for w in cur.get("workflows") or []:
        ow = mw.setdefault(w["wf_run"], {"agents": {}, "finished": False, "stalled": False})
        for a in w["agents"]:
            oa = ow["agents"].get(a["id"])
            base = {"wf_run": w["wf_run"], "workflow": w["workflow"], "label": a["label"], "role": a["role"], "model": a["model"], "tokens": a["tokens"], "requests": a["requests"]}
            if oa is None:
                ev.append({"type": "agent_started", **base})
                ow["agents"][a["id"]] = {"tokens": a["tokens"]}
            elif a["tokens"] - int(oa.get("tokens") or 0) >= PROGRESS_STEP:
                ev.append({"type": "agent_progress", **base})
                oa["tokens"] = a["tokens"]
        if w["finished"] and not ow["finished"]:
            ev.append({"type": "workflow_finished", "wf_run": w["wf_run"], "workflow": w["workflow"], "status": w["status"], "tokens": w["tokens"]})
            ow["finished"] = True
        idle = w.get("idle_seconds")
        if not w["finished"] and idle is not None and idle >= stall_seconds > 0:
            if not ow["stalled"]:
                ev.append({"type": "workflow_stalled", "wf_run": w["wf_run"], "workflow": w["workflow"], "idle_seconds": idle, "tokens": w["tokens"]})
                ow["stalled"] = True
        else:
            ow["stalled"] = False      # 又有動靜了：之後再停住會再記一次
    note = cur.get("note") or ""
    if note and mem.get("note") != note:
        ev.append({"type": "watch_note", "status": note[:200]})
    mem["note"] = note
    return ev, mem


# ------------------------------------------------------------------ notify（預設不設）
def notify_config(profile: dict | None) -> dict | None:
    n = ((profile or {}).get("progress") or {}).get("notify") if isinstance((profile or {}).get("progress"), dict) else None
    if not isinstance(n, dict) or not n.get("command"):
        return None
    cmd = n["command"]
    argv = [str(x) for x in cmd] if isinstance(cmd, list) else shlex.split(str(cmd), posix=os.name != "nt")
    events = [e for e in n.get("events") or [] if e in EVENT_TYPES]
    return {"argv": argv, "events": events} if argv else None


def notify_payload(event: dict) -> dict | None:
    """要送出去的內容：只留白名單欄位；任何欄位看起來像機敏值就整筆不送（回 None）。"""
    out = {k: event[k] for k in NOTIFY_FIELDS if k in event}
    return None if SECRETISH.search(json.dumps(out, ensure_ascii=False)) else out


def notify(cfg: dict | None, event: dict, runner=subprocess.run) -> str:
    """回空字串＝送了或不用送；其他是警告文字（不擋流程）。"""
    if not cfg or (cfg["events"] and event.get("type") not in cfg["events"]):
        return ""
    payload = notify_payload(event)
    if payload is None:
        return "notify：這筆事件看起來有機敏值，沒有送出"
    try:
        r = runner(cfg["argv"], input=json.dumps(payload, ensure_ascii=False), capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=10)
        return "" if r.returncode == 0 else f"notify 指令 exit {r.returncode}"
    except Exception as e:  # noqa: BLE001
        return f"notify 指令跑不起來（{type(e).__name__}）"


# ------------------------------------------------------------------ 寫事件
def tick(run: Path, tr: Transcripts | None, stall_seconds: int, cfg: dict | None = None, now: datetime.datetime | None = None,
         at: str = "", runner=subprocess.run) -> tuple[dict | None, list[dict], list[str]]:
    """看一次：（快照, 這次新增的事件, 警告）。事件附加到 <run>/progress.jsonl，記憶存在 <run>/.watch-state.json。"""
    snap = snapshot(run, tr, now)
    if snap is None:
        return None, [], [f"讀不到 {run / 'run.json'}"]
    mem = load_json(run / STATE)
    mem = mem if isinstance(mem, dict) and mem.get("v") == 1 else {"v": 1, "seq": 0, "mem": {}}
    events, new_mem = diff_events(mem.get("mem") or {}, snap, stall_seconds)
    warns: list[str] = []
    stamp = at or now_iso()
    lines = []
    for e in events:
        mem["seq"] = int(mem.get("seq") or 0) + 1
        e2 = {"v": 1, "seq": mem["seq"], "at": stamp, "run": snap["run"], **e}
        lines.append(e2)
    if lines:
        try:
            with open(run / PROGRESS, "a", encoding="utf-8", newline="\n") as fh:
                for e2 in lines:
                    fh.write(json.dumps(e2, ensure_ascii=False) + "\n")
        except OSError as e:
            return snap, [], [f"寫不出 {PROGRESS}（{e}）"]
    mem["mem"] = new_mem
    try:
        (run / STATE).write_text(json.dumps(mem, ensure_ascii=False), encoding="utf-8")
    except OSError as e:
        warns.append(f"寫不出 {STATE}（{e}）；下次可能重複記事件")
    for e2 in lines:
        w = notify(cfg, e2, runner)
        if w and w not in warns:
            warns.append(w)
    return snap, lines, warns


def text_of(snap: dict) -> str:
    L = [f"{snap['run']}：{snap['state']}" + (f"；等 {snap['waiting_gate']} 段核准" if snap.get("waiting_gate") else "") + (f"；下一段 {snap['next']}" if snap.get("next") else "")]
    for s in snap["stages"]:
        if s["status"] in ("pending", "skipped"):
            continue
        ap = f"，{s['approved']['by']} 核准" if s["approved"] else ("，等核准" if s["gate"] and s["status"] == "done" else "")
        L.append(f"  {s['name']}：{s['status']}{ap}；agent {s['agents'] - s['agents_missing']}/{s['agents']}；紀錄 {len(s['records'])} 份")
    for w in snap.get("workflows") or []:
        idle = "" if w["finished"] or w.get("idle_seconds") is None else f"；{w['idle_seconds']} 秒沒有動靜"
        L.append(f"  workflow {w['workflow'] or w['wf_run']}：{w['status'] or '執行中'}；{len(w['agents'])} 個 agent、{w['tokens']:,} token{idle}")
    if snap.get("note"):
        L.append(f"  注意：{snap['note']}")
    return "\n".join(L)


def status_line(project: Path) -> str:
    """一行狀態：最近動過的 run。任何問題都回空字串（狀態列不能壞）。"""
    try:
        cfg = load_json(project / ".claude" / "teams" / "guard.config.json") or {}
        runs = project / str(cfg.get("runs_dir") or "_tpm/runs")
        cands = [p for p in runs.glob("*/run.json") if p.is_file()]
        if not cands:
            return ""
        run = max(cands, key=lambda p: p.stat().st_mtime).parent
        v = run_view(run)
        if v is None or v["state"] in ("closed", "abandoned"):
            return ""
        cur = v.get("waiting_gate") or (v.get("pending") or {}).get("stage") or v.get("next") or ""
        st = next((s for s in v["stages"] if s["name"] == cur), None)
        part = f" · {st['agents'] - st['agents_missing']}/{st['agents']}" if st and st["agents"] else ""
        tail = " · 等核准" if v.get("waiting_gate") else (" · 停下了" if v["state"] == "blocked" else "")
        return f"{run.name} · {cur or v['state']}{part}{tail}"
    except Exception:  # noqa: BLE001
        return ""


def main(argv: list[str], project: Path | None = None) -> int:
    if "--selftest" in argv:
        return selftest()
    project = project or Path.cwd()
    if argv and argv[0] == "statusline":
        line = status_line(project)
        if line:
            print(line)
        return 0
    run_arg = opt(argv, "--run")
    if not run_arg:
        print(__doc__)
        return 1
    run = Path(run_arg) if Path(run_arg).is_absolute() else project / run_arg
    try:
        stall = max(0, int(opt(argv, "--stall-seconds", "900")))
        interval = max(1, int(opt(argv, "--interval", "5")))
    except ValueError:
        print("--stall-seconds 與 --interval 要是整數", file=sys.stderr)
        return 1
    try:
        root = Path(os.environ["AWF_TRANSCRIPTS_DIR"]) if os.environ.get("AWF_TRANSCRIPTS_DIR") else awf_cost.project_dir(str(project))
    except Exception:  # noqa: BLE001
        root = None
    tr = Transcripts(root, run)
    prof, _ = sr.load_profile_for(run, [])
    cfg = notify_config(prof)
    once = "--once" in argv
    try:
        while True:
            snap, events, warns = tick(run, tr, stall, cfg)
            for w in warns:
                print(f"警告：{w}", file=sys.stderr)
            if snap is None:
                return 1
            if once:
                print(json.dumps(snap, ensure_ascii=False, indent=2) if "--json" in argv else text_of(snap))
                return 0
            for e in events:
                print(json.dumps(e, ensure_ascii=False), flush=True)
            if snap["state"] in ("closed", "abandoned"):
                return 0
            time.sleep(interval)
    except KeyboardInterrupt:
        return 0


# ------------------------------------------------------------------ selftest
def selftest() -> int:
    import tempfile
    import schemalib
    ok = True

    def check(name: str, cond: bool, detail="") -> None:
        nonlocal ok
        ok = ok and bool(cond)
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{detail}"))

    print("awf_watch selftest（暫存目錄，不碰 repo）")
    schema = json.loads((HERE.parent / "schemas" / "progress-event.schema.json").read_text(encoding="utf-8"))
    T0 = datetime.datetime(2026, 1, 1, 0, 0, 0, tzinfo=datetime.timezone.utc)
    ts = lambda sec: (T0 + datetime.timedelta(seconds=sec)).strftime("%Y-%m-%dT%H:%M:%S.000Z")  # noqa: E731
    with tempfile.TemporaryDirectory(prefix="awf-watch.") as tmp:
        root = Path(tmp)
        run = root / "proj" / "_runs" / "20260101-sdlc-demo"
        run.mkdir(parents=True)
        trd = root / "transcripts"
        wf = trd / "sess-1" / "subagents" / "workflows" / "wf_a1"
        wf.mkdir(parents=True)
        (trd / "sess-1" / "workflows").mkdir(parents=True)

        def write_run(stages: dict, **extra) -> None:
            (run / "run.json").write_text(json.dumps({"event": "sdlc", "requirement": "機密需求內容", "stages": stages, **extra}, ensure_ascii=False), encoding="utf-8")

        def agent(aid: str, role: str, label: str) -> None:
            (wf / f"agent-{aid}.jsonl").write_text(json.dumps({"timestamp": ts(0), "message": {"role": "user", "content": f"run 目錄：{run.as_posix()}"}}) + "\n", encoding="utf-8")
            (wf / f"agent-{aid}.meta.json").write_text(json.dumps({"agentType": f"agent-workflow:{role}", "description": label, "workflowPhase": "Research"}), encoding="utf-8")

        def more(aid: str, rid: str, sec: int, tok: int, model: str = "claude-sonnet-5-5") -> None:   # transcript 只會往後附加
            with (wf / f"agent-{aid}.jsonl").open("a", encoding="utf-8") as fh:
                fh.write(json.dumps({"timestamp": ts(sec), "requestId": rid, "message": {"model": model, "usage": {"input_tokens": tok, "output_tokens": 0}}}) + "\n")

        def meta(status: str) -> None:
            (trd / "sess-1" / "workflows" / "wf_a1.json").write_text(json.dumps({"runId": "wf_a1", "scriptPath": str(run / ".wf" / "sdlc-plan.js"), "status": status, "workflowName": "sdlc-plan"}), encoding="utf-8")

        P = lambda **kw: {"status": "pending", "gate": True, **kw}  # noqa: E731
        tr = Transcripts(trd, run)
        seq_all: list[dict] = []

        def step(sec: int, **kw) -> list[str]:
            snap, ev, warns = tick(run, tr, 600, now=T0 + datetime.timedelta(seconds=sec), at=ts(sec), **kw)
            seq_all.extend(ev)
            return [e["type"] + (":" + str(e.get("label") or e.get("stage") or e.get("state") or "")) for e in ev]

        write_run({"plan": P(), "build": P(gate=False)}, awf={"pending": {"stage": "plan", "workflow": "sdlc-plan"}})
        got = step(1)
        check("事件：run 開始、plan 段開跑", got == ["run_state:running", "stage_started:plan"], got)
        check("事件：沒有變化就不重複記", step(2) == [])
        meta("running")
        agent("r1", "tpm-researcher", "research:code")
        more("r1", "q1", 10, 30_000)
        got = step(20)
        check("事件：transcript 出現 agent → agent_started，帶角色、模型與 token", got == ["agent_started:research:code"] and seq_all[-1]["role"] == "tpm-researcher"
              and seq_all[-1]["model"] == "claude-sonnet-5-5" and seq_all[-1]["tokens"] == 30_000, (got, seq_all[-1]))
        more("r1", "q2", 30, 40_000)
        check("事件：token 增加不到門檻不記 agent_progress", step(40) == [])
        more("r1", "q3", 50, 60_000)
        more("r1", "q3", 50, 60_000)
        got = step(60)
        check("事件：token 超過門檻記一筆 agent_progress（同一個 requestId 不重複算）", got == ["agent_progress:research:code"] and seq_all[-1]["tokens"] == 130_000, (got, seq_all[-1]))
        (run / "plan").mkdir()
        (run / "plan" / "research-code.md").write_text("# 紀錄\n", encoding="utf-8")
        (run / "plan" / sr.INDEX_NAME).write_text("# 索引\n", encoding="utf-8")
        (run / "plan" / "pack.md").write_text("# 資料包\n", encoding="utf-8")
        got = step(70)
        check("事件：紀錄檔出現 → record_written（索引與資料包不算）", got == ["record_written:research-code"], got)
        got = step(50 + 599)
        check("停住：還沒超過門檻不記", got == [], got)
        got = step(50 + 601)
        check("停住：workflow 沒結束而且超過門檻沒有動靜 → workflow_stalled（只記一次）", got == ["workflow_stalled:"] and seq_all[-1]["idle_seconds"] == 601 and step(50 + 700) == [], (got, seq_all[-1]))
        more("r1", "q4", 900, 1_000)
        step(905)
        got = step(900 + 601)
        check("停住：恢復動靜之後又停住，會再記一次", got == ["workflow_stalled:"], got)
        meta("completed")
        write_run({"plan": P(status="done"), "build": P(gate=False)})
        got = step(1600)
        check("事件：workflow 結束、段存檔、等核准", got == ["run_state:waiting_gate", "stage_saved:plan", "waiting_gate:plan", "workflow_finished:"], got)
        check("停住：workflow 結束之後不再記 stalled", step(9000) == [])
        write_run({"plan": P(status="done", approved={"by": "human", "at": "t", "kind": "human"}), "build": P(gate=False)})
        got = step(9100)
        check("事件：核准", got == ["run_state:running", "stage_approved:plan"] and seq_all[-1]["by"] == "human", got)
        lines = [json.loads(x) for x in (run / PROGRESS).read_text(encoding="utf-8").splitlines()]
        check("progress.jsonl：一行一個事件、seq 連續、每筆都符合 schema", [e["seq"] for e in lines] == list(range(1, len(lines) + 1)) and len(lines) == len(seq_all)
              and all(not schemalib.validate(e, schema) for e in lines), [schemalib.validate(e, schema) for e in lines if schemalib.validate(e, schema)][:2])
        check("progress.jsonl：沒有需求內容、摘要或路徑", "機密需求內容" not in (run / PROGRESS).read_text(encoding="utf-8") and run.as_posix() not in (run / PROGRESS).read_text(encoding="utf-8"))
        tr2 = Transcripts(trd, run)
        snap, ev, _ = tick(run, tr2, 600, now=T0 + datetime.timedelta(seconds=9200), at=ts(9200))
        check("重啟 watcher：記憶在 .watch-state.json，不會把舊事件再記一次", ev == [] and snap["workflows"][0]["tokens"] == 131_000, ev)
        check("快照：有狀態、各段、紀錄與 workflow 的 agent", snap["state"] == "running" and snap["stages"][0]["records"] == ["research-code"]
              and snap["workflows"][0]["agents"][0]["role"] == "tpm-researcher" and "research:code" in text_of(snap) + json.dumps(snap, ensure_ascii=False), snap)

        # transcript 讀不懂、不存在
        (wf / "agent-bad.jsonl").write_text("這不是 JSON\n{\"message\": 3}\n[1,2]\n", encoding="utf-8")
        snap, ev, warns = tick(run, Transcripts(trd, run), 600, now=T0, at=ts(0))
        check("transcript 有讀不懂的行：不中斷，那個 agent 只是沒有 token", snap is not None and any(a["id"] == "bad" and a["tokens"] == 0 for a in snap["workflows"][0]["agents"]), snap)
        snap, ev, warns = tick(run, Transcripts(root / "nope", run), 600, now=T0, at=ts(0))
        check("找不到 transcript 目錄：照常出 run 的狀態，workflows 是空的", snap is not None and snap["workflows"] == [] and snap["state"] == "running")
        other = trd / "sess-1" / "subagents" / "workflows" / "wf_other"
        other.mkdir()
        (other / "agent-x.jsonl").write_text(json.dumps({"timestamp": ts(0), "message": {"role": "user", "content": "別的 run：/somewhere/else"}}) + "\n", encoding="utf-8")
        snap = snapshot(run, Transcripts(trd, run), T0)
        check("別的 run 的 workflow 不算進來", [w["wf_run"] for w in snap["workflows"]] == ["wf_a1"], snap["workflows"])
        check("run.json 讀不到：回錯誤、不寫任何檔", tick(root / "nope", None, 600)[0] is None and not (root / "nope").exists())

        # notify
        check("notify：profile 沒設就不呼叫（預設關）", notify_config(None) is None and notify_config({"progress": {}}) is None and notify(None, lines[0]) == "")
        sent: list = []

        class R:
            returncode = 0

        def fake(argv, input="", **kw):  # noqa: A002
            sent.append((argv, json.loads(input)))
            return R()
        cfg = notify_config({"progress": {"notify": {"command": ["notify-cmd", "--x"], "events": ["workflow_stalled", "waiting_gate", "nope"]}}})
        for e in lines:
            notify(cfg, {**e, "requirement": "機密需求內容", "summary": "內部摘要", "path": "/secret/path"}, fake)
        check("notify：只送指定的事件類型；不認得的類型忽略", cfg["events"] == ["workflow_stalled", "waiting_gate"] and {p["type"] for _a, p in sent} == {"workflow_stalled", "waiting_gate"} and sent[0][0] == ["notify-cmd", "--x"], sent[:2])
        check("notify：只送白名單欄位（需求、摘要、路徑都不送）", all(set(p) <= set(NOTIFY_FIELDS) for _a, p in sent) and "機密" not in json.dumps(sent, ensure_ascii=False) and "/secret" not in json.dumps(sent))
        n0 = len(sent)
        w = notify(cfg, {"type": "waiting_gate", "v": 1, "seq": 1, "at": "t", "run": "r", "stage": "plan", "label": "token=ghp_" + "a" * 30}, fake)
        check("notify：內容像機敏值就整筆不送，並回警告", len(sent) == n0 and "沒有送出" in w, w)

        def boom(*a, **kw):
            raise OSError("no such command")
        check("notify：指令跑不起來只回警告（不丟例外）", "跑不起來" in notify(cfg, lines[0] | {"type": "waiting_gate"}, boom))
        check("notify：command 可以寫字串", notify_config({"progress": {"notify": {"command": "notify-cmd --x"}}})["argv"] == ["notify-cmd", "--x"])

        # statusline
        proj = root / "proj"
        (proj / ".claude" / "teams").mkdir(parents=True)
        (proj / ".claude" / "teams" / "guard.config.json").write_text(json.dumps({"runs_dir": "_runs"}), encoding="utf-8")
        write_run({"plan": P(status="done"), "build": P(gate=False)})
        check("statusline：一行、有 run 名與在等的段", status_line(proj) == "20260101-sdlc-demo · plan · 等核准", status_line(proj))
        check("statusline：沒有 run、設定壞掉都回空字串（不丟例外）", status_line(root / "nope") == "" and status_line(root) == "")
    print("awf_watch selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
