defmodule Complete.MockProvider do
  @moduledoc """
  Phase-1 deterministic mock inference. Returns a clearly fake multiline
  completion derived from the request so tests and integration can rely
  on stable output. Swapped for a real adapter in Phase 3.
  """

  @doc """
  Returns `%{completion, finish_reason, model}` for a validated request map.
  """
  def generate(%{prefix: prefix, suffix: suffix, max_tokens: max_tokens}) do
    # Deterministic: pure function of inputs.
    n = byte_size(prefix) + byte_size(suffix)

    completion =
      "  // mock completion (max_tokens=#{max_tokens})\n" <>
        "  return #{n};\n"

    %{completion: completion, finish_reason: "stop", model: "mock"}
  end
end
