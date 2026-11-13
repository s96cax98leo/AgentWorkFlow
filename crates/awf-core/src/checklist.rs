//! 可驗證的 checklist：每一項都帶驗證指令，打勾與否以指令實際結果為準，不以人或 agent 的宣稱為準。
//! 原本是 0.x 的 Python 工具 `checklist.py`。
//!
//! 格式（Markdown，一行一項；沒有驗證指令的項目一律算未完成）：
//!     - [ ] SP1.3 初始化改成跨平台 — 驗證：`awf init --help`

use regex::Regex;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const USAGE: &str = "可驗證的 checklist：每一項都帶驗證指令，打勾與否以指令實際結果為準，不以人或 agent 的宣稱為準。

格式（Markdown，一行一項；沒有驗證指令的項目一律算未完成）：
    - [ ] SP1.3 初始化改成跨平台 — 驗證：`awf init --help`
    - [x] SP3.1 checklist 工具 — 驗證：`awf selftest checklist`

用法：
    awf checklist status <檔>                     # 列出未完成的項目
    awf checklist check <檔> [--id <前綴>] [--write] [--cwd <目錄>] [--timeout <秒>]
        實際執行驗證指令。--write 時依結果改寫勾選框（通過打勾、失敗取消）。
        有任何一項（或 --id 篩出的項目）失敗就 exit 1。
    awf checklist --selftest
";

fn item_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(?P<pre>\s*[-*] \[)(?P<box>[ xX])(?P<mid>\] )(?P<id>\S+)\s+(?P<rest>.*)$").expect("固定的正規式"))
}

fn verify_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"驗證[:：]\s*`(?P<cmd>[^`]+)`").expect("固定的正規式"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub line: usize,
    pub id: String,
    pub done: bool,
    pub cmd: String,
    pub desc: String,
}

fn opt(argv: &[String], name: &str, default: &str) -> String {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

pub fn parse(text: &str) -> Vec<Item> {
    text.lines().enumerate().filter_map(|(no, line)| {
        let m = item_re().captures(line)?;
        let rest = m.name("rest")?.as_str();
        let cmd = verify_re().captures(rest).and_then(|v| v.name("cmd")).map(|c| c.as_str().to_string()).unwrap_or_default();
        Some(Item { line: no, id: m.name("id")?.as_str().to_string(), done: m.name("box")?.as_str() != " ", cmd, desc: rest.to_string() })
    }).collect()
}

/// 用系統的 shell 跑一條指令，回傳（是否成功、說明）。逾時就殺掉並算失敗。
pub fn run(cmd: &str, cwd: &Path, timeout: u64) -> (bool, String) {
    let mut c = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    let mut child = match c.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(ch) => ch,
        Err(e) => return (false, format!("跑不起來：{e}")),
    };
    // 另開執行緒讀輸出，避免輸出塞滿管線讓子程序卡住
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = so.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if start.elapsed() >= Duration::from_secs(timeout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    let Some(status) = status else { return (false, format!("逾時（>{timeout} 秒）")) };
    let all = format!("{}{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    let lines: Vec<&str> = all.trim().lines().collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    let code = status.code().unwrap_or(-1);
    let why = if tail.is_empty() { format!("exit {code}") } else { format!("exit {code}｜{}", tail.join(" / ")) };
    (status.success(), why)
}

fn check(path: &Path, argv: &[String], out: &mut dyn FnMut(String)) -> Result<i32, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let cwd = PathBuf::from(opt(argv, "--cwd", "."));
    let timeout: u64 = opt(argv, "--timeout", "600").parse().map_err(|_| "--timeout 要是整數秒".to_string())?;
    let prefix = opt(argv, "--id", "");
    let write = argv.iter().any(|a| a == "--write");
    let mut failed = 0;
    for it in parse(&text) {
        if !prefix.is_empty() && !it.id.starts_with(&prefix) { continue; }
        let (ok, why) = if it.cmd.is_empty() { (false, "缺驗證指令".to_string()) } else { run(&it.cmd, &cwd, timeout) };
        if !ok { failed += 1; }
        out(format!("{}  {}  {}", if ok { "通過" } else { "失敗" }, it.id, why));
        if write {
            if let Some(m) = lines.get(it.line).and_then(|l| item_re().captures(l)) {
                let new = format!("{}{}{}{} {}", &m["pre"], if ok { "x" } else { " " }, &m["mid"], &m["id"], &m["rest"]);
                lines[it.line] = new;
            }
        }
    }
    if write {
        std::fs::write(path, format!("{}\n", lines.join("\n"))).map_err(|e| format!("寫不進 {}：{e}", path.display()))?;
    }
    out(if failed > 0 { format!("共 {failed} 項失敗") } else { "全部通過".to_string() });
    Ok(if failed > 0 { 1 } else { 0 })
}

fn status(path: &Path, out: &mut dyn FnMut(String)) -> Result<i32, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let items = parse(&text);
    let todo: Vec<&Item> = items.iter().filter(|i| !i.done).collect();
    for it in &todo { out(format!("未完成  {}  {}", it.id, it.desc)); }
    out(format!("{}/{} 完成", items.len() - todo.len(), items.len()));
    Ok(0)
}

pub fn selftest() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("awf-checklist-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let f = dir.join("c.md");
    let fail = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
    std::fs::write(&f, format!("# 測試\n- [ ] A1 會過 — 驗證：`echo ok`\n- [x] A2 宣稱完成但會失敗 — 驗證：`{fail}`\n- [x] A3 沒有驗證指令\n")).map_err(|e| e.to_string())?;
    let d = dir.to_string_lossy().to_string();
    let mut sink = |_: String| {};
    let r = (|| {
        if check(&f, &["--write".into(), "--cwd".into(), d.clone()], &mut sink)? != 1 { return Err("有失敗項目時要回 1".to_string()); }
        let got: Vec<(String, bool)> = parse(&std::fs::read_to_string(&f).map_err(|e| e.to_string())?).into_iter().map(|i| (i.id, i.done)).collect();
        let want = vec![("A1".to_string(), true), ("A2".to_string(), false), ("A3".to_string(), false)];
        if got != want { return Err(format!("打勾結果不對：{got:?}")); }
        if check(&f, &["--id".into(), "A1".into(), "--cwd".into(), d.clone()], &mut sink)? != 0 { return Err("只跑 A1 要回 0".to_string()); }
        let (ok, why) = run(if cfg!(windows) { "ping -n 5 127.0.0.1" } else { "sleep 5" }, &dir, 1);
        if ok || !why.starts_with("逾時") { return Err(format!("逾時要算失敗：{why}")); }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// `awf checklist …`
pub fn main(argv: &[String]) -> i32 {
    if argv.first().map(String::as_str) == Some("--selftest") {
        return match selftest() {
            Ok(()) => { println!("selftest 通過"); 0 }
            Err(e) => { eprintln!("selftest 失敗：{e}"); 1 }
        };
    }
    let cmd = argv.first().map(String::as_str);
    if argv.len() < 2 || !matches!(cmd, Some("status") | Some("check")) {
        print!("{USAGE}");
        return 2;
    }
    let path = PathBuf::from(&argv[1]);
    let mut out = |s: String| println!("{s}");
    let r = if cmd == Some("status") { status(&path, &mut out) } else { check(&path, &argv[2..], &mut out) };
    match r {
        Ok(code) => code,
        Err(e) => { eprintln!("{e}"); 1 }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        super::selftest().expect("selftest");
    }
}
