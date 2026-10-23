//! Repository-context assembly: stem-based import matching, and the
//! ranked `repository_context` string that goes out to Phoenix.

use sidecar::context::{
    build_repository_context, enclosing_at, identifiers_near_cursor, REPO_CTX_CAP,
};
use sidecar::index::{build_index, extract, Index};
use sidecar::syntax::{parse, Lang};
use tree_sitter::Tree;

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
    let idx = Index::default();
    build_index(root, idx.clone()).await;

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
    let idx = Index::default();
    build_index(root, idx.clone()).await;

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

#[test]
fn identifiers_near_cursor_extracts_and_caps() {
    let src = "use username;\nfunction getUserName() { return user_id.filter(isActive); }\n";
    let cursor = src.find("user_id").unwrap();
    let ids = identifiers_near_cursor(src, cursor, 2);
    assert!(ids.iter().any(|s| s == "username"), "ids={:?}", ids);
    assert!(ids.iter().any(|s| s == "user"), "ids={:?}", ids);
    assert!(ids.iter().any(|s| s == "id"), "ids={:?}", ids);
    assert!(ids.iter().any(|s| s == "filter"), "ids={:?}", ids);
    assert!(ids.len() <= 12);
}

// ---------- stem-based import matching ----------

#[tokio::test]
async fn symbols_for_stem_matches_across_languages_and_paths() {
    let idx = Index::default();
    let src_ts = "export class Logger {}\n";
    let src_py = "def helper(): pass\n";
    let lang_ts = Lang::from_path(std::path::Path::new("src/utils/logger.ts"));
    let lang_py = Lang::from_path(std::path::Path::new("pkg/utils.py"));
    let (syms_ts, _) = extract(lang_ts, &parse(lang_ts, src_ts).unwrap(), src_ts);
    let (syms_py, _) = extract(lang_py, &parse(lang_py, src_py).unwrap(), src_py);
    idx.set_file(std::path::PathBuf::from("src/utils/logger.ts"), syms_ts, vec![]).await;
    idx.set_file(std::path::PathBuf::from("pkg/utils.py"), syms_py, vec![]).await;

    // "logger" hits only logger.ts.
    let hits = idx.symbols_for_stem("logger").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1.name, "Logger");

    // "utils" hits utils.py.
    let hits = idx.symbols_for_stem("utils").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1.name, "helper");

    // Different case still matches (stem lowercased).
    let hits = idx.symbols_for_stem("LOGGER").await;
    assert_eq!(hits.len(), 1);

    // Misses.
    let hits = idx.symbols_for_stem("nope").await;
    assert!(hits.is_empty());
}

#[tokio::test]
async fn symbols_for_stem_skips_python_init() {
    let idx = Index::default();
    idx.set_file(
        std::path::PathBuf::from("pkg/__init__.py"),
        vec![],
        vec![],
    )
    .await;
    let hits = idx.symbols_for_stem("__init__").await;
    assert!(hits.is_empty());
}
