//! 知識圖（`awf kg`）：把規格層、知識層的文件段落、連結帳本、程式索引與 git log 接成一張圖，
//! 回答「這個需求在哪裡」「改這個檔會影響什麼」「哪些連結斷了」。
//!
//! 這是輔助功能：任何失敗都只印原因並回非零的結束碼，不影響其他子指令；閘門不讀它。
//! 用法與限制見 `docs/知識圖.md`。

pub mod build;
pub mod code;
pub mod export;
pub mod links;
pub mod query;
pub mod resolve;
mod selftest;
pub mod sources;
pub mod store;

pub use build::{build, layout, BuildOpts, BuildReport, Layout, Outcome, DEFAULT_TIMEOUT};
pub use query::{answer_json, check, format_answer, impact, locate, neighbors, open, trace, Answer, Graph, Item};
pub use selftest::selftest;

use std::io::Write;

pub const USAGE: &str = "知識圖：規格、文件、連結帳本、程式索引與 git log 接成的圖。查詢只回「路徑:起訖行＋理由」，不貼原文。

用法：
    awf kg build [--full] [--timeout <秒>]
    awf kg locate \"<需求文字>\" [--top <筆數>]
    awf kg trace <需求｜情境｜AC id>
    awf kg impact <檔案｜符號>… | --diff <base>
    awf kg check [--run <run 目錄>]
    awf kg neighbors <節點> [--depth 2]
    awf kg export [--out <目錄>]
    awf kg --selftest

共同選項：
    --root <目錄>       專案根目錄（預設 CLAUDE_PROJECT_DIR，再沒有就是目前目錄）
    --profile <檔>      開發規範 profile（預設 .claude/teams/methodology.json）
    --dir <目錄>        圖所在的目錄（預設 profile 的 knowledge.dir，再沒有就是 _tpm/kb）
    --spec-root <目錄>  規格根目錄（預設 openspec；只有 build 用）
    --json              給機器讀的輸出（查詢）
    --budget <字數>     輸出的字數上限（預設依 repo 檔數分三級：2400、4000、6000）

build 逾時（預設 60 秒）會放棄這次更新、保留上一版的圖，結束碼 3。其他錯誤結束碼 1。
";

/// 給流程接點（之後的 `awf next`、SessionStart hook）用的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    Updated,
    Unchanged,
    TimedOut,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub status: UpdateStatus,
    /// 一行說明。
    pub note: String,
}

fn summary(r: &BuildReport) -> String {
    match r.outcome {
        Outcome::Updated => format!("知識圖已更新：{} 個檔（重算 {}、移除 {}），{} 個節點、{} 條邊、{} 筆未解析", r.files, r.changed, r.removed, r.nodes, r.edges, r.unresolved),
        Outcome::Unchanged => format!("知識圖沒有變動：{} 個檔，{} 個節點、{} 條邊", r.files, r.nodes, r.edges),
        Outcome::TimedOut => "知識圖更新逾時，這次放棄，上一版的圖照舊可用（可加大 --timeout 或手動跑 awf kg build）".to_string(),
    }
}

/// 增量更新知識圖。不會 panic；失敗與逾時都只回狀態與一行說明，上一版的圖保持可用。
pub fn update(root: Option<&str>, timeout_secs: Option<u64>) -> Update {
    let o = BuildOpts { root: root.map(str::to_string), timeout: timeout_secs, ..Default::default() };
    match build(&o) {
        Ok(r) => {
            let status = match r.outcome {
                Outcome::Updated => UpdateStatus::Updated,
                Outcome::Unchanged => UpdateStatus::Unchanged,
                Outcome::TimedOut => UpdateStatus::TimedOut,
            };
            Update { status, note: summary(&r) }
        }
        Err(msg) => Update { status: UpdateStatus::Failed, note: format!("知識圖更新失敗：{}", crate::kb::pyish::take(&msg.replace('\n', " "), 200)) },
    }
}

#[derive(Default)]
struct Args {
    pos: Vec<String>,
    root: Option<String>,
    profile: Option<String>,
    dir: Option<String>,
    spec_root: Option<String>,
    out: Option<String>,
    diff: Option<String>,
    run: Option<String>,
    json: bool,
    full: bool,
    timeout: Option<u64>,
    budget: Option<usize>,
    top: Option<usize>,
    depth: Option<usize>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    let num = |name: &str, v: &str| v.parse::<u64>().map_err(|_| format!("{name} 要是非負整數：{}", crate::kb::pyish::take(v, 40)));
    while i < args.len() {
        let raw = args[i].as_str();
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (raw, None),
        };
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{name} 後面要給值"))
        };
        match name {
            "--json" => a.json = true,
            "--full" => a.full = true,
            "--root" => a.root = Some(value(&mut i)?),
            "--profile" => a.profile = Some(value(&mut i)?),
            "--dir" => a.dir = Some(value(&mut i)?),
            "--spec-root" => a.spec_root = Some(value(&mut i)?),
            "--out" => a.out = Some(value(&mut i)?),
            "--diff" => a.diff = Some(value(&mut i)?),
            "--run" => a.run = Some(value(&mut i)?),
            "--timeout" => a.timeout = Some(num(name, &value(&mut i)?)?),
            "--budget" => a.budget = Some(num(name, &value(&mut i)?)? as usize),
            "--top" => a.top = Some(num(name, &value(&mut i)?)? as usize),
            "--depth" => a.depth = Some(num(name, &value(&mut i)?)? as usize),
            "--" => {
                a.pos.extend(args[i + 1..].iter().cloned());
                break;
            }
            s if s.starts_with("--") => return Err(format!("不認得的選項：{}", crate::kb::pyish::take(s, 40))),
            _ => a.pos.push(raw.to_string()),
        }
        i += 1;
    }
    Ok(a)
}

