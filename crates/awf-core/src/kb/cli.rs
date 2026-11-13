//! `awf kb …` 的命令列：參數解析照 argparse 的規則（含錯誤訊息），任何錯誤都印一行並 exit 0。

use super::index::{build, BuildOpts};
use super::pyish::{self, PyErr};
use super::search::{format_results, lint, results_json, search, QueryOpts};
use super::text::to_posix;
use super::{selftest, swap_err_sink, warn, DB_NAME};
use crate::pyfmt::repr_str;
use std::io::Write;

// 說明文字照 argparse 在 80 欄寬時的輸出（程式名改成 awf kb，續行多縮一格）。
const HELP_TOP: &str = concat!(
    "usage: awf kb [-h] {build,query,lint} ...\n",
    "\n",
    "知識層：先查到哪個檔的哪幾行，再只讀那幾行\n",
    "\n",
    "positional arguments:\n",
    "  {build,query,lint}\n",
    "    build             建立或增量更新索引\n",
    "    query             查詢（唯讀）\n",
    "    lint              壞連結與孤兒頁\n",
    "\n",
    "options:\n",
    "  -h, --help          show this help message and exit\n",
);
const HELP_BUILD: &str = concat!(
    "usage: awf kb build [-h] [--root ROOT] [--profile PROFILE] [--dir DIR]\n",
    "                    [--sources SOURCES [SOURCES ...]]\n",
    "                    [--exclude EXCLUDE [EXCLUDE ...]]\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --root ROOT           專案根目錄（預設：工具往上三層）\n",
    "  --profile PROFILE     開發規範 profile（預設 <root>/.claude/teams/methodology.json）\n",
    "  --dir DIR             索引目錄（預設取 profile 的 knowledge.dir，再沒有就是 _tpm/kb）\n",
    "  --sources SOURCES [SOURCES ...]\n",
    "                        要索引的 glob（可多個，或用逗號分隔）\n",
    "  --exclude EXCLUDE [EXCLUDE ...]\n",
    "                        排除的 glob\n",
);
const HELP_QUERY: &str = concat!(
    "usage: awf kb query [-h] [--root ROOT] [--profile PROFILE] [--dir DIR]\n",
    "                    [--top TOP] [--budget BUDGET] [--json]\n",
    "                    [--engine {auto,fts5,bm25}]\n",
    "                    text\n",
    "\n",
    "positional arguments:\n",
    "  text                  問題\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --root ROOT           專案根目錄（預設：工具往上三層）\n",
    "  --profile PROFILE     開發規範 profile（預設 <root>/.claude/teams/methodology.json）\n",
    "  --dir DIR             索引目錄（預設取 profile 的 knowledge.dir，再沒有就是 _tpm/kb）\n",
    "  --top TOP             最多幾筆（預設 8）\n",
    "  --budget BUDGET       輸出總字數上限（預設 4000）\n",
    "  --json                輸出 JSON\n",
    "  --engine {auto,fts5,bm25}\n",
);
const HELP_LINT: &str = concat!(
    "usage: awf kb lint [-h] [--root ROOT] [--profile PROFILE] [--dir DIR]\n",
    "\n",
    "options:\n",
    "  -h, --help         show this help message and exit\n",
    "  --root ROOT        專案根目錄（預設：工具往上三層）\n",
    "  --profile PROFILE  開發規範 profile（預設 <root>/.claude/teams/methodology.json）\n",
    "  --dir DIR          索引目錄（預設取 profile 的 knowledge.dir，再沒有就是 _tpm/kb）\n",
);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Help,
    Flag,
    One,
    Plus,
}

struct Opt {
    names: &'static [&'static str],
    kind: Kind,
}

const fn opt(names: &'static [&'static str], kind: Kind) -> Opt {
    Opt { names, kind }
}

const BUILD_OPTS: &[Opt] = &[opt(&["-h", "--help"], Kind::Help), opt(&["--root"], Kind::One), opt(&["--profile"], Kind::One), opt(&["--dir"], Kind::One),
    opt(&["--sources"], Kind::Plus), opt(&["--exclude"], Kind::Plus)];
const QUERY_OPTS: &[Opt] = &[opt(&["-h", "--help"], Kind::Help), opt(&["--root"], Kind::One), opt(&["--profile"], Kind::One), opt(&["--dir"], Kind::One),
    opt(&["--top"], Kind::One), opt(&["--budget"], Kind::One), opt(&["--json"], Kind::Flag), opt(&["--engine"], Kind::One)];
