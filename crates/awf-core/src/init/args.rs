//! `init` 的指令列：照 argparse 的規則解析（選項可以寫唯一的前綴、`--選項=值`、一個選填的位置參數）。
//!
//! 錯誤訊息的文字和 argparse（Python 3.12）相同；`usage:` 那一行與 `-h` 的說明沒有照抄。
//! `merge_gate::pyargs` 沒有位置參數，所以這裡另外寫一份（規則相同）。

use crate::pyfmt::repr_str;

/// 解析完的選項（欄位名對應 Python 版的 `a.<名>`）。
#[derive(Debug, Default, Clone)]
pub struct Args {
    pub positional: Option<String>,
    pub target: Option<String>,
    pub config: Option<String>,
    pub answers: Option<String>,
    pub upgrade: bool,
    pub adopt_new_fields: bool,
    pub force: bool,
    pub overwrite_user_files: bool,
    pub hooks: String,
    pub with_mcp: bool,
    pub with_workflows: bool,
    pub with_merge_gate: bool,
    pub defaults: bool,
    /// 0.x 的選項（`--hooks project` 用的 Python）。1.0 不再需要：收下、提示一行、忽略，舊的腳本才不會壞。
    pub python: Option<String>,
    /// 模版根目錄（Rust 版新增；Python 版用 init.py 自己的位置）。
    pub source: Option<String>,
    /// `--with-merge-gate` 要放進專案的 linux-x64 執行檔（沒給用模版的 `plugin/bin/awf-linux-x64`）。
    pub merge_gate_bin: Option<String>,
    /// 搭配 `--merge-gate-bin`：記這個執行檔 sha256 的 `checksums.json`（沒給就依執行檔產生一份）。
    pub merge_gate_checksums: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ArgStop {
    /// `-h`／`--help`：印說明、exit 0。
    Help,
    /// argparse 的 error：印在 stderr、exit 2。
    Error(String),
}

/// （選項名, 要不要帶值）。順序同 Python 版 `add_argument` 的順序（「ambiguous option」訊息照這個順序列）。
const OPTS: &[(&str, bool)] = &[
    ("--target", true),
    ("--config", true),
    ("--answers", true),
    ("--upgrade", false),
    ("--adopt-new-fields", false),
    ("--force", false),
    ("--overwrite-user-files", false),
    ("--hooks", true),
    ("--with-mcp", false),
    ("--with-workflows", false),
    ("--with-merge-gate", false),
    ("--defaults", false),
    ("--python", true),
    ("--source", true),
    ("--merge-gate-bin", true),
    ("--merge-gate-checksums", true),
];

pub const USAGE: &str = "usage: awf init [-h] [--target TARGET] [--config CONFIG] [--answers ANSWERS] [--upgrade] [--adopt-new-fields] [--force] \
[--overwrite-user-files] [--hooks HOOKS] [--with-mcp] [--with-workflows] [--with-merge-gate] [--defaults] [--source SOURCE] [--merge-gate-bin MERGE_GATE_BIN] [--merge-gate-checksums MERGE_GATE_CHECKSUMS] [positional]";

enum Seen {
    Plain,
    Known(usize, Option<String>),
    Help,
    Unknown,
}

fn negative_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else { return false };
    let all_digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    if all_digits(rest) {
        return true;
    }
    match rest.split_once('.') {
        Some((a, b)) => a.chars().all(|c| c.is_ascii_digit()) && all_digits(b),
        None => false,
    }
}

fn find(name: &str) -> Option<usize> {
    OPTS.iter().position(|(n, _)| *n == name)
}

fn classify(arg: &str) -> Result<Seen, String> {
    if !arg.starts_with('-') {
        return Ok(Seen::Plain);
    }
    if arg == "-h" || arg == "--help" {
        return Ok(Seen::Help);
    }
    if let Some(i) = find(arg) {
        return Ok(Seen::Known(i, None));
    }
    if arg.chars().count() == 1 {
        return Ok(Seen::Plain);
    }
    let (head, explicit) = match arg.split_once('=') {
        Some((h, v)) => (h, Some(v.to_string())),
        None => (arg, None),
    };
    if explicit.is_some() {
        if let Some(i) = find(head) {
            return Ok(Seen::Known(i, explicit));
        }
    }
    if arg.starts_with("--") {
        let mut names: Vec<&str> = OPTS.iter().map(|(n, _)| *n).filter(|n| n.starts_with(head)).collect();
        let help = "--help".starts_with(head);
        if help {
            names.insert(0, "--help");
        }
        if names.len() > 1 {
            return Err(format!("ambiguous option: {arg} could match {}", names.join(", ")));
        }
        if help {
            return Ok(Seen::Help);
        }
        if let Some(i) = names.first().and_then(|n| find(n)) {
            return Ok(Seen::Known(i, explicit));
        }
    }
    if negative_number(arg) || arg.contains(' ') {
        return Ok(Seen::Plain);
    }
    Ok(Seen::Unknown)
}

