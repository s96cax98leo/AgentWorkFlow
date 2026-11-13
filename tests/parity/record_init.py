#!/usr/bin/env python3
"""錄 init（根目錄的 init.py）的對跑基準。

用法：python3 tests/parity/record_init.py > tests/parity/init.expected.json

基準檔本身就是劇本：`cases` 的每個案例有自己的暫存目錄（`@TMP@`），`steps` 依序是檔案操作（write、mkdir、rm、rmtree、
chmod、sub、append）與指令（run：argv、目前目錄、exit code、stdout、stderr、這一步改了哪些檔）。
Rust 的測試（crates/awf-core/tests/init_parity.rs）照同一份劇本做一遍，逐字比對。

模版來源：把 repo 的 `template` 清單（scaffold、plugin 的 hooks／範本／plugin.json、workflow.config.example）複製到暫存目錄
當模版根（`@SRC@`），plugin.json 的版本固定寫成 `VERSION`，所以發佈新版不用重錄；`own_src` 的案例自己再複製一份來改。

「這一步改了哪些檔」怎麼記：每個檔一個簽章 `<x 或 ->:<內容 sha1 前 16 碼>`（x＝有執行權限），目錄記 `d`。
和上一步比有變的才記（刪掉的記 null）；其中簽章和 `base`（第一個案例：全新專案 `--defaults` 的整棵目錄）相同的只記
數量與路徑清單的雜湊（`as_base`），其餘逐檔列出，短的文字檔連內容一起記，方便看差在哪。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`、模版根換成 `@SRC@`、寫進 settings 的 Python 路徑換成 `@PY@`（檔案內容先換再算雜湊）。
- 子行程固定 HOME（`@TMP@/h`）、UTF-8、不寫 .pyc、stdout 不緩衝（init.py 的輸出和它呼叫的 build.py 才會照實際順序）。
- 互動問答用 pty 當 stdin（init.py 只在 stdin 是終端機時詢問）；`tty` 是餵進去的文字，`eof` 表示之後一律 EOF。
- 權限只記有沒有執行權限，不受 umask 影響。
`cmp` 標出不逐字比的輸出：`stderr: "last"` 只比最後一行（argparse 的 usage 沒有照抄）、`"skip"` 不比（Python 版直接當掉的
traceback、`-h` 的說明）。
這支腳本在 Python 版移除時一起刪，基準檔留著（之後用 `AWF_INIT_BLESS=1 cargo test -p awf-core --test init_parity` 重錄）。"""
import hashlib
import json
import os
import pty
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import termios
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TEMPLATE = ["scaffold", "plugin/hooks", "plugin/workflow-templates", "plugin/.claude-plugin", "workflow.config.example"]
VERSION = "9.9.9"
LONG = 2000        # 比這長的檔案內容只記簽章
ENTER = "\n" * 60  # 互動問答剩下的題目一律 Enter


def python_for_hooks():
    exe = sys.executable
    base = getattr(sys, "_base_executable", None)
    if sys.prefix != sys.base_prefix and base and Path(base).is_file():
        exe = base
    return str(Path(exe).resolve())


PY = python_for_hooks()


def copy_template(dst):
    skip = shutil.ignore_patterns("__pycache__", ".DS_Store")
    for rel in TEMPLATE + ["init.py"]:
        src, out = ROOT / rel, Path(dst) / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            shutil.copytree(src, out, ignore=skip)
        else:
            shutil.copy2(src, out)
    pj = Path(dst) / "plugin" / ".claude-plugin" / "plugin.json"
    pj.write_text(json.dumps({"name": "agent-workflow", "version": VERSION}) + "\n", encoding="utf-8")


class World:
    def __init__(self):
        self.root = tempfile.mkdtemp(prefix="init-parity-")
        self.src = os.path.join(self.root, "src")
        copy_template(self.src)
        self.base = {}
        self.cases = []


