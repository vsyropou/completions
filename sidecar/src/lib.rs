//! Sidecar library — JSON-RPC 2.0 over stdio server driving completions.
//!
//! Spawned as a child process by the VS Code extension. Reads LSP-style framed
//! JSON-RPC from stdin, writes responses to stdout, logs to stderr, and
//! forwards completion work to the Phoenix backend over HTTP. See
//! `../protocol/README.md` for the frozen wire contract.

pub mod completion;
pub mod context;
pub mod documents;
pub mod index;
pub mod inference;
pub mod rpc;
pub mod syntax;
pub mod telemetry;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::sync::Mutex;

use documents::Documents;
use rpc::{read_message, write_message, JsonRpcError, RequestId};

/// Shared runtime configuration. Cheap to clone (Arc inside).
#[derive(Clone)]
pub struct Config {
    pub inner: Arc<Mutex<Inner>>,
}

/// The mutable fields behind `Config`. Lock, mutate, drop.
pub struct Inner {
    pub telemetry_url: String,
    pub telemetry_api_key: String,
    pub workspace_root: String,
    pub http: reqwest::Client,
    pub model_url: Option<String>,
    pub model_api_key: Option<String>,
    pub model: Option<String>,
    pub fim_template: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            inner: Arc::new(Mutex::new(Inner {
                telemetry_url: String::new(),
                telemetry_api_key: String::new(),
                workspace_root: String::new(),
                http,
                model_url: None,
                model_api_key: None,
                model: None,
                fim_template: None,
                temperature: None,
                max_tokens: None,
            })),
        }
    }
}

impl Config {
    /// Snapshot the three fields `telemetry::fire` needs to POST one event.
    pub async fn telemetry(&self) -> (String, String, reqwest::Client) {
        let g = self.inner.lock().await;
        (
            g.telemetry_url.clone(),
            g.telemetry_api_key.clone(),
            g.http.clone(),
        )
    }
}

// Track in-flight completion tasks. Cancelling marks the entry so that:
//  - the cancel handler immediately writes a -32003 response for the JSON-RPC request id; and
//  - the task aborts its HTTP work if still running.
// One Mutex covers both fields so the contains+insert pair is atomic — otherwise
// a cancel in the gap would emit -32003 AND the success reply for one id.
#[derive(Default)]
pub struct PendingState {
    pub cancel: HashMap<i64, tokio::task::AbortHandle>,
    pub answered: std::collections::HashSet<i64>,
    /// Join handles for in-flight completion tasks. Shutdown awaits them
    /// (best-effort, capped) so a `shutdown` frame doesn't silently swallow
    /// a completion whose inference is nearly done.
    pub tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub struct Pending {
    pub state: Mutex<PendingState>,
}

/// Run the JSON-RPC loop until `shutdown` or stdin EOF. Entry point called from `main`.
pub async fn run() {
    let config = Config::default();
    let documents: Documents = Documents::default();
    let index = index::Index::default();
    let debounce = completion::Debounce::default();
    let pending = Arc::new(Pending {
        state: Mutex::new(PendingState::default()),
    });

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let writer = Arc::new(Mutex::new(tokio::io::stdout()));

    loop {
        let msg = match read_message(&mut reader).await {
            Ok(Some(m)) => m,
            Ok(None) => {
                tracing::info!("stdin EOF, exiting");
                return;
            }
            Err(e) => {
                // Match the TS decoder: malformed framing is fatal. Log and exit
                // non-zero instead of absorbing garbage and desynchronizing the stream.
                tracing::error!(%e, "framing read error, exiting");
                std::process::exit(1);
            }
        };

        let val: serde_json::Value = match serde_json::from_slice(&msg) {
            Ok(v) => v,
            Err(_) => {
                write_err(&writer, RequestId::Null, JsonRpcError::parse_error()).await;
                continue;
            }
        };

        let Some(method) = val
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_owned)
        else {
            continue;
        };

        let id = val.get("id").and_then(|i| {
            if i.is_i64() {
                i.as_i64().map(RequestId::Num)
            } else if i.is_string() {
                i.as_str().map(|s| RequestId::Str(s.to_owned()))
            } else {
                None
            }
        });

        match (method.as_str(), id) {
            ("initialize", Some(id)) => {
                let resp = match handle_initialize(&config, &val, &index).await {
                    Ok(v) => rpc::success_response(id, v),
                    Err(e) => e.into_response(id),
                };
                write(&writer, resp).await;
            }
            ("shutdown", Some(id)) => {
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
                write(&writer, resp).await;
                tracing::info!("shutdown");
                return;
            }
            ("completion/request", Some(id)) => {
                let cfg = config.clone();
                let docs = documents.clone();
                let idx = index.clone();
                let dbnc = debounce.clone();
                let pnd = pending.clone();
                let w = writer.clone();
                let num_id = match &id {
                    RequestId::Num(n) => Some(*n),
                    _ => None,
                };
                let task_id = id.clone();
                let task = tokio::spawn(async move {
                    let result =
                        completion::handle_completion_request(cfg, docs, idx, val, dbnc).await;
                    // Atomic with the cancel handler: if the id was already answered (cancel
                    // raced us and won), drop the result and clean up. Otherwise claim the
                    // response slot so the cancel handler becomes a no-op.
                    if let Some(n) = num_id {
                        let mut st = pnd.state.lock().await;
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
                    let mut w = w.lock().await;
                    let _ = write_message(&mut *w, &resp).await;
                });
                if let Some(n) = num_id {
                    pending
                        .state
                        .lock()
                        .await
                        .cancel
                        .insert(n, task.abort_handle());
                }
                pending.state.lock().await.tasks.push(task);
            }
            ("completion/cancel", None) => {
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
                        write(&writer, err).await;
                    }
                }
            }
            ("textDocument/didOpen", None) => {
                documents
                    .did_open(val.get("params").unwrap_or(&serde_json::Value::Null))
                    .await;
                refresh_index_for_doc(&index, &config, val.get("params")).await;
            }
            ("textDocument/didChange", None) => {
                if let Err(e) = documents
                    .did_change(val.get("params").unwrap_or(&serde_json::Value::Null))
                    .await
                {
                    // Leave the document untouched; the completion path will
                    // -32001 against the stale server version.
                    tracing::warn!(error = %e, "didChange rejected, document not updated");
                } else {
                    refresh_index_for_doc(&index, &config, val.get("params")).await;
                }
            }
            ("textDocument/didClose", None) => {
                documents
                    .did_close(val.get("params").unwrap_or(&serde_json::Value::Null))
                    .await;
            }
            (_, id_opt) => {
                if let Some(id) = id_opt {
                    write_err(&writer, id, JsonRpcError::method_not_found(&method)).await;
                }
            }
        }
    }
}

