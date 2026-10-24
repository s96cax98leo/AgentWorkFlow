"""把 workflow 的回傳存進 run 目錄，並記錄 SDLC 閘門（Workflow 腳本不能寫檔，由主 session 呼叫）。

run 目錄：{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>/
    run.json            事件、目標 repo、各段狀態、閘門核准紀錄
    <stage>.json        該段 workflow 的完整回傳（每個 agent 的信封）

用法：
    python3 save_run.py init <run_dir> --event sdlc --target <repo> --requirement <描述或路徑>
    python3 save_run.py stage <run_dir> <stage> < result.json     # 存回傳並逐一驗證信封
    python3 save_run.py approve <run_dir> <stage> --by <誰> --at <ISO 時間> [--note <備註>]
        只有狀態 done 的段能核准；SDLC 的前一段要先 done（有閘門的還要已核准）。
        --by policy 表示依專案政策自動核准（全自動流程用），--note 必須寫政策名稱；
        政策核准會讀 <stage>.json 的實際內容（policy_violations）：verifier 有不是 pass 的條目、反作弊有發現、
        任何 blocking 待決問題或 blocker／important 發現、回傳標了 halted，都不能政策核准，要交人。
        回傳物件帶 halted（流程中斷）時，stage 一律記成 blocked。
    python3 save_run.py status <run_dir>
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402

SDLC = ["plan", "design", "build", "test", "deliver", "merge"]
# merge 段不設人工閘門：合併與否由確定性的 merge_gate.py 判定（gate 紀錄寫進 merge-audit.jsonl，hook 憑它放行），
# 它前面的 deliver 段仍要核准。
GATED = {"plan", "design", "test", "deliver"}


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv else default


def load(run: Path) -> dict:
    return json.loads((run / "run.json").read_text(encoding="utf-8"))


def dump(run: Path, data: dict) -> None:
    (run / "run.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def envelopes(node):
    """在回傳結構裡找出所有信封（有 status＋payload 的 dict）。"""
    if isinstance(node, dict):
        if "status" in node and "payload" in node:
            yield node
        for v in node.values():
            yield from envelopes(v)
    elif isinstance(node, list):
        for v in node:
            yield from envelopes(v)


BLOCKING_SEVERITIES = {"blocker", "important"}


def policy_violations(stage: str, result) -> list[str]:
    """政策核准（--by policy）的確定性判定：讀該段 workflow 回傳的實際內容，回傳不能自動核准的理由（空清單＝可以）。
    人工核准不呼叫這個函式。之後 merge gate 也可以重用（傳入同樣結構的回傳即可）。

    - 回傳標了 halted（流程中斷）不行。
    - 任何信封的 open_questions 有 blocking=true 不行。
    - 任何 findings payload 有 blocker／important 等級的發現不行。
    - test 段：必須有 verifier 的 criteria payload；任何一條 result 不是 pass（fail、not_verifiable）或 anti_cheat 非空都不行。
    """
    errs: list[str] = []
    if isinstance(result, dict) and result.get("halted"):
        errs.append(f"回傳標了 halted：{result.get('halt_reason') or result.get('halted')}")
    envs = list(envelopes(result))
    for i, env in enumerate(envs):
        for q in env.get("open_questions") or []:
            if isinstance(q, dict) and q.get("blocking"):
                errs.append(f"信封 {i} 有 blocking 的待決問題：{q.get('question', '')[:80]}")
        payload = env.get("payload") if isinstance(env.get("payload"), dict) else {}
        for f in payload.get("findings") or []:
            if isinstance(f, dict) and f.get("severity") in BLOCKING_SEVERITIES:
                errs.append(f"信封 {i} 有 {f.get('severity')} 發現 {f.get('id', '')}：{f.get('claim', '')[:80]}")
    if stage == "test":
        verifiers = [e["payload"] for e in envs if isinstance(e.get("payload"), dict) and "criteria" in e["payload"]]
        if not verifiers:
            errs.append("test 段找不到 verifier 的 criteria 結果")
        for p in verifiers:
            crit = p.get("criteria") or []
            if not crit:
                errs.append("verifier 的 criteria 是空的")
            for c in crit:
                if not isinstance(c, dict) or c.get("result") != "pass":
                    cid = c.get("id", "?") if isinstance(c, dict) else "?"
                    res = c.get("result") if isinstance(c, dict) else c
                    errs.append(f"驗收條件 {cid} 結果是 {res}，不是 pass")
            if p.get("anti_cheat"):
                errs.append(f"反作弊有 {len(p['anti_cheat'])} 項發現")
    return errs


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    cmd, run = argv[0], Path(argv[1])
    if cmd == "init":
        run.mkdir(parents=True, exist_ok=True)
        if (run / "run.json").exists():
            print(f"已存在：{run / 'run.json'}")
            return 0
        event = opt(argv, "--event")
        dump(run, {
            "event": event,
            "target_repo": opt(argv, "--target"),
            "requirement": opt(argv, "--requirement"),
            "stages": {s: {"status": "pending", "gate": s in GATED} for s in (SDLC if event == "sdlc" else ["run"])},
        })
        print(f"已建立 {run / 'run.json'}")
        return 0
    data = load(run)
    if cmd == "stage":
        stage = argv[2]
        result = json.load(sys.stdin)
        (run / f"{stage}.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        errs = []
        envs = list(envelopes(result))
        for i, env in enumerate(envs):
            errs += [f"信封 {i}：{e}" for e in schemalib.check(env)]
        statuses = [e.get("status") for e in envs]
        st = data["stages"].setdefault(stage, {"status": "pending", "gate": stage in GATED})
        halted = result.get("halted") if isinstance(result, dict) else None
        st["status"] = "invalid" if errs else ("needs_decision" if "needs_decision" in statuses else "blocked" if ("blocked" in statuses or halted) else "done")
        st["envelopes"] = len(envs)
        if halted:
            st["halted"] = str(result.get("halt_reason") or halted)
            print(f"{stage}：workflow 中斷（halted）：{st['halted']}", file=sys.stderr)
        else:
            st.pop("halted", None)
        dump(run, data)
        for e in errs:
            print(e, file=sys.stderr)
        print(f"{stage}：{len(envs)} 份信封，狀態 {st['status']}" + ("（需要使用者核准才能進下一段）" if st.get("gate") and st["status"] == "done" else ""))
        return 1 if errs else 0
    if cmd == "approve":
        stage = argv[2]
        by, at, note = opt(argv, "--by"), opt(argv, "--at"), opt(argv, "--note")
        st = data["stages"].get(stage)
        errs = []
        if st is None:
            errs.append(f"沒有 {stage} 這一段")
        elif st["status"] != "done":
            errs.append(f"{stage} 狀態是 {st['status']}，只有 done 的段能核准")
        if not by or not at:
            errs.append("--by 與 --at 都必填")
        if by == "policy" and not note:
            errs.append("--by policy 時 --note 要寫依據的政策名稱")
        if by == "policy" and st is not None:
            f = run / f"{stage}.json"
            if not f.is_file():
                errs.append(f"找不到 {f.name}，政策核准要讀該段的實際結果")
            else:
                errs += [f"政策判定：{e}" for e in policy_violations(stage, json.loads(f.read_text(encoding="utf-8")))]
        if stage in SDLC and SDLC.index(stage) > 0:
            prev = SDLC[SDLC.index(stage) - 1]
            ps = data["stages"].get(prev, {})
            if ps.get("status") != "done":
                errs.append(f"前一段 {prev} 還沒 done")
            elif ps.get("gate") and not ps.get("approved"):
                errs.append(f"前一段 {prev} 還沒核准")
        if errs:
            for e in errs:
                print(f"不能核准：{e}", file=sys.stderr)
            return 1
        st["approved"] = {"by": by, "at": at, "note": note, "kind": "policy" if by == "policy" else "human"}
        dump(run, data)
        print(f"{stage} 已記錄核准：{by} @ {at}")
        return 0
    if cmd == "status":
        for name, st in data["stages"].items():
            gate = "閘門" if st.get("gate") else ""
            ok = f"核准 {st['approved']['by']}" if st.get("approved") else ""
            print(f"{name:8} {st['status']:15} {gate} {ok}")
        nxt = next((s for s in data["stages"] if data["stages"][s]["status"] != "done" or (data["stages"][s].get("gate") and not data["stages"][s].get("approved"))), None)
        print(f"下一步：{nxt or '全部完成'}")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
