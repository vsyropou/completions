# telemetry

Phoenix (Elixir) backend. Telemetry sink only — the sidecar posts fire-and-forget events here after each completion; this component serves 3 read endpoints over an in-memory ring buffer.

Not an inference proxy. The old `CompletionGateway` / `MockProvider` / `POST /api/v1/completions` are gone.

## Layout

```
lib/complete.ex                       # OTP app module
lib/complete/application.ex           # supervision tree (TelemetryStore, Endpoint)
lib/complete/telemetry_store.ex       # Agent, 500-event ring
lib/complete_web.ex                   # web macro module
lib/complete_web/endpoint.ex          # plug pipeline (auth, body parsers)
lib/complete_web/router.ex            # 3 routes
lib/complete_web/plugs/auth.ex        # Bearer token check
lib/complete_web/controllers/
  telemetry_controller.ex             # create/index/stats
  error_json.ex
test/                                 # ExUnit tests (store + controller)
```

## Endpoints

All behind Bearer auth (`AUTH_TOKEN` env, default `dev-token`).

- `POST /api/v1/telemetry/completions` — store one event. Returns `202 {}`.
- `GET  /api/v1/telemetry/completions` — `{"events": [ ... 50 newest ... ]}` newest-first, full bodies.
- `GET  /api/v1/telemetry/stats` — `{"count": N, "avg_latency_ms": X, "by_language": {...}, "by_provider": {...}}`.

Event body (written by `sidecar/src/telemetry.rs`):
```json
{
  "request_id":    "…",
  "language":      "…",
  "file_path":     "…",
  "prompt":        "<full prompt sent to inference>",
  "response":      "<full completion>",
  "latency_ms":    0,
  "provider":      "model|mock",
  "model":         "…",
  "finish_reason": "stop|length|error"
}
```

## Run

```sh
mix setup         # deps.get
mix phx.server    # :4000
mix test          # store+controller tests
```

## Storage

`Complete.TelemetryStore` is an Elixir `Agent` supervised under `Complete.Application`. Holds max 500 events in-memory (list, prepend, drop-oldest). Restart loses data — that's intentional for a learning project. `# ponytail:` swap for SQLite or Postgres when the cap proves small.

## Config

Only one env var matters: `AUTH_TOKEN` (default `dev-token`). No `INFERENCE_*` — inference parameters live in VS Code settings and flow into the sidecar's `Config`.
