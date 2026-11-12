"""把 workflow 的回傳存進 run 目錄，並記錄 SDLC 閘門（Workflow 腳本不能寫檔，由主 session 呼叫）。

run 目錄：{{RUNS_DIR}}/<YYYYMMDD>-<事件>-<短名>/
    run.json            事件、目標 repo、recipe、各段狀態、閘門核准紀錄、取消紀錄（abandoned）、後續 run（followup_run）、結案（closed）
    context.md          這次 run 的共用背景（目標 repo、事件、需求原文或路徑、recipe 一行摘要）；所有 agent 共用，派工單不再重貼
    <stage>.json        該段 workflow 的完整回傳（每個 agent 的信封）
    <stage>/執行紀錄索引.md  依回傳的 trace 產生：每次派工一列（label、角色、phase、紀錄檔、成果檔、狀態）

run.json 的 recipe（stage plan 時寫入；沒有 recipe 的舊 run 一律當成 feature 的完整流程）：
    {"name", "modifiers", "size", "reasons", "resolved_from",
     "resolved"（recipe.resolve 的完整結果，由本工具依 profile 重算，不採信 workflow 給的段）,
     "locked"（plan 核准後為 true）, "history"（escalate 紀錄 [{from, to, by, at, reason}]）}
    recipe 沒有的段在 stages 裡標成 {"status": "skipped", "gate": false}（不刪，方便看）。

用法：
    python3 save_run.py init <run_dir> --event sdlc --target <repo> --requirement <描述或路徑>
        另寫 <run_dir>/context.md（目標 repo、事件、需求）。
    python3 save_run.py stage <run_dir> <stage> [--profile <methodology.json>] < result.json   # 存回傳並逐一驗證信封
        重存某一段會清掉該段的核准（舊核准對應的是舊結果，已失效，要重新核准）。
        一份信封都沒有（agent 缺件）記成 invalid，不能核准。
        回傳物件帶 halted（流程中斷）時，stage 一律記成 blocked。
        stage plan：回傳物件的 recipe（至少 name、modifiers、size，可帶 reasons）寫進 run.json；resolved 由本工具用
          recipe.resolve 重算（profile 預設是這支工具所在專案的 .claude/teams/methodology.json，不從 run 目錄往上找；找不到用內建節；--profile 可指定），
          stages 依 recipe 重建；context.md 追加 recipe 一行摘要。recipe 已鎖定（plan 核准過）或 escalate 過時不能再換，要改走 escalate。
          intent 檢查（一定要有 intent）：intent 信封 payload.documents 裡 kind=intent 的檔（相對路徑以回傳的 epic_dir 為基準）
          不存在、或缺必要的節（四節＋該 plan_mode 的專屬節，profile 可覆寫）→ invalid，不能核准。
        其他段：recipe 沒有這一段 → invalid（要加段走 escalate）。
        回傳帶 trace（[{label, agent_type, phase, record, ok}]）時，寫 <run_dir>/<stage>/執行紀錄索引.md，並核對紀錄檔：
          ok=true 但紀錄檔不存在：對得回它的信封（artifacts 列了紀錄檔，或 trace 的 digest 和信封的 status＋summary 一致）
          就依信封代寫（標明「由 save_run 代寫」，記進該段 ghostwritten_records）；對不回信封才記進 missing_records
          （缺件：不能政策核准；人工核准可以但會警告）。agent 自己寫的紀錄不會被覆寫。
          dev-merge-steward（沒有 Write 工具）的紀錄由本工具依它的信封代寫（標明「由 save_run 代寫」）。
          回傳沒有 trace（舊 workflow）只印警告，不判缺件。
    python3 save_run.py approve <run_dir> <stage> --by <誰> --at <ISO 時間> [--note <備註>] [--profile <檔>]
        [--answer "<問題>=<答覆>"]… [--answer-scope epic|project] [--answer-expires <YYYY-MM-DD>]
        --answer：人在閘門上回答的問題，記進 <runs_dir>/decisions.jsonl（決策記憶）；之後的資料包會帶上相關的，agent 不再重問。
        只有人的核准能帶 --answer；範圍預設 epic（只給這個需求），project 是整個專案。
    python3 save_run.py decide <run_dir> --question <問題> --answer <答覆> --by <誰> --at <ISO 時間> [--scope epic|project] [--expires <YYYY-MM-DD>]
        不在核准時也可以記一筆決定。
       （plan 以外的段會用 profile 重算 recipe，resolved 被改鬆就拒絕）
        只有狀態 done 的段能核准；recipe 裡 SDLC 前面的每一段都要 done，有閘門的都要已核准（整條閘門鏈，
        不只緊鄰的前一段：build、sync 沒有閘門，不能讓 test 跳過 design 的核准）。recipe 裡有、run.json 卻沒有的段判錯
        （只有沒有 recipe 的舊 run 容許缺 sync）；recipe 沒有的段（skipped）不檢查。已取消的 run 不能核准。
        plan 核准後 recipe 鎖定（locked=true）。
        --by policy 表示依專案政策自動核准（全自動流程用），--note 必須寫政策名稱；plan 段一律由人核准，不能用 policy。
        政策核准會讀 <stage>.json 的實際內容（policy_violations）：verifier 有不是 pass 的條目、反作弊有發現、
        任何 blocking 待決問題或 blocker／important 發現、skeptic 的 verdicts 有 refuted、
        agent 缺件（回傳裡有 null；缺件不是「沒有發現」）、回傳標了 halted、執行紀錄檔缺件，都不能政策核准，要交人。
    python3 save_run.py escalate <run_dir> --recipe <名> [--modifiers a,b] [--size s] --by <誰> --at <ISO 時間> --reason <理由> [--profile <檔>]
        升級 recipe（只能加嚴）：新 resolved＝merge_stricter(resolve(新), 舊)；新 recipe 少了舊的段（降級）一律拒絕。
        新加的段、或設定變嚴的段（design lens、test 審查與模式、deliver 審查軸、intent 必要節）之後已 done 的段
        改成 stale 並清掉核准（要重跑），閘門鏈會擋；history 記一筆。
    python3 save_run.py followup <run_dir> --run <後續 run 目錄> --by <誰> --at <ISO 時間>
        記錄後續 run（hotfix 事後補完整流程用）：run.json 記 followup_run {run, by, at}；後續 run 目錄要有 run.json。
    python3 save_run.py close <run_dir> --by <誰> --at <ISO 時間>
        結案：recipe 要求後續 run（followup=true，例如 hotfix）時，沒有 followup_run 就拒絕。run.json 記 closed {by, at}。
    python3 save_run.py abandon <run_dir> --reason <為什麼取消> --by <誰> --at <ISO 時間>
        需求取消：run.json 記 abandoned {by, at, reason}；之後任何一段都不能核准，publish.py 也拒絕推送。
    python3 save_run.py status <run_dir> [--json]      # --json：給外部程式讀的快照（schemas/run-status.schema.json）
    python3 save_run.py --selftest       # 在暫存目錄跑，不碰 repo
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack as packlib  # noqa: E402
import recipe as rcp  # noqa: E402
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
# 不是 agent 交件的結構（trace 是派工紀錄、recipe 是分類結果），不在裡面找缺件的 null
NOT_DELIVERY_KEYS = {"trace", "recipe"}
NO_ENVELOPE = "沒有任何信封（agent 缺件）"
INDEX_NAME = "執行紀錄索引.md"
STEWARD = "dev-merge-steward"
GHOSTWRITTEN = "由 save_run 代寫"


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def opts(argv: list[str], name: str) -> list[str]:
    return [argv[i + 1] for i, a in enumerate(argv) if a == name and i + 1 < len(argv)]


def decision_errors(items: list[tuple], by: str, scope: str, expires: str) -> list[str]:
    """決策記憶的輸入檢查：只有人能記、問題與答覆都要有、長度有上限、範圍與到期日格式要對。"""
    errs = []
    if by == "policy":
        errs.append("決定只能由人記（--by policy 不能帶答覆）")
    if scope not in packlib.DECISION_SCOPES:
        errs.append(f"範圍只能是 {'、'.join(packlib.DECISION_SCOPES)}：{scope}")
    if expires and not packlib.DATE.match(expires):
        errs.append(f"到期日要寫成 YYYY-MM-DD：{expires}")
    for q, a in items:
        if not q.strip() or not a.strip():
            errs.append(f"答覆要寫成「問題=答覆」，兩邊都不能空：{(q or a)[:60]}")
        elif len(q) > 300 or len(a) > 600:
            errs.append(f"問題最多 300 字、答覆最多 600 字：{q[:40]}")
    return errs


def record_decisions(run: Path, items: list[tuple], by: str, at: str, scope: str, expires: str, stage: str = "") -> int:
    """把決定附加到 <runs_dir>/decisions.jsonl，並重寫給人與知識庫看的 決策記憶.md。"""
    runs = run.resolve().parent
    epic = packlib.run_epic(run)
    with open(runs / packlib.DECISIONS, "a", encoding="utf-8", newline="\n") as f:
        for q, a in items:
            rec = {"question": " ".join(q.split()), "answer": " ".join(a.split()), "scope": scope, "by": by, "at": at, "run": run.resolve().name}
            rec.update({k: v for k, v in (("stage", stage), ("epic", epic), ("expires", expires)) if v})
            f.write(json.dumps(rec, ensure_ascii=False, sort_keys=True) + "\n")
    try:
        import datetime
        packlib._write_lf(runs / packlib.DECISIONS_VIEW, packlib.decisions_view(runs, datetime.date.today().isoformat()))
    except OSError as e:
        print(f"警告：{packlib.DECISIONS_VIEW} 寫不出來（{e}）；decisions.jsonl 已記錄", file=sys.stderr)
    return len(items)


def split_answer(raw: str) -> tuple:
    for sep in ("=", "＝"):
        if sep in raw:
            q, a = raw.split(sep, 1)
            return q.strip(), a.strip()
    return raw.strip(), ""


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
            if (v is None and k in NULL_OK_KEYS) or (path == "$" and k in NOT_DELIVERY_KEYS):
                continue
            out += missing_agents(v, f"{path}.{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            out += missing_agents(v, f"{path}[{i}]")
    return out


BLOCKING_SEVERITIES = {"blocker", "important"}


# ------------------------------------------------------------------ 執行紀錄（trace）
def _md_cell(v) -> str:
    return str(v if v not in (None, "") else "—").replace("|", "\\|").replace("\n", " ")


def _norm(p: str) -> str:
    return str(p).replace("\\", "/")


def record_candidates(run: Path, stage: str, record: str) -> list[Path]:
    """紀錄檔可能的位置：trace 給的路徑（相對路徑以目前目錄＝專案根為基準），以及 <run>/<stage>/<檔名>。"""
    if not record:
        return []
    p = Path(record)
    given = p if p.is_absolute() else Path.cwd() / p
    out = [run / stage / p.name]
    try:      # trace 給的路徑只在 run 目錄底下才採用：拿 run 以外隨便一個存在的檔當紀錄不算
        given.resolve().relative_to(Path(run).resolve())
        out.insert(0, given)
    except (ValueError, OSError):
        pass
    return out


def _envelope_of(entry: dict, stage: str, envs: list[dict]) -> dict | None:
    """信封的 artifacts 列了這個 agent 的紀錄檔，就當成它的信封（agent 依派工前綴把紀錄檔列進 artifacts）。
    對不到時用 trace 的 digest（執行核心記下的 status＋summary）找：剛好只有一個信封相符才算，零個或多個都不猜。"""
    rec = _norm(entry.get("record") or "")
    if rec:
        tail = f"{stage}/{Path(rec).name}"
        for env in envs:
            for a in env.get("artifacts") or []:
                if isinstance(a, str) and (_norm(a) == rec or _norm(a).endswith("/" + tail) or _norm(a) == tail):
                    return env
    dg = entry.get("digest")
    if isinstance(dg, dict) and isinstance(dg.get("summary"), str) and dg["summary"]:
        same = [e for e in envs if isinstance(e.get("summary"), str) and e["summary"][:400] == dg["summary"]
                and str(e.get("status") or "") == str(dg.get("status") or "")]
        if len(same) == 1 or (same and all(e is same[0] or e == same[0] for e in same)):
            return same[0]
    return None


def _ghostwrite(path: Path, entry: dict, env: dict) -> None:
    """依信封代寫紀錄檔：dev-merge-steward（沒有 Write 工具），以及有交件但漏寫紀錄檔的 agent。內容只來自信封，可重現。"""
    p = env.get("payload") if isinstance(env.get("payload"), dict) else {}
    steward = "traceability" in p
    why = "沒有 Write 工具" if steward else "有交件但沒有寫紀錄檔"
    lines = [f"# 執行紀錄：{entry.get('label', '')}（{GHOSTWRITTEN}）", "",
             f"> {entry.get('agent_type', '')} {why}，這份紀錄由 save_run.py 依它交回的信封代寫，內容只來自信封。", "",
             "## 任務摘要", "", str(env.get("summary") or "（信封沒有 summary）"), "",
             "## 做了什麼與證據", ""]
    ev = [e for e in env.get("evidence") or [] if isinstance(e, dict)]
    lines += [f"- {e.get('claim', '')}：{e.get('result', '')}（出處：{e.get('source', '')}）" for e in ev] or ["- （信封沒有 evidence）"]
    lines += ["", "## 結論", ""]
    if steward:
        lines += [f"- decision：{p.get('decision', '')}", f"- head_sha：{p.get('head_sha', '')}",
                  f"- merge_state：{p.get('merge_state', '')}", f"- 需求對照：{len(p.get('traceability') or [])} 條；followups：{len(p.get('followups') or [])} 則",
                  f"- gate_reasons：{'；'.join(str(x) for x in p.get('gate_reasons') or []) or '（無）'}"]
    else:
        lines += [f"- status：{env.get('status', '')}", f"- confidence：{env.get('confidence', '')}",
                  f"- payload 的欄位：{'、'.join(sorted(str(k) for k in p)) or '（無）'}"]
    lines += ["", "## 成果檔", ""]
    lines += [f"- {a}" for a in env.get("artifacts") or []] or ["- （無）"]
    lines += ["", "## 不確定的地方", ""]
    qs = [q for q in env.get("open_questions") or [] if isinstance(q, dict)]
    lines += [f"- {q.get('question', '')}" for q in qs] or ["- （信封沒有待決問題）"]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _head(path: Path) -> str:
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.readline()
    except OSError:
        return ""


def _retried_ok(trace: list, i: int) -> bool:
    """trace[i] 標了 superseded，而且後面有同一個 label、同一個角色、ok=true 的重派（不只看旗標，要真的有後面那筆）。"""
    t = trace[i]
    if t.get("superseded") is not True:
        return False
    return any(isinstance(x, dict) and x.get("ok") is True and x.get("label") == t.get("label") and x.get("agent_type") == t.get("agent_type")
               for x in trace[i + 1:])


# 已經改用執行核心（kernel 片段）的段：profile 有設 models 時，trace 每筆都要有 model 與 tier。0.5 起 SDLC 每一段都是
KERNEL_STAGES = {"plan", "design", "build", "sync", "test", "deliver", "merge"}


def model_errors(stage: str, result, profile: dict | None, modifiers) -> list[str]:
    """模型分級（I7）：trace 記的等級不能低於下限，model 要等於該等級對應的模型。
    profile 沒有 models 或 mode=inherit 時不檢查；trace 沒有 tier 的舊 workflow 只在 KERNEL_STAGES 判錯。"""
    models = (profile or {}).get("models")
    if not isinstance(models, dict) or models.get("mode", "auto") == "inherit":
        return []
    trace = result.get("trace") if isinstance(result, dict) and isinstance(result.get("trace"), list) else []
    errs = []
    for t in trace:
        if not isinstance(t, dict):
            continue
        label, role = str(t.get("label") or ""), str(t.get("agent_type") or "").split(":")[-1]
        tier = t.get("tier")
        if not re.match(r"^(tpm|dev)-", role):
            continue  # 使用者自己的 agent（擴充 agent）：執行核心不替它指定模型，也沒有下限
        if tier in (None, ""):
            if stage in KERNEL_STAGES:
                errs.append(f"{label} 的 trace 沒有 model／tier（profile 有設 models，這一段的派工都要經過執行核心）")
            continue
        if tier not in rcp.TIERS:
            errs.append(f"{label} 的等級「{tier}」不認得")
            continue
        floor = rcp.resolve_model(profile, role, stage, None, modifiers)["floor"]
        if floor and rcp.TIERS.index(tier) < rcp.TIERS.index(floor):
            errs.append(f"{label}（{role}）用了 {tier}，低於下限 {floor}")
        want = rcp.tier_model(profile, tier) or "inherit"
        if t.get("model") != want:
            errs.append(f"{label} 的等級是 {tier}，model 應該是 {want}，trace 記的是 {t.get('model')}")
    return errs


def process_trace(run: Path, stage: str, result, write: bool) -> tuple[bool, list[dict], list[str]]:
    """依回傳的 trace 核對紀錄檔。回傳（有沒有 trace、索引列、缺件理由）。
    write=True 時代寫 steward 的紀錄檔並寫 <run>/<stage>/執行紀錄索引.md；write=False 只核對（政策判定用）。"""
    if not isinstance(result, dict) or "trace" not in result:
        return False, [], []
    trace = result.get("trace")
    if not isinstance(trace, list):
        return True, [], ["trace 格式不對（不是陣列），無法核對執行紀錄"]
    envs = list(envelopes({k: v for k, v in result.items() if k != "trace"}))
    stewards = [e for e in envs if isinstance(e.get("payload"), dict) and "traceability" in e["payload"]]
    rows: list[dict] = []
    missing: list[str] = []
    for i, t in enumerate(trace):
        if not isinstance(t, dict):
            missing.append(f"trace[{i}] 格式不對")
            continue
        label, record, okv = str(t.get("label") or ""), str(t.get("record") or ""), t.get("ok")
        env = _envelope_of(t, stage, envs)
        is_steward = str(t.get("agent_type") or "").endswith(STEWARD)
        status = "完成"
        if okv is True and is_steward:
            env = stewards.pop(0) if stewards else None
            target = run / stage / Path(record or f"{label}.md").name
            if env is None:
                status = "缺紀錄檔（找不到 steward 的信封，無法代寫）"
                missing.append(f"{label} 的紀錄檔無法代寫：回傳裡找不到對應的 merge-decision 信封")
            else:
                if write:
                    _ghostwrite(target, t, env)
                record = record or str(target)
                if target.is_file():
                    status = f"完成（{GHOSTWRITTEN}紀錄）"
                else:
                    status = "缺紀錄檔"
                    missing.append(f"{label} 的紀錄檔 {target} 不存在（steward 的紀錄要由 save_run stage 代寫）")
        elif okv is True:
            if not record:
                status = "缺紀錄檔（trace 沒有指定紀錄檔）"
                missing.append(f"{label} 的 trace 沒有指定紀錄檔")
            elif not any(c.is_file() for c in record_candidates(run, stage, record)):
                target = run / stage / Path(record).name
                if env is not None and write:
                    _ghostwrite(target, t, env)
                if env is not None and target.is_file():
                    status = f"完成（{GHOSTWRITTEN}紀錄：agent 沒寫紀錄檔）"
                else:
                    status = "缺紀錄檔"
                    missing.append(f"{label} 的紀錄檔 {record} 不存在" + ("" if env is None else "（還沒代寫：先跑 save_run.py stage）"))
            elif env is None and GHOSTWRITTEN in _head(next(c for c in record_candidates(run, stage, record) if c.is_file())):
                # 紀錄檔是之前代寫的，這次的回傳卻對不回信封（回傳被換過）：不沿用舊的代寫檔
                status = "缺紀錄檔（代寫的紀錄對不回這次的信封）"
                missing.append(f"{label} 的紀錄檔是代寫的，但這次的回傳裡找不到對應的信封")
            elif GHOSTWRITTEN in _head(next(c for c in record_candidates(run, stage, record) if c.is_file())):
                status = f"完成（{GHOSTWRITTEN}紀錄：agent 沒寫紀錄檔）"
        elif okv is False and _retried_ok(trace, i):
            # 執行核心的失敗升級：這一筆沒交件，後面同一個 label、同一個角色的重派有交件，不算缺件
            status = f"沒有交件，已升一級重派（{t.get('tier') or '?'} → 見下一筆）"
        elif okv is False and t.get("capped") is True:
            status = "沒有派出（派工數到上限）"
            missing.append(f"{label} 沒有派出：派工數到上限（{t.get('max_agents') or '?'}；profile 的 limits.max_agents），要人決定是調高上限還是拆小")
        elif okv is False:
            status = "agent 沒有交件（失敗或交件無效）"
            missing.append(f"{label} 沒有交件（失敗或交件無效）")
        else:
            status = "未完成（ok 不是 true／false）"
            missing.append(f"{label} 未完成（trace 的 ok 不是 true）")
        arts = [a for a in (env or {}).get("artifacts") or [] if isinstance(a, str) and _norm(a) != _norm(record)
                and not _norm(a).endswith("/" + f"{stage}/{Path(record).name}")] if env else []
        rows.append({"label": label, "agent_type": t.get("agent_type") or "", "phase": t.get("phase") or "",
                     "record": record, "artifacts": arts, "status": status})
    if write:
        d = run / stage
        d.mkdir(parents=True, exist_ok=True)
        lines = [f"# 執行紀錄索引：{stage}", "",
                 f"由 save_run.py 依 workflow 回傳的 trace 產生，每列是一次派工（共 {len(rows)} 次）。"
                 "agent 有交件但沒寫紀錄檔時，由 save_run 依信封代寫（狀態欄會標明）；連信封都對不回才算缺件，不能政策核准。"
                 "成果檔取自該 agent 信封的 artifacts。", "",
                 "| label | 角色 | phase | 紀錄檔 | 成果檔 | 狀態 |", "|---|---|---|---|---|---|"]
        for r in rows:
            lines.append("| " + " | ".join(_md_cell(x) for x in (r["label"], r["agent_type"], r["phase"], r["record"],
                                                                  "<br>".join(r["artifacts"]) if r["artifacts"] else "—", r["status"])) + " |")
        if missing:
            lines += ["", "## 缺件", ""] + [f"- {m}" for m in missing]
        (d / INDEX_NAME).write_text("\n".join(lines) + "\n", encoding="utf-8")
    return True, rows, missing


def policy_violations(stage: str, result, run: Path | None = None) -> list[str]:
    """政策核准（--by policy）的確定性判定：讀該段 workflow 回傳的實際內容，回傳不能自動核准的理由（空清單＝可以）。
    人工核准不呼叫這個函式。publish.py 推送前也重跑這個函式（同一套判定）。

    - plan 段一律由人核准（rules.md 規則 16），政策核准一律不行。
    - 一份信封都沒有、或回傳裡有 null（agent 缺件）不行：缺件不是「沒有發現」（rules.md 規則 26）。
    - 回傳標了 halted（流程中斷）不行。
    - 回傳帶 custom_script（自訂腳本）、override_script（覆寫檔或舊副本）、manual_output（done --output 手動給的結果）不行：一律由人核准。
    - 任何信封的 open_questions 有 blocking=true 不行。
    - 任何 findings payload 有 blocker／important 等級的發現不行。
    - 任何 verdicts payload 有 refuted=true（skeptic 推翻了該段的產出）不行。
    - test 段：必須有 verifier 的 criteria payload；任何一條 result 不是 pass（fail、not_verifiable）或 anti_cheat 非空都不行。
    - 給了 run 目錄而回傳帶 trace 時：重新核對紀錄檔（不採信 run.json 的 missing_records），缺了不行。
    """
    errs: list[str] = []
    if stage in HUMAN_ONLY:
        errs.append(f"{stage} 段一律由人核准（rules.md 規則 16），不能政策核准")
    if isinstance(result, dict) and result.get("halted"):
        errs.append(f"回傳標了 halted：{result.get('halt_reason') or result.get('halted')}")
    if isinstance(result, dict) and result.get("custom_script"):
        errs.append("這一段是用自訂腳本跑的（awf.py next --script）：流程沒有經過標準腳本的核對，要由人核准")
    if isinstance(result, dict) and result.get("override_script"):
        errs.append("這一段用的是專案自己的 workflow 腳本（覆寫檔或舊副本），不是 plugin 的範本：要由人核准")
    if isinstance(result, dict) and result.get("manual_output"):
        errs.append("這一段的結果是用 awf.py done --output 手動給的（不是 awf 從 workflow 紀錄取得的）：要由人核准")
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
    if run is not None:
        has_trace, _, miss = process_trace(Path(run), stage, result, write=False)
        errs += [f"執行紀錄缺件：{m}" for m in miss]
        data = None
        try:
            data = load(Path(run))
        except (OSError, ValueError):
            errs.append("讀不到 run.json，無法依 recipe 核對（fail-closed）")
        if data is not None and has_recipe(data) and stage in SDLC and stage != "plan":
            if not has_trace:
                errs.append("執行紀錄缺件：這個 run 有 recipe，回傳卻沒有 trace（無法核對每個 agent 的紀錄）")
            errs += recipe_mismatch(data, result)
            errs += required_roles_missing(data, stage, result)
    return errs


def recipe_mismatch(data: dict, result) -> list[str]:
    """workflow 回傳的 recipe 要和 run.json 鎖定的 resolved 一致（name、modifiers、stages），不一致代表 workflow 用了別的 recipe 裁減（X3 B1）。"""
    want = data["recipe"]
    res = want.get("resolved") or {}
    got = result.get("recipe") if isinstance(result, dict) else None
    if not isinstance(got, dict):
        return [f"回傳沒有 recipe：無法確認 workflow 依 run.json 鎖定的 recipe {want.get('name')} 裁減"]
    errs = []
    if got.get("name") != want.get("name"):
        errs.append(f"回傳的 recipe 是 {got.get('name')}，run.json 鎖定的是 {want.get('name')}")
    if sorted(_mods(got.get("modifiers"))) != sorted(want.get("modifiers") or []):
        errs.append(f"回傳的 modifiers {got.get('modifiers')} 和 run.json 的 {want.get('modifiers')} 不一致")
    if list(got.get("stages") or []) != list(res.get("stages") or []):
        errs.append(f"回傳的段 {got.get('stages')} 和 run.json 的 {res.get('stages')} 不一致")
    return errs


# recipe 要求「一定要派」的角色：test 段依 test.*_review=always，deliver 段依 deliver_axes（label review:<軸>）
REVIEW_ROLES = {"security_review": "dev-security-reviewer", "design_review": "dev-design-reviewer"}


def required_roles_missing(data: dict, stage: str, result) -> list[str]:
    res = data["recipe"].get("resolved") or {}
    trace = result.get("trace") if isinstance(result, dict) and isinstance(result.get("trace"), list) else []
    done = [t for t in trace if isinstance(t, dict) and t.get("ok") is True]
    errs = []
    if stage == "test":
        t = res.get("test") or {}
        for key, role in REVIEW_ROLES.items():
            if t.get(key) == "always" and not any(str(x.get("agent_type") or "").endswith(role) for x in done):
                errs.append(f"recipe 要求 {key}=always，trace 裡沒有交件的 {role}")
        if not any(str(x.get("agent_type") or "").endswith("dev-verifier") for x in done):
            errs.append("trace 裡沒有交件的 dev-verifier")
    if stage == "deliver":
        for axis in res.get("deliver_axes") or []:
            if not any(str(x.get("label") or "") == f"review:{axis}" for x in done):
                errs.append(f"recipe 的 deliver 軸 {axis} 沒有交件的審查（trace 沒有 review:{axis}）")
    return errs


# ------------------------------------------------------------------ recipe
def has_recipe(data: dict) -> bool:
    rec = data.get("recipe")
    return isinstance(rec, dict) and isinstance(rec.get("resolved"), dict)


PROJECT_PROFILE = Path(__file__).resolve().parent.parent / "methodology.json"

# ------------------------------------------------------------------ 限制文件（profile 的 constraints）
CONSTRAINT_VERDICTS = ("符合", "偏離", "不適用")
CONSTRAINT_CHECK_TIMEOUT = 300


def project_root() -> Path:
    """專案根目錄：methodology.json 在 <專案>/.claude/teams/ 底下。"""
    return PROJECT_PROFILE.parent.parent.parent


def constraint_file(c: dict, repo: Path | None) -> Path | None:
    """限制文件的實際位置：先找專案根目錄，再找目標 repo；都沒有回 None。"""
    f = str(c.get("file") or "")
    for base in (project_root(), repo):
        if base is not None and f and (base / f).is_file():
            return base / f
    return None


def constraint_rows(run: Path, stage: str) -> dict:
    """這一段的執行紀錄（<run>/<stage>/*.md）裡的〈限制對照〉列：{id: [(判定, 紀錄檔名)]}。"""
    out: dict = {}
    d = run / stage
    if not d.is_dir():
        return out
    row = re.compile(r"^\s*\|\s*`?\[?([a-z0-9][a-z0-9-]{0,39})\]?`?\s*\|\s*(" + "|".join(CONSTRAINT_VERDICTS) + r")\s*\|")
    for f in sorted(d.glob("*.md")):
        if f.name == INDEX_NAME:
            continue
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if text.startswith("# 執行紀錄：") and GHOSTWRITTEN in text.split("\n", 1)[0]:
            continue  # 代寫的紀錄只來自信封，不算 agent 做過對照
        for line in text.splitlines():
            m = row.match(line)
            if m:
                out.setdefault(m.group(1), []).append((m.group(2), f.name))
    return out


AC_ID = re.compile(r"(?<![\w-])AC[-_][A-Za-z0-9]+(?:[-_.][A-Za-z0-9]+)*")


def _result_subtasks(result) -> list[dict]:
    """這一段回傳裡的子任務清單（design.payload.subtasks、payload.subtasks 或頂層 subtasks）。"""
    r = result if isinstance(result, dict) else {}
    for c in ((r.get("design") or {}).get("payload") if isinstance(r.get("design"), dict) else None, r.get("payload"), r):
        subs = c.get("subtasks") if isinstance(c, dict) else None
        if isinstance(subs, list) and subs and all(isinstance(x, dict) for x in subs):
            return subs
    return []


def plan_check_issues(result, spec: Path | None = None) -> list[str]:
    """拆工的確定性檢查（plan_check）：在 design 閘門就抓出來，不要拖到 build 才爆。回傳問題清單（空＝沒問題）。
    - 子任務 id 重複、相依指向不存在的子任務、相依有循環。
    - 子任務沒有驗收條件（acceptance 是空的）；會改檔的子任務（code、test、doc）沒有 owns。
    - 有 spec 檔而且 spec 與子任務都用「AC-…」編號時：spec 有、沒有任何子任務對應的驗收條件；子任務引用了 spec 沒有的驗收條件。
      其中一邊沒用這種編號就不比（專案的寫法不同，工具判不了）。
    回傳裡沒有子任務時不檢查（那一段沒拆工）。"""
    subs = _result_subtasks(result)
    if not subs:
        return []
    errs: list[str] = []
    ids = [str(x.get("id") or "") for x in subs]
    dup = sorted({i for i in ids if i and ids.count(i) > 1})
    if dup:
        errs.append("子任務 id 重複：" + "、".join(dup))
    known = set(ids)
    deps = {str(x.get("id") or ""): [str(d) for d in x.get("depends_on") or []] for x in subs}
    for x in subs:          # 逐筆看（id 重複時 deps 這個表只留得住一筆）
        lost = [str(d) for d in x.get("depends_on") or [] if str(d) not in known]
        if lost:
            errs.append(f"子任務 {x.get('id')} 相依指向不存在的子任務：" + "、".join(lost))
    left = {i: {d for d in ds if d in known} for i, ds in deps.items()}
    while True:
        free = [i for i, ds in left.items() if not ds]
        if not free:
            break
        for i in free:
            left.pop(i)
        for ds in left.values():
            ds.difference_update(free)
    if left:
        errs.append("子任務相依有循環：" + "、".join(sorted(left)))
    for x in subs:
        i = str(x.get("id") or "?")
        if not [a for a in x.get("acceptance") or [] if str(a).strip()]:
            errs.append(f"子任務 {i} 沒有驗收條件（acceptance 是空的）")
        if x.get("kind") in ("code", "test", "doc") and not [o for o in x.get("owns") or [] if str(o).strip()]:
            errs.append(f"子任務 {i}（{x.get('kind')}）沒有 owns：不知道它可以改哪些檔")
    if spec is not None and spec.is_file():
        try:
            in_spec = set(AC_ID.findall(spec.read_text(encoding="utf-8", errors="replace")))
        except OSError:
            in_spec = set()
        used = set()
        for x in subs:
            for t in [*(x.get("acceptance") or []), *(x.get("story_ids") or [])]:
                used.update(AC_ID.findall(str(t)))
        if in_spec and used:
            orphan, ghost = sorted(in_spec - used), sorted(used - in_spec)
            if orphan:
                errs.append(f"spec 有 {len(orphan)} 條驗收條件沒有任何子任務對應：" + "、".join(orphan[:12]) + ("…" if len(orphan) > 12 else ""))
            if ghost:
                errs.append(f"子任務引用了 spec 沒有的驗收條件：" + "、".join(ghost[:12]) + ("…" if len(ghost) > 12 else ""))
    return errs


def _plan_check_for(run: Path, stage: str, result, data: dict) -> list[str]:
    if stage not in ("plan", "design"):
        return []
    repo = Path(str(data.get("target_repo"))) if data.get("target_repo") else None
    sd = packlib.spec_dir(repo, packlib.run_epic(run) or str((data.get("args") or {}).get("epic") or ""))
    return plan_check_issues(result, sd / "spec.md" if sd is not None else None)


def constraint_issues(run: Path, stage: str, profile: dict | None, repo: Path | None, run_checks: bool = True) -> tuple[list[str], list[dict]]:
    """這一段適用的限制文件有沒有被遵守的確定性檢查。回傳（不能政策核准的理由, 檢查指令的結果）。
    - 文件不存在：不行（限制讀不到，agent 等於沒有依據）。
    - trace=table：這一段的執行紀錄要有該限制的對照列；沒有不行，有「偏離」不行（偏離要由人決定）。
      build 段有 paths 的限制只發給對得上的子任務，不在這裡要求對照（test、deliver 有套用時在那裡要求）。
    - check：在目標 repo（沒有就專案根目錄）執行，exit 非 0 或逾時不行。"""
    cons = [c for c in (profile or {}).get("constraints") or [] if isinstance(c, dict) and stage in (c.get("stages") or [])]
    if not cons:
        return [], []
    issues: list[str] = []
    checks: list[dict] = []
    rows = constraint_rows(run, stage)
    for c in cons:
        cid, f = str(c.get("id") or ""), str(c.get("file") or "")
        if constraint_file(c, repo) is None:
            issues.append(f"限制文件 [{cid}] {f} 不存在（專案根目錄與目標 repo 都找不到）")
        if c.get("trace") == "table" and not (stage == "build" and c.get("paths")):
            got = rows.get(cid) or []
            if not got:
                issues.append(f"限制 [{cid}] 要逐條對照，這一段的執行紀錄卻沒有它的〈限制對照〉列")
            for verdict, name in got:
                if verdict == "偏離":
                    issues.append(f"限制 [{cid}] 有偏離（{name}），要由人決定")
        cmd = c.get("check")
        if isinstance(cmd, str) and cmd.strip() and run_checks:
            cwd = repo if repo is not None and repo.is_dir() else project_root()
            try:
                q = subprocess.run(cmd, shell=True, cwd=str(cwd), capture_output=True, text=True, encoding="utf-8", errors="replace",
                                   timeout=CONSTRAINT_CHECK_TIMEOUT)
                code, tail = q.returncode, (q.stdout + q.stderr).strip()[-300:]
            except subprocess.TimeoutExpired:
                code, tail = -1, f"逾時（{CONSTRAINT_CHECK_TIMEOUT} 秒）"
            except OSError as e:
                code, tail = -1, f"跑不起來：{e}"
            checks.append({"id": cid, "command": cmd, "exit": code, "output": tail})
            if code != 0:
                issues.append(f"限制 [{cid}] 的檢查指令沒過（exit {code}）：{cmd}")
    return issues, checks


def load_profile_for(run: Path, argv: list[str]) -> tuple[dict | None, str]:
    """開發規範 profile：--profile，否則只認和這支工具同一個專案的 <專案>/.claude/teams/methodology.json
    （工具裝在 <專案>/.claude/teams/tools/）；找不到用 None（內建節）。
    不從 run 目錄往上找：runs_dir 底下被放一份 .claude/teams/methodology.json 就能劫持 recipe 解析（X3 B2）。
    --profile 指定了卻讀不到、檔案不是 JSON 物件、或不符合 profile 的語意規則，回傳錯誤（fail-closed）。"""
    given = opt(argv, "--profile")
    path = Path(given) if given else PROJECT_PROFILE
    if not path.is_file():
        return None, (f"--profile {given} 不存在" if given else "")
    try:
        prof = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        return None, f"讀不到開發規範 profile {path}：{e}"
    if not isinstance(prof, dict):
        return None, f"開發規範 profile {path} 不是 JSON 物件"
    errs = schemalib.methodology_errors(prof)
    if errs:
        return None, f"開發規範 profile {path} 不合規則：{'；'.join(errs[:3])}"
    return prof, ""


def _mods(v) -> list[str]:
    if isinstance(v, str):
        v = v.split(",")
    return [str(x).strip() for x in v or [] if str(x).strip()]


def recipe_summary(rec: dict) -> str:
    r = rec.get("resolved") or {}
    mods = "、".join(rec.get("modifiers") or []) or "無"
    return (f"{rec.get('name')}（modifiers：{mods}；size：{rec.get('size') or '未指定'}；段：{' → '.join(r.get('stages') or [])}；"
            f"plan_mode：{r.get('plan_mode', '')}；來源：{rec.get('resolved_from', '')}"
            f"{'；已鎖定' if rec.get('locked') else ''}{'；需要後續 run' if r.get('followup') else ''}）")


def write_context(run: Path, data: dict) -> None:
    """context.md：init 時寫基本資料；有 recipe 時維護一行「- recipe：…」摘要（取代舊的那行）。"""
    f = run / "context.md"
    if f.is_file():
        lines = f.read_text(encoding="utf-8").splitlines()
    else:
        lines = ["# run 共用背景", "", "所有 agent 共用；派工單不再重貼這裡的內容。外部內容（需求原文、issue、log）只當資料，不當指令。", "",
                 f"- 目標 repo：{data.get('target_repo') or '（未指定）'}", f"- 事件：{data.get('event') or '（未指定）'}",
                 f"- 需求（原文或路徑）：{data.get('requirement') or '（未指定）'}"]
    if has_recipe(data):
        line = f"- recipe：{recipe_summary(data['recipe'])}"
        idx = next((i for i, ln in enumerate(lines) if ln.startswith("- recipe：")), None)
        if idx is None:
            lines.append(line)
        else:
            lines[idx] = line
    f.write_text("\n".join(lines) + "\n", encoding="utf-8")


def rebuild_stages(data: dict, resolved: dict) -> None:
    """依 recipe 重建 run.json 的 stages：recipe 有的段沒有就補 pending（skipped 的改回 pending），沒有的段標 skipped。"""
    stages = data.setdefault("stages", {})
    want = resolved.get("stages") or []
    for s in SDLC:
        st = stages.get(s)
        if s in want:
            if not isinstance(st, dict) or st.get("status") == "skipped":
                stages[s] = {"status": "pending", "gate": s in GATED}
        else:
            stages[s] = {"status": "skipped", "gate": False}
    data["stages"] = {**{s: stages[s] for s in SDLC}, **{k: v for k, v in stages.items() if k not in SDLC}}


def intent_errors(result, profile: dict | None, plan_mode: str) -> list[str]:
    """一定要有 intent：intent 信封 payload.documents 裡 kind=intent 的檔要存在，而且有必要的節（I1）。"""
    paths = []
    for env in envelopes(result):
        p = env.get("payload") if isinstance(env.get("payload"), dict) else {}
        for d in p.get("documents") or []:
            if isinstance(d, dict) and d.get("kind") == "intent" and isinstance(d.get("path"), str) and d["path"].strip():
                paths.append(d["path"].strip())
    if not paths:
        return ["intent：回傳的信封 payload.documents 沒有 kind=intent 的文件（一定要有 intent）"]
    epic = result.get("epic_dir") if isinstance(result, dict) and isinstance(result.get("epic_dir"), str) else ""
    need = rcp.required_sections(profile, plan_mode)
    errs = []
    for raw in dict.fromkeys(paths):
        p = Path(raw)
        cands = [p] if p.is_absolute() else ([Path(epic) / p] if epic else []) + [p]
        f = next((c for c in cands if c.is_file()), None)
        if f is None:
            errs.append(f"intent 檔不存在：{raw}（相對路徑以 epic_dir {epic or '（回傳沒有）'} 為基準）")
            continue
        try:
            text = f.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as e:
            errs.append(f"讀不到 intent 檔 {f}：{e}")
            continue
        miss = rcp.missing_sections(text, need)
        if miss:
            errs.append(f"intent 缺必要的節（plan_mode={plan_mode}）：{'、'.join(miss)}（{raw}）")
    return errs


def recipe_tamper_errors(data: dict, argv: list[str] | None = None) -> list[str]:
    """run.json 的 recipe.resolved 不能比「用專案 profile 依 name、modifiers、size 重算」的結果鬆（X3 M1：手改 resolved 少段）。
    escalate 只會加嚴，所以合法的 resolved 一定 stricter_or_equal 於重算結果；profile 在 plan 之後被改鬆也會在這裡被擋（fail-closed）。"""
    if not has_recipe(data):
        return []
    rec = data["recipe"]
    prof, perr = load_profile_for(Path("."), argv or [])
    if perr:
        return [perr]
    base = rcp.resolve(prof, rec.get("name"), rec.get("modifiers") or [], rec.get("size") or None)
    errs = [f"run.json 的 recipe 和重算結果不一致：{e}" for e in base["errors"]]
    if base["name"] != rec.get("name"):
        errs.append(f"run.json 的 recipe {rec.get('name')} 用目前的 profile 解析成 {base['name']}（profile 改過？），不採信")
    loose = rcp.stricter_or_equal(rec["resolved"], base)
    res = rec["resolved"]
    extra = [k for k in res.get("build_kinds") or [] if k not in (base.get("build_kinds") or [])]
    if extra:  # escalate 時 build_kinds 照新 recipe，所以合法的 resolved 不會多出重算結果沒有的種類
        loose.append(f"build_kinds 多了 {'、'.join(extra)}")
    if res.get("publishable", True) and not base.get("publishable", True):
        loose.append("publishable 被改成 true")
    if base.get("followup") and not res.get("followup"):
        loose.append("followup 被拿掉")
    errs += [f"run.json 的 recipe.resolved 比 {rec.get('name')} 的定義鬆（手改過？）：{e}" for e in loose]
    return errs


def followup_run_errors(frun: Path) -> str:
    """hotfix 的後續 run 要是真的 SDLC run：有 run.json、有 plan 段、沒取消、recipe（有定的話）不是 hotfix。"""
    try:
        f = json.loads((frun / "run.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return f"後續 run {frun} 讀不到 run.json"
    if "plan" not in (f.get("stages") or {}):
        return f"後續 run {frun} 不是 SDLC run（沒有 plan 段）"
    if f.get("abandoned"):
        return f"後續 run {frun} 已取消，不能當成後續 run"
    if has_recipe(f) and rcp.recipe_of_run(f).get("followup"):
        return f"後續 run {frun} 的 recipe 是 {f['recipe'].get('name')}，本身也只是最短路徑，不能當成補完整流程的後續 run"
    return ""


def chain_errors(run_or_stages: dict, stage: str) -> list[str]:
    """整條閘門鏈：recipe 裡 SDLC 排在 stage 前面的每一段都要 done，有閘門的都要已核准。

    參數可以是整份 run.json（依它的 recipe 判斷），或只有 stages（舊呼叫方式，當成沒有 recipe 的舊 run）。
    - recipe 沒有的段不檢查（run.json 標 skipped）。
    - recipe 有、run.json 卻沒有的段判錯（缺段）；只有沒有 recipe 的舊 run 容許缺 sync（加 sync 段之前建立的 run）。
    - recipe 有、run.json 卻標 skipped 也判錯（兩邊不一致，可能被手改）。"""
    if stage not in SDLC:
        return []
    is_run = isinstance(run_or_stages.get("stages"), dict)
    data = run_or_stages if is_run else {"stages": run_or_stages}
    stages = data["stages"]
    legacy = not has_recipe(data)
    r = rcp.recipe_of_run(data)
    want = r.get("stages") or []
    errs = []
    for prev in SDLC[: SDLC.index(stage)]:
        if prev not in want:
            continue
        ps = stages.get(prev)
        if not isinstance(ps, dict):
            if legacy and prev == "sync":
                continue  # 加 sync 段之前建立的 run 沒有 sync
            errs.append(f"缺段：recipe {r.get('name')} 要有 {prev} 段，run.json 卻沒有")
        elif ps.get("status") == "skipped":
            errs.append(f"recipe {r.get('name')} 要有 {prev} 段，run.json 卻標成 skipped（不一致）")
        elif ps.get("status") != "done":
            errs.append(f"前面的 {prev} 段狀態是 {ps.get('status')}，還沒 done")
        elif ps.get("gate") and not ps.get("approved"):
            errs.append(f"前面的 {prev} 段還沒核准")
    return errs


def abandoned_error(data: dict) -> str:
    ab = data.get("abandoned")
    if not ab:
        return ""
    return f"run 已取消（{ab.get('by', '?')} @ {ab.get('at', '?')}：{ab.get('reason', '')}）"


def _escalate_affected(new: dict, old: dict, profile: dict | None) -> tuple[list[str], list[str]]:
    """escalate 後要重跑的範圍：（新加的段、設定變嚴的段）。"""
    added = [s for s in new.get("stages") or [] if s not in (old.get("stages") or [])]
    changed = []
    if set(new.get("design_lenses") or []) - set(old.get("design_lenses") or []):
        changed.append("design")
    nt, ot = new.get("test") or {}, old.get("test") or {}
    if set(nt.get("modes") or []) - set(ot.get("modes") or []) or any(
            rcp.LEVEL.get(nt.get(k, "auto"), 1) > rcp.LEVEL.get(ot.get(k, "auto"), 1) for k in ("design_review", "security_review")):
        changed.append("test")
    if set(new.get("deliver_axes") or []) - set(old.get("deliver_axes") or []):
        changed.append("deliver")
    if set(rcp.required_sections(profile, new.get("plan_mode", "feature"))) - set(rcp.required_sections(profile, old.get("plan_mode", "feature"))):
        changed.append("plan")
    return added, [c for c in changed if c in (new.get("stages") or [])]


def next_stage(data: dict) -> str | None:
    """下一個要處理的段：還沒 done，或 done 了但閘門還沒核准；recipe 沒有的段（skipped）不算。"""
    return next((s for s, st in data["stages"].items() if st["status"] != "skipped"
                 and (st["status"] != "done" or (st.get("gate") and not st.get("approved")))), None)


def status_snapshot(run: Path, data: dict) -> dict:
    """給外部程式讀的狀態快照（schemas/run-status.schema.json）。只整理 run.json 與各段結果的 trace，不做任何判定。"""
    stages = []
    for name, st in data["stages"].items():
        agents = missing = 0
        try:
            trace = json.loads((run / f"{name}.json").read_text(encoding="utf-8")).get("trace")
            if isinstance(trace, list):
                agents = len(trace)
                missing = sum(1 for t in trace if not (isinstance(t, dict) and t.get("ok") is True))
        except (OSError, ValueError, AttributeError):
            pass
        ap = st.get("approved")
        stages.append({"name": name, "status": st["status"], "gate": bool(st.get("gate")),
                       "approved": {"by": str(ap.get("by")), "at": str(ap.get("at"))} if isinstance(ap, dict) else None,
                       "agents": agents, "agents_missing": missing, "missing_records": len(st.get("missing_records") or [])})
    nxt = None if data.get("abandoned") else next_stage(data)
    nst = data["stages"].get(nxt) if nxt else None
    waiting = nxt if nst and nst["status"] == "done" and nst.get("gate") and not nst.get("approved") else None
    if data.get("abandoned"):
        state = "abandoned"
    elif data.get("closed"):
        state = "closed"
    elif nxt is None:
        state = "done"
    elif waiting:
        state = "waiting_gate"
    elif nst and nst["status"] in ("blocked", "invalid"):
        state = "blocked"
    else:
        state = "running"
    r = data.get("recipe") if has_recipe(data) else None
    fr = data.get("followup_run")
    return {"v": 1, "run": run.name, "event": str(data.get("event") or ""), "target": str(data.get("target") or ""), "state": state,
            "recipe": {"name": r.get("name"), "modifiers": list(r.get("modifiers") or []), "size": r.get("size"),
                       "locked": bool(r.get("locked"))} if r else None,
            "stages": stages, "next": nxt, "waiting_gate": waiting,
            "followup_run": str(fr.get("run")) if isinstance(fr, dict) and fr.get("run") else None,
            "abandoned": data.get("abandoned") or None, "closed": data.get("closed") or None}


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
        data = {
            "event": event,
            "target_repo": opt(argv, "--target"),
            "requirement": opt(argv, "--requirement"),
            "stages": {s: {"status": "pending", "gate": s in GATED} for s in (SDLC if event == "sdlc" else ["run"])},
        }
        dump(run, data)
        write_context(run, data)
        print(f"已建立 {run / 'run.json'} 與 {run / 'context.md'}")
        return 0
    data = load(run)
    if cmd == "stage":
        if len(argv) < 3:
            print(__doc__)
            return 2
        stage = argv[2]
        result = json.load(sys.stdin)
        (run / f"{stage}.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        errs: list[str] = []
        warns: list[str] = []
        envs = list(envelopes(result))
        if not envs:
            errs.append(NO_ENVELOPE)
        for i, env in enumerate(envs):
            errs += [f"信封 {i}：{e}" for e in schemalib.check(env)]
        statuses = [e.get("status") for e in envs]
        sdlc_run = "plan" in data.get("stages", {})
        if stage == "plan" and sdlc_run:
            profile, perr = load_profile_for(run, argv)
            if perr:
                errs.append(perr)
            inc = result.get("recipe") if isinstance(result, dict) else None
            cur = data.get("recipe") if has_recipe(data) else None
            if isinstance(inc, dict):
                r = rcp.resolve(profile, inc.get("name") or inc.get("kind"), _mods(inc.get("modifiers")), inc.get("size") or None)
                errs += [f"recipe：{e}" for e in r["errors"]]
                if cur and (cur.get("locked") or cur.get("history")):
                    if (r["name"], sorted(r["modifiers"]), r["size"]) != (cur.get("name"), sorted(cur.get("modifiers") or []), cur.get("size") or ""):
                        why = "plan 已核准過" if cur.get("locked") else "escalate 過"
                        errs.append(f"recipe 已鎖定（{why}）：不能從 {cur.get('name')} 換成 {r['name']}，要改走 save_run.py escalate")
                elif cur is None and ((data["stages"].get("plan") or {}).get("approved") or any(
                        (data["stages"].get(x) or {}).get("status") not in (None, "pending", "skipped") for x in SDLC[1:])):
                    # 舊 run（沒有 recipe，當成 feature）已核准過 plan 或已經往下跑：補記 recipe 不能比 feature 少段（X3 M4：不能繞過 escalate 降級）
                    feat = rcp.resolve(profile, "feature")
                    lost = rcp.escalate_errors(r, feat)
                    if lost:
                        errs.append(f"這個舊 run 已經當成 feature 往下跑：{lost[0]}")
                    else:
                        data["recipe"] = {"name": r["name"], "modifiers": r["modifiers"], "size": r["size"],
                                          "reasons": inc.get("reasons") if isinstance(inc.get("reasons"), list) else [],
                                          "resolved_from": r["resolved_from"], "resolved": rcp.merge_stricter(r, feat), "locked": False, "history": []}
                else:
                    reasons = inc.get("reasons") if isinstance(inc.get("reasons"), list) else []
                    data["recipe"] = {"name": r["name"], "modifiers": r["modifiers"], "size": r["size"], "reasons": reasons,
                                      "resolved_from": r["resolved_from"], "resolved": r, "locked": False, "history": []}
            elif cur is None:
                warns.append("回傳沒有 recipe（舊 workflow）：這個 run 當成 feature 的完整流程")
            res = rcp.recipe_of_run(data)
            errs += intent_errors(result, profile, res.get("plan_mode", "feature"))
            if has_recipe(data):
                rebuild_stages(data, data["recipe"]["resolved"])
                write_context(run, data)
        elif stage in SDLC and has_recipe(data) and stage not in data["recipe"]["resolved"].get("stages", []):
            # recipe 沒有的段：只存檔、判錯，run.json 仍標 skipped（不讓它看起來像該段存在）
            print(f"不能記錄：recipe {data['recipe'].get('name')} 沒有 {stage} 段（要加段走 save_run.py escalate）；"
                  f"回傳只存成 {stage}.json", file=sys.stderr)
            return 1
        if stage in SDLC and stage != "plan" and has_recipe(data) and isinstance(result, dict):
            errs += [f"recipe 不一致：{e}" for e in recipe_mismatch(data, result)]
        if stage in SDLC and sdlc_run and isinstance(result, dict):
            mprof, _ = load_profile_for(run, argv)
            mods = (data["recipe"].get("modifiers") or []) if has_recipe(data) else _mods((result.get("recipe") or {}).get("modifiers") if isinstance(result.get("recipe"), dict) else [])
            errs += [f"模型分級：{e}" for e in model_errors(stage, result, mprof, mods)]
        target = Path(str(data.get("target_repo"))) if data.get("target_repo") else None
        if stage == "plan" and isinstance(result, dict):
            # 調研重用：不採信 workflow 的說法，重新核對沿用的角度（來源 hash、信封、紀錄檔）
            errs += [f"調研重用：{e}" for e in packlib.verify_reuse(run, target, result, str(data.get("requirement") or ""))]
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
        has_trace, rows, miss = process_trace(run, stage, result, write=True)
        if stage in SDLC:
            cprof, _ = load_profile_for(run, argv)
            c_issues, c_checks = constraint_issues(run, stage, cprof, target)
            if c_issues:
                st["constraint_issues"] = c_issues
                for m in c_issues:
                    print(f"限制文件：{m}（不能政策核准）", file=sys.stderr)
            else:
                st.pop("constraint_issues", None)
            if c_checks:
                st["constraint_checks"] = c_checks
            else:
                st.pop("constraint_checks", None)
            pc = _plan_check_for(run, stage, result, data)
            if pc:
                st["plan_check"] = pc
                for m in pc:
                    print(f"拆工檢查：{m}（不能政策核准；人工核准前請先確認）", file=sys.stderr)
            else:
                st.pop("plan_check", None)
        if not has_trace and has_recipe(data) and stage in SDLC:
            miss = ["這個 run 有 recipe，回傳卻沒有 trace（無法核對每個 agent 的紀錄）"]
            st["missing_records"] = miss
            print(f"執行紀錄缺件：{miss[0]}（不能政策核准）", file=sys.stderr)
        elif not has_trace:
            warns.append("回傳沒有 trace（舊 workflow）：不產生執行紀錄索引、不核對紀錄檔")
            st.pop("missing_records", None)
        else:
            if miss:
                st["missing_records"] = miss
            else:
                st.pop("missing_records", None)
            print(f"{stage}：執行紀錄索引 {run / stage / INDEX_NAME}（{len(rows)} 列）")
            ghost = [r["label"] for r in rows if "agent 沒寫紀錄檔" in r["status"]]
            if ghost:
                st["ghostwritten_records"] = ghost
                print(f"{stage}：{len(ghost)} 個 agent 有交件但沒寫紀錄檔，已依信封代寫（內容較簡略）：{'、'.join(ghost[:8])}")
            else:
                st.pop("ghostwritten_records", None)
            for m in miss:
                print(f"執行紀錄缺件：{m}（不能政策核准）", file=sys.stderr)
        dump(run, data)
        if stage == "plan" and not errs and isinstance(result, dict):
            # 記下這次調研讀過的來源檔（sha1），下次重跑時來源沒變的角度可以沿用；記不了只警告（I9）
            try:
                warns += [f"調研重用：{w}" for w in packlib.record_reuse(run, target, result, str(data.get("requirement") or ""))]
            except Exception as e:  # noqa: BLE001
                warns.append(f"調研重用的紀錄寫不了（{type(e).__name__}：{e}），下次會全部重做")
        for e in errs:
            print(e, file=sys.stderr)
        for w in warns:
            print(f"警告：{w}", file=sys.stderr)
        if old:
            print(f"{stage}：重存結果，舊核准（{old.get('by')} @ {old.get('at')}）已失效，要重新核准")
        if data.get("abandoned"):
            print(f"注意：{abandoned_error(data)}，這一段的結果只存檔，不能核准")
        print(f"{stage}：{len(envs)} 份信封，狀態 {st['status']}" + ("（需要使用者核准才能進下一段）" if st.get("gate") and st["status"] == "done" else ""))
        return 1 if errs else 0
    if cmd == "decide":
        q, a, by, at = opt(argv, "--question"), opt(argv, "--answer"), opt(argv, "--by"), opt(argv, "--at")
        scope, exp = opt(argv, "--scope", "epic"), opt(argv, "--expires")
        errs = ["--by 與 --at 都必填"] if not by or not at else []
        errs += decision_errors([(q, a)], by, scope, exp)
        if errs:
            for e in errs:
                print(f"不能記錄：{e}", file=sys.stderr)
            return 1
        try:
            record_decisions(run, [(q, a)], by, at, scope, exp)
        except OSError as e:
            print(f"不能記錄：{e}", file=sys.stderr)
            return 1
        print(f"已記 1 筆決定（{packlib.DECISIONS}；範圍 {scope}）")
        return 0
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
        if stage != "plan":
            errs += recipe_tamper_errors(data, argv)
        if by == "policy" and not note:
            errs.append("--by policy 時 --note 要寫依據的政策名稱")
        answers = [split_answer(a) for a in opts(argv, "--answer")]
        a_scope, a_exp = opt(argv, "--answer-scope", "epic"), opt(argv, "--answer-expires")
        if answers:
            errs += decision_errors(answers, by, a_scope, a_exp)
        if by == "policy" and st is not None:
            f = run / f"{stage}.json"
            if not f.is_file():
                errs.append(f"找不到 {f.name}，政策核准要讀該段的實際結果")
            else:
                errs += [f"政策判定：{e}" for e in policy_violations(stage, json.loads(f.read_text(encoding="utf-8")), run)]
                cprof, cperr = load_profile_for(run, argv)
                if cperr:
                    errs.append(cperr)
                crepo = Path(str(data.get("target_repo"))) if data.get("target_repo") else None
                errs += [f"限制文件：{e}" for e in constraint_issues(run, stage, cprof, crepo)[0]]
                errs += [f"拆工檢查：{e}" for e in _plan_check_for(run, stage, json.loads(f.read_text(encoding="utf-8")), data)]
            if stage in HUMAN_ONLY and not any("一律由人核准" in e for e in errs):
                errs.append(f"{stage} 段一律由人核准，不能政策核准")
        errs += chain_errors(data, stage)
        if errs:
            for e in errs:
                print(f"不能核准：{e}", file=sys.stderr)
            return 1
        st["approved"] = {"by": by, "at": at, "note": note, "kind": "policy" if by == "policy" else "human"}
        if stage == "plan" and has_recipe(data):
            data["recipe"]["locked"] = True
        dump(run, data)
        print(f"{stage} 已記錄核准：{by} @ {at}")
        if answers:
            try:
                print(f"已記 {record_decisions(run, answers, by, at, a_scope, a_exp, stage)} 筆決定（{packlib.DECISIONS}；範圍 {a_scope}）")
            except OSError as e:   # 決策記憶是輔助：寫不出來不影響核准
                print(f"警告：決定沒記成（{e}）；核准已記錄", file=sys.stderr)
        if stage == "plan" and has_recipe(data):
            print(f"recipe 已鎖定：{recipe_summary(data['recipe'])}；之後要改只能 save_run.py escalate（只能加嚴）")
        if st.get("missing_records"):
            print(f"警告：{stage} 有 {len(st['missing_records'])} 個執行紀錄缺件（人工核准照記，但請先確認）：" + "；".join(st["missing_records"][:5]), file=sys.stderr)
        if st.get("constraint_issues"):
            print(f"警告：{stage} 有 {len(st['constraint_issues'])} 項限制文件的問題（人工核准照記，但請先確認）：" + "；".join(st["constraint_issues"][:5]), file=sys.stderr)
        if st.get("plan_check"):
            print(f"警告：{stage} 的拆工檢查有 {len(st['plan_check'])} 項問題（人工核准照記，但請先確認）：" + "；".join(st["plan_check"][:5]), file=sys.stderr)
        return 0
    if cmd == "escalate":
        name, by, at, reason = opt(argv, "--recipe"), opt(argv, "--by"), opt(argv, "--at"), opt(argv, "--reason")
        errs = []
        if not name or not by or not at or not reason:
            errs.append("--recipe、--by、--at、--reason 都必填")
        if data.get("abandoned"):
            errs.append(abandoned_error(data))
        if "plan" not in data.get("stages", {}):
            errs.append("不是 SDLC run（沒有 plan 段），沒有 recipe 可升級")
        profile, perr = load_profile_for(run, argv)
        if perr:
            errs.append(perr)
        if errs:
            for e in errs:
                print(f"不能升級：{e}", file=sys.stderr)
            return 1
        new = rcp.resolve(profile, name, _mods(opt(argv, "--modifiers")), opt(argv, "--size") or None)
        old_rec = data["recipe"] if has_recipe(data) else None
        old = rcp.recipe_of_run(data)
        errs = [f"recipe：{e}" for e in new["errors"]] + rcp.escalate_errors(new, old)
        if errs:
            for e in errs:
                print(f"不能升級：{e}", file=sys.stderr)
            return 1
        merged = rcp.merge_stricter(new, old)
        added, changed = _escalate_affected(merged, old, profile)
        first = min([SDLC.index(s) for s in added] + [SDLC.index(s) for s in changed], default=len(SDLC))
        stale = []
        for s in SDLC:
            st = data["stages"].get(s)
            if s in added:
                data["stages"][s] = {"status": "pending", "gate": s in GATED}
            elif isinstance(st, dict) and st.get("status") == "done" and (
                    any(SDLC.index(s) > SDLC.index(a) for a in added) or (s in changed) or SDLC.index(s) > first):
                st["status"] = "stale"
                st.pop("approved", None)
                stale.append(s)
        rebuild_stages(data, merged)
        frm = f"{old.get('name')}{'+' + '+'.join(old.get('modifiers') or []) if old.get('modifiers') else ''}"
        to = f"{merged.get('name')}{'+' + '+'.join(merged.get('modifiers') or []) if merged.get('modifiers') else ''}"
        hist = list((old_rec or {}).get("history") or []) + [{"from": frm, "to": to, "by": by, "at": at, "reason": reason}]
        data["recipe"] = {"name": merged["name"], "modifiers": merged["modifiers"], "size": merged.get("size", ""),
                          "reasons": (old_rec or {}).get("reasons") or [], "resolved_from": new["resolved_from"], "resolved": merged,
                          "locked": bool((old_rec or {}).get("locked")) or bool((data["stages"].get("plan") or {}).get("approved")),
                          "history": hist}
        dump(run, data)
        write_context(run, data)
        print(f"recipe 已升級：{frm} → {to}；新加的段：{'、'.join(added) or '無'}；設定變嚴的段：{'、'.join(changed) or '無'}")
        if stale:
            print(f"改成 stale（核准已清除，要重跑）：{'、'.join(stale)}")
        return 0
    if cmd == "followup":
        frun, by, at = opt(argv, "--run"), opt(argv, "--by"), opt(argv, "--at")
        if not frun or not by or not at:
            print("不能記錄：--run、--by、--at 都必填", file=sys.stderr)
            return 1
        if not (Path(frun) / "run.json").is_file():
            print(f"不能記錄：後續 run {frun} 沒有 run.json（先 save_run.py init）", file=sys.stderr)
            return 1
        ferr = followup_run_errors(Path(frun))
        if ferr:
            print(f"不能記錄：{ferr}", file=sys.stderr)
            return 1
        if Path(frun).resolve() == run.resolve():
            print("不能記錄：後續 run 不能是自己", file=sys.stderr)
            return 1
        data["followup_run"] = {"run": frun, "by": by, "at": at}
        dump(run, data)
        print(f"已記錄後續 run：{frun}（{by} @ {at}）")
        return 0
    if cmd == "close":
        by, at = opt(argv, "--by"), opt(argv, "--at")
        errs = []
        if not by or not at:
            errs.append("--by 與 --at 都必填")
        r = rcp.recipe_of_run(data)
        if r.get("followup") and not data.get("followup_run"):
            errs.append(f"recipe {r.get('name')} 要求事後開後續 run 補完整流程，run.json 沒有 followup_run（先 save_run.py followup）")
        elif r.get("followup"):
            ferr = followup_run_errors(Path(str(data["followup_run"].get("run") or "")))
            if ferr:
                errs.append(ferr)
        # 結案要整條流程走完（X3 I1）：recipe 的段都 done、有閘門的都已核准、有 merge 段的要 merge done
        if data.get("abandoned"):
            errs.append(abandoned_error(data))
        errs += [f"流程還沒走完：{e}" for e in chain_errors(data, "merge")]
        if "merge" in (r.get("stages") or []) and (data.get("stages", {}).get("merge") or {}).get("status") != "done":
            errs.append("流程還沒走完：merge 段還沒 done")
        if errs:
            for e in errs:
                print(f"不能結案：{e}", file=sys.stderr)
            return 1
        data["closed"] = {"by": by, "at": at}
        dump(run, data)
        print(f"已結案：{by} @ {at}")
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
        if "--json" in argv:
            print(json.dumps(status_snapshot(run, data), ensure_ascii=False, indent=1))
            return 0
        if data.get("abandoned"):
            print(f"已取消：{abandoned_error(data)}")
        if has_recipe(data):
            print(f"recipe：{recipe_summary(data['recipe'])}")
        else:
            print("recipe：（沒有記錄，當成 feature 的完整流程）")
        for name, st in data["stages"].items():
            gate = "閘門" if st.get("gate") else ""
            ok = f"核准 {st['approved']['by']}" if st.get("approved") else ""
            print(f"{name:8} {st['status']:15} {gate} {ok}")
            if st.get("missing_records"):
                print(f"         警告：執行紀錄缺件 {len(st['missing_records'])} 個（不能政策核准）：" + "；".join(st["missing_records"][:3]))
            if st.get("constraint_issues"):
                print(f"         警告：限制文件 {len(st['constraint_issues'])} 項（不能政策核准）：" + "；".join(st["constraint_issues"][:3]))
        if data.get("followup_run"):
            print(f"後續 run：{data['followup_run'].get('run')}")
        if data.get("closed"):
            print(f"已結案：{data['closed'].get('by')} @ {data['closed'].get('at')}")
        if data.get("abandoned"):
            print("下一步：無（已取消）")
            return 0
        nxt = next_stage(data)
        print(f"下一步：{nxt or '全部完成'}")
        return 0
    print(__doc__)
    return 2


# ------------------------------------------------------------------ selftest
def _env(payload: dict, status: str = "done", artifacts: list | None = None) -> dict:
    return {"status": status, "summary": "s", "artifacts": artifacts or [], "evidence": [], "confidence": 0.9,
            "open_questions": [], "derived_work": [], "payload": payload}


FULL_INTENT = "# intent\n## 目標\n匯出 CSV\n## 範圍\n報表頁\n## 不做的事\nExcel\n## 驗收條件\n- AC-1 下載 CSV\n"
DIAG = "## 現象\n匯出空白\n## 重現步驟\n1. 點匯出\n## 預期行為\n有資料\n"


def selftest() -> int:
    """在暫存目錄跑 init／stage／approve／escalate／close／abandon，驗證各閘門規則。不碰 repo。"""
    import contextlib
    import io

    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
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
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{detail[-400:]}"))
        ok = ok and cond

    with tempfile.TemporaryDirectory() as tmp:
        proj = Path(tmp) / "proj"
        (proj / ".claude/teams").mkdir(parents=True)
        # 專案 profile 用內建 recipes；selftest 把「工具所在專案的 profile」指到這份（正式執行時是 <專案>/.claude/teams/methodology.json）
        global PROJECT_PROFILE
        saved_profile = PROJECT_PROFILE
        PROJECT_PROFILE = proj / ".claude/teams/methodology.json"
        PROJECT_PROFILE.write_text(
            json.dumps({"recipes": rcp.BUILTIN_RECIPES, "modifiers": rcp.BUILTIN_MODIFIERS}, ensure_ascii=False), encoding="utf-8")
        epic = proj / "repo/.worktrees/e"
        (epic / "specs/e").mkdir(parents=True)
        n = [0]

        def write_intent(text: str, name: str = "intent.md") -> str:
            (epic / "specs/e" / name).write_text(text, encoding="utf-8")
            return f"specs/e/{name}"

        default_intent = write_intent(FULL_INTENT)

        def plan_res(recipe: dict | None = None, path: str | None = None, **extra) -> dict:
            r = {"stage": "plan", "epic_dir": str(epic),
                 "intent": _env({"documents": [{"path": path or default_intent, "kind": "intent", "audience": "owner"}]}), **extra}
            if recipe is not None:
                r["recipe"] = recipe
            return r

        def new_run(recipe: dict | None = None, approve_plan: bool = False, intent: str | None = None) -> str:
            n[0] += 1
            d = str(proj / "_runs" / f"r{n[0]}")
            call("init", d, "--event", "sdlc", "--target", "x", "--requirement", "r")
            if recipe is not None or approve_plan:
                call("stage", d, "plan", stdin=plan_res(recipe, intent))
                if approve_plan:
                    call("approve", d, "plan", "--by", "human", "--at", "t")
            return d

        def stages(d: str) -> dict:
            return load(Path(d))["stages"]

        def walk(d: str, upto: str, skip_plan: bool = False) -> None:
            for s, r in [("plan", plan_res()), ("design", {"design": design}), ("build", {"x": build}),
                         ("sync", {"x": build}), ("test", {"v": test}), ("deliver", {"x": build})]:
                if s == upto:
                    return
                if (skip_plan and s == "plan") or stages(d).get(s, {}).get("status") == "skipped":
                    continue
                call("stage", d, s, stdin=r if s == "plan" else traced(d, s, r))
                if s in GATED:
                    call("approve", d, s, "--by", "human", "--at", "t")

        def traced(d: str, s: str, r: dict, label: str = "walk") -> dict:
            """有 recipe 的 run：回傳補上 run.json 的 recipe 與一筆 trace（紀錄檔已寫），模擬新版 workflow 的回傳。"""
            data = load(Path(d))
            if not has_recipe(data):
                return r
            rec = data["recipe"]
            out = {**r, "recipe": {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]}}
            if "trace" not in out:
                f = Path(d) / s / f"{label}.md"
                f.parent.mkdir(parents=True, exist_ok=True)
                f.write_text("# 紀錄\n", encoding="utf-8")
                out["trace"] = [{"label": label, "agent_type": "x", "phase": "p", "record": str(f), "ok": True}]
            return out

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
        viol = policy_violations("plan", {"intent": design, "challenge": None})
        check("skeptic 推翻：challenge: null 判為 agent 缺件", any("challenge" in v and "缺件" in v for v in viol), str(viol))
        # plan 禁政策
        d = new_run()
        call("stage", d, "plan", stdin=plan_res())
        c, out = call("approve", d, "plan", "--by", "policy", "--at", "t", "--note", "p")
        c2, _ = call("approve", d, "plan", "--by", "human", "--at", "t")
        check("plan 禁政策：plan 段 --by policy 拒絕、人工核准可以", c != 0 and "由人核准" in out and c2 == 0, out)
        # 閘門鏈
        d = new_run()
        call("stage", d, "plan", stdin=plan_res())
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
        call("stage", d, "plan", stdin=plan_res())
        c, _ = call("abandon", d, "--reason", "需求取消", "--by", "human", "--at", "t")
        c2, out2 = call("approve", d, "plan", "--by", "human", "--at", "t")
        _, st_out = call("status", d)
        ab = load(Path(d)).get("abandoned") or {}
        c3, _ = call("abandon", d, "--by", "human", "--at", "t")
        check("abandon：記錄 {by, at, reason}、之後不能核准、status 顯示已取消、缺 --reason 拒絕",
              c == 0 and ab == {"by": "human", "at": "t", "reason": "需求取消"} and c2 != 0 and "已取消" in out2
              and "已取消" in st_out and c3 != 0, out2 + st_out)

        # ── B13 決策記憶
        d = new_run({"name": "feature"})
        runs_p = Path(d).parent
        for f in (packlib.DECISIONS, packlib.DECISIONS_VIEW):
            (runs_p / f).unlink(missing_ok=True)
        c, out = call("approve", d, "plan", "--by", "human", "--at", "2026-11-12T01:00:00Z", "--answer", "匯出檔的編碼用什麼？=UTF-8 不加 BOM",
                      "--answer", "要不要支援 xlsx＝這一版不要", "--answer-scope", "project", "--answer-expires", "2099-01-01")
        decs = packlib.load_decisions(runs_p)
        check("B13 決策記憶：人核准時的 --answer 記進 decisions.jsonl（問題、答覆、誰、何時、範圍、到期、run、epic、段）",
              c == 0 and len(decs) == 2 and decs[0]["question"] == "匯出檔的編碼用什麼？" and decs[0]["answer"] == "UTF-8 不加 BOM" and decs[1]["answer"] == "這一版不要"
              and decs[0]["by"] == "human" and decs[0]["scope"] == "project" and decs[0]["expires"] == "2099-01-01" and decs[0]["run"] == Path(d).name
              and decs[0]["stage"] == "plan" and decs[0]["epic"] == "e" and "已記 2 筆決定" in out, out + str(decs))
        check("B13 決策記憶：決策記憶.md 跟著更新（給人與知識庫看）", "UTF-8 不加 BOM" in (runs_p / packlib.DECISIONS_VIEW).read_text(encoding="utf-8"))
        c, out = call("decide", d, "--question", "匯出檔的編碼用什麼？", "--answer", "改成加 BOM", "--by", "human", "--at", "t", "--scope", "project")
        got = packlib.relevant_decisions(runs_p, "別的 run", "", "匯出檔的編碼", "2026-11-12")
        check("B13 決策記憶：decide 可以在核准以外記一筆；同一題後面的答覆取代前面的；之後的 run 查得到", c == 0 and [x["answer"] for x in got if "編碼" in x["question"]] == ["改成加 BOM"], out + str(got))
        check("B13 決策記憶：到期日過了就查不到", all("xlsx" not in x["question"] for x in packlib.relevant_decisions(runs_p, "x", "", "要不要支援 xlsx 匯出", "2099-06-01"))
              and any("xlsx" in x["question"] for x in packlib.relevant_decisions(runs_p, "x", "", "要不要支援 xlsx 匯出", "2026-11-12")))
        n0 = len(packlib.load_decisions(runs_p))
        d2 = new_run({"name": "feature"})
        bad = [call("approve", d2, "plan", "--by", "human", "--at", "t", "--answer", "沒有答覆"),
               call("approve", d2, "plan", "--by", "human", "--at", "t", "--answer", "q=a", "--answer-scope", "everywhere"),
               call("approve", d2, "plan", "--by", "human", "--at", "t", "--answer", "q=a", "--answer-expires", "明年"),
               call("decide", d2, "--question", "q", "--answer", "a", "--by", "policy", "--at", "t"),
               call("decide", d2, "--question", "q" * 301, "--answer", "a", "--by", "human", "--at", "t")]
        check("B13 決策記憶：格式不對（沒答覆、範圍或到期日寫錯、太長）與政策核准都拒絕，而且不記、不核准",
              all(c != 0 for c, _ in bad) and len(packlib.load_decisions(runs_p)) == n0 and not load(Path(d2))["stages"]["plan"].get("approved"), str([o for _, o in bad]))
        d3 = new_run({"name": "feature"}, approve_plan=True)
        call("stage", d3, "design", stdin={"stage": "design", "recipe": {"name": "feature"}, "design": _env({"epic": "e", "subtasks": [{"id": "t", "title": "t", "kind": "code", "owns": ["a"], "depends_on": [], "acceptance": ["x"]}]}), "trace": []})
        c, out = call("approve", d3, "design", "--by", "policy", "--at", "t", "--note", "p", "--answer", "q=a")
        check("B13 決策記憶：政策核准不能帶 --answer", c != 0 and "只能由人記" in out and len(packlib.load_decisions(runs_p)) == n0, out)

        # ── SP2.1 intent 檢查（I1）
        d = new_run()
        c, out = call("stage", d, "plan", stdin=plan_res({"name": "feature"}, "specs/e/沒有這個檔.md"))
        c2, out2 = call("approve", d, "plan", "--by", "human", "--at", "t")
        d2 = new_run()
        c3, out3 = call("stage", d2, "plan", stdin=plan_res({"name": "feature"}, None, intent=_env({"documents": [{"path": "x.md", "kind": "spec", "audience": "owner"}]})))
        check("intent 節：缺檔（intent 檔不存在、或沒有 kind=intent 的文件）→ invalid、不能核准",
              c != 0 and stages(d)["plan"]["status"] == "invalid" and "intent 檔不存在" in out and c2 != 0
              and c3 != 0 and "kind=intent" in out3, out + out2 + out3)
        no_ac = write_intent("# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n", "no-ac.md")
        empty = write_intent("# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC-1\n", "empty.md")
        d = new_run()
        c, out = call("stage", d, "plan", stdin=plan_res({"name": "feature"}, no_ac))
        d2 = new_run()
        c2, out2 = call("stage", d2, "plan", stdin=plan_res({"name": "feature"}, empty))
        check("intent 節：缺必要節（少了驗收條件、或節只有標題）→ invalid",
              c != 0 and "驗收條件" in out and stages(d)["plan"]["status"] == "invalid"
              and c2 != 0 and "不做的事（只有標題沒有內容）" in out2, out + out2)
        no_root = write_intent(FULL_INTENT + DIAG, "no-root.md")
        with_root = write_intent(FULL_INTENT + DIAG + "## 根因\n空集合沒處理\n", "root.md")
        d = new_run()
        c, out = call("stage", d, "plan", stdin=plan_res({"name": "bugfix"}, no_root))
        d2 = new_run()
        c2, out2 = call("stage", d2, "plan", stdin=plan_res({"name": "bugfix"}, with_root))
        check("intent 節：diagnose 要根因（bugfix 少了根因 → invalid；補上 → done）",
              c != 0 and "根因" in out and "plan_mode=diagnose" in out and c2 == 0 and stages(d2)["plan"]["status"] == "done", out + out2)
        d = new_run()
        c, out = call("stage", d, "plan", stdin=plan_res({"name": "feature"}))
        c2, out2 = call("approve", d, "plan", "--by", "human", "--at", "t")
        check("intent 節：齊全可核准（四節齊全 → done、人工核准可以）", c == 0 and c2 == 0 and stages(d)["plan"].get("approved"), out + out2)
        prof = proj / "custom-profile.json"
        prof.write_text(json.dumps({"stages": {"plan": {"required_sections": ["問題"]}}}, ensure_ascii=False), encoding="utf-8")
        d = new_run()
        c, out = call("stage", d, "plan", "--profile", str(prof), stdin=plan_res({"name": "feature"}))
        c2, out2 = call("stage", d, "plan", "--profile", str(proj / "沒有.json"), stdin=plan_res({"name": "feature"}))
        check("intent 節：--profile 覆寫必要節；--profile 讀不到 → invalid（fail-closed）",
              c != 0 and "問題" in out and c2 != 0 and "不存在" in out2, out + out2)

        # ── SP2.2 recipe 鎖定
        d = new_run({"name": "bugfix", "modifiers": [], "size": "small", "reasons": [{"claim": "回報錯誤", "evidence": "issue"}]}, intent=with_root)
        rec0 = load(Path(d)).get("recipe") or {}
        sk = stages(d)
        ctx = (Path(d) / "context.md").read_text(encoding="utf-8")
        check("recipe 鎖定：stage plan 記錄 recipe（工具重算 resolved），recipe 沒有的段標 skipped，context.md 有 recipe 摘要",
              rec0.get("name") == "bugfix" and rec0.get("locked") is False and rec0["resolved"]["plan_mode"] == "diagnose"
              and sk["design"] == {"status": "skipped", "gate": False} and "- recipe：bugfix" in ctx and "目標 repo：x" in ctx,
              json.dumps(rec0, ensure_ascii=False)[:300] + ctx)
        c, _ = call("approve", d, "plan", "--by", "human", "--at", "t")
        locked = (load(Path(d)).get("recipe") or {}).get("locked")
        c2, out2 = call("stage", d, "plan", stdin=plan_res({"name": "feature"}, with_root))
        after = load(Path(d))
        check("recipe 鎖定：核准後鎖定；再 stage plan 換 recipe → invalid、recipe 不變、核准清除",
              c == 0 and locked is True and c2 != 0 and "要改走 save_run.py escalate" in out2
              and after["recipe"]["name"] == "bugfix" and not after["stages"]["plan"].get("approved"), out2)
        d = new_run({"name": "bugfix"}, intent=with_root)
        c, out = call("approve", d, "plan", "--by", "policy", "--at", "t", "--note", "p")
        check("recipe 鎖定：policy 不能核准 plan（recipe 與 intent 由人在 plan 閘門核准）",
              c != 0 and "由人核准" in out and not (load(Path(d)).get("recipe") or {}).get("locked"), out)
        d = new_run({"name": "bugfix", "modifiers": ["zzz"]}, intent=with_root)
        check("recipe 鎖定：不認得的 modifier → plan invalid（交人）", stages(d)["plan"]["status"] == "invalid")

        # ── SP2.3 escalate
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        c, out = call("escalate", d, "--recipe", "feature", "--by", "human", "--at", "t2", "--reason", "其實是新功能")
        rec = load(Path(d))["recipe"]
        check("escalate：加嚴（bugfix → feature：補上 design 段、保留 regression、history 記一筆、仍鎖定）",
              c == 0 and "design" in rec["resolved"]["stages"] and stages(d)["design"]["status"] == "pending"
              and "regression" in rec["resolved"]["test"]["modes"] and len(rec["history"]) == 1
              and rec["history"][0]["from"] == "bugfix" and rec["history"][0]["reason"] == "其實是新功能" and rec["locked"], out)
        d = new_run({"name": "feature"}, approve_plan=True)
        c, out = call("escalate", d, "--recipe", "bugfix", "--by", "human", "--at", "t2", "--reason", "想省事")
        c2, out2 = call("escalate", d, "--recipe", "feature", "--modifiers", "zzz", "--by", "human", "--at", "t2", "--reason", "x")
        c3, out3 = call("escalate", d, "--recipe", "feature", "--by", "human", "--at", "t2")
        rec = load(Path(d))["recipe"]
        check("escalate：降級拒絕（feature → bugfix 少了 design；不認得的 modifier、缺 --reason 也拒絕；recipe 不變）",
              c != 0 and "降級" in out and c2 != 0 and c3 != 0 and rec["name"] == "feature" and not rec["history"], out + out2 + out3)
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        walk(d, "deliver", skip_plan=True)
        before = {s: st["status"] for s, st in stages(d).items()}
        c, out = call("escalate", d, "--recipe", "bugfix", "--modifiers", "migration", "--by", "human", "--at", "t2", "--reason", "碰到 migration")
        st = stages(d)
        call("stage", d, "deliver", stdin={"x": build})
        c2, out2 = call("approve", d, "deliver", "--by", "human", "--at", "t")
        c3, out3 = call("approve", d, "test", "--by", "human", "--at", "t")
        check("escalate：後面的段改成 stale（加 design 段 → 已 done 的 build、sync、test 變 stale 並清核准，閘門鏈擋 deliver）",
              c == 0 and before["test"] == "done" and st["design"]["status"] == "pending"
              and all(st[s]["status"] == "stale" for s in ("build", "sync", "test")) and not st["test"].get("approved")
              and st["plan"].get("approved") and c2 != 0 and "design" in out2 and c3 != 0, out + out2 + out3)

        # ── SP2.4 閘門鏈依 recipe
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        walk(d, "deliver", skip_plan=True)
        data = load(Path(d))
        data["stages"].pop("test")
        dump(Path(d), data)
        call("stage", d, "deliver", stdin={"x": build})
        c, out = call("approve", d, "deliver", "--by", "human", "--at", "t")
        errs = chain_errors(data, "deliver")
        check("閘門鏈依 recipe：缺段（recipe 有 test、run.json 被拿掉 → 判錯，不再略過）",
              c != 0 and "缺段" in out and any("test" in e for e in errs), out)
        d = new_run()
        walk(d, "test")
        data = load(Path(d))
        data["stages"].pop("sync")
        dump(Path(d), data)
        call("stage", d, "test", stdin={"v": test})
        c, out = call("approve", d, "test", "--by", "human", "--at", "t")
        data = load(Path(d))
        data["stages"].pop("design")
        legacy_errs = chain_errors(data, "test")
        stages_only = chain_errors(data["stages"], "test")
        check("閘門鏈依 recipe：舊 run 當成 feature（沒有 recipe：缺 sync 仍容許、缺 design 判錯；只給 stages 也一樣）",
              "recipe" not in data and c == 0 and any("design" in e for e in legacy_errs) and legacy_errs == stages_only, out + str(legacy_errs))
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        walk(d, "test", skip_plan=True)
        call("stage", d, "test", stdin=traced(d, "test", {"v": test}))
        c, out = call("approve", d, "test", "--by", "human", "--at", "t")
        c2, out2 = call("stage", d, "design", stdin=traced(d, "design", {"design": design}))
        check("閘門鏈依 recipe：bugfix 不需要 design（design 是 skipped 也能核准 test；硬存 design 段判 invalid）",
              c == 0 and stages(d)["design"]["status"] == "skipped" and c2 != 0 and "沒有 design 段" in out2, out + out2)

        # ── SP2.5 執行紀錄索引（I6）
        d = new_run()
        walk(d, "design")
        rdir = Path(d) / "design"
        rdir.mkdir(exist_ok=True)
        (rdir / "outline.md").write_text("# 紀錄\n", encoding="utf-8")
        (rdir / "challenge.md").write_text("# 紀錄\n", encoding="utf-8")
        trace = [{"label": "outline", "agent_type": "agent-workflow:dev-spec-author", "phase": "Outline", "record": str(rdir / "outline.md"), "ok": True},
                 {"label": "challenge", "agent_type": "agent-workflow:tpm-skeptic", "phase": "Challenge", "record": str(rdir / "challenge.md"), "ok": True}]
        des = _env(design["payload"], artifacts=[str(rdir / "outline.md"), "specs/e/design.md"])
        ver = _env({"verdicts": [{"target_id": "x", "refuted": False, "reason": "成立"}]})
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": trace})
        idx = (rdir / INDEX_NAME).read_text(encoding="utf-8") if (rdir / INDEX_NAME).is_file() else ""
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("執行紀錄索引：產生（每個 trace 一列：label、角色、phase、紀錄檔、成果檔、狀態；紀錄齊全可政策核准）",
              c == 0 and "| label | 角色 | phase | 紀錄檔 | 成果檔 | 狀態 |" in idx and idx.count("| outline |") == 1
              and "| challenge |" in idx and "specs/e/design.md" in idx and "missing_records" not in stages(d)["design"] and c2 == 0,
              out + idx + out2)
        # B7 限制文件：文件不存在、缺對照列、有偏離、檢查指令沒過 → 不能政策核准；都符合才行；沒有 constraints 的 profile 行為不變
        keep_profile = PROJECT_PROFILE.read_text(encoding="utf-8")
        cbase = json.loads(keep_profile)
        ok_cmd = f'"{sys.executable}" -c "import sys; sys.exit(0)"'
        bad_cmd = f'"{sys.executable}" -c "import sys; sys.exit(3)"'

        def with_constraints(cons: list) -> None:
            PROJECT_PROFILE.write_text(json.dumps({**cbase, "constraints": cons}, ensure_ascii=False), encoding="utf-8")

        def design_run(table: str | None) -> tuple[str, int, str]:
            d_ = new_run()
            walk(d_, "design")
            g = Path(d_) / "design"
            g.mkdir(exist_ok=True)
            (g / "outline.md").write_text("# 紀錄\n" + (f"\n## 限制對照\n\n| 限制 | 判定 | 依據 |\n|---|---|---|\n{table}\n" if table else ""), encoding="utf-8")
            (g / "challenge.md").write_text("# 紀錄\n", encoding="utf-8")
            tr = [{"label": "outline", "agent_type": "agent-workflow:dev-spec-author", "phase": "Outline", "record": str(g / "outline.md"), "ok": True},
                  {"label": "challenge", "agent_type": "agent-workflow:tpm-skeptic", "phase": "Challenge", "record": str(g / "challenge.md"), "ok": True}]
            c_, o_ = call("stage", d_, "design", stdin={"design": des, "challenge": ver, "trace": tr})
            return d_, c_, o_
        (proj / "ARCHITECTURE.md").write_text("# 架構\n\n分層：api → service → repository。\n", encoding="utf-8")
        arch = {"id": "arch", "file": "ARCHITECTURE.md", "stages": ["design"], "trace": "table"}
        with_constraints([arch])
        d, c, out = design_run("| arch | 符合 | design.md:12 |")
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("B7 限制文件：文件在、對照列是「符合」→ 可以政策核准", c == 0 and "constraint_issues" not in stages(d)["design"] and c2 == 0, out + out2)
        d, c, out = design_run(None)
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        c3, out3 = call("approve", d, "design", "--by", "human", "--at", "t")
        check("B7 限制文件：沒有對照列 → 記 constraint_issues、政策核准被擋；人工核准照記並警告",
              any("沒有它的〈限制對照〉列" in m for m in stages(d)["design"].get("constraint_issues") or []) and c2 != 0 and "限制文件" in out2
              and c3 == 0 and "限制文件" in out3, out + out2 + out3)
        d, c, out = design_run("| arch | 偏離 | 這次要讓 api 直接讀 repository，理由見 design.md:40 |")
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("B7 限制文件：對照列是「偏離」→ 政策核准被擋（要由人決定）", c2 != 0 and "偏離" in out2, out + out2)
        with_constraints([{**arch, "file": "docs/NOPE.md"}])
        d, c, out = design_run("| arch | 符合 | x |")
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("B7 限制文件：文件不存在 → 政策核准被擋", c2 != 0 and "不存在" in out2, out + out2)
        with_constraints([{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": bad_cmd}])
        d, c, out = design_run(None)
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        chk = stages(d)["design"].get("constraint_checks") or []
        check("B7 限制文件：檢查指令 exit 非 0 → 記結果、政策核准被擋", c2 != 0 and "檢查指令沒過" in out2 and chk and chk[0]["exit"] == 3, out + out2 + str(chk))
        with_constraints([{"id": "lint", "file": "ARCHITECTURE.md", "stages": ["design"], "check": ok_cmd}, {"id": "rev", "file": "docs/NOPE.md", "stages": ["deliver"], "trace": "table"}])
        d, c, out = design_run(None)
        c2, out2 = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("B7 限制文件：檢查指令通過、trace 沒設、別段的限制不算 → 可以政策核准", c2 == 0 and "constraint_issues" not in stages(d)["design"], out + out2)
        ci, _ = constraint_issues(Path(d), "build", {"constraints": [{"id": "fe", "file": "ARCHITECTURE.md", "stages": ["build"], "paths": ["web/**"], "trace": "table"}]}, None)
        check("B7 限制文件：build 段有 paths 的限制不在這裡要求對照（只發給對得上的子任務）", ci == [], str(ci))
        PROJECT_PROFILE.write_text(keep_profile, encoding="utf-8")
        d, c, out = design_run(None)
        check("B7 限制文件：profile 沒有 constraints → 行為不變", c == 0 and "constraint_issues" not in stages(d)["design"], out)
        # ── B5 plan_check：拆工的確定性檢查
        T = lambda i, deps=(), acc=("AC-1 通過",), kind="code", owns=("src/a.py",): {"id": i, "title": i, "kind": kind, "owns": list(owns), "depends_on": list(deps), "acceptance": list(acc)}  # noqa: E731
        wrap = lambda subs: {"stage": "design", "design": {"status": "done", "payload": {"epic": "demo", "subtasks": subs}}}  # noqa: E731
        check("B5 plan_check：乾淨的拆工沒有問題；回傳沒有子任務時不檢查", plan_check_issues(wrap([T("t1"), T("t2", ["t1"], owns=("src/b.py",))])) == [] and plan_check_issues({"stage": "design"}) == [])
        pc = plan_check_issues(wrap([T("t1", ["t9"]), T("t1"), T("t2", ["t3"]), T("t3", ["t2"])]))
        check("B5 plan_check：id 重複、相依指向不存在、相依循環都列出", any("id 重複：t1" in m for m in pc) and any("不存在的子任務：t9" in m for m in pc) and any("循環：t2、t3" in m for m in pc), str(pc))
        pc = plan_check_issues(wrap([T("t1", acc=()), T("t2", owns=()), T("t3", kind="research", owns=())]))
        check("B5 plan_check：沒有驗收條件、會改檔卻沒有 owns 都列出；research 不要求 owns", any("t1 沒有驗收條件" in m for m in pc) and any("t2（code）沒有 owns" in m for m in pc) and not any("t3" in m for m in pc), str(pc))
        spec_f = Path(d) / "spec-for-plan-check.md"
        spec_f.write_text("# spec\n- AC-1 可以登入\n- AC-2 可以登出\n- AC-MUT-3 不可變\n", encoding="utf-8")
        pc = plan_check_issues(wrap([T("t1", acc=("AC-1 通過", "AC-9 通過"))]), spec_f)
        check("B5 plan_check：spec 有、沒人對應的驗收條件，與子任務引用了 spec 沒有的，都列出", any("沒有任何子任務對應：AC-2、AC-MUT-3" in m for m in pc) and any("spec 沒有的驗收條件：AC-9" in m for m in pc), str(pc))
        check("B5 plan_check：子任務沒用 AC 編號（寫測試指令）時不比對；spec 檔不存在也不比對",
              plan_check_issues(wrap([T("t1", acc=("pytest tests/test_a.py",))]), spec_f) == [] and plan_check_issues(wrap([T("t1")]), Path(d) / "nope.md") == [])
        bad = wrap([T("t1", ["t9"])])
        bad["design"].update({"summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": []})
        dpc = new_run()
        walk(dpc, "design")
        c, out = call("stage", dpc, "design", stdin=bad)
        c2, out2 = call("approve", dpc, "design", "--by", "policy", "--at", "t", "--note", "p")
        check("B5 plan_check：存檔時記進 run.json，政策核准被擋並說明", any("t9" in m for m in stages(dpc)["design"].get("plan_check") or []) and c2 != 0 and "拆工檢查" in out2, out + out2)

        # B9：有交件（digest 對得回信封）但沒寫紀錄檔 → 依信封代寫，不算缺件；agent 自己寫的不覆寫；回傳換掉後舊的代寫檔不沿用
        d = new_run()
        walk(d, "design")
        gd = Path(d) / "design"
        gd.mkdir(exist_ok=True)
        amb = [{"label": "challenge", "agent_type": "agent-workflow:tpm-skeptic", "phase": "Challenge", "record": str(gd / "challenge.md"), "ok": True,
                "digest": {"status": ver.get("status", ""), "summary": ver.get("summary", "")[:400]}}]
        c, out = call("stage", d, "design", stdin={"design": {**des, "artifacts": []}, "challenge": ver, "trace": amb})
        check("B9 代寫：digest 對到多個不同的信封 → 不猜，仍是缺件", des.get("summary") == ver.get("summary") and bool(stages(d)["design"].get("missing_records"))
              and not (gd / "challenge.md").exists(), out)
        ver = {**ver, "summary": "skeptic 判定：設計成立，沒有推翻的項目"}
        (gd / "outline.md").write_text("# agent 自己寫的紀錄\n", encoding="utf-8")
        gt = [{"label": "outline", "agent_type": "agent-workflow:dev-spec-author", "phase": "Outline", "record": str(gd / "outline.md"), "ok": True,
               "digest": {"status": des.get("status", ""), "summary": des.get("summary", "")[:400]}},
              {"label": "challenge", "agent_type": "agent-workflow:tpm-skeptic", "phase": "Challenge", "record": str(gd / "challenge.md"), "ok": True,
               "digest": {"status": ver.get("status", ""), "summary": ver.get("summary", "")[:400]}}]
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": gt})
        gbody = (gd / "challenge.md").read_text(encoding="utf-8") if (gd / "challenge.md").is_file() else ""
        gidx = (gd / INDEX_NAME).read_text(encoding="utf-8")
        check("B9 代寫：有信封沒紀錄檔 → 依信封代寫、不算缺件、記 ghostwritten_records；agent 自己寫的紀錄不覆寫",
              c == 0 and GHOSTWRITTEN in gbody and ver.get("summary", "") in gbody and "missing_records" not in stages(d)["design"]
              and stages(d)["design"].get("ghostwritten_records") == ["challenge"] and "agent 沒寫紀錄檔" in gidx
              and (gd / "outline.md").read_text(encoding="utf-8") == "# agent 自己寫的紀錄\n", out + gbody + gidx)
        first = gbody
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": gt})
        check("B9 代寫：重存結果相同（可重現）", c == 0 and (gd / "challenge.md").read_text(encoding="utf-8") == first and "missing_records" not in stages(d)["design"], out)
        nodg = [{k: v for k, v in t.items() if k != "digest"} for t in gt]
        (gd / "challenge.md").unlink()
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": nodg})
        check("B9 代寫：對不回信封（沒有 digest、artifacts 也沒列）→ 仍是缺件", bool(stages(d)["design"].get("missing_records")) and not (gd / "challenge.md").exists(), out)
        (gd / "challenge.md").write_text(first, encoding="utf-8")
        wrong = [dict(gt[0]), {**gt[1], "digest": {"status": "done", "summary": "別的信封的摘要"}}]
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": wrong})
        check("B9 代寫：舊的代寫檔對不回這次的信封 → 缺件（不沿用）", any("代寫" in m for m in stages(d)["design"].get("missing_records") or []), out)
        d = new_run()
        walk(d, "design")
        bad = trace + [{"label": "plan:a", "agent_type": "agent-workflow:dev-spec-author", "phase": "Plan", "record": str(Path(d) / "design/plan-a.md"), "ok": True},
                       {"label": "plan:b", "agent_type": "agent-workflow:dev-spec-author", "phase": "Plan", "record": str(Path(d) / "design/plan-b.md"), "ok": False}]
        (Path(d) / "design").mkdir(exist_ok=True)
        for f in ("outline.md", "challenge.md"):
            (Path(d) / "design" / f).write_text("# 紀錄\n", encoding="utf-8")
        bad = [{**t, "record": str(Path(d) / "design" / Path(t["record"]).name)} for t in bad]
        c, out = call("stage", d, "design", stdin={"design": des, "challenge": ver, "trace": bad})
        miss = stages(d)["design"].get("missing_records") or []
        _, st_out = call("status", d)
        idx = (Path(d) / "design" / INDEX_NAME).read_text(encoding="utf-8")
        check("執行紀錄索引：缺紀錄檔（ok=true 但檔不存在 → missing_records、索引標缺件、status 警告；ok=false 也算缺件）",
              len(miss) == 2 and "plan:a" in miss[0] and "plan:b" in miss[1] and "缺紀錄檔" in idx and "agent 沒有交件" in idx and "警告" in st_out, out + st_out + idx)
        c, out = call("approve", d, "design", "--by", "policy", "--at", "t", "--note", "p")
        c2, out2 = call("approve", d, "design", "--by", "human", "--at", "t")
        check("執行紀錄索引：policy 被擋（缺紀錄檔不能政策核准；人工核准可以但警告）",
              c != 0 and "執行紀錄缺件" in out and c2 == 0 and "警告" in out2, out + out2)
        d = new_run()
        md = _env({"decision": "merge", "repo": "acme-fork/app", "pr": 3, "head_sha": "a" * 40, "traceability": [], "out_of_scope": [],
                   "followups": [], "risk_tier": "low", "gate_reasons": [], "merge_state": "merged"})
        md["evidence"] = [{"claim": "gate 判 merge", "source": "merge_gate.py", "result": "exit 0"}]
        strace = [{"label": "analyze", "agent_type": "agent-workflow:dev-merge-steward", "phase": "Analyze", "record": f"{d}/merge/analyze.md", "ok": True},
                  {"label": "execute:merge", "agent_type": "agent-workflow:dev-merge-steward", "phase": "Execute", "record": f"{d}/merge/execute-merge.md", "ok": True}]
        c, out = call("stage", d, "merge", stdin={"stage": "merge", "analysis": md, "execute": md, "trace": strace})
        f1, f2 = Path(d) / "merge/analyze.md", Path(d) / "merge/execute-merge.md"
        body = f1.read_text(encoding="utf-8") if f1.is_file() else ""
        idx = (Path(d) / "merge" / INDEX_NAME).read_text(encoding="utf-8") if (Path(d) / "merge" / INDEX_NAME).is_file() else ""
        check("執行紀錄索引：steward 代寫（沒有 Write 的 steward 由工具依信封代寫紀錄檔，不算缺件）",
              f1.is_file() and f2.is_file() and GHOSTWRITTEN in body and "gate 判 merge" in body
              and "missing_records" not in stages(d)["merge"] and GHOSTWRITTEN in idx, out + body + idx)
        d = new_run()
        c, out = call("stage", d, "plan", stdin=plan_res())
        check("執行紀錄索引：回傳沒有 trace（舊 workflow）只警告，不判缺件",
              c == 0 and "沒有 trace" in out and not (Path(d) / "plan" / INDEX_NAME).exists() and "missing_records" not in stages(d)["plan"], out)

        # ── SP2.6 hotfix 後續
        d = new_run({"name": "hotfix"}, approve_plan=True, intent=with_root)
        c, out = call("close", d, "--by", "human", "--at", "t")
        c2, out2 = call("followup", d, "--run", str(proj / "_runs/沒有"), "--by", "human", "--at", "t")
        check("hotfix 後續：沒有 followup_run 不能 close（後續 run 不存在也不能記錄）",
              c != 0 and "followup_run" in out and c2 != 0 and "closed" not in load(Path(d)), out + out2)
        d_follow = new_run()
        c, out = call("followup", d, "--run", d_follow, "--by", "human", "--at", "t")
        c2, out2 = call("close", d, "--by", "human", "--at", "t")
        check("hotfix 後續：有 followup_run 但流程還沒走完（build 之後都 pending）不能 close",
              c == 0 and c2 != 0 and "流程還沒走完" in out2 and "closed" not in load(Path(d)), out + out2)

        def finish(dd: str) -> None:
            walk(dd, "merge", skip_plan=True)
            data = load(Path(dd))
            data["stages"]["merge"] = {"status": "done", "gate": False, "envelopes": 1}
            dump(Path(dd), data)
        finish(d)
        c2, out2 = call("close", d, "--by", "human", "--at", "t")
        data = load(Path(d))
        d_feat = new_run({"name": "feature"}, approve_plan=True, intent=with_root)
        c3, out3 = call("close", d_feat, "--by", "human", "--at", "t")
        finish(d_feat)
        c4, out4 = call("close", d_feat, "--by", "human", "--at", "t")
        check("hotfix 後續：有 followup_run 可以 close（整條流程也要走完；feature 不需要後續 run，但同樣要走完）",
              c2 == 0 and data["followup_run"]["run"] == d_follow and data["closed"] == {"by": "human", "at": "t"} and c3 != 0 and c4 == 0,
              out2 + out3 + out4)
        # status --json（給外部程式讀的快照）
        def snap(dd: str) -> tuple[dict, list]:
            c_, o_ = call("status", dd, "--json")
            j_ = json.loads(o_) if c_ == 0 else {}
            return j_, schemalib.check_doc(j_, "run-status")
        d_wait = new_run({"name": "bugfix"}, intent=with_root)
        j1, e1 = snap(d_wait)
        check("status --json：plan 存好還沒核准＝waiting_gate，下一步與等待的閘門都是 plan；recipe 沒有的段是 skipped",
              not e1 and j1["state"] == "waiting_gate" and j1["next"] == "plan" and j1["waiting_gate"] == "plan"
              and j1["recipe"]["name"] == "bugfix" and not j1["recipe"]["locked"]
              and {x["name"]: x["status"] for x in j1["stages"]}["design"] == "skipped", str(e1) + str(j1))
        call("approve", d_wait, "plan", "--by", "human", "--at", "t")
        call("stage", d_wait, "build", stdin=traced(d_wait, "build", {"x": build}))
        j2, e2 = snap(d_wait)
        b2 = {x["name"]: x for x in j2["stages"]}["build"]
        check("status --json：核准後 running，跳過 design 直接到下一段；段的派工數取自 trace",
              not e2 and j2["recipe"]["locked"] and j2["waiting_gate"] is None and j2["next"] not in ("plan", "design", "build")
              and b2["agents"] >= 1 and b2["agents_missing"] == 0 and {x["name"]: x for x in j2["stages"]}["plan"]["approved"] == {"by": "human", "at": "t"},
              str(e2) + str(j2))
        j3, e3 = snap(d_feat)
        check("status --json：結案的 run 是 closed、沒有下一步", not e3 and j3["state"] == "closed" and j3["next"] is None, str(e3) + str(j3))
        d_ab = new_run({"name": "docs"}, intent=with_root)
        call("abandon", d_ab, "--by", "human", "--at", "t", "--reason", "不做了")
        j4, e4 = snap(d_ab)
        check("status --json：取消的 run 是 abandoned、沒有下一步", not e4 and j4["state"] == "abandoned" and j4["next"] is None, str(e4) + str(j4))
        call("stage", d_wait, j2["next"], stdin={"halted": True, "halt_reason": "x"})
        j5, e5 = snap(d_wait)
        check("status --json：halted 的段是 blocked", not e5 and j5["state"] == "blocked", str(e5) + str(j5))
        # ── 0.4：執行核心的 trace（升級重派、模型分級）與調研重用
        base_profile = PROJECT_PROFILE.read_text(encoding="utf-8")
        with_models = {**json.loads(base_profile), "models": {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"},
                                                               "defaults": {"*": "standard"}, "floors": {}}}
        SPEC, RES = "agent-workflow:dev-spec-author", "agent-workflow:tpm-researcher"

        def k_run() -> str:
            n[0] += 1
            dd = str(proj / "_runs" / f"k{n[0]}")
            call("init", dd, "--event", "sdlc", "--target", str(proj / "repo"), "--requirement", "r")
            return dd

        def k_entry(dd: str, label: str, agent: str, tier: str, model: str, **extra) -> dict:
            f = Path(dd) / "plan" / (label.replace(":", "-") + ".md")
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text("# 紀錄\n", encoding="utf-8")
            return {"label": label, "agent_type": agent, "phase": "p", "record": str(f), "ok": True, "model": model, "tier": tier, "reason": "x", **extra}

        def k_stage(dd: str, trace: list, **extra) -> tuple[int, str]:
            return call("stage", dd, "plan", stdin={**plan_res({"name": "feature", "modifiers": [], "plan_mode": "feature"}), "trace": trace, **extra})

        d = k_run()
        good = [k_entry(d, "intent", SPEC, "strong", "inherit")]
        c, out = k_stage(d, [{**k_entry(d, "locate:a", RES, "light", "inherit"), "ok": False, "superseded": True},
                             k_entry(d, "locate:a", RES, "standard", "inherit", attempt=2, escalated_from="light"), *good])
        check("升級重派：第一筆沒交件、後面同 label 的重派有交件 → 不算缺件", c == 0 and "missing_records" not in stages(d)["plan"], out)
        c, out = k_stage(d, [{**k_entry(d, "locate:a", RES, "light", "inherit"), "ok": False, "superseded": True}, *good])
        check("升級重派：只有 superseded 旗標、後面沒有交件的重派 → 仍是缺件", bool(stages(d)["plan"].get("missing_records")), out)
        c, out = k_stage(d, [{**k_entry(d, "locate:a", RES, "light", "inherit"), "ok": False, "superseded": True},
                             k_entry(d, "locate:b", RES, "standard", "inherit"), *good])
        check("升級重派：後面交件的是別的 label → 仍是缺件", bool(stages(d)["plan"].get("missing_records")), out)
        check("模型分級：profile 沒有 models 時不檢查 trace 的等級", k_stage(d, [k_entry(d, "intent", SPEC, "light", "haiku")])[0] == 0)
        PROJECT_PROFILE.write_text(json.dumps(with_models, ensure_ascii=False), encoding="utf-8")
        d = k_run()
        c, out = k_stage(d, [k_entry(d, "intent", SPEC, "strong", "opus"), k_entry(d, "research:a", RES, "standard", "sonnet"), k_entry(d, "locate:a", RES, "light", "haiku")])
        check("模型分級：等級不低於下限、model 對得上 → 通過", c == 0 and stages(d)["plan"]["status"] == "done", out)
        c, out = k_stage(d, [k_entry(d, "intent", SPEC, "light", "haiku")])
        check("模型分級：plan 段的 dev-spec-author 用 light（低於下限 strong）→ invalid", c == 1 and stages(d)["plan"]["status"] == "invalid" and "低於下限 strong" in out, out)
        c, out = k_stage(d, [k_entry(d, "intent", SPEC, "strong", "haiku")])
        check("模型分級：說是 strong、model 卻是 haiku → invalid", c == 1 and "model 應該是 opus" in out, out)
        t0 = k_entry(d, "intent", SPEC, "strong", "opus")
        c, out = k_stage(d, [{k: v for k, v in t0.items() if k not in ("tier", "model")}])
        check("模型分級：profile 有 models，plan 的 trace 沒有 tier → invalid", c == 1 and "沒有 model／tier" in out, out)
        c, out = k_stage(d, [k_entry(d, "intent", SPEC, "ultra", "opus")])
        check("模型分級：不認得的等級 → invalid", c == 1 and "不認得" in out, out)
        # 0.5：SDLC 每一段都經過執行核心
        impl = lambda **kw: {"label": "Implement:t1", "agent_type": "agent-workflow:dev-implementer", "phase": "Implement", "record": "x.md", "ok": True, **kw}  # noqa: E731
        check("模型分級（0.5）：build、test、deliver 等段的 trace 沒有 tier → 判錯（都要經過執行核心）",
              all(any("沒有 model／tier" in e for e in model_errors(s, {"trace": [impl()]}, with_models, [])) for s in ("design", "build", "sync", "test", "deliver", "merge")))
        check("模型分級（0.5）：build 段的 implementer 用 standard、model 對得上 → 通過；擴充 agent 沒有 tier 不檢查",
              not model_errors("build", {"trace": [impl(tier="standard", model="sonnet"), {"label": "ext", "agent_type": "my:agent", "ok": True}]}, with_models, []))
        check("模型分級（0.5）：merge 段的 steward 用 standard（低於下限 strong）→ 判錯",
              any("低於下限 strong" in e for e in model_errors("merge", {"trace": [{"label": "execute", "agent_type": "agent-workflow:dev-merge-steward", "ok": True, "tier": "standard", "model": "sonnet"}]}, with_models, [])))
        cap_run = Path(k_run())
        _has, rows, miss = process_trace(cap_run, "build", {"trace": [{"label": "Implement:t9", "agent_type": "agent-workflow:dev-implementer", "phase": "Implement",
                                                                     "record": str(cap_run / "build/Implement-t9.md"), "ok": False, "capped": True, "max_agents": 40}]}, False)
        out_file = Path(d) / "outside.md"
        out_file.write_text("# 不是紀錄\n", encoding="utf-8")
        rc_run = Path(k_run())
        (rc_run / "build").mkdir(parents=True, exist_ok=True)
        (rc_run / "build" / "real.md").write_text("# 紀錄\n", encoding="utf-8")
        check("紀錄檔路徑：trace 指到 run 目錄以外的檔不算紀錄；run 目錄底下的照舊",
              not any(c.is_file() for c in record_candidates(rc_run, "build", str(out_file))) and any(c.is_file() for c in record_candidates(rc_run, "build", str(rc_run / "build" / "real.md"))),
              record_candidates(rc_run, "build", str(out_file)))
        check("派工數上限：到上限沒派出的列成缺件，並說明是上限（不是 agent 失敗）", len(miss) == 1 and "派工數到上限（40" in miss[0] and rows[0]["status"] == "沒有派出（派工數到上限）", miss)
        PROJECT_PROFILE.write_text(json.dumps({**with_models, "models": {**with_models["models"], "mode": "inherit"}}, ensure_ascii=False), encoding="utf-8")
        check("模型分級：mode=inherit 不檢查", k_stage(d, [k_entry(d, "intent", SPEC, "", "inherit")])[0] == 0)
        PROJECT_PROFILE.write_text(base_profile, encoding="utf-8")
        # 調研重用
        (proj / "repo" / "a.py").write_text("x = 1\n", encoding="utf-8")
        d = k_run()
        r_env = {**_env({"lens": "code-contracts", "findings": []}), "summary": "上次的結論"}
        res_t = [k_entry(d, "intent", SPEC, "strong", "inherit"), k_entry(d, "research:code-contracts", RES, "standard", "inherit")]
        c, out = k_stage(d, res_t, research=[r_env], research_by_angle={"code-contracts": r_env}, locations={"code-contracts": [{"path": "a.py", "start": 1, "end": 1, "why": "x"}]}, reused=[])
        store = Path(d) / "plan" / "reuse.json"
        check("調研重用：plan 存檔後記下來源檔的 sha1", c == 0 and store.is_file() and "code-contracts" in json.loads(store.read_text(encoding="utf-8"))["angles"], out)
        c, out = k_stage(d, good_t := [k_entry(d, "intent", SPEC, "strong", "inherit")], research=[r_env], research_by_angle={"code-contracts": r_env}, locations={}, reused=["code-contracts"])
        check("調研重用：沿用的角度來源沒變、結論一致 → 通過（不需要 trace）", c == 0 and stages(d)["plan"]["status"] == "done", out)
        c, out = k_stage(d, good_t, research=[{**r_env, "summary": "偷改"}], research_by_angle={"code-contracts": {**r_env, "summary": "偷改"}}, locations={}, reused=["code-contracts"])
        check("調研重用：沿用的結論和紀錄不一樣 → invalid", c == 1 and "調研重用" in out, out)
        (proj / "repo" / "a.py").write_text("x = 2\n", encoding="utf-8")
        c, out = k_stage(d, good_t, research=[r_env], research_by_angle={"code-contracts": r_env}, locations={}, reused=["code-contracts"])
        check("調研重用：來源變了還沿用 → invalid", c == 1 and "來源已經變了" in out, out)
        c, out = k_stage(d, good_t, research=[r_env], research_by_angle={"code-contracts": r_env}, locations={}, reused=["specs-adr"])
        check("調研重用：沿用沒有紀錄的角度 → invalid", c == 1 and "沒有紀錄" in out, out)
        # escalate 不能清掉後續義務（X3 I1）
        d = new_run({"name": "hotfix"}, approve_plan=True, intent=with_root)
        call("escalate", d, "--recipe", "bugfix", "--by", "human", "--at", "t", "--reason", "r")
        finish(d)
        c, out = call("close", d, "--by", "human", "--at", "t")
        tam = recipe_tamper_errors(load(Path(d)))
        d_dm = new_run({"name": "docs"}, approve_plan=True, intent=with_root)
        call("escalate", d_dm, "--recipe", "bugfix", "--modifiers", "migration", "--by", "human", "--at", "t", "--reason", "r")
        tam2 = recipe_tamper_errors(load(Path(d_dm)))
        check("hotfix 後續：escalate 成 bugfix 之後仍要 followup_run 才能 close；合法的 escalate 不會被當成手改（docs → bugfix＋migration 也是）",
              load(Path(d))["recipe"]["resolved"]["followup"] is True and c != 0 and "followup_run" in out and not tam and not tam2, out + str(tam) + str(tam2))
        # 後續 run 要是真的完整流程：已取消、或本身也是 hotfix 的不行
        d_ab = new_run()
        call("abandon", d_ab, "--reason", "x", "--by", "human", "--at", "t")
        d_hot = new_run({"name": "hotfix"}, approve_plan=True, intent=with_root)
        c, out = call("followup", d, "--run", d_ab, "--by", "human", "--at", "t")
        c2, out2 = call("followup", d, "--run", d_hot, "--by", "human", "--at", "t")
        check("hotfix 後續：已取消的 run、或本身也是 hotfix 的 run 不能當後續 run",
              c != 0 and "已取消" in out and c2 != 0 and "最短路徑" in out2, out + out2)
        # ── X3 B1：workflow 回傳的 recipe 要和 run.json 鎖定的一致；recipe 要求的角色要真的派
        d = new_run({"name": "feature", "modifiers": ["security"]}, approve_plan=True)
        walk(d, "test", skip_plan=True)
        rec = load(Path(d))["recipe"]
        f = Path(d) / "test" / "verify.md"
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text("# 紀錄\n", encoding="utf-8")
        vt = {"label": "verify#1", "agent_type": "agent-workflow:dev-verifier", "phase": "Verify", "record": str(f), "ok": True}
        st_ = {"label": "security", "agent_type": "agent-workflow:dev-security-reviewer", "phase": "Review", "record": str(f), "ok": True}
        good = {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rec["resolved"]["stages"]}
        c, out = call("stage", d, "test", stdin={"v": test, "trace": [vt], "recipe": {"name": "docs", "modifiers": [], "stages": ["plan", "build", "deliver", "merge"]}})
        c2, out2 = call("approve", d, "test", "--by", "policy", "--at", "t", "--note", "p")
        check("X3 B1：回傳的 recipe（docs）和 run.json 鎖定的（feature＋security）不一致 → invalid、不能核准",
              c != 0 and stages(d)["test"]["status"] == "invalid" and "recipe 不一致" in out and c2 != 0, out + out2)
        c, out = call("stage", d, "test", stdin={"v": test, "trace": [vt], "recipe": good})
        c2, out2 = call("approve", d, "test", "--by", "policy", "--at", "t", "--note", "p")
        c3, out3 = call("stage", d, "test", stdin={"v": test, "trace": [vt, st_], "recipe": good})
        c4, out4 = call("approve", d, "test", "--by", "policy", "--at", "t", "--note", "p")
        check("X3 B1：security_review=always 卻沒派 dev-security-reviewer → 政策核准被擋；派了才放行",
              c == 0 and c2 != 0 and "security_review" in out2 and c3 == 0 and c4 == 0, out + out2 + out3 + out4)
        # ── X3 B3：有 recipe 的 run，回傳沒有 trace 算缺件
        c, out = call("stage", d, "test", stdin={"v": test, "recipe": good})
        c2, out2 = call("approve", d, "test", "--by", "policy", "--at", "t", "--note", "p")
        check("X3 B3：有 recipe 的 run 回傳沒有 trace → missing_records、政策核准被擋",
              stages(d)["test"].get("missing_records") and c2 != 0 and "trace" in out2, out + out2)
        # ── X3 B2：runs_dir 底下放的 profile 不會被採用；不合規則的 profile 判錯
        planted = proj / "_runs/.claude/teams"
        planted.mkdir(parents=True, exist_ok=True)
        hijack = {"recipes": {**rcp.BUILTIN_RECIPES, "docs": {**rcp.BUILTIN_RECIPES["docs"], "build_kinds": ["code", "test", "doc"]}},
                  "modifiers": rcp.BUILTIN_MODIFIERS}
        (planted / "methodology.json").write_text(json.dumps(hijack, ensure_ascii=False), encoding="utf-8")
        prof, perr = load_profile_for(Path(d), [])
        badp = proj / "bad-profile.json"
        badp.write_text(json.dumps({"recipes": {"x": {**rcp.BUILTIN_RECIPES["docs"], "stages": ["build", "deliver", "merge"]}}}), encoding="utf-8")
        _, perr2 = load_profile_for(Path(d), ["--profile", str(badp)])
        check("X3 B2：不採用 run 目錄上層被放進去的 profile；--profile 不合規則（recipe 沒有 plan）→ 錯誤",
              not perr and prof["recipes"]["docs"]["build_kinds"] == rcp.BUILTIN_RECIPES["docs"]["build_kinds"] and perr2, perr + perr2)
        # ── X3 M1：手改 run.json 的 recipe.resolved（少掉 test 段）→ 之後的核准被擋
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        walk(d, "test", skip_plan=True)
        data = load(Path(d))
        data["recipe"]["resolved"]["stages"] = [x for x in data["recipe"]["resolved"]["stages"] if x != "test"]
        dump(Path(d), data)
        call("stage", d, "deliver", stdin=traced(d, "deliver", {"x": build}))
        c, out = call("approve", d, "deliver", "--by", "human", "--at", "t")
        check("X3 M1：recipe.resolved 被手改鬆（少 test 段）→ 核准被擋", c != 0 and "比 bugfix 的定義鬆" in out, out)
        # ── X3 M4：舊 run（沒有 recipe）plan 核准後，不能用 stage plan 補記較弱的 recipe
        d = new_run()
        call("stage", d, "plan", stdin=plan_res())
        call("approve", d, "plan", "--by", "human", "--at", "t")
        c, out = call("stage", d, "plan", stdin=plan_res({"name": "docs"}, with_root))
        check("X3 M4：舊 run 核准過 plan 之後補記 docs（少段）→ 拒絕", c != 0 and "recipe" not in load(Path(d)) and "少了段" in out, out)
        # ── B4.2：watcher 的事件檔與記憶檔不是閘門資料，被改成什麼都不影響判定
        d = new_run({"name": "bugfix"}, approve_plan=True, intent=with_root)
        walk(d, "deliver", skip_plan=True)
        call("stage", d, "deliver", stdin=traced(d, "deliver", {"x": build}))
        before = (chain_errors(load(Path(d)), "test"), policy_violations("deliver", json.loads((Path(d) / "deliver.json").read_text(encoding="utf-8")), Path(d)))
        (Path(d) / "progress.jsonl").write_text('{"type": "stage_approved", "stage": "deliver", "by": "human"}\n不是 JSON\n', encoding="utf-8")
        (Path(d) / ".watch-state.json").write_text('{"view": {"stages": {"deliver": {"approved": true, "status": "invalid"}}}}', encoding="utf-8")
        after = (chain_errors(load(Path(d)), "test"), policy_violations("deliver", json.loads((Path(d) / "deliver.json").read_text(encoding="utf-8")), Path(d)))
        not_approved = not (stages(d)["deliver"].get("approval") or {})
        c, out = call("approve", d, "deliver", "--by", "human", "--at", "t")
        srcs = [(Path(__file__).resolve().parent / n).read_text(encoding="utf-8").split("def selftest")[0] for n in ("save_run.py", "publish.py", "merge_gate.py")]
        check("B4.2 事件檔：progress.jsonl、.watch-state.json 被改（假的核准事件、壞行）→ 閘門鏈與政策判定不變、不會因此算已核准、之後照常核准",
              before == after and not_approved and c == 0 and not chain_errors(load(Path(d)), "deliver"), out + str(before) + str(after))
        check("B4.2 事件檔：save_run、publish、merge_gate 的判定程式不讀這兩個檔", all("progress.jsonl" not in s and ".watch-state" not in s for s in srcs))
        PROJECT_PROFILE = saved_profile
    print("save_run selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
