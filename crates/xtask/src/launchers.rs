//! `cargo xtask check-launchers`：核對兩個啟動器（`plugin/bin/awf`、`plugin/bin/awf.cmd`），不需要真的執行檔。
//!
//! 做法：在暫存目錄放啟動器的副本與假的執行檔（印出自己的檔名與收到的參數），把假的 `uname` 放在 PATH 最前面
//! 模擬各個平台，核對挑到的檔名、參數、exit code、符號連結、`AWF_BIN`、錯誤訊息。
//! - unix：`awf` 實際執行（系統的 sh，有 dash 再用 dash 跑一遍）；`awf.cmd` 只做靜態檢查。
//! - Windows：`awf.cmd` 實際執行（拿 xtask 自己當假的執行檔）；`awf` 只做靜態檢查與 `sh -n`。

use crate::dist;
use crate::util::{self, Captured, TempDir};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `plugin/bin/awf` 的行數上限：啟動器只挑平台，長了就是放了邏輯。
pub const SH_MAX_LINES: usize = 40;

/// xtask 的隱藏指令：印出自己的檔名與參數（Windows 上當假的執行檔用）。
pub const ECHO_COMMAND: &str = "__launcher-echo";

/// （`uname -s`、`uname -m`、應該挑到的檔名）。
const UNAME_CASES: &[(&str, &str, &str)] = &[
    ("Darwin", "arm64", "awf-darwin-arm64"),
    ("Darwin", "x86_64", "awf-darwin-x64"),
    ("Linux", "x86_64", "awf-linux-x64"),
    ("Linux", "amd64", "awf-linux-x64"),
    ("Linux", "aarch64", "awf-linux-arm64"),
    ("Linux", "arm64", "awf-linux-arm64"),
    ("MINGW64_NT-10.0-22631", "x86_64", "awf-windows-x64.exe"),
    ("MSYS_NT-10.0-22631", "x86_64", "awf-windows-x64.exe"),
    ("CYGWIN_NT-10.0-22631", "x86_64", "awf-windows-x64.exe"),
    ("MINGW64_NT-10.0-22631", "aarch64", "awf-windows-x64.exe"),
];

/// 要原樣傳到的參數：空白、引號、空字串、萬用字元、變數樣子的字、換行、開頭是減號。
const TRICKY_ARGS: &[&str] = &["a b", "  兩個 空白  ", "say \"hi\"", "it's", "", "*", "$HOME", "`id`", "-n", "a\nb", "\\"];

const FAKE_BIN: &str = "#!/bin/sh\nprintf 'name=%s\\n' \"${0##*/}\"\nprintf 'argc=%s\\n' \"$#\"\nfor a in \"$@\"; do printf 'arg=[%s]\\n' \"$a\"; done\nexit \"${FAKE_EXIT:-0}\"\n";
const FAKE_UNAME: &str = "#!/bin/sh\ncase $1 in\n  -m) printf '%s\\n' \"$FAKE_UNAME_M\" ;;\n  *) printf '%s\\n' \"$FAKE_UNAME_S\" ;;\nesac\n";

/// 假的執行檔應該印出的內容。
pub fn echo_text(name: &str, args: &[String]) -> String {
    let mut s = format!("name={name}\nargc={}\n", args.len());
    for a in args {
        s.push_str(&format!("arg=[{a}]\n"));
    }
    s
}

/// 隱藏指令 `__launcher-echo` 的本體：印出自己的檔名與參數，exit code 取自 `FAKE_EXIT`。
pub fn echo_main(args: &[String]) -> i32 {
    let name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    print!("{}", echo_text(&name, args));
    std::env::var("FAKE_EXIT").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
}

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl Report {
    fn check(&mut self, name: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.passed += 1;
                self.lines.push(format!("  ok    {name}"));
            }
            Err(e) => {
                self.failed += 1;
                self.lines.push(format!("  失敗  {name}：{e}"));
            }
        }
    }

    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.lines.push(format!("  略過  {name}：{why}"));
    }
}

fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg())
    }
}

// ------------------------------------------------------------------ 靜態檢查

