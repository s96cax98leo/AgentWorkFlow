"""產生基準用的虛構專案（示範客戶 ACME 的訂單中心）。內容全部虛構、完全由這支腳本決定：同一版腳本產出的檔案逐位元組相同。

為什麼要有：量 token 要有一個固定的情境才能前後比較。真實專案的 transcript 不能進版控，所以用一個規模接近的虛構 repo：
約 67 個檔、六千多行，其中三份文件超過 400 行（讀取預算的整檔門檻），讓「整檔讀大檔」的成本顯現得出來。

用法：
  python3 tests/bench/make_fixture.py <目標目錄> [--no-git]
      目標目錄必須不存在或是空的。預設會 git init 並做一個 commit（固定作者與時間，commit 雜湊可重現）。
  python3 tests/bench/make_fixture.py --selftest
      產生兩次並比對每個檔的 sha1，確認可重現；檢查規模與大檔數量。

只用標準庫；跨平台（一律寫 LF）。
"""
from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path

# 領域：名稱、中文名、欄位（欄位名, 型別, 說明）
DOMAINS = [
    ("orders", "訂單", [("order_no", "str", "訂單編號"), ("customer_id", "str", "客戶代號"), ("placed_at", "datetime", "下單時間（UTC）"),
                      ("status", "str", "狀態"), ("total_cents", "int", "總金額（分）"), ("currency", "str", "幣別"), ("channel", "str", "通路")]),
    ("customers", "客戶", [("customer_id", "str", "客戶代號"), ("name", "str", "名稱"), ("tier", "str", "等級"), ("timezone", "str", "時區"),
                         ("created_at", "datetime", "建立時間（UTC）"), ("email_opt_in", "bool", "是否收信")]),
    ("inventory", "庫存", [("sku", "str", "料號"), ("warehouse", "str", "倉別"), ("on_hand", "int", "現有量"), ("reserved", "int", "保留量"),
                         ("updated_at", "datetime", "更新時間（UTC）")]),
    ("billing", "帳務", [("invoice_no", "str", "發票號碼"), ("order_no", "str", "訂單編號"), ("issued_at", "datetime", "開立時間（UTC）"),
                       ("amount_cents", "int", "金額（分）"), ("tax_cents", "int", "稅額（分）"), ("state", "str", "狀態")]),
    ("shipping", "出貨", [("shipment_no", "str", "出貨單號"), ("order_no", "str", "訂單編號"), ("carrier", "str", "物流商"),
                        ("shipped_at", "datetime", "出貨時間（UTC）"), ("delivered_at", "datetime", "送達時間（UTC）"), ("state", "str", "狀態")]),
    ("notifications", "通知", [("notice_id", "str", "通知編號"), ("recipient", "str", "收件者"), ("template", "str", "範本"),
                             ("queued_at", "datetime", "排入時間（UTC）"), ("sent_at", "datetime", "送出時間（UTC）"), ("state", "str", "狀態")]),
    ("reports", "報表", [("report_id", "str", "報表編號"), ("kind", "str", "種類"), ("requested_by", "str", "申請人"),
                       ("requested_at", "datetime", "申請時間（UTC）"), ("rows", "int", "筆數"), ("state", "str", "狀態")]),
    ("jobs", "批次作業", [("job_id", "str", "作業編號"), ("kind", "str", "種類"), ("payload", "str", "參數（JSON）"),
                       ("enqueued_at", "datetime", "排入時間（UTC）"), ("finished_at", "datetime", "完成時間（UTC）"), ("state", "str", "狀態")]),
]
LAYERS = ("models", "repository", "service", "handlers", "validators")
STATES = ("pending", "active", "done", "failed", "cancelled")


def _cls(domain: str) -> str:
    return domain[:-1].capitalize() if domain.endswith("s") else domain.capitalize()


