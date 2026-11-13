#!/usr/bin/env python3
"""錄 recipe 解析與模型分級的對跑基準：recipe.py 每個公開函式對一批輸入的實際輸出。

用法：python3 tests/parity/record_recipe.py > tests/parity/recipe.expected.json
內容：
  - builtins：內建常數（鍵的順序也要一樣）。
  - golden：golden-set/recipes 每個案例的 run_case 輸出（含 model_cases）。
  - probes：一行一筆 {"fn", "args", "ok" 或 "ok_sha1" 或 "raise"}。除了正常案例，另外把各欄位換成形狀不對的值；
    Python 丟例外的記例外類別名，另一個實作要回同一類的錯誤。
    為了不讓基準檔太大：長的參數放在 values 表、參數寫 {"$v": 編號}；「把某份資料的某個欄位換掉／拿掉」寫成
    {"$put": [資料, 路徑, 值]}、{"$drop": [資料, 路徑]}；長的輸出只記 json.dumps(輸出, ensure_ascii=False) 的 sha1。
  - cli：指令列的 exit code、stdout、stderr（check-diff 的 git 輸出是假的，連同收到的參數一起記）。
  - selftest：--selftest 的輸出。
同一個實作連錄兩次要逐字相同。這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
import copy
import hashlib
import io
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TEAMS = ROOT / "scaffold" / ".claude" / "teams"
sys.path.insert(0, str(TEAMS / "tools"))
import recipe  # noqa: E402

BI = {"recipes": recipe.BUILTIN_RECIPES, "modifiers": recipe.BUILTIN_MODIFIERS}
FNS = {
    "resolve": recipe.resolve,
    "resolve_model": recipe.resolve_model,
    "tier_model": recipe.tier_model,
    "model_up": recipe.model_up,
    "stricter_or_equal": recipe.stricter_or_equal,
    "merge_stricter": recipe.merge_stricter,
    "escalate_errors": recipe.escalate_errors,
    "classify": lambda p: sorted(recipe.classify(p)),
    "diff_shortfalls": recipe.diff_shortfalls,
    "gate_mode": recipe.gate_mode,
    "recipe_of_run": recipe.recipe_of_run,
    "required_sections": recipe.required_sections,
    "missing_sections": recipe.missing_sections,
    "case_profile": recipe.case_profile,
    "run_case": recipe.run_case,
}
# 形狀不對的值：每個欄位都換一輪
WEIRD = [None, 0, 1, True, False, 1.5, "", "plan", "always", "never", "x", [], ["plan"], ["verify", "x"], [1, "plan", None],
         [["x"]], {}, {"a": 1}, {"plan": 1, "modes": None}]
probes = []
values, value_ids = [], {}
dump = lambda x: json.dumps(x, ensure_ascii=False)  # noqa: E731


class Put:
    """base 的複本，把 path（用 . 分隔）換成 value。"""

    def __init__(self, base, path, value):
        self.base, self.path, self.value = base, path, value

    def enc(self):
        return {"$put": [enc(self.base), self.path, self.value]}

    def make(self):
        d = copy.deepcopy(make(self.base))
        cur, keys = d, self.path.split(".")
        for k in keys[:-1]:
            cur = cur[k]
        self.set(cur, keys[-1])
        return d

    def set(self, cur, key):
        cur[key] = self.value


class Drop(Put):
    def __init__(self, base, path):
        super().__init__(base, path, None)

    def enc(self):
        return {"$drop": [enc(self.base), self.path]}

    def set(self, cur, key):
        del cur[key]


def make(x):
    return x.make() if isinstance(x, Put) else x


def enc(x):
    if isinstance(x, Put):
        return x.enc()
    text = dump(x)
    if len(text) <= 120:
        return json.loads(text)
    if text not in value_ids:
        value_ids[text] = len(values)
        values.append(json.loads(text))
    return {"$v": value_ids[text]}


def probe(fn, *args):
    real = [json.loads(json.dumps(make(a))) for a in args]
    rec = {"fn": fn, "args": [enc(a) for a in args]}
    ok = None
    try:
        ok = FNS[fn](*copy.deepcopy(real))
        text = dump(ok)
        if len(text) > 100:
            rec["ok_sha1"] = hashlib.sha1(text.encode("utf-8")).hexdigest()
        else:
            rec["ok"] = ok
    except Exception as e:  # noqa: BLE001
        rec["raise"] = type(e).__name__
    probes.append(rec)
    return ok


put = lambda doc, path, value: Put(doc, path, value).make()  # noqa: E731
drop = lambda doc, path: Drop(doc, path).make()  # noqa: E731

NAMES = list(recipe.BUILTIN_RECIPES)
MODS = list(recipe.BUILTIN_MODIFIERS)
MOD_SETS = [[]] + [[m] for m in MODS] + [["ui", "security"], ["migration", "public_api"], list(MODS), ["large", "zzz"], ["security", "security"]]

# ---- resolve：內建 profile 的各種組合
resolved = {}
for n in NAMES:
    for ms in MOD_SETS:
        for size in (None, "small"):
            resolved[(n, tuple(ms), size)] = probe("resolve", BI, n, ms, size)
for prof in (None, {}, {"name": "舊 profile"}, {"recipes": {}}, {"recipes": {"bugfix": recipe.BUILTIN_RECIPES["bugfix"]}},
             {"recipes": recipe.BUILTIN_RECIPES}, {"modifiers": recipe.BUILTIN_MODIFIERS},
             {"recipes": recipe.BUILTIN_RECIPES, "modifiers": {"ui": recipe.BUILTIN_MODIFIERS["ui"]}},
             {"recipes": recipe.BUILTIN_RECIPES, "modifiers": {"ui": {}, "security": None}}):
    for n in (None, "", "feature", "bugfix", "nope", "spike"):
        for ms in (None, [], ["ui"], ["security", "nope"], ["migration"]):
            probe("resolve", prof, n, ms, None)
# 自訂 recipe：缺欄位、沒有 plan 段、段的順序亂掉、沒有 lens 的 design、沒有 verify 的 test
CUSTOM = {
    "recipes": {
        "bare": {},
        "noplan": {"stages": ["build", "merge"], "plan_mode": "feature"},
        "shuffled": {"stages": ["merge", "test", "design", "plan", "zzz", "plan"], "plan_mode": "impact", "test": {"modes": ["regression"]}},
        "nolens": {"stages": ["plan", "design"], "design_lenses": [], "test": {"design_review": "always"}},
        "named": {"stages": ["plan"], "name": "原本的名字", "errors": ["原本的"], "size": "原本的", "small": {"split": 1, "research_angles": ["a", "a"]}},
        "feature": {"stages": ["plan", "build"], "plan_mode": "feature", "split": True, "small": {"split": 0}},
    },
    "modifiers": {
        "strict": {"add_stages": ["test", "design"], "test": {"design_review": "always", "security_review": "never", "modes": ["x", "verify"]},
                   "research_angles": ["upstream"], "design_lenses": ["z"], "deliver_axes": ["bugs", "bugs"], "split": 1},
        "loose": {"test": {"design_review": "never", "security_review": "weird"}},
        "empty": {},
    },
}
for n in list(CUSTOM["recipes"]) + ["nope", None, 5, True, 1.5, ["bare"], {"a": 1}]:
    for ms in ([], ["strict"], ["loose", "strict"], ["empty"], ["ui"], "strict", {"strict": 1, "loose": 2}):
        for size in (None, "small", "large", 1):
            probe("resolve", CUSTOM, n, ms, size)
for w in WEIRD:
    probe("resolve", w, "bugfix", ["ui"], None)
    probe("resolve", {"recipes": w}, "plan", None, None)
    probe("resolve", {"recipes": w}, None, None, None)
    probe("resolve", {"recipes": w}, 1, None, None)
    probe("resolve", {"recipes": recipe.BUILTIN_RECIPES, "modifiers": w}, "bugfix", ["ui", "plan"], None)
    probe("resolve", BI, w, None, None)
    probe("resolve", BI, "bugfix", w, None)
    probe("resolve", BI, "bugfix", [w], None)
    probe("resolve", BI, "feature", None, w)
    probe("resolve", {"recipes": {"c": w}}, "c", None, None)
FULL = {**recipe.BUILTIN_RECIPES["feature"], "gate_mode": "plan-only"}
FULL_MOD = {"add_stages": ["design"], "research_angles": ["upstream"], "design_lenses": ["rollback"], "deliver_axes": ["bugs"],
            "test": {"design_review": "always", "security_review": "always", "modes": ["regression"]}, "split": True, "model_floors": {"tpm-skeptic": "strong"}}
FUZZ = {"recipes": {"c": FULL, "d": recipe.BUILTIN_RECIPES["docs"]}, "modifiers": {"m": FULL_MOD}}
for field in list(FULL) + ["test.design_review", "test.security_review", "test.modes", "small.research_angles", "small.split"]:
    for prof in [Put(FUZZ, "recipes.c." + field, w) for w in WEIRD] + [Drop(FUZZ, "recipes.c." + field)]:
        for ms in ([], ["m"]):
            probe("resolve", prof, "c", ms, "small")
for field in list(FULL_MOD) + ["test.design_review", "test.security_review", "test.modes"]:
    for prof in [Put(FUZZ, "modifiers.m." + field, w) for w in WEIRD] + [Drop(FUZZ, "modifiers.m." + field)]:
        probe("resolve", prof, "c", ["m"], None)
        probe("resolve", prof, "d", ["m"], None)

# ---- stricter_or_equal、merge_stricter、escalate_errors：內建兩兩組合，再把欄位換成怪值
plain = {n: resolved[(n, (), None)] for n in NAMES}
for a in NAMES:
    for b in NAMES:
        probe("stricter_or_equal", plain[a], plain[b])
        probe("escalate_errors", plain[a], plain[b])
        probe("merge_stricter", plain[a], plain[b])
for ms in MOD_SETS[1:]:
    for n in ("bugfix", "docs", "feature"):
        withm = resolved[(n, tuple(ms), None)]
        for fn in ("stricter_or_equal", "merge_stricter", "escalate_errors"):
            probe(fn, withm, plain[n])
            probe(fn, plain[n], withm)
R0, R1 = plain["feature"], plain["hotfix"]
for fn in ("stricter_or_equal", "merge_stricter", "escalate_errors"):
    for w in WEIRD:
        probe(fn, w, R0)
        probe(fn, R0, w)
    for field in ("stages", "design_lenses", "deliver_axes", "research_angles", "test", "test.design_review", "test.security_review", "test.modes",
                  "followup", "modifiers", "name"):
        for w in WEIRD[::2] + ["never", ["verify", "x"]]:
            probe(fn, Put(R0, field, w), R1)
            probe(fn, R1, Put(R0, field, w))
        probe(fn, Drop(R0, field), R1)
        probe(fn, R1, Drop(R0, field))
    probe(fn, {}, {})
    probe(fn, {}, R0)
    probe(fn, R0, {})

# ---- classify、diff_shortfalls
PATHS = [
    "src/a.py", "README.md", "docs/a.md", "docs/guide.MDX", "notes.rst", "a.adoc", "docs/robots.txt", "doc/a.txt", "notes.txt", "CMakeLists.txt",
    "docs/conf.py", "lib/docs/render.py", "src/doc/api.ts", "docs/sub/dir/x.TXT", "docsx/a.txt",
    "tests/test_a.py", "tests/helpers.py", "test/x.go", "__tests__/a.js", "e2e/tests/a.spec.ts", "spec/a_spec.rb", "src/a.test.ts", "src/a.spec.tsx",
    "pkg/a_test.go", "conftest.py", "sub/conftest.py", "src/spec/handler.rb", "pkg/e2e/main.go", "app/test/server.go", "test_a.py", "src/test_a.py",
    "test_.py", "test_a", "a_test", "a_test.", "x.test.", "Tests/Thing.cs", "TESTS/a.py", "testing/a.py", "tests", "tests/", "src/test_dir/a.py",
    "tests/README.md", "tests/data.sql", "tests/package.json", "tests/test_x.sql",
    "db/migrations/001.sql", "db/migrate/1.rb", "alembic/versions/a.py", "x.sql", "migrations/0001_init.py", "migration/a.py", "app/flyway/V1.sql",
    "liquibase/changelog.xml", "src/Migrations/A.cs", "mymigrations/a.py", "docs/migrations/readme.md", "x.sqlite", "X.SQL",
    "package.json", "package-lock.json", "sub/package.json", "yarn.lock", "pnpm-lock.yaml", "npm-shrinkwrap.json", "bun.lock", "bun.lockb", "bun.lockbb",
    "poetry.lock", "uv.lock", "Pipfile", "Pipfile.lock", "Cargo.lock", "Cargo.toml", "go.sum", "go.mod", "Gemfile", "Gemfile.lock", "composer.json",
    "composer.lock", "packages.lock.json", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "requirements-dev.txt", "constraints.txt",
    "requirements/base.txt", "requirements/dev.in", "requirements/sub/x.txt", "docs/requirements.txt", "pom.xml", "build.gradle", "build.gradle.kts",
    "settings.gradle", "gradle.lockfile", "gradle/libs.versions.toml", "a.csproj", "src/App/App.fsproj", ".csproj", "Directory.Packages.props",
    "packages.config", "mix.exs", "mix.lock", "pubspec.yaml", "pubspec.lock", "Podfile", "Podfile.lock", "Package.swift", "Package.resolved",
    "deno.json", "deno.jsonc", "deno.lock", "vcpkg.json", "conanfile.txt", "conanfile.py", "flake.lock", "mypackage.json", "package.json.bak",
    "xsetup.py", "PACKAGE.JSON",
    ".claude/teams/rules.md", ".claude/agents/a.md", ".github/workflows/ci.yml", "CLAUDE.md", "sub/AGENTS.md", "claude.md", ".mcp.json", "a/.mcp.json",
    "x.mcp.json", "sub/.claude/a.md", ".githubx/a", "MYCLAUDE.md",
    "", ".", "./", "./src/a.py", "././docs/a.md", ".\\docs\\a.md", "src\\tests\\a.py", "tests\\a.py", "\\tests\\a.py", "/tests/a.py", "/docs/a.txt",
    "x.sql\n", "docs/a.md\n", "docs/a.md\n\n", "docs/a\nb.txt", "docs/a.md ", " docs/a.md", "tests/a\n", "a_test.py\n", "CLAUDE.md\n", "package.json\n",
    "文件/說明.md", "測試/test_中文.py", "src/程式.py",
    "MİGRATİONS/a.py", "mıgratıons/a.py", "a_test.pı", "a_test.pİ", "pacKage.json", "ſetup.py", "x.ſql", "teſt/a.py", "docs/a.tKt",
    "x.ſQL", "TEST_A.PY", "a_TEST.GO", "a.SPEC.TS", "x.test.ts2", "x.test.é",
]
for p in PATHS:
    probe("classify", p)
for w in WEIRD:
    probe("classify", w)
PATH_SETS = [
    [], ["src/a.py"], ["docs/a.md", "README.md"], ["tests/test_a.py"], ["db/migrations/001.sql"], ["package.json", "package-lock.json"],
    [".github/workflows/ci.yml", "CLAUDE.md"], ["src/a.py", "tests/test_a.py", "docs/a.md"],
    ["a%d.py" % i for i in range(8)], ["t/%d_test.go" % i for i in range(7)] + ["m%d.sql" % i for i in range(5)] + ["d%d.md" % i for i in range(6)],
    ["requirements.txt", "go.mod", "Cargo.toml", "uv.lock"], [".claude/a", ".github/b", "CLAUDE.md", ".mcp.json"],
    ["tests/data.sql", "tests/package.json"], PATHS,
]
shapes = dict(plain)
shapes.update({"bugfix+migration": resolved[("bugfix", ("migration",), None)], "bugfix+security": resolved[("bugfix", ("security",), None)],
               "docs+public_api": resolved[("docs", ("public_api",), None)], "feature+all": resolved[("feature", tuple(MODS), None)]})
for r in shapes.values():
    for ps in PATH_SETS:
        probe("diff_shortfalls", r, ps)
RB = plain["bugfix"]
MIXED = ["src/a.py", "tests/test_a.py", "docs/a.md", "x.sql", "package.json", "CLAUDE.md"]
for field in ("publishable", "name", "build_kinds", "stages", "design_lenses", "test", "test.security_review"):
    for w in WEIRD + [["doc", "code"], ["code", "code", "doc"], [1, 2.5, True, 0], [2, 1, 1.0], [None, None], [None, "a"], ["b", 1], "code", "tcd",
                      {"test": 1, "doc": 2}, ["rollback"], ["design"], "design rollback"]:
        probe("diff_shortfalls", Put(RB, field, w), MIXED)
    probe("diff_shortfalls", Drop(RB, field), MIXED)
    probe("diff_shortfalls", Drop(RB, field), [])
for w in WEIRD + ["src/a.py", {"src/a.py": 1, "docs/a.md": 2}, ["src/a.py", None], [None], [["src/a.py"]], [1], ["x.sql", 1]]:
    probe("diff_shortfalls", RB, w)
    probe("diff_shortfalls", w, ["src/a.py"])
    probe("diff_shortfalls", plain["spike"], w)

# ---- gate_mode
GM_PROFILES = [None, {}, BI, {"recipes": recipe.BUILTIN_RECIPES}, {"recipes": {"docs": {"gate_mode": "none"}, "a": {}, "b": {"gate_mode": "strict"},
               "c": {"gate_mode": None}, "d": {"gate_mode": ["plan-only"]}, "e": {"gate_mode": "plan-only"}, "f": None, "5": {"gate_mode": "plan-only"},
               "True": {"gate_mode": "plan-only"}, "g": {"gate_mode": 0}, "h": {"gate_mode": True}}}]
for prof in GM_PROFILES:
    for n in NAMES + ["nope", "", None, "a", "b", "c", "d", "e", "f", "g", "h", 5, True, 1.5, ["docs"], {"a": 1}]:
        for ms in (None, ["security"], ["", None, 0], ["", "x", 3, None, ["y"], {"k": 1}, True]):
            probe("gate_mode", prof, {"name": n, "modifiers": ms})
for w in WEIRD:
    probe("gate_mode", w, {"name": "docs"})
    probe("gate_mode", {"recipes": w}, {"name": "plan"})
    probe("gate_mode", BI, w)
    probe("gate_mode", BI, {"name": "docs", "modifiers": w})
    probe("gate_mode", BI, {"name": w})
    probe("gate_mode", {"recipes": {"docs": w}}, {"name": "docs"})
    probe("gate_mode", {"recipes": {"docs": {"gate_mode": w}}}, {"name": "docs"})
for n in ("docs", "test-only", "bugfix"):
    for ms in MOD_SETS[:3]:
        probe("gate_mode", BI, resolved[(n, tuple(ms), None)])

# ---- recipe_of_run
for run in ({}, {"recipe": None}, {"recipe": {}}, {"recipe": {"resolved": None}}, {"recipe": {"resolved": {}}}, {"recipe": {"resolved": plain["docs"]}},
            {"recipe": {"resolved": []}}, {"recipe": "docs"}, {"recipe": {"name": "docs"}}, {"recipe": {"resolved": {"name": "x"}, "requested": "x"}}):
    probe("recipe_of_run", run)
for w in WEIRD:
    probe("recipe_of_run", w)
    probe("recipe_of_run", {"recipe": w})
    probe("recipe_of_run", {"recipe": {"resolved": w}})

# ---- required_sections、missing_sections
RS_PROFILES = [None, {}, BI, {"stages": {"plan": {"required_sections": ["目標", "背景"]}}}, {"stages": {"plan": {"required_sections": []}}},
               {"plan_modes": {"diagnose": {"sections": ["根因", "目標", "影響"]}, "feature": {}, "impact": None, "custom": {"sections": ["自訂節"]},
                               "revert": {"sections": None}, "baseline": {"sections": "ab"}}},
               {"stages": {"plan": {"required_sections": ["甲", "甲", "乙"]}}, "plan_modes": {"research": {"sections": ["乙", "丙", "丙"]}}}]
for prof in RS_PROFILES:
    for mode in recipe.PLAN_MODES + ["custom", "nope", "", None, 1, True, ["feature"], {"a": 1}]:
        probe("required_sections", prof, mode)
for w in WEIRD:
    probe("required_sections", w, "diagnose")
    probe("required_sections", {"stages": w}, "diagnose")
    probe("required_sections", {"stages": {"plan": w}}, "diagnose")
    probe("required_sections", {"stages": {"plan": {"required_sections": w}}}, "diagnose")
    probe("required_sections", {"plan_modes": w}, "plan")
    probe("required_sections", {"plan_modes": {"diagnose": w}}, "diagnose")
    probe("required_sections", {"plan_modes": {"diagnose": {"sections": w}}}, "diagnose")
    probe("required_sections", None, w)
TEXTS = [
    "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\n\n## 驗收條件\n- AC1\n",
    "", "沒有標題", "## 目標", "## 目標\n", "## 目標\n   \n\t\n", "## 目標\n### 子節\n內容\n## 範圍\n",
    "## 目標\n### 子節\n## 範圍\nx", "# 目標\n## 範圍\nx\n# 不做的事\n## 驗收條件\n", "##目標\nx\n## 範圍\nx",
    "####### 目標\nx\n###### 範圍\nx\n", "## 1. 目標與範圍\nx\n## 4. 驗收條件\n\n- AC\n", "## 目標\r\nx\r\n## 範圍\r\n\r\n## 不做的事\rx\r## 驗收條件",
    "## 目標\x0bx\x0c## 範圍\x1c## 不做的事\x1dx\x1e## 驗收條件\x85y", "## 目標 x ## 範圍\n　\n\xa0\n## 不做的事\n\x1f\n## 驗收條件\n​\n",
    "#\t目標\nx\n#　範圍\nx\n#\xa0不做的事\nx\n#\x1f驗收條件\nx", " ## 目標\nx\n## 範圍 \n  x  \n", "## 目標\n## 目標\nx\n", "## 範圍\nx\n## 目標\n\n### 範圍\n",
    "# A\n## 目標\n### B\n#### C\n# 範圍\ntext\n", "## 目標\n```\n# 不是標題？\n```\n## 範圍\n# \n", "# \n## \nx\n", "#\n## 目標\n#\n",
]
SECTION_SETS = [recipe.BASE_SECTIONS, [], ["目標"], ["", "目標"], ["#"], ["目標", "目標"], ["不存在"], "目標範圍", {"目標": 1, "範圍": 2},
                [1, None], ["目標", 1], [None, "目標"], [["目標"]], None, 0, ["x"]]
for t in TEXTS:
    for secs in SECTION_SETS:
        probe("missing_sections", t, secs)

# ---- 模型分級
MP = {**BI, "models": {"mode": "auto", "tiers": {"light": "haiku", "standard": "sonnet", "strong": "opus"},
                       "defaults": {"*": "standard", "tpm-researcher": "light", "tpm-writer@deliver": "strong", "dev-verifier": "ultra"},
                       "floors": {"tpm-researcher@plan": "standard", "dev-merge-steward": "light", "tpm-writer": "weird"}}}
MODEL_PROFILES = [
    None, {}, BI, MP, put(MP, "models.mode", "fixed"), put(MP, "models.mode", "inherit"), put(MP, "models.mode", ""), put(MP, "models.mode", "weird"),
    {"models": {}}, {"models": {"mode": "auto"}}, {"models": {"tiers": "x", "defaults": 3, "floors": []}},
    {"models": {"defaults": {"*": "light"}, "tiers": {"light": "", "standard": "inherit", "strong": 5}}},
    {**MP, "modifiers": {"security": {"description": "沒有 model_floors"}, "mine": {"model_floors": {"tpm-writer": "strong", "tpm-writer@plan": "light"}},
                         "bad": {"model_floors": ["x"]}, "str": "x"}},
]
ROLES = ["tpm-researcher", "dev-merge-steward", "tpm-skeptic", "dev-spec-author", "dev-security-reviewer", "tpm-writer", "dev-verifier", "zzz"]
for prof in MODEL_PROFILES:
    inherit = recipe.resolve_model(prof, "x", "plan")["mode"] == "inherit"
    for role in (ROLES[:2] if inherit else ROLES):
        for stage in (("plan",) if inherit else ("plan", "merge")):
            for sug in (None, "light", "strong", "ultra"):
                probe("resolve_model", prof, role, stage, sug, None)
for prof in (MP, MODEL_PROFILES[-1]):
    for role in ("tpm-skeptic", "tpm-writer"):
        for stage in ("plan", "design"):
            for sug in (None, "", "light", "ultra", 0, ["light"]):
                for ms in ([], ["security"], ["security", "large", "mine"], ["mine", "bad", "str", "nope"], [None, 1, ["security"], "security"],
                           "security", {"security": 1}):
                    probe("resolve_model", prof, role, stage, sug, ms)
for w in WEIRD:
    probe("resolve_model", w, "tpm-writer", "plan", "light", None)
    probe("resolve_model", {"models": w}, "tpm-writer", "plan", "light", None)
    probe("resolve_model", MP, w, "plan", "light", None)
    probe("resolve_model", MP, "tpm-skeptic", w, "light", None)
    probe("resolve_model", MP, "tpm-skeptic", "plan", w, None)
    probe("resolve_model", MP, "tpm-skeptic", "plan", "light", w)
    probe("resolve_model", MP, "tpm-skeptic", "plan", "light", [w])
    probe("resolve_model", {**MP, "modifiers": w}, "tpm-skeptic", "design", "light", ["security", "plan"])
    for field in ("mode", "tiers", "defaults", "floors", "tiers.standard", "defaults.*", "floors.tpm-researcher@plan"):
        probe("resolve_model", Put(MP, "models." + field, w), "tpm-researcher", "plan", "light", ["security"])
        probe("resolve_model", Put(MP, "models." + field, w), "zzz", "build", None, None)
    probe("tier_model", w, "standard")
    probe("tier_model", {"models": w}, "standard")
    probe("tier_model", {"models": {"tiers": w}}, "standard")
    probe("tier_model", {"models": {"tiers": {"standard": w}}}, "standard")
    probe("tier_model", MP, w)
    probe("model_up", w)
for tier in recipe.TIERS + ["", "ultra", "inherit"]:
    probe("model_up", tier)
    for prof in MODEL_PROFILES:
        probe("tier_model", prof, tier)

# ---- case_profile、run_case（golden 以外的形狀）
for case in ({}, {"profile": "builtin"}, {"profile": "none"}, {"profile": None}, {"profile": {"recipes": {}}}, {"profile": "other"}, {"profile": 1}):
    probe("case_profile", case)
    probe("run_case", case)
for case in ({"recipe": "bugfix", "modifiers": ["migration"], "old": {"recipe": "feature"}, "paths": ["x.sql"], "model_cases": [{"role": "tpm-skeptic", "stage": "plan"}]},
             {"recipe": "hotfix", "old": {}}, {"recipe": "docs", "old": {"recipe": "feature", "modifiers": ["ui"], "size": "small"}},
             {"recipe": "docs", "old": None}, {"recipe": "docs", "paths": None}, {"recipe": "docs", "model_cases": None}, {"recipe": "docs", "model_cases": [None]},
             {"recipe": "docs", "model_cases": [{}], "profile": MP}, {"profile": {"recipes": {"feature": {}}}}, {"profile": {"recipes": {"feature": {"plan_mode": "x"}}}}):
    probe("run_case", case)
for w in WEIRD:
    probe("case_profile", w)
    probe("run_case", w)

# ---- golden-set/recipes
golden = [{"file": p.name, "out": recipe.run_case(case)} for p, case in recipe.golden_cases()]

# ---- 指令列
real_run = subprocess.run


def cli(argv, run_json=None, git=None):
    """跑 recipe.main；@ROOT@ 換成 repo 根目錄，@RUN@ 換成放 run.json 的暫存目錄。git 給假的回傳。"""
    with tempfile.TemporaryDirectory() as tmp:
        if run_json is not None:
            (Path(tmp) / "run.json").write_text(run_json, encoding="utf-8")
        real = [a.replace("@ROOT@", ROOT.as_posix()).replace("@RUN@", tmp) for a in argv]
        seen = []

        def fake(cmd, **kw):
            seen.append([str(c).replace(tmp, "@RUN@") for c in cmd])
            return subprocess.CompletedProcess(cmd, git["code"], git["stdout"], git["stderr"])

        out, err = io.StringIO(), io.StringIO()
        subprocess.run = fake
        rec = {"argv": argv, "run_json": run_json, "git": git}
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                rec["code"] = recipe.main(real)
        except Exception as e:  # noqa: BLE001
            # 沒接住的例外：Python 印 traceback、exit 1。另一個實作只要 exit code 一樣
            rec.update({"code": 1, "crash": type(e).__name__})
        finally:
            subprocess.run = real_run
        rec.update({"stdout": out.getvalue().replace(tmp, "@RUN@"), "stderr": err.getvalue().replace(tmp, "@RUN@"), "git_argv": seen})
        return rec


DEFAULT = "@ROOT@/scaffold/.claude/teams/methodologies/default.json"
LIGHT = "@ROOT@/scaffold/.claude/teams/methodologies/lightweight.json"
OK = {"code": 0, "stderr": ""}
cli_cases = [cli(a) for a in (
    [], ["nope"], ["--help"], ["resolve"], ["resolve", "--recipe", "bugfix"], ["resolve", "--recipe"], ["resolve", "--recipe", "nope", "--modifiers", "ui,,zzz"],
    ["resolve", "--profile", DEFAULT, "--recipe", "bugfix"], ["resolve", "--profile", DEFAULT, "--recipe", "docs", "--modifiers", "ui,security", "--size", "small"],
    ["resolve", "--profile", DEFAULT, "--recipe", "feature", "--size", "small"], ["resolve", "--profile", DEFAULT, "--recipe", "bugfix", "--modifiers", "zzz"],
    ["resolve", "--profile", DEFAULT, "--modifiers", "migration,public_api,large"], ["resolve", "--profile", LIGHT, "--recipe", "hotfix", "--modifiers", "migration"],
    ["resolve", "--profile", "@ROOT@/不存在.json", "--recipe", "bugfix"], ["resolve", "--profile", "@ROOT@/README.md", "--recipe", "bugfix"],
    ["resolve", "--recipe", "resolve", "--size"], ["resolve", "--size", "small", "--recipe", "--size"],
)]
RUN_DOCS = json.dumps({"recipe": {"resolved": plain["docs"]}, "target_repo": "../app"}, ensure_ascii=False)
RUN_BUG = json.dumps({"recipe": {"resolved": plain["bugfix"]}}, ensure_ascii=False)
STATUS = "M\tsrc/a.py\nR100\tsrc/app.py\tdocs/app.py\nD\told.sql\n\nA\tdocs/a.md\n \t \nM\tpackage.json\r\nA\t.github/workflows/ci.yml\n"
for argv, run_json, git in (
    (["check-diff", "--run", "@RUN@"], RUN_DOCS, {**OK, "stdout": "M\tdocs/a.md\nA\tREADME.md\n"}),
    (["check-diff", "--run", "@RUN@"], RUN_DOCS, {**OK, "stdout": STATUS}),
    (["check-diff", "--run", "@RUN@", "--base", "main", "--repo", "/srv/repo"], RUN_BUG, {**OK, "stdout": STATUS}),
    (["check-diff", "--run", "@RUN@", "--base"], RUN_BUG, {**OK, "stdout": ""}),
    (["check-diff", "--run", "@RUN@", "--repo", ""], "{}", {**OK, "stdout": "M\tsrc/a.py\nM\ttests/test_a.py\n"}),
    (["check-diff", "--run", "@RUN@"], '{"target_repo": ""}', {**OK, "stdout": "M\tx.sql\n"}),
    (["check-diff", "--run", "@RUN@"], '{"recipe": {"resolved": {"name": "x", "publishable": false}}}', {**OK, "stdout": ""}),
    (["check-diff", "--run", "@RUN@"], RUN_DOCS, {"code": 128, "stdout": "", "stderr": "\nfatal: bad revision 'origin/main...HEAD'\n  \n"}),
    (["check-diff", "--run", "@RUN@"], RUN_DOCS, {"code": 1, "stdout": "M\tsrc/a.py\n", "stderr": ""}),
    (["check-diff", "--run", "@RUN@"], None, {**OK, "stdout": ""}),
    (["check-diff", "--run", "@RUN@"], "不是 JSON", {**OK, "stdout": ""}),
    (["check-diff", "--run", "@RUN@"], "[]", {**OK, "stdout": ""}),
    (["check-diff", "--run", "@RUN@"], '{"recipe": {"resolved": {"build_kinds": 5}}}', {**OK, "stdout": "M\ta.py\n"}),
):
    cli_cases.append(cli(argv, run_json, git))

buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    code = recipe.selftest()

builtins = {k: getattr(recipe, k) for k in (
    "SDLC", "LEVEL", "PLAN_MODES", "ALL_ANGLES", "ALL_AXES", "CODE_KINDS", "BASE_SECTIONS", "MODE_SECTIONS", "FALLBACK_FEATURE", "BUILTIN_RECIPES",
    "BUILTIN_MODIFIERS", "TIERS", "BUILTIN_TIER_MODELS", "BUILTIN_MODEL_FLOORS", "GATE_MODES")}
builtins["USAGE"] = recipe.__doc__

w = sys.stdout.write
w("{\n")
w('"builtins": ' + dump(builtins) + ",\n")
w('"selftest": ' + dump({"code": code, "stdout": buf.getvalue()}) + ",\n")
for key, rows in (("golden", golden), ("cli", cli_cases), ("values", values), ("probes", probes)):
    w('"%s": [\n' % key + ",\n".join(dump(r) for r in rows) + "\n]" + ("\n" if key == "probes" else ",\n"))
w("}\n")
