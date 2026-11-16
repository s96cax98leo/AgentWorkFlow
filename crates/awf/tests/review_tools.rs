//! 1.0 第二次審查留下的工具端小項：守門端已經擋 team agent，這裡是工具自己也要拒絕。
//!
//! - 會直接交給 git 的值不能以 `-` 開頭（否則被當成 git 的選項，例如 `--output=<檔>`）。
//! - `awf kb` 的索引：連結解開後落在機敏目錄的檔不收。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
static SEQ: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("awf-review-tools-{}-{}-{}", std::process::id(), n, tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建立暫存目錄");
        Tmp(d.canonicalize().expect("解開暫存目錄"))
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 回（exit code，stdout＋stderr）。環境拿掉 `CLAUDE_*`、`AWF_*`。
fn awf(cwd: &Path, args: &[&str]) -> (i32, String) {
    let mut c = Command::new(EXE);
    c.args(args).current_dir(cwd);
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
            c.env_remove(k);
        }
    }
    let o = c.output().expect("執行 awf");
    (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn git_init(d: &Path) {
    let ok = Command::new("git").args(["init", "-q", "."]).current_dir(d).status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "git init");
}

#[test]
fn check_rules_git_refuses_values_that_look_like_options() {
    let t = Tmp::new("cr");
    git_init(&t.0);
    let (code, out) = awf(&t.0, &["check-rules", "--git", ".", "--output=x.txt"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("不能以 - 開頭"), "{out}");
    assert!(!t.0.join("x.txt").exists(), "git 的 --output 不該被執行");
}

#[test]
fn recipe_check_diff_refuses_a_base_that_looks_like_an_option() {
    let t = Tmp::new("rc");
    git_init(&t.0);
    std::fs::create_dir_all(t.0.join("r")).expect("建立 run 目錄");
    std::fs::write(t.0.join("r/run.json"), "{\"recipe\":{\"name\":\"feature\"}}").expect("寫 run.json");
    let (code, out) = awf(&t.0, &["recipe", "check-diff", "--run", "r", "--base", "--output=y.txt"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("不能以 - 開頭"), "{out}");
    assert!(!t.0.join("y.txt").exists(), "git 的 --output 不該被執行");
}

#[test]
fn sync_branch_refuses_a_remote_that_looks_like_an_option() {
    let t = Tmp::new("sb");
    git_init(&t.0);
    let (code, out) = awf(&t.0, &["sync-branch", "plan", "--repo", ".", "--branch", "main", "--remote=--upload-pack=touch pwn"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("\"halted\": true") && out.contains("不能以 - 開頭"), "{out}");
    assert!(!t.0.join("pwn").exists(), "--upload-pack 的指令不該被執行");
}

#[cfg(unix)]
#[test]
fn kb_does_not_index_a_link_into_a_secrets_directory() {
    let t = Tmp::new("kb");
    git_init(&t.0);
    std::fs::create_dir_all(t.0.join("docs")).expect("docs");
    std::fs::create_dir_all(t.0.join("secrets")).expect("secrets");
    std::fs::write(t.0.join("docs/a.md"), "# 正常\n\n內文 okmarker\n").expect("a.md");
    std::fs::write(t.0.join("secrets/prod.md"), "# prod\n\nkbsecretq 不該被索引\n").expect("prod.md");
    std::os::unix::fs::symlink("../secrets/prod.md", t.0.join("docs/prod.md")).expect("建連結");
    let (code, out) = awf(&t.0, &["kb", "build"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("docs/prod.md") && out.contains("機敏檔"), "{out}");
    let (_, hit) = awf(&t.0, &["kb", "query", "kbsecretq"]);
    assert!(!hit.contains("prod.md"), "{hit}");
    let (_, ok) = awf(&t.0, &["kb", "query", "okmarker"]);
    assert!(ok.contains("docs/a.md"), "{ok}");
}
