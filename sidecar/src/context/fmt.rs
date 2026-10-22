//! Formatting of `repository_context` lines (`kind name (file:line)`).

use crate::index::Symbol;

/// Format one index symbol as a context line: `kind name (file:line_1based)`.
/// Kind wording depends on `lang` (fn/def/func/method) so FIM prompts read
/// naturally to the model regardless of source language.
pub(super) fn fmt_symbol(s: &Symbol, file: &str) -> String {
    let kind_word = kind_label(s.lang, s.kind);
    // line here is 0-based row; report 1-based
    format!("{} {} ({}:{})", kind_word, s.name, file, s.line + 1)
}

/// Map a generic kind to a per-language label. Single token, no spaces.
pub(super) fn kind_label(lang: crate::syntax::Lang, kind: &str) -> &'static str {
    use crate::syntax::Lang::*;
    match (lang, kind) {
        (Rust, "function") => "fn",
        (Rust, "module") => "mod",
        (Python, "function") => "def",
        (Go, "function") | (Go, "method") => "func",
        (Java, "method") | (CSharp, "method") => "method",
        (C, "function") | (Cpp, "function") => "fn",
        (_, "function") => "function",
        (_, "method") => "method",
        (_, "class") => "class",
        (_, "interface") => "interface",
        (_, "type") => "type",
        (_, "const") => "const",
        (_, "module") => "module",
        _ => "symbol",
    }
}

