//! 1.2 補的幾條「合法的路」：直接存檔時核對結果有沒有被改過、build 續跑時略過子任務、本機合併的確認、照建議先做。
//! 全部只會讓判定更嚴或多一條要人出面的路；沒用到時 `awf next`／`done` 的輸出逐字不變。

use super::args::{stage_result, subtasks};
use super::{awf_err, find_wf_meta, has, load_json, opt, or_obj, read_config, s_or, setdefault_obj, stage_args, transcripts_root, Env, A};
use crate::kb::pyish;
use crate::save_run::{self as sr, human as hm, py::{self, pj, R}};
use crate::pyfmt;
use serde_json::{json, Map, Value};

/// 結果裡由 awf 自己加的標記（比對 workflow 回傳時不算內容）。
const MARK_KEYS: [&str; 6] = ["custom_script", "override_script", "manual_output", "edited_output", "flow_steps", "skipped_subtasks"];
/// 上一份結果有、重存時不能默默拿掉的「非標準」標記。
const STICKY_MARKS: [&str; 4] = ["custom_script", "override_script", "manual_output", "edited_output"];

fn simple_name(t: &str) -> bool {
    !t.is_empty() && t != "." && t != ".." && !t.contains(['/', '\\', '\0'])
}

/// workflow 紀錄的 `result`（字串形式的 JSON 先讀進來）；不是物件回 None。
fn meta_result(meta: &Value) -> Option<Value> {
    let r = match meta.get("result")? {
        Value::String(t) => py::loads(t).ok()?,
        other => other.clone(),
    };
    r.is_object().then_some(r)
}

/// 這一段的 workflow 實際回傳的結果：`awf next` 排的 pending 還在時照 `awf done` 的找法（[`find_wf_meta`]）；
/// 已經 `awf done` 過的看那一段記下的 `wf_run`（session 與 run id）。找不到、分不出來、讀不懂都回 None（維持現況）。
fn recorded_result(env: &Env, data: &Value, stage: &str) -> Option<Value> {
    let pend = data.get("awf").and_then(|a| a.get("pending")).filter(|p| p.get("stage").and_then(Value::as_str) == Some(stage));
    if let Some(script) = pend.and_then(|p| p.get("script")).and_then(Value::as_str).filter(|s| !s.is_empty()) {
        let want = if pyish::is_abs(script) { script.to_string() } else { pyish::pjoin(&env.project, script) };
        let since = pend.and_then(|p| p.get("since")).and_then(Value::as_f64).unwrap_or(0.0);
        if since > 0.0 {
            if let Ok(Some((m, _))) = find_wf_meta(env, &want, since) {
                return meta_result(&m);
            }
        }
        return None;
    }
    let wf = data.get("stages")?.get(stage)?.get("wf_run")?;
    let (id, sess) = (wf.get("id")?.as_str()?, wf.get("session")?.as_str()?);
    if !simple_name(id) || !simple_name(sess) {
        return None;
    }
    let dir = pj(&pj(&transcripts_root(env).ok()?, sess), "workflows");
    let mut names: Vec<String> = std::fs::read_dir(&dir).ok()?.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| n.starts_with("wf_") && n.ends_with(".json")).collect();
    names.sort();
    let mut hit: Option<Value> = None;
    for n in names {
        let Ok(Some(meta)) = load_json(env, &pj(&dir, &n)) else { continue };
        let stem = n.strip_suffix(".json").unwrap_or(&n);
        let rid = meta.get("runId").and_then(Value::as_str).filter(|r| !r.is_empty()).unwrap_or(stem);
        if rid == id {
            if hit.is_some() {
                return None; // 同一個 id 有兩筆：不猜
            }
            hit = Some(meta);
        }
    }
    meta_result(&hit?)
}

