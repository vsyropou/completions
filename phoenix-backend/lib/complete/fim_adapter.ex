defmodule Complete.FimAdapter do
  @moduledoc """
  Real inference provider for Phase 3. Talks to an OpenAI-compatible
  completions endpoint (`POST {INFERENCE_BASE_URL}/v1/completions`) with a
  FIM (fill-in-the-middle) prompt assembled here. Phoenix is the ONLY
  place FIM tokenization lives.

  ## Prompt layout

      # Path: <file_path>
      [# Related symbols:
      <repository_context>
      ]
      <|fim_prefix|><prefix><|fim_suffix|><suffix><|fim_middle|>

  Repository context is *prepended* as a delimited comment block, not
  spliced into `prefix`/`suffix`. Template overridable via
  `INFERENCE_FIM_TEMPLATE` with `{prefix}` / `{suffix}` placeholders.

  ## Error contract

  All upstream failures (timeout, 5xx, connect refused, malformed body)
  return `%{completion: "", finish_reason: "error", model: model}`.
  The gateway's caller maps that to HTTP 502 per the frozen protocol.
  """

  require Logger

  @type request :: Complete.CompletionGateway.request()
  @type result :: Complete.CompletionGateway.result()

  @doc """
  Builds the prompt sent to the upstream `prompt` field. Pure; exported
  for tests.
  """
  def build_prompt(%{
        file_path: file_path,
        prefix: prefix,
        suffix: suffix,
        repository_context: repository_context
      }) do
    header = "# Path: #{file_path}\n"

    repo_block =
      if repository_context == "" do
        ""
      else
        "# Related symbols:\n#{repository_context}\n"
      end

    (header <> repo_block <> fim_template_from_env()) |> render_fim(prefix, suffix)
  end

  defp render_fim(template, prefix, suffix) do
    template
    |> String.replace("{prefix}", prefix)
    |> String.replace("{suffix}", suffix)
  end

  # Read the template at call time so `Application.put_env` in tests takes
  # effect without recompilation.
  defp fim_template_from_env do
    (Application.get_env(:complete, :inference) || [])
    |> Keyword.get(
      :fim_template,
      "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>"
    )
  end

  @doc """
  Calls the upstream. Returns a completion map; never raises.
  """
  def generate(request) do
    cfg = Application.fetch_env!(:complete, :inference)
    prompt = build_prompt(request)
    url = cfg[:base_url] <> "/v1/completions"

    # Clamp per-request max_tokens by upstream default cap, both in < 1..1024 >.
    max_tokens = min(request.max_tokens, cfg[:max_tokens])

    body =
      Jason.encode!(%{
        model: cfg[:model],
        prompt: prompt,
        max_tokens: max_tokens,
        temperature: cfg[:temperature],
        stop: []
      })

    headers = [{"content-type", "application/json"}]
    finch_opts = [receive_timeout: cfg[:timeout_ms], pool_timeout: cfg[:timeout_ms]]

    req = Finch.build(:post, url, headers, body)

    case Finch.request(req, Complete.Finch, finch_opts) do
      {:ok, %Finch.Response{status: status, body: resp_body}} when status in 200..299 ->
        parse_success(resp_body, cfg[:model])

      {:ok, %Finch.Response{status: status}} ->
        log_upstream_error("http_#{status}")
        error_result(cfg[:model])

      {:error, reason} ->
        log_upstream_error(reason)
        error_result(cfg[:model])
    end
  end

  defp parse_success(body, model) do
    case Jason.decode(body) do
      {:ok, %{"choices" => [%{"text" => text, "finish_reason" => finish_reason} | _]}}
      when is_binary(text) ->
        %{completion: text, finish_reason: map_finish_reason(finish_reason), model: model}

      {:ok, %{"choices" => [%{"text" => text} | _]}} when is_binary(text) ->
        # Some servers omit finish_reason; treat as "stop".
        %{completion: text, finish_reason: "stop", model: model}

      _ ->
        log_upstream_error("malformed_upstream_body")
        error_result(model)
    end
  end

  defp map_finish_reason("stop"), do: "stop"
  defp map_finish_reason("length"), do: "length"
  defp map_finish_reason(_), do: "error"

  defp error_result(model), do: %{completion: "", finish_reason: "error", model: model}

  # One-line telemetry-ish log per failed upstream call.
  defp log_upstream_error(reason) do
    Logger.warning("upstream_inference_error reason=#{inspect(reason)}")
    :telemetry.execute([:complete, :inference, :error], %{count: 1}, %{reason: inspect(reason)})
  end
end
