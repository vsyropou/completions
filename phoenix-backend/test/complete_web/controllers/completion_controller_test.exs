defmodule CompleteWeb.CompletionControllerTest do
  use CompleteWeb.ConnCase, async: true

  @token "dev-token"
  @valid_body %{
    "request_id" => "3f6c9b42-1a2b-4c5d-8e9f-000000000001",
    "language" => "typescript",
    "file_path" => "src/main.ts",
    "prefix" => "function calculate() {\n",
    "suffix" => "\n}",
    "repository_context" => "",
    "max_tokens" => 128
  }

  defp authed(conn) do
    Plug.Conn.put_req_header(conn, "authorization", "Bearer #{@token}")
  end

  defp post_json(conn, body) do
    conn
    |> Plug.Conn.put_req_header("content-type", "application/json")
    |> Phoenix.ConnTest.dispatch(
      CompleteWeb.Endpoint,
      :post,
      "/api/v1/completions",
      Jason.encode!(body)
    )
  end

  test "200 echoes request_id, returns mock completion", %{conn: conn} do
    conn = conn |> authed() |> post_json(@valid_body)

    expected_id = @valid_body["request_id"]

    assert %{
             "request_id" => ^expected_id,
             "completion" => completion,
             "finish_reason" => "stop",
             "model" => "mock"
           } = Phoenix.ConnTest.json_response(conn, 200)

    assert completion =~ "mock completion"
    assert completion =~ "\n"
  end

  test "401 without token", %{conn: conn} do
    conn = post_json(conn, @valid_body)
    assert %{"error" => %{"code" => "unauthorized"}} = Phoenix.ConnTest.json_response(conn, 401)
  end

  test "401 with bad token", %{conn: conn} do
    conn =
      conn
      |> Plug.Conn.put_req_header("authorization", "Bearer wrong")
      |> post_json(@valid_body)

    assert %{"error" => %{"code" => "unauthorized"}} = Phoenix.ConnTest.json_response(conn, 401)
  end

  test "400 on missing required field", %{conn: conn} do
    body = Map.delete(@valid_body, "prefix")
    conn = conn |> authed() |> post_json(body)

    assert %{"error" => %{"code" => "invalid_request"}} =
             Phoenix.ConnTest.json_response(conn, 400)
  end

  test "400 when max_tokens missing", %{conn: conn} do
    body = Map.delete(@valid_body, "max_tokens")
    conn = conn |> authed() |> post_json(body)

    assert %{"error" => %{"code" => "invalid_request"}} =
             Phoenix.ConnTest.json_response(conn, 400)
  end

  test "422 when max_tokens out of range", %{conn: conn} do
    for bad <- [0, 1025, -1, 999_999] do
      body = Map.put(@valid_body, "max_tokens", bad)
      conn = conn |> authed() |> post_json(body)

      assert %{"error" => %{"code" => "validation_failed"}} =
               Phoenix.ConnTest.json_response(conn, 422),
             "expected 422 for max_tokens=#{bad}"
    end
  end

  test "413 on oversized body", %{conn: _conn} do
    # Endpoint caps at 256 KB; craft a body just over it. Plug.Parsers raises,
    # Phoenix's RenderErrors catches it and sends the 413+JSON via ErrorJSON.
    big_prefix = String.duplicate("x", 262_150)
    body = Map.put(@valid_body, "prefix", big_prefix)
    payload = Jason.encode!(body)

    assert {413, _headers, resp_body} =
             assert_error_sent(413, fn ->
               build_conn()
               |> Plug.Conn.put_req_header("authorization", "Bearer #{@token}")
               |> Plug.Conn.put_req_header("content-type", "application/json")
               |> Phoenix.ConnTest.dispatch(
                 CompleteWeb.Endpoint,
                 :post,
                 "/api/v1/completions",
                 payload
               )
             end)

    assert %{"error" => %{"code" => "too_large"}} = Jason.decode!(resp_body)
  end

  test "mock is deterministic: same input -> same completion", %{conn: conn} do
    c1 = conn |> authed() |> post_json(@valid_body)
    r1 = Phoenix.ConnTest.json_response(c1, 200)

    c2 = conn |> authed() |> post_json(@valid_body)
    r2 = Phoenix.ConnTest.json_response(c2, 200)

    assert r1 == r2
  end
end
