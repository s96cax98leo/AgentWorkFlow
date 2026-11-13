//! 去客戶化掃描與文件核對的對跑：照 Python 版錄下來的劇本（tests/parity/build.expected.json 的 `scrub` 與 `docs`）
//! 做一遍，逐字比對 exit code、stdout、stderr。
//! err_any 的步驟只比 exit code 與 stdout（Python 版在那裡直接當掉）；unordered 的步驟 stdout 逐行排序後再比
//! （Python 版用 set 走訪，同一輪有兩則以上錯誤時順序不固定）。
#![cfg(unix)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use xtask::{check_docs, pyre, scrub};

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).filter_map(|i| h.get(i * 2..i * 2 + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

fn sorted_lines(x: &str) -> String {
    let mut v: Vec<&str> = x.lines().collect();
    v.sort();
    v.join("\n")
}

/// 跑一份劇本；回（執行次數、只比 exit code 與 stdout 的次數、排序後才比的次數）。
fn play(section: &str, tag: &str) -> (usize, usize, usize) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("tests/parity/build.expected.json")).expect("讀不到基準");
    let doc: Value = serde_json::from_str(&text).expect("JSON");
    let base = std::env::temp_dir().join(format!("awf-xtask-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("建暫存目錄");
    let tmp_path = base.canonicalize().expect("canonicalize");
    let tmp = tmp_path.to_string_lossy().into_owned();
    let real = |x: &str| x.replace("@TMP@", &tmp);
    let norm = |x: &str| x.replace(&tmp, "@TMP@");
    let mut bad: Vec<String> = Vec::new();
    let (mut runs, mut loose, mut unordered) = (0, 0, 0);
    let steps = doc[section].as_array().expect("劇本");
    for (i, st) in steps.iter().enumerate() {
        let op = s(&st["op"]);
        let result = match op {
            "write" | "write_hex" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                let data = if st.get("hex").is_some() { unhex(s(&st["hex"])) } else { s(&st["text"]).as_bytes().to_vec() };
                std::fs::write(&p, data).expect("write");
                continue;
            }
            "remove" => {
                let p = real(s(&st["path"]));
                let meta = std::fs::symlink_metadata(&p).expect("要刪的路徑");
                if meta.is_dir() {
                    std::fs::remove_dir_all(&p).expect("rmtree");
                } else {
                    std::fs::remove_file(&p).expect("unlink");
                }
                continue;
            }
            "symlink" => {
                let p = real(s(&st["path"]));
                std::fs::create_dir_all(Path::new(&p).parent().expect("上層目錄")).expect("mkdir");
                std::os::unix::fs::symlink(s(&st["target"]), &p).expect("symlink");
                continue;
            }
            "scrub" => {
                let argv: Vec<String> = st["argv"].as_array().map(|a| a.iter().map(|x| real(s(x))).collect()).unwrap_or_default();
                scrub::run_captured(Path::new(&real(s(&st["root"]))), &argv)
            }
            "docs" => check_docs::run_captured(Path::new(&real(s(&st["root"])))),
            "docs_real" => check_docs::run_captured(&xtask::repo_root()),
            other => panic!("不認得的步驟 {other}"),
        };
        runs += 1;
        let (rc, out, err) = result;
        let mut diff = |what: &str, want: &str, got: &str| {
            if want != got {
                bad.push(format!("步驟 {i} {op} {} {what}\n  預期：{want:?}\n  實際：{got:?}", st["argv"]));
            }
        };
        diff("rc", &st["rc"].to_string(), &rc.to_string());
        if st.get("unordered").is_some() {
            unordered += 1;
            diff("stdout（排序後）", &sorted_lines(s(&st["out"])), &sorted_lines(&norm(&out)));
        } else {
            diff("stdout", s(&st["out"]), &norm(&out));
        }
        if st.get("err_any").is_none() {
            diff("stderr", s(&st["err"]), &norm(&err));
        } else {
            loose += 1;
            if err.trim().is_empty() {
                diff("出錯時 stderr 要有說明", "有說明", "空的");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    println!("{section}：{runs} 次執行，{} 次逐字相同，{loose} 次只比 exit code 與 stdout，{unordered} 次排序後才比", runs - loose - unordered);
    assert!(bad.is_empty(), "{} 處不同（共 {} 步）：\n{}", bad.len(), steps.len(), bad.join("\n"));
    (runs, loose, unordered)
}

#[test]
fn scrub_matches_python() {
    let (runs, _, _) = play("scrub", "scrub");
    assert!(runs > 40, "劇本太短：{runs}");
}

#[test]
fn check_docs_matches_python() {
    let (runs, _, _) = play("docs", "docs");
    assert!(runs > 50, "劇本太短：{runs}");
}

/// 公開清單（tests/scrub_terms.txt）的每一條規則都編得過，而且抓得到對應形狀的字串。
/// 會命中的字串一律在執行時組出來，不以完整形狀寫在檔案裡。
#[test]
fn public_terms_catch_their_shapes() {
    let root = xtask::repo_root();
    let base = std::env::temp_dir().join(format!("awf-xtask-public-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("tests")).expect("mkdir");
    std::fs::copy(root.join("tests/scrub_terms.txt"), base.join("tests/scrub_terms.txt")).expect("複製公開清單");
    let home = format!("/{}/{}", "Users", "demo");
    let linux = format!("/{}/{}/x", "home", "demo");
    let win = format!("C:{}{}{}demo", '\\', "Users", '\\');
    let token = format!("gh{}_{}", 'p', "a1B2".repeat(9));
    let cloud = format!("AK{}{}", "IA", "A1B2C3D4".repeat(2));
    let key = format!("-----{} PRIVATE {}-----", "BEGIN", "KEY");
    let mail = format!("someone@{}.com", "gmail");
    let hits = [home.as_str(), linux.as_str(), win.as_str(), token.as_str(), cloud.as_str(), key.as_str(), mail.as_str()];
    let clean = ["someone@example.com", "home/ 目錄", "ghp_ 太短", "x AKIA 太短"];
    let mut text = String::new();
    for h in hits.iter().chain(clean.iter()) {
        text.push_str(&format!("前綴 {h} 後綴\n"));
    }
    std::fs::write(base.join("sample.txt"), &text).expect("write");
    let (rc, out, err) = scrub::run_captured(&base, &[]);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(rc, 1, "{out}{err}");
    for (i, h) in hits.iter().enumerate() {
        assert!(out.contains(&format!("./sample.txt:{}:前綴 {h} 後綴", i + 1)), "第 {} 個形狀沒抓到", i + 1);
    }
    assert_eq!(out.lines().count(), 1 + hits.len(), "乾淨的行被誤判：{out}");
    assert_eq!(err, format!("去客戶化掃描：不合格，共 {} 處命中\n", hits.len()));
}

/// Python 接受、Rust 不接受的寫法：一律明確報錯（exit 非零），不會略過那條規則。
#[test]
fn unsupported_patterns_fail_loudly() {
    for bad in ["(?a)x", "[a--b]", "x\\", "[abc", "\\N{DIGIT ONE}", "(a)\\10", "\\pL", "[\\1x]"] {
        assert!(pyre::compile(bad).is_err(), "應該編不過：{bad}");
    }
    let base = std::env::temp_dir().join(format!("awf-xtask-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("tests")).expect("mkdir");
    std::fs::write(base.join("tests/scrub_terms.txt"), "ok-term\n(?a)secret-shape\n").expect("write");
    std::fs::write(base.join("a.txt"), "乾淨\n").expect("write");
    let (rc, out, err) = scrub::run_captured(&base, &[]);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(rc, 1);
    assert!(out.is_empty());
    assert!(err.contains("第 2 行") && !err.contains("secret-shape"), "錯誤訊息要指出行號、不印規則內容：{err}");
}

/// 改寫後的意思和 Python 相同的幾個邊界。
#[test]
fn translated_classes_follow_python() {
    let hit = |pat: &str, text: &str| pyre::compile(pat).expect("編得過").search(text).expect("跑得動");
    // 字元類別裡的 [ 是一般字元；Python 把 [[x]y 讀成「[ 或 x」接 y
    assert!(hit("[[x]y", "[y") && hit("[[x]y", "xy") && !hit("[[x]y", "y"));
    // && 在類別裡是兩個 &
    assert!(hit("[a&&b]", "&") && hit("[a&&b]", "b"));
    // \s 含 U+001F；\w 不含結合符號、含上標數字
    assert!(hit(r"a\sb", "a\u{1f}b") && hit(r"a[\s]b", "a\u{1c}b") && !hit(r"a\Sb", "a\u{1f}b"));
    assert!(!hit(r"a\wb", "a\u{301}b") && hit(r"a\wb", "a²b") && hit(r"a[^\w]b", "a\u{203f}b"));
    assert!(hit(r"\bword\b", "\u{301}word\u{203f}") && !hit(r"\bword\b", "²word"));
    // 不是次數的 { 是一般字元；反斜線接標點是那個字元本身；八進位
    assert!(hit("a{ 2}", "a{ 2}") && !hit("a{ 2}", "aa") && hit("a{", "a{") && hit("ax{,2}b", "ab") && hit("a{2}", "aa") && hit("a{}", "a{}"));
    assert!(hit(r"\<a", "<a") && !hit(r"\<a", "a") && hit(r"a\>", "a>") && !hit(r"a\>", "a") && hit(r"[\<]", "<"));
    assert!(hit(r"a\0b", "a\0b") && hit(r"\101", "A") && hit(r"[\101]", "A") && hit(r"(a)\1", "aa") && hit(r"a\.b", "a.b") && !hit(r"a\.b", "axb"));
    // (?x) 模式下 \s 仍然含一般空白
    assert!(hit(r"(?x) a \s b", "a b"));
    // 路徑名結尾有換行時，$ 也要配到換行之前
    let p = pyre::compile("name$").expect("編得過");
    assert!(!p.search("./name\n").expect("跑得動") && p.search_name("./name\n").expect("跑得動"));
}
