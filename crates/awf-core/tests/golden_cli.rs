//! `awf golden`：repo 自己的 golden-set 要全過；另用暫存目錄造「正例其實不合格」「反例其實合格」「JSON 壞掉」
//! 「目錄不存在」各一個案例。

use awf_core::golden;
use std::path::{Path, PathBuf};

/// repo 的 `.claude/teams`（scaffold 可能在 repo 根目錄，也可能在 plugin/ 底下）。
fn repo_teams() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for rel in ["scaffold/.claude/teams", "plugin/scaffold/.claude/teams"] {
        let p = root.join(rel);
        if p.join("golden-set").is_dir() {
            return p;
        }
    }
    panic!("找不到 repo 的 golden-set（scaffold/ 與 plugin/scaffold/ 都沒有）");
}

fn count_json(dir: &Path) -> usize {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.path().is_file() && e.path().extension().is_some_and(|x| x == "json")).count()
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read_dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() { copy_tree(&src, &dst) } else { std::fs::copy(&src, &dst).map(|_| ()).expect("copy") }
    }
}

#[test]
fn repo_golden_set_passes() {
    let teams = repo_teams();
    let here = teams.join("golden-set");
    let results = golden::run_all(&here);
    let failed: Vec<String> = results.iter().filter(|r| !r.problems.is_empty()).map(|r| format!("{}/{}：{}", r.kind, r.name, r.problems.join("；"))).collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    // 範例數＝目錄裡的檔數（valid＋內建 profile＋專案 profile；invalid），一個都沒漏
    let own = usize::from(teams.join("methodology.json").is_file());
    let valid = count_json(&here.join("valid")) + count_json(&teams.join("methodologies")) + own;
    let invalid = count_json(&here.join("invalid"));
    assert!(valid > 10 && invalid > 10, "範例太少：正例 {valid}、反例 {invalid}");
    assert_eq!(results.iter().filter(|r| r.kind == "valid").count(), valid);
    assert_eq!(results.iter().filter(|r| r.kind == "invalid").count(), invalid);
    let semantic = results.iter().filter(|r| r.semantic).count();
    assert!(semantic > 0, "golden-set 應該有語意反例");
    // 反例都真的被擋下來；不是語意反例的，光靠 schema 就擋得住
    assert!(results.iter().filter(|r| r.kind == "invalid").all(|r| r.errors > 0 && (r.semantic || r.schema_errors > 0)));

    let dir = here.to_string_lossy().into_owned();
    let (code, out, err) = golden::run_captured(&args(&["--dir", &dir]), Path::new("/nonexistent"), Path::new("/"));
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    assert_eq!(out, format!("golden set：通過 {}，失敗 0（正例 {valid}／{valid}、反例 {invalid}／{invalid}，其中語意反例 {semantic}／{semantic}）\n", valid + invalid));
    // 預設目錄（不給 --dir）與 --verbose
    let (code, out, _) = golden::run_captured(&args(&["--verbose"]), &here, Path::new("/"));
    assert_eq!(code, 0);
    assert_eq!(out.lines().filter(|l| l.starts_with("ok   ")).count(), valid + invalid, "{out}");
}

#[test]
fn golden_reports_bad_examples() {
    let teams = repo_teams();
    let base = std::env::temp_dir().join(format!("awf-golden-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let t = base.join("teams");
    let put = |rel: &str, text: &str| {
        let p = t.join(rel);
        std::fs::create_dir_all(p.parent().expect("上層目錄")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    };
    // schema 用 repo 的；範例從 repo 各拿一個合格的正例與一個（不是語意的）反例當材料
    copy_tree(&teams.join("schemas"), &t.join("schemas"));
    let good = std::fs::read_to_string(teams.join("golden-set/valid/env-needs-decision.json")).expect("正例");
    let wrong = std::fs::read_to_string(teams.join("golden-set/invalid/env-confidence.json")).expect("反例");
    let dir = t.join("golden-set").to_string_lossy().into_owned();
    let run = || golden::run_captured(&args(&["--dir", &dir]), Path::new("/nonexistent"), Path::new("/"));

    // 目錄不存在
    let (code, out, err) = run();
    assert_eq!((code, out.as_str()), (1, ""), "{err}");
    assert!(err.contains("找不到 golden-set 目錄") && err.contains("--dir"), "{err}");
    // 目錄在、沒有範例
    std::fs::create_dir_all(t.join("golden-set/valid")).expect("mkdir");
    let (code, out, _) = run();
    assert!(code == 1 && out.contains("沒有任何範例"), "{out}");
    // 都對：通過
    put("golden-set/valid/a-good.json", &good);
    put("golden-set/invalid/a-wrong.json", &wrong);
    let (code, out, _) = run();
    assert_eq!((code, out.as_str()), (0, "golden set：通過 2，失敗 0（正例 1／1、反例 1／1，其中語意反例 0／0）\n"));
    // 正例其實不合格、反例其實合格、JSON 壞掉
    put("golden-set/valid/b-actually-wrong.json", &wrong);
    put("golden-set/invalid/b-actually-good.json", &good);
    put("golden-set/invalid/c-broken.json", "{ 這不是 JSON");
    put("golden-set/valid/readme.txt", "不是 .json 的檔不看");
    let (code, out, err) = run();
    assert_eq!((code, err.as_str()), (1, ""), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert!(lines[0].starts_with("FAIL valid/b-actually-wrong.json：") && lines[0].contains("驗證器判 不通過"), "{out}");
    assert!(lines[1].starts_with("FAIL invalid/b-actually-good.json：") && lines[1].contains("驗證器判 通過") && lines[1].contains("schema 應該擋但通過"), "{out}");
    assert!(lines[2].starts_with("FAIL invalid/c-broken.json：不是合法 JSON"), "{out}");
    assert_eq!(lines[3], "golden set：通過 2，失敗 3（正例 1／2、反例 1／3，其中語意反例 0／0）");
    // 參數錯誤
    assert_eq!(golden::run_captured(&args(&["--dir"]), Path::new("/nonexistent"), Path::new("/")).0, 2);
    assert_eq!(golden::run_captured(&args(&["--frob"]), Path::new("/nonexistent"), Path::new("/")).0, 2);
    let _ = std::fs::remove_dir_all(&base);
}
