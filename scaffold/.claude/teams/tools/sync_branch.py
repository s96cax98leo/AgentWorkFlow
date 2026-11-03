#!/usr/bin/env python3
"""確定性分支同步工具：讓 epic／功能分支跟上 base（預設 origin/main），策略自動判定或固定。只用標準庫，Windows 可跑。

策略（methodology.json 的 conventions.sync.strategy；沒寫＝auto，--strategy 可覆蓋）：
  auto     已跟上 → none；未推送且無衝突 → rebase；已推送且無衝突 → merge（把 base merge 進來）；有衝突 → transit。
  rebase   已推送（要 force push）或有衝突 → 停下交人。
  merge    有衝突 → 停下交人。
  transit  一律可行：從 base 開中轉分支，把功能分支 merge 進去，衝突在中轉分支上解，再開 PR。
  固定策略遇到做不到的情況一律停下（halted），不自動改走別的策略。

中轉分支命名取 conventions.sync.transit_naming（預設 merge/<短名>-into-<base>-<YYYYMMDD>），
前綴（第一個 / 之前，含 /）要在 guard.config.json 的 allowed_branch_prefixes 裡。腳本不取時間，日期由呼叫端給 --date。

用法（在已 init 的專案根目錄）：
  python3 .claude/teams/tools/sync_branch.py plan --repo <repo 目錄> --branch feature/x \\
      [--base origin/main] [--remote origin] [--strategy auto|rebase|merge|transit] [--short <短名>] \\
      [--date YYYYMMDD] [--config <guard.config.json>] [--methodology <methodology.json>] [--no-fetch]
  python3 .claude/teams/tools/sync_branch.py apply --repo <repo 目錄> --branch feature/x --strategy rebase|merge|transit \\
      [--base origin/main] [--transit-branch <中轉分支>] [--date YYYYMMDD] [--config <guard.config.json>]
  python3 .claude/teams/tools/sync_branch.py check-transit --repo <repo 目錄> --branch feature/x --base origin/main \\
      --expected <檔案1,檔案2,…；沒有就給空字串>
  python3 .claude/teams/tools/sync_branch.py --selftest

  plan           只讀（會 git fetch，除非 --no-fetch）。輸出 behind、published、conflicts、strategy、transit_branch…
  apply          worktree 要在 --branch 上且乾淨。rebase 先寫備份 ref refs/backup/<分支，/ 換成 ->-<日期>（已存在就停）；
                 rebase／merge 失敗會 abort。transit 從 base 開中轉分支（--no-track）再 merge 功能分支，衝突留在 worktree。
  check-transit  中轉分支解完衝突、commit 後執行：HEAD 要是兩個 parent 的 merge commit，base 與功能分支都是祖先，
                 改動的檔案沒有衝突標記；--expected 以外的檔案要和 git 自動合併的結果一模一樣（擋夾帶改動、也擋默默丟掉某一邊的改動）。

輸出：stdout 一個 JSON；停下時 "halted": true 並附 reason。參數錯誤也輸出 JSON（halted）並以 1 結束。
結束碼：0＝成功（plan 有結論、apply 完成、check-transit 通過）；1＝停下交人或檢查不通過；
        2＝apply transit 有衝突，已留在中轉分支的 worktree；3＝apply rebase／merge 失敗，已 abort 回原狀。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

GIT: list[str] = ["git"]
DEFAULT_PREFIXES = ["feature/", "bugfix/", "hotfix/", "release/", "merge/"]  # 同 plugin/hooks/awf_config.py
DEFAULT_TRANSIT = "merge/<短名>-into-<base>-<YYYYMMDD>"
STRATEGIES = ("auto", "rebase", "merge", "transit")
PLACEHOLDER = re.compile(r"\{\{[A-Z_]+\}\}")
DATE = re.compile(r"^\d{8}$")
MARKER = re.compile(r"^(?:<<<<<<< |=======$|>>>>>>> )")


class Halt(Exception):
    pass


class _Parser(argparse.ArgumentParser):
    """參數錯誤一律當成停下（exit 1＋JSON），不和 apply transit 的 exit 2 混在一起。"""

    def error(self, message: str):
        raise Halt(f"參數錯誤：{message}")


# ------------------------------------------------------------------ git
def run(argv: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                          stdin=subprocess.DEVNULL,
                          env={**os.environ, "PYTHONUTF8": "1", "GIT_TERMINAL_PROMPT": "0", "GIT_EDITOR": "true", "LC_ALL": "C"})


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return run([*GIT, "-C", str(repo), *args])


def must(repo: Path, *args: str) -> str:
    r = git(repo, *args)
    if r.returncode:
        raise Halt(f"git {' '.join(args[:3])} 失敗：{(r.stderr or r.stdout).strip()[-300:]}")
    return r.stdout


def lines(out: str) -> list[str]:
    return [l.strip().replace("\\", "/") for l in out.replace("\r\n", "\n").split("\n") if l.strip()]


def resolve(repo: Path, ref: str) -> str:
    r = git(repo, "rev-parse", "--verify", "--quiet", ref + "^{commit}")
    return r.stdout.strip() if r.returncode == 0 else ""


def is_ancestor(repo: Path, a: str, b: str) -> bool:
    r = git(repo, "merge-base", "--is-ancestor", a, b)
    if r.returncode not in (0, 1):
        raise Halt(f"merge-base --is-ancestor 失敗：{r.stderr.strip()[-200:]}")
    return r.returncode == 0


def unmerged(repo: Path) -> list[str]:
    return sorted(set(lines(git(repo, "diff", "--name-only", "--diff-filter=U").stdout)))


def merge_tree(repo: Path, a: str, b: str) -> tuple[str, list[str]]:
    """回傳 (合併結果的 tree, 衝突檔清單)。exit 1＝有衝突；其他失敗（含不支援 --write-tree 的舊 git）→ 停下。"""
    r = git(repo, "merge-tree", "--write-tree", "--name-only", a, b)
    if r.returncode not in (0, 1):
        raise Halt(f"git merge-tree --write-tree 失敗（需要 git 2.38 以上）：{(r.stderr or r.stdout).strip()[-200:]}")
    out = r.stdout.replace("\r\n", "\n").split("\n")
    tree = out[0].strip() if out else ""
    if not re.fullmatch(r"[0-9a-f]{40,64}", tree):
        raise Halt(f"看不懂 merge-tree 的輸出：{r.stdout[:200]!r}")
    files: list[str] = []
    if r.returncode == 1:
        for l in out[1:]:
            if not l.strip():
                break
            files.append(l.strip().replace("\\", "/"))
        if not files:
            raise Halt("merge-tree 回報有衝突，但讀不到衝突檔清單")
    return tree, sorted(set(files))


# ------------------------------------------------------------------ 設定
def load_json(path: Path, what: str) -> dict:
    """檔案不存在＝空設定（用預設值）；存在但讀不懂＝停下（不做樂觀推定）。"""
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise Halt(f"{what} {path} 讀不懂：{e}") from e
    if not isinstance(data, dict):
        raise Halt(f"{what} {path} 不是 JSON 物件")
    return data


def prefixes(repo: Path, config: str | None) -> list[str]:
    cfg = load_json(Path(config) if config else repo / ".claude/teams/guard.config.json", "guard.config")
    v = cfg.get("allowed_branch_prefixes")
    if isinstance(v, str):
        v = v.split(",")
    out = [x.strip() for x in (v or []) if isinstance(x, str) and x.strip() and not PLACEHOLDER.search(x)]
    return out or DEFAULT_PREFIXES


def sync_conventions(repo: Path, methodology: str | None) -> dict:
    prof = load_json(Path(methodology) if methodology else repo / ".claude/teams/methodology.json", "methodology")
    sync = ((prof.get("conventions") or {}).get("sync") or {}) if isinstance(prof.get("conventions"), dict) else {}
    if not isinstance(sync, dict):
        raise Halt("methodology 的 conventions.sync 不是物件")
    strategy = sync.get("strategy", "auto")
    if strategy not in STRATEGIES:
        raise Halt(f"methodology 的 conventions.sync.strategy 不認得：{strategy!r}")
    naming = sync.get("transit_naming") or DEFAULT_TRANSIT
    if not isinstance(naming, str):
        raise Halt("methodology 的 conventions.sync.transit_naming 不是字串")
    return {"strategy": strategy, "transit_naming": naming}


def base_name(base: str, remote: str) -> str:
    for p in (f"refs/remotes/{remote}/", f"{remote}/", "refs/heads/"):
        if base.startswith(p):
            return base[len(p):]
    return base


def check_branch_name(repo: Path, name: str, allowed: list[str]) -> None:
    if git(repo, "check-ref-format", "--branch", name).returncode:
        raise Halt(f"分支名不合法：{name!r}")
    if "/" not in name or name[: name.index("/") + 1] not in allowed:
        raise Halt(f"中轉分支 {name} 的前綴不在 allowed_branch_prefixes（{'、'.join(allowed)}）")


def transit_name(repo: Path, naming: str, short: str, base: str, date: str | None, allowed: list[str]) -> str:
    if not date:
        raise Halt("策略是 transit，但沒有 --date（中轉分支名要日期；腳本不取時間）")
    if not DATE.match(date):
        raise Halt(f"--date 要是 YYYYMMDD：{date!r}")
    name = naming.replace("<短名>", short).replace("<base>", base).replace("<YYYYMMDD>", date)
    if "<" in name or ">" in name:
        raise Halt(f"transit_naming 有不認得的佔位符：{naming}")
    check_branch_name(repo, name, allowed)
    return name


# ------------------------------------------------------------------ plan
def decide(requested: str, behind: int, published: bool, conflicts: list[str]) -> tuple[str, str]:
    """回傳 (strategy, reason)；做不到就 raise Halt。"""
    if behind == 0:
        return "none", "已跟上 base，不用同步"
    if requested == "auto":
        if conflicts:
            return "transit", f"有衝突（{len(conflicts)} 個檔），走中轉分支"
        if not published:
            return "rebase", "還沒推送且無衝突，rebase 保持線性歷史"
        return "merge", "已推送且無衝突，把 base merge 進來（不改寫已推送的歷史）"
    if requested == "rebase":
        if published:
            raise Halt("指定 rebase，但分支已推送：rebase 後要 force push，停下交人")
        if conflicts:
            raise Halt(f"指定 rebase，但有衝突（{'、'.join(conflicts[:10])}），停下交人")
        return "rebase", "指定 rebase；未推送且無衝突"
    if requested == "merge":
        if conflicts:
            raise Halt(f"指定 merge，但有衝突（{'、'.join(conflicts[:10])}），停下交人")
        return "merge", "指定 merge；無衝突"
    return "transit", "指定 transit"


def cmd_plan(a: argparse.Namespace, rec: dict) -> int:
    repo = Path(a.repo).resolve()
    rec.update({"branch": a.branch, "base": a.base, "behind": None, "published": None, "conflicts": [],
                "strategy": None, "requested_strategy": None, "transit_branch": None})
    conv = sync_conventions(repo, a.methodology)
    requested = a.strategy or conv["strategy"]
    rec["requested_strategy"] = requested
    if not a.no_fetch:
        r = git(repo, "fetch", "--quiet", a.remote)
        if r.returncode:
            raise Halt(f"git fetch {a.remote} 失敗：{r.stderr.strip()[-300:]}")
    if not resolve(repo, f"refs/heads/{a.branch}"):
        raise Halt(f"本機沒有分支 {a.branch}")
    if not resolve(repo, a.base):
        raise Halt(f"找不到 base {a.base}")
    behind = must(repo, "rev-list", "--count", f"refs/heads/{a.branch}..{a.base}").strip()
    if not behind.isdigit():
        raise Halt(f"rev-list --count 輸出看不懂：{behind!r}")
    rec["behind"] = int(behind)
    rec["published"] = bool(resolve(repo, f"refs/remotes/{a.remote}/{a.branch}"))
    _, rec["conflicts"] = merge_tree(repo, a.base, f"refs/heads/{a.branch}")
    strategy, reason = decide(requested, rec["behind"], rec["published"], rec["conflicts"])
    if strategy == "transit":
        short = a.short or a.branch.rstrip("/").split("/")[-1]
        rec["transit_branch"] = transit_name(repo, conv["transit_naming"], short, base_name(a.base, a.remote), a.date,
                                             prefixes(repo, a.config))
    rec["strategy"], rec["reason"] = strategy, reason
    return 0


# ------------------------------------------------------------------ apply
def cmd_apply(a: argparse.Namespace, rec: dict) -> int:
    repo = Path(a.repo).resolve()
    rec.update({"strategy": a.strategy, "branch": a.branch, "base": a.base, "head_branch": None, "head_sha": None,
                "conflicts": [], "backup_ref": None})
    cur = git(repo, "symbolic-ref", "--quiet", "--short", "HEAD").stdout.strip()
    if cur != a.branch:
        raise Halt(f"worktree 目前在 {cur or '分離的 HEAD'}，不是 {a.branch}")
    if git(repo, "status", "--porcelain").stdout.strip():
        raise Halt("worktree 不乾淨（有未 commit 或未追蹤的檔案）")
    if not resolve(repo, a.base):
        raise Halt(f"找不到 base {a.base}")

    def finish(code: int, reason: str) -> int:
        rec["head_branch"] = git(repo, "symbolic-ref", "--quiet", "--short", "HEAD").stdout.strip() or None
        rec["head_sha"] = resolve(repo, "HEAD") or None
        rec["reason"] = reason
        return code

    if a.strategy == "rebase":
        if not a.date or not DATE.match(a.date):
            raise Halt("rebase 要 --date YYYYMMDD（備份 ref 用）")
        backup = f"refs/backup/{a.branch.replace('/', '-')}-{a.date}"
        r = git(repo, "update-ref", backup, f"refs/heads/{a.branch}", "")  # 舊值給空字串＝不存在才寫
        if r.returncode:
            raise Halt(f"備份 ref {backup} 寫不進去（可能已存在）：{r.stderr.strip()[-200:]}")
        rec["backup_ref"] = backup
        r = git(repo, "rebase", a.base)
        if r.returncode:
            rec["conflicts"] = unmerged(repo)
            git(repo, "rebase", "--abort")
            return finish(3, f"rebase 失敗，已 abort：{(r.stderr or r.stdout).strip()[-200:]}")
        return finish(0, f"已 rebase 到 {a.base}，原分支備份在 {backup}")
    if a.strategy == "merge":
        r = git(repo, "merge", "--no-ff", "--no-edit", a.base)
        if r.returncode:
            rec["conflicts"] = unmerged(repo)
            git(repo, "merge", "--abort")
            return finish(3, f"merge 失敗，已 abort：{(r.stderr or r.stdout).strip()[-200:]}")
        return finish(0, f"已把 {a.base} merge 進 {a.branch}")
    # transit
    if not a.transit_branch:
        raise Halt("transit 要 --transit-branch（用 plan 輸出的 transit_branch）")
    check_branch_name(repo, a.transit_branch, prefixes(repo, a.config))
    if resolve(repo, f"refs/heads/{a.transit_branch}"):
        raise Halt(f"中轉分支 {a.transit_branch} 已存在")
    must(repo, "switch", "--quiet", "--no-track", "-c", a.transit_branch, a.base)
    r = git(repo, "merge", "--no-ff", "--no-edit", f"refs/heads/{a.branch}")
    if r.returncode:
        rec["conflicts"] = unmerged(repo)
        if rec["conflicts"]:
            return finish(2, "有衝突，已留在中轉分支的 worktree；解完 commit 後跑 check-transit")
        return finish(1, f"merge 失敗但沒有衝突檔，停下交人：{(r.stderr or r.stdout).strip()[-200:]}")
    return finish(0, f"已在 {a.transit_branch} 乾淨合併 {a.branch}")


# ------------------------------------------------------------------ check-transit
def cmd_check_transit(a: argparse.Namespace, rec: dict) -> int:
    repo = Path(a.repo).resolve()
    expected = sorted({x.strip().replace("\\", "/") for x in (a.expected or "").split(",") if x.strip()})
    rec.update({"branch": a.branch, "base": a.base, "merge_commit": None, "expected": expected,
                "marker_files": [], "unexpected_files": [], "reasons": []})
    head = resolve(repo, "HEAD")
    if not head:
        raise Halt("讀不到 HEAD")
    rec["merge_commit"] = head
    for ref in (a.base, f"refs/heads/{a.branch}"):
        if not resolve(repo, ref):
            raise Halt(f"找不到 {ref}")
    parents = must(repo, "rev-list", "--parents", "-n", "1", "HEAD").split()[1:]
    reasons: list[str] = rec["reasons"]
    if len(parents) != 2:
        reasons.append(f"HEAD 不是兩個 parent 的 merge commit（parent 數 {len(parents)}）")
    if not is_ancestor(repo, a.base, "HEAD"):
        reasons.append(f"{a.base} 不是 HEAD 的祖先")
    if not is_ancestor(repo, f"refs/heads/{a.branch}", "HEAD"):
        reasons.append(f"{a.branch} 不是 HEAD 的祖先")
    for f in lines(must(repo, "diff", "--name-only", "--diff-filter=d", a.base, "HEAD")):
        blob = subprocess.run([*GIT, "-C", str(repo), "show", f"HEAD:{f}"], capture_output=True, stdin=subprocess.DEVNULL)
        if blob.returncode or b"\0" in blob.stdout:
            continue  # 二進位檔不掃標記
        if any(MARKER.match(l) for l in blob.stdout.decode("utf-8", "replace").splitlines()):
            rec["marker_files"].append(f)
    if rec["marker_files"]:
        reasons.append(f"還有衝突標記：{'、'.join(rec['marker_files'])}")
    if len(parents) == 2:
        # 整棵樹和自動合併結果比：只看 git show --cc（跟兩個 parent 都不同的檔）會漏掉「把某一邊的改動默默丟掉」
        # （檔案退回成其中一個 parent 的版本），所以用 diff 比對全部檔案，含新增與刪除
        auto_tree, _ = merge_tree(repo, parents[0], parents[1])
        for f in lines(must(repo, "diff", "--name-only", "--no-renames", auto_tree, "HEAD")):
            if f.replace("\\", "/") not in expected:
                rec["unexpected_files"].append(f)
        if rec["unexpected_files"]:
            reasons.append(f"merge commit 改了預期以外的檔案（和自動合併結果不同）：{'、'.join(rec['unexpected_files'])}")
    rec["ok"] = not reasons
    rec["reason"] = "中轉分支檢查通過" if not reasons else "；".join(reasons)
    return 0 if not reasons else 1


# ------------------------------------------------------------------ CLI
def parser() -> argparse.ArgumentParser:
    p = _Parser(description="確定性分支同步工具：讓功能分支跟上 base")
    sub = p.add_subparsers(dest="cmd", required=True, parser_class=_Parser)
    c = sub.add_parser("plan", help="判定同步策略（只讀）")
    c.add_argument("--repo", required=True)
    c.add_argument("--branch", required=True)
    c.add_argument("--base", default="origin/main")
    c.add_argument("--remote", default="origin")
    c.add_argument("--strategy", choices=STRATEGIES, help="預設取 methodology 的 conventions.sync.strategy，沒寫＝auto")
    c.add_argument("--short", help="中轉分支名的 <短名>（預設取分支名最後一段）")
    c.add_argument("--date", help="YYYYMMDD；策略是 transit 時必填")
    c.add_argument("--config", help="guard.config.json（預設 <repo>/.claude/teams/guard.config.json）")
    c.add_argument("--methodology", help="methodology.json（預設 <repo>/.claude/teams/methodology.json）")
    c.add_argument("--no-fetch", action="store_true")
    c = sub.add_parser("apply", help="執行同步")
    c.add_argument("--repo", required=True)
    c.add_argument("--branch", required=True)
    c.add_argument("--strategy", required=True, choices=("rebase", "merge", "transit"))
    c.add_argument("--base", default="origin/main")
    c.add_argument("--transit-branch")
    c.add_argument("--date", help="YYYYMMDD；rebase 的備份 ref 用")
    c.add_argument("--config", help="guard.config.json（檢查中轉分支前綴）")
    c = sub.add_parser("check-transit", help="檢查中轉分支的 merge commit")
    c.add_argument("--repo", required=True)
    c.add_argument("--branch", required=True)
    c.add_argument("--base", required=True)
    c.add_argument("--expected", required=True, help="預期會動的檔案（衝突檔），逗號分隔；沒有就給空字串")
    return p


def execute(argv: list[str]) -> tuple[int, dict]:
    rec: dict = {"halted": False, "reason": ""}
    try:
        a = parser().parse_args(argv)
        rec["command"] = a.cmd
        code = {"plan": cmd_plan, "apply": cmd_apply, "check-transit": cmd_check_transit}[a.cmd](a, rec)
    except Halt as e:
        rec["halted"], rec["reason"] = True, str(e)
        code = 1
    return code, rec


# ------------------------------------------------------------------ selftest
def _selftest() -> int:
    ok = True
    saved = {k: os.environ.get(k) for k in ("GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM")}
    with tempfile.TemporaryDirectory() as d:
        root = Path(d)
        (root / "gitconfig").write_text("", encoding="utf-8")
        os.environ["GIT_CONFIG_GLOBAL"] = str(root / "gitconfig")  # 不受使用者全域設定（簽章、hooksPath）影響
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        origin, repo = root / "origin.git", root / "app"

        def g(*args: str, cwd: Path = repo) -> str:
            r = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, encoding="utf-8")
            if r.returncode:
                raise RuntimeError(f"git {args}：{r.stderr}")
            return r.stdout.strip()

        def commit(path: str, text: str, msg: str) -> None:
            (repo / path).write_text(text, encoding="utf-8")
            g("add", "-A")
            g("commit", "-qm", msg)

        def case(name: str, argv: list[str], want_code: int, **want) -> dict:
            nonlocal ok
            code, rec = execute(argv)
            good = code == want_code and all(rec.get(k) == v for k, v in want.items())
            print(f"  [{'通過' if good else '失敗'}] {name}：exit {code}（預期 {want_code}）"
                  + ("" if good and not os.environ.get("AWF_VERBOSE") else f" {json.dumps(rec, ensure_ascii=False)}"))
            ok = ok and good
            return rec

        def expect(name: str, cond: bool) -> None:
            nonlocal ok
            print(f"  [{'通過' if cond else '失敗'}] {name}")
            ok = ok and cond

        subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "-q", "--bare", str(origin)], check=True)
        subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "-q", str(repo)], check=True)
        for k, v in (("user.name", "t"), ("user.email", "t@example.invalid"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")):
            g("config", k, v)
        g("remote", "add", "origin", str(origin))
        commit("README.md", "# 示範客戶 ACME\n", "init")
        commit("shared.txt", "標題\n內容\n", "shared")
        g("push", "-q", "origin", "main")
        g("branch", "feature/up")
        g("switch", "-q", "-c", "feature/a")
        commit("a.txt", "a\n", "a")
        g("switch", "-q", "-c", "feature/b", "main")
        commit("b.txt", "b\n", "b")
        g("push", "-q", "origin", "feature/b")
        g("switch", "-q", "-c", "feature/c", "main")
        commit("shared.txt", "標題\n功能分支的內容\n", "c")
        g("switch", "-q", "main")
        commit("shared.txt", "標題\nmain 的內容\n", "main 改 shared")
        commit("m.txt", "m\n", "main 新檔")
        g("push", "-q", "origin", "main")
        g("reset", "-q", "--hard", "HEAD~2")  # 本機 main 落後，確認工具只看 origin/main
        R = ["--repo", str(repo)]
        D = ["--date", "20261103"]
        cfg = root / "guard.config.json"
        cfg.write_text(json.dumps({"allowed_branch_prefixes": ["feature/"]}), encoding="utf-8")
        meth = root / "methodology.json"
        meth.write_text(json.dumps({"conventions": {"sync": {"strategy": "merge"}}}), encoding="utf-8")

        print("sync_branch selftest（暫存 git repo，bare repo 當 origin）")
        case("已跟上 → none", ["plan", *R, "--branch", "feature/up", "--base", "main"], 0, strategy="none", behind=0)
        case("未推送、無衝突 → rebase", ["plan", *R, "--branch", "feature/a"], 0, strategy="rebase", published=False, behind=2)
        case("已推送、無衝突 → merge", ["plan", *R, "--branch", "feature/b"], 0, strategy="merge", published=True)
        rec = case("有衝突 → transit", ["plan", *R, "--branch", "feature/c", *D], 0, strategy="transit",
                   transit_branch="merge/c-into-main-20261103", conflicts=["shared.txt"])
        case("methodology 指定 merge → merge", ["plan", *R, "--branch", "feature/a", "--methodology", str(meth)], 0,
             strategy="merge", requested_strategy="merge")
        case("固定 rebase 但已推送 → 停下", ["plan", *R, "--branch", "feature/b", "--strategy", "rebase"], 1, halted=True)
        case("固定 merge 但有衝突 → 停下", ["plan", *R, "--branch", "feature/c", "--strategy", "merge"], 1, halted=True)
        case("transit 前綴不在白名單 → 停下", ["plan", *R, "--branch", "feature/c", *D, "--config", str(cfg)], 1, halted=True)
        case("transit 沒有 --date → 停下", ["plan", *R, "--branch", "feature/c"], 1, halted=True)
        case("fetch 失敗 → 停下", ["plan", *R, "--branch", "feature/a", "--remote", "nope"], 1, halted=True)
        case("參數錯誤 → 停下（exit 1）", ["plan", *R], 1, halted=True)

        g("switch", "-q", "feature/a")
        old_a = g("rev-parse", "feature/a")
        case("apply rebase", ["apply", *R, "--branch", "feature/a", "--strategy", "rebase", *D], 0,
             backup_ref="refs/backup/feature-a-20261103", head_branch="feature/a")
        expect("rebase 留下備份 ref、歷史線性且接在 origin/main 之後",
               g("rev-parse", "refs/backup/feature-a-20261103") == old_a and not g("rev-list", "--merges", "origin/main..feature/a")
               and subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", "origin/main", "feature/a"]).returncode == 0)
        case("同一天再 rebase：備份 ref 已存在 → 停下", ["apply", *R, "--branch", "feature/a", "--strategy", "rebase", *D], 1, halted=True)
        case("apply 時 worktree 不在該分支 → 停下", ["apply", *R, "--branch", "feature/b", "--strategy", "merge"], 1, halted=True)
        g("switch", "-q", "feature/b")
        case("apply merge", ["apply", *R, "--branch", "feature/b", "--strategy", "merge"], 0, head_branch="feature/b")
        expect("merge 後 HEAD 是 merge commit", len(g("rev-list", "--parents", "-n", "1", "HEAD").split()) == 3)

        g("switch", "-q", "feature/c")
        t1 = rec["transit_branch"]
        case("apply transit 有衝突 → exit 2", ["apply", *R, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", t1], 2,
             conflicts=["shared.txt"], head_branch=t1)
        commit("shared.txt", "標題\nmain 的內容\n功能分支的內容\n", "merge: 解衝突")
        CT = ["check-transit", *R, "--branch", "feature/c", "--base", "origin/main"]
        case("解完衝突 commit → check-transit 通過", [*CT, "--expected", "shared.txt"], 0, ok=True)
        case("--expected 沒列衝突檔 → 不通過", [*CT, "--expected", ""], 1, unexpected_files=["shared.txt"])

        def transit_again(t: str) -> None:
            g("switch", "-q", "feature/c")
            code, _ = execute(["apply", *R, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", t])
            if code != 2:
                raise RuntimeError(f"預期 {t} 有衝突")

        transit_again("merge/c-into-main-20261104")
        (repo / "shared.txt").write_text("標題\nmain 的內容\n功能分支的內容\n", encoding="utf-8")
        (repo / "README.md").write_text("# 示範客戶 ACME（夾帶）\n", encoding="utf-8")
        g("add", "-A")
        g("commit", "-qm", "merge: 解衝突＋夾帶")
        case("merge commit 夾帶無關檔案 → 不通過", [*CT, "--expected", "shared.txt"], 1, unexpected_files=["README.md"])

        transit_again("merge/c-into-main-20261106")
        (repo / "shared.txt").write_text("標題\nmain 的內容\n功能分支的內容\n", encoding="utf-8")
        g("rm", "-q", "m.txt")  # 把 main 那邊沒衝突的改動默默丟掉（m.txt 等於功能分支那個 parent，git show --cc 看不到）
        g("add", "-A")
        g("commit", "-qm", "merge: 解衝突＋丟掉 main 的改動")
        case("解衝突時丟掉另一邊沒衝突的改動 → 不通過", [*CT, "--expected", "shared.txt"], 1, unexpected_files=["m.txt"])

        transit_again("merge/c-into-main-20261105")
        g("add", "-A")
        g("commit", "-qm", "merge: 留著衝突標記")
        case("留著衝突標記 → 不通過", [*CT, "--expected", "shared.txt"], 1, marker_files=["shared.txt"])
        case("HEAD 不是 merge commit → 不通過", ["check-transit", *R, "--branch", "feature/a", "--base", "origin/main",
                                               "--expected", ""], 1, ok=False)
        g("switch", "-q", "feature/c")
        case("apply merge 有衝突 → abort、exit 3", ["apply", *R, "--branch", "feature/c", "--strategy", "merge"], 3,
             conflicts=["shared.txt"], head_branch="feature/c")
        case("apply rebase 有衝突 → abort、exit 3", ["apply", *R, "--branch", "feature/c", "--strategy", "rebase", *D], 3,
             conflicts=["shared.txt"], head_branch="feature/c")
        expect("abort 後 worktree 乾淨、分支沒動", not g("status", "--porcelain") and g("rev-parse", "feature/c") == g("rev-parse", "refs/backup/feature-c-20261103"))
    for k, v in saved.items():
        if v is None:
            os.environ.pop(k, None)
        else:
            os.environ[k] = v
    print("sync_branch selftest：" + ("全部通過" if ok else "有失敗"))
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
    code, rec = execute(argv)
    print(json.dumps(rec, ensure_ascii=False, indent=2))
    return code


if __name__ == "__main__":
    sys.exit(main())
