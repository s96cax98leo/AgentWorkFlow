//! 工具先定位：不派 agent，先用確定性的方式找出「哪個檔的哪幾行」值得讀。
//! 原本是 0.x 的 Python 工具 `locate.py`，輸出、結束碼、訊息文字和 0.x 的 Python 版逐字相同
//! （對跑基準在 `tests/parity/deliverables.expected.json` 的 locate 部分；說明文字裡的指令已改成 `awf locate`）。
//!
//! 做法：把目標 repo 的文字檔切成固定行數的視窗，用需求文字當查詢做 BM25 排序（分詞和知識層相同），
//! 取分數夠高的視窗、合併相鄰的，回報行段。這是輔助功能：任何錯誤都回空結果，由 workflow 改派定位 agent。

use crate::kb::{self, pyish};
use crate::pyfmt;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const ANGLES: [&str; 3] = ["code-contracts", "specs-adr", "requirement"];
const DOC_EXTS: [&str; 5] = [".md", ".markdown", ".txt", ".rst", ".adoc"];
const SKIP_EXTS: [&str; 31] = [".lock", ".min.js", ".min.css", ".map", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".ico", ".pdf", ".zip", ".gz", ".woff", ".woff2",
    ".ttf", ".eot", ".mp4", ".mp3", ".wav", ".bin", ".exe", ".dll", ".so", ".dylib", ".class", ".jar", ".pyc", ".sqlite", ".db", ".snap"];
const SKIP_NAMES: [&str; 9] = ["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "poetry.lock", "uv.lock", "cargo.lock", "go.sum", "composer.lock", "gemfile.lock"];
const PRUNE: [&str; 14] = [".git", "node_modules", ".hg", ".svn", "__pycache__", ".worktrees", ".venv", "venv", "dist", "build", "target", ".next", ".codegraph", "graft"];
const MAX_FILES: usize = 4000;
const MAX_FILE_BYTES: u64 = 300 * 1024;
const MAX_TOTAL_BYTES: usize = 24 * 1024 * 1024;
/// 每個視窗幾行。
const WINDOW: usize = 40;
/// 視窗間隔（重疊 10 行）。
const STEP: usize = 30;
pub const MAX_SEGMENTS: usize = 12;
pub const MAX_SEGMENT_LINES: usize = 120;
/// 分數不到第一名這個比例的視窗不要。
const KEEP_RATIO: f64 = 0.35;
/// 但每個檔最好的那個視窗，分數到第一名這個比例就留（不同的檔各留一段，避免只挑到同一個檔）。
const FILE_RATIO: f64 = 0.15;
/// 少於這個數就算不夠。
pub const MIN_SEGMENTS: usize = 3;
/// 選中的行段涵蓋的查詢詞比例低於這個就算不夠。
const MIN_COVERAGE: f64 = 0.3;
/// `knowledge.locate.prune` 最多幾個目錄名。
pub const PRUNE_EXTRA_MAX: usize = 32;

/// 工具定位的門檻（profile 的 `knowledge.locate`）。預設＝原本寫死的值；沒寫或不合範圍的那一項用預設。
#[derive(Debug, Clone, PartialEq)]
pub struct LocateTune {
    /// 每個角度最多幾段。
    pub max_segments: usize,
    /// 合併後每段最多幾行。
    pub segment_lines: usize,
    /// 少於幾段算不夠。
    pub min_segments: usize,
    /// 涵蓋的查詢詞比例低於這個算不夠。
    pub min_coverage: f64,
    /// 最多看幾個檔。
    pub max_files: usize,
    /// 另外不看的目錄名（內建的那些永遠不看，這裡只能加）。
    pub prune: Vec<String>,
}

impl Default for LocateTune {
    fn default() -> Self {
        LocateTune { max_segments: MAX_SEGMENTS, segment_lines: MAX_SEGMENT_LINES, min_segments: MIN_SEGMENTS, min_coverage: MIN_COVERAGE, max_files: MAX_FILES, prune: Vec::new() }
    }
}

/// 目錄名：不含分隔符、不是 `.`／`..`、沒有 glob 字元與控制字元。
pub fn prune_name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 64 && s != "." && s != ".." && !s.chars().any(|c| matches!(c, '/' | '\\' | '*' | '?') || c.is_control())
}

