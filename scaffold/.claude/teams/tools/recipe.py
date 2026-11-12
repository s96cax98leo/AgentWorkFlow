"""recipe：依需求類型決定這次 SDLC 要走哪些段、派哪些角色（確定性解析，和 snippets/recipe.js 同一套規則）。

recipe 定義在開發規範 profile（methodology.json）的 recipes／modifiers；模版內建 11 個 recipe 與 5 個 modifier
（BUILTIN_RECIPES、BUILTIN_MODIFIERS，兩份內建 profile 照抄）。profile 沒有 recipes（舊版）或指定了不存在的 recipe 時，
一律當成 feature（FALLBACK_FEATURE＝改版前的完整流程），只會更嚴，不會變鬆。

規則（JS 版 snippets/recipe.js 必須一致；golden-set/recipes/*.json 兩邊都跑）：
  - 每個 recipe 都有 plan 段（intent 不能省）；stages 依 SDLC 順序排好。
  - size=small 時套 recipe 的 small 覆寫（只能改 research_angles 與 split），之後才疊 modifiers。
  - modifiers 只會加嚴：段、lens、審查軸、測試模式取聯集；design_review／security_review 取較嚴者（never < auto < always）。
  - 不認得的 modifier 記在 errors（呼叫端要停下交人），不默默略過。
  - stricter_or_equal(new, old)：escalate 只准加嚴（段、lens、審查軸、測試模式是超集，審查等級不降）。
  - diff_shortfalls(resolved, paths)：實際的 diff 形狀需要的流程 recipe 沒有時，列出理由（publish 拒推、merge_gate hold）。

用法：
    python3 recipe.py resolve [--profile <methodology.json>] --recipe <名> [--modifiers a,b] [--size small]
    python3 recipe.py check-diff --run <run_dir> [--base <ref>] [--repo <dir>]   # 依 run.json 的 recipe 檢查 diff 形狀
    python3 recipe.py --selftest
只用標準庫。
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
TEAMS = HERE.parent
SDLC = ["plan", "design", "build", "sync", "test", "deliver", "merge"]
LEVEL = {"never": 0, "auto": 1, "always": 2}
PLAN_MODES = ["feature", "diagnose", "impact", "invariants", "changelog", "baseline", "research", "revert"]
ALL_ANGLES = ["requirement", "specs-adr", "code-contracts", "upstream"]
ALL_AXES = ["standards", "spec", "bugs", "history"]
CODE_KINDS = ["code", "test", "doc"]
# intent 一定要有的節（I1）；profile 的 stages.plan.required_sections 可以覆寫（不能是空的）
BASE_SECTIONS = ["目標", "範圍", "不做的事", "驗收條件"]
# 各 plan_mode 另外要有的節；profile 的 plan_modes.<mode>.sections 可以覆寫
MODE_SECTIONS = {
    "feature": [],
    "diagnose": ["現象", "重現步驟", "預期行為", "根因"],
    "impact": ["既有驗收條件異動"],
    "invariants": ["不可改變的行為"],
    "changelog": ["上游變更與破壞性變更"],
    "baseline": ["量測基線"],
    "research": ["調研問題", "判準"],
    "revert": ["回退原因", "回退範圍", "恢復條件"],
}

FALLBACK_FEATURE = {
    "description": "新功能：完整 7 段（改版前的預設流程）",
    "stages": list(SDLC),
    "plan_mode": "feature",
    "research_angles": list(ALL_ANGLES),
    "design_lenses": ["coverage", "plan", "system-design"],
    "test": {"design_review": "auto", "security_review": "auto", "modes": ["verify"]},
    "deliver_axes": list(ALL_AXES),
    "build_kinds": list(CODE_KINDS),
    "publishable": True,
    "followup": False,
    "split": True,
}

_NO_DESIGN = ["plan", "build", "sync", "test", "deliver", "merge"]
_SHORT = ["plan", "build", "test", "deliver", "merge"]


def _r(desc, stages, mode, angles, lenses, test, axes, kinds, publishable=True, followup=False, split=False, small=None, gate=None):
    d = {"description": desc, "stages": stages, "plan_mode": mode, "research_angles": angles, "design_lenses": lenses,
         "test": test, "deliver_axes": axes, "build_kinds": kinds, "publishable": publishable, "followup": followup, "split": split}
    if small:
        d["small"] = small
    if gate:
        d["gate_mode"] = gate  # 閘門模式（見 gate_mode()）；沒寫＝strict
    return d


_T = lambda dr="auto", sr="auto", modes=("verify",): {"design_review": dr, "security_review": sr, "modes": list(modes)}  # noqa: E731

BUILTIN_RECIPES = {
    "feature": {**FALLBACK_FEATURE, "small": {"research_angles": ["requirement", "code-contracts"], "split": False}},
    "bugfix": _r("回報的 bug、錯誤行為：先診斷根因（重現→根因），修正並補回歸測試；沒有 design 段",
                 _NO_DESIGN, "diagnose", ["requirement", "code-contracts"], [], _T(modes=("verify", "regression")),
                 ["spec", "bugs"], list(CODE_KINDS)),
    "change": _r("需求變更（CR）：先盤點既有功能與驗收條件要改、留、刪哪些，design 只寫差異，test 加跑受影響舊驗收條件的回歸",
                 list(SDLC), "impact", ["requirement", "specs-adr", "code-contracts"], ["coverage", "plan", "delta"],
                 _T(modes=("verify", "regression")), list(ALL_AXES), list(CODE_KINDS), split=True),
    "hotfix": _r("線上事故：最短路徑修正，事後一定要開後續 run 補完整流程",
                 _SHORT, "diagnose", ["code-contracts"], [], _T(modes=("verify", "regression")), ["bugs"], list(CODE_KINDS),
                 followup=True),
    "refactor": _r("不改行為的重構：先列出不能改變的行為、先寫 characterization test，驗行為等價",
                   _NO_DESIGN, "invariants", ["code-contracts", "specs-adr"], [], _T(modes=("verify", "equivalence")),
                   ["standards", "bugs", "history"], list(CODE_KINDS)),
    "deps": _r("依賴升級：讀 changelog 找破壞性變更，強制安全審查",
               _NO_DESIGN, "changelog", ["requirement", "code-contracts"], [], _T(sr="always", modes=("verify", "regression")),
               ["standards", "bugs"], list(CODE_KINDS)),
    "docs": _r("只改文件、設定說明、文案：build 只能有 doc 子任務",
               ["plan", "build", "deliver", "merge"], "research", ["requirement"], [], _T("never", "never", ()),
               ["standards", "spec"], ["doc"], gate="plan-only"),
    "test-only": _r("補測試、修 flaky：不能改產品程式",
                    _SHORT, "research", ["code-contracts"], [], _T("never", "auto"), ["standards", "bugs"], ["test"], gate="plan-only"),
    "perf": _r("效能改善：先量基線，改完前後對照",
               list(SDLC), "baseline", ["requirement", "code-contracts"], ["coverage", "plan", "system-design"],
               _T(modes=("verify", "benchmark")), list(ALL_AXES), list(CODE_KINDS), split=True),
    "spike": _r("調研、可行性評估：只有 plan 段，產出 intent 與調研結論，不能推送",
                ["plan"], "research", ["requirement", "specs-adr", "code-contracts"], [], _T("never", "never", ()), [], [],
                publishable=False),
    "revert": _r("合併後回退：build 只跑 git revert，事後補原因與恢復條件",
                 _SHORT, "revert", ["code-contracts"], [], _T(modes=("verify", "regression")), ["bugs"], list(CODE_KINDS)),
}

BUILTIN_MODIFIERS = {
    "ui": {"description": "有畫面改動：一定做設計審查", "test": {"design_review": "always"}},
    "security": {"description": "有安全面（auth、權限、機敏資料、外部輸入）：一定做安全審查",
                 "test": {"security_review": "always"}, "deliver_axes": ["bugs"], "model_floors": {"tpm-skeptic": "strong"}},
    "migration": {"description": "資料庫 migration：強制 design 段與回滾計畫、一定做安全審查",
                  "add_stages": ["design"], "design_lenses": ["coverage", "plan", "rollback"],
                  "test": {"security_review": "always", "modes": ["regression"]}},
    "public_api": {"description": "公開 API 或對外契約：強制 design 段檢查相容性，deliver 加歷史軸",
                   "add_stages": ["design"], "design_lenses": ["coverage", "plan", "compat"], "deliver_axes": ["spec", "history"]},
    "large": {"description": "範圍大：允許拆子計劃、調研四個角度全開",
              "research_angles": list(ALL_ANGLES), "split": True, "model_floors": {"dev-spec-author": "strong"}},
}


# ------------------------------------------------------------------ 模型分級（I7：AI 建議，工具守下限，只會往上）
# 和 snippets/recipe.js 的 resolveModel 完全一致（golden-set/recipes/*.json 的 model_cases 兩邊對跑）。
TIERS = ["light", "standard", "strong"]
# strong 預設 inherit：沿用使用者主對話選的模型，模版不替使用者指定最貴的模型
BUILTIN_TIER_MODELS = {"light": "haiku", "standard": "sonnet", "strong": "inherit"}
# 閘門角色的最低等級：profile 的 floors 只能再往上加，不能蓋掉這份（鍵是「角色」或「角色@段」）
BUILTIN_MODEL_FLOORS = {
    "dev-merge-steward": "strong",
    "dev-security-reviewer": "strong",
    "tpm-skeptic@merge": "strong",
    "dev-spec-author@plan": "strong",
    "tpm-skeptic@plan": "standard",   # plan 閘門前唯一的反駁，不能排到 light
}


def _tier_rank(t) -> int:
    return TIERS.index(t) if isinstance(t, str) and t in TIERS else -1


def _tier_max(*tiers) -> str:
    best = ""
    for t in tiers:
        if _tier_rank(t) > _tier_rank(best):
            best = t
    return best


def _tier_lookup(table, role: str, stage: str) -> str:
    table = table if isinstance(table, dict) else {}
    return _tier_max(table.get(role), table.get(f"{role}@{stage}"))


def model_up(tier: str) -> str:
    """高一級的等級；已經最高（或不認得）回空字串。"""
    i = _tier_rank(tier)
    return TIERS[i + 1] if 0 <= i < len(TIERS) - 1 else ""


def tier_model(profile: dict | None, tier: str) -> str:
    """等級對應的模型別名；profile 寫 inherit 表示沿用主對話模型（回空字串）。"""
    models = (profile or {}).get("models")
    tiers = models.get("tiers") if isinstance(models, dict) and isinstance(models.get("tiers"), dict) else {}
    m = tiers.get(tier) if isinstance(tiers.get(tier), str) and tiers.get(tier) else BUILTIN_TIER_MODELS.get(tier, "")
    return "" if m == "inherit" else m


def resolve_model(profile: dict | None, role: str, stage: str, suggested: str | None = None, modifiers=None) -> dict:
    """回 {mode, tier, model, floor, reason}。
    profile 沒有 models 或 mode=inherit：不指定模型（tier、model 都是空字串），全部沿用主對話模型。
    mode=fixed：用 defaults，不採 AI 建議。mode=auto：AI 建議（suggested）優先，沒有或不認得就用 defaults。
    最後一律取 max(上面的結果, 內建 floor, profile floor, modifiers 的 model_floors)：只會往上。"""
    models = (profile or {}).get("models")
    if not isinstance(models, dict) or (models.get("mode") or "auto") == "inherit":
        return {"mode": "inherit", "tier": "", "model": "", "floor": "", "reason": "沒有設定 models 或 mode=inherit：沿用主對話模型"}
    mode = models.get("mode") or "auto"
    defaults = models.get("defaults") if isinstance(models.get("defaults"), dict) else {}
    default = _tier_max(defaults.get(f"{role}@{stage}")) or _tier_max(defaults.get(role)) or _tier_max(defaults.get("*")) or "standard"
    floor = _tier_max(_tier_lookup(BUILTIN_MODEL_FLOORS, role, stage), _tier_lookup(models.get("floors"), role, stage))
    mods_def = (profile or {}).get("modifiers") or {}
    for m in list(modifiers or []):
        # 內建 modifier 的下限一律生效（profile 把 model_floors 拿掉也一樣）；profile 自己的只能再往上加
        bd = BUILTIN_MODIFIERS.get(m) if isinstance(m, str) else None
        if isinstance(bd, dict):
            floor = _tier_max(floor, _tier_lookup(bd.get("model_floors"), role, stage))
        md = mods_def.get(m) if isinstance(mods_def, dict) and isinstance(m, str) and m in mods_def else None
        if isinstance(md, dict):
            floor = _tier_max(floor, _tier_lookup(md.get("model_floors"), role, stage))
    if mode == "auto" and _tier_rank(suggested) >= 0:
        base, why = suggested, f"AI 建議 {suggested}"
    elif mode == "auto" and suggested:
        base, why = default, f"AI 建議的等級「{suggested}」不認得，用預設 {default}"
    else:
        base, why = default, f"預設 {default}" + ("（mode=fixed，不採 AI 建議）" if mode == "fixed" else "")
    tier = _tier_max(base, floor)
    if tier != base:
        why += f"，低於下限 {floor}，往上調成 {tier}"
    return {"mode": mode, "tier": tier, "model": tier_model(profile, tier), "floor": floor, "reason": why}


def _order(stages) -> list[str]:
    s = set(stages)
    return [x for x in SDLC if x in s]


def _union(a, b) -> list:
    out = list(a or [])
    for x in b or []:
        if x not in out:
            out.append(x)
    return out


def _stricter(a: str, b: str) -> str:
    return a if LEVEL.get(a, 1) >= LEVEL.get(b, 1) else b


def load_profile(path: Path | None = None) -> dict | None:
    p = path or (TEAMS / "methodology.json")
    if not p.is_file():
        return None
    return json.loads(p.read_text(encoding="utf-8"))


def resolve(profile: dict | None, name: str | None, modifiers=None, size: str | None = None) -> dict:
    """回傳解析後的 recipe：{name, requested, resolved_from, modifiers, size, errors, ...recipe 欄位}。"""
    recipes = (profile or {}).get("recipes") or {}
    mods_def = (profile or {}).get("modifiers") or {}
    errors: list[str] = []
    requested = name or "feature"
    if requested in recipes:
        base, used, src = recipes[requested], requested, "profile"
    elif "feature" in recipes:
        base, used, src = recipes["feature"], "feature", f"profile（{requested} 不存在，當成 feature）" if name and name != "feature" else "profile"
    else:
        base, used = FALLBACK_FEATURE, "feature"
        src = "fallback（profile 沒有 recipes）" if not recipes else f"fallback（{requested} 不存在）"
    r = json.loads(json.dumps({k: v for k, v in base.items() if k != "small"}))
    r.setdefault("test", {})
    r["test"] = {"design_review": r["test"].get("design_review", "auto"), "security_review": r["test"].get("security_review", "auto"),
                 "modes": list(r["test"].get("modes") or [])}
    for k, v in (("research_angles", []), ("design_lenses", []), ("deliver_axes", []), ("build_kinds", list(CODE_KINDS))):
        r.setdefault(k, v)
    r.setdefault("publishable", True)
    r.setdefault("followup", False)
    r.setdefault("split", False)
    if "plan" not in r.get("stages", []):
        errors.append(f"recipe {used} 沒有 plan 段（intent 不能省），改加上 plan")
    r["stages"] = _order(["plan", *r.get("stages", [])])
    if size == "small" and isinstance(base.get("small"), dict):
        sm = base["small"]
        if "research_angles" in sm:
            r["research_angles"] = list(sm["research_angles"])
        if "split" in sm:
            r["split"] = bool(sm["split"])
    applied = []
    for m in list(modifiers or []):
        md = mods_def.get(m) or (BUILTIN_MODIFIERS.get(m) if not mods_def else None)
        if md is None:
            errors.append(f"不認得的 modifier：{m}")
            continue
        applied.append(m)
        r["stages"] = _order(_union(r["stages"], md.get("add_stages")))
        r["research_angles"] = _union(r["research_angles"], md.get("research_angles"))
        r["design_lenses"] = _union(r["design_lenses"], md.get("design_lenses"))
        r["deliver_axes"] = _union(r["deliver_axes"], md.get("deliver_axes"))
        t = md.get("test") or {}
        for k in ("design_review", "security_review"):
            if k in t:
                r["test"][k] = _stricter(t[k], r["test"][k])
        r["test"]["modes"] = _union(r["test"]["modes"], t.get("modes"))
        if md.get("split"):
            r["split"] = True
    # 有 design 段就一定要有 lens（不然 design 段沒有人挑錯）
    if "design" in r["stages"] and not r["design_lenses"]:
        r["design_lenses"] = ["coverage", "plan"]
    if "test" in r["stages"] and "verify" not in r["test"]["modes"]:
        r["test"]["modes"] = ["verify", *r["test"]["modes"]]
    r.update({"name": used, "requested": requested, "resolved_from": src, "modifiers": applied, "size": size or "", "errors": errors})
    return r


def stricter_or_equal(new: dict, old: dict) -> list[str]:
    """new 不比 old 鬆就回空清單；否則列出變鬆的地方（escalate 據此拒絕）。"""
    errs = []
    for key, what in (("stages", "段"), ("design_lenses", "design lens"), ("deliver_axes", "deliver 審查軸")):
        lost = [x for x in old.get(key) or [] if x not in (new.get(key) or [])]
        if lost:
            errs.append(f"少了{what}：{'、'.join(lost)}")
    lost = [x for x in (old.get("test") or {}).get("modes") or [] if x not in (new.get("test") or {}).get("modes") or []]
    if lost:
        errs.append(f"少了測試模式：{'、'.join(lost)}")
    for k in ("design_review", "security_review"):
        a, b = (new.get("test") or {}).get(k, "auto"), (old.get("test") or {}).get(k, "auto")
        if LEVEL.get(a, 1) < LEVEL.get(b, 1):
            errs.append(f"{k} 從 {b} 降成 {a}")
    return errs


def merge_stricter(new: dict, old: dict) -> dict:
    """escalate 用：以 new 為準，再把 old 有、new 沒有的段、lens、審查軸、測試模式補回去，審查等級取較嚴者。
    結果一定 stricter_or_equal 於 old 與 new。build_kinds、plan_mode 照 new（升級本來就是換一種做法）。"""
    m = json.loads(json.dumps(new))
    m["stages"] = _order(_union(new.get("stages"), old.get("stages")))
    for k in ("design_lenses", "deliver_axes", "research_angles"):
        m[k] = _union(new.get(k), old.get(k))
    t, ot = m.setdefault("test", {}), old.get("test") or {}
    for k in ("design_review", "security_review"):
        t[k] = _stricter(t.get(k, "auto"), ot.get(k, "auto"))
    t["modes"] = _union(t.get("modes"), ot.get("modes"))
    # 後續 run 的義務只能保留、不能因 escalate 消失（X3 I1）：升級後是否補完整流程，由 close 檢查整條流程是否完成
    m["followup"] = bool(new.get("followup")) or bool(old.get("followup"))
    m["modifiers"] = _union(new.get("modifiers"), old.get("modifiers"))
    return m


def escalate_errors(new: dict, old: dict) -> list[str]:
    """escalate 拒絕的情況：新 recipe 本身少了舊 recipe 的段（明顯是降級）。其他差異由 merge_stricter 補回。"""
    lost = [x for x in old.get("stages") or [] if x not in (new.get("stages") or [])]
    return [f"新 recipe {new.get('name')} 少了段：{'、'.join(lost)}（降級一律拒絕；要降級只能 abandon 後重開 run）"] if lost else []


# ------------------------------------------------------------------ diff 形狀
# 分類一律往嚴的方向判（X3 I6）：認不出來的都算 code。
# - doc：只認文件副檔名；.txt 只有在 docs/ 底下才算（CMakeLists.txt、requirements*.txt 不是文件）。
# - test：根目錄的測試目錄（tests/、test/、__tests__/、e2e/、spec/）底下的檔，或任何位置的測試檔名；
#   巢狀的 src/spec/、app/test/ 底下不是測試檔名的檔算 code。
# - migration、manifest（依賴宣告與鎖檔）、agent_config（agent 與 CI 設定）可和其他類並存，各自要求更嚴的流程。
DOC_EXT = re.compile(r"\.(md|mdx|rst|adoc)$", re.I)
DOC_TXT = re.compile(r"^docs?/.*\.txt$", re.I)
TEST_ROOT = re.compile(r"^(tests?|__tests__|e2e|spec)/", re.I)
TEST_NAME = re.compile(r"(^|/)test_[^/]+\.[a-z]+$|_test\.[a-z]+$|_spec\.[a-z]+$|\.(test|spec)\.[a-z]+$|(^|/)conftest\.py$", re.I)
MIGRATION = re.compile(r"(^|/)migrations?/|(^|/)db/migrate/|(^|/)alembic/|(^|/)(flyway|liquibase)/|\.sql$", re.I)
MANIFEST = re.compile(
    r"(^|/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|npm-shrinkwrap\.json|bun\.lockb?|poetry\.lock|uv\.lock|Pipfile\.lock|Cargo\.lock|go\.sum"
    r"|Gemfile\.lock|composer\.lock|packages\.lock\.json|package\.json|pyproject\.toml|setup\.py|setup\.cfg|requirements[^/]*\.txt|constraints[^/]*\.txt"
    r"|Pipfile|go\.mod|Cargo\.toml|Gemfile|composer\.json|pom\.xml|build\.gradle(\.kts)?|settings\.gradle(\.kts)?|gradle\.lockfile|libs\.versions\.toml"
    r"|[^/]+\.csproj|[^/]+\.fsproj|Directory\.Packages\.props|packages\.config|mix\.exs|mix\.lock|pubspec\.(yaml|lock)|Podfile(\.lock)?"
    r"|Package\.(swift|resolved)|deno\.(json|jsonc|lock)|vcpkg\.json|conanfile\.(txt|py)|flake\.lock)$"
    r"|(^|/)requirements/[^/]+\.(txt|in)$", re.I)
AGENT_CONFIG = re.compile(r"^\.claude/|^\.github/|(^|/)(CLAUDE|AGENTS)\.md$|(^|/)\.mcp\.json$", re.I)


def classify(path: str) -> set[str]:
    p = path.replace("\\", "/")
    while p.startswith("./"):
        p = p[2:]
    cats = set()
    if MIGRATION.search(p):
        cats.add("migration")
    if MANIFEST.search(p):
        cats.add("manifest")
    if AGENT_CONFIG.search(p):
        cats.add("agent_config")
    if TEST_NAME.search(p) or (TEST_ROOT.search(p) and not cats):
        cats.add("test")
    elif (DOC_EXT.search(p) or DOC_TXT.search(p)) and not cats:
        cats.add("doc")
    elif not cats & {"test", "doc"}:
        cats.add("code")
    return cats


def diff_shortfalls(r: dict, paths) -> list[str]:
    """實際 diff 需要、但 recipe 沒有的流程；空清單＝相符。"""
    out = []
    if not r.get("publishable", True):
        out.append(f"recipe {r.get('name')} 不能推送（publishable=false）")
    kinds = set(r.get("build_kinds") or [])
    by: dict[str, list[str]] = {}
    for p in paths:
        for c in classify(p):
            by.setdefault(c, []).append(p)
    for cat, what in (("code", "產品程式"), ("test", "測試"), ("doc", "文件")):
        if by.get(cat) and cat not in kinds:
            out.append(f"recipe {r.get('name')} 不允許改{what}（build_kinds={sorted(kinds)}），卻改了：{'、'.join(by[cat][:5])}")
    if by.get("migration") and ("design" not in r.get("stages", []) or "rollback" not in (r.get("design_lenses") or [])):
        out.append(f"改到 migration（{'、'.join(by['migration'][:3])}），需要 design 段與回滾計畫（加 modifier migration）")
    if by.get("manifest") and (r.get("test") or {}).get("security_review") != "always":
        out.append(f"改到依賴宣告或鎖檔（{'、'.join(by['manifest'][:3])}），需要安全審查（用 deps recipe 或加 modifier security）")
    if by.get("agent_config") and (r.get("test") or {}).get("security_review") != "always":
        out.append(f"改到 agent 或 CI 設定（{'、'.join(by['agent_config'][:3])}），需要安全審查（加 modifier security）")
    return out


GATE_MODES = ["plan-only", "strict"]


def gate_mode(profile: dict | None, run_recipe: dict | None) -> dict:
    """這個 run 的閘門模式：{mode, reason}。
    strict＝每個閘門都等人核准（預設）；plan-only＝plan 由人核准，其餘閘門先試政策核准（結果乾淨才過），不過就交人。
    只會往嚴的方向判：profile 沒寫、寫錯、認不得的 recipe、帶了任何 modifier（ui、security、migration…）一律 strict。
    plan 段不受它影響，一律由人核准。"""
    rr = run_recipe if isinstance(run_recipe, dict) else {}
    name = str(rr.get("name") or "")
    spec = ((profile or {}).get("recipes") or {}).get(name) if isinstance((profile or {}).get("recipes"), dict) else None
    if not name or not isinstance(spec, dict):
        return {"mode": "strict", "reason": "profile 沒有這個 recipe 的定義"}
    mods = [m for m in (rr.get("modifiers") or []) if m]
    if mods:
        return {"mode": "strict", "reason": f"帶了 modifier（{'、'.join(map(str, mods))}），每段都由人核准"}
    mode = spec.get("gate_mode")
    if mode == "plan-only":
        return {"mode": "plan-only", "reason": f"profile 的 recipes.{name}.gate_mode"}
    return {"mode": "strict", "reason": "profile 沒設 gate_mode（預設 strict）" if mode in (None, "strict") else f"gate_mode 的值不認得：{mode}"}


def recipe_of_run(run: dict) -> dict:
    """run.json 的 recipe；沒有（舊 run）就當成 feature 的完整流程。"""
    rec = run.get("recipe")
    if isinstance(rec, dict) and isinstance(rec.get("resolved"), dict):
        return rec["resolved"]
    return resolve(None, "feature")


def required_sections(profile: dict | None, mode: str) -> list[str]:
    plan = (((profile or {}).get("stages") or {}).get("plan") or {})
    base = plan.get("required_sections") or BASE_SECTIONS
    modes = (profile or {}).get("plan_modes") or {}
    extra = (modes.get(mode) or {}).get("sections") if mode in modes else MODE_SECTIONS.get(mode, [])
    return _union(base, extra or [])


def missing_sections(text: str, sections) -> list[str]:
    """markdown 裡沒有標題或標題下沒有內容的節。標題要含節名（例如「## 4. 驗收條件」）。"""
    lines = text.splitlines()
    heads = [(i, ln) for i, ln in enumerate(lines) if re.match(r"^#{1,6}\s", ln)]
    miss = []
    for s in sections:
        hit = next(((i, ln) for i, ln in heads if s in ln), None)
        if hit is None:
            miss.append(s)
            continue
        i = hit[0]
        nxt = next((j for j, _ in heads if j > i and len(re.match(r"^#+", lines[j]).group(0)) <= len(re.match(r"^#+", lines[i]).group(0))), len(lines))
        body = [ln for ln in lines[i + 1:nxt] if ln.strip() and not re.match(r"^#{1,6}\s", ln)]
        if not body:
            miss.append(f"{s}（只有標題沒有內容）")
    return miss


