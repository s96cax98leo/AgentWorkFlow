"""golden set：每個範例同時用 schemalib（hook 用）與官方 jsonschema 驗，兩者對 schema 部分的判斷必須一致；
語意規則（semantic=true 的反例）只由 schemalib 擋。
選填鍵：expect＝schemalib 的錯誤訊息要含這段文字（確認是被對的規則擋下）；config＝這一例用的測試專用設定
（覆蓋 guard.config.json，例如 task_id_prefix），讓依設定而定的規則在模版與任何專案裡都實際跑到，不略過。
每個用到的 schema 都要只用 schemalib 認得的關鍵字（unsupported_keywords），否則判失敗。
範例檔有 payload 鍵＝信封＋該 payload；有 schema 鍵＝獨立文件（例如開發規範 profile methodology），直接用該 schema 驗。
另外把 methodologies/*.json（模版內建的 profile）與專案的 methodology.json 當成正例一起驗。用法：uv run --with jsonschema python3 .claude/teams/golden-set/run.py"""
import json, sys
from pathlib import Path
here = Path(__file__).resolve().parent
sys.path.insert(0, str(here.parent / "tools"))
import schemalib
try:
    import jsonschema
except ImportError:
    jsonschema = None
ok = bad = 0
cases = [(kind, f, json.loads(f.read_text(encoding="utf-8"))) for kind in ("valid", "invalid") for f in sorted((here / kind).glob("*.json"))]
# 內建 profile 與專案實際使用的 profile 都必須是合法的 methodology
profiles = sorted((here.parent / "methodologies").glob("*.json")) + [p for p in [here.parent / "methodology.json"] if p.is_file()]
cases += [("valid", p, {"schema": "methodology", "data": json.loads(p.read_text(encoding="utf-8"))}) for p in profiles]
for kind, f, case in cases:
    schemalib.CONFIG_OVERRIDE = case.get("config")
    if "schema" in case:
        schema = schemalib.doc_schema(case["schema"])
        mine = schemalib.check_doc(case["data"], case["schema"])
    else:
        schema = schemalib.compose(case["payload"])
        mine = schemalib.check(case["data"], case["payload"])
    want_ok = kind == "valid"
    problems = [f"schema 用了 schemalib 不認得的關鍵字：{u}" for u in schemalib.unsupported_keywords(schema)]
    if case.get("expect") and not any(case["expect"] in e for e in mine):
        problems.append(f"錯誤訊息沒有「{case['expect']}」：{mine}")
    if (not mine) != want_ok:
        problems.append(f"schemalib 判 {'通過' if not mine else '不通過'}：{mine}")
    if jsonschema:
        official = list(jsonschema.Draft202012Validator(schema).iter_errors(case["data"]))
        schema_only = not schemalib.validate(case["data"], schema)
        if (not official) != schema_only:
            problems.append(f"官方 jsonschema 與 schemalib 的 schema 判斷不一致：official={len(official)} 錯")
        if kind == "invalid" and not case.get("semantic") and not official:
            problems.append("官方 jsonschema 應該擋但通過")
    schemalib.CONFIG_OVERRIDE = None
    if problems:
        bad += 1; print(f"FAIL {kind}/{f.name}：" + "；".join(problems))
    else:
        ok += 1
print(f"golden set：通過 {ok}，失敗 {bad}" + ("" if jsonschema else "（未裝 jsonschema，只跑 schemalib；請用 uv run --with jsonschema）"))
sys.exit(1 if bad else 0)
