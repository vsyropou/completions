defmodule Complete.Telemetry do
  @moduledoc """
  Telemetry context: persists and aggregates completion events.
  """
  import Ecto.Query
  alias Complete.{Repo, TelemetryEvent}

  @doc "Inserts a telemetry event; returns {:ok, event} | {:error, changeset}."
  def log_event(attrs) do
    %TelemetryEvent{}
    |> TelemetryEvent.changeset(attrs)
    |> Repo.insert()
  end

  @doc "Up to `limit` newest events, newest first."
  def recent_events(limit \\ 50) do
    TelemetryEvent
    |> order_by([e], desc: e.inserted_at, desc: e.id)
    |> limit(^limit)
    |> Repo.all()
  end

  @doc """
  Full-text search over prompt+response.

  Options: `q` (free text), `language`, `provider`, `outcome`, `request_id`,
  `limit` (default 50), `offset` (default 0). Newest first.
  """
  def search_events(opts \\ []) do
    limit = Keyword.get(opts, :limit, 50)
    offset = Keyword.get(opts, :offset, 0)

    TelemetryEvent
    |> maybe_q(opts[:q])
    |> maybe_eq(:language, opts[:language])
    |> maybe_eq(:provider, opts[:provider])
    |> maybe_eq(:outcome, opts[:outcome])
    |> maybe_eq(:request_id, opts[:request_id])
    |> order_by([e], desc: e.inserted_at, desc: e.id)
    |> limit(^limit)
    |> offset(^offset)
    |> Repo.all()
  end

  @doc "Events for one request_id, ordered oldest → newest for lifecycle reading."
  def by_request_id(request_id) do
    TelemetryEvent
    |> where([e], e.request_id == ^request_id)
    |> order_by([e], asc: e.inserted_at, asc: e.id)
    |> Repo.all()
  end

  defp maybe_q(query, nil), do: query
  defp maybe_q(query, ""), do: query

  defp maybe_q(query, q) do
    where(
      query,
      [e],
      fragment(
        "search @@ plainto_tsquery('english', ?)",
        ^q
      )
    )
  end

  defp maybe_eq(query, _field, nil), do: query
  defp maybe_eq(query, field, value), do: where(query, [e], field(e, ^field) == ^value)

  @doc """
  COUNT DISTINCT request_id with any non-cancelled, non-superseded outcome.
  A request counts once if any of requested/served/accepted/error exists.
  """
  def count_non_cancelled do
    TelemetryEvent
    |> where([e], e.outcome not in ["cancelled", "superseded"])
    |> select([e], count(e.request_id, :distinct))
    |> Repo.one()
  end

  @doc "Total number of persisted events."
  def count do
    Repo.aggregate(TelemetryEvent, :count)
  end

  @doc """
  Acceptance rate as a percentage (0.0–100.0): distinct request_ids with an
  `accepted` event divided by distinct request_ids with a `served` event.
  COALESCE-safe: 0.0 when no served requests.
  """
  def acceptance_rate do
    accepted =
      TelemetryEvent
      |> where([e], e.outcome == "accepted")
      |> select([e], count(e.request_id, :distinct))
      |> Repo.one()

    served =
      TelemetryEvent
      |> where([e], e.outcome == "served")
      |> select([e], count(e.request_id, :distinct))
      |> Repo.one()

    if served == 0 do
      0.0
    else
      Float.round(accepted / served * 100.0, 2)
    end
  end

  @doc "Average latency in ms, 0.0 when empty."
  def avg_latency_ms do
    TelemetryEvent
    |> select([e], coalesce(avg(e.latency_ms), 0.0))
    |> Repo.one()
    |> case do
      %Decimal{} = d -> Decimal.to_float(d)
      n when is_float(n) -> n
      _ -> 0.0
    end
  end

  @doc "Map of language => event count."
  def by_language do
    TelemetryEvent
    |> group_by([e], e.language)
    |> select([e], {e.language, count(e.id)})
    |> Repo.all()
    |> Map.new()
  end

  @doc "List of distinct languages seen."
  def distinct_languages do
    TelemetryEvent
    |> distinct(true)
    |> select([e], e.language)
    |> Repo.all()
  end
end