impl LocateTune {
    pub fn of(profile: &Value) -> LocateTune {
        use crate::tune::{at, int, num};
        let k = |name: &str| at(profile, &["knowledge", "locate", name]);
        // prune：整份要合規定才採用（有一項不對就整份不用，避免只生效一半）
        let prune = match k("prune") {
            Some(Value::Array(a)) if a.len() <= PRUNE_EXTRA_MAX && a.iter().all(|x| x.as_str().is_some_and(prune_name_ok)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        LocateTune {
            max_segments: int(k("max_segments"), 1, 40, MAX_SEGMENTS as i64) as usize,
            segment_lines: int(k("segment_lines"), WINDOW as i64, 400, MAX_SEGMENT_LINES as i64) as usize,
            // 只能調高：範本另外用自己的下限（3 段）判斷，調低了兩邊會不一致
            min_segments: int(k("min_segments"), MIN_SEGMENTS as i64, 12, MIN_SEGMENTS as i64) as usize,
            min_coverage: num(k("min_coverage"), 0.05, 1.0, MIN_COVERAGE),
            max_files: int(k("max_files"), 100, 20_000, MAX_FILES as i64) as usize,
            prune,
        }
    }

    fn pruned(&self, name: &str) -> bool {
        PRUNE.contains(&name) || self.prune.iter().any(|p| p == name) || crate::layout::is_worktrees_name(name)
    }
}

thread_local! {
    /// 這次執行用的門檻。`awf locate` 與 `awf pack` 依專案的 profile 放進來；直接呼叫函式時是預設值。
    static LOCATE_TUNE: std::cell::RefCell<LocateTune> = std::cell::RefCell::new(LocateTune::default());
}

pub fn set_tune(t: LocateTune) {
    LOCATE_TUNE.with(|c| *c.borrow_mut() = t);
}

pub fn tune() -> LocateTune {
    LOCATE_TUNE.with(|c| c.borrow().clone())
}

const K1: f64 = 1.5;
const B: f64 = 0.75;
const MAX_TERMS: usize = 400;
/// 程式角度只當「種子」：詞彙比對找不到「新功能會用到、但需求沒提到名字」的檔，所以一律還是派定位 agent。
const SEED_ONLY: [&str; 1] = ["code-contracts"];
/// 取分數最高的幾個檔當種子，沿引用關係擴展一層。
const SEED_FILES: usize = 3;
/// 最多擴展幾個檔。
const MAX_EXPAND: usize = 6;
const GENERIC_STEMS: [&str; 27] = ["__init__", "index", "main", "models", "model", "handlers", "handler", "service", "services", "utils", "util", "helpers",
    "types", "constants", "config", "settings", "repository", "validators", "views", "routes", "urls", "schema", "schemas", "test", "tests", "mod", "lib"];

/// 原本的模組說明（`--help` 與參數不對時原樣印出）。
const DOC: &str = "工具先定位：不派 agent，先用確定性的方式找出「哪個檔的哪幾行」值得讀。

為什麼要有：兩段式調研的定位 agent 實測佔 plan 段約六成 token（每個角度 9～11 輪查詢）。
多數時候需求裡的詞（識別字、路徑、中文名詞）直接對得到程式與文件，這一步不需要模型。

做法：把目標 repo 的文字檔切成固定行數的視窗，用需求文字當查詢做 BM25 排序（分詞和 awf kb 相同：英數字小寫單字、中日韓 bigram），
取分數夠高的視窗、合併相鄰的，回報行段。命中夠多、涵蓋夠多查詢詞才算「夠」（enough）；不夠時 workflow 仍會派定位 agent，並把這裡找到的當成起點。

角度決定看哪些檔：
  code-contracts  程式檔（不看文件）
  specs-adr       specs/、docs/ 底下的文件
  requirement     需求檔本身（需求是專案內的檔案時）加上文件

用法：
    awf locate --repo <目標 repo> --requirement \"<需求文字或專案內的檔案路徑>\" [--angle <角度>]… [--json]
    awf locate --selftest
只用標準庫。結果可重現：同樣的檔案內容與需求，輸出完全相同。這是輔助功能：任何錯誤都回空結果，由 workflow 改派定位 agent。
";

/// 一個行段。
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub why: String,
}

/// 一個角度的定位結果。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Located {
    pub locations: Vec<Location>,
    pub enough: bool,
    pub coverage: f64,
    pub note: String,
}

impl Located {
    fn empty(note: &str) -> Self {
        Located { note: note.to_string(), ..Default::default() }
    }

    /// 和 0.x 的 Python 版回傳的 dict 同樣的鍵。
    pub fn to_json(&self) -> Value {
        let locs: Vec<Value> = self.locations.iter().map(|x| json!({"path": x.path, "start": x.start, "end": x.end, "why": x.why})).collect();
        json!({"locations": locs, "enough": self.enough, "coverage": self.coverage, "note": self.note})
    }
}

fn posix(p: &str) -> String {
    p.replace('\\', "/")
}

/// `str.splitlines()`：\n、\r、\r\n、\v、\f、\x1c～\x1e、\x85、U+2028、U+2029 都算換行。
fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(s.get(start..i).unwrap_or(""));
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = it.peek().copied() {
                    it.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(s.get(start..).unwrap_or(""));
    }
    out
}

