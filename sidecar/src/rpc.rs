//! JSON-RPC 2.0 framing and message shapes over stdio.
//!
//! Wire framing is LSP-style: a `Content-Length: N` header, blank line, then N
//! bytes of UTF-8 JSON. See `../protocol/README.md` §2.1 for the frozen spec.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// JSON-RPC request id. The spec allows number, string, or null.
/// `Null` here only for parse-error replies where we don't have an id yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Num(i64),
    Str(String),
    Null,
}

/// JSON-RPC error object. Codes follow `protocol/README.md` §2.2.
/// Optional `data` is serialized on the wire as the error object's `data` key
/// (JSON-RPC 2.0 §5.1); used here only for -32001 to carry the current version.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

fn err(code: i32, message: impl Into<String>) -> JsonRpcError {
    JsonRpcError {
        code,
        message: message.into(),
        data: None,
    }
}

impl JsonRpcError {
    /// -32700 invalid JSON on the wire.
    pub fn parse_error() -> Self {
        err(-32700, "parse error")
    }
    /// -32601 the method name isn't one we handle.
    pub fn method_not_found(m: &str) -> Self {
        err(-32601, format!("unknown method: {m}"))
    }
    /// -32602 the params object failed validation.
    pub fn invalid_params(m: impl Into<String>) -> Self {
        err(-32602, m)
    }
    /// -32000 the extension asked about a document we never saw open.
    pub fn unknown_document(uri: &str) -> Self {
        err(-32000, format!("unknown document: {uri}"))
    }
    /// -32001 client's `document_version` doesn't match our snapshot — stale request.
    /// `data` carries the sidecar's current version so the caller can resync.
    pub fn version_mismatch(want: i64, got: i64) -> Self {
        JsonRpcError {
            data: Some(serde_json::json!({ "current_version": got })),
            ..err(
                -32001,
                format!("document version mismatch: requested {want}, current {got}"),
            )
        }
    }
    /// -32002 the Phoenix HTTP hop failed.
    pub fn upstream(m: impl Into<String>) -> Self {
        err(-32002, m)
    }
    /// -32003 cancelled via `completion/cancel`.
    pub fn cancelled() -> Self {
        err(-32003, "cancelled")
    }
    /// Serialize to a full JSON-RPC response object.
    pub fn into_response(self, id: RequestId) -> serde_json::Value {
        let err = serde_json::to_value(self).expect("JsonRpcError serializes");
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": err,
        })
    }
}

/// Build a success `result` response object.
pub fn success_response(id: RequestId, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Same 256 KB ceiling Phoenix puts on the HTTP hop — a larger frame here is
/// always corruption, so we error out before allocating or reading the body.
const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Read one framed message. Returns `Ok(None)` on clean EOF (stdin closed),
/// `Err` on malformed framing. Body bytes are the raw JSON — caller parses.
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let line_trim = line.trim_end();
        if line_trim.is_empty() {
            break;
        }
        if let Some(rest) = line_trim.strip_prefix("Content-Length:") {
            let n = rest
                .trim()
                .parse::<usize>()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            if n > MAX_FRAME_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Content-Length {n} exceeds {MAX_FRAME_BYTES} byte cap"),
                ));
            }
            content_length = Some(n);
        }
        // ignore other headers
    }
    let n = match content_length {
        Some(n) => n,
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing Content-Length",
            ))
        }
    };
    let mut buf = vec![0u8; n];
    reader.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// Write one framed message. Serializes `msg`, prepends the `Content-Length`
/// header, writes both, then flushes so the extension sees it immediately.
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &serde_json::Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}
