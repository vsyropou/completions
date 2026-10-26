# code-completion

A learning exercise to `"Bring Your Own Model"` code completion.

This repo is for you if:

- You got addicted to cursor's ai completion harness capabilities.
- You really appriciate zed's `"bring your own model approach"` to their ide.
- You like to host models yourself.
- You want ai code completions when no internet connection is available.
- You value the `Retrieve` part in RAG much more than the `Generation`.
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

## Installation

The app has several dependencies.

Vscode extension packaging.

```sh
npm install -g @vscode/vsce   # or: brew install vsce
```

Rust build and packaging [cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html).

```sh
curl https://sh.rustup.rs -sSf | sh
```

Elixir for the telemetry app you can quickly follow the official [guide](https://elixir-lang.org/install/) or use [asdf](https://asdf-vm.com/) to manage [multiple](https://elixir-lang.org/install/#version-managers) tool versions.

Once all the dependencies are installed. The top-level [`Makefile`](./Makefile) builds the release sidecar, compiles the extension, stages everything, and installs the `.vsix` into VS Code:

```sh
make install        # builds sidecar, extension, packages and installs the extension.
```

Cleaning up

```sh
make uninstall      # code --uninstall-extension vasilis.completion
make clean          # drop staged artifacts
```

## Bringing your model

If you reached this point you are now ready to plug in your model by either navigating to vscode extension settings or editing the user `settings.json` file.

```json
"completion.modelUrl": <model base url>,  e.g.: "http://127.0.0.1:8000"
"completion.model": <your model> e.g.:"qwen2.5-coder-1.5b-instruct"
```

### Host a huggingface model locally with llama.cpp

The sidecar talks to any OpenAI-compatible server on `{model_url}/v1/completions`. The fastest zero-GPU way to bring one up is [`llama.cpp`](https://github.com/ggml-org/llama.cpp), seed here for [installation](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md) then use the bundled Makefile targets to run `llama-server`.

**1. Pull a model**

```sh
make model-pull HF_REPO=<> HF_FILE=<>
```

Defaults to:

```sh
HF_REPO = bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF
HF_FILE = Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf
# Other model hosting configurations:
MODEL_PORT = 8000
LLAMA_CACHE = $(HOME)/.cache/huggingface
MODEL_LOG   = /tmp/llama-server-$(MODEL_PORT).log
```

**2. Serve the model**

```sh
make model               # start llama-server on port 8000 (Ctrl+C to stop)
```

See logs and stop

```sh
make model-logs          # confirm what's listening
make model-stop          # kill a backgrounded server
```

**3. Ping the server** to make sure it's alive and answering prompts:

```sh
curl http://127.0.0.1:8000/v1/completions \
  -H "Content-Type: application/json" \
  -d '{"prompt":"def fib(n):"}' | jq '.'
```

## Telemetry and Statistics (Optional)

Completion requests to sidecar and usage requests are tracked by the [telemetry app](./telemetry/README.md). You can start the service,

```sh
docker compose up -d    # postgres:18 from the repo-root compose file
make telemetry
```

Interact a bit with the completions in vscode and then navigate to [localhost:4000](http://localhost:4000/events) —

## Learn more

- Each component has its own README: [`extension/README.md`](./extension/README.md), [`sidecar/README.md`](./sidecar/README.md), [`telemetry/README.md`](./telemetry/README.md).
- Architecture map with file-level diagram: [`docs/architecture.md`](./docs/architecture.md).
- Sidecar dispatch + request pipeline flowcharts: [`docs/sidecar-flow.md`](./docs/sidecar-flow.md).

## Contributing

As already mentioned in the beginning this is a learning exercise that resulted in building the end to end flow of code completions. The interesting part is tweaking the index and context assembly to optimize for completions quality. This is where I will focus my efforts. Any comments, gotchas, critics or contributions are more than welcome.

## Acknowledgements

The completion architecture (FIM prompting, editor-side context collection) was inspired by [Zed](https://github.com/zed-industries/zed)'s edit-prediction code (GPL-3.0-or-later). No source was copied; implementations here are independent.
