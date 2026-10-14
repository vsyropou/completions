defmodule Complete.Application do
  @moduledoc """
  OTP application entry point. Starts the supervision tree:

    * `Finch` (HTTP client pool named `Complete.Finch`) — used only by
      `Complete.FimAdapter` when `INFERENCE_BASE_URL` is set.
    * `CompleteWeb.Endpoint` — the Phoenix HTTP endpoint serving
      `POST /api/v1/completions`.

  Both children restart independently under a `:one_for_one` strategy.
  """
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      {Finch, name: Complete.Finch},
      CompleteWeb.Endpoint
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: Complete.Supervisor)
  end

  @impl true
  def config_change(changed, _new, removed) do
    CompleteWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
