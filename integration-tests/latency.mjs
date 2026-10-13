// Latency measurement: N=50 sequential completion_rpc round-trips.
// Prints per-request breakdown and p50/p95/p99 summary at the end.
// Requires Phoenix up on :4000 (mock provider) and sidecar/target/debug/sidecar built.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.dirname(fileURLToPath(import.meta.url));
const sidecar = path.join(root, "../sidecar/target/debug/sidecar");
const N = Number(process.env.N || 50);

const child = spawn(sidecar, [], { stdio: ["pipe", "pipe", "ignore"] });

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
    setTimeout(() => reject(new Error(`timeout ${method}`)), 15000);
  });
}
function notify(method, params) {
  send({ jsonrpc: "2.0", method, params });
}

child.stdout.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const he = buf.indexOf("\r\n\r\n");
    if (he === -1) return;
    const header = buf.subarray(0, he).toString("utf8");
    const len = Number(/Content-Length: (\d+)/i.exec(header)[1]);
    if (buf.length < he + 4 + len) return;
    const msg = JSON.parse(buf.subarray(he + 4, he + 4 + len).toString("utf8"));
    buf = buf.subarray(he + 4 + len);
    const p = pending.get(msg.id);
    if (p) {
      pending.delete(msg.id);
      if (msg.error) p.reject(new Error(msg.error.message));
      else p.resolve(msg.result);
    }
  }
});

// Source file: realistic TS, mid-size, with code above + below cursor.
const mkText = () => {
  let lines = [
    "import { helper } from './helper';",
    "",
    "interface User { id: number; name: string }",
    "",
    "export class Service {",
    "  private cache = new Map<number, User>();",
    "  async get(id: number): Promise<User | undefined> {",
    "    const hit = this.cache.get(id);",
    "    if (hit) return hit;",
  ];
  for (let i = 0; i < 30; i++) {
    lines.push(`    // step ${i}`);
  }
  lines.push("    return undefined;");
  lines.push("  }");
  lines.push("}");
  return lines.join("\n") + "\n";
};

const uri = "file:///repo/src/service.ts";
const text = mkText();
notify("textDocument/didOpen", { uri, language: "typescript", version: 1, text });

await request("initialize", {
  workspace_root: "file:///repo",
  phoenix_url: "http://localhost:4000",
  auth_token: "dev-token",
  max_tokens: 128,
  measure_latency: true,
});

const rows = [];
for (let i = 0; i < N; i++) {
  const t0 = performance.now();
  const r = await request("completion/request", {
    document_uri: uri,
    document_version: 1,
    position: { line: 9, character: 12 },
    max_tokens: 128,
  });
  const total = performance.now() - t0;
  const rust = r.latency_ms ?? {};
  rows.push({
    total: total,
    rust_total: rust.total ?? NaN,
    rust_http: rust.http ?? NaN,
  });
}

const pct = (sorted, p) => sorted[Math.floor((p / 100) * (sorted.length - 1))];
const stats = (vals) => {
  const s = [...vals].sort((a, b) => a - b);
  const mean = vals.reduce((a, b) => a + b, 0) / vals.length;
  return { mean: mean.toFixed(1), p50: pct(s, 50).toFixed(1), p95: pct(s, 95).toFixed(1), p99: pct(s, 99).toFixed(1) };
};

console.log(`n=${rows.length}`);
console.log("          mean   p50   p95   p99  (ms)");
console.log("total   ", JSON.stringify(stats(rows.map((r) => r.total))));
console.log("rust_all", JSON.stringify(stats(rows.map((r) => r.rust_total))));
console.log("rust_http", JSON.stringify(stats(rows.map((r) => r.rust_http))));

await request("shutdown");
await new Promise((res) => child.once("exit", res));
notify("textDocument/didClose", { uri });
process.exit(0);
