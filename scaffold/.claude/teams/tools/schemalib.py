"""agent team 的 schema 工具：載入、組合（信封＋payload）、驗證。

只支援 schemas/ 用到的 JSON Schema 關鍵字（type（可為陣列，含 null）、properties、required、enum、items、
minItems、minLength、minimum、maximum），所以 hook 不必依賴第三方套件；
golden-set/run.py 會拿官方 jsonschema 套件交叉比對，確保判斷一致。

獨立文件（不是信封 payload，例如開發規範 profile methodology）用 check_doc 驗。
任務編號規則讀 ../guard.config.json 的 task_id_prefix、task_authority（init.py 產生）。
"""

from __future__ import annotations

import json
import re
from pathlib import Path

TEAMS_DIR = Path(__file__).resolve().parent.parent
SCHEMA_DIR = TEAMS_DIR / "schemas"
PAYLOAD_DIR = SCHEMA_DIR / "payloads"
CONFIG = TEAMS_DIR / "guard.config.json"
_PLACEHOLDER = re.compile(r"\{\{[A-Z_]+\}\}")


def config(path: Path | None = None) -> dict:
    """專案設定（沒有檔案或格式錯時回空 dict）；還沒替換的佔位符當成空值。
    path 預設是本檔旁的 ../guard.config.json；merge_gate 等工具可指定別的專案的設定檔。"""
    try:
        data = json.loads((path or CONFIG).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return {k: ("" if isinstance(v, str) and _PLACEHOLDER.search(v) else v) for k, v in data.items()} if isinstance(data, dict) else {}


def task_id_regex() -> re.Pattern | None:
    """上層系統發的任務編號長相：task_id_prefix 後接數字（例如 TASK-0123）；沒設定前綴就不檢查。"""
    prefix = (config().get("task_id_prefix") or "").strip()
    return re.compile(rf"(?<![\w-]){re.escape(prefix)}\d+\b") if prefix else None


def task_authority() -> str:
    return (config().get("task_authority") or "").strip() or "上層系統"


def payload_names() -> list[str]:
    return sorted(p.name.removesuffix(".schema.json") for p in PAYLOAD_DIR.glob("*.schema.json"))


def load(name: str) -> dict:
    path = SCHEMA_DIR / f"{name}.schema.json"
    if not path.exists():
        path = PAYLOAD_DIR / f"{name}.schema.json"
    return json.loads(path.read_text(encoding="utf-8"))


def _strip_meta(node, in_properties: bool = False):
    """移除 schema 節點上的 $id、title、description，讓內嵌進 workflow 的 schema 精簡。
    `properties` 底下的鍵是欄位名（例如子任務的 title 欄位），不能刪（2026-10-07 乾跑抓到）。"""
    if isinstance(node, dict):
        if in_properties:
            return {k: _strip_meta(v) for k, v in node.items()}
        return {k: _strip_meta(v, k == "properties") for k, v in node.items() if k not in ("$id", "title", "description")}
    if isinstance(node, list):
        return [_strip_meta(v) for v in node]
    return node


def compose(payload: str | None) -> dict:
    """信封＋指定 payload 的完整 schema；payload=None 時 payload 欄位不限形狀。"""
    env = load("envelope")
    if payload:
        env["properties"]["payload"] = load(payload)
    return _strip_meta(env)


_TYPES = {
    "object": dict,
    "array": list,
    "string": str,
    "boolean": bool,
    "number": (int, float),
    "integer": int,
    "null": type(None),
}


def validate(data, schema: dict, path: str = "$") -> list[str]:
    """回傳錯誤清單（空＝通過）。"""
    errs: list[str] = []
    t = schema.get("type")
    if t:
        kinds = t if isinstance(t, list) else [t]
        ok = any(isinstance(data, _TYPES[k]) and not (k in ("number", "integer") and isinstance(data, bool)) for k in kinds)
        if not ok:
            return [f"{path}：型別應為 {'／'.join(kinds)}"]
    if "enum" in schema and data not in schema["enum"]:
        errs.append(f"{path}：值 {data!r} 不在 {schema['enum']}")
    if isinstance(data, str) and "minLength" in schema and len(data) < schema["minLength"]:
        errs.append(f"{path}：不能是空字串")
    if isinstance(data, (int, float)) and not isinstance(data, bool):
        if "minimum" in schema and data < schema["minimum"]:
            errs.append(f"{path}：小於 {schema['minimum']}")
        if "maximum" in schema and data > schema["maximum"]:
            errs.append(f"{path}：大於 {schema['maximum']}")
    if isinstance(data, dict):
        for key in schema.get("required", []):
            if key not in data:
                errs.append(f"{path}：缺欄位 {key}")
        for key, sub in schema.get("properties", {}).items():
            if key in data:
                errs.extend(validate(data[key], sub, f"{path}.{key}"))
    if isinstance(data, list):
        if "minItems" in schema and len(data) < schema["minItems"]:
            errs.append(f"{path}：至少要 {schema['minItems']} 項")
        if "items" in schema:
            for i, item in enumerate(data):
                errs.extend(validate(item, schema["items"], f"{path}[{i}]"))
    return errs


def semantic_errors(env: dict) -> list[str]:
    """schema 表達不了的交接規則。"""
    errs: list[str] = []
    qs = env.get("open_questions") or []
    blocking = [q for q in qs if isinstance(q, dict) and q.get("blocking")]
    if env.get("status") == "done" and blocking:
        errs.append("有 blocking 的 open_questions 時 status 不能是 done")
    for i, q in enumerate(blocking):
        if len(q.get("options") or []) < 2:
            errs.append(f"open_questions[{i}]：blocking 問題要附 2–3 個選項與代價")
    tid = task_id_regex()
    for i, w in enumerate(env.get("derived_work") or []):
        if tid and isinstance(w, str) and tid.search(w):
            errs.append(f"derived_work[{i}]：不可自編任務編號（交 {task_authority()} 編）：{w}")
    payload = env.get("payload") if isinstance(env.get("payload"), dict) else {}
    subtasks = payload.get("subtasks")
    if isinstance(subtasks, list):
        owner: dict[str, str] = {}
        ids = {s.get("id") for s in subtasks if isinstance(s, dict)}
        for s in subtasks:
            if not isinstance(s, dict):
                continue
            for f in s.get("owns") or []:
                if f in owner and owner[f] != s.get("id"):
                    errs.append(f"子任務 {owner[f]} 與 {s.get('id')} 同時擁有 {f}（檔案所有權不得重疊）")
                owner[f] = s.get("id")
            for d in s.get("depends_on") or []:
                if d not in ids:
                    errs.append(f"子任務 {s.get('id')} 依賴不存在的 {d}")
    errs.extend(merge_decision_errors(payload))
    errs.extend(plan_outline_errors(payload))
    return errs


def merge_decision_errors(payload: dict) -> list[str]:
    """merge-decision payload（dev-merge-steward 的對照表）的語意規則；不是這種 payload 就回空清單。
    merge_gate.py 也呼叫這裡，hook、golden-set、gate 三處判斷一致。"""
    if not (isinstance(payload, dict) and "traceability" in payload and "decision" in payload):
        return []
    errs: list[str] = []
    rows = [r for r in payload.get("traceability") or [] if isinstance(r, dict)]
    followups = [f for f in payload.get("followups") or [] if isinstance(f, dict)]
    out_of_scope = payload.get("out_of_scope") or []
    bad = [r.get("requirement_id") for r in rows if r.get("result") != "match"]
    seen: set = set()
    for r in rows:
        rid = r.get("requirement_id")
        if rid in seen:
            errs.append(f"traceability：需求 {rid} 重複出現")
        seen.add(rid)
        if r.get("result") == "match" and not r.get("evidence"):
            errs.append(f"traceability：需求 {rid} 判 match 卻沒有證據")
    if payload.get("decision") == "merge":
        if bad:
            errs.append(f"decision=merge 時不得有 mismatch／missing：{bad}")
        if followups:
            errs.append("decision=merge 時不得有 followups")
        if out_of_scope:
            errs.append("decision=merge 時不得有 out_of_scope（對不到需求的改動）")
    covered = {f.get("requirement_id") for f in followups}
    for rid in bad:
        if rid not in covered:
            errs.append(f"需求 {rid} 不一致或缺證據，卻沒有對應的 followup（新需求）")
    if out_of_scope and not followups:
        errs.append("有 out_of_scope 改動時要列 followups")
    if payload.get("merge_state") in ("merged", "queued") and payload.get("decision") != "merge":
        errs.append(f"merge_state={payload.get('merge_state')} 時 decision 必須是 merge")
    return errs


def _covers(a: str, b: str) -> bool:
    """路徑 a 是否涵蓋 b（相同，或 a 是 b 的上層目錄）。"""
    a, b = a.rstrip("/"), b.rstrip("/")
    return a == b or b.startswith(a + "/")


def plan_outline_errors(payload: dict) -> list[str]:
    """plan-outline payload（要不要拆子計劃）的語意規則；不是這種 payload 就回空清單。"""
    if not (isinstance(payload, dict) and "split" in payload and "sub_plans" in payload):
        return []
    errs: list[str] = []
    subs = [s for s in payload.get("sub_plans") or [] if isinstance(s, dict)]
    if payload.get("split") and len(subs) < 2:
        errs.append("split=true 時至少要 2 個子計劃")
    if not payload.get("split") and subs:
        errs.append("split=false 時 sub_plans 要留空")
    ids = [s.get("id") for s in subs]
    for i in {x for x in ids if ids.count(x) > 1}:
        errs.append(f"子計劃 id {i} 重複")
    deps = {s.get("id"): [d for d in s.get("depends_on") or []] for s in subs}
    for sid, ds in deps.items():
        for d in ds:
            if d == sid or d not in deps:
                errs.append(f"子計劃 {sid} 依賴不存在或是自己：{d}")
    # 相依不得有循環（拓撲排序排不完就是有循環）
    left = {k: {d for d in v if d in deps and d != k} for k, v in deps.items()}
    while left:
        ready = [k for k, v in left.items() if not v]
        if not ready:
            errs.append(f"子計劃相依有循環：{'、'.join(sorted(map(str, left)))}")
            break
        for k in ready:
            left.pop(k)
        for v in left.values():
            v.difference_update(ready)
    for i, a in enumerate(subs):
        for b in subs[i + 1:]:
            for pa in a.get("owns") or []:
                for pb in b.get("owns") or []:
                    if _covers(pa, pb) or _covers(pb, pa):
                        errs.append(f"子計劃 {a.get('id')} 與 {b.get('id')} 的範圍重疊：{pa}／{pb}")
    return errs


def check(env, payload: str | None = None) -> list[str]:
    return validate(env, compose(payload)) + (semantic_errors(env) if isinstance(env, dict) else [])


def doc_schema(name: str) -> dict:
    """不是信封 payload 的獨立 schema（例如 methodology），去掉 $id、title、description。"""
    return _strip_meta(load(name))


def methodology_errors(doc: dict) -> list[str]:
    """開發規範 profile 的語意規則（schema 表達不了的部分）。"""
    errs: list[str] = []
    conv = doc.get("conventions") if isinstance(doc.get("conventions"), dict) else {}
    commit = conv.get("commit") if isinstance(conv.get("commit"), dict) else {}
    rx = commit.get("check_regex")
    if isinstance(rx, str) and rx:
        try:
            re.compile(rx)
        except re.error as e:
            errs.append(f"conventions.commit.check_regex 不是合法的正規式：{e}")
    names = [e.get("name") for e in doc.get("extensions") or [] if isinstance(e, dict)]
    for n in sorted({x for x in names if names.count(x) > 1}, key=str):
        errs.append(f"extensions：name {n} 重複")
    req = doc.get("requirements") if isinstance(doc.get("requirements"), dict) else {}
    if req.get("source") == "command" and not str(req.get("command") or "").strip():
        errs.append("requirements.source=command 時要設 command")
    pat = req.get("id_pattern")
    if isinstance(pat, str) and pat:
        try:
            if re.compile(pat).groups < 1:
                errs.append("requirements.id_pattern 要有一個群組（需求 ID）")
        except re.error as e:
            errs.append(f"requirements.id_pattern 不是合法的正規式：{e}")
    return errs


def check_doc(data, name: str) -> list[str]:
    """驗證獨立文件（目前只有 methodology）：schema＋語意規則。"""
    errs = validate(data, doc_schema(name))
    if name == "methodology" and isinstance(data, dict):
        errs += methodology_errors(data)
    return errs


def extract_json(text: str):
    """從 agent 回報文字取出信封：優先最後一個 ```json 區塊，否則嘗試整段。"""
    blocks = re.findall(r"```json\s*(\{.*?\})\s*```", text, re.S)
    for raw in reversed(blocks):
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            continue
    try:
        return json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return None
