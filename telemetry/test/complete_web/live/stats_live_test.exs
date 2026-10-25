defmodule CompleteWeb.StatsLiveTest do
  use CompleteWeb.ConnCase, async: false
  import Phoenix.LiveViewTest

  alias Complete.Telemetry

  setup do
    pid = Ecto.Adapters.SQL.Sandbox.start_owner!(Complete.Repo, shared: true)
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.stop_owner(pid) end)
    :ok
  end

  defp seed(attrs) do
    base = %{
      request_id: "r1",
      language: "typescript",
      file_path: "src/index.ts",
      prompt: "prefix",
      response: "completion",
      latency_ms: 10,
      provider: "mock",
      model: "m1",
      finish_reason: "stop",
      outcome: "served"
    }

    {:ok, ev} = Telemetry.log_event(Map.merge(base, attrs))
    ev
  end

  test "GET /stats renders 10 non-cancelled and 40.0% for the fixture", %{conn: conn} do
    # fixture: 10 served (s1..s10), 4 accepted (s1..s4), 2 superseded, 1 cancelled
    for i <- 1..10, do: seed(%{request_id: "s#{i}", outcome: "served"})
    for i <- 1..4, do: seed(%{request_id: "s#{i}", outcome: "accepted"})
    seed(%{request_id: "x1", outcome: "superseded"})
    seed(%{request_id: "x2", outcome: "superseded"})
    seed(%{request_id: "c1", outcome: "cancelled"})

    {:ok, _view, html} = live(conn, "/stats")
    assert html =~ "Non-cancelled requests"
    assert html =~ "10"
    assert html =~ "40.0%"
    assert html =~ "Acceptance rate"
    # by-language table shows typescript rows
    assert html =~ "typescript"
  end

  test "refresh event does not crash", %{conn: conn} do
    seed(%{request_id: "k1"})
    {:ok, view, _} = live(conn, "/stats")
    html = render_click(view, "refresh")
    assert html =~ "Non-cancelled requests"
  end
end
