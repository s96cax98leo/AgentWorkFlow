//! run 的存檔、核准與閘門。由 `scaffold/.claude/teams/tools/save_run.py` 移植：輸出、exit code、訊息文字、寫出的檔案內容
//! 都和 Python 版逐字相同（對跑基準在 `tests/parity/save_run.expected.json`）。
//!
//! 這是整個流程的閘門，判定只能和 Python 版一樣或更嚴。分檔：
//! - `py`：Python 語意的小工具（型別不對時丟的例外、JSON 的讀寫、pathlib 的文字規則）。
//! - `trace`：信封、agent 缺件、執行紀錄（trace）的核對與代寫、執行紀錄索引、模型分級。
//! - `policy`：政策核准的判定、recipe 的一致與竄改檢查、整條閘門鏈、開發規範 profile 的讀法。
//! - `checks`：限制文件、拆工檢查、intent 檢查。
//! - `decisions`：決策記憶的輸入檢查與寫入。
//! - `archive`：結案摘要、狀態快照、context.md、recipe 摘要。
//! - `cli`：各個子指令（init、stage、approve、decide、escalate、followup、close、abandon、archive、status）。
//! - `selftest`：`--selftest`。
//!
//! 核心函式一律收明確的路徑（[`Ctx`]：schema 所在的 teams 目錄、專案的 profile、資料包的專案根、相對路徑的基準），
//! 不依賴行程的目前目錄。別的模組在行程內呼叫用 [`run_captured`]（Python 版是 awf.py 用 subprocess 呼叫 save_run.py）。
//!
//! 和 Python 版的差別（都不放寬判定；對跑測試的 `STRICTER`、`KNOWN` 逐項列名）：
//! - 讀進來的 JSON（workflow 回傳、run.json、各段的結果檔、profile）有 NaN、Infinity、落單的代理字元跳脫、
//!   超出 64 位元範圍的整數、巢狀超過 200 層時，一律當成讀不懂（Python 讀得進來）。
//! - Python 版丟出沒接住的例外的地方（traceback、exit 1），這裡印「awf save-run：類別：訊息」、exit 1，同樣不記錄、不核准。
//! - 用法訊息的指令名是 `awf save-run`。

mod archive;
mod checks;
mod cli;
mod decisions;
mod policy;
mod py;
mod selftest;
mod trace;

pub use crate::kb::PyErr;
pub use archive::{archive_text, clip, next_stage, recipe_summary, status_snapshot, write_archive, write_context, ARCHIVE_DIR, ARCHIVE_NAME};
pub use checks::{ac_ids, constraint_file, constraint_issues, constraint_rows, intent_errors, plan_check_for, plan_check_issues, result_subtasks, CONSTRAINT_CHECK_TIMEOUT, CONSTRAINT_VERDICTS};
pub use cli::{approve, run, stage, USAGE};
pub use decisions::{decision_errors, record_decisions, split_answer};
pub use policy::{
    abandoned_error, chain_errors, escalate_affected, followup_run_errors, has_recipe, load_profile, load_profile_for, mods, policy_violations, recipe_mismatch, recipe_tamper_errors,
    required_roles_missing, REVIEW_ROLES,
};
pub use selftest::selftest;
pub use trace::{envelope_of, envelopes, ghostwrite, is_envelope, md_cell, md_cell_str, missing_agents, model_errors, process_trace, record_candidates, retried_ok, Row};

use crate::kb::pyish;
use py::R;
use serde_json::Value;

/// sync（跟上 base）在 build 與 test 之間，沒有人工閘門：之後的 test 會重新驗證同步後的 head。
pub const SDLC: [&str; 7] = ["plan", "design", "build", "sync", "test", "deliver", "merge"];
/// merge 段不設人工閘門：合併與否由確定性的 merge gate 判定，它前面的 deliver 段仍要核准。
pub const GATED: [&str; 4] = ["plan", "design", "test", "deliver"];
/// plan 閘門一律由人核准（rules.md 規則 16），不接受政策核准。
pub const HUMAN_ONLY: [&str; 1] = ["plan"];
/// workflow 回傳裡「值是 null 也正常」的欄位（不是 agent 的交件）；其他位置出現 null 一律當 agent 缺件。
pub const NULL_OK_KEYS: [&str; 8] = ["gate", "halt_reason", "reason", "problem", "next", "summary", "rest", "head_sha"];
/// 不是 agent 交件的結構（trace 是派工紀錄、recipe 是分類結果），不在裡面找缺件的 null。
pub const NOT_DELIVERY_KEYS: [&str; 2] = ["trace", "recipe"];
pub const NO_ENVELOPE: &str = "沒有任何信封（agent 缺件）";
pub const INDEX_NAME: &str = "執行紀錄索引.md";
pub const STEWARD: &str = "dev-merge-steward";
pub const GHOSTWRITTEN: &str = "由 save_run 代寫";
pub const BLOCKING_SEVERITIES: [&str; 2] = ["blocker", "important"];
/// 已經改用執行核心（kernel 片段）的段：profile 有設 models 時，trace 每筆都要有 model 與 tier。0.5 起 SDLC 每一段都是。
pub const KERNEL_STAGES: [&str; 7] = ["plan", "design", "build", "sync", "test", "deliver", "merge"];

