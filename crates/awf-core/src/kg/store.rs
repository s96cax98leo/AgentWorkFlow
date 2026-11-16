//! 知識圖的資料庫：一個 SQLite 檔 `<knowledge.dir>/graph.db`。
//!
//! 寫入一律寫到暫存檔、完成後改名（原子寫入）；查詢一律唯讀開啟。
//! 資料表分三類：每個檔自己的事實（`owner`＝檔案路徑，檔案變了才重算）、
//! 全域來源（規格、決策、連結帳本、git；`owner` 以 `@` 開頭）、跨檔推導的結果（`@derive`，每次重算）。

use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// 改了資料表就遞增：舊圖整份重建。
pub const SCHEMA_VERSION: &str = "1";
/// 改了解析、分詞或推導規則就遞增：舊圖整份重建。
pub const CONTENT_VERSION: &str = "1";
pub const DB_NAME: &str = "graph.db";
pub const STATE_READY: &str = "ready";
pub const STATE_BUILDING: &str = "building";

pub const SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID;
CREATE TABLE files(path TEXT PRIMARY KEY, src TEXT, lang TEXT, size INTEGER, mtime_ns INTEGER, sha256 TEXT, nlines INTEGER, is_test INTEGER) WITHOUT ROWID;
CREATE TABLE nodes(id TEXT PRIMARY KEY, kind TEXT, sub TEXT, name TEXT, path TEXT, start INTEGER, end INTEGER, ntok INTEGER, owner TEXT) WITHOUT ROWID;
CREATE INDEX nodes_owner ON nodes(owner);
CREATE INDEX nodes_path ON nodes(path);
CREATE TABLE edges(src TEXT, dst TEXT, kind TEXT, strength TEXT, provenance TEXT, confidence REAL, owner TEXT, PRIMARY KEY(src, dst, kind, provenance)) WITHOUT ROWID;
CREATE INDEX edges_dst ON edges(dst);
CREATE INDEX edges_owner ON edges(owner);
CREATE TABLE unresolved(src TEXT, kind TEXT, target TEXT, path TEXT, line INTEGER, reason TEXT, PRIMARY KEY(src, kind, target)) WITHOUT ROWID;
CREATE TABLE terms(term TEXT, node TEXT, w INTEGER, owner TEXT, PRIMARY KEY(term, node)) WITHOUT ROWID;
CREATE INDEX terms_owner ON terms(owner);
CREATE TABLE f_imports(path TEXT, seq INTEGER, module TEXT, names TEXT, line INTEGER, PRIMARY KEY(path, seq)) WITHOUT ROWID;
CREATE TABLE f_calls(path TEXT, seq INTEGER, name TEXT, qual TEXT, scope TEXT, line INTEGER, PRIMARY KEY(path, seq)) WITHOUT ROWID;
CREATE TABLE f_inherits(path TEXT, seq INTEGER, scope TEXT, target TEXT, kind TEXT, line INTEGER, PRIMARY KEY(path, seq)) WITHOUT ROWID;
CREATE TABLE f_refs(path TEXT, seq INTEGER, kind TEXT, target TEXT, scope TEXT, line INTEGER, PRIMARY KEY(path, seq)) WITHOUT ROWID;
CREATE TABLE f_commits(sha TEXT, path TEXT, seq INTEGER, PRIMARY KEY(sha, path)) WITHOUT ROWID;
";

/// 所有資料表（測試比對內容用）。
pub const TABLES: [&str; 11] = ["meta", "files", "nodes", "edges", "unresolved", "terms", "f_imports", "f_calls", "f_inherits", "f_refs", "f_commits"];
/// 每個檔自己的事實所在的表與欄位。
const OWNED: [(&str, &str); 7] = [("nodes", "owner"), ("edges", "owner"), ("terms", "owner"), ("f_imports", "path"), ("f_calls", "path"), ("f_inherits", "path"), ("f_refs", "path")];

pub fn e(err: rusqlite::Error) -> String {
    format!("資料庫錯誤：{err}")
}

/// 開一個新的（或複製來的）暫存資料庫來寫。
pub fn open_rw(path: &str, fresh: bool) -> Result<Connection, String> {
    let con = Connection::open(path).map_err(e)?;
    con.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA temp_store=MEMORY;").map_err(e)?;
    if fresh {
        con.execute_batch(SCHEMA).map_err(e)?;
    }
    Ok(con)
}

/// 唯讀開啟（多個 agent 同時查不會互相卡住，也不會動到檔案）。
pub fn open_ro(path: &str) -> Result<Connection, String> {
    let con = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(e)?;
    con.busy_timeout(std::time::Duration::from_secs(3)).map_err(e)?;
    con.execute_batch("PRAGMA query_only=1").map_err(e)?;
    Ok(con)
}

pub fn meta(con: &Connection) -> Result<BTreeMap<String, String>, String> {
    let mut st = con.prepare("SELECT key, value FROM meta").map_err(e)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default()))).map_err(e)?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (k, v) = row.map_err(e)?;
        out.insert(k, v);
    }
    Ok(out)
}

pub fn set_meta(con: &Connection, key: &str, value: &str) -> Result<(), String> {
    con.execute("INSERT OR REPLACE INTO meta VALUES(?,?)", params![key, value]).map_err(e)?;
    Ok(())
}