fn git_files(repo: &str) -> Option<Vec<String>> {
    let q = std::process::Command::new("git").args(["-C", repo, "ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .stdin(std::process::Stdio::null()).output().ok()?;
    if !q.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&q.stdout).into_owned();
    let tn = tune();
    let mut out: Vec<String> = text.split('\0').filter(|n| !n.is_empty())
        .filter(|n| !n.split('/').any(|part| tn.pruned(part)))
        .map(posix).collect();
    out.sort();
    Some(out)
}

fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
    let tn = tune();
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<String> = Vec::new();
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let path = e.path();
        if path.is_dir() {
            // os.walk 不跟著符號連結往下走（連到目錄的連結既不算檔也不走進去）
            let link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            if !link && !tn.pruned(name.as_str()) {
                dirs.push(name);
            }
        } else {
            out.push(if rel.is_empty() { name } else { format!("{rel}/{name}") });
        }
    }
    dirs.sort();
    for d in dirs {
        let sub = if rel.is_empty() { d.clone() } else { format!("{rel}/{d}") };
        walk(&dir.join(&d), &sub, out);
    }
}

/// repo 裡的檔案（相對路徑、posix、排序）。優先用 git ls-files（尊重 .gitignore），不是 git repo 就自己走訪。
pub fn list_files(repo: &str) -> Vec<String> {
    if let Some(v) = git_files(repo) {
        return v;
    }
    let mut out = Vec::new();
    walk(std::path::Path::new(repo), "", &mut out);
    out.sort();
    out
}

fn wanted(path: &str, angle: &str) -> bool {
    let low = path.to_lowercase();
    let name = low.rsplit('/').next().unwrap_or("");
    if kb::is_sensitive(name) || SKIP_NAMES.contains(&name) || SKIP_EXTS.iter().any(|e| low.ends_with(e)) {
        return false;
    }
    let is_doc = DOC_EXTS.iter().any(|e| low.ends_with(e));
    if angle == "code-contracts" {
        return !is_doc;
    }
    if angle == "specs-adr" {
        let s = format!("/{low}");
        // spec 目錄改過名（guard.config.json 的 specs_dir）時，那個目錄底下的文件也算
        let sd = crate::layout::cached().specs_dir.to_lowercase();
        return is_doc && (s.contains("specs/") || s.contains("docs/") || s.contains("/adr") || s.contains(&format!("/{sd}/")));
    }
    is_doc
}

/// 讀成行；太大、二進位、讀不到回 None。
fn read_lines(p: &str) -> Option<Vec<String>> {
    let lm = std::fs::symlink_metadata(p).ok()?;
    if lm.file_type().is_symlink() {
        return None;
    }
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() || m.len() > MAX_FILE_BYTES {
        return None;
    }
    let raw = std::fs::read(p).ok()?;
    if raw.iter().take(4096).any(|b| *b == 0) {
        return None;
    }
    let text = String::from_utf8_lossy(&raw);
    Some(splitlines(&text).into_iter().map(str::to_string).collect())
}

/// （起, 訖）行號，從 1 起算、含兩端。
fn windows(n: usize) -> Vec<(usize, usize)> {
    if n == 0 {
        return Vec::new();
    }
    if n <= WINDOW {
        return vec![(1, n)];
    }
    let mut out = Vec::new();
    let mut s = 1;
    while s <= n {
        let e = n.min(s + WINDOW - 1);
        out.push((s, e));
        if e == n {
            break;
        }
        s += STEP;
    }
    out
}

/// 別的檔引用這個檔時可能寫的名字：檔名（太通用的不算）、上層目錄加檔名（a.b、a/b 兩種寫法）。
fn ref_keys(path: &str) -> Vec<String> {
    let mut parts: Vec<String> = path.split('/').filter(|s| !s.is_empty() && *s != ".").map(str::to_string).collect();
    if let Some(last) = parts.last_mut() {
        // Path.with_suffix("")：最後一個點在中間才算副檔名
        if let Some(i) = last.rfind('.') {
            if i > 0 && i < last.len() - 1 {
                last.truncate(i);
            }
        }
    }
    let stem = parts.last().map(|s| s.to_lowercase()).unwrap_or_default();
    let mut keys = Vec::new();
    if pyfmt::len(&stem) >= 4 && !GENERIC_STEMS.contains(&stem.as_str()) {
        keys.push(stem.clone());
    }
    if parts.len() >= 2 && !stem.is_empty() && stem != "__init__" {
        let parent = parts.get(parts.len() - 2).map(|s| s.to_lowercase()).unwrap_or_default();
        keys.push(format!("{parent}.{stem}"));
        keys.push(format!("{parent}/{stem}"));
    }
    keys
}

/// 查詢詞：需求全文的詞（知識層的 query_terms 只取前 64 個，這裡自己取）。丟掉含虛字的中文 bigram 與單一字元。
fn terms_of(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let toks: Vec<String> = kb::tokenize(text, true).into_iter().filter(|t| seen.insert(t.clone())).collect();
    let stop = |t: &str| {
        let cs: Vec<char> = t.chars().collect();
        cs.len() == 2 && cs.first().is_some_and(|c| is_cjk(*c)) && cs.iter().any(|c| kb::QUERY_STOP_CHARS.contains(*c))
    };
    let kept: Vec<String> = toks.iter().filter(|t| pyfmt::len(t) > 1 && !stop(t)).cloned().collect();
    let mut use_ = if kept.is_empty() { toks } else { kept };
    use_.truncate(MAX_TERMS);
    use_
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xac00..=0xd7af | 0x20000..=0x2fa1f)
}

