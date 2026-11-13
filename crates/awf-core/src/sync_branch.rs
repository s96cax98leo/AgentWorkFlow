//! 確定性分支同步工具：讓 epic／功能分支跟上 base（預設 origin/main），策略自動判定或固定。
//! 由 `scaffold/.claude/teams/tools/sync_branch.py` 移植；判定、exit code、輸出的 JSON 照 Python 版。
//!
//! 策略（methodology.json 的 conventions.sync.strategy；沒寫＝auto，--strategy 可覆蓋）：
//!   auto     已跟上 → none；未推送且無衝突 → rebase；已推送且無衝突 → merge（把 base merge 進來）；有衝突 → transit。
//!   rebase   已推送（要 force push）或有衝突 → 停下交人。
//!   merge    有衝突 → 停下交人。
//!   transit  一律可行：從 base 開中轉分支，把功能分支 merge 進去，衝突在中轉分支上解，再開 PR。
//!   固定策略遇到做不到的情況一律停下（halted），不自動改走別的策略。
//!
//! 輸出：stdout 一個 JSON；停下時 "halted": true 並附 reason。參數錯誤也輸出 JSON（halted）並以 1 結束。
//! 結束碼：0＝成功；1＝停下交人或檢查不通過；2＝apply transit 有衝突，已留在中轉分支的 worktree；
//!         3＝apply rebase／merge 失敗，已 abort 回原狀。
//!
//! 和 Python 版的差別：執行不了 git、設定值形狀讓 Python 丟例外的地方，這裡一律停下（halted、exit 1），不印 traceback。

use crate::merge_gate::pyargs::{self, opt, req, ArgErr, Kind, Opt, Parsed};
use crate::merge_gate::{pyjson, pyutil};
use crate::pyfmt::{self, repr_str};
use crate::recipe::dumps;
use pyutil::{strip, tail};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::process::Command;
use std::sync::OnceLock;

const DEFAULT_PREFIXES: &[&str] = &["feature/", "bugfix/", "hotfix/", "release/", "merge/"]; // 同 hook 的設定預設值
const DEFAULT_TRANSIT: &str = "merge/<短名>-into-<base>-<YYYYMMDD>";
const STRATEGIES: &[&str] = &["auto", "rebase", "merge", "transit"];

pub const USAGE: &str = "確定性分支同步工具：讓功能分支跟上 base

用法（在已 init 的專案根目錄）：
  awf sync-branch plan --repo <repo 目錄> --branch feature/x \\
      [--base origin/main] [--remote origin] [--strategy auto|rebase|merge|transit] [--short <短名>] \\
      [--date YYYYMMDD] [--config <guard.config.json>] [--methodology <methodology.json>] [--no-fetch]
  awf sync-branch apply --repo <repo 目錄> --branch feature/x --strategy rebase|merge|transit \\
      [--base origin/main] [--transit-branch <中轉分支>] [--date YYYYMMDD] [--config <guard.config.json>]
  awf sync-branch check-transit --repo <repo 目錄> --branch feature/x --base origin/main \\
      --expected <檔案1,檔案2,…；沒有就給空字串>
  awf sync-branch --selftest

  plan           只讀（會 git fetch，除非 --no-fetch）。輸出 behind、published、conflicts、strategy、transit_branch…
  apply          worktree 要在 --branch 上且乾淨。rebase 先寫備份 ref refs/backup/<分支，/ 換成 ->-<日期>（已存在就停）；
                 rebase／merge 失敗會 abort。transit 從 base 開中轉分支（--no-track）再 merge 功能分支，衝突留在 worktree。
  check-transit  中轉分支解完衝突、commit 後執行：HEAD 要是兩個 parent 的 merge commit，base 與功能分支都是祖先，
                 改動的檔案沒有衝突標記；--expected 以外的檔案要和 git 自動合併的結果一模一樣。

輸出：stdout 一個 JSON；停下時 \"halted\": true 並附 reason。
結束碼：0＝成功；1＝停下交人或檢查不通過；2＝apply transit 有衝突；3＝apply rebase／merge 失敗，已 abort。
";

fn placeholder_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\{\{[A-Z_]+\}\}").expect("固定的正規式"))
}

/// Python 的 `^\d{8}$`：`$` 也配結尾換行之前。
fn date_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\d{8}(?:\n?\z)").expect("固定的正規式"))
}

fn tree_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[0-9a-f]{40,64}\z").expect("固定的正規式"))
}

/// 執行環境：測試用它固定 git 的設定與日期。
#[derive(Default, Clone)]
pub struct Ctx {
    /// 另外加給 git 的環境變數。
    pub env: Vec<(String, String)>,
}

/// 停下交人（exit 1、halted）。
struct Halt(String);

type R<T> = Result<T, Halt>;
type Rec = Map<String, Value>;

static NULL: Value = Value::Null;

fn g<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object().and_then(|o| o.get(k)).unwrap_or(&NULL)
}

// ------------------------------------------------------------------ git

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
    raw: Vec<u8>,
}

impl Out {
    /// `r.stderr or r.stdout`。
    fn message(&self) -> &str {
        if self.stderr.is_empty() { &self.stdout } else { &self.stderr }
    }
}

struct Git<'a> {
    ctx: &'a Ctx,
    repo: String,
}

