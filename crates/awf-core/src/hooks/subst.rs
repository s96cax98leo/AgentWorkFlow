//! bash 段落裡「會被 shell 另外執行」的內層指令：`$(…)`、反引號、`<(…)`、`>(…)`，雙引號裡的也算（單引號裡的不會執行，不算）。
//!
//! 切詞（shlex）看不到這些：`X=$(awf save-run approve r)` 是一個賦值 token、`echo "$(…)"` 是一個字串參數。
//! 所以比對 token 的規則在切詞前先用這裡把內層抽出來，各自當一段再檢查一次（[`inner_segments`]）。
//! 原字串的判定照舊，內層只會多擋。
//!
//! 上限：巢狀 [`MAX_DEPTH`] 層、一條指令 [`MAX_INNER`] 段。超過時回報 `over`，由呼叫端往嚴的方向處理。
//! heredoc 的內文不是指令（常見的 `git commit -m "$(cat <<'EOF' … EOF)"`），不當成段落；
//! 界線沒加引號的 heredoc 內文裡的指令替換照樣會執行，另外抽出來。

use super::ps::segments;

/// 內層巢狀的上限（每一層都要重新掃一次內文）。
pub const MAX_DEPTH: usize = 4;
/// 一條指令抽出來的內層段落數上限。
pub const MAX_INNER: usize = 256;
/// 抽掉內層之後留在原位的替身：一個看不到內容的變數。
pub const PLACEHOLDER: &str = "$_";

/// 便宜的預先過濾：沒有這些寫法就不用掃。
pub fn has_subst(seg: &str) -> bool {
    seg.contains("$(") || seg.contains('`') || seg.contains("<(") || seg.contains(">(")
}

/// 掃一層的結果。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scan {
    /// 把每個內層換成 [`PLACEHOLDER`] 的原字串（`$(git rev-parse --show-toplevel)/bin/awf` → `$_/bin/awf`）。
    pub flat: String,
    /// 內層的原文（還沒切段、還沒去掉 heredoc 內文）。
    pub inner: Vec<String>,
    /// 引號裡的巢狀太深，沒有掃完。
    pub over: bool,
}

