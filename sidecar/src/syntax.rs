use tree_sitter::{Language, Parser, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    TypeScript,
    Tsx,
}

impl Lang {
    pub fn from_language_id(id: &str) -> Self {
        match id {
            "tsx" | "typescriptreact" => Lang::Tsx,
            _ => Lang::TypeScript, // unknown → typescript per spec
        }
    }

    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("tsx") => Lang::Tsx,
            _ => Lang::TypeScript,
        }
    }

    pub fn language(self) -> Language {
        match self {
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

pub fn parse(lang: Lang, text: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&lang.language()).ok()?;
    // ponytail: full reparse, incremental edit if profiling shows pain
    p.parse(text, None)
}
