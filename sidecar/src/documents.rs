//! In-memory document table. Tracks the text/version/language of every file the
//! extension has told us about. Version is monotonically increasing per URI —
//! stale `didChange` notifications are silently dropped so we never serve an
//! old snapshot to a completion request.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// One open document's current state.
#[derive(Debug, Clone)]
pub struct Document {
    /// Full UTF-8 source text.
    pub text: String,
    /// Last accepted edit version. Monotonic per URI; `didChange` arrives with `+1`.
    pub version: i64,
    /// VS Code language id (e.g. `"python"`, `"typescript"`).
    pub language: String,
}

/// Rejection reasons for `Documents::did_change`.
#[derive(Debug)]
pub enum DidChangeError {
    /// Required field (`uri` / `text` / `version`) missing or wrongly-typed.
    MissingField,
    /// Incoming version was not `current + 1` for a document we track.
    Stale {
        /// Document URI this stale update targeted.
        uri: String,
        /// Version the client sent.
        requested: i64,
        /// Our current version for the same document.
        current: i64,
    },
    /// `didChange` arrived for a document the extension never opened.
    Unopened {
        /// Document URI we don't know about.
        uri: String,
    },
}

impl std::fmt::Display for DidChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField => write!(f, "didChange missing uri/text/version"),
            Self::Stale { uri, requested, current } => write!(
                f,
                "didChange version mismatch: requested {requested}, current {current}, uri {uri}"
            ),
            Self::Unopened { uri } => write!(f, "didChange for unopened doc: {uri}"),
        }
    }
}

impl std::error::Error for DidChangeError {}

/// Thread-safe map of URI → `Document`. Cheap to clone; clones share the
/// same underlying map.
#[derive(Clone, Default)]
pub struct Documents {
    inner: Arc<RwLock<HashMap<String, Document>>>,
}

impl Documents {
    /// `textDocument/didOpen` notification handler.
    pub async fn did_open(&self, params: &serde_json::Value) {
        let Some(uri) = params
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(text) = params
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(version) = params.get("version").and_then(|v| v.as_i64()) else {
            return;
        };
        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("typescript")
            .to_owned();
        let mut m = self.inner.write().await;
        m.insert(
            uri.clone(),
            Document {
                text,
                version,
                language,
            },
        );
    }

    /// `textDocument/didChange` (full-document replacement in v1). The protocol
    /// requires the incoming `version` to be exactly `current + 1`; anything
    /// else is a stale or re-ordered notification and is rejected with an Err —
    /// caller logs and leaves the document untouched.
    pub async fn did_change(&self, params: &serde_json::Value) -> Result<(), DidChangeError> {
        let Some(uri) = params
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return Err(DidChangeError::MissingField);
        };
        let Some(text) = params
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return Err(DidChangeError::MissingField);
        };
        let Some(version) = params.get("version").and_then(|v| v.as_i64()) else {
            return Err(DidChangeError::MissingField);
        };
        let mut m = self.inner.write().await;
        match m.get(&uri) {
            Some(d) if version == d.version + 1 => {
                let lang = d.language.clone();
                m.insert(
                    uri.clone(),
                    Document {
                        text,
                        version,
                        language: lang,
                    },
                );
                Ok(())
            }
            Some(d) => Err(DidChangeError::Stale {
                uri,
                requested: version,
                current: d.version,
            }),
            None => Err(DidChangeError::Unopened { uri }),
        }
    }

    /// `textDocument/didClose` notification handler. Removes the entry.
    pub async fn did_close(&self, params: &serde_json::Value) {
        if let Some(uri) = params.get("uri").and_then(|v| v.as_str()) {
            self.inner.write().await.remove(uri);
        }
    }

    /// Snapshot the current state of a document. Returns `None` if the URI
    /// hasn't been opened (caller maps this to `-32000`).
    pub async fn get(&self, uri: &str) -> Option<Document> {
        self.inner.read().await.get(uri).cloned()
    }
}
