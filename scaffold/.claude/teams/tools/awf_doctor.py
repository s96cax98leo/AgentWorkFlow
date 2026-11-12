#!/usr/bin/env python3
"""doctor：檢查這個專案的 agent-workflow 裝得對不對（awf.py doctor 會轉到這裡）。

用法（在已 init 的專案根目錄）：
    python3 .claude/teams/tools/awf.py doctor [--json] [--plugin-dir <plugin 目錄>]
    python3 .claude/teams/tools/awf_doctor.py --selftest

逐項檢查，每一項附修法：
    環境      python 版本、git、gh、node
    plugin    找不找得到、版本、和專案 .claude/teams/VERSION 一不一致
    安裝來源  同一個 plugin 從不只一個 marketplace 安裝、同一來源裝了不同版本、marketplace 已更新但安裝的還是舊版、安裝目錄不見了
    hook      plugin（與專案 .claude/hooks/）的每個 .py 都編譯得過；plugin 與專案 settings 有沒有重複註冊
    設定      methodology.json（schema＋語意規則）、guard.config.json、workflow.config、擴充 agent 的登記
    覆寫檔    .claude/workflows/ 的覆寫檔依哪一版範本、有沒有沒標記的舊副本

只讀、不改任何檔案、不連網。結束碼：有「問題」是 1，其餘 0（「注意」不算失敗）。
Claude Code 的 installed_plugins.json、known_marketplaces.json 格式沒有承諾穩定：讀不懂就略過那一項並說明。只用標準庫。
"""
from __future__ import annotations

import json
import os
import re
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[2]
sys.path.insert(0, str(HERE))

PLUGIN_NAME = "agent-workflow"
MIN_PY = (3, 9)
OK, WARN, ERR, INFO = "ok", "warn", "error", "info"
MARK = {OK: "通過", WARN: "注意", ERR: "問題", INFO: "說明"}
LB = "{" * 2    # 佔位符的開頭；不直接寫出來，init.py 會把工具原始碼裡的佔位符換掉
UPDATE_FIX = "claude plugin update {key}，再在 Claude Code 裡 /reload-plugins（只跑 marketplace update 不會更新 plugin）"


def load_json(p: Path):
    try:
        return json.loads(p.read_text(encoding="utf-8-sig"))
    except (OSError, ValueError):
        return None


def vtuple(v: str) -> tuple:
    """版本比較用：只看開頭的數字段（0.4.1、0.3.0-x.1 → (0,4,1)、(0,3,0)）。"""
    m = re.match(r"\s*v?(\d+(?:\.\d+)*)", str(v or ""))
    return tuple(int(x) for x in m.group(1).split(".")) if m else ()


def item(cid: str, level: str, msg: str, fix: str = "") -> dict:
    return {"id": cid, "level": level, "msg": msg, "fix": fix}


def config_home(home: Path | None = None) -> Path:
    return home or Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude")


# ------------------------------------------------------------------ 環境
def check_env(which=shutil.which, py: tuple = tuple(sys.version_info[:3])) -> list[dict]:
    out = []
    pv = ".".join(str(x) for x in py)
    out.append(item("python", OK, f"Python {pv}") if py >= MIN_PY else
               item("python", ERR, f"Python {pv} 太舊（工具要 {MIN_PY[0]}.{MIN_PY[1]} 以上）", "安裝新版 Python，並確認 python3（Windows 是 python）指到它"))
    out.append(item("git", OK, "找得到 git") if which("git") else item("git", ERR, "找不到 git", "安裝 git 並確認在 PATH 裡"))
    out.append(item("gh", OK, "找得到 gh") if which("gh") else
               item("gh", WARN, "找不到 gh（GitHub CLI）：推送後開 PR、合併、讀 CI 狀態都需要它", "安裝 GitHub CLI 並執行 gh auth login；只做到 deliver 以前可以先不裝"))
    out.append(item("node", OK, "找得到 node") if which("node") else
               item("node", WARN, "找不到 node：只有乾跑驗證 workflow 腳本（check_workflows.mjs）需要它，平常執行不需要", "要改 workflow 或覆寫檔時再安裝 Node.js"))
    return out


