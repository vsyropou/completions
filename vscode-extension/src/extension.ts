import * as vscode from "vscode";
import { spawn, ChildProcess } from "child_process";
import * as path from "path";
import * as fs from "fs";
import { encodeMessage, FrameDecoder } from "./framing";

const REQUEST_TIMEOUT_MS = 8000;
const SHUTDOWN_TIMEOUT_MS = 2000;
const LANGUAGE_MAP: Record<string, string> = {
  typescript: "typescript",
  typescriptreact: "tsx",
};

let output: vscode.OutputChannel;
let child: ChildProcess | undefined;
let decoder: FrameDecoder | undefined;
let nextId = 1;
let restartedOnce = false;
// ponytail: single map for pending requests; size is bounded by user typing speed.
const pending = new Map<
  number,
  { resolve: (v: unknown) => void; reject: (e: unknown) => void; timer: NodeJS.Timeout }
>();
// uri -> version we last told the sidecar about
const syncedVersions = new Map<string, number>();

function config() {
  return vscode.workspace.getConfiguration("completion");
}

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

function sendRequest(method: string, params: object | undefined, timeoutMs: number): Promise<unknown> {
  return new Promise((resolve, reject) => {
    if (!child?.stdin?.writable) {
      reject(new Error("sidecar not running"));
      return;
    }
    const id = nextId++;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer });
    send({ jsonrpc: "2.0", id, method, ...(params !== undefined ? { params } : {}) });
  });
}

function sendNotification(method: string, params: object) {
  send({ jsonrpc: "2.0", method, params });
}

function failAllPending(err: Error) {
  for (const [, p] of pending) {
    clearTimeout(p.timer);
    p.reject(err);
  }
  pending.clear();
}

function onMessage(body: string) {
  let msg: {
    id?: number;
    result?: unknown;
    error?: { code: number; message: string };
    method?: string;
    params?: unknown;
  };
  try {
    msg = JSON.parse(body);
  } catch {
    output.appendLine(`sidecar: unparseable message: ${body.slice(0, 200)}`);
    return;
  }
  if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
    const p = pending.get(msg.id);
    if (!p) return; // late response for cancelled/timed-out request; discard
    clearTimeout(p.timer);
    pending.delete(msg.id);
    if (msg.error) p.reject(new Error(`${msg.error.code}: ${msg.error.message}`));
    else p.resolve(msg.result);
  }
  // Notifications from sidecar: none defined in v1 protocol; ignore.
}

function killChild() {
  const c = child;
  child = undefined;
  decoder = undefined;
  if (c) {
    try {
      c.kill();
    } catch {
      /* already dead */
    }
  }
}

function spawnSidecar(context: vscode.ExtensionContext): boolean {
  const bin = sidecarPath(context);
  if (!bin || !fs.existsSync(bin)) {
    output.appendLine(`sidecar binary not found: ${bin ?? "(unset)"} — completion inactive`);
    return false;
  }
  output.appendLine(`spawning sidecar: ${bin}`);
  const c = spawn(bin, [], { stdio: ["pipe", "pipe", "pipe"] });
  child = c;
  const dec = new FrameDecoder();
  decoder = dec;

  c.stdout!.on("data", (chunk: Buffer) => {
    let bodies: string[];
    try {
      bodies = dec.feed(chunk);
    } catch (e) {
      output.appendLine(`sidecar: framing error: ${e}`);
      return;
    }
    for (const b of bodies) onMessage(b);
  });
  c.stderr!.on("data", (chunk: Buffer) => {
    output.append(chunk.toString("utf8"));
  });
  c.on("exit", (code, signal) => {
    output.appendLine(`sidecar exited: code=${code} signal=${signal}`);
    if (child !== c) return; // intentional kill (deactivate)
    child = undefined;
    decoder = undefined;
    failAllPending(new Error("sidecar exited"));
    syncedVersions.clear();
    // One restart, no loops.
    if (!restartedOnce) {
      restartedOnce = true;
      output.appendLine("restarting sidecar once after 1s");
      setTimeout(() => {
        if (spawnSidecar(context)) void initializeAndSync();
      }, 1000);
    }
  });
  c.on("error", (e) => {
    output.appendLine(`sidecar spawn error: ${e.message}`);
  });
  return true;
}

function languageOf(doc: vscode.TextDocument): string | undefined {
  return LANGUAGE_MAP[doc.languageId];
}

