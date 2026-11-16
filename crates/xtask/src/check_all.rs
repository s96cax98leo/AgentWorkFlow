//! `cargo xtask check-all`：依序跑 repo 的全部維護檢查，每一步印「通過／失敗／略過 <名稱>」，最後印總結。
//! 要 cargo、git，node 見下。
//!
//! 全部檢查＝`cargo test --workspace`＋`cargo xtask check-all`，兩個都過才算通過：這支不包含 `cargo test`
//! （單元測試、回歸基準、hook 案例、init 的煙霧測試都在那邊）。
//!
//! 步驟（順序固定）：
//!   1. scrub、golden、check-docs、check-recipes、check-kernel、check-patterns、check-budget、check-supply、
//!      check-launchers、gen-legacy（只比對）——和各自的 `cargo xtask <指令>` 相同，在行程內呼叫。
//!   2. build --check：片段與範本沒有漂移（`awf build --check` 對模版 repo 做的事）。
//!   3. 建置 `awf`（`cargo build -p awf`），之後幾步用建出來的執行檔。
//!   4. check-agents：`awf check-agents --plugin-dir plugin`（在 `plugin/scaffold/` 下跑，schema 取自 scaffold）。
//!   5. 各子指令的 selftest：`awf <子指令> --selftest`（在 `plugin/scaffold/` 下跑），清單見 [`SELFTESTS`]。
//!   6. bench --selftest：基準實跑不花 token 的部分。
//!   7. workflow 乾跑：暫存專案 init、產生 12 支腳本，`node check_workflows.mjs` 把每支與全部情境走一遍。
//!
//! 沒有 node 時：需要 node 的步驟（check-recipes、check-kernel、check-patterns、check-budget、workflow 乾跑）印「略過」、
//! 不執行，總結列出；略過不算通過，所以整體 exit 1。確定要在沒有 node 的機器上只看其餘步驟時加 `--allow-skip`
//! （略過照樣列出，只是不影響 exit code）。單獨跑那幾支檢查時行為不變：沒有 node 判失敗。
//!
//! 用法：`cargo xtask check-all [--verbose] [--allow-skip] [--only <名稱片段>]`
//!   --verbose      每一步都印完整輸出（預設只印最後一行；失敗的一律印完整輸出）
//!   --only <片段>  只跑名稱含這段文字的步驟（除錯用；總結會註明不是全部）

use crate::util::{self, Captured};
use crate::{awfbin, bench, check_budget, check_docs, check_kernel, check_patterns, check_recipes, check_supply, gen_legacy, golden, launchers, scrub, tempproj};
use awf_core::merge_gate::pyutil;
use std::path::Path;
use std::time::Instant;

/// 有 `--selftest` 的子指令（實跑確認過；`statusline --selftest` 和 `watch` 是同一支，不重複跑）。
/// 沒有 selftest 的：init、build、check-agents、check-envelope、check-rules、hook（由 `cargo test` 的對跑基準顧）。
pub const SELFTESTS: [&str; 19] = [
    "checklist", "kb", "recipe", "merge-gate", "sync-branch", "publish", "save-run", "deliverables", "locate", "pack", "cost", "script", "state", "watch",
    "doctor", "spec", "run-brief", "tokens",
    "scrub",
];

enum Outcome {
    Pass(String),
    Fail(String),
    /// 沒執行（缺工具）：理由。
    Skip(String),
}

fn from_captured((code, out, err): Captured) -> Outcome {
    let text = format!("{out}{err}");
    if code == 0 {
        Outcome::Pass(text)
    } else {
        Outcome::Fail(format!("{text}（exit {code}）\n"))
    }
}

fn build_awf(root: &Path) -> Outcome {
    match awfbin::build(root) {
        Ok(exe) => Outcome::Pass(format!("{}\n", exe.display())),
        Err(e) => Outcome::Fail(format!("{e}\n")),
    }
}

/// 在 `plugin/scaffold/` 下跑 `awf <args…>`。
fn awf(root: &Path, args: &[&str]) -> Outcome {
    if !awfbin::exe().is_file() {
        return Outcome::Fail(format!("沒有 awf 執行檔（{}；「建置 awf」那一步沒過）\n", awfbin::exe().display()));
    }
    let mut cmd = awfbin::command(&awf_core::template::scaffold(&util::plugin_dir(root)));
    cmd.args(args);
    match util::run(&mut cmd, None) {
        Ok(r) => from_captured((r.code, r.stdout, r.stderr)),
        Err(e) => Outcome::Fail(format!("{e}\n")),
    }
}

fn selftest(root: &Path, sub: &str) -> Outcome {
    awf(root, &[sub, "--selftest"])
}

fn dry_run(root: &Path) -> Outcome {
    let r = match tempproj::rendered(root, "awf-dryrun.") {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(format!("{e}\n")),
    };
    let mut cmd = tempproj::check_workflows(root, &r.proj, &r.wf_dir);
    match util::run(&mut cmd, None) {
        Ok(p) => from_captured((p.code, p.stdout, p.stderr)),
        Err(e) => Outcome::Fail(format!("{e}\n")),
    }
}

struct Step<'a> {
    name: String,
    needs_node: bool,
    run: Box<dyn FnOnce() -> Outcome + 'a>,
}

fn add<'a>(v: &mut Vec<Step<'a>>, name: &str, needs_node: bool, run: Box<dyn FnOnce() -> Outcome + 'a>) {
    v.push(Step { name: name.to_string(), needs_node, run });
}