# ------------------------------------------------------------------ 安裝來源
def installs(home: Path) -> tuple[list[dict], str]:
    """（agent-workflow 的所有安裝紀錄, 讀不懂時的說明）。"""
    f = home / "plugins" / "installed_plugins.json"
    if not f.is_file():
        return [], ""
    d = load_json(f)
    if not isinstance(d, dict) or not isinstance(d.get("plugins"), dict):
        return [], f"{f.name} 讀不懂（Claude Code 的格式可能變了），略過安裝來源的檢查"
    out = []
    for key, entries in d["plugins"].items():
        if str(key).split("@")[0] != PLUGIN_NAME:
            continue
        for e in entries if isinstance(entries, list) else [entries]:
            if isinstance(e, dict):
                out.append({"key": str(key), "market": str(key).partition("@")[2], "scope": str(e.get("scope") or ""), "version": str(e.get("version") or ""),
                            "path": str(e.get("installPath") or ""), "project": str(e.get("projectPath") or "")})
    return out, ""


def market_version(home: Path, market: str) -> str:
    """marketplace 目前提供的版本（本機的 marketplace 目錄；讀不到回空）。"""
    d = load_json(home / "plugins" / "known_marketplaces.json")
    loc = (d.get(market) or {}).get("installLocation") if isinstance(d, dict) and isinstance(d.get(market), dict) else None
    for base in ([Path(str(loc))] if loc else []) + [home / "plugins" / "marketplaces" / market]:
        m = load_json(base / ".claude-plugin" / "marketplace.json")
        for p in (m.get("plugins") or []) if isinstance(m, dict) else []:
            if isinstance(p, dict) and p.get("name") == PLUGIN_NAME:
                if p.get("version"):
                    return str(p["version"])
                src = p.get("source")
                pj = load_json(base / str(src) / ".claude-plugin" / "plugin.json") if isinstance(src, str) else None
                if isinstance(pj, dict) and pj.get("version"):
                    return str(pj["version"])
    return ""


def check_installs(project: Path, home: Path) -> list[dict]:
    recs, note = installs(home)
    if note:
        return [item("install", INFO, note)]
    mine = [r for r in recs if not r["project"] or Path(r["project"]).resolve() == project.resolve()]   # 別的專案的安裝不影響這裡
    if not mine:
        return [item("install", INFO, "Claude Code 沒有登記這個專案可用的 agent-workflow 安裝（用 --plugin-dir、環境變數或在模版 repo 裡執行時不需要）",
                     "要安裝：claude plugin marketplace add <模版 repo>，再 claude plugin install agent-workflow@agentworkflow")]
    out = []
    keys = sorted({r["key"] for r in mine})
    if len(keys) > 1:
        out.append(item("install-sources", WARN, "同一個 plugin 從不只一個來源安裝：" + "、".join(keys) + "。Claude Code 會同時載入，agent、hook、skill 都會重複，版本不同時行為不可預期",
                        "留一個就好：claude plugin uninstall <不要的那個>，再 /reload-plugins"))
    for k in keys:
        vs = sorted({r["version"] for r in mine if r["key"] == k and r["version"]})
        if len(vs) > 1:
            out.append(item("install-versions", WARN, f"{k} 在不同範圍裝了不同版本：" + "、".join(vs), UPDATE_FIX.format(key=k) + "；或移除較舊的那個範圍的安裝"))
    for r in mine:
        if r["path"] and not Path(r["path"]).is_dir():
            out.append(item("install-path", ERR, f"{r['key']}（{r['scope'] or '?'}）登記的安裝目錄不存在", f"重新安裝：claude plugin uninstall {r['key']}，再 claude plugin install {r['key']}"))
        mv = market_version(home, r["market"])
        if mv and r["version"] and vtuple(mv) > vtuple(r["version"]):
            out.append(item("install-stale", WARN, f"{r['key']} 安裝的是 {r['version']}，marketplace 已經是 {mv}", UPDATE_FIX.format(key=r["key"])))
    return out or [item("install", OK, "安裝來源沒有衝突：" + "、".join(f"{r['key']} {r['version']}" for r in mine))]


