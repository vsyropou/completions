defmodule CompleteWeb.TelemetryControllerTest do
  use CompleteWeb.ConnCase, async: false

  alias Complete.Telemetry

  @valid %{
    "request_id" => "r1",
    "language" => "typescript",
    "file_path" => "src/index.ts",
    "prompt" => "prefix",
    "response" => "completion",
    "latency_ms" => 42,
    "provider" => "mock",
    "model" => "m1",
    "finish_reason" => "stop",
    "outcome" => "served"
  }

  setup do
    pid = Ecto.Adapters.SQL.Sandbox.start_owner!(Complete.Repo, shared: true)
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.stop_owner(pid) end)
    :ok
  end

  test "401 without bearer", %{conn: conn} do
    conn = post(conn, "/api/v1/events", @valid)
    assert conn.status == 401
    assert json_response(conn, 401)["error"]["code"] == "unauthorized"
  end

  test "401 on GETs without bearer", %{conn: conn} do
    assert get(conn, "/api/v1/events").status == 401
    assert get(conn, "/api/v1/stats").status == 401
  end

  test "202 on happy path and event is stored", %{conn: conn} do
    conn = authed_post(conn, @valid)
    assert conn.status == 202
    assert json_response(conn, 202) == %{}

    [ev] = Telemetry.recent_events(1)
    assert ev.request_id == "r1"
    assert ev.latency_ms == 42
  end

  test "400 on missing required field", %{conn: conn} do
    body = Map.delete(@valid, "model")
    conn = authed_post(conn, body)
    assert conn.status == 400
    assert json_response(conn, 400)["error"]["code"] == "invalid_request"
  end

  test "400 on non-integer latency_ms", %{conn: conn} do
    conn = authed_post(conn, Map.put(@valid, "latency_ms", "soon"))
    assert conn.status == 400
  end

  test "400 on negative latency_ms", %{conn: conn} do
    conn = authed_post(conn, Map.put(@valid, "latency_ms", -1))
    assert conn.status == 400
  end

  test "400 on bad outcome value", %{conn: conn} do
    conn = authed_post(conn, Map.put(@valid, "outcome", "bogus"))
    assert conn.status == 400
  end

  test "400 on missing outcome", %{conn: conn} do
    conn = authed_post(conn, Map.delete(@valid, "outcome"))
    assert conn.status == 400
  end

  test "GET index returns newest 50 events", %{conn: conn} do
    for i <- 1..60 do
      {:ok, _} =
        Telemetry.log_event(%{
          request_id: "req-#{i}",
          language: "ts",
          file_path: "f",
          prompt: "p",
          response: "r",
          latency_ms: i,
          provider: "m",
          model: "m",
          finish_reason: "stop",
          outcome: "served"
        })
    end

    conn = conn |> authed() |> get("/api/v1/events")
    assert conn.status == 200
    %{"events" => events} = json_response(conn, 200)
    assert length(events) == 50
    assert List.first(events)["request_id"] == "req-60"
  end

  test "GET stats aggregates", %{conn: conn} do
    {:ok, _} =
      Telemetry.log_event(%{
        request_id: "a",
        language: "typescript",
        file_path: "f",
        prompt: "p",
        response: "r",
        latency_ms: 10,
        provider: "m",
        model: "m",
        finish_reason: "stop",
        outcome: "served"
      })

    {:ok, _} =
      Telemetry.log_event(%{
        request_id: "b",
        language: "typescript",
        file_path: "f",
        prompt: "p",
        response: "r",
        latency_ms: 30,
        provider: "m",
        model: "m",
        finish_reason: "stop",
        outcome: "served"
      })

    conn = conn |> authed() |> get("/api/v1/stats")
    assert conn.status == 200
    body = json_response(conn, 200)
    assert body["count"] == 2
    assert body["avg_latency_ms"] == 20.0
    assert body["by_language"] == %{"typescript" => 2}
    assert body["requests_non_cancelled"] == 2
    assert body["acceptance_rate"] == 0.0
  end

  defp authed(conn) do
    put_req_header(conn, "authorization", "Bearer dev-token")
  end

  defp authed_post(conn, body) do
    conn
    |> authed()
    |> put_req_header("content-type", "application/json")
    |> post("/api/v1/events", Jason.encode!(body))
  end
end
