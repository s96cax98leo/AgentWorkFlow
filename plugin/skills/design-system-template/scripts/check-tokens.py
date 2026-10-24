#!/usr/bin/env python3
"""掃寫死的色碼：不在白名單裡的 hex／rgb()／rgba()／hsl()／oklch() 等一律列出。

用法：python3 check-tokens.py [--all] <檔案或目錄>...（Windows 可用 py -3 或 python）
  目錄：遞迴掃 css/scss/ts/tsx/js/jsx/mjs/cjs/vue/svelte/html/svg，略過 node_modules、.next、dist、build、
        coverage 與測試檔（*.test.*、*.spec.*、__tests__/）。
  正本本身（SHA-256 與 references/SHA256SUMS 相同）預設只掃 source.conf 的 ALLOW_RANGES；加 --all 才掃整份。
白名單＝references/tokens.css 的所有色值
       ＋ references/allowlist.txt（一行一個色值）
       ＋ 正本 ALLOW_RANGES 行段出現的色值（references/source.conf 有設 SOURCE 與 ALLOW_RANGES 時）。
  hex 正規化：小寫、3／4 位展開成 6／8 位（#fff ＝ #ffffff）；%23 開頭（SVG data URL）視同 #。
  rgb()／rgba() 正規化：去空白、數字轉成同一寫法、沒有 alpha 視為 1（rgb(0 0 0 / .5) ＝ rgba(0,0,0,0.5)）。
  含 var( 或 calc( 的函式不算色碼，略過。
不偵測：顏色關鍵字（white、black、red…）、color-mix() 本身（裡面的色碼照樣會被掃到）。
結束碼：0＝沒有違規；1＝有違規；2＝用法錯誤。
"""

import hashlib
import os
import re
import sys
from pathlib import Path

ref = Path(__file__).resolve().parent.parent / "references"
args = sys.argv[1:]
if not [a for a in args if a != "--all"]:
    print("用法：check-tokens.py [--all] <檔案或目錄>...", file=sys.stderr)
    sys.exit(2)
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
scan_all = "--all" in args
paths = [Path(a) for a in args if a != "--all"]


def conf():
    c = {}
    p = ref / "source.conf"
    if p.exists():
        for line in p.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                c[k.strip()] = v.strip()
    return c


def ranges(spec):
    out = []
    for part in filter(None, (s.strip() for s in spec.split(","))):
        a, _, b = part.partition("-")
        out.extend(range(int(a) - 1, int(b or a)))
    return out


HEX = re.compile(r"(?<![\w&])(?:#|%23)([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![0-9a-zA-Z_-])")
FUNC = re.compile(r"\b(rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(([^()]*)\)", re.I)


def norm_hex(h):
    h = h.lower()
    if len(h) in (3, 4):
        h = "".join(c * 2 for c in h)
    return "#" + h


def num(s):
    s = s.strip()
    if s.endswith("%"):
        return f"{float(s[:-1]):g}%"
    return f"{float(s):g}"


def norm_func(name, inner):
    name = name.lower()
    if "var(" in inner or "calc(" in inner:
        return None
    parts = [p for p in re.split(r"[\s,/]+", inner.strip()) if p]
    try:
        vals = [num(p) for p in parts]
    except ValueError:
        return None
    if name in ("rgb", "rgba"):
        if len(vals) == 3:
            vals.append("1")
        return "rgba(" + ",".join(vals) + ")"
    return name + "(" + ",".join(vals) + ")"


def colors(line):
    for m in HEX.finditer(line):
        yield m.group(0), norm_hex(m.group(1))
    for m in FUNC.finditer(line):
        n = norm_func(m.group(1), m.group(2))
        if n:
            yield m.group(0), n


c = conf()
allow = set()
tokens = ref / "tokens.css"
if tokens.exists():
    body = re.sub(r"/\*.*?\*/", "", tokens.read_text(encoding="utf-8"), flags=re.S)  # 註解裡的範例不算
    for line in body.splitlines():
        allow.update(n for _, n in colors(line))
al = ref / "allowlist.txt"
if al.exists():
    for line in al.read_text(encoding="utf-8").splitlines():
        line = re.sub(r"(^|\s)#\s.*$", "", line).strip()  # 「# 」開頭是註解；#abc 這種色值保留
        if line:
            allow.update(n for _, n in colors(line.split()[0]))

ds_sha, ds_rng = None, None
src = c.get("SOURCE", "")
if src:
    ds_path = ref / src
    ds_lines = ds_path.read_text(encoding="utf-8").splitlines()
    ds_rng = ranges(c.get("ALLOW_RANGES", ""))
    for i in ds_rng:
        if i < len(ds_lines):
            allow.update(n for _, n in colors(ds_lines[i]))
    sums = ref / "SHA256SUMS"
    if sums.exists():
        ds_sha = sums.read_text(encoding="utf-8").split()[0]

EXT = {".css", ".scss", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue", ".svelte", ".html", ".htm", ".svg"}
SKIP_DIRS = {"node_modules", ".next", "dist", "build", "coverage", "__tests__", ".turbo", "out"}
TEST = re.compile(r"\.(test|spec)\.[a-z]+$")


def files(p):
    if p.is_file():
        yield p
        return
    for root, dirs, fs in os.walk(p):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for f in sorted(fs):
            fp = Path(root) / f
            if fp.suffix in EXT and not TEST.search(f):
                yield fp


if not allow:
    print("警告：白名單是空的（tokens.css 還是骨架？），所有色碼都會列為違規", file=sys.stderr)

viol, scanned = [], 0
for p in paths:
    if not p.exists():
        print(f"找不到：{p}", file=sys.stderr)
        sys.exit(2)
    for fp in files(p):
        data = fp.read_bytes()
        try:
            lines = data.decode("utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        rng = range(len(lines))
        if ds_sha and ds_rng and hashlib.sha256(data).hexdigest() == ds_sha and not scan_all:
            rng = ds_rng
            print(f"[正本] {fp}：只掃 ALLOW_RANGES；--all 掃整份")
        scanned += 1
        for i in rng:
            if i >= len(lines):
                continue
            for raw, n in colors(lines[i]):
                if n not in allow:
                    viol.append((fp, i + 1, raw, n))

for fp, ln, raw, n in viol:
    print(f"{fp}:{ln}: {raw}" + (f"  (= {n})" if n != raw.lower() else ""))
uniq = len({n for *_, n in viol})
print(f"白名單 {len(allow)} 個色值；掃描 {scanned} 個檔；違規 {len(viol)} 處（{uniq} 種色值）")
sys.exit(1 if viol else 0)
