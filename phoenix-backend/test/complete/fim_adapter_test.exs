defmodule Complete.FimAdapterTest do
  use ExUnit.Case, async: false
  use CompleteWeb.ConnCase

  import Plug.Conn

  @token "dev-token"

  @request %{
    request_id: "req-1",
    language: "typescript",
    file_path: "src/main.ts",
    prefix: "function calculate() {\n",
    suffix: "\n}",
    repository_context: "",
    max_tokens: 128
  }

  # FimAdapter.generate/1 reads `Application.get_env(:complete, :inference)`
  # and CompletionGateway picks the provider based on the same key. Mutate
  # it per-test, restore on exit.
  setup do
    original = Application.get_env(:complete, :inference)
    on_exit(fn -> Application.put_env(:complete, :inference, original) end)
    :ok
  end

  describe "build_prompt/1" do
    test "FIM tokens appear in order with repository block prepended" do
      Application.put_env(:complete, :inference, base_url: "http://x", model: "m")

      prompt =
        Complete.FimAdapter.build_prompt(%{
          file_path: "src/foo.ts",
          prefix: "function a() {\n",
          suffix: "\n}",
          repository_context: "symbolA\nsymbolB"
        })

      assert is_binary(prompt)
      assert prompt =~ "# Path: src/foo.ts"
      assert prompt =~ "# Related symbols:"
      assert prompt =~ "symbolA"

      idx_path = :binary.match(prompt, "# Path:") |> elem(0)
      idx_repo = :binary.match(prompt, "# Related symbols:") |> elem(0)
      idx_fpre = :binary.match(prompt, "<|fim_prefix|>") |> elem(0)
      idx_fsuf = :binary.match(prompt, "<|fim_suffix|>") |> elem(0)
      idx_fmid = :binary.match(prompt, "<|fim_middle|>") |> elem(0)

      assert idx_path < idx_repo
      assert idx_repo < idx_fpre
      assert idx_fpre < idx_fsuf
      assert idx_fsuf < idx_fmid

      assert prompt =~ "function a() {\n"
      assert prompt =~ "\n}"
    end

    test "omits repository block when context is empty" do
      Application.put_env(:complete, :inference, base_url: "http://x", model: "m")

      prompt =
        Complete.FimAdapter.build_prompt(%{
          file_path: "src/x.ts",
          prefix: "p",
          suffix: "s",
          repository_context: ""
        })

      refute prompt =~ "# Related symbols:"
      assert prompt =~ "<|fim_prefix|>p<|fim_suffix|>s<|fim_middle|>"
    end
  end

  describe "gateway provider selection" do
    test "falls back to MockProvider when INFERENCE_BASE_URL unset" do
      Application.delete_env(:complete, :inference)
      result = Complete.CompletionGateway.generate(@request)
      assert result.model == "mock"
      assert result.completion =~ "mock completion"
    end
  end

  describe "end-to-end through controller with fake upstream (Bypass)" do
    setup %{conn: conn} do
      bypass = Bypass.open()
      base_url = "http://localhost:#{bypass.port}"

      Application.put_env(:complete, :inference,
        base_url: base_url,
        model: "qwen-test",
        fim_template: "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>",
        max_tokens: 512,
        temperature: 0.0,
        timeout_ms: 5_000
      )

      {:ok, conn: conn, bypass: bypass, base_url: base_url}
    end

    test "200 upstream returns completion, controller returns contract body", %{
      conn: conn,
      bypass: bypass
    } do
      test_pid = self()

      Bypass.expect_once(bypass, "POST", "/v1/completions", fn upstream_conn ->
        {:ok, body, upstream_conn} = Plug.Conn.read_body(upstream_conn)
        decoded = Jason.decode!(body)
        send(test_pid, {:upstream_request, decoded})

        upstream_conn
        |> Plug.Conn.put_resp_content_type("application/json")
        |> Plug.Conn.resp(
          200,
          Jason.encode!(%{
            "choices" => [%{"text" => "ok();", "finish_reason" => "stop"}]
          })
        )
      end)

      conn =
        conn
        |> put_req_header("authorization", "Bearer #{@token}")
        |> put_req_header("content-type", "application/json")
        |> Phoenix.ConnTest.dispatch(
          CompleteWeb.Endpoint,
          :post,
          "/api/v1/completions",
          Jason.encode!(%{
            "request_id" => "r1",
            "language" => "typescript",
            "file_path" => "src/a.ts",
            "prefix" => "x",
            "suffix" => "y",
            "repository_context" => "",
            "max_tokens" => 64
          })
        )

      assert %{
               "request_id" => "r1",
               "completion" => "ok();",
               "finish_reason" => "stop",
               "model" => "qwen-test"
             } = Phoenix.ConnTest.json_response(conn, 200)

      assert_received {:upstream_request, upstream}
      assert upstream["model"] == "qwen-test"
      assert upstream["max_tokens"] == 64
      assert upstream["temperature"] == 0.0
      assert upstream["prompt"] =~ "<|fim_prefix|>x"
      assert upstream["prompt"] =~ "<|fim_suffix|>y"
      assert upstream["prompt"] =~ "<|fim_middle|>"
    end

    test "503 upstream → controller returns 502 upstream_error", %{conn: conn, bypass: bypass} do
      Bypass.expect_once(bypass, "POST", "/v1/completions", fn upstream_conn ->
        Plug.Conn.resp(upstream_conn, 503, "service unavailable")
      end)

      conn =
        conn
        |> put_req_header("authorization", "Bearer #{@token}")
        |> put_req_header("content-type", "application/json")
        |> Phoenix.ConnTest.dispatch(
          CompleteWeb.Endpoint,
          :post,
          "/api/v1/completions",
          Jason.encode!(%{
            "request_id" => "r2",
            "language" => "typescript",
            "file_path" => "src/a.ts",
            "prefix" => "x",
            "suffix" => "y",
            "repository_context" => "",
            "max_tokens" => 64
          })
        )

      assert %{"error" => %{"code" => "upstream_error"}} =
               Phoenix.ConnTest.json_response(conn, 502)
    end

    test "upstream connection refused → 502 upstream_error", %{conn: conn} do
      # Point to a port that's definitely not listening.
      Application.put_env(:complete, :inference,
        base_url: "http://localhost:59999",
        model: "qwen-test",
        max_tokens: 512,
        temperature: 0.0,
        timeout_ms: 2_000
      )

      conn =
        conn
        |> put_req_header("authorization", "Bearer #{@token}")
        |> put_req_header("content-type", "application/json")
        |> Phoenix.ConnTest.dispatch(
          CompleteWeb.Endpoint,
          :post,
          "/api/v1/completions",
          Jason.encode!(%{
            "request_id" => "r3",
            "language" => "typescript",
            "file_path" => "src/a.ts",
            "prefix" => "x",
            "suffix" => "y",
            "repository_context" => "",
            "max_tokens" => 64
          })
        )

      assert %{"error" => %{"code" => "upstream_error"}} =
               Phoenix.ConnTest.json_response(conn, 502)
    end
  end
end
