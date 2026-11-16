//! 1.0 獨立審查（移植）發現的 schema 端問題：
//!
//! - M1：語意規則走訪前要先看型別；型別不對一律回錯誤（不能當成空的）。
//! - M2：`extract_json` 最後一個 json 區塊解析不了就是沒有信封，不退回前一個。
//! - 任務編號的樣式要和 0.x 一樣看待「字」（帶結合符號的編號）。

use awf_core::schema::{self, Schemas};
use serde_json::{json, Value};
use std::path::PathBuf;

fn teams() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin/scaffold/.claude/teams")
}

fn envelope(payload: Value) -> Value {
    json!({"status": "done", "summary": "s", "artifacts": [], "evidence": [], "confidence": 0.9, "open_questions": [], "derived_work": [], "payload": payload})
}

fn sub(id: Value, owns: Value, depends_on: Value) -> Value {
    json!({"id": id, "title": "t", "kind": "code", "owns": owns, "depends_on": depends_on, "acceptance": ["x"]})
}

/// 只驗信封本體（save-run 存檔時的驗法）：payload 的形狀只有語意規則看得到。
fn errs(payload: Value) -> Vec<String> {
    Schemas::new(teams()).check(&envelope(payload), None).expect("schema 讀得到")
}

fn has(errs: &[String], part: &str) -> bool {
    errs.iter().any(|e| e.contains(part))
}

#[test]
fn m1_subtasks_owns_of_wrong_type_is_an_error() {
    // 審查的重現：兩個子任務的 owns 是同一個字串（0.x 逐字元走訪，判重疊）
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!("src/a.py"), json!([])), sub(json!("t2"), json!("src/a.py"), json!([]))]}));
    assert!(has(&e, "owns"), "owns 是字串要報錯：{e:?}");
    // owns 是物件（0.x 走訪鍵，判重疊）
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!({"src/a.py": 1}), json!([])), sub(json!("t2"), json!({"src/a.py": 1}), json!([]))]}));
    assert!(has(&e, "owns"), "owns 是物件要報錯：{e:?}");
    // owns 的元素不是字串（0.x 在這裡丟例外）
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!([{"p": "src/a.py"}]), json!([]))]}));
    assert!(has(&e, "owns"), "owns 的元素是物件要報錯：{e:?}");
    // 其他不是陣列的值
    for bad in [json!(5), json!(true), json!("")] {
        let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), bad.clone(), json!([]))]}));
        assert!(has(&e, "owns"), "owns={bad} 要報錯：{e:?}");
    }
}

#[test]
fn m1_subtasks_depends_on_and_id_of_wrong_type_are_errors() {
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!(["a"]), json!("zz"))]}));
    assert!(has(&e, "depends_on"), "depends_on 是字串要報錯：{e:?}");
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!(["a"]), json!(5))]}));
    assert!(has(&e, "depends_on"), "depends_on 是數字要報錯：{e:?}");
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!(["a"]), json!([7]))]}));
    assert!(has(&e, "depends_on"), "depends_on 的元素是數字要報錯：{e:?}");
    let e = errs(json!({"epic": "e", "subtasks": [sub(json!(["t1"]), json!(["a"]), json!([]))]}));
    assert!(has(&e, "id"), "id 是陣列要報錯：{e:?}");
    // subtasks 本身不是陣列、元素不是物件
    let e = errs(json!({"epic": "e", "subtasks": {"t1": sub(json!("t1"), json!(["a"]), json!([]))}}));
    assert!(has(&e, "subtasks"), "subtasks 是物件要報錯：{e:?}");
    let e = errs(json!({"epic": "e", "subtasks": ["t1"]}));
    assert!(has(&e, "subtasks[0]"), "subtasks 的元素不是物件要報錯：{e:?}");
    // 正常的不受影響；沒有 owns／depends_on（或 null）照舊當成沒有
    assert_eq!(errs(json!({"epic": "e", "subtasks": [sub(json!("t1"), json!(["a"]), json!([])), sub(json!("t2"), json!(["b"]), json!(["t1"]))]})), Vec::<String>::new());
    assert_eq!(errs(json!({"epic": "e", "subtasks": [{"id": "t1", "owns": null}]})), Vec::<String>::new());
}

