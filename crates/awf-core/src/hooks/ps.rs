//! 指令拆段：bash 依引號外的運算子切段（[`segments`]），PowerShell 先拆成 token 再正規化成 bash 風格（[`ps_segments`]）。
//! 原本是 0.x 的 Python 工具 `guard.py` 的 segments／_ungroup／_ps_tokenize／_ps_normalize，逐字元照原本的狀態機。

use super::guard::cmd_name;
use super::py;

/// bash 指令依引號外的 ; & && | || 換行切段。引號內的運算子不切，2>&1、&> 這類重新導向的 & 也不切。
pub fn segments(cmd: &str) -> Vec<String> {
    let ch: Vec<char> = cmd.chars().collect();
    let n = ch.len();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut last_gt = false; // 上一個加進去的片段正好是「>」
    let mut q = '\0';
    let mut i = 0;
    while i < n {
        let c = ch[i];
        if q == '\'' {
            cur.push(c);
            last_gt = c == '>';
            if c == '\'' {
                q = '\0';
            }
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < n {
            cur.push(c);
            cur.push(ch[i + 1]);
            last_gt = false;
            i += 2;
            continue;
        }
        if q == '"' {
            cur.push(c);
            last_gt = c == '>';
            if c == '"' {
                q = '\0';
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            q = c;
            cur.push(c);
            last_gt = false;
            i += 1;
            continue;
        }
        if c == '&' && (last_gt || ch.get(i + 1) == Some(&'>')) {
            cur.push(c);
            last_gt = false;
            i += 1;
            continue;
        }
        if matches!(c, ';' | '&' | '|' | '\n') {
            out.push(std::mem::take(&mut cur));
            last_gt = false;
            i += 1;
            continue;
        }
        cur.push(c);
        last_gt = c == '>';
        i += 1;
    }
    out.push(cur);
    out.iter().map(|x| ungroup(x)).filter(|y| !y.is_empty()).collect()
}

/// 去掉段落頭尾的子 shell「(」「)」與群組「{ 」「}」。算術的 (( 不動，配對完整的 $(…) 不動。
fn ungroup(seg: &str) -> String {
    let mut s: &str = py::strip(seg);
    loop {
        let mut it = s.chars();
        let Some(first) = it.next() else { break };
        let second = it.next();
        let open_paren = first == '(' && !s.starts_with("((");
        let open_brace = first == '{' && second.map(py::is_space).unwrap_or(true);
        if !(open_paren || open_brace) {
            break;
        }
        s = py::lstrip(&s[1..]);
    }
    loop {
        let Some(last) = s.chars().next_back() else { break };
        let count = |c: char| s.chars().filter(|x| *x == c).count();
        let close_paren = last == ')' && count(')') > count('(');
        let close_brace = last == '}' && count('}') > count('{');
        if !(close_paren || close_brace) {
            break;
        }
        s = py::rstrip(&s[..s.len() - 1]);
    }
    s.to_string()
}

const PS_SQ: &[char] = &['\'', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'];
const PS_DQ: &[char] = &['"', '\u{201c}', '\u{201d}', '\u{201e}'];
const PS_PATH_FLAGS: &[&str] = &["-path", "-literalpath", "-lp", "-destination"];
pub const PS_NESTED_SHELLS: &[&str] = &["powershell", "pwsh"];

fn ps_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "set-location" | "sl" | "chdir" | "cd" | "push-location" | "pushd" => "cd",
        "remove-item" | "ri" | "del" | "erase" | "rd" | "rmdir" | "rm" => "rm",
        "move-item" | "mi" | "move" | "mv" => "mv",
        "copy-item" | "cpi" | "copy" | "cp" => "cp",
        "set-content" | "sc" | "add-content" | "ac" | "out-file" | "new-item" | "ni" | "clear-content" | "clc" | "tee-object" | "tee" => "tee",
        _ => return None,
    })
}

