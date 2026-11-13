//! 只給這個 repo 用的維護工具（`cargo xtask <指令>`）：對跑、去客戶化掃描、文件核對、派工預算、內嵌片段。
//! 不放進 plugin。

use std::process::ExitCode;

const USAGE: &str = "用法：cargo xtask <指令>

  scrub [路徑…]   去客戶化掃描（檔案內容與路徑名；不給路徑就掃整個 repo）
  check-docs      文件裡的角色、workflow、command 數量與名單要和實際檔案一致

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
        Some(other) => {
            eprintln!("xtask：不認得的指令「{other}」");
            ExitCode::from(1)
        }
    }
}