/// 解析 `awf init` 後面的參數。
pub fn parse(argv: &[String]) -> Result<Args, ArgStop> {
    let mut a = Args { hooks: "plugin".to_string(), ..Args::default() };
    let mut extras: Vec<String> = Vec::new();
    let mut have_positional = false;
    let mut i = 0;
    while i < argv.len() {
        let Some(arg) = argv.get(i) else { break };
        i += 1;
        if arg == "--" {
            // `--` 之後全部當位置參數
            let rest = argv.get(i..).unwrap_or_default();
            if have_positional {
                extras.push(arg.clone());
                extras.extend(rest.iter().cloned());
            } else if let Some((first, more)) = rest.split_first() {
                a.positional = Some(first.clone());
                extras.extend(more.iter().cloned());
            }
            break;
        }
        let (idx, explicit) = match classify(arg).map_err(ArgStop::Error)? {
            Seen::Help => return Err(ArgStop::Help),
            Seen::Plain => {
                if have_positional {
                    extras.push(arg.clone());
                } else {
                    have_positional = true;
                    a.positional = Some(arg.clone());
                }
                continue;
            }
            Seen::Unknown => {
                extras.push(arg.clone());
                continue;
            }
            Seen::Known(idx, explicit) => (idx, explicit),
        };
        let Some(&(name, takes)) = OPTS.get(idx) else { continue };
        if !takes {
            if let Some(v) = explicit {
                return Err(ArgStop::Error(format!("argument {name}: ignored explicit argument {}", repr_str(&v))));
            }
            match name {
                "--upgrade" => a.upgrade = true,
                "--adopt-new-fields" => a.adopt_new_fields = true,
                "--force" => a.force = true,
                "--overwrite-user-files" => a.overwrite_user_files = true,
                "--with-mcp" => a.with_mcp = true,
                "--with-workflows" => a.with_workflows = true,
                "--with-merge-gate" => a.with_merge_gate = true,
                "--defaults" => a.defaults = true,
                _ => {}
            }
            continue;
        }
        let value = match explicit {
            Some(v) => v,
            None => {
                let next = argv.get(i).filter(|n| n.as_str() != "--");
                let plain = match next {
                    Some(n) => matches!(classify(n).map_err(ArgStop::Error)?, Seen::Plain),
                    None => false,
                };
                if !plain {
                    return Err(ArgStop::Error(format!("argument {name}: expected one argument")));
                }
                i += 1;
                next.cloned().unwrap_or_default()
            }
        };
        match name {
            "--target" => a.target = Some(value),
            "--config" => a.config = Some(value),
            "--answers" => a.answers = Some(value),
            "--hooks" => a.hooks = value,
            "--python" => a.python = Some(value),
            "--source" => a.source = Some(value),
            "--merge-gate-bin" => a.merge_gate_bin = Some(value),
            "--merge-gate-checksums" => a.merge_gate_checksums = Some(value),
            _ => {}
        }
    }
    if !extras.is_empty() {
        return Err(ArgStop::Error(format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(a: &[&str]) -> Result<Args, ArgStop> {
        parse(&a.iter().map(|x| x.to_string()).collect::<Vec<_>>())
    }

    fn msg(a: &[&str]) -> String {
        match run(a) {
            Err(ArgStop::Error(m)) => m,
            _ => String::new(),
        }
    }

    #[test]
    fn errors_match_argparse() {
        assert_eq!(msg(&["a", "b"]), "unrecognized arguments: b");
        assert_eq!(msg(&["--nope"]), "unrecognized arguments: --nope");
        assert_eq!(msg(&["--target"]), "argument --target: expected one argument");
        assert_eq!(msg(&["--with-m"]), "ambiguous option: --with-m could match --with-mcp, --with-merge-gate");
        assert_eq!(msg(&["--h"]), "ambiguous option: --h could match --help, --hooks");
        assert_eq!(msg(&["--force=1"]), "argument --force: ignored explicit argument '1'");
        assert_eq!(msg(&["--merge-gate"]), "ambiguous option: --merge-gate could match --merge-gate-bin, --merge-gate-checksums");
        assert_eq!(run(&["x", "-h"]).err(), Some(ArgStop::Help));
    }

    #[test]
    fn values_match_argparse() {
        let a = run(&["--tar", "t", "--up", "--hooks=project", "--with-mc"]).ok().unwrap_or_default();
        assert_eq!(a.target.as_deref(), Some("t"));
        assert!(a.upgrade && a.with_mcp && !a.force);
        assert_eq!(a.hooks, "project");
        let b = run(&["--force", "dir", "--defaults"]).ok().unwrap_or_default();
        assert_eq!(b.positional.as_deref(), Some("dir"));
        assert!(b.force && b.defaults);
        let c = run(&["--python", "/x/py", "--merge-gate-b", "awf", "--merge-gate-checksums=c.json"]).ok().unwrap_or_default();
        assert_eq!((c.python.as_deref(), c.merge_gate_bin.as_deref(), c.merge_gate_checksums.as_deref()), (Some("/x/py"), Some("awf"), Some("c.json")));
    }
}
