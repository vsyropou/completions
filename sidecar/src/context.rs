use crate::index::{Index, Symbol};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub const REPO_CTX_CAP: usize = 2000;

pub struct Enclosing {
    pub function: Option<String>,
    pub class: Option<String>,
}

/// Walk to the cursor byte offset and find innermost function and class ancestors.
pub fn enclosing_at(tree: &Tree, text: &str, byte_offset: usize) -> Enclosing {
    let root = tree.root_node();
    // A cursor at "end of node" is at the boundary; query one byte earlier so we land
    // inside the last actual token (safer for incomplete files where ERROR truncation
    // refuses the exact end byte).
    let probe = byte_offset.saturating_sub(1).min(text.len());
    let mut node: Option<Node> = root.descendant_for_byte_range(probe, probe).or(Some(root));
    let mut function = None;
    let mut class = None;
    while let Some(n) = node {
        match n.kind() {
            "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "method_definition"
            | "generator_function_declaration" => {
                if function.is_none() {
                    function = name_of(&n, text);
                }
            }
            "class_declaration" => {
                if class.is_none() {
                    class = name_of(&n, text);
                }
            }
            _ => {}
        }
        node = n.parent();
    }
    Enclosing { function, class }
}

fn name_of(node: &Node, text: &str) -> Option<String> {
    // Methods store the name under "name" field too (property_identifier).
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(text.as_bytes()).ok())
        .map(|s| s.to_string())
}

/// First N import statements of the document (raw source text lines).
pub fn imports_of(tree: &Tree, text: &str, cap: usize) -> Vec<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut out = Vec::new();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_statement" {
            if out.len() >= cap {
                break;
            }
            if let Ok(t) = child.utf8_text(text.as_bytes()) {
                out.push(t.trim_end_matches(['\n', ';']).to_string() + ";");
            }
        }
    }
    out
}

