//! 自訂 workflow 腳本的靜態檢查與組裝（`awf next --script` 用）。
//! 由 `scaffold/.claude/teams/tools/awf_script.py` 移植；錯誤訊息與組裝結果是對跑基準的一部分
//! （`tests/parity/tools.expected.json`）。
//!
//! 檢查的規則（每一條都對應一種繞過執行核心的寫法）：
//!   1. 不能直接用 agent()、tryAgent()、workflow()：派工一律走 awfAgent 或 pattern。
//!   2. agentType 要寫成 `NS + '<角色>'` 或完整的字串，而且是登記過的角色；不能派 dev-merge-steward。
//!   3. 迴圈要有上限：只准 `for (… of …)`、`for (… in …)` 與有條件、有遞增的 `for (;;)`；`while`、`do` 不准。
//!   4. 每個流程層級的 return 都要帶 `trace: awfTrace()`。
//!   5. 不能重新定義或碰核心的內部，不能用 eval、Function、import、require、globalThis。
//!   6. 不能用正規式字面值、字串外的反斜線、constructor／__proto__／prototype。
//!
//! 這是事前檢查；真正的把關在 [`assemble`]（把內容包成函式、遮蔽名字、外層覆寫回傳）、執行核心與閘門。
//!
//! 和 Python `re` 對齊的地方：位置一律以字元（code point）計；`\w` 是字母、數字與底線（`\p{L}\p{N}_`，
//! 不用 Rust 的 `\w`，它多了結合符號等，會讓「前後不是識別字」的判斷變鬆）；`\s` 含 U+001C～U+001F；
//! 有 lookaround 與回溯參照的樣式用 fancy-regex（回溯不設上限）。樣式引擎出錯時一律回報不合格，不會默默放行。

use crate::kb::pyish::{is_space, WS_CLASS};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub const USAGE: &str = "自訂 workflow 腳本的靜態檢查與組裝（awf next --script 用）。

標準的 sdlc-*.js 不合用時（例如要照自己的波次排工），主 session 可以為 design、build、deliver 段寫一支自己的腳本。
這支工具先檢查、再把執行核心接上去。檢查不過就不產生腳本。

腳本的寫法：
  - 開頭是 `export const meta = { name, description, … }`（純字面值）。
  - 之後直接寫流程。awf 會在 meta 後面接上這些，腳本直接用、不要自己宣告：
      A（這一段的參數）、NS、METHODOLOGY、MP、RECIPE、
      awfAgent（派工）、awfTrace、awfPrefix、awfConstraints、layersOf 等片段函式、
      六個 pattern：awfFanOut、awfClassifyAct、awfAdversarial、awfGenerateFilter、awfTournament、awfLoop。
  - 信封的 schema 用標記：`const FINDINGS = /*@schema:findings*/ null /*@end*/`。
  - 每個結束點都要 `return { stage: '<段>', …, recipe: RECIPE, trace: awfTrace() }`。

檢查的規則（每一條都對應一種繞過執行核心的寫法）：
  1. 不能直接用 agent()、tryAgent()、workflow()：派工一律走 awfAgent 或 pattern。
  2. agentType 要寫成 `NS + '<角色>'` 或完整的字串，而且是登記過的角色（模版角色，或 guard.config.json 的 extension_agents）。
     不能派 dev-merge-steward。
  3. 迴圈要有上限：只准 `for (… of …)`、`for (… in …)` 與有條件、有遞增的 `for (;;)`；`while`、`do` 不准（改用 awfLoop）。
  4. 每個流程層級的 return 都要帶 `trace: awfTrace()`。
  5. 不能重新定義或碰核心的內部（awfState、awfKernel、awfInit、awfKernelInit…），不能用 eval、Function、import、require、globalThis。
  6. 不能用正規式字面值、字串外的反斜線、constructor／__proto__／prototype（遮罩器看不懂或可以拿來繞過）。

這是事前檢查，幫忙早點發現寫錯；它擋不住所有寫法。真正的把關在三個地方：
  - 組裝時把腳本內容包成函式（assemble）：agent、tryAgent、awfState 等名字被參數遮蔽，內容裡拿不到；
    回傳的 stage、recipe、trace 由外層覆寫，trace 一律取自執行核心。
  - 執行核心在執行時再擋一次（角色白名單、派工數上限）。
  - 閘門驗的是產出：awf save-run stage 的條件和標準腳本完全一樣，而且自訂腳本跑的段不能政策核准，一律交人。

`awf script --selftest`。
";

pub const STAGES: [&str; 3] = ["design", "build", "deliver"];
/// 自訂腳本可以派的模版角色：check_agents 的 CORE 扣掉 dev-merge-steward（合併是信任邊界，只有 sdlc-merge 能派）。
pub const ROLES: [&str; 13] = [
    "tpm-planner", "tpm-researcher", "tpm-reviewer", "tpm-skeptic", "tpm-writer",
    "dev-spec-author", "dev-acceptance-tester", "dev-implementer", "dev-integrator", "dev-verifier",
    "dev-design-reviewer", "dev-security-reviewer", "dev-release-watcher",
];
pub const SNIPPETS: [&str; 5] = ["recipe", "challenge", "layers", "kernel", "patterns"];
pub const PREAMBLE_NAMES: [&str; 6] = ["NS", "A", "METHODOLOGY", "MP", "R", "RECIPE"];
pub const FORBIDDEN: [(&str, &str); 13] = [
    ("agent", "不能直接用 agent()：派工一律走 awfAgent 或 pattern"),
    ("tryAgent", "不能直接用 tryAgent()：派工一律走 awfAgent 或 pattern（才有模型分級與派工數上限）"),
    ("workflow", "不能呼叫別的 workflow"),
    ("awfState", "不能碰執行核心的內部狀態（awfState）"),
    ("awfKernel", "不能碰執行核心的內部狀態（awfKernel）"),
    ("awfInit", "不要自己呼叫 awfInit：awf 已經接好了"),
    ("awfKernelInit", "不要自己呼叫 awfKernelInit：awf 已經接好了（上限與角色白名單由它帶）"),
    ("awfSetModifiers", "不能改 modifiers（只會由 run.json 鎖定的 recipe 帶入）"),
    ("eval", "不能用 eval"),
    ("Function", "不能用 Function 建構式"),
    ("globalThis", "不能用 globalThis"),
    ("require", "不能用 require"),
    ("import", "不能用 import"),
];
pub const CONTROL: [&str; 6] = ["if", "for", "while", "switch", "catch", "with"];
pub const SHADOWED: [&str; 10] =
    ["agent", "tryAgent", "workflow", "awfState", "awfKernel", "awfInit", "awfKernelInit", "awfSetModifiers", "awfSetModelPlan", "awfDispatch"];

