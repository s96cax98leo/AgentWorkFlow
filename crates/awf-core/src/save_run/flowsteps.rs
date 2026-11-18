//! 用步驟宣告的段（profile 的 `flow.stages[].steps`、`events.<名>.steps`）：工具端執行指令、政策核准的條件。
//!
//! workflow 裡 `check` 只是記號、loop 的 `until` 由 agent 回報 exit code；兩種指令都在這裡由工具自己再跑一次。
//! 指令只取自 profile 的定義（不取 args、不取回傳、不取 run.json），文字原樣執行、不代入任何東西。
//! 執行方式和限制文件的檢查指令（`constraints[].check`）相同：目標 repo（沒有就專案根目錄）當工作目錄、有逾時、輸出只留最後 300 字。

use super::checks::shell;
use super::py::{s, truthy, R};
use super::Ctx;
use crate::merge_gate::pyutil;
use crate::stages::Custom;
use crate::steps;
use serde_json::{json, Value};
use std::time::Duration;

/// 執行一份步驟定義裡的指令（`check` 與 loop 的 `until`）：（沒過的說明, 每個指令的紀錄 `{id, kind, command, exit, output}`）。
/// exit 不是 0、逾時、跑不起來都算沒過。
pub fn step_checks(ctx: &Ctx, def_steps: &Value, repo: Option<&str>) -> (Vec<String>, Vec<Value>) {
    let cwd = match repo {
        Some(r) if ctx.is_dir(r) => r.to_string(),
        _ => ctx.project_root(),
    };
    let (mut issues, mut records) = (Vec::new(), Vec::new());
    for c in steps::commands(def_steps) {
        let mut command = shell(&c.command);
        command.current_dir(ctx.abs(&cwd));
        let (code, tail) = match pyutil::run(&mut command, None, Some(Duration::from_secs(c.timeout))) {
            Ok(o) => {
                let text = format!("{}{}", pyutil::text(&o.stdout), pyutil::text(&o.stderr));
                (i64::from(o.code), pyutil::tail(super::py::strip(&text), 300).to_string())
            }
            Err(pyutil::RunErr::Timeout) => (-1, format!("逾時（{} 秒）", c.timeout)),
            Err(pyutil::RunErr::Os(e)) => (-1, format!("跑不起來：{}", crate::kb::PyErr::os(&e, &cwd).msg)),
        };
        records.push(json!({"id": c.id, "kind": c.kind, "command": c.command, "exit": code, "output": tail}));
        if code != 0 {
            let what = if c.kind == "until" { "loop 的結束條件" } else { "檢查指令" };
            issues.push(format!("步驟 {} 的{what}由工具重跑沒過（exit {code}）：{}", c.id, c.command));
        }
    }
    (issues, records)
}

/// 這份結果是不是「`awf next` 產生的標準 flow-steps 腳本、照現在這份定義跑出來、由 `awf done` 從 workflow 紀錄取得」的。
/// 標記（`flow_steps`）由 `awf done` 寫：workflow 自己回傳的同名欄位會被蓋掉或拿掉。
fn marker_issues(stage: &str, def: &Custom, result: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mark = result.get("flow_steps");
    let sha = mark.and_then(|m| m.get("sha1")).and_then(Value::as_str).unwrap_or_default();
    if !mark.is_some_and(Value::is_object) || mark.and_then(|m| m.get("stage")).and_then(Value::as_str) != Some(stage) {
        out.push("這份結果沒有標準步驟腳本的標記（不是 awf next 產生的 flow-steps 腳本、由 awf done 從 workflow 紀錄取得的結果）".to_string());
    } else if sha != steps::def_sha(&def.raw) {
        out.push("步驟定義在這一段跑完之後改過（和產生腳本當時的指紋對不上）：重跑這一段，或由人核准".to_string());
    }
    if result.get("stage").and_then(Value::as_str) != Some(stage) {
        out.push(format!("回傳的 stage 不是 {stage}"));
    }
    out
}

/// 不執行指令就看得出來的問題：定義不合規則、不是標準腳本的結果、照定義核對 trace 對不上。
pub fn static_issues(stage: &str, def: &Custom, profile: &Value, result: &Value) -> Vec<String> {
    let derrs = steps::errors(&def.steps, &format!("flow.stages[{stage}].steps"), steps::max_agents(profile));
    if !derrs.is_empty() {
        return derrs;
    }
    let mut out = marker_issues(stage, def, result);
    out.extend(steps::trace_issues(&def.steps, result));
    out
}

/// 用步驟宣告、`gate: "policy"` 的自訂段，政策核准另外要成立的條件（通用條件在 `policy_violations`）：
///
/// 1. 定義現在仍然合規則；
/// 2. 結果是標準 flow-steps 腳本照「現在這份定義」跑出來、由 `awf done` 從 workflow 紀錄取得的（[`marker_issues`]）；
/// 3. 每一步都有交件、角色對得上、沒有定義以外的派工，loop 每一輪都做完而且最後一輪回報通過（照 trace 核對，[`steps::trace_issues`]）；
/// 4. 所有 `check` 與 loop `until` 的指令由工具現在重跑、全部 exit 0（不採信 run.json 記的結果，也不採信回傳說的）。
///
/// 前三項有任何一項不成立就不執行指令（不替對不上的結果跑指令）。空清單＝成立。
pub fn flow_policy_errors(ctx: &Ctx, stage: &str, def: &Custom, profile: &Value, result: &Value, repo: Option<&str>) -> Vec<String> {
    let mut out = static_issues(stage, def, profile, result);
    if out.is_empty() {
        out.extend(step_checks(ctx, &def.steps, repo).0);
    }
    out
}

/// run.json 記的目標 repo（沒有回 None）。
pub fn target_of(data: &Value) -> R<Option<String>> {
    let t = super::py::get(data, "target_repo")?;
    Ok(if truthy(t) { Some(super::py::pp(&s(t))) } else { None })
}
