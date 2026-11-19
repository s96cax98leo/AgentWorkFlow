//! init 訪談：1.0 新增主題的答案、寫入前的對照表、可重跑的答案檔、`--reconfigure`。
//!
//! - 答案檔的新鍵（都選填；沒寫＝不動 profile 的值）：`models`、`scrub`、`knowledge`、`links`、`deliverables`、`ci`、`parallelism`
//!   （物件就深度合併進 profile 的同名鍵；`models` 可以只寫 mode 字串，`scrub`／`ci`／`deliverables`／`links` 可以只寫 true／false）、
//!   `affected_tests_only`（布林）、`auto_merge`（布林）、`guard`（物件，深度合併進 guard.config.json）、`hooks`（plugin／project）。
//!   `knowledge` 原樣合併：`knowledge.auto.*`、`knowledge.repos` 這些鍵照寫，格式由存檔前的 schema 檢查把關。
//! - 對照表：每個設定「偵測到什麼／你的回答／會寫成什麼」。互動模式看完表要確認才寫；`--defaults`、`--answers` 不問但照樣印。
//! - 答案檔：最後採用的答案存成 `.claude/teams/init-answers.json`（像機敏值的字串不存）。
//! - `--reconfigure`：拿存下來的答案當預設重新問一輪（或用 `--answers` 給新的答案），只把「答案有變的設定」寫回
//!   methodology.json 與 guard.config.json；其他鍵（包含你手改過的）不動。

use super::detect::{self, Topic};
use super::merge::{deep_merge, short};
use super::{crash, die, read_text_sig, Console, Kv, R};
use crate::merge_gate::pyjson;
use crate::merge_gate::pyutil::{self, ReadErr};
use crate::pyfmt;
use serde_json::{json, Map, Value};
use std::path::Path;

/// init 存下來的答案（相對專案根目錄）。
pub const ANSWERS_FILE: &str = ".claude/teams/init-answers.json";
/// 物件就深度合併進 profile 同名鍵的答案。
pub const PROFILE_KEYS: &[&str] = &["models", "scrub", "knowledge", "links", "deliverables", "ci", "parallelism"];
pub const MODEL_MODES: &[&str] = &["auto", "fixed", "inherit"];

fn obj_of(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(o) => o,
        _ => Map::new(),
    }
}

/// 偵測值的簡短寫法（問答前的提示用）。
pub fn brief(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "看不出來".to_string(),
        Value::Array(a) if a.iter().all(Value::is_string) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、"),
        other => short(other, 60),
    }
}

/// 答案是不是「會／要」。
pub fn yes(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => ["y", "yes", "true", "1", "會", "要"].contains(&pyutil::strip(s).to_lowercase().as_str()),
        _ => false,
    }
}

/// 把新主題的答案套進 profile。只改回答到的鍵。
pub fn apply_topic_answers(prof: &mut Map<String, Value>, ans: &Map<String, Value>) -> R<()> {
    for key in PROFILE_KEYS {
        let Some(v) = ans.get(*key).filter(|v| !v.is_null()) else { continue };
        let extra: Map<String, Value> = match (*key, v) {
            (_, Value::Object(o)) => o.clone(),
            ("models", Value::String(s)) => {
                if !MODEL_MODES.contains(&s.as_str()) {
                    return die(format!("models 要是 {} 其中之一，或物件（收到 {}）", MODEL_MODES.join("、"), pyfmt::repr_str(s)));
                }
                obj_of(json!({"mode": s}))
            }
            ("links", Value::Bool(false)) => {
                prof.shift_remove("links");
                continue;
            }
            ("links", Value::Bool(true)) => Map::new(),
            ("deliverables", Value::Bool(b)) => {
                if !prof.get("deliverables").is_some_and(Value::is_object) {
                    if *b {
                        return die("deliverables 寫 true 時 profile 要已經有 deliverables（至少 documents）；改給物件，或 init 之後用 awf deliverables configure 設定");
                    }
                    continue;
                }
                obj_of(json!({"enabled": b}))
            }
            ("scrub" | "ci", Value::Bool(b)) => obj_of(json!({"enabled": b})),
            (k, other) => return die(format!("答案檔的 {k} 形狀不對（收到 {}）；格式見 docs/自訂與擴充.md〈init 問答與答案檔〉", pyfmt::repr(other))),
        };
        let base = match prof.get(*key) {
            None => Map::new(),
            Some(Value::Object(o)) => o.clone(),
            Some(_) => return crash(format!("profile 的 {key} 要是 JSON 物件")),
        };
        prof.insert((*key).to_string(), Value::Object(deep_merge(&base, &extra)));
    }
    // 1.2：合併方式（conventions.merge.mode）與「照建議先做」（decisions.auto_accept）。沒答＝不動 profile
    if let Some(v) = ans.get("merge_mode").filter(|v| !v.is_null()) {
        let Some(m) = v.as_str().filter(|m| ["pr", "local"].contains(m)) else { return die(format!("merge_mode 要是 pr 或 local（收到 {}）", pyfmt::repr(v))) };
        if m == "local" && ans.get("auto_merge") == Some(&Value::Bool(true)) {
            return die("merge_mode: local（本機合併）不能和 auto_merge: true 同時設：兩個只能選一個");
        }
        let conv = prof.entry("conventions").or_insert_with(|| json!({}));
        let Some(conv) = conv.as_object_mut() else { return die("profile 的 conventions 要是物件") };
        conv.insert("merge".to_string(), json!({"mode": m}));
    }
    if let Some(v) = ans.get("auto_accept").filter(|v| !v.is_null()) {
        let Some(m) = v.as_str().filter(|m| ["off", "recommended"].contains(m)) else { return die(format!("auto_accept 要是 off 或 recommended（收到 {}）", pyfmt::repr(v))) };
        let dec = prof.entry("decisions").or_insert_with(|| json!({}));
        let Some(dec) = dec.as_object_mut() else { return die("profile 的 decisions 要是物件") };
        dec.insert("auto_accept".to_string(), json!(m));
    }
    if let Some(v) = ans.get("affected_tests_only").filter(|v| !v.is_null()) {
        let Some(b) = v.as_bool() else { return die(format!("affected_tests_only 要是 true 或 false（收到 {}）", pyfmt::repr(v))) };
        let stages = prof.entry("stages").or_insert_with(|| json!({}));
        let Some(build) = stages.as_object_mut().map(|s| s.entry("build").or_insert_with(|| json!({}))).and_then(Value::as_object_mut) else {
            return crash("profile 的 stages.build 要是 JSON 物件");
        };
        build.insert("affected_tests_only".to_string(), json!(b));
    }
    Ok(())
}

