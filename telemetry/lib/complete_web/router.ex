defmodule CompleteWeb.Router do
  @moduledoc """
  HTTP routing. `:api` enforces JSON content negotiation and
  `CompleteWeb.Plugs.Auth` bearer-token auth; `:browser` serves the
  unauthenticated dev LiveView UI (intentionally open — local tooling,
  not production traffic).
  """
  use CompleteWeb, :router

  pipeline :api do
    plug :accepts, ["json"]
    plug CompleteWeb.Plugs.Auth
  end

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {CompleteWeb.Layouts, :root}
    plug :protect_from_forgery
  end

  scope "/api", CompleteWeb do
    pipe_through :api

    post "/v1/events", TelemetryController, :create
    get "/v1/events", TelemetryController, :index
    get "/v1/stats", TelemetryController, :stats
  end

  scope "/", CompleteWeb do
    pipe_through :browser

    live_session :telemetry do
      live "/events", EventIndexLive, :index
      live "/stats", StatsLive, :index
    end
  end
end
