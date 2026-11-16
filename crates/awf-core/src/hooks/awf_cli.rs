//! 認出 shell 段落裡的 `awf <子指令>`，並判斷 team agent 能不能跑。
//!
//! 1.0 之後工具不再是專案裡的 `.py`，而是單一執行檔 `awf`。`guard_shell.rs` 比對 `.py` 路徑的每一條規則，
//! 在這裡有一條對等的（舊規則全部保留）。原則：只能一樣或更嚴，看不出子指令就當成不允許。
//!
//! 認得的執行檔叫法：`awf`、帶路徑的 `…/bin/awf`、`awf.cmd`／`awf.exe`／`awf.bat`、平台檔名 `awf-<作業系統>-<架構>[.exe]`、
//! 加引號、前面有環境變數或 `env`／`command`／`exec`／`sudo`／`timeout N` 這類包裝、`sh …/bin/awf`、子 shell 與指令替換
//! （`$(awf …)`）、`bash -c`／`eval`／`pwsh -Command`／`cmd /c` 的內層、`xargs`／`find -exec` 這類轉呼叫。
//! 執行檔的位置是變數或萬用字元（看不到實際指令）時：後面接的是 awf 的子指令就照那個子指令判。

use super::guard::cmd_name;
use super::guard_shell::INTERP;
use super::ps::{ps_segments, segments, PS_NESTED_SHELLS};
use super::py::{self, shlex_tokens};
use super::re::Re;

static ENV_ASSIGN: Re = Re::new(r"^[A-Za-z_]\w*=", "");
/// 內嵌程式碼裡把 awf 當成一個詞（`['awf', 'save-run', …]`、`"…/bin/awf save-run"`）。
static INLINE_AWF: Re = Re::new(r"(?:^|[^\w.-])awf(?:-[a-z0-9_]+-[a-z0-9_]+)?(?:\.(?:exe|cmd|bat))?(?![\w-])", "i");
static CODE_FLAG: Re = Re::new(r"^(?:-[a-zA-Z]*[cer]|--eval|--exec|--print|--command|-p|-command)\z", "i");

pub fn all_patterns() -> Vec<&'static Re> {
    vec![&ENV_ASSIGN, &INLINE_AWF, &CODE_FLAG]
}

/// `awf` 的全部子指令（`crates/awf/src/main.rs` 的分派表）。awf 沒有全域旗標：第一個參數就是子指令。
pub const SUBCOMMANDS: &[&str] = &[
    "checklist", "kb", "recipe", "merge-gate", "sync-branch", "publish", "save-run", "deliverables", "locate", "pack", "cost", "script",
    "build", "start", "next", "done", "approve", "render", "handoff", "state", "watch", "statusline", "doctor", "spec", "init",
    "check-agents", "run-brief", "check-envelope", "check-rules", "hook",
    "golden", "tokens",
    "scrub",
    "kg",
    "program",
    "ci",
];
/// `awf kg` 的唯讀查詢，以及查詢可以帶的選項（值另外一個參數的、不帶值的）。清單外的選項（`--out`、`--full`…）一律當寫入。
const KG_QUERIES: &[&str] = &["locate", "trace", "impact", "check", "neighbors"];
const KG_VALUE_FLAGS: &[&str] = &["--root", "--profile", "--dir", "--diff", "--run", "--budget", "--top", "--depth"];
const KG_BARE_FLAGS: &[&str] = &["--json", "--"];
const STATE_VERBS: &[&str] = &["start", "next", "done", "approve", "render", "handoff"];
const HELP: &[&str] = &["-h", "--help"];

