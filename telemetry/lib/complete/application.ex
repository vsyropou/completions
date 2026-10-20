defmodule Complete.Application do
  @moduledoc """
  OTP application entry point. Supervision tree (one-for-one):

    * `Complete.TelemetryStore` — in-memory telemetry event buffer.
    * `CompleteWeb.Endpoint` — the Phoenix HTTP endpoint serving
      `/api/v1/telemetry/*`.
  """
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      Complete.TelemetryStore,
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
