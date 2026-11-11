"""讀取預算：模版角色讀大檔要分段、每個 agent 有讀取額度（由 guard.py 的 guard-paths（Read）與 guard-shell 呼叫）。

為什麼要有：agent 整檔讀大檔會把 context 撐大，之後每一輪都整份重讀，token 幾乎都花在這裡。勸導沒用，所以用 hook 擋。

設定在專案的 .claude/teams/methodology.json 的 limits（awf_config.limits()）：
  mode              enforce（擋）、warn（不擋，只把「本來會擋」寫進狀態目錄的 warn.log）、off；沒寫＝不啟用（由使用者決定要不要開）
  whole_file_lines  一次 Read 最多讀幾行；0＝關閉「大檔要分段讀」與 shell 的 cat 類規則
  read_budget       {"default": 位元組, "roles": {"<模版角色名>": 位元組}}；0＝不限
沒有 methodology.json、讀不懂、沒有 limits 鍵、沒寫 mode：整個功能關閉。

只套用模版角色（tpm-*／dev-*）。主 session、使用者自己的 agent、登記在 extension_agents 的擴充 agent 都不受影響。

三條規則：
  1. Read 這次實際要讀的行數（limit；沒給就是 offset 到檔尾；以 Read 的預設上限 READ_DEFAULT_LINES 封頂）超過 whole_file_lines 就擋。
  2. 每次放行的 Read 估算位元組數並累加。第一次跨過 80% 擋一次（提醒收斂，重試就放行，這次不計入）；
     累計用完之後的 Read 一律擋，要求寫執行紀錄並交件。
  3. shell 的單一簡單指令 cat／Get-Content（gc、type）整檔印出超過門檻的檔案時擋。有管線、導向、截斷參數或解析不了一律放行。

失敗語意：這是輔助功能，不是信任邊界。check_read()、check_shell() 把所有例外吃掉並回傳 None（放行）：
狀態檔壞掉（重置）、沒有權限、路徑怪異、檔案不存在、目錄、二進位檔都放行。機敏檔等既有檢查由 guard.py 先做，不受這裡影響。

狀態檔：<系統暫存目錄>/awf-read-budget/<session_id>/<agent_id>.json（環境變數 AWF_READ_BUDGET_DIR 可覆寫根目錄，測試隔離用），
內容 {"used": 位元組, "warned": 是否已提醒過 80%, "reads": 次數}，原子寫入。超過 STALE_DAYS 天沒動的 session 目錄順手清掉。
只用標準庫。
"""

from __future__ import annotations

import json
import os
import random
import re
import shlex
import stat
import tempfile
import time
import zlib
from pathlib import Path

import awf_config

READ_DEFAULT_LINES = 2000  # Read 工具不帶 limit 時最多讀的行數
BIG_FILE_BYTES = 5 * 1024 * 1024  # 超過就不逐行算，直接當成大檔
BINARY_PROBE = 8192  # 前 8KB 有 NUL 就當二進位檔
SAMPLE_BYTES = 64 * 1024  # 大檔估平均每行位元組用的取樣
WARN_RATIO = 0.8
STALE_DAYS = 7
CLEANUP_PROBES = 3  # 每次最多看幾個舊 session 目錄
WARN_LOG_MAX = 1024 * 1024  # warn.log 超過就輪替成 warn.log.1
MAX_SHELL_LEN = 4096  # 更長的 shell 指令不解析
# Read 會以圖片、PDF、notebook 的方式處理的檔案：行數沒有意義，不套用
SKIP_EXT = frozenset({".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".tif", ".tiff", ".heic", ".avif", ".svgz",
                      ".pdf", ".ipynb"})
ENV_DIR = "AWF_READ_BUDGET_DIR"
HINT = ('先用 Grep 或 `python3 .claude/teams/tools/kb.py query "<問題>"` 找到行段，再帶 offset／limit 讀，一次最多 {n} 行'
        "（設定：.claude/teams/methodology.json 的 limits）")
