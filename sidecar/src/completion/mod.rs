//! The `completion/request` handler: validate input against the doc table,
//! build prefix/suffix/context, POST to the inference server, return the completion.

mod cap;

pub use cap::{cap_prefix, cap_suffix};

use crate::rpc::JsonRpcError;
use serde::Deserialize;
use serde_json::json;

/// Byte budget for the prompt prefix sent to the model.
pub const PREFIX_CAP: usize = 6000;
/// Byte budget for the prompt suffix sent to the model.
pub const SUFFIX_CAP: usize = 1000;

/// Wait this long before the HTTP fan-out so a burst of typing collapses into
/// one Phoenix call. Latest request per URI wins; superseded ones return -32003.
pub const DEBOUNCE_MS: u64 = 200;

/// Latest-wins per-URI generation counter. Clone-cheap (Arc inside).
#[derive(Default, Clone)]
pub struct Debounce {
    inner: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, u64>>>,
}

impl Debounce {
    /// Bump and return the new generation for `uri`.
    pub async fn bump(&self, uri: &str) -> u64 {
        let mut m = self.inner.lock().await;
        let g = m.entry(uri.to_owned()).or_insert(0);
        *g += 1;
        *g
    }
    /// True if `gen` is still the newest generation for `uri`.
    pub async fn is_current(&self, uri: &str, gen: u64) -> bool {
        let m = self.inner.lock().await;
        m.get(uri).copied() == Some(gen)
    }
}

/// Deserialized `completion/request` params. Field names match the protocol.
#[derive(Deserialize)]
pub struct CompletionParams {
    /// URI of the document being completed.
    pub document_uri: String,
    /// Client-side version the request was issued against. Stale versions are rejected.
    pub document_version: i64,
    /// Cursor position within the document.
    pub position: Position,
}

/// Zero-based `{line, character}` position (wire format per `protocol/README.md`).
#[derive(Deserialize)]
pub struct Position {
    /// 0-based line index into the document text.
    pub line: usize,
    /// UTF-8 byte offset within the line (extension converts from UTF-16 before send).
    pub character: usize,
}

