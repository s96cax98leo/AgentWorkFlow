//! 基準實跑：在虛構專案上跑 sdlc-plan，量 token、時間、平均 context，以及重跑的比例。
//! 由 `tests/bench/run_plan.py` 移植；虛構專案在 [`crate::bench_fixture`]。
//!
//! **實跑會真的呼叫 claude，一次要花數百萬 token**，所以不在 check-all 裡，只在發佈前手動跑。
//!
//! 步驟（`--step`，可分開跑；預設依序全部）：
//!   setup   產生虛構專案 → init（`--defaults --hooks project`，讓這版的 hook 生效）→ 產生 workflow 腳本 → 建 run 目錄
//!   plan    備資料包（pack）→ `claude -p` 用 Workflow 跑 `_tpm/wf/sdlc-plan.js` → 讀 workflow 的回傳存成 plan 段 → 量成本
//!   rerun   同一個 run 再跑一次 plan（來源沒變的調研角度會沿用）→ 量成本
//!   report  彙整成 `<work>/bench-result.json` 並印出對照表
//!
//! 用法：
//!   cargo xtask bench --selftest      只測不花 token 的部分（虛構專案、setup、備資料包、組 args、彙整）
//!   cargo xtask bench --work <空目錄> [--model opus] [--step setup|plan|rerun|report] [--timeout 分鐘]
//!   cargo xtask bench fixture <目標目錄> [--no-git]     只產生虛構專案
//!   cargo xtask bench manifest [--write]                虛構專案每個檔的 sha1 和記下來的清單比（--write 重寫清單）
//!
//! 主對話的模型（`--model`）決定 strong 等級用什麼（內建的 strong 是「沿用主對話模型」）；和之前的實測比較時用同一個模型。
//! agent 一律用這個 repo 的 plugin（`--plugin-dir`），不是使用者已安裝的版本。claude 以 `-p` 執行並只允許 Workflow、Read、
//! Write、Edit、Bash、Grep、Glob、Skill；工作目錄是 `<work>/ws`（工作區），目標 repo 在它底下的 `repos/acme-orders`。
//!
//! 驗收門檻：首次 plan 不超過 5M token、10 分鐘、平均 context 不超過 40k；重跑不超過首次的 30%。
//!
//! 工具（init、render、save-run、cost）在行程內呼叫 awf-core；備資料包起這個 repo 建出來的 `awf`（它以目前目錄為基準）。
//! 不起任何外部直譯器；只有實跑會起 `claude`。
//! 和 Python 版不同的地方：selftest 第一項原本看 `.claude/hooks/read_budget.py` 在不在（hook 還是 Python 檔的時候），
//! 這裡改看專案的 settings.json 有沒有註冊 hook（hook 換成執行檔之後不會有那支檔）。

use crate::bench_fixture as fixture;
use crate::tempproj::{self, posix};
use crate::util::{self, Captured, TempDir};
use awf_core::merge_gate::pyutil;
use awf_core::{cost, pyfmt, recipe};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 日期固定：workflow 與工具都不取時間，派工單才會逐次相同
const RUN_REL: &str = "_tpm/runs/20260101-sdlc-export-range";
const EPIC: &str = "export-range";
const REPO_REL: &str = "repos/acme-orders";
const WF_REL: &str = "_tpm/wf";
const ALLOWED: [&str; 8] = ["Workflow", "Read", "Write", "Edit", "Bash", "Grep", "Glob", "Skill"];
const LIMIT_TOKENS: f64 = 5_000_000.0;
const LIMIT_MINUTES: f64 = 10.0;
const LIMIT_CONTEXT: f64 = 40_000.0;
const LIMIT_RERUN: f64 = 0.30;

fn limits() -> Value {
    json!({"total_tokens": 5_000_000, "minutes": 10, "avg_context": 40_000, "rerun_ratio": 0.30})
}