fn sh_static(rep: &mut Report, root: &Path, path: &Path, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    rep.check("awf：第一行是 #!/bin/sh", ensure(text.starts_with("#!/bin/sh\n"), || "不是".to_string()));
    rep.check("awf：行尾都是 LF、結尾有換行", ensure(!bytes.contains(&b'\r') && text.ends_with('\n'), || "有 CR 或結尾沒換行".to_string()));
    let n = text.lines().count();
    rep.check(&format!("awf：不超過 {SH_MAX_LINES} 行"), ensure(n <= SH_MAX_LINES, || format!("現在 {n} 行")));
    rep.check(
        "awf：認得 AWF_BIN",
        ensure(text.contains("AWF_BIN"), || "沒有提到 AWF_BIN".to_string()),
    );
    // 檔名由 awf-$os-$arch 組成，實際挑到哪個由執行檢查核對；這裡只擋寫死了表以外的檔名。
    rep.check("awf：沒有寫死表以外的檔名", literal_names(&text).and_then(|names| names_in_table(&names)));
    rep.check("awf：檔案有執行權限", exec_bit(root, path));
    let mut syntax = 0;
    for shell in ["sh", "dash"] {
        if let Some(bin) = util::which(shell) {
            syntax += 1;
            rep.check(&format!("awf：{shell} -n 語法檢查"), quiet(Command::new(bin).arg("-n").arg(path)));
        }
    }
    if syntax == 0 {
        rep.skip("awf：sh -n 語法檢查", "PATH 裡沒有 sh");
    }
    match util::which("shellcheck") {
        Some(bin) => rep.check("awf：shellcheck", quiet(Command::new(bin).arg("--shell=sh").arg(path))),
        None => rep.skip("awf：shellcheck", "沒有安裝 shellcheck"),
    }
}

/// 子程序要 exit 0；不是的話把輸出當成理由。
fn quiet(cmd: &mut Command) -> Result<(), String> {
    let ran = util::run(cmd, None)?;
    ensure(ran.code == 0, || format!("exit {}：{}{}", ran.code, ran.stdout.trim(), ran.stderr.trim()))
}

/// 文字裡寫死的 `awf-<平台>-<架構>` 檔名（含變數的不算）。
fn literal_names(text: &str) -> Result<Vec<String>, String> {
    let re = regex::Regex::new(r"awf-[a-z0-9]+-[a-z0-9]+(?:\.[a-z]+)?").map_err(|e| e.to_string())?;
    let mut names: Vec<String> = re.find_iter(text).map(|m| m.as_str().to_string()).collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn names_in_table(names: &[String]) -> Result<(), String> {
    let known = dist::file_names();
    let bad: Vec<&str> = names.iter().map(String::as_str).filter(|n| !known.contains(n)).collect();
    ensure(bad.is_empty(), || format!("不在檔名表裡：{}", bad.join("、")))
}

/// 執行權限：unix 看檔案本身；在 git repo 裡另外看 index 記的模式（Windows 的檔案系統沒有這個位元，只能看 index）。
fn exec_bit(root: &Path, path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?.permissions().mode();
        ensure(mode & 0o111 != 0, || format!("檔案模式是 {:o}", mode & 0o777))?;
    }
    if !root.join(".git").exists() {
        return Ok(());
    }
    let Some(git) = util::which("git") else { return Ok(()) };
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let ran = util::run(Command::new(git).arg("-C").arg(root).args(["ls-files", "-s", "--", &rel]), None)?;
    let line = ran.stdout.trim();
    // 還沒 git add 的時候 index 裡沒有這個檔，沒得比。
    ensure(line.is_empty() || line.starts_with("100755 "), || format!("git index 記的模式不是 100755（{line}）；跑 git update-index --chmod=+x {rel}"))
}

/// `awf.cmd` 每一行能用的指令：都是 cmd.exe 內建的。
const CMD_BUILTINS: &[&str] = &["echo", "rem", "set", "setlocal", "endlocal", "goto", "exit"];

/// 一行 `.cmd` 用了哪個非內建指令；都是內建的回 None。
fn cmd_foreign(line: &str, if_re: &regex::Regex) -> Option<String> {
    let mut s = line.trim().trim_start_matches('@').trim().to_string();
    for _ in 0..8 {
        if s.is_empty() || s.starts_with(':') {
            return None;
        }
        let word = s.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
        if word == "if" {
            match if_re.captures(&s).and_then(|c| c.get(1)) {
                Some(rest) => s = rest.as_str().trim().to_string(),
                None => return Some(format!("看不懂的 if：{s}")),
            }
            continue;
        }
        // 唯一能啟動的程式是挑到的執行檔。
        if CMD_BUILTINS.contains(&word.as_str()) || s.starts_with("\"%bin%\"") {
            return None;
        }
        return Some(word);
    }
    Some(format!("if 巢狀太深：{line}"))
}