/// Python 3.12 的 `sum()`：浮點數用 Neumaier 補償加總。
fn py_sum(xs: impl Iterator<Item = f64>) -> f64 {
    let mut it = xs;
    let Some(mut total) = it.next() else { return 0.0 };
    let mut c = 0.0f64;
    for x in it {
        let t = total + x;
        if total.abs() >= x.abs() {
            c += (total - t) + x;
        } else {
            c += (x - t) + total;
        }
        total = t;
    }
    if c != 0.0 && c.is_finite() {
        total += c;
    }
    total
}

struct Win {
    path: String,
    start: usize,
    end: usize,
    /// 查詢詞的編號 → 次數，依第一次出現的順序。
    tf: Vec<(usize, u32)>,
    len: usize,
    score: f64,
}

struct Seg {
    path: String,
    start: usize,
    end: usize,
    score: f64,
    tf: Vec<(usize, u32)>,
}

fn by_score(a: (f64, &str, usize), b: (f64, &str, usize)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(b.1)).then_with(|| a.2.cmp(&b.2))
}

/// 一個角度的定位。`extra_files`＝（顯示用的路徑, 實際路徑），放在 repo 的檔之後。
pub fn locate(repo: &str, text: &str, angle: &str, extra_files: &[(String, String)]) -> Located {
    let terms = terms_of(text);
    if terms.is_empty() {
        return Located::empty("沒有可用的查詢詞");
    }
    if !ANGLES.contains(&angle) {
        return Located::empty(&format!("不認得的角度 {angle}"));
    }
    let seed_only = SEED_ONLY.contains(&angle);
    let tset: HashMap<&str, usize> = terms.iter().enumerate().map(|(i, t)| (t.as_str(), i)).collect();
    let mut docs: Vec<Win> = Vec::new();
    let tn = tune();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut sizes: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    let mut truncated = false;
    let mut files: Vec<(String, String)> = list_files(repo).into_iter().filter(|f| wanted(f, angle)).map(|f| { let full = pyish::pjoin(repo, &f); (f, full) }).collect();
    files.extend(extra_files.iter().cloned());
    let mut slot: Vec<usize> = vec![usize::MAX; terms.len()];
    for (i, (rel, full)) in files.iter().enumerate() {
        if i >= tn.max_files || total > MAX_TOTAL_BYTES {
            truncated = true;
            break;
        }
        let Some(lines) = read_lines(full) else { continue };
        if lines.is_empty() {
            continue;
        }
        total += lines.iter().map(|x| pyfmt::len(x)).sum::<usize>();
        if seed_only {
            texts.insert(rel.clone(), lines.join("\n").to_lowercase());
            sizes.insert(rel.clone(), lines.len());
        }
        for (s, e) in windows(lines.len()) {
            let chunk = lines.get(s - 1..e).unwrap_or(&[]).join("\n");
            let toks = kb::tokenize(&chunk, false);
            let mut tf: Vec<(usize, u32)> = Vec::new();
            for t in &toks {
                if let Some(&ti) = tset.get(t.as_str()) {
                    match slot.get(ti).copied() {
                        Some(k) if k != usize::MAX => {
                            if let Some(x) = tf.get_mut(k) {
                                x.1 += 1;
                            }
                        }
                        _ => {
                            if let Some(x) = slot.get_mut(ti) {
                                *x = tf.len();
                            }
                            tf.push((ti, 1));
                        }
                    }
                }
            }
            for (ti, _) in &tf {
                if let Some(x) = slot.get_mut(*ti) {
                    *x = usize::MAX;
                }
            }
            docs.push(Win { path: rel.clone(), start: s, end: e, tf, len: toks.len().max(1), score: 0.0 });
        }
    }
    if docs.is_empty() {
        return Located::empty("這個角度沒有可以看的檔案");
    }
    let n = docs.len();
    let avg = docs.iter().map(|d| d.len).sum::<usize>() as f64 / n as f64;
    let mut df = vec![0usize; terms.len()];
    for d in &docs {
        for (ti, _) in &d.tf {
            if let Some(x) = df.get_mut(*ti) {
                *x += 1;
            }
        }
    }
    let idf: Vec<f64> = df.iter().map(|&x| (1.0 + (n as f64 - x as f64 + 0.5) / (x as f64 + 0.5)).ln()).collect();
    let idf_of = |ti: usize| idf.get(ti).copied().unwrap_or(0.0);
    for d in docs.iter_mut() {
        // 很短的視窗（幾行的小檔）不因為長度正規化而分數暴增
        let ln = (d.len as f64).max(avg * 0.5);
        d.score = py_sum(d.tf.iter().map(|&(ti, c)| {
            let c = f64::from(c);
            idf_of(ti) * (c * (K1 + 1.0)) / (c + K1 * (1.0 - B + B * ln / avg))
        }));
    }
    let mut ranked: Vec<&Win> = docs.iter().filter(|d| d.score > 0.0).collect();
    ranked.sort_by(|a, b| by_score((a.score, &a.path, a.start), (b.score, &b.path, b.start)));
    let Some(top) = ranked.first().map(|d| d.score) else {
        return Located::empty("查詢詞在這個角度的檔案裡都沒有出現");
    };
    let mut picked: Vec<&Win> = ranked.iter().copied().filter(|d| d.score >= top * KEEP_RATIO).take(tn.max_segments * 2).collect();
    let mut seen_files: HashSet<&str> = picked.iter().map(|d| d.path.as_str()).collect();
    // ranked 已依分數排序：每個還沒入選的檔，第一次出現的就是它最好的視窗
    for d in &ranked {
        if picked.len() >= tn.max_segments * 2 || d.score < top * FILE_RATIO {
            break;
        }
        if seen_files.insert(d.path.as_str()) {
            picked.push(d);
        }
    }
    // 同一個檔相鄰或重疊的視窗合併（合併後不超過 MAX_SEGMENT_LINES）
    picked.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.start.cmp(&b.start)));
    let mut merged: Vec<Seg> = Vec::new();
    for d in picked {
        match merged.last_mut() {
            Some(m) if m.path == d.path && d.start <= m.end + 1 && d.end + 1 - m.start <= tn.segment_lines => {
                m.end = m.end.max(d.end);
                m.score = m.score.max(d.score);
                for &(ti, c) in &d.tf {
                    match m.tf.iter_mut().find(|x| x.0 == ti) {
                        Some(x) => x.1 += c,
                        None => m.tf.push((ti, c)),
                    }
                }
            }
            _ => merged.push(Seg { path: d.path.clone(), start: d.start, end: d.end, score: d.score, tf: d.tf.clone() }),
        }
    }
    merged.sort_by(|a, b| by_score((a.score, &a.path, a.start), (b.score, &b.path, b.start)));
    merged.truncate(tn.max_segments);
    let term = |ti: usize| terms.get(ti).map(String::as_str).unwrap_or("");
    let mut covered: BTreeSet<usize> = BTreeSet::new();
    let mut locs: Vec<Location> = Vec::new();
    for m in &merged {
        covered.extend(m.tf.iter().map(|x| x.0));
        let mut hit: Vec<(usize, u32)> = m.tf.clone();
        hit.sort_by(|a, b| {
            let (ka, kb_) = (-idf_of(a.0) * f64::from(a.1), -idf_of(b.0) * f64::from(b.1));
            ka.partial_cmp(&kb_).unwrap_or(std::cmp::Ordering::Equal).then_with(|| term(a.0).cmp(term(b.0)))
        });
        let names: Vec<&str> = hit.iter().take(5).map(|x| term(x.0)).collect();
        locs.push(Location { path: m.path.clone(), start: m.start, end: m.end, why: format!("工具定位：命中 {}", names.join("、")) });
    }
    if seed_only && !texts.is_empty() {
        // 沿引用關係擴展一層：種子檔引用的檔、引用種子檔的檔（看名字有沒有出現在內容裡；不解析語法）
        let have: HashSet<String> = locs.iter().map(|x| x.path.clone()).collect();
        let mut seeds: Vec<&str> = Vec::new();
        for m in &merged {
            if !seeds.contains(&m.path.as_str()) {
                seeds.push(&m.path);
            }
        }
        seeds.truncate(SEED_FILES);
        let mut best: HashMap<&str, &Win> = HashMap::new();
        for d in &ranked {
            best.entry(d.path.as_str()).or_insert(d);
        }
        let mut added: Vec<(String, String)> = Vec::new();
        for seed in seeds {
            let seed_text = texts.get(seed).map(String::as_str).unwrap_or("");
            let seed_keys = ref_keys(seed);
            for (other, other_text) in &texts {
                if have.contains(other) || other == seed || added.iter().any(|(a, _)| a == other) {
                    continue;
                }
                if ref_keys(other).iter().any(|k| seed_text.contains(k.as_str())) {
                    added.push((other.clone(), format!("工具定位：被 {seed} 引用")));
                } else if !seed_keys.is_empty() && seed_keys.iter().any(|k| other_text.contains(k.as_str())) {
                    added.push((other.clone(), format!("工具定位：引用了 {seed}")));
                }
            }
        }
        let room = MAX_EXPAND.min(tn.max_segments.saturating_sub(locs.len()));
        for (other, why) in added.into_iter().take(room) {
            let (s0, e0) = match best.get(other.as_str()) {
                Some(b) => (b.start, b.end),
                None => (1, sizes.get(&other).copied().unwrap_or(WINDOW).min(WINDOW)),
            };
            locs.push(Location { path: other, start: s0, end: e0, why });
        }
    }
    let coverage = pyish::round_nd(covered.len() as f64 / terms.len() as f64, 3);
    let enough = locs.len() >= tn.min_segments && coverage >= tn.min_coverage && !truncated && !seed_only;
    let note = if truncated {
        "repo 太大，只看了一部分檔案".to_string()
    } else if seed_only {
        "程式角度只當種子：字面對得上的檔與它們的引用關係，其餘由定位 agent 補".to_string()
    } else if enough {
        String::new()
    } else {
        format!("命中不夠（{} 段、涵蓋 {:.0}% 的查詢詞）", locs.len(), coverage * 100.0)
    };
    Located { locations: locs, enough, coverage, note }
}

