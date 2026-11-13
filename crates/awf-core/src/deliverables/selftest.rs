//! `--selftest`：和 Python 版同樣的 107 項檢查，在暫存目錄裡跑一遍。

use super::core::*;
use super::flow::{run as flow_run, *};
use super::py;
use super::*;
use crate::kb::pyish;
use crate::pyfmt;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

fn w(root: &str, rel: &str, text: &str) {
    write_lf(&pyish::pjoin(root, rel), text);
}

fn rd(root: &str, rel: &str) -> String {
    read_text(&pyish::pjoin(root, rel)).unwrap_or_default()
}

fn rm(root: &str, rel: &str) {
    let _ = std::fs::remove_file(pyish::pjoin(root, rel));
}

fn exists(root: &str, rel: &str) -> bool {
    std::path::Path::new(&pyish::pjoin(root, rel)).exists()
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn ids(list: &Value) -> Vec<String> {
    pyfmt::items(Some(list)).iter().map(|d| sget(d, "id").to_string()).collect()
}

fn by_id(list: &[Value], key: &str) -> Map<String, Value> {
    list.iter().map(|d| (sget(d, "id").to_string(), get(d, key).clone())).collect()
}

fn status_of(root: &str) -> Map<String, Value> {
    status_all(root, None).0.unwrap_or_default().into_iter().map(|d| (sget(&d, "id").to_string(), d)).collect()
}

fn st_is(st: &Map<String, Value>, id: &str, status: &str) -> bool {
    st.get(id).is_some_and(|d| sget(d, "status") == status)
}

fn detail(st: &Map<String, Value>, id: &str) -> String {
    st.get(id).map(|d| sget(d, "detail").to_string()).unwrap_or_default()
}

fn sec_details(st: &Map<String, Value>, id: &str) -> String {
    st.get(id).map(|d| pyfmt::items(d.get("sections")).iter().map(|s| sget(s, "detail").to_string()).collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn checks(root: &str) -> Vec<String> {
    check_all(root, None).0.unwrap_or_else(|| vec!["（沒有交付設定）".to_string()])
}

fn with_change(root: &str, doc_id: &str, old: &str, new: &str) -> Vec<String> {
    let rel = format!("{DEFAULT_DIR}/{doc_id}.md");
    let keep = rd(root, &rel);
    w(root, &rel, &keep.replacen(old, new, 1));
    let got = checks(root);
    w(root, &rel, &keep);
    got
}

fn fill(root: &str, doc_id: &str, body: &str) {
    let rel = format!("{DEFAULT_DIR}/{doc_id}.md");
    w(root, &rel, &rd(root, &rel).replace(PLACEHOLDER, body));
}

fn with_key(v: &Value, k: &str, x: Value) -> Value {
    let mut m = v.as_object().cloned().unwrap_or_default();
    m.insert(k.to_string(), x);
    Value::Object(m)
}

fn md_snapshot(dir: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(dir).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| n.ends_with(".md"))
        .map(|n| { let t = read_text(&format!("{dir}/{n}")).unwrap_or_default(); (n, t) }).collect()).unwrap_or_default();
    v.sort();
    v
}

const DB_SECS: [&str; 4] = ["資料表與欄位", "關聯與索引", "資料遷移", "資料保存與清理"];

fn reviewed_db() -> Vec<String> {
    DB_SECS.iter().map(|s| format!("db-design#{s}")).collect()
}

/// 跑全部檢查；結果印在 `out`，回傳 exit code。
pub fn selftest(out: &mut String) -> i32 {
    let mut results: Vec<(bool, String, String)> = Vec::new();
    macro_rules! check {
        ($name:expr, $ok:expr) => {
            results.push(($ok, $name.to_string(), String::new()))
        };
        ($name:expr, $ok:expr, $detail:expr) => {
            results.push(($ok, $name.to_string(), $detail.to_string()))
        };
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("awf-deliv-{}-{nanos}", std::process::id()));
    let _ = std::fs::create_dir_all(&td);
    let td_s = td.to_string_lossy().replace('\\', "/");
    let root_s = pyish::resolve(&td_s).unwrap_or(td_s);
    let root = root_s.as_str();
    let teams = ".claude/teams";
    let cfg_rel = format!("{teams}/{CONFIG_NAME}");
    let pm = "示範客戶 ACME 的 PM";
    w(root, &format!("{teams}/{CATALOG_REL}"), &py::dumps(&builtin_catalog(), None));
    let cat = load_catalog(root);

    // ---- 型錄
    let groups: Vec<String> = ids(get(&cat, "groups"));
    let docs = pyfmt::items(cat.get("documents"));
    check!("型錄：八組都有文件", groups.len() == 8 && groups.iter().all(|g| docs.iter().any(|d| sget(d, "group") == g)), format!("{groups:?}"));
    check!("型錄：每筆都有來源類型、標題、更新時機", docs.iter().all(|d| SOURCE_TYPES.contains(&sget(d, "source")) && !sget(d, "title").is_empty() && WHENS.contains(&sget(d, "when"))));
    let uniq: BTreeSet<&str> = docs.iter().map(|d| sget(d, "id")).collect();
    check!("型錄：id 不重複、至少 40 種", uniq.len() == docs.len() && docs.len() >= 40, docs.len());
    let kinds: BTreeSet<&str> = docs.iter().map(|d| sget(d, "source")).collect();
    check!("型錄：三種來源類型都有", kinds == SOURCE_TYPES.iter().copied().collect::<BTreeSet<&str>>());
    let idx = catalog_index(&cat);
    let presets = pyfmt::items(cat.get("presets"));
    check!("型錄：情境組合五個、列的文件都在型錄裡", presets.len() == 5 && presets.iter().all(|p| pyfmt::items(p.get("documents")).iter().all(|i| i.as_str().is_some_and(|i| idx.contains_key(i)))));
    check!("型錄：非外部文件都有小節", docs.iter().filter(|d| sget(d, "source") != "external").all(|d| !pyfmt::items(d.get("sections")).is_empty()));

    // ---- 沒有交付設定：什麼都不做
    let (cfg0, note0) = load_config(root);
    check!("沒有交付設定：load_config 回 None 並說明", cfg0.is_none() && note0.contains("configure"), note0);
    check!("沒有交付設定：status、check、init、sync 都不做事", status_all(root, None).0.is_none() && check_all(root, None).0.is_none() && init_docs(root).0 == 0 && !exists(root, DEFAULT_DIR));
    let run_s = pyish::pjoin(root, "_tpm/runs/r1");
    let run = run_s.as_str();
    let run_rel = "_tpm/runs/r1";
    w(root, &format!("{run_rel}/run.json"), &py::dumps(&json!({"requirement": "訂單匯出", "target_repo": ".", "args": {"epic": "export"},
        "recipe": {"name": "feature", "modifiers": ["migration"], "resolved": {"name": "feature", "modifiers": ["migration"], "stages": ["plan", "design", "build", "test", "deliver", "merge"]}},
        "stages": {"plan": {"approved": {"by": pm, "at": "2026-01-02T03:04:05Z", "kind": "human"}},
                   "test": {"approved": {"by": "policy", "at": "2026-01-03T00:00:00Z", "kind": "policy"}}}}), None));
    let load_run = || load_json(&pyish::pjoin(run, "run.json")).unwrap_or(Value::Null);
    let empty_pack = |r: (Value, Vec<String>)| r.0 == json!({}) && r.1.is_empty();
    check!("沒有交付設定：pack_args 回空的（流程不變）", empty_pack(pack_args("deliver", run, root, None, &json!({}), Some(&args(&["db/migrations/001.sql"])))));
    w(root, &format!("{teams}/methodology.json"), &py::dumps(&json!({"deliverables": {"enabled": false, "documents": [{"id": "db-design"}]}}), None));
    check!("profile 的 deliverables 沒有 enabled: true 不算啟用", load_config(root).0.is_none());
    w(root, &format!("{teams}/methodology.json"), &py::dumps(&json!({"deliverables": {"enabled": true, "documents": [{"id": "db-design"}]}}), None));
    check!("profile 的 deliverables enabled: true 才啟用", load_config(root).0.is_some_and(|c| get(&c, "documents") == &json!([{"id": "db-design"}])));
    rm(root, &format!("{teams}/methodology.json"));

    // ---- configure
    let (code, res, notes) = configure(root, &args(&["--preset", "hackathon"]));
    check!("configure：情境組合可套用", code == 0 && ids(get(&res, "documents")) == ["sys-arch", "release-notes", "delivery-summary"]
        && pyfmt::items(res.get("documents")).iter().all(|d| sget(d, "reason").contains("黑客松")), get(&res, "documents"));
    check!("configure：沒確認不寫入", get(&res, "written") == &json!(false) && !exists(root, &cfg_rel) && notes.iter().any(|n| n.contains("還沒寫入")));
    check!("configure：--write 沒帶 --by 拒絕", configure(root, &args(&["--preset", "hackathon", "--write"])).0 == 2 && !exists(root, &cfg_rel));
    check!("configure：沒有的情境組合回錯誤", configure(root, &args(&["--preset", "nope"])).0 == 2);
    w(root, "db/migrations/001_orders.sql", "create table orders (id int);\n");
    w(root, "src/app.py", "print('hi')\n");
    let (_, res, _) = configure(root, &args(&["--suggest"]));
    let sug = by_id(pyfmt::items(res.get("suggested")), "reason");
    let nos = by_id(pyfmt::items(res.get("not_suggested")), "reason");
    let has_in = |m: &Map<String, Value>, id: &str, part: &str| m.get(id).and_then(Value::as_str).is_some_and(|r| r.contains(part));
    check!("configure：有 migration 才建議資料庫設計文件（附理由與例子）", has_in(&sug, "db-design", "001_orders.sql"), Value::Object(sug.clone()));
    check!("configure：沒有 API 定義不建議 API 規格書（附理由）", has_in(&nos, "api-spec", "找不到") && !sug.contains_key("api-spec"), get(&Value::Object(nos.clone()), "api-spec"));
    check!("configure：沒有排程不建議批次作業設計", nos.contains_key("batch-design") && !sug.contains_key("batch-design"));
    w(root, "api/openapi.yaml", "openapi: 3.0.0\n");
    w(root, "_tpm/runs/old/run.json", &py::dumps(&json!({"recipe": {"resolved": {"modifiers": ["ui"]}}}), None));
    let sug2 = by_id(pyfmt::items(configure(root, &args(&["--suggest"])).1.get("suggested")), "reason");
    check!("configure：有 API 定義就建議；用過 ui 的 run 建議畫面規格", sug2.contains_key("api-spec") && has_in(&sug2, "ui-spec", "ui"), Value::Object(sug2.clone()));
    let _ = std::fs::remove_dir_all(pyish::pjoin(root, "_tpm/runs/old"));
    rm(root, "api/openapi.yaml");
    w(root, "合約清單.txt", "1. 系統架構設計書\n2. 資料庫規格\n- 測試報告\n3. 專案人員保密切結書\n# 註解\n");
    let list_file = pyish::pjoin(root, "合約清單.txt");
    let (_, res, _) = configure(root, &args(&["--import", &list_file]));
    let got = ids(get(&res, "documents"));
    check!("configure：匯入清單對到型錄（含別名）", ["sys-arch", "db-design", "test-report"].iter().all(|i| got.iter().any(|g| g == i)), format!("{got:?}"));
    check!("configure：對不到的列出來、不自動新增", get(&res, "unmatched") == &json!(["3. 專案人員保密切結書"]) && !got.iter().any(|i| i.starts_with("custom-")), get(&res, "unmatched"));
    let (_, res, _) = configure(root, &args(&["--import", &list_file, "--custom-unmatched"]));
    let cust: Vec<&Value> = pyfmt::items(get(&res, "config").get("documents")).iter().filter(|d| sget(d, "id").starts_with("custom-")).collect();
    check!("configure：--custom-unmatched 才新增成自訂文件", cust.len() == 1 && cust.first().is_some_and(|c| sget(c, "title") == "專案人員保密切結書" && sget(c, "source") == "assisted"), format!("{cust:?}"));
    let (code, res, notes) = configure(root, &args(&["--add", "db-design,api-spec,sys-arch,biz-rules,rtm,dev-record,vuln-scan-report,ops-manual,project-plan", "--write", "--by", pm]));
    let cfg = load_config(root).0.unwrap_or(Value::Null);
    check!("configure：確認後寫入，記下誰確認", code == 0 && get(&res, "written") == &json!(true) && sget(&cfg, "confirmed_by") == pm && pyfmt::items(cfg.get("documents")).len() == 9, format!("{notes:?}"));
    let (_, res, _) = configure(root, &args(&["--remove", "biz-rules", "--add", "nope"]));
    let now = ids(get(&res, "documents"));
    check!("configure：既有設定會保留；--remove 可移除；型錄沒有的 id 不加", !now.iter().any(|i| i == "biz-rules") && now.len() == 8 && !now.iter().any(|i| i == "nope"));
    check!("configure：路徑不合理的 dir 拒絕", configure(root, &args(&["--dir", "../outside"])).0 == 2 && configure(root, &args(&["--dir", "/abs"])).0 == 2);

    // ---- 設定檢查
    check!("設定檢查：id 重複、source 寫錯、sources 跳出專案", config_errors(&json!({"documents": [{"id": "a"}, {"id": "a", "source": "magic"}, {"id": "b", "sources": ["../x"]}]})).len() == 3);
    check!("設定檢查：正常設定沒有錯", config_errors(&json!({"dir": "docs/交付", "documents": [{"id": "db-design", "when": "milestone", "depth": "summary"}]})).is_empty());

    // ---- init：只建設定裡列的
    init_docs(root);
    let d_out = pyish::pjoin(root, DEFAULT_DIR);
    let d = DEFAULT_DIR;
    let mut made: Vec<String> = std::fs::read_dir(&d_out).map(|r| r.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect()).unwrap_or_default();
    made.sort();
    let mut want = args(&["README.md", "api-spec.md", "biz-rules.md", "db-design.md", "dev-record.md", "external", "ops-manual.md", "project-plan.md", "rtm.md", "sys-arch.md"]);
    want.sort();
    check!("init：只建交付設定裡列的文件與總目錄", made == want, format!("{made:?}"));
    check!("init：外部文件只建收件目錄", is_dir(&pyish::pjoin(&d_out, "external/vuln-scan-report")) && !exists(root, &format!("{d}/vuln-scan-report.md")));
    let before = rd(root, &format!("{d}/db-design.md"));
    w(root, &format!("{d}/sys-arch.md"), &format!("{}\n手寫的內容\n", rd(root, &format!("{d}/sys-arch.md"))));
    init_docs(root);
    check!("init：重跑不動已有的檔", rd(root, &format!("{d}/sys-arch.md")).contains("手寫的內容") && rd(root, &format!("{d}/db-design.md")) == before);
    let parsed = parse_doc(&before);
    let titles: Vec<&str> = parsed.sections.iter().take(2).map(|s| s.title.as_str()).collect();
    check!("骨架：檔頭齊全、小節帶來源標記", parsed.meta.get("id") == Some(&json!("db-design")) && parsed.meta.get("source") == Some(&json!("auto")) && titles == ["資料表與欄位", "關聯與索引"]
        && parsed.sections.first().is_some_and(|s| s.sources.iter().any(|g| g == "**/migrations/**")), Value::Object(parsed.meta.clone()));

    // ---- status 四種狀態
    let st = status_of(root);
    check!("status：外部文件沒附是「等人補」", st_is(&st, "vuln-scan-report", "waiting"));
    check!("status：人補的文件還沒寫是「等人補」", st_is(&st, "ops-manual", "waiting") && detail(&st, "ops-manual").contains("系統概觀"), detail(&st, "ops-manual"));
    check!("status：自動文件沒有原料是「缺原料」", st_is(&st, "api-spec", "missing") && detail(&st, "api-spec").contains("找不到原料"), detail(&st, "api-spec"));
    check!("status：自動文件有原料還沒產生是「過期」", st_is(&st, "db-design", "stale") && detail(&st, "db-design").contains("還沒產生"), detail(&st, "db-design"));
    w(root, &format!("{d}/external/vuln-scan-report/報告.pdf"), "x");
    for i in ["db-design", "ops-manual", "sys-arch", "biz-rules", "project-plan", "api-spec"] {
        fill(root, i, "已寫好的內容。");
    }
    w(root, "specs/export/design.md", "# design\n\n## 資料設計\n\norders 表。\n");
    w(root, "specs/export/intent.md", "# intent\n\n匯出訂單。\n");
    w(root, "specs/export/spec.md", "# spec\n\n## 匯出\n\n驗收條件一：匯出 CSV。\n");
    w(root, "api/openapi.yaml", "openapi: 3.0.0\n");
    let st = status_of(root);
    check!("status：寫了但沒記錄來源是「過期」（還沒 sync）", st_is(&st, "db-design", "stale") && detail(&st, "db-design").contains("還沒記錄來源"), detail(&st, "db-design"));
    let cfg = load_config(root).0.unwrap_or(Value::Null);
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    let st = status_of(root);
    check!("status：記錄來源後是「齊全」；外部文件附了也是", st_is(&st, "db-design", "ok") && st_is(&st, "vuln-scan-report", "ok") && st_is(&st, "ops-manual", "ok") && done.iter().any(|k| k == "db-design#資料表與欄位"), detail(&st, "db-design"));
    check!("status：四種狀態的文字都有", STATUS_TEXT.iter().map(|x| x.0).collect::<BTreeSet<&str>>() == ["ok", "stale", "missing", "waiting"].into_iter().collect::<BTreeSet<&str>>());

    // ---- 過期判定
    w(root, "db/migrations/002_items.sql", "create table items (id int);\n");
    let st = status_of(root);
    check!("過期：來源多了一個檔、內文沒動 → 過期", st_is(&st, "db-design", "stale") && sec_details(&st, "db-design").contains("002_items.sql"), detail(&st, "db-design"));
    check!("過期：沒用到這個來源的文件不受影響", st_is(&st, "biz-rules", "ok") && st_is(&st, "ops-manual", "ok"));
    let (done, stale) = stamp(root, &cfg, &cat, None, &[], None);
    check!("過期：內文沒動時 sync 不會把它記成最新", !done.iter().any(|k| k.starts_with("db-design#")) && stale.iter().any(|s| s.contains("db-design#資料表與欄位")) && st_is(&status_of(root), "db-design", "stale"), format!("{done:?}"));
    let p = format!("{d}/db-design.md");
    w(root, &p, &rd(root, &p).replace("已寫好的內容。", "已寫好的內容。  \n"));
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    check!("過期：只改空白不算更新", done.is_empty() && st_is(&status_of(root), "db-design", "stale"));
    let text = rd(root, &p);
    let (head, tail) = text.split_once("## 關聯與索引").unwrap_or((&text, ""));
    w(root, &p, &format!("{}## 關聯與索引{tail}", head.replace("已寫好的內容。", "orders 與 items 兩張表。")));
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    let st = status_of(root);
    check!("過期：只有改寫過的小節變成最新，其他小節仍然過期", done == ["db-design#資料表與欄位"] && st_is(&st, "db-design", "stale") && !detail(&st, "db-design").contains("資料表與欄位"), format!("{done:?} {}", detail(&st, "db-design")));
    let (done, _) = stamp(root, &cfg, &cat, None, &args(&["db-design#關聯與索引", "db-design#資料遷移", "db-design#資料保存與清理"]), None);
    check!("過期：人用 --reviewed 表示內容不用改才放行", done.len() == 3 && st_is(&status_of(root), "db-design", "ok"), format!("{done:?}"));
    w(root, "db/migrations/001_orders.sql", "create table orders (id int, total int);\n");
    check!("過期：來源檔內容改了 → 過期", st_is(&status_of(root), "db-design", "stale"));
    let state = load_state(root);
    check!("來源紀錄放在 .claude/teams（team agent 寫不到），文件裡的標記只有路徑", exists(root, &format!("{teams}/{STATE_NAME}")) && !rd(root, &p).contains("sha1")
        && get(get(get(&state, "db-design"), "資料表與欄位"), "sources").as_object().is_some_and(|o| o.len() >= 2));
    stamp(root, &cfg, &cat, None, &reviewed_db(), None);

    // ---- check
    w(root, &format!("{d}/{INDEX_NAME}"), &index_text(&cfg, &cat));
    for i in ["rtm", "dev-record"] {
        fill(root, i, "之後由工具寫入。");
    }
    stamp(root, &cfg, &cat, None, &[], None);
    let errs = checks(root);
    check!("check：都寫好、來源最新、總目錄一致 → 通過", errs.is_empty(), format!("{errs:?}"));
    check!("check CLI：通過 exit 0", flow_run(root, &args(&["check"]), out) == 0);
    let done_text = "已寫好的內容。";
    check!("check：檔頭缺欄位該擋", with_change(root, "sys-arch", "title: 系統架構設計書\n", "").iter().any(|e| e.contains("檔頭缺 title")));
    check!("check：檔頭 id 和檔名不一致該擋", with_change(root, "sys-arch", "id: sys-arch", "id: other").iter().any(|e| e.contains("不一致")));
    w(root, "db/migrations/009.sql", "x\n");
    check!("check：把來源標記刪掉不能躲過過期判定（改用文件層級的來源）", with_change(root, "db-design", "<!-- awf:source", "<!-- 沒了").iter().any(|e| e.contains("db-design.md〈資料表與欄位〉")));
    rm(root, "db/migrations/009.sql");
    check!("check：過期小節該擋", with_change(root, "db-design", "<!-- awf:source", "<!-- awf:source src/**;").iter().any(|e| e.contains("db-design.md〈資料表與欄位〉")));
    let mail = format!("聯絡 someone{}example.com。", "@");
    let got = with_change(root, "sys-arch", done_text, &mail);
    check!("check：隱私字串（信箱）該擋，而且不印內容", got.iter().any(|e| e.contains("電子郵件")) && !got.iter().any(|e| e.contains("someone@")));
    let cases = [("本機路徑", format!("放在 /{} 底下。", "home/demo/app/")), ("憑證或金鑰", format!("金鑰 ghp_{}。", "a".repeat(30))), ("內部 agent 名稱", format!("由 dev-{} 完成。", "implementer")),
        ("內部網路位址或主機", format!("主機 10.{}。", "1.2.3"))];
    check!("check：隱私字串（家目錄路徑、金鑰、內部 agent 名稱）該擋", cases.iter().all(|(kind, text)| with_change(root, "sys-arch", done_text, text).iter().any(|e| e.contains(kind))));
    check!("check：一般內容不誤擋", checks(root).is_empty());
    check!("check：失效連結該擋、有效連結與外部連結該放", with_change(root, "sys-arch", done_text, "見 [資料庫](nope.md)。").iter().any(|e| e.contains("連結對不到檔"))
        && with_change(root, "sys-arch", done_text, "見 [資料庫](db-design.md#x) 與 [官網](https://example.com/a)。").is_empty());
    w(root, "docs/禁用字.txt", "# 內部代號\nProject\\s*Falcon\n[壞掉\n");
    let cfg2 = with_key(&cfg, "banned_terms_file", json!("docs/禁用字.txt"));
    w(root, &cfg_rel, &py::dumps(&cfg2, None));
    let hit = with_change(root, "sys-arch", done_text, "代號 Project Falcon。");
    check!("check：禁用字（專案自己的清單）該擋，而且不印內容", hit.iter().any(|e| e.contains("出現禁用字")) && !hit.iter().any(|e| e.contains("Falcon")), format!("{hit:?}"));
    check!("check：禁用字清單寫壞的那一行略過並說明、其他照常", checks(root).iter().any(|e| e.contains("寫壞的正規式")));
    w(root, "docs/禁用字.txt", "Project\\s*Falcon\n");
    check!("check：沒出現禁用字該放", checks(root).is_empty());
    w(root, &format!("{d}/{INDEX_NAME}"), &format!("{}\n手改\n", index_text(&cfg, &cat)));
    let c1 = checks(root).iter().any(|e| e.contains("總目錄和交付設定不一致"));
    check!("check：總目錄被改過該擋；index 重寫後通過", c1 && flow_run(root, &args(&["index"]), out) == 0 && checks(root).is_empty());
    w(root, &format!("{d}/{INDEX_NAME}"), "x");
    let c1 = flow_run(root, &args(&["check"]), out) == 1;
    check!("check CLI：有問題 exit 1", c1 && flow_run(root, &args(&["index"]), out) == 0);
    w(root, &format!("{d}/不在設定裡.md"), &format!("聯絡 someone{}example.com\n", "@"));
    check!("check：只檢查交付設定裡列的文件", checks(root).is_empty());
    rm(root, &format!("{d}/不在設定裡.md"));
    let cfg_allow = with_key(&cfg2, "privacy_allow", json!(["support@example\\.com"]));
    w(root, &cfg_rel, &py::dumps(&cfg_allow, None));
    check!("check：privacy_allow 列的字串放行，其他照擋", with_change(root, "sys-arch", done_text, &format!("客服 support{}example.com。", "@")).is_empty()
        && with_change(root, "sys-arch", done_text, &format!("聯絡 other{}example.com。", "@")).iter().any(|e| e.contains("電子郵件")));

    // ---- 受影響的文件
    let aff = |paths: &[&str], milestone: bool| affected(root, run, &args(paths), milestone, false);
    let res = aff(&["db/migrations/003_x.sql", "src/app.py"], false);
    let aids = ids(get(&res, "affected"));
    let dbd = pyfmt::items(res.get("affected")).iter().find(|a| sget(a, "id") == "db-design").cloned().unwrap_or(Value::Null);
    let inl = |l: &[String], i: &str| l.iter().any(|x| x == i);
    check!("受影響：diff 有 migration → 只更新資料庫設計文件的對應小節", inl(&aids, "db-design") && get(&dbd, "sections") == &json!(["資料表與欄位", "關聯與索引", "資料遷移"]) && !inl(&aids, "api-spec") && !inl(&aids, "sys-arch"), format!("{aids:?}"));
    w(root, &format!("{run_rel}/run.json"), &py::dumps(&with_key(&load_run(), "recipe", json!({"name": "feature", "modifiers": ["public_api"], "resolved": {"name": "feature", "modifiers": ["public_api"], "stages": ["plan", "design", "build", "test", "deliver"]}})), None));
    let res = aff(&["src/app.py"], false);
    let aids = ids(get(&res, "affected"));
    check!("受影響：有 public_api → 更新 API 規格書，資料庫設計文件不動", inl(&aids, "api-spec") && !inl(&aids, "db-design"), format!("{aids:?}"));
    check!("受影響：業務規則文件在有 plan 段的 run 都更新（intent 可能改了）", inl(&aids, "biz-rules"));
    check!("受影響：追溯矩陣、開發紀錄、外部文件不派給 agent", !["rtm", "dev-record", "vuln-scan-report"].iter().any(|i| inl(&aids, i)));
    check!("受影響：when 是里程碑的文件在一般 run 不更新，列在 deferred", !inl(&aids, "project-plan") && !inl(&aids, "ops-manual"));
    let mut more = pyfmt::items(cfg2.get("documents")).to_vec();
    more.push(json!({"id": "integration-spec"}));
    w(root, &cfg_rel, &py::dumps(&with_key(&cfg2, "documents", Value::Array(more)), None));
    let res = aff(&["src/app.py"], false);
    let res_m = aff(&["src/app.py"], true);
    check!("受影響：里程碑文件平常列 deferred，--milestone 才更新", ids(get(&res, "deferred")) == ["integration-spec"] && inl(&ids(get(&res_m, "affected")), "integration-spec"), get(&res, "deferred"));
    w(root, &cfg_rel, &py::dumps(&cfg2, None));
    let res = aff(&["jobs/nightly.py", "src/app.py"], false);
    check!("受影響：碰到新領域（第一次出現排程）只提示、不自動加", ids(get(&res, "hints")) == ["batch-design"] && !inl(&ids(get(&res, "affected")), "batch-design")
        && !inl(&ids(get(&load_config(root).0.unwrap_or(Value::Null), "documents")), "batch-design"), get(&res, "hints"));
    let res = aff(&["README.md"], false);
    let aids = ids(get(&res, "affected"));
    check!("受影響：只改文件的 run 不動設計文件", !["db-design", "api-spec", "sys-arch"].iter().any(|i| inl(&aids, i)) || inl(&aids, "api-spec"));
    let (extra, _) = pack_args("design", run, root, None, &load_run(), None);
    let ds: BTreeSet<(String, String)> = pyfmt::items(extra.get("design_sections")).iter().map(|x| (sget(x, "id").to_string(), get(x, "sections").to_string())).collect();
    let ds_want: BTreeSet<(String, String)> = [("sys-arch", json!(["系統設計"])), ("db-design", json!(["資料設計"])), ("api-spec", json!(["API 設計"]))].iter().map(|(i, s)| (i.to_string(), s.to_string())).collect();
    check!("pack_args（design）：列出 design.md 要有的小節", ds == ds_want, extra);
    let (extra, notes) = pack_args("deliver", run, root, None, &load_run(), Some(&args(&["jobs/nightly.py", "api/openapi.yaml"])));
    check!("pack_args（deliver）：受影響的文件、交付摘要路徑、新領域提示", ids(get(&extra, "affected")) == ["biz-rules", "api-spec"] && sget(&extra, "summary").ends_with(&format!("/deliver/{SUMMARY_NAME}"))
        && ids(get(&extra, "hints")).first().is_some_and(|h| h == "batch-design") && notes.iter().any(|n| n.contains("建議把「批次作業設計文件」加進交付設定")), extra);
    let (extra2, _) = pack_args("deliver", run, root, None, &load_run(), Some(&args(&["README.md"])));
    rm(root, &format!("{d}/api-spec.md"));
    pack_args("deliver", run, root, None, &load_run(), Some(&args(&["api/openapi.yaml"])));
    check!("pack_args（deliver）：有交付設定就帶交付摘要路徑；受影響的文件還沒建立時先放骨架", sget(&extra2, "summary").ends_with(SUMMARY_NAME) && exists(root, &format!("{d}/api-spec.md")));
    fill(root, "api-spec", done_text);
    stamp(root, &cfg, &cat, None, &[], None);
    check!("pack_args：build 等其他段不帶", empty_pack(pack_args("build", run, root, None, &json!({}), None)));
    let a1 = py::dumps_sorted(&aff(&["jobs/nightly.py"], false), None);
    check!("受影響：結果可重現", a1 == py::dumps_sorted(&aff(&["jobs/nightly.py"], false), None));

    // ---- 需求追溯矩陣（條件的 id 用小寫，檢查的是對應關係）
    w(root, &format!("{run_rel}/design.json"), &py::dumps(&json!({"spec": {"payload": {"subtasks": [
        {"id": "t1", "title": "匯出", "acceptance": ["ac-1 匯出 CSV", "ac-2"], "spec_refs": ["spec.md#匯出"], "story_ids": ["s-12"]},
        {"id": "t2", "title": "排程", "acceptance": ["ac-3"], "spec_refs": []}]}}}), None));
    w(root, &format!("{run_rel}/test.json"), &py::dumps(&json!({"stage": "test", "verifier": {"status": "ok", "payload": {"commit": "0123456789abcdef0123", "scope_covered": "全部", "scope_skipped": "none", "criteria": [
        {"id": "ac-1", "criterion": "匯出 CSV", "command": "pytest tests/test_export.py", "result": "pass", "evidence": "3 passed"},
        {"id": "s1:ac-2", "criterion": "空資料時回空檔", "command": "pytest -k empty", "result": "pass", "evidence": "1 passed"},
        {"id": "ac-3", "criterion": "每天 02:00 執行", "command": "pytest -k cron", "result": "fail", "evidence": "1 failed"},
        {"id": "ac-9", "criterion": "沒人認領的條件", "command": "", "result": "not_verifiable", "evidence": ""}]}}}), None));
    let (rows, gaps) = matrix_rows(run);
    let row = |id: &str| rows.iter().find(|r| sget(r, "id") == id).cloned().unwrap_or(Value::Null);
    let r1 = row("ac-1");
    check!("追溯矩陣：每條 AC 對到 spec 小節、驗證指令、證據、commit", get(&r1, "spec") == &json!(["spec.md#匯出"]) && get(&r1, "stories") == &json!(["s-12"]) && !sget(&r1, "command").is_empty()
        && sget(&r1, "evidence") == "3 passed" && sget(&r1, "commit").starts_with("0123456789ab") && get(&r1, "gaps") == &json!([]), r1);
    let r2 = row("s1:ac-2");
    check!("追溯矩陣：分片前綴的 id 也對得到", get(&r2, "spec") == &json!(["spec.md#匯出"]) && get(&r2, "gaps") == &json!([]));
    check!("追溯矩陣：缺一環時標出來（沒有 spec 小節、沒通過、沒有指令與證據）", gaps.iter().any(|g| g.starts_with("ac-3：") && g.contains("缺spec 小節") && g.contains("結果是 fail"))
        && gaps.iter().any(|g| g.starts_with("ac-9：") && g.contains("缺驗證指令") && g.contains("缺證據")) && gaps.len() == 2, format!("{gaps:?}"));
    let (code, notes) = write_matrix(root, run);
    let rtm = rd(root, &format!("{d}/rtm.md"));
    check!("追溯矩陣：寫進文件，缺的環節印出來", code == 0 && rtm.contains("| ac-1 | 匯出 CSV | s-12 | spec.md#匯出 | `pytest test_export.py` | 通過 | 3 passed | 0123456789ab | 無 |") && notes.iter().any(|n| n.contains("[缺] ac-3")), rtm);
    write_matrix(root, run);
    check!("追溯矩陣：重跑結果相同（同一個 run 只有一段）", rd(root, &format!("{d}/rtm.md")) == rtm && rtm.matches("<!-- awf:entry ").count() == 1);
    let after = rtm.split("## 追溯矩陣").nth(1).unwrap_or("").to_string();
    let no_hash = regex::Regex::new("[0-9a-f]{12}").map(|r| r.replace_all(&after, "").into_owned()).unwrap_or(after);
    check!("追溯矩陣：不寫 run 目錄名稱（用雜湊當標記）", !no_hash.contains("r1"));
    w(root, "_tpm/runs/r0/run.json", "{}");
    let empty = pyish::pjoin(root, "_tpm/runs/r0");
    check!("追溯矩陣：沒有驗證結果時說明、不當掉", matrix_rows(&empty).1.first().is_some_and(|g| g.contains("沒有驗證結果")) && matrix_block(&empty, "x").0.contains("沒有驗證結果"));

    // ---- sync
    let sum_rel = format!("{run_rel}/deliver/{SUMMARY_NAME}");
    let summary = "# 交付摘要\n\n## 這次交付了什麼\n\n訂單可以匯出成 CSV。\n";
    w(root, &sum_rel, summary);
    let (code, _) = sync(root, run, None, &[], None);
    let rec = rd(root, &format!("{d}/dev-record.md"));
    check!("sync：交付摘要與核准紀錄併進功能開發紀錄", code == 0 && rec.contains("訂單可以匯出成 CSV。") && rec.contains("| 規劃 | 人工核准 | 2026-01-02 |") && !rec.contains("ACME 的 PM") && rec.contains("| 驗證 | 依政策自動核准 | 2026-01-03 |"), rec);
    let snap = md_snapshot(&d_out);
    let state1 = rd(root, &format!("{teams}/{STATE_NAME}"));
    let (code, _) = sync(root, run, None, &[], None);
    check!("sync：重跑結果相同", code == 0 && snap == md_snapshot(&d_out) && state1 == rd(root, &format!("{teams}/{STATE_NAME}")));
    check!("sync：交付摘要的標題降階，不打亂文件結構", rec.contains("##### 這次交付了什麼") && parse_doc(&rec).sections.iter().map(|s| s.title.clone()).collect::<Vec<_>>() == ["開發紀錄"]);
    w(root, &sum_rel, &format!("# 交付摘要\n\n有問題請寄 someone{}example.org。\n", "@"));
    let (code, notes) = sync(root, run, None, &[], None);
    let rec_snap = snap.iter().find(|x| x.0 == "dev-record.md").map(|x| x.1.clone()).unwrap_or_default();
    check!("sync：寫入前掃隱私字串，命中就停、不寫", code == 1 && notes.first().is_some_and(|n| n.contains("隱私字串")) && rd(root, &format!("{d}/dev-record.md")) == rec_snap, format!("{notes:?}"));
    w(root, &sum_rel, summary);
    rm(root, &sum_rel);
    let (code, notes) = sync(root, run, None, &[], None);
    check!("sync：沒有交付摘要時說明，仍記核准紀錄", code == 0 && notes.iter().any(|n| n.contains("沒有交付摘要")));
    let errs = checks(root);
    check!("sync 之後 check 通過", errs.is_empty(), format!("{errs:?}"));

    // ---- 審查（C9.3）找到的繞過手法
    let cur_state = |id: &str| status_of(root).get(id).map(|x| sget(x, "status").to_string()).unwrap_or_default();
    w(root, "db/migrations/010_audit.sql", "create table audit (id int);\n");
    check!("審查：來源改了 → 過期（前提）", cur_state("db-design") == "stale");
    let keep = rd(root, &p);
    w(root, &p, &keep.replacen("<!-- awf:source specs/**/design.md; **/migrations/**; **/migration/**; **/*.sql; **/schema.prisma -->", "<!-- awf:source nothing/here -->", 1)
        .replacen("orders 與 items 兩張表。", "orders 與 items 兩張表，另有稽核表。", 1));
    stamp(root, &cfg, &cat, None, &[], None);
    w(root, "db/migrations/011_more.sql", "create table more (id int);\n");
    let st = status_of(root);
    check!("審查：把來源標記改成對不到檔的 glob，不能讓文件層級的來源失效", st_is(&st, "db-design", "stale") && format!("{}{}", detail(&st, "db-design"), sec_details(&st, "db-design")).contains("011_more.sql"), detail(&st, "db-design"));
    w(root, &p, &keep);
    stamp(root, &cfg, &cat, None, &reviewed_db(), None);
    w(root, "db/migrations/012.sql", "x\n");
    w(root, &p, &keep.replacen("orders 與 items 兩張表。", "orders 與 items 兩張表 。", 1));
    let none_db = !stamp(root, &cfg, &cat, None, &[], None).0.iter().any(|k| k.starts_with("db-design#"));
    check!("審查：只插入空白或改標點不算改寫", none_db && cur_state("db-design") == "stale");
    w(root, &p, &keep.replacen("orders 與 items 兩張表。", "orders、items 與稽核表。", 1).replacen("已寫好的內容。", "改了清單外的小節。", 1));
    let only: BTreeSet<String> = ["db-design#資料表與欄位".to_string()].into_iter().collect();
    let (done, stale) = stamp(root, &cfg, &cat, None, &[], Some(&only));
    check!("審查：自動同步只記受影響清單上的小節，清單外的改動列出來交人", done == ["db-design#資料表與欄位"] && stale.iter().any(|x| x.contains("清單外改動") && x.contains("db-design#關聯與索引")), format!("{done:?} {stale:?}"));
    w(root, &p, &keep);
    stamp(root, &cfg, &cat, None, &reviewed_db(), None);
    check!("審查：.github 開頭的 glob 對得到", match_files(&args(&[".github/workflows/ci.yml", "x/.github/a"]), &[json!(".github/workflows/**")]) == [".github/workflows/ci.yml"]);
    let pwn = std::fs::read_dir(root).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().starts_with("pwn"))).unwrap_or(false);
    check!("審查：base 不能變成 git 的選項", changed_paths(root, &format!("--output={}", pyish::pjoin(root, "pwn"))).is_empty() && !pwn);
    let broken = "## 開發紀錄\n\n<!-- awf:entry abcdef123456 -->\n舊內容\n\n## 重要\n\n人寫的內容\n";
    check!("審查：找不到結尾標記時不寫入（不會吃掉後面的內容）", upsert_entry(broken, "abcdef123456", "新內容", "開發紀錄").is_err());
    let at = "@";
    let leak: Vec<(&str, String)> = vec![
        ("全形＠", "聯絡 a＠example.com".to_string()), ("零寬字元", format!("a\u{200b}{at}example.com")), ("HTML 實體", "a&#64;example.com".to_string()),
        ("小寫 Windows 路徑", format!("c:\\{}\\demo\\x", "users")), ("JSON 跳脫的 Windows 路徑", format!("C:\\\\{}\\\\demo\\\\x", "Users")), ("家目錄縮寫", format!("~{}", "/work/app")),
        ("macOS 暫存目錄", format!("/var{}", "/folders/xy/T/awf-logs/a.log")), ("大寫 agent 名稱", format!("{}-Writer 寫的", "TPM")), ("底線 agent 名稱", format!("tpm_{}", "writer")),
        ("全形連字號", format!("tpm－{}", "writer")), ("localhost", format!("http://{}:8080", "localhost")), ("內網主機", format!("http://wiki.corp.{}/x", "internal")),
        ("github_pat", format!("github_pat_{}", "A".repeat(30))), ("JWT", format!("eyJ{0}.eyJ{0}.{0}k", "abcdefghij")), ("Bearer", format!("Bearer {}", "a".repeat(24))),
        ("引號裡的密碼", format!("\"password\": \"{}\"", "hunter2hunter")), ("中文密碼", format!("密碼：{}", "hunter2hunter")), ("跨行信箱", format!("a{at}exam\nple.com")),
    ];
    let missed: Vec<&str> = leak.iter().filter(|(_, v)| privacy_hits(v, &[], &[]).is_empty()).map(|(k, _)| *k).collect();
    check!("審查：隱私規則抓得到各種變形", missed.is_empty(), format!("漏掉：{}", missed.join("、")));
    check!("審查：隱私規則不誤擋一般內容", privacy_hits("訂單匯出 CSV，版本 1.2.3，見 docs/guide.md 與 https://example.com/a。", &[], &[]).is_empty());
    w(root, "_tpm/runs/2026-11-13-export/run.json", "{}");
    let hit = with_change(root, "sys-arch", done_text, "見 2026-11-13-export 的紀錄。");
    check!("審查：run 目錄名稱不能出現在交付文件", hit.iter().any(|e| e.contains("內部 run 目錄")), format!("{hit:?}"));
    let _ = std::fs::remove_dir_all(pyish::pjoin(root, "_tpm/runs/2026-11-13-export"));
    w(root, &cfg_rel, &py::dumps(&with_key(&cfg_allow, "show_approvers", json!(true)), None));
    check!("審查：show_approvers 開啟才寫核准者名字", record_block(&load_run(), "", true).contains(pm) && !record_block(&load_run(), "", false).contains("ACME"));
    check!("審查：追溯矩陣的證據與指令只留檔名", strip_paths(&json!(format!("見 /var{} 與 _tpm/runs/r9/test/verify.md", "/folders/xy/T/a.log"))) == "見 a.log 與 verify.md");
    w(root, "repos/app/db/migrations/001.sql", "x\n");
    w(root, &format!("{run_rel}/run.json"), &py::dumps(&with_key(&load_run(), "target_repo", json!("repos/app")), None));
    check!("審查：check 帶 --run 時用 run 的目標 repo", repo_of(root, &args(&["--run", run]), Some(run)) == Some(pyish::pjoin(root, "repos/app")));
    w(root, &format!("{run_rel}/run.json"), &py::dumps(&with_key(&load_run(), "target_repo", json!(".")), None));

    // ---- export
    let (code, notes) = export(root, "", Some(None));
    check!("export：沒有 pandoc 時說明怎麼裝並 exit 0", code == 0 && notes.first().is_some_and(|n| n.contains("找不到 pandoc") && n.contains("pandoc.org")));
    let launcher = pyish::pjoin(root, if cfg!(windows) { "pandoc.cmd" } else { "pandoc.sh" });
    if cfg!(windows) {
        write_lf(&launcher, "@echo off\r\n:loop\r\nif \"%~1\"==\"\" goto end\r\nif \"%~1\"==\"-o\" echo docx> \"%~2\"\r\nshift\r\ngoto loop\r\n:end\r\n");
    } else {
        write_lf(&launcher, "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then printf docx > \"$2\"; fi\n  shift\ndone\n");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755));
    }
    let (code, notes) = export(root, &pyish::pjoin(root, "out"), Some(Some(launcher)));
    let mut got: Vec<String> = std::fs::read_dir(pyish::pjoin(root, "out")).map(|r| r.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| n.ends_with(".docx")).collect()).unwrap_or_default();
    got.sort();
    check!("export：有 pandoc 時每份文件轉一個 docx（外部文件不轉）", code == 0 && got.iter().any(|g| g == "資料庫設計文件.docx") && got.len() == 8, format!("{got:?} {:?}", notes.iter().take(2).collect::<Vec<_>>()));

    // ---- CLI
    let c1 = flow_run(root, &args(&["affected"]), out) == 2;
    check!("CLI：缺 --run 回 2", c1 && flow_run(root, &args(&["sync"]), out) == 2);
    check!("CLI：不認得的指令回 2", flow_run(root, &args(&["nope"]), out) == 2);
    check!("CLI：status exit 0", flow_run(root, &args(&["status"]), out) == 0);

    let _ = std::fs::remove_dir_all(&td);
    let bad = results.iter().filter(|r| !r.0).count();
    for (ok, name, detail) in &results {
        let tail = if *ok || detail.is_empty() { String::new() } else { format!("：{}", pyish::take(detail, 600)) };
        out.push_str(&format!("  [{}] {name}{tail}\n", if *ok { "通過" } else { "失敗" }));
    }
    out.push_str(&format!("deliverables selftest：{}（{} 項）\n", if bad == 0 { "全部通過".to_string() } else { format!("{bad} 項失敗") }, results.len()));
    if bad == 0 { 0 } else { 1 }
}
