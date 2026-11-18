//! 狀態機：主 session 每一段只做兩件事——`awf next`（照它說的做）與 `awf done`（把結果存起來）。
//! 原本是 0.x 的 Python 工具 `awf.py`：動作的 JSON、訊息文字、exit code、寫出的檔
//! （run.json、`<run>/.wf/<名>.js`、`接手摘要.md`、資料包）都和 0.x 的 Python 版逐字相同
//! （對跑基準在 `tests/parity/state.expected.json`）。訊息與動作裡的指令已改成 `awf <子指令>`，基準檔的對應字串一起換過。
//!
//! 這支只排順序、組參數、取時間、產生腳本，**不放寬任何閘門**：存檔與驗證仍然是 [`crate::save_run::stage`]，
//! 核准仍然是 [`crate::save_run::approve`]。分檔：
//! - `plugin`：找 plugin、讀 workflow.config、找範本、產生腳本（含自訂腳本的組裝）。
//! - `args`：`--set`、每一段的 args、資料包、交付文件同步、找 workflow 的回傳。
//! - `machine`：下一個動作（decide）、閘門重點、政策核准、自訂腳本、接手摘要。
//! - `auto`：自動接點（每段開跑前更新知識圖、merge 之後併回規格、結案後重寫 vault、中斷回收）；全部是輔助，失敗只警告。
//! - `selftest`：`awf state --selftest`。
//!
//! 和 0.x 的 Python 版的差別（都不放寬判定；對跑測試的 `STRICTER`、`KNOWN` 逐項列名）：
//! - 專案根目錄照全部子指令共用的解析（[`crate::project`]：`CLAUDE_PROJECT_DIR` → 從目前目錄往上找最外層的 `.claude/teams/VERSION`；
//!   0.x 的 Python 版從工具檔的位置往上推）。找不到時 `done`、`approve` 報錯，其他動作用目前目錄。
//! - 「模版自己」：[`crate::template::find`]（`AWF_TEMPLATE_DIR` → plugin 的環境變數 → 從執行檔往上找到 plugin 目錄）。
//! - 時間可以用環境變數 `AWF_NOW`（`YYYY-MM-DDTHH:MM:SSZ` 或 epoch 秒數）固定，方便對跑。
//! - 讀進來的 JSON 有 NaN、Infinity、落單的代理字元跳脫、超出 64 位元範圍的整數時當成讀不懂；巢狀超過 200 層當成沒接住的例外。
//! - 0.x 的 Python 版丟出沒接住的例外的地方（traceback、exit 1），這裡印「awf：類別：訊息」、exit 1。
//! - 用法訊息的指令名是 `awf`。

mod args;
pub mod auto;
mod human;
mod machine;
mod plugin;
mod selftest;

pub use crate::kb::PyErr;
pub use args::{apply_sets, current_session, deliverables_sync, find_wf_meta, run_pack, stage_args, transcripts_root, unreadable_wf_meta};
pub use machine::{carry, custom_action, decide, envelopes, flow_action, gate_action, halted_action, handoff_text, highlights, is_flow, provisional, text_of, try_policy, workflow_action};
pub(crate) use plugin::{override_re, unregistered, version as plugin_version};
pub use plugin::{custom_roles, find_plugin, find_template, plugin_candidates, read_config, render, render_custom, render_flow, workflow_names, Candidate};
pub use human::direct_save_marks;
pub use selftest::selftest;

use crate::kb::pyish;
use crate::save_run::{self as sr, py};
use serde_json::{json, Map, Value};

pub const PLUGIN_NAME: &str = "agent-workflow";
pub const TEMPLATES: &str = "workflow-templates";
pub const ARGS_LINE: &str = "const A = args || {}";
/// event.js 內建的事件類型與各自的必要參數（要和範本的 BUILTIN_EVENTS 一致，selftest 會對照）。
pub const EVENT_TYPES: [(&str, &[&str]); 5] =
    [("meeting", &["source"]), ("decision", &["decision"]), ("demo-check", &["target"]), ("reqs-change", &["change"]), ("upstream", &["what", "repo"])];
/// 不是 event.js 的事件 →（workflow, 段名）。
const FIXED_EVENTS: [(&str, &str, &str); 4] =
    [("brief", "event-brief", "brief"), ("pr-review", "event-pr-review", "pr-review"), ("post-merge", "event-post-merge", "post-merge"), ("merge", "sdlc-merge", "merge")];
/// 每支 workflow 的必要參數（event.js 另外要該事件類型的參數）。
pub const REQUIRED: [(&str, &[&str]); 13] = [
    ("sdlc-plan", &["repo", "epic", "requirement", "run_dir"]),
    ("sdlc-design", &["repo", "epic", "run_dir"]),
    ("sdlc-design-revise", &["repo", "epic", "run_dir", "decisions", "issues"]),
    ("sdlc-build", &["repo", "epic", "run_dir", "subtasks"]),
    ("sdlc-sync", &["repo", "epic", "run_dir"]),
    ("sdlc-test", &["repo", "epic", "run_dir"]),
    ("sdlc-deliver", &["repo", "epic", "run_dir"]),
    ("sdlc-merge", &["repo", "run_dir", "pr"]),
    ("event-brief", &["brief", "run_dir"]),
    ("event-post-merge", &["repo", "run_dir"]),
    ("event-pr-review", &["target", "repo", "run_dir"]),
    ("event", &["event", "run_dir"]),
    // 用步驟宣告的自訂段（profile 的 flow.stages[].run 是 steps）；不是事件，由狀態機在走到那一段時產生
    ("flow-steps", &["stage", "run_dir"]),
];
pub const HINTS: [(&str, &str); 15] = [
    ("repo", "目標 repo（我方的本機 clone）"),
    ("epic", "這個需求的短名（specs/<epic>/、分支名都用它）"),
    ("requirement", "需求原文或檔案路徑"),
    ("pr", "PR 編號：先經使用者同意推 fork、開 PR（awf publish），再用 --set pr=<號> 補上"),
    ("has_ui", "這次改動有沒有畫面（true／false；依 spec 判斷）"),
    ("has_security", "這次改動有沒有安全面（true／false；依 spec 判斷）"),
    ("decisions", "使用者對退回的 design 做的決定（--set-json decisions='[\"…\"]'）"),
    ("issues", "要修的問題（--set-json issues='[{\"target_id\":\"…\",\"reason\":\"…\"}]'）"),
    ("subtasks", "子任務清單：design（或 plan）段的結果裡沒有，確認那一段已經存檔"),
    ("brief", "簡報檔路徑"),
    ("source", "會議紀錄或逐字稿的路徑"),
    ("decision", "決策內容"),
    ("target", "要處理的對象（PR、分支或環境）"),
    ("what", "上游發生了什麼（例如 PR 編號、main 前進、CI 失敗）"),
    ("change", "需求改了什麼"),
];
pub const MERGED_HINT: &str = "0.5 起會議、決策落地、demo 前檢查、需求變動、上游變動共用 event.js（事件類型由 awf start --event <類型> 決定）。";
pub const HANDOFF: &str = "接手摘要.md";

