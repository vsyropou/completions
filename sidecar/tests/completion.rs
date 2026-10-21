//! `completion/request` handler end-to-end and UTF-16 → UTF-8 offset conversion.

use sidecar::completion::{
    cap_prefix, cap_suffix, handle_completion_request, utf16_to_utf8_offset, Debounce, PREFIX_CAP,
    SUFFIX_CAP,
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