# ------------------------------------------------------------------ plugin 與版本
def check_plugin(project: Path, plugin: Path | None, notes: list[str]) -> list[dict]:
    if plugin is None:
        return [item("plugin", ERR, "找不到 agent-workflow plugin（awf.py next 沒辦法產生 workflow 腳本）",
                     "安裝 plugin（claude plugin install agent-workflow@agentworkflow），或用 --plugin-dir／環境變數 AWF_PLUGIN_DIR 指定 plugin 目錄")]
    pj = load_json(plugin / ".claude-plugin" / "plugin.json")
    pv = str(pj.get("version") or "") if isinstance(pj, dict) else ""
    out = [item("plugin", OK if pv else WARN, f"plugin 版本 {pv or '讀不到'}")] + [item("plugin-note", INFO, n) for n in notes]
    vf = project / ".claude" / "teams" / "VERSION"
    tv = vf.read_text(encoding="utf-8").strip() if vf.is_file() else ""
    if not tv:
        out.append(item("version", WARN, "專案沒有 .claude/teams/VERSION（0.2.0 以前套用的，或檔案被刪了）", "跑模版 repo 的 init.py --target <這個專案> --upgrade"))
    elif pv and vtuple(tv) < vtuple(pv):
        out.append(item("version", WARN, f"專案的工具是 {tv}，plugin 是 {pv}：新版 workflow 的回傳，舊的閘門工具可能不認得",
                        "跑模版 repo 的 init.py --target <這個專案> --upgrade（已套用的專案不會自動更新）"))
    elif pv and vtuple(tv) > vtuple(pv):
        out.append(item("version", WARN, f"專案的工具是 {tv}，plugin 還是 {pv}", UPDATE_FIX.format(key="agent-workflow@agentworkflow")))
    else:
        out.append(item("version", OK, f"專案的工具與 plugin 版本一致（{tv}）"))
    return out


# ------------------------------------------------------------------ hook
def compile_errors(d: Path) -> list[str]:
    errs = []
    for f in sorted(d.rglob("*.py")):
        if "__pycache__" in f.parts:
            continue
        try:
            compile(f.read_text(encoding="utf-8"), str(f), "exec")
        except (SyntaxError, ValueError, OSError) as e:
            errs.append(f"{f.relative_to(d).as_posix()}：{type(e).__name__} {getattr(e, 'msg', e)}" + (f"（第 {e.lineno} 行）" if getattr(e, "lineno", None) else ""))
    return errs


def check_hooks(project: Path, plugin: Path | None, has_install: bool) -> list[dict]:
    out = []
    dirs = [("plugin 的 hooks/", plugin / "hooks")] if plugin and (plugin / "hooks").is_dir() else []
    if (project / ".claude" / "hooks").is_dir():
        dirs.append(("專案的 .claude/hooks/", project / ".claude" / "hooks"))
    for label, d in dirs:
        errs = compile_errors(d)
        out.append(item("hooks-compile", ERR, f"{label} 有檔案編譯不過（守門會退回最小規則或失效）：" + "；".join(errs[:5]),
                        "plugin 的檔案：重新安裝或更新 plugin；專案的檔案：跑 init.py --target <這個專案> --upgrade --hooks project") if errs else
                   item("hooks-compile", OK, f"{label} 的 Python 檔都編譯得過（用現在這個 Python {sys.version_info[0]}.{sys.version_info[1]}）"))
    registered = False
    for name in ("settings.json", "settings.local.json"):
        s = load_json(project / ".claude" / name)
        registered = registered or (isinstance(s, dict) and "awf_hook.py" in json.dumps(s.get("hooks") or {}))
    if registered and has_install:
        out.append(item("hooks-double", WARN, "hook 註冊了兩次：專案的 settings 有 awf_hook.py，plugin 也會註冊，每次工具呼叫會檢查兩遍",
                        "擇一：留專案的（較嚴，適合 Windows）就停用 plugin 的 hook；留 plugin 的就把專案 settings 裡 awf_hook.py 的 hooks 拿掉"))
    elif not registered and not has_install and not dirs:
        out.append(item("hooks", WARN, "找不到任何 hook 註冊：守門與交件檢查沒有在跑", "安裝 plugin，或跑 init.py --target <這個專案> --upgrade --hooks project"))
    return out


