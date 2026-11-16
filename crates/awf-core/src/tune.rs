//! 工具端可調值的共用讀法（profile 的 `knowledge.pack`、`knowledge.locate`、`progress.watch`、`limits.run_brief`、`limits.display`）。
//!
//! 規則只有一條：沒寫、型別不對、超出範圍，一律用預設值（＝原本寫死的值）。這些值只影響輔助輸出的大小與節奏，
//! 閘門不讀它們，所以寫壞了不擋流程；範圍寫在 `methodology.schema.json`，這裡的上下限要和它一致。

use serde_json::Value;

/// 專案的 profile（`<root>/.claude/teams/methodology.json`）。沒有、讀不到、不是 JSON 物件都回 `Null`。
pub fn profile(root: &str) -> Value {
    let p = std::path::Path::new(root).join(".claude/teams/methodology.json");
    match std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(v) if v.is_object() => v,
        _ => Value::Null,
    }
}

/// 從目前目錄找到的專案的 profile；找不到專案回 `Null`。
pub fn project_profile() -> Value {
    match crate::project::find(None) {
        Some(f) => profile(&f.root),
        None => Value::Null,
    }
}

/// 沿著鍵往下取；中途不是物件就沒有。
pub fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, k| cur.as_object()?.get(*k))
}

/// 整數設定：要是 JSON 整數（不收布林、小數、字串）而且在 `lo..=hi` 之間，否則用預設。
pub fn int(v: Option<&Value>, lo: i64, hi: i64, default: i64) -> i64 {
    match v {
        Some(Value::Number(n)) if !n.is_f64() => n.as_i64().filter(|x| (lo..=hi).contains(x)).unwrap_or(default),
        _ => default,
    }
}

/// 數字設定（整數或小數）：在 `lo..=hi` 之間才用，否則用預設。
pub fn num(v: Option<&Value>, lo: f64, hi: f64, default: f64) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite() && *x >= lo && *x <= hi).unwrap_or(default),
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn int_falls_back_to_default() {
        let p = json!({"a": {"n": 5, "big": 999, "f": 5.0, "s": "5", "b": true, "neg": -1}});
        assert_eq!(int(at(&p, &["a", "n"]), 1, 10, 3), 5);
        for k in ["big", "f", "s", "b", "neg", "none"] {
            assert_eq!(int(at(&p, &["a", k]), 1, 10, 3), 3, "{k}");
        }
        assert_eq!(int(at(&p, &["x", "n"]), 1, 10, 3), 3);
        assert_eq!(int(at(&Value::Null, &["a"]), 1, 10, 3), 3);
    }

    #[test]
    fn num_falls_back_to_default() {
        let p = json!({"c": 0.5, "i": 1, "big": 2.5, "s": "0.5"});
        assert_eq!(num(p.get("c"), 0.05, 1.0, 0.3), 0.5);
        assert_eq!(num(p.get("i"), 0.05, 1.0, 0.3), 1.0);
        assert_eq!(num(p.get("big"), 0.05, 1.0, 0.3), 0.3);
        assert_eq!(num(p.get("s"), 0.05, 1.0, 0.3), 0.3);
        assert_eq!(num(None, 0.05, 1.0, 0.3), 0.3);
    }
}
