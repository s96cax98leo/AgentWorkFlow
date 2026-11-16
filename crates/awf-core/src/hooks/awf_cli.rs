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
use super::subst;

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
/// 會寫檔的旗標（各子指令實際的 parser 對過一遍）：放行的唯讀子指令帶了其中一個就不算唯讀。
/// `awf checklist check --write` 改寫指定的檔、`awf script … --out` 寫組裝結果；其餘是「之後哪個子指令加了也一樣擋」。
const WRITE_FLAGS: &[&str] = &["--write", "--out", "--output", "-o", "--log", "--update", "--record", "--save", "--fix", "--reviewed"];
/// 會被原樣轉給 git 的參數裡不該出現的 git 選項：`--output=<檔>` 讓 git 把輸出寫成檔，`--upload-pack=<指令>`、`--exec=<指令>` 會執行指令。
/// （`awf check-rules --git <repo> <範圍>`、`awf recipe check-diff --base <ref>`、`awf sync-branch plan --remote <名>` 的值沒有擋 `-` 開頭。）
const GIT_INJECT: &[&str] = &["--output", "--upload-pack", "--receive-pack", "--exec", "--ext-diff", "--textconv", "--open-files-in-pager"];
/// 參數會轉給 git 的子指令。
const GIT_PASSING: &[&str] = &["recipe", "check-rules", "sync-branch", "locate", "cost", "deliverables", "kb"];
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

/// 建置、套件與檔案工具：`awf` 在它們的參數裡是 crate、目標或檔案的名字（`cargo build -p awf $FLAGS`、`rsync bin/awf "$DEST"`），
/// 不是被執行的指令。只用在「子指令看不到」的保守判斷；後面明寫 awf 子指令的照樣算（`cargo run -p awf -- save-run …`）。
const BUILDERS: &[&str] = &[
    "cargo", "rustup", "go", "npm", "npx", "pnpm", "yarn", "node", "pip", "pip3", "uv", "brew", "apt", "apt-get", "docker", "podman",
    "kubectl", "bazel", "cmake", "ninja", "gcc", "clang", "cc", "ld", "ar", "code", "open", "rsync", "scp", "sftp", "shellcheck", "bat",
    "vim", "vi", "nano", "emacs",
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
    let mut builder = false; // 前面有建置工具：awf 多半是它的參數
    for (k, t) in toks.iter().enumerate() {
        let (core, opened) = clean_head(t);
        if opened {
            seen = 0;
            forwarder = false;
            transparent = true;
            consumed = false;
            builder = false;
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
                // 前面是不認得的指令：後面接的是 awf 的子指令才算；子指令是變數（看不到）而且前面不是建置工具時，保守地也算
                has(SUBCOMMANDS, &sub) || (!builder && sub.contains(['$', '`']))
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
        if has(BUILDERS, &name) {
            builder = true;
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
    // ANSI-C 引號（$'save-run'）與大括號展開（{awf,save-run,approve}）：照 shell 會展開的樣子再看一次
    if let Some(nz) = normalize(seg) {
        calls_raw(&nz, depth, out);
    }
    calls_raw(seg, depth, out);
}

fn calls_raw(seg: &str, depth: usize, out: &mut Vec<Call>) {
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
    // 指令替換與 process substitution：內層各自當一段再看一次；另外把內層換成替身再掃一次原字串，
    // 認得出「路徑中段是指令替換」的執行檔（$(git rev-parse --show-toplevel)/plugin/bin/awf）
    let mut inner_segs: Vec<String> = Vec::new();
    if subst::has_subst(seg) {
        let sc = subst::scan(seg);
        if !sc.inner.is_empty() {
            let mut alt = Vec::new();
            scan_tokens(&shlex_tokens(&sc.flat), &mut alt, 0);
            for c in alt {
                if !found.contains(&c) {
                    found.push(c);
                }
            }
            let mut raw = sc.inner;
            let mut k = 0;
            while k < raw.len() && raw.len() <= subst::MAX_INNER {
                let (cmd, bodies) = subst::split_heredocs(&raw[k]);
                for b in bodies {
                    raw.extend(subst::scan(&b).inner);
                }
                raw[k] = cmd;
                k += 1;
            }
            if sc.over || raw.len() > subst::MAX_INNER {
                found.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_DEEP) });
            } else {
                for cmd in &raw {
                    inner_segs.extend(segments(cmd));
                }
            }
        }
    }
    out.append(&mut found);

    // 段落開頭（去掉環境變數與包裝）是 shell、eval 時的內層指令。兩種去法都看：
    // 所有長得像賦值的 token 都丟（原本的做法），以及只丟指令前面的（`bash -c 'X=$(awf …)'` 的內層指令本身就是「X=…」開頭）
    let old_head: Vec<&String> = toks.iter().filter(|x| !ENV_ASSIGN.is_match(x)).collect();
    let (name, mut inner_cmds) = head_inner(strip_wrappers(old_head));
    let lead: Vec<&String> = toks.iter().skip_while(|x| ENV_ASSIGN.is_match(x)).collect();
    for ic in head_inner(strip_wrappers(lead)).1 {
        if !inner_cmds.contains(&ic) {
            inner_cmds.push(ic);
        }
    }
    // shell 不在開頭的（sudo -u root bash -c …、if …; then bash -c …、xargs -I{} bash -c …、find -exec bash -c …）：
    // 任何位置的 shell 名稱後面有 -c 就把它的指令字串展開。開頭是只把參數當文字的指令（echo、grep…）時不看
    for ic in hidden_shells(&toks) {
        if !inner_cmds.contains(&ic) {
            inner_cmds.push(ic);
        }
    }
    // 會執行參數的指令（trap、watch、git bisect run、awk 的 system()、osascript…）
    let (run_shell, run_text) = exec_args(&toks);
    for a in run_shell {
        let ic = (a, false);
        if !inner_cmds.contains(&ic) {
            inner_cmds.push(ic);
        }
    }
    for t in &run_text {
        textual_calls(t, out);
    }
    let shellish = has(SHELL_NAMES, &name) || has(PS_NESTED_SHELLS, &name) || name == "cmd" || matches!(name.as_str(), "eval" | "invoke-expression" | "iex");
    if !shellish && INTERP.is_match(seg) {
        // python -c、node -e 這類內嵌程式裡提到 awf：看不到它怎麼呼叫
        let inline = toks.windows(2).any(|w| CODE_FLAG.is_match(&w[0]) && INLINE_AWF.is_match(&w[1]));
        if inline {
            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_INLINE) });
        }
    }
    if depth >= 4 {
        if inner_cmds.iter().map(|(ic, _)| ic).chain(inner_segs.iter()).any(|ic| maybe(ic)) {
            out.push(Call { sub: String::new(), args: Vec::new(), forwarded: false, direct: false, note: Some(NOTE_DEEP) });
        }
        return;
    }
    for s in &inner_segs {
        calls_at(s, depth + 1, out);
    }
    for (ic, nested_ps) in inner_cmds {
        let inner = if nested_ps { ps_segments(&ic) } else { segments(&ic) };
        for s in inner {
            calls_at(&s, depth + 1, out);
        }
    }
}

