//! profile 繼承（`methodology.json` 的 `extends`）的端到端測試：實際執行編好的 `awf`，在暫存目錄 init 一個專案，
//! 把專案的 profile 改成只寫 `extends` 加一兩個覆寫，再看 doctor、build、產生腳本（awf next 的路）、save-run、hook 讀到的
//! 是不是合併後的值；`extends` 指到不存在的檔時 build 與 next 要報錯、不是默默略過。不呼叫模型。
//! 「沒有 extends 時逐字不變」由既有的回歸基準（`tests/parity/`）與 `golden-set/token-budget.json` 負責。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
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

fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("讀不到 {}：{e}", p.display()))
}

fn load(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{} 不是 JSON：{e}", p.display()))
}

impl Proj {
    fn new(tag: &str, extra_init: &[&str]) -> Proj {
        static N: AtomicUsize = AtomicUsize::new(0);
        let tmp = std::env::temp_dir().join(format!("awf-extends-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let p = Proj { dir: tmp.join("proj"), tmp };
        let cfg = root().join("plugin/workflow.config.example");
        let mut argv = vec!["init", "--target", p.dir.to_str().unwrap(), "--config", cfg.to_str().unwrap()];
        argv.extend_from_slice(extra_init);
        let r = p.awf(&argv, &[]);
        assert_eq!(r.code, 0, "awf init：\n{}", r.all());
        p
    }

    fn teams(&self, rel: &str) -> PathBuf {
        self.dir.join(".claude/teams").join(rel)
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(EXE);
        c.args(args).current_dir(&self.dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            if k.starts_with("CLAUDE_") || k.starts_with("AWF_") {
                c.env_remove(k);
            }
        }
        c.env("CLAUDE_PROJECT_DIR", &self.dir).env("AWF_PLUGIN_DIR", root().join("plugin")).env("AWF_TRANSCRIPTS_DIR", self.tmp.join("transcripts"));
        c
    }

    fn awf(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut c = self.cmd(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().expect("啟動 awf");
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
    }

    fn hook(&self, mode: &str, input: &Value) -> Out {
        let mut c = self.cmd(&["hook", mode]);
        c.stdin(Stdio::piped());
        let mut child = c.spawn().expect("啟動 awf hook");
        if let Some(mut s) = child.stdin.take() {
            let _ = s.write_all(input.to_string().as_bytes());
        }
        let o = child.wait_with_output().unwrap();
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() }
    }
}

/// 腳本裡內嵌的 profile（`/*@methodology*/ … /*@end*/` 之間）。
fn embedded(js: &str) -> Value {
    let a = js.find("/*@methodology*/").expect("有 methodology 標記") + "/*@methodology*/".len();
    let b = a + js[a..].find("/*@end*/").expect("有結尾標記");
    serde_json::from_str(js[a..b].trim()).expect("內嵌的 profile 是 JSON")
}

fn builtin_default() -> Value {
    load(&root().join("plugin/scaffold/.claude/teams/methodologies/default.json"))
}

/// 預期的合併結果：內建 default 疊上覆寫（name 取自己的）。
fn expected() -> Value {
    let mut want = builtin_default();
    want["name"] = json!("acme-portal");
    want["conventions"]["pr_size"]["max_lines"] = json!(450);
    want["gates"] = json!({"extra": ["build"]});
    want["knowledge"]["repos"] = json!([{"id": "acme-specs", "path": "../kg-ref", "sources": ["docs/**/*.md"]}]);
    want.as_object_mut().unwrap().shift_remove("deliverables");
    want
}

/// 專案的 profile 只寫 extends 加幾個覆寫：一層繼承內建 default、一層繼承專案自己的共用檔。
fn write_inheriting(p: &Proj) {
    put(&p.teams("acme-base.json"), &json!({
        "extends": "default",
        "name": "不會被繼承的名字",
        "gates": {"extra": ["build"]},
        "knowledge": {"repos": [{"id": "acme-specs", "path": "../kg-ref", "sources": ["docs/**/*.md"]}]},
    }).to_string());
    put(&p.teams("methodology.json"), &json!({
        "extends": [".claude/teams/acme-base.json"],
        "name": "acme-portal",
        "conventions": {"pr_size": {"max_lines": 450}},
        "deliverables": null,
    }).to_string());
}

#[test]
fn inherited_profile_is_what_every_reader_sees() {
    let p = Proj::new("ok", &["--with-workflows"]);
    write_inheriting(&p);
    let want = expected();

    // 1. doctor：沒有錯誤，列出繼承鏈
    let d = p.awf(&["doctor", "--json"], &[]);
    let items: Value = serde_json::from_str(&d.stdout).unwrap_or_else(|e| panic!("doctor --json 不是 JSON（{e}）：\n{}", d.all()));
    let list = items["checks"].as_array().cloned().unwrap_or_default();
    let errs: Vec<&Value> = list.iter().filter(|i| i["level"] == "error" && (i["id"] == "methodology" || i["id"] == "profile-extends")).collect();
    assert!(errs.is_empty(), "doctor 不該有 profile 的錯誤：{errs:?}\n{}", d.all());
    let chain = list.iter().find(|i| i["id"] == "profile-extends").unwrap_or_else(|| panic!("doctor 要列繼承鏈：\n{}", d.all()));
    assert!(chain["msg"].as_str().unwrap_or_default().contains("內建 default → .claude/teams/acme-base.json → .claude/teams/methodology.json"), "{chain}");
    assert!(list.iter().any(|i| i["id"] == "methodology" && i["level"] == "ok"), "合併後要通過 schema：\n{}", d.all());

    // 2. awf build：覆寫檔內嵌的是合併結果
    let b = p.awf(&["build"], &[]);
    assert_eq!(b.code, 0, "awf build：\n{}", b.all());
    let js = read(&p.dir.join(".claude/workflows/sdlc-plan.js"));
    assert_eq!(embedded(&js), want, "awf build 內嵌的 profile 要等於合併結果");
    assert!(!js.contains("\"extends\""), "內嵌的 profile 不該留著 extends");

    // 3. 產生腳本（awf next 用的同一條路）：一樣是合併結果
    let out = p.tmp.join("wf");
    let r = p.awf(&["render", "--all", "--out", out.to_str().unwrap(), "--plugin-dir", root().join("plugin").to_str().unwrap()], &[]);
    assert_eq!(r.code, 0, "awf render：\n{}", r.all());
    assert_eq!(embedded(&read(&out.join("sdlc-design.js"))), want);

    // 4. save-run：gates.extra 來自被繼承的檔 → build 段有閘門
    let run = p.dir.join("_tpm/runs/r1");
    let r = p.awf(&["save-run", "init", run.to_str().unwrap(), "--event", "sdlc"], &[]);
    assert_eq!(r.code, 0, "save-run init：\n{}", r.all());
    let rj = load(&run.join("run.json"));
    assert_eq!(rj["stages"]["build"]["gate"], json!(true), "繼承來的 gates.extra 要生效：{}", rj["stages"]);
    assert_eq!(rj["stages"]["sync"]["gate"], json!(false));

    // 5. hook：繼承來的 knowledge.repos 對 team agent 唯讀
    let write = |file: &Path| json!({"hook_event_name": "PreToolUse", "tool_name": "Write", "cwd": p.dir, "tool_input": {"file_path": file}, "agent_type": "agent-workflow:dev-implementer"});
    let h = p.hook("guard-paths", &write(&p.tmp.join("kg-ref/docs/api.md")));
    assert_eq!(h.code, 2, "繼承來的 knowledge.repos 要擋寫入：\n{}", h.all());
    assert!(h.stderr.contains("knowledge.repos"), "{}", h.all());
    // 對照：不在清單裡的工作區外路徑照舊是一般的越權寫入（訊息不提 knowledge.repos）
    let h = p.hook("guard-paths", &write(&p.tmp.join("kg-other/a.md")));
    assert!(h.code == 2 && !h.stderr.contains("knowledge.repos"), "不在清單的路徑照舊：\n{}", h.all());

    // 6. 合併後不合 schema：錯誤標出來源檔
    put(&p.teams("acme-base.json"), &json!({"extends": "default", "conventions": {"pr_size": {"max_lines": "很多"}}}).to_string());
    put(&p.teams("methodology.json"), &json!({"extends": [".claude/teams/acme-base.json"], "name": "acme-portal"}).to_string());
    let b = p.awf(&["build"], &[]);
    assert_ne!(b.code, 0, "合併後不合 schema 要拒絕內嵌：\n{}", b.all());
    assert!(b.all().contains("來源：.claude/teams/acme-base.json"), "錯誤要指出來源檔：\n{}", b.all());
}

#[test]
fn missing_source_stops_build_and_next() {
    let p = Proj::new("missing", &["--with-workflows"]);
    put(&p.teams("methodology.json"), &json!({"extends": ["default", ".claude/teams/acme-none.json"], "name": "acme-portal"}).to_string());

    let b = p.awf(&["build"], &[]);
    assert_ne!(b.code, 0, "awf build 要報錯：\n{}", b.all());
    assert!(b.all().contains("acme-none.json") && b.all().contains("不存在"), "{}", b.all());

    let r = p.awf(&["start", "--run", "_tpm/runs/r1", "--event", "sdlc", "--repo", ".", "--epic", "refund", "--requirement", "示範客戶 ACME 的會員要能退款"], &[]);
    assert_eq!(r.code, 0, "awf start：\n{}", r.all());
    let n = p.awf(&["next", "--run", "_tpm/runs/r1"], &[]);
    let said = n.all();
    let refused = n.code != 0 || serde_json::from_str::<Value>(&n.stdout).is_ok_and(|v| v["action"] == "halted");
    assert!(refused && said.contains("acme-none.json"), "awf next 要報錯、不是默默略過：\n{said}");
    assert!(!said.contains("\"action\": \"run_workflow\"") && !said.contains("\"action\":\"run_workflow\""), "不能照常產生腳本：\n{said}");

    // doctor 列出問題與修法
    let d = p.awf(&["doctor"], &[]);
    assert_ne!(d.code, 0, "doctor 要判失敗：\n{}", d.all());
    assert!(d.all().contains("extends") && d.all().contains("acme-none.json"), "{}", d.all());

    // save-run 的政策核准用到 profile 時 fail-closed（讀不懂）
    let r = p.awf(&["recipe", "resolve", "--recipe", "feature"], &[]);
    assert_ne!(r.code, 0, "awf recipe resolve 要報錯：\n{}", r.all());
}

#[test]
fn init_keeps_extends_and_does_not_flatten() {
    let p = Proj::new("init", &[]);
    write_inheriting(&p);
    let before = read(&p.teams("methodology.json"));
    let cfg = root().join("plugin/workflow.config.example");
    // --upgrade --adopt-new-fields：比對合併後的值；有缺也不寫回（不攤平）
    let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--config", cfg.to_str().unwrap(), "--upgrade", "--adopt-new-fields"], &[]);
    assert_eq!(read(&p.teams("methodology.json")), before, "--upgrade --adopt-new-fields 不能改寫有 extends 的 profile：\n{}", r.all());
    assert!(!r.all().contains("讀不懂"), "{}", r.all());
    // --reconfigure 改一個答案：只寫那個葉子，extends 保留、不攤平
    let ans = p.tmp.join("ans.json");
    put(&ans, &json!({"pr_size": {"max_lines": 500, "max_files": 15}}).to_string());
    let r = p.awf(&["init", "--target", p.dir.to_str().unwrap(), "--reconfigure", "--answers", ans.to_str().unwrap()], &[]);
    let after = load(&p.teams("methodology.json"));
    assert_eq!(after["extends"], json!([".claude/teams/acme-base.json"]), "extends 要保留：\n{}", r.all());
    assert!(after.get("stages").is_none() && after.get("recipes").is_none(), "不能把繼承來的值攤平寫回：{after}\n{}", r.all());
    assert_eq!(after["name"], json!("acme-portal"));
    assert_eq!(after["conventions"]["pr_size"]["max_lines"], json!(500), "這次的答案要寫進去：\n{}", r.all());
}