def gen_models(d, zh, fields) -> str:
    c = _cls(d)
    out = [f'"""{zh}的資料模型。時間欄位一律存 UTC，顯示時才轉時區。"""', "from __future__ import annotations", "",
           "from dataclasses import dataclass, field", "from datetime import datetime", "", "",
           "@dataclass", f"class {c}:", f'    """一筆{zh}。"""', ""]
    for n, t, doc in fields:
        out += [f"    {n}: {t}  # {doc}"]
    out += ["    tags: list[str] = field(default_factory=list)", "", "    def to_row(self) -> dict:",
            f'        """轉成匯出用的一列；鍵的順序就是欄位順序。"""', "        return {"]
    for n, t, _ in fields:
        v = f"self.{n}.isoformat()" if t == "datetime" else f"self.{n}"
        out += [f'            "{n}": {v},']
    out += ["        }", ""]
    for n, t, doc in fields:
        out += [f"    def with_{n}(self, value: {t}) -> \"{c}\":", f'        """回傳改了{doc}的新物件，不改原物件。"""',
                f"        data = dict(self.__dict__)", f'        data["{n}"] = value', f"        return {c}(**data)", ""]
    out += ["", f"{d.upper()}_STATES = {STATES!r}", ""]
    return "\n".join(out)


def gen_repository(d, zh, fields) -> str:
    c = _cls(d)
    key = fields[0][0]
    out = [f'"""{zh}的資料存取。示範用的記憶體實作；正式環境由 storage 模組換成資料庫。"""', "from __future__ import annotations", "",
           "from datetime import datetime", "from typing import Iterable, Iterator", "", f"from .models import {c}", "", "",
           f"class {c}Repository:", f'    """{zh}的查詢與寫入。查詢一律回傳 iterator，呼叫端自己決定要不要全部載入。"""', "",
           "    def __init__(self) -> None:", f"        self._items: dict[str, {c}] = {{}}", "",
           f"    def save(self, item: {c}) -> None:", f"        self._items[item.{key}] = item", "",
           f"    def get(self, key: str) -> {c} | None:", "        return self._items.get(key)", "",
           f"    def all(self) -> Iterator[{c}]:", "        yield from self._items.values()", ""]
    for n, t, doc in fields:
        if t == "datetime":
            out += [f"    def between_{n}(self, start: datetime, end: datetime) -> Iterator[{c}]:",
                    f'        """{doc}落在 [start, end) 的{zh}。start、end 必須是 UTC。"""',
                    "        for item in self._items.values():", f"            if start <= item.{n} < end:", "                yield item", ""]
        elif t == "str":
            out += [f"    def by_{n}(self, value: str) -> Iterator[{c}]:", f'        """依{doc}查詢。"""',
                    "        for item in self._items.values():", f"            if item.{n} == value:", "                yield item", "",
                    f"    def count_by_{n}(self) -> dict[str, int]:", f'        """各{doc}的筆數。"""', "        out: dict[str, int] = {}",
                    "        for item in self._items.values():", f"            out[item.{n}] = out.get(item.{n}, 0) + 1", "        return out", ""]
        elif t == "int":
            out += [f"    def sum_{n}(self, items: Iterable[{c}] | None = None) -> int:", f'        """{doc}的加總。"""',
                    f"        return sum(i.{n} for i in (items if items is not None else self._items.values()))", ""]
    return "\n".join(out)


def gen_service(d, zh, fields) -> str:
    c = _cls(d)
    out = [f'"""{zh}的業務邏輯。規則的出處見 docs/業務規則.md 的〈{zh}〉一節。"""', "from __future__ import annotations", "",
           "from datetime import datetime, timedelta", "", f"from .models import {c}", f"from .repository import {c}Repository",
           f"from .validators import validate_{d}", "", "", f"class {c}Service:", f'    """{zh}的用例。"""', "",
           f"    def __init__(self, repo: {c}Repository) -> None:", "        self.repo = repo", "",
           f"    def create(self, item: {c}) -> {c}:", f"        errors = validate_{d}(item)", "        if errors:",
           '            raise ValueError("；".join(errors))', "        self.repo.save(item)", "        return item", ""]
    for i, s in enumerate(STATES):
        out += [f"    def mark_{s}(self, key: str) -> {c}:", f'        """把{zh}改成 {s}。找不到就丟 KeyError。"""',
                "        item = self.repo.get(key)", "        if item is None:", "            raise KeyError(key)",
                f'        changed = item.with_{fields[-1][0]}("{s}")' if fields[-1][1] == "str" else "        changed = item",
                "        self.repo.save(changed)", "        return changed", ""]
    for n, t, doc in fields:
        if t == "datetime":
            out += [f"    def recent_by_{n}(self, now: datetime, days: int = 7) -> list[{c}]:",
                    f'        """最近 days 天（以{doc}計）的{zh}。now 必須是 UTC。"""',
                    f"        return list(self.repo.between_{n}(now - timedelta(days=days), now))", "",
                    f"    def daily_counts_by_{n}(self, start: datetime, end: datetime) -> dict[str, int]:",
                    f'        """[start, end) 內每天的筆數，日期用 UTC 切。"""', "        out: dict[str, int] = {}",
                    f"        for item in self.repo.between_{n}(start, end):", f"            day = item.{n}.date().isoformat()",
                    "            out[day] = out.get(day, 0) + 1", "        return out", ""]
    return "\n".join(out)