async function initializeAndSync() {
  const cfg = config();
  const workspaceRoot =
    vscode.workspace.workspaceFolders?.[0]?.uri.toString() ?? "file:///";
  try {
    await sendRequest(
      "initialize",
      {
        workspace_root: workspaceRoot,
        phoenix_url: cfg.get<string>("phoenixUrl"),
        auth_token: cfg.get<string>("authToken"),
        max_tokens: cfg.get<number>("maxTokens"),
        measure_latency: cfg.get<boolean>("measureLatency"),
      },
      REQUEST_TIMEOUT_MS
    );
  } catch (e) {
    output.appendLine(`initialize failed: ${e}`);
    return;
  }
  for (const doc of vscode.workspace.textDocuments) {
    if (languageOf(doc)) syncOpen(doc);
  }
}

function syncOpen(doc: vscode.TextDocument) {
  const lang = languageOf(doc);
  if (!lang) return;
  syncedVersions.set(doc.uri.toString(), doc.version);
  sendNotification("textDocument/didOpen", {
    uri: doc.uri.toString(),
    language: lang,
    version: doc.version,
    text: doc.getText(),
  });
}

function syncChange(doc: vscode.TextDocument) {
  if (!languageOf(doc)) return;
  syncedVersions.set(doc.uri.toString(), doc.version);
  sendNotification("textDocument/didChange", {
    uri: doc.uri.toString(),
    version: doc.version,
    text: doc.getText(),
  });
}

function syncClose(doc: vscode.TextDocument) {
  if (!syncedVersions.delete(doc.uri.toString())) return;
  sendNotification("textDocument/didClose", { uri: doc.uri.toString() });
}

async function provideInlineCompletionItems(
  doc: vscode.TextDocument,
  position: vscode.Position,
  _context: vscode.InlineCompletionContext,
  token: vscode.CancellationToken
): Promise<vscode.InlineCompletionItem[]> {
  if (!child || !languageOf(doc)) return [];
  const id = nextId;
  const startedAt = Date.now();
  const cancelSub = token.onCancellationRequested(() => {
    sendNotification("completion/cancel", { request_id: id });
  });
  try {
    const result = (await Promise.race([
      sendRequest(
        "completion/request",
        {
          document_uri: doc.uri.toString(),
          document_version: doc.version,
          position: { line: position.line, character: position.character },
          max_tokens: config().get<number>("maxTokens"),
        },
        REQUEST_TIMEOUT_MS
      ),
      new Promise<never>((_, reject) => {
        token.onCancellationRequested(() => reject(new Error("cancelled")));
      }),
    ])) as { text: string; document_version: number; latency_ms?: { total: number; http: number } };
    if (config().get<boolean>("measureLatency") && result.latency_ms) {
      output.appendLine(
        `[latency] total=${Date.now() - startedAt}ms rust_total=${result.latency_ms.total}ms rust_http=${result.latency_ms.http}ms`
      );
    }
    // Stale check: document moved on, or editor switched away.
    if (result.document_version !== doc.version) return [];
    if (vscode.window.activeTextEditor?.document.uri.toString() !== doc.uri.toString()) return [];
    if (token.isCancellationRequested) return [];
    return [new vscode.InlineCompletionItem(result.text, new vscode.Range(position, position))];
  } catch {
    return []; // errors and timeouts: silent, never block typing
  } finally {
    cancelSub.dispose();
  }
}

export function activate(context: vscode.ExtensionContext) {
  output = vscode.window.createOutputChannel("Completion Sidecar");
  context.subscriptions.push(output);

  if (spawnSidecar(context)) {
    void initializeAndSync();
  }

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(syncOpen),
    vscode.workspace.onDidChangeTextDocument((e) => {
      if (e.contentChanges.length > 0) syncChange(e.document);
    }),
    vscode.workspace.onDidCloseTextDocument(syncClose),
    vscode.languages.registerInlineCompletionItemProvider(
      [{ language: "typescript" }, { language: "typescriptreact" }],
      { provideInlineCompletionItems }
    )
  );
}

export async function deactivate() {
  if (child) {
    try {
      await sendRequest("shutdown", undefined, SHUTDOWN_TIMEOUT_MS);
    } catch {
      /* sidecar may already be gone */
    }
    // Give it the remaining grace period, then kill.
    await new Promise((r) => setTimeout(r, 100));
  }
  killChild();
  failAllPending(new Error("deactivated"));
}
