defmodule CompleteWeb.Endpoint do
  @moduledoc """
  Phoenix HTTP endpoint. Plug pipeline order:

    1. `Plug.RequestId` — stamps a request id for logs.
    2. `Plug.Telemetry` — emits `[:phoenix, :endpoint, ...]` telemetry.
    3. `Plug.Parsers` — JSON-only body parsing, 256 KB cap (protocol §3.1).
       Raises `Plug.Parsers.RequestTooLargeError` (→ 413) and
       `Plug.Parsers.ParseError` (→ 400), which Phoenix's built-in
       `render_errors` forwards to `CompleteWeb.ErrorJSON`.
    4. `CompleteWeb.Router` — routes the request.

  Configuration lives in `config/runtime.exs`; `PORT` sets the listen
  port (default 4000).
  """
  use Phoenix.Endpoint, otp_app: :complete

  socket "/live", Phoenix.LiveView.Socket

  plug Plug.RequestId
  plug Plug.Telemetry, event_prefix: [:phoenix, :endpoint]

  # LiveView CSRF token comes from session.
  plug Plug.Session,
    store: :cookie,
    key: "_complete_key",
    signing_salt: "ZjQ0mRhb"

  # 256 KB body cap per protocol v1.0 §3.1. Plug.Parsers raises
  # Plug.Parsers.RequestTooLargeError (status 413) and ParseError (400)
  # on violation; Phoenix.Endpoint.RenderErrors routes them through
  # CompleteWeb.ErrorJSON which emits the protocol-shaped error body.
  plug Plug.Parsers,
    parsers: [:json],
    pass: ["application/json"],
    length: 262_144,
    json_decoder: Phoenix.json_library()

  plug CompleteWeb.Router
end