fn diff_fields(a: &Value, b: &Value) -> Vec<String> {
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return vec!["（整份）".to_string()] };
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).filter(|k| !MARK_KEYS.contains(&k.as_str())).collect();
    keys.sort();
    keys.dedup();
    let mut out = Vec::new();
    for k in keys {
        match (a.get(k), b.get(k)) {
            (Some(x), Some(y)) if pyfmt::eq(x, y) => {}
            (Some(Value::Object(x)), Some(Value::Object(y))) => {
                let mut subs: Vec<&String> = x.keys().chain(y.keys()).collect();
                subs.sort();
                subs.dedup();
                for s in subs {
                    if !matches!((x.get(s), y.get(s)), (Some(p), Some(q)) if pyfmt::eq(p, q)) {
                        out.push(format!("{k}.{s}"));
                    }
                }
            }
            _ => out.push(k.clone()),
        }
    }
    out.truncate(12);
    out
}

/// `awf save-run stage` 直接存一份結果時（不是 `awf done` 取回來的）：
/// - 上一份結果標過非標準（自訂腳本、覆寫檔、手動給的、改過的），這一份沒有標：標記照留（重存不能把非標準變成標準）。
/// - 找得到這一段 workflow 實際回傳的紀錄，而要存的內容和它不一樣：標成 `edited_output`（列出不同的欄位）。
///
/// 只加標記、回傳要印的說明；不讓任何結果存不了。有標記的結果不能政策核准（[`sr::policy_violations`]）。任何讀取失敗都當成沒有紀錄。
pub fn direct_save_marks(ctx: &sr::Ctx, run: &str, stage: &str, data: &Value, result: &mut Value) -> Vec<String> {
    let mut notes = Vec::new();
    if !result.is_object() || !simple_name(stage) {
        return notes;
    }
    if let Ok(Value::Object(old)) = ctx.load_json(&pj(run, &format!("{stage}.json"))) {
        for k in STICKY_MARKS {
            let Some(v) = old.get(k).filter(|v| py::truthy(v)) else { continue };
            if !result.get(k).is_some_and(py::truthy) {
                if let Some(m) = result.as_object_mut() {
                    m.insert(k.to_string(), v.clone());
                }
                notes.push(format!("注意：{stage} 上一份結果標了 {k}（非標準結果），這次直接存檔不會拿掉標記：仍然不能政策核准。要回到標準結果，用 awf next 重跑這一段再 awf done"));
            }
        }
    }
    let known = data.get("awf").and_then(|a| a.get("pending")).is_some() || data.get("stages").and_then(|s| s.get(stage)).and_then(|s| s.get("wf_run")).is_some();
    if !known {
        return notes;
    }
    let mut env = Env::from_env();
    env.project = ctx.abs(&ctx.project_root());
    env.cwd = ctx.cwd.clone();
    if let Some(rec) = recorded_result(&env, data, stage) {
        let diffs = diff_fields(&rec, result);
        if !diffs.is_empty() {
            notes.push(format!(
                "注意：{stage} 要存的結果和 workflow 實際回傳的不一致（不同的欄位：{}）：標成非標準結果，不能政策核准，要由人核准。停在 needs_decision 的段請由人答覆（awf approve … --answer \"#N=<答覆>\"），不要改結果檔",
                diffs.join("、")
            ));
            if let Some(m) = result.as_object_mut() {
                m.insert("edited_output".into(), json!(diffs));
            }
        }
    }
    notes
}

// ------------------------------------------------------------------ build 續跑時略過子任務
fn task_id(t: &Value) -> Option<&str> {
    match t {
        Value::String(s) => Some(s.as_str()),
        other => other.get("id").and_then(Value::as_str),
    }
}

