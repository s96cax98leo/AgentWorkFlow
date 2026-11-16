//! guard-shell：Bash 與 PowerShell 工具共用的 shell 守門。原本是 0.x 的 Python 工具 `guard.py` 的 `_check_shell_rules` 與相關函式。
//!
//! 樣式字串照 0.x 的 Python 版逐字搬過來（差異由 `re.rs` 對齊）；判斷順序、訊息文字都不變。

use super::awf_cli::{self, Deny};
use super::guard::{cmd_name, err, fold, is_secret_path, norm, run_git, splitlines, str_or_empty, tool_input, Guard, Stop, AUDIT_FILES, GIT_GLOBAL_OPTS, R, SLUG};
use super::path;
use super::ps::{self, ps_segments, segments, PS_NESTED_SHELLS};
use super::py::{self, shlex_tokens};
use super::re::{group, Re};
use crate::config;
use crate::pyfmt;
use serde_json::Value;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        pub static $name: Re = Re::new($pat, "");
    };
    ($name:ident, $pat:expr, $flags:expr) => {
        pub static $name: Re = Re::new($pat, $flags);
    };
}

re!(GH_WRITE, concat!(
    r"\bgh\s+(?:",
    r"pr\s+(?:create|comment|review|merge|edit|close|reopen|ready|lock|unlock|update-branch)|",
    r"issue\s+(?:create|comment|edit|close|reopen|delete|lock|transfer)|",
    r"release\s+(?:create|edit|delete|upload)|",
    r"repo\s+(?:edit|delete|rename|archive|sync|create|fork)|",
    r"workflow\s+(?:run|enable|disable)|run\s+(?:rerun|cancel|delete)|",
    r"label\s+(?:create|edit|delete)|secret\s+(?:set|delete)|variable\s+(?:set|delete))\b"));
re!(GH_BYPASS, r"\bgh\s+(?:alias\s+(?:set|import)|extension\s+(?:install|upgrade|exec|create)|ext\s+(?:install|upgrade|exec|create))\b");
re!(GH_ACCOUNT_WRITE, concat!(
    r"\bgh\s+(?:alias\s+(?:delete)|extension\s+(?:remove)|ssh-key\s+(?:add|delete)|gpg-key\s+(?:add|delete)|",
    r"gist\s+(?:create|new|edit|delete|rename)|auth\s+(?:login|logout|refresh|setup-git|switch))\b"));
