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
| `completion.telemetryUrl`      | `http://localhost:4000`                          | Telemetry sink URL (empty = disabled)  |
| `completion.telemetryApiKey`   | `dev-token`                                      | Bearer to telemetry sink               |
| `completion.measureLatency`    | `false`                                          | Late-time logs to Output channel       |
| `completion.modelUrl`          | `""`                                             | Inference base URL; empty → mock       |
| `completion.modelApiKey`       | `""`                                             | Bearer to inference                    |
| `completion.model`             | `""`                                             | Model id                               |
| `completion.fimTemplate`       | Starcoder chain template                         | `{prefix}`/`{suffix}` FIM template     |

All four `model*`/`fimTemplate` settings plus `telemetryUrl`/`telemetryApiKey`/`measureLatency` flow through `initialize` to the sidecar's `Config`. Nothing else about them is needed here — the extension doesn't see the prompts or responses.

## Activation

The extension registers an explicit per-language activation list — see [`package.json`](./package.json) (`activationEvents`). It currently covers 19 languages (`typescript`, `typescriptreact`, `javascript`, `javascriptreact`, `python`, `rust`, `go`, `java`, `c`, `cpp`, `csharp`, `elixir`, `json`, `yaml`, `dockerfile`, `makefile`, `html`, `css`, `markdown`). Adding a new grammar to `sidecar/src/syntax.rs` means a matching `onLanguage:` entry in that list.

## Docs

- Wire protocol: `../protocol/README.md` (frozen v1.1).
- Overall architecture: `../docs/architecture.md`.
