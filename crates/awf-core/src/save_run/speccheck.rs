//! 規格層接進存檔與閘門（S4）：需求來源是 `openspec` 時才啟用，沒選就完全不做事。
//!
//! - plan：proposal 就是 intent，用對照表把 proposal 的節對到必要節（必要內容不能少）。
//! - design：子任務的 `spec_refs`、`acceptance` 填的 id 要在規格（現行＋這次變更的差異）裡找得到，並跑 `spec validate`。
//! - test：逐條結果的 id 用同一組；對不到的、漏驗的都列出來。
//! - 基準：plan、design 核准時把變更的 proposal、差異檔（雜湊）與條目清單記進 run.json（[`spec_baseline`]）；
//!   之後 design、test、deliver 每次重算都和它比（[`baseline_drift`]）。規格檔 team agent 改得到，
//!   不釘住的話，把沒過的情境從差異檔刪掉檢查就過了。合法的修訂路徑是重跑 design 段（重存會清掉核准，基準跟著清掉）。
//!
//! 需求設定的讀法和 `awf merge-gate` 一樣：`guard.config.json` 的 `requirements` 有設 path／command 時以它為準，否則用 profile 的。
//! 規格目錄讀不到、找不到對應的變更都算問題（fail-closed），不當成「沒有規格」。

