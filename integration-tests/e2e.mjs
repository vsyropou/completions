// End-to-end smoke test: sidecar (stdio JSON-RPC) -> Phoenix (HTTP, mock) -> back.
// Run from integration-tests/:  node e2e.mjs
// Requires: Phoenix on :4000 (mix phx.server) and sidecar/target/debug/sidecar built.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";

const root = path.dirname(fileURLToPath(import.meta.url));
const sidecarBin = path.join(root, "../sidecar/target/debug/sidecar");

// The one string the Phoenix mock returns on every completion request.
const MOCK =
  "// hello from mock backend\n" +
  "function greet(name: string): string {\n" +
  "  return `hello ${name}`;\n" +
  "}\n";

// -- Step a: start the sidecar -------------------------------------------------
console.log("[1/5] spawning sidecar:", sidecarBin);
const child = spawn(sidecarBin, [], { stdio: ["pipe", "pipe", "inherit"] });
child.once("error", (e) => {
  console.error("failed to spawn sidecar:", e.message);
  process.exit(1);
});

// -- LSP-style framing helpers (write) -----------------------------------------
function send(obj) {
  const body = Buffer.from(JSON.stringify(obj), "utf8");
  child.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
  child.stdin.write(body);
}

// -- LSP-style framing helpers (read): buffer stdout, parse Content-Length ----
let buf = Buffer.alloc(0);
const pending = new Map();
child.stdout.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const headerEnd = buf.indexOf("\r\n\r\n");
    if (headerEnd === -1) return;
    const m = /Content-Length: (\d+)/i.exec(buf.subarray(0, headerEnd).toString("utf8"));
    if (!m) throw new Error("bad frame header");
    const len = Number(m[1]);
    if (buf.length < headerEnd + 4 + len) return;
    const msg = JSON.parse(buf.subarray(headerEnd + 4, headerEnd + 4 + len).toString("utf8"));
    buf = buf.subarray(headerEnd + 4 + len);
    const p = pending.get(msg.id);
    if (p) {
      pending.delete(msg.id);
      if (msg.error) p.reject(new Error(`rpc ${msg.error.code}: ${msg.error.message}`));
      else p.resolve(msg);
    }
  }
});

let nextId = 1;
function request(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send({ jsonrpc: "2.0", id, method, params });
    setTimeout(() => reject(new Error(`timeout waiting for ${method}`)), 15000);
  });
}
function notify(method, params) {
  send({ jsonrpc: "2.0", method, params });
}

try {
  // -- Step b: initialize ------------------------------------------------------
  const init = await request("initialize", {
    workspace_root: "file:///tmp",
    phoenix_url: "http://localhost:4000",
    auth_token: "dev-token",
    max_tokens: 128,
  });
  assert.equal(init.id, 1, "initialize response id must echo request id");
  console.log("[2/5] initialize ok, capabilities:", JSON.stringify(init.result));

  // -- Step c: didOpen a small TS doc ------------------------------------------
  const uri = "file:///tmp/demo.ts";
  notify("textDocument/didOpen", {
    uri,
    language: "typescript",
    version: 1,
    text: "function demo() {\n  \n}\n",
  });
  console.log("[3/5] didOpen sent:", uri, "version 1");

  // -- Step d: completion/request returns the exact mock string ----------------
  const completion = await request("completion/request", {
    document_uri: uri,
    document_version: 1,
    position: { line: 1, character: 2 },
  });
  assert.equal(completion.result.text, MOCK, "completion text must equal the static mock string");
  console.log("[4/5] completion ok, text matches mock string (", MOCK.length, "bytes )");

  // -- Step e: python didOpen smoke (multi-language path) -----------------------
  const pyUri = "file:///tmp/demo.py";
  notify("textDocument/didOpen", {
    uri: pyUri,
    language: "python",
    version: 1,
    text: "def helper():\n    \n",
  });
  const pyCompletion = await request("completion/request", {
    document_uri: pyUri,
    document_version: 1,
    position: { line: 1, character: 4 },
  });
  assert.equal(pyCompletion.result.text, MOCK, "python completion text must equal mock");
  console.log("[4.5/5] python didOpen + completion ok");

  // -- Step f: shutdown, wait for exit -----------------------------------------
  await request("shutdown");
  await new Promise((res) => child.once("exit", res));
  console.log("[5/5] shutdown ok, sidecar exited");

  console.log("[ok] end-to-end smoke test passed");
  process.exit(0);
} catch (e) {
  console.error("[fail]", e.message);
  child.kill();
  process.exit(1);
}
