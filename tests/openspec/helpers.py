"""openspec 相關測試共用的小工具：決定用回放（預設）還是真 CLI 跑某個 case。

- 預設：環境變數 AWF_OPENSPEC 指到 fake_openspec.py，FAKE_OPENSPEC_CASE 指到 case 目錄，不需要安裝 OpenSpec。
- AWF_OPENSPEC_REAL=1：在暫存目錄 `openspec init` 後放入 case 的檔案，用 PATH 上的真 CLI 跑（驗證回放和實際一致）。
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
CASES = HERE / "cases"
INPUTS = HERE / "inputs"
FAKE = HERE / "fake_openspec.py"
REAL = os.environ.get("AWF_OPENSPEC_REAL") == "1"
BASE_ENV = {**os.environ, "PYTHONUTF8": "1", "NO_COLOR": "1", "OPENSPEC_NO_ANIMATION": "1", "OPENSPEC_TELEMETRY": "0"}
for k in ("AWF_OPENSPEC", "FAKE_OPENSPEC_CASE", "FAKE_OPENSPEC_VERSION", "FAKE_OPENSPEC_SLEEP", "AWF_EPIC"):
    BASE_ENV.pop(k, None)


def case_env(case: str, tmp: Path, **extra: str) -> tuple[Path, dict]:
    """回傳 (cwd, env)。真 CLI 模式會在 tmp 底下建一個 OpenSpec 專案。"""
    env = dict(BASE_ENV)
    if REAL and not extra.get("FAKE_OPENSPEC_VERSION"):
        exe = shutil.which("openspec")
        if not exe:
            raise RuntimeError("AWF_OPENSPEC_REAL=1 但找不到 openspec")
        src = CASES / case
        meta = json.loads((src / "case.json").read_text(encoding="utf-8")) if (src / "case.json").is_file() else {}
        cid = meta.get("change", "add-x")
        proj = tmp / "proj"
        proj.mkdir(parents=True, exist_ok=True)
        subprocess.run([exe, "init", "--tools", "none"], cwd=proj, env=env, capture_output=True, check=True)
        if meta.get("create", True):
            subprocess.run([exe, "new", "change", cid], cwd=proj, env=env, capture_output=True, check=True)
        if (src / "change").is_dir():
            shutil.copytree(src / "change", proj / "openspec" / "changes" / cid, dirs_exist_ok=True)
        if (src / "specs").is_dir():
            shutil.copytree(src / "specs", proj / "openspec" / "specs", dirs_exist_ok=True)
        env.update({k: v for k, v in extra.items()})
        return proj, env
    env["AWF_OPENSPEC"] = json.dumps([sys.executable, str(FAKE)])
    env["FAKE_OPENSPEC_CASE"] = str(CASES / case)
    env.update(extra)
    return tmp, env


def run(argv: list[str], cwd: Path, env: dict, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)


class TempDir:
    def __enter__(self) -> Path:
        self._d = tempfile.TemporaryDirectory()
        return Path(self._d.name).resolve()

    def __exit__(self, *exc) -> None:
        self._d.cleanup()
