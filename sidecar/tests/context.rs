//! Repository-context assembly: excerpt selection, import resolution per language,
//! and the ranked `repository_context` string that goes out to Phoenix.

use sidecar::context::{
    build_repository_context, compute_cursor_excerpt, enclosing_at, identifiers_near_cursor,
    import_specifiers, imports_of, linewise_symmetric_expansion, resolve_c_include,
    resolve_go_import, resolve_import, resolve_import_for, resolve_java_import,
    resolve_python_import, resolve_rust_import, BYTES_PER_TOKEN_GUESS, REPO_CTX_CAP,
};
use sidecar::index::{build_index, extract, Index};
use sidecar::syntax::{parse, Lang};
use std::path::PathBuf;
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
fn linewise_expansion_balances_up_and_down() {
    // 20 lines, cursor on line 10. Budget sized so only ~6 lines fit.
    let mut src = String::new();
    for i in 0..20 {
        src.push_str(&format!("line {i}\n"));
    }
    let cursor = src.match_indices("line 10\n").next().unwrap().0 + 3;
    let (s, e) = linewise_symmetric_expansion(&src, cursor, 6 * BYTES_PER_TOKEN_GUESS);
    // Excerpt contains the cursor and extends in *both* directions (i.e., not one-sided).
    assert!(s < cursor, "start={s} cursor={cursor}");
    assert!(e > cursor, "end={e} cursor={cursor}");
    // Budget caps total included lines (each 8-byte line costs 2 tokens;
    // budget is 18 bytes not tokens in this test setup, so 9 lines fit).
    // The important thing is that we're *under* full file (20 lines) and >0.
    let total = src[s..e].matches('\n').count();
    assert!(total <= 10, "total={total}");
    assert!(total >= 2, "total={total}");
    // Rough symmetry: down-ahead not more than 2 lines ahead of up-ahead.
    let pre_lines = src[s..cursor].matches('\n').count();
    let post_lines = src[cursor..e].matches('\n').count();
    assert!(post_lines.saturating_sub(pre_lines) <= 2, "pre={pre_lines} post={post_lines}");
}

#[test]
fn cursor_excerpt_falls_back_when_no_tree() {
    let src = "fn a() {}\nfn b() {}\nfn c() {}\n";
    let cursor = src.find("b()").unwrap();
    let ex = compute_cursor_excerpt(src, None, cursor, 4);
    assert!(ex.start_byte <= cursor);
    assert!(ex.end_byte >= cursor);
    assert_eq!(ex.cursor_offset_in_excerpt, cursor - ex.start_byte);
}

#[test]
fn cursor_excerpt_empty_and_tiny_inputs() {
    let ex = compute_cursor_excerpt("", None, 0, 8192);
    assert_eq!(ex.start_byte, 0);
    assert_eq!(ex.end_byte, 0);
    // Small text fits fully.
    let s = "abc";
    let ex = compute_cursor_excerpt(s, None, 1, 10);
    assert_eq!((ex.start_byte, ex.end_byte), (0, 3));
}

