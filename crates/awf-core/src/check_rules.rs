//! 掃描產出是否違反硬規則（`.claude/teams/rules.md`），只做機械判定。設定讀 `.claude/teams/guard.config.json`。
//! 原本是 0.x 的 Python 工具 `check_rules.py`；訊息文字是對跑基準的一部分（`tests/parity/small.expected.json`）。
//!
//! 和 0.x 的 Python 版的已知差異：`internal_id_patterns`、`check_regex` 由 fancy-regex 解讀；寫壞時 0.x 的 Python 版直接當掉，
//! 這裡印一行說明後 exit 1（`check_regex` 照舊只警告）。

use crate::kb::pyish::{self, ppath, WS_CLASS};
use crate::pyfmt::{repr_str, truthy};
use crate::run_brief::splitlines;
use crate::schema::Schemas;
use fancy_regex::Regex as FRegex;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const USAGE: &str = "掃描產出是否違反硬規則（.claude/teams/rules.md），只做機械判定。設定讀 .claude/teams/guard.config.json。

用法：
    awf check-rules [--audience internal|owner|external] <檔案…>
    awf check-rules --git <repo> <range>        # 檢查 commit 訊息，例如 origin/main..HEAD
";

const OUTWARD: &[&str] = &["owner", "external"];

struct Ctx {
    backlog: PathBuf,
    attribution: String,
    attr_key: String,
    forbidden: Vec<String>,
    methodology: PathBuf,
    estimate: Regex,
    tnum: Option<crate::hooks::re::Re>,
    internal: Vec<FRegex>,
    judge: Regex,
}

fn py_re(src: &str) -> Regex {
    Regex::new(&src.replace(r"\S", &format!("[^{WS_CLASS}]")).replace(r"\s", &format!("[{WS_CLASS}]"))).expect("固定的正規式")
}

fn norm(p: &str) -> String {
    if cfg!(windows) { p.to_string() } else { ppath(p) }
}

fn cfg_str(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => default.to_string(),
    }
}

impl Ctx {
    fn new(project: &Path) -> Result<Ctx, String> {
        let teams = project.join(".claude").join("teams");
        let sch = Schemas::new(&teams);
        let cfg = sch.config(None);
        let attribution = pyish::strip(&cfg_str(cfg.get("attribution_line"), "Assisted-by: Claude")).to_string();
        let attr_key = pyish::strip(attribution.split(':').next().unwrap_or_default()).to_string();
        let trailers: Vec<String> = match cfg.get("forbidden_trailers") {
            Some(v @ Value::Array(a)) if truthy(v) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            _ => vec!["Co-Authored-By".to_string()],
        };
        let forbidden = trailers.into_iter().filter(|t| t.to_lowercase() != attr_key.to_lowercase()).collect();
        let mut internal = Vec::new();
        if let Some(Value::Array(a)) = cfg.get("internal_id_patterns") {
            for p in a.iter().filter_map(Value::as_str) {
                internal.push(FRegex::new(p).map_err(|e| format!("internal_id_patterns 的「{p}」不是合法的正規式：{e}"))?);
            }
        }
        Ok(Ctx {
            backlog: project.join(cfg_str(cfg.get("backlog_file"), "_tpm/tasks/backlog.md")),
            attribution,
            attr_key,
            forbidden,
            methodology: teams.join("methodology.json"),
            estimate: py_re(r"(?i)\d+(?:\.\d+)?\s*(?:個)?\s*(?:人天|人日|人月|man[- ]?days?|person[- ]?days?)|(?:預估|估計|約)\s*\d+(?:\.\d+)?\s*(?:小時|天)\s*(?:工時|工作量)"),
            tnum: sch.task_id_regex(),
            internal,
            judge: Regex::new("能力不足|不專業|偷懶|很差勁|太弱|不夠聰明|擺爛|不負責任").expect("固定的正規式"),
        })
    }

    fn tnum_all(&self, text: &str) -> Vec<String> {
        match &self.tnum {
            Some(re) => re.find_all(text).into_iter().map(|(a, b)| text[a..b].to_string()).collect(),
            None => Vec::new(),
        }
    }

