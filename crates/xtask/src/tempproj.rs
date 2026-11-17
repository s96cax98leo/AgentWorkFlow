//! 在暫存目錄 init 一個專案、產生全部 workflow 腳本：check-budget 與 check-all 的乾跑共用。
//! 全部在行程內呼叫 awf-core（`awf init`、`awf render` 的本體），不起任何外部直譯器。

use crate::util::{self, TempDir};
use awf_core::merge_gate::pyutil;
use std::path::{Path, PathBuf};
use std::process::Command;

/// init 過的暫存專案與產生好的腳本目錄。
pub struct Rendered {
    pub tmp: TempDir,
    /// 專案根目錄（`<tmp>/proj`）。
    pub proj: PathBuf,
    /// 12 支腳本所在的目錄（`<tmp>/wf`）。
    pub wf_dir: PathBuf,
}

pub fn posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 用預設值 init 一個暫存專案（等同 `awf init --target <proj> --defaults`，模版取自 `root`）。
pub fn init_default(root: &Path, prefix: &str) -> Result<(TempDir, PathBuf), String> {
    let tmp = TempDir::new(prefix)?;
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).map_err(|e| format!("建不了暫存專案：{e}"))?;
    let _ = util::run(Command::new("git").args(["init", "-q"]).current_dir(&proj), None);
    let env = awf_core::init::Env { source: Some(root.to_path_buf()), cwd: tmp.path().to_path_buf(), ..Default::default() };
    let args: Vec<String> = vec!["--target".into(), posix(&proj), "--defaults".into()];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = awf_core::init::run(&args, &env, &mut out, &mut err, &mut std::io::empty(), false);
    if code != 0 {
        let both = format!("{}{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
        return Err(format!("init 失敗：{}", pyutil::tail(&both, 1500)));
    }
    Ok((tmp, proj))
}

/// 狀態機用的環境：專案在 `proj`、模版在 `root`，不讀使用者的 Claude Code 設定（plugin 一律明確指定）。
pub fn state_env(root: &Path, proj: &Path) -> awf_core::state::Env {
    awf_core::state::Env {
        project: posix(proj),
        cwd: posix(proj),
        home: posix(&proj.join(".no-claude-home")),
        template: Some(posix(&util::plugin_dir(root))),
        ..Default::default()
    }
}

/// init 之後產生全部腳本（等同在專案裡 `awf render --all --out <wf> --plugin-dir <root>/plugin`）。
pub fn rendered(root: &Path, prefix: &str) -> Result<Rendered, String> {
    rendered_with(root, prefix, None)
}

/// 同 [`rendered`]；`patch` 有給時先把它的鍵蓋進專案的 methodology.json（最外層逐鍵取代），再產生腳本。
pub fn rendered_with(root: &Path, prefix: &str, patch: Option<&serde_json::Value>) -> Result<Rendered, String> {
    let (tmp, proj) = init_default(root, prefix)?;
    if let Some(serde_json::Value::Object(p)) = patch {
        let f = proj.join(".claude").join("teams").join("methodology.json");
        let mut m = util::read_json(&f)?;
        let o = m.as_object_mut().ok_or("methodology.json 不是 JSON 物件")?;
        for (k, v) in p {
            o.insert(k.clone(), v.clone());
        }
        std::fs::write(&f, format!("{}\n", serde_json::to_string_pretty(&m).unwrap_or_default())).map_err(|e| format!("寫不了 methodology.json：{e}"))?;
    }
    let wf_dir = tmp.path().join("wf");
    let argv: Vec<String> = vec!["render".into(), "--all".into(), "--out".into(), posix(&wf_dir), "--plugin-dir".into(), posix(&root.join("plugin"))];
    let (code, out, err) = awf_core::state::run_captured(&argv, &state_env(root, &proj));
    if code != 0 {
        return Err(format!("render 失敗：{}", pyutil::tail(&format!("{out}{err}"), 1500)));
    }
    Ok(Rendered { tmp, proj, wf_dir })
}

/// 乾跑的指令：`node <模版>/workflow-templates/check_workflows.mjs --project <proj> --agents-dir <模版>/agents --wf-dir <wf_dir>`。
pub fn check_workflows(root: &Path, proj: &Path, wf_dir: &Path) -> Command {
    let plugin = util::plugin_dir(root);
    let mut cmd = Command::new("node");
    cmd.arg(awf_core::template::check_workflows(&plugin)).arg("--project").arg(proj).arg("--agents-dir").arg(plugin.join("agents")).arg("--wf-dir").arg(wf_dir).current_dir(proj);
    cmd
}
