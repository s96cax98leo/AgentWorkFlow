#!/usr/bin/env python3
"""狀態機：主 session 每一段只做兩件事——`awf.py next`（照它說的做）與 `awf.py done`（把結果存起來）。

用法（在已 init 的專案根目錄）：
    python3 .claude/teams/tools/awf.py start --run <run_dir> --event <sdlc|brief|upstream|…> [--repo <repo>] [--epic <短名>]
        [--requirement <需求原文或路徑>] [--base <base>] [--recipe <名>] [--modifiers a,b] [--size s] [--set k=v]… [--set-json k=<JSON>]…
    python3 .claude/teams/tools/awf.py next --run <run_dir> [--set k=v]… [--set-json k=<JSON>]… [--stage <段>] [--workflow <名>] [--text]
    python3 .claude/teams/tools/awf.py next --run <run_dir> --script <自己寫的腳本.js> [--stage <段> --retry]   # design、build、deliver 段改用自訂腳本（先檢查再接上執行核心；見 awf_script.py）
    python3 .claude/teams/tools/awf.py done --run <run_dir> [--output <workflow 回傳的 JSON 檔>] [--stage <段>]
    python3 .claude/teams/tools/awf.py approve --run <run_dir> [<段>] --by <誰> [--note <備註>] [--answer "<問題>=<答覆>"]… [--accept-defaults]
    python3 .claude/teams/tools/awf.py handoff --run <run_dir> [--print]      # 寫 <run_dir>/接手摘要.md：換 session 或換人接手時看
    python3 .claude/teams/tools/awf.py watch --run <run_dir> [--once [--json]] [--interval 5] [--stall-seconds 900]   # 進度事件寫到 <run_dir>/progress.jsonl（見 awf_watch.py）
    python3 .claude/teams/tools/awf.py statusline                             # 一行狀態，給 Claude Code 的 statusLine 用
    python3 .claude/teams/tools/awf.py doctor [--json]                        # 檢查安裝：環境、plugin 與專案版本、安裝來源衝突、hook、設定（見 awf_doctor.py）
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
plan 段的核准一律由人。recipe 的 gate_mode 是 plan-only（內建：docs、test-only；帶任何 modifier 就不算）時，非 plan 的閘門先試政策核准
（save_run.py approve --by policy，條件完全由它判）；不乾淨就照常交人。其他 recipe（strict）每個閘門都等人。只用標準庫。
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
import awf_script  # noqa: E402
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
# event.js 內建的事件類型與各自的必要參數（要和範本的 BUILTIN_EVENTS 一致，selftest 會對照）。
# 這五種形狀相同（多角度 → skeptic → writer），共用一支 event.js；使用者在 profile 的 events 加一筆就多一種。
EVENT_TYPES = {"meeting": ["source"], "decision": ["decision"], "demo-check": ["target"], "reqs-change": ["change"], "upstream": ["what", "repo"]}
EVENTS = {
    "brief": ("event-brief", "brief"), "pr-review": ("event-pr-review", "pr-review"),
    "post-merge": ("event-post-merge", "post-merge"), "merge": ("sdlc-merge", "merge"),
    **{t: ("event", t) for t in EVENT_TYPES},
}
REQUIRED = {
    "sdlc-plan": ["repo", "epic", "requirement", "run_dir"], "sdlc-design": ["repo", "epic", "run_dir"],
    "sdlc-design-revise": ["repo", "epic", "run_dir", "decisions", "issues"], "sdlc-build": ["repo", "epic", "run_dir", "subtasks"],
    "sdlc-sync": ["repo", "epic", "run_dir"], "sdlc-test": ["repo", "epic", "run_dir"], "sdlc-deliver": ["repo", "epic", "run_dir"],
    "sdlc-merge": ["repo", "run_dir", "pr"], "event-brief": ["brief", "run_dir"], "event-post-merge": ["repo", "run_dir"],
    "event-pr-review": ["target", "repo", "run_dir"], "event": ["event", "run_dir"],     # event.js 另外要該事件類型的參數（event_required）
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
MERGED_HINT = "0.5 起會議、決策落地、demo 前檢查、需求變動、上游變動共用 event.js（事件類型由 awf.py start --event <類型> 決定）。"


class AwfError(Exception):
    pass


def profile_events(project: Path) -> dict:
    """profile（methodology.json）的 events：使用者自訂的事件類型。讀不懂就當沒有。"""
    m = load_json(project / ".claude" / "teams" / "methodology.json")
    ev = m.get("events") if isinstance(m, dict) else None
    return {k: v for k, v in ev.items() if isinstance(v, dict)} if isinstance(ev, dict) else {}


def event_route(project: Path, event: str) -> tuple[str, str] | None:
    """事件 →（workflow, 段名）。固定的事件與內建類型查表；其餘看 profile 的 events（走 event.js）。"""
    if event in EVENTS and event not in profile_events(project):
        return EVENTS[event]
    if event in profile_events(project) and (event in EVENT_TYPES or event not in EVENTS):   # 自訂類型不能蓋掉 brief、merge 這些不是 event.js 的事件
        return ("event", event)
    return EVENTS.get(event)


def event_required(project: Path, event: str) -> list[str]:
    """event.js 這個事件類型的必要參數：profile 有定義就照它的 args，否則用內建的。"""
    d = profile_events(project).get(event)
    if d is not None:
        return [str(x) for x in d.get("args") or [] if isinstance(x, str)]
    return list(EVENT_TYPES.get(event, []))


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
        if tpl is None:
            notes.append(f"找不到 plugin 的範本，改用專案裡的 .claude/workflows/{name}.js（舊副本）；請確認 plugin 已安裝，或用 --plugin-dir 指定")
            return text, "legacy", notes
        if not tpl.is_file():     # plugin 在、但這版已經沒有這支（例如 0.5 併進 event.js 的五支）：舊副本不採用，免得跑到舊流程
            raise AwfError(f"plugin 這一版沒有 {name}.js；專案裡的 .claude/workflows/{name}.js 是沒有 awf-override 標記的舊副本，不採用。"
                           + MERGED_HINT + "要用自己的版本，請依現在的範本改寫並在結尾加上 awf-override 標記")
        notes.append(f".claude/workflows/{name}.js 沒有 awf-override 標記（舊版 init 複製的副本），不採用，改用 plugin 的範本；確認沒在用就可以刪掉")
    if tpl is None or not tpl.is_file():
        raise AwfError(f"找不到 workflow 範本 {name}.js：" + ("plugin 沒有這支。" + MERGED_HINT if plugin else "找不到 agent-workflow plugin（先安裝 plugin，或用 --plugin-dir 指定）"))
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


def custom_roles(project: Path) -> list[str]:
    """自訂腳本可以派的 agent_type：模版角色（不含 dev-merge-steward）加上 guard.config.json 登記過的擴充 agent。"""
    ns = read_config(project).get("AGENT_NS", PLUGIN_NAME + ":")
    g = load_json(project / ".claude" / "teams" / "guard.config.json") or {}
    ext = g.get("extension_agents") if isinstance(g.get("extension_agents"), dict) else {}
    return [ns + r for r in awf_script.ROLES] + sorted(str(k) for k in ext)


def render_custom(src: Path, stage: str, project: Path, args: dict) -> tuple[str, dict]:
    """檢查並組裝自訂腳本：（可以執行的腳本, {source, sha1}）。檢查不過丟 AwfError，不產生任何東西。"""
    if stage not in awf_script.STAGES:
        raise AwfError(f"{stage} 段不能用自訂腳本（只開放 {'、'.join(awf_script.STAGES)}）：plan 的調研重用、sync 的衝突核對、test 的分片核對都寫在標準腳本裡，merge 是信任邊界")
    try:
        text = src.read_text(encoding="utf-8").replace("\r\n", "\n")
    except (OSError, UnicodeDecodeError) as e:
        raise AwfError(f"讀不到自訂腳本 {src}：{e}")
    cfg = read_config(project)
    ns = cfg.get("AGENT_NS", PLUGIN_NAME + ":")
    cache: dict = {}
    try:
        snippets = {n: buildlib.load_snippet(n, cache) for n in awf_script.SNIPPETS}
    except buildlib.SnippetError as e:
        raise AwfError(str(e))
    roles = custom_roles(project)
    errs = awf_script.check(text, set(roles), ns, awf_script.defined_names(snippets))
    if errs:
        raise AwfError(f"自訂腳本 {src.name} 沒通過檢查，不執行：\n  - " + "\n  - ".join(errs[:20]))
    meth_file = project / ".claude" / "teams" / "methodology.json"
    methodology = load_json(meth_file)
    if meth_file.is_file() and not isinstance(methodology, dict):
        raise AwfError(f"{meth_file} 讀不懂（不是 JSON 物件）")
    if _unregistered(methodology, project):
        raise AwfError("methodology.json 的 extensions 有 agent 沒登記在 guard.config.json 的 extension_agents（用 /agent-workflow:add-agent 設定）")
    limits = (methodology or {}).get("limits") if isinstance((methodology or {}).get("limits"), dict) else {}
    cap = limits.get("max_agents") if isinstance(limits.get("max_agents"), int) and limits.get("max_agents") > 0 else 40
    js = lambda v: json.dumps(v, ensure_ascii=False, separators=(",", ":"))  # noqa: E731
    S = js(stage)
    pre = [f"// ── 以下由 awf.py 接上（自訂腳本，{stage} 段）：執行核心、pattern 庫、這一段的參數。不要手改 ──",
           "const NS = " + js(ns),
           "const A = Object.assign(" + js(args) + ", args || {})",
           "const METHODOLOGY = " + js(methodology),
           "const MP = METHODOLOGY || {}"]
    pre += [f"// ── 片段 {n} ──\n{snippets[n]}" for n in awf_script.SNIPPETS]
    pre += ["const R = resolveRecipe(METHODOLOGY, A.recipe, [].concat(A.modifiers || []), A.size)",
            "if (R.errors.length) return { stage: " + S + ", halted: true, halt_reason: 'recipe 解析有錯：' + R.errors.join('；'), missing: [], "
            "recipe: { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages }, trace: [] }",
            "const RECIPE = { name: R.name, requested: R.requested, modifiers: R.modifiers, size: R.size, stages: R.stages, summary: recipeSummary(R) }",
            "awfInit({ run_dir: A.run_dir, stage: " + S + ", lang: " + js(cfg.get("OUTPUT_LANGUAGE", "")) + ", rules: '.claude/teams/rules.md', constraints: awfConstraints(MP, " + S + ") })",
            "awfKernelInit({ profile: METHODOLOGY, stage: " + S + ", modifiers: [].concat(A.modifiers || []), max_agents: " + str(cap) + ", roles: " + js(roles) + " })"]
    out = awf_script.assemble(text, "\n".join(pre))
    out = buildlib.MARK.sub(lambda m: f"/*@schema:{m.group(1)}*/ {js(buildlib.schemalib.compose(None if m.group(1) == 'envelope' else m.group(1)))} /*@end*/", out)
    import hashlib
    return out, {"source": _posix(src.name), "sha1": hashlib.sha1(text.encode("utf-8")).hexdigest()}


def custom_action(run: Path, run_arg: str, data: dict, project: Path, argv: list[str]) -> dict:
    """awf.py next --script <檔>：用自訂腳本跑下一段（或 --stage 指定的段）。排序與閘門鏈照舊，只換腳本。"""
    if str(data.get("event") or "") != "sdlc":
        raise AwfError("自訂腳本只用在 SDLC 的 run")
    if data.get("abandoned") or data.get("closed"):
        raise AwfError("這個 run 已經結束")
    stage = opt(argv, "--stage") or sr.next_stage(data)
    if stage is None or stage not in data["stages"]:
        raise AwfError("沒有要跑的段")
    if stage not in awf_script.STAGES:
        render_custom(Path(opt(argv, "--script")), stage, project, {})      # 丟出「這一段不能用自訂腳本」
    st = data["stages"][stage]
    if st.get("status") == "skipped":
        raise AwfError(f"recipe 沒有 {stage} 段（要加段走 save_run.py escalate）")
    if st.get("status") == "done" and st.get("gate") and not st.get("approved") and "--retry" not in argv:
        raise AwfError(f"{stage} 段已經有結果、在等核准；要用自訂腳本重跑這一段就加 --stage {stage} --retry")
    chain = sr.chain_errors(data, stage)
    if chain:
        raise AwfError("前面的段還沒過：" + "；".join(chain))
    src = Path(opt(argv, "--script"))
    src = src if src.is_absolute() else project / src
    a, need, notes = stage_args(run, run_arg, data, stage, STAGE_WORKFLOW.get(stage, ""), project)
    if need:
        return {"action": "need_input", "stage": stage, "workflow": f"custom-{stage}", "need": {k: HINTS.get(k, "這一段需要的參數") for k in need},
                "how": f"python3 .claude/teams/tools/awf.py next --run {run_arg} --script {opt(argv, '--script')} " + " ".join(f"--set {k}=<值>" for k in need)}
    extra, pnotes = run_pack(stage, run_arg, a, run, project)
    a.update(extra)
    text, custom = render_custom(src, stage, project, a)
    name = f"custom-{stage}"
    _write(run / ".wf" / f"{name}.js", text)
    _write(run / ".wf" / f"{name}.args.json", json.dumps(a, ensure_ascii=False, indent=1) + "\n")
    return {"action": "run_workflow", "stage": stage, "workflow": name, "script": f"{run_arg}/.wf/{name}.js", "custom": custom,
            "how": "用 Workflow 工具執行：scriptPath 給 script，不用傳 args（已經內嵌在腳本裡）。",
            "then": f"python3 .claude/teams/tools/awf.py done --run {run_arg}",
            "notes": notes + pnotes + [f"這一段用自訂腳本 {custom['source']} 跑：存檔時的檢查和標準腳本一樣（必要的角色、紀錄、模型下限、recipe），而且不能政策核准，一律由人核准"]}


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
    required = list(REQUIRED.get(workflow, ["run_dir"]))
    if workflow == "event":
        a["event"] = stage      # 事件類型就是段名（執行紀錄寫在 <run_dir>/<類型>/）
        required += [k for k in event_required(project, stage) if k not in required]
    need = [k for k in required if a.get(k) in (None, "", [])]
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
           "recommendation": q.get("recommendation"), "default_applied": _applied(q)} for w, e in envs for q in e.get("open_questions") or [] if isinstance(q, dict)]
    if qs:
        # 要先決定的排前面；agent 已照建議先做的（default_applied）另外列成 provisional，讓人一次確認
        h["open_questions"] = sorted(qs, key=lambda q: not q["blocking"])[:30]
        prov = provisional(res)
        if prov:
            h["provisional"] = prov[:30]
    for k in ("missing", "refuted", "reused", "models", "gate", "halt_reason", "next", "located_by", "test_first", "sub_plans", "custom_script"):
        if res.get(k) not in (None, [], {}, ""):
            h[k] = res[k]
    if isinstance(res.get("angles"), dict) and res["angles"].get("skipped"):
        h["angles_skipped"] = res["angles"]["skipped"]
    if stage == "plan" and sr.has_recipe(data):
        r = data["recipe"]
        h["recipe"] = {"name": r.get("name"), "modifiers": r.get("modifiers") or [], "size": r.get("size") or "", "stages": (r.get("resolved") or {}).get("stages"),
                       "reasons": r.get("reasons") or []}
    for k in ("missing_records", "ghostwritten_records", "constraint_issues", "plan_check"):
        if st.get(k):
            h[k] = st[k]
    idx = run / stage / sr.INDEX_NAME
    if idx.is_file():
        h["records_index"] = f"{run_arg}/{stage}/{sr.INDEX_NAME}"
    return h


def _applied(q: dict) -> str:
    v = str(q.get("default_applied") or "").strip()
    return "" if v.lower() in ("", "none", "無", "n/a") else v


def provisional(res: dict) -> list[dict]:
    """不是 blocking、agent 已先照預設做了的問題：[{question, applied}]（同一題只留一筆）。"""
    out, seen = [], set()
    for _w, e in _envelopes(res):
        for q in e.get("open_questions") or []:
            if isinstance(q, dict) and not q.get("blocking") and _applied(q) and str(q.get("question") or "").strip() and q["question"] not in seen:
                seen.add(q["question"])
                out.append({"question": str(q["question"]).strip(), "applied": _applied(q)})
    return out


def try_policy(run: Path, data: dict, stage: str) -> list[str] | None:
    """gate_mode=plan-only 的 recipe：非 plan 的閘門先試政策核准（save_run approve --by policy，條件完全由它判）。
    過了回 None（data 已更新成核准後的 run.json）；不適用回 []；被拒回理由清單（交人）。plan 段永遠不試。"""
    if stage in sr.HUMAN_ONLY or not sr.has_recipe(data):
        return []
    prof, _err = sr.load_profile_for(run, [])
    gm = rcp.gate_mode(prof, data.get("recipe"))
    if gm["mode"] != "plan-only":
        return []
    prov = provisional(_stage_result(run, stage))
    code, out = _quiet(sr.main, ["approve", str(run), stage, "--by", "policy", "--at", now_iso(), "--note", f"gate_mode=plan-only（{gm['reason']}）"])
    if code:
        return [ln.replace("不能核准：", "").strip() for ln in out.splitlines() if ln.strip()][:12] or ["政策核准沒過"]
    fresh = sr.load(run)
    aw = fresh.setdefault("awf", {})
    for k, v in (data.get("awf") or {}).items():      # 這次 next 還沒存檔的狀態（例如 --set 的值在 args，不在這裡）
        aw.setdefault(k, v)
    aw.setdefault("auto_approved", []).append({"stage": stage, "at": now_iso(), "by": "policy"})
    if prov:                                            # agent 先照建議做的暫定：政策核准沒有人看過，留到下一次要人出面時一起確認
        aw.setdefault("unconfirmed", []).extend({"stage": stage, **q} for q in prov)
    fresh["args"] = data.get("args") or fresh.get("args") or {}
    sr.dump(run, fresh)
    data.clear()
    data.update(fresh)
    return None


def carry(act: dict, data: dict) -> dict:
    """政策核准過的段與還沒有人確認的暫定做法：下一次要人出面的動作（gate、manual、halted、done）一併列出。"""
    aw = data.get("awf") or {}
    if act.get("action") in ("gate", "manual", "halted", "done"):
        if aw.get("auto_approved"):
            act["auto_approved"] = [x["stage"] for x in aw["auto_approved"]]
        if aw.get("unconfirmed"):
            act["unconfirmed"] = aw["unconfirmed"]
            act["say"] = (str(act.get("say") or "") + " unconfirmed 是政策核准的段裡 agent 先照建議做的暫定，還沒有人看過：一併列給使用者確認，"
                          "確認後用 save_run.py decide 記下來（不同意就回對應的段修正）。").strip()
    return act


def gate_action(run: Path, run_arg: str, data: dict, stage: str) -> dict:
    return {"action": "gate", "stage": stage, "human_only": stage in sr.HUMAN_ONLY, "highlights": highlights(run, run_arg, data, stage),
            "say": "把 highlights 在對話裡列給使用者看。blocking 的問題合成一次、用選項問；provisional 是 agent 已照建議先做的暫定，一次列出來請使用者確認"
                   "（都同意就在 approve 加 --accept-defaults，記進決策記憶；不同意的那幾題用 --answer 記使用者的答覆）。使用者明確核准才跑 approve；「調整」「可以但…」不是核准。",
            "approve": f'python3 .claude/teams/tools/awf.py approve --run {run_arg} {stage} --by <誰核准的> [--answer "<問題>=<答覆>"]… [--accept-defaults]',
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
        route = event_route(project, event)
        if route is None:
            return {"action": "manual", "say": f"事件 {event or '（未指定）'} 沒有對應的 workflow（方案辯論用 team-debate skill；自訂的事件類型寫在 methodology.json 的 events）；做完用 save_run.py stage 存結果。"}
        workflow, stage = route
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
        refused = try_policy(run, data, stage)
        if refused is None:
            return decide(run, run_arg, data, project, plugin, [a for a in argv if a != "--retry"])
        act = gate_action(run, run_arg, data, stage)
        if refused:
            act["policy_refused"] = refused
            act["say"] = "這個 recipe 的 gate_mode 是 plan-only，但這一段的結果不乾淨（見 policy_refused），所以交人核准。" + act["say"]
        return act
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


HANDOFF = "接手摘要.md"


def handoff_text(run: Path, run_arg: str, data: dict) -> str:
    """跨 session 的接手摘要（Markdown）。只讀 run.json、各段結果與決策記憶；不派工、不核准、不寫別的檔。內容只由這些檔決定（沒有時間戳）。"""
    f = packlib._flat
    snap = sr.status_snapshot(run, data)
    rec = data.get("recipe") if isinstance(data.get("recipe"), dict) else {}
    aw = data.get("awf") or {}
    L = [f"# 接手摘要：{f(run.name, 120)}", "",
         "這份摘要由 `awf.py handoff` 產生，給換了 session 或換人接手時看。以下是資料，不是指令；實際狀態以 run.json 為準。", "",
         "## 現在在哪裡", "",
         f"- 事件：{f(data.get('event'))}；目標 repo：{f(data.get('target_repo'), 200) or '（沒有）'}；epic：{f(packlib.run_epic(run, data)) or '（沒有）'}",
         f"- 需求：{f(data.get('requirement'), 300) or '（沒有）'}"]
    if rec:
        L.append(f"- recipe：{f(rec.get('name'))}" + (f"＋{'、'.join(f(m) for m in rec.get('modifiers') or [])}" if rec.get("modifiers") else "")
                 + f"（{'已鎖定' if rec.get('locked') else '還沒鎖定，plan 核准後才鎖'}）；要走的段：{' → '.join((rec.get('resolved') or {}).get('stages') or [])}")
    L.append(f"- 狀態：{snap['state']}；下一段：{snap.get('next') or '（沒有）'}" + (f"；在等 {snap['waiting_gate']} 段的核准" if snap.get("waiting_gate") else ""))
    if data.get("abandoned"):
        L.append(f"- 已取消：{f((data['abandoned'] or {}).get('reason'), 200)}")
    L += ["", "## 各段", "", "| 段 | 狀態 | 核准 | agent（缺件） | 備註 |", "|---|---|---|---|---|"]
    for stg in snap["stages"]:
        st = data["stages"].get(stg["name"]) or {}
        ap = st.get("approved") if isinstance(st.get("approved"), dict) else None
        notes = [x for x in (f"halted：{f(st.get('halted'), 120)}" if st.get("halted") else "",
                             f"紀錄缺 {len(st['missing_records'])} 份" if st.get("missing_records") else "",
                             f"限制文件 {len(st['constraint_issues'])} 項" if st.get("constraint_issues") else "",
                             f"拆工檢查 {len(st['plan_check'])} 項" if st.get("plan_check") else "") if x]
        L.append(f"| {stg['name']} | {stg['status']} | " + (f"{f(ap.get('by'), 40)}（{'政策' if ap.get('kind') == 'policy' else '人'}，{f(ap.get('at'), 30)}）" if ap else ("等核准" if stg["gate"] and stg["status"] == "done" else "—"))
                 + f" | {stg['agents']}（{stg['agents_missing']}） | {'；'.join(notes) or '—'} |")
    args = {k: v for k, v in (data.get("args") or {}).items() if k in ("base", "branch", "pr", "has_ui", "has_security", "strategy", "gh_repo") and v not in (None, "")}
    sync = _stage_result(run, "sync")
    if sync.get("head_branch") and sync.get("head_branch") != sync.get("branch"):
        args["head_branch（中轉分支）"] = sync["head_branch"]
    if args:
        L += ["", "## 已經給過的參數", ""] + [f"- {f(k)}：{f(v, 200)}" for k, v in sorted(args.items())]
    pend = aw.get("pending") or {}
    if pend:
        L += ["", "## 跑到一半的段", "", f"- `awf.py next` 已經交出 {f(pend.get('stage'))} 段的腳本（{f(pend.get('workflow'))}），還沒有 `awf.py done`。"
              "workflow 跑完了就跑 done；沒跑完或不確定，重跑 `awf.py next`（換了 session 不能從中斷處續跑，會整段重跑，調研類的結果會沿用）。"]
    if aw.get("auto_approved") or aw.get("unconfirmed"):
        L += ["", "## 還沒有人確認的", ""]
        if aw.get("auto_approved"):
            L.append("- 政策核准（沒有人看過）的段：" + "、".join(f(x.get("stage")) for x in aw["auto_approved"]))
        L += [f"- [{f(q.get('stage'))}] {f(q.get('question'), 200)} → agent 已先照「{f(q.get('applied'), 200)}」做" for q in aw.get("unconfirmed") or []]
    qs = []
    for stg in snap["stages"]:
        for w, e in _envelopes(_stage_result(run, stg["name"])):
            qs += [f"- [{stg['name']}／{f(w, 60)}] {'（要先決定）' if q.get('blocking') else ''}{f(q.get('question'), 240)}" for q in e.get("open_questions") or [] if isinstance(q, dict) and q.get("question")]
    if qs:
        L += ["", "## agent 提過的問題", "", *qs[:40]]
    epic = packlib.run_epic(run, data)
    decs = [d for d in packlib.current_decisions(packlib.load_decisions(run.parent)) if d.get("scope") == "project" or (epic and d.get("epic") == epic) or d.get("run") == run.name]
    if decs:
        L += ["", "## 人已經決定過的（不要再問）", "", *packlib.decision_lines(decs[-30:])]
    L += ["", "## 怎麼接手", "",
          f"1. `python3 .claude/teams/tools/awf.py next --run {run_arg} --text` 看下一步，照做。",
          f"2. 每一段的紀錄在 `{run_arg}/<段>/{sr.INDEX_NAME}`；人的決定在 `{packlib.DECISIONS_VIEW}`。",
          "3. 核准、推送、合併仍然要當下的使用者同意：上一個 session 的同意不算。", ""]
    return "\n".join(L)


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
    for q in h.get("provisional") or []:
        L.append(f"- [暫定，請確認] {q['question']} → 已先照「{q['applied']}」做")
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
    if cmd in ("watch", "statusline"):      # 唯讀的進度監看（awf_watch.py）：不需要 plugin，也不碰閘門
        import awf_watch
        return awf_watch.main([cmd, *argv[1:]] if cmd == "statusline" else argv[1:], project)
    if cmd == "doctor":                     # 唯讀的安裝檢查（awf_doctor.py）
        import awf_doctor
        return awf_doctor.main(argv[1:], project, home)
    try:
        plugin, pnotes = (None, []) if cmd in ("approve", "start", "handoff") else find_plugin(project, opt(argv, "--plugin-dir"), home)
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
            act = carry(custom_action(run, run_arg, data, project, argv) if opt(argv, "--script") else decide(run, run_arg, data, project, plugin, argv), data)
            if act.get("action") == "run_workflow":
                act["notes"] = pnotes + act.get("notes", [])
                data.setdefault("awf", {})["pending"] = {"stage": act["stage"], "workflow": act["workflow"], "script": act["script"], "at": now_iso(),
                                                         "since": datetime.datetime.now().timestamp(), **({"custom": act["custom"]} if act.get("custom") else {})}
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
            if pend.get("custom") and pend.get("stage") == stage:      # 自訂腳本跑的段：記在結果裡，save_run 看到就不給政策核准
                result = {**result, "custom_script": pend["custom"]} if isinstance(result, dict) else {"halted": True, "halt_reason": "自訂腳本沒有回傳物件", "custom_script": pend["custom"], "returned": result}
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
        if cmd == "handoff":
            text = handoff_text(run, run_arg, data)
            if "--print" in argv:
                print(text)
                return 0
            packlib._write_lf(run / HANDOFF, text)
            print(f"已寫 {run_arg}/{HANDOFF}。新的 session 先讀它，再跑 python3 .claude/teams/tools/awf.py next --run {run_arg}")
            return 0
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
            if "--accept-defaults" in argv:
                # 人確認了暫定的做法：沒有另外用 --answer 答的那幾題，照 agent 已套用的預設記進決策記憶
                answered = {a.split("=", 1)[0].strip() for a in opts(argv, "--answer")}
                for q in provisional(_stage_result(run, stage)):
                    if q["question"] not in answered and "=" not in q["question"]:
                        passthru += ["--answer", f'{q["question"]}={q["applied"]}']
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

            # ── handoff：跨 session 的接手摘要
            d = walk_dir = "_runs/bugfix-f"      # 上面走過的 run：有 halted、有核准
            c, out = call("handoff", "--run", d)
            hf = proj / d / HANDOFF
            t1 = hf.read_text(encoding="utf-8") if hf.is_file() else ""
            check("handoff：寫出接手摘要，有狀態、各段與核准、怎麼接手", c == 0 and "## 現在在哪裡" in t1 and "| plan | done |" in t1 and "recipe：bugfix" in t1 and f"awf.py next --run {d}" in t1, out + t1[:400])
            before = (proj / d / "run.json").read_text(encoding="utf-8")
            call("handoff", "--run", d)
            check("handoff：重跑內容相同、不改 run.json、沒有時間戳以外會變的東西", hf.read_text(encoding="utf-8") == t1 and (proj / d / "run.json").read_text(encoding="utf-8") == before)
            check("handoff --print：只印不寫；找不到 run 時回錯誤", call("handoff", "--run", d, "--print")[1].startswith("# 接手摘要") and call("handoff", "--run", "_runs/nope")[0] != 0)

            # ── gate_mode：plan-only 的 recipe，非 plan 的閘門先試政策核准
            def gm_run(name: str, recipe: str, mods=(), env_extra=None) -> str:
                d = f"_runs/{name}"
                call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "改文件")
                run, data = proj / d, sr.load(proj / d)
                res = rcp.resolve(default_profile, recipe, list(mods))
                data["recipe"] = {"name": recipe, "modifiers": list(mods), "size": "", "resolved": res, "locked": True, "history": []}
                sr.rebuild_stages(data, res)
                env = {"status": "done", "summary": "審完", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": [], **(env_extra or {})}
                for st in res["stages"]:
                    if st == "merge":
                        break
                    data["stages"][st].update({"status": "done", "envelopes": 1})
                    trace = []
                    for axis in (res.get("deliver_axes") or []) if st == "deliver" else []:   # 政策核准會核對 trace、紀錄檔、recipe 要求的審查軸
                        rec = run / st / f"review-{axis}.md"
                        rec.parent.mkdir(parents=True, exist_ok=True)
                        rec.write_text("# 紀錄\n", encoding="utf-8")
                        trace.append({"label": f"review:{axis}", "agent_type": "agent-workflow:tpm-reviewer", "phase": "Review", "record": str(rec), "ok": True})
                    (run / f"{st}.json").write_text(json.dumps({"stage": st, "review": env, "subtasks": [{"id": "t1"}], "trace": trace,
                                                                "recipe": {"name": recipe, "modifiers": list(mods), "stages": res["stages"]}}), encoding="utf-8")
                    if st in ("plan",) or (st != "deliver" and data["stages"][st].get("gate")):
                        data["stages"][st]["approved"] = {"by": "human", "at": "t", "kind": "human"}
                sr.dump(run, data)
                return d
            soft = {"open_questions": [{"question": "標題用哪個？", "blocking": False, "options": [], "recommendation": "沿用", "default_applied": "沿用舊標題"}]}
            d = gm_run("gm-clean", "docs", env_extra=soft)
            a = act_of(call("next", "--run", d)[1])
            ap = sr.load(proj / d)["stages"]["deliver"].get("approved") or {}
            check("gate_mode：docs（plan-only）的 deliver 結果乾淨 → 政策核准、直接到下一步（推送仍要人同意）", a["action"] == "manual" and a["stage"] == "merge"
                  and ap.get("kind") == "policy" and "gate_mode=plan-only" in ap.get("note", ""), (a.get("policy_refused"), a.get("action"), ap))
            check("gate_mode：政策核准的段與沒有人看過的暫定做法，在下一次要人出面時列出來", a.get("auto_approved") == ["deliver"]
                  and a.get("unconfirmed") == [{"stage": "deliver", "question": "標題用哪個？", "applied": "沿用舊標題"}] and "unconfirmed" in a["say"], a)
            ht = call("handoff", "--run", d, "--print")[1]
            check("handoff：列出政策核准（沒有人看過）的段與還沒確認的暫定做法", "政策核准（沒有人看過）的段：deliver" in ht and "標題用哪個？ → agent 已先照「沿用舊標題」做" in ht and "（政策，" in ht, ht[-600:])
            hard = {"open_questions": [{"question": "要不要改對外文件？", "blocking": True, "options": [{"choice": "改", "cost": "x"}], "recommendation": "改", "default_applied": "none"}]}
            d = gm_run("gm-dirty", "docs", env_extra=hard)
            a = act_of(call("next", "--run", d)[1])
            check("gate_mode：結果不乾淨（有 blocking 問題）→ 不核准，交人並說明原因", a["action"] == "gate" and a["stage"] == "deliver" and any("blocking" in x for x in a.get("policy_refused") or [])
                  and not sr.load(proj / d)["stages"]["deliver"].get("approved"), a)
            d = gm_run("gm-mod", "docs", mods=("security",))
            a = act_of(call("next", "--run", d)[1])
            check("gate_mode：帶 modifier 一律 strict（照舊等人核准，不試政策核准）", a["action"] == "gate" and "policy_refused" not in a and not sr.load(proj / d)["stages"][a["stage"]].get("approved"), a)
            d = gm_run("gm-strict", "bugfix")
            a = act_of(call("next", "--run", d)[1])
            check("gate_mode：strict 的 recipe（bugfix）照舊等人核准", a["action"] == "gate" and "policy_refused" not in a, a)
            d = "_runs/gm-plan"
            call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "改文件", "--recipe", "docs")
            data = sr.load(proj / d)
            res = rcp.resolve(default_profile, "docs", [])
            data["recipe"] = {"name": "docs", "modifiers": [], "size": "", "resolved": res, "locked": False, "history": []}
            sr.rebuild_stages(data, res)
            data["stages"]["plan"].update({"status": "done", "envelopes": 1})
            (proj / d / "plan.json").write_text(json.dumps({"stage": "plan", "intent": {"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": []}}), encoding="utf-8")
            sr.dump(proj / d, data)
            a = act_of(call("next", "--run", d)[1])
            check("gate_mode：plan 段永遠等人核准（plan-only 也不試政策核准）", a["action"] == "gate" and a["stage"] == "plan" and a["human_only"] is True and "policy_refused" not in a
                  and not sr.load(proj / d)["stages"]["plan"].get("approved"), a)

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
                   "open_questions": [{"question": "編碼用什麼？", "blocking": True, "options": [{"choice": "UTF-8", "cost": "無"}], "recommendation": "UTF-8", "default_applied": ""},
                                      {"question": "欄位順序？", "blocking": False, "options": [{"choice": "照原型", "cost": "無"}], "recommendation": "照原型", "default_applied": "照原型"},
                                      {"question": "要不要加註解？", "blocking": False, "options": [], "recommendation": "不加", "default_applied": "none"}]}
            (run / "plan.json").write_text(json.dumps({"stage": "plan", "missing": ["research:y"], "research": [env], "reused": ["specs-adr"], "trace": [{"label": "x"}]}), encoding="utf-8")
            a = act_of(call("next", "--run", d)[1])
            h = a.get("highlights") or {}
            check("閘門：plan 等人核准，重點列出各 agent 的摘要、所有 open_questions、缺件、沿用的調研、紀錄缺件", a["action"] == "gate" and a["human_only"] is True
                  and h["agents"][0]["summary"] == "調研完成" and h["open_questions"][0]["blocking"] is True and h["open_questions"][0]["options"] == ["UTF-8"]
                  and h["missing"] == ["research:y"] and h["reused"] == ["specs-adr"] and h["missing_records"] == ["research:x"], a)
            check("閘門：--text 給人看的摘要有動作、核准指令與要先決定的問題", "動作：gate（plan 段）" in call("next", "--run", d, "--text")[1] and "[要先決定] 編碼用什麼？" in call("next", "--run", d, "--text")[1])
            check("閘門：agent 已照建議先做的問題列成 provisional（default_applied 是 none 的不算），要先決定的排最前面",
                  h.get("provisional") == [{"question": "欄位順序？", "applied": "照原型"}] and h["open_questions"][0]["blocking"] is True
                  and "[暫定，請確認] 欄位順序？" in call("next", "--run", d, "--text")[1], h)
            c, out = call("approve", "--run", d, "--by", "human", "--answer", "編碼用什麼？=UTF-8", "--accept-defaults")
            dec = {x["question"]: x["answer"] for x in packlib.load_decisions(run.parent)}
            check("approve --accept-defaults：暫定的做法記進決策記憶；另外用 --answer 答的照人的答覆", c == 0 and dec.get("欄位順序？") == "照原型" and dec.get("編碼用什麼？") == "UTF-8" and "要不要加註解？" not in dec, (out, dec))
            st = sr.load(run)["stages"]["plan"]
            check("approve：不指定段時核准正在等的那一段、時間自動取、--answer 記進決策記憶", c == 0 and st.get("approved", {}).get("by") == "human"
                  and re.match(r"^\d{4}-\d\d-\d\dT", st["approved"]["at"]) and dec.get("編碼用什麼？") == "UTF-8", out)
            data = sr.load(run)
            data["stages"]["design"].update({"status": "done", "envelopes": 0})
            sr.dump(run, data)
            c, out = call("approve", "--run", d, "--by", "human")
            check("approve：核准仍由 save_run 把關（沒有信封的段照樣拒絕，狀態機不放寬閘門）", c != 0 and "沒有任何信封" in out and not sr.load(run)["stages"]["design"].get("approved"), out)
            check("approve：沒有在等核准的段、缺 --by 都拒絕", call("approve", "--run", "_runs/args", "--by", "x")[0] != 0 and call("approve", "--run", d)[0] != 0)

            # ── 自訂腳本（next --script）：先檢查、再接上執行核心；閘門條件不變，而且不能政策核准
            def cs_run(name: str, upto: str, recipe: str = "feature") -> str:
                d = f"_runs/{name}"
                call("start", "--run", d, "--event", "sdlc", "--repo", "repos/app", "--epic", "demo", "--requirement", "分波打地基")
                run, data = proj / d, sr.load(proj / d)
                res = rcp.resolve(default_profile, recipe, [])
                data["recipe"] = {"name": recipe, "modifiers": [], "size": "", "resolved": res, "locked": True, "history": []}
                sr.rebuild_stages(data, res)
                for st in res["stages"]:
                    if st == upto:
                        break
                    data["stages"][st].update({"status": "done", "envelopes": 1, "approved": {"by": "human", "at": "t", "kind": "human"}})
                    (run / f"{st}.json").write_text(json.dumps({"stage": st, "subtasks": [{"id": "t1", "title": "x", "owns": ["a.py"]}]}), encoding="utf-8")
                sr.dump(run, data)
                return d

            scripts = proj / "scripts"
            scripts.mkdir()
            (scripts / "waves.js").write_text(awf_script.GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'"), encoding="utf-8")
            (scripts / "direct.js").write_text(awf_script.GOOD.replace("'acme:a11y'", "NS + 'tpm-reviewer'").replace("const done = []", "const done = [await agent('x', {})]"), encoding="utf-8")
            (scripts / "ext.js").write_text(awf_script.GOOD, encoding="utf-8")
            d = cs_run("custom", "build")
            c, out = call("next", "--run", d, "--script", "scripts/waves.js")
            a = act_of(out) if not c else {}
            wf = proj / d / ".wf" / "custom-build.js"
            body = wf.read_text(encoding="utf-8") if wf.is_file() else ""
            check("自訂腳本：合格的可以執行（run_workflow，腳本寫在 <run>/.wf/custom-<段>.js，記下來源與雜湊）",
                  a.get("action") == "run_workflow" and a.get("stage") == "build" and a.get("script") == f"{d}/.wf/custom-build.js" and a.get("custom", {}).get("source") == "waves.js"
                  and len(a.get("custom", {}).get("sha1", "")) == 40 and sr.load(proj / d)["awf"]["pending"].get("custom") == a.get("custom"), out)
            check("自訂腳本：接上執行核心（角色白名單、派工數上限、這一段的參數、recipe、片段與 schema），meta 仍在最前面",
                  body.startswith("export const meta = {") and "function awfAgent(" in body and "function awfLoop(" in body and "function awfInit(" in body
                  and 'awfKernelInit({ profile: METHODOLOGY, stage: "build"' in body and '"agent-workflow:dev-implementer"' in body and '"agent-workflow:dev-merge-steward"' not in body.split("roles:")[1].split("\n")[0]
                  and "max_agents: 40" in body and '"epic":"demo"' in body and "/*@schema:subtasks*/ {" in body and "null /*@end*/" not in body and not PH.search(body), body[:300])
            for bad_name, why in (("direct.js", "不能直接用 agent()"), ("ext.js", "acme:a11y 不是登記過的角色")):
                d_bad = cs_run("custom-" + bad_name[:-3], "build")
                c, out = call("next", "--run", d_bad, "--script", f"scripts/{bad_name}")
                check(f"自訂腳本：{why} → 不執行（不產生腳本、不記等待中的段）", c != 0 and why in out and "沒通過檢查" in out
                      and not (proj / d_bad / ".wf" / "custom-build.js").exists() and "pending" not in (sr.load(proj / d_bad).get("awf") or {}), out)
            g = load_json(teams / "guard.config.json")
            (teams / "guard.config.json").write_text(json.dumps({**g, "extension_agents": {"acme:a11y": "tpm-reviewer"}}), encoding="utf-8")
            d_ext = cs_run("custom-ext-ok", "build")
            c, out = call("next", "--run", d_ext, "--script", "scripts/ext.js")
            check("自訂腳本：guard.config.json 登記過的擴充 agent 可以派，而且進角色白名單", c == 0 and '"acme:a11y"' in (proj / d_ext / ".wf" / "custom-build.js").read_text(encoding="utf-8").split("roles:")[1].split("\n")[0], out)
            (teams / "guard.config.json").write_text(json.dumps(g), encoding="utf-8")
            for stg, upto in (("plan", "plan"), ("test", "test"), ("merge", "merge")):
                d_no = cs_run("custom-no-" + stg, upto)
                c, out = call("next", "--run", d_no, "--script", "scripts/waves.js")
                check(f"自訂腳本：{stg} 段不開放", c != 0 and f"{stg} 段不能用自訂腳本" in out, out)
            d_chain = cs_run("custom-chain", "build")
            dd = sr.load(proj / d_chain)
            dd["stages"]["design"].pop("approved")
            sr.dump(proj / d_chain, dd)
            c, out = call("next", "--run", d_chain, "--script", "scripts/waves.js", "--stage", "build")
            check("自訂腳本：不能跳段（前面的段沒核准就拒絕）", c != 0 and "前面的段還沒過" in out, out)
            c, out = call("next", "--run", "_runs/ev-cs", "--script", "scripts/waves.js") if not call("start", "--run", "_runs/ev-cs", "--event", "meeting")[0] else (1, "自訂腳本只用在 SDLC 的 run")
            check("自訂腳本：事件的 run 不能用", c != 0 and "只用在 SDLC" in out, out)
            # 存檔：條件和標準腳本一樣（沒有 trace 就是 invalid），結果帶 custom_script，政策核准一律拒絕
            env = {"status": "done", "summary": "做完", "artifacts": [], "evidence": [], "confidence": 0.9, "derived_work": [], "payload": {}, "open_questions": []}
            rec_dir = proj / d / "build"
            rec_dir.mkdir(parents=True, exist_ok=True)
            mk = lambda label, role: {"label": label, "agent_type": "agent-workflow:" + role, "phase": "Build", "record": str(rec_dir / (label.replace(":", "-") + ".md")), "ok": True, "tier": "standard", "model": "sonnet"}  # noqa: E731
            res_rcp = {"name": "feature", "modifiers": [], "stages": rcp.resolve(default_profile, "feature", [])["stages"]}
            notrace = root / "cs-notrace.json"
            notrace.write_text(json.dumps({"stage": "build", "halted": False, "results": [env], "recipe": res_rcp}), encoding="utf-8")
            c, out = call("done", "--run", d, "--output", str(notrace), "--no-next")
            st_n = sr.load(proj / d)["stages"]["build"]
            check("自訂腳本：回傳沒有 trace → 和標準腳本一樣記成紀錄缺件（閘門條件不變）", "回傳卻沒有 trace" in out and st_n.get("missing_records"), (out, st_n))
            c, out = call("next", "--run", d, "--script", "scripts/waves.js")
            check("自訂腳本：build 存檔後下一段是 sync，不開放自訂腳本；要重跑 build 得明講 --stage build --retry", c != 0 and "sync 段不能用自訂腳本" in out, out)
            call("next", "--run", d, "--script", "scripts/waves.js", "--stage", "build", "--retry")
            full_trace = [mk("Tests:t1", "dev-acceptance-tester"), mk("Implement:t1", "dev-implementer"), mk("Integrate", "dev-integrator")]
            for x in full_trace:
                Path(x["record"]).write_text("# 紀錄\n", encoding="utf-8")
            good = root / "cs-good.json"
            good.write_text(json.dumps({"stage": "build", "halted": False, "results": [env], "recipe": res_rcp, "trace": full_trace}), encoding="utf-8")
            c, out = call("done", "--run", d, "--output", str(good), "--no-next")
            saved = load_json(proj / d / "build.json") or {}
            st_b = sr.load(proj / d)["stages"]["build"]
            check("自訂腳本：存下來的結果帶 custom_script（來源與雜湊）", saved.get("custom_script") == a.get("custom") and st_b["status"] in ("done", "invalid"), (out, st_b))
            check("自訂腳本：政策核准一律拒絕（要由人核准）", any("自訂腳本" in e for e in sr.policy_violations("build", saved, proj / d))
                  and not any("自訂腳本" in e for e in sr.policy_violations("build", {k: v for k, v in saved.items() if k != "custom_script"}, proj / d)))
            hl = highlights(proj / d, d, sr.load(proj / d), "build")
            check("自訂腳本：閘門重點列出這一段用的是自訂腳本", hl.get("custom_script", {}).get("source") == "waves.js", hl)
            src_roles = re.search(r"^CORE = \{(.*?)\}", (HERE / "check_agents.py").read_text(encoding="utf-8"), re.S | re.M)
            core = set(re.findall(r'"([a-z-]+)"', src_roles.group(1))) if src_roles else set()
            check("自訂腳本：可派的模版角色＝check_agents 的 CORE 扣掉 dev-merge-steward", core and set(awf_script.ROLES) == core - {"dev-merge-steward"}, sorted(core ^ set(awf_script.ROLES)))

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
            check("事件：meeting 用 event（共用的事件 workflow），帶 event、source 與 run_dir", a["action"] == "run_workflow" and a["workflow"] == "event" and a["stage"] == "meeting"
                  and json.loads((proj / d / ".wf" / "event.args.json").read_text(encoding="utf-8")) == {"source": "meetings/a.md", "run_dir": (proj / d).resolve().as_posix(), "event": "meeting"}, a)
            call("start", "--run", "_runs/ev-brief", "--event", "brief", "--set", "brief=briefs/a.md")
            check("事件：形狀不同的事件（brief）仍用自己的 workflow", act_of(call("next", "--run", "_runs/ev-brief")[1])["workflow"] == "event-brief")
            meth_file = teams / "methodology.json"
            meth_saved = meth_file.read_text(encoding="utf-8") if meth_file.is_file() else None
            custom = {"args": ["incident"], "context": "事故：{incident}", "gather": {"angles": [{"key": "cause", "ask": "找原因"}]}, "challenge": {"ask": "反駁"}, "write": {"ask": "寫回顧"}}
            meth_file.write_text(json.dumps({**(json.loads(meth_saved) if meth_saved else {}), "events": {"incident": custom, "merge": custom, "upstream": {**custom, "args": ["what"]}}}, ensure_ascii=False), encoding="utf-8")
            call("start", "--run", "_runs/ev-inc", "--event", "incident")
            a = act_of(call("next", "--run", "_runs/ev-inc")[1])
            check("自訂事件：profile 的 events 加一筆就有這個事件；缺它要的參數回 need_input", a["action"] == "need_input" and set(a["need"]) == {"incident"}, a)
            a = act_of(call("next", "--run", "_runs/ev-inc", "--set", "incident=INC-1")[1])
            check("自訂事件：參數齊了就跑 event，args 帶 event=<類型>", a["action"] == "run_workflow" and a["workflow"] == "event" and a["stage"] == "incident"
                  and json.loads((proj / "_runs/ev-inc" / ".wf" / "event.args.json").read_text(encoding="utf-8")).get("event") == "incident", a)
            check("自訂事件：可以取代內建類型（必要參數照 profile）；不能蓋掉 merge 這種不是 event.js 的事件",
                  event_required(proj, "upstream") == ["what"] and event_route(proj, "upstream") == ("event", "upstream") and event_route(proj, "merge") == ("sdlc-merge", "merge"))
            if meth_saved is None:
                meth_file.unlink()
            else:
                meth_file.write_text(meth_saved, encoding="utf-8")
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
                ev_src = (real / TEMPLATES / "event.js").read_text(encoding="utf-8")
                found = {m.group(1): [x.strip(" '") for x in m.group(2).split(",") if x.strip()] for m in re.finditer(r"^  '?([a-z][a-z-]*)'?: \{\n    title: [^\n]*\n    args: \[([^\]]*)\]", ev_src, re.M)}
                check("真的範本：event.js 內建的事件類型與必要參數和 EVENT_TYPES 一致", found == EVENT_TYPES, found)
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
