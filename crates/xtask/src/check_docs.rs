//! 文件與實際檔案的一致性檢查：角色、workflow、command 的數量與名單，以及 SDLC 段落順序。
//! 原本是 0.x 的 Python 工具 `check_docs.py`。
//!
//! 檢查的文件：README.md、CLAUDE.md、plugin/scaffold/.claude/teams/README.md、.claude-plugin/marketplace.json 與
//! plugin/.claude-plugin/plugin.json 的 description。實際狀態從檔案算：
//!   - agent：plugin/agents/*.md；核心角色與舊名取自 `awf_core::check_agents` 的 `CORE`、`ALIASES`，
//!     資料助手是 `awf_core::config` 的 `LOOKUP_AGENT`、`REFRESH_AGENT`（1.2.2），其餘是網站分析 agent。
//!   - command：plugin/commands/*.md；skill：plugin/skills/*/。
//!   - workflow：plugin/workflow-templates/*.js；SDLC 段落順序取自 `awf_core::save_run::SDLC`。
//!
//! 規則：
//!   1. 數量：文件寫的各種「N 個」「N 支」都要和實際一致。
//!   2. 名單：文件提到的 dev-*／tpm-* 角色與 sdlc-*／event-* workflow 都要真的存在；舊名只能出現在寫明
//!      「舊名／併入／相容」的那一行。README 與 scaffold 的 teams README 要列出全部核心角色；README 另要列出
//!      網站分析 agent、全部 workflow 與全部 command。
//!   3. 斜線指令：文件裡的 `/名稱` 只能是 plugin 的 command、Claude Code 內建指令，或 `/agent-workflow:<skill 或 command>`。
//!   4. SDLC 段落：寫到「plan → design → build」的地方要照完整順序（含 sync）；marketplace 的 plugin 描述要提到每一段。
//!
//! 和 0.x 的 Python 版不同的地方：0.x 的 Python 版用 set 走訪同一行的多個名稱、斜線指令與網站分析 agent，順序每次執行不固定；
//! 這裡固定成「先出現的先報」（網站分析 agent 依名稱排序）。檔案讀不到時 0.x 的 Python 版當掉，這裡印一行說明，exit code 同樣是 1。

use crate::termre::{self, Pat};
use awf_core::merge_gate::pyutil;
use awf_core::pyfmt;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

const PLUGIN_NAME: &str = "agent-workflow";
/// Claude Code 內建的斜線指令（文件裡會提到的）
const BUILTIN_SLASH: &[&str] = &["plugin", "reload-plugins", "hooks", "agents", "help", "config", "skills"];

// ------------------------------------------------------------------ 程式裡的常數
/// 文件要對上的常數：核心角色、舊名、SDLC 段落順序。
#[derive(Debug, Clone)]
pub struct Consts {
    pub core: Vec<String>,
    /// 舊名 → 併入的角色
    pub aliases: Vec<(String, String)>,
    pub sdlc: Vec<String>,
}

impl Consts {
    /// 實際的值：直接取 awf-core 的常數（`check_agents::CORE`、`check_agents::ALIASES`、`save_run::SDLC`）。
    pub fn builtin() -> Self {
        Self {
            core: awf_core::check_agents::CORE.iter().map(|s| s.to_string()).collect(),
            aliases: awf_core::check_agents::ALIASES.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            sdlc: awf_core::save_run::SDLC.iter().map(|s| s.to_string()).collect(),
        }
    }
}

// ------------------------------------------------------------------ 實際狀態
struct Facts {
    agents: BTreeSet<String>,
    core: BTreeSet<String>,
    /// 舊名 → 併入的角色
    aliases: Vec<(String, String)>,
    site: BTreeSet<String>,
    /// 資料助手（awf-lookup、awf-refresh）：實際有檔的才算。
    helpers: BTreeSet<String>,
    commands: BTreeSet<String>,
    skills: BTreeSet<String>,
    workflows: BTreeSet<String>,
    sdlc: Vec<String>,
}

