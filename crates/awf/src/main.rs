//! `awf`：AgentWorkFlow 的單一執行檔。子指令沿用原本的工具名（`awf save-run stage`、`awf hook guard-shell`…）。

use std::process::ExitCode;

const SOURCE_HASH: &str = env!("AWF_SOURCE_HASH");

fn usage() -> &'static str {
    "用法：awf <子指令> [參數…]\n\n  --version    版本與原始碼雜湊\n\n已移植的子指令：\n  checklist    可驗證的 checklist（status、check）\n  hook         hook 入口（移植中）\n"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("awf {} (source {})", awf_core::VERSION, SOURCE_HASH);
            ExitCode::SUCCESS
        }
        None | Some("-h") | Some("--help") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("checklist") => ExitCode::from(awf_core::checklist::main(&args[1..]).clamp(0, 255) as u8),
        Some("hook") => ExitCode::from(awf_core::hooks::main(&args[1..]).clamp(0, 255) as u8),
        Some(other) => {
            eprintln!("awf：不認得的子指令「{other}」\n\n{}", usage());
            ExitCode::from(1)
        }
    }
}
