//! Symbol/import extraction per language, BM25 ranking, tokenizer, and the
//! workspace-walking `build_index`.

use sidecar::index::{bm25_score, build_index, extract, tokenize, Index};
use sidecar::syntax::{parse, Lang};
use std::path::PathBuf;
use tree_sitter::Tree;

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
    let (symbols, imports) = extract(Lang::TypeScript, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    for want in ["bar", "Baz", "Quux", "T", "A", "B", "c"] {
        assert!(names.contains(&want), "missing {} in {:?}", want, names);
    }
    assert_eq!(imports, vec!["./y", "../z"]);
    let baz = symbols.iter().find(|s| s.name == "Baz").unwrap();
    assert_eq!(baz.kind, "class");
    let t = symbols.iter().find(|s| s.name == "T").unwrap();
    assert_eq!(t.kind, "type");
    // Every TS symbol carries Lang::TypeScript.
    assert!(symbols.iter().all(|s| s.lang == Lang::TypeScript));
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

#[test]
fn tokenize_splits_identifiers_and_camelcase() {
    let t = tokenize("function getUserName(USER_ID) { foo_bar.baz2 }");
    // snake_case, SCREAMING_CASE, camelCase all split.
    for want in [
        "function", "get", "user", "name", "id", "foo", "bar", "baz2",
    ] {
        assert!(t.iter().any(|x| x == want), "missing {want} in {:?}", t);
    }
}

#[test]
fn bm25_zero_for_empty() {
    assert_eq!(bm25_score(&["x"], &[], 0.0, 1.0, 1, &Default::default()), 0.0);
}

#[tokio::test]
async fn bm25_rank_prefers_matching_file() {
    let idx = Index::default();
    idx.set_text(
        PathBuf::from("a.ts"),
        "export function helperAdd(a: number, b: number) { return a + b; }".into(),
    )
    .await;
    idx.set_text(
        PathBuf::from("b.ts"),
        "export class Point { x: number; y: number; }".into(),
    )
    .await;
    idx.set_text(
        PathBuf::from("c.ts"),
        "export const PLAIN = 1;".into(),
    )
    .await;
    // "helper"/"add" together discriminatorily match a.ts only.
    let ranked = idx.bm25_rank(&["helper", "add"]).await;
    assert_eq!(ranked.len(), 1, "got {:?}", ranked);
    assert_eq!(ranked[0].0, PathBuf::from("a.ts"));
    // "number" appears in both a.ts and b.ts — both rank, c.ts doesn't.
    let ranked2 = idx.bm25_rank(&["number"]).await;
    let paths: Vec<&str> = ranked2.iter().map(|(p, _)| p.to_str().unwrap()).collect();
    assert!(paths.contains(&"a.ts"), "got {:?}", ranked2);
    assert!(paths.contains(&"b.ts"), "got {:?}", ranked2);
    assert!(!paths.contains(&"c.ts"), "got {:?}", ranked2);
    // Empty query → no results.
    assert!(idx.bm25_rank(&[]).await.is_empty());
    // Term absent everywhere → zero results (we skip zero scores).
    assert!(idx.bm25_rank(&["zzzznomatch"]).await.is_empty());
}

#[tokio::test]
async fn bm25_rank_scores_decrease_down_the_list() {
    let idx = Index::default();
    // High-TF doc.
    idx.set_text(
        PathBuf::from("high.ts"),
        "alpha alpha alpha alpha alpha beta alpha".into(),
    )
    .await;
    idx.set_text(PathBuf::from("low.ts"), "alpha something_else entirely".into())
        .await;
    let ranked = idx.bm25_rank(&["alpha"]).await;
    assert_eq!(ranked.len(), 2);
    assert!(ranked[0].1 > ranked[1].1, "ranking: {:?}", ranked);
}

#[test]
fn extract_python_symbols_and_imports() {
    let src = "import os\nimport sys.path\nfrom collections import defaultdict\n\ndef foo(a, b):\n    return a + b\n\nclass Bar:\n    pass\n";
    let tree = parse(Lang::Python, src).unwrap();
    let (symbols, imports) = extract(Lang::Python, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"foo"), "got {:?}", names);
    assert!(names.contains(&"Bar"), "got {:?}", names);
    assert!(imports.iter().any(|s| s == "os"), "imports={:?}", imports);
    assert!(
        imports.iter().any(|s| s == "collections"),
        "imports={:?}",
        imports
    );
    assert!(symbols.iter().all(|s| s.lang == Lang::Python));
}

