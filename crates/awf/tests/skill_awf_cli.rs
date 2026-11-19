//! `/agent-workflow:awf`（plugin/skills/awf）要涵蓋 `awf --help` 列出的每一個子指令，而且每個子指令的 `--help` 只印說明、不執行。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const EXE: &str = env!("CARGO_BIN_EXE_awf");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn subcommands() -> Vec<String> {
    let o = Command::new(EXE).arg("--help").stdin(Stdio::null()).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let subs: Vec<String> = text.lines().filter(|l| l.starts_with("  ") && !l.starts_with("   ")).filter_map(|l| l.split_whitespace().next()).filter(|w| !w.starts_with('-'))
        .map(|w| w.trim_end_matches('、').to_string()).filter(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '-')).collect();
    assert!(subs.len() >= 35, "從 awf --help 只認出 {} 個子指令：{subs:?}", subs.len());
    subs
}

#[test]
fn skill_covers_every_subcommand() {
    let dir = root().join("plugin/skills/awf");
    let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
    let refs = std::fs::read_to_string(dir.join("reference/指令.md")).unwrap();
    let missing: Vec<String> = subcommands().into_iter().filter(|s| !skill.contains(&format!("awf {s}"))).collect();
    assert!(missing.is_empty(), "plugin/skills/awf/SKILL.md 的對照表少了這些子指令：{missing:?}");
    let missing: Vec<String> = subcommands().into_iter().filter(|s| s != "hook" && s != "state" && !refs.contains(&format!("awf {s}"))).collect();
    assert!(missing.is_empty(), "reference/指令.md 少了這些子指令的寫法：{missing:?}");
    assert!(skill.lines().count() <= 80, "SKILL.md 要精簡（細節放 reference/）");
}

#[test]
fn help_never_executes() {
    // 在空的暫存目錄跑：--help 不能建檔、不能因為「不是專案」而失敗到沒有說明
    let tmp = std::env::temp_dir().join(format!("awf-help-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for sub in subcommands() {
        let o = Command::new(EXE).args([sub.as_str(), "--help"]).current_dir(&tmp).env_remove("CLAUDE_PROJECT_DIR").stdin(Stdio::null()).output().unwrap();
        let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(out.contains("awf ") && out.lines().count() >= 3, "awf {sub} --help 沒有印說明：\n{out}");
        assert!(std::fs::read_dir(&tmp).unwrap().next().is_none(), "awf {sub} --help 不能寫檔");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
