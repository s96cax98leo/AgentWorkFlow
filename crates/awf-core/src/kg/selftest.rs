//! `awf kg --selftest`：在暫存目錄造一個小專案（幾個原始檔、一份 OpenSpec 目錄、連結帳本、決策記憶、kb 文件、筆記），
//! 建圖、五個查詢各驗預期結果，再驗增量、逾時、可重現、匯出。

use super::build::{build, layout, BuildOpts, Outcome};
use super::query::{self, Answer};
use super::{export, run_captured, store, update, UpdateStatus};
use crate::save_run;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Tally {
    passed: usize,
    failed: usize,
}

impl Tally {
    fn check(&mut self, name: &str, cond: bool, detail: &str) {
        if cond {
            self.passed += 1;
            println!("  [通過] {name}");
        } else {
            self.failed += 1;
            println!("  [失敗] {name} {detail}");
        }
    }
}

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, text);
}

const RUN: &str = "2026-01-05-export";
const REQ: &str = "billing/export-invoice";
const SCN: &str = "billing/export-invoice#csv-download";
const CHANGE: &str = "export-csv";
/// 需求來源不是規格層的 run（需求是 epic 名、驗收條件是「AC-…」編號）。
const OLD_RUN: &str = "2026-01-06-legacy";
const SHA: &str = "1111111111111111111111111111111111111111";
const MERGE_SHA: &str = "2222222222222222222222222222222222222222";
const DECISION: &str = "{\"question\":\"匯出要不要含稅\",\"answer\":\"要\",\"scope\":\"project\",\"by\":\"pm\",\"at\":\"2026-01-05T00:00:00Z\",\"run\":\"2026-01-05-export\"}";

