//! Fire-and-forget telemetry: POST each completion to the telemetry sink's
//! `/api/v1/completions` endpoint after the inference result has
//! already been sent back to the editor. Never blocks the response path;
//! errors are logged at `debug` and dropped.

use serde_json::json;

/// POST timeout. Generous — telemetry is a side-channel, latency doesn't matter.
const TELEMETRY_TIMEOUT_SECS: u64 = 5;

/// Payload for one completed inference call. All strings are owned because the
/// POST is spawned and outlives the request handler.
pub struct Telemetry {
    pub request_id: String,
    pub language: String,
    pub file_path: String,
    pub prompt: String,
    pub response: String,
    pub latency_ms: u64,
    pub provider: &'static str,
    pub model: String,
    pub finish_reason: String,
}

/// Spawn the POST. Returns immediately; the task finishes whenever it finishes.
pub fn fire(cfg: crate::Config, t: Telemetry) {
    tokio::spawn(async move {
        let (telemetry_url, telemetry_api_key, http) = cfg.telemetry().await;
        if telemetry_url.is_empty() {
            return; // telemetry sink not configured — skip silently
        }
        let url = format!(
            "{}/api/v1/completions",
            telemetry_url.trim_end_matches('/')
        );
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
