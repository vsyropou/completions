defmodule CompleteWeb.Router do
  @moduledoc """
  HTTP routing. Single API pipeline (`:api`) enforces JSON content
  negotiation and `CompleteWeb.Plugs.Auth` bearer-token auth, then
  scopes telemetry endpoints:

    * `POST /api/v1/telemetry/completions` → append event
    * `GET  /api/v1/telemetry/completions` → recent events (newest 50)
    * `GET  /api/v1/telemetry/stats`       → aggregated latency/language stats
  """
  use CompleteWeb, :router

  pipeline :api do
    plug :accepts, ["json"]
    plug CompleteWeb.Plugs.Auth
  end

  scope "/api", CompleteWeb do
    pipe_through :api

    post "/v1/telemetry/completions", TelemetryController, :create
    get "/v1/telemetry/completions", TelemetryController, :index
    get "/v1/telemetry/stats", TelemetryController, :stats
  end
end
