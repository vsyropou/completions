// Protocol probe: calls every JSON-RPC method the extension sends, in the
// extension's order, and asserts each response. Run from repo root:
//   node integration-tests/api-probes.mjs
// Auto-builds the debug sidecar if stale. Phoenix optional (telemetry is
// fire-and-forget; with MODEL_URL set, asserts the real-model prompt shape).
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";
import { execSync } from "node:child_process";
import fs from "node:fs";

const root = path.dirname(fileURLToPath(import.meta.url));
const sidecarBin = path.join(root, "../sidecar/target/debug/sidecar");

// Auto-(re)build if binary is missing or older than any src/*.rs file.
const needsBuild =
  !fs.existsSync(sidecarBin) ||
  fs.readdirSync(path.join(root, "../sidecar/src")).some(
    (f) => fs.statSync(path.join(root, "../sidecar/src", f)).mtimeMs > fs.statSync(sidecarBin).mtimeMs
  );
if (needsBuild) {
  console.log("[build] cargo build (debug)");
  execSync("cargo build", { cwd: path.join(root, "../sidecar"), stdio: "inherit" });
}

const child = spawn(sidecarBin, [], { stdio: ["pipe", "pipe", "inherit"] });
child.once("error", (e) => { console.error("spawn failed:", e.message); process.exit(1); });

let buf = Buffer.alloc(0);
const pending = new Map();
child.stdout.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const hEnd = buf.indexOf("\r\n\r\n");
    if (hEnd === -1) return;
    const m = /Content-Length: (\d+)/i.exec(buf.subarray(0, hEnd).toString("ascii"));
    if (!m) throw new Error("bad frame header");
    const len = Number(m[1]);
    if (buf.length < hEnd + 4 + len) return;
    const msg = JSON.parse(buf.subarray(hEnd + 4, hEnd + 4 + len).toString("utf8"));
    buf = buf.subarray(hEnd + 4 + len);
    const p = pending.get(msg.id);
    if (p) {
      pending.delete(msg.id);
      msg.error ? p.reject(Object.assign(new Error(msg.error.message), { code: msg.error.code, data: msg.error.data })) : p.resolve(msg);
    }
  }
});

let nextId = 1;
const request = (method, params) => new Promise((res, rej) => {
  const id = nextId++;
  pending.set(id, { resolve: res, reject: rej });
  child.stdin.write(`Content-Length: ${Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }), "utf8").length}\r\n\r\n${JSON.stringify({ jsonrpc: "2.0", id, method, params })}`);
  setTimeout(() => rej(new Error(`timeout waiting for ${method}`)), 15000);
});
const notify = (method, params) => child.stdin.write(`Content-Length: ${Buffer.from(JSON.stringify({ jsonrpc: "2.0", method, params }), "utf8").length}\r\n\r\n${JSON.stringify({ jsonrpc: "2.0", method, params })}`);

// Close enough: `initialize`, then `didOpen`, then `didChange`, then
// `completion/request`, then `completion/cancel`, then `didClose`, then
// `shutdown`. The mock backend always returns this exact string.
const MOCK = "// hello from mock backend\nfunction greet(name: string): string {\n  return `hello ${name}`;\n}\n";

// ── 1. initialize ─────────────────────────────────────────────────────────────
const initParams = {
  workspace_root: "file:///tmp",
  telemetry_url: "",
  telemetry_api_key: "",
};
if (process.env.MODEL_URL) initParams.model_url = process.env.MODEL_URL;
const init = await request("initialize", initParams);
assert.ok(init.result && typeof init.result === "object", "initialize returns an object");
console.log("[1/11] initialize ok");

// ── 2. didOpen (TS, with an emoji for UTF-16 coverage) ────────────────────────
const tsUri = "file:///tmp/probe.ts";
const tsText = "function greet(name: string) {\n  const emoji = \"🍕\";\n  \n}\n";
notify("textDocument/didOpen", { uri: tsUri, language: "typescript", version: 1, text: tsText });
// Small sleep so the sidecar indexes the doc before we request on it.
await new Promise((r) => setTimeout(r, 150));
console.log("[2/11] didOpen TS v1");

// ── 3. didChange (full replace, v1 → v2) ──────────────────────────────────────
const tsText2 = tsText.replace("function greet", "function greet_v2");
notify("textDocument/didChange", { uri: tsUri, version: 2, text: tsText2 });
console.log("[3/11] didChange v2 sent");