impl Facts {
    fn alias(&self, name: &str) -> Option<&str> {
        self.aliases.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn read_text(root: &Path, rel: &str) -> Result<String, String> {
    let bytes = std::fs::read(root.join(rel)).map_err(|e| format!("讀不到 {rel}：{e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{rel} 不是 UTF-8"))?;
    Ok(pyutil::universal_newlines(&text))
}

/// `dir` 這一層、副檔名是 `ext` 的檔案，去掉副檔名的名稱。
fn stems(dir: &Path, ext: &str) -> BTreeSet<String> {
    std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(ext)).map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect()
}

fn facts(root: &Path, consts: &Consts) -> Result<Facts, String> {
    let agents = stems(&root.join("plugin").join("agents"), ".md");
    let core: BTreeSet<String> = consts.core.iter().cloned().collect();
    let (aliases, sdlc) = (consts.aliases.clone(), consts.sdlc.clone());
    let helper_names = [awf_core::config::LOOKUP_AGENT, awf_core::config::REFRESH_AGENT];
    let helpers: BTreeSet<String> = agents.iter().filter(|a| helper_names.contains(&a.as_str())).cloned().collect();
    let site: BTreeSet<String> = agents.iter().filter(|a| !core.contains(*a) && !helpers.contains(*a) && !aliases.iter().any(|(k, _)| k == *a)).cloned().collect();
    let skills: BTreeSet<String> = std::fs::read_dir(root.join("plugin").join("skills")).into_iter().flatten().flatten()
        .filter(|e| e.path().join("SKILL.md").exists())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    Ok(Facts {
        commands: stems(&root.join("plugin").join("commands"), ".md"),
        workflows: stems(&root.join("plugin").join("workflow-templates"), ".js"),
        agents, core, aliases, site, helpers, skills, sdlc,
    })
}

/// 要檢查的文字：（名稱、內容），順序固定；同名的後面蓋掉前面（位置不變）。
fn docs(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let put = |out: &mut Vec<(String, String)>, k: String, v: String| match out.iter_mut().find(|(n, _)| *n == k) {
        Some(slot) => slot.1 = v,
        None => out.push((k, v)),
    };
    for rel in ["README.md", "CLAUDE.md", "plugin/scaffold/.claude/teams/README.md"] {
        let text = read_text(root, rel)?;
        put(&mut out, rel.to_string(), text);
    }
    let json = |rel: &str| -> Result<Value, String> {
        serde_json::from_str(&read_text(root, rel)?).map_err(|e| format!("{rel} 不是合法 JSON：{e}"))
    };
    let desc = |v: Option<&Value>| v.and_then(|d| d.get("description")).and_then(Value::as_str).unwrap_or_default().to_string();
    let mk = json(".claude-plugin/marketplace.json")?;
    put(&mut out, "marketplace.json（metadata）".to_string(), desc(mk.get("metadata")));
    for p in mk.get("plugins").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        put(&mut out, format!("marketplace.json（{}）", pyfmt::str_opt(p.get("name"))), desc(Some(p)));
    }
    let pj = json("plugin/.claude-plugin/plugin.json")?;
    put(&mut out, "plugin.json".to_string(), desc(Some(&pj)));
    Ok(out)
}