impl Git<'_> {
    fn run(&self, args: &[&str]) -> R<Out> {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.repo).args(args);
        for (k, v) in &self.ctx.env { c.env(k, v); }
        c.env("PYTHONUTF8", "1").env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("LC_ALL", "C");
        match pyutil::run(&mut c, None, None) {
            Ok(r) => Ok(Out { code: r.code, stdout: pyutil::text(&r.stdout), stderr: pyutil::text(&r.stderr), raw: r.stdout }),
            Err(pyutil::RunErr::Os(e)) => Err(Halt(format!("執行不了 git：{}", pyutil::os_error_text(&e, "git")))),
            Err(pyutil::RunErr::Timeout) => Err(Halt("執行不了 git：逾時".into())),
        }
    }

    fn must(&self, args: &[&str]) -> R<String> {
        let r = self.run(args)?;
        if r.code != 0 {
            return Err(Halt(format!("git {} 失敗：{}", args.iter().take(3).copied().collect::<Vec<_>>().join(" "), tail(strip(r.message()), 300))));
        }
        Ok(r.stdout)
    }

    fn resolve(&self, reference: &str) -> R<String> {
        let r = self.run(&["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])?;
        Ok(if r.code == 0 { strip(&r.stdout).to_string() } else { String::new() })
    }

    fn is_ancestor(&self, a: &str, b: &str) -> R<bool> {
        let r = self.run(&["merge-base", "--is-ancestor", a, b])?;
        if !matches!(r.code, 0 | 1) { return Err(Halt(format!("merge-base --is-ancestor 失敗：{}", tail(strip(&r.stderr), 200)))); }
        Ok(r.code == 0)
    }

    fn unmerged(&self) -> R<Vec<String>> {
        let mut files = lines(&self.run(&["diff", "--name-only", "--diff-filter=U"])?.stdout);
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// 回傳（合併結果的 tree、衝突檔清單）。exit 1＝有衝突；其他失敗（含不支援 --write-tree 的舊 git）→ 停下。
    fn merge_tree(&self, a: &str, b: &str) -> R<(String, Vec<String>)> {
        let r = self.run(&["merge-tree", "--write-tree", "--name-only", a, b])?;
        if !matches!(r.code, 0 | 1) {
            return Err(Halt(format!("git merge-tree --write-tree 失敗（需要 git 2.38 以上）：{}", tail(strip(r.message()), 200))));
        }
        let text = r.stdout.replace("\r\n", "\n");
        let mut out = text.split('\n');
        let tree = strip(out.next().unwrap_or("")).to_string();
        if !tree_re().is_match(&tree) {
            return Err(Halt(format!("看不懂 merge-tree 的輸出：{}", repr_str(pyutil::head(&r.stdout, 200)))));
        }
        let mut files: Vec<String> = Vec::new();
        if r.code == 1 {
            for l in out {
                if strip(l).is_empty() { break; }
                files.push(strip(l).replace('\\', "/"));
            }
            if files.is_empty() { return Err(Halt("merge-tree 回報有衝突，但讀不到衝突檔清單".into())); }
        }
        files.sort();
        files.dedup();
        Ok((tree, files))
    }

    fn check_branch_name(&self, name: &str, allowed: &[String]) -> R<()> {
        if self.run(&["check-ref-format", "--branch", name])?.code != 0 {
            return Err(Halt(format!("分支名不合法：{}", repr_str(name))));
        }
        let prefix_ok = name.find('/').is_some_and(|i| allowed.iter().any(|a| a == &name[..=i]));
        if !prefix_ok {
            return Err(Halt(format!("中轉分支 {name} 的前綴不在 allowed_branch_prefixes（{}）", allowed.join("、"))));
        }
        Ok(())
    }
}

fn lines(out: &str) -> Vec<String> {
    out.replace("\r\n", "\n").split('\n').filter(|l| !strip(l).is_empty()).map(|l| strip(l).replace('\\', "/")).collect()
}

// ------------------------------------------------------------------ 設定

/// 檔案不存在＝空設定（用預設值）；存在但讀不懂＝停下（不做樂觀推定）。
fn load_json(path: &str, what: &str) -> R<Value> {
    if !std::path::Path::new(path).exists() { return Ok(json!({})); }
    let text = pyutil::read_text(path).map_err(|e| Halt(format!("{what} {path} 讀不懂：{}", e.msg())))?;
    let data = pyjson::loads(&text).map_err(|e| Halt(format!("{what} {path} 讀不懂：{e}")))?;
    if !data.is_object() { return Err(Halt(format!("{what} {path} 不是 JSON 物件"))); }
    Ok(data)
}

fn prefixes(repo: &str, config: Option<&str>) -> R<Vec<String>> {
    let path = config.map(pyutil::path_str).unwrap_or_else(|| pyutil::join(repo, ".claude/teams/guard.config.json"));
    let cfg = load_json(&path, "guard.config")?;
    let items: Vec<Value> = match g(&cfg, "allowed_branch_prefixes") {
        Value::String(s) => s.split(',').map(|x| json!(x)).collect(),
        v if !pyfmt::truthy(v) => Vec::new(),
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.keys().map(|k| json!(k)).collect(),
        other => return Err(Halt(format!("guard.config {path} 的 allowed_branch_prefixes 不是清單（{}）", pyfmt::repr(other)))),
    };
    let out: Vec<String> = items.iter().filter_map(Value::as_str).filter(|x| !strip(x).is_empty() && !placeholder_re().is_match(x)).map(|x| strip(x).to_string()).collect();
    Ok(if out.is_empty() { DEFAULT_PREFIXES.iter().map(|p| p.to_string()).collect() } else { out })
}

struct Conventions {
    strategy: String,
    transit_naming: String,
}

fn sync_conventions(repo: &str, methodology: Option<&str>) -> R<Conventions> {
    let path = methodology.map(pyutil::path_str).unwrap_or_else(|| pyutil::join(repo, ".claude/teams/methodology.json"));
    let prof = load_json(&path, "methodology")?;
    let empty = json!({});
    let sync = match g(&prof, "conventions") {
        conv @ Value::Object(_) if pyfmt::truthy(g(conv, "sync")) => g(conv, "sync"),
        _ => &empty,
    };
    let Value::Object(sync_map) = sync else { return Err(Halt("methodology 的 conventions.sync 不是物件".into())) };
    let strategy = sync_map.get("strategy").cloned().unwrap_or_else(|| json!("auto"));
    let Some(strategy) = strategy.as_str().filter(|s| STRATEGIES.contains(s)) else {
        return Err(Halt(format!("methodology 的 conventions.sync.strategy 不認得：{}", pyfmt::repr(&strategy))));
    };
    let naming = match g(sync, "transit_naming") {
        v if !pyfmt::truthy(v) => DEFAULT_TRANSIT.to_string(),
        Value::String(s) => s.clone(),
        _ => return Err(Halt("methodology 的 conventions.sync.transit_naming 不是字串".into())),
    };
    Ok(Conventions { strategy: strategy.to_string(), transit_naming: naming })
}

fn base_name<'a>(base: &'a str, remote: &str) -> &'a str {
    for p in [format!("refs/remotes/{remote}/"), format!("{remote}/"), "refs/heads/".to_string()] {
        if let Some(rest) = base.strip_prefix(&p) { return rest; }
    }
    base
}

