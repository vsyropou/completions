# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Learning project: low-latency multiline code completion in VS Code, three processes:

```
VS Code extension (TS) ──JSON-RPC 2.0/stdio──▶ Rust sidecar ──▶ Inference server (vLLM/Ollama/etc)
                                                       │
                                                       └──▶ Phoenix (telemetry sink, optional)
```

The Rust sidecar is the smart one: it does prompt assembly (FIM templating), calls the inference server directly, and post-facto notifies Phoenix with a fire-and-forget telemetry event. Phoenix is no longer an inference proxy — it's a 500-event in-memory ring buffer with three read endpoints.

- `vscode-extension/` — activation on `*` (any language), spawns sidecar per window, JSON-RPC client over stdio.
- `sidecar/` — per-window process. `main.rs`/`rpc.rs`/`lib.rs` (dispatch loop) real; `documents`/`syntax`/`index`/`context` real (tree-sitter parses 14 languages, BM25 over repo files, cursor-excerpt expansion); `inference.rs` (FimAdapter ported from Elixir) real; `telemetry.rs` detached POST to Phoenix.
- `phoenix-backend/` — Phoenix is a telemetry sink. `TelemetryStore` (Agent, 500-event cap) + `POST /api/v1/telemetry/completions`, `GET /api/v1/telemetry/completions`, `GET /api/v1/telemetry/stats`. Old `CompletionGateway` / `MockProvider` deleted.
- `protocol/README.md` — **frozen v1.1 wire contract** for the extension↔sidecar hop. Phoenix-side HTTP contract is internal; not in the frozen spec.

## Hard rules in the contract

- Positions on the wire are **UTF-16 code units, zero-based**. Only the sidecar converts to UTF-8 bytes for slicing.
- `document_version` is monotonic per document; sidecar rejects stale with `-32001` (carrying `data.current_version`), extension discards mismatched completions.
- `didChange` is **full-document replacement** only; `didChange` version must equal `current + 1` or the update is swallowed (`warn`, no error response — it's a notification).
- Sidecar stdout is protocol-only; all logs go to stderr (`tracing`). Framing is LSP-style `Content-Length` headers — malformed frames are fatal, 256 KB cap enforced.
- Cancellation: `completion/cancel` uses `AbortHandle` + shared `answered` HashSet under one Mutex. Cancel claims the answer slot and writes `-32003` immediately; the aborted task's claim-on-finish fails so nothing double-sends.
- Debounce: `completion.rs` applies a per-URI 200 ms debounce, latest-wins; superseded requests return `-32003`.

## Commands

Three sibling components, no top-level build. Run from each dir.

**Phoenix** (`phoenix-backend/`, Elixir pinned via `.tool-versions`: erlang 28.2, elixir 1.18.4-otp-28):
```sh
mix setup
mix phx.server        # :4000, code_reloader on
mix test              # telemetry store + controller tests
```

**Sidecar** (`sidecar/`):
```sh
export PATH="$HOME/.cargo/bin:$PATH"   # /usr/local/bin/cargo is broken x86
cargo build
cargo test                             # tests live in tests/ mirroring src/ — 68 integration tests
cargo test --test inference            # run one file
cargo test --test inference test_name  # one test
```
Debug binary at `sidecar/target/debug/sidecar`; it sits silently on stdin when run alone.

**Extension** (`vscode-extension/`):
```sh
npm install
npm run compile                        # tsc → out/
```
Manual loop: F5 in `vscode-extension/` window → dev-host. Set `completion.sidecarPath` in the **dev-host's** settings to the sidecar debug binary.

**End-to-end smoke test** (needs Phoenix on :4000 + sidecar built):
```sh
node integration-tests/e2e.mjs         # asserts static mock round-trips through full chain
# bypasses Phoenix entirely if Phoenix is down — telemetry is non-blocking
```

## Configuration flow (4 settings that matter now)

New-ish. When a request flows through these, they go:

```
package.json contributes ──▶ extension.ts activate() ──▶ initialize.params ──▶ Config ──▶ inference.rs HTTP body
```

| VS Code setting        | initialize param  | Sidecar field   | Used for                              |
|------------------------|-------------------|-----------------|---------------------------------------|
| `completion.modelUrl`     | `model_url`         | `model_url`       | Inference base URL; absent → mock       |
| `completion.modelApiKey`  | `model_api_key`     | `model_api_key`   | Bearer header, iff set                  |
| `completion.model`        | `model`             | `model`           | Model id sent to inference; `default`   |
| `completion.fimTemplate`  | `fim_template`      | `fim_template`    | FIM string with `{prefix}`/`{suffix}`   |

To try a real model, set `modelUrl` + `model` (+ `modelApiKey` if your server needs auth), reload the dev-host window, then watch `GET /api/v1/telemetry/stats` flip `"provider":"mock"` → `"provider":"model"`.

## Multi-language support

`Lang` in `syntax.rs` covers 14 grammar-backed languages (TS/TSX/JS/JSX/Python/Rust/Go/Java/C/Cpp/CSharp/Elixir/Json/Yaml) + `NoParser` fallback. `activationEvents: ["*"]` in package.json. Grammar crates add deps to `sidecar/Cargo.toml` — pulling in another language means a new `Lang` variant, grammar dep, extractor in `index.rs::extract`, and (if the language has them) an import resolver in `context.rs::resolve_import`.

## Test layout (recently relocated — don't re-inline)

All Rust tests live in `sidecar/tests/`, one file per src module:
```
src/{completion,context,documents,index,lib,rpc,syntax,inference,telemetry}.rs
   ↓
tests/{completion,context,documents,dispatch,index,rpc,syntax,inference,telemetry}.rs
```
`tests/dispatch.rs` holds `src/lib.rs`'s tests (the name `lib.rs` would shadow the crate). Shared helpers inline at the top of each test file. New tests go into a matching `tests/<module>.rs` — **never** `#[cfg(test)] mod tests` in `src/`.

## Phoenix telemetry API quick reference

- `POST /api/v1/telemetry/completions` — Bearer auth, `{request_id, language, file_path, prompt, response, latency_ms, provider, model, finish_reason}` → `202`.
- `GET /api/v1/telemetry/completions` — 50 newest, newest-first, full bodies.
- `GET /api/v1/telemetry/stats` — `{count, avg_latency_ms, by_language}`.
- Storage: in-memory, capped 500 events, dies on restart. `# ponytail:` swap for SQLite when the cap proves small.

## Env vars (Phoenix)

`AUTH_TOKEN` only. Inference config moved into the sidecar's Config (from VS Code settings). `INFERENCE_*` env vars are gone.

## Debugging orientation

- Extension: "Completion Sidecar" output channel.
- Sidecar: `tracing` to stderr (visible in extension's output channel).
- Phoenix: endpoint logs to terminal where `mix phx.server` runs.
- Trace a request: mock string is the greppable marker `// hello from mock backend`. Real-inference requests show up in Phoenix's telemetry ring buffer immediately.
- If sidecar won't start, check the machine's cargo (broken Homebrew binary at `/usr/local/bin/cargo` — use `~/.cargo/bin/cargo`).
