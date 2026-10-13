import Config

config :complete, CompleteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "VpFmej5w6VeXKD6RSBcLHkZw6rOVtUYfz/vm7rgF2QRgeMr77MQ6vDw1VqlnZ/5e",
  server: false

config :logger, level: :warning
config :phoenix, :plug_init_mode, :runtime

config :complete, :auth_token, "dev-token"
