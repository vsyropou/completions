# `handle_completion_request` — end-to-end flow

One JSON-RPC `completion/request` in, one `{text, document_version}` reply out.
Lives in [`../src/completion/mod.rs`](../src/completion/mod.rs). Error codes
are protocol-mandated and ordered; see
[`../../protocol/README.md`](../../protocol/README.md) §2.2.

```mermaid
flowchart TD
    Start([handle_completion_request]) --> PP[parse_params: serde_json::from_value]
    PP -->|bad params| E262["-32602 invalid params"]

    PP --> GetDoc["documents.get(uri)"]
    GetDoc -->|unknown| E000["-32000 unknown document"]

    GetDoc --> Ver{"doc.version == p.document_version?"}
    Ver -->|no| E001["-32001 version mismatch<br/>data.current_version"]

    Ver -->|yes| Bump["debounce.bump(uri) → gen"]
    Bump --> Sleep["sleep DEBOUNCE_MS 200ms"]
    Sleep --> Cur{"debounce.is_current(uri, gen)?"}
    Cur -->|superseded| E003["-32003 cancelled"]
    Cur -->|yes| ReDoc["documents.get(uri) re-check"]
    ReDoc -->|version moved| E003
    ReDoc -->|still matched| Offset["byte_offset(text, position)"]

    Offset -->|line / character<br/>out of range| E262
    Offset --> Repo["Repo::build<br/>cap_prefix 6000<br/>cap_suffix 1000<br/>context::repository_context"]

    Repo --> Resp["id / language / file_path for telemetry"]
    Resp --> HTTP["inference::complete<br/>mock or POST {model_url}/v1/completions"]

    HTTP -->|error| E002["-32002 upstream"]
    HTTP -->|ok| Tel["telemetry::fire — detached POST"]
    Tel --> Return["Ok{ text, document_version }"]

    classDef err fill:#633,stroke:#a66,color:#fff
    class E262,E000,E001,E003,E002 err
```

The same list in words:

1. **Validate** — [`parse_params`](../src/completion/mod.rs) deserializes `params`.
2. **Lookup** — `documents.get(uri)`; unknown documents drop out as `-32000`.
3. **Version gate** — request's `document_version` must equal the document's
   current version; otherwise `-32001` carrying the live version so the client
   can resync.
4. **Debounce** — [`debounce_recheck`](../src/completion/mod.rs) bumps the
   per-URI generation, sleeps `DEBOUNCE_MS`, then refuses `-32003` if a newer
   request claimed the slot OR the document version moved while we slept.
5. **Offset** — [`byte_offset`](../src/completion/mod.rs) flattens
   `{line, character}` to a single byte offset in the document text.
6. **Prompt bits** — [`Repo::build`](../src/completion/mod.rs) caps prefix /
   suffix by bytes and pulls `repository_context` via
   [`context::repository_context`](../src/context/mod.rs).
7. **Call inference** — [`inference::complete`](../src/inference.rs); mock if
   `model_url` is unset, otherwise OpenAI-compatible POST.
8. **Telemetry** — fire-and-forget POST to Phoenix via
   [`telemetry::fire`](../src/telemetry.rs). Never awaits the response path.
9. **Reply** — `{text, document_version}` — same version we validated, so a
   racing `didChange` still surfaces client-side.

## Cancellation hand-off

This handler doesn't know about cancellation — the racing partner is
[`dispatch::on_cancel`](../src/dispatch.rs). The task wraps this fn in a
`tokio::spawn` and claims `answered.insert(id)` before writing. If a cancel
got there first, the response is silently dropped.
