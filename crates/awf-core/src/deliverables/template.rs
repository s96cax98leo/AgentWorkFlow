//! 交付範本（1.2.1）：使用者自己的範本檔決定交付文件的小節與版面。
//!
//! - 設定：`documents[].template`（這份文件的範本）與 `summary_template`（交付摘要的範本），都是專案內的相對路徑。
//!   來源和其他交付設定一樣：`deliverables.json`、profile 的 `deliverables`、init 的答案檔、
//!   `awf deliverables configure --template <id>=<路徑> --summary-template <路徑>`。
//! - 小節：設定沒有寫 `sections`（或 `summary_sections`）時，取範本的二級標題（和 `parse_doc` 同一套認法）。
//!   設定有寫的以設定為準。
//! - 骨架：文件還不存在時，照範本的內文建立（保留範本的說明文字與版面），每個小節補上來源標記。
//! - 派工單：`pack-args.json` 受影響的文件多一個 `template`，`sdlc-deliver` 的 writer 照它的版面與用語寫。
//!
//! 這是輔助功能：範本讀不到、太大、不是文字、路徑不合規則時一律當成沒設定（照原本的骨架與小節），
//! 沒設定範本時輸出逐字不變。閘門不讀範本。

use serde_json::{json, Map, Value};

use super::core::{parse_doc, safe_rel};
use super::PLACEHOLDER;
use crate::kb::pyish;

/// 範本檔的大小上限（位元組）。
pub const MAX_BYTES: u64 = 200_000;
/// 從範本取的小節數上限。
const MAX_SECTIONS: usize = 60;

fn sensitive(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_lowercase();
    name == ".env" || name.starts_with(".env.") || name == "id_rsa" || name == "id_ed25519" || name.starts_with("credentials") || name.starts_with("secret.")
        || name.starts_with("secrets.") || [".pem", ".key", ".p12", ".pfx"].iter().any(|e| name.ends_with(e))
}

/// 範本路徑合不合規則：專案內的相對路徑、不是機敏檔。不合規則回原因。
pub fn path_error(raw: &Value) -> Option<String> {
    let Some(s) = raw.as_str() else { return Some("要是字串".to_string()) };
    let rel = safe_rel(raw);
    if rel.is_empty() {
        return Some(format!("要是專案內的相對路徑（不能是絕對路徑或含 ..）：{s}"));
    }
    sensitive(&rel).then(|| format!("不能是機敏檔：{s}"))
}

/// 讀範本（專案根目錄底下）。路徑不合規則、不是檔案、太大、不是 UTF-8 文字都回 `None`。
pub fn read(root: &str, raw: &Value) -> Option<String> {
    if !raw.is_string() || path_error(raw).is_some() {
        return None;
    }
    let p = pyish::pjoin(root, &safe_rel(raw));
    // 解開連結之後還要在專案裡：範本的內容會被抄進交付文件，不能經由連結把專案外的檔帶進來
    let (real, real_root) = (std::fs::canonicalize(&p).ok()?, std::fs::canonicalize(root).ok()?);
    if !real.starts_with(&real_root) || sensitive(&real.to_string_lossy().replace('\\', "/")) {
        return None;
    }
    let meta = std::fs::metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let text = String::from_utf8(std::fs::read(&p).ok()?).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).replace("\r\n", "\n");
    (!text.contains('\0')).then_some(text)
}

/// 去掉範本自己的檔頭（開頭的 `---` 區塊）。
fn body(text: &str) -> &str {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else { return text };
    if pyish::strip(first) != "---" {
        return text;
    }
    let mut used = first.len();
    for ln in lines {
        used += ln.len();
        if pyish::strip(ln) == "---" {
            return text.get(used..).unwrap_or("");
        }
    }
    text
}

/// 範本的二級標題（不重複、照出現順序）。
pub fn headings(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in parse_doc(body(text)).sections {
        if !s.title.is_empty() && !out.contains(&s.title) && out.len() < MAX_SECTIONS {
            out.push(s.title);
        }
    }
    out
}

/// 範本的一級標題（第一個；沒有回 `None`）。自訂文件的標題用它。
pub fn title(text: &str) -> Option<String> {
    let mut fence = false;
    for ln in body(text).split('\n') {
        if ln.trim_start_matches(pyish::is_space).starts_with("```") {
            fence = !fence;
        }
        if !fence && ln.starts_with("# ") {
            let t: String = pyish::strip(ln.get(2..).unwrap_or("")).chars().take(80).collect();
            return (!t.is_empty()).then_some(t);
        }
    }
    None
}