fn strs(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn tail(out: &str, err: &str) -> String {
    pyutil::tail(&format!("{out}{err}"), 2000).to_string()
}

/// 在專案裡跑 `awf save-run …`（行程內）。
fn save_run(proj: &Path, argv: &[&str], stdin: &str) -> (i32, String, String) {
    let p = posix(proj);
    awf_core::save_run::run_captured(&strs(argv), stdin, &awf_core::save_run::Ctx::new(&p, &p))
}

/// 工作區（init 在這裡）底下放目標 repo：`<work>/ws/repos/acme-orders`。這是模版預設的配置（guard.config.json 的 repos_dir）。
/// `--hooks project` 要複製的 awf：已經建好的那一支（測試執行檔在 `deps/` 底下，往上一層找），沒有就先建。
fn awf_for_hooks(root: &Path) -> Result<PathBuf, String> {
    let exe = crate::awfbin::exe();
    if exe.is_file() {
        return Ok(exe);
    }
    if let Some(up) = exe.parent().and_then(Path::parent).map(|d| d.join(exe.file_name().unwrap_or_default())).filter(|p| p.is_file()) {
        return Ok(up);
    }
    crate::awfbin::build(root)
}

pub fn setup(root: &Path, work: &Path) -> Result<PathBuf, String> {
    let proj = work.join("ws");
    let repo = proj.join("repos").join("acme-orders");
    std::fs::create_dir_all(&repo).map_err(|e| format!("建不了 {}：{e}", repo.display()))?;
    fixture::write(&repo)?;
    fixture::git_init(&repo)?;
    // 讀取上限預設不啟用；基準要量「開啟時」的數字（和之前的實測同條件），所以用答案檔開 enforce
    let ans = work.join("bench-answers.json");
    std::fs::write(&ans, "{\"read_limits\": \"enforce\"}").map_err(|e| format!("寫不了 {}：{e}", ans.display()))?;
    // `--hooks project` 會把 awf 執行檔複製進專案：用這個 repo 建出來的那一支（沒建過就先建）
    let env = awf_core::init::Env {
        source: Some(root.to_path_buf()),
        cwd: root.to_path_buf(),
        awf_bin: Some(awf_for_hooks(root)?),
        ..Default::default()
    };
    let args = vec!["--target".to_string(), posix(&proj), "--defaults".into(), "--hooks".into(), "project".into(), "--answers".into(), posix(&ans)];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = awf_core::init::run(&args, &env, &mut out, &mut err, &mut std::io::empty(), false);
    if code != 0 {
        return Err(format!("init 失敗（exit {code}）：{}", tail(&String::from_utf8_lossy(&out), &String::from_utf8_lossy(&err))));
    }
    let req = proj.join("_tpm").join("requirement.md");
    let src = root.join("tests").join("bench").join("requirement.md");
    std::fs::create_dir_all(proj.join("_tpm")).map_err(|e| format!("建不了 _tpm：{e}"))?;
    std::fs::copy(&src, &req).map_err(|e| format!("複製不了 {}：{e}", src.display()))?;
    // workflow 不放在專案：依 plugin 的範本產生一份（乾跑檢查與基準實跑都用它）
    let argv = strs(&["render", "--all", "--out", WF_REL, "--plugin-dir", &posix(&root.join("plugin"))]);
    let (code, out, err) = awf_core::state::run_captured(&argv, &tempproj::state_env(root, &proj));
    if code != 0 {
        return Err(format!("render 失敗（exit {code}）：{}", tail(&out, &err)));
    }
    let (code, out, err) = save_run(&proj, &["init", RUN_REL, "--event", "sdlc", "--target", REPO_REL, "--requirement", "_tpm/requirement.md"], "");
    if code != 0 {
        return Err(format!("save-run init 失敗（exit {code}）：{}", tail(&out, &err)));
    }
    Ok(proj)
}

/// 備資料包並組出 sdlc-plan 的 args。
///
/// `awf pack` 的 `--run` 以行程的目前目錄為基準，而 args 裡的路徑要是相對專案的寫法（派工單才會逐次相同），
/// 所以這一步起 `awf` 執行檔、目前目錄設在專案（呼叫前要先 [`crate::awfbin::build`]）。
pub fn plan_args(proj: &Path) -> Result<Value, String> {
    let r = util::run(crate::awfbin::command(proj).args(["pack", "plan", "--run", RUN_REL]), None)?;
    if r.code != 0 {
        return Err(format!("pack plan 失敗（exit {}）：{}", r.code, tail(&r.stdout, &r.stderr)));
    }
    let extra = util::read_json(&proj.join(RUN_REL).join("plan").join("pack-args.json"))?;
    let mut args = Map::new();
    for (k, v) in [("repo", REPO_REL), ("epic", EPIC), ("requirement", "_tpm/requirement.md"), ("base", "main"), ("run_dir", RUN_REL)] {
        args.insert(k.to_string(), json!(v));
    }
    for (k, v) in extra.as_object().ok_or("pack-args.json 不是物件")? {
        args.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(args))
}

pub fn prompt_for(proj: &Path, args: &Value) -> String {
    let script = proj.join(WF_REL).join("sdlc-plan.js");
    format!(
        "用 Workflow 工具執行一支 workflow：scriptPath 是 {}，args 是下面這個 JSON 物件（原樣傳物件，不要轉成字串、不要增刪欄位）：\n{}\n\
         等 workflow 完成的通知到了之後，只回覆一行「done」。不要自己做 workflow 裡的工作，也不要讀或整理它的回傳。",
        script.display(),
        recipe::dumps(args, None)
    )
}

/// 隨機的 session id（UUID v4 的形狀）。
fn new_session_id() -> String {
    use sha2::{Digest, Sha256};
    let mut seed: Vec<u8> = Vec::new();
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut buf = [0u8; 32];
        if f.read_exact(&mut buf).is_ok() {
            seed.extend(buf);
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    seed.extend(now.to_le_bytes());
    seed.extend(std::process::id().to_le_bytes());
    let mut b: Vec<u8> = Sha256::digest(&seed).iter().take(16).copied().collect();
    if let Some(x) = b.get_mut(6) {
        *x = (*x & 0x0f) | 0x40;
    }
    if let Some(x) = b.get_mut(8) {
        *x = (*x & 0x3f) | 0x80;
    }
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let part = |a: usize, z: usize| h.get(a..z).unwrap_or_default().to_string();
    format!("{}-{}-{}-{}-{}", part(0, 8), part(8, 12), part(12, 16), part(16, 20), part(20, 32))
}

/// **會花 token**：真的呼叫 `claude -p`。回 session id。
fn run_claude(root: &Path, proj: &Path, args: &Value, model: &str, timeout_min: u64) -> Result<String, String> {
    let sid = new_session_id();
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(prompt_for(proj, args)).args(["--model", model, "--session-id", &sid, "--plugin-dir"]).arg(root.join("plugin"))
        .args(["--output-format", "json", "--allowedTools"]).args(ALLOWED).current_dir(proj);
    let r = match pyutil::run(&mut cmd, None, Some(std::time::Duration::from_secs(timeout_min * 60))) {
        Ok(o) => o,
        Err(pyutil::RunErr::Timeout) => return Err(format!("claude 超過 {timeout_min} 分鐘沒有結束")),
        Err(pyutil::RunErr::Os(e)) => return Err(format!("跑不起來 claude：{e}")),
    };
    let log = proj.join("_tpm").join(format!("claude-{sid}.log"));
    let text = format!("{}\n--- stderr ---\n{}", String::from_utf8_lossy(&r.stdout), String::from_utf8_lossy(&r.stderr));
    std::fs::write(&log, text).map_err(|e| format!("寫不了 {}：{e}", log.display()))?;
    if r.code != 0 {
        return Err(format!("claude 結束碼 {}；輸出在 {}", r.code, log.display()));
    }
    Ok(sid)
}

fn session_dir(proj: &Path, sid: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(cost::project_dir(&posix(proj)).map_err(|e| e.to_string())?).join(sid))
}

/// 讀 workflow 的回傳（Claude Code 存在 `<session>/workflows/wf_<id>.json` 的 result）。回傳（中繼資料, result）。
fn wf_result(proj: &Path, sid: &str) -> Result<(Value, Value), String> {
    let dir = session_dir(proj, sid)?.join("workflows");
    let mut metas: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("wf_") && n.ends_with(".json")))
        .collect();
    metas.sort();
    let Some(last) = metas.last() else {
        return Err(format!("找不到 workflow 的紀錄：{}（Workflow 沒有被呼叫，或這版 Claude Code 的存放位置變了）", dir.display()));
    };
    let meta = util::read_json(last)?;
    let mut res = meta.get("result").cloned().unwrap_or(Value::Null);
    if let Value::String(text) = &res {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            res = parsed;
        }
    }
    if !res.is_object() {
        return Err(format!("workflow 沒有回傳物件（status={}）：{}", pyfmt::str_opt(meta.get("status")), last.display()));
    }
    Ok((meta, res))
}

