//! Binary entry point. Configures tracing to stderr (stdout is reserved for the
//! JSON-RPC protocol stream — see `../protocol/README.md`) and hands off to `sidecar::run`.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    sidecar::run().await;
}
