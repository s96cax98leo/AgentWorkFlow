//! 全域來源：規格層、決策記憶、連結帳本、git log，以及文件與程式標記裡提到的 id。
//! 這些每次建圖整個重算（`owner` 以 `@` 開頭）。

use super::build::{add_terms, cap_terms, Layout};
use super::links::{self, End};
use super::resolve::{dir_of, join_norm, World};
use super::store::{self, e, NodeRow};
use crate::kb::{self, pyish};
use crate::spec;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// git log 最多看幾個 commit。
pub const GIT_LOG_LIMIT: usize = 500;
/// id 可以帶的型別前綴。
pub const PREFIXES: [&str; 11] = ["req:", "scn:", "cap:", "ac:", "file:", "sym:", "sec:", "commit:", "run:", "dec:", "chg:"];

fn exists(con: &Connection, id: &str) -> bool {
    con.prepare_cached("SELECT 1 FROM nodes WHERE id=?").and_then(|mut s| s.exists([id])).unwrap_or(false)
}

/// 把人或工具寫的 id 對到節點：已經帶前綴的原樣查，沒帶的依序試情境、需求、能力、決策、變更。
pub fn resolve_id(con: &Connection, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if PREFIXES.iter().any(|p| raw.starts_with(p)) {
        return exists(con, raw).then(|| raw.to_string());
    }
    ["scn:", "req:", "cap:", "dec:", "chg:", "ac:"].iter().map(|p| format!("{p}{raw}")).find(|id| exists(con, id))
}

