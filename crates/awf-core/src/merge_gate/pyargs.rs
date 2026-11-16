//! 照 argparse 的規則解析指令列（子指令＋`--選項 值`）。
//!
//! 兩支閘門工具原本用 argparse，這裡照它的行為：選項可以寫前綴（唯一才算）、`--選項=值`、重複給取最後一個、
//! 值不能長得像選項（負數與含空白的字串例外）。錯誤訊息的文字和 argparse（Python 3.12）相同，
//! sync-branch 把它放進輸出的 reason。`usage:` 那幾行沒有照抄。

use super::pyutil::int_of_str;
use crate::pyfmt::repr_str;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    Int,
    Flag,
    Append,
    Choice(&'static [&'static str]),
}

pub struct Opt {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
}

pub const fn opt(name: &'static str, kind: Kind) -> Opt {
    Opt { name, kind, required: false }
}

pub const fn req(name: &'static str, kind: Kind) -> Opt {
    Opt { name, kind, required: true }
}

#[derive(Debug, PartialEq)]
pub enum ArgErr {
    /// `-h`／`--help`：argparse 印說明後以 0 結束。
    Help,
    /// 最上層 parser 的錯誤（缺子指令、不認得的子指令、多餘的參數）。
    Top(String),
    /// 子指令 parser 的錯誤。
    Sub(String),
}

impl ArgErr {
    pub fn message(&self) -> &str {
        match self {
            ArgErr::Help => "",
            ArgErr::Top(m) | ArgErr::Sub(m) => m,
        }
    }
}

#[derive(Default)]
pub struct Parsed {
    vals: HashMap<&'static str, Vec<String>>,
}

impl Parsed {
    /// 選項的值（重複給時取最後一個）；沒給是 None。
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vals.get(name).and_then(|v| v.last()).map(String::as_str)
    }

    /// 真值判斷後的值：沒給或空字串都是 None（對應 `if a.x:`、`a.x or …`）。
    pub fn truthy(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|s| !s.is_empty())
    }

    pub fn all(&self, name: &str) -> Vec<String> {
        self.vals.get(name).cloned().unwrap_or_default()
    }

    pub fn flag(&self, name: &str) -> bool {
        self.vals.contains_key(name)
    }

    /// `type=int` 的值（解析時已經驗過）。
    pub fn int(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(int_of_str).and_then(|n| i64::try_from(n).ok())
    }
}

fn negative_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else { return false };
    let all_digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    if all_digits(rest) { return true; }
    match rest.split_once('.') {
        Some((a, b)) => a.chars().all(|c| c.is_ascii_digit()) && all_digits(b),
        None => false,
    }
}

enum Seen<'a> {
    /// 不是選項（當成值或位置參數）。
    Plain,
    /// 認得的選項與 `=` 後面的值。
    Known(&'a Opt, Option<String>),
    Help,
    /// 長得像選項但不認得。
    Unknown,
}

fn classify<'a>(opts: &'a [Opt], arg: &str) -> Result<Seen<'a>, String> {
    if !arg.starts_with('-') { return Ok(Seen::Plain); }
    if arg == "-h" || arg == "--help" { return Ok(Seen::Help); }
    if let Some(o) = opts.iter().find(|o| o.name == arg) { return Ok(Seen::Known(o, None)); }
    if arg.chars().count() == 1 { return Ok(Seen::Plain); }
    let (head, explicit) = match arg.split_once('=') {
        Some((h, v)) => (h, Some(v.to_string())),
        None => (arg, None),
    };
    if explicit.is_some() {
        if let Some(o) = opts.iter().find(|o| o.name == head) { return Ok(Seen::Known(o, explicit)); }
    }
    if arg.starts_with("--") {
        let mut names: Vec<&str> = opts.iter().map(|o| o.name).filter(|n| n.starts_with(head)).collect();
        let help = "--help".starts_with(head);
        if help { names.insert(0, "--help"); }
        if names.len() > 1 {
            return Err(format!("ambiguous option: {arg} could match {}", names.join(", ")));
        }
        if help { return Ok(Seen::Help); }
        if let Some(o) = opts.iter().find(|o| names.first() == Some(&o.name)) { return Ok(Seen::Known(o, explicit)); }
    }
    if negative_number(arg) || arg.contains(' ') { return Ok(Seen::Plain); }
    Ok(Seen::Unknown)
}

/// 最上層：找出子指令。回傳（子指令、它後面的參數、子指令之前不認得的參數）。
pub fn split_cmd<'a>(argv: &'a [String], cmds: &[&str]) -> Result<(&'a str, &'a [String], Vec<String>), ArgErr> {
    let mut extras = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        if a == "--" {
            extras.extend(argv[i..].iter().cloned());
            break;
        }
        match classify(&[], a).map_err(ArgErr::Top)? {
            Seen::Help => return Err(ArgErr::Help),
            Seen::Plain => {
                if !cmds.contains(&a.as_str()) {
                    let list = cmds.iter().map(|c| repr_str(c)).collect::<Vec<_>>().join(", ");
                    return Err(ArgErr::Top(format!("argument cmd: invalid choice: {} (choose from {list})", repr_str(a))));
                }
                return Ok((a.as_str(), &argv[i + 1..], extras));
            }
            _ => extras.push(a.clone()),
        }
    }
    Err(ArgErr::Top("the following arguments are required: cmd".into()))
}

