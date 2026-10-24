"""檢查 tpm-*／dev-* agent 的格式：frontmatter 欄位、五段內文、預載 skill 存在、payload 有對應 schema。

agents 可能在 plugin（agent-workflow）或專案的 .claude/agents。找 agents 目錄的順序：
    --agents-dir <dir>  →  --plugin-dir <dir>/agents  →  環境變數 AWF_PLUGIN_DIR、CLAUDE_PLUGIN_ROOT 的 agents/
    →  專案 .claude/agents（裡面有 tpm-*／dev-* 時）
skill 會在這些地方找：plugin 的 skills/、專案 .claude/skills、~/.claude/skills、Claude Code 內建。
模版內建 16 個角色（CORE：tpm 6 個、dev 10 個）；檢查 plugin 的 agents 時，少了任何一個都算錯。
用法：python3 .claude/teams/tools/check_agents.py [--plugin-dir <plugin 目錄>] [--agents-dir <dir>]
"""
import os
import re
import sys
from pathlib import Path

claude = Path(__file__).resolve().parents[2]  # 專案的 .claude/
teams = claude / "teams"


def opt(name):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


plugin = opt("--plugin-dir") or os.environ.get("AWF_PLUGIN_DIR") or os.environ.get("CLAUDE_PLUGIN_ROOT")
plugin = Path(plugin).resolve() if plugin else None
agents_dir = opt("--agents-dir")
if agents_dir:
    agents_dir = Path(agents_dir).resolve()
elif plugin and (plugin / "agents").is_dir():
    agents_dir = plugin / "agents"
else:
    agents_dir = claude / "agents"
skills_dirs = [d for d in (plugin / "skills" if plugin else None, agents_dir.parent / "skills", claude / "skills", Path.home() / ".claude/skills") if d]
builtin = {"code-review", "security-review", "simplify"}  # Claude Code 內建
SECTIONS = ["切分理由", "職責", "硬性約束", "工作方式", "交件格式"]
CORE = {
    "tpm-planner", "tpm-researcher", "tpm-reviewer", "tpm-skeptic", "tpm-upstream-analyst", "tpm-writer",
    "dev-spec-author", "dev-acceptance-tester", "dev-implementer", "dev-integrator", "dev-verifier",
    "dev-design-reviewer", "dev-security-reviewer", "dev-pr-reviewer", "dev-merge-steward", "dev-release-watcher",
}

if not agents_dir.is_dir():
    print(f"找不到 agents 目錄：{agents_dir}（用 --plugin-dir 或 --agents-dir 指定）")
    sys.exit(1)
errs, n = [], 0
for p in sorted(agents_dir.glob("*.md")):
    if not p.stem.startswith(("tpm-", "dev-")):
        continue
    n += 1
    t = p.read_text(encoding="utf-8")
    m = re.match(r"^---\n(.*?)\n---\n", t, re.S)
    if not m:
        errs.append(f"{p.name}: 沒有 frontmatter"); continue
    fm = dict(re.findall(r"^([a-z_]+):\s*(.*)$", m.group(1), re.M))
    for k in ("name", "description", "tools", "model", "skills"):
        if k not in fm: errs.append(f"{p.name}: 缺 {k}")
    if fm.get("name") != p.stem: errs.append(f"{p.name}: name 與檔名不同")
    if fm.get("model") != "inherit": errs.append(f"{p.name}: model 應為 inherit")
    if "Bash(" in fm.get("tools", ""): errs.append(f"{p.name}: tools 不可用 Bash(...)（會讓 Bash 消失）")
    for s in re.findall(r"[\w:-]+", fm.get("skills", "")):
        s = s.rsplit(":", 1)[-1]  # plugin 命名空間，例如 agent-workflow:tpm-team
        if s not in builtin and not any((d / s / "SKILL.md").exists() for d in skills_dirs):
            errs.append(f"{p.name}: 預載的 skill {s} 不存在")
    heads = re.findall(r"^## (.+?)\s*$", t, re.M)
    for s in SECTIONS:
        if s not in heads: errs.append(f"{p.name}: 缺段落「## {s}」")
    for pl in re.findall(r"payloads/([a-z-]+)\.schema\.json", t):
        if not (teams / "schemas/payloads" / f"{pl}.schema.json").exists():
            errs.append(f"{p.name}: payload schema {pl} 不存在")
if n == 0:
    errs.append(f"{agents_dir} 裡沒有 tpm-*／dev-* agent")
if plugin and agents_dir == plugin / "agents":
    missing = sorted(CORE - {p.stem for p in agents_dir.glob("*.md")})
    if missing:
        errs.append(f"plugin 少了模版內建角色（應有 {len(CORE)} 個）：{'、'.join(missing)}")
print("\n".join(errs) if errs else f"agent 格式檢查通過：{n} 個（模版內建 {len(CORE)} 個；{agents_dir}）")
sys.exit(1 if errs else 0)
