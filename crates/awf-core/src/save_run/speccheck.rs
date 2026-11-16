//! 規格層接進存檔與閘門（S4）：需求來源是 `openspec` 時才啟用，沒選就完全不做事。
//!
//! - plan：proposal 就是 intent，用對照表把 proposal 的節對到必要節（必要內容不能少）。
//! - design：子任務的 `spec_refs`、`acceptance` 填的 id 要在規格（現行＋這次變更的差異）裡找得到，並跑 `spec validate`。
//! - test：逐條結果的 id 用同一組；對不到的、漏驗的都列出來。
//!
//! 需求設定的讀法和 `awf merge-gate` 一樣：`guard.config.json` 的 `requirements` 有設 path／command 時以它為準，否則用 profile 的。
//! 規格目錄讀不到、找不到對應的變更都算問題（fail-closed），不當成「沒有規格」。

use super::checks::{intent_file_errors, intent_paths, result_subtasks};
use super::py::{self, get, iter_or, pj, pname, s, s_or, truthy, R};
use super::trace::envelopes;
use super::Ctx;
use crate::{recipe, spec};
use serde_json::Value;
use std::collections::BTreeSet;

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

/// design、test 段的規格檢查。回傳問題清單（空＝沒問題）；`approve --by policy` 重算，不過就拒絕，人工核准只警告。
pub fn spec_issues(stage: &str, result: &Value, sc: &Result<SpecCtx, String>) -> R<Vec<String>> {
    if !matches!(stage, "design" | "test") {
        return Ok(Vec::new());
    }
    let sc = match sc {
        Ok(sc) => sc,
        Err(e) => return Ok(vec![e.clone()]),
    };
    let mut out: Vec<String> = Vec::new();
    if sc.change.is_none() {
        out.push(sc.no_change());
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
