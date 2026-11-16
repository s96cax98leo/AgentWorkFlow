//! 資料包帶知識圖的結果（D7）。圖存在而且能開才做；沒有圖、圖開不了、查不到東西時，資料包與 `pack-args.json` 和沒有這個功能時逐字相同。
//!
//! 各段多帶的內容（都只有 `路徑:起訖行` 加一句理由，有條數上限，包在資料標籤裡，同一張圖重跑結果相同）：
//! - plan：程式行段、相關決策、這個需求會碰到的既有需求與情境；程式角度命中夠時把 `located` 標成夠用。
//! - design：要改的檔案現在被哪些需求、測試連著。
//! - build：每個子任務的檔案的呼叫者與相依（寫進子任務資料包）。
//! - deliver：受影響的交付文件小節（`deliverables.affected` 只會多列；另在資料包列出理由）。
//!
//! 這裡只讀圖，不建圖。

use super::{join, load_json, posix};
use crate::kg::{self, flow, flow::Loc};
use crate::locate;
use serde_json::{json, Map, Value};

/// 程式角度（和 `locate.rs` 的 `SEED_ONLY` 同一組）：工具自己的定位只當種子，有圖而且命中夠才標成夠用。
pub const CODE_ANGLES: [&str; 1] = ["code-contracts"];

pub struct KgCtx {
    pub g: kg::Graph,
    /// 目標 repo 的檔在圖裡的前綴：就是專案根目錄時是空字串，在專案底下時是 `子目錄/`，是 `knowledge.repos` 的一個時是 `<id>:`；
    /// 圖沒有收這個 repo（或沒有指定 repo）時是 None，要用 repo 相對路徑的功能就不做。
    pub prefix: Option<String>,
}

fn canon(p: &str) -> Option<String> {
    std::fs::canonicalize(p).ok().map(|x| x.to_string_lossy().replace('\\', "/"))
}

/// 開專案的圖並算出目標 repo 在圖裡的前綴。沒有圖回 None。
pub fn open(root: &str, repo: Option<&str>) -> Option<KgCtx> {
    let g = flow::open_root(root)?;
    let prefix = repo.and_then(canon).and_then(|r| {
        let base = canon(&g.root)?;
        if r == base {
            return Some(String::new());
        }
        if let Some(rest) = r.strip_prefix(&format!("{base}/")) {
            return Some(format!("{rest}/"));
        }
        g.repos.iter().find(|(_, display)| canon(&if display.starts_with('/') { display.clone() } else { format!("{base}/{display}") }).as_deref() == Some(r.as_str())).map(|(id, _)| format!("{id}:"))
    });
    Some(KgCtx { g, prefix })
}

impl KgCtx {
    /// repo 相對路徑 → 圖裡的路徑。
    fn to_graph(&self, rel: &str) -> Option<String> {
        let rel = posix(rel);
        let rel = rel.trim_start_matches("./");
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return None;
        }
        self.prefix.as_ref().map(|p| format!("{p}{rel}"))
    }

    /// 圖裡的路徑 → repo 相對路徑（不在這個 repo 裡回 None）。
    fn to_repo(&self, path: &str) -> Option<String> {
        let p = self.prefix.as_ref()?;
        if p.is_empty() {
            return kg::build::split_repo(path).is_none().then(|| path.to_string());
        }
        path.strip_prefix(p.as_str()).map(str::to_string)
    }
}