fn run(args: &[String], out: &mut String) -> Result<i32, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        out.push_str(USAGE);
        return Ok(0);
    };
    if matches!(sub, "-h" | "--help") {
        out.push_str(USAGE);
        return Ok(0);
    }
    let a = parse(&args[1..])?;
    let lay = || layout(a.root.as_deref(), a.profile.as_deref(), a.dir.as_deref(), a.spec_root.as_deref());
    let show = |out: &mut String, g: &Graph, ans: &Answer| {
        out.push_str(&if a.json { answer_json(ans) } else { format_answer(ans, a.budget.unwrap_or_else(|| g.budget())) });
        out.push('\n');
    };
    match sub {
        "build" => {
            let o = BuildOpts { root: a.root.clone(), profile: a.profile.clone(), dir: a.dir.clone(), spec_root: a.spec_root.clone(), full: a.full, timeout: a.timeout };
            let r = build(&o)?;
            out.push_str(&summary(&r));
            out.push('\n');
            for n in &r.notes {
                out.push_str(&format!("注意：{n}\n"));
            }
            if r.bad_links > 0 {
                out.push_str(&format!("注意：連結帳本有 {} 行讀不懂，已略過\n", r.bad_links));
            }
            if !r.skipped.is_empty() {
                let shown: Vec<String> = r.skipped.iter().take(5).map(|(p, why)| format!("{p}（{why}）")).collect();
                out.push_str(&format!("略過 {} 個檔：{}{}\n", r.skipped.len(), shown.join("、"), if r.skipped.len() > 5 { "…" } else { "" }));
            }
            Ok(if r.outcome == Outcome::TimedOut { 3 } else { 0 })
        }
        "locate" => {
            let text = a.pos.join(" ");
            if text.trim().is_empty() {
                return Err("用法：awf kg locate \"<需求文字>\"".into());
            }
            let g = open(&lay()?)?;
            show(out, &g, &locate(&g, &text, a.top.unwrap_or(12).clamp(1, 100)));
            Ok(0)
        }
        "trace" => {
            let [id] = a.pos.as_slice() else { return Err("用法：awf kg trace <需求｜情境｜AC id>".into()) };
            let g = open(&lay()?)?;
            show(out, &g, &trace(&g, id)?);
            Ok(0)
        }
        "impact" => {
            let l = lay()?;
            let g = open(&l)?;
            let mut targets = a.pos.clone();
            if let Some(base) = &a.diff {
                targets.extend(query::diff_files(&l.root, base)?);
            }
            if targets.is_empty() {
                if a.diff.is_some() {
                    out.push_str(&format!("{}\n（和 base 比沒有改動的檔）\n{}\n", query::OPEN_TAG, query::CLOSE_TAG));
                    return Ok(0);
                }
                return Err("用法：awf kg impact <檔案｜符號>… 或 --diff <base>".into());
            }
            show(out, &g, &impact(&g, &targets));
            Ok(0)
        }
        "check" => {
            let g = open(&lay()?)?;
            let run = a.run.as_deref().map(|r| crate::kb::to_posix(r).trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()).filter(|r| !r.is_empty());
            show(out, &g, &check(&g, run.as_deref()));
            Ok(0)
        }
        "neighbors" => {
            let [id] = a.pos.as_slice() else { return Err("用法：awf kg neighbors <節點> [--depth 2]".into()) };
            let g = open(&lay()?)?;
            show(out, &g, &neighbors(&g, id, a.depth.unwrap_or(2))?);
            Ok(0)
        }
        "export" => {
            let l = lay()?;
            let g = open(&l)?;
            let r = export::export(&g, &l, a.out.as_deref())?;
            out.push_str(&format!("已寫 {} 頁到 {}（notes/ 沒有動）\n", r.pages, r.dir));
            for k in &r.kept {
                out.push_str(&format!("注意：{k} 不是工具產生的，沒有動\n"));
            }
            Ok(0)
        }
        other => Err(format!("不認得的子指令「{}」\n\n{USAGE}", crate::kb::pyish::take(other, 40))),
    }
}

/// 跑一次並收集輸出（測試用）：（結束碼, stdout, stderr）。
pub fn run_captured(args: &[String]) -> (i32, String, String) {
    let mut out = String::new();
    match run(args, &mut out) {
        Ok(code) => (code, out, String::new()),
        Err(msg) => (1, out, format!("kg：{msg}\n")),
    }
}

pub fn main(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("--selftest") {
        return selftest();
    }
    let (code, out, err) = run_captured(args);
    let _ = std::io::stdout().write_all(out.as_bytes());
    let _ = std::io::stderr().write_all(err.as_bytes());
    code
}
