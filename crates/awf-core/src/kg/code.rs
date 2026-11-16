//! 程式索引：用 tree-sitter 把一個原始檔解析成「定義、import、呼叫、繼承與實作、`@awf:` 標記」。
//!
//! 只看單一檔案的語法樹，不跨檔；跨檔解析在 `resolve.rs`。原始碼一律當資料。
//! 行號 1 起算。樹太深（超過 [`MAX_DEPTH`]）的部分不往下走。

use regex::Regex;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser};

/// 語法樹往下走的層數上限（很長的運算式會巢狀很深）。
const MAX_DEPTH: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lang {
    Ts,
    Tsx,
    Js,
    Py,
    Go,
    Java,
    Rust,
    CSharp,
    Kotlin,
    Php,
    Ruby,
}

/// 支援的語言（顯示用）。
pub const LANG_NAMES: [&str; 10] = ["typescript", "javascript", "python", "go", "java", "rust", "csharp", "kotlin", "php", "ruby"];

impl Lang {
    /// 依副檔名判斷語言。
    pub fn from_path(path: &str) -> Option<Lang> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        Some(match ext.as_str() {
            "ts" | "mts" | "cts" => Lang::Ts,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Lang::Js,
            "py" => Lang::Py,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "rs" => Lang::Rust,
            "cs" => Lang::CSharp,
            "kt" | "kts" => Lang::Kotlin,
            "php" => Lang::Php,
            "rb" => Lang::Ruby,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Ts | Lang::Tsx => "typescript",
            Lang::Js => "javascript",
            Lang::Py => "python",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::Rust => "rust",
            Lang::CSharp => "csharp",
            Lang::Kotlin => "kotlin",
            Lang::Php => "php",
            Lang::Ruby => "ruby",
        }
    }

    pub fn from_name(s: &str) -> Option<Lang> {
        Some(match s {
            "typescript" => Lang::Ts,
            "javascript" => Lang::Js,
            "python" => Lang::Py,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "rust" => Lang::Rust,
            "csharp" => Lang::CSharp,
            "kotlin" => Lang::Kotlin,
            "php" => Lang::Php,
            "ruby" => Lang::Ruby,
            _ => return None,
        })
    }

    fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Js => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Py => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        }
    }
}

/// 一個定義。`parent` 是外層定義的完整名稱（用 `.` 連接），最外層是空字串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    pub kind: &'static str,
    pub parent: String,
    pub start: usize,
    pub end: usize,
}

impl Def {
    pub fn qualified(&self) -> String {
        if self.parent.is_empty() { self.name.clone() } else { format!("{}.{}", self.parent, self.name) }
    }
}

/// 一個 import。`names` 是（原名, 這個檔裡用的名字）；整個模組綁到一個名字時原名是 `*`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub names: Vec<(String, String)>,
    pub line: usize,
}

/// 一次呼叫。`qual` 是點號前面的名字（沒有或太複雜是空字串）；`scope` 是所在定義的完整名稱。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub qual: String,
    pub scope: String,
    pub line: usize,
}

/// 繼承或實作。`kind` 是 `extends` 或 `implements`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inherit {
    pub scope: String,
    pub target: String,
    pub kind: &'static str,
    pub line: usize,
}

/// 註解裡的 `@awf:<id>`。`scope` 是它標的定義（所在的，或緊接在後面的），沒有就是空字串（整個檔）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub target: String,
    pub scope: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSyms {
    pub defs: Vec<Def>,
    pub imports: Vec<Import>,
    pub calls: Vec<Call>,
    pub inherits: Vec<Inherit>,
    pub marks: Vec<Mark>,
}

fn mark_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"@awf:([^\s*)\]}'"`,;]+)"#).expect("樣式"))
}

/// 一段文字裡的 `@awf:<id>`（結尾的句點、冒號去掉）。
pub fn marks_in(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (k, line) in text.split('\n').enumerate() {
        for c in mark_re().captures_iter(line) {
            let id = c[1].trim_end_matches(['.', ':', '。', '，', '、']).to_string();
            if !id.is_empty() && id.chars().count() <= 200 {
                out.push((k, id));
            }
        }
    }
    out
}

const IDENT_KINDS: [&str; 14] = [
    "identifier", "type_identifier", "property_identifier", "field_identifier", "simple_identifier", "constant", "name",
    "package_identifier", "private_property_identifier", "shorthand_property_identifier", "this", "self", "super", "variable_name",
];

fn is_ident(kind: &str) -> bool {
    IDENT_KINDS.contains(&kind)
}

struct Walker<'a> {
    lang: Lang,
    src: &'a [u8],
    out: FileSyms,
    /// 外層定義的完整名稱與種類。
    scope: Vec<(String, &'static str)>,
}

fn kids<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut cur = n.walk();
    n.named_children(&mut cur).collect()
}

fn field<'t>(n: Node<'t>, name: &str) -> Option<Node<'t>> {
    n.child_by_field_name(name)
}

fn fields<'t>(n: Node<'t>, name: &str) -> Vec<Node<'t>> {
    let mut cur = n.walk();
    n.children_by_field_name(name, &mut cur).filter(|c| c.is_named()).collect()
}

fn line(n: Node) -> usize {
    n.start_position().row + 1
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    let t = t.trim_start_matches(['r', 'b', 'f', 'u']).trim_start_matches('#');
    let t = if t.starts_with(['"', '\'', '`']) { t } else { s.trim() };
    t.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
}