def gen_handlers(d, zh, fields) -> str:
    c = _cls(d)
    out = [f'"""{zh}的 HTTP 處理。路由表在 api/routes.py；回應格式見 docs/API規格.md。"""', "from __future__ import annotations", "",
           "from datetime import datetime", "", f"from .service import {c}Service", "", "",
           "def _parse_utc(text: str) -> datetime:", '    """只接受 ISO 8601 且帶 Z 的時間。"""', '    if not text.endswith("Z"):',
           '        raise ValueError("時間要用 UTC（結尾 Z）")', '    return datetime.fromisoformat(text[:-1])', "", ""]
    out += [f"def get_{d}(service: {c}Service, key: str) -> tuple[int, dict]:", "    item = service.repo.get(key)", "    if item is None:",
            '        return 404, {"error": "not_found", "key": key}', "    return 200, item.to_row()", "", "",
            f"def list_{d}(service: {c}Service, query: dict) -> tuple[int, dict]:", f'    """列出{zh}。支援 limit、offset。"""',
            '    limit = min(int(query.get("limit", 50)), 200)', '    offset = int(query.get("offset", 0))',
            "    rows = [i.to_row() for i in service.repo.all()]",
            '    return 200, {"total": len(rows), "items": rows[offset:offset + limit]}', "", ""]
    for n, t, doc in fields:
        if t == "datetime":
            out += [f"def list_{d}_by_{n}(service: {c}Service, query: dict) -> tuple[int, dict]:",
                    f'    """依{doc}的區間列出{zh}。from、to 必填，都是 UTC。"""', "    try:",
                    '        start = _parse_utc(query["from"])', '        end = _parse_utc(query["to"])',
                    "    except (KeyError, ValueError) as e:", '        return 400, {"error": "bad_range", "detail": str(e)}',
                    "    if end <= start:", '        return 400, {"error": "bad_range", "detail": "to 要晚於 from"}',
                    f"    rows = [i.to_row() for i in service.repo.between_{n}(start, end)]",
                    '    return 200, {"total": len(rows), "items": rows}', "", ""]
        elif t == "str":
            out += [f"def list_{d}_by_{n}(service: {c}Service, value: str) -> tuple[int, dict]:",
                    f"    rows = [i.to_row() for i in service.repo.by_{n}(value)]", '    return 200, {"total": len(rows), "items": rows}', "", ""]
    return "\n".join(out)


def gen_validators(d, zh, fields) -> str:
    c = _cls(d)
    out = [f'"""{zh}的欄位檢查。回傳錯誤訊息清單，空清單表示通過。"""', "from __future__ import annotations", "", f"from .models import {c}", "", "",
           f"def validate_{d}(item: {c}) -> list[str]:", "    errors: list[str] = []"]
    for n, t, doc in fields:
        if t == "str":
            out += [f"    if not item.{n} or len(item.{n}) > 64:", f'        errors.append("{doc}必填，最長 64 字")']
        elif t == "int":
            out += [f"    if item.{n} < 0:", f'        errors.append("{doc}不能是負數")']
        elif t == "datetime":
            out += [f"    if item.{n}.tzinfo is not None:", f'        errors.append("{doc}要存成不帶時區的 UTC")']
    out += ["    return errors", ""]
    return "\n".join(out)