pub const USAGE: &str = "狀態機：主 session 每一段只做兩件事——`awf next`（照它說的做）與 `awf done`（把結果存起來）。

用法（在已 init 的專案根目錄）：
    awf start --run <run_dir> --event <sdlc|brief|upstream|…> [--repo <repo>] [--epic <短名>]
        [--requirement <需求原文或路徑>] [--base <base>] [--recipe <名>] [--modifiers a,b] [--size s] [--set k=v]… [--set-json k=<JSON>]…
    awf next --run <run_dir> [--set k=v]… [--set-json k=<JSON>]… [--stage <段>] [--workflow <名>] [--text]
    awf next --run <run_dir> --script <自己寫的腳本.js> [--stage <段> --retry]   # design、build、deliver 段改用自訂腳本（先檢查再接上執行核心）
    awf next --run <run_dir> --skip spec-apply   # merge 之後「把規格差異併回現行規格」這一步先不做，之後不再提
    awf next --run <run_dir> --retry --skip-subtask <id>… --reason <理由> --by <誰>   # build 停下之後由人決定略過卡住的子任務（之後不能政策核准）
    awf next --run <run_dir> --confirm-merge --by <誰> [--confirm-skipped-subtasks]   # 本機合併（conventions.merge.mode: local）：人合併之後由工具核對
    awf done --run <run_dir> [--output <workflow 回傳的 JSON 檔>] [--stage <段>]
    awf approve --run <run_dir> [<段>] --by <誰> [--note <備註>] [--answer \"<問題>=<答覆>\"]… [--accept-defaults]
        停在 needs_decision 的段：每一題 blocking 的問題都用 --answer \"#N=<答覆>\" 答了就視為人工核准、往下走（不必重跑；之後不能政策核准）
    awf approve --run <run_dir> [<段>] --by <誰> --confirm-unconfirmed          # 人確認了全部暫定做法（可以單獨跑，不核准任何一段）
    awf approve --run <run_dir> --reject-unconfirmed <段> --by <誰>            # 推翻那一段照建議先做的答覆：回到 needs_decision
    awf handoff --run <run_dir> [--print]      # 寫 <run_dir>/接手摘要.md：換 session 或換人接手時看
    awf watch --run <run_dir> [--once [--json]] [--interval 5] [--stall-seconds 900]   # 進度事件寫到 <run_dir>/progress.jsonl
    awf statusline                             # 一行狀態，給 Claude Code 的 statusLine 用
    awf doctor [--json]                        # 檢查安裝：環境、plugin 與專案版本、安裝來源衝突、hook、設定
    awf render <workflow>|--all --out <目錄>
    awf state --selftest
    共用選項：--plugin-dir <plugin 目錄>（找不到 plugin 時才需要）

next 回一個動作（JSON；--text 是給人看的摘要）：
    run_workflow  用 Workflow 工具執行 script（args 已經內嵌在腳本裡，不用再傳），完成後跑 then 的指令。
                  有 resume_from 時把它當 resumeFromRunId 傳給 Workflow：已完成而且派工單沒變的 agent 不會重跑。
    gate          這一段等人核准：把 highlights 列給使用者看，核准後跑 approve 的指令；退回就照使用者的意見處理。
    need_input    缺參數：照 need 的說明問使用者或自己判斷，用 next --set 補上。
    manual        要人當下同意、由主 session 自己做的步驟（例如推 fork、開 PR）。
    halted        這一段停下了：reason 與 advice 是原因與建議；處理完跑 retry 的指令（awf next --retry）重新備腳本再跑，不要直接重跑舊腳本。
                  有 answer 時（只差 blocking 的問題沒答）：人答覆後跑 answer 的指令就能往下走，不必重跑。
    done          這個 run 沒有下一步了。

workflow 腳本從哪來：plugin 的 workflow-templates/<名>.js（範本），加上專案的佔位符值（.claude/teams/workflow.config）
與開發規範 profile（.claude/teams/methodology.json），產生 <run_dir>/.wf/<名>.js。
專案的 .claude/workflows/<名>.js 結尾有「// awf-override: <版本>」時當成覆寫（用它、不用 plugin 的範本；版本和 plugin 不同會警告）；
沒有這個標記的同名檔（0.4 以前 init 複製的副本）不採用。

done 會自己找 workflow 的回傳（Claude Code 存在 session 的 workflows/wf_<id>.json），找不到時用 --output 指定檔案。
存檔與驗證仍然是 save-run stage，核准仍然是 save-run approve：這支只是幫忙排順序、組參數、取時間，不放寬任何閘門。
plan 段的核准一律由人。recipe 的 gate_mode 是 plan-only（內建：docs、test-only；帶任何 modifier 就不算）時，非 plan 的閘門先試政策核准
（save-run approve --by policy，條件完全由它判）；不乾淨就照常交人。其他 recipe（strict）每個閘門都等人。
";

/// 失敗：`Awf` 是 0.x 的 Python 版的 AwfError（印「awf：訊息」、exit 1）；`Py` 是 0.x 的 Python 版沒接住的例外（那邊是 traceback、exit 1）。
#[derive(Debug, Clone)]
pub enum Fail {
    Awf(String),
    Py(PyErr),
}

impl From<PyErr> for Fail {
    fn from(e: PyErr) -> Self {
        Fail::Py(e)
    }
}

impl From<crate::recipe::PyErr> for Fail {
    fn from(e: crate::recipe::PyErr) -> Self {
        Fail::Py(py::from_recipe(e))
    }
}

pub type A<T> = Result<T, Fail>;

pub(crate) fn awf_err<T>(msg: impl Into<String>) -> A<T> {
    Err(Fail::Awf(msg.into()))
}

/// 執行環境：所有路徑與時間都明確給（測試與 selftest 可以整組換掉），核心函式不直接讀行程的環境變數。
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// 專案根目錄（絕對路徑）。
    pub project: String,
    /// 相對路徑的基準（0.x 的 Python 版的目前目錄）：`--out`、`--output`、`--plugin-dir`。
    pub cwd: String,
    /// Claude Code 的設定目錄（`CLAUDE_CONFIG_DIR`，沒有就是 `~/.claude`）：找 `plugins/installed_plugins.json`。
    pub home: String,
    /// `AWF_PLUGIN_DIR`、`CLAUDE_PLUGIN_ROOT`（依序）。
    pub plugin_env: [Option<String>; 2],
    /// 模版根目錄＝plugin 目錄（底下有 `scaffold/` 與 `workflow-templates/`，見 [`crate::template`]）：
    /// 執行檔自己所在的 plugin（或在模版 repo 裡直接跑時 repo 的 `plugin/`）。
    pub template: Option<String>,
    /// workflow 紀錄的根目錄（`AWF_TRANSCRIPTS_DIR`）；None 用 Claude Code 的專案目錄（`~/.claude/projects/<專案>`）。
    pub transcripts: Option<String>,
    /// 固定「現在」（epoch 秒數）；None 用系統時間。
    pub now: Option<f64>,
    /// 固定「今天」（YYYY-MM-DD；決策記憶與資料包判斷哪些決定過期）；None 用本機日期。
    pub today: Option<String>,
}

