defmodule CompleteWeb.StatsLive do
  @moduledoc "Aggregate stats."
  use CompleteWeb, :live_view

  alias Complete.Telemetry

  @impl true
  def mount(_params, _session, socket) do
    {:ok, refresh(socket)}
  end

  @impl true
  def handle_event("refresh", _params, socket) do
    {:noreply, refresh(socket)}
  end

  defp refresh(socket) do
    assign(socket,
      requests_non_cancelled: Telemetry.count_non_cancelled(),
      acceptance_rate: Telemetry.acceptance_rate(),
      by_language: Telemetry.by_language()
    )
  end

  @impl true
  def render(assigns) do
    ~H"""
    <h1>Stats</h1>

    <div class="cards">
      <div class="card">
        <div class="label">Non-cancelled requests</div>
        <div class="big"><%= @requests_non_cancelled %></div>
      </div>
      <div class="card">
        <div class="label">Acceptance rate</div>
        <div class="big"><%= @acceptance_rate %>%</div>
        <div class="bar">
          <div class="fill" style={"width: #{min(@acceptance_rate, 100.0)}%;"}></div>
        </div>
      </div>
    </div>

    <button phx-click="refresh">Refresh</button>

    <h2>By language</h2>
    <table>
      <thead>
        <tr><th>language</th><th>count</th></tr>
      </thead>
      <tbody>
        <tr :for={{lang, n} <- Enum.sort_by(@by_language, &elem(&1, 1), :desc)}>
          <td><%= lang %></td>
          <td><%= n %></td>
        </tr>
      </tbody>
    </table>
    """
  end
end
