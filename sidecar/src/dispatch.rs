//! Request handlers: one async fn per JSON-RPC method plus the shared
//! helpers they use. Keeps `lib.rs::run` as a frame-loop that maps method
//! names to these handlers.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::documents::Documents;
use crate::index::{self, Index};
use crate::rpc::{self, write_message, JsonRpcError, RequestId};
use crate::state::{Config, Pending};
use crate::{completion, syntax};

/// Shared stdout writer handle — one Mutex for the whole process.
pub type Writer = Arc<Mutex<tokio::io::Stdout>>;

/// Lock the shared stdout writer and push one framed message. Bytes sent are dropped
/// silently on I/O error — the extension is the only reader, and if it's gone we're
/// shutting down anyway.
async fn write(writer: &Writer, v: serde_json::Value) {
    let mut w = writer.lock().await;
    let _ = write_message(&mut *w, &v).await;
}

/// Shorthand for `write()` of an error response.
async fn write_err(writer: &Writer, id: RequestId, e: JsonRpcError) {
    write(writer, e.into_response(id)).await;
}

/// Handle `initialize`: pull `telemetry_url`, `telemetry_api_key`,
/// `workspace_root` (and the optional `model_*` fields) out
/// of params, stash them in `Config`, and spawn workspace indexing in the
/// background so completions never wait on it.
async fn handle_initialize(
    cfg: &Config,
    val: &serde_json::Value,
    index: &Index,
) -> Result<serde_json::Value, JsonRpcError> {
    let params = val
        .get("params")
        .ok_or_else(|| JsonRpcError::invalid_params("params required"))?;
    let telemetry_url = params
        .get("telemetry_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError::invalid_params("telemetry_url required"))?
        .to_owned();
    let telemetry_api_key = params
        .get("telemetry_api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let workspace_root = params
        .get("workspace_root")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let model_url = params
        .get("model_url")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let model_api_key = params
        .get("model_api_key")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let model = params
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let fim_template = params
        .get("fim_template")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let temperature = params
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|t| t as f32);
    let max_tokens = params
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    {
        let mut g = cfg.inner.lock().await;
        g.telemetry_url = telemetry_url;
        g.telemetry_api_key = telemetry_api_key;
        g.workspace_root = workspace_root.clone();
        g.model_url = model_url;
        g.model_api_key = model_api_key;
        g.model = model;
        g.fim_template = fim_template;
        g.temperature = temperature;
        g.max_tokens = max_tokens;
    }

    // Spawn async workspace indexing — never block completions on it.
    if let Some(root) = workspace_root_to_path(&workspace_root) {
        let idx = index.clone();
        tokio::spawn(async move {
            let n_before = idx.all_paths().await.len();
            index::build_index(root, idx.clone()).await;
            let n_after = idx.all_paths().await.len();
            tracing::info!(files = n_after - n_before, "workspace indexed");
        });
    }

    Ok(serde_json::json!({}))
}

pub async fn on_initialize(cfg: &Config, val: &serde_json::Value, index: &Index, id: RequestId, writer: &Writer) {
    let resp = match handle_initialize(cfg, val, index).await {
        Ok(v) => rpc::success_response(id, v),
        Err(e) => e.into_response(id),
    };
    write(writer, resp).await;
}

pub async fn on_shutdown(pending: &Pending, id: RequestId, writer: &Writer) {
    // Drain in-flight completion tasks so a late `shutdown` doesn't
    // silently eat a request the user is still waiting on. Bounded —
    // give tasks up to 5 s to finish on their own.
    let handles = {
        let mut st = pending.state.lock().await;
        std::mem::take(&mut st.tasks)
    };
    if !handles.is_empty() {
        tracing::info!(n = handles.len(), "shutdown: awaiting in-flight completions");
        let drain = async move {
            for h in handles {
                let _ = h.await;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), drain).await;
    }
    let resp = rpc::success_response(id, serde_json::Value::Null);
    write(writer, resp).await;
    tracing::info!("shutdown");
}

pub async fn on_completion_request(
    cfg: Config,
    docs: Documents,
    idx: Index,
    val: serde_json::Value,
    dbnc: completion::Debounce,
    pnd: Arc<Pending>,
    id: RequestId,
    writer: Writer,
) {
    let num_id = match &id {
        RequestId::Num(n) => Some(*n),
        _ => None,
    };
    let task_id = id.clone();
    let pnd_task = pnd.clone();
    let task = tokio::spawn(async move {
        let result = completion::handle_completion_request(cfg, docs, idx, val, dbnc).await;
        // Atomic with the cancel handler: if the id was already answered (cancel
        // raced us and won), drop the result and clean up. Otherwise claim the
        // response slot so the cancel handler becomes a no-op.
        if let Some(n) = num_id {
            let mut st = pnd_task.state.lock().await;
            if !st.answered.insert(n) {
                st.cancel.remove(&n);
                st.answered.remove(&n);
                return;
            }
            st.cancel.remove(&n);
            drop(st);
        }
        let resp = match result {
            Ok(v) => rpc::success_response(task_id, v),
            Err(e) => e.into_response(task_id),
        };
        let mut w = writer.lock().await;
        let _ = write_message(&mut *w, &resp).await;
    });
    if let Some(n) = num_id {
        pnd.state.lock().await.cancel.insert(n, task.abort_handle());
    }
    pnd.state.lock().await.tasks.push(task);
}

pub async fn on_cancel(pending: &Pending, val: &serde_json::Value, writer: &Writer) {
    if let Some(rid) = val
        .get("params")
        .and_then(|p| p.get("request_id"))
        .and_then(|r| r.as_i64())
    {
        let mut st = pending.state.lock().await;
        let handle = st.cancel.remove(&rid);
        // insert returns false if already answered → no-op race case.
        let should_respond = st.answered.insert(rid);
        drop(st);
        if let Some(h) = handle {
            h.abort();
        }
        if should_respond {
            let err = JsonRpcError::cancelled().into_response(RequestId::Num(rid));
            write(writer, err).await;
        }
    }
}

pub async fn on_did_open(documents: &Documents, index: &Index, config: &Config, val: &serde_json::Value) {
    documents
        .did_open(val.get("params").unwrap_or(&serde_json::Value::Null))
        .await;
    refresh_index_for_doc(index, config, val.get("params")).await;
}

pub async fn on_did_change(documents: &Documents, index: &Index, config: &Config, val: &serde_json::Value) {
    if let Err(e) = documents
        .did_change(val.get("params").unwrap_or(&serde_json::Value::Null))
        .await
    {
        // Leave the document untouched; the completion path will
        // -32001 against the stale server version.
        tracing::warn!(error = %e, "didChange rejected, document not updated");
    } else {
        refresh_index_for_doc(index, config, val.get("params")).await;
    }
}

pub async fn on_did_close(documents: &Documents, val: &serde_json::Value) {
    documents
        .did_close(val.get("params").unwrap_or(&serde_json::Value::Null))
        .await;
}

pub async fn on_unknown(writer: &Writer, id: RequestId, method: &str) {
    write_err(writer, id, JsonRpcError::method_not_found(method)).await;
}

pub async fn on_parse_error(writer: &Writer) {
    write_err(writer, RequestId::Null, JsonRpcError::parse_error()).await;
}

/// Convert a `file:///repo` URI to a filesystem path. `None` if empty.
fn workspace_root_to_path(root: &str) -> Option<std::path::PathBuf> {
    let p = root.strip_prefix("file://").unwrap_or(root);
    if p.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(p))
    }
}

/// Re-extract symbols/imports for a single open document so completions see fresh state.
async fn refresh_index_for_doc(index: &Index, config: &Config, params: Option<&serde_json::Value>) {
    let Some(p) = params else { return };
    let Some(uri) = p.get("uri").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(text) = p.get("text").and_then(|v| v.as_str()) else {
        return;
    };
    let lang_id = p
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("typescript");
    let workspace_root = config.inner.lock().await.workspace_root.clone();
    let rel = crate::state::repo_relative_str(&workspace_root, uri);
    let lang = syntax::Lang::from_language_id(lang_id);
    let rel_path = std::path::PathBuf::from(&rel);
    // NoParser / Json / Yaml: content only (BM25). Symbols come from grammars we know.
    let (symbols, imports_strs) = match syntax::parse(lang, text) {
        Some(tree) => index::extract(lang, &tree, text),
        None => (Vec::new(), Vec::new()),
    };
    index.set_file(rel_path.clone(), symbols, imports_strs).await;
    index.set_text(rel_path, text.to_owned()).await;
}