/// 子指令的選項。`extras` 是最上層已經收到的多餘參數；最後還有多餘的就報 `unrecognized arguments`。
pub fn parse(opts: &[Opt], argv: &[String], mut extras: Vec<String>) -> Result<Parsed, ArgErr> {
    let mut out = Parsed::default();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if a == "--" {
            extras.push(a.clone());
            extras.extend(argv[i..].iter().cloned());
            break;
        }
        let (o, explicit) = match classify(opts, a).map_err(ArgErr::Sub)? {
            Seen::Help => return Err(ArgErr::Help),
            Seen::Plain | Seen::Unknown => {
                extras.push(a.clone());
                continue;
            }
            Seen::Known(o, explicit) => (o, explicit),
        };
        if o.kind == Kind::Flag {
            if let Some(v) = explicit {
                return Err(ArgErr::Sub(format!("argument {}: ignored explicit argument {}", o.name, repr_str(&v))));
            }
            out.vals.entry(o.name).or_default().push(String::new());
            continue;
        }
        let value = match explicit {
            Some(v) => v,
            None => {
                let next = argv.get(i).filter(|n| n.as_str() != "--");
                let plain = match next {
                    Some(n) => matches!(classify(opts, n).map_err(ArgErr::Sub)?, Seen::Plain),
                    None => false,
                };
                if !plain { return Err(ArgErr::Sub(format!("argument {}: expected one argument", o.name))); }
                i += 1;
                next.cloned().unwrap_or_default()
            }
        };
        match o.kind {
            Kind::Int => {
                if int_of_str(&value).and_then(|n| i64::try_from(n).ok()).is_none() {
                    return Err(ArgErr::Sub(format!("argument {}: invalid int value: {}", o.name, repr_str(&value))));
                }
            }
            Kind::Choice(list) => {
                if !list.contains(&value.as_str()) {
                    let shown = list.iter().map(|c| repr_str(c)).collect::<Vec<_>>().join(", ");
                    return Err(ArgErr::Sub(format!("argument {}: invalid choice: {} (choose from {shown})", o.name, repr_str(&value))));
                }
            }
            _ => {}
        }
        let slot = out.vals.entry(o.name).or_default();
        if o.kind != Kind::Append { slot.clear(); }
        slot.push(value);
    }
    let missing: Vec<&str> = opts.iter().filter(|o| o.required && !out.vals.contains_key(o.name)).map(|o| o.name).collect();
    if !missing.is_empty() {
        return Err(ArgErr::Sub(format!("the following arguments are required: {}", missing.join(", "))));
    }
    if !extras.is_empty() {
        return Err(ArgErr::Top(format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: &[Opt] = &[req("--branch", Kind::Str), opt("--base", Kind::Str), opt("--strategy", Kind::Choice(&["a", "b"])),
        opt("--pr", Kind::Int), opt("--no-fetch", Kind::Flag), opt("--findings", Kind::Append)];

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    fn run(a: &[&str]) -> Result<Parsed, ArgErr> {
        let argv = args(a);
        let (_, rest, extras) = split_cmd(&argv, &["plan"])?;
        parse(OPTS, rest, extras)
    }

    fn msg(a: &[&str]) -> String {
        run(a).err().map(|e| e.message().to_string()).unwrap_or_default()
    }

    #[test]
    fn errors_match_argparse() {
        assert_eq!(msg(&["plan", "--strategy", "z"]), "argument --strategy: invalid choice: 'z' (choose from 'a', 'b')");
        assert_eq!(msg(&["zz"]), "argument cmd: invalid choice: 'zz' (choose from 'plan')");
        assert_eq!(msg(&[]), "the following arguments are required: cmd");
        assert_eq!(msg(&["plan", "--b", "1"]), "ambiguous option: --b could match --branch, --base");
        assert_eq!(msg(&["plan", "--branch"]), "argument --branch: expected one argument");
        assert_eq!(msg(&["--foo", "plan", "--branch", "x", "y", "--z"]), "unrecognized arguments: --foo y --z");
        assert_eq!(msg(&["plan", "--branch", "x", "--", "q"]), "unrecognized arguments: -- q");
        assert_eq!(msg(&["plan"]), "the following arguments are required: --branch");
        assert_eq!(msg(&["plan", "--branch", "x", "--pr", "1.5"]), "argument --pr: invalid int value: '1.5'");
        assert_eq!(run(&["plan", "-h"]).err(), Some(ArgErr::Help));
    }

    #[test]
    fn values_match_argparse() {
        let p = run(&["plan", "--bra=x", "--branch", "y", "--pr", "-3", "--findings", "a", "--findings=b", "--no-f", "--base", "has space -x"]).ok().expect("解析");
        assert_eq!(p.get("--branch"), Some("y"));
        assert_eq!(p.int("--pr"), Some(-3));
        assert_eq!(p.all("--findings"), vec!["a", "b"]);
        assert!(p.flag("--no-fetch"));
        assert_eq!(p.get("--base"), Some("has space -x"));
        assert_eq!(p.get("--strategy"), None);
    }
}
