use awf_core::kg::code::{parse, sexp, Lang};

#[test]
fn dump() {
    for f in ["sample.ts", "sample.tsx", "sample.js", "sample.py", "sample.go", "Sample.java", "sample.rs", "Sample.cs", "Sample.kt", "sample.php", "sample.rb"] {
        let p = format!("{}/tests/kg_fixtures/{f}", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&p).unwrap();
        let lang = Lang::from_path(f).unwrap();
        let s = parse(lang, &src).unwrap();
        println!("=== {f}");
        for d in &s.defs {
            println!("  def {} {} [{}-{}]", d.kind, d.qualified(), d.start, d.end);
        }
        for d in &s.imports {
            println!("  import {} {:?} @{}", d.module, d.names, d.line);
        }
        for d in &s.calls {
            println!("  call {}|{} in {} @{}", d.qual, d.name, d.scope, d.line);
        }
        for d in &s.inherits {
            println!("  {} {} -> {} @{}", d.kind, d.scope, d.target, d.line);
        }
        for d in &s.marks {
            println!("  mark {} on {} @{}", d.target, d.scope, d.line);
        }
        if std::env::var("KG_SEXP").is_ok_and(|v| f.ends_with(&v)) {
            println!("{}", sexp(lang, &src).unwrap());
        }
    }
}
