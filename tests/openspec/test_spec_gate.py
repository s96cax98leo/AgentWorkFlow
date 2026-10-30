#!/usr/bin/env python3
"""spec_gate.py 的測試：plan／design／build／test 各段的過關與不過關，加 CLI 故障（版本不符、找不到、逾時）。

用法：python3 tests/openspec/test_spec_gate.py        （預設回放錄好的 CLI 輸出，不需要安裝 OpenSpec）
      AWF_OPENSPEC_REAL=1 python3 tests/openspec/test_spec_gate.py   （改用 PATH 上的真 OpenSpec 1.15.x）
"""
from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helpers import INPUTS, TOOLS, TempDir, case_env, run  # noqa: E402

GATE = TOOLS / "spec_gate.py"
KEYS = {"engine", "cli_version", "stage", "change", "pass", "checks", "at"}


def gate(case: str, stage: str, *extra: str, change: str = "add-x", **env_extra: str):
    with TempDir() as tmp:
        cwd, env = case_env(case, tmp, **env_extra)
        r = run([sys.executable, str(GATE), stage, "--change", change, "--cwd", str(cwd), *extra], cwd=cwd, env=env)
    try:
        out = json.loads(r.stdout)
    except json.JSONDecodeError:
        out = None
    return r, out


def check(out: dict, cid: str) -> dict:
    return next(c for c in out["checks"] if c["id"] == cid)


