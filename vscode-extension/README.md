# Completion

Low-latency multiline code completion as you type. When you pause in a
TypeScript file, the extension asks a local helper process for a suggestion
and shows it as ghost text — press Tab to accept. The helper talks to a
Phoenix backend which calls the model. Everything except the model call
runs on your machine.

## Architecture

```
+------------------+  Content-Length frames (stdio)   +------------------+  HTTP   +-----------+
|                  |  <------------------------------>|                  | ------->|           |
|  VS Code         |                                  |  Sidecar (Rust)  |         |  Phoenix  |
|  Extension (TS)  |  JSON-RPC over framed stdin/out  |  sidecar-<plat>- |  /v1    |  backend  |
|                  |                                  |  <arch>          |         |           |
+------------------+                                  +------------------+         +-----------+
        ^                                                                                |
        | users types — VS Code fires inline-completion request on pause                 |
        | (Tab accepts)                                                                v
                                                                                  model call
```

## Sidecar binary resolution

At activation, the extension resolves the sidecar binary in this order:

1. **Config override** — `completion.sidecarPath` (string). If set and
   non-empty, used as-is. Relative paths resolve against the current working
   directory.
2. **Built-in** — `<extensionPath>/bin/sidecar-<platform>-<arch>` (plus
   `.exe` on Windows), where `<platform>` is Node's `process.platform`
   (`darwin`, `linux`, `win32`) and `<arch>` is `process.arch` (`x64`,
   `arm64`, …).

If the resolved path is missing, the extension logs
`sidecar binary not found: <path>` to its output channel and stays inactive.

## Configuration

All under the `completion` namespace (Settings → search "completion"):

| Key | Type | Default | Purpose |
|---|---|---|---|
| `completion.sidecarPath` | string | `""` | Override sidecar binary path (dev use) |
| `completion.phoenixUrl` | string | `http://localhost:4000` | Phoenix backend URL |
| `completion.authToken` | string | `dev-token` | Phoenix bearer token |
| `completion.maxTokens` | number | `128` | Max tokens per completion |
| `completion.measureLatency` | boolean | `false` | Log per-request latency breakdown to the output channel |

## Activation events

- `onLanguage:typescript`, `onLanguage:typescriptreact` — the extension
  activates the first time you open a TS / TSX file.

On activation:

1. Creates the `Completion Sidecar` output channel.
2. Spawns the sidecar binary (see resolution order above).
3. Sends `initialize` with workspace root, `phoenixUrl`, `authToken`,
   `maxTokens`, `measureLatency`.
4. Sends `textDocument/didOpen` for every already-open TS/TSX document.
5. Registers the inline-completion provider and the `didOpen` / `didChange`
   / `didClose` document-sync listeners.

If the sidecar exits unexpectedly, the extension restarts it once after 1 s —
no restart loops. On `deactivate()` it sends `shutdown`, waits 100 ms, then
kills the child and fails any pending requests.

## Install & run

### Dev-mode (F5)

1. Build the sidecar binary for your platform and place it at
   `out/sidecar` inside this folder — or install it as
   `bin/sidecar-<platform>-<arch>`. Either way, if your binary lives
   somewhere else, set `completion.sidecarPath` in settings to its absolute
   path.
2. Open the `vscode-extension/` folder in VS Code.
3. Run `npm install` once, then `npm run compile` (output goes to `out/`).
4. Press F5 — an Extension Development Host window opens with the extension
   active.
5. Packaging as a VSIX (`vsce package`) for distribution is a later option;
   not covered here.

### Configure Phoenix

Set these in user or workspace settings (Settings UI or `settings.json`):

```jsonc
{
  "completion.phoenixUrl": "http://localhost:4000",
  "completion.authToken": "<your-token>"
}
```

### Verify it's alive

View → Output, then pick **Completion Sidecar** from the dropdown. On
activation you should see `spawning sidecar: <path>`. If you enable
`completion.measureLatency`, each request logs a `[latency]` line.

### Trigger a completion

Just type — the provider fires on pause. Accept with Tab.

## Troubleshooting

| Symptom | Likely cause | Check |
|---|---|---|
| No ghost text, ever | Sidecar didn't start | Output channel for `sidecar binary not found`; verify `completion.sidecarPath` or `<extensionPath>/bin/sidecar-<platform>-<arch>` exists |
| No ghost text, ever | Phoenix not reachable | `curl $completion.phoenixUrl`; look for `initialize failed` in output |
| No ghost text, ever | Auth token mismatch | Token in settings must match what Phoenix expects — see Phoenix README |
| Ghost text stalls, then nothing | Request timeout (8 s) | Backend slow or hung — Phoenix logs; raise load or reduce `maxTokens` |
| Output channel quiet | Extension not activated | Open a `.ts` / `.tsx` file — activation only fires on `typescript`/`typescriptreact` |
| Still quiet after that | Wrong file language | Check the language mode in the status bar; must be `TypeScript` or `TypeScript React` |
| `sidecar exited` once, then inactive | Sidecar crashed; the extension restarts it exactly once | Run the binary by hand to see stderr; check binary arch matches your machine |

## Uninstall / restart cleanly

- **Reload this window only:** Command Palette → "Developer: Reload Window" —
  VS Code calls `deactivate()` (graceful `shutdown`, 100 ms grace, kill) and
  re-activates fresh.
- **Remove the extension:** close the dev host or, if installed as a VSIX,
  uninstall from the Extensions view. Any still-running sidecar exits with
  the parent; nothing else is left behind.