# ------------------------------------------------------------------ 設定
def check_config(project: Path, plugin: Path | None) -> list[dict]:
    out = []
    teams = project / ".claude" / "teams"
    mf = teams / "methodology.json"
    meth = load_json(mf)
    if not mf.is_file():
        out.append(item("methodology", ERR, "沒有 .claude/teams/methodology.json（開發規範 profile）", "跑 init.py --target <這個專案> --upgrade，或用 /agent-workflow:init-workflow 重新設定"))
    elif not isinstance(meth, dict):
        out.append(item("methodology", ERR, "methodology.json 不是合法的 JSON 物件", "修正 JSON 語法（常見：多了結尾逗號、少了引號）"))
    else:
        try:
            import schemalib
            errs = schemalib.check_doc(meth, "methodology")
        except Exception as e:  # noqa: BLE001 驗證器本身出錯不能讓 doctor 崩潰
            errs, meth_note = [], f"驗證器跑不起來（{type(e).__name__}：{e}）"
            out.append(item("methodology", WARN, "methodology.json 沒辦法驗證：" + meth_note, "跑 init.py --target <這個專案> --upgrade 補齊工具與 schema"))
        else:
            out.append(item("methodology", ERR, f"methodology.json 有 {len(errs)} 個問題：" + "；".join(errs[:5]) + ("…" if len(errs) > 5 else ""),
                            "照訊息修正；欄位說明見模版的 docs/自訂與擴充.md") if errs else item("methodology", OK, "methodology.json 通過 schema 與語意規則"))
    gf = teams / "guard.config.json"
    guard = load_json(gf)
    if not gf.is_file():
        out.append(item("guard", WARN, "沒有 .claude/teams/guard.config.json：hook 只套用安全預設（只管 tpm-／dev- 角色，不擋主 session）", "跑 init.py --target <這個專案> --upgrade"))
    elif not isinstance(guard, dict):
        out.append(item("guard", ERR, "guard.config.json 不是合法的 JSON 物件：hook 讀不到設定", "修正 JSON 語法"))
    elif LB in gf.read_text(encoding="utf-8", errors="replace"):
        out.append(item("guard", ERR, "guard.config.json 還有沒換掉的佔位符（兩個左大括號開頭的那種）", "補上 workflow.config 的值後跑 init.py --target <這個專案> --upgrade"))
    else:
        out.append(item("guard", OK, "guard.config.json 讀得懂" + ("；auto_merge 已啟用" if isinstance(guard.get("auto_merge"), dict) and guard["auto_merge"].get("enabled") else "")))
    if not (teams / "workflow.config").is_file():
        out.append(item("config", WARN, "沒有 .claude/teams/workflow.config：workflow 範本的佔位符沒有值可以換，awf.py next 會拒絕", "跑 init.py --target <這個專案> --upgrade"))
    if isinstance(meth, dict):
        import awf
        miss = awf._unregistered(meth, project)
        if miss:
            out.append(item("extensions", ERR, "methodology.json 的 extensions 有 agent 沒登記守門角色：" + "、".join(miss), "用 /agent-workflow:add-agent 設定，或在 guard.config.json 的 extension_agents 補上"))
    return out


def check_overrides(project: Path, plugin: Path | None) -> list[dict]:
    d = project / ".claude" / "workflows"
    if not d.is_dir():
        return []
    import awf
    pv = awf._version(plugin)
    out, old = [], []
    for f in sorted(d.glob("*.js")):
        m = awf.OVERRIDE.search(f.read_text(encoding="utf-8", errors="replace"))
        if not m:
            old.append(f.name)
        elif pv and m.group(1) != pv:
            out.append(item("override", WARN, f"覆寫檔 {f.name} 是依 {m.group(1)} 版的範本，plugin 現在是 {pv}：新版的修正不會自動帶進來", "對照 plugin 的 workflow-templates/ 更新覆寫檔，再把結尾的 awf-override 版本改成新的"))
        else:
            out.append(item("override", INFO, f"採用專案的覆寫檔 {f.name}"))
    if old:
        out.append(item("override-old", INFO, ".claude/workflows/ 有沒有標記的同名檔（舊版複製的副本，不會被採用）：" + "、".join(old[:8]) + ("…" if len(old) > 8 else ""),
                        "確認沒在用：跑 init.py --target <這個專案> --upgrade 會移除沒改過的、保留改過的並說明"))
    return out