fn steps<'a>(root: &'a Path) -> Vec<Step<'a>> {
    let mut v: Vec<Step<'a>> = Vec::new();
    add(&mut v, "scrub", false, Box::new(move || from_captured(scrub::run_captured(root, &[]))));
    add(&mut v, "golden", false, Box::new(move || from_captured(golden::run_captured(root, &[]))));
    add(&mut v, "check-docs", false, Box::new(move || from_captured(check_docs::run_captured(root))));
    add(&mut v, "check-recipes", true, Box::new(move || from_captured(check_recipes::run_captured(root))));
    add(&mut v, "check-kernel", true, Box::new(move || from_captured(check_kernel::run_captured(root))));
    add(&mut v, "check-patterns", true, Box::new(move || from_captured(check_patterns::run_captured(root))));
    add(&mut v, "check-budget", true, Box::new(move || from_captured(check_budget::run_captured(root, &[]))));
    add(&mut v, "check-supply", false, Box::new(move || from_captured(check_supply::run_captured(root))));
    add(&mut v, "check-launchers", false, Box::new(move || from_captured(launchers::run_captured(root))));
    add(&mut v, "gen-legacy（只比對）", false, Box::new(move || from_captured(gen_legacy::run_captured(root, &[]))));
    add(&mut v, "build --check（片段與範本沒有漂移）", false, Box::new(move || {
        from_captured(awf_core::build::run_captured(&["--check".to_string()], &awf_core::build::Paths::template(&util::plugin_dir(root))))
    }));
    add(&mut v, "建置 awf", false, Box::new(move || build_awf(root)));
    add(&mut v, "check-agents", false, Box::new(move || awf(root, &["check-agents", "--plugin-dir", ".."])));
    for sub in SELFTESTS {
        add(&mut v, &format!("selftest：{sub}"), false, Box::new(move || selftest(root, sub)));
    }
    add(&mut v, "bench --selftest", false, Box::new(move || from_captured(bench::run_captured(root, &["--selftest".to_string()]))));
    add(&mut v, "workflow 乾跑（check_workflows.mjs）", true, Box::new(move || dry_run(root)));
    v
}

fn last_line(text: &str) -> &str {
    text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default()
}

/// 指令本體：`root` 是 repo 根目錄。輸出邊跑邊印（一步可能要幾十秒），所以不走 `Captured`；回 exit code。
pub fn run(root: &Path, args: &[String]) -> i32 {
    let (only, rest) = match util::take_opt(args, "--only") {
        Ok(x) => x,
        Err(e) => {
            eprintln!("check-all：{e}");
            return 2;
        }
    };
    let verbose = rest.iter().any(|a| a == "--verbose" || a == "-v");
    let allow_skip = rest.iter().any(|a| a == "--allow-skip");
    if let Some(bad) = rest.iter().find(|a| !["--verbose", "-v", "--allow-skip"].contains(&a.as_str())) {
        eprintln!("check-all：不認得的參數「{bad}」");
        return 2;
    }
    let has_node = util::which("node").is_some();
    let all = steps(root);
    let total = all.len();
    let (mut passed, mut failed, mut skipped): (Vec<String>, Vec<String>, Vec<String>) = Default::default();
    let mut ran = 0;
    let start = Instant::now();
    for st in all {
        if only.as_deref().is_some_and(|o| !st.name.contains(o)) {
            continue;
        }
        ran += 1;
        let t = Instant::now();
        let outcome = if st.needs_node && !has_node { Outcome::Skip("沒有 node".to_string()) } else { (st.run)() };
        let secs = t.elapsed().as_secs_f64();
        let name = st.name;
        match outcome {
            Outcome::Pass(text) => {
                if verbose {
                    print!("{}", pyutil::strip(&text));
                    println!();
                }
                println!("通過  {name}（{secs:.1}s）{}", if verbose { String::new() } else { format!("  {}", last_line(&text)) });
                passed.push(name);
            }
            Outcome::Fail(text) => {
                println!("{}", text.trim_end());
                println!("失敗  {name}（{secs:.1}s）");
                failed.push(name);
            }
            Outcome::Skip(why) => {
                println!("略過  {name}（{why}，沒有執行）");
                skipped.push(format!("{name}（{why}）"));
            }
        }
    }
    println!("\n================ 總結 ================");
    println!("通過 {}、失敗 {}、略過 {}（共 {ran} 步，{:.0} 秒）", passed.len(), failed.len(), skipped.len(), start.elapsed().as_secs_f64());
    for (label, list) in [("失敗", &failed), ("略過（沒有執行，不算通過）", &skipped)] {
        if !list.is_empty() {
            println!("{label}：");
            for x in list {
                println!("  - {x}");
            }
        }
    }
    if only.is_some() {
        println!("注意：用了 --only，只跑了 {ran}／{total} 步，不是全部檢查。");
    }
    println!("全部檢查＝cargo test --workspace＋cargo xtask check-all：這裡不包含 cargo test（單元測試、回歸基準、hook 案例、init 煙霧測試），要另外跑，兩個都過才算通過。");
    if ran == 0 {
        println!("沒有任何步驟符合 --only");
        return 2;
    }
    if !failed.is_empty() {
        return 1;
    }
    if !skipped.is_empty() && !allow_skip {
        println!("有步驟略過：裝好缺的工具再跑一次；確定只看其餘步驟時加 --allow-skip。");
        return 1;
    }
    0
}
