defmodule CompleteWeb.ConnCase do
  @moduledoc """
  Test case template for endpoint-level tests. Builds a conn via
  `Phoenix.ConnTest`, routes through `CompleteWeb.Endpoint`.
  """
  use ExUnit.CaseTemplate

  using do
    quote do
      @endpoint CompleteWeb.Endpoint

      import Plug.Conn
      import Phoenix.ConnTest
    end
  end

  setup _tags do
    {:ok, conn: Phoenix.ConnTest.build_conn()}
  end
end
