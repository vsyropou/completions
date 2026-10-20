// Skeleton VS Code extension: shows the flow of information from the editor,
// through stdio JSON-RPC framing, to the sidecar process — and back.
//
// Happy-path by design. Real-impl concerns (retry, restart, cancel,
// staleness guards) are noted inline as `placeholder:` comments.

import * as vscode from "vscode";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";
import * as fs from "fs";
import { encodeMessage, FrameDecoder } from "./framing";

const REQUEST_TIMEOUT_MS = 8000;

// Languages we sync / offer completions on. Keys are VS Code `languageId`s;
// values are what we pass to the sidecar in `didOpen`'s `language` field
// (free-string per protocol §1). Anything outside this map → "unknown".
const LANGUAGE_MAP: Record<string, string> = {
  typescript: "typescript",
  typescriptreact: "typescriptreact",
  javascript: "javascript",
  javascriptreact: "javascriptreact",
  python: "python",
  rust: "rust",
  go: "go",
  java: "java",
  c: "c",
  cpp: "cpp",
  csharp: "csharp",
  elixir: "elixir",
  json: "json",
  yaml: "yaml",
  dockerfile: "dockerfile",
  makefile: "makefile",
  html: "html",
  css: "css",
  markdown: "markdown",
};

function languageFor(doc: vscode.TextDocument): string {
  return LANGUAGE_MAP[doc.languageId] ?? "unknown";
}

// Files we mirror to the sidecar plus ask completions for. Any real on-disk
// text document counts — unmapped languages go through as `"unknown"` and
// the sidecar decides what to do (currently: prefix/suffix only, no parse).
function isSupported(doc: vscode.TextDocument): boolean {
  return doc.uri.scheme === "file";
}

let output: vscode.OutputChannel;
let child: ChildProcess | undefined;
let decoder: FrameDecoder | undefined;
let nextId = 1;

// In-flight JSON-RPC requests, by id. Resolved when the sidecar replies.
const pending = new Map<
  number,
  { resolve: (v: unknown) => void; reject: (e: unknown) => void; timer: NodeJS.Timeout }
>();

function config() {
  return vscode.workspace.getConfiguration("completion");
}

// Which binary will we spawn? Setting override, else the bundled sidecar.
function sidecarPath(context: vscode.ExtensionContext): string | undefined {
  const override = config().get<string>("sidecarPath");
  if (override) return override;
  const exe = `sidecar-${process.platform}-${process.arch}${process.platform === "win32" ? ".exe" : ""}`;
  return path.join(context.extensionPath, "bin", exe);
}

function send(msg: object) {
  if (!child?.stdin?.writable) return;
  child.stdin.write(encodeMessage(JSON.stringify(msg)));
}

