//! Per-language tree-sitter extractors. Reached only through the `extract`
//! facade in `mod.rs`; nothing here is visible outside the `index` module.

use super::Symbol;
use crate::syntax::Lang;
use tree_sitter::{Node, Tree};

pub(super) fn extract_typescript(tree: &Tree, text: &str, lang: Lang) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" => collect_import(&child, text, &mut imports),
            "function_declaration" => push_named(&mut symbols, &child, text, "function", lang),
            "class_declaration" => push_named(&mut symbols, &child, text, "class", lang),
            "interface_declaration" => push_named(&mut symbols, &child, text, "interface", lang),
            "type_alias_declaration" => push_named(&mut symbols, &child, text, "type", lang),
            "lexical_declaration" | "variable_declaration" => {
                collect_var_names(&mut symbols, &child, text, lang)
            }
            "export_statement" => {
                if let Some(inner) = child.named_child(0) {
                    match inner.kind() {
                        "function_declaration" => {
                            push_named(&mut symbols, &inner, text, "function", lang)
                        }
                        "class_declaration" => {
                            push_named(&mut symbols, &inner, text, "class", lang)
                        }
                        "interface_declaration" => {
                            push_named(&mut symbols, &inner, text, "interface", lang)
                        }
                        "type_alias_declaration" => {
                            push_named(&mut symbols, &inner, text, "type", lang)
                        }
                        "lexical_declaration" | "variable_declaration" => {
                            collect_var_names(&mut symbols, &inner, text, lang)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_python(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                push_named(&mut symbols, &child, text, "function", Lang::Python)
            }
            "class_definition" => push_named(&mut symbols, &child, text, "class", Lang::Python),
            "import_statement" => {
                // `import x[, y]` — names live in dotted_name children.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "dotted_name" {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                    }
                }
            }
            "import_from_statement" => {
                // `from x.y import z` — module under field "module_name".
                if let Some(m) = child.child_by_field_name("module_name") {
                    if let Ok(s) = m.utf8_text(text.as_bytes()) {
                        imports.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_rust(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_item" => push_named(&mut symbols, &child, text, "function", Lang::Rust),
            "mod_item" => push_named(&mut symbols, &child, text, "module", Lang::Rust),
            "struct_item" => push_named(&mut symbols, &child, text, "class", Lang::Rust),
            "enum_item" => push_named(&mut symbols, &child, text, "class", Lang::Rust),
            "trait_item" => push_named(&mut symbols, &child, text, "interface", Lang::Rust),
            "use_declaration" => {
                // Take the whole `use ...;` text minus the keyword/semicolon.
                if let Ok(s) = child.utf8_text(text.as_bytes()) {
                    let t = s
                        .trim_start_matches("use")
                        .trim_end_matches(';')
                        .trim()
                        .to_string();
                    if !t.is_empty() {
                        imports.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_go(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                push_named(&mut symbols, &child, text, "function", Lang::Go)
            }
            "method_declaration" => push_named(&mut symbols, &child, text, "method", Lang::Go),
            "type_declaration" => {
                // type_spec children hold the actual names
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "type_spec" {
                        push_named(&mut symbols, &c, text, "type", Lang::Go);
                    }
                }
            }
            "import_declaration" => {
                // import "path" or import ( "a" "b" )
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    match c.kind() {
                        "import_spec" => {
                            let mut c3 = c.walk();
                            for cc in c.children(&mut c3) {
                                if cc.kind() == "interpreted_string_literal" {
                                    if let Ok(s) = cc.utf8_text(text.as_bytes()) {
                                        imports.push(s.trim_matches('"').to_string());
                                    }
                                }
                            }
                        }
                        "interpreted_string_literal" => {
                            if let Ok(s) = c.utf8_text(text.as_bytes()) {
                                imports.push(s.trim_matches('"').to_string());
                            }
                        }
                        "import_spec_list" => {
                            let mut c3 = c.walk();
                            for spec in c.children(&mut c3) {
                                if spec.kind() == "import_spec" {
                                    let mut c4 = spec.walk();
                                    for cc in spec.children(&mut c4) {
                                        if cc.kind() == "interpreted_string_literal" {
                                            if let Ok(s) = cc.utf8_text(text.as_bytes()) {
                                                imports
                                                    .push(s.trim_matches('"').to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_java(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                push_named(&mut symbols, &child, text, "class", Lang::Java)
            }
            "method_declaration" => push_named(&mut symbols, &child, text, "method", Lang::Java),
            "import_declaration" => {
                // import a.b.C; — scoped_identifier child carries path.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_c_cpp(tree: &Tree, text: &str, lang: Lang) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                // Grab the function_declarator → declarator → identifier chain.
                let mut name: Option<Node> = None;
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "function_declarator" {
                        if let Some(decl) = c.child_by_field_name("declarator") {
                            name = Some(decl);
                        }
                    }
                }
                if let Some(n) = name {
                    if let Ok(s) = n.utf8_text(text.as_bytes()) {
                        // declarator may be parenthesized/pointed — trim junk
                        let clean = s.trim_start_matches('*').trim_start_matches('&').trim();
                        let clean = clean
                            .trim_start_matches('(')
                            .trim_end_matches(')')
                            .trim();
                        if !clean.is_empty() {
                            symbols.push(Symbol {
                                name: clean.to_string(),
                                kind: "function",
                                line: child.start_position().row,
                                lang,
                            });
                        }
                    }
                }
            }
            "struct_specifier" | "class_specifier" => {
                push_named(&mut symbols, &child, text, "class", lang)
            }
            "preproc_include" => {
                // #include "x" or <x> — path under field "path"
                if let Some(p) = child.child_by_field_name("path") {
                    if let Ok(s) = p.utf8_text(text.as_bytes()) {
                        imports
                            .push(s.trim_matches(&['"', '<', '>'][..]).to_string());
                    }
                }
            }
            "declaration" => {
                // const type NAME = ...: treat as const if init_declarator present, ignore otherwise.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "init_declarator" {
                        if let Some(d) = c.child_by_field_name("declarator") {
                            if let Ok(s) = d.utf8_text(text.as_bytes()) {
                                let clean = s.trim_start_matches('*').trim_start_matches('&').trim();
                                if !clean.is_empty()
                                    && clean.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                                {
                                    symbols.push(Symbol {
                                        name: clean.to_string(),
                                        kind: "const",
                                        line: c.start_position().row,
                                        lang,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

pub(super) fn extract_csharp(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "struct_declaration" => {
                push_named(&mut symbols, &child, text, "class", Lang::CSharp)
            }
            "method_declaration" => {
                push_named(&mut symbols, &child, text, "method", Lang::CSharp)
            }
            "using_directive" => {
                // using System.Text; — first identifier/qualified_name child.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if matches!(
                        c.kind(),
                        "identifier" | "qualified_name" | "name_equals" | "namespace_or_type"
                    ) && c.kind() != "name_equals"
                    {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

/// Push a `Symbol` for a declaration that carries its name under the `name`
/// field (covers functions, classes, interfaces, type aliases).
pub(super) fn push_named(symbols: &mut Vec<Symbol>, node: &Node, text: &str, kind: &'static str, lang: Lang) {
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
            symbols.push(Symbol {
                name: name.to_string(),
                kind,
                line: node.start_position().row,
                lang,
            });
        }
    }
}

/// `const`/`let` declarations can bind several names (`const A = 1, B = 2`);
/// walk each `variable_declarator` and push each as `const`.
pub(super) fn collect_var_names(symbols: &mut Vec<Symbol>, node: &Node, text: &str, lang: Lang) {
    let mut cursor = node.walk();
    for decl in node.children(&mut cursor) {
        if decl.kind() == "variable_declarator" {
            if let Some(name_node) = decl.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: "const",
                        line: decl.start_position().row,
                        lang,
                    });
                }
            }
        }
    }
}

/// Pull the module specifier string out of an `import_statement` and push it
/// (quotes stripped) onto `imports`.
pub(super) fn collect_import(node: &Node, text: &str, imports: &mut Vec<String>) {
    // import_statement children: "import", import_clause, "from", string, ";"
    // Find the string child.
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.kind() == "string" {
            if let Ok(s) = c.utf8_text(text.as_bytes()) {
                // strip quotes
                let t = s.trim_start_matches(&['"', '\''][..]);
                let t = t.trim_end_matches(&['"', '\''][..]);
                imports.push(t.to_string());
            }
        }
    }
}