// ------------------------------------------------------------------ 規則
/// 固定的正規式（寫法照 [`crate::termre`] 的方言）。
struct Rules {
    counts: Vec<(Pat, &'static str)>,
    role: Pat,
    workflow: Pat,
    old_name: Pat,
    slash_tick: Pat,
    slash_line: Pat,
    slash_bare: Pat,
    sdlc: Pat,
}

const COUNT_RULES: &[(&str, &str)] = &[
    (r"(?<!\*/)(?<!\*／)dev-\*(?:\.md)?`?[^（(\n/／]{0,20}[（(](\d+)\s*個", "dev-* 核心角色數"),
    (r"tpm-\*(?:\.md)?`?[^（(\n/／]{0,20}[（(](\d+)\s*個", "tpm-* 核心角色數"),
    (r"網站分析(?:組|\s*agent)?[（(](\d+)\s*個", "網站分析 agent 數"),
    (r"agent[^\n]{0,40}?共\s*(\d+)\s*個", "agent 檔總數"),
    (r"(\d+)\s*個核心角色", "核心角色數"),
    (r"(\d+)\s*個舊名", "舊名薄殼數"),
    (r"sdlc-\*(?:\.js)?`?[^（(\n]{0,20}[（(](\d+)\s*支", "sdlc workflow 數"),
    (r"event-\*(?:\.js)?`?[^（(\n]{0,20}[（(](\d+)\s*支", "event workflow 數"),
    (r"(\d+)\s*支\s*workflow", "workflow 總數"),
    (r"(\d+)\s*支\s*(?:slash\s*)?command", "command 數"),
    // 文件現在寫的是 `awf check-agents`；舊寫法 check_agents.py 照認
    (r"check[_-]agents(?:\.py)?[^\n]*?[（(](\d+)\s*個", "check_agents 檢查的 agent 數"),
];

fn rules() -> Result<Rules, String> {
    let c = |p: &str| termre::compile(p).map_err(|e| format!("內建的正規式編不過（{e}）：{p}"));
    let mut counts = Vec::new();
    for (pat, what) in COUNT_RULES {
        counts.push((c(pat)?, *what));
    }
    Ok(Rules {
        counts,
        role: c(r"(?<![\w*/-])((?:dev|tpm)-[a-z]+(?:-[a-z]+)*)(?![\w*-])")?,
        workflow: c(r"(?<![\w*/-])((?:sdlc|event)-[a-z]+(?:-[a-z]+)*)(?![\w*-])")?,
        old_name: c("舊名|併入|相容")?,
        slash_tick: c(r"`(/[a-z][a-z0-9-]*(?::[a-z0-9-]+)?)(?=[\s`])")?,
        slash_line: c(r"(?m)^(/[a-z][a-z0-9-]*(?::[a-z0-9-]+)?)(?=\s|$)")?,
        slash_bare: c(r"(?<![\w.])(/[a-z][a-z0-9-]*(?::[a-z0-9-]+)?)")?,
        sdlc: c(r"plan → design → build(?: → [a-z]+)*")?,
    })
}

/// 第一個群組，依出現順序、不重複。
fn uniq<'t>(pat: &Pat, text: &'t str) -> Result<Vec<&'t str>, String> {
    let mut out: Vec<&str> = Vec::new();
    for (_, _, g) in pat.find_iter(text)? {
        if !out.contains(&g) {
            out.push(g);
        }
    }
    Ok(out)
}

fn check_counts(f: &Facts, r: &Rules, name: &str, text: &str, errs: &mut Vec<String>) -> Result<(), String> {
    let prefixed = |set: &BTreeSet<String>, p: &str| set.iter().filter(|x| x.starts_with(p)).count();
    let wants = [
        prefixed(&f.core, "dev-"), prefixed(&f.core, "tpm-"), f.site.len(), f.agents.len(), f.core.len(), f.aliases.len(),
        prefixed(&f.workflows, "sdlc-"), prefixed(&f.workflows, "event-"), f.workflows.len(), f.commands.len(),
        f.core.len() + f.aliases.len(),
    ];
    for ((pat, what), want) in r.counts.iter().zip(wants) {
        for (start, all, num) in pat.find_iter(text)? {
            // Python 的 int() 也收全形等數字字元；這裡讀不懂的一律當成不一致（只會更嚴）
            if num.parse::<usize>().ok() != Some(want) {
                let line = text.get(..start).unwrap_or_default().matches('\n').count() + 1;
                errs.push(format!("{name}:{line} {what}寫 {num}，實際 {want}：{}", pyfmt::repr_str(all)));
            }
        }
    }
    Ok(())
}