const LINT_OPTS: &[Opt] = &[opt(&["-h", "--help"], Kind::Help), opt(&["--root"], Kind::One), opt(&["--profile"], Kind::One), opt(&["--dir"], Kind::One)];
const TOP_OPTS: &[Opt] = &[opt(&["-h", "--help"], Kind::Help)];

/// 解析的中途結束：印說明，或參數錯誤（訊息照 argparse）。
enum Stop {
    Help(&'static str),
    Error(String),
}

enum Class<'a> {
    Pos,
    Unknown,
    Opt(&'static Opt, Option<&'a str>),
}

fn find(opts: &'static [Opt], name: &str) -> Option<&'static Opt> {
    opts.iter().find(|o| o.names.contains(&name))
}

fn negative_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else { return false };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    digits(rest) || rest.split_once('.').is_some_and(|(a, b)| a.bytes().all(|x| x.is_ascii_digit()) && digits(b))
}

/// argparse 的 `_parse_optional`：這個字串是選項、位置參數，還是不認得的選項。
fn classify<'a>(arg: &'a str, opts: &'static [Opt]) -> Class<'a> {
    if !arg.starts_with('-') {
        return Class::Pos;
    }
    if let Some(o) = find(opts, arg) {
        return Class::Opt(o, None);
    }
    if arg.len() == 1 {
        return Class::Pos;
    }
    let split = arg.split_once('=');
    if let Some((name, explicit)) = split {
        if let Some(o) = find(opts, name) {
            return Class::Opt(o, Some(explicit));
        }
    }
    // 可以只打開頭幾個字（--to 就是 --top）
    let mut hits: Vec<(&'static Opt, Option<&'a str>)> = Vec::new();
    if arg.starts_with("--") {
        let (prefix, explicit) = match split {
            Some((p, e)) => (p, Some(e)),
            None => (arg, None),
        };
        for o in opts {
            if o.names.iter().any(|n| n.starts_with(prefix)) {
                hits.push((o, explicit));
            }
        }
    } else {
        let short = pyish::take(arg, 2);
        for o in opts {
            if o.names.contains(&short) {
                hits.push((o, Some(arg.get(short.len()..).unwrap_or(""))));
            } else if o.names.iter().any(|n| n.starts_with(arg)) {
                hits.push((o, None));
            }
        }
    }
    if let [(o, explicit)] = hits.as_slice() {
        return Class::Opt(o, *explicit);
    }
    if negative_number(arg) || arg.contains(' ') {
        return Class::Pos;
    }
    Class::Unknown
}

