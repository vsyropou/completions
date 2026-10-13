defmodule Complete.CompletionGateway do
  @moduledoc """
  Single entry point for completion inference. Provider selection:

    * `INFERENCE_BASE_URL` set (via `config :complete, :inference`) →
      `Complete.FimAdapter` (real upstream HTTP).
    * otherwise → `Complete.MockProvider` (Phase 1 deterministic mock).

  Callers (i.e. the controller) never pick a provider and never change
  between phases; only this module routes.
  """

  @type request :: %{
          request_id: String.t(),
          language: String.t(),
          file_path: String.t(),
          prefix: String.t(),
          suffix: String.t(),
          repository_context: String.t(),
          max_tokens: pos_integer()
        }

  @type result :: %{
          completion: String.t(),
          finish_reason: String.t(),
          model: String.t()
        }

  @doc "Returns `%{completion, finish_reason, model}` for a validated request map."
  def generate(request) do
    provider().generate(request)
  end

  defp provider do
    if Application.get_env(:complete, :inference) do
      Complete.FimAdapter
    else
      Complete.MockProvider
    end
  end
end
