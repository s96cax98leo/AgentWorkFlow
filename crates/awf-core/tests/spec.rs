//! 規格層（`awf spec`）的整合測試：在暫存目錄建虛構的規格與變更（示範客戶 ACME 的訂單與登入），
//! 走過讀目錄、穩定 id、validate、apply 的每一條規則。沒有 Python 版，不對跑。

use awf_core::spec::{self, ChangeState, IdOptions};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const ORDERS: &str = "# orders Specification\n\n## Purpose\n示範客戶 ACME 的訂單。\n\n## Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立\n\n#### Scenario: 庫存不足\n- **WHEN** 庫存是 0\n- **THEN** 回覆缺貨\n\n### Requirement: 取消訂單\n系統 SHALL 讓會員在出貨前取消訂單。\n\n#### Scenario: 出貨前\n- **WHEN** 訂單還沒出貨\n- **THEN** 可以取消\n\n### Requirement: 傳真下單\n系統 SHALL 接受傳真訂單。\n\n#### Scenario: 收到傳真\n- **WHEN** 收到傳真\n- **THEN** 建立訂單\n";

/// CRLF、標題前後空白、圍欄裡的假標題。
const LOGIN: &str = "# login Specification\r\n\r\n## Purpose\r\n示範客戶 ACME 的會員登入。\r\n\r\n## Requirements\r\n\r\n  ###   Requirement:   密碼登入  \r\n系統 SHALL 驗證帳號密碼。\r\n\r\n```md\r\n### Requirement: 圍欄裡的不算\r\n#### Scenario: 這個也不算\r\n```\r\n\r\n#### Scenario: 密碼正確\r\n- **WHEN** 密碼正確\r\n- **THEN** 進入首頁\r\n";

const D_ADDED: &str = "## ADDED Requirements\n\n### Requirement: 申請退款\n系統 SHALL 讓會員在七天內申請退款。\n\n#### Scenario: 七天內\n- **WHEN** 收貨未滿七天\n- **THEN** 受理退款\n";
const D_MODIFIED: &str = "## MODIFIED Requirements\n\n### Requirement: 建立訂單\n系統 SHALL 讓會員建立訂單並寄出確認信。\n\n#### Scenario: 庫存足夠\n- **WHEN** 會員送出訂單\n- **THEN** 訂單成立並寄出確認信\n";
const D_REMOVED: &str = "## REMOVED Requirements\n\n### Requirement: 傳真下單\n**Reason**: 已經沒有人用傳真。\n";
const D_RENAMED: &str = "## RENAMED Requirements\n\n- FROM: `### Requirement: 取消訂單`\n- TO: `### Requirement: 撤回訂單`\n";

struct Fx {
    base: PathBuf,
}

