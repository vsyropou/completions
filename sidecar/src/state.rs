//! State shared across request handlers: runtime `Config` and the
//! cancellation/coordination `Pending` registry. Both are cheap-clone handles
//! (`Arc` inside); handlers clone them per request.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

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
