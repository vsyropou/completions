//! The `completion/request` handler: validate input against the doc table,
//! build prefix/suffix/context, POST to Phoenix, return the completion.
//!
//! Two non-obvious pieces live here:
//! - UTF-16 → UTF-8 offset conversion (VS Code speaks UTF-16 code units;
//!   Rust strings are UTF-8 bytes) — see [`utf16_to_utf8_offset`].
//! - Capping prefix/suffix to fit the HTTP body limit without splitting a
//!   multi-byte character — see [`cap_prefix`] / [`cap_suffix`].

use crate::rpc::JsonRpcError;
use serde::Deserialize;
use serde_json::json;

pub const PREFIX_CAP: usize = 6000;
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
    pub document_uri: String,
    pub document_version: i64,
    pub position: Position,
}

/// Zero-based `{line, character}` in UTF-16 code units (protocol §4).
#[derive(Deserialize)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

/// Convert a zero-based `{line, character}` UTF-16 position into a byte offset
/// in `text`. Errors map to JSON-RPC `-32602 invalid params`. Mid-surrogate-pair
/// positions resolve to the boundary *before* the pair (protocol §4).
pub fn utf16_to_utf8_offset(
    text: &str,
    target_line: usize,
    target_char: usize,
) -> Result<usize, JsonRpcError> {
    let mut line_start = 0usize;
    for _ in 0..target_line {
        match text[line_start..].find('\n') {
            Some(rel) => line_start += rel + 1,
            None => {
                return Err(JsonRpcError::invalid_params(format!(
                    "line {target_line} out of range"
                )))
            }
        }
    }
    let line_end = text[line_start..]
        .find('\n')
        .map(|r| line_start + r)
        .unwrap_or(text.len());
    let line_str = &text[line_start..line_end];

    let mut units = 0usize;
    let mut byte_in_line = 0usize;
    for ch in line_str.chars() {
        if units == target_char {
            return Ok(line_start + byte_in_line);
        }
        let u16_len = ch.len_utf16();
        // Mid-surrogate-pair: cursor sits between high and low halves.
        // Frozen protocol §4: boundary BEFORE the pair.
        if u16_len == 2 && target_char > units && target_char < units + u16_len {
            return Ok(line_start + byte_in_line);
        }
        units += u16_len;
        byte_in_line += ch.len_utf8();
    }
    if units == target_char {
        return Ok(line_start + byte_in_line);
    }
    Err(JsonRpcError::invalid_params(format!(
        "character {target_char} out of range (line has {units} UTF-16 units)"
    )))
}

/// Take the first `max` bytes of `s`, backing off to a UTF-8 char boundary so
/// we never split a multi-byte character.
pub fn cap_suffix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Take the last `max` bytes of `s`, walking forward to a UTF-8 char boundary
/// so we never split a multi-byte character.
pub fn cap_prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
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
    let params_value = req
        .get("params")
        .cloned()
        .ok_or_else(|| JsonRpcError::invalid_params("params required"))?;
    let p: CompletionParams = serde_json::from_value(params_value)
        .map_err(|e| JsonRpcError::invalid_params(e.to_string()))?;

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

    // Debounce: claim the slot, wait, bail if someone else claimed it while we slept.
    let gen = debounce.bump(&p.document_uri).await;
    tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
    if !debounce.is_current(&p.document_uri, gen).await {
        tracing::debug!(uri = %p.document_uri, "debounced (superseded)");
        return Err(JsonRpcError::cancelled());
    }
    // Typing can move the doc during our sleep; refuse if the version we
    // validated has gone stale. Same code the client would already send.
    let doc = match documents.get(&p.document_uri).await {
        Some(d) if d.version == p.document_version => d,
        _ => return Err(JsonRpcError::cancelled()),
    };

    let offset = utf16_to_utf8_offset(&doc.text, p.position.line, p.position.character)?;
    // Trim prompt to a syntax-aware excerpt, then apply the byte caps as a hard
    // ceiling. The excerpt function already keeps us well under both caps.
    let lang = crate::syntax::Lang::from_language_id(&doc.language);
    let tree_opt = crate::syntax::parse(lang, &doc.text);
    let excerpt = crate::context::compute_cursor_excerpt(
        &doc.text,
        tree_opt.as_ref(),
        offset,
        // PREFIX_CAP+SUFFIX_CAP / 3 tokens ≈ the existing total budget.
        (PREFIX_CAP + SUFFIX_CAP) / 3,
    );
    let prefix = cap_prefix(&doc.text[excerpt.start_byte..offset], PREFIX_CAP).to_owned();
    let suffix = cap_suffix(&doc.text[offset..excerpt.end_byte], SUFFIX_CAP).to_owned();

    let workspace_root = cfg.inner.lock().await.workspace_root.clone();
    let request_id = uuid::Uuid::new_v4().to_string();
    let language = doc.language.clone();
    let file_path = crate::repo_relative_str(&workspace_root, &p.document_uri);

    // Identifiers near cursor feed BM25 ranking for repo excerpts.
    let query_terms = crate::context::identifiers_near_cursor(&doc.text, offset, 3);

    // Phase 2: build repository_context from tree-sitter + in-memory index.
    let repository_context = match tree_opt.as_ref() {
        Some(tree) => {
            crate::context::build_repository_context_with_query(
                &index,
                &file_path,
                tree,
                &doc.text,
                offset,
                &query_terms,
            )
            .await
        }
        None => String::new(),
    };

    let http_start = std::time::Instant::now();
    let (prompt, completion, finish_reason, model) = crate::inference::complete(
        &cfg,
        &doc.language,
        &file_path,
        &prefix,
        &suffix,
        &repository_context,
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