fn cmd_static(rep: &mut Report, bytes: &[u8]) {
    let crlf_ok = {
        let mut ok = bytes.ends_with(b"\r\n");
        let mut prev = 0u8;
        for (i, &b) in bytes.iter().enumerate() {
            let next = bytes.get(i + 1).copied().unwrap_or(0);
            if (b == b'\n' && prev != b'\r') || (b == b'\r' && next != b'\n') {
                ok = false;
            }
            prev = b;
        }
        ok
    };
    rep.check("awf.cmd：行尾都是 CRLF", ensure(crlf_ok, || "有單獨的 LF 或 CR，或結尾沒有換行".to_string()));
    // cmd.exe 用主控台的字碼頁讀批次檔，非 ASCII 的位元組在某些字碼頁會吃掉後面的字元。
    rep.check("awf.cmd：只有 ASCII", ensure(bytes.is_ascii(), || "有非 ASCII 的位元組".to_string()));
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let windows: Vec<&str> = dist::file_names().into_iter().filter(|n| n.ends_with(".exe")).collect();
    rep.check(
        "awf.cmd：提到的檔名對得上檔名表",
        literal_names(&text).and_then(|names| {
            names_in_table(&names)?;
            let lack: Vec<&str> = windows.iter().copied().filter(|w| !names.iter().any(|n| n == w)).collect();
            ensure(lack.is_empty(), || format!("沒有提到 {}", lack.join("、")))?;
            let other: Vec<&str> = names.iter().map(String::as_str).filter(|n| !n.ends_with(".exe")).collect();
            ensure(other.is_empty(), || format!("提到了不是 Windows 的檔：{}", other.join("、")))
        }),
    );
    let lower = text.to_ascii_lowercase();
    let needs = ["%~dp0", "%*", "awf_bin", "%processor_architecture%", "processor_architew6432", "exit /b 127", "exit /b %errorlevel%"];
    let lack: Vec<&str> = needs.iter().copied().filter(|n| !lower.contains(n)).collect();
    rep.check("awf.cmd：定位、轉參數、轉 exit code、AWF_BIN、架構判斷都在", ensure(lack.is_empty(), || format!("缺 {}", lack.join("、"))));
    let foreign = regex::Regex::new(r#"(?i)^if\s+(?:not\s+)?(?:/i\s+)?(?:defined\s+\S+|exist\s+(?:"[^"]*"|\S+)|(?:"[^"]*"|[^\s=]+)==(?:"[^"]*"|\S+))\s+(.*)$"#)
        .map_err(|e| e.to_string())
        .and_then(|if_re| {
            let mut bad = Vec::new();
            for (i, line) in text.lines().enumerate() {
                let body = line.trim().trim_start_matches('@').trim().to_ascii_lowercase();
                let is_text = body.starts_with("rem") || body.starts_with("echo");
                if let Some(word) = cmd_foreign(line, &if_re) {
                    bad.push(format!("第 {} 行用了 {word}", i + 1));
                } else if !is_text && (line.contains('|') || line.replace(">&", "").contains('&') || line.contains('(')) {
                    bad.push(format!("第 {} 行有串接或區塊（|、&、括號）", i + 1));
                }
            }
            ensure(bad.is_empty(), || bad.join("；"))
        });
    rep.check("awf.cmd：只用 cmd 內建指令、沒有串接與區塊", foreign);
}

fn attrs_static(rep: &mut Report, root: &Path) {
    let text = util::read_text(&root.join(".gitattributes")).unwrap_or_default();
    let has = |path: &str, attr: &str| {
        text.lines().any(|l| {
            let mut it = l.split_whitespace();
            it.next() == Some(path) && it.any(|a| a == attr)
        })
    };
    let mut lack = Vec::new();
    if !has("plugin/bin/awf", "eol=lf") {
        lack.push("plugin/bin/awf text eol=lf".to_string());
    }
    if !has("plugin/bin/awf.cmd", "eol=crlf") {
        lack.push("plugin/bin/awf.cmd text eol=crlf".to_string());
    }
    for name in dist::file_names() {
        if !has(&format!("plugin/bin/{name}"), "binary") {
            lack.push(format!("plugin/bin/{name} binary"));
        }
    }
    rep.check(".gitattributes：啟動器的行尾與執行檔的 binary 規則", ensure(lack.is_empty(), || format!("缺 {}", lack.join("、"))));
}

// ------------------------------------------------------------------ 實際執行：awf（unix）

/// 一次呼叫的環境。
#[derive(Default, Clone)]
struct Call {
    uname_s: String,
    uname_m: String,
    awf_bin: Option<PathBuf>,
    exit: Option<i32>,
    args: Vec<String>,
}

impl Call {
    fn on(s: &str, m: &str) -> Self {
        Self { uname_s: s.to_string(), uname_m: m.to_string(), ..Self::default() }
    }
}

/// 暫存目錄裡的佈置：`bin 目錄/`（啟動器與五個假執行檔，目錄名有空白）、`fake/uname`、`empty/`（只有啟動器）。
struct Stage {
    tmp: TempDir,
    bin: PathBuf,
    empty: PathBuf,
    path_env: std::ffi::OsString,
}

#[cfg(unix)]
fn write_exec(path: &Path, body: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).map_err(|e| format!("寫不了 {}：{e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("改不了權限 {}：{e}", path.display()))
}

#[cfg(not(unix))]
fn write_exec(path: &Path, body: &str) -> Result<(), String> {
    std::fs::write(path, body).map_err(|e| format!("寫不了 {}：{e}", path.display()))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link).map_err(|e| format!("建不了符號連結 {}：{e}", link.display()))
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> Result<(), String> {
    Err(format!("這個平台不建符號連結（{}）", link.display()))
}

fn mkdir(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| format!("建不了目錄 {}：{e}", p.display()))
}

fn stage_sh(sh_src: &Path) -> Result<Stage, String> {
    let tmp = TempDir::new("awf-launchers-")?;
    let bin = tmp.path().join("bin 目錄");
    let empty = tmp.path().join("empty");
    let fake = tmp.path().join("fake");
    for d in [&bin, &empty, &fake] {
        mkdir(d)?;
    }
    let body = util::read_text(sh_src)?;
    write_exec(&bin.join("awf"), &body)?;
    write_exec(&empty.join("awf"), &body)?;
    for name in dist::file_names() {
        write_exec(&bin.join(name), FAKE_BIN)?;
    }
    write_exec(&fake.join("uname"), FAKE_UNAME)?;
    let mut dirs = vec![fake];
    dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    let path_env = std::env::join_paths(dirs).map_err(|e| format!("組不出 PATH：{e}"))?;
    Ok(Stage { tmp, bin, empty, path_env })
}

/// 跑啟動器。`interp` 有給就用那個 shell 解讀（`dash <啟動器>`），沒給就直接執行（走 shebang）。
fn launch(stage: &Stage, interp: Option<&Path>, launcher: &Path, call: &Call) -> Result<util::Ran, String> {
    let mut cmd = match interp {
        Some(sh) => {
            let mut c = Command::new(sh);
            c.arg(launcher);
            c
        }
        None => Command::new(launcher),
    };
    cmd.args(&call.args)
        .env("PATH", &stage.path_env)
        .env("FAKE_UNAME_S", &call.uname_s)
        .env("FAKE_UNAME_M", &call.uname_m)
        .env_remove("AWF_BIN")
        .env_remove("FAKE_EXIT");
    if let Some(b) = &call.awf_bin {
        cmd.env("AWF_BIN", b);
    }
    if let Some(code) = call.exit {
        cmd.env("FAKE_EXIT", code.to_string());
    }
    // 不經 util::run：它會把輸出的換行正規化，這裡要逐位元組比對參數（含換行）。
    let out = cmd.output().map_err(|e| format!("跑不起來 {}：{e}", launcher.display()))?;
    Ok(util::Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// 應該成功：exit code、挑到的檔名、參數都對，stderr 沒東西。
fn expect_run(stage: &Stage, interp: Option<&Path>, launcher: &Path, call: &Call, name: &str) -> Result<(), String> {
    let ran = launch(stage, interp, launcher, call)?;
    let want = echo_text(name, &call.args);
    let code = call.exit.unwrap_or(0);
    ensure(ran.code == code, || format!("exit code 應該是 {code}，實際 {}（stderr：{}）", ran.code, ran.stderr.trim()))?;
    ensure(ran.stdout == want, || format!("輸出應該是 {want:?}，實際 {:?}", ran.stdout))?;
    ensure(ran.stderr.is_empty(), || format!("stderr 應該是空的，實際 {:?}", ran.stderr))
}

/// 應該被拒絕：exit 127、stdout 沒東西、stderr 是一行而且含指定的字。
fn expect_127(stage: &Stage, interp: Option<&Path>, launcher: &Path, call: &Call, needles: &[&str]) -> Result<(), String> {
    let ran = launch(stage, interp, launcher, call)?;
    ensure(ran.code == 127, || format!("exit code 應該是 127，實際 {}", ran.code))?;
    ensure(ran.stdout.is_empty(), || format!("stdout 應該是空的，實際 {:?}", ran.stdout))?;
    ensure(ran.stderr.trim_end().lines().count() == 1 && ran.stderr.starts_with("awf："), || format!("stderr 應該是一行 awf：開頭的訊息，實際 {:?}", ran.stderr))?;
    let lack: Vec<&str> = needles.iter().copied().filter(|n| !ran.stderr.contains(n)).collect();
    ensure(lack.is_empty(), || format!("訊息裡沒有 {}：{:?}", lack.join("、"), ran.stderr))
}

fn sh_runtime(rep: &mut Report, stage: &Stage, tag: &str, interp: Option<&Path>) {
    let awf = stage.bin.join("awf");
    for (s, m, name) in UNAME_CASES {
        rep.check(&format!("awf（{tag}）：{s} {m} → {name}"), expect_run(stage, interp, &awf, &Call::on(s, m), name));
    }

    let mut call = Call::on("Linux", "x86_64");
    call.args = TRICKY_ARGS.iter().map(|a| a.to_string()).collect();
    rep.check(&format!("awf（{tag}）：參數原樣傳到（空白、引號、空字串、*、換行）"), expect_run(stage, interp, &awf, &call, "awf-linux-x64"));

    for code in [1, 3, 42, 255] {
        let mut call = Call::on("Darwin", "arm64");
        call.exit = Some(code);
        rep.check(&format!("awf（{tag}）：exit code {code} 原樣傳回"), expect_run(stage, interp, &awf, &call, "awf-darwin-arm64"));
    }

    // 符號連結：絕對路徑、相對路徑、兩層；連結所在的目錄裡沒有執行檔，要找回啟動器真正的位置。
    let links = stage.tmp.path().join(format!("links {tag}"));
    let deeper = links.join("deeper");
    let made = mkdir(&deeper)
        .and_then(|_| symlink(&awf, &links.join("abs")))
        .and_then(|_| symlink(Path::new("../bin 目錄/awf"), &links.join("rel")))
        .and_then(|_| symlink(Path::new("../rel"), &deeper.join("chain")));
    let mut call = Call::on("Linux", "aarch64");
    call.args = vec!["x y".to_string()];
    for (what, link) in [("絕對路徑", links.join("abs")), ("相對路徑", links.join("rel")), ("兩層", deeper.join("chain"))] {
        let result = made.clone().and_then(|_| expect_run(stage, interp, &link, &call, "awf-linux-arm64"));
        rep.check(&format!("awf（{tag}）：經符號連結呼叫（{what}）"), result);
    }

    // 以名字經 PATH 呼叫（plugin 的 bin/ 進 PATH 之後就是這樣用），目錄名有空白。
    let by_name = (|| {
        let sh = interp.map(Path::to_path_buf).or_else(|| util::which("sh")).ok_or("PATH 裡沒有 sh")?;
        let mut dirs = vec![stage.bin.clone()];
        dirs.extend(std::env::split_paths(&stage.path_env));
        let path = std::env::join_paths(dirs).map_err(|e| format!("組不出 PATH：{e}"))?;
        let out = Command::new(sh)
            .args(["-c", "awf \"$@\"", "sh", "p q", "*"])
            .env("PATH", path)
            .env("FAKE_UNAME_S", "Darwin")
            .env("FAKE_UNAME_M", "x86_64")
            .env_remove("AWF_BIN")
            .env_remove("FAKE_EXIT")
            .output()
            .map_err(|e| format!("跑不起來：{e}"))?;
        let want = echo_text("awf-darwin-x64", &["p q".to_string(), "*".to_string()]);
        let got = String::from_utf8_lossy(&out.stdout);
        ensure(out.status.code() == Some(0) && got == want, || format!("應該是 {want:?}，實際 {got:?}（{}）", String::from_utf8_lossy(&out.stderr).trim()))
    })();
    rep.check(&format!("awf（{tag}）：以名字經 PATH 呼叫"), by_name);

    // AWF_BIN：不看平台，直接用它（平台不支援、同目錄沒有執行檔也一樣）。
    let custom = stage.tmp.path().join("自己 建的").join("my-awf");
    let made = custom.parent().map(mkdir).unwrap_or(Ok(())).and_then(|_| write_exec(&custom, FAKE_BIN));
    let mut call = Call::on("Plan9", "mips");
    call.awf_bin = Some(custom.clone());
    call.args = vec!["a b".to_string(), String::new()];
    call.exit = Some(7);
    rep.check(&format!("awf（{tag}）：AWF_BIN 有設就用它"), made.and_then(|_| expect_run(stage, interp, &stage.empty.join("awf"), &call, "my-awf")));

    let mut call = Call::on("Linux", "x86_64");
    call.awf_bin = Some(stage.tmp.path().join("沒有這個檔"));
    rep.check(&format!("awf（{tag}）：AWF_BIN 指到不存在的檔 → 127"), expect_127(stage, interp, &awf, &call, &["AWF_BIN", "沒有這個檔"]));

    rep.check(
        &format!("awf（{tag}）：執行檔不存在 → 127"),
        expect_127(stage, interp, &stage.empty.join("awf"), &Call::on("Linux", "aarch64"), &["找不到執行檔", "awf-linux-arm64", "AWF_BIN"]),
    );
    rep.check(
        &format!("awf（{tag}）：不支援的作業系統 → 127"),
        expect_127(stage, interp, &awf, &Call::on("FreeBSD", "amd64"), &["不支援這個平台", "FreeBSD", "AWF_BIN"]),
    );
    rep.check(
        &format!("awf（{tag}）：不支援的架構 → 127"),
        expect_127(stage, interp, &awf, &Call::on("Linux", "riscv64"), &["不支援這個平台", "riscv64", "AWF_BIN"]),
    );
}

// ------------------------------------------------------------------ 實際執行：awf.cmd（Windows）

fn cmd_call(launcher: &Path, args: &[&str], awf_bin: Option<&Path>, exit: Option<i32>) -> Result<util::Ran, String> {
    let mut cmd = Command::new(launcher);
    cmd.arg(ECHO_COMMAND).args(args).env_remove("AWF_BIN").env_remove("FAKE_EXIT");
    if let Some(b) = awf_bin {
        cmd.env("AWF_BIN", b);
    }
    if let Some(code) = exit {
        cmd.env("FAKE_EXIT", code.to_string());
    }
    util::run(&mut cmd, None)
}

fn cmd_runtime(rep: &mut Report, cmd_src: &Path) {
    let setup = (|| {
        let tmp = TempDir::new("awf-launchers-cmd-")?;
        let bin = tmp.path().join("bin dir");
        let empty = tmp.path().join("empty");
        let own = tmp.path().join("own build");
        for d in [&bin, &empty, &own] {
            mkdir(d)?;
        }
        let me = std::env::current_exe().map_err(|e| format!("找不到 xtask 自己：{e}"))?;
        let copy = |from: &Path, to: &Path| std::fs::copy(from, to).map(|_| ()).map_err(|e| format!("複製到 {} 失敗：{e}", to.display()));
        copy(cmd_src, &bin.join("awf.cmd"))?;
        copy(cmd_src, &empty.join("awf.cmd"))?;
        copy(&me, &bin.join("awf-windows-x64.exe"))?;
        copy(&me, &own.join("my-awf.exe"))?;
        Ok::<_, String>((tmp, bin, empty, own))
    })();
    let (_tmp, bin, empty, own) = match setup {
        Ok(v) => v,
        Err(e) => {
            rep.check("awf.cmd：佈置暫存目錄", Err(e));
            return;
        }
    };
    let strs = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let args = ["a b", "*", "x=y"];
    rep.check(
        "awf.cmd：挑到 awf-windows-x64.exe、參數原樣傳到",
        cmd_call(&bin.join("awf.cmd"), &args, None, None).and_then(|ran| {
            let want = echo_text("awf-windows-x64.exe", &strs(&args));
            ensure(ran.code == 0 && ran.stdout == want, || format!("應該是 {want:?}，實際 exit {} {:?}（{}）", ran.code, ran.stdout, ran.stderr.trim()))
        }),
    );
    rep.check(
        "awf.cmd：exit code 原樣傳回",
        cmd_call(&bin.join("awf.cmd"), &[], None, Some(42)).and_then(|ran| ensure(ran.code == 42, || format!("應該是 42，實際 {}", ran.code))),
    );
    rep.check(
        "awf.cmd：AWF_BIN 有設就用它",
        cmd_call(&empty.join("awf.cmd"), &["p q"], Some(&own.join("my-awf.exe")), Some(7)).and_then(|ran| {
            let want = echo_text("my-awf.exe", &strs(&["p q"]));
            ensure(ran.code == 7 && ran.stdout == want, || format!("應該是 exit 7 {want:?}，實際 exit {} {:?}（{}）", ran.code, ran.stdout, ran.stderr.trim()))
        }),
    );
    rep.check(
        "awf.cmd：執行檔不存在 → 127",
        cmd_call(&empty.join("awf.cmd"), &[], None, None).and_then(|ran| {
            ensure(ran.code == 127 && ran.stdout.is_empty() && ran.stderr.contains("not found") && ran.stderr.contains("awf-windows-x64.exe"), || {
                format!("應該是 exit 127 與找不到的訊息，實際 exit {} {:?} {:?}", ran.code, ran.stdout, ran.stderr)
            })
        }),
    );
}

// ------------------------------------------------------------------ 入口

/// `cargo xtask check-launchers`。
pub fn run_captured(root: &Path) -> Captured {
    let bin_dir = root.join("plugin").join("bin");
    let (sh_path, cmd_path) = (bin_dir.join("awf"), bin_dir.join("awf.cmd"));
    let mut rep = Report::default();

    match std::fs::read(&sh_path) {
        Ok(bytes) => sh_static(&mut rep, root, &sh_path, &bytes),
        Err(e) => rep.check("awf：讀得到", Err(format!("{}：{e}", sh_path.display()))),
    }
    match std::fs::read(&cmd_path) {
        Ok(bytes) => cmd_static(&mut rep, &bytes),
        Err(e) => rep.check("awf.cmd：讀得到", Err(format!("{}：{e}", cmd_path.display()))),
    }
    attrs_static(&mut rep, root);

    if cfg!(unix) {
        if sh_path.is_file() {
            match stage_sh(&sh_path) {
                Ok(stage) => {
                    sh_runtime(&mut rep, &stage, "sh", None);
                    match util::which("dash") {
                        Some(dash) => sh_runtime(&mut rep, &stage, "dash", Some(&dash)),
                        None => rep.skip("awf（dash）：實際執行", "沒有安裝 dash"),
                    }
                }
                Err(e) => rep.check("awf：佈置暫存目錄", Err(e)),
            }
        }
        rep.skip("awf.cmd：實際執行", "不是 Windows，只做靜態檢查");
    } else {
        rep.skip("awf：實際執行", "不是 unix，只做靜態檢查");
        if cfg!(windows) && cmd_path.is_file() {
            cmd_runtime(&mut rep, &cmd_path);
        }
    }

    let mut out = rep.lines.join("\n");
    out.push('\n');
    if rep.failed == 0 {
        out.push_str(&format!("啟動器檢查通過：{} 項（略過 {} 項）\n", rep.passed, rep.skipped));
        (0, out, String::new())
    } else {
        (1, out, format!("啟動器檢查失敗：{} 項沒過（通過 {} 項、略過 {} 項）\n", rep.failed, rep.passed, rep.skipped))
    }
}
