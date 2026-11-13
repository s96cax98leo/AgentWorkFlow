//! 規格層的讀取：把 OpenSpec 目錄（現行規格 `specs/`、變更 `changes/`）讀成資料模型。
//!
//! 只讀目錄與 Markdown 格式；規格檔的內容一律當資料，不解讀成指令。
//! 行號都是 1 起算；`file` 都是相對於規格根目錄、用 `/` 分隔的路徑。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 行段（1 起算，含頭尾）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// 情境（`#### Scenario: <標題>`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scenario {
    /// `<能力>/<需求代稱>#<情境代稱>`；讀取時是空的，由 `ids::assign` 填。
    pub id: String,
    pub title: String,
    pub span: Span,
}

/// 需求（`### Requirement: <標題>`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Requirement {
    /// `<能力>/<需求代稱>`；讀取時是空的，由 `ids::assign` 填。
    pub id: String,
    pub title: String,
    /// 標題之後、第一個情境之前的文字（去頭尾空白）。
    pub body: String,
    pub file: String,
    pub span: Span,
    pub scenarios: Vec<Scenario>,
}

/// 能力（`specs/<能力>/spec.md`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capability {
    pub name: String,
    pub file: String,
    pub requirements: Vec<Requirement>,
}

/// 差異區塊的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaKind {
    Added,
    Modified,
    Removed,
    Renamed,
}

impl DeltaKind {
    pub fn label(self) -> &'static str {
        match self {
            DeltaKind::Added => "ADDED",
            DeltaKind::Modified => "MODIFIED",
            DeltaKind::Removed => "REMOVED",
            DeltaKind::Renamed => "RENAMED",
        }
    }
}

/// RENAMED 區塊的一組 FROM／TO。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rename {
    /// 被改名的需求的 id（沿用舊 id）；現行規格找不到時是空的。
    pub id: String,
    pub from: String,
    pub to: String,
    /// FROM 那一行的行號。
    pub line: usize,
}

/// 一個變更對一個能力的差異（`changes/<名>/specs/<能力>/spec.md`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Delta {
    pub capability: String,
    pub file: String,
    pub added: Vec<Requirement>,
    pub modified: Vec<Requirement>,
    pub removed: Vec<Requirement>,
    pub renamed: Vec<Rename>,
    /// 差異檔的 `## Purpose`（只在建新能力時用）。
    pub purpose: Option<String>,
}

/// 變更的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeState {
    Active,
    Archived,
}

/// tasks.md 的勾選統計。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskStats {
    pub done: usize,
    pub total: usize,
}

/// 變更（`changes/<名>/` 或 `changes/archive/<日期>-<名>/`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    /// 相對於規格根目錄的目錄。
    pub dir: String,
    pub state: ChangeState,
    /// 歸檔目錄名開頭的日期（`YYYY-MM-DD`）；進行中的變更沒有。
    pub archived_date: Option<String>,
    pub deltas: Vec<Delta>,
    pub tasks: TaskStats,
    pub has_proposal: bool,
    pub has_tasks: bool,
    pub has_design: bool,
}

/// 一個問題。`scope` 是 `spec:<能力>`、`change:<名>` 或 `ids`；`code` 是固定的短代碼。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Problem {
    pub file: String,
    pub line: usize,
    pub message: String,
    pub scope: String,
    pub code: &'static str,
}

/// 整個規格目錄。
#[derive(Debug, Clone)]
pub struct Tree {
    pub root: PathBuf,
    /// 顯示用的根目錄（使用者給的寫法，分隔一律 `/`）。
    pub display: String,
    pub capabilities: Vec<Capability>,
    /// 進行中的在前（依名稱），已歸檔的在後（依目錄名，也就是依日期）。
    pub changes: Vec<Change>,
    pub problems: Vec<Problem>,
    /// 沒有讀的路徑（符號連結指到根目錄外、檔名不是 UTF-8）。
    pub skipped: Vec<String>,
}

/// `Tree::find` 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub file: String,
    pub span: Span,
    pub title: String,
    pub is_scenario: bool,
}

