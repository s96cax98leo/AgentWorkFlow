#!/usr/bin/env python3
"""狀態機：主 session 每一段只做兩件事——`awf.py next`（照它說的做）與 `awf.py done`（把結果存起來）。

用法（在已 init 的專案根目錄）：
    python3 .claude/teams/tools/awf.py start --run <run_dir> --event <sdlc|brief|upstream|…> [--repo <repo>] [--epic <短名>]
        [--requirement <需求原文或路徑>] [--base <base>] [--recipe <名>] [--modifiers a,b] [--size s] [--set k=v]… [--set-json k=<JSON>]…
    python3 .claude/teams/tools/awf.py next --run <run_dir> [--set k=v]… [--set-json k=<JSON>]… [--stage <段>] [--workflow <名>] [--text]
    python3 .claude/teams/tools/awf.py done --run <run_dir> [--output <workflow 回傳的 JSON 檔>] [--stage <段>]
    python3 .claude/teams/tools/awf.py approve --run <run_dir> [<段>] --by <誰> [--note <備註>] [--answer "<問題>=<答覆>"]…
    python3 .claude/teams/tools/awf.py render <workflow>|--all --out <目錄>
    python3 .claude/teams/tools/awf.py --selftest
    共用選項：--plugin-dir <plugin 目錄>（找不到 plugin 時才需要）

next 回一個動作（JSON；--text 是給人看的摘要）：
    run_workflow  用 Workflow 工具執行 script（args 已經內嵌在腳本裡，不用再傳），完成後跑 then 的指令。
                  有 resume_from 時把它當 resumeFromRunId 傳給 Workflow：已完成而且派工單沒變的 agent 不會重跑。
    gate          這一段等人核准：把 highlights 列給使用者看，核准後跑 approve 的指令；退回就照使用者的意見處理。
    need_input    缺參數：照 need 的說明問使用者或自己判斷，用 next --set 補上。
    manual        要人當下同意、由主 session 自己做的步驟（例如推 fork、開 PR）。
    halted        這一段停下了：reason 與 advice 是原因與建議；處理完用 retry 的動作重跑。
    done          這個 run 沒有下一步了。

workflow 腳本從哪來：plugin 的 workflow-templates/<名>.js（範本），加上專案的佔位符值（.claude/teams/workflow.config）
與開發規範 profile（.claude/teams/methodology.json），產生 <run_dir>/.wf/<名>.js。改了 profile 不用再跑 build.py。
專案的 .claude/workflows/<名>.js 結尾有「// awf-override: <版本>」時當成覆寫（用它、不用 plugin 的範本；版本和 plugin 不同會警告）；
沒有這個標記的同名檔（0.4 以前 init 複製的副本）不採用。

done 會自己找 workflow 的回傳（Claude Code 存在 session 的 workflows/wf_<id>.json），找不到時用 --output 指定檔案。
存檔與驗證仍然是 save_run.py stage，核准仍然是 save_run.py approve：這支只是幫忙排順序、組參數、取時間，不放寬任何閘門。
plan 段的核准一律由人；狀態機不會自己核准任何一段。只用標準庫。
"""

from __future__ import annotations

import contextlib
import datetime
import io
import json
import os
import re
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import awf_cost  # noqa: E402
import build as buildlib  # noqa: E402
import pack as packlib  # noqa: E402
import recipe as rcp  # noqa: E402
import save_run as sr  # noqa: E402

PROJECT = HERE.parents[2]
PLUGIN_NAME = "agent-workflow"
TEMPLATES = "workflow-templates"
OVERRIDE = re.compile(r"^// awf-override: *(\S+)\s*$", re.M)
PH = re.compile(r"\{\{([A-Z_]+)\}\}")
ARGS_LINE = "const A = args || {}"
# 事件 → （workflow, 段名）。段名是 workflow 的 awfInit stage，執行紀錄寫在 <run_dir>/<段名>/
EVENTS = {
    "brief": ("event-brief", "brief"), "upstream": ("event-upstream", "upstream"), "meeting": ("event-meeting", "meeting"),
    "pr-review": ("event-pr-review", "pr-review"), "reqs-change": ("event-reqs-change", "reqs-change"),
    "decision": ("event-decision", "decision"), "demo-check": ("event-demo-check", "demo-check"),
    "post-merge": ("event-post-merge", "post-merge"), "merge": ("sdlc-merge", "merge"),
}
REQUIRED = {
    "sdlc-plan": ["repo", "epic", "requirement", "run_dir"], "sdlc-design": ["repo", "epic", "run_dir"],
    "sdlc-design-revise": ["repo", "epic", "run_dir", "decisions", "issues"], "sdlc-build": ["repo", "epic", "run_dir", "subtasks"],
    "sdlc-sync": ["repo", "epic", "run_dir"], "sdlc-test": ["repo", "epic", "run_dir"], "sdlc-deliver": ["repo", "epic", "run_dir"],
    "sdlc-merge": ["repo", "run_dir", "pr"], "event-brief": ["brief", "run_dir"], "event-meeting": ["source", "run_dir"],
    "event-decision": ["decision", "run_dir"], "event-demo-check": ["target", "run_dir"], "event-post-merge": ["repo", "run_dir"],
    "event-upstream": ["what", "repo", "run_dir"], "event-reqs-change": ["change", "run_dir"], "event-pr-review": ["target", "repo", "run_dir"],
}
HINTS = {
    "repo": "目標 repo（我方的本機 clone）", "epic": "這個需求的短名（specs/<epic>/、分支名都用它）", "requirement": "需求原文或檔案路徑",
    "pr": "PR 編號：先經使用者同意推 fork、開 PR（publish.py），再用 --set pr=<號> 補上",
    "has_ui": "這次改動有沒有畫面（true／false；依 spec 判斷）", "has_security": "這次改動有沒有安全面（true／false；依 spec 判斷）",
    "decisions": "使用者對退回的 design 做的決定（--set-json decisions='[\"…\"]'）", "issues": "要修的問題（--set-json issues='[{\"target_id\":\"…\",\"reason\":\"…\"}]'）",
    "subtasks": "子任務清單：design（或 plan）段的結果裡沒有，確認那一段已經存檔",
    "brief": "簡報檔路徑", "source": "會議紀錄或逐字稿的路徑", "decision": "決策內容", "target": "要處理的對象（PR、分支或環境）",
    "what": "上游發生了什麼（例如 PR 編號、main 前進、CI 失敗）", "change": "需求改了什麼",
}
STAGE_WORKFLOW = {s: f"sdlc-{s}" for s in sr.SDLC}


class AwfError(Exception):
    pass


def _utf8() -> None:
    for s in (sys.stdout, sys.stderr):
        try:
            s.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass


def _posix(p) -> str:
    return str(p).replace("\\", "/")


def opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def opts(argv: list[str], name: str) -> list[str]:
    return [argv[i + 1] for i, a in enumerate(argv) if a == name and i + 1 < len(argv)]


def now_iso() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load_json(p: Path):
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


# ------------------------------------------------------------------ plugin 與範本
def _version(plugin: Path | None) -> str:
    d = load_json(plugin / ".claude-plugin" / "plugin.json") if plugin else None
    return str(d.get("version") or "") if isinstance(d, dict) else ""


def plugin_candidates(project: Path, home: Path | None = None) -> list[dict]:
    """Claude Code 登記的 agent-workflow 安裝（installed_plugins.json；格式沒承諾穩定，讀不懂就回空）。"""
    home = home or Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude")
    d = load_json(home / "plugins" / "installed_plugins.json")
    out = []
    for key, entries in ((d or {}).get("plugins") or {}).items() if isinstance(d, dict) and isinstance(d.get("plugins"), dict) else []:
        if str(key).split("@")[0] != PLUGIN_NAME:
            continue
        for e in entries if isinstance(entries, list) else [entries]:
            if isinstance(e, dict) and e.get("installPath") and (Path(str(e["installPath"])) / TEMPLATES).is_dir():
                mine = bool(e.get("projectPath")) and Path(str(e["projectPath"])).resolve() == project.resolve()
                out.append({"key": str(key), "path": Path(str(e["installPath"])), "version": str(e.get("version") or ""), "mine": mine,
                            "other_project": bool(e.get("projectPath")) and not mine, "updated": str(e.get("lastUpdated") or "")})
    return out