GEN = {"models": gen_models, "repository": gen_repository, "service": gen_service, "handlers": gen_handlers, "validators": gen_validators}

EXPORT_CSV = '''"""訂單匯出（CSV）。目前只有「全部匯出」：一次把所有訂單載進記憶體再寫檔。

已知限制（見 docs/維運手冊.md〈匯出逾時〉）：
- 沒有日期區間，客服只能整包下載再自己篩。
- 時間欄位直接輸出 UTC，客戶端常誤讀成當地時間。
- 訂單超過約五萬筆時，同步匯出會超過閘道的 30 秒逾時。
"""
from __future__ import annotations

import csv
import io

from ..orders.repository import OrderRepository

EXPORT_COLUMNS = ("order_no", "customer_id", "placed_at", "status", "total_cents", "currency", "channel")
SYNC_ROW_LIMIT = 50000  # 超過就該改走背景批次（尚未實作）


def export_orders_csv(repo: OrderRepository) -> str:
    """把全部訂單匯出成 CSV 字串。第一列是欄位名。"""
    buf = io.StringIO()
    writer = csv.DictWriter(buf, fieldnames=EXPORT_COLUMNS, lineterminator="\\n")
    writer.writeheader()
    for order in sorted(repo.all(), key=lambda o: o.placed_at):
        row = order.to_row()
        writer.writerow({k: row[k] for k in EXPORT_COLUMNS})
    return buf.getvalue()


def export_filename(prefix: str = "orders") -> str:
    """下載檔名。目前固定，不帶日期區間。"""
    return f"{prefix}-all.csv"
'''

EXPORT_HANDLER = '''"""匯出的 HTTP 處理。GET /exports/orders.csv 直接回傳整份 CSV。"""
from __future__ import annotations

from ..orders.repository import OrderRepository
from .csv_export import export_filename, export_orders_csv


def download_orders(repo: OrderRepository, query: dict) -> tuple[int, dict, str]:
    """回傳（狀態碼、標頭、內容）。query 目前沒有用到任何參數。"""
    body = export_orders_csv(repo)
    headers = {"Content-Type": "text/csv; charset=utf-8",
               "Content-Disposition": f'attachment; filename="{export_filename()}"'}
    return 200, headers, body
'''

TIMEZONE = '''"""時區換算。客戶的時區存在 customers.timezone（IANA 名稱）；系統內一律用不帶時區的 UTC。"""
from __future__ import annotations

from datetime import datetime, timezone
from zoneinfo import ZoneInfo


def to_local(utc_naive: datetime, tz_name: str) -> datetime:
    """UTC（不帶時區）轉成指定時區的當地時間（帶時區）。"""
    return utc_naive.replace(tzinfo=timezone.utc).astimezone(ZoneInfo(tz_name))


def to_utc_naive(local: datetime, tz_name: str) -> datetime:
    """當地時間轉成不帶時區的 UTC。local 不帶時區時視為 tz_name 的時間。"""
    if local.tzinfo is None:
        local = local.replace(tzinfo=ZoneInfo(tz_name))
    return local.astimezone(timezone.utc).replace(tzinfo=None)


def day_range_utc(day: str, tz_name: str) -> tuple[datetime, datetime]:
    """某個當地日期（YYYY-MM-DD）對應的 UTC 區間 [start, end)。"""
    start_local = datetime.fromisoformat(day).replace(tzinfo=ZoneInfo(tz_name))
    end_local = start_local.replace(hour=23, minute=59, second=59, microsecond=999999)
    start = start_local.astimezone(timezone.utc).replace(tzinfo=None)
    end = end_local.astimezone(timezone.utc).replace(tzinfo=None)
    return start, end
'''

