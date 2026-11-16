//! 先壓縮、再給模型讀：跑一個指令，完整輸出寫進 log 檔，終端只印重點。
//! 原本是 0.x 的 Python 工具 `run_brief.py`；摘要的文字是對跑基準的一部分
//! （`tests/parity/small.expected.json`）。
//!
//! 做法參考 caveman（Apache-2.0）的「Read less」，用自己的寫法。

use crate::kb::pyish::{self, ppath, WS_CLASS};
use crate::pyfmt;
use regex::Regex;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

pub const USAGE: &str = "先壓縮、再給模型讀：跑一個指令，完整輸出寫進 log 檔，終端只印重點（token 精簡的 Read less）。

用法：
    awf run-brief [--log <檔>] [--name <名>] [--tail <行數>] -- <指令…>
        跑 <指令>（不經 shell；需要 shell 語法時寫成 -- bash -c \"…\" 或 -- pwsh -c \"…\"）。
        完整 stdout＋stderr 寫進 log。派工單會指定 --log <run 目錄>/<段>/logs/<label>.log（和執行紀錄放一起留存）；
        --log 的目標只能是 runs_dir 底下某個 run 目錄裡的 .md／.log／.txt，不能是工具自己的狀態檔（run.json、稽核檔、
        決策與連結帳本、資料包、執行紀錄索引…）或 archive/ 底下；不合就拒絕（exit 2）、不執行指令。
        沒給時寫到環境變數 AWF_LOG_DIR，再沒有就寫到系統暫存目錄的 awf-logs/<名>-<序號>.log（不寫進目標 repo，避免誤 commit）。
        終端只印：結束碼、偵測到的框架、失敗的測試（最多 20 則）、最後 N 行（預設 20；認不得格式時 40）、log 路徑。
        結束碼原樣傳回（指令找不到時 127）。
    awf run-brief --selftest
agent 的執行紀錄要附 log 路徑，讓使用者需要時能看完整輸出。
";

pub const MAX_FAILS: usize = 20;
pub const DEFAULT_TAIL: i64 = 20;
pub const UNKNOWN_TAIL: i64 = 40;

/// 濃縮的大小（profile 的 `limits.run_brief`）。預設＝原本寫死的值；沒寫或不合範圍的那一項用預設。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BriefTune {
    /// 失敗的測試最多列幾則。
    pub max_fails: usize,
    /// 認得框架時結尾留幾行。
    pub tail: i64,
    /// 認不得框架時結尾留幾行。
    pub unknown_tail: i64,
}

impl Default for BriefTune {
    fn default() -> Self {
        BriefTune { max_fails: MAX_FAILS, tail: DEFAULT_TAIL, unknown_tail: UNKNOWN_TAIL }
    }
}

impl BriefTune {
    pub fn of(profile: &serde_json::Value) -> BriefTune {
        use crate::tune::{at, int};
        let k = |name: &str| at(profile, &["limits", "run_brief", name]);
        BriefTune {
            max_fails: int(k("max_fails"), 1, 200, MAX_FAILS as i64) as usize,
            tail: int(k("tail"), 5, 500, DEFAULT_TAIL),
            unknown_tail: int(k("unknown_tail"), 5, 500, UNKNOWN_TAIL),
        }
    }
}

struct Framework {
    name: &'static str,
    detect: Regex,
    fail: Regex,
}

/// 把 Python `re` 的 `\s`、`\S` 換成同範圍的字元類別（Rust 的 `\s` 少了 U+001C～U+001F）。
fn py_re(src: &str) -> Regex {
    let s = src.replace(r"\S", &format!("[^{WS_CLASS}]")).replace(r"\s", &format!("[{WS_CLASS}]"));
    Regex::new(&s).expect("固定的正規式")
}

