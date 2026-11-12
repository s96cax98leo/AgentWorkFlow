#!/usr/bin/env python3
"""交付文件：型錄、交付設定、逐節的來源標記與過期判定、需求追溯矩陣。

用法（在已 init 的專案根目錄）：
    python3 deliverables.py configure [--preset <名>] [--suggest] [--import <清單檔>] [--add a,b] [--remove a,b]
                                      [--custom-unmatched] [--dir <目錄>] [--write --by <誰>] [--json]
    python3 deliverables.py init                     # 依交付設定建目錄與文件骨架（已有的檔不動）
    python3 deliverables.py status [--json]          # 每份文件：齊全、過期、缺原料、等人補
    python3 deliverables.py check                    # 檔頭、過期小節、禁用字、隱私字串、失效連結、總目錄；有問題 exit 1
    python3 deliverables.py index                    # 重寫總目錄（README.md）
    python3 deliverables.py affected --run <run_dir> [--paths-file <檔>] [--milestone] [--close] [--json]
    python3 deliverables.py matrix --run <run_dir> [--stdout]     # 需求追溯矩陣
    python3 deliverables.py sync --run <run_dir> [--reviewed <文件id>#<小節>]…   # 併交付摘要、更新來源紀錄
    python3 deliverables.py export [--out <目錄>]    # 有 pandoc 就轉 docx
    python3 deliverables.py --selftest

設計：
- 交付什麼由專案決定：`.claude/teams/deliverables.json`（configure 在人確認後寫）；沒有時看 profile 的
  `deliverables`（要 `enabled: true`）。兩者都沒有就什麼都不做，流程也不會多派 agent。
- 型錄（`.claude/teams/deliverables/catalog.json`）只是可選的清單與預設內容；設定裡可以改寫任何欄位，也可以自訂文件。
- 每個小節標題下一行是來源標記 `<!-- awf:source <glob>; <glob> -->`。來源檔的雜湊記在
  `.claude/teams/deliverables.state.json`（team agent 寫不到），來源改了而內文沒動，那一節就是過期。
- 這是輔助工具：閘門（save_run、publish、merge_gate）不讀它的輸出。只用標準庫，跨平台。
"""
from __future__ import annotations

import fnmatch
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
PROJECT = TOOLS.parents[2]
CONFIG_NAME = "deliverables.json"
STATE_NAME = "deliverables.state.json"
CATALOG_REL = "deliverables/catalog.json"
DEFAULT_DIR = "docs/deliverables"
INDEX_NAME = "README.md"
RECORD_ID = "dev-record"
MATRIX_ID = "rtm"
SUMMARY_NAME = "交付摘要.md"
PLACEHOLDER = "（待補）"
SOURCE_TYPES = ("auto", "assisted", "external")
WHENS = ("run", "milestone", "close")
DEPTHS = ("full", "summary")
STATUS_TEXT = {"ok": "齊全", "stale": "過期", "missing": "缺原料", "waiting": "等人補"}
MARK = re.compile(r"^<!--\s*awf:source\s*(.*?)\s*-->\s*$")
ENTRY = re.compile(r"^<!--\s*awf:entry\s+([0-9a-f]{6,40})\s*-->\s*$")
SKIP_DIRS = {".git", "node_modules", ".worktrees", ".venv", "venv", "__pycache__", "dist", "build", ".idea", ".codegraph", "graft"}
MAX_FILES = 20000
MAX_SECTION_FILES = 300

# 隱私字串：交付文件是給外部的人看的。命中任何一條，check 失敗、sync 不寫。
AGENT_ROLES = ("planner", "researcher", "reviewer", "skeptic", "writer", "spec-author", "acceptance-tester", "implementer", "integrator", "verifier",
               "design-reviewer", "security-reviewer", "merge-steward", "release-watcher", "upstream-analyst", "pr-reviewer")
PRIVACY = [
    ("電子郵件", re.compile(r"[A-Za-z0-9._%+-]+\s*@\s*[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+")),
    ("本機路徑", re.compile(r"(?:/" r"Users/|/" r"home/|/root/|/var/folders/|/private/(?:tmp|var)/|/tmp/|~/)[^\s]*|[A-Za-z]:\\+" r"Users\\+[^\\\s]+|\\\\[\w.-]+\\[\w$.-]+", re.I)),
    ("憑證或金鑰", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}|\bsk-[A-Za-z0-9_-]{20,}|\bAKIA[0-9A-Z]{16}\b"
                             r"|\bxox[abprs]-[A-Za-z0-9-]{10,}|\bAIza[0-9A-Za-z_-]{30,}|\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"
                             r"|hooks\.slack\.com/services/\S+|(?i:\bbearer\s+[A-Za-z0-9._~+/-]{16,})")),
    ("密碼或 token 的值", re.compile(r"(?i)(?:\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?key|token|client[_-]?secret)\b|密碼|密鑰|金鑰)[\"']?\s*[:=]\s*[\"']?[^\s\"'<>（）]{6,}")),
    ("內部 agent 名稱", re.compile(r"(?i)\b(?:tpm|dev)\s*[-_\\]{1,2}\s*(?:" + "|".join(r.replace("-", r"[-_]") for r in AGENT_ROLES) + r")\b")),
    ("內部網路位址或主機", re.compile(r"(?i)\b(?:10\.\d{1,3}|192\.168|172\.(?:1[6-9]|2\d|3[01])|127\.\d{1,3})\.\d{1,3}\.\d{1,3}\b|\blocalhost\b"
                                   r"|\b[\w-]+(?:\.[\w-]+)*\.(?:internal|intranet|corp|lan|local|localdomain)\b|\bf[cd][0-9a-f]{2}:[0-9a-f:]{4,}")),
]
ZERO_WIDTH = re.compile("[\u200b-\u200f\u2060-\u2064\ufeff\u00ad]")


def privacy_text(text: str) -> str:
    """比對前的正規化：全形轉半形（NFKC）、去掉零寬字元、HTML 實體還原、反斜線跳脫的連字號還原。"""
    import html
    t = unicodedata.normalize("NFKC", html.unescape(text))
    return ZERO_WIDTH.sub("", t).replace("\\-", "-")


def _utf8() -> None:
    for s in (sys.stdout, sys.stderr):
        try:
            s.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass


def posix(p) -> str:
    return str(p).replace("\\", "/")


def sha1_text(text: str) -> str:
    return hashlib.sha1(text.encode("utf-8", "replace")).hexdigest()


def sha1_file(p: Path):
    try:
        return hashlib.sha1(p.read_bytes().replace(b"\r\n", b"\n")).hexdigest()
    except OSError:
        return None