const SHELL_NAMES: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "fish", "busybox"];
const CMD_WRAPPERS: &[&str] = &["sudo", "command", "env", "nohup", "time", "nice", "timeout", "exec", "builtin", "stdbuf"];
/// 轉呼叫：子指令可能從 stdin 或別處來，看不到就當寫入。
const FORWARDERS: &[&str] = &[
    "xargs", "parallel", "watch", "find", "fd", "entr", "flock", "chronic", "unbuffer", "script", "su", "doas", "runuser", "setsid",
    "start-process", "saps", "start", "invoke-command", "icm",
];
/// 前面只有這些時，後面的 awf 就是被執行的指令。
const TRANSPARENT: &[&str] = &[
    "bash", "sh", "zsh", "dash", "ksh", "fish", "busybox", "cmd", "pwsh", "powershell", "&", ".", "source", "call",
    "if", "then", "else", "elif", "do", "while", "until", "!", "{", "(",
];
/// 這些指令把 awf 當成檔名或文字，不是執行它。
const HARMLESS: &[&str] = &[
    "cat", "ls", "ll", "dir", "grep", "egrep", "fgrep", "rg", "ag", "head", "tail", "less", "more", "wc", "file", "stat", "which",
    "whereis", "where", "type", "echo", "printf", "test", "[", "[[", "cd", "pushd", "mkdir", "rmdir", "rm", "cp", "mv", "ln", "chmod",
    "chown", "touch", "git", "gh", "rustc", "make", "diff", "cmp", "sha256sum", "shasum", "md5", "md5sum", "du", "tar", "zip", "unzip",
    "basename", "dirname", "realpath", "readlink", "strings", "nm", "otool", "ldd", "objdump", "codesign", "xattr", "lipo", "strip",
    "install", "sed", "awk", "sort", "uniq", "cut", "tr", "jq", "tee", "hexdump", "xxd", "od", "export", "set", "unset",
    "get-content", "gc", "get-item", "gi", "get-childitem", "gci", "test-path", "get-command", "gcm", "select-string", "sls",
    "write-host", "write-output", "get-filehash", "resolve-path",
];

fn has(list: &[&str], s: &str) -> bool {
    list.contains(&s)
}

/// 段落裡一次 `awf` 呼叫。
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// 子指令（awf 後面第一個參數）；看不到是空字串。
    pub sub: String,
    /// 子指令之後的參數。
    pub args: Vec<String>,
    /// 經 xargs／find -exec 這類轉呼叫執行。
    pub forwarded: bool,
    /// 直接執行（前面沒有別的指令），可以相信參數的位置。
    pub direct: bool,
    /// 看不清楚的原因（內嵌程式、執行檔是變數）。
    pub note: Option<&'static str>,
}

const NOTE_INLINE: &str = "（內嵌程式呼叫）";
const NOTE_OPAQUE: &str = "（執行檔是變數或萬用字元，看不到實際指令）";
const NOTE_DEEP: &str = "（巢狀太深，無法確認）";
const NOTE_MANY: &str = "（同一段裡 awf 呼叫太多，無法確認）";
const MAX_CALLS: usize = 32;
const NOTE_FORWARD: &str = "（經轉呼叫程式執行，看不到子指令）";

/// 是不是 awf 的執行檔名：`awf`，或平台檔名 `awf-<作業系統>-<架構>`（副檔名已由 `cmd_name` 去掉）。
fn is_exe(tok: &str) -> bool {
    let n = cmd_name(tok);
    let n = n.trim_end_matches('\n');
    if n == "awf" {
        return true;
    }
    let Some(rest) = n.strip_prefix("awf-") else { return false };
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
}

/// 去掉 token 開頭的 `$(`、`(`、反引號、`<(`、`{`、`!`、`&`、引號。回傳（剩下的, 是否開了子 shell 或指令替換）。
fn clean_head(tok: &str) -> (&str, bool) {
    let core = tok.trim_start_matches(['$', '(', '`', '<', '{', '!', '&', '@', '"', '\'']);
    let cut = &tok[..tok.len() - core.len()];
    (core, cut.contains('(') || cut.contains('`'))
}

fn clean_tail(tok: &str) -> String {
    tok.trim_end_matches([')', '`', ';', '}', '"', '\'']).to_string()
}

fn is_number(tok: &str) -> bool {
    let t = tok.trim_end_matches(['s', 'm', 'h', 'd']);
    !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit() || c == b'.')
}

fn is_opaque(tok: &str) -> bool {
    tok.contains(['$', '`', '*', '?', '['])
}