class GateTest(unittest.TestCase):
    def assertGate(self, r, out, code: int, stage: str):
        self.assertEqual(r.returncode, code, r.stdout + r.stderr)
        self.assertIsInstance(out, dict, f"stdout 不是 JSON：{r.stdout!r}")
        self.assertEqual(set(out), KEYS)
        self.assertEqual(out["engine"], "openspec")
        self.assertEqual(out["stage"], stage)
        self.assertEqual(out["pass"], code == 0)

    # plan
    def test_plan_pass(self):
        r, out = gate("plan-pass", "plan")
        self.assertGate(r, out, 0, "plan")
        self.assertTrue(check(out, "proposal_done")["pass"])
        self.assertFalse(any(c["id"].startswith("validate") for c in out["checks"]), "plan 段不跑 validate")

    def test_plan_fail_no_proposal(self):
        r, out = gate("plan-fail", "plan")
        self.assertGate(r, out, 1, "plan")
        self.assertEqual({k: check(out, "proposal_done")[k] for k in ("id", "pass")}, {"id": "proposal_done", "pass": False})

    # design
    def test_design_pass(self):
        r, out = gate("design-pass", "design")
        self.assertGate(r, out, 0, "design")
        for cid in ("planning_complete", "validate_strict", "apply_ready"):
            self.assertTrue(check(out, cid)["pass"], cid)

    def test_design_fail_strict(self):
        # 沒有 SHALL：strict 下 WARNING 視為失敗；validate exit 1 但 stdout 有完整 JSON，要照 JSON 判定
        r, out = gate("design-fail", "design")
        self.assertGate(r, out, 1, "design")
        c = check(out, "validate_strict")
        self.assertFalse(c["pass"])
        issue = c["detail"]["issues"][0]
        self.assertEqual(issue["level"], "WARNING")
        self.assertEqual(issue["path"], "demo-cap/spec.md")
        self.assertIn("SHALL", issue["message"])

    # build
    def test_build_pass(self):
        r, out = gate("build-pass", "build", "--subtasks", str(INPUTS / "design-t123.json"))
        self.assertGate(r, out, 0, "build")
        for cid in ("tasks_all_done", "tasks_tagged", "subtasks_match"):
            self.assertTrue(check(out, cid)["pass"], cid)

    def test_build_fail_unchecked(self):
        r, out = gate("build-fail-unchecked", "build", "--subtasks", str(INPUTS / "design-t123.json"))
        self.assertGate(r, out, 1, "build")
        c = check(out, "tasks_all_done")
        self.assertFalse(c["pass"])
        self.assertEqual(c["detail"]["progress"], {"total": 5, "complete": 4, "remaining": 1})
        self.assertEqual(c["detail"]["unchecked"], [{"line": 9, "description": "2.2 [t3] 實作稽核紀錄"}])

    def test_build_fail_untagged(self):
        r, out = gate("build-fail-untagged", "build", "--subtasks", str(INPUTS / "design-t123.json"))
        self.assertGate(r, out, 1, "build")
        c = check(out, "tasks_tagged")
        self.assertFalse(c["pass"])
        self.assertEqual([t["description"] for t in c["detail"]["untagged"]], ["2.1 實作匯出"])

    def test_build_fail_subtask_set(self):
        r, out = gate("build-pass", "build", "--subtasks", str(INPUTS / "design-t12.json"))
        self.assertGate(r, out, 1, "build")
        c = check(out, "subtasks_match")
        self.assertFalse(c["pass"])
        self.assertEqual(c["detail"]["extra"], ["t3"])

    # test
    def test_test_pass(self):
        r, out = gate("build-pass", "test", "--criteria", str(INPUTS / "test-pass.json"), "--verify", str(INPUTS / "verify-clean.md"))
        self.assertGate(r, out, 0, "test")
        for cid in ("criteria_match", "criteria_pass", "anti_cheat_empty", "opsx_verify"):
            self.assertTrue(check(out, cid)["pass"], cid)

    def test_test_fail_missing(self):
        r, out = gate("build-pass", "test", "--criteria", str(INPUTS / "test-missing.json"), "--verify", str(INPUTS / "verify-clean.md"))
        self.assertGate(r, out, 1, "test")
        c = check(out, "criteria_match")
        self.assertFalse(c["pass"])
        self.assertEqual(c["detail"]["missing"], ["demo-cap/匯出紀錄/寫入紀錄"])

    def test_test_fail_extra(self):
        r, out = gate("build-pass", "test", "--criteria", str(INPUTS / "test-extra.json"), "--verify", str(INPUTS / "verify-clean.md"))
        self.assertGate(r, out, 1, "test")
        self.assertEqual(check(out, "criteria_match")["detail"]["extra"], ["misc/extra"])

    def test_test_fail_result_anti_cheat_critical(self):
        r, out = gate("build-pass", "test", "--criteria", str(INPUTS / "test-fail-result.json"), "--verify", str(INPUTS / "verify-critical.md"))
        self.assertGate(r, out, 1, "test")
        self.assertFalse(check(out, "criteria_pass")["pass"])
        self.assertFalse(check(out, "opsx_verify")["pass"])
        r, out = gate("build-pass", "test", "--criteria", str(INPUTS / "test-anti-cheat.json"), "--verify", str(INPUTS / "verify-skipped.md"))
        self.assertGate(r, out, 1, "test")
        self.assertFalse(check(out, "anti_cheat_empty")["pass"])
        self.assertFalse(check(out, "opsx_verify")["pass"], "有檢查被略過時不算通過")

    # CLI 故障
    def test_version_mismatch(self):
        r, out = gate("plan-pass", "plan", FAKE_OPENSPEC_VERSION="1.16.0")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertEqual(set(out), KEYS)
        self.assertEqual(out["checks"][0]["id"], "SPEC_ENGINE_VERSION")
        self.assertEqual(out["checks"][0]["detail"], "需要 1.15.x，實際 1.16.0")

    def test_cli_not_found(self):
        r, out = gate("plan-pass", "plan", AWF_OPENSPEC=json.dumps(["awf-no-such-openspec-binary"]))
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertEqual(out["checks"][0]["id"], "SPEC_ENGINE_UNAVAILABLE")

    def test_cli_timeout(self):
        if not __import__("os").environ.get("AWF_OPENSPEC_REAL") == "1":
            r, out = gate("plan-pass", "plan", FAKE_OPENSPEC_SLEEP="3", AWF_SPEC_GATE_TIMEOUT="1")
            self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
            self.assertEqual(out["checks"][0]["id"], "SPEC_ENGINE_TIMEOUT")

    def test_format_stdout_only_json(self):
        r, out = gate("design-pass", "design", "--at", "2026-11-01T10:00:00+08:00")
        self.assertEqual(out["at"], "2026-11-01T10:00:00+08:00")
        self.assertRegex(out["cli_version"], r"^1\.15\.")
        for c in out["checks"]:
            self.assertEqual(set(c), {"id", "pass", "detail"})


if __name__ == "__main__":
    unittest.main(verbosity=2)
