// POST telemetry events directly to the sink. Same endpoint and event shape
// the sidecar uses — one store, shared by both emitters, keyed on request_id.
// Fire-and-forget: failures are swallowed; telemetry must never block typing.

export interface TelemetryEvent {
  request_id: string;
  language: string;
  file_path: string;
  prompt: string;
  response: string;
  latency_ms: number;
  provider: string;
  model: string;
  finish_reason: string;
  outcome:
    | "requested"
    | "served"
    | "superseded"
    | "accepted"
    | "cancelled"
    | "error";
}

let telemetryUrl = "";
let telemetryApiKey = "";

export function configureTelemetry(url: string, apiKey: string) {
  telemetryUrl = url.replace(/\/+$/, "");
  telemetryApiKey = apiKey;
}

// Worst-case inference-server mirror of the sidecar's rule: model URL set → "model".
function providerFor(modelUrl: string | undefined): string {
  return modelUrl ? "model" : "mock";
}

// The extension only ever emits two outcomes: accepted (correlates to the
// sidecar's request_id via InlineCompletionItem.command) and cancelled
// (fires on CancellationToken before the sidecar's request_id arrives, so it
// uses the JSON-RPC id — these events won't reconcile; they're diagnostic).
export function emitOutcome(
  outcome: "accepted" | "cancelled",
  requestId: string,
  language: string,
  filePath: string,
  modelUrl: string | undefined
): void {
  if (!telemetryUrl) return;
  const body: TelemetryEvent = {
    request_id: requestId,
    language,
    file_path: filePath,
    prompt: "",
    response: "",
    latency_ms: 0,
    provider: providerFor(modelUrl),
    model: "",
    finish_reason: "",
    outcome,
  };
  fetch(`${telemetryUrl}/api/v1/events`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(telemetryApiKey ? { authorization: `Bearer ${telemetryApiKey}` } : {}),
    },
    body: JSON.stringify(body),
  })
    .then((r) => {
      if (!r.ok) {
        // eslint-disable-next-line no-console
        console.error(`telemetry POST failed: ${r.status} ${r.statusText}`);
      }
    })
    .catch((e) => {
      // eslint-disable-next-line no-console
      console.error(`telemetry POST error: ${e}`);
    });
}
