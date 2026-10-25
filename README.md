# code-completion

A learning exercise to `"Bring Your Own Model"` code completion.

This repo is for you if:

- You got addicted to cursor's ai completion harness capabilities.
- You really appriciate zed's `"bring your own model approach"` to their ide.
- You like to host models yourself.
- You want ai code completions when no internet connection is available.
- You value the `Retrieve` part in RAG much more than the Generation.
- You like to learn while building and breaking stuff.

Repo layout:

```
VS Code extension (TS) ──JSON-RPC 2.0/stdio──▶ Rust sidecar ──▶ Inference server
                                                       │
                                                       └──▶ Phoenix (telemetry sink, optional)
```

The Rust sidecar does the smart work — prompt assembly (FIM), repo context, cursor excerpt, BM25 ranking, and the HTTP call to the inference server. Phoenix is no longer in the response path; it receives fire-and-forget telemetry events and exposes them via a small read API.

├─ [`extension/`](./extension/) — VS Code extension in TypeScript  
├─ [`sidecar/`](./sidecar/) — Rust per-window process  
├─ [`telemetry/`](./telemetry/) — Phoenix backend (telemetry store + read endpoints)  
├─ [`protocol/README.md`](./protocol/README.md) — frozen v1.3 contract for extension↔sidecar  
├─ docs - [`arhitecture`](./docs/architecture.md) and [`index`](./docs/architecture.md) documentation

## Install and Run

```sh
npm install -g @vscode/vsce   # or: brew install vsce
```

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

The sidecar talks to any OpenAI-compatible server on `{model_url}/v1/completions`. The fastest zero-GPU way to bring one up is [`llama.cpp`](https://github.com/ggml-org/llama.cpp) — install it however you like, then use the bundled Makefile targets to run `llama-server`.

**1. Install `llama.cpp`.** Follow [their install docs](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md) — `brew install llama.cpp`, build from source, or grab a pre-built binary. You need `llama-server` on your `$PATH`.

**2. Pull + run with the Makefile.** Defaults host [`bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF`](https://huggingface.co/bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF) (Q4_K_M quantization, ~1 GB download on first pull) on `127.0.0.1:8000`:

```sh
make model-pull          # warm the HF cache; downloads once to ~/.cache/huggingface
make model               # start llama-server on port 8000 (Ctrl+C to stop)
make model-stop          # kill a backgrounded server
make model-logs          # confirm what's listening
```

**3. Ping the server** to make sure it's alive and answering prompts:

```sh
curl http://127.0.0.1:8000/v1/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"qwen2.5-coder-1.5b-instruct","prompt":"def fib(n):","max_tokens":32,"temperature":0}' | jq '.'
```

**4. Point VS Code at it.** In either user settings or in the installed app settings:

```json
"completion.modelUrl": "http://127.0.0.1:8000",
"completion.model": "qwen2.5-coder-1.5b-instruct"
```

`modelApiKey` can stay empty — `llama-server` accepts any bearer (or none). Reload the dev-host window.

**5. Swap the model.** Override the Makefile variables for a different repo/quantization/port:

```sh
make model HF_REPO=bartowski/Qwen2.5-Coder-7B-Instruct-GGUF \
           HF_FILE=Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf \
           MODEL_PORT=8001
```

## Phoenix telemetry

- `POST /api/v1/events` — sidecar and extension emit outcome-keyed events (`requested`/`served`/`superseded`/`accepted`/`cancelled`/`error`), correlated by `request_id`
- `GET /api/v1/events` — 50 newest events (Bearer auth)
- `GET /api/v1/stats` — `{count, avg_latency_ms, by_language, requests_non_cancelled, acceptance_rate}`
- LiveView UI (no auth, dev tool): `/events` (searchable log + per-request reconciliation) and `/stats` (count + acceptance cards)

Events persist in Postgres (`docker-compose.yml` at repo root — `docker compose up -d`, DB `telemetry`@localhost:5432, named volume keeps them across runs). Full-text search over prompt/response via a generated `tsvector` column.

## Learn more

- Each component has its own README: [`extension/README.md`](./extension/README.md), [`sidecar/README.md`](./sidecar/README.md), [`telemetry/README.md`](./telemetry/README.md).
- Full contract: [`protocol/README.md`](./protocol/README.md) (frozen v1.3).
- Architecture map with file-level diagram: [`docs/architecture.md`](./docs/architecture.md).
- Sidecar dispatch + request pipeline flowcharts: [`docs/sidecar-flow.md`](./docs/sidecar-flow.md).

## Acknowledgements

The completion architecture (FIM prompting, editor-side context collection) was inspired by [Zed](https://github.com/zed-industries/zed)'s edit-prediction code (GPL-3.0-or-later). No source was copied; implementations here are independent.