fn frameworks() -> &'static [Framework] {
    static F: OnceLock<Vec<Framework>> = OnceLock::new();
    F.get_or_init(|| {
        [
            ("pytest", r"(?m)=+ .*(passed|failed|error).* in [\d.]+s|^collected \d+ items", r"(?m)^(?:FAILED|ERROR) (\S+)"),
            ("jest/vitest", r"(?m)^\s*Tests?:\s+.*(passed|failed)|^\s*Test Files\s+.*(passed|failed)", r"(?m)^\s*(?:●|FAIL|×|✕)\s+(.+?)\s*$"),
            ("go test", r"(?m)^(?:ok|FAIL|---)\s|^PASS$", r"(?m)^--- FAIL: (\S+)"),
            ("unittest", r"(?m)^Ran \d+ tests? in", r"(?m)^(?:FAIL|ERROR): (\S+ \(.+?\))"),
        ]
        .into_iter()
        .map(|(name, d, f)| Framework { name, detect: py_re(d), fail: py_re(f) })
        .collect()
    })
}

/// 回（框架名，失敗的測試）；認不得時框架名是空字串。
pub fn detect(text: &str) -> (String, Vec<String>) {
    for fw in frameworks() {
        if fw.detect.is_match(text) {
            let mut fails: Vec<String> = Vec::new();
            for c in fw.fail.captures_iter(text) {
                let f = pyish::strip(c.get(1).map(|m| m.as_str()).unwrap_or_default());
                if !f.is_empty() && !fails.iter().any(|x| x == f) {
                    fails.push(f.to_string());
                }
            }
            return (fw.name.to_string(), fails);
        }
    }
    (String::new(), Vec::new())
}

/// Python 的 `str.splitlines()`。
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let is_break = matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !is_break {
            continue;
        }
        out.push(&text[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(j, '\n')) = it.peek() {
                it.next();
                next = j + 1;
            }
        }
        start = next;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn norm(p: &str) -> String {
    if cfg!(windows) { p.to_string() } else { ppath(p) }
}

/// `Path(p).name`
fn path_name(p: &str) -> String {
    let seps: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    p.split(seps).filter(|s| !s.is_empty() && *s != ".").last().unwrap_or_default().to_string()
}

/// 沒給 `--log` 時的 log 路徑：`AWF_LOG_DIR` 或系統暫存目錄的 `awf-logs/<名>-<序號>.log`。
pub fn default_log(name: &str) -> std::io::Result<String> {
    let d = match std::env::var("AWF_LOG_DIR") {
        Ok(v) if !v.is_empty() => norm(&v),
        _ => norm(&std::env::temp_dir().join("awf-logs").to_string_lossy()),
    };
    std::fs::create_dir_all(&d)?;
    static SAFE: OnceLock<Regex> = OnceLock::new();
    let re = SAFE.get_or_init(|| Regex::new(r"[^A-Za-z0-9._-]+").expect("固定的正規式"));
    let replaced = re.replace_all(name, "-");
    let safe = match replaced.trim_matches('-') {
        "" => "cmd",
        s => s,
    };
    let mut i = 1u64;
    loop {
        let p = Path::new(&d).join(format!("{safe}-{i}.log"));
        if !p.exists() {
            return Ok(p.to_string_lossy().into_owned());
        }
        i += 1;
    }
}

/// 跑指令，stdout 與 stderr 合在同一條管線（順序照實際輸出）。回（結束碼，輸出）。
fn run_merged(cmd: &[String]) -> std::io::Result<(i32, String)> {
    let Some(prog) = cmd.first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "沒有指令"));
    };
    let (mut reader, writer) = std::io::pipe()?;
    let mut c = Command::new(prog);
    c.args(cmd.get(1..).unwrap_or_default()).stdin(Stdio::inherit()).stdout(writer.try_clone()?).stderr(writer);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((127, format!("找不到指令：[Errno 2] No such file or directory: {}\n", pyfmt::repr_str(prog))));
        }
        Err(e) => return Err(e),
    };
    drop(c); // 放掉自己手上的寫入端，子行程結束後才讀得到 EOF
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    let status = child.wait()?;
    let code = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            -status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            1
        }
    });
    // Python 的 text=True：換行一律變成 \n
    let text = String::from_utf8_lossy(&buf).replace("\r\n", "\n").replace('\r', "\n");
    Ok((code, text))
}

