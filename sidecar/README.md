# sidecar

Per-window Rust process that does the smart work of the completion pipeline. Spawned by the VS Code extension in [`../extension/`](../extension/) over stdio, talks to the inference server directly, posts fire-and-forget telemetry to Phoenix in [`../telemetry/`](../telemetry/).

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
  main.rs            # tracing init, calls sidecar::run
  lib.rs             # frame loop: reads stdin, maps methods to dispatch handlers
  dispatch.rs        # one handler per JSON-RPC method, cancellation race
  state.rs           # Config + Pending (shared state across handlers)
  rpc.rs             # Content-Length framing, JsonRpcError
  documents.rs       # HashMap<Uri, Document>, version checks
  completion.rs      # completion/request handler + debounce
  syntax.rs          # tree-sitter Lang enum (14 languages) + NoParser
  index/             # workspace walk, per-language extractors, BM25 ranker
    mod.rs           #   storage + bm25_rank
    extract.rs       #   per-language tree-sitter extractors
  context/           # cursor excerpt, repo-context assembly, import resolution
    mod.rs           #   public surface + selection logic
    fmt.rs           #   symbol-line formatting
    resolve.rs       #   per-language import specifier → path
  inference.rs       # FimAdapter port: build_prompt, complete() HTTP call
  telemetry.rs       # fire-and-forget POST to Phoenix

tests/               # mirrors src/ — integration tests only, no #[cfg(test)] in src/
docs/                # deep-dives per module — see Docs section below
```

Tests live exclusively under [`tests/`](./tests/) (integration tests use the crate's `pub` surface only). No `#[cfg(test)]` in [`src/`](./src/) — keep it that way.

## Build and run

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo build
cargo test
cargo test --test inference         # one file
cargo test --test inference test_name
```

Run standalone — the binary sits silently on stdin. Drive it from [`integration-tests/e2e.mjs`](../integration-tests/e2e.mjs) for a full request/response session.

### Run it by hand (no editor)

The sidecar speaks LSP-style framing (a `Content-Length` header, blank line, JSON body) over stdin/stdout. You can drive it from a shell via `printf` (heredocs sneak in trailing newlines that break the frame parser):

```sh
{ printf 'Content-Length: 234\r\n\r\n'
  printf '%s' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspace_root":"file:///tmp","model_url":"http://127.0.0.1:8000","model":"qwen2.5-coder-1.5b-instruct","telemetry_url":"http://localhost:4000","telemetry_api_key":"dev-token"}}'
  printf 'Content-Length: 149\r\n\r\n'
  printf '%s' '{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"uri":"file:///tmp/demo.py","language":"python","version":1,"text":"def helper():\n    "}}'
  printf 'Content-Length: 159\r\n\r\n'
  printf '%s' '{"jsonrpc":"2.0","id":2,"method":"completion/request","params":{"document_uri":"file:///tmp/demo.py","document_version":1,"position":{"line":1,"character":4}}}'
  sleep 5                                                              # give the 200 ms debounce + inference time to finish before shutdown
  printf 'Content-Length: 44\r\n\r\n'
  printf '%s' '{"jsonrpc":"2.0","id":3,"method":"shutdown"}'
} | ./target/debug/sidecar
```

Local `llama-server` on CPU takes 1–3 s per request. The `sleep 5` keeps pace; drop it too low and `shutdown` arrives while inference is in flight (the sidecar now waits up to 5 s for in-flight completions before exiting, but the README example should still give the round trip headroom).

**Read the output.** stdout carries only framed protocol (the tracing logs go to stderr, so `2>/dev/null` cleans up). The framed stream is a sequence of `Content-Length: N\r\n\r\n<json>` blocks glued back-to-back with no separator. Two inline recipes, no scripts needed:

```sh
# Pretty-print every frame — awk splits records at each Content-Length header, jq formats each JSON body.
...your printf chain... | ./target/debug/sidecar 2>/dev/null \
  | awk 'NF' RS='Content-Length: [0-9]+\r\n\r\n' \
  | jq .

# Extract just the AI's text (the .result.text of the id=2 response), unescaped into real newlines.
...your printf chain... | ./target/debug/sidecar 2>/dev/null \
  | awk 'NF' RS='Content-Length: [0-9]+\r\n\r\n' \
  | jq -r 'select(.id == 2) | .result.text'
```

Each `Content-Length` value is the byte length of the JSON body that follows. Compute one with `python3 -c 'import json; s=json.dumps(msg, separators=(",",":")); print(len(s))'` (compact separators — extra whitespace counts as body bytes). The sidecar emits the responses framed the same way; logs go to stderr (via `tracing` in [`src/main.rs`](./src/main.rs)), protocol bytes to stdout. The sibling [`integration-tests/e2e.mjs`](../integration-tests/e2e.mjs) does this plumbing for you — it's the easier path.

## Config (from `initialize` JSON-RPC call)

| Param          | Type           | Default                                  | Used for                       |
|----------------|----------------|------------------------------------------|--------------------------------|
| `workspace_root` | string         | —                                        | Workspace URI; sets index scope |
| `telemetry_url`  | string         | —                                        | Telemetry sink URL             |
| `telemetry_api_key`| string         | —                                      | Bearer to telemetry sink       |
| `measure_latency`| bool           | false                                    | Verbose stderr latency logs    |
| `model_url`      | string or null | —                                        | Inference base URL             |
| `model_api_key`  | string or null | —                                        | Bearer to inference            |
| `model`          | string or null | `"default"`                              | Model id sent to inference     |
| `fim_template`   | string or null | Starcoder chain                          | FIM prompt template            |
| `temperature`    | number or null | 0.0                                      | Sampling temperature           |
| `max_tokens`     | number or null | 32                                       | Output token cap               |

Anything else in `initialize.params` is ignored (forward-compat).

## Docs

Module deep-dives (algorithm, public API, mermaid flowchart):

- [`docs/completion.md`](./docs/completion.md) — `completion/request` end-to-end: UTF-16→UTF-8 offsets, debounce, error-code precedence.
- [`docs/context.md`](./docs/context.md) — cursor excerpt (syntax + linewise fallback) and `repository_context` assembly, per-language import resolution.
- [`docs/index.md`](./docs/index.md) — workspace index storage, tokenizer, BM25 ranker, extractor dispatch.
- [`docs/inference.md`](./docs/inference.md) — FIM prompt layout, mock short-circuit, OpenAI-compatible HTTP hop.
- [`docs/dispatch.md`](./docs/dispatch.md) — per-method handlers, the cancellation race, shutdown drain.

Cross-cutting references:

- JSON-RPC wire shape: [`../protocol/README.md`](../protocol/README.md) (frozen v1.2).
- End-to-end request pipeline flowchart: [`../docs/sidecar-flow.md`](../docs/sidecar-flow.md).
- Overall project shape: [`../docs/architecture.md`](../docs/architecture.md).