/// 各角度的定位結果（依角度給的順序；同一個角度重複給只留一筆）。repo 不存在或需求是空的回空清單。
pub fn locate_all(repo: Option<&str>, requirement_text: &str, requirement_path: Option<&str>, angles: &[String]) -> Vec<(String, Located)> {
    let mut out: Vec<(String, Located)> = Vec::new();
    let Some(repo) = repo else { return out };
    if !std::path::Path::new(repo).is_dir() || pyish::is_blank(requirement_text) {
        return out;
    }
    for a in angles {
        let mut extra: Vec<(String, String)> = Vec::new();
        if a == "requirement" {
            if let Some(rp) = requirement_path {
                let rel = posix(&pyish::relpath(rp, repo));
                // 需求檔在 repo 裡：list_files 已經會看到
                if rel.starts_with("../") || pyish::is_abs(&rel) {
                    extra.push((rel, rp.to_string()));
                }
            }
        }
        let r = locate(repo, requirement_text, a, &extra);
        match out.iter_mut().find(|(k, _)| k == a) {
            Some(x) => x.1 = r,
            None => out.push((a.clone(), r)),
        }
    }
    out
}

/// `json.dumps(…, ensure_ascii=False, indent=1, sort_keys=True)`
fn dumps_sorted(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let mut m = Map::new();
                for k in keys {
                    m.insert(k.clone(), sort(o.get(k).unwrap_or(&Value::Null)));
                }
                Value::Object(m)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    crate::recipe::dumps(&sort(v), Some(1))
}