/// 答案裡要寫進 guard.config.json 的部分（深度合併進去，答案優先）。`with_main`＝連主 session 的兩個開關一起算
/// （一般的 init 由既有的做法改那兩個開關，這裡不重複；`--reconfigure` 才一起算）。
pub fn guard_overlay(ans: &Map<String, Value>, with_main: bool) -> R<Map<String, Value>> {
    let mut g = match ans.get("guard") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(other) => return die(format!("答案檔的 guard 要是物件（收到 {}）", pyfmt::repr(other))),
    };
    if let Some(v) = ans.get("auto_merge").filter(|v| !v.is_null()) {
        let Some(b) = v.as_bool() else { return die(format!("auto_merge 要是 true 或 false（收到 {}）；細項寫在 guard 的 auto_merge", pyfmt::repr(v))) };
        let am = g.entry("auto_merge").or_insert_with(|| json!({}));
        match am.as_object_mut() {
            Some(o) => {
                o.insert("enabled".to_string(), json!(b));
            }
            None => return die("答案檔的 guard.auto_merge 要是物件"),
        }
    }
    // 主線分支與目錄布局：寫了就要合規則（不合規則的值工具會當成沒寫，這裡先講清楚）
    if let Some((_, why)) = crate::layout::problems(&Value::Object(g.clone())).into_iter().next() {
        return die(format!("答案檔的 guard：{why}"));
    }
    if with_main {
        if let Some(v) = ans.get("main_session_push_merge") {
            let free = yes(v);
            g.insert("main_session_git_rules".to_string(), json!(!free));
            g.insert("main_session_github_rules".to_string(), json!(!free));
        }
    }
    Ok(g)
}

/// 答案檔的 `hooks`。
pub fn hooks_answer(ans: &Map<String, Value>) -> R<Option<String>> {
    match ans.get("hooks") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == "plugin" || s == "project" => Ok(Some(s.clone())),
        Some(other) => die(format!("答案檔的 hooks 只能是 plugin 或 project（收到 {}）", pyfmt::repr(other))),
    }
}

fn starts_y(s: &str) -> bool {
    let t = pyutil::strip(s).to_lowercase();
    t.starts_with('y') || t == "會" || t == "要"
}

fn yn(b: bool) -> &'static str {
    if b { "y" } else { "n" }
}

/// `ask_more` 問的主題（順序就是問的順序）；`--reconfigure <主題>` 可以只重問其中一個（另外加上 `main_session`）。
pub const MORE_TOPICS: &[&str] = &["requirements", "models", "scrub", "links", "ci", "build_loop", "auto_merge", "hooks", "knowledge", "deliverables", "parallelism", "guard"];

/// 可以單獨重問的主題。其餘的（開發規範、慣例、限制文件、讀取預算）題目互相有關，要整輪一起問。
pub fn single_topics() -> Vec<&'static str> {
    let mut v: Vec<&str> = MORE_TOPICS.to_vec();
    v.push("main_session");
    v
}

/// 還沒設定過的主題：答案檔（`init-answers.json`）裡沒有這個主題任何一個鍵的答案。
/// 交付文件另外看專案有沒有 `.claude/teams/deliverables.json`（那一份是用 `awf deliverables configure` 設的，不進答案檔）。
/// 只看「有沒有回答過」，不判斷值好不好；直接手改 methodology.json、沒經過訪談的設定在這裡仍然算還沒設定。
pub fn unset_topics<'a>(target: &Path, topics: &'a [Topic], saved: &Map<String, Value>) -> Vec<&'a Topic> {
    topics.iter().filter(|t| {
        let answered = t.answer_keys.iter().any(|k| saved.get(*k).is_some_and(|v| !v.is_null()));
        let own_file = t.id == "deliverables" && target.join(".claude").join("teams").join("deliverables.json").is_file();
        !(answered || own_file)
    }).collect()
}

/// 守門主題會問、所以互動模式不在佔位符那一輪重複問的佔位符。
pub const GUARD_PLACEHOLDERS: &[&str] = &["PROTECTED_BRANCHES", "DOCS_DIRS", "BASE_BRANCH", "BASE_REMOTE", "SPECS_DIR", "WORKTREES_DIR", "SPEC_TEMPLATES_DIR"];
/// `DOCS_DIRS` 沒有紀錄時的值（同 workflow.config.example 的預設）。
pub const GUARD_DOCS: &[&str] = &["docs"];

/// tpm-writer 可以寫的文件目錄：工作區裡的相對路徑，不能是絕對路徑、含 `..`，也不能是模版設定、CI 或 git 的目錄。
pub fn writer_dir_ok(d: &str) -> bool {
    let p = d.replace('\\', "/");
    let p = p.trim_end_matches('/');
    let first = p.split('/').next().unwrap_or_default().to_lowercase();
    !(p.is_empty() || p.starts_with('/') || p.starts_with('~') || p.contains(':') || p.split('/').any(|s| s == ".." || s == "." || s.is_empty())
        || [".claude", ".github", ".git"].contains(&first.as_str()))
}

/// `knowledge.repos` 的上限（同 methodology.schema.json 的 maxItems）。
const MAX_REPOS: usize = 16;

/// 逗號分隔的清單（去空白；空的與 `-` 不算）。
fn comma_list(s: &str) -> Vec<String> {
    s.split([',', '，']).map(pyutil::strip).filter(|x| !x.is_empty() && *x != "-").map(str::to_string).collect()
}

fn str_list(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

/// 從路徑取一個知識圖用的 repo id（小寫英數與連字號、字母開頭、最多 32 字），和已經用掉的不重複。
pub fn repo_id(path: &str, used: &[String]) -> String {
    let base = path.replace('\\', "/");
    let base = base.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_lowercase();
    let mut id: String = base.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' }).collect();
    id = id.trim_matches('-').to_string();
    if !id.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        id = format!("repo-{id}");
    }
    let id: String = id.trim_matches('-').chars().take(28).collect();
    let id = id.trim_matches('-').to_string();
    let mut out = id.clone();
    let mut n = 2;
    while used.contains(&out) {
        out = format!("{id}-{n}");
        n += 1;
    }
    out
}

