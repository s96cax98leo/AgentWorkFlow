//! 和 Python `pathlib`／`os.path` 相同的路徑運算，路徑一律用字串表示（分隔符是 `/`）。
//!
//! 守門的判定是「解析後的路徑在不在某個目錄底下」，細節照 Python：`Path()` 的正規化（去掉 `.`、重複斜線、結尾斜線）、
//! `resolve()`（逐層解開 symlink，不存在的部分照字面處理 `..`）、`relative_to()`（逐段比對）。
//! Windows：反斜線先換成 `/`，磁碟代號開頭（`C:/…`）與 UNC（`//server/share/…`）算絕對路徑，逐段比對不分大小寫。

use crate::config::PyErr;
use std::collections::HashMap;

/// `str(Path(s))`（分隔符固定用 `/`）。
pub fn pp(s: &str) -> String {
    #[cfg(windows)]
    let owned = s.replace('\\', "/");
    #[cfg(windows)]
    let s: &str = &owned;
    let (drive, rest) = split_drive(s);
    if drive.is_empty() && rest.is_empty() {
        return ".".into();
    }
    let abs = rest.starts_with('/');
    let dbl = cfg!(not(windows)) && rest.starts_with("//") && !rest.starts_with("///");
    let body = rest.split('/').filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join("/");
    let mut out = String::from(drive);
    if abs {
        out.push_str(if dbl { "//" } else { "/" });
        out.push_str(&body);
    } else if body.is_empty() && drive.is_empty() {
        out.push('.');
    } else {
        out.push_str(&body);
    }
    out
}

/// 磁碟代號或 UNC 前綴（只有 Windows 有）與其餘部分。
fn split_drive(s: &str) -> (&str, &str) {
    if cfg!(windows) {
        let b = s.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            return (&s[..2], &s[2..]);
        }
        if s.starts_with("//") && !s.starts_with("///") {
            // //server/share
            let mut it = s[2..].splitn(3, '/');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                if !a.is_empty() && !b.is_empty() {
                    let n = 2 + a.len() + 1 + b.len();
                    return (&s[..n], &s[n..]);
                }
            }
        }
    }
    ("", s)
}

/// `Path(s).is_absolute()`。
pub fn is_abs(s: &str) -> bool {
    if cfg!(windows) {
        let t = s.replace('\\', "/");
        let (d, r) = split_drive(&t);
        !d.is_empty() && r.starts_with('/')
    } else {
        s.starts_with('/')
    }
}

/// `os.path.isabs(s)`。Windows 上（Python 3.12 以前）斜線開頭就算。
pub fn os_isabs(s: &str) -> bool {
    if cfg!(windows) {
        let t = s.replace('\\', "/");
        let (_, r) = split_drive(&t);
        r.starts_with('/')
    } else {
        s.starts_with('/')
    }
}

/// `Path(base) / p`。
pub fn join(base: &str, p: &str) -> String {
    if is_abs(p) || (cfg!(windows) && os_isabs(p)) {
        return pp(p);
    }
    if p.is_empty() {
        return pp(base);
    }
    pp(&format!("{}/{}", base, p))
}

/// `Path(s).parts`。
pub fn parts(s: &str) -> Vec<String> {
    let n = pp(s);
    let (drive, rest) = split_drive(&n);
    let mut out = Vec::new();
    if rest.starts_with('/') {
        out.push(format!("{}{}", drive, if rest.starts_with("//") && cfg!(not(windows)) { "//" } else { "/" }));
    } else if !drive.is_empty() {
        out.push(drive.to_string());
    }
    for c in rest.split('/') {
        if !c.is_empty() && c != "." {
            out.push(c.to_string());
        }
    }
    out
}

fn is_anchor(s: &str) -> bool {
    let (_, rest) = split_drive(s);
    rest.is_empty() || rest.chars().all(|c| c == '/')
}

/// `Path(s).name`。
pub fn name(s: &str) -> String {
    let n = pp(s);
    if n == "." || is_anchor(&n) {
        return String::new();
    }
    let (_, rest) = split_drive(&n);
    rest.rsplit('/').next().unwrap_or("").to_string()
}

/// `Path(s).parent`。
pub fn parent(s: &str) -> String {
    let n = pp(s);
    if n == "." || is_anchor(&n) {
        return n;
    }
    let (drive, rest) = split_drive(&n);
    match rest.rfind('/') {
        Some(0) => format!("{}/", drive),
        Some(i) => {
            let head = &rest[..i];
            if head.chars().all(|c| c == '/') { format!("{}{}", drive, head) } else { format!("{}{}", drive, head) }
        }
        None => if drive.is_empty() { ".".into() } else { drive.to_string() },
    }
}

/// `Path(s).suffix`。
pub fn suffix(s: &str) -> String {
    let n = name(s);
    match n.rfind('.') {
        Some(i) if i > 0 && i + 1 < n.len() => n[i..].to_string(),
        _ => String::new(),
    }
}

pub fn exists(s: &str) -> bool {
    !s.contains('\0') && std::fs::metadata(s).is_ok()
}

