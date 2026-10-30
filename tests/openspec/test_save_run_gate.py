#!/usr/bin/env python3
"""save_run.py approve 和 spec_gate 的接線測試。

- 非 openspec（default profile、沒有 methodology.json）：輸出、stderr、exit code 和改動前逐字相同，不產生 spec-gate-*.json。
- openspec profile：既有檢查之後才跑閘門；閘門不過就拒絕核准，過了就存 spec-gate-<stage>.json。
用法：python3 tests/openspec/test_save_run_gate.py   （AWF_OPENSPEC_REAL=1 改用真 CLI）
"""
from __future__ import annotations

import json
import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helpers import INPUTS, ROOT, TempDir, case_env, run  # noqa: E402

TEAMS = ROOT / "scaffold" / ".claude" / "teams"
AT = "2026-11-01T10:00:00+08:00"


def project(tmp: Path, profile: str | None) -> tuple[Path, Path]:
    """在 tmp/app 放一份 .claude/teams（tools、schemas），profile=None 表示沒有 methodology.json。回傳 (save_run.py, run 目錄)。"""
    teams = tmp / "app" / ".claude" / "teams"
    shutil.copytree(TEAMS / "tools", teams / "tools")
    shutil.copytree(TEAMS / "schemas", teams / "schemas")
    if profile:
        prof = json.loads((TEAMS / "methodologies" / f"{profile}.json").read_text(encoding="utf-8"))
        (teams / "methodology.json").write_text(json.dumps(prof, ensure_ascii=False), encoding="utf-8")
    rundir = tmp / "app" / "_runs" / "20261101-sdlc-demo"
    return teams / "tools" / "save_run.py", rundir


def set_stages(rundir: Path, **stages: dict) -> None:
    data = json.loads((rundir / "run.json").read_text(encoding="utf-8"))
    for k, v in stages.items():
        data["stages"][k].update(v)
    (rundir / "run.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def approved(rundir: Path, stage: str):
    return json.loads((rundir / "run.json").read_text(encoding="utf-8"))["stages"][stage].get("approved")


class NativeUnchanged(unittest.TestCase):
    """改動前就要通過：非 openspec 模式的行為不變。"""

    def _run(self, profile, stages, stage, *extra):
        with TempDir() as tmp:
            save_run, rd = project(tmp, profile)
            env = case_env("plan-pass", tmp)[1]
            run([sys.executable, str(save_run), "init", str(rd), "--event", "sdlc", "--target", "acme/app", "--requirement", "示範需求"], cwd=tmp, env=env)
            set_stages(rd, **stages)
            r = run([sys.executable, str(save_run), "approve", str(rd), stage, *extra], cwd=tmp, env=env)
            return r, sorted(p.name for p in rd.glob("spec-gate-*.json")), approved(rd, stage)

    def test_default_profile_approve_plan(self):
        for profile in ("default", "lightweight", None):
            with self.subTest(profile=profile):
                r, gates, ap = self._run(profile, {"plan": {"status": "done"}}, "plan", "--by", "kevin", "--at", AT)
                self.assertEqual((r.returncode, r.stdout, r.stderr), (0, f"plan 已記錄核准：kevin @ {AT}\n", ""))
                self.assertEqual(gates, [])
                self.assertEqual(ap, {"by": "kevin", "at": AT, "note": "", "kind": "human"})

    def test_default_profile_errors_unchanged(self):
        r, gates, _ = self._run("default", {"plan": {"status": "done"}, "design": {"status": "done"}}, "design", "--by", "kevin", "--at", AT)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (1, "", "不能核准：前一段 plan 還沒核准\n"))
        r, gates, _ = self._run("default", {}, "plan", "--by", "kevin")
        self.assertEqual((r.returncode, r.stderr), (1, "不能核准：plan 狀態是 pending，只有 done 的段能核准\n不能核准：--by 與 --at 都必填\n"))
        self.assertEqual(gates, [])


class OpenSpecGate(unittest.TestCase):
    """改動前會失敗：openspec profile 下 approve 要先過閘門。"""

    def _run(self, case, stages, stage, *extra, files: dict | None = None):
        with TempDir() as tmp:
            save_run, rd = project(tmp, "openspec")
            cwd, env = case_env(case, tmp)
            run([sys.executable, str(save_run), "init", str(rd), "--event", "sdlc", "--target", "acme/app", "--requirement", "示範需求"], cwd=tmp, env=env)
            set_stages(rd, **stages)
            for name, src in (files or {}).items():
                dst = cwd / name if name.startswith("openspec/") else rd / name
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy(src, dst)
            r = run([sys.executable, str(save_run), "approve", str(rd), stage, "--by", "kevin", "--at", AT, "--cwd", str(cwd), *extra], cwd=tmp, env=env)
            gate = {p.name: json.loads(p.read_text(encoding="utf-8")) for p in rd.glob("spec-gate-*.json")}
            return r, gate, approved(rd, stage)

    PLAN_OK = {"plan": {"status": "done", "approved": {"by": "kevin", "at": AT, "note": "", "kind": "human"}}}

    def test_design_gate_fails(self):
        r, gate, ap = self._run("design-fail", {**self.PLAN_OK, "design": {"status": "done"}}, "design", "--change", "add-x")
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stderr, "不能核准：spec_gate design 未過：validate_strict\n")
        self.assertIsNone(ap)
        self.assertFalse(gate["spec-gate-design.json"]["pass"])

    def test_design_gate_passes(self):
        r, gate, ap = self._run("design-pass", {**self.PLAN_OK, "design": {"status": "done"}}, "design", "--change", "add-x")
        self.assertEqual((r.returncode, r.stdout), (0, f"design 已記錄核准：kevin @ {AT}\n"), r.stderr)
        self.assertEqual(ap["by"], "kevin")
        self.assertTrue(gate["spec-gate-design.json"]["pass"])

    def test_change_required(self):
        r, gate, ap = self._run("plan-pass", {"plan": {"status": "done"}}, "plan")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stderr, "不能核准：openspec 模式要指定 change id（--change，或 run.json 的 epic）\n")
        self.assertIsNone(ap)
        self.assertEqual(gate, {})

    def test_existing_checks_first(self):
        # 既有檢查沒過時，訊息不變，也不跑閘門
        r, gate, _ = self._run("plan-pass", {}, "plan", "--change", "add-x")
        self.assertEqual((r.returncode, r.stderr), (1, "不能核准：plan 狀態是 pending，只有 done 的段能核准\n"))
        self.assertEqual(gate, {})

    def test_test_stage_also_checks_build(self):
        # build 段沒有人工閘門，所以核准 test 時一併跑 build 閘門（tasks 沒勾完不能過）
        stages = {**self.PLAN_OK, "design": {"status": "done", "approved": {"by": "kevin", "at": AT, "note": "", "kind": "human"}},
                  "build": {"status": "done"}, "test": {"status": "done"}}
        files = {"test.json": INPUTS / "test-pass.json", "design.json": INPUTS / "design-t123.json",
                 "openspec/changes/add-x/awf/opsx-verify.md": INPUTS / "verify-clean.md"}
        r, gate, ap = self._run("build-fail-unchecked", stages, "test", "--change", "add-x", files=files)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stderr, "不能核准：spec_gate build 未過：tasks_all_done\n")
        self.assertIsNone(ap)
        r, gate, ap = self._run("build-pass", stages, "test", "--change", "add-x", files=files)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(sorted(gate), ["spec-gate-build.json", "spec-gate-test.json"])
        self.assertTrue(all(g["pass"] for g in gate.values()))


if __name__ == "__main__":
    unittest.main(verbosity=2)