impl Tree {
    pub fn capability(&self, name: &str) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.name == name)
    }

    pub fn change(&self, name: &str, state: ChangeState) -> Option<&Change> {
        self.changes.iter().find(|c| c.name == name && c.state == state)
    }

    /// 用 id 找現行規格的需求或情境（id 要先由 `ids::assign` 填好）。
    pub fn find(&self, id: &str) -> Option<Found> {
        for cap in &self.capabilities {
            for r in &cap.requirements {
                if r.id == id {
                    return Some(Found { file: r.file.clone(), span: r.span.clone(), title: r.title.clone(), is_scenario: false });
                }
                for s in &r.scenarios {
                    if s.id == id {
                        return Some(Found { file: r.file.clone(), span: s.span.clone(), title: s.title.clone(), is_scenario: true });
                    }
                }
            }
        }
        None
    }
}

// ───────────────────────── Markdown 的行層解析 ─────────────────────────

/// 切行：`\n` 或 `\r\n`，去掉開頭的 BOM；結尾的換行不多算一行。
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut v: Vec<&str> = t.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// 標題比對用的正規化：連續空白併成一個、去頭尾。
pub fn norm_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 顯示用：控制字元換成空白（規格文字是資料，不讓它影響終端機）。
pub(crate) fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

pub(crate) struct Heading {
    pub idx: usize,
    pub level: usize,
    pub text: String,
}

pub(crate) struct Scan {
    pub heads: Vec<Heading>,
    pub fenced: Vec<bool>,
    /// 沒有關閉的圍欄從哪一行開始（0 起算）。
    pub unclosed: Option<usize>,
}

fn indent_ok(line: &str) -> Option<&str> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        None
    } else {
        Some(t)
    }
}

/// 圍欄：回傳（字元、長度、後面的文字）。
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let t = indent_ok(line)?;
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    if n < 3 {
        return None;
    }
    Some((c, n, t.get(n..)?))
}

fn heading(line: &str) -> Option<(usize, String)> {
    let t = indent_ok(line)?;
    let n = t.chars().take_while(|&c| c == '#').count();
    if n == 0 || n > 6 {
        return None;
    }
    let rest = t.get(n..)?;
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let mut s = rest.trim();
    let stripped = s.trim_end_matches('#');
    if stripped.len() < s.len() && (stripped.is_empty() || stripped.ends_with(|c: char| c.is_whitespace())) {
        s = stripped.trim_end();
    }
    Some((n, norm_title(s)))
}

/// 找出所有標題；圍欄程式碼區塊裡的不算。
pub(crate) fn scan<S: AsRef<str>>(lines: &[S]) -> Scan {
    let mut heads = Vec::new();
    let mut fenced = Vec::with_capacity(lines.len());
    let mut open: Option<(char, usize, usize)> = None;
    for (i, l) in lines.iter().enumerate() {
        let l = l.as_ref();
        if let Some((c, n, _)) = open {
            fenced.push(true);
            if let Some((c2, n2, rest)) = fence(l) {
                if c2 == c && n2 >= n && rest.trim().is_empty() {
                    open = None;
                }
            }
            continue;
        }
        match fence(l) {
            Some((c, n, rest)) if !(c == '`' && rest.contains('`')) => {
                open = Some((c, n, i));
                fenced.push(true);
            }
            _ => {
                fenced.push(false);
                if let Some((level, text)) = heading(l) {
                    heads.push(Heading { idx: i, level, text });
                }
            }
        }
    }
    Scan { heads, fenced, unclosed: open.map(|o| o.2) }
}

/// `Requirement: 標題` → `標題`。關鍵字不分大小寫，冒號接受全形。
fn keyword<'a>(text: &'a str, kw: &str) -> Option<&'a str> {
    let head = text.get(..kw.len())?;
    if !head.eq_ignore_ascii_case(kw) {
        return None;
    }
    let rest = text.get(kw.len()..)?.trim_start();
    let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('：'))?;
    Some(rest.trim())
}

#[derive(Debug, Clone)]
pub(crate) struct RawScn {
    pub title: String,
    pub start: usize,
    pub end_excl: usize,
}

/// 需求區塊（0 起算；`end_excl` 是下一個同級或更高級標題的那一行，或檔尾）。
#[derive(Debug, Clone)]
pub(crate) struct RawReq {
    pub title: String,
    pub start: usize,
    pub end_excl: usize,
    pub scenarios: Vec<RawScn>,
}