def load_json(p: Path):
    try:
        return json.loads(Path(p).read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return None


def write_lf(p: Path, text: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def safe_rel(raw) -> str:
    """相對路徑（不含 ..、不是絕對路徑）；不合理回空字串。"""
    s = posix(raw or "").strip().rstrip("/")
    if not s or s.startswith("/") or re.match(r"^[A-Za-z]:", s) or any(seg in ("..", "") for seg in s.split("/")):
        return ""
    return s


def glob_re(g: str):
    """`**` 跨目錄、`*` 不跨目錄、`?` 一個字元。和資料包工具的寫法一致。"""
    g = re.sub(r"^(?:\./)+", "", posix(g).strip())
    out = []
    i = 0
    while i < len(g):
        c = g[i]
        if g.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
            continue
        if g.startswith("**", i):
            out.append(".*")
            i += 2
            continue
        if c == "*":
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        else:
            out.append(re.escape(c))
        i += 1
    return re.compile("^" + "".join(out) + "$")


def repo_files(repo: Path) -> list[str]:
    """repo 裡的檔（相對路徑、排序）。有 git 用 ls-files，沒有就走訪（跳過常見的產出目錄）。"""
    try:
        r = subprocess.run(["git", "-C", str(repo), "ls-files", "-co", "--exclude-standard"], capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=20)
        if r.returncode == 0 and r.stdout.strip():
            return sorted(posix(x) for x in r.stdout.splitlines() if x.strip())[:MAX_FILES]
    except (OSError, subprocess.SubprocessError):
        pass
    out: list[str] = []
    for base, dirs, files in os.walk(str(repo)):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for f in sorted(files):
            out.append(posix(os.path.relpath(os.path.join(base, f), str(repo))))
            if len(out) >= MAX_FILES:
                return out
    return sorted(out)


def match_files(files: list[str], globs) -> list[str]:
    res = [glob_re(g) for g in globs if isinstance(g, str) and g.strip()]
    return [f for f in files if any(r.match(f) for r in res)]


# ------------------------------------------------------------------ 型錄與設定
def load_catalog(root: Path = PROJECT) -> dict:
    for p in (root / ".claude" / "teams" / CATALOG_REL, TOOLS.parent / CATALOG_REL):
        data = load_json(p)
        if isinstance(data, dict) and isinstance(data.get("documents"), list):
            return data
    return {"groups": [], "documents": [], "presets": []}


def catalog_index(cat: dict) -> dict:
    return {d["id"]: d for d in cat.get("documents", []) if isinstance(d, dict) and isinstance(d.get("id"), str)}


def config_errors(cfg) -> list[str]:
    """交付設定的格式與語意（schema 表達不了的一起檢查）。"""
    if not isinstance(cfg, dict):
        return ["交付設定不是物件"]
    errs = []
    if "dir" in cfg and not safe_rel(cfg.get("dir")):
        errs.append("dir 要是專案內的相對路徑（不能是絕對路徑或含 ..）：%r" % (cfg.get("dir"),))
    docs = cfg.get("documents")
    if not isinstance(docs, list):
        return errs + ["documents 要是陣列"]
    seen = set()
    for i, d in enumerate(docs):
        if not isinstance(d, dict) or not isinstance(d.get("id"), str) or not re.match(r"^[a-z0-9][a-z0-9-]{0,63}$", d.get("id") or ""):
            errs.append("documents[%d] 要有 id（小寫英數與連字號）" % i)
            continue
        if d["id"] in seen:
            errs.append("documents 的 id 重複：%s" % d["id"])
        seen.add(d["id"])
        for key, allowed in (("source", SOURCE_TYPES), ("when", WHENS), ("depth", DEPTHS)):
            if key in d and d[key] not in allowed:
                errs.append("%s 的 %s 要是 %s：%r" % (d["id"], key, "／".join(allowed), d[key]))
        for key in ("sections", "sources", "on", "design_sections"):
            if key in d and not (isinstance(d[key], list) and all(isinstance(x, str) and x.strip() for x in d[key])):
                errs.append("%s 的 %s 要是字串陣列" % (d["id"], key))
        for g in d.get("sources") or []:
            if isinstance(g, str) and (g.startswith("/") or re.match(r"^[A-Za-z]:", g) or ".." in posix(g).split("/")):
                errs.append("%s 的 sources 不能是絕對路徑或含 ..：%s" % (d["id"], g))
    return errs


def load_config(root: Path = PROJECT):
    """回傳（設定或 None, 來源說明）。專案的 deliverables.json 優先；沒有時看 profile 的 deliverables（要 enabled: true）。"""
    p = root / ".claude" / "teams" / CONFIG_NAME
    if p.is_file():
        cfg = load_json(p)
        if not isinstance(cfg, dict):
            return None, "%s 讀不懂" % posix(p.relative_to(root))
        errs = config_errors(cfg)
        return (None, "交付設定有問題：" + "；".join(errs[:4])) if errs else (cfg, posix(p.relative_to(root)))
    prof = load_json(root / ".claude" / "teams" / "methodology.json")
    d = prof.get("deliverables") if isinstance(prof, dict) else None
    if isinstance(d, dict) and d.get("enabled") is True and isinstance(d.get("documents"), list):
        errs = config_errors(d)
        return (None, "profile 的 deliverables 有問題：" + "；".join(errs[:4])) if errs else (d, "methodology.json 的 deliverables")
    return None, "沒有交付設定（用 deliverables.py configure 產生；沒有時流程不產生也不檢查交付文件）"


def resolve_docs(cfg: dict, cat: dict) -> list[dict]:
    """設定裡的每份文件＝型錄的預設內容，被設定裡寫的欄位蓋掉。型錄沒有的 id 是自訂文件（預設 assisted）。"""
    idx = catalog_index(cat)
    out = []
    for d in cfg.get("documents") or []:
        if not isinstance(d, dict) or not isinstance(d.get("id"), str):
            continue
        base = dict(idx.get(d["id"]) or {"id": d["id"], "title": d["id"], "group": "custom", "source": "assisted", "sections": []})
        base.update({k: v for k, v in d.items() if v not in (None, "")})
        base.setdefault("when", "run")
        base.setdefault("depth", "full")
        base.setdefault("sections", [])
        base["custom"] = d["id"] not in idx
        out.append(base)
    return out


def out_dir(cfg: dict, root: Path) -> Path:
    return root / (safe_rel(cfg.get("dir")) or DEFAULT_DIR)


def doc_path(cfg: dict, root: Path, doc: dict) -> Path:
    return out_dir(cfg, root) / (doc["id"] + ".md")


def source_root(cfg: dict, root: Path, repo: Path | None = None) -> Path:
    """來源 glob 的基準：指定的目標 repo → 設定的 repo → 專案根目錄。"""
    if repo is not None:
        return repo
    rel = safe_rel(cfg.get("repo"))
    return root / rel if rel else root


# ------------------------------------------------------------------ 文件的讀寫
def skeleton(doc: dict, groups: dict) -> str:
    head = ["---", "id: " + doc["id"], "title: " + str(doc.get("title") or doc["id"]), "group: " + str(doc.get("group") or "custom"),
            "source: " + str(doc.get("source") or "assisted"), "depth: " + str(doc.get("depth") or "full"), "---", "",
            "# " + str(doc.get("title") or doc["id"]), ""]
    if doc.get("source") == "external":
        head += ["這份文件由外部提供。請把檔案放在 `external/%s/`，檢查時只看有沒有附。" % doc["id"], ""]
        return "\n".join(head)
    srcs = "; ".join(doc.get("sources") or [])
    for s in doc.get("sections") or []:
        head += ["## " + s, "<!-- awf:source %s -->" % srcs, "", PLACEHOLDER, ""]
    return "\n".join(head)


def parse_doc(text: str) -> dict:
    """回傳 {meta, sections:[{title, sources:[glob], body, line}]}。小節＝二級標題；來源標記是標題後第一個非空行。"""
    lines = text.replace("\r\n", "\n").split("\n")
    meta: dict = {}
    i = 0
    if lines and lines[0].strip() == "---":
        i = 1
        while i < len(lines) and lines[i].strip() != "---":
            m = re.match(r"^([A-Za-z_][\w-]*)\s*:\s*(.*)$", lines[i])
            if m:
                meta[m.group(1)] = m.group(2).strip()
            i += 1
        i += 1
    sections: list[dict] = []
    cur = None
    fence = False
    for n in range(i, len(lines)):
        ln = lines[n]
        if ln.lstrip().startswith("```"):
            fence = not fence
        if not fence and ln.startswith("## ") and not ln.startswith("###"):
            cur = {"title": ln[3:].strip(), "sources": None, "body": [], "line": n + 1}
            sections.append(cur)
            continue
        if cur is None:
            continue
        m = MARK.match(ln.strip())
        if m and cur["sources"] is None and not "".join(cur["body"]).strip():
            cur["sources"] = [x.strip() for x in m.group(1).split(";") if x.strip()]
            continue
        cur["body"].append(ln)
    for s in sections:
        s["body"] = "\n".join(s["body"]).strip()
        s["sources"] = s["sources"] or []
    return {"meta": meta, "sections": sections}


def doc_sections(doc: dict, text: str) -> dict:
    """parse_doc，再把每個小節的來源設成「文件層級的 sources ∪ 小節的來源標記」：標記只能加、不能減，
    把標記刪掉或改成對不到檔的 glob，都躲不過過期判定。"""
    parsed = parse_doc(text)
    base = [g for g in doc.get("sources") or [] if isinstance(g, str)]
    for s in parsed["sections"]:
        s["marker"] = list(s["sources"])
        s["sources"] = base + [g for g in s["sources"] if g not in base]
    return parsed


def body_hash(body: str) -> str:
    """內文的雜湊：空白、標點、全形半形的差異都不算改動（只動這些不能讓過期的小節變成最新）。"""
    t = unicodedata.normalize("NFKC", body)
    return sha1_text("".join(ch for ch in t if not (ch.isspace() or unicodedata.category(ch)[0] in "PSZC")))


def is_placeholder(body: str) -> bool:
    return not body.strip() or body.strip() == PLACEHOLDER


def section_sources(sec: dict, files: list[str], src_root: Path) -> dict:
    """這一節的來源檔 → 雜湊。來源寫的是 glob，對不到檔就是空的。"""
    safe = [g for g in sec.get("sources") or [] if not (g.startswith("/") or re.match(r"^[A-Za-z]:", g) or ".." in posix(g).split("/"))]
    out = {}
    for f in match_files(files, safe)[:MAX_SECTION_FILES]:
        h = sha1_file(src_root / f)
        if h:
            out[f] = h
    return out


def load_state(root: Path) -> dict:
    data = load_json(root / ".claude" / "teams" / STATE_NAME)
    return data if isinstance(data, dict) else {}


def save_state(root: Path, state: dict) -> None:
    write_lf(root / ".claude" / "teams" / STATE_NAME, json.dumps(state, ensure_ascii=False, indent=1, sort_keys=True) + "\n")


def section_state(doc_id: str, sec: dict, state: dict, files: list[str], src_root: Path) -> tuple[str, str]:
    """（狀態, 說明）。狀態：placeholder、fresh、stale、unstamped。"""
    if is_placeholder(sec["body"]):
        return "placeholder", "還沒寫"
    if not sec["sources"]:
        return "fresh", ""        # 這份文件沒有設定來源（例如由工具寫的開發紀錄）：沒有東西可以過期
    rec = ((state.get(doc_id) or {}).get(sec["title"]) or {}) if isinstance(state.get(doc_id), dict) else {}
    if not isinstance(rec, dict) or "sources" not in rec:
        return "unstamped", "有內容但沒有來源紀錄（跑 deliverables.py sync 記錄）"
    cur = section_sources(sec, files, src_root)
    if cur == rec.get("sources"):
        return "fresh", ""
    changed = sorted(set(k for k in set(cur) | set(rec.get("sources") or {}) if cur.get(k) != (rec.get("sources") or {}).get(k)))
    if body_hash(sec["body"]) == rec.get("body"):
        return "stale", "來源改了、內文沒動：" + "、".join(changed[:4])
    return "unstamped", "內文改過，還沒記錄來源（跑 deliverables.py sync）"


# ------------------------------------------------------------------ status
def doc_status(doc: dict, cfg: dict, root: Path, state: dict, files: list[str], src_root: Path) -> dict:
    out = {"id": doc["id"], "title": doc.get("title"), "source": doc.get("source"), "when": doc.get("when"),
           "file": posix(doc_path(cfg, root, doc).relative_to(root)), "sections": []}

    def done(status: str, detail: str) -> dict:
        out.update(status=status, detail=detail)
        return out

    if doc.get("source") == "external":
        ext = out_dir(cfg, root) / "external" / doc["id"]
        got = sorted(p.name for p in ext.iterdir() if p.is_file() and not p.name.startswith(".")) if ext.is_dir() else []
        return done("ok", "已附 %d 個檔" % len(got)) if got else done("waiting", "等外部提供；放在 %s/" % posix(ext.relative_to(root)))
    p = doc_path(cfg, root, doc)
    have_material = bool(match_files(files, doc.get("sources") or []))
    if not p.is_file():
        return done("missing", "文件還沒建立（跑 deliverables.py init）" if have_material or not doc.get("sources") else "文件還沒建立，也找不到原料（%s）" % "、".join((doc.get("sources") or [])[:3]))
    parsed = doc_sections(doc, p.read_text(encoding="utf-8", errors="replace"))
    titles = [s["title"] for s in parsed["sections"]]
    absent = [s for s in doc.get("sections") or [] if s not in titles]
    kinds: dict = {}
    for s in parsed["sections"]:
        st, why = section_state(doc["id"], s, state, files, src_root)
        out["sections"].append({"title": s["title"], "state": st, "detail": why})
        kinds.setdefault(st, []).append(s["title"])
    if kinds.get("stale"):
        return done("stale", "過期的小節：" + "、".join(kinds["stale"]))
    if kinds.get("unstamped"):
        return done("stale", "還沒記錄來源的小節：" + "、".join(kinds["unstamped"]))
    todo = (kinds.get("placeholder") or []) + absent
    if todo:
        if doc.get("source") == "assisted":
            return done("waiting", "等人補的小節：" + "、".join(todo))
        if not have_material:
            return done("missing", "找不到原料（%s）；小節：%s" % ("、".join((doc.get("sources") or ["沒有設定 sources"])[:3]), "、".join(todo)))
        return done("stale", "原料已有、還沒產生的小節：" + "、".join(todo))
    return done("ok", "")


def status_all(root: Path = PROJECT, repo: Path | None = None):
    cfg, note = load_config(root)
    if cfg is None:
        return None, note
    cat = load_catalog(root)
    src_root = source_root(cfg, root, repo)
    files = repo_files(src_root)
    state = load_state(root)
    return [doc_status(d, cfg, root, state, files, src_root) for d in resolve_docs(cfg, cat)], note


# ------------------------------------------------------------------ index
def index_text(cfg: dict, cat: dict) -> str:
    groups = {g["id"]: g["title"] for g in cat.get("groups", []) if isinstance(g, dict)}
    groups.setdefault("custom", "自訂")
    label = {"auto": "自動產生", "assisted": "工具給原料、人補內容", "external": "外部提供"}
    when = {"run": "每次交付更新", "milestone": "里程碑", "close": "結案"}
    lines = ["# 交付文件總目錄", "", "這份目錄由 `deliverables.py index` 產生，請不要手改。", ""]
    docs = resolve_docs(cfg, cat)
    order = [g["id"] for g in cat.get("groups", []) if isinstance(g, dict)] + ["custom"]
    for gid in order:
        mine = [d for d in docs if (d.get("group") or "custom") == gid or (gid == "custom" and (d.get("group") or "custom") not in order)]
        if not mine:
            continue
        lines += ["## " + groups.get(gid, gid), "", "| 文件 | 檔案 | 來源 | 更新時機 |", "|---|---|---|---|"]
        for d in mine:
            link = "external/%s/" % d["id"] if d.get("source") == "external" else "[%s.md](%s.md)" % (d["id"], d["id"])
            lines.append("| %s | %s | %s | %s |" % (d.get("title"), link, label.get(d.get("source"), d.get("source")), when.get(d.get("when"), d.get("when"))))
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


# ------------------------------------------------------------------ check
def load_patterns(root: Path, rel) -> tuple[list, list[str]]:
    """一行一個正規式（# 開頭是註解）。讀不到或寫壞的列在說明裡。"""
    rel = safe_rel(rel)
    if not rel:
        return [], []
    try:
        raw = (root / rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return [], ["禁用字清單讀不到：%s" % rel]
    pats, notes = [], []
    for ln in raw.splitlines():
        ln = ln.strip()
        if not ln or ln.startswith("#"):
            continue
        try:
            pats.append(re.compile(ln))
        except re.error:
            notes.append("禁用字清單有寫壞的正規式（略過，不印內容）")
    return pats, notes


def privacy_hits(text: str, allow=(), extra=()) -> list[str]:
    """命中的隱私字串種類與行號（不印內容本身，避免把機敏值再印一次）。比對前先正規化（privacy_text），
    另把整份文字去掉換行再掃一次，抓拆成兩行的信箱與 token。extra＝（種類, 正規式）的額外規則（例如 run 目錄名稱）。"""
    allowed = []
    for a in allow or ():
        try:
            allowed.append(re.compile(a))
        except (re.error, TypeError):
            pass
    rules = list(PRIVACY) + [x for x in extra or () if x and x[1] is not None]
    out = []
    norm_text = privacy_text(text.replace("\r\n", "\n"))
    for n, ln in enumerate(norm_text.split("\n"), 1):
        for name, rx in rules:
            for m in rx.finditer(ln):
                if not any(a.search(m.group(0)) for a in allowed):
                    out.append("第 %d 行：%s" % (n, name))
                    break
    if not out:
        joined = re.sub(r"\s*\n\s*", "", norm_text)
        for name, rx in rules[:3]:
            if any(not any(a.search(m.group(0)) for a in allowed) for m in rx.finditer(joined)):
                out.append("跨行：%s" % name)
    return out


def run_name_rule(root: Path):
    """runs_dir 底下的 run 目錄名稱與 runs_dir 本身：都是內部資訊，不能出現在交付文件。"""
    guard = load_json(root / ".claude" / "teams" / "guard.config.json")
    rd = safe_rel(guard.get("runs_dir") if isinstance(guard, dict) else "") or "_tpm/runs"
    names = []
    try:
        names = sorted(d.name for d in (root / rd).iterdir() if d.is_dir() and len(d.name) >= 6)[-500:]
    except OSError:
        pass
    pats = [re.escape(rd)] + [re.escape(n) for n in names]
    return ("內部 run 目錄", re.compile("|".join(pats)))


LINK = re.compile(r"(?<!\!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")


def broken_links(text: str, base: Path) -> list[str]:
    out = []
    fence = False
    for n, ln in enumerate(text.replace("\r\n", "\n").split("\n"), 1):
        if ln.lstrip().startswith("```"):
            fence = not fence
        if fence:
            continue
        for m in LINK.finditer(ln):
            t = m.group(1)
            if re.match(r"^(?:[a-z][a-z0-9+.-]*:|#|//)", t, re.I):
                continue
            t = t.split("#", 1)[0]
            if t and not (base / t).exists():
                out.append("第 %d 行：連結對不到檔（%s）" % (n, t))
    return out


def check_all(root: Path = PROJECT, repo: Path | None = None):
    """回傳（問題清單, 說明）。只檢查交付設定裡列的文件。沒有交付設定回 (None, 說明)。"""
    cfg, note = load_config(root)
    if cfg is None:
        return None, note
    cat = load_catalog(root)
    docs = resolve_docs(cfg, cat)
    src_root = source_root(cfg, root, repo)
    files = repo_files(src_root)
    state = load_state(root)
    banned, notes = load_patterns(root, cfg.get("banned_terms_file"))
    allow = cfg.get("privacy_allow") if isinstance(cfg.get("privacy_allow"), list) else []
    errs: list[str] = list(notes)
    runrule = run_name_rule(root)
    d_out = out_dir(cfg, root)

    for doc in docs:
        if doc.get("source") == "external":
            st = doc_status(doc, cfg, root, state, files, src_root)
            if st["status"] != "ok":
                errs.append("%s：%s" % (doc["id"], st["detail"]))
            continue
        p = doc_path(cfg, root, doc)
        rel = posix(p.relative_to(root))
        if not p.is_file():
            errs.append("%s：文件不存在（跑 deliverables.py init）" % rel)
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        parsed = doc_sections(doc, text)
        for key in ("id", "title", "group", "source"):
            if not parsed["meta"].get(key):
                errs.append("%s：檔頭缺 %s" % (rel, key))
        if parsed["meta"].get("id") and parsed["meta"]["id"] != doc["id"]:
            errs.append("%s：檔頭的 id（%s）和檔名不一致" % (rel, parsed["meta"]["id"]))
        for s in parsed["sections"]:
            st, why = section_state(doc["id"], s, state, files, src_root)
            if st in ("stale", "unstamped"):
                errs.append("%s〈%s〉：%s" % (rel, s["title"], why))
        for n, ln in enumerate(text.split("\n"), 1):
            if any(b.search(ln) for b in banned):
                errs.append("%s 第 %d 行：出現禁用字" % (rel, n))
        errs += ["%s %s" % (rel, h) for h in privacy_hits(text, allow, [runrule])]
        errs += ["%s %s" % (rel, h) for h in broken_links(text, p.parent)]
    idx = d_out / INDEX_NAME
    if not idx.is_file():
        errs.append("%s：總目錄不存在（跑 deliverables.py index）" % posix(idx.relative_to(root)))
    elif idx.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n") != index_text(cfg, cat):
        errs.append("%s：總目錄和交付設定不一致（跑 deliverables.py index）" % posix(idx.relative_to(root)))
    return errs, note


# ------------------------------------------------------------------ 受影響的文件
def classify(path: str) -> set:
    try:
        import recipe as recipe_tool
        return set(recipe_tool.classify(path))
    except Exception:  # noqa: BLE001  分不出來一律當程式
        return {"code"}


def run_facts(run: Path, paths) -> dict:
    """這次 run 的事實：改動檔、改動分類、modifiers、recipe、有哪些段、事件類型。"""
    data = load_json(run / "run.json") or {}
    rec = data.get("recipe") if isinstance(data.get("recipe"), dict) else {}
    res = rec.get("resolved") if isinstance(rec.get("resolved"), dict) else {}
    mods = res.get("modifiers") or rec.get("modifiers") or []
    stages = [s for s in (res.get("stages") or list((data.get("stages") or {}).keys())) if isinstance(s, str)]
    cats: set = set()
    plist = sorted(set(re.sub(r"^(?:\./)+", "", posix(p).strip()) for p in (paths or []) if isinstance(p, str) and p.strip()))
    for p in plist:
        cats |= classify(p)
    return {"paths": plist, "categories": cats, "modifiers": set(m for m in mods if isinstance(m, str)),
            "recipe": str(res.get("name") or rec.get("name") or ""), "stages": set(stages), "event": str(data.get("event") or "")}


def token_hit(tok: str, facts: dict) -> str:
    """一個觸發條件有沒有成立；成立回理由（空字串＝沒成立）。"""
    tok = tok.strip()
    if tok.startswith("mod:"):
        return "這次有 %s" % tok[4:] if tok[4:] in facts["modifiers"] else ""
    if tok.startswith("path:"):
        hit = match_files(facts["paths"], [tok[5:]])
        return "改到 %s" % hit[0] if hit else ""
    if tok.startswith("stage:"):
        return "這次有 %s 段" % tok[6:] if tok[6:] in facts["stages"] else ""
    if tok.startswith("recipe:"):
        return "需求類型是 %s" % tok[7:] if tok[7:] == facts["recipe"] else ""
    if tok.startswith("event:"):
        return "事件是 %s" % tok[6:] if tok[6:] == facts["event"] else ""
    return "改動裡有 %s 類的檔" % tok if tok in facts["categories"] else ""


def why_affected(on, facts: dict) -> str:
    for tok in on or []:
        if isinstance(tok, str):
            why = token_hit(tok, facts)
            if why:
                return why
    return ""


def affected(root: Path, run: Path, paths, milestone: bool = False, close: bool = False) -> dict:
    """這次 run 要更新哪些文件的哪些小節。回傳 {enabled, affected, deferred, hints, design_sections, note}。
    只看交付設定裡列的；沒列但對得上的放 hints（只提示，不自動加）。"""
    cfg, note = load_config(root)
    if cfg is None:
        return {"enabled": False, "note": note, "affected": [], "deferred": [], "hints": [], "design_sections": []}
    cat = load_catalog(root)
    docs = resolve_docs(cfg, cat)
    facts = run_facts(run, paths)
    out = {"enabled": True, "note": note, "affected": [], "deferred": [], "hints": [], "design_sections": []}
    for d in docs:
        if d.get("design_sections"):
            out["design_sections"].append({"id": d["id"], "title": d.get("title"), "sections": list(d["design_sections"])})
        if d.get("source") == "external" or d["id"] in (MATRIX_ID, RECORD_ID):
            continue        # 外部文件不由流程產生；追溯矩陣與開發紀錄由工具寫（matrix、sync）
        why = why_affected(d.get("on"), facts)
        if not why:
            continue
        when = d.get("when") or "run"
        if (when == "milestone" and not (milestone or close)) or (when == "close" and not close):
            out["deferred"].append({"id": d["id"], "title": d.get("title"), "when": when, "why": why})
            continue
        sec_on = d.get("section_on") if isinstance(d.get("section_on"), dict) else {}
        secs = [s for s in d.get("sections") or [] if s in sec_on and why_affected(sec_on[s], facts)] if sec_on else []
        secs = secs or list(d.get("sections") or [])
        out["affected"].append({"id": d["id"], "title": d.get("title"), "file": posix(doc_path(cfg, root, d).relative_to(root)), "depth": d.get("depth") or "full",
                                "source": d.get("source"), "why": why, "sections": secs, "sources": list(d.get("sources") or [])})
    mine = {d["id"] for d in docs}
    for d in cat.get("documents", []):
        if d["id"] in mine or d.get("source") == "external" or not isinstance(d.get("suggest"), dict):
            continue
        sug = d["suggest"]
        why = why_affected(["mod:" + m for m in sug.get("modifiers") or []] + ["path:" + g for g in sug.get("paths") or []], facts)
        if why:
            out["hints"].append({"id": d["id"], "title": d.get("title"), "why": why})
    return out


def changed_paths(repo: Path, base: str) -> list[str]:
    if not base or base.startswith("-") or not re.fullmatch(r"[\w./@^~{}-]+", base):
        return []          # 不讓 base 變成 git 的選項（--output=… 之類）
    for rng in (base + "...HEAD", base):
        try:
            r = subprocess.run(["git", "-C", str(repo), "diff", "--name-only", "--end-of-options", rng], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=20)
        except (OSError, subprocess.SubprocessError):
            return []
        if r.returncode == 0:
            return [posix(x) for x in r.stdout.splitlines() if x.strip()]
    return []


def pack_args(stage: str, run: Path, root: Path, repo: Path | None, data: dict, paths=None) -> tuple[dict, list[str]]:
    """給資料包工具用：這一段的 workflow 需要的交付資訊。沒有交付設定回空的（流程不變）。"""
    cfg, _ = load_config(root)
    if cfg is None or stage not in ("design", "deliver"):
        return {}, []
    if paths is None and stage == "deliver" and repo is not None:
        args = data.get("args") if isinstance(data.get("args"), dict) else {}
        paths = changed_paths(repo, str(args.get("base") or "origin/main"))
    res = affected(root, run, paths or [])
    notes: list[str] = []
    if stage == "design":
        if not res["design_sections"]:
            return {}, []
        return {"design_sections": res["design_sections"]}, ["交付文件：design.md 要有 %d 份文件需要的小節" % len(res["design_sections"])]
    out: dict = {"summary": posix(run / "deliver" / SUMMARY_NAME)}      # 有交付設定就寫交付摘要（給客戶看的這次交付說明）
    if res["affected"]:
        out["affected"] = res["affected"]
        cat = load_catalog(root)
        for a in res["affected"]:            # 文件還沒建立就先放骨架，writer 才有小節與來源標記可以改
            ensure_doc(cfg, cat, root, a["id"])
        notes.append("交付文件：這次要更新 " + "、".join("%s（%s）" % (a["title"], "、".join(a["sections"])) for a in res["affected"]))
    for h in res["hints"]:
        notes.append("交付文件：建議把「%s」加進交付設定（%s）；要加請跑 deliverables.py configure --add %s --write" % (h["title"], h["why"], h["id"]))
    for d in res["deferred"]:
        notes.append("交付文件：「%s」受影響，但設定成 %s 才更新，這次不動" % (d["title"], "里程碑" if d["when"] == "milestone" else "結案"))
    if res["hints"]:
        out["hints"] = res["hints"]
    return out, notes


# ------------------------------------------------------------------ 需求追溯矩陣
def _criteria(obj):
    """從 test 段的結果裡找逐條驗收結果（信封、payload 或直接的物件都接受）。"""
    if isinstance(obj, dict):
        if isinstance(obj.get("criteria"), list) and "commit" in obj:
            return obj
        for k in ("verifier", "payload", "result", "combined"):
            got = _criteria(obj.get(k))
            if got:
                return got
    return None


def _subtasks(obj):
    if isinstance(obj, dict):
        if isinstance(obj.get("subtasks"), list):
            return obj["subtasks"]
        for k in ("payload", "result", "plan", "design", "spec"):
            got = _subtasks(obj.get(k))
            if got:
                return got
    return []


def _cell(v, limit: int = 120) -> str:
    s = re.sub(r"\s+", " ", str(v if v is not None else "")).replace("|", "｜").strip()
    return s if len(s) <= limit else s[: limit - 1] + "…"


def matrix_rows(run: Path) -> tuple[list[dict], list[str]]:
    """每條驗收條件一列：條件 ↔ spec 小節 ↔ 驗證指令 ↔ 結果與證據 ↔ commit。缺的環節列在 gaps。"""
    test = _criteria(load_json(run / "test.json") or {})
    tasks = _subtasks(load_json(run / "design.json") or {}) or _subtasks(load_json(run / "build.json") or {})
    if not test:
        return [], ["這個 run 沒有驗證結果（test.json 裡找不到逐條驗收結果）"]
    commit = str(test.get("commit") or "")
    rows, gaps = [], []
    for c in test.get("criteria") or []:
        if not isinstance(c, dict):
            continue
        cid = str(c.get("id") or "")
        bare = cid.split(":", 1)[1] if ":" in cid else cid       # 分片驗證的 id 是 <分片>:<原 id>
        owners = [t for t in tasks if isinstance(t, dict) and any(isinstance(a, str) and bare and (a == bare or a.startswith(bare + " ") or a.startswith(bare + "：") or a.startswith(bare + ":"))
                                                                    for a in (t.get("acceptance") or []))]
        refs = sorted({r for t in owners for r in (t.get("spec_refs") or []) if isinstance(r, str)})
        stories = sorted({r for t in owners for r in (t.get("story_ids") or []) if isinstance(r, str)})
        row = {"id": cid, "criterion": c.get("criterion"), "stories": stories, "spec": refs, "tasks": [str(t.get("id")) for t in owners],
               "command": c.get("command"), "result": c.get("result"), "evidence": c.get("evidence"), "commit": commit}
        miss = [name for name, v in (("spec 小節", refs), ("驗證指令", c.get("command")), ("證據", c.get("evidence")), ("commit", commit if commit not in ("", "n/a") else "")) if not v]
        if c.get("result") != "pass":
            miss.append("結果是 %s" % c.get("result"))
        if miss:
            gaps.append("%s：%s" % (cid or "（沒有 id）", "、".join("缺" + m if not m.startswith("結果") else m for m in miss)))
        row["gaps"] = miss
        rows.append(row)
    return rows, gaps


PATHISH = re.compile(r"(?:[A-Za-z]:)?(?:[\w.~$-]*[/\\])+([\w.-]+)")


def strip_paths(v) -> str:
    """證據與指令裡的路徑只留檔名：交付文件不需要 run 目錄、暫存目錄或本機路徑。"""
    return PATHISH.sub(lambda m: m.group(1), str(v if v is not None else ""))


def matrix_block(run: Path, title: str) -> tuple[str, list[str]]:
    rows, gaps = matrix_rows(run)
    lines = ["### " + title, "", "| 驗收條件 | 內容 | 需求／Story | spec 小節 | 驗證指令 | 結果 | 證據 | commit | 缺的環節 |", "|---|---|---|---|---|---|---|---|---|"]
    label = {"pass": "通過", "fail": "未通過", "not_verifiable": "無法驗證"}
    for r in rows:
        lines.append("| %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
            _cell(r["id"], 40), _cell(r["criterion"]), _cell("、".join(r["stories"]) or "—", 60), _cell("、".join(r["spec"]) or "—", 80),
            "`%s`" % _cell(strip_paths(r["command"]), 80).replace("`", "'") if r["command"] else "—", label.get(r["result"], _cell(r["result"], 20)), _cell(strip_paths(r["evidence"])),
            _cell(str(r["commit"])[:12] or "—", 12), _cell("、".join(r["gaps"]) or "無", 80)))
    if not rows:
        lines.append("| — | %s | — | — | — | — | — | — | — |" % _cell(gaps[0] if gaps else "沒有資料"))
    return "\n".join(lines) + "\n", gaps


def entry_key(run: Path) -> str:
    return sha1_text(run.name)[:12]


def upsert_entry(text: str, key: str, block: str, heading: str) -> str:
    """把一段內容放進文件的某個小節底下；同一個 key 已經有就整段換掉（重跑結果相同）。"""
    lines = text.replace("\r\n", "\n").rstrip("\n").split("\n")
    out, i, placed = [], 0, False
    start = "<!-- awf:entry %s -->" % key
    end = "<!-- awf:entry-end %s -->" % key
    while i < len(lines):
        if lines[i].strip() == start:
            j = i
            while j < len(lines) and lines[j].strip() != end:
                j += 1
            if j >= len(lines):
                raise ValueError("文件裡有 %s 卻沒有對應的結尾標記，不寫入（避免蓋掉後面的內容）" % start)
            out += [start] + block.rstrip("\n").split("\n") + [end]
            placed = True
            i = j + 1
            continue
        out.append(lines[i])
        i += 1
    if not placed:
        body = [start] + block.rstrip("\n").split("\n") + [end]
        try:
            h = next(n for n, ln in enumerate(out) if ln.strip() == "## " + heading)
            nxt = next((n for n in range(h + 1, len(out)) if out[n].startswith("## ")), len(out))
            seg = [ln for ln in out[h + 1:nxt] if ln.strip() != PLACEHOLDER]
            while seg and not seg[-1].strip():
                seg.pop()
            out = out[:h + 1] + seg + [""] + body + [""] + out[nxt:]
        except StopIteration:
            out += ["", "## " + heading, ""] + body
    return "\n".join(out).rstrip("\n") + "\n"


def epic_of(run: Path, data: dict) -> str:
    args = data.get("args") if isinstance(data.get("args"), dict) else {}
    return str(args.get("epic") or data.get("epic") or "").strip()


def ensure_doc(cfg: dict, cat: dict, root: Path, doc_id: str):
    doc = next((d for d in resolve_docs(cfg, cat) if d["id"] == doc_id), None)
    if doc is None:
        return None, None
    p = doc_path(cfg, root, doc)
    if not p.is_file():
        write_lf(p, skeleton(doc, {}))
    return doc, p


def write_matrix(root: Path, run: Path) -> tuple[int, list[str]]:
    cfg, note = load_config(root)
    if cfg is None:
        return 0, [note]
    cat = load_catalog(root)
    doc, p = ensure_doc(cfg, cat, root, MATRIX_ID)
    if doc is None:
        return 0, ["交付設定沒有列需求追溯矩陣（%s）；要加請跑 deliverables.py configure --add %s --write" % (MATRIX_ID, MATRIX_ID)]
    data = load_json(run / "run.json") or {}
    block, gaps = matrix_block(run, epic_of(run, data) or "這次交付")
    hits = privacy_hits(block, cfg.get("privacy_allow") if isinstance(cfg.get("privacy_allow"), list) else [], [run_name_rule(root)])
    if hits:
        return 1, ["追溯矩陣的內容有隱私字串，沒有寫入：" + "；".join(hits[:5])]
    sec = (doc.get("sections") or ["追溯矩陣"])[0]
    try:
        write_lf(p, upsert_entry(p.read_text(encoding="utf-8", errors="replace"), entry_key(run), block, sec))
    except ValueError as e:
        return 1, ["需求追溯矩陣沒有寫入：%s" % e]
    return 0, ["需求追溯矩陣：%s" % posix(p.relative_to(root))] + ["  [缺] " + g for g in gaps]


# ------------------------------------------------------------------ sync
def record_block(run: Path, data: dict, summary: str, show_approvers: bool = False) -> str:
    title = epic_of(run, data) or "這次交付"
    lines = ["### " + title, ""]
    body = re.sub(r"^#\s+.*\n+", "", summary.replace("\r\n", "\n").strip(), count=1)
    body = re.sub(r"^(#{1,3})\s", lambda m: "#" * min(6, len(m.group(1)) + 3) + " ", body, flags=re.M)     # 標題降階，不打亂文件結構
    lines += [body or "（沒有交付摘要）", "", "#### 核准紀錄", "", "| 階段 | 核准者 | 時間 |", "|---|---|---|"]
    names = {"plan": "規劃", "design": "設計", "build": "實作", "sync": "同步", "test": "驗證", "deliver": "交付", "merge": "合併"}
    any_row = False
    for st, rec in (data.get("stages") or {}).items():
        ap = rec.get("approved") if isinstance(rec, dict) else None
        if isinstance(ap, dict):
            who = "依政策自動核准" if ap.get("kind") == "policy" or ap.get("by") == "policy" else (_cell(ap.get("by"), 40) if show_approvers else "人工核准")
            lines.append("| %s | %s | %s |" % (names.get(st, _cell(st, 20)), who, _cell(str(ap.get("at") or "")[:10], 12)))
            any_row = True
    if not any_row:
        lines.append("| — | — | — |")
    return "\n".join(lines) + "\n"


def stamp(root: Path, cfg: dict, cat: dict, repo: Path | None, reviewed=(), only=None) -> tuple[list[str], list[str]]:
    """更新來源紀錄。內文改過的小節記下現在的來源；來源改了而內文沒動的不記（仍然過期），
    除非人用 --reviewed <文件 id>#<小節> 表示「看過了，內容不用改」。回傳（記錄了哪些, 仍然過期的）。
    only（{"<文件 id>#<小節>"…}）：只記這些；其他改過的小節列為「清單外改動」，不記（流程自動同步時用 workflow 回傳的受影響清單）。"""
    src_root = source_root(cfg, root, repo)
    files = repo_files(src_root)
    state = load_state(root)
    ok = set(reviewed or ())
    done, stale = [], []
    for doc in resolve_docs(cfg, cat):
        if doc.get("source") == "external":
            continue
        p = doc_path(cfg, root, doc)
        if not p.is_file():
            continue
        for s in doc_sections(doc, p.read_text(encoding="utf-8", errors="replace"))["sections"]:
            st, why = section_state(doc["id"], s, state, files, src_root)
            key = "%s#%s" % (doc["id"], s["title"])
            if only is not None and st == "unstamped" and key not in only and key not in ok:
                stale.append("%s：清單外改動（這次沒有要更新這一節，內文卻改了；人看過後用 --reviewed 記錄）" % key)
                continue
            if st == "unstamped" or (st == "stale" and key in ok):
                state.setdefault(doc["id"], {})[s["title"]] = {"sources": section_sources(s, files, src_root), "body": body_hash(s["body"])}
                done.append(key)
            elif st == "stale":
                stale.append("%s：%s" % (key, why))
    if done:
        save_state(root, state)
    return done, stale


def sync(root: Path, run: Path, repo: Path | None = None, reviewed=(), only=None) -> tuple[int, list[str]]:
    cfg, note = load_config(root)
    if cfg is None:
        return 0, [note]
    cat = load_catalog(root)
    data = load_json(run / "run.json")
    if not isinstance(data, dict):
        return 1, ["讀不到 %s" % posix(run / "run.json")]
    notes: list[str] = []
    allow = cfg.get("privacy_allow") if isinstance(cfg.get("privacy_allow"), list) else []
    doc, p = ensure_doc(cfg, cat, root, RECORD_ID)
    sp = run / "deliver" / SUMMARY_NAME
    if doc is not None:
        summary = sp.read_text(encoding="utf-8", errors="replace") if sp.is_file() else ""
        if not summary:
            notes.append("這個 run 沒有交付摘要（%s），功能開發紀錄只記核准紀錄" % posix(sp))
        block = record_block(run, data, summary, cfg.get("show_approvers") is True)
        hits = privacy_hits(block, allow, [run_name_rule(root)])
        if hits:
            return 1, ["交付摘要有隱私字串，沒有寫入功能開發紀錄：" + "；".join(hits[:5])]
        sec = (doc.get("sections") or ["開發紀錄"])[0]
        try:
            write_lf(p, upsert_entry(p.read_text(encoding="utf-8", errors="replace"), entry_key(run), block, sec))
        except ValueError as e:
            return 1, ["功能開發紀錄沒有寫入：%s" % e]
        notes.append("功能開發紀錄：%s" % posix(p.relative_to(root)))
    if any(d["id"] == MATRIX_ID for d in resolve_docs(cfg, cat)):
        code, mnotes = write_matrix(root, run)
        notes += mnotes
        if code:
            return code, notes
    if only is not None:      # 工具自己寫的兩份（開發紀錄、追溯矩陣）也算在這次要更新的範圍
        only = set(only) | {"%s#%s" % (i, t) for i in (RECORD_ID, MATRIX_ID) for d in resolve_docs(cfg, cat) if d["id"] == i for t in (d.get("sections") or [])}
    done, stale = stamp(root, cfg, cat, repo, reviewed, only)
    if done:
        notes.append("記錄來源：" + "、".join(done[:12]) + ("…" if len(done) > 12 else ""))
    notes += ["  [過期] " + s for s in stale]
    write_lf(out_dir(cfg, root) / INDEX_NAME, index_text(cfg, cat))
    return 0, notes


# ------------------------------------------------------------------ configure
def _norm(s: str) -> str:
    return re.sub(r"[\s\-_／/、（）()\[\]【】「」:：.。·]+", "", str(s or "")).lower()


def match_title(line: str, cat: dict):
    """合約或客戶清單的一行 → 型錄的文件 id；對不到回 None。"""
    key = _norm(re.sub(r"^\s*(?:[-*•]|\d+[.)、]|[（(]?[一二三四五六七八九十]+[)）、.])\s*", "", line))
    if not key:
        return None
    best = None
    for d in cat.get("documents", []):
        names = [_norm(d.get("title"))] + [_norm(a) for a in d.get("aliases") or []]
        for n in names:
            if not n:
                continue
            if n == key:
                return d["id"]
            if len(n) >= 2 and (n in key or (len(key) >= 3 and key in n)) and (best is None or len(n) > best[0]):
                best = (len(n), d["id"])
    return best[1] if best else None


def past_modifiers(root: Path) -> set:
    guard = load_json(root / ".claude" / "teams" / "guard.config.json") or {}
    runs = root / (safe_rel(guard.get("runs_dir") if isinstance(guard, dict) else "") or "_tpm/runs")
    mods: set = set()
    if runs.is_dir():
        for rj in sorted(runs.glob("*/run.json"))[-200:]:
            rec = (load_json(rj) or {}).get("recipe")
            res = rec.get("resolved") if isinstance(rec, dict) and isinstance(rec.get("resolved"), dict) else {}
            mods |= {m for m in (res.get("modifiers") or (rec or {}).get("modifiers") or []) if isinstance(m, str)} if isinstance(rec, dict) else set()
    return mods


def suggest(root: Path, cat: dict, repo: Path | None = None) -> tuple[list[dict], list[dict]]:
    """依 repo 實況建議要交的文件。回傳（建議, 不建議），每筆都有理由。"""
    files = repo_files(repo or root)
    mods = past_modifiers(root)
    yes, no = [], []
    for d in cat.get("documents", []):
        sug = d.get("suggest")
        if not isinstance(sug, dict):
            continue
        hit = match_files(files, sug.get("paths") or [])
        mod = sorted(set(sug.get("modifiers") or []) & mods)
        if sug.get("always"):
            yes.append({"id": d["id"], "title": d["title"], "reason": sug.get("reason") or ""})
        elif hit:
            yes.append({"id": d["id"], "title": d["title"], "reason": "%s（例如 %s）" % (sug.get("reason") or "repo 有相關的檔", hit[0])})
        elif mod:
            yes.append({"id": d["id"], "title": d["title"], "reason": "過去的 run 用過 %s" % "、".join(mod)})
        else:
            no.append({"id": d["id"], "title": d["title"], "reason": "repo 裡找不到相關的檔（%s）" % "、".join((sug.get("paths") or ["—"])[:2])})
    return yes, no


def configure(root: Path, argv: list[str]) -> tuple[int, dict, list[str]]:
    cat = load_catalog(root)
    idx = catalog_index(cat)
    cur = load_json(root / ".claude" / "teams" / CONFIG_NAME)
    cur = cur if isinstance(cur, dict) else {}
    chosen: dict = {d["id"]: dict(d) for d in cur.get("documents") or [] if isinstance(d, dict) and isinstance(d.get("id"), str)}
    reasons: dict = {}
    notes: list[str] = []
    out: dict = {"unmatched": [], "suggested": [], "not_suggested": []}
    preset = _opt(argv, "--preset")
    if preset:
        p = next((x for x in cat.get("presets", []) if x.get("id") == preset), None)
        if p is None:
            return 2, out, ["沒有這個情境組合：%s（有：%s）" % (preset, "、".join(x["id"] for x in cat.get("presets", [])))]
        for i in p["documents"]:
            chosen.setdefault(i, {"id": i})
            reasons.setdefault(i, "情境組合「%s」" % p.get("title"))
    repo_raw = _opt(argv, "--repo")
    repo = (root / repo_raw) if repo_raw and not Path(repo_raw).is_absolute() else (Path(repo_raw) if repo_raw else None)
    if "--suggest" in argv:
        yes, no = suggest(root, cat, repo)
        out["suggested"], out["not_suggested"] = yes, no
        for s in yes:
            chosen.setdefault(s["id"], {"id": s["id"]})
            reasons.setdefault(s["id"], s["reason"])
    imp = _opt(argv, "--import")
    if imp:
        try:
            raw = Path(imp).read_text(encoding="utf-8", errors="replace")
        except OSError as e:
            return 2, out, ["清單檔讀不到（%s）" % e]
        n = 0
        for ln in raw.splitlines():
            if not ln.strip() or ln.strip().startswith("#"):
                continue
            got = match_title(ln, cat)
            if got:
                chosen.setdefault(got, {"id": got})
                reasons.setdefault(got, "清單裡的「%s」" % ln.strip()[:40])
            else:
                out["unmatched"].append(ln.strip())
                if "--custom-unmatched" in argv:
                    n += 1
                    cid = "custom-%d" % (len([k for k in chosen if k.startswith("custom-")]) + 1)
                    chosen[cid] = {"id": cid, "title": re.sub(r"^\s*(?:[-*•]|\d+[.)、])\s*", "", ln.strip())[:80], "source": "assisted", "sections": ["內容"]}
                    reasons[cid] = "清單裡對不到型錄的項目，新增成自訂文件"
    for i in [x.strip() for x in _opt(argv, "--add").split(",") if x.strip()]:
        if i not in idx and i not in chosen:
            notes.append("型錄沒有 %s；自訂文件請直接在 %s 裡寫 title、source、sections" % (i, CONFIG_NAME))
            continue
        chosen.setdefault(i, {"id": i})
        reasons.setdefault(i, "指定加入")
    for i in [x.strip() for x in _opt(argv, "--remove").split(",") if x.strip()]:
        chosen.pop(i, None)
    order = {d["id"]: n for n, d in enumerate(cat.get("documents", []))}
    docs = sorted(chosen.values(), key=lambda d: (order.get(d["id"], 10 ** 6), d["id"]))
    new = {k: v for k, v in cur.items() if k not in ("documents", "confirmed_by")}
    if _opt(argv, "--dir"):
        new["dir"] = _opt(argv, "--dir")
    new.setdefault("dir", DEFAULT_DIR)
    new["documents"] = docs
    errs = config_errors(new)
    if errs:
        return 2, out, errs
    out["config"] = new
    out["documents"] = [{"id": d["id"], "title": d.get("title") or (idx.get(d["id"]) or {}).get("title"), "source": d.get("source") or (idx.get(d["id"]) or {}).get("source"),
                         "reason": reasons.get(d["id"], "已在交付設定裡")} for d in docs]
    if "--write" in argv:
        by = _opt(argv, "--by")
        if not by:
            return 2, out, ["--write 要帶 --by <誰確認的>：交付設定要有人確認才寫入"]
        new["confirmed_by"] = by
        write_lf(root / ".claude" / "teams" / CONFIG_NAME, json.dumps(new, ensure_ascii=False, indent=1) + "\n")
        out["written"] = True
        notes.append("已寫入 .claude/teams/%s（%d 份文件）。接著跑 deliverables.py init 建骨架。" % (CONFIG_NAME, len(docs)))
    else:
        out["written"] = False
        notes.append("這是提案，還沒寫入。確認後加 --write --by <誰> 重跑同一個指令。")
    return 0, out, notes


# ------------------------------------------------------------------ init／export
def init_docs(root: Path) -> tuple[int, list[str]]:
    cfg, note = load_config(root)
    if cfg is None:
        return 0, [note]
    cat = load_catalog(root)
    made = []
    for d in resolve_docs(cfg, cat):
        if d.get("source") == "external":
            ext = out_dir(cfg, root) / "external" / d["id"]
            if not ext.is_dir():
                ext.mkdir(parents=True, exist_ok=True)
                write_lf(ext / ".gitkeep", "")
                made.append(posix(ext.relative_to(root)) + "/")
            continue
        p = doc_path(cfg, root, d)
        if not p.is_file():
            write_lf(p, skeleton(d, {}))
            made.append(posix(p.relative_to(root)))
    write_lf(out_dir(cfg, root) / INDEX_NAME, index_text(cfg, cat))
    return 0, ["建立 %d 個：%s" % (len(made), "、".join(made[:20]) + ("…" if len(made) > 20 else "")) if made else "文件都已經在，沒有新增",
               "總目錄：%s" % posix((out_dir(cfg, root) / INDEX_NAME).relative_to(root))]


def export(root: Path, out: str = "", pandoc: str | None = "") -> tuple[int, list[str]]:
    cfg, note = load_config(root)
    if cfg is None:
        return 0, [note]
    exe = shutil.which("pandoc") if pandoc == "" else pandoc
    if not exe:
        return 0, ["找不到 pandoc，沒有轉檔。安裝方式：macOS `brew install pandoc`、Windows `winget install JohnMacFarlane.Pandoc`、"
                   "Linux 用套件管理工具安裝 pandoc（https://pandoc.org/installing.html）。Markdown 檔本身可以直接交付。"]
    cat = load_catalog(root)
    dest = Path(out) if out else out_dir(cfg, root) / "_export"
    dest.mkdir(parents=True, exist_ok=True)
    notes = []
    for d in resolve_docs(cfg, cat):
        p = doc_path(cfg, root, d)
        if d.get("source") == "external" or not p.is_file():
            continue
        target = dest / (re.sub(r'[\\/:*?"<>|]+', "-", str(d.get("title") or d["id"])) + ".docx")
        try:
            r = subprocess.run([exe, str(p), "-f", "markdown", "-o", str(target)], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
            notes.append("%s → %s" % (d["id"], posix(target)) if r.returncode == 0 else "%s 轉檔失敗：%s" % (d["id"], (r.stderr or "").strip()[:200]))
        except (OSError, subprocess.SubprocessError) as e:
            notes.append("%s 轉檔失敗：%s" % (d["id"], e))
    return 0, notes or ["沒有可以轉的文件"]


# ------------------------------------------------------------------ CLI
def _opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def _opts(argv: list[str], name: str) -> list[str]:
    return [argv[i + 1] for i, a in enumerate(argv) if a == name and i + 1 < len(argv)]


def _repo_of(root: Path, argv: list[str], run: Path | None = None) -> Path | None:
    raw = _opt(argv, "--repo")
    if not raw and run is not None:
        raw = str((load_json(run / "run.json") or {}).get("target_repo") or "")
    if not raw:
        return None
    p = Path(raw)
    return p if p.is_absolute() else root / p


def main(argv: list[str], root: Path = PROJECT) -> int:
    _utf8()
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    if argv[0] == "--selftest":
        return selftest()
    cmd, rest = argv[0], argv[1:]
    as_json = "--json" in rest
    run = Path(_opt(rest, "--run")) if _opt(rest, "--run") else None
    if cmd in ("affected", "matrix", "sync") and run is None:
        print("缺 --run <run 目錄>")
        return 2
    if cmd == "configure":
        code, out, notes = configure(root, rest)
        if as_json:
            print(json.dumps({**out, "notes": notes}, ensure_ascii=False, indent=1))
            return code
        for d in out.get("documents", []):
            print("  %-24s %s（%s）— %s" % (d["id"], d["title"], d["source"], d["reason"]))
        for s in out.get("not_suggested", []):
            print("  [不建議] %s：%s" % (s["title"], s["reason"]))
        for u in out.get("unmatched", []):
            print("  [對不到型錄] %s" % u)
        for n in notes:
            print(n)
        return code
    if cmd == "init":
        code, notes = init_docs(root)
    elif cmd == "index":
        cfg, note = load_config(root)
        if cfg is None:
            code, notes = 0, [note]
        else:
            write_lf(out_dir(cfg, root) / INDEX_NAME, index_text(cfg, load_catalog(root)))
            code, notes = 0, ["總目錄：%s" % posix((out_dir(cfg, root) / INDEX_NAME).relative_to(root))]
    elif cmd == "status":
        res, note = status_all(root, _repo_of(root, rest, run))
        if as_json:
            print(json.dumps({"enabled": res is not None, "note": note, "documents": res or []}, ensure_ascii=False, indent=1))
            return 0
        if res is None:
            print(note)
            return 0
        for d in res:
            print("  [%s] %s（%s）%s" % (STATUS_TEXT[d["status"]], d["title"], d["id"], "：" + d["detail"] if d["detail"] else ""))
        print("共 %d 份：" % len(res) + "、".join("%s %d" % (STATUS_TEXT[k], sum(1 for d in res if d["status"] == k)) for k in STATUS_TEXT))
        return 0
    elif cmd == "check":
        errs, note = check_all(root, _repo_of(root, rest, run))
        if errs is None:
            print(note)
            return 0
        for e in errs:
            print("  [問題] " + e)
        print("交付文件檢查：%s" % ("通過" if not errs else "%d 個問題" % len(errs)))
        return 1 if errs else 0
    elif cmd == "affected":
        paths = None
        pf = _opt(rest, "--paths-file")
        if pf:
            try:
                paths = [x for x in Path(pf).read_text(encoding="utf-8", errors="replace").splitlines() if x.strip()]
            except OSError as e:
                print("改動清單讀不到（%s）" % e)
                return 2
        else:
            repo = _repo_of(root, rest, run)
            data = load_json(run / "run.json") or {}
            args = data.get("args") if isinstance(data.get("args"), dict) else {}
            paths = changed_paths(repo, _opt(rest, "--base") or str(args.get("base") or "origin/main")) if repo is not None else []
        res = affected(root, run, paths, "--milestone" in rest, "--close" in rest)
        if as_json:
            print(json.dumps(res, ensure_ascii=False, indent=1))
            return 0
        if not res["enabled"]:
            print(res["note"])
            return 0
        for a in res["affected"]:
            print("  [更新] %s：%s（%s）" % (a["title"], "、".join(a["sections"]), a["why"]))
        for d in res["deferred"]:
            print("  [這次不動] %s（%s 才更新）" % (d["title"], d["when"]))
        for h in res["hints"]:
            print("  [建議加入] %s（%s）" % (h["title"], h["why"]))
        if not (res["affected"] or res["deferred"] or res["hints"]):
            print("這次沒有受影響的交付文件")
        return 0
    elif cmd == "matrix":
        if "--stdout" in rest:
            block, gaps = matrix_block(run, epic_of(run, load_json(run / "run.json") or {}) or "這次交付")
            print(block)
            for g in gaps:
                print("[缺] " + g)
            return 0
        code, notes = write_matrix(root, run)
    elif cmd == "sync":
        code, notes = sync(root, run, _repo_of(root, rest, run), _opts(rest, "--reviewed"))
    elif cmd == "export":
        code, notes = export(root, _opt(rest, "--out"))
    else:
        print("不認得的指令：%s" % cmd)
        return 2
    for n in notes:
        print(n)
    return code


# ------------------------------------------------------------------ selftest
def selftest() -> int:
    results: list[tuple[bool, str, str]] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        results.append((bool(ok), name, detail))

    def write(p: Path, text: str) -> None:
        write_lf(p, text)

    cat_src = load_json(TOOLS.parent / CATALOG_REL)
    with tempfile.TemporaryDirectory(prefix="awf-deliv-") as td:
        root = Path(td).resolve()
        teams = root / ".claude" / "teams"
        write(teams / CATALOG_REL, json.dumps(cat_src, ensure_ascii=False))
        cat = load_catalog(root)

        # ---- 型錄
        groups = [g["id"] for g in cat["groups"]]
        docs = cat["documents"]
        check("型錄：八組都有文件", len(groups) == 8 and all(any(d["group"] == g for d in docs) for g in groups), str(groups))
        check("型錄：每筆都有來源類型、標題、更新時機", all(d.get("source") in SOURCE_TYPES and d.get("title") and d.get("when") in WHENS for d in docs))
        check("型錄：id 不重複、至少 40 種", len({d["id"] for d in docs}) == len(docs) >= 40, str(len(docs)))
        check("型錄：三種來源類型都有", {d["source"] for d in docs} == set(SOURCE_TYPES))
        check("型錄：情境組合五個、列的文件都在型錄裡", len(cat["presets"]) == 5 and all(i in catalog_index(cat) for p in cat["presets"] for i in p["documents"]))
        check("型錄：非外部文件都有小節", all(d["sections"] for d in docs if d["source"] != "external"))

        # ---- 沒有交付設定：什麼都不做
        cfg, note = load_config(root)
        check("沒有交付設定：load_config 回 None 並說明", cfg is None and "configure" in note, note)
        check("沒有交付設定：status、check、init、sync 都不做事", status_all(root)[0] is None and check_all(root)[0] is None and init_docs(root)[0] == 0
              and not (root / DEFAULT_DIR).exists())
        run = root / "_tpm" / "runs" / "r1"
        write(run / "run.json", json.dumps({"requirement": "訂單匯出", "target_repo": ".", "args": {"epic": "export"},
                                            "recipe": {"name": "feature", "modifiers": ["migration"], "resolved": {"name": "feature", "modifiers": ["migration"], "stages": ["plan", "design", "build", "test", "deliver", "merge"]}},
                                            "stages": {"plan": {"approved": {"by": "示範客戶 ACME 的 PM", "at": "2026-01-02T03:04:05Z", "kind": "human"}},
                                                       "test": {"approved": {"by": "policy", "at": "2026-01-03T00:00:00Z", "kind": "policy"}}}}))
        check("沒有交付設定：pack_args 回空的（流程不變）", pack_args("deliver", run, root, None, {}, ["db/migrations/001.sql"]) == ({}, []))
        write(teams / "methodology.json", json.dumps({"deliverables": {"enabled": False, "documents": [{"id": "db-design"}]}}))
        check("profile 的 deliverables 沒有 enabled: true 不算啟用", load_config(root)[0] is None)
        write(teams / "methodology.json", json.dumps({"deliverables": {"enabled": True, "documents": [{"id": "db-design"}]}}))
        check("profile 的 deliverables enabled: true 才啟用", (load_config(root)[0] or {}).get("documents") == [{"id": "db-design"}])
        (teams / "methodology.json").unlink()

        # ---- configure
        code, out, notes = configure(root, ["--preset", "hackathon"])
        check("configure：情境組合可套用", code == 0 and [d["id"] for d in out["documents"]] == ["sys-arch", "release-notes", "delivery-summary"] and all("黑客松" in d["reason"] for d in out["documents"]), str(out.get("documents")))
        check("configure：沒確認不寫入", out["written"] is False and not (teams / CONFIG_NAME).exists() and any("還沒寫入" in n for n in notes))
        check("configure：--write 沒帶 --by 拒絕", configure(root, ["--preset", "hackathon", "--write"])[0] == 2 and not (teams / CONFIG_NAME).exists())
        check("configure：沒有的情境組合回錯誤", configure(root, ["--preset", "nope"])[0] == 2)
        write(root / "db/migrations/001_orders.sql", "create table orders (id int);\n")
        write(root / "src/app.py", "print('hi')\n")
        code, out, _ = configure(root, ["--suggest"])
        sug = {s["id"]: s["reason"] for s in out["suggested"]}
        nos = {s["id"]: s["reason"] for s in out["not_suggested"]}
        check("configure：有 migration 才建議資料庫設計文件（附理由與例子）", "db-design" in sug and "001_orders.sql" in sug["db-design"], str(sug))
        check("configure：沒有 API 定義不建議 API 規格書（附理由）", "api-spec" in nos and "找不到" in nos["api-spec"] and "api-spec" not in sug, str(nos.get("api-spec")))
        check("configure：沒有排程不建議批次作業設計", "batch-design" in nos and "batch-design" not in sug)
        write(root / "api/openapi.yaml", "openapi: 3.0.0\n")
        write(root / "_tpm/runs/old/run.json", json.dumps({"recipe": {"resolved": {"modifiers": ["ui"]}}}))
        sug2 = {s["id"]: s["reason"] for s in configure(root, ["--suggest"])[1]["suggested"]}
        check("configure：有 API 定義就建議；用過 ui 的 run 建議畫面規格", "api-spec" in sug2 and "ui-spec" in sug2 and "ui" in sug2["ui-spec"], str(sug2))
        shutil.rmtree(str(root / "_tpm/runs/old"))
        (root / "api/openapi.yaml").unlink()
        write(root / "合約清單.txt", "1. 系統架構設計書\n2. 資料庫規格\n- 測試報告\n3. 專案人員保密切結書\n# 註解\n")
        code, out, _ = configure(root, ["--import", str(root / "合約清單.txt")])
        got = [d["id"] for d in out["documents"]]
        check("configure：匯入清單對到型錄（含別名）", {"sys-arch", "db-design", "test-report"} <= set(got), str(got))
        check("configure：對不到的列出來、不自動新增", out["unmatched"] == ["3. 專案人員保密切結書"] and not any(i.startswith("custom-") for i in got), str(out["unmatched"]))
        code, out, _ = configure(root, ["--import", str(root / "合約清單.txt"), "--custom-unmatched"])
        cust = [d for d in out["config"]["documents"] if d["id"].startswith("custom-")]
        check("configure：--custom-unmatched 才新增成自訂文件", len(cust) == 1 and cust[0]["title"] == "專案人員保密切結書" and cust[0]["source"] == "assisted", str(cust))
        code, out, notes = configure(root, ["--add", "db-design,api-spec,sys-arch,biz-rules,rtm,dev-record,vuln-scan-report,ops-manual,project-plan", "--write", "--by", "示範客戶 ACME 的 PM"])
        cfg, src = load_config(root)
        check("configure：確認後寫入，記下誰確認", code == 0 and out["written"] and cfg and cfg["confirmed_by"] == "示範客戶 ACME 的 PM" and len(cfg["documents"]) == 9, str(notes))
        code, out, _ = configure(root, ["--remove", "biz-rules", "--add", "nope"])
        check("configure：既有設定會保留；--remove 可移除；型錄沒有的 id 不加", [d["id"] for d in out["documents"]].count("biz-rules") == 0 and len(out["documents"]) == 8 and "nope" not in [d["id"] for d in out["documents"]])
        check("configure：路徑不合理的 dir 拒絕", configure(root, ["--dir", "../outside"])[0] == 2 and configure(root, ["--dir", "/abs"])[0] == 2)

        # ---- 設定檢查
        check("設定檢查：id 重複、source 寫錯、sources 跳出專案", len(config_errors({"documents": [{"id": "a"}, {"id": "a", "source": "magic"}, {"id": "b", "sources": ["../x"]}]})) == 3)
        check("設定檢查：正常設定沒有錯", config_errors({"dir": "docs/交付", "documents": [{"id": "db-design", "when": "milestone", "depth": "summary"}]}) == [])

        # ---- init：只建設定裡列的
        code, notes = init_docs(root)
        d_out = root / DEFAULT_DIR
        made = sorted(p.name for p in d_out.iterdir())
        check("init：只建交付設定裡列的文件與總目錄", made == sorted(["README.md", "api-spec.md", "biz-rules.md", "db-design.md", "dev-record.md", "external", "ops-manual.md", "project-plan.md", "rtm.md", "sys-arch.md"]), str(made))
        check("init：外部文件只建收件目錄", (d_out / "external/vuln-scan-report").is_dir() and not (d_out / "vuln-scan-report.md").exists())
        before = (d_out / "db-design.md").read_text(encoding="utf-8")
        write(d_out / "sys-arch.md", (d_out / "sys-arch.md").read_text(encoding="utf-8") + "\n手寫的內容\n")
        init_docs(root)
        check("init：重跑不動已有的檔", "手寫的內容" in (d_out / "sys-arch.md").read_text(encoding="utf-8") and (d_out / "db-design.md").read_text(encoding="utf-8") == before)
        parsed = parse_doc(before)
        check("骨架：檔頭齊全、小節帶來源標記", parsed["meta"].get("id") == "db-design" and parsed["meta"].get("source") == "auto" and [s["title"] for s in parsed["sections"]][:2] == ["資料表與欄位", "關聯與索引"]
              and "**/migrations/**" in parsed["sections"][0]["sources"], str(parsed["meta"]))

        # ---- status 四種狀態
        def status_of() -> dict:
            return {d["id"]: d for d in status_all(root)[0]}

        st = status_of()
        check("status：外部文件沒附是「等人補」", st["vuln-scan-report"]["status"] == "waiting")
        check("status：人補的文件還沒寫是「等人補」", st["ops-manual"]["status"] == "waiting" and "系統概觀" in st["ops-manual"]["detail"], str(st["ops-manual"]))
        check("status：自動文件沒有原料是「缺原料」", st["api-spec"]["status"] == "missing" and "找不到原料" in st["api-spec"]["detail"], str(st["api-spec"]))
        check("status：自動文件有原料還沒產生是「過期」", st["db-design"]["status"] == "stale" and "還沒產生" in st["db-design"]["detail"], str(st["db-design"]))
        write(d_out / "external/vuln-scan-report/報告.pdf", "x")

        def fill(doc_id: str, body: str = "已寫好的內容。") -> None:
            p = d_out / (doc_id + ".md")
            write(p, p.read_text(encoding="utf-8").replace(PLACEHOLDER, body))

        for i in ("db-design", "ops-manual", "sys-arch", "biz-rules", "project-plan", "api-spec"):
            fill(i)
        write(root / "specs/export/design.md", "# design\n\n## 資料設計\n\norders 表。\n")
        write(root / "specs/export/intent.md", "# intent\n\n匯出訂單。\n")
        write(root / "specs/export/spec.md", "# spec\n\n## 匯出\n\nAC-1 匯出 CSV。\n")
        write(root / "api/openapi.yaml", "openapi: 3.0.0\n")
        st = status_of()
        check("status：寫了但沒記錄來源是「過期」（還沒 sync）", st["db-design"]["status"] == "stale" and "還沒記錄來源" in st["db-design"]["detail"], str(st["db-design"]))
        cfg, _ = load_config(root)
        done, stale = stamp(root, cfg, cat, None)
        st = status_of()
        check("status：記錄來源後是「齊全」；外部文件附了也是", st["db-design"]["status"] == "ok" and st["vuln-scan-report"]["status"] == "ok" and st["ops-manual"]["status"] == "ok" and "db-design#資料表與欄位" in done, str(st["db-design"]))
        check("status：四種狀態的文字都有", set(STATUS_TEXT) == {"ok", "stale", "missing", "waiting"})

        # ---- 過期判定
        write(root / "db/migrations/002_items.sql", "create table items (id int);\n")
        st = status_of()
        check("過期：來源多了一個檔、內文沒動 → 過期", st["db-design"]["status"] == "stale" and "002_items.sql" in " ".join(s["detail"] for s in st["db-design"]["sections"]), str(st["db-design"]["detail"]))
        check("過期：沒用到這個來源的文件不受影響", st["biz-rules"]["status"] == "ok" and st["ops-manual"]["status"] == "ok")
        done, stale = stamp(root, cfg, cat, None)
        check("過期：內文沒動時 sync 不會把它記成最新", not any(k.startswith("db-design#") for k in done) and any("db-design#資料表與欄位" in s for s in stale) and status_of()["db-design"]["status"] == "stale", str(done))
        p = d_out / "db-design.md"
        write(p, p.read_text(encoding="utf-8").replace("已寫好的內容。", "已寫好的內容。  \n"))
        done, _ = stamp(root, cfg, cat, None)
        check("過期：只改空白不算更新", not done and status_of()["db-design"]["status"] == "stale")
        text = p.read_text(encoding="utf-8")
        head, _, tail = text.partition("## 關聯與索引")
        write(p, head.replace("已寫好的內容。", "orders 與 items 兩張表。") + "## 關聯與索引" + tail)
        done, stale = stamp(root, cfg, cat, None)
        st = status_of()
        check("過期：只有改寫過的小節變成最新，其他小節仍然過期", done == ["db-design#資料表與欄位"] and st["db-design"]["status"] == "stale" and "資料表與欄位" not in st["db-design"]["detail"], str((done, st["db-design"]["detail"])))
        done, _ = stamp(root, cfg, cat, None, reviewed=["db-design#關聯與索引", "db-design#資料遷移", "db-design#資料保存與清理"])
        check("過期：人用 --reviewed 表示內容不用改才放行", len(done) == 3 and status_of()["db-design"]["status"] == "ok", str(done))
        write(root / "db/migrations/001_orders.sql", "create table orders (id int, total int);\n")
        check("過期：來源檔內容改了 → 過期", status_of()["db-design"]["status"] == "stale")
        state = load_state(root)
        check("來源紀錄放在 .claude/teams（team agent 寫不到），文件裡的標記只有路徑", (teams / STATE_NAME).is_file() and "sha1" not in p.read_text(encoding="utf-8") and len(state["db-design"]["資料表與欄位"]["sources"]) >= 2)
        stamp(root, cfg, cat, None, reviewed=["db-design#%s" % s for s in ("資料表與欄位", "關聯與索引", "資料遷移", "資料保存與清理")])

        # ---- check
        write(d_out / INDEX_NAME, index_text(cfg, cat))
        for i in ("rtm", "dev-record"):
            fill(i, "之後由工具寫入。")
        stamp(root, cfg, cat, None)
        errs, _ = check_all(root)
        check("check：都寫好、來源最新、總目錄一致 → 通過", errs == [], str(errs))
        check("check CLI：通過 exit 0", main(["check"], root) == 0)

        def with_change(doc_id: str, old: str, new: str) -> list[str]:
            pth = d_out / (doc_id + ".md")
            keep = pth.read_text(encoding="utf-8")
            write(pth, keep.replace(old, new, 1))
            got, _ = check_all(root)
            write(pth, keep)
            return got

        check("check：檔頭缺欄位該擋", any("檔頭缺 title" in e for e in with_change("sys-arch", "title: 系統架構設計書\n", "")))
        check("check：檔頭 id 和檔名不一致該擋", any("不一致" in e for e in with_change("sys-arch", "id: sys-arch", "id: other")))
        check("check：把來源標記刪掉不能躲過過期判定（改用文件層級的來源）", (write(root / "db/migrations/009.sql", "x\n"), any("db-design.md〈資料表與欄位〉" in e for e in with_change("db-design", "<!-- awf:source", "<!-- 沒了")))[1])
        (root / "db/migrations/009.sql").unlink()
        check("check：過期小節該擋", any("db-design.md〈資料表與欄位〉" in e for e in with_change("db-design", "<!-- awf:source", "<!-- awf:source src/**;")))
        check("check：隱私字串（信箱）該擋，而且不印內容", any("電子郵件" in e for e in with_change("sys-arch", "已寫好的內容。", "聯絡 someone@example.com。")) and not any("someone@" in e for e in with_change("sys-arch", "已寫好的內容。", "聯絡 someone@example.com。")))
        check("check：隱私字串（家目錄路徑、金鑰、內部 agent 名稱）該擋", all(any(kind in e for e in with_change("sys-arch", "已寫好的內容。", text)) for kind, text in (
            ("本機路徑", "放在 /" + "home/demo/app/ 底下。"), ("憑證或金鑰", "金鑰 ghp_" + "a" * 30 + "。"), ("內部 agent 名稱", "由 dev-" + "implementer 完成。"), ("內部網路位址或主機", "主機 10.1.2.3。"))))
        check("check：一般內容不誤擋", check_all(root)[0] == [])
        check("check：失效連結該擋、有效連結與外部連結該放", any("連結對不到檔" in e for e in with_change("sys-arch", "已寫好的內容。", "見 [資料庫](nope.md)。"))
              and with_change("sys-arch", "已寫好的內容。", "見 [資料庫](db-design.md#x) 與 [官網](https://example.com/a)。") == [])
        write(root / "docs/禁用字.txt", "# 內部代號\nProject\\s*Falcon\n[壞掉\n")
        cfg2 = dict(cfg, banned_terms_file="docs/禁用字.txt")
        write(teams / CONFIG_NAME, json.dumps(cfg2, ensure_ascii=False))
        hit = with_change("sys-arch", "已寫好的內容。", "代號 Project Falcon。")
        check("check：禁用字（專案自己的清單）該擋，而且不印內容", any("出現禁用字" in e for e in hit) and not any("Falcon" in e for e in hit), str(hit))
        check("check：禁用字清單寫壞的那一行略過並說明、其他照常", any("寫壞的正規式" in e for e in check_all(root)[0]))
        write(root / "docs/禁用字.txt", "Project\\s*Falcon\n")
        check("check：沒出現禁用字該放", check_all(root)[0] == [])
        write(d_out / INDEX_NAME, index_text(cfg, cat) + "\n手改\n")
        check("check：總目錄被改過該擋；index 重寫後通過", any("總目錄和交付設定不一致" in e for e in check_all(root)[0]) and main(["index"], root) == 0 and check_all(root)[0] == [])
        check("check CLI：有問題 exit 1", (write(d_out / INDEX_NAME, "x"), main(["check"], root))[1] == 1 and main(["index"], root) == 0)
        other = d_out / "不在設定裡.md"
        write(other, "聯絡 someone@example.com\n")
        check("check：只檢查交付設定裡列的文件", check_all(root)[0] == [])
        other.unlink()
        write(teams / CONFIG_NAME, json.dumps(dict(cfg2, privacy_allow=["support@example\\.com"]), ensure_ascii=False))
        check("check：privacy_allow 列的字串放行，其他照擋", with_change("sys-arch", "已寫好的內容。", "客服 support@example.com。") == []
              and any("電子郵件" in e for e in with_change("sys-arch", "已寫好的內容。", "聯絡 other@example.com。")))

        # ---- 受影響的文件
        res = affected(root, run, ["db/migrations/003_x.sql", "src/app.py"])
        ids = [a["id"] for a in res["affected"]]
        dbd = next(a for a in res["affected"] if a["id"] == "db-design")
        check("受影響：diff 有 migration → 只更新資料庫設計文件的對應小節", "db-design" in ids and dbd["sections"] == ["資料表與欄位", "關聯與索引", "資料遷移"] and "api-spec" not in ids and "sys-arch" not in ids, str(ids))
        write(run / "run.json", json.dumps(dict(load_json(run / "run.json"), recipe={"name": "feature", "modifiers": ["public_api"], "resolved": {"name": "feature", "modifiers": ["public_api"], "stages": ["plan", "design", "build", "test", "deliver"]}})))
        res = affected(root, run, ["src/app.py"])
        ids = [a["id"] for a in res["affected"]]
        check("受影響：有 public_api → 更新 API 規格書，資料庫設計文件不動", "api-spec" in ids and "db-design" not in ids, str(ids))
        check("受影響：業務規則文件在有 plan 段的 run 都更新（intent 可能改了）", "biz-rules" in ids)
        check("受影響：追溯矩陣、開發紀錄、外部文件不派給 agent", not {"rtm", "dev-record", "vuln-scan-report"} & set(ids))
        check("受影響：when 是里程碑的文件在一般 run 不更新，列在 deferred", "project-plan" not in ids and "ops-manual" not in ids)
        write(teams / CONFIG_NAME, json.dumps(dict(cfg2, documents=cfg2["documents"] + [{"id": "integration-spec"}]), ensure_ascii=False))
        res = affected(root, run, ["src/app.py"])
        res_m = affected(root, run, ["src/app.py"], milestone=True)
        check("受影響：里程碑文件平常列 deferred，--milestone 才更新", [d["id"] for d in res["deferred"]] == ["integration-spec"] and "integration-spec" in [a["id"] for a in res_m["affected"]], str(res["deferred"]))
        write(teams / CONFIG_NAME, json.dumps(cfg2, ensure_ascii=False))
        res = affected(root, run, ["jobs/nightly.py", "src/app.py"])
        check("受影響：碰到新領域（第一次出現排程）只提示、不自動加", [h["id"] for h in res["hints"]] == ["batch-design"] and "batch-design" not in [a["id"] for a in res["affected"]]
              and "batch-design" not in [d["id"] for d in load_config(root)[0]["documents"]], str(res["hints"]))
        res = affected(root, run, ["README.md"])
        check("受影響：只改文件的 run 不動設計文件", not {"db-design", "api-spec", "sys-arch"} & {a["id"] for a in res["affected"]} or "api-spec" in [a["id"] for a in res["affected"]])
        extra, notes = pack_args("design", run, root, None, load_json(run / "run.json"))
        check("pack_args（design）：列出 design.md 要有的小節", {d["id"]: d["sections"] for d in extra["design_sections"]} == {"sys-arch": ["系統設計"], "db-design": ["資料設計"], "api-spec": ["API 設計"]}, str(extra))
        extra, notes = pack_args("deliver", run, root, None, load_json(run / "run.json"), ["jobs/nightly.py", "api/openapi.yaml"])
        check("pack_args（deliver）：受影響的文件、交付摘要路徑、新領域提示", [a["id"] for a in extra["affected"]] == ["biz-rules", "api-spec"] and extra["summary"].endswith("/deliver/" + SUMMARY_NAME)
              and extra["hints"][0]["id"] == "batch-design" and any("建議把「批次作業設計文件」加進交付設定" in n for n in notes), str(extra)[:400])
        extra2, _ = pack_args("deliver", run, root, None, load_json(run / "run.json"), ["README.md"])
        (d_out / "api-spec.md").unlink()
        pack_args("deliver", run, root, None, load_json(run / "run.json"), ["api/openapi.yaml"])
        check("pack_args（deliver）：有交付設定就帶交付摘要路徑；受影響的文件還沒建立時先放骨架", extra2.get("summary", "").endswith(SUMMARY_NAME) and (d_out / "api-spec.md").is_file())
        fill("api-spec")
        stamp(root, cfg, cat, None)
        check("pack_args：build 等其他段不帶", pack_args("build", run, root, None, {}) == ({}, []))
        a1 = json.dumps(affected(root, run, ["jobs/nightly.py"]), ensure_ascii=False, sort_keys=True)
        check("受影響：結果可重現", a1 == json.dumps(affected(root, run, ["jobs/nightly.py"]), ensure_ascii=False, sort_keys=True))

        # ---- 需求追溯矩陣
        write(run / "design.json", json.dumps({"spec": {"payload": {"subtasks": [
            {"id": "t1", "title": "匯出", "acceptance": ["AC-1 匯出 CSV", "AC-2"], "spec_refs": ["spec.md#匯出"], "story_ids": ["S-12"]},
            {"id": "t2", "title": "排程", "acceptance": ["AC-3"], "spec_refs": []}]}}}))
        write(run / "test.json", json.dumps({"stage": "test", "verifier": {"status": "ok", "payload": {"commit": "0123456789abcdef0123", "scope_covered": "全部", "scope_skipped": "none", "criteria": [
            {"id": "AC-1", "criterion": "匯出 CSV", "command": "pytest tests/test_export.py", "result": "pass", "evidence": "3 passed"},
            {"id": "s1:AC-2", "criterion": "空資料時回空檔", "command": "pytest -k empty", "result": "pass", "evidence": "1 passed"},
            {"id": "AC-3", "criterion": "每天 02:00 執行", "command": "pytest -k cron", "result": "fail", "evidence": "1 failed"},
            {"id": "AC-9", "criterion": "沒人認領的條件", "command": "", "result": "not_verifiable", "evidence": ""}]}}}))
        rows, gaps = matrix_rows(run)
        by = {r["id"]: r for r in rows}
        check("追溯矩陣：每條 AC 對到 spec 小節、驗證指令、證據、commit", by["AC-1"]["spec"] == ["spec.md#匯出"] and by["AC-1"]["stories"] == ["S-12"] and by["AC-1"]["command"] and by["AC-1"]["evidence"] == "3 passed"
              and by["AC-1"]["commit"].startswith("0123456789ab") and by["AC-1"]["gaps"] == [], str(by["AC-1"]))
        check("追溯矩陣：分片前綴的 id 也對得到", by["s1:AC-2"]["spec"] == ["spec.md#匯出"] and by["s1:AC-2"]["gaps"] == [])
        check("追溯矩陣：缺一環時標出來（沒有 spec 小節、沒通過、沒有指令與證據）", any(g.startswith("AC-3：") and "缺spec 小節" in g and "結果是 fail" in g for g in gaps)
              and any(g.startswith("AC-9：") and "缺驗證指令" in g and "缺證據" in g for g in gaps) and len(gaps) == 2, str(gaps))
        code, notes = write_matrix(root, run)
        rtm = (d_out / "rtm.md").read_text(encoding="utf-8")
        check("追溯矩陣：寫進文件，缺的環節印出來", code == 0 and "| AC-1 | 匯出 CSV | S-12 | spec.md#匯出 | `pytest test_export.py` | 通過 | 3 passed | 0123456789ab | 無 |" in rtm and any("[缺] AC-3" in n for n in notes), rtm[-600:])
        write_matrix(root, run)
        check("追溯矩陣：重跑結果相同（同一個 run 只有一段）", (d_out / "rtm.md").read_text(encoding="utf-8") == rtm and rtm.count("<!-- awf:entry ") == 1)
        check("追溯矩陣：不寫 run 目錄名稱（用雜湊當標記）", "r1" not in re.sub(r"[0-9a-f]{12}", "", rtm.split("## 追溯矩陣")[1]))
        empty = root / "_tpm" / "runs" / "r0"
        write(empty / "run.json", "{}")
        check("追溯矩陣：沒有驗證結果時說明、不當掉", "沒有驗證結果" in matrix_rows(empty)[1][0] and "沒有驗證結果" in matrix_block(empty, "x")[0])

        # ---- sync
        write(run / "deliver" / SUMMARY_NAME, "# 交付摘要\n\n## 這次交付了什麼\n\n訂單可以匯出成 CSV。\n")
        code, notes = sync(root, run)
        rec = (d_out / "dev-record.md").read_text(encoding="utf-8")
        check("sync：交付摘要與核准紀錄併進功能開發紀錄", code == 0 and "訂單可以匯出成 CSV。" in rec and "| 規劃 | 人工核准 | 2026-01-02 |" in rec and "ACME 的 PM" not in rec and "| 驗證 | 依政策自動核准 | 2026-01-03 |" in rec, rec[-500:])
        snap = {p.name: p.read_text(encoding="utf-8") for p in d_out.glob("*.md")}
        state1 = (teams / STATE_NAME).read_text(encoding="utf-8")
        code, _ = sync(root, run)
        check("sync：重跑結果相同", code == 0 and snap == {p.name: p.read_text(encoding="utf-8") for p in d_out.glob("*.md")} and state1 == (teams / STATE_NAME).read_text(encoding="utf-8"))
        check("sync：交付摘要的標題降階，不打亂文件結構", "##### 這次交付了什麼" in rec and [s["title"] for s in parse_doc(rec)["sections"]] == ["開發紀錄"])
        write(run / "deliver" / SUMMARY_NAME, "# 交付摘要\n\n有問題請寄 someone@example.org。\n")
        code, notes = sync(root, run)
        check("sync：寫入前掃隱私字串，命中就停、不寫", code == 1 and "隱私字串" in notes[0] and (d_out / "dev-record.md").read_text(encoding="utf-8") == snap["dev-record.md"], str(notes))
        write(run / "deliver" / SUMMARY_NAME, "# 交付摘要\n\n## 這次交付了什麼\n\n訂單可以匯出成 CSV。\n")
        (run / "deliver" / SUMMARY_NAME).unlink()
        code, notes = sync(root, run)
        check("sync：沒有交付摘要時說明，仍記核准紀錄", code == 0 and any("沒有交付摘要" in n for n in notes))
        check("sync 之後 check 通過", check_all(root)[0] == [], str(check_all(root)[0]))

        # ---- 審查（C9.3）找到的繞過手法
        def cur_state(doc_id: str) -> str:
            return status_of()[doc_id]["status"]

        write(root / "db/migrations/010_audit.sql", "create table audit (id int);\n")
        check("審查：來源改了 → 過期（前提）", cur_state("db-design") == "stale")
        p = d_out / "db-design.md"
        keep = p.read_text(encoding="utf-8")
        write(p, keep.replace("<!-- awf:source specs/**/design.md; **/migrations/**; **/migration/**; **/*.sql; **/schema.prisma -->", "<!-- awf:source nothing/here -->", 1)
              .replace("orders 與 items 兩張表。", "orders 與 items 兩張表，另有稽核表。", 1))
        done, _ = stamp(root, cfg, cat, None)
        write(root / "db/migrations/011_more.sql", "create table more (id int);\n")
        check("審查：把來源標記改成對不到檔的 glob，不能讓文件層級的來源失效", cur_state("db-design") == "stale" and "011_more.sql" in status_of()["db-design"]["detail"] + " ".join(x["detail"] for x in status_of()["db-design"]["sections"]), str(status_of()["db-design"]))
        write(p, keep)
        stamp(root, cfg, cat, None, reviewed=["db-design#%s" % t for t in ("資料表與欄位", "關聯與索引", "資料遷移", "資料保存與清理")])
        write(root / "db/migrations/012.sql", "x\n")
        write(p, keep.replace("orders 與 items 兩張表。", "orders 與 items 兩張表 。", 1))
        check("審查：只插入空白或改標點不算改寫", not any(k.startswith("db-design#") for k in stamp(root, cfg, cat, None)[0]) and cur_state("db-design") == "stale")
        write(p, keep.replace("orders 與 items 兩張表。", "orders、items 與稽核表。", 1).replace("已寫好的內容。", "改了清單外的小節。", 1))
        done, stale = stamp(root, cfg, cat, None, only={"db-design#資料表與欄位"})
        check("審查：自動同步只記受影響清單上的小節，清單外的改動列出來交人", done == ["db-design#資料表與欄位"] and any("清單外改動" in x and "db-design#關聯與索引" in x for x in stale), str((done, stale)))
        write(p, keep)
        stamp(root, cfg, cat, None, reviewed=["db-design#%s" % t for t in ("資料表與欄位", "關聯與索引", "資料遷移", "資料保存與清理")])
        check("審查：.github 開頭的 glob 對得到", match_files([".github/workflows/ci.yml", "x/.github/a"], [".github/workflows/**"]) == [".github/workflows/ci.yml"])
        check("審查：base 不能變成 git 的選項", changed_paths(root, "--output=" + str(root / "pwn")) == [] and not list(root.glob("pwn*")))
        broken = "## 開發紀錄\n\n<!-- awf:entry abcdef123456 -->\n舊內容\n\n## 重要\n\n人寫的內容\n"
        try:
            upsert_entry(broken, "abcdef123456", "新內容", "開發紀錄")
            got_err = False
        except ValueError:
            got_err = True
        check("審查：找不到結尾標記時不寫入（不會吃掉後面的內容）", got_err)
        leak = {"全形＠": "聯絡 a＠example.com", "零寬字元": "a\u200b@example.com", "HTML 實體": "a&#64;example.com", "小寫 Windows 路徑": "c:\\users\\demo\\x",
                "JSON 跳脫的 Windows 路徑": "C:\\\\Users\\\\demo\\\\x", "家目錄縮寫": "~/work/app", "macOS 暫存目錄": "/var/folders/xy/T/awf-logs/a.log",
                "大寫 agent 名稱": "TPM-Writer 寫的", "底線 agent 名稱": "tpm_writer", "全形連字號": "tpm－writer", "localhost": "http://localhost:8080",
                "內網主機": "http://wiki.corp.internal/x", "github_pat": "github_pat_" + "A" * 30, "JWT": "eyJabcdefghij.eyJabcdefghij.abcdefghijk",
                "Bearer": "Bearer " + "a" * 24, "引號裡的密碼": '"password": "hunter2hunter"', "中文密碼": "密碼：hunter2hunter", "跨行信箱": "a@exam\nple.com"}
        missed = [k for k, v in leak.items() if not privacy_hits(v)]
        check("審查：隱私規則抓得到各種變形", not missed, "漏掉：" + "、".join(missed))
        check("審查：隱私規則不誤擋一般內容", privacy_hits("訂單匯出 CSV，版本 1.2.3，見 docs/guide.md 與 https://example.com/a。") == [])
        write(root / "_tpm/runs/2026-11-13-export/run.json", "{}")
        hit = with_change("sys-arch", "已寫好的內容。", "見 2026-11-13-export 的紀錄。")
        check("審查：run 目錄名稱不能出現在交付文件", any("內部 run 目錄" in e for e in hit), str(hit))
        shutil.rmtree(str(root / "_tpm/runs/2026-11-13-export"))
        write(teams / CONFIG_NAME, json.dumps(dict(cfg2, privacy_allow=["support@example\\.com"], show_approvers=True), ensure_ascii=False))
        check("審查：show_approvers 開啟才寫核准者名字", "示範客戶 ACME 的 PM" in record_block(run, load_json(run / "run.json"), "", True) and "ACME" not in record_block(run, load_json(run / "run.json"), ""))
        check("審查：追溯矩陣的證據與指令只留檔名", strip_paths("見 /var/folders/xy/T/a.log 與 _tpm/runs/r9/test/verify.md") == "見 a.log 與 verify.md")
        sub = root / "repos/app"
        write(sub / "db/migrations/001.sql", "x\n")
        write(run / "run.json", json.dumps(dict(load_json(run / "run.json"), target_repo="repos/app")))
        check("審查：check 帶 --run 時用 run 的目標 repo", _repo_of(root, ["--run", str(run)], run) == sub)
        write(run / "run.json", json.dumps(dict(load_json(run / "run.json"), target_repo=".")))

        # ---- export
        code, notes = export(root, pandoc=None)
        check("export：沒有 pandoc 時說明怎麼裝並 exit 0", code == 0 and "找不到 pandoc" in notes[0] and "pandoc.org" in notes[0])
        fake = root / "fake_pandoc.py"
        write(fake, "import sys\nopen(sys.argv[sys.argv.index('-o') + 1], 'w').write('docx')\n")
        launcher = root / ("pandoc.cmd" if os.name == "nt" else "pandoc.sh")
        write(launcher, ("@echo off\r\n\"%s\" \"%s\" %%*\r\n" if os.name == "nt" else "#!/bin/sh\nexec \"%s\" \"%s\" \"$@\"\n") % (sys.executable, fake))
        os.chmod(str(launcher), 0o755)
        code, notes = export(root, str(root / "out"), pandoc=str(launcher))
        got = sorted(p.name for p in (root / "out").glob("*.docx"))
        check("export：有 pandoc 時每份文件轉一個 docx（外部文件不轉）", code == 0 and "資料庫設計文件.docx" in got and len(got) == 8, str((got, notes[:2])))

        # ---- CLI
        check("CLI：缺 --run 回 2", main(["affected"], root) == 2 and main(["sync"], root) == 2)
        check("CLI：不認得的指令回 2", main(["nope"], root) == 2)
        check("CLI：status exit 0", main(["status"], root) == 0)

    bad = [r for r in results if not r[0]]
    for ok, name, detail in results:
        print("  [%s] %s%s" % ("通過" if ok else "失敗", name, "" if ok or not detail else "：" + detail[:600]))
    print("deliverables selftest：%s（%d 項）" % ("全部通過" if not bad else "%d 項失敗" % len(bad), len(results)))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
