//! FIM (fill-in-the-middle) inference: build prompts, call upstream or mock.
//!
//! Port of Phoenix's `Complete.FimAdapter` to Rust. Same prompt layout,
//! same error contract, same HTTP shape against an OpenAI-compatible
//! `/v1/completions` endpoint. When `model_url` is absent the static mock
//! string is returned so the learning-skeleton e2e stays green.

use crate::rpc::JsonRpcError;
use serde_json::json;

/// Default FIM template when none configured.
const DEFAULT_FIM_TEMPLATE: &str = "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>";

/// ponytail: fixed for now — upstream defaults from the Elixir adapter.
const MAX_TOKENS: u32 = 128;
const TEMPERATURE: f32 = 0.2;
const TIMEOUT_MS: u64 = 25_000;

/// The one static string the learning mock returns.
const MOCK_COMPLETION: &str = "// hello from mock backend\nfunction greet(name: string): string {\n  return `hello ${name}`;\n}\n";

/// Build the prompt sent to the upstream `prompt` field. Matches Elixir's
/// `Complete.FimAdapter.build_prompt/1` exactly: `# Path:` header, optional
/// `# Related symbols:` block, then the FIM template with prefix/suffix spliced.
pub fn build_prompt(
    file_path: &str,
    repository_context: &str,
    fim_template: &str,
    prefix: &str,
    suffix: &str,
) -> String {
    let header = format!("# Path: {file_path}\n");
    let repo_block = if repository_context.is_empty() {
        String::new()
    } else {
        format!("# Related symbols:\n{repository_context}\n")
    };
    let template = if fim_template.is_empty() {
        DEFAULT_FIM_TEMPLATE
    } else {
        fim_template
    };
    let rendered = template
        .replace("{prefix}", prefix)
        .replace("{suffix}", suffix);
    format!("{header}{repo_block}{rendered}")
}

/// Result tuple: `(prompt, completion, finish_reason, model)`.
pub type CompleteResult = (String, String, String, String);

/// Call the upstream FIM endpoint, or return the static mock when
/// `cfg.model_url` is `None`. Errors map to JSON-RPC `-32002 upstream`.
pub async fn complete(
    cfg: &crate::Config,
    _language: &str,
    file_path: &str,
    prefix: &str,
    suffix: &str,
    repository_context: &str,
    request_id: &str,
) -> Result<CompleteResult, JsonRpcError> {
    let (model_url, model_api_key, model, fim_template, http) = {
        let g = cfg.inner.lock().await;
        (
            g.model_url.clone(),
            g.model_api_key.clone(),
            g.model.clone(),
            g.fim_template.clone(),
            g.http.clone(),
        )
    };

    let model = model.unwrap_or_else(|| "default".into());
    let fim_template = fim_template.unwrap_or_else(|| DEFAULT_FIM_TEMPLATE.into());
    let prompt = build_prompt(file_path, repository_context, &fim_template, prefix, suffix);

    let Some(base_url) = model_url else {
        return Ok((prompt, MOCK_COMPLETION.into(), "stop".into(), "mock".into()));
    };

    let url = format!("{}/v1/completions", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "prompt": prompt,
        "max_tokens": MAX_TOKENS,
        "temperature": TEMPERATURE,
        "stop": [],
    });

    let mut req = http
        .post(&url)
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .json(&body);
    if let Some(key) = model_api_key {
        req = req.bearer_auth(key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| JsonRpcError::upstream(format!("inference http send: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(JsonRpcError::upstream(format!(
            "inference {}: {}",
            status.as_u16(),
            body.chars().take(200).collect::<String>()
        )));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| JsonRpcError::upstream(format!("invalid inference json: {e}")))?;

    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.as_object())
        .ok_or_else(|| JsonRpcError::upstream("inference response missing choices[0]"))?;

    let text = choice
        .get("text")
        .and_then(|t| t.as_str())
        .ok_or_else(|| JsonRpcError::upstream("inference response missing choices[0].text"))?;

    let finish_reason = match choice.get("finish_reason").and_then(|f| f.as_str()) {
        Some("stop") => "stop",
        Some("length") => "length",
        Some(_) => "error",
        None => "stop",
    };

    tracing::info!(%request_id, model = %model, "inference ok");
    Ok((prompt, text.to_owned(), finish_reason.into(), model))
}
