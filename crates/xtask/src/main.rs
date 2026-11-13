//! 只給這個 repo 用的維護工具（`cargo xtask <指令>`）：對跑、去客戶化掃描、文件核對、派工預算、內嵌片段。
//! 不放進 plugin。

use std::process::ExitCode;

const USAGE: &str = "用法：cargo xtask <指令>

  scrub [路徑…]   去客戶化掃描（檔案內容與路徑名；不給路徑就掃整個 repo）
  check-docs      文件裡的角色、workflow、command 數量與名單要和實際檔案一致
  check-recipes   recipe 解析：awf_core::recipe 與 snippets/recipe.js 對 golden-set/recipes 的結果要相同
  check-kernel    執行核心片段（snippets/kernel.js）的單元檢查
  check-patterns  pattern 庫（snippets/patterns.js）與自訂腳本端到端
  check-budget [--update] [--budget <檔>]
                  派工數與派工單字數對 golden-set/token-budget.json（超過 10% 失敗；--update 寫回基準）
  check-supply    供應鏈：只用標準庫、plugin 不連網、action 釘 SHA、版控沒有快取與私鑰

check-recipes 以下這五個指令可以加 --root <目錄> 指定 repo 根目錄（預設是這個 repo；測試用）。
其他指令在移植中，完成的會列在這裡。";

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
        Some(name @ ("check-recipes" | "check-kernel" | "check-patterns" | "check-budget" | "check-supply")) => {
            let (root, rest) = match xtask::util::take_opt(rest, "--root") {
                Ok((root, rest)) => (root.map(std::path::PathBuf::from).unwrap_or_else(xtask::repo_root), rest),
                Err(e) => {
                    eprintln!("xtask：{e}");
                    return ExitCode::from(2);
                }
            };
            finish(match name {
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
