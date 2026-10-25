defmodule CompleteWeb.TelemetryController do
  use CompleteWeb, :controller

  alias Complete.Telemetry

  @required ~w(request_id language file_path prompt response latency_ms provider model finish_reason outcome)
  @outcomes ~w(requested served superseded accepted cancelled error)

  @doc """
  `POST /api/v1/events`. Validates the body shape and
  persists the event. Responds `202` with `{}`.
  """
  def create(conn, params) do
    case validate(params) do
      :ok ->
        attrs = %{
          request_id: params["request_id"],
          language: params["language"],
          file_path: params["file_path"],
          prompt: params["prompt"],
          response: params["response"],
          latency_ms: params["latency_ms"],
          provider: params["provider"],
          model: params["model"],
          finish_reason: params["finish_reason"],
          outcome: params["outcome"]
        }

        case Telemetry.log_event(attrs) do
          {:ok, _event} ->
            conn |> put_status(:accepted) |> json(%{})

          {:error, _changeset} ->
            conn
            |> put_status(:bad_request)
            |> json(%{error: %{code: "invalid_request", message: "failed to persist event"}})
        end

      {:error, message} ->
        conn
        |> put_status(:bad_request)
        |> json(%{error: %{code: "invalid_request", message: message}})
    end
  end

  @doc "GET /api/v1/events → newest 50 persisted events."
  def index(conn, _params) do
    events =
      Telemetry.recent_events(50)
      |> Enum.map(&event_json/1)

    json(conn, %{events: events})
  end

  @doc "GET /api/v1/stats → aggregated latency/language stats."
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

  # All string fields must be binaries; `latency_ms` must be a non-negative
  # integer; `outcome` must be a known value. Returns `:ok` or
  # `{:error, msg}` naming the first bad field.
  defp validate(params) do
    Enum.reduce_while(@required, :ok, fn key, :ok ->
      v = Map.get(params, key)

      cond do
        key == "latency_ms" ->
          if(is_integer(v) and v >= 0,
            do: {:cont, :ok},
            else: {:halt, {:error, "missing or invalid required field: #{key}"}}
          )

        key == "outcome" ->
          if(v in @outcomes,
            do: {:cont, :ok},
            else: {:halt, {:error, "missing or invalid required field: #{key}"}}
          )

        true ->
          if(is_binary(v),
            do: {:cont, :ok},
            else: {:halt, {:error, "missing or invalid required field: #{key}"}}
          )
      end
    end)
  end
end
