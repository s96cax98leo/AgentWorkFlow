#!/usr/bin/env python3
"""AgentWorkFlow 專案初始化：把 scaffold/ 複製到目標專案，替換 {{佔位符}}，.tmpl 去掉副檔名。

只用 Python 標準庫，macOS、Linux、Windows（PowerShell 或 cmd）都能直接跑：

  python3 init.py --target <專案目錄>（或直接寫 <專案目錄>） [--config <檔>] [--answers <檔>] [--force] [--overwrite-user-files]
                  [--hooks plugin|project] [--with-mcp] [--with-merge-gate] [--defaults] [--python <路徑>]
  （Windows 沒有 python3 指令時用 py -3 init.py … 或 python init.py …）

  --target    要初始化的專案目錄（不存在會建立）
  --config    設定檔（格式見 workflow.config.example）；預設依序找 ./workflow.config、本腳本旁的 workflow.config；
              都沒有時互動詢問（非互動環境或加 --defaults 時直接用預設值）
  --answers   開發規範與慣例的答案檔（JSON，格式見 docs/自訂與擴充.md〈自訂開發規範〉）；給了就不詢問，直接套用
              （選哪個 methodology profile、code style、commit 格式、分支前綴、PR 大小上限、文件格式）
  --force     覆蓋**模版自己的檔案**（.claude/teams/ 底下、.claude/hooks/ 的 awf 腳本、看得出是模版產生的 workflow），
              用來更新已 init 過的專案。注意：.claude/teams/rules.md、methodology.json 等你可能改過的模版檔也會被覆蓋，先用 git 備份。
              使用者的檔案（CLAUDE.md、.claude/settings.json、.mcp.json、不是模版產生的同名 workflow／hook）即使加 --force 也不整份覆蓋：
              CLAUDE.md 只追加一段引用、settings.json 與 .mcp.json 深度合併、其餘保留並列進略過清單
  --overwrite-user-files
              連使用者的檔案也整份覆蓋成模版版本（會蓋掉你自己的 CLAUDE.md、settings.json 等，只在想重設時用）
  --hooks     plugin（預設）：hooks 由 agent-workflow plugin 註冊，settings.json 只放權限
              project：把 plugin/hooks 複製到 <專案>/.claude/hooks/，settings.json 註冊 hooks（這時不要再啟用 plugin 的 hooks）；
              既有的 settings.json 會保留使用者的欄位與 hooks，只把 awf_hook.py 的 hooks 合併進去（重跑不重複）
  --with-mcp  產生 .mcp.json（預設只產生 .mcp.json.example）
  --with-merge-gate
              放 .github/workflows/agent-merge-gate.yml（auto_merge 的 merge gate CI）。預設不放：沒有設定 gate App 時
              它會讓每個 PR 失敗。啟用前先完成 docs/自動合併信任模型.md 的啟用前檢查清單
  --defaults  沒有設定檔時不詢問，全部用預設值（開發規範用 default profile；主 session 的 git／GitHub 限制維持開啟）

套用到既有專案：使用者 .claude/ 下的 agents、skills、commands 一律不動；既有的 CLAUDE.md 不覆蓋，
模版說明寫成 .claude/teams/agent-workflow.CLAUDE.md，再在 CLAUDE.md 結尾追加一段 @ 引用（重跑不重複追加）。

開發規範（methodology）：模版只管工作流程，TDD、spec 驅動、commit 格式、PR 大小等由使用者決定。
互動模式在佔位符問答之後，逐項詢問開發規範 profile 與慣例（每題都有預設值，Enter 直接用）；
結果合併成 <專案>/.claude/teams/methodology.json，再由 build.py 內嵌進 workflow。
最後問一題主 session 的習慣（答案檔的鍵 main_session_push_merge）：你既有的 skill 或習慣會不會在主 session 直接 push main、
合併 PR？答會時，guard.config.json 的 main_session_git_rules、main_session_github_rules 設 false（auto_merge 啟用後主 session 仍不能合併）。
  --python    --hooks project 時寫進 settings.json 的 Python 絕對路徑（預設：跑 init.py 的這個 Python）
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCAFFOLD = HERE / "scaffold"
EXAMPLE = HERE / "workflow.config.example"
PLUGIN_HOOKS = HERE / "plugin" / "hooks"
METHODOLOGIES = SCAFFOLD / ".claude" / "teams" / "methodologies"
PH = re.compile(r"\{\{([A-Z_]+)\}\}")
# init.py 自己算的佔位符（不從 workflow.config 讀），值已是 JSON 片段，原樣代入
COMPUTED = ("AWF_HOOK_CMD", "AWF_HOOK_SHELL")


def die(msg: str) -> None:
    print(f"init.py：{msg}", file=sys.stderr)
    sys.exit(1)


def read_kv(path: Path) -> dict[str, str]:
    """讀 KEY=value（不執行設定檔內容）；值兩端的雙引號去掉。保留檔案裡的順序。"""
    out: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        line = line.rstrip("\r")
        if not re.match(r"^[A-Z_]+=", line):
            continue
        k, _, v = line.partition("=")
        if v.startswith('"'):
            v = v[1:]
        if v.endswith('"'):
            v = v[:-1]
        out[k] = v
    return out


def hint_for(key: str) -> str:
    prev = ""
    for line in EXAMPLE.read_text(encoding="utf-8").splitlines():
        if line.startswith(key + "="):
            return re.sub(r"^#\s*", "", prev)
        prev = line
    return ""


def collect_values(config: Path | None, defaults: bool, target: Path) -> dict[str, str]:
    ex = read_kv(EXAMPLE)
    cf = read_kv(config) if config else {}
    interactive = config is None and not defaults and sys.stdin.isatty()
    if interactive:
        print("沒有 workflow.config，逐一詢問（Enter＝用預設值；輸入 - ＝留空）：")
    vals: dict[str, str] = {}
    for k, default in ex.items():
        val = default
        if config is not None:
            val = cf.get(k, default)
        elif interactive and k != "METHODOLOGY":  # 開發規範 profile 在後面的開發規範問答一起問
            print(f"\n{hint_for(k)}")
            try:
                ans = input(f"{k} [{default}]: ")
            except EOFError:
                ans = ""
            if ans == "-":
                val = ""
            elif ans:
                val = ans
        if k == "WORKSPACE_DIR" and not val:
            val = str(target)
        vals[k] = val
    return vals


# ---------------------------------------------------------------- 開發規範（methodology profile）
def ask(prompt: str, default: str) -> str:
    """互動詢問一題：Enter＝預設值；輸入 - ＝留空。"""
    try:
        ans = input(f"{prompt} [{default}]: ").strip()
    except EOFError:
        ans = ""
    if ans == "-":
        return ""
    return ans or default


def profile_names() -> list[str]:
    return sorted(p.stem for p in METHODOLOGIES.glob("*.json"))


def load_profile(name: str) -> dict:
    """name 是內建 profile 名稱（scaffold/.claude/teams/methodologies/<name>.json）或 JSON 檔路徑。"""
    p = Path(name).expanduser()
    if not (p.suffix == ".json" and p.is_file()):
        p = METHODOLOGIES / f"{name}.json"
    if not p.is_file():
        die(f"找不到開發規範 profile「{name}」（內建：{'、'.join(profile_names())}；也可以給 JSON 檔路徑）")
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        die(f"profile {p} 不是合法 JSON：{e}")
    return {}


def deep_merge(base: dict, extra: dict) -> dict:
    out = dict(base)
    for k, v in extra.items():
        out[k] = deep_merge(out[k], v) if isinstance(v, dict) and isinstance(out.get(k), dict) else v
    return out


def _int_or_none(v, what: str):
    if v is None or v == "" or str(v).strip().lower() in ("0", "none", "null", "不限"):
        return None
    try:
        n = int(v)
    except (TypeError, ValueError):
        die(f"{what} 要是正整數或「不限」（收到 {v!r}）")
    return n if n > 0 else None


def apply_answers(profile: dict, ans: dict) -> dict:
    """把使用者的回答（答案檔或互動）套進 profile。只改回答到的項目，其餘沿用 profile。"""
    prof = json.loads(json.dumps(profile))
    conv = prof.setdefault("conventions", {})
    if "code_style" in ans:
        cs = ans["code_style"]
        conv["code_style"] = deep_merge(conv.get("code_style", {}), cs if isinstance(cs, dict) else {"description": str(cs), "command": None, "tool": None})
    if "commit" in ans:
        c = ans["commit"]
        if isinstance(c, dict):
            conv["commit"] = deep_merge(conv.get("commit", {}), c)
        else:
            # 只改格式說明時，profile 的範例與檢查 regex 可能跟新格式不符，一併清掉
            conv["commit"] = {"format": str(c), "examples": {}, "check_regex": None}
    if "commit_regex" in ans:
        conv.setdefault("commit", {})["check_regex"] = ans["commit_regex"] or None
    if "branch_prefix" in ans and ans["branch_prefix"]:
        conv.setdefault("branch", {})["prefix"] = str(ans["branch_prefix"])
    if "pr_size" in ans:
        ps = ans["pr_size"] or {}
        conv["pr_size"] = deep_merge(conv.get("pr_size", {}), {
            "max_lines": _int_or_none(ps.get("max_lines"), "pr_size.max_lines"),
            "max_files": _int_or_none(ps.get("max_files"), "pr_size.max_files")})
    if "doc_format" in ans:
        d = ans["doc_format"]
        conv["doc_format"] = deep_merge(conv.get("doc_format", {}), d if isinstance(d, dict) else {"format": str(d)})
    if "test_first" in ans:
        prof.setdefault("stages", {}).setdefault("build", {})["test_first"] = bool(ans["test_first"])
    if isinstance(ans.get("requirements"), dict):
        prof["requirements"] = ans["requirements"]
    if isinstance(ans.get("overrides"), dict):
        prof = deep_merge(prof, ans["overrides"])
    return prof


def ask_methodology(default_name: str, branch_prefix: str) -> tuple[str, dict]:
    """互動模式：逐項確認使用者的開發規範與慣例習慣，回傳 (profile 名稱, 回答)。"""
    print("\n== 開發規範與慣例（模版只管工作流程，做法由你決定；Enter＝用預設值） ==")
    for n in profile_names():
        print(f"  - {n}：{load_profile(n).get('description', '')}")
    name = ask("開發規範 profile（上面的名稱，或自己的 profile JSON 路徑）", default_name or "default")
    prof = load_profile(name)
    conv = prof.get("conventions", {})
    ans: dict = {}
    build = prof.get("stages", {}).get("build", {})
    tf = ask("build 段要不要先寫會失敗的測試再實作（y／n）", "y" if build.get("test_first", True) else "n")
    if tf.lower().startswith("y") != bool(build.get("test_first", True)):
        ans["test_first"] = tf.lower().startswith("y")
    cs = conv.get("code_style", {})
    cmd = ask("code style 檢查指令或工具（例如 ruff format --check .；- ＝沒有，照 repo 既有設定）", cs.get("command") or "-")
    if cmd and cmd != "-" and cmd != cs.get("command"):
        ans["code_style"] = {"command": cmd, "tool": cmd.split()[0], "description": f"提交前跑 {cmd}，不得新增違規。"}
    cm = conv.get("commit", {})
    fmt = ask("commit 格式說明", cm.get("format", ""))
    if fmt != cm.get("format", ""):
        ans["commit"] = fmt
    rx = ask("commit 標題檢查 regex（- ＝不檢查）", cm.get("check_regex") or "-")
    rx = "" if rx == "-" else rx
    if rx != (cm.get("check_regex") or "") or "commit" in ans:
        ans["commit_regex"] = rx or None
    bp = ask("分支前綴", branch_prefix or conv.get("branch", {}).get("prefix", "feature/"))
    ans["branch_prefix"] = bp
    ps = conv.get("pr_size", {})
    ml = ask("PR 大小上限：行數（0 ＝不限）", str(ps.get("max_lines") or 0))
    mf = ask("PR 大小上限：檔案數（0 ＝不限）", str(ps.get("max_files") or 0))
    ans["pr_size"] = {"max_lines": ml, "max_files": mf}
    df = ask("文件格式", conv.get("doc_format", {}).get("format", "Markdown"))
    if df != conv.get("doc_format", {}).get("format"):
        ans["doc_format"] = df
    return name, ans


def ask_main_session(answers_file: Path | None, interactive: bool) -> bool:
    """主 session 會不會直接 push main、合併 PR：答案檔的 main_session_push_merge > 互動問答 > 預設（不會）。"""
    if answers_file:
        try:
            ans = json.loads(answers_file.read_text(encoding="utf-8-sig"))
        except (OSError, json.JSONDecodeError):
            ans = {}
        v = ans.get("main_session_push_merge") if isinstance(ans, dict) else None
        if isinstance(v, str):
            return v.strip().lower() in ("y", "yes", "true", "1", "會")
        return v is True
    if not interactive:
        return False
    print("\n== 主 session 的習慣 ==")
    a = ask("你既有的 skill 或習慣會不會在主 session 直接 push main、合併 PR（y／n）", "n")
    return a.strip().lower().startswith("y") or a.strip() == "會"


def build_methodology(vals: dict[str, str], answers_file: Path | None, interactive: bool, defaults: bool) -> dict:
    """決定專案的開發規範 profile：答案檔 > 互動問答 > workflow.config 的 METHODOLOGY（--defaults 用 default）。"""
    name = "default" if defaults and not answers_file else (vals.get("METHODOLOGY") or "default")
    ans: dict = {}
    if answers_file:
        try:
            ans = json.loads(answers_file.read_text(encoding="utf-8-sig"))
        except (OSError, json.JSONDecodeError) as e:
            die(f"讀不到答案檔 {answers_file}：{e}")
        if not isinstance(ans, dict):
            die(f"答案檔 {answers_file} 要是 JSON 物件")
        name = str(ans.get("methodology") or name)
    elif interactive:
        name, ans = ask_methodology(name, vals.get("BRANCH_PREFIX", ""))
    prof = apply_answers(load_profile(name), ans)
    # 分支前綴：回答了就改 BRANCH_PREFIX（workflow 的分支名用它）；沒回答就讓 profile 跟 BRANCH_PREFIX 一致
    branch = prof.setdefault("conventions", {}).setdefault("branch", {})
    if ans.get("branch_prefix"):
        vals["BRANCH_PREFIX"] = branch["prefix"]
        allowed = [x.strip() for x in vals.get("ALLOWED_BRANCH_PREFIXES", "").split(",") if x.strip()]
        if branch["prefix"] not in allowed:
            vals["ALLOWED_BRANCH_PREFIXES"] = ",".join([branch["prefix"], *allowed])
    elif vals.get("BRANCH_PREFIX"):
        branch["prefix"] = vals["BRANCH_PREFIX"]
    vals["METHODOLOGY"] = str(prof.get("name") or name)
    sys.path.insert(0, str(SCAFFOLD / ".claude" / "teams" / "tools"))
    import schemalib  # noqa: E402  只用標準庫
    errs = schemalib.check_doc(prof, "methodology")
    if errs:
        die("開發規範 profile 不符合 schemas/methodology.schema.json：" + "；".join(errs[:5]))
    return prof


# ---------------------------------------------------------------- hooks 的啟動指令
def python_for_hooks(override: str | None) -> str:
    """寫進 settings.json 的 Python：預設是跑 init.py 的這個；在 venv 裡時改用它底下的基礎 Python（venv 可能被刪）。"""
    if override:
        p = Path(override).expanduser()
        if not p.is_file():
            die(f"--python 指定的檔案不存在：{override}")
        return str(p.resolve())
    exe = sys.executable
    base = getattr(sys, "_base_executable", None)
    if sys.prefix != sys.base_prefix and base and Path(base).is_file():
        exe = base
    return str(Path(exe).resolve())


def hook_command(python: str) -> tuple[str, str]:
    """回傳 (AWF_HOOK_CMD, AWF_HOOK_SHELL)，都是已經 JSON 跳脫好的片段。

    - macOS／Linux：shell form 交給 sh -c，Python 用絕對路徑，不靠 PATH 上的 python3。
    - Windows：明確指定 "shell": "powershell"（PowerShell 一定在），用呼叫運算子 & 執行絕對路徑的 python.exe。
      不用 exec form（args）是因為較舊的 Claude Code 不認得 args，會只跑 command 而讓守門失效。
    ${CLAUDE_PROJECT_DIR} 一律包在雙引號裡：PowerShell 下 Claude Code 會改寫成 ${env:CLAUDE_PROJECT_DIR}。
    出處見 docs/踩坑手冊.md（Windows 與 PowerShell 一節）。
    """
    script = '"${CLAUDE_PROJECT_DIR}/.claude/hooks/awf_hook.py"'
    if os.name == "nt":
        cmd, shell = f'& "{python}" {script}', ', "shell": "powershell"'
    else:
        cmd, shell = f'"{python}" {script}', ""
    return json.dumps(cmd, ensure_ascii=False)[1:-1], shell


def python3_works() -> bool:
    exe = shutil.which("python3")
    if not exe:
        return False
    try:
        r = subprocess.run([exe, "-c", "print('ok')"], capture_output=True, text=True, timeout=20)
    except (OSError, subprocess.SubprocessError):
        return False
    return r.returncode == 0 and r.stdout.strip() == "ok"


# ---------------------------------------------------------------- 複製與替換
CLAUDE_IMPORT = ".claude/teams/agent-workflow.CLAUDE.md"
CLAUDE_MARK = "<!-- agent-workflow：由 init.py 從模版產生"  # 模版 CLAUDE.md 第一行的標記，用來認出 init 自己產生的 CLAUDE.md
# 使用者的檔案：已存在時不整份覆蓋（--force 也一樣），改走合併或追加；只有 --overwrite-user-files 才整份覆蓋
MERGE_JSON = (".claude/settings.json", ".mcp.json")
# 和使用者共用的目錄：同名檔案要看得出是模版產生的，--force 才覆蓋（否則當成使用者的檔案保留）
SHARED_DIRS = (".claude/workflows/", ".claude/hooks/", ".github/workflows/")
OURS = re.compile(r"agent-workflow|agent-merge-gate|/\*@schema:")


def merge_settings(user: dict, tmpl: dict) -> dict:
    """深度合併：使用者已有的值一律保留（純量、清單都不改），只補上模版有、使用者沒有的鍵；
    hooks 依事件合併：awf_hook.py 的同一模式（guard-shell、guard-paths、envelope）在同一 matcher 只註冊一次，
    已註冊過就更新成這次的指令（例如換了 Python 路徑），使用者自己的 hooks 原樣保留。"""
    out = dict(user)
    for k, v in tmpl.items():
        if k == "hooks" and isinstance(v, dict):
            out[k] = merge_hooks(user.get(k) if isinstance(user.get(k), dict) else {}, v)
        elif k not in out:
            out[k] = v
        elif isinstance(v, dict) and isinstance(out[k], dict):
            out[k] = merge_settings(out[k], v)
    return out


def _awf_mode(h: dict) -> str | None:
    m = re.search(r"awf_hook\.py\W*\s+(guard-shell|guard-paths|envelope)\b", str(h.get("command", "")))
    return m.group(1) if m else None


def merge_hooks(user: dict, tmpl: dict) -> dict:
    out = json.loads(json.dumps(user))
    for event, entries in tmpl.items():
        cur = out.setdefault(event, [])
        for entry in entries:
            for th in entry.get("hooks", []):
                mode = _awf_mode(th)
                hit = None
                for ue in cur:
                    if ue.get("matcher") != entry.get("matcher"):
                        continue
                    for i, uh in enumerate(ue.get("hooks", [])):
                        if mode and _awf_mode(uh) == mode:
                            hit = (ue, i)
                if hit:
                    hit[0]["hooks"][hit[1]] = th
                else:
                    cur.append({**{k: x for k, x in entry.items() if k != "hooks"}, "hooks": [th]})
    return out


class Writer:
    def __init__(self, target: Path, vals: dict[str, str], force: bool, overwrite_user: bool = False):
        self.target, self.vals, self.force, self.overwrite_user = target, vals, force, overwrite_user
        self.written: list[Path] = []
        self.skipped: list[Path] = []       # 模版檔已存在（加 --force 更新）
        self.user_kept: list[Path] = []     # 使用者的檔案，保留不動
        self.merged: list[Path] = []        # 使用者的檔案，合併或追加了模版的內容
        self.leftovers: list[str] = []

    def rel(self, p: Path) -> str:
        return p.relative_to(self.target).as_posix()

    def render_file(self, src: Path, dst: Path) -> str:
        text = src.read_text(encoding="utf-8")
        is_json = dst.suffix == ".json" or dst.name.endswith(".json.example")
        text = self.render(text, is_json)
        for m in PH.finditer(text):
            self.leftovers.append(f"{self.rel(dst)}：{m.group(0)}")
        return text

    @staticmethod
    def write_text(dst: Path, text: str) -> None:
        dst.parent.mkdir(parents=True, exist_ok=True)
        # newline="" 保留原本的換行字元，Windows 上也不會改成 CRLF
        with open(dst, "w", encoding="utf-8", newline="") as f:
            f.write(text)

    def put_user(self, src: Path, dst: Path) -> bool:
        """使用者可能已經有的檔案。已處理回 True；回 False 時交給一般流程（目標不存在或 --overwrite-user-files）。"""
        if not dst.exists() or self.overwrite_user:
            return False
        rel = self.rel(dst)
        if rel == "CLAUDE.md":
            self.put_claude_md(src, dst)
            return True
        if rel in MERGE_JSON:
            text = self.render_file(src, dst)
            try:
                user = json.loads(dst.read_text(encoding="utf-8-sig"))
                tmpl = json.loads(text)
            except (OSError, json.JSONDecodeError):
                self.user_kept.append(dst)
                print(f"注意：{rel} 不是合法 JSON，沒有合併；請照 {src.name} 手動補上模版的設定")
                return True
            if not isinstance(user, dict):
                self.user_kept.append(dst)
                return True
            merged = merge_settings(user, tmpl)
            if merged != user:
                self.write_text(dst, json.dumps(merged, ensure_ascii=False, indent=2) + "\n")
                self.merged.append(dst)
            else:
                self.user_kept.append(dst)
            return True
        if rel.startswith(SHARED_DIRS):
            try:
                ours = OURS.search(dst.read_text(encoding="utf-8")) is not None
            except (OSError, UnicodeDecodeError):
                ours = False
            if not ours:
                self.user_kept.append(dst)
                return True
        return False

    def put_claude_md(self, src: Path, dst: Path) -> None:
        """既有 CLAUDE.md：init 自己產生的照一般規則（--force 才更新）；使用者的不覆蓋，
        模版內容寫成 .claude/teams/agent-workflow.CLAUDE.md，再在結尾追加一段 @ 引用（已引用過就不再追加）。"""
        with open(dst, encoding="utf-8", newline="") as f:
            body = f.read()
        if CLAUDE_MARK in body:  # 先前 init 在空專案產生的 CLAUDE.md
            if self.force:
                self.write_text(dst, self.render_file(src, dst))
                self.written.append(dst)
            else:
                self.skipped.append(dst)
            return
        self.put(src, self.target / CLAUDE_IMPORT)
        if "@" + CLAUDE_IMPORT in body:
            self.user_kept.append(dst)
            return
        nl = "\r\n" if "\r\n" in body else "\n"
        block = (("" if body.endswith(("\n", "\r")) or not body else nl) + nl
                 + "<!-- agent-workflow：模版的工作流程說明（init.py 追加；移除模版時刪掉這兩行） -->" + nl
                 + "@" + CLAUDE_IMPORT + nl)
        with open(dst, "a", encoding="utf-8", newline="") as f:
            f.write(block)
        self.merged.append(dst)

    def render(self, text: str, as_json: bool) -> str:
        def sub(m: re.Match) -> str:
            k = m.group(1)
            if k not in self.vals:
                return m.group(0)
            if k in COMPUTED:
                return self.vals[k]
            return json.dumps(self.vals[k], ensure_ascii=False)[1:-1] if as_json else self.vals[k]

        return PH.sub(sub, text)

    def put(self, src: Path, dst: Path, text_render: bool = True) -> None:
        if self.put_user(src, dst):
            return
        if dst.exists() and not (self.force or self.overwrite_user):
            self.skipped.append(dst)
            return
        dst.parent.mkdir(parents=True, exist_ok=True)
        try:
            text = self.render_file(src, dst) if text_render else src.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            shutil.copy2(src, dst)
            self.written.append(dst)
            return
        self.write_text(dst, text)
        shutil.copymode(src, dst)
        self.written.append(dst)


def dest_for(rel: Path, target: Path, runs: str, hooks: str, with_mcp: bool, with_merge_gate: bool = False) -> Path | None:
    parts = rel.parts
    name = rel.name
    if parts[0] == "_runs":
        rel = Path(runs, *parts[1:])
    posix = rel.as_posix()
    if posix == ".github/workflows/agent-merge-gate.yml.tmpl" and not with_merge_gate:
        return None  # 沒有 gate App 時這個 CI 會讓每個 PR 失敗，預設不放
    if posix == ".claude/settings.json.tmpl" and hooks != "plugin":
        return None
    if posix == ".claude/settings.project-hooks.json.tmpl":
        if hooks != "project":
            return None
        rel = rel.with_name("settings.json.tmpl")
    if posix == ".mcp.json.tmpl" and not with_mcp:
        rel = rel.with_name(".mcp.json.example.tmpl")
    if name.endswith(".tmpl"):
        rel = rel.with_name(rel.name[: -len(".tmpl")])
    return target / rel


def skip_file(p: Path) -> bool:
    return "__pycache__" in p.parts or p.name == ".DS_Store"


def main() -> None:
    ap = argparse.ArgumentParser(prog="init.py", description="AgentWorkFlow 專案初始化", add_help=True)
    ap.add_argument("positional", nargs="?", help="目標專案目錄（等同 --target）")
    ap.add_argument("--target")
    ap.add_argument("--config")
    ap.add_argument("--answers", help="開發規範與慣例的答案檔（JSON）")
    ap.add_argument("--force", action="store_true", help="覆蓋模版自己的檔案；使用者的檔案仍走合併")
    ap.add_argument("--overwrite-user-files", action="store_true", help="連 CLAUDE.md、settings.json 等使用者檔案也整份覆蓋")
    ap.add_argument("--hooks", default="plugin")
    ap.add_argument("--with-mcp", action="store_true")
    ap.add_argument("--with-merge-gate", action="store_true", help="放 .github/workflows/agent-merge-gate.yml（預設不放）")
    ap.add_argument("--defaults", action="store_true")
    ap.add_argument("--python")
    a = ap.parse_args()

    if a.target and a.positional:
        die(f"多給了一個目錄：{a.positional}")
    target_arg = a.target or a.positional
    if not target_arg:
        print(__doc__, file=sys.stderr)
        die("缺 --target")
    if a.hooks not in ("plugin", "project"):
        die(f"--hooks 只能是 plugin 或 project（收到 {a.hooks}）")
    if not SCAFFOLD.is_dir():
        die(f"找不到 {SCAFFOLD}")
    if not EXAMPLE.is_file():
        die(f"找不到 {EXAMPLE}")

    config: Path | None = None
    if a.config:
        config = Path(a.config).expanduser()
        if not config.is_file():
            die(f"找不到設定檔 {a.config}")
    else:
        for c in (Path.cwd() / "workflow.config", HERE / "workflow.config"):
            if c.is_file():
                config = c
                break

    answers: Path | None = None
    if a.answers:
        answers = Path(a.answers).expanduser()
        if not answers.is_file():
            die(f"找不到答案檔 {a.answers}")

    target = Path(target_arg).expanduser()
    target.mkdir(parents=True, exist_ok=True)
    target = target.resolve()

    vals = collect_values(config, a.defaults, target)
    interactive = config is None and not a.defaults and sys.stdin.isatty()
    methodology = build_methodology(vals, answers, interactive, a.defaults)
    main_free = ask_main_session(answers, interactive)
    python = python_for_hooks(a.python)
    vals["AWF_HOOK_CMD"], vals["AWF_HOOK_SHELL"] = hook_command(python)
    runs = (vals.get("RUNS_DIR") or "_tpm/runs").strip("/")
    briefs = (vals.get("BRIEFS_DIR") or "").strip("/")

    w = Writer(target, vals, a.force, a.overwrite_user_files)
    for src in sorted(p for p in SCAFFOLD.rglob("*") if p.is_file()):
        if skip_file(src):
            continue
        dst = dest_for(src.relative_to(SCAFFOLD), target, runs, a.hooks, a.with_mcp, a.with_merge_gate)
        if dst is not None:
            w.put(src, dst)

    if a.hooks == "project":
        for src in sorted(p for p in PLUGIN_HOOKS.rglob("*") if p.is_file()):
            if skip_file(src) or src.name == "hooks.json":
                continue
            w.put(src, target / ".claude" / "hooks" / src.relative_to(PLUGIN_HOOKS), text_render=False)

    if briefs:
        (target / briefs).mkdir(parents=True, exist_ok=True)

    # 主 session 的習慣：會直接 push main、合併 PR 時放寬主 session 的 git／GitHub 限制（只改這次寫出的 guard.config.json）
    gcfg = target / ".claude" / "teams" / "guard.config.json"
    if main_free:
        if gcfg in w.written:
            t = gcfg.read_text(encoding="utf-8")
            t = re.sub(r'("main_session_(?:git|github)_rules"\s*:\s*)true', r"\1false", t)
            Writer.write_text(gcfg, t)
            print("主 session 的 git／GitHub 限制已放寬（main_session_git_rules、main_session_github_rules = false）。"
                  "注意：之後啟用 auto_merge 時，主 session 仍不能合併、核准 PR（信任模型要求）。")
        else:
            print(f"注意：{gcfg.relative_to(target).as_posix()} 已存在、沒有改；要放寬主 session 的限制，"
                  "自己把 main_session_git_rules、main_session_github_rules 改成 false。")

    # 開發規範 profile：寫成專案的 .claude/teams/methodology.json（已存在且沒加 --force 時保留專案自己的）
    meth = target / ".claude" / "teams" / "methodology.json"
    if meth.exists() and not a.force:
        w.skipped.append(meth)
    else:
        meth.parent.mkdir(parents=True, exist_ok=True)
        meth.write_text(json.dumps(methodology, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        w.written.append(meth)
    print(f"開發規範 profile：{methodology.get('name')}（{meth.relative_to(target).as_posix()}"
          f"{'，已存在、保留原檔' if meth in w.skipped else ''}）")

    print(f"寫入 {len(w.written)} 個檔案到 {target}")
    if w.merged:
        print(f"合併進 {len(w.merged)} 個既有的使用者檔案（原內容保留）：")
        for p in w.merged:
            how = "結尾追加一段 @ 引用" if p.name == "CLAUDE.md" else "深度合併，使用者的值優先"
            print(f"  - {p.relative_to(target).as_posix()}（{how}）")
    if w.user_kept:
        print(f"保留 {len(w.user_kept)} 個使用者的檔案，沒有改動（--force 也不覆蓋；要整份換成模版版本加 --overwrite-user-files）：")
        for p in w.user_kept:
            print(f"  - {p.relative_to(target).as_posix()}")
    if w.skipped:
        print(f"略過 {len(w.skipped)} 個已存在的模版檔（加 --force 更新）：")
        for p in w.skipped:
            print(f"  - {p.relative_to(target).as_posix()}")
    st_path = target / ".claude" / "settings.json"
    if st_path.is_file():
        try:
            has_awf = "awf_hook.py" in json.dumps(json.loads(st_path.read_text(encoding="utf-8-sig")).get("hooks", {}))
        except (OSError, json.JSONDecodeError, AttributeError):
            has_awf = False
        if a.hooks == "plugin" and has_awf:
            print("注意：.claude/settings.json 裡有 awf_hook.py 的 hooks（先前用 --hooks project init 過），但這次是 --hooks plugin。"
                  "plugin 也會註冊同一組 hooks，請擇一：刪掉 settings.json 裡的 awf_hook.py hooks，或改回 --hooks project。")
        if a.hooks == "project" and not has_awf:
            print("注意：.claude/settings.json 沒有 awf_hook.py 的 hooks（可能不是合法 JSON），請照 .claude/settings.README.md 手動註冊。")
    if w.leftovers:
        print("警告：還有沒替換的佔位符（workflow.config 缺這些值）：")
        for x in sorted(set(w.leftovers)):
            print(f"  - {x}")

    # ------------------------------------------------------------ 內嵌 schema 與自我檢查
    build = target / ".claude" / "teams" / "tools" / "build.py"
    if build.is_file():
        r = subprocess.run([sys.executable, str(build)], cwd=target)
        if r.returncode:
            print("警告：build.py 失敗，請手動跑 python3 .claude/teams/tools/build.py")

    if a.hooks == "plugin" and os.name == "nt" and not python3_works():
        print("\n警告：這台 Windows 的 python3 指令跑不起來（常見是 Microsoft Store 的空殼或只裝了 py 啟動器）。"
              "plugin 的 hooks 用 python3 啟動，跑不起來時守門會擋下所有 shell 與改檔動作。"
              "改用 --hooks project 重跑（init.py 會寫入 Python 絕對路徑），或安裝能用 python3 執行的 Python。")

    print("\n下一步：")
    if a.hooks == "plugin":
        print(f"""  1. 在 Claude Code 安裝 plugin（hooks、agents、skills、commands 都由 plugin 提供）：
       /plugin marketplace add {HERE}
       /plugin install agent-workflow@agentworkflow
  2. 確認 hooks 只註冊一次：{target / '.claude' / 'settings.json'} 不應有 "hooks"；在 Claude Code 跑 /hooks，awf_hook.py guard-shell 只出現一次（來源是 plugin）。""")
    else:
        print(f"""  1. hooks 已複製到 {target / '.claude' / 'hooks'}，並註冊在 .claude/settings.json（Python：{python}）。
     若也要用 plugin 的 agents／skills／commands：安裝 plugin 後，到 /plugin 停用它的 hooks，或改回 --hooks plugin 重新 init（加 --force）。
  2. 確認 hooks 只註冊一次：在 Claude Code 跑 /hooks，awf_hook.py guard-shell 只出現一次（來源是專案 settings）。""")
    claude_doc = CLAUDE_IMPORT if (target / CLAUDE_IMPORT).exists() else "CLAUDE.md"
    print(f"""  3. 填寫 {target / '.claude' / 'teams' / 'rules.md'} 的「## 專案規則」段與 {claude_doc} 的「專案特定」段。
  4. 檢查 {target / '.claude' / 'teams' / 'guard.config.json'}（上游、fork、禁止路徑、分支前綴）與
     {target / '.claude' / 'teams' / 'methodology.json'}（開發規範與慣例；改了要重跑 python3 .claude/teams/tools/build.py）。
  5. 驗證（在 {target} 執行；Windows 把 python3 換成 py -3 或 python）：
       uv run --no-project --with jsonschema python3 .claude/teams/golden-set/run.py
       node .claude/teams/tools/check_workflows.mjs --agents-dir {HERE / 'plugin' / 'agents'}
       python3 .claude/teams/tools/check_agents.py --plugin-dir {HERE / 'plugin'}
       python3 {HERE / 'plugin' / 'hooks' / 'tests' / 'run.py'} --project {target}""")
    gate = target / ".github" / "workflows" / "agent-merge-gate.yml"
    if gate.exists():
        print(f"""  6. merge gate CI 已放在 {gate}。在 GitHub 設好 gate App、ruleset 綁定來源之前，它會讓 PR 的 check 失敗；
     啟用前檢查清單見 {HERE / 'docs' / '自動合併信任模型.md'}。""")
    else:
        print(f"""  6. merge gate CI（auto_merge 用）預設沒有放。要啟用自動合併時，先完成 {HERE / 'docs' / '自動合併信任模型.md'}
     的啟用前檢查清單，再加 --with-merge-gate 重跑 init。""")
    print(f"""  套用到既有專案時保留了什麼、怎麼移除模版：見 {HERE / 'docs' / '自訂與擴充.md'}〈套用到既有專案〉。""")


if __name__ == "__main__":
    main()
