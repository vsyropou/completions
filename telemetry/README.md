# telemetry

Telemetry sink: the sidecar and the VS Code extension POST fire-and-forget events here; this component persists them in Postgres and serves read endpoints + a LiveView UI.

## Layout

```
lib/complete.ex                       # OTP app module
lib/complete/application.ex           # supervision tree (Complete.Repo, Endpoint)
lib/complete/repo.ex                  # Ecto Postgres repo
lib/complete/telemetry_event.ex       # Ecto schema (telemetry_events table)
lib/complete/telemetry.ex             # context: log/search/aggregates (acceptance rate, non-cancelled count)
lib/complete_web.ex                   # web macro module
lib/complete_web/endpoint.ex          # plug pipeline (+ LiveView socket)
lib/complete_web/router.ex            # /api routes + browser :telemetry live_session
lib/complete_web/plugs/auth.ex        # Bearer token check for /api/*
lib/complete_web/controllers/telemetry_controller.ex
lib/complete_web/live/event_index_live.ex    # /events — searchable raw log
lib/complete_web/live/stats_live.ex          # /stats  — count + acceptance cards
priv/repo/migrations/                 # telemetry_events table + generated tsvector col + GIN index
```

## Endpoints

The HTTP API is behind Bearer auth (`AUTH_TOKEN` env, default `dev-token`); the LiveView UI is plain browser GET, unauthenticated dev tool.

- `POST /api/v1/events` — store one event. `202 {}` / `400`.
- `GET  /api/v1/events` — `{"events": [ ... 50 newest ... ]}` newest-first.
- `GET  /api/v1/stats` — `{"count", "avg_latency_ms", "by_language", "requests_non_cancelled", "acceptance_rate"}`.
- `GET  /events` — LiveView: search/filter raw events, per-request_id reconciliation view.
- `GET  /stats` — LiveView: the two aggregation cards (non-cancelled count, accepted %) + by-language table.

Event body (emitted by both `sidecar/src/telemetry.rs` and `extension/src/telemetry.ts`):

```json
{
  "request_id": "req-42",
  "language": "typescript",
  "file_path": "src/index.ts",
  "prompt": "<full prompt or empty>",
  "response": "<full completion or empty>",
  "latency_ms": 0,
  "provider": "model|mock",
  "model": "…",
  "finish_reason": "stop|length|error|\"\"",
  "outcome": "requested|served|superseded|accepted|cancelled|error"
}
```

One logical request yields several events correlated by `request_id` — the sidecar emits `requested`/`served`/`superseded`/`error`; the extension (via VS Code's accept hook on the `InlineCompletionItem`'s command) emits `accepted` correlated to the sidecar's `request_id`, plus diagnostic `cancelled` events with an extension-local `req-ext-N` id.

## Run

```sh
docker compose up -d    # postgres:18 from the repo-root compose file
mix setup               # deps.get + ecto.create + ecto.migrate
mix phx.server          # :4000 — UI at /events and /stats
mix test                # sandbox tests (auto-create+migrate telemetry_test)
```

## Storage

`telemetry_events` in Postgres (`telemetry` db, complete/complete creds, localhost:5432 from the repo-root `docker-compose.yml`). A stored generated `tsvector` column over `prompt || ' ' || response` with a GIN index backs full-text search; the two metric queries are plain `COUNT DISTINCT request_id` aggregates. Persistence across restarts is the docker `postgres_data` named volume.

## Config

- `AUTH_TOKEN` (default `dev-token`) — Bearer auth for the HTTP API.
- `DATABASE_URL` (prod only) — overrides the dev-hardcoded `postgres://complete:complete@localhost/telemetry`.
- No `INFERENCE_*` — inference params live in VS Code settings and flow into the sidecar's `Config`.
