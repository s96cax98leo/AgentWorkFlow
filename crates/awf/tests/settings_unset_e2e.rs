//! 「只補還沒設定的主題」（`awf init --detect --unset`、`--reconfigure --unset`）的端到端測試：實際執行編好的 `awf`。
//! 看：只列沒回答過的主題、補完就不再列、已經設定過的主題不能經由 `--unset` 被改、升級後設定原樣保留並提示還沒設定的。不呼叫模型。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

struct Proj {
    tmp: PathBuf,
    dir: PathBuf,
}

impl Drop for Proj {
    fn drop(&mut self) {
        if std::env::var("KEEP").as_deref() != Ok("1") {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
}

impl Proj {
    fn new(tag: &str) -> Proj {
        static N: AtomicUsize = AtomicUsize::new(0);
        let tmp = std::env::temp_dir().join(format!("awf-unset-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let p = Proj { dir: tmp.join("proj"), tmp };
        let cfg = root().join("plugin/workflow.config.example");
        let (code, out) = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", cfg.to_str().unwrap()]);
        assert_eq!(code, 0, "awf init：\n{out}");
        p
    }

    fn put(&self, rel: &str, text: &str) -> String {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("讀不到 {rel}：{e}"))
    }

    fn awf(&self, args: &[&str]) -> (i32, String) {
        let o = Command::new(EXE).args(args).current_dir(&self.dir).env("CLAUDE_PROJECT_DIR", &self.dir).env_remove("AWF_TEMPLATE_DIR").env("AWF_PLUGIN_DIR", root().join("plugin"))
            .stdin(Stdio::null()).output().expect("執行 awf");
        (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }

    fn unset_ids(&self) -> Vec<String> {
        let (code, out) = self.awf(&["init", "--detect", "--unset", "--json", "--target", "."]);
        assert_eq!(code, 0, "{out}");
        let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{out}"));
        v["topics"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect()
    }
}

#[test]
fn only_unset_topics_are_listed_and_filled() {
    let p = Proj::new("fill");
    let before = p.unset_ids();
    assert!(before.contains(&"models".to_string()) && before.contains(&"knowledge".to_string()) && before.contains(&"deliverables".to_string()), "{before:?}");

    // 非互動、沒給答案檔：只列，不改
    let meth0 = p.read(".claude/teams/methodology.json");
    let (code, out) = p.awf(&["init", "--target", ".", "--reconfigure", "--unset"]);
    assert!(code == 0 && out.contains("還沒設定的主題") && out.contains("沒有問、沒有改"), "{out}");
    assert_eq!(p.read(".claude/teams/methodology.json"), meth0);

    // 補兩個主題
    let ans = p.put("a1.json", &json!({"models": "auto", "knowledge": {"sources": ["docs/**/*.md"]}}).to_string());
    let (code, out) = p.awf(&["init", "--target", ".", "--reconfigure", "--unset", "--answers", &ans]);
    assert_eq!(code, 0, "{out}");
    let meth: Value = serde_json::from_str(&p.read(".claude/teams/methodology.json")).unwrap();
    assert_eq!(meth["models"]["mode"], json!("auto"));
    assert_eq!(meth["knowledge"]["sources"], json!(["docs/**/*.md"]));
    let after = p.unset_ids();
    assert!(!after.contains(&"models".to_string()) && !after.contains(&"knowledge".to_string()) && after.len() + 2 == before.len(), "{before:?} → {after:?}");

    // 已經設定過的主題不能經由 --unset 被改（要改用 --reconfigure <主題>）
    let ans2 = p.put("a2.json", &json!({"models": "inherit"}).to_string());
    let (code, out) = p.awf(&["init", "--target", ".", "--reconfigure", "--unset", "--answers", &ans2]);
    assert!(code != 0 && out.contains("已經設定過的：models"), "{out}");
    let meth: Value = serde_json::from_str(&p.read(".claude/teams/methodology.json")).unwrap();
    assert_eq!(meth["models"]["mode"], json!("auto"), "被拒絕時不能改到設定");

    // 交付文件：有 deliverables.json 就算設定過
    p.put(".claude/teams/deliverables.json", &json!({"documents": [{"id": "db-design"}]}).to_string());
    assert!(!p.unset_ids().contains(&"deliverables".to_string()));

    // 用法不對
    let (code, out) = p.awf(&["init", "--target", ".", "--unset"]);
    assert!(code != 0 && out.contains("--unset 要搭配"), "{out}");
    let (code, out) = p.awf(&["init", "--target", ".", "--reconfigure", "models", "--unset"]);
    assert!(code != 0 && out.contains("只能擇一"), "{out}");
}

#[test]
fn upgrade_keeps_settings_and_hints_unset_topics() {
    let p = Proj::new("up");
    let ans = p.put("a1.json", &json!({"models": "auto"}).to_string());
    let (code, out) = p.awf(&["init", "--target", ".", "--reconfigure", "--answers", &ans]);
    assert_eq!(code, 0, "{out}");
    p.put("docs/templates/handover.md", "# 交付說明書\n\n## 範圍\n");
    let (code, out) = p.awf(&["deliverables", "configure", "--template", "handover=docs/templates/handover.md", "--write", "--by", "tester"]);
    assert_eq!(code, 0, "{out}");
    let keep = [".claude/teams/methodology.json", ".claude/teams/deliverables.json", ".claude/teams/workflow.config", ".claude/teams/guard.config.json", ".claude/teams/init-answers.json"];
    let before: Vec<String> = keep.iter().map(|f| p.read(f)).collect();
    p.put(".claude/teams/VERSION", "1.0.0\n");
    let (code, out) = p.awf(&["init", "--target", ".", "--upgrade"]);
    assert_eq!(code, 0, "{out}");
    for (f, old) in keep.iter().zip(&before) {
        assert_eq!(&p.read(f), old, "升級後 {f} 要原樣保留");
    }
    assert!(out.contains("設定都保留了，不用重設") && out.contains("--reconfigure --unset") && out.contains("settings-missing"), "{out}");
    let hint = out.lines().find(|l| l.contains("還沒設定過的主題")).unwrap_or_default();
    assert!(!hint.contains("模型分級") && !hint.contains("交付文件") && hint.contains("知識庫與知識圖"), "提示只列沒設定過的：{hint}");
}
