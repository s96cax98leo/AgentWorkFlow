//! 工具端可調值的函式層測試（第 12 批）：命令列不好造輸入的項目在這裡直接呼叫函式。

use awf_core::config::{self, SecretExtra};
use awf_core::publish;
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
    for (path, line) in [("src/a.py", "x = 1"), ("conf/vault/prod.yaml", "token: ACME-12345678"), ("a/.env", "A=1"), ("src/b.py", "-----BEGIN PRIVATE KEY-----")] {
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
