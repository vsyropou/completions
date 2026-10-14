defmodule CompleteWeb.CompletionController do
  use CompleteWeb, :controller

  @required ~w(request_id language file_path prefix suffix repository_context)

  @doc """
  `POST /api/v1/completions`. Validates the body against the frozen
  protocol, delegates inference to `Complete.CompletionGateway.generate/1`,
  and shapes the response.

  Responses:

    * `200` — completion JSON
    * `400` `invalid_request` — missing/non-string required field, malformed `max_tokens`
    * `422` `validation_failed` — `max_tokens` outside `1..1024`
    * `502` `upstream_error` — provider returned `finish_reason: "error"`

  Add new validation rules as a `validate_*` clause threaded through the
  `with` below; each returns `{:error, http_status, code, message}` and
  short-circuits to the 4xx JSON body.
  """
  def create(conn, params) do
    with :ok <- validate_required(params),
         :ok <- validate_max_tokens_type(params["max_tokens"]),
         :ok <- validate_max_tokens_range(params["max_tokens"]) do
      request = %{
        request_id: params["request_id"],
        language: params["language"],
        file_path: params["file_path"],
        prefix: params["prefix"],
        suffix: params["suffix"],
        repository_context: params["repository_context"],
        max_tokens: params["max_tokens"]
      }

      case Complete.CompletionGateway.generate(request) do
        %{finish_reason: "error"} ->
          conn
          |> put_status(:bad_gateway)
          |> json(%{error: %{code: "upstream_error", message: "inference provider failed"}})

        %{completion: completion, finish_reason: reason, model: model} ->
          json(conn, %{
            request_id: request.request_id,
            completion: completion,
            finish_reason: reason,
            model: model
          })
      end
    else
      {:error, status, code, message} ->
        conn
        |> put_status(status)
        |> json(%{error: %{code: code, message: message}})
    end
  end

  defp validate_required(params) do
    case Enum.find(@required, fn key -> not is_binary(params[key]) end) do
      nil ->
        :ok

      key ->
        {:error, 400, "invalid_request", "missing or invalid required field: #{key}"}
    end
  end

  defp validate_max_tokens_type(nil),
    do: {:error, 400, "invalid_request", "missing required field: max_tokens"}

  defp validate_max_tokens_type(i) when is_integer(i), do: :ok

  defp validate_max_tokens_type(_),
    do: {:error, 400, "invalid_request", "max_tokens must be an integer"}

  defp validate_max_tokens_range(max_tokens) when max_tokens in 1..1024, do: :ok

  defp validate_max_tokens_range(max_tokens) do
    {:error, 422, "validation_failed",
     "max_tokens must be between 1 and 1024, got: #{max_tokens}"}
  end
end