/// 跑 cmd、寫 log、回（結束碼，摘要文字）。`tail` 是 0 或沒給時用預設行數。
pub fn brief(cmd: &[String], log: &str, tail: Option<i64>) -> std::io::Result<(i32, String)> {
    brief_with(cmd, log, tail, &BriefTune::default())
}

/// 同 [`brief`]，大小用 `tune`（profile 的 `limits.run_brief`）。
pub fn brief_with(cmd: &[String], log: &str, tail: Option<i64>, tune: &BriefTune) -> std::io::Result<(i32, String)> {
    let log = norm(log);
    let (code, text) = run_merged(cmd)?;
    if let Some(parent) = Path::new(&log).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&log, &text)?;
    let (fw, fails) = detect(&text);
    let n = match tail {
        Some(t) if t != 0 => t,
        _ if !fw.is_empty() => tune.tail,
        _ => tune.unknown_tail,
    };
    let lines = splitlines(text.trim_end_matches('\n'));
    let total = lines.len() as i64;
    let fw_text = if fw.is_empty() { format!("認不得（印最後 {n} 行）") } else { fw };
    let mut out = vec![format!("結束碼：{code}"), format!("框架：{fw_text}")];
    if !fails.is_empty() {
        let max_fails = tune.max_fails;
        let more = if fails.len() > max_fails { format!("，只列前 {max_fails} 則") } else { String::new() };
        out.push(format!("失敗的測試（{} 則{more}）：", fails.len()));
        out.extend(fails.iter().take(max_fails).map(|f| format!("  - {f}")));
    }
    out.push(format!("最後 {} 行（共 {total} 行）：", n.min(total)));
    // lines[-n:]：n 是負數時等於 lines[|n|:]
    let skip = if n > 0 { (total - n).max(0) } else { (-n).min(total) } as usize;
    out.extend(lines.iter().skip(skip).map(|ln| format!("  {ln}")));
    out.push(format!("完整 log：{log}"));
    Ok((code, out.join("\n")))
}

fn opt(argv: &[String], name: &str) -> String {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned().unwrap_or_default()
}

/// Python 的 `int(s)`（十進位）。
fn py_int(s: &str) -> Option<i64> {
    let t = pyish::strip(s);
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    t.replace('_', "").parse::<i64>().ok()
}

/// selftest 用的假指令（取代 0.x 的 Python 版臨時寫的小腳本）：`awf run-brief --selftest-emit <種類>`。
fn emit(kind: &str) -> i32 {
    match kind {
        "lines" => {
            for i in 0..100 {
                println!("line {i}");
            }
            3
        }
        "many" => {
            println!("collected 30 items");
            for i in 0..30 {
                println!("FAILED tests/t.py::t{i}");
            }
            println!("===== 30 failed in 1.00s =====");
            1
        }
        _ => {
            println!("hi");
            0
        }
    }
}