JOB_QUEUE = '''"""背景批次的佇列（示範用的記憶體實作）。排程與重試規則見 docs/批次作業.md。"""
from __future__ import annotations

import json
from datetime import datetime

from ..jobs.models import Job
from ..jobs.repository import JobRepository

KINDS = ("nightly_report", "inventory_sync", "invoice_batch")  # 允許排入的作業種類
MAX_ATTEMPTS = 3


def enqueue(repo: JobRepository, kind: str, payload: dict, now: datetime) -> Job:
    """排入一個作業。kind 不在允許清單就丟 ValueError。"""
    if kind not in KINDS:
        raise ValueError(f"未知的作業種類：{kind}")
    job = Job(job_id=f"{kind}-{now.strftime('%Y%m%d%H%M%S')}", kind=kind, payload=json.dumps(payload, ensure_ascii=False),
              enqueued_at=now, finished_at=now, state="pending")
    repo.save(job)
    return job


def run_next(repo: JobRepository, handlers: dict, now: datetime) -> Job | None:
    """取一個 pending 的作業執行。handlers 是 {kind: callable(payload) -> None}。"""
    for job in repo.by_state("pending"):
        handler = handlers.get(job.kind)
        if handler is None:
            repo.save(job.with_state("failed"))
            return job
        try:
            handler(json.loads(job.payload))
        except Exception:  # noqa: BLE001 — 作業失敗只記狀態，不讓佇列停下
            repo.save(job.with_state("failed"))
            return job
        repo.save(job.with_state("done").with_finished_at(now))
        return job
    return None
'''

ROUTES_HEAD = '''"""路由表：路徑 → 處理函式。新增端點要同步更新 docs/API規格.md。"""
from __future__ import annotations

ROUTES = [
    ("GET", "/exports/orders.csv", "acme_orders.export.handlers:download_orders"),
'''

TEST_EXPORT = '''"""匯出的測試。"""
from datetime import datetime

from acme_orders.export.csv_export import EXPORT_COLUMNS, export_filename, export_orders_csv
from acme_orders.orders.models import Order
from acme_orders.orders.repository import OrderRepository


def _order(no: str, at: datetime) -> Order:
    return Order(order_no=no, customer_id="C001", placed_at=at, status="done", total_cents=1200, currency="TWD", channel="web")


def test_export_has_header_and_sorted_rows():
    repo = OrderRepository()
    repo.save(_order("B", datetime(2026, 1, 2)))
    repo.save(_order("A", datetime(2026, 1, 1)))
    lines = export_orders_csv(repo).splitlines()
    assert lines[0] == ",".join(EXPORT_COLUMNS)
    assert [ln.split(",")[0] for ln in lines[1:]] == ["A", "B"]


def test_export_filename_is_fixed():
    assert export_filename() == "orders-all.csv"
'''


def doc_business_rules() -> str:
    out = ["# 業務規則", "", "示範客戶 ACME 訂單中心的業務規則。每條規則有編號，程式與測試引用編號。", ""]
    n = 0
    for d, zh, fields in DOMAINS:
        out += [f"## {zh}", ""]
        for fn, t, doc in fields:
            for k, rule in enumerate((f"{doc}建立後由系統維護，人工修改要留稽核紀錄。", f"查詢{zh}時可以用{doc}當條件；條件為空視為不限制。",
                                      f"{doc}出現在匯出與報表時，欄位名固定為 `{fn}`，不翻譯。", f"{doc}的異動要在 5 分鐘內反映到報表。",
                                      f"{doc}缺漏的{zh}不得進入下一個狀態。", f"客服只能查看、不能修改{doc}。")):
                n += 1
                out += [f"### BR-{n:03d} {zh}／{doc}（{k + 1}）", "", f"- 規則：{rule}", f"- 適用：{d} 模組。", f"- 型別：`{t}`。",
                        f"- 例外：示範環境不檢查。", ""]
    out += ["## 匯出", "", "### BR-900 訂單匯出的欄位", "", "- 規則：欄位順序固定為 order_no、customer_id、placed_at、status、total_cents、currency、channel。",
            "- 適用：export 模組。", "", "### BR-901 匯出的時間", "", "- 規則：目前輸出 UTC。客服多次反映客戶把它當成當地時間（見〈維運手冊〉的客訴紀錄）。",
            "- 待決：是否改成依客戶時區輸出，或同時輸出兩欄。", "", "### BR-902 匯出的筆數上限", "",
            "- 規則：同步匯出以 50,000 筆為上限（`SYNC_ROW_LIMIT`），超過應改走背景批次並通知申請人。目前**尚未實作**，超過時會逾時。", ""]
    return "\n".join(out)