fn str_or(doc: &Value, key: &str, default: &str) -> String {
    doc.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
}

/// 照範本建立文件骨架：我們的檔頭＋範本的內文；每個小節標題下補來源標記，空的小節補「（待補）」。
/// 設定有寫、範本沒有的小節接在最後（檢查時才不會缺節）。
pub fn skeleton(doc: &Value, tpl: &str) -> String {
    let id = str_or(doc, "id", "");
    let title = str_or(doc, "title", &id);
    let srcs: Vec<String> = doc.get("sources").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let marker = format!("<!-- awf:source {} -->", srcs.join("; "));
    let mut out: Vec<String> = vec!["---".into(), format!("id: {id}"), format!("title: {title}"), format!("group: {}", str_or(doc, "group", "custom")),
        format!("source: {}", str_or(doc, "source", "assisted")), format!("depth: {}", str_or(doc, "depth", "full")), "---".into(), String::new()];
    let text = body(tpl);
    let lines: Vec<&str> = text.trim_matches('\n').split('\n').collect();
    let is_fence = |ln: &str| ln.trim_start_matches(pyish::is_space).starts_with("```");
    let is_h2 = |ln: &str| ln.starts_with("## ") && !ln.starts_with("###");
    let (mut fence, mut has_h1) = (false, false);
    for ln in &lines {
        if is_fence(ln) {
            fence = !fence;
        }
        if !fence && ln.starts_with("# ") {
            has_h1 = true;
        }
    }
    if !has_h1 {
        out.extend([format!("# {title}"), String::new()]);
    }
    fence = false;
    let mut seen: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let ln = lines[i];
        if is_fence(ln) {
            fence = !fence;
        }
        if fence || !is_h2(ln) {
            out.push(ln.to_string());
            i += 1;
            continue;
        }
        seen.push(pyish::strip(ln.get(3..).unwrap_or("")).to_string());
        out.push(ln.to_string());
        // 這一節到下一個二級標題之前的內容
        let mut j = i + 1;
        let mut f2 = false;
        while j < lines.len() {
            if is_fence(lines[j]) {
                f2 = !f2;
            }
            if !f2 && is_h2(lines[j]) {
                break;
            }
            j += 1;
        }
        let seg = &lines[i + 1..j];
        let first = seg.iter().position(|l| !pyish::is_blank(l));
        let marked = first.is_some_and(|k| pyish::strip(seg[k]).starts_with("<!-- awf:source"));
        if !marked {
            out.push(marker.clone());
        }
        let rest_blank = match (first, marked) {
            (None, _) => true,
            (Some(k), true) => seg[k + 1..].iter().all(|l| pyish::is_blank(l)),
            (Some(_), false) => false,
        };
        if rest_blank {
            if marked {
                out.push(seg[first.unwrap_or(0)].to_string());
            }
            out.extend([String::new(), PLACEHOLDER.to_string(), String::new()]);
        } else {
            if !marked {
                out.push(String::new());
            }
            let start = seg.iter().position(|l| !pyish::is_blank(l)).unwrap_or(0);
            out.extend(seg[start..].iter().map(|l| l.to_string()));
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
        }
        i = j;
    }
    if out.last().is_some_and(|l| !l.is_empty()) {
        out.push(String::new());
    }
    for s in doc.get("sections").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>()).unwrap_or_default() {
        if !seen.iter().any(|t| t == s) {
            out.extend([format!("## {s}"), marker.clone(), String::new(), PLACEHOLDER.to_string(), String::new()]);
        }
    }
    out.join("\n")
}

/// 交付摘要的節名規則（和 `flow::summary_sections` 採用的條件一致：1–40 字的一行、沒有反引號）。
fn summary_name_ok(s: &str) -> bool {
    let n = s.chars().count();
    (1..=40).contains(&n) && s.trim() == s && !s.contains(['`', '\r', '\n'])
}

