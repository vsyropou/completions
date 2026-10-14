# Sidecar

A Rust subprocess that the VS Code extension spawns to bridge the editor and
the Phoenix completion backend.

```
+----------------+  JSON-RPC 2.0   +----------+   HTTP/JSON   +-----------+
| VS Code        |  over stdio     | Sidecar  |  (POST /api/  |  Phoenix  |
| extension (TS) | <-------------> | (Rust)   | ------------> |  (Elixir) |
+----------------+                 +----------+  /v1/completions) +--------+
                                       |
                                       +--> stderr: tracing logs
```

stdin is protocol-in, stdout is protocol-out, stderr is logs. Never write
anything else to stdout.

## Process lifecycle

1. The VS Code extension spawns the sidecar binary when the workspace opens.
2. The sidecar runs one async loop reading LSP-style framed JSON-RPC from
   stdin (`Content-Length: N\r\n\r\n{...}`).
3. `initialize` stores `phoenix_url` / `auth_token` / `workspace_root` and
   kicks off background workspace indexing. The loop then handles requests
   and notifications.
4. `shutdown` replies `null` and the process exits with code 0. The extension
   kills the process if it hasn't exited within 2 s.

## Module map

- **`src/main.rs`** — binary entry. Sets the tracing subscriber to stderr and
  calls `sidecar::run()`. Nothing else lives here.
- **`src/lib.rs`** — the JSON-RPC dispatch loop (`run`), the shared `Config`
  / `Inner` state, and the in-flight-task tracker used by
  `completion/cancel`. Owns the protocol routing table. Start here.
- **`src/rpc.rs`** — framing: `read_message` / `write_message` (Content-Length
  header + JSON body), the `RequestId` enum, the `Response` envelope, and
  `JsonRpcError` with the seven error codes from the protocol.
- **`src/documents.rs`** — `Documents` hashmap: uri → `{text, version, language}`
  for every open file. Drops stale `didChange` notifications where
  `new_version <= current_version`.
- **`src/completion.rs`** — the `completion/request` handler. Parses params,
  validates version, converts UTF-16 cursor to a UTF-8 byte offset, caps
  prefix/suffix on char boundaries, POSTs to Phoenix, returns the completion.
- **`src/syntax.rs`** — tree-sitter glue. `Lang` enum maps
  language-id / extension → TS or TSX grammar; `parse()` gives back a `Tree`.
- **`src/index.rs`** — workspace-wide symbol index. `build_index` walks
  `.ts`/`.tsx` files and fills two maps: path → top-level `Symbol`s and
  path → import specifiers. Refreshed per file on `didOpen` / `didChange`.
- **`src/context.rs`** — builds the `repository_context` string sent to
  Phoenix: same-file symbols (minus the function/class you're typing in)
  plus symbols transitively imported from other files. Capped at
  `REPO_CTX_CAP` bytes.

## Input: JSON-RPC from the extension

Seven messages, all defined in `../protocol/README.md` §2.2. Requests (need
a reply): `initialize`, `completion/request`, `shutdown`. Notifications:
`textDocument/didOpen`, `textDocument/didChange`, `textDocument/didClose`,
`completion/cancel`.

## Output: HTTP to Phoenix

`POST {phoenix_url}/api/v1/completions` with body
`{request_id, language, file_path, prefix, suffix, repository_context, max_tokens}`.
Header: `Authorization: Bearer <auth_token>` from `initialize`.
Timeouts: 10 s connect, 30 s total (frozen in `Config::default`).

## Versioning and cancellation

The extension monotonically bumps `version` on each edit and tags every
`completion/request` with the version it saw. If the request's
`document_version` doesn't match what we have by the time we get to it, we
return `-32001` and the extension discards the reply (its local version is
already newer anyway).

`completion/cancel {request_id}` aborts the in-flight HTTP work via a Tokio
`AbortHandle` and writes a `-32003` response. One `Mutex` covers both the
abort handle and an `answered` set so we can never emit both a `-32003` and
a success reply for the same id.

## Where to change things

- **Context selection** (what goes into `repository_context`): `src/context.rs`,
  specifically `build_repository_context`.
- **Ranking / which files are candidates**: `src/index.rs`, `build_index` and
  the maps inside `Index` — that's the set `context.rs` draws from.
- **Prefix/suffix sizing**: `PREFIX_CAP` / `SUFFIX_CAP` in `src/completion.rs`.

## Build & test locally

```sh
cargo build            # debug build
cargo test             # 20 unit + integration tests
cargo doc --open       # read the rustdoc in a browser
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

New here? Read in this order: `src/lib.rs` (the loop) → `src/completion.rs`
(one request end-to-end) → `src/rpc.rs` (framing) → `src/documents.rs`
(state) → `src/context.rs` + `src/index.rs` (context construction) →
`src/syntax.rs` (tree-sitter basics).

## Wire contract

The JSON-RPC and HTTP shapes are frozen in `../protocol/README.md`. Don't
restate them here; read and link.
