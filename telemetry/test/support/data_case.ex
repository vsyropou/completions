defmodule Complete.DataCase do
  @moduledoc """
  Test case template for tests that touch the database. Uses the SQL
  sandbox so each test runs in a rolled-back transaction.
  """
  use ExUnit.CaseTemplate

  using do
    quote do
      alias Complete.Repo

      import Ecto
      import Ecto.Changeset
      import Ecto.Query
      import Complete.DataCase
    end
  end

  setup tags do
    pid = Ecto.Adapters.SQL.Sandbox.start_owner!(Complete.Repo, shared: not tags[:async])
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.stop_owner(pid) end)
    :ok
  end

  @doc """
  A helper that transforms changeset errors into a map of messages.

      assert %{outcome: ["is invalid"]} = errors_on(changeset)
  """
  def errors_on(changeset) do
    Ecto.Changeset.traverse_errors(changeset, fn {message, opts} ->
      Regex.replace(~r"%{(\w+)}", message, fn _, key ->
        opts |> Keyword.get(String.to_existing_atom(key), key) |> to_string()
      end)
    end)
  end
end
