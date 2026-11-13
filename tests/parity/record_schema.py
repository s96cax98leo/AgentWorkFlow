#!/usr/bin/env python3
"""錄 schema 驗證的對跑基準：golden-set 每個範例與內建 profile，用 schemalib 驗出來的完整錯誤清單。

用法：python3 tests/parity/record_schema.py > tests/parity/schema.expected.json
另外附一組探測案例（probes）：直接測 validate 的各個關鍵字與 Python 特有的格式（repr、True == 1）。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
sys.path.insert(0, str(TEAMS / "tools"))
import schemalib  # noqa: E402

out = {"cases": [], "probes": [], "extract": []}
gs = TEAMS / "golden-set"
cases = [(kind, f, json.loads(f.read_text(encoding="utf-8"))) for kind in ("valid", "invalid") for f in sorted((gs / kind).glob("*.json"))]
cases += [("profile", p, {"schema": "methodology", "data": json.loads(p.read_text(encoding="utf-8"))}) for p in sorted((TEAMS / "methodologies").glob("*.json"))]
for kind, f, case in cases:
    schemalib.CONFIG_OVERRIDE = case.get("config")
    if "schema" in case:
        schema = schemalib.doc_schema(case["schema"])
        errs = schemalib.check_doc(case["data"], case["schema"])
    else:
        schema = schemalib.compose(case["payload"])
        errs = schemalib.check(case["data"], case["payload"])
    schemalib.CONFIG_OVERRIDE = None
    out["cases"].append({"kind": kind, "file": f.relative_to(TEAMS).as_posix(), "errors": errs, "unsupported": schemalib.unsupported_keywords(schema)})

PROBES = [
    ({"type": "string", "enum": ["a", "b"]}, "c"),
    ({"enum": [1, "x", None]}, True),
    ({"enum": [1, "x", None]}, 1.0),
    ({"enum": [1, "x", None]}, False),
    ({"enum": ["it's"]}, 'say "hi"'),
    ({"type": ["string", "null"]}, 3),
    ({"type": "integer"}, 1.0),
    ({"type": "integer"}, True),
    ({"type": "number"}, 2),
    ({"type": "number", "minimum": 0, "maximum": 1}, 1.5),
    ({"type": "number", "minimum": 0.5}, 0),
    ({"type": "string", "minLength": 1}, ""),
    ({"type": "string", "maxLength": 3}, "一二三四"),
    ({"type": "string", "pattern": "^[a-z]+$"}, "Abc\n"),
    ({"type": "string", "pattern": "^[a-z]+$"}, "abc"),
    ({"type": "array", "minItems": 2, "items": {"type": "integer"}}, ["x"]),
    ({"type": "array", "maxItems": 1}, [1, 2, 3]),
    ({"type": "object", "required": ["a", "b"], "properties": {"a": {"type": "string"}}, "additionalProperties": False}, {"a": 1, "c": 2}),
    ({"type": "object", "additionalProperties": {"type": "integer"}}, {"x": "y", "z": 1}),
    ({"type": "object", "properties": {"t": {"type": "array", "items": {"type": "object", "required": ["id"]}}}}, {"t": [{}, {"id": 1}, 3]}),
]
for schema, data in PROBES:
    out["probes"].append({"schema": schema, "data": data, "errors": schemalib.validate(data, schema)})

TEXTS = [
    'x\n```json\n{"a": 1}\n```\ny\n```json\n{"b": [1, 2]}\n```\n',
    '```json\n{"a": 1}\n```\n```json\n{壞的}\n```',
    '{"status": "done"}',
    "不是 JSON",
    "",
]
for t in TEXTS:
    out["extract"].append({"text": t, "value": schemalib.extract_json(t)})
json.dump(out, sys.stdout, ensure_ascii=False, indent=1)
print()
