defmodule CompleteWeb.TelemetryController do
  use Phoenix.Controller, formats: [:json]

  import Plug.Conn

  alias Complete.{Telemetry, TelemetryEvent}

  @fields ~w(request_id language file_path prompt response latency_ms provider model finish_reason outcome)

  def create(conn, params) do
    changeset = TelemetryEvent.changeset(%TelemetryEvent{}, Map.take(params, @fields))

    case Complete.Repo.insert(changeset) do
      {:ok, _} ->
        conn |> put_status(:accepted) |> json(%{})

      {:error, _} ->
        conn
        |> put_status(:bad_request)
        |> json(%{error: %{code: "invalid_request", message: "invalid event payload"}})
    end
  end

  def index(conn, _params) do
    events =
      Telemetry.recent_events(50)
      |> Enum.map(&event_json/1)

    json(conn, %{events: events})
  end

  def stats(conn, _params) do
    json(conn, %{
      count: Telemetry.count(),
      avg_latency_ms: Telemetry.avg_latency_ms(),
      by_language: Telemetry.by_language(),
      requests_non_cancelled: Telemetry.count_non_cancelled(),
      acceptance_rate: Telemetry.acceptance_rate()
    })
  end

  defp event_json(e) do
    %{
      request_id: e.request_id,
      language: e.language,
      file_path: e.file_path,
      prompt: e.prompt,
      response: e.response,
      latency_ms: e.latency_ms,
      provider: e.provider,
      model: e.model,
      finish_reason: e.finish_reason,
      outcome: e.outcome,
      inserted_at: e.inserted_at
    }
  end
end