fn cost_of(proj: &Path, sid: &str) -> Result<Value, String> {
    let dir = posix(&session_dir(proj, sid)?);
    let mut last = None;
    for d in cost::find_wf_dirs(&[dir]) {
        if let Some(row) = cost::summarize(&d, "").map_err(|e| e.to_string())? {
            last = Some(row.to_json());
        }
    }
    last.ok_or_else(|| "讀不到 workflow 的 transcript（這版格式讀不了）".to_string())
}

fn slim_groups(v: Option<&Value>) -> Value {
    Value::Array(pyfmt::items(v).iter().map(|r| json!({"name": r.get("name"), "agents": r.get("agents"), "tokens": r.get("tokens")})).collect())
}

fn sorted_strings(mut xs: Vec<String>) -> Value {
    xs.sort();
    json!(xs)
}

pub fn summary_row(c: &Value, res: &Value, args: &Value) -> Value {
    let reused = match res.get("reused") {
        Some(Value::Array(a)) => sorted_strings(a.iter().map(pyfmt::str_of).collect()),
        Some(Value::Object(o)) => sorted_strings(o.keys().cloned().collect()),
        _ => json!([]),
    };
    let offered = sorted_strings(args.get("reuse").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default());
    let or = |v: Option<&Value>, default: Value| v.filter(|x| pyfmt::truthy(x)).cloned().unwrap_or(default);
    json!({
        "total_tokens": c.get("total_tokens"), "minutes": c.get("minutes"), "avg_context": c.get("avg_context"), "agents": c.get("agents"),
        "requests": c.get("requests"), "tokens": c.get("tokens"),
        "by_role": slim_groups(c.get("by_role")), "by_model": slim_groups(c.get("by_model")),
        "halted": pyfmt::truthy_opt(res.get("halted")), "halt_reason": or(res.get("halt_reason"), json!("")), "missing": or(res.get("missing"), json!([])),
        "reused": reused, "reuse_offered": offered,
        "recipe": res.get("recipe").filter(|r| pyfmt::truthy(r)).and_then(|r| r.get("name")),
    })
}

