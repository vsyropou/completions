# Sidecar flow

Two flowcharts: the dispatch loop from [`sidecar/src/lib.rs`](../sidecar/src/lib.rs), then the per-request pipeline inside [`completion.rs::handle_completion_request`](../sidecar/src/completion.rs). Functions named inline map 1:1 to the source.

## 1. `sidecar::run` — dispatch loop

```
                stdin (JSON-RPC over LSP-style frames)
                          │
                          ▼
                  read_message()                                  [rpc.rs]
                          │
                          ▼
                 method? (str)  id? (int/str)
                          │
        ┌──────────┬──────┴────────┬────────────┬────────────────┐
        ▼          ▼               ▼            ▼                ▼
   initialize   shutdown    completion/    completion/    textDocument/*
                              request         cancel        didOpen|didChange|didClose
        │          │               │            │                │
        ▼          ▼               ▼            ▼                ▼
  handle_      drain          tokio::spawn   abort_handle   documents
  initialize   pending        the handler    .abort()       .did_open/etc
  (sync)       tasks                       │   + write -32003
        │          │               │        (sync)
        │          │               ▼
        │          │       (background) ──────┐
        │          │                          │
        ▼          ▼                          ▼
   Config ← ─  success/null   write response on stdout
   fields                       + free slot in Pending
   stashed
        │
        ▼
    workspace_index
    (tokio::spawn,
     background)
```

After each non-shutdown branch, loop back to `read_message()`. After `shutdown`, drain pending tasks (≤5 s), write a `null` result, and exit.

## 2. `handle_completion_request` — per-request pipeline

```
JSON-RPC request
      │
      ▼
parse params ─────────────────► JsonRpcError::invalid_params (if malformed)
      │
      ▼
documents.get(uri) ───────────► -32000 unknown_document (if missing)
      │
      ▼
doc.version == p.document_version? ─► -32001 stale version (if mismatch)
      │
      ▼
debounce.bump(uri) ──┐
                     │  sleep 200 ms
                     ▼
debounce.is_current? ─► -32003 cancelled (superseded by a newer request)
      │
      │  version still matches? ─► -32003 cancelled (doc moved during debounce)
      ▼
utf16_to_utf8_offset(position)                              [completion.rs::utf16_to_utf8_offset]
      │
      ▼
syntax::Lang::from_language_id(doc.language)                [syntax.rs]
      │
      ▼
syntax::parse(lang, &doc.text) ────► Option<Tree>
      │
      ▼
context::compute_cursor_excerpt(...)                        [context.rs::compute_cursor_excerpt]
      │  syntax-aware window around cursor, falls back to linewise
      ▼
cap_prefix / cap_suffix                                     [completion.rs]
      │  hard byte caps (3 B ≈ 1 token)
      ▼
context::identifiers_near_cursor(text, offset, 3) ────► Vec<String>   (BM25 query terms)
      │
      ▼
context::build_repository_context_with_query                [context.rs::build_repository_context_with_query]
      │
      ├─(a)─▶ same-file symbols                       (Index::symbols_for)
      │        filter: enclosing.function/class ≠ s.name
      │        filter: symbol name not in prefix_tail (400 B before cursor)
      │
      ├─(b)─▶ symbols from imports                    (Index::imports_of + resolve_import)
      │        dedupe, cap REPO_CTX_CAP bytes
      │
      └─(c)─▶ BM25 top-3 ranked excerpts               (Index::bm25_rank)
               skip: current_file, files already in (b)
      ▼
"# Related symbols:\n...\n"  (or empty if tree parse failed / no matches)
      │
      ▼
inference::complete(cfg, lang, file, prefix, suffix, repo_ctx, request_id)
      │                                                                  [inference.rs::complete]
      ├─ build_prompt: "# Path: <file>" + repo_block + fim_template_with_{prefix,suffix}
      │
      ├─ model_url is None? ──► return static MOCK_COMPLETION
      │
      └─ else POST {model_url}/v1/completions
                 body: {model, prompt, max_tokens: cfg.max_tokens default 32,
                        temperature: cfg.temperature default 0.0,
                        stop: ["\n\n\n", "```", "<|fim_middle|>", "<|fim_suffix|>"]}
              timeout 25 s
              ──► -32002 upstream (network / non-2xx / malformed JSON)
      │
      ▼
(prompt, completion, finish_reason, model)
      │
      ▼
telemetry::fire   (tokio::spawn, detached)                  [telemetry.rs]
      │  POST {telemetry_url}/api/v1/completions
      │  body: {request_id, language, file_path, prompt, response, latency_ms,
      │         provider, model, finish_reason}
      │  errors → tracing::debug, dropped silently
      ▼
response = {text, document_version}
      │
      ▼
locked stdout write (Content-Length frame)
```

## 3. Cancellation & graceful shutdown

- `completion/cancel` pops the `AbortHandle` for that request-id from `Pending.cancel`, claims the id in `Pending.answered`, aborts the task, and writes `-32003` synchronously. If the task already responded (the id was already in `answered`), it's a no-op — never double-writes.
- `shutdown` drains `Pending.tasks: Vec<JoinHandle<()>>` (cap 5 s) so a racing completion can finish writing before the process exits, then returns `null` and exits 0.

## Where the code lives

| Step                                | File                                                          |
|-------------------------------------|---------------------------------------------------------------|
| stdin read / frame parse            | [`src/rpc.rs`](../sidecar/src/rpc.rs)                         |
| dispatch loop + shutdown drain      | [`src/lib.rs`](../sidecar/src/lib.rs)                         |
| debounce + request handler          | [`src/completion.rs`](../sidecar/src/completion.rs)           |
| UTF-16↔UTF-8 conversion             | [`src/completion.rs::utf16_to_utf8_offset`](../sidecar/src/completion.rs) |
| tree-sitter Lang + parse            | [`src/syntax.rs`](../sidecar/src/syntax.rs)                   |
| cursor excerpt / BM25 / enclosing   | [`src/context.rs`](../sidecar/src/context.rs)                 |
| workspace index (symbols, imports)  | [`src/index.rs`](../sidecar/src/index.rs)                     |
| prompt build + HTTP / mock fallback | [`src/inference.rs`](../sidecar/src/inference.rs)             |
| telemetry POST (fire-and-forget)    | [`src/telemetry.rs`](../sidecar/src/telemetry.rs)             |
