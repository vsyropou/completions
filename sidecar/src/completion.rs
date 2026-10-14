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

const PREFIX_CAP: usize = 6000;
const SUFFIX_CAP: usize = 1000;

/// Deserialized `completion/request` params. Field names match the protocol.
#[derive(Deserialize)]
pub struct CompletionParams {
    pub document_uri: String,
    pub document_version: i64,
    pub position: Position,
    #[serde(default)]
    pub max_tokens: Option<u32>,
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
/// version mismatch (`-32001`) → upstream errors (`-32002`).
pub async fn handle_completion_request(
    cfg: crate::Config,
    documents: crate::documents::Documents,
    index: crate::index::Index,
    req: serde_json::Value,
) -> Result<serde_json::Value, JsonRpcError> {
    let t0 = std::time::Instant::now();
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

    let offset = utf16_to_utf8_offset(&doc.text, p.position.line, p.position.character)?;
    let prefix = cap_prefix(&doc.text[..offset], PREFIX_CAP).to_owned();
    let suffix = cap_suffix(&doc.text[offset..], SUFFIX_CAP).to_owned();

    let (phoenix_url, auth_token, workspace_root, default_max_tokens, measure_latency, http) =
        cfg.snapshot().await;
    let max_tokens = p.max_tokens.unwrap_or(default_max_tokens);
    let request_id = uuid::Uuid::new_v4().to_string();
    let file_path = crate::repo_relative_str(&workspace_root, &p.document_uri);

    // Phase 2: build repository_context from tree-sitter + in-memory index.
    let lang = crate::syntax::Lang::from_language_id(&doc.language);
    let repository_context = match crate::syntax::parse(lang, &doc.text) {
        Some(tree) => {
            crate::context::build_repository_context(&index, &file_path, &tree, &doc.text, offset)
                .await
        }
        None => String::new(),
    };

    let url = format!("{}/api/v1/completions", phoenix_url.trim_end_matches('/'));
    let body = json!({
        "request_id": request_id,
        "language": doc.language,
        "file_path": file_path,
        "prefix": prefix,
        "suffix": suffix,
        "repository_context": repository_context,
        "max_tokens": max_tokens,
    });

    let http_start = std::time::Instant::now();
    let resp = http
        .post(&url)
        .bearer_auth(&auth_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| JsonRpcError::upstream(format!("http send: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(JsonRpcError::upstream(format!(
            "phoenix {}: {}",
            status.as_u16(),
            body.chars().take(200).collect::<String>()
        )));
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| JsonRpcError::upstream(format!("invalid phoenix json: {e}")))?;
    let http_ms = http_start.elapsed().as_millis() as u64;
    let completion = v
        .get("completion")
        .and_then(|c| c.as_str())
        .ok_or_else(|| JsonRpcError::upstream("phoenix response missing 'completion'"))?;

