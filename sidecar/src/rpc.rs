use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Num(i64),
    Str(String),
    Null,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    pub fn parse_error() -> Self {
        Self {
            code: -32700,
            message: "parse error".into(),
        }
    }
    pub fn method_not_found(m: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unknown method: {m}"),
        }
    }
    pub fn invalid_params(m: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: m.into(),
        }
    }
    pub fn unknown_document(uri: &str) -> Self {
        Self {
            code: -32000,
            message: format!("unknown document: {uri}"),
        }
    }
    pub fn version_mismatch(want: i64, got: i64) -> Self {
        Self {
            code: -32001,
            message: format!("document version mismatch: requested {want}, current {got}"),
        }
    }
    pub fn upstream(m: impl Into<String>) -> Self {
        Self {
            code: -32002,
            message: m.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self {
            code: -32003,
            message: "cancelled".into(),
        }
    }
    pub fn into_response(self, id: RequestId) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": self.code, "message": self.message }
        })
    }
}

pub fn success_response(id: RequestId, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

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
