defmodule Complete.TelemetryStoreTest do
  use ExUnit.Case, async: false

  alias Complete.TelemetryStore

  @event %{
    request_id: "r1",
    language: "typescript",
    file_path: "src/index.ts",
    prompt: "prefix",
    response: "completion",
    latency_ms: 42,
    provider: "mock",
    model: "m1",
    finish_reason: "stop"
  }

  setup do
    # TelemetryStore is supervised by the app; reset its state between tests
    # by filtering to empty.
    Agent.update(TelemetryStore, fn _ -> [] end)
    :ok
  end

  test "add/1 stores events newest-first; recent/1 returns them" do
    :ok = TelemetryStore.add(@event)
    :ok = TelemetryStore.add(%{@event | request_id: "r2"})

    assert [e2, e1] = TelemetryStore.recent(10)
    assert e2.request_id == "r2"
    assert e1.request_id == "r1"
  end

  test "caps buffer at 500, dropping the oldest" do
    for i <- 1..600 do
      TelemetryStore.add(%{@event | request_id: "req-#{i}"})
    end

    assert TelemetryStore.count() == 500
    ids = TelemetryStore.recent(500) |> Enum.map(& &1.request_id)
    # Newest first, oldest surviving is req-101.
    assert List.first(ids) == "req-600"
    assert List.last(ids) == "req-101"
  end

  test "recent/1 respects n" do
    for i <- 1..10 do
      TelemetryStore.add(%{@event | request_id: "req-#{i}"})
    end

    assert length(TelemetryStore.recent(3)) == 3
  end

  test "stats/0 aggregates count, latency avg, language buckets" do
    TelemetryStore.add(%{@event | latency_ms: 10, language: "typescript"})
    TelemetryStore.add(%{@event | latency_ms: 20, language: "typescript"})
    TelemetryStore.add(%{@event | latency_ms: 30, language: "elixir"})

    stats = TelemetryStore.stats()

    assert stats.count == 3
    assert stats.avg_latency_ms == 20.0
    assert stats.by_language == %{"typescript" => 2, "elixir" => 1}
  end

  test "stats/0 on empty buffer is zero" do
    stats = TelemetryStore.stats()
    assert stats.count == 0
    assert stats.avg_latency_ms == 0.0
    assert stats.by_language == %{}
  end
end
