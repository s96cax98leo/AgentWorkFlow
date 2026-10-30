"""讀專案的守門設定：${CLAUDE_PROJECT_DIR}/.claude/teams/guard.config.json。

沒有設定檔時用安全預設（只套用角色規則：plugin 的 tpm-*／dev-* 不推送、不對 GitHub 寫入、只寫自己的範圍），
不擋主 session 的一般操作，所以 plugin 在還沒 init 的專案裡也不會誤擋。

「模版角色」＝ agent_type 的命名空間在 team_agent_namespaces（預設只有 agent-workflow:），且角色名以
team_agent_prefixes 開頭（is_team_agent）。使用者自己放在 .claude/agents 的 agent 沒有命名空間，
即使叫 dev-helper、tpm-notes 也不算模版角色，套用和主 session 一樣的規則。
清單欄位可以是 JSON 陣列，也可以是逗號分隔的字串（init.py 從 workflow.config 填值時是字串）。
還沒替換的佔位符（雙大括號包住的大寫名稱）一律當成空值。
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path

DEFAULT_NS = "agent-workflow:"
DEFAULTS = {
    "runs_dir": "_tpm/runs",
    "briefs_dir": "_tpm/briefs",
    "repos_dir": "",  # 空字串＝工作區本身就是目標 repo
    "upstream_repos": [],
    "fork_repo": "",
    "forbidden_paths": [],
    "allowed_branch_prefixes": ["feature/", "bugfix/", "hotfix/", "release/", "merge/"],
    "writer_dirs": [],  # 空＝runs_dir、briefs_dir、docs/
    "team_agent_prefixes": ["tpm-", "dev-"],
    # 模版角色的命名空間：agent_type 去掉角色名後剩下的前綴（含冒號）。清單含空字串時，沒有命名空間的
    # dev-*／tpm-*（例如專案 .claude/agents 裡的同名 agent）也算模版角色，也就是舊版行為。
    "team_agent_namespaces": [DEFAULT_NS],
    "role_map": {
        "dev-spec-author": "spec-author",
        "dev-acceptance-tester": "acceptance-tester",
        "dev-implementer": "implementer",
        "dev-verifier": "verifier",
        "dev-design-reviewer": "design-reviewer",
        "dev-security-reviewer": "security-reviewer",
        "dev-pr-reviewer": "pr-reviewer",
        "tpm-reviewer": "pr-reviewer",
        "tpm-skeptic": "pr-reviewer",
        "tpm-upstream-analyst": "pr-reviewer",
        "dev-merge-steward": "merge-steward",
        "dev-release-watcher": "release-watcher",
    },
    "protected_branches": ["main", "master"],  # 有設定檔時，主 session 也不能直接 push 到這些分支（合併只走 PR）
    "main_session_git_rules": True,  # 有設定檔時，分支前綴、force push、--no-verify、push 受保護分支也套用到主 session
    # 有設定檔時，主 session 的 gh pr merge、--approve、REST 合併與核准、gh 寫入必須帶 --repo 也照 agent 的規則擋。
    # 設 false 放行主 session；但 auto_merge 啟用時主 session 一律不能合併、核准，上游與禁止的 repo 不論開關一律擋。
    "main_session_github_rules": True,
    "auto_merge": {},  # 自動合併的有條件開放；欄位與預設見 AUTO_MERGE_DEFAULTS、auto_merge()
    # 使用者加進 workflow 的自己的 agent（methodology.json 的 extensions）：{"完整 agent_type": "套用哪個模版角色的規則"}。
    # 登記後就算模版角色（推送、GitHub 寫入、寫入範圍照對應角色擋，信封也會驗）；只能對應 EXTENSION_ROLES。
    "extension_agents": {},
}
# auto_merge 預設關閉。開放條件見 guard.py 的 merge_exception() 與 docs/自動合併信任模型.md。
AUTO_MERGE_DEFAULTS = {
    "enabled": False,
    "repos": {},  # {"owner/repo": "low"|"medium"|"high"}：白名單與各 repo 的風險上限（merge_gate 用）
    "merger_role": DEFAULT_NS + "dev-merge-steward",  # 唯一能合併的角色，比對完整 agent_type（含命名空間）
    "merge_method": "squash",  # steward 的 gh pr merge 必須帶對應旗標（--squash／--merge／--rebase）
    # 能寫凍結旗標的角色（完整 agent_type）：只能用 Write 寫入 {"frozen": true, …}；解凍（刪除、改成 false）由人做
    "freeze_writers": [DEFAULT_NS + "tpm-writer", DEFAULT_NS + "dev-release-watcher"],
    "daily_cap": 5,
    "upstream_authorized_by": {},  # {"owner/repo": "授權人" 或 {"by", "link", "scope", "expires"}}；上游沒有授權人一律擋
    "freeze_file": "",  # 空＝<runs_dir>/auto-merge.freeze.json；內容 {"frozen": true, "reason": …}（event-post-merge 寫）
}
LIST_KEYS = ("upstream_repos", "forbidden_paths", "allowed_branch_prefixes", "writer_dirs", "team_agent_prefixes", "protected_branches")
BOOL_KEYS = ("main_session_git_rules", "main_session_github_rules")
PLACEHOLDER = re.compile(r"\{\{[A-Z_]+\}\}")


def workspace() -> Path:
    return Path(os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd())


def config_path() -> Path:
    return workspace() / ".claude" / "teams" / "guard.config.json"


def _clean(v):
    if isinstance(v, str):
        return "" if PLACEHOLDER.search(v) else v.strip()
    if isinstance(v, list):
        return [x for x in (_clean(i) for i in v) if x != ""]
    if isinstance(v, dict):
        return {k: _clean(x) for k, x in v.items()}
    return v


def _namespaces(v) -> list[str]:
    """team_agent_namespaces 正規化：保留空字串（代表「沒有命名空間」），去掉還沒替換的佔位符；
    非空的命名空間一律以冒號結尾。字串形式用逗號分隔（空字串要用 JSON 陣列的 "" 表示）。"""
    if isinstance(v, str):
        items = [x for x in v.split(",") if x.strip()]
    elif isinstance(v, list):
        items = [x for x in v if isinstance(x, str)]
    else:
        return []
    out: list[str] = []
    for x in items:
        if PLACEHOLDER.search(x):
            continue
        x = x.strip()
        if x and not x.endswith(":"):
            x += ":"
        if x not in out:
            out.append(x)
    return out


def _bool(v, default: bool) -> bool:
    """布林設定：接受 true／false，也接受 init 從 workflow.config 填進來的字串（"false"、"no"、"0"）。"""
    if isinstance(v, bool):
        return v
    if isinstance(v, str) and v.strip():
        return v.strip().lower() not in ("false", "no", "0", "off")
    return default


def load() -> dict:
    """回傳合併預設後的設定；cfg["_configured"] 表示專案有設定檔。"""
    cfg = json.loads(json.dumps(DEFAULTS))
    cfg["_configured"] = False
    p = config_path()
    if p.exists():
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            data = None
        if isinstance(data, dict):
            cfg["_configured"] = True
            for k, v in data.items():
                if k.startswith("_"):
                    continue
                if k == "team_agent_namespaces":
                    ns = _namespaces(v)
                    if ns:
                        cfg[k] = ns
                    continue
                v = _clean(v)
                if k in BOOL_KEYS:
                    cfg[k] = _bool(v, DEFAULTS[k])
                    continue
                if k in LIST_KEYS and isinstance(v, str):
                    v = [x.strip() for x in v.split(",") if x.strip()]
                if v in ("", [], None) and k in DEFAULTS and k not in ("upstream_repos", "forbidden_paths", "fork_repo", "repos_dir"):
                    continue  # 空值不蓋掉有意義的預設
                cfg[k] = v
    for k in ("runs_dir", "briefs_dir", "repos_dir"):
        cfg[k] = (cfg.get(k) or "").strip("/")
    if not cfg["writer_dirs"]:
        cfg["writer_dirs"] = [d for d in (cfg["runs_dir"], cfg["briefs_dir"], "docs") if d]
    cfg["writer_dirs"] = [d.strip("/") + "/" for d in cfg["writer_dirs"]]
    return cfg


def full_role(role: str, cfg: dict | None = None) -> str:
    """設定裡的角色名補上命名空間：沒有命名空間（不含 :）時補 team_agent_namespaces 的第一個非空值（預設 agent-workflow:）。
    合併角色只認 plugin 發佈的 agent，專案 .claude/agents 裡的同名 agent 沒有命名空間，比對不會相等。"""
    role = (role or "").strip()
    if not role or ":" in role:
        return role
    ns = next((n for n in ((cfg or {}).get("team_agent_namespaces") or [DEFAULT_NS]) if n), "")
    return ns + role


def auto_merge(cfg: dict) -> dict:
    """auto_merge 設定合併預設值並正規化：repos 一律變成 {repo: tier}、角色補命名空間、freeze_file 變成工作區相對路徑。"""
    raw = cfg.get("auto_merge") if isinstance(cfg.get("auto_merge"), dict) else {}
    am = {**AUTO_MERGE_DEFAULTS, **{k: v for k, v in raw.items() if not k.startswith("_") and v not in ("", None)}}
    repos = am.get("repos")
    if isinstance(repos, str):
        repos = [x.strip() for x in repos.split(",") if x.strip()]
    if isinstance(repos, list):
        repos = {r: "low" for r in repos if isinstance(r, str) and r}
    am["repos"] = repos if isinstance(repos, dict) else {}
    am["enabled"] = am.get("enabled") is True
    am["merger_role"] = full_role(str(am.get("merger_role") or ""), cfg)
    fw = am.get("freeze_writers")
    fw = [x.strip() for x in fw.split(",")] if isinstance(fw, str) else (fw if isinstance(fw, list) else [])
    am["freeze_writers"] = [full_role(x, cfg) for x in fw if isinstance(x, str) and x.strip()]
    am["merge_method"] = str(am.get("merge_method") or "squash").strip().lower()
    if not isinstance(am.get("upstream_authorized_by"), dict):
        am["upstream_authorized_by"] = {}
    am["freeze_file"] = (str(am.get("freeze_file") or "").replace("\\", "/").strip()
                         or ((cfg.get("runs_dir") or "_tpm/runs").strip("/") + "/auto-merge.freeze.json"))
    return am


def role_name(agent: str) -> str:
    """plugin 的 agent 會帶命名空間（例如 agent-workflow:dev-implementer），比對前去掉。"""
    return (agent or "").rsplit(":", 1)[-1]


def namespace(agent: str) -> str:
    """agent_type 的命名空間（含冒號）；沒有命名空間回空字串。"""
    agent = agent or ""
    return agent.rsplit(":", 1)[0] + ":" if ":" in agent else ""


# plugin 發佈的模版角色名。沒有命名空間（或其他命名空間）但角色名和這些完全相同的 agent，
# 照樣套模版角色規則（fail-closed）：它多半是模版 agent 的複本，或是冒用模版角色名，給它較嚴的規則才安全。
# 使用者自己取的其他名字（dev-helper、tpm-notes）不受影響。合併、寫凍結檔仍要比對完整 agent_type，冒用不會因此放行。
RESERVED_ROLES = frozenset({
    "dev-spec-author", "dev-acceptance-tester", "dev-implementer", "dev-integrator", "dev-verifier",
    "dev-design-reviewer", "dev-security-reviewer", "dev-pr-reviewer", "dev-merge-steward", "dev-release-watcher",
    "tpm-planner", "tpm-researcher", "tpm-reviewer", "tpm-skeptic", "tpm-upstream-analyst", "tpm-writer",
})


# 擴充 agent 能套用的模版角色：只有唯讀或只寫報告的角色。合併、凍結、實作、整合、寫文件的角色不開放，
# 對應到其他值時一律改套 tpm-reviewer（最嚴：只能寫 run 目錄）。
EXTENSION_ROLES = frozenset({
    "tpm-researcher", "tpm-reviewer", "tpm-skeptic",
    "dev-verifier", "dev-pr-reviewer", "dev-security-reviewer", "dev-design-reviewer",
})
EXTENSION_FALLBACK = "tpm-reviewer"


def extension_role(agent: str, cfg: dict) -> str:
    """登記在 extension_agents 的擴充 agent 回傳要套用的模版角色名；沒登記回空字串。比對完整 agent_type。"""
    ext = cfg.get("extension_agents")
    if not agent or not isinstance(ext, dict) or agent not in ext:
        return ""
    role = role_name(str(ext.get(agent) or ""))
    return role if role in EXTENSION_ROLES else EXTENSION_FALLBACK


def is_team_agent(agent: str, cfg: dict) -> bool:
    """agent 是不是模版角色：傳**完整的 agent_type**（含命名空間）。
    命名空間要在 team_agent_namespaces 裡，且角色名以 team_agent_prefixes 開頭；或角色名就是 RESERVED_ROLES 之一。
    所有守門（guard-shell、guard-paths、交件信封、role_map 對應、freeze_writers、merger_role）都走這個判斷。"""
    if not agent:
        return False
    if role_name(agent) in RESERVED_ROLES or extension_role(agent, cfg):
        return True
    ns = cfg.get("team_agent_namespaces")
    ns = ns if isinstance(ns, list) and ns else [DEFAULT_NS]
    return namespace(agent) in ns and role_name(agent).startswith(tuple(cfg["team_agent_prefixes"]))


def team_role(agent: str, cfg: dict) -> str:
    """模版角色回傳去掉命名空間的角色名（例如 dev-implementer），擴充 agent 回傳它套用的角色，都不是回空字串。"""
    if role_name(agent) not in RESERVED_ROLES:
        ext = extension_role(agent, cfg)
        if ext:
            return ext
    return role_name(agent) if is_team_agent(agent, cfg) else ""
