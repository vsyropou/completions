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

/// JSON-RPC response envelope. Either `result` or `error` is set, never both.
#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object. Codes follow `protocol/README.md` §2.2.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    /// -32700 invalid JSON on the wire.
    pub fn parse_error() -> Self {
        Self {
            code: -32700,
            message: "parse error".into(),
        }
    }
    /// -32601 the method name isn't one we handle.
    pub fn method_not_found(m: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unknown method: {m}"),
        }
    }
    /// -32602 the params object failed validation.
    pub fn invalid_params(m: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: m.into(),
        }
    }
    /// -32000 the extension asked about a document we never saw open.
    pub fn unknown_document(uri: &str) -> Self {
        Self {
            code: -32000,
            message: format!("unknown document: {uri}"),
        }
    }
    /// -32001 client's `document_version` doesn't match our snapshot — stale request.
    pub fn version_mismatch(want: i64, got: i64) -> Self {
        Self {
            code: -32001,
            message: format!("document version mismatch: requested {want}, current {got}"),
        }
    }
    /// -32002 the Phoenix HTTP hop failed.
    pub fn upstream(m: impl Into<String>) -> Self {
        Self {
            code: -32002,
            message: m.into(),
        }
    }
    /// -32003 cancelled via `completion/cancel`.
    pub fn cancelled() -> Self {
        Self {
            code: -32003,
            message: "cancelled".into(),
        }
    }
    /// Serialize to a full JSON-RPC response object.
    pub fn into_response(self, id: RequestId) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": self.code, "message": self.message }
        })
    }
}

/// Build a success `result` response object.
pub fn success_response(id: RequestId, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