fn transit_name(git: &Git, naming: &str, short: &str, base: &str, date: Option<&str>, allowed: &[String]) -> R<String> {
    let Some(date) = date else { return Err(Halt("策略是 transit，但沒有 --date（中轉分支名要日期；腳本不取時間）".into())) };
    if !date_re().is_match(date) { return Err(Halt(format!("--date 要是 YYYYMMDD：{}", repr_str(date)))); }
    let name = naming.replace("<短名>", short).replace("<base>", base).replace("<YYYYMMDD>", date);
    if name.contains('<') || name.contains('>') { return Err(Halt(format!("transit_naming 有不認得的佔位符：{naming}"))); }
    git.check_branch_name(&name, allowed)?;
    Ok(name)
}

// ------------------------------------------------------------------ plan

/// 回傳（strategy、reason）；做不到就停下。
fn decide(requested: &str, behind: u64, published: bool, conflicts: &[String]) -> R<(&'static str, String)> {
    let shown = || conflicts.iter().take(10).cloned().collect::<Vec<_>>().join("、");
    if behind == 0 { return Ok(("none", "已跟上 base，不用同步".into())); }
    match requested {
        "auto" => Ok(if !conflicts.is_empty() {
            ("transit", format!("有衝突（{} 個檔），走中轉分支", conflicts.len()))
        } else if !published {
            ("rebase", "還沒推送且無衝突，rebase 保持線性歷史".into())
        } else {
            ("merge", "已推送且無衝突，把 base merge 進來（不改寫已推送的歷史）".into())
        }),
        "rebase" => {
            if published { return Err(Halt("指定 rebase，但分支已推送：rebase 後要 force push，停下交人".into())); }
            if !conflicts.is_empty() { return Err(Halt(format!("指定 rebase，但有衝突（{}），停下交人", shown()))); }
            Ok(("rebase", "指定 rebase；未推送且無衝突".into()))
        }
        "merge" => {
            if !conflicts.is_empty() { return Err(Halt(format!("指定 merge，但有衝突（{}），停下交人", shown()))); }
            Ok(("merge", "指定 merge；無衝突".into()))
        }
        _ => Ok(("transit", "指定 transit".into())),
    }
}

/// `decide` 的對跑入口：回傳（strategy、reason），停下時回停下的理由。
pub fn decide_text(requested: &str, behind: u64, published: bool, conflicts: &[String]) -> Result<(String, String), String> {
    decide(requested, behind, published, conflicts).map(|(s, r)| (s.to_string(), r)).map_err(|Halt(m)| m)
}

fn set(rec: &mut Rec, pairs: Value) {
    if let Value::Object(o) = pairs {
        for (k, v) in o { rec.insert(k, v); }
    }
}