# ------------------------------------------------------------------ CLI
def _opt(argv, name, default=""):
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    if argv and argv[0] == "--selftest":
        return selftest()
    if argv and argv[0] == "resolve":
        prof = load_profile(Path(_opt(argv, "--profile")) if _opt(argv, "--profile") else None)
        mods = [m for m in _opt(argv, "--modifiers").split(",") if m]
        r = resolve(prof, _opt(argv, "--recipe") or None, mods, _opt(argv, "--size") or None)
        print(json.dumps(r, ensure_ascii=False, indent=2))
        return 1 if r["errors"] else 0
    if argv and argv[0] == "check-diff":
        import subprocess
        run = json.loads((Path(_opt(argv, "--run")) / "run.json").read_text(encoding="utf-8"))
        repo = _opt(argv, "--repo") or run.get("target_repo") or "."
        base = _opt(argv, "--base", "origin/main")
        # --name-status：改名與刪除的舊路徑也算進來（X3 M2：src/app.py 搬到 docs/app.py 不能被當成只改文件）
        p = subprocess.run(["git", "-C", repo, "diff", "--name-status", "-M", f"{base}...HEAD"], capture_output=True, text=True)
        if p.returncode != 0:
            print(f"讀不到 diff：{p.stderr.strip()}（fail-closed）", file=sys.stderr)
            return 1
        paths = [x for ln in p.stdout.splitlines() if ln.strip() for x in ln.split("\t")[1:] if x.strip()]
        errs = diff_shortfalls(recipe_of_run(run), paths)
        for e in errs:
            print(f"recipe 不足：{e}（要升級就用 save_run.py escalate）", file=sys.stderr)
        print("diff 形狀和 recipe 相符" if not errs else f"共 {len(errs)} 項不符")
        return 1 if errs else 0
    print(__doc__)
    return 2