fn scan_tokens(toks: &[String], out: &mut Vec<Call>, depth: usize) {
    let mut seen = 0usize; // 前面算得上「指令」的 token 數
    let mut first = String::new();
    let mut forwarder = false;
    let mut transparent = true;
    let mut consumed = false; // 已經進到某次 awf 呼叫的參數裡
    for (k, t) in toks.iter().enumerate() {
        let (core, opened) = clean_head(t);
        if opened {
            seen = 0;
            forwarder = false;
            transparent = true;
            consumed = false;
            first.clear();
        }
        if ENV_ASSIGN.is_match(t) {
            continue; // AWF_BIN=/x/awf 這類設定不是指令
        }
        if out.len() >= MAX_CALLS {
            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_MANY) });
            return;
        }
        if !consumed && !core.is_empty() && is_exe(core) {
            let direct = seen == 0 || (transparent && !forwarder);
            // 不是直接執行時，「--」後面才是交給 awf 的參數（cargo run -p awf -- save-run …）
            let skip = usize::from(!direct && toks.get(k + 1).is_some_and(|x| x == "--"));
            let sub = toks.get(k + 1 + skip).map(|x| clean_tail(x)).unwrap_or_default();
            let take = if forwarder || direct {
                true
            } else if has(HARMLESS, &first) {
                false
            } else {
                has(SUBCOMMANDS, &sub) // 前面是不認得的指令：後面接的是 awf 的子指令才算
            };
            if take {
                let from = (k + 2 + skip).min(toks.len());
                // run-brief 的「--」後面是它要執行的指令，照同一套規則再看一次
                if sub == "run-brief" {
                    if let Some(d) = toks[from..].iter().position(|x| x == "--") {
                        if depth < 6 {
                            scan_tokens(&toks[from + d + 1..], out, depth + 1);
                        } else {
                            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_DEEP) });
                        }
                    }
                }
                let args: Vec<String> = toks[from..].iter().map(|x| clean_tail(x)).collect();
                out.push(Call { sub, args, forwarded: forwarder, direct: direct && !forwarder, note: None });
                consumed = true;
                continue;
            }
        }
        // 指令位置是變數、指令替換或萬用字元：看不到實際執行檔
        if !consumed && seen == 0 && !ENV_ASSIGN.is_match(t) && is_opaque(t) {
            let mut j = k;
            if (t.starts_with("$(") || t.starts_with('`')) && !(t.ends_with(')') || (t.len() > 1 && t.ends_with('`'))) {
                while j + 1 < toks.len() && j < k + 16 {
                    j += 1;
                    if toks[j].ends_with(')') || toks[j].ends_with('`') {
                        break;
                    }
                }
            }
            let sub = toks.get(j + 1).map(|x| clean_tail(x)).unwrap_or_default();
            if has(SUBCOMMANDS, &sub) {
                let args: Vec<String> = toks[(j + 2).min(toks.len())..].iter().map(|x| clean_tail(x)).collect();
                out.push(Call { sub, args, forwarded: false, direct: false, note: Some(NOTE_OPAQUE) });
                consumed = true;
            }
        }
        if ENV_ASSIGN.is_match(t) || t.starts_with('-') || is_number(t) {
            continue;
        }
        let name = cmd_name(core);
        if has(CMD_WRAPPERS, &name) {
            continue;
        }
        if seen == 0 {
            first = name.clone();
        }
        seen += 1;
        if has(FORWARDERS, &name) {
            forwarder = true;
        }
        if !has(TRANSPARENT, &name) {
            transparent = false;
        }
    }
}

fn is_dash_c(t: &str) -> bool {
    match t.strip_prefix('-') {
        Some(b) => b.bytes().all(|c| c.is_ascii_alphabetic()) && b.contains('c'),
        None => false,
    }
}

/// 便宜的預先過濾：去掉引號與跳脫字元後沒有「awf」、也沒有變數或萬用字元，就不可能是 awf 呼叫。
fn maybe(seg: &str) -> bool {
    if seg.contains(['$', '`', '*', '?', '[']) {
        return true;
    }
    let mut state = 0u8; // 已經比對到 "awf" 的前幾個字元
    for c in seg.chars() {
        if matches!(c, '"' | '\'' | '\\' | '^') {
            continue;
        }
        let c = c.to_ascii_lowercase();
        state = match (state, c) {
            (0, 'a') | (1, 'a') | (2, 'a') => 1,
            (1, 'w') => 2,
            (2, 'f') => return true,
            _ => 0,
        };
    }
    false
}

