#!/usr/bin/env python3
"""工具先定位：不派 agent，先用確定性的方式找出「哪個檔的哪幾行」值得讀。

為什麼要有：兩段式調研的定位 agent 實測佔 plan 段約六成 token（每個角度 9～11 輪查詢）。
多數時候需求裡的詞（識別字、路徑、中文名詞）直接對得到程式與文件，這一步不需要模型。

做法：把目標 repo 的文字檔切成固定行數的視窗，用需求文字當查詢做 BM25 排序（分詞和 kb.py 相同：英數字小寫單字、中日韓 bigram），
取分數夠高的視窗、合併相鄰的，回報行段。命中夠多、涵蓋夠多查詢詞才算「夠」（enough）；不夠時 workflow 仍會派定位 agent，並把這裡找到的當成起點。

角度決定看哪些檔：
  code-contracts  程式檔（不看文件）
  specs-adr       specs/、docs/ 底下的文件
  requirement     需求檔本身（需求是專案內的檔案時）加上文件

用法：
    python3 locate.py --repo <目標 repo> --requirement "<需求文字或專案內的檔案路徑>" [--angle <角度>]… [--json]
    python3 locate.py --selftest
只用標準庫。結果可重現：同樣的檔案內容與需求，輸出完全相同。這是輔助功能：任何錯誤都回空結果，由 workflow 改派定位 agent。
"""
from __future__ import annotations

import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[2]
sys.path.insert(0, str(HERE))
import kb  # noqa: E402  共用分詞與機敏檔判斷

ANGLES = ("code-contracts", "specs-adr", "requirement")
DOC_EXTS = (".md", ".markdown", ".txt", ".rst", ".adoc")
SKIP_EXTS = (".lock", ".min.js", ".min.css", ".map", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".pdf", ".zip", ".gz", ".woff", ".woff2",
             ".ttf", ".eot", ".mp4", ".mp3", ".wav", ".bin", ".exe", ".dll", ".so", ".dylib", ".class", ".jar", ".pyc", ".sqlite", ".db", ".snap")
SKIP_NAMES = {"package-lock.json", "pnpm-lock.yaml", "yarn.lock", "poetry.lock", "uv.lock", "cargo.lock", "go.sum", "composer.lock", "gemfile.lock"}
PRUNE = {".git", "node_modules", ".hg", ".svn", "__pycache__", ".worktrees", ".venv", "venv", "dist", "build", "target", ".next", ".codegraph", "graft"}
MAX_FILES = 4000
MAX_FILE_BYTES = 300 * 1024
MAX_TOTAL_BYTES = 24 * 1024 * 1024
WINDOW = 40          # 每個視窗幾行
STEP = 30            # 視窗間隔（重疊 10 行）
MAX_SEGMENTS = 12
MAX_SEGMENT_LINES = 120
KEEP_RATIO = 0.35    # 分數不到第一名這個比例的視窗不要
FILE_RATIO = 0.15    # 但每個檔最好的那個視窗，分數到第一名這個比例就留（不同的檔各留一段，避免只挑到同一個檔）
MIN_SEGMENTS = 3     # 少於這個數就算不夠
MIN_COVERAGE = 0.3   # 選中的行段涵蓋的查詢詞比例低於這個就算不夠
K1, B = 1.5, 0.75


def _posix(p: str) -> str:
    return str(p).replace("\\", "/")