#[test]
fn cursor_excerpt_syntax_larger_than_linewise_for_function_body() {
    // A function with lots of lines inside — syntax expansion should pick
    // the whole function if it fits the budget, not just a few lines.
    let mut src = String::from("function filled() {\n");
    for i in 0..20 {
        src.push_str(&format!("  const v{i} = {i};\n"));
    }
    src.push_str("}\nfunction other() { return 99; }\n");
    let t = tree_for(&src);
    let cursor = src.find("const v10").unwrap();
    // Budget ~200 tokens ~= 600 bytes. That's smaller than the whole file
    // but bigger than any 2-3 line window.
    let ex = compute_cursor_excerpt(&src, Some(&t), cursor, 200);
    let slice = &src[ex.start_byte..ex.end_byte];
    // Should include the function header because the whole `filled` block fits.
    assert!(slice.contains("function filled()"), "slice=\n{slice}");
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

// ---------- cross-language `resolve_import` tests ----------

/// Build an in-memory index with the given (path, source) pairs, parsing
/// each with its own language so symbols land in the right slots.
async fn index_with(files: &[(&str, &str)]) -> Index {
    let idx = Index::default();
    for (path, src) in files {
        let lang = Lang::from_path(std::path::Path::new(path));
        let (symbols, imports) = match parse(lang, src) {
            Some(t) => extract(lang, &t, src),
            None => (Vec::new(), Vec::new()),
        };
        let p = PathBuf::from(path);
        idx.set_file(p.clone(), symbols, imports).await;
        idx.set_text(p, src.to_string()).await;
    }
    idx
}

#[tokio::test]
async fn resolve_python_absolute_and_relative() {
    let idx = index_with(&[
        ("pkg/__init__.py", ""),
        ("pkg/utils.py", "def helper(): pass\n"),
        ("pkg/sub/__init__.py", ""),
        ("pkg/sub/deep.py", "def deep(): pass\n"),
        ("main.py", "from pkg.utils import helper\nfrom .pkg.sub import deep\n"),
    ])
    .await;
    // `from pkg.utils import helper` — absolute, walk up from main.py's dir.
    let got = resolve_python_import(&idx, "main.py", "pkg.utils").await;
    assert_eq!(got, Some(PathBuf::from("pkg/utils.py")));
    // `import pkg.sub.deep`
    let got = resolve_python_import(&idx, "main.py", "pkg.sub.deep").await;
    assert_eq!(got, Some(PathBuf::from("pkg/sub/deep.py")));
    // `from .pkg.sub import deep` — one dot = current dir of main.py.
    let got = resolve_python_import(&idx, "main.py", ".pkg.sub").await;
    assert_eq!(got, Some(PathBuf::from("pkg/sub/__init__.py")));
    // Missing → None.
    let got = resolve_python_import(&idx, "main.py", "nonexistent.thing").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_python_parent_dot() {
    let idx = index_with(&[
        ("top/__init__.py", ""),
        ("top/util.py", "def u(): pass\n"),
        ("top/sub/__init__.py", ""),
        ("top/sub/leaf.py", "from ..util import u\n"),
    ])
    .await;
    // Two dots from top/sub/ → top/.
    let got = resolve_python_import(&idx, "top/sub/leaf.py", "..util").await;
    assert_eq!(got, Some(PathBuf::from("top/util.py")));
}

#[tokio::test]
async fn resolve_rust_crate_and_super() {
    let idx = index_with(&[
        ("src/lib.rs", "pub mod calc;\n"),
        ("src/calc.rs", "pub fn add() {}\n"),
        ("src/calc/inner.rs", "use super::helper;\n"),
        ("src/calc/helper.rs", "pub fn h() {}\n"),
        ("src/use_site.rs", "use crate::calc::add;\n"),
    ])
    .await;
    // crate::calc → src/calc.rs
    let got = resolve_rust_import(&idx, "src/use_site.rs", "crate::calc").await;
    assert_eq!(got, Some(PathBuf::from("src/calc.rs")));
    // crate::calc::inner → src/calc/inner.rs (walks up from src/use_site.rs's dir).
    let got = resolve_rust_import(&idx, "src/use_site.rs", "crate::calc::inner").await;
    assert_eq!(got, Some(PathBuf::from("src/calc/inner.rs")));
    // super::helper from src/calc/inner.rs → src/calc/helper.rs.
    let got = resolve_rust_import(&idx, "src/calc/inner.rs", "super::helper").await;
    assert_eq!(got, Some(PathBuf::from("src/calc/helper.rs")));
    // External crate (no prefix) → None.
    let got = resolve_rust_import(&idx, "src/use_site.rs", "serde").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_go_relative_only() {
    let idx = index_with(&[
        ("main.go", "package main\nimport \"./util\"\n"),
        ("util.go", "package main\nfunc Util() {}\n"),
    ])
    .await;
    let got = resolve_go_import(&idx, "main.go", "./util").await;
    assert_eq!(got, Some(PathBuf::from("util.go")));
    // Module paths not resolvable.
    let got = resolve_go_import(&idx, "main.go", "github.com/x/y").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_java_package_path() {
    let idx = index_with(&[
        ("src/main/java/com/example/Foo.java", "public class Foo {}\n"),
        (
            "src/main/java/com/example/UseFoo.java",
            "import com.example.Foo;\npublic class UseFoo {}\n",
        ),
    ])
    .await;
    let got = resolve_java_import(&idx, "src/main/java/com/example/UseFoo.java", "com.example.Foo").await;
    assert_eq!(got, Some(PathBuf::from("src/main/java/com/example/Foo.java")));
    // Not indexed → None.
    let got = resolve_java_import(&idx, "src/main/java/com/example/UseFoo.java", "java.util.List").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_cpp_quoted_include() {
    let idx = index_with(&[
        ("src/main.cpp", "#include \"util.h\"\n"),
        ("src/util.h", "int add(int,int);\n"),
        ("src/util.cpp", "int add(int a,int b){return a+b;}\n"),
        ("include/shared.h", "#pragma once\n"),
    ])
    .await;
    // Adjacent .h resolution.
    let got = resolve_c_include(&idx, "src/main.cpp", "util.h").await;
    assert_eq!(got, Some(PathBuf::from("src/util.h")));
    // Walking up: `include/shared.h` referenced from src/ — we don't auto-add "include"
    // so this should NOT resolve (ponytail: no -I flag handling). Skip that assertion.
    // System-style include: no path separator — not resolved.
    let got = resolve_c_include(&idx, "src/main.cpp", "stdio").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_csharp_and_no_parser_skip() {
    let idx = index_with(&[("a.cs", "using System.Text;\n")]).await;
    let got = resolve_import_for(&idx, Lang::CSharp, "a.cs", "System.Text").await;
    assert_eq!(got, None);
    let got = resolve_import_for(&idx, Lang::Elixir, "a.ex", "Foo").await;
    assert_eq!(got, None);
    let got = resolve_import_for(&idx, Lang::NoParser, "README.md", "anything").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn resolve_import_dispatches_on_ext() {
    // Dispatching via `resolve_import` picks the right resolver by the importing
    // file's extension.
    let idx = index_with(&[
        ("main.py", "import pkg\n"),
        ("pkg.py", "def helper(): pass\n"),
        ("app/main.go", "package main\n"),
        ("app/util.go", "package main\nfunc U() {}\n"),
    ])
    .await;
    let got = resolve_import(&idx, "main.py", "pkg").await;
    assert_eq!(got, Some(PathBuf::from("pkg.py")));
    let got = resolve_import(&idx, "app/main.go", "./util").await;
    assert_eq!(got, Some(PathBuf::from("app/util.go")));
}