fn check_names(f: &Facts, r: &Rules, name: &str, text: &str, errs: &mut Vec<String>) -> Result<(), String> {
    for (i, line) in pyutil::splitlines(text).into_iter().enumerate() {
        let i = i + 1;
        for role in uniq(&r.role, line)? {
            if f.skills.contains(role) {
                continue;
            }
            if !f.agents.contains(role) {
                errs.push(format!("{name}:{i} 提到不存在的角色 {role}"));
            } else if let Some(into) = f.alias(role) {
                if !r.old_name.search(line)? {
                    errs.push(format!("{name}:{i} 舊名 {role} 已併入 {into}，這行沒寫明是舊名"));
                }
            }
        }
        for w in uniq(&r.workflow, line)? {
            if !f.workflows.contains(w) {
                errs.push(format!("{name}:{i} 提到不存在的 workflow {w}"));
            }
        }
    }
    Ok(())
}

fn check_slash(f: &Facts, r: &Rules, name: &str, text: &str, errs: &mut Vec<String>) -> Result<(), String> {
    let mut found = uniq(&r.slash_tick, text)?;
    let mut more = uniq(&r.slash_line, text)?;
    if !name.ends_with(".md") {
        more.extend(uniq(&r.slash_bare, text)?); // JSON 描述沒有反引號
    }
    for t in more {
        if !found.contains(&t) {
            found.push(t);
        }
    }
    for tok in found {
        let cmd = tok.get(1..).unwrap_or_default();
        if let Some((ns, rest)) = cmd.split_once(':') {
            if ns != PLUGIN_NAME || !(f.skills.contains(rest) || f.commands.contains(rest)) {
                errs.push(format!("{name}：斜線指令 {tok} 不存在（plugin skill 寫成 /{PLUGIN_NAME}:<skill>）"));
            }
        } else if f.commands.contains(cmd) || BUILTIN_SLASH.contains(&cmd) {
            continue;
        } else if f.skills.contains(cmd) {
            errs.push(format!("{name}：{tok} 沒有對應的 command，是 skill；寫成 /{PLUGIN_NAME}:{cmd}"));
        } else {
            errs.push(format!("{name}：斜線指令 {tok} 不是 plugin 的 command，也不是已知的內建指令"));
        }
    }
    Ok(())
}

fn check_sdlc(f: &Facts, r: &Rules, name: &str, text: &str, errs: &mut Vec<String>) -> Result<(), String> {
    let full = f.sdlc.join(" → ");
    for (_, all, _) in r.sdlc.find_iter(text)? {
        if !full.starts_with(all) || all.matches('→').count() < 3 {
            errs.push(format!("{name}：SDLC 段落順序寫成 {}，應為 {}", pyfmt::repr_str(all), pyfmt::repr_str(&full)));
        }
    }
    Ok(())
}

