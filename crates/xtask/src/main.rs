//! 只給這個 repo 用的維護工具（`cargo xtask <指令>`）：全部檢查（check-all）、去客戶化掃描、golden、文件核對、派工預算、供應鏈、基準實跑。
//! 不放進 plugin。

use std::process::ExitCode;

const USAGE: &str = "用法：cargo xtask <指令>

  check-all [--verbose] [--allow-skip] [--only <名稱片段>]
                  依序跑下面全部檢查、片段與範本的漂移檢查、各子指令的 selftest、workflow 乾跑，最後印總結。
                  不包含 cargo test --workspace（另外跑）。沒有 node 時相關步驟印「略過」並 exit 1（--allow-skip 才放行）
  scrub [路徑…]   去客戶化掃描（檔案內容與路徑名；不給路徑就掃整個 repo）
  golden [--verbose]
                  golden set：schema 的正例要零錯誤、反例要有錯誤（valid/、invalid/ 與內建 profile）
  check-docs      文件裡的角色、workflow、command 數量與名單要和實際檔案一致
  check-recipes   recipe 解析：awf_core::recipe 與 snippets/recipe.js 對 golden-set/recipes 的結果要相同
  check-kernel    執行核心片段（snippets/kernel.js）的單元檢查
  check-patterns  pattern 庫（snippets/patterns.js）與自訂腳本端到端
  check-budget [--update] [--budget <檔>]
                  派工數與派工單字數對 golden-set/token-budget.json（超過 10% 失敗；--update 寫回基準）
  check-supply    供應鏈：action 釘 SHA、版控沒有快取與私鑰、Cargo.lock 在版控、相依都來自 crates.io 且和允許清單一致、
                  Node 腳本只用內建模組（Python 檔還在的期間另掃：只用標準庫、plugin 不連網）
  check-launchers 啟動器（plugin/bin/awf、awf.cmd）：用假的執行檔核對挑檔、參數、exit code、錯誤訊息
  gen-legacy [--write]
                  從 git 歷史算 plugin/workflow-templates/legacy.json（預設只比對；--write 才寫）
  bench --selftest
                  基準實跑不花 token 的部分（虛構專案、setup、備資料包、彙整）
  bench --work <空目錄> [--model opus] [--step setup|plan|rerun|report] [--timeout 分鐘]
                  基準實跑：會真的呼叫 claude，一次花數百萬 token，只在發佈前手動跑
  bench fixture <目錄> [--no-git] ｜ bench manifest [--write]
                  只產生虛構專案 ｜ 虛構專案和記下來的 sha1 清單比
  dist [--target <triple>] [--out <目錄>]
                  建置發佈用的執行檔並複製成 plugin/bin 用的檔名（預設本機平台、輸出到 target/dist）
  checksums [--dir <目錄>] [--write|--check] [--require-all]
                  執行檔的 sha256 與原始碼雜湊（checksums.json；預設目錄 plugin/bin）

golden、check-recipes、check-kernel、check-patterns、check-budget、check-supply、gen-legacy 可以加 --root <目錄>
指定 repo 根目錄（預設是這個 repo；測試用）。";

fn finish((code, out, err): (i32, String, String)) -> ExitCode {
    print!("{out}");
    eprint!("{err}");
    ExitCode::from(code.clamp(0, 255) as u8)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("scrub") => finish(xtask::scrub::run_captured(&xtask::repo_root(), rest)),
        Some("check-docs") => finish(xtask::check_docs::run_captured(&xtask::repo_root())),
        Some("check-launchers") => finish(xtask::launchers::run_captured(&xtask::repo_root())),
        Some("dist") => finish(xtask::dist::run_captured(&xtask::repo_root(), rest)),
        Some("checksums") => finish(xtask::checksums::run_captured(&xtask::repo_root(), rest)),
        // 隱藏指令：check-launchers 在 Windows 上拿 xtask 自己當假的執行檔。
        Some(xtask::launchers::ECHO_COMMAND) => ExitCode::from(xtask::launchers::echo_main(rest).clamp(0, 255) as u8),
        Some("check-all") => ExitCode::from(xtask::check_all::run(&xtask::repo_root(), rest).clamp(0, 255) as u8),
        Some("bench") => finish(xtask::bench::run_captured(&xtask::repo_root(), rest)),
        Some(name @ ("golden" | "check-recipes" | "check-kernel" | "check-patterns" | "check-budget" | "check-supply" | "gen-legacy")) => {
            let (root, rest) = match xtask::util::take_opt(rest, "--root") {
                Ok((root, rest)) => (root.map(std::path::PathBuf::from).unwrap_or_else(xtask::repo_root), rest),
                Err(e) => {
                    eprintln!("xtask：{e}");
                    return ExitCode::from(2);
                }
            };
            finish(match name {
                "golden" => xtask::golden::run_captured(&root, &rest),
                "gen-legacy" => xtask::gen_legacy::run_captured(&root, &rest),
                "check-recipes" => xtask::check_recipes::run_captured(&root),
                "check-kernel" => xtask::check_kernel::run_captured(&root),
                "check-patterns" => xtask::check_patterns::run_captured(&root),
                "check-budget" => xtask::check_budget::run_captured(&root, &rest),
                _ => xtask::check_supply::run_captured(&root),
            })
        }
        Some(other) => {
            eprintln!("xtask：不認得的指令「{other}」");
            ExitCode::from(1)
        }
    }
}
