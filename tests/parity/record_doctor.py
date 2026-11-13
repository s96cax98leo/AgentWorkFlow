#!/usr/bin/env python3
"""doctor 的對跑基準：在暫存專案裡對 Python 版 awf_doctor 實跑，結果印成 JSON。

用法（repo 根目錄）：
    python3 tests/parity/record_doctor.py > tests/parity/doctor.expected.json

步驟：write／rm（改暫存目錄的檔）、checks（跑 run_checks，記項目、文字版、JSON 版與結束碼）、fn（函式探測）。
暫存路徑換成 @TMP@。Rust 版沒有 python 與 hooks-compile 兩項（執行檔不需要 Python），所以這裡先濾掉再記，
文字版、JSON 版與結束碼也用濾過的項目算。連錄兩次要逐字相同。
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

SRC = Path(__file__).resolve().parents[2]
TOOLS = SRC / "scaffold" / ".claude" / "teams" / "tools"
sys.path.insert(0, str(TOOLS))
sys.dont_write_bytecode = True
for k in ("AWF_PLUGIN_DIR", "CLAUDE_PLUGIN_ROOT", "CLAUDE_CONFIG_DIR"):
    os.environ.pop(k, None)

import awf  # noqa: E402
import awf_doctor as D  # noqa: E402

DROP = ("python", "hooks-compile")
ALL = ["git", "gh", "node"]
steps: list[dict] = []


def main() -> None:
    with tempfile.TemporaryDirectory() as td:
        root = Path(td).resolve()

        def norm(x):
            return json.loads(json.dumps(x, ensure_ascii=False).replace(str(root), "@TMP@"))

        def w(rel: str, text: str) -> None:
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text, encoding="utf-8")
            steps.append({"op": "write", "path": rel, "text": text.replace(str(root), "@TMP@")})

        def rm(rel: str) -> None:
            (root / rel).unlink()
            steps.append({"op": "rm", "path": rel})

        def checks(note: str, project: str = "proj", plugin_dir: str = "plugin", tools=ALL, no_plugin: bool = False) -> list[dict]:
            real = awf.find_plugin
            if no_plugin:
                awf.find_plugin = lambda *a, **k: (None, [])
            try:
                items = D.run_checks(root / project, root / "home", str(root / plugin_dir) if plugin_dir else "", lambda n: "/x/" + n if n in tools else None, (3, 11, 0))
            finally:
                awf.find_plugin = real
            items = [i for i in items if i["id"] not in DROP]
            bad = any(i["level"] == D.ERR for i in items)
            steps.append(norm({"op": "checks", "note": note, "project": project, "plugin_dir": plugin_dir, "tools": list(tools), "no_plugin": no_plugin,
                               "items": items, "text": D.text_of(items), "json": json.dumps({"ok": not bad, "checks": items}, ensure_ascii=False, indent=2), "code": 1 if bad else 0}))
            return items

        def fn(name: str, *args) -> None:
            ret = getattr(D, name)(*[root / "home" if a == "@HOME@" else a for a in args])
            steps.append(norm({"op": "fn", "name": name, "args": list(args), "ret": list(ret) if isinstance(ret, tuple) else ret}))

        good = json.loads((TOOLS.parent / "methodologies" / "default.json").read_text(encoding="utf-8"))
        dump = lambda d: json.dumps(d, ensure_ascii=False)  # noqa: E731
        T = "proj/.claude/teams/"

        def installed(plugins) -> None:
            w("home/plugins/installed_plugins.json", dump({"version": 2, "plugins": plugins}))

        def inst(scope="user", path="cache/a", version="0.5.0", project=None) -> dict:
            e = {"scope": scope, "installPath": str(root / path), "version": version}
            if project:
                e["projectPath"] = str(root / project)
            return e

        # ── 環境
        for tools in (ALL, [], ["git"], ["gh", "node"]):
            checks("環境：" + "、".join(tools), project="empty", plugin_dir="", tools=tools)
        # ── 假的 plugin 與專案
        w("plugin/.claude-plugin/plugin.json", dump({"name": "agent-workflow", "version": "0.5.0"}))
        w("plugin/workflow-templates/sdlc-plan.js", "// x\n")
        w("plugin/hooks/hooks.json", "{}")
        w(T + "methodology.json", dump(good))
        w(T + "guard.config.json", dump({"runs_dir": "_tpm/runs"}))
        w(T + "workflow.config", 'PROJECT_NAME="示範客戶 ACME"\n')
        w(T + "VERSION", "0.5.0\n")
        w("home/plugins/.keep", "")
        checks("乾淨的專案")
        checks("乾淨的專案，缺 gh 與 node", tools=["git"])
        checks("還沒套用的專案", project="empty", plugin_dir="")
        # ── 版本
        for v in ("0.4.1\n", "0.6.0\n", "v0.5.0", "0.5.0-rc.1\n", "0.10.0", "  0.5\n", "abc\n", "\n", "0.5.0.1"):
            w(T + "VERSION", v)
            checks("版本：" + v.strip())
        rm(T + "VERSION")
        checks("版本：沒有 VERSION 檔")
        w(T + "VERSION", "0.5.0\n")
        w("plugin/.claude-plugin/plugin.json", dump({"name": "agent-workflow"}))
        checks("plugin.json 沒有版本")
        w("plugin/.claude-plugin/plugin.json", "{壞掉")
        checks("plugin.json 讀不懂")
        w("plugin/.claude-plugin/plugin.json", "﻿" + dump({"name": "agent-workflow", "version": "0.5.0"}))
        checks("plugin.json 開頭有 BOM")
        w("plugin/.claude-plugin/plugin.json", dump({"name": "agent-workflow", "version": "0.5.0"}))
        for a in ("0.10.0", "0.9.9", "0.3.0-x.1", "", "v1.2", " v1", "vv1", "1..2", "1.2x.3", "1.x", "x", "1.", ".1", "10", "0.0.0", "1.2.3.4.5", "\t2.0 beta"):
            fn("vtuple", a)
        # ── 找不到 plugin
        checks("找不到 plugin", plugin_dir="", no_plugin=True)
        checks("--plugin-dir 指錯", plugin_dir="nope")
        checks("沒給 --plugin-dir：用模版 repo 自己的 plugin", plugin_dir="")
        # ── 安裝來源
        w("cache/a/workflow-templates/x.js", "")
        w("cache/b/workflow-templates/x.js", "")
        installed({"agent-workflow@agentworkflow": [inst()]})
        checks("安裝：一個來源一個版本")
        installed({"agent-workflow@agentworkflow": [inst()], "agent-workflow@my-fork": [inst(path="cache/b", version="0.3.0")], "other-plugin@x": [inst(path="cache/b", version="9")]})
        checks("安裝：兩個來源")
        installed({"agent-workflow@agentworkflow": [inst()], "agent-workflow@my-fork": [inst("project", "cache/b", "0.3.0", "別的專案")]})
        checks("安裝：另一個來源只裝在別的專案")
        installed({"agent-workflow@agentworkflow": [inst(), inst("project", "cache/b", "0.4.1", "proj")]})
        checks("安裝：同一來源兩個範圍不同版本")
        installed({"agent-workflow@agentworkflow": [inst(), inst("project", "cache/b", "0.4.1", "proj")], "agent-workflow@my-fork": [inst(path="不見了", version="0.3.0")]})
        checks("安裝：來源、版本、目錄三種問題一起")
        installed({"agent-workflow@agentworkflow": [inst(path="不見了")]})
        checks("安裝：安裝目錄不存在")
        installed({"agent-workflow@agentworkflow": [{"installPath": str(root / "不見了"), "version": "0.5.0"}]})
        checks("安裝：安裝目錄不存在、沒有 scope")
        installed({"agent-workflow@agentworkflow": inst()})
        checks("安裝：紀錄不是清單")
        installed({"agent-workflow@agentworkflow": ["x", 3, None, inst(version="")]})
        checks("安裝：清單裡有不是物件的項目、版本是空的")
        installed({"agent-workflow": [inst()], "agent-workflow-x@m": [inst(path="不見了")]})
        checks("安裝：鍵沒有 marketplace、名字只是開頭相同")
        installed({"other@x": [inst()]})
        checks("安裝：只有別的 plugin")
        installed([])
        checks("安裝：plugins 不是物件")
        w("home/plugins/installed_plugins.json", "[]")
        checks("安裝：登記檔不是物件")
        w("home/plugins/installed_plugins.json", "{壞掉")
        checks("安裝：登記檔讀不懂")
        # marketplace 的版本
        installed({"agent-workflow@agentworkflow": [inst(version="0.4.1")]})
        w("market/.claude-plugin/marketplace.json", dump({"name": "agentworkflow", "plugins": [{"name": "agent-workflow", "source": "./plugin", "version": "0.5.0"}]}))
        w("home/plugins/known_marketplaces.json", dump({"agentworkflow": {"installLocation": str(root / "market")}}))
        checks("安裝：marketplace 已是新版")
        fn("market_version", "@HOME@", "agentworkflow")
        fn("market_version", "@HOME@", "nope")
        w("market/.claude-plugin/marketplace.json", dump({"plugins": [{"name": "other", "version": "9"}, "x", {"name": "agent-workflow", "source": "./plugin"}]}))
        w("market/plugin/.claude-plugin/plugin.json", dump({"version": "0.6.0"}))
        fn("market_version", "@HOME@", "agentworkflow")
        checks("安裝：marketplace.json 沒寫版本時讀 plugin.json")
        w("market/.claude-plugin/marketplace.json", dump({"plugins": [{"name": "agent-workflow", "source": {"source": "github"}}]}))
        fn("market_version", "@HOME@", "agentworkflow")
        w("market/.claude-plugin/marketplace.json", dump({"plugins": {"name": "agent-workflow"}}))
        fn("market_version", "@HOME@", "agentworkflow")
        w("home/plugins/known_marketplaces.json", dump({"agentworkflow": "x"}))
        w("home/plugins/marketplaces/agentworkflow/.claude-plugin/marketplace.json", dump({"plugins": [{"name": "agent-workflow", "version": "0.4.1"}]}))
        fn("market_version", "@HOME@", "agentworkflow")
        checks("安裝：marketplace 在預設位置、版本相同")
        w("home/plugins/known_marketplaces.json", "{壞掉")
        fn("market_version", "@HOME@", "agentworkflow")
        rm("home/plugins/marketplaces/agentworkflow/.claude-plugin/marketplace.json")
        fn("market_version", "@HOME@", "agentworkflow")
        rm("home/plugins/installed_plugins.json")
        checks("安裝：沒有登記檔")
        # ── hook
        hook = {"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "python3 .claude/hooks/awf_hook.py guard-shell"}]}]}}
        w("proj/.claude/settings.json", dump(hook))
        installed({"agent-workflow@agentworkflow": [inst()]})
        checks("hook：專案與 plugin 都註冊")
        rm("home/plugins/installed_plugins.json")
        checks("hook：只有專案註冊")
        w("proj/.claude/settings.json", dump({"hooks": {}}))
        w("proj/.claude/settings.local.json", dump(hook))
        installed({"agent-workflow@agentworkflow": [inst()]})
        checks("hook：註冊在 settings.local.json")
        w("proj/.claude/settings.local.json", dump({"permissions": {"allow": ["Bash(python3 .claude/hooks/awf_hook.py:*)"]}}))
        checks("hook：awf_hook.py 只出現在 hooks 以外的地方")
        w("proj/.claude/settings.json", "[1]")
        w("proj/.claude/settings.local.json", "{壞掉")
        checks("hook：settings 讀不懂")
        rm("proj/.claude/settings.json")
        rm("proj/.claude/settings.local.json")
        rm("home/plugins/installed_plugins.json")
        rm("plugin/hooks/hooks.json")
        (root / "plugin/hooks").rmdir()
        steps.append({"op": "rmdir", "path": "plugin/hooks"})
        checks("hook：沒有任何註冊、也沒有 hooks 目錄")
        w("proj/.claude/hooks/README.md", "x\n")
        checks("hook：只有專案的 hooks 目錄")
        rm("proj/.claude/hooks/README.md")
        (root / "proj/.claude/hooks").rmdir()
        steps.append({"op": "rmdir", "path": "proj/.claude/hooks"})
        w("plugin/hooks/hooks.json", "{}")
        # ── 設定
        for text, note in (("{壞掉", "不是 JSON"), ("[]", "不是物件"), ("null", "null"), ("{}", "空物件")):
            w(T + "methodology.json", text)
            checks("methodology：" + note)
        w(T + "methodology.json", dump({**good, "constraints": [{"id": "a", "file": "/etc/passwd", "stages": ["design"]}]}))
        checks("methodology：違反語意規則")
        w(T + "methodology.json", dump({**good, "constraints": [{"id": "a", "file": "/x/%d" % i, "stages": ["design"]} for i in range(7)]}))
        checks("methodology：超過五個問題")
        w(T + "methodology.json", dump({**good, "extensions": [{"name": "x", "agent": "my:agent", "stage": "plan", "instructions": "看一下"}, {"name": "y", "agent": "a:first", "stage": "test", "instructions": "看一下"}]}))
        checks("擴充 agent 沒登記")
        w(T + "guard.config.json", dump({"runs_dir": "_tpm/runs", "extension_agents": {"my:agent": "tpm-reviewer", "a:first": "tpm-reviewer"}}))
        checks("擴充 agent 都登記了")
        rm(T + "methodology.json")
        checks("沒有 methodology.json")
        w(T + "methodology.json", dump(good))
        lb = "{" * 2
        for text, note in (('{"runs_dir": "' + lb + 'RUNS_DIR}}"}', "還有佔位符"), ("[]", "不是物件"), ("{壞掉", "不是 JSON"), (dump({"auto_merge": {"enabled": True}}), "auto_merge 啟用"),
                           (dump({"auto_merge": {"enabled": False}}), "auto_merge 關閉"), (dump({"auto_merge": True}), "auto_merge 不是物件"), ("﻿{}", "BOM")):
            w(T + "guard.config.json", text)
            checks("guard：" + note)
        rm(T + "guard.config.json")
        rm(T + "workflow.config")
        checks("缺 guard.config.json、workflow.config")
        w(T + "guard.config.json", dump({"runs_dir": "_tpm/runs"}))
        w(T + "workflow.config", 'PROJECT_NAME="示範客戶 ACME"\n')
        # ── 覆寫檔
        W = "proj/.claude/workflows/"
        w(W + "sdlc-plan.js", "// 我的版本\n// awf-override: 0.4.1\n")
        w(W + "sdlc-test.js", "// 現行\n// awf-override: 0.5.0\n")
        w(W + "sdlc-build.js", "// 舊副本，沒有標記\n")
        w(W + "event-meeting.js", "// 我的會議流程\n// awf-override: 0.4.1\n")
        w(W + "event-decision.js", "// 沒有標記的舊副本\n")
        w(W + "notes.txt", "不是 js\n")
        checks("覆寫檔：各種情況")
        w("plugin/workflow-templates/event-meeting.js", "// 還在\n")
        checks("覆寫檔：併掉的那支在 plugin 裡還有同名範本")
        rm(W + "event-meeting.js")
        for i in range(9):
            w(W + "old-%d.js" % i, "// 舊\n")
        checks("覆寫檔：超過八個舊副本")
        w(W + "sdlc-plan.js", "// awf-override: 0.4.1 多一個字\n")
        w(W + "sdlc-test.js", "x\n  // awf-override: 0.5.0\n// awf-override:0.5.0   \n")
        checks("覆寫檔：標記格式的邊界")
        checks("覆寫檔：找不到 plugin", plugin_dir="", no_plugin=True)
    print(json.dumps({"steps": steps}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
