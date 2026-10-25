defmodule Complete.TelemetryTest do
  use Complete.DataCase, async: false

  alias Complete.Telemetry

  @event %{
    request_id: "r1",
    language: "typescript",
    file_path: "src/index.ts",
    prompt: "prefix",
    response: "completion",
    latency_ms: 42,
    provider: "mock",
    model: "m1",
    finish_reason: "stop",
    outcome: "served"
  }

  test "log_event/1 persists an event" do
    assert {:ok, ev} = Telemetry.log_event(@event)
    assert ev.request_id == "r1"
    assert ev.outcome == "served"
  end

  test "log_event/1 rejects invalid outcome" do
    assert {:error, changeset} =
             Telemetry.log_event(%{@event | outcome: "bogus"})

    assert %{outcome: _} = errors_on(changeset)
  end

  test "log_event/1 accepts empty-string secondary fields (requested/accepted shape)" do
    # Empty strings are the "not applicable" marker for prompt/response/model/
    # finish_reason on non-served outcomes. The controller validates presence-
    # as-binary upstream; the changeset must not reject the empties.
    req_or_acc = %{
      @event
      | prompt: "",
        response: "",
        model: "",
        finish_reason: "",
        latency_ms: 0,
        outcome: "requested"
    }

    assert {:ok, _} = Telemetry.log_event(req_or_acc)
    assert {:ok, _} = Telemetry.log_event(%{req_or_acc | outcome: "accepted"})
  end

  test "recent_events/1 returns newest first, capped" do
    for i <- 1..5 do
      {:ok, _} = Telemetry.log_event(%{@event | request_id: "req-#{i}", latency_ms: i})
      # ensure clock differences when timestamps tie at usec
      Process.sleep(2)
    end

    events = Telemetry.recent_events(3)
    assert length(events) == 3
    assert List.first(events).request_id == "req-5"
  end

  test "search_events/1 with q free-text matches prompt/response" do
    {:ok, _} = Telemetry.log_event(%{@event | prompt: "compute fibonacci sequence"})
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r2", prompt: "render a button"})

    hits = Telemetry.search_events(q: "fibonacci")
    assert length(hits) == 1
    assert hd(hits).request_id == "r1"
  end

  test "search_events/1 filters by language, provider, outcome" do
    {:ok, _} = Telemetry.log_event(@event)
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r2", language: "elixir"})
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r3", outcome: "cancelled"})
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r4", provider: "vllm"})

    assert [e] = Telemetry.search_events(language: "elixir")
    assert e.request_id == "r2"

    assert [e] = Telemetry.search_events(outcome: "cancelled")
    assert e.request_id == "r3"

    assert [e] = Telemetry.search_events(provider: "vllm")
    assert e.request_id == "r4"

    # combined
    ids =
      Telemetry.search_events(language: "typescript", outcome: "served")
      |> Enum.map(& &1.request_id)

    assert Enum.sort(ids) == ["r1", "r4"]
  end

  test "aggregates with mixed fixture: 10 served, 4 accepted, 2 superseded, 1 cancelled" do
    # 10 distinct served requests → request ids s1..s10
    for i <- 1..10 do
      {:ok, _} = Telemetry.log_event(%{@event | request_id: "s#{i}", outcome: "served"})
    end

    # 4 of them also accepted
    for i <- 1..4 do
      {:ok, _} = Telemetry.log_event(%{@event | request_id: "s#{i}", outcome: "accepted"})
    end

    # 2 more requests only superseded
    for i <- 1..2 do
      {:ok, _} = Telemetry.log_event(%{@event | request_id: "x#{i}", outcome: "superseded"})
    end

    # 1 request only cancelled
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "c1", outcome: "cancelled"})

    # 10 served + 4 accepted, distinct = 10 (superseded/cancelled excluded)
    assert Telemetry.count_non_cancelled() == 10
    assert Telemetry.acceptance_rate() == 40.0
  end

  test "acceptance_rate/0 returns 0.0 when no served" do
    {:ok, _} = Telemetry.log_event(%{@event | outcome: "accepted"})
    assert Telemetry.acceptance_rate() == 0.0
  end

  test "count/0, avg_latency_ms/0, by_language/0" do
    {:ok, _} = Telemetry.log_event(%{@event | latency_ms: 10, language: "typescript"})
    {:ok, _} = Telemetry.log_event(%{@event | latency_ms: 30, language: "typescript"})

    assert Telemetry.count() == 2
    assert Telemetry.avg_latency_ms() == 20.0
    assert Telemetry.by_language() == %{"typescript" => 2}
  end

  test "distinct_languages/0" do
    {:ok, _} = Telemetry.log_event(%{@event | language: "typescript"})
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r2", language: "elixir"})
    {:ok, _} = Telemetry.log_event(%{@event | request_id: "r3", language: "elixir"})

    langs = Telemetry.distinct_languages() |> Enum.sort()
    assert langs == ["elixir", "typescript"]
  end
end
