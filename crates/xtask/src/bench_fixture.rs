//! 產生基準用的虛構專案（示範客戶 ACME 的訂單中心）。原本是 0.x 的 Python 工具 `make_fixture.py`。
//! 內容全部虛構、完全由這支程式決定：同一版產出的檔案逐位元組相同。
//!
//! 為什麼要有：量 token 要有一個固定的情境才能前後比較。真實專案的 transcript 不能進版控，所以用一個規模接近的虛構
//! repo：約 67 個檔、六千多行，其中三份文件超過 400 行（讀取預算的整檔門檻），讓「整檔讀大檔」的成本顯現得出來。
//!
//! 虛構專案本身是一個 Python 專案（它是被分析的對象，不是我們的工具）：這裡只是把它的檔案內容當文字寫出來，
//! 不執行、也不需要 Python。固定不變的幾個檔放在 `bench_data/*.txt`。
//!
//! 產出要和 0.x 的 Python 版逐檔相同：每個檔的 sha1 記在 `tests/fixtures/bench-fixture.sha1.json`（0.x 的 Python 版還在時記下來的），
//! selftest 逐檔核對。0.x 的 Python 版的 selftest 另有一項「所有 .py 都能編譯」，這裡沒有 Python 可以編譯，改成核對那份清單
//! （清單記錄時 0.x 的 Python 版確認過都能編譯）。

use crate::util::{self, TempDir};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