/// `awf next --retry --skip-subtask <id>… --reason <理由> --by <誰>`：把卡住的子任務記成略過（run.json 的 `awf.skipped_subtasks`）。
/// 只在 build 段停下（blocked／needs_decision）之後、由人決定時用；id 要在這個 run 的子任務清單裡、還沒合進整合分支；
/// 還要做的子任務不能依賴被略過的（要一起略過）；略過之後不能一個子任務都不剩。不符合一律拒絕、什麼都不記。
pub fn skip_subtasks(env: &Env, run: &str, data: &mut Value, argv: &[String]) -> A<Vec<String>> {
    let ids = sr::opts(argv, "--skip-subtask");
    let (reason, by) = (opt(argv, "--reason"), opt(argv, "--by"));
    if s_or(py::get(data, "event")?) != "sdlc" || py::truthy(py::get(data, "abandoned")?) || py::truthy(py::get(data, "closed")?) {
        return awf_err("--skip-subtask 只用在進行中的 SDLC run");
    }
    if !has(argv, "--retry") || pyish::strip(reason).is_empty() || pyish::strip(by).is_empty() || by == "policy" || by == hm::AUTO_BY {
        return awf_err("略過子任務要由人決定：awf next --run <run_dir> --retry --skip-subtask <id>… --reason <為什麼略過> --by <誰決定的>（--reason、--by 必填，--by 不能是 policy）");
    }
    let stage = if opt(argv, "--stage").is_empty() { sr::next_stage(data)?.unwrap_or_default() } else { opt(argv, "--stage").to_string() };
    let status = data.get("stages").and_then(|s| s.get("build")).and_then(|s| s.get("status")).and_then(Value::as_str).unwrap_or_default().to_string();
    if stage != "build" || !["blocked", "needs_decision"].contains(&status.as_str()) {
        return awf_err(format!("--skip-subtask 只能在 build 段停下（blocked 或 needs_decision）之後用；現在要跑的是 {}、build 的狀態是 {}", if stage.is_empty() { "（沒有）" } else { &stage }, if status.is_empty() { "（沒有）" } else { &status }));
    }
    let all = subtasks(env, run)?;
    let all: Vec<&Value> = all.as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let known: Vec<&str> = all.iter().filter_map(|t| task_id(t)).collect();
    let prev = stage_result(env, run, "build")?;
    let integrated: Vec<String> = prev.get("integrated").and_then(Value::as_array).map(|l| l.iter().filter_map(|t| task_id(t).map(str::to_string)).collect()).unwrap_or_default();
    let mut skipped = hm::skipped_subtasks(data);
    let mut errs: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new();
    for id in &ids {
        if !known.contains(&id.as_str()) {
            errs.push(format!("沒有 {} 這個子任務（這個 run 的子任務：{}）", crate::hooks::safe::text(id, 60), known.join("、")));
        } else if integrated.contains(id) {
            errs.push(format!("{id} 已經合進整合分支，不能略過"));
        } else if !skipped.contains(id) && !fresh.contains(id) {
            fresh.push(id.clone());
        }
    }
    skipped.extend(fresh.iter().cloned());
    for t in &all {
        let Some(id) = task_id(t) else { continue };
        if skipped.iter().any(|s| s == id) {
            continue;
        }
        let deps: Vec<&str> = t.get("depends_on").and_then(Value::as_array).map(|d| d.iter().filter_map(Value::as_str).filter(|d| skipped.iter().any(|s| s == d)).collect()).unwrap_or_default();
        if !deps.is_empty() {
            errs.push(format!("{id} 依賴被略過的 {}：被依賴的子任務不能單獨略過（要一起略過 {id}）", deps.join("、")));
        }
    }
    if !errs.is_empty() {
        return awf_err(format!("不能略過：{}", errs.join("；")));
    }
    if fresh.is_empty() {
        return Ok(Vec::new());
    }
    let at = env.now_iso();
    {
        let aw = setdefault_obj(data, "awf")?;
        let list = aw.entry("skipped_subtasks".to_string()).or_insert_with(|| json!([]));
        let Value::Array(l) = list else { return awf_err("run.json 的 awf.skipped_subtasks 格式不對") };
        for id in &fresh {
            l.push(json!({"id": id, "by": by, "at": at, "reason": pyish::take(&py::squash(reason), 300)}));
        }
    }
    let (a, _, _) = stage_args(env, run, data, "build", "sdlc-build")?;
    if !a.get("subtasks").and_then(Value::as_array).is_some_and(|l| !l.is_empty()) {
        return awf_err("不能略過：略過之後 build 沒有剩下要做的子任務。整個需求不做了用 awf save-run abandon；要改拆工就重跑 design");
    }
    Ok(vec![format!("已略過子任務 {}（{by}：{}）：這個 run 之後的 build 與後面各段都不能政策核准，推送與合併前要人再確認一次", fresh.join("、"), pyish::take(&py::squash(reason), 120))])
}

