//! apply：把變更的差異併回現行規格，再把變更目錄搬進 `changes/archive/`。
//!
//! 分兩步：`plan` 全部算好（不碰磁碟上的任何檔），`commit` 才寫。
//! `commit` 先把新內容寫到暫存檔，再依序 rename；任何一步失敗都把已經換掉的還原。

use super::diff::unified;
use super::read::{clean, last_nonblank, raw_delta, raw_reqs, read_text, rel_path, scan, split_lines, ChangeState, DeltaKind, RawReq, Tree};
use std::fs;
use std::path::{Path, PathBuf};

const TMP_SUFFIX: &str = ".awf-new";
const BAK_SUFFIX: &str = ".awf-old";

/// 一個要寫的規格檔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    /// 相對於規格根目錄的路徑。
    pub file: String,
    /// 現在的內容；`None` 代表新檔。
    pub old: Option<String>,
    pub new: String,
}

/// apply 的計畫。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub change: String,
    pub writes: Vec<FileWrite>,
    /// 變更目錄現在的位置與歸檔後的位置（相對於規格根目錄）。
    pub from: String,
    pub to: String,
}

fn find(lines: &[String], title: &str) -> Result<RawReq, String> {
    let sc = scan(lines);
    raw_reqs(lines.len(), &sc.heads).into_iter().find(|r| r.title == title).ok_or_else(|| format!("現行規格找不到需求「{}」", clean(title)))
}

/// ADDED 要插在哪一行之前：有 `## Requirements` 就是那一節的結尾，沒有就是檔尾（都不含結尾的空白行）。
fn insert_pos(lines: &[String]) -> usize {
    let sc = scan(lines);
    let tops: Vec<_> = sc.heads.iter().filter(|h| h.level <= 2).collect();
    let end = match tops.iter().position(|h| h.level == 2 && h.text.eq_ignore_ascii_case("requirements")) {
        Some(k) => tops.get(k + 1).map_or(lines.len(), |n| n.idx),
        None => lines.len(),
    };
    let mut p = end.min(lines.len());
    while p > 0 && lines.get(p - 1).is_some_and(|l| l.trim().is_empty()) {
        p -= 1;
    }
    p
}