class Case:
    def __init__(self, w, name, own_src=False):
        self.w, self.name = w, name
        self.dir = os.path.join(w.root, "c%03d" % len(w.cases))
        os.makedirs(os.path.join(self.dir, "cwd"))
        os.makedirs(os.path.join(self.dir, "h"))
        self.real = os.path.realpath(self.dir)
        self.src = w.src
        if own_src:
            self.src = os.path.join(self.dir, "src")
            copy_template(self.src)
        self.steps = []
        self.prev = self.snapshot()[0]
        entry = {"name": name, "steps": self.steps}
        if own_src:
            entry["own_src"] = True
        w.cases.append(entry)

    # ---------------------------------------------------------------- 路徑
    def norm(self, s):
        for a in (os.path.realpath(self.src), self.src):
            s = s.replace(a, "@SRC@")
        for a in (self.real, self.dir):
            s = s.replace(a, "@TMP@")
        return s.replace(PY, "@PY@")

    def abs(self, s):
        return s.replace("@TMP@", self.dir).replace("@SRC@", self.src)

    def p(self, rel):
        return os.path.join(self.dir, rel)

    def read(self, rel):
        with open(self.p(rel), encoding="utf-8", newline="") as f:
            return f.read()

    # ---------------------------------------------------------------- 檔案操作
    def write(self, rel, text, hexdata=None, absolute=False):
        """absolute＝內容裡的 @TMP@ 換成暫存目錄的實際路徑再寫。"""
        step = {"op": "write", "path": rel}
        data = bytes.fromhex(hexdata) if hexdata is not None else (self.abs(text) if absolute else text).encode("utf-8")
        if hexdata is not None:
            step["hex"] = hexdata
        else:
            step["text"] = text
        if absolute:
            step["abs"] = True
        self.steps.append(step)
        os.makedirs(os.path.dirname(self.p(rel)), exist_ok=True)
        with open(self.p(rel), "wb") as f:
            f.write(data)

    def write_json(self, rel, data, indent=2, tail="\n"):
        self.write(rel, json.dumps(data, ensure_ascii=False, indent=indent) + tail)

    def mkdir(self, rel):
        self.steps.append({"op": "mkdir", "path": rel})
        os.makedirs(self.p(rel), exist_ok=True)

    def rm(self, rel):
        self.steps.append({"op": "rm", "path": rel})
        os.unlink(self.p(rel))

    def rmtree(self, rel):
        self.steps.append({"op": "rmtree", "path": rel})
        shutil.rmtree(self.p(rel))

    def chmod(self, rel, mode):
        self.steps.append({"op": "chmod", "path": rel, "mode": mode})
        os.chmod(self.p(rel), mode)

    def sub(self, rel, old, new):
        """把檔案裡第一個 old 換成 new。"""
        keep = self.read(rel)
        assert old in keep, (rel, old)
        self.steps.append({"op": "sub", "path": rel, "old": old, "new": new})
        with open(self.p(rel), "w", encoding="utf-8", newline="") as f:
            f.write(keep.replace(old, new, 1))

    def append(self, rel, text):
        self.steps.append({"op": "append", "path": rel, "text": text})
        with open(self.p(rel), "a", encoding="utf-8", newline="") as f:
            f.write(text)

    def load(self, rel):
        return json.loads(self.read(rel))

    # ---------------------------------------------------------------- 目錄快照
    def snapshot(self):
        sigs, texts = {}, {}
        for base, dirs, files in os.walk(self.dir):
            if base == self.dir:
                dirs[:] = [d for d in dirs if d != "src"]
            dirs.sort()
            rel_base = os.path.relpath(base, self.dir).replace(os.sep, "/")
            for d in dirs:
                sigs[(d if rel_base == "." else rel_base + "/" + d) + "/"] = "d"
            for f in sorted(files):
                full = os.path.join(base, f)
                rel = f if rel_base == "." else rel_base + "/" + f
                data = Path(full).read_bytes()
                try:
                    text = self.norm(data.decode("utf-8"))
                    data = text.encode("utf-8")
                    if len(text) <= LONG:
                        texts[rel] = text
                except UnicodeDecodeError:
                    pass
                x = "x" if stat.S_IMODE(os.stat(full).st_mode) & 0o111 else "-"
                sigs[rel] = x + ":" + hashlib.sha1(data).hexdigest()[:16]
        return sigs, texts

    def changed(self, base_at="proj"):
        """base_at：這個案例的專案目錄（相對暫存目錄）；和 base 比的時候把它當成 base 的 proj。"""
        after, texts = self.snapshot()
        same, files = [], {}
        for k in sorted(set(self.prev) | set(after)):
            if self.prev.get(k) == after.get(k):
                continue
            as_base = "proj" + k[len(base_at):] if k == base_at + "/" or k.startswith(base_at + "/") else None
            if k in after and as_base is not None and self.w.base.get(as_base) == after[k]:
                same.append(k)
            elif k not in after:
                files[k] = None
            else:
                files[k] = [after[k], texts[k]] if k in texts else after[k]
        self.prev = after
        return files, [len(same), hashlib.sha1("\n".join(same).encode("utf-8")).hexdigest()[:16]]

    # ---------------------------------------------------------------- 執行
    def run(self, *argv, cwd="cwd", tty=None, eof=False, stdin=None, cmp=None, base_at="proj"):
        cmd = [sys.executable, os.path.join(self.src, "init.py")] + [self.abs(a) for a in argv]
        env = {"PATH": os.environ.get("PATH", ""), "HOME": self.p("h"), "PYTHONUTF8": "1", "PYTHONDONTWRITEBYTECODE": "1",
               "PYTHONUNBUFFERED": "1", "LC_ALL": "en_US.UTF-8", "TMPDIR": os.environ.get("TMPDIR", "/tmp")}
        step = {"op": "run", "argv": list(argv), "cwd": cwd}
        if tty is not None:
            step["tty"] = tty
            if eof:
                step["eof"] = True
            master, slave = pty.openpty()
            attrs = termios.tcgetattr(slave)
            attrs[3] &= ~termios.ECHO
            termios.tcsetattr(slave, termios.TCSANOW, attrs)
            os.write(master, tty.encode("utf-8") + (b"\x04" * 80 if eof else b""))
            p = subprocess.Popen(cmd, cwd=self.p(cwd), env=env, stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            os.close(slave)
            try:
                out, err = p.communicate(timeout=120)
            finally:
                p.kill()
                os.close(master)
        else:
            if stdin is not None:
                step["stdin"] = stdin
            r = subprocess.run(cmd, cwd=self.p(cwd), env=env, input=(stdin or "").encode("utf-8"), capture_output=True, timeout=120)
            p, out, err = r, r.stdout, r.stderr
        step["code"] = p.returncode
        step["stdout"] = self.norm(out.decode("utf-8"))
        step["stderr"] = self.norm(err.decode("utf-8"))
        if cmp:
            step["cmp"] = cmp
        if base_at != "proj":
            step["base_at"] = base_at
        step["files"], step["as_base"] = self.changed(base_at)
        self.steps.append(step)
        return step


CRASH = {"stderr": "skip"}     # Python 版直接當掉（traceback）；Rust 版印一行說明，exit code 相同
USAGE = {"stderr": "last"}     # argparse 的錯誤：只比最後一行
T = "@TMP@/proj"

EXISTING_FILES = {
    ".claude/agents/dev-helper.md": "---\nname: dev-helper\ndescription: 使用者自己的 agent（示範客戶 ACME）\n---\n幫忙整理程式。\n",
    ".claude/skills/my-skill/SKILL.md": "---\nname: my-skill\ndescription: 使用者自己的 skill\n---\n照做。\n",
    ".claude/commands/brief.md": "使用者自己的 /brief 指令（和 plugin 的 brief 同名）\n",
    ".claude/workflows/sdlc-plan.js": "// 使用者自己的同名 workflow\nexport const meta = { name: 'sdlc-plan' }\n",
    ".github/workflows/ci.yml": "name: mine\non: [push]\n",
    ".gitignore": "node_modules/\n.env\n",
}
EXISTING_CLAUDE = "# 示範客戶 ACME 的專案\r\n\r\n原本的說明，用 CRLF 換行、結尾沒有換行。"
EXISTING_SETTINGS = {
    "permissions": {"allow": ["Bash(npm test)"]},
    "env": {"MY_VAR": "1"},
    "model": "sonnet",
    "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo user-hook"}]}],
              "Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}]},
}
CUSTOM_CONFIG = (
    "﻿# 示範客戶 ACME 的設定\r\n"
    "PROJECT_NAME=acme-portal\r\n"
    "CLIENT_NAME=\"示範客戶 ACME\"\r\n"
    "REPOS_DIR=\r\n"
    "UPSTREAM_REPO=acme-upstream/portal,acme-upstream/lib\r\n"
    "FORK_REPO=acme-team/portal\r\n"
    "OUTPUT_LANGUAGE=English\r\n"
    "TASK_ID_PREFIX=REQ-\r\n"
    "BRANCH_PREFIX=feat/\r\n"
    "ALLOWED_BRANCH_PREFIXES=feat/,fix/\r\n"
    "RUNS_DIR=/_acme/agent-runs/\r\n"
    "BRIEFS_DIR=\r\n"
    "ATTRIBUTION_LINE=Assisted-by: \"ACME\" bot \\ tools\r\n"
    "NOT_IN_EXAMPLE=ignored\r\n"
    "lower_case=ignored\r\n"
    "PROJECT_NAME=acme-portal-2\r\n"
)