/// 把字串參數交給 shell 執行的指令（`watch 'cmd'`、`su -c 'cmd'`、`ssh host 'cmd'`、`parallel 'cmd' ::: x`）。
const STRING_RUNNERS: &[&str] = &[
    "watch", "su", "ssh", "sshpass", "parallel", "entr", "flock", "script", "setsid", "unbuffer", "chronic", "doas", "runuser", "nohup",
    "xargs", "find", "fd", "tmux", "screen", "expect", "at",
];
const AWK_NAMES: &[&str] = &["awk", "gawk", "mawk", "nawk"];
/// 一段裡最多看幾個「會被執行的參數」。
const MAX_EXEC_ARGS: usize = 16;

/// 會執行參數的指令帶的內容。回（照 shell 語法執行的字串, 別的語言的程式文字）；只收提得到 awf 的。
/// 開頭是只把參數當文字的指令時，只認開頭那個指令自己（`git bisect run …`、`awk '…'`），後面的字不算。
fn exec_args(toks: &[String]) -> (Vec<String>, Vec<String>) {
    let mut shell: Vec<String> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let Some(first_at) = toks.iter().position(|x| !ENV_ASSIGN.is_match(x)) else { return (shell, text) };
    let first_harmless = has(HARMLESS, &cmd_name(clean_head(&toks[first_at]).0));
    for (k, t) in toks.iter().enumerate().skip(first_at) {
        if shell.len() + text.len() >= MAX_EXEC_ARGS || (first_harmless && k > first_at) {
            break;
        }
        let name = cmd_name(clean_head(t).0);
        let rest = &toks[k + 1..];
        let mut sh = |a: &str| {
            if maybe(a) {
                shell.push(a.to_string());
            }
        };
        match name.as_str() {
            // trap '<指令>' <訊號…>
            "trap" => {
                if let Some(a) = rest.iter().find(|x| !x.starts_with('-')) {
                    sh(a);
                }
            }
            // env -S '<指令>'
            "env" => {
                for (j, a) in rest.iter().enumerate() {
                    if a == "-S" || a == "--split-string" {
                        if let Some(v) = rest.get(j + 1) {
                            sh(v);
                        }
                    } else if let Some(v) = a.strip_prefix("--split-string=") {
                        sh(v);
                    } else if let Some(v) = a.strip_prefix("-S").filter(|v| !v.is_empty()) {
                        sh(v);
                    }
                }
            }
            // git bisect run <指令…>、git rebase -x／--exec '<指令>'、git submodule foreach '<指令>'、alias.x='!<指令>'
            "git" => {
                if let Some(b) = rest.iter().position(|x| x == "bisect") {
                    if let Some(r) = rest[b..].iter().position(|x| x == "run") {
                        let cmd = &rest[b + r + 1..];
                        if !cmd.is_empty() {
                            sh(&super::ps::join(cmd));
                        }
                    }
                }
                for (j, a) in rest.iter().enumerate() {
                    if a == "-x" || a == "--exec" {
                        if let Some(v) = rest.get(j + 1) {
                            sh(v);
                        }
                    } else if let Some(v) = a.strip_prefix("--exec=") {
                        sh(v);
                    } else if a == "foreach" {
                        let cmd: Vec<String> = rest[j + 1..].iter().filter(|x| !x.starts_with('-')).cloned().collect();
                        sh(&cmd.join(" "));
                    } else if a.starts_with("alias.") {
                        if let Some(c) = a.split_once('=').and_then(|(_, v)| v.strip_prefix('!')) {
                            sh(c);
                        }
                    }
                }
            }
            // awk 的 system()、"cmd" | getline
            n if has(AWK_NAMES, n) => {
                for a in rest {
                    if (a.contains("system") || a.contains("getline")) && maybe(a) {
                        text.push(a.clone());
                    }
                }
            }
            "osascript" => {
                for a in rest {
                    if a.contains("do shell script") && maybe(a) {
                        text.push(a.clone());
                    }
                }
            }
            n if has(STRING_RUNNERS, n) => {
                for a in rest {
                    if a.chars().any(py::is_space) {
                        sh(a);
                    }
                }
            }
            _ => {}
        }
    }
    (shell, text)
}

