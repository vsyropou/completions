// LSP-style Content-Length framing, pure functions so they can be
// unit-tested with plain `node --test` (no vscode runtime needed).

export function encodeMessage(body: string): Buffer {
  const payload = Buffer.from(body, "utf8");
  const header = Buffer.from(`Content-Length: ${payload.length}\r\n\r\n`, "ascii");
  return Buffer.concat([header, payload]);
}

// Incremental decoder: feed() chunks as they arrive, returns complete bodies.
// ponytail: single-decoder state, one connection per process.
export class FrameDecoder {
  private buf: Buffer = Buffer.alloc(0);

  feed(chunk: Buffer): string[] {
    this.buf = Buffer.concat([this.buf, chunk]);
    const out: string[] = [];
    for (;;) {
      const headerEnd = this.buf.indexOf("\r\n\r\n");
      if (headerEnd === -1) break;
      const header = this.buf.subarray(0, headerEnd).toString("ascii");
      const m = /Content-Length: (\d+)/i.exec(header);
      if (!m) throw new Error("missing Content-Length header");
      const len = parseInt(m[1], 10);
      const start = headerEnd + 4;
      if (this.buf.length < start + len) break;
      out.push(this.buf.subarray(start, start + len).toString("utf8"));
      this.buf = this.buf.subarray(start + len);
    }
    return out;
  }
}
