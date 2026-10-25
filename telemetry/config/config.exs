import Config

config :complete, ecto_repos: [Complete.Repo]

config :complete, CompleteWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [json: CompleteWeb.ErrorJSON],
    layout: false
  ],
  live_view: [signing_salt: "ZqF8VpqMXWy3tULh"]

config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id]

config :phoenix, :json_library, Jason

import_config "#{config_env()}.exs"
