# `build_index` — workspace indexing

One-shot background walk, spawned by [`dispatch::on_initialize`](../src/dispatch.rs)
after `initialize` arrives. Fills the in-memory [`Index`](../src/index/mod.rs)
table that `repository_context` reads from on every completion.

## Concrete walk — a tiny workspace

Workspace `file:///app`: three candidate files, one junk dir:

```
/app
├── main.py          "from utils import helper\nhelper()"
├── utils.py         "def helper(): return 42"
├── README.md        (not a source extension, skipped)
└── target/          (always skipped — hardcoded junk-dir list)
```

Resulting `Index` state:

| Map                | Key                   | Value |
|--------------------|-----------------------|-------|
| `inner` (symbols)  | `utils.py`            | helper fn, line 0, Python |
| `imports`          | `utils.py`            | `[]` |
| `imports`          | `main.py`             | `["utils"]` |
| `texts`            | `utils.py`, `main.py` | full bodies (BM25 corpus) |
| `doc_lens`         | same files            | token counts from `tokenize` |

Later, when the user types in `main.py`,
[`context::build_repository_context_with_query`](../src/context/mod.rs) reads
`imports_of(main.py)` → `["utils"]` → `symbols_for_stem("utils")` → finds
`helper` in `utils.py` and prepends it to the prompt. No disk, no re-walk.

## What populates the index

1. [`ignore::WalkBuilder`](https://docs.rs/ignore) — respects .gitignore,
   skips `node_modules .git target dist build deps` defensively.
2. **Extension filter** — 22 whitelisted extensions (see `EXTENSIONS` in
   [`../src/index/walk.rs`](../src/index/walk.rs)).
3. `fs::read_to_string` per file — errors are skipped, not fatal.
4. [`Lang::from_path`](../src/syntax.rs) → optional tree-sitter parse.
   - `NoParser` → **text only** (`index.set_text`). Body still feeds BM25.
   - Grammar-backed → [`index::extract`](../src/index/extract.rs) →
     `set_file` + `set_text`.
5. Refresh on every `didOpen` / `didChange` via
   [`dispatch::refresh_index_for_doc`](../src/dispatch.rs) so the index stays
   warm for whatever the user is actively editing.

## Live-edit refresh

The same per-file shape continues via `didOpen` / `didChange`:
`Lang::from_language_id` → `syntax::parse` → `index::extract` → `set_file` +
`set_text`. The index never re-walks the workspace; it stays fast because
edits are local.