/// 執行環境：所有路徑都明確給，不看行程的目前目錄。
#[derive(Debug, Clone)]
pub struct Ctx {
    /// schema 所在的 `.claude/teams`（信封與狀態快照的 schema、guard.config.json）。
    pub teams: String,
    /// 專案的開發規範 profile（Python 版的 `PROJECT_PROFILE`）：`<專案>/.claude/teams/methodology.json`。
    /// 專案根目錄由它往上三層推得（限制文件的位置、檢查指令的工作目錄）。
    pub profile: String,
    /// 資料包（調研重用）認的專案根目錄：需求檔、來源檔要在它底下。
    pub pack_root: String,
    /// 相對路徑的基準（Python 版的目前目錄）。
    pub cwd: String,
    /// 固定「今天」（YYYY-MM-DD）；None 用本機日期。只影響 決策記憶.md 裡哪些決定算過期。
    pub today: Option<String>,
}

impl Ctx {
    /// 專案根目錄是 `root`、相對路徑以 `cwd` 為基準。
    pub fn new(root: &str, cwd: &str) -> Ctx {
        let teams = pyish::pjoin(root, ".claude/teams");
        Ctx { profile: pyish::pjoin(&teams, "methodology.json"), teams, pack_root: root.to_string(), cwd: cwd.to_string(), today: None }
    }

    /// 命令列用：專案根目錄取 `CLAUDE_PROJECT_DIR`（沒有就用目前目錄），相對路徑以目前目錄為基準。
    pub fn from_env() -> Ctx {
        let cwd = std::env::current_dir().ok().and_then(|p| p.to_str().map(|t| t.replace('\\', "/"))).unwrap_or_else(|| ".".to_string());
        Ctx::new(&crate::deliverables::default_root(), &cwd)
    }

    /// Python 版的 `project_root()`：profile 在 `<專案>/.claude/teams/` 底下。
    pub fn project_root(&self) -> String {
        py::pparent(&py::pparent(&py::pparent(&self.profile)))
    }

    /// 相對路徑接上基準目錄（只用來碰檔案系統；訊息裡印的仍是原本的寫法）。
    pub fn abs(&self, p: &str) -> String {
        if pyish::is_abs(p) { p.to_string() } else { pyish::pjoin(&self.cwd, p) }
    }

    pub(crate) fn is_file(&self, p: &str) -> bool {
        !p.contains('\0') && std::path::Path::new(&self.abs(p)).is_file()
    }

    pub(crate) fn is_dir(&self, p: &str) -> bool {
        !p.contains('\0') && std::path::Path::new(&self.abs(p)).is_dir()
    }

    pub(crate) fn exists(&self, p: &str) -> bool {
        !p.contains('\0') && std::path::Path::new(&self.abs(p)).exists()
    }

    /// `Path(p).read_text(encoding="utf-8")`
    pub(crate) fn read_text(&self, p: &str) -> R<String> {
        let raw = std::fs::read(self.abs(p)).map_err(|e| PyErr::os(&e, p))?;
        match String::from_utf8(raw) {
            Ok(t) => Ok(if t.contains('\r') { t.replace("\r\n", "\n").replace('\r', "\n") } else { t }),
            Err(e) => {
                let pos = e.utf8_error().valid_up_to();
                let b = e.as_bytes().get(pos).copied().unwrap_or(0);
                let why = match e.utf8_error().error_len() {
                    None => "unexpected end of data",
                    Some(_) if (0x80..=0xc1).contains(&b) || b >= 0xf5 => "invalid start byte",
                    Some(_) => "invalid continuation byte",
                };
                Err(PyErr::other("UnicodeDecodeError", format!("'utf-8' codec can't decode byte 0x{b:02x} in position {pos}: {why}")))
            }
        }
    }