fn env(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

fn design_of(epic: &str, acc: &str, refs: &[&str], owns: &[&str]) -> Value {
    json!({"design": env(json!({"epic": epic, "subtasks": [{"id": "t1", "title": "t1", "kind": "code", "owns": owns, "depends_on": [], "acceptance": [acc], "spec_refs": refs}]}))})
}

fn test_of(id: &str, command: &str, commit: &str) -> Value {
    json!({"verifier": env(json!({"commit": commit, "scope_covered": "x", "scope_skipped": "none", "anti_cheat": [], "criteria": [{"id": id, "criterion": "c", "command": command, "result": "pass", "evidence": "e"}]}))})
}

/// 連結帳本：不手寫，用 `save_run` 的寫入端照各段的結果寫出來（和 `awf save-run stage` 存檔時走的是同一組函式）。
/// 兩個 run：一個需求來源是規格層（穩定 id），一個不是（epic 名與「AC-…」編號）。design 存兩次，帳本裡會有一筆 supersede。
fn write_ledger(root: &Path) -> Result<(), String> {
    let rs = root.to_string_lossy().into_owned();
    let ctx = save_run::Ctx::new(&rs, &rs);
    let profile = json!({"requirements": {"source": "openspec", "path": "openspec"}, "links": {}});
    let fail = |e: crate::kb::PyErr| format!("寫連結帳本失敗（{}）", e.msg);
    let save = |run: &str, stage: &str, result: &Value, sc: Option<&save_run::SpecCtx>| -> Result<(), String> {
        if stage == "design" {
            put(root, &format!("{run}/design.json"), &result.to_string());
        }
        let links = save_run::stage_links(&ctx, run, stage, result, sc);
        save_run::write_links(&ctx, run, stage, &links, true).map(|_| ()).map_err(fail)
    };

    let run = format!("_tpm/runs/{RUN}");
    let data = json!({"epic": CHANGE, "args": {"change": CHANGE}});
    put(root, &format!("{run}/run.json"), &data.to_string());
    let sc = save_run::open_spec(&ctx, &run, &data, &profile).ok_or("需求來源沒有被當成 openspec")??;
    if sc.change.as_deref() != Some(CHANGE) {
        return Err(format!("找不到進行中的變更 {CHANGE}"));
    }
    let sc = Some(&sc);
    let acc = format!("{SCN} 下載 CSV");
    save(&run, "plan", &json!({}), sc)?;
    save(&run, "design", &design_of(CHANGE, &acc, &[REQ], &["src/billing/export.ts", "src/billing/old.ts"]), sc)?;
    // 重存 design：先前宣告的 old.ts 作廢
    save(&run, "design", &design_of(CHANGE, &acc, &[REQ], &["src/billing/export.ts", "src/billing/gone.ts"]), sc)?;
    save(&run, "build", &json!({"t1": env(json!({"task_id": "t1", "branch": "b", "commits": [], "files_changed": ["src/billing/format.ts"], "tests": []}))}), sc)?;
    save(&run, "test", &test_of(SCN, "npx vitest src/billing/export.test.ts", SHA), sc)?;
    save(&run, "deliver", &json!({"deliverables": {"docs": [], "updated": [{"id": "guide", "file": "docs/guide.md", "sections": ["匯出"]}], "summary": null}}), sc)?;
    save(&run, "merge", &json!({"merged": true, "head_sha": MERGE_SHA}), sc)?;
    put(root, "_tpm/runs/decisions.jsonl", &format!("{DECISION}\n"));
    let links = save_run::decision_links(&ctx, &run, "decide", 1, sc);
    save_run::write_links(&ctx, &run, "decide", &links, false).map_err(fail)?;

    let old = format!("_tpm/runs/{OLD_RUN}");
    put(root, &format!("{old}/run.json"), &json!({"epic": "legacy"}).to_string());
    save(&old, "plan", &json!({}), None)?;
    save(&old, "design", &design_of("legacy", "AC-1 匯出要有表頭", &["docs/guide.md#其他"], &["src/lib/money.ts"]), None)?;
    save(&old, "test", &test_of("AC-1", "make check", ""), None)?;
    Ok(())
}

fn project(root: &Path) {
    put(root, ".claude/teams/guard.config.json", "{\"runs_dir\": \"_tpm/runs\"}\n");
    put(root, "tsconfig.json", "{\n  // 路徑別名\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@lib/*\": [\"src/lib/*\"], }, },\n}\n");
    put(root, "src/billing/export.ts", "import { formatRow } from \"./format\";\nimport { toCents } from \"@lib/money\";\nimport axios from \"axios\";\n\n// @awf:billing/export-invoice\nexport function exportInvoice(rows: number[]): string {\n  const marker = \"BODY_TEXT_NOT_IN_OUTPUT\";\n  return rows.map((r) => formatRow(toCents(r))).join(marker);\n}\n");
    put(root, "src/billing/format.ts", "export function formatRow(n: number): string {\n  return String(n);\n}\n");
    put(root, "src/lib/money.ts", "export function toCents(n: number): number {\n  return n * 100;\n}\n");
    put(root, "src/billing/export.test.ts", "import { exportInvoice } from \"./export\";\n\n// @awf:billing/export-invoice#csv-download\nfunction testCsv() {\n  exportInvoice([1]);\n}\n\n// @awf:billing/no-such-requirement\nfunction testOther() {}\n");
    put(root, "src/jobs/__init__.py", "");
    put(root, "src/jobs/nightly.py", "from .util import helper\n\n\ndef run():\n    return helper()\n");
    put(root, "src/jobs/util.py", "def helper():\n    return 1\n");
    put(root, "src/jobs/secrets.py", "TOKEN = 'x'\n");
    put(root, "ignored/skip.ts", "export function skipped() {}\n");
    put(root, ".gitignore", "ignored/\n");
    put(
        root,
        "openspec/specs/billing/spec.md",
        "# billing Specification\n\n## Purpose\n示範客戶 ACME 的帳務。\n\n## Requirements\n\n### Requirement: Export invoice\nThe system SHALL export invoices.\n\n#### Scenario: CSV download\n- **WHEN** the user clicks export\n- **THEN** a CSV file is downloaded\n\n### Requirement: Archive invoice\nThe system SHALL archive invoices.\n\n#### Scenario: Yearly archive\n- **WHEN** the year ends\n- **THEN** invoices are archived\n",
    );
    put(root, "docs/guide.md", "# 使用手冊\n\n## 匯出\n\n匯出的程式在 `src/billing/export.ts`，需求是 `billing/export-invoice`。\n另見 [格式](../src/billing/format.ts)。\n\n## 其他\n\n沒有內容。\n");
    put(root, "_tpm/kb/vault/notes/idea.md", "# 想法\n\n之後要看 [[billing/export-invoice]] 能不能支援別的格式，還有 [[billing/nothing-here]]。\n");
    put(
        root,
        &format!("openspec/changes/{CHANGE}/specs/billing/spec.md"),
        "## MODIFIED Requirements\n\n### Requirement: Export invoice\nThe system SHALL export invoices as CSV.\n\n#### Scenario: CSV download\n- **WHEN** the user clicks export\n- **THEN** a CSV file with a header row is downloaded\n",
    );
    put(root, &format!("openspec/changes/{CHANGE}/proposal.md"), "# 匯出 CSV\n\n## Why\n示範客戶 ACME 要能下載 CSV。\n");
    put(root, ".claude/teams/deliverables.json", "{\"dir\": \"docs\", \"documents\": [{\"id\": \"guide\", \"title\": \"使用手冊\", \"source\": \"assisted\", \"sections\": [\"匯出\"]}]}\n");
}

/// 帳本後面接兩行讀不懂的（一行不是 JSON，一行是定案前的舊格式）。
fn append_bad_lines(root: &Path) {
    let p = root.join("_tpm/runs/links.jsonl");
    let mut text = std::fs::read_to_string(&p).unwrap_or_default();
    text.push_str("這一行不是 JSON\n{\"kind\":\"ac-file\",\"from\":\"a\",\"to\":\"src/x.ts\"}\n");
    let _ = std::fs::write(p, text);
}

fn has(ans: &Answer, group: &str, needle: &str) -> bool {
    ans.items.iter().any(|i| i.group == group && (i.path == needle || i.id == needle))
}

fn opts(root: &Path) -> BuildOpts {
    BuildOpts { root: Some(root.to_string_lossy().into_owned()), ..Default::default() }
}

fn edge(db: &str, src: &str, dst: &str, kind: &str) -> bool {
    store::open_ro(db).ok().and_then(|c| c.query_row("SELECT 1 FROM edges WHERE src=? AND dst=? AND kind=?", [src, dst, kind], |r| r.get::<_, i64>(0)).ok()).is_some()
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=awf", "-c", "user.email=awf@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run_all(root: &Path, t: &mut Tally) -> Result<(), String> {
    let rs = root.to_string_lossy().into_owned();
    let cli = |args: &[&str]| -> (i32, String, String) {
        let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        v.push("--root".into());
        v.push(rs.clone());
        run_captured(&v)
    };

    println!("圖還沒建");
    let (code, _, err) = cli(&["locate", "export"]);
    t.check("沒有圖時查詢回非零並說明怎麼建", code == 1 && err.contains("awf kg build"), &err);

    println!("建圖");
    write_ledger(root)?;
    let ledger_text = std::fs::read_to_string(root.join("_tpm/runs/links.jsonl")).unwrap_or_default();
    let written: std::collections::BTreeSet<String> = save_run::active_links(&ledger_text).iter().filter_map(|l| l["kind"].as_str().map(str::to_string)).collect();
    t.check("帳本由 save_run 的寫入端寫出：七種連結都有、有一筆 supersede", written.len() == save_run::LINK_KINDS.len() && save_run::LINK_KINDS.iter().all(|k| written.contains(*k)) && ledger_text.contains("\"supersede\""), &format!("{written:?}"));
    append_bad_lines(root);
    let r = build(&opts(root))?;
    let lay = layout(Some(&rs), None, None, None)?;
    let db = lay.db.clone();
    t.check("第一次建圖", r.outcome == Outcome::Updated && r.changed == r.files && r.files >= 8, &format!("{r:?}"));
    t.check("機敏檔不收", r.skipped.iter().any(|(p, why)| p == "src/jobs/secrets.py" && why == "機敏檔"), &format!("{:?}", r.skipped));
    t.check("帳本的壞行略過並計數", r.bad_links == 2, &r.bad_links.to_string());
    let g = query::open(&lay)?;
    t.check("尊重 .gitignore", g.node("file:ignored/skip.ts").is_none() && g.node("file:src/billing/format.ts").is_some(), "");
    t.check("規格的需求與情境有穩定 id", g.node(&format!("req:{REQ}")).is_some() && g.node(&format!("scn:{SCN}")).is_some(), "");
    t.check("節點 id 不含行號", g.node("sym:src/billing/export.ts#exportInvoice").is_some_and(|n| n.start == 6 && n.end == 9), "");
    t.check("相對路徑 import", edge(&db, "file:src/billing/export.ts", "file:src/billing/format.ts", "imports"), "");
    t.check("tsconfig 的 paths", edge(&db, "file:src/billing/export.ts", "file:src/lib/money.ts", "imports"), "");
    t.check("已 import 的名字的呼叫", edge(&db, "sym:src/billing/export.ts#exportInvoice", "sym:src/billing/format.ts#formatRow", "calls"), "");
    t.check("Python 的相對 import", edge(&db, "file:src/jobs/nightly.py", "file:src/jobs/util.py", "imports") && edge(&db, "sym:src/jobs/nightly.py#run", "sym:src/jobs/util.py#helper", "calls"), "");
    let unres: i64 = g.con.query_row("SELECT COUNT(*) FROM unresolved WHERE kind='import' AND target='axios' AND reason='external'", [], |r| r.get(0)).unwrap_or(0);
    t.check("解析不了的 import 進 unresolved", unres == 1, "");
    t.check("@awf 標記收成 strong 連結", g.out_edges("sym:src/billing/export.ts#exportInvoice").iter().any(|e| e.kind == "marks" && e.dst == format!("req:{REQ}") && e.strength == "strong" && e.provenance == "mark"), "");
    t.check("筆記的 [[需求 id]] 收成 weak 連結", g.in_edges(&format!("req:{REQ}")).iter().any(|e| e.kind == "mentions" && e.strength == "weak" && e.provenance == "note:wikilink"), "");
    t.check("文件提到的路徑收成 weak 連結", g.in_edges("file:src/billing/export.ts").iter().any(|e| e.kind == "mentions" && e.src == "sec:docs/guide.md#使用手冊/匯出"), "");
    let m = &g.meta;
    t.check("兩個版本號與 index_state", m.get("schema_version").map(String::as_str) == Some(store::SCHEMA_VERSION) && m.get("content_version").map(String::as_str) == Some(store::CONTENT_VERSION) && m.get("index_state").map(String::as_str) == Some(store::STATE_READY), &format!("{m:?}"));

    println!("查詢");
    let a = query::locate(&g, "export invoice csv", 12);
    t.check("locate：找到程式與規格", has(&a, "位置", "src/billing/export.ts") && has(&a, "位置", "openspec/specs/billing/spec.md"), &format!("{:?}", a.items));
    let a = query::locate(&g, "money cents", 30);
    // 第二層擴展出來的：理由寫的中間節點要真的和這個節點有邊
    let second: Vec<(&str, &str)> = a.items.iter().filter_map(|i| Some((i.id.as_str(), i.reason.split_once("連到 ")?.1.split_once("（它連到命中的 ")?.0))).collect();
    let linked = |id: &str, mid: &str| g.out_edges(id).iter().any(|e| e.dst.split_once(':').is_some_and(|x| x.1 == mid)) || g.in_edges(id).iter().any(|e| e.src.split_once(':').is_some_and(|x| x.1 == mid));
    t.check("locate：第二層的理由寫實際經過的中間節點", !second.is_empty() && second.iter().all(|(id, mid)| linked(id, mid)), &format!("{:?}", a.items));
    let (code, out, _) = cli(&["locate", "export invoice csv"]);
    t.check("locate：輸出包在資料標籤裡、只有路徑與行號", code == 0 && out.starts_with(query::OPEN_TAG) && out.trim_end().ends_with(query::CLOSE_TAG) && out.contains("src/billing/export.ts:6-9"), &out);
    t.check("輸出不貼原文", !out.contains("BODY_TEXT_NOT_IN_OUTPUT") && !out.contains("SHALL") && !out.contains("含稅"), &out);
    let dec = format!("dec:{}", save_run::decision_id(DECISION));
    let kinds_in = |a: &Answer| -> Vec<&'static str> { save_run::LINK_KINDS.iter().copied().filter(|k| a.items.iter().any(|i| i.reason.starts_with(&format!("{k}・strong・ledger:")))).collect() };
    let a = query::trace(&g, REQ)?;
    t.check("trace：規格、run、檔案、測試、commit、決策、交付文件小節", has(&a, "規格", "openspec/specs/billing/spec.md") && has(&a, "run", &format!("_tpm/runs/{RUN}")) && has(&a, "檔案與符號", "src/billing/export.ts") && has(&a, "測試", "src/billing/export.test.ts") && has(&a, "commit", &format!("commit:{SHA}")) && has(&a, "commit", &format!("commit:{MERGE_SHA}")) && has(&a, "決策", &dec) && has(&a, "交付文件小節", "docs/guide.md"), &format!("{:?}", a.items));
    t.check("trace：帳本的七種連結每一種都走得到", kinds_in(&a).len() == save_run::LINK_KINDS.len(), &format!("{:?}", kinds_in(&a)));
    t.check("trace：build 段實際改到的檔也連上", has(&a, "檔案與符號", "src/billing/format.ts"), &format!("{:?}", a.items));
    t.check("trace：supersede 之前的連結不算", !a.items.iter().any(|i| i.id == "file:src/billing/old.ts") && has(&a, "檔案與符號", "file:src/billing/gone.ts"), &format!("{:?}", a.items));
    let a = query::trace(&g, SCN)?;
    t.check("trace：從情境也走得到決策（記在所屬的需求與變更上）與交付文件小節", has(&a, "決策", &dec) && has(&a, "交付文件小節", "docs/guide.md") && has(&a, "規格小節", "openspec/specs/billing/spec.md"), &format!("{:?}", a.items));
    let a = query::trace(&g, CHANGE)?;
    t.check("trace：變更名稱走得到 run 與它動到的需求", has(&a, "run", &format!("_tpm/runs/{RUN}")) && a.items.iter().any(|i| i.group == "規格" && i.id == format!("req:{REQ}")), &format!("{:?}", a.items));
    let a = query::trace(&g, "legacy")?;
    t.check("trace：需求來源不是規格層時，epic 走得到 run、AC、spec 小節、檔案、驗證指令", has(&a, "run", &format!("_tpm/runs/{OLD_RUN}")) && has(&a, "規格", "ac:AC-1") && has(&a, "規格小節", "docs/guide.md") && has(&a, "檔案與符號", "src/lib/money.ts") && has(&a, "測試", "cmd:make check"), &format!("{:?}", a.items));
    t.check("trace：說明缺 commit", a.notes.iter().any(|n| n.contains("commit")), &format!("{:?}", a.notes));
    t.check("trace：不認得的 id 回錯誤", query::trace(&g, "billing/nope").is_err(), "");
    let a = query::impact(&g, &["src/billing/format.ts".to_string()]);
    t.check("impact：引用它的程式", has(&a, "引用它的程式", "src/billing/export.ts"), &format!("{:?}", a.items));
    t.check("impact：間接匯入它的測試", has(&a, "受影響的測試", "src/billing/export.test.ts"), &format!("{:?}", a.items));
    let a = query::impact(&g, &["src/billing/export.ts".to_string()]);
    t.check("impact：走回需求、決策、文件小節、交付文件小節", has(&a, "需求", "openspec/specs/billing/spec.md") && has(&a, "決策", &dec) && has(&a, "文件小節", "docs/guide.md") && has(&a, "交付文件小節", "docs/guide.md"), &format!("{:?}", a.items));
    let a = query::impact(&g, &["formatRow".to_string()]);
    t.check("impact：可以給符號名", has(&a, "引用它的程式", "src/billing/export.ts"), &format!("{:?}", a.items));
    let a = query::check(&g, None);
    t.check("check：連到的檔案不見了", a.items.iter().any(|i| i.group == "斷掉的連結" && i.id == "file:src/billing/gone.ts"), &format!("{:?}", a.items));
    t.check("check：標記與筆記指到不存在的 id", a.items.iter().any(|i| i.reason.contains("billing/no-such-requirement")) && a.items.iter().any(|i| i.reason.contains("billing/nothing-here")), &format!("{:?}", a.items));
    t.check("check：缺的環節", a.items.iter().any(|i| i.group == "缺的連結" && i.id == "ac:AC-1" && i.reason.contains("commit") && !i.reason.contains("測試")) && !a.items.iter().any(|i| i.group == "缺的連結" && i.id == format!("scn:{SCN}")), &format!("{:?}", a.items));
    let a = query::check(&g, Some("別的-run"));
    t.check("check --run：只看那個 run", a.items.is_empty(), &format!("{:?}", a.items));
    let a = query::check(&g, Some(OLD_RUN));
    t.check("check --run：另一個 run 斷掉的檔不算進來", a.items.iter().any(|i| i.id == "ac:AC-1") && !a.items.iter().any(|i| i.id == "file:src/billing/gone.ts"), &format!("{:?}", a.items));
    let a = query::neighbors(&g, "src/billing/format.ts#formatRow", 2)?;
    t.check("neighbors：兩層內的鄰居", has(&a, "距離 1", "src/billing/export.ts") && a.items.iter().any(|i| i.group == "距離 2" && i.id == format!("req:{REQ}")), &format!("{:?}", a.items));
    let (code, out, _) = cli(&["trace", SCN, "--json"]);
    let parsed = serde_json::from_str::<serde_json::Value>(&out).ok();
    t.check("--json 給機器讀", code == 0 && parsed.is_some_and(|v| v["items"].as_array().is_some_and(|a| !a.is_empty())), &out);
    let (_, small, _) = cli(&["neighbors", "src/billing/export.ts", "--depth", "3", "--budget", "200"]);
    t.check("超過字數上限會截斷並說明", small.contains("已截斷"), &small);
    t.check("字數上限分三級", g.budget() == 2400 && query::TIERS.len() == 3, "");
    drop(g);

    println!("可重現與增量");
    let d1 = store::dump_all(&db)?;
    let r = build(&opts(root))?;
    t.check("沒有改動時回「沒變」", r.outcome == Outcome::Unchanged && r.changed == 0, &format!("{r:?}"));
    let r = build(&BuildOpts { full: true, ..opts(root) })?;
    let d2 = store::dump_all(&db)?;
    t.check("同一份輸入建兩次，所有資料表內容相同", r.changed == r.files && d1 == d2, "");
    put(root, "src/billing/format.ts", "export function formatRow(n: number): string {\n  return pad(String(n));\n}\n\nfunction pad(s: string): string {\n  return s;\n}\n");
    let _ = std::fs::remove_file(root.join("src/jobs/util.py"));
    let r = build(&opts(root))?;
    t.check("只重算變了的檔", r.outcome == Outcome::Updated && r.changed == 1 && r.removed == 1, &format!("{r:?}"));
    let g = query::open(&lay)?;
    t.check("新的符號進圖、刪掉的檔的連結變成未解析", g.node("sym:src/billing/format.ts#pad").is_some() && g.node("file:src/jobs/util.py").is_none() && !edge(&db, "file:src/jobs/nightly.py", "file:src/jobs/util.py", "imports"), "");
    drop(g);
    let inc = store::dump_all(&db)?;
    build(&BuildOpts { full: true, ..opts(root) })?;
    t.check("增量的結果和整份重建相同", inc == store::dump_all(&db)?, "");
    put(root, "src/jobs/util.py", "def helper():\n    return 1\n");
    build(&opts(root))?;
    t.check("目標回來後未解析的 import 自動接上", edge(&db, "file:src/jobs/nightly.py", "file:src/jobs/util.py", "imports"), "");

    println!("帳本的格式版本");
    let ledger_path = root.join("_tpm/runs/links.jsonl");
    let ledger_before = std::fs::read_to_string(&ledger_path).unwrap_or_default();
    let newer = json!({"schema_version": save_run::LINK_SCHEMA_VERSION + 1, "op": "add", "run": RUN, "stage": "design"});
    let _ = std::fs::write(&ledger_path, format!("{ledger_before}{newer}\n"));
    let (code, out, _) = cli(&["build"]);
    let ledger_edges = |db: &str| store::open_ro(db).ok().and_then(|c| c.query_row("SELECT COUNT(*) FROM edges WHERE owner='@ledger'", [], |r| r.get::<_, i64>(0)).ok()).unwrap_or(-1);
    t.check("帳本的格式比認得的新：整個帳本略過並在 build 訊息說明", code == 0 && out.contains("整個帳本略過") && ledger_edges(&db) == 0, &out);
    let _ = std::fs::write(&ledger_path, &ledger_before);
    build(&opts(root))?;
    t.check("帳本換回認得的版本後連結回來", ledger_edges(&db) > 0, "");

    println!("逾時與中斷");
    put(root, "src/billing/format.ts", "export function formatRow(n: number): string {\n  return String(n);\n}\n");
    let before = std::fs::read(&db).map_err(|e| e.to_string())?;
    let r = build(&BuildOpts { timeout: Some(0), ..opts(root) })?;
    t.check("逾時：放棄這次、上一版的圖原封不動", r.outcome == Outcome::TimedOut && std::fs::read(&db).is_ok_and(|b| b == before), &format!("{:?}", r.outcome));
    let (code, out, _) = cli(&["build", "--timeout", "0"]);
    t.check("逾時：指令回非零並說明", code == 3 && out.contains("逾時"), &out);
    let left: Vec<String> = std::fs::read_dir(&lay.dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".tmp-")).collect()).unwrap_or_default();
    t.check("沒有留下暫存檔", left.is_empty(), &format!("{left:?}"));
    let u = update(Some(&rs), Some(0));
    t.check("流程接點：逾時回狀態與一行說明", u.status == UpdateStatus::TimedOut && !u.note.contains('\n'), &u.note);
    let u = update(Some(&rs), None);
    t.check("流程接點：更新", u.status == UpdateStatus::Updated, &u.note);
    let u = update(Some(&rs), None);
    t.check("流程接點：沒變", u.status == UpdateStatus::Unchanged, &u.note);
    let u = update(Some(&format!("{rs}/沒有這個目錄")), None);
    t.check("流程接點：失敗不 panic", u.status == UpdateStatus::Failed, &u.note);
    let half = root.join("half");
    let _ = std::fs::create_dir_all(&half);
    let half_db = half.join(store::DB_NAME);
    let _ = std::fs::copy(&db, &half_db);
    if let Ok(c) = rusqlite::Connection::open(&half_db) {
        let _ = store::set_meta(&c, "index_state", store::STATE_BUILDING);
    }
    let (code, _, err) = cli(&["check", "--dir", &half.to_string_lossy()]);
    t.check("建到一半中斷的圖看得出來", code == 1 && err.contains("沒有建完") && err.contains("--full"), &err);
    let _ = std::fs::remove_dir_all(&half);

    println!("匯出");
    let g = query::open(&lay)?;
    let vault = PathBuf::from(&lay.dir).join("vault");
    put(&vault, "能力/手寫的.md", "人放在這裡的檔\n");
    let note_before = std::fs::read(vault.join("notes/idea.md")).unwrap_or_default();
    let r1 = export::export(&g, &lay, None)?;
    let read = |rel: &str| std::fs::read_to_string(vault.join(rel)).unwrap_or_default();
    let page = read("需求/billing/export-invoice.md");
    let dec_name = save_run::decision_id(DECISION);
    let dec_page = format!("決策/{dec_name}.md");
    t.check("每個能力、需求、決策、模組一頁，加 hot 與索引", ["能力/billing.md", "需求/billing/export-invoice.md", dec_page.as_str(), "模組/src/billing.md", "hot.md", "index.md", "需求/_index.md"].iter().all(|p| vault.join(p).is_file()), &format!("{} 頁", r1.pages));
    t.check("頁面有 frontmatter、[[連結]] 與 路徑:行號", page.starts_with("---\n") && page.contains("[[能力/billing]]") && page.contains(&format!("[[決策/{dec_name}]]")) && page.contains("`src/billing/export.ts:1-9`") && page.contains("[[notes/idea]]"), &page);
    t.check("頁面不放原文", !page.contains("SHALL") && !read(&dec_page).contains("含稅"), &page);
    t.check("不碰 notes/ 與不是工具產生的檔", std::fs::read(vault.join("notes/idea.md")).is_ok_and(|b| b == note_before) && read("能力/手寫的.md") == "人放在這裡的檔\n" && r1.kept == vec!["能力/手寫的.md".to_string()], &format!("{:?}", r1.kept));
    let snap = |dir: &Path| -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap_or_default()));
                }
            }
        }
        out.sort();
        out
    };
    let s1 = snap(&vault);
    put(&vault, "需求/billing/過期的頁.md", &format!("---\n{}\n---\n", export::MARK));
    export::export(&g, &lay, None)?;
    t.check("匯出可重現，舊頁會清掉", s1 == snap(&vault), "");
    drop(g);
    let (code, out, _) = cli(&["export", "--out", "exported"]);
    t.check("export --out", code == 0 && root.join("exported/hot.md").is_file(), &out);
    let _ = std::fs::remove_dir_all(root.join("exported"));

    println!("git（沒有 git 時略過）");
    if git_ok(root, &["init", "-q"]) && git_ok(root, &["add", "src/billing/format.ts"]) && git_ok(root, &["commit", "-q", "-m", "init"]) {
        let r = build(&opts(root))?;
        let g = query::open(&lay)?;
        t.check("檔案 ↔ commit", r.outcome == Outcome::Updated && g.in_edges("file:src/billing/format.ts").iter().any(|e| e.kind == "touches" && e.provenance == "git"), &format!("{r:?}"));
        put(root, "src/billing/format.ts", "export function formatRow(n: number): string {\n  return `${n}`;\n}\n");
        let a = query::impact(&g, &query::diff_files(&rs, "HEAD").unwrap_or_default());
        t.check("impact --diff", has(&a, "引用它的程式", "src/billing/export.ts"), &format!("{:?}", a.items));
    } else {
        println!("  [略過] 這台機器跑不了 git");
    }
    Ok(())
}

pub fn selftest() -> i32 {
    let root = std::env::temp_dir().join(format!("awf-kg-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    if std::fs::create_dir_all(&root).is_err() {
        println!("kg selftest：建不出暫存目錄");
        return 1;
    }
    let root = root.canonicalize().unwrap_or(root);
    project(&root);
    let mut t = Tally { passed: 0, failed: 0 };
    if let Err(msg) = run_all(&root, &mut t) {
        t.failed += 1;
        println!("  [失敗] 中途出錯：{msg}");
    }
    let _ = std::fs::remove_dir_all(&root);
    println!("\nkg selftest：{} 通過、{} 失敗", t.passed, t.failed);
    i32::from(t.failed > 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        assert_eq!(super::selftest(), 0);
    }
}
