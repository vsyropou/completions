//! Fire-and-forget telemetry: POST each completion event to the telemetry
//! sink's `/api/v1/events` endpoint after the inference result has
//! already been sent back to the editor (or when a request terminates early).
//! Never blocks the response path; errors are logged at `debug` and dropped.

use serde_json::json;

/// POST timeout. Generous — telemetry is a side-channel, latency doesn't matter.
const TELEMETRY_TIMEOUT_SECS: u64 = 5;

/// Payload for one event in a request's life. All strings are owned because
/// the POST is spawned and outlives the request handler.
pub struct Telemetry {
    /// Unique request id (sidecar-assigned counter).
    pub request_id: String,
    /// VS Code language id (e.g. `"python"`).
    pub language: String,
    /// Workspace-relative forward-slash path of the completed file.
    pub file_path: String,
    /// Full prompt body sent to the inference server. Empty for non-served outcomes.
    pub prompt: String,
    /// Raw completion text returned. Empty for non-served outcomes.
    pub response: String,
    /// Wall-time of the inference HTTP hop in milliseconds. 0 for non-served outcomes.
    pub latency_ms: u64,
    /// Which path served this: `"mock"` (no `model_url`) or `"model"`.
    pub provider: &'static str,
    /// Model id actually used (empty when mock).
    pub model: String,
    /// Provider's `finish_reason` string, normalized in [`crate::inference`]. Empty unless served.
    pub finish_reason: String,
    /// Request outcome: `requested` | `served` | `superseded` | `error`.
    /// `accepted` and `cancelled` events come from the extension directly.
    pub outcome: &'static str,
}

/// Spawn the POST. Returns immediately; the task finishes whenever it finishes.
pub fn fire(cfg: crate::Config, t: Telemetry) {
    tokio::spawn(async move {
        let (telemetry_url, telemetry_api_key, http) = cfg.telemetry().await;
        if telemetry_url.is_empty() {
            return; // telemetry sink not configured — skip silently
        }
        let url = format!("{}/api/v1/events", telemetry_url.trim_end_matches('/'));
        let body = json!({
            "request_id": t.request_id,
            "language": t.language,
            "file_path": t.file_path,
            "prompt": t.prompt,
            "response": t.response,
            "latency_ms": t.latency_ms,
            "provider": t.provider,
            "model": t.model,
            "finish_reason": t.finish_reason,
            "outcome": t.outcome,
        });
        let result = http
            .post(&url)
            .timeout(std::time::Duration::from_secs(TELEMETRY_TIMEOUT_SECS))
            .bearer_auth(telemetry_api_key)
            .json(&body)
            .send()
            .await;
        if let Err(e) = result {
            tracing::debug!(url = %url, error = %e, "telemetry post failed");
        }
    });
}