impl<'a> Walker<'a> {
    fn text(&self, n: Node) -> &'a str {
        n.utf8_text(self.src).unwrap_or("")
    }

    fn cur_scope(&self) -> String {
        self.scope.last().map(|s| s.0.clone()).unwrap_or_default()
    }

    fn in_type(&self) -> bool {
        matches!(self.scope.last().map(|s| s.1), Some("class" | "interface" | "struct" | "trait" | "enum" | "impl" | "module_type"))
    }

    /// 型別或名稱節點的最後一段名字（`a.b.C<T>` → `C`）。
    fn type_name(&self, n: Node, depth: usize) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let k = n.kind();
        if is_ident(k) {
            return Some(self.text(n).trim_start_matches('$').to_string());
        }
        for f in ["name", "property", "attribute", "field"] {
            if let Some(c) = field(n, f) {
                if let Some(s) = self.type_name(c, depth + 1) {
                    return Some(s);
                }
            }
        }
        if let Some(c) = field(n, "type") {
            return self.type_name(c, depth + 1);
        }
        let ks = kids(n);
        let pick_last = matches!(
            k,
            "scoped_type_identifier" | "scoped_identifier" | "nested_type_identifier" | "member_expression" | "qualified_name" | "attribute"
                | "qualified_type" | "scope_resolution" | "selector_expression" | "qualified_identifier" | "user_type" | "navigation_expression"
                | "dotted_name" | "nested_identifier" | "namespace_name" | "member_access_expression"
        );
        if pick_last {
            return ks.iter().rev().find_map(|c| self.type_name(*c, depth + 1));
        }
        ks.iter().find_map(|c| self.type_name(*c, depth + 1))
    }

    /// 呼叫的對象：（名字, 點號前的名字）。
    fn callee(&self, n: Node, depth: usize) -> Option<(String, String)> {
        if depth > 6 {
            return None;
        }
        let k = n.kind();
        if is_ident(k) {
            let t = self.text(n);
            return if t.is_empty() { None } else { Some((t.trim_start_matches('$').to_string(), String::new())) };
        }
        match k {
            "generic_function" | "parenthesized_expression" | "await_expression" | "non_null_expression" | "generic_name" | "instantiation_expression" | "generic_type" => {
                let inner = field(n, "function").or_else(|| field(n, "name")).or_else(|| kids(n).into_iter().next())?;
                return self.callee(inner, depth + 1);
            }
            _ => {}
        }
        let name_node = ["property", "attribute", "field", "name", "method"].iter().find_map(|f| field(n, f));
        let qual_node = ["object", "operand", "value", "path", "expression", "scope", "receiver"].iter().find_map(|f| field(n, f));
        if let Some(nn) = name_node {
            let name = self.type_name(nn, 0)?;
            let qual = qual_node.map(|q| self.qual(q)).unwrap_or_default();
            return Some((name, qual));
        }
        if matches!(k, "navigation_expression" | "qualified_name" | "scoped_identifier" | "member_access_expression" | "scope_resolution") {
            let ks = kids(n);
            let name = ks.last().and_then(|c| self.type_name(*c, 0))?;
            let qual = if ks.len() >= 2 { self.qual(ks[0]) } else { String::new() };
            return Some((name, qual));
        }
        None
    }

    /// 呼叫的接收者：簡單的名字原樣；太複雜（另一個呼叫的結果之類）記成 `?`，和「沒有接收者」分開。
    fn qual(&self, n: Node) -> String {
        let s = self.simple(n);
        if s.is_empty() { "?".to_string() } else { s }
    }

    /// 簡單的限定詞：識別字原樣，`a.b` 取最後一段，其餘（呼叫的結果之類）是空字串。
    fn simple(&self, n: Node) -> String {
        let k = n.kind();
        if is_ident(k) {
            return self.text(n).trim_start_matches('$').to_string();
        }
        if k.contains("call") || k.contains("invocation") || k.contains("literal") || k.contains("string") {
            return String::new();
        }
        let t = self.text(n);
        if t.len() <= 80 && t.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$' | '\\')) {
            return t.rsplit(['.', ':', '\\']).find(|s| !s.is_empty()).unwrap_or("").trim_start_matches('$').to_string();
        }
        String::new()
    }

    fn add_call(&mut self, n: Node, target: Option<(String, String)>) {
        if let Some((name, qual)) = target {
            if !name.is_empty() && name.len() <= 120 {
                self.out.calls.push(Call { name, qual, scope: self.cur_scope(), line: line(n) });
            }
        }
    }

    // ------------------------------------------------------------ 定義
    /// 這個節點是不是定義：（名字, 種類, 指定的外層）。
    fn def_of(&self, n: Node) -> Option<(String, &'static str, Option<String>)> {
        let k = n.kind();
        let name = |f: &str| field(n, f).and_then(|c| self.type_name(c, 0));
        let fn_kind = if self.in_type() { "method" } else { "function" };
        let got: Option<(Option<String>, &'static str)> = match self.lang {
            Lang::Ts | Lang::Tsx | Lang::Js => match k {
                "function_declaration" | "generator_function_declaration" | "function_signature" => Some((name("name"), "function")),
                "class_declaration" | "abstract_class_declaration" | "class" => Some((name("name"), "class")),
                "method_definition" | "method_signature" | "abstract_method_signature" => Some((name("name"), "method")),
                "interface_declaration" => Some((name("name"), "interface")),
                "type_alias_declaration" => Some((name("name"), "type")),
                "enum_declaration" => Some((name("name"), "enum")),
                "internal_module" | "module" => Some((name("name"), "namespace")),
                "variable_declarator" | "public_field_definition" | "field_definition" => {
                    let v = field(n, "value").map(|v| v.kind());
                    if matches!(v, Some("arrow_function" | "function_expression" | "function" | "generator_function")) {
                        let nm = field(n, "name").or_else(|| field(n, "property")).filter(|c| is_ident(c.kind())).map(|c| self.text(c).to_string());
                        Some((nm, if k == "variable_declarator" { "function" } else { "method" }))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            Lang::Py => match k {
                "function_definition" => Some((name("name"), fn_kind)),
                "class_definition" => Some((name("name"), "class")),
                _ => None,
            },
            Lang::Go => match k {
                "function_declaration" => Some((name("name"), "function")),
                "method_declaration" => {
                    let recv = field(n, "receiver").and_then(|r| kids(r).into_iter().next()).and_then(|p| field(p, "type")).and_then(|t| self.type_name(t, 0));
                    return name("name").map(|nm| (nm, "method", Some(recv.unwrap_or_default())));
                }
                "method_elem" | "method_spec" => Some((name("name"), "method")),
                "type_spec" | "type_alias" => {
                    let tk = match field(n, "type").map(|t| t.kind()) {
                        Some("struct_type") => "struct",
                        Some("interface_type") => "interface",
                        _ => "type",
                    };
                    Some((name("name"), tk))
                }
                _ => None,
            },
            Lang::Java => match k {
                "class_declaration" | "record_declaration" => Some((name("name"), "class")),
                "interface_declaration" | "annotation_type_declaration" => Some((name("name"), "interface")),
                "enum_declaration" => Some((name("name"), "enum")),
                "method_declaration" | "constructor_declaration" => Some((name("name"), "method")),
                _ => None,
            },
            Lang::Rust => match k {
                "function_item" | "function_signature_item" => Some((name("name"), fn_kind)),
                "struct_item" | "union_item" => Some((name("name"), "struct")),
                "enum_item" => Some((name("name"), "enum")),
                "trait_item" => Some((name("name"), "trait")),
                "type_item" => Some((name("name"), "type")),
                "const_item" | "static_item" => Some((name("name"), "const")),
                "mod_item" => Some((name("name"), "module")),
                "macro_definition" => Some((name("name"), "macro")),
                _ => None,
            },
            Lang::CSharp => match k {
                "class_declaration" | "record_declaration" => Some((name("name"), "class")),
                "struct_declaration" => Some((name("name"), "struct")),
                "interface_declaration" => Some((name("name"), "interface")),
                "enum_declaration" => Some((name("name"), "enum")),
                "method_declaration" | "constructor_declaration" | "local_function_statement" => Some((name("name"), "method")),
                _ => None,
            },
            Lang::Kotlin => match k {
                "function_declaration" => Some((name("name").or_else(|| self.first_ident(n)), fn_kind)),
                "class_declaration" => {
                    let is_iface = (0..n.child_count()).filter_map(|i| n.child(i as u32)).any(|c| c.kind() == "interface");
                    Some((name("name").or_else(|| self.first_ident(n)), if is_iface { "interface" } else { "class" }))
                }
                "object_declaration" => Some((name("name").or_else(|| self.first_ident(n)), "class")),
                _ => None,
            },
            Lang::Php => match k {
                "function_definition" => Some((name("name"), "function")),
                "class_declaration" => Some((name("name"), "class")),
                "interface_declaration" => Some((name("name"), "interface")),
                "trait_declaration" => Some((name("name"), "trait")),
                "enum_declaration" => Some((name("name"), "enum")),
                "method_declaration" => Some((name("name"), "method")),
                _ => None,
            },
            Lang::Ruby => match k {
                "method" | "singleton_method" => Some((name("name"), fn_kind)),
                "class" => Some((name("name"), "class")),
                "module" => Some((name("name"), "module_type")),
                _ => None,
            },
        };
        match got {
            Some((Some(nm), kind)) if !nm.is_empty() && nm.len() <= 200 => Some((nm, if kind == "module_type" { "module" } else { kind }, None)),
            _ => None,
        }
    }

    fn first_ident(&self, n: Node) -> Option<String> {
        kids(n).into_iter().find(|c| is_ident(c.kind())).map(|c| self.text(c).to_string())
    }

    /// 不是定義、但底下的定義要掛在某個名字下面（Rust 的 `impl T`）。
    fn scope_only(&self, n: Node) -> Option<String> {
        if self.lang == Lang::Rust && n.kind() == "impl_item" {
            return field(n, "type").and_then(|t| self.type_name(t, 0));
        }
        None
    }

    // ------------------------------------------------------------ import
    fn imports_of(&mut self, n: Node) {
        let k = n.kind();
        let ln = line(n);
        match self.lang {
            Lang::Ts | Lang::Tsx | Lang::Js => match k {
                "import_statement" => {
                    let Some(src) = field(n, "source") else { return };
                    let mut names = Vec::new();
                    for c in kids(n) {
                        if c.kind() == "import_clause" {
                            for p in kids(c) {
                                match p.kind() {
                                    "identifier" => names.push(("default".to_string(), self.text(p).to_string())),
                                    "namespace_import" => {
                                        if let Some(id) = self.first_ident(p) {
                                            names.push(("*".to_string(), id));
                                        }
                                    }
                                    "named_imports" => {
                                        for s in kids(p) {
                                            if s.kind() == "import_specifier" {
                                                let nm = field(s, "name").map(|x| self.text(x).to_string()).unwrap_or_default();
                                                let al = field(s, "alias").map(|x| self.text(x).to_string()).unwrap_or_else(|| nm.clone());
                                                if !nm.is_empty() {
                                                    names.push((nm, al));
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    self.out.imports.push(Import { module: unquote(self.text(src)), names, line: ln });
                }
                "export_statement" => {
                    if let Some(src) = field(n, "source") {
                        self.out.imports.push(Import { module: unquote(self.text(src)), names: Vec::new(), line: ln });
                    }
                }
                "call_expression" => {
                    let f = field(n, "function").map(|f| (f.kind(), self.text(f)));
                    if matches!(f, Some(("identifier", "require")) | Some(("import", _))) {
                        let arg = field(n, "arguments").and_then(|a| kids(a).into_iter().next());
                        let Some(arg) = arg.filter(|a| a.kind() == "string") else { return };
                        let mut names = Vec::new();
                        if let Some(p) = n.parent().filter(|p| p.kind() == "variable_declarator") {
                            if let Some(nm) = field(p, "name") {
                                if nm.kind() == "identifier" {
                                    names.push(("*".to_string(), self.text(nm).to_string()));
                                } else if nm.kind() == "object_pattern" {
                                    for s in kids(nm) {
                                        match s.kind() {
                                            "shorthand_property_identifier_pattern" => names.push((self.text(s).to_string(), self.text(s).to_string())),
                                            "pair_pattern" => {
                                                let key = field(s, "key").map(|x| self.text(x).to_string()).unwrap_or_default();
                                                let val = field(s, "value").map(|x| self.text(x).to_string()).unwrap_or_default();
                                                if !key.is_empty() && !val.is_empty() {
                                                    names.push((key, val));
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        self.out.imports.push(Import { module: unquote(self.text(arg)), names, line: ln });
                    }
                }
                _ => {}
            },
            Lang::Py => match k {
                "import_statement" => {
                    for c in fields(n, "name") {
                        let (m, al) = if c.kind() == "aliased_import" {
                            let m = field(c, "name").map(|x| self.text(x).to_string()).unwrap_or_default();
                            let al = field(c, "alias").map(|x| self.text(x).to_string()).unwrap_or_default();
                            (m, al)
                        } else {
                            let m = self.text(c).to_string();
                            (m.clone(), m)
                        };
                        if !m.is_empty() {
                            self.out.imports.push(Import { module: m, names: vec![("*".to_string(), al)], line: ln });
                        }
                    }
                }
                "import_from_statement" => {
                    let module = field(n, "module_name").map(|x| self.text(x).split_whitespace().collect::<String>()).unwrap_or_default();
                    let mut names = Vec::new();
                    for c in fields(n, "name") {
                        if c.kind() == "aliased_import" {
                            let m = field(c, "name").map(|x| self.text(x).to_string()).unwrap_or_default();
                            let al = field(c, "alias").map(|x| self.text(x).to_string()).unwrap_or_default();
                            names.push((m, al));
                        } else {
                            names.push((self.text(c).to_string(), self.text(c).to_string()));
                        }
                    }
                    if kids(n).iter().any(|c| c.kind() == "wildcard_import") {
                        names.push(("*".to_string(), "*".to_string()));
                    }
                    if !module.is_empty() {
                        self.out.imports.push(Import { module, names, line: ln });
                    }
                }
                _ => {}
            },
            Lang::Go => {
                if k == "import_spec" {
                    let Some(p) = field(n, "path") else { return };
                    let module = unquote(self.text(p));
                    let alias = field(n, "name").map(|x| self.text(x).to_string()).unwrap_or_else(|| module.rsplit('/').next().unwrap_or("").to_string());
                    self.out.imports.push(Import { module, names: vec![("*".to_string(), alias)], line: ln });
                }
            }
            Lang::Java => {
                if k == "import_declaration" {
                    let ks = kids(n);
                    let Some(first) = ks.iter().find(|c| matches!(c.kind(), "scoped_identifier" | "identifier")) else { return };
                    let module = self.text(*first).split_whitespace().collect::<String>();
                    let wild = ks.iter().any(|c| c.kind() == "asterisk");
                    let last = module.rsplit('.').next().unwrap_or("").to_string();
                    let names = if wild { vec![("*".to_string(), "*".to_string())] } else { vec![(last.clone(), last)] };
                    self.out.imports.push(Import { module, names, line: ln });
                }
            }
            Lang::Rust => match k {
                "use_declaration" => {
                    if let Some(arg) = field(n, "argument") {
                        let mut leaves = Vec::new();
                        self.use_tree(arg, "", &mut leaves, 0);
                        for (path, alias) in leaves {
                            let last = path.rsplit("::").next().unwrap_or("").to_string();
                            self.out.imports.push(Import { module: path, names: vec![(last, alias)], line: ln });
                        }
                    }
                }
                "mod_item" => {
                    if field(n, "body").is_none() {
                        if let Some(nm) = field(n, "name") {
                            let s = self.text(nm).to_string();
                            self.out.imports.push(Import { module: format!("mod:{s}"), names: vec![("*".to_string(), s)], line: ln });
                        }
                    }
                }
                _ => {}
            },
            Lang::CSharp => {
                if k == "using_directive" {
                    let ks = kids(n);
                    let Some(last) = ks.last() else { return };
                    let module = self.text(*last).split_whitespace().collect::<String>();
                    let alias = field(n, "name").map(|x| self.text(x).to_string()).unwrap_or_else(|| "*".to_string());
                    self.out.imports.push(Import { module, names: vec![("*".to_string(), alias)], line: ln });
                }
            }
            Lang::Kotlin => {
                if k == "import" || k == "import_header" {
                    let ks = kids(n);
                    let Some(first) = ks.first() else { return };
                    let module = self.text(*first).split_whitespace().collect::<String>();
                    let wild = self.text(n).trim_end().ends_with('*');
                    let last = module.rsplit('.').next().unwrap_or("").to_string();
                    let alias = if ks.len() >= 2 && is_ident(ks[ks.len() - 1].kind()) { self.text(ks[ks.len() - 1]).to_string() } else { last.clone() };
                    let names = if wild { vec![("*".to_string(), "*".to_string())] } else { vec![(last, alias)] };
                    self.out.imports.push(Import { module, names, line: ln });
                }
            }
            Lang::Php => match k {
                "namespace_use_clause" => {
                    let ks = kids(n);
                    let Some(first) = ks.iter().find(|c| matches!(c.kind(), "qualified_name" | "name")) else { return };
                    let module = self.text(*first).trim_start_matches('\\').to_string();
                    let last = module.rsplit('\\').next().unwrap_or("").to_string();
                    let alias = field(n, "alias").map(|x| self.text(x).to_string()).unwrap_or_else(|| last.clone());
                    self.out.imports.push(Import { module, names: vec![(last, alias)], line: ln });
                }
                "require_expression" | "require_once_expression" | "include_expression" | "include_once_expression" => {
                    if let Some(s) = kids(n).into_iter().find(|c| matches!(c.kind(), "string" | "encapsed_string")) {
                        let m = unquote(self.text(s));
                        let m = if m.starts_with('.') || m.starts_with('/') { m } else { format!("./{m}") };
                        self.out.imports.push(Import { module: m, names: Vec::new(), line: ln });
                    }
                }
                _ => {}
            },
            Lang::Ruby => {
                if k == "call" && field(n, "receiver").is_none() {
                    let m = field(n, "method").map(|x| self.text(x)).unwrap_or("");
                    if m == "require" || m == "require_relative" {
                        let arg = field(n, "arguments").and_then(|a| kids(a).into_iter().next()).filter(|a| a.kind() == "string");
                        if let Some(a) = arg {
                            let s = unquote(self.text(a));
                            let module = if m == "require_relative" && !s.starts_with('.') { format!("./{s}") } else { s };
                            self.out.imports.push(Import { module, names: Vec::new(), line: ln });
                        }
                    }
                }
            }
        }
    }

    /// 把 Rust 的 use 樹攤平成（完整路徑, 綁定的名字）。
    fn use_tree(&self, n: Node, prefix: &str, out: &mut Vec<(String, String)>, depth: usize) {
        if depth > 8 || out.len() > 200 {
            return;
        }
        let join = |p: &str, s: &str| if p.is_empty() { s.to_string() } else if s.is_empty() { p.to_string() } else { format!("{p}::{s}") };
        match n.kind() {
            "use_as_clause" => {
                let p = field(n, "path").map(|x| self.text(x).split_whitespace().collect::<String>()).unwrap_or_default();
                let a = field(n, "alias").map(|x| self.text(x).to_string()).unwrap_or_default();
                out.push((join(prefix, &p), a));
            }
            "scoped_use_list" => {
                let p = field(n, "path").map(|x| self.text(x).split_whitespace().collect::<String>()).unwrap_or_default();
                let pre = join(prefix, &p);
                if let Some(l) = field(n, "list") {
                    for c in kids(l) {
                        self.use_tree(c, &pre, out, depth + 1);
                    }
                }
            }
            "use_list" => {
                for c in kids(n) {
                    self.use_tree(c, prefix, out, depth + 1);
                }
            }
            "use_wildcard" => {
                let p = kids(n).first().map(|x| self.text(*x).split_whitespace().collect::<String>()).unwrap_or_default();
                out.push((join(&join(prefix, &p), "*"), "*".to_string()));
            }
            "self" => {
                let last = prefix.rsplit("::").next().unwrap_or("").to_string();
                out.push((prefix.to_string(), last));
            }
            _ => {
                let p = self.text(n).split_whitespace().collect::<String>();
                let full = join(prefix, &p);
                let last = full.rsplit("::").next().unwrap_or("").to_string();
                out.push((full, last));
            }
        }
    }

    // ------------------------------------------------------------ 呼叫
    fn calls_of(&mut self, n: Node) {
        let k = n.kind();
        match self.lang {
            Lang::Ts | Lang::Tsx | Lang::Js => match k {
                "call_expression" => {
                    let t = field(n, "function").and_then(|f| self.callee(f, 0));
                    if !matches!(&t, Some((nm, q)) if q.is_empty() && (nm == "require" || nm == "import")) {
                        self.add_call(n, t);
                    }
                }
                "new_expression" => {
                    let t = field(n, "constructor").and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
                _ => {}
            },
            Lang::Py => {
                if k == "call" {
                    let t = field(n, "function").and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
            }
            Lang::Go | Lang::Rust => {
                if k == "call_expression" {
                    let t = field(n, "function").and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
            }
            Lang::Java => match k {
                "method_invocation" => {
                    let nm = field(n, "name").map(|x| self.text(x).to_string());
                    let q = field(n, "object").map(|o| self.qual(o)).unwrap_or_default();
                    self.add_call(n, nm.map(|x| (x, q)));
                }
                "object_creation_expression" => {
                    let t = field(n, "type").and_then(|x| self.type_name(x, 0));
                    self.add_call(n, t.map(|x| (x, String::new())));
                }
                _ => {}
            },
            Lang::CSharp => match k {
                "invocation_expression" => {
                    let t = field(n, "function").and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
                "object_creation_expression" => {
                    let t = field(n, "type").and_then(|x| self.type_name(x, 0));
                    self.add_call(n, t.map(|x| (x, String::new())));
                }
                _ => {}
            },
            Lang::Kotlin => {
                if k == "call_expression" {
                    let t = kids(n).into_iter().next().and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
            }
            Lang::Php => match k {
                "function_call_expression" => {
                    let t = field(n, "function").and_then(|f| self.callee(f, 0));
                    self.add_call(n, t);
                }
                "member_call_expression" | "nullsafe_member_call_expression" | "scoped_call_expression" => {
                    let nm = field(n, "name").and_then(|x| self.type_name(x, 0));
                    let q = field(n, "object").or_else(|| field(n, "scope")).map(|o| self.qual(o)).unwrap_or_default();
                    self.add_call(n, nm.map(|x| (x, q)));
                }
                "object_creation_expression" => {
                    let t = kids(n).into_iter().find(|c| matches!(c.kind(), "name" | "qualified_name")).and_then(|x| self.type_name(x, 0));
                    self.add_call(n, t.map(|x| (x, String::new())));
                }
                _ => {}
            },
            Lang::Ruby => {
                if k == "call" {
                    let nm = field(n, "method").map(|x| self.text(x).to_string());
                    let q = field(n, "receiver").map(|o| self.qual(o)).unwrap_or_default();
                    if !matches!(nm.as_deref(), Some("require" | "require_relative")) {
                        self.add_call(n, nm.map(|x| (x, q)));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ 繼承與實作
    fn inherit(&mut self, scope: &str, n: Node, kind: &'static str) {
        if let Some(t) = self.type_name(n, 0) {
            if !t.is_empty() && t.len() <= 200 {
                self.out.inherits.push(Inherit { scope: scope.to_string(), target: t, kind, line: line(n) });
            }
        }
    }

    /// `n` 是剛認出來的定義，`scope` 是它的完整名稱。
    fn inherits_of(&mut self, n: Node, scope: &str) {
        let k = n.kind();
        match self.lang {
            Lang::Ts | Lang::Tsx | Lang::Js => {
                for c in kids(n) {
                    match c.kind() {
                        "class_heritage" => {
                            let inner = kids(c);
                            let has_clauses = inner.iter().any(|x| matches!(x.kind(), "extends_clause" | "implements_clause"));
                            if !has_clauses {
                                if let Some(x) = inner.first() {
                                    self.inherit(scope, *x, "extends");
                                }
                            }
                            for x in inner {
                                let ik = match x.kind() {
                                    "extends_clause" => "extends",
                                    "implements_clause" => "implements",
                                    _ => continue,
                                };
                                for t in kids(x) {
                                    if t.kind() != "type_arguments" {
                                        self.inherit(scope, t, ik);
                                    }
                                }
                            }
                        }
                        "extends_type_clause" | "extends_clause" => {
                            for t in kids(c) {
                                self.inherit(scope, t, "extends");
                            }
                        }
                        _ => {}
                    }
                }
            }
            Lang::Py => {
                if k == "class_definition" {
                    if let Some(a) = field(n, "superclasses") {
                        for c in kids(a) {
                            if matches!(c.kind(), "identifier" | "attribute") {
                                self.inherit(scope, c, "extends");
                            }
                        }
                    }
                }
            }
            Lang::Go => {
                if k == "type_spec" {
                    if let Some(t) = field(n, "type").filter(|t| t.kind() == "struct_type") {
                        let list = kids(t).into_iter().find(|c| c.kind() == "field_declaration_list");
                        for f in list.map(kids).unwrap_or_default() {
                            if f.kind() == "field_declaration" && field(f, "name").is_none() {
                                if let Some(ty) = field(f, "type") {
                                    self.inherit(scope, ty, "extends");
                                }
                            }
                        }
                    }
                }
            }
            Lang::Java => {
                if let Some(s) = field(n, "superclass") {
                    for t in kids(s) {
                        self.inherit(scope, t, "extends");
                    }
                }
                if let Some(s) = field(n, "interfaces") {
                    for l in kids(s) {
                        for t in if l.kind() == "type_list" { kids(l) } else { vec![l] } {
                            self.inherit(scope, t, "implements");
                        }
                    }
                }
                for c in kids(n) {
                    if c.kind() == "extends_interfaces" {
                        for l in kids(c) {
                            for t in if l.kind() == "type_list" { kids(l) } else { vec![l] } {
                                self.inherit(scope, t, "extends");
                            }
                        }
                    }
                }
            }
            Lang::Rust => {
                if k == "trait_item" {
                    if let Some(b) = field(n, "bounds") {
                        for t in kids(b) {
                            if !matches!(t.kind(), "lifetime") {
                                self.inherit(scope, t, "extends");
                            }
                        }
                    }
                }
            }
            Lang::CSharp => {
                for c in kids(n) {
                    if c.kind() == "base_list" {
                        for t in kids(c) {
                            self.inherit(scope, t, "extends");
                        }
                    }
                }
            }
            Lang::Kotlin => {
                for c in kids(n) {
                    if c.kind() == "delegation_specifiers" {
                        for t in kids(c) {
                            self.inherit(scope, t, "extends");
                        }
                    }
                }
            }
            Lang::Php => {
                for c in kids(n) {
                    let ik = match c.kind() {
                        "base_clause" => "extends",
                        "class_interface_clause" => "implements",
                        _ => continue,
                    };
                    for t in kids(c) {
                        self.inherit(scope, t, ik);
                    }
                }
            }
            Lang::Ruby => {
                if k == "class" {
                    if let Some(s) = field(n, "superclass") {
                        if let Some(t) = kids(s).into_iter().next() {
                            self.inherit(scope, t, "extends");
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ 走訪
    fn visit(&mut self, n: Node, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let k = n.kind();
        if k.contains("comment") {
            let base = line(n);
            for (off, id) in marks_in(self.text(n)) {
                self.out.marks.push(Mark { target: id, scope: String::new(), line: base + off });
            }
            return;
        }
        let mut pushed = false;
        if let Some((name, kind, parent)) = self.def_of(n) {
            let parent = parent.unwrap_or_else(|| self.cur_scope());
            let d = Def { name, kind, parent, start: line(n), end: n.end_position().row + 1 };
            let q = d.qualified();
            self.out.defs.push(d);
            self.inherits_of(n, &q);
            self.scope.push((q, kind));
            pushed = true;
        } else if let Some(name) = self.scope_only(n) {
            let outer = self.cur_scope();
            let q = if outer.is_empty() { name } else { format!("{outer}.{name}") };
            if let Some(tr) = field(n, "trait") {
                self.inherit(&q, tr, "implements");
            }
            self.scope.push((q, "impl"));
            pushed = true;
        }
        self.imports_of(n);
        self.calls_of(n);
        for c in kids(n) {
            self.visit(c, depth + 1);
        }
        if pushed {
            self.scope.pop();
        }
    }
}

/// 標記掛到哪個定義：包住那一行的最小定義；沒有的話，三行之內開始的定義；都沒有就是整個檔。
fn attach_marks(out: &mut FileSyms) {
    for m in &mut out.marks {
        let inside = out.defs.iter().filter(|d| d.start <= m.line && m.line <= d.end).min_by_key(|d| (d.end - d.start, d.start));
        let pick = inside.or_else(|| out.defs.iter().filter(|d| d.start > m.line && d.start <= m.line + 3).min_by_key(|d| d.start));
        if let Some(d) = pick {
            m.scope = d.qualified();
        }
    }
}

/// 解析一個原始檔。語法有錯的部分 tree-sitter 會跳過，其餘照收；完全解析不了回 None。
pub fn parse(lang: Lang, src: &str) -> Option<FileSyms> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).ok()?;
    let tree = parser.parse(src.as_bytes(), None)?;
    let mut w = Walker { lang, src: src.as_bytes(), out: FileSyms::default(), scope: Vec::new() };
    w.visit(tree.root_node(), 0);
    let mut out = w.out;
    attach_marks(&mut out);
    Some(out)
}

/// 除錯與測試用：語法樹的 S 運算式。
pub fn sexp(lang: Lang, src: &str) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).ok()?;
    Some(parser.parse(src.as_bytes(), None)?.root_node().to_sexp())
}

/// 測試檔（給「受影響的測試」用）：路徑裡有 test／tests／spec／__tests__ 目錄，或檔名是常見的測試命名。
pub fn is_test_path(path: &str) -> bool {
    let low = path.to_lowercase();
    let name = low.rsplit('/').next().unwrap_or(&low);
    let stem = name.split('.').next().unwrap_or(name);
    low.split('/').rev().skip(1).any(|d| matches!(d, "test" | "tests" | "__tests__" | "spec"))
        || name.contains(".test.")
        || name.contains(".spec.")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || (path.ends_with("Test.java") || path.ends_with("Tests.java") || path.ends_with("Test.kt") || path.ends_with("Tests.cs") || path.ends_with("Test.php"))
}