re!(GH_INNER_REPO, r#"(\bgh(?:\.exe)?(?:\s+[a-z][\w-]*)?)\s+((?:-R|--repo)(?:\s+|=)(?:"[^"]*"|'[^']*'|[^\s;&|]+))(?=\s+[a-z])"#, "i");
re!(GH_API_WRITE, r#"\bgh(?:\.exe)?\s+api\b(?=.*(?:-X\s*|--method[\s=]+)["']?(?:POST|PATCH|PUT|DELETE)|.*\s(?:-f|-F|--field|--raw-field|--input)[\s=])"#, "i");
re!(DESTRUCTIVE, r"\b(rm|mv|cp|sed\s+-i|git\s+(commit|checkout|reset|merge|pull|stash|clean)|tee|>)\b");
re!(GH_PR_MERGE, r"\bgh(?:\.exe)?\s+pr\s+merge\b", "i");
re!(GH_PR_REVIEW, r"\bgh(?:\.exe)?\s+pr\s+review\b", "i");
re!(API_CLIENT, r"(?:^|[\s/\\(&;|`])(?:gh(?:\.exe)?\s+api|curl(?:\.exe)?|wget(?:\.exe)?|https?|xh|invoke-restmethod|irm|invoke-webrequest|iwr)(?=\s|$)", "i");
re!(MERGE_TARGET, r#"/pulls/[^/\s"']+/merge(?![\w-])|\brepos/[^/\s]+/[^/\s]+/merges\b|\bmergePullRequest\b|\benablePullRequestAutoMerge\b|\bmergeBranch\b"#, "i");
re!(REVIEW_TARGET, r#"/pulls/[^/\s"']+/reviews\b|\baddPullRequestReview\b|\bsubmitPullRequestReview\b"#, "i");
re!(APPROVE_WORD, r"\bAPPROVE\b", "i");
re!(APPROVE_FLAG, r"(?:^|\s)(?:--approve(?:=\S*)?|-[A-Za-z]*a[A-Za-z]*)(?=\s|$)");
re!(APPROVE_INPUT, r"(?:^|\s)--input[\s=]|=@\S|\s@\S");
re!(GITHUB_API_HOST, r"\bapi\.github\.com\b|/api/v3/|/api/graphql\b", "i");
re!(WEB_CLIENT, r"(?:^|[\s/\\])(curl|wget|https?|xh|invoke-restmethod|irm|invoke-webrequest|iwr)(?:\.exe)?(?=\s|$)", "i");
re!(CURL_WRITE, r#"(?:^|\s)(?:-X\s*["']?(?:POST|PUT|PATCH|DELETE)|--request[\s=]+["']?(?:POST|PUT|PATCH|DELETE)|-d|--data(?:-\w+)?|--json|-F|--form(?:-string)?|-T|--upload-file|--post-data|--post-file|--method[\s=]+["']?(?:POST|PUT|PATCH|DELETE))(?=[\s=]|$)"#);
re!(PS_WEB_WRITE, r#"(?:^|\s)(?:-Method\s+["']?(?:Post|Put|Patch|Delete)|-Body|-InFile|-Form)(?=[\s:]|$)"#, "i");
re!(HTTPIE_WRITE, r"(?:^|\s)(?:POST|PUT|PATCH|DELETE)\s|\s[\w.-]+:?=\S");
re!(INTERP, r"(?:^|[\s/\\(;&|`])(?:python[\d.]*|py|pypy[\d.]*|node|nodejs|deno|bun|ruby|perl|php|pwsh|powershell)(?:\.exe)?(?=\s|$)", "i");
re!(SHA40, r"^[0-9a-f]{40}$");
re!(PR_URL, r"^https?://[^/]+/([\w.-]+/[\w.-]+)/pull/(\d+)(?:[/?#].*)?$");
re!(SUBST, r"\$\(|`|<\(|\$\{");
re!(SHELL_WRITE, concat!(
    r"(?<![&<>=-])>(?!&)|\b(?:rm|mv|cp|tee|truncate|dd|install|ln|touch|chmod|sed\s+-i|perl\s+-p?i)\b",
    r#"|\bgit\s+(?:checkout|restore|rm|mv|stash|reset)\b|open\([^)]*["'][wax+]|write_text|write_bytes|writelines|\.write\(|unlink|os\.remove|shutil\."#,
    r"|set-content|add-content|out-file|new-item|remove-item|clear-content|copy-item|move-item|rename-item"), "i");
re!(STATE_FRAGMENT, r"audit|freeze|\brun\W{0,16}\.json\b|guard\W{0,16}\.?config|methodology\W{0,16}\.json", "i");
re!(WRITE_API, r"\bopen\s*\(|appendFile|writeFile|createWriteStream|\.write\b|write_text|write_bytes|>>?|\bFile\.(?:write|open)|out-file|set-content|add-content", "i");
re!(CONTEXT_FRAGMENT, r"(?:^|[^\w-])(?:context|pack)\W{0,16}\.md\b|\b(?:decisions|links)\W{0,16}\.jsonl\b|決策記憶\W{0,16}\.md|結案摘要\W{0,16}\.md|[/\\]archive[/\\]", "i");
re!(RUN_INDEX_FRAGMENT, r"執行紀錄索引");
re!(KG_TARGET, r"(?:^|[^\w.-])graph\.db(?![\w])|(?:^|[/\\\s'\x22=])vault[/\\](?:notes|能力|需求|決策|模組|hot\.md|index\.md)", "i");

/// shell 寫知識圖的檔（graph.db、vault 的頁與 notes/；盡力而為，看不到實際路徑時只比對寫法）。
fn shell_writes_kg(seg: &str) -> bool {
    let probe = NULL_REDIR.replace_all(&norm(seg), " ");
    KG_TARGET.is_match(&probe) && (SHELL_WRITE.is_match(&probe) || (INTERP.is_match(&probe) && INTERP_WRITE.is_match(&probe)))
}
re!(NULL_REDIR, r"\d?>>?\s*/dev/null|\d?>>?\s*\$null|\d>&\d", "i");
re!(HARMLESS, r"[\d&]?>>?\s*/dev/null\b|[\d&]?>>?\s*\$null\b|\d?>&\d", "i");
re!(QUOTED, r#"'[^']*'|"(?:\\.|[^"\\])*""#);
re!(QUOTE_CHARS, r#"["']"#);
re!(RECORD_REDIRECT, r"(?<![<>=-])>");
re!(RECORD_WRITE_CMD, concat!(
    r"(?:^|[\s/\\(`])(?:cp|mv|install|rsync|scp|ln|link|touch|truncate|rm|rmdir|unlink|shred|chmod|chown|chgrp|mkfifo|mknod|patch|tee|",
    r"unzip|tee-object|set-content|add-content|out-file|new-item|copy-item|move-item|rename-item|remove-item|clear-content|",
    r"export-csv|export-clixml|set-itemproperty|expand-archive|compress-archive)(?:\.exe)?(?=\s|$)",
    r"|\bdd\b.*\bof=",
    r"|\b(?:sed|perl|ruby)\b.*\s(?:-[a-zA-Z]*i\S*|--in-place\S*)(?=\s|$)",
    r"|\btar\b\s+(?:\S+\s+)*?(?:-?[a-zA-Z]*x[a-zA-Z]*|--extract|--get)(?=\s|$)",
    r"|\bcurl\b.*\s(?:-[a-zA-Z]*[oO]\S*|--output(?:-dir)?|--remote-name\S*)(?=[\s=]|$)",
    r"|\bwget\b(?!.*(?:\s-[a-zA-Z]*O\s*-(?:\s|$)|--output-document[=\s]+-(?:\s|$)))",
    r"|\bfind\b.*\s-(?:delete|fprint\w*|fls)(?=\s|$)",
    r"|(?:^|\s)-outfile(?=[\s:]|$)|\[(?:system\.)?io\.file\]::",
    r"|\bgit\s+(?:checkout|switch|restore|reset|stash|clean|rm|mv|commit|merge|pull|rebase|cherry-pick|revert|am|clone|init|",
    r"checkout-index|read-tree|update-index|worktree\s+(?:add|remove|move)|apply(?!.*\s--(?:check|stat|numstat|summary)\b))\b"), "i");
re!(INTERP_WRITE, concat!(
    r#"\bopen\s*\([^)]*,\s*(?:mode\s*=\s*)?["'](?:[rbt]*[wax+]|\s*\+?>)"#,
    r"|write_text|write_bytes|writelines|\.write\s*\(|writeFile|appendFile|createWriteStream|copyFile|\brename(?:Sync)?\s*\(",
    r"|\bos\.(?:remove|unlink|rename|replace|rmdir|removedirs|makedirs|mkdir|truncate|symlink|link|chmod|system|popen|open)\b",
    r"|\bshutil\.|\.unlink\s*\(|\.touch\s*\(|\.mkdir\s*\(|\.rename\s*\(|\.symlink_to\s*\(",
    r"|\w+Sync\s*\(|\bFile\.(?:write|open|new)|\bIO::File|\bFileUtils\.|\bsubprocess\b|child_process|\bsystem\s*\(|\bexec[lv]p?e?\s*\(|\bpty\."), "i");
re!(ENV_ASSIGN, r"^[A-Za-z_]\w*=");
re!(FREEZE_DESTROY, r"\b(?:rm|mv|unlink|truncate|remove-item|move-item|clear-content)\b|os\.remove|shutil\.", "i");
re!(FREEZE_TRUE, r#"frozen["'\\]*\s*:\s*true"#, "i");
re!(FREEZE_FALSE, r"\bfalse\b|\bnull\b|\bnone\b", "i");
re!(TEAM_CONFIG_TARGET, concat!(
    r#"(?:^|[/\\\s"'=])\.claude[/\\](?:teams|workflows)(?:[/\\]|\s|$)|(?:^|[/\\\s"'=])\.wf(?:[/\\]|\s|$)"#,
    r"|methodology\.json|guard\.config\.json"), "i");
re!(TEAM_TOOL_CALL, r#"(?:python[\d.]*|py|pypy[\d.]*)(?:\.exe)?(?:\s+-\S+)*\s+["']?[^\s"']*\.claude[/\\]teams[/\\]tools[/\\][\w.-]+\.py["']?"#, "i");
re!(TEAM_CONFIG_MKDIR, r"(?:^|[\s;&|(`])(?:mkdir|md|rmdir)(?:\.exe)?(?=\s|$)", "i");
re!(MULTI_SLASH, r"(?<![:/])[/\\\\]{2,}");
re!(TEAMS_REL, r#"(?:^|[\s"'=])\.?[/\\\\]?(?:teams|workflows)(?:[/\\\\]|\s|$)"#);
re!(PUSH_RE, r"\bgit\b.*\b(?:push|send-pack)\b");
re!(GIT_WORD, r"\bgit\b");
re!(NO_VERIFY, r"(?:^|\s)--no-verify\b");
re!(MERGE_GATE_PY, r"(?:^|[\s/\\])merge_gate\.py\b");
re!(PUBLISH_PY, r"(?:^|[\s/\\])publish\.py\b");
re!(CHECK_WORD, r"(?:^|\s)check(?:\s|$)");
re!(HEREDOC, r"<<|(?:^|\s)-(?:\s|$)");
re!(SAVE_RUN_LAUNCHERS, r"^(?:python[\d.]*|py|pypy[\d.]*|uv|run|pipx|poetry|pdm|rye|hatch)\z");
re!(REPO_FLAG_ANY, r"(?:-R|--repo)[\s=]+\S+");
re!(REFS_HEADS, r"^refs/heads/");

/// 這個模組所有固定的樣式（測試用：確認都編得過）。
pub fn all_patterns() -> Vec<&'static Re> {
    vec![&GH_WRITE, &GH_BYPASS, &GH_ACCOUNT_WRITE, &GH_INNER_REPO, &GH_API_WRITE, &DESTRUCTIVE, &GH_PR_MERGE, &GH_PR_REVIEW, &API_CLIENT,
         &MERGE_TARGET, &REVIEW_TARGET, &APPROVE_WORD, &APPROVE_FLAG, &APPROVE_INPUT, &GITHUB_API_HOST, &WEB_CLIENT, &CURL_WRITE, &PS_WEB_WRITE,
         &HTTPIE_WRITE, &INTERP, &SHA40, &PR_URL, &SUBST, &SHELL_WRITE, &STATE_FRAGMENT, &WRITE_API, &CONTEXT_FRAGMENT, &RUN_INDEX_FRAGMENT, &KG_TARGET,
         &NULL_REDIR, &HARMLESS, &QUOTED, &QUOTE_CHARS, &RECORD_REDIRECT, &RECORD_WRITE_CMD, &INTERP_WRITE, &ENV_ASSIGN, &FREEZE_DESTROY,
         &FREEZE_TRUE, &FREEZE_FALSE, &TEAM_CONFIG_TARGET, &TEAM_TOOL_CALL, &TEAM_CONFIG_MKDIR, &MULTI_SLASH, &TEAMS_REL, &PUSH_RE, &GIT_WORD,
         &NO_VERIFY, &MERGE_GATE_PY, &PUBLISH_PY, &CHECK_WORD, &HEREDOC, &SAVE_RUN_LAUNCHERS, &REPO_FLAG_ANY, &REFS_HEADS, &GIT_GLOBAL_OPTS, &SLUG]
}

const VALUE_FLAGS: &[&str] = &["--repo", "-R", "--match-head-commit", "-b", "--body", "-F", "--body-file", "-t", "--subject", "--author-email"];
const SHELL_NAMES: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "fish", "busybox"];
const CMD_WRAPPERS: &[&str] = &["sudo", "command", "env", "nohup", "time", "nice", "timeout", "exec", "builtin", "stdbuf"];
const SECRET_READERS: &[&str] = &[
    "cat", "tac", "type", "get-content", "gc", "less", "more", "head", "tail", "bat", "nl", "strings", "xxd", "od", "hexdump",
    "base64", "sed", "awk", "gawk", "cut", "sort", "uniq", "jq", "yq", "openssl", "format-hex", "fhx", "vi", "vim", "nano", "view",
    "grep", "egrep", "fgrep", "rg", "ag", "select-string", "sls", "findstr",
];
const GREP_LIKE: &[&str] = &["grep", "egrep", "fgrep", "rg", "ag", "select-string", "sls", "findstr"];
const CD_CMDS: &[&str] = &["cd", "pushd", "chdir", "set-location", "sl", "push-location"];
const EXEC_FORWARDERS: &[&str] = &["xargs", "parallel", "watch", "find", "fd", "entr", "flock", "chronic", "unbuffer", "script", "su", "doas", "runuser", "setsid"];

fn has(list: &[&str], s: &str) -> bool {
    list.contains(&s)
}

fn basename_lower(t: &str) -> String {
    py::lower(t.rsplit(['\\', '/']).next().unwrap_or(""))
}

/// gh --repo X pr merge 3、gh pr -R X merge 3 → gh pr merge 3 --repo X（其餘不動）。
fn canon_gh(seg: &str) -> String {
    let mut seg = seg.to_string();
    for _ in 0..4 {
        let Some(caps) = GH_INNER_REPO.captures(&seg) else { return seg };
        let (Some((ms, me)), Some(g1), Some(g2)) = (caps.first().copied().flatten(), group(&seg, &caps, 1), group(&seg, &caps, 2)) else { return seg };
        seg = format!("{}{}{} {}", &seg[..ms], g1, &seg[me..], g2);
    }
    seg
}

fn is_interp_review(text: &str) -> bool {
    INTERP.is_match(text) && REVIEW_TARGET.is_match(text)
}

/// 合併 PR：gh pr merge（含 --auto），或用 API 打 merge 端點。
fn is_merge_cmd(text: &str) -> bool {
    if GH_PR_MERGE.is_match(text) {
        return true;
    }
    (API_CLIENT.is_match(text) || INTERP.is_match(text)) && MERGE_TARGET.is_match(text)
}

/// 核准 PR：gh pr review --approve／-a，或用 API 送出 APPROVE 審查；看不到內容一律當成核准。
fn is_approve_cmd(seg: &str) -> bool {
    if let Some((_, e)) = GH_PR_REVIEW.find(seg) {
        if APPROVE_FLAG.is_match(&seg[e..]) {
            return true;
        }
    }
    if API_CLIENT.is_match(seg) && REVIEW_TARGET.is_match(seg) {
        return APPROVE_WORD.is_match(seg) || APPROVE_INPUT.is_match(seg);
    }
    false
}

/// 不經 gh 直接打 GitHub API 的寫入。
fn is_rest_write(seg: &str) -> bool {
    if !GITHUB_API_HOST.is_match(seg) {
        return false;
    }
    let Some(caps) = WEB_CLIENT.captures(seg) else { return false };
    let (Some((_, me)), Some(name)) = (caps.first().copied().flatten(), group(seg, &caps, 1)) else { return false };
    let name = py::lower(name);
    let rest = &seg[me..];
    if matches!(name.as_str(), "invoke-restmethod" | "irm" | "invoke-webrequest" | "iwr") {
        return PS_WEB_WRITE.is_match(rest);
    }
    if matches!(name.as_str(), "http" | "https" | "xh") {
        return HTTPIE_WRITE.is_match(rest);
    }
    CURL_WRITE.is_match(rest) || PS_WEB_WRITE.is_match(rest)
}

/// 短旗標 `-[A-Za-z]*<ch>(.*)$`：回傳旗標後面黏著的值。
fn short_flag_tail(t: &str, ch: u8) -> Option<&str> {
    let body = t.strip_prefix('-')?;
    let b = body.as_bytes();
    let run = b.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let upper = if ch.is_ascii_alphabetic() { run } else { (run + 1).min(b.len()) };
    let i = (0..upper).rev().find(|i| b[*i] == ch)?;
    let rest = &body[i + 1..];
    let core = rest.strip_suffix('\n').unwrap_or(rest);
    if core.contains('\n') { None } else { Some(core) }
}

/// 旗標每次出現的值（依出現順序）：--repo x、--repo=x、-R x、-Rx、-R=x，以及短旗標合寫（-sR x）。
fn flag_values(toks: &[String], names: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let mut step = 1;
        for n in names {
            if t == n {
                out.push(toks.get(i + 1).cloned().unwrap_or_default());
                step = 2;
                break;
            }
            if n.starts_with("--") {
                if let Some(v) = t.strip_prefix(&format!("{n}=")) {
                    out.push(v.to_string());
                    break;
                }
            }
            if n.len() == 2 && n.starts_with('-') && !t.starts_with("--") {
                if let Some(v) = short_flag_tail(t, n.as_bytes()[1]) {
                    if !v.is_empty() {
                        out.push(v.strip_prefix('=').unwrap_or(v).to_string());
                    } else {
                        out.push(toks.get(i + 1).cloned().unwrap_or_default());
                        step = 2;
                    }
                    break;
                }
            }
        }
        i += step;
    }
    out
}

fn flag(toks: &[String], names: &[&str]) -> Option<String> {
    flag_values(toks, names).pop()
}

fn flag_dup(toks: &[String], names: &[&str]) -> bool {
    flag_values(toks, names).len() > 1
}

fn tokens(seg: &str) -> Vec<String> {
    shlex_tokens(&GIT_GLOBAL_OPTS.replace_all(seg, "git "))
}

struct GhPr {
    repo: String,
    pr: Option<i128>,
    args: Vec<String>,
}

/// 解析 gh pr <sub> <編號或 URL> --repo <owner/repo> …；不是這個子指令回 None。
fn gh_pr(seg: &str, sub: &str) -> Option<GhPr> {
    let toks = tokens(seg);
    let names: Vec<String> = toks.iter().map(|t| cmd_name(t)).collect();
    for i in 0..toks.len().saturating_sub(2) {
        if names[i] == "gh" && toks[i + 1] == "pr" && toks[i + 2] == sub {
            let args: Vec<String> = toks[i + 3..].to_vec();
            let mut repo = flag(&args, &["--repo", "-R"]).unwrap_or_default();
            let mut pr = None;
            let first_pos = args.iter().enumerate()
                .find(|(k, a)| !a.starts_with('-') && (*k == 0 || !has(VALUE_FLAGS, &args[k - 1])))
                .map(|(_, a)| a.clone());
            if let Some(p0) = first_pos {
                if let Some(caps) = PR_URL.captures(&p0) {
                    let url_repo = group(&p0, &caps, 1).unwrap_or("").to_string();
                    if !repo.is_empty() && repo != url_repo {
                        return Some(GhPr { repo, pr: None, args });
                    }
                    repo = url_repo;
                    pr = group(&p0, &caps, 2).filter(|d| d.bytes().all(|c| c.is_ascii_digit())).and_then(|d| d.parse::<i128>().ok());
                } else if !p0.is_empty() && p0.bytes().all(|c| c.is_ascii_digit()) {
                    pr = p0.parse::<i128>().ok();
                }
            }
            return Some(GhPr { repo, pr, args });
        }
    }
    None
}

fn method_flags(m: &str) -> Option<[&'static str; 2]> {
    match m {
        "squash" => Some(["--squash", "-s"]),
        "merge" => Some(["--merge", "-m"]),
        "rebase" => Some(["--rebase", "-r"]),
        _ => None,
    }
}

fn dup_flag_problem(args: &[String], groups: &[&[&str]]) -> Option<String> {
    for names in groups {
        if flag_dup(args, names) {
            return Some(format!("{} 只能寫一次（重複出現時 gh 取最後一個值，無法確認實際目標）", names.join("／")));
        }
    }
    None
}

fn freezes_only(text: &str) -> bool {
    if FREEZE_DESTROY.is_match(text) {
        return false;
    }
    FREEZE_TRUE.is_match(text) && !FREEZE_FALSE.is_match(text)
}

fn strip_harmless(seg: &str) -> String {
    HARMLESS.replace_all(seg, " ")
}

fn is_dash_c(t: &str) -> bool {
    // re.fullmatch(r"-[a-zA-Z]*c[a-zA-Z]*", t)
    match t.strip_prefix('-') {
        Some(b) => b.bytes().all(|c| c.is_ascii_alphabetic()) && b.contains('c'),
        None => false,
    }
}

/// 機敏檔讀取：段落是否讀出機敏檔內容。grep 類帶 -q／--quiet／-Quiet 只判有無，放行。
fn reads_secret(seg: &str) -> Option<String> {
    let toks = shlex_tokens(seg);
    let mut i = 0;
    while i < toks.len() && (ENV_ASSIGN.is_match(&toks[i]) || matches!(cmd_name(&toks[i]).as_str(), "sudo" | "command" | "env" | "nohup" | "time" | "xargs")) {
        i += 1;
    }
    if i >= toks.len() {
        return None;
    }
    let name = cmd_name(&toks[i]);
    let args = &toks[i + 1..];
    if !has(SECRET_READERS, &name) {
        return None;
    }
    let mut hits: Vec<String> = args.iter().filter(|a| !a.starts_with('-') && is_secret_path(a)).cloned().collect();
    for a in args {
        if a.starts_with('-') {
            if let Some((_, v)) = a.split_once('=') {
                if is_secret_path(v) {
                    hits.push(v.to_string());
                }
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    if has(GREP_LIKE, &name) {
        let quiet = args.iter().any(|a| {
            let low = py::lower(a);
            if matches!(low.as_str(), "--quiet" | "--silent" | "-quiet" | "/q") {
                return true;
            }
            let core = a.strip_suffix('\n').unwrap_or(a);
            match core.strip_prefix('-') {
                Some(b) => b.bytes().all(|c| c.is_ascii_alphabetic()) && b.contains('q'),
                None => false,
            }
        });
        if quiet {
            return None;
        }
    }
    hits.into_iter().next()
}

/// 推送的參數；不是推送回 None。
fn push_args(seg: &str) -> Option<Vec<String>> {
    let toks = tokens(seg);
    for verb in ["push", "send-pack"] {
        if let Some(i) = toks.iter().position(|t| t == verb) {
            return Some(toks[i + 1..].to_vec());
        }
    }
    None
}

fn is_force_push(seg: &str) -> bool {
    let Some(args) = push_args(seg) else { return false };
    args.iter().any(|a| {
        if a == "-f" || a == "--force" || a.starts_with("--force-with-lease") || a.starts_with("--force-if-includes") {
            return true;
        }
        let core = a.strip_suffix('\n').unwrap_or(a);
        let short = match core.strip_prefix('-') {
            Some(b) => b.bytes().all(|c| c.is_ascii_alphabetic()) && b.contains('f'),
            None => false,
        };
        short || (a.starts_with('+') && a.chars().count() > 1)
    })
}

/// 這個指令要建立的分支名（checkout -b、switch -c、branch <名>、worktree add -b）。
fn new_branches(seg: &str) -> Vec<String> {
    let toks = tokens(seg);
    let Some(gi) = toks.iter().position(|t| t == "git") else { return Vec::new() };
    let t = &toks[gi + 1..];
    let Some(first) = t.first() else { return Vec::new() };
    let mut out = Vec::new();
    match first.as_str() {
        "checkout" | "switch" | "worktree" => {
            let flags: &[&str] = match first.as_str() {
                "checkout" | "worktree" => &["-b", "-B"],
                _ => &["-c", "-C", "--create", "--force-create"],
            };
            if first == "worktree" && (t.len() < 2 || t[1] != "add") {
                return Vec::new();
            }
            for (i, a) in t.iter().enumerate() {
                if has(flags, a) && i + 1 < t.len() {
                    out.push(t[i + 1].clone());
                }
            }
        }
        "branch" => {
            let rest = &t[1..];
            const NOT_CREATE: &[&str] = &["-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy", "--list", "-l", "-a", "-r", "--show-current",
                "--contains", "--merged", "--no-merged", "-u", "--set-upstream-to", "--unset-upstream", "--edit-description", "-v", "-vv"];
            if rest.iter().any(|a| has(NOT_CREATE, a) || a.starts_with("--format")) {
                return Vec::new();
            }
            if let Some(n) = rest.iter().find(|a| !a.starts_with('-')) {
                out.push(n.clone());
            }
        }
        _ => {}
    }
    out
}

/// team agent 執行 <tool>.py 的寫入子指令時回傳子指令名；唯讀的子指令、沒有參數回 None。
fn save_run_write(seg: &str, tool: &str, readonly: &[&str], depth: usize) -> Option<String> {
    let toks = shlex_tokens(seg);
    if depth < 4 && !toks.is_empty() {
        let mut head: Vec<&String> = toks.iter().filter(|x| !ENV_ASSIGN.is_match(x)).collect();
        while !head.is_empty() && has(CMD_WRAPPERS, &cmd_name(head[0])) {
            head.remove(0);
            while !head.is_empty() && head[0].starts_with('-') {
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
        }
        for ic in inner_cmds {
            let inner = if nested_ps { ps_segments(&ic) } else { segments(&ic) };
            for s in inner {
                if let Some(got) = save_run_write(&s, tool, readonly, depth + 1) {
                    return Some(got);
                }
            }
        }
    }
    let target = format!("{tool}.py");
    for (k, t) in toks.iter().enumerate() {
        if basename_lower(t) != target {
            continue;
        }
        let before: Vec<String> = toks[..k].iter().filter(|x| !ENV_ASSIGN.is_match(x)).map(|x| cmd_name(x))
            .filter(|b| !has(CMD_WRAPPERS, b) && !b.starts_with('-')).collect();
        let sub = toks.get(k + 1).cloned().unwrap_or_default();
        if before.iter().any(|b| has(EXEC_FORWARDERS, b)) {
            return if has(readonly, &sub) { None } else if sub.is_empty() { Some("（經轉呼叫程式執行，看不到子指令）".into()) } else { Some(sub) };
        }
        if !before.is_empty() && !before.iter().all(|b| SAVE_RUN_LAUNCHERS.is_match(b)) {
            continue; // cat／grep／less save_run.py 之類，不是執行
        }
        return if has(readonly, &sub) || sub.is_empty() { None } else { Some(sub) };
    }
    if INTERP.is_match(seg) {
        let m = regex::escape(tool);
        let pat = format!(r#"\bimport\s+{m}\b|\bfrom\s+{m}\b|import_module\b.*{m}|\brunpy\b|-m\s+{m}\b|__import__\s*\(\s*["']{m}"#);
        if Re::dynamic(pat, "").is_match(seg) {
            return Some("（內嵌程式呼叫）".into());
        }
    }
    None
}

impl Guard {
    /// `_resolve(base, p)`：去引號、正規化、展開 ~，相對路徑接在 base 後面再解析。
    fn resolve_arg(&self, base: &str, p: &str) -> R<String> {
        let mut p = norm(p.trim_matches(|c| c == '"' || c == '\''));
        if p.starts_with('~') {
            p = path::expanduser(&p);
        }
        let q = path::pp(&p);
        let full = if path::is_abs(&q) { q } else { path::join(base, &q) };
        Ok(path::resolve(&full)?)
    }

    /// 每一段實際作用的目錄：依序套用 cd／pushd／Set-Location，git 段落再套 -C、--git-dir、--work-tree。
    fn seg_dirs(&self, segs: &[String], cwd: &str) -> R<Vec<String>> {
        let mut out = Vec::new();
        let mut d = cwd.to_string();
        for seg in segs {
            let mut toks = shlex_tokens(seg);
            while !toks.is_empty() && ENV_ASSIGN.is_match(&toks[0]) {
                toks.remove(0);
            }
            let names: Vec<String> = toks.iter().map(|t| cmd_name(t)).collect();
            if names.first().is_some_and(|n| has(CD_CMDS, n)) {
                let arg = toks[1..].iter().find(|t| !t.starts_with('-') || *t == "-");
                if let Some(a) = arg {
                    if a != "-" {
                        d = self.resolve_arg(&d, a)?;
                    }
                }
                out.push(d.clone());
                continue;
            }
            let mut g = d.clone();
            if let Some(gi) = names.iter().position(|n| n == "git") {
                let mut k = gi + 1;
                while k < toks.len() && toks[k].starts_with('-') {
                    let t = toks[k].clone();
                    let mut val: Option<String> = None;
                    let mut step = 1;
                    if matches!(t.as_str(), "-C" | "--git-dir" | "--work-tree") && k + 1 < toks.len() {
                        val = Some(toks[k + 1].clone());
                        step = 2;
                    } else if t.starts_with("--git-dir=") || t.starts_with("--work-tree=") {
                        val = t.split_once('=').map(|(_, v)| v.to_string());
                    } else if matches!(t.as_str(), "-c" | "--namespace" | "--exec-path" | "--config-env") {
                        step = 2;
                    }
                    if let Some(v) = val {
                        let v = self.resolve_arg(&g, &v)?;
                        g = if t.starts_with("--git-dir") && path::name(&v) == ".git" { path::parent(&v) } else { v };
                    }
                    k += step;
                }
            }
            out.push(g);
        }
        Ok(out)
    }

    fn git_out(&self, d: &str, args: &[&str]) -> String {
        let mut full = vec!["-C", d];
        full.extend_from_slice(args);
        match run_git(&full) {
            Some((0, out)) => py::strip(&out).to_string(),
            _ => String::new(),
        }
    }

    /// git push 會更新的遠端分支名。--all／--mirror／--branches 回傳 ["*"]。
    fn push_target_branches(&self, seg: &str, d: &str) -> Vec<String> {
        let Some(args) = push_args(seg) else { return Vec::new() };
        if args.iter().any(|a| matches!(a.as_str(), "--all" | "--mirror" | "--branches")) {
            return vec!["*".into()];
        }
        let mut pos: Vec<&String> = Vec::new();
        let mut k = 0;
        while k < args.len() {
            let a = &args[k];
            if matches!(a.as_str(), "--repo" | "-o" | "--push-option" | "--receive-pack" | "--exec" | "--remote") {
                k += 2;
                continue;
            }
            if !a.starts_with('-') {
                pos.push(a);
            }
            k += 1;
        }
        let refspecs: Vec<&String> = pos.iter().skip(1).copied().collect();
        if refspecs.is_empty() {
            let up = self.git_out(d, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}"]);
            let cur = match up.split_once('/') {
                Some((_, b)) => b.to_string(),
                None => self.git_out(d, &["rev-parse", "--abbrev-ref", "HEAD"]),
            };
            return if !cur.is_empty() && cur != "HEAD" { vec![cur] } else { Vec::new() };
        }
        let mut out = Vec::new();
        for r in refspecs {
            let r = r.trim_start_matches('+');
            let mut dst = match r.split_once(':') {
                Some((_, b)) => b.to_string(),
                None => r.to_string(),
            };
            if dst == "HEAD" || dst == "@" {
                dst = self.git_out(d, &["rev-parse", "--abbrev-ref", "HEAD"]);
            }
            let dst = REFS_HEADS.replace_all(&dst, "");
            if !dst.is_empty() {
                out.push(dst);
            }
        }
        out
    }

    fn pushes_protected(&self, seg: &str, d: &str) -> Option<String> {
        let mut protected: Vec<String> = self.cfg.protected_branches.clone();
        protected.sort();
        protected.dedup();
        for b in self.push_target_branches(seg, d) {
            if b == "*" && !protected.is_empty() {
                return Some(protected.join("、"));
            }
            if protected.contains(&b) {
                return Some(b);
            }
        }
        None
    }

    fn push_target_urls(&self, seg: &str, d: &str) -> Vec<String> {
        let Some(args) = push_args(seg) else { return Vec::new() };
        let remote = match args.iter().find(|a| !a.starts_with('-')) {
            Some(r) => r.clone(),
            None => match run_git(&["-C", d, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}"]) {
                Some((0, out)) => py::strip(&out).split('/').next().unwrap_or("").to_string(),
                _ => String::new(),
            },
        };
        if remote.is_empty() {
            return vec![String::new()];
        }
        if remote.contains("://") || remote.contains('@') {
            return vec![remote];
        }
        match run_git(&["-C", d, "remote", "get-url", "--all", "--push", &remote]) {
            Some((0, out)) => py::split_ws(&out),
            _ => vec![remote],
        }
    }

    /// 凍結旗標。檔案存在且 frozen 不是 false 就算凍結；讀不到或不是 JSON 也算凍結。回傳理由，沒凍結回 None。
    fn frozen(&self) -> Option<String> {
        let f = self.freeze_path();
        if !path::exists(&f) {
            return None;
        }
        let data = match config::read_text(&f).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(d) => d,
            None => return Some(format!("{} 存在但讀不懂（視為凍結）", self.am.freeze_file)),
        };
        if data.is_object() && matches!(data.get("frozen"), Some(Value::Bool(false))) {
            return None;
        }
        let reason = match data.get("reason") {
            Some(v) if data.is_object() && pyfmt::truthy(v) => pyfmt::str_of(v),
            _ => "frozen".to_string(),
        };
        Some(format!("{}：{}", self.am.freeze_file, reason))
    }

    /// 上游 repo 的授權人；沒有、空白或已過期回 None。
    fn upstream_auth(&self, repo: &str) -> Option<String> {
        let mut v = self.am.upstream_authorized_by.get(repo).cloned();
        if let Some(Value::Object(o)) = &v {
            let exp = match o.get("expires") {
                Some(x) if pyfmt::truthy(x) => py::strip(&pyfmt::str_of(x)).to_string(),
                _ => String::new(),
            };
            if !exp.is_empty() {
                let day = py::parse_iso_date(py::head(&exp, 10))?;
                let today = py::local_today()?;
                if day < today {
                    return None;
                }
            }
            v = o.get("by").cloned();
        }
        let s = match &v {
            Some(x) if pyfmt::truthy(x) => py::strip(&pyfmt::str_of(x)).to_string(),
            _ => String::new(),
        };
        if s.is_empty() { None } else { Some(s) }
    }

    /// steward 對這個 repo 寫入的共同條件：白名單、禁止 repo、上游要有授權人。
    fn steward_repo_problem(&self, repo: &str) -> Option<String> {
        if repo.is_empty() || !SLUG.is_match(repo) {
            return Some("要明確寫 --repo <owner>/<repo>".into());
        }
        if !self.am.repos.contains_key(repo) {
            return Some(format!("{repo} 不在 auto_merge.repos 白名單"));
        }
        let is_up = self.cfg.upstream_repos.iter().any(|u| u == repo);
        if is_up && self.upstream_auth(repo).is_none() {
            return Some(format!("{repo} 是上游 repo，auto_merge.upstream_authorized_by 沒有（有效的）授權人"));
        }
        let other = self.forbidden_entries().1.into_iter().any(|s| !self.cfg.upstream_repos.contains(&s) && s == repo);
        if other {
            return Some(format!("{repo} 在 forbidden_paths"));
        }
        None
    }

    /// merge-audit.jsonl 裡同 repo、PR、head_sha 的最後一筆。
    fn last_audit(&self, repo: &str, pr: i128, sha: &str) -> R<Option<Value>> {
        let runs = if self.cfg.runs_dir.is_empty() { "_tpm/runs" } else { &self.cfg.runs_dir };
        let p = path::join(&path::join(&self.ws, runs), "merge-audit.jsonl");
        let text = match config::read_text(&p) {
            Ok(t) => t,
            Err("UnicodeDecodeError") => return err("UnicodeDecodeError", "merge-audit.jsonl 不是 UTF-8"),
            Err("ValueError") => return err("ValueError", "embedded null byte"),
            Err(_) => return Ok(None),
        };
        let mut found = None;
        for line in splitlines(&text) {
            let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
            if !rec.is_object() {
                continue;
            }
            let same_pr = rec.get("pr").and_then(py::to_int) == Some(pr);
            let same_repo = rec.get("repo").and_then(Value::as_str) == Some(repo);
            let rec_sha = match rec.get("head_sha") {
                Some(v) if pyfmt::truthy(v) => py::lower(&pyfmt::str_of(v)),
                _ => String::new(),
            };
            if same_repo && same_pr && rec_sha == sha {
                found = Some(rec);
            }
        }
        Ok(found)
    }

    /// auto_merge 的合併條件；全部成立回 None，否則回第一個不成立的理由。
    fn merge_refusal(&self, seg: &str, data: &Value) -> R<Option<String>> {
        let am = &self.am;
        if !am.enabled {
            return Ok(Some("auto_merge 未啟用".into()));
        }
        if !self.is_steward(data)? {
            let who = if am.merger_role.is_empty() { "merger_role" } else { &am.merger_role };
            return Ok(Some(format!("只有 {who}（完整 agent_type）能合併")));
        }
        if SUBST.is_match(seg) {
            return Ok(Some("合併指令不能含指令替換或變數展開".into()));
        }
        if API_CLIENT.is_match(seg) || !GH_PR_MERGE.is_match(seg) {
            return Ok(Some("只接受 gh pr merge，不接受 API 合併".into()));
        }
        let Some(parsed) = gh_pr(seg, "merge") else { return Ok(Some("看不懂這個 gh pr merge".into())) };
        if let Some(dup) = dup_flag_problem(&parsed.args, &[&["--repo", "-R"], &["--match-head-commit"]]) {
            return Ok(Some(dup));
        }
        let Some(pr) = parsed.pr else { return Ok(Some("要寫 PR 編號（或同一個 repo 的 PR URL）".into())) };
        if parsed.args.iter().any(|a| a == "--admin") {
            return Ok(Some("不能用 --admin 繞過保護規則".into()));
        }
        let repo = &parsed.repo;
        if let Some(bad) = self.steward_repo_problem(repo) {
            return Ok(Some(bad));
        }
        let sha = py::lower(&flag(&parsed.args, &["--match-head-commit"]).unwrap_or_default());
        if !SHA40.is_match(&sha) {
            return Ok(Some("要帶 --match-head-commit <40 字元 head SHA>".into()));
        }
        let sha12 = py::head(&sha, 12);
        let Some(rec) = self.last_audit(repo, pr, &sha)? else {
            return Ok(Some(format!("merge-audit.jsonl 沒有 {repo}#{pr} @ {sha12} 的 gate 紀錄（先跑 awf merge-gate check）")));
        };
        if rec.get("decision").and_then(Value::as_str) != Some("merge") {
            return Ok(Some(format!("{repo}#{pr} @ {sha12} 最後一次 gate 判定是 {}，不是 merge", pyfmt::str_opt(rec.get("decision")))));
        }
        let want = match method_flags(&am.merge_method) {
            Some(f) => f[0].to_string(),
            None => format!("--{}", am.merge_method),
        };
        let used: Vec<&str> = ["squash", "merge", "rebase"].into_iter()
            .filter(|m| method_flags(m).is_some_and(|fl| parsed.args.iter().any(|a| fl.contains(&a.as_str())))).collect();
        if used != [am.merge_method.as_str()] {
            return Ok(Some(format!("合併方式要明確寫 {want}（auto_merge.merge_method={}）", am.merge_method)));
        }
        if let Some(fz) = self.frozen() {
            return Ok(Some(format!("自動合併已凍結（{fz}；解凍由人處理）")));
        }
        Ok(None)
    }

    /// steward 在白名單 repo 上可以做的非合併寫入。都成立回 None；否則回理由。
    fn steward_write_refusal(&self, seg: &str, data: &Value) -> R<Option<String>> {
        if !(self.am.enabled && self.is_steward(data)?) {
            return Ok(Some("不是啟用中的 merger_role".into()));
        }
        if SUBST.is_match(seg) || API_CLIENT.is_match(seg) {
            return Ok(Some("不能含指令替換、變數展開或 API 呼叫".into()));
        }
        let writes: [(&str, &[&str]); 3] = [("review", &["--request-changes", "-r"]), ("comment", &[]), ("ready", &["--undo"])];
        for (sub, need) in writes {
            let Some(p) = gh_pr(seg, sub) else { continue };
            if let Some(dup) = dup_flag_problem(&p.args, &[&["--repo", "-R"]]) {
                return Ok(Some(dup));
            }
            if !need.is_empty() && !p.args.iter().any(|a| need.contains(&a.as_str())) {
                return Ok(Some(format!("gh pr {sub} 只允許 {}", need.join("／"))));
            }
            if sub == "review" && p.args.iter().any(|a| matches!(a.as_str(), "--approve" | "-a" | "--comment" | "-c") || a.starts_with("--approve=")) {
                return Ok(Some("gh pr review 只允許 --request-changes".into()));
            }
            if sub == "comment" && p.args.iter().any(|a| a.starts_with("--delete-last") || a.starts_with("--edit-last")) {
                return Ok(Some("gh pr comment 不能改或刪既有留言".into()));
            }
            if p.pr.is_none() {
                return Ok(Some("要寫 PR 編號".into()));
            }
            return Ok(self.steward_repo_problem(&p.repo));
        }
        Ok(Some("不是 steward 允許的寫入（只有 gh pr merge、review --request-changes、comment、ready --undo）".into()))
    }

    /// agent 用內嵌程式（或在 run 目錄裡）寫 run 狀態、稽核紀錄、凍結檔、守門設定，檔名拆開寫也算。
    fn agent_writes_state(&self, seg: &str, cwd: &str) -> R<bool> {
        let runs = &self.runs;
        let in_runs_cwd = !runs.is_empty() && {
            let rel = self.rel_to_workspace(cwd)?.unwrap_or_default();
            fold(&format!("{rel}/")).starts_with(&fold(runs))
        };
        let near_runs = in_runs_cwd || (!runs.is_empty() && fold(&norm(seg)).contains(&fold(runs.trim_end_matches('/'))));
        let probe = NULL_REDIR.replace_all(seg, " ");
        let writes = |p: &str| WRITE_API.is_match(p) || SHELL_WRITE.is_match(p);
        if RUN_INDEX_FRAGMENT.is_match(&probe) && writes(&probe) {
            return Ok(true);
        }
        if !(INTERP.is_match(seg) || near_runs) {
            return Ok(false);
        }
        let frag = STATE_FRAGMENT.is_match(&probe) || (near_runs && (self.stage_json_fragment.is_match(&probe) || CONTEXT_FRAGMENT.is_match(&probe)));
        Ok(frag && writes(&probe))
    }

    /// run_brief.py 的 --log 要明確寫在 runs_dir 底下、副檔名 .log。
    fn run_brief_problem(&self, toks: &[String], base: &str) -> R<Option<String>> {
        if toks.first().is_some_and(|t| t == "--selftest") {
            return Ok(None);
        }
        let runs = &self.runs;
        if runs.is_empty() {
            return Ok(Some("沒有 runs_dir，不能用 awf run-brief 寫 log".into()));
        }
        if flag_dup(toks, &["--log"]) {
            return Ok(Some("--log 只能寫一次".into()));
        }
        let li = toks.iter().position(|t| t == "--log");
        let dd = toks.iter().position(|t| t == "--");
        let bad = match li {
            None => true,
            Some(i) => i + 1 >= toks.len() || dd.is_some_and(|d| d < i),
        };
        if bad {
            return Ok(Some(format!("要帶 --log {runs}<run>/<段>/logs/<label>.log")));
        }
        let raw = li.and_then(|i| toks.get(i + 1)).cloned().unwrap_or_default();
        let rel = self.rel_to_workspace(&self.resolve_arg(base, &raw)?)?;
        match rel {
            Some(r) if r.starts_with(runs.as_str()) && r.ends_with(".log") => Ok(None),
            _ => Ok(Some(format!("--log 只能是 {runs} 底下的 .log（{raw}）"))),
        }
    }

    /// 只寫執行紀錄的角色：這段 shell 有沒有寫檔；有就回傳理由。巢狀的指令遞迴檢查。
    fn record_shell_write(&self, seg: &str, base: &str, depth: usize) -> R<Option<String>> {
        if depth > 4 {
            return Ok(Some("巢狀太深，無法確認".into()));
        }
        let probe = strip_harmless(&QUOTED.replace_all(seg, " "));
        if RECORD_REDIRECT.is_match(&probe) {
            return Ok(Some("> 重導".into()));
        }
        let all = shlex_tokens(seg);
        let mut i = 0;
        while i < all.len() && (ENV_ASSIGN.is_match(&all[i]) || has(CMD_WRAPPERS, &cmd_name(&all[i]))
            || (i > 0 && has(CMD_WRAPPERS, &cmd_name(&all[i - 1])) && all[i].starts_with('-'))) {
            i += 1;
        }
        let toks = &all[i..];
        let Some(first) = toks.first() else { return Ok(None) };
        let name = cmd_name(first);
        let args = &toks[1..];
        // run_brief.py：檢查 --log，內層指令（-- 之後，不經 shell）照同一套規則
        if let Some(rb) = toks.iter().position(|t| basename_lower(t) == "run_brief.py") {
            if rb == 0 || INTERP.is_match(&toks[..rb].join(" ")) {
                let rest = &toks[rb + 1..];
                if let Some(bad) = self.run_brief_problem(rest, base)? {
                    return Ok(Some(format!("awf run-brief {bad}")));
                }
                if let Some(d) = rest.iter().position(|t| t == "--") {
                    return self.record_shell_write(&ps::join(&rest[d + 1..]), base, depth + 1);
                }
                return Ok(None);
            }
        }
        // awf run-brief：同一套（--log 的位置、「--」後面的內層指令）；不是直接執行就看不清楚參數，擋
        if let Some(c) = awf_cli::calls_here(seg).into_iter().find(|c| c.sub == "run-brief") {
            if !c.direct {
                return Ok(Some("awf run-brief 要直接執行（不能經轉呼叫或變數）".into()));
            }
            if let Some(bad) = self.run_brief_problem(&c.args, base)? {
                return Ok(Some(format!("awf run-brief {bad}")));
            }
            if let Some(d) = c.args.iter().position(|t| t == "--") {
                return self.record_shell_write(&ps::join(&c.args[d + 1..]), base, depth + 1);
            }
            return Ok(None);
        }
        if has(SHELL_NAMES, &name) {
            for (k, t) in args.iter().enumerate() {
                if is_dash_c(t) && k + 1 < args.len() {
                    for inner in segments(&args[k + 1]) {
                        if let Some(why) = self.record_shell_write(&inner, base, depth + 1)? {
                            return Ok(Some(why));
                        }
                    }
                    return Ok(None);
                }
            }
        }
        if has(PS_NESTED_SHELLS, &name) || name == "cmd" {
            for (k, t) in args.iter().enumerate() {
                let low = py::lower(t);
                if matches!(low.as_str(), "-encodedcommand" | "-e" | "-ec" | "-enc") {
                    return Ok(Some("看不到 -EncodedCommand 的內容".into()));
                }
                if matches!(low.as_str(), "-c" | "-command" | "/c" | "/k") && k + 1 < args.len() {
                    for inner in ps_segments(&args[k + 1..].join(" ")) {
                        if let Some(why) = self.record_shell_write(&inner, base, depth + 1)? {
                            return Ok(Some(why));
                        }
                    }
                    return Ok(None);
                }
            }
        }
        if matches!(name.as_str(), "eval" | "invoke-expression" | "iex") {
            for inner in segments(&args.join(" ")) {
                if let Some(why) = self.record_shell_write(&inner, base, depth + 1)? {
                    return Ok(Some(why));
                }
            }
            return Ok(None);
        }
        if let Some((s, e)) = RECORD_WRITE_CMD.find(&probe) {
            return Ok(Some(py::strip(&probe[s..e]).to_string()));
        }
        // find -exec／xargs 帶的指令可能在引號裡：連引號內容一起看
        if matches!(name.as_str(), "find" | "xargs" | "parallel" | "watch") {
            let p2 = strip_harmless(&QUOTE_CHARS.replace_all(seg, " "));
            if let Some((s, e)) = RECORD_WRITE_CMD.find(&p2) {
                return Ok(Some(py::strip(&p2[s..e]).to_string()));
            }
        }
        if INTERP.is_match(seg) && INTERP_WRITE.is_match(seg) {
            return Ok(Some("直譯器內嵌程式寫檔或開子程序".into()));
        }
        Ok(None)
    }

    /// Bash／PowerShell 段落是否寫入受保護檔。
    fn shell_writes_protected(&self, seg: &str) -> R<Option<String>> {
        let Some(hit) = self.protected_target(seg, false)? else { return Ok(None) };
        let probe = NULL_REDIR.replace_all(seg, " ");
        Ok(if SHELL_WRITE.is_match(&probe) { Some(hit) } else { None })
    }

    /// shell 寫入 .claude/teams/、.claude/workflows/、run 目錄的 .wf/、methodology.json、guard.config.json（盡力而為）。
    fn shell_writes_team_config(&self, seg: &str, cwd: &str) -> R<bool> {
        let probe = TEAM_TOOL_CALL.replace_all(&MULTI_SLASH.replace_all(&norm(seg), "/"), " ");
        let here = match path::resolve(cwd) {
            Ok(r) => format!("/{}/", fold(&r).trim_matches('/')),
            Err(e) if e.class == "RuntimeError" || e.class == "ValueError" => return Err(Stop::Err(e)),
            Err(_) => format!("/{}/", fold(&norm(cwd)).trim_matches('/')),
        };
        let mut inside = here.contains("/.claude/teams/") || here.contains("/.claude/workflows/") || here.contains("/.wf/");
        if !inside && here.ends_with("/.claude/") && TEAMS_REL.is_match(&probe) {
            inside = true; // cd .claude && echo > teams/x
        }
        if !inside && !TEAM_CONFIG_TARGET.is_match(&probe) {
            return Ok(false);
        }
        let probe = NULL_REDIR.replace_all(&probe, " ");
        Ok(SHELL_WRITE.is_match(&probe) || TEAM_CONFIG_MKDIR.is_match(&probe) || (INTERP.is_match(&probe) && INTERP_WRITE.is_match(&probe)))
    }

    /// Bash 與 PowerShell 工具共用的 shell 守門規則。
    pub fn check_shell_rules(&self, data: &Value) -> R<()> {
        let ti = tool_input(data)?;
        let mut cmd = match ti.get("command") {
            Some(Value::String(s)) => s.clone(),
            Some(v) if pyfmt::truthy(v) => return err("TypeError", "command 不是字串"),
            _ => String::new(),
        };
        if cmd.is_empty() {
            return Ok(());
        }
        let tool = data.get("tool_name").and_then(Value::as_str).unwrap_or("");
        let is_ps = tool == "PowerShell";
        let segs: Vec<String> = if is_ps {
            let s = ps_segments(&cmd);
            cmd = s.join(" ; "); // 正規化後的指令（Set-Location 已換成 cd）
            s
        } else {
            segments(&cmd)
        };
        let segs: Vec<String> = segs.iter().map(|s| canon_gh(s)).collect();
        let cfg = &self.cfg;
        let am = &self.am;
        let raw_agent = str_or_empty(data, "agent_type")?;
        let agent = config::role_name(&raw_agent).to_string();
        let team = config::is_team_agent(&raw_agent, cfg);
        let configured = cfg.configured;
        let git_rules = team || (configured && cfg.main_session_git_rules);
        let gh_rules = team || (configured && cfg.main_session_github_rules);
        let am_on = configured && am.enabled;
        let cwd = match data.get("cwd") {
            Some(Value::String(s)) if !s.is_empty() => path::pp(s),
            Some(v) if pyfmt::truthy(v) => return err("TypeError", "cwd 不是字串"),
            _ => self.ws.clone(),
        };
        let dirs = self.seg_dirs(&segs, &cwd)?;
        let fdir = segs.iter().enumerate().filter(|(_, s)| GIT_WORD.is_match(s)).last().and_then(|(i, _)| dirs.get(i).cloned())
            .or_else(|| dirs.last().cloned()).unwrap_or_else(|| cwd.clone());
        let (fpaths, fslugs) = self.forbidden_entries();
        let who_main = |a: &str| if a.is_empty() { "主 session".to_string() } else { a.to_string() };
        if (gh_rules || am_on) && is_interp_review(&cmd) {
            return Err(self.block(&format!("{} 不能用直譯器（python -c、node -e、heredoc）打 PR reviews 端點；核准一律由人在 GitHub 上做", who_main(&agent))));
        }
        // steward 在 auto_merge 條件下允許的段落（合併、退回、留言、轉回 draft）；其餘段落照一般規則
        let mut allowed = vec![false; segs.len()];
        for (i, s) in segs.iter().enumerate() {
            allowed[i] = if GH_PR_MERGE.is_match(s) {
                self.merge_refusal(s, data)?.is_none()
            } else {
                GH_WRITE.is_match(s) && self.steward_write_refusal(s, data)?.is_none()
            };
        }
        let rest_cmd = segs.iter().enumerate().filter(|(i, _)| !allowed[*i]).map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" ; ");
        // rest_cmd 在下面的迴圈裡不變：整條的判斷只算一次（上千段的指令原本每段重算，會拖到逾時）
        let rest_merge = std::cell::OnceCell::new();
        let record_only = team && config::RECORD_ONLY_ROLES.contains(&config::team_role(&raw_agent, cfg).as_str());

        if record_only && !is_ps && INTERP.is_match(&cmd) && HEREDOC.is_match(&cmd) && INTERP_WRITE.is_match(&cmd) {
            // heredoc／stdin 餵給直譯器的程式碼被換行切成好幾段，整條一起看
            return Err(self.block(&format!("{agent} 是唯讀角色，不能用 heredoc／stdin 餵給直譯器的程式寫檔或開子程序")));
        }
        let runs_label = if self.runs.is_empty() { "run 目錄" } else { &self.runs };
        let merger_label = if am.merger_role.is_empty() { "merger_role" } else { &am.merger_role };
        for (i, seg) in segs.iter().enumerate() {
            let d = &dirs[i];
            if PUSH_RE.is_match(seg) {
                if team {
                    return Err(self.block(&format!("{agent} 不能 push；推送由主 session 在決策者當下同意後執行")));
                }
                for url in self.push_target_urls(seg, d) {
                    for s in &fslugs {
                        if !s.is_empty() && url.contains(s.as_str()) {
                            return Err(self.block(&format!("不允許推到上游或禁止的 repo {s}（{url}）；要決策者當下明確同意並由決策者自己執行")));
                        }
                    }
                }
                if git_rules && is_force_push(seg) {
                    return Err(self.block("不 force push（含 --force-with-lease、+refspec）"));
                }
                if git_rules || am_on {
                    if let Some(hit) = self.pushes_protected(seg, d) {
                        return Err(self.block(&format!("不直接 push 到受保護分支 {hit}（protected_branches）；改推功能分支、開 PR 到 fork，合併走 PR 流程")));
                    }
                }
            }
            if git_rules && GIT_WORD.is_match(seg) && NO_VERIFY.is_match(seg) {
                return Err(self.block("不用 --no-verify 跳過 git hooks"));
            }
            if git_rules {
                let prefixes = &cfg.allowed_branch_prefixes;
                for b in new_branches(seg) {
                    if !prefixes.is_empty() && !prefixes.iter().any(|p| b.starts_with(p.as_str())) {
                        return Err(self.block(&format!("分支名 {b} 不在允許的前綴（{}）；改用允許的前綴並在回報註明", prefixes.join("、"))));
                    }
                }
            }
            let who = who_main(&agent);
            let ok = allowed[i];
            if team || configured {
                if let Some(sec) = reads_secret(seg) {
                    return Err(self.block(&format!("{who} 不讀機敏檔 {sec} 的內容（.env、金鑰、憑證）；要判斷有無只能用 grep -q（rules.md 規則 5）")));
                }
            }
            if (gh_rules || am_on) && GH_BYPASS.is_match(seg) {
                return Err(self.block(&format!("{who} 不能設定 gh alias 或安裝／執行 gh extension（可以繞過守門）；需要時由人自己設定")));
            }
            if team && (GH_BYPASS.is_match(seg) || GH_ACCOUNT_WRITE.is_match(seg)) {
                return Err(self.block(&format!("{agent} 不能改 GitHub 帳號層級的設定（alias、extension、ssh／gpg 金鑰、gist、登入）")));
            }
            let awf = awf_cli::calls(seg);
            let steward = if team && !awf.is_empty() { self.is_steward(data)? } else { false };
            let awf_deny: Vec<Deny> = if team { awf.iter().filter_map(|c| c.team_deny(steward)).collect() } else { Vec::new() };
            if team && ((MERGE_GATE_PY.is_match(seg) && CHECK_WORD.is_match(seg) && !self.is_steward(data)?) || awf_deny.contains(&Deny::MergeGate)) {
                return Err(self.block(&format!("{agent} 不能跑 awf merge-gate check；只有 {merger_label}（完整 agent_type）能跑")));
            }
            if team {
                if save_run_write(seg, "pack", &["--selftest"], 0).is_some() || awf_deny.contains(&Deny::Pack) {
                    return Err(self.block(&format!("{agent} 不能跑 awf pack（資料包與調研重用的紀錄只由主 session 產生，agent 只讀 pack.md）")));
                }
                if save_run_write(seg, "deliverables", &["status", "check", "affected", "--selftest", "-h", "--help"], 0).is_some() || awf_deny.contains(&Deny::Deliverables) {
                    return Err(self.block(&format!("{agent} 只能跑 awf deliverables status／check／affected（交付設定、來源紀錄、追溯矩陣與開發紀錄只由主 session 寫；\
agent 只改派工單指定的文件小節）")));
                }
                if save_run_write(seg, "kb", &["query", "lint", "--selftest", "-h", "--help"], 0).is_some() || awf_deny.contains(&Deny::Kb) {
                    return Err(self.block(&format!("{agent} 只能跑 awf kb query／lint（build 會改寫共用的索引與卡片，只由主 session 或 awf pack 執行）")));
                }
                let sr = save_run_write(seg, "save_run", &["status", "--selftest"], 0)
                    .or_else(|| awf_deny.iter().find_map(|d| if let Deny::SaveRun(x) = d { Some(x.clone()) } else { None }));
                if let Some(sr) = sr {
                    return Err(self.block(&format!("{agent} 不能跑 awf save-run {sr}（team agent 只能跑唯讀的 status 與 --selftest）；\
run 狀態、核准、放棄、升級、後續、結案一律由主 session（或人）執行")));
                }
                let aw = save_run_write(seg, "awf", &["--selftest"], 0)
                    .or_else(|| awf_deny.iter().find_map(|d| if let Deny::State(x) = d { Some(x.clone()) } else { None }));
                if let Some(aw) = aw {
                    return Err(self.block(&format!("{agent} 不能跑 awf {aw}（狀態機會存段結果、核准、產生下一段的腳本，只由主 session 執行；team agent 只能跑 awf state --selftest）")));
                }
                // 1.0 才有的子指令：會改共用狀態或設定的只由主 session 跑；看不出子指令的一律不允許
                for d in &awf_deny {
                    let why = match d {
                        Deny::Init => "不能跑 awf init（套用與升級會改寫 .claude/ 底下的設定與工具，只由人或主 session 執行）".to_string(),
                        Deny::Build => "不能跑 awf build（會改寫 workflow 腳本的內嵌內容，只由人或主 session 執行）".to_string(),
                        Deny::Spec(x) => format!("不能跑 awf spec {x}（規格與編號登記只由主 session 改；team agent 只能跑 awf spec status／validate／show，以及不帶 --write／--adopt／--retire 的 ids）"),
                        Deny::Kg(x) => format!("不能跑 awf kg {x}（圖與 vault 只由主 session 建與匯出；team agent 只能跑 awf kg locate／trace／impact／check／neighbors）"),
                        Deny::Ci(x) => format!("不能跑 awf ci {x}（CI 檔與 required checks 只由主 session 或人寫；team agent 只能跑不帶 --write 的 awf ci render 與 awf ci check）"),
                        Deny::Watch => "只能跑 awf watch --once（持續監看會一直寫進度事件，由主 session 或人執行）".to_string(),
                        Deny::Unknown(x) => format!("不能跑 awf {x}（看不出是哪個子指令，無法確認是否允許；請直接寫 awf <子指令> <參數>）"),
                        Deny::Opaque(x) => format!("不能這樣呼叫 awf{x}；請直接寫 awf <子指令> <參數>"),
                        _ => continue,
                    };
                    return Err(self.block(&format!("{agent} {why}")));
                }
            }
            if record_only {
                if let Some(why) = self.record_shell_write(seg, d, 0)? {
                    return Err(self.block(&format!("{agent} 是唯讀角色（只有 Write 能寫 {runs_label}<run>/<段>/<label>.md 的執行紀錄），不能用 shell 寫檔\
（{why}）；指令輸出直接摘進執行紀錄、丟 /dev/null，長輸出用 awf run-brief --log {}<run>/<段>/logs/<label>.log -- <指令>", self.runs)));
                }
            }
            if team && self.agent_writes_state(seg, d)? {
                return Err(self.block(&format!("{agent} 不能寫 run.json、<段>.json、稽核紀錄、凍結檔或守門設定（含拆開檔名的寫法）；run 狀態只由 awf save-run 在主 session 寫")));
            }
            if team && shell_writes_kg(seg) {
                return Err(self.block(&format!("{agent} 不能寫知識圖的檔（graph.db 與 vault，含 vault/notes/；圖只由主 session 跑 awf kg build／export 產生，筆記是人寫的）")));
            }
            if team && self.shell_writes_team_config(seg, d)? {
                return Err(self.block(&format!("{agent} 不能寫 .claude/teams/、.claude/workflows/、run 目錄的 .wf/、methodology.json 或 guard.config.json\
（守門設定、profile 與 workflow 腳本只由人或主 session 改）")));
            }
            for (tool_name, sub, tool_re) in [("awf publish", "publish", &PUBLISH_PY), ("awf merge-gate", "merge-gate", &MERGE_GATE_PY)] {
                let via_awf: Vec<&awf_cli::Call> = awf.iter().filter(|c| c.sub == sub).collect();
                if tool_re.is_match(seg) || !via_awf.is_empty() {
                    let mut targs = shlex_tokens(seg);
                    for c in &via_awf {
                        targs.push(String::new()); // 隔開，避免前一組最後的旗標吃到下一組的第一個參數
                        targs.extend(c.args.iter().cloned());
                    }
                    // 每個出現的值都要檢查（argparse 取最後一個）
                    for pd in flag_values(&targs, &["--project-dir"]) {
                        if self.resolve_arg(d, &pd)? != self.ws_real()? {
                            return Err(self.block(&format!("{tool_name} 的 --project-dir 只能是本專案（{}），不能指到 {pd}", path::display(&self.ws))));
                        }
                    }
                    if sub == "merge-gate" {
                        for rd in flag_values(&targs, &["--runs-dir"]) {
                            let runs = if cfg.runs_dir.is_empty() { "_tpm/runs" } else { &cfg.runs_dir };
                            if self.resolve_arg(&self.ws, &rd)? != path::resolve(&path::join(&self.ws, runs))? {
                                return Err(self.block(&format!("awf merge-gate 的 --runs-dir 只能是 guard.config.json 的 runs_dir（{}）", cfg.runs_dir)));
                            }
                        }
                    }
                }
            }
            if (gh_rules || am_on) && !ok && (is_merge_cmd(seg) || *rest_merge.get_or_init(|| is_merge_cmd(&rest_cmd))) {
                let why = if am.enabled && GH_PR_MERGE.is_match(seg) { self.merge_refusal(seg, data)? } else { None };
                let why = why.filter(|w| !w.is_empty()).map(|w| format!("：{w}")).unwrap_or_default();
                return Err(self.block(&format!("{who} 不能合併 PR（gh pr merge、gh api／curl 打 merge 端點）{why}；合併只由 auto_merge 的 merger_role 在 gate 通過後執行，其餘由人在 GitHub 上做")));
            }
            if (gh_rules || am_on) && is_approve_cmd(seg) {
                return Err(self.block(&format!("{who} 不能核准 PR（gh pr review --approve、API 送 APPROVE）；agent 與主 session 一律不核准")));
            }
            let rest_write = is_rest_write(seg);
            if rest_write {
                for s in &fslugs {
                    if !s.is_empty() && seg.contains(s.as_str()) {
                        return Err(self.block(&format!("不允許對上游或禁止的 repo {s} 做寫入動作；要決策者當下明確同意並自己執行")));
                    }
                }
            }
            if gh_rules && rest_write {
                return Err(self.block("不用 curl／Invoke-RestMethod／Invoke-WebRequest 直接對 GitHub API 寫入；改用 gh 並明確寫 --repo（才受守門規則檢查）"));
            }
            let prot = if team || configured { self.shell_writes_protected(seg)? } else { None };
            if let Some(prot) = &prot {
                if team {
                    return Err(self.block(&format!("{agent} 不能寫 {prot}（守門設定、gate 稽核紀錄與凍結檔只由人或確定性工具寫；auto_merge 設定改走 PR＋CODEOWNERS）")));
                }
                if configured && *prot == am.freeze_file && !freezes_only(seg) {
                    return Err(self.block(&format!("主 session 不解凍自動合併（刪除凍結檔或寫 frozen=false）；解凍由人自己做（在提示列用 ! 執行，或手動編輯 {prot}）")));
                }
                if AUDIT_FILES.contains(&prot.as_str()) || (prot == "guard.config.json" && am.enabled) {
                    return Err(self.block(&format!("主 session 不直接改 {prot}（稽核紀錄只由 awf merge-gate、awf publish 寫；auto_merge 啟用時 guard.config.json 改走 PR＋CODEOWNERS）")));
                }
            }
            if team && (PUBLISH_PY.is_match(seg) || awf_deny.contains(&Deny::Publish)) {
                return Err(self.block(&format!("{agent} 不能執行 awf publish；推送與開 PR 由主 session 執行")));
            }
            if ok {
                continue; // steward 允許的寫入已檢查過 repo 白名單與上游授權
            }
            let gh_write = GH_WRITE.is_match(seg);
            let gh_api_write = GH_API_WRITE.is_match(seg);
            if gh_write || gh_api_write {
                if team {
                    let why = if self.is_steward(data)? { self.steward_write_refusal(seg, data)? } else { None };
                    let why = why.filter(|w| !w.is_empty()).map(|w| format!("：{w}")).unwrap_or_default();
                    return Err(self.block(&format!("{agent} 不能對 GitHub 做寫入動作（{}）{why}", py::head(seg, 60))));
                }
                for s in &fslugs {
                    if !s.is_empty() && seg.contains(s.as_str()) {
                        return Err(self.block(&format!("不允許對上游或禁止的 repo {s} 做寫入動作；要決策者當下明確同意並自己執行")));
                    }
                }
                let fork = &cfg.fork_repo;
                if gh_rules && configured && gh_write {
                    if flag_dup(&shlex_tokens(seg), &["--repo", "-R"]) {
                        return Err(self.block("gh 寫入動作的 --repo／-R 只能寫一次（重複出現時 gh 取最後一個值，無法確認實際目標）"));
                    }
                    if !fork.is_empty() && !Re::dynamic(format!(r"(?:-R|--repo)[\s=]+{}\b", regex::escape(fork)), "").is_match(seg) {
                        return Err(self.block(&format!("gh 寫入動作要明確寫 --repo {fork}（在 fork 裡 gh 可能預設到上游）")));
                    }
                    if fork.is_empty() && !REPO_FLAG_ANY.is_match(seg) {
                        return Err(self.block("gh 寫入動作要明確寫 --repo <owner>/<repo>（gh 可能預設到上游）"));
                    }
                }
                if gh_rules && configured && gh_api_write && !fork.is_empty() && !seg.contains(&format!("repos/{fork}/")) {
                    return Err(self.block(&format!("gh api 寫入只能對 repos/{fork}/")));
                }
            }
            for e in &fpaths {
                let fseg = fold(seg);
                let mut hit = fseg.contains(&fold(e));
                if !hit && !path::os_isabs(e) {
                    hit = fseg.contains(&fold(&path::join(&self.ws, e)));
                }
                if hit && DESTRUCTIVE.is_match(seg) {
                    return Err(self.block(&format!("不動 {e}（禁止路徑，只能讀）")));
                }
            }
        }
        self.forward(super::guard::git_toplevel(&fdir), "guard-git", data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn patterns_compile() {
        for r in all_patterns() {
            assert!(r.compiles(), "樣式編不過：{}", r.source());
        }
    }

    #[test]
    fn flags() {
        assert_eq!(flag_values(&v(&["--repo", "a/b", "-R=c/d", "-sRe/f", "--repo=g/h", "-R"]), &["--repo", "-R"]), v(&["a/b", "c/d", "e/f", "g/h", ""]));
        assert_eq!(flag_values(&v(&["-sR", "x/y"]), &["--repo", "-R"]), v(&["x/y"]));
        assert!(flag_dup(&v(&["-R", "a", "--repo", "b"]), &["--repo", "-R"]));
    }

    #[test]
    fn merges_and_approvals() {
        assert!(is_merge_cmd("gh pr merge 3"));
        assert!(is_merge_cmd("curl -X PUT https://api.github.com/repos/a/b/pulls/3/merge"));
        assert!(!is_merge_cmd("curl https://example.com/merge-sort.html"));
        assert!(is_approve_cmd("gh pr review 3 -ab x"));
        assert!(is_approve_cmd("gh pr review 3 --approve"));
        assert!(!is_approve_cmd("gh pr review 3 --request-changes -b x"));
        assert!(is_rest_write("curl -d '{}' https://api.github.com/repos/a/b/issues"));
        assert!(!is_rest_write("curl https://api.github.com/repos/a/b/issues"));
        assert_eq!(canon_gh("gh --repo a/b pr merge 3"), "gh pr merge 3 --repo a/b");
        assert_eq!(canon_gh("gh pr -R a/b merge 3"), "gh pr merge 3 -R a/b");
    }

    #[test]
    fn branches_and_pushes() {
        assert_eq!(new_branches("git checkout -b feature/x"), v(&["feature/x"]));
        assert_eq!(new_branches("git -C repo switch -c wip"), v(&["wip"]));
        assert_eq!(new_branches("git branch -d old"), Vec::<String>::new());
        assert!(is_force_push("git push -uf origin x"));
        assert!(is_force_push("git push origin +x"));
        assert!(!is_force_push("git push origin x"));
        assert_eq!(reads_secret("cat .env").as_deref(), Some(".env"));
        assert_eq!(reads_secret("grep -q KEY .env"), None);
        assert_eq!(reads_secret("cat .env.example"), None);
    }

    #[test]
    fn tool_calls() {
        let ro = &["status", "--selftest"];
        assert_eq!(save_run_write("python3 .claude/teams/tools/save_run.py approve x", "save_run", ro, 0).as_deref(), Some("approve"));
        assert_eq!(save_run_write("python3 .claude/teams/tools/save_run.py status x", "save_run", ro, 0), None);
        assert_eq!(save_run_write("cat .claude/teams/tools/save_run.py", "save_run", ro, 0), None);
        assert_eq!(save_run_write("bash -c 'python3 save_run.py close r'", "save_run", ro, 0).as_deref(), Some("close"));
        assert_eq!(save_run_write("xargs python3 save_run.py", "save_run", ro, 0).as_deref(), Some("（經轉呼叫程式執行，看不到子指令）"));
        assert_eq!(save_run_write("python3 -c 'import save_run'", "save_run", ro, 0).as_deref(), Some("（內嵌程式呼叫）"));
    }
}