/// 偵測到的清單（某個主題的某一項）。
fn found_list(topics: &[Topic], topic: &str, key: &str) -> Vec<String> {
    topics.iter().filter(|t| t.id == topic).flat_map(|t| t.detected.iter()).filter(|f| f.key == key)
        .flat_map(|f| match &f.value {
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>(),
            Value::String(s) => vec![s.clone()],
            _ => Vec::new(),
        }).collect()
}

/// 互動模式：問 1.0 新增的主題（接在既有的題目後面）。預設值的來源：存下來的答案（`ans` 裡已有的）＞偵測＞profile。
/// 回答和預設相同、而且預設來自 profile 時不寫進答案。`only`＝只問這一個主題（`--reconfigure <主題>`）。
pub fn ask_more(con: &mut Console, ans: &mut Map<String, Value>, prof: &Map<String, Value>, topics: &[Topic], vals: &Kv, hooks_given: bool, only: Option<&str>) -> R<()> {
    let sug = detect::suggested(topics);
    let want = |id: &str| only.is_none_or(|o| o == id);
    if only.is_none() {
        con.say("\n== 其他設定（都可以之後改 methodology.json；Enter＝用預設值） ==");
    }
    let at = |v: Option<&Value>, k: &str| v.and_then(|x| x.get(k)).cloned();

    // 需求來源：偵測到規格層才問
    if want("requirements") {
    if let Some(req) = sug.get("requirements").filter(|_| !ans.contains_key("requirements") || only.is_some()) {
        let a = con.ask("偵測到 openspec/：需求來源要不要用規格層（y／n；n＝照 profile）", "y");
        if starts_y(&a) {
            ans.insert("requirements".to_string(), req.clone());
        }
    } else if only.is_some() {
        con.say("  需求來源：沒有偵測到規格層（openspec/）。其他來源（需求 JSON、指令、spec 檔）寫在答案檔的 requirements，或直接改 methodology.json 的 requirements。");
    }
    }

    // 模型分級
    if want("models") {
    let mode_prof = at(prof.get("models"), "mode").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "inherit".to_string());
    let mode_now = match ans.get("models") {
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.get("mode").and_then(Value::as_str).unwrap_or(&mode_prof).to_string(),
        None => mode_prof.clone(),
    };
    let mode = con.ask("模型分級（inherit＝全部沿用主對話模型；auto＝依工作輕重分級，審查與合併有下限；fixed＝只用 profile 的預設等級）", &mode_now);
    if !MODEL_MODES.contains(&mode.as_str()) {
        return die(format!("模型分級要是 {} 其中之一（收到 {}）", MODEL_MODES.join("、"), pyfmt::repr_str(&mode)));
    }
    if mode != mode_prof || ans.contains_key("models") {
        let mut m = match ans.get("models") {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        };
        m.insert("mode".to_string(), json!(mode));
        ans.insert("models".to_string(), Value::Object(m));
    }
    }

    // 去客戶化掃描
    if want("scrub") {
    let scrub_seed = ans.get("scrub").or_else(|| sug.get("scrub")).or_else(|| prof.get("scrub")).cloned();
    let scrub_on = match &scrub_seed {
        Some(Value::Bool(b)) => *b,
        Some(o) => o.get("enabled") == Some(&Value::Bool(true)),
        None => false,
    };
    con.say("  去客戶化掃描（可以不開）：推送前擋掉你指定的字串（客戶名、內部代號）。規則是你自己寫的正規式。");
    let a = con.ask("要不要啟用去客戶化掃描（y／n）", yn(scrub_on));
    if starts_y(&a) {
        let files_now = at(scrub_seed.as_ref(), "terms_files").and_then(|v| v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))).filter(|s| !s.is_empty()).unwrap_or_else(|| ".claude/teams/scrub_terms.txt".to_string());
        let files = con.ask("規則檔位置（逗號分隔；一行一個正規式；不存在的檔會略過並提示）", &files_now);
        let when_now = at(scrub_seed.as_ref(), "when").and_then(|v| v.as_array().map(|a| !a.is_empty())).unwrap_or(true);
        let when = con.ask("要不要在 awf publish 推送前自動掃（y／n；n＝只在手動跑 awf scrub 時掃）", yn(when_now));
        let list: Vec<&str> = files.split(',').map(pyutil::strip).filter(|x| !x.is_empty() && *x != "-").collect();
        ans.insert("scrub".to_string(), json!({"enabled": true, "terms_files": list, "when": if starts_y(&when) { json!(["publish"]) } else { json!([]) }}));
    } else if scrub_on || ans.contains_key("scrub") {
        ans.insert("scrub".to_string(), json!({"enabled": false}));
    }
    }

    // 連結帳本
    if want("links") {
    let links_seed = ans.get("links").cloned().or_else(|| prof.get("links").cloned());
    let links_on = matches!(&links_seed, Some(Value::Bool(true)) | Some(Value::Object(_)));
    let a = con.ask("要不要記連結帳本（每段存檔時記驗收條件 ↔ spec 小節、檔案、測試、commit 的連結並檢查有沒有缺；y／n）", yn(links_on));
    if starts_y(&a) {
        let enforce_now = at(links_seed.as_ref(), "enforce") == Some(Value::Bool(true));
        let e = con.ask("連結不齊時要不要擋政策核准（y／n；n＝只列警告）", yn(enforce_now));
        if !links_on || starts_y(&e) != enforce_now || ans.contains_key("links") {
            ans.insert("links".to_string(), json!({"enforce": starts_y(&e)}));
        }
    } else if links_on {
        ans.insert("links".to_string(), json!(false));
    }
    }

    // 專案的 CI
    if want("ci") {
    let ci_seed = ans.get("ci").or_else(|| sug.get("ci")).or_else(|| prof.get("ci")).cloned();
    let ci_on = match &ci_seed {
        Some(Value::Bool(b)) => *b,
        Some(o) => o.get("enabled") == Some(&Value::Bool(true)),
        None => false,
    };
    let seed_jobs: Vec<Value> = at(ci_seed.as_ref(), "jobs").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    con.say("  專案的測試／lint CI（可以不開）：由 awf ci render 依 profile 產生 GitHub Actions 的檔（只印，--write 才寫）；既有的 CI 檔不會動。");
    for j in &seed_jobs {
        con.say(&format!("    偵測到或已設定的檢查：{} → {}", pyfmt::str_opt(j.get("id")), pyfmt::str_opt(j.get("run"))));
    }
    let a = con.ask("要不要啟用專案的 CI（y／n）", yn(ci_on));
    if starts_y(&a) {
        let run_of = |id: &str| seed_jobs.iter().find(|j| j.get("id").and_then(Value::as_str) == Some(id)).and_then(|j| j.get("run")).and_then(Value::as_str).unwrap_or("-").to_string();
        let mut jobs: Vec<Value> = Vec::new();
        for (id, label, required) in [("test", "測試指令", true), ("lint", "lint 指令", false)] {
            let cmd = con.ask(&format!("{label}（- ＝沒有）"), &run_of(id));
            if !cmd.is_empty() && cmd != "-" {
                jobs.push(json!({"id": id, "name": id, "run": cmd, "required": required}));
            }
        }
        jobs.extend(seed_jobs.iter().filter(|j| !["test", "lint"].contains(&j.get("id").and_then(Value::as_str).unwrap_or_default())).cloned());
        if jobs.is_empty() {
            con.say("  沒有任何檢查指令：CI 先不啟用（之後在 methodology.json 的 ci.jobs 補上再開）。");
        } else {
            let mut ci = match ci_seed {
                Some(Value::Object(o)) => o,
                _ => Map::new(),
            };
            ci.insert("enabled".to_string(), json!(true));
            ci.entry("provider").or_insert_with(|| json!("github"));
            ci.insert("jobs".to_string(), Value::Array(jobs));
            ans.insert("ci".to_string(), Value::Object(ci));
        }
    } else if ci_on || ans.contains_key("ci") {
        ans.insert("ci".to_string(), json!({"enabled": false}));
    }
    }

    // build 修正迴圈
    if want("build_loop") {
    let aff_prof = at(at(prof.get("stages"), "build").as_ref(), "affected_tests_only") == Some(Value::Bool(true));
    let aff_now = ans.get("affected_tests_only").and_then(Value::as_bool).unwrap_or(aff_prof);
    let a = con.ask("build 的修正迴圈要不要只跑受影響的測試（用知識圖算；test 段的閘門照舊全跑；y／n）", yn(aff_now));
    if starts_y(&a) != aff_prof || ans.contains_key("affected_tests_only") {
        ans.insert("affected_tests_only".to_string(), json!(starts_y(&a)));
    }
    }

    // 自動合併
    if want("auto_merge") {
    // 合併方式：只有已經有答案、或使用者改掉預設時才記（預設 pr＝開 PR，照舊）
    let mm_prof = at(at(prof.get("conventions"), "merge").as_ref(), "mode").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "pr".to_string());
    let mm_now = ans.get("merge_mode").and_then(Value::as_str).map(str::to_string).unwrap_or(mm_prof.clone());
    let mm = con.ask("合併方式（pr＝開 PR，由合併 agent 或人在平台上合併；local＝不開 PR，由人自己在本機合併、工具只核對，不能和自動合併同時用）", &mm_now);
    if mm != "pr" && mm != "local" {
        return die(format!("合併方式只能是 pr 或 local（收到 {}）", pyfmt::repr_str(&mm)));
    }
    if mm != mm_prof || ans.contains_key("merge_mode") {
        ans.insert("merge_mode".to_string(), json!(mm));
    }
    let aa_prof = at(prof.get("decisions"), "auto_accept").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "off".to_string());
    let aa_now = ans.get("auto_accept").and_then(Value::as_str).map(str::to_string).unwrap_or(aa_prof.clone());
    let aa = con.ask("blocking 的問題要不要照建議先做（off＝每一題都等人答；recommended＝有建議、標成低風險的先照建議做，下一個閘門一次列出給人確認或推翻；plan 段與高風險一律等人）", &aa_now);
    if aa != "off" && aa != "recommended" {
        return die(format!("照建議先做只能是 off 或 recommended（收到 {}）", pyfmt::repr_str(&aa)));
    }
    if aa != aa_prof || ans.contains_key("auto_accept") {
        ans.insert("auto_accept".to_string(), json!(aa));
    }
    if mm != "local" {
    let am_now = ans.get("auto_merge").and_then(Value::as_bool).unwrap_or(false);
    let a = con.ask("要不要啟用自動合併（y／n；啟用前要先完成 docs/自動合併信任模型.md 的檢查清單，白名單沒填之前不會合併任何 repo）", yn(am_now));
    if starts_y(&a) || ans.contains_key("auto_merge") {
        ans.insert("auto_merge".to_string(), json!(starts_y(&a)));
    }
    }
    }

    // hook 註冊方式（指令列已經指定就不問）
    if !hooks_given && want("hooks") {
        let h_now = ans.get("hooks").or_else(|| sug.get("hooks")).and_then(Value::as_str).unwrap_or("plugin").to_string();
        let h = con.ask("hook 註冊方式（plugin＝由 plugin 註冊；project＝註冊在專案的 settings.json，Windows 建議）", &h_now);
        if h != "plugin" && h != "project" {
            return die(format!("hook 註冊方式只能是 plugin 或 project（收到 {}）", pyfmt::repr_str(&h)));
        }
        if h != "plugin" || ans.contains_key("hooks") {
            ans.insert("hooks".to_string(), json!(h));
        }
    }

    // 知識圖：要不要建、提問時要不要帶位置、別的 repo
    if want("knowledge") {
        let seed = ans.get("knowledge").filter(|v| v.is_object()).cloned();
        let auto_now = |k: &str| at(at(seed.as_ref(), "auto").as_ref(), k).or_else(|| at(at(prof.get("knowledge"), "auto").as_ref(), k)) == Some(Value::Bool(true));
        con.say("  知識圖（可以不建）：把規格、文件、程式的位置連成一張圖（awf kg），資料包與派工單會帶位置。全部是輔助：失敗只警告、不擋流程。");
        let (up_now, pr_now) = (auto_now("update"), auto_now("prompt"));
        let up = starts_y(&con.ask("要不要建知識圖（y＝每段開跑前自動建立與更新；n＝不自動建，你手動跑過 awf kg build 之後才會跟著更新）", yn(up_now)));
        let pr = starts_y(&con.ask("提問時要不要帶位置（訊息提到需求 id、檔名或符號名時查圖，帶幾行位置；y／n）", yn(pr_now)));
        let repos_now: Vec<Value> = at(seed.as_ref(), "repos").or_else(|| at(prof.get("knowledge"), "repos")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let paths_now: Vec<String> = repos_now.iter().filter_map(|r| r.get("path").and_then(Value::as_str)).map(str::to_string).collect();
        let mut cands = found_list(topics, "knowledge", "sibling_repos");
        cands.extend(found_list(topics, "knowledge", "requirements_outside"));
        cands.retain(|c| !paths_now.contains(c));
        if !cands.is_empty() {
            con.say(&format!("    候選（同一層目錄的 git repo、需求來源指到的工作區外路徑；不會自動加）：{}", cands.join("、")));
        }
        let shown = if paths_now.is_empty() { "-".to_string() } else { paths_now.join(",") };
        let paths = comma_list(&con.ask("有沒有別的 repo 要收進知識圖（路徑，逗號分隔；- ＝沒有；收進來的 repo 對 team agent 唯讀，和 linked_repos 重複時也是）", &shown));
        if paths.len() > MAX_REPOS {
            return die(format!("knowledge.repos 最多 {MAX_REPOS} 個（收到 {} 個）", paths.len()));
        }
        let mut repos: Vec<Value> = Vec::new();
        let mut used: Vec<String> = Vec::new();
        let mut added: Vec<usize> = Vec::new();
        for p in &paths {
            if repos.iter().any(|r: &Value| r.get("path").and_then(Value::as_str) == Some(p.as_str())) {
                continue;
            }
            let entry = match repos_now.iter().find(|r| r.get("path").and_then(Value::as_str) == Some(p.as_str())) {
                Some(r) => r.clone(),
                None => {
                    added.push(repos.len());
                    json!({"id": repo_id(p, &used), "path": p, "sources": []})
                }
            };
            used.push(entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string());
            repos.push(entry);
        }
        if !added.is_empty() {
            let globs = comma_list(&con.ask("新加的 repo 要收哪些檔（相對那個 repo 的 glob，逗號分隔；機敏檔不會收）", "**/*.md"));
            if globs.is_empty() {
                return die("別的 repo 至少要有一個要收的 glob（例如 docs/**/*.md）");
            }
            for i in &added {
                if let Some(o) = repos.get_mut(*i).and_then(Value::as_object_mut) {
                    o.insert("sources".to_string(), json!(globs));
                }
            }
        }
        let mut k = seed.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
        let mut auto = k.get("auto").and_then(Value::as_object).cloned().unwrap_or_default();
        if up != up_now {
            auto.insert("update".to_string(), json!(up));
        }
        if pr != pr_now {
            auto.insert("prompt".to_string(), json!(pr));
        }
        if !auto.is_empty() {
            k.insert("auto".to_string(), Value::Object(auto));
        }
        if repos != repos_now {
            k.insert("repos".to_string(), Value::Array(repos));
        }
        if !k.is_empty() {
            ans.insert("knowledge".to_string(), Value::Object(k));
        }
    }

    // 交付文件：只問要不要開（文件清單用 awf deliverables configure）
    if want("deliverables") {
        let own_file = !found_list(topics, "deliverables", "config").is_empty();
        let seed = ans.get("deliverables").cloned();
        let has_docs = prof.get("deliverables").is_some_and(Value::is_object) || seed.as_ref().is_some_and(Value::is_object);
        if own_file {
            con.say("  交付文件：專案已經有 .claude/teams/deliverables.json，以那一份為準（用 /agent-workflow:deliverables 或 awf deliverables configure 調整）。");
        } else if !has_docs {
            con.say("  交付文件：這個 profile 沒有文件清單，之後用 /agent-workflow:deliverables（或 awf deliverables configure）設定與啟用。");
        } else {
            let on_now = match &seed {
                Some(Value::Bool(b)) => *b,
                Some(o) => o.get("enabled") == Some(&Value::Bool(true)),
                None => at(prof.get("deliverables"), "enabled") == Some(Value::Bool(true)),
            };
            con.say("  交付文件（可以不開）：流程照 profile 的文件清單維護給外部看的文件（deliver 段只改受影響的小節）。清單與細項之後用 /agent-workflow:deliverables 調整。");
            let on = starts_y(&con.ask("要不要產交付文件（y／n）", yn(on_now)));
            if on != on_now || seed.is_some() {
                match seed {
                    Some(Value::Object(mut o)) => {
                        o.insert("enabled".to_string(), json!(on));
                        ans.insert("deliverables".to_string(), Value::Object(o));
                    }
                    _ => {
                        ans.insert("deliverables".to_string(), json!(on));
                    }
                }
            }
        }
    }

    // 平行與拆分：兩個門檻
    if want("parallelism") {
        let seed = ans.get("parallelism").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut out = seed.clone();
        for (key, dflt, prompt) in [
            ("split_threshold", 8u64, "預估子任務超過幾個就拆成子計劃平行規劃（整數，至少 2）"),
            ("max_sub_plans", 4u64, "最多拆幾個子計劃（也是平行規劃與分片驗證的 agent 數上限；整數，至少 2）"),
        ] {
            let prof_v = at(prof.get("parallelism"), key).and_then(|v| v.as_u64()).unwrap_or(dflt);
            let now = seed.get(key).and_then(Value::as_u64).unwrap_or(prof_v);
            let a = con.ask(prompt, &now.to_string());
            let Some(n) = pyutil::strip(&a).parse::<u64>().ok().filter(|n| (2..=1000).contains(n)) else {
                return die(format!("{key} 要是 2 以上的整數（收到 {}）", pyfmt::repr_str(&a)));
            };
            if n != prof_v || seed.contains_key(key) {
                out.insert(key.to_string(), json!(n));
            }
        }
        if !out.is_empty() {
            ans.insert("parallelism".to_string(), Value::Object(out));
        }
    }

    // 守門設定：受保護分支、驗收測試目錄、文件目錄（其餘的寫在答案檔的 guard 或直接改 guard.config.json）
    if want("guard") {
        let seed = ans.get("guard").and_then(Value::as_object).cloned().unwrap_or_default();
        let sug_guard = sug.get("guard").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut out = seed.clone();
        // 範本會寫成什麼＝佔位符的值（workflow.config 或 init 記下的）；沒有或留空就是範本原本的預設
        let from_vals = |k: &str, dflt: &[&str]| -> Vec<String> {
            match vals.get(k).map(comma_list) {
                Some(l) if !l.is_empty() => l,
                _ => dflt.iter().map(|s| s.to_string()).collect(),
            }
        };
        for (key, base, prompt) in [
            ("protected_branches", from_vals("PROTECTED_BRANCHES", detect::GUARD_PROTECTED), "受保護分支（逗號分隔；agent 與主 session 都不能直接 push，合併走 PR）"),
            ("acceptance_test_dirs", from_vals("", detect::GUARD_ACCEPTANCE), "驗收測試目錄（逗號分隔；dev-implementer 不能改裡面的檔）"),
        ] {
            let now = str_list(seed.get(key)).or_else(|| str_list(sug_guard.get(key))).unwrap_or_else(|| base.clone());
            let list = comma_list(&con.ask(prompt, &now.join(",")));
            if list.is_empty() {
                return die(format!("{key} 不能是空的（守門設定只能更嚴；要拿掉請由人直接改 guard.config.json）"));
            }
            if list != base || seed.contains_key(key) {
                out.insert(key.to_string(), json!(list));
            }
        }
        // 文件目錄：writer_dirs＝run 目錄＋簡報目錄＋這裡回答的目錄（前兩個一律保留）
        let fixed: Vec<String> = [vals.get("RUNS_DIR").filter(|s| !s.is_empty()).unwrap_or("_tpm/runs"), vals.get("BRIEFS_DIR").unwrap_or_default()]
            .iter().map(|d| d.trim_matches('/').to_string()).filter(|d| !d.is_empty()).collect();
        let base: Vec<String> = match vals.get("DOCS_DIRS") {
            Some(s) => comma_list(s),
            None => GUARD_DOCS.iter().map(|s| s.to_string()).collect(),
        };
        let now = match str_list(seed.get("writer_dirs")) {
            Some(l) => l.into_iter().filter(|d| !fixed.contains(&d.trim_matches('/').to_string())).collect(),
            None => base.clone(),
        };
        let shown = if now.is_empty() { "-".to_string() } else { now.join(",") };
        let list = comma_list(&con.ask("tpm-writer 可以寫的文件目錄（逗號分隔，相對工作區；- ＝沒有；run 目錄與簡報目錄一律可以寫）", &shown));
        if let Some(bad) = list.iter().find(|d| !writer_dir_ok(d)) {
            return die(format!("文件目錄要是工作區裡的相對路徑，不能是絕對路徑、含 ..，也不能在 .claude、.github、.git 底下（收到 {}）", pyfmt::repr_str(bad)));
        }
        if list != base || seed.contains_key("writer_dirs") {
            let mut all = fixed.clone();
            all.extend(list.into_iter().filter(|d| !fixed.contains(&d.trim_matches('/').to_string())));
            out.insert("writer_dirs".to_string(), json!(all));
        }
        // 主線分支：沒帶 base 時各段的比對基準與 PR 的目標分支（remote 用佔位符 BASE_REMOTE 或答案檔的 guard.base_remote）
        {
            let base = vals.get("BASE_BRANCH").map(pyutil::strip).filter(|s| !s.is_empty()).unwrap_or(crate::layout::DEFAULT_BASE_BRANCH).to_string();
            let pick = |m: &Map<String, Value>| m.get("base_branch").and_then(Value::as_str).map(str::to_string);
            let now = pick(&seed).or_else(|| pick(&sug_guard)).unwrap_or_else(|| base.clone());
            let got = pyutil::strip(&con.ask("主線分支（沒帶 base 時各段的比對基準與 PR 的目標分支；只寫分支名，不含 remote）", &now)).to_string();
            if !crate::layout::branch_ok(&got) {
                return die(format!("主線分支只收英數與 . _ / -（收到 {}）", pyfmt::repr_str(&got)));
            }
            if got != base || seed.contains_key("base_branch") {
                out.insert("base_branch".to_string(), json!(got));
            }
        }
        // spec 目錄：每個需求的 intent、spec、plan 放在目標 repo 的 <spec 目錄>/<epic>/（worktree 目錄與範本目錄用佔位符或答案檔的 guard）
        {
            use crate::layout::{dir_norm, dir_ok, DEFAULT_SPECS_DIR, DIR_RULE};
            let base = vals.get("SPECS_DIR").map(|s| dir_norm(s)).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SPECS_DIR.to_string());
            let pick = |m: &Map<String, Value>| m.get("specs_dir").and_then(Value::as_str).map(str::to_string);
            let now = pick(&seed).or_else(|| pick(&sug_guard)).unwrap_or_else(|| base.clone());
            let got = dir_norm(&con.ask("spec 目錄（每個需求的 intent、spec、plan 放在目標 repo 的 <這個目錄>/<epic>/；相對於目標 repo）", &now));
            if !dir_ok(&got) {
                return die(format!("spec 目錄不合規則（{DIR_RULE}；收到 {}）", pyfmt::repr_str(&got)));
            }
            if got != base || seed.contains_key("specs_dir") {
                out.insert("specs_dir".to_string(), json!(got));
                // 範本目錄沒另外指定時跟著 spec 目錄
                let tpl = vals.get("SPEC_TEMPLATES_DIR").map(|s| dir_norm(s)).unwrap_or_default();
                if !seed.contains_key("spec_templates_dir") && (tpl.is_empty() || tpl == format!("{base}/_templates")) && got != base {
                    out.insert("spec_templates_dir".to_string(), json!(format!("{got}/_templates")));
                }
            }
        }
        if !out.is_empty() {
            ans.insert("guard".to_string(), Value::Object(out));
        }
    }
    if only.is_none() {
        con.say("  其餘的細項（知識庫來源 knowledge.sources、交付文件清單、分片驗證與分波、其他守門設定）：寫在答案檔的 knowledge、deliverables、parallelism、guard，或之後直接改設定檔。");
    }
    Ok(())
}

