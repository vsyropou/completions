//! Integration tests for the fire-and-forget telemetry POST in `completion.rs`.
//! Each case drives `handle_completion_request` end-to-end against a real TCP
//! listener standing in for Phoenix.

use sidecar::completion::{handle_completion_request, Debounce};
use sidecar::Config;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Accept `n` connections, read headers+body off each, reply with a bare 204,
/// and return the raw request bytes in arrival order. Each telemetry fire is a
/// detached spawned POST, so they land in an arbitrary-but-sequential order —
/// callers assert on the SET of arrived bodies, not the order.
async fn serve_n(listener: TcpListener, n: usize) -> Vec<String> {
    let mut raws = Vec::with_capacity(n);
    for _ in 0..n {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let raw = loop {
            let n = sock.read(&mut chunk).await.unwrap();
            if n == 0 {
                break buf;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n").map(|p| p + 4) {
                let mut content_length = 0usize;
                for line in String::from_utf8_lossy(&buf[..pos]).lines() {
                    let lower = line.to_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                if buf.len() >= pos + content_length {
                    break buf;
                }
            }
        };
        let _ = sock
            .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
            .await;
        raws.push(String::from_utf8(raw).unwrap());
    }
    raws
}

/// Accept one connection, read headers+body, wait `delay` (so tests can observe
/// a hanging sink), reply with a bare 204, and return the raw request bytes.
async fn serve_one(listener: TcpListener, delay: Duration) -> String {
    let (mut sock, _) = listener.accept().await.unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let raw = loop {
        let n = sock.read(&mut chunk).await.unwrap();
        if n == 0 {
            break buf;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n").map(|p| p + 4) {
            let mut content_length = 0usize;
            for line in String::from_utf8_lossy(&buf[..pos]).lines() {
                let lower = line.to_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            if buf.len() >= pos + content_length {
                break buf;
            }
        }
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let _ = sock
        .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
        .await;
    String::from_utf8(raw).unwrap()
}

async fn cfg_with_telemetry(telemetry_url: Option<String>) -> Config {
    let cfg = Config::default();
    {
        let mut g = cfg.inner.lock().await;
        g.workspace_root = "file:///repo".into();
        g.telemetry_url = telemetry_url.unwrap_or_default();
        g.telemetry_api_key = "test-token".into();
        // model_url None → mock path → inference returns instantly.
        g.model_url = None;
    }
    cfg
}

async fn open_documents() -> sidecar::documents::Documents {
    let docs = sidecar::documents::Documents::default();
    docs.did_open(&serde_json::json!({
        "uri": "file:///repo/src/main.ts",
        "language": "typescript",
        "version": 3,
        "text": "l0\nconst x = 1;\nl2"
    }))
    .await;
    docs
}

fn req() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "completion/request",
        "id": 1,
        "params": {
            "document_uri": "file:///repo/src/main.ts",
            "document_version": 3,
            "position": { "line": 1, "character": 8 }
        }
    })
}

#[tokio::test]
async fn telemetry_body_has_all_fields() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // One completion emits two events — `requested` up front, `served` after
    // inference — each from its own detached POST task.
    let server = tokio::spawn(serve_n(listener, 2));

    let cfg = cfg_with_telemetry(Some(format!("http://{addr}"))).await;
    let docs = open_documents().await;
    let res = handle_completion_request(
        cfg,
        docs,
        sidecar::index::Index::default(),
        req(),
        Debounce::default(),
    )
    .await
    .unwrap();
    assert!(res["text"].is_string(), "completion text returned");
    assert!(res["request_id"].is_string(), "v1.3: request_id in the result");

    let raws = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("telemetry POSTs arrived")
        .unwrap();
    let parse = |raw: &String| {
        let split = find_subslice(raw.as_bytes(), b"\r\n\r\n").unwrap();
        let headers = raw[..split].to_string();
        let body: serde_json::Value = serde_json::from_str(&raw[split + 4..]).unwrap();
        (headers, body)
    };
    let (headers, bodies): (Vec<_>, Vec<_>) = raws.iter().map(parse).unzip();
    let served = bodies
        .iter()
        .find(|b| b["outcome"] == "served")
        .expect("a served event arrived");
    let requested = bodies
        .iter()
        .find(|b| b["outcome"] == "requested")
        .expect("a requested event arrived");

    // Both events share the request id and the wire schema.
    assert_eq!(served["request_id"], requested["request_id"]);
    assert_eq!(requested["language"], "typescript");
    assert_eq!(requested["prompt"], "");
    assert_eq!(requested["response"], "");
    assert_eq!(requested["latency_ms"], 0);

    let body = served;
    for key in [
        "request_id",
        "language",
        "file_path",
        "prompt",
        "response",
        "latency_ms",
        "provider",
        "model",
        "finish_reason",
        "outcome",
    ] {
        assert!(body.get(key).is_some(), "missing key {key} in {body}");
    }
    assert_eq!(body["language"], "typescript");
    assert_eq!(body["file_path"], "src/main.ts");
    assert_eq!(body["provider"], "mock");
    assert_eq!(body["model"], "mock");
    assert_eq!(body["finish_reason"], "stop");
    let prompt = body["prompt"].as_str().unwrap();
    assert!(prompt.contains("# Path: src/main.ts"), "prompt carries path");
    assert!(prompt.contains("const x"), "prompt carries cursor-line prefix; got: {prompt}");
    let response = body["response"].as_str().unwrap();
    assert!(
        response.contains("hello from mock backend"),
        "full mock response on the wire"
    );
    assert!(body["latency_ms"].is_number());
    for h in &headers {
        assert!(
            h.contains("authorization: Bearer test-token"),
            "{h}"
        );
        assert!(
            h.to_lowercase().contains("content-type: application/json"),
            "{h}"
        );
        assert!(
            h.starts_with("POST /api/v1/events "),
            "{h}"
        );
    }
}