use super::checks::{intent_file_errors, intent_paths, result_subtasks};
use super::py::{self, get, iter_or, pj, pname, s, s_or, truthy, R};
use super::trace::envelopes;
use super::Ctx;
use crate::{recipe, spec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 讀好的規格與這個 run 對應的變更。
pub struct SpecCtx {
    pub conf: spec::SourceCfg,
    pub opened: spec::Opened,
    /// 這個 run 對應的進行中變更；找不到是 None。
    pub change: Option<String>,
    /// 找變更時試過的名字（給訊息用）。
    pub tried: Vec<String>,
}

impl SpecCtx {
    /// 找得到的 id（現行規格＋這次變更的差異）。
    pub fn known(&self) -> BTreeSet<String> {
        spec::known_ids(&self.opened.tree, self.change.as_deref())
    }

    /// 這次變更要驗的條目（沒有對應的變更是空的）。
    pub fn items(&self) -> Vec<spec::Item> {
        self.change.as_deref().and_then(|c| spec::change_items(&self.opened.tree, c)).unwrap_or_default()
    }

    fn no_change(&self) -> String {
        format!(
            "需求來源是 openspec，但規格目錄 {} 找不到這個 run 對應的進行中變更（試過：{}）；在 run.json 的 args.change 或設定的 requirements.change 指定變更名稱",
            self.conf.display,
            if self.tried.is_empty() { "沒有可用的名字".to_string() } else { self.tried.join("、") }
        )
    }
}

/// 生效的需求設定：`guard.config.json` 有設 path／command 時用它的，否則用 profile 的。來源不是 openspec 回 None。
pub fn spec_setting(ctx: &Ctx, profile: &Value) -> Option<Value> {
    let guard = ctx.load_json(&pj(&ctx.teams, "guard.config.json")).ok();
    let from_guard = guard.as_ref().and_then(|g| g.get("requirements")).filter(|rc| crate::merge_gate::configured(rc));
    let rc = match from_guard {
        Some(rc) => rc,
        None => profile.get("requirements").filter(|rc| rc.is_object())?,
    };
    (rc.get("source").and_then(Value::as_str) == Some("openspec")).then(|| rc.clone())
}

/// 讀規格並找這個 run 對應的變更。需求來源不是 openspec 回 None（沒啟用）；讀不到回 Some(Err)。
/// 變更名稱依序試：run.json 的 `args.change`、設定的 `requirements.change`、epic（run 的 epic 或 `args.epic`）。
pub fn open_spec(ctx: &Ctx, run: &str, data: &Value, profile: &Value) -> Option<Result<SpecCtx, String>> {
    let rc = spec_setting(ctx, profile)?;
    Some((|| {
        let conf = spec::source_cfg(&rc, &ctx.abs(&ctx.project_root())).map_err(|e| format!("需求來源 openspec 的設定不對：{e}"))?;
        let opened = spec::open_source(&conf).map_err(|e| format!("讀不到規格目錄 {}：{e}", conf.display))?;
        let args = data.get("args").filter(|a| a.is_object());
        let arg = |k: &str| args.and_then(|a| a.get(k)).and_then(Value::as_str).map(|t| t.trim().to_string()).unwrap_or_default();
        let epic = super::run_epic(ctx, run).unwrap_or_default();
        let mut tried: Vec<String> = Vec::new();
        for name in [arg("change"), conf.change.clone(), epic, arg("epic")] {
            if !name.is_empty() && !tried.contains(&name) {
                tried.push(name);
            }
        }
        let active = spec::active_changes(&opened.tree);
        let change = tried.iter().find(|n| active.contains(&n.as_str())).cloned();
        Ok(SpecCtx { conf, opened, change, tried })
    })())
}

fn head(items: &[String], n: usize) -> String {
    format!("{}{}", items.iter().take(n).cloned().collect::<Vec<_>>().join("、"), if items.len() > n { "…" } else { "" })
}

/// 開頭的 id：到第一個空白或全形冒號為止（`orders/建立訂單#庫存足夠 訂單成立` → `orders/建立訂單#庫存足夠`）。
pub fn lead_token(entry: &str) -> &str {
    let t = entry.trim();
    t.split(|c: char| c.is_whitespace() || c == '：').next().unwrap_or(t)
}

/// 需求來源是 openspec 時的 intent 檢查：proposal 就是 intent。
/// proposal 要存在，對照之後必要的節都要有內容；信封另外列的 kind=intent 文件（不是 proposal.md）照原本的規則檢查。
pub fn intent_errors_spec(ctx: &Ctx, sc: &Result<SpecCtx, String>, result: &Value, profile: &Value, plan_mode: &Value) -> R<Vec<String>> {
    let sc = match sc {
        Ok(sc) => sc,
        Err(e) => return Ok(vec![format!("intent：{e}")]),
    };
    let Some(change) = sc.change.as_deref() else {
        return Ok(vec![format!("intent：{}", sc.no_change())]);
    };
    let mut errs: Vec<String> = Vec::new();
    let dir = sc.opened.tree.change(change, spec::ChangeState::Active).map(|c| c.dir.clone()).unwrap_or_else(|| format!("changes/{change}"));
    let shown = format!("{}/{dir}/proposal.md", sc.conf.display);
    match std::fs::read(sc.conf.root.join(&dir).join("proposal.md")) {
        Err(_) => errs.push(format!("intent：變更「{change}」沒有 proposal.md（需求來源是 openspec 時，proposal 就是 intent）：{shown}")),
        Ok(raw) => {
            let text = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
            let need: Vec<String> = recipe::required_sections(profile, plan_mode).map_err(py::from_recipe)?.iter().map(s).collect();
            let scenarios = sc.items().iter().filter(|i| i.is_scenario).count();
            let miss = spec::proposal_missing(&text, &need, sc.conf.section_map.as_ref(), scenarios);
            if !miss.is_empty() {
                errs.push(format!("intent（proposal）缺必要的節（plan_mode={}）：{}（{shown}；節名對照見 requirements.section_map）", s(plan_mode), miss.join("、")));
            }
        }
    }
    let others: Vec<String> = intent_paths(result)?.into_iter().filter(|p| pname(p) != "proposal.md").collect();
    if !others.is_empty() {
        errs.extend(intent_file_errors(ctx, result, &others, profile, plan_mode)?);
    }
    Ok(errs)
}

/// test 段彙整後的逐條結果：（原本的 id、去掉分片前綴的 id、驗證指令、那份結果的 commit）。
/// 有 `verifier`（彙整結果）就只看它，否則看回傳裡每一份帶 criteria 的 payload。
pub fn criteria_rows(result: &Value) -> Vec<(String, String, String, String)> {
    let main = result.get("verifier").filter(|v| v.is_object());
    let payloads: Vec<&Value> = envelopes(main.unwrap_or(result)).into_iter().filter_map(|e| e.get("payload")).filter(|p| p.get("criteria").is_some_and(Value::is_array)).collect();
    let mut out = Vec::new();
    for p in payloads {
        let commit = p.get("commit").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        for c in p.get("criteria").and_then(Value::as_array).into_iter().flatten().filter(|c| c.is_object()) {
            let id = c.get("id").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let bare = id.split_once(':').map_or(id.as_str(), |(_, rest)| rest).trim().to_string();
            let command = c.get("command").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            out.push((id, bare, command, commit.clone()));
        }
    }
    out
}

/// 核准紀錄裡放規格基準的鍵（`stages.<plan|design>.approved.spec`）。跟著核准走：重存該段、escalate 改成 stale 時和核准一起清掉。
pub const BASELINE_KEY: &str = "spec";
/// 基準的格式版本。
pub const BASELINE_VERSION: u64 = 1;
/// 一個變更的差異檔最多幾個（超過就不記基準，當成問題）。
const BASELINE_MAX_FILES: usize = 2000;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 變更目錄底下 `specs/**` 的每個檔：（相對於變更目錄的路徑 → sha256）。符號連結不跟，記成 `symlink:<目標>`。
fn delta_files(change_dir: &std::path::Path) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![("specs".to_string(), change_dir.join("specs"))];
    while let Some((rel, dir)) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && rel == "specs" => continue,
            Err(e) => return Err(format!("讀不到 {rel}：{e}")),
        };
        for e in rd {
            let e = e.map_err(|e| format!("讀不到 {rel}：{e}"))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let (child, path) = (format!("{rel}/{name}"), e.path());
            let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("讀不到 {child}：{e}"))?;
            if meta.file_type().is_symlink() {
                let to = std::fs::read_link(&path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                out.insert(child, format!("symlink:{to}"));
            } else if meta.is_dir() {
                stack.push((child, path));
            } else {
                let raw = std::fs::read(&path).map_err(|e| format!("讀不到 {child}：{e}"))?;
                out.insert(child, sha256_hex(&raw));
            }
            if out.len() > BASELINE_MAX_FILES {
                return Err(format!("差異檔超過 {BASELINE_MAX_FILES} 個"));
            }
        }
    }
    Ok(out)
}