// ------------------------------------------------------------------ 對照表

fn at_path<'a>(doc: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut keys = path.split('.');
    let mut cur = doc.get(keys.next()?)?;
    for k in keys {
        cur = cur.get(k)?;
    }
    Some(cur)
}

fn show(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "（沒有）".to_string(),
        Some(Value::String(s)) if s.chars().count() <= 48 => s.clone(),
        Some(v) => short(v, 48),
    }
}

/// 對照表的一列：（設定, 答案檔的鍵, 寫到哪個檔, 欄位路徑）。
const ROWS: &[(&str, &str, &str, &str)] = &[
    ("開發規範 profile", "methodology", "methodology.json", "name"),
    ("測試先行", "test_first", "methodology.json", "stages.build.test_first"),
    ("code style 檢查指令", "code_style", "methodology.json", "conventions.code_style.command"),
    ("commit 格式", "commit", "methodology.json", "conventions.commit.format"),
    ("commit 檢查 regex", "commit_regex", "methodology.json", "conventions.commit.check_regex"),
    ("分支前綴", "branch_prefix", "methodology.json", "conventions.branch.prefix"),
    ("跟上主線的策略", "sync_strategy", "methodology.json", "conventions.sync.strategy"),
    ("PR 大小上限", "pr_size", "methodology.json", "conventions.pr_size"),
    ("文件格式", "doc_format", "methodology.json", "conventions.doc_format.format"),
    ("需求來源", "requirements", "methodology.json", "requirements.source"),
    ("限制文件", "constraints", "methodology.json", "constraints"),
    ("模型分級", "models", "methodology.json", "models.mode"),
    ("讀取預算", "read_limits", "methodology.json", "limits.mode"),
    ("平行與拆分", "parallelism", "methodology.json", "parallelism"),
    ("去客戶化掃描", "scrub", "methodology.json", "scrub"),
    ("知識庫與知識圖", "knowledge", "methodology.json", "knowledge"),
    ("連結帳本", "links", "methodology.json", "links"),
    ("交付文件", "deliverables", "methodology.json", "deliverables.enabled"),
    ("專案的 CI", "ci", "methodology.json", "ci"),
    ("修正迴圈只跑受影響的測試", "affected_tests_only", "methodology.json", "stages.build.affected_tests_only"),
    ("合併方式", "merge_mode", "methodology.json", "conventions.merge.mode"),
    ("照建議先做", "auto_accept", "methodology.json", "decisions.auto_accept"),
    ("自動合併", "auto_merge", "guard.config.json", "auto_merge.enabled"),
    ("主 session 的 git／GitHub 限制", "main_session_push_merge", "guard.config.json", "main_session_git_rules"),
    ("hook 註冊方式", "hooks", "settings", ""),
    ("守門設定", "guard", "guard.config.json", ""),
];