    /// `Path(p).read_text(encoding="utf-8", errors="replace")`
    pub(crate) fn read_text_lossy(&self, p: &str) -> R<String> {
        let raw = std::fs::read(self.abs(p)).map_err(|e| PyErr::os(&e, p))?;
        let t = String::from_utf8_lossy(&raw);
        Ok(if t.contains('\r') { t.replace("\r\n", "\n").replace('\r', "\n") } else { t.into_owned() })
    }

    /// `Path(p).write_text(text, encoding="utf-8")`
    pub(crate) fn write_text(&self, p: &str, text: &str) -> R<()> {
        std::fs::write(self.abs(p), text.as_bytes()).map_err(|e| PyErr::os(&e, p))
    }

    /// `Path(p).mkdir(parents=True, exist_ok=True)`
    pub(crate) fn mkdir_p(&self, p: &str) -> R<()> {
        let full = self.abs(p);
        pyish::mkdir_p(&full).map_err(|mut e| {
            if full != p {
                e.msg = e.msg.replace(&full, p);
            }
            e
        })
    }

    /// `str(Path(p).resolve())`
    pub(crate) fn resolve(&self, p: &str) -> R<String> {
        pyish::resolve(&self.abs(p))
    }

    /// `json.loads(Path(p).read_text(encoding="utf-8"))`，JSON 照閘門的規則讀（見 [`py::loads`]）。
    pub(crate) fn load_json(&self, p: &str) -> R<Value> {
        py::loads(&self.read_text(p)?)
    }

    pub(crate) fn today(&self) -> String {
        if let Some(t) = &self.today {
            return t.clone();
        }
        match crate::hooks::py::local_today() {
            Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
            None => {
                let (y, m, d) = crate::merge_gate::pyutil::today_utc();
                format!("{y:04}-{m:02}-{d:02}")
            }
        }
    }
}

/// `opt(argv, name, default)`：第一個 `name` 後面那個參數。
/// `pack.run_epic(run)`：run.json 讀得進來、不是空值也不是物件時，Python 版在 `.get` 丟 AttributeError
/// （`pack::run_epic` 這時回空字串）。這裡先擋下來，不讓讀不懂的 run.json 被當成「沒有 epic」繼續做。
pub(crate) fn run_epic(ctx: &Ctx, run: &str) -> py::R<String> {
    let dir = ctx.abs(run);
    if let Some(v) = crate::pack::load_json(&py::pj(&dir, "run.json")) {
        if py::truthy(&v) && !v.is_object() {
            return Err(py::attr_err(&v, "get"));
        }
    }
    Ok(crate::pack::run_epic(&dir, None))
}

pub fn opt<'a>(argv: &'a [String], name: &str, default: &'a str) -> &'a str {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).map_or(default, String::as_str)
}

/// `opts(argv, name)`：每個 `name` 後面那個參數。
pub fn opts(argv: &[String], name: &str) -> Vec<String> {
    argv.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| argv.get(i + 1).cloned()).collect()
}

/// 讀 `<run>/run.json`。讀不到、不是 utf-8、不是看得懂的 JSON 都回錯誤（Python 版在這裡丟例外結束）。
pub fn load(ctx: &Ctx, run: &str) -> R<Value> {
    ctx.load_json(&py::pj(run, "run.json"))
}

/// 寫 `<run>/run.json`（`json.dumps(data, ensure_ascii=False, indent=2)` 加結尾換行，逐位元相同）。
pub fn dump(ctx: &Ctx, run: &str, data: &Value) -> R<()> {
    ctx.write_text(&py::pj(run, "run.json"), &format!("{}\n", py::dumps(data, Some(2))))
}

/// 在行程內跑一次 `save-run`：回傳（exit code, stdout, stderr）。`stdin` 只有 `stage` 會讀。
pub fn run_captured(argv: &[String], stdin: &str, ctx: &Ctx) -> (i32, String, String) {
    let (mut out, mut err) = (String::new(), String::new());
    let code = cli::run(ctx, argv, &mut || Ok(stdin.to_string()), &mut out, &mut err);
    (code, out, err)
}

/// `awf save-run …`
pub fn main(args: &[String]) -> i32 {
    let ctx = Ctx::from_env();
    let (mut out, mut err) = (String::new(), String::new());
    let mut read_stdin = || -> R<String> {
        use std::io::Read;
        let mut raw = Vec::new();
        std::io::stdin().read_to_end(&mut raw).map_err(|e| PyErr::os(&e, "<stdin>"))?;
        String::from_utf8(raw).map_err(|_| PyErr::other("UnicodeDecodeError", "'utf-8' codec can't decode stdin"))
    };
    let code = cli::run(&ctx, args, &mut read_stdin, &mut out, &mut err);
    print!("{out}");
    eprint!("{err}");
    code
}
