# extension

VS Code extension in TypeScript. Thin adapter: spawns the Rust [`sidecar`](../sidecar/) process, forwards document events, and renders inline completions. All real work (prompt building, inference, telemetry) happens in the sidecar.

## Layout

```
package.json       # activationEvents explicit list, settings contributions
tsconfig.json      # compiled output to out/
src/
  extension.ts     # activation, child-process spawn, JSON-RPC client, document sync
  framing.ts       # Content-Length header encode/decode (LSP-style)
  telemetry.ts     # fire-and-forget POST of `accepted`/`cancelled` outcomes
out/               # tsc output (not committed)
bin/               # staged sidecar binary (created by `make compile`)
```

## Build and run

```sh
npm install
npm run compile    # tsc → out/
```

## Settings

User-facing:

| Setting                      | Example Value                                                      | Used for                              |
| ---------------------------- | ------------------------------------------------------------------ | ------------------------------------- |
| `completion.modelUrl`        | `http://localhost:8000`                                            | Inference base URL; empty → mock      |
| `completion.modelApiKey`     | `<your api key here>`                                              | Bearer to inference; empty -> no auth |
| `completion.model`           | `""`                                                               | Model id; empty = server decides      |
| `completion.sidecarPath`     | Optiona - dev only): `""`                                          | Override the sidecar binary path      |
| `completion.telemetryUrl`    | Optional: `http://localhost:4000`                                  | Telemetry sink URL (empty = disabled) |
| `completion.telemetryApiKey` | Optional: `""`                                                     | Bearer to telemetry sink              |
| `completion.fimTemplate`     | `<\|fim_prefix\|>{prefix}<\|fim_suffix\|>{suffix}<\|fim_middle\|>` | `{prefix}`/`{suffix}` FIM template    |
| `completion.temperature`     | `0.0`                                                              | Sampling temperature                  |
| `completion.maxTokens`       | `32`                                                               | Output token cap                      |

### FIM templates per model

Swapping `completion.model` often means swapping `completion.fimTemplate` too — each model family is trained on different FIM marker tokens. Copy the matching row:

| Model                      | `completion.fimTemplate` value                                                            | Notes                                            |
| -------------------------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| Qwen2.5-Coder (any size)   | `<\|fim_prefix\|>{prefix}<\|fim_suffix\|>{suffix}<\|fim_middle\|>`                          | Sidecar default; also llama.cpp's preset FIM models |
| DeepSeek-Coder-V2 / V3     | `<｜fim▁begin｜>{prefix}<｜fim▁hole｜>{suffix}<｜fim▁end｜>`                                   | Full-width bars (`｜`) and `▁`; copy verbatim   |
| StarCoder2 (3b/7b/15b)     | `<fim_prefix>{prefix}<fim_suffix>{suffix}<fim_middle>`                                      | No `\|` wrappers around the tags                 |
| CodeLlama                  | `<PRE>{prefix}<SUF>{suffix}<MID>`                                                           |                                                  |
| GLM-4.5 / GLM-5            | raw FIM not supported                                                                       | Chat-format only — skip for completions          |
| Granite Code               | raw FIM not supported                                                                       | Instruct-only — skip for completions             |

When in doubt, check `added_tokens_decoder` in the model's `tokenizer_config.json` for the exact FIM token spelling.

## Activation

The extension registers an explicit per-language activation list — see [`package.json`](./package.json) (`activationEvents`). It currently covers 19 languages (`typescript`, `typescriptreact`, `javascript`, `javascriptreact`, `python`, `rust`, `go`, `java`, `c`, `cpp`, `csharp`, `elixir`, `json`, `yaml`, `dockerfile`, `makefile`, `html`, `css`, `markdown`). Adding a new grammar to [`sidecar/src/syntax.rs`](../sidecar/src/syntax.rs) means a matching `onLanguage:` entry in that list.

## Docs

- Wire protocol: [`../protocol/README.md`](../protocol/README.md).
- Overall architecture: [`../docs/architecture.md`](../docs/architecture.md).
