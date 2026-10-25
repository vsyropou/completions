defmodule CompleteWeb.EventIndexLiveTest do
  use CompleteWeb.ConnCase, async: false
  import Phoenix.LiveViewTest

  alias Complete.Telemetry

  setup do
    # Shared sandbox so the LiveView process can read what the test inserts.
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

  test "GET /events renders seeded rows", %{conn: conn} do
    seed(%{request_id: "seed-1", file_path: "foo/bar.ts"})
    {:ok, _view, html} = live(conn, "/events")
    assert html =~ "seed-1"
    assert html =~ "foo/bar.ts"
    assert html =~ "served"
  end

  test "filter by language hides other languages", %{conn: conn} do
    seed(%{request_id: "ts-1", language: "typescript", file_path: "a.ts"})
    seed(%{request_id: "ex-1", language: "elixir", file_path: "b.ex"})

    {:ok, view, _html} = live(conn, "/events")

    html =
      view
      |> element("form")
      |> render_submit(%{q: "", language: "elixir", outcome: ""})

    assert html =~ "ex-1"
    refute html =~ "ts-1"
  end

  test "filter by outcome", %{conn: conn} do
    seed(%{request_id: "ok-1", outcome: "served"})
    seed(%{request_id: "err-1", outcome: "error"})

    {:ok, view, _} = live(conn, "/events")

    html =
      view
      |> element("form")
      |> render_submit(%{q: "", language: "", outcome: "error"})

    assert html =~ "err-1"
    refute html =~ "ok-1"
  end

  test "clicking request_id reconciles to that request only", %{conn: conn} do
    seed(%{request_id: "A", outcome: "requested"})
    seed(%{request_id: "A", outcome: "served"})
    seed(%{request_id: "A", outcome: "accepted"})
    seed(%{request_id: "B", outcome: "served"})

    {:ok, view, _} = live(conn, "/events")
    html = render_click(view, "request", %{"id" => "A"})

    assert html =~ "Reconciling request"
    assert html =~ ~s(>A</a>)
    assert html =~ "requested"
    assert html =~ "accepted"
    # row for request B is gone (id only appears in one place)
    refute html =~ ~s(>B</a>)
  end

  test "pagination with limit/offset", %{conn: conn} do
    for i <- 1..60 do
      seed(%{request_id: "pg-#{i}", latency_ms: i})
    end

    {:ok, view, html} = live(conn, "/events")
    # page 1 shows the latest — pg-60 exists, oldest (pg-1..pg-10) does not
    assert html =~ "pg-60"
    refute html =~ "pg-1<"

    next_html = render_click(view, "next")
    assert next_html =~ "offset 50"
    assert next_html =~ "pg-10"
    refute next_html =~ "pg-60"
  end
end