impl Fx {
    fn new(tag: &str) -> Fx {
        let base = std::env::temp_dir().join(format!("awf-spec-it-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let fx = Fx { base };
        fx.write("openspec/specs/orders/spec.md", ORDERS);
        fx.write("openspec/specs/login/spec.md", LOGIN);
        fx
    }

    fn root(&self) -> PathBuf {
        self.base.join("openspec")
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.base.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.base.join(rel)).unwrap()
    }

    /// 建一個變更；`deltas` 是（能力、差異檔內容）。
    fn change(&self, name: &str, deltas: &[(&str, &str)]) {
        self.write(&format!("openspec/changes/{name}/proposal.md"), "# 提案\n\n示範客戶 ACME 的需求。\n");
        self.write(&format!("openspec/changes/{name}/tasks.md"), "## 1. 實作\n- [x] 1.1 第一件\n- [ ] 1.2 第二件\n");
        for (cap, text) in deltas {
            self.write(&format!("openspec/changes/{name}/specs/{cap}/spec.md"), text);
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        spec::run_captured(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &self.base)
    }

    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![self.base.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let name = p.strip_prefix(&self.base).unwrap().to_string_lossy().replace('\\', "/");
                if p.is_dir() {
                    out.push((name, Vec::new()));
                    stack.push(p);
                } else {
                    out.push((name, fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn titles(fx: &Fx, cap: &str) -> Vec<String> {
    let t = spec::load(&fx.root(), "openspec").unwrap();
    t.capability(cap).map(|c| c.requirements.iter().map(|r| r.title.clone()).collect()).unwrap_or_default()
}

// ───────────────────────── S1 讀目錄 ─────────────────────────

#[test]
fn s1_model_has_capabilities_changes_and_line_spans() {
    let fx = Fx::new("s1-model");
    fx.change("add-refund", &[("orders", &format!("{D_ADDED}\n{D_MODIFIED}\n{D_REMOVED}\n{D_RENAMED}"))]);
    fx.change("old-thing", &[("orders", D_REMOVED)]);
    fs::create_dir_all(fx.root().join("changes/archive")).unwrap();
    fs::rename(fx.root().join("changes/old-thing"), fx.root().join("changes/archive/2025-12-31-old-thing")).unwrap();
    fx.write("openspec/changes/add-refund/design.md", "# 設計\n");

    let o = spec::open(&fx.root(), "openspec", &IdOptions::default()).unwrap();
    let t = &o.tree;
    assert!(t.problems.is_empty(), "{:?}", t.problems);
    assert_eq!(t.capabilities.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["login", "orders"]);
    let login = &t.capabilities[0].requirements[0];
    assert_eq!((login.title.as_str(), login.span.start, login.span.end, login.file.as_str()), ("密碼登入", 8, 18, "specs/login/spec.md"));
    assert_eq!(login.scenarios.len(), 1);
    assert_eq!((login.scenarios[0].span.start, login.scenarios[0].span.end), (16, 18));
    assert_eq!(login.id, "login/密碼登入");
    assert_eq!(login.scenarios[0].id, "login/密碼登入#密碼正確");
    let orders = &t.capabilities[1];
    assert_eq!(orders.requirements.len(), 3);
    assert_eq!(orders.requirements[0].scenarios.len(), 2);
    assert_eq!(orders.requirements[0].body, "系統 SHALL 讓會員建立訂單。");

    let ch = t.change("add-refund", ChangeState::Active).unwrap();
    assert!(ch.has_proposal && ch.has_tasks && ch.has_design);
    assert_eq!((ch.tasks.done, ch.tasks.total), (1, 2));
    let d = &ch.deltas[0];
    assert_eq!((d.added.len(), d.modified.len(), d.removed.len(), d.renamed.len()), (1, 1, 1, 1));
    assert_eq!(d.modified[0].id, "orders/建立訂單");
    assert_eq!(d.renamed[0].id, "orders/取消訂單");
    let ar = t.change("old-thing", ChangeState::Archived).unwrap();
    assert_eq!(ar.archived_date.as_deref(), Some("2025-12-31"));
    assert_eq!(ar.dir, "changes/archive/2025-12-31-old-thing");
}

#[test]
fn s1_empty_file_missing_root_and_non_utf8() {
    let fx = Fx::new("s1-empty");
    fx.write("openspec/specs/empty/spec.md", "");
    let (c, o, e) = fx.run(&["status"]);
    assert_eq!(c, 0, "{e}");
    assert!(o.contains("empty　需求 0、情境 0"), "{o}");
    let (c, _, e) = fx.run(&["status", "--root", "沒有這個目錄"]);
    assert_eq!(c, 1);
    assert!(e.contains("找不到規格目錄"), "{e}");
    fs::write(fx.root().join("specs/empty/spec.md"), [0xff, 0xfe, 0x00, 0x23]).unwrap();
    let (c, o, _) = fx.run(&["validate"]);
    assert_eq!(c, 1);
    assert!(o.contains("openspec/specs/empty/spec.md:1：檔案不是 UTF-8 文字"), "{o}");
}

#[test]
fn s1_duplicate_requirement_is_an_error() {
    let fx = Fx::new("s1-dup");
    fx.write("openspec/specs/login/spec.md", "### Requirement: 密碼登入\n#### Scenario: 一\n\n### Requirement: 密碼登入\n#### Scenario: 二\n");
    let (c, o, _) = fx.run(&["validate"]);
    assert_eq!(c, 1);
    assert!(o.contains("openspec/specs/login/spec.md:4：需求「密碼登入」重複（第一次在第 1 行）"), "{o}");
    let (c, _, e) = fx.run(&["ids", "--write"]);
    assert_eq!(c, 1, "同名時不寫 ids.json");
    assert!(e.contains("沒有寫入") && !fx.root().join("ids.json").exists(), "{e}");
}

#[test]
fn s1_odd_input_does_not_panic() {
    let fx = Fx::new("s1-odd");
    let odd = "###\n### Requirement\n### Requirement:\n#### Scenario:\n####Scenario: 沒空白\n### Requirement: 🙂 表情\n#### Scenario: \u{202e}反向\n```\n### Requirement: 沒關的圍欄\n";
    fx.write("openspec/specs/odd/spec.md", odd);
    fx.change("odd", &[("odd", "## RENAMED Requirements\n- FROM:\n- TO: ``\n- FROM: 🙂\n## ADDED Requirements\n### Requirement:\n")]);
    for args in [&["status"][..], &["validate"], &["ids"], &["status", "--json"], &["validate", "--json"], &["apply", "--change", "odd"]] {
        let (c, _, _) = fx.run(args);
        assert!((0..=2).contains(&c), "{args:?}");
    }
    let (_, o, _) = fx.run(&["validate", "--json"]);
    let v: Value = serde_json::from_str(&o).unwrap();
    assert_eq!(v["ok"], Value::Bool(false));
    assert!(v["problems"].as_array().unwrap().iter().any(|p| p["code"] == "unclosed-fence"));
}

// ───────────────────────── S2 穩定 id ─────────────────────────

#[test]
fn s2_ids_preview_then_write_is_reproducible() {
    let fx = Fx::new("s2-write");
    let before = fx.snapshot();
    let (c, o, _) = fx.run(&["ids"]);
    assert_eq!(c, 0);
    assert!(o.contains("+ orders/建立訂單　「建立訂單」") && o.contains("+ orders/建立訂單#庫存不足") && o.contains("預覽"), "{o}");
    assert_eq!(fx.snapshot(), before, "沒有 --write 不能寫檔");

    assert_eq!(fx.run(&["ids", "--write"]).0, 0);
    let text = fx.read("openspec/ids.json");
    assert!(text.starts_with("{\n  \"requirements\": {\n    \"login/密碼登入\": {\n      \"scenarios\": {\n        \"密碼正確\": \"密碼正確\"\n      },\n      \"title\": \"密碼登入\"\n    },\n"), "{text}");
    assert!(text.ends_with("  \"version\": 1\n}\n") && !text.contains("\\u"));
    assert!(!fx.root().join("ids.json.awf-new").exists());
    // 重跑、刪掉重建，都逐位元相同
    let (c, o, _) = fx.run(&["ids", "--write"]);
    assert!(c == 0 && o.contains("已經是最新的"), "{o}");
    fs::remove_file(fx.root().join("ids.json")).unwrap();
    fx.run(&["ids", "--write"]);
    assert_eq!(fx.read("openspec/ids.json"), text);
    let fx2 = Fx::new("s2-write-b");
    fx2.run(&["ids", "--write"]);
    assert_eq!(fx2.read("openspec/ids.json"), text);
}

#[test]
fn s2_retitle_keeps_id_after_human_confirms() {
    let fx = Fx::new("s2-retitle");
    fx.run(&["ids", "--write"]);
    let reg = fx.read("openspec/ids.json");
    fx.write("openspec/specs/orders/spec.md", &ORDERS.replace("Requirement: 建立訂單", "Requirement: 建立新訂單"));

    let (c, o, _) = fx.run(&["ids", "--write"]);
    assert_eq!(c, 1, "有待確認要回非零");
    assert!(o.contains("待確認（1）") && o.contains("orders/建立訂單　登記的標題「建立訂單」找不到了；可能改成了 「建立新訂單」（openspec/specs/orders/spec.md:8）"), "{o}");
    assert_eq!(fx.read("openspec/ids.json"), reg, "待確認的不能自動猜、不能動");
    let (_, o, _) = fx.run(&["status"]);
    assert!(o.contains("待確認的 id（1）"), "{o}");

    let (c, _, e) = fx.run(&["ids", "--adopt", "orders/建立訂單=沒有這個標題", "--write"]);
    assert!(c == 1 && e.contains("--adopt"), "{e}");
    let (c, o, e) = fx.run(&["ids", "--adopt", "orders/建立訂單=建立新訂單", "--write"]);
    assert_eq!(c, 0, "{o}{e}");
    let (c, o, _) = fx.run(&["show", "orders/建立訂單"]);
    assert_eq!(c, 0);
    assert_eq!(o, "openspec/specs/orders/spec.md:8-17　需求：建立新訂單\n");
    let (_, o, _) = fx.run(&["show", "orders/建立訂單#庫存不足"]);
    assert_eq!(o, "openspec/specs/orders/spec.md:15-17　情境：庫存不足\n");
    assert!(fx.read("openspec/ids.json").contains("\"title\": \"建立新訂單\""));
}

#[test]
fn s2_retire_and_scenario_retitle() {
    let fx = Fx::new("s2-retire");
    fx.run(&["ids", "--write"]);
    fx.write("openspec/specs/orders/spec.md", &ORDERS.replace("Scenario: 庫存不足", "Scenario: 庫存不足時").replace("Requirement: 傳真下單", "Requirement: 傳真與電話下單"));
    let (c, o, _) = fx.run(&["ids"]);
    assert!(c == 1 && o.contains("待確認（2）") && o.contains("orders/建立訂單#庫存不足") && o.contains("orders/傳真下單"), "{o}");
    let (c, o, e) = fx.run(&["ids", "--adopt", "orders/建立訂單#庫存不足=庫存不足時", "--retire", "orders/傳真下單", "--write"]);
    assert_eq!(c, 0, "{o}{e}");
    assert_eq!(fx.run(&["show", "orders/建立訂單#庫存不足"]).0, 0);
    assert_eq!(fx.run(&["show", "orders/傳真下單"]).0, 1);
    assert_eq!(fx.run(&["show", "orders/傳真與電話下單"]).0, 0);
}

#[test]
fn s2_renamed_block_keeps_id_without_confirmation() {
    let fx = Fx::new("s2-renamed");
    fx.run(&["ids", "--write"]);
    fx.change("rename-cancel", &[("orders", D_RENAMED)]);
    let (_, o, _) = fx.run(&["status"]);
    assert!(o.contains("RENAMED　orders/取消訂單　「撤回訂單」"), "{o}");
    assert_eq!(fx.run(&["apply", "--change", "rename-cancel", "--date", "2026-01-02", "--write"]).0, 0);
    let (c, o, _) = fx.run(&["ids", "--write"]);
    assert_eq!(c, 0, "{o}");
    assert!(o.contains("~ orders/取消訂單　標題「取消訂單」→「撤回訂單」") && o.contains("待確認（0）"), "{o}");
    let (_, o, _) = fx.run(&["show", "orders/取消訂單"]);
    assert!(o.ends_with("需求：撤回訂單\n"), "{o}");
    assert_eq!(fx.run(&["show", "orders/撤回訂單"]).0, 1, "不該另外長出新 id");
}

#[test]
fn s2_id_pattern_and_bad_inputs() {
    let fx = Fx::new("s2-pattern");
    fx.write("openspec/specs/billing/spec.md", "### Requirement: REQ-12: 開立發票\n#### Scenario: 付款完成\n\n### Requirement: [us-3] 作廢發票\n#### Scenario: 當月\n");
    let (c, o, e) = fx.run(&["ids", "--id-pattern", r"^\[?((?i:req|us)-\d+)"]);
    assert_eq!(c, 0, "{e}");
    assert!(o.contains("+ billing/req-12　") && o.contains("+ billing/us-3　") && o.contains("+ billing/us-3#當月"), "{o}");
    let (c, o, _) = fx.run(&["ids"]);
    assert!(c == 0 && o.contains("+ billing/req-12-開立發票"), "預設不啟用：{o}");
    let (c, _, e) = fx.run(&["ids", "--id-pattern", "("]);
    assert!(c == 1 && e.contains("--id-pattern"), "{e}");
    fx.write("openspec/ids.json", "{ 壞掉");
    let (c, _, e) = fx.run(&["status"]);
    assert!(c == 1 && e.contains("ids.json"), "{e}");
}

// ───────────────────────── S3 validate ─────────────────────────

#[test]
fn s3_validate_passes_on_clean_tree() {
    let fx = Fx::new("s3-clean");
    fx.change("add-refund", &[("orders", &format!("{D_ADDED}\n{D_MODIFIED}\n{D_REMOVED}\n{D_RENAMED}")), ("billing", &D_ADDED.replace("申請退款", "開立發票"))]);
    let (c, o, e) = fx.run(&["validate"]);
    assert_eq!(c, 0, "{o}{e}");
    assert!(o.contains("2 個能力、1 個進行中的變更"));
    let (c, o, _) = fx.run(&["validate", "--change", "add-refund", "--json"]);
    assert_eq!(c, 0);
    assert_eq!(serde_json::from_str::<Value>(&o).unwrap()["ok"], Value::Bool(true));
}

#[test]
fn s3_validate_each_rule_blocks() {
    let cases: &[(&str, &str, &str)] = &[
        ("modified-missing", "## MODIFIED Requirements\n### Requirement: 沒有這個\n#### Scenario: 一\n", "spec.md:2：MODIFIED 指到的需求「沒有這個」在現行規格 specs/orders/spec.md 找不到"),
        ("removed-missing", "## REMOVED Requirements\n### Requirement: 沒有這個\n", "spec.md:2：REMOVED 指到的需求「沒有這個」"),
        ("renamed-missing", "## RENAMED Requirements\n- FROM: `### Requirement: 沒有這個`\n- TO: `### Requirement: 新名字`\n", "spec.md:2：RENAMED 的 FROM「沒有這個」"),
        ("renamed-to-exists", "## RENAMED Requirements\n- FROM: `### Requirement: 取消訂單`\n- TO: `### Requirement: 建立訂單`\n", "spec.md:2：RENAMED 的 TO「建立訂單」在現行規格 specs/orders/spec.md 已經存在"),
        ("added-exists", "## ADDED Requirements\n### Requirement: 建立訂單\n#### Scenario: 一\n", "spec.md:2：ADDED 的需求「建立訂單」在現行規格 specs/orders/spec.md 已經存在"),
        ("two-blocks", "## MODIFIED Requirements\n### Requirement: 建立訂單\n#### Scenario: 一\n\n## REMOVED Requirements\n### Requirement: 建立訂單\n", "spec.md:2：需求「建立訂單」同時出現在 REMOVED 與 MODIFIED 區塊"),
        ("same-block-twice", "## ADDED Requirements\n### Requirement: 甲\n#### Scenario: 一\n### Requirement: 甲\n#### Scenario: 一\n", "spec.md:4：需求「甲」在 ADDED 區塊出現兩次"),
        ("rename-from-and-modified", "## RENAMED Requirements\n- FROM: `### Requirement: 取消訂單`\n- TO: `### Requirement: 撤回訂單`\n## MODIFIED Requirements\n### Requirement: 取消訂單\n#### Scenario: 一\n", "spec.md:5：需求「取消訂單」同時出現在 RENAMED 與 MODIFIED 區塊"),
        ("from-without-to", "## RENAMED Requirements\n- FROM: `### Requirement: 取消訂單`\n", "spec.md:2：FROM 沒有對應的 TO"),
        ("to-without-from", "## RENAMED Requirements\n- TO: `### Requirement: 撤回訂單`\n", "spec.md:2：TO 前面沒有對應的 FROM"),
        ("added-no-scenario", "## ADDED Requirements\n### Requirement: 申請退款\n只有說明。\n", "spec.md:2：需求「申請退款」沒有情境"),
        ("modified-no-scenario", "## MODIFIED Requirements\n### Requirement: 建立訂單\n只有說明。\n", "spec.md:2：需求「建立訂單」沒有情境"),
        ("outside-section", "### Requirement: 申請退款\n#### Scenario: 一\n", "spec.md:1：需求「申請退款」不在 ADDED／MODIFIED／REMOVED 任何一個區塊裡"),
        ("empty-delta", "# 只有標題\n", "spec.md:1：差異檔沒有任何 ADDED／MODIFIED／REMOVED／RENAMED 的內容"),
    ];
    for (tag, delta, want) in cases {
        let fx = Fx::new(&format!("s3-{tag}"));
        fx.change("c", &[("orders", delta)]);
        let (c, o, _) = fx.run(&["validate", "--change", "c"]);
        assert_eq!(c, 1, "{tag}：{o}");
        let line = format!("openspec/changes/c/specs/orders/{want}");
        assert!(o.lines().any(|l| l.starts_with(&line)), "{tag}：想要「{line}」\n{o}");
        let before = fx.snapshot();
        let (c, _, e) = fx.run(&["apply", "--change", "c", "--write", "--date", "2026-01-02"]);
        assert!(c == 1 && e.contains("拒絕 apply"), "{tag}：{e}");
        assert_eq!(fx.snapshot(), before, "{tag}：被拒絕時不能動任何檔");
    }
}

#[test]
fn s3_validate_spec_rules_and_missing_files() {
    let fx = Fx::new("s3-spec");
    fx.write("openspec/specs/login/spec.md", "### Requirement: 密碼登入\n只有說明。\n\n### Requirement: 第二個\n#### Scenario: 一\n#### Scenario: 一\n");
    fx.write("openspec/changes/no-files/specs/orders/spec.md", D_ADDED);
    let (c, o, e) = fx.run(&["validate"]);
    assert_eq!(c, 1);
    for want in [
        "openspec/specs/login/spec.md:1：需求「密碼登入」沒有情境",
        "openspec/specs/login/spec.md:6：情境「一」在同一個需求裡重複",
        "openspec/changes/no-files/proposal.md:1：變更缺 proposal.md",
        "openspec/changes/no-files/tasks.md:1：變更缺 tasks.md",
    ] {
        assert!(o.lines().any(|l| l.starts_with(want)), "想要「{want}」\n{o}");
    }
    assert!(e.contains("共 4 個問題"), "{e}");
    // REMOVED 不需要情境；MODIFIED 可以用 RENAMED 的新名字
    let fx = Fx::new("s3-ok");
    fx.change("c", &[("orders", &format!("{D_REMOVED}\n{D_RENAMED}\n## MODIFIED Requirements\n### Requirement: 撤回訂單\n#### Scenario: 出貨前\n- **THEN** 可以撤回\n"))]);
    let (c, o, _) = fx.run(&["validate"]);
    assert_eq!(c, 0, "{o}");
    let (c, _, e) = fx.run(&["validate", "--change", "沒有這個"]);
    assert!(c == 1 && e.contains("找不到變更"), "{e}");
}

// ───────────────────────── S3 apply ─────────────────────────

fn apply_one(tag: &str, delta: &str) -> (Fx, String) {
    let fx = Fx::new(tag);
    fx.change("c", &[("orders", delta)]);
    let before = fx.snapshot();
    let (c, preview, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02"]);
    assert_eq!(c, 0, "{e}");
    assert!(preview.contains("--- a/openspec/specs/orders/spec.md\n+++ b/openspec/specs/orders/spec.md\n@@ ") && preview.contains("沒有寫任何檔"), "{preview}");
    assert_eq!(fx.snapshot(), before, "沒有 --write 時目錄要逐位元不變");
    let (c, o, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02", "--write"]);
    assert_eq!(c, 0, "{o}{e}");
    assert!(fx.root().join("changes/archive/2026-01-02-c/specs/orders/spec.md").is_file() && !fx.root().join("changes/c").exists());
    let (c, o, e) = fx.run(&["validate"]);
    assert_eq!(c, 0, "apply 之後 validate 要過：{o}{e}");
    assert!(fx.snapshot().iter().all(|(n, _)| !n.contains(".awf-")), "不能留暫存檔");
    (fx, preview)
}

#[test]
fn s3_apply_added() {
    let (fx, preview) = apply_one("s3-added", D_ADDED);
    assert_eq!(titles(&fx, "orders"), ["建立訂單", "取消訂單", "傳真下單", "申請退款"]);
    assert!(preview.contains("+### Requirement: 申請退款\n") && !preview.lines().any(|l| l.starts_with('-') && !l.starts_with("---")), "{preview}");
    assert!(fx.read("openspec/specs/orders/spec.md").starts_with(ORDERS), "原本的內容不動，只加在結尾");
}

#[test]
fn s3_apply_modified() {
    let (fx, preview) = apply_one("s3-modified", D_MODIFIED);
    assert_eq!(titles(&fx, "orders"), ["建立訂單", "取消訂單", "傳真下單"]);
    let text = fx.read("openspec/specs/orders/spec.md");
    assert!(text.contains("並寄出確認信。") && !text.contains("庫存不足"), "整塊取代：{text}");
    assert!(preview.contains("-#### Scenario: 庫存不足\n"), "{preview}");
}

#[test]
fn s3_apply_removed() {
    let (fx, preview) = apply_one("s3-removed", D_REMOVED);
    assert_eq!(titles(&fx, "orders"), ["建立訂單", "取消訂單"]);
    assert!(preview.contains("-### Requirement: 傳真下單\n") && !preview.lines().any(|l| l.starts_with('+') && !l.starts_with("+++")), "{preview}");
    assert!(fx.read("openspec/specs/orders/spec.md").ends_with("- **THEN** 可以取消\n"));
}

#[test]
fn s3_apply_renamed() {
    let (fx, preview) = apply_one("s3-renamed", D_RENAMED);
    assert_eq!(titles(&fx, "orders"), ["建立訂單", "撤回訂單", "傳真下單"]);
    assert!(preview.contains("-### Requirement: 取消訂單\n+### Requirement: 撤回訂單\n"), "{preview}");
    assert_eq!(fx.read("openspec/specs/orders/spec.md"), ORDERS.replace("Requirement: 取消訂單", "Requirement: 撤回訂單"));
}

#[test]
fn s3_apply_mixed_new_capability_and_reproducible() {
    let mixed = format!("{D_ADDED}\n{D_MODIFIED}\n{D_REMOVED}\n{D_RENAMED}");
    let billing = "## Purpose\n示範客戶 ACME 的發票。\n\n## ADDED Requirements\n### Requirement: 開立發票\n#### Scenario: 付款完成\n- **THEN** 開立發票\n";
    let mut results = Vec::new();
    for tag in ["s3-mixed-a", "s3-mixed-b"] {
        let fx = Fx::new(tag);
        fx.change("add-refund", &[("orders", &mixed), ("billing", billing), ("login", &D_ADDED.replace("申請退款", "簡訊驗證"))]);
        let before = fx.snapshot();
        let (c, preview, e) = fx.run(&["apply", "--change", "add-refund", "--date", "2026-01-02"]);
        assert_eq!(c, 0, "{e}");
        assert_eq!(fx.snapshot(), before);
        assert!(preview.contains("--- /dev/null\n+++ b/openspec/specs/billing/spec.md\n@@ -0,0 +1,"), "{preview}");
        assert!(preview.contains("歸檔：openspec/changes/add-refund → openspec/changes/archive/2026-01-02-add-refund\n"));
        let (c, o, e) = fx.run(&["apply", "--change", "add-refund", "--date", "2026-01-02", "--write"]);
        assert_eq!(c, 0, "{o}{e}");
        assert_eq!(titles(&fx, "orders"), ["建立訂單", "撤回訂單", "申請退款"]);
        assert_eq!(fx.read("openspec/specs/billing/spec.md"), "# billing Specification\n\n## Purpose\n示範客戶 ACME 的發票。\n\n## Requirements\n\n### Requirement: 開立發票\n#### Scenario: 付款完成\n- **THEN** 開立發票\n");
        let login = fx.read("openspec/specs/login/spec.md");
        assert!(login.ends_with("- **THEN** 受理退款\r\n") && !login.replace("\r\n", "").contains('\n'), "CRLF 的檔維持 CRLF");
        assert_eq!(fx.run(&["validate"]).0, 0);
        // 第二次：明確說已歸檔，不重做
        let after = fx.snapshot();
        let (c, _, e) = fx.run(&["apply", "--change", "add-refund", "--date", "2026-01-03", "--write"]);
        assert!(c == 1 && e.contains("已歸檔（openspec/changes/archive/2026-01-02-add-refund），不重做"), "{e}");
        assert_eq!(fx.snapshot(), after);
        results.push((preview, after));
    }
    assert_eq!(results[0], results[1], "同樣的輸入要得到逐位元相同的結果");
}

#[test]
fn s3_apply_refuses_when_archive_target_exists_or_date_is_bad() {
    let fx = Fx::new("s3-target");
    fx.change("c", &[("orders", D_ADDED)]);
    fs::create_dir_all(fx.root().join("changes/archive/2026-01-02-c")).unwrap();
    let before = fx.snapshot();
    let (c, _, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02", "--write"]);
    assert!(c == 1 && e.contains("歸檔目錄 changes/archive/2026-01-02-c 已經存在"), "{e}");
    for d in ["2026-02-30", "2026-1-2", "明天", "2026-01-02/../x"] {
        let (c, _, e) = fx.run(&["apply", "--change", "c", "--date", d, "--write"]);
        assert!(c == 1 && e.contains("--date"), "{d}：{e}");
    }
    assert_eq!(fx.snapshot(), before);
    // 沒給 --date 用系統日期
    let fx = Fx::new("s3-today");
    fx.change("c", &[("orders", D_ADDED)]);
    let (c, o, _) = fx.run(&["apply", "--change", "c"]);
    assert_eq!(c, 0);
    let line = o.lines().find(|l| l.starts_with("歸檔：")).unwrap();
    let name = line.rsplit('/').next().unwrap();
    assert!(name.len() == "2026-01-02-c".len() && name.ends_with("-c") && spec::check_date(&name[..10]).is_ok(), "{line}");
}

// ───────────────────────── 守門 ─────────────────────────

#[test]
fn guard_rejects_out_of_bounds_names_and_roots() {
    let fx = Fx::new("guard-names");
    fx.change("c", &[("orders", D_ADDED)]);
    let before = fx.snapshot();
    for name in ["../c", "c/../c", "a/b", "a\\b", "..", ".", "archive", ".hidden", ""] {
        for sub in ["apply", "validate"] {
            let (c, _, e) = fx.run(&[sub, "--change", name]);
            assert!(c != 0 && e.contains("變更名稱不合法"), "{sub} {name}：{e}");
        }
    }
    for root in ["../openspec", "openspec/../openspec", "a/../../b", ""] {
        let (c, _, e) = fx.run(&["status", "--root", root]);
        assert!(c != 0 && e.contains("--root"), "{root}：{e}");
    }
    assert_eq!(fx.snapshot(), before);
    // 絕對路徑可以（規格在別的 repo）
    let abs = fx.root().to_string_lossy().to_string();
    let (c, o, e) = fx.run(&["validate", "--root", &abs]);
    assert_eq!(c, 0, "{o}{e}");
    // 不認得的選項與子指令
    assert_eq!(fx.run(&["status", "--write"]).0, 2);
    assert_eq!(fx.run(&["nope"]).0, 2);
    assert_eq!(fx.run(&["show"]).0, 1);
    assert_eq!(fx.run(&["show", "orders/沒有"]).0, 1);
}

#[cfg(unix)]
#[test]
fn guard_does_not_follow_symlinks_out_of_root() {
    use std::os::unix::fs::symlink;
    let fx = Fx::new("guard-symlink");
    fx.write("outside/secret/spec.md", "### Requirement: 外面的需求\n#### Scenario: 一\n");
    fx.write("outside/evil-change/proposal.md", "x\n");
    symlink(fx.base.join("outside/secret"), fx.root().join("specs/secret")).unwrap();
    fs::create_dir_all(fx.root().join("changes")).unwrap();
    symlink(fx.base.join("outside/evil-change"), fx.root().join("changes/evil")).unwrap();
    symlink(fx.base.join("outside/secret/spec.md"), fx.root().join("specs/login/spec.md.link")).unwrap();
    let (c, o, e) = fx.run(&["status"]);
    assert_eq!(c, 0);
    assert!(!o.contains("secret") && !o.contains("evil") && !o.contains("外面的需求"), "{o}");
    assert!(e.contains("略過 openspec/specs/secret") && e.contains("略過 openspec/changes/evil"), "{e}");

    // 差異指向一個「spec.md 是指到外面的符號連結」的能力：不讀也不覆寫
    fs::create_dir_all(fx.root().join("specs/linked")).unwrap();
    symlink(fx.base.join("outside/secret/spec.md"), fx.root().join("specs/linked/spec.md")).unwrap();
    fx.change("c", &[("linked", D_ADDED)]);
    let outside_before = fx.read("outside/secret/spec.md");
    let (c, _, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02", "--write"]);
    assert!(c == 1 && e.contains("不覆寫"), "{e}");
    assert_eq!(fx.read("outside/secret/spec.md"), outside_before);
    assert!(fx.root().join("changes/c").is_dir());
    // ids.json 是指到外面的符號連結：不讀
    symlink(fx.base.join("outside/secret/spec.md"), fx.root().join("ids.json")).unwrap();
    let (c, _, e) = fx.run(&["status"]);
    assert!(c == 1 && e.contains("ids.json"), "{e}");
}

#[cfg(unix)]
#[test]
fn guard_failed_write_is_rolled_back() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new("guard-rollback");
    fx.change("c", &[("orders", &format!("{D_ADDED}\n{D_REMOVED}")), ("billing", &D_ADDED.replace("申請退款", "開立發票"))]);
    let changes = fx.root().join("changes");
    fs::create_dir_all(changes.join("archive")).unwrap();
    let before = fx.snapshot();
    // changes/ 唯讀：規格檔換得掉，但最後一步（把變更搬進 archive）會失敗
    fs::set_permissions(&changes, fs::Permissions::from_mode(0o555)).unwrap();
    let writable_anyway = fs::write(changes.join("probe"), "x").is_ok();
    if writable_anyway {
        // 以 root 身分跑測試時權限擋不住，這個情境測不了（還原的每一步另有單元測試）
        let _ = fs::remove_file(changes.join("probe"));
        fs::set_permissions(&changes, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let (c, _, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02", "--write"]);
    fs::set_permissions(&changes, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(c, 1);
    assert!(e.contains("搬不了") && e.contains("已經還原"), "{e}");
    assert_eq!(fx.snapshot(), before, "失敗後要和執行前逐位元相同（新建的 billing 目錄也要收掉）");
    // 權限恢復後可以正常完成
    let (c, o, e) = fx.run(&["apply", "--change", "c", "--date", "2026-01-02", "--write"]);
    assert_eq!(c, 0, "{o}{e}");
    assert_eq!(fx.run(&["validate"]).0, 0);
}

#[test]
fn status_json_and_selftest() {
    let fx = Fx::new("status-json");
    fx.change("add-refund", &[("orders", &format!("{D_ADDED}\n{D_RENAMED}"))]);
    let (c, o, _) = fx.run(&["status", "--json"]);
    assert_eq!(c, 0);
    let v: Value = serde_json::from_str(&o).unwrap();
    assert_eq!(v["capabilities"][1]["name"], "orders");
    assert_eq!(v["capabilities"][1]["requirements"], 3);
    assert_eq!(v["capabilities"][1]["scenarios"], 4);
    assert_eq!(v["changes"][0]["tasks"]["done"], 1);
    assert_eq!(v["changes"][0]["requirements"][0], serde_json::json!({"op": "RENAMED", "id": "orders/取消訂單", "title": "撤回訂單"}));
    assert_eq!(v["changes"][0]["requirements"][1]["id"], "orders/申請退款");
    assert_eq!(v["pending_ids"], serde_json::json!([]));
    let (c, o, e) = spec::run_captured(&["--selftest".to_string()], Path::new("."));
    assert_eq!(c, 0, "{o}{e}");
    assert!(o.lines().last().unwrap().ends_with("通過") && !o.contains("[失敗]"));
}
