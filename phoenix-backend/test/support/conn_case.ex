defmodule CompleteWeb.ConnCase do
  @moduledoc false

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
