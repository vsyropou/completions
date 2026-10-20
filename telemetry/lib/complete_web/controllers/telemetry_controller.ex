defmodule CompleteWeb.TelemetryController do
  use CompleteWeb, :controller

  alias Complete.TelemetryStore

  @required ~w(request_id language file_path prompt response latency_ms provider model finish_reason)

  @doc """
  `POST /api/v1/completions`. Validates the body shape and
  appends the event to `Complete.TelemetryStore`. Responds `202` with
  `{}`.
  """
  def create(conn, params) do
    case validate(params) do
      :ok ->
        TelemetryStore.add(%{
          request_id: params["request_id"],
          language: params["language"],
          file_path: params["file_path"],
          prompt: params["prompt"],
          response: params["response"],
          latency_ms: params["latency_ms"],
          provider: params["provider"],
          model: params["model"],
          finish_reason: params["finish_reason"]
        })

        conn |> put_status(:accepted) |> json(%{})

      {:error, message} ->
        conn
        |> put_status(:bad_request)
        |> json(%{error: %{code: "invalid_request", message: message}})
    end
  end

  @doc "GET /api/v1/completions → newest 50 buffered events."
  def index(conn, _params) do
    json(conn, %{events: TelemetryStore.recent(50)})
  end

  @doc "GET /api/v1/stats → aggregated latency/language stats."
  def stats(conn, _params) do
    json(conn, TelemetryStore.stats())
  end

  # All string fields must be binaries; `latency_ms` must be a non-negative
  # integer. Returns `:ok` or `{:error, msg}` naming the first bad field.
  defp validate(params) do
    Enum.reduce_while(@required, :ok, fn key, :ok ->
      v = Map.get(params, key)

      cond do
        key == "latency_ms" ->
          if(is_integer(v) and v >= 0,
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
