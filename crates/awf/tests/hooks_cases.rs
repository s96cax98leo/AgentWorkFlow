//! hook 的案例重放：照 `tests/hooks/cases.jsonl` 搭環境，對 `awf hook <模式>` 逐案例執行，
//! exit code、stdout、stderr 逐字比對。不需要 Python。資料檔的格式見 `tests/hooks/README.md`。
//!
//! - 每一組（`{"group": …}`）有自己的暫存目錄，從空的開始、照順序執行；組和組之間互不影響，所以平行跑。
//!   有 `perf` 步驟的組最後單獨跑（量時間時不和別組搶 CPU）。
//! - 子行程的環境是固定的：只有 PATH（最前面是假的 python3）、HOME（空目錄）、TMPDIR、固定的 git 日期與空的 git 設定，
//!   外面的 `GIT_*`、`CLAUDE_*`、`AWF_*` 都不會帶進去。git 的 remote 只是設定裡的網址，不會連出去。
//! - `AWF_HOOKS_RECORD=<檔>`：把這次實際的結果照同樣的格式寫出來（全部通過時和資料檔逐字相同），
//!   新增案例時用它取得 stderr 的原文。
#![cfg(unix)]

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};

const EXE: &str = env!("CARGO_BIN_EXE_awf");
/// 從 Python 版測試台轉過來的案例數；之後只會加，不會少。
const MIN_CASES: usize = 1335;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ───────── 資料檔 ─────────

struct Group {
    name: String,
    /// 這一組的 hook 呼叫彼此獨立（沒有哪一次呼叫會改變檔案狀態）：可以拆成幾份、各自重建環境後平行跑。
    independent: bool,
    steps: Vec<(usize, Value)>,
}

impl Group {
    fn hooks(&self) -> usize {
        self.steps.iter().filter(|(_, st)| st["op"] == "hook").count()
    }
}

struct Data {
    groups: Vec<Group>,
    lines: Vec<String>,
    blobs: Map<String, Value>,
    python_stub: String,
}

fn load() -> Data {
    let dir = repo_root().join("tests/hooks");
    let text = std::fs::read_to_string(dir.join("cases.jsonl")).expect("讀 tests/hooks/cases.jsonl");
    let fx: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("fixtures.json")).expect("讀 tests/hooks/fixtures.json")).expect("fixtures.json 是 JSON");
    let mut groups: Vec<Group> = Vec::new();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("cases.jsonl 第 {} 行不是 JSON：{e}", i + 1));
        if let Some(g) = v.get("group").and_then(Value::as_str) {
            groups.push(Group { name: g.to_string(), independent: v["independent"].as_bool().unwrap_or(false), steps: Vec::new() });
        } else if v.get("#").is_some() {
            continue;
        } else {
            groups.last_mut().unwrap_or_else(|| panic!("cases.jsonl 第 {} 行：步驟前面要先有一行 group", i + 1)).steps.push((i + 1, v));
        }
    }
    for g in &groups {
        let stateful = g.steps.iter().any(|(_, st)| st["op"] == "check" || st["op"] == "perf");
        let countable = g.steps.iter().any(|(_, st)| st["op"] == "na");
        assert!(!(g.independent && (stateful || countable)), "cases.jsonl 的組「{}」標了 independent，卻有 check／perf／na 步驟（前兩種要看前面累積的狀態）", g.name);
    }
    Data {
        groups,
        lines,
        blobs: fx["blobs"].as_object().cloned().unwrap_or_default(),
        python_stub: fx["python_stub"].as_str().expect("fixtures.json 的 python_stub").to_string(),
    }
}

