"""掃描產出是否違反硬規則（.claude/teams/rules.md），只做機械判定。設定讀 .claude/teams/guard.config.json。

用法：
    python3 check_rules.py [--audience internal|owner|external] <檔案…>
    python3 check_rules.py --git <repo> <range>        # 檢查 commit 訊息，例如 origin/main..HEAD
audience：internal＝內部文件；owner＝給決策者版；external＝對外文件。
規則：
  R1 不寫人天、人月、工時估算（出現「數字＋人天」這類估算才算；「不寫人天」這種規則敘述不算）
  R2 不自編任務編號：出現的任務編號必須已在 backlog_file（設定檔，預設 _tpm/tasks/backlog.md，格式 id:<編號>）
  R3 給決策者或對外的文件不寫內部編號（任務編號、internal_id_patterns）
  R4 commit 要有署名行 attribution_line 的 trailer，不用 forbidden_trailers（預設 Co-Authored-By）
  R6 commit 標題要符合開發規範 profile（.claude/teams/methodology.json）的 conventions.commit.check_regex；沒設就不檢查格式
  R5 給決策者或對外的文件不評價人
違規 exit 1。
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402

WORKSPACE = Path(__file__).resolve().parents[3]
CFG = schemalib.config()
BACKLOG = WORKSPACE / (CFG.get("backlog_file") or "_tpm/tasks/backlog.md")
ATTRIBUTION = (CFG.get("attribution_line") or "Assisted-by: Claude").strip()
ATTR_KEY = ATTRIBUTION.split(":", 1)[0].strip()
FORBIDDEN_TRAILERS = [t for t in (CFG.get("forbidden_trailers") or ["Co-Authored-By"]) if t.lower() != ATTR_KEY.lower()]
METHODOLOGY = schemalib.TEAMS_DIR / "methodology.json"

ESTIMATE = re.compile(r"\d+(?:\.\d+)?\s*(?:個)?\s*(?:人天|人日|人月|man[- ]?days?|person[- ]?days?)|(?:預估|估計|約)\s*\d+(?:\.\d+)?\s*(?:小時|天)\s*(?:工時|工作量)", re.I)
TNUM = schemalib.task_id_regex()
INTERNAL = [re.compile(p) for p in (CFG.get("internal_id_patterns") or [])]
JUDGE = re.compile(r"能力不足|不專業|偷懶|很差勁|太弱|不夠聰明|擺爛|不負責任")
OUTWARD = ("owner", "external")


def known_ids() -> set[str]:
    if not BACKLOG.exists() or not TNUM:
        return set()
    return {m.group(0) for m in TNUM.finditer(" ".join(re.findall(r"id:(\S+)", BACKLOG.read_text(encoding="utf-8"))))}


def scan_file(path: Path, audience: str, ids: set[str]) -> list[str]:
    out = []
    for n, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        loc = f"{path}:{n}"
        if ESTIMATE.search(line):
            out.append(f"R1 {loc}：工時估算「{ESTIMATE.search(line).group(0)}」")
        for t in (m.group(0) for m in TNUM.finditer(line)) if TNUM else ():
            if audience in OUTWARD:
                out.append(f"R3 {loc}：給 {audience} 的文件不寫內部編號 {t}")
            elif ids and t not in ids:
                out.append(f"R2 {loc}：{t} 不在 backlog，疑似自編任務編號")
        if audience in OUTWARD:
            for pat in INTERNAL:
                for d in pat.findall(line):
                    out.append(f"R3 {loc}：給 {audience} 的文件不寫內部編號 {d}")
            if JUDGE.search(line):
                out.append(f"R5 {loc}：評價人的用語「{JUDGE.search(line).group(0)}」")
        for tr in FORBIDDEN_TRAILERS:
            if re.search(rf"^{re.escape(tr)}:", line, re.I):
                out.append(f"R4 {loc}：用 {ATTR_KEY}，不用 {tr}")
    return out


def commit_regex(path: Path = METHODOLOGY) -> re.Pattern | None:
    """開發規範 profile 的 commit 標題格式；沒有 profile、沒設或設 null 時回 None（只檢查署名行）。"""
    try:
        rx = ((json.loads(path.read_text(encoding="utf-8")).get("conventions") or {}).get("commit") or {}).get("check_regex")
    except (OSError, ValueError, AttributeError):
        return None
    if not isinstance(rx, str) or not rx.strip():
        return None
    try:
        return re.compile(rx)
    except re.error:
        print(f"警告：{path} 的 conventions.commit.check_regex 不是合法的正規式，略過格式檢查")
        return None


def scan_git(repo: str, rng: str) -> list[str]:
    log = subprocess.run(["git", "-C", repo, "log", "--format=%h%x00%B%x1e", rng], capture_output=True, text=True, check=True).stdout
    out = []
    subject_rx = commit_regex()
    for rec in filter(str.strip, log.split("\x1e")):
        sha, _, body = rec.strip().partition("\x00")
        subject = body.strip().splitlines()[0] if body.strip() else ""
        if subject_rx and not subject_rx.search(subject):
            out.append(f"R6 commit {sha}：標題「{subject[:60]}」不符合開發規範 profile 的 commit 格式（{subject_rx.pattern}）")
        for tr in FORBIDDEN_TRAILERS:
            if re.search(rf"^{re.escape(tr)}:", body, re.I | re.M):
                out.append(f"R4 commit {sha}：用 {ATTR_KEY}，不用 {tr}")
        if not re.search(rf"^{re.escape(ATTR_KEY)}:", body, re.I | re.M):
            out.append(f"R4 commit {sha}：缺 {ATTR_KEY} trailer（署名行：{ATTRIBUTION}）")
    return out


def main(argv: list[str]) -> int:
    if argv[:1] == ["--git"]:
        errs = scan_git(argv[1], argv[2])
    else:
        audience = "internal"
        if "--audience" in argv:
            i = argv.index("--audience")
            audience = argv[i + 1]
            argv = argv[:i] + argv[i + 2:]
        if audience not in ("internal", *OUTWARD):
            print(f"--audience 只能是 internal、owner、external（收到 {audience}）")
            return 2
        ids = known_ids()
        errs = [e for f in argv for e in scan_file(Path(f), audience, ids)]
    for e in errs:
        print(e)
    print(f"規則檢查：{'通過' if not errs else f'{len(errs)} 項違規'}")
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