/// `YYYY-MM-DDTHH:MM:SSZ` 或 epoch 秒數 → epoch 秒數。
fn parse_now(raw: &str) -> Option<f64> {
    let t = raw.trim();
    if let Ok(f) = t.parse::<f64>() {
        return f.is_finite().then_some(f);
    }
    let b = t.as_bytes();
    if b.len() != 20 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || b.get(10) != Some(&b'T') || b.get(13) != Some(&b':') || b.get(16) != Some(&b':') || b.get(19) != Some(&b'Z') {
        return None;
    }
    let num = |a: usize, z: usize| t.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (y, m, d, hh, mm, ss) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    // days_from_civil
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hh * 3600 + mm * 60 + ss) as f64)
}

impl Env {
    /// 命令列用：從行程的環境收集。
    pub fn from_env() -> Env {
        let var = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        let cwd = std::env::current_dir().ok().and_then(|p| p.to_str().map(|t| t.replace('\\', "/"))).unwrap_or_else(|| ".".to_string());
        let home = var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| {
            let h = var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| h.replace('\\', "/")).unwrap_or_else(|| "/".to_string());
            pyish::pjoin(&h, ".claude")
        });
        let now = var("AWF_NOW").and_then(|v| parse_now(&v));
        let today = now.map(|t| {
            let (y, m, d) = crate::hooks::py::civil_from_days((t.floor() as i64).div_euclid(86_400));
            format!("{y:04}-{m:02}-{d:02}")
        });
        Env {
            project: crate::deliverables::default_root(),
            cwd,
            home,
            plugin_env: [var("AWF_PLUGIN_DIR"), var("CLAUDE_PLUGIN_ROOT")],
            template: crate::template::find_str(),
            transcripts: var("AWF_TRANSCRIPTS_DIR"),
            now,
            today,
        }
    }

    /// 現在（epoch 秒數，到微秒；同 `datetime.now().timestamp()`）。
    pub fn now_ts(&self) -> f64 {
        self.now.unwrap_or_else(|| {
            let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            d.as_micros() as f64 / 1e6
        })
    }

    /// `datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")`
    pub fn now_iso(&self) -> String {
        let t = self.now_ts().floor() as i64;
        let (y, m, d) = crate::hooks::py::civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400);
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, s / 3600, s % 3600 / 60, s % 60)
    }

    /// 相對路徑接上目前目錄（只用來碰檔案系統）。
    pub fn abs(&self, p: &str) -> String {
        if pyish::is_abs(p) { p.to_string() } else { pyish::pjoin(&self.cwd, p) }
    }

    /// 專案的 `.claude/teams`。
    pub fn teams(&self) -> String {
        pyish::pjoin(&self.project, ".claude/teams")
    }

    /// 閘門工具（save_run）用的環境：同一個專案、同一個基準目錄、同一個「今天」。
    pub fn sr(&self) -> sr::Ctx {
        let mut ctx = sr::Ctx::new(&self.project, &self.cwd);
        ctx.today = self.today.clone();
        ctx
    }

    /// 模版自己的 plugin 目錄（有 workflow-templates/ 才算）。
    pub(crate) fn template_plugin(&self) -> Option<String> {
        let p = self.template.clone()?;
        is_dir(&pyish::pjoin(&p, TEMPLATES)).then_some(p)
    }
}

// ------------------------------------------------------------------ 小工具
pub(crate) fn is_file(p: &str) -> bool {
    !p.contains('\0') && std::path::Path::new(p).is_file()
}

pub(crate) fn is_dir(p: &str) -> bool {
    !p.contains('\0') && std::path::Path::new(p).is_dir()
}

/// `opt(argv, name)`：第一個 `name` 後面那個參數，沒有回空字串。
pub fn opt<'a>(argv: &'a [String], name: &str) -> &'a str {
    sr::opt(argv, name, "")
}

pub(crate) fn has(argv: &[String], name: &str) -> bool {
    argv.iter().any(|a| a == name)
}

/// `load_json(p)`：讀不到、不是 utf-8、不是看得懂的 JSON 回 None；巢狀太深是沒接住的例外。
pub(crate) fn load_json(env: &Env, p: &str) -> py::R<Option<Value>> {
    match env.sr().load_json(p) {
        Ok(v) => Ok(Some(v)),
        Err(e) if py::os_or_value(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `load_json(p) or {}`
pub(crate) fn load_json_or(env: &Env, p: &str) -> py::R<Value> {
    Ok(load_json(env, p)?.filter(py::truthy).unwrap_or_else(|| json!({})))
}

/// `_write(path, text)`：建上層目錄、不轉換行。
pub(crate) fn write(path: &str, text: &str) -> py::R<()> {
    pyish::mkdir_p(&py::pparent(path))?;
    std::fs::write(path, text.as_bytes()).map_err(|e| PyErr::os(&e, path))
}

/// `d.setdefault(k, {})`，回傳可以改的物件；`d` 不是物件是 AttributeError，原本的值不是物件時照 Python 對它做 `x[k] = v` 會丟的例外。
pub(crate) fn setdefault_obj<'a>(d: &'a mut Value, k: &str) -> py::R<&'a mut Map<String, Value>> {
    if !d.is_object() {
        return Err(py::attr_err(d, "setdefault"));
    }
    let m = py::obj_mut(d)?;
    py::obj_mut(m.entry(k.to_string()).or_insert_with(|| json!({})))
}

/// `(v or {})`
pub(crate) fn or_obj(v: &Value) -> Value {
    if py::truthy(v) { v.clone() } else { json!({}) }
}

/// `str(v or "")`
pub(crate) fn s_or(v: &Value) -> String {
    py::s_or(v, "")
}

fn fixed_event(event: &str) -> Option<(&'static str, &'static str)> {
    FIXED_EVENTS.iter().find(|(e, _, _)| *e == event).map(|(_, w, s)| (*w, *s))
}

fn builtin_type(event: &str) -> Option<&'static [&'static str]> {
    EVENT_TYPES.iter().find(|(e, _)| *e == event).map(|(_, a)| *a)
}