/// 規格現在的樣子（核准時記成基準；之後重算時拿來和基準比）：
/// `{v, change, proposal, files, items}`——proposal 是 `proposal.md` 的 sha256（沒有這個檔是空字串），
/// files 是差異檔（`specs/**`）各自的 sha256，items 是條目的穩定 id → 標題。
/// `conf`、`opened` 是讀好的規格，`change` 是進行中的變更名稱。
pub fn spec_snapshot(conf: &spec::SourceCfg, opened: &spec::Opened, change: &str) -> Result<Value, String> {
    let ch = opened.tree.change(change, spec::ChangeState::Active).ok_or_else(|| format!("規格目錄 {} 沒有進行中的變更「{change}」", conf.display))?;
    let dir = conf.root.join(&ch.dir);
    let proposal = match std::fs::read(dir.join("proposal.md")) {
        Ok(raw) => sha256_hex(&raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("讀不到變更「{change}」的 proposal.md：{e}")),
    };
    let files = delta_files(&dir).map_err(|e| format!("變更「{change}」的差異檔：{e}"))?;
    let items: BTreeMap<String, String> = spec::change_items(&opened.tree, change).unwrap_or_default().into_iter().map(|i| (i.id, i.title)).collect();
    Ok(json!({"v": BASELINE_VERSION, "change": change, "proposal": proposal, "files": files, "items": items}))
}

/// 核准 plan、design 時要記的規格基準。需求來源不是 openspec（`sc` 是 None）回 None；算不出來回 Some(Err)。
pub fn spec_baseline(sc: Option<&Result<SpecCtx, String>>) -> Option<Result<Value, String>> {
    Some(match sc? {
        Err(e) => Err(e.clone()),
        Ok(sc) => match sc.change.as_deref() {
            None => Err(sc.no_change()),
            Some(c) => spec_snapshot(&sc.conf, &sc.opened, c),
        },
    })
}

/// run.json 裡某一段核准時記的規格基準。
pub fn baseline_of<'a>(data: &'a Value, stage: &str) -> Option<&'a Value> {
    data.get("stages")?.get(stage)?.get("approved")?.get(BASELINE_KEY).filter(|b| b.is_object() && b.get("items").is_some_and(Value::is_object))
}