/// plan：資料包多一節，並在條件都成立時把程式角度的 `located` 標成夠用。回傳（資料包的行, 說明）。
pub fn plan(kx: &KgCtx, text: &str, names: &[String], located: Option<&mut Map<String, Value>>, reused: &dyn Fn(&str) -> bool) -> (Vec<String>, Vec<String>) {
    let facts = flow::plan_facts(&kx.g, text, names);
    let mut notes = Vec::new();
    if let Some(located) = located {
        for angle in CODE_ANGLES {
            if reused(angle) {
                continue;
            }
            // 只看目標 repo 裡的；圖的判斷條件見 PlanFacts::code_enough
            let direct: Vec<Loc> = facts.direct.iter().filter_map(|l| kx.to_repo(&l.path).map(|p| Loc { path: p, ..l.clone() })).collect();
            let mine = flow::PlanFacts { direct: direct.clone(), linked: facts.linked, ..Default::default() };
            if !mine.code_enough() {
                continue;
            }
            // 再加一條：工具自己（awf locate 的詞比對）找到的種子至少有一個檔和圖的結果相同，兩種方法互相印證才算
            let tool: Vec<Value> = located.get(angle).and_then(|x| x.get("locations")).and_then(Value::as_array).cloned().unwrap_or_default();
            let tool_path = |v: &Value| v.get("path").and_then(Value::as_str).map(posix).unwrap_or_default();
            if !tool.iter().any(|t| direct.iter().any(|d| d.path == tool_path(t))) {
                continue;
            }
            let mut locs: Vec<Value> = direct.iter().map(|d| json!({"path": d.path, "start": d.start, "end": d.end, "why": d.why})).collect();
            for t in tool {
                let dup = locs.iter().any(|l| tool_path(l) == tool_path(&t) && l.get("start") == t.get("start"));
                if !dup && locs.len() < locate::MAX_SEGMENTS {
                    locs.push(t);
                }
            }
            locs.truncate(locate::MAX_SEGMENTS);
            let files: std::collections::BTreeSet<&str> = direct.iter().map(|d| d.path.as_str()).collect();
            notes.push(format!("知識圖：{angle} 命中夠（直接命中 {} 個符號、{} 個檔，接得到需求，和工具定位有重疊），不派定位 agent", direct.len(), files.len()));
            located.insert(angle.to_string(), json!({"locations": locs, "enough": true, "source": "kg"}));
        }
    }
    let lines = flow::section(
        "知識圖查到的位置（只讀這些行段，帶 offset／limit；要再找用 `awf kg locate \"<關鍵詞>\"`）",
        &[("程式行段", &facts.code), ("相關決策", &facts.decisions), ("這個需求會碰到的既有需求與情境", &facts.reqs)],
    );
    if !lines.is_empty() {
        notes.push(format!("知識圖：程式 {} 段、決策 {} 筆、既有需求與情境 {} 個", facts.code.len(), facts.decisions.len(), facts.reqs.len()));
    }
    (lines, notes)
}

/// design：要改的檔（plan 段定位到的程式檔；沒有就用需求文字找到的）現在被哪些需求、測試連著。
pub fn design(kx: &KgCtx, run: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let mut paths: Vec<String> = Vec::new();
    let plan = load_json(&join(run, "plan.json"));
    if let Some(by_angle) = plan.as_ref().and_then(|p| p.get("locations")).and_then(Value::as_object) {
        for locs in by_angle.values() {
            for l in locs.as_array().into_iter().flatten() {
                if let Some(p) = l.get("path").and_then(Value::as_str).and_then(|p| kx.to_graph(p)) {
                    paths.push(p);
                }
            }
        }
    }
    let mut files: Vec<String> = flow::known_files(&kx.g, &paths).into_iter().filter(|f| !kx.g.is_test(f)).collect();
    if files.is_empty() {
        // plan 段沒有留定位結果：用需求文字直接命中的程式檔
        for l in flow::plan_facts(&kx.g, text, &[]).direct {
            if !files.contains(&l.path) {
                files.push(l.path);
            }
        }
    }
    files.truncate(flow::MAX_LINKS);
    let (reqs, tests) = flow::file_links(&kx.g, &files);
    let lines = flow::section("知識圖：要改的檔現在連著的需求與測試（改之前先看；只讀這些行段）", &[("需求與情境", &reqs), ("測試", &tests)]);
    let notes = if lines.is_empty() { Vec::new() } else { vec![format!("知識圖：看了 {} 個檔，連著 {} 個需求或情境、{} 個測試", files.len(), reqs.len(), tests.len())] };
    (lines, notes)
}

/// build：一個子任務擁有的檔（圖裡有的；目錄或 glob 取固定前綴底下的程式檔）的呼叫者與相依。回傳要接在子任務資料包後面的行。
pub fn task(kx: &KgCtx, owns: &[String]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for o in owns {
        let fixed: String = posix(o).split(['*', '?']).next().unwrap_or_default().to_string();
        let Some(p) = kx.to_graph(&fixed) else { continue };
        let exact = flow::known_files(&kx.g, std::slice::from_ref(&p));
        let found = if !exact.is_empty() && fixed == posix(o) {
            exact
        } else {
            // 目錄或 glob：前綴要停在目錄邊界（`src/exp*` 不拿來當前綴）
            let dir = if p.ends_with('/') || p.ends_with(':') { p.clone() } else if fixed == posix(o) { format!("{p}/") } else { p.rsplit_once('/').map(|x| format!("{}/", x.0)).unwrap_or_default() };
            flow::files_under(&kx.g, &dir, 8)
        };
        for f in found {
            if !files.contains(&f) && files.len() < 8 {
                files.push(f);
            }
        }
    }
    let (callers, tests, deps) = flow::callers_and_deps(&kx.g, &files);
    flow::section("這些檔的呼叫者與相依（知識圖；只讀這些行段，要再找用 `awf kg impact <檔>`）", &[("呼叫者（改介面會影響它們）", &callers), ("會受影響的測試", &tests), ("相依（它用到的）", &deps)])
}

/// 把一節接在資料包後面。
pub fn append(text: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return text.to_string();
    }
    format!("{}\n\n{}\n", text.trim_end_matches('\n'), lines.join("\n"))
}
