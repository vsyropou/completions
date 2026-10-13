use crate::syntax::{parse, Lang};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tree_sitter::{Node, Tree};

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str, // "function" | "class" | "interface" | "type" | "const"
    pub line: usize,        // 0-based
}

#[derive(Default, Clone)]
pub struct Index {
    /// file path (relative to workspace_root, forward slashes) → symbols
    inner: Arc<RwLock<HashMap<PathBuf, Vec<Symbol>>>>,
    /// file path → import specifiers (as written, e.g. "./y")
    imports: Arc<RwLock<HashMap<PathBuf, Vec<String>>>>,
}

impl Index {
    pub async fn set_file(&self, path: PathBuf, symbols: Vec<Symbol>, imports: Vec<String>) {
        self.inner.write().await.insert(path.clone(), symbols);
        self.imports.write().await.insert(path, imports);
    }

    pub async fn symbols_for(&self, path: &Path) -> Vec<Symbol> {
        self.inner
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn imports_of(&self, path: &Path) -> Vec<String> {
        self.imports
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn all_paths(&self) -> Vec<PathBuf> {
        self.inner.read().await.keys().cloned().collect()
    }
}

/// Extract top-level symbols and import specifiers from a parse tree.
pub fn extract(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                // second child string is module specifier
                collect_import(&child, text, &mut imports);
            }
            "function_declaration" => {
                push_named(&mut symbols, &child, text, "function");
            }
            "class_declaration" => {
                push_named(&mut symbols, &child, text, "class");
            }
            "interface_declaration" => {
                push_named(&mut symbols, &child, text, "interface");
            }
            "type_alias_declaration" => {
                push_named(&mut symbols, &child, text, "type");
            }
            "lexical_declaration" | "variable_declaration" => {
                // const/let x = ... at top level
                collect_var_names(&mut symbols, &child, text);
            }
            "export_statement" => {
                // unwrap exported declaration, e.g. `export function foo() {}` or `export const x = 1`
                if let Some(inner) = child.named_child(0) {
                    match inner.kind() {
                        "function_declaration" => {
                            push_named(&mut symbols, &inner, text, "function")
                        }
                        "class_declaration" => push_named(&mut symbols, &inner, text, "class"),
                        "interface_declaration" => {
                            push_named(&mut symbols, &inner, text, "interface")
                        }
                        "type_alias_declaration" => push_named(&mut symbols, &inner, text, "type"),
                        "lexical_declaration" | "variable_declaration" => {
                            collect_var_names(&mut symbols, &inner, text)
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

fn push_named(symbols: &mut Vec<Symbol>, node: &Node, text: &str, kind: &'static str) {
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
            symbols.push(Symbol {
                name: name.to_string(),
                kind,
                line: node.start_position().row,
            });
        }
    }
}

fn collect_var_names(symbols: &mut Vec<Symbol>, node: &Node, text: &str) {
    let mut cursor = node.walk();
    for decl in node.children(&mut cursor) {
        if decl.kind() == "variable_declarator" {
            if let Some(name_node) = decl.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: "const",
                        line: decl.start_position().row,
                    });
                }
            }
        }
    }
}

fn collect_import(node: &Node, text: &str, imports: &mut Vec<String>) {
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

/// Walk the workspace, parse .ts/.tsx files, fill the index. Async but synchronous CPU work — fine for MVP.
pub async fn build_index(root: PathBuf, index: Index) {
    use ignore::WalkBuilder;
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false);
    let walker = builder.build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            continue;
        }
        // Skip common junk dirs defensively even if gitignore missed.
        let path_str = path.to_string_lossy();
        const SKIP: &[&str] = &["node_modules", ".git", "target", "dist", "build", "deps"];
        if SKIP
            .iter()
            .any(|seg| path_str.contains(&format!("/{seg}/")))
        {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "ts" && ext != "tsx" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lang = Lang::from_path(path);
        let Some(tree) = parse(lang, &text) else {
            continue;
        };
        let (symbols, imports) = extract(&tree, &text);
        let rel = path
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.to_path_buf());
        index.set_file(rel, symbols, imports).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ts(text: &str) -> Tree {
        parse(Lang::TypeScript, text).unwrap()
    }

    #[test]
    fn extract_top_level_symbols_and_imports() {
        let src = r#"
import { foo } from "./y";
import * as z from "../z";
export function bar() {}
class Baz {}
export interface Quux {}
type T = number;
export const A = 1, B = 2;
let c = 3;
"#;
        let tree = parse_ts(src);
        let (symbols, imports) = extract(&tree, src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        for want in ["bar", "Baz", "Quux", "T", "A", "B", "c"] {
            assert!(names.contains(&want), "missing {} in {:?}", want, names);
        }
        assert_eq!(imports, vec!["./y", "../z"]);
        let baz = symbols.iter().find(|s| s.name == "Baz").unwrap();
        assert_eq!(baz.kind, "class");
        let t = symbols.iter().find(|s| s.name == "T").unwrap();
        assert_eq!(t.kind, "type");
    }

    #[tokio::test]
    async fn build_index_walks_fixture() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let idx = Index::default();
        build_index(root, idx.clone()).await;

        let calc = idx.symbols_for(std::path::Path::new("src/calc.ts")).await;
        let names: Vec<&str> = calc.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Calculator"));
        assert!(names.contains(&"makePoint"));

        let utils = idx.symbols_for(std::path::Path::new("src/utils.ts")).await;
        let kinds: Vec<(&str, &str)> = utils.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(kinds.contains(&("helperAdd", "function")));
        assert!(kinds.contains(&("TAU", "const")));
        assert!(kinds.contains(&("Point", "interface")));

        let calc_imports = idx.imports_of(std::path::Path::new("src/calc.ts")).await;
        assert_eq!(calc_imports, vec!["./utils"]);
    }
}