fn cmd_plan(ctx: &Ctx, a: &Parsed, rec: &mut Rec) -> R<i32> {
    let git = Git { ctx, repo: pyutil::resolve(a.get("--repo").unwrap_or("")) };
    let (branch, base, remote) = (a.get("--branch").unwrap_or(""), a.get("--base").unwrap_or("origin/main"), a.get("--remote").unwrap_or("origin"));
    set(rec, json!({"branch": branch, "base": base, "behind": null, "published": null, "conflicts": [],
        "strategy": null, "requested_strategy": null, "transit_branch": null}));
    let conv = sync_conventions(&git.repo, a.truthy("--methodology"))?;
    let requested = a.truthy("--strategy").unwrap_or(&conv.strategy).to_string();
    rec.insert("requested_strategy".into(), json!(requested));
    if !a.flag("--no-fetch") {
        let r = git.run(&["fetch", "--quiet", remote])?;
        if r.code != 0 { return Err(Halt(format!("git fetch {remote} 失敗：{}", tail(strip(&r.stderr), 300)))); }
    }
    let head_ref = format!("refs/heads/{branch}");
    if git.resolve(&head_ref)?.is_empty() { return Err(Halt(format!("本機沒有分支 {branch}"))); }
    if git.resolve(base)?.is_empty() { return Err(Halt(format!("找不到 base {base}"))); }
    let out = git.must(&["rev-list", "--count", &format!("{head_ref}..{base}")])?;
    let behind_text = strip(&out);
    let behind: u64 = match behind_text.parse() {
        Ok(n) if behind_text.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => return Err(Halt(format!("rev-list --count 輸出看不懂：{}", repr_str(behind_text)))),
    };
    rec.insert("behind".into(), json!(behind));
    let published = !git.resolve(&format!("refs/remotes/{remote}/{branch}"))?.is_empty();
    rec.insert("published".into(), json!(published));
    let (_, conflicts) = git.merge_tree(base, &head_ref)?;
    rec.insert("conflicts".into(), json!(conflicts));
    let (strategy, reason) = decide(&requested, behind, published, &conflicts)?;
    if strategy == "transit" {
        let short = a.truthy("--short").unwrap_or_else(|| branch.trim_end_matches('/').rsplit('/').next().unwrap_or(""));
        let allowed = prefixes(&git.repo, a.truthy("--config"))?;
        let name = transit_name(&git, &conv.transit_naming, short, base_name(base, remote), a.truthy("--date"), &allowed)?;
        rec.insert("transit_branch".into(), json!(name));
    }
    rec.insert("strategy".into(), json!(strategy));
    rec.insert("reason".into(), json!(reason));
    Ok(0)
}

// ------------------------------------------------------------------ apply

fn finish(git: &Git, rec: &mut Rec, code: i32, reason: String) -> R<i32> {
    let cur = git.run(&["symbolic-ref", "--quiet", "--short", "HEAD"])?.stdout;
    let sha = git.resolve("HEAD")?;
    let or_null = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
    rec.insert("head_branch".into(), or_null(strip(&cur)));
    rec.insert("head_sha".into(), or_null(&sha));
    rec.insert("reason".into(), json!(reason));
    Ok(code)
}

fn cmd_apply(ctx: &Ctx, a: &Parsed, rec: &mut Rec) -> R<i32> {
    let git = Git { ctx, repo: pyutil::resolve(a.get("--repo").unwrap_or("")) };
    let (branch, base, strategy) = (a.get("--branch").unwrap_or(""), a.get("--base").unwrap_or("origin/main"), a.get("--strategy").unwrap_or(""));
    set(rec, json!({"strategy": strategy, "branch": branch, "base": base, "head_branch": null, "head_sha": null, "conflicts": [], "backup_ref": null}));
    let cur = git.run(&["symbolic-ref", "--quiet", "--short", "HEAD"])?.stdout;
    let cur = strip(&cur);
    if cur != branch {
        return Err(Halt(format!("worktree 目前在 {}，不是 {branch}", if cur.is_empty() { "分離的 HEAD" } else { cur })));
    }
    if !strip(&git.run(&["status", "--porcelain"])?.stdout).is_empty() {
        return Err(Halt("worktree 不乾淨（有未 commit 或未追蹤的檔案）".into()));
    }
    if git.resolve(base)?.is_empty() { return Err(Halt(format!("找不到 base {base}"))); }
    let head_ref = format!("refs/heads/{branch}");

    if strategy == "rebase" {
        let Some(date) = a.truthy("--date").filter(|d| date_re().is_match(d)) else {
            return Err(Halt("rebase 要 --date YYYYMMDD（備份 ref 用）".into()));
        };
        let backup = format!("refs/backup/{}-{date}", branch.replace('/', "-"));
        let r = git.run(&["update-ref", &backup, &head_ref, ""])?; // 舊值給空字串＝不存在才寫
        if r.code != 0 { return Err(Halt(format!("備份 ref {backup} 寫不進去（可能已存在）：{}", tail(strip(&r.stderr), 200)))); }
        rec.insert("backup_ref".into(), json!(backup));
        let r = git.run(&["rebase", base])?;
        if r.code != 0 {
            rec.insert("conflicts".into(), json!(git.unmerged()?));
            git.run(&["rebase", "--abort"])?;
            return finish(&git, rec, 3, format!("rebase 失敗，已 abort：{}", tail(strip(r.message()), 200)));
        }
        return finish(&git, rec, 0, format!("已 rebase 到 {base}，原分支備份在 {backup}"));
    }
    if strategy == "merge" {
        let r = git.run(&["merge", "--no-ff", "--no-edit", base])?;
        if r.code != 0 {
            rec.insert("conflicts".into(), json!(git.unmerged()?));
            git.run(&["merge", "--abort"])?;
            return finish(&git, rec, 3, format!("merge 失敗，已 abort：{}", tail(strip(r.message()), 200)));
        }
        return finish(&git, rec, 0, format!("已把 {base} merge 進 {branch}"));
    }
    // transit
    let Some(transit) = a.truthy("--transit-branch") else {
        return Err(Halt("transit 要 --transit-branch（用 plan 輸出的 transit_branch）".into()));
    };
    git.check_branch_name(transit, &prefixes(&git.repo, a.truthy("--config"))?)?;
    if !git.resolve(&format!("refs/heads/{transit}"))?.is_empty() { return Err(Halt(format!("中轉分支 {transit} 已存在"))); }
    git.must(&["switch", "--quiet", "--no-track", "-c", transit, base])?;
    let r = git.run(&["merge", "--no-ff", "--no-edit", &head_ref])?;
    if r.code != 0 {
        let conflicts = git.unmerged()?;
        let any = !conflicts.is_empty();
        rec.insert("conflicts".into(), json!(conflicts));
        if any { return finish(&git, rec, 2, "有衝突，已留在中轉分支的 worktree；解完 commit 後跑 check-transit".into()); }
        return finish(&git, rec, 1, format!("merge 失敗但沒有衝突檔，停下交人：{}", tail(strip(r.message()), 200)));
    }
    finish(&git, rec, 0, format!("已在 {transit} 乾淨合併 {branch}"))
}