/// 從 `i`（左括號的下一個字元）找配對的右括號；回它的位置，沒有就回結尾。引號裡的括號不算。
fn close_paren(ch: &[char], mut i: usize, depth: usize, over: &mut bool) -> usize {
    let n = ch.len();
    let mut open = 0usize;
    let mut dq = false;
    while i < n {
        let c = ch[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if dq {
            if c == '"' {
                dq = false;
            } else if c == '$' && ch.get(i + 1) == Some(&'(') {
                if depth >= 32 {
                    *over = true;
                    return n;
                }
                i = close_paren(ch, i + 2, depth + 1, over) + 1;
                continue;
            } else if c == '`' {
                i = close_tick(ch, i + 1) + 1;
                continue;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                i += 1;
                while i < n && ch[i] != '\'' {
                    i += 1;
                }
            }
            '"' => dq = true,
            '`' => {
                i = close_tick(ch, i + 1) + 1;
                continue;
            }
            '(' => open += 1,
            ')' => {
                if open == 0 {
                    return i;
                }
                open -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    n
}

/// 從 `i`（反引號的下一個字元）找下一個沒跳脫的反引號；沒有就回結尾。
fn close_tick(ch: &[char], mut i: usize) -> usize {
    while i < ch.len() {
        match ch[i] {
            '\\' => i += 2,
            '`' => return i,
            _ => i += 1,
        }
    }
    ch.len()
}

/// 掃一層：抽出這段字串直接包含的內層指令。
pub fn scan(seg: &str) -> Scan {
    let ch: Vec<char> = seg.chars().collect();
    let n = ch.len();
    let mut out = Scan::default();
    let mut dq = false;
    let mut i = 0;
    while i < n {
        let c = ch[i];
        if c == '\\' {
            out.flat.push(c);
            if let Some(x) = ch.get(i + 1) {
                out.flat.push(*x);
            }
            i += 2;
            continue;
        }
        if !dq && (c == '\'' || (c == '$' && ch.get(i + 1) == Some(&'\''))) {
            // 單引號與 $'…'：內容不會執行，原樣帶過
            let ansi = c == '$';
            if ansi {
                out.flat.push('$');
                i += 1;
            }
            out.flat.push('\'');
            i += 1;
            while i < n {
                let x = ch[i];
                out.flat.push(x);
                i += 1;
                if ansi && x == '\\' {
                    if let Some(y) = ch.get(i) {
                        out.flat.push(*y);
                        i += 1;
                    }
                    continue;
                }
                if x == '\'' {
                    break;
                }
            }
            continue;
        }
        if c == '"' {
            dq = !dq;
            out.flat.push(c);
            i += 1;
            continue;
        }
        let opens_paren = ch.get(i + 1) == Some(&'(') && (c == '$' || (!dq && (c == '<' || c == '>')));
        if opens_paren {
            let end = close_paren(&ch, i + 2, 0, &mut out.over);
            out.inner.push(ch[(i + 2).min(end)..end].iter().collect());
            out.flat.push_str(PLACEHOLDER);
            i = end + 1;
            continue;
        }
        if c == '`' {
            let end = close_tick(&ch, i + 1);
            // 反引號裡的 \`、\\、\$ 少一層跳脫
            let mut body = String::new();
            let mut k = i + 1;
            while k < end {
                if ch[k] == '\\' && k + 1 < end && matches!(ch[k + 1], '`' | '\\' | '$') {
                    body.push(ch[k + 1]);
                    k += 2;
                } else {
                    body.push(ch[k]);
                    k += 1;
                }
            }
            out.inner.push(body);
            out.flat.push_str(PLACEHOLDER);
            i = end + 1;
            continue;
        }
        out.flat.push(c);
        i += 1;
    }
    out
}

/// 一行裡的 heredoc 界線：回（界線, 是否加了引號, 是否 `<<-`）。`<<<`（here-string）不算。
fn heredoc_marks(line: &str) -> Vec<(String, bool, bool)> {
    let ch: Vec<char> = line.chars().collect();
    let n = ch.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < n {
        if !(ch[i] == '<' && ch[i + 1] == '<') {
            i += 1;
            continue;
        }
        let mut k = i + 2;
        if ch.get(k) == Some(&'<') {
            while k < n && ch[k] == '<' {
                k += 1;
            }
            i = k;
            continue;
        }
        let dash = ch.get(k) == Some(&'-');
        if dash {
            k += 1;
        }
        while k < n && (ch[k] == ' ' || ch[k] == '\t') {
            k += 1;
        }
        let mut word = String::new();
        let mut quoted = false;
        while k < n {
            let c = ch[k];
            if c == '\'' || c == '"' {
                quoted = true;
                let q = c;
                k += 1;
                while k < n && ch[k] != q {
                    word.push(ch[k]);
                    k += 1;
                }
                k += 1;
                continue;
            }
            if c == '\\' {
                quoted = true;
                if let Some(x) = ch.get(k + 1) {
                    word.push(*x);
                }
                k += 2;
                continue;
            }
            if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')') {
                break;
            }
            word.push(c);
            k += 1;
        }
        if !word.is_empty() {
            out.push((word, quoted, dash));
        }
        i = k.max(i + 2);
    }
    out
}

/// 去掉 heredoc 的內文。回（剩下的指令文字, 界線沒加引號的內文）。找不到結尾界線時，後面全部當內文。
pub fn split_heredocs(text: &str) -> (String, Vec<String>) {
    if !text.contains("<<") {
        return (text.to_string(), Vec::new());
    }
    let mut cmd: Vec<&str> = Vec::new();
    let mut bodies: Vec<String> = Vec::new();
    let mut pending: std::collections::VecDeque<(String, bool, bool)> = std::collections::VecDeque::new();
    let mut body: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if let Some((mark, quoted, dash)) = pending.front() {
            let probe = if *dash { line.trim_start_matches('\t') } else { line };
            if probe.trim_end_matches('\r') == mark {
                if !quoted {
                    bodies.push(body.join("\n"));
                }
                body.clear();
                pending.pop_front();
            } else {
                body.push(line);
            }
            continue;
        }
        cmd.push(line);
        pending.extend(heredoc_marks(line));
    }
    if let Some((_, quoted, _)) = pending.front() {
        if !quoted {
            bodies.push(body.join("\n"));
        }
    }
    (cmd.join("\n"), bodies)
}

fn collect(seg: &str, depth: usize, out: &mut Vec<String>, over: &mut bool) {
    if !has_subst(seg) {
        return;
    }
    let sc = scan(seg);
    *over |= sc.over;
    let mut raw = sc.inner;
    let mut k = 0;
    // heredoc 內文裡的指令替換（界線沒加引號）接在同一層後面
    while k < raw.len() {
        let (cmd, bodies) = split_heredocs(&raw[k]);
        for b in bodies {
            if has_subst(&b) {
                let inner = scan(&b);
                *over |= inner.over;
                raw.extend(inner.inner);
            }
        }
        if raw.len() > MAX_INNER {
            *over = true;
            return;
        }
        raw[k] = cmd;
        k += 1;
    }
    for cmd in raw {
        for s in segments(&cmd) {
            if out.len() >= MAX_INNER {
                *over = true;
                return;
            }
            let deeper = has_subst(&s);
            out.push(s.clone());
            if deeper {
                if depth + 1 >= MAX_DEPTH {
                    *over = true;
                } else {
                    collect(&s, depth + 1, out, over);
                }
            }
        }
    }
}

/// 這一段裡所有內層指令（各層都展開）切成的段落。第二個值＝超過上限、沒有展開完。
pub fn inner_segments(seg: &str) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut over = false;
    collect(seg, 0, &mut out, &mut over);
    (out, over)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner(s: &str) -> Vec<String> {
        inner_segments(s).0
    }

    #[test]
    fn one_level() {
        assert_eq!(scan("X=$(awf save-run approve r)").inner, vec!["awf save-run approve r"]);
        assert_eq!(scan("X=$(awf save-run approve r)").flat, "X=$_");
        assert_eq!(scan("echo \"a $(b c) d\" `e f`").inner, vec!["b c", "e f"]);
        assert_eq!(scan("echo '$(not run)' $'\\'$(nor this)'").inner, Vec::<String>::new());
        assert_eq!(scan("cat <(a b) > >(c d)").inner, vec!["a b", "c d"]);
        assert_eq!(scan("echo \"<(text)\"").inner, Vec::<String>::new());
        assert_eq!(scan("$(git rev-parse --show-toplevel)/bin/awf save-run approve r").flat, "$_/bin/awf save-run approve r");
        assert_eq!(scan("echo \"$(echo \")\") tail\"").inner, vec!["echo \")\""]);
        assert_eq!(scan("echo $( (a); b ) c").inner, vec![" (a); b "]);
        assert_eq!(scan("echo `a \\`b\\` c`").inner, vec!["a `b` c"]);
        assert_eq!(scan("echo $(unterminated").inner, vec!["unterminated"]);
    }

    #[test]
    fn nested_and_split() {
        assert_eq!(inner("echo \"$(cd x && awf save-run approve r)\""), vec!["cd x", "awf save-run approve r"]);
        assert_eq!(inner("echo $(echo $(echo `awf build`))"), vec!["echo $(echo `awf build`)", "echo `awf build`", "awf build"]);
        assert_eq!(inner("ls"), Vec::<String>::new());
        let deep = "echo $(a $(b $(c $(d $(e)))))";
        let (segs, over) = inner_segments(deep);
        assert!(over, "{segs:?}");
        let many = format!("echo {}", "$(x) ".repeat(MAX_INNER + 5));
        assert!(inner_segments(&many).1);
    }

    #[test]
    fn heredocs() {
        let commit = "git commit -m \"$(cat <<'EOF'\nfix: awf save-run approve r 的說明\ncat .env\nEOF\n)\"";
        assert_eq!(inner(commit), vec!["cat <<'EOF'"]);
        // 界線沒加引號：內文裡的指令替換會執行
        let open = "X=$(cat <<EOF\nawf build\n$(awf save-run approve r)\nEOF\n)";
        assert_eq!(inner(open), vec!["cat <<EOF", "awf save-run approve r"]);
        let (cmd, bodies) = split_heredocs("a <<-X\n\tb\n\tX\nc <<< d\ne");
        assert_eq!(cmd, "a <<-X\nc <<< d\ne");
        assert_eq!(bodies, vec!["\tb"]);
        assert_eq!(split_heredocs("a <<E\nb"), ("a <<E".to_string(), vec!["b".to_string()]));
    }
}