def golden_cases() -> list[tuple[Path, dict]]:
    d = TEAMS / "golden-set" / "recipes"
    return [(p, json.loads(p.read_text(encoding="utf-8"))) for p in sorted(d.glob("*.json"))]


def case_profile(case: dict) -> dict | None:
    prof = case.get("profile", "builtin")
    if prof == "builtin":
        return {"recipes": BUILTIN_RECIPES, "modifiers": BUILTIN_MODIFIERS}
    if prof == "none":
        return None
    return prof


def run_case(case: dict) -> dict:
    """golden case 的實際輸出（JS 版用同一個格式比對）。"""
    r = resolve(case_profile(case), case.get("recipe"), case.get("modifiers"), case.get("size"))
    out = {k: r[k] for k in ("name", "stages", "plan_mode", "research_angles", "design_lenses", "test", "deliver_axes",
                             "build_kinds", "publishable", "followup", "split", "modifiers", "errors")}
    if "old" in case:
        old = resolve(case_profile(case), case["old"].get("recipe"), case["old"].get("modifiers"), case["old"].get("size"))
        out["escalate_errors"] = escalate_errors(r, old)
        merged = merge_stricter(r, old)
        out["merged_stages"] = merged["stages"]
        out["merged_ok"] = not stricter_or_equal(merged, old) and not stricter_or_equal(merged, r)
    if "paths" in case:
        out["shortfalls"] = diff_shortfalls(r, case["paths"])
    if "model_cases" in case:
        out["models"] = [{k: v for k, v in resolve_model(case_profile(case), m.get("role"), m.get("stage"), m.get("suggested"), r["modifiers"]).items()
                          if k != "reason"} for m in case["model_cases"]]
    return out