pub fn is_file(s: &str) -> bool {
    !s.contains('\0') && std::fs::metadata(s).map(|m| m.is_file()).unwrap_or(false)
}

pub fn is_dir(s: &str) -> bool {
    !s.contains('\0') && std::fs::metadata(s).map(|m| m.is_dir()).unwrap_or(false)
}

/// `os.getcwd()`。
pub fn cwd() -> Result<String, PyErr> {
    std::env::current_dir()
        .map(|p| {
            let s = p.to_string_lossy().into_owned();
            if cfg!(windows) { s.replace('\\', "/") } else { s }
        })
        .map_err(|e| PyErr::new("FileNotFoundError", &e.to_string()))
}

#[cfg(not(windows))]
fn join_real(mut path: String, rest: &str, seen: &mut HashMap<String, Option<String>>, depth: usize) -> (String, bool) {
    let mut rest = rest.to_string();
    if rest.starts_with('/') {
        rest = rest.trim_start_matches('/').to_string();
        path = "/".into();
    }
    let mut remaining: &str = &rest;
    while !remaining.is_empty() {
        let (comp, tail) = match remaining.find('/') {
            Some(i) => (&remaining[..i], &remaining[i + 1..]),
            None => (remaining, ""),
        };
        remaining = tail;
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            if path.is_empty() {
                path = "..".into();
            } else if path != "/" {
                match path.rfind('/') {
                    Some(0) => path = "/".into(),
                    Some(i) => path.truncate(i),
                    None => path.clear(),
                }
            }
            continue;
        }
        let newpath = if path.ends_with('/') { format!("{}{}", path, comp) } else { format!("{}/{}", path, comp) };
        let is_link = std::fs::symlink_metadata(&newpath).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(prev) = seen.get(&newpath) {
            match prev {
                Some(p) => {
                    path = p.clone();
                    continue;
                }
                None => return (join_tail(&newpath, remaining), false), // symlink 迴圈
            }
        }
        if depth > 64 {
            return (join_tail(&newpath, remaining), false);
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(&newpath) {
            Ok(t) => t.to_string_lossy().into_owned(),
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        let (p, ok) = join_real(path.clone(), &target, seen, depth + 1);
        if !ok {
            return (join_tail(&p, remaining), false);
        }
        path = p;
        seen.insert(newpath, Some(path.clone()));
    }
    (path, true)
}

#[cfg(not(windows))]
fn join_tail(a: &str, rest: &str) -> String {
    if rest.is_empty() {
        a.to_string()
    } else if a.ends_with('/') {
        format!("{}{}", a, rest)
    } else {
        format!("{}/{}", a, rest)
    }
}

#[cfg(not(windows))]
fn realpath(abs: &str) -> String {
    let mut seen = HashMap::new();
    let (p, _) = join_real(String::new(), abs, &mut seen, 0);
    // os.path.abspath：再做一次字面正規化
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    format!("/{}", out.join("/"))
}

#[cfg(windows)]
fn realpath(abs: &str) -> String {
    let _unused: HashMap<String, Option<String>> = HashMap::new();
    let n = pp(abs);
    let ps = parts(&n);
    // 字面處理 ..
    let mut stack: Vec<String> = Vec::new();
    for (i, c) in ps.iter().enumerate() {
        if c == ".." && i > 0 {
            if stack.len() > 1 {
                stack.pop();
            }
        } else {
            stack.push(c.clone());
        }
    }
    // 最長的存在前綴交給系統解析（symlink、junction、大小寫），其餘照字面接上
    let mut k = stack.len();
    while k > 0 {
        let prefix = glue(&stack[..k]);
        if let Ok(c) = std::fs::canonicalize(&prefix) {
            let mut s = c.to_string_lossy().replace('\\', "/");
            if let Some(r) = s.strip_prefix("//?/UNC/") {
                s = format!("//{}", r);
            } else if let Some(r) = s.strip_prefix("//?/") {
                s = r.to_string();
            }
            let mut out = s.trim_end_matches('/').to_string();
            if out.len() == 2 && out.ends_with(':') {
                out.push('/');
            }
            for c in &stack[k..] {
                if !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(c);
            }
            return out;
        }
        k -= 1;
    }
    glue(&stack)
}

#[cfg(windows)]
fn glue(ps: &[String]) -> String {
    let mut out = String::new();
    for (i, c) in ps.iter().enumerate() {
        if i > 0 && !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(c);
    }
    out
}

/// `Path(s).resolve()`。含 NUL 回 ValueError；symlink 迴圈回 RuntimeError；取不到目前目錄回 OSError 類。
pub fn resolve(s: &str) -> Result<String, PyErr> {
    if s.contains('\0') {
        return Err(PyErr::new("ValueError", "embedded null byte"));
    }
    let n = pp(s);
    let abs = if is_abs(&n) { n } else { join(&cwd()?, &n) };
    let r = realpath(&abs);
    if let Err(e) = std::fs::metadata(&r) {
        let code = e.raw_os_error().unwrap_or(0);
        let eloop = if cfg!(target_os = "linux") { 40 } else if cfg!(windows) { 1921 } else { 62 };
        if code == eloop {
            return Err(PyErr::new("RuntimeError", &format!("Symlink loop from '{}'", r)));
        }
    }
    Ok(r)
}

/// `Path(t).relative_to(root).as_posix()`；不在底下回 None（Python 丟 ValueError）。相同時是 "."。
pub fn relative_to(t: &str, root: &str) -> Option<String> {
    let a = parts(t);
    let b = parts(root);
    if b.len() > a.len() {
        return None;
    }
    let same = |x: &str, y: &str| if cfg!(windows) { x.to_lowercase() == y.to_lowercase() } else { x == y };
    if !a.iter().zip(&b).all(|(x, y)| same(x, y)) {
        return None;
    }
    let rest = &a[b.len()..];
    Some(if rest.is_empty() { ".".into() } else { rest.join("/") })
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty())
}

