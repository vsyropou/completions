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
    pub text: String,
    pub version: i64,
    pub language: String,
}

/// One didChange applied to a document. Full-replacement protocol, so
/// `offset`/`deleted_len` are a coarse diff over the two snapshots, and
/// `inserted` is a preview of what was typed.
#[derive(Debug, Clone)]
pub struct EditEvent {
    pub version: i64,
    pub offset: usize,
    pub deleted_len: usize,
    pub inserted_len: usize,
    pub preview: String,
}

const EDIT_HISTORY_LEN: usize = 5;

/// Rejection reasons for `Documents::did_change`. Surfaced in the completion
/// flow via structured logging in the caller; the extension treats `didChange`
/// as a notification so there is no JSON-RPC channel to report these on.
#[derive(Debug)]
pub enum DidChangeError {
    /// Required field (`uri` / `text` / `version`) missing or wrongly-typed.
    MissingField,
    /// Incoming version was not `current + 1` for a document we track.
    Stale { uri: String, requested: i64, current: i64 },
    /// `didChange` arrived for a document the extension never opened.
    Unopened { uri: String },
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
    edits: Arc<RwLock<HashMap<String, Vec<EditEvent>>>>,
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
        drop(m);
        // Fresh open: drop any stale history for a re-opened URI.
        self.edits.write().await.remove(&uri);
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
                let old_text = d.text.clone();
                let old_version = d.version;
                m.insert(
                    uri.clone(),
                    Document {
                        text: text.clone(),
                        version,
                        language: lang,
                    },
                );
                drop(m);
                self.record_edit(&uri, &old_text, old_version, &text, version)
                    .await;
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
            let mut m = self.inner.write().await;
            m.remove(uri);
            drop(m);
            self.edits.write().await.remove(uri);
        }
    }

    /// Snapshot the current state of a document. Returns `None` if the URI
    /// hasn't been opened (caller maps this to `-32000`).
    pub async fn get(&self, uri: &str) -> Option<Document> {
        self.inner.read().await.get(uri).cloned()
    }

    /// Recent edits for a document, oldest → newest. Empty if never opened.
    pub async fn recent_edits(&self, uri: &str) -> Vec<EditEvent> {
        self.edits
            .read()
            .await
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    /// Derived from the full-text replacement diff: common-prefix/common-suffix
    /// scan gives a coarse edit location and a typed-text preview. ponytail:
    /// linear scan over the new text; fine at editor-typing scale.
    async fn record_edit(
        &self,
        uri: &str,
        old_text: &str,
        _old_version: i64,
        new_text: &str,
        new_version: i64,
    ) {
        let old_b = old_text.as_bytes();
        let new_b = new_text.as_bytes();
        let mut start = 0usize;
        while start < old_b.len().min(new_b.len()) && old_b[start] == new_b[start] {
            start += 1;
        }
        let mut old_end = old_b.len();
        let mut new_end = new_b.len();
        while old_end > start && new_end > start && old_b[old_end - 1] == new_b[new_end - 1] {
            old_end -= 1;
            new_end -= 1;
        }
        let inserted = &new_text[start.min(new_text.len())..new_end.min(new_text.len())];
        // Snap to char boundaries for the preview (byte-level scan above may split one).
        let preview: String = inserted.chars().take(80).collect();
        let ev = EditEvent {
            version: new_version,
            offset: start,
            deleted_len: old_end.saturating_sub(start),
            inserted_len: inserted.len(),
            preview,
        };
        let mut map = self.edits.write().await;
        let ring = map.entry(uri.to_owned()).or_default();
        if ring.len() >= EDIT_HISTORY_LEN {
            ring.remove(0);
        }
        ring.push(ev);
    }
}
