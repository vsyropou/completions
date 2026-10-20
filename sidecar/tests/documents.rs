//! `Documents` didOpen/didChange/didClose acceptance rules and edit-history ring.

use sidecar::documents::{DidChangeError, Documents};

async fn open(docs: &Documents, uri: &str, text: &str, version: i64) {
    docs.did_open(&serde_json::json!({
        "uri": uri, "text": text, "version": version, "language": "typescript"
    }))
    .await;
}

async fn change(docs: &Documents, uri: &str, text: &str, version: i64) {
    docs.did_change(&serde_json::json!({ "uri": uri, "text": text, "version": version }))
        .await
        .expect("did_change should succeed");
}

async fn change_result(
    docs: &Documents,
    uri: &str,
    text: &str,
    version: i64,
) -> Result<(), DidChangeError> {
    docs.did_change(&serde_json::json!({ "uri": uri, "text": text, "version": version }))
        .await
}

#[tokio::test]
async fn version_must_be_exactly_next() {
    let docs = Documents::default();
    open(&docs, "u", "a", 3).await;
    change(&docs, "u", "b", 4).await;
    assert_eq!(docs.get("u").await.unwrap().text, "b");

    // equal: rejected, document untouched
    let Err(DidChangeError::Stale { .. }) = change_result(&docs, "u", "c", 4).await else {
        panic!("duplicate version must be rejected");
    };
    assert_eq!(docs.get("u").await.unwrap().text, "b");
    assert_eq!(docs.get("u").await.unwrap().version, 4);

    // regression: rejected, document untouched
    assert!(
        change_result(&docs, "u", "d", 2).await.is_err(),
        "regression must be rejected"
    );
    assert_eq!(docs.get("u").await.unwrap().text, "b");

    // skip-ahead: rejected (protocol §4: exactly current+1)
    assert!(
        change_result(&docs, "u", "e", 7).await.is_err(),
        "skip-ahead must be rejected"
    );
    assert_eq!(docs.get("u").await.unwrap().text, "b");

    // next in sequence: accepted
    change(&docs, "u", "e", 5).await;
    assert_eq!(docs.get("u").await.unwrap().text, "e");
    assert_eq!(docs.get("u").await.unwrap().version, 5);
}

#[tokio::test]
async fn version_regression_resets_nothing() {
    let docs = Documents::default();
    open(&docs, "u", "a", 1).await;
    change(&docs, "u", "b", 2).await;
    let edits_before = docs.recent_edits("u").await;
    assert!(
        change_result(&docs, "u", "z", 1).await.is_err(),
        "regression must fail"
    );
    let doc = docs.get("u").await.unwrap();
    assert_eq!(doc.text, "b");
    assert_eq!(doc.version, 2);
    assert_eq!(docs.recent_edits("u").await.len(), edits_before.len());
}

#[tokio::test]
async fn did_change_for_unopened_doc_is_rejected() {
    let docs = Documents::default();
    let Err(DidChangeError::Unopened { .. }) = change_result(&docs, "u", "x", 1).await else {
        panic!("didChange for unopened doc must be rejected");
    };
    assert!(docs.get("u").await.is_none());
}

#[tokio::test]
async fn edit_history_ring_records_up_to_five() {
    let docs = Documents::default();
    open(&docs, "u", "hello world", 1).await;
    assert!(docs.recent_edits("u").await.is_empty());

    for v in 2..=8 {
        change(&docs, "u", &format!("hello world v{v}"), v).await;
    }
    let edits = docs.recent_edits("u").await;
    // Ring cap is 5 — mirrors EDIT_HISTORY_LEN in src/documents.rs.
    assert_eq!(edits.len(), 5, "ring caps at five");
    // Oldest in ring is v4 (v2 and v3 evicted), newest is v8.
    assert_eq!(edits[0].version, 4);
    assert_eq!(edits[4].version, 8);
    // Last change was "v7" → "v8": only the digit differs, so preview is "8".
    assert!(edits[4].preview.contains('8'), "preview: {}", edits[4].preview);
    assert_eq!(edits[4].deleted_len, 1); // one digit replaced

    // didClose wipes the ring.
    docs.did_close(&serde_json::json!({ "uri": "u" })).await;
    assert!(docs.recent_edits("u").await.is_empty());
}

#[tokio::test]
async fn edit_history_offsets_and_preview() {
    let docs = Documents::default();
    open(&docs, "u", "fn main() {}", 1).await;
    change(&docs, "u", "fn main() { return 1; }", 2).await;
    let edits = docs.recent_edits("u").await;
    assert_eq!(edits.len(), 1);
    let e = &edits[0];
    // Common prefix "fn main() {" is 11 bytes; insertion happens at offset 11.
    assert_eq!(e.offset, 11);
    assert!(e.preview.contains("return 1;"), "preview: {}", e.preview);
}
