//! 人答覆 blocking 問題之後往下走（1.2）、照建議先做的確認與推翻、略過的子任務。
//!
//! 這裡只有「讀結果、對答覆、列出還缺什麼」的判定與 run.json 的記法；什麼時候能用由 `cli`（核准）與狀態機決定。
//! 原始信封一律不改寫：人答了什麼另外記在 run.json 那一段的 `decided`。
//! 有 `decided` 的段、有子任務被略過的 run 一律不能政策核准（[`super::policy_violations_with`]）。

use super::py::{self, get, idx, iter_or, s, s_or, truthy, R};
use super::trace::{envelopes, missing_agents};
use super::{dump, load, Ctx, BLOCKING_SEVERITIES};
use serde_json::{json, Map, Value};

/// run.json 那一段記「人已決定」的欄位。
pub const DECIDED: &str = "decided";
/// 照建議先做（`decisions.auto_accept`）時記的答覆者；和人的答覆區分，不寫進跨 run 的決策記憶。
pub const AUTO_BY: &str = "auto-recommended";
/// 帶這幾個 modifier 的 recipe 不照建議先做（一律交人）。
pub const AUTO_EXCLUDED_MODIFIERS: [&str; 3] = ["security", "migration", "public_api"];

fn blank_text(t: &str) -> bool {
    ["", "none", "無", "n/a"].contains(&py::strip(t).to_lowercase().as_str())
}

/// 這一段結果裡的 blocking 問題（信封的順序；同一題只留一筆）：`[{id: "#N", question, recommendation, risk}]`。
/// `risk` 沒標、不是 low 都當 high。
pub fn blocking_questions(result: &Value) -> R<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    for env in envelopes(result) {
        for q in iter_or(get(env, "open_questions")?)? {
            if !q.is_object() || !truthy(get(&q, "blocking")?) {
                continue;
            }
            let text = py::squash(&s_or(get(&q, "question")?, ""));
            if out.iter().any(|x| x.get("question").and_then(Value::as_str) == Some(text.as_str())) {
                continue;
            }
            let rec = py::squash(&s_or(get(&q, "recommendation")?, ""));
            let risk = if get(&q, "risk")?.as_str() == Some("low") { "low" } else { "high" };
            out.push(json!({"id": format!("#{}", out.len() + 1), "question": text, "recommendation": if blank_text(&rec) { String::new() } else { rec }, "risk": risk}));
        }
    }
    Ok(out)
}

/// 這一段停在 needs_decision，但「不是只差問題沒答」的理由（空的＝答完就能往下走）。
/// halted、有 agent 回報 blocked 或缺件、有未通過的驗收條目、反作弊發現、blocker／important 發現、skeptic 推翻，
/// 都不是答覆能解決的：照舊處理後重跑。等人決定的信封卻沒有 blocking 問題，也不知道它在等什麼，不適用。
pub fn not_answerable(stage: &str, result: &Value) -> R<Vec<String>> {
    let Some(o) = result.as_object() else { return Ok(vec!["這一段的結果不是物件".to_string()]) };
    let mut why: Vec<String> = Vec::new();
    if o.get("halted").is_some_and(truthy) {
        why.push("workflow 標了 halted（流程中斷）".to_string());
    }
    for p in missing_agents(result) {
        why.push(format!("agent 缺件：{p}"));
    }
    let empty = json!({});
    let envs = envelopes(result);
    if envs.is_empty() {
        why.push(super::NO_ENVELOPE.to_string());
    }
    for (i, env) in envs.iter().enumerate() {
        let status = get(env, "status")?.as_str().unwrap_or_default();
        if status == "blocked" {
            why.push(format!("信封 {i} 回報 blocked"));
        }
        let qs = iter_or(get(env, "open_questions")?)?;
        if status == "needs_decision" && !qs.iter().any(|q| q.is_object() && q.get("blocking").is_some_and(truthy)) {
            why.push(format!("信封 {i} 在等人決定，卻沒有 blocking 的問題（不知道它在等什麼）"));
        }
        let payload = match get(env, "payload")? {
            v @ Value::Object(_) => v,
            _ => &empty,
        };
        for f in iter_or(get(payload, "findings")?)? {
            if f.get("severity").and_then(Value::as_str).is_some_and(|x| BLOCKING_SEVERITIES.contains(&x)) {
                why.push(format!("信封 {i} 有 {} 發現 {}", s(get(&f, "severity")?), s_or(get(&f, "id")?, "")));
            }
        }
        for v in iter_or(get(payload, "verdicts")?)? {
            if !v.is_object() || get(&v, "refuted")? != &Value::Bool(false) {
                why.push(format!("信封 {i} 的 skeptic 推翻了產出"));
            }
        }
        if stage == "test" && payload.get("criteria").is_some() {
            for c in iter_or(get(payload, "criteria")?)? {
                if c.get("result").and_then(Value::as_str) != Some("pass") {
                    why.push(format!("驗收條件 {} 不是 pass", c.get("id").map(s).unwrap_or_else(|| "?".to_string())));
                }
            }
            if truthy(get(payload, "anti_cheat")?) {
                why.push("反作弊有發現".to_string());
            }
        }
    }
    if why.is_empty() && blocking_questions(result)?.is_empty() {
        why.push("沒有 blocking 的問題可以答".to_string());
    }
    Ok(why)
}

