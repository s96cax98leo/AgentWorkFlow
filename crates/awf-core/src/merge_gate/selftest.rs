//! `awf merge-gate --selftest`：照 0.x 的 Python 版 selftest 的案例與順序，在暫存專案裡實際跑 `check`。
//! 輸出的每一行（案例名、判定、預期）和 0.x 的 Python 版相同。

use super::{exit_code, parse_diff, pyutil, read_audit, run, Ctx};
use crate::pyfmt::{self, repr_str};
use crate::recipe;
use crate::schema::Schemas;
use serde_json::{json, Value};
use std::process::Command;

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TS: &str = "2099-10-24T09:00:00+00:00";

#[derive(Default)]
struct Case<'a> {
    cfg: Option<Value>,
    files: Vec<(&'a str, Value)>,
    args: Vec<String>,
    followup: Option<&'a str>,
    audit: bool,
    why: Option<&'a str>,
}

struct St<'a> {
    ctx: &'a Ctx,
    proj: String,
    out: &'a mut String,
    ok: bool,
    base_cfg: Value,
    defaults: Vec<(&'static str, Value)>,
}

fn row(rid: &str, text: &str, result: &str) -> Value {
    json!({"requirement_id": rid, "requirement": text, "evidence": [format!("results.json {rid}")], "result": result})
}

struct Dec<'a> {
    dec: &'a str,
    followups: Value,
    oos: Value,
    sha: &'a str,
    repo: &'a str,
}

impl Default for Dec<'_> {
    fn default() -> Self {
        Dec { dec: "merge", followups: json!([]), oos: json!([]), sha: SHA, repo: "acme-fork/app" }
    }
}

fn decision(rows: &Value, d: Dec) -> Value {
    json!({"status": "done", "summary": "示範", "artifacts": [], "evidence": [], "confidence": 0.9,
        "open_questions": [], "derived_work": [],
        "payload": {"decision": d.dec, "repo": d.repo, "pr": 3, "head_sha": d.sha, "requirements_source": "json",
            "traceability": rows, "out_of_scope": d.oos, "followups": d.followups,
            "risk_tier": "low", "gate_reasons": []}})
}

/// 證據要帶驗證時的 commit，才能綁 head SHA。
fn evw(items: &Value, commit: &str) -> Value {
    json!({"commit": commit, "results": items})
}

/// 行數和 hunk 標頭一致的 diff（parse_diff 會核對行數）。
fn mk_diff(path: &str, plus: &[&str], minus: &[&str]) -> String {
    let mut s = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{},{} +1,{} @@\n", if minus.is_empty() { 0 } else { 1 }, minus.len(), plus.len());
    for l in minus { s.push_str(&format!("-{l}\n")); }
    for l in plus { s.push_str(&format!("+{l}\n")); }
    s
}

fn merged(base: &Value, key: &str, patch: Value) -> Value {
    let mut out = base.clone();
    if let (Some(dst), Some(src)) = (out.get_mut(key).and_then(Value::as_object_mut), patch.as_object()) {
        for (k, v) in src { dst.insert(k.clone(), v.clone()); }
    }
    out
}

fn with(base: &Value, key: &str, value: Value) -> Value {
    let mut out = base.clone();
    out[key] = value;
    out
}

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

impl St<'_> {
    fn path(&self, rel: &str) -> String {
        format!("{}/{rel}", self.proj)
    }

    fn write(&self, rel: &str, body: &Value) -> Result<(), String> {
        let text = match body {
            Value::String(s) => s.clone(),
            other => recipe::dumps(other, None),
        };
        let p = self.path(rel);
        std::fs::create_dir_all(pyutil::parent(&p)).and_then(|_| std::fs::write(&p, text)).map_err(|e| format!("寫不進 {p}：{e}"))
    }

    fn line(&mut self, good: bool, text: &str) {
        self.out.push_str(&format!("  [{}] {text}\n", if good { "通過" } else { "失敗" }));
        self.ok = self.ok && good;
    }

    fn run(&mut self, name: &str, want: &[&str], c: Case) -> Result<Value, String> {
        self.write(".claude/teams/guard.config.json", c.cfg.as_ref().unwrap_or(&self.base_cfg))?;
        for (f, body) in self.defaults.iter().filter(|(f, _)| !c.files.iter().any(|(g, _)| g == f)) { self.write(f, body)?; }
        for (f, body) in &c.files { self.write(f, body)?; }
        let mut argv = strs(&["check", "--project-dir", &self.proj, "--decision", &self.path("decision.json"), "--repo", "acme-fork/app",
            "--pr", "3", "--head-sha", SHA, "--evidence", &self.path("evidence.json"), "--checks", &self.path("checks.json"),
            "--diff", &self.path("diff.txt"), "--tests-before", "10", "--tests-after", "11", "--timestamp", TS]);
        if !c.audit { argv.push("--no-audit".into()); }
        argv.extend(c.args);
        let ctx = Ctx { teams: self.ctx.teams.clone(), project_dir_env: None, env: self.ctx.env.clone(), stdin: Some(String::new()) };
        let (mut out, mut errs) = (String::new(), String::new());
        let code = run(&ctx, &argv, &mut out, &mut errs);
        let mut res: Value = serde_json::from_str(&out).unwrap_or_else(|_| json!({"decision": "?", "reasons": [format!("{}{}", pyutil::tail(&out, 200), pyutil::tail(&errs, 300))]}));
        let got = res["decision"].as_str().unwrap_or("?").to_string();
        let mut good = want.contains(&got.as_str()) && got != "?" && code == exit_code(&got);
        let mut notes: Vec<Value> = Vec::new();
        if let Some(fu) = c.followup {
            if !pyfmt::items(res.get("followups")).iter().any(|f| f["requirement_id"] == fu) {
                good = false;
                notes.push(json!(format!("缺 {fu} 的 followup")));
            }
        }
        if let Some(why) = c.why {
            // 判定要因為這條理由，不是剛好被別的理由擋下
            if !pyfmt::items(res.get("reasons")).iter().any(|r| r.as_str().is_some_and(|r| r.contains(why))) {
                good = false;
                notes.push(json!(format!("理由裡沒有「{why}」")));
            }
        }
        if let Some(r) = res.get_mut("reasons").and_then(Value::as_array_mut) { r.extend(notes); }
        let tail = if good { String::new() } else { format!(" {}", pyfmt::repr(&res["reasons"])) };
        self.line(good, &format!("{name}：{got}（預期 {}）{tail}", want.join("／")));
        Ok(res)
    }
}

