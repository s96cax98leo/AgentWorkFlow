#!/usr/bin/env python3
"""錄交付文件（deliverables.py）與工具先定位（locate.py）的對跑基準。

用法：python3 tests/parity/record_deliverables.py > tests/parity/deliverables.expected.json

基準檔本身就是劇本：`steps` 依序是檔案操作（write、rm、rmtree、chmod、git、pandoc）、指令（run：工具、argv、exit code、
stdout、這一步改了哪些檔）與只能在程式內呼叫的函式（call：pack_args、sync 的 only、stamp、matrix_rows、locate_all）。
`probes` 是不碰檔案的函式探測（body_hash、privacy_text、privacy_hits、parse_doc、match_title、strip_paths…）。
Rust 的測試（crates/awf-core/tests/deliverables_parity.rs）照同一份劇本做一遍，逐字比對。

為了連錄兩次相同：
- 暫存路徑換成 `@TMP@`（實體路徑與原路徑都換）。
- 需要 git 的地方用環境變數固定作者、提交者與日期；不讀使用者的 git 設定。
- pandoc 有沒有裝因機器而異：劇本用 `pandoc` 步驟指定（沒有、假的執行檔、不存在的路徑），不看 PATH。
- 兩支工具的輸出都沒有時間戳（run.json 裡的核准時間是劇本寫死的）。
隱私規則的測試內容（信箱、路徑、金鑰的形狀）一律用十六進位存，基準檔與這支腳本都不出現那些字面值。
這支腳本在 Python 版移除時一起刪，基準檔留著。"""
import contextlib
import hashlib
import io
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
TOOLS = ROOT / "scaffold" / ".claude" / "teams" / "tools"
sys.path.insert(0, str(TOOLS))
os.environ.pop("CLAUDE_PROJECT_DIR", None)
import deliverables as dv  # noqa: E402
import locate as loc  # noqa: E402