/// 腳本讀不懂（沒收尾、開頭不是 meta…）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ------------------------------------------------------------------ 和 Python re 對齊的小工具

/// Python `re` 的 `\w`（放在字元類別裡）。
const W: &str = r"\p{L}\p{N}_";

/// Python `re` 的 `\w`：字母、數字（含 No、Nl）與底線。
pub(crate) fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    match RE.get_or_init(|| Regex::new(r"^[\p{L}\p{N}]$").ok()) {
        Some(re) => re.is_match(c.encode_utf8(&mut [0u8; 4])),
        // 編不過就當成不是字：「前後不是識別字」比較容易成立，只會多擋
        None => false,
    }
}

/// `[\w$]`
fn is_ident(c: char) -> bool {
    is_word(c) || c == '$'
}

/// 字元陣列加上對應的字串：樣式在字串上比，位置換回字元位置。
struct Txt {
    s: String,
    /// 每個字元的位元組起點，最後多一格是總長。
    at: Vec<usize>,
}

impl Txt {
    fn new(chars: &[char]) -> Txt {
        let mut s = String::with_capacity(chars.len());
        let mut at = Vec::with_capacity(chars.len() + 1);
        for c in chars {
            at.push(s.len());
            s.push(*c);
        }
        at.push(s.len());
        Txt { s, at }
    }

    fn b(&self, ci: usize) -> usize {
        self.at.get(ci).copied().unwrap_or(self.s.len())
    }

    /// `s[a:b]`（字元位置）
    fn slice(&self, a: usize, b: usize) -> &str {
        let (x, y) = (self.b(a), self.b(b));
        if x >= y { "" } else { self.s.get(x..y).unwrap_or("") }
    }

    /// `s[a:]`
    fn from(&self, a: usize) -> &str {
        self.s.get(self.b(a)..).unwrap_or("")
    }

    /// 位元組位置 → 字元位置
    fn c(&self, byte: usize) -> usize {
        self.at.partition_point(|x| *x < byte)
    }
}

type Fancy = Option<fancy_regex::Regex>;

fn fancy(src: String) -> Fancy {
    fancy_regex::RegexBuilder::new(&src).backtrack_limit(usize::MAX).build().ok()
}

/// 不前接識別字或點：`(?<![\w$.])`
fn nb() -> String {
    format!(r"(?<![{W}$.])")
}

/// 樣式引擎出錯（編不過或執行失敗）：一律算不合格。
struct Fault;

/// `re.finditer`：每筆是（整體起點, 整體終點, 第 1 組的範圍），位元組位置。
fn find_all(re: &Fancy, hay: &str) -> Result<Vec<(usize, usize, Option<(usize, usize)>)>, Fault> {
    let re = re.as_ref().ok_or(Fault)?;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= hay.len() {
        let Some(caps) = re.captures_from_pos(hay, pos).map_err(|_| Fault)? else { break };
        let Some(m) = caps.get(0) else { break };
        out.push((m.start(), m.end(), caps.get(1).map(|g| (g.start(), g.end()))));
        pos = if m.end() > m.start() {
            m.end()
        } else {
            m.end() + hay.get(m.end()..).and_then(|r| r.chars().next()).map(char::len_utf8).unwrap_or(1)
        };
    }
    Ok(out)
}

fn found(re: &Fancy, hay: &str) -> Result<bool, Fault> {
    re.as_ref().ok_or(Fault)?.is_match(hay).map_err(|_| Fault)
}

/// `(?<![\w$.])(名字一|名字二)` 後面不接識別字的所有位置：回（起點, 終點, 第幾個名字）。
/// `dollar_ends` 是 false 時對應 `\b`（後面是 `$` 也算成立），true 時對應 `(?![\w$])`。
fn ident_hits(body: &[char], names: &[&str], dollar_ends: bool) -> Vec<(usize, usize, usize)> {
    let names: Vec<Vec<char>> = names.iter().map(|n| n.chars().collect()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let prev_ok = i == 0 || body.get(i - 1).is_none_or(|p| !(is_ident(*p) || *p == '.'));
        let mut hit = None;
        if prev_ok {
            for (k, name) in names.iter().enumerate() {
                let end = i + name.len();
                if body.get(i..end).is_some_and(|s| s == name.as_slice()) {
                    let next_ok = body.get(end).is_none_or(|c| if dollar_ends { !is_ident(*c) } else { !is_word(*c) });
                    if next_ok {
                        hit = Some((i, end, k));
                        break;
                    }
                }
            }
        }
        match hit {
            Some(h) => {
                out.push(h);
                i = h.1;
            }
            None => i += 1,
        }
    }
    out
}

fn line_of(text: &[char], pos: usize) -> usize {
    text.iter().take(pos).filter(|c| **c == '\n').count() + 1
}

