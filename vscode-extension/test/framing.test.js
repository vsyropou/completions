const test = require("node:test");
const assert = require("node:assert");
const { encodeMessage, FrameDecoder } = require("../out/framing");

test("round-trip two concatenated messages", () => {
  const a = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} });
  const b = JSON.stringify({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { text: "héllo" } });
  const wire = Buffer.concat([encodeMessage(a), encodeMessage(b)]);
  const dec = new FrameDecoder();
  // Feed in odd-sized chunks to exercise incremental parsing.
  const out = [];
  for (let i = 0; i < wire.length; i += 7) {
    out.push(...dec.feed(wire.subarray(i, Math.min(i + 7, wire.length))));
  }
  assert.deepStrictEqual(out, [a, b]);
});
