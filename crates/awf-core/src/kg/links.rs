//! 連結帳本 `<runs_dir>/links.jsonl` 的讀取（只讀；帳本由 `awf save-run` 寫，見 `save_run/links.rs`）。
//!
//! 格式以 `schemas/link.schema.json` 為準：一行一個 JSON 物件，一定有 `schema_version`、`op`、`run`、`stage`。
//!
//! - `op: "add"`：一條連結，另有 `kind`（底線寫法，例如 `ac_spec`）、`from`／`to`（`{type, id}` 物件）、`source`、`strength`。
//! - `op: "supersede"`：同一個 run、同一段在它之前的連結全部作廢。
//!
//! 照順序套用，留下來的才是有效的連結。壞行（不是 JSON、不是物件、缺必要欄位、端點型別不認得）略過並計數。
//! `schema_version` 比這裡認得的新時，整個帳本略過（欄位可能換了意思，不猜）。
//! 同一個版本裡可以多出新的連結種類：不認得的種類照樣收，查詢時歸在「其他連結」。

use serde_json::Value;

/// 認得的最新格式版本（和寫入端同一個常數）。
pub const KNOWN_VERSION: u64 = crate::save_run::LINK_SCHEMA_VERSION;
/// 端點的型別（`link.schema.json` 的 `from.type`／`to.type`）。
pub const END_TYPES: [&str; 10] = ["requirement", "change", "run", "ac", "spec", "file", "test", "commit", "decision", "deliverable"];

/// 一端：型別與 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    pub ty: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub kind: String,
    pub from: End,
    pub to: End,
    pub run: String,
    pub stage: String,
    /// 這條連結取自哪個欄位（例如 `design.json:subtasks[t1].owns`）。
    pub source: String,
    pub strong: bool,
    pub confidence: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    /// 有效的連結（已套用 supersede），照寫入順序。
    pub links: Vec<Link>,
    /// 略過的壞行數。
    pub bad: usize,
    /// 帳本裡出現比 [`KNOWN_VERSION`] 新的版本時是那個版本；這時 `links` 是空的。
    pub newer: Option<u64>,
}

fn text<'a>(obj: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= 500)
}

fn end(v: Option<&Value>) -> Option<End> {
    let o = v?.as_object()?;
    let ty = text(o, "type").filter(|t| END_TYPES.contains(t))?;
    Some(End { ty: ty.to_string(), id: text(o, "id")?.to_string() })
}

pub fn parse_ledger(text_in: &str) -> Ledger {
    let mut out = Ledger::default();
    for raw in text_in.lines() {
        let ln = raw.trim();
        if ln.is_empty() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(ln) else {
            out.bad += 1;
            continue;
        };
        let Some(ver) = obj.get("schema_version").and_then(Value::as_u64).filter(|v| *v >= 1) else {
            out.bad += 1;
            continue;
        };
        if ver > KNOWN_VERSION {
            out.newer = Some(out.newer.map_or(ver, |n| n.max(ver)));
            continue;
        }
        let (Some(run), Some(stage)) = (text(&obj, "run"), text(&obj, "stage")) else {
            out.bad += 1;
            continue;
        };
        match text(&obj, "op") {
            Some("supersede") => out.links.retain(|l| l.run != run || l.stage != stage),
            Some("add") => {
                let (Some(kind), Some(from), Some(to)) = (text(&obj, "kind"), end(obj.get("from")), end(obj.get("to"))) else {
                    out.bad += 1;
                    continue;
                };
                let strong = text(&obj, "strength") != Some("weak");
                let confidence = obj.get("confidence").and_then(Value::as_f64).filter(|c| (0.0..=1.0).contains(c)).unwrap_or(if strong { 1.0 } else { 0.6 });
                out.links.push(Link {
                    kind: kind.to_string(),
                    from,
                    to,
                    run: run.to_string(),
                    stage: stage.to_string(),
                    source: text(&obj, "source").unwrap_or("").to_string(),
                    strong,
                    confidence,
                });
            }
            _ => out.bad += 1,
        }
    }
    if out.newer.is_some() {
        out.links.clear();
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
    use serde_json::json;

    fn add(kind: &str, from: (&str, &str), to: (&str, &str), run: &str, stage: &str) -> String {
        json!({"schema_version": 1, "op": "add", "kind": kind, "from": {"type": from.0, "id": from.1}, "to": {"type": to.0, "id": to.1}, "run": run, "stage": stage, "source": "x", "strength": "strong"}).to_string()
    }

    #[test]
    fn supersede_and_bad_lines() {
        let text = [
            add("ac_file", ("ac", "AC-1"), ("file", "src/a.ts"), "r1", "design"),
            add("ac_file", ("ac", "AC-1"), ("file", "src/b.ts"), "r1", "build"),
            add("ac_file", ("ac", "AC-1"), ("file", "src/a.ts"), "r2", "design"),
            json!({"schema_version": 1, "op": "supersede", "run": "r1", "stage": "design"}).to_string(),
            add("ac_file", ("ac", "AC-1"), ("file", "src/c.ts"), "r1", "design"),
            "不是 json".to_string(),
            "[1,2]".to_string(),
            "{\"kind\":\"ac-file\",\"from\":\"a\",\"to\":\"b\"}".to_string(),
            json!({"schema_version": 1, "op": "add", "kind": "ac_file", "from": {"type": "別的", "id": "x"}, "to": {"type": "file", "id": "y"}, "run": "r1", "stage": "design"}).to_string(),
            json!({"schema_version": 1, "op": "移除", "run": "r1", "stage": "design"}).to_string(),
            add("新的種類", ("ac", "AC-1"), ("file", "src/d.ts"), "r1", "test"),
        ]
        .join("\n");
        let l = parse_ledger(&text);
        assert_eq!(l.bad, 5);
        assert_eq!(l.newer, None);
        let got: Vec<(&str, &str, &str)> = l.links.iter().map(|x| (x.run.as_str(), x.stage.as_str(), x.to.id.as_str())).collect();
        assert_eq!(got, vec![("r1", "build", "src/b.ts"), ("r2", "design", "src/a.ts"), ("r1", "design", "src/c.ts"), ("r1", "test", "src/d.ts")]);
        assert!(l.links.iter().all(|x| x.strong && x.confidence == 1.0));
    }

    #[test]
    fn newer_version_skips_everything() {
        let text = format!("{}\n{}\n", add("ac_file", ("ac", "AC-1"), ("file", "src/a.ts"), "r1", "design"), json!({"schema_version": KNOWN_VERSION + 1, "op": "add", "run": "r1", "stage": "design"}));
        let l = parse_ledger(&text);
        assert!(l.links.is_empty());
        assert_eq!(l.newer, Some(KNOWN_VERSION + 1));
    }
}