// ------------------------------------------------------------------ 遮罩

struct Masker<'a> {
    text: &'a [char],
    out: Vec<char>,
    /// 'T'＝樣板字串裡；'C'＝樣板字串的 ${ } 裡
    stack: Vec<char>,
    depth: i64,
    /// 進 ${ } 時的大括號深度
    cdepth: Vec<i64>,
}

impl Masker<'_> {
    fn blank(&mut self, a: usize, b: usize) {
        for k in a..b.min(self.out.len()) {
            if let Some(c) = self.out.get_mut(k) {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        }
    }

    /// 從反引號後面開始掃樣板字串的文字；遇到 ${ 回到程式碼模式。回傳下一個要處理的位置。
    fn template(&mut self, start: usize) -> Result<usize, ScriptError> {
        let n = self.text.len();
        let mut j = start;
        while j < n {
            let c = self.text.get(j).copied().unwrap_or(' ');
            if c == '\\' {
                self.blank(j, (j + 2).min(n));
                j += 2;
                continue;
            }
            if c == '`' {
                self.stack.pop();
                return Ok(j + 1);
            }
            if c == '$' && self.text.get(j + 1) == Some(&'{') {
                self.stack.push('C');
                self.cdepth.push(self.depth);
                self.depth += 1;
                return Ok(j + 2);
            }
            self.blank(j, j + 1);
            j += 1;
        }
        Err(ScriptError("樣板字串沒有收尾".into()))
    }

    fn run(mut self) -> Result<Vec<char>, ScriptError> {
        let text = self.text;
        let n = text.len();
        let ch = |i: usize| text.get(i).copied();
        let mut i = 0;
        while i < n {
            let c = ch(i).unwrap_or(' ');
            let next = ch(i + 1);
            if c == '/' && next == Some('/') {
                let j = (i..n).find(|k| ch(*k) == Some('\n')).unwrap_or(n);
                self.blank(i, j);
                i = j;
            } else if c == '/' && next == Some('*') {
                let Some(j) = (i + 2..n).find(|k| ch(*k) == Some('*') && ch(*k + 1) == Some('/')) else {
                    return Err(ScriptError("區塊註解沒有收尾".into()));
                };
                self.blank(i, j + 2);
                i = j + 2;
            } else if c == '\'' || c == '"' {
                let mut j = i + 1;
                while j < n && ch(j) != Some(c) {
                    if ch(j) == Some('\n') {
                        return Err(ScriptError(format!(
                            "第 {} 行的字串沒有收尾（腳本裡的正規式字面值如果含引號，請改用 new RegExp 以外的寫法或拆開）",
                            line_of(text, i)
                        )));
                    }
                    j += if ch(j) == Some('\\') { 2 } else { 1 };
                }
                if j >= n {
                    return Err(ScriptError("字串沒有收尾".into()));
                }
                self.blank(i + 1, j);
                i = j + 1;
            } else if c == '`' {
                self.stack.push('T');
                i = self.template(i + 1)?;
            } else if c == '/' {
                // 除號前面一定是值（識別字、數字、右括號）；其他位置的 / 是正規式字面值，裡面的引號與 /* 會騙過遮罩，所以不准
                let mut j = i;
                while j > 0 && matches!(ch(j - 1), Some(' ' | '\t')) {
                    j -= 1;
                }
                // text[..j] 是去掉行內空白後的前文；prev 是它的最後一個字
                let prev = if j > 0 { ch(j - 1).unwrap_or('\n') } else { '\n' };
                let mut k = j;
                while k > 0 && ch(k - 1).is_some_and(is_ident) {
                    k -= 1;
                }
                let word: String = text.get(k..j).unwrap_or_default().iter().collect();
                let value_before = is_ident(prev) || prev == ')' || prev == ']';
                let keyword = ["return", "typeof", "case", "in", "of", "void", "delete", "do", "else", "yield", "await"].contains(&word.as_str());
                if !value_before || keyword {
                    return Err(ScriptError(format!(
                        "第 {} 行有正規式字面值：自訂腳本不能用 /…/ 的寫法（改用字串方法，或 new RegExp('…')）",
                        line_of(text, i)
                    )));
                }
                i += 1;
            } else if c == '\\' {
                return Err(ScriptError(format!("第 {} 行在字串外面有反斜線（例如用 \\u 跳脫拼出識別字），不准", line_of(text, i))));
            } else if c == '{' {
                self.depth += 1;
                i += 1;
            } else if c == '}' {
                self.depth -= 1;
                if self.stack.last() == Some(&'C') && self.cdepth.last() == Some(&self.depth) {
                    self.stack.pop();
                    self.cdepth.pop();
                    i = self.template(i + 1)?;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        if !self.stack.is_empty() {
            return Err(ScriptError("樣板字串沒有收尾".into()));
        }
        if self.depth != 0 {
            return Err(ScriptError("大括號沒有成對".into()));
        }
        Ok(self.out)
    }
}

fn mask_chars(text: &[char]) -> Result<Vec<char>, ScriptError> {
    Masker { text, out: text.to_vec(), stack: Vec::new(), depth: 0, cdepth: Vec::new() }.run()
}

/// 把字串與註解的內容換成空白（字數與換行不變），樣板字串裡 ${…} 的程式碼保留。讀不懂（沒收尾）就回 ScriptError。
pub fn mask(text: &str) -> Result<String, ScriptError> {
    let chars: Vec<char> = text.chars().collect();
    Ok(mask_chars(&chars)?.into_iter().collect())
}

/// m[i] 是開括號，回傳對應的閉括號位置；找不到回 None。
fn match_close(m: &[char], i: usize) -> Option<usize> {
    let pair = |c: char| match c {
        '(' => Some(')'),
        '{' => Some('}'),
        '[' => Some(']'),
        _ => None,
    };
    let mut want = vec![pair(m.get(i).copied()?)?];
    let mut j = i + 1;
    while j < m.len() && !want.is_empty() {
        let c = m.get(j).copied().unwrap_or(' ');
        if let Some(p) = pair(c) {
            want.push(p);
        } else if matches!(c, ')' | '}' | ']') && want.pop() != Some(c) {
            return None;
        }
        j += 1;
    }
    if want.is_empty() { Some(j - 1) } else { None }
}

fn meta_end_chars(m: &[char]) -> Result<usize, ScriptError> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(&format!(r"^[{WS_CLASS}]*export[{WS_CLASS}]+const[{WS_CLASS}]+meta[{WS_CLASS}]*=[{WS_CLASS}]*\{{")).ok());
    let t = Txt::new(m);
    let Some(mm) = re.as_ref().and_then(|r| r.find(&t.s)) else {
        return Err(ScriptError("腳本要以 export const meta = {…} 開頭".into()));
    };
    match match_close(m, t.c(mm.end()).saturating_sub(1)) {
        Some(end) => Ok(end + 1),
        None => Err(ScriptError("meta 的大括號沒有成對".into())),
    }
}

/// `export const meta = {…}` 結束的位置（閉大括號之後；以字元計）。開頭不是它就回 ScriptError。`m` 是遮罩過的腳本。
pub fn meta_end(text: &str, m: &str) -> Result<usize, ScriptError> {
    let _ = text;
    let chars: Vec<char> = m.chars().collect();
    meta_end_chars(&chars)
}

/// m[:i] 最後一個非空白的 token（一個識別字或一個符號）與它的起點。
fn prev_token(m: &[char], i: usize) -> (String, Option<usize>) {
    let mut j = i;
    while j > 0 && m.get(j - 1).is_some_and(|c| is_space(*c)) {
        j -= 1;
    }
    if j == 0 {
        return (String::new(), None);
    }
    let last = j - 1;
    let c = m.get(last).copied().unwrap_or(' ');
    if is_ident(c) {
        let mut k = last;
        while k > 0 && m.get(k - 1).is_some_and(|x| is_ident(*x)) {
            k -= 1;
        }
        return (m.get(k..=last).unwrap_or_default().iter().collect(), Some(k));
    }
    if c == '>' && last > 0 && m.get(last - 1) == Some(&'=') {
        return ("=>".into(), Some(last - 1));
    }
    (c.to_string(), Some(last))
}

/// 函式本體的大括號範圍。`=> {`，或 `) {` 而且那組小括號前面不是 if／for／while／switch／catch。
fn function_spans(m: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for (i, c) in m.iter().enumerate() {
        if *c != '{' {
            continue;
        }
        let (tok, at) = prev_token(m, i);
        let mut is_fn = tok == "=>";
        if tok == ")" {
            let mut depth = 0i64;
            let mut open = None;
            let mut j = at.unwrap_or(0) as i64;
            while j >= 0 {
                match m.get(j as usize) {
                    Some(')') => depth += 1,
                    Some('(') => {
                        depth -= 1;
                        if depth == 0 {
                            open = Some(j as usize);
                            break;
                        }
                    }
                    _ => {}
                }
                j -= 1;
            }
            let before = match open {
                Some(j) => prev_token(m, j).0,
                None => String::new(),
            };
            is_fn = !CONTROL.contains(&before.as_str());
        }
        if is_fn {
            if let Some(end) = match_close(m, i).filter(|e| *e > 0) {
                spans.push((i, end));
            }
        }
    }
    spans
}

struct Patterns {
    getter: Fancy,
    redefine: Fancy,
    assign: Fancy,
    lit: Fancy,
    for_head: Option<Regex>,
    trace: Fancy,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let (s, nb) = (WS_CLASS, nb());
        Patterns {
            getter: fancy(format!(r#"{nb}(?:get|set)[{s}]+['"\[{W}$]+[{s}]*\("#)),
            redefine: fancy(format!(r"{nb}(?:function[{s}]*\*?[{s}]*|(?:const|let|var)[{s}]+)([{W}$]+)")),
            assign: fancy(format!(r"{nb}([{W}$]+)[{s}]*(?:=(?![=>])|\+=|-=|\|\|=|&&=|\?\?=)")),
            lit: fancy(format!(r#"^[{s}]*:[{s}]*(NS[{s}]*\+[{s}]*)?(['"])([^'"\n]*)\2"#)),
            for_head: Regex::new(&format!(r"^[{s}]*(?:const|let|var)[{s}]+[{W}$\[\]{{}},{s}.:]+?[{s}]+(?:of|in)[{s}]+[^{s}]")).ok(),
            trace: fancy(format!(r"{nb}trace[{s}]*:[{s}]*awfTrace[{s}]*\([{s}]*\)")),
        }
    })
}

/// 回傳不合格的理由（空清單＝合格）。
pub fn check(text: &str, allowed_types: &BTreeSet<String>, ns: &str, reserved: &BTreeSet<String>) -> Vec<String> {
    if text.contains("/*@snippet") || text.contains("/*@methodology") {
        return vec!["腳本不能自己放片段或 profile 的標記（awf 會接上）；只有 /*@schema:<名>*/ null /*@end*/ 可以用".into()];
    }
    if text.contains("{{") {
        return vec!["腳本不能有雙大括號的佔位符（要用的值從 A、NS、METHODOLOGY 取）".into()];
    }
    let t: Vec<char> = text.chars().collect();
    let (m, start) = match mask_chars(&t).and_then(|m| meta_end_chars(&m).map(|s| (m, s))) {
        Ok(x) => x,
        Err(e) => return vec![format!("讀不懂這支腳本：{e}")],
    };
    let mut errs: Vec<String> = Vec::new();
    match check_body(&t, &m, start, allowed_types, ns, reserved, &mut errs) {
        Ok(()) => errs,
        Err(Fault) => {
            errs.push("靜態檢查的樣式引擎出錯，無法確認這支腳本合格（請回報）".into());
            errs
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_body(t: &[char], m: &[char], start: usize, allowed_types: &BTreeSet<String>, ns: &str, reserved: &BTreeSet<String>,
              errs: &mut Vec<String>) -> Result<(), Fault> {
    let pats = patterns();
    let body = m.get(start..).unwrap_or_default();
    let off = start;
    let tt = Txt::new(t);
    let mt = Txt::new(m);
    let bt = Txt::new(body);
    let raw_tail = tt.from(start);
    let at = |pos: usize| format!("第 {} 行", line_of(t, off + pos));

    if !ident_hits(body, &["export"], false).is_empty() {
        errs.push("除了開頭的 meta，不能再有 export".into());
    }
    // 1、5：禁用的識別字
    for (name, why) in FORBIDDEN {
        if let Some(hit) = ident_hits(body, &[name], true).first() {
            errs.push(format!("{}：{why}", at(hit.0)));
        }
    }
    if ["constructor", "__proto__", "prototype"].iter().any(|w| raw_tail.contains(w)) {
        errs.push("不能出現 constructor、__proto__、prototype（字串裡也不行：可以拿來建構函式、繞過檢查）".into());
    }
    let quoted_key = {
        let tail = t.get(start..).unwrap_or_default();
        let key: Vec<char> = "agentType".chars().collect();
        let quote = |c: Option<&char>| matches!(c, Some('\'' | '"' | '`'));
        (1..tail.len()).any(|i| tail.get(i..i + key.len()).is_some_and(|s| s == key.as_slice()) && quote(tail.get(i - 1)) && quote(tail.get(i + key.len())))
    };
    if found(&pats.getter, &bt.s)? || quoted_key {
        errs.push("agentType 不能寫成加引號的鍵、計算出來的鍵或 getter（要是 agentType: NS + '<角色>'）".into());
    }
    // 5：不能重新定義 awf 接上的名字
    let is_reserved = |name: &str| reserved.contains(name) || PREAMBLE_NAMES.contains(&name) || name.starts_with("awf");
    for (s, _, g1) in find_all(&pats.redefine, &bt.s)? {
        let name = g1.and_then(|(a, b)| bt.s.get(a..b)).unwrap_or_default();
        if is_reserved(name) {
            errs.push(format!("{}：不能重新定義 {name}（awf 接上的名字；awf 開頭的名字保留給執行核心）", at(bt.c(s))));
        }
    }
    for (s, _, g1) in find_all(&pats.assign, &bt.s)? {
        let name = g1.and_then(|(a, b)| bt.s.get(a..b)).unwrap_or_default();
        if is_reserved(name) {
            let pos = bt.c(s);
            let pre = bt.slice(pos.saturating_sub(8), pos);
            let trimmed = pre.trim_end_matches(is_space);
            let declared = trimmed.len() < pre.len() && ["const", "let", "var"].iter().any(|k| trimmed.ends_with(k));
            if !declared {
                errs.push(format!("{}：不能改寫 {name}", at(pos)));
            }
        }
    }
    // 2：agentType
    let type_hits = ident_hits(body, &["agentType"], false);
    for (s, e, _) in &type_hits {
        let pos = off + e;
        let bad = || format!("{}：agentType 要寫成 NS + '<角色>' 或完整的字串（不能是變數或組出來的值）", at(*s));
        let raw = tt.from(pos);
        let Some(lit) = pats.lit.as_ref().ok_or(Fault)?.captures(raw).map_err(|_| Fault)? else {
            errs.push(bad());
            continue;
        };
        let lit_end = raw.get(..lit.get(0).map(|g| g.end()).unwrap_or(0)).unwrap_or_default().chars().count();
        let next = mt.from(pos + lit_end).chars().find(|c| !is_space(*c));
        if next.is_some_and(|c| !matches!(c, ',' | '}' | ')')) {
            errs.push(bad());
            continue;
        }
        let with_ns = lit.get(1).is_some_and(|g| !g.as_str().is_empty());
        let full = format!("{}{}", if with_ns { ns } else { "" }, lit.get(3).map(|g| g.as_str()).unwrap_or_default());
        if !allowed_types.contains(&full) {
            errs.push(format!(
                "{}：{full} 不是登記過的角色（模版角色，或 guard.config.json 的 extension_agents；dev-merge-steward 不能由自訂腳本派）",
                at(*s)
            ));
        }
    }
    if type_hits.is_empty() {
        errs.push("腳本沒有派任何 agent（找不到 agentType）".into());
    }
    // 3：迴圈
    for (s, _, k) in ident_hits(body, &["while", "do"], true) {
        let word = if k == 0 { "while" } else { "do" };
        errs.push(format!("{}：不能用 {word} 迴圈（沒有上限）；改用 pattern 的 awfLoop（要給 max）", at(s)));
    }
    for (s, e, _) in ident_hits(body, &["for"], true) {
        let open = (e..body.len()).find(|k| body.get(*k).is_some_and(|c| !is_space(*c))).filter(|k| body.get(*k) == Some(&'('));
        let Some((open, end)) = open.and_then(|o| match_close(body, o).map(|c| (o, c))) else {
            errs.push(format!("{}：這個 for 讀不懂（for await 不准）", at(s)));
            continue;
        };
        let head = body.get(open + 1..end).unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        let (mut depth, mut cur) = (0i64, String::new());
        for ch in head {
            if matches!(ch, '(' | '[' | '{') {
                depth += 1;
            } else if matches!(ch, ')' | ']' | '}') {
                depth -= 1;
            }
            if *ch == ';' && depth == 0 {
                parts.push(std::mem::take(&mut cur));
            } else {
                cur.push(*ch);
            }
        }
        parts.push(cur);
        let head_s: String = head.iter().collect();
        let ok = match parts.as_slice() {
            [_] => pats.for_head.as_ref().ok_or(Fault)?.is_match(&head_s),
            [_, cond, step] => (cond.contains('<') || cond.contains('>')) && !step.chars().all(is_space),
            _ => false,
        };
        if !ok {
            errs.push(format!("{}：for 迴圈要有上限（for…of、for…in，或有條件、有遞增的 for）", at(s)));
        }
    }
    // 4：流程層級的 return 都要帶 trace
    let spans = function_spans(body);
    let top: Vec<(usize, usize)> = ident_hits(body, &["return"], true).into_iter().map(|(s, e, _)| (s, e))
        .filter(|(s, _)| !spans.iter().any(|(a, b)| a < s && s < b)).collect();
    if top.is_empty() {
        errs.push("腳本沒有 return：每個結束點都要回傳 { …, trace: awfTrace() }".into());
    }
    for (s, e) in top {
        let (mut j, mut depth) = (e, 0i64);
        while j < body.len() {
            let ch = body.get(j).copied().unwrap_or(' ');
            if matches!(ch, '(' | '[' | '{') {
                depth += 1;
            } else if matches!(ch, ')' | ']' | '}') {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            } else if matches!(ch, ';' | '\n') && depth == 0 && !bt.slice(e, j).chars().all(is_space) {
                break;
            }
            j += 1;
        }
        if !found(&pats.trace, bt.slice(e, j))? {
            errs.push(format!("{}：這個 return 沒有帶 trace: awfTrace()（閘門靠 trace 核對派了誰、紀錄齊不齊）", at(s)));
        }
    }
    Ok(())
}

/// 片段定義的頂層名字（函式與常數）。
pub fn defined_names<'a>(snippets: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    static RE: OnceLock<[Option<Regex>; 2]> = OnceLock::new();
    let res = RE.get_or_init(|| {
        let s = WS_CLASS;
        [
            Regex::new(&format!(r"(?m)^(?:async[{s}]+)?function[{s}]*\*?[{s}]*([{W}$]+)")).ok(),
            Regex::new(&format!(r"(?m)^(?:const|let|var)[{s}]+([{W}$]+)")).ok(),
        ]
    });
    let mut out = BTreeSet::new();
    for src in snippets {
        for re in res.iter().flatten() {
            out.extend(re.captures_iter(src).filter_map(|c| c.get(1).map(|g| g.as_str().to_string())));
        }
    }
    out
}

/// 把 preamble 接在 meta 後面，再把腳本內容包成一個函式執行。
///
/// 包起來有兩個作用（這才是擋，靜態檢查只是提早發現）：
///   - 函式的參數遮蔽 agent、tryAgent、awfState 等名字：內容裡拿到的是 undefined，直接派工或改核心狀態會出錯。
///   - 回傳值的 stage、recipe、trace 由外層覆寫：trace 一律取自執行核心，腳本自己給的不採用。
pub fn assemble(text: &str, preamble: &str, stage: &str) -> Result<String, ScriptError> {
    let t: Vec<char> = text.chars().collect();
    let end = meta_end_chars(&mask_chars(&t)?)?;
    let head: String = t.get(..end).unwrap_or_default().iter().collect();
    let body: String = t.get(end..).unwrap_or_default().iter().collect();
    let stage_js = format!("\"{}\"", stage.replace('"', ""));
    let wrap_open = format!("const __awfBody = async ({}) => {{", SHADOWED.join(", "));
    let wrap_close = [
        "}".to_string(),
        "const __awfOut = await __awfBody()".to_string(),
        format!("if (!__awfOut || typeof __awfOut !== 'object' || Array.isArray(__awfOut)) return {{ stage: {stage_js}, halted: true, halt_reason: '自訂腳本沒有回傳物件', missing: [], recipe: RECIPE, trace: awfTrace() }}"),
        format!("return {{ ...__awfOut, stage: {stage_js}, recipe: RECIPE, trace: awfTrace() }}"),
    ]
    .join("\n");
    Ok(format!(
        "{}\n\n{}\n\n// ── 自訂腳本的內容（包成函式：遮蔽 agent 等名字；回傳的 trace 由外層取自執行核心）──\n{wrap_open}{}{}\n{wrap_close}\n",
        head.trim_end_matches(is_space),
        preamble.trim_end_matches('\n'),
        if body.starts_with('\n') { "" } else { "\n" },
        body.trim_end_matches('\n'),
    ))
}

// ------------------------------------------------------------------ selftest

pub const GOOD: &str = r#"export const meta = { name: 'my-waves', description: '分兩波打地基：agent() 這個字寫在字串裡沒關係' }
const SUBTASKS = /*@schema:subtasks*/ null /*@end*/
const waves = [['t1'], ['t2', 't3']]
const done = []
for (const [i, wave] of waves.entries()) {
  const r = await awfFanOut(wave.map((id) => ({ prompt: `做 ${id}（不要 while 迴圈）`, label: `Implement:${id}`, agentType: NS + 'dev-implementer', schema: SUBTASKS })), { max: 4 })
  if (r.halted) return { stage: 'build', halted: true, halt_reason: r.reason, missing: r.missing, recipe: RECIPE, trace: awfTrace() }
  done.push(...r.results)
}
for (let i = 0; i < 2; i++) { done.push(i) }
const pick = (xs) => { return xs[0] }
const ext = await awfAgent('看一下', { label: 'ext', agentType: 'acme:a11y', schema: SUBTASKS })
return {
  stage: 'build', halted: false, results: done, first: pick(done), ext,
  recipe: RECIPE,
  trace: awfTrace(),
}
"#;

fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `awf script --selftest`：回（exit code，輸出）。
pub fn selftest_captured() -> (i32, String) {
    let mut ok = true;
    let mut out = String::new();
    let ns = "agent-workflow:";
    let mut allowed: BTreeSet<String> = ROLES.iter().map(|r| format!("{ns}{r}")).collect();
    allowed.insert("acme:a11y".into());
    let reserved: BTreeSet<String> = ["awfAgent", "tryAgent", "resolveRecipe", "layersOf"].iter().map(|s| s.to_string()).collect();
    let run = |t: &str| check(t, &allowed, ns, &reserved);
    out.push_str("awf_script selftest\n");
    {
        let mut expect = |name: &str, cond: bool, detail: String| {
            let extra = if cond { String::new() } else { format!("：{}", head_chars(&detail, 500)) };
            out.push_str(&format!("  [{}] {name}{extra}\n", if cond { "通過" } else { "失敗" }));
            ok = ok && cond;
        };
        expect("合格的腳本：沒有任何錯誤", run(GOOD).is_empty(), format!("{:?}", run(GOOD)));
        let sample = "const a = 'agent(1)' // agent()\nconst b = `x ${ agent(2) } y` /* while */";
        let mk = mask(sample).unwrap_or_default();
        expect("遮罩：字串與註解的內容換成空白，樣板字串裡 ${ } 的程式碼保留",
               !mk.contains("agent(1)") && mk.contains("agent(2)") && !mk.contains("while") && mk.chars().count() == sample.chars().count(), mk.clone());

        let before_last_return = |t: &str| t.find("return {\n  stage").and_then(|i| t.get(..i)).unwrap_or(t).to_string();
        let halted_return = "if (r.halted) return { stage: 'build', halted: true, halt_reason: r.reason, missing: r.missing, recipe: RECIPE, trace: awfTrace() }";
        let for_loop = "for (let i = 0; i < 2; i++)";
        let for_line = "for (let i = 0; i < 2; i++) { done.push(i) }";
        let bad_cases: Vec<(&str, String, &str)> = vec![
            ("直接呼叫 agent() 被擋", GOOD.replace("const done = []", "const done = [await agent('x', {})]"), r"不能直接用 agent\(\)"),
            ("藏在樣板字串 ${ } 裡的 agent() 也被擋", GOOD.replace("做 ${id}", "做 ${await agent('x', {})}"), r"不能直接用 agent\(\)"),
            ("把 agent 另存成別名被擋", GOOD.replace("const done = []", "const done = []\nconst go = agent"), r"不能直接用 agent\(\)"),
            ("直接呼叫 tryAgent() 被擋", GOOD.replace("await awfAgent('看一下'", "await tryAgent('看一下'"), "不能直接用 tryAgent"),
            ("未登記的角色被擋", GOOD.replace("'acme:a11y'", "'acme:unknown'"), "acme:unknown 不是登記過的角色"),
            ("dev-merge-steward 不能派", GOOD.replace("NS + 'dev-implementer'", "NS + 'dev-merge-steward'"), "dev-merge-steward 不是登記過的角色"),
            ("agentType 是變數被擋", GOOD.replace("agentType: 'acme:a11y'", "agentType: who"), "agentType 要寫成"),
            ("agentType 是組出來的值被擋", GOOD.replace("NS + 'dev-implementer'", "NS + 'dev-' + kind"), "agentType 要寫成"),
            ("while 迴圈被擋", GOOD.replace(for_line, "while (true) { done.push(1) }"), "不能用 while 迴圈"),
            ("do 迴圈被擋", GOOD.replace(for_line, "do { done.push(1) } while (done.length < 3)"), "不能用 do 迴圈"),
            ("沒有條件的 for 被擋", GOOD.replace(for_loop, "for (;;)"), "for 迴圈要有上限"),
            ("沒有遞增的 for 被擋", GOOD.replace(for_loop, "for (let i = 0; i < 2;)"), "for 迴圈要有上限"),
            ("有一個 return 沒帶 trace 被擋", GOOD.replace("missing: r.missing, recipe: RECIPE, trace: awfTrace() }", "missing: r.missing, recipe: RECIPE }"), "第 7 行：這個 return 沒有帶 trace"),
            ("最後的 return 沒帶 trace 被擋", GOOD.replace("  trace: awfTrace(),\n", ""), "第 13 行：這個 return 沒有帶 trace"),
            ("trace 不是 awfTrace() 被擋", GOOD.replace("  trace: awfTrace(),\n", "  trace: [],\n"), "沒有帶 trace"),
            ("完全沒有 return 被擋", before_last_return(GOOD).replace(halted_return, ""), "腳本沒有 return"),
            ("重新定義 awfAgent 被擋", GOOD.replace("const done = []", "const done = []\nfunction awfAgent() { return {} }"), "不能重新定義 awfAgent"),
            ("改寫 awfTrace 被擋", GOOD.replace("const done = []", "const done = []\nawfTrace = () => []"), "不能改寫 awfTrace"),
            ("重新宣告 RECIPE 被擋", GOOD.replace("const done = []", "const done = []\nconst RECIPE = {}"), "不能重新定義 RECIPE"),
            ("碰核心內部狀態被擋", GOOD.replace("const done = []", "const done = []\nawfState().trace.push({})"), "awfState"),
            ("自己呼叫 awfKernelInit 被擋", GOOD.replace("const done = []", "const done = []\nawfKernelInit({ max_agents: 0 })"), "不要自己呼叫 awfKernelInit"),
            ("eval 被擋", GOOD.replace("const done = []", "const done = [eval('1')]"), "不能用 eval"),
            (".constructor 被擋", GOOD.replace("const done = []", "const done = [(() => 1).constructor('return 1')]"), "constructor"),
            ("三元運算式取出 agent 被擋", GOOD.replace("const done = []", "const done = []\nconst go = 1 ? agent : 0"), r"不能直接用 agent\(\)"),
            ("\\u 跳脫拼出識別字被擋", GOOD.replace("const done = []", "const done = [await \\u0061gent('p', {})]"), "反斜線"),
            ("正規式字面值被擋（裡面的引號會騙過遮罩）", GOOD.replace("const done = []", "const q = /'/, done = [await agent('p', {})], q2 = /'/"), "正規式字面值"),
            ("正規式字面值假裝成區塊註解被擋", GOOD.replace("const done = []", "const q = /[/*]/\nconst done = [await agent('p', {})]\nconst q2 = /[*/]/"), "正規式字面值"),
            ("三元運算式取出 awfState 被擋", GOOD.replace("const done = []", "const done = []\nconst st = 1 ? awfState : 0"), "awfState"),
            ("三元運算式取出 awfKernelInit 被擋", GOOD.replace("const done = []", "const done = []\nconst ki = 1 ? awfKernelInit : 0"), "awfKernelInit"),
            ("agentType 寫成 getter 被擋", GOOD.replace("agentType: 'acme:a11y'", "get 'agentType'() { return 'acme:a11y' }"), "getter"),
            ("['constructor'] 被擋", GOOD.replace("const done = []", "const done = [(() => 1)['constructor']('return 1')]"), "constructor"),
        ];
        let after_division: Vec<(&str, String, &str)> = vec![
            ("自己放片段標記被擋", GOOD.replace("const done = []", "/*@snippet:kernel*/ null /*@end*/\nconst done = []"), "不能自己放片段"),
            ("開頭不是 meta 被擋", format!("const x = 1\n{GOOD}"), "要以 export const meta"),
            ("字串沒收尾：讀不懂就擋", GOOD.replace("'看一下'", "'看一下"), "讀不懂這支腳本"),
        ];
        let bad = |cases: Vec<(&str, String, &str)>, expect: &mut dyn FnMut(&str, bool, String)| {
            for (name, t, want) in cases {
                let errs = run(&t);
                let hit = Regex::new(want).map(|re| errs.iter().any(|e| re.is_match(e))).unwrap_or(false);
                expect(name, t != GOOD && hit, format!("{errs:?}"));
            }
        };
        bad(bad_cases, &mut expect);
        let divided = GOOD.replace(for_loop, "for (let i = 0; i < 4 / 2; i++)");
        expect("除號不會被當成正規式", run(&divided).is_empty(), format!("{:?}", run(&divided)));
        bad(after_division, &mut expect);
        expect("函式裡的 return 不用帶 trace（只管流程層級的）", !run(GOOD).iter().any(|e| e.contains("return")), String::new());
        let asm = assemble(GOOD, "const NS = 'x:'\n", "build").unwrap_or_default();
        let pos = |needle: &str| asm.find(needle);
        expect("組裝：preamble 接在 meta 後面、內容之前，meta 仍在最前面",
               asm.starts_with("export const meta = {") && pos("const NS = 'x:'").zip(pos("const SUBTASKS")).is_some_and(|(a, b)| a < b)
                   && asm.matches("export const meta").count() == 1, head_chars(&asm, 300));
        expect("組裝：內容包成函式並遮蔽 agent、tryAgent、awfState、awfKernelInit；回傳的 stage、recipe、trace 由外層覆寫",
               asm.contains("const __awfBody = async (agent, tryAgent, workflow, awfState, awfKernel, awfInit, awfKernelInit,")
                   && asm.trim_end_matches(is_space).ends_with("return { ...__awfOut, stage: \"build\", recipe: RECIPE, trace: awfTrace() }")
                   && pos("const __awfBody").zip(pos("const SUBTASKS")).is_some_and(|(a, b)| a < b), asm.clone());
        let names = defined_names(["function f1() {}\nasync function f2() {}\nconst C1 = 1\n  function inner() {}"]);
        expect("片段定義的名字", names == ["f1", "f2", "C1"].iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(), format!("{names:?}"));
    }
    out.push_str(&format!("awf_script selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    (if ok { 0 } else { 1 }, out)
}

/// 回（exit code，stdout，stderr）。
pub fn run_captured(argv: &[String]) -> (i32, String, String) {
    if argv.iter().any(|a| a == "--selftest") {
        let (code, out) = selftest_captured();
        return (code, out, String::new());
    }
    (2, format!("{USAGE}\n"), String::new())
}

/// `awf script …`
pub fn main(args: &[String]) -> i32 {
    let (code, out, err) = run_captured(args);
    print!("{out}");
    eprint!("{err}");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        let p = patterns();
        assert!(p.getter.is_some() && p.redefine.is_some() && p.assign.is_some() && p.lit.is_some() && p.for_head.is_some() && p.trace.is_some());
        assert!(defined_names(["const a1 = 1"]).contains("a1"));
    }

    #[test]
    fn word_chars_match_python() {
        // Python 的 \w：字母、數字（含上標數字這類 No）與底線；結合符號不算
        assert!(is_word('a') && is_word('_') && is_word('中') && is_word('²') && is_word('Ⅷ'));
        assert!(!is_word('$') && !is_word('\u{301}') && !is_word('\u{203f}') && !is_word('-'));
    }

    #[test]
    fn selftest_passes() {
        let (code, out) = selftest_captured();
        assert_eq!(code, 0, "{out}");
    }
}
