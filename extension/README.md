# extension

VS Code extension in TypeScript. Thin adapter: spawns the Rust `sidecar` process, forwards document events, and renders inline completions. All real work (prompt building, inference, telemetry) happens in the sidecar.

## Layout

```
package.json       # activationEvents: ['*'], settings contributions
tsconfig.json      # compiled output to out/
src/
  extension.ts     # activation, child-process spawn, JSON-RPC client, document sync
  framing.ts       # Content-Length header encode/decode (LSP-style)
out/               # tsc output (not committed)
```

## Build and run

```sh
npm install
npm run compile    # tsc → out/
```

Manual loop:
1. Build the sidecar first: `cd ../sidecar && cargo build`.
2. Open this directory (`extension/`) in VS Code.
3. Press F5 → Extension Development Host.
4. In the **dev host's** settings, set `completion.sidecarPath` to `…/sidecar/target/debug/sidecar`.
5. Open a source file in the dev host — ghost text appears within a keystroke.

## Settings

User-facing, all under the `completion.*` prefix:

| Setting                     | Default                                          | Used for                               |
|-----------------------------|--------------------------------------------------|----------------------------------------|
| `completion.sidecarPath`       | `""` (auto)                                      | Override the sidecar binary path       |
| `completion.phoenixUrl`        | `http://localhost:4000`                          | Telemetry sink URL                     |
| `completion.authToken`         | `dev-token`                                      | Bearer to Phoenix+sidecar              |
| `completion.maxTokens`         | `128`                                            | Init default for max tokens            |
| `completion.measureLatency`    | `false`                                          | Late-time logs to Output channel       |
| `completion.modelUrl`          | `""`                                             | Inference base URL; empty → mock       |
| `completion.modelApiKey`       | `""`                                             | Bearer to inference                    |
| `completion.model`             | `""`                                             | Model id                               |
| `completion.fimTemplate`       | Starcoder chain template                         | `{prefix}`/`{suffix}` FIM template     |

All four `model*`/`fimTemplate` settings flow through `initialize` to the sidecar's `Config`. Nothing else about them is needed here — the extension doesn't see the prompts or responses.

## Activation

`activationEvents: ["*"]` in `package.json` — the extension activates on every language. The provider's document selector (`src/extension.ts`) gates which `languageId`s actually produce completions; the sidecar's `syntax.rs` decides which ones have real tree-sitter grammars.

## Docs

- Wire protocol: `../protocol/README.md` (frozen v1.1).
- Overall architecture: `../docs/architecture.md`.
