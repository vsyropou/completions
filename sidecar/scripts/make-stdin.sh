#!/bin/sh
# Generate a framed JSON-RPC session into /tmp/sidecar-in.txt.
# Feed it to the sidecar from the shell (`< /tmp/sidecar-in.txt`) or via the
# Zed debug config (.zed/debug.json stdio).
# Content-Length is computed with ${#m} — edit the JSON freely.
{
  m='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspace_root":"file:///tmp","telemetry_url":"","telemetry_api_key":""}}'
  printf 'Content-Length: %d\r\n\r\n%s' ${#m} "$m"

  # File 1: utils.py — imports format_greeting, has a helper the completion will need.
  m='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"uri":"file:///tmp/utils.py","language":"python","version":1,"text":"def calculate_total(items):\n    return sum(items)\n\ndef format_greeting(name):\n    return f\x27Hello, {name}!\x27\n\ndef parse_config(path):\n    return {\"path\": path}\n"}}'
  printf 'Content-Length: %d\r\n\r\n%s' ${#m} "$m"

  # File 2: main.py — imports utils, cursor at a call site.
  m='{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"uri":"file:///tmp/main.py","language":"python","version":1,"text":"from utils import format_greeting, calculate_total\n\ndef main():\n    x = format_greeting(\"world\")\n    total = calculate_total([1,2,3])\n    print(total)\n"}}'
  printf 'Content-Length: %d\r\n\r\n%s' ${#m} "$m"

  # Completion request at "format_greeting(\"world\")" — column 21 (after the opening quote).
  m='{"jsonrpc":"2.0","id":2,"method":"completion/request","params":{"document_uri":"file:///tmp/main.py","document_version":1,"position":{"line":3,"character":21}}}'
  printf 'Content-Length: %d\r\n\r\n%s' ${#m} "$m"

  m='{"jsonrpc":"2.0","id":3,"method":"shutdown"}'
  printf 'Content-Length: %d\r\n\r\n%s' ${#m} "$m"
} > /tmp/sidecar-in.txt

echo "wrote /tmp/sidecar-in.txt"