def doc_api() -> str:
    out = ["# API 規格", "", "所有時間參數都用 UTC（ISO 8601，結尾 Z）。錯誤回應一律是 `{\"error\": <代碼>, \"detail\": <說明>}`。", ""]
    for d, zh, fields in DOMAINS:
        out += [f"## {zh}（/{d}）", "", f"### GET /{d}/{{key}}", "", f"取得一筆{zh}。找不到回 404。", "", "回應欄位：", ""]
        out += [f"| 欄位 | 型別 | 說明 |", "|---|---|---|"] + [f"| `{n}` | {t} | {doc} |" for n, t, doc in fields] + [""]
        out += [f"### GET /{d}", "", f"列出{zh}。參數：`limit`（預設 50，最大 200）、`offset`。", ""]
        for n, t, doc in fields:
            if t == "datetime":
                out += [f"### GET /{d}/by-{n}?from=&to=", "", f"依{doc}的區間列出{zh}。`from`、`to` 必填；`to` 要晚於 `from`，否則回 400 `bad_range`。", "",
                        "範例：", "", "```", f"GET /{d}/by-{n}?from=2026-01-01T00:00:00Z&to=2026-01-02T00:00:00Z", "```", ""]
            elif t == "str":
                out += [f"### GET /{d}/by-{n}/{{value}}", "", f"依{doc}列出{zh}。", ""]
    out += ["## 匯出（/exports）", "", "### GET /exports/orders.csv", "", "下載全部訂單的 CSV。**目前不接受任何參數**；檔名固定 `orders-all.csv`。",
            "", "- 回應標頭：`Content-Type: text/csv; charset=utf-8`、`Content-Disposition: attachment`。", "- 已知問題：訂單多時會超過閘道 30 秒逾時（回 504）。", ""]
    return "\n".join(out)


def doc_architecture() -> str:
    out = ["# 系統架構", "", "## 分層", "", "每個領域模組都是五層：models → repository → service → handlers，另有 validators。", "",
           "| 層 | 職責 | 不可以做的事 |", "|---|---|---|", "| models | 資料結構、轉成匯出列 | 存取資料庫 |", "| repository | 查詢與寫入 | 業務判斷 |",
           "| service | 業務邏輯 | 處理 HTTP |", "| handlers | 參數解析、回應格式 | 直接碰 repository 的內部狀態 |", "| validators | 欄位檢查 | 有副作用 |", "",
           "## 時間與時區", "", "- 資料庫與程式內部一律用不帶時區的 UTC。", "- 顯示給人看時才用 `shared/timezone.py` 轉成客戶時區（`customers.timezone`）。",
           "- 「某一天」的區間用 `day_range_utc` 算，不要自己加減 8 小時。", "", "## 背景批次", "",
           "- 佇列在 `shared/job_queue.py`，允許的種類列在 `KINDS`；新增種類要同時加處理函式與〈批次作業〉文件。", "- 作業失敗只記狀態，不重拋例外。", ""]
    for d, zh, fields in DOMAINS:
        out += [f"## 模組：{zh}（src/acme_orders/{d}/）", ""]
        for layer in LAYERS:
            out += [f"### {d}/{layer}.py", "", f"- 用途：{zh}的 {layer} 層。", f"- 相依：" + ("無" if layer == "models" else "同模組的 models") + "。",
                    f"- 主要符號：" + "、".join(f"`{n}`" for n, _, _ in fields[:4]) + " 相關的函式。", ""]
            for n, t, doc in fields:
                out += [f"  - `{n}`（{t}）：{doc}，在 {layer} 層的處理方式照〈分層〉的規定。"]
            out += [""]
    return "\n".join(out)


