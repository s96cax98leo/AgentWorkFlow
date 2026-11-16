//! 限制文件（profile 的 constraints）、拆工的確定性檢查（plan_check）、intent 檢查。

use super::py::{self, get, iter_or, pj, s, s_or, truthy, R, NULL};
use super::trace::envelopes;
use super::{Ctx, GHOSTWRITTEN, INDEX_NAME};
use crate::kb::pyish;
use crate::merge_gate::pyutil;
use crate::{pack, recipe};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

pub const CONSTRAINT_VERDICTS: [&str; 3] = ["符合", "偏離", "不適用"];
/// 檢查指令的逾時（秒）：限制沒寫 `timeout` 時的預設。
pub const CONSTRAINT_CHECK_TIMEOUT: u64 = 300;

/// 一項限制的檢查指令逾時秒數：`constraints[].timeout`（1–3600 的整數）；沒寫或不合法用預設。逾時仍然算沒過。
pub fn constraint_timeout(c: &Value) -> u64 {
    c.get("timeout").and_then(Value::as_u64).filter(|n| (1..=3600).contains(n)).unwrap_or(CONSTRAINT_CHECK_TIMEOUT)
}

/// 限制文件的實際位置：先找專案根目錄，再找目標 repo；都沒有回 None。
pub fn constraint_file(ctx: &Ctx, c: &Value, repo: Option<&str>) -> R<Option<String>> {
    let f = s_or(get(c, "file")?, "");
    let root = ctx.project_root();
    for base in [Some(root.as_str()), repo].into_iter().flatten() {
        if !f.is_empty() && ctx.is_file(&pj(base, &f)) {
            return Ok(Some(pj(base, &f)));
        }
    }
    Ok(None)
}

static ROW: LazyLock<Option<Regex>> = LazyLock::new(|| {
    let ws = format!("[{}]*", pyish::WS_CLASS);
    Regex::new(&format!(r"^{ws}\|{ws}`?\[?([a-z0-9][a-z0-9-]{{0,39}})\]?`?{ws}\|{ws}({}){ws}\|", CONSTRAINT_VERDICTS.join("|"))).ok()
});

/// 這一段的執行紀錄（`<run>/<stage>/*.md`）裡的〈限制對照〉列：{id: [(判定, 紀錄檔名)]}。
/// 代寫的紀錄只來自信封，不算 agent 做過對照。
pub fn constraint_rows(ctx: &Ctx, run: &str, stage: &str) -> HashMap<String, Vec<(String, String)>> {
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let d = pj(run, stage);
    if !ctx.is_dir(&d) {
        return out;
    }
    let mut names: Vec<String> = match std::fs::read_dir(ctx.abs(&d)) {
        Ok(rd) => rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".md")).collect(),
        Err(_) => return out,
    };
    names.sort();
    let Some(row) = ROW.as_ref() else { return out };
    for name in names {
        if name == INDEX_NAME {
            continue;
        }
        let Ok(text) = ctx.read_text_lossy(&pj(&d, &name)) else { continue };
        if text.starts_with("# 執行紀錄：") && text.split('\n').next().unwrap_or_default().contains(GHOSTWRITTEN) {
            continue;
        }
        for line in pyutil::splitlines(&text) {
            if let Some(m) = row.captures(line) {
                let (id, verdict) = (m.get(1).map_or("", |x| x.as_str()), m.get(2).map_or("", |x| x.as_str()));
                out.entry(id.to_string()).or_default().push((verdict.to_string(), name.clone()));
            }
        }
    }
    out
}

static AC_BODY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"AC[-_][A-Za-z0-9]+(?:[-_.][A-Za-z0-9]+)*").ok());
static WORD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_-]$").ok());

