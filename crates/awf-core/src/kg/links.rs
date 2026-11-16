//! 連結帳本 `<runs_dir>/links.jsonl` 的讀取（只讀；帳本由 `awf save-run` 寫）。
//!
//! 一行一個 JSON 物件。這裡只要求最小欄位，其餘欄位略過；壞行（不是 JSON、不是物件、缺必要欄位）略過並計數。
//!
//! | 欄位 | 必要 | 說明 | 也接受的別名 |
//! |---|---|---|---|
//! | `kind` | 是 | 連結種類，見 [`KINDS`] | `type`、`link` |
//! | `from` | 是 | 起點的 id | `src`、`source` |
//! | `to` | 是 | 終點的 id | `dst`、`target` |
//! | `run` | 否 | run 目錄名 | |
//! | `stage` | 否 | 哪一段寫的 | |
//! | `strength` | 否 | `strong`（預設）或 `weak` | |
//! | `confidence` | 否 | 0–1 的數字；沒寫時 strong＝1、weak＝0.6 | |
//!
//! 兩端的 id 可以帶型別前綴（`req:`、`scn:`、`cap:`、`ac:`、`file:`、`test:`、`sec:`、`doc:`、`commit:`、`run:`、`dec:`）；
//! 沒帶時依 `kind` 的對照表判斷，`kind` 不認得就看 id 的長相。

use serde_json::Value;

/// 一端是什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// 需求、情境或 AC 的 id。
    Id,
    Run,
    File,
    /// 文件小節（`路徑#標題`）或規格的 id。
    Sec,
    Commit,
    Dec,
    /// 看長相。
    Any,
}

/// 認得的連結種類與兩端的型別。
pub const KINDS: [(&str, End, End); 7] = [
    ("req-run", End::Id, End::Run),
    ("ac-spec", End::Id, End::Sec),
    ("ac-file", End::Id, End::File),
    ("ac-test", End::Id, End::File),
    ("ac-commit", End::Id, End::Commit),
    ("decision-req", End::Dec, End::Id),
    ("change-doc", End::File, End::Sec),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub kind: String,
    pub from: String,
    pub to: String,
    pub run: String,
    pub stage: String,
    pub strong: bool,
    pub confidence: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    pub links: Vec<Link>,
    /// 略過的壞行數。
    pub bad: usize,
}

/// 種類的寫法統一：小寫，底線、空白、箭頭換成 `-`。
pub fn norm_kind(s: &str) -> String {
    let low = s.trim().to_lowercase().replace("->", "-").replace('→', "-");
    let mut out = String::new();
    for c in low.chars() {
        let c = if c == '_' || c.is_whitespace() { '-' } else { c };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('-').to_string()
}

pub fn ends_of(kind: &str) -> (End, End) {
    KINDS.iter().find(|k| k.0 == kind).map(|k| (k.1, k.2)).unwrap_or((End::Any, End::Any))
}

fn pick<'a>(obj: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| obj.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= 500)
}

pub fn parse_ledger(text: &str) -> Ledger {
    let mut out = Ledger::default();
    for raw in text.lines() {
        let ln = raw.trim();
        if ln.is_empty() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(ln) else {
            out.bad += 1;
            continue;
        };
        let (Some(kind), Some(from), Some(to)) = (pick(&obj, &["kind", "type", "link"]), pick(&obj, &["from", "src", "source"]), pick(&obj, &["to", "dst", "target"])) else {
            out.bad += 1;
            continue;
        };
        let strong = pick(&obj, &["strength"]).is_none_or(|s| !s.eq_ignore_ascii_case("weak"));
        let confidence = obj.get("confidence").and_then(Value::as_f64).filter(|c| (0.0..=1.0).contains(c)).unwrap_or(if strong { 1.0 } else { 0.6 });
        out.links.push(Link {
            kind: norm_kind(kind),
            from: from.to_string(),
            to: to.to_string(),
            run: pick(&obj, &["run"]).unwrap_or("").to_string(),
            stage: pick(&obj, &["stage"]).unwrap_or("").to_string(),
            strong,
            confidence,
        });
    }
    out
}

/// 讀帳本；檔案不存在或讀不到回空的帳本。
pub fn read_ledger(path: &str) -> Ledger {
    match std::fs::read(path) {
        Ok(bytes) => parse_ledger(&String::from_utf8_lossy(&bytes)),
        Err(_) => Ledger::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerant() {
        let text = "{\"kind\":\"ac-file\",\"from\":\"a/b#c\",\"to\":\"src/x.ts\",\"run\":\"r1\",\"stage\":\"design\",\"extra\":[1,2]}\n\
不是 json\n\
[1,2]\n\
{\"kind\":\"ac-test\",\"from\":\"a/b\"}\n\
\n\
{\"type\":\"AC → Commit\",\"src\":\"a/b\",\"dst\":\"abc1234\",\"strength\":\"weak\",\"confidence\":2}\n";
        let l = parse_ledger(text);
        assert_eq!(l.bad, 3);
        assert_eq!(l.links.len(), 2);
        assert_eq!(l.links[0].kind, "ac-file");
        assert!(l.links[0].strong);
        assert_eq!(l.links[1].kind, "ac-commit");
        assert!(!l.links[1].strong);
        assert_eq!(l.links[1].confidence, 0.6);
        assert_eq!(ends_of("ac-commit"), (End::Id, End::Commit));
        assert_eq!(ends_of("別的"), (End::Any, End::Any));
    }
}