# ------------------------------------------------------------------ 主流程
def run_checks(project: Path, home: Path | None = None, plugin_dir: str = "", which=shutil.which, py: tuple = tuple(sys.version_info[:3])) -> list[dict]:
    home = config_home(home)
    out = check_env(which, py)
    if not (project / ".claude" / "teams").is_dir():
        out.append(item("project", ERR, "這個專案還沒套用 agent-workflow（沒有 .claude/teams/）",
                        "在 Claude Code 裡用 /agent-workflow:init-workflow（會先問開發習慣）；想先用預設值：python3 <模版 repo>/init.py --target . --defaults"))
        return out + check_installs(project, home)
    import awf
    plugin, notes = None, []
    try:
        plugin, notes = awf.find_plugin(project, plugin_dir, home)
    except awf.AwfError as e:
        out.append(item("plugin", ERR, str(e), "確認 --plugin-dir 指到 plugin 目錄（裡面有 workflow-templates/）"))
        return out
    inst = check_installs(project, home)
    has_install = any(i["id"] != "install" or i["level"] == OK for i in inst)
    for part in (lambda: check_plugin(project, plugin, notes), lambda: inst, lambda: check_hooks(project, plugin, has_install),
                 lambda: check_config(project, plugin), lambda: check_overrides(project, plugin)):
        try:
            out += part()
        except Exception as e:  # noqa: BLE001 單一檢查出錯不影響其他檢查
            out.append(item("doctor", WARN, f"有一項檢查自己出錯了（{type(e).__name__}：{e}），略過"))
    return out


def text_of(items: list[dict]) -> str:
    lines = []
    for i in items:
        lines.append(f"[{MARK[i['level']]}] {i['msg']}")
        if i["fix"] and i["level"] != OK:
            lines.append(f"       修法：{i['fix']}")
    n_err, n_warn = sum(i["level"] == ERR for i in items), sum(i["level"] == WARN for i in items)
    lines.append(f"doctor：{n_err} 個問題、{n_warn} 個注意" if n_err or n_warn else "doctor：都正常")
    return "\n".join(lines)


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def main(argv: list[str], project: Path | None = None, home: Path | None = None) -> int:
    if "--selftest" in argv:
        return selftest()
    items = run_checks(project or PROJECT, home, opt(argv, "--plugin-dir"))
    print(json.dumps({"ok": not any(i["level"] == ERR for i in items), "checks": items}, ensure_ascii=False, indent=2) if "--json" in argv else text_of(items))
    return 1 if any(i["level"] == ERR for i in items) else 0