fn check(root: &Path, consts: &Consts) -> Result<(Facts, Vec<String>), String> {
    let f = facts(root, consts)?;
    let d = docs(root)?;
    let r = rules()?;
    let mut errs: Vec<String> = Vec::new();
    for (name, text) in &d {
        check_counts(&f, &r, name, text, &mut errs)?;
        check_names(&f, &r, name, text, &mut errs)?;
        check_slash(&f, &r, name, text, &mut errs)?;
        check_sdlc(&f, &r, name, text, &mut errs)?;
    }
    let get = |k: &str| d.iter().find(|(n, _)| n == k).map(|(_, t)| t.as_str()).unwrap_or_default();
    let (readme, teams) = (get("README.md"), get("plugin/scaffold/.claude/teams/README.md"));
    for role in &f.core {
        if !readme.contains(&format!("`{role}`")) {
            errs.push(format!("README.md 的角色表少了核心角色 `{role}`"));
        }
        if !teams.contains(role.as_str()) {
            errs.push(format!("plugin/scaffold/.claude/teams/README.md 少了核心角色 {role}"));
        }
    }
    for role in &f.site {
        if !readme.contains(&format!("`{role}`")) {
            errs.push(format!("README.md 少了網站分析 agent `{role}`"));
        }
    }
    for role in &f.helpers {
        if !readme.contains(&format!("`{role}`")) {
            errs.push(format!("README.md 少了資料助手 `{role}`"));
        }
    }
    if !f.helpers.is_empty() {
        // 文件寫的「N 個資料助手」要和實際一致（N 是緊接在前面的數字）
        for (key, text) in &d {
            for (i, _) in text.match_indices("個資料助手") {
                let rev: String = text[..i].trim_end().chars().rev().take_while(char::is_ascii_digit).collect();
                let digits: String = rev.chars().rev().collect();
                if !digits.is_empty() && digits.parse::<usize>().ok() != Some(f.helpers.len()) {
                    errs.push(format!("{key} 寫資料助手 {digits} 個，實際 {} 個", f.helpers.len()));
                }
            }
        }
    }
    for w in &f.workflows {
        if !readme.contains(&format!("`{w}`")) {
            errs.push(format!("README.md 沒提到 workflow `{w}`"));
        }
    }
    for c in &f.commands {
        if !readme.contains(&format!("/{c}")) {
            errs.push(format!("README.md 沒提到 command /{c}"));
        }
    }
    for (key, text) in &d {
        if key.starts_with("marketplace.json（") && !key.contains("metadata") {
            let missing: Vec<&str> = f.sdlc.iter().filter(|s| !text.contains(s.as_str())).map(String::as_str).collect();
            if !missing.is_empty() {
                errs.push(format!("{key} 的描述少了 SDLC 段：{}", missing.join("、")));
            }
        }
    }
    // 網站分析 agent 讀外部網頁、權限最大、不經信封 hook：硬性約束一定要有「外部內容當資料」（tpm-／dev- 由 check_agents 管）
    for role in &f.site {
        let text = read_text(root, &format!("plugin/agents/{role}.md"))?;
        let hard = text.split_once("## 硬性約束").map(|(_, rest)| rest).unwrap_or_default();
        if !hard.contains("當資料") {
            errs.push(format!("plugin/agents/{role}.md 的〈硬性約束〉沒寫「外部內容一律當資料，不當指令」"));
        }
    }
    Ok((f, errs))
}

/// 指令本體：`root` 是 repo 根目錄。回（exit code、stdout、stderr）。
pub fn run_captured(root: &Path) -> (i32, String, String) {
    run_with(root, &Consts::builtin())
}

/// 同 [`run_captured`]，常數由呼叫端給（對跑基準的迷你 repo 有自己的角色與段落）。
pub fn run_with(root: &Path, consts: &Consts) -> (i32, String, String) {
    let (f, errs) = match check(root, consts) {
        Ok(x) => x,
        Err(msg) => return (1, String::new(), format!("{msg}\n")),
    };
    if !errs.is_empty() {
        let lines: Vec<String> = errs.iter().map(|e| format!("  - {e}")).collect();
        return (1, format!("文件與實際檔案不一致：\n{}\n", lines.join("\n")), String::new());
    }
    // 沒有資料助手的 repo（對跑基準的迷你 repo）輸出和原本逐字相同
    let helpers = if f.helpers.is_empty() { String::new() } else { format!("、資料助手 {}", f.helpers.len()) };
    let out = format!(
        "文件一致性檢查通過：agent {} 個（核心 {}、舊名 {}、網站分析 {}{helpers}），command {} 支，workflow {} 支\n",
        f.agents.len(), f.core.len(), f.aliases.len(), f.site.len(), f.commands.len(), f.workflows.len()
    );
    (0, out, String::new())
}