/// `stage` 重算時要比的基準是哪一段核准時記的：design 段比 plan 的；test、deliver（與 publish、merge gate）比 design 的，
/// 這個 run 沒有 design 段（recipe 跳過）就比 plan 的。
pub fn baseline_stage(data: &Value, stage: &str) -> &'static str {
    let has_design = data.get("stages").and_then(|s| s.get("design")).is_some_and(|d| d.is_object() && d.get("status").and_then(Value::as_str) != Some("skipped"));
    if stage != "design" && has_design { "design" } else { "plan" }
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object).map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default()
}

/// 現在的規格（`now`）和核准時的基準（`base`）差在哪裡。`from` 是基準出自哪一段（訊息用）。
/// `full` 為否時只比 proposal 與條目有沒有變少（design 段比 plan 的基準：差異檔本來就是 design 段在寫）。
pub fn baseline_diff(base: &Value, now: &Value, from: &str, full: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let hint = if from == "design" { "要修訂規格請重跑 design 段（重存會清掉核准），不能直接改" } else { "要改需求請重跑 plan 段並重新核准" };
    if text(base, "change") != text(now, "change") {
        out.push(format!("這個 run 對應的變更從「{}」（{from} 核准時）換成「{}」；{hint}", py::take(&text(base, "change"), 80), py::take(&text(now, "change"), 80)));
        return out;
    }
    if text(base, "proposal") != text(now, "proposal") {
        out.push(format!("變更的 proposal.md 在 {from} 核准之後改過（內容雜湊不同）；{hint}"));
    }
    let (old_items, new_items) = (str_map(base.get("items")), str_map(now.get("items")));
    let gone: Vec<String> = old_items.keys().filter(|k| !new_items.contains_key(*k)).map(|k| py::take(k, 80).to_string()).collect();
    if !gone.is_empty() {
        out.push(format!("變更的規格條目比 {from} 核准時少了 {} 條（從差異檔刪掉，或 ids.json 把 id 改掉了）：{}；{hint}", gone.len(), head(&gone, 8)));
    }
    let moved: Vec<String> = old_items.iter().filter(|(k, t)| new_items.get(*k).is_some_and(|n| n != *t)).map(|(k, _)| py::take(k, 80).to_string()).collect();
    if !moved.is_empty() {
        out.push(format!("有 {} 個 id 現在對到的條目和 {from} 核准時不同（標題或 ids.json 改過）：{}；{hint}", moved.len(), head(&moved, 8)));
    }
    if full {
        let (old_files, new_files) = (str_map(base.get("files")), str_map(now.get("files")));
        let mut changed: Vec<String> = Vec::new();
        for (f, h) in &old_files {
            match new_files.get(f) {
                None => changed.push(format!("{}（刪除）", py::take(f, 80))),
                Some(n) if n != h => changed.push(format!("{}（改過）", py::take(f, 80))),
                _ => {}
            }
        }
        changed.extend(new_files.keys().filter(|f| !old_files.contains_key(*f)).map(|f| format!("{}（新增）", py::take(f, 80))));
        if !changed.is_empty() {
            out.push(format!("變更的差異檔在 {from} 核准之後變了 {} 個：{}；{hint}", changed.len(), head(&changed, 8)));
        }
    }
    out
}

/// 規格和這個 run 核准時記下的基準比（`data` 是 run.json）。回傳問題（空＝沒變）。
/// 該有基準卻沒有（那一段還沒核准、或核准時沒記成）也算問題：沒有東西可以比，不能當成沒變。
pub fn baseline_drift(stage: &str, data: &Value, conf: &spec::SourceCfg, opened: &spec::Opened, change: &str) -> Vec<String> {
    let from = baseline_stage(data, stage);
    let Some(base) = baseline_of(data, from) else {
        return vec![format!("run.json 沒有 {from} 核准時的規格基準（{from} 還沒核准，或核准時需求來源還不是 openspec）：沒有東西可以比，重新核准 {from} 段")];
    };
    match spec_snapshot(conf, opened, change) {
        Ok(now) => baseline_diff(base, &now, from, stage != "design"),
        Err(e) => vec![format!("算不出規格現在的樣子，無法和 {from} 核准時的基準比：{e}")],
    }
}

