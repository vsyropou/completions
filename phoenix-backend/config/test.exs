import Config

config :complete, CompleteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "kQrVuUo1ZmFpR2QUh5GjYjZGRlYTZkZGEzNjU4ZTY5ZjlmYzI3OGY5ZGUzZGQz",
  server: false

config :complete, :auth_token, "dev-token"

config :logger, level: :warning