type Field = (&'static str, &'static str, &'static str);
/// 領域：名稱、中文名、欄位（欄位名、型別、說明）
type Domain = (&'static str, &'static str, &'static [Field]);

const DOMAINS: [Domain; 8] = [
    ("orders", "訂單", &[("order_no", "str", "訂單編號"), ("customer_id", "str", "客戶代號"), ("placed_at", "datetime", "下單時間（UTC）"),
        ("status", "str", "狀態"), ("total_cents", "int", "總金額（分）"), ("currency", "str", "幣別"), ("channel", "str", "通路")]),
    ("customers", "客戶", &[("customer_id", "str", "客戶代號"), ("name", "str", "名稱"), ("tier", "str", "等級"), ("timezone", "str", "時區"),
        ("created_at", "datetime", "建立時間（UTC）"), ("email_opt_in", "bool", "是否收信")]),
    ("inventory", "庫存", &[("sku", "str", "料號"), ("warehouse", "str", "倉別"), ("on_hand", "int", "現有量"), ("reserved", "int", "保留量"),
        ("updated_at", "datetime", "更新時間（UTC）")]),
    ("billing", "帳務", &[("invoice_no", "str", "發票號碼"), ("order_no", "str", "訂單編號"), ("issued_at", "datetime", "開立時間（UTC）"),
        ("amount_cents", "int", "金額（分）"), ("tax_cents", "int", "稅額（分）"), ("state", "str", "狀態")]),
    ("shipping", "出貨", &[("shipment_no", "str", "出貨單號"), ("order_no", "str", "訂單編號"), ("carrier", "str", "物流商"),
        ("shipped_at", "datetime", "出貨時間（UTC）"), ("delivered_at", "datetime", "送達時間（UTC）"), ("state", "str", "狀態")]),
    ("notifications", "通知", &[("notice_id", "str", "通知編號"), ("recipient", "str", "收件者"), ("template", "str", "範本"),
        ("queued_at", "datetime", "排入時間（UTC）"), ("sent_at", "datetime", "送出時間（UTC）"), ("state", "str", "狀態")]),
    ("reports", "報表", &[("report_id", "str", "報表編號"), ("kind", "str", "種類"), ("requested_by", "str", "申請人"),
        ("requested_at", "datetime", "申請時間（UTC）"), ("rows", "int", "筆數"), ("state", "str", "狀態")]),
    ("jobs", "批次作業", &[("job_id", "str", "作業編號"), ("kind", "str", "種類"), ("payload", "str", "參數（JSON）"),
        ("enqueued_at", "datetime", "排入時間（UTC）"), ("finished_at", "datetime", "完成時間（UTC）"), ("state", "str", "狀態")]),
];
const LAYERS: [&str; 5] = ["models", "repository", "service", "handlers", "validators"];
const STATES: [&str; 5] = ["pending", "active", "done", "failed", "cancelled"];

const EXPORT_CSV: &str = include_str!("bench_data/export_csv.txt");
const EXPORT_HANDLER: &str = include_str!("bench_data/export_handler.txt");
const TIMEZONE: &str = include_str!("bench_data/timezone.txt");
const JOB_QUEUE: &str = include_str!("bench_data/job_queue.txt");
const ROUTES_HEAD: &str = include_str!("bench_data/routes_head.txt");
const TEST_EXPORT: &str = include_str!("bench_data/test_export.txt");
const MANIFEST: &str = include_str!("../tests/fixtures/bench-fixture.sha1.json");
/// 清單檔的位置（相對 repo 根目錄）與開頭的說明。
pub const MANIFEST_REL: &str = "crates/xtask/tests/fixtures/bench-fixture.sha1.json";
const MANIFEST_NOTE: &str = "基準用虛構專案每個檔的 sha1。最初由 0.x 的 Python 版 tests/bench/make_fixture.py 的產出記下來（當時每個 .py 都確認過能編譯）；cargo xtask bench --selftest 核對現在的產出和它逐檔相同。內容確實該變時用 cargo xtask bench manifest --write 重寫，並在 commit 說明。";

/// 類別名：去掉結尾的 s、第一個字母大寫（名稱都是小寫英文字）。
fn cls(domain: &str) -> String {
    let base = domain.strip_suffix('s').unwrap_or(domain);
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

fn s(x: &str) -> String {
    x.to_string()
}

fn gen_models(d: &str, zh: &str, fields: &[Field]) -> String {
    let c = cls(d);
    let mut out = vec![
        format!("\"\"\"{zh}的資料模型。時間欄位一律存 UTC，顯示時才轉時區。\"\"\""), s("from __future__ import annotations"), s(""),
        s("from dataclasses import dataclass, field"), s("from datetime import datetime"), s(""), s(""),
        s("@dataclass"), format!("class {c}:"), format!("    \"\"\"一筆{zh}。\"\"\""), s(""),
    ];
    for (n, t, doc) in fields {
        out.push(format!("    {n}: {t}  # {doc}"));
    }
    out.extend([s("    tags: list[str] = field(default_factory=list)"), s(""), s("    def to_row(self) -> dict:"),
        s("        \"\"\"轉成匯出用的一列；鍵的順序就是欄位順序。\"\"\""), s("        return {")]);
    for (n, t, _) in fields {
        let v = if *t == "datetime" { format!("self.{n}.isoformat()") } else { format!("self.{n}") };
        out.push(format!("            \"{n}\": {v},"));
    }
    out.extend([s("        }"), s("")]);
    for (n, t, doc) in fields {
        out.extend([format!("    def with_{n}(self, value: {t}) -> \"{c}\":"), format!("        \"\"\"回傳改了{doc}的新物件，不改原物件。\"\"\""),
            s("        data = dict(self.__dict__)"), format!("        data[\"{n}\"] = value"), format!("        return {c}(**data)"), s("")]);
    }
    let states: Vec<String> = STATES.iter().map(|x| format!("'{x}'")).collect();
    out.extend([s(""), format!("{}_STATES = ({})", d.to_ascii_uppercase(), states.join(", ")), s("")]);
    out.join("\n")
}

fn gen_repository(d: &str, zh: &str, fields: &[Field]) -> String {
    let c = cls(d);
    let key = fields.first().map(|f| f.0).unwrap_or_default();
    let mut out = vec![
        format!("\"\"\"{zh}的資料存取。示範用的記憶體實作；正式環境由 storage 模組換成資料庫。\"\"\""), s("from __future__ import annotations"), s(""),
        s("from datetime import datetime"), s("from typing import Iterable, Iterator"), s(""), format!("from .models import {c}"), s(""), s(""),
        format!("class {c}Repository:"), format!("    \"\"\"{zh}的查詢與寫入。查詢一律回傳 iterator，呼叫端自己決定要不要全部載入。\"\"\""), s(""),
        s("    def __init__(self) -> None:"), format!("        self._items: dict[str, {c}] = {{}}"), s(""),
        format!("    def save(self, item: {c}) -> None:"), format!("        self._items[item.{key}] = item"), s(""),
        format!("    def get(self, key: str) -> {c} | None:"), s("        return self._items.get(key)"), s(""),
        format!("    def all(self) -> Iterator[{c}]:"), s("        yield from self._items.values()"), s(""),
    ];
    for (n, t, doc) in fields {
        match *t {
            "datetime" => out.extend([
                format!("    def between_{n}(self, start: datetime, end: datetime) -> Iterator[{c}]:"),
                format!("        \"\"\"{doc}落在 [start, end) 的{zh}。start、end 必須是 UTC。\"\"\""),
                s("        for item in self._items.values():"), format!("            if start <= item.{n} < end:"), s("                yield item"), s(""),
            ]),
            "str" => out.extend([
                format!("    def by_{n}(self, value: str) -> Iterator[{c}]:"), format!("        \"\"\"依{doc}查詢。\"\"\""),
                s("        for item in self._items.values():"), format!("            if item.{n} == value:"), s("                yield item"), s(""),
                format!("    def count_by_{n}(self) -> dict[str, int]:"), format!("        \"\"\"各{doc}的筆數。\"\"\""), s("        out: dict[str, int] = {}"),
                s("        for item in self._items.values():"), format!("            out[item.{n}] = out.get(item.{n}, 0) + 1"), s("        return out"), s(""),
            ]),
            "int" => out.extend([
                format!("    def sum_{n}(self, items: Iterable[{c}] | None = None) -> int:"), format!("        \"\"\"{doc}的加總。\"\"\""),
                format!("        return sum(i.{n} for i in (items if items is not None else self._items.values()))"), s(""),
            ]),
            _ => {}
        }
    }
    out.join("\n")
}

fn gen_service(d: &str, zh: &str, fields: &[Field]) -> String {
    let c = cls(d);
    let mut out = vec![
        format!("\"\"\"{zh}的業務邏輯。規則的出處見 docs/業務規則.md 的〈{zh}〉一節。\"\"\""), s("from __future__ import annotations"), s(""),
        s("from datetime import datetime, timedelta"), s(""), format!("from .models import {c}"), format!("from .repository import {c}Repository"),
        format!("from .validators import validate_{d}"), s(""), s(""), format!("class {c}Service:"), format!("    \"\"\"{zh}的用例。\"\"\""), s(""),
        format!("    def __init__(self, repo: {c}Repository) -> None:"), s("        self.repo = repo"), s(""),
        format!("    def create(self, item: {c}) -> {c}:"), format!("        errors = validate_{d}(item)"), s("        if errors:"),
        s("            raise ValueError(\"；\".join(errors))"), s("        self.repo.save(item)"), s("        return item"), s(""),
    ];
    let last = fields.last().copied().unwrap_or(("", "", ""));
    for st in STATES {
        out.extend([
            format!("    def mark_{st}(self, key: str) -> {c}:"), format!("        \"\"\"把{zh}改成 {st}。找不到就丟 KeyError。\"\"\""),
            s("        item = self.repo.get(key)"), s("        if item is None:"), s("            raise KeyError(key)"),
            if last.1 == "str" { format!("        changed = item.with_{}(\"{st}\")", last.0) } else { s("        changed = item") },
            s("        self.repo.save(changed)"), s("        return changed"), s(""),
        ]);
    }
    for (n, t, doc) in fields {
        if *t == "datetime" {
            out.extend([
                format!("    def recent_by_{n}(self, now: datetime, days: int = 7) -> list[{c}]:"),
                format!("        \"\"\"最近 days 天（以{doc}計）的{zh}。now 必須是 UTC。\"\"\""),
                format!("        return list(self.repo.between_{n}(now - timedelta(days=days), now))"), s(""),
                format!("    def daily_counts_by_{n}(self, start: datetime, end: datetime) -> dict[str, int]:"),
                s("        \"\"\"[start, end) 內每天的筆數，日期用 UTC 切。\"\"\""), s("        out: dict[str, int] = {}"),
                format!("        for item in self.repo.between_{n}(start, end):"), format!("            day = item.{n}.date().isoformat()"),
                s("            out[day] = out.get(day, 0) + 1"), s("        return out"), s(""),
            ]);
        }
    }
    out.join("\n")
}

fn gen_handlers(d: &str, zh: &str, fields: &[Field]) -> String {
    let c = cls(d);
    let mut out = vec![
        format!("\"\"\"{zh}的 HTTP 處理。路由表在 api/routes.py；回應格式見 docs/API規格.md。\"\"\""), s("from __future__ import annotations"), s(""),
        s("from datetime import datetime"), s(""), format!("from .service import {c}Service"), s(""), s(""),
        s("def _parse_utc(text: str) -> datetime:"), s("    \"\"\"只接受 ISO 8601 且帶 Z 的時間。\"\"\""), s("    if not text.endswith(\"Z\"):"),
        s("        raise ValueError(\"時間要用 UTC（結尾 Z）\")"), s("    return datetime.fromisoformat(text[:-1])"), s(""), s(""),
    ];
    out.extend([
        format!("def get_{d}(service: {c}Service, key: str) -> tuple[int, dict]:"), s("    item = service.repo.get(key)"), s("    if item is None:"),
        s("        return 404, {\"error\": \"not_found\", \"key\": key}"), s("    return 200, item.to_row()"), s(""), s(""),
        format!("def list_{d}(service: {c}Service, query: dict) -> tuple[int, dict]:"), format!("    \"\"\"列出{zh}。支援 limit、offset。\"\"\""),
        s("    limit = min(int(query.get(\"limit\", 50)), 200)"), s("    offset = int(query.get(\"offset\", 0))"),
        s("    rows = [i.to_row() for i in service.repo.all()]"),
        s("    return 200, {\"total\": len(rows), \"items\": rows[offset:offset + limit]}"), s(""), s(""),
    ]);
    for (n, t, doc) in fields {
        match *t {
            "datetime" => out.extend([
                format!("def list_{d}_by_{n}(service: {c}Service, query: dict) -> tuple[int, dict]:"),
                format!("    \"\"\"依{doc}的區間列出{zh}。from、to 必填，都是 UTC。\"\"\""), s("    try:"),
                s("        start = _parse_utc(query[\"from\"])"), s("        end = _parse_utc(query[\"to\"])"),
                s("    except (KeyError, ValueError) as e:"), s("        return 400, {\"error\": \"bad_range\", \"detail\": str(e)}"),
                s("    if end <= start:"), s("        return 400, {\"error\": \"bad_range\", \"detail\": \"to 要晚於 from\"}"),
                format!("    rows = [i.to_row() for i in service.repo.between_{n}(start, end)]"),
                s("    return 200, {\"total\": len(rows), \"items\": rows}"), s(""), s(""),
            ]),
            "str" => out.extend([
                format!("def list_{d}_by_{n}(service: {c}Service, value: str) -> tuple[int, dict]:"),
                format!("    rows = [i.to_row() for i in service.repo.by_{n}(value)]"), s("    return 200, {\"total\": len(rows), \"items\": rows}"), s(""), s(""),
            ]),
            _ => {}
        }
    }
    out.join("\n")
}

fn gen_validators(d: &str, zh: &str, fields: &[Field]) -> String {
    let c = cls(d);
    let mut out = vec![
        format!("\"\"\"{zh}的欄位檢查。回傳錯誤訊息清單，空清單表示通過。\"\"\""), s("from __future__ import annotations"), s(""), format!("from .models import {c}"), s(""), s(""),
        format!("def validate_{d}(item: {c}) -> list[str]:"), s("    errors: list[str] = []"),
    ];
    for (n, t, doc) in fields {
        match *t {
            "str" => out.extend([format!("    if not item.{n} or len(item.{n}) > 64:"), format!("        errors.append(\"{doc}必填，最長 64 字\")")]),
            "int" => out.extend([format!("    if item.{n} < 0:"), format!("        errors.append(\"{doc}不能是負數\")")]),
            "datetime" => out.extend([format!("    if item.{n}.tzinfo is not None:"), format!("        errors.append(\"{doc}要存成不帶時區的 UTC\")")]),
            _ => {}
        }
    }
    out.extend([s("    return errors"), s("")]);
    out.join("\n")
}

fn gen_layer(layer: &str, d: &str, zh: &str, fields: &[Field]) -> String {
    match layer {
        "models" => gen_models(d, zh, fields),
        "repository" => gen_repository(d, zh, fields),
        "service" => gen_service(d, zh, fields),
        "handlers" => gen_handlers(d, zh, fields),
        _ => gen_validators(d, zh, fields),
    }
}

fn doc_business_rules() -> String {
    let mut out = vec![s("# 業務規則"), s(""), s("示範客戶 ACME 訂單中心的業務規則。每條規則有編號，程式與測試引用編號。"), s("")];
    let mut n = 0;
    for (d, zh, fields) in DOMAINS {
        out.extend([format!("## {zh}"), s("")]);
        for (fname, t, doc) in fields {
            let rules = [
                format!("{doc}建立後由系統維護，人工修改要留稽核紀錄。"), format!("查詢{zh}時可以用{doc}當條件；條件為空視為不限制。"),
                format!("{doc}出現在匯出與報表時，欄位名固定為 `{fname}`，不翻譯。"), format!("{doc}的異動要在 5 分鐘內反映到報表。"),
                format!("{doc}缺漏的{zh}不得進入下一個狀態。"), format!("客服只能查看、不能修改{doc}。"),
            ];
            for (k, rule) in rules.iter().enumerate() {
                n += 1;
                out.extend([format!("### BR-{n:03} {zh}／{doc}（{}）", k + 1), s(""), format!("- 規則：{rule}"), format!("- 適用：{d} 模組。"), format!("- 型別：`{t}`。"),
                    s("- 例外：示範環境不檢查。"), s("")]);
            }
        }
    }
    out.extend([
        s("## 匯出"), s(""), s("### BR-900 訂單匯出的欄位"), s(""), s("- 規則：欄位順序固定為 order_no、customer_id、placed_at、status、total_cents、currency、channel。"),
        s("- 適用：export 模組。"), s(""), s("### BR-901 匯出的時間"), s(""), s("- 規則：目前輸出 UTC。客服多次反映客戶把它當成當地時間（見〈維運手冊〉的客訴紀錄）。"),
        s("- 待決：是否改成依客戶時區輸出，或同時輸出兩欄。"), s(""), s("### BR-902 匯出的筆數上限"), s(""),
        s("- 規則：同步匯出以 50,000 筆為上限（`SYNC_ROW_LIMIT`），超過應改走背景批次並通知申請人。目前**尚未實作**，超過時會逾時。"), s(""),
    ]);
    out.join("\n")
}

fn doc_api() -> String {
    let mut out = vec![s("# API 規格"), s(""), s("所有時間參數都用 UTC（ISO 8601，結尾 Z）。錯誤回應一律是 `{\"error\": <代碼>, \"detail\": <說明>}`。"), s("")];
    for (d, zh, fields) in DOMAINS {
        out.extend([format!("## {zh}（/{d}）"), s(""), format!("### GET /{d}/{{key}}"), s(""), format!("取得一筆{zh}。找不到回 404。"), s(""), s("回應欄位："), s("")]);
        out.extend([s("| 欄位 | 型別 | 說明 |"), s("|---|---|---|")]);
        out.extend(fields.iter().map(|(n, t, doc)| format!("| `{n}` | {t} | {doc} |")));
        out.push(s(""));
        out.extend([format!("### GET /{d}"), s(""), format!("列出{zh}。參數：`limit`（預設 50，最大 200）、`offset`。"), s("")]);
        for (n, t, doc) in fields {
            match *t {
                "datetime" => out.extend([
                    format!("### GET /{d}/by-{n}?from=&to="), s(""), format!("依{doc}的區間列出{zh}。`from`、`to` 必填；`to` 要晚於 `from`，否則回 400 `bad_range`。"), s(""),
                    s("範例："), s(""), s("```"), format!("GET /{d}/by-{n}?from=2026-01-01T00:00:00Z&to=2026-01-02T00:00:00Z"), s("```"), s(""),
                ]),
                "str" => out.extend([format!("### GET /{d}/by-{n}/{{value}}"), s(""), format!("依{doc}列出{zh}。"), s("")]),
                _ => {}
            }
        }
    }
    out.extend([
        s("## 匯出（/exports）"), s(""), s("### GET /exports/orders.csv"), s(""), s("下載全部訂單的 CSV。**目前不接受任何參數**；檔名固定 `orders-all.csv`。"),
        s(""), s("- 回應標頭：`Content-Type: text/csv; charset=utf-8`、`Content-Disposition: attachment`。"), s("- 已知問題：訂單多時會超過閘道 30 秒逾時（回 504）。"), s(""),
    ]);
    out.join("\n")
}

fn doc_architecture() -> String {
    let mut out = vec![
        s("# 系統架構"), s(""), s("## 分層"), s(""), s("每個領域模組都是五層：models → repository → service → handlers，另有 validators。"), s(""),
        s("| 層 | 職責 | 不可以做的事 |"), s("|---|---|---|"), s("| models | 資料結構、轉成匯出列 | 存取資料庫 |"), s("| repository | 查詢與寫入 | 業務判斷 |"),
        s("| service | 業務邏輯 | 處理 HTTP |"), s("| handlers | 參數解析、回應格式 | 直接碰 repository 的內部狀態 |"), s("| validators | 欄位檢查 | 有副作用 |"), s(""),
        s("## 時間與時區"), s(""), s("- 資料庫與程式內部一律用不帶時區的 UTC。"), s("- 顯示給人看時才用 `shared/timezone.py` 轉成客戶時區（`customers.timezone`）。"),
        s("- 「某一天」的區間用 `day_range_utc` 算，不要自己加減 8 小時。"), s(""), s("## 背景批次"), s(""),
        s("- 佇列在 `shared/job_queue.py`，允許的種類列在 `KINDS`；新增種類要同時加處理函式與〈批次作業〉文件。"), s("- 作業失敗只記狀態，不重拋例外。"), s(""),
    ];
    for (d, zh, fields) in DOMAINS {
        out.extend([format!("## 模組：{zh}（src/acme_orders/{d}/）"), s("")]);
        for layer in LAYERS {
            let symbols: Vec<String> = fields.iter().take(4).map(|(n, _, _)| format!("`{n}`")).collect();
            out.extend([
                format!("### {d}/{layer}.py"), s(""), format!("- 用途：{zh}的 {layer} 層。"),
                format!("- 相依：{}。", if layer == "models" { "無" } else { "同模組的 models" }),
                format!("- 主要符號：{} 相關的函式。", symbols.join("、")), s(""),
            ]);
            for (n, t, doc) in fields {
                out.push(format!("  - `{n}`（{t}）：{doc}，在 {layer} 層的處理方式照〈分層〉的規定。"));
            }
            out.push(s(""));
        }
    }
    out.join("\n")
}

fn doc_ops() -> String {
    let mut out = vec![
        s("# 維運手冊"), s(""), s("## 匯出逾時"), s(""), s("- 現象：客服下載訂單 CSV 時回 504。"), s("- 原因：`export_orders_csv` 一次載入全部訂單，超過閘道 30 秒。"),
        s("- 暫時處理：請客服改在離峰時段下載。"), s("- 根本解法（未排程）：加日期區間；超過 `SYNC_ROW_LIMIT` 時改走背景批次，完成後通知申請人。"), s(""),
        s("## 客訴紀錄（去識別化）"), s(""),
    ];
    for i in 1..41usize {
        let summary = match i % 4 {
            0 => "匯出的時間被當成當地時間，對帳差一天。",
            1 => "只想要上個月的訂單，卻只能整包下載。",
            2 => "下載逾時，重試三次才成功。",
            _ => "報表筆數和畫面不一致，查明是快取延遲。",
        };
        let zh = DOMAINS.get(i % DOMAINS.len()).map(|d| d.1).unwrap_or_default();
        out.extend([
            format!("### 案例 {i:02}"), s(""), format!("- 日期：2026-{:02}-{:02}", (i % 12) + 1, (i % 27) + 1), format!("- 模組：{zh}"),
            format!("- 摘要：{summary}"), s("- 處理：已回覆客戶，列入改善清單。"), s(""),
        ]);
    }
    out.extend([s("## 例行作業"), s("")]);
    for (_, zh, _) in DOMAINS {
        for step in ["每日檢查錯誤率", "每週檢查資料量成長", "每月演練還原"] {
            out.push(format!("- {zh}：{step}，結果記在值班表。"));
        }
    }
    out.join("\n") + "\n"
}

fn doc_jobs() -> String {
    [
        "# 批次作業", "", "| 種類 | 排程 | 處理函式 | 失敗處理 |", "|---|---|---|---|", "| nightly_report | 每日 02:00 UTC | reports.service | 重試 3 次後通知值班 |",
        "| inventory_sync | 每 15 分鐘 | inventory.service | 下次排程補跑 |", "| invoice_batch | 每日 03:00 UTC | billing.service | 重試 3 次後通知帳務 |", "",
        "## 新增作業種類的步驟", "", "1. 在 `shared/job_queue.py` 的 `KINDS` 加上種類。", "2. 寫處理函式，接收 payload（dict）。",
        "3. 在這份文件的表格加一列。", "4. 作業完成要通知人時，用 notifications 模組排入通知，不要直接寄信。", "",
    ]
    .join("\n")
}

/// 全部檔案：相對路徑 → 內容（照路徑排序）。
pub fn files() -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let pkg = "src/acme_orders";
    out.insert(format!("{pkg}/__init__.py"), s("\"\"\"示範客戶 ACME 的訂單中心（虛構，基準測試用）。\"\"\"\n"));
    let mut routes = vec![ROUTES_HEAD.trim_end_matches('\n').to_string()];
    for (d, zh, fields) in DOMAINS {
        out.insert(format!("{pkg}/{d}/__init__.py"), format!("\"\"\"{zh}模組。\"\"\"\n"));
        for layer in LAYERS {
            out.insert(format!("{pkg}/{d}/{layer}.py"), gen_layer(layer, d, zh, fields));
        }
        routes.extend([
            format!("    (\"GET\", \"/{d}/{{key}}\", \"acme_orders.{d}.handlers:get_{d}\"),"),
            format!("    (\"GET\", \"/{d}\", \"acme_orders.{d}.handlers:list_{d}\"),"),
        ]);
        routes.extend(fields.iter().filter(|(_, t, _)| *t == "datetime" || *t == "str")
            .map(|(n, _, _)| format!("    (\"GET\", \"/{d}/by-{n}\", \"acme_orders.{d}.handlers:list_{d}_by_{n}\"),")));
    }
    routes.extend([s("]"), s("")]);
    out.insert(format!("{pkg}/api/__init__.py"), s("\"\"\"HTTP 入口。\"\"\"\n"));
    out.insert(format!("{pkg}/api/routes.py"), routes.join("\n"));
    out.insert(format!("{pkg}/export/__init__.py"), s("\"\"\"匯出。\"\"\"\n"));
    out.insert(format!("{pkg}/export/csv_export.py"), s(EXPORT_CSV));
    out.insert(format!("{pkg}/export/handlers.py"), s(EXPORT_HANDLER));
    out.insert(format!("{pkg}/shared/__init__.py"), s("\"\"\"共用工具。\"\"\"\n"));
    out.insert(format!("{pkg}/shared/timezone.py"), s(TIMEZONE));
    out.insert(format!("{pkg}/shared/job_queue.py"), s(JOB_QUEUE));
    out.insert(s("tests/test_export.py"), s(TEST_EXPORT));
    out.insert(s("tests/__init__.py"), s(""));
    out.insert(s("docs/業務規則.md"), doc_business_rules());
    out.insert(s("docs/API規格.md"), doc_api());
    out.insert(s("docs/系統架構.md"), doc_architecture());
    out.insert(s("docs/維運手冊.md"), doc_ops());
    out.insert(s("docs/批次作業.md"), doc_jobs());
    // README 的字面和 0.x 的 Python 版相同（裡面提到的是當初產生它的腳本）；改了它，虛構專案就變了、前後的量測不能比較
    out.insert(s("README.md"), s(concat!(
        "# ACME 訂單中心（虛構）\n\n示範客戶 ACME 的訂單、客戶、庫存、帳務、出貨、通知、報表與批次作業。",
        "這個 repo 由 AgentWorkFlow 的 `tests/bench/make_fixture.py` 產生，只用來量工作流程的成本。\n\n",
        "- 程式：`src/acme_orders/<模組>/`，每個模組五層（見 `docs/系統架構.md`）。\n- 測試：`python3 -m pytest tests`（設 `PYTHONPATH=src`）。\n",
        "- 規則：`docs/業務規則.md`；API：`docs/API規格.md`；維運：`docs/維運手冊.md`；批次：`docs/批次作業.md`。\n",
    )));
    out.insert(s("pyproject.toml"), s("[project]\nname = \"acme-orders\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\n\n[tool.pytest.ini_options]\npythonpath = [\"src\"]\n"));
    out.insert(s(".gitignore"), s("__pycache__/\n*.pyc\n.worktrees/\n_tpm/\n"));
    out
}

fn sha1_hex(data: &[u8]) -> String {
    Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn full(target: &Path, rel: &str) -> std::path::PathBuf {
    rel.split('/').fold(target.to_path_buf(), |p, part| p.join(part))
}

/// 每個檔的 sha1（不寫檔）。
pub fn sums() -> BTreeMap<String, String> {
    files().into_iter().map(|(rel, text)| (rel, sha1_hex(text.replace("\r\n", "\n").as_bytes()))).collect()
}

/// 寫出所有檔案，回傳 {相對路徑: sha1}。目標目錄必須不存在或是空的。
pub fn write(target: &Path) -> Result<BTreeMap<String, String>, String> {
    if target.exists() && std::fs::read_dir(target).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} 不是空的，請換一個目錄", target.display()));
    }
    let mut out = BTreeMap::new();
    for (rel, text) in files() {
        let data = text.replace("\r\n", "\n").into_bytes();
        let p = full(target, &rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}：{e}", dir.display()))?;
        }
        std::fs::write(&p, &data).map_err(|e| format!("寫不了 {}：{e}", p.display()))?;
        out.insert(rel, sha1_hex(&data));
    }
    Ok(out)
}