/// **會花 token**：跑一次 plan 並量成本。
fn measure(root: &Path, proj: &Path, model: &str, timeout_min: u64, name: &str) -> Result<Value, String> {
    let args = plan_args(proj)?;
    let sid = run_claude(root, proj, &args, model, timeout_min)?;
    let (meta, res) = wf_result(proj, &sid)?;
    let run = proj.join(RUN_REL);
    let res_text = recipe::dumps(&res, Some(1));
    std::fs::write(run.join(format!("{name}-result.json")), &res_text).map_err(|e| format!("寫不了 {name}-result.json：{e}"))?;
    let (code, out, err) = save_run(proj, &["stage", RUN_REL, "plan"], &res_text);
    let c = cost_of(proj, &sid)?;
    let mut row = summary_row(&c, &res, &args);
    let both = format!("{out}{err}");
    let lines = pyutil::splitlines(pyutil::strip(&both));
    let tail: Vec<&str> = lines.iter().skip(lines.len().saturating_sub(6)).copied().collect();
    if let Some(o) = row.as_object_mut() {
        o.insert("session".into(), json!(sid));
        o.insert("wf_status".into(), meta.get("status").cloned().unwrap_or(Value::Null));
        o.insert("save_run_exit".into(), json!(code));
        o.insert("save_run_tail".into(), json!(tail));
    }
    let doc = json!({"summary": row, "cost": c});
    std::fs::write(run.join(format!("{name}-cost.json")), recipe::dumps(&doc, Some(1))).map_err(|e| format!("寫不了 {name}-cost.json：{e}"))?;
    Ok(row)
}

fn num(row: &Value, k: &str) -> Option<f64> {
    row.get(k).and_then(Value::as_f64)
}

fn completed(row: &Value) -> bool {
    !pyfmt::truthy_opt(row.get("halted")) && row.get("save_run_exit").and_then(Value::as_i64) == Some(0)
}