fn calls_at(seg: &str, depth: usize, out: &mut Vec<Call>) {
    if !maybe(seg) {
        return;
    }
    let toks = shlex_tokens(seg);
    let mut found: Vec<Call> = Vec::new();
    scan_tokens(&toks, &mut found, 0);
    if seg.contains('\\') {
        // Windows 路徑的反斜線在 POSIX 切詞會被吃掉（.\bin\awf.cmd）：換成 / 再看一次
        let mut alt = Vec::new();
        scan_tokens(&shlex_tokens(&seg.replace('\\', "/")), &mut alt, 0);
        for c in alt {
            if !found.contains(&c) {
                found.push(c);
            }
        }
    }
    out.append(&mut found);

    let mut head: Vec<&String> = toks.iter().filter(|x| !ENV_ASSIGN.is_match(x)).collect();
    while !head.is_empty() && has(CMD_WRAPPERS, &cmd_name(head[0])) {
        head.remove(0);
        while !head.is_empty() && (head[0].starts_with('-') || is_number(head[0])) {
            head.remove(0);
        }
    }
    let name = head.first().map(|h| cmd_name(h)).unwrap_or_default();
    let args: Vec<&String> = head.iter().skip(1).copied().collect();
    let nested_ps = has(PS_NESTED_SHELLS, &name) || name == "cmd";
    let mut inner_cmds: Vec<String> = Vec::new();
    if has(SHELL_NAMES, &name) {
        if let Some(k) = args.iter().enumerate().position(|(k, t)| is_dash_c(t) && k + 1 < args.len()) {
            inner_cmds.push(args[k + 1].clone());
        }
    } else if nested_ps {
        if let Some(k) = args.iter().enumerate().position(|(k, t)| matches!(py::lower(t).as_str(), "-c" | "-command" | "/c" | "/k") && k + 1 < args.len()) {
            inner_cmds.push(args[k + 1..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
        }
    } else if matches!(name.as_str(), "eval" | "invoke-expression" | "iex") {
        inner_cmds.push(args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
    } else if INTERP.is_match(seg) {
        // python -c、node -e 這類內嵌程式裡提到 awf：看不到它怎麼呼叫
        let inline = toks.windows(2).any(|w| CODE_FLAG.is_match(&w[0]) && INLINE_AWF.is_match(&w[1]));
        if inline {
            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_INLINE) });
        }
    }
    if depth >= 4 {
        if !inner_cmds.is_empty() && inner_cmds.iter().any(|ic| maybe(ic)) {
            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_DEEP) });
        }
        return;
    }
    for ic in inner_cmds {
        let inner = if nested_ps { ps_segments(&ic) } else { segments(&ic) };
        for s in inner {
            calls_at(&s, depth + 1, out);
        }
    }
}

/// 這一段裡所有的 awf 呼叫（含 `bash -c`、`eval`、`pwsh -Command`、`cmd /c` 的內層）。
pub fn calls(seg: &str) -> Vec<Call> {
    let mut out = Vec::new();
    calls_at(seg, 0, &mut out);
    out
}

/// 只看這一層（不展開內層 shell）。
pub fn calls_here(seg: &str) -> Vec<Call> {
    let mut out = Vec::new();
    if maybe(seg) {
        scan_tokens(&shlex_tokens(seg), &mut out, 0);
        if out.is_empty() && seg.contains('\\') {
            scan_tokens(&shlex_tokens(&seg.replace('\\', "/")), &mut out, 0);
        }
    }
    out
}

/// team agent 不能跑的理由。
#[derive(Debug, Clone, PartialEq)]
pub enum Deny {
    Pack,
    Deliverables,
    Kb,
    Kg(String),
    Program(String),
    SaveRun(String),
    State(String),
    Publish,
    MergeGate,
    Init,
    Build,
    Spec(String),
    Watch,
    Ci(String),
    Unknown(String),
    Opaque(&'static str),
}

impl Call {
    fn first(&self) -> &str {
        self.args.first().map(String::as_str).unwrap_or("")
    }