def find_plugin(project: Path, explicit: str = "", home: Path | None = None) -> tuple[Path | None, list[str]]:
    """plugin 目錄（要有 workflow-templates/）：--plugin-dir、環境變數、模版 repo 自己、Claude Code 登記的安裝。"""
    notes: list[str] = []
    for raw in (explicit, os.environ.get("AWF_PLUGIN_DIR"), os.environ.get("CLAUDE_PLUGIN_ROOT")):
        if raw and (Path(raw) / TEMPLATES).is_dir():
            return Path(raw), notes
    if explicit:
        raise AwfError(f"--plugin-dir {explicit} 底下沒有 {TEMPLATES}/")
    repo_plugin = HERE.parents[3] / "plugin" if len(HERE.parents) > 3 else None   # 在模版 repo 裡直接跑（scaffold/.claude/teams/tools/）
    if repo_plugin is not None and (repo_plugin / TEMPLATES).is_dir() and HERE.parents[2].name == "scaffold":
        return repo_plugin, notes
    cands = [c for c in plugin_candidates(project, home) if not c["other_project"]]
    if not cands:
        return None, notes
    want = (project / ".claude" / "teams" / "VERSION").read_text(encoding="utf-8").strip() if (project / ".claude" / "teams" / "VERSION").is_file() else ""
    cands.sort(key=lambda c: (c["mine"], c["version"] == want, c["updated"]), reverse=True)
    if len({c["key"] for c in cands}) > 1:
        notes.append("裝了不只一個來源的 agent-workflow（" + "、".join(sorted({c["key"] for c in cands})) + f"），這次用 {cands[0]['key']}；用 --plugin-dir 可以指定")
    return cands[0]["path"], notes


def read_config(project: Path) -> dict:
    """init.py 記下的佔位符值（KEY=value）。"""
    out: dict = {}
    try:
        lines = (project / ".claude" / "teams" / "workflow.config").read_text(encoding="utf-8-sig").splitlines()
    except OSError:
        return out
    for line in lines:
        if re.match(r"^[A-Z_]+=", line):
            k, _, v = line.rstrip("\r").partition("=")
            out[k] = v[1:-1] if len(v) >= 2 and v.startswith('"') and v.endswith('"') else v.strip('"') if v in ('"', '""') else v
    return out


def find_template(name: str, project: Path, plugin: Path | None) -> tuple[str, str, list[str]]:
    """（腳本文字, 來源, 說明）。來源：override（專案的覆寫檔）、plugin（範本）、legacy（找不到 plugin 時退回專案裡的舊副本）。"""
    if not re.match(r"^[a-z][a-z0-9-]{0,63}$", name):
        raise AwfError(f"workflow 名稱不對：{name}")
    notes: list[str] = []
    tpl = plugin / TEMPLATES / f"{name}.js" if plugin else None
    ov = project / ".claude" / "workflows" / f"{name}.js"
    if ov.is_file():
        text = ov.read_text(encoding="utf-8").replace("\r\n", "\n")
        m = OVERRIDE.search(text)
        if m:
            pv = _version(plugin)
            if pv and m.group(1) != pv:
                notes.append(f"用的是專案的覆寫檔 .claude/workflows/{name}.js（依 {m.group(1)} 版的範本，plugin 現在是 {pv}）：新版的修正不會自動帶進來，請對照更新")
            else:
                notes.append(f"用的是專案的覆寫檔 .claude/workflows/{name}.js")
            return text, "override", notes
        if tpl is None or not tpl.is_file():
            notes.append(f"找不到 plugin 的範本，改用專案裡的 .claude/workflows/{name}.js（舊副本）；請確認 plugin 已安裝，或用 --plugin-dir 指定")
            return text, "legacy", notes
        notes.append(f".claude/workflows/{name}.js 沒有 awf-override 標記（舊版 init 複製的副本），不採用，改用 plugin 的範本；確認沒在用就可以刪掉")
    if tpl is None or not tpl.is_file():
        raise AwfError(f"找不到 workflow 範本 {name}.js：" + ("plugin 沒有這支" if plugin else "找不到 agent-workflow plugin（先安裝 plugin，或用 --plugin-dir 指定）"))
    return tpl.read_text(encoding="utf-8").replace("\r\n", "\n"), "plugin", notes


