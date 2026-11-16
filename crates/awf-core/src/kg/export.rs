//! `awf kg export`：把圖寫成 Obsidian 可以開的 vault（`<knowledge.dir>/vault/`）。
//!
//! 每個能力、需求、決策、模組（有程式檔的目錄）一頁，加上 `hot.md` 與各區的索引。頁面只有結構化欄位、
//! `[[連結]]` 與 `路徑:行號`，不放原文（標題只放在 frontmatter 的欄位裡）。內容只由圖決定，可重現。
//!
//! 整個由工具重寫，但只動自己產生的檔（frontmatter 有 [`MARK`] 的）；`notes/` 是人寫的，完全不碰。

use super::build::Layout;
use super::query::{Edge, Graph, Node};
use crate::kb::{self, pyish};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// 工具產生的頁面在 frontmatter 裡的標記；清舊頁時只刪有這一行的檔。
pub const MARK: &str = "generated_by: awf kg export";
/// 工具管的子目錄。
pub const AREAS: [&str; 4] = ["能力", "需求", "決策", "模組"];

pub struct ExportReport {
    pub dir: String,
    pub pages: usize,
    /// 在工具管的位置上、但不是工具產生的檔（沒有動）。
    pub kept: Vec<String>,
}

/// 檔名裡不能用的字元換成 `-`（Obsidian 連結與 Windows 檔名）。
fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']') || c.is_control() { '-' } else { c }).collect();
    let t = t.split('/').map(|seg| if seg.is_empty() || seg == "." || seg == ".." { "-" } else { seg }).collect::<Vec<_>>().join("/");
    if t.is_empty() { "-".to_string() } else { t }
}

fn short(id: &str) -> &str {
    id.split_once(':').map(|x| x.1).unwrap_or(id)
}

fn page_of(id: &str) -> Option<String> {
    let (kind, rest) = id.split_once(':')?;
    Some(match kind {
        "cap" => format!("能力/{}", safe(rest)),
        "req" => format!("需求/{}", safe(rest)),
        "scn" => format!("需求/{}", safe(rest.split('#').next().unwrap_or(rest))),
        "dec" => format!("決策/{}", safe(rest)),
        _ => return None,
    })
}

fn module_page(dir: &str) -> String {
    if dir.is_empty() { "模組/_root".to_string() } else { format!("模組/{}", safe(dir)) }
}

fn loc(n: &Node) -> String {
    if n.path.is_empty() {
        n.id.clone()
    } else if n.end == 0 {
        n.path.clone()
    } else if n.start == n.end {
        format!("{}:{}", n.path, n.start)
    } else {
        format!("{}:{}-{}", n.path, n.start, n.end)
    }
}

fn quoted(s: &str) -> String {
    pyish::json_str(&kb::neutralize(&s.replace(['\n', '\r'], " ")))
}

struct Page {
    front: Vec<(String, String)>,
    body: Vec<String>,
}

impl Page {
    fn new(kind: &str, id: &str) -> Page {
        Page { front: vec![("kind".into(), kind.into()), ("id".into(), quoted(id))], body: Vec::new() }
    }

    fn field(&mut self, k: &str, v: String) {
        self.front.push((k.to_string(), v));
    }

    fn section(&mut self, title: &str, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        self.body.push(String::new());
        self.body.push(format!("## {title}"));
        self.body.push(String::new());
        self.body.extend(lines);
    }

    fn text(&self) -> String {
        let mut out = vec!["---".to_string(), MARK.to_string()];
        out.extend(self.front.iter().map(|(k, v)| format!("{k}: {v}")));
        out.push("---".to_string());
        out.extend(self.body.iter().cloned());
        out.join("\n") + "\n"
    }
}

fn nodes_of(g: &Graph, kind: &str) -> Vec<Node> {
    let ids: Vec<String> = (|| -> rusqlite::Result<Vec<String>> {
        let mut st = g.con.prepare("SELECT id FROM nodes WHERE kind=? ORDER BY id")?;
        let rows = st.query_map([kind], |r| r.get::<_, String>(0))?;
        rows.collect()
    })()
    .unwrap_or_default();
    ids.iter().filter_map(|i| g.node(i)).collect()
}

