# code-completion

Low-latency multiline code completion in VS Code, backed by a custom pipeline. A Rust **sidecar** process per editor window keeps file state and index in memory, builds the prompt, and posts it to a **Phoenix** backend, which calls a FIM model (or a deterministic mock when no GPU is attached) and returns a completion that VS Code renders as ghost text.

```
VS Code extension ──JSON-RPC 2.0 over stdio──▶ Rust sidecar ──HTTP/JSON──▶ Phoenix ──HTTP──▶ Inference (vLLM / SGLang)
        ◀──────────── ghost text ────────────             ◀──────── completion ──────
```

Architecture details: [docs/architecture.md](./docs/architecture.md). Wire contract: [protocol/README.md](./protocol/README.md) (frozen v1.0).

## Quickstart (mock mode, no GPU)

Three terminals, repos sibling-cloned under `code-completion/`.

### 1. Phoenix backend

```sh
cd phoenix-backend
mix deps.get
mix phx.server
```

Runs on `http://localhost:4000`. With `INFERENCE_BASE_URL` unset, it serves deterministic mock completions. Dev default auth token is `dev-token`.

### 2. Rust sidecar

```sh
cd sidecar
cargo build
```

Binary lands at `sidecar/target/debug/sidecar`.

### 3. VS Code extension

```sh
cd vscode-extension
npm install
npm run compile
```

Point the extension at the debug binary — open VS Code settings (JSON) and set:

```json
"completion.sidecarPath": "/absolute/path/to/code-completion/sidecar/target/debug/sidecar"
```

(In the Extension Development Host, use its own settings — `File → Preferences → Settings` while focused on the dev-host window.)

Then: open `vscode-extension/` in VS Code, press **F5**. An Extension Development Host launches. Inside it, open any `.ts` file, type e.g. `function add(a, b) {` and pause — a multiline ghost text completion appears.

## Verify mock end-to-end

With Phoenix running and the sidecar built:

```sh
node integration-tests/e2e.mjs
```

Expected output ends with:

```
initialize -> {"capabilities":...}
completion -> {"text":"...","document_version":2}
stale version correctly rejected with -32001
unknown document correctly rejected with -32000
emoji-position completion ok -> ...
shutdown -> null
sidecar exited cleanly
E2E PASS
```

## Surfacing code changes

| You edited | To pick it up |
|---|---|
| Rust sidecar (`sidecar/src/*.rs`) | `cargo build`, then in the dev-host window run **Developer: Reload Window** — the extension respawns the sidecar from the new binary |
| Phoenix (`phoenix-backend/lib/**/*.ex`) | Nothing — `code_reloader: true` in dev, next HTTP request compiles + runs the new code |
| Extension (`vscode-extension/src/*.ts`) | `npm run compile`, then reload the Extension Development Host window (`Cmd+R` in that window, or restart the F5 launch) |

## Real FIM inference (optional, needs a GPU host)

Serve the model with an OpenAI-compatible endpoint. vLLM:

```sh
python -m vllm.entrypoints.openai.api_server \
  --model Qwen/Qwen2.5-Coder-32B-Instruct \
  --port 8000
```

Or SGLang:

```sh
python -m sglang.launch_server \
  --model-path Qwen/Qwen2.5-Coder-32B-Instruct \
  --port 8000
```

Then restart Phoenix pointed at it:

```sh
export INFERENCE_BASE_URL=http://localhost:8000
# optional overrides:
# export INFERENCE_MODEL=Qwen/Qwen2.5-Coder-32B-Instruct
# export INFERENCE_FIM_TEMPLATE='<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>'
# export INFERENCE_MAX_TOKENS=512 INFERENCE_TEMPERATURE=0.0 INFERENCE_TIMEOUT_MS=25000
mix phx.server
```

`Complete.CompletionGateway` switches from `MockProvider` to `FimAdapter` based on `INFERENCE_BASE_URL` being set ([phoenix-backend/lib/complete/completion_gateway.ex](./phoenix-backend/lib/complete/completion_gateway.ex)).

## Watching latency

In the dev-host settings:

```json
"completion.measureLatency": true
```

Then **View → Output**, pick the **Completion Sidecar** channel, and watch for lines like:

```
[latency] total=42ms rust_total=40ms rust_http=38ms
```

For aggregate numbers over 50 requests (Phoenix + sidecar up, sidecar not attached to an editor):

```sh
node integration-tests/latency.mjs
# prints n, mean, p50, p95, p99 for total / rust_all / rust_http
```

## Debugging checklist

Work top-down; the first matching symptom usually names the broken hop.

1. **No ghost text at all.**
   - **View → Output → "Completion Sidecar"** — see `sidecar binary not found: …`? Fix `completion.sidecarPath`.
   - See `sidecar spawn error` or `sidecar exited: code=…`? Run the same binary by hand to see stderr.
   - Nothing in Output at all? Extension didn't activate — open a `.ts` file (activation is `onLanguage:typescript` / `typescriptreact`).
2. **`initialize failed: …` in Output.** Extension can't reach the sidecar. Check the binary runs standalone: `sidecar/target/debug/sidecar` should sit silently waiting on stdin (`Ctrl+C` to kill).
3. **HTTP 401 / `unauthorized` in Phoenix log.** Token mismatch. Extension setting `completion.authToken` must equal `AUTH_TOKEN` env (or fall back to the shared dev default `dev-token`).
4. **HTTP 502 `upstream_error` / 504 `upstream_timeout`.** Inference server down or unreachable. Confirm `curl $INFERENCE_BASE_URL/v1/models` responds; check `INFERENCE_BASE_URL` / `INFERENCE_MODEL` against what the GPU host actually exposes.
5. **Completions appear but feel stale / off-by-one.** Check `document_version` in sidecar trace logs against the editor: sidecar rejects mismatches with `-32001`. If the editor is at version 43 and the sidecar insists on 42, a `didChange` was dropped — reload the window and watch Output for `restarting sidecar once after 1s`.
6. **`-32000 unknown document`.** Sidecar hasn't seen the file. Make sure it's open in the editor and matches a language in `LANGUAGE_MAP` ([vscode-extension/src/extension.ts](./vscode-extension/src/extension.ts)).

## Project layout

```
code-completion/
├── protocol/                Frozen v1.0 wire contract (JSON-RPC + HTTP)
│   └── README.md            — do not edit; see lead for changes
├── vscode-extension/        TypeScript extension (F5 dev loop)
│   └── README.md            Extension-specific notes
├── sidecar/                 Rust context builder + JSON-RPC server
│   ├── src/                 completion, context, documents, index, rpc, syntax
│   └── README.md            Sidecar-specific notes
├── phoenix-backend/         Elixir gateway, auth, validation, FIM adapter
│   └── README.md            Backend-specific notes
├── integration-tests/       Node scripts (e2e, latency) driving the sidecar directly
└── docs/                    Longer-form documentation
```

Per-component READMEs:
- [vscode-extension/README.md](./vscode-extension/README.md)
- [sidecar/README.md](./sidecar/README.md)
- [phoenix-backend/README.md](./phoenix-backend/README.md)

## The contract is frozen

[protocol/README.md](./protocol/README.md) is **v1.0, frozen**. Don't edit it. Shape changes (fields, methods, semantics) go through the lead and require a version bump that every component adopts together.
