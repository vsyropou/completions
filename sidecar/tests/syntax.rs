//! Language id / file-extension → grammar mapping, and grammar load smoke test.

use sidecar::syntax::{parse, Lang};
use tree_sitter::Parser;

/// Every language id the extension may send maps to the right variant.
#[test]
fn language_id_round_trip() {
    let cases: &[(&str, Lang)] = &[
        ("typescript", Lang::TypeScript),
        ("typescriptreact", Lang::Tsx),
        ("javascript", Lang::JavaScript),
        ("javascriptreact", Lang::Jsx),
        ("python", Lang::Python),
        ("rust", Lang::Rust),
        ("go", Lang::Go),
        ("java", Lang::Java),
        ("c", Lang::C),
        ("cpp", Lang::Cpp),
        ("csharp", Lang::CSharp),
        ("elixir", Lang::Elixir),
        ("json", Lang::Json),
        ("yaml", Lang::Yaml),
        // NoParser fallbacks
        ("html", Lang::NoParser),
        ("css", Lang::NoParser),
        ("markdown", Lang::NoParser),
        ("makefile", Lang::NoParser),
        ("dockerfile", Lang::NoParser),
        ("unknown", Lang::NoParser),
        ("", Lang::NoParser),
    ];
    for (id, want) in cases {
        assert_eq!(Lang::from_language_id(id), *want, "id={id}");
    }
}

/// `set_language` should succeed for every grammar we ship.
#[test]
fn parser_set_language_succeeds() {
    for lang in [
        Lang::TypeScript,
        Lang::Tsx,
        Lang::JavaScript,
        Lang::Python,
        Lang::Rust,
        Lang::Go,
        Lang::Java,
        Lang::C,
        Lang::Cpp,
        Lang::CSharp,
        Lang::Elixir,
        Lang::Json,
        Lang::Yaml,
    ] {
        let language = lang.language().expect("language for {lang:?}");
        let mut p = Parser::new();
        p.set_language(&language)
            .unwrap_or_else(|e| panic!("set_language {lang:?}: {e}"));
    }
}

#[test]
fn no_parser_returns_none() {
    assert!(parse(Lang::NoParser, "<html></html>").is_none());
}
