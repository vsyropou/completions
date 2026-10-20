# sidecar

Per-window Rust process that does the smart work of the completion pipeline. Spawned by the VS Code extension (`extension/`) over stdio, talks to the inference server directly, posts fire-and-forget telemetry to Phoenix (`telemetry/`).

## What it does

For each typed character in the editor:
1. Accepts `completion/request` over JSON-RPC.
2. Debounces per URI (200 ms, latest-wins).
3. Computes the cursor excerpt (syntax-aware → linewise fallback) + pulls related repo excerpts via BM25 ranking.
4. Builds a FIM prompt (template from Config + repo context + prefix/suffix).
5. POSTs it to `{model_url}/v1/completions` (OpenAI-compatible), or returns a static mock when `model_url` is unset.
6. Reports the event to Phoenix as fire-and-forget telemetry.
7. Returns the completion to the extension.

## Layout

```
src/
  main.rs        # tracing init, calls sidecar::run
  lib.rs         # dispatch loop, Config, shared state
  rpc.rs         # Content-Length framing, JsonRpcError
  documents.rs   # HashMap<Uri, Document>, version checks, edit history ring
  completion.rs  # completion/request handler + debounce
  syntax.rs      # tree-sitter Lang enum (14 languages) + NoParser
  index.rs       # workspace walk, symbol extraction per Lang, BM25 ranker
  context.rs     # cursor excerpt, identifier extraction, resolve_import
  inference.rs   # FimAdapter port: build_prompt, complete() HTTP call
  telemetry.rs   # fire-and-forget POST to Phoenix

tests/           # mirrors src/ — 68 integration tests
  {completion,context,dispatch,documents,index,inference,rpc,syntax,telemetry}.rs
```

Tests live exclusively under `tests/` (integration tests use the crate's `pub` surface only). No `#[cfg(test)]` in `src/` — keep it that way.

## Build and run

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo build
cargo test
cargo test --test inference         # one file
cargo test --test inference test_name
```

Run standalone — the binary sits silently on stdin. Drive it from `integration-tests/e2e.mjs` for a full request/response session.

## Config (from `initialize` JSON-RPC call)

| Param          | Type           | Default                                  | Used for                       |
|----------------|----------------|------------------------------------------|--------------------------------|
| `workspace_root` | string         | —                                        | Workspace URI; sets index scope |
| `phoenix_url`    | string         | —                                        | Telemetry sink URL             |
| `auth_token`     | string         | —                                        | Bearer to Phoenix              |
| `max_tokens`     | int            | 128                                      | Max completion tokens           |
| `measure_latency`| bool           | false                                    | Verbose stderr latency logs    |
| `model_url`      | string or null | —                                        | Inference base URL             |
| `model_api_key`  | string or null | —                                        | Bearer to inference            |
| `model`          | string or null | `"default"`                              | Model id sent to inference     |
| `fim_template`   | string or null | Starcoder chain                          | FIM prompt template            |

Anything else in `initialize.params` is ignored (forward-compat).

## Docs

- Dispatch / shared state: `lib.rs` header docs.
- JSON-RPC wire shape: `../../protocol/README.md` (frozen v1.1).
- Tree-sitter per-language node names: `index.rs::extract`.