pub(crate) fn raw_reqs(n_lines: usize, heads: &[Heading]) -> Vec<RawReq> {
    let mut out = Vec::new();
    for (k, h) in heads.iter().enumerate() {
        if h.level != 3 {
            continue;
        }
        let Some(title) = keyword(&h.text, "Requirement") else { continue };
        let after = heads.get(k + 1..).unwrap_or(&[]);
        let end_excl = after.iter().find(|x| x.level <= 3).map_or(n_lines, |x| x.idx);
        let mut scenarios = Vec::new();
        for (j, s) in after.iter().enumerate() {
            if s.idx >= end_excl {
                break;
            }
            if s.level != 4 {
                continue;
            }
            let Some(st) = keyword(&s.text, "Scenario") else { continue };
            let s_end = after.get(j + 1..).unwrap_or(&[]).iter().find(|x| x.level <= 4).map_or(end_excl, |x| x.idx.min(end_excl));
            scenarios.push(RawScn { title: st.to_string(), start: s.idx, end_excl: s_end });
        }
        out.push(RawReq { title: title.to_string(), start: h.idx, end_excl, scenarios });
    }
    out
}

/// `[start, end_excl)` 裡最後一個非空白行（0 起算）；都空白就回 `start`。
pub(crate) fn last_nonblank<S: AsRef<str>>(lines: &[S], start: usize, end_excl: usize) -> usize {
    let mut i = end_excl.min(lines.len());
    while i > start + 1 {
        if lines.get(i - 1).is_some_and(|l| !l.as_ref().trim().is_empty()) {
            return i - 1;
        }
        i -= 1;
    }
    start
}

fn build_req(lines: &[&str], r: &RawReq, file: &str) -> Requirement {
    let body_end = r.scenarios.first().map_or(r.end_excl, |s| s.start);
    let body = lines.get(r.start + 1..body_end).unwrap_or(&[]).join("\n").trim().to_string();
    let scenarios = r
        .scenarios
        .iter()
        .map(|s| Scenario { id: String::new(), title: s.title.clone(), span: Span { start: s.start + 1, end: last_nonblank(lines, s.start, s.end_excl) + 1 } })
        .collect();
    Requirement {
        id: String::new(),
        title: r.title.clone(),
        body,
        file: file.to_string(),
        span: Span { start: r.start + 1, end: last_nonblank(lines, r.start, r.end_excl) + 1 },
        scenarios,
    }
}

fn delta_kind(text: &str) -> Option<DeltaKind> {
    let mut it = text.split_whitespace();
    let (a, b) = (it.next()?, it.next()?);
    if it.next().is_some() || !b.eq_ignore_ascii_case("requirements") {
        return None;
    }
    [DeltaKind::Added, DeltaKind::Modified, DeltaKind::Removed, DeltaKind::Renamed].into_iter().find(|k| a.eq_ignore_ascii_case(k.label()))
}

pub(crate) fn prob(scope: &str, code: &'static str, file: &str, line: usize, message: String) -> Problem {
    Problem { file: file.to_string(), line, message, scope: scope.to_string(), code }
}

