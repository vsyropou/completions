import Config

config :complete, Complete.Repo,
  database: "telemetry_test",
  username: "complete",
  password: "complete",
  hostname: "localhost",
  pool: Ecto.Adapters.SQL.Sandbox

config :complete, CompleteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "kQrVuUo1ZmFpR2QUh5GjYjZGRlYTZkZGEzNjU4ZTY5ZjlmYzI3OGY5ZGUzZGQzYWJj",
  server: false

config :complete, :auth_token, "dev-token"

config :logger, level: :warning
