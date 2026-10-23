//! One-shot workspace walk that fills an [`Index`]: respects .gitignore,
//! skips vendored dirs, parses each matching file, stores symbols + text.

use super::{extract, Index};
use crate::syntax::{parse, Lang};
use ignore::WalkBuilder;
use std::path::PathBuf;

/// Walk `root`, parse every source file in `EXTENSIONS` (respecting
/// .gitignore and a hardcoded junk-dir skip-list), and fill the shared
/// index with symbols, imports, and text for BM25 ranking.
pub async fn build_index(root: PathBuf, index: Index) {
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
