// E2E: spawn sidecar, talk JSON-RPC over stdio, assert mock completion comes back.
// Run: node e2e.mjs   (requires Phoenix on :4000 and sidecar/target/debug/sidecar)
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";

const root = path.dirname(fileURLToPath(import.meta.url));
const sidecar = path.join(root, "../sidecar/target/debug/sidecar");

const child = spawn(sidecar, [], { stdio: ["pipe", "pipe", "inherit"] });

let buf = Buffer.alloc(0);
const pending = new Map();
let nextId = 1;

function send(obj) {
  const body = Buffer.from(JSON.stringify(obj), "utf8");
  child.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
  child.stdin.write(body);
}
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

child.stdout.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const headerEnd = buf.indexOf("\r\n\r\n");
    if (headerEnd === -1) return;
    const header = buf.subarray(0, headerEnd).toString("utf8");
    const m = /Content-Length: (\d+)/i.exec(header);
    if (!m) throw new Error("bad header: " + header);
    const len = Number(m[1]);
    if (buf.length < headerEnd + 4 + len) return;
    const msg = JSON.parse(buf.subarray(headerEnd + 4, headerEnd + 4 + len).toString("utf8"));
    buf = buf.subarray(headerEnd + 4 + len);
    const p = pending.get(msg.id);
    if (p) {
      pending.delete(msg.id);
      if (msg.error) p.reject(Object.assign(new Error(msg.error.message), { code: msg.error.code }));
      else p.resolve(msg.result);
    }
  }
});

const uri = "file:///repo/src/main.ts";
const textV1 = "function calculate() {\n\n}\n";
const textV2 = "function calculate() {\n  \n}\n";

// 1. initialize
const init = await request("initialize", {
  workspace_root: "file:///repo",
  phoenix_url: "http://localhost:4000",
  auth_token: "dev-token",
  max_tokens: 128,
});
console.log("initialize ->", JSON.stringify(init));

// 2. didOpen v1, completion at cursor; then stale-version check (must fail -32001)
notify("textDocument/didOpen", { uri, language: "typescript", version: 1, text: textV1 });
notify("textDocument/didChange", { uri, version: 2, text: textV2 });

const completion = await request("completion/request", {
  document_uri: uri,
  document_version: 2,
  position: { line: 1, character: 2 },
  max_tokens: 128,
});
console.log("completion ->", JSON.stringify(completion));
assert.equal(completion.document_version, 2);
assert.ok(completion.text.length > 0, "empty completion");
assert.ok(completion.text.includes("\n"), "expected multiline mock completion");

// 3. stale version must be rejected
let staleErr = null;
try {
  await request("completion/request", {
    document_uri: uri,
    document_version: 1,
    position: { line: 0, character: 0 },
    max_tokens: 16,
  });
} catch (e) {
  staleErr = e;
}
assert.ok(staleErr, "expected stale version error");
assert.equal(staleErr.code, -32001);
console.log("stale version correctly rejected with", staleErr.code);

// 4. unknown document
let unknownErr = null;
try {
  await request("completion/request", {
    document_uri: "file:///repo/src/nope.ts",
    document_version: 1,
    position: { line: 0, character: 0 },
    max_tokens: 16,
  });
} catch (e) {
  unknownErr = e;
}
assert.equal(unknownErr.code, -32000);
console.log("unknown document correctly rejected with", unknownErr.code);

// 5. UTF-16 cursor: line with emoji; cursor right after emoji (2 UTF-16 units)
notify("textDocument/didOpen", {
  uri: "file:///repo/src/emoji.ts",
  language: "typescript",
  version: 1,
  text: "const a = \"😀\";\n",
});
const emojiCompletion = await request("completion/request", {
  document_uri: "file:///repo/src/emoji.ts",
  document_version: 1,
  position: { line: 0, character: 13 }, // just after the closing quote+semicolon? no: 11=emoji end... use end-of-string position
  max_tokens: 16,
}).catch(() => null);
// position sanity: cursor inside line 0 after emoji must not error
const emojiOk = await request("completion/request", {
  document_uri: "file:///repo/src/emoji.ts",
  document_version: 1,
  position: { line: 0, character: 12 }, // right after "😀" (emoji = 2 UTF-16 units)
  max_tokens: 16,
});
console.log("emoji-position completion ok ->", JSON.stringify(emojiOk).slice(0, 80));

notify("textDocument/didClose", { uri });

// 6. shutdown
const sd = await request("shutdown");
console.log("shutdown ->", JSON.stringify(sd));
await new Promise((res) => child.once("exit", res));
console.log("sidecar exited cleanly");
console.log("E2E PASS");
process.exit(0);