/// 寫入前的對照表：每個設定「偵測到什麼／你的回答／會寫成什麼」。
/// `guard`＝這次會合併進 guard.config.json 的值；`main_free`＝主 session 的限制要不要放寬；`hooks`＝最後採用的註冊方式。
pub fn table(topics: &[Topic], ans: &Map<String, Value>, prof: &Map<String, Value>, guard: &Map<String, Value>, main_free: bool, hooks: &str) -> Vec<String> {
    let mut out = vec!["== 設定對照表（設定｜偵測到｜你的回答｜會寫成） ==".to_string()];
    for (label, key, file, path) in ROWS {
        let det = detect::detected_for(topics, key);
        let answered = ans.get(*key).filter(|v| !v.is_null());
        let written = match (*file, *key) {
            ("settings", _) => format!("hooks 由 {hooks} 註冊"),
            (_, "main_session_push_merge") => format!("guard.config.json：main_session_git_rules、main_session_github_rules = {}", !main_free),
            (_, "auto_merge") => format!("guard.config.json：auto_merge.enabled = {}", at_path(guard, path).and_then(Value::as_bool).unwrap_or(false)),
            (_, "guard") => {
                if guard.is_empty() { "guard.config.json：用範本的預設".to_string() } else { format!("guard.config.json：合併 {}", short(&Value::Object(guard.clone()), 60)) }
            }
            _ => format!("{file}：{path} = {}", show(at_path(prof, path))),
        };
        // 偵測到、有回答、或值不是空的才列，表才不會被「沒有」塞滿
        let empty = !matches!(*key, "methodology" | "auto_merge" | "main_session_push_merge" | "hooks") && at_path(prof, path).is_none_or(Value::is_null);
        if det.is_empty() && answered.is_none() && empty {
            continue;
        }
        out.push(format!(
            "  {label}｜{}｜{}｜{written}",
            if det.is_empty() { "—".to_string() } else { det },
            match answered {
                Some(v) => show(Some(v)),
                None => "（沒回答，用預設）".to_string(),
            }
        ));
    }
    if let Some(ov) = ans.get("overrides").and_then(Value::as_object).filter(|o| !o.is_empty()) {
        out.push(format!("  其他覆寫（overrides）｜—｜{}｜methodology.json：深度合併", short(&Value::Object(ov.clone()), 48)));
    }
    out
}

