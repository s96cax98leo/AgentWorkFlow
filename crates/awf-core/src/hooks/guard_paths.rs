//! guard-paths：Edit／Write／NotebookEdit 的寫入範圍，Read／Grep 的機敏檔。原本是 0.x 的 Python 工具 `guard.py` 的 `check_paths`、`check_read`。

use super::guard::{err, fold, git_toplevel, is_secret_path, norm, str_or_empty, tool_input, Guard, AUDIT_FILES, PACK_STATE, R, SECRET_FILE_OK, SHARED_CONTEXT_NAMES};
use super::path;
use super::re::Re;
use crate::config;
use crate::pyfmt;
use serde_json::Value;

static SECRET_GLOB: Re = Re::new(r"(?:^|[/{,])\.env|id_(?:rsa|dsa|ecdsa|ed25519)|\.(?:pem|p12|pfx|key|keystore|jks)\b|\.npmrc|\.pypirc|credentials", "i");
/// linked repo 裡的 specs/<epic>/*.md（相對於那個 repo）
static LINKED_SPEC_DOC: Re = Re::new(r"^(?:\.worktrees/[^/]+/)?specs/[^/]+/[^/]+\.md$", "");
static VERIFIER_REPORT: Re = Re::new(r"^(?:verifier(?:-[\w.-]+)?\.md)\z", "");
static SECURITY_REPORT: Re = Re::new(r"^(?:security-review\.md)\z", "");
static DESIGN_REPORT: Re = Re::new(r"^(?:design-review\.md)\z", "");

const RECORD_RESERVED_NAMES: &[&str] = &["context.md", "pack.md", "執行紀錄索引.md", "run.json", "決策記憶.md", "結案摘要.md"];

/// 唯讀審查角色只能寫自己的報告：（比對用的樣式, 訊息裡印的 Python 樣式原文）。
fn review_report(role: &str) -> Option<(&'static Re, &'static str)> {
    match role {
        "dev-verifier" => Some((&VERIFIER_REPORT, r"verifier(?:-[\w.-]+)?\.md")),
        "dev-security-reviewer" => Some((&SECURITY_REPORT, r"security-review\.md")),
        "dev-design-reviewer" => Some((&DESIGN_REPORT, r"design-review\.md")),
        _ => None,
    }
}

pub fn all_patterns() -> Vec<&'static Re> {
    vec![&SECRET_GLOB, &LINKED_SPEC_DOC, &VERIFIER_REPORT, &SECURITY_REPORT, &DESIGN_REPORT]
}

impl Guard {
    /// Read（file_path）、Grep（path、glob）讀機敏檔：主 session 與 agent 都擋（沒有設定檔時只擋模版角色）。
    pub fn check_read(&self, data: &Value) -> R<()> {
        let ti = tool_input(data)?;
        let raw_agent = str_or_empty(data, "agent_type")?;
        if !(self.cfg.configured || config::is_team_agent(&raw_agent, &self.cfg)) {
            return Ok(());
        }
        let rn = config::role_name(&raw_agent);
        let who = if rn.is_empty() { "主 session" } else { rn };
        for key in ["file_path", "path", "notebook_path"] {
            if let Some(Value::String(v)) = ti.get(key) {
                if !v.is_empty() && is_secret_path(v) {
                    return Err(self.block(&format!("{who} 不讀機敏檔 {v}（.env、金鑰、憑證）；要判斷有無只能在 shell 用 grep -q（rules.md 規則 5）")));
                }
            }
        }
        if let Some(Value::String(g)) = ti.get("glob") {
            if SECRET_GLOB.is_match(g) && !SECRET_FILE_OK.is_match(g) {
                return Err(self.block(&format!("{who} 不搜尋機敏檔（glob {g}）；要判斷有無只能在 shell 用 grep -q（rules.md 規則 5）")));
            }
        }
        Ok(())
    }