/// `AC_ID.findall(text)`：`(?<![\w-])AC[-_][A-Za-z0-9]+(?:[-_.][A-Za-z0-9]+)*`。
pub fn ac_ids(text: &str) -> Vec<String> {
    let (Some(body), Some(word)) = (AC_BODY.as_ref(), WORD.as_ref()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(m) = body.find_at(text, pos) {
        let before = text.get(..m.start()).and_then(|t| t.chars().next_back());
        let blocked = before.is_some_and(|c| word.is_match(c.encode_utf8(&mut [0u8; 4])));
        if blocked {
            pos = m.start() + text.get(m.start()..).and_then(|t| t.chars().next()).map_or(1, char::len_utf8);
        } else {
            out.push(m.as_str().to_string());
            pos = m.end();
        }
        if pos > text.len() {
            break;
        }
    }
    out
}

/// 這一段回傳裡的子任務清單（design.payload.subtasks、payload.subtasks 或頂層 subtasks）。
pub fn result_subtasks(result: &Value) -> Vec<Value> {
    let Some(r) = result.as_object() else { return Vec::new() };
    let design_payload = r.get("design").and_then(Value::as_object).and_then(|d| d.get("payload"));
    for c in [design_payload, r.get("payload"), Some(result)] {
        if let Some(Value::Array(subs)) = c.and_then(Value::as_object).and_then(|c| c.get("subtasks")) {
            if !subs.is_empty() && subs.iter().all(Value::is_object) {
                return subs.clone();
            }
        }
    }
    Vec::new()
}

/// 拆工的確定性檢查（plan_check）：在 design 閘門就抓出來，不要拖到 build 才爆。回傳問題清單（空＝沒問題）。
/// - 子任務 id 重複、相依指向不存在的子任務、相依有循環。
/// - 子任務沒有驗收條件（acceptance 是空的）；會改檔的子任務（code、test、doc）沒有 owns。
/// - 有 spec 檔而且 spec 與子任務都用「AC-…」編號時：spec 有、沒有任何子任務對應的驗收條件；子任務引用了 spec 沒有的驗收條件。
///   其中一邊沒用這種編號就不比（專案的寫法不同，工具判不了）。
///
/// 回傳裡沒有子任務時不檢查（那一段沒拆工）。
pub fn plan_check_issues(ctx: &Ctx, result: &Value, spec: Option<&str>) -> R<Vec<String>> {
    let subs = result_subtasks(result);
    if subs.is_empty() {
        return Ok(Vec::new());
    }
    let mut errs: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for x in &subs {
        ids.push(s_or(get(x, "id")?, ""));
    }
    let mut dup: Vec<&String> = ids.iter().filter(|i| !i.is_empty() && ids.iter().filter(|j| j == i).count() > 1).collect();
    dup.sort();
    dup.dedup();
    if !dup.is_empty() {
        errs.push(format!("子任務 id 重複：{}", dup.iter().map(|x| x.as_str()).collect::<Vec<_>>().join("、")));
    }
    let known = |d: &str| ids.iter().any(|i| i == d);
    let mut deps: Vec<(String, Vec<String>)> = Vec::new();
    for (x, id) in subs.iter().zip(&ids) {
        let ds: Vec<String> = iter_or(get(x, "depends_on")?)?.iter().map(s).collect();
        match deps.iter_mut().find(|(k, _)| k == id) {
            Some(slot) => slot.1 = ds,
            None => deps.push((id.clone(), ds)),
        }
    }
    for x in &subs {
        // 逐筆看（id 重複時 deps 這個表只留得住一筆）
        let lost: Vec<String> = iter_or(get(x, "depends_on")?)?.iter().map(s).filter(|d| !known(d)).collect();
        if !lost.is_empty() {
            errs.push(format!("子任務 {} 相依指向不存在的子任務：{}", s(get(x, "id")?), lost.join("、")));
        }
    }
    let mut left: Vec<(String, Vec<String>)> = deps
        .iter()
        .map(|(i, ds)| {
            let mut set: Vec<String> = Vec::new();
            for d in ds {
                if known(d) && !set.contains(d) {
                    set.push(d.clone());
                }
            }
            (i.clone(), set)
        })
        .collect();
    loop {
        let free: Vec<String> = left.iter().filter(|(_, ds)| ds.is_empty()).map(|(i, _)| i.clone()).collect();
        if free.is_empty() {
            break;
        }
        left.retain(|(i, _)| !free.contains(i));
        for (_, ds) in left.iter_mut() {
            ds.retain(|d| !free.contains(d));
        }
    }
    if !left.is_empty() {
        let mut names: Vec<&str> = left.iter().map(|(i, _)| i.as_str()).collect();
        names.sort_unstable();
        errs.push(format!("子任務相依有循環：{}", names.join("、")));
    }
    let any_filled = |v: &Value| -> R<bool> { Ok(iter_or(v)?.iter().any(|a| !py::strip(&s(a)).is_empty())) };
    for x in &subs {
        let i = s_or(get(x, "id")?, "?");
        if !any_filled(get(x, "acceptance")?)? {
            errs.push(format!("子任務 {i} 沒有驗收條件（acceptance 是空的）"));
        }
        let kind = get(x, "kind")?;
        if matches!(kind.as_str(), Some("code" | "test" | "doc")) && !any_filled(get(x, "owns")?)? {
            errs.push(format!("子任務 {i}（{}）沒有 owns：不知道它可以改哪些檔", s(kind)));
        }
    }
    if let Some(spec) = spec.filter(|p| ctx.is_file(p)) {
        let mut in_spec: Vec<String> = ctx.read_text_lossy(spec).map(|t| ac_ids(&t)).unwrap_or_default();
        in_spec.sort();
        in_spec.dedup();
        let mut used: Vec<String> = Vec::new();
        for x in &subs {
            let mut both = iter_or(get(x, "acceptance")?)?;
            both.extend(iter_or(get(x, "story_ids")?)?);
            for t in both {
                used.extend(ac_ids(&s(&t)));
            }
        }
        used.sort();
        used.dedup();
        if !in_spec.is_empty() && !used.is_empty() {
            let orphan: Vec<&String> = in_spec.iter().filter(|x| !used.contains(x)).collect();
            let ghost: Vec<&String> = used.iter().filter(|x| !in_spec.contains(x)).collect();
            let first12 = |v: &[&String]| format!("{}{}", v.iter().take(12).map(|x| x.as_str()).collect::<Vec<_>>().join("、"), if v.len() > 12 { "…" } else { "" });
            if !orphan.is_empty() {
                errs.push(format!("spec 有 {} 條驗收條件沒有任何子任務對應：{}", orphan.len(), first12(&orphan)));
            }
            if !ghost.is_empty() {
                errs.push(format!("子任務引用了 spec 沒有的驗收條件：{}", first12(&ghost)));
            }
        }
    }
    Ok(errs)
}

/// plan、design 段存檔與政策核准時的拆工檢查：spec 檔取目標 repo 裡這個 epic 的 `spec.md`。
pub fn plan_check_for(ctx: &Ctx, run: &str, stage: &str, result: &Value, data: &Value) -> R<Vec<String>> {
    if !matches!(stage, "plan" | "design") {
        return Ok(Vec::new());
    }
    let target = get(data, "target_repo")?;
    let repo = if truthy(target) { Some(ctx.abs(&py::pp(&s(target)))) } else { None };
    let mut epic = super::run_epic(ctx, run)?;
    if epic.is_empty() {
        let args = get(data, "args")?;
        epic = if truthy(args) { s_or(get(args, "epic")?, "") } else { String::new() };
    }
    let sd = pack::spec_dir(repo.as_deref(), &epic, "", &ctx.pack_root);
    plan_check_issues(ctx, result, sd.map(|d| pj(&d, "spec.md")).as_deref())
}

/// `sh -c <cmd>`（Python 的 `subprocess.run(cmd, shell=True)`）。
fn shell(cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string()));
        c.arg("/c").arg(cmd);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg(cmd);
        c
    }
}

