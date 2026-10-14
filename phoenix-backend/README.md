# phoenix-backend

HTTP/JSON gateway between the Rust sidecar and an LLM inference provider.
Single endpoint: `POST /api/v1/completions`. The full wire contract is
frozen in [`../protocol/README.md`](../protocol/README.md); this README
covers the Elixir side.

## Where it sits

```
VS Code extension --(JSON-RPC/stdio)--> Rust sidecar --(HTTP/JSON)--> phoenix-backend --(HTTP)--> inference provider
```

Phoenix is the only place FIM prompt tokenization happens.

## HTTP surface

`POST /api/v1/completions`, `Content-Type: application/json`, `Authorization: Bearer <AUTH_TOKEN>`.
Body: see [protocol §3.1](../protocol/README.md).

Status codes (table verbatim from the frozen contract):

| status | code | when |
|---|---|---|
| 400 | `invalid_request` | malformed JSON / missing field |
| 401 | `unauthorized` | bad/missing bearer token |
| 413 | `too_large` | body over limit |
| 422 | `validation_failed` | schema valid JSON, semantically invalid |
| 502 | `upstream_error` | inference provider failed |
| 504 | `upstream_timeout` | inference provider timed out |

## Configuration (env vars)

All read in `config/runtime.exs`:

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `4000` | HTTP listen port |
| `AUTH_TOKEN` | _required in prod_ | Bearer token for the endpoint |
| `INFERENCE_BASE_URL` | _unset → mock mode_ | URL of the real inference provider |
| `INFERENCE_MODEL` | `Qwen/Qwen2.5-Coder-32B-Instruct` | Model name sent upstream |
| `INFERENCE_FIM_TEMPLATE` | `<\|fim_prefix\|>{prefix}<\|fim_suffix\|>{suffix}<\|fim_middle\|>` | FIM prompt template |
| `INFERENCE_MAX_TOKENS` | `512` | Per-request cap clamp |
| `INFERENCE_TEMPERATURE` | `0.0` | Sampling temperature |
| `INFERENCE_TIMEOUT_MS` | `25000` | Upstream HTTP timeout |

`PHX_SERVER`, `SECRET_KEY_BASE`, `PHX_HOST` (prod only) are standard Phoenix.

## Module map

- **`Complete.Application`** — OTP application. Boots the `Finch` HTTP
  client pool (used only by the real adapter) and `CompleteWeb.Endpoint`
  under a `:one_for_one` supervisor.
- **`Complete.CompletionGateway`** — Single entry point for completion
  inference. Hides provider selection from callers: controllers call
  `generate/1` and never pick a provider themselves.
- **`Complete.MockProvider`** — Phase-1 deterministic mock. Returns a
  stable fake completion (pure function of inputs) so dev and tests use
  predictable output without any upstream.
- **`Complete.FimAdapter`** — Real inference adapter. Builds the FIM
  prompt (header + repo-context block + template), POSTs to
  `{INFERENCE_BASE_URL}/v1/completions` via `Finch`, and maps failures
  (HTTP error, timeout, malformed body) onto the `finish_reason: "error"`
  contract. Never raises.
- **`CompleteWeb.Endpoint`** — Plug pipeline entry: `RequestId`,
  `Telemetry`, `Plug.Parsers` (JSON-only, 256 KB cap), then the router.
  Oversized or malformed bodies raise out of `Plug.Parsers` and land in
  `ErrorJSON`.
- **`CompleteWeb.Router`** — One `:api` pipeline (JSON content
  negotiation + auth plug) and one route:
  `POST /api/v1/completions` → `CompletionController.create`.
- **`CompleteWeb.Plugs.Auth`** — Bearer-token check against `AUTH_TOKEN`
  using `Plug.Crypto.secure_compare` (constant-time). Halts 401 with the
  protocol-shaped error body on missing or wrong token.
- **`CompleteWeb.CompletionController`** — Validates the request body
  (required string fields, `max_tokens` type and 1..1024 range), calls
  `Complete.CompletionGateway.generate/1`, and shapes 200 / 4xx / 502
  JSON responses.
- **`CompleteWeb.ErrorJSON`** — Phoenix error renderer. Maps
  `Plug.Parsers` failures to protocol codes (`too_large` for 413,
  `invalid_request` for 400) so parser-level rejects stay in the frozen
  error shape.

## Mock vs real

The decision point is one line in `Complete.CompletionGateway.provider/0`
(`lib/complete/completion_gateway.ex`):

```elixir
if Application.get_env(:complete, :inference) do
```

`config/runtime.exs` sets that config only when `INFERENCE_BASE_URL` is
in the environment, so unset env var → `MockProvider`; set →
`FimAdapter`. Nothing else branches.

## FIM template format

Default template (override via `INFERENCE_FIM_TEMPLATE`):

```
<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>
```

`{prefix}` and `{suffix}` are replaced literally. The full prompt sent
upstream includes a `# Path: <file_path>` header and an optional
`# Related symbols:` block for repository context — both prepended by
`Complete.FimAdapter.build_prompt/1` in
`lib/complete/fim_adapter.ex`. Change the layout there.

## How to run

```sh
mix deps.get
mix phx.server
mix test
```

Typical dev session:

```sh
AUTH_TOKEN=dev-token mix phx.server
curl -XPOST -H "Authorization: Bearer dev-token" -H "Content-Type: application/json" \
  -d '{"request_id":"1","language":"typescript","file_path":"a.ts","prefix":"",\
       "suffix":"","repository_context":"","max_tokens":10}' \
  http://localhost:4000/api/v1/completions
```

## Extending

- **New validation rule** — add a `validate_*` clause in
  `CompleteWeb.CompletionController` (`lib/complete_web/controllers/completion_controller.ex`)
  returning `{:error, http_status, code, message}`, and thread it
  through the `with` in `create/2`.
- **Change upstream error mapping** — edit the `render/2` clauses in
  `CompleteWeb.ErrorJSON` (`lib/complete_web/controllers/error_json.ex`).
  Non-2xx responses and network failures are normalized to
  `finish_reason: "error"` inside `Complete.FimAdapter`; only the HTTP
  status mapping lives in `ErrorJSON` / the controller.