fn link_line(g: &Graph, id: &str, ed: &Edge) -> String {
    let tail = format!("（{}・{}・{}）", ed.kind, ed.strength, ed.provenance);
    match (page_of(id), g.node(id)) {
        (Some(p), Some(n)) => format!("- [[{p}]] `{}` {tail}", loc(&n)),
        (Some(p), None) => format!("- [[{p}]] {tail}"),
        (None, Some(n)) if n.kind == "file" || n.kind == "sym" => format!("- `{}` [[{}]] {tail}", loc(&n), module_page(super::resolve::dir_of(&n.path))),
        (None, Some(n)) => format!("- `{}` {tail}", loc(&n)),
        (None, None) => format!("- `{}` {tail}（不在圖裡）", short(id)),
    }
}

/// 一個需求（含它的情境）連出去與連進來的東西，依種類分組。
fn req_links(g: &Graph, ids: &[String]) -> BTreeMap<&'static str, BTreeSet<String>> {
    let mut out: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    for id in ids {
        for ed in g.out_edges(id) {
            let group = match ed.kind.as_str() {
                "contains" | "changes" => continue,
                "req-run" => "run",
                "ac-file" => "檔案",
                "ac-test" => "測試",
                "ac-commit" => "commit",
                "ac-spec" => "規格小節",
                _ => "其他",
            };
            out.entry(group).or_default().insert(link_line(g, &ed.dst, &ed));
        }
        for ed in g.in_edges(id) {
            let group = match ed.kind.as_str() {
                "contains" => continue,
                "marks" => "標記",
                "decision-req" => "決策",
                "changes" => "進行中的變更",
                "mentions" | "links" if ed.provenance.starts_with("note:") => "筆記",
                "mentions" | "links" => "文件小節",
                _ => "其他",
            };
            let line = if group == "筆記" {
                let path = g.node(&ed.src).map(|n| n.path).unwrap_or_default();
                let name = path.rsplit_once("/vault/").map(|x| x.1).unwrap_or(&path).trim_end_matches(".md").to_string();
                format!("- [[{}]]（{}・{}）", safe(&name), ed.kind, ed.strength)
            } else {
                link_line(g, &ed.src, &ed)
            };
            out.entry(group).or_default().insert(line);
        }
    }
    out
}

