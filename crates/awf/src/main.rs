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
        Some("kb") => ExitCode::from(awf_core::kb::main(&args[1..]).clamp(0, 255) as u8),
        Some("recipe") => ExitCode::from(awf_core::recipe::main(&args[1..]).clamp(0, 255) as u8),
        Some("merge-gate") => ExitCode::from(awf_core::merge_gate::main(&args[1..]).clamp(0, 255) as u8),
        Some("sync-branch") => ExitCode::from(awf_core::sync_branch::main(&args[1..]).clamp(0, 255) as u8),
        Some("publish") => ExitCode::from(awf_core::publish::main(&args[1..]).clamp(0, 255) as u8),
        Some("save-run") => ExitCode::from(awf_core::save_run::main(&args[1..]).clamp(0, 255) as u8),
        Some("deliverables") => ExitCode::from(awf_core::deliverables::main(&args[1..]).clamp(0, 255) as u8),
        Some("locate") => ExitCode::from(awf_core::locate::main(&args[1..]).clamp(0, 255) as u8),
        Some("pack") => ExitCode::from(awf_core::pack::main(&args[1..]).clamp(0, 255) as u8),
        Some("cost") => ExitCode::from(awf_core::cost::main(&args[1..]).clamp(0, 255) as u8),
        Some("script") => ExitCode::from(awf_core::script::main(&args[1..]).clamp(0, 255) as u8),
        Some("build") => ExitCode::from(awf_core::build::main(&args[1..]).clamp(0, 255) as u8),
        Some("start" | "next" | "done" | "approve" | "render" | "handoff" | "state") => ExitCode::from(awf_core::state::main(&args).clamp(0, 255) as u8),
        Some("watch" | "statusline") => ExitCode::from(awf_core::watch::main(&args).clamp(0, 255) as u8),
        Some("doctor") => ExitCode::from(awf_core::doctor::main(&args[1..]).clamp(0, 255) as u8),
        Some("spec") => ExitCode::from(awf_core::spec::main(&args[1..]).clamp(0, 255) as u8),
        Some("init") => ExitCode::from(awf_core::init::main(&args[1..]).clamp(0, 255) as u8),
        Some("check-agents") => ExitCode::from(awf_core::check_agents::main(&args[1..]).clamp(0, 255) as u8),
        Some("run-brief") => ExitCode::from(awf_core::run_brief::main(&args[1..]).clamp(0, 255) as u8),
        Some("check-envelope") => ExitCode::from(awf_core::check_envelope::main(&args[1..]).clamp(0, 255) as u8),
        Some("check-rules") => ExitCode::from(awf_core::check_rules::main(&args[1..]).clamp(0, 255) as u8),
        Some("hook") => ExitCode::from(awf_core::hooks::main(&args[1..]).clamp(0, 255) as u8),
        Some(other) => {
            eprintln!("awf：不認得的子指令「{other}」\n\n{}", usage());
            ExitCode::from(1)
        }
    }
}