fn req_problems(reqs: &[Requirement], need_scenario: bool, scope: &str, file: &str, out: &mut Vec<Problem>) {
    for r in reqs {
        if r.title.is_empty() {
            out.push(prob(scope, "empty-title", file, r.span.start, "需求沒有標題".into()));
        }
        if need_scenario && r.scenarios.is_empty() {
            out.push(prob(scope, "no-scenario", file, r.span.start, format!("需求「{}」沒有情境（至少要一個 `#### Scenario:`）", clean(&r.title))));
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for s in &r.scenarios {
            if s.title.is_empty() {
                out.push(prob(scope, "empty-title", file, s.span.start, "情境沒有標題".into()));
            }
            if let Some(first) = seen.get(s.title.as_str()) {
                out.push(prob(scope, "dup-scenario", file, s.span.start, format!("情境「{}」在同一個需求裡重複（第一次在第 {first} 行）", clean(&s.title))));
            } else {
                seen.insert(&s.title, s.span.start);
            }
        }
    }
}

/// 解析現行規格的一個 spec.md。
pub fn parse_spec_text(text: &str, capability: &str, file: &str) -> (Capability, Vec<Problem>) {
    let scope = format!("spec:{capability}");
    let lines = split_lines(text);
    let sc = scan(&lines);
    let mut problems = Vec::new();
    if let Some(i) = sc.unclosed {
        problems.push(prob(&scope, "unclosed-fence", file, i + 1, "圍欄程式碼區塊沒有關閉，之後的標題都不會被當成需求".into()));
    }
    for h in &sc.heads {
        if h.level == 2 {
            if let Some(k) = delta_kind(&h.text) {
                problems.push(prob(&scope, "delta-in-spec", file, h.idx + 1, format!("現行規格不該有差異區塊「{} Requirements」", k.label())));
            }
        }
    }
    let requirements: Vec<Requirement> = raw_reqs(lines.len(), &sc.heads).iter().map(|r| build_req(&lines, r, file)).collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for r in &requirements {
        if let Some(first) = seen.get(r.title.as_str()) {
            problems.push(prob(&scope, "dup-title", file, r.span.start, format!("需求「{}」重複（第一次在第 {first} 行）", clean(&r.title))));
        } else {
            seen.insert(&r.title, r.span.start);
        }
    }
    req_problems(&requirements, true, &scope, file, &mut problems);
    (Capability { name: capability.to_string(), file: file.to_string(), requirements }, problems)
}

/// 差異檔的原始結構（apply 要用行段）。
pub(crate) struct RawDelta {
    pub items: Vec<(DeltaKind, RawReq)>,
    /// 不在任何差異區塊裡的需求。
    pub orphans: Vec<RawReq>,
    pub renames: Vec<Rename>,
    pub purpose: Option<String>,
    /// （行號、代碼、說明）
    pub notes: Vec<(usize, &'static str, String)>,
}

fn strip_marks(s: &str) -> &str {
    s.trim().trim_matches('*').trim()
}

/// `- FROM: \`### Requirement: 舊\`` → （是不是 FROM、名稱）。`**FROM:**` 這種粗體寫法也接受。
fn rename_line(line: &str) -> Option<(bool, String)> {
    let u = line.replace("**", "");
    let u = u.trim();
    let u = u.strip_prefix(['-', '*', '+']).unwrap_or(u).trim();
    if let Some(r) = keyword(u, "FROM") {
        return Some((true, rename_value(r)));
    }
    keyword(u, "TO").map(|r| (false, rename_value(r)))
}

fn rename_value(v: &str) -> String {
    let v = strip_marks(v).trim_matches('`').trim();
    let v = v.trim_start_matches('#').trim_start();
    let v = keyword(v, "Requirement").unwrap_or(v);
    norm_title(v)
}

pub(crate) fn raw_delta<S: AsRef<str>>(lines: &[S]) -> RawDelta {
    let sc = scan(lines);
    let mut notes = Vec::new();
    if let Some(i) = sc.unclosed {
        notes.push((i + 1, "unclosed-fence", "圍欄程式碼區塊沒有關閉，之後的標題都不會被當成需求".to_string()));
    }
    // 區塊：（起、訖、種類、是不是 Purpose）
    let tops: Vec<&Heading> = sc.heads.iter().filter(|h| h.level <= 2).collect();
    let mut sections: Vec<(usize, usize, Option<DeltaKind>, bool)> = Vec::new();
    for (k, h) in tops.iter().enumerate() {
        let end = tops.get(k + 1).map_or(lines.len(), |n| n.idx);
        let kind = if h.level == 2 { delta_kind(&h.text) } else { None };
        sections.push((h.idx, end, kind, h.level == 2 && h.text.eq_ignore_ascii_case("purpose")));
    }
    let section_of = |idx: usize| sections.iter().find(|s| idx > s.0 && idx < s.1).and_then(|s| s.2);
    let mut items = Vec::new();
    let mut orphans = Vec::new();
    for r in raw_reqs(lines.len(), &sc.heads) {
        match section_of(r.start) {
            Some(DeltaKind::Renamed) => notes.push((r.start + 1, "rename-format", "RENAMED 區塊只放成對的 FROM／TO，不放需求內文".to_string())),
            Some(k) => items.push((k, r)),
            None => orphans.push(r),
        }
    }
    let mut renames = Vec::new();
    let mut purpose = None;
    for &(start, end, kind, is_purpose) in &sections {
        let body = lines.get(start + 1..end).unwrap_or(&[]);
        if is_purpose {
            let t = body.iter().map(|l| l.as_ref()).collect::<Vec<_>>().join("\n").trim().to_string();
            if !t.is_empty() {
                purpose = Some(t);
            }
        }
        if kind != Some(DeltaKind::Renamed) {
            continue;
        }
        let mut from: Option<(String, usize)> = None;
        for (off, l) in body.iter().enumerate() {
            let idx = start + 1 + off;
            if sc.fenced.get(idx).copied().unwrap_or(false) {
                continue;
            }
            let Some((is_from, name)) = rename_line(l.as_ref()) else { continue };
            if name.is_empty() {
                notes.push((idx + 1, "rename-format", "FROM／TO 後面沒有需求標題".to_string()));
                continue;
            }
            if is_from {
                if let Some((_, line)) = from.replace((name, idx + 1)) {
                    notes.push((line, "rename-pair", "FROM 沒有對應的 TO".to_string()));
                }
            } else {
                match from.take() {
                    Some((f, line)) => renames.push(Rename { id: String::new(), from: f, to: name, line }),
                    None => notes.push((idx + 1, "rename-pair", "TO 前面沒有對應的 FROM".to_string())),
                }
            }
        }
        if let Some((_, line)) = from {
            notes.push((line, "rename-pair", "FROM 沒有對應的 TO".to_string()));
        }
    }
    RawDelta { items, orphans, renames, purpose, notes }
}

/// 解析變更裡一個能力的差異檔。
pub fn parse_delta_text(text: &str, change: &str, capability: &str, file: &str) -> (Delta, Vec<Problem>) {
    let scope = format!("change:{change}");
    let lines = split_lines(text);
    let raw = raw_delta(&lines);
    let mut problems: Vec<Problem> = raw.notes.iter().map(|(l, c, m)| prob(&scope, c, file, *l, m.clone())).collect();
    for r in &raw.orphans {
        problems.push(prob(&scope, "no-section", file, r.start + 1, format!("需求「{}」不在 ADDED／MODIFIED／REMOVED 任何一個區塊裡", clean(&r.title))));
    }
    let mut d = Delta { capability: capability.to_string(), file: file.to_string(), renamed: raw.renames, purpose: raw.purpose, ..Delta::default() };
    for (k, r) in &raw.items {
        let req = build_req(&lines, r, file);
        match k {
            DeltaKind::Added => d.added.push(req),
            DeltaKind::Modified => d.modified.push(req),
            _ => d.removed.push(req),
        }
    }
    req_problems(&d.added, true, &scope, file, &mut problems);
    req_problems(&d.modified, true, &scope, file, &mut problems);
    req_problems(&d.removed, false, &scope, file, &mut problems);
    // 同一個需求不能出現兩次（同區塊或跨區塊）；唯一的例外是 MODIFIED 用 RENAMED 的新名字。
    let mut seen: HashMap<String, (DeltaKind, bool)> = HashMap::new();
    let mut entries: Vec<(String, DeltaKind, bool, usize)> = Vec::new();
    for r in &d.renamed {
        if r.from == r.to {
            problems.push(prob(&scope, "rename-pair", file, r.line, format!("RENAMED 的 FROM 與 TO 相同（「{}」）", clean(&r.from))));
            continue;
        }
        entries.push((r.from.clone(), DeltaKind::Renamed, false, r.line));
        entries.push((r.to.clone(), DeltaKind::Renamed, true, r.line));
    }
    for (k, list) in [(DeltaKind::Removed, &d.removed), (DeltaKind::Modified, &d.modified), (DeltaKind::Added, &d.added)] {
        for r in list {
            entries.push((r.title.clone(), k, false, r.span.start));
        }
    }
    for (title, kind, is_to, line) in entries {
        match seen.get(&title) {
            Some(&(DeltaKind::Renamed, true)) if kind == DeltaKind::Modified => {
                seen.insert(title, (kind, false));
            }
            Some(&(prev, _)) => {
                let msg = if prev == kind {
                    format!("需求「{}」在 {} 區塊出現兩次", clean(&title), kind.label())
                } else {
                    format!("需求「{}」同時出現在 {} 與 {} 區塊", clean(&title), prev.label(), kind.label())
                };
                problems.push(prob(&scope, "dup-delta", file, line, msg));
            }
            None => {
                seen.insert(title, (kind, is_to));
            }
        }
    }
    if d.added.is_empty() && d.modified.is_empty() && d.removed.is_empty() && d.renamed.is_empty() && problems.is_empty() {
        problems.push(prob(&scope, "empty-delta", file, 1, "差異檔沒有任何 ADDED／MODIFIED／REMOVED／RENAMED 的內容".into()));
    }
    (d, problems)
}

/// 數 tasks.md 的勾選框（圍欄裡的不算）。
pub fn count_tasks(text: &str) -> TaskStats {
    let lines = split_lines(text);
    let sc = scan(&lines);
    let mut st = TaskStats::default();
    for (i, l) in lines.iter().enumerate() {
        if sc.fenced.get(i).copied().unwrap_or(false) {
            continue;
        }
        let t = l.trim_start();
        let Some(t) = t.strip_prefix(['-', '*', '+']) else { continue };
        let t = t.trim_start();
        if t.starts_with("[ ]") {
            st.total += 1;
        } else if t.starts_with("[x]") || t.starts_with("[X]") {
            st.total += 1;
            st.done += 1;
        }
    }
    st
}

// ───────────────────────── 讀目錄 ─────────────────────────

/// 相對路徑（`/` 分隔）接到根目錄。
pub(crate) fn rel_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for c in rel.split('/').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

/// 把歸檔目錄名拆成（日期、變更名）。
pub fn split_archive_name(dir: &str) -> (Option<String>, String) {
    let b = dir.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if b.len() > 11 && digits(0..4) && b.get(4) == Some(&b'-') && digits(5..7) && b.get(7) == Some(&b'-') && digits(8..10) && b.get(10) == Some(&b'-') {
        if let (Some(d), Some(n)) = (dir.get(..10), dir.get(11..)) {
            return (Some(d.to_string()), n.to_string());
        }
    }
    (None, dir.to_string())
}

struct Loader {
    root: PathBuf,
    canon: PathBuf,
    problems: Vec<Problem>,
    skipped: Vec<String>,
}

impl Loader {
    fn inside(&self, p: &Path) -> bool {
        fs::canonicalize(p).map(|c| c.starts_with(&self.canon)).unwrap_or(false)
    }

    /// `rel` 底下的子目錄名（排序；指到根目錄外的符號連結不跟）。
    fn dirs(&mut self, rel: &str) -> Vec<String> {
        let dir = rel_path(&self.root, rel);
        if !dir.is_dir() {
            return Vec::new();
        }
        if !self.inside(&dir) {
            self.skipped.push(rel.to_string());
            return Vec::new();
        }
        let Ok(rd) = fs::read_dir(&dir) else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                self.skipped.push(format!("{rel}/（檔名不是 UTF-8）"));
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if !self.inside(&p) {
                self.skipped.push(format!("{rel}/{name}"));
                continue;
            }
            out.push(name);
        }
        out.sort();
        out
    }

    fn is_file(&mut self, rel: &str) -> bool {
        let p = rel_path(&self.root, rel);
        if !p.is_file() {
            return false;
        }
        if !self.inside(&p) {
            if !self.skipped.iter().any(|s| s == rel) {
                self.skipped.push(rel.to_string());
            }
            return false;
        }
        true
    }

    fn file(&mut self, rel: &str, scope: &str) -> Option<String> {
        if !self.is_file(rel) {
            return None;
        }
        match read_text(&rel_path(&self.root, rel)) {
            Ok(t) => Some(t),
            Err(e) => {
                self.problems.push(prob(scope, "read", rel, 1, e));
                None
            }
        }
    }

    fn change(&mut self, dir: &str, name: &str, state: ChangeState, date: Option<String>) -> Change {
        let scope = format!("change:{name}");
        let mark = self.problems.len();
        let has_proposal = self.is_file(&format!("{dir}/proposal.md"));
        let has_design = self.is_file(&format!("{dir}/design.md"));
        let has_tasks = self.is_file(&format!("{dir}/tasks.md"));
        let tasks = self.file(&format!("{dir}/tasks.md"), &scope).as_deref().map(count_tasks).unwrap_or_default();
        let mut deltas = Vec::new();
        for cap in self.dirs(&format!("{dir}/specs")) {
            let file = format!("{dir}/specs/{cap}/spec.md");
            if let Some(text) = self.file(&file, &scope) {
                let (d, ps) = parse_delta_text(&text, name, &cap, &file);
                self.problems.extend(ps);
                deltas.push(d);
            }
        }
        if state == ChangeState::Archived {
            // 已歸檔的是歷史，不再檢查
            self.problems.truncate(mark);
        }
        Change { name: name.to_string(), dir: dir.to_string(), state, archived_date: date, deltas, tasks, has_proposal, has_tasks, has_design }
    }
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    let b = fs::read(path).map_err(|e| format!("讀不到檔案：{e}"))?;
    String::from_utf8(b).map_err(|_| "檔案不是 UTF-8 文字".to_string())
}

/// 讀整個規格目錄。`display` 是訊息裡顯示的根目錄寫法。
pub fn load(root: &Path, display: &str) -> Result<Tree, String> {
    let display = display.replace('\\', "/").trim_end_matches('/').to_string();
    if !root.is_dir() {
        return Err(format!("找不到規格目錄：{display}"));
    }
    let canon = fs::canonicalize(root).map_err(|e| format!("讀不到規格目錄 {display}：{e}"))?;
    let mut ld = Loader { root: root.to_path_buf(), canon, problems: Vec::new(), skipped: Vec::new() };
    let mut capabilities = Vec::new();
    for cap in ld.dirs("specs") {
        let file = format!("specs/{cap}/spec.md");
        if let Some(text) = ld.file(&file, &format!("spec:{cap}")) {
            let (c, ps) = parse_spec_text(&text, &cap, &file);
            ld.problems.extend(ps);
            capabilities.push(c);
        }
    }
    let mut changes = Vec::new();
    for name in ld.dirs("changes") {
        if name == "archive" {
            continue;
        }
        changes.push(ld.change(&format!("changes/{name}"), &name, ChangeState::Active, None));
    }
    for dir in ld.dirs("changes/archive") {
        let (date, name) = split_archive_name(&dir);
        changes.push(ld.change(&format!("changes/archive/{dir}"), &name, ChangeState::Archived, date));
    }
    Ok(Tree { root: root.to_path_buf(), display, capabilities, changes, problems: ld.problems, skipped: ld.skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "# 訂單 Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n\n#### Scenario: 庫存不足\n- **WHEN** 庫存是 0\n- **THEN** 回覆缺貨\n\n### Requirement: 取消訂單\n系統 SHALL 讓會員取消。\n\n#### Scenario: 出貨前\n- **WHEN** 還沒出貨\n- **THEN** 可以取消\n";

    #[test]
    fn spec_lines_and_scenarios() {
        let (c, ps) = parse_spec_text(SPEC, "orders", "specs/orders/spec.md");
        assert!(ps.is_empty(), "{ps:?}");
        assert_eq!(c.requirements.len(), 2);
        let r = &c.requirements[0];
        assert_eq!((r.title.as_str(), r.span.start, r.span.end), ("建立訂單", 8, 17));
        assert_eq!(r.body, "系統 SHALL 讓會員建立訂單。");
        assert_eq!(r.scenarios.len(), 2);
        assert_eq!((r.scenarios[0].span.start, r.scenarios[0].span.end), (11, 13));
        assert_eq!((r.scenarios[1].span.start, r.scenarios[1].span.end), (15, 17));
        assert_eq!((c.requirements[1].span.start, c.requirements[1].span.end), (19, 24));
    }

    #[test]
    fn crlf_gives_same_model() {
        let (a, _) = parse_spec_text(SPEC, "orders", "f");
        let (b, ps) = parse_spec_text(&SPEC.replace('\n', "\r\n"), "orders", "f");
        assert!(ps.is_empty());
        assert_eq!(a, b);
    }

    #[test]
    fn heading_whitespace_and_fullwidth_colon() {
        let t = "  ###   Requirement:   登入   ###\n說明\n\n####  Scenario：  密碼正確  \n- **THEN** 進首頁\n";
        let (c, ps) = parse_spec_text(t, "login", "f");
        assert!(ps.is_empty(), "{ps:?}");
        assert_eq!(c.requirements[0].title, "登入");
        assert_eq!(c.requirements[0].scenarios[0].title, "密碼正確");
    }

    #[test]
    fn fenced_headings_are_ignored() {
        let t = "### Requirement: 匯出\n範例：\n\n```md\n### Requirement: 假的\n#### Scenario: 假的\n```\n\n~~~~\n### Requirement: 也是假的\n~~~\n~~~~\n\n#### Scenario: 真的\n- **THEN** 有檔案\n";
        let (c, ps) = parse_spec_text(t, "export", "f");
        assert!(ps.is_empty(), "{ps:?}");
        assert_eq!(c.requirements.len(), 1);
        assert_eq!(c.requirements[0].scenarios.len(), 1);
        assert_eq!(c.requirements[0].scenarios[0].span.start, 14);
    }

    #[test]
    fn unclosed_fence_is_reported() {
        let (c, ps) = parse_spec_text("### Requirement: 甲\n#### Scenario: 一\n```\n### Requirement: 乙\n", "x", "f");
        assert_eq!(c.requirements.len(), 1);
        assert!(ps.iter().any(|p| p.code == "unclosed-fence" && p.line == 3));
    }

    #[test]
    fn duplicate_titles_are_errors_not_overrides() {
        let t = "### Requirement: 登入\n#### Scenario: 一\n### Requirement:  登入 \n#### Scenario: 二\n#### Scenario: 二\n";
        let (c, ps) = parse_spec_text(t, "login", "f");
        assert_eq!(c.requirements.len(), 2);
        assert!(ps.iter().any(|p| p.code == "dup-title" && p.line == 3));
        assert!(ps.iter().any(|p| p.code == "dup-scenario" && p.line == 5));
    }

    #[test]
    fn empty_file_and_missing_scenario() {
        let (c, ps) = parse_spec_text("", "x", "f");
        assert!(c.requirements.is_empty() && ps.is_empty());
        let (_, ps) = parse_spec_text("\u{feff}### Requirement: 沒情境\n文字\n", "x", "f");
        assert!(ps.iter().any(|p| p.code == "no-scenario" && p.line == 1));
        let (_, ps) = parse_spec_text("## ADDED Requirements\n", "x", "f");
        assert!(ps.iter().any(|p| p.code == "delta-in-spec"));
    }

    const DELTA: &str = "# Delta\n\n## ADDED Requirements\n\n### Requirement: 退款\n說明\n\n#### Scenario: 七天內\n- **THEN** 退款\n\n## MODIFIED Requirements\n\n### Requirement: 查詢訂單\n新的說明\n\n#### Scenario: 有訂單\n- **THEN** 列出\n\n## REMOVED Requirements\n\n### Requirement: 傳真下單\n**Reason**: 沒人用\n\n## RENAMED Requirements\n\n- FROM: `### Requirement: 建立訂單`\n- TO: `### Requirement: 查詢訂單`\n";

    #[test]
    fn delta_four_sections() {
        let (d, ps) = parse_delta_text(DELTA, "c", "orders", "f");
        assert!(ps.is_empty(), "{ps:?}");
        assert_eq!((d.added.len(), d.modified.len(), d.removed.len(), d.renamed.len()), (1, 1, 1, 1));
        assert_eq!(d.renamed[0], Rename { id: String::new(), from: "建立訂單".into(), to: "查詢訂單".into(), line: 26 });
        assert_eq!(d.added[0].span.start, 5);
    }

    #[test]
    fn rename_line_variants() {
        assert_eq!(rename_line("- FROM: `### Requirement: 舊`"), Some((true, "舊".into())));
        assert_eq!(rename_line("* **TO:** 新 名稱"), Some((false, "新 名稱".into())));
        assert_eq!(rename_line("- 其他說明"), None);
    }

    #[test]
    fn delta_problems() {
        let t = "### Requirement: 孤兒\n#### Scenario: 一\n\n## ADDED Requirements\n### Requirement: 甲\n\n## REMOVED Requirements\n### Requirement: 甲\n\n## RENAMED Requirements\n- FROM: `### Requirement: 乙`\n- FROM: `### Requirement: 丙`\n- TO: `### Requirement: 丁`\n- TO: 戊\n";
        let (_, ps) = parse_delta_text(t, "c", "orders", "f");
        let codes: Vec<(&str, usize)> = ps.iter().map(|p| (p.code, p.line)).collect();
        assert!(codes.contains(&("no-section", 1)), "{codes:?}");
        assert!(codes.contains(&("no-scenario", 5)));
        assert!(codes.contains(&("dup-delta", 5)));
        assert!(codes.contains(&("rename-pair", 11)));
        assert!(codes.contains(&("rename-pair", 14)));
        assert!(!codes.contains(&("no-scenario", 8)), "REMOVED 不需要情境");
        let (_, ps) = parse_delta_text("# 只有標題\n", "c", "orders", "f");
        assert!(ps.iter().any(|p| p.code == "empty-delta"));
    }

    #[test]
    fn tasks_and_archive_name() {
        let st = count_tasks("## 1. 做\n- [x] 1.1 甲\n- [ ] 1.2 乙\n  * [X] 丙\n```\n- [ ] 範例\n```\n- 不是勾選\n");
        assert_eq!(st, TaskStats { done: 2, total: 3 });
        assert_eq!(split_archive_name("2026-01-02-add-refund"), (Some("2026-01-02".into()), "add-refund".into()));
        assert_eq!(split_archive_name("加退款"), (None, "加退款".into()));
    }
}