WIN_RESERVED = re.compile(r"^(?:con|prn|aux|nul|com\d|lpt\d)$", re.I)


# ---------------------------------------------------------------- 共用
def _role(data: dict, cfg: dict) -> str:
    """模版角色回傳角色名（套過 ROLE_ALIASES）；主 session、使用者自己的 agent、擴充 agent 回空字串。"""
    raw = data.get("agent_type")
    if not isinstance(raw, str) or not raw or not awf_config.is_team_agent(raw, cfg):
        return ""
    if awf_config.role_name(raw) not in awf_config.RESERVED_ROLES and awf_config.extension_role(raw, cfg):
        return ""  # 登記在 extension_agents 的使用者 agent：不套讀取預算
    return awf_config.team_role(raw, cfg)


def _int(v) -> int | None:
    if isinstance(v, bool):
        return None
    if isinstance(v, int):
        return v
    if isinstance(v, float) and v == int(v):
        return int(v)
    if isinstance(v, str) and re.fullmatch(r"\s*\d+\s*", v):
        return int(v)
    return None


def _resolve(raw: str, data: dict) -> Path:
    p = raw.replace("\\", "/") if os.name == "nt" else raw
    m = re.match(r"^/([A-Za-z])(/.*)?$", p) if os.name == "nt" else None
    if m:  # Git Bash 把磁碟機代號寫成路徑第一層（/c/…）
        p = m.group(1) + ":" + (m.group(2) or "/")
    path = Path(p)
    if not path.is_absolute():
        cwd = data.get("cwd")
        path = Path(cwd if isinstance(cwd, str) and cwd else awf_config.workspace()) / path
    return path


def _inspect(path: Path) -> dict | None:
    """回傳 {"size", "lines", "avg"}；lines 為 None 表示檔案超過 BIG_FILE_BYTES（不逐行算）。
    不是一般檔、二進位檔、圖片／PDF／notebook 回 None（不套用規則）。"""
    if path.suffix.lower() in SKIP_EXT:
        return None
    st = os.stat(path)
    if not stat.S_ISREG(st.st_mode):
        return None
    size = st.st_size
    if size == 0:
        return {"size": 0, "lines": 0, "avg": 0.0}
    with open(path, "rb") as f:
        buf = f.read(SAMPLE_BYTES if size > BIG_FILE_BYTES else BIG_FILE_BYTES + 1)
    if b"\0" in buf[:BINARY_PROBE]:
        return None
    nl = buf.count(b"\n")
    if size > BIG_FILE_BYTES or len(buf) > BIG_FILE_BYTES:
        return {"size": size, "lines": None, "avg": len(buf) / max(1, nl)}
    lines = nl + (0 if buf.endswith(b"\n") else 1)
    return {"size": len(buf), "lines": lines, "avg": len(buf) / max(1, lines)}


def _safe(name, fallback: str) -> str:
    """把 session_id／agent_id 清成安全的檔名：只留英數、底線、連字號（沒有點與分隔符，路徑跳不出去）。"""
    raw = name if isinstance(name, str) and name else fallback
    out = re.sub(r"[^A-Za-z0-9_-]", "_", raw)[:80] or "_"
    if out != raw:  # 清過的名字加上原字串的雜湊，避免不同 id 清完撞名
        out += "-%08x" % (zlib.crc32(raw.encode("utf-8", "replace")) & 0xFFFFFFFF)
    if WIN_RESERVED.match(out):
        out = "_" + out
    return out


MAX_SINGLE_READ = 100 * 1024  # 單次讀取最多記這麼多位元組（Read 工具一次回傳的內容大約就這個量級）


def state_root() -> Path:
    override = os.environ.get(ENV_DIR)
    return Path(override) if override else Path(tempfile.gettempdir()) / "awf-read-budget"