/// 把一份差異檔併進一份規格。順序和 OpenSpec 一樣：RENAMED → REMOVED → MODIFIED → ADDED。
/// `old` 是 `None` 時從 `header`（新檔的檔頭）開始。換行樣式（LF／CRLF）與 BOM 照原檔。
pub fn merge_text(old: Option<&str>, delta_text: &str, header: &[String]) -> Result<String, String> {
    let eol = if old.is_some_and(|o| o.contains("\r\n")) { "\r\n" } else { "\n" };
    let bom = old.is_some_and(|o| o.starts_with('\u{feff}'));
    let mut lines: Vec<String> = match old {
        Some(o) => split_lines(o).into_iter().map(String::from).collect(),
        None => header.to_vec(),
    };
    let dl = split_lines(delta_text);
    let raw = raw_delta(&dl);
    let block = |r: &RawReq| -> Vec<String> { dl.get(r.start..=last_nonblank(&dl, r.start, r.end_excl)).unwrap_or(&[]).iter().map(|l| l.to_string()).collect() };

    for r in &raw.renames {
        let b = find(&lines, &r.from)?;
        if let Some(l) = lines.get_mut(b.start) {
            *l = format!("### Requirement: {}", r.to);
        }
    }
    for (_, r) in raw.items.iter().filter(|(k, _)| *k == DeltaKind::Removed) {
        let b = find(&lines, &r.title)?;
        let end = b.end_excl.min(lines.len());
        if b.start <= end {
            lines.drain(b.start..end);
        }
    }
    for (_, r) in raw.items.iter().filter(|(k, _)| *k == DeltaKind::Modified) {
        let b = find(&lines, &r.title)?;
        let last = last_nonblank(&lines, b.start, b.end_excl);
        if b.start <= last && last < lines.len() {
            lines.splice(b.start..=last, block(r));
        }
    }
    for (_, r) in raw.items.iter().filter(|(k, _)| *k == DeltaKind::Added) {
        if find(&lines, &r.title).is_ok() {
            return Err(format!("需求「{}」已經存在，不能再 ADDED", clean(&r.title)));
        }
        let p = insert_pos(&lines);
        let mut ins = Vec::new();
        if p > 0 {
            ins.push(String::new());
        }
        ins.extend(block(r));
        if lines.get(p).is_some_and(|l| !l.trim().is_empty()) {
            ins.push(String::new());
        }
        lines.splice(p..p, ins);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut out = String::new();
    if bom {
        out.push('\u{feff}');
    }
    for l in &lines {
        out.push_str(l);
        out.push_str(eol);
    }
    Ok(out)
}

/// 新能力的檔頭，照現有規格檔的慣例（`sample`）；沒有現有檔時用 OpenSpec 的標準樣子。
pub fn new_header(sample: Option<&str>, capability: &str, change: &str, purpose: Option<&str>) -> Vec<String> {
    let (mut title, mut suffix, mut has_purpose, mut has_reqs) = (true, true, true, true);
    if let Some(s) = sample {
        let lines = split_lines(s);
        let sc = scan(&lines);
        let h1 = sc.heads.iter().find(|h| h.level == 1);
        title = h1.is_some();
        suffix = h1.is_some_and(|h| h.text.ends_with(" Specification"));
        has_purpose = sc.heads.iter().any(|h| h.level == 2 && h.text.eq_ignore_ascii_case("purpose"));
        has_reqs = sc.heads.iter().any(|h| h.level == 2 && h.text.eq_ignore_ascii_case("requirements"));
    }
    let mut out = Vec::new();
    if title {
        out.push(if suffix { format!("# {capability} Specification") } else { format!("# {capability}") });
    }
    if has_purpose {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push("## Purpose".to_string());
        match purpose {
            Some(p) => out.extend(split_lines(p).into_iter().map(String::from)),
            None => out.push(format!("待補：這個能力由變更 {change} 建立，請補上用途。")),
        }
    }
    if has_reqs {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push("## Requirements".to_string());
    }
    out
}

/// 算出 apply 要做的事。不寫任何檔。呼叫前應該先跑 validate。
pub fn plan(tree: &Tree, change: &str, date: &str) -> Result<Plan, String> {
    let ch = tree.change(change, ChangeState::Active).ok_or_else(|| format!("找不到進行中的變更「{}」", clean(change)))?;
    let to = format!("changes/archive/{date}-{change}");
    if fs::symlink_metadata(rel_path(&tree.root, &to)).is_ok() {
        return Err(format!("歸檔目錄 {to} 已經存在"));
    }
    let sample = match tree.capabilities.first() {
        Some(c) => Some(read_text(&rel_path(&tree.root, &c.file)).map_err(|e| format!("{}：{e}", c.file))?),
        None => None,
    };
    let mut writes = Vec::new();
    for d in &ch.deltas {
        let file = format!("specs/{}/spec.md", d.capability);
        let path = rel_path(&tree.root, &file);
        let old = if tree.capability(&d.capability).is_some() {
            Some(read_text(&path).map_err(|e| format!("{file}：{e}"))?)
        } else if fs::symlink_metadata(&path).is_ok() {
            return Err(format!("{file} 存在但不能讀（可能是指到規格目錄外的符號連結），不覆寫"));
        } else {
            None
        };
        let delta_text = read_text(&rel_path(&tree.root, &d.file)).map_err(|e| format!("{}：{e}", d.file))?;
        let header = new_header(sample.as_deref(), &d.capability, change, d.purpose.as_deref());
        let new = merge_text(old.as_deref(), &delta_text, &header).map_err(|e| format!("{}：{e}", d.file))?;
        if old.as_deref() != Some(new.as_str()) {
            writes.push(FileWrite { file, old, new });
        }
    }
    Ok(Plan { change: change.to_string(), writes, from: ch.dir.clone(), to })
}

/// 計畫的文字：每個檔的 unified diff，加上歸檔的搬移。
pub fn render(plan: &Plan, display: &str) -> String {
    let mut out = String::new();
    for w in &plan.writes {
        out.push_str(&unified(w.old.as_deref(), &w.new, &format!("{display}/{}", w.file)));
    }
    if plan.writes.is_empty() {
        out.push_str("規格檔沒有變動。\n");
    }
    out.push_str(&format!("歸檔：{display}/{} → {display}/{}\n", plan.from, plan.to));
    out
}

enum Undo {
    RemoveFile(PathBuf),
    RemoveDir(PathBuf),
    Restore { bak: PathBuf, target: PathBuf },
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn mkdirs(root: &Path, canon: &Path, rel: &str, undo: &mut Vec<Undo>) -> Result<PathBuf, String> {
    let mut p = root.to_path_buf();
    for c in rel.split('/').filter(|c| !c.is_empty()) {
        p.push(c);
        if fs::symlink_metadata(&p).is_err() {
            fs::create_dir(&p).map_err(|e| format!("建不了目錄 {rel}：{e}"))?;
            undo.push(Undo::RemoveDir(p.clone()));
        }
    }
    let inside = p.is_dir() && fs::canonicalize(&p).map(|c| c.starts_with(canon)).unwrap_or(false);
    if !inside {
        return Err(format!("{rel} 不是規格目錄裡的目錄（可能是指到外面的符號連結），不寫"));
    }
    Ok(p)
}

fn tick(n: &mut usize, fail_at: Option<usize>) -> Result<(), String> {
    *n += 1;
    if fail_at == Some(*n) {
        return Err(format!("第 {n} 個搬移步驟失敗（測試用的模擬失敗）"));
    }
    Ok(())
}

fn run(root: &Path, canon: &Path, plan: &Plan, fail_at: Option<usize>, undo: &mut Vec<Undo>, baks: &mut Vec<PathBuf>) -> Result<(), String> {
    let clean_rel = |p: &str, prefix: &str| p.starts_with(prefix) && !p.contains('\\') && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..");
    if !clean_rel(&plan.from, "changes/") || !clean_rel(&plan.to, "changes/archive/") {
        return Err("歸檔的來源或目的不在規格目錄的 changes/ 底下，不搬".to_string());
    }
    let from = rel_path(root, &plan.from);
    let from_ok = fs::symlink_metadata(&from).is_ok_and(|m| m.is_dir());
    if !from_ok {
        return Err(format!("{} 不是目錄（或是符號連結），不搬", plan.from));
    }
    // 一、全部準備好：建目錄、寫暫存檔
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf, bool)> = Vec::new();
    for w in &plan.writes {
        let (dir_rel, name) = w.file.rsplit_once('/').ok_or_else(|| format!("{} 不是合法的規格檔路徑", w.file))?;
        if !w.file.starts_with("specs/") || w.file.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            return Err(format!("{} 不在規格目錄的 specs/ 底下，不寫", w.file));
        }
        let dir = mkdirs(root, canon, dir_rel, undo)?;
        let target = dir.join(name);
        let meta = fs::symlink_metadata(&target).ok();
        if meta.as_ref().is_some_and(|m| !m.is_file()) {
            return Err(format!("{} 不是一般檔案（可能是符號連結），不寫", w.file));
        }
        if meta.is_some() != w.old.is_some() {
            return Err(format!("{} 在計算之後被別人動過，請重跑", w.file));
        }
        let (tmp, bak) = (with_suffix(&target, TMP_SUFFIX), with_suffix(&target, BAK_SUFFIX));
        if fs::symlink_metadata(&bak).is_ok() {
            return Err(format!("{}{BAK_SUFFIX} 還在（上一次沒有收乾淨），請確認內容後移除再重跑", w.file));
        }
        fs::write(&tmp, w.new.as_bytes()).map_err(|e| format!("寫不了 {} 的暫存檔：{e}", w.file))?;
        undo.push(Undo::RemoveFile(tmp.clone()));
        staged.push((target, tmp, bak, meta.is_some()));
    }
    let (to_dir, to_name) = plan.to.rsplit_once('/').ok_or_else(|| format!("{} 不是合法的歸檔路徑", plan.to))?;
    let to = mkdirs(root, canon, to_dir, undo)?.join(to_name);
    if fs::symlink_metadata(&to).is_ok() {
        return Err(format!("歸檔目錄 {} 已經存在", plan.to));
    }
    // 二、依序換上
    let mut n = 0;
    for (w, (target, tmp, bak, exists)) in plan.writes.iter().zip(&staged) {
        if *exists {
            tick(&mut n, fail_at)?;
            fs::rename(target, bak).map_err(|e| format!("挪不開 {}：{e}", w.file))?;
            undo.push(Undo::Restore { bak: bak.clone(), target: target.clone() });
            baks.push(bak.clone());
        }
        tick(&mut n, fail_at)?;
        fs::rename(tmp, target).map_err(|e| format!("換不了 {}：{e}", w.file))?;
        undo.push(Undo::RemoveFile(target.clone()));
    }
    // 三、歸檔
    tick(&mut n, fail_at)?;
    fs::rename(&from, &to).map_err(|e| format!("搬不了 {} → {}：{e}", plan.from, plan.to))
}

