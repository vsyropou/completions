defmodule Complete.Repo do
  use Ecto.Repo,
    otp_app: :complete,
    adapter: Ecto.Adapters.Postgres
end