def existing(c, claude=EXISTING_CLAUDE, settings=EXISTING_SETTINGS):
    for rel, body in EXISTING_FILES.items():
        c.write("proj/" + rel, body)
    if claude is not None:
        c.write("proj/CLAUDE.md", claude)
    if settings is not None:
        c.write("proj/.claude/settings.json", json.dumps(settings, ensure_ascii=False))


def record_kv(c):
    text = c.read("proj/.claude/teams/workflow.config")
    return dict(l.split("=", 1) for l in text.splitlines() if re.match(r"^[A-Z_]+=", l))


def legacy_copy(c, name, meth):
    """舊版 init 複製進專案的 workflow 副本（佔位符換成紀錄檔的值、內嵌當時的 profile）。"""
    kv = record_kv(c)
    body = (ROOT / "tests/fixtures/legacy-0.4.1" / name).read_text(encoding="utf-8")
    body = re.sub(r"\{\{([A-Z_]+)\}\}", lambda m: kv.get(m.group(1), m.group(0)), body)
    return body.replace("/*@methodology*/ null /*@end*/", "/*@methodology*/ " + json.dumps(meth, ensure_ascii=False, separators=(",", ":")) + " /*@end*/")


def basics(w):
    c = Case(w, "base：全新專案 --defaults")
    c.run("--target", T, "--defaults")
    w.base = dict(c.prev)
    # base 訂下來之後重算這一步的紀錄（全部都和 base 相同）
    c.prev = {k: v for k, v in c.prev.items() if not k.startswith("proj")}
    c.steps[-1]["files"], c.steps[-1]["as_base"] = c.changed()

    c = Case(w, "位置參數當目標")
    c.run(T, "--defaults")
    c = Case(w, "相對路徑的目標、選項寫前綴與等號")
    c.run("--tar=../proj", "--def")
    c = Case(w, "~ 開頭的目標")
    c.run("--target", "~/proj", "--defaults", base_at="h/proj")
    c = Case(w, "沒給 --defaults、stdin 不是終端機：不詢問")
    c.run("--target", T, stdin="不會被讀到\n")
    c = Case(w, "目標目錄有多層還不存在、結尾有斜線")
    c.run("--target", "@TMP@/a/b/proj/", "--defaults", base_at="a/b/proj")
    c = Case(w, "重跑：模版檔略過，VERSION 與紀錄檔不重寫")
    c.run("--target", T, "--defaults")
    c.run("--target", T, "--defaults")
    c.rm("proj/.claude/teams/VERSION")
    c.rm("proj/.claude/teams/rules.md")
    c.run("--target", T, "--defaults")
    c.rm("proj/.claude/teams/workflow.config")
    c.run("--target", T, "--defaults")


def configs(w):
    c = Case(w, "--config：BOM、CRLF、引號、重複鍵、空值、範例沒有的鍵")
    c.write("my.config", CUSTOM_CONFIG)
    c.run("--target", T, "--config", "@TMP@/my.config")
    c.run("--target", T, "--force")
    c = Case(w, "--config 相對路徑")
    c.write("cwd/sub/my.config", "PROJECT_NAME=rel-config\nRUNS_DIR=_runs_here\n")
    c.run("--target", T, "--config", "./sub//my.config")
    c = Case(w, "目前目錄的 workflow.config 自動採用；有紀錄檔之後不覆寫紀錄")
    c.write("cwd/workflow.config", "PROJECT_NAME=from-cwd\nBRIEFS_DIR=_in/briefs\n")
    c.run("--target", T)
    c.write("cwd/workflow.config", "PROJECT_NAME=from-cwd-2\n")
    c.run("--target", T, "--overwrite-user-files")
    c = Case(w, "模版根的 workflow.config 自動採用", own_src=True)
    c.write("src/workflow.config", "PROJECT_NAME=from-here\n")
    c.run("--target", T)
    c.write("cwd/workflow.config", "PROJECT_NAME=cwd-wins\n")
    c.run("--target", "@TMP@/proj2", base_at="proj2")
    c = Case(w, "設定值裡有佔位符的樣子：列成沒替換的佔位符")
    c.write("my.config", "PROJECT_NAME={{NOPE}}\nCLIENT_NAME={{PROJECT_NAME}}\n")
    c.run("--target", T, "--config", "@TMP@/my.config")
    c = Case(w, "設定值有 JSON 要跳脫的字元")
    c.write("my.config", "PROJECT_NAME=a\"b\\c\td\nTASK_AUTHORITY=\"\nSTAKEHOLDER=\"需求方\nDECISION_OWNER=拍板\"\n")
    c.run("--target", T, "--config", "@TMP@/my.config")
    c = Case(w, "METHODOLOGY 由設定檔指定；--defaults 也照設定檔的佔位符")
    c.write("my.config", "METHODOLOGY=lightweight\nBRANCH_PREFIX=\n")
    c.run("--target", T, "--config", "@TMP@/my.config")
    c.run("--target", "@TMP@/proj2", "--config", "@TMP@/my.config", "--defaults", base_at="proj2")


def options(w):
    c = Case(w, "--hooks project")
    c.run("--target", T, "--defaults", "--hooks", "project")
    c.run("--target", T, "--defaults", "--hooks", "project")
    c.run("--target", T, "--force", "--hooks", "project")
    c = Case(w, "--hooks project --python")
    c.write("bin/fake python", "#!/bin/sh\n")
    c.run("--target", T, "--defaults", "--hooks=project", "--python", "@TMP@/bin/fake python")
    c.run("--target", T, "--force", "--hooks", "project", "--python", "../bin/../bin/fake python")
    c.run("--target", T, "--force", "--hooks", "project")
    c = Case(w, "--hooks project 之後改回 plugin：提醒擇一")
    c.run("--target", T, "--defaults", "--hooks", "project")
    c.run("--target", T, "--force")
    c = Case(w, "--with-mcp")
    c.run("--target", T, "--defaults", "--with-mcp")
    c = Case(w, "--with-merge-gate")
    c.run("--target", T, "--defaults", "--with-merge-gate")
    c.run("--target", T, "--defaults")
    c = Case(w, "--with-workflows：覆寫檔與標記；升級不覆蓋改過的覆寫檔")
    c.run("--target", T, "--defaults", "--with-workflows")
    c.sub("proj/.claude/workflows/sdlc-plan.js", "export const meta", "// 我改過這支\nexport const meta")
    c.run("--target", T, "--upgrade", "--with-workflows")
    c.run("--target", T, "--upgrade")
    c.run("--target", T, "--defaults", "--with-workflows")
    c.run("--target", T, "--with-workflows", "--overwrite-user-files", "--defaults")
    c = Case(w, "全部選項一起")
    c.run(T, "--defaults", "--hooks", "project", "--with-mcp", "--with-merge-gate", "--with-workflows")
    c.run(T, "--upgrade", "--hooks", "project", "--with-mcp", "--with-merge-gate")


