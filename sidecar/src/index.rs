//! Workspace symbol index: path → list of top-level symbols and import
//! specifiers for every `.ts` / `.tsx` file. Built once in the background on
//! `initialize`, refreshed per-file on every `didOpen`/`didChange`.
//!
//! `context.rs` consults this to assemble the `repository_context` field of
//! the Phoenix HTTP payload — see [`build_index`] and [`extract`]. If you're
//! tuning *which* symbols end up ranked/picked, that's in `context.rs`'s
//! selection logic, not here.

use crate::syntax::{parse, Lang};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tree_sitter::{Node, Tree};

/// A single top-level declaration. Kind strings match what we report in
/// `repository_context` lines, but wording depends on `lang`.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str, // "function" | "class" | "interface" | "type" | "const" | "method" | "module"
    pub line: usize,        // 0-based
    pub lang: Lang,         // language that produced this symbol
}

/// Two maps keyed by workspace-relative path. Cheap to clone; clones share
/// the underlying storage.
#[derive(Default, Clone)]
pub struct Index {
    /// file path (relative to workspace_root, forward slashes) → symbols
    inner: Arc<RwLock<HashMap<PathBuf, Vec<Symbol>>>>,
    /// file path → import specifiers (as written, e.g. "./y")
    imports: Arc<RwLock<HashMap<PathBuf, Vec<String>>>>,
    /// file path → full source text (for BM25 ranking).
    texts: Arc<RwLock<HashMap<PathBuf, String>>>,
    /// Corpus stats for BM25: file path → (doc_len_tokens). Computed alongside `texts`.
    doc_lens: Arc<RwLock<HashMap<PathBuf, usize>>>,
}

impl Index {
    /// Replace the entry for one file (called on `didOpen` / `didChange`).
    pub async fn set_file(&self, path: PathBuf, symbols: Vec<Symbol>, imports: Vec<String>) {
        self.inner.write().await.insert(path.clone(), symbols);
        self.imports.write().await.insert(path, imports);
    }

    /// Replace the source text for one file (called alongside `set_file`).
    pub async fn set_text(&self, path: PathBuf, text: String) {
        let len = tokenize(&text).len();
        self.doc_lens.write().await.insert(path.clone(), len);
        self.texts.write().await.insert(path, text);
    }

    /// Snapshot of source text for one path. Empty if never seen.
    pub async fn text_for(&self, path: &Path) -> Option<String> {
        self.texts.read().await.get(path).cloned()
    }

