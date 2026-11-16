//! 跨檔解析：把每個檔的 import、呼叫、繼承接到別的檔的定義上。
//!
//! 只做確定的：相對路徑 import、`tsconfig` 的 paths、同檔與已 import 的名字，加上幾個由語言規則決定的位置
//! （Go 的 module 路徑、Java／Kotlin 的套件目錄、Rust 的 `mod` 與 `crate::`、Python 套件根目錄）。
//! 解析不了的寫進 `unresolved`，不猜。每次建圖整個重算（所以目標後來出現時會自動接上）。

use super::code::Lang;
use super::store::{self, e};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// 走訪時順便收的設定檔（相對路徑）。
#[derive(Debug, Clone, Default)]
pub struct Aux {
    pub tsconfigs: Vec<String>,
    pub gomods: Vec<String>,
    pub cargos: Vec<String>,
}

pub struct FileInfo {
    pub lang: Option<Lang>,
}

pub struct DefInfo {
    pub qual: String,
    pub sub: String,
    pub id: String,
}

struct TsCfg {
    dir: String,
    base: Option<String>,
    paths: Vec<(String, Vec<String>)>,
}

pub struct World {
    pub files: BTreeMap<String, FileInfo>,
    pub defs: HashMap<String, Vec<DefInfo>>,
    by_base: HashMap<String, Vec<String>>,
    by_dir: HashMap<String, Vec<String>>,
    ts: Vec<TsCfg>,
    gomods: Vec<(String, String)>,
    cargos: Vec<String>,
}

pub struct Imp {
    pub targets: Vec<String>,
    pub names: Vec<(String, String)>,
    pub external: bool,
}

pub enum Res {
    Edge(String, f64, bool),
    Unres(&'static str),
    Skip,
}

/// 路徑開頭的 `<repo id>:`（別的 repo 的檔；含冒號）與其餘的部分。
fn repo_prefix(p: &str) -> (&str, &str) {
    match super::build::split_repo(p) {
        Some((id, _)) => p.split_at(id.len() + 1),
        None => ("", p),
    }
}

/// 所在的目錄。別的 repo 的檔保留 `<repo id>:` 前綴（`spec:a.md` → `spec:`）。
pub fn dir_of(p: &str) -> &str {
    match p.rsplit_once('/') {
        Some(x) => x.0,
        None => repo_prefix(p).0,
    }
}

pub fn base_of(p: &str) -> &str {
    repo_prefix(p).1.rsplit('/').next().unwrap_or(p)
}

/// 把 `rel` 接在 `dir` 後面並化簡 `.` 與 `..`；跑出根目錄回 None。`dir` 帶 `<repo id>:` 前綴時留在那個 repo 裡。
pub fn join_norm(dir: &str, rel: &str) -> Option<String> {
    let (prefix, dir) = repo_prefix(dir);
    if !prefix.is_empty() {
        return join_plain(dir, rel).map(|p| format!("{prefix}{p}"));
    }
    join_plain(dir, rel)
}

fn join_plain(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if rel.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// 去掉 JSON 裡的註解與結尾逗號（tsconfig 常這樣寫）。
fn strip_jsonc(text: &str) -> String {
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 1;
        } else if c == ',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if !matches!(b.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn load_ts(root: &str, rel: &str) -> Option<TsCfg> {
    let text = std::fs::read_to_string(format!("{root}/{rel}")).ok()?;
    let v: Value = serde_json::from_str(&strip_jsonc(&text)).ok()?;
    let co = v.get("compilerOptions")?;
    let dir = dir_of(rel).to_string();
    let base = co.get("baseUrl").and_then(Value::as_str).and_then(|b| join_norm(&dir, b));
    let mut paths = Vec::new();
    if let Some(Value::Object(m)) = co.get("paths") {
        for (k, v) in m {
            let subs: Vec<String> = v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            if !subs.is_empty() {
                paths.push((k.clone(), subs));
            }
        }
    }
    paths.sort();
    if base.is_none() && paths.is_empty() {
        return None;
    }
    Some(TsCfg { dir, base, paths })
}

fn go_module(root: &str, rel: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("{root}/{rel}")).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("module ")).map(|m| m.trim().trim_matches('"').to_string()).filter(|m| !m.is_empty())
}

fn under(path: &str, dir: &str) -> bool {
    dir.is_empty() || path.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
}