fn git(ctx: &Ctx, dir: &str, args: &[&str]) -> Result<String, String> {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).args(["-c", "user.name=示範", "-c", "user.email=demo@example.com", "-c", "commit.gpgsign=false"]).args(args);
    for (k, v) in &ctx.env { c.env(k, v); }
    match pyutil::run(&mut c, None, None) {
        Ok(r) if r.code == 0 => Ok(pyutil::strip(&pyutil::text(&r.stdout)).to_string()),
        Ok(r) => Err(format!("git {args:?} 失敗：{}", pyutil::text(&r.stderr))),
        Err(_) => Err(format!("執行不了 git {args:?}")),
    }
}

/// `gen_cmd` 是「印出需求清單 JSON」的指令（字串或陣列），給 command 需求來源的案例用。回傳 exit code。
pub fn selftest(ctx: &Ctx, gen_cmd: &Value, out: &mut String) -> i32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("awf-merge-gate-selftest-{}-{nanos}", std::process::id()));
    let proj = dir.to_string_lossy().into_owned();
    let made = std::fs::create_dir_all(dir.join(".claude/teams")).map_err(|e| format!("建不了暫存目錄 {proj}：{e}"));
    let result = made.and_then(|_| body(ctx, gen_cmd, &proj, out));
    let _ = std::fs::remove_dir_all(&dir);
    let ok = match result {
        Ok(ok) => ok,
        Err(e) => {
            out.push_str(&format!("  [失敗] selftest 中斷：{e}\n"));
            false
        }
    };
    out.push_str(&format!("merge_gate selftest：{}\n", if ok { "全部通過" } else { "有失敗" }));
    if ok { 0 } else { 1 }
}