ANSWERS = {
    "lightweight＋各項慣例": {
        "methodology": "lightweight",
        "code_style": {"tool": "ruff", "command": "ruff check .", "description": "提交前跑 ruff check，不得新增違規。"},
        "commit": "類型: 摘要（示範客戶 ACME 的格式）",
        "commit_regex": "^(feat|fix): .+",
        "branch_prefix": "work/",
        "sync_strategy": "transit",
        "pr_size": {"max_lines": 300, "max_files": "不限"},
        "doc_format": "AsciiDoc",
        "test_first": True,
        "requirements": {"source": "json", "path": "docs/requirements.json"},
        "main_session_push_merge": True,
    },
    "字串形式的 code_style、commit 物件、regex 清空": {
        "code_style": "照 repo 既有的 lint 設定",
        "commit": {"format": "自訂格式", "examples": {"feat": "feat: 示範"}},
        "commit_regex": "",
        "branch_prefix": "feature/",
        "sync_strategy": "rebase",
        "pr_size": {"max_lines": "0", "max_files": 12.7},
        "doc_format": {"format": "Markdown", "lint": "markdownlint"},
        "test_first": 0,
        "requirements": "不是物件就不套用",
        "main_session_push_merge": "會",
    },
    "pr_size 的各種寫法": {"pr_size": {"max_lines": " 250 ", "max_files": True}, "branch_prefix": "", "sync_strategy": "",
                     "main_session_push_merge": "no"},
    "pr_size 空的": {"pr_size": None, "test_first": "", "main_session_push_merge": 1},
    "pr_size 負數與 None 字樣": {"pr_size": {"max_lines": -5, "max_files": "None"}, "main_session_push_merge": " YES "},
    "read_limits 字串": {"read_limits": "warn"},
    "read_limits 物件": {"read_limits": {"mode": "enforce", "whole_file_lines": "800", "read_budget": {"default": 120000, "roles": {"tpm-researcher": "90000", "dev-verifier": 0}}, "max_agents": 25}},
    "read_limits 整數額度、max_agents 0": {"read_limits": {"read_budget": 50000, "max_agents": 0, "whole_file_lines": 0}},
    "read_limits 空的 mode": {"read_limits": {"mode": ""}},
    "constraints 簡寫字串": {"constraints": "ARCHITECTURE.md, docs/DESIGN.md=design+build、deliver, -, docs\\win\\Review.MD=plan test,  , ARCHITECTURE.md"},
    "constraints 清單（物件與簡寫混用、同名、長檔名）": {"constraints": [
        {"id": "arch", "file": "ARCHITECTURE.md", "stages": ["design"], "trace": "table"},
        "docs/arch.md",
        "other/arch.md=merge",
        "docs/" + "很長的檔名-" * 3 + "a-very-long-file-name-that-goes-past-forty-characters.md",
        "x/" + "a-very-long-file-name-that-goes-past-forty-characters.txt=sync",
        "…….md",
        7,
    ]},
    "constraints 空的": {"constraints": []},
    "overrides": {"overrides": {"conventions": {"pr_size": {"max_files": 3}, "extra": {"k": [1, 2.5, None, True]}}, "description": "示範客戶 ACME 的做法"},
                  "branch_prefix": "x/"},
}
BAD_ANSWERS = {
    "sync_strategy 不認得": {"sync_strategy": "squash"},
    "read_limits 型別不對": {"read_limits": 5},
    "read_limits 清單": {"read_limits": ["warn"]},
    "read_limits.mode 不認得": {"read_limits": {"mode": "strict"}},
    "read_limits 負數": {"read_limits": {"whole_file_lines": -1}},
    "read_limits 不是數字": {"read_limits": {"read_budget": "很多"}},
    "read_limits 布林": {"read_limits": {"max_agents": True}},
    "read_limits 小數": {"read_limits": {"read_budget": {"default": 1.5}}},
    "read_limits 角色額度不對": {"read_limits": {"read_budget": {"roles": {"tpm-writer": None}}}},
    "pr_size 不是數字": {"pr_size": {"max_lines": "abc"}},
    "pr_size 清單": {"pr_size": {"max_files": [1]}},
    "constraints 的段不認得": {"constraints": "docs/A.md=design+review+ship"},
    "profile 找不到": {"methodology": "nope"},
    "profile 不合 schema": {"overrides": {"stages": "不是物件"}},
    "profile 不合 schema（很多條）": {"overrides": {"stages": 1, "conventions": {"pr_size": {"max_lines": "x"}, "sync": {"strategy": "y"}}, "recipes": [], "modifiers": 3, "limits": {"mode": "z"}, "name": 5}},
    "constraints 語意規則": {"constraints": [{"id": "a", "file": "/abs/path.md", "stages": ["design"]}, {"id": "a", "file": "../up.md", "stages": ["plan"]}]},
}


def answers(w):
    for name, ans in ANSWERS.items():
        c = Case(w, "--answers：" + name)
        c.write("answers.json", json.dumps(ans, ensure_ascii=False))
        c.run("--target", T, "--answers", "@TMP@/answers.json")
    for name, ans in BAD_ANSWERS.items():
        c = Case(w, "--answers 錯誤：" + name)
        c.write("answers.json", json.dumps(ans, ensure_ascii=False))
        c.run("--target", T, "--answers", "@TMP@/answers.json")
    c = Case(w, "--answers：BOM、相對路徑、搭配 --defaults 與 --config")
    c.write("cwd/a.json", "﻿" + json.dumps({"methodology": "lightweight", "branch_prefix": "zz/"}))
    c.write("my.config", "PROJECT_NAME=with-answers\nMETHODOLOGY=default\nALLOWED_BRANCH_PREFIXES= zz/ , yy/\n")
    c.run("--target", T, "--answers", "a.json", "--defaults")
    c.run("--target", "@TMP@/proj2", "--answers", "a.json", "--config", "@TMP@/my.config", base_at="proj2")
    c = Case(w, "--answers：methodology 指到自己的 profile 檔")
    prof = json.loads((ROOT / "scaffold/.claude/teams/methodologies/lightweight.json").read_text(encoding="utf-8"))
    prof["name"] = "acme-flow"
    prof["description"] = "示範客戶 ACME 自己的做法"
    prof["conventions"].pop("branch", None)
    c.write_json("cwd/mine.json", prof)
    c.write("a.json", json.dumps({"methodology": "mine.json"}))
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.write("my.config", "METHODOLOGY=@TMP@/cwd/mine.json\n", absolute=True)
    c.run("--target", "@TMP@/proj2", "--config", "@TMP@/my.config", base_at="proj2")
    c.write("bad.json", "{不是 JSON")
    c.write("a2.json", json.dumps({"methodology": "@TMP@/bad.json"}), absolute=True)
    c.run("--target", "@TMP@/proj3", "--answers", "@TMP@/a2.json")
    c.write("cwd/list.json", "[1, 2]")
    c.write("a3.json", json.dumps({"methodology": "list.json"}))
    c.run("--target", "@TMP@/proj4", "--answers", "@TMP@/a3.json", cmp=CRASH)
    c = Case(w, "--answers 錯誤：找不到、不是 JSON、不是物件")
    c.run("--target", T, "--answers", "@TMP@/nope.json")
    c.write("a.json", "{\"methodology\": ")
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.write("a.json", "[\"default\"]")
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.write("a.json", "")
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.write("a.json", None, hexdata="ff fe 00".replace(" ", ""))
    c.run("--target", T, "--answers", "@TMP@/a.json", cmp=CRASH)