/// 這一段的 args 裡拿掉被略過的子任務（`stage_args` 的 build 那一段呼叫）。回傳拿掉了哪幾個。
pub fn drop_skipped(data: &Value, a: &mut Map<String, Value>) -> Vec<String> {
    let skipped = hm::skipped_subtasks(data);
    let mut gone = Vec::new();
    if skipped.is_empty() {
        return gone;
    }
    if let Some(Value::Array(list)) = a.get_mut("subtasks") {
        list.retain(|t| match task_id(t) {
            Some(id) if skipped.iter().any(|s| s == id) => {
                gone.push(id.to_string());
                false
            }
            _ => true,
        });
    }
    gone
}

// ------------------------------------------------------------------ 給後面的段與閘門看的提示
/// 靠答覆往下走的段：`[{stage, by, auto?, id, question, answer}]`。
pub fn decided_list(data: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for (name, st) in data.get("stages").and_then(Value::as_object).into_iter().flatten() {
        let Some(d) = st.get(hm::DECIDED).filter(|d| d.is_object()) else { continue };
        for a in d.get("answers").and_then(Value::as_array).into_iter().flatten() {
            let mut m = Map::new();
            m.insert("stage".into(), json!(name));
            m.insert("by".into(), d.get("by").cloned().unwrap_or(Value::Null));
            if d.get("auto") == Some(&Value::Bool(true)) {
                m.insert("auto".into(), json!(true));
            }
            for k in ["id", "question", "answer"] {
                m.insert(k.into(), a.get(k).cloned().unwrap_or(Value::Null));
            }
            out.push(Value::Object(m));
        }
    }
    out
}

/// 要開跑下一段時的說明：前面有哪幾題是靠答覆往下走的、哪些子任務被略過（沒有就是空的）。
pub fn run_notes(data: &Value) -> Vec<String> {
    let mut notes = Vec::new();
    let dec = decided_list(data);
    if !dec.is_empty() {
        let mut stages: Vec<String> = dec.iter().filter_map(|d| d.get("stage").and_then(Value::as_str).map(str::to_string)).collect();
        stages.dedup();
        notes.push(format!("{} 段有 {} 題 blocking 的問題是靠答覆往下走的：那一段的產出文件還沒反映這些答覆，資料包〈這個 run 的決定與略過〉已經列出，這一段要照答覆做", stages.join("、"), dec.len()));
    }
    let sk = hm::skipped_subtasks(data);
    if !sk.is_empty() {
        notes.push(format!("這個 run 有子任務被略過（{}）：對應的驗收條件不會達成，資料包已經列出", sk.join("、")));
    }
    notes
}