    /// 子指令的第一個參數要在唯讀清單裡；沒有參數時，直接執行放行（只會印用法），轉呼叫擋。
    fn write_sub(&self, readonly: &[&str]) -> Option<String> {
        let s = self.first();
        if has(readonly, s) {
            None
        } else if s.is_empty() {
            if self.forwarded { Some(NOTE_FORWARD.to_string()) } else { None }
        } else {
            Some(s.to_string())
        }
    }

    /// `awf kg`：只准唯讀的查詢子指令，而且只能帶查詢的選項。回傳被擋的原因。
    fn kg_deny(&self) -> Option<String> {
        let s = self.first();
        if s == "--selftest" || has(HELP, s) {
            return None;
        }
        if s.is_empty() {
            return if self.forwarded { Some(NOTE_FORWARD.to_string()) } else { None };
        }
        if !has(KG_QUERIES, s) {
            return Some(s.to_string());
        }
        let mut i = 1;
        while i < self.args.len() {
            let a = self.args[i].as_str();
            if a == "--" {
                break;
            }
            if a.starts_with("--") {
                let name = a.split('=').next().unwrap_or(a);
                if has(KG_VALUE_FLAGS, name) {
                    i += usize::from(!a.contains('='));
                } else if !has(KG_BARE_FLAGS, a) {
                    return Some(format!("{s} {name}"));
                }
            }
            i += 1;
        }
        None
    }

    /// `awf ci`：只准 render（不帶 --write）與 check，而且只能帶清單裡的選項。回傳被擋的原因。
    fn ci_deny(&self) -> Option<String> {
        let s = self.first();
        if s == "--selftest" || has(HELP, s) {
            return None;
        }
        if s.is_empty() {
            return if self.forwarded { Some(NOTE_FORWARD.to_string()) } else { None };
        }
        if s != "render" && s != "check" {
            return Some(s.to_string());
        }
        let mut i = 1;
        while i < self.args.len() {
            let a = self.args[i].as_str();
            if a == "--root" || a == "--profile" {
                i += 1;
            } else if a.starts_with('-') && !has(HELP, a) {
                return Some(format!("{s} {}", a.split('=').next().unwrap_or(a)));
            }
            i += 1;
        }
        None
    }

    /// `awf spec ids` 只准唯讀的旗標。
    fn spec_ids_writes(&self) -> bool {
        let mut i = 1;
        while i < self.args.len() {
            let a = self.args[i].as_str();
            if a == "--json" || a.starts_with("--root=") || a.starts_with("--id-pattern=") {
                i += 1;
            } else if a == "--root" || a == "--id-pattern" {
                i += 2;
            } else {
                return true;
            }
        }
        false
    }