// JSON-RPC request: write to stdin, wait for matching `id` on stdout.
function sendRequest(method: string, params: object | undefined): Promise<unknown> {
  return new Promise((resolve, reject) => {
    if (!child?.stdin?.writable) {
      reject(new Error("sidecar not running"));
      return;
    }
    const id = nextId++;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out`));
    }, REQUEST_TIMEOUT_MS);
    pending.set(id, { resolve, reject, timer });
    send({ jsonrpc: "2.0", id, method, ...(params !== undefined ? { params } : {}) });
  });
}

// JSON-RPC notification: fire and forget.
function sendNotification(method: string, params: object) {
  send({ jsonrpc: "2.0", method, params });
}

// One decoded frame from the sidecar → resolve the matching pending request.
function onMessage(body: string) {
  const msg = JSON.parse(body) as {
    id?: number;
    result?: unknown;
    error?: { code: number; message: string };
  };
  if (msg.id === undefined) return;
  const p = pending.get(msg.id);
  if (!p) return;
  clearTimeout(p.timer);
  pending.delete(msg.id);
  if (msg.error) p.reject(new Error(`${msg.error.code}: ${msg.error.message}`));
  else p.resolve(msg.result);
}

// Spawn the sidecar binary, wire stdout frames → onMessage, stderr → output.
function spawnSidecar(context: vscode.ExtensionContext): boolean {
  const bin = sidecarPath(context);
  if (!bin || !fs.existsSync(bin)) {
    output.appendLine(`sidecar binary not found: ${bin ?? "(unset)"} — completion inactive`);
    return false;
  }
  output.appendLine(`spawning sidecar: ${bin}`);
  const c = spawn(bin, [], { stdio: ["pipe", "pipe", "pipe"] });
  child = c;
  decoder = new FrameDecoder();

  c.stdout!.on("data", (chunk: Buffer) => {
    for (const body of decoder!.feed(chunk)) onMessage(body);
  });
  c.stderr!.on("data", (chunk: Buffer) => {
    output.append(chunk.toString("utf8"));
  });
  c.on("exit", (code, signal) => {
    output.appendLine(`sidecar exited: code=${code} signal=${signal}`);
    child = undefined;
    decoder = undefined;
    for (const [, p] of pending) {
      clearTimeout(p.timer);
      p.reject(new Error("sidecar exited"));
    }
    pending.clear();
    // placeholder: real impl restarts the sidecar once on unexpected exit.
  });
  return true;
}

// Hand the sidecar the workspace root + telemetry connection settings,
// then replay `didOpen` for every file the editor already has open.
async function initializeAndSync() {
  const cfg = config();
  await sendRequest("initialize", {
    workspace_root: vscode.workspace.workspaceFolders?.[0]?.uri.toString() ?? "file:///",
    telemetry_url: cfg.get<string>("telemetryUrl"),
    telemetry_api_key: cfg.get<string>("telemetryApiKey"),
    measure_latency: cfg.get<boolean>("measureLatency"),
    model_url: cfg.get<string>("modelUrl"),
    model_api_key: cfg.get<string>("modelApiKey"),
    model: cfg.get<string>("model") || undefined,
    fim_template: cfg.get<string>("fimTemplate"),
  });
  for (const doc of vscode.workspace.textDocuments) {
    if (isSupported(doc)) syncOpen(doc);
  }
}

// Editor events → sidecar notifications (full-document replacement for v1).
function syncOpen(doc: vscode.TextDocument) {
  if (!isSupported(doc)) return;
  sendNotification("textDocument/didOpen", {
    uri: doc.uri.toString(),
    language: languageFor(doc),
    version: doc.version,
    text: doc.getText(),
  });
}

function syncChange(doc: vscode.TextDocument) {
  if (!isSupported(doc)) return;
  sendNotification("textDocument/didChange", {
    uri: doc.uri.toString(),
    version: doc.version,
    text: doc.getText(),
  });
}

function syncClose(doc: vscode.TextDocument) {
  if (!isSupported(doc)) return;
  sendNotification("textDocument/didClose", { uri: doc.uri.toString() });
}

// VS Code asks for a completion inline (typing pause). We forward to the
// sidecar and return whatever text we get back, at the cursor position.
async function provideInlineCompletionItems(
  doc: vscode.TextDocument,
  position: vscode.Position,
  _context: vscode.InlineCompletionContext,
  token: vscode.CancellationToken
): Promise<vscode.InlineCompletionItem[]> {
  if (!child || !isSupported(doc)) return [];
  // placeholder: real impl sends `completion/cancel` when `token` fires.
  try {
    const result = (await sendRequest("completion/request", {
      document_uri: doc.uri.toString(),
      document_version: doc.version,
      position: { line: position.line, character: position.character },
    })) as { text: string };
    output.appendLine(`completion: ${result.text.length} chars`);
    if (token.isCancellationRequested) return [];
    // placeholder: real impl discards if doc.version has moved on.
    return [new vscode.InlineCompletionItem(result.text, new vscode.Range(position, position))];
  } catch {
    return []; // never block typing on backend trouble
  }
}

// Activation entry point (runs when first file of a supported language opens).
export function activate(context: vscode.ExtensionContext) {
  output = vscode.window.createOutputChannel("Completion Sidecar");
  context.subscriptions.push(output);

  if (spawnSidecar(context)) {
    initializeAndSync().catch((e) => output.appendLine(`initialize failed: ${e}`));
  }

  const selector: vscode.DocumentSelector = Object.keys(LANGUAGE_MAP).map((language) => ({
    language,
    scheme: "file",
  }));

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(syncOpen),
    vscode.workspace.onDidChangeTextDocument((e) => {
      if (e.contentChanges.length > 0) syncChange(e.document);
    }),
    vscode.workspace.onDidCloseTextDocument(syncClose),
    vscode.languages.registerInlineCompletionItemProvider(selector, {
      provideInlineCompletionItems,
    })
  );
}

// Best-effort shutdown: ask nicely, then kill.
export async function deactivate() {
  if (child) {
    try {
      await sendRequest("shutdown", undefined);
    } catch {
      // already gone
    }
    child.kill();
    child = undefined;
    decoder = undefined;
  }
}