/// `EVENTS.get(event)`
fn events_get(event: &str) -> Option<(String, String)> {
    fixed_event(event).map(|(w, s)| (w.to_string(), s.to_string())).or_else(|| builtin_type(event).map(|_| ("event".to_string(), event.to_string())))
}

pub(crate) fn required_of(workflow: &str) -> Vec<String> {
    REQUIRED.iter().find(|(w, _)| *w == workflow).map_or_else(|| vec!["run_dir".to_string()], |(_, a)| a.iter().map(|x| x.to_string()).collect())
}

pub(crate) fn hint(k: &str) -> &'static str {
    HINTS.iter().find(|(h, _)| *h == k).map_or("這一段需要的參數", |(_, t)| *t)
}

pub(crate) fn stage_workflow(stage: &str) -> String {
    crate::stages::template_of(stage).map_or_else(|| format!("sdlc-{stage}"), str::to_string)
}

/// profile（methodology.json）的 events：使用者自訂的事件類型。讀不懂就當沒有。
pub fn profile_events(env: &Env) -> py::R<Map<String, Value>> {
    let file = pyish::pjoin(&env.teams(), "methodology.json");
    // 繼承（extends）解析失敗照「讀不懂」當沒有（產生腳本時 plugin::methodology 會報錯停下）
    let m = load_json(env, &file)?.and_then(|v| crate::profile::resolve_str(&file, v).ok());
    let mut out = Map::new();
    if let Some(Value::Object(ev)) = m.as_ref().and_then(|m| m.as_object()).and_then(|m| m.get("events")) {
        for (k, v) in ev {
            if v.is_object() {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(out)
}

/// 事件 →（workflow, 段名）。固定的事件與內建類型查表；其餘看 profile 的 events（走 event.js）。
pub fn event_route(env: &Env, event: &str) -> py::R<Option<(String, String)>> {
    let custom = profile_events(env)?;
    let known = events_get(event);
    if known.is_some() && !custom.contains_key(event) {
        return Ok(known);
    }
    // 自訂類型不能蓋掉 brief、merge 這些不是 event.js 的事件
    if custom.contains_key(event) && (builtin_type(event).is_some() || known.is_none()) {
        return Ok(Some(("event".to_string(), event.to_string())));
    }
    Ok(known)
}

/// event.js 這個事件類型的必要參數：profile 有定義就照它的 args，否則用內建的。
pub fn event_required(env: &Env, event: &str) -> py::R<Vec<String>> {
    if let Some(d) = profile_events(env)?.get(event) {
        // extends：沒有自己寫 args 時用底（內建類型）的必要參數；有寫就整個取代（陣列不合併，和範本的合併規則一致）
        if d.get("args").is_none() {
            if let Some(base) = d.get("extends").and_then(Value::as_str).and_then(builtin_type) {
                return Ok(base.iter().map(|x| x.to_string()).collect());
            }
        }
        let a = py::get(d, "args")?;
        return Ok(py::iter_or(a)?.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
    }
    Ok(builtin_type(event).map(|a| a.iter().map(|x| x.to_string()).collect()).unwrap_or_default())
}

/// `emit(act, argv)`
fn emit(act: &Value, argv: &[String], out: &mut String) -> py::R<()> {
    let text = if has(argv, "--text") { text_of(act)? } else { py::dumps(act, Some(1)) };
    out.push_str(&text);
    out.push('\n');
    Ok(())
}

/// `_load_run(run)`
fn load_run(env: &Env, run: &str) -> A<Value> {
    let file = py::pj(run, "run.json");
    if !is_file(&file) {
        return awf_err(format!("找不到 {file}（先用 awf start 建立）"));
    }
    Ok(sr::load(&env.sr(), run)?)
}

/// 把 `save_run stage` 的 stdout 與 stderr 接回 0.x 的 Python 版印出來的先後順序（那邊兩個串流導到同一個緩衝區）。
/// stage 的輸出順序是固定的：[stderr] halted、限制文件、拆工檢查 → [stdout] 執行紀錄索引、代寫 → [stderr] 紀錄缺件、錯誤、警告
/// → [stdout] 其餘。前面那一組 stderr 的內容可以從存檔後的 run.json 逐字重組；對不上就不拆（stderr 全部放在索引那一行前面）。
fn merge_stage_output(stage: &str, st: &Value, out: &str, err: &str) -> String {
    let index_head = format!("{stage}：執行紀錄索引 ");
    if !out.starts_with(&index_head) {
        return format!("{err}{out}");
    }
    let lines_of = |k: &str| -> Vec<String> { st.get(k).and_then(Value::as_array).map(|a| a.iter().map(py::s).collect()).unwrap_or_default() };
    let mut head_out = out.find('\n').map_or(out.len(), |i| i + 1);
    let ghost = lines_of("ghostwritten_records");
    if !ghost.is_empty() {
        let line = format!("{stage}：{} 個 agent 有交件但沒寫紀錄檔，已依信封代寫（內容較簡略）：{}\n", ghost.len(), ghost.iter().take(8).cloned().collect::<Vec<_>>().join("、"));
        if out.get(head_out..).is_some_and(|rest| rest.starts_with(&line)) {
            head_out += line.len();
        }
    }
    let mut first = String::new();
    if let Some(h) = st.get("halted").filter(|h| py::truthy(h)) {
        first.push_str(&format!("{stage}：workflow 中斷（halted）：{}\n", py::s(h)));
    }
    if sr::SDLC.contains(&stage) {
        for m in lines_of("constraint_issues") {
            first.push_str(&format!("限制文件：{m}（不能政策核准）\n"));
        }
        for m in lines_of("plan_check") {
            first.push_str(&format!("拆工檢查：{m}（不能政策核准；人工核准前請先確認）\n"));
        }
    }
    let split = if err.starts_with(&first) { first.len() } else { err.len() };
    format!("{}{}{}{}", err.get(..split).unwrap_or(err), out.get(..head_out).unwrap_or(out), err.get(split..).unwrap_or_default(), out.get(head_out..).unwrap_or_default())
}

/// `str.rstrip()`
fn rstrip(t: &str) -> &str {
    t.trim_end_matches(pyish::is_space)
}

/// `float(v or 0)`
fn py_float(v: &Value) -> py::R<f64> {
    if !py::truthy(v) {
        return Ok(0.0);
    }
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(t) => {
            let c = pyish::strip(t).replace('_', "");
            c.parse::<f64>().ok().filter(|_| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')))
                .ok_or_else(|| PyErr::other("ValueError", format!("could not convert string to float: {}", crate::pyfmt::repr_str(t))))
        }
        other => Err(py::type_err(format!("float() argument must be a string or a real number, not '{}'", py::tname(other)))),
    }
}

fn cmd_render(env: &Env, argv: &[String], plugin: Option<&str>, out: &mut String, err: &mut String) -> A<i32> {
    let out_dir = opt(argv, "--out");
    let names: Vec<String> = if has(argv, "--all") { workflow_names(env, plugin)? } else { argv.get(1).filter(|a| !a.starts_with('-')).cloned().into_iter().collect() };
    if names.is_empty() || out_dir.is_empty() {
        return awf_err("用法：awf render <workflow>|--all --out <目錄>");
    }
    for n in &names {
        let (text, notes) = render(env, n, plugin, None)?;
        write(&env.abs(&py::pj(out_dir, &format!("{n}.js"))), &text)?;
        for x in notes {
            err.push_str(&format!("注意（{n}）：{x}\n"));
        }
    }
    out.push_str(&format!("已產生 {} 支 workflow 到 {out_dir}\n", names.len()));
    Ok(0)
}

fn cmd_start(env: &Env, argv: &[String], run: &str, run_arg: &str, out: &mut String) -> A<i32> {
    let event = opt(argv, "--event");
    if event.is_empty() {
        return awf_err("缺 --event");
    }
    let ctx = env.sr();
    let init: Vec<String> = ["init", run, "--event", event, "--target", opt(argv, "--repo"), "--requirement", opt(argv, "--requirement")].iter().map(|x| x.to_string()).collect();
    let (code, o, e) = sr::run_captured(&init, "", &ctx);
    if code != 0 {
        return awf_err(pyish::strip(&format!("{o}{e}")));
    }
    let mut data = sr::load(&ctx, run)?;
    for k in ["repo", "epic", "base", "recipe", "modifiers", "size"] {
        let v = opt(argv, &format!("--{k}"));
        if !v.is_empty() {
            setdefault_obj(&mut data, "args")?.insert(k.to_string(), json!(v));
        }
    }
    let errs = apply_sets(&mut data, argv)?;
    if !errs.is_empty() {
        return awf_err(errs.join("；"));
    }
    sr::dump(&ctx, run, &data)?;
    out.push_str(&format!("已建立 {run_arg}/run.json。下一步：awf next --run {run_arg}\n"));
    Ok(0)
}

fn cmd_next(env: &Env, argv: &[String], run: &str, run_arg: &str, mut data: Value, plugin: Option<&str>, pnotes: &[String], out: &mut String) -> A<i32> {
    let errs = apply_sets(&mut data, argv)?;
    if !errs.is_empty() {
        return awf_err(errs.join("；"));
    }
    if !opt(argv, "--skip").is_empty() {
        auto::skip(env, &mut data, opt(argv, "--skip"))?;
    }
    let skip_notes = if sr::opts(argv, "--skip-subtask").is_empty() { Vec::new() } else { human::skip_subtasks(env, run, &mut data, argv)? };
    // 中斷回收（輔助）：上一次交出去的 plan 段沒有結果時，把已經做完的調研記進 reuse.json；任何失敗都當成沒有可回收的
    let recovered = auto::recover_interrupted(env, run, &mut data).unwrap_or_default();
    let act = if opt(argv, "--script").is_empty() { decide(env, run, run_arg, &mut data, plugin, argv)? } else { custom_action(env, run, run_arg, &mut data, argv)? };
    let mut act = carry(act, &data)?;
    if py::get(&act, "action")?.as_str() == Some("run_workflow") {
        let mut notes: Vec<Value> = pnotes.iter().map(|n| json!(n)).collect();
        notes.extend(py::iter(act.get("notes").unwrap_or(&json!([])))?);
        // 靠答覆往下走的段、被略過的子任務（沒有就不加）
        notes.extend(skip_notes.iter().chain(human::run_notes(&data).iter()).map(|n| json!(n)));
        if !recovered.is_empty() {
            notes.push(json!(format!("中斷回收：上次這一段沒跑完，已經做完而且來源沒變的調研會沿用（{}）", recovered.join("、"))));
            py::obj_mut(&mut act)?.insert("highlights".into(), json!({"recovered": recovered}));
        }
        py::obj_mut(&mut act)?.insert("notes".into(), Value::Array(notes));
        let mut pending = Map::new();
        for k in ["stage", "workflow", "script"] {
            pending.insert(k.to_string(), py::idx(&act, k)?.clone());
        }
        pending.insert("at".into(), json!(env.now_iso()));
        pending.insert("since".into(), json!(env.now_ts()));
        let mut mark = Map::new();
        for k in ["custom", "override"] {
            if let Some(v) = act.get(k).filter(|v| py::truthy(v)) {
                pending.insert(k.to_string(), v.clone());
                mark.insert(k.to_string(), v.clone());
            }
        }
        // 用步驟宣告的自訂段：段名與定義的指紋跟著 pending 走，awf done 從 workflow 紀錄取得結果時寫進結果（不算非標準）
        if let Some(v) = act.get("steps").filter(|v| py::truthy(v)) {
            pending.insert("steps".to_string(), v.clone());
        }
        let stage = py::s(py::idx(&act, "stage")?);
        // 中斷回收只認「交出去當時的 session」的 transcript，所以在會回收的那一種動作（SDLC 的 plan 段、標準腳本）記下來。
        // 認不出 session 就不寫（之後也就不回收）；其他動作的 pending 不變
        if mark.is_empty() && stage == "plan" && act.get("workflow").and_then(Value::as_str) == Some("sdlc-plan") && data.get("event").and_then(Value::as_str) == Some("sdlc") {
            let sess = current_session(env).unwrap_or_default();
            if !sess.is_empty() {
                pending.insert("session".into(), json!(sess));
            }
        }
        let aw = setdefault_obj(&mut data, "awf")?;
        aw.insert("pending".into(), Value::Object(pending));
        if !mark.is_empty() {
            // 記在 run.json：之後就算重跑 next 蓋掉 pending，這一段用過非標準腳本的事實也還在
            let ns = py::obj_mut(aw.entry("nonstandard".to_string()).or_insert_with(|| json!({})))?;
            ns.insert(stage, Value::Object(mark));
        }
    }
    sr::dump(&env.sr(), run, &data)?;
    emit(&act, argv, out)?;
    Ok(0)
}

fn cmd_done(env: &Env, argv: &[String], run: &str, run_arg: &str, mut data: Value, plugin: Option<&str>, out: &mut String) -> A<i32> {
    let ctx = env.sr();
    let pend = or_obj(py::get(&or_obj(py::get(&data, "awf")?), "pending")?);
    let stage_v = if opt(argv, "--stage").is_empty() { py::get(&pend, "stage")?.clone() } else { json!(opt(argv, "--stage")) };
    if !py::truthy(&stage_v) {
        return awf_err("不知道要存哪一段：先跑 awf next，或用 --stage 指定");
    }
    let stage = py::s(&stage_v);
    let src = opt(argv, "--output");
    let mut meta: Option<(Value, String)> = None;
    let mut fallback = false;
    let result: Value;
    if !src.is_empty() {
        match load_json(env, &py::pp(src))? {
            Some(v) if !v.is_null() => result = v,
            Some(_) => return awf_err(format!("{src} 讀不到或不是 JSON")),
            None => {
                // 檔在、但不是讀得懂的 JSON：和 awf save-run stage 一樣，這一段不能留著舊的結果與舊核准（1.0 審查 M3）。
                // 檔不存在（路徑打錯）不算，什麼都不改。記進 run.json 的說明不帶路徑
                let p = py::pp(src);
                let abs = if pyish::is_abs(&p) { p } else { pyish::pjoin(&env.cwd, &p) };
                if std::path::Path::new(&abs).is_file() {
                    let (mut o, mut e) = (String::new(), String::new());
                    sr::stage_unreadable(&ctx, run, &stage, "--output 的檔不是讀得懂的 JSON", &mut o, &mut e)?;
                    return awf_err(format!("{src} 不是讀得懂的 JSON：{stage} 段已標成 invalid、舊核准失效；修正檔案後重新 awf done，或重跑這一段"));
                }
                return awf_err(format!("{src} 讀不到或不是 JSON"));
            }
        }
    } else {
        let script_v = py::get(&pend, "script")?;
        let script = match script_v {
            v if !py::truthy(v) => String::new(),
            Value::String(t) => t.clone(),
            other => return Err(Fail::Py(py::type_err(format!("expected str, bytes or os.PathLike object, not {}", py::tname(other))))),
        };
        let mut want = if !script.is_empty() && !pyish::is_abs(&script) { pyish::pjoin(&env.project, &script) } else { script };
        let mut since_at = py_float(py::get(&pend, "since")?)?;
        // 沒有 pending（主 session 沒先跑 awf next --retry 就直接重跑了上一次的腳本）：只在能確定是哪一筆時才用 .wf/ 底下的標準腳本去對
        let mut seen = String::new();
        if want.is_empty() {
            let wf_name = if s_or(py::get(&data, "event")?) == "sdlc" {
                if is_flow(&data, &stage) { format!("flow-{stage}") } else { stage_workflow(&stage) }
            } else {
                event_route(env, &s_or(py::get(&data, "event")?))?.filter(|(_, st)| *st == stage).map(|(w, _)| w).unwrap_or_default()
            };
            if let Some((sc, at, id)) = human::done_fallback(run, &data, &stage, &wf_name) {
                (want, since_at, seen) = (sc, at, id);
                fallback = true;
            }
        }
        // 找到紀錄、但回傳讀不懂：和 --output 的檔讀不懂走同一條路（這一段不能留著舊的結果與舊核准）。找不到紀錄則什麼都不改
        let unreadable = |why: &str| -> A<i32> {
            let (mut o, mut e) = (String::new(), String::new());
            sr::stage_unreadable(&ctx, run, &stage, why, &mut o, &mut e)?;
            awf_err(format!("找到這一段的 workflow 紀錄，但{why}：{stage} 段已標成 invalid、舊核准失效；重跑這一段，或把正確的回傳存成 JSON 檔，用 --output <檔> 指定"))
        };
        let since = since_at;
        let found = args::find_wf_meta_except(env, &want, since, &seen)?;
        let Some((m, mf)) = found else {
            if unreadable_wf_meta(env, &want, since) {
                return unreadable("紀錄檔不是讀得懂的 JSON");
            }
            return awf_err(args::wf_meta_missing(env, &want));
        };
        let mut r = py::get(&m, "result")?.clone();
        if let Value::String(t) = &r {
            // 字串形式的 JSON（以 { 或 [ 開頭）讀不懂就是回傳讀不懂；一般的字串照舊當成「沒有回傳物件」
            let looks_json = t.trim_start().starts_with(['{', '[']);
            match py::loads(t) {
                Ok(v) => r = v,
                Err(_) if looks_json => return unreadable("回傳不是讀得懂的 JSON"),
                Err(e) if py::os_or_value(&e) => {}
                Err(e) => return Err(Fail::Py(e)),
            }
        }
        if !r.is_object() && !r.is_array() {
            return awf_err(format!("workflow 沒有回傳物件（status={}）：{mf}。它可能還在跑，或中途失敗；失敗就重跑這一段", py::s(py::get(&m, "status")?)));
        }
        result = r;
        meta = Some((m, mf));
    }
    // 不是「標準腳本、而且回傳是 awf 自己從 transcript 找到的」，就在結果裡留標記；save_run 看到標記不給政策核准
    let sticky = or_obj(py::get(&or_obj(py::get(&or_obj(py::get(&data, "awf")?), "nonstandard")?), &stage)?);
    // 退路取得的結果：那支腳本是上一次 next 備的，上一次記下的非標準標記（覆寫檔）照樣算在這一次
    let mine = if pyfmt_eq(py::get(&pend, "stage")?, &stage_v) { pend.clone() } else if fallback { sticky.clone() } else { json!({}) };
    let mut marks = Map::new();
    for (k, name) in [("custom", "custom_script"), ("override", "override_script")] {
        let own = py::get(&mine, k)?;
        if py::truthy(own) {
            marks.insert(name.to_string(), own.clone());
        } else if meta.is_none() && py::truthy(py::get(&sticky, k)?) {
            marks.insert(name.to_string(), py::idx(&sticky, k)?.clone());
        }
    }
    if meta.is_none() {
        marks.insert("manual_output".into(), json!(true));
    }
    let mut result = result;
    // 用步驟宣告的自訂段的標記（flow_steps）只由這裡寫：workflow 自己回傳的同名欄位一律拿掉；
    // 只有「標準腳本、回傳取自 workflow 紀錄」時才寫上 awf next 當時記下的段名與定義指紋
    if let Value::Object(m) = &mut result {
        m.shift_remove("flow_steps");
        let own = py::get(&mine, "steps")?;
        if marks.is_empty() && meta.is_some() && own.is_object() {
            m.insert("flow_steps".into(), own.clone());
        }
    }
    // 人決定略過的子任務記進 build 的結果（閘門與後面各段看得到；沒有略過時結果不變）
    let skipped_ids = sr::human::skipped_subtasks(&data);
    if stage == "build" && !skipped_ids.is_empty() {
        if let Value::Object(m) = &mut result {
            m.insert("skipped_subtasks".into(), json!(skipped_ids));
        }
    }
    if !marks.is_empty() {
        let mut m = match &result {
            Value::Object(o) => o.clone(),
            other => {
                let mut m = Map::new();
                m.insert("halted".into(), json!(true));
                m.insert("halt_reason".into(), json!("workflow 沒有回傳物件"));
                m.insert("returned".into(), other.clone());
                m
            }
        };
        for (k, v) in marks {
            m.insert(k, v);
        }
        result = Value::Object(m);
    }
    if meta.is_some() && !py::truthy(py::get(&mine, "custom")?) && !py::truthy(py::get(&mine, "override")?) && py::truthy(&sticky) {
        // 這次是標準腳本、回傳取自 transcript：解除標記
        if let Some(aw) = data.get_mut("awf").filter(|a| py::truthy(a)) {
            match aw.get_mut("nonstandard") {
                Some(Value::Object(ns)) => {
                    ns.shift_remove(&stage);
                }
                None => {}
                Some(other) => return Err(Fail::Py(py::attr_err(other, "pop"))),
            }
        }
        sr::dump(&ctx, run, &data)?;
    }
    let (mut o, mut e) = (String::new(), String::new());
    let code = sr::stage(&ctx, run, &stage, &result, "", &mut o, &mut e)?;
    let mut data = sr::load(&ctx, run)?;
    let st_now = py::get(py::idx(&data, "stages")?, &stage)?.clone();
    out.push_str(rstrip(&merge_stage_output(&stage, &st_now, &o, &e)));
    out.push('\n');
    if let Some(Value::Object(st)) = data.get_mut("stages").and_then(|s| s.get_mut(&stage)) {
        match &meta {
            Some((m, mf)) => {
                let id = py::get(m, "runId")?;
                let stem = py::pname(mf).strip_suffix(".json").map(str::to_string).unwrap_or_else(|| py::pname(mf));
                let id = if py::truthy(id) { py::s(id) } else { stem };
                st.insert("wf_run".into(), json!({"id": id, "session": py::pname(&py::pparent(&py::pparent(mf)))}));
            }
            None => {
                st.shift_remove("wf_run");
            }
        }
    }
    match data.get_mut("awf") {
        Some(Value::Object(aw)) => {
            aw.shift_remove("pending");
        }
        Some(other) if py::truthy(other) => return Err(Fail::Py(py::attr_err(other, "pop"))),
        _ => {}
    }
    sr::dump(&ctx, run, &data)?;
    if stage == "deliver" && code == 0 {
        if let Some(d @ Value::Object(_)) = result.as_object().and_then(|r| r.get("deliverables")) {
            out.push_str(&deliverables_sync(env, run, &data, d.get("updated")));
            out.push('\n');
        }
    }
    if !has(argv, "--no-next") {
        match decide(env, run, run_arg, &mut data, plugin, &[]) {
            Ok(act) => {
                let action = py::get(&act, "action")?.as_str().unwrap_or_default().to_string();
                let brief = if action == "run_workflow" {
                    // 還沒備好腳本：要跑 next 才會產生
                    let st = py::s(py::idx(&act, "stage")?);
                    json!({"action": "next", "stage": st, "say": format!("下一段是 {st}：跑 awf next --run {run_arg}")})
                } else if action == "gate" || action == "halted" {
                    act
                } else {
                    let mut b = Map::new();
                    for (k, v) in act.as_object().into_iter().flatten() {
                        if ["action", "stage", "workflow", "reason", "advice", "say", "need"].contains(&k.as_str()) {
                            b.insert(k.clone(), v.clone());
                        }
                    }
                    Value::Object(b)
                };
                out.push_str("下一步：\n");
                emit(&brief, argv, out)?;
            }
            Err(Fail::Awf(m)) => out.push_str(&format!("下一步算不出來：{m}\n")),
            Err(e) => return Err(e),
        }
    }
    Ok(if code != 0 { 1 } else { 0 })
}

fn pyfmt_eq(a: &Value, b: &Value) -> bool {
    crate::pyfmt::eq(a, b)
}

fn cmd_approve(env: &Env, argv: &[String], run: &str, run_arg: &str, data: &Value, out: &mut String, err: &mut String) -> A<i32> {
    let ctx = env.sr();
    let stages = py::idx(data, "stages")?;
    // 推翻某一段照建議先做的答覆（不核准任何一段）
    let reject = opt(argv, "--reject-unconfirmed");
    if has(argv, "--reject-unconfirmed") {
        let code = sr::human::reject_cmd(&ctx, run, reject, opt(argv, "--by"), &env.now_iso(), out, err)?;
        if code == 0 {
            out.push_str(&format!("下一步：awf next --run {run_arg}\n"));
        }
        return Ok(code);
    }
    let mut skip: Vec<String> = ["--run", "--by", "--note", "--answer-scope", "--answer-expires", "--profile"].iter().map(|n| opt(argv, n).to_string()).collect();
    skip.extend(sr::opts(argv, "--answer"));
    let mut stage = String::new();
    for a in argv.iter().skip(1).filter(|a| !a.starts_with('-')) {
        if py::contains(stages, a)? && !skip.contains(a) {
            stage = a.clone();
            break;
        }
    }
    if stage.is_empty() {
        let snap = sr::status_snapshot(&ctx, run, data)?;
        stage = s_or(py::get(&snap, "waiting_gate")?);
    }
    if stage.is_empty() && !sr::opts(argv, "--answer").is_empty() {
        // 沒有在等核准的段、但帶了答覆：下一段正停在 needs_decision 的話，就是要答它的 blocking 問題
        if let Some(n) = sr::next_stage(data)?.filter(|n| py::get(py::get(stages, n).unwrap_or(&Value::Null), "status").is_ok_and(|s| s.as_str() == Some("needs_decision"))) {
            stage = n;
        }
    }
    if stage.is_empty() && has(argv, "--confirm-unconfirmed") {
        // 只確認暫定做法、不核准任何一段
        return Ok(sr::human::confirm_cmd(&ctx, run, opt(argv, "--by"), &env.now_iso(), out, err)?);
    }
    if stage.is_empty() {
        return awf_err("現在沒有在等核准的段；要指定就寫 awf approve --run <run_dir> <段> --by <誰>");
    }
    if opt(argv, "--by").is_empty() {
        return awf_err("缺 --by <誰核准的>");
    }
    let mut pass: Vec<String> = vec!["approve".into(), run.to_string(), stage.clone(), "--at".into(), env.now_iso()];
    for n in ["--by", "--note", "--answer-scope", "--answer-expires", "--profile"] {
        if !opt(argv, n).is_empty() {
            pass.push(n.to_string());
            pass.push(opt(argv, n).to_string());
        }
    }
    for a in sr::opts(argv, "--answer") {
        pass.push("--answer".into());
        pass.push(a);
    }
    if has(argv, "--confirm-unconfirmed") {
        pass.push("--confirm-unconfirmed".into());
    }
    if has(argv, "--accept-defaults") {
        // 人確認了暫定的做法：沒有另外用 --answer 答的那幾題，照 agent 已套用的預設記進決策記憶
        let answered: Vec<String> = sr::opts(argv, "--answer").iter().map(|a| pyish::strip(a.split('=').next().unwrap_or_default()).to_string()).collect();
        for q in provisional(&args::stage_result(env, run, &stage)?)? {
            let (question, applied) = (py::s(py::idx(&q, "question")?), py::s(py::idx(&q, "applied")?));
            if !answered.contains(&question) && !question.contains('=') {
                pass.push("--answer".into());
                pass.push(format!("{question}={applied}"));
            }
        }
    }
    let code = sr::approve(&ctx, &pass, out, err)?;
    if code == 0 {
        out.push_str(&format!("下一步：awf next --run {run_arg}\n"));
    }
    Ok(code)
}

fn dispatch(env: &Env, argv: &[String], out: &mut String, err: &mut String) -> A<i32> {
    let cmd = argv.first().map(String::as_str).unwrap_or_default();
    let (plugin, pnotes) = if matches!(cmd, "approve" | "start" | "handoff") { (None, Vec::new()) } else { find_plugin(env, opt(argv, "--plugin-dir"))? };
    let plugin = plugin.as_deref();
    if cmd == "render" {
        return cmd_render(env, argv, plugin, out, err);
    }
    let run_arg = opt(argv, "--run").replace('\\', "/");
    let run_arg = run_arg.trim_end_matches('/');
    if run_arg.is_empty() {
        return awf_err("缺 --run <run 目錄>");
    }
    let run = if pyish::is_abs(run_arg) { pyish::ppath(run_arg) } else { pyish::pjoin(&env.project, run_arg) };
    let run = run.as_str();
    if cmd == "start" {
        return cmd_start(env, argv, run, run_arg, out);
    }
    let data = load_run(env, run)?;
    match cmd {
        "next" => cmd_next(env, argv, run, run_arg, data, plugin, &pnotes, out),
        "done" => cmd_done(env, argv, run, run_arg, data, plugin, out),
        "handoff" => {
            let text = handoff_text(env, run, run_arg, &data)?;
            if has(argv, "--print") {
                out.push_str(&text);
                out.push('\n');
                return Ok(0);
            }
            let file = py::pj(run, HANDOFF);
            crate::pack::write_lf(&file, &text).map_err(|e| PyErr::os(&e, &file))?;
            out.push_str(&format!("已寫 {run_arg}/{HANDOFF}。新的 session 先讀它，再跑 awf next --run {run_arg}\n"));
            Ok(0)
        }
        "approve" => cmd_approve(env, argv, run, run_arg, &data, out, err),
        other => awf_err(format!("不認得的指令：{other}")),
    }
}

/// 指令本體：stdout 的文字寫進 `out`、stderr 的寫進 `err`，回傳 exit code。`argv[0]` 是子指令（start、next、done…）。
pub fn run(env: &Env, argv: &[String], out: &mut String, err: &mut String) -> i32 {
    if has(argv, "--selftest") {
        return selftest(env, out);
    }
    let Some(cmd) = argv.first().map(String::as_str) else {
        out.push_str(USAGE);
        out.push('\n');
        return 2;
    };
    if cmd == "-h" || cmd == "--help" {
        out.push_str(USAGE);
        out.push('\n');
        return 0;
    }
    match dispatch(env, argv, out, err) {
        Ok(code) => code,
        Err(Fail::Awf(m)) => {
            err.push_str(&format!("awf：{m}\n"));
            1
        }
        Err(Fail::Py(e)) => {
            // 0.x 的 Python 版在這裡是沒接住的例外（traceback、exit 1）
            err.push_str(&format!("awf：{}：{}\n", e.class, e.msg));
            1
        }
    }
}

/// 在行程內跑一次：回傳（exit code, stdout, stderr）。
pub fn run_captured(argv: &[String], env: &Env) -> (i32, String, String) {
    let (mut out, mut err) = (String::new(), String::new());
    let code = run(env, argv, &mut out, &mut err);
    (code, out, err)
}

/// `awf start／next／done／approve／render／handoff …`、`awf state --selftest`。`args[0]` 是子指令名。
pub fn main(args: &[String]) -> i32 {
    let argv = if args.first().map(String::as_str) == Some("state") { args.get(1..).unwrap_or_default() } else { args };
    match argv.first().map(String::as_str) {
        // 唯讀的進度監看與安裝檢查：不需要 plugin，也不碰閘門
        Some("watch" | "statusline") if !has(argv, "--selftest") => return crate::watch::main(argv),
        Some("doctor") if !has(argv, "--selftest") => return crate::doctor::main(argv.get(1..).unwrap_or_default()),
        _ => {}
    }
    // done、approve 會把結果與核准記進閘門（轉給 save-run）：找不到專案根就不做，不拿目前目錄當專案
    if matches!(argv.first().map(String::as_str), Some("done" | "approve")) && !has(argv, "--selftest") && crate::project::find(None).is_none() {
        eprintln!("awf：{}", crate::project::not_found_message());
        return 1;
    }
    let (code, out, err) = run_captured(argv, &Env::from_env());
    print!("{out}");
    eprint!("{err}");
    code
}