/// 把 `--answer` 對到 blocking 問題：鍵是序號（`#N`）或問題原文。每一題 blocking 都要有答覆；
/// 對不到任何問題的答覆也拒絕（序號超出範圍、原文不是這一段的問題）。
/// 回傳（要記進決策記憶的（問題, 答覆）、run.json 的 decided.answers、錯誤）。
pub fn match_answers(result: &Value, answers: &[(String, String)]) -> R<(Vec<(String, String)>, Vec<Value>, Vec<String>)> {
    let blocking = blocking_questions(result)?;
    let mut others: Vec<String> = Vec::new();
    for env in envelopes(result) {
        for q in iter_or(get(env, "open_questions")?)? {
            if q.is_object() {
                others.push(py::squash(&s_or(get(&q, "question")?, "")));
            }
        }
    }
    let mut errs: Vec<String> = Vec::new();
    let mut got: Vec<Option<String>> = vec![None; blocking.len()];
    let mut items: Vec<(String, String)> = Vec::new();
    for (k, a) in answers {
        let key = py::squash(k);
        let pos = blocking.iter().position(|b| b.get("id").and_then(Value::as_str) == Some(key.as_str()) || b.get("question").and_then(Value::as_str) == Some(key.as_str()));
        match pos {
            Some(i) => {
                if got.get(i).is_some_and(Option::is_some) {
                    errs.push(format!("同一題答了兩次：{}", py::take(&key, 60)));
                }
                if let Some(slot) = got.get_mut(i) {
                    *slot = Some(a.clone());
                }
            }
            None if key.starts_with('#') => errs.push(format!("答覆 {} 對不到問題：這一段的 blocking 問題只有 #1～#{}", py::take(&key, 20), blocking.len())),
            None if others.contains(&key) => items.push((py::take(&key, 300).to_string(), a.clone())),
            None => errs.push(format!("答覆對不到這一段的任何問題：{}（用序號 #N 或問題原文）", py::take(&key, 60))),
        }
    }
    let mut decided: Vec<Value> = Vec::new();
    let mut lacking: Vec<String> = Vec::new();
    for (b, a) in blocking.iter().zip(&got) {
        let (id, q) = (s(idx(b, "id")?), s(idx(b, "question")?));
        match a {
            Some(a) if !py::strip(a).is_empty() => {
                decided.push(json!({"id": id, "question": q, "answer": py::squash(a)}));
                items.push((py::take(&q, 300).to_string(), a.clone()));
            }
            _ => lacking.push(format!("{id} {}", py::take(&q, 60))),
        }
    }
    if !lacking.is_empty() {
        errs.push(format!("每一題 blocking 的問題都要有答覆，還缺：{}", lacking.join("；")));
    }
    Ok((items, decided, errs))
}

