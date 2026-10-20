defmodule Complete.TelemetryStore do
  @cap 500

  @moduledoc """
  In-memory buffer for completion telemetry events posted by the sidecar.

  Events are prepended (newest first), capped at #{@cap} — the oldest are
  dropped. Read side exposes `recent/1`, `count/0`, and `stats/0` for the
  telemetry API.

  # ponytail: in-memory 500-event cap; swap for SQLite when the cap proves small.
  """
  use Agent

  @type event :: %{
          request_id: String.t(),
          language: String.t(),
          file_path: String.t(),
          prompt: String.t(),
          response: String.t(),
          latency_ms: non_neg_integer(),
          provider: String.t(),
          model: String.t(),
          finish_reason: String.t()
        }

  def start_link(_opts) do
    Agent.start_link(fn -> [] end, name: __MODULE__)
  end

  @doc "Appends `event` at the head, trimming to the #{@cap}-event cap."
  @spec add(event()) :: :ok
  def add(event) do
    Agent.update(__MODULE__, fn events -> [event | events] |> Enum.take(@cap) end)
  end

  @doc "Returns up to `n` most-recent events, newest first."
  @spec recent(non_neg_integer()) :: [event()]
  def recent(n) do
    Agent.get(__MODULE__, &Enum.take(&1, n))
  end

  @doc "Number of events currently buffered."
  @spec count() :: non_neg_integer()
  def count do
    Agent.get(__MODULE__, &length/1)
  end

  @doc """
  Aggregates across buffered events:

    * `count`
    * `avg_latency_ms` — float, `0.0` when empty
    * `by_language` — map of `language => count`
  """
  @spec stats() :: %{count: non_neg_integer(), avg_latency_ms: float(), by_language: map()}
  def stats do
    Agent.get(__MODULE__, fn events ->
      count = length(events)

      avg =
        if count > 0 do
          events |> Enum.map(& &1.latency_ms) |> Enum.sum() |> Kernel./(count)
        else
          0.0
        end

      by_language =
        events
        |> Enum.group_by(& &1.language)
        |> Map.new(fn {lang, evs} -> {lang, length(evs)} end)

      %{count: count, avg_latency_ms: avg, by_language: by_language}
    end)
  end
end