def list_files(repo: Path) -> list[str]:
    """repo 裡的檔案（相對路徑、posix、排序）。優先用 git ls-files（尊重 .gitignore），不是 git repo 就自己走訪。"""
    try:
        q = subprocess.run(["git", "-C", str(repo), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                           capture_output=True, timeout=20)
        if q.returncode == 0:
            names = [n for n in q.stdout.decode("utf-8", "replace").split("\0") if n]
            return sorted(_posix(n) for n in names if not any(part in PRUNE for part in Path(n).parts))
    except (OSError, subprocess.SubprocessError):
        pass
    out = []
    for base, dirs, files in os.walk(repo):
        dirs[:] = sorted(d for d in dirs if d not in PRUNE)
        for f in files:
            out.append(_posix(os.path.relpath(os.path.join(base, f), repo)))
    return sorted(out)


def wanted(path: str, angle: str) -> bool:
    low = path.lower()
    name = low.rsplit("/", 1)[-1]
    if kb.is_sensitive(name) or name in SKIP_NAMES or low.endswith(SKIP_EXTS):
        return False
    is_doc = low.endswith(DOC_EXTS)
    if angle == "code-contracts":
        return not is_doc
    if angle == "specs-adr":
        return is_doc and ("specs/" in "/" + low or "docs/" in "/" + low or "/adr" in "/" + low)
    return is_doc


def read_lines(p: Path) -> list[str] | None:
    """讀成行；太大、二進位、讀不到回 None。"""
    try:
        if p.is_symlink() or not p.is_file() or p.stat().st_size > MAX_FILE_BYTES:
            return None
        raw = p.read_bytes()
    except OSError:
        return None
    if b"\0" in raw[:4096]:
        return None
    return raw.decode("utf-8", "replace").splitlines()


def windows(lines: list[str]) -> list[tuple[int, int]]:
    """（起, 訖）行號，從 1 起算、含兩端。"""
    n = len(lines)
    if n == 0:
        return []
    if n <= WINDOW:
        return [(1, n)]
    out, s = [], 1
    while s <= n:
        e = min(n, s + WINDOW - 1)
        out.append((s, e))
        if e == n:
            break
        s += STEP
    return out


MAX_TERMS = 400
# 程式角度只當「種子」：詞彙比對找得到字面對得上的檔，找不到「新功能會用到、但需求沒提到名字」的檔（要靠推理），
# 所以程式角度一律還是派定位 agent，只是從種子出發。文件角度命中夠就不派。
SEED_ONLY = {"code-contracts"}
SEED_FILES = 3        # 取分數最高的幾個檔當種子，沿引用關係擴展一層
MAX_EXPAND = 6        # 最多擴展幾個檔
GENERIC_STEMS = {"__init__", "index", "main", "models", "model", "handlers", "handler", "service", "services", "utils", "util", "helpers",
                 "types", "constants", "config", "settings", "repository", "validators", "views", "routes", "urls", "schema", "schemas", "test", "tests", "mod", "lib"}


def ref_keys(path: str) -> list[str]:
    """別的檔引用這個檔時可能寫的名字：檔名（太通用的不算）、上層目錄加檔名（a.b、a/b 兩種寫法）。"""
    parts = Path(path).with_suffix("").parts
    stem = parts[-1].lower() if parts else ""
    keys = []
    if len(stem) >= 4 and stem not in GENERIC_STEMS:
        keys.append(stem)
    if len(parts) >= 2 and stem and stem != "__init__":
        parent = parts[-2].lower()
        keys += [f"{parent}.{stem}", f"{parent}/{stem}"]
    return keys


def terms_of(text: str) -> list[str]:
    """查詢詞：需求全文的詞（kb.query_terms 只取前 64 個，長需求的後半段會被丟掉，所以這裡自己取）。丟掉含虛字的中文 bigram 與單一字元。"""
    toks = list(dict.fromkeys(kb.tokenize(text, query=True)))
    kept = [t for t in toks if len(t) > 1 and not (len(t) == 2 and kb._CJK_RE.match(t) and (t[0] in kb.QUERY_STOP_CHARS or t[1] in kb.QUERY_STOP_CHARS))]
    return (kept or toks)[:MAX_TERMS]


def locate(repo: Path, text: str, angle: str, extra_files: list[tuple[str, Path]] | None = None) -> dict:
    """回 {"locations": [{path, start, end, why}], "enough": bool, "coverage": float, "note": str}。"""
    terms = terms_of(text)
    empty = {"locations": [], "enough": False, "coverage": 0.0, "note": ""}
    if not terms or angle not in ANGLES:
        return {**empty, "note": "沒有可用的查詢詞" if not terms else f"不認得的角度 {angle}"}
    tset = set(terms)
    docs: list[dict] = []     # 每個視窗：path, start, end, tf(dict), length
    texts: dict[str, str] = {}   # 檔案內容（小寫），擴展引用關係用
    sizes: dict[str, int] = {}
    total = 0
    truncated = False
    files = [(f, repo / f) for f in list_files(repo) if wanted(f, angle)]
    files += list(extra_files or [])
    for i, (rel, full) in enumerate(files):
        if i >= MAX_FILES or total > MAX_TOTAL_BYTES:
            truncated = True
            break
        lines = read_lines(full)
        if not lines:
            continue
        total += sum(len(x) for x in lines)
        if angle in SEED_ONLY:
            texts[rel] = "\n".join(lines).lower()
            sizes[rel] = len(lines)
        for s, e in windows(lines):
            toks = kb.tokenize("\n".join(lines[s - 1:e]))
            tf: dict[str, int] = {}
            for t in toks:
                if t in tset:
                    tf[t] = tf.get(t, 0) + 1
            docs.append({"path": rel, "start": s, "end": e, "tf": tf, "len": len(toks) or 1})
    if not docs:
        return {**empty, "note": "這個角度沒有可以看的檔案"}
    n = len(docs)
    avg = sum(d["len"] for d in docs) / n
    df = {t: sum(1 for d in docs if t in d["tf"]) for t in terms}
    idf = {t: math.log(1 + (n - df[t] + 0.5) / (df[t] + 0.5)) for t in terms}
    for d in docs:
        ln = max(d["len"], avg * 0.5)   # 很短的視窗（幾行的小檔）不因為長度正規化而分數暴增
        d["score"] = sum(idf[t] * (c * (K1 + 1)) / (c + K1 * (1 - B + B * ln / avg)) for t, c in d["tf"].items())
    ranked = sorted((d for d in docs if d["score"] > 0), key=lambda d: (-d["score"], d["path"], d["start"]))
    if not ranked:
        return {**empty, "note": "查詢詞在這個角度的檔案裡都沒有出現"}
    top = ranked[0]["score"]
    picked = [d for d in ranked if d["score"] >= top * KEEP_RATIO][:MAX_SEGMENTS * 2]
    seen_files = {d["path"] for d in picked}
    for d in ranked:                      # ranked 已依分數排序：每個還沒入選的檔，第一次出現的就是它最好的視窗
        if len(picked) >= MAX_SEGMENTS * 2 or d["score"] < top * FILE_RATIO:
            break
        if d["path"] not in seen_files:
            seen_files.add(d["path"])
            picked.append(d)
    # 同一個檔相鄰或重疊的視窗合併（合併後不超過 MAX_SEGMENT_LINES）
    merged: list[dict] = []
    for d in sorted(picked, key=lambda d: (d["path"], d["start"])):
        m = merged[-1] if merged else None
        if m and m["path"] == d["path"] and d["start"] <= m["end"] + 1 and d["end"] - m["start"] + 1 <= MAX_SEGMENT_LINES:
            m["end"] = max(m["end"], d["end"])
            m["score"] = max(m["score"], d["score"])
            for t, c in d["tf"].items():
                m["tf"][t] = m["tf"].get(t, 0) + c
        else:
            merged.append({"path": d["path"], "start": d["start"], "end": d["end"], "score": d["score"], "tf": dict(d["tf"])})
    merged = sorted(merged, key=lambda d: (-d["score"], d["path"], d["start"]))[:MAX_SEGMENTS]
    covered = set()
    locs = []
    for m in merged:
        covered |= set(m["tf"])
        hit = sorted(m["tf"], key=lambda t: (-idf[t] * m["tf"][t], t))[:5]
        locs.append({"path": m["path"], "start": m["start"], "end": m["end"], "why": "工具定位：命中 " + "、".join(hit)})
    if angle in SEED_ONLY and texts:
        # 沿引用關係擴展一層：種子檔引用的檔、引用種子檔的檔（看名字有沒有出現在內容裡；不解析語法）
        have = {x["path"] for x in locs}
        seeds = list(dict.fromkeys(m["path"] for m in merged))[:SEED_FILES]
        best = {}
        for d in ranked:
            best.setdefault(d["path"], d)
        added: list[tuple[str, str]] = []
        for seed in seeds:
            seed_text, seed_keys = texts.get(seed, ""), ref_keys(seed)
            for other in sorted(texts):
                if other in have or other == seed or any(other == a for a, _ in added):
                    continue
                if any(k in seed_text for k in ref_keys(other)):
                    added.append((other, f"工具定位：被 {seed} 引用"))
                elif seed_keys and any(k in texts[other] for k in seed_keys):
                    added.append((other, f"工具定位：引用了 {seed}"))
        for other, why in added[:max(0, min(MAX_EXPAND, MAX_SEGMENTS - len(locs)))]:
            b = best.get(other)
            s0, e0 = (b["start"], b["end"]) if b else (1, min(sizes.get(other, WINDOW), WINDOW))
            locs.append({"path": other, "start": s0, "end": e0, "why": why})
    coverage = round(len(covered) / len(terms), 3)
    enough = len(locs) >= MIN_SEGMENTS and coverage >= MIN_COVERAGE and not truncated and angle not in SEED_ONLY
    note = ("repo 太大，只看了一部分檔案" if truncated
            else "程式角度只當種子：字面對得上的檔與它們的引用關係，其餘由定位 agent 補" if angle in SEED_ONLY
            else "" if enough else f"命中不夠（{len(locs)} 段、涵蓋 {coverage:.0%} 的查詢詞）")
    return {"locations": locs, "enough": enough, "coverage": coverage, "note": note}


def locate_all(repo: Path | None, requirement_text: str, requirement_path: Path | None = None, angles=ANGLES) -> dict:
    """各角度的定位結果 {角度: {locations, enough, coverage, note}}。任何錯誤都回空（由 workflow 改派定位 agent）。"""
    out: dict = {}
    if repo is None or not repo.is_dir() or not requirement_text.strip():
        return out
    for a in angles:
        try:
            extra = None
            if a == "requirement" and requirement_path is not None:
                try:
                    rel = _posix(os.path.relpath(requirement_path, repo))
                except ValueError:
                    rel = _posix(str(requirement_path))
                if not rel.startswith("../") and not Path(rel).is_absolute():
                    extra = None           # 需求檔在 repo 裡：list_files 已經會看到
                else:
                    extra = [(rel, requirement_path)]
            r = locate(repo, requirement_text, a, extra)
        except Exception as e:  # noqa: BLE001  輔助功能不擋流程
            r = {"locations": [], "enough": False, "coverage": 0.0, "note": f"工具定位失敗（{type(e).__name__}）"}
        out[a] = r
    return out


# ------------------------------------------------------------------ selftest
def selftest() -> int:
    fails = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        print(f"  [{'通過' if ok else '失敗'}] {name}" + ("" if ok else f"：{detail[:400]}"))
        if not ok:
            fails.append(name)

    print("locate selftest")
    with tempfile.TemporaryDirectory(prefix="awf-locate.") as tmp:
        repo = Path(tmp) / "repo"
        (repo / "services" / "orders").mkdir(parents=True)
        (repo / "services" / "billing").mkdir(parents=True)
        (repo / "docs" / "adr").mkdir(parents=True)
        (repo / "specs" / "export").mkdir(parents=True)
        filler = "".join(f"def helper_{i}():\n    return {i}\n\n" for i in range(60))
        (repo / "services" / "orders" / "export.py").write_text(
            filler + "def export_orders_csv(tenant, date_from=None, date_to=None):\n    \"\"\"訂單匯出：依日期區間篩選，時間欄位輸出 UTC。\"\"\"\n"
            "    rows = load_orders(tenant)\n    return to_csv(rows)\n" + filler, encoding="utf-8")
        (repo / "services" / "orders" / "routes.py").write_text(
            "ROUTES = {\n    'GET /exports/orders.csv': 'export_orders_csv',\n}\n\ndef register(app):\n    for k, v in ROUTES.items():\n        app.add(k, v)\n", encoding="utf-8")
        (repo / "services" / "orders" / "batch.py").write_text(
            "def enqueue_export(tenant):\n    \"\"\"大量匯出改走背景批次：排入佇列，完成後通知申請人。\"\"\"\n    queue.put(('export_orders_csv', tenant))\n", encoding="utf-8")
        (repo / "services" / "billing" / "invoice.py").write_text("".join(f"def invoice_{i}():\n    return {i}\n\n" for i in range(80)), encoding="utf-8")
        (repo / "docs" / "adr" / "0003-export.md").write_text("# ADR 3：訂單匯出格式\n\n匯出一律用 CSV，時間欄位輸出 UTC。\n\n## 決策\n\n日期區間由呼叫端指定。\n", encoding="utf-8")
        (repo / "specs" / "export" / "spec.md").write_text("# 訂單匯出\n\n## 驗收條件\n\n- AC1 匯出檔名帶日期區間\n- AC2 超過同步上限改走背景批次\n", encoding="utf-8")
        (repo / "docs" / "onboarding.md").write_text("# 新人須知\n\n先裝開發環境，再跑測試。\n", encoding="utf-8")
        (repo / ".env").write_text("SECRET=訂單匯出 export_orders_csv\n", encoding="utf-8")
        (repo / "package-lock.json").write_text('{"orders": "export_orders_csv 訂單匯出"}\n', encoding="utf-8")
        (repo / "blob.bin").write_bytes(b"\0\1export_orders_csv" * 50)
        req = "訂單匯出支援日期區間與客戶時區：GET /exports/orders.csv 接受 from、to；大量匯出改走背景批次，完成後通知申請人。"

        r = locate(repo, req, "code-contracts")
        paths = [x["path"] for x in r["locations"]]
        check("程式角度：命中匯出、路由、批次三個檔，不含文件", {"services/orders/export.py", "services/orders/routes.py", "services/orders/batch.py"} <= set(paths)
              and not any(p.endswith(".md") for p in paths), str(r))
        check("程式角度：無關的檔（billing）不在前面", "services/billing/invoice.py" not in paths[:3], str(paths))
        check("行段指到函式所在的位置，不是整個檔", any(x["path"] == "services/orders/export.py" and x["start"] > 100 and x["end"] - x["start"] < MAX_SEGMENT_LINES for x in r["locations"]), str(r["locations"]))
        check("機敏檔、lock 檔、二進位檔不看", not any(p in (".env", "package-lock.json", "blob.bin") for p in paths), str(paths))
        check("程式角度只當種子：enough=false 並說明；why 都以「工具定位：」開頭", r["enough"] is False and "種子" in r["note"]
              and all(x["why"].startswith("工具定位：") for x in r["locations"]), str(r))
        (repo / "services" / "orders" / "loader.py").write_text("def load_orders(tenant):\n    return []\n", encoding="utf-8")
        (repo / "services" / "orders" / "export.py").write_text(
            "from .loader import load_orders\n" + (repo / "services" / "orders" / "export.py").read_text(encoding="utf-8"), encoding="utf-8")
        (repo / "tests").mkdir()
        (repo / "tests" / "test_misc.py").write_text("from services.orders.export import export_orders_csv\n\ndef test_it():\n    assert True\n", encoding="utf-8")
        x = locate(repo, req, "code-contracts")
        why = {i["path"]: i["why"] for i in x["locations"]}
        check("沿引用關係擴展：種子檔 import 的檔與引用種子檔的檔都列進來，並寫明關係",
              "被 services/orders/export.py 引用" in why.get("services/orders/loader.py", "") and "tests/test_misc.py" in why, str(why))
        check("擴展不會把無關的檔帶進來", "services/billing/invoice.py" not in why, str(why))
        (repo / "services" / "orders" / "loader.py").unlink()
        (repo / "tests" / "test_misc.py").unlink()
        (repo / "tests").rmdir()
        (repo / "services" / "orders" / "export.py").write_text(
            (repo / "services" / "orders" / "export.py").read_text(encoding="utf-8").replace("from .loader import load_orders\n", "", 1), encoding="utf-8")
        check("行段格式：path 是相對 posix 路徑、起訖是正整數、每段不超過上限",
              all(not Path(x["path"]).is_absolute() and "\\" not in x["path"] and 1 <= x["start"] <= x["end"] and x["end"] - x["start"] + 1 <= MAX_SEGMENT_LINES for x in r["locations"])
              and len(r["locations"]) <= MAX_SEGMENTS, str(r["locations"]))

        d = locate(repo, req, "specs-adr")
        dp = [x["path"] for x in d["locations"]]
        check("文件角度：只看 specs／docs 的文件，命中 spec 與 ADR", set(dp) >= {"specs/export/spec.md", "docs/adr/0003-export.md"} and all(p.endswith(".md") for p in dp), str(d))
        check("文件角度：只有兩個檔時算不夠（交給定位 agent 補）", d["enough"] is False and "命中不夠" in d["note"], str(d))
        for i in range(3):
            (repo / "docs" / "adr" / f"001{i}-export-{i}.md").write_text(f"# ADR 1{i}：訂單匯出的日期區間（{i}）\n\n大量匯出改走背景批次，完成後通知申請人。\n", encoding="utf-8")
        d2 = locate(repo, req, "specs-adr")
        check("文件角度：命中夠多、涵蓋夠 → enough=true（不用派定位 agent）", d2["enough"] is True and len(d2["locations"]) >= MIN_SEGMENTS, str(d2))
        for i in range(3):
            (repo / "docs" / "adr" / f"001{i}-export-{i}.md").unlink()

        check("可重現：同樣的輸入輸出完全相同", locate(repo, req, "code-contracts") == r and locate(repo, req, "specs-adr") == d)
        z = locate(repo, "完全無關的主題：量子糾纏與黑洞蒸發 xyzzy", "code-contracts")
        check("沒命中：回空、enough=false 並說明", z["locations"] == [] and z["enough"] is False and z["note"], str(z))
        check("空的需求、不認得的角度：回空", locate(repo, "", "code-contracts")["locations"] == [] and locate(repo, req, "nope")["enough"] is False)
        e = locate(repo, "export_orders_csv", "code-contracts")
        check("只有一個英文識別字也找得到（中英混合的 repo）", any(x["path"] == "services/orders/export.py" for x in e["locations"]), str(e))

        reqf = Path(tmp) / "ws" / "requirement.md"
        reqf.parent.mkdir()
        reqf.write_text("# 需求\n\n" + req + "\n", encoding="utf-8")
        allr = locate_all(repo, req, reqf)
        check("locate_all：三個角度都有結果；需求檔在 repo 外也會列進 requirement 角度",
              set(allr) == set(ANGLES) and any(x["path"].endswith("requirement.md") for x in allr["requirement"]["locations"]), str(allr.get("requirement")))
        check("locate_all：repo 不存在或需求是空的 → 空結果", locate_all(Path(tmp) / "nope", req) == {} and locate_all(repo, "  ") == {})
        (repo / "big.py").write_bytes(b"x = 1\n" * 200000)
        check("太大的檔略過", not any(x["path"] == "big.py" for x in locate(repo, "x", "code-contracts")["locations"]))
    print("locate selftest：" + (f"全部通過" if not fails else f"{len(fails)} 項失敗"))
    return 1 if fails else 0


def main(argv: list[str]) -> int:
    for s in (sys.stdout, sys.stderr):
        try:
            s.reconfigure(encoding="utf-8")  # type: ignore[union-attr]
        except (AttributeError, ValueError):
            pass
    if "--selftest" in argv:
        return selftest()
    if "-h" in argv or "--help" in argv or "--repo" not in argv:
        print(__doc__)
        return 0 if ("-h" in argv or "--help" in argv) else 2

    def opt(name: str) -> str:
        return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else ""
    repo = Path(opt("--repo"))
    raw = opt("--requirement")
    angles = [argv[i + 1] for i, a in enumerate(argv) if a == "--angle" and i + 1 < len(argv)] or list(ANGLES)
    out = locate_all(repo, raw, None, angles)
    if "--json" in argv:
        print(json.dumps(out, ensure_ascii=False, indent=1, sort_keys=True))
    else:
        for a, r in out.items():
            print(f"{a}：{len(r['locations'])} 段，{'夠' if r['enough'] else '不夠'}{'（' + r['note'] + '）' if r['note'] else ''}")
            for x in r["locations"]:
                print(f"  {x['path']}:{x['start']}-{x['end']} ｜ {x['why']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
