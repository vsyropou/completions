import Config

config :complete, Complete.Repo,
  database: "telemetry",
  username: "complete",
  password: "complete",
  hostname: "localhost",
  pool_size: 10

config :complete, CompleteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4000],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "qOoqf+m6Tf6m+fSNwdOx4dN2j/PUNIi0PHpWw6LCrnCBFuAT82wbJUucOaPUgKYR",
  watchers: []

config :phoenix, :stacktrace_depth, 20
config :phoenix, :plug_init_mode, :runtime

# Dev/test-only fallback token. Production MUST set AUTH_TOKEN.
config :complete, :auth_token, "dev-token"