def render(name: str, project: Path, plugin: Path | None, args: dict | None = None) -> tuple[str, list[str]]:
    """產生可以執行的 workflow 腳本：換佔位符、內嵌開發規範 profile；有 args 時一併內嵌（主 session 不用再傳）。"""
    text, source, notes = find_template(name, project, plugin)
    cfg = read_config(project)
    text = PH.sub(lambda m: cfg[m.group(1)] if m.group(1) in cfg else m.group(0), text)
    left = sorted(set(PH.findall(text)))
    if left:
        raise AwfError(f"{name}.js 還有沒換掉的佔位符（.claude/teams/workflow.config 缺這些值；跑 init.py --upgrade 補上）：" + "、".join(left))
    meth_file = project / ".claude" / "teams" / "methodology.json"
    methodology = load_json(meth_file)
    if meth_file.is_file() and not isinstance(methodology, dict):
        raise AwfError(f"{meth_file} 讀不懂（不是 JSON 物件）")
    missing = _unregistered(methodology, project)
    if missing:
        raise AwfError("methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents：" + "、".join(missing)
                       + "（用 /agent-workflow:add-agent 設定）")
    meth = json.dumps(methodology, ensure_ascii=False, separators=(",", ":"))
    text = buildlib.METH_MARK.sub(lambda _: f"/*@methodology*/ {meth} /*@end*/", text)
    if args is not None:
        if text.count("\n" + ARGS_LINE + "\n") != 1:
            raise AwfError(f"{name}.js 沒有唯一的一行「{ARGS_LINE}」，沒辦法內嵌參數（覆寫檔請保留這一行）")
        emb = json.dumps(args, ensure_ascii=False, separators=(",", ":"))
        text = text.replace("\n" + ARGS_LINE + "\n", "\nconst A = Object.assign(" + emb + ", args || {})\n", 1)
    pv, tv = _version(plugin), ""
    try:
        tv = (project / ".claude" / "teams" / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        pass
    if source == "plugin" and pv and tv and pv != tv:
        notes.append(f"plugin 是 {pv}，專案的工具是 {tv}：跑 init.py --upgrade 讓兩邊一致（不一致時閘門工具可能不認得新版 workflow 的回傳）")
    return text, notes


def _unregistered(methodology, project: Path) -> list[str]:
    ext = (methodology or {}).get("extensions") if isinstance(methodology, dict) else None
    if not isinstance(ext, list) or not ext:
        return []
    g = load_json(project / ".claude" / "teams" / "guard.config.json") or {}
    reg = g.get("extension_agents") if isinstance(g.get("extension_agents"), dict) else {}
    return sorted({str(e.get("agent")) for e in ext if isinstance(e, dict) and e.get("agent") and e.get("agent") not in reg})


def workflow_names(project: Path, plugin: Path | None) -> list[str]:
    names = {p.stem for p in (plugin / TEMPLATES).glob("*.js")} if plugin else set()
    ovd = project / ".claude" / "workflows"
    names |= {p.stem for p in ovd.glob("*.js") if OVERRIDE.search(p.read_text(encoding="utf-8", errors="replace"))} if ovd.is_dir() else set()
    return sorted(names)


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:   # 不轉換行：Workflow 與 check_workflows 都照 \n 解析
        f.write(text)


# ------------------------------------------------------------------ 參數
def _coerce(v: str):
    return True if v == "true" else False if v == "false" else int(v) if re.match(r"^-?\d+$", v) else v


def apply_sets(data: dict, argv: list[str]) -> list[str]:
    """--set k=v、--set-json k=<JSON> 記進 run.json 的 args（之後每一段都帶）。回傳錯誤。"""
    store = data.setdefault("args", {})
    errs = []
    for raw in opts(argv, "--set"):
        k, sep, v = raw.partition("=")
        if not sep or not re.match(r"^[a-z_][a-z0-9_]*$", k):
            errs.append(f"--set 要寫成 k=v：{raw}")
        elif k == "run_dir":
            errs.append("run_dir 由 --run 決定，不能用 --set 改")
        else:
            store[k] = _coerce(v)
    for raw in opts(argv, "--set-json"):
        k, sep, v = raw.partition("=")
        try:
            if not sep or not re.match(r"^[a-z_][a-z0-9_]*$", k) or k == "run_dir":
                raise ValueError
            store[k] = json.loads(v)
        except ValueError:
            errs.append(f"--set-json 要寫成 k=<JSON>：{raw[:80]}")
    return errs


def _stage_result(run: Path, stage: str) -> dict:
    d = load_json(run / f"{stage}.json")
    return d if isinstance(d, dict) else {}


def _subtasks(run: Path) -> list:
    d = _stage_result(run, "design")
    p = (d.get("design") or {}).get("payload") if isinstance(d.get("design"), dict) else None
    if isinstance(p, dict) and isinstance(p.get("subtasks"), list) and p["subtasks"]:
        return p["subtasks"]
    p = _stage_result(run, "plan").get("subtasks")
    return p if isinstance(p, list) else []


def stage_args(run: Path, run_arg: str, data: dict, stage: str, workflow: str, project: Path) -> tuple[dict, list[str], list[str]]:
    """這一段 workflow 的 args：（args, 缺的必要參數, 說明）。來源：run.json（需求、recipe、--set 記的值）與前面各段的結果。"""
    store = dict(data.get("args") or {})
    notes: list[str] = []
    a: dict = {k: v for k, v in store.items() if v not in (None, "")}
    # run 目錄給絕對路徑：agent 常常 cd 進目標 repo 的 worktree 工作，相對路徑會讓紀錄與 log 寫到別的目錄
    a["run_dir"] = Path(run).resolve().as_posix()
    if data.get("target_repo") and "repo" not in a:
        a["repo"] = data["target_repo"]
    if data.get("requirement") and workflow in ("sdlc-plan", "sdlc-design"):
        a.setdefault("requirement", data["requirement"])
    if sr.has_recipe(data):    # 鎖在 run.json 的 recipe 優先（--set 改不了）
        rec = data["recipe"]
        a["recipe"], a["modifiers"], a["size"] = rec.get("name"), list(rec.get("modifiers") or []), rec.get("size") or ""
        if not a["size"]:
            a.pop("size")
    elif isinstance(a.get("modifiers"), str):
        a["modifiers"] = [m for m in a["modifiers"].split(",") if m]
    if workflow.startswith("sdlc-") and workflow != "sdlc-merge":
        head = _stage_result(run, "sync").get("head_branch")
        default = read_config(project).get("BRANCH_PREFIX", "") + str(a.get("epic") or "")
        if isinstance(head, str) and head and head != default and stage in ("build", "test", "deliver", "sync"):
            a.setdefault("branch", head)
            notes.append(f"sync 走過中轉分支：這一段帶 branch={a['branch']}")
    if workflow == "sdlc-merge":
        a.pop("branch", None)     # merge 用 PR 號，不帶 branch
    if workflow == "sdlc-build":
        prev = _stage_result(run, "build")
        if prev.get("halted") and isinstance(prev.get("remaining_subtasks"), list) and prev["remaining_subtasks"]:
            a["subtasks"] = prev["remaining_subtasks"]
            a["integrated"] = prev.get("integrated") or []
            notes.append(f"續跑 build：只做還沒合進整合分支的 {len(a['subtasks'])} 個子任務")
        elif "subtasks" not in a:
            a["subtasks"] = _subtasks(run)
        if not a.get("subtasks"):
            a.pop("subtasks", None)
    if workflow in ("sdlc-test", "sdlc-design-revise"):
        sp = _stage_result(run, "design").get("sub_plans")
        if isinstance(sp, list) and sp:
            a.setdefault("sub_plans", sp)
    need = [k for k in REQUIRED.get(workflow, ["run_dir"]) if a.get(k) in (None, "", [])]
    if workflow == "sdlc-test":
        mods = set(a.get("modifiers") or [])
        for k, m in (("has_ui", "ui"), ("has_security", "security")):
            if k not in a and m in mods:
                a[k] = True
            if k not in a:
                need.append(k)
    return a, need, notes


def run_pack(stage: str, run_arg: str, a: dict, run: Path, project: Path) -> tuple[dict, list[str]]:
    """備資料包（輔助：失敗只少一份資料包）。回傳（要併進 args 的值, 說明）。"""
    argv = [stage, "--run", run_arg]
    if stage == "build" and a.get("subtasks"):
        f = run / ".wf" / "build.subtasks.json"
        _write(f, json.dumps(a["subtasks"], ensure_ascii=False))
        argv += ["--subtasks", str(f)] + (["--epic", str(a["epic"])] if a.get("epic") else [])
    buf = io.StringIO()
    cwd = os.getcwd()
    try:
        os.chdir(project)      # run_dir 是相對於專案根目錄的路徑
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            packlib.run_pack(argv, project)
        extra = load_json(run / stage / packlib.ARGS)
        extra = extra if isinstance(extra, dict) else {}
        # 資料包路徑也給絕對路徑（和 run_dir 同理）；pack-args.json 本身維持相對路徑，內容可重現
        absp = lambda v: (Path(project) / v).resolve().as_posix() if isinstance(v, str) and v and not Path(v).is_absolute() else v
        if "pack" in extra:
            extra["pack"] = absp(extra["pack"])
        if isinstance(extra.get("task_packs"), dict):
            extra["task_packs"] = {k: absp(v) for k, v in extra["task_packs"].items()}
        return extra, [ln.strip() for ln in buf.getvalue().splitlines() if ln.strip()][:6]
    except Exception as e:  # noqa: BLE001  輔助功能不擋流程
        return {}, [f"資料包沒有產生（{type(e).__name__}：{e}）；這一段照常跑"]
    finally:
        os.chdir(cwd)


# ------------------------------------------------------------------ 找 workflow 的回傳
def transcripts_root(project: Path) -> Path:
    return Path(os.environ["AWF_TRANSCRIPTS_DIR"]) if os.environ.get("AWF_TRANSCRIPTS_DIR") else awf_cost.project_dir(str(project))


def find_wf_meta(project: Path, script: str, since: float = 0.0) -> tuple[dict | None, Path | None]:
    """最近一次用這支腳本跑的 workflow 紀錄（<session>/workflows/wf_<id>.json；格式沒承諾穩定）。"""
    want = Path(script).resolve() if script else None
    best: tuple[float, dict, Path] | None = None
    root = transcripts_root(project)
    for f in root.glob("*/workflows/wf_*.json") if root.is_dir() else []:
        try:
            mt = f.stat().st_mtime
        except OSError:
            continue
        if mt + 1 < since or (best and mt <= best[0]):
            continue
        meta = load_json(f)
        sp = meta.get("scriptPath") if isinstance(meta, dict) else None
        if not isinstance(sp, str) or want is None:
            continue
        try:
            same = (Path(sp) if Path(sp).is_absolute() else project / sp).resolve() == want
        except OSError:
            same = False
        if same:
            best = (mt, meta, f)
    return (best[1], best[2]) if best else (None, None)


def current_session(project: Path) -> str:
    """目前這個 session 的 id（猜的：專案 transcript 目錄裡最近寫過的那個）。猜不到回空字串。"""
    root = transcripts_root(project)
    try:
        files = [(f.stat().st_mtime, f.stem) for f in root.glob("*.jsonl")]
    except OSError:
        return ""
    return max(files)[1] if files else ""


# ------------------------------------------------------------------ 狀態機
def _envelopes(node, where: str = "", out: list | None = None) -> list:
    out = [] if out is None else out
    if isinstance(node, dict):
        if sr.is_envelope(node):
            out.append((where, node))
        elif where.split(".")[-1] not in sr.NOT_DELIVERY_KEYS:
            for k, v in node.items():
                _envelopes(v, f"{where}.{k}" if where else str(k), out)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            _envelopes(v, f"{where}[{i}]", out)
    return out


def highlights(run: Path, run_arg: str, data: dict, stage: str) -> dict:
    """閘門上要列給人看的重點（取自這一段存下來的結果；只整理，不判定）。"""
    res = _stage_result(run, stage)
    st = data["stages"].get(stage) or {}
    envs = _envelopes(res)
    h: dict = {"agents": [{"where": w, "status": e.get("status"), "summary": str(e.get("summary") or "")[:200]} for w, e in envs][:40]}
    qs = [{"from": w, "question": q.get("question"), "blocking": bool(q.get("blocking")), "options": [o.get("choice") for o in q.get("options") or [] if isinstance(o, dict)],
           "recommendation": q.get("recommendation")} for w, e in envs for q in e.get("open_questions") or [] if isinstance(q, dict)]
    if qs:
        h["open_questions"] = qs[:30]
    for k in ("missing", "refuted", "reused", "models", "gate", "halt_reason", "next", "located_by", "test_first", "sub_plans"):
        if res.get(k) not in (None, [], {}, ""):
            h[k] = res[k]
    if isinstance(res.get("angles"), dict) and res["angles"].get("skipped"):
        h["angles_skipped"] = res["angles"]["skipped"]
    if stage == "plan" and sr.has_recipe(data):
        r = data["recipe"]
        h["recipe"] = {"name": r.get("name"), "modifiers": r.get("modifiers") or [], "size": r.get("size") or "", "stages": (r.get("resolved") or {}).get("stages"),
                       "reasons": r.get("reasons") or []}
    for k in ("missing_records", "ghostwritten_records", "constraint_issues"):
        if st.get(k):
            h[k] = st[k]
    idx = run / stage / sr.INDEX_NAME
    if idx.is_file():
        h["records_index"] = f"{run_arg}/{stage}/{sr.INDEX_NAME}"
    return h


def gate_action(run: Path, run_arg: str, data: dict, stage: str) -> dict:
    return {"action": "gate", "stage": stage, "human_only": stage in sr.HUMAN_ONLY, "highlights": highlights(run, run_arg, data, stage),
            "say": "把 highlights 在對話裡列給使用者看（open_questions 逐一問；blocking 的用選項問）。使用者明確核准才跑 approve；「調整」「可以但…」不是核准。",
            "approve": f'python3 .claude/teams/tools/awf.py approve --run {run_arg} {stage} --by <誰核准的> [--answer "<問題>=<答覆>"]…',
            "cost": f"python3 .claude/teams/tools/awf_cost.py --project . --run-dir {run_arg}"}


def workflow_action(run: Path, run_arg: str, data: dict, stage: str, workflow: str, project: Path, plugin: Path | None, prepare: bool = True) -> dict:
    a, need, notes = stage_args(run, run_arg, data, stage, workflow, project)
    if need:
        return {"action": "need_input", "stage": stage, "workflow": workflow, "need": {k: HINTS.get(k, "這一段需要的參數") for k in need},
                "how": f"python3 .claude/teams/tools/awf.py next --run {run_arg} " + " ".join(f"--set {k}=<值>" for k in need)}
    if prepare and workflow.startswith("sdlc-") and stage in sr.SDLC:
        extra, pnotes = run_pack(stage, run_arg, a, run, project)
        a.update(extra)
        notes += pnotes
    text, rnotes = render(workflow, project, plugin, a)
    script = run / ".wf" / f"{workflow}.js"
    if prepare:
        _write(script, text)
        _write(run / ".wf" / f"{workflow}.args.json", json.dumps(a, ensure_ascii=False, indent=1) + "\n")
    act = {"action": "run_workflow", "stage": stage, "workflow": workflow, "script": f"{run_arg}/.wf/{workflow}.js",
           "how": "用 Workflow 工具執行：scriptPath 給 script，不用傳 args（已經內嵌在腳本裡）。", "args_summary": {k: (v if isinstance(v, (str, int, bool)) else f"<{type(v).__name__}，{len(v)} 項>") for k, v in a.items()},
           "then": f"python3 .claude/teams/tools/awf.py done --run {run_arg}", "notes": notes + rnotes}
    prev = (data["stages"].get(stage) or {}).get("wf_run")
    if isinstance(prev, dict) and prev.get("id") and (data["stages"].get(stage) or {}).get("status") in ("blocked", "needs_decision", "invalid"):
        if prev.get("session") and prev.get("session") == current_session(project):
            act["resume_from"] = prev["id"]
            act["how"] += f" 這一段上次停在中途：加 resumeFromRunId=\"{prev['id']}\"，已完成而且派工單沒變的 agent 不會重跑。"
        else:
            act["notes"].append("上次的 workflow 是在別的 session 跑的，不能從中斷處續跑；這次整段重跑（plan 段會沿用來源沒變的調研）")
    return act


def halted_action(run: Path, run_arg: str, data: dict, stage: str, workflow: str, project: Path, plugin: Path | None) -> dict:
    res = _stage_result(run, stage)
    st = data["stages"].get(stage) or {}
    status = st.get("status")
    reason = res.get("halt_reason") or {"invalid": "這一段的結果沒通過 save_run 的檢查（看 done 印出的錯誤）", "needs_decision": "有 agent 在等人決定",
                                        "blocked": "有 agent 回報 blocked"}.get(status, str(status))
    act = {"action": "halted", "stage": stage, "status": status, "reason": reason, "advice": res.get("next") or "",
           "highlights": highlights(run, run_arg, data, stage),
           "say": "把原因與 open_questions 列給使用者；人答覆的決定用 save_run.py decide 記下來。處理完用 retry 的方式重跑這一段。"}
    try:
        act["retry"] = workflow_action(run, run_arg, data, stage, workflow, project, plugin, prepare=False)
        act["retry"]["how_to_prepare"] = f"python3 .claude/teams/tools/awf.py next --run {run_arg} --retry"
    except AwfError as e:
        act["retry_error"] = str(e)
    return act


def decide(run: Path, run_arg: str, data: dict, project: Path, plugin: Path | None, argv: list[str] = ()) -> dict:
    """下一個動作。只讀 run.json 與各段存下來的結果；要執行 workflow 時才寫 <run>/.wf/ 與資料包。"""
    argv = list(argv)
    if data.get("abandoned"):
        return {"action": "done", "state": "abandoned", "say": sr.abandoned_error(data)}
    if data.get("closed"):
        return {"action": "done", "state": "closed"}
    event = str(data.get("event") or "")
    retry = "--retry" in argv
    forced_wf = opt(argv, "--workflow")
    if event != "sdlc":
        if event not in EVENTS:
            return {"action": "manual", "say": f"事件 {event or '（未指定）'} 沒有對應的 workflow（方案辯論用 team-debate skill）；做完用 save_run.py stage 存結果。"}
        workflow, stage = EVENTS[event]
        st = data["stages"].get(stage) or {}
        if st.get("status") == "done":
            tip = "合併後跑 event-post-merge 監看。" if event == "merge" and _stage_result(run, stage).get("merged") else ""
            return {"action": "done", "state": "done", "highlights": highlights(run, run_arg, data, stage), "say": ("把結果整理給使用者。" + tip).strip()}
        if st.get("status") in ("blocked", "invalid", "needs_decision") and not retry:
            return halted_action(run, run_arg, data, stage, workflow, project, plugin)
        return workflow_action(run, run_arg, data, stage, forced_wf or workflow, project, plugin)
    stage = opt(argv, "--stage") or sr.next_stage(data)
    if stage is None:
        rec = rcp.recipe_of_run(data)
        tips = ["這個 run 的每一段都完成了。"]
        if "merge" in (rec.get("stages") or []) and _stage_result(run, "merge").get("merged"):
            tips.append("已合併：跑 event-post-merge 監看。")
        if rec.get("followup") and not data.get("followup_run"):
            tips.append("這個 recipe 走的是最短路徑：結案前要開後續 run 並用 save_run.py followup 記錄。")
        tips.append(f"結案：python3 .claude/teams/tools/save_run.py close {run_arg} --by <誰> --at {now_iso()}")
        return {"action": "done", "state": "done", "say": " ".join(tips)}
    if stage not in data["stages"] or stage not in sr.SDLC:
        raise AwfError(f"沒有 {stage} 這一段")
    st = data["stages"][stage]
    if st.get("status") == "skipped":
        raise AwfError(f"recipe 沒有 {stage} 段（要加段走 save_run.py escalate）")
    workflow = forced_wf or STAGE_WORKFLOW[stage]
    if forced_wf and forced_wf not in ([STAGE_WORKFLOW[stage]] + (["sdlc-design-revise"] if stage == "design" else [])):
        raise AwfError(f"{stage} 段不能用 {forced_wf}")
    if st.get("status") == "done" and st.get("gate") and not st.get("approved") and not forced_wf:
        return gate_action(run, run_arg, data, stage)
    chain = sr.chain_errors(data, stage)
    if chain:
        return {"action": "halted", "stage": stage, "status": "chain", "reason": "前面的段還沒過：" + "；".join(chain), "advice": "先完成並核准前面的段"}
    if st.get("status") in ("blocked", "invalid", "needs_decision") and not retry and not forced_wf:
        return halted_action(run, run_arg, data, stage, workflow, project, plugin)
    if stage == "merge" and not (data.get("args") or {}).get("pr"):
        g = load_json(project / ".claude" / "teams" / "guard.config.json") or {}
        auto = bool((g.get("auto_merge") or {}).get("enabled")) if isinstance(g.get("auto_merge"), dict) else False
        a, _, _ = stage_args(run, run_arg, data, "deliver", "sdlc-deliver", project)
        branch = a.get("branch") or (read_config(project).get("BRANCH_PREFIX", "") + str(a.get("epic") or ""))
        return {"action": "manual", "stage": "merge", "what": "推 fork、開 PR",
                "say": "推 fork、開 PR 要使用者當下同意（寫入上游要另外再同意一次）。同意後由主 session 跑 publish，再把 PR 號補上。"
                       + ("" if auto else " 注意：guard.config.json 的 auto_merge 沒有啟用，agent 不會合併；PR 開好後由人合併，合併後跑 save_run.py close。"),
                "publish": f'python3 .claude/teams/tools/publish.py push --repo-dir {a.get("repo", "<repo>")} --branch {branch} --run {run_arg} --title "<PR 標題>" --body-file <PR 說明檔>',
                "then": f"python3 .claude/teams/tools/awf.py next --run {run_arg} --set pr=<PR 號>" if auto else ""}
    return workflow_action(run, run_arg, data, stage, workflow, project, plugin)


# ------------------------------------------------------------------ 指令
def _load_run(run: Path) -> dict:
    if not (run / "run.json").is_file():
        raise AwfError(f"找不到 {run / 'run.json'}（先用 awf.py start 建立）")
    return sr.load(run)


def _quiet(fn, *a, stdin_text: str | None = None) -> tuple[int, str]:
    out = io.StringIO()
    old = sys.stdin
    if stdin_text is not None:
        sys.stdin = io.StringIO(stdin_text)
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = fn(*a)
    finally:
        sys.stdin = old
    return int(code or 0), out.getvalue()


def text_of(act: dict) -> str:
    L = [f"動作：{act.get('action')}" + (f"（{act.get('stage')} 段）" if act.get("stage") else "")]
    for k, label in (("workflow", "workflow"), ("script", "腳本"), ("how", "做法"), ("resume_from", "續跑"), ("then", "完成後"), ("reason", "原因"), ("advice", "建議"),
                     ("say", "說明"), ("approve", "核准"), ("publish", "推送")):
        if act.get(k):
            L.append(f"{label}：{act[k]}")
    for k, v in (act.get("need") or {}).items():
        L.append(f"缺 {k}：{v}")
    for n in act.get("notes") or []:
        L.append(f"- {n}")
    h = act.get("highlights") or {}
    for q in h.get("open_questions") or []:
        L.append(f"- {'[要先決定] ' if q.get('blocking') else ''}{q.get('question')}")
    for k in ("missing", "missing_records", "constraint_issues"):
        if h.get(k):
            L.append(f"- {k}：{h[k]}")
    return "\n".join(L)


def emit(act: dict, argv: list[str]) -> None:
    print(text_of(act) if "--text" in argv else json.dumps(act, ensure_ascii=False, indent=1))


def main(argv: list[str], project: Path | None = None, home: Path | None = None) -> int:
    _utf8()
    project = project or PROJECT
    if "--selftest" in argv:
        return selftest()
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0 if argv else 2
    cmd = argv[0]
    try:
        plugin, pnotes = (None, []) if cmd in ("approve", "start") else find_plugin(project, opt(argv, "--plugin-dir"), home)
        if cmd == "render":
            out = opt(argv, "--out")
            names = workflow_names(project, plugin) if "--all" in argv else [a for a in argv[1:2] if not a.startswith("-")]
            if not names or not out:
                raise AwfError("用法：awf.py render <workflow>|--all --out <目錄>")
            for n in names:
                text, notes = render(n, project, plugin)
                _write(Path(out) / f"{n}.js", text)
                for x in notes:
                    print(f"注意（{n}）：{x}", file=sys.stderr)
            print(f"已產生 {len(names)} 支 workflow 到 {out}")
            return 0
        run_arg = _posix(opt(argv, "--run")).rstrip("/")
        if not run_arg:
            raise AwfError("缺 --run <run 目錄>")
        run = Path(run_arg) if Path(run_arg).is_absolute() else project / run_arg
        if cmd == "start":
            event = opt(argv, "--event")
            if not event:
                raise AwfError("缺 --event")
            code, out = _quiet(sr.main, ["init", str(run), "--event", event, "--target", opt(argv, "--repo"), "--requirement", opt(argv, "--requirement")])
            if code:
                raise AwfError(out.strip())
            data = sr.load(run)
            for k in ("repo", "epic", "base", "recipe", "modifiers", "size"):
                if opt(argv, f"--{k}"):
                    data.setdefault("args", {})[k] = opt(argv, f"--{k}")
            errs = apply_sets(data, argv)
            if errs:
                raise AwfError("；".join(errs))
            sr.dump(run, data)
            print(f"已建立 {run_arg}/run.json。下一步：python3 .claude/teams/tools/awf.py next --run {run_arg}")
            return 0
        data = _load_run(run)
        if cmd == "next":
            errs = apply_sets(data, argv)
            if errs:
                raise AwfError("；".join(errs))
            act = decide(run, run_arg, data, project, plugin, argv)
            if act.get("action") == "run_workflow":
                act["notes"] = pnotes + act.get("notes", [])
                data.setdefault("awf", {})["pending"] = {"stage": act["stage"], "workflow": act["workflow"], "script": act["script"], "at": now_iso(),
                                                         "since": datetime.datetime.now().timestamp()}
            sr.dump(run, data)
            emit(act, argv)
            return 0
        if cmd == "done":
            pend = (data.get("awf") or {}).get("pending") or {}
            stage = opt(argv, "--stage") or pend.get("stage")
            if not stage:
                raise AwfError("不知道要存哪一段：先跑 awf.py next，或用 --stage 指定")
            meta, src = None, opt(argv, "--output")
            if src:
                result = load_json(Path(src))
                if result is None:
                    raise AwfError(f"{src} 讀不到或不是 JSON")
            else:
                script = pend.get("script") or ""
                meta, mf = find_wf_meta(project, str(project / script) if script and not Path(script).is_absolute() else script, float(pend.get("since") or 0))
                if meta is None:
                    raise AwfError("找不到這一段 workflow 的回傳（它還沒跑完，或這版 Claude Code 的存放位置變了）。把 Workflow 的回傳存成 JSON 檔，用 --output <檔> 指定")
                result = meta.get("result")
                if isinstance(result, str):
                    try:
                        result = json.loads(result)
                    except ValueError:
                        pass
                if not isinstance(result, (dict, list)):
                    raise AwfError(f"workflow 沒有回傳物件（status={meta.get('status')}）：{mf}。它可能還在跑，或中途失敗；失敗就重跑這一段")
            code, out = _quiet(sr.main, ["stage", str(run), stage], stdin_text=json.dumps(result, ensure_ascii=False))
            print(out.rstrip())
            data = sr.load(run)
            st = data["stages"].get(stage)
            if isinstance(st, dict):
                if meta is not None:
                    st["wf_run"] = {"id": str(meta.get("runId") or (mf.stem if mf else "")), "session": mf.parent.parent.name if mf else ""}
                else:
                    st.pop("wf_run", None)
            (data.get("awf") or {}).pop("pending", None)
            sr.dump(run, data)
            if "--no-next" not in argv:
                try:
                    act = decide(run, run_arg, data, project, plugin, [])
                    brief = {k: v for k, v in act.items() if k in ("action", "stage", "workflow", "reason", "advice", "say", "need")}
                    if act.get("action") == "run_workflow":   # 還沒備好腳本：要跑 next 才會產生
                        brief = {"action": "next", "stage": act["stage"], "say": f"下一段是 {act['stage']}：跑 python3 .claude/teams/tools/awf.py next --run {run_arg}"}
                    elif act.get("action") in ("gate", "halted"):
                        brief = act
                    print("下一步：")
                    emit(brief, argv)
                except AwfError as e:
                    print(f"下一步算不出來：{e}")
            return 1 if code else 0
        if cmd == "approve":
            given = [a for a in argv[1:] if not a.startswith("-")]
            skip = {opt(argv, n) for n in ("--run", "--by", "--note", "--answer-scope", "--answer-expires", "--profile")} | set(opts(argv, "--answer"))
            names = [a for a in given if a in data["stages"] and a not in skip]
            stage = names[0] if names else (sr.status_snapshot(run, data).get("waiting_gate") or "")
            if not stage:
                raise AwfError("現在沒有在等核准的段；要指定就寫 awf.py approve --run <run_dir> <段> --by <誰>")
            if not opt(argv, "--by"):
                raise AwfError("缺 --by <誰核准的>")
            passthru = []
            for n in ("--by", "--note", "--answer-scope", "--answer-expires", "--profile"):
                if opt(argv, n):
                    passthru += [n, opt(argv, n)]
            for a in opts(argv, "--answer"):
                passthru += ["--answer", a]
            code = sr.main(["approve", str(run), stage, "--at", now_iso(), *passthru])
            if not code:
                print(f"下一步：python3 .claude/teams/tools/awf.py next --run {run_arg}")
            return code
        raise AwfError(f"不認得的指令：{cmd}")
    except AwfError as e:
        print(f"awf：{e}", file=sys.stderr)
        return 1


# ------------------------------------------------------------------ selftest
# 佔位符在這裡用 <<KEY>> 寫，測試時才換成雙大括號：這支工具本身也會被 init.py 換佔位符，原始碼裡不能出現真的佔位符
FAKE_TEMPLATE = """export const meta = { name: '%s', description: 'x' }
const A = args || {}
const NS = '<<AGENT_NS>>'
const MP = /*@methodology*/ null /*@end*/
return { stage: 'x', lang: '<<OUTPUT_LANGUAGE>>', branch: '<<BRANCH_PREFIX>>' + A.epic }
""".replace("<<", "{" * 2).replace(">>", "}" * 2)


def selftest() -> int:
    ok = True
    n = 0

    def check(name, cond, detail=""):
        nonlocal ok, n
        n += 1
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{str(detail)[:600]}"))
        ok = ok and bool(cond)

    print("awf selftest（暫存目錄，不碰 repo）")
    saved_profile, saved_env = sr.PROJECT_PROFILE, {k: os.environ.get(k) for k in ("AWF_TRANSCRIPTS_DIR", "AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT")}
    with tempfile.TemporaryDirectory(prefix="awf-sm.") as tmp:
        try:
            root = Path(tmp)
            proj, plugin, home, tr = root / "proj", root / "plugin", root / "home", root / "transcripts"
            teams = proj / ".claude" / "teams"
            teams.mkdir(parents=True)
            (teams / "workflow.config").write_text('AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE="繁體中文"\nBRANCH_PREFIX=feature/\nFORK_REPO=\n', encoding="utf-8")
            default_profile = load_json(HERE.parent / "methodologies" / "default.json") or {}
            (teams / "methodology.json").write_text(json.dumps(default_profile, ensure_ascii=False), encoding="utf-8")
            (teams / "guard.config.json").write_text(json.dumps({"auto_merge": {"enabled": True}}), encoding="utf-8")
            (teams / "VERSION").write_text("9.9.9\n", encoding="utf-8")
            (plugin / TEMPLATES).mkdir(parents=True)
            (plugin / ".claude-plugin").mkdir()
            (plugin / ".claude-plugin" / "plugin.json").write_text(json.dumps({"version": "9.9.9"}), encoding="utf-8")
            for w in REQUIRED:
                (plugin / TEMPLATES / f"{w}.js").write_text(FAKE_TEMPLATE % w, encoding="utf-8")
            sr.PROJECT_PROFILE = teams / "methodology.json"
            os.environ["AWF_TRANSCRIPTS_DIR"] = str(tr)
            os.environ.pop("AWF_PLUGIN_DIR", None)
            os.environ.pop("CLAUDE_PLUGIN_ROOT", None)

            def call(*a) -> tuple[int, str]:
                return _quiet(main, [*a, "--plugin-dir", str(plugin)] if a[0] not in ("approve", "start") else list(a), proj, home)

            def act_of(out: str) -> dict:
                return json.loads(out[out.index("{"):])

            # ── 範本與產生
            text, notes = render("sdlc-plan", proj, plugin, {"epic": "demo", "run_dir": "_runs/r"})
            check("產生：佔位符換成 workflow.config 的值（去掉引號）、profile 內嵌、args 內嵌", "'agent-workflow:'" in text and "'繁體中文'" in text and not PH.search(text)
                  and '"name":"default"' in text and 'const A = Object.assign({"epic":"demo","run_dir":"_runs/r"}, args || {})' in text and notes == [], text[:400])
            plain, _ = render("sdlc-plan", proj, plugin)
            check("產生：不給 args 時保留原本那一行（和 init 產生的覆寫檔相同）", "\n" + ARGS_LINE + "\n" in plain)
            (teams / "workflow.config").write_text("AGENT_NS=agent-workflow:\n", encoding="utf-8")
            try:
                render("sdlc-plan", proj, plugin)
                bad = ""
            except AwfError as e:
                bad = str(e)
            check("產生：workflow.config 缺值時拒絕（不產生帶佔位符的腳本）", "BRANCH_PREFIX" in bad and "OUTPUT_LANGUAGE" in bad, bad)
            (teams / "workflow.config").write_text('AGENT_NS=agent-workflow:\nOUTPUT_LANGUAGE="繁體中文"\nBRANCH_PREFIX=feature/\n', encoding="utf-8")
            ovd = proj / ".claude" / "workflows"
            ovd.mkdir()
            (ovd / "sdlc-plan.js").write_text((FAKE_TEMPLATE % "sdlc-plan").replace("stage: 'x'", "stage: 'old-copy'"), encoding="utf-8")
            t2, n2 = render("sdlc-plan", proj, plugin)
            check("覆寫：沒有 awf-override 標記的同名檔不採用（用 plugin 的範本）並說明", "old-copy" not in t2 and any("不採用" in x for x in n2), n2)
            (ovd / "sdlc-plan.js").write_text((FAKE_TEMPLATE % "sdlc-plan").replace("stage: 'x'", "stage: 'mine'") + "// awf-override: 9.9.9\n", encoding="utf-8")
            t3, n3 = render("sdlc-plan", proj, plugin)
            check("覆寫：有標記的覆寫檔生效；profile 照樣內嵌", "stage: 'mine'" in t3 and '"name":"default"' in t3 and any("覆寫檔" in x for x in n3) and not any("plugin 現在是" in x for x in n3), n3)
            (ovd / "sdlc-plan.js").write_text((FAKE_TEMPLATE % "sdlc-plan") + "// awf-override: 0.1.0\n", encoding="utf-8")
            check("覆寫：版本和 plugin 不同會警告", any("plugin 現在是 9.9.9" in x for x in render("sdlc-plan", proj, plugin)[1]))
            (ovd / "sdlc-plan.js").unlink()
            (teams / "VERSION").write_text("0.4.1\n", encoding="utf-8")
            check("版本：plugin 和專案的工具版本不同會提醒跑 init.py --upgrade", any("init.py --upgrade" in x for x in render("sdlc-plan", proj, plugin)[1]))
            (teams / "VERSION").write_text("9.9.9\n", encoding="utf-8")
            (teams / "methodology.json").write_text(json.dumps({**default_profile, "extensions": [{"name": "x", "agent": "my-agent", "stage": "test", "instructions": "i"}]}, ensure_ascii=False), encoding="utf-8")
            try:
                render("sdlc-test", proj, plugin)
                bad = ""
            except AwfError as e:
                bad = str(e)
            check("產生：擴充 agent 沒登記守門角色就拒絕（和 build.py 一樣）", "my-agent" in bad, bad)
            (teams / "methodology.json").write_text(json.dumps(default_profile, ensure_ascii=False), encoding="utf-8")

            # ── 找 plugin
            (home / "plugins").mkdir(parents=True)
            other = root / "other-plugin"
            (other / TEMPLATES).mkdir(parents=True)
            (home / "plugins" / "installed_plugins.json").write_text(json.dumps({"version": 2, "plugins": {
                "agent-workflow@a": [{"installPath": str(plugin), "version": "9.9.9", "lastUpdated": "2026-01-01"}],
                "agent-workflow@b": [{"installPath": str(other), "version": "1.0.0", "lastUpdated": "2026-06-01"}],
                "agent-workflow@c": [{"installPath": str(other), "version": "9.9.9", "projectPath": str(root / "elsewhere")}],
                "something-else@a": [{"installPath": str(other)}]}}), encoding="utf-8")
            if HERE.parents[2].name != "scaffold":     # 在模版 repo 裡跑時會先用 repo 自己的 plugin，這一項只在已 init 的專案驗
                got, gn = find_plugin(proj, "", home)
                check("找 plugin：從 installed_plugins.json 找，版本和專案相同的優先，別的專案的安裝不算，多個來源會提醒", got == plugin and any("不只一個來源" in x for x in gn), (got, gn))
            else:
                cands = plugin_candidates(proj, home)
                check("找 plugin：installed_plugins.json 的 agent-workflow 安裝都列出來，別的專案的標出來", len(cands) == 3 and sum(c["other_project"] for c in cands) == 1, cands)
            check("找 plugin：登記檔壞掉或不存在當成沒有（不丟例外）", plugin_candidates(proj, root / "nope") == [])

            # ── 狀態機：每個 recipe 從 start 走到 done
            def walk(recipe: str, mods=(), fail_at: str = "") -> list[str]:
                d = f"_runs/{recipe}{'-' + '-'.join(mods) if mods else ''}{'-f' if fail_at else ''}"
                call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "做一件事")
                seq, run = [], proj / d
                for _ in range(40):
                    c, out = call("next", "--run", d)
                    if c:
                        seq.append("ERR:" + out.strip()[:120])
                        break
                    a = act_of(out)
                    seq.append(a["action"] + (":" + a["stage"] if a.get("stage") else ""))
                    data = sr.load(run)
                    if a["action"] == "run_workflow":
                        s = a["stage"]
                        if not (run / ".wf" / f"{a['workflow']}.js").is_file():
                            seq.append("ERR:no script")
                            break
                        if s == "plan":
                            res = rcp.resolve(default_profile, recipe, list(mods))
                            data["recipe"] = {"name": recipe, "modifiers": list(mods), "size": "", "resolved": res, "locked": False, "history": []}
                            sr.rebuild_stages(data, res)
                        bad_now = fail_at == s and not data["stages"][s].get("failed_once")
                        data["stages"][s].update({"status": "blocked" if bad_now else "done", "envelopes": 1, "failed_once": True})
                        (run / f"{s}.json").write_text(json.dumps({"stage": s, "halted": bad_now, "halt_reason": "缺件：x" if bad_now else None,
                                                                    "subtasks": [{"id": "t1"}] if s == "plan" else None, "merged": s == "merge"}), encoding="utf-8")
                        sr.dump(run, data)
                    elif a["action"] == "gate":
                        data["stages"][a["stage"]]["approved"] = {"by": "human", "at": "t"}
                        sr.dump(run, data)
                    elif a["action"] == "need_input":
                        call("next", "--run", d, *[x for k in a["need"] for x in ("--set", f"{k}=true")])
                        seq.pop()
                    elif a["action"] == "manual":
                        call("next", "--run", d, "--set", "pr=7")
                    elif a["action"] == "halted":
                        c2, o2 = call("next", "--run", d, "--retry")
                        a2 = act_of(o2)
                        seq.append(("retry:" + a2["stage"]) if a2["action"] == "run_workflow" else "ERR:retry")
                        data = sr.load(run)
                        data["stages"][a["stage"]]["status"] = "done"
                        (run / f"{a['stage']}.json").write_text(json.dumps({"stage": a["stage"], "subtasks": [{"id": "t1"}], "merged": a["stage"] == "merge"}), encoding="utf-8")
                        sr.dump(run, data)
                    else:
                        break
                return seq

            def expected(recipe: str, mods=()) -> list[str]:
                out = []
                for s in rcp.resolve(default_profile, recipe, list(mods))["stages"]:
                    if s == "merge":
                        out.append("manual:merge")
                    out.append(f"run_workflow:{s}")
                    if s in sr.GATED:
                        out.append(f"gate:{s}")
                return out + ["done"]

            names = sorted((default_profile.get("recipes") or {}))
            bad = {r: (walk(r), expected(r)) for r in names}
            bad = {r: v for r, v in bad.items() if v[0] != v[1]}
            check(f"狀態機：{len(names)} 個 recipe 從 start 走到 done，動作序列＝recipe 的段＋有閘門的段等核准＋merge 前先推送；沒有的段直接跳過", len(names) >= 11 and not bad, bad)
            check("狀態機：modifier 加回的段也照走（bugfix＋migration 有 design）", walk("bugfix", ("migration",)) == expected("bugfix", ("migration",)) and "run_workflow:design" in expected("bugfix", ("migration",)))
            seq = walk("bugfix", fail_at="build")
            check("狀態機：halted 的段先回 halted（附原因），--retry 才重跑，之後照常往下", seq[seq.index("run_workflow:build") + 1: seq.index("run_workflow:build") + 3] == ["halted:build", "retry:build"]
                  and seq[-1] == "done" and "run_workflow:sync" in seq, seq)

            # ── 參數
            d = "_runs/args"
            call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "需求", "--recipe", "bugfix", "--set", "base=main")
            c, out = call("next", "--run", d)
            a = act_of(out)
            emb = json.loads((proj / d / ".wf" / "sdlc-plan.args.json").read_text(encoding="utf-8"))
            check("參數：plan 段帶 repo、epic、requirement、run_dir、使用者指定的 recipe 與 --set 的值，並備好資料包", a["action"] == "run_workflow" and emb.get("repo") == "repos/app"
                  and emb.get("epic") == "demo" and emb.get("requirement") == "需求" and emb.get("run_dir") == (proj / d).resolve().as_posix() and emb.get("recipe") == "bugfix" and emb.get("base") == "main"
                  and emb.get("pack") == (proj / d).resolve().as_posix() + "/plan/pack.md" and (proj / d / "plan" / "pack.md").is_file(), emb)
            check("參數：腳本裡內嵌同一份 args，next 記下等待中的段", json.dumps(emb, ensure_ascii=False, separators=(",", ":")) in (proj / d / ".wf" / "sdlc-plan.js").read_text(encoding="utf-8")
                  and sr.load(proj / d)["awf"]["pending"]["stage"] == "plan")
            run = proj / d
            data = sr.load(run)
            res = rcp.resolve(default_profile, "feature", ["ui"])
            data["recipe"] = {"name": "feature", "modifiers": ["ui"], "size": "", "resolved": res, "locked": True, "history": []}
            sr.rebuild_stages(data, res)
            for s in ("plan", "design", "build", "sync"):
                data["stages"][s].update({"status": "done", "approved": {"by": "h", "at": "t"}})
            sr.dump(run, data)
            subs = [{"id": "t1", "title": "a", "kind": "code", "owns": ["a.py"], "depends_on": [], "acceptance": ["x"]}]
            (run / "design.json").write_text(json.dumps({"design": {"status": "done", "summary": "s", "payload": {"epic": "demo", "subtasks": subs}}, "sub_plans": [{"id": "api"}]}), encoding="utf-8")
            (run / "sync.json").write_text(json.dumps({"head_branch": "merge/demo-transit"}), encoding="utf-8")
            c, out = call("next", "--run", d, "--set", "recipe=docs")
            a = act_of(out)
            check("參數：test 段缺 has_security 時回 need_input（modifier ui 已經讓 has_ui=true）", a["action"] == "need_input" and list(a["need"]) == ["has_security"], a)
            c, out = call("next", "--run", d, "--set", "has_security=false")
            emb = json.loads((run / ".wf" / "sdlc-test.args.json").read_text(encoding="utf-8"))
            check("參數：test 段帶 has_ui、has_security、sub_plans、中轉分支；recipe 以 run.json 鎖定的為準（--set 改不了）", emb.get("has_ui") is True and emb.get("has_security") is False
                  and emb.get("sub_plans") == [{"id": "api"}] and emb.get("branch") == "merge/demo-transit" and emb.get("recipe") == "feature" and emb.get("modifiers") == ["ui"], emb)
            a_build, need, _ = stage_args(run, d, sr.load(run), "build", "sdlc-build", proj)
            check("參數：build 段的 subtasks 取自 design 的結果", a_build.get("subtasks") == subs and not need)
            (run / "build.json").write_text(json.dumps({"halted": True, "remaining_subtasks": [{"id": "t2"}], "integrated": ["t1"]}), encoding="utf-8")
            a_build, _, _ = stage_args(run, d, sr.load(run), "build", "sdlc-build", proj)
            check("參數：build halted 後續跑只帶 remaining_subtasks 與 integrated", a_build.get("subtasks") == [{"id": "t2"}] and a_build.get("integrated") == ["t1"])
            a_merge, need, _ = stage_args(run, d, sr.load(run), "merge", "sdlc-merge", proj)
            check("參數：merge 段不帶 branch，缺 pr", "branch" not in a_merge and need == ["pr"])
            check("參數：--set 寫錯、想改 run_dir 都拒絕", call("next", "--run", d, "--set", "沒有等號")[0] != 0 and call("next", "--run", d, "--set", "run_dir=/tmp/x")[0] != 0)
            c, out = call("next", "--run", d, "--stage", "design", "--workflow", "sdlc-design-revise")
            a = act_of(out)
            check("退回 design：--workflow sdlc-design-revise 缺 decisions、issues 時回 need_input；別段不能用它", a["action"] == "need_input" and set(a["need"]) == {"decisions", "issues"}
                  and call("next", "--run", d, "--stage", "test", "--workflow", "sdlc-design-revise")[0] != 0, a)

            # ── 閘門重點
            d = "_runs/gate"
            call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "需求")
            run = proj / d
            data = sr.load(run)
            data["stages"]["plan"].update({"status": "done", "envelopes": 1, "missing_records": ["research:x"]})
            sr.dump(run, data)
            env = {"status": "done", "summary": "調研完成", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {},
                   "open_questions": [{"question": "編碼用什麼？", "blocking": True, "options": [{"choice": "UTF-8", "cost": "無"}], "recommendation": "UTF-8", "default_applied": ""}]}
            (run / "plan.json").write_text(json.dumps({"stage": "plan", "missing": ["research:y"], "research": [env], "reused": ["specs-adr"], "trace": [{"label": "x"}]}), encoding="utf-8")
            a = act_of(call("next", "--run", d)[1])
            h = a.get("highlights") or {}
            check("閘門：plan 等人核准，重點列出各 agent 的摘要、所有 open_questions、缺件、沿用的調研、紀錄缺件", a["action"] == "gate" and a["human_only"] is True
                  and h["agents"][0]["summary"] == "調研完成" and h["open_questions"][0]["blocking"] is True and h["open_questions"][0]["options"] == ["UTF-8"]
                  and h["missing"] == ["research:y"] and h["reused"] == ["specs-adr"] and h["missing_records"] == ["research:x"], a)
            check("閘門：--text 給人看的摘要有動作、核准指令與要先決定的問題", "動作：gate（plan 段）" in call("next", "--run", d, "--text")[1] and "[要先決定] 編碼用什麼？" in call("next", "--run", d, "--text")[1])
            c, out = call("approve", "--run", d, "--by", "human", "--answer", "編碼用什麼？=UTF-8")
            st = sr.load(run)["stages"]["plan"]
            check("approve：不指定段時核准正在等的那一段、時間自動取、--answer 記進決策記憶", c == 0 and st.get("approved", {}).get("by") == "human"
                  and re.match(r"^\d{4}-\d\d-\d\dT", st["approved"]["at"]) and packlib.load_decisions(run.parent)[-1]["answer"] == "UTF-8", out)
            data = sr.load(run)
            data["stages"]["design"].update({"status": "done", "envelopes": 0})
            sr.dump(run, data)
            c, out = call("approve", "--run", d, "--by", "human")
            check("approve：核准仍由 save_run 把關（沒有信封的段照樣拒絕，狀態機不放寬閘門）", c != 0 and "沒有任何信封" in out and not sr.load(run)["stages"]["design"].get("approved"), out)
            check("approve：沒有在等核准的段、缺 --by 都拒絕", call("approve", "--run", "_runs/args", "--by", "x")[0] != 0 and call("approve", "--run", d)[0] != 0)

            # ── done：自己找 workflow 的回傳
            d = "_runs/done"
            call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "需求")
            run = proj / d
            call("next", "--run", d)
            sess = tr / "sess-1" / "workflows"
            sess.mkdir(parents=True)
            (tr / "sess-1.jsonl").write_text("{}\n", encoding="utf-8")
            result = {"stage": "plan", "halted": True, "halt_reason": "缺件：intent", "trace": [], "missing": ["intent"]}
            (sess / "wf_old.json").write_text(json.dumps({"runId": "wf_old", "scriptPath": str(run / ".wf" / "sdlc-design.js"), "status": "completed", "result": {"stage": "design"}}), encoding="utf-8")
            (sess / "wf_new.json").write_text(json.dumps({"runId": "wf_new", "scriptPath": str(run / ".wf" / "sdlc-plan.js"), "status": "completed", "result": result}), encoding="utf-8")
            c, out = call("done", "--run", d)
            data = sr.load(run)
            check("done：找到用這支腳本跑的 workflow 回傳，交給 save_run stage 存檔（內容原樣），記下 workflow 的 run id，清掉等待中的段",
                  load_json(run / "plan.json") == result and data["stages"]["plan"]["wf_run"] == {"id": "wf_new", "session": "sess-1"} and "pending" not in data.get("awf", {})
                  and data["stages"]["plan"]["status"] in ("blocked", "invalid"), out)
            check("done：印出下一步（這一段 halted → 原因與建議）", "下一步：" in out and '"action": "halted"' in out and "缺件：intent" in out, out)
            a = act_of(call("next", "--run", d, "--retry")[1])
            check("續跑：同一個 session 裡 halted 後重跑，帶 resume_from（＝resumeFromRunId）", a["action"] == "run_workflow" and a.get("resume_from") == "wf_new" and "resumeFromRunId" in a["how"], a)
            (tr / "sess-2.jsonl").write_text("{}\n", encoding="utf-8")
            os.utime(tr / "sess-2.jsonl", (2e9, 2e9))
            a = act_of(call("next", "--run", d, "--retry")[1])
            check("續跑：換了 session 就不帶，並說明整段重跑", "resume_from" not in a and any("別的 session" in x for x in a["notes"]), a)
            out_file = root / "out.json"
            out_file.write_text(json.dumps(result), encoding="utf-8")
            d2 = "_runs/done2"
            call("start", "--run", d2, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "需求")
            _quiet(sr.main, ["stage", str(proj / d2), "plan"], stdin_text=json.dumps(result))
            manual = (proj / d2 / "plan.json").read_text(encoding="utf-8")
            check("done：存下來的內容和手動跑 save_run.py stage 的一樣；--output 指定檔案也可以", (run / "plan.json").read_text(encoding="utf-8") == manual
                  and call("done", "--run", d, "--output", str(out_file), "--stage", "plan")[0] in (0, 1) and (run / "plan.json").read_text(encoding="utf-8") == manual)
            d3 = "_runs/done3"
            call("start", "--run", d3, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "需求")
            call("next", "--run", d3)
            c, out = call("done", "--run", d3)
            check("done：找不到回傳時明說，並提示用 --output", c != 0 and "--output" in out, out)
            check("done：沒跑過 next 又沒指定段時拒絕", call("done", "--run", d2)[0] != 0)

            # ── 事件與結束狀態
            d = "_runs/ev"
            call("start", "--run", d, "--event", "meeting", "--set", "source=meetings/a.md")
            a = act_of(call("next", "--run", d)[1])
            check("事件：meeting 用 event-meeting，帶 source 與 run_dir；缺參數時回 need_input", a["action"] == "run_workflow" and a["workflow"] == "event-meeting" and a["stage"] == "meeting"
                  and json.loads((proj / d / ".wf" / "event-meeting.args.json").read_text(encoding="utf-8")) == {"source": "meetings/a.md", "run_dir": (proj / d).resolve().as_posix()}, a)
            call("start", "--run", "_runs/ev2", "--event", "upstream")
            a = act_of(call("next", "--run", "_runs/ev2")[1])
            check("事件：缺必要參數回 need_input 並說明怎麼補", a["action"] == "need_input" and set(a["need"]) == {"what", "repo"} and "--set what=" in a["how"], a)
            call("start", "--run", "_runs/ev3", "--event", "debate")
            check("事件：沒有對應 workflow 的事件回 manual", act_of(call("next", "--run", "_runs/ev3")[1])["action"] == "manual")
            data = sr.load(proj / d)
            data["abandoned"] = {"by": "h", "at": "t", "reason": "取消"}
            sr.dump(proj / d, data)
            check("已取消的 run：next 回 done（abandoned），不再派工", act_of(call("next", "--run", d)[1]) .get("state") == "abandoned")
            check("找不到 run.json、缺 --run、不認得的指令都回錯誤", call("next", "--run", "_runs/nope")[0] != 0 and call("next")[0] != 0 and call("nope", "--run", d)[0] != 0)

            # ── render --all
            outd = root / "rendered"
            c, out = call("render", "--all", "--out", str(outd))
            check("render --all：每支範本都產生，沒有殘留佔位符", c == 0 and len(list(outd.glob("*.js"))) == len(REQUIRED) and not any(PH.search(p.read_text(encoding="utf-8")) for p in outd.glob("*.js")), out)
            real = HERE.parents[3] / "plugin" if len(HERE.parents) > 3 else None
            if real is not None and (real / TEMPLATES).is_dir() and HERE.parents[2].name == "scaffold":
                names = sorted(p.stem for p in (real / TEMPLATES).glob("*.js"))
                miss = [w for w in names if w not in REQUIRED] + [w for w in REQUIRED if w not in names]
                bad = [w for w in names if (real / TEMPLATES / f"{w}.js").read_text(encoding="utf-8").count("\n" + ARGS_LINE + "\n") != 1]
                check("真的範本：每一支都在 REQUIRED 裡、都有唯一的一行可以內嵌 args", not miss and not bad, (miss, bad))
        finally:
            sr.PROJECT_PROFILE = saved_profile
            for k, v in saved_env.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
    print(f"awf selftest：{'全部通過' if ok else '有失敗'}（{n} 項）")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
