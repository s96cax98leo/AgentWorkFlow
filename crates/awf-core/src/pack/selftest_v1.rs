//! `awf pack --selftest` 的 1.0 案例：子任務資料包認規格層的穩定 id、資料包帶知識圖的結果。
//!
//! 和原本的檢查項分開（原本那一段的輸出是對跑基準的一部分，項數與文字不動）；只有命令列的 `--selftest` 會接著跑這一段。
//! 專案用 `awf kg --selftest` 的那一個（原始檔、OpenSpec 目錄、由 `save_run` 寫出來的連結帳本）。

use super::*;
use crate::kg;

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn slurp(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

const RUN_REL: &str = "_tpm/runs/2026-01-05-export";
/// 三個識別字各命中一個檔的一個符號（export.ts、format.ts、money.ts）。
const REQ_TEXT: &str = "示範客戶 ACME：exportInvoice 匯出 CSV 時用 formatRow 和 toCents 加上表頭";

/// 四段各跑一次，回傳每個產出檔的內容（照固定順序）。
fn run_stages(root: &str, work: &str, extra: &[&str]) -> Vec<(String, String)> {
    let run_dir = join(work, "run");
    let pf = join(work, "paths.txt");
    let subs = join(work, "subs.json");
    let mut sink = String::new();
    for stage in ["plan", "design", "build", "deliver"] {
        let mut cli = argv(&[stage, "--run", &run_dir, "--repo", root, "--requirement", REQ_TEXT, "--no-kb", "--json", "--today", "2026-01-10"]);
        match stage {
            "build" => cli.extend(argv(&["--subtasks", &subs])),
            "deliver" => cli.extend(argv(&["--paths-file", &pf])),
            _ => {}
        }
        cli.extend(argv(extra));
        let mut err = String::new();
        let _ = run(root, &cli, &mut sink, &mut err);
    }
    let files = ["plan/pack.md", "plan/pack-args.json", "design/pack.md", "design/pack-args.json", "build/pack.md", "build/pack-args.json", "build/t1/pack.md", "deliver/pack.md", "deliver/pack-args.json"];
    files.iter().map(|f| (f.to_string(), slurp(&join(&run_dir, f)))).collect()
}

pub fn selftest(out: &mut String) -> i32 {
    let mut ok_all = true;
    let mut n = 0usize;
    out.push_str("pack selftest（1.0：規格層的 id、知識圖）\n");
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
    let tmp = std::env::temp_dir().join(format!("awf-pack-v1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);
    let tmp = tmp.canonicalize().unwrap_or(tmp);
    // 專案與「這次的 run、輸入檔」分開放：run 目錄在專案裡的話，工具定位會掃到上一次的資料包，結果就不是只由輸入決定
    let work_s = tmp.join("work").to_string_lossy().replace('\\', "/");
    let work = work_s.as_str();
    let tmp_all = tmp.clone();
    let tmp = tmp.join("proj");
    let _ = std::fs::create_dir_all(&tmp);
    let root_s = tmp.to_string_lossy().replace('\\', "/");
    let root = root_s.as_str();
    let finish = |out: &mut String, ok: bool, n: usize| {
        let _ = std::fs::remove_dir_all(&tmp_all);
        out.push_str(&format!("pack selftest（1.0）：{}（{n} 項）\n", if ok { "全部通過" } else { "有失敗" }));
        if ok { 0 } else { 1 }
    };
    if let Err(e) = kg::selftest_project(&tmp) {
        out.push_str(&format!("  [失敗] 造不出測試專案：{e}\n"));
        return finish(out, false, n);
    }
    let run_dir = join(work, "run");
    let put = |rel: &str, text: &str| {
        let p = if rel.starts_with(".claude/") { join(root, rel) } else { join(work, rel) };
        let _ = pyish::mkdir_p(&pparent(&p));
        let _ = std::fs::write(&p, text.as_bytes());
    };
    put(".claude/teams/methodology.json", &json!({"requirements": {"source": "openspec", "path": "openspec"}}).to_string());
    put("run/run.json", &slurp(&join(root, &format!("{RUN_REL}/run.json"))));
    put("paths.txt", "src/billing/export.ts\n");
    let task = |refs: Value| json!({"epic": "export-csv", "subtasks": [{"id": "t1", "title": "匯出加表頭", "kind": "code", "owns": ["src/billing/export.ts"], "depends_on": [], "acceptance": ["有表頭"], "spec_refs": refs}]});
    put("subs.json", &task(json!(["billing/export-invoice", "billing/archive-invoice#yearly-archive 年度封存", "billing/no-such-thing"])).to_string());

    // ---- 規格層的 id（還沒有圖）
    let base = run_stages(root, work, &[]);
    let get = |set: &[(String, String)], f: &str| set.iter().find(|x| x.0 == f).map(|x| x.1.clone()).unwrap_or_default();
    let t1 = get(&base, "build/t1/pack.md");
    let build_args = load_json(&join(&run_dir, "build/pack-args.json")).unwrap_or_else(|| json!({}));
    let errs: Vec<String> = pyfmt::items(build_args.get("spec_ref_errors")).iter().map(pyfmt::str_of).collect();
    check!("規格層 id：這次變更的差異優先（需求的行段取自變更目錄）", t1.contains("- openspec/changes/export-csv/specs/billing/spec.md:3-"), hpy::head(&t1, 600));
    check!("規格層 id：變更沒動到的情境取自現行規格、id 後面可以接說明", t1.contains("- openspec/specs/billing/spec.md:18-20"), hpy::head(&t1, 600));
    check!("規格層 id：找不到的才算錯", errs.len() == 1 && errs[0].contains("billing/no-such-thing") && errs[0].contains("沒有這個 id"), format!("{errs:?}"));
    check!("規格層 id：資料包不放規格內文以外的東西（只有行段與標題）", !t1.contains("SHALL"), hpy::head(&t1, 600));
    put(".claude/teams/methodology.json", "{}");
    let plain = run_stages(root, work, &[]);
    let errs_plain = pyfmt::items(load_json(&join(&run_dir, "build/pack-args.json")).unwrap_or_else(|| json!({})).get("spec_ref_errors")).len();
    check!("規格層 id：需求來源不是 openspec 時不認（照原本的寫法，沒有 spec 目錄就不定位）", !get(&plain, "build/t1/pack.md").contains("openspec/") && errs_plain == 0, get(&plain, "build/t1/pack.md"));
    put(".claude/teams/methodology.json", &json!({"requirements": {"source": "openspec", "path": "openspec"}}).to_string());
    put("subs.json", &task(json!(["billing/export-invoice"])).to_string());
    let base = run_stages(root, work, &[]);
    check!("沒有圖：資料包裡沒有知識圖的內容", base.iter().all(|(_, t)| !t.contains("知識圖")), "");

    // ---- 有圖
    let built = kg::build(&kg::BuildOpts { root: Some(root.to_string()), ..Default::default() });
    check!("建圖", built.is_ok(), format!("{:?}", built.as_ref().err()));
    let with = run_stages(root, work, &[]);
    let plan = get(&with, "plan/pack.md");
    let plan_args: Value = serde_json::from_str(&get(&with, "plan/pack-args.json")).unwrap_or_else(|_| json!({}));
    check!("plan：資料包多一節，帶程式行段、既有需求，包在資料標籤裡", plan.contains("## 知識圖查到的位置") && plan.contains("- src/billing/export.ts:") && plan.contains("billing/export-invoice") && plan.contains("<kg-data") && plan.contains("</kg-data>"), hpy::head(&plan, 1500));
    check!("plan：進行中的變更動到的需求排在前面", plan.contains("進行中的變更 export-csv 動到它"), "");
    check!("plan：只有位置與理由，不帶程式內文", !plan.contains("BODY_TEXT_NOT_IN_OUTPUT") && !plan.contains(root), "");
    let code = plan_args.get("located").and_then(|l| l.get("code-contracts")).cloned().unwrap_or_else(|| json!({}));
    let locs = pyfmt::items(code.get("locations"));
    check!("plan：程式角度命中夠（三個符號、三個檔、接得到需求、和工具定位有重疊）→ located 標成夠用", code.get("enough") == Some(&json!(true)) && code.get("source") == Some(&json!("kg")) && locs.len() >= 3 && locs.len() <= locate::MAX_SEGMENTS
        && locs.iter().all(|l| l.get("start").and_then(Value::as_u64).is_some_and(|s| s >= 1) && l.get("end").and_then(Value::as_u64).is_some()) && locs.iter().take(3).all(|l| l.get("path").and_then(Value::as_str).is_some_and(|p| p.starts_with("src/"))), code.to_string());
    let base_plan: Value = serde_json::from_str(&get(&base, "plan/pack-args.json")).unwrap_or_else(|_| json!({}));
    check!("plan：沒有圖時同一個角度不算夠（只是定位 agent 的起點）", base_plan.get("located").and_then(|l| l.get("code-contracts")).and_then(|c| c.get("enough")) == Some(&json!(false)), base_plan.to_string());
    let other_keys = |v: &Value| -> Value {
        let mut m = v.as_object().cloned().unwrap_or_default();
        if let Some(l) = m.get_mut("located").and_then(Value::as_object_mut) {
            l.remove("code-contracts");
        }
        Value::Object(m)
    };
    check!("plan：pack-args 其他的鍵和沒有圖時相同", other_keys(&plan_args) == other_keys(&base_plan), "");
    {
        // 只命中一個符號：不夠，located 不動
        let mut sink = String::new();
        let mut err = String::new();
        let _ = run(root, &argv(&["plan", "--run", &run_dir, "--repo", root, "--requirement", "示範客戶 ACME：formatRow 的輸出要補零", "--no-kb", "--json", "--today", "2026-01-10"]), &mut sink, &mut err);
        let a = load_json(&join(&run_dir, "plan/pack-args.json")).unwrap_or_else(|| json!({}));
        let c = a.get("located").and_then(|l| l.get("code-contracts"));
        check!("plan：命中不夠（一個符號）時不標成夠用", c.is_none_or(|c| c.get("enough") != Some(&json!(true)) && c.get("source").is_none()), a.to_string());
    }
    let design = get(&with, "design/pack.md");
    check!("design：要改的檔現在連著的需求與測試", design.contains("要改的檔現在連著的需求與測試") && design.contains("billing/export-invoice") && design.contains("src/billing/export.test.ts"), hpy::head(&design, 1500));
    let t1 = get(&with, "build/t1/pack.md");
    check!("build：子任務資料包帶檔案的呼叫者與相依", t1.contains("呼叫者與相依") && t1.contains("- src/billing/export.test.ts:") && t1.contains("- src/billing/format.ts:") && t1.contains("- src/lib/money.ts:"), hpy::head(&t1, 1500));
    check!("build：原本的內容還在（子任務、spec 行段），圖的一節接在最後", t1.starts_with(&get(&base, "build/t1/pack.md").trim_end_matches('\n').to_string()), "");
    check!("build：pack-args 和沒有圖時相同（內容寫在資料包裡，不加新的鍵）", get(&with, "build/pack-args.json") == get(&base, "build/pack-args.json"), "");
    let deliv = |set: &[(String, String)]| -> Vec<(String, Vec<String>)> {
        let v: Value = serde_json::from_str(&get(set, "deliver/pack-args.json")).unwrap_or_else(|_| json!({}));
        pyfmt::items(v.get("deliverables").and_then(|d| d.get("affected"))).iter().map(|a| (pyfmt::str_of(a.get("id").unwrap_or(&Value::Null)), pyfmt::items(a.get("sections")).iter().map(pyfmt::str_of).collect())).collect()
    };
    check!("deliver：沒有圖時這份文件不在受影響清單（設定裡沒有寫觸發條件）", deliv(&base).is_empty(), format!("{:?}", deliv(&base)));
    check!("deliver：有圖時依連結多列受影響的小節", deliv(&with) == vec![("guide".to_string(), vec!["匯出".to_string()])], format!("{:?}", deliv(&with)));
    check!("deliver：資料包列出多列的小節與理由", get(&with, "deliver/pack.md").contains("依連結多列的交付文件小節") && get(&with, "deliver/pack.md").contains("- docs/guide.md ｜ 小節「匯出」"), hpy::head(&get(&with, "deliver/pack.md"), 1500));
    check!("design：pack-args 和沒有圖時相同", get(&with, "design/pack-args.json") == get(&base, "design/pack-args.json"), "");
    let again = run_stages(root, work, &[]);
    check!("可重現：同一張圖重跑，每個檔逐字相同", again == with, again.iter().zip(&with).filter(|(a, b)| a != b).map(|(a, _)| a.0.clone()).collect::<Vec<_>>().join("、"));
    let off = run_stages(root, work, &["--no-kg"]);
    check!("--no-kg：和沒有圖時逐字相同", off == base, off.iter().zip(&base).filter(|(a, b)| a != b).map(|(a, _)| a.0.clone()).collect::<Vec<_>>().join("、"));
    // 圖壞掉：開不了就當沒有
    let db = kg::layout(Some(root), None, None, None).map(|l| l.db).unwrap_or_default();
    let _ = std::fs::write(&db, b"not a database");
    let mut err = String::new();
    let mut sink = String::new();
    let code = run(root, &argv(&["plan", "--run", &run_dir, "--repo", root, "--requirement", REQ_TEXT, "--no-kb", "--json", "--today", "2026-01-10"]), &mut sink, &mut err);
    let broken = run_stages(root, work, &[]);
    check!("圖開不了：照常產出，和沒有圖時逐字相同，也不多印警告", code == 0 && broken == base && err.is_empty(), format!("{code} {err}"));
    finish(out, ok_all, n)
}
