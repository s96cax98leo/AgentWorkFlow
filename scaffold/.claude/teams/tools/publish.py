#!/usr/bin/env python3
"""確定性推送工具：把一條功能分支推到我方 fork、開 PR 到 fork，前後各做一次確定性檢查。只用標準庫，Windows 可跑。

無人值守時誰推分支、開 PR？由主 session 呼叫這支，不交給 agent（agent 讀得到 PR 內容，可能被說服）。
hook 會擋 tpm-*／dev-* 執行這支；主 session 呼叫時 hook 只看到「python3 …/publish.py」，push 與 gh 寫入在這支裡面做，
所以這支自己把關：

  1. 目標只能是 guard.config.json 的 fork_repo；remote 的 push URL 必須指向 fork，指向上游或禁止的 repo 一律拒絕。
  2. 分支名要符合 allowed_branch_prefixes，不能是 protected_branches（預設 main、master）或 PR 的 base。不 force push。
  3. 推送前掃 diff（<remote>/<base>...<分支>）新增的行：常見金鑰樣式、私鑰區塊、機敏檔名，
     以及 --secrets-file 列的機敏值（逐字比對）。命中就拒絕，只報檔名與樣式名稱，不印出值。
  4. 依政策判定能不能推：--run 的 run.json 裡該段（預設 deliver）要 done 且已核准，前面有閘門的段也都要已核准；
     核准是 --by policy 時，重跑 save_run.policy_violations 讀該段實際結果，有任何違規就拒絕。
  5. git push <remote> refs/heads/<分支>:refs/heads/<分支>，再 gh pr create --repo <fork>。
  6. 每次執行（含拒絕、dry-run）append 一行到 <runs_dir>/publish-audit.jsonl。

用法（在已 init 的專案根目錄）：
  python3 .claude/teams/tools/publish.py push --repo-dir <目標 repo 目錄> --branch feature/x \\
      --run <run_dir> --title "<PR 標題>" --body-file <PR 說明檔> \\
      [--base main] [--remote origin] [--stage deliver] [--draft] [--no-pr] [--dry-run] \\
      [--secrets-file <機敏值清單，一行一個>] [--project-dir <專案根>]
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


def scan_secrets(diff: str, literals: list[str]) -> list[str]:
    """回傳命中的「檔名：樣式名稱」；不回傳值本身。只看新增的行與新增的檔名。"""
    hits: list[str] = []
    cur = "?"
    compiled = [(name, re.compile(p)) for name, p in SECRET_PATTERNS]
    for line in diff.splitlines():
        if line.startswith("+++ "):
            p = line[4:].strip()
            cur = p[2:] if p.startswith("b/") else p
            if cur != "/dev/null" and SECRET_FILES.search(cur) and not SECRET_FILE_OK.search(cur):
                hits.append(f"{cur}：機敏檔名")
            continue
        if not line.startswith("+"):
            continue
        body = line[1:]
        for name, rx in compiled:
            if rx.search(body):
                hits.append(f"{cur}：{name}")
        if any(v and v in body for v in literals):
            hits.append(f"{cur}：--secrets-file 列的機敏值")
    return sorted(set(hits))


def check_policy(run_dir: Path | None, stage: str) -> str:
    """回傳核准紀錄的摘要；不能推就 raise Refused。"""
    if run_dir is None:
        raise Refused("沒有 --run：推送前要有 run.json 的閘門紀錄")
    f = run_dir / "run.json"
    try:
        data = json.loads(f.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise Refused(f"讀不到 {f}：{e}") from e
    stages = data.get("stages") or {}
    st = stages.get(stage)
    if not isinstance(st, dict):
        raise Refused(f"run.json 沒有 {stage} 這一段")
    if st.get("status") != "done":
        raise Refused(f"{stage} 狀態是 {st.get('status')}，不是 done")
    appr = st.get("approved")
    if not appr:
        raise Refused(f"{stage} 還沒核准（save_run.py approve）")
    if stage in save_run.SDLC:
        for prev in save_run.SDLC[: save_run.SDLC.index(stage)]:
            ps = stages.get(prev) or {}
            if ps.get("status") != "done" or (ps.get("gate") and not ps.get("approved")):
                raise Refused(f"前面的 {prev} 段沒有 done 或沒有核准")
    if appr.get("by") == "policy" or appr.get("kind") == "policy":
        sf = run_dir / f"{stage}.json"
        try:
            result = json.loads(sf.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            raise Refused(f"政策核准要讀 {sf.name}，讀不到：{e}") from e
        errs = save_run.policy_violations(stage, result)
        if errs:
            raise Refused("政策判定不通過：" + "；".join(errs[:5]))
    return f"{stage} 由 {appr.get('by')} 核准（{appr.get('kind', '')} @ {appr.get('at', '')}）"


# ------------------------------------------------------------------ 主流程
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
           "stage": a.stage, "decision": "refused", "reasons": [], "pr_url": "", "dry_run": bool(a.dry_run)}
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
        d = git(repo, "diff", "--no-color", "--no-ext-diff", f"{base_ref}...refs/heads/{a.branch}")
        if d.returncode:
            raise Refused(f"git diff 失敗：{d.stderr.strip()[-200:]}")
        if not d.stdout.strip():
            raise Refused(f"{a.branch} 相對 {a.remote}/{a.base} 沒有改動")
        literals = []
        if a.secrets_file:
            try:
                literals = [l.strip() for l in Path(a.secrets_file).read_text(encoding="utf-8").splitlines() if l.strip()]
            except OSError as e:
                raise Refused(f"讀不到 --secrets-file：{e}") from e
        hits = scan_secrets(d.stdout, literals)
        if hits:
            raise Refused(f"diff 疑似含機敏值（{len(hits)} 處）：" + "、".join(hits[:10]))
        rec["reasons"].append(check_policy(run_dir, a.stage))
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
                argv = [*GH, "pr", "create", "--repo", cfg["fork_repo"], "--base", a.base, "--head", a.branch,
                        "--title", a.title, "--body-file", str(Path(a.body_file).resolve())]
                if a.draft:
                    argv.append("--draft")
                g = run(argv, cwd=repo)
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
open(sys.argv[0] + ".log", "a", encoding="utf-8").write(" ".join(sys.argv[1:]) + "\n")
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
        g("checkout", "-q", "main")
        g("branch", "wip/x", "feature/export")

        body = root / "body.md"
        body.write_text("示範 PR 說明\n", encoding="utf-8")
        base = {"summary": "s", "artifacts": [], "evidence": [], "confidence": 1, "open_questions": [], "derived_work": []}

        def make_run(name: str, deliver_by: str | None = "使用者", deliver_payload: dict | None = None) -> Path:
            rd = proj / "_tpm/runs" / name
            sp = str(HERE / "save_run.py")
            sub = lambda *a, stdin="": subprocess.run([sys.executable, sp, *a], input=stdin, capture_output=True, text=True, encoding="utf-8")
            sub("init", str(rd), "--event", "sdlc")
            for stage in save_run.SDLC:
                payload = deliver_payload if (stage == "deliver" and deliver_payload is not None) else {}
                if stage == "test":
                    payload = {"commit": "a" * 40, "scope_covered": "全部", "scope_skipped": "none", "anti_cheat": [],
                               "criteria": [{"id": "AC-1", "criterion": "c", "command": "true", "result": "pass", "evidence": "e"}]}
                sub("stage", str(rd), stage, stdin=json.dumps([{**base, "status": "done", "payload": payload}]))
                if stage == "deliver" and deliver_by is None:
                    break
                if stage == "deliver" and deliver_by == "policy-forged":
                    data = json.loads((rd / "run.json").read_text(encoding="utf-8"))
                    data["stages"]["deliver"]["approved"] = {"by": "policy", "at": "t", "note": "p", "kind": "policy"}
                    (rd / "run.json").write_text(json.dumps(data), encoding="utf-8")
                    break
                sub("approve", str(rd), stage, "--by", deliver_by if stage == "deliver" else "使用者", "--at", "t", "--note", "p")
            return rd

        good_run = make_run("r-good")
        unapproved = make_run("r-pending", deliver_by=None)
        finding = {"lens": "bugs", "findings": [{"id": "B1", "severity": "important", "claim": "c", "location": "a:1",
                                                 "recommendation": "r", "confidence": 0.9}]}
        forged = make_run("r-forged", deliver_by="policy-forged", deliver_payload=finding)

        def case(name: str, want: str, **kw) -> dict:
            nonlocal ok
            args = {"repo_dir": str(repo), "branch": "feature/export", "run": str(good_run), "title": "docs: 匯出說明",
                    "body_file": str(body), "base": "main", "remote": "origin", "stage": "deliver", "draft": False,
                    "no_pr": False, "dry_run": False, "secrets_file": None, "project_dir": str(proj), "timestamp": "2026-10-24T09:00:00+00:00"}
            args.update(kw)
            code, rec = publish(argparse.Namespace(**args))
            good = rec["decision"] == want and (code == 0) == (want in ("published", "dry-run", "pushed"))
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
        case("deliver 段還沒核准 → 拒絕", "refused", run=str(unapproved))
        case("政策核准但實際結果有 important 發現 → 拒絕", "refused", run=str(forged))
        case("沒有 --run → 拒絕", "refused", run=None)
        cfg_up = {**cfg, "fork_repo": "acme-up/app"}
        (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg_up), encoding="utf-8")
        case("fork_repo 設成上游 → 拒絕", "refused")
        (proj / ".claude/teams/guard.config.json").write_text(json.dumps(cfg), encoding="utf-8")
        audit = proj / "_tpm/runs/publish-audit.jsonl"
        recs = [json.loads(l) for l in audit.read_text(encoding="utf-8").splitlines() if l.strip()]
        need = {"timestamp", "repo", "branch", "base", "head_sha", "run", "stage", "decision", "reasons", "pr_url"}
        good = len(recs) == 11 and all(need <= set(r) for r in recs) and recs[1]["decision"] == "published"
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
    c.add_argument("--stage", default="deliver", help="要求已核准的段（預設 deliver）")
    c.add_argument("--base", default="main", help="PR 的 base 分支（預設 main）")
    c.add_argument("--remote", default="origin", help="指向 fork 的 remote（預設 origin）")
    c.add_argument("--title")
    c.add_argument("--body-file")
    c.add_argument("--draft", action="store_true")
    c.add_argument("--no-pr", action="store_true", help="只推分支，不開 PR")
    c.add_argument("--dry-run", action="store_true", help="只做檢查，不推送、不開 PR")
    c.add_argument("--secrets-file", help="機敏值清單（一行一個，逐字比對；只報有／沒有）")
    c.add_argument("--project-dir", help="專案根目錄（預設 CLAUDE_PROJECT_DIR，或本檔所在專案）")
    c.add_argument("--timestamp", help="ISO 8601；預設系統時間（UTC）")
    a = p.parse_args(argv)
    code, rec = publish(a)
    print(json.dumps(rec, ensure_ascii=False, indent=2))
    return code


if __name__ == "__main__":
    sys.exit(main())
