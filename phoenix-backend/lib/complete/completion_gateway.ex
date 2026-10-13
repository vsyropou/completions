defmodule Complete.CompletionGateway do
  @moduledoc """
  Single entry point for completion inference. Phase 1 delegates to
  `Complete.MockProvider`; Phase 3 will route to a real HTTP adapter
  (via `Complete.Finch`) behind this same function. Callers should not
  call providers directly.
  """

  @doc """
  `request` is a validated map with atom keys:
  `:request_id, :language, :file_path, :prefix, :suffix,
   :repository_context, :max_tokens`.

  Returns `%{completion: binary, finish_reason: binary, model: binary}`.
  """
  def generate(request), do: Complete.MockProvider.generate(request)
end
