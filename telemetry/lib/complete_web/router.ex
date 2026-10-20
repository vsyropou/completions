defmodule CompleteWeb.Router do
  @moduledoc """
  HTTP routing. Single API pipeline (`:api`) enforces JSON content
  negotiation and `CompleteWeb.Plugs.Auth` bearer-token auth, then
  scopes telemetry endpoints:

    * `POST /api/v1/completions` → append event
    * `GET  /api/v1/completions` → recent events (newest 50)
    * `GET  /api/v1/stats`       → aggregated latency/language stats
  """
  use CompleteWeb, :router

  pipeline :api do
    plug :accepts, ["json"]
    plug CompleteWeb.Plugs.Auth
  end

  scope "/api", CompleteWeb do
    pipe_through :api

    post "/v1/completions", TelemetryController, :create
    get "/v1/completions", TelemetryController, :index
    get "/v1/stats", TelemetryController, :stats
  end
end
