//! Sidecar library — JSON-RPC 2.0 over stdio server driving completions.
//!
//! Spawned as a child process by the VS Code extension. Reads LSP-style framed
//! JSON-RPC from stdin, writes responses to stdout, logs to stderr, and calls
//! the inference backend over HTTP. See `../protocol/README.md` for the frozen
//! wire contract.
//!
//! Layout: this file is only module wiring + the read/dispatch loop. Message
//! handlers live in [`dispatch`], shared mutable state in [`state`], wire
//! framing in [`rpc`]. Domains: [`documents`], [`syntax`], [`index`],
//! [`context`], [`completion`], [`inference`], [`telemetry`].

pub mod completion;
pub mod context;
pub mod dispatch;
pub mod documents;
pub mod index;
pub mod inference;
pub mod rpc;
pub mod state;
pub mod syntax;
pub mod telemetry;

use std::sync::Arc;

use tokio::io::BufReader;
use tokio::sync::Mutex;

use documents::Documents;
use rpc::{read_message, RequestId};

// Re-exports: pre-split public surface — call sites use `crate::Config` etc.
pub use state::{repo_relative_str, Config, Inner, Pending, PendingState};

/// Run the JSON-RPC loop until `shutdown` or stdin EOF. Entry point called from `main`.
pub async fn run() {
    let config = Config::default();
    let documents: Documents = Documents::default();
    let index = index::Index::default();
    let debounce = completion::Debounce::default();
    let pending = Arc::new(state::Pending {
        state: Mutex::new(state::PendingState::default()),
    });

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let writer: dispatch::Writer = Arc::new(Mutex::new(tokio::io::stdout()));

    loop {
        let msg = match read_message(&mut reader).await {
            Ok(Some(m)) => m,
            Ok(None) => {
                tracing::info!("stdin EOF, exiting");
                return;
            }
            Err(e) => {
                // Match the TS decoder: malformed framing is fatal. Log and exit
                // non-zero instead of absorbing garbage and desynchronizing the stream.
                tracing::error!(%e, "framing read error, exiting");
                std::process::exit(1);
            }
        };

        let val: serde_json::Value = match serde_json::from_slice(&msg) {
            Ok(v) => v,
            Err(_) => {
                dispatch::on_parse_error(&writer).await;
                continue;
            }
        };

        let Some(method) = val
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_owned)
        else {
            continue;
        };

        let id = val.get("id").and_then(|i| {
            if i.is_i64() {
                i.as_i64().map(RequestId::Num)
            } else if i.is_string() {
                i.as_str().map(|s| RequestId::Str(s.to_owned()))
            } else {
                None
            }
        });

        match (method.as_str(), id) {
            ("initialize", Some(id)) => {
                dispatch::on_initialize(&config, &val, &index, id, &writer).await;
            }
            ("shutdown", Some(id)) => {
                dispatch::on_shutdown(&pending, id, &writer).await;
                return;
            }
            ("completion/request", Some(id)) => {
                dispatch::on_completion_request(
                    config.clone(),
                    documents.clone(),
                    index.clone(),
                    val,
                    debounce.clone(),
                    pending.clone(),
                    id,
                    writer.clone(),
                )
                .await;
            }
            ("completion/cancel", None) => {
                dispatch::on_cancel(&pending, &val, &writer).await;
            }
            ("textDocument/didOpen", None) => {
                dispatch::on_did_open(&documents, &index, &config, &val).await;
            }
            ("textDocument/didChange", None) => {
                dispatch::on_did_change(&documents, &index, &config, &val).await;
            }
            ("textDocument/didClose", None) => {
                dispatch::on_did_close(&documents, &val).await;
            }
            (_, Some(id)) => {
                dispatch::on_unknown(&writer, id, &method).await;
            }
            (_, None) => {}
        }
    }
}