// ------------------------------------------------------------------ check-transit

fn is_marker(line: &str) -> bool {
    line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> ") || line == "======="
}

fn cmd_check_transit(ctx: &Ctx, a: &Parsed, rec: &mut Rec) -> R<i32> {
    let git = Git { ctx, repo: pyutil::resolve(a.get("--repo").unwrap_or("")) };
    let (branch, base) = (a.get("--branch").unwrap_or(""), a.get("--base").unwrap_or(""));
    let mut expected: Vec<String> = a.get("--expected").unwrap_or("").split(',').map(|x| strip(x).replace('\\', "/")).filter(|x| !x.is_empty()).collect();
    expected.sort();
    expected.dedup();
    set(rec, json!({"branch": branch, "base": base, "merge_commit": null, "expected": expected,
        "marker_files": [], "unexpected_files": [], "reasons": []}));
    let head_sha = git.resolve("HEAD")?;
    if head_sha.is_empty() { return Err(Halt("讀不到 HEAD".into())); }
    rec.insert("merge_commit".into(), json!(head_sha));
    let head_ref = format!("refs/heads/{branch}");
    for reference in [base, head_ref.as_str()] {
        if git.resolve(reference)?.is_empty() { return Err(Halt(format!("找不到 {reference}"))); }
    }
    let out = git.must(&["rev-list", "--parents", "-n", "1", "HEAD"])?;
    let parents: Vec<&str> = out.split(pyutil::is_py_space).filter(|x| !x.is_empty()).skip(1).collect();
    // 理由隨時寫回 rec：中途停下時，已經發現的也留在輸出裡
    let mut reasons: Vec<String> = Vec::new();
    fn note(rec: &mut Rec, reasons: &mut Vec<String>, text: String) {
        reasons.push(text);
        rec.insert("reasons".into(), json!(reasons));
    }
    if parents.len() != 2 { note(rec, &mut reasons, format!("HEAD 不是兩個 parent 的 merge commit（parent 數 {}）", parents.len())); }
    if !git.is_ancestor(base, "HEAD")? { note(rec, &mut reasons, format!("{base} 不是 HEAD 的祖先")); }
    if !git.is_ancestor(&head_ref, "HEAD")? { note(rec, &mut reasons, format!("{branch} 不是 HEAD 的祖先")); }
    let mut marker_files: Vec<String> = Vec::new();
    for f in lines(&git.must(&["diff", "--name-only", "--diff-filter=d", base, "HEAD"])?) {
        let blob = git.run(&["show", &format!("HEAD:{f}")])?;
        if blob.code != 0 || blob.raw.contains(&0) { continue; } // 二進位檔不掃標記
        let text = String::from_utf8_lossy(&blob.raw);
        if pyutil::splitlines(&text).iter().any(|l| is_marker(l)) {
            marker_files.push(f);
            rec.insert("marker_files".into(), json!(marker_files));
        }
    }
    if !marker_files.is_empty() { note(rec, &mut reasons, format!("還有衝突標記：{}", marker_files.join("、"))); }
    if let [p1, p2] = parents[..] {
        // 整棵樹和自動合併結果比：只看 git show --cc（跟兩個 parent 都不同的檔）會漏掉「把某一邊的改動默默丟掉」
        // （檔案退回成其中一個 parent 的版本），所以用 diff 比對全部檔案，含新增與刪除
        let (auto_tree, _) = git.merge_tree(p1, p2)?;
        let mut unexpected: Vec<String> = Vec::new();
        for f in lines(&git.must(&["diff", "--name-only", "--no-renames", &auto_tree, "HEAD"])?) {
            if !expected.contains(&f) {
                unexpected.push(f);
                rec.insert("unexpected_files".into(), json!(unexpected));
            }
        }
        if !unexpected.is_empty() {
            note(rec, &mut reasons, format!("merge commit 改了預期以外的檔案（和自動合併結果不同）：{}", unexpected.join("、")));
        }
    }
    rec.insert("ok".into(), json!(reasons.is_empty()));
    rec.insert("reason".into(), json!(if reasons.is_empty() { "中轉分支檢查通過".to_string() } else { reasons.join("；") }));
    Ok(if reasons.is_empty() { 0 } else { 1 })
}

// ------------------------------------------------------------------ CLI