fn build_pages(g: &Graph) -> BTreeMap<String, String> {
    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    let mut index: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut busiest: Vec<(usize, String)> = Vec::new();

    for cap in nodes_of(g, "cap") {
        let mut p = Page::new("capability", short(&cap.id));
        p.field("spec", quoted(&loc(&cap)));
        let reqs: Vec<String> = g.out_edges(&cap.id).into_iter().filter(|e| e.kind == "contains").filter_map(|e| page_of(&e.dst)).map(|x| format!("- [[{x}]]")).collect();
        p.section("需求", reqs);
        let page = page_of(&cap.id).unwrap_or_default();
        index.entry("能力").or_default().push(format!("- [[{page}]]"));
        pages.insert(format!("{page}.md"), p.text());
    }

    for req in nodes_of(g, "req") {
        let mut p = Page::new("requirement", short(&req.id));
        p.field("title", quoted(&req.name));
        p.field("state", req.sub.clone());
        p.field("spec", quoted(&loc(&req)));
        if let Some(c) = g.in_edges(&req.id).into_iter().find(|e| e.kind == "contains" && e.src.starts_with("cap:")) {
            p.body.push(format!("能力：[[{}]]", page_of(&c.src).unwrap_or_default()));
        }
        let mut ids = vec![req.id.clone()];
        let mut scns = Vec::new();
        for ed in g.out_edges(&req.id) {
            if ed.kind == "contains" {
                if let Some(n) = g.node(&ed.dst) {
                    scns.push(format!("- `{}` `{}`", short(&n.id), loc(&n)));
                }
                ids.push(ed.dst);
            }
        }
        p.section("情境", scns);
        let links = req_links(g, &ids);
        busiest.push((links.values().map(BTreeSet::len).sum(), page_of(&req.id).unwrap_or_default()));
        for group in ["進行中的變更", "run", "規格小節", "檔案", "標記", "測試", "commit", "決策", "文件小節", "筆記", "其他"] {
            if let Some(lines) = links.get(group) {
                p.section(group, lines.iter().cloned().collect());
            }
        }
        let page = page_of(&req.id).unwrap_or_default();
        index.entry("需求").or_default().push(format!("- [[{page}]]"));
        pages.insert(format!("{page}.md"), p.text());
    }

    let mut recent: Vec<(usize, String)> = Vec::new();
    for dec in nodes_of(g, "dec") {
        let mut p = Page::new("decision", short(&dec.id));
        if !dec.sub.is_empty() {
            p.field("scope", quoted(&dec.sub));
        }
        if !dec.path.is_empty() {
            p.field("record", quoted(&loc(&dec)));
        }
        let mut lines = Vec::new();
        for ed in g.out_edges(&dec.id) {
            lines.push(link_line(g, &ed.dst, &ed));
        }
        p.section("連結", lines);
        let page = page_of(&dec.id).unwrap_or_default();
        index.entry("決策").or_default().push(format!("- [[{page}]]"));
        recent.push((dec.start, page.clone()));
        pages.insert(format!("{page}.md"), p.text());
    }

    // 模組：有程式檔的目錄
    let mut mods: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    for f in nodes_of(g, "file") {
        if !matches!(f.sub.as_str(), "doc" | "note") {
            mods.entry(super::resolve::dir_of(&f.path).to_string()).or_default().push(f);
        }
    }
    for (dir, files) in &mods {
        let mut p = Page::new("module", if dir.is_empty() { "." } else { dir });
        p.field("files", files.len().to_string());
        let (mut uses, mut used_by, mut reqs) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut lines = Vec::new();
        for f in files {
            let syms: i64 = g.con.query_row("SELECT COUNT(*) FROM nodes WHERE owner=? AND kind='sym'", [&f.path], |r| r.get(0)).unwrap_or(0);
            lines.push(format!("- `{}`（{}，{syms} 個符號）", loc(f), f.sub));
            for ed in g.out_edges(&f.id) {
                if ed.kind == "imports" {
                    let d = super::resolve::dir_of(short(&ed.dst)).to_string();
                    if &d != dir {
                        uses.insert(format!("- [[{}]]", module_page(&d)));
                    }
                }
            }
            for ed in g.in_edges(&f.id) {
                match ed.kind.as_str() {
                    "imports" => {
                        let d = super::resolve::dir_of(short(&ed.src)).to_string();
                        if &d != dir {
                            used_by.insert(format!("- [[{}]]", module_page(&d)));
                        }
                    }
                    "ac-file" | "ac-test" => {
                        if let Some(pg) = page_of(&ed.src) {
                            reqs.insert(format!("- [[{pg}]]"));
                        }
                    }
                    _ => {}
                }
            }
            let marks: Vec<String> = (|| -> rusqlite::Result<Vec<String>> {
                let mut st = g.con.prepare_cached("SELECT e.dst FROM edges e JOIN nodes s ON s.id = e.src WHERE e.kind='marks' AND s.path=? ORDER BY e.dst")?;
                let rows = st.query_map([&f.path], |r| r.get::<_, String>(0))?;
                rows.collect()
            })()
            .unwrap_or_default();
            reqs.extend(marks.iter().filter_map(|m| page_of(m)).map(|pg| format!("- [[{pg}]]")));
        }
        p.section("檔案", lines);
        p.section("相關需求", reqs.into_iter().collect());
        p.section("用到的模組", uses.into_iter().collect());
        p.section("被這些模組用到", used_by.into_iter().collect());
        let page = module_page(dir);
        index.entry("模組").or_default().push(format!("- [[{page}]]"));
        pages.insert(format!("{page}.md"), p.text());
    }

    for area in AREAS {
        let mut p = Page::new("index", area);
        let mut lines = index.remove(area).unwrap_or_default();
        lines.sort();
        p.field("count", lines.len().to_string());
        p.body.extend(lines);
        pages.insert(format!("{area}/_index.md"), p.text());
    }

    let count = |sql: &str| -> i64 { g.con.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    let mut hot = Page::new("hot", "hot");
    for (k, sql) in [
        ("files", "SELECT COUNT(*) FROM files"),
        ("requirements", "SELECT COUNT(*) FROM nodes WHERE kind='req'"),
        ("decisions", "SELECT COUNT(*) FROM nodes WHERE kind='dec'"),
        ("symbols", "SELECT COUNT(*) FROM nodes WHERE kind='sym'"),
        ("edges", "SELECT COUNT(*) FROM edges"),
        ("unresolved", "SELECT COUNT(*) FROM unresolved"),
        ("broken_links", "SELECT COUNT(*) FROM edges e WHERE NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=e.dst) OR NOT EXISTS(SELECT 1 FROM nodes n WHERE n.id=e.src)"),
    ] {
        hot.field(k, count(sql).to_string());
    }
    hot.body.push("索引：[[index]]".to_string());
    let changes: Vec<String> = nodes_of(g, "chg").iter().map(|c| format!("- `{}` `{}`", short(&c.id), c.path)).collect();
    hot.section("進行中的變更", changes);
    recent.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hot.section("最近的決策", recent.iter().take(5).map(|r| format!("- [[{}]]", r.1)).collect());
    busiest.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hot.section("連結最多的需求", busiest.iter().filter(|b| b.0 > 0).take(10).map(|b| format!("- [[{}]]（{} 條）", b.1, b.0)).collect());
    pages.insert("hot.md".to_string(), hot.text());

    let mut idx = Page::new("index", "index");
    idx.body.push("[[hot]]".to_string());
    idx.section("分區", AREAS.iter().map(|a| format!("- [[{a}/_index]]")).collect());
    pages.insert("index.md".to_string(), idx.text());
    pages
}

fn is_ours(p: &Path) -> bool {
    std::fs::read_to_string(p).is_ok_and(|t| t.lines().take(3).any(|l| l == MARK))
}

/// 清掉工具管的目錄裡自己產生的舊頁；不是自己產生的留著並回報。
fn clean(dir: &Path, rel: &str, kept: &mut Vec<String>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ents: Vec<_> = rd.flatten().collect();
    ents.sort_by_key(|x| x.file_name());
    for ent in ents {
        let name = ent.file_name().to_string_lossy().into_owned();
        let p = ent.path();
        let r = format!("{rel}/{name}");
        match ent.file_type() {
            Ok(t) if t.is_dir() => {
                clean(&p, &r, kept, depth + 1);
                let _ = std::fs::remove_dir(&p);
            }
            Ok(t) if t.is_file() && is_ours(&p) => {
                let _ = std::fs::remove_file(&p);
            }
            _ => kept.push(r),
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("建不出目錄 {}（{err}）", parent.display()))?;
    }
    let tmp = path.with_extension(format!("md.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("寫不進 {}（{err}）", path.display())
    })
}

pub fn export(g: &Graph, lay: &Layout, out: Option<&str>) -> Result<ExportReport, String> {
    let dir = match out.filter(|o| !o.is_empty()) {
        Some(o) if pyish::is_abs(o) => pyish::ppath(o),
        Some(o) => pyish::pjoin(&lay.root, o),
        None => pyish::pjoin(&lay.dir, "vault"),
    };
    let base = Path::new(&dir);
    std::fs::create_dir_all(base.join("notes")).map_err(|err| format!("建不出目錄 {dir}（{err}）"))?;
    let pages = build_pages(g);
    let mut kept = Vec::new();
    for area in AREAS {
        clean(&base.join(area), area, &mut kept, 0);
    }
    let mut written = 0;
    for (rel, text) in &pages {
        let p = base.join(rel);
        if p.is_file() && !is_ours(&p) {
            kept.push(rel.clone());
            continue;
        }
        write_atomic(&p, text)?;
        written += 1;
    }
    kept.sort();
    kept.dedup();
    Ok(ExportReport { dir: kb::to_posix(&pyish::relpath(&dir, &lay.root)), pages: written, kept })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe("billing/export-invoice#csv"), "billing/export-invoice-csv");
        assert_eq!(safe("../x"), "-/x");
        assert_eq!(page_of("scn:billing/export#csv").as_deref(), Some("需求/billing/export"));
        assert_eq!(module_page(""), "模組/_root");
    }
}