/// 照計畫寫入。失敗時還原成寫入前的樣子並回錯誤。
pub fn commit(root: &Path, plan: &Plan) -> Result<(), String> {
    commit_with(root, plan, None)
}

/// 同 `commit`；`fail_at` 讓第幾個搬移步驟失敗（1 起算），只給測試驗證還原用。
pub fn commit_with(root: &Path, plan: &Plan, fail_at: Option<usize>) -> Result<(), String> {
    let canon = fs::canonicalize(root).map_err(|e| format!("讀不到規格目錄：{e}"))?;
    let mut undo = Vec::new();
    let mut baks = Vec::new();
    match run(root, &canon, plan, fail_at, &mut undo, &mut baks) {
        Ok(()) => {
            for b in baks {
                let _ = fs::remove_file(b);
            }
            Ok(())
        }
        Err(e) => {
            let mut left = Vec::new();
            for u in undo.into_iter().rev() {
                match u {
                    Undo::RemoveFile(p) => {
                        if fs::symlink_metadata(&p).is_ok() && fs::remove_file(&p).is_err() {
                            left.push(p);
                        }
                    }
                    Undo::RemoveDir(p) => {
                        let _ = fs::remove_dir(&p);
                    }
                    Undo::Restore { bak, target } => {
                        if fs::rename(&bak, &target).is_err() {
                            left.push(bak);
                        }
                    }
                }
            }
            if left.is_empty() {
                Err(format!("{e}；已經還原，規格目錄和執行前一樣"))
            } else {
                let names: Vec<String> = left.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
                Err(format!("{e}；還原時有檔案處理不了，請人工檢查：{}", names.join("、")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::read::{load, parse_spec_text};

    const OLD: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n舊的說明。\n\n#### Scenario: 庫存足夠\n- **THEN** 成立\n\n### Requirement: 傳真下單\n說明。\n\n#### Scenario: 收到傳真\n- **THEN** 建單\n\n### Requirement: 取消訂單\n說明。\n\n#### Scenario: 出貨前\n- **THEN** 可以取消\n";

    fn titles(text: &str) -> Vec<String> {
        parse_spec_text(text, "orders", "f").0.requirements.into_iter().map(|r| r.title).collect()
    }

    #[test]
    fn added_goes_to_the_end_of_requirements() {
        let d = "## ADDED Requirements\n\n### Requirement: 退款\n說明。\n\n#### Scenario: 七天內\n- **THEN** 退款\n\n";
        let new = merge_text(Some(OLD), d, &[]).unwrap();
        assert_eq!(new, format!("{OLD}\n### Requirement: 退款\n說明。\n\n#### Scenario: 七天內\n- **THEN** 退款\n"));
        let with_tail = format!("{OLD}\n## 附註\n其他文字\n");
        let new = merge_text(Some(&with_tail), d, &[]).unwrap();
        assert!(new.contains("- **THEN** 可以取消\n\n### Requirement: 退款\n"), "{new}");
        assert!(new.ends_with("- **THEN** 退款\n\n## 附註\n其他文字\n"), "{new}");
    }

    #[test]
    fn modified_replaces_the_whole_block() {
        let d = "## MODIFIED Requirements\n### Requirement: 建立訂單\n新的說明。\n\n#### Scenario: 庫存足夠\n- **THEN** 成立並寄信\n\n#### Scenario: 庫存不足\n- **THEN** 缺貨\n";
        let new = merge_text(Some(OLD), d, &[]).unwrap();
        assert!(new.contains("### Requirement: 建立訂單\n新的說明。\n\n#### Scenario: 庫存足夠\n- **THEN** 成立並寄信\n\n#### Scenario: 庫存不足\n- **THEN** 缺貨\n\n### Requirement: 傳真下單\n"), "{new}");
        assert!(!new.contains("舊的說明"));
        assert_eq!(titles(&new), ["建立訂單", "傳真下單", "取消訂單"]);
    }

    #[test]
    fn removed_and_renamed() {
        let d = "## REMOVED Requirements\n### Requirement: 傳真下單\n**Reason**: 沒人用\n\n## RENAMED Requirements\n- FROM: `### Requirement: 取消訂單`\n- TO: `### Requirement: 撤回訂單`\n";
        let new = merge_text(Some(OLD), d, &[]).unwrap();
        assert_eq!(titles(&new), ["建立訂單", "撤回訂單"]);
        assert!(new.contains("- **THEN** 成立\n\n### Requirement: 撤回訂單\n說明。\n"), "{new}");
        assert!(new.ends_with("- **THEN** 可以取消\n"));
    }

    #[test]
    fn mixed_rename_then_modify_and_crlf_is_kept() {
        let d = "## RENAMED Requirements\n- FROM: `### Requirement: 建立訂單`\n- TO: `### Requirement: 下單`\n\n## MODIFIED Requirements\n### Requirement: 下單\n新說明。\n\n#### Scenario: 一\n- **THEN** 好\n\n## REMOVED Requirements\n### Requirement: 取消訂單\n\n## ADDED Requirements\n### Requirement: 退款\n#### Scenario: 一\n- **THEN** 退\n";
        let old = OLD.replace('\n', "\r\n");
        let new = merge_text(Some(&old), d, &[]).unwrap();
        assert_eq!(titles(&new), ["下單", "傳真下單", "退款"]);
        assert!(!new.replace("\r\n", "").contains('\n'), "要全部是 CRLF");
        assert!(new.ends_with("- **THEN** 退\r\n"));
    }

    #[test]
    fn missing_target_is_an_error() {
        assert!(merge_text(Some(OLD), "## REMOVED Requirements\n### Requirement: 沒有這個\n", &[]).is_err());
        assert!(merge_text(Some(OLD), "## ADDED Requirements\n### Requirement: 建立訂單\n#### Scenario: 一\n", &[]).is_err());
    }

    #[test]
    fn new_capability_header_follows_convention() {
        assert_eq!(new_header(None, "billing", "add-billing", None), ["# billing Specification", "", "## Purpose", "待補：這個能力由變更 add-billing 建立，請補上用途。", "", "## Requirements"]);
        assert_eq!(new_header(Some("# 登入\n\n### Requirement: 甲\n"), "billing", "c", Some("帳單")), ["# billing"]);
        assert_eq!(new_header(Some(OLD), "billing", "c", Some("帳單\n第二行")), ["# billing Specification", "", "## Purpose", "帳單", "第二行", "", "## Requirements"]);
        let new = merge_text(None, "## ADDED Requirements\n### Requirement: 開發票\n#### Scenario: 一\n- **THEN** 開\n", &new_header(Some("# 登入\n"), "billing", "c", None)).unwrap();
        assert_eq!(new, "# billing\n\n### Requirement: 開發票\n#### Scenario: 一\n- **THEN** 開\n");
    }

    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("awf-spec-unit-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("specs/orders")).unwrap();
        fs::create_dir_all(root.join("changes/c/specs/orders")).unwrap();
        fs::create_dir_all(root.join("changes/c/specs/billing")).unwrap();
        fs::write(root.join("specs/orders/spec.md"), OLD).unwrap();
        fs::write(root.join("changes/c/proposal.md"), "# 提案\n").unwrap();
        fs::write(root.join("changes/c/tasks.md"), "- [x] 1.1 做\n").unwrap();
        fs::write(root.join("changes/c/specs/orders/spec.md"), "## REMOVED Requirements\n### Requirement: 傳真下單\n").unwrap();
        fs::write(root.join("changes/c/specs/billing/spec.md"), "## ADDED Requirements\n### Requirement: 開發票\n#### Scenario: 一\n- **THEN** 開\n").unwrap();
        root
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.push((p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), Vec::new()));
                    stack.push(p);
                } else {
                    out.push((p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn plan_does_not_touch_disk_and_commit_archives() {
        let root = fixture("commit");
        let before = snapshot(&root);
        let tree = load(&root, "openspec").unwrap();
        let p = plan(&tree, "c", "2026-01-02").unwrap();
        assert_eq!(snapshot(&root), before);
        assert_eq!(p.writes.len(), 2);
        assert_eq!(p.to, "changes/archive/2026-01-02-c");
        let text = render(&p, "openspec");
        assert!(text.contains("--- /dev/null\n+++ b/openspec/specs/billing/spec.md\n") && text.contains("-### Requirement: 傳真下單\n"), "{text}");
        commit(&root, &p).unwrap();
        assert!(!root.join("changes/c").exists() && root.join("changes/archive/2026-01-02-c/tasks.md").is_file());
        assert_eq!(fs::read_to_string(root.join("specs/billing/spec.md")).unwrap(), p.writes[0].new);
        assert!(snapshot(&root).iter().all(|(n, _)| !n.contains(".awf-")));
        let tree = load(&root, "openspec").unwrap();
        assert!(plan(&tree, "c", "2026-01-02").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn every_failure_point_rolls_back() {
        // 兩個檔：billing 是新檔（1 步）、orders 是換檔（2 步），加上歸檔 1 步，共 4 步
        for step in 1..=4 {
            let root = fixture(&format!("rollback{step}"));
            let before = snapshot(&root);
            let tree = load(&root, "openspec").unwrap();
            let p = plan(&tree, "c", "2026-01-02").unwrap();
            let err = commit_with(&root, &p, Some(step)).unwrap_err();
            assert!(err.contains("已經還原"), "{err}");
            assert_eq!(snapshot(&root), before, "第 {step} 步失敗後要和原來逐位元相同");
            let _ = fs::remove_dir_all(&root);
        }
        let root = fixture("rollback5");
        let tree = load(&root, "openspec").unwrap();
        let p = plan(&tree, "c", "2026-01-02").unwrap();
        assert!(commit_with(&root, &p, Some(5)).is_ok(), "只有 4 步，第 5 步不存在");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_refuses_paths_outside_specs() {
        let root = fixture("outside");
        let before = snapshot(&root);
        for file in ["../x/spec.md", "changes/c/proposal.md", "specs/../../x.md"] {
            let p = Plan { change: "c".into(), writes: vec![FileWrite { file: file.into(), old: None, new: "x".into() }], from: "changes/c".into(), to: "changes/archive/2026-01-02-c".into() };
            assert!(commit(&root, &p).is_err(), "{file}");
            assert_eq!(snapshot(&root), before);
        }
        let _ = fs::remove_dir_all(&root);
    }
}
