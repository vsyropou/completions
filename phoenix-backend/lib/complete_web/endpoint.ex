defmodule CompleteWeb.Endpoint do
  use Phoenix.Endpoint, otp_app: :complete

  plug Plug.RequestId
  plug Plug.Telemetry, event_prefix: [:phoenix, :endpoint]

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