/// 對照驗收門檻。沒跑的項目不在結果裡（不算通過）；數字讀不到的項目算沒過。
pub fn verdict(first: Option<&Value>, rerun: Option<&Value>) -> Map<String, Value> {
    let mut v = Map::new();
    let Some(first) = first else { return v };
    v.insert("total_tokens".into(), json!(num(first, "total_tokens").is_some_and(|t| t <= LIMIT_TOKENS)));
    v.insert("minutes".into(), json!(num(first, "minutes").is_some_and(|m| m <= LIMIT_MINUTES)));
    v.insert("avg_context".into(), json!(num(first, "avg_context").is_some_and(|c| c <= LIMIT_CONTEXT)));
    v.insert("completed".into(), json!(completed(first)));
    if let Some(rerun) = rerun {
        let ratio_ok = match (num(rerun, "total_tokens"), num(first, "total_tokens")) {
            (Some(r), Some(f)) => r <= f * LIMIT_RERUN,
            _ => false,
        };
        v.insert("rerun_ratio".into(), json!(ratio_ok));
        v.insert("rerun_completed".into(), json!(completed(rerun)));
    }
    v
}

/// 彙整成 `<work>/bench-result.json`；回（結果、要印的文字）。
pub fn report(proj: &Path, work: &Path, model: &str) -> Result<(Value, String), String> {
    let load = |name: &str| -> Result<Option<Value>, String> {
        let p = proj.join(RUN_REL).join(format!("{name}-cost.json"));
        if !p.is_file() {
            return Ok(None);
        }
        Ok(util::read_json(&p)?.get("summary").cloned())
    };
    let (first, rerun) = (load("first")?, load("rerun")?);
    let v = verdict(first.as_ref(), rerun.as_ref());
    let ratio = match (first.as_ref().and_then(|f| num(f, "total_tokens")), rerun.as_ref().and_then(|r| num(r, "total_tokens"))) {
        (Some(f), Some(r)) if f != 0.0 => Some(((r / f) * 1000.0).round() / 1000.0),
        _ => None,
    };
    let pass = !v.is_empty() && ["total_tokens", "minutes", "avg_context", "completed", "rerun_ratio", "rerun_completed"].iter().all(|k| v.get(*k) == Some(&json!(true)));
    let out = json!({"model": model, "limits": limits(), "first": first, "rerun": rerun, "rerun_ratio": ratio, "verdict": v, "pass": pass});
    let dst = work.join("bench-result.json");
    std::fs::write(&dst, recipe::dumps(&out, Some(1))).map_err(|e| format!("寫不了 {}：{e}", dst.display()))?;
    let mut text = String::new();
    for (name, row) in [("首次", &first), ("重跑", &rerun)] {
        let Some(row) = row else { continue };
        let halted = if pyfmt::truthy_opt(row.get("halted")) { format!("、halted：{}", pyfmt::str_opt(row.get("halt_reason"))) } else { String::new() };
        let reused: Vec<String> = pyfmt::items(row.get("reused")).iter().map(pyfmt::str_of).collect();
        let reused = if reused.is_empty() { String::new() } else { format!("、沿用 {}", reused.join("、")) };
        text.push_str(&format!(
            "{name}：{:.2}M token、{} 分、平均 context {:.1}k、{} 個 agent{halted}{reused}\n",
            num(row, "total_tokens").unwrap_or(0.0) / 1e6,
            pyfmt::str_opt(row.get("minutes")),
            num(row, "avg_context").unwrap_or(0.0) / 1000.0,
            pyfmt::str_opt(row.get("agents")),
        ));
    }
    if let Some(r) = ratio {
        text.push_str(&format!("重跑／首次：{:.0}%\n", r * 100.0));
    }
    let marks: Vec<String> = v.iter().map(|(k, ok)| format!("{k}={}", if ok == &json!(true) { "過" } else { "沒過" })).collect();
    text.push_str(&format!("門檻：{} → {}\n", marks.join("、"), if pass { "達標" } else { "未達標" }));
    Ok((out, text))
}

/// 專案的 settings.json 有註冊 hook（`--hooks project` 的結果）。
fn hooks_registered(proj: &Path) -> bool {
    util::read_json(&proj.join(".claude").join("settings.json")).ok()
        .and_then(|v| v.get("hooks").and_then(Value::as_object).map(|h| !h.is_empty()))
        .unwrap_or(false)
}

