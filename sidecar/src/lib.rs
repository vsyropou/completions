pub mod completion;
pub mod documents;
pub mod rpc;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::sync::Mutex;

use documents::Documents;
use rpc::{read_message, write_message, JsonRpcError, RequestId};

#[derive(Clone)]
pub struct Config {
    pub(crate) inner: Arc<Mutex<Inner>>,
}

pub struct Inner {
    pub phoenix_url: String,
    pub auth_token: String,
    pub workspace_root: String,
    pub max_tokens_default: u32,
    pub http: reqwest::Client,
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
                phoenix_url: String::new(),
                auth_token: String::new(),
                workspace_root: String::new(),
                max_tokens_default: 128,
                http,
            })),
        }
    }
}

impl Config {
    /// Take a snapshot of the config needed for one HTTP call.
    pub async fn snapshot(&self) -> (String, String, String, u32, reqwest::Client) {
        let g = self.inner.lock().await;
        (
            g.phoenix_url.clone(),
            g.auth_token.clone(),
            g.workspace_root.clone(),
            g.max_tokens_default,
            g.http.clone(),
        )
    }
}

// Track in-flight completion tasks. Cancelling marks the entry so that:
//  - the cancel handler immediately writes a -32003 response for the JSON-RPC request id; and
//  - the task aborts its HTTP work if still running.
struct Pending {
    // Maps JSON-RPC numeric request id -> abort handle of the spawn.
    cancel: Mutex<HashMap<i64, tokio::task::AbortHandle>>,
    // Set of ids that have already been answered (so cancel racing completion is safe).
    answered: Mutex<std::collections::HashSet<i64>>,
}

pub async fn run() {
    let config = Config::default();
    let documents: Documents = Documents::default();
    let pending = Arc::new(Pending {
        cancel: Mutex::new(HashMap::new()),
        answered: Mutex::new(std::collections::HashSet::new()),
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
                tracing::warn!(%e, "framing read error");
                write_err(&writer, RequestId::Null, JsonRpcError::parse_error()).await;
                continue;
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
                let resp = match handle_initialize(&config, &val).await {
                    Ok(v) => rpc::success_response(id, v),
                    Err(e) => e.into_response(id),
                };
                write(&writer, resp).await;
            }
            ("shutdown", Some(id)) => {
                let resp = rpc::success_response(id, serde_json::Value::Null);
                write(&writer, resp).await;
                tracing::info!("shutdown");
                return;
            }
            ("completion/request", Some(id)) => {
                let cfg = config.clone();
                let docs = documents.clone();
                let pnd = pending.clone();
                let w = writer.clone();
                let num_id = match &id {
                    RequestId::Num(n) => Some(*n),
                    _ => None,
                };
                let task_id = id.clone();
                let task = tokio::spawn(async move {
                    let result = completion::handle_completion_request(cfg, docs, val).await;
                    // If id was cancelled while we worked, skip responding (already answered -32003).
                    if let Some(n) = num_id {
                        if pnd.answered.lock().await.contains(&n) {
                            pnd.cancel.lock().await.remove(&n);
                            pnd.answered.lock().await.remove(&n);
                            return;
                        }
                        pnd.answered.lock().await.insert(n);
                        pnd.cancel.lock().await.remove(&n);
                    }
                    let resp = match result {
                        Ok(v) => rpc::success_response(task_id, v),
                        Err(e) => e.into_response(task_id),
                    };
                    let mut w = w.lock().await;
                    let _ = write_message(&mut *w, &resp).await;
                });
                if let Some(n) = num_id {
                    pending.cancel.lock().await.insert(n, task.abort_handle());
                }
            }
            ("completion/cancel", None) => {
                if let Some(rid) = val
                    .get("params")
                    .and_then(|p| p.get("request_id"))
                    .and_then(|r| r.as_i64())
                {
                    let handle = pending.cancel.lock().await.remove(&rid);
                    let already_answered = pending.answered.lock().await.contains(&rid);
                    if let Some(h) = handle {
                        h.abort();
                    }
                    if !already_answered {
                        pending.answered.lock().await.insert(rid);
                        let err = JsonRpcError::cancelled().into_response(RequestId::Num(rid));
                        write(&writer, err).await;
                    }
                }
            }
            ("textDocument/didOpen", None) => {
                documents
                    .did_open(val.get("params").unwrap_or(&serde_json::Value::Null))
                    .await;
            }
            ("textDocument/didChange", None) => {
                documents
                    .did_change(val.get("params").unwrap_or(&serde_json::Value::Null))
                    .await;
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

async fn write(writer: &Arc<Mutex<tokio::io::Stdout>>, v: serde_json::Value) {
    let mut w = writer.lock().await;
    let _ = write_message(&mut *w, &v).await;
}

async fn write_err(writer: &Arc<Mutex<tokio::io::Stdout>>, id: RequestId, e: JsonRpcError) {
    write(writer, e.into_response(id)).await;
}

async fn handle_initialize(
    cfg: &Config,
    val: &serde_json::Value,
) -> Result<serde_json::Value, JsonRpcError> {
    let params = val
        .get("params")
        .ok_or_else(|| JsonRpcError::invalid_params("params required"))?;
    let phoenix_url = params
        .get("phoenix_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JsonRpcError::invalid_params("phoenix_url required"))?
        .to_owned();
    let auth_token = params
        .get("auth_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let workspace_root = params
        .get("workspace_root")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let max_tokens = params
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(128);

    let mut g = cfg.inner.lock().await;
    g.phoenix_url = phoenix_url;
    g.auth_token = auth_token;
    g.workspace_root = workspace_root;
    g.max_tokens_default = max_tokens;

    Ok(serde_json::json!({ "capabilities": { "languages": ["typescript", "tsx"] } }))
}