    /// team agent 跑這次呼叫會被擋的理由；None＝可以跑。`steward`＝是 auto_merge 的 merger_role。
    pub fn team_deny(&self, steward: bool) -> Option<Deny> {
        if self.sub.is_empty() {
            if let Some(n) = self.note {
                return Some(Deny::Opaque(n));
            }
            return if self.forwarded { Some(Deny::Unknown(NOTE_FORWARD.to_string())) } else { None };
        }
        match self.sub.as_str() {
            "--version" | "-V" | "-h" | "--help" => None,
            "pack" => self.write_sub(&["--selftest"]).map(|_| Deny::Pack),
            "deliverables" => self.write_sub(&["status", "check", "affected", "--selftest", "-h", "--help"]).map(|_| Deny::Deliverables),
            "kb" => self.write_sub(&["query", "lint", "--selftest", "-h", "--help"]).map(|_| Deny::Kb),
            "kg" => self.kg_deny().map(Deny::Kg),
            "program" => self.write_sub(&["status", "--selftest", "-h", "--help"]).map(Deny::Program),
            "save-run" => self.write_sub(&["status", "--selftest"]).map(Deny::SaveRun),
            "state" => {
                if self.first() == "--selftest" { None } else { Some(Deny::State(format!("state {}", self.first()).trim_end().to_string())) }
            }
            s if has(STATE_VERBS, s) => Some(Deny::State(s.to_string())),
            "publish" => Some(Deny::Publish),
            "merge-gate" => {
                let f = self.first();
                if steward || f == "--selftest" || has(HELP, f) || (f.is_empty() && !self.forwarded) { None } else { Some(Deny::MergeGate) }
            }
            "init" => Some(Deny::Init),
            "build" => Some(Deny::Build),
            "spec" => match self.first() {
                "status" | "validate" | "show" | "--selftest" | "-h" | "--help" => None,
                "" => if self.forwarded { Some(Deny::Spec(NOTE_FORWARD.to_string())) } else { None },
                "ids" => if self.spec_ids_writes() { Some(Deny::Spec("ids --write／--adopt／--retire".into())) } else { None },
                other => Some(Deny::Spec(other.to_string())),
            },
            "watch" => {
                let ok = self.args.iter().any(|a| a == "--once" || a == "--selftest") || has(HELP, self.first());
                if ok { None } else { Some(Deny::Watch) }
            }
            "statusline" | "doctor" | "hook" | "checklist" | "recipe" | "sync-branch" | "locate" | "cost" | "script" | "check-agents"
            | "run-brief" | "check-envelope" | "check-rules" => None,
            // 兩支都只讀檔（golden：跑 golden-set；tokens：check／verify 掃色碼與比對正本），沒有會寫檔的子指令或選項
            "golden" | "tokens" => None,
            // 去客戶化掃描：只讀檔、只印路徑與規則編號，沒有會寫檔的選項
            "scrub" => None,
            // 專案的 CI：render（只印）與 check 可以跑；--write 會寫 .github/workflows/ 與 guard.config.json，只由主 session 跑
            "ci" => self.ci_deny().map(Deny::Ci),
            other => Some(Deny::Unknown(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subs(seg: &str) -> Vec<String> {
        calls(seg).into_iter().map(|c| format!("{} {}", c.sub, c.args.join(" ")).trim().to_string()).collect()
    }

    fn denied(seg: &str) -> bool {
        calls(seg).iter().any(|c| c.team_deny(false).is_some())
    }

    #[test]
    fn patterns_compile() {
        for r in all_patterns() {
            assert!(r.compiles(), "樣式編不過：{}", r.source());
        }
    }

    #[test]
    fn spellings() {
        for c in [
            "awf save-run approve r", "./awf save-run approve r", "/p/bin/awf save-run approve r", "\"/p a/bin/awf\" save-run approve r",
            "awf.cmd save-run approve r", "AWF.EXE save-run approve r", "bin/awf-darwin-arm64 save-run approve r",
            "bin\\awf-windows-x64.exe save-run approve r", ".\\bin\\awf.cmd save-run approve r", "X=1 awf save-run approve r", "AWF_BIN=/x/awf plugin/bin/awf kb build",
            "env X=1 awf save-run approve r", "command awf save-run approve r", "exec awf save-run approve r",
            "timeout 5 awf save-run approve r", "sh plugin/bin/awf save-run approve r", "a\"w\"f save-run approve r",
            "echo $(awf save-run approve r)", "echo `awf save-run approve r`", "\"${CLAUDE_PLUGIN_ROOT}/bin/awf\" save-run approve r",
            "bash -c 'awf save-run approve r'", "bash -lc \"exec awf save-run approve r\"", "eval awf save-run approve r",
            "eval 'a''wf save-run approve r'", "pwsh -Command \"awf save-run approve r\"", "cmd /c awf.cmd save-run approve r",
            "xargs awf save-run", "find . -exec awf save-run approve {} +", "$AWF_BIN save-run approve r", "$(which awf) save-run approve r",
            "bin/aw? save-run approve r", "python3 -c \"import subprocess; subprocess.run(['awf','save-run','approve'])\"",
            "ionice awf save-run approve r", "cargo run -q -p awf -- save-run approve r", "if awf save-run approve r", "awf run-brief --log x.log -- awf save-run approve r",
        ] {
            assert!(denied(c), "應該擋：{c}");
        }
        for c in [
            "awf save-run status r", "awf kb query x", "cat plugin/bin/awf", "ls -l plugin/bin/awf save-run", "grep -r awf build",
            "git commit -m 'awf save-run approve'", "echo awf save-run approve", "awf kb query awf build", "awf", "awf --version",
            "cargo build -p awf", "cargo test -p awf --test hooks_entry", "go build -o awf ./cmd", "$AWF_BIN kb query x", "awf doctor", "awf watch --once", "awf statusline",
            "awf hook guard-shell", "awf spec status", "awf spec ids --json", "awf state --selftest", "awf merge-gate --selftest",
            "which awf", "command -v awf", "AWF_BIN=target/debug/awf awf kb query x", "make awf build", "awf run-brief --log x.log -- npm test",
            "awf golden", "awf golden --dir .claude/teams/golden-set --verbose", "awf tokens check src", "awf tokens verify", "awf tokens --selftest",
            "awf scrub", "awf scrub docs --json", "awf scrub --selftest",
        ] {
            assert!(!denied(c), "應該放：{c}（{:?}）", calls(c));
        }
    }

    #[test]
    fn verdicts() {
        assert_eq!(subs("awf save-run approve r"), vec!["save-run approve r"]);
        let d = |c: &str| calls(c)[0].team_deny(false);
        assert_eq!(d("awf pack plan r"), Some(Deny::Pack));
        assert_eq!(d("awf pack --selftest"), None);
        assert_eq!(d("awf deliverables sync"), Some(Deny::Deliverables));
        assert_eq!(d("awf kb build"), Some(Deny::Kb));
        assert_eq!(d("awf kg build"), Some(Deny::Kg("build".into())));
        assert_eq!(d("awf kg export --out x"), Some(Deny::Kg("export".into())));
        assert_eq!(d("awf kg locate x --out y"), Some(Deny::Kg("locate --out".into())));
        assert_eq!(d("awf kg check --full"), Some(Deny::Kg("check --full".into())));
        for ok in ["awf kg locate 匯出 發票", "awf kg trace billing/export-invoice --json", "awf kg impact --diff main", "awf kg check --run _tpm/runs/r", "awf kg neighbors a#b --depth=3", "awf kg --selftest", "awf kg"] {
            assert_eq!(d(ok), None, "{ok}");
        }
        assert_eq!(d("awf ci render --write"), Some(Deny::Ci("render --write".into())));
        assert_eq!(d("awf ci render --root . --write"), Some(Deny::Ci("render --write".into())));
        assert_eq!(d("awf ci render --write=1"), Some(Deny::Ci("render --write".into())));
        assert_eq!(d("awf ci install"), Some(Deny::Ci("install".into())));
        for ok in ["awf ci render", "awf ci render --root . --profile p.json", "awf ci check", "awf ci --selftest", "awf ci", "awf ci --help"] {
            assert_eq!(d(ok), None, "{ok}");
        }
        assert_eq!(d("awf save-run close r"), Some(Deny::SaveRun("close".into())));
        assert_eq!(d("awf next r"), Some(Deny::State("next".into())));
        assert_eq!(d("awf state next"), Some(Deny::State("state next".into())));
        assert_eq!(d("awf publish --selftest"), Some(Deny::Publish));
        assert_eq!(d("awf merge-gate check --pr 3"), Some(Deny::MergeGate));
        assert_eq!(calls("awf merge-gate check --pr 3")[0].team_deny(true), None);
        assert_eq!(d("awf init --target ."), Some(Deny::Init));
        assert_eq!(d("awf build --check"), Some(Deny::Build));
        assert_eq!(d("awf spec apply --change x"), Some(Deny::Spec("apply".into())));
        assert!(d("awf spec ids --write").is_some());
        assert!(d("awf spec ids --adopt a=b").is_some());
        assert!(d("awf spec ids --root x --retire y").is_some());
        assert_eq!(d("awf spec ids --root x --json"), None);
        assert_eq!(d("awf watch"), Some(Deny::Watch));
        assert_eq!(d("awf bogus"), Some(Deny::Unknown("bogus".into())));
        assert_eq!(d("awf $X"), Some(Deny::Unknown("$X".into())));
    }
}
