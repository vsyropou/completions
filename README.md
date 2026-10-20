# code-completion-light

Low-latency multiline code completion in VS Code. Three processes, split roles:

```
VS Code extension (TS) ──JSON-RPC 2.0/stdio──▶ Rust sidecar ──▶ Inference server
                                                       │
                                                       └──▶ Phoenix (telemetry sink, optional)
```

The Rust sidecar does the smart work — prompt assembly (FIM), repo context, cursor excerpt, BM25 ranking, and the HTTP call to the inference server. Phoenix is no longer in the response path; it receives fire-and-forget telemetry events and exposes them via a small read API.

Top-level layout:

- `extension/` — VS Code extension in TypeScript
- `sidecar/` — Rust per-window process
- `telemetry/` — Phoenix backend (telemetry store + read endpoints)
- `sidecar/tests/` — integration tests, mirrors `sidecar/src/`
- `protocol/README.md` — frozen v1.1 contract for extension↔sidecar
- `docs/architecture.md` — architecture map
- `integration-tests/e2e.mjs` — smoke-test that drives the full pipeline

## Install

Each component has its own toolchain. Elixir/Erlang pinned via `telemetry/.tool-versions` (erlang 28.2, elixir 1.18.4-otp-28) — install via asdf/mise first.

```sh
cd telemetry && mix setup                                       # deps.get
cd ../sidecar && cargo build                                    # debug binary at target/debug/sidecar
export PATH="$HOME/.cargo/bin:$PATH"                            # /usr/local/bin/cargo on this machine is broken
cd ../extension && npm install && npm run compile               # tsc → out/
```

## Run

Three terminals, plus a VS Code dev-host window:

1. **Telemetry** — `cd telemetry && mix phx.server` (port 4000). Optional — the sidecar skips telemetry when Phoenix is down, so completions work either way.
2. **Inference** — point at your model server (vLLM, Ollama, etc) via VS Code settings, or leave `completion.modelUrl` empty to fall back to the mock string.
3. **Extension** — open `extension/` in VS Code, press F5 → Extension Development Host. In the dev-host, set `completion.sidecarPath` to `…/sidecar/target/debug/sidecar`. Open a `.ts`, `.py`, or any other `.rs`-supported file and type — ghost text appears.

**Headless smoke test** (with Phoenix up):
```sh
node integration-tests/e2e.mjs
```

## Follow the completion's journey

1. **You type.** VS Code fires `provideInlineCompletionItems` on our provider in `extension/src/extension.ts`. The extension forwards the doc state to the sidecar via `didOpen`/`didChange`/`didClose` and sends a `completion/request` over stdio JSON-RPC.
2. **Framing.** LSP-style headers (`Content-Length: N\r\n\r\n` + JSON body) — see `extension/src/framing.ts` on the writer side and `sidecar/src/rpc.rs` on the reader side.
3. **Dispatch.** `sidecar/src/lib.rs::run` reads frames, dispatches on `method` — `initialize`, document notifications, `completion/request`, `completion/cancel`, `shutdown`. Document state lives in `documents.rs`; syntax/OPL extraction in `syntax.rs` (tree-sitter, 14 languages) + `index.rs`; prompt-shape in `context.rs` (cursor-excerpt + repo BM25).
4. **Inference.** `sidecar/src/inference.rs` builds the FIM prompt (template + repo context + prefix/suffix) and POSTs to `{model_url}/v1/completions` directly. Mock provider returns a static string when `model_url` is unset.
5. **Telemetry.** After the inference returns, `sidecar/src/telemetry.rs` fires a detached POST to `{phoenix_url}/api/v1/telemetry/completions` with the full prompt+response+latency. Never blocks the editor.
6. **Back.** The sidecar maps the inference result into a JSON-RPC response `{text, document_version}` → extension wraps it as a `vscode.InlineCompletionItem` → VS Code renders ghost text.

## Phoenix telemetry read API

- `GET /api/v1/telemetry/completions` — 50 newest events (Bearer auth)
- `GET /api/v1/telemetry/completions/{id}` — (not implemented; use the list endpoint)
- `GET /api/v1/telemetry/stats` — `{count, avg_latency_ms, by_language, by_provider}`

Events are stored in-memory, capped at 500, newest-first. Phoenix restart wipes them.

## Learn more

- Each component has its own README (`extension/README.md`, `sidecar/README.md`, `telemetry/README.md`).
- Full contract: `protocol/README.md` (frozen v1.1).
- Architecture map with file-level diagram: `docs/architecture.md`.