def doc_ops() -> str:
    out = ["# 維運手冊", "", "## 匯出逾時", "", "- 現象：客服下載訂單 CSV 時回 504。", "- 原因：`export_orders_csv` 一次載入全部訂單，超過閘道 30 秒。",
           "- 暫時處理：請客服改在離峰時段下載。", "- 根本解法（未排程）：加日期區間；超過 `SYNC_ROW_LIMIT` 時改走背景批次，完成後通知申請人。", "",
           "## 客訴紀錄（去識別化）", ""]
    for i in range(1, 41):
        out += [f"### 案例 {i:02d}", "", f"- 日期：2026-{(i % 12) + 1:02d}-{(i % 27) + 1:02d}", f"- 模組：{DOMAINS[i % len(DOMAINS)][1]}",
                "- 摘要：" + ("匯出的時間被當成當地時間，對帳差一天。" if i % 4 == 0 else "只想要上個月的訂單，卻只能整包下載。" if i % 4 == 1
                           else "下載逾時，重試三次才成功。" if i % 4 == 2 else "報表筆數和畫面不一致，查明是快取延遲。"),
                "- 處理：已回覆客戶，列入改善清單。", ""]
    out += ["## 例行作業", ""]
    for d, zh, _ in DOMAINS:
        for step in ("每日檢查錯誤率", "每週檢查資料量成長", "每月演練還原"):
            out += [f"- {zh}：{step}，結果記在值班表。"]
    return "\n".join(out) + "\n"


def doc_jobs() -> str:
    out = ["# 批次作業", "", "| 種類 | 排程 | 處理函式 | 失敗處理 |", "|---|---|---|---|", "| nightly_report | 每日 02:00 UTC | reports.service | 重試 3 次後通知值班 |",
           "| inventory_sync | 每 15 分鐘 | inventory.service | 下次排程補跑 |", "| invoice_batch | 每日 03:00 UTC | billing.service | 重試 3 次後通知帳務 |", "",
           "## 新增作業種類的步驟", "", "1. 在 `shared/job_queue.py` 的 `KINDS` 加上種類。", "2. 寫處理函式，接收 payload（dict）。",
           "3. 在這份文件的表格加一列。", "4. 作業完成要通知人時，用 notifications 模組排入通知，不要直接寄信。", ""]
    return "\n".join(out)


def files() -> dict[str, str]:
    out: dict[str, str] = {}
    pkg = "src/acme_orders"
    out[f"{pkg}/__init__.py"] = '"""示範客戶 ACME 的訂單中心（虛構，基準測試用）。"""\n'
    routes = [ROUTES_HEAD.rstrip("\n")]
    for d, zh, fields in DOMAINS:
        out[f"{pkg}/{d}/__init__.py"] = f'"""{zh}模組。"""\n'
        for layer in LAYERS:
            out[f"{pkg}/{d}/{layer}.py"] = GEN[layer](d, zh, fields)
        routes += [f'    ("GET", "/{d}/{{key}}", "acme_orders.{d}.handlers:get_{d}"),', f'    ("GET", "/{d}", "acme_orders.{d}.handlers:list_{d}"),']
        routes += [f'    ("GET", "/{d}/by-{n}", "acme_orders.{d}.handlers:list_{d}_by_{n}"),' for n, t, _ in fields if t in ("datetime", "str")]
    out[f"{pkg}/api/__init__.py"] = '"""HTTP 入口。"""\n'
    out[f"{pkg}/api/routes.py"] = "\n".join(routes + ["]", ""])
    out[f"{pkg}/export/__init__.py"] = '"""匯出。"""\n'
    out[f"{pkg}/export/csv_export.py"] = EXPORT_CSV
    out[f"{pkg}/export/handlers.py"] = EXPORT_HANDLER
    out[f"{pkg}/shared/__init__.py"] = '"""共用工具。"""\n'
    out[f"{pkg}/shared/timezone.py"] = TIMEZONE
    out[f"{pkg}/shared/job_queue.py"] = JOB_QUEUE
    out["tests/test_export.py"] = TEST_EXPORT
    out["tests/__init__.py"] = ""
    out["docs/業務規則.md"] = doc_business_rules()
    out["docs/API規格.md"] = doc_api()
    out["docs/系統架構.md"] = doc_architecture()
    out["docs/維運手冊.md"] = doc_ops()
    out["docs/批次作業.md"] = doc_jobs()
    out["README.md"] = ("# ACME 訂單中心（虛構）\n\n示範客戶 ACME 的訂單、客戶、庫存、帳務、出貨、通知、報表與批次作業。"
                        "這個 repo 由 AgentWorkFlow 的 `tests/bench/make_fixture.py` 產生，只用來量工作流程的成本。\n\n"
                        "- 程式：`src/acme_orders/<模組>/`，每個模組五層（見 `docs/系統架構.md`）。\n- 測試：`python3 -m pytest tests`（設 `PYTHONPATH=src`）。\n"
                        "- 規則：`docs/業務規則.md`；API：`docs/API規格.md`；維運：`docs/維運手冊.md`；批次：`docs/批次作業.md`。\n")
    out["pyproject.toml"] = '[project]\nname = "acme-orders"\nversion = "0.1.0"\nrequires-python = ">=3.9"\n\n[tool.pytest.ini_options]\npythonpath = ["src"]\n'
    out[".gitignore"] = "__pycache__/\n*.pyc\n.worktrees/\n_tpm/\n"
    return out