def interactive(w):
    c = Case(w, "互動：全部 Enter")
    c.run("--target", T, tty=ENTER)
    c = Case(w, "互動：每題都回答")
    ans = ["acme-shop", "示範客戶 ACME", "-", "acme-upstream/shop", "acme-team/shop", "secrets,acme-upstream/private", "English", "REQ-", "需求系統",
           "示範決策者", "示範需求方", "feat/", "feat/,fix/", "_acme/runs", "-", "Assisted-by: ACME bot", "acme-ds", "",
           # 開發規範
           "lightweight", "y", "ruff check .", "類型: 摘要", "^(feat|fix): .+", "work/", "merge", "400", "0", "AsciiDoc",
           "ARCHITECTURE.md, docs/DESIGN.md=design+build", "enforce", "600", "80000",
           # 主 session
           "會"]
    c.run("--target", T, tty="\n".join(ans) + "\n" + ENTER)
    c = Case(w, "互動：留空（-）、前後空白、大寫 Y")
    ans = ["  my proj  ", "-", "", "", "", "", "", "-", "", "", "", "", "", "", "", "", "", "",
           " default ", " N ", "-", "", "-", "-", "-", "-", "-", "", "-", "-", "0", "0", "Yes please"]
    c.run("--target", T, tty="\n".join(ans) + "\n" + ENTER)
    c = Case(w, "互動：讀取上限 warn、其餘 Enter；重跑有紀錄檔仍詢問")
    ans = [""] * 18 + ["", "n", "", "", "", "", "", "", "", "", "", "warn", "", "", "y"]
    c.run("--target", T, tty="\n".join(ans) + "\n" + ENTER)
    c.run("--target", T, tty=ENTER)
    c = Case(w, "互動：答到一半 EOF")
    c.run("--target", T, tty="eof-proj\n示範客戶 ACME\n", eof=True)
    c = Case(w, "互動：一開始就 EOF")
    c.run("--target", T, tty="", eof=True)
    c = Case(w, "互動：有答案檔時只問佔位符")
    c.write("a.json", json.dumps({"methodology": "lightweight", "main_session_push_merge": "y", "test_first": True}))
    c.run("--target", T, "--answers", "@TMP@/a.json", tty="from-tty\n" + ENTER)
    c = Case(w, "互動：profile 名稱打錯")
    c.run("--target", T, tty="\n" * 18 + "nope\n" + ENTER)
    c = Case(w, "互動：同步策略打錯")
    c.run("--target", T, tty="\n" * 18 + "\n" * 6 + "squash\n" + ENTER)
    c = Case(w, "互動：PR 上限不是數字")
    c.run("--target", T, tty="\n" * 18 + "\n" * 7 + "很多\n" + ENTER)
    c = Case(w, "終端機但有 --defaults、--config、--upgrade：不詢問")
    c.run("--target", T, "--defaults", tty="不會被讀到\n")
    c.write("my.config", "PROJECT_NAME=tty-config\n")
    c.run("--target", "@TMP@/proj2", "--config", "@TMP@/my.config", tty="不會被讀到\n", base_at="proj2")
    c.run("--target", T, "--upgrade", tty="不會被讀到\n")
    c.rm("proj/.claude/teams/workflow.config")
    c.run("--target", T, "--force", tty="force-asks\n" + ENTER)