GIT_ENV = {"GIT_AUTHOR_NAME": "awf", "GIT_AUTHOR_EMAIL": "awf" + "@" + "example.invalid", "GIT_COMMITTER_NAME": "awf",
           "GIT_COMMITTER_EMAIL": "awf" + "@" + "example.invalid", "GIT_AUTHOR_DATE": "2026-01-02T03:04:05Z", "GIT_COMMITTER_DATE": "2026-01-02T03:04:05Z",
           "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
WHICH = shutil.which
LONG = 4000        # 比這長的檔案內容改記 sha1


def hx(s):
    return s.encode("utf-8").hex()


class Rec:
    def __init__(self):
        self.td = tempfile.mkdtemp(prefix="deliv-parity-")
        self.real = os.path.realpath(self.td)
        self.steps = []

    def norm(self, s):
        return s.replace(self.real, "@TMP@").replace(self.td, "@TMP@")

    def p(self, rel):
        return os.path.join(self.td, rel)

    def abs(self, s):
        return s.replace("@TMP@", self.td)

    def read(self, rel):
        return Path(self.p(rel)).read_text(encoding="utf-8")

    def write(self, rel, text, secret=False, repeat=1):
        """secret＝內容有隱私規則會擋的字串：劇本裡用十六進位存。repeat＝內容重複幾次（大檔不必整份放進基準）。"""
        step = {"op": "write", "path": rel}
        step["hex" if secret else "text"] = hx(text) if secret else text
        if repeat != 1:
            step["repeat"] = repeat
        self.steps.append(step)
        os.makedirs(os.path.dirname(self.p(rel)), exist_ok=True)
        with open(self.p(rel), "wb") as f:
            f.write(text.encode("utf-8") * repeat)

    def sub(self, rel, old, new, secret=False, count=1):
        """把檔案裡的 old 換成 new（寫成一個 write 步驟），回傳原本的內容。"""
        keep = self.read(rel)
        assert old in keep, (rel, old)
        self.write(rel, keep.replace(old, new, count), secret)
        return keep

    def rm(self, rel):
        self.steps.append({"op": "rm", "path": rel})
        os.unlink(self.p(rel))

    def rmtree(self, rel):
        self.steps.append({"op": "rmtree", "path": rel})
        shutil.rmtree(self.p(rel))

    def chmod(self, rel, mode):
        self.steps.append({"op": "chmod", "path": rel, "mode": mode})
        os.chmod(self.p(rel), mode)

    def git(self, cwd, *args):
        self.steps.append({"op": "git", "cwd": cwd, "args": list(args)})
        os.makedirs(self.p(cwd), exist_ok=True)
        subprocess.run(["git", *args], cwd=self.p(cwd), env={**os.environ, **GIT_ENV}, check=True, capture_output=True)

    def pandoc(self, value):
        """None：當成沒有裝；字串：這個路徑；"default"：恢復成在 PATH 裡找。"""
        self.steps.append({"op": "pandoc", "value": value})
        shutil.which = WHICH if value == "default" else (lambda name, v=value: self.abs(v) if v else None)

    def snapshot(self):
        out = {}
        for base, dirs, files in os.walk(self.td):
            dirs[:] = sorted(d for d in dirs if d != ".git")
            for f in sorted(files):
                full = os.path.join(base, f)
                rel = os.path.relpath(full, self.td).replace(os.sep, "/")
                text = self.norm(Path(full).read_bytes().decode("utf-8", errors="replace"))
                out[rel] = text if len(text) <= LONG else {"sha1": hashlib.sha1(text.encode("utf-8")).hexdigest(), "chars": len(text)}
        return out

    def changed(self, before):
        after = self.snapshot()
        return {k: after.get(k) for k in sorted(set(before) | set(after)) if before.get(k) != after.get(k)}

    def run(self, tool, *argv):
        before = self.snapshot()
        out, err = io.StringIO(), io.StringIO()
        real = [self.abs(a) for a in argv]
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = dv.main(real, Path(self.p("proj"))) if tool == "deliverables" else loc.main(real)
        self.steps.append({"op": "run", "tool": tool, "argv": list(argv), "code": code, "stdout": self.norm(out.getvalue()),
                           "stderr": self.norm(err.getvalue()), "files": self.changed(before)})
        return out.getvalue()

    def call(self, fn, **kw):
        before = self.snapshot()
        root = Path(self.p("proj"))
        a = {k: (self.abs(v) if isinstance(v, str) else v) for k, v in kw.items()}
        with contextlib.redirect_stdout(io.StringIO()):
            if fn == "pack_args":
                res = dv.pack_args(a["stage"], Path(a["run"]), root, Path(a["repo"]) if a.get("repo") else None, a["data"], a.get("paths"))
                res = [res[0], res[1]]
            elif fn == "sync":
                only = None if a.get("only") is None else set(a["only"])
                res = list(dv.sync(root, Path(a["run"]), Path(a["repo"]) if a.get("repo") else None, a.get("reviewed") or [], only))
            elif fn == "stamp":
                cfg, _ = dv.load_config(root)
                only = None if a.get("only") is None else set(a["only"])
                res = list(dv.stamp(root, cfg, dv.load_catalog(root), None, a.get("reviewed") or [], only))
            elif fn == "matrix_rows":
                res = list(dv.matrix_rows(Path(a["run"])))
            elif fn == "locate_all":
                got = loc.locate_all(Path(a["repo"]), a["text"], Path(a["req_path"]) if a.get("req_path") else None, a["angles"])
                res = [[k, v] for k, v in got.items()]
            else:
                raise SystemExit("不認得的函式：" + fn)
        self.steps.append({"op": "call", "fn": fn, "args": kw, "result": json.loads(self.norm(json.dumps(res, ensure_ascii=False))), "files": self.changed(before)})
        return res


# ------------------------------------------------------------------ 劇本：交付文件
PM = "示範客戶 ACME 的 PM"
RUN = "@TMP@/proj/_tpm/runs/r1"
T = ".claude/teams"
CFG = "proj/%s/deliverables.json" % T
D = "proj/docs/deliverables"
DONE = "已寫好的內容。"
AT = "@"
MAIL = "someone" + AT + "example.com"
HOME_PATH = "/" + "home/demo/app/"
MAC_PATH = "/" + "Users" + "/demo/Documents/x.md"
WIN_PATH = "C:" + "\\" + "Users" + "\\demo\\x"
TOKEN = "gh" + "p_" + "a" * 30
AGENT = "dev" + "-implementer"
AGENT_UP = "TPM" + "-Writer"


def run_json(**over):
    data = {"requirement": "訂單匯出", "target_repo": ".", "args": {"epic": "export"},
            "recipe": {"name": "feature", "modifiers": ["migration"], "resolved": {"name": "feature", "modifiers": ["migration"], "stages": ["plan", "design", "build", "test", "deliver", "merge"]}},
            "stages": {"plan": {"approved": {"by": PM, "at": "2026-01-02T03:04:05Z", "kind": "human"}},
                       "design": {"approved": {"by": "審查者甲", "at": "2026-01-02T08:00:00Z"}},
                       "test": {"approved": {"by": "policy", "at": "2026-01-03T00:00:00Z", "kind": "policy"}},
                       "build": {"status": "done"}, "自訂段｜x": {"approved": {"by": "人", "at": None}}}}
    data.update(over)
    return json.dumps(data, ensure_ascii=False)


def scenario_deliverables(r):
    dl = lambda *a: r.run("deliverables", *a)  # noqa: E731
    r.write("proj/%s/deliverables/catalog.json" % T, json.dumps(json.loads((TOOLS.parent / "deliverables" / "catalog.json").read_text(encoding="utf-8")), ensure_ascii=False))
    r.write("proj/_tpm/runs/r1/run.json", run_json())

    # ---- 沒有交付設定
    for argv in ([], ["--help"], ["-h"], ["status"], ["status", "--json"], ["check"], ["init"], ["index"], ["affected"], ["matrix"], ["sync"],
                 ["affected", "--run", RUN], ["affected", "--run", RUN, "--json"], ["matrix", "--run", RUN], ["matrix", "--run", RUN, "--stdout"],
                 ["sync", "--run", RUN], ["export"], ["nope"], ["configure"], ["configure", "--json"], ["affected", "--run"]):
        dl(*argv)
    r.call("pack_args", stage="deliver", run=RUN, repo=None, data={}, paths=["db/migrations/001.sql"])

    # ---- 設定錯誤時的訊息
    for bad in ("不是 JSON", "[]", '{"documents": "x"}', '{"dir": "/abs", "documents": []}', '{"dir": "../x", "documents": [{"id": "a"}]}',
                json.dumps({"dir": 5, "documents": [{"id": "a"}, {"id": "a", "source": "magic"}, {"id": "B"}, 3, {"id": "b", "sources": ["../x", "/abs", "c:/x", "ok/**"]},
                                                     {"id": "c", "when": "never", "depth": 1, "sections": "x", "on": [""], "design_sections": [1]}, {"id": "d\n"}]}),
                '{"documents": [{"id": "ok", "sources": "a/b"}]}'):
        r.write(CFG, bad)
        dl("status")
        dl("check")
        dl("configure", "--add", "sys-arch")
    dl("init")
    dl("sync", "--run", RUN)
    dl("affected", "--run", RUN, "--json")
    r.rm(CFG)
    for prof in ({"deliverables": {"enabled": False, "documents": [{"id": "db-design"}]}}, {"deliverables": {"enabled": True, "documents": [{"id": "db-design", "source": "x"}]}},
                 {"deliverables": {"enabled": True, "documents": [{"id": "db-design"}, {"id": "my-notes", "title": "自訂筆記", "sections": ["內容"]}]}}):
        r.write("proj/%s/methodology.json" % T, json.dumps(prof, ensure_ascii=False))
        dl("status")
    dl("status", "--json")
    dl("init")
    dl("check")
    r.rmtree("proj/docs")
    r.rm("proj/%s/methodology.json" % T)

    # ---- configure：情境組合
    for p in ("hackathon", "poc", "internal-tool", "outsourced", "ops-handover", "nope"):
        dl("configure", "--preset", p)
    dl("configure", "--preset", "hackathon", "--json")
    dl("configure", "--preset", "hackathon", "--write")
    dl("configure", "--preset")
    # ---- configure：--suggest
    r.write("proj/db/migrations/001_orders.sql", "create table orders (id int);\n")
    r.write("proj/src/app.py", "print('hi')\n")
    dl("configure", "--suggest")
    dl("configure", "--suggest", "--json")
    r.write("proj/api/openapi.yaml", "openapi: 3.0.0\n")
    r.write("proj/jobs/nightly.py", "def run():\n    pass\n")
    r.write("proj/web/App.tsx", "export default function App() { return null }\n")
    r.write("proj/node_modules/pkg/x.proto", "syntax = 'proto3';\n")
    r.write("proj/_tpm/runs/old/run.json", json.dumps({"recipe": {"resolved": {"modifiers": ["ui", "security"]}}}))
    r.write("proj/_tpm/runs/old2/run.json", json.dumps({"recipe": {"modifiers": ["public_api"]}}))
    r.write("proj/_tpm/runs/bad/run.json", "壞掉")
    dl("configure", "--suggest")
    dl("configure", "--suggest", "--json")
    r.write("proj/repos/empty/README.md", "# 空的\n")
    dl("configure", "--suggest", "--repo", "repos/empty")
    dl("configure", "--suggest", "--repo", "@TMP@/proj/repos/empty", "--preset", "poc")
    r.rmtree("proj/_tpm/runs/old")
    r.rmtree("proj/_tpm/runs/old2")
    r.rmtree("proj/_tpm/runs/bad")
    for f in ("proj/api/openapi.yaml", "proj/jobs/nightly.py", "proj/web/App.tsx"):
        r.rm(f)
    r.rmtree("proj/node_modules")
    # ---- configure：--import
    r.write("proj/合約清單.txt", "1. 系統架構設計書\n2. 資料庫規格\n- 測試報告\n3. 專案人員保密切結書\n# 註解\n\n（四）API 規格書\n• 週報\n5、 W B S\n六. 操作手冊\n7) 使用者手冊（含管理者）\n設計\nxy\n* 資料庫\n")
    dl("configure", "--import", "@TMP@/proj/合約清單.txt")
    dl("configure", "--import", "@TMP@/proj/合約清單.txt", "--json")
    dl("configure", "--import", "@TMP@/proj/合約清單.txt", "--custom-unmatched")
    dl("configure", "--import", "@TMP@/proj/合約清單.txt", "--custom-unmatched", "--json")
    dl("configure", "--import", "@TMP@/proj/沒有這個檔.txt")
    dl("configure", "--import", "@TMP@/proj/src")
    # ---- configure：--add／--remove／--dir，沒 --write 不寫入
    dl("configure", "--add", "db-design, api-spec,,nope,sys-arch", "--remove", "sys-arch,不存在")
    dl("configure", "--add", "db-design", "--dir", "docs/交付")
    dl("configure", "--add", "db-design", "--dir", "../outside")
    dl("configure", "--add", "db-design", "--dir", "/abs", "--json")
    dl("configure", "--add", "db-design", "--write", "--by")
    dl("status")
    # ---- configure：--write --by
    dl("configure", "--add", "db-design,api-spec,sys-arch,biz-rules,rtm,dev-record,vuln-scan-report,ops-manual,project-plan", "--write", "--by", PM)
    dl("configure")
    dl("configure", "--json")
    dl("configure", "--remove", "biz-rules", "--add", "nope")
    dl("configure", "--preset", "hackathon", "--suggest")

    # ---- init 與四種狀態
    dl("status")
    dl("check")
    dl("init")
    dl("init")
    dl("status")
    dl("status", "--json")
    dl("check")
    r.write(D + "/external/vuln-scan-report/報告.pdf", "x")
    r.write(D + "/external/vuln-scan-report/.hidden", "x")
    for i in ("db-design", "ops-manual", "sys-arch", "biz-rules", "project-plan", "api-spec"):
        r.sub("%s/%s.md" % (D, i), dv.PLACEHOLDER, DONE, count=-1)
    r.write("proj/specs/export/design.md", "# design\n\n## 資料設計\n\norders 表。\n")
    r.write("proj/specs/export/intent.md", "# intent\n\n匯出訂單。\n")
    r.write("proj/specs/export/spec.md", "# spec\n\n## 匯出\n\n條件一：匯出 CSV。\n")
    r.write("proj/api/openapi.yaml", "openapi: 3.0.0\r\ninfo: {}\r\n")
    dl("status")
    dl("check")
    dl("sync", "--run", RUN)
    dl("status")
    dl("status", "--json")
    dl("check")
    # ---- 過期判定
    r.write("proj/db/migrations/002_items.sql", "create table items (id int);\n")
    dl("status")
    dl("status", "--json")
    dl("check")
    dl("sync", "--run", RUN)
    r.sub(D + "/db-design.md", DONE, DONE + "  \n", count=-1)
    dl("sync", "--run", RUN)
    r.sub(D + "/db-design.md", DONE + "  \n", "已寫好的內容！（Ｏｋ）\n", count=-1)
    dl("sync", "--run", RUN)
    text = r.read(D + "/db-design.md")
    head, sep, tail = text.partition("## 關聯與索引")
    r.write(D + "/db-design.md", head.replace("已寫好的內容！（Ｏｋ）", "orders 與 items 兩張表。") + sep + tail)
    dl("sync", "--run", RUN)
    dl("status")
    dl("sync", "--run", RUN, "--reviewed", "db-design#關聯與索引", "--reviewed", "db-design#資料遷移", "--reviewed", "不存在#小節", "--reviewed")
    dl("status")
    dl("sync", "--run", RUN, "--reviewed", "db-design#資料保存與清理")
    dl("status")
    r.write("proj/db/migrations/001_orders.sql", "create table orders (id int, total int);\n")
    r.rm("proj/db/migrations/002_items.sql")
    dl("status", "--json")
    dl("sync", "--run", RUN, *[x for s in ("資料表與欄位", "關聯與索引", "資料遷移", "資料保存與清理") for x in ("--reviewed", "db-design#" + s)])
    for i in ("rtm", "dev-record"):
        if dv.PLACEHOLDER in r.read("%s/%s.md" % (D, i)):
            r.sub("%s/%s.md" % (D, i), dv.PLACEHOLDER, "之後由工具寫入。", count=-1)
    dl("sync", "--run", RUN)
    dl("check")
    dl("status")

    # ---- check 的每一種問題（改一處、檢查、還原）
    def with_change(doc, old, new, secret=False, extra=()):
        rel = "%s/%s.md" % (D, doc)
        keep = r.sub(rel, old, new, secret)
        dl("check", *extra)
        r.write(rel, keep)

    with_change("sys-arch", "title: 系統架構設計書\n", "")
    with_change("sys-arch", "id: sys-arch", "id: other")
    with_change("sys-arch", "group: design\n", "group:\n")
    with_change("sys-arch", "---\nid: sys-arch", "id: sys-arch")
    r.write("proj/db/migrations/009.sql", "x\n")
    with_change("db-design", "<!-- awf:source", "<!-- 沒了")
    r.rm("proj/db/migrations/009.sql")
    with_change("db-design", "<!-- awf:source", "<!-- awf:source src/**; ../x; /abs;")
    with_change("sys-arch", DONE, "改過但還沒記錄。")
    with_change("sys-arch", DONE, "見 [資料庫](nope.md)、[有](db-design.md#x \"標題\")、[官網](https://example.com/a)、[錨](#x)、[協定](//x/y)、![圖](nope.png)、[信](MAILTO:x)。\n```\n[區塊裡](nope2.md)\n```\n[區塊後](nope3.md)")
    with_change("sys-arch", DONE, DONE.replace("。", " 。"))
    secrets = [("信箱", "聯絡 " + MAIL + "。"), ("全形信箱", "聯絡 ｓｏｍｅｏｎｅ＠ｅｘａｍｐｌｅ．ｃｏｍ"), ("拆行信箱", "聯絡 someone" + AT + "exam\nple.com"),
               ("家目錄", "放在 " + HOME_PATH + " 底下。"), ("mac 路徑", "見 " + MAC_PATH), ("windows 路徑", "見 " + WIN_PATH), ("全形路徑", "放在 ／ｔｍｐ／x"),
               ("金鑰", "金鑰 " + TOKEN + "。"), ("拆行金鑰", "金鑰 gh" + "p_" + "a" * 12 + "\n" + "a" * 12), ("私鑰", "-----BEGIN RSA " + "PRIVATE KEY-----"),
               ("密碼", "password" + " = hunter2hunter"), ("中文密碼", "密碼：" + "hunter2hunter"), ("全形密碼", "ｔｏｋｅｎ：" + "ａｂｃｄｅｆｇｈ"),
               ("agent", "由 " + AGENT + " 完成。"), ("大寫 agent", "由 " + AGENT_UP + " 寫的"), ("全形 agent", "tpm" + "－" + "writer"), ("跳脫的 agent", "tpm" + "\\-" + "skeptic"),
               ("內網", "主機 10." + "1.2.3。"), ("全形內網", "主機 １９２．１６８．" + "０．１"), ("localhost", "http://local" + "host:8080"), ("內網主機", "wiki.corp." + "internal"),
               ("零寬", "some\u200bone" + AT + "exa\u200dmple.com"), ("實體", "someone&#64;example&period;com"), ("多種", MAIL + " 與 " + TOKEN + "\n第二行 " + AGENT)]
    for _, text in secrets:
        with_change("sys-arch", DONE, text, secret=True)
    with_change("sys-arch", DONE, "一般內容：版本 1.2.3、docs/guide.md、https://example.com/a、token 的用法、密碼政策、10 個人。")
    # 禁用字
    r.write(CFG, json.dumps(dict(json.loads(r.read(CFG)), banned_terms_file="docs/禁用字.txt"), ensure_ascii=False))
    dl("check")
    r.write("proj/docs/禁用字.txt", "# 內部代號\nProject\\s*Falcon\n[壞掉\n\n(?i)secret\\w+plan\n\\bGamma\\b\n")
    dl("check")
    with_change("sys-arch", DONE, "代號 Project Falcon。\n沒事的一行\nSecretMasterPlan 與 Gamma、Gammas。")
    r.write("proj/docs/禁用字.txt", "Project\\s*Falcon\n")
    dl("check")
    # privacy_allow
    r.write(CFG, json.dumps(dict(json.loads(r.read(CFG)), privacy_allow=["support@example\\.com", "[壞掉", 5, "^10\\.0\\.0\\.\\d+$"]), ensure_ascii=False))
    with_change("sys-arch", DONE, "客服 support" + AT + "example.com。", secret=True)
    with_change("sys-arch", DONE, "聯絡 other" + AT + "example.com 與 support" + AT + "example.com。", secret=True)
    with_change("sys-arch", DONE, "主機 10.0." + "0.7 可以，10.0." + "1.7 不行。", secret=True)
    # 總目錄
    keep = r.read(D + "/README.md")
    r.write(D + "/README.md", keep + "\n手改\n")
    dl("check")
    dl("index")
    r.write(D + "/README.md", keep.replace("\n", "\r\n"))
    dl("check")
    r.rm(D + "/README.md")
    dl("check")
    dl("index")
    # 文件不存在、外部文件沒附、清單外的檔不檢查
    keep = r.read(D + "/ops-manual.md")
    r.rm(D + "/ops-manual.md")
    r.rm(D + "/external/vuln-scan-report/報告.pdf")
    r.write(D + "/不在設定裡.md", "聯絡 " + MAIL + "\n", secret=True)
    dl("check")
    dl("status")
    r.write(D + "/ops-manual.md", keep)
    r.write(D + "/external/vuln-scan-report/報告.pdf", "x")
    r.rm(D + "/不在設定裡.md")
    # run 目錄名稱與 runs_dir
    r.write("proj/_tpm/runs/2026-11-13-export/run.json", "{}")
    with_change("sys-arch", DONE, "見 2026-11-13-export 的紀錄，放在 _tpm/runs 底下；r1 太短不算。")
    r.write("proj/%s/guard.config.json" % T, json.dumps({"runs_dir": "work/紀錄"}))
    r.write("proj/work/紀錄/20261113-abc/run.json", json.dumps({"recipe": {"resolved": {"modifiers": ["ui"]}}}))
    with_change("sys-arch", DONE, "見 20261113-abc 與 work/紀錄；2026-11-13-export 這時不算。")
    dl("configure", "--suggest")
    r.write("proj/%s/guard.config.json" % T, json.dumps({"runs_dir": "../x"}))
    with_change("sys-arch", DONE, "見 2026-11-13-export。")
    r.rm("proj/%s/guard.config.json" % T)
    r.rmtree("proj/work")
    r.rmtree("proj/_tpm/runs/2026-11-13-export")
    dl("check")

    # ---- affected：各種改動分類、modifier、--milestone、--close
    r.write(CFG, json.dumps(dict(json.loads(r.read(CFG)), documents=json.loads(r.read(CFG))["documents"] + [
        {"id": "integration-spec"}, {"id": "closure-report"}, {"id": "test-cases"}, {"id": "third-party"}, {"id": "dev-guide", "when": "run", "depth": "summary"},
        {"id": "meeting-minutes"}, {"id": "defect-list"}, {"id": "my-notes", "title": "自訂筆記", "sections": ["甲", "乙"], "on": ["doc", "path:notes/**"], "section_on": {"乙": ["path:notes/b/**"]},
                                                                  "sources": ["notes/**"], "design_sections": ["筆記設計"]}]), ensure_ascii=False))
    path_sets = {"migration": ["db/migrations/003_x.sql", "src/app.py"], "code": ["./src/app.py", "  src\\util.py  ", ""], "doc": ["README.md", "docs/x.md"],
                 "test": ["tests/test_a.py"], "manifest": ["package.json"], "agent": [".claude/settings.json", ".github/workflows/ci.yml"],
                 "api": ["api/openapi.yaml", "svc/routes/a.py"], "jobs": ["jobs/nightly.py", "web/App.tsx", "infra/main.tf"], "notes": ["notes/a.md"], "notes-b": ["notes/b/x.md"], "empty": []}
    for name, paths in path_sets.items():
        r.write("proj/paths/%s.txt" % name, "\n".join(paths) + "\n")
        dl("affected", "--run", RUN, "--paths-file", "@TMP@/proj/paths/%s.txt" % name)
        dl("affected", "--run", RUN, "--paths-file", "@TMP@/proj/paths/%s.txt" % name, "--json")
    dl("affected", "--run", RUN, "--paths-file", "@TMP@/proj/paths/api.txt", "--milestone")
    dl("affected", "--run", RUN, "--paths-file", "@TMP@/proj/paths/api.txt", "--close", "--json")
    dl("affected", "--run", RUN, "--paths-file", "@TMP@/proj/paths/沒有.txt")
    dl("affected", "--run", RUN)
    variants = [dict(recipe={"name": "feature", "modifiers": ["public_api", "ui", "security", "large"]}),
                dict(recipe={"name": "bugfix", "resolved": {"name": "hotfix", "modifiers": [], "stages": ["plan", "build"]}}),
                dict(recipe="壞的", stages={"plan": {}, "deliver": {}}, event="meeting"), dict(recipe={"name": "change"}, stages=None, event="reqs-change")]
    for v in variants:
        r.write("proj/_tpm/runs/r2/run.json", run_json(**v))
        dl("affected", "--run", "@TMP@/proj/_tpm/runs/r2", "--paths-file", "@TMP@/proj/paths/code.txt")
        dl("affected", "--run", "@TMP@/proj/_tpm/runs/r2", "--paths-file", "@TMP@/proj/paths/code.txt", "--milestone", "--json")
    r.write("proj/_tpm/runs/r3/run.json", "壞掉的 JSON")
    dl("affected", "--run", "@TMP@/proj/_tpm/runs/r3", "--paths-file", "@TMP@/proj/paths/migration.txt")
    dl("affected", "--run", "@TMP@/proj/_tpm/runs/不存在/", "--paths-file", "@TMP@/proj/paths/migration.txt")
    # 用 git 算改動
    r.write("proj/repos/app/.gitignore", "ignored/\n")
    r.write("proj/repos/app/db/migrations/001.sql", "x\n")
    r.write("proj/repos/app/src/app.py", "print(1)\n")
    r.write("proj/repos/app/ignored/x.sql", "x\n")
    r.git("proj/repos/app", "init", "-q", "-b", "main")
    r.git("proj/repos/app", "add", "-A")
    r.git("proj/repos/app", "commit", "-q", "-m", "base")
    r.git("proj/repos/app", "checkout", "-q", "-b", "feat")
    r.write("proj/repos/app/db/migrations/002.sql", "y\n")
    r.write("proj/repos/app/svc/openapi.json", "{}\n")
    r.write("proj/repos/app/文件/說明.md", "# 說明\n")
    r.git("proj/repos/app", "add", "-A")
    r.git("proj/repos/app", "commit", "-q", "-m", "feat")
    r.write("proj/repos/app/untracked.proto", "x\n")
    r.write("proj/_tpm/runs/r4/run.json", run_json(target_repo="repos/app", args={"epic": "git", "base": "main"}))
    R4 = "@TMP@/proj/_tpm/runs/r4"
    dl("affected", "--run", R4)
    dl("affected", "--run", R4, "--json")
    dl("affected", "--run", R4, "--base", "feat")
    dl("affected", "--run", R4, "--base", "沒有這個分支")
    dl("affected", "--run", R4, "--base", "--output=@TMP@/proj/pwn")
    dl("affected", "--run", RUN, "--repo", "@TMP@/proj/repos/app", "--base", "main~0")
    dl("affected", "--run", RUN, "--repo", "repos/app", "--base", "main", "--paths-file", "@TMP@/proj/paths/doc.txt")
    dl("status", "--run", R4)
    dl("status", "--repo", "repos/app", "--json")
    dl("check", "--run", R4)
    dl("configure", "--suggest", "--repo", "repos/app")
    r.call("pack_args", stage="design", run=RUN, repo=None, data={}, paths=None)
    r.call("pack_args", stage="deliver", run=RUN, repo=None, data={}, paths=["jobs/nightly.py", "api/openapi.yaml", "notes/b/x.md"])
    r.call("pack_args", stage="deliver", run=R4, repo="@TMP@/proj/repos/app", data={"args": {"base": "main"}}, paths=None)
    r.call("pack_args", stage="deliver", run=R4, repo="@TMP@/proj/repos/app", data={"args": "x"}, paths=None)
    r.call("pack_args", stage="deliver", run=RUN, repo=None, data={}, paths=["README.md"])
    r.call("pack_args", stage="build", run=RUN, repo=None, data={}, paths=None)
    r.call("pack_args", stage="deliver", run=RUN, repo=None, data={}, paths=["tests/test_a.py", "package.json"])
    dl("status")

    # ---- matrix：齊全與缺一環（條件的 id 用小寫）
    dl("matrix", "--run", RUN, "--stdout")
    dl("matrix", "--run", RUN)
    r.write("proj/_tpm/runs/r1/design.json", json.dumps({"spec": {"payload": {"subtasks": [
        {"id": "t1", "title": "匯出", "acceptance": ["ac-1 匯出 CSV", "ac-2", 5], "spec_refs": ["spec.md#匯出", "spec.md#格式", 7], "story_ids": ["st-12"]},
        {"id": "t2", "title": "排程", "acceptance": ["ac-3：每天", "ac-1:另一個擁有者"], "spec_refs": [], "story_ids": ["st-13", "st-12"]}, "不是物件"]}}}, ensure_ascii=False))
    r.write("proj/_tpm/runs/r1/test.json", json.dumps({"stage": "test", "verifier": {"status": "ok", "payload": {"commit": "0123456789abcdef0123", "criteria": [
        {"id": "ac-1", "criterion": "匯出 CSV", "command": "pytest tests/test_export.py", "result": "pass", "evidence": "3 passed"},
        {"id": "s1:ac-2", "criterion": "空資料時回空檔｜含 | 直線\n與換行", "command": "pytest -k `empty` " + "x" * 90, "result": "pass", "evidence": "見 logs/out/a.log 與 C:\\tmp\\b.log、./c.txt"},
        {"id": "ac-3", "criterion": "每天 02:00 執行", "command": "pytest -k cron", "result": "fail", "evidence": "1 failed"},
        {"id": "ac-9", "criterion": "沒人認領的條件" * 20, "command": "", "result": "not_verifiable", "evidence": ""},
        {"id": "", "criterion": None, "command": None, "result": "skipped", "evidence": None}, {"criterion": "沒有 id", "result": "不在對照表裡而且很長很長很長很長很長的結果", "command": 5, "evidence": 1.5}, "不是物件"]}}}, ensure_ascii=False))
    dl("matrix", "--run", RUN, "--stdout")
    r.call("matrix_rows", run=RUN)
    dl("matrix", "--run", RUN)
    dl("matrix", "--run", RUN)
    dl("matrix", "--run", "@TMP@/proj/_tpm/runs/r4", "--stdout")
    dl("matrix", "--run", "@TMP@/proj/_tpm/runs/r4")
    r.write("proj/_tpm/runs/r5/run.json", run_json(args={}, epic="  第二個需求  "))
    r.write("proj/_tpm/runs/r5/build.json", json.dumps({"result": {"subtasks": [{"id": 9, "acceptance": ["c1"], "spec_refs": ["a.md#x"]}]}}))
    r.write("proj/_tpm/runs/r5/test.json", json.dumps({"combined": {"commit": "n/a", "criteria": [{"id": "c1", "criterion": "直接的物件", "command": "make test", "result": "pass", "evidence": "ok"}]}}))
    dl("matrix", "--run", "@TMP@/proj/_tpm/runs/r5", "--stdout")
    dl("matrix", "--run", "@TMP@/proj/_tpm/runs/r5")
    r.write("proj/_tpm/runs/r6/run.json", run_json(args={"epic": "有隱私"}))
    r.write("proj/_tpm/runs/r6/test.json", json.dumps({"payload": {"commit": "abc", "criteria": [{"id": "c1", "criterion": "寄給 " + MAIL, "command": "x", "result": "pass", "evidence": "y"}]}}), secret=True)
    dl("matrix", "--run", "@TMP@/proj/_tpm/runs/r6")
    keep = r.sub(D + "/rtm.md", "<!-- awf:entry-end " + dv.entry_key(Path("r1")) + " -->", "（結尾標記不見了）")
    dl("matrix", "--run", RUN)
    r.write(D + "/rtm.md", keep)
    dl("check")

    # ---- sync：一般、重跑、--reviewed、清單外改動、隱私命中不寫
    SUM = "proj/_tpm/runs/r1/deliver/" + dv.SUMMARY_NAME
    r.write(SUM, "# 交付摘要\n\n## 這次交付了什麼\n\n訂單可以匯出成 CSV。\n\n### 細節\n\n#### 第四層\n\n#不是標題\n# \n")
    dl("sync", "--run", RUN)
    dl("sync", "--run", RUN)
    dl("check")
    r.write(SUM, "# 交付摘要\r\n\r\n有問題請寄 " + MAIL + "。\r\n", secret=True)
    dl("sync", "--run", RUN)
    r.write(SUM, "沒有大標題的摘要。\n")
    dl("sync", "--run", RUN)
    r.rm(SUM)
    dl("sync", "--run", RUN)
    r.write("proj/_tpm/runs/r5/deliver/" + dv.SUMMARY_NAME, "#  第二次\n\n內容。\n")
    dl("sync", "--run", "@TMP@/proj/_tpm/runs/r5")
    dl("sync", "--run", "@TMP@/proj/_tpm/runs/r3")
    dl("sync", "--run", "@TMP@/proj/_tpm/runs/不存在")
    dl("sync", "--run", "@TMP@/proj/_tpm/runs/r6")
    r.write(CFG, json.dumps(dict(json.loads(r.read(CFG)), show_approvers=True), ensure_ascii=False))
    dl("sync", "--run", RUN)
    r.write(CFG, json.dumps({k: v for k, v in json.loads(r.read(CFG)).items() if k != "show_approvers"}, ensure_ascii=False))
    dl("sync", "--run", RUN)
    # 清單外改動（只能在程式內給 only）
    r.write("proj/db/migrations/012.sql", "x\n")
    keep = r.read(D + "/db-design.md")
    r.write(D + "/db-design.md", keep.replace("orders 與 items 兩張表。", "orders、items 與稽核表。", 1).replace("已寫好的內容！（Ｏｋ）", "改了清單外的小節。", 1))
    r.call("stamp", reviewed=[], only=["db-design#資料表與欄位"])
    r.call("sync", run=RUN, repo=None, reviewed=["db-design#資料遷移"], only=["db-design#關聯與索引"])
    r.call("sync", run=RUN, repo=None, reviewed=[], only=[])
    dl("status")
    dl("sync", "--run", RUN, "--reviewed", "db-design#資料保存與清理")
    dl("check")
    keep = r.sub(D + "/dev-record.md", "<!-- awf:entry-end " + dv.entry_key(Path("r1")) + " -->", "")
    dl("sync", "--run", RUN)
    r.write(D + "/dev-record.md", keep)
    # 設定裡沒有開發紀錄與追溯矩陣
    full = r.read(CFG)
    r.write(CFG, json.dumps(dict(json.loads(full), documents=[{"id": "sys-arch"}, {"id": "ops-manual"}]), ensure_ascii=False))
    dl("sync", "--run", RUN)
    dl("matrix", "--run", RUN)
    dl("status")
    dl("check")
    r.write(CFG, full)
    dl("index")

    # ---- export
    r.pandoc(None)
    dl("export")
    r.write("proj/bin/pandoc-ok.sh", "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then printf docx > \"$2\"; fi\n  shift\ndone\n")
    r.chmod("proj/bin/pandoc-ok.sh", 0o755)
    r.write("proj/bin/pandoc-bad.sh", "#!/bin/sh\necho '  轉檔錯誤：不支援這個格式  ' >&2\nexit 3\n")
    r.chmod("proj/bin/pandoc-bad.sh", 0o755)
    r.pandoc("@TMP@/proj/bin/pandoc-ok.sh")
    dl("export")
    dl("export", "--out", "@TMP@/proj/out/docx")
    r.pandoc("@TMP@/proj/bin/pandoc-bad.sh")
    dl("export", "--out", "@TMP@/proj/out/bad")
    r.pandoc("@TMP@/proj/bin/沒有這個執行檔")
    dl("export", "--out", "@TMP@/proj/out/none")
    r.pandoc("default")

    # ---- 自訂目錄與自訂文件
    r.write(CFG, json.dumps({"dir": "docs/交付//", "repo": "repos/app", "confirmed_by": PM, "documents": [
        {"id": "db-design", "title": "資料庫文件（自訂標題）", "when": "milestone", "sections": ["表"], "group": "不存在的組"},
        {"id": "my-notes", "title": "筆記：a/b*c?", "sections": ["內容"], "sources": ["文件/**"]}, {"id": "ext-1", "source": "external", "title": None, "group": ""},
        {"id": "plain"}]}, ensure_ascii=False))
    dl("init")
    dl("status")
    dl("status", "--json")
    dl("check")
    r.pandoc("@TMP@/proj/bin/pandoc-ok.sh")
    dl("export")
    r.pandoc("default")
    dl("configure")
    dl("configure", "--json")
    dl("configure", "--remove", "plain", "--add", "rtm", "--write", "--by", "審查者甲")
    dl("sync", "--run", "@TMP@/proj/_tpm/runs/r4")
    dl("status")
    dl("deliverables-不存在的指令")
    dl("--selftest")


# ------------------------------------------------------------------ 劇本：工具先定位
REQ = "訂單匯出支援日期區間與客戶時區：GET /exports/orders.csv 接受 from、to；大量匯出改走背景批次，完成後通知申請人。"
REQS = [REQ, "export_orders_csv", "訂單匯出", "How does the invoice batch export work with timezone handling?", "invoice 發票 開立與 billing 對帳的規則",
        "完全無關的主題：量子糾纏與黑洞蒸發 xyzzy", "怎麼了嗎", "的", "x", "", "   ", "ADR 匯出格式 CSV UTC", "load_orders 的引用關係與 helper_3", "新人須知 開發環境 測試",
        "Ｅｘｐｏｒｔ ＣＳＶ ｆｕｌｌｗｉｄｔｈ", "訂單" * 300 + " orders " + "匯出 " * 200]


def locate_files(r, base):
    filler = "".join("def helper_%d():\n    return %d\n\n" % (i, i) for i in range(60))
    files = {
        "services/orders/export.py": "from .loader import load_orders\n" + filler + "def export_orders_csv(tenant, date_from=None, date_to=None):\n    \"\"\"訂單匯出：依日期區間篩選，時間欄位輸出 UTC。\"\"\"\n"
                                     "    rows = load_orders(tenant)\n    return to_csv(rows)\n" + filler,
        "services/orders/routes.py": "ROUTES = {\n    'GET /exports/orders.csv': 'export_orders_csv',\n}\n\ndef register(app):\n    for k, v in ROUTES.items():\n        app.add(k, v)\n",
        "services/orders/batch.py": "def enqueue_export(tenant):\n    \"\"\"大量匯出改走背景批次：排入佇列，完成後通知申請人。\"\"\"\n    queue.put(('export_orders_csv', tenant))\n",
        "services/orders/loader.py": "def load_orders(tenant):\n    return []\n",
        "services/orders/__init__.py": "",
        "services/billing/invoice.py": "".join("def invoice_%d():\n    return %d\n\n" % (i, i) for i in range(80)) + "# 發票開立：billing 對帳，時區 timezone 用客戶設定\n",
        "services/billing/timezone_utils.py": "import invoice\n\ndef to_customer_timezone(ts, tz):\n    \"\"\"客戶時區轉換。\"\"\"\n    return ts\n",
        "tests/test_misc.py": "from services.orders.export import export_orders_csv\n\ndef test_it():\n    assert True\n",
        "docs/adr/0003-export.md": "# ADR 3：訂單匯出格式\n\n匯出一律用 CSV，時間欄位輸出 UTC。\n\n## 決策\n\n日期區間由呼叫端指定。\n",
        "docs/adr/0010-export-0.md": "# ADR 10：訂單匯出的日期區間\n\n大量匯出改走背景批次，完成後通知申請人。\n",
        "docs/adr/0011-timezone.md": "# ADR 11：客戶時區\r\n\r\n匯出的時間欄位依客戶時區顯示。\r\nGET /exports/orders.csv 接受 from、to。\r\n",
        "specs/export/spec.md": "# 訂單匯出\n\n## 驗收條件\n\n- 條件一 匯出檔名帶日期區間\n- 條件二 超過同步上限改走背景批次\n" + "".join("\n第 %d 段補充說明，和主題無關的填充文字。\n" % i for i in range(70)) + "\n## 附錄\n\n客戶時區的處理見 ADR 11。\n",
        "docs/onboarding.md": "# 新人須知\n\n先裝開發環境，再跑測試。\n",
        "docs/notes.txt": "訂單匯出的雜記：orders export。\n",
        "README.rst": "訂單系統\n========\n\n匯出與發票。\n",
        "guide.adoc": "= 指南\n\n訂單匯出的操作方式。\n",
        ".env": "SECRET=訂單匯出 export_orders_csv\n",
        "package-lock.json": '{"orders": "export_orders_csv 訂單匯出"}\n',
        "web/app.min.js": "var export_orders_csv=1;\n",
        "node_modules/pkg/index.js": "export_orders_csv 訂單匯出\n",
        "empty.py": "",
        "data/sep.py": "a = 1\x0cb = 2\x1cexport_orders_csv\u2028c = 3\rd = 4\n",
    }
    for rel, text in files.items():
        r.write("%s/%s" % (base, rel), text)
    r.steps.append({"op": "write", "path": base + "/blob.bin", "hex": (b"\0\1export_orders_csv" * 50).hex()})
    Path(r.p(base + "/blob.bin")).write_bytes(b"\0\1export_orders_csv" * 50)
    r.steps.append({"op": "write", "path": base + "/bad-utf8.py", "hex": b"# \xff\xfe export_orders_csv \xe8\xa8\n".hex()})
    Path(r.p(base + "/bad-utf8.py")).write_bytes(b"# \xff\xfe export_orders_csv \xe8\xa8\n")


def scenario_locate(r):
    lc = lambda *a: r.run("locate", *a)  # noqa: E731
    for argv in ([], ["--help"], ["-h", "--repo", "x"], ["--requirement", "x"], ["--repo"], ["--repo", "@TMP@/沒有這個目錄", "--requirement", REQ],
                 ["--repo", "@TMP@/沒有這個目錄", "--requirement", REQ, "--json"]):
        lc(*argv)
    locate_files(r, "loc")
    for base in ("loc",):
        for q in REQS:
            lc("--repo", "@TMP@/" + base, "--requirement", q)
            lc("--repo", "@TMP@/" + base, "--requirement", q, "--json")
        for a in loc.ANGLES + ("nope",):
            lc("--repo", "@TMP@/" + base, "--requirement", REQ, "--angle", a)
            lc("--repo", "@TMP@/" + base, "--requirement", REQS[3], "--angle", a, "--json")
        lc("--repo", "@TMP@/" + base, "--requirement", REQ, "--angle", "specs-adr", "--angle", "code-contracts", "--angle", "specs-adr", "--angle")
        lc("--repo", "@TMP@/" + base, "--requirement")
    r.write("ws/requirement.md", "# 需求\n\n" + REQ + "\n")
    r.write("loc/docs/req-inside.md", "# 需求（在 repo 裡）\n\n" + REQ + "\n")
    r.call("locate_all", repo="@TMP@/loc", text=REQ, req_path="@TMP@/ws/requirement.md", angles=list(loc.ANGLES))
    r.call("locate_all", repo="@TMP@/loc", text=REQ, req_path="@TMP@/loc/docs/req-inside.md", angles=["requirement", "nope"])
    r.call("locate_all", repo="@TMP@/loc", text="  ", req_path=None, angles=list(loc.ANGLES))
    r.call("locate_all", repo="@TMP@/沒有", text=REQ, req_path=None, angles=list(loc.ANGLES))
    # 種子擴展：拿掉引用關係再看一次
    r.sub("loc/services/orders/export.py", "from .loader import load_orders\n", "")
    r.rm("loc/tests/test_misc.py")
    lc("--repo", "@TMP@/loc", "--requirement", REQ, "--angle", "code-contracts", "--json")
    # 太大的檔、很多小檔
    r.write("loc/big.py", "export_orders_csv = 1\n", repeat=20000)
    for i in range(30):
        r.write("loc/many/mod_%02d.py" % i, "# 訂單匯出 export 模組 %d\ndef export_part_%d():\n    return %d\n" % (i, i, i))
    lc("--repo", "@TMP@/loc", "--requirement", REQ, "--json")
    lc("--repo", "@TMP@/loc", "--requirement", "export 模組", "--angle", "code-contracts")
    # git repo：尊重 .gitignore
    r.write("loc/.gitignore", "many/\nignored.md\n")
    r.write("loc/ignored.md", "# 訂單匯出\n\n被忽略的檔。\n")
    r.write("loc/docs/未追蹤.md", "# 訂單匯出（還沒 add）\n\n日期區間。\n")
    r.git("loc", "init", "-q", "-b", "main")
    r.git("loc", "add", "services", "docs/adr", "specs", ".gitignore")
    r.git("loc", "commit", "-q", "-m", "init")
    for q in REQS[:5]:
        lc("--repo", "@TMP@/loc", "--requirement", q, "--json")
    lc("--repo", "@TMP@/loc", "--requirement", REQ)
    lc("--selftest")


# ------------------------------------------------------------------ 函式探測
def fuzz_privacy():
    """固定種子的隨機組合：把會命中與不會命中的片段接起來。"""
    rnd = random.Random(20261113)
    parts = ["聯絡", "someone", AT, "＠", "example.com", "example", ".", "．", "com", " ", "\n", "\t", "\u3000", "\u200b", "\x1c", "&#64;", "&amp;", "&commat;", "&#x40;", "&nbsp;",
             "/" + "tmp/", "~/", "/" + "root/", "/var/" + "folders/", "/private/" + "var/", "c:", "\\", "\\\\", "Users", "users", "USERS", "srv", "share$",
             "gh" + "p_", "github_" + "pat_", "s" + "k-", "AK" + "IA", "xox" + "b-", "AI" + "za", "ey" + "J", "a" * 20, "A" * 16, "0" * 10, "bearer ", "Bearer ", "BEARER\t",
             "hooks.slack" + ".com/services/", "password", "PASSWORD", "passwd", "pwd", "secret", "api_key", "api-key", "apikey", "access_key", "token", "client-secret", "密碼", "密鑰", "金鑰",
             ":", "=", "：", "＝", "\"", "'", "hunter2", "hunter2hunter", "tpm", "dev", "TPM", "Dev", "-", "_", "－", "\\-", "--", "writer", "Writer", "skeptic", "spec-author", "spec_author",
             "merge-steward", "pr-reviewer", "wr\u0131ter", "wr\u0130ter", "10.", "192.168.", "172.16.", "172.32.", "127.0.", "1.2.3", "0.1", "１０．", "١٠.", "localhost", "LOCALHOST",
             "wiki", ".corp", ".internal", ".intranet", ".lan", ".local", ".localdomain", ".locale", "fd00:", "fc12:", "fe80:", "ab:cd:ef", "中", "文", "。", "，", "x", "1", "é", "ß", "K", "ſ",
             "PASSWORD\u0130D", "pr\u0131vate", "/pr\u0131vate/tmp/", "f\u0131le", "ema\u0131l", "-----BEGIN ", "PRIVATE KEY-----", "RSA ", "<", ">", "（", "）", "|", "#", ";", "&", "&#", "&#x", "&#12;", "&#55357;", "&notit;", "&lt"]
    out = []
    for _ in range(int(os.environ.get("AWF_PARITY_FUZZ") or 1500)):     # 要多跑一些時用環境變數加大（不存進基準）
        out.append("".join(rnd.choice(parts) for _ in range(rnd.randint(2, 9))))
    return out


def probes():
    cat = dv.load_json(TOOLS.parent / "deliverables" / "catalog.json")
    pr = {}
    bodies = ["", " ", "內文", "內 文", "內文。", "內文．", "ＡＢＣ１２３", "ABC123", "abc123", "a-b_c", "a b\tc\n", "café", "cafe\u0301", "①②③ ½ x²", "㈱ ｶﾞ ㌔", "\u200b零寬\ufeff", "表｜格|線", "ﬁ ﬂ ǆ",
              "emoji 😀 👍🏽", "數學 ∑ ± × ÷ ≠", "貨幣 $ € ¥ ￥", "箭頭 → ⇒", "한국어 텍스트", "かな カタカナ ｶﾀｶﾅ", "العربية ١٢٣", "हिन्दी", "ไทย", "\x1c\x1d\x1e\x1f\x85\xa0", "\u2028\u2029", "ªº µ", "Ⅳ ⅳ",
              "orders 與 items 兩張表。", "orders 與 items 兩張表 。", "orders、items 與稽核表。", "`code` *md* [連結](a.md) <!-- 註解 -->", "\ue000私用\U000f0000", "\u0378 未指派", "a\u0300\u0301b", "ｧｨｩ", "〇一二三", "々〆〤",
              "\u1e9b\u0323", "\u2126 \u212a \u212b", "\ufdfa", "\u3300", "0\ufe0f\u20e3", "🇹🇼"]
    pr["body_hash"] = [[hx(b), dv.body_hash(b)] for b in bodies]
    texts = ["", "plain", "a＠b．ｃｏｍ", "a\u200b\u200c\u200d\u200e\u200f\u2060\u2061\u2064\ufeff\u00adb", "a\u2065b\u2066b", "&#64; &#x40; &#X40 &commat; &commat &amp;amp; &lt;b&gt;", "&#0; &#128; &#x80 &#150; &#xD800; &#x110000; &#1; &#11; &#65534; &#99999999999999999999;",
             "&notit; &notin; &not; &noti &no &; & &# &#x &#; &#xZ; &#12a &#x1g", "&" + "a" * 40 + ";", "&AMP &AMP; &amp &ampx &Amp;", "tpm\\-writer \\- \\\\-", "ｔｐｍ－ｗｒｉｔｅｒ", "ﬁle ½ ㎏ ㈱", "行一\r\n行二\r行三", "&hellip; &nbsp;x &zwnj; &shy; &ZeroWidthSpace;",
             "&lt &gt &quot &apos; &copy &reg &para &sect &middot &times &divide &eth &thorn", "&period; &colon; &sol; &bsol; &lowbar; &hyphen; &dash; &num; &dollar; &percnt;", "&bne; &nvlt; &fjlig; &ThickSpace;", "x & y &z; &\t; &<;"]
    pr["privacy_text"] = [[hx(t), hx(dv.privacy_text(t))] for t in texts]
    fixed = [MAIL, "a" + AT + "b", "a" + AT + "b.c", "a " + AT + " b.c", "a\x1c" + AT + "\x1fb.c", "聯絡a" + AT + "b.c。", "a" + AT + "\nb.c", "x\na" + AT + "b\n.c\ny", HOME_PATH, MAC_PATH, WIN_PATH, WIN_PATH.lower(), "C:\\\\" + "Users" + "\\\\demo",
             "~/work", "x~/work", "/" + "tmp/x", "/" + "TMP/x", "/var/" + "FOLDERS/a", "\\\\server\\share$", "\\\\伺服器\\分享", "\\\\a\\", TOKEN, "x" + TOKEN, "_" + TOKEN, "中" + TOKEN, "-" + TOKEN, "gh" + "p_" + "a" * 19, "github_" + "pat_" + "A_" * 12,
             "s" + "k-" + "a-b_" * 6, "AK" + "IA" + "A1" * 8, "AK" + "IA" + "A1" * 8 + "X", "中AK" + "IA" + "A1" * 8 + "中", "xox" + "b-" + "a1-" * 5, "AI" + "za" + "x" * 30, "ey" + "J" + "a" * 8 + "." + "b" * 8 + "." + "c" * 8, "ey" + "J" + "a" * 8 + "." + "b" * 7 + "." + "c" * 8,
             "hooks.slack" + ".com/services/T0/B0", "Bearer " + "a" * 16, "bearer\n" + "a" * 16, "xbearer " + "a" * 16, "BEARER  " + "a/b+c~d." * 3, "-----BEGIN " + "PRIVATE KEY-----", "-----BEGIN EC " + "PRIVATE KEY-----", "-----BEGIN rsa " + "PRIVATE KEY-----",
             "password" + "=hunter2", "password" + "=hunter", "Password" + ": 'hunter2hunter'", "passwords" + "=hunter2x", "my_password" + "=hunter2x", "mypassword" + "=hunter2x", "中password中" + "=hunter2x", "password中" + "=hunter2x", "api key" + "=abcdefg", "api_key" + "\t=\t\"abcdef\"",
             "access-key" + ":<abcdef>", "client_secret" + "＝abcdefgh", "token" + ": （待補）xxxxxx", "token" + ": xx（待補）", "密碼" + "是 hunter2hunter", "金鑰" + "=abcdef", "密鑰" + "：\"abc def\"", "PASSW\u0130RD" + "=hunter2x", "pwd" + "=" + "中" * 6, "secret" + "='12345'",
             AGENT, AGENT_UP, "tpm" + "_writer", "tpm" + " - " + "writer", "tpm" + "--" + "writer", "tpm" + "---" + "writer", "tpm" + "\\" + "writer", "tpm" + "writer", "xtpm" + "-writer", "tpm" + "-writers", "中tpm" + "-writer中", "tpm" + "-spec_author", "dev" + "_pr-reviewer",
             "tpm" + "-wr\u0131ter", "tpm" + "-WR\u0130TER", "dev" + "\n-\nverifier", "10." + "1.2.3", "10." + "1.2", "10." + "1.2.3.4", "110." + "1.2.3", "10." + "1.2.3x", "x10." + "1.2.3", "中10." + "1.2.3中", "192.168" + ".0.1", "172.15" + ".0.1", "172.16" + ".0.1", "172.31" + ".255.255", "172.32" + ".0.1",
             "127.0" + ".0.1", "10." + "1234.2.3", "١٠." + "١.٢.٣", "１０." + "１.２.３", "local" + "host", "LOCAL" + "HOST", "xlocal" + "host", "local" + "hosts", "中local" + "host中", "a.inter" + "nal", "a.b-c.d.l" + "an", "x.loc" + "al", "x.loc" + "ale", "x.loc" + "al中", ".inter" + "nal", "中.co" + "rp", "a_b.co" + "rp",
             "fd" + "00:1234", "fc" + "ab:1:2", "fd" + "0:1234", "xfd" + "00:1234", "FD" + "00:ABCD", "fe" + "80:1234", "訂單匯出 CSV，版本 1.2.3，見 docs/guide.md 與 https://example.com/a。", "", "\n\n", "行一\r\n" + MAIL + "\r\n行三", MAIL + "\n" + MAIL + "\n" + TOKEN + " " + AGENT,
             "a" + AT + "exam\nple.com", "a\n" + AT + "\nb.c", "gh" + "p_" + "a" * 10 + " \n " + "a" * 10, "pass\nword" + "=hunter2x", "tpm\n" + "-writer", "10.1\n" + ".2.3", "someone&#64;example.com", "ｓｏｍｅｏｎｅ＠ｅｘａｍｐｌｅ．ｃｏｍ", "some\u200bone" + AT + "example.com"]
    cases = [[t, [], None] for t in fixed + fuzz_privacy()]
    cases += [[MAIL, ["someone"], None], [MAIL, ["^other"], None], [MAIL + " other" + AT + "example.com", ["^someone@"], None], [MAIL, ["[壞", 5, None, "example\\.com$"], None], [MAIL, ["(?i)SOMEONE"], None],
              ["10." + "0.0.7 與 10." + "0.1.7", ["^10\\.0\\.0\\.\\d+$"], None], [AGENT, ["\\bdev\\b"], None], ["a" + AT + "exam\nple.com", ["ample"], None], [TOKEN, ["\\w{30}\\Z"], None], [MAIL, ["\\<x"], None], [MAIL, ["[[s]omeone"], None],
              ["見 2026-11-13-export 的紀錄", [], ["2026-11-13-export", "_tpm/runs"]], ["放在 _tpm/runs 底下", [], ["_tpm/runs"]], ["a.b(c)", [], ["a.b(c)", "x"]], ["axb(c)", [], ["a.b(c)"]], ["_tpm/runs 與 " + MAIL, ["_tpm"], ["_tpm/runs"]]]
    out = []
    for text, allow, names in cases:
        extra = [("內部 run 目錄", dv.re.compile("|".join(dv.re.escape(n) for n in names)))] if names else []
        out.append({"text": hx(text), "allow": allow, "names": names, "hits": dv.privacy_hits(text, allow, extra)})
    pr["privacy_hits"] = out
    docs = ["", "沒有檔頭\n## 節\n內文", "---\nid: a\ntitle:  標題  \n 縮排: x\nbad line\nk-e_y9 : v : w\n9x: no\n中文: no\n_ok:\n---\n# 大標\n前言\n## 一\n<!-- awf:source a/**; b ;; c -->\n\n內文一\n### 小節\n更多\n## 二  \n\n\n<!--awf:source-->\n（待補）\n## 三\n先有內文\n<!-- awf:source x -->\n",
            "---\nid: a\n", "---\r\nid: a\r\n---\r\n## 節\r\n<!-- awf:source a -->\r\n內\r\n", "## 節\n```\n## 不是標題\n<!-- awf:source x -->\n```\n ```\n## 是標題\n   <!--   awf:source   p q ; r   -->   \n<!-- awf:source 第二個 -->\n", "##沒空格\n## \n##  兩個空格\n### 三級\n## 尾\n",
            "## 節\n\n \n\t\n<!-- awf:source late -->\n內文\n", "## 節\n<!-- awf:sourcex -->\n<!-- awf:source-->\n<!-- awf:source; -->\n", "## 節\n<!-- AWF:SOURCE x -->\n<!-- awf:source a -->x\n", "---\n---\n## 節\n", " ---\nid: x\n---\n## 節\n", "---\nid: a\rb\n---\n## 節\x0c\n內\x1c文\n"]
    pr["parse_doc"] = [{"text": t, "doc": dv.parse_doc(t)} for t in docs]
    pr["doc_sections"] = [{"text": t, "sources": s, "doc": dv.doc_sections({"sources": s} if s is not None else {}, t)} for t in docs[2:6] for s in (None, [], ["a/**", "z", 5], ["b"])]
    lines = ["1. 系統架構設計書", "2. 資料庫規格", "- 測試報告", "3. 專案人員保密切結書", "", "  ", "（四）API 規格書", "(五) 週報", "• 週報", "5、 W B S", "六. 操作手冊", "7) 使用者手冊（含管理者）", "設計", "xy", "* 資料庫", "一、需求規格書", "十二）上線檢查表",
             "A P I 規 格 書", "api-spec", "API_SPEC", "系統架構", "架構", "書", "測試", "測試計畫書與測試報告", "需求追溯矩陣（RTM）", "RTM", "rtm", "１. 系統架構設計書", "١. 弱點掃描報告", "#3 風險", "【交付】原始碼掃描報告", "「維運手冊」", "Ｗｂｓ", "İndex", "移交文件．清冊"]
    pr["match_title"] = [[ln, dv.match_title(ln, cat)] for ln in lines]
    paths = [None, "", 5, 1.5, True, ["x"], "見 /var/" + "folders/xy/T/a.log 與 _tpm/runs/r9/test/verify.md", "C:\\tmp\\b.log", "c:/x/y.z", "./a", "a/", "a/b", "a//b///c.txt", "~/x/y", "$HOME/x.y", "http://example.com/a/b.html?q=1", "中文/路徑/檔.md", "a/中.md 與 b\\c\\d-e.f",
             "pytest tests/test_export.py -k 'a/b'", "無路徑", "1/2", "a / b", "x/.hidden", "x/..", "../..", "/", "//", "a\\", "\\\\srv\\share\\f.txt", "ａ／ｂ", "é/ü.txt", "a_b/c-d/e.f.g", "a|b/c"]
    pr["strip_paths"] = [[v, dv.strip_paths(v)] for v in paths]
    globs = [["**/migrations/**", ["db/migrations/001.sql", "migrations/a", "a/b/migrations/c/d", "migrations", "db/migrationsx/a", "xmigrations/a"]], ["**/*.sql", ["a.sql", "x/a.sql", "x/y/a.sql", "a.sqlx", ".sql", "a.sql\n", "a\nb.sql"]],
             ["specs/**/design.md", ["specs/design.md", "specs/a/design.md", "specs/a/b/design.md", "xspecs/a/design.md", "specs/a/xdesign.md"]], ["./docs/*.md", ["docs/a.md", "docs/a/b.md", "./docs/a.md", "docs/.md"]], ["a?c", ["abc", "a/c", "ac", "a中c"]],
             ["**", ["", "a", "a/b"]], ["a**b", ["ab", "axb", "a/x/b"]], ["***", ["a/b"]], [".github/workflows/**", [".github/workflows/ci.yml", "x/.github/a", "xgithub/workflows/a"]], ["a.b+c(d)[e]{f}^$|\\", ["a.b+c(d)[e]{f}^$|/", "axb+c(d)[e]{f}^$|/"]],
             ["  docs\\**  ", ["docs/a", "docs"]], ["*", ["a", "a/b", ""]], ["**/", ["a/", "a", ""]], ["檔*.md", ["檔案.md", "檔/案.md"]]]
    pr["glob"] = [{"pattern": g, "paths": [[p, bool(dv.glob_re(g).match(p))] for p in ps]} for g, ps in globs]
    rels = [None, "", 0, 5, True, "docs", " docs/x/ ", "docs\\x", "/abs", "c:/x", "C:x", "a/../b", "..", "a//b", "./a", "a/./b", ".", "docs/交付//", "a/b/", "~", "a b", "\ta\n"]
    pr["safe_rel"] = [[v, dv.safe_rel(v)] for v in rels]
    cfgs = [None, [], "x", {}, {"documents": None}, {"documents": []}, {"dir": "", "documents": []}, {"dir": None, "documents": []}, {"dir": "docs/交付", "documents": [{"id": "db-design", "when": "milestone", "depth": "summary"}]},
            {"documents": [{"id": "a"}, {"id": "a", "source": "magic"}, {"id": "b", "sources": ["../x"]}]}, {"documents": [1, "x", {}, {"id": 5}, {"id": ""}, {"id": "A"}, {"id": "-a"}, {"id": "a" * 64}, {"id": "a" * 65}, {"id": "a\n"}, {"id": "a\n\n"}, {"id": "9-x"}]},
            {"documents": [{"id": "a", "source": None, "when": 1, "depth": ["full"]}, {"id": "b", "sections": [], "sources": [""], "on": ["x", " "], "design_sections": "x"}, {"id": "c", "sources": ["/a", "c:\\x", "a\\..\\b", "ok", 5]},
                           {"id": "d", "sources": "a/../b"}, {"id": "e", "sources": {"/k": 1}}, {"id": "f", "source": "auto", "when": "close", "depth": "full", "sections": ["甲"]}]}]
    pr["config_errors"] = [[c, dv.config_errors(c)] for c in cfgs]
    sk = [{"id": "x"}, {"id": "x", "title": "標題", "group": "g", "source": "auto", "depth": "summary", "sections": ["甲", "乙"], "sources": ["a/**", "b"]}, {"id": "e", "source": "external", "title": "", "sections": ["不會出現"]},
          {"id": "n", "title": 5, "group": None, "source": "", "sections": []}]
    pr["skeleton"] = [[d, dv.skeleton(d, {})] for d in sk]
    icfgs = [{"documents": []}, {"documents": [{"id": "db-design"}, {"id": "vuln-scan-report"}, {"id": "my", "title": "自訂"}, {"id": "g2", "group": "不存在", "when": "weird", "source": "assisted"}, {"id": "pm1", "group": "pm", "title": None},
                           {"id": "sys-arch", "when": "close", "source": "external"}, 5, {"id": "rtm", "title": ""}]}]
    pr["index_text"] = [[c, dv.index_text(c, cat)] for c in icfgs]
    pr["resolve_docs"] = [[c, dv.resolve_docs(c, cat)] for c in icfgs]
    ups = [["", "k1", "內容", "節"], ["# 文件\n\n## 節\n<!-- awf:source -->\n\n（待補）\n\n## 下一節\n\n內文\n", "k1", "### 標題\n\n| a |\n", "節"], ["## 節\n\n舊內文\n\n\n", "k1", "新\n\n", "節"], ["## 節\n<!-- awf:entry k1 -->\n舊\n<!-- awf:entry-end k1 -->\n尾\n", "k1", "新一\n新二", "節"],
           ["## 節\n  <!-- awf:entry k1 -->  \n舊\n<!-- awf:entry-end k1 -->\n<!-- awf:entry k1 -->\n舊二\n<!-- awf:entry-end k1 -->", "k1", "新", "節"], ["## 節\n<!-- awf:entry k1 -->\n舊\n\n## 重要\n", "k1", "新", "節"], ["## 別的\n內文", "k1", "新", "節"],
           ["## 節 \n內文\r\n## 節\r\n第二個\r\n", "k2", "新", "節"], ["##  節\n", "k1", "", "節"], ["## 節\n<!-- awf:entry k2 -->\n別人的\n<!-- awf:entry-end k2 -->\n", "k1", "新", "節"], ["\n\n", "k", "x", ""]]
    res = []
    for text, key, block, heading in ups:
        try:
            res.append({"args": [text, key, block, heading], "ok": dv.upsert_entry(text, key, block, heading)})
        except ValueError as e:
            res.append({"args": [text, key, block, heading], "err": str(e)})
    pr["upsert_entry"] = res
    datas = [json.loads(run_json()), {}, {"args": "x", "epic": 7, "stages": {"plan": {"approved": {"by": "p" * 60, "at": 20260102}}, "x": "y", "merge": {"approved": {"kind": "policy"}}, "sync": {"approved": {"by": "policy", "at": ""}}, "deliver": {"approved": {"by": None, "at": "2026-01-02 長字串"}}}},
             {"args": {"epic": "  史詩｜一  "}, "stages": []}, {"epic": "直接的", "stages": {"很長的自訂段名稱超過二十個字元的情況要截斷喔喔喔": {"approved": {}}}}]
    sums = ["", "# 交付摘要\n\n## 這次交付了什麼\n\n訂單可以匯出成 CSV。\n", "  \n# 標題\n", "#標題\n內文", "# \n\n# 第二個大標\n## 二\n### 三\n#### 四\n##### 五\n###### 六\n#\t七\n ## 縮排\n", "沒有標題\r\n## 節\r\n", "#\n\n內文", "# 只有標題", "## 直接二級\n# 之後的一級\n", "#\u3000全形空白\n內"]
    pr["record_block"] = [{"data": d, "summary": s, "show": show, "out": dv.record_block(Path("r"), d, s, show)} for d in datas for s in (sums if d is datas[0] else sums[:2]) for show in (False, True)]
    runs = ["r1", "a/b/r1", "a/b/r1/", "2026-11-13-export", ".", "", "/", "中文 run", "a/..", "x/./y"]
    pr["entry_key"] = [[x, dv.entry_key(Path(x))] for x in runs]
    return pr


def main():
    warnings.simplefilter("ignore")     # 壞掉的正規式會讓 re 發警告，和輸出無關
    r = Rec()
    try:
        scenario_deliverables(r)
        scenario_locate(r)
        out = {"steps": r.steps, "probes": probes()}
    finally:
        shutil.which = WHICH
        shutil.rmtree(r.td, ignore_errors=True)
    json.dump(out, sys.stdout, ensure_ascii=False, indent=1, sort_keys=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
