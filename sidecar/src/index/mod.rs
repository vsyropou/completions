//! Workspace symbol index: path → list of top-level symbols and import
//! specifiers for every tracked source file. Built once in the background on
//! `initialize`, refreshed per-file on every `didOpen`/`didChange`.
//!
//! `context.rs` consults this to assemble the `repository_context` field of
//! the Phoenix HTTP payload — see [`build_index`] and [`extract`]. If you're
//! tuning *which* symbols end up ranked/picked, that's in `context.rs`'s
//! selection logic, not here.

mod bm25;
mod extract;
mod walk;

pub use bm25::{bm25_score, tokenize};
pub use extract::extract;
pub use walk::build_index;

use crate::syntax::Lang;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// A single top-level declaration. Kind strings match what we report in
/// `repository_context` lines, but wording depends on `lang`.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// Identifier as written in source.
    pub name: String,
    /// Coarse category tag: `"function" | "class" | "interface" | "type" | "const" | "method" | "module"`.
    pub kind: &'static str,
    /// 0-based source line where the declaration starts.
    pub line: usize,
    /// Language that produced this symbol.
    pub lang: Lang,
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
    pub(super) texts: Arc<RwLock<HashMap<PathBuf, String>>>,
    /// Corpus stats for BM25: file path → (doc_len_tokens). Computed alongside `texts`.
    pub(super) doc_lens: Arc<RwLock<HashMap<PathBuf, usize>>>,
}

impl Index {
    /// Replace the entry for one file (called on `didOpen` / `didChange`).
    pub async fn set_file(&self, path: PathBuf, symbols: Vec<Symbol>, imports: Vec<String>) {
        self.inner.write().await.insert(path.clone(), symbols);
        self.imports.write().await.insert(path, imports);
    }

    /// Symbols for any indexed file whose stem (file name sans extension,
    /// lowercased) equals `stem`. Cheap O(files) scan — replaces the
    /// per-language import→path resolution logic with a fuzzy match.
    /// `__init__` is not a real stem (Python packages) and is skipped.
    pub async fn symbols_for_stem(&self, stem: &str) -> Vec<(PathBuf, Symbol)> {
        let want = stem.to_lowercase();
        if want.is_empty() || want == "__init__" {
            return Vec::new();
        }
        let inner = self.inner.read().await;
        let mut out = Vec::new();
        for (path, symbols) in inner.iter() {
            let matches = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase() == want)
                .unwrap_or(false);
            if matches {
                for s in symbols {
                    out.push((path.clone(), s.clone()));
                }
            }
        }
        out
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
}
