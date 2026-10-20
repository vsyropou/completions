# Architecture

Three processes, two hops, one frozen contract. The frozen wire shapes live in [protocol/README.md](../protocol/README.md) — this page is the map, that file is the law.

```
┌──────────────────────────────┐         ┌──────────────────────────┐         ┌────────────────────────┐
│  VS Code extension (TS)      │         │  Rust sidecar            │         │  Phoenix (Elixir)      │
│                              │         │                          │         │  (telemetry sink)      │
│  extension/src/              │         │  sidecar/src/            │         │  telemetry/lib/        │
│                              │         │                          │         │                        │
│  ┌────────────────────────┐  │ stdin/  │  ┌────────────────────┐  │         │  ┌──────────────────┐  │
│  │ extension.ts           │  │ stdout  │  │ rpc.rs    (JSON-   │  │         │  │ TelemetryStore   │  │
│  │  • spawn sidecar       │─▶│headers  │─▶│           RPC)     │─│────────▶│  │  (Agent, 500-cap │  │
│  │  • inline completion   │  │ +frame  │  │ documents.rs       │  │ teleme- │  │   in-memory)     │  │
│  │    provider            │◀─│         │◀─│ context.rs (excerpt│◀│ │ POST   │  └──────────────────┘  │
│  │  • document sync       │  │         │  │ index.rs  (BM25)   │  │ async   │          │             │
│  └────────────────────────┘  │         │  │ syntax.rs (ts)     │  │         │          │ read API    │
│                              │         │  │ inference.rs (FIM) │  │         │          ▼             │
│  Output channel:             │         │  │ telemetry.rs       │  │         │  GET /api/v1/*         │
│   "Completion Sidecar"       │         │  └────────────────────┘  │         └────────────────────────┘
└──────────────────────────────┘         └────────────┬─────────────┘
                                                      │
                                                      │ HTTP
                                                      ▼
                                          ┌─────────────────────────┐
                                          │ Inference server        │
                                          │ vLLM / Ollama / etc     │
                                          │ POST {model_url}/       │
                                          │   v1/completions        │
                                          └─────────────────────────┘
```

## Hop 1 — Extension ↔ Sidecar (JSON-RPC 2.0 over stdio)

LSP-style framing: `Content-Length: N\r\n\r\n<json>`. stdout is protocol-only; logs go to stderr.

Requests (response expected):

- `initialize` — handshake; sends `workspace_root`, `telemetry_url`, `telemetry_api_key`, `measure_latency`, plus inference config `model_url` / `model_api_key` / `model` / `fim_template` / `temperature` / `max_tokens`. Returns `capabilities`.
- `completion/request` — ask for a completion. Params: `document_uri`, `document_version`, `position {line, character}` (UTF-16, zero-based). Returns `{text, document_version}` or a JSON-RPC error (`-32000` unknown document, `-32001` stale version, `-32002` upstream error, `-32003` cancelled).
- `shutdown` — graceful exit; sidecar responds then exits 0.

Notifications (no response):

- `textDocument/didOpen` — file opened: `uri`, `language`, `version`, full `text`.
- `textDocument/didChange` — full-document replacement: `uri`, `version`, full `text`. Version must be `current + 1` or it is rejected silently.
- `textDocument/didClose` — evict: `uri`.
- `completion/cancel` — best-effort cancel: `request_id` equals the JSON-RPC `id` of the `completion/request` to drop.

## Hop 2 — Sidecar ↔ Inference (HTTP/JSON)

The sidecar calls the inference server directly — no Phoenix on this path.

```
POST {model_url}/v1/completions
Content-Type: application/json
Authorization: Bearer {model_api_key}      # only when configured
```

Body: `{model, prompt, max_tokens: <cfg>, temperature: <cfg>, stop: ["\n\n\n", "```", "<|fim_middle|>", "<|fim_suffix|>"]}`. The sidecar builds `prompt` per `fim_template` (defaults to the Starcoder chain `<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>`), prefixed with `# Path: <file_path>` plus an optional `# Related symbols:` block from [`index.rs`](../sidecar/src/index.rs) + [`context.rs`](../sidecar/src/context.rs). `temperature` defaults to `0.0` and `max_tokens` to `32` (VS Code settings `completion.temperature` / `completion.maxTokens`) — inline ghost text wants short, deterministic answers.

Response (200): OpenAI-compatible `{"choices":[{"text":…, "finish_reason":…}]}`. Errors map to JSON-RPC `-32002`.

When `model_url` is empty, the sidecar's mock path returns a static deterministic string (the `// hello from mock backend` marker).

## Hop 3 — Sidecar ↔ Phoenix (telemetry, asynchronous)

Fire-and-forget, never blocks the editor:

```
POST {telemetry_url}/api/v1/completions
Content-Type: application/json
Authorization: Bearer {telemetry_api_key}
```

Body: `{request_id, language, file_path, prompt, response, latency_ms, provider, model, finish_reason}`. Phoenix stores to an in-memory Agent (500-event cap) and replies `202` — the sidecar does not await.

## Ground rules

- Positions are UTF-16, zero-based, everywhere on the wire. Only the sidecar converts to UTF-8 bytes for slicing.
- `document_version` is monotonically increasing per document. `didChange` must send `current + 1` or the update is dropped (`warn` only — it's a notification).
- Stale versions on `completion/request` → `-32001` with `data.current_version`. Extension discards responses whose `document_version` doesn't match the current editor state.
- Cancellation: `-32003`, claimed atomically via one Mutex over pending+answered state — no double-send.
- Debounce: 200 ms per URI, latest-wins. Superseded requests return `-32003`.
- Frame cap: malformed Content-Length headers terminate the sidecar; 256 KB per-frame cap enforced on both sides.
- Bearer auth for both HTTP hops (`telemetry_api_key` for telemetry, `model_api_key` for inference).

Full field-by-field contract, error table, and timeouts: [protocol/README.md](../protocol/README.md) (frozen v1.2).