/// `@REPEAT:<次數>:<單位字數>:<單位>@` 展開，再把 `@TMP@` 換成這一組的暫存目錄。
fn expand(s: &str, root: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("@REPEAT:") {
        out.push_str(&rest[..i]);
        let body = &rest[i + 8..];
        let parsed = (|| {
            let (n, r) = body.split_once(':')?;
            let (len, r) = r.split_once(':')?;
            let (n, len): (usize, usize) = (n.parse().ok()?, len.parse().ok()?);
            let end = r.char_indices().nth(len).map(|(b, _)| b)?;
            r[end..].starts_with('@').then(|| (n, &r[..end], &r[end + 1..]))
        })();
        match parsed {
            Some((n, unit, tail)) => {
                out.push_str(&unit.repeat(n));
                rest = tail;
            }
            None => {
                out.push_str("@REPEAT:");
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out.replace("@TMP@", root)
}

fn expand_deep(v: &Value, root: &str) -> Value {
    match v {
        Value::String(s) => Value::String(expand(s, root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| expand_deep(x, root)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand_deep(x, root))).collect()),
        other => other.clone(),
    }
}

/// 和 Python 的 `json.dumps(x, ensure_ascii=False)` 逐字相同（hook 的輸入當初就是這樣產生的）。
fn py_dumps(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => py_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_dumps(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_str(k, out);
                out.push_str(": ");
                py_dumps(x, out);
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dumps(v: &Value) -> String {
    let mut s = String::new();
    py_dumps(v, &mut s);
    s
}

// ───────── 執行環境 ─────────

struct Base {
    dir: PathBuf,
    path_env: String,
    home: PathBuf,
}

impl Base {
    fn new(stub: &str) -> Base {
        let dir = std::env::temp_dir().join(format!("awf-hookcases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (bin, home) = (dir.join("bin"), dir.join("home"));
        std::fs::create_dir_all(&bin).expect("建立暫存目錄");
        std::fs::create_dir_all(&home).expect("建立暫存目錄");
        let py = bin.join("python3");
        std::fs::write(&py, stub).expect("寫假的 python3");
        std::fs::set_permissions(&py, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path_env = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        Base { dir, path_env, home }
    }

    /// 固定的環境：外面的變數一律不帶進去。
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("PATH", &self.path_env)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00")
            // 暫存目錄明講：scratchpad 的規則看的是「系統暫存目錄底下」，要和這裡搭環境用的是同一個
            // （macOS 沒設 TMPDIR 時，Rust 取的是使用者的暫存目錄，hook 取的是 /tmp）
            .env("TMPDIR", std::env::temp_dir());
        c
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Default)]
struct Outcome {
    fails: Vec<(usize, String)>,
    cases: usize,
    added: usize,
    na: usize,
    calls: usize,
    record: Vec<(usize, String)>,
    notes: Vec<String>,
}

struct Runner<'a> {
    base: &'a Base,
    data: &'a Data,
    group: &'a str,
    /// 只執行第幾個到第幾個 hook 呼叫（拆開跑的時候）；其餘步驟（搭環境）照常做
    only: Option<(usize, usize)>,
    seen_hooks: usize,
    root: String,
    norm: Vec<String>,
    out: Outcome,
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn walk(dir: &Path, recursive: bool, skip: Option<&Path>, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if skip.is_some_and(|x| p == x) {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(p.clone());
        if recursive && is_dir {
            walk(&p, true, skip, out);
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), &dst)?;
        }
    }
    Ok(())
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).expect("hex")).collect()
}

/// 指令是不是「awf… hook <模式>」（執行檔可以帶路徑與引號）。
fn runs_hook(cmd: &str, mode: &str) -> bool {
    let needle = format!(" hook {mode}");
    cmd.match_indices(&needle).any(|(i, _)| {
        let after = cmd[i + needle.len()..].chars().next();
        let before = cmd[..i].rsplit(' ').next().unwrap_or("").trim_end_matches('"');
        !after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') && before.contains("awf")
    })
}

impl<'a> Runner<'a> {
    fn new(base: &'a Base, data: &'a Data, unit: &Unit) -> Runner<'a> {
        let dir = base.dir.join(format!("g{}-{}", unit.group, unit.only.map(|r| r.0).unwrap_or(0)));
        std::fs::create_dir_all(&dir).expect("建立這一組的暫存目錄");
        // 輸入用沒解開符號連結的路徑（macOS 的暫存目錄是符號連結），和原本的測試台一樣；輸出兩種寫法都換回 @TMP@
        let root = dir.to_string_lossy().into_owned();
        let real = dir.canonicalize().expect("解開暫存目錄").to_string_lossy().into_owned();
        let mut norm = vec![root.clone(), real];
        norm.sort_by_key(|x| std::cmp::Reverse(x.len()));
        norm.dedup();
        Runner { base, data, group: &data.groups[unit.group].name, only: unit.only, seen_hooks: 0, root, norm, out: Outcome::default() }
    }

    fn x(&self, v: &Value) -> String {
        expand(s(v), &self.root)
    }

    fn normalize(&self, bytes: &[u8]) -> String {
        let mut t = String::from_utf8_lossy(bytes).into_owned();
        for n in &self.norm {
            t = t.replace(n.as_str(), "@TMP@");
        }
        t
    }

    fn fail(&mut self, line: usize, name: &str, why: String) {
        self.out.fails.push((line, format!("第 {line} 行（組 {}）［{name}］{why}", self.group)));
    }

    fn step(&mut self, line: usize, st: &Value) {
        let op = s(&st["op"]).to_string();
        let name = s(&st["name"]).to_string();
        let res: Result<(), String> = match op.as_str() {
            "mkdir" => std::fs::create_dir_all(self.x(&st["path"])).map_err(|e| e.to_string()),
            "write" => self.write(st),
            "copy" => copy_tree(&repo_root().join(s(&st["from"])), Path::new(&self.x(&st["to"]))).map_err(|e| e.to_string()),
            "rm" => {
                let p = self.x(&st["path"]);
                let r = if Path::new(&p).is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                r.map_err(|e| e.to_string())
            }
            "mtime" => {
                let when = SystemTime::now() - Duration::from_secs(st["days_ago"].as_u64().unwrap_or(0) * 86_400);
                std::fs::File::open(self.x(&st["path"])).and_then(|f| f.set_modified(when)).map_err(|e| e.to_string())
            }
            "git" => self.git(st),
            "hook" => {
                let k = self.seen_hooks;
                self.seen_hooks += 1;
                if self.only.is_none_or(|(lo, hi)| lo <= k && k < hi) {
                    self.hook(line, st);
                }
                Ok(())
            }
            "check" => {
                if st.get("added").is_some() {
                    self.out.added += 1;
                } else {
                    self.out.cases += 1;
                }
                self.check(st)
            }
            "perf" => self.perf(line, st),
            "na" => {
                self.out.cases += 1;
                self.out.na += 1;
                Ok(())
            }
            other => Err(format!("不認得的 op：{other}")),
        };
        if let Err(why) = res {
            self.fail(line, if name.is_empty() { &op } else { &name }, why);
        }
    }

    fn write(&self, st: &Value) -> Result<(), String> {
        let p = self.x(&st["path"]);
        let bytes = if let Some(h) = st.get("hex") {
            unhex(s(h))
        } else if let Some(b) = st.get("blob") {
            let text = self.data.blobs.get(s(b)).ok_or_else(|| format!("fixtures.json 沒有 blob「{}」", s(b)))?;
            expand(s(text), &self.root).into_bytes()
        } else {
            self.x(&st["text"]).into_bytes()
        };
        if let Some(parent) = Path::new(&p).parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{p}：{e}"))
    }

    fn git(&self, st: &Value) -> Result<(), String> {
        let args: Vec<String> = st["args"].as_array().map(|a| a.iter().map(|x| self.x(x)).collect()).unwrap_or_default();
        let out = self.base.command("git").args(&args).current_dir(self.x(&st["cwd"])).output().map_err(|e| format!("git 跑不起來：{e}"))?;
        if !out.status.success() {
            return Err(format!("git {} 失敗：{}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }

    fn hook(&mut self, line: usize, st: &Value) {
        let name = s(&st["name"]).to_string();
        let label = if name.is_empty() { format!("（沒有名稱的呼叫：{}）", s(&st["mode"])) } else { name.clone() };
        let stdin = match st.get("stdin") {
            Some(v) => dumps(&expand_deep(v, &self.root)),
            None => self.x(&st["stdin_raw"]),
        };
        let mut c = self.base.command(EXE);
        c.arg("hook").arg(s(&st["mode"])).current_dir(&self.root)
            .env("CLAUDE_PROJECT_DIR", self.x(&st["proj"]))
            .env("AWF_READ_BUDGET_DIR", format!("{}/rb-state", self.root));
        if let Some(env) = st.get("env").and_then(Value::as_object) {
            for (k, v) in env {
                c.env(k, self.x(v));
            }
        }
        let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("啟動 awf");
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(stdin.as_bytes()); // 用法錯誤時對方不讀 stdin
        }
        let out = child.wait_with_output().expect("等 awf 結束");
        self.out.calls += 1;
        let rc = out.status.code().unwrap_or(-1);
        let (so, se) = (self.normalize(&out.stdout), self.normalize(&out.stderr));

        let mut why = Vec::new();
        if st["rc"].as_i64() != Some(rc as i64) {
            why.push(format!("exit code 預期 {}、實際 {rc}", st["rc"]));
        }
        for (key, got) in [("stdout", &so), ("stderr", &se)] {
            let want = s(&st[key]);
            if want != got {
                why.push(format!("{key} 不同\n    預期：{want:?}\n    實際：{got:?}"));
            }
        }
        if let Some(not) = st.get("rc_not").and_then(Value::as_array) {
            if not.iter().any(|x| x.as_i64() == Some(rc as i64)) {
                why.push(format!("exit code 不能是 {rc}"));
            }
        }
        if !why.is_empty() {
            self.fail(line, &label, why.join("\n  "));
        }
        if !name.is_empty() {
            self.out.cases += 1;
        }
        for also in st.get("also").and_then(Value::as_array).into_iter().flatten() {
            self.out.cases += 1;
            let alts: Vec<&str> = also["stderr_has"].as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default();
            if !alts.iter().any(|a| se.contains(a)) {
                self.fail(line, s(&also["name"]), format!("stderr 沒有 {alts:?} 其中之一：{se:?}"));
            }
        }

        // 記錄：把實際結果填回這一行（rc 後面依序是 stdout、stderr，空的不寫）
        let mut rec = Map::new();
        for (k, v) in st.as_object().into_iter().flatten() {
            if k == "stdout" || k == "stderr" {
                continue;
            }
            if k == "rc" {
                rec.insert("rc".into(), Value::from(rc));
                for (key, got) in [("stdout", &so), ("stderr", &se)] {
                    if !got.is_empty() {
                        rec.insert(key.into(), Value::String(got.clone()));
                    }
                }
            } else {
                rec.insert(k.clone(), v.clone());
            }
        }
        self.out.record.push((line, dumps(&Value::Object(rec))));
    }

    fn read_json(&self, p: &str) -> Result<Value, String> {
        serde_json::from_str(&std::fs::read_to_string(p).map_err(|e| format!("{p}：{e}"))?).map_err(|e| format!("{p}：{e}"))
    }

    /// 一個檢查項（不是 hook 呼叫）：檔案在不在、JSON 的欄位、目錄裡有沒有某種檔名、註冊設定。
    fn check(&self, st: &Value) -> Result<(), String> {
        if let Some(p) = st.get("exists") {
            let p = self.x(p);
            return Path::new(&p).exists().then_some(()).ok_or(format!("{p} 應該存在"));
        }
        if let Some(p) = st.get("absent") {
            let p = self.x(p);
            return (!Path::new(&p).exists()).then_some(()).ok_or(format!("{p} 不應該存在"));
        }
        if let Some(p) = st.get("json") {
            let doc = self.read_json(&self.x(p))?;
            for (k, want) in st.get("expect").and_then(Value::as_object).into_iter().flatten() {
                if doc.get(k) != Some(want) {
                    return Err(format!("{k} 預期 {want}、實際 {}", doc.get(k).unwrap_or(&Value::Null)));
                }
            }
            for (k, r) in st.get("range").and_then(Value::as_object).into_iter().flatten() {
                let got = doc.get(k).and_then(Value::as_i64);
                if !got.is_some_and(|g| r[0].as_i64().is_some_and(|lo| lo <= g) && r[1].as_i64().is_some_and(|hi| g <= hi)) {
                    return Err(format!("{k} 預期在 {r} 之間、實際 {got:?}"));
                }
            }
            return Ok(());
        }
        if let Some(f) = st.get("files") {
            let skip = f.get("except").map(|p| PathBuf::from(self.x(p)));
            let mut all = Vec::new();
            walk(Path::new(&self.x(&f["dir"])), f["recursive"].as_bool().unwrap_or(false), skip.as_deref(), &mut all);
            let hit: Vec<String> = all.iter().filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                n.starts_with(s(&f["prefix"])) && n.ends_with(s(&f["suffix"])) && n.contains(s(&f["contains"]))
            }).map(|p| p.to_string_lossy().into_owned()).collect();
            return match s(&st["count"]) {
                "none" if !hit.is_empty() => Err(format!("不應該有這些檔：{hit:?}")),
                "some" if hit.is_empty() => Err("應該至少有一個符合的檔".to_string()),
                "none" | "some" => Ok(()),
                other => Err(format!("count 只能是 none 或 some：{other}")),
            };
        }
        if let Some(p) = st.get("jsonl") {
            // 檔案不在或有一行讀不懂：當成沒有任何紀錄
            let text = std::fs::read_to_string(self.x(p)).unwrap_or_default();
            let rows: Vec<Value> = text.lines().map(serde_json::from_str).collect::<Result<_, _>>().unwrap_or_default();
            let picked: Vec<&Value> = rows.iter().filter(|r| {
                st["where"].as_object().into_iter().flatten().all(|(k, vals)| vals.as_array().is_some_and(|a| r.get(k).is_some_and(|v| a.contains(v))))
            }).collect();
            if s(&st["count"]) == "none" {
                return picked.is_empty().then_some(()).ok_or(format!("不應該有紀錄：{picked:?}"));
            }
            let got: BTreeSet<String> = picked.iter().map(|r| s(&r[s(&st["field"])]).to_string()).collect();
            let want: BTreeSet<String> = st["set"].as_array().into_iter().flatten().map(|v| s(v).to_string()).collect();
            return (got == want).then_some(()).ok_or(format!("{} 預期 {want:?}、實際 {got:?}", s(&st["field"])));
        }
        if st.get("hooks_file").is_some() {
            return self.registration(st);
        }
        Err("check 沒有可以檢查的欄位".to_string())
    }

    /// 註冊設定（plugin 的 hooks.json、專案 settings 範本）的形狀。
    fn registration(&self, st: &Value) -> Result<(), String> {
        let file = s(&st["hooks_file"]);
        let mut text = std::fs::read_to_string(repo_root().join(file)).map_err(|e| format!("{file}：{e}"))?;
        for (k, v) in st.get("subst").and_then(Value::as_object).into_iter().flatten() {
            text = text.replace(k.as_str(), s(v));
        }
        let cfg: Value = serde_json::from_str(&text).map_err(|e| format!("{file}：{e}"))?;
        let empty = Vec::new();
        let pre = cfg["hooks"]["PreToolUse"].as_array().unwrap_or(&empty);
        let cmd_of = |e: &Value| s(&e["hooks"][0]["command"]).to_string();
        let entry = |mode: &str| pre.iter().find(|e| runs_hook(&cmd_of(e), mode));
        let on_failure = |e: Option<&Value>, vals: &Value| -> Result<(), String> {
            let got = e.map(|e| e["hooks"][0]["onFailure"].clone()).unwrap_or(Value::Null);
            vals.as_array().is_some_and(|a| a.contains(&got)).then_some(()).ok_or(format!("onFailure 預期 {vals}、實際 {got}"))
        };
        let mode = s(&st["mode"]);
        match s(&st["rule"]) {
            "no_bash" => (!text.contains("bash ") && !text.contains(".sh")).then_some(()).ok_or("設定裡出現 bash 或 .sh".to_string()),
            "has_matcher" => pre.iter().any(|e| e["matcher"] == st["matcher"]).then_some(()).ok_or(format!("沒有 matcher {}", st["matcher"])),
            "entry" => entry(mode).map(|_| ()).ok_or(format!("PreToolUse 沒有跑 awf hook {mode} 的項目")),
            "covers" => {
                let m: Vec<&str> = entry(mode).map(|e| s(&e["matcher"]).split('|').collect()).unwrap_or_default();
                let missing: Vec<&str> = st["tools"].as_array().into_iter().flatten().map(s).filter(|t| !m.contains(t)).collect();
                missing.is_empty().then_some(()).ok_or(format!("{mode} 的 matcher 少了 {missing:?}"))
            }
            "on_failure" => on_failure(entry(mode), &st["values"]),
            "matcher_on_failure" => on_failure(pre.iter().find(|e| e["matcher"] == st["matcher"]), &st["values"]),
            "event_runs" => {
                let cmd = s(&cfg["hooks"][s(&st["event"])][0]["hooks"][0]["command"]).to_string();
                runs_hook(&cmd, mode).then_some(()).ok_or(format!("{} 跑的是「{cmd}」，不是 awf hook {mode}", s(&st["event"])))
            }
            other => Err(format!("不認得的 rule：{other}")),
        }
    }

    /// 讀取預算的耗時：在測試行程裡直接呼叫（和 Python 版一樣量「讀設定＋檢查＋讀寫狀態檔」，不含行程啟動）。
    fn perf(&mut self, line: usize, st: &Value) -> Result<(), String> {
        std::env::set_var("AWF_READ_BUDGET_DIR", format!("{}/rb-state", self.root));
        std::env::set_var("CLAUDE_PROJECT_DIR", self.x(&st["proj"]));
        let data = expand_deep(&st["stdin"], &self.root);
        let times = st["times"].as_u64().unwrap_or(30) as usize;
        let mut took = Vec::with_capacity(times);
        let mut blocked = Vec::new();
        for _ in 0..times {
            let t0 = Instant::now();
            let ws = awf_core::config::workspace().map_err(|e| e.msg)?;
            let cfg = awf_core::config::load(&ws).map_err(|e| e.msg)?;
            let msg = match s(&st["fn"]) {
                "check_read" => awf_core::hooks::read_budget::check_read(&data, &cfg, &ws),
                "check_shell" => awf_core::hooks::read_budget::check_shell(&data, &cfg, &ws),
                other => return Err(format!("不認得的 fn：{other}")),
            };
            took.push(t0.elapsed().as_secs_f64() * 1000.0);
            blocked.extend(msg);
        }
        std::env::remove_var("AWF_READ_BUDGET_DIR");
        std::env::remove_var("CLAUDE_PROJECT_DIR");
        if let Some(c) = st.get("counted") {
            self.out.cases += 1;
            let r = if blocked.is_empty() { self.check(c) } else { Err(format!("有 {} 次被擋：{}", blocked.len(), blocked[0])) };
            if let Err(why) = r {
                self.fail(line, s(&c["name"]), why);
            }
        }
        self.out.cases += 1;
        took.sort_by(f64::total_cmp);
        let (stat, ms) = match s(&st["stat"]) {
            "median" => ("中位數", took[took.len() / 2]),
            _ => ("平均", took.iter().sum::<f64>() / took.len() as f64),
        };
        let limit = st["limit_ms"].as_f64().unwrap_or(50.0);
        self.out.notes.push(s(&st["name"]).replace("@MS@", &format!("{ms:.2}ms")));
        (ms <= limit).then_some(()).ok_or(format!("{stat} {ms:.2}ms 超過 {limit:.0}ms"))
    }
}

/// 一個工作單位：一整組，或 independent 的組裡的一段 hook 呼叫。
struct Unit {
    group: usize,
    only: Option<(usize, usize)>,
    hooks: usize,
}

/// independent 的組每份最多這麼多次呼叫。
const SHARD: usize = 48;

fn units(data: &Data) -> Vec<Unit> {
    let mut out = Vec::new();
    for (i, g) in data.groups.iter().enumerate() {
        let n = g.hooks();
        if g.independent && n > SHARD {
            let mut lo = 0;
            while lo < n {
                let hi = (lo + SHARD).min(n);
                out.push(Unit { group: i, only: Some((lo, hi)), hooks: hi - lo });
                lo = hi;
            }
        } else {
            out.push(Unit { group: i, only: None, hooks: n });
        }
    }
    out
}

fn run_unit(base: &Base, data: &Data, unit: &Unit) -> Outcome {
    let mut r = Runner::new(base, data, unit);
    for (line, st) in &data.groups[unit.group].steps {
        r.step(*line, st);
    }
    let _ = std::fs::remove_dir_all(&r.root);
    r.out
}

/// 這個檔的測試一次只跑一個：perf 步驟會設整個行程的環境變數。
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn replay_all_cases() {
    let _serial = serial();
    let started = Instant::now();
    let data = load();
    let base = Base::new(&data.python_stub);
    let has_perf = |g: &Group| g.steps.iter().any(|(_, st)| st["op"] == "perf");
    // 大的先跑，整體時間取決於最大的那一份
    let (serial_units, mut parallel): (Vec<Unit>, Vec<Unit>) = units(&data).into_iter().partition(|u| has_perf(&data.groups[u.group]));
    parallel.sort_by_key(|u| std::cmp::Reverse(u.hooks));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(parallel.len().max(1));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(unit) = parallel.get(k) else { break };
                let out = run_unit(&base, &data, unit);
                results.lock().unwrap_or_else(|e| e.into_inner()).push(out);
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    for unit in &serial_units {
        results.push(run_unit(&base, &data, unit));
    }

    let mut total = Outcome::default();
    for o in results {
        total.fails.extend(o.fails);
        total.record.extend(o.record);
        total.notes.extend(o.notes);
        total.cases += o.cases;
        total.added += o.added;
        total.na += o.na;
        total.calls += o.calls;
    }
    if let Some(path) = std::env::var_os("AWF_HOOKS_RECORD") {
        // 沒有步驟的行（group、註解）照原樣；其餘用這次的結果
        let mut lines = data.lines.clone();
        for (line, text) in &total.record {
            lines[line - 1] = text.clone();
        }
        std::fs::write(&path, lines.join("\n") + "\n").expect("寫 AWF_HOOKS_RECORD");
    }
    for n in &total.notes {
        eprintln!("{n}");
    }
    eprintln!(
        "hook 案例：{} 個（其中 {} 個不適用於執行檔、只列名稱），另有新增的檢查 {} 個；呼叫 awf hook {} 次，{} 組，{:.1} 秒",
        total.cases, total.na, total.added, total.calls, data.groups.len(), started.elapsed().as_secs_f64()
    );
    total.fails.sort();
    total.fails.dedup(); // 拆開跑的組，搭環境的步驟每一份都做一次
    if !total.fails.is_empty() {
        let shown: Vec<&str> = total.fails.iter().take(30).map(|(_, m)| m.as_str()).collect();
        panic!("{} 個案例和 tests/hooks/cases.jsonl 不一致（列前 {} 個）：\n{}", total.fails.len(), shown.len(), shown.join("\n"));
    }
    assert!(total.cases >= MIN_CASES, "案例數 {} 少於 {MIN_CASES}：資料檔被截短了？", total.cases);
}

#[test]
fn helpers_match_python() {
    let _serial = serial();
    let v: Value = serde_json::from_str(r#"{"a": "中\"\\\n\t\u0001x", "b": [1, true, null, {"c": -2}], "d": {}}"#).expect("json");
    assert_eq!(dumps(&v), r#"{"a": "中\"\\\n\t\u0001x", "b": [1, true, null, {"c": -2}], "d": {}}"#);
    assert_eq!(expand("a@REPEAT:3:2:xy@b@TMP@/c", "/t"), "axyxyxyb/t/c");
    assert_eq!(expand("@REPEAT:2:1:中@@REPEAT:2:2:\nx@", "/t"), "中中\nx\nx");
    assert_eq!(expand("@REPEAT:壞掉", "/t"), "@REPEAT:壞掉");
    assert!(runs_hook("\"${CLAUDE_PLUGIN_ROOT}/bin/awf\" hook guard-shell", "guard-shell"));
    assert!(runs_hook("\"C:/Program Files/awf/awf.exe\" hook guard-paths", "guard-paths"));
    assert!(!runs_hook("\"/opt/bin/awf\" hook guard-shell-x", "guard-shell"));
    assert!(!runs_hook("python3 hook guard-shell", "guard-shell"));
}