const PLAN: &[Opt] = &[
    req("--repo", Kind::Str), req("--branch", Kind::Str), opt("--base", Kind::Str), opt("--remote", Kind::Str),
    opt("--strategy", Kind::Choice(STRATEGIES)), opt("--short", Kind::Str), opt("--date", Kind::Str),
    opt("--config", Kind::Str), opt("--methodology", Kind::Str), opt("--no-fetch", Kind::Flag),
];
const APPLY: &[Opt] = &[
    req("--repo", Kind::Str), req("--branch", Kind::Str), req("--strategy", Kind::Choice(&["rebase", "merge", "transit"])),
    opt("--base", Kind::Str), opt("--transit-branch", Kind::Str), opt("--date", Kind::Str), opt("--config", Kind::Str),
];
const CHECK_TRANSIT: &[Opt] = &[req("--repo", Kind::Str), req("--branch", Kind::Str), req("--base", Kind::Str), req("--expected", Kind::Str)];

/// 執行一個子指令：回傳（exit code、輸出的 JSON）。`-h`／`--help` 回 None（呼叫端印說明、exit 0）。
pub fn execute(ctx: &Ctx, argv: &[String]) -> Option<(i32, Value)> {
    let mut rec = Rec::new();
    rec.insert("halted".into(), json!(false));
    rec.insert("reason".into(), json!(""));
    let parsed = pyargs::split_cmd(argv, &["plan", "apply", "check-transit"]).and_then(|(cmd, rest, extras)| {
        let opts = match cmd {
            "plan" => PLAN,
            "apply" => APPLY,
            _ => CHECK_TRANSIT,
        };
        pyargs::parse(opts, rest, extras).map(|a| (cmd, a))
    });
    let result = match parsed {
        Err(ArgErr::Help) => return None,
        // 參數錯誤一律當成停下（exit 1＋JSON），不和 apply transit 的 exit 2 混在一起
        Err(e) => Err(Halt(format!("參數錯誤：{}", e.message()))),
        Ok((cmd, a)) => {
            rec.insert("command".into(), json!(cmd));
            match cmd {
                "plan" => cmd_plan(ctx, &a, &mut rec),
                "apply" => cmd_apply(ctx, &a, &mut rec),
                _ => cmd_check_transit(ctx, &a, &mut rec),
            }
        }
    };
    let code = match result {
        Ok(code) => code,
        Err(Halt(reason)) => {
            rec.insert("halted".into(), json!(true));
            rec.insert("reason".into(), json!(reason));
            1
        }
    };
    Some((code, Value::Object(rec)))
}

/// CLI 的本體：stdout 的文字寫進 `out`，回傳 exit code。
pub fn run(ctx: &Ctx, argv: &[String], out: &mut String) -> i32 {
    if argv.iter().any(|a| a == "--selftest") { return selftest(ctx, out); }
    match execute(ctx, argv) {
        None => {
            out.push_str(USAGE);
            0
        }
        Some((code, rec)) => {
            out.push_str(&dumps(&rec, Some(2)));
            out.push('\n');
            code
        }
    }
}

/// `awf sync-branch …`
pub fn main(args: &[String]) -> i32 {
    let mut out = String::new();
    let code = run(&Ctx::default(), args, &mut out);
    print!("{out}");
    code
}

// ------------------------------------------------------------------ selftest

struct Tester<'a> {
    ctx: Ctx,
    repo: String,
    out: &'a mut String,
    ok: bool,
    verbose: bool,
}

impl Tester<'_> {
    fn git_in(&self, dir: Option<&str>, args: &[&str]) -> Result<String, String> {
        let mut c = Command::new("git");
        if let Some(d) = dir { c.arg("-C").arg(d); }
        c.args(args);
        for (k, v) in &self.ctx.env { c.env(k, v); }
        match pyutil::run(&mut c, None, None) {
            Ok(r) if r.code == 0 => Ok(strip(&pyutil::text(&r.stdout)).to_string()),
            Ok(r) => Err(format!("git {args:?}：{}", pyutil::text(&r.stderr))),
            Err(_) => Err(format!("執行不了 git {args:?}")),
        }
    }

    fn g(&self, args: &[&str]) -> Result<String, String> {
        self.git_in(Some(&self.repo), args)
    }

    fn put(&self, path: &str, text: &str) -> Result<(), String> {
        std::fs::write(format!("{}/{path}", self.repo), text).map_err(|e| e.to_string())
    }

    fn commit(&self, path: &str, text: &str, msg: &str) -> Result<(), String> {
        self.put(path, text)?;
        self.g(&["add", "-A"])?;
        self.g(&["commit", "-qm", msg]).map(|_| ())
    }

    fn case(&mut self, name: &str, argv: &[&str], want_code: i32, want: Value) -> Value {
        let argv: Vec<String> = argv.iter().map(|x| x.to_string()).collect();
        let (code, rec) = execute(&self.ctx, &argv).unwrap_or((0, Value::Null));
        let good = code == want_code && want.as_object().is_some_and(|w| w.iter().all(|(k, v)| pyfmt::eq(g(&rec, k), v)));
        let detail = if good && !self.verbose { String::new() } else { format!(" {}", dumps(&rec, None)) };
        self.out.push_str(&format!("  [{}] {name}：exit {code}（預期 {want_code}）{detail}\n", if good { "通過" } else { "失敗" }));
        self.ok = self.ok && good;
        rec
    }

    fn expect(&mut self, name: &str, cond: bool) {
        self.out.push_str(&format!("  [{}] {name}\n", if cond { "通過" } else { "失敗" }));
        self.ok = self.ok && cond;
    }

    fn transit_again(&mut self, t: &str) -> Result<(), String> {
        self.g(&["switch", "-q", "feature/c"])?;
        let argv: Vec<String> = ["apply", "--repo", &self.repo, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", t].iter().map(|x| x.to_string()).collect();
        match execute(&self.ctx, &argv) {
            Some((2, _)) => Ok(()),
            _ => Err(format!("預期 {t} 有衝突")),
        }
    }
}