/// Python 的 `int(s)`：前後可以有空白、可以帶正負號、數字之間可以有底線。超出範圍的取邊界值。
fn py_int(s: &str) -> Option<i64> {
    let t = pyish::strip(s);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let ok = !digits.is_empty() && !digits.starts_with('_') && !digits.ends_with('_') && !digits.contains("__") && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if !ok {
        return None;
    }
    let mut v: i64 = 0;
    for b in digits.bytes().filter(u8::is_ascii_digit) {
        v = v.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    Some(if neg { -v } else { v })
}

#[derive(Default)]
struct Parsed {
    values: Vec<(&'static str, Vec<String>)>,
    text: Option<String>,
    extras: Vec<String>,
}

impl Parsed {
    fn one(&self, name: &str) -> Option<String> {
        self.values.iter().rev().find(|(n, _)| *n == name).and_then(|(_, v)| v.first().cloned())
    }
    fn many(&self, name: &str) -> Option<Vec<String>> {
        self.values.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| v.clone())
    }
    fn has(&self, name: &str) -> bool {
        self.values.iter().any(|(n, _)| *n == name)
    }
}

fn display(o: &Opt) -> String {
    o.names.join("/")
}

/// 子指令的參數。`positional`：有沒有位置參數 text（只有 query 有）。
fn parse_sub(args: &[String], opts: &'static [Opt], help: &'static str, positional: bool) -> Result<Parsed, Stop> {
    let mut p = Parsed::default();
    let mut text_at: Option<usize> = None;
    let mut i = 0;
    let is_arg = |s: Option<&String>| s.is_some_and(|s| s != "--" && matches!(classify(s, opts), Class::Pos));
    while let Some(a) = args.get(i) {
        if a == "--" {
            // 之後全部是位置參數；`--` 緊鄰 text 時被它吃掉，不然留在多出來的參數裡
            let rest = args.get(i + 1..).unwrap_or(&[]);
            if positional && p.text.is_none() {
                if let Some((first, more)) = rest.split_first() {
                    p.text = Some(first.clone());
                    p.extras.extend(more.iter().cloned());
                }
            } else if positional && i > 0 && text_at == Some(i - 1) {
                p.extras.extend(rest.iter().cloned());
            } else {
                p.extras.push(a.clone());
                p.extras.extend(rest.iter().cloned());
            }
            break;
        }
        match classify(a, opts) {
            Class::Pos => {
                if positional && p.text.is_none() {
                    p.text = Some(a.clone());
                    text_at = Some(i);
                } else {
                    p.extras.push(a.clone());
                }
                i += 1;
            }
            Class::Unknown => {
                p.extras.push(a.clone());
                i += 1;
            }
            Class::Opt(o, explicit) => {
                let name = o.names.last().copied().unwrap_or("");
                i += 1;
                match o.kind {
                    Kind::Help | Kind::Flag => {
                        if let Some(ex) = explicit {
                            // -hh 這種連寫的短選項可以；其他帶值的寫法不行
                            let rest = if o.kind == Kind::Help && !a.starts_with("--") { ex.trim_start_matches('h') } else { ex };
                            if ex.is_empty() || !rest.is_empty() || a.starts_with("--") {
                                return Err(Stop::Error(format!("argument {}: ignored explicit argument {}", display(o), repr_str(rest))));
                            }
                        }
                        if o.kind == Kind::Help {
                            return Err(Stop::Help(help));
                        }
                        p.values.push((name, Vec::new()));
                    }
                    Kind::One => {
                        let v = match explicit {
                            Some(ex) => ex.to_string(),
                            None if is_arg(args.get(i)) => {
                                i += 1;
                                args.get(i - 1).cloned().unwrap_or_default()
                            }
                            None => return Err(Stop::Error(format!("argument {}: expected one argument", display(o)))),
                        };
                        if matches!(name, "--top" | "--budget") && py_int(&v).is_none() {
                            return Err(Stop::Error(format!("argument {name}: invalid int value: {}", repr_str(&v))));
                        }
                        if name == "--engine" && !matches!(v.as_str(), "auto" | "fts5" | "bm25") {
                            return Err(Stop::Error(format!("argument {name}: invalid choice: {} (choose from 'auto', 'fts5', 'bm25')", repr_str(&v))));
                        }
                        p.values.push((name, vec![v]));
                    }
                    Kind::Plus => {
                        let mut vals = Vec::new();
                        match explicit {
                            Some(ex) => vals.push(ex.to_string()),
                            None => {
                                while is_arg(args.get(i)) {
                                    vals.extend(args.get(i).cloned());
                                    i += 1;
                                }
                            }
                        }
                        if vals.is_empty() {
                            return Err(Stop::Error(format!("argument {}: expected at least one argument", display(o))));
                        }
                        p.values.push((name, vals));
                    }
                }
            }
        }
    }
    if positional && p.text.is_none() {
        return Err(Stop::Error("the following arguments are required: text".into()));
    }
    Ok(p)
}

fn run(argv: &[String], out: &mut String) -> Result<(), PyErr> {
    // 最上層：-h、子指令名；子指令之前不認得的選項留到最後一起報
    let mut top_extras: Vec<String> = Vec::new();
    let mut cmd: Option<(&str, &[String])> = None;
    let mut stop: Option<Stop> = None;
    for (i, a) in argv.iter().enumerate() {
        if a != "--" {
            match classify(a, TOP_OPTS) {
                Class::Unknown => {
                    top_extras.push(a.clone());
                    continue;
                }
                Class::Opt(o, explicit) => {
                    let rest = explicit.map(|ex| if a.starts_with("--") { ex } else { ex.trim_start_matches('h') });
                    stop = Some(match (explicit, rest) {
                        (Some(ex), Some(r)) if ex.is_empty() || !r.is_empty() || a.starts_with("--") => {
                            Stop::Error(format!("argument {}: ignored explicit argument {}", display(o), repr_str(r)))
                        }
                        _ => Stop::Help(HELP_TOP),
                    });
                    break;
                }
                Class::Pos => {}
            }
        }
        cmd = Some((a.as_str(), argv.get(i + 1..).unwrap_or(&[])));
        break;
    }
    let parsed = match (stop, cmd) {
        (Some(s), _) => Err(s),
        (None, Some(("build", rest))) => parse_sub(rest, BUILD_OPTS, HELP_BUILD, false),
        (None, Some(("query", rest))) => parse_sub(rest, QUERY_OPTS, HELP_QUERY, true),
        (None, Some(("lint", rest))) => parse_sub(rest, LINT_OPTS, HELP_LINT, false),
        (None, Some((other, _))) => Err(Stop::Error(format!("argument cmd: invalid choice: {} (choose from 'build', 'query', 'lint')", repr_str(other)))),
        (None, None) => Ok(Parsed::default()),
    };
    let parsed = parsed.and_then(|p| {
        let extras: Vec<String> = top_extras.iter().chain(p.extras.iter()).cloned().collect();
        if extras.is_empty() { Ok(p) } else { Err(Stop::Error(format!("unrecognized arguments: {}", extras.join(" ")))) }
    });
    let a = match parsed {
        Ok(p) => p,
        Err(Stop::Help(text)) => {
            out.push_str(text);
            return Ok(());
        }
        Err(Stop::Error(msg)) => {
            // 參數錯也不擋流程：印一行，exit 0
            out.push_str(&format!("kb：參數錯誤：{msg}（用法見 awf kb --help）\n"));
            return Ok(());
        }
    };
    let (root, dir, profile) = (a.one("--root"), a.one("--dir"), a.one("--profile"));
    match cmd.map(|c| c.0) {
        Some("build") => {
            let st = build(&BuildOpts { root, sources: a.many("--sources"), exclude: a.many("--exclude"), kb_dir: dir, profile, ..Default::default() })?;
            let engine = if st.fts5 { "fts5+bm25" } else { "bm25（這台 Python 沒有 FTS5）" };
            let db = to_posix(&if st.dir.ends_with('/') { format!("{}{DB_NAME}", st.dir) } else { format!("{}/{DB_NAME}", st.dir) });
            out.push_str(&format!("kb build：{} 檔（重切 {}、未變 {}、移除 {}、略過 {}），共 {} 段；引擎 {engine}；索引 {db}\n",
                st.files, st.rechunked, st.unchanged + st.touched, st.removed, st.skipped.len(), st.chunks));
            for (rel, why) in st.skipped.iter().take(20) {
                out.push_str(&format!("  略過 {rel}：{why}\n"));
            }
        }
        Some("query") => {
            let int = |name: &str, default: i64| a.one(name).and_then(|v| py_int(&v)).unwrap_or(default);
            let o = QueryOpts { root, top: int("--top", 8), budget: int("--budget", 4000), engine: a.one("--engine").unwrap_or_else(|| "auto".into()), kb_dir: dir, profile };
            let info = search(a.text.as_deref().unwrap_or(""), &o)?;
            if a.has("--json") {
                out.push_str(&results_json(&info.results));
                out.push('\n');
                if !info.note.is_empty() {
                    warn(&info.note);
                } else if info.truncated > 0 {
                    warn(&format!("已截斷，另有 {} 筆", info.truncated));
                }
            } else if !info.note.is_empty() && info.results.is_empty() {
                out.push_str(&format!("kb：{}\n", info.note));
            } else {
                if !info.note.is_empty() {
                    warn(&info.note);
                }
                out.push_str(&format_results(&info));
                out.push('\n');
            }
        }
        Some("lint") => {
            let res = lint(root.as_deref(), dir.as_deref(), profile.as_deref())?;
            if !res.note.is_empty() {
                out.push_str(&format!("kb：{}\n", res.note));
                return Ok(());
            }
            out.push_str(&format!("kb lint：壞連結 {} 筆、孤兒頁 {} 個\n", res.broken.len(), res.orphans.len()));
            if !res.broken.is_empty() {
                out.push_str("壞連結：\n");
                for x in &res.broken {
                    out.push_str(&format!("  {}:{} → {}（{}）\n", x.path, x.line, x.target, if x.kind == "wiki" { "wikilink" } else { "相對連結" }));
                }
            }
            if !res.orphans.is_empty() {
                out.push_str("孤兒頁（sources 裡沒有任何檔連到它）：\n");
                for p in &res.orphans {
                    out.push_str(&format!("  {p}\n"));
                }
            }
        }
        _ => out.push_str(HELP_TOP),
    }
    Ok(())
}

fn run_to(argv: &[String], out: &mut String) -> i32 {
    if let Err(e) = run(argv, out) {
        // 輔助功能失敗不擋流程
        out.push_str(&format!("kb：{}：{}（略過，不擋流程）\n", e.class, pyish::take(&pyish::collapse_ws(&e.msg), 200)));
    }
    0
}

/// 跑一次命令列，回傳（結束碼, stdout, stderr）。測試與對跑用；`--selftest` 不在這裡收（它自己印）。
pub fn run_captured(argv: &[String]) -> (i32, String, String) {
    let prev = swap_err_sink(Some(String::new()));
    let mut out = String::new();
    let code = run_to(argv, &mut out);
    let err = swap_err_sink(prev).unwrap_or_default();
    (code, out, err)
}

/// `awf kb …`
pub fn main(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--selftest") {
        return selftest();
    }
    let mut out = String::new();
    let code = run_to(args, &mut out);
    let _ = std::io::stdout().write_all(out.as_bytes());
    code
}