/// design、test、deliver 段的規格檢查。回傳問題清單（空＝沒問題）；`approve --by policy` 重算，不過就拒絕，人工核准只警告。
/// `data` 是 run.json（取核准時記下的規格基準）。
pub fn spec_issues(stage: &str, result: &Value, sc: &Result<SpecCtx, String>, data: &Value) -> R<Vec<String>> {
    if !matches!(stage, "design" | "test" | "deliver") {
        return Ok(Vec::new());
    }
    let sc = match sc {
        Ok(sc) => sc,
        Err(e) => return Ok(vec![e.clone()]),
    };
    let mut out: Vec<String> = Vec::new();
    match sc.change.as_deref() {
        None => out.push(sc.no_change()),
        Some(c) => out.extend(baseline_drift(stage, data, &sc.conf, &sc.opened, c)),
    }
    let known = sc.known();
    let items = sc.items();
    let scope = match sc.change.as_deref() {
        Some(c) => format!("現行規格＋變更「{c}」的差異"),
        None => "現行規格".to_string(),
    };
    if stage == "design" {
        let problems = spec::validate(&sc.opened.tree, &sc.opened.ids, sc.change.as_deref());
        for p in problems.iter().take(10) {
            out.push(format!("spec validate：{}", spec::format_problem(&sc.conf.display, p)));
        }
        if problems.len() > 10 {
            out.push(format!("spec validate：另有 {} 項（awf spec validate 看全部）", problems.len() - 10));
        }
        if !sc.opened.ids.pending.is_empty() {
            let ids: Vec<String> = sc.opened.ids.pending.iter().map(|p| p.id.clone()).collect();
            out.push(format!("有 {} 個 id 待確認是不是改了標題（awf spec ids）：{}", ids.len(), head(&ids, 8)));
        }
        let subs = result_subtasks(result);
        let mut covered: BTreeSet<String> = BTreeSet::new();
        for x in &subs {
            let tid = s_or(get(x, "id")?, "?");
            for field in ["spec_refs", "acceptance"] {
                let mut lost: Vec<String> = Vec::new();
                for e in iter_or(get(x, field)?)? {
                    let text = s(&e);
                    let tok = lead_token(&text);
                    // acceptance 可以有不對應規格的條目（例如「lint 通過」）：開頭長得像規格 id（有 `/`）的才核對
                    if tok.is_empty() || (field == "acceptance" && !tok.contains('/')) {
                        continue;
                    }
                    if known.contains(tok) {
                        if field == "acceptance" {
                            covered.insert(tok.to_string());
                        }
                    } else {
                        lost.push(py::take(tok, 80).to_string());
                    }
                }
                if !lost.is_empty() {
                    out.push(format!("子任務 {tid} 的 {field} 有 {} 項在{scope}裡找不到（規格 id 寫在開頭，awf spec status 列得出）：{}", lost.len(), head(&lost, 8)));
                }
            }
        }
        if !subs.is_empty() {
            let orphan: Vec<String> = items.iter().filter(|i| !covered.contains(&i.id) && !covered.contains(&i.requirement)).map(|i| i.id.clone()).collect();
            if !orphan.is_empty() {
                out.push(format!("變更的規格差異有 {} 條沒有任何子任務的 acceptance 對應：{}", orphan.len(), head(&orphan, 8)));
            }
        }
    }
    if stage == "test" && truthy(result) {
        let rows = criteria_rows(result);
        let hit = |id: &str, bare: &str| known.contains(id) || known.contains(bare);
        let unknown: Vec<String> = rows.iter().filter(|(id, bare, _, _)| !hit(id, bare)).map(|(id, ..)| py::take(id, 80).to_string()).collect();
        if !unknown.is_empty() {
            out.push(format!("逐條結果有 {} 個 id 在{scope}裡找不到：{}", unknown.len(), head(&unknown, 8)));
        }
        let seen: BTreeSet<&str> = rows.iter().flat_map(|(id, bare, _, _)| [id.as_str(), bare.as_str()]).collect();
        let unverified: Vec<String> = items.iter().filter(|i| !seen.contains(i.id.as_str())).map(|i| i.id.clone()).collect();
        if !unverified.is_empty() {
            out.push(format!("變更的規格差異有 {} 條沒有逐條結果：{}", unverified.len(), head(&unverified, 8)));
        }
    }
    Ok(out)
}
