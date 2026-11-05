"""把 workflow 的回傳存進 run 目錄，並記錄 SDLC 閘門（Workflow 腳本不能寫檔，由主 session 呼叫）。

run 目錄：{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>/
    run.json            事件、目標 repo、各段狀態、閘門核准紀錄、取消紀錄（abandoned）
    <stage>.json        該段 workflow 的完整回傳（每個 agent 的信封）

用法：
    python3 save_run.py init <run_dir> --event sdlc --target <repo> --requirement <描述或路徑>
    python3 save_run.py stage <run_dir> <stage> < result.json     # 存回傳並逐一驗證信封
        重存某一段會清掉該段的核准（舊核准對應的是舊結果，已失效，要重新核准）。
        一份信封都沒有（agent 缺件）記成 invalid，不能核准。
        回傳物件帶 halted（流程中斷）時，stage 一律記成 blocked。
    python3 save_run.py approve <run_dir> <stage> --by <誰> --at <ISO 時間> [--note <備註>]
        只有狀態 done 的段能核准；SDLC 前面的每一段都要 done，有閘門的都要已核准（整條閘門鏈，
        不只緊鄰的前一段：build、sync 沒有閘門，不能讓 test 跳過 design 的核准）。已取消的 run 不能核准。
        --by policy 表示依專案政策自動核准（全自動流程用），--note 必須寫政策名稱；plan 段一律由人核准，不能用 policy。
        政策核准會讀 <stage>.json 的實際內容（policy_violations）：verifier 有不是 pass 的條目、反作弊有發現、
        任何 blocking 待決問題或 blocker／important 發現、skeptic 的 verdicts 有 refuted、
        agent 缺件（回傳裡有 null；缺件不是「沒有發現」）、回傳標了 halted，都不能政策核准，要交人。
    python3 save_run.py abandon <run_dir> --reason <為什麼取消> --by <誰> --at <ISO 時間>
        需求取消：run.json 記 abandoned {by, at, reason}；之後任何一段都不能核准，publish.py 也拒絕推送。
    python3 save_run.py status <run_dir>
    python3 save_run.py --selftest       # 在暫存目錄跑，不碰 repo
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402

# sync（跟上 base）在 build 與 test 之間，沒有人工閘門：之後的 test 會重新驗證同步後的 head
SDLC = ["plan", "design", "build", "sync", "test", "deliver", "merge"]
# merge 段不設人工閘門：合併與否由確定性的 merge_gate.py 判定（gate 紀錄寫進 merge-audit.jsonl，hook 憑它放行），
# 它前面的 deliver 段仍要核准。
GATED = {"plan", "design", "test", "deliver"}
# plan 閘門一律由人核准（rules.md 規則 16），不接受政策核准
HUMAN_ONLY = {"plan"}
# workflow 回傳裡「值是 null 也正常」的欄位（不是 agent 的交件）；其他位置出現 null 一律當 agent 缺件
NULL_OK_KEYS = {"gate", "halt_reason", "reason", "problem", "next", "summary", "rest", "head_sha"}
NO_ENVELOPE = "沒有任何信封（agent 缺件）"


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def load(run: Path) -> dict:
    return json.loads((run / "run.json").read_text(encoding="utf-8"))


def dump(run: Path, data: dict) -> None:
    (run / "run.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def is_envelope(node) -> bool:
    return isinstance(node, dict) and "status" in node and "payload" in node


def envelopes(node):
    """在回傳結構裡找出所有信封（有 status＋payload 的 dict）。"""
    if isinstance(node, dict):
        if is_envelope(node):
            yield node
        for v in node.values():
            yield from envelopes(v)
    elif isinstance(node, list):
        for v in node:
            yield from envelopes(v)


def missing_agents(node, path: str = "$") -> list[str]:
    """回傳結構裡 agent 缺件的位置：信封之外出現的 null（例如 analysis: [null]、challenge: null）。
    信封內部的欄位由 schema 驗證，不在這裡看；NULL_OK_KEYS 是 workflow 自己填的非交件欄位。"""
    if node is None:
        return [path]
    if is_envelope(node):
        return []
    out: list[str] = []
    if isinstance(node, dict):
        for k, v in node.items():
            if v is None and k in NULL_OK_KEYS:
                continue
            out += missing_agents(v, f"{path}.{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            out += missing_agents(v, f"{path}[{i}]")
    return out


BLOCKING_SEVERITIES = {"blocker", "important"}


def policy_violations(stage: str, result) -> list[str]:
    """政策核准（--by policy）的確定性判定：讀該段 workflow 回傳的實際內容，回傳不能自動核准的理由（空清單＝可以）。
    人工核准不呼叫這個函式。publish.py 推送前也重跑這個函式（同一套判定）。

    - plan 段一律由人核准（rules.md 規則 16），政策核准一律不行。
    - 一份信封都沒有、或回傳裡有 null（agent 缺件）不行：缺件不是「沒有發現」（rules.md 規則 26）。
    - 回傳標了 halted（流程中斷）不行。
    - 任何信封的 open_questions 有 blocking=true 不行。
    - 任何 findings payload 有 blocker／important 等級的發現不行。
    - 任何 verdicts payload 有 refuted=true（skeptic 推翻了該段的產出）不行。
    - test 段：必須有 verifier 的 criteria payload；任何一條 result 不是 pass（fail、not_verifiable）或 anti_cheat 非空都不行。
    """
    errs: list[str] = []
    if stage in HUMAN_ONLY:
        errs.append(f"{stage} 段一律由人核准（rules.md 規則 16），不能政策核准")
    if isinstance(result, dict) and result.get("halted"):
        errs.append(f"回傳標了 halted：{result.get('halt_reason') or result.get('halted')}")
    envs = list(envelopes(result))
    if not envs:
        errs.append(NO_ENVELOPE)
    for p in missing_agents(result):
        errs.append(f"agent 缺件：{p} 是 null（缺件不是「沒有發現」）")
    for i, env in enumerate(envs):
        for q in env.get("open_questions") or []:
            if isinstance(q, dict) and q.get("blocking"):
                errs.append(f"信封 {i} 有 blocking 的待決問題：{str(q.get('question', ''))[:80]}")
        payload = env.get("payload") if isinstance(env.get("payload"), dict) else {}
        for f in payload.get("findings") or []:
            if isinstance(f, dict) and f.get("severity") in BLOCKING_SEVERITIES:
                errs.append(f"信封 {i} 有 {f.get('severity')} 發現 {f.get('id', '')}：{str(f.get('claim', ''))[:80]}")
        for v in payload.get("verdicts") or []:
            if not isinstance(v, dict):
                errs.append(f"信封 {i} 的 verdicts 有格式不對的條目")
            elif v.get("refuted") is not False:
                errs.append(f"信封 {i} 的 skeptic 推翻了 {v.get('target_id', '?')}：{str(v.get('reason', ''))[:80]}")
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


def chain_errors(stages: dict, stage: str) -> list[str]:
    """整條閘門鏈：SDLC 裡 stage 前面的每一段（取 run.json 實際有的段）都要 done，有閘門的都要已核准。"""
    if stage not in SDLC:
        return []
    errs = []
    for prev in SDLC[: SDLC.index(stage)]:
        ps = stages.get(prev)
        if ps is None:
            continue  # 加 sync 段之前建立的 run 沒有 sync
        if ps.get("status") != "done":
            errs.append(f"前面的 {prev} 段狀態是 {ps.get('status')}，還沒 done")
        elif ps.get("gate") and not ps.get("approved"):
            errs.append(f"前面的 {prev} 段還沒核准")
    return errs


def abandoned_error(data: dict) -> str:
    ab = data.get("abandoned")
    if not ab:
        return ""
    return f"run 已取消（{ab.get('by', '?')} @ {ab.get('at', '?')}：{ab.get('reason', '')}）"


def main(argv: list[str]) -> int:
    if argv and argv[0] == "--selftest":
        return selftest()
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
        if len(argv) < 3:
            print(__doc__)
            return 2
        stage = argv[2]
        result = json.load(sys.stdin)
        (run / f"{stage}.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        errs = []
        envs = list(envelopes(result))
        if not envs:
            errs.append(NO_ENVELOPE)
        for i, env in enumerate(envs):
            errs += [f"信封 {i}：{e}" for e in schemalib.check(env)]
        statuses = [e.get("status") for e in envs]
        st = data["stages"].setdefault(stage, {"status": "pending", "gate": stage in GATED})
        halted = result.get("halted") if isinstance(result, dict) else None
        st["status"] = "invalid" if errs else ("needs_decision" if "needs_decision" in statuses else "blocked" if ("blocked" in statuses or halted) else "done")
        st["envelopes"] = len(envs)
        old = st.pop("approved", None)
        if halted:
            st["halted"] = str(result.get("halt_reason") or halted)
            print(f"{stage}：workflow 中斷（halted）：{st['halted']}", file=sys.stderr)
        else:
            st.pop("halted", None)
        dump(run, data)
        for e in errs:
            print(e, file=sys.stderr)
        if old:
            print(f"{stage}：重存結果，舊核准（{old.get('by')} @ {old.get('at')}）已失效，要重新核准")
        if data.get("abandoned"):
            print(f"注意：{abandoned_error(data)}，這一段的結果只存檔，不能核准")
        print(f"{stage}：{len(envs)} 份信封，狀態 {st['status']}" + ("（需要使用者核准才能進下一段）" if st.get("gate") and st["status"] == "done" else ""))
        return 1 if errs else 0
    if cmd == "approve":
        if len(argv) < 3:
            print(__doc__)
            return 2
        stage = argv[2]
        by, at, note = opt(argv, "--by"), opt(argv, "--at"), opt(argv, "--note")
        st = data["stages"].get(stage)
        errs = []
        if data.get("abandoned"):
            errs.append(abandoned_error(data))
        if st is None:
            errs.append(f"沒有 {stage} 這一段")
        elif st["status"] != "done":
            errs.append(f"{stage} 狀態是 {st['status']}，只有 done 的段能核准")
        elif not st.get("envelopes"):
            errs.append(f"{stage} {NO_ENVELOPE}")
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
            if stage in HUMAN_ONLY and not any("一律由人核准" in e for e in errs):
                errs.append(f"{stage} 段一律由人核准，不能政策核准")
        errs += chain_errors(data["stages"], stage)
        if errs:
            for e in errs:
                print(f"不能核准：{e}", file=sys.stderr)
            return 1
        st["approved"] = {"by": by, "at": at, "note": note, "kind": "policy" if by == "policy" else "human"}
        dump(run, data)
        print(f"{stage} 已記錄核准：{by} @ {at}")
        return 0
    if cmd == "abandon":
        reason, by, at = opt(argv, "--reason"), opt(argv, "--by"), opt(argv, "--at")
        if not reason or not by or not at:
            print("不能取消：--reason、--by、--at 都必填", file=sys.stderr)
            return 1
        if data.get("abandoned"):
            print(f"已經取消過：{abandoned_error(data)}（保留原紀錄）")
            return 0
        data["abandoned"] = {"by": by, "at": at, "reason": reason}
        dump(run, data)
        print(f"已取消：{by} @ {at}：{reason}；之後不能核准、不能推送")
        return 0
    if cmd == "status":
        if data.get("abandoned"):
            print(f"已取消：{abandoned_error(data)}")
        for name, st in data["stages"].items():
            gate = "閘門" if st.get("gate") else ""
            ok = f"核准 {st['approved']['by']}" if st.get("approved") else ""
            print(f"{name:8} {st['status']:15} {gate} {ok}")
        if data.get("abandoned"):
            print("下一步：無（已取消）")
            return 0
        nxt = next((s for s in data["stages"] if data["stages"][s]["status"] != "done" or (data["stages"][s].get("gate") and not data["stages"][s].get("approved"))), None)
        print(f"下一步：{nxt or '全部完成'}")
        return 0
    print(__doc__)
    return 2


# ------------------------------------------------------------------ selftest
def _env(payload: dict, status: str = "done") -> dict:
    return {"status": status, "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9,
            "open_questions": [], "derived_work": [], "payload": payload}


def selftest() -> int:
    """在暫存目錄跑 init／stage／approve／abandon，驗證各閘門規則。不碰 repo。"""
    import contextlib
    import io

    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    intent = _env({"kind": "intent", "paths": ["a.md"]})
    design = _env({"epic": "e", "subtasks": [{"id": "t", "title": "t", "kind": "code", "owns": ["a"], "depends_on": [], "acceptance": ["x"]}]})
    build = _env({"task_id": "t", "branch": "b", "commits": [], "files_changed": [], "tests": []})
    test = _env({"commit": "a" * 40, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [],
                 "criteria": [{"id": "C1", "criterion": "c", "command": "x", "result": "pass", "evidence": "e"}]})
    refuted = _env({"verdicts": [{"target_id": "x", "refuted": True, "reason": "缺資料模型"}]})
    ok = True

    def call(*a: str, stdin=None) -> tuple[int, str]:
        out, err = io.StringIO(), io.StringIO()
        old_in = sys.stdin
        sys.stdin = io.StringIO(json.dumps(stdin) if stdin is not None else "")
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main(list(a))
        finally:
            sys.stdin = old_in
        return code, out.getvalue() + err.getvalue()

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{detail[-300:]}"))
        ok = ok and cond

    with tempfile.TemporaryDirectory() as tmp:
        n = [0]

        def new_run() -> str:
            n[0] += 1
            d = str(Path(tmp) / f"r{n[0]}")
            call("init", d, "--event", "sdlc", "--target", "x", "--requirement", "r")
            return d

        def stages(d: str) -> dict:
            return load(Path(d))["stages"]

        def walk(d: str, upto: str) -> None:
            for s, r in [("plan", {"intent": intent}), ("design", {"design": design}), ("build", {"x": build}),
                         ("sync", {"x": build}), ("test", {"v": test}), ("deliver", {"x": build})]:
                if s == upto:
                    return
                call("stage", d, s, stdin=r)
                if s in GATED:
                    call("approve", d, s, "--by", "human", "--at", "t")

        print("save_run selftest（暫存目錄，不碰 repo）")
        # 重存清核准
        d = new_run()
        walk(d, "test")
        call("stage", d, "test", stdin={"v": test})
        c1, _ = call("approve", d, "test", "--by", "human", "--at", "t")
        c2, out = call("stage", d, "test", stdin={"v": test})
        check("重存清核准：重存 test 後舊核准失效，並在輸出說明", c1 == 0 and c2 == 0 and not stages(d)["test"].get("approved") and "已失效" in out, out)
        # 零信封
        d = new_run()
        c, out = call("stage", d, "plan", stdin={"intent": None})
        c2, out2 = call("approve", d, "plan", "--by", "human", "--at", "t")
        check("零信封：記成 invalid 而且不能核准", c != 0 and stages(d)["plan"]["status"] == "invalid" and NO_ENVELOPE in out and c2 != 0, out + out2)
        # skeptic 推翻
        d = new_run()
        walk(d, "design")
        call("stage", d, "design", stdin={"design": design, "analysis": [refuted]})
        c, out = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        d2 = new_run()
        walk(d2, "design")
        call("stage", d2, "design", stdin={"design": design, "analysis": [None]})
        c2, out2 = call("approve", d2, "design", "--by", "policy", "--at", "t", "--note", "p")
        d3 = new_run()
        walk(d3, "design")
        call("stage", d3, "design", stdin={"design": design, "analysis": [_env({"verdicts": [{"target_id": "x", "refuted": False, "reason": "成立"}]})]})
        c3, out3 = call("approve", d3, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("skeptic 推翻：refuted 與 skeptic 缺件（null）都不能政策核准，全部成立才行",
              c != 0 and "推翻" in out and c2 != 0 and "缺件" in out2 and c3 == 0, out + out2 + out3)
        viol = policy_violations("plan", {"intent": intent, "challenge": None})
        check("skeptic 推翻：challenge: null 判為 agent 缺件", any("challenge" in v and "缺件" in v for v in viol), str(viol))
        # plan 禁政策
        d = new_run()
        call("stage", d, "plan", stdin={"intent": intent})
        c, out = call("approve", d, "plan", "--by", "policy", "--at", "t", "--note", "p")
        c2, _ = call("approve", d, "plan", "--by", "human", "--at", "t")
        check("plan 禁政策：plan 段 --by policy 拒絕、人工核准可以", c != 0 and "由人核准" in out and c2 == 0, out)
        # 閘門鏈
        d = new_run()
        call("stage", d, "plan", stdin={"intent": intent})
        call("approve", d, "plan", "--by", "human", "--at", "t")
        call("stage", d, "design", stdin={"design": design})
        for s, r in [("build", {"x": build}), ("sync", {"x": build}), ("test", {"v": test})]:
            call("stage", d, s, stdin=r)
        c, out = call("approve", d, "test", "--by", "human", "--at", "t")
        call("approve", d, "design", "--by", "human", "--at", "t")
        c2, out2 = call("approve", d, "test", "--by", "human", "--at", "t")
        check("閘門鏈：design 沒核准時 test 不能核准，補核准後可以", c != 0 and "design" in out and c2 == 0, out + out2)
        # abandon
        d = new_run()
        call("stage", d, "plan", stdin={"intent": intent})
        c, _ = call("abandon", d, "--reason", "需求取消", "--by", "human", "--at", "t")
        c2, out2 = call("approve", d, "plan", "--by", "human", "--at", "t")
        _, st_out = call("status", d)
        ab = load(Path(d)).get("abandoned") or {}
        c3, _ = call("abandon", d, "--by", "human", "--at", "t")
        check("abandon：記錄 {by, at, reason}、之後不能核准、status 顯示已取消、缺 --reason 拒絕",
              c == 0 and ab == {"by": "human", "at": "t", "reason": "需求取消"} and c2 != 0 and "已取消" in out2
              and "已取消" in st_out and c3 != 0, out2 + st_out)
    print("save_run selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
