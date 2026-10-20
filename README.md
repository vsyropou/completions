# code-completion — learning skeleton

A three-process code-completion pipeline you can read end-to-end. Skeleton state: plumbing is real, intelligence is placeholder. Typing in VS Code triggers a request that flows Extension → Rust sidecar → Phoenix → static mock response → ghost text in the editor.

```
VS Code extension (TS) ──JSON-RPC 2.0/stdio──▶ Rust sidecar ──HTTP/JSON──▶ Phoenix (Elixir) ──▶ static mock
```

Full contract (the "real" system you'll grow toward): [protocol/README.md](./protocol/README.md), [docs/architecture.md](./docs/architecture.md).

## Install

Three sibling dirs, each with its own toolchain. Elixir/Erlang versions pinned in `phoenix-backend/.tool-versions` (erlang 28.2, elixir 1.18.4-otp-28) — install via asdf/mise first.

```sh
cd phoenix-backend && mix setup                          # deps.get
cd ../sidecar && cargo build                             # debug binary
cd ../vscode-extension && npm install && npm run compile # compile TS
```

Gotcha — if `cargo` fails with "Bad CPU type in executable": `/usr/local/bin/cargo` is stale. Use rustup's instead:
```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

## Run

Three terminals, plus a VS Code dev-host window:

1. **Phoenix** — `cd phoenix-backend && mix phx.server` (port 4000, code-reloader on).
2. **Sidecar** — nothing to start by hand; the VS Code extension spawns one per window.
3. **Extension** — open `vscode-extension/` in VS Code, press **F5** to launch an Extension Development Host. In its settings set `completion.sidecarPath` to the absolute path of `sidecar/target/debug/sidecar`. In the dev-host, open a `.ts` file and type — you should see a static ghost-text suggestion appear.
4. **Headless smoke check** — with Phoenix running and the sidecar built:
   ```sh
   node integration-tests/e2e.mjs
   ```
   Drives the sidecar over stdio without an editor and asserts the mock response round-trips.

## Follow the completion's journey

The mock returns one fixed string: `// hello from mock backend` followed by a small `greet` function. Grep that string to find every hop:

1. **You type a `.ts` snippet.** VS Code fires `provideInlineCompletionItems` on our provider → `vscode-extension/src/extension.ts`. The extension forwards the document body to the sidecar via `didOpen` / `didChange` notifications, then sends a `completion/request`.
2. **Framing.** The request isn't a plain string — it's `Content-Length: N\r\n\r\n` + a JSON-RPC 2.0 envelope over stdin/stdout. See `vscode-extension/src/framing.ts` on the writer side and `sidecar/src/rpc.rs` on the reader side.
3. **Dispatch.** `sidecar/src/main.rs` → `lib.rs::run` reads frames, dispatches on `method` — `initialize`, `didOpen`/`didChange`/`didClose` (document store in `documents.rs`), `completion/request` (entry into `completion.rs`), `completion/cancel`, `shutdown`. Everything the dispatch does on top of that is `// placeholder:` — this is where tree-sitter parsing, repo indexing, prompt building, and version checks will live.
4. **HTTP hop.** `completion.rs` builds the body (`prefix`, `suffix`, `repository_context`, `max_tokens`, `request_id`) and posts to `http://localhost:4000/api/v1/completions` with `Authorization: Bearer dev-token`. Auth must match Phoenix's `AUTH_TOKEN` env.
5. **Phoenix.** `lib/complete_web/router.ex` → `CompletionController.create/2` (auth plug → validation → `CompletionGateway.generate/1` → `MockProvider.generate/1`). `MockProvider` returns the static string. Watch the Phoenix console — you'll see the request logged.
6. **Back the same way.** Phoenix responds `{request_id, completion, finish_reason, model}` → sidecar parses, maps to JSON-RPC result `{text, document_version}` → extension inserts the `text` as a `vscode.InlineCompletionItem` and VS Code renders it as ghost text.

Things to look at while learning:
- `vscode-extension/src/extension.ts` — single file, the whole VS Code side.
- `sidecar/src/rpc.rs` — how to read `Content-Length`-framed JSON-RPC off stdin in Rust.
- `sidecar/src/lib.rs` — the dispatch loop that ties rpc/documents/completion together.
- `phoenix-backend/lib/complete_web/controllers/completion_controller.ex` — clean Elixir `with`-chain for validation.

## Where the placeholders live

Every place a real implementation will go is marked with a `// placeholder:` (Rust/TS) or `@moduledoc placeholder:` (Elixir) comment explaining intent. Protocol and architecture docs describe the frozen/complete contract — the skeleton is a readable subset of it.