def existing_projects(w):
    c = Case(w, "既有專案：--hooks project 跑兩次，第二次加 --force")
    existing(c)
    c.run("--target", T, "--defaults", "--hooks", "project")
    c.run("--target", T, "--defaults", "--hooks", "project", "--force")
    c.run("--target", T, "--defaults", "--hooks", "project")
    c = Case(w, "既有專案：--hooks plugin；--with-workflows、--with-merge-gate 碰到使用者同名檔")
    existing(c)
    c.write("proj/.github/workflows/agent-merge-gate.yml", "name: 使用者自己的同名 CI\n")
    c.run("--target", T, "--defaults", "--with-workflows", "--with-merge-gate")
    c.run("--target", T, "--force", "--with-workflows", "--with-merge-gate")
    c.run("--target", T, "--upgrade")
    c = Case(w, "既有專案：--overwrite-user-files 整份覆寫")
    existing(c)
    c.write("proj/.mcp.json", "{\"mcpServers\": {\"mine\": {\"command\": \"x\"}}}")
    c.run("--target", T, "--defaults", "--hooks", "project", "--with-mcp", "--with-workflows", "--with-merge-gate")
    c.write("a.json", json.dumps({"methodology": "lightweight", "main_session_push_merge": True}))
    c.run("--target", T, "--overwrite-user-files", "--hooks", "project", "--with-mcp", "--with-workflows", "--with-merge-gate", "--answers", "@TMP@/a.json")
    for name, body in [("結尾有換行", "# 我的專案\n\n說明。\n"), ("結尾沒有換行", "# 我的專案\n\n說明。"), ("空檔", ""), ("CRLF 結尾有換行", "# 我的專案\r\n說明。\r\n"),
                       ("只有 CR 結尾", "# 我的專案\r"), ("已經有引用", "# 我的專案\n@.claude/teams/agent-workflow.CLAUDE.md\n")]:
        c = Case(w, "既有 CLAUDE.md：" + name)
        c.write("proj/CLAUDE.md", body)
        c.run("--target", T, "--defaults")
        c.run("--target", T, "--defaults")
        c.append("proj/.claude/teams/agent-workflow.CLAUDE.md", "\n自己加在專案特定段後面的內容\n")
        c.sub("proj/.claude/teams/agent-workflow.CLAUDE.md", "\n", "\n（改過模版的部分）\n")
        c.run("--target", T, "--force")
    c = Case(w, "既有 CLAUDE.md 不是 UTF-8")
    c.write("proj/CLAUDE.md", None, hexdata="ff fe 41 00".replace(" ", ""))
    c.run("--target", T, "--defaults", cmp=CRASH)
    c = Case(w, "init 產生的 CLAUDE.md：重跑略過，--force 只更新專案特定之前")
    c.run("--target", T, "--defaults")
    c.append("proj/CLAUDE.md", "\n我們的專案特定內容\n")
    c.sub("proj/CLAUDE.md", "# my-project", "# 改過的標題")
    c.run("--target", T, "--defaults")
    c.run("--target", T, "--force")
    c.sub("proj/CLAUDE.md", "## 專案特定", "## 專案特定（改了標題）")
    c.run("--target", T, "--force")

    settings_cases = [
        ("不是合法 JSON", "{ 不是 JSON", None),
        ("最外層是陣列", "[1, 2]", None),
        ("BOM、縮排 4、數字與巢狀", "﻿" + json.dumps({"permissions": {"allow": [], "deny": ["Read(./secret)"], "defaultMode": "plan"}, "n": 1.0, "big": 12345678901234567890, "e": 1e-7, "$schema": "mine"}, indent=4), None),
        ("hooks 不是物件", json.dumps({"hooks": ["x"], "permissions": "all"}), None),
        ("空物件", "{}", None),
        ("重複鍵與跳脫字元", "{\"a\": 1, \"b\": \"\\u4e2d\\n\\u0001\\/\", \"a\": {\"x\": null}, \"permissions\": {\"allow\": [\"Bash(ls)\"]}}", None),
    ]
    for name, body, _ in settings_cases:
        c = Case(w, "既有 settings.json：" + name)
        c.write("proj/.claude/settings.json", body)
        c.run("--target", T, "--defaults")
        c.run("--target", T, "--force", "--hooks", "project")
        c.run("--target", T, "--force", "--hooks", "project")
    c = Case(w, "既有 settings.json：舊的 awf hook 掛在別的 matcher、和使用者的 hook 混在一起")
    old = {"hooks": {
        "PreToolUse": [
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "python3 \"${CLAUDE_PROJECT_DIR}/.claude/hooks/awf_hook.py\" guard-shell"}]},
            {"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "echo mine"}, {"type": "command", "command": "\"/old/python\" \"x/awf_hook.py\" guard-paths", "timeout": 3}]},
            {"matcher": "Other", "hooks": []},
            {"matcher": "Keep", "hooks": [{"type": "command", "command": "awf_hook.py unknown-mode"}, {"type": "command", "command": "awf_hook.pyc envelope"}]},
            "不是物件的項目",
            {"matcher": "NoHooks"},
        ],
        "SubagentStop": [{"hooks": [{"type": "command", "command": "py awf_hook.py   envelope"}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "py awf_hook.py envelope"}]}],
    }}
    c.write_json("proj/.claude/settings.json", old)
    c.run("--target", T, "--defaults", "--hooks", "project")
    c = Case(w, "既有 settings.json：hooks 的事件不是陣列、hook 不是物件")
    c.write_json("proj/.claude/settings.json", {"hooks": {"PreToolUse": {"matcher": "x"}}})
    c.run("--target", T, "--defaults", "--hooks", "project", cmp=CRASH)
    c.write_json("proj/.claude/settings.json", {"hooks": {"PreToolUse": [{"hooks": ["字串"]}]}})
    c.run("--target", T, "--defaults", "--hooks", "project", cmp=CRASH)
    c = Case(w, "既有 settings.json：已經和模版一樣")
    c.run("--target", T, "--defaults")
    c.rmtree("proj/.claude/teams")
    c.run("--target", T, "--defaults")

    c = Case(w, "既有 .mcp.json：預設不碰，--with-mcp 深度合併")
    mine = {"mcpServers": {"browser": {"command": "mine", "args": ["--mine"]}, "db": {"command": "pg"}}, "other": [1]}
    c.write_json("proj/.mcp.json", mine, indent=None, tail="")
    c.run("--target", T, "--defaults")
    c.run("--target", T, "--defaults", "--with-mcp")
    c.run("--target", T, "--force", "--with-mcp")
    c.write("proj/.mcp.json", "不是 JSON")
    c.run("--target", T, "--force", "--with-mcp")

    c = Case(w, "共用目錄裡的同名檔：看得出是模版的才更新")
    c.run("--target", T, "--defaults", "--hooks", "project", "--with-merge-gate")
    c.write("proj/.claude/hooks/guard.py", "# 使用者自己的同名檔\n")
    c.write("proj/.claude/hooks/awf_config.py", "# 舊版 agent-workflow 的檔\n")
    c.write("proj/.claude/hooks/envelope_hook.py", None, hexdata="ff00ff")
    c.write("proj/.github/workflows/agent-merge-gate.yml", "# agent-merge-gate 舊版\n")
    c.run("--target", T, "--defaults", "--hooks", "project", "--with-merge-gate")
    c.run("--target", T, "--force", "--hooks", "project", "--with-merge-gate")

    c = Case(w, "主 session 會 push／合併：新專案放寬，既有 guard.config 不改")
    c.write("a.json", json.dumps({"main_session_push_merge": "Y"}))
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.run("--target", T, "--answers", "@TMP@/a.json")
    c.run("--target", T, "--answers", "@TMP@/a.json", "--force")
    c.run("--target", T, "--answers", "@TMP@/a.json", "--overwrite-user-files")

    c = Case(w, "模版裡的非文字檔、可執行檔、CRLF、__pycache__、.DS_Store", own_src=True)
    c.write("src/scaffold/.claude/teams/tools/blob.bin", None, hexdata="00ff10807f")
    c.write("src/scaffold/.claude/teams/tools/run.sh", "#!/bin/sh\necho {{PROJECT_NAME}}\n")
    c.chmod("src/scaffold/.claude/teams/tools/run.sh", 0o755)
    c.write("src/scaffold/.claude/teams/crlf.md", "第一行 {{CLIENT_NAME}}\r\n第二行\r只有 CR\n")
    c.write("src/scaffold/.claude/teams/tools/__pycache__/x.pyc", "不複製")
    c.write("src/scaffold/.DS_Store", "不複製")
    c.write("src/scaffold/_runs/sub/keep.json.example", "{\"p\": \"{{PROJECT_NAME}}\", \"x\": \"{{UNKNOWN_KEY}}\"}\n")
    c.write("src/scaffold/a-b/x.md", "排序：a-b 在 a/ 之後\n")
    c.write("src/scaffold/a/y.md", "排序\n")
    c.write("src/plugin/hooks/extra.bin", None, hexdata="fffe")
    c.write("src/plugin/hooks/__pycache__/y.pyc", "不複製")
    c.write("my.config", "PROJECT_NAME=a\"b\nCLIENT_NAME=示範客戶 ACME\n")
    c.run("--target", T, "--config", "@TMP@/my.config", "--hooks", "project")
    c.write("proj/.claude/teams/tools/blob.bin", "改過")
    c.chmod("proj/.claude/teams/tools/run.sh", 0o644)
    c.run("--target", T, "--force", "--hooks", "project")


