//! `awf`：AgentWorkFlow 的單一執行檔。子指令沿用原本的工具名（`awf save-run stage`、`awf hook guard-shell`…）。

use std::process::ExitCode;

const SOURCE_HASH: &str = env!("AWF_SOURCE_HASH");

fn usage() -> &'static str {
    "用法：awf <子指令> [參數…]

  --version        版本與原始碼雜湊
  --help、help     這份說明

流程（主 session 用）：
  start            開一個新的 run，回第一個動作
  next             回下一個動作（跑 workflow、閘門、要人補資料、結束）
  done             收下這一段 workflow 的回傳並存檔
  approve          核准一段閘門（轉給 save-run approve，時間自動取）
  render           用範本與專案設定產生 workflow 腳本
  handoff          寫這個 run 的接手摘要
  state            狀態機本身的選項（例如 state --selftest）
  save-run         存各段結果、核准、升級、結案與放棄
  pack             備每一段的資料包與調研重用紀錄
  watch            看 run 的進度，把變化記成進度事件並通知
  statusline       印一行給狀態列用的進度

閘門與推送：
  merge-gate       確定性的合併閘門：複核需求對照、head、checks、風險後判 merge 或 hold
  publish          確定性的推送：掃機敏值、確認閘門鏈都核准後推分支並開 PR
  sync-branch      決定分支怎麼跟上 base（rebase、merge 或中轉分支）並檢查中轉分支

查詢與輔助：
  kb               知識層：建索引、查詢專案文件與結案摘要
  kg               知識圖：規格、文件、連結帳本與程式索引接成的圖（locate、trace、impact、check）
  program          多需求排程：讀 program.json，列每個 epic 的狀態、被誰擋住、現在可以開工的
  locate           用需求的用詞在 repo 裡定位相關的檔案行段
  recipe           依需求類型解析這次要跑哪些段、用什麼模型
  deliverables     交付文件的設定、盤點、同步與匯出
  cost             統計 workflow 花的 token 與時間
  spec             規格層：讀規格目錄（現行規格與變更），做 validate 與 apply
  run-brief        跑一個指令，完整輸出寫進 log 檔，終端只印重點
  checklist        可驗證的 checklist：照驗證指令的實際結果打勾

檢查：
  doctor           檢查安裝：環境、版本、hook 註冊、專案設定
  build            把片段、schema 與專案設定內嵌進 workflow（--check 只比對）
  script           自訂 workflow 腳本的靜態檢查與組裝
  check-agents     檢查 agent 檔的格式與角色名單
  check-envelope   驗一份交件信封（schema 與交接的語意規則）
  check-rules      掃描產出有沒有違反 rules.md 的硬規則（只做機械判定）
  golden           跑專案的 golden-set：交件信封與各種 payload 的正例、反例、語意反例
  tokens           設計系統的 token 檢查：掃寫死的色碼（check）、tokens.css 與正本一字不差（verify）
  scrub            去客戶化掃描：用專案自己訂的規則掃檔案內容與路徑名（預設不啟用，在 profile 的 scrub 設定）
  ci               專案的測試／lint CI：依 profile 的 ci 產生 GitHub Actions 的 workflow 檔（預設不啟用；render 只印，--write 才寫）

安裝：
  init             把模版套用到專案，或升級已套用的專案

hook：
  hook             hook 入口（guard-shell、guard-paths、envelope、session-start）

大部分子指令可以加 --selftest 跑自己的檢查。
"
}

/// 自己沒有處理 `--help` 的子指令：`awf <子指令> --help` 只印說明、不執行（原本 `build --help` 會真的去內嵌、
/// `doctor --help` 會真的跑檢查、`check-envelope --help` 會把 `--help` 當檔名）。回 `None`＝這個子指令自己會處理。
fn sub_help(sub: &str) -> Option<ExitCode> {
    let text = match sub {
        "start" | "next" | "done" | "approve" | "render" | "handoff" => {
            let _ = awf_core::state::main(&["state".to_string(), "--help".to_string()]);
            return Some(ExitCode::SUCCESS);
        }
        "statusline" => "印一行給狀態列用的進度（唯讀）。\n\n用法：\n    awf statusline\n\n整合方式見 docs/監控與整合.md；看完整進度用 awf watch --once。\n",
        "build" => "把片段、schema 與專案的 methodology.json 內嵌進 workflow 腳本。\n\n用法：\n    awf build            # 專案裡只處理覆寫檔（.claude/workflows/*.js）；模版 repo 裡處理 plugin/workflow-templates/\n    awf build --check    # 只比對、不寫：內嵌內容和片段有漂移時 exit 1\n\n改了 schema、profile 或片段之後重跑。擴充 agent 沒登記守門角色時拒絕內嵌（exit 1）。\n",
        "doctor" => "安裝檢查（只讀、不連網）：環境、plugin 與專案版本、hook 註冊、專案設定與體檢提示。有 error 才 exit 1。\n\n用法：\n    awf doctor [--json] [--plugin-dir <plugin 目錄>]\n    awf doctor --selftest\n\n每一項都有修法（fix）；照它說的做。\n",
        "check-agents" => "檢查 tpm-*／dev-* agent 檔的格式與角色名單（只讀）。\n\n用法：\n    awf check-agents --plugin-dir <AgentWorkFlow>/plugin\n    awf check-agents --agents-dir <agents 目錄>\n",
        "check-envelope" => awf_core::check_envelope::USAGE,
        "check-rules" => awf_core::check_rules::USAGE,
        "hook" => "hook 入口：由 Claude Code 的 hook 呼叫，從 stdin 讀 hook 的輸入（JSON）。平常不用手動跑。\n\n用法：\n    awf hook <guard-shell|guard-paths|envelope|session-start|mark-dirty|prompt-context> < hook-input.json\n\n註冊方式與失敗語意見 docs/自訂與擴充.md 與 plugin/hooks/hooks.json。\n",
        _ => return None,
    };
    print!("{text}");
    Some(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [sub, flag] = args.as_slice() {
        if flag == "--help" || flag == "-h" {
            if let Some(code) = sub_help(sub) {
                return code;
            }
        }
    }
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("awf {} (source {})", awf_core::VERSION, SOURCE_HASH);
            ExitCode::SUCCESS
        }
        None | Some("-h") | Some("--help") | Some("help") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("checklist") => ExitCode::from(awf_core::checklist::main(&args[1..]).clamp(0, 255) as u8),
        Some("kb") => ExitCode::from(awf_core::kb::main(&args[1..]).clamp(0, 255) as u8),
        Some("kg") => ExitCode::from(awf_core::kg::main(&args[1..]).clamp(0, 255) as u8),
        Some("program") => ExitCode::from(awf_core::program::main(&args[1..]).clamp(0, 255) as u8),
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
        Some("golden") => ExitCode::from(awf_core::golden::main(&args[1..]).clamp(0, 255) as u8),
        Some("tokens") => ExitCode::from(awf_core::tokens::main(&args[1..]).clamp(0, 255) as u8),
        Some("ci") => ExitCode::from(awf_core::ci::main(&args[1..]).clamp(0, 255) as u8),
        Some("scrub") => ExitCode::from(awf_core::scrub::main(&args[1..]).clamp(0, 255) as u8),
        Some("hook") => ExitCode::from(awf_core::hooks::main(&args[1..]).clamp(0, 255) as u8),
        Some(other) => {
            eprintln!("awf：不認得的子指令「{other}」\n\n{}", usage());
            ExitCode::from(1)
        }
    }
}
