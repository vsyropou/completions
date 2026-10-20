//! Integration tests for the FIM inference port (`src/inference.rs`).
//! Uses a tiny TCP listener to stub the upstream HTTP endpoint.

use sidecar::inference::{build_prompt, complete};
use sidecar::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One-shot HTTP server: accept one connection, read headers+body, reply, return the raw request.
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

async fn cfg_with_model(model_url: Option<String>, api_key: Option<String>) -> Config {
    let cfg = Config::default();
    {
        let mut g = cfg.inner.lock().await;
        g.model_url = model_url;
        g.model_api_key = api_key;
        g.model = Some("test-model".into());
        g.fim_template = None; // use default
    }
    cfg
}

#[test]
fn build_prompt_matches_elixir_layout() {
    // Manual expansion of Elixir build_prompt:
    //   header = "# Path: src/foo.ts\n"
    //   repo_block = "# Related symbols:\nctx\n"
    //   template = "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>"
    //   result = header <> repo_block <> render(template)
    let got = build_prompt(
        "src/foo.ts",
        "const a = 1;",
        "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>",
        "let x = ",
        " + 1;",
    );
    let want = "# Path: src/foo.ts\n# Related symbols:\nconst a = 1;\n<|fim_prefix|>let x = <|fim_suffix|> + 1;<|fim_middle|>";
    assert_eq!(got, want);
}

#[test]
fn build_prompt_empty_repo_context_omits_block() {
    let got = build_prompt(
        "src/foo.ts",
        "",
        "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>",
        "pre",
        "post",
    );
    assert_eq!(
        got,
        "# Path: src/foo.ts\n<|fim_prefix|>pre<|fim_suffix|>post<|fim_middle|>"
    );
}

#[test]
fn build_prompt_template_substitution() {
    let got = build_prompt("f.ts", "", "PREFIX{prefix}SUFFIX{suffix}END", "AAA", "BBB");
    assert_eq!(got, "# Path: f.ts\nPREFIXAAASUFFIXBBBEND");
}

#[test]
fn build_prompt_default_template_used_when_empty() {
    let got = build_prompt("f.ts", "", "", "P", "S");
    assert_eq!(
        got,
        "# Path: f.ts\n<|fim_prefix|>P<|fim_suffix|>S<|fim_middle|>"
    );
}

#[tokio::test]
async fn mock_fallback_when_model_url_none() {
    let cfg = cfg_with_model(None, None).await;
    let (prompt, completion, finish_reason, model) =
        complete(&cfg, "typescript", "src/a.ts", "pre", "post", "ctx", "req-1")
            .await
            .unwrap();
    assert_eq!(
        completion,
        "// hello from mock backend\nfunction greet(name: string): string {\n  return `hello ${name}`;\n}\n"
    );
    assert_eq!(finish_reason, "stop");
    assert_eq!(model, "mock");
    assert!(prompt.contains("src/a.ts"));
}

#[tokio::test]
async fn bearer_header_present_when_api_key_set() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 50\r\nConnection: close\r\n\r\n{\"choices\":[{\"text\":\"ok\",\"finish_reason\":\"stop\"}]}";
    let server = tokio::spawn(serve_once(listener, reply));

    let cfg = cfg_with_model(Some(format!("http://{addr}")), Some("secret".into())).await;
    let (_, _, _, _) = complete(&cfg, "ts", "f.ts", "pre", "post", "", "req-2")
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(
        raw.to_lowercase().contains("authorization: bearer secret"),
        "got: {raw}"
    );
}

#[tokio::test]
async fn bearer_header_absent_when_no_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 50\r\nConnection: close\r\n\r\n{\"choices\":[{\"text\":\"ok\",\"finish_reason\":\"stop\"}]}";
    let server = tokio::spawn(serve_once(listener, reply));

    let cfg = cfg_with_model(Some(format!("http://{addr}")), None).await;
    let (_, _, _, _) = complete(&cfg, "ts", "f.ts", "pre", "post", "", "req-3")
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(
        !raw.to_lowercase().contains("authorization:"),
        "got: {raw}"
    );
}

#[tokio::test]
async fn finish_reason_mappings() {
    let cases = [
        ("stop", "stop"),
        ("length", "length"),
        ("other", "error"),
    ];
    for (given, want) in cases {
        let json = format!(
            r#"{{"choices":[{{"text":"x","finish_reason":"{given}"}}]}}"#
        );
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            json.len(),
            json
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Leak the reply string so it lives for 'static
        let reply_static: &'static str = Box::leak(reply.into_boxed_str());
        let server = tokio::spawn(serve_once(listener, reply_static));

        let cfg = cfg_with_model(Some(format!("http://{addr}")), None).await;
        let (_, _, finish, _) = complete(&cfg, "ts", "f.ts", "pre", "post", "", "req-4")
            .await
            .unwrap();
        assert_eq!(finish, want, "given={given}");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn missing_finish_reason_defaults_to_stop() {
    let json = r#"{"choices":[{"text":"x"}]}"#.to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(),
        json
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reply_static: &'static str = Box::leak(reply.into_boxed_str());
    let server = tokio::spawn(serve_once(listener, reply_static));

    let cfg = cfg_with_model(Some(format!("http://{addr}")), None).await;
    let (_, _, finish, _) = complete(&cfg, "ts", "f.ts", "pre", "post", "", "req-5")
        .await
        .unwrap();
    assert_eq!(finish, "stop");
    server.await.unwrap();
}

#[tokio::test]
async fn http_500_maps_to_upstream_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reply = "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: 11\r\nConnection: close\r\n\r\nserver boom";
    let server = tokio::spawn(serve_once(listener, reply));

    let cfg = cfg_with_model(Some(format!("http://{addr}")), None).await;
    let err = complete(&cfg, "ts", "f.ts", "pre", "post", "", "req-6")
        .await
        .unwrap_err();
    assert_eq!(err.code, -32002);
    server.await.unwrap();
}
