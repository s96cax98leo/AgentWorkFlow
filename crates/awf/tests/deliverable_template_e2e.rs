//! 交付範本的端到端測試：實際執行編好的 `awf`，在暫存目錄 init 一個專案，放使用者自己的範本檔，
//! 看設定、骨架、受影響清單（派工單的來源）是不是照範本；範本路徑不合規則、讀不到時退回原本的做法。不呼叫模型。
//! 「沒設定範本時逐字不變」由既有的回歸基準（`tests/parity/deliverables.expected.json`、`pack.expected.json`）負責。

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

const HANDOVER: &str = "---\ntitle: 公司範本\n---\n# 系統交付說明書\n\n| 文件編號 | 版本 |\n|---|---|\n| （PM 填） | 1.0 |\n\n## 一、交付範圍\n\n（說明這次交付涵蓋的功能，條列）\n\n## 二、驗證結果\n\n## 三、待客戶配合\n";
const SUMMARY: &str = "# 交付摘要\n\n## 本次重點\n\n## 驗收方式\n";

impl Proj {
    fn new(tag: &str) -> Proj {
        static N: AtomicUsize = AtomicUsize::new(0);
        let tmp = std::env::temp_dir().join(format!("awf-dtpl-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let p = Proj { dir: tmp.join("proj"), tmp };
        let cfg = root().join("plugin/workflow.config.example");
        let (code, out) = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", cfg.to_str().unwrap()]);
        assert_eq!(code, 0, "awf init：\n{out}");
        p.put("docs/templates/handover.md", HANDOVER);
        p.put("docs/templates/summary.md", SUMMARY);
        p
    }

    fn put(&self, rel: &str, text: &str) {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("讀不到 {rel}：{e}"))
    }

    fn awf(&self, args: &[&str]) -> (i32, String) {
        let o = Command::new(EXE).args(args).current_dir(&self.dir).env("CLAUDE_PROJECT_DIR", &self.dir).env_remove("AWF_TEMPLATE_DIR").env("AWF_PLUGIN_DIR", root().join("plugin"))
            .stdin(Stdio::null()).output().expect("執行 awf");
        (o.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }

    fn affected(&self) -> Value {
        self.put("paths.txt", "src/lib.rs\n");
        std::fs::create_dir_all(self.dir.join("_tpm/runs/r1")).unwrap();
        let (code, out) = self.awf(&["deliverables", "affected", "--run", "_tpm/runs/r1", "--paths-file", "paths.txt", "--json"]);
        assert_eq!(code, 0, "{out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{out}"))
    }
}

#[test]
fn configure_with_template_then_skeleton_and_dispatch_follow_it() {
    let p = Proj::new("cfg");
    let (code, out) = p.awf(&["deliverables", "configure", "--template", "handover=docs/templates/handover.md", "--summary-template", "docs/templates/summary.md", "--write", "--by", "tester"]);
    assert_eq!(code, 0, "{out}");
    let cfg: Value = serde_json::from_str(&p.read(".claude/teams/deliverables.json")).unwrap();
    let doc = &cfg["documents"][0];
    assert_eq!(doc["template"], json!("docs/templates/handover.md"));
    assert_eq!(doc["title"], json!("系統交付說明書"), "自訂文件的標題取範本的一級標題");
    assert_eq!(doc["sections"], json!(["一、交付範圍", "二、驗證結果", "三、待客戶配合"]), "小節取範本的二級標題");
    assert_eq!(cfg["summary_template"], json!("docs/templates/summary.md"));

    let (code, out) = p.awf(&["deliverables", "init"]);
    assert_eq!(code, 0, "{out}");
    let text = p.read("docs/deliverables/handover.md");
    assert!(text.starts_with("---\nid: handover\n"), "檔頭是工具的：\n{text}");
    assert!(text.contains("| 文件編號 | 版本 |") && text.contains("（說明這次交付涵蓋的功能，條列）"), "範本的版面與說明要保留：\n{text}");
    assert!(!text.contains("公司範本"), "範本自己的檔頭不帶進來：\n{text}");
    assert_eq!(text.matches("<!-- awf:source").count(), 3, "每個小節都要有來源標記：\n{text}");
    assert!(text.contains("## 三、待客戶配合\n<!-- awf:source  -->\n\n（待補）"), "空的小節補待補：\n{text}");

    // 派工單的來源：受影響的文件帶範本路徑；pack 的 deliver 參數帶交付摘要的範本與它的小節
    let aff = p.affected();
    assert_eq!(aff["affected"][0]["template"], json!("docs/templates/handover.md"), "{aff}");
    let (code, out) = p.awf(&["deliverables", "status", "--json"]);
    assert_eq!(code, 0, "{out}");

    // 已有的文件不會被範本蓋掉
    p.put("docs/deliverables/handover.md", &text.replace("（待補）", "人寫的內容"));
    let (_, out) = p.awf(&["deliverables", "init"]);
    assert!(out.contains("文件都已經在") && p.read("docs/deliverables/handover.md").contains("人寫的內容"), "{out}");
}

#[test]
fn handwritten_config_takes_sections_from_template() {
    let p = Proj::new("hand");
    // 設定檔只寫 template、沒寫 sections：小節在載入時從範本取
    p.put(".claude/teams/deliverables.json", &json!({"dir": "docs/deliverables", "summary_template": "docs/templates/summary.md",
        "documents": [{"id": "handover", "title": "交付說明", "source": "auto", "on": ["code"], "template": "docs/templates/handover.md"}]}).to_string());
    let aff = p.affected();
    assert_eq!(aff["affected"][0]["sections"], json!(["一、交付範圍", "二、驗證結果", "三、待客戶配合"]), "{aff}");
    let (code, out) = p.awf(&["deliverables", "init"]);
    assert_eq!(code, 0, "{out}");
    assert!(p.read("docs/deliverables/handover.md").contains("# 系統交付說明書"));

    // 設定檔有寫 sections：以設定為準，範本沒有的小節接在骨架最後
    let p2 = Proj::new("hand2");
    p2.put(".claude/teams/deliverables.json", &json!({"documents": [{"id": "handover", "title": "交付說明", "source": "auto", "on": ["code"],
        "template": "docs/templates/handover.md", "sections": ["一、交付範圍", "附錄"]}]}).to_string());
    assert_eq!(p2.affected()["affected"][0]["sections"], json!(["一、交付範圍", "附錄"]));
    p2.awf(&["deliverables", "init"]);
    assert!(p2.read("docs/deliverables/handover.md").contains("## 附錄\n<!-- awf:source  -->\n\n（待補）"));
}

#[test]
fn bad_or_missing_template_falls_back() {
    let p = Proj::new("bad");
    for bad in ["handover=../x.md", "handover=/etc/hosts", "handover=docs/templates/none.md", "handover", "handover=.env"] {
        let (code, out) = p.awf(&["deliverables", "configure", "--template", bad]);
        assert_eq!(code, 2, "{bad} 要被拒絕：{out}");
    }
    let (code, out) = p.awf(&["deliverables", "configure", "--summary-template", "../s.md"]);
    assert_eq!(code, 2, "{out}");

    // 設定檔裡寫了不合規則的路徑：整份設定判有問題（不默默採用）
    p.put(".claude/teams/deliverables.json", &json!({"documents": [{"id": "handover", "title": "交付說明", "source": "auto", "on": ["code"], "sections": ["甲"], "template": "/abs/t.md"}]}).to_string());
    let aff = p.affected();
    assert_eq!(aff["enabled"], json!(false), "{aff}");
    assert!(aff["note"].as_str().unwrap_or("").contains("template"), "{aff}");

    // 範本是指到專案外的連結：不讀（內容會被抄進交付文件）
    #[cfg(unix)]
    {
        p.put("../outside.md", "# 外面的檔\n\n## 不該出現\n");
        std::os::unix::fs::symlink(p.tmp.join("outside.md"), p.dir.join("docs/templates/link.md")).unwrap();
        let (code, out) = p.awf(&["deliverables", "configure", "--template", "handover=docs/templates/link.md"]);
        assert_eq!(code, 2, "{out}");
    }

    // 範本檔後來不見了：當成沒設定，照原本的骨架，受影響清單不帶 template
    p.put(".claude/teams/deliverables.json", &json!({"documents": [{"id": "handover", "title": "交付說明", "source": "auto", "on": ["code"], "sections": ["甲"], "template": "docs/templates/gone.md"}]}).to_string());
    let aff = p.affected();
    assert!(aff["affected"][0].get("template").is_none() && aff["affected"][0]["sections"] == json!(["甲"]), "{aff}");
    let (code, out) = p.awf(&["deliverables", "init"]);
    assert_eq!(code, 0, "{out}");
    assert!(p.read("docs/deliverables/handover.md").contains("# 交付說明\n\n## 甲\n"));
}