/// `--log` 不能指到的檔名（不分大小寫）：工具自己的狀態檔與主 session 才能寫的共用檔。
/// 副檔名規則已經擋掉 .json／.jsonl，這裡仍然列出來：規則是「這些檔不能被 log 蓋掉」，不靠副檔名剛好不同。
fn reserved_log_names() -> Vec<String> {
    let mut v: Vec<String> = ["run.json", "merge-audit.jsonl", "publish-audit.jsonl", "links.jsonl", "decisions.jsonl", "reuse.json", "pack-args.json", "progress.jsonl"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(crate::hooks::guard::SHARED_CONTEXT_NAMES.iter().map(|s| s.to_lowercase()));
    v.push(crate::save_run::INDEX_NAME.to_lowercase());
    v.push(crate::state::HANDOFF.to_lowercase());
    v
}

/// `--log <log>` 能不能寫：回 None 是可以，否則回拒絕的理由。
/// 規則（照解開符號連結與 `..` 之後的實際位置判）：在 `<root>/<runs_dir>/<run>/` 底下、副檔名是 .md／.log／.txt、
/// 檔名不是工具的狀態檔、路徑不經過 `archive/`。`root` 是專案根、`cwd` 是相對路徑的基準。
pub fn log_target_error(log: &str, root: &str, runs_dir: &str, cwd: &str) -> Option<String> {
    use crate::kb::pyish;
    let abs = |p: &str, base: &str| if pyish::is_abs(p) { p.to_string() } else { pyish::pjoin(base, p) };
    let real = |p: &str| pyish::resolve(p).ok().map(|r| r.replace('\\', "/"));
    let runs_dir = if runs_dir.trim().is_empty() { "_tpm/runs" } else { runs_dir };
    let Some(runs) = real(&abs(runs_dir, root)) else { return Some(format!("解析不了 runs_dir（{runs_dir}）")) };
    let Some(target) = real(&abs(log, cwd)) else { return Some(format!("解析不了路徑 {log}")) };
    let base = format!("{}/", runs.trim_end_matches('/'));
    let Some(rel) = target.strip_prefix(&base) else {
        return Some(format!("的目標要在 {runs_dir}/<run>/ 底下（{log} 實際是 {target}）"));
    };
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Some(format!("的目標要在某個 run 目錄裡（{runs_dir}/<run>/…），不能直接放在 {runs_dir}/"));
    }
    let name = parts[parts.len() - 1].to_lowercase();
    if ![".md", ".log", ".txt"].iter().any(|e| name.len() > e.len() && name.ends_with(e)) {
        return Some(format!("的副檔名只能是 .md、.log、.txt（{log}）"));
    }
    if reserved_log_names().contains(&name) {
        return Some(format!("不能指到工具自己的檔（{}）", parts[parts.len() - 1]));
    }
    if parts[1..parts.len() - 1].iter().any(|d| d.to_lowercase() == "archive") {
        return Some(format!("不能寫到 run 目錄的 archive/ 底下（{log}）"));
    }
    None
}

/// 用行程的環境檢查 `--log`：專案根照 [`crate::project`] 的解析，runs_dir 取自守門設定。
/// 專案根找不到、守門設定讀不懂都拒絕（確認不了 runs_dir 在哪）；不帶 `--log` 不受影響。
fn check_log_opt(log: &str) -> Result<(), String> {
    let Some(found) = crate::project::find(None) else {
        return Err(format!("要寫在專案的 run 目錄裡，但{}（不帶 --log 會寫到暫存目錄）", crate::project::not_found_message()));
    };
    let cfg = crate::config::load(&found.root).map_err(|e| format!("要寫在 runs_dir 底下，但守門設定讀不懂（{}：{}）；不帶 --log 會寫到暫存目錄", e.class, e.msg))?;
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| ".".to_string());
    match log_target_error(log, &found.root, &cfg.runs_dir, &cwd) {
        Some(m) => Err(m),
        None => Ok(()),
    }
}

/// `awf run-brief …`
pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("--selftest") => return selftest(),
        Some("--selftest-emit") => return emit(argv.get(1).map(String::as_str).unwrap_or_default()),
        _ => {}
    }
    let Some(i) = argv.iter().position(|a| a == "--").filter(|i| i + 1 < argv.len()) else {
        println!("{USAGE}");
        return 2;
    };
    let (opts, cmd) = (&argv[..i], &argv[i + 1..]);
    let log_opt = opt(opts, "--log");
    let log = if !log_opt.is_empty() {
        if let Err(m) = check_log_opt(&log_opt) {
            eprintln!("awf run-brief：--log {m}。沒有執行指令");
            return 2;
        }
        log_opt
    } else {
        let name = opt(opts, "--name");
        let name = if name.is_empty() { path_name(cmd.first().map(String::as_str).unwrap_or_default()) } else { name };
        match default_log(&name) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("awf run-brief：建不了 log 目錄：{e}");
                return 1;
            }
        }
    };
    let tail_opt = opt(opts, "--tail");
    let tail = if tail_opt.is_empty() {
        None
    } else {
        match py_int(&tail_opt) {
            Some(n) => Some(n),
            None => {
                eprintln!("ValueError: invalid literal for int() with base 10: {}", pyfmt::repr_str(&tail_opt));
                return 1;
            }
        }
    };
    // 大小讀專案的 profile（limits.run_brief）；找不到專案或沒設定就是預設值
    let tune = BriefTune::of(&crate::tune::project_profile());
    match brief_with(cmd, &log, tail, &tune) {
        Ok((code, text)) => {
            println!("{text}");
            // 和 Python 的 sys.exit 一樣：被訊號結束的負數結束碼取低 8 位
            if code < 0 { code & 0xFF } else { code }
        }
        Err(e) => {
            eprintln!("awf run-brief：{e}");
            1
        }
    }
}

