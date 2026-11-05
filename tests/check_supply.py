#!/usr/bin/env python3
"""供應鏈檢查（run_all.py 會自動執行 tests/check_*.py）。

plugin 會在使用者的所有專案執行，init.py 與 scaffold 工具會被複製進使用者的專案，
所以這幾類檔案要守住下面幾條：

1. 只 import 標準庫或 repo 內模組：plugin/hooks/*.py、scaffold/.claude/teams/tools/*.py、init.py。
   （golden-set 用到的 jsonschema 是選用依賴，不在這些檔內。）
2. plugin/ 下的 .py 沒有網路呼叫：不 import urllib、http.client、socket、requests 等。
3. action 都釘 40 字元 commit SHA：.github/workflows/*.yml 與 scaffold/.github/workflows/*.tmpl。
4. 版控（git ls-files）裡沒有 __pycache__、.env、私鑰形狀的檔案或內容。

失敗時逐條列出，exit 1。只用標準庫，Python 3.9 以上，跨平台。
"""

from __future__ import annotations

import ast
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 只能用標準庫的檔案
STDLIB_ONLY_GLOBS = [
    "plugin/hooks/*.py",
    "scaffold/.claude/teams/tools/*.py",
    "init.py",
]
# repo 內模組：這些目錄下的 .py 可以互相 import（hook 會把 tools 目錄加進 sys.path 用 schemalib）
INTERNAL_DIRS = ["", "plugin/hooks", "scaffold/.claude/teams/tools"]

# 網路相關模組（只看頂層名稱，http 另外只擋 http.client／http.server）
NETWORK_MODULES = {
    "urllib", "urllib2", "urllib3", "socket", "ssl", "requests", "httpx", "aiohttp",
    "ftplib", "smtplib", "poplib", "imaplib", "telnetlib", "nntplib", "xmlrpc",
    "socketserver", "websocket", "websockets", "pycurl", "asyncio",
}
NETWORK_SUBMODULES = {"http.client", "http.server", "http.cookiejar", "asyncio.streams"}

WORKFLOW_GLOBS = [".github/workflows/*.yml", ".github/workflows/*.yaml", "scaffold/.github/workflows/*.tmpl"]
USES_RE = re.compile(r"^\s*(?:-\s*)?uses:\s*['\"]?([^\s#'\"]+)", re.M)
PINNED_RE = re.compile(r"@[0-9a-f]{40}$")

# 私鑰形狀：檔名與內容。內容樣式用拼接寫，避免這支檔案自己被掃到。
KEY_NAME_RE = re.compile(r"(^|/)(id_(rsa|dsa|ecdsa|ed25519)|.*\.(pem|key|p12|pfx|jks|keystore|ppk))$", re.I)
ENV_NAME_RE = re.compile(r"(^|/)\.env(\.[^/]*)?$")
ENV_ALLOWED_RE = re.compile(r"\.(example|sample|template|tmpl)$")
KEY_CONTENT_RE = re.compile("-----BEGIN [A-Z ]*" + "PRIVATE KEY-----")

# Python 3.9 沒有 sys.stdlib_module_names，用這份後備清單（3.9 標準庫的頂層模組）
_STDLIB_FALLBACK = set("""
__future__ _thread abc aifc argparse array ast asynchat asyncio asyncore atexit audioop base64 bdb
binascii binhex bisect builtins bz2 calendar cgi cgitb chunk cmath cmd code codecs codeop collections
colorsys compileall concurrent configparser contextlib contextvars copy copyreg cProfile crypt csv
ctypes curses dataclasses datetime dbm decimal difflib dis distutils doctest email encodings ensurepip
enum errno faulthandler fcntl filecmp fileinput fnmatch formatter fractions ftplib functools gc
getopt getpass gettext glob graphlib grp gzip hashlib heapq hmac html http imaplib imghdr imp
importlib inspect io ipaddress itertools json keyword lib2to3 linecache locale logging lzma mailbox
mailcap marshal math mimetypes mmap modulefinder msilib msvcrt multiprocessing netrc nis nntplib
ntpath numbers operator optparse os ossaudiodev parser pathlib pdb pickle pickletools pipes pkgutil
platform plistlib poplib posix posixpath pprint profile pstats pty pwd py_compile pyclbr pydoc queue
quopri random re readline reprlib resource rlcompleter runpy sched secrets select selectors shelve
shlex shutil signal site smtpd smtplib sndhdr socket socketserver spwd sqlite3 sre_compile
sre_constants sre_parse ssl stat statistics string stringprep struct subprocess sunau symbol
symtable sys sysconfig syslog tabnanny tarfile telnetlib tempfile termios textwrap threading time
timeit tkinter token tokenize trace traceback tracemalloc tty turtle turtledemo types typing
unicodedata unittest urllib uu uuid venv warnings wave weakref webbrowser winreg winsound wsgiref
xdrlib xml xmlrpc zipapp zipfile zipimport zlib zoneinfo
""".split())
STDLIB = set(getattr(sys, "stdlib_module_names", ())) or _STDLIB_FALLBACK


