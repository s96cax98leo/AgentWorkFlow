"""驗證一份交件信封：schema＋交接語意規則。

用法：
    python3 check_envelope.py <file.json> [--payload findings]
    echo '<回報文字，最後一個 ```json 區塊是信封>' | python3 check_envelope.py - [--payload ...]
不帶 --payload 時，若信封裡有 "payload_type" 以外的提示也不猜，只驗信封本體。
通過 exit 0；不通過 exit 1，原因印到 stderr。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import schemalib  # noqa: E402


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    payload = None
    if "--payload" in argv:
        payload = argv[argv.index("--payload") + 1]
    src = argv[0]
    text = sys.stdin.read() if src == "-" else Path(src).read_text(encoding="utf-8")
    env = schemalib.extract_json(text)
    if env is None:
        print("找不到交件信封：回報最後要有一個 ```json 區塊（格式見 .claude/teams/schemas/envelope.schema.json）", file=sys.stderr)
        return 1
    errs = schemalib.check(env, payload)
    if errs:
        print("交件信封不合格（.claude/teams/tools/check_envelope.py）：", file=sys.stderr)
        for e in errs:
            print(f"- {e}", file=sys.stderr)
        return 1
    print(f"信封合格：status={env['status']}，confidence={env['confidence']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
