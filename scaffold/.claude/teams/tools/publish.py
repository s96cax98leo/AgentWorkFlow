#!/usr/bin/env python3
"""確定性推送工具：把一條功能分支推到我方 fork、開 PR 到 fork，前後各做一次確定性檢查。只用標準庫，Windows 可跑。

無人值守時誰推分支、開 PR？由主 session 呼叫這支，不交給 agent（agent 讀得到 PR 內容，可能被說服）。
hook 會擋 tpm-*／dev-* 執行這支；主 session 呼叫時 hook 只看到「python3 …/publish.py」，push 與 gh 寫入在這支裡面做，
所以這支自己把關：

  1. 目標只能是 guard.config.json 的 fork_repo；remote 的 push URL 必須指向 fork，指向上游或禁止的 repo 一律拒絕。
  2. 分支名要符合 allowed_branch_prefixes，不能是 protected_branches（預設 main、master）或 PR 的 base。不 force push。
  3. 推送前掃 diff（<remote>/<base>...<分支>）新增的行：常見金鑰樣式、私鑰區塊、機敏檔名，
     以及 --secrets-file 列的機敏值（逐字比對）。命中就拒絕，只報檔名與樣式名稱，不印出值。
     diff 用 merge_gate.parse_diff 解析（引號路徑、改名、「++ 」開頭的新增行）；機敏檔名比對每個 diff --git 區塊，
     再比對 git diff --name-only --diff-filter=ACMRT，所以二進位檔與空檔也會命中；diff 解析不了一律拒絕。
  4. 依政策判定能不能推（和 save_run.py 共用同一套判定）：run 已取消（save_run.py abandon）一律拒絕；
     閘門鏈依 run.json 的 recipe（沒有 recipe 的舊 run 當成 feature）：recipe 有的段到 deliver 都要在 run.json 裡，
     每一段都要 done、有閘門的都要已核准（save_run.chain_errors），deliver 一定要已核准；recipe 沒有的段不要求核准。
     recipe 不能推送（publishable=false，例如 spike）一律拒絕。--stage 只能是 deliver 或更後面的段（例如 merge，要求它也 done），
     給更前面的段或不是 SDLC 的段一律拒絕，不能用來降低要求。到該段為止任何一段的核准是 --by policy 時，
     重跑 save_run.policy_violations 讀該段實際結果（skeptic 推翻、agent 缺件、blocking 問題、執行紀錄缺件等），有任何違規就拒絕。
  4b. diff 形狀（base...分支 的檔名，改名與刪除的舊路徑也算）跑 recipe.diff_shortfalls：recipe 不允許的改動
     （例如 docs 改了程式、沒有 design 卻碰 migration、改依賴宣告卻沒有安全審查）就拒絕，提示用 save_run.py escalate 升級。
  5. git push <remote> refs/heads/<分支>:refs/heads/<分支>，再 gh pr create --repo <fork>。
     有 --supersedes N 時，PR 說明末尾加一行「取代 #N」；舊 PR 由人關閉，本工具不執行 gh pr close。
  6. 每次執行（含拒絕、dry-run）append 一行到 <runs_dir>/publish-audit.jsonl。

用法（在已 init 的專案根目錄）：
  python3 .claude/teams/tools/publish.py push --repo-dir <目標 repo 目錄> --branch feature/x \\
      --run <run_dir> --title "<PR 標題>" --body-file <PR 說明檔> \\
      [--base main] [--remote origin] [--stage deliver] [--draft] [--no-pr] [--dry-run] \\
      [--secrets-file <機敏值清單，一行一個>] [--supersedes <舊 PR 編號>] [--project-dir <專案根>]
  python3 .claude/teams/tools/publish.py --selftest     # 用假的 git push 與 gh，不連網、不真的推送

結束碼：0＝已推送（或 dry-run 檢查全過）；1＝拒絕或推送／開 PR 失敗；2＝參數錯誤。
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

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import merge_gate  # noqa: E402
import recipe as rcp  # noqa: E402
import save_run  # noqa: E402
import schemalib  # noqa: E402

GIT: list[str] = ["git"]
GH: list[str] = ["gh"]
DEFAULT_PREFIXES = ["feature/", "bugfix/", "hotfix/", "release/", "merge/"]
DEFAULT_PROTECTED = ["main", "master"]
PLACEHOLDER = re.compile(r"\{\{[A-Z_]+\}\}")
SLUG_IN_URL = re.compile(r"[:/]([\w.-]+/[\w.-]+?)(?:\.git)?/?$")

# 常見金鑰樣式（只回報名稱，不印值）
SECRET_PATTERNS = [
    ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    ("GitHub token", r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{60,}\b"),
    ("GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}\b"),
    ("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b"),
    ("Slack webhook", r"https://hooks\.slack\.com/services/[A-Za-z0-9/]{20,}"),
    ("Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
    ("Stripe live key", r"\b[sr]k_live_[0-9A-Za-z]{20,}\b"),
    ("OpenAI／Anthropic 類 API key", r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{32,}\b"),
    ("私鑰區塊", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----"),
    ("JWT", r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
    ("連線字串內的密碼", r"\b[a-z][a-z0-9+.-]*://[^\s:/@]+:[^\s@/]{6,}@[\w.-]+"),
    ("疑似寫死的密碼或金鑰", r"(?i)\b(?:password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|private[_-]?key)\b[\"']?\s*[:=]\s*[\"'][^\"'\s]{12,}[\"']"),
]
SECRET_FILES = re.compile(r"(?:^|/)(?:\.env(?:\.[\w-]+)?|id_(?:rsa|dsa|ecdsa|ed25519)|[^/]+\.(?:pem|p12|pfx|key|keystore|jks)|\.npmrc|\.pypirc|credentials(?:\.json)?)$")
SECRET_FILE_OK = re.compile(r"\.env\.(?:example|sample|template)$")


class Refused(Exception):
    pass


# ------------------------------------------------------------------ 設定
def project_dir(arg: str | None) -> Path:
    if arg:
        return Path(arg).resolve()
    if os.environ.get("CLAUDE_PROJECT_DIR"):
        return Path(os.environ["CLAUDE_PROJECT_DIR"]).resolve()
    return schemalib.TEAMS_DIR.parent.parent


def _list(v, default: list[str]) -> list[str]:
    if isinstance(v, str):
        v = [x.strip() for x in v.split(",")]
    out = [x.strip() for x in (v or []) if isinstance(x, str) and x.strip() and not PLACEHOLDER.search(x)]
    return out or default


def load_config(proj: Path) -> dict:
    p = proj / ".claude" / "teams" / "guard.config.json"
    cfg = schemalib.config(p if p.exists() else None) if p.exists() else {}
    return {
        "fork_repo": str(cfg.get("fork_repo") or "").strip(),
        "upstream_repos": _list(cfg.get("upstream_repos"), []),
        "forbidden": [x for x in _list(cfg.get("forbidden_paths"), []) if re.match(r"^[\w.-]+/[\w.-]+$", x)],
        "prefixes": _list(cfg.get("allowed_branch_prefixes"), DEFAULT_PREFIXES),
        "protected": _list(cfg.get("protected_branches"), DEFAULT_PROTECTED),
        "runs_dir": str(cfg.get("runs_dir") or "_tpm/runs").strip().strip("/") or "_tpm/runs",
    }


# ------------------------------------------------------------------ git／gh
def run(argv: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                          stdin=subprocess.DEVNULL, env={**os.environ, "PYTHONUTF8": "1", "GIT_TERMINAL_PROMPT": "0"})


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return run([*GIT, "-C", str(repo), *args])


def slug_of(url: str) -> str:
    m = SLUG_IN_URL.search(url.strip())
    return m.group(1).lower() if m else ""


# ------------------------------------------------------------------ 檢查
def check_target(cfg: dict, branch: str, base: str) -> None:
    fork = cfg["fork_repo"]
    if not fork:
        raise Refused("guard.config.json 沒有 fork_repo，不知道能推到哪裡")
    bad = {s.lower() for s in cfg["upstream_repos"] + cfg["forbidden"]}
    if fork.lower() in bad:
        raise Refused(f"fork_repo {fork} 同時列在 upstream_repos 或 forbidden_paths")
    if not branch or branch.startswith(("-", "+", "refs/")) or ":" in branch or ".." in branch:
        raise Refused(f"分支名不合法：{branch!r}（只寫分支名，例如 feature/x）")
    if branch in cfg["protected"] or branch == base:
        raise Refused(f"{branch} 是受保護分支或 PR 的 base，不直接推送")
    if not branch.startswith(tuple(cfg["prefixes"])):
        raise Refused(f"分支名 {branch} 不在允許的前綴（{'、'.join(cfg['prefixes'])}）")


def check_remote(repo: Path, remote: str, cfg: dict) -> str:
    r = git(repo, "remote", "get-url", "--all", "--push", remote)
    urls = r.stdout.split() if r.returncode == 0 else []
    if not urls:
        raise Refused(f"{repo} 沒有 remote {remote}")
    fork = cfg["fork_repo"].lower()
    for url in urls:
        s = slug_of(url)
        for up in cfg["upstream_repos"] + cfg["forbidden"]:
            if up.lower() == s or up.lower() in url.lower():
                raise Refused(f"remote {remote} 指向上游或禁止的 repo {up}")
        if s != fork:
            raise Refused(f"remote {remote} 的 push URL 指向 {s or url}，不是 fork_repo {cfg['fork_repo']}")
    return urls[0]


def secret_file(path: str) -> bool:
    return bool(path) and path != "/dev/null" and bool(SECRET_FILES.search(path)) and not SECRET_FILE_OK.search(path)


def scan_secrets(diff: str, literals: list[str], names: list[str] | None = None) -> list[str]:
    """回傳命中的「檔名：樣式名稱」；不回傳值本身。看新增的行與留下來的檔名（新增、修改、改名或複製後的路徑）。

    diff 用 merge_gate.parse_diff 解析（和 gate 同一套：引號路徑、改名、「++ 」開頭的新增行）；
    機敏檔名從每個 diff --git 區塊比對，所以沒有 ---／+++ 行的二進位檔（Binary files … differ）與空檔也會命中。
    names：git diff --name-only --diff-filter=ACMRT 的結果，再比對一次（diff 文字解析出問題時的第二道）。
    diff 解析不了就當成命中（無法確認沒有機敏值，fail-closed）。"""
    hits: list[str] = []
    parsed = merge_gate.parse_diff(diff)
    hits += [f"diff：{e}，無法確認沒有機敏值" for e in parsed["errors"]]
    kept = [e["new"] for e in parsed["entries"] if not e["deleted"] and e["new"]]
    hits += [f"{p}：機敏檔名" for p in [*kept, *(names or [])] if secret_file(p)]
    compiled = [(name, re.compile(p)) for name, p in SECRET_PATTERNS]
    for cur, body in parsed["added_lines"]:
        for name, rx in compiled:
            if rx.search(body):
                hits.append(f"{cur}：{name}")
        if any(v and v in body for v in literals):
            hits.append(f"{cur}：--secrets-file 列的機敏值")
    return sorted(set(hits))


MIN_STAGE = "deliver"  # 推送是 deliver 之後的動作：閘門鏈固定要求到 deliver


def check_policy(run_dir: Path | None, stage: str = MIN_STAGE) -> str:
    """回傳核准紀錄的摘要；不能推就 raise Refused。

    固定要求整條閘門鏈到 deliver（含）都 done、有閘門的都已核准；--stage 只能指定 deliver 或更後面的段（更嚴），
    給更前面的段、或不是 SDLC 的段一律拒絕（不能用 --stage 降低要求）。"""
    sdlc = save_run.SDLC
    if stage not in sdlc or sdlc.index(stage) < sdlc.index(MIN_STAGE):
        raise Refused(f"--stage {stage!r} 不行：推送前至少要 {MIN_STAGE} 段核准，--stage 只能是 {'、'.join(sdlc[sdlc.index(MIN_STAGE):])}")
    if run_dir is None:
        raise Refused("沒有 --run：推送前要有 run.json 的閘門紀錄")
    f = run_dir / "run.json"
    try:
        data = json.loads(f.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise Refused(f"讀不到 {f}：{e}") from e
    if data.get("abandoned"):
        raise Refused(save_run.abandoned_error(data) + "，不推送")
    stages = data.get("stages") if isinstance(data.get("stages"), dict) else {}
    tamper = save_run.recipe_tamper_errors(data)
    if tamper:
        raise Refused("；".join(tamper[:3]))
    r = rcp.recipe_of_run(data)
    want = r.get("stages") or []
    if not r.get("publishable", True):
        raise Refused(f"recipe {r.get('name')} 不能推送（publishable=false；spike 拒推，要落地請另開 run）")
    for s in dict.fromkeys([MIN_STAGE, stage]):
        if s not in want:
            raise Refused(f"recipe {r.get('name')} 沒有 {s} 段，不能推送")
    # recipe 裡有閘門的段（與 deliver、--stage）一段都不能少；chain_errors 只略過沒有 recipe 的舊 run 沒有的 sync
    missing = [s for s in sdlc[: sdlc.index(stage) + 1] if s in want and (s in save_run.GATED or s in (MIN_STAGE, stage))
               and not isinstance(stages.get(s), dict)]
    if missing:
        raise Refused(f"run.json 沒有 {'、'.join(missing)} 這{'幾' if len(missing) > 1 else '一'}段")
    for s in dict.fromkeys([MIN_STAGE, stage]):
        st = stages[s]
        if st.get("status") != "done":
            raise Refused(f"{s} 狀態是 {st.get('status')}，不是 done")
        if (s == MIN_STAGE or st.get("gate")) and not st.get("approved"):
            raise Refused(f"{s} 還沒核准（save_run.py approve）")
    appr = stages[MIN_STAGE]["approved"] if isinstance(stages[MIN_STAGE]["approved"], dict) else {}
    chain = save_run.chain_errors(data, stage)
    if chain:
        raise Refused(f"recipe 閘門鏈（{r.get('name')}）不完整：" + "；".join(chain[:5]))
    # 到 stage 為止凡是政策核准的段，都重跑同一套政策判定（run.json 可能被手改，不信任紀錄本身）
    for s in [x for x in sdlc[: sdlc.index(stage) + 1] if x in want and isinstance(stages.get(x), dict)]:
        a2 = stages[s].get("approved") or {}
        if not isinstance(a2, dict):
            raise Refused(f"{s} 段的核准紀錄格式不對（{a2!r}），不採信")
        if a2.get("by") != "policy" and a2.get("kind") != "policy":
            continue
        sf = run_dir / f"{s}.json"
        try:
            result = json.loads(sf.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            raise Refused(f"政策核准要讀 {sf.name}，讀不到：{e}") from e
        errs = save_run.policy_violations(s, result, run_dir)
        if errs:
            raise Refused(f"{s} 段政策判定不通過：" + "；".join(errs[:5]))
    tail = f"；要求到 {stage} 段，已 done" if stage != MIN_STAGE else ""
    return f"recipe {r.get('name')}；{MIN_STAGE} 由 {appr.get('by')} 核准（{appr.get('kind', '')} @ {appr.get('at', '')}）{tail}"


def check_shape(run_dir: Path, paths: list[str]) -> str:
    """diff 形狀和 run.json 的 recipe 要相符（recipe.diff_shortfalls）；不符就 raise Refused，提示用 escalate 升級。"""
    try:
        data = json.loads((run_dir / "run.json").read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise Refused(f"讀不到 {run_dir / 'run.json'}：{e}") from e
    r = rcp.recipe_of_run(data)
    short = rcp.diff_shortfalls(r, paths)
    if short:
        raise Refused(f"diff 形狀和 recipe {r.get('name')} 不符：" + "；".join(short[:5])
                      + f"（要升級 recipe 用 save_run.py escalate {run_dir} --recipe <名> --by --at --reason，重跑缺的段後再推）")
    return f"diff 形狀和 recipe {r.get('name')} 相符（{len(paths)} 個檔）"


# ------------------------------------------------------------------ 主流程
def supersedes_line(n: int) -> str:
    return f"取代 #{n}（原 PR 由人關閉；本工具不關閉）"


def positive_int(v: str) -> int:
    try:
        n = int(v)
    except ValueError:
        n = 0
    if n <= 0:
        raise argparse.ArgumentTypeError(f"要是正整數：{v!r}")
    return n


def append_audit(proj: Path, cfg: dict, rec: dict) -> Path:
    d = Path(cfg["runs_dir"])
    path = (d if d.is_absolute() else proj / d) / "publish-audit.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    return path


def publish(a: argparse.Namespace) -> tuple[int, dict]:
    proj = project_dir(a.project_dir)
    cfg = load_config(proj)
    repo = Path(a.repo_dir).resolve()
    run_dir = Path(a.run).resolve() if a.run else None
    rec = {"timestamp": a.timestamp or datetime.now(timezone.utc).isoformat(timespec="seconds"),
           "repo": cfg["fork_repo"], "branch": a.branch, "base": a.base, "head_sha": "", "run": str(run_dir or ""),
           "stage": a.stage, "decision": "refused", "reasons": [], "pr_url": "", "dry_run": bool(a.dry_run),
           "supersedes": getattr(a, "supersedes", None)}
    code = 1
    try:
        check_target(cfg, a.branch, a.base)
        check_remote(repo, a.remote, cfg)
        head = git(repo, "rev-parse", "--verify", f"refs/heads/{a.branch}" + "^" + "{commit}")
        if head.returncode:
            raise Refused(f"本機沒有分支 {a.branch}")
        rec["head_sha"] = head.stdout.strip()
        base_ref = f"refs/remotes/{a.remote}/{a.base}"
        if git(repo, "rev-parse", "--verify", base_ref + "^" + "{commit}").returncode:
            raise Refused(f"找不到 {a.remote}/{a.base}（先 git fetch {a.remote} {a.base}），無法掃 diff")
        rng = f"{base_ref}...refs/heads/{a.branch}"
        # 明寫 a/、b/ 前綴：使用者設了 diff.noprefix／diff.mnemonicPrefix 時 parse_diff 認不得標頭，會一律拒絕
        d = git(repo, "diff", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", rng)
        if d.returncode:
            raise Refused(f"git diff 失敗：{d.stderr.strip()[-200:]}")
        if not d.stdout.strip():
            raise Refused(f"{a.branch} 相對 {a.remote}/{a.base} 沒有改動")
        nm = git(repo, "diff", "--name-only", "-z", "--no-ext-diff", "--diff-filter=ACMRT", rng)
        if nm.returncode:
            raise Refused(f"git diff --name-only 失敗：{nm.stderr.strip()[-200:]}")
        names = [n for n in nm.stdout.split("\0") if n]
        literals = []
        if a.secrets_file:
            try:
                literals = [l.strip() for l in Path(a.secrets_file).read_text(encoding="utf-8").splitlines() if l.strip()]
            except OSError as e:
                raise Refused(f"讀不到 --secrets-file：{e}") from e
        hits = scan_secrets(d.stdout, literals, names)
        if hits:
            raise Refused(f"diff 疑似含機敏值（{len(hits)} 處）：" + "、".join(hits[:10]))
        rec["reasons"].append(check_policy(run_dir, a.stage))
        shape_paths = sorted(set(merge_gate.parse_diff(d.stdout)["files"]) | set(names))
        rec["reasons"].append(check_shape(run_dir, shape_paths))
        if not a.no_pr and not (a.title and a.body_file and Path(a.body_file).is_file()):
            raise Refused("開 PR 要有 --title 與存在的 --body-file（或加 --no-pr 只推分支）")
        if a.dry_run:
            rec["decision"] = "dry-run"
            rec["reasons"].append("檢查全部通過（dry-run，沒有推送）")
            code = 0
        else:
            p = git(repo, "push", a.remote, f"refs/heads/{a.branch}:refs/heads/{a.branch}")
            if p.returncode:
                raise Refused(f"git push 失敗：{p.stderr.strip()[-300:]}")
            rec["decision"] = "pushed"
            if not a.no_pr:
                body_file = Path(a.body_file).resolve()
                tmp_body = None
                if rec["supersedes"]:
                    text = body_file.read_text(encoding="utf-8").rstrip("\n")
                    text += f"\n\n{supersedes_line(rec['supersedes'])}\n"
                    fd, name = tempfile.mkstemp(prefix="publish-body-", suffix=".md")
                    with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
                        f.write(text)
                    tmp_body = body_file = Path(name)
                argv = [*GH, "pr", "create", "--repo", cfg["fork_repo"], "--base", a.base, "--head", a.branch,
                        "--title", a.title, "--body-file", str(body_file)]
                if a.draft:
                    argv.append("--draft")
                try:
                    g = run(argv, cwd=repo)
                finally:
                    if tmp_body:
                        tmp_body.unlink(missing_ok=True)
                if g.returncode:
                    raise Refused(f"已推送，但 gh pr create 失敗：{g.stderr.strip()[-300:]}")
                rec["pr_url"] = (g.stdout.strip().splitlines() or [""])[-1]
                rec["decision"] = "published"
            code = 0
    except Refused as e:
        rec["reasons"].append(str(e))
    rec["audit"] = str(append_audit(proj, cfg, {k: v for k, v in rec.items() if k != "audit"}))
    return code, rec


# ------------------------------------------------------------------ selftest
FAKE_GIT = r'''import subprocess, sys
args = sys.argv[1:]
log = sys.argv[0] + ".log"
if "push" in args:
    open(log, "a", encoding="utf-8").write(" ".join(args) + "\n")
    sys.exit(0)
sys.exit(subprocess.run(["git", *args]).returncode)
'''
FAKE_GH = r'''import sys
args = sys.argv[1:]
open(sys.argv[0] + ".log", "a", encoding="utf-8").write(" ".join(args) + "\n")
if "--body-file" in args:
    body = open(args[args.index("--body-file") + 1], encoding="utf-8").read()
    open(sys.argv[0] + ".body", "w", encoding="utf-8").write(body)
print("https://github.com/acme-fork/app/pull/7")
'''


def _selftest() -> int:
    global GIT, GH
    ok = True
    with tempfile.TemporaryDirectory() as d:
        root = Path(d)
        proj = root / "proj"
        (proj / ".claude/teams").mkdir(parents=True)
        cfg = {"runs_dir": "_tpm/runs", "fork_repo": "acme-fork/app", "upstream_repos": "acme-up/app",
               "allowed_branch_prefixes": "feature/,bugfix/", "protected_branches": ["main", "master"]}
        (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg), encoding="utf-8")
        # 專案 profile 用內建 recipes（stage plan 用 --profile 指定這份；check_policy 重算 recipe 時也讀這份）
        save_run.PROJECT_PROFILE = proj / ".claude/teams/methodology.json"
        (proj / ".claude/teams/methodology.json").write_text(
            json.dumps({"recipes": rcp.BUILTIN_RECIPES, "modifiers": rcp.BUILTIN_MODIFIERS}, ensure_ascii=False), encoding="utf-8")
        epic = proj / "epic"
        (epic / "specs/e").mkdir(parents=True)
        (epic / "specs/e/intent.md").write_text(
            "# intent\n## 目標\nx\n## 範圍\nx\n## 不做的事\nx\n## 驗收條件\n- AC-1\n## 調研問題\nx\n## 判準\nx\n## 現象\nx\n## 重現步驟\nx\n## 預期行為\nx\n## 根因\nx\n", encoding="utf-8")
        fg, fh = root / "fake_git.py", root / "fake_gh.py"
        fg.write_text(FAKE_GIT, encoding="utf-8")
        fh.write_text(FAKE_GH, encoding="utf-8")
        GIT, GH = [sys.executable, str(fg)], [sys.executable, str(fh)]

        repo = proj / "repos/app"
        repo.mkdir(parents=True)

        def g(*args: str) -> str:
            r = subprocess.run(["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                               capture_output=True, text=True, encoding="utf-8")
            if r.returncode:
                raise RuntimeError(r.stderr)
            return r.stdout.strip()

        g("init", "-q", "-b", "main")
        (repo / "README.md").write_text("# 示範客戶 ACME\n", encoding="utf-8")
        g("add", "-A")
        g("commit", "-qm", "init")
        g("remote", "add", "origin", "https://github.com/acme-fork/app.git")
        g("remote", "add", "upstream", "https://github.com/acme-up/app.git")
        g("update-ref", "refs/remotes/origin/main", "HEAD")
        g("update-ref", "refs/remotes/upstream/main", "HEAD")
        g("checkout", "-qb", "feature/export")
        (repo / "docs").mkdir()
        (repo / "docs/export.md").write_text("# 匯出 CSV\n", encoding="utf-8")
        g("add", "-A")
        g("commit", "-qm", "docs: 匯出說明")
        g("checkout", "-qb", "feature/leak")
        key = "AKIA" + "Q" * 16  # 執行時組出來，不在原始碼留完整樣式
        (repo / "config.py").write_text(f"AWS_KEY = '{key}'\n", encoding="utf-8")
        g("add", "-A")
        g("commit", "-qm", "feat: 設定")
        # A-4：沒有 ---／+++ 行的機敏檔（二進位憑證、空的 .env），以及內容以「++ 」開頭的金鑰行（整行是 +++，舊版當成標頭）
        for br, fn, data in (("feature/p12", "certs/client.p12", b"\x00\x01\x02\xff binary"), ("feature/emptyenv", ".env", b""),
                             ("feature/plus3", "notes.md", f"++ {key}\n".encode())):
            g("checkout", "-q", "main")
            g("checkout", "-qb", br)
            (repo / fn).parent.mkdir(parents=True, exist_ok=True)
            (repo / fn).write_bytes(data)
            g("add", "-A")
            g("commit", "-qm", f"feat: {fn}")
        g("checkout", "-q", "main")
        g("checkout", "-qb", "feature/code")
        (repo / "src").mkdir()
        (repo / "src/export.py").write_text("def export():\n    return 'csv'\n", encoding="utf-8")
        g("add", "-A")
        g("commit", "-qm", "feat: 匯出")
        g("checkout", "-q", "main")
        g("branch", "wip/x", "feature/export")

        body = root / "body.md"
        body.write_text("示範 PR 說明\n", encoding="utf-8")
        base = {"summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": []}

        def make_run(name: str, deliver_by: str | None = "使用者", deliver_payload: dict | None = None,
                     deliver_extra: dict | None = None, recipe: str | None = None) -> Path:
            rd = proj / "_tpm/runs" / name
            sp = str(HERE / "save_run.py")
            sub = lambda *a, stdin="": subprocess.run([sys.executable, sp, *a], input=stdin, capture_output=True, text=True, encoding="utf-8")
            sub("init", str(rd), "--event", "sdlc")
            for stage in save_run.SDLC:
                if stage != "plan" and json.loads((rd / "run.json").read_text(encoding="utf-8"))["stages"][stage]["status"] == "skipped":
                    continue
                payload = deliver_payload if (stage == "deliver" and deliver_payload is not None) else {}
                if stage == "test":
                    payload = {"commit": "a" * 40, "scope_covered": "全部", "scope_skipped": "none", "anti_cheat": [],
                               "criteria": [{"id": "AC-1", "criterion": "c", "command": "true", "result": "pass", "evidence": "e"}]}
                res = {"x": [{**base, "status": "done", "payload": payload}]}
                if stage == "plan":
                    res = {"epic_dir": str(epic), "intent": {**base, "status": "done", "payload": {
                        "documents": [{"path": "specs/e/intent.md", "kind": "intent", "audience": "owner"}]}}}
                    if recipe:
                        res["recipe"] = {"name": recipe, "modifiers": [], "size": "small"}
                if stage == "deliver" and deliver_extra:
                    res.update(deliver_extra)
                if recipe and stage != "plan":
                    # 新版 workflow 的回傳：帶 run.json 鎖定的 recipe 與 trace（每個該派的角色一筆，紀錄檔已寫）
                    rec = json.loads((rd / "run.json").read_text(encoding="utf-8"))["recipe"]
                    rv = rec["resolved"]
                    res["recipe"] = {"name": rec["name"], "modifiers": rec["modifiers"], "stages": rv["stages"]}
                    roles = [("agent", "agent-workflow:dev-implementer")]
                    if stage == "test":
                        roles = [("verify#1", "agent-workflow:dev-verifier")] + [
                            (k, "agent-workflow:" + save_run.REVIEW_ROLES[k]) for k in save_run.REVIEW_ROLES if rv["test"].get(k) == "always"]
                    if stage == "deliver":
                        roles = [(f"review:{a}", "agent-workflow:tpm-reviewer") for a in rv.get("deliver_axes") or []]
                    (rd / stage).mkdir(parents=True, exist_ok=True)
                    res["trace"] = []
                    for label, at in roles:
                        f = rd / stage / (label.replace(":", "-").replace("#", "-") + ".md")
                        f.write_text("# 紀錄\n", encoding="utf-8")
                        res["trace"].append({"label": label, "agent_type": at, "phase": "p", "record": str(f), "ok": True})
                prof = ["--profile", str(proj / ".claude/teams/methodology.json")]
                sub("stage", str(rd), stage, *prof, stdin=json.dumps(res))
                if stage == "deliver" and deliver_by is None:
                    break
                if stage == "deliver" and deliver_by == "policy-forged":
                    data = json.loads((rd / "run.json").read_text(encoding="utf-8"))
                    data["stages"]["deliver"]["approved"] = {"by": "policy", "at": "t", "note": "p", "kind": "policy"}
                    (rd / "run.json").write_text(json.dumps(data), encoding="utf-8")
                    break
                sub("approve", str(rd), stage, "--by", deliver_by if stage == "deliver" else "使用者", "--at", "t", "--note", "p", *prof)
            return rd

        good_run = make_run("r-good")
        unapproved = make_run("r-pending", deliver_by=None)
        finding = {"lens": "bugs", "findings": [{"id": "B1", "severity": "important", "claim": "c", "location": "a:1",
                                                 "recommendation": "r", "confidence": 0.9}]}
        forged = make_run("r-forged", deliver_by="policy-forged", deliver_payload=finding)
        refuted = {"verdicts": [{"target_id": "PR 說明", "refuted": True, "reason": "漏列驗收條件"}]}
        skeptic = make_run("r-skeptic", deliver_by="policy-forged", deliver_payload=refuted)
        lost = make_run("r-lost", deliver_by="policy-forged", deliver_extra={"docs": None})
        docs_run = make_run("r-docs", recipe="docs")
        bug_run = make_run("r-bugfix", recipe="bugfix")
        spike_run = make_run("r-spike", deliver_by=None, recipe="spike")
        dropped = make_run("r-abandon")
        subprocess.run([sys.executable, str(HERE / "save_run.py"), "abandon", str(dropped), "--reason", "需求取消",
                        "--by", "使用者", "--at", "t"], capture_output=True, text=True, encoding="utf-8")

        n_cases = [0]

        def case(name: str, want: str, why: str | None = None, **kw) -> dict:
            nonlocal ok
            n_cases[0] += 1
            args = {"repo_dir": str(repo), "branch": "feature/export", "run": str(good_run), "title": "docs: 匯出說明",
                    "body_file": str(body), "base": "main", "remote": "origin", "stage": "deliver", "draft": False,
                    "no_pr": False, "dry_run": False, "secrets_file": None, "supersedes": None, "project_dir": str(proj),
                    "timestamp": "2026-10-24T09:00:00+00:00"}
            args.update(kw)
            code, rec = publish(argparse.Namespace(**args))
            good = rec["decision"] == want and (code == 0) == (want in ("published", "dry-run", "pushed"))
            if why and not any(why in r for r in rec["reasons"]):  # 要因為這條理由拒絕，不是剛好被別的理由擋下
                good = False
                rec["reasons"].append(f"理由裡沒有「{why}」")
            verbose = (not good) or bool(os.environ.get("AWF_VERBOSE"))
            print(f"  [{'通過' if good else '失敗'}] {name}：{rec['decision']}（預期 {want}）" + (f" {rec['reasons']}" if verbose else ""))
            ok = ok and good
            return rec

        print("publish selftest（假的 git push 與 gh，不連網）")
        for f in (Path(str(fg) + ".log"), Path(str(fh) + ".log")):
            f.unlink(missing_ok=True)
        case("dry-run：檢查全過但不推送", "dry-run", dry_run=True)
        pushed = Path(str(fg) + ".log").read_text(encoding="utf-8") if Path(str(fg) + ".log").exists() else ""
        good = pushed == ""
        print(f"  [{'通過' if good else '失敗'}] dry-run 沒有呼叫 git push")
        ok = ok and good
        rec = case("功能分支、已核准、無機敏值 → 推送並開 PR", "published")
        pushed = Path(str(fg) + ".log").read_text(encoding="utf-8")
        ghlog = Path(str(fh) + ".log").read_text(encoding="utf-8")
        good = ("push origin refs/heads/feature/export:refs/heads/feature/export" in pushed and "--force" not in pushed
                and "pr create --repo acme-fork/app" in ghlog and rec["pr_url"].endswith("/pull/7"))
        print(f"  [{'通過' if good else '失敗'}] push 只推該分支、不 force；gh pr create 明確寫 --repo fork")
        ok = ok and good
        Path(str(fh) + ".log").unlink(missing_ok=True)
        rec = case("--supersedes 12 → 推送並開 PR", "published", supersedes=12)
        ghlog = Path(str(fh) + ".log").read_text(encoding="utf-8")
        sent = Path(str(fh) + ".body").read_text(encoding="utf-8")
        good = ("取代 #12" in sent and sent.startswith("示範 PR 說明") and "pr close" not in ghlog
                and str(body) not in ghlog and rec["supersedes"] == 12 and "取代" not in body.read_text(encoding="utf-8"))
        print(f"  [{'通過' if good else '失敗'}] PR 說明含「取代 #12」、原說明檔不動、沒有執行 gh pr close")
        ok = ok and good
        case("推到受保護分支 main → 拒絕", "refused", branch="main")
        case("分支前綴不合規（wip/）→ 拒絕", "refused", branch="wip/x")
        case("remote 指向上游 → 拒絕", "refused", remote="upstream")
        leak = case("diff 含 AWS 金鑰樣式 → 拒絕", "refused", branch="feature/leak")
        good = key not in json.dumps(leak, ensure_ascii=False) and any("AWS" in r for r in leak["reasons"])
        print(f"  [{'通過' if good else '失敗'}] 機敏值命中只報樣式名稱，不印出值")
        ok = ok and good
        lit = root / "secrets.txt"
        lit.write_text("匯出 CSV\n", encoding="utf-8")
        case("diff 含 --secrets-file 列的值 → 拒絕", "refused", secrets_file=str(lit))
        case("新增二進位憑證檔（.p12，diff 只有 Binary files 行）→ 拒絕", "refused", why="certs/client.p12：機敏檔名", branch="feature/p12")
        case("新增空的 .env（diff 沒有 ---／+++ 行）→ 拒絕", "refused", why=".env：機敏檔名", branch="feature/emptyenv")
        rec = case("新增行內容以「++ 」開頭的金鑰（整行是 +++）→ 拒絕", "refused", why="notes.md：AWS access key", branch="feature/plus3")
        good = key not in json.dumps(rec, ensure_ascii=False)
        print(f"  [{'通過' if good else '失敗'}] 「++ 」開頭的金鑰命中也不印出值")
        ok = ok and good
        # A-7：--stage 只能要求 deliver 或更後面的段；給更前面的段不能降低要求
        case("--stage test（比 deliver 前面，deliver 還沒核准）→ 拒絕", "refused", why="--stage 'test' 不行", run=str(unapproved), stage="test")
        case("--stage plan（比 deliver 前面）→ 拒絕", "refused", why="--stage 'plan' 不行", stage="plan")
        case("--stage run（不是 SDLC 的段）→ 拒絕", "refused", why="--stage 'run' 不行", stage="run")
        mpend = proj / "_tpm/runs/r-merge-pending"
        mpend.mkdir(parents=True)
        data = json.loads((good_run / "run.json").read_text(encoding="utf-8"))
        data["stages"]["merge"] = {"status": "pending", "gate": False}
        (mpend / "run.json").write_text(json.dumps(data), encoding="utf-8")
        case("--stage merge（更嚴：merge 段還沒 done）→ 拒絕", "refused", why="merge 狀態是 pending", stage="merge", run=str(mpend))
        case("--stage merge（deliver 已核准、merge 已 done）→ dry-run 通過", "dry-run", why="要求到 merge 段", stage="merge", dry_run=True)
        nodeliver = proj / "_tpm/runs/r-nodeliver"
        nodeliver.mkdir(parents=True)
        data = json.loads((good_run / "run.json").read_text(encoding="utf-8"))
        data["stages"].pop("deliver")
        data["stages"].pop("design")
        (nodeliver / "run.json").write_text(json.dumps(data), encoding="utf-8")
        case("run.json 少了有閘門的 deliver、design 段（手改）→ 拒絕", "refused", why="run.json 沒有 design、deliver", run=str(nodeliver))
        case("deliver 段還沒核准 → 拒絕", "refused", run=str(unapproved))
        case("政策核准但實際結果有 important 發現 → 拒絕", "refused", run=str(forged))
        case("政策核准但 skeptic 推翻了產出 → 拒絕", "refused", run=str(skeptic))
        case("政策核准但 agent 缺件（docs: null）→ 拒絕", "refused", run=str(lost))
        case("run 已 abandon（需求取消）→ 拒絕", "refused", run=str(dropped))
        case("recipe 閘門鏈：bugfix 沒有 design 段（skipped）不要求核准 → dry-run 通過", "dry-run", why="recipe bugfix", run=str(bug_run),
             branch="feature/code", dry_run=True)
        bug_pending = proj / "_tpm/runs/r-bugfix-test-pending"
        bug_pending.mkdir(parents=True)
        data = json.loads((bug_run / "run.json").read_text(encoding="utf-8"))
        data["stages"]["test"].pop("approved")
        (bug_pending / "run.json").write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        case("recipe 閘門鏈：bugfix 的 test 段沒核准 → 拒絕", "refused", why="前面的 test 段還沒核准", run=str(bug_pending), branch="feature/code")
        bug_gone = proj / "_tpm/runs/r-bugfix-no-build"
        bug_gone.mkdir(parents=True)
        data["stages"]["test"]["approved"] = {"by": "使用者", "at": "t", "note": "", "kind": "human"}
        data["stages"].pop("build")
        (bug_gone / "run.json").write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        case("recipe 閘門鏈：recipe 有的 build 段被拿掉 → 拒絕", "refused", why="缺段", run=str(bug_gone), branch="feature/code")
        case("spike 拒推（recipe publishable=false）", "refused", why="spike 拒推", run=str(spike_run))
        case("diff 形狀：docs 改程式拒推（提示 escalate）", "refused", why="save_run.py escalate", run=str(docs_run), branch="feature/code")
        case("diff 形狀：相符放行（docs recipe 只改文件）→ 推送並開 PR", "published", why="diff 形狀和 recipe docs 相符", run=str(docs_run))
        case("沒有 --run → 拒絕", "refused", run=None)
        cfg_up = {**cfg, "fork_repo": "acme-up/app"}
        (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg_up), encoding="utf-8")
        case("fork_repo 設成上游 → 拒絕", "refused")
        (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg), encoding="utf-8")
        audit = proj / "_tpm/runs/publish-audit.jsonl"
        recs = [json.loads(l) for l in audit.read_text(encoding="utf-8").splitlines() if l.strip()]
        need = {"timestamp", "repo", "branch", "base", "head_sha", "run", "stage", "decision", "reasons", "pr_url"}
        good = len(recs) == n_cases[0] and all(need <= set(r) for r in recs) and recs[1]["decision"] == "published"
        print(f"  [{'通過' if good else '失敗'}] publish-audit.jsonl 每次執行都有一行、欄位齊全（{len(recs)} 行）")
        ok = ok and good
    GIT, GH = ["git"], ["gh"]
    print("publish selftest：" + ("全部通過" if ok else "有失敗"))
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
    p = argparse.ArgumentParser(description="確定性推送工具：推功能分支到 fork 並開 PR")
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("push", help="檢查後推送並開 PR")
    c.add_argument("--repo-dir", required=True, help="目標 repo（或 worktree）目錄")
    c.add_argument("--branch", required=True, help="要推的本機分支（遠端用同名）")
    c.add_argument("--run", help="run 目錄（含 run.json）；沒有就拒絕")
    c.add_argument("--stage", default="deliver", help="閘門鏈要求到哪一段（預設 deliver；只能是 deliver 或更後面的段，不能降低要求）")
    c.add_argument("--base", default="main", help="PR 的 base 分支（預設 main）")
    c.add_argument("--remote", default="origin", help="指向 fork 的 remote（預設 origin）")
    c.add_argument("--title")
    c.add_argument("--body-file")
    c.add_argument("--draft", action="store_true")
    c.add_argument("--no-pr", action="store_true", help="只推分支，不開 PR")
    c.add_argument("--dry-run", action="store_true", help="只做檢查，不推送、不開 PR")
    c.add_argument("--secrets-file", help="機敏值清單（一行一個，逐字比對；只報有／沒有）")
    c.add_argument("--supersedes", type=positive_int, help="這個 PR 取代的舊 PR 編號；只在說明加一行，不關閉舊 PR")
    c.add_argument("--project-dir", help="專案根目錄（預設 CLAUDE_PROJECT_DIR，或本檔所在專案）")
    c.add_argument("--timestamp", help="ISO 8601；預設系統時間（UTC）")
    a = p.parse_args(argv)
    code, rec = publish(a)
    print(json.dumps(rec, ensure_ascii=False, indent=2))
    return code


if __name__ == "__main__":
    sys.exit(main())
