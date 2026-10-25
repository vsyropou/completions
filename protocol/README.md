# Completion Protocol — FROZEN v1

Contract version: **v1.3** (frozen 2026-10-25). No field renames, endpoint changes, or coordinate changes without lead approval and a version bump.

Wire format follows the [JSON-RPC 2.0 specification](https://www.jsonrpc.org/specification), with [Language Server Protocol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#headerPart) style framing (`Content-Length: N\r\n\r\n` + UTF-8 JSON body).

## 1. Overview

```
VS Code <--JSON-RPC 2.0 over stdio--> Rust sidecar --HTTP/JSON--> Inference
                                            \--HTTP/JSON--> Telemetry (optional)
```

- All logs go to **stderr** on the Rust process. stdout is protocol only.
- Positions are **zero-based** `{line, character}` where `character` is a
  **UTF-8 byte offset** within the line. The extension converts from VS
  Code's native UTF-16 before send.
- Document `version` is an integer, **monotonically increasing** per document.
- `request_id` for each request is a monotonically-increasing, sidecar-minted string (`"req-N"`, per-process). It is minted when `completion/request` passes validation and is echoed back in the response. All telemetry events for that request — sidecar-emitted (`requested`/`served`/`superseded`/`error`) and extension-emitted (`accepted`/`cancelled`, POSTed directly to the telemetry sink) — are correlated by this id.

---

## 2. TypeScript ↔ Rust (JSON-RPC 2.0 over stdio)

### 2.1 Framing

LSP-style headers, single `Content-Length`, blank line, UTF-8 JSON body:

```
Content-Length: 87\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"initialize",...}
```

**Cap:** each frame's decoded body is limited to **256 KB** on both sides; the sidecar's parser exits fatally on oversize or malformed headers ([`rpc.rs::read_message`](../sidecar/src/rpc.rs)), the extension's decoder matches ([`framing.ts`](../extension/src/framing.ts)).

### 2.2 Methods

Requests (need a response): `initialize`, `completion/request`, `shutdown`.
Notifications (no response): `textDocument/didOpen`, `textDocument/didChange`, `textDocument/didClose`, `completion/cancel`.

#### `initialize` — request

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "workspace_root": "file:///repo",
    "telemetry_url": "http://localhost:4000",
    "telemetry_api_key": "dev-token",
    "measure_latency": false,
    "model_url": "",
    "model_api_key": "",
    "model": "",
    "fim_template": "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>",
    "temperature": 0.0,
    "max_tokens": 32
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { "capabilities": { "languages": ["typescript", "tsx"...] } }
}
```

#### `textDocument/didOpen` — notification

```json
{
  "jsonrpc": "2.0",
  "method": "textDocument/didOpen",
  "params": {
    "uri": "file:///repo/src/main.ts",
    "language": "typescript",
    "version": 1,
    "text": "function hello() {\n}"
  }
}
```

#### `textDocument/didChange` — notification

Full-document replacement only in v1.

```json
{
  "jsonrpc": "2.0",
  "method": "textDocument/didChange",
  "params": {
    "uri": "file:///repo/src/main.ts",
    "version": 2,
    "text": "function hello() {\n  return 42;\n}"
  }
}
```

#### `textDocument/didClose` — notification

```json
{
  "jsonrpc": "2.0",
  "method": "textDocument/didClose",
  "params": { "uri": "file:///repo/src/main.ts" }
}
```

#### `completion/request` — request

```json
{
  "jsonrpc": "2.0",
  "id": 123,
  "method": "completion/request",
  "params": {
    "document_uri": "file:///repo/src/main.ts",
    "document_version": 42,
    "position": { "line": 10, "character": 8 }
  }
}
```

Success response:

```json
{
  "jsonrpc": "2.0",
  "id": 123,
  "result": {
    "text": "return result;\n",
    "document_version": 42,
    "request_id": "req-7"
  }
}
```

Error response (JSON-RPC standard error object):

```json
{
  "jsonrpc": "2.0",
  "id": 123,
  "error": {
    "code": -32001,
    "message": "document version mismatch: requested 42, current 43"
  }
}
```

Error codes:

| code | meaning |
|---|---|
| -32700 | parse error |
| -32601 | unknown method |
| -32602 | invalid params |
| -32000 | unknown document |
| -32001 | document version mismatch / stale request |
| -32002 | upstream (Phoenix) error |
| -32003 | cancelled |

#### `completion/cancel` — notification

```json
{
  "jsonrpc": "2.0",
  "method": "completion/cancel",
  "params": { "request_id": 123 }
}
```

The `request_id` equals the JSON-RPC `id` of the `completion/request` to cancel. Cancellation is best-effort: the extension **must** discard stale responses even if cancellation arrives late.

#### `shutdown` — request

```json
{ "jsonrpc": "2.0", "id": 999, "method": "shutdown" }
```

Response: `{ "jsonrpc": "2.0", "id": 999, "result": null }`. Rust awaits in-flight `completion/request` tasks (best-effort, 5 s cap) before responding, so a `shutdown` that races a completion doesn't silently drop it. Then exits with code 0. The extension then closes stdio and terminates the process if it does not exit within 2s.

---

## 3. Rust ↔ Phoenix (HTTP/JSON)

### 3.1 Telemetry endpoint

The telemetry hop is fire-and-forget event emission — Phoenix never sits in the completion path. The Rust sidecar and the VS Code extension each POST events independently; events for the same logical request are reconciled by `request_id`.

```
POST /api/v1/events
Content-Type: application/json
Authorization: Bearer <token>
```

Event body (all fields required; "outcome" ∈ `requested` | `served` | `superseded` | `accepted` | `cancelled` | `error`):

```json
{
  "request_id": "req-7",
  "language": "typescript",
  "file_path": "src/main.ts",
  "prompt": "<fim prompt or empty>",
  "response": "<completion or empty>",
  "latency_ms": 0,
  "provider": "mock",
  "model": "default",
  "finish_reason": "",
  "outcome": "requested"
}
```

- Emitted by the **sidecar**: `requested` (before the inference hop), `served` (on success), `superseded` (debounce loser), `error` (inference failure).
- Emitted by the **extension**, correlating `request_id` from the completion response: `accepted` (user inserted the ghost text — VS Code runs the InlineCompletionItem's `command`), `cancelled` (CancellationToken fired; uses the JSON-RPC id as `req-ext-N` because the sidecar request id hasn't arrived yet — orphan events, diagnostic only).
- Phoenix responds `202 {}`; validation failures get `400`. The sidecar drops failures silently.

### 3.2 Timeouts

- Rust sidecar telemetry POST: **5 s** total (fire-and-forget; failures are logged at `debug` and dropped).
- Rust → inference server: **10 s** connect, **30 s** total.

---

## 4. Coordinates and versions

- `position.line`: zero-based line.
- `position.character`: zero-based **UTF-8 byte offset** within the line. The extension is responsible for converting the editor's native position (UTF-16 code units in VS Code and JetBrains) into bytes before send. Any client that emits something else (code points, byte offsets from the doc start, etc.) must do the same.
- A completion response is valid only if `document_version` in the response equals the document's current version in the extension. Otherwise the extension discards it.

---

## 5. Authentication

- Phoenix validates the `Authorization: Bearer <token>` header against a token from config (`AUTH_TOKEN` env).
- MVP = a single shared token. No user database.

---

## 6. Cancellation semantics

- VS Code `CancellationToken` → extension sends `completion/cancel`.
- Rust drops the in-flight HTTP request (abort) and replies to the JSON-RPC request with error `-32003` if it hasn't replied yet.
- Phoenix best-effort cancels the upstream HTTP call when the client disconnects (Bandit/Finch handle disconnect). GPU work may continue; that is accepted for MVP.
- The extension ignores responses for cancelled IDs.

---

*End of frozen contract v1.3.*
