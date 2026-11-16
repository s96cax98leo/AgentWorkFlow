//! 各項檢查共用的小工具：找執行檔、跑子程序、讀檔、指令列的 `--root`。

use awf_core::merge_gate::pyutil;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 一次指令的結果：（exit code、stdout、stderr）。
pub type Captured = (i32, String, String);

/// repo 裡的模版根目錄（plugin 目錄）。xtask 的 `root` 一律是 repo 根目錄；模版裡的位置都由 `awf_core::template` 決定。
pub fn plugin_dir(root: &Path) -> PathBuf {
    root.join("plugin")
}

/// 模版的 `.claude/teams`（schema、golden-set、內建 profile 都在這裡）。
pub fn teams_dir(root: &Path) -> PathBuf {
    awf_core::template::teams(&plugin_dir(root))
}

/// 共用片段的目錄。
pub fn snippet_dir(root: &Path) -> PathBuf {
    awf_core::template::snippets(&plugin_dir(root))
}

/// `shutil.which(name)`：照 PATH 找執行檔；Windows 另外試 PATHEXT 的副檔名。
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        std::iter::once(String::new()).chain(raw.split(';').filter(|e| !e.is_empty()).map(str::to_string)).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// 子程序的結果，輸出已經照 UTF-8 解成文字（解不了的位元組換成替代字元）。
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// 跑子程序並收輸出；跑不起來回說明文字。
pub fn run(cmd: &mut Command, input: Option<&str>) -> Result<Ran, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    match pyutil::run(cmd, input.map(|s| s.as_bytes().to_vec()), None) {
        Ok(o) => Ok(Ran { code: o.code, stdout: text(&o.stdout), stderr: text(&o.stderr) }),
        Err(pyutil::RunErr::Os(e)) => Err(format!("跑不起來 {program}：{e}")),
        Err(pyutil::RunErr::Timeout) => Err(format!("{program} 逾時")),
    }
}

/// 子程序輸出的解碼：UTF-8（壞掉的位元組換成替代字元），`\r\n` 與 `\r` 轉成 `\n`（同 Python 的文字模式）。
fn text(bytes: &[u8]) -> String {
    pyutil::universal_newlines(&String::from_utf8_lossy(bytes))
}

/// `node -e <js> <args…>`。
pub fn node_eval(node: &Path, js: &str, args: &[PathBuf], input: Option<&str>) -> Result<Ran, String> {
    let mut cmd = Command::new(node);
    cmd.arg("-e").arg(js).args(args);
    run(&mut cmd, input)
}

/// `path.read_text(encoding="utf-8")`：嚴格 UTF-8，`\r\n`、`\r` 轉成 `\n`。
pub fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} 不是 UTF-8", path.display()))?;
    Ok(pyutil::universal_newlines(&text))
}

/// 讀 JSON 檔。
pub fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    serde_json::from_str(&read_text(path)?).map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))
}

/// 從參數裡拿掉 `<name> <值>`（或 `<name>=<值>`），回（值、剩下的參數）。有給名字卻沒給值回錯誤。
pub fn take_opt(args: &[String], name: &str) -> Result<(Option<String>, Vec<String>), String> {
    let mut value = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    let prefix = format!("{name}=");
    while let Some(a) = it.next() {
        if a == name {
            match it.next() {
                Some(v) => value = Some(v.clone()),
                None => return Err(format!("{name} 後面要接值")),
            }
        } else if let Some(v) = a.strip_prefix(&prefix) {
            value = Some(v.to_string());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((value, rest))
}

/// 暫存目錄（結束時刪掉）。名字帶行程編號與流水號，同一個行程裡不會撞。
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let name = format!("{prefix}{}-{}-{nanos}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
        let p = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&p).map_err(|e| format!("建不了暫存目錄 {}：{e}", p.display()))?;
        Ok(Self(p.canonicalize().unwrap_or(p)))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