#[tokio::test]
async fn completion_returns_even_when_sink_hangs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Sink: read body, then hang for 2s before replying. Sidecar must not wait.
    let server = tokio::spawn(serve_one(listener, Duration::from_millis(2000)));

    let cfg = cfg_with_telemetry(Some(format!("http://{addr}"))).await;
    let docs = open_documents().await;
    let t0 = Instant::now();
    let res = handle_completion_request(
        cfg,
        docs,
        sidecar::index::Index::default(),
        req(),
        Debounce::default(),
    )
    .await
    .unwrap();
    let elapsed = t0.elapsed();
    assert!(res["text"].is_string());
    assert!(
        elapsed < Duration::from_millis(1500),
        "completion blocked on telemetry sink: {elapsed:?}"
    );
    let _ = tokio::time::timeout(Duration::from_secs(4), server).await;
}

#[tokio::test]
async fn superseded_burst_emits_superseded_then_requested_and_served() {
    // First request is superseded by the second arriving inside the debounce
    // window; both fire telemetry, so the sink sees three events: superseded
    // (req-1), requested + served (req-2). Assert the set, not the order —
    // each fire is its own spawned POST.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_n(listener, 3));

    let cfg = cfg_with_telemetry(Some(format!("http://{addr}"))).await;
    let docs = open_documents().await;
    let debounce = Debounce::default();

    let (r1, r2) = tokio::join!(
        handle_completion_request(
            cfg.clone(),
            docs.clone(),
            sidecar::index::Index::default(),
            req(),
            debounce.clone(),
        ),
        handle_completion_request(
            cfg.clone(),
            docs,
            sidecar::index::Index::default(),
            req(),
            debounce,
        )
    );
    // Exactly one of the pair wins the debounce; the loser got -32003.
    let (winner, loser) = if r1.is_ok() { (r1, r2) } else { (r2, r1) };
    assert!(winner.is_ok(), "exactly one burst request must succeed");
    assert_eq!(loser.unwrap_err().code, -32003);

    let raws = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("telemetry POSTs arrived")
        .unwrap();
    let outcomes: Vec<String> = raws
        .iter()
        .map(|raw| {
            let split = find_subslice(raw.as_bytes(), b"\r\n\r\n").unwrap();
            let body: serde_json::Value = serde_json::from_str(&raw[split + 4..]).unwrap();
            assert!(body["request_id"].as_str().unwrap().starts_with("req-"));
            body["outcome"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(outcomes.len(), 3);
    for want in ["requested", "served", "superseded"] {
        assert!(
            outcomes.iter().any(|o| o == want),
            "missing outcome {want} in {outcomes:?}"
        );
    }
}

#[tokio::test]
async fn no_http_attempt_when_telemetry_url_unset() {
    // Empty telemetry_url → telemetry::fire returns early. Nothing to observe
    // externally; the completion path must still succeed with no panics, and a
    // closed port would have errored loudly if we'd tried it.
    let cfg = cfg_with_telemetry(None).await;
    let docs = open_documents().await;
    let res = handle_completion_request(
        cfg,
        docs,
        sidecar::index::Index::default(),
        req(),
        Debounce::default(),
    )
    .await
    .unwrap();
    assert!(res["text"].is_string(), "completion works with no telemetry sink");
}