/// 家目錄（`os.path.expanduser("~")` 的來源）。
pub fn home() -> Option<String> {
    if cfg!(windows) {
        env_nonempty("USERPROFILE").or_else(|| match (env_nonempty("HOMEDRIVE"), env_nonempty("HOMEPATH")) {
            (d, Some(p)) => Some(format!("{}{}", d.unwrap_or_default(), p)),
            _ => None,
        }).map(|h| h.replace('\\', "/"))
    } else {
        env_nonempty("HOME")
    }
}

/// `os.path.expanduser(p)`：只展開 `~` 與 `~/…`；`~使用者` 原樣（Python 會查使用者資料庫，這裡不查）。
pub fn expanduser(p: &str) -> String {
    if !p.starts_with('~') {
        return p.to_string();
    }
    let end = p.find(|c| c == '/' || (cfg!(windows) && c == '\\')).unwrap_or(p.len());
    if end != 1 {
        return p.to_string();
    }
    let Some(h) = home() else { return p.to_string() };
    let h = h.trim_end_matches('/');
    let out = format!("{}{}", h, &p[end..]);
    if out.is_empty() { "/".into() } else { out }
}

/// `tempfile.gettempdir()`：TMPDIR、TEMP、TMP，再來是平台預設；取第一個存在的目錄。
pub fn gettempdir() -> String {
    let mut cands: Vec<String> = ["TMPDIR", "TEMP", "TMP"].iter().filter_map(|k| env_nonempty(k)).collect();
    if cfg!(windows) {
        cands.push(std::env::temp_dir().to_string_lossy().into_owned());
    } else {
        cands.extend(["/tmp", "/var/tmp", "/usr/tmp"].iter().map(|s| s.to_string()));
    }
    for c in cands {
        if is_dir(&c) {
            let c = if cfg!(windows) { c.replace('\\', "/") } else { c };
            // os.path.abspath：去掉結尾斜線
            let n = pp(&c);
            return if is_abs(&n) { n } else { cwd().map(|d| join(&d, &n)).unwrap_or(n) };
        }
    }
    cwd().unwrap_or_else(|_| ".".into())
}

/// 訊息裡的路徑寫法（`str(Path)`）：Windows 用反斜線。
pub fn display(s: &str) -> String {
    if cfg!(windows) { s.replace('/', "\\") } else { s.to_string() }
}

/// 在 PATH 上找可執行檔（`shutil.which`）。
pub fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let mut v = vec![String::new()];
        v.extend(env_nonempty("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into()).split(';').map(|s| s.to_string()));
        v
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("{}{}", name, ext));
            let Ok(md) = std::fs::metadata(&cand) else { continue };
            if !md.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if md.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Some(cand.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn normalize_like_pathlib() {
        assert_eq!(pp(""), ".");
        assert_eq!(pp("a/./b//c/"), "a/b/c");
        assert_eq!(pp("//a//b"), "//a/b");
        assert_eq!(pp("///a"), "/a");
        assert_eq!(pp("/"), "/");
        assert_eq!(name("/a/b.txt"), "b.txt");
        assert_eq!(name("/"), "");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("a"), ".");
        assert_eq!(parent("/a/b/c"), "/a/b");
        assert_eq!(suffix(".env"), "");
        assert_eq!(suffix("a.tar.gz"), ".gz");
        assert_eq!(parts("/a/b"), vec!["/", "a", "b"]);
        assert_eq!(join("/a", "/b"), "/b");
        assert_eq!(join("/a", "b/../c"), "/a/b/../c");
    }

    #[test]
    fn relative() {
        assert_eq!(relative_to("/a/b/c", "/a").as_deref(), Some("b/c"));
        assert_eq!(relative_to("/a", "/a").as_deref(), Some("."));
        assert_eq!(relative_to("/ab", "/a"), None);
        assert_eq!(resolve("/nonexistent-awf/x/../y").unwrap(), "/nonexistent-awf/y");
        assert_eq!(resolve("a\0b").unwrap_err().class, "ValueError");
    }
}
