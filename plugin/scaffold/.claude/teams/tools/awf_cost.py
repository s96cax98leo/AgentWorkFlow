#!/usr/bin/env python3
"""workflow 的成本與時間：讀 Claude Code 存的 workflow transcript，依 workflow、角色、模型加總 token。

為什麼要有：使用者問「這段花了多少 token 與時間」時只能手算；要省 token 也得先量得到。

資料來源（Claude Code 的內部格式，官方沒有承諾穩定，所以解析集中在 read_agent／read_wf_meta 兩個轉接函式）：
  <session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.jsonl      每筆 assistant 訊息有 message.usage、message.model
  <session 目錄>/subagents/workflows/wf_<id>/agent-<agentId>.meta.json  agentType、description（＝label）、workflowPhase、model
  <session 目錄>/workflows/wf_<id>.json                                 workflowName、status、durationMs、scriptPath、agentCount
同一個請求的 usage 會重複出現在多行，依 requestId 去重（沒有就用 message.id）。

用法：
  python3 awf_cost.py <路徑>… [--run-dir <run 目錄>] [--json]
      <路徑> 可以是 wf_<id> 目錄、session 目錄，或 ~/.claude/projects/<專案> 目錄（往下找所有 wf_*）。
      --run-dir：只算派工單提到這個 run 目錄的 workflow（派工單的共同前綴含 run_dir）。
  python3 awf_cost.py --project <專案根目錄> [--run-dir …] [--json]
      由專案路徑推算 ~/.claude/projects/ 底下的目錄。
  python3 awf_cost.py --selftest

讀不懂格式（找不到任何 usage）時明說並 exit 0：這支工具只做估算，不參與任何閘門判定。只用標準庫。
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
from datetime import datetime
from pathlib import Path

KINDS = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")
SHORT = {"input_tokens": "input", "cache_creation_input_tokens": "cache_write", "cache_read_input_tokens": "cache_read",
         "output_tokens": "output"}
SCAN_PROMPT_BYTES = 200_000  # 比對 run_dir 時每個 agent 最多掃的開頭位元組


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def project_dir(root: str) -> Path:
    """Claude Code 把專案路徑裡不是英數的字元都換成 '-' 當目錄名。"""
    return Path.home() / ".claude" / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(Path(root).expanduser().resolve()))


def parse_ts(s) -> datetime | None:
    try:
        return datetime.fromisoformat(str(s).replace("Z", "+00:00"))
    except (ValueError, TypeError):
        return None


def read_agent(path: Path) -> dict:
    """轉接函式：一個 agent 的 transcript → {tokens, requests, models, first, last, head}。格式變了只改這裡。"""
    tokens = dict.fromkeys(KINDS, 0)
    seen: set = set()
    models: dict[str, int] = {}
    first = last = None
    head = ""
    try:
        with path.open(encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if len(head) < SCAN_PROMPT_BYTES:
                    head += line[: SCAN_PROMPT_BYTES - len(head)]
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(e, dict):
                    continue
                t = parse_ts(e.get("timestamp"))
                if t:
                    first = t if first is None or t < first else first
                    last = t if last is None or t > last else last
                m = e.get("message")
                if not isinstance(m, dict) or not isinstance(m.get("usage"), dict):
                    continue
                rid = e.get("requestId") or m.get("id")
                if rid is not None:
                    if rid in seen:
                        continue
                    seen.add(rid)
                for k in KINDS:
                    v = m["usage"].get(k)
                    tokens[k] += v if isinstance(v, int) else 0
                if isinstance(m.get("model"), str):
                    models[m["model"]] = models.get(m["model"], 0) + 1
    except OSError:
        pass
    return {"tokens": tokens, "requests": sum(models.values()) or len(seen), "models": models, "first": first, "last": last,
            "head": head}


def tail_agent(path: Path, st: dict | None = None) -> dict:
    """轉接函式（增量版，watcher 用）：從上次讀到的位置往後讀，累加到 st 並回傳它。
    st：{offset, tokens, seen, requests, model, first, last}；第一次給 None。只讀完整的行（最後一行還沒寫完就留到下次）。"""
    st = st if isinstance(st, dict) else {"offset": 0, "tokens": 0, "seen": set(), "requests": 0, "model": "", "first": None, "last": None}
    try:
        with path.open("rb") as fh:
            fh.seek(st["offset"])
            chunk = fh.read()
    except OSError:
        return st
    end = chunk.rfind(b"\n")
    if end < 0:
        return st
    st["offset"] += end + 1
    for raw in chunk[: end + 1].splitlines():
        try:
            e = json.loads(raw.decode("utf-8", errors="replace"))
        except ValueError:
            continue
        if not isinstance(e, dict):
            continue
        t = parse_ts(e.get("timestamp"))
        if t:
            st["first"] = t if st["first"] is None or t < st["first"] else st["first"]
            st["last"] = t if st["last"] is None or t > st["last"] else st["last"]
        m = e.get("message")
        if not isinstance(m, dict) or not isinstance(m.get("usage"), dict):
            continue
        rid = e.get("requestId") or m.get("id")
        if rid is not None:
            if rid in st["seen"]:
                continue
            st["seen"].add(rid)
        st["requests"] += 1
        st["tokens"] += sum(v for k in KINDS if isinstance(v := m["usage"].get(k), int))
        if isinstance(m.get("model"), str):
            st["model"] = m["model"]
    return st


def read_meta(path: Path) -> dict:
    try:
        d = json.loads(path.read_text(encoding="utf-8"))
        return d if isinstance(d, dict) else {}
    except (OSError, ValueError):
        return {}


def read_wf_meta(wf_dir: Path) -> dict:
    """轉接函式：<session>/workflows/wf_<id>.json（wf_dir 是 <session>/subagents/workflows/wf_<id>）。"""
    return read_meta(wf_dir.parents[2] / "workflows" / (wf_dir.name + ".json"))


def find_wf_dirs(paths: list[Path]) -> list[Path]:
    out: list[Path] = []
    for p in paths:
        if p.name.startswith("wf_") and p.is_dir():
            out.append(p)
        elif p.is_dir():
            out.extend(sorted(d for d in p.glob("**/subagents/workflows/wf_*") if d.is_dir()))
    seen: set = set()
    return [d for d in out if not (d in seen or seen.add(d))]


def summarize(wf_dir: Path, run_dir: str = "") -> dict | None:
    """一個 workflow run 的統計；run_dir 有給而且沒有任何派工單提到它就回 None。"""
    agents = []
    mentions = not run_dir
    needles = [run_dir, json.dumps(run_dir, ensure_ascii=False)[1:-1], json.dumps(run_dir)[1:-1]] if run_dir else []
    for f in sorted(wf_dir.glob("agent-*.jsonl")):
        a = read_agent(f)
        meta = read_meta(f.with_name(f.name[: -len(".jsonl")] + ".meta.json"))
        if not mentions and any(n in a["head"] for n in needles):
            mentions = True
        agents.append({"id": f.stem[len("agent-"):], "role": str(meta.get("agentType") or "?").split(":")[-1],
                       "label": meta.get("description") or "", "phase": meta.get("workflowPhase") or "",
                       "model": next(iter(sorted(a["models"], key=a["models"].get, reverse=True)), meta.get("model") or "?"),
                       "tokens": a["tokens"], "requests": a["requests"], "first": a["first"], "last": a["last"]})
    wf = read_wf_meta(wf_dir)
    if not mentions and run_dir and run_dir in str(wf.get("scriptPath") or ""):
        mentions = True
    if not mentions or not agents:
        return None
    total = {k: sum(a["tokens"][k] for a in agents) for k in KINDS}
    reqs = sum(a["requests"] for a in agents)
    firsts = [a["first"] for a in agents if a["first"]]
    lasts = [a["last"] for a in agents if a["last"]]

    def group(key: str) -> list[dict]:
        g: dict[str, dict] = {}
        for a in agents:
            r = g.setdefault(a[key], {"name": a[key], "agents": 0, "tokens": 0, "requests": 0})
            r["agents"] += 1
            r["tokens"] += sum(a["tokens"].values())
            r["requests"] += a["requests"]
        return sorted(g.values(), key=lambda r: -r["tokens"])

    ctx = total["input_tokens"] + total["cache_creation_input_tokens"] + total["cache_read_input_tokens"]
    return {"run_id": wf_dir.name, "workflow": wf.get("workflowName") or "", "status": wf.get("status") or "",
            "start": min(firsts).isoformat() if firsts else "", "end": max(lasts).isoformat() if lasts else "",
            "minutes": round((max(lasts) - min(firsts)).total_seconds() / 60, 1) if firsts and lasts else None,
            "agents": len(agents), "requests": reqs, "tokens": {SHORT[k]: total[k] for k in KINDS},
            "total_tokens": sum(total.values()), "avg_context": round(ctx / reqs) if reqs else 0,
            "by_role": group("role"), "by_model": group("model"),
            "agent_list": [{"id": a["id"], "role": a["role"], "label": a["label"], "phase": a["phase"], "model": a["model"],
                            "tokens": sum(a["tokens"].values()), "requests": a["requests"]} for a in agents]}


def fmt(n: int) -> str:
    return f"{n / 1e6:.2f}M" if n >= 1e6 else (f"{n / 1e3:.0f}k" if n >= 1e3 else str(n))


def render(rows: list[dict]) -> str:
    out = []
    for r in rows:
        t = r["tokens"]
        out.append(f"{r['run_id']}　{r['workflow'] or '（名稱未知）'}　{r['status'] or ''}　開始 {r['start'][:16].replace('T', ' ')}")
        out.append(f"  時長 {r['minutes']} 分｜agent {r['agents']} 個｜請求 {r['requests']} 次｜總 token {fmt(r['total_tokens'])}"
                   f"（cache 讀取 {fmt(t['cache_read'])}、cache 寫入 {fmt(t['cache_write'])}、input {fmt(t['input'])}、"
                   f"output {fmt(t['output'])}）｜每次請求平均 context {fmt(r['avg_context'])}")
        out.append("  依角色：" + "、".join(f"{g['name']}×{g['agents']}＝{fmt(g['tokens'])}" for g in r["by_role"]))
        out.append("  依模型：" + "、".join(f"{g['name']}＝{fmt(g['tokens'])}（{g['requests']} 次）" for g in r["by_model"]))
    if len(rows) > 1:
        out.append(f"合計：{len(rows)} 次 workflow，總 token {fmt(sum(r['total_tokens'] for r in rows))}，"
                   f"時長 {round(sum(r['minutes'] or 0 for r in rows), 1)} 分")
    out.append("（估算：依 Claude Code 的 transcript 加總，格式沒有官方承諾；不參與任何閘門判定）")
    return "\n".join(out)


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    run_dir = opt(argv, "--run-dir")
    paths = [Path(a).expanduser() for i, a in enumerate(argv)
             if not a.startswith("--") and (i == 0 or argv[i - 1] not in ("--run-dir", "--project"))]
    if "--project" in argv:
        paths.append(project_dir(opt(argv, "--project")))
    if not paths:
        print(__doc__)
        return 2
    rows = [r for r in (summarize(d, run_dir) for d in find_wf_dirs(paths)) if r]
    rows.sort(key=lambda r: r["start"])
    usable = [r for r in rows if r["requests"]]
    if "--json" in argv:
        print(json.dumps({"ok": bool(usable), "runs": rows}, ensure_ascii=False, indent=1))
        return 0
    if not rows:
        print("找不到 workflow transcript（路徑底下沒有 subagents/workflows/wf_*，或沒有派工單提到指定的 run 目錄）")
        return 0
    if not usable:
        print("這版 transcript 格式讀不了：找到 workflow 目錄，但沒有任何一筆 usage。Claude Code 的內部格式可能改了，"
              "請回報版本；這不影響流程。")
        return 0
    print(render(rows))
    return 0


def selftest() -> int:
    ok = True

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        ok = ok and bool(cond)
        print(f"  [{'通過' if cond else '失敗'}] {name}" + (f"：{detail}" if not cond and detail else ""))

    def line(rid, model, u, ts, extra=None):
        d = {"timestamp": ts, "requestId": rid, "message": {"model": model, "usage": u}}
        d.update(extra or {})
        return json.dumps(d, ensure_ascii=False)

    with tempfile.TemporaryDirectory() as td:
        sess = Path(td) / "proj" / "sess1"
        wf = sess / "subagents" / "workflows" / "wf_abc"
        wf.mkdir(parents=True)
        (sess / "workflows").mkdir()
        (sess / "workflows" / "wf_abc.json").write_text(json.dumps(
            {"workflowName": "sdlc-plan", "status": "completed", "scriptPath": "/x/sdlc-plan.js"}), encoding="utf-8")
        u1 = {"input_tokens": 10, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 5}
        u2 = {"input_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 3000, "output_tokens": 15}
        prompt = json.dumps({"timestamp": "2026-01-01T00:00:00Z", "type": "user",
                             "message": {"content": "共同規則…背景在 _tpm/runs/示範-r1/context.md"}}, ensure_ascii=False)
        (wf / "agent-a1.jsonl").write_text("\n".join([
            prompt, line("r1", "claude-opus-x", u1, "2026-01-01T00:01:00Z"), line("r1", "claude-opus-x", u1, "2026-01-01T00:01:01Z"),
            "不是 JSON 的一行", line("r2", "claude-opus-x", u2, "2026-01-01T00:11:00Z")]) + "\n", encoding="utf-8")
        (wf / "agent-a1.meta.json").write_text(json.dumps(
            {"agentType": "agent-workflow:tpm-researcher", "description": "research:code", "workflowPhase": "Research"}),
            encoding="utf-8")
        (wf / "agent-a2.jsonl").write_text(line("r3", "claude-haiku-x", u1, "2026-01-01T00:05:00Z") + "\n", encoding="utf-8")
        (wf / "agent-a2.meta.json").write_text("壞掉的 meta", encoding="utf-8")
        other = sess / "subagents" / "workflows" / "wf_other"
        other.mkdir()
        (other / "agent-b1.jsonl").write_text(line("r9", "claude-opus-x", u1, "2026-01-02T00:00:00Z") + "\n", encoding="utf-8")
        empty = sess / "subagents" / "workflows" / "wf_empty"
        empty.mkdir()
        (empty / "agent-c1.jsonl").write_text(json.dumps({"timestamp": "2026-01-03T00:00:00Z", "message": {"content": "x"}}) + "\n",
                                              encoding="utf-8")

        r = summarize(wf)
        check("依 requestId 去重後加總（重複的 r1 只算一次）",
              r and r["tokens"] == {"input": 40, "cache_write": 200, "cache_read": 5000, "output": 25} and r["requests"] == 3, str(r))
        check("時長取最早與最晚的 timestamp（含派工單那一行）", r and r["minutes"] == 11.0, str(r and r["minutes"]))
        check("每次請求平均 context＝(input＋cache 讀寫)／請求數", r and r["avg_context"] == round(5240 / 3), str(r and r["avg_context"]))
        roles = {g["name"]: g for g in r["by_role"]}
        check("角色取自 meta 的 agentType（去掉命名空間）；meta 壞掉的記成 ?",
              roles.get("tpm-researcher", {}).get("tokens") == 4150 and roles.get("?", {}).get("tokens") == 1115, str(roles))
        check("依模型分組", {g["name"]: g["tokens"] for g in r["by_model"]} == {"claude-opus-x": 4150, "claude-haiku-x": 1115},
              str(r["by_model"]))
        check("workflow 名稱與狀態取自 workflows/wf_<id>.json", r["workflow"] == "sdlc-plan" and r["status"] == "completed")
        check("label 與 phase 取自 meta", r["agent_list"][0]["label"] == "research:code" and r["agent_list"][0]["phase"] == "Research")
        check("--run-dir：派工單提到 run 目錄才算（中文路徑）",
              summarize(wf, "_tpm/runs/示範-r1") is not None and summarize(other, "_tpm/runs/示範-r1") is None)
        check("從 session 目錄往下找到所有 wf_*", [d.name for d in find_wf_dirs([sess])] == ["wf_abc", "wf_empty", "wf_other"],
              str(find_wf_dirs([sess])))

        import contextlib
        import io
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = main([str(empty)])
        check("沒有任何 usage 時明說格式讀不了並 exit 0", code == 0 and "格式讀不了" in buf.getvalue(), buf.getvalue())
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = main([str(Path(td) / "nowhere")])
        check("路徑不存在時說明並 exit 0", code == 0 and "找不到" in buf.getvalue(), buf.getvalue())
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = main([str(sess), "--json"])
        j = json.loads(buf.getvalue())
        check("--json 輸出可解析，含每個 run 與合計需要的欄位",
              code == 0 and j["ok"] and {x["run_id"] for x in j["runs"]} == {"wf_abc", "wf_other", "wf_empty"}
              and all(k in j["runs"][0] for k in ("total_tokens", "minutes", "by_role", "by_model", "avg_context")), buf.getvalue()[:300])
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            main([str(sess)])
        check("人讀的輸出有估算聲明與合計", "估算" in buf.getvalue() and "合計：" in buf.getvalue(), buf.getvalue())
        check("專案路徑換成 projects 目錄名（非英數一律換成 -）",
              project_dir("/tmp/示範_專案 a").name.endswith("-------a") and "/" not in project_dir("/tmp/x").name)
    print("awf_cost selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
