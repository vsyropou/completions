//! Tree-sitter support. Maps language ids / file extensions to the right
//! grammar and produces a parse tree for source text. Languages without a
//! usable crates.io grammar (html/css/markdown/makefile/dockerfile) fall back
//! to [`Lang::NoParser`] — `parse` returns `None` and callers emit an empty
//! repository_context (plain prefix/suffix still flow through).

use tree_sitter::{Language, Parser, Tree};

/// Which grammar to use. `NoParser` covers languages we intentionally
/// don't parse (markup/config formats where tree-sitter adds nothing for
/// the prompt we build).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Python,
    Rust,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Elixir,
    Json,
    Yaml,
    /// Languages we accept but don't parse (html, css, markdown, makefile,
    /// dockerfile, anything unrecognised). `parse` returns `None` and
    /// outline/import extraction produces empty lists.
    NoParser,
}

impl Lang {
    /// Pick a grammar from an LSP language id (VS Code `languageId` values
    /// mapped through the extension, e.g. `"typescript"`, `"python"`).
    /// Unknown / no-grammar ids map to `NoParser`.
    pub fn from_language_id(id: &str) -> Self {
        match id {
            "typescript" => Lang::TypeScript,
            "tsx" | "typescriptreact" => Lang::Tsx,
            "javascript" => Lang::JavaScript,
            "javascriptreact" | "jsx" => Lang::Jsx,
            "python" => Lang::Python,
            "rust" => Lang::Rust,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "c" => Lang::C,
            "cpp" | "c++" => Lang::Cpp,
            "csharp" => Lang::CSharp,
            "elixir" => Lang::Elixir,
            "json" => Lang::Json,
            "yaml" => Lang::Yaml,
            _ => Lang::NoParser,
        }
    }

    /// Pick a grammar from a file extension (`.ts` → TS, `.tsx` → TSX, …).
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("ts") => Lang::TypeScript,
            Some("tsx") => Lang::Tsx,
            Some("js") | Some("mjs") | Some("cjs") => Lang::JavaScript,
            Some("jsx") => Lang::Jsx,
            Some("py") => Lang::Python,
            Some("rs") => Lang::Rust,
            Some("go") => Lang::Go,
            Some("java") => Lang::Java,
            Some("c") | Some("h") => Lang::C,
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => Lang::Cpp,
            Some("cs") => Lang::CSharp,
            Some("ex") | Some("exs") => Lang::Elixir,
            Some("json") => Lang::Json,
            Some("yaml") | Some("yml") => Lang::Yaml,
            _ => Lang::NoParser,
        }
    }

    /// The tree-sitter `Language` handle for this variant, or `None` for
    /// `NoParser` (no grammar wired).
    pub fn language(self) -> Option<Language> {
        Some(match self {
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Jsx => tree_sitter_javascript::LANGUAGE.into(), // jsx is a superset mode; close enough
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::language(),
            Lang::NoParser => return None,
        })
    }
}

/// Parse `text`. Returns `None` for `Lang::NoParser`, if the grammar fails
/// to load, or if tree-sitter declines to produce a tree.
pub fn parse(lang: Lang, text: &str) -> Option<Tree> {
    let language = lang.language()?;
    let mut p = Parser::new();
    p.set_language(&language).ok()?;
    // ponytail: full reparse, incremental edit if profiling shows pain
    p.parse(text, None)
}
