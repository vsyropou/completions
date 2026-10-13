defmodule Complete.Application do
  @moduledoc false
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