/// 不花 token 的部分：回（是否全過、輸出）。
pub fn selftest(root: &Path) -> (bool, String) {
    let mut ok = true;
    let mut out = String::new();
    let mut check = |out: &mut String, name: &str, cond: bool, detail: String| {
        ok = ok && cond;
        out.push_str(&format!("{}{name}{}\n", if cond { "通過  " } else { "失敗  " }, if !cond && !detail.is_empty() { format!("：{detail}") } else { String::new() }));
    };
    let tmp = match TempDir::new("awf-bench.") {
        Ok(t) => t,
        Err(e) => return (false, format!("失敗  建不了暫存目錄：{e}\n")),
    };
    let work = tmp.path();
    if let Err(e) = crate::awfbin::build(root) {
        return (false, format!("失敗  建置 awf：{e}\n"));
    }
    let proj = match setup(root, work) {
        Ok(p) => p,
        Err(e) => return (false, format!("失敗  setup：{e}\n")),
    };
    let run = proj.join(RUN_REL);
    let set = run.join("run.json").is_file() && proj.join(WF_REL).join("sdlc-plan.js").is_file() && !proj.join(".claude").join("workflows").exists()
        && hooks_registered(&proj) && proj.join(REPO_REL).join(".git").is_dir();
    check(&mut out, "setup：專案已 init、有 run.json 與知識庫", set, String::new());
    let pack_file = run.join("plan").join("pack.md");
    let step = |proj: &Path| -> Result<(Value, Vec<u8>), String> {
        let a = plan_args(proj)?;
        let pack = std::fs::read(&pack_file).map_err(|e| format!("讀不到 pack.md：{e}"))?;
        Ok((a, pack))
    };
    let ((a1, pack1), (a2, pack2)) = match (step(&proj), step(&proj)) {
        (Ok(x), Ok(y)) => (x, y),
        (x, y) => {
            out.push_str(&format!("失敗  備資料包：{}\n", x.err().or(y.err()).unwrap_or_default()));
            return (false, out);
        }
    };
    let filled = ["repo", "epic", "requirement", "base", "run_dir", "pack"].iter().all(|k| pyfmt::truthy_opt(a1.get(*k)));
    check(&mut out, "args 有必填欄位與資料包", filled, recipe::dumps(&a1, None));
    check(&mut out, "資料包可重現（兩次內容相同）", pack1 == pack2 && a1 == a2, String::new());
    let pack_text = String::from_utf8_lossy(&pack1);
    check(&mut out, "資料包有知識庫命中目標 repo 的行段", pack_text.contains(&format!("{REPO_REL}/docs/")), pack_text.chars().take(400).collect());
    let p = prompt_for(&proj, &a1);
    check(&mut out, "提示帶 scriptPath 與 args", p.contains("sdlc-plan.js") && p.contains(RUN_REL), String::new());
    let good = json!({"total_tokens": 4_000_000, "minutes": 8.0, "avg_context": 30_000, "halted": false, "save_run_exit": 0});
    let with = |k: &str, v: Value| {
        let mut g = good.clone();
        if let Some(o) = g.as_object_mut() {
            o.insert(k.to_string(), v);
        }
        g
    };
    let all_true = |v: &Map<String, Value>| !v.is_empty() && v.values().all(|x| x == &json!(true));
    check(&mut out, "verdict：達標", all_true(&verdict(Some(&good), Some(&with("total_tokens", json!(1_000_000))))), String::new());
    check(&mut out, "verdict：重跑超過 30% 沒過", verdict(Some(&good), Some(&with("total_tokens", json!(1_300_000)))).get("rerun_ratio") == Some(&json!(false)), String::new());
    check(&mut out, "verdict：超過 5M 沒過", verdict(Some(&with("total_tokens", json!(5_000_001))), None).get("total_tokens") == Some(&json!(false)), String::new());
    check(&mut out, "verdict：halted 不算完成", verdict(Some(&with("halted", json!(true))), None).get("completed") == Some(&json!(false)), String::new());
    match report(&proj, work, "x") {
        Ok((res, text)) => {
            out.push_str(&text);
            check(&mut out, "report：沒跑過時不算達標", res.get("pass") == Some(&json!(false)), String::new());
        }
        Err(e) => check(&mut out, "report：沒跑過時不算達標", false, e),
    }
    (ok, out)
}

fn usage() -> Captured {
    (2, "用法：\n  cargo xtask bench --selftest\n  cargo xtask bench --work <空目錄> [--model opus] [--step setup|plan|rerun|report] [--timeout 分鐘]   ← 會真的呼叫 claude，花數百萬 token\n  \
         cargo xtask bench fixture <目標目錄> [--no-git]\n  cargo xtask bench manifest [--write]\n".into(), String::new())
}

