//! `completion/request` handler end-to-end.

use sidecar::completion::{
    cap_prefix, cap_suffix, handle_completion_request, Debounce, PREFIX_CAP, SUFFIX_CAP,
};
use sidecar::documents::Documents;
use sidecar::index::Index;
use sidecar::rpc::RequestId;
use sidecar::Config;

async fn mock_config() -> Config {
    let cfg = Config::default();
    {
        let mut g = cfg.inner.lock().await;
        g.workspace_root = "file:///repo".into();
        // max_tokens_default was removed in v1.2 — nothing to set.
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
            "position": { "line": 1, "character": 8 }
        }
    })
}

async fn open_documents() -> Documents {
    let docs = Documents::default();
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
async fn version_mismatch_returns_32001() {
    let cfg = mock_config().await;
    let docs = open_documents().await;
    let mut req = open_doc();
    req["params"]["document_version"] = serde_json::json!(99);
    let err = handle_completion_request(cfg, docs, Index::default(), req, Debounce::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, -32001);
    // JSON-RPC `error.data` carries the sidecar's current version so the
    // caller can resync.
    let resp = err.into_response(RequestId::Num(1));
    assert_eq!(resp["error"]["data"]["current_version"], 3);
}

#[tokio::test]
async fn unknown_document_returns_32000() {
    let cfg = mock_config().await;
    let docs = Documents::default();
    let err = handle_completion_request(cfg, docs, Index::default(), open_doc(), Debounce::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, -32000);
}

#[tokio::test]
async fn byte_offset_within_line_multibyte() {
    // Doc with an emoji on line 0. "ab😀cd" — emoji is 4 UTF-8 bytes at offset 2..6.
    // Wire contract: character is a UTF-8 byte offset within the line.
    let cfg = mock_config().await;
    let docs = Documents::default();
    docs.did_open(&serde_json::json!({
        "uri": "file:///repo/src/emoji.ts",
        "language": "typescript",
        "version": 1,
        "text": "ab😀cd\nl1"
    }))
    .await;
    let mut req = open_doc();
    req["params"] = serde_json::json!({
        "document_uri": "file:///repo/src/emoji.ts",
        "document_version": 1,
        // character: 6 → right after the emoji (bytes "ab😀").
        "position": { "line": 0, "character": 6 }
    });
    let out = handle_completion_request(cfg, docs, Index::default(), req, Debounce::default())
        .await;
    // We don't care about the completion text (mock) — only that the handler
    // didn't return invalid_params for a mid-emoji byte offset.
    assert!(out.is_ok(), "expected Ok(..), got: {out:?}");
}

#[tokio::test]
async fn byte_offset_out_of_range_returns_invalid_params() {
    // ASCII doc "abcd" on line 0 (4 bytes), character: 10 → out of range.
    let cfg = mock_config().await;
    let docs = Documents::default();
    docs.did_open(&serde_json::json!({
        "uri": "file:///repo/src/short.ts",
        "language": "typescript",
        "version": 1,
        "text": "abcd"
    }))
    .await;
    let mut req = open_doc();
    req["params"] = serde_json::json!({
        "document_uri": "file:///repo/src/short.ts",
        "document_version": 1,
        "position": { "line": 0, "character": 10 }
    });
    let err = handle_completion_request(cfg, docs, Index::default(), req, Debounce::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, -32602);
}

#[tokio::test]
async fn line_out_of_range_returns_invalid_params() {
    // Doc has 2 lines (0 and 1); ask for line 5.
    let cfg = mock_config().await;
    let docs = open_documents().await;
    let mut req = open_doc();
    req["params"]["position"] = serde_json::json!({ "line": 5, "character": 0 });
    let err = handle_completion_request(cfg, docs, Index::default(), req, Debounce::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, -32602);
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