/// 取出字串裡 $( … ) 的內容（會被 PowerShell 執行），支援巢狀括號。
fn ps_subexpressions(text: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let mut found = None;
        let mut k = i;
        while k + 1 < text.len() {
            if text[k] == '$' && text[k + 1] == '(' {
                found = Some(k);
                break;
            }
            k += 1;
        }
        let Some(start) = found else { return out };
        let mut depth = 1;
        let mut j = start + 2;
        while j < text.len() && depth != 0 {
            match text[j] {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        let end = if depth == 0 { j - 1 } else { text.len() };
        out.push(text[(start + 2).min(end)..end].iter().collect());
        i = j;
    }
}

/// 把 PowerShell 指令拆成「段落 → token」，去掉引號與跳脫；雙引號裡的 $(…) 另外拆成段落。
#[allow(unused_assignments)] // 巨集在結尾收尾時重設的狀態不會再被讀
fn ps_tokenize(cmd: &str, depth: usize) -> Vec<Vec<String>> {
    let ch: Vec<char> = cmd.chars().collect();
    let n = ch.len();
    let mut segs: Vec<Vec<String>> = Vec::new();
    let mut nested: Vec<String> = Vec::new();
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_empty = true; // cur 還沒有任何片段
    let mut last_ends_gt = false; // 最後一個片段以 > 結尾
    let mut has_tok = false;
    let mut i = 0;

    macro_rules! end_tok {
        () => {{
            if has_tok {
                toks.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur_empty = true;
            last_ends_gt = false;
            has_tok = false;
        }};
    }
    macro_rules! end_seg {
        () => {{
            end_tok!();
            if !toks.is_empty() {
                segs.push(std::mem::take(&mut toks));
            }
            toks.clear();
        }};
    }

    while i < n {
        let c = ch[i];
        if c == '`' {
            // 跳脫字元；行尾的 ` 是續行
            if ch.get(i + 1) == Some(&'\r') && ch.get(i + 2) == Some(&'\n') {
                i += 3;
                end_tok!();
                continue;
            }
            if i + 1 < n && (ch[i + 1] == '\r' || ch[i + 1] == '\n') {
                i += 2;
                end_tok!();
                continue;
            }
            if i + 1 < n {
                cur.push(ch[i + 1]);
                cur_empty = false;
                last_ends_gt = ch[i + 1] == '>';
                has_tok = true;
            }
            i += 2;
            continue;
        }
        if PS_SQ.contains(&c) {
            has_tok = true;
            i += 1;
            while i < n {
                if PS_SQ.contains(&ch[i]) {
                    if i + 1 < n && PS_SQ.contains(&ch[i + 1]) {
                        cur.push('\'');
                        cur_empty = false;
                        last_ends_gt = false;
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                cur.push(ch[i]);
                cur_empty = false;
                last_ends_gt = ch[i] == '>';
                i += 1;
            }
            continue;
        }
        if PS_DQ.contains(&c) {
            has_tok = true;
            i += 1;
            let mut raw: Vec<char> = Vec::new();
            while i < n {
                let x = ch[i];
                if x == '`' && i + 1 < n {
                    raw.push(ch[i + 1]);
                    i += 2;
                    continue;
                }
                if PS_DQ.contains(&x) {
                    if i + 1 < n && PS_DQ.contains(&ch[i + 1]) {
                        raw.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                raw.push(x);
                i += 1;
            }
            cur.extend(raw.iter());
            cur_empty = false;
            last_ends_gt = raw.last() == Some(&'>');
            nested.extend(ps_subexpressions(&raw));
            continue;
        }
        if c == '&' && !cur_empty && last_ends_gt {
            // 2>&1、*>&1 這類重新導向，不是運算子
            cur.push(c);
            last_ends_gt = false;
            i += 1;
            continue;
        }
        if matches!(c, ';' | '\n' | '|' | '&' | '(' | ')' | '{' | '}' | '\r') {
            end_seg!();
            i += 1;
            continue;
        }
        if py::is_space(c) || c == ',' {
            // 逗號是陣列元素分隔（-ArgumentList 'pr','merge'）
            end_tok!();
            i += 1;
            continue;
        }
        cur.push(c);
        cur_empty = false;
        last_ends_gt = c == '>';
        has_tok = true;
        i += 1;
    }
    end_seg!();
    // 巢狀深度設上限，避免惡意輸入造成過深的遞迴（Python 會丟 RecursionError 而擋下；這裡超過就不再展開，由呼叫端另外擋）
    if depth < 64 {
        for inner in nested {
            segs.extend(ps_tokenize(&inner, depth + 1));
        }
    } else if !nested.is_empty() {
        super::re::mark_fault();
    }
    segs
}

/// 把一段 PowerShell token 正規化成 bash 風格的 token；巢狀的 shell 字串會展開成多段。
fn ps_normalize(toks: &[String], depth: usize) -> Vec<Vec<String>> {
    let mut toks = toks;
    while let Some(first) = toks.first() {
        if first == "." || first == "&" {
            toks = &toks[1..];
        } else {
            break;
        }
    }
    let Some(first) = toks.first() else { return Vec::new() };
    let name = cmd_name(first);
    let rest = &toks[1..];
    let with_name = |n: &str| -> Vec<Vec<String>> {
        let mut v = vec![n.to_string()];
        v.extend(rest.iter().cloned());
        vec![v]
    };
    if name == "git" || name == "gh" {
        return with_name(&name);
    }
    if let Some(canon) = ps_alias(&name) {
        let mut v = vec![canon.to_string()];
        v.extend(rest.iter().filter(|t| !PS_PATH_FLAGS.contains(&py::lower(t).as_str())).cloned());
        return vec![v];
    }
    if depth < 4 && (name == "invoke-expression" || name == "iex") {
        return rest.iter().filter(|t| !t.starts_with('-')).flat_map(|t| ps_parse(t, depth + 1)).collect();
    }
    if depth < 4 && matches!(name.as_str(), "start-process" | "saps" | "start") {
        let mut exe: Option<String> = None;
        let mut args: Vec<String> = Vec::new();
        let mut k = 0;
        while k < rest.len() {
            let t = &rest[k];
            let low = py::lower(t);
            if (low == "-filepath" || low == "-file") && k + 1 < rest.len() {
                exe = Some(rest[k + 1].clone());
                k += 2;
                continue;
            }
            if (low == "-argumentlist" || low == "-args") && k + 1 < rest.len() {
                k += 1;
                while k < rest.len() && !rest[k].starts_with('-') {
                    args.extend(py::split_ws(&rest[k]));
                    k += 1;
                }
                continue;
            }
            if t.starts_with('-') {
                k += if matches!(low.as_str(), "-workingdirectory" | "-verb" | "-windowstyle" | "-redirectstandardoutput" | "-redirectstandarderror" | "-redirectstandardinput") { 2 } else { 1 };
                continue;
            }
            if exe.is_none() {
                exe = Some(t.clone());
            } else {
                args.extend(py::split_ws(t));
            }
            k += 1;
        }
        return match exe {
            Some(e) if !e.is_empty() => {
                let mut v = vec![e];
                v.extend(args);
                ps_normalize(&v, depth + 1)
            }
            _ => Vec::new(),
        };
    }
    if depth < 4 && PS_NESTED_SHELLS.contains(&name.as_str()) {
        for (k, t) in rest.iter().enumerate() {
            let low = py::lower(t);
            if (low == "-command" || low == "-c") && k + 1 < rest.len() {
                return ps_parse(&rest[k + 1..].join(" "), depth + 1);
            }
        }
        return with_name(&name);
    }
    if depth < 4 && name == "cmd" {
        for (k, t) in rest.iter().enumerate() {
            let low = py::lower(t);
            if (low == "/c" || low == "/k") && k + 1 < rest.len() {
                return ps_parse(&rest[k + 1..].join(" "), depth + 1);
            }
        }
        return with_name(&name);
    }
    if depth < 4 && matches!(name.as_str(), "bash" | "sh" | "wsl") {
        if let Some(k) = rest.iter().position(|t| t == "-c") {
            if k + 1 < rest.len() {
                return segments(&rest[k + 1..].join(" ")).iter().map(|s| py::shlex_tokens(s)).collect();
            }
        }
    }
    with_name(first)
}

fn ps_parse(cmd: &str, depth: usize) -> Vec<Vec<String>> {
    ps_tokenize(cmd, 0).iter().flat_map(|t| ps_normalize(t, depth)).collect()
}

/// 組回 bash 風格字串，讓後面的 shlex 拆得回同樣的 token。
pub fn join(toks: &[String]) -> String {
    toks.iter().map(|t| {
        let needs = t.is_empty() || t.chars().any(|c| py::is_space(c) || matches!(c, '\'' | '"' | '\\' | ';' | '&' | '|'));
        if needs { py::shlex_quote(t) } else { t.clone() }
    }).collect::<Vec<_>>().join(" ")
}

/// PowerShell 指令 → 正規化後的段落。Windows 路徑的反斜線一律換成 /。
pub fn ps_segments(cmd: &str) -> Vec<String> {
    ps_parse(cmd, 0).into_iter().filter(|t| !t.is_empty())
        .map(|t| join(&t.iter().map(|x| x.replace('\\', "/")).collect::<Vec<_>>())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_segments() {
        assert_eq!(segments("(cd x && git push o HEAD:main)"), vec!["cd x", "git push o HEAD:main"]);
        assert_eq!(segments("a 2>&1 | b &> f; echo 'x;y' \"p|q\""), vec!["a 2>&1", "b &> f", "echo 'x;y' \"p|q\""]);
        assert_eq!(segments("{ a; }"), vec!["a"]);
        assert_eq!(segments("echo $(x)"), vec!["echo $(x)"]);
    }

    #[test]
    fn powershell_segments() {
        assert_eq!(ps_segments("Set-Location repos\\app; & 'C:\\Git\\cmd\\git.exe' push"), vec!["cd repos/app", "git push"]);
        assert_eq!(ps_segments("iex 'gh pr merge 3'"), vec!["gh pr merge 3"]);
        assert_eq!(ps_segments("Start-Process gh -ArgumentList 'pr','merge','3'"), vec!["gh pr merge 3"]);
        assert_eq!(ps_segments("pwsh -Command \"git push\""), vec!["git push"]);
        assert_eq!(ps_segments("\"$(git push)\""), vec!["'$(git push)'", "git push"]);
        assert_eq!(ps_segments("'x' | Out-File -Append a.txt"), vec!["x", "tee -Append a.txt"]);
    }
}
