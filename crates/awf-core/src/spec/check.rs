//! validate：讀取時發現的問題，加上「差異 對 現行規格」的交叉檢查。

use super::ids::IdReport;
use super::read::{clean, prob, ChangeState, Problem, Tree};
use std::collections::HashSet;

/// 進行中的變更對現行規格的檢查：指到的需求存不存在、ADDED 有沒有撞名、缺不缺檔。
pub fn cross_check(tree: &Tree) -> Vec<Problem> {
    let mut out = Vec::new();
    for ch in tree.changes.iter().filter(|c| c.state == ChangeState::Active) {
        let scope = format!("change:{}", ch.name);
        if !ch.has_proposal {
            out.push(prob(&scope, "missing-file", &format!("{}/proposal.md", ch.dir), 1, "變更缺 proposal.md".into()));
        }
        if !ch.has_tasks {
            out.push(prob(&scope, "missing-file", &format!("{}/tasks.md", ch.dir), 1, "變更缺 tasks.md".into()));
        }
        for d in &ch.deltas {
            let target = format!("specs/{}/spec.md", d.capability);
            let current: HashSet<&str> = tree.capability(&d.capability).map(|c| c.requirements.iter().map(|r| r.title.as_str()).collect()).unwrap_or_default();
            let mut renamed_to: HashSet<&str> = HashSet::new();
            for r in &d.renamed {
                if !current.contains(r.from.as_str()) {
                    out.push(prob(&scope, "missing-target", &d.file, r.line, format!("RENAMED 的 FROM「{}」在現行規格 {target} 找不到", clean(&r.from))));
                } else {
                    renamed_to.insert(r.to.as_str());
                }
                if r.from != r.to && current.contains(r.to.as_str()) {
                    out.push(prob(&scope, "already-exists", &d.file, r.line, format!("RENAMED 的 TO「{}」在現行規格 {target} 已經存在", clean(&r.to))));
                }
            }
            for r in &d.modified {
                if !current.contains(r.title.as_str()) && !renamed_to.contains(r.title.as_str()) {
                    out.push(prob(&scope, "missing-target", &d.file, r.span.start, format!("MODIFIED 指到的需求「{}」在現行規格 {target} 找不到", clean(&r.title))));
                }
            }
            for r in &d.removed {
                if !current.contains(r.title.as_str()) {
                    out.push(prob(&scope, "missing-target", &d.file, r.span.start, format!("REMOVED 指到的需求「{}」在現行規格 {target} 找不到", clean(&r.title))));
                }
            }
            for r in &d.added {
                if current.contains(r.title.as_str()) {
                    out.push(prob(&scope, "already-exists", &d.file, r.span.start, format!("ADDED 的需求「{}」在現行規格 {target} 已經存在（要改內容請用 MODIFIED）", clean(&r.title))));
                }
            }
        }
    }
    out
}

/// 全部的問題（排序、去重）。給 `change` 時只留那個變更與它動到的能力的問題。
pub fn validate(tree: &Tree, ids: &IdReport, change: Option<&str>) -> Vec<Problem> {
    let mut all: Vec<Problem> = tree.problems.iter().cloned().chain(cross_check(tree)).chain(ids.problems.iter().cloned()).collect();
    if let Some(name) = change {
        let scope = format!("change:{name}");
        let caps: HashSet<String> = tree.change(name, ChangeState::Active).map(|c| c.deltas.iter().map(|d| format!("spec:{}", d.capability)).collect()).unwrap_or_default();
        all.retain(|p| p.scope == scope || caps.contains(&p.scope));
    }
    all.sort();
    all.dedup();
    all
}

/// `路徑:行號：說明`
pub fn format_problem(display: &str, p: &Problem) -> String {
    format!("{display}/{}:{}：{}", p.file, p.line, p.message)
}