    /// Snapshot of symbols for one path. Empty if never seen.
    pub async fn symbols_for(&self, path: &Path) -> Vec<Symbol> {
        self.inner
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// Snapshot of import specifiers (as written) for one path. Empty if never seen.
    pub async fn imports_of(&self, path: &Path) -> Vec<String> {
        self.imports
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// All indexed paths (workspace-relative). Used by `context.rs` for ranking
    /// and by `initialize` for logging.
    pub async fn all_paths(&self) -> Vec<PathBuf> {
        self.inner.read().await.keys().cloned().collect()
    }

    /// Rank all indexed files by BM25 against `query_terms`. Returns
    /// `(path, score)` pairs, descending. Skips files that score zero.
    pub async fn bm25_rank(&self, query_terms: &[&str]) -> Vec<(PathBuf, f64)> {
        if query_terms.is_empty() {
            return Vec::new();
        }
        let texts = self.texts.read().await;
        let lens = self.doc_lens.read().await;
        let n_docs = texts.len();
        if n_docs == 0 {
            return Vec::new();
        }
        let avg_len = lens.values().sum::<usize>() as f64 / n_docs as f64;
        if avg_len == 0.0 {
            return Vec::new();
        }
        // Document frequency per term.
        let mut token_cache: std::collections::HashMap<PathBuf, Vec<String>> =
            std::collections::HashMap::new();
        for (p, t) in texts.iter() {
            token_cache.insert(p.clone(), tokenize(t));
        }
        let mut df: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for toks in token_cache.values() {
            let mut seen = std::collections::HashSet::new();
            for q in query_terms {
                if toks.iter().any(|t| t == q) && seen.insert(*q) {
                    *df.entry(*q).or_insert(0) += 1;
                }
            }
        }
        let mut out: Vec<(PathBuf, f64)> = Vec::new();
        for (p, toks) in token_cache.iter() {
            let len = lens.get(p).copied().unwrap_or(1);
            let s = bm25_score(query_terms, toks, len as f64, avg_len, n_docs, &df);
            if s > 0.0 {
                out.push((p.clone(), s));
            }
        }
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// Split `text` into lowercase identifier-ish tokens. Splits snake_case,
/// SCREAMING_CASE, and camelCase so "getUserName" matches "get", "user", "name"
/// and "USER_ID" matches "user", "id".
pub fn tokenize(text: &str) -> Vec<String> {
    fn commit(cur: &mut String, out: &mut Vec<String>) {
        if cur.is_empty() {
            return;
        }
        // snake_case is split at commit time so "FOO_BAR" gives "foo","bar".
        for piece in cur.split('_') {
            if !piece.is_empty() {
                out.push(piece.to_string());
            }
        }
        cur.clear();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            let camel_break = ch.is_uppercase() && prev_lower_or_digit;
            if camel_break {
                commit(&mut cur, &mut out);
            }
            cur.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        } else {
            commit(&mut cur, &mut out);
            prev_lower_or_digit = false;
        }
    }
    commit(&mut cur, &mut out);
    out
}

/// Standard BM25 (k1=1.5, b=0.75). `doc_tokens` is the pre-tokenized document.
/// ponytail: O(terms × tokens) scan; fine while ranked docs are hundreds of lines.
pub fn bm25_score(
    query_terms: &[&str],
    doc_tokens: &[String],
    doc_len: f64,
    avg_len: f64,
    n_docs: usize,
    df: &std::collections::HashMap<&str, usize>,
) -> f64 {
    const K1: f64 = 1.5;
    const B: f64 = 0.75;
    if doc_tokens.is_empty() {
        return 0.0;
    }
    // Term freq per query term.
    let mut tf: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for t in doc_tokens {
        for q in query_terms {
            if t == q {
                *tf.entry(*q).or_insert(0) += 1;
            }
        }
    }
    let mut score = 0.0;
    for (q, f) in tf.iter() {
        let d = df.get(*q).copied().unwrap_or(0);
        if d == 0 {
            continue;
        }
        // Robertson IDF, clamped at zero for very common terms.
        let idf = ((n_docs as f64 - d as f64 + 0.5) / (d as f64 + 0.5) + 1.0).ln();
        let tf_norm =
            (*f as f64) * (K1 + 1.0) / (*f as f64 + K1 * (1.0 - B + B * doc_len / avg_len));
        score += idf * tf_norm;
    }
    score
}

/// Extract top-level symbols and import specifiers from a parse tree.
/// `lang` picks the grammar shapes we look for; `NoParser`/`Json`/`Yaml`
/// produce empty lists (callers still index source text for BM25).
pub fn extract(lang: Lang, tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    match lang {
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Jsx => {
            extract_typescript(tree, text, lang)
        }
        Lang::Python => extract_python(tree, text),
        Lang::Rust => extract_rust(tree, text),
        Lang::Go => extract_go(tree, text),
        Lang::Java => extract_java(tree, text),
        Lang::C | Lang::Cpp => extract_c_cpp(tree, text, lang),
        Lang::CSharp => extract_csharp(tree, text),
        // ponytail: elixir AST nests def/defmodule under `call` targets — v1 keeps
        // elixir content-only (BM25 still sees it), revisit if prompts demand.
        Lang::Elixir | Lang::Json | Lang::Yaml | Lang::NoParser => (Vec::new(), Vec::new()),
    }
}

fn extract_typescript(tree: &Tree, text: &str, lang: Lang) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" => collect_import(&child, text, &mut imports),
            "function_declaration" => push_named(&mut symbols, &child, text, "function", lang),
            "class_declaration" => push_named(&mut symbols, &child, text, "class", lang),
            "interface_declaration" => push_named(&mut symbols, &child, text, "interface", lang),
            "type_alias_declaration" => push_named(&mut symbols, &child, text, "type", lang),
            "lexical_declaration" | "variable_declaration" => {
                collect_var_names(&mut symbols, &child, text, lang)
            }
            "export_statement" => {
                if let Some(inner) = child.named_child(0) {
                    match inner.kind() {
                        "function_declaration" => {
                            push_named(&mut symbols, &inner, text, "function", lang)
                        }
                        "class_declaration" => {
                            push_named(&mut symbols, &inner, text, "class", lang)
                        }
                        "interface_declaration" => {
                            push_named(&mut symbols, &inner, text, "interface", lang)
                        }
                        "type_alias_declaration" => {
                            push_named(&mut symbols, &inner, text, "type", lang)
                        }
                        "lexical_declaration" | "variable_declaration" => {
                            collect_var_names(&mut symbols, &inner, text, lang)
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

fn extract_python(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                push_named(&mut symbols, &child, text, "function", Lang::Python)
            }
            "class_definition" => push_named(&mut symbols, &child, text, "class", Lang::Python),
            "import_statement" => {
                // `import x[, y]` — names live in dotted_name children.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "dotted_name" {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                    }
                }
            }
            "import_from_statement" => {
                // `from x.y import z` — module under field "module_name".
                if let Some(m) = child.child_by_field_name("module_name") {
                    if let Ok(s) = m.utf8_text(text.as_bytes()) {
                        imports.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

fn extract_rust(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_item" => push_named(&mut symbols, &child, text, "function", Lang::Rust),
            "mod_item" => push_named(&mut symbols, &child, text, "module", Lang::Rust),
            "struct_item" => push_named(&mut symbols, &child, text, "class", Lang::Rust),
            "enum_item" => push_named(&mut symbols, &child, text, "class", Lang::Rust),
            "trait_item" => push_named(&mut symbols, &child, text, "interface", Lang::Rust),
            "use_declaration" => {
                // Take the whole `use ...;` text minus the keyword/semicolon.
                if let Ok(s) = child.utf8_text(text.as_bytes()) {
                    let t = s
                        .trim_start_matches("use")
                        .trim_end_matches(';')
                        .trim()
                        .to_string();
                    if !t.is_empty() {
                        imports.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

fn extract_go(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                push_named(&mut symbols, &child, text, "function", Lang::Go)
            }
            "method_declaration" => push_named(&mut symbols, &child, text, "method", Lang::Go),
            "type_declaration" => {
                // type_spec children hold the actual names
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "type_spec" {
                        push_named(&mut symbols, &c, text, "type", Lang::Go);
                    }
                }
            }
            "import_declaration" => {
                // import "path" or import ( "a" "b" )
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    match c.kind() {
                        "import_spec" => {
                            let mut c3 = c.walk();
                            for cc in c.children(&mut c3) {
                                if cc.kind() == "interpreted_string_literal" {
                                    if let Ok(s) = cc.utf8_text(text.as_bytes()) {
                                        imports.push(s.trim_matches('"').to_string());
                                    }
                                }
                            }
                        }
                        "interpreted_string_literal" => {
                            if let Ok(s) = c.utf8_text(text.as_bytes()) {
                                imports.push(s.trim_matches('"').to_string());
                            }
                        }
                        "import_spec_list" => {
                            let mut c3 = c.walk();
                            for spec in c.children(&mut c3) {
                                if spec.kind() == "import_spec" {
                                    let mut c4 = spec.walk();
                                    for cc in spec.children(&mut c4) {
                                        if cc.kind() == "interpreted_string_literal" {
                                            if let Ok(s) = cc.utf8_text(text.as_bytes()) {
                                                imports
                                                    .push(s.trim_matches('"').to_string());
                                            }
                                        }
                                    }
                                }
                            }
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

fn extract_java(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                push_named(&mut symbols, &child, text, "class", Lang::Java)
            }
            "method_declaration" => push_named(&mut symbols, &child, text, "method", Lang::Java),
            "import_declaration" => {
                // import a.b.C; — scoped_identifier child carries path.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

fn extract_c_cpp(tree: &Tree, text: &str, lang: Lang) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                // Grab the function_declarator → declarator → identifier chain.
                let mut name: Option<Node> = None;
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "function_declarator" {
                        if let Some(decl) = c.child_by_field_name("declarator") {
                            name = Some(decl);
                        }
                    }
                }
                if let Some(n) = name {
                    if let Ok(s) = n.utf8_text(text.as_bytes()) {
                        // declarator may be parenthesized/pointed — trim junk
                        let clean = s.trim_start_matches('*').trim_start_matches('&').trim();
                        let clean = clean
                            .trim_start_matches('(')
                            .trim_end_matches(')')
                            .trim();
                        if !clean.is_empty() {
                            symbols.push(Symbol {
                                name: clean.to_string(),
                                kind: "function",
                                line: child.start_position().row,
                                lang,
                            });
                        }
                    }
                }
            }
            "struct_specifier" | "class_specifier" => {
                push_named(&mut symbols, &child, text, "class", lang)
            }
            "preproc_include" => {
                // #include "x" or <x> — path under field "path"
                if let Some(p) = child.child_by_field_name("path") {
                    if let Ok(s) = p.utf8_text(text.as_bytes()) {
                        imports
                            .push(s.trim_matches(&['"', '<', '>'][..]).to_string());
                    }
                }
            }
            "declaration" => {
                // const type NAME = ...: treat as const if init_declarator present, ignore otherwise.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if c.kind() == "init_declarator" {
                        if let Some(d) = c.child_by_field_name("declarator") {
                            if let Ok(s) = d.utf8_text(text.as_bytes()) {
                                let clean = s.trim_start_matches('*').trim_start_matches('&').trim();
                                if !clean.is_empty()
                                    && clean.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                                {
                                    symbols.push(Symbol {
                                        name: clean.to_string(),
                                        kind: "const",
                                        line: c.start_position().row,
                                        lang,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

fn extract_csharp(tree: &Tree, text: &str) -> (Vec<Symbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "interface_declaration" | "struct_declaration" => {
                push_named(&mut symbols, &child, text, "class", Lang::CSharp)
            }
            "method_declaration" => {
                push_named(&mut symbols, &child, text, "method", Lang::CSharp)
            }
            "using_directive" => {
                // using System.Text; — first identifier/qualified_name child.
                let mut c2 = child.walk();
                for c in child.children(&mut c2) {
                    if matches!(
                        c.kind(),
                        "identifier" | "qualified_name" | "name_equals" | "namespace_or_type"
                    ) && c.kind() != "name_equals"
                    {
                        if let Ok(s) = c.utf8_text(text.as_bytes()) {
                            imports.push(s.to_string());
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    (symbols, imports)
}

/// Push a `Symbol` for a declaration that carries its name under the `name`
/// field (covers functions, classes, interfaces, type aliases).
fn push_named(symbols: &mut Vec<Symbol>, node: &Node, text: &str, kind: &'static str, lang: Lang) {
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
            symbols.push(Symbol {
                name: name.to_string(),
                kind,
                line: node.start_position().row,
                lang,
            });
        }
    }
}

/// `const`/`let` declarations can bind several names (`const A = 1, B = 2`);
/// walk each `variable_declarator` and push each as `const`.
fn collect_var_names(symbols: &mut Vec<Symbol>, node: &Node, text: &str, lang: Lang) {
    let mut cursor = node.walk();
    for decl in node.children(&mut cursor) {
        if decl.kind() == "variable_declarator" {
            if let Some(name_node) = decl.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: "const",
                        line: decl.start_position().row,
                        lang,
                    });
                }
            }
        }
    }
}

/// Pull the module specifier string out of an `import_statement` and push it
/// (quotes stripped) onto `imports`.
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

/// Walk the workspace, parse every supported-language file, fill the index.
/// Extensions we index: ts tsx js mjs cjs jsx py rs go java c h cc cpp cxx hpp cs ex exs json yaml yml.
/// NoParser variants (json/yaml/etc.) contribute *content* only (BM25) — no symbols.
/// Async but synchronous CPU work — fine for MVP.
pub async fn build_index(root: PathBuf, index: Index) {
    use ignore::WalkBuilder;
    const EXTENSIONS: &[&str] = &[
        "ts", "tsx", "js", "mjs", "cjs", "jsx", "py", "rs", "go", "java", "c", "h", "cc",
        "cpp", "cxx", "hpp", "hh", "cs", "ex", "exs", "json", "yaml", "yml",
    ];
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
        if !EXTENSIONS.contains(&ext) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let lang = Lang::from_path(path);
        let rel = path
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| path.to_path_buf());
        // NoParser: text only (BM25 still needs the body).
        if lang == Lang::NoParser {
            index.set_text(rel, text).await;
            continue;
        }
        let (symbols, imports) = match parse(lang, &text) {
            Some(tree) => extract(lang, &tree, &text),
            None => (Vec::new(), Vec::new()),
        };
        index.set_file(rel.clone(), symbols, imports).await;
        index.set_text(rel, text).await;
    }
}
