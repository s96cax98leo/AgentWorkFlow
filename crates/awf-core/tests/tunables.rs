//! 工具端可調值的函式層測試（第 12 批）：命令列不好造輸入的項目在這裡直接呼叫函式。

use awf_core::config::{self, SecretExtra};
use awf_core::publish;
use awf_core::{locate, pack, run_brief, watch};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("awf-core-tunables-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude/teams")).expect("建立暫存目錄");
        Tmp(d.canonicalize().expect("解開暫存目錄"))
    }

    fn root(&self) -> String {
        self.0.to_string_lossy().replace('\\', "/")
    }

    fn guard_config(&self, v: &Value) {
        std::fs::write(self.0.join(".claude/teams/guard.config.json"), v.to_string()).expect("寫設定");
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn diff_of(path: &str, added: &str) -> String {
    format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+{added}\n")
}

// ------------------------------------------------------------------ T15：publish 的機敏掃描

/// 沒設定（沒有設定檔、設定檔沒寫這兩個鍵）：和原本的掃描逐字相同。
#[test]
fn t15_publish_scan_unchanged_without_setting() {
    let t = Tmp::new("t15-none");
    assert!(SecretExtra::for_project(&t.root()).is_empty(), "沒有設定檔");
    t.guard_config(&json!({"runs_dir": "_runs"}));
    let extra = SecretExtra::for_project(&t.root());
    assert!(extra.is_empty());
    // 內建樣式會命中的一行（分開拼，原始碼裡不放完整的標頭）
    let pem = format!("-----BEGIN {} KEY-----", "PRIVATE");
    for (path, line) in [("src/a.py", "x = 1"), ("conf/vault/prod.yaml", "token: ACME-12345678"), ("a/.env", "A=1"), ("src/b.py", pem.as_str())] {
        let d = diff_of(path, line);
        assert_eq!(publish::scan_secrets_with(&d, &[], &[], &extra), publish::scan_secrets(&d, &[], &[]), "{path}");
    }
}

/// 有設定：命中的檔名與新增行都算；內建的照舊；訊息不印值。
#[test]
fn t15_publish_scan_adds_project_patterns() {
    let t = Tmp::new("t15-set");
    t.guard_config(&json!({"secret_files_extra": ["(^|/)vault/.*\\.ya?ml$"], "secret_patterns_extra": ["\\bACME-[0-9]{8}\\b"]}));
    let extra = SecretExtra::for_project(&t.root());
    assert!(extra.errors.is_empty() && extra.files.len() == 1 && extra.patterns.len() == 1);
    let hits = publish::scan_secrets_with(&diff_of("conf/Vault/prod.yaml", "name: x"), &[], &[], &extra).expect("掃描");
    assert_eq!(hits, ["conf/Vault/prod.yaml：機敏檔名"]);
    let hits = publish::scan_secrets_with(&diff_of("src/a.py", "token = ACME-12345678"), &[], &[], &extra).expect("掃描");
    assert_eq!(hits, ["src/a.py：專案設定的機敏樣式（secret_patterns_extra 第 1 條）"]);
    assert!(!hits.join("").contains("12345678"), "不印值");
    assert!(publish::scan_secrets_with(&diff_of("src/a.py", "token = acme-12345678"), &[], &[], &extra).expect("掃描").is_empty(), "值的樣式分大小寫");
    assert!(publish::scan_secrets_with(&diff_of("src/a.py", "x = 1"), &[], &[], &extra).expect("掃描").is_empty());
    let names = vec!["conf/vault/bin.yml".to_string()];
    assert_eq!(publish::scan_secrets_with(&diff_of("src/a.py", "x = 1"), &[], &names, &extra).expect("掃描"), ["conf/vault/bin.yml：機敏檔名"], "沒有內容的檔也比對檔名");
    assert_eq!(publish::scan_secrets_with(&diff_of("a/.env", "A=1"), &[], &[], &extra).expect("掃描"), ["a/.env：機敏檔名"], "內建的照舊");
}

/// 寫壞的設定：掃描一律有命中（無法確認沒有機敏值），publish 因此拒推。
#[test]
fn t15_publish_scan_fails_closed_on_bad_setting() {
    for bad in [json!({"secret_patterns_extra": ["ACME-("]}), json!({"secret_files_extra": "vault/"}), json!({"secret_files_extra": [1]}), json!({"secret_patterns_extra": vec!["a"; config::SECRET_EXTRA_MAX + 1]})] {
        let t = Tmp::new("t15-bad");
        t.guard_config(&bad);
        let extra = SecretExtra::for_project(&t.root());
        assert!(!extra.errors.is_empty(), "{bad}");
        let hits = publish::scan_secrets_with(&diff_of("src/a.py", "x = 1"), &[], &[], &extra).expect("掃描");
        assert!(hits.iter().any(|h| h.contains("無法確認沒有機敏值") && h.contains("secret_")), "{bad}：{hits:?}");
    }
}

// ------------------------------------------------------------------ T17、T19、T23、T24：讀設定（沒寫、不合範圍 → 預設）

#[test]
fn tool_sizes_default_when_unset_or_out_of_range() {
    for p in [Value::Null, json!({}), json!({"knowledge": 3, "progress": [], "limits": "x"}), json!({"knowledge": {"pack": [], "locate": 1}, "progress": {"watch": "x"}, "limits": {"run_brief": null}})] {
        assert_eq!(pack::PackTune::of(&p), pack::PackTune::default(), "{p}");
        assert_eq!(locate::LocateTune::of(&p), locate::LocateTune::default(), "{p}");
        assert_eq!(watch::WatchTune::of(&p), watch::WatchTune::default(), "{p}");
        assert_eq!(run_brief::BriefTune::of(&p), run_brief::BriefTune::default(), "{p}");
    }
    let d = pack::PackTune::default();
    assert_eq!((d.top, d.budget, d.decisions, d.requirement_lines, d.headings), (12, 3000, 12, 200, 40), "預設＝原本寫死的值");
    let d = locate::LocateTune::default();
    assert_eq!((d.max_segments, d.segment_lines, d.min_segments, d.min_coverage, d.max_files, d.prune.len()), (12, 120, 3, 0.3, 4000, 0));
    let d = watch::WatchTune::default();
    assert_eq!((d.interval, d.stall_seconds, d.progress_step), (5, 900, 50_000));
    let d = run_brief::BriefTune::default();
    assert_eq!((d.max_fails, d.tail, d.unknown_tail), (20, 20, 40));

    let bad = json!({
        "knowledge": {"pack": {"top": 0, "budget": 1, "decisions": -1, "requirement_lines": 99999, "headings": "40"},
                      "locate": {"max_segments": 0, "segment_lines": 5, "min_segments": 2, "min_coverage": 0, "max_files": 1, "prune": ["../x"]}},
        "progress": {"watch": {"interval": 0, "stall_seconds": -5, "progress_step": 10}},
        "limits": {"run_brief": {"max_fails": 0, "tail": 1, "unknown_tail": 5000}},
    });
    assert_eq!(pack::PackTune::of(&bad), pack::PackTune::default());
    assert_eq!(locate::LocateTune::of(&bad), locate::LocateTune::default());
    assert_eq!(watch::WatchTune::of(&bad), watch::WatchTune::default());
    assert_eq!(run_brief::BriefTune::of(&bad), run_brief::BriefTune::default());
}

#[test]
fn tool_sizes_follow_the_profile() {
    let p = json!({
        "knowledge": {"pack": {"top": 8, "budget": 2000, "decisions": 0, "requirement_lines": 120, "headings": 20},
                      "locate": {"max_segments": 8, "segment_lines": 80, "min_segments": 4, "min_coverage": 0.4, "max_files": 2000, "prune": ["vendor"]}},
        "progress": {"watch": {"interval": 10, "stall_seconds": 0, "progress_step": 100000}},
        "limits": {"run_brief": {"max_fails": 10, "tail": 30, "unknown_tail": 60}},
    });
    let t = pack::PackTune::of(&p);
    assert_eq!((t.top, t.budget, t.decisions, t.requirement_lines, t.headings), (8, 2000, 0, 120, 20));
    let t = locate::LocateTune::of(&p);
    assert_eq!((t.max_segments, t.segment_lines, t.min_segments, t.min_coverage, t.max_files, t.prune.clone()), (8, 80, 4, 0.4, 2000, vec!["vendor".to_string()]));
    let t = watch::WatchTune::of(&p);
    assert_eq!((t.interval, t.stall_seconds, t.progress_step), (10, 0, 100_000));
    let t = run_brief::BriefTune::of(&p);
    assert_eq!((t.max_fails, t.tail, t.unknown_tail), (10, 30, 60));
}

/// T17：〈既有決定〉的筆數跟著設定（直接呼叫函式；預設 12、設成 2、設成 0）。
#[test]
fn t17_decisions_cap_follows_the_tune() {
    let t = Tmp::new("t17-dec");
    let runs = format!("{}/_runs", t.root());
    std::fs::create_dir_all(&runs).expect("建立 runs");
    let lines: String = (1..=15).map(|i| json!({"id": format!("d{i}"), "run": "r1", "epic": "e", "scope": "epic", "question": format!("問題 {i}"), "answer": "答", "by": "human", "at": "2026-01-01"}).to_string() + "\n").collect();
    std::fs::write(format!("{runs}/decisions.jsonl"), lines).expect("寫決定");
    let n = |tune: pack::PackTune| {
        pack::set_tune(tune);
        let got = pack::relevant_decisions(&runs, "r1", "e", "需求", "2026-01-02", &[]).expect("決定").len();
        pack::set_tune(pack::PackTune::default());
        got
    };
    assert_eq!(n(pack::PackTune::default()), 12, "預設最多 12 筆");
    assert_eq!(n(pack::PackTune { decisions: 2, ..pack::PackTune::default() }), 2);
    assert_eq!(n(pack::PackTune { decisions: 0, ..pack::PackTune::default() }), 0);
}

/// T23：agent_progress 的間距跟著設定（預設 5 萬 token；設成 1 萬時多 2 萬就記一筆）。
#[test]
fn t23_progress_step_follows_the_tune() {
    let snap = |tokens: i64| json!({"state": "running", "stages": [], "workflows": [{"wf_run": "w1", "workflow": "sdlc-plan", "tokens": tokens, "finished": false, "status": "running",
        "agents": [{"id": "a1", "label": "research:x", "role": "tpm-researcher", "model": "m", "tokens": tokens, "requests": 1}]}]});
    let kinds = |ev: &[Value]| ev.iter().map(|e| e["type"].as_str().unwrap_or_default().to_string()).collect::<Vec<_>>();
    let (first, mem) = watch::diff_events(&json!({}), &snap(1000), 0).expect("第一次");
    assert!(kinds(&first).contains(&"agent_started".to_string()), "{first:?}");
    let (ev, _) = watch::diff_events(&mem, &snap(21_000), 0).expect("預設");
    assert!(!kinds(&ev).contains(&"agent_progress".to_string()), "預設要多 5 萬才記：{ev:?}");
    let (ev, _) = watch::diff_events(&mem, &snap(51_000), 0).expect("預設");
    assert!(kinds(&ev).contains(&"agent_progress".to_string()), "多 5 萬就記：{ev:?}");
    watch::set_progress_step(10_000);
    let (ev, _) = watch::diff_events(&mem, &snap(21_000), 0).expect("設成 1 萬");
    watch::set_progress_step(50_000);
    assert!(kinds(&ev).contains(&"agent_progress".to_string()), "設成 1 萬時多 2 萬就記：{ev:?}");
}