    /// Write／Edit 的路徑碰到 .claude/teams/、.claude/workflows/、.wf/、methodology.json 或 guard.config.json。
    fn team_config_path(&self, p: &str) -> R<bool> {
        let mut cands = vec![norm(p)];
        match path::resolve(&norm(p)) {
            Ok(r) => cands.push(r),
            Err(e) if e.class == "FileNotFoundError" || e.class == "OSError" => {}
            Err(e) => return Err(e.into()),
        }
        for c in cands {
            let f = fold(&c);
            let g = format!("/{f}");
            let base = f.rsplit('/').next().unwrap_or("");
            if g.contains("/.claude/teams/") || g.contains("/.claude/workflows/") || g.contains("/.wf/") || base == "methodology.json" || base == "guard.config.json" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Write／Edit 的路徑在某個 repo 的 `.github/workflows/` 底下（CI 檔；照寫的路徑與解開符號連結後的路徑都看）。
    fn ci_workflow_path(&self, p: &str) -> R<bool> {
        let mut cands = vec![norm(p)];
        match path::resolve(&norm(p)) {
            Ok(r) => cands.push(r),
            Err(e) if e.class == "FileNotFoundError" || e.class == "OSError" => {}
            Err(e) => return Err(e.into()),
        }
        Ok(cands.iter().any(|c| format!("/{}", fold(c)).contains("/.github/workflows/")))
    }

    /// 模版角色不論寫入範圍都不能寫的檔（守門設定、稽核紀錄、凍結檔、profile 與 workflow 腳本、CI 檔、知識圖、
    /// run 狀態與共用背景）：回碰到的是哪一種。給「請工具代寫」的路徑用（`awf run-brief --log <路徑>`），
    /// 判法和 [`Self::check_paths_rules`] 對 Write／Edit 的同一批規則一致。
    pub fn gate_file_hit(&self, p: &str) -> R<Option<String>> {
        if let Some(hit) = self.under_forbidden(p)? {
            return Ok(Some(format!("{hit}（禁止路徑）")));
        }
        if let Some(pt) = self.protected_target(p, true)? {
            return Ok(Some(pt));
        }
        if self.team_config_path(p)? {
            return Ok(Some(".claude/teams/、.claude/workflows/、.wf/ 底下的檔、methodology.json 或 guard.config.json".into()));
        }
        if self.ci_workflow_path(p)? {
            return Ok(Some(".github/workflows/ 底下的檔".into()));
        }
        if super::guard::kg_path(p) {
            return Ok(Some("知識圖的檔（graph.db 與 vault）".into()));
        }
        if let Some(repo) = self.kg_repo_hit(p)? {
            return Ok(Some(format!("{repo} 底下的檔（knowledge.repos 列的 repo 對 team agent 唯讀）")));
        }
        let runs = self.runs.as_str();
        let frel = fold(self.rel_to_workspace(p)?.as_deref().unwrap_or(""));
        let pname = path::name(p);
        let fname = fold(&pname);
        let in_runs = !runs.is_empty() && frel.starts_with(&fold(runs));
        if fname == "run.json" && (runs.is_empty() || in_runs) {
            return Ok(Some("run.json".into()));
        }
        let state = fname.strip_suffix(".json").is_some_and(|s| self.run_stages.iter().any(|x| x == s) || PACK_STATE.contains(&s));
        if in_runs && state {
            return Ok(Some(format!("{pname}（閘門資料）")));
        }
        if (in_runs && SHARED_CONTEXT_NAMES.contains(&fname.as_str())) || fname == fold("執行紀錄索引.md") {
            return Ok(Some(format!("{pname}（共用背景或閘門核對用的索引）")));
        }
        if in_runs && format!("/{frel}").contains("/archive/") {
            return Ok(Some("run 目錄的 archive/".into()));
        }
        Ok(None)
    }

    fn spec_doc_match(&self, rel: &str) -> bool {
        let repo = if self.repos.is_empty() { String::new() } else { format!("{}[^/]+/", regex::escape(&self.repos)) };
        Re::dynamic(format!(r"^{repo}(?:\.worktrees/[^/]+/)?specs/[^/]+/[^/]+\.md$"), "").is_match(rel)
    }

    /// Edit／Write／NotebookEdit 的寫入範圍；Read／Grep 只看機敏檔（讀取預算由呼叫端接著做）。
    pub fn check_paths_rules(&self, data: &Value) -> R<()> {
        let ti = tool_input(data)?;
        let mut fp = String::new();
        for key in ["file_path", "notebook_path"] {
            match ti.get(key) {
                Some(Value::String(s)) if !s.is_empty() => {
                    fp = s.clone();
                    break;
                }
                Some(v) if pyfmt::truthy(v) => return err("AttributeError", &format!("{key} 不是字串")),
                _ => {}
            }
        }
        if fp.is_empty() {
            return Ok(());
        }
        let cfg = &self.cfg;
        let am = &self.am;
        let runs = self.runs.as_str();
        let repos = self.repos.as_str();
        let raw_agent = str_or_empty(data, "agent_type")?;
        let agent = config::role_name(&raw_agent).to_string();
        let role = config::team_role(&raw_agent, cfg); // 模版角色才套寫入範圍
        let has_role = !role.is_empty();
        let mut p = path::pp(&norm(&fp));
        if !path::is_abs(&p) {
            let base = match data.get("cwd") {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                Some(v) if pyfmt::truthy(v) => return err("TypeError", "cwd 不是字串"),
                _ => self.ws.clone(),
            };
            p = path::join(&base, &p);
        }
        let mut rel = self.rel_to_workspace(&p)?;
        if let Some(hit) = self.under_forbidden(&p)? {
            return Err(self.block(&format!("不動 {hit}（禁止路徑，只能讀）")));
        }
        let prot = self.protected_target(&p, true)?;
        let pname = path::name(&p);
        let shown = |rel: &Option<String>| match rel {
            Some(r) if !r.is_empty() => r.clone(),
            _ => fp.clone(),
        };
        if let Some(pt) = &prot {
            if !agent.is_empty() {
                // 唯一例外：freeze_writers（完整 agent_type）可以用 Write 寫入 {"frozen": true, …}
                if !(*pt == am.freeze_file && self.writes_freeze(data)?) {
                    let fw = if am.freeze_writers.is_empty() { "freeze_writers".to_string() } else { am.freeze_writers.join("、") };
                    return Err(self.block(&format!("{agent} 不能寫 {pt}（守門設定、稽核紀錄與凍結檔只由人或確定性工具寫；凍結檔只有 \
{fw} 能用 Write 寫入 frozen=true；auto_merge 設定改走 PR＋CODEOWNERS）")));
                }
            } else if cfg.configured && *pt == am.freeze_file && !self.write_is_freeze(data)? {
                return Err(self.block(&format!("主 session 不解凍自動合併（刪除凍結檔、寫 frozen=false 或用 Edit 改）；解凍由人自己做（手動編輯 {pt}）")));
            } else if cfg.configured && (AUDIT_FILES.contains(&pt.as_str()) || (pt == "guard.config.json" && am.enabled)) {
                return Err(self.block(&format!("主 session 不直接改 {pt}（稽核紀錄只由 awf merge-gate、awf publish 寫；auto_merge 啟用時 guard.config.json 改走 PR＋CODEOWNERS）")));
            }
        }
        let frel = fold(rel.as_deref().unwrap_or(""));
        let fname = fold(&pname);
        let in_runs_fold = !runs.is_empty() && frel.starts_with(&fold(runs));
        if has_role && self.team_config_path(&p)? {
            return Err(self.block(&format!("{agent} 不能寫 .claude/teams/、.claude/workflows/、run 目錄的 .wf/ 底下的檔、methodology.json 或 guard.config.json（不論在哪個目錄；\
守門設定、profile 與 workflow 腳本只由人或主 session 改）（{}）", shown(&rel))));
        }
        if has_role && self.ci_workflow_path(&p)? {
            return Err(self.block(&format!("{agent} 不能寫 .github/workflows/ 底下的檔（不論在哪個 repo；CI 檔決定哪些檢查會跑，只由人或主 session 改，\
例如 awf ci render --write；需要改 CI 就寫進交件的 open_questions）（{}）", shown(&rel))));
        }
        if has_role && super::guard::kg_path(&p) {
            return Err(self.block(&format!("{agent} 不能寫知識圖的檔（graph.db 與 vault，含 vault/notes/；圖只由主 session 跑 awf kg build／export 產生，筆記是人寫的）（{}）", shown(&rel))));
        }
        if has_role {
            if let Some(repo) = self.kg_repo_hit(&p)? {
                return Err(self.block(&format!("{agent} 不能寫 {repo} 底下的檔（profile 的 knowledge.repos 列的 repo 對 team agent 唯讀：可以讀、不能寫，\
同時登記在 linked_repos 也一樣；要改它由人或主 session 做，或由人把它從 knowledge.repos 拿掉）（{}）", shown(&rel))));
            }
        }
        if has_role && fname == "run.json" && (runs.is_empty() || in_runs_fold) {
            return Err(self.block(&format!("{agent} 不能寫 run.json（run 狀態只由 awf save-run 在主 session 寫）")));
        }
        let is_state_file = fname == "run.json"
            || fname.strip_suffix(".json").is_some_and(|s| self.run_stages.iter().any(|x| x == s) || PACK_STATE.contains(&s));
        if has_role && in_runs_fold && is_state_file {
            return Err(self.block(&format!("{agent} 不能寫 {pname}（{runs} 底下的 run.json、<段>.json 是 awf save-run 寫的閘門資料，awf publish 與政策核准會重讀）")));
        }
        if has_role && ((in_runs_fold && SHARED_CONTEXT_NAMES.contains(&fname.as_str())) || fname == fold("執行紀錄索引.md")) {
            return Err(self.block(&format!("{agent} 不能寫 {pname}（runs_dir 底下的 context.md、pack.md、決策記憶、結案摘要是所有 agent 共用的背景、執行紀錄索引.md 是閘門核對用的索引，\
都只由主 session 跑 awf save-run／awf pack 產生）")));
        }
        if has_role && in_runs_fold && format!("/{}", frel.replace('\\', "/")).contains("/archive/") {
            return Err(self.block(&format!("{agent} 不能寫 run 目錄的 archive/（結案摘要只由主 session 跑 awf save-run 產生，會收進知識層）")));
        }
        let parent = path::parent(&p);
        if has_role && self.in_scratchpad(&p, data)? {
            // 這個 session 的 scratchpad：模版角色都可以寫（中間產物）。上面的規則已經判過
            return self.forward(git_toplevel(&parent), "guard-paths", data);
        }
        if has_role && !self.cfg.broken.is_empty() {
            // 設定讀不懂：寫入範圍、禁止路徑、linked_repos 都不知道，模版角色的寫入先全部擋（讀與 scratchpad 照常）
            return Err(self.block(&format!("{agent} 的寫入先全部擋下：守門設定讀不懂時，不知道哪些路徑可以寫（{}）", shown(&rel))));
        }
        let lrel = if has_role { self.linked_rel(&p)? } else { None };
        if config::RECORD_ONLY_ROLES.contains(&role.as_str()) {
            // 只寫執行紀錄的角色：只能寫 <runs_dir>/<run>/<段>/<檔名>.md（白名單，照原樣比對；至少兩層子目錄，上一層是段名）
            let sub: Vec<&str> = match &rel {
                Some(r) if !runs.is_empty() && r.starts_with(runs) => r[runs.len()..].split('/').collect(),
                _ => Vec::new(),
            };
            let ok = sub.len() >= 3 && sub.iter().all(|s| !s.is_empty())
                && self.run_stages.iter().any(|x| x == sub[sub.len() - 2])
                && pname.ends_with(".md")
                && !RECORD_RESERVED_NAMES.iter().any(|n| fold(n) == fname);
            if !ok {
                let runs_label = if runs.is_empty() { "run 目錄" } else { runs };
                return Err(self.block(&format!("{agent} 只能寫執行紀錄：{runs_label}<run>/<段>/<label>.md（段：{}；\
不能是 context.md、執行紀錄索引.md）（{}）；run.json、merge-audit.jsonl、凍結檔與其他閘門資料不能寫", self.run_stages.join("、"), shown(&rel))));
            }
        } else if role == "tpm-writer" {
            // 凍結檔照 auto_merge.freeze_file 的實際設定；只認設定的那個路徑本身（同名的其他檔不算）
            let is_freeze = prot.as_deref() == Some(am.freeze_file.as_str())
                && path::resolve(&p)? == path::resolve(&self.freeze_path())?
                && self.writes_freeze(data)?;
            let ok = is_freeze
                || rel.as_deref().is_some_and(|r| cfg.writer_dirs.iter().any(|d| r.starts_with(d.as_str())) || self.spec_doc_match(r))
                || lrel.as_deref().is_some_and(|l| LINKED_SPEC_DOC.is_match(l));
            if !ok {
                return Err(self.block(&format!("tpm-writer 只能寫 {} 與 repo 的 specs/<epic>/*.md（{}）", cfg.writer_dirs.join("、"), shown(&rel))));
            }
        } else if role.starts_with("tpm-") {
            if !(rel.as_deref().is_some_and(|r| !runs.is_empty() && r.starts_with(runs))) {
                let runs_label = if runs.is_empty() { "run 目錄" } else { runs };
                return Err(self.block(&format!("{agent} 只能寫 {runs_label}（{}）", shown(&rel))));
            }
        } else if role.starts_with("dev-") {
            // .claude/ 是黑名單（不分大小寫）；repos_dir、runs_dir 是白名單，照原樣比對
            let ok = rel.as_deref().is_some_and(|r| {
                (if repos.is_empty() { !fold(r).starts_with(".claude/") } else { r.starts_with(repos) }) || (!runs.is_empty() && r.starts_with(runs))
            });
            let linked = !ok && lrel.as_deref().is_some_and(|l| !fold(l).starts_with(".claude/"));
            if !ok && !linked {
                let place = if repos.is_empty() { "工作區（.claude/ 除外）".to_string() } else { format!("{repos}<repo>/") };
                return Err(self.block(&format!("{agent} 只能改 {place} 與 {runs}（{}）；工作區以外的 repo 要先由人登記在 guard.config.json 的 linked_repos", shown(&rel))));
            }
            if linked {
                rel = lrel.clone(); // 之後的規則照相對於那個 repo 的路徑判
            }
            let r = rel.clone().unwrap_or_default();
            let in_runs = !linked && !runs.is_empty() && r.starts_with(runs);
            if let Some((pat, src)) = review_report(&role) {
                if !in_runs && !pat.is_match(&pname) && !(role != "dev-verifier" && format!("/{r}").contains("/evidence/")) {
                    let extra = if role != "dev-verifier" { "與 specs/<epic>/evidence/" } else { "" };
                    return Err(self.block(&format!("{agent} 是唯讀審查角色，只能寫自己的報告（{src}）{extra}及 {runs}，不能改產品程式或測試（{r}）")));
                }
            }
            if role == "dev-implementer" {
                for t in &cfg.acceptance_test_dirs {
                    let t = t.trim_matches('/');
                    if !t.is_empty() && fold(&format!("/{r}")).contains(&fold(&format!("/{t}/"))) {
                        return Err(self.block(&format!("{agent} 不能改驗收測試（{t}/；acceptance_test_dirs），驗收測試由 dev-acceptance-tester 寫（{r}）")));
                    }
                }
            }
        }
        self.forward(git_toplevel(&parent), "guard-paths", data)
    }
}