fn awf_list(data: &Value, key: &str) -> Vec<Value> {
    data.get("awf").and_then(|a| a.get(key)).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// 還沒有人確認的暫定做法（`awf.unconfirmed`）：一筆一行的說明。
pub fn unconfirmed_of(data: &Value) -> Vec<String> {
    awf_list(data, "unconfirmed")
        .iter()
        .map(|q| format!("[{}] {} → {}", py::take(&s_or(q.get("stage").unwrap_or(&Value::Null), "?"), 40), py::take(&py::squash(&s_or(q.get("question").unwrap_or(&Value::Null), "")), 80), py::take(&py::squash(&s_or(q.get("applied").unwrap_or(&Value::Null), "")), 60)))
        .collect()
}

/// 照建議先做（`auto: true`）而且還沒有人確認的那幾筆。
pub fn auto_unconfirmed(data: &Value) -> Vec<Value> {
    awf_list(data, "unconfirmed").into_iter().filter(|q| q.get("auto") == Some(&Value::Bool(true))).collect()
}

/// 被略過的子任務（`awf.skipped_subtasks`）的 id。格式不對的項目也算一筆（不因為讀不懂就當成沒有）。
/// 資料包的一節：這個 run 裡靠答覆往下走的題目與被略過的子任務。沒有這些紀錄時回空（資料包逐字不變）。
/// 內容是人打的字與 agent 寫的問題：每一欄都先壓成一行、截短（資料包開頭已經聲明全部是資料）。
pub fn pack_lines(data: &Value) -> Vec<String> {
    let show = |v: Option<&Value>, n: usize| crate::hooks::safe::text(v.and_then(Value::as_str).unwrap_or_default(), n);
    let mut out: Vec<String> = Vec::new();
    if let Some(stages) = data.get("stages").and_then(Value::as_object) {
        for (name, st) in stages {
            let Some(d) = st.get("decided") else { continue };
            let auto = d.get("auto") == Some(&Value::Bool(true)) && d.get("confirmed").is_none();
            for a in d.get("answers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
                out.push(format!(
                    "- {name} 段｜{}｜答覆：{}（{}）",
                    show(a.get("question"), 200),
                    show(a.get("answer"), 300),
                    if auto { "照建議先做，還沒有人確認過".to_string() } else { format!("{} 答的", show(d.get("by"), 40)) }
                ));
            }
        }
    }
    let mut sk: Vec<String> = Vec::new();
    for x in data.get("awf").and_then(|a| a.get("skipped_subtasks")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        sk.push(format!("- 子任務 {} 被略過、沒有做（{} 決定；理由：{}）", show(x.get("id"), 80), show(x.get("by"), 40), show(x.get("reason"), 300)));
    }
    if out.is_empty() && sk.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["## 這個 run 的決定與略過".to_string(), String::new()];
    if !out.is_empty() {
        lines.push("以下的題目是靠答覆往下走的；那一段的產出文件還沒有反映答覆，以答覆為準：".to_string());
        lines.append(&mut out);
    }
    if !sk.is_empty() {
        if lines.len() > 2 {
            lines.push(String::new());
        }
        lines.push("以下的子任務由人決定略過：它們的驗收條件沒有實作，不要當成已完成，也不要自己補做：".to_string());
        lines.append(&mut sk);
    }
    lines
}

pub fn skipped_subtasks(data: &Value) -> Vec<String> {
    awf_list(data, "skipped_subtasks").iter().map(|x| x.get("id").and_then(Value::as_str).map_or_else(|| "?".to_string(), str::to_string)).collect()
}

/// 推送、合併之前要先處理的事（空的＝沒有）：還有沒人確認的暫定做法一律擋；
/// 有子任務被略過時，`ack_skipped`（人當下確認過）為 false 就擋。
pub fn run_holds(data: &Value, ack_skipped: bool) -> Vec<String> {
    let mut out = Vec::new();
    let un = unconfirmed_of(data);
    if !un.is_empty() {
        out.push(format!(
            "還有 {} 項暫定做法沒有人確認過（{}）：先由人確認（awf approve --run <run_dir> --confirm-unconfirmed --by <誰>）或推翻（--reject-unconfirmed <段>）",
            un.len(),
            un.iter().take(3).cloned().collect::<Vec<_>>().join("；")
        ));
    }
    let sk = skipped_subtasks(data);
    if !sk.is_empty() && !ack_skipped {
        out.push(format!("這個 run 有子任務被略過（{}）：要人當下確認才能繼續（確認後加 --confirm-skipped-subtasks）", sk.join("、")));
    }
    out
}

/// 人確認了全部暫定做法：清掉 `awf.unconfirmed`，照建議先做的段記下確認者。回傳要記進決策記憶的（問題, 答覆）。
pub fn confirm_all(data: &mut Value, by: &str, at: &str) -> R<Vec<(String, String)>> {
    let list = awf_list(data, "unconfirmed");
    let mut items = Vec::new();
    for q in &list {
        let (question, applied) = (py::squash(&s_or(get(q, "question")?, "")), py::squash(&s_or(get(q, "applied")?, "")));
        if !question.is_empty() && !applied.is_empty() && !question.contains('=') {
            items.push((py::take(&question, 300).to_string(), py::take(&applied, 600).to_string()));
        }
        if let Some(Value::Object(d)) = q.get("stage").and_then(Value::as_str).and_then(|st| data.get_mut("stages").and_then(|x| x.get_mut(st)).and_then(|x| x.get_mut(DECIDED))) {
            d.insert("confirmed".into(), json!({"by": by, "at": at}));
        }
    }
    if let Some(Value::Object(aw)) = data.get_mut("awf") {
        aw.shift_remove("unconfirmed");
    }
    Ok(items)
}

/// `awf save-run confirm-unconfirmed <run> --by <誰> --at <時間>`：人確認全部暫定做法（不核准任何一段）。
pub fn confirm_cmd(ctx: &Ctx, run: &str, by: &str, at: &str, out: &mut String, err: &mut String) -> R<i32> {
    let mut data = load(ctx, run)?;
    let n = unconfirmed_of(&data).len();
    if by.is_empty() || at.is_empty() || by == "policy" || by == AUTO_BY {
        err.push_str("不能記錄：--by 與 --at 都必填，而且確認只能由人做\n");
        return Ok(1);
    }
    if n == 0 {
        out.push_str("沒有等人確認的暫定做法\n");
        return Ok(0);
    }
    let items = confirm_all(&mut data, by, at)?;
    dump(ctx, run, &data)?;
    out.push_str(&format!("已確認 {n} 項暫定做法：{by} @ {at}\n"));
    record(ctx, run, &items, by, at, "", out, err);
    Ok(0)
}

/// 決策記憶是輔助：寫不出來只警告。
pub fn record(ctx: &Ctx, run: &str, items: &[(String, String)], by: &str, at: &str, stage: &str, out: &mut String, err: &mut String) {
    if items.is_empty() {
        return;
    }
    match super::record_decisions(ctx, run, items, by, at, "epic", "", stage, err) {
        Ok(n) => out.push_str(&format!("已記 {n} 筆決定（{}；範圍 epic）\n", crate::pack::DECISIONS)),
        Err(e) => err.push_str(&format!("警告：決定沒記成（{}）\n", e.msg)),
    }
}

/// `awf save-run reject-unconfirmed <run> <段> --by <誰> --at <時間>`：人推翻某一段照建議先做的答覆。
/// 那一段回到 needs_decision（答覆與核准清掉），排在它後面、已經 done 的段改成 stale（核准清掉，要重跑）。
/// 只認照建議先做的段（`decided.auto`）；別的段照舊回對應的段修正。
pub fn reject_cmd(ctx: &Ctx, run: &str, stage: &str, by: &str, at: &str, out: &mut String, err: &mut String) -> R<i32> {
    let mut data = load(ctx, run)?;
    let auto = data.get("stages").and_then(|x| x.get(stage)).and_then(|x| x.get(DECIDED)).is_some_and(|d| d.get("auto") == Some(&Value::Bool(true)));
    let mut errs: Vec<String> = Vec::new();
    if by.is_empty() || at.is_empty() || by == "policy" || by == AUTO_BY {
        errs.push("--by 與 --at 都必填，而且推翻只能由人做".to_string());
    }
    if truthy(get(&data, "abandoned")?) || truthy(get(&data, "closed")?) {
        errs.push("這個 run 已經結束".to_string());
    }
    if !auto {
        errs.push(format!("{stage} 段沒有照建議先做的答覆可以推翻（只有 decisions.auto_accept 先做的段適用）"));
    }
    if !errs.is_empty() {
        for e in errs {
            err.push_str(&format!("不能推翻：{e}\n"));
        }
        return Ok(1);
    }
    let want = get(&crate::recipe::recipe_of_run(&data).map_err(py::from_recipe)?, "stages")?.clone();
    let order = crate::stages::run_order(&want);
    let at_pos = order.iter().position(|x| x == stage).unwrap_or(order.len());
    let mut stale: Vec<String> = Vec::new();
    if let Some(Value::Object(stages)) = data.get_mut("stages") {
        for (i, name) in order.iter().enumerate() {
            let Some(Value::Object(st)) = stages.get_mut(name.as_str()) else { continue };
            if i == at_pos {
                st.insert("status".into(), json!("needs_decision"));
                st.shift_remove(DECIDED);
                st.shift_remove("approved");
                st.insert("auto_rejected".into(), json!({"by": by, "at": at}));
            } else if i > at_pos && st.get("status").and_then(Value::as_str) == Some("done") {
                st.insert("status".into(), json!("stale"));
                st.shift_remove("approved");
                st.shift_remove(DECIDED);
                stale.push(name.clone());
            }
        }
    }
    if let Some(Value::Object(aw)) = data.get_mut("awf") {
        let gone = |q: &Value| q.get("stage").and_then(Value::as_str).is_some_and(|x| x == stage || stale.iter().any(|n| n == x));
        let rest: Vec<Value> = aw.get("unconfirmed").and_then(Value::as_array).map(|l| l.iter().filter(|q| !gone(q)).cloned().collect()).unwrap_or_default();
        if rest.is_empty() {
            aw.shift_remove("unconfirmed");
        } else {
            aw.insert("unconfirmed".into(), Value::Array(rest));
        }
    }
    dump(ctx, run, &data)?;
    out.push_str(&format!("{stage}：照建議先做的答覆已推翻（{by} @ {at}），回到 needs_decision；由人答覆（awf approve … --answer）或重跑這一段\n"));
    if !stale.is_empty() {
        out.push_str(&format!("改成 stale（核准已清除，要重跑）：{}\n", stale.join("、")));
    }
    Ok(0)
}

/// 照建議先做的資格（`decisions.auto_accept: recommended`）。回傳 `Ok(Some(answers))`＝可以，`Ok(None)`＝不適用（交人）。
/// 條件：設定開啟；不是 plan、不是自訂段；recipe 已鎖定而且沒有排除的 modifier；這一段沒被人推翻過；
/// 只差問題沒答（[`not_answerable`]）；每一題 blocking 都有建議、risk 標成 low；題數不超過上限（預設 5，可設 1～20）。
pub fn auto_acceptable(profile: &Value, data: &Value, stage: &str, result: &Value) -> R<Option<Vec<Value>>> {
    let conf = profile.get("decisions").filter(|d| d.is_object());
    if conf.and_then(|d| d.get("auto_accept")).and_then(Value::as_str) != Some("recommended") {
        return Ok(None);
    }
    if !super::SDLC.contains(&stage) || super::HUMAN_ONLY.contains(&stage) || !super::has_recipe(data)? {
        return Ok(None);
    }
    let rec = idx(data, "recipe")?;
    let mods = iter_or(get(rec, "modifiers")?)?;
    if !truthy(get(rec, "locked")?) || mods.iter().any(|m| m.as_str().is_none_or(|m| AUTO_EXCLUDED_MODIFIERS.contains(&m))) {
        return Ok(None);
    }
    let st = get(idx(data, "stages")?, stage)?;
    if get(st, "status")?.as_str() != Some("needs_decision") || truthy(get(st, "auto_rejected")?) || !truthy(get(st, "envelopes")?) {
        return Ok(None);
    }
    if !not_answerable(stage, result)?.is_empty() {
        return Ok(None);
    }
    let max = crate::tune::int(conf.and_then(|d| d.get("auto_accept_max")), 1, 20, 5) as usize;
    let blocking = blocking_questions(result)?;
    if blocking.is_empty() || blocking.len() > max {
        return Ok(None);
    }
    let mut answers = Vec::new();
    for b in &blocking {
        let rec = s(idx(b, "recommendation")?);
        if rec.is_empty() || idx(b, "risk")?.as_str() != Some("low") {
            return Ok(None);
        }
        answers.push(json!({"id": idx(b, "id")?, "question": idx(b, "question")?, "answer": rec}));
    }
    Ok(Some(answers))
}

/// 把「已決定」記進 run.json 的那一段：狀態改成 done（原始信封不動）。`auto`＝照建議先做（不是人答的）。
pub fn mark_decided(data: &mut Value, stage: &str, by: &str, at: &str, answers: Vec<Value>, auto: bool) -> R<()> {
    let st = py::obj_mut(data.get_mut("stages").and_then(|x| x.get_mut(stage)).ok_or_else(|| py::key_err(stage))?)?;
    let mut d = Map::new();
    d.insert("by".into(), json!(by));
    d.insert("at".into(), json!(at));
    d.insert("from".into(), json!("needs_decision"));
    if auto {
        d.insert("auto".into(), json!(true));
    }
    d.insert("answers".into(), Value::Array(answers));
    st.insert(DECIDED.into(), Value::Object(d));
    st.insert("status".into(), json!("done"));
    Ok(())
}