const SAMPLES: &[(&str, &str)] = &[
    ("pytest", "collected 3 items\n\ntests/test_a.py .F.\n\nFAILED tests/test_a.py::test_two - assert 1 == 2\n===== 1 failed, 2 passed in 0.12s =====\n"),
    ("jest/vitest", " FAIL  src/a.test.ts\n  ● sum › adds\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 4 passed, 5 total\n"),
    ("go test", "=== RUN   TestAdd\n--- FAIL: TestAdd (0.00s)\n    a_test.go:9: got 1\nFAIL\nFAIL\texample.com/a\t0.01s\n"),
];

fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    s.chars().skip(total.saturating_sub(n)).collect()
}

pub fn selftest() -> i32 {
    let mut ok = true;
    let mut check = |name: &str, cond: bool, detail: &str| {
        let extra = if cond { String::new() } else { format!("：{}", tail_chars(detail, 400)) };
        println!("  [{}] {name}{extra}", if cond { "通過" } else { "失敗" });
        ok = ok && cond;
    };
    println!("run_brief selftest");
    for (fw, sample) in SAMPLES {
        let (got, fails) = detect(sample);
        check(&format!("偵測 {fw}"), got == *fw && !fails.is_empty(), &format!("{got} {fails:?}"));
    }
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "awf".into());
    let me = |kind: &str| vec![exe.clone(), "run-brief".to_string(), "--selftest-emit".to_string(), kind.to_string()];
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("awf-run-brief-{}-{nanos}", std::process::id()));
    let t = tmp.to_string_lossy().into_owned();
    let p = |rel: &str| Path::new(&t).join(rel).to_string_lossy().into_owned();
    let fail = |e: std::io::Error| (-999, format!("執行失敗：{e}"));

    let log = p("logs/x.log");
    let (code, text) = brief(&me("lines"), &log, None).unwrap_or_else(fail);
    check("結束碼原樣傳回", code == 3, &text);
    let logged = std::fs::read_to_string(&log).unwrap_or_default();
    check("完整輸出寫進 log", logged.matches("line ").count() == 100, "");
    check("認不得的格式只印最後 40 行", text.contains("line 99") && !text.contains("line 59") && text.contains("line 60"), &text);
    check("摘要附 log 路徑", text.contains(&norm(&log)), "");
    let (code, text) = brief(&me("many"), &p("many.log"), None).unwrap_or_else(fail);
    let head = text.split("最後").next().unwrap_or_default();
    check("失敗測試最多列 20 則", code == 1 && head.contains("t19") && !head.contains("t25"), &text);
    let (code, text) = brief(&["awf-no-such-command-xyz".to_string()], &p("nf.log"), None).unwrap_or_else(fail);
    check("找不到指令時結束碼 127", code == 127, &text);
    let old = std::env::var_os("AWF_LOG_DIR");
    std::env::set_var("AWF_LOG_DIR", p("envlogs"));
    let mut argv = vec!["--name".to_string(), "demo".to_string(), "--".to_string()];
    argv.extend(me("hi"));
    let rc = main(&argv);
    check("CLI：沒給 --log 時寫到 AWF_LOG_DIR/<名>-<序號>.log（不寫進目標 repo）", rc == 0 && Path::new(&p("envlogs/demo-1.log")).is_file(), "");
    match old {
        Some(v) => std::env::set_var("AWF_LOG_DIR", v),
        None => std::env::remove_var("AWF_LOG_DIR"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    println!("run_brief selftest：{}", if ok { "全部通過" } else { "有失敗" });
    if ok { 0 } else { 1 }
}