/// Handle a `completion/request` JSON-RPC request end-to-end. Returns the
/// `result` body (or an error to be serialized via `JsonRpcError::into_response`).
///
/// Order of checks is protocol-mandated: unknown document (`-32000`) →
/// version mismatch (`-32001`) → upstream errors (`-32002`). Rapid bursts
/// of requests for one URI coalesce via `debounce`; superseded ones return -32003.
pub async fn handle_completion_request(
    cfg: crate::Config,
    documents: crate::documents::Documents,
    index: crate::index::Index,
    req: serde_json::Value,
    debounce: Debounce,
) -> Result<serde_json::Value, JsonRpcError> {
    let p = parse_params(req)?;
    let doc = documents
        .get(&p.document_uri)
        .await
        .ok_or_else(|| JsonRpcError::unknown_document(&p.document_uri))?;
    if doc.version != p.document_version {
        return Err(JsonRpcError::version_mismatch(
            p.document_version,
            doc.version,
        ));
    }
    let doc = debounce_recheck(&documents, &p, debounce).await?;
    let offset = byte_offset(&doc.text, &p.position)?;

    let repo = Repo::build(&cfg, &index, &doc, &p.document_uri, offset).await;

    let workspace_root = cfg.inner.lock().await.workspace_root.clone();
    static NEXT_REQ_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let request_id = format!(
        "req-{}",
        NEXT_REQ_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let language = doc.language.clone();
    let file_path = crate::repo_relative_str(&workspace_root, &p.document_uri);

    let http_start = std::time::Instant::now();
    let (prompt, completion, finish_reason, model) = crate::inference::complete(
        &cfg,
        &doc.language,
        &file_path,
        &repo.prefix,
        &repo.suffix,
        &repo.context,
        &request_id,
    )
    .await?;
    let http_ms = http_start.elapsed().as_millis() as u64;

    // Editor got its answer; telemetry fans out in the background and is
    // dropped on any failure. Provider mirrors the inference routing.
    let provider = if cfg.inner.lock().await.model_url.is_some() {
        "model"
    } else {
        "mock"
    };
    crate::telemetry::fire(
        cfg.clone(),
        crate::telemetry::Telemetry {
            request_id,
            language,
            file_path,
            prompt,
            response: completion.clone(),
            latency_ms: http_ms,
            provider,
            model,
            finish_reason,
        },
    );

    Ok(json!({
        "text": completion,
        "document_version": doc.version,
    }))
}

/// Deserialize+validate the `params` block. `-32602` on any shape error.
fn parse_params(req: serde_json::Value) -> Result<CompletionParams, JsonRpcError> {
    let params = req
        .get("params")
        .cloned()
        .ok_or_else(|| JsonRpcError::invalid_params("params required"))?;
    serde_json::from_value(params).map_err(|e| JsonRpcError::invalid_params(e.to_string()))
}

/// Bump the debounce counter, sleep `DEBOUNCE_MS`, then refuse the request if
/// (a) a newer request claimed the slot, or (b) the document version moved
/// during the sleep (typing mid-flight). `-32003` in both cases.
async fn debounce_recheck(
    documents: &crate::documents::Documents,
    p: &CompletionParams,
    debounce: Debounce,
) -> Result<crate::documents::Document, JsonRpcError> {
    let gen = debounce.bump(&p.document_uri).await;
    tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
    if !debounce.is_current(&p.document_uri, gen).await {
        tracing::debug!(uri = %p.document_uri, "debounced (superseded)");
        return Err(JsonRpcError::cancelled());
    }
    match documents.get(&p.document_uri).await {
        Some(d) if d.version == p.document_version => Ok(d),
        _ => Err(JsonRpcError::cancelled()),
    }
}

/// Flatten `{line, character}` into a byte offset within `text`.
/// `character` is a UTF-8 byte offset within the target line (extension
/// converts from UTF-16 before send), so byte arithmetic alone is safe.
fn byte_offset(text: &str, pos: &Position) -> Result<usize, JsonRpcError> {
    let line_start = if pos.line == 0 {
        0
    } else {
        match text.match_indices('\n').nth(pos.line - 1) {
            Some((i, _)) => i + 1,
            None => {
                return Err(JsonRpcError::invalid_params(format!(
                    "line {} out of range",
                    pos.line
                )))
            }
        }
    };
    let line_end = text[line_start..]
        .find('\n')
        .map(|r| line_start + r)
        .unwrap_or(text.len());
    if pos.character > line_end - line_start {
        return Err(JsonRpcError::invalid_params(format!(
            "character {} out of range (line has {} bytes)",
            pos.character,
            line_end - line_start
        )));
    }
    Ok(line_start + pos.character)
}

/// Prompt side of a request: capped prefix/suffix + repository_context.
pub struct Repo {
    /// Last `PREFIX_CAP` bytes of text before the cursor.
    pub prefix: String,
    /// First `SUFFIX_CAP` bytes of text after the cursor.
    pub suffix: String,
    /// Symbols+excerpts block for the FIM prompt's `# Related symbols:` section.
    pub context: String,
}

impl Repo {
    /// Assemble the prompt-side bits from the document + index. Byte-capped,
    /// tree-sitter optional — no tree means empty `repository_context`.
    async fn build(
        cfg: &crate::Config,
        index: &crate::index::Index,
        doc: &crate::documents::Document,
        uri: &str,
        offset: usize,
    ) -> Self {
        let prefix = cap_prefix(&doc.text[..offset], PREFIX_CAP).to_owned();
        let suffix = cap_suffix(&doc.text[offset..], SUFFIX_CAP).to_owned();
        let context = crate::context::repository_context(cfg, index, doc, uri, offset).await;
        Self {
            prefix,
            suffix,
            context,
        }
    }
}
