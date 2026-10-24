#!/usr/bin/env python3
"""逐一確認 references/tokens.css 的每個「變數: 值」都能在正本找到原字串（一字不差）。

用法：python3 scripts/verify-tokens.py [tokens.css] [正本檔]
預設讀本 skill 的 references/，正本檔名取自 references/source.conf 的 SOURCE。
全部找得到、正本 SHA-256 與 SHA256SUMS 相符、（有設 TOKEN_RANGE 時）第 1 部分與正本該段整段相同，才結束碼 0。
沒有 HTML 正本（SOURCE 留空）時，只檢查 tokens.css 沒有重複宣告，並提示改用人工比對。
"""
import hashlib
import re
import sys
from pathlib import Path

ref = Path(__file__).resolve().parents[1] / "references"
conf = {}
if (ref / "source.conf").exists():
    for line in (ref / "source.conf").read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            conf[k.strip()] = v.strip()

tokens_path = Path(sys.argv[1]) if len(sys.argv) > 1 else ref / "tokens.css"
src = sys.argv[2] if len(sys.argv) > 2 else conf.get("SOURCE", "")
css = tokens_path.read_text(encoding="utf-8")
body = re.sub(r"/\*.*?\*/", lambda m: " " * len(m.group(0)), css, flags=re.S)  # 去註解、保留位置
decls = [m.group(0) for m in re.finditer(r"(--[a-zA-Z0-9-]+)\s*:\s*[^;{}]+(?=;)", body)]
cut = css.find("第 2 部分")
names1 = re.findall(r"(--[a-zA-Z0-9-]+)\s*:", body[:cut] if cut >= 0 else body)
part2 = re.findall(r"(--[a-zA-Z0-9-]+)\s*:", body[cut:]) if cut >= 0 else []

if not decls:
    print("tokens.css 還沒有任何宣告（仍是骨架）：先照 SKILL.md 從正本擷取")
    sys.exit(1)

if not src:
    dup = sorted({n for n in names1 if names1.count(n) > 1})
    print(f"沒有設定正本（source.conf 的 SOURCE 是空的）：宣告 {len(decls)} 個；同名重複 {len(dup)} 個 {dup}")
    print("請人工對照設計正本；能取得 HTML／CSS 正本時，填 source.conf 再跑一次")
    sys.exit(0)

ds_path = Path(src) if Path(src).is_absolute() else ref / src
errs = []
sums = (ref / "SHA256SUMS").read_text(encoding="utf-8").split()
digest = hashlib.sha256(ds_path.read_bytes()).hexdigest()
if not sums or sums[0] != digest:
    errs.append(f"正本 SHA-256 不符：SHA256SUMS={sums[0] if sums else '(空)'}，實際={digest}")

ds = ds_path.read_text(encoding="utf-8")
missing = [raw for raw in decls if raw not in ds]
errs += [f"正本找不到：{raw}" for raw in missing]

rng = conf.get("TOKEN_RANGE", "")
same = None
if rng:
    a, _, b = rng.partition("-")
    block = "\n".join(ds.splitlines()[int(a) - 1:int(b or a)])
    same = block in css
    if not same:
        errs.append(f"tokens.css 第 1 部分與正本 DS:{rng} 不是整段相同")

print(f"正本：{ds_path.name}  SHA-256 {digest}")
print(f"宣告總數 {len(decls)}；找得到 {len(decls) - len(missing)}；找不到 {len(missing)}")
print(f"第 1 部分：{len(names1)} 個宣告＝{len(set(names1))} 個變數＋同名覆寫 {len(names1) - len(set(names1))} 個")
print(f"第 2 部分（別名，正本 :root 外）：{len(part2)} 個" + (f"：{', '.join(part2)}" if part2 else ""))
if same is not None:
    print(f"DS:{rng} 整段比對：" + ("相同" if same else "不同"))
if errs:
    print("\n".join(errs))
    sys.exit(1)
print("結果：全部找得到")
