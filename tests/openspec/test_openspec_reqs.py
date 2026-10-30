#!/usr/bin/env python3
"""openspec-reqs.mjs 的測試：delta 的每個 Scenario 展開成一筆需求，給 merge_gate.py 的 command adapter 用。

- demo-cap（2 條 Requirement、3 個 Scenario）產 3 筆，經 merge_gate.load_requirements 也是 3 筆。
- change 不存在：exit 1，stderr 帶 CLI 診斷的 code，stdout 空。
- REMOVED、只有 RENAMED 的 delta 不產條目；slug 碰撞（兩個 Scenario 得到同一個 id）exit 1。
- slug／scenarioKey 和共用測試向量（vectors/scenario-key-vectors.json）一致。
用法：python3 tests/openspec/test_openspec_reqs.py   （AWF_OPENSPEC_REAL=1 改用真 CLI；沒有 node 時整組略過）
"""
from __future__ import annotations

import json
import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helpers import HERE, TOOLS, TempDir, case_env, run  # noqa: E402

REQS = TOOLS / "openspec-reqs.mjs"
NODE = shutil.which("node")
DEMO = ["demo-cap/匯出報表/正常匯出", "demo-cap/匯出報表/磁碟已滿", "demo-cap/匯出紀錄/寫入紀錄"]


def reqs(case: str, *argv: str, **env_extra: str):
    with TempDir() as tmp:
        cwd, env = case_env(case, tmp, **env_extra)
        return run([NODE, str(REQS), *argv], cwd=cwd, env=env)


@unittest.skipUnless(NODE, "沒有 node")
class OpenSpecReqs(unittest.TestCase):
    def test_demo_cap_three(self):
        r = reqs("design-pass", "add-x")
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertEqual([x["id"] for x in out["requirements"]], DEMO)
        first = out["requirements"][1]
        self.assertEqual(first["source"], "openspec:demo-cap")
        self.assertTrue(first["text"].startswith("匯出報表：磁碟已滿\n- **GIVEN** 磁碟剩餘空間為 0"), first["text"])

    def test_epic_from_env(self):
        r = reqs("design-pass", AWF_EPIC="add-x")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(json.loads(r.stdout)["requirements"]), 3)

    def test_no_epic(self):
        r = reqs("design-pass")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")
        self.assertIn("AWF_EPIC", r.stderr)

    def test_change_missing(self):
        r = reqs("missing", "nope")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")
        self.assertIn("show_error", r.stderr)

    def test_removed_and_renamed_skipped(self):
        r = reqs("reqs-mixed", "add-x")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual([x["id"] for x in json.loads(r.stdout)["requirements"]],
                         ["demo-cap/匯出報表/正常匯出", "demo-cap/匯出報表/顯示進度"])

    def test_slug_collision(self):
        r = reqs("reqs-collision", "add-x")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")
        self.assertIn("demo-cap/匯出報表/逾時-60-秒", r.stderr)

    def test_vectors(self):
        vec = HERE / "vectors" / "scenario-key-vectors.json"
        script = (
            "import { slug, scenarioKey } from " + json.dumps(REQS.as_uri()) + ";"
            "import { readFileSync } from 'node:fs';"
            "const v = JSON.parse(readFileSync(" + json.dumps(str(vec)) + ", 'utf8'));"
            "const bad = [];"
            "for (const c of v.slug) if (slug(c.input) !== c.expected) bad.push(c.id);"
            "for (const c of v.scenarioKey) if (scenarioKey(c.capability, c.requirement, c.scenario) !== c.expected) bad.push(c.id);"
            "for (const c of v.collisions) if (slug(c.a) !== c.slug || slug(c.b) !== c.slug) bad.push(c.id);"
            "console.log(JSON.stringify({ n: v.slug.length + v.scenarioKey.length + v.collisions.length, bad }));"
        )
        with TempDir() as tmp:
            r = run([NODE, "--input-type=module", "-e", script], cwd=tmp, env=case_env("design-pass", tmp)[1])
        self.assertEqual(r.returncode, 0, r.stderr)
        res = json.loads(r.stdout)
        self.assertGreaterEqual(res["n"], 20)
        self.assertEqual(res["bad"], [])

    def test_merge_gate_three(self):
        # merge_gate 的 command adapter：profile 的 command 不帶參數，epic 由 AWF_EPIC 帶入。
        # command 不以引號開頭：Windows 的 cmd /c 遇到開頭是引號、整串又有多組引號時會剝掉最外層引號
        with TempDir() as tmp:
            cwd, env = case_env("design-pass", tmp, AWF_EPIC="add-x")
            code = (
                "import sys, json; from pathlib import Path; sys.path.insert(0, " + json.dumps(str(TOOLS)) + "); import merge_gate as m;"
                "cfg = {'requirements': {'source': 'command', 'command': " + json.dumps(f'node "{REQS}"') + "}};"
                "r, label = m.load_requirements(cfg, Path('.'), None, None, {});"
                "print(json.dumps([x['id'] for x in r], ensure_ascii=False))"
            )
            r = run([sys.executable, "-c", code], cwd=cwd, env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout), DEMO)


if __name__ == "__main__":
    unittest.main(verbosity=2)
