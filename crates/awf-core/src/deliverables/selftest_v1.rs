//! `awf deliverables --selftest` 的 1.0 案例：專案層的追溯矩陣、小節因為連到的需求或符號變了而過期、結案摘要的〈連結〉一節。
//!
//! 和原本的檢查項分開（原本那一段的輸出是對跑基準的一部分，項數與文字不動）；只有命令列的 `--selftest` 會接著跑這一段。
//! 專案用 `awf kg --selftest` 的那一個（原始檔、OpenSpec 目錄、由 `save_run` 寫出來的連結帳本）。

use super::core::*;
use super::*;
use crate::kg;
use serde_json::{json, Value};

const DOC: &str = "---\nid: guide\ntitle: 使用手冊\ngroup: custom\nsource: assisted\ndepth: full\n---\n\n# 使用手冊\n\n## 匯出\n<!-- awf:source src/billing/format.ts -->\n\n匯出對應需求 `billing/export-invoice`，程式在 `src/billing/export.ts`。\n\n## 其他\n<!-- awf:source src/billing/format.ts -->\n\n這一節沒有連到任何需求。\n";
const RUN_REL: &str = "_tpm/runs/2026-01-05-export";

pub fn selftest(out: &mut String) -> i32 {
    let mut ok_all = true;
    let mut n = 0usize;
    out.push_str("deliverables selftest（1.0：知識圖）\n");
    macro_rules! check {
        ($name:expr, $cond:expr) => {
            check!($name, $cond, String::new())
        };
        ($name:expr, $cond:expr, $detail:expr) => {{
            let cond: bool = $cond;
            n += 1;
            let tail = if cond { String::new() } else { format!("：{}", $detail) };
            out.push_str(&format!("  [{}] {}{tail}\n", if cond { "通過" } else { "失敗" }, $name));
            ok_all = ok_all && cond;
        }};
    }
    let tmp = std::env::temp_dir().join(format!("awf-deliverables-v1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);
    let tmp = tmp.canonicalize().unwrap_or(tmp);
    let root_s = tmp.to_string_lossy().replace('\\', "/");
    let root = root_s.as_str();
    let finish = |out: &mut String, ok: bool, n: usize| {
        let _ = std::fs::remove_dir_all(&tmp);
        out.push_str(&format!("deliverables selftest（1.0）：{}（{n} 項）\n", if ok { "全部通過" } else { "有失敗" }));
        if ok { 0 } else { 1 }
    };
    if let Err(e) = kg::selftest_project(&tmp) {
        out.push_str(&format!("  [失敗] 造不出測試專案：{e}\n"));
        return finish(out, false, n);
    }
    let w = |rel: &str, text: &str| {
        let p = tmp.join(rel);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, text);
    };
    let rd = |rel: &str| std::fs::read_to_string(tmp.join(rel)).unwrap_or_default();
    w(&format!(".claude/teams/{CONFIG_NAME}"), &json!({"dir": "docs", "documents": [{"id": "guide", "title": "使用手冊", "source": "assisted", "sections": ["匯出", "其他"], "sources": ["src/billing/format.ts"]}]}).to_string());
    w("docs/guide.md", DOC);
    let state_rel = format!(".claude/teams/{STATE_NAME}");
    let cfg = load_config(root).0.unwrap_or_else(|| json!({}));
    let cat = load_catalog(root);
    let build = || kg::build(&kg::BuildOpts { root: Some(root.to_string()), ..Default::default() }).map(|_| ()).unwrap_or_else(|e| eprintln!("建圖失敗：{e}"));
    let status = || -> (String, Vec<(String, String, String)>) {
        let docs = status_all(root, None).0.unwrap_or_default();
        let d = docs.iter().find(|d| sget(d, "id") == "guide").cloned().unwrap_or_else(|| json!({}));
        (sget(&d, "status").to_string(), crate::pyfmt::items(d.get("sections")).iter().map(|s| (sget(s, "title").to_string(), sget(s, "state").to_string(), sget(s, "detail").to_string())).collect())
    };
    let sec = |all: &[(String, String, String)], title: &str| all.iter().find(|s| s.0 == title).map(|s| (s.1.clone(), s.2.clone())).unwrap_or_default();
    let cli = |args: &[&str]| -> (i32, String) {
        let mut o = String::new();
        let code = run(root, &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &mut o);
        (code, o)
    };

    // ---- 沒有圖
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    let state0 = rd(&state_rel);
    check!("沒有圖：sync 照舊記錄來源，紀錄裡沒有連結", done.len() == 2 && !state0.contains("\"links\"") && status().0 == "ok", format!("{done:?} {state0}"));
    let (code, text) = cli(&["matrix", "--project"]);
    check!("沒有圖：專案層的矩陣說明原因並回非零", code == 1 && text.contains("awf kg build") && text.contains("--run"), format!("{code} {text}"));
    let (code, text) = cli(&["matrix"]);
    check!("單一 run 的矩陣照舊要 --run", code == 2 && text.contains("缺 --run"), text);

    // ---- 有圖：專案層的矩陣
    build();
    let (code, text) = cli(&["matrix", "--project"]);
    let row = |id: &str| text.lines().find(|l| l.starts_with(&format!("| {id}（"))).unwrap_or_default().to_string();
    check!("專案矩陣：現行需求一列，帶規格位置、檔案、測試、commit", code == 0 && row("billing/export-invoice").contains("openspec/specs/billing/spec.md:") && row("billing/export-invoice").contains("src/billing/export.ts")
        && row("billing/export-invoice").contains("src/billing/export.test.ts") && row("billing/export-invoice").contains("111111111111"), text);
    check!("專案矩陣：沒有東西支撐的需求照樣列出並計數", row("billing/archive-invoice").contains("| — | — | — |") && text.contains("現行需求 2 個，其中 1 個沒有任何檔案、測試或 commit 連到"), text);
    check!("專案矩陣：只有位置，不帶程式內文與本機路徑", !text.contains("BODY_TEXT_NOT_IN_OUTPUT") && !text.contains(root), "");
    let (code, js) = cli(&["matrix", "--project", "--json"]);
    let v: Value = serde_json::from_str(&js).unwrap_or(Value::Null);
    check!("專案矩陣：--json", code == 0 && crate::pyfmt::items(v.get("requirements")).len() == 2 && v.get("unsupported") == Some(&json!(1)), js);
    check!("專案矩陣：重跑結果相同", cli(&["matrix", "--project"]).1 == text, "");

    // ---- 有圖：小節連到的需求或符號變了
    check!("有圖但紀錄裡沒有連結：狀態不變", status().0 == "ok" && rd(&state_rel) == state0, format!("{:?}", status()));
    let _ = std::fs::remove_file(tmp.join(&state_rel));
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    let state: Value = serde_json::from_str(&rd(&state_rel)).unwrap_or(Value::Null);
    let links = state.get("guide").and_then(|g| g.get("匯出")).and_then(|s| s.get("links")).cloned().unwrap_or(Value::Null);
    check!("sync 記下小節連到的需求與程式檔的指紋", done.len() == 2 && links.get("req:billing/export-invoice").is_some_and(Value::is_string) && links.get("file:src/billing/export.ts").is_some_and(Value::is_string), state.to_string());
    check!("沒有連到東西的小節不記連結；文件檔不記", state.get("guide").and_then(|g| g.get("其他")).is_some_and(|s| s.get("links").is_none()) && links.as_object().is_some_and(|o| o.keys().all(|k| !k.ends_with(".md"))), state.to_string());
    check!("剛記完：沒有過期", status().0 == "ok", format!("{:?}", status()));
    // 需求的文字改了
    let spec_rel = "openspec/specs/billing/spec.md";
    let spec_old = rd(spec_rel);
    w(spec_rel, &spec_old.replace("The system SHALL export invoices.", "The system SHALL export invoices as CSV and PDF."));
    check!("圖還沒重建：看不到變動（圖是上次建的樣子）", status().0 == "ok", "");
    build();
    let (st, secs) = status();
    check!("連到的需求改了、內文沒動 → 那一節過期", st == "stale" && sec(&secs, "匯出").0 == "stale" && sec(&secs, "匯出").1.contains("連到的需求或符號改了") && sec(&secs, "匯出").1.contains("req:billing/export-invoice"), format!("{st} {secs:?}"));
    check!("沒有連到它的小節不受影響", sec(&secs, "其他").0 == "fresh", format!("{secs:?}"));
    let errs = check_all(root, None).0.unwrap_or_default();
    check!("check 也列出來", errs.iter().any(|e| e.contains("匯出") && e.contains("連到的需求或符號改了")), format!("{errs:?}"));
    let (done, stale) = stamp(root, &cfg, &cat, None, &[], None);
    check!("內文沒動時 sync 不會把它記成最新", done.is_empty() && stale.iter().any(|s| s.starts_with("guide#匯出")) && status().0 == "stale", format!("{done:?} {stale:?}"));
    let (done, _) = stamp(root, &cfg, &cat, None, &["guide#匯出".to_string()], None);
    check!("人用 --reviewed 表示看過了才放行，並重新記下指紋", done == ["guide#匯出"] && status().0 == "ok", format!("{done:?} {:?}", status()));
    // 程式檔改了
    w("src/billing/export.ts", &rd("src/billing/export.ts").replace("join(marker)", "join(marker + marker)"));
    build();
    let (st, secs) = status();
    check!("連到的程式檔改了 → 那一節過期", st == "stale" && sec(&secs, "匯出").1.contains("file:src/billing/export.ts") && sec(&secs, "其他").0 == "fresh", format!("{st} {secs:?}"));
    // 改寫內文：照原本的規則變成「還沒記錄」，sync 之後記新的指紋
    w("docs/guide.md", &DOC.replace("程式在", "實作在"));
    build();
    let (done, _) = stamp(root, &cfg, &cat, None, &[], None);
    check!("改寫內文後 sync：記成最新", done == ["guide#匯出"] && status().0 == "ok", format!("{done:?} {:?}", status()));
    // 只會更嚴：原本就過期的說明不變
    w("src/billing/format.ts", &rd("src/billing/format.ts").replace("String(n)", "String(n + 0)"));
    let (_, secs) = status();
    check!("原本就判過期的小節維持原本的說明（只會更嚴）", sec(&secs, "其他").0 == "stale" && sec(&secs, "其他").1.contains("來源改了") && sec(&secs, "匯出").1.contains("來源改了"), format!("{secs:?}"));
    // 圖拿掉：回到只看來源
    let lay = kg::layout(Some(root), None, None, None);
    if let Ok(lay) = &lay {
        let _ = std::fs::remove_file(&lay.db);
    }
    let (_, secs) = status();
    check!("圖拿掉之後：只看來源（紀錄裡多的連結不影響）", sec(&secs, "匯出").0 == "stale" && sec(&secs, "匯出").1.contains("來源改了"), format!("{secs:?}"));

    // ---- 結案摘要的〈連結〉一節
    let ctx = crate::save_run::Ctx::new(root, root);
    let data: Value = serde_json::from_str(&rd(&format!("{RUN_REL}/run.json"))).unwrap_or_else(|_| json!({}));
    let with = crate::save_run::archive_text(&ctx, RUN_REL, &data).unwrap_or_else(|e| format!("例外：{}", e.msg));
    check!("結案摘要：有帳本時多〈連結〉一節，照種類列出這個 run 的連結", with.contains("\n## 連結\n") && with.contains("- 驗收條件 → 檔案（") && with.contains("src/billing/export.ts") && with.contains("- 需求 ↔ run（2 條）") && with.contains("這次改動 → 交付文件小節"), with);
    check!("結案摘要：重存後作廢的連結不列、別的 run 的不列", !with.contains("src/billing/old.ts") && !with.contains("src/lib/money.ts"), with);
    check!("結案摘要：重跑結果相同", crate::save_run::archive_text(&ctx, RUN_REL, &data).ok().as_deref() == Some(with.as_str()), "");
    let ledger = tmp.join("_tpm/runs/links.jsonl");
    let keep = std::fs::read(&ledger).unwrap_or_default();
    let _ = std::fs::remove_file(&ledger);
    let without = crate::save_run::archive_text(&ctx, RUN_REL, &data).unwrap_or_else(|e| format!("例外：{}", e.msg));
    let cut = match (with.find("\n## 連結\n"), with.find("\n## 成本\n")) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &with[..a], &with[b..]),
        _ => String::new(),
    };
    check!("結案摘要：沒有帳本時沒有這一節，其餘逐字相同", !without.contains("## 連結") && without == cut, without);
    let _ = std::fs::write(&ledger, keep);

    // ---- 1.1：交付摘要的節與讀者（設定的 summary_sections、audience）：合規則才帶進資料包，否則範本用內建的
    let ss = |v: Value| super::flow::summary_sections(&json!({"summary_sections": v}));
    check!("交付摘要的節：合規則的清單原樣帶", ss(json!(["交付內容", "驗證方式"])) == Some(json!(["交付內容", "驗證方式"])));
    check!(
        "交付摘要的節：沒寫、空的、重複、太多、太長、有換行或反引號、前後有空白、不是字串 → 不帶（範本用內建的三節）",
        super::flow::summary_sections(&json!({})).is_none()
            && ss(json!([])).is_none()
            && ss(json!(["甲", "甲"])).is_none()
            && ss(json!(["1", "2", "3", "4", "5", "6", "7", "8", "9"])).is_none()
            && ss(json!(["字".repeat(41)])).is_none()
            && ss(json!(["甲\n乙"])).is_none()
            && ss(json!(["甲`乙"])).is_none()
            && ss(json!([" 甲"])).is_none()
            && ss(json!(["甲", 7])).is_none()
            && ss(json!("甲")).is_none()
    );
    let au = |v: Value| super::flow::audience(&json!({"audience": v}));
    check!(
        "讀者：1–20 字的一行才帶；沒寫或不合規則不帶（範本用「客戶」）",
        au(json!("內部使用單位")).as_deref() == Some("內部使用單位") && super::flow::audience(&json!({})).is_none() && au(json!("")).is_none() && au(json!("字".repeat(21))).is_none() && au(json!("甲\n乙")).is_none() && au(json!(7)).is_none()
    );
    check!(
        "設定檢查：summary_sections 重複會報錯",
        crate::schema::deliverables_errors(Some(&json!({"documents": [], "summary_sections": ["甲", "甲"]})), "deliverables").iter().any(|e| e.contains("summary_sections")) && crate::schema::deliverables_errors(Some(&json!({"documents": [], "summary_sections": ["甲", "乙"]})), "deliverables").is_empty()
    );
    finish(out, ok_all, n)
}