fn body(ctx: &Ctx, gen_cmd: &Value, proj: &str, out: &mut String) -> Result<bool, String> {
    let reqs = json!([{"id": "REQ-1", "text": "匯出按鈕下載 CSV", "source": "示範客戶 ACME Story"},
        {"id": "REQ-2", "text": "超過一萬筆時提示縮小範圍", "source": "示範客戶 ACME Story"}]);
    let base_cfg = json!({"runs_dir": "_tpm/runs", "upstream_repos": "acme-up/app",
        "auto_merge": {"enabled": true, "repos": {"acme-fork/app": "medium", "acme-up/app": "low"}, "daily_cap": 2},
        "requirements": {"source": "json", "path": "reqs.json"}});
    let good_rows = json!([row("REQ-1", "匯出按鈕下載 CSV", "match"), row("REQ-2", "超過一萬筆時提示縮小範圍", "match")]);
    let good_ev = json!([{"requirement_id": "REQ-1", "result": "pass", "evidence": "1 passed"}, {"requirement_id": "REQ-2", "result": "pass", "evidence": "1 passed"}]);
    let test_lines = ["def test_csv():", "    assert export() == 'csv'"];
    let test_diff = mk_diff("tests/test_export.py", &test_lines, &[]);
    let defaults = vec![
        ("decision.json", decision(&good_rows, Dec::default())),
        ("evidence.json", evw(&good_ev, SHA)),
        ("checks.json", json!([{"name": "ci", "state": "SUCCESS"}])),
        ("diff.txt", json!(test_diff)),
    ];
    let mut s = St { ctx, proj: proj.to_string(), out, ok: true, base_cfg: base_cfg.clone(), defaults };
    s.write("reqs.json", &reqs)?;
    s.write("specs/demo/spec.md", &json!("# 示範需求\n\n## 驗收條件\n\n- **AC-1**：匯出按鈕下載 CSV\n- **AC-2**：超過一萬筆時提示縮小範圍\n\n參考 AC-1 的說明（不是新條目）\n"))?;
    let am = |patch: Value| merged(&base_cfg, "auto_merge", patch);
    let files = |list: Vec<(&'static str, Value)>| Case { files: list, ..Case::default() };
    let diff_case = |text: String| files(vec![("diff.txt", json!(text))]);
    let f = |rel: &str| format!("{proj}/{rel}");

    s.out.push_str("merge_gate selftest\n");
    s.run("traceability 全部一致（json 需求來源）→ merge", &["merge"], Case::default())?;
    let spec_rows = json!([row("AC-1", "匯出按鈕下載 CSV", "match"), row("AC-2", "超過一萬筆時提示縮小範圍", "match")]);
    let spec_ev = json!([{"requirement_id": "AC-1", "result": "pass", "evidence": "ok"}, {"requirement_id": "AC-2", "result": "pass", "evidence": "ok"}]);
    s.run("traceability 全部一致（spec-md 範例 adapter）→ merge", &["merge"], Case {
        cfg: Some(with(&base_cfg, "requirements", json!({"source": "spec-md", "path": "specs/demo/spec.md"}))),
        files: vec![("decision.json", decision(&spec_rows, Dec::default())), ("evidence.json", evw(&spec_ev, SHA))], ..Case::default() })?;
    let issue_files = || vec![("decision.json", decision(&json!([row("ISSUE-7", "登入頁顯示錯誤訊息", "match")]), Dec::default())),
        ("evidence.json", evw(&json!([{"requirement_id": "ISSUE-7", "result": "pass", "evidence": "e2e ok"}]), SHA))];
    s.run("traceability 全部一致（command 需求來源：團隊自訂腳本）→ merge", &["merge"], Case {
        cfg: Some(with(&base_cfg, "requirements", json!({"source": "command", "command": gen_cmd}))), files: issue_files(), ..Case::default() })?;
    // guard.config.json 沒設 requirements（path、command 都空）時，改讀開發規範 profile（methodology.json）的需求來源
    s.write(".claude/teams/methodology.json", &json!({"name": "acme-demo", "requirements": {"source": "command", "command": gen_cmd}}))?;
    let res = s.run("traceability 全部一致（需求來源取自 methodology profile：command 需求來源）→ merge", &["merge"], Case {
        cfg: Some(with(&base_cfg, "requirements", json!({"source": "json", "path": "", "command": ""}))), files: issue_files(), ..Case::default() })?;
    let src = res.get("requirements_source").map(pyfmt::str_of).unwrap_or_else(|| "None".into());
    s.line(res["requirements_source"].as_str().unwrap_or("").starts_with("command"), &format!("methodology profile 的需求來源被採用（requirements_source={src}）"));
    s.run("guard.config.json 有設 requirements 時優先於 methodology profile（json 需求來源）→ merge", &["merge"], Case::default())?;
    std::fs::remove_file(f(".claude/teams/methodology.json")).map_err(|e| e.to_string())?;
    let fu = json!([{"requirement_id": "REQ-2", "expected": "提示縮小範圍", "actual": "沒有提示", "adjustment": "補提示"}]);
    s.run("traceability mismatch（steward 判 hold）→ hold 並產出 followup", &["hold"], Case {
        files: vec![("decision.json", decision(&json!([good_rows[0], row("REQ-2", "超過一萬筆時提示縮小範圍", "mismatch")]), Dec { dec: "hold", followups: fu, ..Dec::default() }))],
        followup: Some("REQ-2"), ..Case::default() })?;
    s.run("traceability mismatch（steward 宣稱 merge、驗證卻 fail）→ hold 並產出 followup", &["hold"], Case {
        files: vec![("evidence.json", evw(&json!([good_ev[0], {"requirement_id": "REQ-2", "result": "fail", "evidence": "沒有提示"}]), SHA))],
        followup: Some("REQ-2"), ..Case::default() })?;
    s.run("traceability 漏列（需求清單有 REQ-2，對照表沒有）→ hold", &["hold"], Case {
        files: vec![("decision.json", decision(&json!([good_rows[0]]), Dec::default()))], followup: Some("REQ-2"), ..Case::default() })?;
    s.run("traceability 證據缺這條需求 → hold", &["hold"], Case { files: vec![("evidence.json", evw(&json!([good_ev[0]]), SHA))], followup: Some("REQ-2"), ..Case::default() })?;
    let oos = json!([{"location": "src/billing.py", "change": "改了計費"}]);
    s.run("traceability out_of_scope（對不到需求的改動）→ hold", &["hold"], Case {
        files: vec![("decision.json", decision(&good_rows, Dec { dec: "hold", followups: json!([{"requirement_id": "out_of_scope", "expected": "x", "actual": "y", "adjustment": "z"}]), oos, ..Dec::default() }))],
        followup: Some("out_of_scope"), ..Case::default() })?;
    let b40 = "b".repeat(40);
    s.run("head SHA 不符（對照時與目前不同）→ hold", &["hold"], files(vec![("decision.json", decision(&good_rows, Dec { sha: &b40, ..Dec::default() }))]))?;
    s.run("head SHA 不符（審查時與目前不同）→ hold", &["hold"], Case { args: strs(&["--reviewed-sha", &"c".repeat(40)]), ..Case::default() })?;
    s.run("測試數減少 → hold", &["hold"], Case { args: strs(&["--tests-before", "12"]), ..Case::default() })?;
    let skip_diff = mk_diff("tests/test_export.py", &[test_lines[0], test_lines[1], "@pytest.mark.skip(reason='flaky')", "def test_big():", "    pass"], &[]);
    s.run("新增 skip → hold", &["hold"], diff_case(skip_diff))?;
    s.run("新增 it.only → hold", &["hold"], diff_case(test_diff.replace("+def test_csv():", "+it.only('csv', () => {})")))?;
    let ci_diff = "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1 +1 @@\n-on: push\n+on: pull_request\n";
    s.run("改 CI 檔（.github/workflows）→ escalate", &["escalate", "hold"], diff_case(ci_diff.into()))?;
    let lock_diff = "diff --git a/package-lock.json b/package-lock.json\n--- a/package-lock.json\n+++ b/package-lock.json\n@@ -1 +1 @@\n-a\n+b\n";
    s.run("改依賴鎖檔 → escalate", &["escalate"], diff_case(lock_diff.into()))?;
    let jest_diff = "diff --git a/jest.config.js b/jest.config.js\n--- a/jest.config.js\n+++ b/jest.config.js\n@@ -1 +1 @@\n-coverageThreshold: 90\n+coverageThreshold: 50\n";
    s.run("改測試／覆蓋率設定（jest.config.js）→ escalate", &["escalate"], diff_case(jest_diff.into()))?;
    s.run("新增 # noqa → escalate", &["escalate"], diff_case(mk_diff("tests/test_export.py", &[test_lines[0], test_lines[1], "import os  # noqa: F401"], &[])))?;
    s.run("新增 eslint-disable → escalate", &["escalate"], diff_case(test_diff.replace("+def test_csv():", "+// eslint-disable-next-line no-unused-vars")))?;
    // ── diff 解析（改名前後路徑、刪除、引號路徑、「++ 」開頭的新增行、解析不了一律 hold）
    let mv_out = "diff --git a/.github/workflows/deploy.yml b/docs/deploy.md\nsimilarity index 100%\nrename from .github/workflows/deploy.yml\nrename to docs/deploy.md\n";
    s.run("把敏感檔改名移出 .github（只有 rename，0 行）→ escalate（看舊路徑）", &["escalate"], Case {
        why: Some("改到高風險路徑：.github/workflows/deploy.yml"), ..diff_case(format!("{test_diff}{mv_out}")) })?;
    let q = r#""a/.github/\346\270\254\350\251\246.yml" "b/docs/\346\270\254\350\251\246.md""#;
    let mv_quoted = format!("diff --git {q}\nsimilarity index 100%\nrename from \".github/\\346\\270\\254\\350\\251\\246.yml\"\nrename to \"docs/\\346\\270\\254\\350\\251\\246.md\"\n");
    s.run("引號路徑（非 ASCII 檔名）的敏感檔改名移出 → escalate（還原 octal 跳脫）", &["escalate"], Case {
        why: Some("改到高風險路徑：.github/測試.yml"), ..diff_case(format!("{test_diff}{mv_quoted}")) })?;
    let rm_sens = "diff --git a/.github/CODEOWNERS b/.github/CODEOWNERS\ndeleted file mode 100644\nindex 1111111..0000000\n--- a/.github/CODEOWNERS\n+++ /dev/null\n@@ -1 +0,0 @@\n-* @acme/owners\n";
    s.run("刪除敏感檔（CODEOWNERS）→ escalate", &["escalate"], Case { why: Some("改到高風險路徑：.github/CODEOWNERS"), ..diff_case(format!("{test_diff}{rm_sens}")) })?;
    let qskip = "diff --git \"a/tests/\\346\\270\\254\\350\\251\\246.py\" \"b/tests/\\346\\270\\254\\350\\251\\246.py\"\nnew file mode 100644\n--- /dev/null\n+++ \"b/tests/\\346\\270\\254\\350\\251\\246.py\"\n@@ -0,0 +1,2 @@\n+@pytest.mark.skip(reason='flaky')\n+def test_x(): pass\n";
    s.run("引號路徑（非 ASCII 測試檔）新增 skip → hold（新增行不能被忽略）", &["hold"], Case { why: Some("tests/測試.py: @pytest.mark.skip"), ..diff_case(format!("{test_diff}{qskip}")) })?;
    let plus3 = mk_diff("tests/test_export.py", &[test_lines[0], test_lines[1], "++ it.only('csv', () => {})"], &[]);
    s.run("新增行內容以「++ 」開頭（整行是 +++）不能當成檔案標頭 → hold", &["hold"], Case { why: Some("新增 skip／only／fixme"), ..diff_case(plus3) })?;
    s.run("解析不了的 diff --git 標頭（沒有 a/ b/ 前綴）→ hold", &["hold"], Case {
        why: Some("解析不了的 diff --git 標頭"), ..diff_case(format!("{test_diff}diff --git x.py y.py\n--- x.py\n+++ y.py\n@@ -0,0 +1 @@\n+print(1)\n")) })?;
    s.run("diff 在 hunk 中間被截斷 → hold", &["hold"], Case { why: Some("可能被截斷"), ..diff_case(test_diff.replace("+1,2 @@", "+1,5 @@")) })?;
    let pd = parse_diff(concat!("diff --git \"a/caf\\303\\251 x\\t.txt\" \"b/caf\\303\\251 x\\t.txt\"\n--- \"a/caf\\303\\251 x\\t.txt\"\n",
        "+++ \"b/caf\\303\\251 x\\t.txt\"\n@@ -0,0 +1 @@\n+hi\n",
        "diff --git a/my file.txt b/my file.txt\n--- a/my file.txt\n+++ b/my file.txt\n@@ -1 +1 @@\n-a\n+b\n"))?;
    let pv = pd.to_value();
    let good = pv["files"] == json!({"café x\t.txt": [1, 0], "my file.txt": [1, 1]}) && pd.errors.is_empty()
        && pv["added_lines"] == json!([["café x\t.txt", "hi"], ["my file.txt", "b"]]);
    s.line(good, &format!("parse_diff 還原引號路徑的 octal 與 \\t 跳脫、檔名含空白（{} {}）", pyfmt::repr(&pv["files"]), pyfmt::repr(&pv["errors"])));

    // rebase 後沿用 verdict：在暫存 git repo 做 feature commit A，main 前進後 cherry-pick 成 A2（patch 相同），另做 C（patch 不同）
    let gd = f("gitrepo");
    std::fs::create_dir_all(&gd).map_err(|e| e.to_string())?;
    let g = |args: &[&str]| git(ctx, &gd, args);
    let put = |name: &str, text: &str| std::fs::write(format!("{gd}/{name}"), text).map_err(|e| e.to_string());
    g(&["init", "-q"])?;
    put("a.txt", "1\n")?;
    g(&["add", "-A"])?;
    g(&["commit", "-qm", "base"])?;
    let base0 = g(&["rev-parse", "HEAD"])?;
    put("a.txt", "1\n2\n")?;
    g(&["commit", "-qam", "A"])?;
    let sha_a = g(&["rev-parse", "HEAD"])?;
    g(&["checkout", "-q", &base0])?;
    put("b.txt", "x\n")?;
    g(&["add", "-A"])?;
    g(&["commit", "-qm", "main 前進"])?;
    let base1 = g(&["rev-parse", "HEAD"])?;
    g(&["cherry-pick", &sha_a])?;
    let sha_a2 = g(&["rev-parse", "HEAD"])?;
    g(&["checkout", "-q", &base1])?;
    put("a.txt", "1\n3\n")?;
    g(&["commit", "-qam", "C"])?;
    let sha_c = g(&["rev-parse", "HEAD"])?;
    let carry_cfg = am(json!({"carry_verdict_on_rebase": true}));
    let rb = strs(&["--head-sha", &sha_a2, "--base-sha", &base1, "--git-dir", &gd]);
    let old_files = || vec![("decision.json", decision(&good_rows, Dec { sha: &sha_a, ..Dec::default() })), ("evidence.json", evw(&good_ev, &sha_a))];
    let res = s.run("rebase 後 patch-id 相同、carry 開啟 → merge（沿用 verdict）", &["merge"], Case { cfg: Some(carry_cfg.clone()), files: old_files(), args: rb.clone(), ..Case::default() })?;
    let good = pyfmt::items(res.get("reasons")).iter().any(|r| r.as_str().is_some_and(|r| r.contains("沿用 verdict")));
    s.line(good, &format!("沿用 verdict 寫進 reasons（{}）", res.get("reasons").map(pyfmt::repr).unwrap_or_else(|| "None".into())));
    s.run("rebase 後 patch-id 相同、carry 關閉 → hold", &["hold"], Case { files: old_files(), args: rb.clone(), ..Case::default() })?;
    s.run("rebase 後 patch 不同、carry 開啟 → hold", &["hold"], Case { cfg: Some(carry_cfg.clone()), files: old_files(),
        args: strs(&["--head-sha", &sha_c, "--base-sha", &base1, "--git-dir", &gd]), ..Case::default() })?;
    s.run("carry 開啟但沒給 base → hold", &["hold"], Case { cfg: Some(carry_cfg.clone()), files: old_files(), args: strs(&["--head-sha", &sha_a2, "--git-dir", &gd]), ..Case::default() })?;
    let d40 = "d".repeat(40);
    s.run("carry 開啟但舊 commit 不存在（算不出 patch-id）→ hold", &["hold"], Case { cfg: Some(carry_cfg),
        files: vec![("decision.json", decision(&good_rows, Dec { sha: &d40, ..Dec::default() }))], args: rb, ..Case::default() })?;
    s.run("required check 失敗 → hold", &["hold"], files(vec![("checks.json", json!([{"name": "ci", "state": "FAILURE"}]))]))?;
    s.run("驗證有 anti_cheat 發現 → hold", &["hold"], files(vec![("evidence.json", json!({"commit": SHA, "anti_cheat": ["tests/test_export.py:3 斷言被放寬"], "results": good_ev}))]))?;
    let finding = |id: &str, sev: &str| json!({"lens": "bugs", "findings": [{"id": id, "severity": sev, "claim": "c", "location": "a:1", "recommendation": "r", "confidence": 0.9}]});
    let fpath = f("findings.json");
    s.run("留存 important 發現 → hold", &["hold"], Case { args: strs(&["--findings", &fpath, "--reviewed-sha", SHA]), files: vec![("findings.json", finding("B1", "important"))], ..Case::default() })?;
    s.run("repo 不在白名單 → escalate", &["escalate"], Case { cfg: Some(with(&base_cfg, "auto_merge", with(&base_cfg["auto_merge"], "repos", json!({})))), ..Case::default() })?;
    s.run("required checks 輸入讀不到（fail-closed）→ hold", &["hold"], files(vec![("checks.json", json!("not json"))]))?;
    // ── SHA 綁定的輸入缺漏一律 hold
    s.run("證據沒有 commit（無法綁定 head SHA）→ hold", &["hold"], Case { why: Some("驗證證據沒有 commit"), ..files(vec![("evidence.json", good_ev.clone())]) })?;
    s.run("證據的 commit 是 n/a（等同沒有 commit）→ hold", &["hold"], Case { why: Some("驗證證據沒有 commit"), ..files(vec![("evidence.json", evw(&good_ev, "n/a"))]) })?;
    s.run("有 findings 卻沒有 reviewed-sha → hold", &["hold"], Case { args: strs(&["--findings", &fpath]), files: vec![("findings.json", finding("N1", "nit"))], why: Some("沒有 reviewed-sha"), ..Case::default() })?;
    s.run("有 findings 且 reviewed-sha 等於 head（只有 nit）→ merge", &["merge"], Case { args: strs(&["--findings", &fpath, "--reviewed-sha", SHA]), files: vec![("findings.json", finding("N1", "nit"))], ..Case::default() })?;
    s.run("SHA 不符（驗證證據的 commit 與目前 head 不同）→ hold", &["hold"], Case { why: Some("head SHA 不符：驗證時"), ..files(vec![("evidence.json", evw(&good_ev, &"e".repeat(40)))]) })?;
    // ── skipped／neutral 預設不算通過
    let checks = |name: &str, conclusion: &str| vec![("checks.json", json!([{"name": "ci", "state": "SUCCESS"}, {"name": name, "conclusion": conclusion}]))];
    s.run("skipped 不算通過（required check 被跳過）→ hold", &["hold"], Case { files: checks("e2e", "skipped"), why: Some("被跳過或 neutral"), ..Case::default() })?;
    s.run("neutral 也不算通過 → hold", &["hold"], Case { files: checks("lint", "neutral"), why: Some("被跳過或 neutral"), ..Case::default() })?;
    let allow_cfg = am(json!({"allowed_skipped_checks": ["e2e"]}));
    s.run("allowed_skipped_checks 有列的 check 被跳過 → merge", &["merge"], Case { cfg: Some(allow_cfg.clone()), files: checks("e2e", "skipped"), ..Case::default() })?;
    s.run("allowed_skipped_checks 沒列的 check 被跳過 → hold", &["hold"], Case { cfg: Some(allow_cfg.clone()), files: checks("lint", "skipped"), why: Some("被跳過或 neutral"), ..Case::default() })?;
    s.run("allowed_skipped_checks 不放行 failure → hold", &["hold"], Case { cfg: Some(allow_cfg), files: checks("e2e", "failure"), why: Some("未通過"), ..Case::default() })?;
    // ── 上游授權（判定日 2099-10-24）
    let up = strs(&["--repo", "acme-up/app"]);
    let up_files = || vec![("decision.json", decision(&good_rows, Dec { repo: "acme-up/app", ..Dec::default() }))];
    let up_cfg = |auth: Value| Some(am(json!({"upstream_authorized_by": auth})));
    let up_case = |auth: Value, why: Option<&'static str>| Case { cfg: up_cfg(auth), files: up_files(), args: up.clone(), why, ..Case::default() };
    s.run("上游 repo 沒有授權人 → escalate", &["escalate"], up_case(json!({}), Some("沒有授權人")))?;
    s.run("上游 repo 授權人空白（by 是空字串）→ escalate", &["escalate"], up_case(json!({"acme-up/app": {"by": " ", "expires": "2099-12-31"}}), Some("沒有授權人")))?;
    s.run("上游 repo 授權過期（expires 早於判定日）→ escalate", &["escalate"],
        up_case(json!({"acme-up/app": {"by": "示範維護者", "link": "https://example.com/acme/1", "expires": "2099-10-23"}}), Some("授權過期")))?;
    s.run("上游 repo 授權過期日讀不懂 → escalate", &["escalate"], up_case(json!({"acme-up/app": {"by": "示範維護者", "expires": "明年"}}), Some("讀不懂")))?;
    s.run("上游 repo 授權有效（expires 當天仍有效）→ merge", &["merge"], up_case(json!({"acme-up/app": {"by": "示範維護者", "expires": "2099-10-24"}}), None))?;
    s.run("上游 repo 授權人是字串 → merge", &["merge"], up_case(json!({"acme-up/app": "示範維護者"}), None))?;
    // ── 判定日＝max(--timestamp, 系統 UTC 日期)。其餘案例用 2099 年的 --timestamp（晚於系統日期）保持可重現；
    //    這裡故意傳舊日期，期待值只依賴「系統日期晚於 2000-01-01」，跨午夜也只會更晚
    let today = pyutil::today_utc();
    let day = |n: i64| pyutil::add_days(today, n).map(pyutil::iso).unwrap_or_default();
    let old_ts = strs(&["--timestamp", "2000-01-01T00:00:00+00:00"]);
    s.run("--timestamp 用舊日期不能讓已過期的上游授權復活（expires 昨天）→ escalate", &["escalate"], Case {
        cfg: up_cfg(json!({"acme-up/app": {"by": "示範維護者", "expires": day(-1)}})), files: up_files(),
        args: up.iter().chain(&old_ts).cloned().collect(), why: Some("授權過期"), ..Case::default() })?;
    // ── 其餘條件
    s.run("steward 判 escalate → escalate（gate 不放寬）", &["escalate"], Case { why: Some("steward 判 escalate"),
        ..files(vec![("decision.json", decision(&good_rows, Dec { dec: "escalate", ..Dec::default() }))]) })?;
    s.run("auto_merge 關閉 → escalate", &["escalate"], Case { cfg: Some(am(json!({"enabled": false}))), why: Some("auto_merge 未啟用"), ..Case::default() })?;
    let src_diff = mk_diff("src/export.py", &["def export():", "    return 'csv'"], &[]);
    s.run("風險超過上限（medium 改動、repo 上限 low）→ escalate", &["escalate"], Case {
        cfg: Some(am(json!({"repos": {"acme-fork/app": "low"}}))), why: Some("超過 acme-fork/app 的上限"), ..diff_case(src_diff.clone()) })?;
    s.run("風險 medium 在上限內 → merge", &["merge"], diff_case(src_diff.clone()))?;
    // ── recipe（--recipe）：diff 形狀超出 recipe → hold
    let bi = recipe::builtin_profile();
    // 重算 recipe 用的是專案 profile：補上內建 recipes
    s.write(".claude/teams/methodology.json", &bi)?;
    let resolve = |name: &str| recipe::resolve(&bi, &json!(name), &Value::Null, &Value::Null).map_err(|e| e.to_string());
    let run_json = |name: &str, locked: bool, resolved: Value| json!({"stages": {}, "recipe": {"name": name, "modifiers": [], "size": "", "locked": locked, "resolved": resolved}});
    let mut forged = resolve("docs")?;
    forged["build_kinds"] = json!(["code", "test", "doc"]);
    forged["stages"] = json!(["plan", "build", "deliver", "merge"]);
    for (name, body) in [("run-docs.json", run_json("docs", true, resolve("docs")?)), ("recipe-bugfix.json", run_json("bugfix", true, resolve("bugfix")?)),
        ("recipe-bad.json", json!({"name": "x"})), ("recipe-raw.json", resolve("feature")?),
        ("recipe-unlocked.json", run_json("bugfix", false, resolve("bugfix")?)), ("recipe-forged.json", run_json("docs", true, forged))] {
        s.write(name, &body)?;
    }
    let rc = |file: &str, why: Option<&'static str>, diff: Option<String>| Case {
        args: strs(&["--recipe", &f(file)]), why, files: diff.map(|d| vec![("diff.txt", json!(d))]).unwrap_or_default(), ..Case::default() };
    s.run("recipe 不足：hold（docs recipe 卻改了產品程式）", &["hold"], rc("run-docs.json", Some("recipe 不足"), Some(src_diff.clone())))?;
    s.run("recipe 不足：hold（bugfix 碰 migration）", &["hold"], rc("recipe-bugfix.json", Some("recipe 不足"), Some(mk_diff("db/migrations/001.sql", &["alter table t add c int;"], &[]))))?;
    s.run("recipe 相符：照常（bugfix recipe、改產品程式）→ merge", &["merge"], rc("recipe-bugfix.json", None, Some(src_diff.clone())))?;
    s.run("recipe 沒給：照現行（docs 形狀的檢查不跑）→ merge", &["merge"], diff_case(src_diff.clone()))?;
    s.run("recipe 讀不到 → hold（fail-closed）", &["hold"], rc("沒有.json", Some("輸入錯誤"), None))?;
    s.run("recipe 格式不對 → hold（fail-closed）", &["hold"], rc("recipe-bad.json", Some("不是 run.json"), None))?;
    s.run("recipe 只接受 run.json：單獨的 resolved JSON → hold", &["hold"], rc("recipe-raw.json", Some("不是 run.json"), Some(src_diff.clone())))?;
    s.run("recipe 還沒鎖定（plan 沒核准）→ hold", &["hold"], rc("recipe-unlocked.json", Some("還沒鎖定"), Some(src_diff.clone())))?;
    s.run("recipe.resolved 被改鬆（docs 卻允許 code）→ hold", &["hold"], rc("recipe-forged.json", Some("定義鬆"), Some(src_diff)))?;
    let big_lines: Vec<String> = (0..500).map(|i| format!("x{i} = {i}")).collect();
    let big_diff = mk_diff("src/big.py", &big_lines.iter().map(String::as_str).collect::<Vec<_>>(), &[]);
    s.run("diff 過大（超過 large_lines）→ escalate", &["escalate"], Case { why: Some("diff 過大"), ..diff_case(big_diff) })?;
    s.run("diff 為空 → hold", &["hold"], Case { why: Some("diff 是空的"), ..diff_case(String::new()) })?;
    s.write("empty_reqs.json", &json!("[]"))?;
    s.run("需求清單空 → hold", &["hold"], Case { cfg: Some(with(&base_cfg, "requirements", json!({"source": "json", "path": "empty_reqs.json"}))),
        files: vec![("decision.json", decision(&json!([]), Dec::default()))], why: Some("需求清單是空的"), ..Case::default() })?;
    s.run("多出需求 ID（對照表有需求清單沒有的 REQ-9）→ hold", &["hold"], Case { why: Some("需求清單裡沒有的 ID"),
        ..files(vec![("decision.json", decision(&json!([good_rows[0], good_rows[1], row("REQ-9", "清單裡沒有的需求", "match")]), Dec::default()))]) })?;
    // ── CI 帶入的凍結與當日合併數
    let arg_case = |args: &[&str], why: Option<&'static str>| Case { args: strs(args), why, ..Case::default() };
    s.run("--frozen（CI 由 repo variable 帶入）→ hold", &["hold"], arg_case(&["--frozen", "repo variable AGENT_AUTOMERGE_FROZEN=true"], Some("已凍結")))?;
    s.run("--merged-today 已達上限 → hold", &["hold"], arg_case(&["--merged-today", "2"], Some("每日合併上限")))?;
    s.run("--merged-today 未達上限 → merge", &["merge"], arg_case(&["--merged-today", "1"], None))?;
    s.run("--merged-today 讀不懂（API 失敗）→ hold", &["hold"], arg_case(&["--merged-today", "unknown"], Some("--merged-today")))?;
    // 每日上限：先清掉前面寫的紀錄，再放入今天已合併 2 次（上限 2）
    let audit = "_tpm/runs/merge-audit.jsonl";
    let audit_line = |date: &str, i: i64| format!("{}\n", recipe::dumps(&json!({"timestamp": format!("{date}T0{i}:00:00+00:00"), "repo": "acme-fork/app", "pr": i, "head_sha": SHA, "decision": "merge", "reasons": []}), None));
    s.write(audit, &json!(format!("{}{}", audit_line("2099-10-24", 1), audit_line("2099-10-24", 2))))?;
    s.run("超過每日上限 → hold", &["hold"], Case { why: Some("每日合併上限"), ..Case::default() })?;
    // 系統日期今天（與明天，防跨午夜）已合併 2 次：用舊的 --timestamp 也不能規避
    let both: String = [day(0), day(1)].iter().flat_map(|d| [audit_line(d, 1), audit_line(d, 2)]).collect();
    s.write(audit, &json!(both))?;
    s.run("--timestamp 用舊日期不能規避每日上限（系統日期今天已合併 2 次）→ hold", &["hold"], Case { args: old_ts, why: Some("每日合併上限"), ..Case::default() })?;
    s.write(audit, &json!(""))?;
    let frz = "_tpm/runs/auto-merge.freeze.json";
    s.write(frz, &json!({"frozen": true, "reason": "main CI 在合併後轉紅（示範）"}))?;
    s.run("凍結旗標 frozen=true（預設路徑）→ hold", &["hold"], Case::default())?;
    s.write(frz, &json!("not json"))?;
    s.run("凍結旗標讀不懂（fail-closed）→ hold", &["hold"], Case::default())?;
    s.write(frz, &json!({"frozen": false, "reason": "已由人解凍"}))?;
    s.run("凍結旗標 frozen=false（人已解凍）→ merge", &["merge"], Case::default())?;
    std::fs::remove_file(f(frz)).map_err(|e| e.to_string())?;
    s.write("freeze/flag", &json!("x"))?;
    s.run("凍結旗標存在（auto_merge.freeze_file 自訂路徑）→ hold", &["hold"], Case { cfg: Some(am(json!({"freeze_file": "freeze/flag"}))), ..Case::default() })?;
    std::fs::remove_file(f("freeze/flag")).map_err(|e| e.to_string())?;
    s.run("稽核紀錄：判定後 append 一行", &["merge"], Case { audit: true, ..Case::default() })?;
    let recs = read_audit(&f(audit))?;
    let need = ["decision", "head_sha", "pr", "reasons", "repo", "timestamp"];
    let good = recs.len() == 1 && recs.first().is_some_and(|r| need.iter().all(|k| r.get(k).is_some()) && r["head_sha"] == SHA && r["decision"] == "merge" && r["pr"] == 3);
    s.line(good, &format!("merge-audit.jsonl 欄位齊全（[{}]）", need.iter().map(|k| repr_str(k)).collect::<Vec<_>>().join(", ")));
    // schema 語意規則：宣稱 merge 但有 mismatch
    let bad = decision(&json!([good_rows[0], row("REQ-2", "x", "mismatch")]), Dec::default());
    let errs = Schemas::new(&ctx.teams).check(&bad, Some("merge-decision"))?;
    let shown: Vec<Value> = errs.iter().take(1).map(|e| json!(e)).collect();
    s.line(errs.iter().any(|e| e.contains("mismatch")), &format!("schemalib：traceability 宣稱 merge 但有 mismatch 要被擋（{}）", pyfmt::repr(&Value::Array(shown))));
    Ok(s.ok)
}
