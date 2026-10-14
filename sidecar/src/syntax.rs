//! Tree-sitter support for TypeScript and TSX. Maps language ids / file
//! extensions to the right grammar and produces a parse tree for source text.

use tree_sitter::{Language, Parser, Tree};

/// Which TS-flavored grammar to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    TypeScript,
    Tsx,
}

impl Lang {
    /// Pick a grammar from an LSP language id (`"typescript"`, `"tsx"`, …).
    /// Unknown ids fall back to TypeScript per spec.
    pub fn from_language_id(id: &str) -> Self {
        match id {
            "tsx" | "typescriptreact" => Lang::Tsx,
            _ => Lang::TypeScript, // unknown → typescript per spec
        }
    }

    /// Pick a grammar from a file extension (`.ts` → TS, `.tsx` → TSX).
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("tsx") => Lang::Tsx,
            _ => Lang::TypeScript,
        }
    }

    /// The tree-sitter `Language` handle for this variant.
    pub fn language(self) -> Language {
        match self {
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

/// Parse `text`. Returns `None` if the grammar fails to load (shouldn't happen
/// in practice) or tree-sitter declines to produce a tree.
pub fn parse(lang: Lang, text: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&lang.language()).ok()?;
    // ponytail: full reparse, incremental edit if profiling shows pain
    p.parse(text, None)
}