// ------------------------------------------------------------------ 答案檔

/// 像機敏值的字串換成空字串（答案檔會進版控）。回被略去的鍵路徑。
fn redact(v: &mut Value, path: &str, hit: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if crate::watch::SECRETISH.is_match(s) {
                hit.push(path.to_string());
                s.clear();
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                redact(x, &format!("{path}[{i}]"), hit);
            }
        }
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                redact(x, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, hit);
            }
        }
        _ => {}
    }
}

/// 把最後採用的答案整理成要存的內容；回（文字, 被略去的鍵）。鍵照字母排序，重跑的 diff 才穩定。
pub fn answers_text(ans: &Map<String, Value>) -> (String, Vec<String>) {
    let mut keys: Vec<&String> = ans.keys().collect();
    keys.sort();
    let mut doc = Map::new();
    for k in keys {
        if let Some(v) = ans.get(k).filter(|v| !v.is_null()) {
            doc.insert(k.clone(), v.clone());
        }
    }
    let mut v = Value::Object(doc);
    let mut hit = Vec::new();
    redact(&mut v, "", &mut hit);
    (format!("{}\n", crate::recipe::dumps(&v, Some(2))), hit)
}

/// 讀存下來的答案；沒有這個檔回 None。
pub fn load_answers(path: &Path) -> R<Option<Map<String, Value>>> {
    let text = match read_text_sig(path) {
        Ok(t) => t,
        Err(ReadErr::Os(_)) if !path.exists() => return Ok(None),
        Err(e) => return crash(format!("讀不了 {}：{}", path.display(), e.msg())),
    };
    match pyjson::loads(&text) {
        Ok(Value::Object(o)) => Ok(Some(o)),
        Ok(_) => die(format!("{} 要是 JSON 物件", path.display())),
        Err(e) => die(format!("{} 不是合法 JSON：{e}", path.display())),
    }
}

