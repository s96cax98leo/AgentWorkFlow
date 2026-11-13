//! 決策記憶：人在閘門上回答的問題，記進 `<runs_dir>/decisions.jsonl`，並重寫給人與知識庫看的 `決策記憶.md`。

use super::py::{self, pj, pname, pparent, R};
use super::Ctx;
use crate::kb::PyErr;
use crate::pack;
use serde_json::{Map, Value};

/// 決策記憶的輸入檢查：只有人能記、問題與答覆都要有、長度有上限、範圍與到期日格式要對。
/// `items` 是（問題, 答覆）。
pub fn decision_errors(items: &[(String, String)], by: &str, scope: &str, expires: &str) -> Vec<String> {
    let mut errs = Vec::new();
    if by == "policy" {
        errs.push("決定只能由人記（--by policy 不能帶答覆）".to_string());
    }
    if !pack::DECISION_SCOPES.contains(&scope) {
        errs.push(format!("範圍只能是 {}：{scope}", pack::DECISION_SCOPES.join("、")));
    }
    if !expires.is_empty() && !pack::DATE.is_match(expires) {
        errs.push(format!("到期日要寫成 YYYY-MM-DD：{expires}"));
    }
    for (q, a) in items {
        if py::strip(q).is_empty() || py::strip(a).is_empty() {
            errs.push(format!("答覆要寫成「問題=答覆」，兩邊都不能空：{}", py::take(if q.is_empty() { a } else { q }, 60)));
        } else if q.chars().count() > 300 || a.chars().count() > 600 {
            errs.push(format!("問題最多 300 字、答覆最多 600 字：{}", py::take(q, 40)));
        }
    }
    errs
}

/// 把決定附加到 `<runs_dir>/decisions.jsonl`，並重寫 `決策記憶.md`。回傳記了幾筆。
/// 附加失敗回 OSError（呼叫端決定是拒絕還是只警告）；檢視檔寫不出來只在 `err` 留警告。
#[allow(clippy::too_many_arguments)]
pub fn record_decisions(ctx: &Ctx, run: &str, items: &[(String, String)], by: &str, at: &str, scope: &str, expires: &str, stage: &str, err: &mut String) -> R<usize> {
    use std::io::Write;
    let resolved = ctx.resolve(run)?;
    let runs = pparent(&resolved);
    let run_name = pname(&resolved);
    let epic = pack::run_epic(&ctx.abs(run), None);
    let file = pj(&runs, pack::DECISIONS);
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&file).map_err(|e| PyErr::os(&e, &file))?;
    for (q, a) in items {
        let mut rec = Map::new();
        let mut put = |k: &str, v: &str| {
            rec.insert(k.to_string(), Value::String(v.to_string()));
        };
        put("question", &py::squash(q));
        put("answer", &py::squash(a));
        put("scope", scope);
        put("by", by);
        put("at", at);
        put("run", &run_name);
        for (k, v) in [("stage", stage), ("epic", epic.as_str()), ("expires", expires)] {
            if !v.is_empty() {
                put(k, v);
            }
        }
        let line = format!("{}\n", py::dumps_sorted(&Value::Object(rec)));
        f.write_all(line.as_bytes()).map_err(|e| PyErr::os(&e, &file))?;
    }
    drop(f);
    let view = pj(&runs, pack::DECISIONS_VIEW);
    if let Err(e) = pack::write_lf(&view, &pack::decisions_view(&runs, &ctx.today())) {
        err.push_str(&format!("警告：{} 寫不出來（{}）；decisions.jsonl 已記錄\n", pack::DECISIONS_VIEW, PyErr::os(&e, &view).msg));
    }
    Ok(items.len())
}

/// `--answer "<問題>=<答覆>"`：用第一個 `=`（或全形 `＝`）切開；`=` 優先。
pub fn split_answer(raw: &str) -> (String, String) {
    for sep in ['=', '＝'] {
        if let Some((q, a)) = raw.split_once(sep) {
            return (py::strip(q).to_string(), py::strip(a).to_string());
        }
    }
    (py::strip(raw).to_string(), String::new())
}