    let mut out = json!({
        "text": completion,
        "document_version": doc.version,
    });
    if measure_latency {
        let total_ms = t0.elapsed().as_millis() as u64;
        out.as_object_mut().unwrap().insert(
            "latency_ms".to_string(),
            json!({ "total": total_ms, "http": http_ms }),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_once(listener: TcpListener, reply: &'static str) -> String {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n").map(|p| p + 4) {
                let headers = String::from_utf8_lossy(&buf[..pos]).to_string();
                let mut content_length = 0usize;
                for line in headers.lines() {
                    let lower = line.to_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                if buf.len() >= pos + content_length {
                    sock.write_all(reply.as_bytes()).await.unwrap();
                    break;
                }
            }
        }
        String::from_utf8(buf).unwrap()
    }

    fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    async fn mock_config(url: String) -> Config {
        let cfg = Config::default();
        {
            let mut g = cfg.inner.lock().await;
            g.phoenix_url = url;
            g.auth_token = "tok".into();
            g.workspace_root = "file:///repo".into();
            g.max_tokens_default = 128;
        }
        cfg
    }

    fn open_doc() -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "completion/request",
            "id": 7,
            "params": {
                "document_uri": "file:///repo/src/main.ts",
                "document_version": 3,
                "position": { "line": 1, "character": 8 },
                "max_tokens": 64
            }
        })
    }

    async fn open_documents() -> crate::documents::Documents {
        let docs = crate::documents::Documents::default();
        docs.did_open(&serde_json::json!({
            "uri": "file:///repo/src/main.ts",
            "language": "typescript",
            "version": 3,
            "text": "l0\nconst x = 1;\nl2"
        }))
        .await;
        docs
    }

    #[tokio::test]
    async fn http_success_returns_completion() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 86\r\nConnection: close\r\n\r\n{\"request_id\":\"x\",\"completion\":\"  return 42;\\n\",\"finish_reason\":\"stop\",\"model\":\"mock\"}";
        let server = tokio::spawn(serve_once(listener, reply));

        let cfg = mock_config(format!("http://{addr}")).await;
        let docs = open_documents().await;
        let res = handle_completion_request(cfg, docs, crate::index::Index::default(), open_doc())
            .await
            .unwrap();
        assert_eq!(res["text"], "  return 42;\n");
        assert_eq!(res["document_version"], 3);

        let body = server.await.unwrap();
        assert!(
            body.starts_with("POST /api/v1/completions "),
            "got: {}",
            body
        );
        // repo-relative file_path
        assert!(body.contains("\"file_path\":\"src/main.ts\""));
        assert!(body.contains("\"language\":\"typescript\""));
        assert!(body.contains("\"prefix\":\"l0\\nconst x \""));
        assert!(body.contains("\"suffix\":\"= 1;\\nl2\""));
        assert!(body.contains("\"max_tokens\":64"));
        assert!(body.contains("\"repository_context\":\"\""));
        assert!(body.contains("authorization: Bearer tok"));
    }

    #[tokio::test]
    async fn latency_ms_present_when_opted_in() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 86\r\nConnection: close\r\n\r\n{\"request_id\":\"x\",\"completion\":\"  return 42;\\n\",\"finish_reason\":\"stop\",\"model\":\"mock\"}";
        let server = tokio::spawn(serve_once(listener, reply));

        let cfg = mock_config(format!("http://{addr}")).await;
        cfg.inner.lock().await.measure_latency = true;
        let docs = open_documents().await;
        let res = handle_completion_request(cfg, docs, crate::index::Index::default(), open_doc())
            .await
            .unwrap();
        let lm = res["latency_ms"].as_object().expect("latency_ms present");
        let total = lm["total"].as_u64().unwrap();
        let http = lm["http"].as_u64().unwrap();
        assert!(total >= http, "total={} http={}", total, http);

        // Opt-in default false: same request without the flag yields no key.
        let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr2 = listener2.local_addr().unwrap();
        let server2 = tokio::spawn(serve_once(listener2, reply));
        let cfg2 = mock_config(format!("http://{addr2}")).await;
        let docs2 = open_documents().await;
        let res2 =
            handle_completion_request(cfg2, docs2, crate::index::Index::default(), open_doc())
                .await
                .unwrap();
        assert!(res2.get("latency_ms").is_none());
        server.await.unwrap();
        server2.await.unwrap();
    }

    #[tokio::test]
    async fn http_401_maps_to_upstream_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reply = "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 53\r\nConnection: close\r\n\r\n{\"error\":{\"code\":\"unauthorized\",\"message\":\"invalid\"}}";
        let server = tokio::spawn(serve_once(listener, reply));

        let cfg = mock_config(format!("http://{addr}")).await;
        let docs = open_documents().await;
        let err = handle_completion_request(cfg, docs, crate::index::Index::default(), open_doc())
            .await
            .unwrap_err();
        assert_eq!(err.code, -32002, "got: {:?}", err);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn version_mismatch_returns_32001() {
        let cfg = mock_config("http://127.0.0.1:1".into()).await;
        let docs = open_documents().await;
        let mut req = open_doc();
        req["params"]["document_version"] = serde_json::json!(99);
        let err = handle_completion_request(cfg, docs, crate::index::Index::default(), req)
            .await
            .unwrap_err();
        assert_eq!(err.code, -32001);
    }

    #[tokio::test]
    async fn unknown_document_returns_32000() {
        let cfg = mock_config("http://127.0.0.1:1".into()).await;
        let docs = crate::documents::Documents::default();
        let err = handle_completion_request(cfg, docs, crate::index::Index::default(), open_doc())
            .await
            .unwrap_err();
        assert_eq!(err.code, -32000);
    }

    #[test]
    fn utf16_basic_ascii() {
        assert_eq!(utf16_to_utf8_offset("hello\nworld", 1, 3).unwrap(), 9);
    }

    #[test]
    fn utf16_multibyte() {
        // "é" is 2 bytes utf8, 1 utf16 unit
        let s = "ébc";
        assert_eq!(utf16_to_utf8_offset(s, 0, 1).unwrap(), 2);
        assert_eq!(utf16_to_utf8_offset(s, 0, 3).unwrap(), 4);
    }

    #[test]
    fn utf16_surrogate_pair_boundary_before() {
        // "😀" = U+1F600, 4 utf8 bytes, 2 utf16 units (surrogate pair)
        let s = "a😀b";
        // char 0 → 0 (before 'a')
        assert_eq!(utf16_to_utf8_offset(s, 0, 0).unwrap(), 0);
        // char 1 → 1 (after 'a', before emoji)
        assert_eq!(utf16_to_utf8_offset(s, 0, 1).unwrap(), 1);
        // char 2 → mid-pair (after high surrogate): protocol §4 → boundary BEFORE pair → 1
        assert_eq!(utf16_to_utf8_offset(s, 0, 2).unwrap(), 1);
        // char 3 → after emoji: byte 5
        assert_eq!(utf16_to_utf8_offset(s, 0, 3).unwrap(), 5);
        // char 4 → after 'b': byte 6
        assert_eq!(utf16_to_utf8_offset(s, 0, 4).unwrap(), 6);
    }

    #[test]
    fn utf16_out_of_range_errors() {
        assert!(utf16_to_utf8_offset("ab", 5, 0).is_err());
        assert!(utf16_to_utf8_offset("ab", 0, 10).is_err());
    }

    #[test]
    fn prefix_and_suffix_caps_on_char_boundary() {
        let many = "a".repeat(10_000);
        let p = cap_prefix(&many, PREFIX_CAP);
        assert_eq!(p.len(), 6000);
        let s = cap_suffix(&many, SUFFIX_CAP);
        assert_eq!(s.len(), 1000);

        // Non-boundary: emoji 4 bytes.
        // Build 5998 ascii + emoji (4 bytes) + 1 ascii. Total 6003 bytes.
        let mut s = "a".repeat(5998);
        s.push('😀');
        s.push('z');
        assert_eq!(s.len(), 6003);
        let capped = cap_prefix(&s, PREFIX_CAP);
        // start at 6003-6000=3, boundary ok (ascii)
        assert_eq!(capped.len(), 6000);

        // Cap start lands exactly at the emoji's first byte: boundary ok, returns the emoji.
        // "a😀" = 5 bytes; cap 4 → start=1 (emoji's first byte) → "😀".
        let capped2 = cap_prefix("a😀", 4);
        assert_eq!(capped2.len(), 4);
        assert_eq!(capped2, "😀");

        // Cap start lands strictly inside the emoji's byte span → walk forward past emoji.
        // "ab😀" = 6 bytes (emoji at 2..6); cap 3 → start=3 (inside) → walk to 6 → "".
        let capped2b = cap_prefix("ab😀", 3);
        assert_eq!(capped2b.len(), 0);

        // "ab😀": len=6, cap 4 → start=2, which IS the emoji's first byte → boundary ok → "😀".
        let capped3 = cap_prefix("ab😀", 4);
        assert_eq!(capped3.len(), 4);
        assert_eq!(capped3, "😀");

        // "abc😀": len=7, cap 4 → start=3, inside emoji (3..7 range? emoji at 3..7; 3 is first byte) → boundary ok → "😀".
        let capped4 = cap_prefix("abc😀", 4);
        assert_eq!(capped4.len(), 4);

        // "abcd😀": len=8, cap 5 → start=3 → 'd' boundary → "d😀".
        let capped5 = cap_prefix("abcd😀", 5);
        assert_eq!(capped5.len(), 5);
        assert_eq!(capped5, "d😀");

        // Non-boundary: cap such that we land MID-emoji. "abcd😀": len=8, cap 6 → start=2 → 'c' boundary → "cd😀".
        // Duplicate sanity check: same as capped2b.
        let capped6 = cap_prefix("ab😀", 3);
        assert_eq!(capped6.len(), 0);

        // Suffix: cap inside emoji → walks backward. "😀ab": len=6, cap 5 → end=5 (inside emoji? emoji is 0..4; byte 5 is 'b').
        // Try "😀ab" cap 3 → end=3, inside emoji → walk back to 0 → "".
        let suff = cap_suffix("😀ab", 3);
        assert_eq!(suff.len(), 0);
        let suff2 = cap_suffix("😀ab", 4);
        assert_eq!(suff2, "😀");
    }
}
