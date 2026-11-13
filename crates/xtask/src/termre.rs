//! 禁用清單（`tests/scrub_terms*.txt`）與文件檢查用的正規式方言：把它改寫成 Rust（`fancy_regex`）讀起來意思相同的寫法。
//!
//! 這些清單從 0.x 沿用下來，當時由 Python 的 `re` 解讀；使用者自己的 `scrub_terms.local.txt` 也是照那套語法寫的，
//! 所以語法維持不變（下面說的「Python」指的就是這套語法），repo 裡不需要 Python。
//!
//! 逐條對照過的差異：
//!   - `\s`、`\S`：Python 的空白比 Rust 多 U+001C～U+001F，換成同範圍的字元類別。
//!   - `\w`、`\W`、`\b`、`\B`：Python 的 `\w` 是字母（L）、數字字元（N）與底線；Rust 的 `\w` 還多了結合符號、
//!     連接標點等。換成 `[\p{L}\p{N}_]`，`\b`／`\B` 用前後查看展開。
//!   - `\Z`：Python 是「字串結尾」，等於 Rust 的 `\z`。
//!   - 字元類別裡的 `[`：Python 當一般字元，Rust 當巢狀類別，所以補上跳脫；`&&`、`~~` 在 Rust 是集合運算，同樣跳脫；
//!     `--` 兩邊解讀差太多，直接拒絕。類別裡的 `\b` 是退格字元。
//!   - `\d`（都是 Nd）、`.`（都不含 `\n`）、`^`／`$` 在 `(?m)` 下的行為兩邊相同，不用改。
//!   - `$`（沒有 `(?m)`）：Python 也會配到結尾換行之前，Rust 只配結尾。呼叫端的文字都是切好的行（沒有換行），
//!     路徑名另外由 [`Pat::search_name`] 補上「去掉結尾換行再比一次」。
//!
//!   - `{`：Python 只把 `{n}`、`{n,}`、`{,m}`、`{n,m}`（中間沒有空白）當次數，其他都是一般字元；Rust 會把
//!     `{ 2}` 也當次數，所以不是次數的 `{` 補上跳脫。
//!   - 反斜線接標點（`\<`、`\>`、`\.` 等）：Python 一律是那個字元本身，Rust 的 `\<`、`\>` 是字的邊界。
//!     全部改寫成 `\x{…}`，不靠引擎各自的規則。
//!   - 八進位（`\0`、`\012`）：改寫成 `\x{…}`。`\1`～`\9` 是群組參照；後面還接數字的（`\10`）兩邊讀法不同，拒絕。
//!
//! Rust 編不過的寫法（`(?a)`、`\N{…}` 等）與上面拒絕的寫法一律回錯誤，由呼叫端明確報錯，不會默默略過。
//! 反過來，Python 不接受而 Rust 接受的寫法（`(?<名>…)`、`\x{41}` 等）這裡不全擋：0.x 的 Python 版遇到會直接當掉。

use awf_core::kb::pyish::WS_CLASS;

const W: &str = r"\p{L}\p{N}_";

/// `\s` 的字元類別內容；空白字元寫成跳脫，`(?x)` 模式下才不會被當成排版用的空白丟掉。
fn ws() -> String {
    WS_CLASS.replace(' ', r"\x20")
}

fn is_octal(c: Option<&char>) -> bool {
    matches!(c, Some('0'..='7'))
}

/// 反斜線後面是數字、其他字母或標點時的改寫。`i` 指向下一個還沒讀的字元，吃掉更多字元時往後移。
fn escape_other(n: char, chars: &[char], i: &mut usize, in_class: bool, out: &mut String) -> Result<(), String> {
    if n.is_ascii_digit() {
        let mut digits = String::from(n);
        if n == '0' {
            while digits.len() < 3 && is_octal(chars.get(*i)) {
                digits.push(chars.get(*i).copied().unwrap_or('0'));
                *i += 1;
            }
        } else if is_octal(Some(&n)) && is_octal(chars.get(*i)) && is_octal(chars.get(*i + 1)) {
            digits.extend(chars.get(*i..*i + 2).unwrap_or_default());
            *i += 2;
        } else if in_class {
            return Err("字元類別裡不能有群組參照".into());
        } else if chars.get(*i).is_some_and(char::is_ascii_digit) {
            return Err("兩位數的群組參照，兩種實作的讀法不同".into());
        } else {
            out.push('\\');
            out.push(n);
            return Ok(());
        }
        let v = u32::from_str_radix(&digits, 8).map_err(|_| "八進位跳脫讀不懂".to_string())?;
        if v > 0o377 {
            return Err("八進位跳脫超出範圍".into());
        }
        out.push_str(&format!(r"\x{{{v:x}}}"));
    } else if n.is_ascii_alphabetic() {
        let ok = if in_class { "dDafnrtvxuU" } else { "AdDafnrtvxuU" };
        if !ok.contains(n) {
            return Err(format!("Python 的 re 沒有 \\{n} 這個跳脫"));
        }
        out.push('\\');
        out.push(n);
    } else {
        out.push_str(&format!(r"\x{{{:x}}}", n as u32));
    }
    Ok(())
}

/// `chars[i..]`（`{` 之後）是不是 Python 認的次數寫法：`n}`、`n,}`、`,m}`、`n,m}`。
fn is_repeat(chars: &[char], i: usize) -> bool {
    let mut j = i;
    let digits = |j: &mut usize| {
        let start = *j;
        while chars.get(*j).is_some_and(char::is_ascii_digit) {
            *j += 1;
        }
        *j > start
    };
    let lo = digits(&mut j);
    if chars.get(j) == Some(&',') {
        j += 1;
        digits(&mut j);
        return chars.get(j) == Some(&'}');
    }
    lo && chars.get(j) == Some(&'}')
}

