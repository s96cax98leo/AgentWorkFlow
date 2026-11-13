//! 只給這個 repo 用的維護工具（`cargo xtask <指令>`）：對跑、去客戶化掃描、文件核對、派工預算、內嵌片段。
//! 不放進 plugin。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            println!("用法：cargo xtask <指令>\n\n指令在移植中，完成的會列在這裡。");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask：不認得的指令「{other}」");
            ExitCode::from(1)
        }
    }
}
