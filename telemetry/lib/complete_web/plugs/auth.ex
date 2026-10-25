defmodule CompleteWeb.Plugs.Auth do
  @moduledoc "Bearer-token auth (protocol v1.0 §5). Single shared token from config."

  @behaviour Plug
  import Plug.Conn

  def init(opts), do: opts

  def call(conn, _opts) do
    expected = Application.fetch_env!(:complete, :auth_token)

    case get_req_header(conn, "authorization") do
      ["Bearer " <> token] ->
        if Plug.Crypto.secure_compare(token, expected) do
          conn
        else
          reject(conn, "invalid bearer token")
        end

      _ ->
        reject(conn, "missing bearer token")
    end
  end

  defp reject(conn, message) do
    conn
    |> put_status(:unauthorized)
    |> Phoenix.Controller.json(%{error: %{code: "unauthorized", message: message}})
    |> halt()
  end
end