/// CLI 的本體：stdout 的文字寫進 `out`，回傳 exit code。
pub fn run(argv: &[String], out: &mut String) -> i32 {
    let has = |name: &str| argv.iter().any(|a| a == name);
    if has("--selftest") {
        return selftest(out);
    }
    if has("-h") || has("--help") || !has("--repo") {
        out.push_str(DOC);
        out.push('\n');
        return if has("-h") || has("--help") { 0 } else { 2 };
    }
    let opt = |name: &str| -> String {
        match argv.iter().position(|a| a == name) {
            Some(i) if i + 1 < argv.len() => argv.get(i + 1).cloned().unwrap_or_default(),
            _ => String::new(),
        }
    };
    let repo = pyish::ppath(&opt("--repo"));
    // 門檻讀專案的 profile（knowledge.locate）；找不到專案或沒設定就是預設值
    set_tune(LocateTune::of(&crate::tune::project_profile()));
    let raw = opt("--requirement");
    let mut angles: Vec<String> = argv.iter().enumerate().filter(|(i, a)| *a == "--angle" && i + 1 < argv.len()).filter_map(|(i, _)| argv.get(i + 1).cloned()).collect();
    if angles.is_empty() {
        angles = ANGLES.iter().map(|a| a.to_string()).collect();
    }
    let res = locate_all(Some(&repo), &raw, None, &angles);
    if has("--json") {
        let mut m = Map::new();
        for (a, r) in &res {
            m.insert(a.clone(), r.to_json());
        }
        out.push_str(&dumps_sorted(&Value::Object(m)));
        out.push('\n');
    } else {
        for (a, r) in &res {
            let note = if r.note.is_empty() { String::new() } else { format!("（{}）", r.note) };
            out.push_str(&format!("{a}：{} 段，{}{note}\n", r.locations.len(), if r.enough { "夠" } else { "不夠" }));
            for x in &r.locations {
                out.push_str(&format!("  {}:{}-{} ｜ {}\n", x.path, x.start, x.end, x.why));
            }
        }
    }
    0
}

/// `awf locate …`
pub fn main(args: &[String]) -> i32 {
    let mut out = String::new();
    let code = run(args, &mut out);
    print!("{out}");
    code
}

// ------------------------------------------------------------------ selftest
fn put(root: &str, rel: &str, text: &str) {
    let p = pyish::pjoin(root, rel);
    if let Some(i) = p.rfind('/') {
        let _ = std::fs::create_dir_all(p.get(..i).unwrap_or("."));
    }
    let _ = std::fs::write(&p, text.as_bytes());
}