/// 這一段適用的限制文件有沒有被遵守的確定性檢查。回傳（不能政策核准的理由, 檢查指令的結果）。
/// - 文件不存在：不行（限制讀不到，agent 等於沒有依據）。
/// - trace=table：這一段的執行紀錄要有該限制的對照列；沒有不行，有「偏離」不行（偏離要由人決定）。
///   build 段有 paths 的限制只發給對得上的子任務，不在這裡要求對照（test、deliver 有套用時在那裡要求）。
/// - check：在目標 repo（沒有就專案根目錄）執行，exit 非 0 或逾時不行。
///
/// `profile` 是 null 表示沒有 profile。
pub fn constraint_issues(ctx: &Ctx, run: &str, stage: &str, profile: &Value, repo: Option<&str>, run_checks: bool) -> R<(Vec<String>, Vec<Value>)> {
    let all = if truthy(profile) { get(profile, "constraints")? } else { &NULL };
    let mut cons: Vec<Value> = Vec::new();
    for c in iter_or(all)? {
        if !c.is_object() {
            continue;
        }
        let stages = get(&c, "stages")?;
        if truthy(stages) && py::contains(stages, stage)? {
            cons.push(c);
        }
    }
    if cons.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut issues: Vec<String> = Vec::new();
    let mut checks: Vec<Value> = Vec::new();
    let rows = constraint_rows(ctx, run, stage);
    for c in &cons {
        let (cid, f) = (s_or(get(c, "id")?, ""), s_or(get(c, "file")?, ""));
        if constraint_file(ctx, c, repo)?.is_none() {
            issues.push(format!("限制文件 [{cid}] {f} 不存在（專案根目錄與目標 repo 都找不到）"));
        }
        if get(c, "trace")?.as_str() == Some("table") && !(stage == "build" && truthy(get(c, "paths")?)) {
            let got = rows.get(&cid).map(Vec::as_slice).unwrap_or_default();
            if got.is_empty() {
                issues.push(format!("限制 [{cid}] 要逐條對照，這一段的執行紀錄卻沒有它的〈限制對照〉列"));
            }
            for (verdict, name) in got {
                if verdict == "偏離" {
                    issues.push(format!("限制 [{cid}] 有偏離（{name}），要由人決定"));
                }
            }
        }
        let Some(cmd) = get(c, "check")?.as_str().filter(|t| !py::strip(t).is_empty()) else { continue };
        if !run_checks {
            continue;
        }
        let cwd = match repo {
            Some(r) if ctx.is_dir(r) => r.to_string(),
            _ => ctx.project_root(),
        };
        let mut command = shell(cmd);
        command.current_dir(ctx.abs(&cwd));
        // 逾時秒數：constraints[].timeout（1–3600；沒寫或不合法用預設）。逾時仍然算沒過
        let secs = constraint_timeout(c);
        let (code, tail) = match pyutil::run(&mut command, None, Some(Duration::from_secs(secs))) {
            Ok(o) => {
                let text = format!("{}{}", pyutil::text(&o.stdout), pyutil::text(&o.stderr));
                (i64::from(o.code), pyutil::tail(py::strip(&text), 300).to_string())
            }
            Err(pyutil::RunErr::Timeout) => (-1, format!("逾時（{secs} 秒）")),
            Err(pyutil::RunErr::Os(e)) => (-1, format!("跑不起來：{}", crate::kb::PyErr::os(&e, &cwd).msg)),
        };
        checks.push(json!({"id": cid, "command": cmd, "exit": code, "output": tail}));
        if code != 0 {
            issues.push(format!("限制 [{cid}] 的檢查指令沒過（exit {code}）：{cmd}"));
        }
    }
    Ok((issues, checks))
}