def selftest() -> int:
    ok = True

    def check(name, cond, detail=""):
        nonlocal ok
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{detail}"))
        ok = ok and cond

    print("recipe selftest")
    bi = {"recipes": BUILTIN_RECIPES, "modifiers": BUILTIN_MODIFIERS}
    check("內建 11 個 recipe、5 個 modifier", len(BUILTIN_RECIPES) == 11 and len(BUILTIN_MODIFIERS) == 5, str(list(BUILTIN_RECIPES)))
    check("每個 recipe 都有 plan 段", all("plan" in r["stages"] for r in BUILTIN_RECIPES.values()))
    check("會寫程式的 recipe 都有 build、deliver、merge",
          all({"build", "deliver", "merge"} <= set(r["stages"]) for r in BUILTIN_RECIPES.values() if r["build_kinds"]))
    check("沒有 recipes 的舊 profile 當成 feature 完整流程", resolve({"name": "x"}, "bugfix")["stages"] == SDLC
          and resolve(None, None)["name"] == "feature")
    check("未知 recipe 當成 feature", resolve(bi, "nope")["name"] == "feature" and resolve(bi, "nope")["stages"] == SDLC)
    bug = resolve(bi, "bugfix")
    check("bugfix 沒有 design、plan_mode=diagnose", "design" not in bug["stages"] and bug["plan_mode"] == "diagnose")
    loose = [f"{m}@{n}" for m in BUILTIN_MODIFIERS for n in BUILTIN_RECIPES if stricter_or_equal(resolve(bi, n, [m]), resolve(bi, n))]
    check("modifier 疊上去只會加嚴（全部組合）", not loose, str(loose))
    mig = resolve(bi, "bugfix", ["migration"])
    check("migration 加回 design 段與 rollback lens", "design" in mig["stages"] and "rollback" in mig["design_lenses"])
    check("不認得的 modifier 記在 errors", resolve(bi, "bugfix", ["zzz"])["errors"] != [])
    feat = resolve(bi, "feature")
    merged = merge_stricter(feat, bug)
    check("escalate：bugfix → feature 接受，合併後不比兩者鬆、保留 regression",
          escalate_errors(feat, bug) == [] and not stricter_or_equal(merged, bug) and not stricter_or_equal(merged, feat)
          and "regression" in merged["test"]["modes"])
    check("escalate：feature → bugfix 少了 design 段，拒絕", escalate_errors(bug, feat) != [])
    check("escalate：hotfix → bugfix 仍保留後續 run 的義務（不能靠升級清掉）", merge_stricter(bug, resolve(bi, "hotfix"))["followup"] is True)
    check("size=small 只影響 feature 的調研角度", resolve(bi, "feature", size="small")["research_angles"] == ["requirement", "code-contracts"])
    docs = resolve(bi, "docs")
    check("docs 改程式 → 不符", diff_shortfalls(docs, ["src/a.py"]) != [] and diff_shortfalls(docs, ["docs/a.md", "README.md"]) == [])
    gm = lambda prof, n, mods=(): gate_mode(prof, {"name": n, "modifiers": list(mods)})["mode"]  # noqa: E731
    check("gate_mode：內建 profile 的 docs、test-only 是 plan-only，其餘 strict", gm(bi, "docs") == "plan-only" and gm(bi, "test-only") == "plan-only"
          and all(gm(bi, n) == "strict" for n in bi["recipes"] if n not in ("docs", "test-only")))
    check("gate_mode：帶任何 modifier 一律 strict；沒有 profile、認不得的 recipe、值寫錯都是 strict", gm(bi, "docs", ["security"]) == "strict" and gm(None, "docs") == "strict"
          and gm(bi, "nope") == "strict" and gm({"recipes": {"docs": {"gate_mode": "none"}}}, "docs") == "strict" and gm({"recipes": {"docs": {}}}, "docs") == "strict")
    check("test-only 改產品程式 → 不符", diff_shortfalls(resolve(bi, "test-only"), ["src/a.py"]) != []
          and diff_shortfalls(resolve(bi, "test-only"), ["tests/test_a.py"]) == [])
    check("bugfix 碰 migration → 不符；加 migration modifier 相符",
          diff_shortfalls(bug, ["db/migrations/001.sql"]) != [] and diff_shortfalls(mig, ["db/migrations/001.sql"]) == [])
    check("bugfix 改依賴宣告 → 不符；deps 相符", diff_shortfalls(bug, ["package.json"]) != []
          and diff_shortfalls(resolve(bi, "deps"), ["package.json", "package-lock.json"]) == [])
    check("spike 不能推送", diff_shortfalls(resolve(bi, "spike"), []) != [])
    check("intent 節：feature 四節、bugfix 另加根因", required_sections(None, "feature") == BASE_SECTIONS
          and "根因" in required_sections(None, "diagnose"))
    md = "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC1\n"
    check("intent 節：空節判缺", missing_sections(md, BASE_SECTIONS) == ["不做的事（只有標題沒有內容）"], str(missing_sections(md, BASE_SECTIONS)))
    # 模型分級（I7）
    mp = {**bi, "models": {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"},
                           "defaults": {"*": "standard", "tpm-researcher": "light"}, "floors": {"tpm-researcher@plan": "standard"}}}
    rm = resolve_model
    check("模型：沒有 models 或 mode=inherit 全部沿用主對話模型", rm(bi, "tpm-researcher", "plan")["model"] == "" and rm(bi, "x", "plan")["mode"] == "inherit"
          and rm({**mp, "models": {**mp["models"], "mode": "inherit"}}, "dev-merge-steward", "merge")["tier"] == "")
    check("模型：AI 建議低於下限會被往上調", rm(mp, "tpm-researcher", "plan", "light")["tier"] == "standard"
          and rm(mp, "tpm-researcher", "design", "light")["tier"] == "light")
    check("模型：AI 可以建議更高", rm(mp, "tpm-researcher", "plan", "strong")["model"] == "opus")
    check("模型：閘門角色的內建下限 profile 蓋不掉", rm({**mp, "models": {**mp["models"], "floors": {"dev-merge-steward": "light"}}}, "dev-merge-steward", "merge", "light")["tier"] == "strong"
          and rm(mp, "tpm-skeptic", "merge", "light")["tier"] == "strong" and rm(mp, "dev-spec-author", "plan", "light")["tier"] == "strong")
    check("模型：mode=fixed 不採 AI 建議", rm({**mp, "models": {**mp["models"], "mode": "fixed"}}, "tpm-writer", "deliver", "light")["tier"] == "standard")
    check("模型：不認得的等級當成沒建議", rm(mp, "tpm-writer", "deliver", "ultra")["tier"] == "standard")
    check("模型：modifier 的 model_floors 只加嚴", rm(mp, "tpm-skeptic", "plan", "light", ["security"])["tier"] == "strong"
          and rm(mp, "tpm-skeptic", "design", "light", [])["tier"] == "light")
    check("模型：plan 的 skeptic 內建下限 standard", rm(mp, "tpm-skeptic", "plan", "light", [])["tier"] == "standard")
    nomf = {**mp, "modifiers": {"security": {"description": "舊 profile：沒有 model_floors"}}}
    check("模型：profile 的 modifier 拿掉 model_floors，內建的下限照樣生效", rm(nomf, "tpm-skeptic", "design", "light", ["security"])["tier"] == "strong")
    check("模型：models 形狀不對不丟例外", rm({"models": {"tiers": "x", "defaults": 3, "floors": []}}, "tpm-writer", "plan")["model"] == "sonnet"
          and rm({"models": {"mode": ""}}, "tpm-writer", "plan")["mode"] == "auto")
    lower = [(r_, st, sg) for r_ in ("tpm-researcher", "dev-merge-steward", "tpm-skeptic", "dev-spec-author", "zzz") for st in SDLC for sg in (None, *TIERS)
             if _tier_rank(rm(mp, r_, st, sg)["tier"]) < _tier_rank(rm(mp, r_, st, sg)["floor"])]
    check("模型：任何組合都不低於下限", not lower, str(lower[:3]))
    check("模型：等級寫 inherit 表示沿用主對話模型", tier_model({"models": {"tiers": {"strong": "inherit"}}}, "strong") == "" and model_up("light") == "standard" and model_up("strong") == "")
    cases = golden_cases()
    check("golden-set/recipes 有案例", len(cases) >= 8, str(len(cases)))
    bad = 0
    for p, case in cases:
        got = run_case(case)
        want = case.get("expect") or {}
        diff = {k: (got.get(k), v) for k, v in want.items() if got.get(k) != v}
        if diff:
            bad += 1
            check(f"golden {p.name}", False, json.dumps(diff, ensure_ascii=False)[:400])
    check(f"golden 案例全部相符（{len(cases)} 個）", bad == 0)
    # X3 I6：這些路徑以前會被當成文件或測試而放行
    docs_r, tonly, bugr = resolve(bi, "docs"), resolve(bi, "test-only"), resolve(bi, "bugfix")
    sneak_docs = ["lib/docs/render.py", "src/doc/api.ts", "docs/conf.py", "CMakeLists.txt", ".claude/teams/rules.md", ".claude/agents/a.md", "CLAUDE.md"]
    sneak_test = ["src/spec/handler.rb", "pkg/e2e/main.go", "app/test/server.go"]
    sneak_bug = ["db/migrate/1.rb", "alembic/versions/a.py", "x.sql", "gradle/libs.versions.toml", "requirements/base.txt", "a.csproj",
                 "Directory.Packages.props", "mix.lock", "pubspec.lock", "constraints.txt", ".github/workflows/ci.yml"]
    leaked = [p for p in sneak_docs if not diff_shortfalls(docs_r, [p])] + [p for p in sneak_test if not diff_shortfalls(tonly, [p])] \
        + [p for p in sneak_bug if not diff_shortfalls(bugr, [p])]
    check("diff 形狀：程式藏在 docs/、spec/ 目錄、.txt、agent／CI 設定、各語言的 migration 與依賴宣告都會被抓到", not leaked, str(leaked))
    check("diff 形狀：正常的文件、測試照樣放行",
          not diff_shortfalls(docs_r, ["docs/a.md", "README.md", "docs/robots.txt"]) and not diff_shortfalls(tonly, ["tests/helpers.py", "src/a.test.ts", "e2e/tests/a.spec.ts"]))
    print("recipe selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