/// 改寫後的正規式，以及原式有沒有 `\b` 或 `\B`。`relaxed` 是 true 時把它們拿掉（配到的只會更多，用來先粗篩）。
fn translate(pat: &str, relaxed: bool) -> Result<(String, bool), String> {
    let chars: Vec<char> = pat.chars().collect();
    let mut out = String::with_capacity(pat.len() + 16);
    let mut boundary = false;
    let mut in_class = false;
    let ws = ws();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        i += 1;
        if in_class {
            match (c, next) {
                ('\\', None) => return Err("結尾是落單的反斜線".into()),
                ('\\', Some(n)) => {
                    i += 1;
                    match n {
                        's' => out.push_str(&ws),
                        'S' => out.push_str(&format!("[^{ws}]")),
                        'w' => out.push_str(W),
                        'W' => out.push_str(&format!("[^{W}]")),
                        'b' => out.push_str(r"\x08"),
                        _ => escape_other(n, &chars, &mut i, true, &mut out)?,
                    }
                }
                ('[', _) => out.push_str(r"\["),
                (']', _) => {
                    out.push(']');
                    in_class = false;
                }
                ('&', Some('&')) => {
                    out.push_str(r"\&\&");
                    i += 1;
                }
                ('~', Some('~')) => {
                    out.push_str(r"\~\~");
                    i += 1;
                }
                ('-', Some('-')) => return Err("字元類別裡有連續兩個減號，兩種實作的解讀不同".into()),
                _ => out.push(c),
            }
            continue;
        }
        match (c, next) {
            ('\\', None) => return Err("結尾是落單的反斜線".into()),
            ('\\', Some(n)) => {
                i += 1;
                match n {
                    's' => out.push_str(&format!("[{ws}]")),
                    'S' => out.push_str(&format!("[^{ws}]")),
                    'w' => out.push_str(&format!("[{W}]")),
                    'W' => out.push_str(&format!("[^{W}]")),
                    'b' | 'B' => {
                        boundary = true;
                        if relaxed {
                            out.push_str("(?:)");
                        } else if n == 'b' {
                            out.push_str(&format!("(?:(?<=[{W}])(?![{W}])|(?<![{W}])(?=[{W}]))"));
                        } else {
                            out.push_str(&format!("(?:(?<=[{W}])(?=[{W}])|(?<![{W}])(?![{W}]))"));
                        }
                    }
                    'Z' => out.push_str(r"\z"),
                    _ => escape_other(n, &chars, &mut i, false, &mut out)?,
                }
            }
            ('{', _) if !is_repeat(&chars, i) => out.push_str(r"\{"),
            ('[', _) => {
                out.push('[');
                in_class = true;
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    out.push_str(r"\]");
                    i += 1;
                }
            }
            _ => out.push(c),
        }
    }
    if in_class {
        return Err("字元類別沒有結尾的 ]".into());
    }
    Ok((out, boundary))
}

/// 一條改寫好的正規式。
pub struct Pat {
    exact: fancy_regex::Regex,
    /// 不需要回溯引擎就能跑的版本：`fast_exact` 是 true 時和 `exact` 等價，否則只是「配到的不會更少」的粗篩。
    fast: Option<regex::Regex>,
    fast_exact: bool,
}

/// 照 Python `re.compile(pat)` 的意思編譯；編不過回錯誤說明（不含原式內容）。
pub fn compile(pat: &str) -> Result<Pat, String> {
    let (src, boundary) = translate(pat, false)?;
    let exact = fancy_regex::RegexBuilder::new(&src).backtrack_limit(usize::MAX).build()
        .map_err(|_| "這個寫法 Rust 的正規式引擎不接受".to_string())?;
    let fast = translate(pat, true).ok().and_then(|(s, _)| regex::Regex::new(&s).ok());
    Ok(Pat { exact, fast, fast_exact: !boundary })
}

impl Pat {
    /// `pat.search(text) is not None`
    pub fn search(&self, text: &str) -> Result<bool, String> {
        if let Some(fast) = &self.fast {
            if !fast.is_match(text) {
                return Ok(false);
            }
            if self.fast_exact {
                return Ok(true);
            }
        }
        self.exact.is_match(text).map_err(|e| format!("正規式執行失敗：{e}"))
    }

    /// 同 [`Pat::search`]，但文字結尾有換行時，去掉換行再比一次（Python 的 `$` 會配到結尾換行之前）。
    pub fn search_name(&self, text: &str) -> Result<bool, String> {
        if self.search(text)? {
            return Ok(true);
        }
        match text.strip_suffix('\n') {
            Some(t) => self.search(t),
            None => Ok(false),
        }
    }

    /// `pat.finditer(text)`：每筆是（整段配到的起點、整段、第一個群組）。
    pub fn find_iter<'t>(&self, text: &'t str) -> Result<Vec<(usize, &'t str, &'t str)>, String> {
        let mut out = Vec::new();
        for c in self.exact.captures_iter(text) {
            let c = c.map_err(|e| format!("正規式執行失敗：{e}"))?;
            let Some(all) = c.get(0) else { continue };
            out.push((all.start(), all.as_str(), c.get(1).map(|m| m.as_str()).unwrap_or_default()));
        }
        Ok(out)
    }
}