/// 一定要有 intent：intent 信封 payload.documents 裡 kind=intent 的檔要存在，而且有必要的節（I1）。
/// `profile` 是 null 表示沒有 profile；`plan_mode` 是 recipe 的 plan_mode（原值）。
pub fn intent_errors(ctx: &Ctx, result: &Value, profile: &Value, plan_mode: &Value) -> R<Vec<String>> {
    let paths = intent_paths(result)?;
    if paths.is_empty() {
        return Ok(vec!["intent：回傳的信封 payload.documents 沒有 kind=intent 的文件（一定要有 intent）".to_string()]);
    }
    intent_file_errors(ctx, result, &paths, profile, plan_mode)
}

/// 回傳的信封 payload.documents 裡 kind=intent 的檔（照出現順序、不重複）。
pub fn intent_paths(result: &Value) -> R<Vec<String>> {
    let mut paths: Vec<String> = Vec::new();
    let empty = json!({});
    for env in envelopes(result) {
        let p = match get(env, "payload")? {
            v @ Value::Object(_) => v,
            _ => &empty,
        };
        for d in iter_or(get(p, "documents")?)? {
            let Some(o) = d.as_object() else { continue };
            if o.get("kind").and_then(Value::as_str) != Some("intent") {
                continue;
            }
            if let Some(path) = o.get("path").and_then(Value::as_str).map(py::strip).filter(|t| !t.is_empty()) {
                if !paths.iter().any(|x| x == path) {
                    paths.push(path.to_string());
                }
            }
        }
    }
    Ok(paths)
}

/// 這幾個 intent 檔要存在，而且有必要的節。
pub fn intent_file_errors(ctx: &Ctx, result: &Value, paths: &[String], profile: &Value, plan_mode: &Value) -> R<Vec<String>> {
    let epic = result.as_object().and_then(|r| r.get("epic_dir")).and_then(Value::as_str).unwrap_or_default();
    let need = Value::Array(recipe::required_sections(profile, plan_mode).map_err(py::from_recipe)?);
    let mut errs = Vec::new();
    for raw in paths {
        let p = py::pp(raw);
        let mut cands: Vec<String> = Vec::new();
        if !pyish::is_abs(&p) && !epic.is_empty() {
            cands.push(pj(epic, &p));
        }
        cands.push(p);
        let Some(f) = cands.into_iter().find(|c| ctx.is_file(c)) else {
            errs.push(format!("intent 檔不存在：{raw}（相對路徑以 epic_dir {} 為基準）", if epic.is_empty() { "（回傳沒有）" } else { epic }));
            continue;
        };
        let text = match ctx.read_text(&f) {
            Ok(t) => t,
            Err(e) => {
                errs.push(format!("讀不到 intent 檔 {f}：{}", e.msg));
                continue;
            }
        };
        let miss = recipe::missing_sections(&text, &need).map_err(py::from_recipe)?;
        if !miss.is_empty() {
            errs.push(format!("intent 缺必要的節（plan_mode={}）：{}（{raw}）", s(plan_mode), py::join("、", &miss)?));
        }
    }
    Ok(errs)
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[test]
    fn constraint_timeout_defaults_and_bounds() {
        assert_eq!(constraint_timeout(&json!({"id": "a"})), CONSTRAINT_CHECK_TIMEOUT);
        assert_eq!(constraint_timeout(&json!({"timeout": 30})), 30);
        assert_eq!(constraint_timeout(&json!({"timeout": 3600})), 3600);
        // 不合法的值一律退回預設，不會變成不逾時
        for bad in [json!(0), json!(3601), json!(-5), json!(1.5), json!("60"), json!(null)] {
            assert_eq!(constraint_timeout(&json!({"timeout": bad})), CONSTRAINT_CHECK_TIMEOUT);
        }
    }
}
