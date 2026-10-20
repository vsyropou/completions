//! JSON-RPC framing round-trip and decoder rejection tests.

use sidecar::rpc::{read_message, write_message};
use tokio::io::BufReader;

#[tokio::test]
async fn framing_round_trip_two_messages() {
    let m1 = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
    let m2 = serde_json::json!({"jsonrpc":"2.0","method":"shutdown"});
    let mut buf: Vec<u8> = Vec::new();
    write_message(&mut buf, &m1).await.unwrap();
    write_message(&mut buf, &m2).await.unwrap();

    let mut br = BufReader::new(&buf[..]);
    let r1 = read_message(&mut br).await.unwrap().unwrap();
    let r2 = read_message(&mut br).await.unwrap().unwrap();
    let r3 = read_message(&mut br).await.unwrap();

    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&r1).unwrap(),
        m1
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&r2).unwrap(),
        m2
    );
    assert!(r3.is_none());
}

#[tokio::test]
async fn missing_content_length_errors() {
    let wire = b"Content-Type: application/json\r\n\r\n{}";
    let mut br = BufReader::new(&wire[..]);
    let err = read_message(&mut br).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("Content-Length"));
}

#[tokio::test]
async fn negative_or_unparsable_length_errors() {
    let wire = b"Content-Length: -1\r\n\r\n{}";
    let mut br = BufReader::new(&wire[..]);
    assert!(read_message(&mut br).await.is_err());

    let wire = b"Content-Length: abc\r\n\r\n{}";
    let mut br = BufReader::new(&wire[..]);
    assert!(read_message(&mut br).await.is_err());
}

#[tokio::test]
async fn oversized_content_length_errors_before_reading_body() {
    // 1 byte over the 256 KB cap; body never sent. The decoder must fail
    // on the header, not block waiting for bytes.
    let wire = b"Content-Length: 262145\r\n\r\n";
    let mut br = BufReader::new(&wire[..]);
    let err = read_message(&mut br).await.unwrap_err();
    assert!(err.to_string().contains("exceeds"), "got: {err}");
}