// ------------------------------------------------------------------ 照建議先做（decisions.auto_accept）
/// 這一段停在 needs_decision 而且符合「照建議先做」的條件（[`hm::auto_acceptable`]）時：照建議記答覆、狀態改成 done，
/// 並把每一題記進 `awf.unconfirmed`（`auto: true`），由下一個要人出面的動作一次列出。回傳有沒有做。
/// 不寫進跨 run 的決策記憶；這種段不能政策核准，人確認之前也不能推送、合併。
pub fn try_auto_accept(env: &Env, run: &str, data: &mut Value, stage: &str) -> A<bool> {
    // 沒設定時連 profile 都不經過驗證器（和關閉時逐字相同）
    let raw = crate::layout::read_profile(&pj(&env.teams(), "methodology.json"));
    if raw.get("decisions").and_then(|d| d.get("auto_accept")).and_then(Value::as_str) != Some("recommended") {
        return Ok(false);
    }
    let ctx = env.sr();
    let (prof, perr) = sr::load_profile_for(&ctx, &[]);
    if !perr.is_empty() {
        return Ok(false);
    }
    let result = stage_result(env, run, stage)?;
    let Some(answers) = hm::auto_acceptable(&prof, data, stage, &result)? else { return Ok(false) };
    let at = env.now_iso();
    hm::mark_decided(data, stage, hm::AUTO_BY, &at, answers.clone(), true)?;
    let aw = setdefault_obj(data, "awf")?;
    let Value::Array(l) = aw.entry("unconfirmed".to_string()).or_insert_with(|| json!([])) else { return awf_err("run.json 的 awf.unconfirmed 格式不對") };
    for a in &answers {
        l.push(json!({"stage": stage, "question": a.get("question"), "applied": a.get("answer"), "auto": true, "id": a.get("id")}));
    }
    sr::dump(&ctx, run, data)?;
    Ok(true)
}

// ------------------------------------------------------------------ 本機合併（conventions.merge.mode: local）
/// profile 的 `conventions.merge.mode` 是不是 `local`（讀不到、沒寫、別的值都不是：走原本開 PR 的路）。
pub fn merge_local(env: &Env) -> bool {
    let p = crate::layout::read_profile(&pj(&env.teams(), "methodology.json"));
    p.get("conventions").and_then(|c| c.get("merge")).and_then(|m| m.get("mode")).and_then(Value::as_str) == Some("local")
}

