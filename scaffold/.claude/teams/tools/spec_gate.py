#!/usr/bin/env python3
"""OpenSpec 段落閘門：用 OpenSpec CLI（1.15.x）的 --json 輸出，確定性判定某一段能不能核准。不呼叫 LLM。

只在開發規範 profile 設了 spec_engine=openspec 時使用；save_run.py approve 會在既有檢查之後自動呼叫它。

用法：
    python3 .claude/teams/tools/spec_gate.py <plan|design|build|test> --change <change id>
        [--cwd <OpenSpec 專案根，預設目前目錄>] [--subtasks <design.json 或 subtasks payload>]
        [--criteria <test.json 或 criteria payload／信封>] [--verify <opsx:verify 報告，預設 openspec/changes/<id>/awf/opsx-verify.md>]
        [--at <ISO 時間，預設現在>]

各段過關條件：
    plan    status 的 proposal artifact 為 done（這一段不跑 validate）
    design  status 的 isPlanningComplete=true；validate <id> --strict 的 item valid=true；
            instructions apply 的 state=ready，而且沒有 missing*／warnings
    build   instructions apply 的 progress.remaining=0；每個 task 的 description 都帶 [<子任務 id>]，
            其集合等於 subtasks payload 的 id 集合（--subtasks）
    test    verifier criteria 的 id 集合＝delta 展開的 scenarioKey 集合（openspec-reqs.mjs）；每條 result=pass；
            anti_cheat 為空；/opsx:verify 報告的 Final Assessment 寫「No critical issues」或「All checks passed」，
            沒有「N critical issue(s)」，也沒有略過的檢查（「in the checks that ran」）

輸出：stdout 一個 JSON {"engine":"openspec","cli_version","stage","change","pass","checks":[{"id","pass","detail"}],"at"}；
      stderr 只放診斷文字。結束碼：0＝過關；1＝不過關；2＝工具無法執行（找不到 CLI、版本不符、逾時、輸出不是 JSON）。
CLI 由環境變數 AWF_OPENSPEC 指定（JSON 陣列或單一路徑，預設 PATH 上的 openspec）；每次呼叫上限 AWF_SPEC_GATE_TIMEOUT 秒（預設 60）。
validate 失敗時 exit 1 但 stdout 仍是完整報告：一律以 stdout 的 JSON 判定，不把非 0 當成工具故障。
只用標準庫，Windows 可跑。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

STAGES = ("plan", "design", "build", "test")
VERSION_RE = re.compile(r"^1\.15\.\d+")
TAG_RE = re.compile(r"\[([A-Za-z0-9_-]+)\]")
TOOLS = Path(__file__).resolve().parent


class EngineError(Exception):
    """CLI 無法執行：code 是 SPEC_ENGINE_UNAVAILABLE／SPEC_ENGINE_VERSION／SPEC_ENGINE_TIMEOUT。"""

    def __init__(self, code: str, detail: str):
        super().__init__(detail)
        self.code, self.detail = code, detail


def cli_prefix() -> list[str]:
    raw = os.environ.get("AWF_OPENSPEC", "").strip()
    if raw.startswith("["):
        try:
            v = json.loads(raw)
        except json.JSONDecodeError as e:
            raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"AWF_OPENSPEC 不是合法的 JSON 陣列：{e}") from e
        if not (isinstance(v, list) and v and all(isinstance(x, str) for x in v)):
            raise EngineError("SPEC_ENGINE_UNAVAILABLE", "AWF_OPENSPEC 要是非空的字串陣列")
        return v
    exe = raw or shutil.which("openspec")
    if not exe:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", "找不到 openspec（PATH 上沒有，也沒設 AWF_OPENSPEC）")
    return [exe]


def timeout() -> float:
    try:
        return float(os.environ.get("AWF_SPEC_GATE_TIMEOUT") or 60)
    except ValueError:
        return 60.0


def env() -> dict:
    return {**os.environ, "NO_COLOR": "1", "OPENSPEC_NO_ANIMATION": "1", "OPENSPEC_TELEMETRY": "0", "PYTHONUTF8": "1"}


def call(argv: list[str], cwd: Path) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, cwd=cwd, env=env(), capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=timeout(), stdin=subprocess.DEVNULL)
    except FileNotFoundError as e:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"無法執行 {argv[0]}：{e}") from e
    except subprocess.TimeoutExpired as e:
        raise EngineError("SPEC_ENGINE_TIMEOUT", f"{' '.join(argv[-4:])} 超過 {timeout():g} 秒") from e
    except OSError as e:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"無法執行 {argv[0]}：{e}") from e


def openspec(args: list[str], cwd: Path) -> tuple[int, dict]:
    """跑一個 openspec 指令並解析 stdout 的 JSON（不看 exit code）；stdout 不是 JSON 物件才算工具故障。"""
    r = call([*cli_prefix(), *args], cwd)
    try:
        data = json.loads(r.stdout)
    except json.JSONDecodeError as e:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE",
                          f"openspec {' '.join(args)} 的輸出不是 JSON（exit {r.returncode}）：{(r.stderr or r.stdout).strip()[-300:]}") from e
    if not isinstance(data, dict):
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"openspec {' '.join(args)} 的輸出不是 JSON 物件")
    return r.returncode, data


def cli_version(cwd: Path) -> str:
    r = call([*cli_prefix(), "--version"], cwd)
    ver = (r.stdout or "").strip().splitlines()[-1].strip() if (r.stdout or "").strip() else ""
    if r.returncode or not ver:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"openspec --version 失敗（exit {r.returncode}）：{r.stderr.strip()[-200:]}")
    if not VERSION_RE.match(ver):
        raise EngineError("SPEC_ENGINE_VERSION", f"需要 1.15.x，實際 {ver}")
    return ver


def diagnostics(data: dict) -> list | None:
    """CLI 失敗時的診斷陣列（status 是陣列時）；成功時 status 是字串或不存在。"""
    st = data.get("status")
    return st if isinstance(st, list) and st else None


def chk(cid: str, ok: bool, detail=None) -> dict:
    return {"id": cid, "pass": bool(ok), "detail": detail if detail is not None else ("ok" if ok else "")}


def change_missing(data: dict) -> dict | None:
    diag = diagnostics(data)
    if diag:
        return chk("change_exists", False, {"diagnostics": diag})
    return None


def load_json(path: str | None, what: str):
    if not path:
        return None, f"沒有提供 {what}"
    try:
        return json.loads(Path(path).read_text(encoding="utf-8")), None
    except (OSError, json.JSONDecodeError) as e:
        return None, f"讀不到 {what} {path}：{e}"


def payloads(node, key: str):
    """在 workflow 回傳、信封或 payload 裡找出所有帶 key 陣列的 dict。"""
    if isinstance(node, dict):
        if isinstance(node.get(key), list):
            yield node
        for v in node.values():
            yield from payloads(v, key)
    elif isinstance(node, list):
        for v in node:
            yield from payloads(v, key)


# ------------------------------------------------------------------ 各段
def gate_plan(cid: str, cwd: Path, _a) -> list[dict]:
    _, st = openspec(["status", "--change", cid, "--json"], cwd)
    miss = change_missing(st)
    if miss:
        return [miss]
    arts = {a.get("id"): a.get("status") for a in st.get("artifacts") or [] if isinstance(a, dict)}
    return [chk("proposal_done", arts.get("proposal") == "done", {"proposal": arts.get("proposal")})]


def gate_design(cid: str, cwd: Path, _a) -> list[dict]:
    _, st = openspec(["status", "--change", cid, "--json"], cwd)
    miss = change_missing(st)
    if miss:
        return [miss]
    checks = [chk("planning_complete", st.get("isPlanningComplete") is True,
                  {"isPlanningComplete": st.get("isPlanningComplete"),
                   "artifacts": {a.get("id"): a.get("status") for a in st.get("artifacts") or [] if isinstance(a, dict)}})]
    _, va = openspec(["validate", cid, "--strict", "--json"], cwd)
    items = [i for i in va.get("items") or [] if isinstance(i, dict) and i.get("id") == cid]
    issues = [{k: i.get(k) for k in ("level", "path", "message", "line") if k in i}
              for it in items for i in it.get("issues") or [] if isinstance(i, dict)]
    ok = len(items) == 1 and items[0].get("valid") is True
    checks.append(chk("validate_strict", ok, {"valid": items[0].get("valid") if items else None, "issues": issues}
                      if items else {"error": "validate 的輸出沒有這個 change 的 item", "diagnostics": diagnostics(va)}))
    _, ap = openspec(["instructions", "apply", "--change", cid, "--json"], cwd)
    extra = {k: v for k, v in ap.items() if (k.startswith("missing") or k == "warnings") and v}
    checks.append(chk("apply_ready", ap.get("state") == "ready" and not extra and not diagnostics(ap),
                      {"state": ap.get("state"), **extra, **({"diagnostics": diagnostics(ap)} if diagnostics(ap) else {})}))
    return checks


def gate_build(cid: str, cwd: Path, a) -> list[dict]:
    _, ap = openspec(["instructions", "apply", "--change", cid, "--json"], cwd)
    miss = change_missing(ap)
    if miss:
        return [miss]
    prog = ap.get("progress") if isinstance(ap.get("progress"), dict) else {}
    tasks = [t for t in ap.get("tasks") or [] if isinstance(t, dict)]
    unchecked = [{"line": t.get("line"), "description": t.get("description")} for t in tasks if not t.get("done")]
    done_ok = prog.get("remaining") == 0 and bool(tasks) and not unchecked
    checks = [chk("tasks_all_done", done_ok, {"progress": prog, "unchecked": unchecked})]
    untagged = [{"line": t.get("line"), "description": t.get("description")} for t in tasks if not TAG_RE.search(str(t.get("description") or ""))]
    checks.append(chk("tasks_tagged", bool(tasks) and not untagged, {"untagged": untagged}))
    tagged = sorted({m.group(1) for t in tasks for m in [TAG_RE.search(str(t.get("description") or ""))] if m})
    data, err = load_json(a.subtasks, "subtasks payload（--subtasks）")
    found = list(payloads(data, "subtasks")) if data is not None else []
    if err or not found:
        checks.append(chk("subtasks_match", False, {"error": err or "找不到 subtasks payload"}))
    else:
        want = sorted({str(s.get("id")) for s in found[-1]["subtasks"] if isinstance(s, dict) and s.get("id")})
        checks.append(chk("subtasks_match", set(want) == set(tagged),
                          {"subtasks": want, "tasks": tagged, "missing": sorted(set(want) - set(tagged)), "extra": sorted(set(tagged) - set(want))}))
    return checks


def scenario_keys(cid: str, cwd: Path) -> tuple[list[str], str | None]:
    """回傳 (scenarioKey 清單, 錯誤)。change 不存在、slug 碰撞等 spec 本身的問題算不過關（exit 1），找不到 node 才算工具故障。"""
    node = shutil.which("node")
    if not node:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", "找不到 node（scenarioKey 由 openspec-reqs.mjs 計算）")
    r = call([node, str(TOOLS / "openspec-reqs.mjs"), cid], cwd)
    if r.returncode:
        return [], f"openspec-reqs.mjs exit {r.returncode}：{r.stderr.strip()[-300:]}"
    try:
        return [str(x["id"]) for x in json.loads(r.stdout)["requirements"]], None
    except (json.JSONDecodeError, KeyError, TypeError) as e:
        raise EngineError("SPEC_ENGINE_UNAVAILABLE", f"openspec-reqs.mjs 的輸出格式不對：{e}") from e


VERIFY_OK = re.compile(r"\bNo critical issues\b|\bAll checks passed\b", re.I)
VERIFY_CRIT = re.compile(r"\b[1-9]\d* critical issues?\b", re.I)
VERIFY_SKIP = re.compile(r"in the checks that ran|\bnot verified\b", re.I)


def gate_test(cid: str, cwd: Path, a) -> list[dict]:
    keys, kerr = scenario_keys(cid, cwd)
    if kerr:
        return [chk("scenario_keys", False, kerr)]
    data, err = load_json(a.criteria, "verifier 的 criteria（--criteria）")
    found = list(payloads(data, "criteria")) if data is not None else []
    if err or not found:
        detail = {"error": err or "找不到 criteria payload"}
        checks = [chk(c, False, detail) for c in ("criteria_match", "criteria_pass", "anti_cheat_empty")]
    else:
        crit = [c for p in found for c in p["criteria"] if isinstance(c, dict)]
        ids = [str(c.get("id")) for c in crit]
        dup = sorted({i for i in ids if ids.count(i) > 1})
        missing, extra = sorted(set(keys) - set(ids)), sorted(set(ids) - set(keys))
        checks = [chk("criteria_match", not missing and not extra and not dup,
                      {"expected": len(keys), "actual": len(ids), "missing": missing, "extra": extra, "duplicate": dup})]
        not_pass = [{"id": c.get("id"), "result": c.get("result")} for c in crit if c.get("result") != "pass"]
        checks.append(chk("criteria_pass", bool(crit) and not not_pass, {"not_pass": not_pass}))
        anti = [x for p in found for x in (p.get("anti_cheat") or [])]
        checks.append(chk("anti_cheat_empty", not anti, {"anti_cheat": anti}))
    vpath = Path(a.verify) if a.verify else cwd / "openspec" / "changes" / cid / "awf" / "opsx-verify.md"
    try:
        text = vpath.read_text(encoding="utf-8")
    except OSError as e:
        checks.append(chk("opsx_verify", False, {"error": f"讀不到 /opsx:verify 報告 {vpath.name}：{e.strerror}"}))
        return checks
    crit_m, ok_m, skip_m = VERIFY_CRIT.search(text), VERIFY_OK.search(text), VERIFY_SKIP.search(text)
    checks.append(chk("opsx_verify", bool(ok_m) and not crit_m and not skip_m,
                      {"no_critical": bool(ok_m), "critical": crit_m.group(0) if crit_m else None, "skipped": skip_m.group(0) if skip_m else None}))
    return checks


GATES = {"plan": gate_plan, "design": gate_design, "build": gate_build, "test": gate_test}


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    ap = argparse.ArgumentParser(description="OpenSpec 段落閘門")
    ap.add_argument("stage", choices=STAGES)
    ap.add_argument("--change", required=True)
    ap.add_argument("--cwd", default=".")
    ap.add_argument("--subtasks")
    ap.add_argument("--criteria")
    ap.add_argument("--verify")
    ap.add_argument("--at")
    a = ap.parse_args(argv)
    cwd = Path(a.cwd).resolve()
    at = a.at or datetime.now(timezone.utc).isoformat(timespec="seconds")
    out = {"engine": "openspec", "cli_version": None, "stage": a.stage, "change": a.change, "pass": False, "checks": [], "at": at}
    try:
        out["cli_version"] = cli_version(cwd)
        out["checks"] = GATES[a.stage](a.change, cwd, a)
    except EngineError as e:
        out["checks"] = [chk(e.code, False, e.detail)]
        print(f"spec_gate：{e.code}：{e.detail}", file=sys.stderr)
        print(json.dumps(out, ensure_ascii=False))
        return 2
    out["pass"] = bool(out["checks"]) and all(c["pass"] for c in out["checks"])
    failed = [c["id"] for c in out["checks"] if not c["pass"]]
    if failed:
        print(f"spec_gate {a.stage} 未過：{'、'.join(failed)}", file=sys.stderr)
    print(json.dumps(out, ensure_ascii=False))
    return 0 if out["pass"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