def write(target: Path) -> dict[str, str]:
    """寫出所有檔案，回傳 {相對路徑: sha1}。"""
    if target.exists() and any(target.iterdir()):
        raise SystemExit(f"{target} 不是空的，請換一個目錄")
    sums: dict[str, str] = {}
    for rel, text in sorted(files().items()):
        data = text.replace("\r\n", "\n").encode("utf-8")
        p = target / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        sums[rel] = hashlib.sha1(data).hexdigest()
    return sums


def git_init(target: Path) -> None:
    env = dict(os.environ, GIT_AUTHOR_NAME="bench", GIT_AUTHOR_EMAIL="bench@example.invalid", GIT_COMMITTER_NAME="bench",
               GIT_COMMITTER_EMAIL="bench@example.invalid", GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")
    for cmd in (["git", "init", "-q", "-b", "main"], ["git", "add", "-A"], ["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "chore: 基準用的虛構專案"]):
        subprocess.run(cmd, cwd=target, env=env, check=True)


def stats(target: Path, sums: dict[str, str]) -> dict:
    lines = {rel: (target / rel).read_bytes().count(b"\n") for rel in sums}
    return {"files": len(sums), "lines": sum(lines.values()), "big_files": sorted(r for r, n in lines.items() if n > 400)}


def selftest() -> int:
    ok = True

    def check(name: str, cond: bool, detail: str = "") -> None:
        nonlocal ok
        ok = ok and bool(cond)
        print(("通過  " if cond else "失敗  ") + name + (f"：{detail}" if detail and not cond else ""))

    with tempfile.TemporaryDirectory() as tmp:
        a, b = Path(tmp) / "a", Path(tmp) / "b"
        sa, sb = write(a), write(b)
        st = stats(a, sa)
        check("兩次產出的每個檔 sha1 相同", sa == sb)
        check("規模：至少 50 個檔、6000 行", st["files"] >= 50 and st["lines"] >= 6000, str(st))
        check("至少 3 個檔超過 400 行（讀取預算的整檔門檻）", len(st["big_files"]) >= 3, str(st["big_files"]))
        bad = []
        for rel in sa:
            if rel.endswith(".py"):
                try:
                    compile((a / rel).read_text(encoding="utf-8"), rel, "exec")
                except SyntaxError as e:
                    bad.append(f"{rel}: {e}")
        check("所有 .py 都能編譯", not bad, "；".join(bad[:3]))
        check("沒有 CRLF", all(b"\r" not in (a / rel).read_bytes() for rel in sa))
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    args = [x for x in argv if not x.startswith("--")]
    if len(args) != 1:
        print(__doc__)
        return 2
    target = Path(args[0]).resolve()
    target.mkdir(parents=True, exist_ok=True)
    sums = write(target)
    if "--no-git" not in argv:
        git_init(target)
    st = stats(target, sums)
    print(f"已產生 {st['files']} 個檔、{st['lines']} 行；超過 400 行的檔：{len(st['big_files'])} 個")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