    fn known_ids(&self) -> HashSet<String> {
        if self.tnum.is_none() {
            return HashSet::new();
        }
        let Ok(bytes) = std::fs::read(&self.backlog) else { return HashSet::new() };
        let text = String::from_utf8_lossy(&bytes);
        let ids: Vec<&str> = py_re(r"id:(\S+)").captures_iter(&text).filter_map(|c| c.get(1).map(|m| m.as_str())).collect();
        self.tnum_all(&ids.join(" ")).into_iter().collect()
    }

    fn trailer_re(tr: &str, multiline: bool) -> Regex {
        Regex::new(&format!("(?i{})^{}:", if multiline { "m" } else { "" }, regex::escape(tr))).expect("跳脫過的字串")
    }

    fn scan_file(&self, path: &str, audience: &str, ids: &HashSet<String>) -> Result<Vec<String>, String> {
        let shown = norm(path);
        let bytes = std::fs::read(path).map_err(|e| format!("讀不到 {shown}：{e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        let outward = OUTWARD.contains(&audience);
        let trailers: Vec<(&String, Regex)> = self.forbidden.iter().map(|t| (t, Self::trailer_re(t, false))).collect();
        let mut out = Vec::new();
        for (i, line) in splitlines(&text).into_iter().enumerate() {
            let loc = format!("{shown}:{}", i + 1);
            if let Some(m) = self.estimate.find(line) {
                out.push(format!("R1 {loc}：工時估算「{}」", m.as_str()));
            }
            for t in self.tnum_all(line) {
                if outward {
                    out.push(format!("R3 {loc}：給 {audience} 的文件不寫內部編號 {t}"));
                } else if !ids.is_empty() && !ids.contains(&t) {
                    out.push(format!("R2 {loc}：{t} 不在 backlog，疑似自編任務編號"));
                }
            }
            if outward {
                for pat in &self.internal {
                    for d in findall(pat, line) {
                        out.push(format!("R3 {loc}：給 {audience} 的文件不寫內部編號 {d}"));
                    }
                }
                if let Some(m) = self.judge.find(line) {
                    out.push(format!("R5 {loc}：評價人的用語「{}」", m.as_str()));
                }
            }
            for (tr, re) in &trailers {
                if re.is_match(line) {
                    out.push(format!("R4 {loc}：用 {}，不用 {tr}", self.attr_key));
                }
            }
        }
        Ok(out)
    }

    /// 開發規範 profile 的 commit 標題格式；沒有 profile、沒設或設 null 時回 None。寫壞時多回一行警告。
    fn commit_regex(&self) -> (Option<(FRegex, String)>, Option<String>) {
        let rx = std::fs::read_to_string(&self.methodology).ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("conventions").and_then(|c| c.get("commit")).and_then(|c| c.get("check_regex")).cloned());
        let Some(Value::String(rx)) = rx else { return (None, None) };
        if pyish::is_blank(&rx) {
            return (None, None);
        }
        match FRegex::new(&rx) {
            Ok(re) => (Some((re, rx)), None),
            Err(_) => (None, Some(format!("警告：{} 的 conventions.commit.check_regex 不是合法的正規式，略過格式檢查", norm(&self.methodology.to_string_lossy())))),
        }
    }

    fn scan_git(&self, repo: &str, rng: &str, out_text: &mut String) -> Result<Vec<String>, String> {
        let res = Command::new("git").args(["-C", repo, "log", "--format=%h%x00%B%x1e", rng]).output().map_err(|e| format!("跑不了 git：{e}"))?;
        if !res.status.success() {
            return Err(format!("git log 失敗：{}", String::from_utf8_lossy(&res.stderr).trim_end()));
        }
        let log = String::from_utf8_lossy(&res.stdout).replace("\r\n", "\n").replace('\r', "\n");
        let (subject_rx, warn) = self.commit_regex();
        if let Some(w) = warn {
            out_text.push_str(&w);
            out_text.push('\n');
        }
        let attr_re = Self::trailer_re(&self.attr_key, true);
        let mut out = Vec::new();
        for rec in log.split('\x1e').filter(|r| !pyish::is_blank(r)) {
            let rec = pyish::strip(rec);
            let (sha, body) = rec.split_once('\0').unwrap_or((rec, ""));
            let subject = splitlines(pyish::strip(body)).first().copied().unwrap_or_default();
            if let Some((re, pattern)) = &subject_rx {
                if !re.is_match(subject).unwrap_or(false) {
                    let head: String = subject.chars().take(60).collect();
                    out.push(format!("R6 commit {sha}：標題「{head}」不符合開發規範 profile 的 commit 格式（{pattern}）"));
                }
            }
            for tr in &self.forbidden {
                if Self::trailer_re(tr, true).is_match(body) {
                    out.push(format!("R4 commit {sha}：用 {}，不用 {tr}", self.attr_key));
                }
            }
            if !attr_re.is_match(body) {
                out.push(format!("R4 commit {sha}：缺 {} trailer（署名行：{}）", self.attr_key, self.attribution));
            }
        }
        Ok(out)
    }
}

/// Python 的 `pattern.findall(line)`，每筆轉成 `str()` 的樣子：沒有群組是整段、一個群組是該群組、多個群組是 tuple。
fn findall(re: &FRegex, line: &str) -> Vec<String> {
    let groups = re.captures_len().saturating_sub(1);
    re.captures_iter(line).filter_map(Result::ok).map(|c| {
        let g = |i: usize| c.get(i).map(|m| m.as_str()).unwrap_or_default();
        match groups {
            0 => g(0).to_string(),
            1 => g(1).to_string(),
            n => format!("({})", (1..=n).map(|i| repr_str(g(i))).collect::<Vec<_>>().join(", ")),
        }
    }).collect()
}

/// 回（exit code，stdout，stderr）。`project` 是專案根目錄。
pub fn run_captured(argv: &[String], project: &Path) -> (i32, String, String) {
    let ctx = match Ctx::new(project) {
        Ok(c) => c,
        Err(e) => return (1, String::new(), format!("awf check-rules：{e}\n")),
    };
    let mut out = String::new();
    let errs = if argv.first().map(String::as_str) == Some("--git") {
        let (Some(repo), Some(rng)) = (argv.get(1), argv.get(2)) else {
            return (1, String::new(), format!("awf check-rules：--git 要給 <repo> 與 <range>\n\n{USAGE}"));
        };
        ctx.scan_git(repo, rng, &mut out)
    } else {
        let mut files: Vec<String> = argv.to_vec();
        let mut audience = "internal".to_string();
        if let Some(i) = files.iter().position(|a| a == "--audience") {
            let Some(a) = files.get(i + 1).cloned() else {
                return (1, String::new(), format!("awf check-rules：--audience 後面要接 internal、owner 或 external\n\n{USAGE}"));
            };
            audience = a;
            files.drain(i..i + 2);
        }
        if audience != "internal" && !OUTWARD.contains(&audience.as_str()) {
            return (2, format!("--audience 只能是 internal、owner、external（收到 {audience}）\n"), String::new());
        }
        let ids = ctx.known_ids();
        let mut all = Vec::new();
        let mut failed = None;
        for f in &files {
            match ctx.scan_file(f, &audience, &ids) {
                Ok(v) => all.extend(v),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(all),
        }
    };
    let errs = match errs {
        Ok(e) => e,
        Err(e) => return (1, out, format!("awf check-rules：{e}\n")),
    };
    for e in &errs {
        out.push_str(e);
        out.push('\n');
    }
    let verdict = if errs.is_empty() { "通過".to_string() } else { format!("{} 項違規", errs.len()) };
    out.push_str(&format!("規則檢查：{verdict}\n"));
    (if errs.is_empty() { 0 } else { 1 }, out, String::new())
}

/// `awf check-rules …`（專案根目錄照 [`crate::project`] 的解析，找不到就用目前目錄）
pub fn main(argv: &[String]) -> i32 {
    let project = PathBuf::from(crate::project::root_or_cwd(None));
    let (code, out, err) = run_captured(argv, &project);
    print!("{out}");
    eprint!("{err}");
    code
}