/// 設定載入後補上範本帶來的小節：文件有 `template`、沒寫 `sections` 時取範本的二級標題；
/// 有 `summary_template`、沒寫 `summary_sections` 時同理（要 1–8 個、每個合節名規則才採用）。
/// 沒有任何範本設定時 `cfg` 原樣不動。
pub fn fill(root: &str, cfg: &mut Value) {
    let Some(obj) = cfg.as_object_mut() else { return };
    if let Some(docs) = obj.get_mut("documents").and_then(Value::as_array_mut) {
        for d in docs.iter_mut() {
            let Some(m) = d.as_object_mut() else { continue };
            let has_sections = m.get("sections").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
            if has_sections {
                continue;
            }
            let Some(text) = m.get("template").and_then(|t| read(root, t)) else { continue };
            let hs = headings(&text);
            if !hs.is_empty() {
                m.insert("sections".to_string(), json!(hs));
            }
        }
    }
    if obj.get("summary_sections").is_none() {
        if let Some(text) = obj.get("summary_template").and_then(|t| read(root, t)) {
            let hs = headings(&text);
            if (1..=8).contains(&hs.len()) && hs.iter().all(|s| summary_name_ok(s)) {
                obj.insert("summary_sections".to_string(), json!(hs));
            }
        }
    }
}

/// 設定裡的範本路徑（合規則而且讀得到才回）。給資料包帶給派工單用。
pub fn usable(root: &str, raw: &Value) -> Option<String> {
    read(root, raw).map(|_| safe_rel(raw))
}

/// `configure --template a=路徑,b=路徑` 的解析：回（id, 路徑）；寫法不對回錯誤。
pub fn parse_pairs(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((id, path)) = part.split_once('=') else { return Err(format!("--template 要寫成 <文件 id>=<範本路徑>（收到 {part}）")) };
        let (id, path) = (id.trim(), path.trim());
        if id.is_empty() || path.is_empty() {
            return Err(format!("--template 要寫成 <文件 id>=<範本路徑>（收到 {part}）"));
        }
        out.push((id.to_string(), path.to_string()));
    }
    Ok(out)
}

/// 把範本設到一份文件上（configure 用）：寫 `template`；原本沒有 `sections` 的自訂文件順便寫進範本的小節，讓設定檔看得到。
pub fn apply(doc: &mut Map<String, Value>, path: &str, text: &str, custom: bool) {
    doc.insert("template".to_string(), json!(path));
    let hs = headings(text);
    let lacks = !doc.get("sections").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
    if (custom || lacks) && !hs.is_empty() {
        doc.insert("sections".to_string(), json!(hs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPL: &str = "---\ntitle: 公司範本\n---\n# 系統交付說明書\n\n文件編號：（由 PM 填）\n\n## 一、範圍\n\n說明這次交付涵蓋的功能。\n\n## 二、驗證結果\n\n```\n## 這不是標題\n```\n\n## 三、待辦\n";

    #[test]
    fn headings_follow_the_template() {
        assert_eq!(headings(TPL), ["一、範圍", "二、驗證結果", "三、待辦"]);
        assert_eq!(title(TPL).as_deref(), Some("系統交付說明書"));
    }

    #[test]
    fn skeleton_keeps_layout_and_adds_markers() {
        let doc = json!({"id": "handover", "title": "交付說明", "sources": ["src/**"], "sections": ["一、範圍", "附錄"]});
        let s = skeleton(&doc, TPL);
        assert!(s.starts_with("---\nid: handover\ntitle: 交付說明\n"), "{s}");
        assert!(!s.contains("公司範本") && s.contains("# 系統交付說明書") && s.contains("文件編號：（由 PM 填）"), "{s}");
        assert!(s.contains("## 一、範圍\n<!-- awf:source src/** -->\n\n說明這次交付涵蓋的功能。"), "{s}");
        assert!(s.contains("## 三、待辦\n<!-- awf:source src/** -->\n\n（待補）"), "{s}");
        assert!(s.contains("## 附錄\n<!-- awf:source src/** -->\n\n（待補）"), "{s}");
        let parsed = parse_doc(&s);
        let titles: Vec<&str> = parsed.sections.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["一、範圍", "二、驗證結果", "三、待辦", "附錄"]);
        assert!(parsed.sections.iter().all(|x| x.marker == ["src/**"]), "{:?}", parsed.sections);
    }

    #[test]
    fn bad_paths_are_rejected() {
        for bad in [json!("/etc/x.md"), json!("../x.md"), json!("a/.env"), json!("keys/id_rsa"), json!(3)] {
            assert!(path_error(&bad).is_some(), "{bad}");
        }
        assert!(path_error(&json!("docs/templates/srs.md")).is_none());
    }

    #[test]
    fn pairs() {
        assert_eq!(parse_pairs("a=x.md, b=t/y.md").unwrap(), [("a".to_string(), "x.md".to_string()), ("b".to_string(), "t/y.md".to_string())]);
        assert!(parse_pairs("a").is_err() && parse_pairs("=x").is_err());
    }
}