fn fixture_cmd(args: &[String]) -> Captured {
    let targets: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [target] = targets.as_slice() else { return usage() };
    let target = PathBuf::from(target);
    let res = std::fs::create_dir_all(&target).map_err(|e| format!("建不了 {}：{e}", target.display())).and_then(|()| fixture::write(&target));
    let sums = match res {
        Ok(s) => s,
        Err(e) => return (1, String::new(), format!("{e}\n")),
    };
    if !args.iter().any(|a| a == "--no-git") {
        if let Err(e) = fixture::git_init(&target) {
            return (1, String::new(), format!("{e}\n"));
        }
    }
    let st = fixture::stats(&target, &sums);
    (0, format!("已產生 {} 個檔、{} 行；超過 400 行的檔：{} 個\n", st.files, st.lines, st.big_files.len()), String::new())
}

fn manifest_cmd(root: &Path, args: &[String]) -> Captured {
    if args.iter().any(|a| a == "--write") {
        let p = root.join(fixture::MANIFEST_REL);
        return match std::fs::write(&p, fixture::manifest_text()) {
            Ok(()) => (0, format!("已寫 {}（重新編譯後 selftest 才會用到新的清單）\n", fixture::MANIFEST_REL), String::new()),
            Err(e) => (1, String::new(), format!("寫不了 {}：{e}\n", fixture::MANIFEST_REL)),
        };
    }
    let diff = fixture::manifest_diff();
    if diff.is_empty() {
        return (0, "虛構專案和清單逐檔相同\n".into(), String::new());
    }
    (1, format!("虛構專案和清單不同：\n{}\n", diff.iter().map(|d| format!("  - {d}")).collect::<Vec<_>>().join("\n")), String::new())
}

/// 指令本體：`root` 是 repo 根目錄。
pub fn run_captured(root: &Path, args: &[String]) -> Captured {
    if args.iter().any(|a| a == "--selftest") {
        let (ok1, out1) = fixture::selftest();
        let (ok2, out2) = selftest(root);
        return (if ok1 && ok2 { 0 } else { 1 }, format!("{out1}{out2}"), String::new());
    }
    match args.first().map(String::as_str) {
        Some("fixture") => return fixture_cmd(args.get(1..).unwrap_or_default()),
        Some("manifest") => return manifest_cmd(root, args.get(1..).unwrap_or_default()),
        _ => {}
    }
    let opt = |name: &str, default: &str| util::take_opt(args, name).ok().and_then(|(v, _)| v).unwrap_or_else(|| default.to_string());
    let work_raw = opt("--work", "");
    if work_raw.is_empty() {
        return usage();
    }
    let fail = |e: String| (1, String::new(), format!("{e}\n"));
    let work = PathBuf::from(&work_raw);
    if let Err(e) = std::fs::create_dir_all(&work) {
        return fail(format!("建不了 {work_raw}：{e}"));
    }
    let work = work.canonicalize().unwrap_or(work);
    let (model, step) = (opt("--model", "opus"), opt("--step", "all"));
    let Ok(timeout_min) = opt("--timeout", "45").parse::<u64>() else { return fail("--timeout 要是分鐘數（整數）".into()) };
    let mut proj = work.join("ws");
    let mut out = String::new();
    if let Err(e) = crate::awfbin::build(root) {
        return fail(format!("建置 awf：{e}"));
    }
    let on = |s: &str| step == "all" || step == s;
    if on("setup") {
        proj = match setup(root, &work) {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        out.push_str(&format!("工作區：{}（目標 repo：{REPO_REL}）\n", proj.display()));
    }
    for (s, name) in [("plan", "first"), ("rerun", "rerun")] {
        if on(s) {
            match measure(root, &proj, &model, timeout_min, name) {
                Ok(row) => out.push_str(&format!("{}\n", recipe::dumps(&row, Some(1)))),
                Err(e) => return (1, out, format!("{e}\n")),
            }
        }
    }
    if on("report") {
        return match report(&proj, &work, &model) {
            Ok((res, text)) => (if res.get("pass") == Some(&json!(true)) { 0 } else { 1 }, format!("{out}{text}"), String::new()),
            Err(e) => (1, out, format!("{e}\n")),
        };
    }
    (0, out, String::new())
}