def upgrades(w):
    c = Case(w, "--upgrade：還沒 init 過")
    c.run("--target", T, "--upgrade")
    c.mkdir("proj2/.claude")
    c.run("--target", "@TMP@/proj2", "--upgrade")

    c = Case(w, "--upgrade：自訂過的專案（methodology、guard.config、專案規則、舊模版檔、四種 workflow）")
    c.run("--target", T, "--defaults")
    meth = c.load("proj/.claude/teams/methodology.json")
    meth["conventions"].pop("sync", None)
    for k in ("recipes", "modifiers", "models", "limits", "knowledge"):
        meth.pop(k, None)
    meth["conventions"]["pr_size"] = {"max_lines": 123, "max_files": 7}
    c.write_json("proj/.claude/teams/methodology.json", meth, tail="")
    guard = c.load("proj/.claude/teams/guard.config.json")
    guard["main_session_git_rules"] = False
    guard["auto_merge"]["upstream_authorized_by"] = {"acme/app": {"by": "示範決策者", "link": "https://example.com/acme", "scope": "feature/*", "expires": "2099-01-01"}}
    guard["extension_agents"]["acme-db-reviewer"] = "tpm-reviewer"
    guard["auto_merge"].pop("carry_verdict_on_rebase", None)
    guard.pop("linked_repos", None)
    guard["requirements"].pop("_說明", None)
    guard["_說明"] = "舊版的說明"
    c.write_json("proj/.claude/teams/guard.config.json", guard, tail="")
    c.append("proj/.claude/teams/rules.md", "\n- 示範客戶 ACME 的專案規則：不碰 legacy/ 目錄\n")
    c.sub("proj/.claude/teams/rules.md", "## 專案規則", "（舊版通用規則）\n\n## 專案規則")
    c.append("proj/CLAUDE.md", "\n我們的專案特定內容\n")
    c.append("proj/.claude/teams/tools/schemalib.py", "\n# 舊版殘留\n")
    c.write("proj/.claude/teams/tools/old_tool.py", "# 舊版才有的工具\n")
    c.write("proj/.claude/teams/schemas/payloads/old.schema.json", "{}\n")
    c.write("proj/.claude/teams/tools/__pycache__/old.pyc", "不算殘留")
    c.write("proj/.claude/teams/VERSION", "0.1.0\n")
    old = {n: legacy_copy(c, n, meth) for n in ("event-meeting.js", "event-reqs-change.js")}
    c.write("proj/.claude/workflows/event-meeting.js", old["event-meeting.js"])
    c.write("proj/.claude/workflows/event-reqs-change.js", old["event-reqs-change.js"].replace("const A = args || {}", "const A = args || {} // 我改過"))
    c.write("proj/.claude/workflows/sdlc-plan.js", "export const meta = { name: 'sdlc-plan', description: '我的版本' }\nconst X = /*@schema:findings*/ null /*@end*/\n// awf-override: 0.1.0\n")
    c.write("proj/.claude/workflows/sdlc-test.js", "// 使用者自己的同名檔，沒有模版的痕跡\n")
    c.write("proj/.claude/workflows/my-own.js", "export const meta = { name: 'my-own', description: '使用者自己的' }\n")
    c.run("--target", T, "--upgrade")
    c.run("--target", T, "--upgrade", "--adopt-new-fields")
    c.run("--target", T, "--upgrade", "--adopt-new-fields")
    c.run("--target", T, "--force")
    c.run("--target", T, "--defaults", "--adopt-new-fields")

    c = Case(w, "--upgrade：原樣的舊副本（CRLF）移除後 workflows 目錄清空")
    c.run("--target", T, "--defaults")
    meth = c.load("proj/.claude/teams/methodology.json")
    c.write("proj/.claude/workflows/event-meeting.js", legacy_copy(c, "event-meeting.js", meth).replace("\n", "\r\n"))
    c.rm("proj/.claude/teams/VERSION")
    c.run("--target", T, "--upgrade")
    c = Case(w, "--upgrade：同名的 workflow 不是 UTF-8（整理時跳過，內嵌失敗）")
    c.run("--target", T, "--defaults")
    c.write("proj/.claude/workflows/event-demo-check.js", None, hexdata="ff")
    c.run("--target", T, "--upgrade", cmp=CRASH)
    c = Case(w, "--upgrade：佔位符的值對不上的舊副本保留；--config 給對的值才移除")
    c.write("my.config", "AGENT_NS=acme:\nOUTPUT_LANGUAGE=English\n")
    c.run("--target", T, "--config", "@TMP@/my.config")
    meth = c.load("proj/.claude/teams/methodology.json")
    body = legacy_copy(c, "event-reqs-change.js", meth)
    c.write("proj/.claude/workflows/event-reqs-change.js", body)
    c.write("other.config", "AGENT_NS=other:\n")
    c.run("--target", T, "--upgrade", "--config", "@TMP@/other.config")
    c.run("--target", T, "--upgrade", "--config", "@TMP@/my.config")
    c = Case(w, "--force：沒有紀錄檔時用範例預設值；之後 --config 換值")
    c.write("my.config", "RUNS_DIR=_acme/agent-runs\nPROJECT_NAME=keep-me\n")
    c.run("--target", T, "--config", "@TMP@/my.config")
    c.run("--target", T, "--force")
    c.write("cwd/workflow.config", "PROJECT_NAME=cwd-ignored\n")
    c.run("--target", T, "--force")
    c.rm("proj/.claude/teams/workflow.config")
    c.rm("cwd/workflow.config")
    c.run("--target", T, "--force")
    c.write("new.config", "PROJECT_NAME=changed\nRUNS_DIR=_new/runs\n")
    c.run("--target", T, "--upgrade", "--config", "@TMP@/new.config")

    meth_cases = [
        ("不是合法 JSON", "{ 壞掉", []),
        ("不是物件", "[]", []),
        ("不合 schema", None, [("requirements", "壞掉")]),
        ("name 是 lightweight、少幾個欄位", None, [("name", "lightweight")]),
        ("沒有 name", None, [("name", None)]),
    ]
    for name, body, edits in meth_cases:
        c = Case(w, "--upgrade：methodology.json " + name)
        c.run("--target", T, "--defaults")
        if body is None:
            meth = c.load("proj/.claude/teams/methodology.json")
            for k, v in edits:
                if v is None:
                    meth.pop(k, None)
                else:
                    meth[k] = v
            meth.pop("limits", None)
            meth["conventions"].pop("sync", None)
            meth["conventions"]["pr_size"] = {"max_lines": 50, "max_files": 9}
            meth["stages"]["design"].pop("system_design", None)
            meth["stages"]["build"].pop("unit_tests", None)
            c.write_json("proj/.claude/teams/methodology.json", meth)
        else:
            c.write("proj/.claude/teams/methodology.json", body)
        c.run("--target", T, "--upgrade")
        c.run("--target", T, "--upgrade", "--adopt-new-fields")
        c.run("--target", T, "--force", "--adopt-new-fields")
    c = Case(w, "--adopt-new-fields：補上後不合 schema 就不寫回；既有值不是物件不補")
    c.run("--target", T, "--defaults")
    meth = c.load("proj/.claude/teams/methodology.json")
    meth["stages"]["design"] = "自己寫的字串"
    meth.pop("models", None)
    c.write_json("proj/.claude/teams/methodology.json", meth)
    c.run("--target", T, "--upgrade", "--adopt-new-fields")

    c = Case(w, "內嵌失敗：擴充 agent 沒登記（--upgrade exit 1，--force 只警告）")
    c.run("--target", T, "--defaults", "--with-workflows")
    meth = c.load("proj/.claude/teams/methodology.json")
    meth["extensions"] = [{"name": "db-review", "agent": "acme-db-reviewer", "stage": "test", "instructions": "檢查 migration"}]
    c.write_json("proj/.claude/teams/methodology.json", meth)
    c.run("--target", T, "--upgrade")
    c.run("--target", T, "--force")
    c.run("--target", T, "--defaults")
    c.sub("proj/.claude/teams/guard.config.json", "\"extension_agents\": {", "\"extension_agents\": {\n    \"acme-db-reviewer\": \"tpm-reviewer\",")
    c.run("--target", T, "--upgrade", "--with-workflows")

    c = Case(w, "--force：guard.config.json 壞掉、rules.md 沒有專案規則標題")
    c.run("--target", T, "--defaults")
    c.write("proj/.claude/teams/guard.config.json", "[\"不是物件\"]")
    c.sub("proj/.claude/teams/rules.md", "## 專案規則", "## 我們的規則")
    c.run("--target", T, "--force")
    c.write("proj/.claude/teams/guard.config.json", "{ 壞掉")
    c.run("--target", T, "--upgrade")
    c = Case(w, "--force：guard.config.json 和模版一樣、只有順序不同")
    c.run("--target", T, "--defaults")
    guard = c.load("proj/.claude/teams/guard.config.json")
    c.write_json("proj/.claude/teams/guard.config.json", dict(reversed(list(guard.items()))), indent=4)
    c.run("--target", T, "--force")
    c = Case(w, "--hooks project：settings.json 壞掉時提醒手動註冊")
    c.write("proj/.claude/settings.json", "{ 壞掉")
    c.run("--target", T, "--defaults", "--hooks", "project")
    c = Case(w, "legacy.json 壞掉或沒有：同名檔都當成改過的", own_src=True)
    c.run("--target", T, "--defaults")
    meth = c.load("proj/.claude/teams/methodology.json")
    c.write("proj/.claude/workflows/event-meeting.js", legacy_copy(c, "event-meeting.js", meth))
    c.write("proj/.claude/workflows/sdlc-plan.js", "// agent-workflow 的舊副本\n")
    c.write("src/plugin/workflow-templates/legacy.json", "{ 壞掉")
    c.run("--target", T, "--upgrade")
    c.rm("src/plugin/workflow-templates/legacy.json")
    c.run("--target", T, "--upgrade")


