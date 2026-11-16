//! 程式索引：每種語言用一個小範例檔（`tests/kg_fixtures/`）驗「定義、import、呼叫」各至少一例，外加繼承與 `@awf:` 標記。

use awf_core::kg::code::{parse, FileSyms, Lang};

fn load(file: &str) -> FileSyms {
    let p = format!("{}/tests/kg_fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&p).expect("讀範例檔");
    let lang = Lang::from_path(file).expect("認得副檔名");
    parse(lang, &src).expect("解析")
}

fn has_def(s: &FileSyms, kind: &str, qual: &str) -> bool {
    s.defs.iter().any(|d| d.kind == kind && d.qualified() == qual)
}

fn has_import(s: &FileSyms, module: &str, orig: &str, alias: &str) -> bool {
    s.imports.iter().any(|i| i.module == module && (orig.is_empty() || i.names.iter().any(|(o, a)| o == orig && a == alias)))
}

fn has_call(s: &FileSyms, qual: &str, name: &str, scope: &str) -> bool {
    s.calls.iter().any(|c| c.qual == qual && c.name == name && c.scope == scope)
}

fn has_inherit(s: &FileSyms, kind: &str, scope: &str, target: &str) -> bool {
    s.inherits.iter().any(|i| i.kind == kind && i.scope == scope && i.target == target)
}

#[test]
fn typescript() {
    let s = load("sample.ts");
    assert!(has_def(&s, "interface", "Shape") && has_def(&s, "class", "Circle") && has_def(&s, "method", "Circle.area"));
    assert!(has_def(&s, "function", "build") && has_def(&s, "function", "main"));
    assert!(has_import(&s, "./util", "helper", "h") && has_import(&s, "node:path", "*", "path") && has_import(&s, "@/base", "default", "Base"));
    assert!(has_call(&s, "", "h", "Circle.area") && has_call(&s, "this", "radius", "Circle.area") && has_call(&s, "path", "join", "main"));
    assert!(has_call(&s, "", "Circle", "build"), "new 也算呼叫");
    assert!(has_inherit(&s, "extends", "Circle", "Base") && has_inherit(&s, "implements", "Circle", "Shape"));
    assert!(s.marks.iter().any(|m| m.target == "billing/export-invoice" && m.scope == "Circle"), "標記掛到緊接在後面的定義");
}

#[test]
fn tsx() {
    let s = load("sample.tsx");
    assert!(has_def(&s, "function", "App") && has_def(&s, "function", "track"));
    assert!(has_import(&s, "./button", "Button", "Button"));
    assert!(has_call(&s, "", "track", "App") && has_call(&s, "console", "log", "track"));
}

#[test]
fn javascript() {
    let s = load("sample.js");
    assert!(has_def(&s, "class", "Dog") && has_def(&s, "method", "Dog.speak") && has_def(&s, "function", "run"));
    assert!(has_import(&s, "fs", "*", "fs") && has_import(&s, "./util", "helper", "helper") && has_import(&s, "./mod.js", "default", "def"));
    assert!(has_call(&s, "", "helper", "Animal.speak") && has_call(&s, "", "def", "run"));
    assert!(has_call(&s, "?", "speak", "run"), "接收者是運算式時記成 ?");
    assert!(!s.calls.iter().any(|c| c.name == "require"), "require 是 import，不算呼叫");
    assert!(has_inherit(&s, "extends", "Dog", "Animal"));
}

#[test]
fn python() {
    let s = load("sample.py");
    assert!(has_def(&s, "class", "Service") && has_def(&s, "method", "Service.run") && has_def(&s, "function", "main"));
    assert!(has_import(&s, "os", "*", "os") && has_import(&s, "pkg.mod", "*", "pm") && has_import(&s, ".util", "other", "o") && has_import(&s, "..core", "Base", "Base"));
    assert!(has_call(&s, "self", "step", "Service.run") && has_call(&s, "", "helper", "Service.run") && has_call(&s, "pm", "compute", "Service.step"));
    assert!(has_inherit(&s, "extends", "Service", "Base"));
    assert!(s.marks.iter().any(|m| m.target == "billing/export-invoice#csv" && m.scope == "Service.run"), "標記掛到包住它的定義");
}

#[test]
fn go() {
    let s = load("sample.go");
    assert!(has_def(&s, "struct", "Server") && has_def(&s, "interface", "Runner") && has_def(&s, "method", "Server.Run") && has_def(&s, "function", "NewServer"));
    assert!(has_import(&s, "fmt", "*", "fmt") && has_import(&s, "example.com/acme/app/internal/util", "*", "util"));
    assert!(has_call(&s, "util", "Helper", "Server.Run") && has_call(&s, "", "NewServer", "main"));
    assert!(has_inherit(&s, "extends", "Server", "Base"), "內嵌的 struct");
}

#[test]
fn java() {
    let s = load("Sample.java");
    assert!(has_def(&s, "class", "Sample") && has_def(&s, "method", "Sample.run") && has_def(&s, "interface", "Runner"));
    assert!(has_import(&s, "com.acme.app.util.Helper", "Helper", "Helper"));
    assert!(has_call(&s, "Helper", "assist", "Sample.run") && has_call(&s, "", "step", "Sample.run") && has_call(&s, "", "Other", "Sample.run"));
    assert!(has_inherit(&s, "extends", "Sample", "Base") && has_inherit(&s, "implements", "Sample", "Runner") && has_inherit(&s, "extends", "Runner", "Marker"));
}

#[test]
fn rust() {
    let s = load("sample.rs");
    assert!(has_def(&s, "trait", "Runner") && has_def(&s, "struct", "Server") && has_def(&s, "method", "Server.new") && has_def(&s, "function", "main"));
    assert!(has_import(&s, "mod:util", "*", "util") && has_import(&s, "crate::core::helper", "helper", "h") && has_import(&s, "std::collections::HashMap", "HashMap", "HashMap"));
    assert!(has_call(&s, "", "h", "Server.step") && has_call(&s, "self", "step", "Server.run") && has_call(&s, "util", "assist", "Server.run") && has_call(&s, "Server", "new", "main"));
    assert!(has_inherit(&s, "implements", "Server", "Runner") && has_inherit(&s, "extends", "Runner", "Marker"));
    assert!(s.marks.iter().any(|m| m.target == "billing/export-invoice" && m.scope == "Server.run"));
}

#[test]
fn php() {
    let s = load("sample.php");
    assert!(has_def(&s, "class", "Sample") && has_def(&s, "method", "Sample.run") && has_def(&s, "function", "format_name"));
    assert!(has_import(&s, "Acme\\App\\Util\\Helper", "Helper", "Helper") && has_import(&s, "Acme\\App\\Contracts\\Runner", "Runner", "R") && has_import(&s, "./bootstrap.php", "", ""));
    assert!(has_call(&s, "Helper", "assist", "Sample.run") && has_call(&s, "this", "step", "Sample.run") && has_call(&s, "", "format_name", "Sample.run"));
    assert!(has_inherit(&s, "extends", "Sample", "Base") && has_inherit(&s, "implements", "Sample", "R"));
}

#[test]
fn ruby() {
    let s = load("sample.rb");
    assert!(has_def(&s, "module", "Acme") && has_def(&s, "class", "Acme.Sample") && has_def(&s, "method", "Acme.Sample.run") && has_def(&s, "function", "helper"));
    assert!(has_import(&s, "json", "", "") && has_import(&s, "./util", "", ""));
    assert!(has_call(&s, "Util", "assist", "Acme.Sample.run") && has_call(&s, "", "helper", "Acme.Sample.run") && has_call(&s, "JSON", "dump", "helper"));
    assert!(has_inherit(&s, "extends", "Acme.Sample", "Base"));
}

/// 選用的語言：預設的執行檔沒有放文法，副檔名不認得（不會進圖）。
#[test]
fn optional_languages() {
    assert_eq!(Lang::from_path("a/Sample.cs").is_some(), cfg!(feature = "lang-csharp"));
    assert_eq!(Lang::from_path("a/Sample.kt").is_some(), cfg!(feature = "lang-kotlin"));
    assert!(Lang::from_path("a/readme.md").is_none());
}

#[cfg(feature = "lang-csharp")]
#[test]
fn csharp() {
    let s = load("Sample.cs");
    assert!(has_def(&s, "class", "Sample") && has_def(&s, "method", "Sample.Run") && has_def(&s, "interface", "IRunner"));
    assert!(has_import(&s, "Acme.App.Util", "", ""));
    assert!(has_call(&s, "Helper", "Assist", "Sample.Run") && has_call(&s, "", "Step", "Sample.Run"));
    assert!(has_inherit(&s, "extends", "Sample", "Base"));
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin() {
    let s = load("Sample.kt");
    assert!(has_def(&s, "class", "Sample") && has_def(&s, "method", "Sample.run") && has_def(&s, "function", "main"));
    assert!(has_import(&s, "com.acme.app.util.assist", "assist", "a"));
    assert!(has_call(&s, "Helper", "assist", "Sample.run") && has_call(&s, "", "step", "Sample.run"));
    assert!(has_inherit(&s, "extends", "Sample", "Base"));
}