/// `git init` 並做一個 commit（固定作者與時間，commit 雜湊可重現）。
pub fn git_init(target: &Path) -> Result<(), String> {
    let cmds: [&[&str]; 3] = [&["init", "-q", "-b", "main"], &["add", "-A"], &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "chore: 基準用的虛構專案"]];
    for args in cmds {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(target)
            .env("GIT_AUTHOR_NAME", "bench").env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
            .env("GIT_COMMITTER_NAME", "bench").env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z").env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z");
        let r = util::run(&mut cmd, None)?;
        if r.code != 0 {
            return Err(format!("git {} 失敗：{}", args.join(" "), r.stderr.trim()));
        }
    }
    Ok(())
}

/// 規模：（檔案數、總行數、超過 400 行的檔）。
pub struct Stats {
    pub files: usize,
    pub lines: usize,
    pub big_files: Vec<String>,
}

pub fn stats(target: &Path, sums: &BTreeMap<String, String>) -> Stats {
    let lines: Vec<(&String, usize)> = sums.keys().map(|rel| (rel, std::fs::read(full(target, rel)).map(|b| b.iter().filter(|c| **c == b'\n').count()).unwrap_or(0))).collect();
    Stats {
        files: sums.len(),
        lines: lines.iter().map(|(_, n)| n).sum(),
        big_files: lines.iter().filter(|(_, n)| *n > 400).map(|(r, _)| (*r).clone()).collect(),
    }
}