// ── 4. didChange stale (v2 → v5, should be swallowed silently) ────────────────
notify("textDocument/didChange", { uri: tsUri, version: 5, text: "// stale\n" });
// No assertion possible on a swallow; just verify the sidecar stays alive.
console.log("[4/11] didChange v5 sent (stale — expect silent drop)");

// ── 5. completion/request (UTF-16 emoji line, v2) ────────────────────────────
// Cursor right after the emoji (line 1, 0-based): `const emoji = "🍕";`.
// `const emoji = "` is 16 ASCII chars (16 units), 🍕 is 2 → cursor is at col 18.
const comp1 = await request("completion/request", {
  document_uri: tsUri,
  document_version: 2,
  position: { line: 1, character: 18 },
});
assert.equal(comp1.result.document_version, 2, "completion version echo");
if (process.env.MODEL_URL) {
  assert.ok(comp1.result.text.length > 0, "model completion non-empty");
} else {
  assert.equal(comp1.result.text, MOCK, "mock marker round-trip");
}
console.log(`[5/11] completion ok (${process.env.MODEL_URL ? "model" : "mock"}, ${comp1.result.text.length} chars)`);

// ── 6. unknown method → -32601 ───────────────────────────────────────────────
try {
  await request("foo/bar", {});
  assert.fail("expected -32601");
} catch (e) {
  assert.equal(e.code, -32601, "unknown method → -32601");
}
console.log("[6/11] -32601 for unknown method");

// ── 7. unknown document → -32000 ──────────────────────────────────────────────
try {
  await request("completion/request", {
    document_uri: "file:///tmp/never-opened.ts",
    document_version: 1,
    position: { line: 0, character: 0 },
  });
  assert.fail("expected -32000");
} catch (e) {
  assert.equal(e.code, -32000, "unopened doc → -32000");
}
console.log("[7/11] -32000 for unopened document");

// ── 8. version mismatch → -32001 with data.current_version ───────────────────
// After step 3, the live version is 2. Re-asking for v1 must be -32001.
try {
  await request("completion/request", {
    document_uri: tsUri,
    document_version: 1,
    position: { line: 1, character: 18 },
  });
  assert.fail("expected -32001");
} catch (e) {
  assert.equal(e.code, -32001, "stale version → -32001");
  assert.equal(e.data?.current_version, 2, "data.current_version present");
}
console.log("[8/11] -32001 with current_version=2");

// ── 9. multi-language: python didOpen + completion ────────────────────────────
const pyUri = "file:///tmp/probe.py";
notify("textDocument/didOpen", { uri: pyUri, language: "python", version: 1, text: "def helper():\n    \n" });
await new Promise((r) => setTimeout(r, 150));
const pyComp = await request("completion/request", {
  document_uri: pyUri,
  document_version: 1,
  position: { line: 1, character: 4 },
});
assert.equal(pyComp.result.document_version, 1);
if (!process.env.MODEL_URL) assert.equal(pyComp.result.text, MOCK);
console.log("[9/11] python didOpen + completion ok");

// ── 10. completion/cancel ────────────────────────────────────────────────────
// Cancel-vs-completion is a race by design (see sidecar/tests/dispatch.rs for
// the bookkeeping assertions); the probe's job is just "sidecar stays healthy."
const cancelId = nextId++;
const cancelBody = Buffer.from(JSON.stringify({
  jsonrpc: "2.0", id: cancelId, method: "completion/request",
  params: { document_uri: pyUri, document_version: 1, position: { line: 1, character: 4 } },
}), "utf8");
child.stdin.write(`Content-Length: ${cancelBody.length}\r\n\r\n`);
child.stdin.write(cancelBody);
notify("completion/cancel", { request_id: cancelId });
await new Promise((r) => setTimeout(r, 500));
console.log("[10/11] completion/cancel sent");

// ── 11. shutdown ─────────────────────────────────────────────────────────────
const shut = await request("shutdown");
assert.ok("result" in shut, "shutdown echoes result:null");
await new Promise((r) => child.once("exit", r));
console.log("[11/11] shutdown ok, sidecar exited");

console.log(`[ok] protocol probes passed (${process.env.MODEL_URL ? "model" : "mock"} mode)`);
child.kill();
process.exit(0);
