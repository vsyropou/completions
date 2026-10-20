# code-completion

Low-latency multiline code completion in VS Code. Three processes, split roles:

```
VS Code extension (TS) ──JSON-RPC 2.0/stdio──▶ Rust sidecar ──▶ Inference server
                                                       │
                                                       └──▶ Phoenix (telemetry sink, optional)
```

The Rust sidecar does the smart work — prompt assembly (FIM), repo context, cursor excerpt, BM25 ranking, and the HTTP call to the inference server. Phoenix is no longer in the response path; it receives fire-and-forget telemetry events and exposes them via a small read API.

Top-level layout:

- [`extension/`](./extension/) — VS Code extension in TypeScript
- [`sidecar/`](./sidecar/) — Rust per-window process
- [`telemetry/`](./telemetry/) — Phoenix backend (telemetry store + read endpoints)
- [`sidecar/tests/`](./sidecar/tests/) — integration tests, mirrors [`sidecar/src/`](./sidecar/src/)
- [`protocol/README.md`](./protocol/README.md) — frozen v1.2 contract for extension↔sidecar
- [`docs/architecture.md`](./docs/architecture.md) — architecture map
- [`integration-tests/e2e.mjs`](./integration-tests/e2e.mjs) — smoke-test that drives the full pipeline

## Install

Each component has its own toolchain. Elixir/Erlang pinned via [`telemetry/.tool-versions`](./telemetry/.tool-versions) (erlang 28.2, elixir 1.18.4-otp-28) — install via asdf/mise first.

```sh
cd telemetry && mix setup                                       # deps.get
cd ../sidecar && cargo build                                    # debug binary at target/debug/sidecar
export PATH="$HOME/.cargo/bin:$PATH"                            # /usr/local/bin/cargo on this machine is broken
cd ../extension && npm install && npm run compile               # tsc → out/
```

### Packaged install via Makefile

The top-level [`Makefile`](./Makefile) builds the release sidecar, compiles the extension, stages everything, and installs the `.vsix` into VS Code:

```sh
make compile        # release sidecar + extension compile, stages extension/bin/sidecar
make package        # produce extension/completion-<version>.vsix via vsce
make install        # code --install-extension extension/completion-<version>.vsix
make uninstall      # code --uninstall-extension vasilis.completion
make clean          # drop staged artifacts
```

`make install` is the equivalent of "ship it" for a single-machine setup; `make package` alone gives you a `.vsix` you can hand to someone else's VS Code (`Extensions: Install from VSIX…`).

## Host a local model with llama.cpp

The sidecar talks to any OpenAI-compatible server on `{model_url}/v1/completions`. The fastest zero-GPU way to bring one up is `llama.cpp`.

**1. Install `llama.cpp`** (macOS and Linux via brew):

```sh
brew install llama.cpp
```

Or build from source (see [`llama.cpp` README](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md)):

```sh
git clone https://github.com/ggerganov/llama.cpp && cd llama.cpp
cmake -B build && cmake --build build --config Release
```

**2. Run the bundled Makefile targets.** Defaults host `Qwen2.5-Coder-1.5B-Instruct-GGUF` (bartowski repack, Q4_K_M quantization, ~1 GB download on first pull) on `127.0.0.1:8000`:

```sh
make model-pull          # warm the HF cache; downloads once to ~/.cache/huggingface
make model               # start llama-server on port 8000 (Ctrl+C to stop)
make model-stop          # kill the running server
make model-logs          # confirm what's listening
```

**3. Point VS Code at it.** In user settings, set:

```json
"completion.modelUrl": "http://127.0.0.1:8000",
"completion.model": "qwen2.5-coder-1.5b-instruct"
```

`modelApiKey` can stay empty — `llama-server` accepts any bearer (or none). Reload the dev-host window.

**4. Watch the telemetry pick it up.** With Phoenix running (`cd telemetry && mix phx.server`), `curl http://localhost:4000/api/v1/stats` should flip events' `"provider"` from `"mock"` → `"model"`.

**Swap the model.** Override the Makefile variables for a different repo/quantization/port:

```sh
make model HF_REPO=bartowski/Qwen2.5-Coder-7B-Instruct-GGUF \
           HF_FILE=Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf \
           MODEL_PORT=8001
```

Cache location: HF default (`~/.cache/huggingface`) unless you export `LLAMA_CACHE`. Auth-token-protected HF repos need `HF_TOKEN` exported before `make model-pull`.

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

1. **You type.** VS Code fires `provideInlineCompletionItems` on our provider in [`extension/src/extension.ts`](./extension/src/extension.ts). The extension forwards the doc state to the sidecar via `didOpen`/`didChange`/`didClose` and sends a `completion/request` over stdio JSON-RPC.
2. **Framing.** LSP-style headers (`Content-Length: N\r\n\r\n` + JSON body) — see [`extension/src/framing.ts`](./extension/src/framing.ts) on the writer side and [`sidecar/src/rpc.rs`](./sidecar/src/rpc.rs) on the reader side.
3. **Dispatch.** [`sidecar/src/lib.rs::run`](./sidecar/src/lib.rs) reads frames, dispatches on `method` — `initialize`, document notifications, `completion/request`, `completion/cancel`, `shutdown`. Document state lives in [`documents.rs`](./sidecar/src/documents.rs); syntax/OPL extraction in [`syntax.rs`](./sidecar/src/syntax.rs) (tree-sitter, 14 languages) + [`index.rs`](./sidecar/src/index.rs); prompt-shape in [`context.rs`](./sidecar/src/context.rs) (cursor-excerpt + repo BM25).
4. **Inference.** [`sidecar/src/inference.rs`](./sidecar/src/inference.rs) builds the FIM prompt (template + repo context + prefix/suffix) and POSTs to `{model_url}/v1/completions` directly. Mock provider returns a static string when `model_url` is unset.
5. **Telemetry.** After the inference returns, [`sidecar/src/telemetry.rs`](./sidecar/src/telemetry.rs) fires a detached POST to `{telemetry_url}/api/v1/completions` with the full prompt+response+latency. Never blocks the editor.
6. **Back.** The sidecar maps the inference result into a JSON-RPC response `{text, document_version}` → extension wraps it as a `vscode.InlineCompletionItem` → VS Code renders ghost text.

## Phoenix telemetry read API

- `GET /api/v1/completions` — 50 newest events (Bearer auth)
- `GET /api/v1/completions/{id}` — (not implemented; use the list endpoint)
- `GET /api/v1/stats` — `{count, avg_latency_ms, by_language, by_provider}`

Events are stored in-memory, capped at 500, newest-first. Phoenix restart wipes them.

## Learn more

- Each component has its own README: [`extension/README.md`](./extension/README.md), [`sidecar/README.md`](./sidecar/README.md), [`telemetry/README.md`](./telemetry/README.md).
- Full contract: [`protocol/README.md`](./protocol/README.md) (frozen v1.2).
- Architecture map with file-level diagram: [`docs/architecture.md`](./docs/architecture.md).
