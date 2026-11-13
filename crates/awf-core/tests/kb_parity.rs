//! 知識層的對跑：照 Python 版錄下來的劇本（tests/parity/kb.expected.json）做一遍，逐字比對。
//!
//! 劇本的 `steps` 是檔案操作與指令；指令比 exit code、stdout、stderr，以及索引目錄的檢視檔與資料表內容。
//! `probes` 是直接呼叫函式的案例。暫存路徑換成 `@TMP@` 再比。劇本用到符號連結，所以只在 unix 上跑。
#![cfg(unix)]

use awf_core::kb;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

fn expected() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/kb.expected.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))).expect("JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn sha16(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn rows(con: &rusqlite::Connection, sql: &str) -> rusqlite::Result<Vec<Vec<Value>>> {
    let mut st = con.prepare(sql)?;
    let n = st.column_count();
    let mut out = Vec::new();
    let mut q = st.query([])?;
    while let Some(r) = q.next()? {
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            row.push(match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(v) => json!(v),
                rusqlite::types::ValueRef::Real(v) => json!(v),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(b) => json!(String::from_utf8_lossy(b)),
            });
        }
        out.push(row);
    }
    Ok(out)
}

fn digest(rows: &[Vec<Value>]) -> Value {
    json!([rows.len(), sha16(serde_json::to_string(rows).unwrap_or_default().as_bytes())])
}

/// kb.sqlite 各資料表的列（和錄製腳本的 dump_db 同一個格式）。
fn dump_db(path: &Path) -> Value {
    if !path.is_file() {
        return Value::Null;
    }
    let read = || -> rusqlite::Result<Value> {
        let con = rusqlite::Connection::open(path)?;
        let meta = rows(&con, "SELECT key, value FROM meta ORDER BY key")?;
        let chunks: Vec<Vec<Value>> = rows(&con, "SELECT id, path, start, end, level, headings, sec, part, ntok, text FROM chunks ORDER BY id")?
            .into_iter()
            .map(|mut r| {
                let text = r.pop().unwrap_or_default();
                r.push(json!(sha16(s(&text).as_bytes())));
                r
            })
            .collect();
        let mut out = json!({
            "meta": meta,
            "files": rows(&con, "SELECT path, mtime_ns, size, sha1, title, frontmatter, nlines FROM files ORDER BY path")?,
            "chunks": chunks,
            "links": rows(&con, "SELECT path, line, kind, target FROM links ORDER BY rowid")?,
            "postings": digest(&rows(&con, "SELECT term, chunk_id, th, ta, tp, tb FROM postings ORDER BY term, chunk_id")?),
        });
        if meta.iter().any(|r| r == &[json!("fts5"), json!("1")]) {
            out["fts"] = digest(&rows(&con, "SELECT rowid, head, anc, path, body FROM fts ORDER BY rowid")?);
        }
        Ok(out)
    };
    read().unwrap_or_else(|_| json!("unreadable"))
}

struct Stage {
    td: String,
    real: String,
}

impl Stage {
    fn p(&self, rel: &str) -> PathBuf {
        Path::new(&self.td).join(rel)
    }

    fn norm(&self, text: &str) -> String {
        text.replace(&self.real, "@TMP@").replace(&self.td, "@TMP@")
    }

    fn set_mtime(&self, rel: &str, ns: u64) {
        let f = std::fs::OpenOptions::new().write(true).open(self.p(rel)).unwrap_or_else(|e| panic!("開不了 {rel}：{e}"));
        f.set_modified(UNIX_EPOCH + Duration::from_nanos(ns)).unwrap_or_else(|e| panic!("改不了 {rel} 的時間：{e}"));
    }

    fn walk(&self, base: &Path, dir: &Path, files: &mut Map<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            let name = e.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if e.is_dir() {
                self.walk(base, &e, files);
            } else if !name.starts_with(kb::DB_NAME) {
                let rel = e.strip_prefix(base).ok().and_then(|r| r.to_str()).unwrap_or_default().to_string();
                let text = String::from_utf8_lossy(&std::fs::read(&e).unwrap_or_default()).into_owned();
                files.insert(rel, json!(self.norm(&text)));
            }
        }
    }

    fn snapshot(&self, d: &str) -> Value {
        let base = self.p(d);
        let mut files = Map::new();
        self.walk(&base, &base, &mut files);
        json!({"files": files, "db": dump_db(&base.join(kb::DB_NAME))})
    }
}