/// 清單檔的內容（照現在的產出）。
pub fn manifest_text() -> String {
    let files: serde_json::Map<String, serde_json::Value> = sums().into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect();
    format!("{}\n", awf_core::recipe::dumps(&serde_json::json!({"_說明": MANIFEST_NOTE, "files": files}), Some(1)))
}

/// 現在的產出和清單不同的地方（空的＝逐檔相同）。
pub fn manifest_diff() -> Vec<String> {
    let want: BTreeMap<String, String> = serde_json::from_str::<serde_json::Value>(MANIFEST).ok()
        .and_then(|v| v.get("files").and_then(|f| f.as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()))
        .unwrap_or_default();
    let have = sums();
    let mut out = Vec::new();
    for (rel, sum) in &have {
        match want.get(rel) {
            None => out.push(format!("{rel}：清單沒有")),
            Some(w) if w != sum => out.push(format!("{rel}：內容不同")),
            _ => {}
        }
    }
    out.extend(want.keys().filter(|k| !have.contains_key(*k)).map(|k| format!("{k}：清單有、現在沒有產生")));
    out
}

/// `--selftest`：回（是否全過、輸出）。
pub fn selftest() -> (bool, String) {
    let mut ok = true;
    let mut out = String::new();
    let mut check = |name: &str, cond: bool, detail: String| {
        ok = ok && cond;
        out.push_str(&format!("{}{name}{}\n", if cond { "通過  " } else { "失敗  " }, if !cond && !detail.is_empty() { format!("：{detail}") } else { String::new() }));
    };
    let tmp = match TempDir::new("awf-bench-fixture.") {
        Ok(t) => t,
        Err(e) => return (false, format!("失敗  建不了暫存目錄：{e}\n")),
    };
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    let (sa, sb) = match (write(&a), write(&b)) {
        (Ok(x), Ok(y)) => (x, y),
        (x, y) => return (false, format!("失敗  寫不出虛構專案：{}\n", x.err().or(y.err()).unwrap_or_default())),
    };
    let st = stats(&a, &sa);
    check("兩次產出的每個檔 sha1 相同", sa == sb, String::new());
    check("規模：至少 50 個檔、6000 行", st.files >= 50 && st.lines >= 6000, format!("{} 個檔、{} 行", st.files, st.lines));
    check("至少 3 個檔超過 400 行（讀取預算的整檔門檻）", st.big_files.len() >= 3, st.big_files.join("、"));
    let diff = manifest_diff();
    check("和記下來的清單逐檔相同（清單取自 0.x 的 Python 版的產出，當時所有 .py 都能編譯）", diff.is_empty(), diff.iter().take(3).cloned().collect::<Vec<_>>().join("；"));
    let no_cr = sa.keys().all(|rel| std::fs::read(full(&a, rel)).map(|d| !d.contains(&b'\r')).unwrap_or(false));
    check("沒有 CRLF", no_cr, String::new());
    (ok, out)
}