#[test]
fn m1_plan_outline_of_wrong_type_is_an_error() {
    let plan = |a_dep: Value, b_dep: Value, a_owns: Value| json!({"split": true, "sub_plans": [
        {"id": "a", "title": "a", "owns": a_owns, "depends_on": a_dep}, {"id": "b", "title": "b", "owns": ["src/b"], "depends_on": b_dep}]});
    // 審查的重現：depends_on 是字串的循環相依（0.x 逐字元走訪，判循環）
    let e = errs(plan(json!("b"), json!("a"), json!(["src/a"])));
    assert!(has(&e, "depends_on"), "depends_on 是字串要報錯：{e:?}");
    let e = errs(plan(json!([]), json!([]), json!("src/b")));
    assert!(has(&e, "owns"), "owns 是字串要報錯：{e:?}");
    let e = errs(plan(json!([]), json!([]), json!([1])));
    assert!(has(&e, "owns"), "owns 的元素是數字要報錯：{e:?}");
    let e = errs(json!({"split": true, "sub_plans": "a,b"}));
    assert!(has(&e, "sub_plans"), "sub_plans 是字串要報錯：{e:?}");
    let e = errs(json!({"split": true, "sub_plans": [{"id": 1, "owns": ["x"]}, {"id": 1.0, "owns": ["y"]}]}));
    assert!(has(&e, "id"), "子計劃的 id 是數字要報錯：{e:?}");
    assert_eq!(errs(plan(json!([]), json!(["a"]), json!(["src/a"]))), Vec::<String>::new());
}

#[test]
fn m1_merge_decision_of_wrong_type_is_an_error() {
    let dec = |rows: Value, followups: Value| json!({"decision": "merge", "traceability": rows, "followups": followups, "out_of_scope": []});
    let row = |id: Value| json!({"requirement_id": id, "requirement": "r", "evidence": ["e"], "result": "match"});
    // 審查的重現：requirement_id 是 1 與 1.0（0.x 當成同一個鍵，判重複）
    let e = schema::merge_decision_errors(&dec(json!([row(json!(1)), row(json!(1.0))]), json!([])));
    assert!(has(&e, "requirement_id"), "requirement_id 是數字要報錯：{e:?}");
    let e = schema::merge_decision_errors(&dec(json!({"REQ-1": row(json!("REQ-1"))}), json!([])));
    assert!(has(&e, "traceability"), "traceability 是物件要報錯：{e:?}");
    let e = schema::merge_decision_errors(&dec(json!(["REQ-1"]), json!([])));
    assert!(has(&e, "traceability[0]"), "traceability 的元素不是物件要報錯：{e:?}");
    let e = schema::merge_decision_errors(&dec(json!([row(json!("REQ-1"))]), json!("REQ-2")));
    assert!(has(&e, "followups"), "followups 是字串要報錯：{e:?}");
    let e = schema::merge_decision_errors(&dec(json!([row(json!(["REQ-1"]))]), json!([])));
    assert!(has(&e, "requirement_id"), "requirement_id 是陣列要報錯：{e:?}");
    assert_eq!(schema::merge_decision_errors(&dec(json!([row(json!("REQ-1"))]), json!([]))), Vec::<String>::new());
}

#[test]
fn m1_methodology_gate_inputs_of_wrong_type_are_errors() {
    // 閘門會讀的三個欄位（限制文件、recipe、modifier）：save-run 讀 profile 時只跑語意規則，型別不對不能當成沒寫
    let e = schema::methodology_errors(&json!({"constraints": {"id": "c1", "file": "README.md", "stages": ["test"]}}));
    assert!(has(&e, "constraints"), "{e:?}");
    let e = schema::methodology_errors(&json!({"constraints": ["README.md"]}));
    assert!(has(&e, "constraints[0]"), "{e:?}");
    let e = schema::methodology_errors(&json!({"constraints": [{"id": "c1", "file": "README.md", "stages": "test"}]}));
    assert!(has(&e, "stages"), "{e:?}");
    let e = schema::methodology_errors(&json!({"constraints": [{"id": "c1", "file": "README.md", "stages": ["test"], "paths": "src/**"}]}));
    assert!(has(&e, "paths"), "{e:?}");
    let e = schema::methodology_errors(&json!({"recipes": ["feature"]}));
    assert!(has(&e, "recipes"), "{e:?}");
    let e = schema::methodology_errors(&json!({"modifiers": "security"}));
    assert!(has(&e, "modifiers"), "{e:?}");
    assert_eq!(schema::methodology_errors(&json!({"constraints": [{"id": "c1", "file": "README.md", "stages": ["test"], "paths": ["src/**"]}], "constraints_note": 1})), Vec::<String>::new());
}