impl World {
    pub fn load(con: &Connection, root: &str, aux: &Aux) -> Result<World, String> {
        let mut files = BTreeMap::new();
        let mut by_base: HashMap<String, Vec<String>> = HashMap::new();
        let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut st = con.prepare("SELECT path, src, lang FROM files ORDER BY path").map_err(e)?;
            let mut rows = st.query([]).map_err(e)?;
            while let Some(r) = rows.next().map_err(e)? {
                let path: String = r.get(0).map_err(e)?;
                let src: String = r.get(1).map_err(e)?;
                let lang: String = r.get(2).map_err(e)?;
                if src == "code" {
                    by_base.entry(base_of(&path).to_string()).or_default().push(path.clone());
                    by_dir.entry(dir_of(&path).to_string()).or_default().push(path.clone());
                }
                files.insert(path, FileInfo { lang: Lang::from_name(&lang) });
            }
        }
        let mut defs: HashMap<String, Vec<DefInfo>> = HashMap::new();
        {
            let mut st = con.prepare("SELECT owner, name, sub, id FROM nodes WHERE kind='sym' ORDER BY owner, start, id").map_err(e)?;
            let mut rows = st.query([]).map_err(e)?;
            while let Some(r) = rows.next().map_err(e)? {
                let owner: String = r.get(0).map_err(e)?;
                defs.entry(owner).or_default().push(DefInfo { qual: r.get(1).map_err(e)?, sub: r.get(2).map_err(e)?, id: r.get(3).map_err(e)? });
            }
        }
        let mut ts: Vec<TsCfg> = aux.tsconfigs.iter().filter_map(|p| load_ts(root, p)).collect();
        ts.sort_by(|a, b| b.dir.len().cmp(&a.dir.len()).then(a.dir.cmp(&b.dir)));
        let mut gomods: Vec<(String, String)> = aux.gomods.iter().filter_map(|p| go_module(root, p).map(|m| (dir_of(p).to_string(), m))).collect();
        gomods.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.cmp(b)));
        let mut cargos: Vec<String> = aux.cargos.iter().map(|p| dir_of(p).to_string()).collect();
        cargos.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        Ok(World { files, defs, by_base, by_dir, ts, gomods, cargos })
    }

    fn is_code(&self, p: &str) -> bool {
        self.files.get(p).is_some_and(|f| f.lang.is_some())
    }

    fn first(&self, cands: &[String]) -> Option<String> {
        cands.iter().find(|c| self.is_code(c)).cloned()
    }

    /// 完全相同的完整名稱。
    pub fn exact(&self, path: &str, qual: &str) -> Option<&str> {
        self.defs.get(path)?.iter().find(|d| d.qual == qual).map(|d| d.id.as_str())
    }

    /// 這個檔裡叫這個名字的定義：先找最外層同名的，再找唯一一個「最後一段同名、不是方法」的。
    pub fn find_def(&self, path: &str, name: &str) -> Option<&DefInfo> {
        let ds = self.defs.get(path)?;
        if let Some(d) = ds.iter().find(|d| d.qual == name) {
            return Some(d);
        }
        let mut hit = ds.iter().filter(|d| d.sub != "method" && d.qual.rsplit('.').next() == Some(name));
        match (hit.next(), hit.next()) {
            (Some(d), None) => Some(d),
            _ => None,
        }
    }

    /// 型別 `ty` 的方法 `name`。
    pub fn find_method(&self, path: &str, ty: &str, name: &str) -> Option<&str> {
        let t = self.find_def(path, ty)?;
        self.exact(path, &format!("{}.{}", t.qual, name))
    }

    /// `scope` 所在的型別（由內往外找第一個不是函式的定義）。
    fn enclosing_type(&self, path: &str, scope: &str) -> Option<String> {
        let ds = self.defs.get(path)?;
        let mut s = scope;
        loop {
            if s.is_empty() {
                return None;
            }
            match ds.iter().find(|d| d.qual == s) {
                Some(d) if !matches!(d.sub.as_str(), "function" | "method") => return Some(s.to_string()),
                Some(_) => {}
                // Rust 的 impl 區塊、Go 的接收者：型別本身可能定義在別的檔
                None if ds.iter().any(|d| d.qual.strip_prefix(s).is_some_and(|r| r.starts_with('.'))) => return Some(s.to_string()),
                None => {}
            }
            s = s.rsplit_once('.').map(|x| x.0).unwrap_or("");
        }
    }

    fn js_candidates(&self, base: &str) -> Option<String> {
        let mut c = vec![base.to_string()];
        for ext in [".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs"] {
            c.push(format!("{base}{ext}"));
        }
        for (from, tos) in [(".js", [".ts", ".tsx"]), (".jsx", [".tsx", ".ts"]), (".mjs", [".mts", ".ts"])] {
            if let Some(stem) = base.strip_suffix(from) {
                for t in tos {
                    c.push(format!("{stem}{t}"));
                }
            }
        }
        for idx in ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"] {
            c.push(format!("{base}{idx}"));
        }
        self.first(&c)
    }

    fn rust_under(&self, base: &str, segs: &[&str]) -> Option<String> {
        for k in (1..=segs.len()).rev() {
            let p = segs[..k].join("/");
            let stem = if base.is_empty() { p } else { format!("{base}/{p}") };
            if let Some(hit) = self.first(&[format!("{stem}.rs"), format!("{stem}/mod.rs")]) {
                return Some(hit);
            }
        }
        None
    }

    /// 這個 Rust 檔代表的模組，其子模組放在哪個目錄。
    fn rust_mod_dir(path: &str) -> String {
        let dir = dir_of(path);
        let name = base_of(path);
        if matches!(name, "mod.rs" | "lib.rs" | "main.rs") {
            dir.to_string()
        } else {
            let stem = name.strip_suffix(".rs").unwrap_or(name);
            if dir.is_empty() { stem.to_string() } else { format!("{dir}/{stem}") }
        }
    }

    /// 一個 import 指到哪些檔。Err 是解析不了的原因；`external` 表示看起來是專案外的套件。
    pub fn resolve_import(&self, path: &str, lang: Lang, module: &str, names: &[(String, String)], mod_aliases: &[String]) -> Result<Vec<String>, &'static str> {
        const EXTERNAL: &str = "external";
        const MISSING: &str = "找不到這個檔";
        let dir = dir_of(path);
        match lang {
            Lang::Ts | Lang::Tsx | Lang::Js => {
                if module.starts_with('.') {
                    let base = join_norm(dir, module).ok_or(MISSING)?;
                    return self.js_candidates(&base).map(|p| vec![p]).ok_or(MISSING);
                }
                if let Some(cfg) = self.ts.iter().find(|c| under(path, &c.dir)) {
                    let anchor = cfg.base.clone().unwrap_or_else(|| cfg.dir.clone());
                    for (pat, subs) in &cfg.paths {
                        let star = match pat.split_once('*') {
                            Some((pre, suf)) => module.strip_prefix(pre).and_then(|r| r.strip_suffix(suf)),
                            None if pat == module => Some(""),
                            None => None,
                        };
                        let Some(star) = star else { continue };
                        for s in subs {
                            if let Some(hit) = join_norm(&anchor, &s.replace('*', star)).and_then(|b| self.js_candidates(&b)) {
                                return Ok(vec![hit]);
                            }
                        }
                        return Err("tsconfig 的 paths 對得上，但找不到檔");
                    }
                    if let Some(b) = &cfg.base {
                        if let Some(hit) = join_norm(b, module).and_then(|x| self.js_candidates(&x)) {
                            return Ok(vec![hit]);
                        }
                    }
                }
                Err(EXTERNAL)
            }
            Lang::Py => {
                let dots = module.chars().take_while(|c| *c == '.').count();
                let rest: Vec<&str> = module[dots..].split('.').filter(|s| !s.is_empty()).collect();
                let lookup = |base: &str| -> Vec<String> {
                    let stem = match (base.is_empty(), rest.is_empty()) {
                        (_, true) => base.to_string(),
                        (true, false) => rest.join("/"),
                        (false, false) => format!("{base}/{}", rest.join("/")),
                    };
                    let mut out = Vec::new();
                    if !rest.is_empty() {
                        if let Some(p) = self.first(&[format!("{stem}.py"), format!("{stem}/__init__.py")]) {
                            out.push(p);
                        }
                    }
                    // from 套件 import 子模組
                    for (orig, _) in names {
                        if orig != "*" {
                            let sub = if stem.is_empty() { orig.clone() } else { format!("{stem}/{orig}") };
                            if let Some(p) = self.first(&[format!("{sub}.py"), format!("{sub}/__init__.py")]) {
                                if !out.contains(&p) {
                                    out.push(p);
                                }
                            }
                        }
                    }
                    out
                };
                if dots > 0 {
                    let mut base = dir.to_string();
                    for _ in 1..dots {
                        base = join_norm(&base, "..").ok_or(MISSING)?;
                    }
                    let hit = lookup(&base);
                    return if hit.is_empty() { Err(MISSING) } else { Ok(hit) };
                }
                // 絕對 import：repo 根目錄，或這個檔所屬套件的根目錄
                let mut pkg_root = dir.to_string();
                while !pkg_root.is_empty() && self.is_code(&format!("{pkg_root}/__init__.py")) {
                    pkg_root = dir_of(&pkg_root).to_string();
                }
                for base in ["", pkg_root.as_str()] {
                    let hit = lookup(base);
                    if !hit.is_empty() {
                        return Ok(hit);
                    }
                }
                Err(EXTERNAL)
            }
            Lang::Go => {
                for (mdir, m) in &self.gomods {
                    let rest = if module == m { Some("") } else { module.strip_prefix(m.as_str()).and_then(|r| r.strip_prefix('/')) };
                    let Some(rest) = rest else { continue };
                    let d = join_norm(mdir, rest).ok_or(MISSING)?;
                    let hit: Vec<String> = self.by_dir.get(&d).map(|v| v.iter().filter(|p| p.ends_with(".go") && !p.ends_with("_test.go")).cloned().collect()).unwrap_or_default();
                    return if hit.is_empty() { Err(MISSING) } else { Ok(hit) };
                }
                Err(EXTERNAL)
            }
            Lang::Java | Lang::Kotlin => {
                let exts: &[&str] = if lang == Lang::Java { &["java"] } else { &["kt", "java"] };
                let segs: Vec<&str> = module.split('.').filter(|s| !s.is_empty() && *s != "*").collect();
                // a.b.C，或 a.b.C.member（static import、Kotlin 的頂層函式放在 a/b/ 底下某個檔，後者不猜）
                for take in [segs.len(), segs.len().saturating_sub(1)] {
                    if take == 0 {
                        continue;
                    }
                    for ext in exts {
                        let suffix = format!("{}.{ext}", segs[..take].join("/"));
                        let name = base_of(&suffix);
                        let hits: Vec<&String> = self.by_base.get(name).map(|v| v.iter().filter(|p| *p == &suffix || p.ends_with(&format!("/{suffix}"))).collect()).unwrap_or_default();
                        match hits.len() {
                            0 => {}
                            1 => return Ok(vec![hits[0].clone()]),
                            _ => return Err("有多個檔符合這個套件路徑"),
                        }
                    }
                }
                Err(EXTERNAL)
            }
            Lang::Rust => {
                if let Some(m) = module.strip_prefix("mod:") {
                    let base = Self::rust_mod_dir(path);
                    let stem = if base.is_empty() { m.to_string() } else { format!("{base}/{m}") };
                    return self.first(&[format!("{stem}.rs"), format!("{stem}/mod.rs")]).map(|p| vec![p]).ok_or(MISSING);
                }
                let segs: Vec<&str> = module.split("::").filter(|s| !s.is_empty() && *s != "*").collect();
                let Some(first) = segs.first().copied() else { return Err(EXTERNAL) };
                match first {
                    "crate" => {
                        let Some(cdir) = self.cargos.iter().find(|d| under(path, d)) else { return Err("找不到這個 crate 的 Cargo.toml") };
                        let src = if cdir.is_empty() { "src".to_string() } else { format!("{cdir}/src") };
                        if let Some(hit) = self.rust_under(&src, &segs[1..]) {
                            return Ok(vec![hit]);
                        }
                        self.first(&[format!("{src}/lib.rs"), format!("{src}/main.rs")]).map(|p| vec![p]).ok_or(MISSING)
                    }
                    "self" => self.rust_under(&Self::rust_mod_dir(path), &segs[1..]).map(|p| vec![p]).ok_or(MISSING),
                    "super" => {
                        let mut base = Self::rust_mod_dir(path);
                        let mut rest = &segs[..];
                        while rest.first() == Some(&"super") {
                            base = dir_of(&base).to_string();
                            rest = &rest[1..];
                        }
                        self.rust_under(&base, rest).map(|p| vec![p]).ok_or(MISSING)
                    }
                    f if mod_aliases.iter().any(|a| a == f) => self.rust_under(&Self::rust_mod_dir(path), &segs).map(|p| vec![p]).ok_or(MISSING),
                    _ => Err(EXTERNAL),
                }
            }
            Lang::CSharp => Err("命名空間不對應檔案，不解析"),
            Lang::Php => {
                if module.starts_with('.') || module.starts_with('/') {
                    let base = join_norm(dir, module).ok_or(MISSING)?;
                    return self.first(&[base]).map(|p| vec![p]).ok_or(MISSING);
                }
                let segs: Vec<&str> = module.split('\\').filter(|s| !s.is_empty()).collect();
                for skip in 0..segs.len().saturating_sub(1) {
                    let suffix = format!("{}.php", segs[skip..].join("/"));
                    let name = base_of(&suffix);
                    let hits: Vec<&String> = self.by_base.get(name).map(|v| v.iter().filter(|p| *p == &suffix || p.ends_with(&format!("/{suffix}"))).collect()).unwrap_or_default();
                    match hits.len() {
                        0 => {}
                        1 => return Ok(vec![hits[0].clone()]),
                        _ => return Err("有多個檔符合這個命名空間"),
                    }
                }
                Err(EXTERNAL)
            }
            Lang::Ruby => {
                if module.starts_with('.') {
                    let base = join_norm(dir, module).ok_or(MISSING)?;
                    return self.first(&[format!("{base}.rb"), base]).map(|p| vec![p]).ok_or(MISSING);
                }
                self.first(&[format!("lib/{module}.rb")]).map(|p| vec![p]).ok_or(EXTERNAL)
            }
        }
    }

    fn same_dir(&self, path: &str, lang: Lang) -> Vec<&String> {
        if !matches!(lang, Lang::Go | Lang::Java | Lang::Kotlin) {
            return Vec::new();
        }
        self.by_dir.get(dir_of(path)).map(|v| v.iter().filter(|p| p.as_str() != path && self.files.get(*p).and_then(|f| f.lang) == Some(lang)).collect()).unwrap_or_default()
    }

    fn unique<'a>(&'a self, hits: impl Iterator<Item = &'a str>) -> Option<&'a str> {
        let mut it = hits;
        match (it.next(), it.next()) {
            (Some(h), None) => Some(h),
            _ => None,
        }
    }

    pub fn resolve_call(&self, path: &str, lang: Lang, imps: &[Imp], name: &str, qual: &str, scope: &str) -> Res {
        let edge = |id: &str| Res::Edge(id.to_string(), 1.0, true);
        let alias = |n: &str| imps.iter().find_map(|i| i.names.iter().find(|(_, al)| al == n).map(|(orig, _)| (i, orig.as_str())));
        let class = self.enclosing_type(path, scope);
        if qual == "?" {
            return Res::Unres("接收者是運算式");
        }
        if qual.is_empty() {
            if matches!(lang, Lang::Java | Lang::CSharp | Lang::Kotlin | Lang::Ruby) {
                if let Some(id) = class.as_ref().and_then(|c| self.exact(path, &format!("{c}.{name}"))) {
                    return edge(id);
                }
            }
            let mut s = scope;
            loop {
                let cand = if s.is_empty() { name.to_string() } else { format!("{s}.{name}") };
                if let Some(d) = self.defs.get(path).and_then(|ds| ds.iter().find(|d| d.qual == cand && (s.is_empty() || d.sub != "method"))) {
                    return edge(&d.id);
                }
                if s.is_empty() {
                    break;
                }
                s = s.rsplit_once('.').map(|x| x.0).unwrap_or("");
            }
            if let Some((imp, orig)) = alias(name) {
                if imp.external {
                    return Res::Skip;
                }
                let whole = orig == "default" || orig == "*";
                for t in &imp.targets {
                    if let Some(d) = self.find_def(t, if whole { name } else { orig }) {
                        return edge(&d.id);
                    }
                }
                return match imp.targets.first() {
                    Some(t) if whole => Res::Edge(format!("file:{t}"), 0.8, false),
                    _ => Res::Unres("匯入的檔裡找不到這個名字"),
                };
            }
            if let Some(d) = self.find_def(path, name) {
                return edge(&d.id);
            }
            let sib = self.same_dir(path, lang);
            if let Some(id) = self.unique(sib.iter().filter_map(|p| self.find_def(p, name)).map(|d| d.id.as_str())) {
                return Res::Edge(id.to_string(), 0.9, false);
            }
            let wild = imps.iter().filter(|i| !i.external && i.names.iter().any(|(o, a)| o == "*" && a == "*"));
            if let Some(id) = self.unique(wild.flat_map(|i| i.targets.iter()).filter_map(|t| self.find_def(t, name)).map(|d| d.id.as_str())) {
                return Res::Edge(id.to_string(), 0.9, false);
            }
            return Res::Unres("找不到定義");
        }
        if matches!(qual, "this" | "self" | "cls" | "Self" | "super") {
            if qual != "super" {
                if let Some(id) = class.as_ref().and_then(|c| self.exact(path, &format!("{c}.{name}"))) {
                    return edge(id);
                }
            }
            return Res::Unres("可能是繼承來的或定義在別的檔");
        }
        if let Some((imp, orig)) = alias(qual) {
            if imp.external {
                return Res::Skip;
            }
            for t in &imp.targets {
                if orig != "*" {
                    if let Some(id) = self.find_method(t, orig, name) {
                        return edge(id);
                    }
                }
                if let Some(d) = self.find_def(t, name) {
                    return edge(&d.id);
                }
            }
            return Res::Unres("匯入的檔裡找不到這個名字");
        }
        if let Some(id) = self.find_method(path, qual, name) {
            return edge(id);
        }
        let sib = self.same_dir(path, lang);
        if let Some(id) = self.unique(sib.iter().filter_map(|p| self.find_method(p, qual, name))) {
            return Res::Edge(id.to_string(), 0.9, false);
        }
        Res::Unres("接收者的型別未知")
    }

    pub fn resolve_type(&self, path: &str, lang: Lang, imps: &[Imp], target: &str) -> Res {
        if let Some(d) = self.find_def(path, target) {
            return Res::Edge(d.id.clone(), 1.0, true);
        }
        if let Some((imp, orig)) = imps.iter().find_map(|i| i.names.iter().find(|(_, al)| al == target).map(|(orig, _)| (i, orig.as_str()))) {
            if imp.external {
                return Res::Skip;
            }
            for t in &imp.targets {
                if let Some(d) = self.find_def(t, if orig == "default" || orig == "*" { target } else { orig }) {
                    return Res::Edge(d.id.clone(), 1.0, true);
                }
            }
            return Res::Unres("匯入的檔裡找不到這個名字");
        }
        let sib = self.same_dir(path, lang);
        if let Some(id) = self.unique(sib.iter().filter_map(|p| self.find_def(p, target)).map(|d| d.id.as_str())) {
            return Res::Edge(id.to_string(), 0.9, false);
        }
        Res::Unres("找不到定義")
    }
}