/// 別的語言的程式文字裡看得到的 `awf <子指令> <參數…>`（`system("awf save-run approve r")`）：照字面當成一次呼叫。
fn textual_calls(text: &str, out: &mut Vec<Call>) {
    for (_, end) in INLINE_AWF.find_all(text).into_iter().take(8) {
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in text[end..].chars() {
            let wordy = c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '=' | ':' | '$' | '{' | '}' | '*');
            if wordy {
                cur.push(c);
                continue;
            }
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            if matches!(c, ';' | '|' | '&' | ')' | '\n') || words.len() >= 12 {
                break;
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        let Some(sub) = words.first().cloned() else { continue };
        if has(SUBCOMMANDS, &sub) {
            out.push(Call { sub, args: words[1..].to_vec(), forwarded: false, direct: false, note: None });
        }
    }
}

/// shell 選項裡自己帶一個值的（`bash -o pipefail -c …`）。
const SHELL_VALUE_FLAGS: &[&str] = &["-o", "+o", "-O", "+O", "--rcfile", "--init-file"];

/// `bash [選項] -c [選項] <指令字串> [參數…]`：選項裡有 -c 時，第一個不是選項的參數就是指令字串。
fn shell_command_arg(args: &[String]) -> Option<&String> {
    let mut saw_c = false;
    let mut k = 0;
    while k < args.len() {
        let a = args[k].as_str();
        if a == "--" {
            return if saw_c { args.get(k + 1) } else { None };
        }
        if a.len() > 1 && (a.starts_with('-') || a.starts_with('+')) {
            if is_dash_c(a) {
                saw_c = true;
            }
            k += if has(SHELL_VALUE_FLAGS, a) { 2 } else { 1 };
            continue;
        }
        return if saw_c { Some(&args[k]) } else { None };
    }
    None
}

/// 段落裡任何位置的 shell（`bash -c`、`pwsh -Command`、`cmd /c`、`eval`）帶的內層指令。回 [(內層指令, 是不是 PowerShell 語法)]。
/// 開頭是只把參數當文字的指令（echo、grep、git…）時回空的。
pub(crate) fn hidden_shells(toks: &[String]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let first = toks.iter().find(|x| !ENV_ASSIGN.is_match(x)).map(|t| cmd_name(clean_head(t).0)).unwrap_or_default();
    if has(HARMLESS, &first) {
        return out;
    }
    for (k, t) in toks.iter().enumerate() {
        if out.len() >= 8 {
            break;
        }
        let name = cmd_name(clean_head(t).0);
        let rest = &toks[k + 1..];
        if has(SHELL_NAMES, &name) {
            if let Some(ic) = shell_command_arg(rest) {
                out.push((ic.clone(), false));
            }
            if let Some(j) = rest.iter().position(|x| is_dash_c(x)) {
                if let Some(ic) = rest.get(j + 1).filter(|x| x.as_str() != "--") {
                    out.push((ic.clone(), false));
                }
            }
        } else if has(PS_NESTED_SHELLS, &name) || name == "cmd" {
            if let Some(j) = rest.iter().position(|x| matches!(py::lower(x).as_str(), "-c" | "-command" | "/c" | "/k")) {
                if j + 1 < rest.len() {
                    out.push((rest[j + 1..].join(" "), true));
                }
            }
        } else if matches!(name.as_str(), "eval" | "invoke-expression" | "iex") && !rest.is_empty() {
            out.push((rest.join(" "), false));
        }
    }
    out
}

/// 去掉開頭的包裝指令（sudo、env、timeout…）和它們的旗標、數字、環境變數設定。
fn strip_wrappers(mut head: Vec<&String>) -> Vec<&String> {
    while !head.is_empty() && has(CMD_WRAPPERS, &cmd_name(head[0])) {
        head.remove(0);
        while !head.is_empty() && (head[0].starts_with('-') || is_number(head[0]) || ENV_ASSIGN.is_match(head[0])) {
            head.remove(0);
        }
    }
    head
}

/// 開頭是 shell（`bash -c`）、PowerShell／cmd（`-Command`、`/c`）、eval 時的內層指令。回（指令名, [(內層指令, 是不是 PowerShell 語法)]）。
fn head_inner(head: Vec<&String>) -> (String, Vec<(String, bool)>) {
    let name = head.first().map(|h| cmd_name(h)).unwrap_or_default();
    let args: Vec<&String> = head.iter().skip(1).copied().collect();
    let nested_ps = has(PS_NESTED_SHELLS, &name) || name == "cmd";
    let mut inner: Vec<(String, bool)> = Vec::new();
    if has(SHELL_NAMES, &name) {
        if let Some(k) = args.iter().enumerate().position(|(k, t)| is_dash_c(t) && k + 1 < args.len()) {
            inner.push((args[k + 1].clone(), false));
        }
    } else if nested_ps {
        if let Some(k) = args.iter().enumerate().position(|(k, t)| matches!(py::lower(t).as_str(), "-c" | "-command" | "/c" | "/k") && k + 1 < args.len()) {
            inner.push((args[k + 1..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "), true));
        }
    } else if matches!(name.as_str(), "eval" | "invoke-expression" | "iex") {
        inner.push((args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "), false));
    }
    (name, inner)
}

/// 大括號展開的數量上限（超過就不展開，照原字串判）。
const MAX_BRACE_WORDS: usize = 64;

/// 解 `$'…'` 的跳脫（`\n`、`\x2d`、`\055`、`-`…）。
fn ansi_c(body: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let num = |from: usize, max: usize, radix: u32| -> (Option<char>, usize) {
        let mut j = from;
        let mut v: u32 = 0;
        while j < body.len() && j - from < max {
            let Some(d) = body[j].to_digit(radix) else { break };
            v = v * radix + d;
            j += 1;
        }
        (if j > from { char::from_u32(v) } else { None }, j)
    };
    while i < body.len() {
        let c = body[i];
        if c != '\\' || i + 1 >= body.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let e = body[i + 1];
        i += 2;
        let (ch, next) = match e {
            'n' => (Some('\n'), i),
            't' => (Some('\t'), i),
            'r' => (Some('\r'), i),
            'a' => (Some('\x07'), i),
            'b' => (Some('\x08'), i),
            'e' | 'E' => (Some('\x1b'), i),
            'f' => (Some('\x0c'), i),
            'v' => (Some('\x0b'), i),
            'x' => num(i, 2, 16),
            'u' => num(i, 4, 16),
            'U' => num(i, 8, 16),
            '0'..='7' => num(i - 1, 3, 8),
            other => (Some(other), i),
        };
        match ch {
            Some(x) => out.push(x),
            None => {
                out.push('\\');
                out.push(e);
            }
        }
        i = next;
    }
    out
}

/// 展開一個字裡沒加引號的 `{a,b,c}`（巢狀也展開）。`${VAR}` 不是。超過上限回 None。
fn brace_expand(word: &str, depth: usize, budget: &mut usize) -> Option<Vec<String>> {
    let ch: Vec<char> = word.chars().collect();
    let n = ch.len();
    let mut i = 0;
    while i < n {
        match ch[i] {
            '\\' => i += 2,
            '\'' => {
                i += 1;
                while i < n && ch[i] != '\'' {
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                i += 1;
                while i < n && ch[i] != '"' {
                    i += if ch[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            '$' if ch.get(i + 1) == Some(&'{') => {
                while i < n && ch[i] != '}' {
                    i += 1;
                }
                i += 1;
            }
            '{' => {
                // 找配對的 }，記下最外層的逗號
                let mut level = 0usize;
                let mut commas: Vec<usize> = Vec::new();
                let mut j = i + 1;
                let mut close = None;
                while j < n {
                    match ch[j] {
                        '\\' => j += 1,
                        '{' => level += 1,
                        '}' if level == 0 => {
                            close = Some(j);
                            break;
                        }
                        '}' => level -= 1,
                        ',' if level == 0 => commas.push(j),
                        _ => {}
                    }
                    j += 1;
                }
                let Some(close) = close.filter(|_| !commas.is_empty()) else {
                    i += 1;
                    continue;
                };
                if depth >= 4 {
                    return None;
                }
                let prefix: String = ch[..i].iter().collect();
                let suffix: String = ch[close + 1..].iter().collect();
                let mut cuts = vec![i];
                cuts.extend(commas);
                cuts.push(close);
                let mut out = Vec::new();
                for w in cuts.windows(2) {
                    let alt: String = ch[w[0] + 1..w[1]].iter().collect();
                    out.extend(brace_expand(&format!("{prefix}{alt}{suffix}"), depth + 1, budget)?);
                }
                return Some(out);
            }
            _ => i += 1,
        }
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    Some(vec![word.to_string()])
}

/// 一個字（引號照留）：有沒加引號的大括號就展開，接到 `out`。
fn push_word(word: &mut String, out: &mut String, changed: &mut bool, budget: &mut usize) {
    if word.contains('{') && word.contains(',') {
        if let Some(ws) = brace_expand(word, 0, budget) {
            if ws.len() > 1 {
                *changed = true;
            }
            out.push_str(&ws.join(" "));
            word.clear();
            return;
        }
    }
    out.push_str(word);
    word.clear();
}

/// 照 bash 會展開的樣子改寫：`$'…'` 解成一般的引號字串、`$"…"` 去掉 `$`、沒加引號的 `{a,b}` 展開成多個字。
/// 沒有這些寫法（或展開超過上限）回 None。
fn normalize(seg: &str) -> Option<String> {
    let brace = seg.contains('{') && seg.contains(',');
    if !(seg.contains("$'") || seg.contains("$\"") || brace) {
        return None;
    }
    let ch: Vec<char> = seg.chars().collect();
    let n = ch.len();
    let mut out = String::with_capacity(seg.len());
    let mut word = String::new();
    let mut budget = MAX_BRACE_WORDS;
    let mut changed = false;
    let mut i = 0;
    while i < n {
        let c = ch[i];
        if c == '\\' {
            word.push(c);
            if let Some(x) = ch.get(i + 1) {
                word.push(*x);
            }
            i += 2;
            continue;
        }
        if c == '$' && ch.get(i + 1) == Some(&'\'') {
            let mut j = i + 2;
            while j < n && ch[j] != '\'' {
                j += if ch[j] == '\\' { 2 } else { 1 };
            }
            let j = j.min(n);
            word.push_str(&py::shlex_quote(&ansi_c(&ch[i + 2..j])));
            changed = true;
            i = j + 1;
            continue;
        }
        if c == '$' && ch.get(i + 1) == Some(&'"') {
            changed = true;
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            word.push(c);
            i += 1;
            while i < n {
                let x = ch[i];
                word.push(x);
                i += 1;
                if x == '\\' && c == '"' {
                    if let Some(y) = ch.get(i) {
                        word.push(*y);
                        i += 1;
                    }
                    continue;
                }
                if x == c {
                    break;
                }
            }
            continue;
        }
        if py::is_space(c) {
            push_word(&mut word, &mut out, &mut changed, &mut budget);
            out.push(c);
            i += 1;
            continue;
        }
        word.push(c);
        i += 1;
    }
    push_word(&mut word, &mut out, &mut changed, &mut budget);
    if changed { Some(out) } else { None }
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
    /// 唯讀的子指令帶了會寫檔的旗標，或參數裡塞了 git 的選項（內容＝子指令與旗標）。
    WriteFlag(String),
    /// `awf script` 的組裝（會寫 `--out`）。
    Script(String),
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

    /// 參數裡第一個會寫檔的旗標（`--write`、`--out=x`…）；`--` 之後的不看。
    fn write_flag(&self) -> Option<&str> {
        self.args.iter().take_while(|a| a.as_str() != "--").map(|a| a.split('=').next().unwrap_or(a)).find(|n| has(WRITE_FLAGS, n))
    }

    /// 參數裡塞了 git 的選項（見 [`GIT_INJECT`]）；回那個選項的名字。
    fn git_inject(&self) -> Option<&'static str> {
        self.args.iter().find_map(|a| GIT_INJECT.iter().copied().find(|g| a.contains(g)))
    }

    /// 放行的唯讀子指令另外要過的兩關：不能帶會寫檔的旗標、不能把 git 的選項塞進參數。
    fn flag_deny(&self) -> Option<Deny> {
        let sub = self.sub.as_str();
        if has(GIT_PASSING, sub) {
            if let Some(g) = self.git_inject() {
                return Some(Deny::WriteFlag(format!("{sub} …{g}")));
            }
        }
        if matches!(sub, "deliverables" | "checklist") {
            if let Some(f) = self.write_flag() {
                return Some(Deny::WriteFlag(format!("{sub} {} {f}", self.first()).replace("  ", " ")));
            }
        }
        None
    }

    /// `awf script`：team agent 只能跑 `--selftest` 與不寫檔的靜態檢查；`assemble`、任何 `--out` 都擋。
    fn script_deny(&self) -> Option<String> {
        if self.first() == "assemble" {
            return Some("assemble".into());
        }
        self.write_flag().map(|f| format!("{} {f}", self.first()).trim().to_string())
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
        self.sub_deny(steward).or_else(|| self.flag_deny())
    }

    fn sub_deny(&self, steward: bool) -> Option<Deny> {
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
            // 自訂腳本的組裝會寫檔（--out）：只由主 session 經 awf next --script 做
            "script" => self.script_deny().map(Deny::Script),
            // checklist check 的 --write、轉給 git 的參數另外在 flag_deny 看
            "statusline" | "doctor" | "hook" | "checklist" | "recipe" | "sync-branch" | "locate" | "cost" | "check-agents"
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
        // 唯讀子指令帶寫入旗標、組裝腳本、塞 git 選項
        assert_eq!(d("awf script assemble a.js --out b.js"), Some(Deny::Script("assemble".into())));
        assert_eq!(d("awf script check a.js --out=b.js"), Some(Deny::Script("check --out".into())));
        assert_eq!(d("awf script check a.js"), None);
        assert_eq!(d("awf script --selftest"), None);
        assert_eq!(d("awf checklist check x.md --write"), Some(Deny::WriteFlag("checklist check --write".into())));
        assert_eq!(d("awf checklist check x.md --id A"), None);
        assert_eq!(d("awf deliverables status --write"), Some(Deny::WriteFlag("deliverables status --write".into())));
        assert_eq!(d("awf deliverables affected --base main --json"), None);
        assert_eq!(d("awf check-rules --git . --output=/tmp/x"), Some(Deny::WriteFlag("check-rules …--output".into())));
        assert_eq!(d("awf sync-branch plan --remote=--upload-pack=x"), Some(Deny::WriteFlag("sync-branch …--upload-pack".into())));
        assert_eq!(d("awf check-rules --git . main..HEAD"), None);
        assert_eq!(d("awf run-brief --log a.log -- ctest --output-on-failure"), None);
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