fn git(root: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(["-c", "core.quotepath=false"]).args(args).stdin(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 檔案 ↔ commit：HEAD 變了才重讀 git log。沒有 git 或不是 repo 就清空。
pub fn git_facts(con: &Connection, root: &str, full: bool, before: &BTreeMap<String, String>, notes: &mut Vec<String>) -> Result<(), String> {
    let head = git(root, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    if !full && !head.is_empty() && before.get("git_head") == Some(&head) {
        return Ok(());
    }
    con.execute("DELETE FROM f_commits", []).map_err(e)?;
    store::set_meta(con, "git_head", &head)?;
    if head.is_empty() {
        return Ok(());
    }
    let n = GIT_LOG_LIMIT.to_string();
    let Some(log) = git(root, &["log", "--no-merges", "-n", &n, "--format=%x01%H", "--name-only", "--relative"]) else {
        notes.push("git log 讀不到，這次沒有 commit 的連結".to_string());
        return Ok(());
    };
    let mut st = con.prepare_cached("INSERT OR IGNORE INTO f_commits VALUES(?,?,?)").map_err(e)?;
    let (mut sha, mut seq) = (String::new(), 0i64);
    for line in log.lines() {
        if let Some(s) = line.strip_prefix('\u{1}') {
            sha = s.trim().to_string();
            seq += 1;
        } else if !line.trim().is_empty() && !sha.is_empty() {
            st.execute(params![sha, kb::to_posix(line.trim()), seq]).map_err(e)?;
        }
    }
    Ok(())
}

pub fn git_edges(con: &Connection, w: &World) -> Result<(), String> {
    let mut rows: Vec<(String, String)> = Vec::new();
    {
        let mut st = con.prepare("SELECT sha, path FROM f_commits ORDER BY seq, path").map_err(e)?;
        let mut q = st.query([]).map_err(e)?;
        while let Some(r) = q.next().map_err(e)? {
            rows.push((r.get(0).map_err(e)?, r.get(1).map_err(e)?));
        }
    }
    for (sha, path) in rows {
        if !w.files.contains_key(&path) {
            continue;
        }
        let id = format!("commit:{sha}");
        store::put_node(con, &NodeRow { id: &id, kind: "commit", sub: "git", name: &sha[..sha.len().min(12)], path: "", start: 0, end: 0, ntok: 0, owner: "@git" })?;
        store::put_edge(con, &id, &format!("file:{path}"), "touches", "strong", "git", 1.0, "@git")?;
    }
    Ok(())
}

fn spec_path(lay: &Layout, file: &str) -> String {
    if lay.spec_display.is_empty() { file.to_string() } else { format!("{}/{}", lay.spec_display.trim_end_matches('/'), file) }
}

fn put_req(con: &Connection, lay: &Layout, r: &spec::Requirement, sub: &str) -> Result<Option<String>, String> {
    if r.id.is_empty() {
        return Ok(None);
    }
    let id = format!("req:{}", r.id);
    if exists(con, &id) {
        return Ok(Some(id));
    }
    let path = spec_path(lay, &r.file);
    let mut terms = BTreeMap::new();
    let mut ntok = add_terms(&mut terms, &r.title, 3);
    ntok += add_terms(&mut terms, &r.body, 1);
    ntok += add_terms(&mut terms, &r.id, 2);
    store::put_node(con, &NodeRow { id: &id, kind: "req", sub, name: &r.title, path: &path, start: r.span.start, end: r.span.end, ntok, owner: "@spec" })?;
    store::put_terms(con, &id, &cap_terms(terms), "@spec")?;
    for s in &r.scenarios {
        if s.id.is_empty() {
            continue;
        }
        let sid = format!("scn:{}", s.id);
        let mut terms = BTreeMap::new();
        let ntok = add_terms(&mut terms, &s.title, 3) + add_terms(&mut terms, &r.title, 1);
        store::put_node(con, &NodeRow { id: &sid, kind: "scn", sub, name: &s.title, path: &path, start: s.span.start, end: s.span.end, ntok, owner: "@spec" })?;
        store::put_terms(con, &sid, &terms, "@spec")?;
        store::put_edge(con, &id, &sid, "contains", "strong", "spec", 1.0, "@spec")?;
    }
    Ok(Some(id))
}

/// 規格層：能力、需求、情境，以及進行中的變更碰到哪些需求。規格目錄不存在就什麼都不做。
pub fn specs(con: &Connection, lay: &Layout, notes: &mut Vec<String>) -> Result<(), String> {
    if !Path::new(&lay.spec_root).is_dir() {
        return Ok(());
    }
    let opened = match spec::open(Path::new(&lay.spec_root), &lay.spec_display, &spec::IdOptions::default()) {
        Ok(o) => o,
        Err(err) => {
            notes.push(format!("規格目錄讀不懂，這次沒有規格的節點（{}）", pyish::take(&err, 160)));
            return Ok(());
        }
    };
    for cap in &opened.tree.capabilities {
        let cid = format!("cap:{}", cap.name);
        let path = spec_path(lay, &cap.file);
        let mut terms = BTreeMap::new();
        let ntok = add_terms(&mut terms, &cap.name, 3);
        let end = cap.requirements.iter().map(|r| r.span.end).max().unwrap_or(1);
        store::put_node(con, &NodeRow { id: &cid, kind: "cap", sub: "", name: &cap.name, path: &path, start: 1, end, ntok, owner: "@spec" })?;
        store::put_terms(con, &cid, &terms, "@spec")?;
        for r in &cap.requirements {
            if let Some(rid) = put_req(con, lay, r, "current")? {
                store::put_edge(con, &cid, &rid, "contains", "strong", "spec", 1.0, "@spec")?;
            }
        }
    }
    for ch in opened.tree.changes.iter().filter(|c| c.state == spec::ChangeState::Active) {
        let id = format!("chg:{}", ch.name);
        let path = spec_path(lay, &ch.dir);
        let mut terms = BTreeMap::new();
        let ntok = add_terms(&mut terms, &ch.name, 3);
        store::put_node(con, &NodeRow { id: &id, kind: "chg", sub: "active", name: &ch.name, path: &path, start: 0, end: 0, ntok, owner: "@spec" })?;
        store::put_terms(con, &id, &terms, "@spec")?;
        for d in &ch.deltas {
            for (list, what) in [(&d.added, "added"), (&d.modified, "modified"), (&d.removed, "removed")] {
                for r in list {
                    // 新增的需求還不在現行規格裡：節點指到差異檔
                    if let Some(rid) = put_req(con, lay, r, "delta")? {
                        store::put_edge(con, &id, &rid, "changes", "strong", &format!("spec:{what}"), 1.0, "@spec")?;
                    }
                }
            }
            for rn in &d.renamed {
                if !rn.id.is_empty() {
                    store::put_edge(con, &id, &format!("req:{}", rn.id), "changes", "strong", "spec:renamed", 1.0, "@spec")?;
                }
            }
        }
    }
    Ok(())
}

/// 決策的 id：紀錄自己有 `id` 就用它，沒有就用 run、時間與問題的雜湊（不含行號）。
pub fn decision_id(d: &Value) -> String {
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
    if !s("id").trim().is_empty() {
        return s("id").trim().to_string();
    }
    store::sha256_hex(format!("{}\u{1f}{}\u{1f}{}", s("run"), s("at"), s("question")).as_bytes())[..10].to_string()
}

/// 決策記憶 `<runs_dir>/decisions.jsonl`（人在閘門上的答覆）。
pub fn decisions(con: &Connection, lay: &Layout) -> Result<(), String> {
    if lay.runs.is_empty() {
        return Ok(());
    }
    let rel = format!("{}/decisions.jsonl", lay.runs);
    let Ok(text) = std::fs::read_to_string(pyish::pjoin(&lay.root, &rel)) else { return Ok(()) };
    for (k, line) in text.lines().enumerate() {
        let Ok(d) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let s = |key: &str| d.get(key).and_then(Value::as_str).unwrap_or("");
        if !d.is_object() || s("question").trim().is_empty() || s("answer").trim().is_empty() {
            continue;
        }
        let key = decision_id(&d);
        let id = format!("dec:{key}");
        let mut terms = BTreeMap::new();
        let ntok = add_terms(&mut terms, s("question"), 2) + add_terms(&mut terms, s("answer"), 1);
        store::put_node(con, &NodeRow { id: &id, kind: "dec", sub: s("scope"), name: &key, path: &rel, start: k + 1, end: k + 1, ntok, owner: "@dec" })?;
        store::put_terms(con, &id, &cap_terms(terms), "@dec")?;
        if !s("run").is_empty() {
            let run = run_node(con, lay, s("run"), "@dec")?;
            store::put_edge(con, &id, &run, "decided-in", "strong", "decisions", 1.0, "@dec")?;
        }
    }
    Ok(())
}

fn run_node(con: &Connection, lay: &Layout, name: &str, owner: &str) -> Result<String, String> {
    let id = format!("run:{name}");
    let rel = if lay.runs.is_empty() { String::new() } else { format!("{}/{name}", lay.runs) };
    let path = if !rel.is_empty() && Path::new(&pyish::pjoin(&lay.root, &rel)).is_dir() { rel } else { String::new() };
    store::put_node(con, &NodeRow { id: &id, kind: "run", sub: "", name, path: &path, start: 0, end: 0, ntok: 0, owner })?;
    Ok(id)
}

/// 文件與程式標記裡提到的 id、路徑。
pub fn refs(con: &Connection, w: &World) -> Result<(), String> {
    const OWNER: &str = "@derive";
    let mut rows: Vec<(String, String, String, String, usize)> = Vec::new();
    {
        let mut st = con.prepare("SELECT path, kind, target, scope, line FROM f_refs ORDER BY path, seq").map_err(e)?;
        let mut q = st.query([]).map_err(e)?;
        while let Some(r) = q.next().map_err(e)? {
            rows.push((r.get(0).map_err(e)?, r.get(1).map_err(e)?, r.get(2).map_err(e)?, r.get(3).map_err(e)?, r.get::<_, i64>(4).map_err(e)? as usize));
        }
    }
    let is_note = |p: &str| p.contains("/vault/notes/");
    for (path, kind, target, scope, line) in rows {
        match kind.as_str() {
            "mark" => match resolve_id(con, &target) {
                Some(dst) => store::put_edge(con, &scope, &dst, "marks", "strong", "mark", 1.0, OWNER)?,
                None => store::put_unresolved(con, &scope, "mark", &target, &path, line, "找不到這個 id")?,
            },
            "wiki" => {
                let bare = target.split('|').next().unwrap_or("").trim();
                let prov = if is_note(&path) { "note:wikilink" } else { "doc:wikilink" };
                let hit = resolve_id(con, bare).or_else(|| bare.split_once('#').and_then(|(a, _)| resolve_id(con, a)));
                match hit {
                    Some(dst) => store::put_edge(con, &scope, &dst, "mentions", "weak", prov, 0.8, OWNER)?,
                    None => store::put_unresolved(con, &scope, "wikilink", bare, &path, line, "找不到這個 id")?,
                }
            }
            "md" => {
                let t = target.split(['#', '?']).next().unwrap_or("");
                if t.is_empty() || t.contains("://") || t.starts_with("mailto:") {
                    continue;
                }
                let Some(p) = join_norm(dir_of(&path), t) else { continue };
                if w.files.contains_key(&p) {
                    store::put_edge(con, &scope, &format!("file:{p}"), "links", "weak", "doc:link", 0.9, OWNER)?;
                } else {
                    store::put_unresolved(con, &scope, "link", &p, &path, line, "檔案不存在或不在圖裡")?;
                }
            }
            "tick" => {
                let spec_hit = ["scn:", "req:"].iter().map(|p| format!("{p}{target}")).find(|id| exists(con, id));
                if let Some(dst) = spec_hit {
                    store::put_edge(con, &scope, &dst, "mentions", "weak", "doc:text", 0.6, OWNER)?;
                } else if w.files.contains_key(&target) {
                    store::put_edge(con, &scope, &format!("file:{target}"), "mentions", "weak", "doc:text", 0.6, OWNER)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn strip_loc(raw: &str) -> &str {
    let p = raw.split("::").next().unwrap_or(raw);
    match p.rsplit_once(':') {
        Some((head, tail)) if !head.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit() || b == b'-') => head,
        _ => p,
    }
}

fn looks_sha(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

struct Ends<'a> {
    con: &'a Connection,
    lay: &'a Layout,
    w: &'a World,
    has_spec: bool,
}

impl Ends<'_> {
    fn commit(&self, raw: &str) -> Result<String, String> {
        let full: Option<String> = self
            .con
            .prepare_cached("SELECT id FROM nodes WHERE kind='commit' AND id >= ?1 AND id < ?1 || 'g' ORDER BY id LIMIT 2")
            .and_then(|mut s| s.query_map([format!("commit:{raw}")], |r| r.get::<_, String>(0)).and_then(|it| it.collect::<Result<Vec<_>, _>>()))
            .ok()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.into_iter().next());
        if let Some(id) = full {
            return Ok(id);
        }
        let id = format!("commit:{raw}");
        store::put_node(self.con, &NodeRow { id: &id, kind: "commit", sub: "ledger", name: &raw[..raw.len().min(12)], path: "", start: 0, end: 0, ntok: 0, owner: "@ledger" })?;
        Ok(id)
    }

    fn sec(&self, raw: &str) -> Result<String, String> {
        if let Some(id) = resolve_id(self.con, raw) {
            return Ok(id);
        }
        let Some((path, heading)) = raw.split_once('#') else { return Ok(format!("file:{}", kb::to_posix(strip_loc(raw)))) };
        let path = kb::to_posix(path);
        let heading = heading.trim();
        let hit: Option<String> = self
            .con
            .prepare_cached("SELECT id FROM nodes WHERE kind='sec' AND path=?1 AND (name=?2 OR id=?3) ORDER BY start LIMIT 1")
            .and_then(|mut s| s.query_row(params![path, heading, format!("sec:{path}#{heading}")], |r| r.get(0)))
            .ok();
        Ok(hit.unwrap_or_else(|| format!("sec:{path}#{heading}")))
    }

    fn id(&self, raw: &str) -> Result<String, String> {
        if let Some(id) = resolve_id(self.con, raw) {
            return Ok(id);
        }
        let id = format!("ac:{raw}");
        let sub = if self.has_spec { "unknown" } else { "" };
        store::put_node(self.con, &NodeRow { id: &id, kind: "ac", sub, name: raw, path: "", start: 0, end: 0, ntok: 0, owner: "@ledger" })?;
        Ok(id)
    }

    fn end(&self, raw: &str, ty: End) -> Result<String, String> {
        let raw = raw.trim();
        for (p, t) in [("test:", End::File), ("doc:", End::Sec)] {
            if let Some(rest) = raw.strip_prefix(p) {
                return self.end(rest, t);
            }
        }
        if let Some(p) = super::sources::PREFIXES.iter().find(|p| raw.starts_with(**p)) {
            let rest = &raw[p.len()..];
            return match *p {
                "run:" => run_node(self.con, self.lay, rest, "@ledger"),
                "commit:" => self.commit(rest),
                "ac:" | "req:" | "scn:" | "cap:" | "dec:" | "chg:" if !exists(self.con, raw) => self.id(rest),
                _ => Ok(raw.to_string()),
            };
        }
        match ty {
            End::Id => self.id(raw),
            End::Run => run_node(self.con, self.lay, raw, "@ledger"),
            End::File => Ok(format!("file:{}", kb::to_posix(strip_loc(raw)))),
            End::Sec => self.sec(raw),
            End::Commit => self.commit(raw),
            End::Dec => match resolve_id(self.con, raw) {
                Some(id) => Ok(id),
                None => {
                    let id = format!("dec:{raw}");
                    store::put_node(self.con, &NodeRow { id: &id, kind: "dec", sub: "ledger", name: raw, path: "", start: 0, end: 0, ntok: 0, owner: "@ledger" })?;
                    Ok(id)
                }
            },
            End::Any => {
                if let Some(id) = resolve_id(self.con, raw) {
                    Ok(id)
                } else if self.w.files.contains_key(&kb::to_posix(strip_loc(raw))) {
                    Ok(format!("file:{}", kb::to_posix(strip_loc(raw))))
                } else if raw.contains('#') && self.w.files.contains_key(&kb::to_posix(raw.split('#').next().unwrap_or(""))) {
                    self.sec(raw)
                } else if looks_sha(raw) {
                    self.commit(raw)
                } else {
                    self.id(raw)
                }
            }
        }
    }
}

/// 連結帳本 `<runs_dir>/links.jsonl`。回傳略過的壞行數。
pub fn ledger(con: &Connection, lay: &Layout, w: &World) -> Result<usize, String> {
    if lay.runs.is_empty() {
        return Ok(0);
    }
    let led = links::read_ledger(&pyish::pjoin(&lay.root, &format!("{}/links.jsonl", lay.runs)));
    let has_spec = con.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='req')", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1;
    let ends = Ends { con, lay, w, has_spec };
    for l in &led.links {
        let (ft, tt) = links::ends_of(&l.kind);
        let src = ends.end(&l.from, ft)?;
        let dst = ends.end(&l.to, tt)?;
        let prov = format!("ledger:{}/{}", l.run, l.stage);
        store::put_edge(con, &src, &dst, &l.kind, if l.strong { "strong" } else { "weak" }, &prov, l.confidence, "@ledger")?;
        if !l.run.is_empty() && l.kind != "req-run" {
            let run = run_node(con, lay, &l.run, "@ledger")?;
            if src.starts_with("req:") || src.starts_with("scn:") || src.starts_with("ac:") {
                store::put_edge(con, &src, &run, "req-run", "strong", &prov, 1.0, "@ledger")?;
            }
        }
    }
    Ok(led.bad)
}