#[test]
fn m2_extract_json_takes_only_the_last_block() {
    let good = r#"{"decision": "merge"}"#;
    let text = |last: &str| format!("先前的版本：\n```json\n{good}\n```\n更正：\n```json\n{last}\n```\n");
    // 最後一塊解析得了：用它
    assert_eq!(schema::extract_json(&text(r#"{"decision": "hold"}"#)), Some(json!({"decision": "hold"})));
    // 審查的重現：最後一塊含 NaN、落單的代理字元跳脫、巢狀超過 128 層 —— 都不能退回前一塊
    let deep = format!("{{\"decision\": \"hold\", \"x\": {}1{}}}", "[".repeat(200), "]".repeat(200));
    for last in [r#"{"decision": "hold", "x": NaN}"#, r#"{"decision": "hold", "x": "\ud800"}"#, deep.as_str(), r#"{"decision": "hold","#] {
        assert_eq!(schema::extract_json(&text(last)), None, "最後一塊讀不懂時要回 None：{}", &last[..last.len().min(60)]);
    }
    // 合格信封後面接一塊讀不懂的（交件 hook 的重現）
    let env = envelope(json!({}));
    assert_eq!(schema::extract_json(&format!("```json\n{env}\n```\n```json\n{{\"x\": NaN}}\n```")), None);
    // 沒有 json 區塊時照舊試整段
    assert_eq!(schema::extract_json(good), Some(json!({"decision": "merge"})));
    assert_eq!(schema::extract_json("沒有信封"), None);
}

#[test]
fn m2_malformed_last_block_still_counts_as_the_last_block() {
    let good = r#"{"decision": "merge"}"#;
    // 寫壞到連區塊的樣子都不完整：少右括號、沒有收尾的圍欄、不是物件 —— 都還是「最後一個區塊」
    for tail in ["```json\n{\"decision\": \"hold\",\n```", "```json\n{\"decision\": \"hold\"", "```json\n這裡不是 JSON\n```", "```json\n```"] {
        assert_eq!(schema::extract_json(&format!("```json\n{good}\n```\n更正：\n{tail}")), None, "{tail}");
    }
    // 不是 json 區塊的圍欄不算
    assert_eq!(schema::extract_json(&format!("```json\n{good}\n```\n```jsonl\n壞的\n```\n```text\n壞的\n```")), Some(json!({"decision": "merge"})));
    assert!(schema::has_json_block("```json\n{壞的"));
    assert!(!schema::has_json_block("```text\n{}\n```"));
}

#[test]
fn m2_recovered_envelope_is_dropped_when_a_later_block_is_unreadable() {
    // 中斷回收（awf 從 agent 的 transcript 取它最後交的信封）：之後交的 json 區塊讀不懂時，不能回頭用更早的那一份
    let dir = std::env::temp_dir().join(format!("awf-review-m2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建立暫存目錄");
    let env = envelope(json!({}));
    let line = |text: String| json!({"message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}).to_string();
    let good = line(format!("```json\n{env}\n```"));
    let write = |name: &str, lines: &[String]| {
        let p = dir.join(name);
        std::fs::write(&p, lines.join("\n")).expect("寫 transcript");
        p.to_string_lossy().into_owned()
    };
    let last = awf_core::state::auto::last_envelope;
    assert_eq!(last(&write("ok.jsonl", &[good.clone(), line("收尾說明，沒有信封".into())])), Some(env.clone()));
    assert_eq!(last(&write("bad.jsonl", &[good.clone(), line("更正：\n```json\n{\"status\": \"blocked\", \"x\": NaN}\n```".into())])), None);
    // 字串形式的 content 一樣
    let plain = json!({"message": {"role": "assistant", "content": "```json\n{\"status\": NaN}\n```"}}).to_string();
    assert_eq!(last(&write("bad2.jsonl", &[good, plain])), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn m2_backticks_inside_a_json_string_do_not_break_the_last_block() {
    // 信封的字串裡有「```」（例如證據貼了一段程式碼圍欄）：仍然是讀得懂的最後一個區塊
    let v = json!({"status": "done", "summary": "用法：\n```\nawf next\n```\n結束"});
    assert_eq!(schema::extract_json(&format!("說明\n```json\n{v}\n```\n收尾")), Some(v.clone()));
    // 沒有收尾圍欄、但內容完整：一樣是最後一個區塊
    assert_eq!(schema::extract_json(&format!("```json\n{{\"old\": 1}}\n```\n```json\n{v}")), Some(v));
}