def errors(w):
    c = Case(w, "錯誤：參數")
    c.run()
    c.run("--defaults")
    c.run("--target", "")
    c.run("--target", T, "@TMP@/other")
    c.run("--target", T, "--hooks", "both")
    c.run("--target", T, "--config", "@TMP@/nope.config")
    c.run("--target", T, "--config", "@TMP@")
    c.run("--target", T, "--defaults", "--python", "@TMP@/nope/python")
    c.run("--target", T, "--defaults", "--python", "@TMP@")
    c.run("--target", T, "--nope", cmp=USAGE)
    c.run("--target", T, "extra", "more", cmp=USAGE)
    c.run("--target", cmp=USAGE)
    c.run("--target", "--defaults", cmp=USAGE)
    c.run("--with-m", T, cmp=USAGE)
    c.run("--force=1", T, cmp=USAGE)
    c.run("-x", T, cmp=USAGE)
    c.run("-h", cmp={"stdout": "skip"})
    c.run(T, "--help", cmp={"stdout": "skip"})
    c.run("--he", cmp={"stdout": "skip"})
    c = Case(w, "錯誤：目標是檔案")
    c.write("proj", "我是檔案")
    c.run("--target", T, "--defaults", cmp=CRASH)
    c = Case(w, "錯誤：模版缺 scaffold", own_src=True)
    c.rmtree("src/scaffold")
    c.run("--target", T, "--defaults")
    c = Case(w, "錯誤：模版缺 workflow.config.example", own_src=True)
    c.rm("src/workflow.config.example")
    c.run("--target", T, "--defaults")
    c = Case(w, "錯誤：plugin.json 讀不到 version", own_src=True)
    c.write("src/plugin/.claude-plugin/plugin.json", "{\"name\": \"x\"}")
    c.run("--target", T, "--defaults")
    c.write("src/plugin/.claude-plugin/plugin.json", "{ 壞掉")
    c.run("--target", T, "--defaults")
    c.write("src/plugin/.claude-plugin/plugin.json", "{\"version\": 5}")
    c.run("--target", T, "--defaults")
    c.rm("src/plugin/.claude-plugin/plugin.json")
    c.run("--target", T, "--defaults")
    c = Case(w, "錯誤：設定檔不是 UTF-8")
    c.write("my.config", None, hexdata="50524f4a3dff0a")
    c.run("--target", T, "--config", "@TMP@/my.config", cmp=CRASH)
    c = Case(w, "模版缺 plugin 的 hooks 與範本目錄也能跑", own_src=True)
    c.rmtree("src/plugin/hooks")
    c.rmtree("src/plugin/workflow-templates")
    c.run("--target", T, "--defaults", "--hooks", "project", "--with-workflows")
    c.run("--target", T, "--upgrade", "--hooks", "project")


def main():
    w = World()
    try:
        for part in (basics, configs, options, answers, interactive, existing_projects, upgrades, errors):
            part(w)
    finally:
        shutil.rmtree(w.root, ignore_errors=True)
    runs = sum(1 for c in w.cases for s in c["steps"] if s["op"] == "run")
    doc = {"_說明": "init 的對跑基準（劇本）。由 tests/parity/record_init.py 產生，不要手改。", "template": TEMPLATE, "version": VERSION,
           "long": LONG, "counts": {"cases": len(w.cases), "runs": runs}, "base": dict(sorted(w.base.items())), "cases": w.cases}
    sys.stdout.write(json.dumps(doc, ensure_ascii=False, indent=1) + "\n")
    print(f"{len(w.cases)} 個案例、{runs} 次執行", file=sys.stderr)


if __name__ == "__main__":
    main()