/// Lock the shared stdout writer and push one framed message. Bytes sent are dropped
/// silently on I/O error — the extension is the only reader, and if it's gone we're
/// shutting down anyway.
async fn write(writer: &Arc<Mutex<tokio::io::Stdout>>, v: serde_json::Value) {
    let mut w = writer.lock().await;
    let _ = write_message(&mut *w, &v).await;
}

/// Shorthand for `write()` of an error response.
async fn write_err(writer: &Arc<Mutex<tokio::io::Stdout>>, id: RequestId, e: JsonRpcError) {
    write(writer, e.into_response(id)).await;
}

/// Handle `initialize`: pull `telemetry_url`, `telemetry_api_key`,
/// `workspace_root`, `measure_latency` (and the optional `model_*` fields) out
/// of params, stash them in `Config`, and spawn workspace indexing in the
/// background so completions never wait on it.
async fn handle_initialize(
    cfg: &Config,
    val: &serde_json::Value,
    index: &index::Index,
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
async fn refresh_index_for_doc(
    index: &index::Index,
    config: &Config,
    params: Option<&serde_json::Value>,
) {
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
    let rel = repo_relative_str(&workspace_root, uri);
    let lang = syntax::Lang::from_language_id(lang_id);
    let rel_path = std::path::PathBuf::from(&rel);
    // NoParser / Json / Yaml: content only (BM25). Symbols come from grammars we know.
    let (symbols, imports_strs) = match syntax::parse(lang, text) {
        Some(tree) => index::extract(lang, &tree, text),
        None => (Vec::new(), Vec::new()),
    };
    index
        .set_file(rel_path.clone(), symbols, imports_strs)
        .await;
    index.set_text(rel_path, text.to_owned()).await;
}

/// Strip the workspace prefix (or `file://`) off a URI to yield a repo-relative
/// forward-slash path — the shape Phoenix expects in `file_path`.
pub fn repo_relative_str(workspace_root: &str, uri: &str) -> String {
    if !workspace_root.is_empty() {
        if let Some(stripped) = uri.strip_prefix(workspace_root) {
            return stripped.trim_start_matches('/').to_owned();
        }
    }
    uri.strip_prefix("file://")
        .map(|s| s.trim_start_matches('/').to_owned())
        .unwrap_or_else(|| uri.to_owned())
}