fn parse_names(s: &str) -> Vec<(String, String)> {
    s.split('\u{1f}').filter(|x| !x.is_empty()).filter_map(|x| x.split_once('\u{1e}')).map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// `f_imports.names` 的存法。
pub fn pack_names(names: &[(String, String)]) -> String {
    names.iter().map(|(a, b)| format!("{a}\u{1e}{b}")).collect::<Vec<_>>().join("\u{1f}")
}

fn src_node(w: &World, path: &str, scope: &str) -> String {
    match w.exact(path, scope) {
        Some(id) if !scope.is_empty() => id.to_string(),
        _ => format!("file:{path}"),
    }
}

/// 把程式的 import、呼叫、繼承接起來，寫進 `edges`（`@derive`）與 `unresolved`。
pub fn link_code(con: &Connection, w: &World) -> Result<(), String> {
    const OWNER: &str = "@derive";
    // import
    let mut imps: HashMap<String, Vec<Imp>> = HashMap::new();
    {
        let mut raw: Vec<(String, String, String, usize)> = Vec::new();
        let mut st = con.prepare("SELECT path, module, names, line FROM f_imports ORDER BY path, seq").map_err(e)?;
        let mut rows = st.query([]).map_err(e)?;
        while let Some(r) = rows.next().map_err(e)? {
            raw.push((r.get(0).map_err(e)?, r.get(1).map_err(e)?, r.get(2).map_err(e)?, r.get::<_, i64>(3).map_err(e)? as usize));
        }
        let mut mod_aliases: HashMap<&str, Vec<String>> = HashMap::new();
        for (path, module, _, _) in &raw {
            if let Some(m) = module.strip_prefix("mod:") {
                mod_aliases.entry(path.as_str()).or_default().push(m.to_string());
            }
        }
        for (path, module, names, line) in &raw {
            let Some(lang) = w.files.get(path).and_then(|f| f.lang) else { continue };
            let names = parse_names(names);
            let none = Vec::new();
            let res = w.resolve_import(path, lang, module, &names, mod_aliases.get(path.as_str()).unwrap_or(&none));
            let src = format!("file:{path}");
            let (targets, external) = match res {
                Ok(t) => {
                    for dst in &t {
                        store::put_edge(con, &src, &format!("file:{dst}"), "imports", "strong", "code:import", 1.0, OWNER)?;
                    }
                    (t, false)
                }
                Err(reason) => {
                    store::put_unresolved(con, &src, "import", module, path, *line, reason)?;
                    (Vec::new(), true)
                }
            };
            imps.entry(path.clone()).or_default().push(Imp { targets, names, external });
        }
    }
    let none: Vec<Imp> = Vec::new();
    let put = |src: String, kind: &str, shown: &str, path: &str, line: usize, res: Res| -> Result<(), String> {
        match res {
            Res::Edge(dst, conf, strong) => store::put_edge(con, &src, &dst, kind, if strong { "strong" } else { "weak" }, &format!("code:{kind}"), conf, OWNER),
            Res::Unres(reason) => store::put_unresolved(con, &src, kind, shown, path, line, reason),
            Res::Skip => Ok(()),
        }
    };
    {
        let mut st = con.prepare("SELECT path, name, qual, scope, line FROM f_calls ORDER BY path, seq").map_err(e)?;
        let mut rows = st.query([]).map_err(e)?;
        while let Some(r) = rows.next().map_err(e)? {
            let path: String = r.get(0).map_err(e)?;
            let name: String = r.get(1).map_err(e)?;
            let qual: String = r.get(2).map_err(e)?;
            let scope: String = r.get(3).map_err(e)?;
            let line = r.get::<_, i64>(4).map_err(e)? as usize;
            let Some(lang) = w.files.get(&path).and_then(|f| f.lang) else { continue };
            let res = w.resolve_call(&path, lang, imps.get(&path).unwrap_or(&none), &name, &qual, &scope);
            let shown = if qual.is_empty() { name.clone() } else { format!("{qual}.{name}") };
            put(src_node(w, &path, &scope), "calls", &shown, &path, line, res)?;
        }
    }
    {
        let mut st = con.prepare("SELECT path, scope, target, kind, line FROM f_inherits ORDER BY path, seq").map_err(e)?;
        let mut rows = st.query([]).map_err(e)?;
        while let Some(r) = rows.next().map_err(e)? {
            let path: String = r.get(0).map_err(e)?;
            let scope: String = r.get(1).map_err(e)?;
            let target: String = r.get(2).map_err(e)?;
            let kind: String = r.get(3).map_err(e)?;
            let line = r.get::<_, i64>(4).map_err(e)? as usize;
            let Some(lang) = w.files.get(&path).and_then(|f| f.lang) else { continue };
            let res = w.resolve_type(&path, lang, imps.get(&path).unwrap_or(&none), &target);
            put(src_node(w, &path, &scope), &kind, &target, &path, line, res)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(join_norm("a/b", "../c/./d").as_deref(), Some("a/c/d"));
        assert_eq!(join_norm("a", "../../x"), None);
        assert_eq!(join_norm("", "./x").as_deref(), Some("x"));
        assert_eq!(strip_jsonc("{ // c\n \"a\": [1, 2,], /* x */ \"b\": \"//no\", }"), "{ \n \"a\": [1, 2],  \"b\": \"//no\" }");
        assert_eq!(World::rust_mod_dir("src/a/mod.rs"), "src/a");
        assert_eq!(World::rust_mod_dir("src/a.rs"), "src/a");
    }
}