/// 兩份快照哪裡不同（只列出鍵，不然訊息太長）。
fn snap_diff(want: &Value, got: &Value) -> String {
    let mut out = Vec::new();
    for part in ["files", "db"] {
        let (w, g) = (&want[part], &got[part]);
        match (w.as_object(), g.as_object()) {
            (Some(wo), Some(go)) => {
                let keys: HashSet<&String> = wo.keys().chain(go.keys()).collect();
                let mut bad: Vec<&String> = keys.into_iter().filter(|k| wo.get(*k) != go.get(*k)).collect();
                bad.sort();
                for k in bad {
                    out.push(format!("{part}.{k}\n      要：{}\n      得：{}", wo.get(k).unwrap_or(&Value::Null), go.get(k).unwrap_or(&Value::Null)));
                }
            }
            _ if w != g => out.push(format!("{part}\n      要：{w}\n      得：{g}")),
            _ => {}
        }
    }
    out.join("\n    ")
}

#[test]
fn steps_match_python() {
    let exp = expected();
    let nanos = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let td = std::env::temp_dir().join(format!("kb-parity-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&td).expect("建暫存目錄");
    let st = Stage { td: td.to_string_lossy().into_owned(), real: std::fs::canonicalize(&td).expect("暫存目錄").to_string_lossy().into_owned() };
    let mut last_snap: HashMap<String, Value> = HashMap::new();
    let mut bad: Vec<String> = Vec::new();
    let (mut runs, mut same) = (0usize, 0usize);
    let steps = exp["steps"].as_array().expect("steps");
    for (i, step) in steps.iter().enumerate() {
        let path = s(&step["path"]);
        match s(&step["op"]) {
            "write" => {
                let data = match step.get("hex") {
                    Some(h) => (0..s(h).len() / 2).filter_map(|k| u8::from_str_radix(s(h).get(k * 2..k * 2 + 2)?, 16).ok()).collect(),
                    None => s(&step["text"]).as_bytes().to_vec(),
                };
                let full = st.p(path);
                std::fs::create_dir_all(full.parent().expect("上層目錄")).expect("建目錄");
                std::fs::write(&full, data).unwrap_or_else(|e| panic!("寫不進 {path}：{e}"));
                st.set_mtime(path, step["mtime"].as_u64().expect("mtime"));
            }
            "utime" => st.set_mtime(path, step["mtime"].as_u64().expect("mtime")),
            "rm" => std::fs::remove_file(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "rmtree" => std::fs::remove_dir_all(st.p(path)).unwrap_or_else(|e| panic!("刪不掉 {path}：{e}")),
            "symlink" => {
                let full = st.p(path);
                std::fs::create_dir_all(full.parent().expect("上層目錄")).expect("建目錄");
                std::os::unix::fs::symlink(s(&step["target"]), &full).unwrap_or_else(|e| panic!("建不了連結 {path}：{e}"));
            }
            "sql" => {
                let con = rusqlite::Connection::open(st.p(path)).expect("開資料庫");
                con.execute_batch(s(&step["sql"])).expect("執行 SQL");
            }
            "fts5" => kb::set_fts5_override(step["value"].as_bool()),
            "run" => {
                runs += 1;
                let shown: Vec<&str> = step["argv"].as_array().expect("argv").iter().map(s).collect();
                let argv: Vec<String> = shown.iter().map(|a| a.replace("@TMP@", &st.td)).collect();
                let (code, out, err) = kb::run_captured(&argv);
                let (out, err) = (st.norm(&out), st.norm(&err));
                let mut diff = Vec::new();
                if Some(i64::from(code)) != step["code"].as_i64() {
                    diff.push(format!("exit code 要 {}、得 {code}", step["code"]));
                }
                if out != s(&step["stdout"]) {
                    diff.push(format!("stdout\n    要：{:?}\n    得：{out:?}", s(&step["stdout"])));
                }
                if err != s(&step["stderr"]) {
                    diff.push(format!("stderr\n    要：{:?}\n    得：{err:?}", s(&step["stderr"])));
                }
                for (d, want) in step.get("snap").and_then(Value::as_object).into_iter().flatten() {
                    let got = st.snapshot(d);
                    let want = if *want == "same" { last_snap.get(d).cloned().unwrap_or(Value::Null) } else { want.clone() };
                    if want != got {
                        diff.push(format!("索引目錄 {d}\n    {}", snap_diff(&want, &got)));
                    }
                    last_snap.insert(d.clone(), want);
                }
                if diff.is_empty() {
                    same += 1;
                } else {
                    bad.push(format!("第 {i} 步 {shown:?}\n  {}", diff.join("\n  ")));
                }
            }
            other => panic!("劇本有不認得的步驟：{other}"),
        }
    }
    kb::set_fts5_override(None);
    let _ = std::fs::remove_dir_all(&td);
    println!("kb 對跑：{runs} 個指令，{same} 個逐字相同");
    assert!(runs >= 300, "指令數不對：{runs}");
    assert!(bad.is_empty(), "{} 個指令不同（共 {runs} 個）：\n{}", bad.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn probes_match_python() {
    let exp = expected();
    let pr = &exp["probes"];
    let mut bad: Vec<String> = Vec::new();
    let mut n = 0usize;
    let mut check = |what: &str, want: &Value, got: Value| {
        n += 1;
        if want != &got {
            bad.push(format!("{what}\n  要：{want}\n  得：{got}"));
        }
    };
    for c in pr["tokenize"].as_array().expect("tokenize") {
        let t = s(&c["text"]);
        check(&format!("tokenize（索引）{t:?}"), &c["index"], json!(kb::tokenize(t, false)));
        check(&format!("tokenize（查詢）{t:?}"), &c["query"], json!(kb::tokenize(t, true)));
        check(&format!("query_terms {t:?}"), &c["terms"], json!(kb::query_terms(t)));
    }
    for c in pr["to_posix"].as_array().expect("to_posix") {
        check(&format!("to_posix {}", c[0]), &c[1], json!(kb::to_posix(s(&c[0]))));
    }
    for c in pr["neutralize"].as_array().expect("neutralize") {
        check(&format!("neutralize {}", c[0]), &c[1], json!(kb::neutralize(s(&c[0]))));
    }
    for c in pr["is_sensitive"].as_array().expect("is_sensitive") {
        check(&format!("is_sensitive {}", c[0]), &c[1], json!(kb::is_sensitive(s(&c[0]))));
    }
    for c in pr["glob"].as_array().expect("glob") {
        let rx = kb::glob_regex(s(&c["pattern"])).expect("glob 編得出來");
        for p in c["paths"].as_array().expect("paths") {
            check(&format!("glob {} 對 {}", c["pattern"], p[0]), &p[1], json!(rx.is_match(s(&p[0]))));
        }
    }
    let mut used = HashSet::new();
    for c in pr["card_name"].as_array().expect("card_name") {
        check(&format!("card_name {}", c[0]), &c[1], json!(kb::card_name(s(&c[0]), &mut used)));
    }
    for c in pr["split"].as_array().expect("split") {
        let d = kb::split_document(s(&c["text"]), c["markdown"].as_bool().unwrap_or(true));
        let chunks: Vec<Value> = d.chunks.iter()
            .map(|ch| json!({"start": ch.start, "end": ch.end, "level": ch.level, "headings": ch.headings, "sec": ch.sec, "part": ch.part, "text": ch.text}))
            .collect();
        let links: Vec<Value> = d.links.iter().map(|(ln, kind, t)| json!([ln, kind, t])).collect();
        let got = json!({"nlines": d.nlines, "title": d.title, "frontmatter": d.frontmatter, "chunks": chunks, "links": links});
        let head: String = s(&c["text"]).chars().take(40).collect();
        check(&format!("split_document {head:?}"), &c["doc"], got);
    }
    println!("kb 探測案例：{n} 個");
    assert!(n >= 150, "探測案例數不對：{n}");
    assert!(bad.is_empty(), "{} 個探測案例不同（共 {n} 個）：\n{}", bad.len(), bad.join("\n"));
}
