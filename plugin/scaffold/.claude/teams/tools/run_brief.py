"""先壓縮、再給模型讀：跑一個指令，完整輸出寫進 log 檔，終端只印重點（token 精簡的 Read less）。

做法參考 caveman（https://github.com/juliusbrussee/caveman，Apache-2.0）的「Read less」：模型讀 log／測試輸出之前先濃縮，
實測整個 session 的輸入 token 少約三成。這裡用自己的寫法、只用標準庫，不安裝也不呼叫 caveman。

用法：
    python3 run_brief.py [--log <檔>] [--name <名>] [--tail <行數>] -- <指令…>
        跑 <指令>（不經 shell；需要 shell 語法時寫成 -- bash -c "…" 或 -- pwsh -c "…"）。
        完整 stdout＋stderr 寫進 log。派工單會指定 --log <run 目錄>/<段>/logs/<label>.log（和執行紀錄放一起留存）；
        沒給時寫到環境變數 AWF_LOG_DIR，再沒有就寫到系統暫存目錄的 awf-logs/<名>-<序號>.log（不寫進目標 repo，避免誤 commit）。
        終端只印：結束碼、偵測到的框架、失敗的測試（最多 20 則）、最後 N 行（預設 20；認不得格式時 40）、log 路徑。
        結束碼原樣傳回（指令找不到時 127）。
    python3 run_brief.py --selftest
agent 的執行紀錄要附 log 路徑，讓使用者需要時能看完整輸出。
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

MAX_FAILS = 20
DEFAULT_TAIL = 20
UNKNOWN_TAIL = 40

# (框架, 偵測輸出的正規式, 抓失敗測試名稱的正規式)
FRAMEWORKS = [
    ("pytest", re.compile(r"=+ .*(passed|failed|error).* in [\d.]+s|^collected \d+ items", re.M),
     re.compile(r"^(?:FAILED|ERROR) (\S+)", re.M)),
    ("jest/vitest", re.compile(r"^\s*Tests?:\s+.*(passed|failed)|^\s*Test Files\s+.*(passed|failed)", re.M),
     re.compile(r"^\s*(?:●|FAIL|×|✕)\s+(.+?)\s*$", re.M)),
    ("go test", re.compile(r"^(?:ok|FAIL|---)\s|^PASS$", re.M),
     re.compile(r"^--- FAIL: (\S+)", re.M)),
    ("unittest", re.compile(r"^Ran \d+ tests? in", re.M),
     re.compile(r"^(?:FAIL|ERROR): (\S+ \(.+?\))", re.M)),
]


def _opt(argv: list[str], name: str, default: str = "") -> str:
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def detect(text: str) -> tuple[str, list[str]]:
    for name, rx, fail_rx in FRAMEWORKS:
        if rx.search(text):
            fails: list[str] = []
            for m in fail_rx.finditer(text):
                f = m.group(1).strip()
                if f and f not in fails:
                    fails.append(f)
            return name, fails
    return "", []


def default_log(name: str) -> Path:
    d = Path(os.environ.get("AWF_LOG_DIR") or Path(tempfile.gettempdir()) / "awf-logs")
    d.mkdir(parents=True, exist_ok=True)
    safe = re.sub(r"[^A-Za-z0-9._-]+", "-", name).strip("-") or "cmd"
    i = 1
    while (d / f"{safe}-{i}.log").exists():
        i += 1
    return d / f"{safe}-{i}.log"


def brief(cmd: list[str], log: Path, tail: int | None = None) -> tuple[int, str]:
    """跑 cmd、寫 log、回 (結束碼, 摘要文字)。"""
    try:
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
        code, text = p.returncode, p.stdout or ""
    except FileNotFoundError as e:
        code, text = 127, f"找不到指令：{e}\n"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text(text, encoding="utf-8")
    fw, fails = detect(text)
    n = tail or (DEFAULT_TAIL if fw else UNKNOWN_TAIL)
    lines = text.rstrip("\n").splitlines()
    out = [f"結束碼：{code}", f"框架：{fw or '認不得（印最後 %d 行）' % n}"]
    if fails:
        out.append(f"失敗的測試（{len(fails)} 則{'，只列前 %d 則' % MAX_FAILS if len(fails) > MAX_FAILS else ''}）：")
        out += [f"  - {f}" for f in fails[:MAX_FAILS]]
    out.append(f"最後 {min(n, len(lines))} 行（共 {len(lines)} 行）：")
    out += [f"  {ln}" for ln in lines[-n:]]
    out.append(f"完整 log：{log}")
    return code, "\n".join(out)


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    if argv[:1] == ["--selftest"]:
        return selftest()
    if "--" not in argv or argv.index("--") == len(argv) - 1:
        print(__doc__)
        return 2
    i = argv.index("--")
    opts, cmd = argv[:i], argv[i + 1:]
    log = Path(_opt(opts, "--log")) if _opt(opts, "--log") else default_log(_opt(opts, "--name") or Path(cmd[0]).name)
    tail = int(_opt(opts, "--tail")) if _opt(opts, "--tail") else None
    code, text = brief(cmd, log, tail)
    print(text)
    return code


SAMPLES = {
    "pytest": "collected 3 items\n\ntests/test_a.py .F.\n\nFAILED tests/test_a.py::test_two - assert 1 == 2\n===== 1 failed, 2 passed in 0.12s =====\n",
    "jest/vitest": " FAIL  src/a.test.ts\n  ● sum › adds\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 4 passed, 5 total\n",
    "go test": "=== RUN   TestAdd\n--- FAIL: TestAdd (0.00s)\n    a_test.go:9: got 1\nFAIL\nFAIL\texample.com/a\t0.01s\n",
}


def selftest() -> int:
    ok = True

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        print(f"  [{'通過' if cond else '失敗'}] {name}" + ("" if cond else f"：{detail[-400:]}"))
        ok = ok and cond

    print("run_brief selftest")
    for fw, sample in SAMPLES.items():
        got, fails = detect(sample)
        check(f"偵測 {fw}", got == fw and len(fails) >= 1, f"{got} {fails}")
    with tempfile.TemporaryDirectory() as tmp:
        t = Path(tmp)
        script = t / "emit.py"
        body = "\n".join(f"line {i}" for i in range(100))
        script.write_text(f"import sys\nprint({body!r})\nsys.exit(3)\n", encoding="utf-8")
        log = t / "logs" / "x.log"
        code, text = brief([sys.executable, str(script)], log)
        check("結束碼原樣傳回", code == 3, text)
        check("完整輸出寫進 log", log.is_file() and log.read_text(encoding="utf-8").count("line ") == 100)
        check("認不得的格式只印最後 40 行", "line 99" in text and "line 59" not in text and "line 60" in text, text)
        check("摘要附 log 路徑", str(log) in text)
        many = t / "many.py"
        many.write_text("print('collected 30 items')\n" + "".join(f"print('FAILED tests/t.py::t{i}')\n" for i in range(30))
                        + "print('===== 30 failed in 1.00s =====')\nimport sys; sys.exit(1)\n", encoding="utf-8")
        code, text = brief([sys.executable, str(many)], t / "many.log")
        check("失敗測試最多列 20 則", code == 1 and "t19" in text.split("最後")[0] and "t25" not in text.split("最後")[0], text)
        code, text = brief(["awf-no-such-command-xyz"], t / "nf.log")
        check("找不到指令時結束碼 127", code == 127, text)
        old = os.environ.get("AWF_LOG_DIR")
        os.environ["AWF_LOG_DIR"] = str(t / "envlogs")
        try:
            rc = main(["--name", "demo", "--", sys.executable, "-c", "print('hi')"])
            check("CLI：沒給 --log 時寫到 AWF_LOG_DIR/<名>-<序號>.log（不寫進目標 repo）", rc == 0 and (t / "envlogs" / "demo-1.log").is_file())
        finally:
            if old is None:
                os.environ.pop("AWF_LOG_DIR", None)
            else:
                os.environ["AWF_LOG_DIR"] = old
    print("run_brief selftest：" + ("全部通過" if ok else "有失敗"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
