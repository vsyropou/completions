# Architecture

Three processes, two hops, one contract. The frozen wire shapes live in [protocol/README.md](../protocol/README.md) — this page is the map, that file is the law.

```
┌──────────────────────────────┐         ┌──────────────────────────┐         ┌───────────────────────────┐
│  VS Code extension (TS)      │         │  Rust sidecar            │         │  Phoenix backend (Elixir) │
│                              │         │                          │         │                           │
│  vscode-extension/src/       │         │  sidecar/src/            │         │  phoenix-backend/lib/     │
│                              │         │                          │         │                           │
│  ┌────────────────────────┐  │ stdin/  │  ┌────────────────────┐  │  HTTP   │  ┌───────────────────────┐│
│  │ extension.ts           │  │ stdout  │  │ rpc.rs    (JSON-   │  │  JSON   │  │ CompletionController  ││
│  │  • spawn sidecar       │─▶│headers  │─▶│           RPC)     │─▶│ +Bearer │─▶│   create/2            ││
│  │  • inline completion   │  │ +frame  │  │ documents.rs       │  │         │  └─────────┬───────────┘│
│  │    provider            │◀─│         │◀─│ context.rs (prompt)│◀─│         │            │            │
│  │  • document sync       │  │         │  │ index.rs  (repo)   │  │         │            ▼            │
│  └────────────────────────┘  │         │  │ syntax.rs (ts)     │  │         │  ┌───────────────────────┐│
│                              │         │  └────────────────────┘  │         │  │ CompletionGateway     ││
│  Output channel:             │         │                          │         │  │  • MockProvider       ││
│   "Completion Sidecar"       │         │  stderr → logs           │         │  │  • FimAdapter (Finch) ││
└──────────────────────────────┘         └──────────────────────────┘         │  └─────────┬───────────┘│
                                                                              └────────────┼───────────┘
                                                                                           │ HTTP
                                                                                           ▼
                                                                              ┌─────────────────────────┐
                                                                              │ Inference server        │
                                                                              │ vLLM / SGLang (OpenAI-  │
                                                                              │ compatible) :8000       │
                                                                              └─────────────────────────┘
```

## Hop 1 — Extension ↔ Sidecar (JSON-RPC 2.0 over stdio)

LSP-style framing: `Content-Length: N\r\n\r\n<json>`. stdout is protocol-only; logs go to stderr.

Requests (response expected):

- `initialize` — handshake; sends `workspace_root`, `phoenix_url`, `auth_token`, `max_tokens`, `measure_latency`. Returns `capabilities`.
- `completion/request` — ask for a completion. Params: `document_uri`, `document_version`, `position {line, character}` (UTF-16, zero-based), `max_tokens`. Returns `{text, document_version}` or a JSON-RPC error (`-32000` unknown document, `-32001` stale version, `-32002` upstream error, `-32003` cancelled).
- `shutdown` — graceful exit; sidecar responds then exits 0.

Notifications (no response):

- `textDocument/didOpen` — file opened: `uri`, `language`, `version`, full `text`.
- `textDocument/didChange` — full-document replacement: `uri`, `version`, full `text`.
- `textDocument/didClose` — evict: `uri`.
- `completion/cancel` — best-effort cancel: `request_id` equals the JSON-RPC `id` of the `completion/request` to drop.

## Hop 2 — Sidecar ↔ Phoenix (HTTP/JSON)

Single endpoint:

```
POST /api/v1/completions
Content-Type: application/json
Authorization: Bearer <token>
```

Body: `{request_id, language, file_path, prefix, suffix, repository_context, max_tokens}`. The sidecar splits the document at the UTF-16 cursor position, converts to UTF-8 byte offsets, fills `prefix` / `suffix`, and generates `request_id` (UUID v4).

Response (200): `{request_id, completion, finish_reason, model}`. Errors are uniform JSON `{error: {code, message}}` with status ∈ 400 / 401 / 413 / 422 / 502 / 504.

## Hop 3 — Phoenix ↔ Inference

`Complete.CompletionGateway` dispatches:

- `INFERENCE_BASE_URL` unset → `MockProvider` (deterministic string, no HTTP hop).
- Set → `FimAdapter` (Finch HTTP client) sends the FIM-templated request to the inference server and parses the response.

## Ground rules

- Positions are UTF-16, zero-based, everywhere on the wire. Only the sidecar converts to UTF-8 bytes for slicing.
- `document_version` is monotonically increasing per document. The extension discards any completion whose `document_version` doesn't match the editor's current version; the sidecar also rejects stale requests with `-32001`.
- Body limit on the HTTP hop is **256 KB**, enforced by Phoenix (`413 too_large`).
- Single shared bearer token (`AUTH_TOKEN` env on Phoenix; `completion.authToken` in VS Code). No user DB in v1.
- Cancellation is best-effort at every hop; the extension always ignores late responses.

Full field-by-field contract, error table, and timeout budgets: [protocol/README.md](../protocol/README.md) (frozen v1.0 — changes go through the lead with a version bump).
