defmodule CompleteWeb.Router do
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