/// Import specifiers (the "./y" parts) — for symbol lookup.
pub fn import_specifiers(tree: &Tree, text: &str, cap: usize) -> Vec<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut out = Vec::new();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_statement" {
            if out.len() >= cap {
                break;
            }
            let mut cur = child.walk();
            for c in child.children(&mut cur) {
                if c.kind() == "string" {
                    if let Ok(s) = c.utf8_text(text.as_bytes()) {
                        let t = s.trim_matches(&['"', '\''][..]).to_string();
                        out.push(t);
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Resolve an import specifier ("./y", "../z") against the current file path.
/// Returns repo-relative "src/y.ts" (or ".tsx" if that variant exists).
pub async fn resolve_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    if !spec.starts_with('.') {
        return None; // package imports not indexed
    }
    let current_dir = Path::new(current_file).parent().unwrap_or(Path::new(""));
    let mut base = current_dir.join(spec);
    // Try ./y.ts, ./y.tsx, ./y/index.ts, ./y/index.tsx
    for cand in [
        format!("{}.ts", base.display()),
        format!("{}.tsx", base.display()),
        format!("{}/index.ts", base.display()),
        format!("{}/index.tsx", base.display()),
    ] {
        let p = normalize(&PathBuf::from(&cand));
        if !index.symbols_for(&p).await.is_empty() {
            return Some(p);
        }
    }
    // Also allow the spec itself when it already ends with .ts/.tsx
    if spec.ends_with(".ts") || spec.ends_with(".tsx") {
        base = current_dir.join(spec);
        let p = normalize(&base);
        if !index.symbols_for(&p).await.is_empty() {
            return Some(p);
        }
    }
    None
}

fn normalize(p: &Path) -> PathBuf {
    // Resolve "a/./b" and "a/../b" lexically (Path::normalize equivalent via components).
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            RootDir => out.push("/"),
            Normal(part) => out.push(part),
            Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

/// Build the repository_context string for the HTTP payload.
pub async fn build_repository_context(
    index: &Index,
    current_file: &str,
    tree: &Tree,
    text: &str,
    byte_offset: usize,
) -> String {
    let enclosing = enclosing_at(tree, text, byte_offset);
    let current_path = PathBuf::from(current_file);

    let mut lines: Vec<String> = Vec::new();

    // (a) same-file symbols, minus enclosing function/class name
    let in_file = index.symbols_for(&current_path).await;
    for s in in_file.iter() {
        if enclosing.function.as_deref() == Some(s.name.as_str())
            || enclosing.class.as_deref() == Some(s.name.as_str())
        {
            continue;
        }
        lines.push(fmt_symbol(s, current_file));
    }

    // (b) symbols from files that this file imports
    let specs = import_specifiers(tree, text, 8);
    for spec in specs.iter() {
        if let Some(resolved) = resolve_import(index, current_file, spec).await {
            let symbols = index.symbols_for(&resolved).await;
            let rel_str = resolved.to_string_lossy().replace('\\', "/");
            for s in symbols.iter() {
                lines.push(fmt_symbol(s, &rel_str));
            }
        }
    }

    // Dedupe (same name+file may appear via multiple imports), preserve order.
    let mut seen = std::collections::HashSet::new();
    lines.retain(|l| seen.insert(l.clone()));

    // Cap at REPO_CTX_CAP bytes on line boundary.
    let mut out = String::new();
    for line in lines {
        let next_len = out.len() + line.len() + 1;
        if next_len > REPO_CTX_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn fmt_symbol(s: &Symbol, file: &str) -> String {
    // line here is 0-based row; report 1-based
    format!("{} {} ({}:{})", s.kind, s.name, file, s.line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{parse, Lang};

    fn tree_for(src: &str) -> Tree {
        parse(Lang::TypeScript, src).unwrap()
    }

    #[test]
    fn enclosing_function_incomplete_file_missing_close_brace() {
        // Mid-typing state: function not closed. Tree-sitter produces a function_expression
        // (not declaration) wrapped in an expression_statement — our list handles both.
        let src = "function a() { const x = 1";
        let t = tree_for(src);
        let offset = src.len();
        let enc = enclosing_at(&t, src, offset);
        assert_eq!(enc.function.as_deref(), Some("a"));
        assert_eq!(enc.class, None);
    }

    #[test]
    fn enclosing_class_for_method_inner_cursor() {
        let src = "class Foo { bar() { const x = 1; } }";
        let t = tree_for(src);
        // Cursor inside `const x = 1;`
        let offset = src.find("const x").unwrap() + 2;
        let enc = enclosing_at(&t, src, offset);
        assert_eq!(enc.class.as_deref(), Some("Foo"));
        assert_eq!(enc.function.as_deref(), Some("bar"));
    }

    #[test]
    fn import_lines_extracted() {
        let src = "import { foo } from \"./y\";\nimport * as z from \"../z\";\nconst a = 1;\n";
        let t = tree_for(src);
        let imports = imports_of(&t, src, 8);
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0], "import { foo } from \"./y\";");
        let specs = import_specifiers(&t, src, 8);
        assert_eq!(specs, vec!["./y", "../z"]);
    }

    #[test]
    fn nested_function_inner_cursor() {
        let src = "function outer() {\n  function inner() {\n    😀const a = 1;\n  }\n}\n";
        let t = tree_for(src);
        // Cursor inside `const a = 1` line, past the emoji
        let line_start = src.find("😀").unwrap();
        let offset = line_start + 8; // past emoji (4 bytes) + "cons"
        let enc = enclosing_at(&t, src, offset);
        assert_eq!(enc.function.as_deref(), Some("inner"));
    }

    #[tokio::test]
    async fn repository_context_uses_same_file_and_imported_symbols() {
        // Build index from the fixture workspace.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let idx = crate::index::Index::default();
        crate::index::build_index(root, idx.clone()).await;

        // In-memory "open document" — main.ts with a new function being typed.
        let main_src = "import { Calculator } from \"./calc\";\n\nfunction scratch() {\n  \n}\n";
        let t = tree_for(main_src);
        let offset = main_src.find("\n  \n").unwrap() + 3; // cursor inside function body

        let ctx = build_repository_context(&idx, "src/main.ts", &t, main_src, offset).await;
        println!("CONTEXT:\n{ctx}");

        // Contains symbols from the imported file (calc.ts) — resolved via ./calc → src/calc.ts.
        assert!(
            ctx.contains("class Calculator (src/calc.ts:3)"),
            "ctx={}",
            ctx
        );
        assert!(
            ctx.contains("function makePoint (src/calc.ts:9)"),
            "ctx={}",
            ctx
        );
        // Excludes the enclosing function itself (scratch isn't yet in the index for main.ts).
        assert!(!ctx.contains("scratch"));
        // Cap respected.
        assert!(ctx.len() <= REPO_CTX_CAP);
    }

    #[tokio::test]
    async fn repository_context_same_file_symbols_excluded_enclosing() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let idx = crate::index::Index::default();
        crate::index::build_index(root, idx.clone()).await;

        // Same file: calc.ts cursor inside Calculator.add method.
        let src = "import { helperAdd, Point } from \"./utils\";\n\nexport class Calculator {\n  add(a: number, b: number) {\n    return helperAdd(a, b);\n  }\n}\n\nexport function makePoint(x: number, y: number): Point {\n  return { x, y };\n}\n";
        let t = tree_for(src);
        let cursor = src.find("helperAdd(a, b)").unwrap();
        let ctx = build_repository_context(&idx, "src/calc.ts", &t, src, cursor).await;
        println!("CTX2:\n{ctx}");

        // Same-file: makePoint should appear; the enclosing class Calculator should not.
        assert!(
            ctx.contains("function makePoint (src/calc.ts:9)"),
            "ctx={}",
            ctx
        );
        assert!(!ctx.contains("class Calculator"), "ctx={}", ctx);
        // Imported utils.ts symbols also listed.
        assert!(
            ctx.contains("function helperAdd (src/utils.ts:1)"),
            "ctx={}",
            ctx
        );
        assert!(
            ctx.contains("interface Point (src/utils.ts:7)"),
            "ctx={}",
            ctx
        );
    }
}