fn git(repo: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(repo).args(args).stdin(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn hexish(t: &str) -> bool {
    (7..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn ref_ok(t: &str) -> bool {
    !t.is_empty() && !t.starts_with('-') && !t.contains("..") && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
}

struct Local {
    repo: String,
    branch: String,
    base: String,
}

fn local_target(env: &Env, run: &str, data: &Value) -> R<Local> {
    let (a, _, _) = stage_args(env, run, data, "deliver", "sdlc-deliver")?;
    let branch = match a.get("branch").filter(|b| py::truthy(b)) {
        Some(b) => py::s(b),
        None => format!("{}{}", read_config(env)?.get("BRANCH_PREFIX").and_then(Value::as_str).unwrap_or_default(), a.get("epic").map(s_or).unwrap_or_default()),
    };
    let repo = a.get("repo").map(py::s).unwrap_or_default();
    let repo = if repo.is_empty() || pyish::is_abs(&repo) { repo } else { pyish::pjoin(&env.project, &repo) };
    let base = crate::layout::run_base_for_project(&env.project, data).unwrap_or_else(|| crate::layout::for_project(&env.project).base_branch);
    Ok(Local { repo, branch, base })
}

/// 本機合併的確定性核對。回傳（每一項的結果, 合併 commit, 審過的 commit）；有任何一項不成立就不算完成。
fn local_checks(env: &Env, run: &str, data: &Value, t: &Local, ack_skipped: bool) -> R<(Vec<(String, bool, String)>, String, String)> {
    let mut out: Vec<(String, bool, String)> = Vec::new();
    let mut put = |name: &str, res: Result<String, String>| {
        let ok = res.is_ok();
        out.push((name.to_string(), ok, res.unwrap_or_else(|e| e)));
        ok
    };
    let ctx = env.sr();
    // 1. 整條閘門鏈到 deliver 都已核准（和推送前的檢查同一個函式，含自訂段、recipe 竄改、政策核准重算）
    put("閘門鏈", match crate::publish::check_policy(&ctx, Some(run), "deliver") {
        Ok(m) => Ok(m),
        Err(crate::publish::Stop::Refused(m) | crate::publish::Stop::Crash(m)) => Err(m),
    });
    // 2. 沒人確認的暫定做法、被略過的子任務
    let holds = hm::run_holds(data, ack_skipped);
    put("待確認事項", if holds.is_empty() { Ok("沒有".to_string()) } else { Err(holds.join("；")) });
    // 3. 凍結旗標、auto_merge 與 local 不能同時設
    put("凍結旗標", match crate::config::load(&env.project) {
        Err(e) => Err(format!("守門設定讀不懂（{}），不採信", e.msg)),
        Ok(cfg) => {
            let am = crate::config::auto_merge(&cfg);
            let ff = if pyish::is_abs(&am.freeze_file) { am.freeze_file.clone() } else { pyish::pjoin(&env.project, &am.freeze_file) };
            if am.enabled {
                Err("guard.config.json 的 auto_merge 已啟用，不能同時用本機合併（conventions.merge.mode: local）".to_string())
            } else if std::path::Path::new(&ff).exists() && !std::fs::read_to_string(&ff).ok().and_then(|x| py::loads(&x).ok()).is_some_and(|v| v.get("frozen") == Some(&Value::Bool(false))) {
                Err(format!("合併已凍結（{}；解凍由人處理）", am.freeze_file))
            } else {
                Ok("沒有凍結".to_string())
            }
        }
    });
    // 4. 審過的 commit 已經由一個合併 commit 合進主線
    let mut found = (String::new(), String::new());
    let located: Result<String, String> = (|| {
        if t.repo.is_empty() || !std::path::Path::new(&t.repo).is_dir() {
            return Err("找不到目標 repo".to_string());
        }
        if !ref_ok(&t.base) || !ref_ok(&t.branch) {
            return Err("主線或分支的名稱不合規則".to_string());
        }
        let want = py::get(&crate::recipe::recipe_of_run(data).map_err(|e| e.msg)?, "stages").map_err(|e| e.msg)?.clone();
        let has_test = py::truthy(&want) && py::contains(&want, "test").unwrap_or(false);
        let (reviewed, from) = if has_test {
            let test = stage_result(env, run, "test").map_err(|e| e.msg)?;
            let c = test.get("verifier").and_then(|v| v.get("payload")).and_then(|p| p.get("commit")).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            if !hexish(&c) {
                return Err("test 段的結果沒有驗證當下的 commit（test.json 的 verifier.payload.commit）".to_string());
            }
            (c, "test 段驗證的 commit")
        } else {
            let c = git(&t.repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}^{{commit}}", t.branch)]).ok_or_else(|| format!("這個 recipe 沒有 test 段，要拿分支 {} 的 head 來對，但分支不在了", t.branch))?;
            (c, "分支的 head（這個 recipe 沒有 test 段）")
        };
        let sha = git(&t.repo, &["rev-parse", "--verify", "--quiet", &format!("{reviewed}^{{commit}}")]).filter(|s| hexish(s)).ok_or_else(|| format!("審過的 commit {reviewed} 不在目標 repo 裡"))?;
        let head = git(&t.repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}^{{commit}}", t.base)]).filter(|s| hexish(s)).ok_or_else(|| format!("目標 repo 沒有本機分支 {}", t.base))?;
        if git(&t.repo, &["merge-base", "--is-ancestor", &sha, &head]).is_none() {
            return Err(format!("審過的 commit {}（{from}）還不在主線 {} 上", pyish::take(&sha, 12), t.base));
        }
        // 主線的 first-parent 鏈上、不是審過的 commit 的祖先的那些 commit（新到舊）；其中最舊的、已經包含審過的 commit 的那一個就是合併 commit
        let chain = git(&t.repo, &["rev-list", "--first-parent", "--max-count=5000", &head, &format!("^{sha}")]).unwrap_or_default();
        let chain: Vec<&str> = chain.lines().filter(|l| hexish(l)).collect();
        let contains = |c: &str| git(&t.repo, &["merge-base", "--is-ancestor", &sha, c]).is_some();
        let (mut lo, mut hi) = (0usize, chain.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if chain.get(mid).is_some_and(|c| contains(c)) { lo = mid + 1 } else { hi = mid }
        }
        let merge = lo.checked_sub(1).and_then(|i| chain.get(i)).map(|c| c.to_string()).ok_or_else(|| format!("主線 {} 上找不到把審過的 commit 合進來的合併 commit（快轉合併沒有合併 commit：請用 git merge --no-ff）", t.base))?;
        let parents = git(&t.repo, &["rev-list", "--parents", "-n", "1", &merge]).unwrap_or_default();
        let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
        if parents.len() < 2 || !parents.iter().skip(1).any(|p| *p == sha) {
            return Err(format!("合併 commit {} 合進來的不是審過的那個 commit（{}）：分支在審查之後又有改動，要重跑 sync 與 test", pyish::take(&merge, 12), pyish::take(&sha, 12)));
        }
        found = (merge.clone(), sha.clone());
        Ok(format!("{} 把 {}（{from}）合進 {}", pyish::take(&merge, 12), pyish::take(&sha, 12), t.base))
    })();
    let merged = put("合併 commit", located);
    // 5. 主線合併前後的 diff 形狀和 recipe 相符
    if merged {
        let names = git(&t.repo, &["diff", "--name-only", "-z", &format!("{}^1", found.0), &found.0]);
        put("diff 形狀", match names {
            None => Err("合併前後的 diff 取不到".to_string()),
            Some(n) => {
                let paths: Vec<String> = n.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect();
                match crate::publish::check_shape(&ctx, run, &paths) {
                    Ok(m) => Ok(m),
                    Err(crate::publish::Stop::Refused(m) | crate::publish::Stop::Crash(m)) => Err(m),
                }
            }
        });
    }
    Ok((out, found.0, found.1))
}

/// merge 段、本機合併：沒帶 `--confirm-merge` 回 `manual`（請人自己合併）；帶了就做確定性核對，
/// 通過＝寫 merge 段的結果（`mode: local`）、附加一筆 `decision: local-merge` 的稽核紀錄、回 None（呼叫端接著算下一步）；
/// 不通過回 `manual` 並列出缺什麼。agent 與主 session 都不合併：這裡只核對人合併的結果。
pub fn local_merge(env: &Env, run: &str, run_arg: &str, data: &mut Value, argv: &[String]) -> A<Option<Value>> {
    let t = local_target(env, run, data)?;
    let show = |x: &str| crate::hooks::safe::text(x, 120);
    let mut act = json!({
        "action": "manual", "stage": "merge", "what": "本機合併（conventions.merge.mode: local）",
        "say": format!(
            "這個專案設定成本機合併、不開 PR：請使用者自己在目標 repo 把 {} 合進 {}（要留合併 commit）。agent 與主 session 都不代為合併；合併好之後由主 session 跑 then 的指令，工具會核對閘門鏈、審過的 commit、diff 形狀與凍結旗標。",
            show(&t.branch), show(&t.base)),
        "merge": format!("git -C {} switch {} && git -C {} merge --no-ff {}", hm_quote(&t.repo), hm_quote(&t.base), hm_quote(&t.repo), hm_quote(&t.branch)),
        "then": format!("awf next --run {run_arg} --confirm-merge --by <誰合併的>"),
    });
    // auto_merge（agent 對 PR 合併）與本機合併是兩條不同的路，同時設＝設定互相矛盾：報錯、不猜
    if crate::config::load(&env.project).is_ok_and(|cfg| crate::config::auto_merge(&cfg).enabled) {
        return awf_err("guard.config.json 的 auto_merge 已啟用，不能同時用本機合併（conventions.merge.mode: local）：兩個只能選一個");
    }
    if !has(argv, "--confirm-merge") {
        return Ok(Some(act));
    }
    let by = opt(argv, "--by");
    if pyish::strip(by).is_empty() || by == "policy" || by == hm::AUTO_BY {
        return awf_err("--confirm-merge 要帶 --by <誰合併的>（本機合併由人做，不能是 policy）");
    }
    let (checks, merge, sha) = local_checks(env, run, data, &t, has(argv, "--confirm-skipped-subtasks"))?;
    let failed: Vec<String> = checks.iter().filter(|c| !c.1).map(|c| format!("{}：{}", c.0, c.2)).collect();
    if !failed.is_empty() {
        let m = py::obj_mut(&mut act)?;
        m.insert("not_done".into(), json!(failed));
        m.insert("reason".into(), json!(format!("本機合併還不算完成：{}", failed.join("；"))));
        return Ok(Some(act));
    }
    let at = env.now_iso();
    let list: Vec<Value> = checks.iter().map(|c| json!({"name": c.0, "ok": c.1, "detail": c.2})).collect();
    let result = json!({
        "stage": "merge", "mode": "local", "merged": true, "merge_state": "merged", "merge_commit": merge, "reviewed_sha": sha, "head_sha": sha,
        "base": t.base, "branch": t.branch, "by": by, "at": at, "checks": list, "skipped_subtasks": hm::skipped_subtasks(data),
        "gate": "本機合併：由人合併，工具只核對（閘門鏈、審過的 commit、diff 形狀、凍結旗標）",
    });
    super::write(&pj(run, "merge.json"), &format!("{}\n", py::dumps(&result, Some(2))))?;
    // 稽核紀錄是合併的證據：寫不進去就不算完成（和 merge gate 一樣）
    let runs = crate::publish::load_config(&env.project).map(|c| c.runs_dir)?;
    let audit = pj(&pyish::pjoin(&env.project, &runs), "merge-audit.jsonl");
    let rec = json!({"timestamp": at, "repo": t.repo, "pr": Value::Null, "head_sha": sha, "decision": "local-merge", "merge_commit": merge, "base": t.base,
        "by": by, "run": py::pname(run), "reasons": checks.iter().map(|c| format!("{}：{}", c.0, c.2)).collect::<Vec<_>>()});
    {
        use std::io::Write;
        pyish::mkdir_p(&py::pparent(&audit))?;
        let line = format!("{}\n", crate::recipe::dumps(&rec, None));
        std::fs::OpenOptions::new().append(true).create(true).open(&audit).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| crate::kb::PyErr::os(&e, &audit))?;
    }
    let st = py::obj_mut(data.get_mut("stages").and_then(|s| s.get_mut("merge")).ok_or_else(|| py::key_err("merge"))?)?;
    st.insert("status".into(), json!("done"));
    st.insert("envelopes".into(), json!(0));
    st.insert("local_merge".into(), json!({"commit": merge, "reviewed_sha": sha, "by": by, "at": at}));
    for k in ["halted", "approved", "unreadable", "wf_run", "missing_records"] {
        st.shift_remove(k);
    }
    if let Some(Value::Object(aw)) = data.get_mut("awf") {
        aw.shift_remove("pending");
    }
    sr::dump(&env.sr(), run, data)?;
    Ok(None)
}

fn hm_quote(t: &str) -> String {
    crate::hooks::safe::sh_quote(t)
}

/// `awf done` 沒有 pending（主 session 沒先跑 `awf next --retry` 就直接重跑了上一次的腳本）時的退路：
/// 只在能確定的時候採用——這一段停在 blocked／needs_decision／invalid、標準腳本 `<run>/.wf/<名>.js` 還在、
/// 上一次存檔之後用那支腳本跑的紀錄恰好一筆（已經存過的那一筆不算）。回傳（腳本路徑, 起算時間, 已經存過的 run id）；其餘回 None（照舊交人）。
pub fn done_fallback(run: &str, data: &Value, stage: &str, workflow: &str) -> Option<(String, f64, String)> {
    let st = data.get("stages")?.get(stage)?;
    if !["blocked", "needs_decision", "invalid"].contains(&st.get("status")?.as_str()?) || !simple_name(workflow) {
        return None;
    }
    let script = pj(run, &format!(".wf/{workflow}.js"));
    if !super::is_file(&script) {
        return None;
    }
    let saved = std::fs::metadata(pj(run, &format!("{stage}.json"))).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
    let seen = or_obj(st.get("wf_run").unwrap_or(&Value::Null)).get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    (saved > 0.0).then_some((script, saved, seen))
}
