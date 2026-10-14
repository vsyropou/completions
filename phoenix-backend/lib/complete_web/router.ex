defmodule CompleteWeb.Router do
  @moduledoc """
  HTTP routing. Single API pipeline (`:api`) enforces JSON content
  negotiation and `CompleteWeb.Plugs.Auth` bearer-token auth, then
  scopes one route:

    * `POST /api/v1/completions` → `CompleteWeb.CompletionController.create/2`
  """
  use CompleteWeb, :router

  pipeline :api do
    plug :accepts, ["json"]
    plug CompleteWeb.Plugs.Auth
  end

  scope "/api", CompleteWeb do
    pipe_through :api

    post "/v1/completions", CompletionController, :create
  end
end