/// 這份圖能不能用：版本相同而且上次有建完。回傳不能用的原因。
pub fn usable(m: &BTreeMap<String, String>) -> Result<(), String> {
    let get = |k: &str| m.get(k).map(String::as_str).unwrap_or("");
    if get("schema_version") != SCHEMA_VERSION || get("content_version") != CONTENT_VERSION {
        return Err("圖的版本和這個 awf 不同".to_string());
    }
    if get("index_state") != STATE_READY {
        return Err("圖上次沒有建完".to_string());
    }
    Ok(())
}

/// 刪掉一個檔的全部事實。
pub fn delete_owner(con: &Connection, owner: &str) -> Result<(), String> {
    for (table, col) in OWNED {
        con.prepare_cached(&format!("DELETE FROM {table} WHERE {col}=?")).map_err(e)?.execute([owner]).map_err(e)?;
    }
    con.prepare_cached("DELETE FROM files WHERE path=?").map_err(e)?.execute([owner]).map_err(e)?;
    Ok(())
}

/// 刪掉全域來源與推導的結果（`owner` 以 `@` 開頭的）。
pub fn delete_global(con: &Connection) -> Result<(), String> {
    for table in ["nodes", "edges", "terms"] {
        con.execute(&format!("DELETE FROM {table} WHERE owner LIKE '@%'"), []).map_err(e)?;
    }
    con.execute("DELETE FROM unresolved", []).map_err(e)?;
    Ok(())
}

pub struct NodeRow<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub sub: &'a str,
    pub name: &'a str,
    pub path: &'a str,
    pub start: usize,
    pub end: usize,
    pub ntok: usize,
    pub owner: &'a str,
}

pub fn put_node(con: &Connection, n: &NodeRow) -> Result<(), String> {
    con.prepare_cached("INSERT OR IGNORE INTO nodes VALUES(?,?,?,?,?,?,?,?,?)")
        .map_err(e)?
        .execute(params![n.id, n.kind, n.sub, n.name, n.path, n.start as i64, n.end as i64, n.ntok as i64, n.owner])
        .map_err(e)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn put_edge(con: &Connection, src: &str, dst: &str, kind: &str, strength: &str, provenance: &str, confidence: f64, owner: &str) -> Result<(), String> {
    if src == dst {
        return Ok(());
    }
    con.prepare_cached("INSERT OR IGNORE INTO edges VALUES(?,?,?,?,?,?,?)").map_err(e)?.execute(params![src, dst, kind, strength, provenance, confidence, owner]).map_err(e)?;
    Ok(())
}

pub fn put_unresolved(con: &Connection, src: &str, kind: &str, target: &str, path: &str, line: usize, reason: &str) -> Result<(), String> {
    con.prepare_cached("INSERT OR IGNORE INTO unresolved VALUES(?,?,?,?,?,?)").map_err(e)?.execute(params![src, kind, target, path, line as i64, reason]).map_err(e)?;
    Ok(())
}

pub fn put_terms(con: &Connection, node: &str, weights: &BTreeMap<String, i64>, owner: &str) -> Result<(), String> {
    let mut st = con.prepare_cached("INSERT OR IGNORE INTO terms VALUES(?,?,?,?)").map_err(e)?;
    for (t, w) in weights {
        st.execute(params![t, node, w, owner]).map_err(e)?;
    }
    Ok(())
}

fn cell(v: rusqlite::types::ValueRef) -> String {
    use rusqlite::types::ValueRef as V;
    match v {
        V::Null => "∅".to_string(),
        V::Integer(i) => i.to_string(),
        V::Real(f) => format!("{f:.4}"),
        V::Text(t) => String::from_utf8_lossy(t).into_owned(),
        V::Blob(b) => format!("<{} bytes>", b.len()),
    }
}

/// 一張表排序後的內容，一列一行（欄位用 `\u{1f}` 分隔）。
pub fn dump_table(con: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut st = con.prepare(&format!("SELECT * FROM {table}")).map_err(e)?;
    let n = st.column_count();
    let mut rows = st.query([]).map_err(e)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(e)? {
        let cells: Vec<String> = (0..n).map(|i| r.get_ref(i).map(cell).unwrap_or_default()).collect();
        out.push(cells.join("\u{1f}"));
    }
    out.sort();
    Ok(out)
}

/// 全部資料表排序後的內容（測試「同一份輸入產生相同的圖」用）。
pub fn dump_all(db: &str) -> Result<String, String> {
    let con = open_ro(db)?;
    let mut out = String::new();
    for t in TABLES {
        out.push_str(&format!("== {t}\n"));
        for row in dump_table(&con, t)? {
            out.push_str(&row);
            out.push('\n');
        }
    }
    Ok(out)
}

/// 圖的內容雜湊：節點、邊、未解析三張表排序後的 sha256。
pub fn graph_hash(con: &Connection) -> Result<String, String> {
    let mut h = Sha256::new();
    for t in ["nodes", "edges", "unresolved", "terms"] {
        h.update(t.as_bytes());
        for row in dump_table(con, t)? {
            h.update(row.as_bytes());
            h.update(b"\n");
        }
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn count(con: &Connection, table: &str) -> usize {
    con.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
}