def _state_path(data: dict) -> Path:
    agent = data.get("agent_id")
    if not isinstance(agent, str) or not agent:
        agent = "type-" + str(data.get("agent_type") or "")
    return state_root() / _safe(data.get("session_id"), "no-session") / (_safe(agent, "no-agent") + ".json")


def _load_state(p: Path) -> tuple[dict, bool]:
    """回傳（狀態, 是不是新的）。內容壞掉就重置。"""
    fresh = {"used": 0, "warned": False, "reads": 0}
    try:
        st = json.loads(p.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return fresh, True
    except (OSError, ValueError):
        return fresh, False
    if (not isinstance(st, dict) or isinstance(st.get("used"), bool) or not isinstance(st.get("used"), int) or st["used"] < 0
            or not isinstance(st.get("warned"), bool)):
        return fresh, False
    reads = st.get("reads")
    return {"used": st["used"], "warned": st["warned"], "reads": reads if isinstance(reads, int) and not isinstance(reads, bool) else 0}, False


def _save_state(p: Path, st: dict) -> None:
    p.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp = p.with_name(f"{p.name}.{os.getpid()}.tmp")
    tmp.write_text(json.dumps(st), encoding="utf-8")
    os.replace(tmp, p)


def _cleanup(root: Path) -> None:
    """清掉超過 STALE_DAYS 天沒動的 session 目錄。每次隨機看 CLEANUP_PROBES 個；只刪自己寫的 .json／.tmp，
    目錄裡有別的東西就留著（rmdir 失敗），所以 AWF_READ_BUDGET_DIR 指錯地方也不會刪到別人的檔案。失敗一律忽略。"""
    try:
        names = os.listdir(root)
        cutoff = time.time() - STALE_DAYS * 86400
        for n in random.sample(names, min(CLEANUP_PROBES, len(names))):
            d = os.path.join(root, n)
            try:
                st = os.lstat(d)
                if not stat.S_ISDIR(st.st_mode) or st.st_mtime >= cutoff:
                    continue
                for f in os.listdir(d)[:200]:
                    if f.endswith((".json", ".tmp")):
                        os.unlink(os.path.join(d, f))
                os.rmdir(d)
            except OSError:
                pass
    except Exception:  # noqa: BLE001
        pass


def _warn_log(data: dict, role: str, rule: str, **extra) -> None:
    """mode=warn：把「本來會擋」記一行 JSON 到 <狀態目錄>/warn.log。寫不進去就算了。"""
    try:
        root = state_root()
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
        log = root / "warn.log"
        try:
            if log.stat().st_size > WARN_LOG_MAX:
                os.replace(log, root / "warn.log.1")
        except OSError:
            pass
        rec = {"ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "rule": rule, "role": role,
               "agent_type": data.get("agent_type"), "agent_id": data.get("agent_id"), "session_id": data.get("session_id"), **extra}
        with open(log, "a", encoding="utf-8") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    except Exception:  # noqa: BLE001
        pass


def _kb(n: int) -> str:
    return f"{max(0, n) / 1000:.0f} KB"


# ---------------------------------------------------------------- Read
def check_read(data: dict, cfg: dict) -> str | None:
    """回傳擋下的訊息；None＝放行。任何例外都放行。"""
    try:
        return _check_read(data, cfg)
    except Exception:  # noqa: BLE001  輔助功能不擋流程
        return None


def _check_read(data: dict, cfg: dict) -> str | None:
    if data.get("tool_name") != "Read":
        return None
    role = _role(data, cfg)
    if not role:
        return None
    lim = awf_config.limits()
    if not lim:
        return None
    ti = data.get("tool_input") or {}
    fp = ti.get("file_path")
    if not isinstance(fp, str) or not fp:
        return None
    info = _inspect(_resolve(fp, data))
    if info is None:
        return None
    enforce = lim["mode"] == "enforce"
    offset = max(1, _int(ti.get("offset")) or 1)
    limit = _int(ti.get("limit"))
    limit = limit if limit and limit > 0 else None
    total, size = info["lines"], info["size"]
    if total is None:  # 超過 5MB：不知道總行數
        want = min(limit or READ_DEFAULT_LINES, READ_DEFAULT_LINES)
        est = min(size, int(want * info["avg"]) + 1)
        total_txt = f"超過 {BIG_FILE_BYTES // (1024 * 1024)}MB，行數未計"
    else:
        remaining = max(0, total - offset + 1)
        want = min(limit or remaining, remaining, READ_DEFAULT_LINES)
        est = size if want >= total else min(size, int(want * info["avg"]) + 1)
        total_txt = f"共 {total} 行"

    n = lim["whole_file_lines"]
    if n and want > n:
        if enforce:
            return (f"{role} 讀 {fp}：這次要讀 {want} 行，超過一次讀取的門檻 {n} 行（檔案{total_txt}）。" + HINT.format(n=n))
        _warn_log(data, role, "whole_file", file=fp, lines=want, total_lines=total, threshold=n)

    rb = lim["read_budget"]
    budget = rb["roles"].get(role, rb["default"])
    if budget <= 0:
        return None
    if not isinstance(data.get("agent_id"), str) or not data.get("agent_id"):
        return None  # 沒有 agent_id 就分不出是哪一個 agent：不記帳（不然同型別的 agent 會共用額度，後派的一開工就被擋）
    est = min(est, MAX_SINGLE_READ)  # Read 一次回傳的內容有上限：單行的大檔不能一次把額度扣光
    sp = _state_path(data)
    st, is_new = _load_state(sp)
    if is_new:
        _cleanup(sp.parent.parent)
    if st["used"] >= budget:
        if enforce:
            return (f"{role} 的讀取額度已用完（已讀約 {_kb(st['used'])}，額度 {_kb(budget)}）。"
                    "請把目前結論寫進執行紀錄檔並交件；不確定的地方寫進 open_questions。"
                    "（設定：.claude/teams/methodology.json 的 limits.read_budget）")
        _warn_log(data, role, "budget_exhausted", file=fp, used=st["used"], budget=budget)
    elif not st["warned"] and st["used"] + est >= budget * WARN_RATIO:
        st["warned"] = True
        if enforce:
            _save_state(sp, st)  # 這次被擋的讀取不計入額度
            return (f"{role} 的讀取額度已用 80%（已讀約 {_kb(st['used'])}，這次約 {_kb(est)}，額度 {_kb(budget)}，"
                    f"剩約 {_kb(budget - st['used'])}）。這次讀取重試一次就會放行；請開始收斂、把已知結論寫進執行紀錄檔。"
                    "（這個提醒每個 agent 只出現一次）")
        _warn_log(data, role, "budget_80", file=fp, used=st["used"], estimate=est, budget=budget)
    st["used"] += est
    st["reads"] += 1
    _save_state(sp, st)
    return None


# ---------------------------------------------------------------- shell 的 cat 類讀取
# 有這些字元就不是「單一簡單指令」（管線、串接、導向、子運算式、變數、萬用字元、跳脫）：一律放行
# `~` 只有在字首才是家目錄展開（由 _cat_targets 判斷）；路徑中間的 `~` 是字面值（Windows 的 8.3 短路徑，例如 RUNNER~1）
NOT_SIMPLE = re.compile(r"[|;&<>\n\r(){}$`*?\[\]!#]")
CAT_FLAGS = re.compile(r"^-[nbsAETvetu]+$")  # cat 不會截斷輸出的旗標
BASH_CATS = frozenset({"cat"})
PS_CATS = frozenset({"get-content", "gc", "cat", "type"})
PS_VALUE_FLAGS = frozenset({"-path", "-literalpath", "-lp"})
PS_IGNORED_VALUE_FLAGS = frozenset({"-encoding"})
PS_IGNORED_SWITCHES = frozenset({"-raw"})


def _cmd_name(tok: str) -> str:
    name = tok.replace("\\", "/").rsplit("/", 1)[-1].lower()
    return name[:-4] if name.endswith(".exe") else name


def _ps_split(cmd: str) -> list[str] | None:
    """PowerShell 的簡單切詞：空白分隔，單雙引號包住的算一個（不處理跳脫；引號沒關回 None）。"""
    toks: list[str] = []
    cur: list[str] = []
    quote = ""
    started = False
    for ch in cmd:
        if quote:
            if ch == quote:
                quote = ""
            else:
                cur.append(ch)
        elif ch in "'\"":
            quote, started = ch, True
        elif ch.isspace():
            if started:
                toks.append("".join(cur))
                cur, started = [], False
        else:
            cur.append(ch)
            started = True
    if quote:
        return None
    if started:
        toks.append("".join(cur))
    return toks


def _cat_targets(tool: str, cmd: str) -> list[str]:
    """指令是「cat 類整檔印出」時回傳檔案清單；不是、或看不懂，回空清單。"""
    if len(cmd) > MAX_SHELL_LEN or NOT_SIMPLE.search(cmd):
        return []
    if tool == "PowerShell":
        toks = _ps_split(cmd)
        if not toks or _cmd_name(toks[0]) not in PS_CATS:
            return []
        files: list[str] = []
        i = 1
        while i < len(toks):
            t = toks[i]
            low = t.lower()
            if low in PS_VALUE_FLAGS or low in PS_IGNORED_VALUE_FLAGS:
                if i + 1 >= len(toks):
                    return []
                if low in PS_VALUE_FLAGS:
                    files.append(toks[i + 1])
                i += 2
                continue
            if low in PS_IGNORED_SWITCHES:
                i += 1
                continue
            if t.startswith("-") or "," in t or t.startswith("~"):  # 其他參數（-TotalCount、-Tail、-First、-Wait…）、陣列、家目錄：放行
                return []
            files.append(t)
            i += 1
        return files
    try:
        toks = shlex.split(cmd)
    except ValueError:
        return []
    if not toks or _cmd_name(toks[0]) not in BASH_CATS:
        return []
    files = []
    opts = True
    for t in toks[1:]:
        if opts and t == "--":
            opts = False
        elif opts and t.startswith("-"):
            if not CAT_FLAGS.match(t):
                return []
        elif t.startswith("~"):  # 家目錄展開：看不懂就放行
            return []
        else:
            files.append(t)
    return files


def check_shell(data: dict, cfg: dict) -> str | None:
    """回傳擋下的訊息；None＝放行。任何例外都放行。"""
    try:
        return _check_shell(data, cfg)
    except Exception:  # noqa: BLE001
        return None


def _check_shell(data: dict, cfg: dict) -> str | None:
    tool = data.get("tool_name")
    cmd = (data.get("tool_input") or {}).get("command")
    if tool not in ("Bash", "PowerShell") or not isinstance(cmd, str) or not cmd.strip():
        return None
    role = _role(data, cfg)
    if not role:
        return None
    files = _cat_targets(tool, cmd.strip())
    if not files:
        return None
    lim = awf_config.limits()
    if not lim or not lim["whole_file_lines"]:
        return None
    n = lim["whole_file_lines"]
    for f in files:
        try:
            info = _inspect(_resolve(f, data))
        except OSError:
            continue
        if info is None or (info["lines"] is not None and info["lines"] <= n):
            continue
        total_txt = f"共 {info['lines']} 行" if info["lines"] is not None else f"超過 {BIG_FILE_BYTES // (1024 * 1024)}MB，行數未計"
        if lim["mode"] != "enforce":
            _warn_log(data, role, "shell_whole_file", file=f, total_lines=info["lines"], threshold=n)
            return None
        return (f"{role} 用 shell 整檔印出 {f}：檔案{total_txt}，超過一次讀取的門檻 {n} 行。" + HINT.format(n=n))
    return None
