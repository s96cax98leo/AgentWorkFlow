//! 交件信封檢查（`awf hook envelope`）。由 `plugin/hooks/envelope_hook.py` 移植。
//! 只檢查 team 角色（見 `config::is_team_agent`）與 team 任務；驗證器是 `crate::schema`（原本的 schemalib.py）。
//!
//! 掛在四個事件：
//!   - PreToolUse SubagentHandback：回報在 tool_input.message。主要攔截點
//!   - PreToolUse StructuredOutput：只檢查 schema 管不到的語意規則
//!   - SubagentStop：子 agent 以文字結束時；stop_hook_active 為 true 就放行，避免無限迴圈
//!   - TaskCompleted：team 任務的描述要寫 `output: <runs_dir>/<run>/<檔>.json`，這裡讀那個檔驗證
//!
//! 專案還沒 init 時不檢查、直接放行。Python 版看的是 `.claude/teams/tools/schemalib.py` 在不在；
//! 這裡驗證器內建，改成「schemalib.py 或 schemas/envelope.schema.json 任一存在」就算已 init。

use super::guard::{err, say, Stop, R};
use super::path;
use super::pyjson;
use super::re::{group, Re};
use crate::config::{self, Config, PyErr};
use crate::pyfmt;
use crate::schema::{extract_json, Schemas};
use serde_json::Value;

static OUTPUT_FILE: Re = Re::new(r"output:\s*(\S+\.json)", "");

fn fail(errs: &[String], place: &str) -> Stop {
    say(&format!("交件信封不合格（{place}，格式見 .claude/teams/schemas/envelope.schema.json）："));
    for e in errs {
        say(&format!("- {e}"));
    }
    Stop::Exit(2)
}

fn check(sch: &Schemas, env: &Value) -> R<Vec<String>> {
    sch.check(env, None).map_err(|m| {
        let class = if path::exists(&path::join(&sch.teams_dir.to_string_lossy(), "schemas/envelope.schema.json")) { "JSONDecodeError" } else { "FileNotFoundError" };
        Stop::Err(PyErr::new(class, &m))
    })
}

/// `x.get(key, "")` 當文字用（給 extract_json）：字串原樣；其他型別在 Python 會丟 TypeError。
fn text_of(v: Option<&Value>) -> R<String> {
    match v {
        None => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => err("TypeError", "expected string or bytes-like object"),
    }
}

const NEED_BLOCK: &str = "回報最後要附一個 ```json 區塊（交件信封）";

pub fn run(data: &Value, cfg: &Config, ws: &str) -> R<()> {
    let teams = path::join(ws, ".claude/teams");
    if !path::exists(&path::join(&teams, "tools/schemalib.py")) && !path::exists(&path::join(&teams, "schemas/envelope.schema.json")) {
        return Ok(());
    }
    let sch = Schemas::new(teams);
    let event = data.get("hook_event_name").and_then(Value::as_str).unwrap_or("");
    let agent = super::guard::str_or_empty(data, "agent_type")?;
    let team = config::is_team_agent(&agent, cfg); // 完整 agent_type；使用者自己的 agent 不檢查信封

    if event == "PreToolUse" {
        let tool = data.get("tool_name").and_then(Value::as_str).unwrap_or("");
        let ti = match data.get("tool_input") {
            Some(v) if pyfmt::truthy(v) => v.clone(),
            _ => Value::Object(Default::default()),
        };
        if tool == "StructuredOutput" {
            if ti.is_object() && ti.get("status").is_some() && ti.get("payload").is_some() {
                let errs = sch.semantic_errors(&ti);
                if !errs.is_empty() {
                    return Err(fail(&errs, "StructuredOutput"));
                }
            }
            return Ok(());
        }
        if !team {
            return Ok(());
        }
        if !ti.is_object() {
            return err("AttributeError", "tool_input 不是物件");
        }
        let Some(env) = extract_json(&text_of(ti.get("message"))?) else {
            return Err(fail(&[NEED_BLOCK.to_string()], "SubagentHandback"));
        };
        let errs = check(&sch, &env)?;
        if !errs.is_empty() {
            return Err(fail(&errs, "SubagentHandback"));
        }
        return Ok(());
    }

    if event == "SubagentStop" {
        if pyfmt::truthy_opt(data.get("stop_hook_active")) || !team {
            return Ok(());
        }
        let msg = data.get("last_assistant_message");
        if !pyfmt::truthy_opt(msg) {
            return Ok(()); // 用 SubagentHandback 交件時沒有這個欄位；已在 PreToolUse 檢查過
        }
        let errs = match extract_json(&text_of(msg)?) {
            None => vec![NEED_BLOCK.to_string()],
            Some(env) => check(&sch, &env)?,
        };
        if !errs.is_empty() {
            return Err(fail(&errs, "SubagentStop"));
        }
        return Ok(());
    }

    if event == "TaskCompleted" {
        let runs = format!("{}/", cfg.runs_dir);
        let part = |k: &str| data.get(k).map(pyfmt::str_of).unwrap_or_default();
        let desc = format!("{}\n{}", part("task_name"), part("task_description"));
        if !desc.contains(&runs) {
            return Ok(());
        }
        let Some(caps) = OUTPUT_FILE.captures(&desc) else {
            return Err(fail(&[format!("team 任務的描述要寫 `output: {runs}<run>/<檔>.json`")], "TaskCompleted"));
        };
        let mut out = path::pp(group(&desc, &caps, 1).unwrap_or(""));
        if !path::is_abs(&out) {
            let base = match data.get("cwd") {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                Some(v) if pyfmt::truthy(v) => return err("TypeError", "cwd 不是字串"),
                _ => ws.to_string(),
            };
            out = path::join(&base, &out);
        }
        let shown = path::display(&out);
        if !path::exists(&out) {
            return Err(fail(&[format!("找不到產出檔 {shown}")], "TaskCompleted"));
        }
        let text = match config::read_text(&out) {
            Ok(t) => t,
            Err(class) => return err(class, "讀不到產出檔"),
        };
        let env: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                let detail = pyjson::error_message(&text).unwrap_or_else(|| e.to_string());
                return Err(fail(&[format!("{shown} 不是合法 JSON：{detail}")], "TaskCompleted"));
            }
        };
        let errs = check(&sch, &env)?;
        if !errs.is_empty() {
            return Err(fail(&errs, &format!("TaskCompleted {}", path::name(&out))));
        }
    }
    Ok(())
}
