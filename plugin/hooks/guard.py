"""agent-workflow 的守門邏輯（由 awf_hook.py 的 guard-shell、guard-paths 呼叫；規則見專案的 .claude/teams/rules.md）。

設定讀 ${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json（awf_config.py）。沒有設定檔時只套用角色規則。

「角色規則」只套用在模版角色（awf_config.is_team_agent：命名空間在 team_agent_namespaces、角色名以 dev-／tpm- 開頭）。
使用者自己的 agent（例如 .claude/agents/dev-helper.md，agent_type 沒有命名空間）和主 session 套用同一套規則。
主 session 的 git 規則由 main_session_git_rules、GitHub 規則由 main_session_github_rules 開關；
auto_merge 啟用時主 session 一律不能合併、核准、直接 push 受保護分支，上游與禁止的 repo 不論開關一律擋。

shell 守門同時處理 Bash 工具與 PowerShell 工具（tool_name 為 "PowerShell"，指令一樣在 tool_input.command）：
PowerShell 指令先經 ps_segments() 拆段、正規化成和 bash 一樣的「指令 參數…」形式，再套同一套規則。

hook 只從 session 的專案目錄載入，所以 agent 改工作區裡另一個 repo 時，那個 repo 自己的
.claude/hooks/guard-git、guard-paths 不會被 Claude Code 觸發。這裡判斷完工作區的規則後，
**轉呼叫目標 repo（或 worktree）自己的 guard**（有的話；先找 .py，再找 bash 腳本），並把 dev-* 角色對應成 repo 認得的角色名（role_map）。

合併與核准：agent 與主 session 一律不核准（--approve）；合併預設一律擋。guard.config.json 的 auto_merge 啟用時，
只有完整 agent_type 等於 merger_role 的 agent，在 repo 白名單、--match-head-commit、merge-audit.jsonl 的 gate 紀錄、
上游授權人、沒有凍結檔都成立時，才能 gh pr merge（merge_refusal()）。守門設定、稽核紀錄與凍結檔 agent 一律不能寫。
這些只是減速帶，真正的信任邊界在 GitHub 伺服器端（ruleset＋綁定來源 App 的 gate check），見 docs/自動合併信任模型.md。

用法：python3 guard.py shell|paths < hook-input.json（舊名 bash 等同 shell）；exit 2 擋下（stderr 回給 Claude）。
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import awf_config  # noqa: E402

CFG = awf_config.load()
WORKSPACE = awf_config.workspace()
RUNS = CFG["runs_dir"] + "/" if CFG["runs_dir"] else ""
REPOS = CFG["repos_dir"] + "/" if CFG["repos_dir"] else ""
SPEC_ROOTS = awf_config.spec_layout_roots()  # 開發規範 profile 的 spec_layout.root；沒設或讀不到＝[]
SLUG = re.compile(r"^[\w.-]+/[\w.-]+$")


def block(msg: str) -> None:
    print(f"已被 agent-workflow hook 擋下：{msg}（規則：.claude/teams/rules.md；設定：.claude/teams/guard.config.json）", file=sys.stderr)
    sys.exit(2)


def norm(p: str) -> str:
    p = p.replace("\\", "/")
    m = re.match(r"^/([A-Za-z])(/.*)?$", p)
    if m:
        p = m.group(1) + ":" + (m.group(2) or "/")
    return p


def rel_to_workspace(p: Path) -> str | None:
    try:
        return Path(norm(str(p))).resolve().relative_to(WORKSPACE.resolve()).as_posix()
    except ValueError:
        return None


def git_toplevel(start: Path) -> Path | None:
    d = start
    while not d.exists() and d != d.parent:
        d = d.parent
    if d.is_file():
        d = d.parent
    r = subprocess.run(["git", "-C", str(d), "rev-parse", "--show-toplevel"], capture_output=True, text=True, encoding="utf-8", errors="replace")
    return Path(r.stdout.strip()) if r.returncode == 0 and r.stdout.strip() else None


GIT_GLOBAL_OPTS = re.compile(r"\bgit\s+(?:(?:-C|-c)\s+(?:\"[^\"]*\"|'[^']*'|\S+)\s+|--(?:git-dir|work-tree|namespace)=\S+\s+|--no-pager\s+|--bare\s+)+")


def forward(top: Path | None, guard: str, data: dict) -> None:
    """轉呼叫目標 repo 自己的 guard；它擋（exit 2）我們就擋。工作區本身的 hooks 不轉呼叫（避免重複）。"""
    if not top or top.resolve() == WORKSPACE.resolve():
        return
    script = find_repo_guard(top, guard)
    if script is None:
        # worktree 從沒有 guard 的分支開出時，改用主 repo 的 guard（規則一樣適用）
        r = subprocess.run(["git", "-C", str(top), "rev-parse", "--path-format=absolute", "--git-common-dir"], capture_output=True, text=True, encoding="utf-8", errors="replace")
        main = Path(r.stdout.strip()).parent if r.returncode == 0 else None
        script = find_repo_guard(main, guard) if main else None
        if script is None:
            return
    fwd = json.loads(json.dumps(data))
    ti = fwd.get("tool_input") or {}
    if "command" in ti:
        # repo 的 guard 常比對「git commit」這種緊連寫法；`git -C <dir> commit` 會漏掉。目錄已解析成 top，轉呼叫前拿掉全域選項。
        ti["command"] = GIT_GLOBAL_OPTS.sub("git ", ti["command"])
    agent = awf_config.team_role(data.get("agent_type") or "", CFG)
    if agent in CFG["role_map"]:
        fwd["agent_type"] = CFG["role_map"][agent]
    elif agent:
        fwd.pop("agent_type", None)  # 沒有對應的角色，以主 session 身分套用 repo 規則
    env = dict(os.environ, CLAUDE_PROJECT_DIR=str(top), PYTHONUTF8="1")
    if script.suffix == ".py":
        argv = [sys.executable, str(script)]
    else:
        bash = find_bash()
        if not bash:
            block(f"{top.name} 有自己的 {script.name}，但找不到 bash 來執行它（Windows 請安裝 Git for Windows，"
                  f"或設 CLAUDE_CODE_GIT_BASH_PATH；也可以把 repo 的 guard 改寫成 {guard}.py）")
        argv = [bash, str(script)]
    r = subprocess.run(argv, input=json.dumps(fwd), capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
    if r.returncode == 2:
        print(f"{r.stderr.strip()}（由 agent-workflow hook 轉呼叫 {top.name} 的 {script.name}）", file=sys.stderr)
        sys.exit(2)


def find_repo_guard(top: Path, guard: str) -> Path | None:
    """repo 自己的 guard：先找 <guard>.py（跨平台），再找 bash 腳本 <guard>.sh。"""
    for ext in (".py", ".sh"):
        p = top / ".claude" / "hooks" / f"{guard}{ext}"
        if p.is_file():
            return p
    return None


def find_bash() -> str | None:
    """找可以跑 bash 腳本的 bash。Windows 上避開 System32 的 WSL bash.exe，優先用 Git for Windows 的。"""
    if os.name != "nt":
        return shutil.which("bash")
    env = os.environ.get("CLAUDE_CODE_GIT_BASH_PATH")
    if env and Path(env).is_file():
        return env
    git = shutil.which("git")
    if git:
        root = Path(git).resolve().parent.parent  # <Git>/cmd/git.exe 或 <Git>/bin/git.exe
        for cand in (root / "bin" / "bash.exe", root / "usr" / "bin" / "bash.exe"):
            if cand.is_file():
                return str(cand)
    b = shutil.which("bash")
    if b and "system32" not in b.lower():
        return b
    return None


# ---------------------------------------------------------------- 禁止路徑與 repo
def forbidden_entries() -> tuple[list[str], list[str]]:
    """forbidden_paths 的每一項：工作區相對路徑或絕對路徑；長得像 owner/repo 且不是現有路徑的，也當成遠端 repo。"""
    paths, slugs = [], []
    for e in CFG["forbidden_paths"]:
        e = e.rstrip("/")
        if not e:
            continue
        p = Path(e) if os.path.isabs(e) else WORKSPACE / e
        if SLUG.match(e) and not p.exists():
            slugs.append(e)
        paths.append(e)
    return paths, slugs + list(CFG["upstream_repos"])


def under_forbidden(path: Path) -> str | None:
    rel = rel_to_workspace(path)
    absp = Path(norm(str(path))).resolve().as_posix() if path.is_absolute() else None
    for e in forbidden_entries()[0]:
        if os.path.isabs(e):
            target = Path(e).resolve().as_posix()
            if absp and (absp == target or absp.startswith(target + "/")):
                return e
        elif rel is not None and (rel == e or rel.startswith(e + "/")):
            return e
    return None


# ---------------------------------------------------------------- Bash
GH_WRITE = re.compile(
    r"\bgh\s+(?:"
    r"pr\s+(?:create|comment|review|merge|edit|close|reopen|ready|lock|unlock)|"
    r"issue\s+(?:create|comment|edit|close|reopen|delete|lock|transfer)|"
    r"release\s+(?:create|edit|delete|upload)|"
    r"repo\s+(?:edit|delete|rename|archive|sync|create|fork)|"
    r"workflow\s+(?:run|enable|disable)|run\s+(?:rerun|cancel|delete)|"
    r"label\s+(?:create|edit|delete)|secret\s+(?:set|delete)|variable\s+(?:set|delete))\b"
)
# 段落已依引號外的運算子切好，所以 lookahead 可以看整段（引號裡 URL 的 & 不會截斷判斷）
GH_API_WRITE = re.compile(r"\bgh(?:\.exe)?\s+api\b(?=.*(?:-X\s*|--method[\s=]+)[\"']?(?:POST|PATCH|PUT|DELETE)|.*\s(?:-f|-F|--field|--raw-field|--input)[\s=])", re.I)
DESTRUCTIVE = re.compile(r"\b(rm|mv|cp|sed\s+-i|git\s+(commit|checkout|reset|merge|pull|stash|clean)|tee|>)\b")


def segments(cmd: str) -> list[str]:
    """bash 指令依引號外的 ; & && | || 換行切段。引號（含字串裡的 $(…)）內的運算子不切，
    2>&1、&> 這類重新導向的 & 也不切。"""
    out: list[str] = []
    cur: list[str] = []
    q = ""
    i, n = 0, len(cmd)
    while i < n:
        c = cmd[i]
        if q == "'":
            cur.append(c)
            q = "" if c == "'" else q
            i += 1
            continue
        if c == "\\" and i + 1 < n:
            cur.append(cmd[i : i + 2])
            i += 2
            continue
        if q == '"':
            cur.append(c)
            q = "" if c == '"' else q
            i += 1
            continue
        if c in "'\"":
            q = c
            cur.append(c)
            i += 1
            continue
        if c == "&" and ((cur and cur[-1] == ">") or cmd.startswith(">", i + 1)):
            cur.append(c)
            i += 1
            continue
        if c in ";&|\n":
            out.append("".join(cur))
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    out.append("".join(cur))
    return [x.strip() for x in out if x.strip()]


# ---------------------------------------------------------------- GitHub 合併／核准／REST 寫入
# 合併與核准各自是一個判斷函式；SP6 的 auto_merge 例外只掛在 merge_exception()，不要把條件散進 check_shell。
GH_PR_MERGE = re.compile(r"\bgh(?:\.exe)?\s+pr\s+merge\b", re.I)
GH_PR_REVIEW = re.compile(r"\bgh(?:\.exe)?\s+pr\s+review\b", re.I)
# 會直接打 API 的指令：gh api、curl、wget、httpie（http／https／xh）、PowerShell 的 Invoke-RestMethod／Invoke-WebRequest（irm／iwr）
API_CLIENT = re.compile(r"(?:^|[\s/\\(&;|`])(?:gh(?:\.exe)?\s+api|curl(?:\.exe)?|wget(?:\.exe)?|https?|xh|invoke-restmethod|irm|invoke-webrequest|iwr)(?=\s|$)", re.I)
# REST：PUT /repos/{o}/{r}/pulls/{n}/merge、POST /repos/{o}/{r}/merges（直接合併分支）；GraphQL：mergePullRequest、enablePullRequestAutoMerge、mergeBranch
MERGE_TARGET = re.compile(r"/pulls/[^/\s\"']+/merge(?![\w-])|\brepos/[^/\s]+/[^/\s]+/merges\b|\bmergePullRequest\b|\benablePullRequestAutoMerge\b|\bmergeBranch\b", re.I)
REVIEW_TARGET = re.compile(r"/pulls/[^/\s\"']+/reviews\b|\baddPullRequestReview\b|\bsubmitPullRequestReview\b", re.I)
APPROVE_WORD = re.compile(r"\bAPPROVE\b", re.I)
APPROVE_FLAG = re.compile(r"(?:^|\s)(?:--approve(?:=\S*)?|-[A-Za-z]*a[A-Za-z]*)(?=\s|$)")
GITHUB_API_HOST = re.compile(r"\bapi\.github\.com\b|/api/v3/|/api/graphql\b", re.I)
WEB_CLIENT = re.compile(r"(?:^|[\s/\\])(curl|wget|https?|xh|invoke-restmethod|irm|invoke-webrequest|iwr)(?:\.exe)?(?=\s|$)", re.I)
CURL_WRITE = re.compile(r"(?:^|\s)(?:-X\s*[\"']?(?:POST|PUT|PATCH|DELETE)|--request[\s=]+[\"']?(?:POST|PUT|PATCH|DELETE)|-d|--data(?:-\w+)?|--json|-F|--form(?:-string)?|-T|--upload-file|--post-data|--post-file|--method[\s=]+[\"']?(?:POST|PUT|PATCH|DELETE))(?=[\s=]|$)")
PS_WEB_WRITE = re.compile(r"(?:^|\s)(?:-Method\s+[\"']?(?:Post|Put|Patch|Delete)|-Body|-InFile|-Form)(?=[\s:]|$)", re.I)
HTTPIE_WRITE = re.compile(r"(?:^|\s)(?:POST|PUT|PATCH|DELETE)\s|\s[\w.-]+:?=\S")


def is_merge_cmd(text: str) -> bool:
    """合併 PR：gh pr merge（含 --auto），或用 API 打 merge 端點（REST／GraphQL；任何 method 寫法、旗標順序）。
    text 可以是一段或整條指令；傳整條時，用變數拼 URL（U=…/pulls/3/merge; gh api $U -X PUT）也抓得到。"""
    if GH_PR_MERGE.search(text):
        return True
    return bool(API_CLIENT.search(text) and MERGE_TARGET.search(text))


def is_approve_cmd(seg: str) -> bool:
    """核准 PR：gh pr review --approve／-a（含 -ab 這類合併短旗標），或用 API 送出 APPROVE 審查。
    reviews 端點用 --input 或 @檔案 帶內容時看不到內容，一律當成核准（fail-closed）。"""
    m = GH_PR_REVIEW.search(seg)
    if m and APPROVE_FLAG.search(seg[m.end():]):
        return True
    if API_CLIENT.search(seg) and REVIEW_TARGET.search(seg):
        return bool(APPROVE_WORD.search(seg) or re.search(r"(?:^|\s)--input[\s=]|=@\S|\s@\S", seg))
    return False


def is_rest_write(seg: str) -> bool:
    """不經 gh 直接打 GitHub API 的寫入：curl／wget／httpie／Invoke-RestMethod／Invoke-WebRequest 對 api.github.com
    （或 GHES 的 /api/v3、/api/graphql）送 POST／PUT／PATCH／DELETE 或帶 body。gh api 的寫入由 GH_API_WRITE 管。"""
    if not GITHUB_API_HOST.search(seg):
        return False
    m = WEB_CLIENT.search(seg)
    if not m:
        return False
    name, rest = m.group(1).lower(), seg[m.end():]
    if name in ("invoke-restmethod", "irm", "invoke-webrequest", "iwr"):
        return bool(PS_WEB_WRITE.search(rest))
    if name in ("http", "https", "xh"):
        return bool(HTTPIE_WRITE.search(rest))
    # curl 在 Windows PowerShell 5 是 Invoke-WebRequest 的別名，兩種旗標都看
    return bool(CURL_WRITE.search(rest) or PS_WEB_WRITE.search(rest))


# ---------------------------------------------------------------- auto_merge 的有條件開放（SP6）
# 信任邊界在 GitHub 伺服器端（ruleset＋綁定來源 App 的 gate check）；這裡是減速帶，見 docs/自動合併信任模型.md。
AM = awf_config.auto_merge(CFG)
SHA40 = re.compile(r"^[0-9a-f]{40}$")
PR_URL = re.compile(r"^https?://[^/]+/([\w.-]+/[\w.-]+)/pull/(\d+)(?:[/?#].*)?$")
SUBST = re.compile(r"\$\(|`|<\(|\$\{")  # 指令替換、程序替換、變數展開：看不到實際值，一律不放行
VALUE_FLAGS = ("--repo", "-R", "--match-head-commit", "-b", "--body", "-F", "--body-file", "-t", "--subject", "--author-email")


def _flag(toks: list[str], *names: str) -> str | None:
    """取旗標值：--repo x、--repo=x、-R x。沒有回 None。"""
    for i, t in enumerate(toks):
        for n in names:
            if t == n and i + 1 < len(toks):
                return toks[i + 1]
            if n.startswith("--") and t.startswith(n + "="):
                return t.split("=", 1)[1]
    return None


def _gh_pr(seg: str, sub: str) -> tuple[str, int | None, list[str]] | None:
    """解析 gh pr <sub> <編號或 URL> --repo <owner/repo> …，回傳 (repo, pr, 參數)；不是這個子指令回 None。"""
    toks = tokens(seg)
    names = [_cmd_name(t) for t in toks]
    for i in range(len(toks) - 2):
        if names[i] == "gh" and toks[i + 1] == "pr" and toks[i + 2] == sub:
            args = toks[i + 3:]
            repo = _flag(args, "--repo", "-R") or ""
            pr = None
            pos = [a for k, a in enumerate(args) if not a.startswith("-") and (k == 0 or args[k - 1] not in VALUE_FLAGS)]
            if pos:
                m = PR_URL.match(pos[0])
                if m:
                    if repo and repo != m.group(1):
                        return repo, None, args
                    repo, pr = m.group(1), int(m.group(2))
                elif pos[0].isdigit():
                    pr = int(pos[0])
            return repo, pr, args
    return None


def is_steward(data: dict) -> bool:
    """完整 agent_type（含命名空間）等於 auto_merge.merger_role，且是模版角色；沒有命名空間的同名 agent 不算。"""
    a = data.get("agent_type") or ""
    return bool(AM["merger_role"]) and a == AM["merger_role"] and awf_config.is_team_agent(a, CFG)


def freeze_path() -> Path:
    f = Path(norm(AM["freeze_file"]))
    return f if f.is_absolute() else WORKSPACE / f


def frozen() -> str | None:
    """凍結旗標（event-post-merge 寫的 {"frozen": true, "reason": …}）。檔案存在且 frozen 不是 false 就算凍結；
    讀不到或不是 JSON 也算凍結（fail-closed）。回傳理由，沒凍結回 None。"""
    f = freeze_path()
    if not f.exists():
        return None
    try:
        data = json.loads(f.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return f"{AM['freeze_file']} 存在但讀不懂（視為凍結）"
    if isinstance(data, dict) and data.get("frozen") is False:
        return None
    reason = data.get("reason") if isinstance(data, dict) else ""
    return f"{AM['freeze_file']}：{reason or 'frozen'}"


METHOD_FLAGS = {"squash": ("--squash", "-s"), "merge": ("--merge", "-m"), "rebase": ("--rebase", "-r")}


def upstream_auth(repo: str) -> str | None:
    """上游 repo 的授權人；沒有、空白或已過期回 None。值可以是字串，或 {"by", "link", "scope", "expires": "YYYY-MM-DD"}。"""
    v = AM["upstream_authorized_by"].get(repo)
    if isinstance(v, dict):
        exp = str(v.get("expires") or "").strip()
        if exp:
            from datetime import date
            try:
                if date.fromisoformat(exp[:10]) < date.today():
                    return None
            except ValueError:
                return None
        v = v.get("by")
    v = str(v or "").strip()
    return v or None


def steward_repo_problem(repo: str) -> str | None:
    """steward 對這個 repo 寫入的共同條件：白名單、禁止 repo、上游要有授權人。"""
    if not repo or not SLUG.match(repo):
        return "要明確寫 --repo <owner>/<repo>"
    if repo not in AM["repos"]:
        return f"{repo} 不在 auto_merge.repos 白名單"
    if repo in CFG["upstream_repos"] and not upstream_auth(repo):
        return f"{repo} 是上游 repo，auto_merge.upstream_authorized_by 沒有（有效的）授權人"
    other = [s for s in forbidden_entries()[1] if s not in CFG["upstream_repos"]]
    if repo in other:
        return f"{repo} 在 forbidden_paths"
    return None


def last_audit(repo: str, pr: int, sha: str) -> dict | None:
    """<runs_dir>/merge-audit.jsonl 裡同 repo、PR、head_sha 的最後一筆（merge_gate.py 寫的）。"""
    path = WORKSPACE / (CFG["runs_dir"] or "_tpm/runs") / "merge-audit.jsonl"
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    found = None
    for line in lines:
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(rec, dict):
            continue
        try:
            same_pr = int(rec.get("pr")) == pr
        except (TypeError, ValueError):
            same_pr = False
        if rec.get("repo") == repo and same_pr and str(rec.get("head_sha") or "").lower() == sha:
            found = rec
    return found


def merge_refusal(seg: str, data: dict) -> str | None:
    """auto_merge 的合併條件；全部成立回 None，否則回第一個不成立的理由。只處理 gh pr merge，API 合併一律不放行。"""
    if not AM["enabled"]:
        return "auto_merge 未啟用"
    if not is_steward(data):
        return f"只有 {AM['merger_role'] or 'merger_role'}（完整 agent_type）能合併"
    if SUBST.search(seg):
        return "合併指令不能含指令替換或變數展開"
    if API_CLIENT.search(seg) or not GH_PR_MERGE.search(seg):
        return "只接受 gh pr merge，不接受 API 合併"
    parsed = _gh_pr(seg, "merge")
    if not parsed:
        return "看不懂這個 gh pr merge"
    repo, pr, args = parsed
    if pr is None:
        return "要寫 PR 編號（或同一個 repo 的 PR URL）"
    if "--admin" in args:
        return "不能用 --admin 繞過保護規則"
    bad = steward_repo_problem(repo)
    if bad:
        return bad
    sha = (_flag(args, "--match-head-commit") or "").lower()
    if not SHA40.match(sha):
        return "要帶 --match-head-commit <40 字元 head SHA>"
    rec = last_audit(repo, pr, sha)
    if rec is None:
        return f"merge-audit.jsonl 沒有 {repo}#{pr} @ {sha[:12]} 的 gate 紀錄（先跑 merge_gate.py check）"
    if rec.get("decision") != "merge":
        return f"{repo}#{pr} @ {sha[:12]} 最後一次 gate 判定是 {rec.get('decision')}，不是 merge"
    want = METHOD_FLAGS.get(AM["merge_method"], ("--" + AM["merge_method"],))
    used = [m for m, flags in METHOD_FLAGS.items() if any(a in flags for a in args)]
    if used != [AM["merge_method"]]:
        return f"合併方式要明確寫 {want[0]}（auto_merge.merge_method={AM['merge_method']}）"
    fz = frozen()
    if fz:
        return f"自動合併已凍結（{fz}；解凍由人處理）"
    return None


def merge_exception(seg: str, agent: str, data: dict) -> bool:
    """合併的例外放行點：merge_refusal() 的條件全部成立才放行（agent 參數保留給舊呼叫，比對用 data 的完整 agent_type）。
    核准（is_approve_cmd）與直接 REST 寫入（is_rest_write）不走這個例外。"""
    return merge_refusal(seg, data) is None


STEWARD_WRITES = (("review", ("--request-changes", "-r")), ("comment", ()), ("ready", ("--undo",)))


def steward_write_refusal(seg: str, data: dict) -> str | None:
    """steward 在白名單 repo 上可以做的非合併寫入：gh pr review --request-changes、gh pr comment、gh pr ready --undo。
    都成立回 None；不是這三種或條件不成立回理由。"""
    if not (AM["enabled"] and is_steward(data)):
        return "不是啟用中的 merger_role"
    if SUBST.search(seg) or API_CLIENT.search(seg):
        return "不能含指令替換、變數展開或 API 呼叫"
    for sub, need in STEWARD_WRITES:
        parsed = _gh_pr(seg, sub)
        if not parsed:
            continue
        repo, pr, args = parsed
        if need and not any(a in need for a in args):
            return f"gh pr {sub} 只允許 {'／'.join(need)}"
        if sub == "review" and any(a in ("--approve", "-a", "--comment", "-c") or a.startswith("--approve=") for a in args):
            return "gh pr review 只允許 --request-changes"
        if sub == "comment" and any(a.startswith(("--delete-last", "--edit-last")) for a in args):
            return "gh pr comment 不能改或刪既有留言"
        if pr is None:
            return "要寫 PR 編號"
        return steward_repo_problem(repo)
    return "不是 steward 允許的寫入（只有 gh pr merge、review --request-changes、comment、ready --undo）"


# ---------------------------------------------------------------- 守門設定、gate 稽核紀錄、凍結檔：agent 一律不能寫
SHELL_WRITE = re.compile(
    r"(?<![&<>=-])>(?!&)|\b(?:rm|mv|cp|tee|truncate|dd|install|ln|touch|chmod|sed\s+-i|perl\s+-p?i)\b"
    r"|\bgit\s+(?:checkout|restore|rm|mv|stash|reset)\b|open\([^)]*[\"'][wax+]|write_text|write_bytes|writelines|\.write\(|unlink|os\.remove|shutil\."
    r"|set-content|add-content|out-file|new-item|remove-item|clear-content|copy-item|move-item|rename-item", re.I)


AUDIT_FILES = ("merge-audit.jsonl", "publish-audit.jsonl")  # merge_gate.py、publish.py 寫的稽核紀錄


def protected_target(text: str, is_path: bool) -> str | None:
    """回傳碰到的受保護檔：guard.config.json（任何位置的 .claude/teams/ 底下）、稽核紀錄（AUDIT_FILES）、凍結檔（auto_merge.freeze_file）。"""
    t = norm(text)
    freeze = AM["freeze_file"]
    freeze_name = freeze.rsplit("/", 1)[-1]
    if is_path:
        rel = rel_to_workspace(Path(t)) or t
        base = rel.rsplit("/", 1)[-1]
        if rel.endswith(".claude/teams/guard.config.json"):
            return "guard.config.json"
        if base in AUDIT_FILES:
            return base
        if rel == freeze or base == freeze_name:
            return freeze
        return None
    for n in ("guard.config.json", *AUDIT_FILES, freeze_name):
        if n and n in t:
            return freeze if n == freeze_name else n
    return None


def shell_writes_protected(seg: str) -> str | None:
    """Bash／PowerShell 段落是否寫入受保護檔（重新導向、tee、cp、sed -i、Set-Content、python 寫檔…）。盡量擋，不是完備。"""
    hit = protected_target(seg, False)
    if not hit:
        return None
    probe = re.sub(r"\d?>>?\s*/dev/null|\d?>>?\s*\$null|\d>&\d", " ", seg, flags=re.I)
    return hit if SHELL_WRITE.search(probe) else None


def push_target_branches(seg: str, d: Path) -> list[str]:
    """git push 會更新的遠端分支名（去掉 + 與 refs/heads/）。--all／--mirror／--branches 回傳 ["*"]（涵蓋所有分支）。
    沒寫 refspec 時用 @{push}，再退回目前分支；HEAD 換成目前分支名。"""
    toks = tokens(seg)
    if "push" not in toks:
        return []
    args = toks[toks.index("push") + 1:]
    if any(a in ("--all", "--mirror", "--branches") for a in args):
        return ["*"]
    pos, k = [], 0
    while k < len(args):
        a = args[k]
        if a in ("--repo", "-o", "--push-option", "--receive-pack", "--exec"):
            k += 2
            continue
        if not a.startswith("-"):
            pos.append(a)
        k += 1

    def git_out(*a: str) -> str:
        r = subprocess.run(["git", "-C", str(d), *a], capture_output=True, text=True, encoding="utf-8", errors="replace")
        return r.stdout.strip() if r.returncode == 0 else ""

    refspecs = pos[1:]
    if not refspecs:
        up = git_out("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}")
        cur = up.split("/", 1)[1] if "/" in up else git_out("rev-parse", "--abbrev-ref", "HEAD")
        return [cur] if cur and cur != "HEAD" else []
    out = []
    for r in refspecs:
        r = r.lstrip("+")
        dst = r.split(":", 1)[1] if ":" in r else r
        if dst in ("HEAD", "@"):
            dst = git_out("rev-parse", "--abbrev-ref", "HEAD")
        dst = re.sub(r"^refs/heads/", "", dst)
        if dst:
            out.append(dst)
    return out


def pushes_protected(seg: str, d: Path) -> str | None:
    """push 目標含 protected_branches（預設 main、master）時回傳分支名，否則 None。"""
    protected = set(CFG.get("protected_branches") or [])
    for b in push_target_branches(seg, d):
        if b == "*" and protected:
            return "、".join(sorted(protected))
        if b in protected:
            return b
    return None


# ---------------------------------------------------------------- PowerShell 指令解析
# PowerShell 的引號：'…'（'' 跳脫）、"…"（`x 或 "" 跳脫，內含 $(…) 會執行）；也接受 Unicode 彎引號。
PS_SQ = "'\u2018\u2019\u201a\u201b"
PS_DQ = '"\u201c\u201d\u201e'
# 這些字元在引號外就是段落邊界：; 換行 | & 以及 ( ) { }（子運算式、script block 裡的指令一樣會執行）
PS_BREAK = set(";\n|&(){}")
PS_ALIASES = {
    "cd": ("set-location", "sl", "chdir", "cd", "push-location", "pushd"),
    "rm": ("remove-item", "ri", "del", "erase", "rd", "rmdir", "rm"),
    "mv": ("move-item", "mi", "move", "mv"),
    "cp": ("copy-item", "cpi", "copy", "cp"),
    "tee": ("set-content", "sc", "add-content", "ac", "out-file", "new-item", "ni", "clear-content", "clc", "tee-object", "tee"),
}
PS_ALIAS = {a: canon for canon, names in PS_ALIASES.items() for a in names}
PS_PATH_FLAGS = {"-path", "-literalpath", "-lp", "-destination"}
PS_NESTED_SHELLS = {"powershell", "pwsh"}


def _ps_tokenize(cmd: str) -> list[list[str]]:
    """把 PowerShell 指令拆成「段落 → token」，去掉引號與跳脫；雙引號裡的 $(…) 另外拆成段落。"""
    segs: list[list[str]] = []
    nested: list[str] = []  # 雙引號字串裡 $(…) 的內容，之後遞迴解析
    toks: list[str] = []
    cur: list[str] = []
    has_tok = False
    i, n = 0, len(cmd)

    def end_tok():
        nonlocal cur, has_tok
        if has_tok:
            toks.append("".join(cur))
        cur, has_tok = [], False

    def end_seg():
        nonlocal toks
        end_tok()
        if toks:
            segs.append(toks)
        toks = []

    while i < n:
        c = cmd[i]
        if c == "`":  # 跳脫字元；行尾的 ` 是續行
            if cmd.startswith("\r\n", i + 1):
                i += 3
                end_tok()
                continue
            if i + 1 < n and cmd[i + 1] in "\r\n":
                i += 2
                end_tok()
                continue
            if i + 1 < n:
                cur.append(cmd[i + 1])
                has_tok = True
            i += 2
            continue
        if c in PS_SQ:
            has_tok = True
            i += 1
            while i < n:
                if cmd[i] in PS_SQ:
                    if i + 1 < n and cmd[i + 1] in PS_SQ:
                        cur.append("'")
                        i += 2
                        continue
                    i += 1
                    break
                cur.append(cmd[i])
                i += 1
            continue
        if c in PS_DQ:
            has_tok = True
            i += 1
            raw: list[str] = []
            while i < n:
                ch = cmd[i]
                if ch == "`" and i + 1 < n:
                    raw.append(cmd[i + 1])
                    i += 2
                    continue
                if ch in PS_DQ:
                    if i + 1 < n and cmd[i + 1] in PS_DQ:
                        raw.append('"')
                        i += 2
                        continue
                    i += 1
                    break
                raw.append(ch)
                i += 1
            text = "".join(raw)
            cur.append(text)
            nested.extend(_ps_subexpressions(text))
            continue
        if c == "&" and cur and cur[-1].endswith(">"):  # 2>&1、*>&1 這類重新導向，不是運算子
            cur.append(c)
            i += 1
            continue
        if c in PS_BREAK or c == "\r":
            end_seg()
            i += 1
            continue
        if c.isspace() or c == ",":  # 逗號是陣列元素分隔（-ArgumentList 'pr','merge'）
            end_tok()
            i += 1
            continue
        cur.append(c)
        has_tok = True
        i += 1
    end_seg()
    for inner in nested:
        segs.extend(_ps_tokenize(inner))
    return segs


def _ps_subexpressions(text: str) -> list[str]:
    """取出字串裡 $( … ) 的內容（會被 PowerShell 執行），支援巢狀括號。"""
    out, i = [], 0
    while True:
        i = text.find("$(", i)
        if i < 0:
            return out
        depth, j = 1, i + 2
        while j < len(text) and depth:
            depth += {"(": 1, ")": -1}.get(text[j], 0)
            j += 1
        out.append(text[i + 2 : j - 1] if depth == 0 else text[i + 2 :])
        i = j


def _cmd_name(tok: str) -> str:
    """C:\\Program Files\\Git\\cmd\\git.exe → git；其他指令名轉小寫（PowerShell 指令名不分大小寫）。"""
    base = re.split(r"[\\/]", tok)[-1].lower()
    return re.sub(r"\.(exe|cmd|bat|com|ps1)$", "", base)


def _ps_normalize(toks: list[str], depth: int) -> list[list[str]]:
    """把一段 PowerShell token 正規化成 bash 風格的 token；巢狀的 shell 字串會展開成多段。"""
    while toks and toks[0] in (".", "&"):  # 呼叫運算子、dot-source
        toks = toks[1:]
    if not toks:
        return []
    name = _cmd_name(toks[0])
    rest = toks[1:]
    if name in ("git", "gh"):
        return [[name, *rest]]
    if name in PS_ALIAS:
        canon = PS_ALIAS[name]
        return [[canon, *[t for t in rest if t.lower() not in PS_PATH_FLAGS]]]
    if depth < 4 and name in ("invoke-expression", "iex"):
        return [s for t in rest if not t.startswith("-") for s in _ps_parse(t, depth + 1)]
    if depth < 4 and name in ("start-process", "saps", "start"):
        exe, args, k = None, [], 0
        while k < len(rest):
            t, low = rest[k], rest[k].lower()
            if low in ("-filepath", "-file") and k + 1 < len(rest):
                exe, k = rest[k + 1], k + 2
                continue
            if low in ("-argumentlist", "-args") and k + 1 < len(rest):
                k += 1
                while k < len(rest) and not rest[k].startswith("-"):
                    args.extend(rest[k].split())
                    k += 1
                continue
            if t.startswith("-"):
                k += 2 if low in ("-workingdirectory", "-verb", "-windowstyle", "-redirectstandardoutput", "-redirectstandarderror", "-redirectstandardinput") else 1
                continue
            if exe is None:
                exe = t
            else:
                args.extend(t.split())
            k += 1
        return _ps_normalize([exe, *args], depth + 1) if exe else []
    if depth < 4 and name in PS_NESTED_SHELLS:
        for k, t in enumerate(rest):
            if t.lower() in ("-command", "-c") and k + 1 < len(rest):
                return _ps_parse(" ".join(rest[k + 1 :]), depth + 1)
        return [[name, *rest]]
    if depth < 4 and name == "cmd":
        for k, t in enumerate(rest):
            if t.lower() in ("/c", "/k") and k + 1 < len(rest):
                return _ps_parse(" ".join(rest[k + 1 :]), depth + 1)
        return [[name, *rest]]
    if depth < 4 and name in ("bash", "sh", "wsl") and "-c" in rest:
        k = rest.index("-c")
        if k + 1 < len(rest):
            return [shlex_tokens(s) for s in segments(" ".join(rest[k + 1 :]))]
    return [[toks[0], *rest]]


def shlex_tokens(seg: str) -> list[str]:
    try:
        return shlex.split(seg)
    except ValueError:
        return seg.split()


def _ps_parse(cmd: str, depth: int = 0) -> list[list[str]]:
    return [norm for toks in _ps_tokenize(cmd) for norm in _ps_normalize(toks, depth)]


def _join(toks: list[str]) -> str:
    """組回 bash 風格字串，讓後面的 shlex 拆得回同樣的 token。"""
    return " ".join(shlex.quote(t) if (not t or re.search(r"[\s'\"\\;&|]", t)) else t for t in toks)


def ps_segments(cmd: str) -> list[str]:
    """PowerShell 指令 → 正規化後的段落（git／gh 指令名去路徑與 .exe、cmdlet 別名換成 cd／rm／mv／cp／tee）。
    Windows 路徑的反斜線一律換成 /，禁止路徑等比對才對得上（repos\\x 與 repos/x 視為同一個）。"""
    return [_join([x.replace("\\", "/") for x in t]) for t in _ps_parse(cmd) if t]


def effective_dir(cmd: str, cwd: Path) -> Path:
    d = cwd
    m = re.search(r"(?:^|&&|;)\s*cd\s+(\"[^\"]+\"|'[^']+'|\S+)", cmd)
    if m:
        d = (cwd / m.group(1).strip("\"'")).resolve()
    m = re.search(r"\bgit\s+-C\s+(\"[^\"]+\"|'[^']+'|\S+)", cmd)
    if m:
        d = (cwd / m.group(1).strip("\"'")).resolve()
    return d


def tokens(seg: str) -> list[str]:
    seg = GIT_GLOBAL_OPTS.sub("git ", seg)
    try:
        return shlex.split(seg)
    except ValueError:
        return seg.split()


def push_target_urls(seg: str, d: Path) -> list[str]:
    toks = tokens(seg)
    if "push" not in toks:
        return []
    args = toks[toks.index("push") + 1:]
    remote = next((a for a in args if not a.startswith("-")), None)
    if remote is None:
        r = subprocess.run(["git", "-C", str(d), "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}"], capture_output=True, text=True, encoding="utf-8", errors="replace")
        remote = r.stdout.strip().split("/")[0] if r.returncode == 0 else ""
    if not remote:
        return [""]
    if "://" in remote or "@" in remote:
        return [remote]
    r = subprocess.run(["git", "-C", str(d), "remote", "get-url", "--all", "--push", remote], capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.stdout.split() if r.returncode == 0 else [remote]


def is_force_push(seg: str) -> bool:
    toks = tokens(seg)
    if "push" not in toks:
        return False
    args = toks[toks.index("push") + 1:]
    return any(a in ("-f", "--force") or a.startswith("--force-with-lease") or a.startswith("--force-if-includes")
               or (re.match(r"^-[a-zA-Z]*f[a-zA-Z]*$", a) is not None) or (a.startswith("+") and len(a) > 1) for a in args)


def new_branches(seg: str) -> list[str]:
    """這個指令要建立的分支名（checkout -b、switch -c、branch <名>、worktree add -b）。"""
    toks = tokens(seg)
    if "git" not in toks:
        return []
    t = toks[toks.index("git") + 1:]
    if not t:
        return []
    out = []
    if t[0] in ("checkout", "switch", "worktree"):
        flags = {"checkout": ("-b", "-B"), "switch": ("-c", "-C", "--create", "--force-create"), "worktree": ("-b", "-B")}[t[0]]
        if t[0] == "worktree" and (len(t) < 2 or t[1] != "add"):
            return []
        for i, a in enumerate(t):
            if a in flags and i + 1 < len(t):
                out.append(t[i + 1])
    elif t[0] == "branch":
        rest = t[1:]
        if any(a in ("-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy", "--list", "-l", "-a", "-r", "--show-current", "--contains", "--merged", "--no-merged", "-u", "--set-upstream-to", "--unset-upstream", "--edit-description", "-v", "-vv") or a.startswith("--format") for a in rest):
            return []
        names = [a for a in rest if not a.startswith("-")]
        if names:
            out.append(names[0])
    return out


def check_shell(data: dict) -> None:
    """Bash 與 PowerShell 工具共用的 shell 守門。"""
    cmd = (data.get("tool_input") or {}).get("command") or ""
    if not cmd:
        return
    if data.get("tool_name") == "PowerShell":
        segs = ps_segments(cmd)
        cmd = " ; ".join(segs)  # 正規化後的指令（Set-Location 已換成 cd），給 effective_dir 用
    else:
        segs = segments(cmd)
    raw_agent = data.get("agent_type") or ""
    agent = awf_config.role_name(raw_agent)
    team = awf_config.is_team_agent(raw_agent, CFG)  # 模版角色；使用者自己的 agent 和主 session 同一套規則
    configured = CFG["_configured"]
    git_rules = team or (configured and CFG["main_session_git_rules"])
    gh_rules = team or (configured and CFG["main_session_github_rules"])
    am_on = configured and AM["enabled"]  # 信任模型：auto_merge 啟用時，主 session 不論開關都不能合併、核准
    cwd = Path(data.get("cwd") or WORKSPACE)
    d = effective_dir(cmd, cwd)
    fpaths, fslugs = forbidden_entries()
    # steward 在 auto_merge 條件下允許的段落（合併、退回、留言、轉回 draft）；其餘段落照一般規則
    allowed = {i for i, s in enumerate(segs) if (merge_exception(s, agent, data) if GH_PR_MERGE.search(s)
                                                 else bool(GH_WRITE.search(s)) and steward_write_refusal(s, data) is None)}
    rest_cmd = " ; ".join(s for i, s in enumerate(segs) if i not in allowed)

    for i, seg in enumerate(segs):
        if re.search(r"\bgit\b.*\bpush\b", seg):
            if team:
                block(f"{agent} 不能 push；推送由主 session 在決策者當下同意後執行")
            for url in push_target_urls(seg, d):
                for s in fslugs:
                    if s and s in url:
                        block(f"不允許推到上游或禁止的 repo {s}（{url}）；要決策者當下明確同意並由決策者自己執行")
            if git_rules and is_force_push(seg):
                block("不 force push（含 --force-with-lease、+refspec）")
            if git_rules or am_on:
                hit = pushes_protected(seg, d)
                if hit:
                    block(f"不直接 push 到受保護分支 {hit}（protected_branches）；改推功能分支、開 PR 到 fork，合併走 PR 流程")
        if git_rules and re.search(r"\bgit\b", seg) and re.search(r"(?:^|\s)--no-verify\b", seg):
            block("不用 --no-verify 跳過 git hooks")
        if git_rules:
            prefixes = tuple(CFG["allowed_branch_prefixes"])
            for b in new_branches(seg):
                if prefixes and not b.startswith(prefixes):
                    block(f"分支名 {b} 不在允許的前綴（{'、'.join(prefixes)}）；改用允許的前綴並在回報註明")
        who = agent or "主 session"
        ok = i in allowed
        if (gh_rules or am_on) and not ok and (is_merge_cmd(seg) or is_merge_cmd(rest_cmd)):
            why = merge_refusal(seg, data) if (AM["enabled"] and GH_PR_MERGE.search(seg)) else None
            block(f"{who} 不能合併 PR（gh pr merge、gh api／curl 打 merge 端點）" + (f"：{why}" if why else "")
                  + "；合併只由 auto_merge 的 merger_role 在 gate 通過後執行，其餘由人在 GitHub 上做")
        if (gh_rules or am_on) and is_approve_cmd(seg):
            block(f"{who} 不能核准 PR（gh pr review --approve、API 送 APPROVE）；agent 與主 session 一律不核准")
        if is_rest_write(seg):
            for s in fslugs:
                if s and s in seg:
                    block(f"不允許對上游或禁止的 repo {s} 做寫入動作；要決策者當下明確同意並自己執行")
        if gh_rules and is_rest_write(seg):
            block("不用 curl／Invoke-RestMethod／Invoke-WebRequest 直接對 GitHub API 寫入；改用 gh 並明確寫 --repo（才受守門規則檢查）")
        prot = shell_writes_protected(seg) if (team or configured) else None
        if prot and team:
            block(f"{agent} 不能寫 {prot}（守門設定、gate 稽核紀錄與凍結檔只由人或確定性工具寫；auto_merge 設定改走 PR＋CODEOWNERS）")
        if prot and (prot in AUDIT_FILES or (prot == "guard.config.json" and AM["enabled"])):
            block(f"主 session 不直接改 {prot}（稽核紀錄只由 merge_gate.py、publish.py 寫；auto_merge 啟用時 guard.config.json 改走 PR＋CODEOWNERS）")
        if team and re.search(r"(?:^|[\s/\\])publish\.py\b", seg):
            block(f"{agent} 不能執行 publish.py；推送與開 PR 由主 session 執行")
        if ok:
            continue  # steward 允許的寫入已檢查過 repo 白名單與上游授權，不再套一般的 gh 寫入規則
        if GH_WRITE.search(seg) or GH_API_WRITE.search(seg):
            if team:
                why = steward_write_refusal(seg, data) if is_steward(data) else None
                block(f"{agent} 不能對 GitHub 做寫入動作（{seg[:60]}）" + (f"：{why}" if why else ""))
            for s in fslugs:
                if s and s in seg:
                    block(f"不允許對上游或禁止的 repo {s} 做寫入動作；要決策者當下明確同意並自己執行")
            if gh_rules and configured and GH_WRITE.search(seg):
                fork = CFG.get("fork_repo") or ""
                if fork and not re.search(rf"(?:-R|--repo)[\s=]+{re.escape(fork)}\b", seg):
                    block(f"gh 寫入動作要明確寫 --repo {fork}（在 fork 裡 gh 可能預設到上游）")
                if not fork and not re.search(r"(?:-R|--repo)[\s=]+\S+", seg):
                    block("gh 寫入動作要明確寫 --repo <owner>/<repo>（gh 可能預設到上游）")
            if gh_rules and configured and GH_API_WRITE.search(seg) and CFG.get("fork_repo") and f"repos/{CFG['fork_repo']}/" not in seg:
                block(f"gh api 寫入只能對 repos/{CFG['fork_repo']}/")
        for e in fpaths:
            names = {e, (WORKSPACE / e).as_posix()} if not os.path.isabs(e) else {e}
            if any(n in seg for n in names) and DESTRUCTIVE.search(seg):
                block(f"不動 {e}（禁止路徑，只能讀）")

    forward(git_toplevel(d), "guard-git", data)


# ---------------------------------------------------------------- Edit／Write
def spec_doc_pattern() -> re.Pattern:
    repo = rf"{re.escape(REPOS)}[^/]+/" if REPOS else ""
    return re.compile(rf"^{repo}(?:\.worktrees/[^/]+/)?specs/[^/]+/[^/]+\.md$")


def spec_layout_pattern() -> re.Pattern | None:
    """由 spec_layout.root 推導的放行規則，以 OR 加在 spec_doc_pattern 之外（原規則不動）。沒設定時回 None。
    {epic} 換成一層目錄；root 底下任一層的 *.md 都放行（OpenSpec 的 delta 在 specs/<capability>/spec.md）。"""
    if not SPEC_ROOTS:
        return None
    repo = rf"{re.escape(REPOS)}[^/]+/" if REPOS else ""
    roots = "|".join("[^/]+".join(re.escape(part) for part in r.split("{epic}")) for r in SPEC_ROOTS)
    return re.compile(rf"^{repo}(?:\.worktrees/[^/]+/)?(?:{roots})/(?:[^/]+/)*[^/]+\.md$")


def writes_freeze(data: dict) -> bool:
    """freeze_writers 用 Write 寫入的內容是 {"frozen": true, …}（只能凍結，不能解凍；Edit／NotebookEdit 一律不算）。"""
    a = data.get("agent_type") or ""
    if a not in AM["freeze_writers"] or not awf_config.is_team_agent(a, CFG) or data.get("tool_name") != "Write":
        return False
    try:
        body = json.loads((data.get("tool_input") or {}).get("content") or "")
    except ValueError:
        return False
    return isinstance(body, dict) and body.get("frozen") is True


def check_paths(data: dict) -> None:
    ti = data.get("tool_input") or {}
    fp = ti.get("file_path") or ti.get("notebook_path") or ""
    if not fp:
        return
    raw_agent = data.get("agent_type") or ""
    agent = awf_config.role_name(raw_agent)
    role = awf_config.team_role(raw_agent, CFG)  # 模版角色才套寫入範圍；使用者自己的 agent 和主 session 一樣
    path = Path(norm(fp))
    if not path.is_absolute():
        path = Path(data.get("cwd") or WORKSPACE) / path
    rel = rel_to_workspace(path)
    hit = under_forbidden(path)
    if hit:
        block(f"不動 {hit}（禁止路徑，只能讀）")
    prot = protected_target(str(path), True)
    if prot and agent:
        # 唯一例外：freeze_writers（完整 agent_type）可以用 Write 寫入 {"frozen": true, …}；凍結只會讓合併更嚴，解凍由人做
        if not (prot == AM["freeze_file"] and writes_freeze(data)):
            block(f"{agent} 不能寫 {prot}（守門設定、稽核紀錄與凍結檔只由人或確定性工具寫；凍結檔只有 "
                  f"{'、'.join(AM['freeze_writers']) or 'freeze_writers'} 能用 Write 寫入 frozen=true；auto_merge 設定改走 PR＋CODEOWNERS）")
    elif prot and CFG["_configured"] and (prot in AUDIT_FILES or (prot == "guard.config.json" and AM["enabled"])):
        block(f"主 session 不直接改 {prot}（稽核紀錄只由 merge_gate.py、publish.py 寫；auto_merge 啟用時 guard.config.json 改走 PR＋CODEOWNERS）")
    if role == "tpm-writer":
        layout = spec_layout_pattern()
        ok = rel is not None and (rel.startswith(tuple(CFG["writer_dirs"])) or spec_doc_pattern().match(rel) is not None
                                  or (layout is not None and layout.match(rel) is not None))
        if not ok:
            # 沒設 spec_layout 時 note 是空字串，訊息和原本一字不差
            note = "（spec_layout 另放行 " + "、".join(r.replace("{epic}", "<epic>") + "/ 底下的 *.md" for r in SPEC_ROOTS) + "）" if SPEC_ROOTS else ""
            block(f"tpm-writer 只能寫 {'、'.join(CFG['writer_dirs'])} 與 repo 的 specs/<epic>/*.md{note}（{rel or fp}）")
    elif role.startswith("tpm-"):
        if not (rel is not None and RUNS and rel.startswith(RUNS)):
            block(f"{agent} 只能寫 {RUNS or 'run 目錄'}（{rel or fp}）")
    elif role.startswith("dev-"):
        ok = rel is not None and ((rel.startswith(REPOS) if REPOS else not rel.startswith(".claude/")) or (RUNS and rel.startswith(RUNS)))
        if not ok:
            where = f"{REPOS}<repo>/" if REPOS else "工作區（.claude/ 除外）"
            block(f"{agent} 只能改 {where} 與 {RUNS}（{rel or fp}）")
    forward(git_toplevel(path.parent), "guard-paths", data)


check_bash = check_shell  # 舊名，相容用


if __name__ == "__main__":
    mode = sys.argv[1]
    data = json.load(sys.stdin)
    (check_paths if mode == "paths" else check_shell)(data)
    sys.exit(0)