def rel(p: Path) -> str:
    return p.relative_to(ROOT).as_posix()


def files(globs: list[str]) -> list[Path]:
    out: list[Path] = []
    for g in globs:
        out.extend(sorted(ROOT.glob(g)))
    return [p for p in out if p.is_file() and "__pycache__" not in p.parts]


def imports(path: Path) -> list[tuple[int, str]]:
    """回傳 (行號, 完整模組名)。相對 import 回傳空字串前綴的名稱，視為 repo 內。"""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    out: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            out.extend((node.lineno, a.name) for a in node.names)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                continue
            mod = node.module or ""
            out.append((node.lineno, mod))
            # from http import client 這種寫法也要看到 http.client
            out.extend((node.lineno, f"{mod}.{a.name}") for a in node.names)
        elif isinstance(node, ast.Call):
            # __import__("x")、importlib.import_module("x")
            fn = node.func
            name = fn.id if isinstance(fn, ast.Name) else fn.attr if isinstance(fn, ast.Attribute) else ""
            if name in ("__import__", "import_module") and node.args:
                a0 = node.args[0]
                if isinstance(a0, ast.Constant) and isinstance(a0.value, str):
                    out.append((node.lineno, a0.value))
    return out


def internal_modules() -> set[str]:
    names: set[str] = set()
    for d in INTERNAL_DIRS:
        base = ROOT / d if d else ROOT
        names.update(p.stem for p in base.glob("*.py"))
    return names


def check_stdlib_only(errors: list[str]) -> None:
    internal = internal_modules()
    for p in files(STDLIB_ONLY_GLOBS):
        try:
            found = imports(p)
        except SyntaxError as e:
            errors.append(f"{rel(p)}：無法解析（{e}）")
            continue
        for line, mod in found:
            top = mod.split(".")[0]
            if top and top not in STDLIB and top not in internal:
                errors.append(f"{rel(p)}:{line}：import 了非標準庫模組 {mod}")


def check_no_network(errors: list[str]) -> None:
    for p in files(["plugin/**/*.py"]):
        try:
            found = imports(p)
        except SyntaxError as e:
            errors.append(f"{rel(p)}：無法解析（{e}）")
            continue
        for line, mod in found:
            if mod.split(".")[0] in NETWORK_MODULES or any(mod == s or mod.startswith(s + ".") for s in NETWORK_SUBMODULES):
                errors.append(f"{rel(p)}:{line}：plugin 不得有網路呼叫（import {mod}）")


def check_pinned_actions(errors: list[str]) -> None:
    for p in files(WORKFLOW_GLOBS):
        text = p.read_text(encoding="utf-8")
        for m in USES_RE.finditer(text):
            ref = m.group(1)
            if ref.startswith("./") or ref.startswith("docker://"):
                continue
            if not PINNED_RE.search(ref):
                line = text.count("\n", 0, m.start(1)) + 1
                errors.append(f"{rel(p)}:{line}：action 沒有釘 40 字元 commit SHA（{ref}）")


def check_tracked_files(errors: list[str]) -> None:
    r = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True)
    if r.returncode != 0:
        # gate 只會更嚴：拿不到版控清單就算失敗，不推定沒問題
        errors.append(f"git ls-files 失敗：{r.stderr.decode('utf-8', 'replace').strip()}")
        return
    for name in r.stdout.decode("utf-8", "replace").split("\0"):
        if not name:
            continue
        if "__pycache__" in name.split("/") or name.endswith((".pyc", ".pyo")):
            errors.append(f"{name}：版控裡不該有 Python 快取檔")
        elif ENV_NAME_RE.search(name) and not ENV_ALLOWED_RE.search(name):
            errors.append(f"{name}：版控裡不該有 .env 檔")
        elif KEY_NAME_RE.search(name):
            errors.append(f"{name}：版控裡不該有私鑰形狀的檔案")
        else:
            p = ROOT / name
            try:
                if p.is_file() and p.stat().st_size <= 2_000_000:
                    if KEY_CONTENT_RE.search(p.read_bytes().decode("utf-8", "replace")):
                        errors.append(f"{name}：內容含私鑰")
            except OSError:
                pass


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    checks = [
        ("只用標準庫", check_stdlib_only),
        ("plugin 不連網", check_no_network),
        ("action 釘 SHA", check_pinned_actions),
        ("版控沒有快取、.env、私鑰", check_tracked_files),
    ]
    failed = 0
    for name, fn in checks:
        errors: list[str] = []
        fn(errors)
        if errors:
            failed += len(errors)
            print(f"[失敗] {name}")
            for e in errors:
                print(f"  - {e}")
        else:
            print(f"[通過] {name}")
    if failed:
        print(f"\n供應鏈檢查失敗：{failed} 條")
        return 1
    print("\n供應鏈檢查通過")
    return 0


if __name__ == "__main__":
    sys.exit(main())