#[test]
fn extract_rust_symbols_and_imports() {
    let src = "use std::collections::HashMap;\nuse crate::foo;\n\nfn helper() {}\n\npub mod inner {}\n\nstruct Point { x: i32 }\n";
    let tree = parse(Lang::Rust, src).unwrap();
    let (symbols, imports) = extract(Lang::Rust, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"helper"), "got {:?}", names);
    assert!(names.contains(&"inner"), "got {:?}", names);
    assert!(names.contains(&"Point"), "got {:?}", names);
    assert!(
        imports
            .iter()
            .any(|s| s.contains("std::collections::HashMap")),
        "imports={:?}",
        imports
    );
    assert!(imports.iter().any(|s| s.contains("crate::foo")));
}

#[test]
fn extract_go_symbols_and_imports() {
    let src = "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\nfunc helper() {}\n\ntype T struct{}\n\nfunc (t T) Method() {}\n";
    let tree = parse(Lang::Go, src).unwrap();
    let (symbols, imports) = extract(Lang::Go, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"helper"), "got {:?}", names);
    assert!(names.contains(&"Method"), "got {:?}", names);
    assert!(names.contains(&"T"), "got {:?}", names);
    assert!(imports.iter().any(|s| s == "fmt"), "imports={:?}", imports);
    assert!(imports.iter().any(|s| s == "os"), "imports={:?}", imports);
}

#[test]
fn extract_java_symbols_and_imports() {
    let src = "import java.util.List;\n\npublic class Foo {\n    public int add(int a, int b) { return a + b; }\n}\n";
    let tree = parse(Lang::Java, src).unwrap();
    let (symbols, imports) = extract(Lang::Java, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Foo"), "got {:?}", names);
    assert!(
        imports.iter().any(|s| s == "java.util.List"),
        "imports={:?}",
        imports
    );
}

#[test]
fn extract_cpp_symbols_and_imports() {
    let src = "#include <stdio.h>\n#include \"local.h\"\n\nint add(int a, int b) { return a + b; }\n\nconst int TAU_INT = 2;\n";
    let tree = parse(Lang::Cpp, src).unwrap();
    let (symbols, imports) = extract(Lang::Cpp, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"add"), "got {:?}", names);
    assert!(names.contains(&"TAU_INT"), "got {:?}", names);
    assert!(imports.iter().any(|s| s == "stdio.h"), "{:?}", imports);
    assert!(imports.iter().any(|s| s == "local.h"), "{:?}", imports);
}

#[test]
fn extract_csharp_symbols_and_imports() {
    let src = "using System;\nusing System.Text;\n\npublic class Foo {\n    public int Add(int a, int b) { return a + b; }\n}\n";
    let tree = parse(Lang::CSharp, src).unwrap();
    let (symbols, imports) = extract(Lang::CSharp, &tree, src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Foo"), "got {:?}", names);
    // Add is directly inside class_declaration, not top-level — our walk only
    // looks at root, so we don't expect "Add" here. But "Foo" is enough.
    assert!(imports.iter().any(|s| s == "System"), "imports={:?}", imports);
}

#[test]
fn extract_elixir_is_content_only() {
    // Elixir stays symbol-less in v1 (its `call` AST is too noisy);
    // build_index still feeds text so BM25 sees it.
    let src = "defmodule Foo do\n  def bar, do: 1\nend\n";
    let tree = parse(Lang::Elixir, src).unwrap();
    let (symbols, imports) = extract(Lang::Elixir, &tree, src);
    assert!(symbols.is_empty());
    assert!(imports.is_empty());
}

#[tokio::test]
async fn build_index_indexes_python_and_json_content() {
    // Build a small fixture in a tempdir: one .py, one .json.
    let tmp = std::env::temp_dir().join(format!("idx-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("a.py"),
        "def helper(): pass\n",
    )
    .unwrap();
    std::fs::write(tmp.join("b.json"), "{\"k\": \"MYTHING\"}\n").unwrap();
    let idx = Index::default();
    build_index(tmp.clone(), idx.clone()).await;
    // Python symbols extracted.
    let py = idx.symbols_for(std::path::Path::new("a.py")).await;
    assert!(
        py.iter().any(|s| s.name == "helper" && s.lang == Lang::Python),
        "py={:?}",
        py
    );
    // JSON has no symbols, but text is indexed for BM25.
    let js = idx.symbols_for(std::path::Path::new("b.json")).await;
    assert!(js.is_empty());
    let text = idx.text_for(std::path::Path::new("b.json")).await;
    assert!(text.unwrap().contains("MYTHING"));
    // BM25 finds the json content (tokenizes to lowercase single token "mything").
    let ranked = idx.bm25_rank(&["mything"]).await;
    assert!(ranked.iter().any(|(p, _)| p.ends_with("b.json")), "ranked={:?}", ranked);
    let _ = std::fs::remove_dir_all(&tmp);
}