fn slurp(root: &str, rel: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(pyish::pjoin(root, rel)).unwrap_or_default()).into_owned()
}

/// `--selftest`：和 0.x 的 Python 版同樣的檢查項，逐項印結果。
pub fn selftest(out: &mut String) -> i32 {
    let mut fails = 0usize;
    out.push_str("locate selftest\n");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("awf-locate.{}-{nanos}", std::process::id()));
    let tmp_s = tmp.to_string_lossy().replace('\\', "/");
    let repo = pyish::pjoin(&tmp_s, "repo");
    {
        let mut check = |name: &str, ok: bool, detail: String| {
            let tail = if ok { String::new() } else { format!("：{}", pyish::take(&detail, 400)) };
            out.push_str(&format!("  [{}] {name}{tail}\n", if ok { "通過" } else { "失敗" }));
            if !ok {
                fails += 1;
            }
        };
        let show = |r: &Located| r.to_json().to_string();
        let filler: String = (0..60).map(|i| format!("def helper_{i}():\n    return {i}\n\n")).collect();
        let export_body = format!("{filler}def export_orders_csv(tenant, date_from=None, date_to=None):\n    \"\"\"訂單匯出：依日期區間篩選，時間欄位輸出 UTC。\"\"\"\n    rows = load_orders(tenant)\n    return to_csv(rows)\n{filler}");
        put(&repo, "services/orders/export.py", &export_body);
        put(&repo, "services/orders/routes.py", "ROUTES = {\n    'GET /exports/orders.csv': 'export_orders_csv',\n}\n\ndef register(app):\n    for k, v in ROUTES.items():\n        app.add(k, v)\n");
        put(&repo, "services/orders/batch.py", "def enqueue_export(tenant):\n    \"\"\"大量匯出改走背景批次：排入佇列，完成後通知申請人。\"\"\"\n    queue.put(('export_orders_csv', tenant))\n");
        let invoices: String = (0..80).map(|i| format!("def invoice_{i}():\n    return {i}\n\n")).collect();
        put(&repo, "services/billing/invoice.py", &invoices);
        put(&repo, "docs/adr/0003-export.md", "# ADR 3：訂單匯出格式\n\n匯出一律用 CSV，時間欄位輸出 UTC。\n\n## 決策\n\n日期區間由呼叫端指定。\n");
        put(&repo, "specs/export/spec.md", "# 訂單匯出\n\n## 驗收條件\n\n- AC1 匯出檔名帶日期區間\n- AC2 超過同步上限改走背景批次\n");
        put(&repo, "docs/onboarding.md", "# 新人須知\n\n先裝開發環境，再跑測試。\n");
        put(&repo, ".env", "SECRET=訂單匯出 export_orders_csv\n");
        put(&repo, "package-lock.json", "{\"orders\": \"export_orders_csv 訂單匯出\"}\n");
        let _ = std::fs::write(pyish::pjoin(&repo, "blob.bin"), b"\0\x01export_orders_csv".repeat(50));
        let req = "訂單匯出支援日期區間與客戶時區：GET /exports/orders.csv 接受 from、to；大量匯出改走背景批次，完成後通知申請人。";

        let r = locate(&repo, req, "code-contracts", &[]);
        let paths: Vec<&str> = r.locations.iter().map(|x| x.path.as_str()).collect();
        check("程式角度：命中匯出、路由、批次三個檔，不含文件",
            ["services/orders/export.py", "services/orders/routes.py", "services/orders/batch.py"].iter().all(|p| paths.contains(p)) && !paths.iter().any(|p| p.ends_with(".md")), show(&r));
        check("程式角度：無關的檔（billing）不在前面", !paths.iter().take(3).any(|p| *p == "services/billing/invoice.py"), format!("{paths:?}"));
        check("行段指到函式所在的位置，不是整個檔",
            r.locations.iter().any(|x| x.path == "services/orders/export.py" && x.start > 100 && x.end - x.start < MAX_SEGMENT_LINES), show(&r));
        check("機敏檔、lock 檔、二進位檔不看", !paths.iter().any(|p| [".env", "package-lock.json", "blob.bin"].contains(p)), format!("{paths:?}"));
        check("程式角度只當種子：enough=false 並說明；why 都以「工具定位：」開頭",
            !r.enough && r.note.contains("種子") && r.locations.iter().all(|x| x.why.starts_with("工具定位：")), show(&r));
        put(&repo, "services/orders/loader.py", "def load_orders(tenant):\n    return []\n");
        put(&repo, "services/orders/export.py", &format!("from .loader import load_orders\n{}", slurp(&repo, "services/orders/export.py")));
        put(&repo, "tests/test_misc.py", "from services.orders.export import export_orders_csv\n\ndef test_it():\n    assert True\n");
        let x = locate(&repo, req, "code-contracts", &[]);
        let why: HashMap<&str, &str> = x.locations.iter().map(|i| (i.path.as_str(), i.why.as_str())).collect();
        check("沿引用關係擴展：種子檔 import 的檔與引用種子檔的檔都列進來，並寫明關係",
            why.get("services/orders/loader.py").is_some_and(|w| w.contains("被 services/orders/export.py 引用")) && why.contains_key("tests/test_misc.py"), show(&x));
        check("擴展不會把無關的檔帶進來", !why.contains_key("services/billing/invoice.py"), show(&x));
        let _ = std::fs::remove_file(pyish::pjoin(&repo, "services/orders/loader.py"));
        let _ = std::fs::remove_file(pyish::pjoin(&repo, "tests/test_misc.py"));
        let _ = std::fs::remove_dir(pyish::pjoin(&repo, "tests"));
        put(&repo, "services/orders/export.py", &slurp(&repo, "services/orders/export.py").replacen("from .loader import load_orders\n", "", 1));
        check("行段格式：path 是相對 posix 路徑、起訖是正整數、每段不超過上限",
            r.locations.iter().all(|x| !pyish::is_abs(&x.path) && !x.path.contains('\\') && 1 <= x.start && x.start <= x.end && x.end - x.start + 1 <= MAX_SEGMENT_LINES)
                && r.locations.len() <= MAX_SEGMENTS, show(&r));

        let d = locate(&repo, req, "specs-adr", &[]);
        let dp: Vec<&str> = d.locations.iter().map(|x| x.path.as_str()).collect();
        check("文件角度：只看 specs／docs 的文件，命中 spec 與 ADR",
            dp.contains(&"specs/export/spec.md") && dp.contains(&"docs/adr/0003-export.md") && dp.iter().all(|p| p.ends_with(".md")), show(&d));
        check("文件角度：只有兩個檔時算不夠（交給定位 agent 補）", !d.enough && d.note.contains("命中不夠"), show(&d));
        for i in 0..3 {
            put(&repo, &format!("docs/adr/001{i}-export-{i}.md"), &format!("# ADR 1{i}：訂單匯出的日期區間（{i}）\n\n大量匯出改走背景批次，完成後通知申請人。\n"));
        }
        let d2 = locate(&repo, req, "specs-adr", &[]);
        check("文件角度：命中夠多、涵蓋夠 → enough=true（不用派定位 agent）", d2.enough && d2.locations.len() >= MIN_SEGMENTS, show(&d2));
        for i in 0..3 {
            let _ = std::fs::remove_file(pyish::pjoin(&repo, &format!("docs/adr/001{i}-export-{i}.md")));
        }

        check("可重現：同樣的輸入輸出完全相同", locate(&repo, req, "code-contracts", &[]) == r && locate(&repo, req, "specs-adr", &[]) == d, String::new());
        let z = locate(&repo, "完全無關的主題：量子糾纏與黑洞蒸發 xyzzy", "code-contracts", &[]);
        check("沒命中：回空、enough=false 並說明", z.locations.is_empty() && !z.enough && !z.note.is_empty(), show(&z));
        check("空的需求、不認得的角度：回空", locate(&repo, "", "code-contracts", &[]).locations.is_empty() && !locate(&repo, req, "nope", &[]).enough, String::new());
        let e = locate(&repo, "export_orders_csv", "code-contracts", &[]);
        check("只有一個英文識別字也找得到（中英混合的 repo）", e.locations.iter().any(|x| x.path == "services/orders/export.py"), show(&e));

        put(&tmp_s, "ws/requirement.md", &format!("# 需求\n\n{req}\n"));
        let reqf = pyish::pjoin(&tmp_s, "ws/requirement.md");
        let all: Vec<String> = ANGLES.iter().map(|a| a.to_string()).collect();
        let allr = locate_all(Some(&repo), req, Some(&reqf), &all);
        let req_res = allr.iter().find(|(a, _)| a == "requirement").map(|(_, r)| r.clone()).unwrap_or_default();
        check("locate_all：三個角度都有結果；需求檔在 repo 外也會列進 requirement 角度",
            allr.len() == ANGLES.len() && ANGLES.iter().all(|a| allr.iter().any(|(k, _)| k == a)) && req_res.locations.iter().any(|x| x.path.ends_with("requirement.md")), show(&req_res));
        check("locate_all：repo 不存在或需求是空的 → 空結果",
            locate_all(Some(&pyish::pjoin(&tmp_s, "nope")), req, None, &all).is_empty() && locate_all(Some(&repo), "  ", None, &all).is_empty(), String::new());
        let _ = std::fs::write(pyish::pjoin(&repo, "big.py"), b"x = 1\n".repeat(200000));
        check("太大的檔略過", !locate(&repo, "x", "code-contracts", &[]).locations.iter().any(|x| x.path == "big.py"), String::new());
    }
    let _ = std::fs::remove_dir_all(&tmp);
    out.push_str(&format!("locate selftest：{}\n", if fails == 0 { "全部通過".to_string() } else { format!("{fails} 項失敗") }));
    if fails == 0 { 0 } else { 1 }
}