// ------------------------------------------------------------------ --reconfigure 的差異

/// 一項變動：欄位路徑與新值（None＝拿掉）。
pub type Change = (Vec<String>, Option<Value>);

/// 兩份設定的差異（物件往下比，陣列與純量整個比）。
pub fn diff_leaves(old: &Map<String, Value>, new: &Map<String, Value>, prefix: &[String], out: &mut Vec<Change>) {
    for (k, nv) in new {
        let mut p = prefix.to_vec();
        p.push(k.clone());
        match (old.get(k), nv) {
            (Some(Value::Object(o)), Value::Object(n)) => diff_leaves(o, n, &p, out),
            (Some(ov), nv) if ov == nv => {}
            _ => out.push((p, Some(nv.clone()))),
        }
    }
    for k in old.keys().filter(|k| !new.contains_key(*k)) {
        let mut p = prefix.to_vec();
        p.push(k.clone());
        out.push((p, None));
    }
}

/// 讀某個路徑現在的值。
pub fn get_path<'a>(doc: &'a Map<String, Value>, path: &[String]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut cur = doc.get(first)?;
    for k in rest {
        cur = cur.get(k)?;
    }
    Some(cur)
}

/// 把一項變動寫進設定：中間的鍵不存在就建；中間的鍵不是物件就不動、回 false。
pub fn apply_change(doc: &mut Map<String, Value>, change: &Change) -> bool {
    let (path, value) = change;
    let Some((last, mids)) = path.split_last() else { return false };
    let mut cur = doc;
    for k in mids {
        if !cur.contains_key(k) {
            if value.is_none() {
                return true; // 要拿掉的本來就沒有
            }
            cur.insert(k.clone(), Value::Object(Map::new()));
        }
        let Some(next) = cur.get_mut(k).and_then(Value::as_object_mut) else { return false };
        cur = next;
    }
    match value {
        Some(v) => {
            cur.insert(last.clone(), v.clone());
        }
        None => {
            cur.shift_remove(last);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(v: Value) -> Map<String, Value> {
        obj_of(v)
    }

    #[test]
    fn topic_answers_merge_and_shorthand() {
        let mut p = m(json!({"knowledge": {"sources": ["docs/**/*.md"]}, "stages": {"build": {"test_first": true}}}));
        let a = m(json!({"models": "auto", "scrub": true, "links": true, "ci": {"enabled": true, "jobs": [{"id": "t", "run": "make test"}]},
            "knowledge": {"auto": {"update": true}, "repos": ["../other"]}, "affected_tests_only": true}));
        assert!(apply_topic_answers(&mut p, &a).is_ok());
        assert_eq!(p["models"], json!({"mode": "auto"}));
        assert_eq!(p["scrub"], json!({"enabled": true}));
        assert_eq!(p["links"], json!({}));
        assert_eq!(p["knowledge"], json!({"sources": ["docs/**/*.md"], "auto": {"update": true}, "repos": ["../other"]}));
        assert_eq!(p["stages"]["build"], json!({"test_first": true, "affected_tests_only": true}));
        assert!(apply_topic_answers(&mut p, &m(json!({"links": false}))).is_ok());
        assert!(!p.contains_key("links"));
        assert!(apply_topic_answers(&mut p, &m(json!({"models": "cheap"}))).is_err());
        assert!(apply_topic_answers(&mut p, &m(json!({"deliverables": true}))).is_err());
        assert!(apply_topic_answers(&mut p, &m(json!({"ci": "yes"}))).is_err());
    }

    #[test]
    fn overlay_and_changes() {
        let g = guard_overlay(&m(json!({"auto_merge": true, "guard": {"protected_branches": ["main", "release"]}, "main_session_push_merge": "會"})), true).unwrap_or_default();
        assert_eq!(Value::Object(g), json!({"protected_branches": ["main", "release"], "auto_merge": {"enabled": true}, "main_session_git_rules": false, "main_session_github_rules": false}));
        assert!(guard_overlay(&m(json!({"auto_merge": "on"})), false).is_err());
        let (old, new) = (m(json!({"a": {"b": 1, "c": [1]}, "d": 1, "e": 2})), m(json!({"a": {"b": 2, "c": [1]}, "d": 1, "f": {"g": 1}})));
        let mut ch = Vec::new();
        diff_leaves(&old, &new, &[], &mut ch);
        assert_eq!(ch.len(), 3, "{ch:?}");
        let mut doc = m(json!({"a": {"b": 1, "mine": true}, "e": 2, "keep": "x"}));
        for c in &ch {
            assert!(apply_change(&mut doc, c));
        }
        assert_eq!(Value::Object(doc), json!({"a": {"b": 2, "mine": true}, "keep": "x", "f": {"g": 1}}));
    }

    #[test]
    fn repo_ids_and_lists() {
        assert_eq!(repo_id("../acme-specs", &[]), "acme-specs");
        assert_eq!(repo_id("/srv/Shared Lib/", &[]), "shared-lib");
        assert_eq!(repo_id("../2024_api", &[]), "repo-2024-api");
        assert_eq!(repo_id("C:\\work\\App", &["app".to_string()]), "app-2");
        assert_eq!(repo_id("..", &[]), "repo");
        let long = repo_id(&format!("../{}", "a".repeat(80)), &[]);
        assert!(long.len() <= 32 && long.starts_with('a'), "{long}");
        assert_eq!(comma_list(" a , -, ,b，c "), vec!["a", "b", "c"]);
        for ok in ["docs", "docs/handbook/", "handbook", "文件"] {
            assert!(writer_dir_ok(ok), "{ok}");
        }
        for bad in ["", "/etc", "~/docs", "C:\\docs", "docs/../x", "..", ".", ".claude/teams", ".GitHub/workflows", ".git", "a//b"] {
            assert!(!writer_dir_ok(bad), "{bad}");
        }
        assert!(single_topics().contains(&"knowledge") && single_topics().contains(&"main_session") && !single_topics().contains(&"conventions"));
    }

    #[test]
    fn answers_text_is_sorted_and_redacted() {
        let (text, hit) = answers_text(&m(json!({"scrub": true, "methodology": "default", "x": null})));
        assert!(text.find("methodology") < text.find("scrub") && !text.contains("\"x\""), "{text}");
        assert!(hit.is_empty());
    }
}
