#!/usr/bin/env python3
"""確定性 merge gate：機械複核合併 agent（dev-merge-steward）的需求對照表，並檢查其他合併條件。

agent 可能被 PR 內容說服；這支程式只看結構化輸入，不讀 PR 說明，說服不了。
hook 與目標專案 CI 都呼叫它；只有判定 merge 時 exit 0。只用標準庫，Windows 可跑。

不綁定任何開發規範：需求清單與驗證證據都先正規化，再比對。
  需求清單  [{"id", "text", "source"}]                       ← guard.config.json 的 requirements 指定來源（adapter）
  驗證證據  [{"requirement_id", "result", "evidence"}]        ← --evidence；result 是 pass／fail／not_verifiable

需求來源 adapter（guard.config.json）：
  "requirements": {
    "source": "json" | "command" | "spec-md",
    "path": "<json 或 markdown 路徑，相對專案根目錄；--requirements 可覆蓋>",
    "command": "<輸出需求清單 JSON 到 stdout 的指令；字串或陣列>",
    "id_pattern": "<spec-md 用：每行套用的正規式，第一個群組是需求 ID>"
  }
  - json：檔案內容是需求清單，或 {"requirements": [...]}。
  - command：團隊自己寫的腳本（例如從 issue、Story、PRD 轉出），stdout 輸出同上格式；
    執行時帶環境變數 AWF_REPO、AWF_PR、AWF_HEAD_SHA。
  - spec-md：內建的「範例」adapter，示範如何從 markdown 逐行抓 ID，不是規範；團隊的需求寫法不同就改用 json 或 command。
設定來源（和 sdlc 各段共用同一份需求設定）：
  1. guard.config.json 的 requirements 有設 path 或 command 時，以它為準（auto_merge 的設定走 PR＋CODEOWNERS，優先權最高）。
  2. 否則讀開發規範 profile：<專案>/.claude/teams/methodology.json 的 requirements（init.py 依使用者選的 methodology 產生，格式相同）。
  3. 都沒有時只能靠 --requirements；仍沒有就判 hold。

用法：
  python3 .claude/teams/tools/merge_gate.py check \\
      --decision <steward 信封或 payload 的 JSON 檔，- 代表 stdin> \\
      --repo acme-fork/app --pr 3 --head-sha <目前 PR head SHA> \\
      --evidence <正規化證據 JSON；也接受 verifier 的 criteria 信封> \\
      --checks <required checks JSON：gh pr checks --json name,state 的輸出，或 {"名稱": "success"}> \\
      --diff <git diff base...head 的輸出檔> \\
      --tests-before N --tests-after N \\
      [--findings <審查發現 JSON> ...] [--reviewed-sha SHA ...] [--base-sha SHA] [--git-dir <repo>] \\
      [--requirements <路徑>] [--requirements-source json|command|spec-md] \\
      [--timestamp <ISO 8601>] [--project-dir <專案根>] [--runs-dir <目錄>] [--no-audit]
  python3 .claude/teams/tools/merge_gate.py --selftest

輸出：stdout 一個 JSON（decision、reasons、escalations、followups、risk_tier…）。
結束碼：0＝merge；1＝hold；3＝escalate；2＝參數錯誤。輸入缺漏一律判 hold（fail-closed）。
每次判定 append 一行到 <runs_dir>/merge-audit.jsonl（runs_dir 讀 guard.config.json，和 hook 一致）。
放寬閘門（改測試／lint／型別／覆蓋率設定、新增壓制註解）一律 escalate，要人會簽。
auto_merge.carry_verdict_on_rebase（預設 false）開啟且有 --base-sha 時，對照／驗證／審查的 SHA 和目前 head 不同，
gate 會自己用 git 算兩邊的 patch-id，相同才沿用 verdict（寫進 reasons 與 carried），否則照舊 hold。
凍結旗標 auto_merge.freeze_file（預設 <runs_dir>/auto-merge.freeze.json，內容 {"frozen": true, …}）存在且 frozen 不是 false 時一律 hold，和 hook 讀同一個檔。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402

GATE_VERSION = "1"
TIERS = ("low", "medium", "high")
DEFAULT_DAILY_CAP = 5
DEFAULT_SMALL_LINES = 100   # low 等級的改動行數上限
DEFAULT_LARGE_LINES = 400   # 超過就是 high
DEFAULT_LARGE_FILES = 20
EXIT = {"merge": 0, "hold": 1, "escalate": 3}
PASS_CHECK = {"success", "neutral", "skipped", "pass"}

# 動到就交給人（escalate）的路徑：CI、.github、hooks、gate、release、版本檔、依賴與鎖檔、migration、auth、CODEOWNERS
SENSITIVE = [
    (r"(^|/)\.github/", "GitHub 設定或 CI"),
    (r"(^|/)(\.gitlab-ci\.yml|Jenkinsfile|azure-pipelines\.yml|bitbucket-pipelines\.yml)$|(^|/)\.circleci/|(^|/)\.buildkite/", "CI 設定"),
    (r"(^|/)\.claude/|(^|/)\.claude-plugin/", "agent 設定、守門或 gate"),
    (r"(^|/)(\.husky|\.githooks)/|^(plugin/)?hooks/|(^|/)\.pre-commit-config\.yaml$", "hooks"),
    (r"merge_gate|(^|/)guard\.config\.json", "merge gate"),
    (r"(^|/)release[^/]*(/|$)|(^|/)\.release|(^|/)(\.goreleaser\.ya?ml|\.releaserc[^/]*)$", "release"),
    (r"(^|/)(VERSION|version\.txt|_version\.py|version\.py)$", "版本檔"),
    (r"(^|/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml|npm-shrinkwrap\.json|bun\.lockb?|poetry\.lock|uv\.lock|Pipfile\.lock|Cargo\.lock|go\.sum|Gemfile\.lock|composer\.lock|packages\.lock\.json|[^/]*\.lock)$", "依賴鎖檔"),
    (r"(^|/)(package\.json|pyproject\.toml|setup\.py|setup\.cfg|requirements[^/]*\.txt|Pipfile|go\.mod|Cargo\.toml|Gemfile|composer\.json|pom\.xml|build\.gradle(\.kts)?)$", "依賴或版本宣告"),
    (r"(^|/)migrations?/", "資料庫 migration"),
    (r"(^|/)CODEOWNERS$", "CODEOWNERS"),
    (r"(^|/)(auth|oauth|authn|authz)(/|\.[a-z]+$)|(^|/)[^/]*[_-](auth|oauth)\.[a-z]+$", "auth"),
    # 放寬閘門：改這些設定可能放寬測試、lint、型別或覆蓋率門檻，要人會簽
    (r"(^|/)(jest|vitest|playwright|cypress)\.config\.[a-z]+$|(^|/)karma\.conf\.[a-z]+$|(^|/)(pytest\.ini|tox\.ini|\.coveragerc|codecov\.ya?ml|mypy\.ini|\.flake8|\.?ruff\.toml|\.rubocop\.yml)$"
     r"|(^|/)\.nycrc[^/]*$|(^|/)\.eslintrc[^/]*$|(^|/)eslint\.config\.[a-z]+$|(^|/)tsconfig[^/]*\.json$|(^|/)\.golangci\.ya?ml$|(^|/)phpunit\.xml[^/]*$|(^|/)\.stylelintrc[^/]*$",
     "測試／lint／型別／覆蓋率設定"),
]
DOC = re.compile(r"(^|/)docs?/|\.(md|mdx|rst|txt|adoc)$", re.I)
TEST = re.compile(r"(^|/)(tests?|__tests__|spec|e2e)/|(^|/)test_[^/]+$|_test\.[a-z]+$|\.(test|spec)\.[a-z]+$", re.I)
# 新增的 skip／only／fixme（含常見框架寫法）
SKIP = re.compile(
    r"\b(it|test|describe|context|suite)\.(skip|only|fixme|todo)\b|\b(xit|xdescribe|fit|fdescribe|xtest)\s*\("
    r"|@pytest\.mark\.(skip|skipif|xfail)\b|\bpytest\.(skip|xfail)\s*\(|@unittest\.(skip|skipIf|skipUnless|expectedFailure)\b"
    r"|\bt\.Skip(Now|f)?\s*\(|@(Disabled|Ignore)\b|\btest\.fixme\b"
)
# 新增的壓制註解（lint、型別、覆蓋率）：放寬檢查要人會簽
SUPPRESS = re.compile(
    r"eslint-disable|@ts-(ignore|nocheck|expect-error)\b|#\s*type:\s*ignore|#\s*noqa\b|pragma:\s*no\s*cover"
    r"|(istanbul|c8)\s+ignore|//\s*nolint\b|\bNOLINT\b|@SuppressWarnings\b|rubocop:disable"
)
# 內建範例 adapter（spec-md）的預設 ID 樣式：行首（可帶清單符號、表格線、粗體）的 AC-1／AC1／REQ-1 之類
DEFAULT_ID_PATTERN = r"^\s*(?:[-*+]\s+|\d+[.)]\s+|\|\s*|#+\s+)?(?:\*\*|`)?\[?([A-Z][A-Z0-9]*-?\d+)\]?(?:\*\*|`)?(?=[\s:：|.)、]|$)"


class InputError(Exception):
    pass


# ------------------------------------------------------------------ 設定與路徑
def project_dir(arg: str | None = None) -> Path:
    if arg:
        return Path(arg).resolve()
    if os.environ.get("CLAUDE_PROJECT_DIR"):
        return Path(os.environ["CLAUDE_PROJECT_DIR"]).resolve()
    return schemalib.TEAMS_DIR.parent.parent


def load_config(proj: Path) -> dict:
    p = proj / ".claude" / "teams" / "guard.config.json"
    return schemalib.config(p if p.exists() else None)


def runs_dir(proj: Path, cfg: dict, override: str | None = None) -> Path:
    """和 hook（awf_config）同一個來源：guard.config.json 的 runs_dir，預設 _tpm/runs，相對專案根。"""
    d = (override or cfg.get("runs_dir") or "_tpm/runs").strip()
    p = Path(d)
    return p if p.is_absolute() else proj / d.strip("/")


def freeze_file(proj: Path, cfg: dict) -> Path:
    """凍結旗標：auto_merge.freeze_file，預設 <runs_dir>/auto-merge.freeze.json（和 hook 的 awf_config.auto_merge 同一個預設）。
    event-post-merge 寫 {"frozen": true, "reason": …}；gate 判 hold，hook 也不放行 steward 的合併。解凍（刪檔或改 false）由人做。"""
    am = cfg.get("auto_merge") if isinstance(cfg.get("auto_merge"), dict) else {}
    raw = str(am.get("freeze_file") or "").strip()
    if not raw or re.search(r"\{\{[A-Z_]+\}\}", raw):
        return runs_dir(proj, cfg) / "auto-merge.freeze.json"
    p = Path(raw)
    return p if p.is_absolute() else proj / raw.strip("/")


def _list(v) -> list[str]:
    if isinstance(v, str):
        return [x.strip() for x in v.split(",") if x.strip()]
    return [x for x in (v or []) if isinstance(x, str) and x.strip()]


# ------------------------------------------------------------------ 需求來源 adapter
def _normalize_requirements(data, source: str) -> list[dict]:
    items = data.get("requirements") if isinstance(data, dict) else data
    if not isinstance(items, list):
        raise InputError(f"需求清單格式不對（{source}）：要是陣列，或帶 requirements 陣列的物件")
    out, seen = [], set()
    for i, r in enumerate(items):
        if not isinstance(r, dict) or not str(r.get("id") or "").strip():
            raise InputError(f"需求清單第 {i} 項缺 id（{source}）")
        rid = str(r["id"]).strip()
        if rid in seen:
            raise InputError(f"需求清單有重複的 id {rid}（{source}）")
        seen.add(rid)
        out.append({"id": rid, "text": str(r.get("text") or ""), "source": str(r.get("source") or source)})
    return out


def adapter_json(path: Path, **_) -> list[dict]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise InputError(f"讀不到需求清單 {path}：{e}") from e
    return _normalize_requirements(data, f"json:{path.name}")


def adapter_command(command, proj: Path, env_extra: dict, **_) -> list[dict]:
    if not command:
        raise InputError("requirements.source=command 但沒有設定 command")
    shell = isinstance(command, str)
    try:
        r = subprocess.run(command, shell=shell, cwd=proj, capture_output=True, text=True, encoding="utf-8",
                           errors="replace", timeout=120, env={**os.environ, "PYTHONUTF8": "1", **env_extra})
    except (OSError, subprocess.TimeoutExpired) as e:
        raise InputError(f"需求清單指令執行失敗：{e}") from e
    if r.returncode:
        raise InputError(f"需求清單指令 exit {r.returncode}：{r.stderr.strip()[-300:]}")
    try:
        data = json.loads(r.stdout)
    except json.JSONDecodeError as e:
        raise InputError(f"需求清單指令的輸出不是 JSON：{e}") from e
    return _normalize_requirements(data, "command")


def adapter_spec_md(path: Path, id_pattern: str | None = None, **_) -> list[dict]:
    """範例 adapter：逐行套 id_pattern，第一個群組當 ID、該行其餘文字當需求原文；同一 ID 只取第一次出現。
    只是示範怎麼把 markdown 轉成正規化清單，不代表需求必須這樣寫。"""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        raise InputError(f"讀不到需求文件 {path}：{e}") from e
    pat = re.compile(id_pattern or DEFAULT_ID_PATTERN)
    out, seen = [], set()
    for n, line in enumerate(text.splitlines(), 1):
        m = pat.search(line)
        if not m or m.group(1) in seen:
            continue
        seen.add(m.group(1))
        rest = line[m.end():].strip(" \t:：|*`-、.)")
        out.append({"id": m.group(1), "text": rest, "source": f"{path.name}:{n}"})
    if not out:
        raise InputError(f"{path} 裡找不到符合 id_pattern 的需求（spec-md 是範例 adapter，需求寫法不同請改用 json 或 command）")
    return out


ADAPTERS = {"json": adapter_json, "command": adapter_command, "spec-md": adapter_spec_md}


def _configured(rc) -> bool:
    """需求設定「有設」＝path 或 command 不是空的（還沒替換的佔位符當成空）。"""
    if not isinstance(rc, dict):
        return False
    return any(isinstance(rc.get(k), (str, list)) and rc.get(k) and not re.search(r"\{\{[A-Z_]+\}\}", str(rc.get(k))) for k in ("path", "command"))


def methodology_requirements(proj: Path) -> dict:
    """開發規範 profile（.claude/teams/methodology.json）的 requirements；沒有檔案或格式錯回空 dict。"""
    try:
        data = json.loads((proj / ".claude" / "teams" / "methodology.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    rc = data.get("requirements") if isinstance(data, dict) else None
    return rc if isinstance(rc, dict) else {}


def requirements_config(cfg: dict, proj: Path) -> tuple[dict, str]:
    """回傳 (需求設定, 出處)：guard.config.json 有設 path／command 時以它為準，否則用 methodology profile 的。"""
    rc = cfg.get("requirements") if isinstance(cfg.get("requirements"), dict) else {}
    if _configured(rc):
        return rc, "guard.config.json"
    mc = methodology_requirements(proj)
    if mc:
        return mc, "methodology.json"
    return rc, "guard.config.json"


def load_requirements(cfg: dict, proj: Path, path_override: str | None, source_override: str | None, env_extra: dict) -> tuple[list[dict], str]:
    rc, _origin = requirements_config(cfg, proj)
    source = source_override or rc.get("source") or ("json" if (path_override or "").endswith(".json") else "spec-md")
    if source not in ADAPTERS:
        raise InputError(f"不認得的需求來源 {source}（可用：{'、'.join(ADAPTERS)}）")
    raw = path_override or rc.get("path") or ""
    path = (Path(raw) if Path(raw).is_absolute() else proj / raw) if raw else None
    if source != "command" and not path:
        raise InputError("沒有需求來源：請在 guard.config.json 或 .claude/teams/methodology.json 設 requirements.path，或帶 --requirements")
    reqs = ADAPTERS[source](path=path, command=rc.get("command"), id_pattern=rc.get("id_pattern"), proj=proj, env_extra=env_extra)
    label = f"{source}:{raw}" if raw else source
    return reqs, (label + "（methodology.json）" if _origin == "methodology.json" and not path_override else label)


# ------------------------------------------------------------------ 其他輸入的正規化
def normalize_evidence(data) -> tuple[list[dict], str | None, list]:
    """回傳 (證據清單, 驗證時的 commit, anti_cheat)。接受：正規化清單；{"commit","anti_cheat","results"}；
    或 criteria payload／信封（例如 dev-verifier 的產出，id 視為 requirement_id）。"""
    if isinstance(data, dict) and isinstance(data.get("payload"), dict):
        data = data["payload"]
    commit, anti, items = None, [], data
    if isinstance(data, dict):
        commit = data.get("commit")
        anti = data.get("anti_cheat") or []
        items = data.get("results", data.get("criteria"))
    if not isinstance(items, list):
        raise InputError("證據格式不對：要是 [{requirement_id, result, evidence}] 或帶 results／criteria 的物件")
    out = []
    for i, e in enumerate(items):
        if not isinstance(e, dict):
            raise InputError(f"證據第 {i} 項不是物件")
        rid = e.get("requirement_id", e.get("id"))
        if not rid:
            raise InputError(f"證據第 {i} 項缺 requirement_id")
        out.append({"requirement_id": str(rid), "result": str(e.get("result") or ""), "evidence": str(e.get("evidence") or "")})
    return out, (commit if commit and commit != "n/a" else None), anti


def normalize_checks(data) -> list[tuple[str, str]]:
    if isinstance(data, dict):
        return [(str(k), str(v)) for k, v in data.items()]
    if not isinstance(data, list):
        raise InputError("required checks 格式不對")
    out = []
    for c in data:
        if not isinstance(c, dict):
            raise InputError("required checks 每項要是物件")
        state = c.get("conclusion") or c.get("state") or c.get("bucket") or c.get("status") or ""
        out.append((str(c.get("name") or c.get("context") or "?"), str(state)))
    return out


def normalize_findings(data) -> list[dict]:
    if isinstance(data, dict) and isinstance(data.get("payload"), dict):
        data = data["payload"]
    if isinstance(data, dict):
        data = data.get("findings", [])
    return [f for f in data if isinstance(f, dict)] if isinstance(data, list) else []


def parse_diff(text: str) -> dict:
    """git diff 輸出 → {files: {path: [added, deleted]}, added_lines: [(path, line)]}。"""
    files: dict[str, list[int]] = {}
    added: list[tuple[str, str]] = []
    cur, old = None, None
    for line in text.splitlines():
        if line.startswith("diff --git "):
            m = re.match(r"diff --git a/(.+?) b/(.+)$", line)
            cur = m.group(2) if m else None
            old = m.group(1) if m else None
            if cur:
                files.setdefault(cur, [0, 0])
        elif line.startswith("--- "):
            p = line[4:].strip()
            old = p[2:] if p.startswith("a/") else old
        elif line.startswith("+++ "):
            p = line[4:].strip()
            new = p[2:] if p.startswith("b/") else None
            if new is None and old:  # 刪除檔案
                new = old
            if new and new != cur:
                if cur in files and files[cur] == [0, 0]:
                    files.pop(cur)
                cur = new
                files.setdefault(cur, [0, 0])
        elif cur and line.startswith("+"):
            files[cur][0] += 1
            added.append((cur, line[1:]))
        elif cur and line.startswith("-"):
            files[cur][1] += 1
    return {"files": files, "added_lines": added}


def risk_tier(diff: dict, am: dict) -> tuple[str, list[str]]:
    files = diff["files"]
    lines = sum(a + d for a, d in files.values())
    hits = []
    for path in files:
        for pat, what in SENSITIVE:
            if re.search(pat, path):
                hits.append(f"{path}（{what}）")
                break
    large = int(am.get("large_lines") or DEFAULT_LARGE_LINES)
    small = int(am.get("small_lines") or DEFAULT_SMALL_LINES)
    if hits:
        return "high", [f"改到高風險路徑：{'、'.join(hits)}"]
    if lines > large or len(files) > int(am.get("large_files") or DEFAULT_LARGE_FILES):
        return "high", [f"diff 過大：{len(files)} 個檔、{lines} 行"]
    if files and lines <= small and (all(DOC.search(p) for p in files) or all(TEST.search(p) for p in files)):
        return "low", []
    return "medium", []


# ------------------------------------------------------------------ 判定
def evaluate(inp: dict, cfg: dict, audit_lines: list[dict]) -> dict:
    """純函式：inp 是已載入的輸入，回傳判定結果（不寫檔）。hold 理由優先於 escalate。"""
    hold: list[str] = []
    esc: list[str] = []
    followups: list[dict] = []
    repo, pr, head = inp.get("repo") or "", inp.get("pr"), (inp.get("head_sha") or "").strip()
    am = cfg.get("auto_merge") if isinstance(cfg.get("auto_merge"), dict) else {}

    def need(key: str, what: str) -> bool:
        if inp.get(key) is None:
            hold.append(f"缺輸入：{what}（fail-closed）")
            return False
        return True

    if not head:
        hold.append("缺輸入：目前 head SHA")

    # rebase 後沿用 verdict（auto_merge.carry_verdict_on_rebase，預設關閉）：patch-id 由 gather() 用 git 自己算，相同才沿用
    carry = bool(am.get("carry_verdict_on_rebase")) and bool(inp.get("base_sha"))
    pids = inp.get("patch_ids") or {}
    carried: list[str] = []

    def sha_mismatch(old: str, when: str) -> None:
        if carry:
            a, b = pids.get(old), pids.get(head)
            if a and b and a == b:
                carried.append(f"沿用 verdict：{when} {old[:12]} → 目前 {head[:12]}（patch-id 相同）")
                return
            why = "patch-id 不同" if a and b else "無法計算 patch-id"
            hold.append(f"head SHA 不符：{when} {old}，目前 {head}（{why}，不沿用）")
        else:
            hold.append(f"head SHA 不符：{when} {old}，目前 {head}")

    # 1. steward 的信封／payload：schema＋語意規則
    payload: dict = {}
    if need("decision", "steward 的 merge-decision"):
        d = inp["decision"]
        if isinstance(d, dict) and isinstance(d.get("payload"), dict):
            errs = schemalib.check(d, "merge-decision")
            payload = d["payload"]
        else:
            errs = schemalib.validate(d, schemalib.compose("merge-decision")["properties"]["payload"]) + schemalib.merge_decision_errors(d)
            payload = d if isinstance(d, dict) else {}
        if errs:
            hold.append(f"merge-decision 不合格：{errs[:5]}")
        if payload.get("repo") != repo or payload.get("pr") != pr:
            hold.append(f"merge-decision 的 repo／pr（{payload.get('repo')}#{payload.get('pr')}）與這次（{repo}#{pr}）不同")
        if head and payload.get("head_sha") != head:
            sha_mismatch(str(payload.get("head_sha")), "對照時")
        sd = payload.get("decision")
        if sd == "hold":
            hold.append("steward 判 hold（gate 只會更嚴，不會放寬）")
        elif sd == "escalate":
            esc.append("steward 判 escalate（gate 只會更嚴，不會放寬）")
        followups.extend(f for f in payload.get("followups") or [] if isinstance(f, dict))

    # 2. 需求對照（traceability）機械複核
    ev_by: dict[str, list[dict]] = {}
    ev_ok = need("evidence", "驗證證據")
    if need("requirements", "需求清單"):
        reqs = inp["requirements"]
        for e in inp.get("evidence") or []:
            ev_by.setdefault(e["requirement_id"], []).append(e)
        rows = {r.get("requirement_id"): r for r in payload.get("traceability") or [] if isinstance(r, dict)}
        req_ids = {r["id"] for r in reqs}
        have_fu = {f.get("requirement_id") for f in followups}

        def fu(rid: str, expected: str, actual: str, adj: str) -> None:
            if rid not in have_fu:
                followups.append({"requirement_id": rid, "expected": expected, "actual": actual, "adjustment": adj})
                have_fu.add(rid)

        for r in reqs:
            rid, row = r["id"], rows.get(r["id"])
            evs = ev_by.get(rid, [])
            if row is None:
                hold.append(f"traceability：需求 {rid} 沒有出現在對照表（missing）")
                fu(rid, r["text"], "對照表漏列這條需求", "補上這條需求的對照與驗證證據")
                continue
            if row.get("result") != "match":
                hold.append(f"traceability：需求 {rid} 判 {row.get('result')}")
                fu(rid, r["text"], f"對照結果 {row.get('result')}", "依需求調整實作或補證據")
                continue
            if not ev_ok:
                continue
            if not evs:
                hold.append(f"traceability：需求 {rid} 宣稱 match，但驗證證據裡沒有這條（missing）")
                fu(rid, r["text"], "沒有驗證證據", "補驗證並附證據")
            elif any(e["result"] != "pass" for e in evs):
                bad = [e["result"] for e in evs if e["result"] != "pass"]
                hold.append(f"traceability：需求 {rid} 宣稱 match，但驗證結果是 {bad}（mismatch）")
                fu(rid, r["text"], f"驗證結果 {bad}：{evs[0]['evidence'][:200]}", "修正實作直到驗證 pass")
        extra = [k for k in rows if k not in req_ids]
        if extra:
            hold.append(f"traceability：對照表有需求清單裡沒有的 ID {extra}")
        oos = payload.get("out_of_scope") or []
        if oos:
            hold.append(f"traceability：有 {len(oos)} 項對不到需求的改動（out_of_scope）")
            for o in oos:
                if isinstance(o, dict):
                    followups.append({"requirement_id": "out_of_scope", "expected": "改動都要對到需求",
                                      "actual": f"{o.get('location')}：{o.get('change')}", "adjustment": "移出這個 PR，或先補需求再改"})
        if not reqs:
            hold.append("traceability：需求清單是空的")

    # 3. 驗證證據：全部 pass、anti_cheat 為空、驗證時的 commit 等於 head
    if inp.get("evidence") is not None:
        notpass = sorted({e["requirement_id"] for e in inp["evidence"] if e["result"] != "pass"})
        if notpass:
            hold.append(f"驗證未全部 pass：{notpass}")
        if not inp["evidence"]:
            hold.append("驗證證據是空的")
        if inp.get("anti_cheat"):
            hold.append(f"反作弊檢查有發現：{inp['anti_cheat'][:5]}")
        if inp.get("evidence_commit") and head and inp["evidence_commit"] != head:
            sha_mismatch(inp["evidence_commit"], "驗證時")
    for s in inp.get("reviewed_shas") or []:
        if head and s != head:
            sha_mismatch(s, "審查時")

    # 4. required checks
    if need("checks", "required checks 狀態"):
        if not inp["checks"]:
            hold.append("沒有任何 required check")
        failed = [f"{n}={s}" for n, s in inp["checks"] if s.lower() not in PASS_CHECK]
        if failed:
            hold.append(f"required checks 未通過：{failed}")

    # 5. 留存的 important／blocker 發現（不確定就停：skeptic 推翻也不算數，要真的修掉、重審後不再出現）
    sev = [f"{f.get('id')}({f.get('severity')})" for f in inp.get("findings") or [] if f.get("severity") in ("important", "blocker")]
    if sev:
        hold.append(f"有留存的 important／blocker 發現：{sev}")

    # 6. 測試數不減少
    if need("tests_before", "合併前測試數") and need("tests_after", "合併後測試數"):
        if inp["tests_after"] < inp["tests_before"]:
            hold.append(f"測試數減少：{inp['tests_before']} → {inp['tests_after']}")

    # 7. diff：新增 skip／only／fixme、風險等級
    tier = "high"
    if need("diff", "diff"):
        diff = inp["diff"]
        skips = [f"{p}: {l.strip()[:80]}" for p, l in diff["added_lines"] if SKIP.search(l)]
        if skips:
            hold.append(f"新增 skip／only／fixme：{skips[:5]}")
        sup = [f"{p}: {l.strip()[:80]}" for p, l in diff["added_lines"] if SUPPRESS.search(l)]
        if sup:
            esc.append(f"新增壓制註解（放寬 lint／型別／覆蓋率檢查，要人會簽）：{sup[:5]}")
        if not diff["files"]:
            hold.append("diff 是空的")
        tier, why = risk_tier(diff, am)
        esc.extend(why)

    # 8. 白名單、風險上限、上游授權、每日上限
    repos = am.get("repos") if isinstance(am.get("repos"), dict) else {}
    if not am.get("enabled"):
        esc.append("auto_merge 未啟用（guard.config.json 的 auto_merge.enabled）")
    if repo not in repos:
        esc.append(f"repo {repo} 不在自動合併白名單（auto_merge.repos）")
    else:
        cap_tier = repos.get(repo) if repos.get(repo) in TIERS else "low"
        if TIERS.index(tier) > TIERS.index(cap_tier):
            esc.append(f"風險等級 {tier} 超過 {repo} 的上限 {cap_tier}")
    if tier == "high" and not any("高風險" in e or "過大" in e for e in esc):
        esc.append("風險等級 high 一律交給人")
    if repo in _list(cfg.get("upstream_repos")):
        auth = am.get("upstream_authorized_by") if isinstance(am.get("upstream_authorized_by"), dict) else {}
        if not str(auth.get(repo) or "").strip():
            esc.append(f"{repo} 是上游 repo，沒有 upstream_authorized_by 授權人")
    if inp.get("frozen"):
        hold.append(f"自動合併已凍結（{inp['frozen']} 存在；解凍由人處理）")
    cap = am.get("daily_cap")
    cap = DEFAULT_DAILY_CAP if cap in (None, "") else int(cap)
    day = (inp.get("timestamp") or "")[:10]
    merged_today = sum(1 for a in audit_lines if a.get("decision") == "merge" and str(a.get("timestamp", ""))[:10] == day)
    if merged_today >= cap:
        hold.append(f"超過每日合併上限：今天（{day}）已合併 {merged_today} 次，上限 {cap}")

    decision = "hold" if hold else ("escalate" if esc else "merge")
    reasons = hold + esc if decision != "merge" else ["全部檢查通過"] + carried
    return {"decision": decision, "repo": repo, "pr": pr, "head_sha": head, "reasons": reasons,
            "hold_reasons": hold, "escalations": esc, "followups": followups, "carried": carried,
            "risk_tier": tier, "requirements_source": inp.get("requirements_source", ""),
            "timestamp": inp.get("timestamp"), "gate_version": GATE_VERSION}


# ------------------------------------------------------------------ 稽核紀錄
def read_audit(path: Path) -> list[dict]:
    out = []
    try:
        for line in path.read_text(encoding="utf-8").splitlines():
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(rec, dict):
                out.append(rec)
    except OSError:
        pass
    return out


def append_audit(path: Path, result: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    rec = {k: result[k] for k in ("timestamp", "repo", "pr", "head_sha", "decision", "reasons", "risk_tier",
                                  "followups", "carried", "requirements_source", "gate_version") if k in result}
    with path.open("a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")


# ------------------------------------------------------------------ CLI
def _read_json(arg: str):
    try:
        text = sys.stdin.read() if arg == "-" else Path(arg).read_text(encoding="utf-8")
        return json.loads(text) if text.strip().startswith(("{", "[")) else schemalib.extract_json(text)
    except (OSError, json.JSONDecodeError) as e:
        raise InputError(f"讀不到 {arg}：{e}") from e


def patch_id(git_dir: Path, base: str, sha: str) -> str | None:
    """git diff $(merge-base base sha) sha | git patch-id --stable；任何失敗回 None（呼叫端 fail-closed）。"""
    try:
        def git(*args: str, data: bytes | None = None) -> bytes:
            return subprocess.run(["git", "-C", str(git_dir), *args], input=data, capture_output=True, check=True).stdout
        mb = git("merge-base", base, sha).decode().strip()
        diff = git("diff", mb, sha)
        out = git("patch-id", "--stable", data=diff).decode().split() if diff.strip() else []
        return out[0] if out else None
    except (OSError, subprocess.CalledProcessError, UnicodeDecodeError):
        return None


def gather(a: argparse.Namespace, cfg: dict, proj: Path) -> dict:
    inp: dict = {"repo": a.repo, "pr": a.pr, "head_sha": a.head_sha, "reviewed_shas": a.reviewed_sha or [], "base_sha": a.base_sha,
                 "tests_before": a.tests_before, "tests_after": a.tests_after,
                 "timestamp": a.timestamp or datetime.now(timezone.utc).isoformat(timespec="seconds")}
    errors = []

    def attempt(fn):
        try:
            fn()
        except InputError as e:
            errors.append(str(e))

    def dec():
        if a.decision:
            inp["decision"] = _read_json(a.decision)

    def reqs():
        env = {"AWF_REPO": a.repo, "AWF_PR": str(a.pr), "AWF_HEAD_SHA": a.head_sha}
        inp["requirements"], inp["requirements_source"] = load_requirements(cfg, proj, a.requirements, a.requirements_source, env)

    def ev():
        if a.evidence:
            inp["evidence"], inp["evidence_commit"], inp["anti_cheat"] = normalize_evidence(_read_json(a.evidence))

    def chk():
        if a.checks:
            inp["checks"] = normalize_checks(_read_json(a.checks))

    def fnd():
        inp["findings"] = [f for p in a.findings or [] for f in normalize_findings(_read_json(p))]

    def dif():
        if a.diff:
            try:
                inp["diff"] = parse_diff(Path(a.diff).read_text(encoding="utf-8", errors="replace"))
            except OSError as e:
                raise InputError(f"讀不到 diff {a.diff}：{e}") from e

    for fn in (dec, reqs, ev, chk, fnd, dif):
        attempt(fn)
    am = cfg.get("auto_merge") if isinstance(cfg.get("auto_merge"), dict) else {}
    if am.get("carry_verdict_on_rebase") and a.base_sha:
        d = inp.get("decision")
        p = d.get("payload") if isinstance(d, dict) and isinstance(d.get("payload"), dict) else d
        shas = {a.head_sha, *(a.reviewed_sha or []), inp.get("evidence_commit"), p.get("head_sha") if isinstance(p, dict) else None}
        git_dir = Path(a.git_dir).resolve() if a.git_dir else proj
        inp["patch_ids"] = {s: patch_id(git_dir, a.base_sha, s) for s in shas if isinstance(s, str) and s}
    return inp, errors


def cmd_check(a: argparse.Namespace) -> int:
    proj = project_dir(a.project_dir)
    cfg = load_config(proj)
    audit = runs_dir(proj, cfg, a.runs_dir) / "merge-audit.jsonl"
    inp, errors = gather(a, cfg, proj)
    ff = freeze_file(proj, cfg)
    if ff.exists():
        try:
            fz = json.loads(ff.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            fz = {}  # 讀不懂也算凍結（fail-closed）
        if not (isinstance(fz, dict) and fz.get("frozen") is False):
            inp["frozen"] = f"{ff}：{fz.get('reason', '') if isinstance(fz, dict) else ''}"
    result = evaluate(inp, cfg, read_audit(audit))
    if errors:  # 輸入讀不到：一律 hold
        result["decision"] = "hold"
        result["reasons"] = [f"輸入錯誤：{e}" for e in errors] + [r for r in result["reasons"] if r != "全部檢查通過"]
    if not a.no_audit:
        append_audit(audit, result)
        result["audit"] = str(audit)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return EXIT[result["decision"]]


# ------------------------------------------------------------------ selftest
SHA = "a" * 40


def _selftest() -> int:
    ok = True
    with tempfile.TemporaryDirectory() as d:
        proj = Path(d)
        (proj / ".claude/teams").mkdir(parents=True)
        (proj / "specs/demo").mkdir(parents=True)
        reqs = [{"id": "REQ-1", "text": "匯出按鈕下載 CSV", "source": "示範客戶 ACME Story"},
                {"id": "REQ-2", "text": "超過一萬筆時提示縮小範圍", "source": "示範客戶 ACME Story"}]
        (proj / "reqs.json").write_text(json.dumps(reqs, ensure_ascii=False), encoding="utf-8")
        (proj / "specs/demo/spec.md").write_text(
            "# 示範需求\n\n## 驗收條件\n\n- **AC-1**：匯出按鈕下載 CSV\n- **AC-2**：超過一萬筆時提示縮小範圍\n\n參考 AC-1 的說明（不是新條目）\n",
            encoding="utf-8")
        gen = proj / "gen_reqs.py"
        gen.write_text("import json,os\nprint(json.dumps({'requirements': [{'id': 'ISSUE-7', 'text': '登入頁顯示錯誤訊息', 'source': 'issue #7 of ' + os.environ['AWF_REPO']}]}))\n", encoding="utf-8")
        base_cfg = {"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app",
                    "auto_merge": {"enabled": True, "repos": {"acme-fork/app": "medium", "acme-up/app": "low"}, "daily_cap": 2},
                    "requirements": {"source": "json", "path": "reqs.json"}}

        def write_cfg(cfg):
            (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg, ensure_ascii=False), encoding="utf-8")

        def row(rid, text, result="match"):
            return {"requirement_id": rid, "requirement": text, "evidence": [f"results.json {rid}"], "result": result}

        def decision(rows, dec="merge", followups=None, oos=None, sha=SHA, repo="acme-fork/app", pr=3):
            return {"status": "done", "summary": "示範", "artifacts": [], "evidence": [], "confidence": 0.9,
                    "open_questions": [], "derived_work": [],
                    "payload": {"decision": dec, "repo": repo, "pr": pr, "head_sha": sha, "requirements_source": "json",
                                "traceability": rows, "out_of_scope": oos or [], "followups": followups or [],
                                "risk_tier": "low", "gate_reasons": []}}

        good_rows = [row(r["id"], r["text"]) for r in reqs]
        good_ev = [{"requirement_id": r["id"], "result": "pass", "evidence": "1 passed"} for r in reqs]
        test_diff = "diff --git a/tests/test_export.py b/tests/test_export.py\n--- a/tests/test_export.py\n+++ b/tests/test_export.py\n@@ -1 +1,2 @@\n+def test_csv():\n+    assert export() == 'csv'\n"

        def run(name, want, cfg=None, files=None, args=None, expect_followup=None, audit=False):
            nonlocal ok
            write_cfg(cfg or base_cfg)
            files = {"decision.json": decision(good_rows), "evidence.json": good_ev,
                     "checks.json": [{"name": "ci", "state": "SUCCESS"}], "diff.txt": test_diff, **(files or {})}
            for fn, body in files.items():
                (proj / fn).write_text(body if isinstance(body, str) else json.dumps(body, ensure_ascii=False), encoding="utf-8")
            argv = ["check", "--project-dir", str(proj), "--decision", str(proj / "decision.json"), "--repo", "acme-fork/app",
                    "--pr", "3", "--head-sha", SHA, "--evidence", str(proj / "evidence.json"), "--checks", str(proj / "checks.json"),
                    "--diff", str(proj / "diff.txt"), "--tests-before", "10", "--tests-after", "11",
                    "--timestamp", "2026-10-24T09:00:00+00:00", *([] if audit else ["--no-audit"]), *(args or [])]
            r = subprocess.run([sys.executable, str(Path(__file__).resolve()), *argv], capture_output=True, text=True,
                               encoding="utf-8", env={**os.environ, "PYTHONUTF8": "1"})
            try:
                res = json.loads(r.stdout)
            except json.JSONDecodeError:
                res = {"decision": "?", "reasons": [r.stdout[-200:] + r.stderr[-300:]]}
            got = res["decision"]
            good = (got in want) and r.returncode == EXIT.get(got, -1)
            if expect_followup and not any(f.get("requirement_id") == expect_followup for f in res.get("followups") or []):
                good = False
                res["reasons"].append(f"缺 {expect_followup} 的 followup")
            print(f"  [{'通過' if good else '失敗'}] {name}：{got}（預期 {'／'.join(want)}）" + ("" if good else f" {res['reasons']}"))
            ok = ok and good
            return res

        print("merge_gate selftest")
        run("traceability 全部一致（json 需求來源）→ merge", ("merge",))
        spec_rows = [row("AC-1", "匯出按鈕下載 CSV"), row("AC-2", "超過一萬筆時提示縮小範圍")]
        spec_ev = [{"requirement_id": "AC-1", "result": "pass", "evidence": "ok"}, {"requirement_id": "AC-2", "result": "pass", "evidence": "ok"}]
        run("traceability 全部一致（spec-md 範例 adapter）→ merge", ("merge",),
            cfg={**base_cfg, "requirements": {"source": "spec-md", "path": "specs/demo/spec.md"}},
            files={"decision.json": decision(spec_rows), "evidence.json": spec_ev})
        run("traceability 全部一致（command 需求來源：團隊自訂腳本）→ merge", ("merge",),
            cfg={**base_cfg, "requirements": {"source": "command", "command": [sys.executable, str(gen)]}},
            files={"decision.json": decision([row("ISSUE-7", "登入頁顯示錯誤訊息")]),
                   "evidence.json": [{"requirement_id": "ISSUE-7", "result": "pass", "evidence": "e2e ok"}]})
        # guard.config.json 沒設 requirements（path、command 都空）時，改讀開發規範 profile（methodology.json）的需求來源
        meth = proj / ".claude/teams/methodology.json"
        meth.write_text(json.dumps({"name": "acme-demo", "requirements": {"source": "command", "command": [sys.executable, str(gen)]}},
                                   ensure_ascii=False), encoding="utf-8")
        res = run("traceability 全部一致（需求來源取自 methodology profile：command 需求來源）→ merge", ("merge",),
                  cfg={**base_cfg, "requirements": {"source": "json", "path": "", "command": ""}},
                  files={"decision.json": decision([row("ISSUE-7", "登入頁顯示錯誤訊息")]),
                         "evidence.json": [{"requirement_id": "ISSUE-7", "result": "pass", "evidence": "e2e ok"}]})
        good = str(res.get("requirements_source", "")).startswith("command")
        print(f"  [{'通過' if good else '失敗'}] methodology profile 的需求來源被採用（requirements_source={res.get('requirements_source')}）")
        ok = ok and good
        run("guard.config.json 有設 requirements 時優先於 methodology profile（json 需求來源）→ merge", ("merge",))
        meth.unlink()
        fu = [{"requirement_id": "REQ-2", "expected": "提示縮小範圍", "actual": "沒有提示", "adjustment": "補提示"}]
        run("traceability mismatch（steward 判 hold）→ hold 並產出 followup", ("hold",),
            files={"decision.json": decision([good_rows[0], row("REQ-2", "超過一萬筆時提示縮小範圍", "mismatch")], "hold", fu)},
            expect_followup="REQ-2")
        res = run("traceability mismatch（steward 宣稱 merge、驗證卻 fail）→ hold 並產出 followup", ("hold",),
                  files={"evidence.json": [good_ev[0], {"requirement_id": "REQ-2", "result": "fail", "evidence": "沒有提示"}]},
                  expect_followup="REQ-2")
        run("traceability 漏列（需求清單有 REQ-2，對照表沒有）→ hold", ("hold",),
            files={"decision.json": decision([good_rows[0]])}, expect_followup="REQ-2")
        run("traceability 證據缺這條需求 → hold", ("hold",), files={"evidence.json": [good_ev[0]]}, expect_followup="REQ-2")
        oos = [{"location": "src/billing.py", "change": "改了計費"}]
        run("traceability out_of_scope（對不到需求的改動）→ hold", ("hold",),
            files={"decision.json": decision(good_rows, "hold", [{"requirement_id": "out_of_scope", "expected": "x", "actual": "y", "adjustment": "z"}], oos)},
            expect_followup="out_of_scope")
        run("head SHA 不符（對照時與目前不同）→ hold", ("hold",), files={"decision.json": decision(good_rows, sha="b" * 40)})
        run("head SHA 不符（審查時與目前不同）→ hold", ("hold",), args=["--reviewed-sha", "c" * 40])
        run("測試數減少 → hold", ("hold",), args=["--tests-before", "12"])
        skip_diff = test_diff + "+@pytest.mark.skip(reason='flaky')\n+def test_big():\n+    pass\n"
        run("新增 skip → hold", ("hold",), files={"diff.txt": skip_diff})
        run("新增 it.only → hold", ("hold",), files={"diff.txt": test_diff.replace("+def test_csv():", "+it.only('csv', () => {})")})
        ci_diff = "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1 +1 @@\n-on: push\n+on: pull_request\n"
        run("改 CI 檔（.github/workflows）→ escalate", ("escalate", "hold"), files={"diff.txt": ci_diff})
        lock_diff = "diff --git a/package-lock.json b/package-lock.json\n--- a/package-lock.json\n+++ b/package-lock.json\n@@ -1 +1 @@\n-a\n+b\n"
        run("改依賴鎖檔 → escalate", ("escalate",), files={"diff.txt": lock_diff})
        jest_diff = "diff --git a/jest.config.js b/jest.config.js\n--- a/jest.config.js\n+++ b/jest.config.js\n@@ -1 +1 @@\n-coverageThreshold: 90\n+coverageThreshold: 50\n"
        run("改測試／覆蓋率設定（jest.config.js）→ escalate", ("escalate",), files={"diff.txt": jest_diff})
        run("新增 # noqa → escalate", ("escalate",), files={"diff.txt": test_diff + "+import os  # noqa: F401\n"})
        run("新增 eslint-disable → escalate", ("escalate",),
            files={"diff.txt": test_diff.replace("+def test_csv():", "+// eslint-disable-next-line no-unused-vars")})
        # rebase 後沿用 verdict：在暫存 git repo 做 feature commit A，main 前進後 cherry-pick 成 A2（patch 相同），另做 C（patch 不同）
        gd = proj / "gitrepo"
        gd.mkdir()

        def git(*args: str) -> str:
            return subprocess.run(["git", "-C", str(gd), "-c", "user.name=示範", "-c", "user.email=demo@example.com",
                                   "-c", "commit.gpgsign=false", *args], capture_output=True, text=True, check=True).stdout.strip()

        git("init", "-q")
        (gd / "a.txt").write_text("1\n", encoding="utf-8")
        git("add", "-A"); git("commit", "-qm", "base")
        base0 = git("rev-parse", "HEAD")
        (gd / "a.txt").write_text("1\n2\n", encoding="utf-8")
        git("commit", "-qam", "A")
        sha_a = git("rev-parse", "HEAD")
        git("checkout", "-q", base0)
        (gd / "b.txt").write_text("x\n", encoding="utf-8")
        git("add", "-A"); git("commit", "-qm", "main 前進")
        base1 = git("rev-parse", "HEAD")
        git("cherry-pick", sha_a)
        sha_a2 = git("rev-parse", "HEAD")
        git("checkout", "-q", base1)
        (gd / "a.txt").write_text("1\n3\n", encoding="utf-8")
        git("commit", "-qam", "C")
        sha_c = git("rev-parse", "HEAD")
        carry_cfg = {**base_cfg, "auto_merge": {**base_cfg["auto_merge"], "carry_verdict_on_rebase": True}}
        rb = ["--head-sha", sha_a2, "--base-sha", base1, "--git-dir", str(gd)]
        res = run("rebase 後 patch-id 相同、carry 開啟 → merge（沿用 verdict）", ("merge",), cfg=carry_cfg,
                  files={"decision.json": decision(good_rows, sha=sha_a)}, args=rb)
        good = any("沿用 verdict" in r for r in res.get("reasons") or [])
        print(f"  [{'通過' if good else '失敗'}] 沿用 verdict 寫進 reasons（{res.get('reasons')}）")
        ok = ok and good
        run("rebase 後 patch-id 相同、carry 關閉 → hold", ("hold",), files={"decision.json": decision(good_rows, sha=sha_a)}, args=rb)
        run("rebase 後 patch 不同、carry 開啟 → hold", ("hold",), cfg=carry_cfg,
            files={"decision.json": decision(good_rows, sha=sha_a)}, args=["--head-sha", sha_c, "--base-sha", base1, "--git-dir", str(gd)])
        run("carry 開啟但沒給 base → hold", ("hold",), cfg=carry_cfg,
            files={"decision.json": decision(good_rows, sha=sha_a)}, args=["--head-sha", sha_a2, "--git-dir", str(gd)])
        run("carry 開啟但舊 commit 不存在（算不出 patch-id）→ hold", ("hold",), cfg=carry_cfg,
            files={"decision.json": decision(good_rows, sha="d" * 40)}, args=rb)
        run("required check 失敗 → hold", ("hold",), files={"checks.json": [{"name": "ci", "state": "FAILURE"}]})
        run("驗證有 anti_cheat 發現 → hold", ("hold",),
            files={"evidence.json": {"commit": SHA, "anti_cheat": ["tests/test_export.py:3 斷言被放寬"], "results": good_ev}})
        run("留存 important 發現 → hold", ("hold",), args=["--findings", str(proj / "findings.json")],
            files={"findings.json": {"lens": "bugs", "findings": [{"id": "B1", "severity": "important", "claim": "c", "location": "a:1", "recommendation": "r", "confidence": 0.9}]}})
        run("repo 不在白名單 → escalate", ("escalate",), cfg={**base_cfg, "auto_merge": {**base_cfg["auto_merge"], "repos": {}}})
        run("required checks 輸入讀不到（fail-closed）→ hold", ("hold",), files={"checks.json": "not json"})
        # 每日上限：先清掉 selftest 前面寫的紀錄，再放入今天已合併 2 次（上限 2）
        audit = proj / "_tpm/runs/merge-audit.jsonl"
        audit.parent.mkdir(parents=True, exist_ok=True)
        audit.write_text("".join(json.dumps({"timestamp": "2026-10-24T0{}:00:00+00:00".format(i), "repo": "acme-fork/app", "pr": i,
                                             "head_sha": SHA, "decision": "merge", "reasons": []}) + "\n" for i in (1, 2)), encoding="utf-8")
        run("超過每日上限 → hold", ("hold",))
        audit.write_text("", encoding="utf-8")
        frz = proj / "_tpm/runs/auto-merge.freeze.json"
        frz.write_text(json.dumps({"frozen": True, "reason": "main CI 在合併後轉紅（示範）"}, ensure_ascii=False), encoding="utf-8")
        run("凍結旗標 frozen=true（預設路徑）→ hold", ("hold",))
        frz.write_text("not json", encoding="utf-8")
        run("凍結旗標讀不懂（fail-closed）→ hold", ("hold",))
        frz.write_text(json.dumps({"frozen": False, "reason": "已由人解凍"}, ensure_ascii=False), encoding="utf-8")
        run("凍結旗標 frozen=false（人已解凍）→ merge", ("merge",))
        frz.unlink()
        custom = proj / "freeze/flag"
        custom.parent.mkdir(parents=True, exist_ok=True)
        custom.write_text("x", encoding="utf-8")
        run("凍結旗標存在（auto_merge.freeze_file 自訂路徑）→ hold", ("hold",),
            cfg={**base_cfg, "auto_merge": {**base_cfg["auto_merge"], "freeze_file": "freeze/flag"}})
        custom.unlink()
        run("稽核紀錄：判定後 append 一行", ("merge",), audit=True)
        recs = read_audit(audit)
        need = {"repo", "pr", "head_sha", "decision", "reasons", "timestamp"}
        good = len(recs) == 1 and need <= set(recs[0]) and recs[0]["head_sha"] == SHA and recs[0]["decision"] == "merge" and recs[0]["pr"] == 3
        print(f"  [{'通過' if good else '失敗'}] merge-audit.jsonl 欄位齊全（{sorted(need)}）")
        ok = ok and good
        # schema 語意規則（schemalib）：宣稱 merge 但有 mismatch
        bad = decision([good_rows[0], row("REQ-2", "x", "mismatch")])
        errs = schemalib.check(bad, "merge-decision")
        good = any("mismatch" in e for e in errs)
        print(f"  [{'通過' if good else '失敗'}] schemalib：traceability 宣稱 merge 但有 mismatch 要被擋（{errs[:1]}）")
        ok = ok and good
    print("merge_gate selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    argv = sys.argv[1:] if argv is None else argv
    if "--selftest" in argv:
        return _selftest()
    p = argparse.ArgumentParser(description="確定性 merge gate（只有 merge 時 exit 0）")
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check", help="判定一個 PR 能不能合併")
    c.add_argument("--decision", help="steward 的 merge-decision 信封或 payload（JSON 檔；- 代表 stdin）")
    c.add_argument("--repo", required=True, help="owner/name")
    c.add_argument("--pr", required=True, type=int)
    c.add_argument("--head-sha", required=True, help="目前 PR 的 head commit")
    c.add_argument("--reviewed-sha", action="append", help="審查／驗證當下的 head（可重複）")
    c.add_argument("--base-sha", help="PR 的 base commit；auto_merge.carry_verdict_on_rebase 開啟時用來算 patch-id，判斷 rebase 後能不能沿用 verdict")
    c.add_argument("--git-dir", help="算 patch-id 用的 git repo（預設專案根）")
    c.add_argument("--evidence", help="正規化證據 [{requirement_id,result,evidence}]，或 criteria 信封")
    c.add_argument("--requirements", help="需求清單路徑（覆蓋 guard.config.json 的 requirements.path）")
    c.add_argument("--requirements-source", choices=sorted(ADAPTERS), help="覆蓋 requirements.source")
    c.add_argument("--checks", help="required checks 狀態 JSON")
    c.add_argument("--findings", action="append", help="審查發現 JSON（findings payload 或信封，可重複）")
    c.add_argument("--diff", help="git diff <base>...<head> 的輸出檔")
    c.add_argument("--tests-before", type=int)
    c.add_argument("--tests-after", type=int)
    c.add_argument("--timestamp", help="ISO 8601；預設系統時間（UTC）")
    c.add_argument("--project-dir", help="專案根目錄（預設 CLAUDE_PROJECT_DIR，或本檔所在專案）")
    c.add_argument("--runs-dir", help="覆蓋 guard.config.json 的 runs_dir")
    c.add_argument("--no-audit", action="store_true", help="不寫 merge-audit.jsonl（只供除錯）")
    a = p.parse_args(argv)
    return cmd_check(a)


if __name__ == "__main__":
    sys.exit(main())