/// 在暫存 git repo（bare repo 當 origin）把每種策略與檢查走一遍。回傳 exit code。
pub fn selftest(ctx: &Ctx, out: &mut String) -> i32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("awf-sync-branch-selftest-{}-{nanos}", std::process::id()));
    let root_text = root.to_string_lossy().into_owned();
    let result = std::fs::create_dir_all(&root).map_err(|e| e.to_string()).and_then(|_| selftest_body(ctx, &root_text, out));
    let _ = std::fs::remove_dir_all(&root);
    let ok = match result {
        Ok(ok) => ok,
        Err(e) => {
            out.push_str(&format!("  [失敗] selftest 中斷：{e}\n"));
            false
        }
    };
    out.push_str(&format!("sync_branch selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    if ok { 0 } else { 1 }
}

fn selftest_body(ctx: &Ctx, root: &str, out: &mut String) -> Result<bool, String> {
    let write = |path: String, text: &str| std::fs::write(&path, text).map_err(|e| format!("{path}：{e}"));
    write(format!("{root}/gitconfig"), "")?;
    // 不受使用者全域設定（簽章、hooksPath）影響
    let mut env = ctx.env.clone();
    env.push(("GIT_CONFIG_GLOBAL".into(), format!("{root}/gitconfig")));
    env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    let (origin, repo) = (format!("{root}/origin.git"), format!("{root}/app"));
    let mut s = Tester { ctx: Ctx { env }, repo: repo.clone(), out, ok: true, verbose: std::env::var_os("AWF_VERBOSE").is_some_and(|v| !v.is_empty()) };

    s.git_in(None, &["-c", "init.defaultBranch=main", "init", "-q", "--bare", &origin])?;
    s.git_in(None, &["-c", "init.defaultBranch=main", "init", "-q", &repo])?;
    for (k, v) in [("user.name", "t"), ("user.email", "t@example.invalid"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        s.g(&["config", k, v])?;
    }
    s.g(&["remote", "add", "origin", &origin])?;
    s.commit("README.md", "# 示範客戶 ACME\n", "init")?;
    s.commit("shared.txt", "標題\n內容\n", "shared")?;
    s.g(&["push", "-q", "origin", "main"])?;
    s.g(&["branch", "feature/up"])?;
    s.g(&["switch", "-q", "-c", "feature/a"])?;
    s.commit("a.txt", "a\n", "a")?;
    s.g(&["switch", "-q", "-c", "feature/b", "main"])?;
    s.commit("b.txt", "b\n", "b")?;
    s.g(&["push", "-q", "origin", "feature/b"])?;
    s.g(&["switch", "-q", "-c", "feature/c", "main"])?;
    s.commit("shared.txt", "標題\n功能分支的內容\n", "c")?;
    s.g(&["switch", "-q", "main"])?;
    s.commit("shared.txt", "標題\nmain 的內容\n", "main 改 shared")?;
    s.commit("m.txt", "m\n", "main 新檔")?;
    s.g(&["push", "-q", "origin", "main"])?;
    s.g(&["reset", "-q", "--hard", "HEAD~2"])?; // 本機 main 落後，確認工具只看 origin/main
    let r = repo.as_str();
    let date = "20261103";
    let cfg = format!("{root}/guard.config.json");
    write(cfg.clone(), &dumps(&json!({"allowed_branch_prefixes": ["feature/"]}), None))?;
    let meth = format!("{root}/methodology.json");
    write(meth.clone(), &dumps(&json!({"conventions": {"sync": {"strategy": "merge"}}}), None))?;
    let halted = json!({"halted": true});

    s.out.push_str("sync_branch selftest（暫存 git repo，bare repo 當 origin）\n");
    s.case("已跟上 → none", &["plan", "--repo", r, "--branch", "feature/up", "--base", "main"], 0, json!({"strategy": "none", "behind": 0}));
    s.case("未推送、無衝突 → rebase", &["plan", "--repo", r, "--branch", "feature/a"], 0, json!({"strategy": "rebase", "published": false, "behind": 2}));
    s.case("已推送、無衝突 → merge", &["plan", "--repo", r, "--branch", "feature/b"], 0, json!({"strategy": "merge", "published": true}));
    let rec = s.case("有衝突 → transit", &["plan", "--repo", r, "--branch", "feature/c", "--date", date], 0,
        json!({"strategy": "transit", "transit_branch": "merge/c-into-main-20261103", "conflicts": ["shared.txt"]}));
    s.case("methodology 指定 merge → merge", &["plan", "--repo", r, "--branch", "feature/a", "--methodology", &meth], 0, json!({"strategy": "merge", "requested_strategy": "merge"}));
    s.case("固定 rebase 但已推送 → 停下", &["plan", "--repo", r, "--branch", "feature/b", "--strategy", "rebase"], 1, halted.clone());
    s.case("固定 merge 但有衝突 → 停下", &["plan", "--repo", r, "--branch", "feature/c", "--strategy", "merge"], 1, halted.clone());
    s.case("transit 前綴不在白名單 → 停下", &["plan", "--repo", r, "--branch", "feature/c", "--date", date, "--config", &cfg], 1, halted.clone());
    s.case("transit 沒有 --date → 停下", &["plan", "--repo", r, "--branch", "feature/c"], 1, halted.clone());
    s.case("fetch 失敗 → 停下", &["plan", "--repo", r, "--branch", "feature/a", "--remote", "nope"], 1, halted.clone());
    s.case("參數錯誤 → 停下（exit 1）", &["plan", "--repo", r], 1, halted.clone());

    s.g(&["switch", "-q", "feature/a"])?;
    let old_a = s.g(&["rev-parse", "feature/a"])?;
    s.case("apply rebase", &["apply", "--repo", r, "--branch", "feature/a", "--strategy", "rebase", "--date", date], 0,
        json!({"backup_ref": "refs/backup/feature-a-20261103", "head_branch": "feature/a"}));
    let linear = s.g(&["rev-parse", "refs/backup/feature-a-20261103"])? == old_a && s.g(&["rev-list", "--merges", "origin/main..feature/a"])?.is_empty()
        && s.g(&["merge-base", "--is-ancestor", "origin/main", "feature/a"]).is_ok();
    s.expect("rebase 留下備份 ref、歷史線性且接在 origin/main 之後", linear);
    s.case("同一天再 rebase：備份 ref 已存在 → 停下", &["apply", "--repo", r, "--branch", "feature/a", "--strategy", "rebase", "--date", date], 1, halted.clone());
    s.case("apply 時 worktree 不在該分支 → 停下", &["apply", "--repo", r, "--branch", "feature/b", "--strategy", "merge"], 1, halted);
    s.g(&["switch", "-q", "feature/b"])?;
    s.case("apply merge", &["apply", "--repo", r, "--branch", "feature/b", "--strategy", "merge"], 0, json!({"head_branch": "feature/b"}));
    let parents = s.g(&["rev-list", "--parents", "-n", "1", "HEAD"])?;
    s.expect("merge 後 HEAD 是 merge commit", parents.split_whitespace().count() == 3);

    s.g(&["switch", "-q", "feature/c"])?;
    let t1 = rec["transit_branch"].as_str().unwrap_or("").to_string();
    s.case("apply transit 有衝突 → exit 2", &["apply", "--repo", r, "--branch", "feature/c", "--strategy", "transit", "--transit-branch", &t1], 2,
        json!({"conflicts": ["shared.txt"], "head_branch": t1}));
    let resolved = "標題\nmain 的內容\n功能分支的內容\n";
    s.commit("shared.txt", resolved, "merge: 解衝突")?;
    let ct = |expected: &'static str| vec!["check-transit", "--repo", r, "--branch", "feature/c", "--base", "origin/main", "--expected", expected];
    s.case("解完衝突 commit → check-transit 通過", &ct("shared.txt"), 0, json!({"ok": true}));
    s.case("--expected 沒列衝突檔 → 不通過", &ct(""), 1, json!({"unexpected_files": ["shared.txt"]}));

    s.transit_again("merge/c-into-main-20261104")?;
    s.put("shared.txt", resolved)?;
    s.put("README.md", "# 示範客戶 ACME（夾帶）\n")?;
    s.g(&["add", "-A"])?;
    s.g(&["commit", "-qm", "merge: 解衝突＋夾帶"])?;
    s.case("merge commit 夾帶無關檔案 → 不通過", &ct("shared.txt"), 1, json!({"unexpected_files": ["README.md"]}));

    s.transit_again("merge/c-into-main-20261106")?;
    s.put("shared.txt", resolved)?;
    s.g(&["rm", "-q", "m.txt"])?; // 把 main 那邊沒衝突的改動默默丟掉（m.txt 等於功能分支那個 parent，git show --cc 看不到）
    s.g(&["add", "-A"])?;
    s.g(&["commit", "-qm", "merge: 解衝突＋丟掉 main 的改動"])?;
    s.case("解衝突時丟掉另一邊沒衝突的改動 → 不通過", &ct("shared.txt"), 1, json!({"unexpected_files": ["m.txt"]}));

    s.transit_again("merge/c-into-main-20261105")?;
    s.g(&["add", "-A"])?;
    s.g(&["commit", "-qm", "merge: 留著衝突標記"])?;
    s.case("留著衝突標記 → 不通過", &ct("shared.txt"), 1, json!({"marker_files": ["shared.txt"]}));
    s.case("HEAD 不是 merge commit → 不通過", &["check-transit", "--repo", r, "--branch", "feature/a", "--base", "origin/main", "--expected", ""], 1, json!({"ok": false}));
    s.g(&["switch", "-q", "feature/c"])?;
    s.case("apply merge 有衝突 → abort、exit 3", &["apply", "--repo", r, "--branch", "feature/c", "--strategy", "merge"], 3, json!({"conflicts": ["shared.txt"], "head_branch": "feature/c"}));
    s.case("apply rebase 有衝突 → abort、exit 3", &["apply", "--repo", r, "--branch", "feature/c", "--strategy", "rebase", "--date", date], 3, json!({"conflicts": ["shared.txt"], "head_branch": "feature/c"}));
    let clean = s.g(&["status", "--porcelain"])?.is_empty() && s.g(&["rev-parse", "feature/c"])? == s.g(&["rev-parse", "refs/backup/feature-c-20261103"])?;
    s.expect("abort 後 worktree 乾淨、分支沒動", clean);
    Ok(s.ok)
}