# ------------------------------------------------------------------ selftest
def selftest() -> int:
    import tempfile
    ok = True

    def check(name: str, cond, detail="") -> None:
        nonlocal ok
        ok = ok and bool(cond)
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{str(detail)[-500:]}"))

    def ids(items, level=None):
        return [i["id"] for i in items if level is None or i["level"] == level]

    def w(p: Path, text: str) -> Path:
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
        return p

    all_tools = lambda n: "/x/" + n  # noqa: E731
    with tempfile.TemporaryDirectory() as td:
        root = Path(td).resolve()
        print("awf_doctor selftest（暫存目錄，不碰 repo 與使用者的 ~/.claude）")
        # ── 環境
        e = check_env(all_tools)
        check("環境：python、git、gh、node 都在 → 四項通過", ids(e, OK) == ["python", "git", "gh", "node"], e)
        e = check_env(lambda n: None if n in ("gh", "node") else "/x", (3, 8, 0))
        check("環境：Python 太舊是問題；缺 gh、node 只是注意，各附修法", ids(e, ERR) == ["python"] and ids(e, WARN) == ["gh", "node"] and all(i["fix"] for i in e if i["level"] != OK), e)
        check("環境：缺 git 是問題", "git" in ids(check_env(lambda n: None), ERR))
        # ── 假的 plugin 與專案
        plugin = root / "plugin"
        w(plugin / ".claude-plugin/plugin.json", json.dumps({"name": PLUGIN_NAME, "version": "0.5.0"}))
        w(plugin / "workflow-templates/sdlc-plan.js", "// x\n")
        w(plugin / "hooks/awf_hook.py", "x = 1\n")
        proj = root / "proj"
        teams = proj / ".claude/teams"
        good = json.loads((HERE.parent / "methodologies" / "default.json").read_text(encoding="utf-8"))
        w(teams / "methodology.json", json.dumps(good, ensure_ascii=False))
        w(teams / "guard.config.json", json.dumps({"runs_dir": "_tpm/runs"}))
        w(teams / "workflow.config", 'PROJECT_NAME="示範客戶 ACME"\n')
        w(teams / "VERSION", "0.5.0\n")
        home = root / "home"
        (home / "plugins").mkdir(parents=True)

        def doctor(**kw):
            return run_checks(proj, home, str(plugin), all_tools, **kw)

        r = doctor()
        check("乾淨的專案：沒有問題也沒有注意", not ids(r, ERR) and not ids(r, WARN), [i for i in r if i["level"] in (ERR, WARN)])
        check("乾淨的專案：版本一致、hook 編譯得過、profile 通過都列出來", {"version", "hooks-compile", "methodology", "guard"} <= set(ids(r, OK)), ids(r, OK))
        check("輸出：文字版每個不通過的項目下一行是修法；結尾有總結", "doctor：都正常" in text_of(r) and "修法：" in text_of(check_env(lambda n: None)))
        # ── 還沒套用
        r = run_checks(root / "empty", home, "", all_tools)
        check("還沒套用的專案：說明並給兩種套用方式，不往下檢查設定", ids(r, ERR) == ["project"] and "init-workflow" in r[4]["fix"] and "--defaults" in r[4]["fix"] and "methodology" not in ids(r), r)
        # ── 版本
        w(teams / "VERSION", "0.4.1\n")
        r = doctor()
        check("版本：專案的工具比 plugin 舊 → 注意，修法是 init.py --upgrade", "version" in ids(r, WARN) and "--upgrade" in next(i["fix"] for i in r if i["id"] == "version"), r)
        w(teams / "VERSION", "0.6.0\n")
        r = doctor()
        check("版本：plugin 比專案舊 → 注意，修法是 plugin update 加 /reload-plugins", "version" in ids(r, WARN) and "plugin update" in next(i["fix"] for i in r if i["id"] == "version")
              and "/reload-plugins" in next(i["fix"] for i in r if i["id"] == "version"), r)
        (teams / "VERSION").unlink()
        check("版本：沒有 VERSION 檔 → 注意", "version" in ids(doctor(), WARN))
        w(teams / "VERSION", "0.5.0\n")
        check("版本比較：0.10.0 比 0.9.9 新；帶後綴的版本只看數字段", vtuple("0.10.0") > vtuple("0.9.9") and vtuple("0.3.0-x.1") == (0, 3, 0) and vtuple("") == ())
        # ── 找不到 plugin
        saved = {k: os.environ.pop(k, None) for k in ("AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT")}
        try:
            import awf
            real = awf.find_plugin
            awf.find_plugin = lambda *a, **k: (None, [])
            r = run_checks(proj, home, "", all_tools)
            awf.find_plugin = real
        finally:
            for k, v in saved.items():
                if v is not None:
                    os.environ[k] = v
        check("找不到 plugin → 問題，修法提到安裝與 --plugin-dir", "plugin" in ids(r, ERR) and "--plugin-dir" in next(i["fix"] for i in r if i["id"] == "plugin"), r)
        r = run_checks(proj, home, str(root / "nope"), all_tools)
        check("--plugin-dir 指錯 → 問題並說明，不丟例外", "plugin" in ids(r, ERR), r)
        # ── 安裝來源
        inst_dir = w(root / "cache/a/workflow-templates/x.js", "").parents[1]
        other_dir = w(root / "cache/b/workflow-templates/x.js", "").parents[1]

        def installed(plugins: dict) -> None:
            w(home / "plugins/installed_plugins.json", json.dumps({"version": 2, "plugins": plugins}))

        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.5.0"}]})
        r = check_installs(proj, home)
        check("安裝來源：只有一個來源、一個版本 → 通過", ids(r, OK) == ["install"], r)
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.5.0"}],
                   "agent-workflow@my-fork": [{"scope": "user", "installPath": str(other_dir), "version": "0.3.0"}],
                   "other-plugin@x": [{"scope": "user", "installPath": str(other_dir), "version": "9"}]})
        r = check_installs(proj, home)
        check("安裝來源：同一個 plugin 從兩個 marketplace 安裝 → 注意，列出兩個來源，修法是移除其中一個",
              ids(r, WARN) == ["install-sources"] and "agent-workflow@my-fork" in r[0]["msg"] and "agent-workflow@agentworkflow" in r[0]["msg"] and "uninstall" in r[0]["fix"], r)
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.5.0"}],
                   "agent-workflow@my-fork": [{"scope": "project", "projectPath": str(root / "別的專案"), "installPath": str(other_dir), "version": "0.3.0"}]})
        check("安裝來源：另一個來源只裝在別的專案 → 不算衝突", ids(check_installs(proj, home), OK) == ["install"], check_installs(proj, home))
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.5.0"},
                                                     {"scope": "project", "projectPath": str(proj), "installPath": str(other_dir), "version": "0.4.1"}]})
        check("安裝來源：同一來源在兩個範圍裝了不同版本 → 注意", ids(check_installs(proj, home), WARN) == ["install-versions"], check_installs(proj, home))
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(root / "不見了"), "version": "0.5.0"}]})
        check("安裝來源：登記的安裝目錄不存在 → 問題", ids(check_installs(proj, home), ERR) == ["install-path"], check_installs(proj, home))
        mk = root / "market"
        w(mk / ".claude-plugin/marketplace.json", json.dumps({"name": "agentworkflow", "plugins": [{"name": PLUGIN_NAME, "source": "./plugin", "version": "0.5.0"}]}))
        w(home / "plugins/known_marketplaces.json", json.dumps({"agentworkflow": {"installLocation": str(mk)}}))
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.4.1"}]})
        r = check_installs(proj, home)
        check("安裝來源：marketplace 已經是新版、安裝的還是舊版 → 注意，修法是 plugin update（不是 marketplace update）",
              ids(r, WARN) == ["install-stale"] and "0.4.1" in r[0]["msg"] and "0.5.0" in r[0]["msg"] and "claude plugin update agent-workflow@agentworkflow" in r[0]["fix"], r)
        w(mk / ".claude-plugin/marketplace.json", json.dumps({"plugins": [{"name": PLUGIN_NAME, "source": "./plugin"}]}))
        w(mk / "plugin/.claude-plugin/plugin.json", json.dumps({"version": "0.6.0"}))
        check("安裝來源：marketplace.json 沒寫版本時讀 plugin.json", market_version(home, "agentworkflow") == "0.6.0")
        w(home / "plugins/installed_plugins.json", "{壞掉")
        r = check_installs(proj, home)
        check("安裝來源：登記檔讀不懂 → 只說明、略過，不算問題", ids(r) == ["install"] and r[0]["level"] == INFO and "讀不懂" in r[0]["msg"], r)
        (home / "plugins/installed_plugins.json").unlink()
        check("安裝來源：沒有登記檔 → 說明（用 --plugin-dir 時不需要）", check_installs(proj, home)[0]["level"] == INFO)
        # ── hook
        w(plugin / "hooks/guard.py", "def broken(:\n    pass\n")
        r = doctor()
        bad = next((i for i in r if i["id"] == "hooks-compile" and i["level"] == ERR), None)
        check("hook：plugin 的 hook 檔有語法錯誤 → 問題，指出檔名與行號", bad is not None and "guard.py" in bad["msg"] and "第 1 行" in bad["msg"], r)
        import contextlib
        import io
        with contextlib.redirect_stdout(io.StringIO()):
            code = main(["--plugin-dir", str(plugin)], proj, home)
        check("hook：doctor 的結束碼跟著是 1（main）", code == 1)
        w(plugin / "hooks/guard.py", "y = 2\n")
        w(proj / ".claude/hooks/awf_hook.py", "import os\n")
        w(proj / ".claude/settings.json", json.dumps({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "python3 .claude/hooks/awf_hook.py guard-shell"}]}]}}))
        installed({"agent-workflow@agentworkflow": [{"scope": "user", "installPath": str(inst_dir), "version": "0.5.0"}]})
        w(home / "plugins/known_marketplaces.json", "{}")
        r = doctor()
        check("hook：專案 settings 與 plugin 都註冊 → 注意（重複註冊），修法是擇一", "hooks-double" in ids(r, WARN) and len([i for i in r if i["id"] == "hooks-compile"]) == 2, r)
        (home / "plugins/installed_plugins.json").unlink()
        check("hook：只有專案註冊（沒有 plugin 安裝紀錄）→ 不算重複", "hooks-double" not in ids(doctor()))
        shutil.rmtree(proj / ".claude/hooks")
        (proj / ".claude/settings.json").unlink()
        # ── 設定
        w(teams / "methodology.json", "{壞掉")
        check("設定：methodology.json 不是 JSON → 問題", "methodology" in ids(doctor(), ERR))
        w(teams / "methodology.json", json.dumps({**good, "constraints": [{"id": "a", "file": "/etc/passwd", "stages": ["design"]}]}, ensure_ascii=False))
        r = doctor()
        check("設定：methodology.json 違反語意規則（限制文件用絕對路徑）→ 問題並帶出訊息", "methodology" in ids(r, ERR) and "constraints" in next(i["msg"] for i in r if i["id"] == "methodology"), r)
        w(teams / "methodology.json", json.dumps({**good, "extensions": [{"name": "x", "agent": "my:agent", "stage": "plan", "instructions": "看一下"}]}, ensure_ascii=False))
        r = doctor()
        check("設定：擴充 agent 沒登記守門角色 → 問題，修法提到 add-agent", "extensions" in ids(r, ERR) and "add-agent" in next(i["fix"] for i in r if i["id"] == "extensions"), r)
        w(teams / "methodology.json", json.dumps(good, ensure_ascii=False))
        w(teams / "guard.config.json", '{"runs_dir": "' + LB + 'RUNS_DIR}}"}')
        check("設定：guard.config.json 還有佔位符 → 問題", "guard" in ids(doctor(), ERR))
        w(teams / "guard.config.json", "[]")
        check("設定：guard.config.json 不是物件 → 問題", "guard" in ids(doctor(), ERR))
        (teams / "guard.config.json").unlink()
        (teams / "workflow.config").unlink()
        r = doctor()
        check("設定：缺 guard.config.json、workflow.config → 注意（各一筆）", {"guard", "config"} <= set(ids(r, WARN)), r)
        w(teams / "guard.config.json", json.dumps({"runs_dir": "_tpm/runs"}))
        w(teams / "workflow.config", 'PROJECT_NAME="示範客戶 ACME"\n')
        # ── 覆寫檔
        w(proj / ".claude/workflows/sdlc-plan.js", "// 我的版本\n// awf-override: 0.4.1\n")
        w(proj / ".claude/workflows/sdlc-test.js", "// 現行\n// awf-override: 0.5.0\n")
        w(proj / ".claude/workflows/sdlc-build.js", "// 舊副本，沒有標記\n")
        r = doctor()
        check("覆寫檔：依舊版範本的覆寫檔 → 注意；版本相同的只說明；沒標記的舊副本列出來並說明不會被採用",
              [i["id"] for i in r if i["id"].startswith("override")] == ["override", "override", "override-old"] and ids([i for i in r if i["id"] == "override"], WARN) == ["override"]
              and "sdlc-build.js" in next(i["msg"] for i in r if i["id"] == "override-old"), [i for i in r if i["id"].startswith("override")])
        # ── 不改檔、可以輸出 JSON
        before = sorted((str(p.relative_to(root)), p.stat().st_size) for p in root.rglob("*") if p.is_file() and "__pycache__" not in p.parts)
        doctor()
        after = sorted((str(p.relative_to(root)), p.stat().st_size) for p in root.rglob("*") if p.is_file() and "__pycache__" not in p.parts)
        check("只讀：跑完沒有新增或改動任何檔案", before == after)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = main(["--json", "--plugin-dir", str(plugin)], proj, home)
        d = json.loads(buf.getvalue())
        check("--json：輸出 {ok, checks[]}，每筆有 id、level、msg、fix；沒有問題時 exit 0", code == 0 and d["ok"] is True and all(set(i) == {"id", "level", "msg", "fix"} for i in d["checks"]), buf.getvalue()[:300])
    print("awf_doctor selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
